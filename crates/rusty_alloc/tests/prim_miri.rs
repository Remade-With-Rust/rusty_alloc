//! G4 shape: the os-layer logic runs under miri against the mock prim backend
//! (`cargo +nightly miri test --test prim_miri`). Native semantics (zeroing,
//! decommit faults, TLS destructors) are covered by `tests/prim.rs` instead —
//! the mock deliberately does not model them.

#![cfg(miri)]

use rusty_alloc::os;
use rusty_alloc::types::SEGMENT_SIZE;

#[test]
fn aligned_alloc_free_under_miri() {
    let b = os::alloc_aligned(64 * 1024, SEGMENT_SIZE, true, false).unwrap();
    assert_eq!((b.ptr as usize) % SEGMENT_SIZE, 0);
    // SAFETY: committed mock block; touch both ends so miri validates the range.
    unsafe {
        b.ptr.write(1);
        b.ptr.add(b.size - 1).write(2);
    }
    // SAFETY: freeing our own block.
    unsafe { os::free(b).unwrap() };
}

#[test]
fn page_rounding() {
    assert_eq!(os::page_align_up(1), os::page_size());
    assert_eq!(os::page_align_up(os::page_size()), os::page_size());
    assert_eq!(os::page_align_up(os::page_size() + 1), 2 * os::page_size());
}
