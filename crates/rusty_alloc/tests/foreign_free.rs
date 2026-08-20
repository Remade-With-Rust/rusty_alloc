//! The foreign-pointer guard must actually FIRE (hardening gate H-19,
//! residual risk R-001).
//!
//! `free` masks a pointer to its 32 MiB segment base and reads `slice_offset`
//! out of it. For a pointer this allocator returned that is correct by
//! construction; for a pointer belonging to something else it reads whatever
//! sits at that address and follows it — the mechanism behind the
//! mixed-allocator crashes this project has already documented. The guard
//! (`debug_foreign_pointer_guard`) catches that in debug and `debug_checks`
//! builds by asking the global segment map whether the address is inside a
//! window we own.
//!
//! **This test exists because a gate nobody has watched fail is not a gate.**
//! The suite passing merely shows the guard produces no FALSE POSITIVES; only
//! this shows it produces a true positive. The guard is a `debug_assert!`, so
//! tripping it panics — and a panic that unwinds through the allocator is not
//! something to do in-process, so the check runs in a CHILD (the same shape
//! `double_free.rs` uses for its abort).

use std::process::Command;

const MARKER: &str = "RUSTY_ALLOC_FOREIGN_FREE_CHILD";

/// The child: hand `free` an address the allocator never returned.
///
/// A static's address is ideal — it is a real, readable, mapped address (so
/// the failure mode under test is "wrong metadata", not "segfault on the
/// mask"), and it is guaranteed never to lie inside a segment we allocated.
fn child_foreign_free() -> ! {
    static NOT_OURS: [u64; 64] = [0; 64];
    let foreign = (&raw const NOT_OURS).cast::<u8>().cast_mut();

    // Allocate something first so the allocator is fully initialised and the
    // segment map is populated — otherwise an empty map would make this pass
    // for the wrong reason.
    let real = rusty_alloc::alloc::malloc(128);
    assert!(!real.is_null(), "child: malloc failed");

    // SAFETY: deliberately violating free's contract, which is the point of
    // the test. In this build configuration the guard fires BEFORE any
    // metadata is derived from the foreign address, so no invalid memory is
    // ever read; the process dies on the assertion instead.
    unsafe { rusty_alloc::alloc::free(foreign) };

    // Only reached if the guard did NOT fire.
    eprintln!("child: the foreign-pointer guard did not fire");
    std::process::exit(0);
}

#[test]
#[cfg_attr(miri, ignore)] // spawns a child process
fn foreign_pointer_is_rejected_in_debug_builds() {
    if std::env::var(MARKER).is_ok() {
        child_foreign_free();
    }

    // The guard is compiled in only for debug_assertions / debug_checks. In a
    // release build without `debug_checks` there is nothing to test, and
    // saying so is more honest than a vacuous pass.
    if !cfg!(any(debug_assertions, feature = "debug_checks")) {
        eprintln!("skipped: guard is debug/debug_checks-only, and this is a release build");
        return;
    }

    let exe = std::env::current_exe().expect("current_exe");
    let out = Command::new(exe)
        .env(MARKER, "1")
        .arg("--test-threads=1")
        .output()
        .expect("spawn child");

    // BOTH streams: libtest CAPTURES a panicking test's output and reprints it
    // under "---- <test> stdout ----", so the assertion message lands on the
    // child's STDOUT even though a bare panic would go to stderr. Reading only
    // stderr made this test report "died for an unrelated reason" while the
    // guard was in fact working perfectly — the instrument was wrong, not the
    // code under test.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        !out.status.success(),
        "the child exited successfully: free() ACCEPTED a foreign pointer. \
         The guard is not protecting the free path.\n{combined}"
    );
    assert!(
        combined.contains("never returned"),
        "child died, but not on the foreign-pointer assertion — the test may be \
         measuring an unrelated failure.\n{combined}"
    );
}
