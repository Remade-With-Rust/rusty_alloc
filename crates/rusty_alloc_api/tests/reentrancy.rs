//! The allocator must never allocate THROUGH the global allocator while it is
//! serving a request.
//!
//! It did: the options environment pass in `options.rs` built its keys with
//! `to_uppercase` and `format!` and read them with `std::env::var`, all of
//! which return owned `String`s — **251 allocations re-entered the global
//! allocator on the first allocation of every process** on Windows (fewer on
//! Linux, where `std::env::var` allocates less per call), each landing in the
//! heap that was still being set up. mimalloc makes none. Reported from a
//! consumer behind a counting `GlobalAlloc` (`docs/plans/youslowbro.md` §4);
//! this test is that instrument, made permanent.
//!
//! The wrapper counts a call as re-entrant when it arrives while the same
//! thread is already inside one — a per-thread depth, so concurrent threads
//! cannot produce a false positive. The counter is cumulative from process
//! start, so the harness's own first allocation (which runs the pass) is
//! covered without any ordering assumption about which test runs first.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use rusty_alloc_api::RustyAlloc;

static CALLS: AtomicUsize = AtomicUsize::new(0);
static REENTRANT: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    // `const` and `Copy`: no lazy initialisation and no destructor, so reading
    // it inside the allocator allocates nothing and cannot fail at thread exit.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

struct Counting;

impl Counting {
    /// Enter one allocator call; `true` when this thread was already inside one.
    fn enter() -> bool {
        CALLS.fetch_add(1, Relaxed);
        let nested = DEPTH.with(|d| {
            let n = d.get();
            d.set(n + 1);
            n > 0
        });
        if nested {
            REENTRANT.fetch_add(1, Relaxed);
        }
        nested
    }

    fn leave() {
        DEPTH.with(|d| d.set(d.get() - 1));
    }
}

// SAFETY: every method forwards to `RustyAlloc` with the same arguments and
// returns its result unchanged; the bookkeeping around the call touches only
// atomics and a `Copy` thread-local.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        Counting::enter();
        // SAFETY: forwarded GlobalAlloc contract.
        let p = unsafe { RustyAlloc.alloc(l) };
        Counting::leave();
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        Counting::enter();
        // SAFETY: forwarded GlobalAlloc contract.
        unsafe { RustyAlloc.dealloc(p, l) };
        Counting::leave();
    }

    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        Counting::enter();
        // SAFETY: forwarded GlobalAlloc contract.
        let p = unsafe { RustyAlloc.alloc_zeroed(l) };
        Counting::leave();
        p
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        Counting::enter();
        // SAFETY: forwarded GlobalAlloc contract.
        let q = unsafe { RustyAlloc.realloc(p, l, n) };
        Counting::leave();
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn the_allocator_never_re_enters_the_global_allocator() {
    // Walk every regime once so the paths that set state up have all run on
    // this thread: a fresh heap's first small block, a medium page, a large
    // span, a dedicated huge segment, a moving realloc, and a cross-thread
    // free (a worker's heap is created, used and abandoned).
    let small = black_box(vec![1u8; 64]);
    let medium = black_box(vec![2u8; 8 << 10]);
    let large = black_box(vec![3u8; 300 << 10]);
    #[cfg(not(miri))]
    let huge = black_box(vec![4u8; 33 << 20]);
    let mut grown: Vec<u64> = Vec::with_capacity(8);
    grown.extend(0..(1 << 16));
    let from_worker = std::thread::spawn(|| black_box(vec![5u8; 4096]))
        .join()
        .expect("worker");
    drop((small, medium, large, black_box(grown), from_worker));
    #[cfg(not(miri))]
    drop(huge);

    let calls = CALLS.load(Relaxed);
    let reentrant = REENTRANT.load(Relaxed);
    println!("global allocator calls so far: {calls}, re-entrant: {reentrant}");
    assert!(
        calls > 0,
        "the counting wrapper is not the global allocator"
    );
    assert_eq!(
        reentrant, 0,
        "rusty_alloc allocated through the global allocator {reentrant} times \
         while serving a request — the start-up options pass used to do this \
         251 times (docs/plans/youslowbro.md §4)"
    );
}
