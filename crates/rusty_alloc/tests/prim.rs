//! M1 gate: native integration tests of the prim/os layer (plan §8 M1).
//! These exercise the REAL OS backend — they are skipped under miri, where
//! `tests/prim_miri.rs` runs the same shapes against the mock.

#![cfg(not(miri))]

use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rusty_alloc::types::SEGMENT_SIZE;
use rusty_alloc::{os, prim};

#[test]
fn segment_aligned_alloc() {
    // The load-bearing requirement: 32 MiB-aligned segment reservations
    // (ptr → segment is a mask, so alignment IS the addressing scheme).
    let b = os::alloc_aligned(2 * 1024 * 1024, SEGMENT_SIZE, true, false).unwrap();
    assert_eq!(
        (b.ptr as usize) % SEGMENT_SIZE,
        0,
        "segment alignment violated"
    );
    assert!(b.size >= 2 * 1024 * 1024);
    // Touch first and last byte — the mapping must be real, writable memory.
    // SAFETY: b covers [ptr, ptr+size), committed above.
    unsafe {
        b.ptr.write(0xAB);
        b.ptr.add(b.size - 1).write(0xCD);
        assert_eq!(b.ptr.read(), 0xAB);
    }
    // SAFETY: freeing the block we just allocated, no live refs remain.
    unsafe { os::free(b).unwrap() };
}

#[test]
fn fresh_commit_is_zero() {
    let b = os::alloc_aligned(1024 * 1024, os::page_size(), true, false).unwrap();
    assert!(b.is_zero, "prim must report fresh mappings zero");
    // SAFETY: committed range of b; read-only walk.
    unsafe {
        for i in (0..b.size).step_by(4096) {
            assert_eq!(b.ptr.add(i).read(), 0, "fresh page not zero at +{i}");
        }
    }
    // SAFETY: freeing our own block.
    unsafe { os::free(b).unwrap() };
}

#[test]
fn reserve_commit_decommit_cycle() {
    // Reserve without commit, commit a page, write, decommit, recommit if the
    // platform demands it, and verify the contents came back zero.
    let ps = os::page_size();
    let b = os::alloc_aligned(16 * ps, ps, false, false).unwrap();
    let p = b.ptr;
    // SAFETY: p..p+ps lies in our reservation; commit makes it accessible.
    unsafe {
        os::commit(p, ps).unwrap();
        p.write(42);
        assert_eq!(p.read(), 42);
        let needs_recommit = os::decommit(p, ps).unwrap();
        if needs_recommit {
            os::commit(p, ps).unwrap();
        }
        // Windows: fresh recommit is zero. Linux MADV_DONTNEED: zero on touch.
        assert_eq!(p.read(), 0, "decommitted page kept its contents");
    }
    // SAFETY: freeing our own block.
    unsafe { os::free(b).unwrap() };
}

#[test]
fn purge_reset_keeps_accessible() {
    let ps = os::page_size();
    let b = os::alloc_aligned(4 * ps, ps, true, false).unwrap();
    // SAFETY: committed range of b.
    unsafe {
        b.ptr.write(7);
        // reset policy: contents undefined afterwards, but the page must not fault.
        os::purge(b.ptr, ps, false).unwrap();
        let _ = b.ptr.read(); // must not crash; value is undefined
        b.ptr.write(9); // must be writable again
        assert_eq!(b.ptr.read(), 9);
    }
    // SAFETY: freeing our own block.
    unsafe { os::free(b).unwrap() };
}

#[test]
fn protect_roundtrip() {
    let ps = os::page_size();
    let b = os::alloc_aligned(ps, ps, true, false).unwrap();
    // SAFETY: committed range of b; we only touch it while unprotected.
    unsafe {
        os::protect(b.ptr, ps, true).unwrap();
        os::protect(b.ptr, ps, false).unwrap();
        b.ptr.write(1);
        assert_eq!(b.ptr.read(), 1);
    }
    // SAFETY: freeing our own block.
    unsafe { os::free(b).unwrap() };
}

#[test]
fn alignment_invariant_sweep() {
    // Bounded randomized sweep (xorshift, no deps): sizes × alignments, every
    // block aligned + writable. The cargo-fuzz targets extend this in CI later.
    let mut state = 0x243F_6A88_85A3_08D3u64; // seed: pi digits, fixed for determinism
    let mut rng = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..50 {
        let size = 1 + (rng() as usize % (2 * 1024 * 1024));
        let align_pow = 12 + (rng() as usize % 14); // 4 KiB ..= 32 MiB
        let align = 1usize << align_pow;
        let b = os::alloc_aligned(size, align, true, false).unwrap();
        assert_eq!((b.ptr as usize) % align, 0, "size={size} align={align}");
        assert_eq!(b.size % os::page_size(), 0);
        assert!(b.size >= size);
        // SAFETY: committed block; touch both ends.
        unsafe {
            b.ptr.write(1);
            b.ptr.add(b.size - 1).write(2);
        }
        // SAFETY: freeing our own block.
        unsafe { os::free(b).unwrap() };
    }
}

#[test]
fn thread_ids_differ_and_clock_advances() {
    let main_id = prim::thread_id();
    let other_id = std::thread::spawn(prim::thread_id).join().unwrap();
    assert_ne!(main_id, 0);
    assert_ne!(main_id, other_id, "thread ids must differ across threads");

    let t0 = prim::clock_now();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let t1 = prim::clock_now();
    assert!(t1 > t0, "monotonic clock did not advance");
    assert!(
        t1 - t0 >= 1_000_000,
        "10ms sleep advanced < 1ms — wrong clock scale?"
    );
}

static DTOR_RAN: AtomicBool = AtomicBool::new(false);
static DTOR_VALUE: AtomicUsize = AtomicUsize::new(0);

#[cfg(windows)]
unsafe extern "system" fn tls_dtor(v: *const c_void) {
    DTOR_VALUE.store(v as usize, Ordering::SeqCst);
    DTOR_RAN.store(true, Ordering::SeqCst);
}

#[cfg(not(windows))]
unsafe extern "C" fn tls_dtor(v: *mut c_void) {
    DTOR_VALUE.store(v as usize, Ordering::SeqCst);
    DTOR_RAN.store(true, Ordering::SeqCst);
}

#[test]
fn tls_dtor_runs_at_thread_exit() {
    // The M4 keystone: mi_thread_done hangs off this exact mechanism.
    let slot = prim::TlsSlot::new(Some(tls_dtor)).expect("out of TLS slots");
    std::thread::spawn(move || {
        slot.set(0x1234 as *mut c_void);
        assert_eq!(slot.get() as usize, 0x1234);
    })
    .join()
    .unwrap();
    assert!(
        DTOR_RAN.load(Ordering::SeqCst),
        "TLS destructor did not fire at thread exit"
    );
    assert_eq!(
        DTOR_VALUE.load(Ordering::SeqCst),
        0x1234,
        "dtor got the wrong value"
    );
}

#[test]
fn numa_reports_at_least_one_node() {
    assert!(prim::numa_node_count() >= 1);
}
