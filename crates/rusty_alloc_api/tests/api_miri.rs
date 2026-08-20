//! Miri-runnable exercises of the safe surface (H-23). The api crate's Miri
//! gate previously passed VACUOUSLY — zero tests executed — which the
//! hardening registry rules `Incomplete`. These run under
//! `cargo +nightly miri test -p rusty_alloc-api` and under the normal suites.

use rusty_alloc_api::{Heap, RustyAlloc};
use std::alloc::{GlobalAlloc, Layout};

#[test]
fn global_alloc_roundtrip_all_bins() {
    let a = RustyAlloc;
    // Small, medium, large, and an alignment above the word size.
    for (size, align) in [(1, 1), (8, 8), (57, 1), (1024, 16), (70_000, 8), (256, 64)] {
        let layout = Layout::from_size_align(size, align).unwrap();
        // SAFETY: layout is non-zero-size; each block is written within its
        // extent and freed exactly once with the same layout.
        unsafe {
            let p = a.alloc(layout);
            assert!(!p.is_null());
            assert_eq!(p.addr() % align, 0, "misaligned block");
            core::ptr::write_bytes(p, 0xAB, size);
            assert_eq!(*p, 0xAB);
            assert_eq!(*p.add(size - 1), 0xAB);
            a.dealloc(p, layout);
        }
    }
}

#[test]
fn global_alloc_zeroed_is_zero() {
    let a = RustyAlloc;
    let layout = Layout::from_size_align(300, 8).unwrap();
    // SAFETY: non-zero layout; block freed once.
    unsafe {
        let p = a.alloc_zeroed(layout);
        assert!(!p.is_null());
        for i in 0..300 {
            assert_eq!(*p.add(i), 0, "alloc_zeroed handed back a dirty byte");
        }
        a.dealloc(p, layout);
    }
}

#[test]
fn global_alloc_realloc_preserves_prefix() {
    let a = RustyAlloc;
    let layout = Layout::from_size_align(64, 8).unwrap();
    // SAFETY: non-zero layouts; the old pointer is consumed by realloc; the
    // final block is freed once with its current layout.
    unsafe {
        let p = a.alloc(layout);
        assert!(!p.is_null());
        core::ptr::write_bytes(p, 0x5C, 64);
        let np = a.realloc(p, layout, 4096);
        assert!(!np.is_null());
        for i in 0..64 {
            assert_eq!(*np.add(i), 0x5C, "realloc lost the prefix");
        }
        a.dealloc(np, Layout::from_size_align(4096, 8).unwrap());
    }
}

#[test]
fn first_class_heap_alloc_and_drop_migrates() {
    let h = Heap::new();
    let layout = Layout::from_size_align(128, 8).unwrap();
    let p = h.alloc(layout).expect("heap alloc failed");
    // SAFETY: freshly allocated block of 128 bytes.
    unsafe {
        core::ptr::write_bytes(p.as_ptr(), 0x77, 128);
    }
    drop(h); // delete-semantics: blocks migrate to the backing heap...
    // SAFETY: ...and stay valid; contents must have survived the migration,
    // and the block is freed through the global surface exactly once.
    unsafe {
        assert_eq!(*p.as_ptr(), 0x77);
        assert_eq!(*p.as_ptr().add(127), 0x77);
        RustyAlloc.dealloc(p.as_ptr(), layout);
    }
}

#[test]
fn destroyable_heap_releases_wholesale() {
    let h = Heap::new_destroyable();
    let layout = Layout::from_size_align(64, 8).unwrap();
    for _ in 0..32 {
        let p = h.alloc(layout).expect("heap alloc failed");
        // SAFETY: live block; never touched after the heap drops.
        unsafe { core::ptr::write_bytes(p.as_ptr(), 1, 64) };
    }
    drop(h); // destroy-semantics: everything released at once, no leak
}
