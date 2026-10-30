//! Gates for the shared light heap (`--cfg ra_shared_light_heap`,
//! `docs/plans/shared-light-heap.md`).
//!
//! Every test runs its allocations on FRESH threads, because whether a thread
//! is light is per-thread state, and the tests are serialised because they
//! share the one process-wide shared heap. The test binary's own global
//! allocator is the system's, so only the explicit calls below touch ours.
#![cfg(ra_shared_light_heap)]

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use rusty_alloc::alloc::{collect, free, malloc, malloc_aligned, zalloc};
use rusty_alloc::shared_light::{
    SHARED_LIGHT_BYTES, SHARED_MAX_SIZE, SHARED_TID, test_hold_lock, test_release_lock, used_pages,
};
use rusty_alloc::types::{LARGE_OBJ_SIZE_MAX, MEDIUM_OBJ_SIZE_MAX};
use rusty_alloc::{init, segment};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// The owner id stamped on the segment holding `p`.
fn owner(p: *mut u8) -> usize {
    // SAFETY: `p` is a live block of ours, so its segment header is live.
    unsafe { (*segment::segment_of(p)).thread_id.load(Ordering::Acquire) }
}

/// Whether the calling thread is still light (has no heap of its own).
fn light() -> bool {
    init::heap_box_fast() == init::empty_heap_box_ptr()
}

fn on_fresh<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(f).join().expect("worker panicked")
}

/// After every live shared block is freed, a collect from a light thread must
/// bring the shared heap back to zero pages in use.
fn assert_shared_pages_return() {
    on_fresh(|| {
        collect(true);
        assert!(light(), "collect on a light thread created a heap");
    });
    assert_eq!(
        used_pages().unwrap_or(0),
        0,
        "shared pages still in use after every block was freed"
    );
}

#[test]
fn small_requests_of_a_light_thread_are_shared() {
    let _g = serial();
    on_fresh(|| {
        let mut taken = 0;
        let mut size = 1;
        while size <= SHARED_MAX_SIZE && taken + size <= SHARED_LIGHT_BYTES {
            let p = malloc(size);
            assert!(!p.is_null());
            assert_eq!(
                owner(p),
                SHARED_TID,
                "size {size} was not served by the shared heap"
            );
            assert!(light(), "size {size} gave the thread its own heap");
            // SAFETY: `p` is live and ours, `size` bytes.
            unsafe {
                p.write_bytes(0xA5, size);
                free(p);
            }
            taken += size;
            size = size * 2 + 1;
        }
        let z = zalloc(200);
        assert_eq!(owner(z), SHARED_TID);
        // SAFETY: 200 live bytes.
        let bytes = unsafe { core::slice::from_raw_parts(z, 200) };
        assert!(bytes.iter().all(|&b| b == 0));
        // SAFETY: live and ours.
        unsafe { free(z) };
    });
    assert_shared_pages_return();
}

#[test]
fn big_requests_never_land_in_the_shared_heap() {
    let _g = serial();
    let sizes = [
        SHARED_MAX_SIZE + 1,
        SHARED_MAX_SIZE * 2,
        MEDIUM_OBJ_SIZE_MAX,
        MEDIUM_OBJ_SIZE_MAX + 1,
        1 << 20,
        LARGE_OBJ_SIZE_MAX,
        LARGE_OBJ_SIZE_MAX + 1, // huge
    ];
    for size in sizes {
        on_fresh(move || {
            let p = malloc(size);
            assert!(!p.is_null());
            assert_eq!(
                owner(p),
                init::thread_id(),
                "size {size} landed in a shared segment"
            );
            assert!(!light(), "size {size} did not promote the thread");
            // Once promoted, small requests are the thread's own too.
            let s = malloc(64);
            assert_eq!(
                owner(s),
                init::thread_id(),
                "a promoted thread's small block is shared"
            );
            // SAFETY: both live and ours.
            unsafe {
                free(s);
                free(p);
            }
        });
    }
    // Every size around the cutoff, one fresh thread each.
    for size in SHARED_MAX_SIZE - 64..=SHARED_MAX_SIZE + 64 {
        on_fresh(move || {
            let p = malloc(size);
            let shared = owner(p) == SHARED_TID;
            assert_eq!(
                shared,
                size <= SHARED_MAX_SIZE,
                "size {size} routed wrongly"
            );
            // SAFETY: live and ours.
            unsafe { free(p) };
        });
    }
    assert_shared_pages_return();
}

#[test]
fn a_thread_is_promoted_once_it_has_taken_the_threshold() {
    let _g = serial();
    on_fresh(|| {
        let mut live = Vec::new();
        let mut taken = 0usize;
        while light() {
            let p = malloc(256);
            assert!(!p.is_null());
            live.push(p);
            taken += 256;
            assert!(
                taken <= SHARED_LIGHT_BYTES + 256,
                "still light after {taken} bytes"
            );
        }
        let last = *live.last().unwrap();
        assert_eq!(
            owner(last),
            init::thread_id(),
            "the promoting allocation is not the thread's own"
        );
        assert!(
            live[..live.len() - 1]
                .iter()
                .all(|&p| owner(p) == SHARED_TID)
        );
        for p in live {
            // SAFETY: each live and ours.
            unsafe { free(p) };
        }
    });
    assert_shared_pages_return();
}

#[test]
fn aligned_requests_promote() {
    let _g = serial();
    on_fresh(|| {
        let p = malloc_aligned(64, 4096);
        assert!(!p.is_null());
        assert_eq!(p.addr() % 4096, 0);
        assert_eq!(owner(p), init::thread_id());
        assert!(!light());
        // SAFETY: live and ours.
        unsafe { free(p) };
    });
}

/// rusty_WRT's daemon shape: 3 live blocks in each of 15 classes (those the
/// shared heap serves at this geometry), held by many light threads at once,
/// then freed by a DIFFERENT thread. The threads start one after another, as
/// daemon threads do: two allocating at the same moment would promote each
/// other (contention is a promotion signal), which is the next test.
#[test]
fn light_threads_share_pages_and_cross_thread_frees_return_them() {
    const ALL: [usize; 15] = [
        16, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048, 4096,
    ];
    let _g = serial();
    let sizes: Vec<usize> = ALL.into_iter().filter(|&s| s <= SHARED_MAX_SIZE).collect();
    let held: Vec<Vec<usize>> = (0..8)
        .map(|_| {
            let sizes = sizes.clone();
            std::thread::spawn(move || {
                let mut v = Vec::new();
                for &s in &sizes {
                    for _ in 0..3 {
                        let p = malloc(s);
                        assert_eq!(owner(p), SHARED_TID);
                        // SAFETY: s live bytes.
                        unsafe { p.write_bytes(s as u8, s) };
                        v.push(p.expose_provenance());
                    }
                }
                assert!(light());
                v
            })
            .join()
            .expect("worker")
        })
        .collect();
    // Eight threads' 45 blocks each, but the pages are shared: far fewer than
    // eight threads' worth of per-class pages.
    let pages = used_pages().expect("shared heap exists");
    assert!(
        pages <= sizes.len() * 2,
        "{pages} shared pages for 8 threads x {} classes",
        sizes.len()
    );
    on_fresh(move || {
        for (i, v) in held.into_iter().enumerate() {
            for (j, a) in v.into_iter().enumerate() {
                let p = core::ptr::with_exposed_provenance_mut::<u8>(a);
                let s = sizes[j / 3];
                // SAFETY: a live block of `s` bytes written above.
                let ok = unsafe { core::slice::from_raw_parts(p, s) }
                    .iter()
                    .all(|&b| b == s as u8);
                assert!(ok, "thread {i} block {j} corrupted");
                // SAFETY: live, freed once, from a thread that did not allocate it.
                unsafe { free(p) };
            }
        }
    });
    assert_shared_pages_return();
}

/// A light thread that finds the shared lock taken is allocating at the same
/// moment as another thread: it must take a heap of its own, not wait.
#[test]
fn contention_promotes_instead_of_waiting() {
    let _g = serial();
    test_hold_lock();
    let promoted = on_fresh(|| {
        let p = malloc(64);
        let r = (owner(p) == init::thread_id(), light());
        // SAFETY: live and ours.
        unsafe { free(p) };
        r
    });
    test_release_lock();
    assert_eq!(
        promoted,
        (true, false),
        "a contended light thread waited or stayed light"
    );
}

/// Many light threads allocating and freeing each other's blocks at once.
#[test]
fn stress_light_threads_freeing_each_others_blocks() {
    let _g = serial();
    let pool: std::sync::Arc<Mutex<Vec<(usize, usize)>>> = std::sync::Arc::default();
    let workers: Vec<_> = (0..16)
        .map(|t| {
            let pool = pool.clone();
            std::thread::spawn(move || {
                let mut taken = 0;
                for i in 0..400usize {
                    let s = 8 + (i * 37 + t * 11) % 300;
                    if taken + s > SHARED_LIGHT_BYTES {
                        break;
                    }
                    taken += s;
                    let p = malloc(s);
                    assert!(!p.is_null());
                    // SAFETY: s live bytes.
                    unsafe { p.write_bytes((s % 251) as u8, s) };
                    let other = {
                        let mut g = pool.lock().unwrap();
                        g.push((p.expose_provenance(), s));
                        let n = g.len();
                        if n > 32 {
                            Some(g.swap_remove(i % n))
                        } else {
                            None
                        }
                    };
                    if let Some((a, n)) = other {
                        let q = core::ptr::with_exposed_provenance_mut::<u8>(a);
                        // SAFETY: a live block of n bytes some worker wrote.
                        let ok = unsafe { core::slice::from_raw_parts(q, n) }
                            .iter()
                            .all(|&b| b == (n % 251) as u8);
                        assert!(ok, "corrupted block");
                        // SAFETY: live, freed once.
                        unsafe { free(q) };
                    }
                }
            })
        })
        .collect();
    for w in workers {
        w.join().expect("worker");
    }
    for (a, _) in pool.lock().unwrap().drain(..) {
        // SAFETY: each a live block, freed once.
        unsafe { free(core::ptr::with_exposed_provenance_mut::<u8>(a)) };
    }
    assert_shared_pages_return();
}

/// The 2.2.7 hang class, for the shared lock: a thread `ExitProcess` killed
/// while it held the lock, and the exiting thread allocating afterwards (an
/// FLS callback, as rusty_alloc's own teardown is). The allocation must give
/// up on the lock and promote, and the process must exit.
///
/// If this regresses the child cannot be killed, and it holds the inheritable
/// handles of whatever started the test (a CI runner's pipes), so `cargo
/// test` stalls after this test has failed. End the child's last thread with
/// `TerminateThread`.
#[cfg(all(windows, not(miri)))]
#[test]
fn exit_with_the_shared_lock_held_by_a_killed_thread_does_not_hang() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    const MARKER: &str = "RUSTY_ALLOC_SHARED_EXIT_CHILD";
    const CODE: i32 = 43;

    static HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    unsafe extern "system" fn on_exit(_: *const core::ffi::c_void) {
        let p = malloc(32);
        assert!(!p.is_null());
    }
    if std::env::var(MARKER).is_ok() {
        std::thread::spawn(|| {
            rusty_alloc::shared_light::test_hold_lock();
            HELD.store(true, Ordering::Release);
            loop {
                std::thread::park();
            }
        });
        while !HELD.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        std::thread::spawn(|| {
            use windows_sys::Win32::System::Threading::{FlsAlloc, FlsSetValue};
            // SAFETY: a callback of the FLS signature; the value is a non-null
            // marker so the callback fires for this thread at process exit.
            unsafe {
                let idx = FlsAlloc(Some(on_exit));
                assert_ne!(idx, u32::MAX);
                FlsSetValue(idx, core::ptr::without_provenance(1));
            }
            std::process::exit(CODE);
        });
        loop {
            std::thread::park();
        }
    }

    let _g = serial();
    let mut c = Command::new(std::env::current_exe().expect("current_exe"))
        .env(MARKER, "1")
        .args([
            "--exact",
            "exit_with_the_shared_lock_held_by_a_killed_thread_does_not_hang",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child");
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = c.try_wait().expect("wait") {
            break s;
        }
        assert!(
            Instant::now() < deadline,
            "child {} hung at exit on the shared lock (end it with TerminateThread)",
            c.id()
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(CODE));
}
