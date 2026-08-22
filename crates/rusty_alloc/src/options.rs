//! Options table, environment parsing, and the registered hooks (mirrors
//! `options.c`). Option INDICES are ABI: the enum matches the oracle v2.4.5
//! ordering exactly, deprecated slots included.
//!
//! Env: `MIMALLOC_<NAME>` (compat) and `RUSTY_ALLOC_<NAME>` (ours) — e.g.
//! `MIMALLOC_SHOW_STATS=1`, `MIMALLOC_PURGE_DELAY=0`. Parsed once on first
//! option access. Values follow mimalloc: booleans accept 1/0/true/false/
//! yes/no/on/off; sizes are plain integers (`_size` options are KiB).

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, AtomicU64, Ordering};

/// Number of options (== `_mi_option_last` in v2.4.5).
pub const OPTION_COUNT: usize = 38;

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
    0, 0, 0, 1,         // deprecated x3 / abandoned_page_purge(1)
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
    0, 0, 0,     // guarded_min/max/precise
    1000,  // guarded_sample_rate
    0,     // guarded_sample_seed
    0,     // target_segments_per_thread
    10000, // generic_collect
    1,     // allow_thp
];

static VALUES: [AtomicI64; OPTION_COUNT] = [const { AtomicI64::new(i64::MIN) }; OPTION_COUNT];
static ENV_PARSED: AtomicBool = AtomicBool::new(false);

fn ensure_init() {
    if ENV_PARSED.swap(true, Ordering::AcqRel) {
        return;
    }
    for i in 0..OPTION_COUNT {
        let name = OPTION_NAMES[i].to_uppercase();
        let val = std::env::var(format!("RUSTY_ALLOC_{name}"))
            .or_else(|_| std::env::var(format!("MIMALLOC_{name}")))
            .ok()
            .and_then(|s| parse_value(&s));
        let v = val.unwrap_or(DEFAULTS[i]);
        VALUES[i].store(v, Ordering::Release);
    }
}

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
pub fn out_fmt(msg: &str) {
    let (f, a) = OUTPUT_FUN.load();
    if f.is_null() {
        eprint!("{msg}");
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
        out_fmt(&format!("rusty_alloc: error {err}\n"));
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
