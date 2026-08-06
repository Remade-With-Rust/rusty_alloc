//! Double-free detection must ABORT, not corrupt.
//!
//! Without the check in `page_push_local`, freeing a block twice wraps the
//! page's `used` counter to `u32::MAX`: the page never retires and the same
//! block sits on the free list twice, so a later pair of allocations hand the
//! SAME memory to two owners. That is silent heap corruption, and it is the
//! precise failure this allocator exists to prevent.
//!
//! Testing an abort needs a child process — the abort kills whoever runs it —
//! so the test re-executes its own binary with a marker variable set.

use std::process::Command;

const MARKER: &str = "RUSTY_ALLOC_DOUBLE_FREE_CHILD";

/// The child: free the same block twice and expect never to return.
fn child_double_free() -> ! {
    let p = rusty_alloc::alloc::malloc(64);
    assert!(!p.is_null(), "child: malloc failed");
    // SAFETY: p is live and ours.
    unsafe { rusty_alloc::alloc::free(p) };
    // SAFETY: DELIBERATELY WRONG — this is the bug under test. The allocator
    // must abort here rather than return.
    unsafe { rusty_alloc::alloc::free(p) };
    // If we get here the detection did not fire. Exit with a code the parent
    // can distinguish from an abort.
    eprintln!("child: second free RETURNED — double free was not detected");
    std::process::exit(97);
}

// Miri cannot run this: it needs `current_exe()` (a `readlink`, blocked by
// Miri's isolation) and a child process, which Miri cannot spawn at all. The
// detection it covers is plain integer arithmetic — nothing Miri would have
// checked — so skipping costs no coverage.
#[cfg_attr(miri, ignore)]
#[test]
fn double_free_aborts_instead_of_corrupting() {
    if std::env::var(MARKER).is_ok() {
        child_double_free();
    }

    let exe = std::env::current_exe().expect("current_exe");
    let out = Command::new(exe)
        .env(MARKER, "1")
        // Run only this test in the child, and don't let the harness capture
        // its own abort.
        .args(["--exact", "double_free_aborts_instead_of_corrupting"])
        .output()
        .expect("spawn child");

    assert!(
        !out.status.success(),
        "child exited successfully — a double free was accepted silently"
    );
    assert_ne!(
        out.status.code(),
        Some(97),
        "the second free RETURNED: detection did not fire"
    );
}
