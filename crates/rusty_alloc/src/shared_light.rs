//! The shared light heap (`--cfg ra_shared_light_heap`;
//! `docs/plans/shared-light-heap.md`).
//!
//! A thread starts LIGHT: its TLS heap stays the empty sentinel, so every
//! allocation it makes reaches the first-allocation slow path, which serves
//! small requests from this one process-wide heap under a lock. Once the
//! thread has taken [`SHARED_LIGHT_BYTES`] from it, finds the lock contended,
//! or asks for anything this heap does not serve, the slow path creates the
//! thread's own heap as it always did, and the thread never comes back.
//!
//! The shared heap's segments carry [`SHARED_TID`], an owner id no thread
//! has, so every free of a shared block takes the existing cross-thread route
//! and the shared heap drains it under the lock. Nothing on the malloc or free
//! fast path changes.

use core::cell::Cell;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use crate::init::{self, HeapBox};
use crate::types::SMALL_OBJ_SIZE_MAX;

/// Segment owner id of the shared heap. Thread ids are thread-pointer or OS
/// thread values, none of which is all-ones.
pub const SHARED_TID: usize = usize::MAX;

/// Bytes a thread may take from the shared heap over its life before it is
/// promoted to a heap of its own.
///
/// rusty_WRT's idle daemon threads hold about 33 KiB (3 blocks in each of 15
/// classes up to 4 KiB); a busy thread passes this within its first few
/// hundred allocations, so the shared phase is bounded start-up cost.
pub const SHARED_LIGHT_BYTES: usize = 64 * 1024;

/// The largest request the shared heap serves: the small-page classes.
/// Medium, large and huge requests always promote, so a big block can never
/// sit in a shared page.
pub const SHARED_MAX_SIZE: usize = SMALL_OBJ_SIZE_MAX;

static SHARED_BOX: AtomicPtr<HeapBox> = AtomicPtr::new(ptr::null_mut());
static SHARED_LOCK: AtomicBool = AtomicBool::new(false);

/// Set by the first thread that ever allocates, which takes a heap of its own
/// at once instead of starting light.
///
/// That thread is the process's main thread in practice, and the one most
/// likely to be busy. Started light, a busy main thread promotes after 64 KiB
/// and then pays for pages in BOTH heaps, because its early blocks stay live
/// in shared pages: rusty_WRT's boot census measured single-threaded daemons
/// +72 KiB (odhcpd) and +28 KiB (logd) that way, which cancelled the
/// multi-threaded win (procd −96 KiB) and left the system neutral. Exempting
/// it also means a single-threaded process never touches the shared heap.
static FIRST_CLAIMED: AtomicBool = AtomicBool::new(false);

ra_thread_local! {
    /// Bytes this thread has taken from the shared heap.
    static LIGHT_BYTES: Cell<usize> = const { Cell::new(0) };
    /// Set while this thread holds the shared lock and runs the shared heap,
    /// so a re-entrant allocation (a hook) is promoted instead of waiting on
    /// a lock its own thread holds.
    static IN_SHARED: Cell<bool> = const { Cell::new(false) };
}

#[inline]
fn in_shared() -> bool {
    IN_SHARED.with(Cell::get)
}

/// Mark `seg` as the shared heap's if `heap` is the shared heap. Called by
/// the heap on every segment it creates or adopts, before any block of it is
/// handed out or re-homed.
///
/// Keyed on the HEAP, not on a thread flag, so that a heap created by a
/// re-entrant call while this thread runs the shared heap is stamped as its
/// own. Until this store the segment carries the creating thread's id; a
/// fresh segment has no blocks anyone else holds, and frees from other
/// threads into an adopted one take the remote route under either id.
///
/// # Safety
/// `seg` is a live segment `heap` has just created or is adopting.
#[inline]
pub(crate) unsafe fn stamp_if_shared(heap: &crate::heap::Heap, seg: *mut crate::segment::Segment) {
    let hb = SHARED_BOX.load(Ordering::Relaxed);
    // SAFETY: a non-null shared box is live for the life of the process; the
    // comparison reads no field of `heap`.
    if !hb.is_null() && ptr::eq(unsafe { (*hb).heap.get() }.cast_const(), heap) {
        // SAFETY: per contract.
        unsafe { (*seg).thread_id.store(SHARED_TID, Ordering::Release) };
    }
}

/// How long an allocating light thread waits for the shared lock before it
/// gives up and takes a heap of its own: about one critical section.
///
/// A thread that finds the lock taken is, by that fact, allocating at the same
/// moment as another thread, which is what a busy thread does and an idle one
/// does not. So contention is a promotion signal, not something to wait out.
/// Waiting was measured: 32 threads storming the shared phase together ran
/// 3.2x slower than with per-thread heaps. It also means no allocation ever
/// waits unboundedly on this lock, including on one whose holder
/// `ExitProcess` has killed (the 2.2.7 hang class).
const SPIN_LIMIT: u32 = 128;

/// Take the shared lock within [`SPIN_LIMIT`] spins, or report false.
fn lock_briefly() -> bool {
    for _ in 0..SPIN_LIMIT {
        if !SHARED_LOCK.load(Ordering::Relaxed)
            && SHARED_LOCK
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Take the shared lock, however long it takes. Diagnostics only: nothing on
/// an allocation path waits here.
fn lock() {
    while !lock_briefly() {
        #[cfg(feature = "std")]
        std::thread::yield_now();
    }
}

/// Take the shared lock only if it is free right now.
fn try_lock() -> bool {
    SHARED_LOCK
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
}

fn unlock() {
    SHARED_LOCK.store(false, Ordering::Release);
}

/// Run `f` on the shared heap with the lock held and [`in_shared`] set,
/// creating the heap on first use. Requires the lock to be held by the
/// caller. None when the heap cannot be created.
///
/// # Safety
/// The caller holds `SHARED_LOCK`.
unsafe fn with_heap_locked<R>(f: impl FnOnce(&mut crate::heap::Heap) -> R) -> Option<R> {
    let mut hb = SHARED_BOX.load(Ordering::Relaxed);
    if hb.is_null() {
        // Created under the lock, so two light threads cannot both make one.
        hb = init::create_shared_heap_box();
        if hb.is_null() {
            return None;
        }
        SHARED_BOX.store(hb, Ordering::Relaxed);
    }
    IN_SHARED.with(|c| c.set(true));
    // SAFETY: the lock gives this thread exclusive use of the shared heap,
    // and the box is live for the life of the process.
    let r = f(unsafe { &mut *(*hb).heap.get() });
    IN_SHARED.with(|c| c.set(false));
    Some(r)
}

/// Serve `size` from the shared heap if this thread is still light and the
/// request is one the shared heap takes. None means "promote": the caller
/// creates the thread's own heap and allocates there, exactly as before.
pub(crate) fn try_alloc(size: usize, zero: bool) -> Option<*mut u8> {
    if !crate::SHARED_LIGHT || size > SHARED_MAX_SIZE {
        return None;
    }
    // Re-entry (a hook that allocates while this thread runs the shared
    // heap) must not wait on a lock it holds; it gets its own heap, whose
    // segments `stamp_if_shared` leaves alone because it is not this heap.
    if in_shared() || !init::in_default_subproc() {
        return None;
    }
    // The first allocating thread is never light (see `FIRST_CLAIMED`). The
    // load keeps the common case to a read; the swap decides the race.
    if !FIRST_CLAIMED.load(Ordering::Relaxed) && !FIRST_CLAIMED.swap(true, Ordering::Relaxed) {
        return None;
    }
    let used = LIGHT_BYTES.with(Cell::get);
    if used.saturating_add(size) > SHARED_LIGHT_BYTES {
        return None;
    }
    // Contended: this thread is busy, not light (see `SPIN_LIMIT`).
    if !lock_briefly() {
        return None;
    }
    // SAFETY: the lock is held.
    let served = unsafe {
        with_heap_locked(|h| {
            let p = if zero {
                h.zalloc(size)
            } else {
                h.malloc(size).0
            };
            // The deferred-free hook must not run under the lock: it may
            // allocate. Take the flag and fire it after unlocking.
            let due = core::mem::take(&mut h.deferred_due);
            (p, due)
        })
    };
    unlock();
    let (p, due) = served?;
    if due {
        crate::options::deferred_free(false);
    }
    if p.is_null() {
        // The shared heap is out of memory; let the thread's own heap try.
        return None;
    }
    LIGHT_BYTES.with(|c| c.set(used + size));
    Some(p)
}

/// `collect` from a thread that has no heap of its own: collect the shared
/// heap if nobody is using it, instead of creating a heap only to collect
/// it. True when handled (including "busy, skipped").
pub(crate) fn collect(force: bool) -> bool {
    if !crate::SHARED_LIGHT || in_shared() {
        return false;
    }
    if SHARED_BOX.load(Ordering::Relaxed).is_null() {
        return true; // nothing shared yet: nothing to collect
    }
    if !try_lock() {
        return true; // another thread is in it, and will collect as it goes
    }
    // SAFETY: the lock is held; the shared heap is a live, REMAINING heap, so
    // a forced collect may adopt orphans into it (`adopt_segment` stamps them
    // SHARED_TID).
    let _ = unsafe { with_heap_locked(|h| h.collect(force)) };
    unlock();
    true
}

/// The shared heap's box, or null before the first light allocation. For
/// tests and diagnostics.
pub fn shared_box() -> *mut HeapBox {
    SHARED_BOX.load(Ordering::Acquire)
}

/// Pages in use across the shared heap's segments, read under the lock;
/// None before the shared heap exists. For tests and diagnostics.
pub fn used_pages() -> Option<usize> {
    if shared_box().is_null() {
        return None;
    }
    lock();
    // SAFETY: the lock is held; the segment list is the heap's own.
    let n = unsafe {
        with_heap_locked(|h| {
            let mut n = 0usize;
            let mut s = h.segments;
            while !s.is_null() {
                n += (*s).used_pages as usize;
                s = (*s).next;
            }
            n
        })
    };
    unlock();
    n
}

/// Take the shared lock (the "thread killed while holding it" half of the
/// exit test, and the contention test). Test-only.
#[doc(hidden)]
pub fn test_hold_lock() {
    lock();
}

/// Release a lock taken by [`test_hold_lock`]. Test-only.
#[doc(hidden)]
pub fn test_release_lock() {
    unlock();
}
