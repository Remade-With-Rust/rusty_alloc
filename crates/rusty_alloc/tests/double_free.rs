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

const ABANDON_MARKER: &str = "RUSTY_ALLOC_ABANDON_DOUBLE_FREE_CHILD";

/// A block abandoned by a dying thread, then freed twice from another thread.
/// NEVER-page `remote_free` used to push the same block onto `xthread_free`
/// twice and return (OH-rusty_alloc-11).
#[cfg_attr(miri, ignore)]
#[test]
fn abandoned_double_free_aborts() {
    if std::env::var(ABANDON_MARKER).is_ok() {
        let addr = std::thread::spawn(|| {
            let p = rusty_alloc::alloc::malloc(192);
            assert!(!p.is_null());
            p.expose_provenance()
        })
        .join()
        .expect("allocator thread");
        let p = core::ptr::with_exposed_provenance_mut::<u8>(addr);
        // SAFETY: `p` is live; the SECOND call is the bug under test, and the
        // allocator must abort there rather than return.
        unsafe {
            rusty_alloc::alloc::free(p);
            rusty_alloc::alloc::free(p);
        }
        eprintln!("child: second free of abandoned block RETURNED");
        std::process::exit(97);
    }

    let exe = std::env::current_exe().expect("current_exe");
    let out = Command::new(exe)
        .env(ABANDON_MARKER, "1")
        .args(["--exact", "abandoned_double_free_aborts"])
        .output()
        .expect("spawn child");
    assert!(
        !out.status.success(),
        "abandoned double free was accepted silently"
    );
    assert_ne!(
        out.status.code(),
        Some(97),
        "the second free of an abandoned block RETURNED"
    );
}

const SET_DEFAULT_ABANDON_MARKER: &str = "RUSTY_ALLOC_SET_DEFAULT_ABANDON_DF_CHILD";
const FIRST_CLASS_ABANDON_MARKER: &str = "RUSTY_ALLOC_FIRST_CLASS_ABANDON_DF_CHILD";

/// `set_default_heap` then thread exit: the installed heap must be abandoned
/// (NEVER), so a second remote free aborts (OH-rusty_alloc-13).
#[cfg_attr(miri, ignore)]
#[test]
fn set_default_heap_abandoned_double_free_aborts() {
    if std::env::var(SET_DEFAULT_ABANDON_MARKER).is_ok() {
        let addr = std::thread::spawn(|| {
            let h = rusty_alloc::init::create_heap(0, false, -1);
            assert!(!h.is_null());
            // SAFETY: `h` is a live heap this thread just created and owns.
            unsafe {
                let _prev = rusty_alloc::init::set_default_heap(h);
                let p = rusty_alloc::alloc::malloc(80);
                assert!(!p.is_null());
                p.expose_provenance()
            }
        })
        .join()
        .expect("set_default thread");
        let p = core::ptr::with_exposed_provenance_mut::<u8>(addr);
        // SAFETY: `p` is live; the SECOND call is the bug under test, and the
        // allocator must abort there rather than return.
        unsafe {
            rusty_alloc::alloc::free(p);
            rusty_alloc::alloc::free(p);
        }
        eprintln!("child: second free after set_default_heap+exit RETURNED");
        std::process::exit(97);
    }

    let exe = std::env::current_exe().expect("current_exe");
    let out = Command::new(exe)
        .env(SET_DEFAULT_ABANDON_MARKER, "1")
        .args(["--exact", "set_default_heap_abandoned_double_free_aborts"])
        .output()
        .expect("spawn child");
    assert!(
        !out.status.success(),
        "set_default abandoned double free was accepted silently"
    );
    assert_ne!(
        out.status.code(),
        Some(97),
        "the second free after set_default_heap+exit RETURNED"
    );
}

/// First-class heap leaked at thread exit (never `heap_delete`): second remote
/// free must abort (OH-rusty_alloc-13 sibling of the default-heap path).
#[cfg_attr(miri, ignore)]
#[test]
fn first_class_heap_abandoned_double_free_aborts() {
    if std::env::var(FIRST_CLASS_ABANDON_MARKER).is_ok() {
        let addr = std::thread::spawn(|| {
            let h = rusty_alloc::init::create_heap(0, false, -1);
            assert!(!h.is_null());
            // SAFETY: `h` is a live heap this thread just created and owns.
            let p = unsafe { rusty_alloc::alloc::heap_malloc(h, 64) };
            assert!(!p.is_null());
            p.expose_provenance()
        })
        .join()
        .expect("first-class heap thread");
        let p = core::ptr::with_exposed_provenance_mut::<u8>(addr);
        // SAFETY: `p` is live; the SECOND call is the bug under test, and the
        // allocator must abort there rather than return.
        unsafe {
            rusty_alloc::alloc::free(p);
            rusty_alloc::alloc::free(p);
        }
        eprintln!("child: second free of first-class abandoned block RETURNED");
        std::process::exit(97);
    }

    let exe = std::env::current_exe().expect("current_exe");
    let out = Command::new(exe)
        .env(FIRST_CLASS_ABANDON_MARKER, "1")
        .args(["--exact", "first_class_heap_abandoned_double_free_aborts"])
        .output()
        .expect("spawn child");
    assert!(
        !out.status.success(),
        "first-class abandoned double free was accepted silently"
    );
    assert_ne!(
        out.status.code(),
        Some(97),
        "the second free of a first-class abandoned block RETURNED"
    );
}
