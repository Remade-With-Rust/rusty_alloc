//! A dying thread must NOT adopt abandoned segments.
//!
//! REGRESSION TEST for the 0.3.1 segfault (FFAI, 6/8 runs, reproducible with
//! five JPEG decodes). `thread_done` and `heap_delete` both call a forced
//! collect. Once `force` started reclaiming orphans, a thread on its way out
//! would adopt every abandoned segment from previously-dead threads into the
//! heap it was about to destroy — re-homing their pages onto a `DelayedList`
//! freed moments later.
//!
//! The shape that matters is: some threads exit leaving orphans behind, THEN
//! more threads start and exit. FFAI saw it get RARER with more threads (live
//! heaps adopt the orphans first, leaving fewer for a dying one to take).
//!
//! ⚠ THIS TEST DOES NOT YET REPRODUCE THE CRASH. Reintroducing the bug
//! deliberately (`collect_inner(true, true)` in `collect_for_teardown`) still
//! passes 4/4 here, so it does NOT guard the regression and must not be
//! treated as if it does. The missing ingredient is almost certainly
//! CROSS-THREAD frees: every thread below frees its own blocks, so no remote
//! thread is pushing onto the dying heap's `DelayedList` — which is what makes
//! a re-homed `xheap` pointer a live target. Extend it that way before
//! trusting it.

use rusty_alloc::alloc::{free, malloc};

const WAVES: usize = 6;
const PER_WAVE: usize = 4;

/// Allocate spans and leak a few, so this thread's segment cannot retire and
/// must be ABANDONED when the thread exits.
fn leaky_worker() {
    let mut v = Vec::with_capacity(256);
    for i in 0..256 {
        let n = 8192 + (i * 1013) % 131_072;
        let p = malloc(n);
        assert!(!p.is_null());
        // SAFETY: n bytes we own; touch so the pages are real.
        unsafe { core::ptr::write_bytes(p, 0x33, n) };
        v.push(p);
    }
    for p in v.drain(..248) {
        // SAFETY: allocated above, freed exactly once.
        unsafe { free(p) };
    }
    // The remaining 8 leak on purpose: a segment with live blocks cannot
    // retire, so thread exit must abandon it.
    core::mem::forget(v);
}

// Miri-ignored: leaks on purpose (see above), and Miri's leak checker is right
// to object. The bug this guards is a use-after-free at thread teardown, which
// the native run exercises.
#[cfg_attr(miri, ignore)]
#[test]
fn dying_thread_does_not_adopt_orphans() {
    // Wave 0 seeds the abandoned list. Every later wave has orphans waiting
    // when its threads exit — which is exactly when 0.3.1 crashed.
    for _ in 0..WAVES {
        let hs: Vec<_> = (0..PER_WAVE)
            .map(|_| std::thread::spawn(leaky_worker))
            .collect();
        for h in hs {
            h.join().unwrap();
        }
    }

    // Surviving this far is the assertion: under 0.3.1 a thread exiting with
    // orphans present adopted them into its own dying heap and the process
    // faulted. Allocate afterwards to confirm the allocator is still usable.
    let mut keep = Vec::new();
    for i in 0..1024 {
        let p = malloc(64 + (i * 37) % 8192);
        assert!(!p.is_null(), "allocator unusable after thread-exit waves");
        keep.push(p);
    }
    for p in keep {
        // SAFETY: allocated just above, freed once.
        unsafe { free(p) };
    }
}
