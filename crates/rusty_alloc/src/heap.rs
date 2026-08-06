//! The heap: per-bin page queues, the small-size direct table, the generic
//! (heartbeat) allocation path, and free (mirrors `heap.c` + the hot parts of
//! `alloc.c`/`page.c`). M2: ONE global heap behind a lock (crate::alloc);
//! per-thread heaps + lock removal are M4.

use core::ptr;
use core::sync::atomic::Ordering;

use crate::bins::{self, BIN_COUNT, PAGES_DIRECT};
use crate::page::{
    Block, DelayedList, Page, XFLAG_DELAYED, XFLAG_NORMAL, block_next, page_all_free, page_collect,
    page_extend, page_pop, page_push_local, page_set_flag, pflags,
};
use crate::segment::{
    self, Segment, SegmentKind, huge_free, page_area, page_index, segment_of, span_alloc, span_free,
};
use crate::types::{
    BIN_FULL, BIN_HUGE, LARGE_OBJ_SIZE_MAX, MEDIUM_OBJ_SIZE_MAX, MEDIUM_PAGE_SLICES,
    SEGMENT_SLICE_SIZE, SMALL_OBJ_SIZE_MAX, SMALL_SIZE_MAX, wsize_from_size,
};

/// Largest bin index actually reachable from a size (rest are M3 large pages).
pub const MAX_NORMAL_BIN: usize = bins_max();

const fn bins_max() -> usize {
    // const-eval of bins::bin(MEDIUM_OBJ_SIZE_MAX)
    let w = wsize_from_size(MEDIUM_OBJ_SIZE_MAX) - 1;
    let b = (usize::BITS - 1 - w.leading_zeros()) as usize;
    ((b << 2) + ((w >> (b - 2)) & 0x03)) - 3
}

/// A doubly-linked queue of pages serving one bin.
#[derive(Clone, Copy)]
pub struct PageQueue {
    /// Front page — the one the fast path allocates from.
    pub first: *mut Page,
    /// Back page.
    pub last: *mut Page,
    /// Block size of every page in this queue.
    pub block_size: usize,
}

/// Always-on counters (plan §7.5 instrument #2): the primary evidence for
/// sub-1% bricks and the work-parity check for every A/B.
#[derive(Clone, Copy, Default)]
pub struct Stats {
    /// Successful allocations.
    pub allocs: u64,
    /// Frees.
    pub frees: u64,
    /// Entries into the generic (slow) path.
    pub generic: u64,
    /// Pages freshly carved.
    pub pages_fresh: u64,
    /// Normal segments reserved from the OS.
    pub segments: u64,
    /// Huge (dedicated-segment) allocations.
    pub huge_allocs: u64,
    /// Free-list extensions performed.
    pub extends: u64,
    /// Large (in-segment span) allocations.
    pub large_allocs: u64,
    /// Pages retired (span returned to the segment).
    pub pages_retired: u64,
    /// Empty segments returned to the OS.
    pub segments_freed: u64,
    /// Reallocs resolved in place (no move).
    pub realloc_in_place: u64,
    /// Reallocs that moved the block.
    pub realloc_moved: u64,
    /// Blocks processed off the delayed (cross-thread) list.
    pub delayed_frees: u64,
    /// Abandoned segments adopted from dead threads.
    pub reclaims: u64,
    /// Guarded objects handed out (secure/guarded builds).
    pub guarded: u64,
    /// Page-sized ranges purged (decommitted/reset) back to the OS.
    pub purges: u64,
}

impl Stats {
    /// Const-init zero stats.
    pub const fn new() -> Stats {
        Stats {
            allocs: 0,
            frees: 0,
            generic: 0,
            pages_fresh: 0,
            segments: 0,
            huge_allocs: 0,
            extends: 0,
            large_allocs: 0,
            pages_retired: 0,
            segments_freed: 0,
            realloc_in_place: 0,
            realloc_moved: 0,
            delayed_frees: 0,
            reclaims: 0,
            guarded: 0,
            purges: 0,
        }
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

/// The heap.
pub struct Heap {
    /// Page queues, indexed by bin; `pages[BIN_FULL]` parks full pages.
    pub pages: [PageQueue; BIN_COUNT],
    /// `direct[wsize] → page`: the small-malloc fast path (null → generic).
    pub direct: [*mut Page; PAGES_DIRECT],
    /// Owned Normal segments.
    pub segments: *mut Segment,
    /// Empty segments currently parked in the list. Policy: keep ONE empty
    /// segment cached (upstream caches segments too); free beyond that —
    /// without this, large alloc/free cycles pay a 32 MiB OS round-trip each.
    pub empty_segments: u32,
    /// Address of this heap's [`DelayedList`] (inside the owning HeapBox);
    /// pages carry it in `xheap` so remote threads can nudge us. Null only
    /// during const bootstrap.
    pub delayed: *const DelayedList,
    /// Huge (dedicated-segment) allocations owned by this heap, so
    /// delete/destroy/abandon can account for them.
    pub huge_segments: *mut Segment,
    /// Arena restriction: allocate segments only from this arena (−1 = any
    /// arena, then the OS — `mi_heap_new_in_arena`).
    pub arena_id: i32,
    /// Heap tag stamped on every page (visitor filtering; `mi_heap_new_ex`).
    pub tag: i32,
    /// Per-heap CSPRNG: free-list keys, guarded sampling (M8).
    pub rng: crate::random::Random,
    /// Guarded-object sampling: 1-in-N (0 = off), and the size window.
    pub guarded_rate: usize,
    /// Countdown to the next guarded object.
    pub guarded_count: usize,
    /// Minimum size eligible for a guard page.
    pub guarded_min: usize,
    /// Maximum size eligible for a guard page.
    pub guarded_max: usize,
    /// Counters.
    pub stats: Stats,
}

impl Heap {
    /// Const-init empty heap (static bootstrap: usable before any OS call).
    pub const fn new() -> Heap {
        let mut pages = [PageQueue {
            first: ptr::null_mut(),
            last: ptr::null_mut(),
            block_size: 0,
        }; BIN_COUNT];
        let mut i = 1;
        while i <= MAX_NORMAL_BIN {
            pages[i].block_size = bins::bin_size(i);
            i += 1;
        }
        Heap {
            pages,
            direct: [crate::page::empty_page_ptr(); PAGES_DIRECT],
            segments: ptr::null_mut(),
            empty_segments: 0,
            delayed: ptr::null(),
            huge_segments: ptr::null_mut(),
            arena_id: -1,
            tag: 0,
            rng: crate::random::Random::new(),
            guarded_rate: 0,
            guarded_count: 0,
            guarded_min: 0,
            guarded_max: usize::MAX,
            stats: Stats::new(),
        }
    }

    /// `mi_heap_guarded_set_sample_rate`: guard 1-in-N eligible objects
    /// (0 disables; 1 guards every one). A nonzero seed makes the sequence
    /// reproducible for debugging.
    pub fn guarded_set_sample_rate(&mut self, rate: usize, seed: usize) {
        self.guarded_rate = rate;
        if seed != 0 {
            let s = seed as u32;
            self.rng.seed_from(
                [s, s ^ 0x9E37, s.rotate_left(7), s ^ 0xA5A5, s, s, s, s],
                seed as u64,
            );
        }
        self.guarded_count = if rate == 0 {
            0
        } else {
            1 + self.rng.below(rate)
        };
    }

    /// `mi_heap_guarded_set_size_bound`.
    pub fn guarded_set_size_bound(&mut self, min: usize, max: usize) {
        self.guarded_min = min;
        self.guarded_max = max;
    }

    /// Whether this allocation should get a guard page (samples down the
    /// countdown). Only consulted in `secure`/guarded builds.
    #[inline]
    fn guarded_should_sample(&mut self, size: usize) -> bool {
        if self.guarded_rate == 0 || size < self.guarded_min || size > self.guarded_max {
            return false;
        }
        if self.guarded_count > 1 {
            self.guarded_count -= 1;
            return false;
        }
        self.guarded_count = 1 + self.rng.below(self.guarded_rate);
        true
    }

    /// Allocate `size` bytes. Returns (ptr-or-null, block-known-zero).
    #[inline]
    pub fn malloc(&mut self, size: usize) -> (*mut u8, bool) {
        if size <= SMALL_SIZE_MAX {
            let w = wsize_from_size(size);
            // NEVER null — an empty slot holds the shared empty-page sentinel,
            // whose free list is permanently null. That is what lets this be a
            // SINGLE test: "did we get a block?" also answers "was there a
            // page?" (upstream's `_mi_page_empty`).
            let p = self.direct[w];
            // SAFETY: direct entries always point at a live page of this heap
            // or at the immortal sentinel; we hold the heap lock.
            let b = unsafe { page_pop(p) };
            if !b.is_null() {
                self.stat_alloc();
                // SAFETY: p live per above.
                return (b, unsafe { (*p).free_is_zero });
            }
        }
        self.malloc_generic(size)
    }

    /// The slow path — mimalloc's heartbeat: runs when a fast list is dry, so
    /// deferred work (collect, extend, fresh pages; later: purge, deferred
    /// frees) happens at a regular allocation cadence.
    fn malloc_generic(&mut self, size: usize) -> (*mut u8, bool) {
        self.stats.generic += 1;
        // Guarded objects (secure/guarded builds): sampled allocations get a
        // dedicated segment whose trailing page is PROT_NONE, so an overflow
        // faults immediately instead of corrupting a neighbour.
        if self.guarded_rate != 0 && self.guarded_should_sample(size) {
            let (p, z) = self.guarded_alloc(size);
            if !p.is_null() {
                return (p, z);
            }
        }
        // Heartbeat: process cross-thread delayed frees at slow-path cadence
        // (this is what un-parks full pages whose blocks died remotely), and
        // fire the registered deferred-free hook (mi_register_deferred_free).
        // SAFETY: we are the owner thread.
        unsafe { self.process_delayed() };
        crate::options::deferred_free(false);
        if size > MEDIUM_OBJ_SIZE_MAX {
            return if size <= LARGE_OBJ_SIZE_MAX {
                self.large_alloc(size)
            } else {
                self.huge_alloc(size, 8, 0)
            };
        }
        let bin = bins::bin(size);
        // SAFETY: all page/queue manipulation below happens under the heap
        // lock on pages owned by this heap; raw pointers are used so no two
        // Rust references to the same Page coexist.
        unsafe {
            let q: *mut PageQueue = &raw mut self.pages[bin];
            let mut p = (*q).first;
            while !p.is_null() {
                page_collect(p);
                if (*p).free.is_null() && (*p).capacity < (*p).reserved {
                    let seg = segment_of(p.cast());
                    let idx = page_index(seg, p);
                    page_extend(p, page_area(seg, idx));
                    self.stats.extends += 1;
                }
                if !(*p).free.is_null() {
                    if p != (*q).first {
                        queue_remove(q, p);
                        queue_push_front(q, p);
                    }
                    self.update_direct(bin);
                    let b = page_pop(p);
                    self.stat_alloc();
                    return (b, (*p).free_is_zero);
                }
                // Truly full: park it so the queue front stays useful. The
                // DELAYED flag makes remote frees nudge our delayed list —
                // a parked page is invisible to scans (loom-modeled).
                let next = (*p).next;
                queue_remove(q, p);
                (*p).flags |= pflags::IN_FULL;
                page_set_flag(p, XFLAG_DELAYED);
                queue_push_front(&raw mut self.pages[BIN_FULL], p);
                p = next;
            }
            // No usable page — carve a fresh one.
            let bsize = (*q).block_size;
            let p = self.fresh_page(bin, bsize);
            if p.is_null() {
                self.update_direct(bin);
                return (ptr::null_mut(), false); // OOM
            }
            let seg = segment_of(p.cast());
            let idx = page_index(seg, p);
            page_extend(p, page_area(seg, idx));
            self.stats.extends += 1;
            self.update_direct(bin);
            let b = page_pop(p);
            self.stat_alloc();
            (b, (*p).free_is_zero)
        }
    }

    /// Carve a fresh page for `bin` from the segment list (new segment if all
    /// are exhausted) and push it to the queue front.
    fn fresh_page(&mut self, bin: usize, bsize: usize) -> *mut Page {
        let slices = if bsize <= SMALL_OBJ_SIZE_MAX {
            1
        } else {
            MEDIUM_PAGE_SLICES
        };
        // SAFETY: heap lock held; segments list is owned by this heap.
        unsafe {
            let (p, fresh) = match self.span_from_segments(slices) {
                Some(v) => v,
                None => return ptr::null_mut(),
            };
            (*p).block_size = bsize;
            (*p).reserved = ((slices * SEGMENT_SLICE_SIZE) / bsize) as u32;
            (*p).capacity = 0;
            (*p).used = 0;
            (*p).free = ptr::null_mut();
            (*p).local_free = ptr::null_mut();
            (*p).bin = bin as u8;
            (*p).flags = 0;
            // Bump-fresh spans of an eager-committed zero mapping are zero;
            // RECLAIMED spans are recycled memory and are not.
            (*p).free_is_zero = fresh;
            (*p).xheap.store(self.delayed as usize, Ordering::Release);
            (*p).heap_tag = self.tag;
            #[cfg(feature = "secure")]
            {
                // Fresh per-page keys: an attacker who learns one page's
                // encoding cannot steer another.
                (*p).keys = [self.rng.next_usize() | 1, self.rng.next_usize()];
            }
            page_set_flag(p, XFLAG_NORMAL);
            self.stats.pages_fresh += 1;
            queue_push_front(&raw mut self.pages[bin], p);
            p
        }
    }

    /// First-fit a span across owned segments, allocating a new segment when
    /// all are exhausted. Returns (span start, span-is-fresh-zero).
    ///
    /// # Safety
    /// Heap lock held.
    unsafe fn span_from_segments(&mut self, slices: usize) -> Option<(*mut Page, bool)> {
        // SAFETY: heap lock held; segments list is owned by this heap.
        unsafe {
            let mut seg = self.segments;
            while !seg.is_null() {
                let was_empty = (*seg).used_pages == 0;
                let (p, fresh) = span_alloc(seg, slices);
                if !p.is_null() {
                    if was_empty {
                        self.empty_segments -= 1;
                    }
                    return Some((p, fresh));
                }
                seg = (*seg).next;
            }
            // Before reserving fresh OS memory: adopt abandoned segments from
            // dead threads (bounded — this is a slow-path heartbeat duty).
            // ADOPT UNTIL SATISFIED, not twice. The old bound was `tries < 2`:
            // with orphans piled up we adopted at most two, re-scanned, and
            // then took a FRESH 32 MiB segment from the arena anyway while the
            // rest sat unclaimed. Measured: 25 abandoned segments, and a
            // 2048-block allocation burst reclaimed 4. Each orphan is 32 MiB,
            // which is the RSS tail.
            //
            // Now each adopted segment is tried IMMEDIATELY — the one just
            // taken is the one most likely to have room — and the loop stops as
            // soon as the request is met or the list is empty. The cap only
            // exists so a huge orphan backlog cannot stall one allocation; it
            // is not a reclaim budget.
            const MAX_ADOPT: usize = 32;
            for _ in 0..MAX_ADOPT {
                let aseg = crate::init::abandoned_pop();
                if aseg.is_null() {
                    break;
                }
                self.adopt_segment(aseg);
                let was_empty = (*aseg).used_pages == 0;
                let (p, fresh) = span_alloc(aseg, slices);
                if !p.is_null() {
                    if was_empty && self.empty_segments > 0 {
                        self.empty_segments -= 1;
                    }
                    return Some((p, fresh));
                }
            }
            let seg = segment::segment_alloc(self.arena_id).ok()?;
            self.stats.segments += 1;
            (*seg).next = self.segments;
            self.segments = seg;
            let (p, fresh) = span_alloc(seg, slices);
            debug_assert!(!p.is_null());
            Some((p, fresh))
        }
    }

    /// Large objects (64 KiB..16 MiB): one single-block page spanning enough
    /// slices, allocated fresh per request and retired on free (no queue —
    /// span reclamation IS the reuse mechanism).
    fn large_alloc(&mut self, size: usize) -> (*mut u8, bool) {
        let slices = size.div_ceil(SEGMENT_SLICE_SIZE);
        // SAFETY: heap lock held; page/segment owned by this heap.
        unsafe {
            let Some((p, fresh)) = self.span_from_segments(slices) else {
                return (ptr::null_mut(), false);
            };
            (*p).block_size = slices * SEGMENT_SLICE_SIZE;
            (*p).reserved = 1;
            (*p).capacity = 1;
            (*p).used = 1;
            (*p).free = ptr::null_mut();
            (*p).local_free = ptr::null_mut();
            (*p).bin = BIN_HUGE as u8; // marker: unqueued single-block span
            (*p).flags = pflags::SINGLE_BLOCK; // unqueued single-block span
            (*p).free_is_zero = fresh;
            // Unqueued → never scanned → remote frees must go via the
            // delayed list (same rule as parked-full pages).
            (*p).xheap.store(self.delayed as usize, Ordering::Release);
            (*p).heap_tag = self.tag;
            page_set_flag(p, XFLAG_DELAYED);
            let seg = segment_of(p.cast::<u8>());
            let idx = page_index(seg, p);
            self.stat_alloc();
            self.stats.large_allocs += 1;
            (page_area(seg, idx), fresh)
        }
    }

    /// Guarded allocation: a dedicated huge segment whose page after the
    /// block is protected. The block is placed so its END abuts the guard
    /// (buffer overflows fault on the first byte past the object).
    fn guarded_alloc(&mut self, size: usize) -> (*mut u8, bool) {
        let ps = crate::os::page_size();
        let payload = crate::os::page_align_up(size.max(1));
        // huge_alloc gives us a dedicated segment with a page-aligned block.
        let (block, _z) = self.huge_alloc(payload + ps, ps, 0);
        if block.is_null() {
            return (ptr::null_mut(), false);
        }
        // SAFETY: block is the start of a fresh dedicated reservation with at
        // least payload + one page of committed memory.
        unsafe {
            let guard = block.add(payload);
            if crate::os::protect(guard, ps, true).is_err() {
                return (block, true); // protection unavailable: still valid memory
            }
            // Record it on the SEGMENT: the protection must be lifted before
            // this memory can be recycled through an arena (the M8 P0).
            (*segment_of(block)).guarded = true;
            self.stats.guarded += 1;
            // Right-align the object against the guard page.
            let p = guard.sub(size.max(1));
            let seg = segment_of(block);
            let pg: *mut Page = &raw mut (*seg).pages[1];
            (*pg).flags |= pflags::HAS_ALIGNED;
            (p, true)
        }
    }

    fn huge_alloc(&mut self, size: usize, align: usize, offset: usize) -> (*mut u8, bool) {
        match segment::huge_alloc(size, align, offset) {
            Ok((seg, block)) => {
                // SAFETY: fresh segment we own; page slot 1 is its block's
                // metadata. DELAYED + xheap route remote frees through our
                // delayed list, unifying huge with the protocol.
                unsafe {
                    let pg: *mut Page = &raw mut (*seg).pages[1];
                    (*pg).xheap.store(self.delayed as usize, Ordering::Release);
                    (*pg).flags |= pflags::HUGE_SEGMENT | pflags::SINGLE_BLOCK;
                    page_set_flag(pg, XFLAG_DELAYED);
                    (*seg).next = self.huge_segments;
                    self.huge_segments = seg;
                }
                self.stat_alloc();
                self.stats.huge_allocs += 1;
                // SAFETY: seg live; recycled arena chunks are NOT zero.
                (block, unsafe { (*seg).mem_is_zero })
            }
            Err(_) => (ptr::null_mut(), false),
        }
    }

    /// Unlink a huge segment from this heap's list.
    ///
    /// # Safety
    /// Owner thread; `seg` is in the list.
    unsafe fn remove_huge_segment(&mut self, seg: *mut Segment) {
        // SAFETY: list owned by this heap.
        unsafe {
            let mut cur = &raw mut self.huge_segments;
            while !(*cur).is_null() {
                if *cur == seg {
                    *cur = (*seg).next;
                    return;
                }
                cur = &raw mut (**cur).next;
            }
            debug_assert!(false, "huge segment not in heap list");
        }
    }

    /// Aligned allocation (M5: the full `_at` form): returns `p` with
    /// `(p + offset) % align == 0`. Tiers, cheapest first:
    /// natural fit through the bins (blocks sit at `i * bsize` in 64 KiB-
    /// aligned areas, so `bsize % align == 0` ⟺ every block qualifies);
    /// oversize-and-adjust (interior pointer, page marked `has_aligned`);
    /// dedicated huge segment with computed placement.
    pub fn malloc_aligned_at(
        &mut self,
        size: usize,
        align: usize,
        offset: usize,
    ) -> (*mut u8, bool) {
        if !align.is_power_of_two() || align > crate::types::SEGMENT_SIZE / 2 {
            return (ptr::null_mut(), false);
        }
        if align <= 8 && offset == 0 {
            return self.malloc(size);
        }
        // Natural fit: only sound when the offset keeps block starts aligned.
        if offset.is_multiple_of(align)
            && size <= MEDIUM_OBJ_SIZE_MAX
            && align <= SEGMENT_SLICE_SIZE
        {
            if bins::good_size(size).is_multiple_of(align) {
                return self.malloc(size);
            }
            let asize = (size.max(1) + align - 1) & !(align - 1);
            if bins::good_size(asize).is_multiple_of(align) {
                return self.malloc(asize);
            }
        }
        self.stats.generic += 1;
        // Huge sizes get exact placement in a dedicated segment.
        if size > LARGE_OBJ_SIZE_MAX {
            return self.huge_alloc(size, align, offset);
        }
        // Oversize-and-adjust: allocate size + align - 1, hand out the first
        // interior position satisfying the constraint.
        let oversize = size.max(1) + align - 1;
        if oversize > LARGE_OBJ_SIZE_MAX {
            return self.huge_alloc(size, align, offset);
        }
        let (block, is_zero) = self.malloc(oversize);
        if block.is_null() {
            return (ptr::null_mut(), false);
        }
        let p = block.with_addr(((block.addr() + offset + align - 1) & !(align - 1)) - offset);
        debug_assert!((p.addr() + offset).is_multiple_of(align));
        if p != block {
            // SAFETY: block is ours, just allocated on this thread; marking
            // the page before the pointer escapes (see Page::has_aligned).
            unsafe {
                let seg = segment_of(block);
                let pg = segment::page_of(seg, block);
                (*pg).flags |= pflags::HAS_ALIGNED;
            }
        }
        // A fresh-zero block is zero at every interior position too.
        (p, is_zero)
    }

    /// Process cross-thread delayed frees (the heartbeat's first duty).
    ///
    /// # Safety
    /// Caller must be this heap's owning thread.
    pub unsafe fn process_delayed(&mut self) {
        if self.delayed.is_null() {
            return;
        }
        // SAFETY: delayed points into our own live HeapBox; blocks on it are
        // dead blocks of pages we own.
        unsafe {
            let mut b = (*self.delayed).head.swap(0, Ordering::AcqRel) as *mut Block;
            while !b.is_null() {
                let next = (*b).next;
                self.free_local(b.cast());
                self.stats.delayed_frees += 1;
                b = next;
            }
        }
    }

    /// `mi_collect`: drain delayed frees and collect every queued page,
    /// retiring the empties (force is accepted for ABI shape; M4 treats all
    /// collects as forced).
    ///
    /// # Safety
    /// Caller must be this heap's owning thread.
    pub unsafe fn collect(&mut self, _force: bool) {
        // SAFETY: owner thread per contract.
        unsafe {
            self.process_delayed();
            let mut bin = 1;
            while bin <= MAX_NORMAL_BIN {
                let q: *mut PageQueue = &raw mut self.pages[bin];
                let mut p = (*q).first;
                while !p.is_null() {
                    let next = (*p).next;
                    page_collect(p);
                    if page_all_free(p) && !((*q).first == p && (*q).last == p) {
                        queue_remove(q, p);
                        self.update_direct(bin);
                        let seg = segment_of(p.cast::<u8>());
                        self.retire_span(seg, p);
                    }
                    p = next;
                }
                bin += 1;
            }
        }
    }

    /// Adopt an abandoned segment: take ownership, re-home every live page,
    /// retire the already-dead ones (called with the segment OFF the global
    /// abandoned list, thread_id already set to us by the caller).
    ///
    /// # Safety
    /// `seg` must be a Normal segment popped from the abandoned list, owned
    /// by nobody; calling thread becomes the owner.
    pub unsafe fn adopt_segment(&mut self, seg: *mut Segment) {
        // SAFETY: we are the sole owner as of now; walking spans follows the
        // segment invariants (every span start is marked).
        unsafe {
            if (*seg).kind == SegmentKind::Huge {
                // Single-block segment: re-home, collect any NEVER-pushed
                // free, release if its block already died.
                let pg: *mut Page = &raw mut (*seg).pages[1];
                (*pg).xheap.store(self.delayed as usize, Ordering::Release);
                page_set_flag(pg, XFLAG_DELAYED);
                page_collect(pg);
                self.stats.reclaims += 1;
                if (*pg).used == 0 {
                    let _ = huge_free(seg);
                    self.stats.segments_freed += 1;
                } else {
                    (*seg).next = self.huge_segments;
                    self.huge_segments = seg;
                }
                return;
            }
            (*seg).next = self.segments;
            self.segments = seg;
            self.stats.reclaims += 1;
            let end = (*seg).next_free_slice as usize;
            let mut idx = segment::HEADER_SLICES;
            while idx < end {
                let slot: *mut Page = &raw mut (*seg).pages[idx];
                let len = ((*slot).slice_count as usize).max(1);
                if (*slot).block_size > 0 {
                    (*slot)
                        .xheap
                        .store(self.delayed as usize, Ordering::Release);
                    if ((*slot).bin as usize) <= MAX_NORMAL_BIN {
                        (*slot).flags &= !pflags::IN_FULL;
                        (*slot).next = ptr::null_mut();
                        (*slot).prev = ptr::null_mut();
                        page_set_flag(slot, XFLAG_NORMAL);
                        page_collect(slot);
                        // Do NOT retire empty pages during this walk:
                        // span_free COALESCES, rewriting the span layout we
                        // are iterating, so the next step can land mid-span
                        // and queue a bogus page (rare corrupting race found
                        // by the M8 parallel gate). Empty pages are queued
                        // like any other and retired by the next collect.
                        queue_push_front(&raw mut self.pages[(*slot).bin as usize], slot);
                    } else {
                        // Large single-block span: stays unqueued; a dead one
                        // is retired by a later collect, same rule as above.
                        page_set_flag(slot, XFLAG_DELAYED);
                        page_collect(slot);
                    }
                }
                idx += len;
            }
            // Queue fronts changed wholesale — rebuild the small-bin table.
            let mut bin = 1;
            while bin <= MAX_NORMAL_BIN {
                self.update_direct(bin);
                bin += 1;
            }
            // Adopted large spans that are already dead: retire them now that
            // the walk is over (the layout is ours to mutate again).
            let mut idx = segment::HEADER_SLICES;
            while idx < (*seg).next_free_slice as usize {
                let slot: *mut Page = &raw mut (*seg).pages[idx];
                let len = ((*slot).slice_count as usize).max(1);
                if (*slot).block_size > 0
                    && (*slot).slice_offset == 0
                    && (*slot).bin as usize == BIN_HUGE
                    && (*slot).used == 0
                {
                    self.retire_span(seg, slot);
                    break; // layout changed: leave the rest to later collects
                }
                idx += len;
            }
            if (*seg).used_pages == 0 {
                if self.empty_segments == 0 {
                    self.empty_segments = 1;
                } else {
                    self.remove_segment(seg);
                    let _ = segment::segment_free(seg);
                    self.stats.segments_freed += 1;
                }
            }
        }
    }

    /// Free a block owned by THIS heap's thread. Null is a no-op.
    ///
    /// # Safety
    /// `p` must be null or a live pointer whose segment is owned by the
    /// calling thread (or a Huge segment); routing is `alloc::free`'s job.
    pub unsafe fn free_local(&mut self, p: *mut u8) {
        if p.is_null() {
            return;
        }
        let seg = segment_of(p);
        // SAFETY: forwarded contract; resolve the page once for the callee.
        unsafe {
            let pg = segment::page_of(seg, p);
            self.free_local_at(seg, pg, p)
        }
    }

    /// Work-parity counters, present only in debug builds.
    ///
    /// This mirrors upstream exactly: mimalloc defines `MI_STAT 2` when
    /// `MI_DEBUG > 0` and **`MI_STAT 0` otherwise**, so the release oracle we
    /// benchmark against carries no counters at all. Ours were unconditional,
    /// which both cost real instructions on the hottest paths in the allocator
    /// and made every published ratio an unfair comparison — a counters-on
    /// build measured against a counters-off one. They stay on for `cargo
    /// test` (a debug profile), which is where the counters do their job:
    /// proving two binaries perform identical work.
    #[inline(always)]
    fn stat_alloc(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.stats.allocs += 1;
        }
    }

    #[inline(always)]
    fn stat_free(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.stats.frees += 1;
        }
    }

    /// The tail of a local free that emptied its page: unqueue and retire the
    /// span, unless this is the bin's only page (keep one warm — approximates
    /// upstream's retire delay).
    ///
    /// This is the ONLY part of a fast-path free that needs the owning heap —
    /// upstream's `mi_free_block_local` touches none — and it runs on a small
    /// fraction of frees, so `alloc::free` inlines the rest and calls here
    /// only when a page actually empties.
    ///
    /// Stays `#[inline]`, and that was MEASURED. Marking it `#[cold]` +
    /// `#[inline(never)]` — on the theory that inlining its tree (`retire_span`
    /// -> `span_free` -> `segment_free`) was what forced `alloc::free` to save
    /// callee-saved registers on the fast path — made things WORSE: perl
    /// 1.0068 -> 1.0084, sqlite 1.0041 -> 1.0048, and `free`'s prologue gained
    /// back the `push r15` it was supposed to remove.
    ///
    /// # Safety
    /// `pg` is a queued, now-empty binned page of `seg`, owned by this heap.
    #[inline]
    pub unsafe fn retire_emptied(&mut self, seg: *mut Segment, pg: *mut Page) {
        // SAFETY: forwarded contract.
        unsafe {
            let bin = (*pg).bin as usize;
            let q: *mut PageQueue = &raw mut self.pages[bin];
            // NOTE (RSS investigation, 2026-08-06): deferring this retire —
            // keeping the emptied page queued so the next round reuses the SAME
            // memory instead of first-fitting a span elsewhere — was tried and
            // measured NO CHANGE to peak RSS (62.8 vs 62.6 MiB). Span re-carve
            // churn is real (504 pages retired and re-carved per round) but it
            // is NOT the +18% gap. Reverted; do not re-try without a new
            // mechanism.
            if !((*q).first == pg && (*q).last == pg) {
                queue_remove(q, pg);
                self.update_direct(bin);
                self.retire_span(seg, pg);
            }
        }
    }

    /// Free with the segment ALREADY resolved. `alloc::free` computes the
    /// segment (and page) to route ownership; recomputing them here cost a
    /// mask + shift + two loads on every free for nothing (M9 brick #2,
    /// byte-identical behaviour).
    ///
    /// # Safety
    /// As [`free_local`], with `seg == segment_of(p)`.
    pub unsafe fn free_local_at(&mut self, seg: *mut Segment, pg: *mut Page, p: *mut u8) {
        debug_assert_eq!(seg, segment_of(p));
        // SAFETY: caller resolved this page already.
        debug_assert!(pg.is_null() || unsafe { pg == segment::page_of(seg, p) });
        // SAFETY: p is ours per the contract, so seg is a live segment header.
        unsafe {
            match (*seg).kind {
                SegmentKind::Huge => {
                    self.stat_free();
                    self.remove_huge_segment(seg);
                    let _ = huge_free(seg);
                }
                SegmentKind::Normal => {
                    // Page ALREADY resolved by alloc::free -- resolving it a
                    // second time here was the single biggest remaining item
                    // in the free path (page_of is the allocator's hottest
                    // function; the profile showed free costing 2.7x
                    // mimalloc's while our malloc was already at parity).
                    self.stat_free();
                    if (*pg).bin as usize == BIN_HUGE {
                        // Large single-block span: retire immediately.
                        self.retire_span(seg, pg);
                        return;
                    }
                    page_push_local(pg, p.cast());
                    if (*pg).flags & pflags::IN_FULL != 0 {
                        // Un-park: it has space again; back to NORMAL remote
                        // pushes (page is scannable once more).
                        (*pg).flags &= !pflags::IN_FULL;
                        queue_remove(&raw mut self.pages[BIN_FULL], pg);
                        page_set_flag(pg, XFLAG_NORMAL);
                        let bin = (*pg).bin as usize;
                        queue_push_front(&raw mut self.pages[bin], pg);
                        self.update_direct(bin);
                    }
                    if page_all_free(pg) {
                        // Retire unless it is the queue's only page (keep one
                        // warm — approximates upstream's retire delay).
                        let bin = (*pg).bin as usize;
                        let q: *mut PageQueue = &raw mut self.pages[bin];
                        if !((*q).first == pg && (*q).last == pg) {
                            queue_remove(q, pg);
                            self.update_direct(bin);
                            self.retire_span(seg, pg);
                        }
                    }
                }
            }
        }
    }

    /// Return a span to its segment; release the segment when it empties.
    ///
    /// # Safety
    /// Heap lock held; `pg` is an unqueued span-start of `seg` with no live
    /// blocks.
    unsafe fn retire_span(&mut self, seg: *mut Segment, pg: *mut Page) {
        // SAFETY: forwarded contract.
        unsafe {
            // span_free coalesces, then purges the merged span (the RSS
            // lever) and reports whether it did.
            if span_free(seg, pg) {
                self.stats.purges += 1;
            }
            self.stats.pages_retired += 1;
            if (*seg).used_pages == 0 {
                if self.empty_segments == 0 {
                    // Park one empty segment for reuse (stays in the list —
                    // span_from_segments will find its full free span).
                    self.empty_segments = 1;
                } else {
                    self.remove_segment(seg);
                    let _ = segment::segment_free(seg);
                    self.stats.segments_freed += 1;
                }
            }
        }
    }

    /// Unlink `seg` from the heap's segment list.
    ///
    /// # Safety
    /// Heap lock held; `seg` is in the list.
    unsafe fn remove_segment(&mut self, seg: *mut Segment) {
        // SAFETY: list owned by this heap under the lock.
        unsafe {
            let mut cur = &raw mut self.segments;
            while !(*cur).is_null() {
                if *cur == seg {
                    *cur = (*seg).next;
                    return;
                }
                cur = &raw mut (**cur).next;
            }
            debug_assert!(false, "segment not in heap list");
        }
    }

    // realloc/expand/usable_size moved to `alloc` in M4: they operate on
    // blocks that may be owned by OTHER threads, so they must not require
    // `&mut` on any particular heap.

    /// `mi_heap_visit_blocks` core: walk every live page area of this heap
    /// (Normal spans + huge blocks); optionally enumerate allocated blocks.
    /// The visitor returns false to stop; the return mirrors that.
    ///
    /// # Safety
    /// Owner thread.
    pub unsafe fn visit_blocks(
        &mut self,
        visit_blocks: bool,
        f: &mut dyn FnMut(&AreaInfo, *mut u8, usize) -> bool,
    ) -> bool {
        // SAFETY: owner thread; collect keeps free-list snapshots exact.
        unsafe {
            let mut seg = self.segments;
            while !seg.is_null() {
                if !visit_segment_blocks(seg, -1, visit_blocks, true, f) {
                    return false;
                }
                seg = (*seg).next;
            }
            let mut seg = self.huge_segments;
            while !seg.is_null() {
                if !visit_segment_blocks(seg, -1, visit_blocks, true, f) {
                    return false;
                }
                seg = (*seg).next;
            }
        }
        true
    }

    /// `mi_unsafe_heap_page_is_under_utilized`.
    ///
    /// # Safety
    /// Owner thread; `p` a live pointer of this heap.
    pub unsafe fn page_under_utilized(&mut self, p: *mut u8, perc: usize) -> bool {
        let seg = segment_of(p);
        // SAFETY: p ours per contract.
        unsafe {
            let pg = segment::page_of(seg, p);
            page_collect(pg);
            ((*pg).used as usize) * 100 < ((*pg).capacity as usize) * perc
        }
    }

    /// Refresh the direct table for `bin` to its current queue front.
    fn update_direct(&mut self, bin: usize) {
        let bsize = bins::bin_size(bin);
        if bsize > SMALL_SIZE_MAX {
            return;
        }
        // An empty queue publishes the sentinel, not null, so `malloc` never
        // has to test for a missing page.
        let page = match self.pages[bin].first {
            p if p.is_null() => crate::page::empty_page_ptr(),
            p => p,
        };
        let w_hi = bsize / crate::types::INTPTR_SIZE;
        // With ALIGN2W, bins ≤ 8 exist only at 1 and even indices; each even
        // bin also serves the odd wsize below it. Bins > 8 are contiguous.
        let w_lo = match bin {
            0 | 1 => 0,
            2 => 2,
            3..=8 => bin - 1,
            _ => bins::bin_size(bin - 1) / crate::types::INTPTR_SIZE + 1,
        };
        let mut w = w_lo;
        while w <= w_hi {
            self.direct[w] = page;
            w += 1;
        }
    }
}

/// Area descriptor handed to block visitors (mirrors `mi_heap_area_t`).
pub struct AreaInfo {
    /// Start of the page's block area.
    pub blocks: *mut u8,
    /// Bytes reserved for this area.
    pub reserved: usize,
    /// Bytes currently usable (committed; == reserved under eager commit).
    pub committed: usize,
    /// Allocated blocks in the area.
    pub used: usize,
    /// Block size.
    pub block_size: usize,
    /// Block size including padding/metadata (== block_size here).
    pub full_block_size: usize,
    /// The owning heap's tag at stamp time.
    pub heap_tag: i32,
}

/// Visit every live page of ONE segment; optionally each allocated block.
/// `tag_filter >= 0` skips pages with a different heap tag. `owner` selects
/// exact snapshots (collect) vs read-only walks (abandoned segments).
///
/// # Safety
/// `seg` live; if `owner`, calling thread owns it; else it must be pinned
/// (e.g. the abandoned-list lock is held).
pub unsafe fn visit_segment_blocks(
    seg: *mut Segment,
    tag_filter: i32,
    visit_blocks: bool,
    owner: bool,
    f: &mut dyn FnMut(&AreaInfo, *mut u8, usize) -> bool,
) -> bool {
    // SAFETY: per contract; span walk follows the segment invariants.
    unsafe {
        if (*seg).kind == SegmentKind::Huge {
            let pg: *mut Page = &raw mut (*seg).pages[1];
            if (*pg).used == 0 || (tag_filter >= 0 && (*pg).heap_tag != tag_filter) {
                return true;
            }
            let bsize = (*pg).block_size;
            let block = seg.cast::<u8>().add((*seg).total_size - bsize);
            let area = AreaInfo {
                blocks: block,
                reserved: bsize,
                committed: bsize,
                used: 1,
                block_size: bsize,
                full_block_size: bsize,
                heap_tag: (*pg).heap_tag,
            };
            if !f(&area, ptr::null_mut(), 0) {
                return false;
            }
            if visit_blocks && !f(&area, block, bsize) {
                return false;
            }
            return true;
        }
        let end = (*seg).next_free_slice as usize;
        let mut idx = segment::HEADER_SLICES;
        while idx < end {
            let slot: *mut Page = &raw mut (*seg).pages[idx];
            let len = ((*slot).slice_count as usize).max(1);
            if (*slot).block_size == 0 || (tag_filter >= 0 && (*slot).heap_tag != tag_filter) {
                idx += len;
                continue;
            }
            if owner {
                page_collect(slot);
            }
            let bsize = (*slot).block_size;
            let area_ptr = page_area(seg, idx);
            let area = AreaInfo {
                blocks: area_ptr,
                reserved: len * SEGMENT_SLICE_SIZE,
                committed: len * SEGMENT_SLICE_SIZE,
                used: (*slot).used as usize,
                block_size: bsize,
                full_block_size: bsize,
                heap_tag: (*slot).heap_tag,
            };
            if !f(&area, ptr::null_mut(), 0) {
                return false;
            }
            if visit_blocks && (*slot).used > 0 {
                // Free-mark bitmap over capacity blocks (≤ 8192 → 1 KiB stack).
                let cap = (*slot).capacity as usize;
                let mut freemap = [0u64; 128];
                // Bounds-checked marking: a link that does not resolve to a
                // block of THIS page means the list is corrupt — skip it
                // rather than index the bitmap out of range.
                let mut mark = |b: *mut Block| {
                    let addr = b.addr();
                    if addr < area_ptr.addr() {
                        return;
                    }
                    let off = (addr - area_ptr.addr()) / bsize;
                    if off < cap {
                        freemap[off / 64] |= 1 << (off % 64);
                    }
                };
                // Links must be read through block_next: they are ENCODED in
                // secure builds (reading them raw here indexed the bitmap
                // with garbage — found by the M8 parallel gate).
                let mut b = (*slot).free;
                while !b.is_null() {
                    mark(b);
                    b = block_next(slot, b);
                }
                let mut b = (*slot).local_free;
                while !b.is_null() {
                    mark(b);
                    b = block_next(slot, b);
                }
                // Cross-thread chain: snapshot the head; nodes are stable
                // once pushed (never unlinked until a collect).
                let x = (*slot).xthread_free.load(Ordering::Acquire);
                let mut b = crate::ptr_with_addr(slot.cast::<Block>(), x & !crate::page::XMASK);
                while !b.is_null() {
                    mark(b);
                    b = block_next(slot, b);
                }
                for i in 0..cap {
                    if freemap[i / 64] & (1 << (i % 64)) == 0 {
                        let block = area_ptr.add(i * bsize);
                        if !f(&area, block, bsize) {
                            return false;
                        }
                    }
                }
            }
            idx += len;
        }
        true
    }
}

/// Push `page` at the queue front.
///
/// # Safety
/// Heap lock held; `page` not currently in any queue.
unsafe fn queue_push_front(q: *mut PageQueue, page: *mut Page) {
    // SAFETY: caller contract — page is a live slot.
    unsafe { crate::page::debug_validate_page(page, "queue_push_front") };
    // SAFETY: caller contract.
    unsafe {
        (*page).prev = ptr::null_mut();
        (*page).next = (*q).first;
        if !(*q).first.is_null() {
            (*(*q).first).prev = page;
        } else {
            (*q).last = page;
        }
        (*q).first = page;
    }
}

/// Unlink `page` from its queue.
///
/// # Safety
/// Heap lock held; `page` currently linked in `q`.
unsafe fn queue_remove(q: *mut PageQueue, page: *mut Page) {
    // SAFETY: caller contract.
    unsafe {
        if !(*page).prev.is_null() {
            (*(*page).prev).next = (*page).next;
        } else {
            (*q).first = (*page).next;
        }
        if !(*page).next.is_null() {
            (*(*page).next).prev = (*page).prev;
        } else {
            (*q).last = (*page).prev;
        }
        (*page).next = ptr::null_mut();
        (*page).prev = ptr::null_mut();
    }
}
