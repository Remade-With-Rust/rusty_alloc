//! Public allocation entry points (mirrors `alloc.c`/`free.c` routing).
//!
//! M4: NO LOCK. Each thread allocates from its own TLS heap; `free` routes by
//! ownership — the segment's `thread_id` decides between the owner's local
//! path and the loom-modeled remote protocol. `usable_size`/`realloc`/`expand`
//! work on any thread's blocks (they only read page metadata that is stable
//! while the block is live).

use core::ptr;
use core::sync::atomic::Ordering;

use crate::heap::Heap;
use crate::init;
use crate::page::{Block, Page, pflags, remote_free};
use crate::segment::{self, Segment, SegmentKind, page_of, segment_of};
use crate::types::{BIN_HUGE, SMALL_SIZE_MAX};

/// Recover the block start from a possibly-interior pointer (aligned-at
/// blocks). Identity unless the page carries `has_aligned`.
///
/// # Safety
/// `pg` must be the live page of `p`.
unsafe fn unalign(pg: *mut Page, p: *mut u8) -> *mut u8 {
    // SAFETY: page fields are stable while any of its blocks are live.
    unsafe {
        if (*pg).flags.load(Ordering::Relaxed) & (pflags::HAS_ALIGNED | pflags::SINGLE_BLOCK) == 0 {
            return p;
        }
        let seg = segment_of(p);
        let idx = segment::page_index(seg, pg);
        let area = segment::page_area(seg, idx);
        let off = p.addr() - area.addr();
        let bsize = (*pg).block_size;
        area.add((off / bsize) * bsize)
    }
}

/// The heap that OWNS `pg` — recovered from the page's `xheap` back-pointer
/// (container-of over the `HeapBox`'s offset-0 delayed list), so it is right
/// even when the thread holds several first-class heaps.
///
/// # Safety
/// `pg` must be a live page.
#[inline]
unsafe fn owner_heap(pg: *mut Page) -> *mut Heap {
    // SAFETY: page metadata is stable while any of its blocks are live.
    unsafe {
        let xh = (*pg).xheap.load(core::sync::atomic::Ordering::Acquire);
        if xh != 0 {
            (*init::box_of_xheap(xh)).heap.get()
        } else {
            // Pre-M6 pages / fallback. Reaching here on a thread whose heap
            // creation FAILED is impossible for a live page: the local free
            // path requires this thread to own the segment, which requires it
            // to have allocated, which requires a created heap.
            let h = my_heap();
            debug_assert!(!h.is_null(), "owner_heap fallback on a heapless thread");
            h
        }
    }
}

/// This thread's heap, creating it on first use — NULL when creation fails
/// (memory exhaustion). Every caller is a slow path and must treat null as
/// "the allocation fails": return null / skip the bookkeeping. The malloc
/// FAST path never sees this — the TLS slot holds the immortal sentinel, and
/// `malloc_slow` performs its own null check after `ensure_heap`.
#[inline]
fn my_heap() -> *mut Heap {
    let hb = init::heap_box();
    if hb.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: non-null ⇒ this thread's live box.
    unsafe { (*hb).heap.get() }
}

/// Catch a FOREIGN pointer handed to `free` — one this allocator never
/// returned — before it is used to derive metadata (hardening gate H-19,
/// residual risk R-001).
///
/// `free` masks the pointer to a 32 MiB segment base and reads `slice_offset`
/// out of it. For a pointer we own that is correct by construction; for a
/// pointer belonging to *another* allocator it reads whatever happens to sit
/// at that address and follows it. That is the mechanism behind the
/// mixed-allocator crashes this project has already documented (a
/// jemalloc-linked redis under `LD_PRELOAD`), and upstream mimalloc's
/// `mi_free` has the identical shape.
///
/// The global segment map already answers "is this address in a window we
/// own?" in one load and one bit test — it was built for
/// `mi_is_in_heap_region` and simply was not wired to the free path. It is
/// wired here **under `debug_assertions` and `debug_checks` only**, because:
///
///   * it is a DIAGNOSTIC, not a safety oracle — a racing segment release can
///     flip the answer, so a hard release-mode abort could fire on a
///     legitimate free; and
///   * on the release fast path it would cost a dependent load of a 1 MiB
///     sparse table per free, which is exactly the kind of cost this crate
///     measures before adopting.
///
/// What it buys: every test, every Miri run, every fuzz iteration and every
/// debug-built consumer now turns "mysterious corruption later" into "this
/// pointer was not ours, at this call". That is where the bug is findable.
#[inline(always)]
fn debug_foreign_pointer_guard(p: *mut u8) {
    #[cfg(any(debug_assertions, feature = "debug_checks"))]
    {
        debug_assert!(
            crate::segment_map::contains(p),
            "rusty_alloc: free() called on a pointer this allocator never returned \
             ({p:p} is not in any registered segment window). A foreign pointer here \
             reads metadata from memory we do not own — see UNSAFE.md and R-001."
        );
    }
    #[cfg(not(any(debug_assertions, feature = "debug_checks")))]
    {
        let _ = p;
    }
}

/// Bump a per-heap realloc counter, tolerating a heapless thread (a thread
/// whose heap creation failed under memory exhaustion has no counters).
///
/// DEBUG ONLY, and that is the point: like `Heap::stat_alloc`/`stat_free` — and
/// like upstream's `MI_STAT` — the realloc counters exist for diagnostics, not
/// for the shipped allocator. Keeping them live cost the RELEASE realloc path a
/// full `my_heap()` resolution (a TLS load, a null check, a `heap.get()`) on
/// every call, purely to increment a number nobody reads in production — the
/// realloc decision itself needs only `usable_size(p)`, never the owning heap.
/// In release this compiles to nothing and the resolution disappears.
#[inline]
fn stat_realloc(in_place: bool) {
    #[cfg(debug_assertions)]
    {
        let h = my_heap();
        if !h.is_null() {
            // SAFETY: own live heap; counters only.
            unsafe {
                if in_place {
                    (*h).stats.realloc_in_place += 1;
                } else {
                    (*h).stats.realloc_moved += 1;
                }
            }
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = in_place;
    }
}

/// Allocate `size` bytes (8-byte aligned; 16-multiple bins are 16-aligned).
/// `malloc(0)` returns a valid unique pointer.
///
/// The fast path here reads through RAW POINTERS only and takes ONE branch
/// beyond the size class test. The TLS slot is never null — an uninitialised
/// thread holds the shared empty-heap SENTINEL, whose direct table of empty
/// pages routes the first allocation to [`malloc_slow`] exactly as an
/// exhausted real page does — so the "is this thread initialised?" test is
/// gone from the hot path entirely, and no `&mut` is ever formed on a heap
/// that might be the shared sentinel. The miss is a tail call, so the fast
/// path also needs no callee-saved registers.
#[inline]
pub fn malloc(size: usize) -> *mut u8 {
    let hb = init::heap_box_fast();
    // SAFETY: raw reads only — `hb` may be the shared immortal sentinel (see
    // `heap_box_fast`), which must never see a `&mut` or a write. `direct`
    // entries always point at a live page of this thread's heap or at the
    // immortal empty-page sentinel, and `page_pop` on the sentinel returns
    // null before its first store.
    unsafe {
        if size <= SMALL_SIZE_MAX {
            let w = crate::types::wsize_from_size(size);
            let h = (*hb).heap.get();
            let p = (*h).direct[w];
            let b = crate::page::page_pop(p);
            if !b.is_null() {
                #[cfg(debug_assertions)]
                {
                    // Raw-pointer RMW: `b` is non-null, so `h` is this
                    // thread's REAL heap (the sentinel never yields a block).
                    (*h).stats.allocs += 1;
                }
                return b;
            }
        }
        malloc_slow(hb, size)
    }
}

/// [`malloc`] for C++ `operator new`: the same fast path, but the miss TAIL
/// CALLS a cold arm that also runs the caller's out-of-memory handler, so the
/// caller needs no null test of its own.
///
/// This exists because of what a null test costs the caller. `malloc`'s miss
/// is a tail call precisely so its fast path needs no callee-saved registers
/// (see its doc) — but a caller that inspects the RESULT and may then use
/// `size` again has to keep `size` alive across that call, and one live value
/// is enough to give the whole function a frame. On `larson-sized` that frame
/// cost the `operator new[]` export **3.00 Ir on every allocation**, 5.8% of
/// the allocator, for an OOM arm that never runs.
///
/// `on_oom` is a plain `fn` pointer rather than a generic so this stays a
/// single instantiation; it is passed straight through to the cold arm, so it
/// is never live across anything either, and LLVM folds it to a direct call
/// at each inlined call site.
#[inline]
pub fn malloc_or(size: usize, on_oom: fn(usize) -> *mut u8) -> *mut u8 {
    let hb = init::heap_box_fast();
    // SAFETY: as [`malloc`] — raw reads only, `hb` may be the immortal
    // sentinel, and `page_pop` on the sentinel returns null before any store.
    unsafe {
        if size <= SMALL_SIZE_MAX {
            let w = crate::types::wsize_from_size(size);
            let h = (*hb).heap.get();
            let p = (*h).direct[w];
            let b = crate::page::page_pop(p);
            if !b.is_null() {
                #[cfg(debug_assertions)]
                {
                    // Raw-pointer RMW: `b` is non-null, so `h` is this
                    // thread's REAL heap (the sentinel never yields a block).
                    (*h).stats.allocs += 1;
                }
                return b;
            }
        }
        malloc_or_slow(hb, size, on_oom)
    }
}

/// The cold arm of [`malloc_or`]: the ordinary slow path, then the caller's
/// OOM handler if even that could not serve.
///
/// # Safety
/// As [`malloc_slow`].
#[cold]
#[inline(never)]
unsafe fn malloc_or_slow(
    hb: *mut init::HeapBox,
    size: usize,
    on_oom: fn(usize) -> *mut u8,
) -> *mut u8 {
    // Duplicating `malloc_slow`'s body here to avoid nesting a second cold
    // frame measured EXACTLY FLAT — 4 instructions over 16,107 slow calls —
    // because the call it would remove is already a tail-call `jmp`. Reverted;
    // the plain call is the clearer code for the same instruction count.
    // SAFETY: forwarded contract.
    let p = unsafe { malloc_slow(hb, size) };
    if p.is_null() { on_oom(size) } else { p }
}

/// Everything `malloc`'s fast path could not serve: an uninitialised thread
/// (the sentinel box), a dry fast list, or a non-small size. Cold and
/// out-of-line so the fast path carries no register setup for it; `&mut Heap`
/// is formed only HERE, after the sentinel has been replaced by the thread's
/// real heap.
///
/// # Safety
/// `hb` must be the calling thread's TLS box: its real heap box, or the
/// shared sentinel.
#[cold]
#[inline(never)]
unsafe fn malloc_slow(hb: *mut init::HeapBox, size: usize) -> *mut u8 {
    if hb != init::empty_heap_box_ptr() {
        // SAFETY: a non-sentinel box is this thread's live, initialised box —
        // exclusive &mut scope on the calling thread's own heap. Straight to
        // the generic path: the fast list was dry (or the size non-small) a
        // moment ago on this same thread, so re-running the fast path here
        // could only repeat the miss — `mixed` measured +0.68 Ir/op for that
        // repeat before this went direct. This arm must stay a TAIL call: the
        // first OOM-handling shape kept the once-per-thread init in this
        // function, whose register needs cost the common slow path a
        // push/pop + call/ret pair (+2.4 Ir/op on `mixed`).
        //
        // REFUTED 2026-08-21 — peeking the MEDIUM bin's queue front here,
        // the way `Heap::malloc` now does, is a large regression: `big` and
        // `large` +25.00 Ir/op each, `med` +0.37, `realloc` +1.03, against
        // `mixed` −30.22. The reason is which list the block is on. A tight
        // alloc/free loop frees into `local_free`, so the queue front's `free`
        // list is ALWAYS dry when the next allocation arrives — the peek can
        // never hit, and every one of them pays the bin computation, the queue
        // read and a failed `page_pop` before falling through anyway. It pays
        // only where a workload leaves populated free lists behind
        // (`mixed`, and `rptest` through `Heap::malloc`). A fast path is worth
        // its cost only where the thing it looks for is actually there.
        return unsafe { (*(*hb).heap.get()).malloc_generic(size).0 };
    }
    malloc_first(size)
}

/// A fresh thread's very first allocation: create the heap, then allocate.
/// Split from [`malloc_slow`] so its register needs (keeping `size` alive
/// across `init_thread_heap`) are paid once per THREAD, not once per slow
/// call.
#[cold]
#[inline(never)]
fn malloc_first(size: usize) -> *mut u8 {
    let hb = init::ensure_heap(init::empty_heap_box_ptr());
    if hb.is_null() {
        // Heap creation failed (memory exhaustion): the malloc contract on
        // OOM is a null return, never an abort. The TLS slot still holds the
        // sentinel, so a later call retries creation.
        return ptr::null_mut();
    }
    // SAFETY: hb is this thread's live, initialised box.
    unsafe { (*(*hb).heap.get()).malloc_generic(size).0 }
}

/// Allocate `size` zeroed bytes.
pub fn zalloc(size: usize) -> *mut u8 {
    // Same TLS shape as `malloc` and `malloc_aligned_at`: `heap_box_fast` and
    // one compare against the shared sentinel, not `my_heap` — which goes
    // through `heap_box`, the variant that CREATES the heap when it finds the
    // sentinel, and so carries the once-per-thread initialisation and a null
    // check for its failure on a path taken once per ALLOCATION. `rptest`
    // calls calloc 43,449 times and paid it on every one.
    let hb = init::heap_box_fast();
    if hb == init::empty_heap_box_ptr() {
        return zalloc_first(size);
    }
    // `Heap::zalloc` zeroes with the popped page in hand, avoiding the
    // `usable_size` re-resolution the old `malloc` + `zero_block` pair paid on
    // every recycled block (opps.md #5).
    // SAFETY: a non-sentinel box is this thread's live, initialised box.
    unsafe { (*(*hb).heap.get()).zalloc(size) }
}

/// A fresh thread's first zeroing allocation: create the heap, then allocate.
/// Cold and out of line, as [`malloc_first`] is.
#[cold]
#[inline(never)]
fn zalloc_first(size: usize) -> *mut u8 {
    let h = my_heap();
    if h.is_null() {
        return ptr::null_mut(); // heap creation failed: OOM ⇒ null
    }
    // SAFETY: own live heap.
    unsafe { (*h).zalloc(size) }
}

/// Make a just-popped block fully zero. Even "fresh zero" blocks carry the
/// free-list link in their first word (upstream `_mi_page_malloc_zero` zeroes
/// exactly that word); recycled blocks need the full memset.
///
/// # Safety
/// `p` must be a live block just returned by this allocator.
unsafe fn zero_block(p: *mut u8, is_zero: bool) {
    if is_zero {
        // SAFETY: every block has ≥ 8 usable bytes (min bin is 8).
        unsafe { p.cast::<usize>().write(0) };
    } else {
        // SAFETY: p is live with usable_size(p) bytes.
        unsafe { core::ptr::write_bytes(p, 0, usable_size(p)) };
    }
}

/// `calloc`: overflow-checked `count * size`, zeroed.
pub fn calloc(count: usize, size: usize) -> *mut u8 {
    match count.checked_mul(size) {
        Some(total) => zalloc(total),
        None => ptr::null_mut(),
    }
}

/// `mi_mallocn`: overflow-checked `count * size`, NOT zeroed.
pub fn mallocn(count: usize, size: usize) -> *mut u8 {
    match count.checked_mul(size) {
        Some(total) => malloc(total),
        None => ptr::null_mut(),
    }
}

/// Small-size fast entry (`mi_malloc_small`): caller guarantees ≤ 1 KiB.
pub fn malloc_small(size: usize) -> *mut u8 {
    debug_assert!(size <= SMALL_SIZE_MAX);
    malloc(size)
}

/// Zeroed small-size fast entry (`mi_zalloc_small`).
pub fn zalloc_small(size: usize) -> *mut u8 {
    debug_assert!(size <= SMALL_SIZE_MAX);
    zalloc(size)
}

/// `mi_malloc_aligned(size, alignment)`.
pub fn malloc_aligned(size: usize, align: usize) -> *mut u8 {
    malloc_aligned_at(size, align, 0)
}

/// `mi_malloc_aligned_at(size, alignment, offset)`: returns `p` with
/// `(p + offset) % alignment == 0`.
// NOTE (2026-08-21): marking this and `malloc_aligned` `#[inline]` — so the
// facts the C shims establish about `align` would travel with the call and let
// LLVM drop the peek's re-derivation — measured **+4.56 Ir/op** and was
// reverted. The export gains a frame worth more than the check it removes,
// the same result the `realloc_inline` twin produced.
pub fn malloc_aligned_at(size: usize, align: usize, offset: usize) -> *mut u8 {
    let hb = init::heap_box_fast();
    // SAFETY: raw reads only, exactly as `malloc` does — `hb` may be the
    // shared immortal sentinel, which must never see a `&mut` or a write. Its
    // `direct` entries point at the immortal empty page, whose `free` is null,
    // so the peek below simply fails and routes to the cold path. That is what
    // lets the "is this thread initialised?" test leave the hot path
    // altogether, rather than being a compare every aligned allocation pays.
    unsafe {
        // `align - 1` is computed ONCE and used three times: as the bound
        // test, as the power-of-two test, and as the mask the block address is
        // checked against. Testing `mask < SEGMENT_SIZE / 2` rather than
        // `align <= SEGMENT_SIZE / 2` also disposes of `align == 0` for free —
        // it makes the mask `usize::MAX`, which fails the bound — so the
        // wrapping subtraction below never runs on a degenerate alignment.
        let mask = align.wrapping_sub(1);
        if offset == 0
            && size <= SMALL_SIZE_MAX
            && mask < crate::types::SEGMENT_SIZE / 2
            && align & mask == 0
        {
            let h = (*hb).heap.get();
            let w = crate::types::wsize_from_size(size);
            let p = (*h).direct[w];
            let b = (*p).free;
            if !b.is_null() && b.addr() & mask == 0 {
                let blk = crate::page::page_pop(p);
                if !blk.is_null() {
                    #[cfg(debug_assertions)]
                    {
                        // Raw-pointer RMW: `blk` is non-null, so `h` is this
                        // thread's REAL heap (the sentinel never yields).
                        (*h).stats.allocs += 1;
                    }
                    return blk;
                }
            }
        }
        malloc_aligned_slow(hb, size, align, offset)
    }
}

/// Everything the aligned peek could not serve. Cold and out of line so the
/// peek carries no register setup for it — the same shape as `malloc_slow`.
///
/// # Safety
/// `hb` must be the calling thread's TLS box: its real heap box, or the
/// shared sentinel.
#[cold]
#[inline(never)]
unsafe fn malloc_aligned_slow(
    hb: *mut init::HeapBox,
    size: usize,
    align: usize,
    offset: usize,
) -> *mut u8 {
    if hb != init::empty_heap_box_ptr() {
        // MEDIUM sizes get the peek here, in the COLD function, rather than
        // beside the small one in the hot entry.
        //
        // `direct[]` stops at SMALL_WSIZE_MAX, so a medium aligned request
        // reaches this point having never looked at a page — and then pays
        // `malloc_aligned_at_slow`'s validation and natural-fit search to
        // arrive at the bin's queue front anyway. `rptest` allocates
        // 8..4000 bytes aligned and took that road on 29% of its allocations.
        //
        // It belongs HERE and not in the hot entry: three shapes of that were
        // measured and every one taxed the small path the `aligned` scan
        // gates — a shared select +6.19 Ir/op, nested arms +2.50, a duplicated
        // `else if` +2.50. This placement leaves the hot entry byte-for-byte
        // unchanged and still catches the case one call earlier than before.
        // SAFETY: raw reads on a non-sentinel box, as the hot entry does.
        unsafe {
            let mask = align.wrapping_sub(1);
            if offset == 0
                && size > SMALL_SIZE_MAX
                && size <= crate::types::MEDIUM_OBJ_SIZE_MAX
                && mask < crate::types::SEGMENT_SIZE / 2
                && align & mask == 0
            {
                let h = (*hb).heap.get();
                let p = (*h).pages[crate::bins::bin(size)].first;
                if !p.is_null() {
                    let b = (*p).free;
                    if !b.is_null() && b.addr() & mask == 0 {
                        let blk = crate::page::page_pop(p);
                        if !blk.is_null() {
                            #[cfg(debug_assertions)]
                            {
                                (*h).stats.allocs += 1;
                            }
                            return blk;
                        }
                    }
                }
            }
        }
        // Straight to the SLOW half. `Heap::malloc_aligned_at` would open with
        // the very same peek this function was called because it failed —
        // same guard, same heap, same page, same free list, and nothing has
        // run in between — so re-running it can only fail again.
        // SAFETY: a non-sentinel box is this thread's live, initialised box.
        return unsafe {
            (*(*hb).heap.get())
                .malloc_aligned_at_slow(size, align, offset)
                .0
        };
    }
    malloc_aligned_first(size, align, offset)
}

/// A fresh thread's first ALIGNED allocation: create the heap, then allocate.
/// Cold and out of line for the reason [`malloc_first`] is — the register
/// needs of heap creation are paid once per thread, not once per call.
#[cold]
#[inline(never)]
fn malloc_aligned_first(size: usize, align: usize, offset: usize) -> *mut u8 {
    let h = my_heap();
    if h.is_null() {
        return ptr::null_mut(); // heap creation failed: OOM ⇒ null
    }
    // SAFETY: own heap.
    unsafe { (*h).malloc_aligned_at(size, align, offset).0 }
}

/// `mi_zalloc_aligned`.
pub fn zalloc_aligned(size: usize, align: usize) -> *mut u8 {
    zalloc_aligned_at(size, align, 0)
}

/// `mi_zalloc_aligned_at`.
pub fn zalloc_aligned_at(size: usize, align: usize, offset: usize) -> *mut u8 {
    // SIBLING CHECK: `zalloc` and `malloc_aligned_at` were both moved off
    // `my_heap` — the variant that CREATES the heap on finding the sentinel,
    // and so carries the once-per-thread initialisation and a null check on a
    // path taken once per allocation. This entry had the same shape and the
    // same defect. No benchmark in the corpus routes through it (`rptest`'s
    // calloc is unaligned), so it carries no measurement of its own; it is the
    // identical fix on identical code, worth −99,505 Ir on `rptest` where the
    // twin could be measured.
    let hb = init::heap_box_fast();
    if hb == init::empty_heap_box_ptr() {
        return zalloc_aligned_first(size, align, offset);
    }
    // SAFETY: a non-sentinel box is this thread's live, initialised box;
    // zero_block contract (zeroes [p, p+usable)).
    unsafe {
        let (p, is_zero) = (*(*hb).heap.get()).malloc_aligned_at(size, align, offset);
        if !p.is_null() {
            zero_block(p, is_zero);
        }
        p
    }
}

/// A fresh thread's first aligned zeroing allocation. Cold and out of line.
#[cold]
#[inline(never)]
fn zalloc_aligned_first(size: usize, align: usize, offset: usize) -> *mut u8 {
    let h = my_heap();
    if h.is_null() {
        return ptr::null_mut(); // heap creation failed: OOM ⇒ null
    }
    // SAFETY: own heap; zero_block contract.
    unsafe {
        let (p, is_zero) = (*h).malloc_aligned_at(size, align, offset);
        if !p.is_null() {
            zero_block(p, is_zero);
        }
        p
    }
}

/// `mi_calloc_aligned` (overflow-checked, zeroed).
pub fn calloc_aligned(count: usize, size: usize, align: usize) -> *mut u8 {
    calloc_aligned_at(count, size, align, 0)
}

/// `mi_calloc_aligned_at`.
pub fn calloc_aligned_at(count: usize, size: usize, align: usize, offset: usize) -> *mut u8 {
    match count.checked_mul(size) {
        Some(total) => zalloc_aligned_at(total, align, offset),
        None => ptr::null_mut(),
    }
}

/// `mi_realloc_aligned`: realloc that preserves the alignment constraint.
///
/// # Safety
/// As [`realloc`].
pub unsafe fn realloc_aligned(p: *mut u8, newsize: usize, align: usize) -> *mut u8 {
    // SAFETY: forwarded contract.
    unsafe { realloc_aligned_at(p, newsize, align, 0) }
}

/// `mi_realloc_aligned_at`.
///
/// # Safety
/// As [`realloc`].
pub unsafe fn realloc_aligned_at(
    p: *mut u8,
    newsize: usize,
    align: usize,
    offset: usize,
) -> *mut u8 {
    if p.is_null() {
        return malloc_aligned_at(newsize, align, offset);
    }
    // SAFETY: p live per contract.
    let usable = unsafe { usable_size(p) };
    if newsize <= usable
        && newsize >= usable / 2
        && (p.addr() + offset).is_multiple_of(align.max(1))
    {
        stat_realloc(true);
        return p;
    }
    let np = malloc_aligned_at(newsize, align, offset);
    if np.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: both live and disjoint; prefix preserved, then p consumed.
    unsafe {
        core::ptr::copy_nonoverlapping(p, np, usable.min(newsize));
        free(p);
        stat_realloc(false);
    }
    np
}

/// `mi_rezalloc`: zero-preserving realloc — valid only on blocks that were
/// zero-initialized (`zalloc`/`calloc`/`rezalloc` lineage); grown space reads
/// zero.
///
/// # Safety
/// As [`realloc`]; additionally `p`'s content contract must hold.
pub unsafe fn rezalloc(p: *mut u8, newsize: usize) -> *mut u8 {
    // SAFETY: forwarded contract.
    unsafe { rezalloc_aligned_at(p, newsize, 1, 0) }
}

/// `mi_recalloc` (count·size overflow-checked).
///
/// # Safety
/// As [`rezalloc`].
pub unsafe fn recalloc(p: *mut u8, newcount: usize, size: usize) -> *mut u8 {
    match newcount.checked_mul(size) {
        // SAFETY: forwarded contract.
        Some(total) => unsafe { rezalloc(p, total) },
        None => ptr::null_mut(),
    }
}

/// `mi_rezalloc_aligned`.
///
/// # Safety
/// As [`rezalloc`].
pub unsafe fn rezalloc_aligned(p: *mut u8, newsize: usize, align: usize) -> *mut u8 {
    // SAFETY: forwarded contract.
    unsafe { rezalloc_aligned_at(p, newsize, align, 0) }
}

/// `mi_rezalloc_aligned_at` — the general zero-preserving realloc. A zalloc'd
/// block is zero across its FULL usable extent (the zalloc invariant), so
/// in-place keeps need nothing and moves zero exactly `[old_usable, new_usable)`.
///
/// # Safety
/// As [`rezalloc`].
pub unsafe fn rezalloc_aligned_at(
    p: *mut u8,
    newsize: usize,
    align: usize,
    offset: usize,
) -> *mut u8 {
    if p.is_null() {
        return if align <= 1 {
            zalloc(newsize)
        } else {
            zalloc_aligned_at(newsize, align, offset)
        };
    }
    // SAFETY: p live per contract.
    let usable = unsafe { usable_size(p) };
    if newsize <= usable
        && newsize >= usable / 2
        && (p.addr() + offset).is_multiple_of(align.max(1))
    {
        stat_realloc(true);
        return p;
    }
    let np = if align <= 1 {
        malloc(newsize)
    } else {
        malloc_aligned_at(newsize, align, offset)
    };
    if np.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: both live and disjoint; copy the zero-invariant prefix, zero
    // the fresh tail up to the new usable extent, consume p.
    unsafe {
        let keep = usable.min(newsize);
        core::ptr::copy_nonoverlapping(p, np, keep);
        let new_usable = usable_size(np);
        core::ptr::write_bytes(np.add(keep), 0, new_usable - keep);
        free(p);
        stat_realloc(false);
    }
    np
}

/// `mi_recalloc_aligned`.
///
/// # Safety
/// As [`rezalloc`].
pub unsafe fn recalloc_aligned(p: *mut u8, newcount: usize, size: usize, align: usize) -> *mut u8 {
    // SAFETY: forwarded contract.
    unsafe { recalloc_aligned_at(p, newcount, size, align, 0) }
}

/// `mi_recalloc_aligned_at`.
///
/// # Safety
/// As [`rezalloc`].
pub unsafe fn recalloc_aligned_at(
    p: *mut u8,
    newcount: usize,
    size: usize,
    align: usize,
    offset: usize,
) -> *mut u8 {
    match newcount.checked_mul(size) {
        // SAFETY: forwarded contract.
        Some(total) => unsafe { rezalloc_aligned_at(p, total, align, offset) },
        None => ptr::null_mut(),
    }
}

/// Free a block from this allocator, from ANY thread. Null is a no-op.
///
/// # Safety
/// `p` must be null or a pointer obtained from this allocator, not freed
/// before, with no live references into the block.
///
/// NOT `#[inline]`, and that was MEASURED rather than assumed. A call-count
/// profile showed the cdylib's exported `free` making a real call in here per
/// free (100,082 per 100,000) where upstream's `free` is one flat symbol, and
/// `malloc` next door already carried the attribute — a tidy-looking asymmetry.
/// Adding `#[inline]` here plus splitting the cold path out made every
/// deterministic workload SLOWER: batch_lifo 73.98 -> 74.98 Ir/op, perl
/// 1.0101 -> 1.0105, sqlite 1.0056 -> 1.0060. Reverted. The extra frame is
/// apparently cheaper than the register pressure of inlining this much fast
/// path into every call site.
///
/// RETESTED after M13 moved the general path out of line, on the theory that a
/// much smaller body would inline cheaply. Still a loss: perl 1.0044 -> 1.0055,
/// sqlite 1.0029 -> 1.0033. Three attempts, three refutations — treat this as
/// settled and do not try it a fourth time without a NEW mechanism.
pub unsafe fn free(p: *mut u8) {
    // SAFETY: forwarded contract.
    unsafe { free_inline(p) }
}

/// The body of [`free`], for the ONE caller that should carry it inline: the
/// LD_PRELOAD override's exported `free`, which otherwise pays a GOT-indirect
/// `jmp` thunk on every call. This is NOT the thrice-refuted "#[inline] on
/// `free`" — that inlined the body into every internal caller (realloc and
/// friends) and lost to the register pressure it created there. Internal
/// callers keep calling the outlined [`free`] above; only an export that IS
/// `free` and does nothing else should use this.
///
/// # Safety
/// As [`free`].
#[inline(always)]
pub unsafe fn free_inline(p: *mut u8) {
    // REFUTED TWICE (2026-08-22): folding the null test into the segment mask, the
    // way mimalloc does (`lea -0x1(%rdi),%rsi; and mask,%rsi; jle` — a block is
    // never at offset 0, so masking `p - 1` picks the same segment, and NULL
    // becomes negative so the mask's own flags answer the null test). Written
    // in Rust as `(p.addr().wrapping_sub(1) & !(SEGMENT_SIZE-1)) as isize <= 0`
    // it measured +1.00 Ir/call, 24.000 -> 25.000. LLVM does not reuse the
    // `and`'s flags: it re-derives the whole thing as an unsigned range check
    // and then needs a `movabs` for the sign-cleared 64-bit mask, which no
    // longer fits a sign-extended immediate —
    //     lea -0x1(%rdi),%rsi ; cmp $0x2000000,%rsi ; jl ; movabs $0x7ff..,%rax ; and %rax,%rsi
    // five instructions where the plain form below is four. Counting
    // mimalloc's own `mov %rdi,%rdx` (it must preserve `p`), upstream pays
    // four here too — there was never a gap. Do not retry.
    // The asm form is blocked too, and by the LANGUAGE rather than by codegen:
    // fusing the two needs the masked value as an `out(reg)` operand AND a
    // `label` to jump to, and "using both label and output operands for inline
    // assembly" is unstable (rust#119364). A label-only block can express the
    // null test but not the fusion, which leaves it at the same two
    // instructions it already costs. Revisit if that feature stabilises.
    if p.is_null() {
        return;
    }
    debug_foreign_pointer_guard(p);
    let seg = segment_of(p);
    // SAFETY: p is ours per the contract → seg is a live segment header. The
    // OWNING heap is recovered from the page's xheap back-pointer (container-
    // of over the HeapBox's offset-0 delayed list) — correct even when the
    // thread has several first-class heaps. tid gates local vs remote.
    unsafe {
        let owner_tid = (*seg).thread_id.load(core::sync::atomic::Ordering::Acquire);
        // The thread id is read BEFORE the page resolution on purpose: read
        // after it, LLVM assigned the id to the register holding the just-
        // computed page pointer, and every later page access had to be
        // re-derived as `slot + (-slice_offset)` — a `neg` plus two-register
        // addressing on the whole fast path.
        // On x86-64 this comparison is deferred to the branch itself (see the
        // `cmp {tid}, fs:0` below): `thread_id()` IS the fs base, so reading it
        // into a register and then comparing is two instructions where the
        // compare can take `fs:0` as its memory operand and be one. Rust
        // cannot express that — an `asm!` read must produce a register — so
        // the FUSED form is written where the branch is.
        #[cfg(not(all(target_arch = "x86_64", target_os = "linux", not(miri))))]
        let local = owner_tid == init::thread_id();
        // ONE page resolution, then ONE flags byte answers every question the
        // free path used to ask with separate loads (M9 brick #3): huge-vs-
        // normal segment, single-block span, interior (aligned-at) pointer.
        // page_of works for both kinds: a huge segment's interior slices all
        // offset back to slot 1.
        let pg = page_of(seg, p);
        let flags = (*pg).flags.load(Ordering::Relaxed);
        // ONE test decides the whole shape of the free (upstream's
        // `page->flags.full_aligned == 0`). A clear byte means: binned page in
        // a Normal segment, queued, exact pointer — so the general path's
        // segment-kind match, bin compare, full-queue re-test and unalign are
        // all provably unnecessary and are skipped rather than re-derived.
        if flags & pflags::SLOW_FREE == 0 {
            // SAFETY: reads the thread pointer at `fs:0` and compares it with
            // the segment's owner id — the same test as `owner_tid ==
            // init::thread_id()`, with the load folded into the compare's
            // memory operand. Reads only; no stack.
            #[cfg(all(target_arch = "x86_64", target_os = "linux", not(miri)))]
            core::arch::asm!(
                "cmp {tid}, fs:0",
                "jne {remote}",
                tid = in(reg) owner_tid,
                remote = label {
                    // SAFETY: `pg` is the live page this free resolved; a
                    // non-owning thread hands the block to its owner.
                    unsafe { remote_free(pg, p.cast::<Block>()) };
                    return;
                },
                options(nostack, readonly),
            );
            // From here the free is LOCAL. On x86-64 that is decided by the
            // asm above, which jumps away on a mismatch; elsewhere by the
            // ordinary comparison below.
            #[cfg(not(all(target_arch = "x86_64", target_os = "linux", not(miri))))]
            if !local {
                remote_free(pg, p.cast::<Block>());
                return;
            }
            {
                // Routing the whole free on ONE byte is only safe while that
                // byte agrees with the fields it summarises. These two check
                // exactly that, against INDEPENDENT representations: the
                // segment's own kind tag, and the page's bin. A desync (a page
                // built without its flag, a flag cleared by a stray write)
                // would otherwise send a huge or unqueued span down the binned
                // path and corrupt silently — so assert it where the decision
                // is actually made, not in a helper that may go uncalled.
                debug_assert_eq!(
                    (*seg).kind,
                    SegmentKind::Normal,
                    "free fast path: HUGE_SEGMENT clear but segment is Huge"
                );
                debug_assert_ne!(
                    (*pg).bin as usize,
                    BIN_HUGE,
                    "free fast path: SINGLE_BLOCK clear but bin is BIN_HUGE"
                );
                // Upstream's `mi_free_block_local` touches NO heap: the PAGE
                // owns `local_free`. The only parts of a fast-path free that
                // need the owning heap are the stats counter — which, like
                // upstream's `MI_STAT`, exists only in debug builds — and
                // retiring a page that just emptied, which is rare. So the
                // shipped fast path is push + decrement + ONE branch: both
                // cold outcomes of the decrement — 0 (page emptied, retire)
                // and negative (double free, abort) — share a single `<= 0`
                // test on its flags, and the shared handler is reached by a
                // tail jump. With no `call` left in the function, the fast
                // path needs no stack adjustment at all (the M16 lesson,
                // applied to the alignment push this time).
                // THE DECREMENT, AND WHY IT IS ASM.
                //
                // `used -= 1` followed by a test of the result is five
                // instructions from safe Rust — load, dec, store, test, branch
                // — because LLVM will not emit a memory-destination RMW when
                // the decremented value is also needed to drive a branch. It
                // even re-tests a value `dec` has already set the flags for.
                // mimalloc emits `subw $1, used; je`: TWO. That gap is 3 Ir on
                // every local free, `docs/opps.md` #6 measured it on four
                // workloads (sh6bench +366M, sh8bench +852M, alloc-test +200M,
                // cfrac +183M), and it was refuted TWICE from safe Rust — the
                // second time by deleting the return value entirely, which
                // produced byte-identical code because the caller then had to
                // re-read the field.
                //
                // It is not reachable from safe Rust by any arrangement of
                // this code. So it is written directly, as the one instruction
                // pair it is. `used` is a plain owner-only `u32` — no atomic,
                // no other thread may touch it — so a non-atomic RMW is
                // exactly right, and the flags `sub` leaves are precisely the
                // `<= 0` the two cold outcomes share: 0 means the page just
                // emptied, negative means the count wrapped and this is a
                // DOUBLE FREE.
                crate::page::page_link_local(pg, p.cast::<Block>());
                #[cfg(debug_assertions)]
                {
                    (*owner_heap(pg)).stats.frees += 1;
                }
                #[cfg(all(target_arch = "x86_64", not(miri)))]
                {
                    // SAFETY: `pg` is a live page of this thread; `USED_OFFSET`
                    // is `offset_of!(Page, used)`, checked against the real
                    // layout by a unit test. The asm reads and writes only that
                    // one `u32` and uses no stack.
                    core::arch::asm!(
                        // The offset is a `const` operand, not an address
                        // computed into a register: passing `pg + USED_OFFSET`
                        // as `in(reg)` costs a `lea` that mimalloc does not
                        // pay, because x86 addressing carries the displacement
                        // for free.
                        "sub dword ptr [{pg} + {off}], 1",
                        "jle {cold}",
                        pg = in(reg) pg,
                        off = const crate::page::USED_OFFSET,
                        cold = label {
                            // Page emptied, or the count wrapped. The common
                            // outcome is "sole page in its queue, keep it warm",
                            // which needs only the page's own links.
                            //
                            // A `label` block does not inherit the enclosing
                            // `unsafe`; it is a separate item.
                            // SAFETY: `pg` is the live page this free resolved,
                            // owned by this thread.
                            unsafe {
                                if (*pg).used == 0
                                    && (*pg).next.is_null()
                                    && (*pg).prev.is_null()
                                {
                                    return;
                                }
                                return retire_or_abort(pg);
                            }
                        },
                        options(nostack),
                    );
                }
                #[cfg(not(all(target_arch = "x86_64", not(miri))))]
                {
                    let u = (*pg).used.wrapping_sub(1);
                    (*pg).used = u;
                    if (u as i32) <= 0 {
                        if u == 0 && (*pg).next.is_null() && (*pg).prev.is_null() {
                            return;
                        }
                        return retire_or_abort(pg);
                    }
                }
            }
            return;
        }
        free_general(p, seg, pg, owner_tid);
    }
}

/// Cold tail of a fast-path free whose decrement went non-positive: 0 means
/// the page just emptied (retire it), negative means the count wrapped — a
/// DOUBLE FREE — and the process aborts. Folding both into one function keeps
/// the hot path to a single rarely-taken branch AND removes the last `call`
/// from `free`'s body, which is what lets LLVM drop the alignment push from
/// the prologue (a `jmp` needs no aligned stack; a `call` does).
///
/// Takes ONLY `pg` (+ the already-computed count) and re-derives the segment,
/// for the same reason [`free_general`] takes only `p`. Passing `seg` in would
/// keep it LIVE across the whole fast path — it is otherwise dead the moment
/// `page_of` is done with it — just to serve the rare branch that actually
/// needs it. Re-deriving costs one mask here; keeping it alive costs a
/// register everywhere.
///
/// Re-reads `(*pg).used` here rather than taking it as an argument: passing
/// the value in would give it a use beyond the hot path's compare, which is
/// exactly what stops LLVM folding the decrement into a memory-destination
/// `dec` whose own flags feed the branch. The field still holds the
/// post-decrement value (owner-thread field, nothing ran in between), so the
/// cold reload observes the same number.
///
/// # Safety
/// `pg` must be a queued binned page owned by the calling thread on which
/// `page_push_local` just returned a non-positive count.
#[cold]
#[inline(never)]
unsafe fn retire_or_abort(pg: *mut Page) {
    // SAFETY: owner-thread page field, per the contract.
    if (unsafe { (*pg).used } as i32) < 0 {
        crate::page::double_free_abort();
    }
    // KEEP-ONE-WARM, decided from the PAGE rather than from the heap.
    //
    // `retire_emptied` declines to retire a page that is the sole member of
    // its queue, and it answered that with `q.first == pg && q.last == pg` —
    // which costs a segment mask, an ACQUIRE load of `xheap` plus the
    // container-of to reach the owning heap, a `bin` load and the queue's
    // address, all before the two compares that decide nothing needs doing.
    //
    // A QUEUED page is its queue's only element exactly when both of its own
    // links are null: `queue_push_front` writes `prev = null`, and `next` is
    // null only when it was pushed onto an empty queue. `retire_or_abort` is
    // reached only from the free fast path, whose `SLOW_FREE == 0` test has
    // already established the page is binned, in a Normal segment, and
    // QUEUED — so the equivalence holds here.
    //
    // Two loads and two tests, and the whole heap resolution is skipped on
    // what is the common outcome for any workload that cycles one live block
    // through a size class.
    // SAFETY: owner-thread page links, per the contract.
    if unsafe { (*pg).next.is_null() && (*pg).prev.is_null() } {
        return;
    }
    // SAFETY: forwarded contract; `pg` points into its own segment's header,
    // so masking it recovers that segment exactly as it would from a block.
    unsafe {
        let seg = segment_of(pg.cast());
        (*owner_heap(pg)).retire_emptied(seg, pg);
    }
}

/// Everything a free can be once the flags byte is NOT clear: an interior
/// (aligned-at) pointer, a huge segment, an unqueued single-block span, or a
/// page parked in the full queue. Counted at 1.57% of frees on `batch_lifo`.
///
/// Takes ONLY `p` and re-derives the segment, page and flags, even though
/// [`free`] already has all three. That is deliberate: `p` is already in the
/// argument register, so the cold call needs no setup at all, and the ~10
/// instructions of re-derivation are paid on 1.6% of frees. An earlier attempt
/// threaded all five values through instead and measured WORSE than not
/// splitting — the argument setup sat on the HOT path, which is exactly what
/// this signature avoids.
///
/// # Safety
/// As [`free`], and `(*page_of(segment_of(p), p)).flags & SLOW_FREE != 0`.
#[cold]
#[inline(never)]
unsafe fn free_general(p: *mut u8, seg: *mut Segment, pg: *mut Page, owner_tid: usize) {
    // SAFETY: forwarded contract from `free`.
    unsafe {
        // `seg` and `pg` come from the caller, which has just derived them.
        //
        // This function used to take ONLY `p` and re-derive everything, and
        // that was the right call when it was measured: at 1.6% of frees
        // (`batch_lifo`) the ~10 instructions of re-derivation were cheaper
        // than keeping values live across the fast path, and an attempt to
        // thread all five through measured worse.
        //
        // The frequency is what changed. On a CROSS-THREAD workload the flags
        // byte is rarely clear — a remote thread cannot un-park a page it does
        // not own, so every remote free to a full page lands here — and the
        // deterministic cross-thread proxy puts that at **74.6% of frees**,
        // not 1.6%. Only the two EXPENSIVE derivations are passed (a segment
        // mask and the `page_of` follow-back); the two atomic loads below are
        // one instruction each and stay, which is what keeps the call's
        // argument setup to two registers the caller already has live.
        // Acquire BEFORE reading page metadata, for the same reason `free`
        // does: it synchronizes with an abandoning thread's release, so the
        // flags we read cannot predate an adoption.
        debug_assert_eq!(seg, segment_of(p));
        debug_assert_eq!(pg, page_of(seg, p));
        // NOTE: passing the caller's `flags` byte in as a fourth argument
        // too, sparing the load below, measured FLAT — the argument costs what
        // the load saves. `seg` and `pg` are worth passing because they are
        // DERIVATIONS (a mask, a follow-back); a single atomic load is not.
        // `owner_tid` is HANDED IN: the fast path loaded it a moment ago and
        // still has it in a register, and the call that reaches here is cold,
        // so the argument costs nothing at the call site and saves an atomic
        // load on every general free. (Passing `flags` as well measured flat —
        // an atomic load is one instruction and so is the argument that
        // replaces it — and passing either into `free_local_at` is a large
        // regression: see the xmalloc-test campaign.)
        let flags = (*pg).flags.load(Ordering::Relaxed);
        // Interior-pointer recovery is needed only for aligned-at blocks in a
        // NORMAL segment; a huge segment's block is the exact pointer we
        // returned, and a plain binned page never adjusts.
        // REFUTED (2026-08-22): splitting the interior-pointer arm out as a
        // `#[cold]` tail, so the common path would reach its dispatch with
        // nothing live across a call. Byte-identical — `unalign` is not what
        // gives this function its frame. The disassembly says so directly: the
        // three `push`es survive the split because `remote_free` is INLINED
        // here, spin loop and all (there is a `pause` in the body), and its
        // CAS protocol is what holds the callee-saved registers. Outlining
        // THAT would put a call on the hot path of every cross-thread free,
        // which is the one workload where this function dominates. Left as it
        // was.
        let block = if flags & pflags::HAS_ALIGNED != 0 && flags & pflags::HUGE_SEGMENT == 0 {
            unalign(pg, p)
        } else {
            p
        };
        // On x86-64 the comparison IS the branch: `thread_id()` is the fs
        // base, so the compare can take `fs:0` as its memory operand instead
        // of loading it into a register first. Same fusion as the fast path's.
        #[cfg(all(target_arch = "x86_64", target_os = "linux", not(miri)))]
        {
            core::arch::asm!(
                "cmp {tid}, fs:0",
                "jne {remote}",
                tid = in(reg) owner_tid,
                remote = label {
                    // SAFETY: `pg` is the live page this free resolved.
                    unsafe { remote_free(pg, block.cast::<Block>()) };
                    return;
                },
                options(nostack, readonly),
            );
            // Fell through: this thread owns the page.
            (*owner_heap(pg)).free_local_at(seg, pg, block);
        }
        #[cfg(not(all(target_arch = "x86_64", target_os = "linux", not(miri))))]
        if owner_tid == init::thread_id() {
            // Hand the already-resolved segment through (M9 brick #2).
            (*owner_heap(pg)).free_local_at(seg, pg, block);
        } else {
            // Remote: the loom-modeled protocol (huge pages sit DELAYED, so
            // this lands on the owner's delayed list and the owner's
            // heartbeat performs the release + unlink).
            remote_free(pg, block.cast::<Block>());
        }
    }
}

/// Usable size of a live block (≥ requested); 0 for null. Any thread.
///
/// # Safety
/// `p` must be null or a live pointer from this allocator.
pub unsafe fn usable_size(p: *const u8) -> usize {
    if p.is_null() {
        return 0;
    }
    let seg = segment_of(p.cast_mut());
    // SAFETY: page metadata of a live block is stable while it lives; for
    // interior (aligned-at) pointers the usable extent runs from p to the
    // block's end.
    unsafe {
        let pg = page_of(seg, p.cast_mut());
        // ONE flags load answers the whole question, the same way the free
        // fast path routes on one byte. The previous shape asked three
        // separate questions: `(*seg).kind == Huge` (a load from the SEGMENT,
        // a second cache line), then `unalign`, which loads the page flags
        // ITSELF only to discover there is nothing to unalign, then a
        // subtraction of a difference that is zero whenever it took that route.
        //
        // `HUGE_SEGMENT` mirrors `(*seg).kind == Huge` — the free fast path
        // already routes on that equivalence and asserts it in debug builds.
        let flags = (*pg).flags.load(Ordering::Relaxed);
        if flags & (pflags::HAS_ALIGNED | pflags::SINGLE_BLOCK | pflags::HUGE_SEGMENT) == 0 {
            return (*pg).block_size;
        }
        debug_assert_eq!(
            flags & pflags::HUGE_SEGMENT != 0,
            (*seg).kind == SegmentKind::Huge,
            "usable_size: HUGE_SEGMENT flag disagrees with the segment kind"
        );
        usable_size_slow(pg, p, flags)
    }
}

/// The interior-pointer / single-block / huge tail of [`usable_size`], out of
/// line so the common answer is a load, a test and a load.
///
/// Splitting this out is not about the branch — it is about what the caller
/// has to SPEND to contain it. `usable_size` is inlined into `realloc`, so the
/// block arithmetic here landed in `realloc`'s body and its register needs
/// showed up in `realloc`'s prologue and epilogue, paid on every call
/// including the overwhelming majority that never reach this code.
///
/// # Safety
/// `pg` is `p`'s live page and `flags` is its flags byte, with at least one of
/// HAS_ALIGNED / SINGLE_BLOCK / HUGE_SEGMENT set.
#[cold]
#[inline(never)]
unsafe fn usable_size_slow(pg: *mut Page, p: *const u8, flags: u8) -> usize {
    // SAFETY: forwarded contract.
    unsafe {
        if flags & pflags::HUGE_SEGMENT != 0 {
            return (*pg).block_size;
        }
        let start = unalign(pg, p.cast_mut());
        (*pg).block_size - (p.addr() - start.addr())
    }
}

/// NOTE (2026-08-21): replacing this `copy_nonoverlapping` with a bounded
/// hand-rolled 32-byte-chunk copy — on the theory that the PLT call into
/// libc's `memcpy` dominates a 64- or 128-byte move — measured **+31.47 Ir/op**
/// on the `realloc` scan and was reverted. The call is not the cost: glibc's
/// AVX `memcpy` moves 64 bytes in a couple of wide load/store pairs, while the
/// chunk loop pays an increment, a compare and a branch per chunk plus an
/// overlapping tail. Do not retry without a new mechanism.
/// `realloc`: null → malloc; in-place when the block still fits and stays at
/// least half-used; else alloc-copy-free (works across threads — the free is
/// routed). Null on failure with the original untouched.
///
/// # Safety
/// `p` must be null or a live pointer from this allocator; invalidated on move.
pub unsafe fn realloc(p: *mut u8, newsize: usize) -> *mut u8 {
    if p.is_null() {
        return malloc(newsize);
    }
    // SAFETY: p live per contract.
    let usable = unsafe { usable_size(p) };
    // NOTE: rewriting this as the one-compare unsigned range check
    // `newsize.wrapping_sub(usable >> 1) <= usable - (usable >> 1)` measured
    // FLAT — LLVM already emits that shape from the readable form.
    if newsize <= usable && newsize >= usable / 2 {
        // Keep in place. NOTE: this counter costs a TLS heap lookup (a call
        // into ld.so's __tls_get_addr in a cdylib) on the most common realloc
        // outcome, so removing it was tried as a brick -- and measured FLAT on
        // the deterministic instrument (perl 1.0602 -> 1.0602, sqlite
        // 1.0305 -> 1.0305, unchanged to four digits). Reverted: an
        // unmeasurable gain does not justify losing a work-parity counter.
        stat_realloc(true);
        return p;
    }
    // NOTE (2026-08-21): moving everything below into an `#[inline(never)]`
    // `realloc_move(p, newsize, usable)` — so `realloc`'s prologue would not
    // carry the register needs of an inlined malloc, memcpy and free — cost
    // **+12.00 Ir/op** and was reverted. The argument setup plus the call and
    // return are paid on every MOVING realloc, and the scan is all moves; the
    // frame it saved was smaller than the call it added.
    let np = malloc(newsize);
    if np.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: both live and disjoint; prefix preserved then p consumed.
    unsafe {
        core::ptr::copy_nonoverlapping(p, np, usable.min(newsize));
        // `free_inline`, not the outlined `free`: this function has ALREADY
        // masked `p` to its segment for `usable_size`, and inlining the free
        // here lets LLVM common-subexpression that away instead of masking
        // back to the same header a second time. Splitting `free_inline` to
        // pass the segment explicitly was tried first and cost +1 Ir on EVERY
        // free (batch_lifo 60.00 -> 61.00); letting the inliner find it costs
        // other callers nothing because only this one opts in.
        free_inline(p);
        stat_realloc(false);
    }
    np
}

/// `mi_reallocn`: overflow-checked `count * size` realloc.
///
/// # Safety
/// As [`realloc`].
pub unsafe fn reallocn(p: *mut u8, count: usize, size: usize) -> *mut u8 {
    match count.checked_mul(size) {
        // SAFETY: forwarded contract.
        Some(total) => unsafe { realloc(p, total) },
        None => ptr::null_mut(),
    }
}

/// `mi_reallocf`: like realloc but frees `p` when reallocation fails.
///
/// # Safety
/// As [`realloc`]; `p` is always consumed.
pub unsafe fn reallocf(p: *mut u8, newsize: usize) -> *mut u8 {
    // SAFETY: forwarded contract.
    let np = unsafe { realloc(p, newsize) };
    if np.is_null() && !p.is_null() {
        // SAFETY: realloc failed → p still live and ours to free.
        unsafe { free(p) };
    }
    np
}

/// `mi_expand`: strictly in-place resize; null when impossible.
///
/// # Safety
/// `p` must be null or a live pointer from this allocator.
pub unsafe fn expand(p: *mut u8, newsize: usize) -> *mut u8 {
    if p.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: p live per contract.
    let usable = unsafe { usable_size(p) };
    if newsize <= usable {
        p
    } else {
        ptr::null_mut()
    }
}

/// `mi_is_in_heap_region`: best-effort diagnostic backed by the segment map.
pub fn is_in_heap_region(p: *const u8) -> bool {
    crate::segment_map::contains(p)
}

/// `mi_collect`: drain cross-thread frees and retire empty pages of the
/// calling thread's heap.
pub fn collect(force: bool) {
    let h = my_heap();
    if h.is_null() {
        return; // heapless thread: nothing to collect
    }
    // SAFETY: own heap.
    unsafe { (*h).collect(force) };
}

/// Snapshot of the CALLING thread's heap counters (per-heap by design —
/// work-parity gates run single-threaded where these are exact).
pub fn stats() -> crate::heap::Stats {
    let h = my_heap();
    if h.is_null() {
        return crate::heap::Stats::new(); // heapless thread: zero counters
    }
    // SAFETY: own heap.
    unsafe { (*h).stats }
}

// ---------------------------------------------------------------------------
// First-class heap operations (M6, plan §5.6): each takes an explicit
// HeapBox. Heaps allocate only on their owning thread (debug-asserted, the C
// contract); frees still route anywhere via the ownership machinery.
// ---------------------------------------------------------------------------

/// Resolve a HeapBox to its inner heap, asserting the owner-thread contract.
///
/// # Safety
/// `hb` must be a live heap owned by the calling thread.
#[inline]
unsafe fn heap_of(hb: *mut init::HeapBox) -> *mut Heap {
    // SAFETY: hb live per contract.
    unsafe {
        debug_assert_eq!(
            (*hb).owner_tid,
            init::thread_id(),
            "heap used off its owning thread"
        );
        (*hb).heap.get()
    }
}

/// `mi_heap_malloc`.
///
/// # Safety
/// `hb` live and owned by the calling thread.
pub unsafe fn heap_malloc(hb: *mut init::HeapBox, size: usize) -> *mut u8 {
    // SAFETY: forwarded contract.
    unsafe { (*heap_of(hb)).malloc(size).0 }
}

/// `mi_heap_zalloc`.
///
/// # Safety
/// As [`heap_malloc`].
pub unsafe fn heap_zalloc(hb: *mut init::HeapBox, size: usize) -> *mut u8 {
    // SAFETY: forwarded contract.
    unsafe {
        let (p, is_zero) = (*heap_of(hb)).malloc(size);
        if !p.is_null() {
            zero_block(p, is_zero);
        }
        p
    }
}

/// `mi_heap_malloc_aligned_at` (the general form; offset 0 = `_aligned`).
///
/// # Safety
/// As [`heap_malloc`].
pub unsafe fn heap_malloc_aligned_at(
    hb: *mut init::HeapBox,
    size: usize,
    align: usize,
    offset: usize,
) -> *mut u8 {
    // SAFETY: forwarded contract.
    unsafe { (*heap_of(hb)).malloc_aligned_at(size, align, offset).0 }
}

/// `mi_heap_zalloc_aligned_at`.
///
/// # Safety
/// As [`heap_malloc`].
pub unsafe fn heap_zalloc_aligned_at(
    hb: *mut init::HeapBox,
    size: usize,
    align: usize,
    offset: usize,
) -> *mut u8 {
    // SAFETY: forwarded contract.
    unsafe {
        let (p, is_zero) = (*heap_of(hb)).malloc_aligned_at(size, align, offset);
        if !p.is_null() {
            zero_block(p, is_zero);
        }
        p
    }
}

/// `mi_heap_realloc` (grown heap-relatively; the free of a moved block still
/// routes by ownership, so cross-heap pointers are handled).
///
/// # Safety
/// As [`heap_malloc`]; `p` as [`realloc`].
pub unsafe fn heap_realloc(hb: *mut init::HeapBox, p: *mut u8, newsize: usize) -> *mut u8 {
    if p.is_null() {
        // SAFETY: forwarded contract.
        return unsafe { heap_malloc(hb, newsize) };
    }
    // SAFETY: p live per contract.
    let usable = unsafe { usable_size(p) };
    if newsize <= usable && newsize >= usable / 2 {
        // Debug-only counter (see `stat_realloc`): the release path returns `p`
        // with no heap touch at all.
        #[cfg(debug_assertions)]
        // SAFETY: hb live.
        unsafe {
            (*heap_of(hb)).stats.realloc_in_place += 1;
        }
        return p;
    }
    // SAFETY: forwarded contracts.
    unsafe {
        let np = heap_malloc(hb, newsize);
        if np.is_null() {
            return ptr::null_mut();
        }
        core::ptr::copy_nonoverlapping(p, np, usable.min(newsize));
        free(p);
        #[cfg(debug_assertions)]
        {
            (*heap_of(hb)).stats.realloc_moved += 1;
        }
        np
    }
}

/// `mi_heap_collect`.
///
/// # Safety
/// As [`heap_malloc`].
pub unsafe fn heap_collect(hb: *mut init::HeapBox, force: bool) {
    // SAFETY: forwarded contract.
    unsafe { (*heap_of(hb)).collect(force) };
}

/// `mi_heap_contains_block`: whether `p` lies inside memory owned by `hb`
/// (segment walk; diagnostic).
///
/// # Safety
/// `hb` live and owned by the calling thread; `p` any pointer.
pub unsafe fn heap_contains_block(hb: *mut init::HeapBox, p: *const u8) -> bool {
    if p.is_null() {
        return false;
    }
    let target = segment_of(p.cast_mut());
    // SAFETY: walking our own segment lists under the owner thread.
    unsafe {
        let h = heap_of(hb);
        let mut seg = (*h).segments;
        while !seg.is_null() {
            if seg == target {
                return true;
            }
            seg = (*seg).next;
        }
        let mut seg = (*h).huge_segments;
        while !seg.is_null() {
            if seg == target {
                return true;
            }
            seg = (*seg).next;
        }
    }
    false
}

/// `mi_heap_check_owned`: contains + block-grid validity (diagnostic).
///
/// # Safety
/// As [`heap_contains_block`].
pub unsafe fn heap_check_owned(hb: *mut init::HeapBox, p: *const u8) -> bool {
    // SAFETY: forwarded contract.
    if !unsafe { heap_contains_block(hb, p) } {
        return false;
    }
    let seg = segment_of(p.cast_mut());
    // SAFETY: contains ⇒ live segment of ours.
    unsafe {
        if (*seg).kind == SegmentKind::Huge {
            return true;
        }
        let pg = page_of(seg, p.cast_mut());
        (*pg).block_size > 0
    }
}

/// `mi_check_owned`: any-heap ownership (segment-map + registry walk-free
/// approximation: the map answers region membership).
pub fn check_owned(p: *const u8) -> bool {
    is_in_heap_region(p)
}
