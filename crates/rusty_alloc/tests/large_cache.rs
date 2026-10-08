//! The per-heap large-span cache (`Heap::large_cache`): a freed large block's
//! span is kept for the next allocation of the same slice count, never for
//! another size, and handed back by a collect.
//!
//! Own test binary: the cache is per thread, and the assertions read this
//! thread's heap counters.

use rusty_alloc::alloc::{collect, free, malloc, stats};
use rusty_alloc::types::SEGMENT_SLICE_SIZE;

/// A large span at every geometry (two slices; `tests/spans.rs`). A literal
/// 2 MiB is a HUGE block under `ra_small_profile`, which the cache never sees,
/// so the test passed there without testing anything.
const TWO_MIB: usize = 2 * SEGMENT_SLICE_SIZE;

#[test]
fn same_size_reuses_the_span_other_sizes_do_not_and_collect_flushes() {
    // Same size: the very same block comes back, no span carved for it.
    let a = malloc(TWO_MIB);
    assert!(!a.is_null());
    // SAFETY: live block; then freed once.
    unsafe { core::ptr::write_bytes(a, 0x5A, TWO_MIB) };
    // The address alone cannot tell the cache from first-fit (a retired span
    // is re-carved in the same place), but retirement can: without the cache
    // the free retires the span at once.
    let retired = stats().pages_retired;
    // SAFETY: freed once.
    unsafe { free(a) };
    assert_eq!(
        stats().pages_retired,
        retired,
        "the freed large span was retired, not cached"
    );
    let b = malloc(TWO_MIB);
    assert_eq!(
        b, a,
        "a same-size large allocation did not reuse the cached span"
    );
    // SAFETY: b is a live TWO_MIB block (the cached span, re-handed).
    unsafe { core::ptr::write_bytes(b, 0x11, TWO_MIB) };

    // A different size never takes the cached span: it must still be cached
    // (and so not coalesced away) when `b` is freed and re-requested.
    // SAFETY: freed once.
    unsafe { free(b) };
    let c = malloc(TWO_MIB + SEGMENT_SLICE_SIZE);
    assert!(!c.is_null());
    assert_ne!(c, a, "a larger allocation was handed the cached 2 MiB span");
    let d = malloc(TWO_MIB);
    assert_eq!(
        d, a,
        "the cached span did not survive an allocation of another size"
    );

    // A RUN of large frees is a teardown: the second free in a row retires
    // the cached span and its own, so nothing is left cached to be refilled
    // out of address order by the next load (a same-thread model swap peaked
    // at 1.04 models instead of 1.01 before this rule).
    // SAFETY: c is live, freed once.
    unsafe { free(c) }; // lone free after an allocation: cached
    let retired = stats().pages_retired;
    // SAFETY: d is live, freed once.
    unsafe { free(d) }; // second in a row: flush both
    assert_eq!(
        stats().pages_retired - retired,
        2,
        "a run of large frees left a span cached"
    );

    // A collect hands a cached span back to its segment.
    let e = malloc(TWO_MIB);
    assert!(!e.is_null());
    // SAFETY: e is live, freed once.
    unsafe { free(e) }; // lone free: cached
    let retired = stats().pages_retired;
    collect(false);
    assert!(
        stats().pages_retired > retired,
        "collect did not retire the cached large span"
    );
}
