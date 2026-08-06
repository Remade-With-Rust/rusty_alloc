//! M8 hardening gates. The free-list-encoding assertions only mean something
//! in a `secure` build; the guarded-object and purge gates run in both.

use rusty_alloc::alloc::{free, malloc, usable_size};
use rusty_alloc::init;

#[test]
fn csprng_streams_differ_per_heap() {
    // Two heaps must not share a stream (keys would correlate).
    let h1 = init::create_heap(0, true, -1);
    let h2 = init::create_heap(0, true, -1);
    // SAFETY: heaps ours on this thread.
    unsafe {
        let a: Vec<usize> = (0..8)
            .map(|_| (*(*h1).heap.get()).rng.next_usize())
            .collect();
        let b: Vec<usize> = (0..8)
            .map(|_| (*(*h2).heap.get()).rng.next_usize())
            .collect();
        assert_ne!(a, b, "per-heap CSPRNG streams collided");
        init::heap_destroy(h1);
        init::heap_destroy(h2);
    }
}

#[test]
fn free_list_survives_heavy_churn() {
    // Encoding must be transparent: same behaviour, secure or not.
    let mut live: Vec<(*mut u8, usize, u8)> = Vec::new();
    let mut state = 0x5EC0_0DE5_1234_5678u64;
    let mut rng = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let iters = if cfg!(miri) { 500 } else { 50_000 };
    for i in 0..iters {
        if !live.is_empty() && (live.len() > 800 || rng() % 2 == 0) {
            let idx = (rng() as usize) % live.len();
            let (p, size, tag) = live.swap_remove(idx);
            // SAFETY: tracked live block.
            unsafe {
                assert_eq!(p.read(), tag);
                assert_eq!(p.add(size - 1).read(), tag);
                free(p);
            }
        } else {
            let size = 8 + (rng() as usize) % 3000;
            let p = malloc(size);
            assert!(!p.is_null());
            let tag = (i as u8) | 1;
            // SAFETY: fresh live block.
            unsafe {
                assert!(usable_size(p) >= size);
                p.write(tag);
                p.add(size - 1).write(tag);
            }
            live.push((p, size, tag));
        }
    }
    for (p, size, tag) in live.drain(..) {
        // SAFETY: tracked live blocks.
        unsafe {
            assert_eq!(p.read(), tag);
            assert_eq!(p.add(size - 1).read(), tag);
            free(p);
        }
    }
}

#[test]
#[cfg(not(miri))] // guard pages need real OS protection
fn guarded_objects_are_protected() {
    let h = init::create_heap(0, true, -1);
    // SAFETY: heap ours; guard every eligible object in [1, 64 KiB].
    unsafe {
        (*(*h).heap.get()).guarded_set_size_bound(1, 64 * 1024);
        (*(*h).heap.get()).guarded_set_sample_rate(1, 0x1234);
        let before = (*(*h).heap.get()).stats.guarded;
        let mut ps = Vec::new();
        for size in [64usize, 500, 4096, 20_000] {
            let p = rusty_alloc::alloc::heap_malloc(h, size);
            assert!(!p.is_null());
            // The object is usable end to end...
            core::ptr::write_bytes(p, 0xA7, size);
            assert_eq!(p.read(), 0xA7);
            assert_eq!(p.add(size - 1).read(), 0xA7);
            // ...and it ends exactly at a page boundary (guard follows).
            assert_eq!(
                (p.addr() + size) % rusty_alloc::os::page_size(),
                0,
                "guarded object not right-aligned against its guard page"
            );
            ps.push(p);
        }
        let after = (*(*h).heap.get()).stats.guarded;
        assert!(
            after - before >= 4,
            "guarded sampling did not fire: {before} → {after}"
        );
        for p in ps {
            free(p);
        }
        init::heap_destroy(h);
    }
}

#[test]
fn purge_returns_memory() {
    // Retiring large spans must drive purges (the RSS lever). Counter-first:
    // a deterministic count, not a timing. Purging is opt-in in v1, so the
    // gate turns it on explicitly for this test.
    rusty_alloc::options::set(15, 10); // purge_delay
    let before = rusty_alloc::alloc::stats().purges;
    let mut ps = Vec::new();
    for _ in 0..24 {
        let p = malloc(600 * 1024); // multi-slice spans
        assert!(!p.is_null());
        // SAFETY: live block; touch to commit.
        unsafe { core::ptr::write_bytes(p, 1, 600 * 1024) };
        ps.push(p);
    }
    for p in ps {
        // SAFETY: tracked live blocks.
        unsafe { free(p) };
    }
    rusty_alloc::alloc::collect(true);
    let after = rusty_alloc::alloc::stats().purges;
    rusty_alloc::options::set(15, -1); // restore the v1 default
    assert!(after > before, "no spans purged: {before} → {after}");
}
