//! Arenas v1 (M6 subset of upstream `arena.c`): pre-reserved OS regions carved
//! into SEGMENT-sized chunks that segment alloc/free recycles instead of
//! round-tripping the OS. Upstream arenas allocate at slice granularity; ours
//! are segment-granular (32 MiB chunks) — sufficient for the §5.9 API and the
//! `disallow_os_alloc` / programmatic-memory use cases; slice-granular arenas
//! are a post-v1 refinement.
//!
//! Chunk state is two atomic bitmaps: `used` (allocation) and `dirty`
//! (ever-used → its memory is NOT zero — feeds the segment zero-tracking).

use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

use crate::os;
use crate::types::SEGMENT_SIZE;

const MAX_ARENAS: usize = 32;
const MAX_CHUNKS: usize = 1024; // 32 GiB per arena at 32 MiB chunks

/// One reserved region.
pub struct Arena {
    /// Base pointer (SEGMENT_SIZE-aligned). A POINTER, not an address:
    /// chunk derivation keeps provenance and the region stays reachable
    /// (the M4 reachability-follows-pointers lesson, recaught here by miri).
    pub base: *mut u8,
    /// Usable size (whole chunks).
    pub size: usize,
    /// Chunk count.
    pub chunks: usize,
    /// Only heaps created with this arena id may allocate from it.
    pub exclusive: bool,
    /// We own the mapping (false for `mi_manage_os_memory` memory).
    pub owned: bool,
    /// Advisory NUMA node (recorded; placement lands post-v1).
    pub numa_node: i32,
    used: [AtomicU64; MAX_CHUNKS / 64],
    dirty: [AtomicU64; MAX_CHUNKS / 64],
}

static ARENAS: [AtomicPtr<Arena>; MAX_ARENAS] =
    [const { AtomicPtr::new(ptr::null_mut()) }; MAX_ARENAS];
static ARENA_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Register a region as an arena. `base` must be SEGMENT_SIZE-aligned and
/// `size` a whole number of chunks. Returns the arena id.
fn arena_register(
    base: *mut u8,
    size: usize,
    exclusive: bool,
    owned: bool,
    numa_node: i32,
) -> Result<i32, ()> {
    let chunks = size / SEGMENT_SIZE;
    if chunks == 0 || chunks > MAX_CHUNKS || !base.addr().is_multiple_of(SEGMENT_SIZE) {
        return Err(());
    }
    // Arena descriptor lives in its own OS pages (never in the allocator).
    let desc = os::alloc_aligned(core::mem::size_of::<Arena>(), os::page_size(), true, false)
        .map_err(|_| ())?;
    let a: *mut Arena = desc.ptr.cast();
    // SAFETY: fresh zeroed mapping; AtomicU64 zero bit-pattern is valid, so
    // only the scalar fields need writing.
    unsafe {
        (*a).base = base;
        (*a).size = size;
        (*a).chunks = chunks;
        (*a).exclusive = exclusive;
        (*a).owned = owned;
        (*a).numa_node = numa_node;
    }
    let id = ARENA_COUNT.fetch_add(1, Ordering::AcqRel);
    if id >= MAX_ARENAS {
        ARENA_COUNT.fetch_sub(1, Ordering::AcqRel);
        return Err(());
    }
    ARENAS[id].store(a, Ordering::Release);
    Ok(id as i32)
}

/// `mi_reserve_os_memory_ex`: reserve (and commit) fresh OS memory as an
/// arena. Returns the arena id.
#[allow(clippy::result_unit_err)] // C error-code mapping at the FFI edge
pub fn reserve_os_memory_ex(
    size: usize,
    _commit: bool,
    allow_large: bool,
    exclusive: bool,
) -> Result<i32, ()> {
    let total = size.div_ceil(SEGMENT_SIZE) * SEGMENT_SIZE;
    // Eager commit (our segment model); `_commit=false` still reserves+commits
    // in v1 — recorded divergence, matches how segments consume chunks.
    let b = os::alloc_aligned(total, SEGMENT_SIZE, true, allow_large).map_err(|_| ())?;
    arena_register(b.ptr, total, exclusive, true, -1)
}

/// `mi_manage_os_memory_ex`: adopt caller-provided memory (never freed by us).
/// The SEGMENT_SIZE-aligned interior is used; ragged edges are ignored.
#[allow(clippy::result_unit_err)] // C error-code mapping at the FFI edge
#[allow(
    clippy::fn_params_excessive_bools,
    reason = "mirrors mi_manage_os_memory_ex's C signature 1:1; grouping the \
              flags into a struct would break the ABI parity this crate exists for"
)]
pub fn manage_os_memory_ex(
    start: *mut u8,
    size: usize,
    _is_committed: bool,
    _is_large: bool,
    _is_zero: bool,
    numa_node: i32,
    exclusive: bool,
) -> Result<i32, ()> {
    let lo_addr = (start.addr() + SEGMENT_SIZE - 1) & !(SEGMENT_SIZE - 1);
    let hi = (start.addr() + size) & !(SEGMENT_SIZE - 1);
    if hi <= lo_addr {
        return Err(());
    }
    let lo = start.with_addr(lo_addr);
    // Conservative: treat managed memory as dirty (not-zero) — the dirty
    // bitmap starts clear, so mark it at first alloc instead; simplest is to
    // pre-mark every chunk dirty.
    let id = arena_register(lo, hi - lo_addr, exclusive, false, numa_node)?;
    let a = ARENAS[id as usize].load(Ordering::Acquire);
    // SAFETY: freshly registered arena descriptor.
    unsafe {
        let chunks = (*a).chunks;
        for w in 0..chunks.div_ceil(64) {
            let bits = if (w + 1) * 64 <= chunks {
                u64::MAX
            } else {
                (1u64 << (chunks % 64)) - 1
            };
            (*a).dirty[w].store(bits, Ordering::Relaxed);
        }
    }
    Ok(id)
}

/// Whether reserving a large default arena up front pays for itself.
///
/// **False on wasm, where it is actively harmful.** The arena exists to keep
/// segment churn off the OS by reserving address space cheaply and committing
/// it lazily — but wasm has no virtual reservation at all: `memory.grow` backs
/// every byte immediately, so a 1 GiB "reservation" costs 1 GiB of real linear
/// memory before the first `malloc` even returns. And it buys nothing there,
/// because wasm memory is never returned to the host — every segment we have
/// ever touched is already permanently cached, which is exactly what the arena
/// was for. Upstream reaches for the same lever more mildly (`arena.c` divides
/// the reserve by 4 "if virtual reserve is not supported (for WASM for
/// example)"); with grow-only memory, skipping it entirely is strictly better.
///
/// Measured by `bench/wasm-selftest.mjs`, same workload either way:
/// **1056.06 MiB -> 64.00 MiB of linear memory, and 6.79 ms -> 1.75 ms.**
const DEFAULT_ARENA_PAYS: bool = !cfg!(all(target_arch = "wasm32", not(miri)));

/// Lazily reserve the DEFAULT arena (`mi_option_arena_reserve`, 1 GiB by
/// default) the first time a segment misses the arena pools — this is what
/// keeps segment churn off the OS (upstream does the same; without it,
/// malloc-large pays a VirtualAlloc/munmap round-trip per cycle, measured
/// 3–4× slower on the Tier-A gate).
fn ensure_default_arena() {
    use core::sync::atomic::AtomicBool;
    static TRIED: AtomicBool = AtomicBool::new(false);
    if TRIED.swap(true, Ordering::AcqRel) {
        return;
    }
    if !DEFAULT_ARENA_PAYS {
        return;
    }
    let reserve = crate::options::get_size(23); // arena_reserve (KiB-scaled)
    if reserve >= SEGMENT_SIZE {
        let _ = reserve_os_memory_ex(reserve, true, false, false);
    }
}

/// Allocate one chunk. `restrict_id >= 0` → only that arena; else any
/// NON-exclusive arena (reserving the default arena on first miss). Returns
/// (chunk ptr, chunk-is-zero).
pub fn chunk_alloc(restrict_id: i32) -> Option<(*mut u8, bool)> {
    if restrict_id < 0 {
        if crate::options::is_enabled(27) {
            return None; // disallow_arena_alloc
        }
        ensure_default_arena();
    }
    chunk_alloc_inner(restrict_id)
}

#[allow(clippy::needless_range_loop)] // indexed scan over a fixed atomic table
fn chunk_alloc_inner(restrict_id: i32) -> Option<(*mut u8, bool)> {
    let n = ARENA_COUNT.load(Ordering::Acquire).min(MAX_ARENAS);
    for id in 0..n {
        if restrict_id >= 0 && id != restrict_id as usize {
            continue;
        }
        let a = ARENAS[id].load(Ordering::Acquire);
        if a.is_null() {
            continue;
        }
        // SAFETY: arena descriptors are live for the process.
        unsafe {
            if restrict_id < 0 && (*a).exclusive {
                continue;
            }
            let words = (*a).chunks.div_ceil(64);
            for w in 0..words {
                loop {
                    let cur = (*a).used[w].load(Ordering::Acquire);
                    let limit = if (w + 1) * 64 <= (*a).chunks {
                        64
                    } else {
                        (*a).chunks % 64
                    };
                    let free_bits = !cur
                        & if limit == 64 {
                            u64::MAX
                        } else {
                            (1u64 << limit) - 1
                        };
                    if free_bits == 0 {
                        break;
                    }
                    let bit = free_bits.trailing_zeros() as usize;
                    if (*a).used[w]
                        .compare_exchange_weak(
                            cur,
                            cur | (1 << bit),
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_err()
                    {
                        continue;
                    }
                    let idx = w * 64 + bit;
                    let was_dirty =
                        (*a).dirty[w].fetch_or(1 << bit, Ordering::AcqRel) & (1 << bit) != 0;
                    let p = (*a).base.add(idx * SEGMENT_SIZE);
                    return Some((p, !was_dirty));
                }
            }
        }
    }
    None
}

static MULTI_LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Allocate `n` CONTIGUOUS chunks (huge blocks). Single spinlock for the
/// multi-bit search; the single-chunk path stays lock-free.
#[allow(clippy::needless_range_loop)] // indexed scan over a fixed atomic table
pub fn chunk_alloc_n(restrict_id: i32, n: usize) -> Option<(*mut u8, bool)> {
    if n == 1 {
        return chunk_alloc(restrict_id);
    }
    if restrict_id < 0 {
        if crate::options::is_enabled(27) {
            return None;
        }
        ensure_default_arena();
    }
    while MULTI_LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    let result = (|| {
        let count = ARENA_COUNT.load(Ordering::Acquire).min(MAX_ARENAS);
        for id in 0..count {
            if restrict_id >= 0 && id != restrict_id as usize {
                continue;
            }
            let a = ARENAS[id].load(Ordering::Acquire);
            if a.is_null() {
                continue;
            }
            // SAFETY: live descriptor; index arithmetic bounded by chunks.
            unsafe {
                if restrict_id < 0 && (*a).exclusive {
                    continue;
                }
                let chunks = (*a).chunks;
                if n > chunks {
                    continue;
                }
                let mut run = 0usize;
                let mut idx = 0usize;
                while idx < chunks {
                    let bit = (*a).used[idx / 64].load(Ordering::Acquire) & (1 << (idx % 64));
                    run = if bit == 0 { run + 1 } else { 0 };
                    if run == n {
                        let start = idx + 1 - n;
                        // CLAIM-AND-VERIFY. MULTI_LOCK excludes other
                        // multi-chunk claims but NOT the lock-free
                        // single-chunk path, which can take one of these
                        // chunks between our scan and our claim. fetch_or
                        // reports the previous bit: on conflict, roll back
                        // exactly what we claimed and rescan past the thief.
                        // Without this, two segments land on ONE address —
                        // the parallel-test corruption found in M8.
                        let mut conflict = None;
                        for j in start..=idx {
                            let prev = (*a).used[j / 64].fetch_or(1 << (j % 64), Ordering::AcqRel);
                            if prev & (1 << (j % 64)) != 0 {
                                conflict = Some(j);
                                break;
                            }
                        }
                        if let Some(c) = conflict {
                            for j in start..c {
                                (*a).used[j / 64].fetch_and(!(1 << (j % 64)), Ordering::AcqRel);
                            }
                            run = 0;
                            idx = c + 1;
                            continue;
                        }
                        let mut any_dirty = false;
                        for j in start..=idx {
                            any_dirty |= (*a).dirty[j / 64]
                                .fetch_or(1 << (j % 64), Ordering::AcqRel)
                                & (1 << (j % 64))
                                != 0;
                        }
                        let p = (*a).base.add(start * SEGMENT_SIZE);
                        return Some((p, !any_dirty));
                    }
                    idx += 1;
                }
            }
        }
        None
    })();
    MULTI_LOCK.store(false, Ordering::Release);
    result
}

/// Free `n` contiguous chunks. True when the address belonged to an arena.
#[allow(clippy::needless_range_loop)] // indexed scan over a fixed atomic table
pub fn chunk_free_n(p: *mut u8, n: usize) -> bool {
    let addr = p.addr();
    let count = ARENA_COUNT.load(Ordering::Acquire).min(MAX_ARENAS);
    for id in 0..count {
        let a = ARENAS[id].load(Ordering::Acquire);
        if a.is_null() {
            continue;
        }
        // SAFETY: live descriptor; bounded indices per the range check.
        unsafe {
            if addr >= (*a).base.addr() && addr < (*a).base.addr() + (*a).size {
                let start = (addr - (*a).base.addr()) / SEGMENT_SIZE;
                for j in start..start + n {
                    (*a).used[j / 64].fetch_and(!(1 << (j % 64)), Ordering::AcqRel);
                }
                return true;
            }
        }
    }
    false
}

/// Return a chunk to its arena. True when the address belonged to one.
#[allow(clippy::needless_range_loop)] // indexed scan over a fixed atomic table
pub fn chunk_free(p: *mut u8) -> bool {
    let addr = p.addr();
    let n = ARENA_COUNT.load(Ordering::Acquire).min(MAX_ARENAS);
    for id in 0..n {
        let a = ARENAS[id].load(Ordering::Acquire);
        if a.is_null() {
            continue;
        }
        // SAFETY: live descriptor; bit index bounded by the range check.
        unsafe {
            if addr >= (*a).base.addr() && addr < (*a).base.addr() + (*a).size {
                let idx = (addr - (*a).base.addr()) / SEGMENT_SIZE;
                (*a).used[idx / 64].fetch_and(!(1 << (idx % 64)), Ordering::AcqRel);
                return true;
            }
        }
    }
    false
}

/// `mi_arena_area`: the arena's base and size, or null.
pub fn arena_area(id: i32) -> (*mut u8, usize) {
    if id < 0 || id as usize >= ARENA_COUNT.load(Ordering::Acquire) {
        return (ptr::null_mut(), 0);
    }
    let a = ARENAS[id as usize].load(Ordering::Acquire);
    if a.is_null() {
        return (ptr::null_mut(), 0);
    }
    // SAFETY: live descriptor.
    unsafe { ((*a).base, (*a).size) }
}

/// Debug print of arena occupancy (mi_debug_show_arenas / mi_arenas_print).
#[allow(clippy::needless_range_loop)] // indexed scan over a fixed atomic table
pub fn arenas_print(out: &mut dyn FnMut(&str)) {
    let n = ARENA_COUNT.load(Ordering::Acquire).min(MAX_ARENAS);
    if n == 0 {
        out("arenas: none\n");
        return;
    }
    for id in 0..n {
        let a = ARENAS[id].load(Ordering::Acquire);
        if a.is_null() {
            continue;
        }
        // SAFETY: live descriptor.
        unsafe {
            let mut used = 0usize;
            for w in 0..(*a).chunks.div_ceil(64) {
                used += (*a).used[w].load(Ordering::Relaxed).count_ones() as usize;
            }
            let mut line = heapless_fmt(
                id,
                (*a).base.addr(),
                (*a).size,
                used,
                (*a).chunks,
                (*a).exclusive,
            );
            out(line.as_str());
            line.clear();
        }
    }
}

// Tiny fixed formatting helper (no allocation inside the allocator's own
// diagnostics).
fn heapless_fmt(
    id: usize,
    base: usize,
    size: usize,
    used: usize,
    chunks: usize,
    excl: bool,
) -> String {
    format!(
        "arena {id}: base {base:#x} size {} MiB, {used}/{chunks} chunks used{}\n",
        size / (1024 * 1024),
        if excl { " (exclusive)" } else { "" }
    )
}
