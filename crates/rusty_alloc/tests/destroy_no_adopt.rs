//! A heap that can be destroyed must never adopt an abandoned segment.
//!
//! `heap_destroy` releases every segment the heap owns. A segment a thread
//! left behind when it exited still holds whatever of its blocks are alive,
//! and those belong to whoever they were handed to. If a destroyable heap
//! adopts such a segment, destroying the heap unmaps blocks that are in use.
//!
//! It did, until 2.6.0, and nothing noticed until the `Allocator` conformance
//! suite ran in a release build with a destroyable heap and a
//! cross-thread hand-off in one process: a segmentation fault in about one run
//! in three. This test does not wait for the fault. It asks the heap, before
//! destroying it, whether it owns a dead thread's block; the answer has to be
//! no. Alone in its test binary so nothing else takes the orphans first.
//!
//! Native only, as every adoption test here is: under miri's mock backend a
//! thread's exit hook never runs, so no segment is ever abandoned and there is
//! nothing to adopt or to refuse.

use rusty_alloc::{alloc, init};
use std::sync::Mutex;

/// One test at a time, whatever the harness is told. The tests share the
/// process's abandoned list: run together, the control test's ordinary heap
/// could take another test's orphans before its destroyable heap had the
/// chance to, and a heap that adopted nothing because there was nothing left
/// proves nothing.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// A block handed from the thread that allocated it to the test.
struct Orphan(*mut u8, usize);
// SAFETY: a heap block is plain memory; it is used by one thread at a time,
// the worker that fills it and then the test that reads and frees it.
unsafe impl Send for Orphan {}

const fn scale(native: usize, miri: usize) -> usize {
    if cfg!(miri) { miri } else { native }
}

/// Threads that each allocate, hand their blocks out and exit: every one
/// leaves a segment abandoned with live blocks in it.
fn orphans() -> Vec<Vec<Orphan>> {
    // Collected on purpose: the workers run together, so each has a segment
    // of its own. One after another, the second would adopt what the first
    // left and there would be one orphan segment, not eight.
    #[allow(clippy::needless_collect)]
    let handles: Vec<_> = (0..scale(8, 2))
        .map(|t| {
            std::thread::spawn(move || {
                (0..scale(64, 8))
                    .map(|i| {
                        let size = 64 + 16 * i;
                        let p = alloc::malloc(size);
                        assert!(!p.is_null());
                        // SAFETY: a fresh block of `size` bytes.
                        unsafe { p.write_bytes(t as u8 + 1, size) };
                        Orphan(p, size)
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    handles
        .into_iter()
        .map(|h| h.join().expect("worker"))
        .collect()
}

/// Every orphan still holds what its thread wrote, and can be freed.
fn check_and_free(orphans: Vec<Vec<Orphan>>) {
    for (t, blocks) in orphans.into_iter().enumerate() {
        for Orphan(p, size) in blocks {
            // SAFETY: the block is live: nothing has freed it, and no heap
            // that was destroyed owned its segment (the test's whole claim).
            unsafe {
                assert_eq!(p.read(), t as u8 + 1, "an orphan's first byte changed");
                assert_eq!(
                    p.add(size - 1).read(),
                    t as u8 + 1,
                    "an orphan's last byte changed"
                );
                alloc::free(p);
            }
        }
    }
}

fn owns_any(hb: *mut init::HeapBox, orphans: &[Vec<Orphan>]) -> Option<usize> {
    orphans.iter().position(|blocks| {
        blocks
            .iter()
            // SAFETY: `hb` is this thread's live heap; the pointer is only
            // compared by segment address.
            .any(|o| unsafe { alloc::heap_contains_block(hb, o.0) })
    })
}

/// Needing a span is when a heap adopts: before it reserves fresh memory it
/// takes what dead threads left. A destroyable heap must not.
#[test]
#[cfg_attr(
    miri,
    ignore = "the miri mock's thread-exit destructors never fire, so nothing is abandoned"
)]
fn a_destroyable_heap_does_not_adopt_when_it_needs_a_span() {
    let _serial = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let orphans = orphans();
    let hb = init::create_heap(0, true, -1);
    assert!(!hb.is_null());
    // SAFETY: `hb` is a live heap of this thread until `heap_destroy`; its
    // blocks are not used after that.
    unsafe {
        // Many sizes, so the heap needs a span many times over.
        for i in 0..scale(2000, 200) {
            assert!(!alloc::heap_malloc(hb, 32 + (i % 40) * 48).is_null());
        }
        if let Some(t) = owns_any(hb, &orphans) {
            panic!("the destroyable heap adopted thread {t}'s segment, live blocks and all");
        }
        init::heap_destroy(hb);
    }
    check_and_free(orphans);
}

/// A forced collect is the other place a heap adopts.
#[test]
#[cfg_attr(
    miri,
    ignore = "the miri mock's thread-exit destructors never fire, so nothing is abandoned"
)]
fn a_destroyable_heap_does_not_adopt_in_a_collect() {
    let _serial = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let orphans = orphans();
    let hb = init::create_heap(0, true, -1);
    assert!(!hb.is_null());
    // SAFETY: as the test above.
    unsafe {
        assert!(!alloc::heap_malloc(hb, 100).is_null());
        alloc::heap_collect(hb, true);
        if let Some(t) = owns_any(hb, &orphans) {
            panic!("a collect on the destroyable heap adopted thread {t}'s segment");
        }
        init::heap_destroy(hb);
    }
    check_and_free(orphans);
}

/// The control: an ordinary heap DOES adopt, which is what the two tests
/// above would be vacuous without. If nothing ever adopted here, "it did not
/// adopt" would prove nothing.
#[test]
#[cfg_attr(
    miri,
    ignore = "the miri mock's thread-exit destructors never fire, so nothing is abandoned"
)]
fn an_ordinary_heap_still_adopts() {
    let _serial = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let orphans = orphans();
    let hb = init::create_heap(0, false, -1);
    assert!(!hb.is_null());
    // SAFETY: `hb` is a live heap of this thread until `heap_delete`.
    unsafe {
        assert!(!alloc::heap_malloc(hb, 100).is_null());
        alloc::heap_collect(hb, true);
        assert!(
            owns_any(hb, &orphans).is_some(),
            "an ordinary heap adopted nothing: this binary's other tests prove nothing"
        );
        // Deleting a heap hands its blocks on; the orphans stay valid.
        init::heap_delete(hb);
    }
    check_and_free(orphans);
}
