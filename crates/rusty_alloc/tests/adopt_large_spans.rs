//! A segment abandoned with live LARGE blocks, whose blocks are then freed
//! from another thread, must be fully reusable once adopted.
//!
//! `docs/plans/huge-free-retention.md`, mechanism 1: a model loaded on one
//! thread, which then exited, and dropped on another kept ~0.6 of the model's
//! size resident and unreusable, whatever `purge_delay` said. Adoption retired
//! ONE dead large span per segment and left the rest to "later collects", but
//! a large span is never queued, so no collect ever reached it again.
//!
//! Own test binary: it reasons about which segments exist process-wide, and a
//! parallel test abandoning or adopting segments would muddy that.

use rusty_alloc::alloc::{free, malloc, stats};
use rusty_alloc::segment::USABLE_SLICES;
use rusty_alloc::types::SEGMENT_SLICE_SIZE;
use std::thread;

/// A large (single-block) span at EVERY geometry: two slices is past
/// `MEDIUM_OBJ_SIZE_MAX` everywhere this crate builds (`tests/spans.rs`). A
/// literal 2.5 MB was a large span at the shipped 32 MiB segment and a HUGE
/// block, one segment each, under `ra_small_profile` and `ra_segment_size =
/// "256k"`, where this test then failed for a reason unrelated to its subject.
const SIZE: usize = 2 * SEGMENT_SLICE_SIZE;
/// Enough to FILL one segment (255 at the shipped geometry, 3 at the small
/// profile), like a model's weight matrices. Filling it is what makes the test
/// bite: with room left at the bump frontier, a reload fits even when the
/// adoption retired only one dead span, and the old behaviour would pass.
const N: usize = USABLE_SLICES / 2;

fn alloc_all() -> Vec<usize> {
    (0..N)
        .map(|_| {
            let p = malloc(SIZE);
            assert!(!p.is_null());
            // SAFETY: a live block of SIZE bytes; touch it like loaded weights.
            unsafe { core::ptr::write_bytes(p, 0x5A, SIZE) };
            p as usize
        })
        .collect()
}

// Not under Miri: the test needs a thread's EXIT to abandon its segments, and
// Miri runs against the mock prim, whose thread-exit (TLS destructor) hooks
// never fire (`prim/mock.rs`: "TLS destructors do not fire"). There the
// loader's segments are never abandoned, nothing adopts them, and the premise
// never occurs; Miri reported an assertion, not undefined behaviour.
#[cfg_attr(miri, ignore)]
#[test]
fn an_adopted_segment_gives_back_every_dead_large_span() {
    // The loader allocates, then exits: its segment is abandoned with every
    // block still live.
    let blocks = thread::spawn(alloc_all).join().unwrap();
    // Another thread drops the "model": frees into the abandoned segment.
    for p in blocks {
        // SAFETY: live blocks from `alloc_all`, each freed once.
        unsafe { free(p as *mut u8) };
    }
    // A third thread loads the next one. It adopts the orphan, which now
    // holds nothing live, so it needs no fresh segment at all.
    let fresh = thread::spawn(|| {
        let before = stats().segments;
        let blocks = alloc_all();
        let fresh = stats().segments - before;
        for p in blocks {
            // SAFETY: as above.
            unsafe { free(p as *mut u8) };
        }
        fresh
    })
    .join()
    .unwrap();
    assert_eq!(
        fresh, 0,
        "the adopted segment's dead large spans were not all retired, so the \
         next load took {fresh} fresh segment(s) while their memory sat unused"
    );
}
