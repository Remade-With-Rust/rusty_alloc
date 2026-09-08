//! Fixed-region prim backend: memory is a range someone hands us, once.
//!
//! P1 of `docs/plans/small-metal.md`. This is the backend for a target with no
//! OS at all — a microcontroller, where "memory" is a region the linker
//! reserved and there is no `mmap`, no `VirtualAlloc`, and nothing to give it
//! back to. It is the second implementation of the prim seam that P1 asks for;
//! the first is the four platform arms in [`super`].
//!
//! It is **always compiled** so that it is type-checked and unit-tested on the
//! host, and **selected** only where no platform arm matches. Nothing about a
//! host build changes because this file exists.
//!
//! Every consequence of "the region is all there is", and what each one costs:
//!
//! - **The backend cannot allocate.** It *is* the allocator's memory source, so
//!   its own bookkeeping must be a fixed static: [`MAX_EXTENTS`] free extents in
//!   two `AtomicUsize` arrays, guarded by a spin lock. That bound is a real
//!   limit — a fragmentation pattern needing more than [`MAX_EXTENTS`] holes
//!   fails the free rather than corrupting anything (see [`free`]).
//! - **`free` genuinely frees**, unlike the wasm backend: an extent returns to
//!   the list and coalesces with its neighbours. So
//!   [`super::FREE_RETURNS_MEMORY`] is true here and the arena's adopt-on-free
//!   path folds away, as it does on every platform with a working `free`.
//! - **There is no MMU**, so [`commit`] / [`decommit`] / [`reset`] are no-ops
//!   over memory that is always backed, and [`protect`] returns an error rather
//!   than pretending. That is the same call the wasm backend makes and for the
//!   same reason: a guard page that cannot trap would let a `secure` build
//!   claim a hardening it does not have.
//! - **There is no clock and one thread**, so [`clock_now`] is a monotonic
//!   counter (purge *ordering* survives; duration does not) and [`thread_id`] is
//!   a non-zero constant. TLS is a fixed static table whose destructors never
//!   run, because there is no thread exit to run them at.
//! - **Memory is not known-zero.** A `.bss` region starts zeroed, but a range
//!   handed back by [`free`] and re-served does not, and the backend cannot tell
//!   the two apart. [`alloc`] therefore reports `is_zero: false` always, which
//!   is the conservative direction: a caller that needs zeros writes them.
//!
//! **What this backend does NOT solve, and P1 does not claim it does:** the
//! allocator above it asks for `SEGMENT_SIZE`-aligned 32 MiB reservations, and
//! a chip-sized region can satisfy exactly none of them. That is §2.1 of the
//! plan, it is P2's work, and it shows up here as an honest `Err` from
//! [`alloc`] rather than as anything this file can fix.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use super::{Alloc, MemConfig, PrimError, TlsDtor, align_up};

/// Synthetic error code. The backends surface no errno, so any non-zero
/// sentinel does; this one is distinct from wasm's `0xBEEF` and the mock's.
const FERR: PrimError = 0xF13D;

/// The region is smaller than one [`FIXED_PAGE`].
pub const FERR_TOO_SMALL: PrimError = 0xF13E;

/// The region cannot hold a single `SEGMENT_SIZE` segment at the ACTIVE
/// geometry, so the allocator above this backend could never serve anything.
///
/// Almost always one missing flag: `--cfg ra_small_profile` keeps
/// `SEGMENT_SIZE` at 32 MiB, and a kilobyte-scale region yields zero segments.
pub const FERR_GEOMETRY: PrimError = 0xF13F;

/// A region is already registered; this backend takes one, once.
pub const FERR_REGISTERED: PrimError = 0xF140;

/// The smallest region this backend will accept, for a `SEGMENT_SIZE`-ALIGNED
/// base: one segment for the allocator plus one page for its heap descriptor.
///
/// Exposed so a firmware can settle its budget at COMPILE time rather than on
/// silicon, which is what the first outside adopter asked for
/// (`docs/plans/embedded-adoption.md`):
///
/// ```ignore
/// const _: () = assert!(REGION_BYTES >= rusty_alloc::prim::fixed::MIN_REGION);
/// ```
///
/// An UNALIGNED base needs up to `SEGMENT_SIZE - 1` more, because the first
/// segment can only start on a segment boundary; [`init_region`] checks the
/// real base and is therefore exact where this constant is optimistic. Align
/// the region and the two agree.
pub const MIN_REGION: usize = crate::types::SEGMENT_SIZE + FIXED_PAGE;

/// Page granularity reported to the layers above. A chip has no paging
/// hardware, so this is a bookkeeping unit rather than a hardware fact; 4 KiB
/// matches the flash/RAM block size the ESP parts use and keeps `page_align_up`
/// rounding modest on a region measured in tens of kilobytes.
const FIXED_PAGE: usize = 4096;

/// Free extents tracked at once.
///
/// The bound exists because this backend cannot allocate its own bookkeeping.
/// 32 is far more than a chip needs — the layers above make a handful of large
/// reservations, not many small ones — and exceeding it is reported, never
/// papered over.
const MAX_EXTENTS: usize = 32;

/// The region, published once by [`init_region`]. Zero length means "no region
/// yet", which every entry point checks.
static REGION_BASE: AtomicUsize = AtomicUsize::new(0);
static REGION_LEN: AtomicUsize = AtomicUsize::new(0);

/// The free list: `EXT_BASE[i] .. EXT_BASE[i] + EXT_LEN[i]`, kept sorted by
/// base so that coalescing is a look at the two neighbours. Only ever touched
/// with [`LOCK`] held, so plain `Relaxed` access is correct.
static EXT_BASE: [AtomicUsize; MAX_EXTENTS] = [const { AtomicUsize::new(0) }; MAX_EXTENTS];
static EXT_LEN: [AtomicUsize; MAX_EXTENTS] = [const { AtomicUsize::new(0) }; MAX_EXTENTS];
static EXT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Spin lock over the free list. On the single-threaded target this backend is
/// for it never contends; it is here so the statics are sound under the
/// `Sync` the seam requires, not for throughput.
static LOCK: AtomicBool = AtomicBool::new(false);

/// Guards one named lock. Not reentrant — hold at most one at a time, and
/// never call out to something that takes the same one.
struct Guard(&'static AtomicBool);

impl Guard {
    fn acquire(lock: &'static AtomicBool) -> Self {
        while lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Under `ra_single_threaded` a genuinely contended acquire is not a
            // race, because there is no second thread to race with. It can only
            // be REENTRANCY: an interrupt handler that allocated while the main
            // context was inside the allocator. That wedges forever -- the
            // preempted context can never run to release the lock -- and
            // surfaces as a watchdog reset with a backtrace pointing into
            // `spin_loop`, which names nothing.
            //
            // The LOAD is not redundant. `compare_exchange_weak` may fail
            // SPURIOUSLY, so a failed CAS is not by itself proof of anything;
            // only a lock actually observed held is. Getting this wrong would
            // panic firmwares at random, which is worse than the hang it
            // replaces.
            #[cfg(ra_single_threaded)]
            if lock.load(Ordering::Relaxed) {
                reentered();
            }
            core::hint::spin_loop();
        }
        Self(lock)
    }
}

/// The allocator was re-entered on a target that promised one context.
///
/// Separated and `#[cold]` so the happy path is unchanged: the CAS already
/// happens, and only its failure arm gains a load and a call that never
/// returns.
///
/// If the firmware's panic handler itself allocates it will re-enter here and
/// panic again, which aborts. That is a defined ending and a diagnosable one;
/// the behaviour being replaced is an unbounded spin with no message at all.
#[cfg(ra_single_threaded)]
#[cold]
#[inline(never)]
fn reentered() -> ! {
    // A literal, not a format: `core::fmt` is not on this crate's `no_std`
    // budget, and this message must survive a build that has no formatter.
    panic!(
        "rusty_alloc: the allocator was re-entered. On a target built with \
         --cfg ra_single_threaded nothing else can hold this lock, so this is \
         almost certainly an interrupt handler that allocated while the main \
         context was inside the allocator. prim::fixed's lock is NOT \
         reentrant: do not allocate in an ISR. Note that ra_single_threaded \
         means single CONTEXT, and an interrupt handler is a second context on \
         one core."
    )
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// How many bytes of `[base, base + len)` can ever back SEGMENTS.
///
/// The rule this answers used to live only in a design document
/// (`docs/plans/small-metal.md`: *"size an embedded region as
/// `k * 64 KiB + 4 KiB`, or the tail is dead to large allocations"*), which
/// meant a firmware author picking a round number learned it by reading prose
/// or not at all. A 220 KiB region at the small profile yields three segments
/// and strands 24,576 bytes — 11 % of the budget, silently.
///
/// Segments are carved from the first `SEGMENT_SIZE`-aligned address upward and
/// page-sized blocks from the top down, so the answer is
/// `floor((end - first_aligned - FIXED_PAGE) / SEGMENT_SIZE) * SEGMENT_SIZE`:
/// the leading bytes before alignment are unusable, and one page is reserved at
/// the top for the heap descriptor without which the allocator cannot start.
///
/// **This is not the same question as [`region_stats`]'s `free`.** That reports
/// bytes nobody has taken, and the stranded tail is genuinely available to
/// page-sized allocations — so it is free, and it is also useless for segments.
/// Reporting one number as if it answered both is how a clean number ends up
/// measuring nothing; they are separate on purpose.
///
/// `const fn`, so a seam crate can size its region at compile time:
///
/// ```ignore
/// const _: () = assert!(usable_bytes(0, REGION_BYTES) > 0);
/// ```
#[must_use]
pub const fn usable_bytes(base: usize, len: usize) -> usize {
    let seg = crate::types::SEGMENT_SIZE;
    let Some(end) = base.checked_add(len) else {
        return 0;
    };
    // First segment-aligned address at or above `base`, without overflowing.
    let Some(run_up) = base.checked_add(seg - 1) else {
        return 0;
    };
    let first = run_up & !(seg - 1);
    if first >= end {
        return 0;
    }
    let avail = end - first;
    // One page at the top for the heap descriptor. Without it `create_heap`
    // fails and no segment can be used even if one fits.
    if avail <= FIXED_PAGE {
        return 0;
    }
    ((avail - FIXED_PAGE) / seg) * seg
}

/// Hand the backend the region it will serve from, once.
///
/// Takes `&'static mut [u8]` because that is exactly the claim being made: the
/// range lives forever and nobody else may touch it. On a chip this is the
/// linker-reserved heap symbol; in a test it is a `static mut` array or a
/// leaked box.
///
/// Returns `Err` if a region is already registered, or if this one is too small
/// to hold anything after alignment.
///
/// # Errors
/// - [`FERR_TOO_SMALL`] — below one [`FIXED_PAGE`].
/// - [`FERR_GEOMETRY`] — cannot hold one `SEGMENT_SIZE` segment at this
///   geometry, so the allocator above could never serve an allocation. This is
///   the one that used to be accepted silently: `init_region` returned `Ok`,
///   the build was clean, and the first `Vec` on the board returned null with a
///   backtrace pointing at whatever happened to allocate first. Reported by the
///   first outside firmware to adopt 2.0.0 (`docs/plans/embedded-adoption.md`).
/// - [`FERR_REGISTERED`] — a region is already registered.
pub fn init_region(region: &'static mut [u8]) -> Result<(), PrimError> {
    let len = region.len();
    if len < FIXED_PAGE {
        return Err(FERR_TOO_SMALL);
    }
    let base = region.as_mut_ptr().expose_provenance();

    // EXACT, not conservative. `MIN_REGION` assumes a segment-aligned base; the
    // real base is in hand here, so ask the question that actually matters --
    // does an aligned segment plus a page fit inside this region? A check
    // against `len` alone would accept a region whose base sits one byte past a
    // segment boundary and still fail on the board.
    if usable_bytes(base, len) == 0 {
        return Err(FERR_GEOMETRY);
    }

    let _g = Guard::acquire(&LOCK);
    if REGION_LEN.load(Ordering::Relaxed) != 0 {
        return Err(FERR_REGISTERED);
    }
    install_region(base, len);
    Ok(())
}

/// The install half of [`init_region`], with no geometry check.
///
/// Split out for the unit tests, which exercise this backend as a plain extent
/// allocator -- first fit, coalescing, two-ended placement -- on a region far
/// smaller than a 32 MiB segment. That is a legitimate thing to test and NOT a
/// legitimate thing to ship: an allocator handed a region that cannot hold one
/// segment is dead on arrival, which is exactly what [`init_region`] now
/// refuses. Private, so the refusal has no public bypass.
fn install_region(base: usize, len: usize) {
    REGION_BASE.store(base, Ordering::Relaxed);
    REGION_LEN.store(len, Ordering::Relaxed);
    EXT_BASE[0].store(base, Ordering::Relaxed);
    EXT_LEN[0].store(len, Ordering::Relaxed);
    EXT_COUNT.store(1, Ordering::Relaxed);
}

/// The region's occupancy: `(used, free, total)` bytes.
///
/// A fixed-region allocator that cannot report how much of its region is out
/// is unmeasurable on exactly the deployment it exists for — `esp_alloc::HEAP`
/// answers `used()`/`free()` and P4 of `docs/plans/small-metal.md` compares
/// against it. `used` is derived (`total - free`) rather than counted, so it
/// cannot drift from the free list.
///
/// A snapshot: another thread could change it, though on the single-threaded
/// targets this backend serves there is no other thread.
#[must_use]
pub fn region_stats() -> (usize, usize, usize) {
    let _g = Guard::acquire(&LOCK);
    let total = REGION_LEN.load(Ordering::Relaxed);
    let free: usize = (0..EXT_COUNT.load(Ordering::Relaxed))
        .map(|i| EXT_LEN[i].load(Ordering::Relaxed))
        .sum();
    (total - free, free, total)
}

/// Remove the extent at `idx`, shifting the tail down to keep the list sorted.
fn remove_at(idx: usize) {
    let n = EXT_COUNT.load(Ordering::Relaxed);
    for i in idx..n - 1 {
        EXT_BASE[i].store(EXT_BASE[i + 1].load(Ordering::Relaxed), Ordering::Relaxed);
        EXT_LEN[i].store(EXT_LEN[i + 1].load(Ordering::Relaxed), Ordering::Relaxed);
    }
    EXT_COUNT.store(n - 1, Ordering::Relaxed);
}

/// Insert `(base, len)` at `idx`, shifting the tail up. Caller has checked
/// there is room.
fn insert_at(idx: usize, base: usize, len: usize) {
    let n = EXT_COUNT.load(Ordering::Relaxed);
    let mut i = n;
    while i > idx {
        EXT_BASE[i].store(EXT_BASE[i - 1].load(Ordering::Relaxed), Ordering::Relaxed);
        EXT_LEN[i].store(EXT_LEN[i - 1].load(Ordering::Relaxed), Ordering::Relaxed);
        i -= 1;
    }
    EXT_BASE[idx].store(base, Ordering::Relaxed);
    EXT_LEN[idx].store(len, Ordering::Relaxed);
    EXT_COUNT.store(n + 1, Ordering::Relaxed);
}

/// A slice must be at least a page.
///
/// `bins::good_size` answers the large range with `os::page_align_up`, but the
/// large path allocates EXACT SLICES. `usable_size >= good_size` — an
/// ABI-visible promise, and a proptest — therefore holds only while a slice is
/// no smaller than a page. Every other backend gets this for free (a 64 KiB
/// slice over a 4 KiB page); this is the only one where the two can be tuned
/// into conflict, so this is where it is written down. Found by dropping the
/// small profile to a 2 KiB slice: `good_size(49_153)` promised 53,248 while
/// the 25-slice span delivered 51,200.
const _: () = assert!(
    crate::types::SEGMENT_SLICE_SIZE >= FIXED_PAGE,
    "SEGMENT_SLICE_SIZE must be >= FIXED_PAGE or good_size over-promises"
);

pub(super) fn mem_init() -> MemConfig {
    MemConfig {
        page_size: FIXED_PAGE,
        alloc_granularity: FIXED_PAGE,
        large_page_size: 0,
        has_overcommit: false,
        // A sub-range can be returned independently: `free` takes any extent.
        has_partial_free: true,
    }
}

/// Where inside `[base, base + len)` a `size`-byte `align`-aligned block goes:
/// the HIGHEST such address when `from_top`, the lowest otherwise. `None` when
/// it does not fit. `align` is a power of two (it comes from a `Layout` or from
/// [`FIXED_PAGE`]), so the top-down case is a mask.
fn place(base: usize, len: usize, size: usize, align: usize, from_top: bool) -> Option<usize> {
    if size > len {
        return None;
    }
    let at = if from_top {
        (base + len - size) & !(align - 1)
    } else {
        align_up(base, align)
    };
    // Top-down can mask below `base`; bottom-up can align past the end. Compare
    // on the sum, not a subtraction that would wrap.
    if at < base || at.saturating_add(size) > base + len {
        return None;
    }
    Some(at)
}

/// Two-ended first-fit over the free list, honouring `try_alignment`.
///
/// **Coarsely-aligned requests take the bottom; merely page-aligned ones take
/// the top.** That split is the whole point on a chip-sized region. A
/// `SEGMENT_SIZE` reservation can only start on a `SEGMENT_SIZE` boundary, so
/// every byte handed out below one pushes it to the next — a single 4 KiB heap
/// block placed at the bottom of the region costs an entire segment of reach.
/// Measured on a XIAO ESP32-S3 (docs/plans/small-metal.md §2.9): bottom-only
/// placement needed a 192 KiB region for a workload whose segments and metadata
/// total 132 KiB, with 61,440 bytes sitting on the free list that no segment
/// request could ever use. Requests that do NOT care about coarse alignment are
/// the ones that can move, so they are the ones that move.
///
/// Alignment slack around the chosen placement is not lost: head and tail stay
/// on the list as their own extents, which is what makes repeated aligned
/// requests on a small region survivable at all.
///
/// # Errors
/// [`FERR`] when no region is registered, when no extent can hold
/// `size` at `try_alignment` — which is what a `SEGMENT_SIZE` request on a
/// chip-sized region does — or when splitting would need more than
/// [`MAX_EXTENTS`] entries.
pub(super) unsafe fn alloc(
    size: usize,
    try_alignment: usize,
    _commit: bool,
    _allow_large: bool,
) -> Result<Alloc, PrimError> {
    if size == 0 {
        return Err(FERR);
    }
    let align = try_alignment.max(FIXED_PAGE);
    let size = align_up(size, FIXED_PAGE);

    let _g = Guard::acquire(&LOCK);
    if REGION_LEN.load(Ordering::Relaxed) == 0 {
        return Err(FERR);
    }

    // Page-aligned requests search from the HIGHEST extent down and settle at
    // its top; coarsely-aligned ones search from the lowest up, as before.
    let from_top = align == FIXED_PAGE;
    let n = EXT_COUNT.load(Ordering::Relaxed);
    for k in 0..n {
        let i = if from_top { n - 1 - k } else { k };
        let base = EXT_BASE[i].load(Ordering::Relaxed);
        let len = EXT_LEN[i].load(Ordering::Relaxed);
        let Some(aligned) = place(base, len, size, align, from_top) else {
            continue;
        };
        let head = aligned - base;
        let tail = (base + len) - (aligned + size);

        // Splitting an extent into head + tail costs one extra entry; growing
        // the list by one must stay inside the bound, or nothing moves.
        if head > 0 && tail > 0 && n + 1 > MAX_EXTENTS {
            return Err(FERR);
        }

        remove_at(i);
        let mut at = i;
        if head > 0 {
            insert_at(at, base, head);
            at += 1;
        }
        if tail > 0 {
            insert_at(at, aligned + size, tail);
        }
        return Ok(Alloc {
            ptr: core::ptr::with_exposed_provenance_mut(aligned),
            is_large: false,
            // Conservative: a recycled extent holds whatever its last tenant
            // left. See the module doc.
            is_zero: false,
        });
    }
    Err(FERR)
}

/// Return an extent to the free list, coalescing with either neighbour.
///
/// # Errors
/// [`FERR`] if the range is not inside the registered region, or if the list is
/// full and the range touches neither neighbour. The latter is the
/// [`MAX_EXTENTS`] bound biting; it refuses rather than dropping the range.
pub(super) unsafe fn free(ptr: *mut u8, size: usize) -> Result<(), PrimError> {
    if size == 0 {
        return Ok(());
    }
    let base = ptr.expose_provenance();
    let size = align_up(size, FIXED_PAGE);

    let _g = Guard::acquire(&LOCK);
    let rbase = REGION_BASE.load(Ordering::Relaxed);
    let rlen = REGION_LEN.load(Ordering::Relaxed);
    if rlen == 0 || base < rbase || base + size > rbase + rlen {
        return Err(FERR);
    }

    let n = EXT_COUNT.load(Ordering::Relaxed);
    // Sorted insertion point: the first extent starting above `base`.
    let idx = EXT_BASE[..n]
        .iter()
        .position(|e| e.load(Ordering::Relaxed) > base)
        .unwrap_or(n);

    let prev_touches = idx > 0 && {
        let pb = EXT_BASE[idx - 1].load(Ordering::Relaxed);
        pb + EXT_LEN[idx - 1].load(Ordering::Relaxed) == base
    };
    let next_touches = idx < n && EXT_BASE[idx].load(Ordering::Relaxed) == base + size;

    match (prev_touches, next_touches) {
        // Bridges two extents: absorb both into the earlier one.
        (true, true) => {
            let grown = EXT_LEN[idx - 1].load(Ordering::Relaxed)
                + size
                + EXT_LEN[idx].load(Ordering::Relaxed);
            EXT_LEN[idx - 1].store(grown, Ordering::Relaxed);
            remove_at(idx);
        }
        (true, false) => {
            let grown = EXT_LEN[idx - 1].load(Ordering::Relaxed) + size;
            EXT_LEN[idx - 1].store(grown, Ordering::Relaxed);
        }
        (false, true) => {
            EXT_BASE[idx].store(base, Ordering::Relaxed);
            let grown = EXT_LEN[idx].load(Ordering::Relaxed) + size;
            EXT_LEN[idx].store(grown, Ordering::Relaxed);
        }
        (false, false) => {
            if n >= MAX_EXTENTS {
                return Err(FERR);
            }
            insert_at(idx, base, size);
        }
    }
    Ok(())
}

/// Always backed; reports NOT-known-zero, because [`decommit`] preserves
/// contents here.
#[allow(
    clippy::unnecessary_wraps,
    reason = "the prim backends share one signature; a no-op backend still returns the contract's Result"
)]
pub(super) unsafe fn commit(_ptr: *mut u8, _size: usize) -> Result<bool, PrimError> {
    Ok(false)
}

/// No-op. `false` = no re-commit needed, contents preserved.
#[allow(
    clippy::unnecessary_wraps,
    reason = "the prim backends share one signature; a no-op backend still returns the contract's Result"
)]
pub(super) unsafe fn decommit(_ptr: *mut u8, _size: usize) -> Result<bool, PrimError> {
    Ok(false)
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "the prim backends share one signature; a no-op backend still returns the contract's Result"
)]
pub(super) unsafe fn reset(_ptr: *mut u8, _size: usize) -> Result<(), PrimError> {
    Ok(())
}

/// No MMU. Fail loudly rather than pretend — same reasoning as the wasm arm.
pub(super) unsafe fn protect(_ptr: *mut u8, _size: usize, _on: bool) -> Result<(), PrimError> {
    Err(FERR)
}

pub(super) fn numa_node_count() -> usize {
    1
}

/// One thread, so one id. Must be non-zero: zero is the allocator's "segment is
/// abandoned" sentinel.
#[inline]
pub(super) fn thread_id() -> usize {
    1
}

/// No clock. A monotonic counter preserves purge ORDERING, which is all the
/// purge policy reads; duration does not survive.
///
/// **Two 32-bit words, not one `AtomicU64`.** The seam's return type is `u64`,
/// but this backend's whole reason to exist is a target without 64-bit
/// atomics — an `AtomicU64` here would be the one §2.2 site the port itself
/// introduced. Widening two `AtomicU32`s under their own lock keeps the full
/// range without one, and a 32-bit counter alone would wrap and invert purge
/// ordering, which is exactly the property this function exists to provide.
///
/// The lock is separate from [`LOCK`] on purpose: [`Guard`] is not reentrant,
/// and a shared lock would deadlock the moment an allocation path wanted a
/// timestamp.
static CLOCK_LOCK: AtomicBool = AtomicBool::new(false);
static TICK_LO: AtomicU32 = AtomicU32::new(0);
static TICK_HI: AtomicU32 = AtomicU32::new(0);

pub(super) fn clock_now() -> u64 {
    let _g = Guard::acquire(&CLOCK_LOCK);
    let (lo, carry) = TICK_LO.load(Ordering::Relaxed).overflowing_add(1);
    TICK_LO.store(lo, Ordering::Relaxed);
    let hi = if carry {
        let h = TICK_HI.load(Ordering::Relaxed).wrapping_add(1);
        TICK_HI.store(h, Ordering::Relaxed);
        h
    } else {
        TICK_HI.load(Ordering::Relaxed)
    };
    (u64::from(hi) << 32) | u64::from(lo)
}

/// TLS for a single-threaded world: a fixed static table. Destructors are
/// accepted and never run — there is no thread exit.
const MAX_TLS: usize = 8;
static TLS_VALUES: [AtomicUsize; MAX_TLS] = [const { AtomicUsize::new(0) }; MAX_TLS];
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

pub(super) struct TlsSlotImpl(usize);

pub(super) fn tls_new(_dtor: Option<TlsDtor>) -> Option<TlsSlotImpl> {
    let idx = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
    if idx < MAX_TLS {
        Some(TlsSlotImpl(idx))
    } else {
        None
    }
}

pub(super) fn tls_get(slot: &TlsSlotImpl) -> *mut c_void {
    core::ptr::with_exposed_provenance_mut(TLS_VALUES[slot.0].load(Ordering::Relaxed))
}

pub(super) fn tls_set(slot: &TlsSlotImpl, value: *mut c_void) {
    TLS_VALUES[slot.0].store(value.expose_provenance(), Ordering::Relaxed);
}

pub(super) fn tls_raw(slot: &TlsSlotImpl) -> usize {
    slot.0
}

pub(super) fn tls_from_raw(raw: usize) -> TlsSlotImpl {
    TlsSlotImpl(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SEGMENT_SIZE;

    /// Unlike the wasm backend, `free` here really frees — so the arena's
    /// adopt-on-free path folds away, exactly as on every platform whose free
    /// works. A `const` assertion because it is a compile-time fact.
    const _: () = assert!(super::super::FREE_RETURNS_MEMORY);

    /// The whole free list, as `(base_offset, len)` against the region base.
    fn extents() -> Vec<(usize, usize)> {
        let _g = Guard::acquire(&LOCK);
        let rbase = REGION_BASE.load(Ordering::Relaxed);
        (0..EXT_COUNT.load(Ordering::Relaxed))
            .map(|i| {
                (
                    EXT_BASE[i].load(Ordering::Relaxed) - rbase,
                    EXT_LEN[i].load(Ordering::Relaxed),
                )
            })
            .collect()
    }

    fn free_total() -> usize {
        extents().iter().map(|e| e.1).sum()
    }

    /// P1's kill test: a 512 KiB static region, served and recycled.
    ///
    /// One `#[test]` rather than several, because the free list is process-wide
    /// state that `init_region` deliberately refuses to re-initialise — so the
    /// ordering has to be explicit rather than left to the harness.
    /// P1's kill test asks for a 512 KiB region; this is it, plus ONE page.
    /// `static mut` rather than a leak so the test needs no allocator of its
    /// own — the one under test is the allocator.
    ///
    /// The odd page and the 64 KiB alignment are what make §2.9's half two
    /// able to fail. A region that is an exact multiple of `SEGMENT_SIZE`
    /// cannot tell the two placement policies apart — a page off either end
    /// costs a segment — so at 512 KiB flat the assertion would pass under the
    /// bug it exists to catch. `K * SEGMENT_SIZE + FIXED_PAGE` on an aligned
    /// base is the shape the board actually has, and the shape that
    /// discriminates.
    ///
    /// The alignment is taken at RUNTIME from an oversized backing array rather
    /// than with `#[repr(align(65536))]`, because rustc 1.97.1 on MSVC crashes
    /// (STATUS_ILLEGAL_INSTRUCTION) compiling a half-megabyte static at that
    /// alignment. Carving the window also matches how a linker script hands a
    /// chip its heap, so nothing is lost by it.
    const REGION_ALIGN: usize = 64 * 1024;
    const N: usize = 512 * 1024 + FIXED_PAGE;
    static mut BACKING: [u8; N + REGION_ALIGN] = [0; N + REGION_ALIGN];
    static mut OTHER: [u8; FIXED_PAGE] = [0; FIXED_PAGE];

    #[test]
    fn serves_and_recycles_a_static_region() {
        // The REGION_ALIGN-aligned window inside BACKING. `add` keeps the
        // array's provenance, so the slice below is a real borrow of it.
        let bp = (&raw mut BACKING).cast::<u8>();
        let skip = align_up(bp.expose_provenance(), REGION_ALIGN) - bp.expose_provenance();
        // SAFETY: `skip < REGION_ALIGN`, so `skip + N` is inside BACKING.
        let rp = unsafe { bp.add(skip) };
        // SAFETY: the only reference ever taken to REGION, handed straight to
        // init_region which requires (and consumes) exactly that exclusivity.
        let region: &'static mut [u8] = unsafe { core::slice::from_raw_parts_mut(rp, N) };

        // The negative cases FIRST: each must be refused without consuming the
        // one registration this backend accepts.
        let tiny: &'static mut [u8] = &mut [];
        assert_eq!(
            init_region(tiny),
            Err(FERR_TOO_SMALL),
            "a region below one page is refused, and says which"
        );

        // Branch on the ACTIVE geometry, because both arms are real. At the
        // shipped 32 MiB segment this 512 KiB region cannot hold one, so
        // `init_region` refuses it -- correctly, since an allocator handed it
        // would be dead on arrival -- and the extent allocator beneath, which is
        // what the rest of this test exercises, is installed directly. At the
        // small profile a 64 KiB segment fits eight times over and the public
        // entry point is used as a firmware would.
        if usable_bytes(rp.expose_provenance(), N) == 0 {
            assert_eq!(
                init_region(region),
                Err(FERR_GEOMETRY),
                "a region that cannot hold one segment is refused BEFORE the board"
            );
            install_region(rp.expose_provenance(), N);
        } else {
            init_region(region).expect("this geometry's segment fits in N");
        }
        assert_eq!(free_total(), N, "the whole region starts free");
        assert_eq!(extents().len(), 1, "as one extent");

        // A second registration is refused, and says so distinctly.
        let op = &raw mut OTHER;
        // SAFETY: as above; the call is expected to fail before it stores it.
        let other: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(op.cast::<u8>(), FIXED_PAGE) };
        let second = init_region(other);
        assert!(second.is_err(), "no second region");

        // Serve three page-aligned blocks.
        // SAFETY: the prim contract — sizes are page multiples, alignment a
        // power of two.
        let (a, b, c) = unsafe {
            (
                alloc(64 * 1024, FIXED_PAGE, true, false).expect("a"),
                alloc(128 * 1024, FIXED_PAGE, true, false).expect("b"),
                alloc(64 * 1024, FIXED_PAGE, true, false).expect("c"),
            )
        };
        assert_eq!(free_total(), N - 256 * 1024, "three blocks are out");
        assert!(!a.is_zero, "recycled memory is never claimed zero");

        // Blocks are distinct, in the region, and do not overlap.
        let base = REGION_BASE.load(Ordering::Relaxed);
        for (p, len) in [(a.ptr, 64 * 1024), (b.ptr, 128 * 1024), (c.ptr, 64 * 1024)] {
            let off = p.expose_provenance() - base;
            assert!(off + len <= N, "block lies inside the region");
        }
        assert_ne!(a.ptr, b.ptr);
        assert_ne!(b.ptr, c.ptr);

        // Write a pattern through each and read it back: the region is real
        // memory, not just bookkeeping.
        for (p, len, tag) in [(a.ptr, 64 * 1024, 0xA5u8), (b.ptr, 128 * 1024, 0x5Au8)] {
            // SAFETY: `p` is a live block of `len` bytes from `alloc` above.
            unsafe {
                core::ptr::write_bytes(p, tag, len);
                assert_eq!(*p, tag);
                assert_eq!(*p.add(len - 1), tag);
            }
        }

        // Free the middle block: it becomes its own extent, no coalescing.
        let holes = extents().len();
        // SAFETY: `b` came from `alloc` and is unfreed.
        unsafe { free(b.ptr, 128 * 1024).expect("free b") };
        assert_eq!(free_total(), N - 128 * 1024);
        assert_eq!(extents().len(), holes + 1, "an isolated hole");

        // Free its neighbours: everything coalesces back to one extent.
        // SAFETY: both came from `alloc` and are unfreed.
        unsafe {
            free(a.ptr, 64 * 1024).expect("free a");
            free(c.ptr, 64 * 1024).expect("free c");
        }
        assert_eq!(free_total(), N, "the whole region is back");
        assert_eq!(extents().len(), 1, "coalesced into one extent");

        // The region is reusable: the same 256 KiB can be served again.
        // SAFETY: prim contract, as above.
        let d = unsafe { alloc(256 * 1024, FIXED_PAGE, true, false).expect("d") };
        assert_eq!(free_total(), N - 256 * 1024);
        // SAFETY: `d` is live and unfreed.
        unsafe { free(d.ptr, 256 * 1024).expect("free d") };
        assert_eq!(free_total(), N);

        // ---- §2.1, both sides ----
        //
        // This lives HERE, and not in a test of its own, because the free list
        // is process-wide and the harness orders tests arbitrarily: standalone,
        // it passed while NO region was registered, i.e. for the trivial reason
        // rather than the interesting one. Measured, not assumed.
        //
        // P1 wrote this as a one-sided refusal, because at the shipped geometry
        // a segment cannot come out of a chip-sized region. P2 made the
        // geometry a parameter, so the property is now two-sided and says
        // something either way — which is the point of having kept it.
        assert_eq!(free_total(), N, "the whole region is free before this");
        // SAFETY: prim contract; SEGMENT_SIZE is a power of two.
        let seg = unsafe { alloc(SEGMENT_SIZE, SEGMENT_SIZE, true, false) };
        if SEGMENT_SIZE > N {
            // The shipped 32 MiB geometry: 512 KiB cannot hold a segment, and
            // the refusal must cost nothing.
            assert!(
                seg.is_err(),
                "a {SEGMENT_SIZE}-byte segment cannot come out of a {N}-byte region"
            );
            assert_eq!(
                free_total(),
                N,
                "a refused request leaves the list untouched"
            );

            // Nor can the ALIGNMENT be met — the half no larger region fixes.
            // SAFETY: prim contract, as above.
            let al = unsafe { alloc(FIXED_PAGE, SEGMENT_SIZE, true, false) };
            assert!(
                al.is_err(),
                "SEGMENT_SIZE alignment is unsatisfiable in a region smaller than it"
            );
        } else {
            // The small profile: this is what P2 bought. A whole segment, at
            // segment alignment, served from a chip-sized region.
            let a = seg.expect("a segment must fit once the geometry allows it");
            assert_eq!(
                a.ptr.expose_provenance() % SEGMENT_SIZE,
                0,
                "a segment must be SEGMENT_SIZE-aligned — `segment_of` masks on it"
            );
            // `saturating_sub`: the compiler const-evaluates this arm even when
            // the branch is dead, and at the shipped geometry SEGMENT_SIZE > N.
            assert_eq!(free_total(), N.saturating_sub(SEGMENT_SIZE));
            // SAFETY: `a` is live and unfreed.
            unsafe { free(a.ptr, SEGMENT_SIZE).expect("free the segment") };
        }
        // Either way the region ends whole: a refusal consumed nothing, and a
        // served segment was handed back.
        assert_eq!(free_total(), N);

        // ---- §2.9, the two-ended placement, both halves ----
        //
        // Also here rather than standalone, for the same process-wide-state
        // reason as §2.1 above.
        //
        // HALF ONE, the mechanism: a merely page-aligned request goes to the
        // TOP, leaving the low end of the region contiguous. Under the old
        // bottom-only first-fit this offset was 0 and the surviving extent
        // started at FIXED_PAGE — which is exactly how a 4 KiB heap block used
        // to cost a whole segment of reach.
        // SAFETY: prim contract — a page multiple at a power-of-two alignment.
        let top = unsafe { alloc(FIXED_PAGE, FIXED_PAGE, true, false).expect("top") };
        assert_eq!(
            top.ptr.expose_provenance() - REGION_BASE.load(Ordering::Relaxed),
            N - FIXED_PAGE,
            "a page-aligned request is placed at the top of the region"
        );
        assert_eq!(
            extents(),
            vec![(0, N - FIXED_PAGE)],
            "and leaves the low end as ONE contiguous extent"
        );
        // SAFETY: `top` is live and unfreed.
        unsafe { free(top.ptr, FIXED_PAGE).expect("free top") };
        assert_eq!(free_total(), N);

        // HALF TWO, the consequence that was actually measured: taking that
        // page must not cost a single SEGMENT_SIZE-aligned segment. Counted
        // both ways rather than asserted, so the test says what it means at
        // whichever geometry it is compiled for (at the shipped 32 MiB one
        // both counts are 0, and the equality still holds honestly).
        let clean = greedy_segments();
        assert_eq!(free_total(), N, "counting segments leaves the region whole");
        // SAFETY: prim contract, as above.
        let hdr = unsafe { alloc(FIXED_PAGE, FIXED_PAGE, true, false).expect("hdr") };
        let with_hdr = greedy_segments();
        assert_eq!(
            with_hdr, clean,
            "a page-sized block must not cost a whole segment of reach"
        );
        // SAFETY: `hdr` is live and unfreed.
        unsafe { free(hdr.ptr, FIXED_PAGE).expect("free hdr") };
        assert_eq!(free_total(), N, "and the region ends whole");
    }

    /// Serve `SEGMENT_SIZE`-aligned segments until the region refuses, then
    /// hand them all back. Returns how many it managed — the region's segment
    /// *reach*, which is the quantity §2.9's placement rule protects.
    fn greedy_segments() -> usize {
        let mut held = Vec::new();
        // SAFETY: prim contract — SEGMENT_SIZE is a power of two, and every
        // pointer collected here is freed below before the function returns.
        while let Ok(a) = unsafe { alloc(SEGMENT_SIZE, SEGMENT_SIZE, true, false) } {
            held.push(a.ptr);
        }
        let n = held.len();
        for p in held {
            // SAFETY: each `p` came from the `alloc` above and is unfreed.
            unsafe { free(p, SEGMENT_SIZE).expect("free a counted segment") };
        }
        n
    }

    /// `usable_bytes` is pure arithmetic, so it gets its own test with no
    /// global state -- and the case that motivated it, from the first outside
    /// adopter's report.
    #[test]
    fn usable_bytes_answers_the_question_a_firmware_asks() {
        let seg = SEGMENT_SIZE;

        // Aligned base: MIN_REGION is exactly enough for one segment, and one
        // byte less is not.
        assert_eq!(
            usable_bytes(0, MIN_REGION),
            seg,
            "MIN_REGION buys a segment"
        );
        assert_eq!(
            usable_bytes(0, MIN_REGION - 1),
            0,
            "one byte short buys none"
        );

        // The page at the top is not optional: a region of exactly one segment
        // has nowhere to put the heap descriptor, so it can serve nothing.
        assert_eq!(
            usable_bytes(0, seg),
            0,
            "a segment with no page for the heap is unusable"
        );

        // An UNALIGNED base loses the run-up. This is why `init_region` checks
        // the real base rather than comparing `len` against `MIN_REGION`: this
        // region is >= MIN_REGION and still yields nothing.
        assert_eq!(
            usable_bytes(FIXED_PAGE, MIN_REGION),
            0,
            "unaligned base eats the segment"
        );
        assert_eq!(
            usable_bytes(FIXED_PAGE, MIN_REGION + seg),
            seg,
            "one more segment of slack absorbs the misalignment"
        );

        // The stranded tail, which used to be recorded only in a design doc.
        // Sized in segments so it says the same thing at either geometry; at
        // the small profile this is the report's 220 KiB case exactly.
        let three_and_a_bit = 3 * seg + FIXED_PAGE + seg / 2;
        assert_eq!(
            usable_bytes(0, three_and_a_bit),
            3 * seg,
            "a ragged region yields whole segments and strands the remainder"
        );
        let stranded = three_and_a_bit - usable_bytes(0, three_and_a_bit) - FIXED_PAGE;
        assert_eq!(
            stranded,
            seg / 2,
            "and the strand is exactly the ragged part"
        );
    }

    /// The reentrancy detector, watched firing.
    ///
    /// "A failure mode nobody has watched fire is a claim, not a defence" is
    /// this repo's own line, and it applies to the thing that replaced the
    /// hang. Only compiled where the detector is: run it with
    /// `RUSTFLAGS="--cfg ra_single_threaded" cargo test -p rusty_alloc --lib prim::fixed`,
    /// which CI does.
    ///
    /// Deliberately NOT in `tools/gate-selftest.sh`: poisoning this gate
    /// removes the detector, and the test then HANGS instead of failing --
    /// which is the whole point of the defect, and useless in a CI job. The
    /// evidence that it fires is this test passing where the detector exists
    /// and the code not compiling it where it does not.
    #[cfg(ra_single_threaded)]
    #[test]
    #[should_panic(expected = "re-entered")]
    fn a_reentrant_acquire_is_diagnosed_not_hung() {
        static LOCK2: AtomicBool = AtomicBool::new(false);
        let _outer = Guard::acquire(&LOCK2);
        // Exactly what an allocating ISR does: acquire while the outer context
        // still holds it. Without the detector this line never returns.
        let _inner = Guard::acquire(&LOCK2);
    }

    /// The no-MMU decisions, pinned so a future edit has to mean it.
    #[test]
    fn no_mmu_semantics_are_explicit() {
        let cfg = mem_init();
        assert_eq!(cfg.page_size, FIXED_PAGE);
        assert_eq!(cfg.large_page_size, 0, "no large pages without an MMU");
        assert!(!cfg.has_overcommit, "nothing to overcommit");
        assert!(cfg.has_partial_free, "any extent can be returned");
        assert_ne!(thread_id(), 0, "zero is the abandoned-segment sentinel");
        assert_eq!(numa_node_count(), 1);
        // A monotonic counter, not a clock.
        assert!(clock_now() < clock_now());
        // SAFETY: `protect` on this backend inspects nothing and always fails;
        // it never dereferences the pointer, so a null one is in contract.
        let p = unsafe { protect(core::ptr::null_mut(), FIXED_PAGE, true) };
        assert!(
            p.is_err(),
            "a guard page that cannot trap must not report success"
        );
    }
}
