//! M3 span-lifecycle gates. This file is its own process, and everything runs
//! in ONE test fn, so the global heap's counters are fully deterministic here
//! (unlike the parallel suites).

use rusty_alloc::alloc::{expand, free, malloc, realloc, stats, usable_size, zalloc};

#[test]
fn span_lifecycle_and_realloc() {
    // Sizes derived from the geometry, not written in MiB. "1 MiB" is a
    // 16-slice large span at the shipped 32 MiB segment and a HUGE block at a
    // 64 KiB one (P2, `docs/plans/small-metal.md`), so a literal silently
    // stops testing the large path the moment the geometry moves. `L` is one
    // slice past a medium page — large by definition — and `L2` is twice that,
    // both comfortably inside `LARGE_OBJ_SIZE_MAX` at either geometry.
    use rusty_alloc::segment::USABLE_SLICES;
    use rusty_alloc::types::SEGMENT_SLICE_SIZE;
    // Two slices is past MEDIUM_OBJ_SIZE_MAX at every geometry this crate
    // builds, so `l` is a large SPAN; `l2` doubles it and both stay well
    // inside `USABLE_SLICES` (511 at the shipped geometry, 7 at the small
    // profile — which is what a fixed `2 MiB` overshot).
    let l: usize = SEGMENT_SLICE_SIZE * 2;
    let l2: usize = SEGMENT_SLICE_SIZE * 4;
    // A span filling a large FRACTION of a segment, for the coalescing check
    // below — the point is "most of one segment", not "12 MiB".
    let big_span: usize = SEGMENT_SLICE_SIZE * (USABLE_SLICES * 3 / 8);

    // --- Large path basics -------------------------------------------------
    let s0 = stats();
    let p = malloc(l); // one slice past a medium page -> a large span
    assert!(!p.is_null());
    // SAFETY: live `l`-byte block.
    unsafe {
        assert!(usable_size(p) >= l);
        p.write(7);
        p.add(l - 1).write(8);
    }
    let s1 = stats();
    assert_eq!(s1.large_allocs - s0.large_allocs, 1, "large path not taken");

    // --- Span reclamation: free + realloc similar size must NOT grow the
    // segment count (the span is reused first-fit) ---------------------------
    // SAFETY: live block freed once.
    unsafe { free(p) };
    let s2 = stats();
    assert_eq!(
        s2.pages_retired - s1.pages_retired,
        1,
        "large span not retired"
    );
    // "Similar size": slightly smaller than `l`, so it must reuse the span
    // just retired rather than take a fresh segment.
    let q = malloc(l * 7 / 8);
    assert!(!q.is_null());
    let s3 = stats();
    assert_eq!(
        s3.segments, s2.segments,
        "reclaimed span not reused — new segment allocated"
    );
    // SAFETY: live block.
    unsafe { free(q) };

    // --- zalloc over a RECYCLED span must be re-zeroed ----------------------
    let d = malloc(l2);
    // SAFETY: live block, dirtied then freed.
    unsafe {
        core::ptr::write_bytes(d, 0xAB, l2);
        free(d);
    }
    let z = zalloc(l2);
    assert!(!z.is_null());
    // SAFETY: live zeroed block.
    unsafe {
        for i in (0..l2).step_by(4096) {
            assert_eq!(
                z.add(i).read(),
                0,
                "recycled span leaked dirty bytes at +{i}"
            );
        }
        free(z);
    }

    // --- Binned page retire: burst-free a size class, pages return ---------
    let s4 = stats();
    let blocks: Vec<*mut u8> = (0..2000).map(|_| malloc(2048)).collect();
    let s5 = stats();
    assert!(
        s5.pages_fresh - s4.pages_fresh >= 2,
        "burst should carve several pages"
    );
    for &b in &blocks {
        // SAFETY: tracked live blocks.
        unsafe { free(b) };
    }
    let s6 = stats();
    assert!(
        s6.pages_retired - s5.pages_retired >= (s5.pages_fresh - s4.pages_fresh) - 1,
        "empty pages not retired (one may stay warm): fresh {} retired {}",
        s5.pages_fresh - s4.pages_fresh,
        s6.pages_retired - s5.pages_retired
    );

    // --- Coalescing observable: after retiring everything, a full-segment
    // large alloc must fit in the SAME segment count -------------------------
    let big = malloc(big_span);
    assert!(!big.is_null());
    let s7 = stats();
    assert_eq!(
        s7.segments, s6.segments,
        "coalescing failed — the large span needed a new segment"
    );
    // SAFETY: live block.
    unsafe { free(big) };

    // --- realloc semantics --------------------------------------------------
    let r = malloc(100);
    // SAFETY: live blocks; realloc contract followed throughout.
    unsafe {
        core::ptr::write_bytes(r, 0x5A, 100);
        // Grow within the same bin (112B usable): in place. Asserted by
        // POINTER IDENTITY (the behaviour callers depend on) AND the counter.
        let s_ip0 = stats().realloc_in_place;
        let r2 = realloc(r, 112);
        assert_eq!(r2, r, "grow-within-usable must stay in place");
        // The counter is the SECONDARY witness; pointer identity above is the
        // behaviour callers depend on. Debug-only because `realloc_in_place` is
        // now a `#[cfg(debug_assertions)]` counter (see `alloc::stat_realloc` —
        // it was costing the release realloc path a full heap resolution just
        // to bump a diagnostic). In release it stays 0, so assert only in debug.
        #[cfg(debug_assertions)]
        {
            assert_eq!(stats().realloc_in_place - s_ip0, 1);
        }
        // Grow across bins: moves, prefix preserved.
        let r3 = realloc(r2, 4096);
        assert!(!r3.is_null());
        for i in 0..100 {
            assert_eq!(r3.add(i).read(), 0x5A, "realloc lost prefix at {i}");
        }
        // Shrink far below half: moves to a smaller class.
        let r4 = realloc(r3, 16);
        assert!(!r4.is_null());
        assert_eq!(r4.read(), 0x5A);
        assert!(usable_size(r4) < 4096);
        // expand: in-place only.
        assert_eq!(expand(r4, 8), r4);
        assert!(expand(r4, 1 << 20).is_null(), "expand must never move");
        free(r4);
    }

    // --- segment map -------------------------------------------------------
    let m = malloc(64);
    assert!(rusty_alloc::alloc::is_in_heap_region(m));
    let stack_local = 0u8;
    // The NEGATIVE direction needs an exact map. The small profile's range
    // table (P2, `docs/plans/small-metal.md`) is chip-sized — 64 entries, 1 KiB
    // of BSS against the bitmap's 1 MiB — and this battery manages orders of
    // magnitude more segments than any chip, so it overflows and `contains`
    // degrades PERMISSIVELY on purpose: over-reporting costs a diagnostic,
    // under-reporting would abort legitimate frees through the `debug_checks`
    // guard. Assert the property only while the map can still decide, and
    // check the degradation is exactly what happened rather than skipping
    // blind.
    if rusty_alloc::segment_map::range_table_overflowed() {
        assert!(
            rusty_alloc::alloc::is_in_heap_region(&stack_local),
            "an overflowed range table must answer permissively, not wrongly"
        );
    } else {
        assert!(!rusty_alloc::alloc::is_in_heap_region(&stack_local));
    }
    // SAFETY: live block.
    unsafe { free(m) };

    // --- strict leak accounting for this whole test ------------------------
    let end = stats();
    assert_eq!(
        end.allocs - s0.allocs,
        end.frees - s0.frees,
        "leak inside span lifecycle test"
    );
}

// NOTE: exactly one #[test] lives in this file on purpose — the counter
// asserts above need a process where no other test races the global heap.
// The realloc storm lives in alloc_core.rs with the other parallel-safe tests.
