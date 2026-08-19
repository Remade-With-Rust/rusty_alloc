//! Public allocation entry points (mirrors `alloc.c`/`free.c` routing).
//!
//! M4: NO LOCK. Each thread allocates from its own TLS heap; `free` routes by
//! ownership — the segment's `thread_id` decides between the owner's local
//! path and the loom-modeled remote protocol. `usable_size`/`realloc`/`expand`
//! work on any thread's blocks (they only read page metadata that is stable
//! while the block is live).

use core::ptr;

use crate::heap::Heap;
use crate::init;
use crate::page::{Block, Page, pflags, remote_free};
use crate::segment::{self, SegmentKind, page_of, segment_of};
use crate::types::{BIN_HUGE, SMALL_SIZE_MAX};

/// Recover the block start from a possibly-interior pointer (aligned-at
/// blocks). Identity unless the page carries `has_aligned`.
///
/// # Safety
/// `pg` must be the live page of `p`.
unsafe fn unalign(pg: *mut Page, p: *mut u8) -> *mut u8 {
    // SAFETY: page fields are stable while any of its blocks are live.
    unsafe {
        if (*pg).flags & (pflags::HAS_ALIGNED | pflags::SINGLE_BLOCK) == 0 {
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
            my_heap() // pre-M6 pages / fallback
        }
    }
}

#[inline]
fn my_heap() -> *mut Heap {
    // SAFETY: heap_box returns this thread's live box.
    unsafe { (*init::heap_box()).heap.get() }
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
    let hb = init::ensure_heap(hb);
    // SAFETY: `hb` is now this thread's live, initialised box — exclusive
    // &mut scope on the calling thread's own heap. Straight to the generic
    // path: the fast list was dry (or the size non-small) a moment ago on
    // this same thread, so re-running the fast path here could only repeat
    // the miss — `mixed` measured +0.68 Ir/op for that repeat before this
    // went direct.
    unsafe { (*(*hb).heap.get()).malloc_generic(size).0 }
}

/// Allocate `size` zeroed bytes.
pub fn zalloc(size: usize) -> *mut u8 {
    // SAFETY: own heap; zero_block contract below.
    unsafe {
        let (p, is_zero) = (*my_heap()).malloc(size);
        if !p.is_null() {
            zero_block(p, is_zero);
        }
        p
    }
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
pub fn malloc_aligned_at(size: usize, align: usize, offset: usize) -> *mut u8 {
    // SAFETY: own heap.
    unsafe { (*my_heap()).malloc_aligned_at(size, align, offset).0 }
}

/// `mi_zalloc_aligned`.
pub fn zalloc_aligned(size: usize, align: usize) -> *mut u8 {
    zalloc_aligned_at(size, align, 0)
}

/// `mi_zalloc_aligned_at`.
pub fn zalloc_aligned_at(size: usize, align: usize, offset: usize) -> *mut u8 {
    // SAFETY: own heap; zero_block contract (zeroes [p, p+usable)).
    unsafe {
        let (p, is_zero) = (*my_heap()).malloc_aligned_at(size, align, offset);
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
        // SAFETY: own heap, stats only.
        unsafe { (*my_heap()).stats.realloc_in_place += 1 };
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
        (*my_heap()).stats.realloc_moved += 1;
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
        // SAFETY: own heap, stats only.
        unsafe { (*my_heap()).stats.realloc_in_place += 1 };
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
        (*my_heap()).stats.realloc_moved += 1;
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
    if p.is_null() {
        return;
    }
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
        let local = owner_tid == init::thread_id();
        // ONE page resolution, then ONE flags byte answers every question the
        // free path used to ask with separate loads (M9 brick #3): huge-vs-
        // normal segment, single-block span, interior (aligned-at) pointer.
        // page_of works for both kinds: a huge segment's interior slices all
        // offset back to slot 1.
        let pg = page_of(seg, p);
        let flags = (*pg).flags;
        // ONE test decides the whole shape of the free (upstream's
        // `page->flags.full_aligned == 0`). A clear byte means: binned page in
        // a Normal segment, queued, exact pointer — so the general path's
        // segment-kind match, bin compare, full-queue re-test and unalign are
        // all provably unnecessary and are skipped rather than re-derived.
        if flags & pflags::SLOW_FREE == 0 {
            if local {
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
                let used_now = crate::page::page_push_local(pg, p.cast::<Block>());
                #[cfg(debug_assertions)]
                {
                    (*owner_heap(pg)).stats.frees += 1;
                }
                if (used_now as i32) <= 0 {
                    return retire_or_abort(pg);
                }
            } else {
                remote_free(pg, p.cast::<Block>());
            }
            return;
        }
        free_general(p);
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
unsafe fn free_general(p: *mut u8) {
    // SAFETY: forwarded contract from `free`.
    unsafe {
        let seg = segment_of(p);
        // Acquire BEFORE reading page metadata, for the same reason `free`
        // does: it synchronizes with an abandoning thread's release, so the
        // flags we read cannot predate an adoption.
        let owner_tid = (*seg).thread_id.load(core::sync::atomic::Ordering::Acquire);
        let pg = page_of(seg, p);
        let flags = (*pg).flags;
        // Interior-pointer recovery is needed only for aligned-at blocks in a
        // NORMAL segment; a huge segment's block is the exact pointer we
        // returned, and a plain binned page never adjusts.
        let block = if flags & pflags::HAS_ALIGNED != 0 && flags & pflags::HUGE_SEGMENT == 0 {
            unalign(pg, p)
        } else {
            p
        };
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
        if (*seg).kind == SegmentKind::Huge {
            return (*pg).block_size;
        }
        let start = unalign(pg, p.cast_mut());
        (*pg).block_size - (p.addr() - start.addr())
    }
}

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
    if newsize <= usable && newsize >= usable / 2 {
        // Keep in place. NOTE: this counter costs a TLS heap lookup (a call
        // into ld.so's __tls_get_addr in a cdylib) on the most common realloc
        // outcome, so removing it was tried as a brick -- and measured FLAT on
        // the deterministic instrument (perl 1.0602 -> 1.0602, sqlite
        // 1.0305 -> 1.0305, unchanged to four digits). Reverted: an
        // unmeasurable gain does not justify losing a work-parity counter.
        // SAFETY: own heap for stats only.
        unsafe { (*my_heap()).stats.realloc_in_place += 1 };
        return p;
    }
    let np = malloc(newsize);
    if np.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: both live and disjoint; prefix preserved then p consumed.
    unsafe {
        core::ptr::copy_nonoverlapping(p, np, usable.min(newsize));
        free(p);
        (*my_heap()).stats.realloc_moved += 1;
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
    // SAFETY: own heap.
    unsafe { (*my_heap()).collect(force) };
}

/// Snapshot of the CALLING thread's heap counters (per-heap by design —
/// work-parity gates run single-threaded where these are exact).
pub fn stats() -> crate::heap::Stats {
    // SAFETY: own heap.
    unsafe { (*my_heap()).stats }
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
        // SAFETY: hb live.
        unsafe { (*heap_of(hb)).stats.realloc_in_place += 1 };
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
        (*heap_of(hb)).stats.realloc_moved += 1;
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
