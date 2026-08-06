//! G3: loom model of the cross-thread-free / delayed-free / abandonment
//! protocol — written BEFORE the implementation (plan §8 M4) and kept as its
//! specification. Models the PROTOCOL (one page word + one heap list + the
//! abandon sequence), not the allocator.
//!
//! The page's `xthread` word packs a block-list head pointer with a 2-bit flag:
//! - `NORMAL`  (0): remote frees push onto the page's own list.
//! - `DELAYED` (1): page is invisible to the owner's scan (full queue / large
//!   span) — remote frees must instead nudge the OWNER's delayed list so the
//!   heartbeat sees them.
//! - `FREEING` (2): transient — a remote is dereferencing the heap pointer
//!   RIGHT NOW. Other delayed pushes spin; the abandoner MUST wait this out
//!   before tearing the heap down (this is the use-after-free the state
//!   exists to prevent).
//! - `NEVER`   (3): page abandoned, no owning heap — remote frees go on the
//!   page's own list; the adopter collects them.
//!
//! Run: RUSTFLAGS="--cfg loom" cargo test --test loom_xthread --release
//!
//! Blocks are modeled as small integers; "losing" one means the protocol lost
//! memory, pushing after heap death means use-after-free — both assert.

#![cfg(loom)]

use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use loom::thread;

const XMASK: usize = 3;
const NORMAL: usize = 0;
const DELAYED: usize = 1;
const FREEING: usize = 2;
const NEVER: usize = 3;

/// Blocks are ids shifted past the flag bits; 0 = empty list. A real block's
/// `next` is intrusive; the model keeps a tiny id→next table instead.
struct Model {
    /// Page word: (head_id << 8) | flag — id-encoding stands in for pointers.
    xthread: AtomicUsize,
    /// The owning heap's delayed list head (id-encoded, no flags).
    heap_delayed: AtomicUsize,
    /// Heap storage validity — false after the abandoner "frees" it.
    heap_alive: AtomicBool,
    /// next[] table standing in for intrusive block links (ids 1..=N).
    next: [AtomicUsize; 8],
}

impl Model {
    fn new(initial_flag: usize) -> Model {
        Model {
            xthread: AtomicUsize::new(initial_flag),
            heap_delayed: AtomicUsize::new(0),
            heap_alive: AtomicBool::new(true),
            next: Default::default(),
        }
    }

    /// The remote-free protocol (mirrors `page::remote_free`).
    fn remote_free(&self, id: usize) {
        loop {
            let x = self.xthread.load(Ordering::Acquire);
            match x & XMASK {
                DELAYED => {
                    // Claim the transient FREEING state.
                    if self
                        .xthread
                        .compare_exchange(
                            x,
                            (x & !XMASK) | FREEING,
                            Ordering::AcqRel,
                            Ordering::Relaxed,
                        )
                        .is_ok()
                    {
                        // THE invariant: while we hold FREEING the heap must
                        // be alive — the abandoner is required to spin us out.
                        assert!(
                            self.heap_alive.load(Ordering::Acquire),
                            "UAF: delayed push into a dead heap"
                        );
                        // Push onto the heap's delayed list.
                        loop {
                            let d = self.heap_delayed.load(Ordering::Acquire);
                            self.next[id].store(d, Ordering::Relaxed);
                            if self
                                .heap_delayed
                                .compare_exchange(d, id << 8, Ordering::AcqRel, Ordering::Relaxed)
                                .is_ok()
                            {
                                break;
                            }
                        }
                        // Restore DELAYED (list bits may have changed? no —
                        // only WE can push while FREEING; owner may collect
                        // though, so re-read and preserve the pointer bits).
                        loop {
                            let y = self.xthread.load(Ordering::Acquire);
                            if self
                                .xthread
                                .compare_exchange(
                                    y,
                                    (y & !XMASK) | DELAYED,
                                    Ordering::AcqRel,
                                    Ordering::Relaxed,
                                )
                                .is_ok()
                            {
                                break;
                            }
                        }
                        return;
                    }
                }
                FREEING => loom::thread::yield_now(),
                flag => {
                    // NORMAL or NEVER: push onto the page's own list.
                    self.next[id].store(x >> 8 << 8, Ordering::Relaxed); // keep id<<8 form
                    if self
                        .xthread
                        .compare_exchange(x, (id << 8) | flag, Ordering::AcqRel, Ordering::Relaxed)
                        .is_ok()
                    {
                        return;
                    }
                }
            }
        }
    }

    /// Owner collect: steal the page list, preserving the flag (mirrors
    /// `page_collect`). Returns the number of blocks taken.
    fn owner_collect(&self) -> usize {
        let mut taken = 0;
        loop {
            let x = self.xthread.load(Ordering::Acquire);
            if x >> 8 == 0 {
                return taken;
            }
            if self
                .xthread
                .compare_exchange(x, x & XMASK, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                // Walk the stolen chain.
                let mut id = x >> 8;
                while id != 0 {
                    taken += 1;
                    id = self.next[id].load(Ordering::Relaxed) >> 8;
                }
                return taken;
            }
        }
    }

    /// Owner drain of the heap's delayed list.
    fn owner_drain_delayed(&self) -> usize {
        let d = self.heap_delayed.swap(0, Ordering::AcqRel);
        let mut n = 0;
        let mut id = d >> 8;
        while id != 0 {
            n += 1;
            id = self.next[id].load(Ordering::Relaxed) >> 8;
        }
        n
    }

    /// Abandoner (mirrors `thread_done`): flag → NEVER (spinning out any
    /// FREEING remote), THEN drain delayed, THEN kill the heap.
    fn abandon(&self) -> usize {
        loop {
            let x = self.xthread.load(Ordering::Acquire);
            if x & XMASK == FREEING {
                loom::thread::yield_now();
                continue;
            }
            if self
                .xthread
                .compare_exchange(x, (x & !XMASK) | NEVER, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
        let drained = self.owner_drain_delayed();
        self.heap_alive.store(false, Ordering::Release);
        drained
    }
}

/// ONE remote races a delayed push against the abandoner — the minimal model
/// of the use-after-free the FREEING state exists to prevent. Small enough to
/// explore exhaustively (no preemption bound, no branch budget, no mutual
/// spins to trip loom's livelock heuristic). Multi-pusher list races are
/// covered by `normal_push_vs_collect` and `park_unpark_vs_remote`.
#[test]
fn delayed_push_vs_abandon() {
    loom::model(|| {
        let m = Arc::new(Model::new(DELAYED));
        let m1 = m.clone();
        let t1 = thread::spawn(move || m1.remote_free(1));
        let ma = m.clone();
        let ta = thread::spawn(move || ma.abandon());
        t1.join().unwrap();
        let drained_at_abandon = ta.join().unwrap();
        // The block ends in exactly one place: drained at abandon, on the
        // page list (pushed under NEVER), or a late delayed entry (pushed
        // while FREEING, before the abandoner's drain).
        let on_page = m.owner_collect();
        let late_delayed = m.owner_drain_delayed();
        assert_eq!(
            drained_at_abandon + on_page + late_delayed,
            1,
            "protocol lost the block"
        );
        assert_eq!(m.xthread.load(Ordering::Relaxed) & XMASK, NEVER);
    });
}

/// Extended soak (set LOOM_EXTENDED=1; CI nightly): two remotes vs the
/// abandoner under a preemption bound — the wide-space variant.
#[test]
fn delayed_push_vs_abandon_two_remotes_extended() {
    if std::env::var_os("LOOM_EXTENDED").is_none() {
        eprintln!("skipped (set LOOM_EXTENDED=1): 2-remote abandon soak");
        return;
    }
    let mut b = loom::model::Builder::new();
    b.preemption_bound = Some(2);
    b.max_branches = 100_000;
    b.check(|| {
        let m = Arc::new(Model::new(DELAYED));
        let m1 = m.clone();
        let m2 = m.clone();
        let t1 = thread::spawn(move || m1.remote_free(1));
        let t2 = thread::spawn(move || m2.remote_free(2));
        let ma = m.clone();
        let ta = thread::spawn(move || ma.abandon());
        t1.join().unwrap();
        t2.join().unwrap();
        let drained_at_abandon = ta.join().unwrap();
        let on_page = m.owner_collect();
        let late_delayed = m.owner_drain_delayed();
        assert_eq!(
            drained_at_abandon + on_page + late_delayed,
            2,
            "protocol lost a block"
        );
    });
}

/// Remote NORMAL pushes race the owner's collect; nothing may be lost.
#[test]
fn normal_push_vs_collect() {
    loom::model(|| {
        let m = Arc::new(Model::new(NORMAL));
        let m1 = m.clone();
        let m2 = m.clone();
        let t1 = thread::spawn(move || m1.remote_free(1));
        let t2 = thread::spawn(move || m2.remote_free(2));
        let mo = m.clone();
        let to = thread::spawn(move || mo.owner_collect());
        let c1 = to.join().unwrap();
        t1.join().unwrap();
        t2.join().unwrap();
        let c2 = m.owner_collect();
        assert_eq!(c1 + c2, 2, "collect lost a block");
    });
}

/// Owner park/unpark (NORMAL↔DELAYED) races a remote free; the block must end
/// up either on the page list or in the delayed list — never dropped.
#[test]
fn park_unpark_vs_remote() {
    loom::model(|| {
        let m = Arc::new(Model::new(NORMAL));
        let mr = m.clone();
        let tr = thread::spawn(move || mr.remote_free(1));
        // Owner parks the page as full (NORMAL → DELAYED), preserving bits.
        let mo = m.clone();
        let tp = thread::spawn(move || {
            loop {
                let x = mo.xthread.load(Ordering::Acquire);
                if x & XMASK == FREEING {
                    loom::thread::yield_now();
                    continue;
                }
                if mo
                    .xthread
                    .compare_exchange(
                        x,
                        (x & !XMASK) | DELAYED,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    break;
                }
            }
        });
        tr.join().unwrap();
        tp.join().unwrap();
        let total = m.owner_collect() + m.owner_drain_delayed();
        assert_eq!(total, 1, "block lost across park transition");
    });
}
