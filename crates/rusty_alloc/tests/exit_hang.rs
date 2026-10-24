//! A process must EXIT even when another thread died holding an allocator lock.
//!
//! REGRESSION TEST for the 2.2.6 exit hang (ArtCraft, DesignCraft CLI, 1 run in
//! ~260, one core, a loaded box). `ExitProcess` terminates every other thread
//! wherever it stands, THEN runs the FLS callbacks of the exiting thread, and
//! `thread_done_cb` is one of them. A rayon worker that was killed mid-way
//! through its own teardown still held `HEAPS_LOCK`; the exiting thread's
//! `heaps_unregister` spun on it forever, at 100% of a core, in a process that
//! `TerminateProcess` cannot end because it is already terminating.
//!
//! The race needs a thread to be killed inside a few-instruction critical
//! section, which is why it took hundreds of runs to see. This test removes the
//! race: a thread parks INSIDE `for_each_heap`'s callback, where `HEAPS_LOCK` is
//! held, and the test thread — which owns a heap, so its FLS callback is armed —
//! exits the process. Deterministic on Windows. On unix it passes trivially:
//! `exit` runs no thread-key destructors, so `thread_done` never runs there.
//!
//! ⚠ If this regresses, the child hangs and CANNOT be killed: `TerminateProcess`
//! refuses a terminating process. The test fails at the timeout and names the
//! pid; end the child by terminating its one remaining thread (`TerminateThread`).

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const MARKER: &str = "RUSTY_ALLOC_EXIT_HANG_CHILD";
const EXIT_CODE: i32 = 41;

fn child() -> ! {
    static HELD: AtomicBool = AtomicBool::new(false);
    // Arm this thread's FLS callback: its first allocation creates its heap
    // and stores it in the done slot.
    let p = rusty_alloc::alloc::malloc(64);
    assert!(!p.is_null(), "child: malloc failed");

    std::thread::spawn(|| {
        rusty_alloc::init::for_each_heap(&mut |_| {
            HELD.store(true, Ordering::Release);
            loop {
                std::thread::park();
            }
        });
    });
    while !HELD.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
    // HEAPS_LOCK is held by a thread `exit` is about to kill.
    std::process::exit(EXIT_CODE);
}

// Miri cannot spawn a child process.
#[cfg_attr(miri, ignore)]
#[test]
fn exit_with_a_lock_held_by_a_killed_thread_does_not_hang() {
    if std::env::var(MARKER).is_ok() {
        child();
    }

    let exe = std::env::current_exe().expect("current_exe");
    let mut c = Command::new(exe)
        .env(MARKER, "1")
        .args([
            "--exact",
            "exit_with_a_lock_held_by_a_killed_thread_does_not_hang",
        ])
        // Not inherited: a hung child would hold the harness's pipes open and
        // stall `cargo test` long after this test has failed.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child");

    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = c.try_wait().expect("wait child") {
            break s;
        }
        assert!(
            Instant::now() < deadline,
            "child {} did not exit in 30 s: it is spinning in thread teardown on a lock \
             held by a thread ExitProcess killed (it cannot be ended with TerminateProcess)",
            c.id()
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(EXIT_CODE), "child exited abnormally");
}
