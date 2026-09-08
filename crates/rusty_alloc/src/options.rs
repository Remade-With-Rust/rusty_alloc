//! Options table, environment parsing, and the registered hooks (mirrors
//! `options.c`). Option INDICES are ABI: the enum matches the oracle v2.4.5
//! ordering exactly, deprecated slots included.
//!
//! Env: `MIMALLOC_<NAME>` (compat) and `RUSTY_ALLOC_<NAME>` (ours) — e.g.
//! `MIMALLOC_SHOW_STATS=1`, `MIMALLOC_PURGE_DELAY=0`. Parsed once on first
//! option access. Values follow mimalloc: booleans accept 1/0/true/false/
//! yes/no/on/off; sizes are plain integers (`_size` options are KiB).

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

// The only two 64-bit atomics left in the crate, and the only two whose width
// is a CONTRACT rather than a choice (P3 of `docs/plans/small-metal.md`):
// `VALUES` backs `options::{get,set}`, which are `i64` in an API frozen at
// v2.0.0, and `HEARTBEAT` is handed to a registered `DeferredFreeFun` whose C
// ABI declares it `u64`. Every other 64-bit atomic in the crate was a bitmap
// and was narrowed to `u32` instead. On a target with the real thing this is
// `core`; only 32-bit RISC-V / Xtensa pull the shim, and only there.
#[cfg(target_has_atomic = "64")]
use core::sync::atomic::{AtomicI64, AtomicU64};
// With `std` on a 32-bit target, threads are real and the shim's lock table is
// the honest answer. Without it, the crate already serves `no_std` only on
// single-threaded targets — that is what `lib.rs`'s `SingleThreadCell` rests on
// — so paying `portable_atomic`'s lock table (measured at **4,288 bytes of
// BSS** in a shipped XIAO ESP32-S3 firmware, larger than the heap descriptor
// the allocator starts with) buys atomicity nothing can observe. Two `u32`
// halves cost 8 bytes and no lock. See `split64` below.
#[cfg(all(not(target_has_atomic = "64"), feature = "std"))]
use portable_atomic::{AtomicI64, AtomicU64};
#[cfg(all(not(target_has_atomic = "64"), not(feature = "std")))]
use split64::{AtomicI64, AtomicU64};

/// A 64-bit atomic as two `AtomicU32` halves, for single-threaded `no_std`.
///
/// **Sound only because the crate is single-threaded wherever it is used** —
/// the same standing assumption as `lib.rs`'s `SingleThreadCell`, `prim::fixed`'s
/// constant thread id, and its never-contended spin lock. A `no_std` build on a
/// target that grows threads must revisit all four together. Nothing here is
/// `unsafe`: a struct of `AtomicU32` is `Sync` already, so this adds no unsafe
/// to the crate.
///
/// Only the four operations `options.rs` actually performs are provided; a
/// fifth would need its own thought about which half moves first.
///
/// **Orderings are normalised, not forwarded.** A caller's `Ordering` describes
/// one 64-bit access; this performs two 32-bit ones, so there is nothing
/// faithful to forward it to. Forwarding is also a panic: `AtomicU32::load`
/// rejects `Release`/`AcqRel` and `store` rejects `Acquire`/`AcqRel`, so the
/// `compare_exchange(.., AcqRel, ..)` that `set_default` performs would abort
/// the firmware. Loads use `Acquire` and stores `Release` — valid for every
/// caller, and stronger than a single-threaded target can observe.
// `test` in the cfg so the module COMPILES AND ITS TEST RUNS on the host. Gated
// only on the target that uses it, the test below would never execute anywhere
// CI or a developer runs — and a test that cannot run is worse than no test,
// because it looks like coverage.
#[cfg(any(all(not(target_has_atomic = "64"), not(feature = "std")), test))]
mod split64 {
    use core::sync::atomic::{AtomicU32, Ordering};

    /// Split a `u64` into `(lo, hi)` and back. Free-standing so both wrappers
    /// share one definition of which half is which.
    const fn split(v: u64) -> (u32, u32) {
        (v as u32, (v >> 32) as u32)
    }
    const fn join(lo: u32, hi: u32) -> u64 {
        ((hi as u64) << 32) | lo as u64
    }

    #[derive(Debug)]
    pub struct AtomicU64 {
        lo: AtomicU32,
        hi: AtomicU32,
    }

    impl AtomicU64 {
        pub const fn new(v: u64) -> Self {
            let (lo, hi) = split(v);
            Self {
                lo: AtomicU32::new(lo),
                hi: AtomicU32::new(hi),
            }
        }
        pub fn load(&self, _ord: Ordering) -> u64 {
            join(
                self.lo.load(Ordering::Acquire),
                self.hi.load(Ordering::Acquire),
            )
        }
        pub fn store(&self, v: u64, _ord: Ordering) {
            let (lo, hi) = split(v);
            self.lo.store(lo, Ordering::Release);
            self.hi.store(hi, Ordering::Release);
        }
        pub fn fetch_add(&self, v: u64, ord: Ordering) -> u64 {
            let prev = self.load(ord);
            self.store(prev.wrapping_add(v), ord);
            prev
        }
    }

    /// The signed half of the same thing: options are `i64` in an API frozen at
    /// v2.0.0, and the bit pattern round-trips exactly.
    #[derive(Debug)]
    pub struct AtomicI64(AtomicU64);

    impl AtomicI64 {
        pub const fn new(v: i64) -> Self {
            Self(AtomicU64::new(v as u64))
        }
        pub fn load(&self, ord: Ordering) -> i64 {
            self.0.load(ord) as i64
        }
        pub fn store(&self, v: i64, ord: Ordering) {
            self.0.store(v as u64, ord);
        }
        /// `Ordering` pair mirrors `core`'s signature; single-threaded, so the
        /// read-compare-write cannot be interleaved.
        pub fn compare_exchange(
            &self,
            current: i64,
            new: i64,
            success: Ordering,
            _failure: Ordering,
        ) -> Result<i64, i64> {
            let seen = self.load(success);
            if seen == current {
                self.store(new, success);
                Ok(seen)
            } else {
                Err(seen)
            }
        }
    }

    /// The orderings `options.rs` actually passes, exercised so a future caller
    /// forwarding `AcqRel` cannot reintroduce the panic the module doc names.
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_ordering_options_uses_is_accepted() {
            let v = AtomicI64::new(i64::MIN);
            v.store(-1, Ordering::Release);
            assert_eq!(v.load(Ordering::Acquire), -1);
            // `set_default`'s ordering pair — the one that would abort.
            assert_eq!(
                v.compare_exchange(-1, 7, Ordering::AcqRel, Ordering::Acquire),
                Ok(-1)
            );
            assert_eq!(v.load(Ordering::Acquire), 7);
            assert_eq!(
                v.compare_exchange(-1, 9, Ordering::AcqRel, Ordering::Acquire),
                Err(7)
            );

            // Halves join in the right order across the 32-bit boundary.
            let u = AtomicU64::new(u64::from(u32::MAX));
            assert_eq!(u.fetch_add(1, Ordering::Relaxed), u64::from(u32::MAX));
            assert_eq!(u.load(Ordering::Relaxed), 1u64 << 32);
        }
    }
}

/// Number of options (== `_mi_option_last` in v2.4.5).
pub const OPTION_COUNT: usize = 38;

/// Index of `generic_collect` in [`OPTION_NAMES`] — how many trips of the
/// allocator's generic (slow) path go by between automatic collects.
///
/// Named because it is the one option the allocator reads on its own hot-ish
/// path. It was declared with a default of 10,000 and **read by nothing** until
/// P4d of `docs/plans/small-metal.md`, which is why a small-profile heap could
/// starve on its own per-class page cache and never recover.
pub const GENERIC_COLLECT: usize = 36;
const _: () = assert!(GENERIC_COLLECT < OPTION_COUNT);

/// Default trips of the generic path between automatic collects.
///
/// Upstream's 10,000 is tuned for a segment of 512 slices, where one cached
/// page per size class is 14 % of the segment and waiting is free. At the small
/// profile a segment holds 16, so ~24 classes is every slice there is — and the
/// P4d stress battery makes only ~649 generic trips in TOTAL, so a 10,000-trip
/// timer never fires at all before the heap has starved.
///
/// **This is the cheap half of upstream's `retire_expire`.** That mechanism
/// gives each retired page its own countdown, decremented on every generic
/// trip, so a sole empty page ages out after ~16 rather than waiting for a
/// sweep. Implementing it means tracking a retired-bin range on the heap, and
/// `alloc::retire_or_abort` is deliberately written to decide keep-one-warm
/// from the PAGE's own links precisely so it never has to resolve the heap —
/// a measured optimisation. Since `collect` now reclaims a bin's last page,
/// a short sweep period buys the same ageing without touching that path.
/// Upstream's per-page countdown stays unimplemented and is recorded in
/// `docs/plans/small-metal.md` §6.
/// Shipped geometry: upstream's 10,000.
#[cfg(not(ra_small_profile))]
pub const GENERIC_COLLECT_DEFAULT: i64 = 10_000;
/// Small profile: 512, for the reasons above.
#[cfg(ra_small_profile)]
pub const GENERIC_COLLECT_DEFAULT: i64 = 512;

/// Option names in ABI index order (also the env-var suffixes, uppercased).
pub const OPTION_NAMES: [&str; OPTION_COUNT] = [
    "show_errors",
    "show_stats",
    "verbose",
    "eager_commit",
    "arena_eager_commit",
    "purge_decommits",
    "allow_large_os_pages",
    "reserve_huge_os_pages",
    "reserve_huge_os_pages_at",
    "reserve_os_memory",
    "deprecated_segment_cache",
    "deprecated_page_reset",
    "abandoned_page_purge",
    "deprecated_segment_reset",
    "eager_commit_delay",
    "purge_delay",
    "use_numa_nodes",
    "disallow_os_alloc",
    "os_tag",
    "max_errors",
    "max_warnings",
    "max_segment_reclaim",
    "destroy_on_exit",
    "arena_reserve",
    "arena_purge_mult",
    "purge_extend_delay",
    "abandoned_reclaim_on_free",
    "disallow_arena_alloc",
    "retry_on_oom",
    "visit_abandoned",
    "guarded_min",
    "guarded_max",
    "guarded_precise",
    "guarded_sample_rate",
    "guarded_sample_seed",
    "target_segments_per_thread",
    "generic_collect",
    "allow_thp",
];

const DEFAULTS: [i64; OPTION_COUNT] = [
    0,  // show_errors
    0,  // show_stats
    0,  // verbose
    1,  // eager_commit
    2,  // arena_eager_commit
    1,  // purge_decommits
    0,  // allow_large_os_pages
    0,  // reserve_huge_os_pages
    -1, // reserve_huge_os_pages_at
    0,  // reserve_os_memory (KiB)
    // abandoned_page_purge defaults ON (upstream does the same). An abandoned
    // segment has no owner to reuse its pages, so holding them resident buys
    // nothing and costs 32 MiB a time — the RSS tail measured against mimalloc.
    0,
    0,
    0,
    1,         // deprecated x3 / abandoned_page_purge(1)
    1,         // eager_commit_delay
    -1,        // purge_delay: v1 ships purging OPT-IN (see LEDGER M8 open defect)
    0,         // use_numa_nodes
    0,         // disallow_os_alloc
    100,       // os_tag
    32,        // max_errors
    32,        // max_warnings
    10,        // max_segment_reclaim (%)
    0,         // destroy_on_exit
    1_048_576, // arena_reserve (KiB = 1 GiB)
    10,        // arena_purge_mult
    1,         // purge_extend_delay
    1,         // abandoned_reclaim_on_free
    0,         // disallow_arena_alloc
    400,       // retry_on_oom (ms)
    0,         // visit_abandoned
    0,
    0,
    0,                       // guarded_min/max/precise
    1000,                    // guarded_sample_rate
    0,                       // guarded_sample_seed
    0,                       // target_segments_per_thread
    GENERIC_COLLECT_DEFAULT, // generic_collect
    1,                       // allow_thp
];

static VALUES: [AtomicI64; OPTION_COUNT] = [const { AtomicI64::new(i64::MIN) }; OPTION_COUNT];
static ENV_PARSED: AtomicBool = AtomicBool::new(false);

fn ensure_init() {
    if ENV_PARSED.swap(true, Ordering::AcqRel) {
        return;
    }
    // A firmware has no environment, no owned strings and no formatter, so
    // without `std` every option keeps its compiled-in default — the whole of
    // the no_std option story (P3 of `docs/plans/small-metal.md`, §2.5). This
    // is deletion, not a port: there is nothing to read, so the environment
    // pass does not exist rather than existing and returning nothing.
    for i in 0..OPTION_COUNT {
        VALUES[i].store(DEFAULTS[i], Ordering::Release);
    }
    #[cfg(feature = "std")]
    for i in 0..OPTION_COUNT {
        let name = OPTION_NAMES[i].to_uppercase();
        let val = std::env::var(std::format!("RUSTY_ALLOC_{name}"))
            .or_else(|_| std::env::var(std::format!("MIMALLOC_{name}")))
            .ok()
            .and_then(|s| parse_value(&s));
        if let Some(v) = val {
            VALUES[i].store(v, Ordering::Release);
        }
    }
}

#[cfg(feature = "std")]
fn parse_value(s: &str) -> Option<i64> {
    match s.trim().to_ascii_lowercase().as_str() {
        "" | "1" | "true" | "yes" | "on" => Some(1),
        "0" | "false" | "no" | "off" => Some(0),
        t => t.parse::<i64>().ok(),
    }
}

/// `mi_option_get`.
pub fn get(option: usize) -> i64 {
    if option >= OPTION_COUNT {
        return 0;
    }
    ensure_init();
    let v = VALUES[option].load(Ordering::Acquire);
    if v == i64::MIN { DEFAULTS[option] } else { v }
}

/// `mi_option_set`.
pub fn set(option: usize, value: i64) {
    if option < OPTION_COUNT {
        ensure_init();
        VALUES[option].store(value, Ordering::Release);
    }
}

/// `mi_option_set_default`: only if still at the built-in default.
pub fn set_default(option: usize, value: i64) {
    if option < OPTION_COUNT {
        ensure_init();
        let _ = VALUES[option].compare_exchange(
            DEFAULTS[option],
            value,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// `mi_option_is_enabled`.
pub fn is_enabled(option: usize) -> bool {
    get(option) != 0
}

/// `mi_option_get_clamp`.
pub fn get_clamp(option: usize, min: i64, max: i64) -> i64 {
    get(option).clamp(min, max)
}

/// `mi_option_get_size`: `_size` options are stored in KiB.
pub fn get_size(option: usize) -> usize {
    let v = get(option).max(0) as usize;
    match option {
        9 | 23 => v * 1024, // reserve_os_memory, arena_reserve
        _ => v,
    }
}

/// `mi_options_print` via the output hook.
///
/// std-only: building the line needs an owned string. A `no_std` consumer that
/// wants this can format into a stack buffer and call [`out_fmt`], which is
/// the seam that survives.
#[cfg(feature = "std")]
pub fn print() {
    ensure_init();
    for (i, name) in OPTION_NAMES.iter().enumerate() {
        out_fmt(&format!("option '{name}': {}\n", get(i)));
    }
}

// ---------------------------------------------------------------------------
// Registered hooks (mi_register_output / _error / _deferred_free)
// ---------------------------------------------------------------------------

/// C output hook signature.
pub type OutputFun = unsafe extern "C" fn(msg: *const core::ffi::c_char, arg: *mut c_void);
/// C error hook signature.
pub type ErrorFun = unsafe extern "C" fn(err: i32, arg: *mut c_void);
/// C deferred-free hook signature.
pub type DeferredFreeFun = unsafe extern "C" fn(force: bool, heartbeat: u64, arg: *mut c_void);

static OUTPUT_FUN: AtomicUsize2 = AtomicUsize2::new();
static ERROR_FUN: AtomicUsize2 = AtomicUsize2::new();
static DEFERRED_FUN: AtomicUsize2 = AtomicUsize2::new();
static HEARTBEAT: AtomicU64 = AtomicU64::new(0);

/// (fn ptr, arg) pair stored as two atomics (registration is set-once-ish;
/// tearing between the two reads yields a stale-but-valid pair).
struct AtomicUsize2 {
    f: AtomicPtr<c_void>,
    a: AtomicPtr<c_void>,
}

impl AtomicUsize2 {
    /// The FUNCTION pointer alone. `load` reads both halves; a caller that
    /// only needs to know whether a hook is registered at all should not pay
    /// for the argument it is not going to use.
    #[inline]
    fn load_fun(&self) -> *mut c_void {
        self.f.load(Ordering::Acquire)
    }

    const fn new() -> Self {
        AtomicUsize2 {
            f: AtomicPtr::new(core::ptr::null_mut()),
            a: AtomicPtr::new(core::ptr::null_mut()),
        }
    }
    fn set(&self, f: *mut c_void, a: *mut c_void) {
        self.a.store(a, Ordering::Release);
        self.f.store(f, Ordering::Release);
    }
    fn load(&self) -> (*mut c_void, *mut c_void) {
        (
            self.f.load(Ordering::Acquire),
            self.a.load(Ordering::Acquire),
        )
    }
}

/// `mi_register_output`.
pub fn register_output(f: Option<OutputFun>, arg: *mut c_void) {
    OUTPUT_FUN.set(f.map_or(core::ptr::null_mut(), |f| f as *mut c_void), arg);
}

/// `mi_register_error`.
pub fn register_error(f: Option<ErrorFun>, arg: *mut c_void) {
    ERROR_FUN.set(f.map_or(core::ptr::null_mut(), |f| f as *mut c_void), arg);
}

/// `mi_register_deferred_free`.
pub fn register_deferred_free(f: Option<DeferredFreeFun>, arg: *mut c_void) {
    DEFERRED_FUN.set(f.map_or(core::ptr::null_mut(), |f| f as *mut c_void), arg);
}

/// Route a message to the registered output hook, else stderr.
///
/// Takes `&str` and needs no allocation, so this SEAM survives `no_std` — a
/// firmware that registers an output hook still gets the allocator's messages
/// over its serial log. Only the *stderr fallback* and the `format!`-based
/// CALLERS are std-only (P3 of `docs/plans/small-metal.md`, §2.5).
pub fn out_fmt(msg: &str) {
    let (f, a) = OUTPUT_FUN.load();
    if f.is_null() {
        #[cfg(feature = "std")]
        std::eprint!("{msg}");
        return;
    }
    // NUL-terminate on the stack for the C hook (bounded copy).
    let bytes = msg.as_bytes();
    let mut buf = [0u8; 512];
    let n = bytes.len().min(511);
    buf[..n].copy_from_slice(&bytes[..n]);
    // SAFETY: f was registered with the documented signature; buf is a valid
    // NUL-terminated C string for the duration of the call.
    unsafe {
        let fun: OutputFun = core::mem::transmute::<*mut c_void, OutputFun>(f);
        fun(buf.as_ptr().cast(), a);
    }
}

/// Report an error code through the hook (else stderr when show_errors).
pub fn error(err: i32) {
    let (f, a) = ERROR_FUN.load();
    if !f.is_null() {
        // SAFETY: registered with the documented signature.
        unsafe {
            let fun: ErrorFun = core::mem::transmute::<*mut c_void, ErrorFun>(f);
            fun(err, a);
        }
    } else if is_enabled(0) {
        // The error CODE still reaches a registered hook above; only the
        // human-readable fallback needs a formatter (P3, §2.5).
        #[cfg(feature = "std")]
        out_fmt(&std::format!("rusty_alloc: error {err}\n"));
    }
}

/// Fire the deferred-free hook (called from the allocation heartbeat).
pub fn deferred_free(force: bool) {
    // Peek at the FUNCTION pointer alone first. `mi_register_deferred_free` is
    // unregistered in nearly every process, and this runs on every slow-path
    // allocation — loading the argument pointer too, only to discard it when
    // there is no hook, is an atomic load spent on nothing.
    if DEFERRED_FUN.load_fun().is_null() {
        return;
    }
    fire_deferred(force);
}

/// Actually call the registered hook.
///
/// Out of line because it is an INDIRECT call, and `deferred_free` inlines
/// into `Heap::malloc_generic`: an indirect call with values live across it
/// forces the whole heartbeat's caller to preserve callee-saved registers, on
/// every slow-path allocation, for a hook that is unregistered in nearly every
/// process. The peek above is all the common path executes.
#[cold]
#[inline(never)]
fn fire_deferred(force: bool) {
    let (f, a) = DEFERRED_FUN.load();
    if !f.is_null() {
        let hb = HEARTBEAT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: registered with the documented signature.
        unsafe {
            let fun: DeferredFreeFun = core::mem::transmute::<*mut c_void, DeferredFreeFun>(f);
            fun(force, hb, a);
        }
    }
}
