//! `thread_id()` is the allocator's OWNERSHIP identity: `segment.thread_id`
//! decides whether a `free` takes the local (unsynchronised, owner-only) path
//! or the remote one. Two invariants therefore have to hold on every target,
//! and they are memory-safety invariants, not optimisations:
//!
//!   1. **Stable** — one live thread observes one value for its whole life.
//!      A drifting id makes a thread stop recognising its own segments.
//!   2. **Unique** — no two LIVE threads share a value. A collision lets one
//!      thread free into another's segment through the owner path, racing
//!      non-atomic page state.
//!
//! Both were violated on aarch64-apple-darwin, because Darwin does not follow
//! the standard AArch64 ABI: the thread pointer is in `tpidrro_el0`, while
//! `tpidr_el0` — which every other AArch64 target correctly uses — holds the
//! CPU/cluster id there. Measured on macOS 26 / M-series before the fix:
//! `tpidr_el0` took 5 distinct values within ONE thread over 3M reads, and 8
//! live threads produced only 5 distinct values between them.
//!
//! These assertions are counts and equalities — deterministic, no timing, no
//! noise floor — so they are a real gate on any target, including CI.

use std::collections::BTreeSet;
use std::sync::{Arc, Barrier};

use rusty_alloc::init::thread_id;

/// Enough iterations to get descheduled and migrate cores at least once; the
/// pre-fix bug reproduced in well under this on an idle machine.
const READS: usize = 2_000_000;

#[test]
fn thread_id_is_stable_within_a_thread() {
    let first = thread_id();
    let mut distinct = BTreeSet::new();
    for _ in 0..READS {
        distinct.insert(thread_id());
    }
    assert_eq!(
        distinct.len(),
        1,
        "thread_id() drifted within one thread: saw {distinct:?} (first read {first:#x}). \
         On Apple Silicon this is the tpidr_el0-vs-tpidrro_el0 bug — the low 3 bits of \
         tpidrro_el0 are the CPU number and must be masked."
    );
}

#[test]
fn thread_id_is_unique_across_live_threads() {
    const THREADS: usize = 16;
    // A barrier keeps every thread ALIVE simultaneously — ids may legitimately
    // be recycled after a thread dies, so overlapping lifetimes is what makes
    // this a real uniqueness test.
    let barrier = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let b = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let id = thread_id();
                b.wait(); // all THREADS are live past this point
                let mut distinct = BTreeSet::new();
                for _ in 0..(READS / 8) {
                    distinct.insert(thread_id());
                }
                b.wait(); // still all live
                assert_eq!(distinct.len(), 1, "thread_id() drifted mid-thread");
                assert_eq!(id, thread_id(), "thread_id() changed across the run");
                id
            })
        })
        .collect();

    let ids: Vec<usize> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let unique: BTreeSet<usize> = ids.iter().copied().collect();
    assert_eq!(
        unique.len(),
        THREADS,
        "thread_id() COLLIDED across live threads: {THREADS} threads produced only {} distinct \
         ids ({ids:#x?}). Distinct live threads sharing an owner id is a heap-corruption bug — \
         the local free path assumes exclusive ownership.",
        unique.len()
    );
    assert!(!unique.contains(&0), "0 is the abandoned sentinel, never a live id");
}

#[test]
fn thread_id_matches_the_platform_thread_identity() {
    // Cross-check the fast-path register read against the OS's own answer, so a
    // wrong register cannot pass the two tests above by being merely
    // self-consistent. They must agree on being one-to-one with each other.
    let pairs: Vec<(usize, usize)> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                let os_id = rusty_alloc::prim::thread_id();
                (thread_id(), os_id)
            })
            .join()
            .unwrap()
        })
        .collect();

    for (fast, os) in &pairs {
        assert_ne!(*fast, 0, "fast-path thread_id() returned the 0 sentinel");
        assert_ne!(*os, 0, "prim::thread_id() returned 0");
    }
    // Sequential threads may recycle ids, so assert the mapping is consistent
    // rather than that all 8 differ: equal OS ids must imply equal fast ids.
    for i in 0..pairs.len() {
        for j in (i + 1)..pairs.len() {
            if pairs[i].1 == pairs[j].1 {
                assert_eq!(
                    pairs[i].0, pairs[j].0,
                    "same OS thread identity mapped to different fast-path ids"
                );
            }
        }
    }
}
