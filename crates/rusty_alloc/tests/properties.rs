//! H-28: the invariants this allocator DOCUMENTS, expressed as properties
//! over generated inputs rather than a handful of examples.
//!
//! Every property here is a claim the crate makes in prose somewhere — the
//! README, a doc comment, or the threat model — so a failure is a broken
//! promise, not a surprising-but-legal result. They are cheap (no timing, no
//! noise floor) and deterministic given the seed proptest prints on failure.

use proptest::prelude::*;
use rusty_alloc::alloc;

/// Proptest's default config writes a `.proptest-regressions` file, which
/// needs `getcwd` — an operation Miri's isolation denies, and the only reason
/// these properties could not run under Miri at all. Turning persistence off
/// lets the WHOLE suite run interpreted (a property checked by Miri is worth
/// far more than one skipped there), and under Miri the case count drops
/// because interpretation is ~3 orders of magnitude slower.
fn cfg() -> ProptestConfig {
    ProptestConfig {
        failure_persistence: None,
        cases: if cfg!(miri) { 4 } else { 256 },
        ..ProptestConfig::default()
    }
}

/// Sizes that span every routing decision the allocator makes: the small
/// bins, the medium range, the 64 KiB binned cutoff, large spans, and past
/// `LARGE_OBJ_SIZE_MAX` into dedicated huge segments.
fn any_size() -> impl Strategy<Value = usize> {
    prop_oneof![
        5 => 0usize..1024,          // small bins (the fast path)
        3 => 1024usize..65_536,     // medium, crossing the binned cutoff
        2 => 65_536usize..1_000_000, // large spans
        1 => 1_000_000usize..(if cfg!(miri) { 1_200_000 } else { 8_000_000 }), // huge segments
    ]
}

proptest! {
    #![proptest_config(cfg())]

    /// The `usable_size` contract: at least what was asked for, and stable
    /// while the block is live. Downstream code sizes buffers off this.
    #[test]
    fn usable_size_is_at_least_requested_and_stable(size in any_size()) {
        let p = alloc::malloc(size);
        prop_assert!(!p.is_null(), "malloc({size}) returned null");
        // SAFETY: p is a live block from this allocator.
        unsafe {
            let u1 = alloc::usable_size(p);
            let u2 = alloc::usable_size(p);
            prop_assert!(u1 >= size, "usable_size {u1} < requested {size}");
            prop_assert_eq!(u1, u2, "usable_size changed while the block was live");
            alloc::free(p);
        }
    }

    /// `zalloc` promises zero across the FULL usable extent, not merely the
    /// requested prefix — the invariant `rezalloc`'s zero-preservation rests
    /// on. Writing a pattern first makes a recycled block the likely case.
    #[test]
    fn zalloc_is_zero_across_the_whole_usable_extent(size in 1usize..200_000) {
        // Dirty a block of the same class first, so zalloc is likely to be
        // handed recycled rather than virgin memory.
        let dirty = alloc::malloc(size);
        if !dirty.is_null() {
            // SAFETY: live block, written within its own extent.
            unsafe {
                core::ptr::write_bytes(dirty, 0xDD, size);
                alloc::free(dirty);
            }
        }
        let p = alloc::zalloc(size);
        prop_assert!(!p.is_null());
        // SAFETY: live block; zalloc's contract covers [p, p+usable).
        unsafe {
            let u = alloc::usable_size(p);
            for i in [0, size / 2, size - 1, u - 1] {
                prop_assert_eq!(*p.add(i), 0, "zalloc byte {} was not zero", i);
            }
            alloc::free(p);
        }
    }

    /// Aligned allocation returns what it promised, for every power of two up
    /// to a page, and the block is still usable for the requested size.
    #[test]
    fn aligned_blocks_are_aligned(size in 1usize..100_000, shift in 0u32..13) {
        let align = 1usize << shift;
        let p = alloc::malloc_aligned(size, align);
        prop_assert!(!p.is_null(), "malloc_aligned({size}, {align}) returned null");
        prop_assert_eq!(p.addr() % align, 0, "block not {}-aligned", align);
        // SAFETY: live block.
        unsafe {
            prop_assert!(alloc::usable_size(p) >= size);
            alloc::free(p);
        }
    }

    /// `realloc` preserves `min(old, new)` bytes — across a move as well as
    /// in place, which is why the sizes are generated independently.
    #[test]
    fn realloc_preserves_the_prefix(old in 1usize..80_000, new in 1usize..80_000) {
        let p = alloc::malloc(old);
        prop_assert!(!p.is_null());
        // SAFETY: live block; realloc consumes p when it moves.
        unsafe {
            core::ptr::write_bytes(p, 0x3C, old);
            let np = alloc::realloc(p, new);
            prop_assert!(!np.is_null());
            let keep = old.min(new);
            for i in [0, keep / 2, keep - 1] {
                prop_assert_eq!(*np.add(i), 0x3C, "realloc lost prefix byte {}", i);
            }
            prop_assert!(alloc::usable_size(np) >= new);
            alloc::free(np);
        }
    }

    /// Live blocks are DISJOINT. This is the property whose violation is the
    /// failure the whole project exists to prevent (two owners, one block),
    /// so it is checked by writing a per-block tag and re-reading every tag
    /// only after ALL allocations are live.
    #[test]
    fn live_blocks_never_overlap(sizes in prop::collection::vec(1usize..8_000, 1..64)) {
        let mut live: Vec<(*mut u8, usize, u8)> = Vec::new();
        for (i, &n) in sizes.iter().enumerate() {
            let p = alloc::malloc(n);
            if p.is_null() {
                continue;
            }
            let tag = (i as u8).wrapping_add(1);
            // SAFETY: live block, written within its own extent.
            unsafe { core::ptr::write_bytes(p, tag, n) };
            live.push((p, n, tag));
        }
        // Verify only AFTER every block is live: an overlap would have been
        // overwritten by a later allocation, which a check-as-you-go loop
        // would miss entirely.
        for &(p, n, tag) in &live {
            // SAFETY: still-live block.
            unsafe {
                for i in [0, n / 2, n - 1] {
                    prop_assert_eq!(*p.add(i), tag, "block contents overlapped");
                }
            }
        }
        for (p, _, _) in live {
            // SAFETY: each block live and freed exactly once.
            unsafe { alloc::free(p) };
        }
    }

    /// `good_size` must be idempotent and never shrink a request — it is the
    /// function G2 pins against the C oracle's bin geometry.
    #[test]
    fn good_size_is_idempotent_and_never_shrinks(size in 0usize..2_000_000) {
        let g = rusty_alloc::bins::good_size(size);
        prop_assert!(g >= size, "good_size({size}) = {g} shrank the request");
        prop_assert_eq!(rusty_alloc::bins::good_size(g), g, "good_size is not idempotent");
    }

    /// A block's usable extent is never smaller than `good_size` promised for
    /// it: the two answers about the same request must agree.
    #[test]
    fn usable_size_agrees_with_good_size(size in 1usize..100_000) {
        let g = rusty_alloc::bins::good_size(size);
        let p = alloc::malloc(size);
        prop_assert!(!p.is_null());
        // SAFETY: live block.
        unsafe {
            let u = alloc::usable_size(p);
            prop_assert!(
                u >= g,
                "usable_size {u} is below the {g} that good_size promised for a {size}-byte request"
            );
            alloc::free(p);
        }
    }

    /// `free(null)` is a documented no-op, and `malloc(0)` returns a real,
    /// freeable pointer rather than null (the C contract this crate mirrors).
    #[test]
    fn zero_and_null_edge_cases(_seed in 0u8..8) {
        // SAFETY: freeing null is explicitly a no-op.
        unsafe { alloc::free(core::ptr::null_mut()) };
        let p = alloc::malloc(0);
        prop_assert!(!p.is_null(), "malloc(0) must return a unique freeable pointer");
        // SAFETY: live block from malloc(0).
        unsafe { alloc::free(p) };
    }
}
