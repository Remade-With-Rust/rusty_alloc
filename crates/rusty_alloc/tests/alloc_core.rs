//! M2 core allocator tests (G1-shaped, direct API — no global_allocator here;
//! that gate lives in rusty_alloc_bench/tests/selfhost.rs).

use rusty_alloc::alloc::{
    calloc, free, malloc, malloc_aligned, mallocn, realloc, usable_size, zalloc, zalloc_aligned,
};
use rusty_alloc::types::{MEDIUM_OBJ_SIZE_MAX, SMALL_SIZE_MAX};

#[test]
fn malloc_free_roundtrip_all_regimes() {
    // small page, medium page, huge segment
    for size in [
        1,
        8,
        24,
        100,
        1024,
        4096,
        SMALL_SIZE_MAX,
        100 * 1024,
        MEDIUM_OBJ_SIZE_MAX,
        300 * 1024,
        33 * 1024 * 1024,
    ] {
        let p = malloc(size);
        assert!(!p.is_null(), "malloc({size})");
        // SAFETY: fresh live block of ≥ size bytes.
        unsafe {
            assert!(usable_size(p) >= size, "usable < size for {size}");
            p.write(1);
            p.add(size.saturating_sub(1)).write(2);
            free(p);
        }
    }
}

#[test]
fn malloc_zero_size_is_valid_unique() {
    let a = malloc(0);
    let b = malloc(0);
    assert!(!a.is_null() && !b.is_null());
    assert_ne!(a, b, "malloc(0) must return unique pointers");
    // SAFETY: both live, freed once.
    unsafe {
        free(a);
        free(b);
    }
}

#[test]
fn zalloc_is_zero_and_reuse_is_rezeroed() {
    for _ in 0..3 {
        let sizes = [16usize, 200, 1024, 8192, 200 * 1024];
        let ptrs: Vec<*mut u8> = sizes.iter().map(|&s| zalloc(s)).collect();
        for (&size, &p) in sizes.iter().zip(&ptrs) {
            assert!(!p.is_null());
            // SAFETY: live block of ≥ size bytes.
            unsafe {
                for i in 0..size {
                    assert_eq!(p.add(i).read(), 0, "zalloc({size}) non-zero at {i}");
                }
                // Dirty it so the next round proves re-zeroing of recycled blocks.
                core::ptr::write_bytes(p, 0xFF, size);
                free(p);
            }
        }
    }
}

#[test]
fn calloc_overflow_returns_null() {
    assert!(calloc(usize::MAX / 2, 3).is_null());
    assert!(mallocn(usize::MAX, 2).is_null());
}

#[test]
fn block_reuse_same_size_class() {
    // Freeing then reallocating the same class must recycle memory (sharded
    // free lists working) — observed via the address coming back.
    // Pin the page with a live block: since M8, a page whose last block is
    // freed is RETIRED (its span returns to the segment), so an unpinned page
    // legitimately never hands the same address back.
    let pin = malloc(64);
    let p1 = malloc(64);
    // SAFETY: live then freed once.
    unsafe { free(p1) };
    // Hold non-matching blocks so we drain toward the recycled one.
    let mut held = Vec::new();
    let mut seen = false;
    for _ in 0..256 {
        let p = malloc(64);
        held.push(p);
        if p == p1 {
            seen = true;
            break;
        }
    }
    for p in held {
        // SAFETY: tracked live blocks.
        unsafe { free(p) };
    }
    // SAFETY: live block.
    unsafe { free(pin) };
    assert!(seen, "freed block never recycled — free lists broken");
}

#[test]
fn aligned_allocations() {
    for (size, align) in [
        (1usize, 16usize),
        (24, 16),
        (100, 32),
        (256, 256),
        (1000, 512),
        (5000, 4096),
        (100, 65536),
        (300_000, 4096),
        (1024, 1 << 20),
    ] {
        let p = malloc_aligned(size, align);
        assert!(!p.is_null(), "malloc_aligned({size}, {align})");
        assert_eq!(p as usize % align, 0, "misaligned for ({size}, {align})");
        // SAFETY: live block of ≥ size bytes.
        unsafe {
            assert!(usable_size(p) >= size);
            p.write(3);
            free(p);
        }
        let z = zalloc_aligned(size, align);
        assert!(!z.is_null());
        assert_eq!(z as usize % align, 0);
        // SAFETY: live zeroed block.
        unsafe {
            assert_eq!(z.read(), 0);
            assert_eq!(z.add(size - 1).read(), 0);
            free(z);
        }
    }
}

#[test]
fn churn_sweep_randomized() {
    // Bounded random churn across the full size spectrum with content checks —
    // an in-process mini-G1.
    let mut state = 0x1234_5678_9ABC_DEF0u64;
    let mut rng = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut live: Vec<(*mut u8, usize, u8)> = Vec::new();
    // miri interprets every instruction — same shape, 100× smaller run.
    let iters: u64 = if cfg!(miri) { 2_000 } else { 200_000 };
    for i in 0..iters {
        if !live.is_empty() && (live.len() > 2000 || rng() % 100 < 47) {
            let idx = (rng() as usize) % live.len();
            let (p, size, tag) = live.swap_remove(idx);
            // SAFETY: tracked live block; verify canary then free.
            unsafe {
                assert_eq!(p.read(), tag, "canary head corrupted");
                assert_eq!(p.add(size - 1).read(), tag, "canary tail corrupted");
                free(p);
            }
        } else {
            let r = rng() % 1000;
            let size = if r < 800 {
                8 + (rng() as usize % 1016)
            } else if r < 980 {
                1024 + (rng() as usize % 15 * 1024)
            } else {
                16 * 1024 + (rng() as usize % (256 * 1024))
            }
            .max(1);
            let p = malloc(size);
            assert!(!p.is_null());
            let tag = (i as u8) | 1;
            // SAFETY: fresh live block of ≥ size bytes.
            unsafe {
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
    // Counters are process-global and tests run in parallel — a strict
    // allocs==frees leak check needs a single-test process (the replay gate
    // does it); here just assert the counters observed our work.
    let s = rusty_alloc::alloc::stats();
    assert!(s.allocs >= iters / 2);
}

#[test]
fn aligned_at_offsets() {
    use rusty_alloc::alloc::{malloc_aligned_at, zalloc_aligned_at};
    // (p + offset) % align == 0 across tiers: natural-fit, oversize-adjust
    // (interior pointer), large spans, and huge placement.
    for (size, align, offset) in [
        (64usize, 64usize, 0usize),
        (100, 128, 8),
        (100, 128, 24),
        (1000, 4096, 16),
        (5000, 512, 128),
        (200 * 1024, 4096, 32),          // large span, adjusted
        (100, 65536, 40),                // big align, small size
        (20 * 1024 * 1024, 1 << 20, 64), // huge placement
    ] {
        let p = malloc_aligned_at(size, align, offset);
        assert!(!p.is_null(), "malloc_aligned_at({size},{align},{offset})");
        assert_eq!(
            (p as usize + offset) % align,
            0,
            "constraint violated ({size},{align},{offset})"
        );
        // SAFETY: live block of ≥ size usable bytes from p.
        unsafe {
            assert!(rusty_alloc::alloc::usable_size(p) >= size);
            p.write(0xEE);
            p.add(size - 1).write(0xDD);
            assert_eq!(p.read(), 0xEE);
            rusty_alloc::alloc::free(p); // interior-pointer recovery path
        }
        let z = zalloc_aligned_at(size.min(300_000), align, offset);
        assert!(!z.is_null());
        // SAFETY: live zeroed block.
        unsafe {
            assert_eq!((z as usize + offset) % align, 0);
            assert_eq!(z.read(), 0);
            assert_eq!(z.add(size.min(300_000) - 1).read(), 0);
            rusty_alloc::alloc::free(z);
        }
    }
}

#[test]
fn rezalloc_grows_zero() {
    use rusty_alloc::alloc::{recalloc, rezalloc, zalloc};
    let p = zalloc(100);
    // SAFETY: zalloc lineage maintained throughout; each pointer consumed once.
    unsafe {
        core::ptr::write_bytes(p, 0x11, 100);
        // Grow far: moved; prefix kept; tail reads zero to the new usable end.
        let p2 = rezalloc(p, 5000);
        assert!(!p2.is_null());
        for i in 0..100 {
            assert_eq!(p2.add(i).read(), 0x11, "prefix lost at {i}");
        }
        let us = rusty_alloc::alloc::usable_size(p2);
        for i in 100..us {
            assert_eq!(p2.add(i).read(), 0, "rezalloc tail not zero at {i}");
        }
        // recalloc overflow guard.
        assert!(recalloc(core::ptr::null_mut(), usize::MAX / 2, 3).is_null());
        rusty_alloc::alloc::free(p2);
    }
}

#[test]
fn align_storm() {
    use rusty_alloc::alloc::{free, malloc_aligned_at, usable_size};
    // Randomized aligned churn with canaries — the M5 fuzz-shaped gate.
    let mut state = 0xA11C_C0FF_EE55_EED5u64;
    let mut rng = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let iters = if cfg!(miri) { 400 } else { 30_000 };
    let mut live: Vec<(*mut u8, usize, usize, usize, u8)> = Vec::new(); // p,size,align,offset,tag
    for i in 0..iters {
        if !live.is_empty() && (live.len() > 600 || rng() % 2 == 0) {
            let idx = (rng() as usize) % live.len();
            let (p, size, align, offset, tag) = live.swap_remove(idx);
            // SAFETY: tracked live block.
            unsafe {
                assert_eq!((p as usize + offset) % align, 0);
                assert_eq!(p.read(), tag);
                assert_eq!(p.add(size - 1).read(), tag);
                free(p);
            }
        } else {
            let size = 1 + (rng() as usize) % 20_000;
            let align = 1usize << (3 + (rng() as usize) % 12); // 8..16 KiB
            let offset = if rng() % 3 == 0 {
                (rng() as usize) % 256
            } else {
                0
            };
            let p = malloc_aligned_at(size, align, offset);
            assert!(!p.is_null());
            let tag = (i as u8) | 1;
            // SAFETY: fresh live block; usable ≥ size from p.
            unsafe {
                assert!(usable_size(p) >= size);
                p.write(tag);
                p.add(size - 1).write(tag);
            }
            live.push((p, size, align, offset, tag));
        }
    }
    for (p, size, align, offset, tag) in live.drain(..) {
        // SAFETY: tracked live blocks.
        unsafe {
            assert_eq!((p as usize + offset) % align, 0);
            assert_eq!(p.read(), tag);
            assert_eq!(p.add(size - 1).read(), tag);
            free(p);
        }
    }
}

#[test]
fn realloc_storm() {
    // Seeded randomized realloc churn with content verification — the M3
    // fuzz-shaped gate (cargo-fuzz targets extend this in CI later).
    let mut state = 0xC0FF_EE00_D15E_A5E5u64;
    let mut rng = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let iters: u64 = if cfg!(miri) { 1_500 } else { 60_000 };
    let mut live: Vec<(*mut u8, usize, u8)> = Vec::new();
    for i in 0..iters {
        match rng() % 10 {
            // realloc a random live block
            0..=3 if !live.is_empty() => {
                let idx = (rng() as usize) % live.len();
                let (p, size, tag) = live[idx];
                let grow = rng() % 3 != 0;
                let newsize = if grow {
                    (size * 2 + rng() as usize % 512).min(1 << 21)
                } else {
                    (size / 3).max(1)
                };
                // SAFETY: tracked live block; prefix checked below.
                unsafe {
                    let np = realloc(p, newsize);
                    assert!(!np.is_null());
                    for j in 0..size.min(newsize).min(64) {
                        assert_eq!(np.add(j).read(), tag, "storm: prefix lost at {j}");
                    }
                    let ntag = (i as u8) | 1;
                    core::ptr::write_bytes(np, ntag, newsize);
                    live[idx] = (np, newsize, ntag);
                }
            }
            // free
            4..=6 if !live.is_empty() => {
                let idx = (rng() as usize) % live.len();
                let (p, size, tag) = live.swap_remove(idx);
                // SAFETY: tracked live block.
                unsafe {
                    assert_eq!(p.read(), tag);
                    assert_eq!(p.add(size - 1).read(), tag);
                    free(p);
                }
            }
            // alloc
            _ => {
                if live.len() > 1200 {
                    continue;
                }
                let size = 1 + (rng() as usize) % 100_000;
                let p = malloc(size);
                assert!(!p.is_null());
                let tag = (i as u8) | 1;
                // SAFETY: fresh live block.
                unsafe { core::ptr::write_bytes(p, tag, size) };
                live.push((p, size, tag));
            }
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
