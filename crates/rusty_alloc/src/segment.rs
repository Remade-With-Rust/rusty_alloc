//! Segments: 32 MiB-aligned reservations sliced into 64 KiB slices (M3 subset
//! of upstream `segment.c`). The alignment IS the addressing scheme —
//! `ptr → segment` is a mask, `ptr → page` two shifts and a table walk.
//!
//! M3 adds SPAN RECLAMATION: freed page spans return to a per-segment
//! first-fit free list with left/right coalescing, so pages of any size class
//! can reuse the space (M2 leaked slices to their first size class forever).
//!
//! Span invariants (what makes coalescing O(1)):
//! - The carved region `[HEADER_SLICES, next_free_slice)` is partitioned into
//!   spans; every span's FIRST slot has `slice_offset == 0`, and slot
//!   `block_size > 0` ⟺ the span is a live page (0 ⟺ free span).
//! - Every span's LAST slot's `slice_offset` points back to its first slot, so
//!   the left neighbor of any span is found by one follow-back.
//! - Free spans link through `next`/`prev` into `Segment::free_spans`.
//!
//! Eager commit (the oracle's default); decommit/purge of free spans is the
//! purge policy work of M7.

use core::ptr;
use core::sync::atomic::AtomicUsize;

use crate::os;
use crate::page::Page;
use crate::prim::PrimError;
use crate::segment_map;
use crate::types::{SEGMENT_SIZE, SEGMENT_SLICE_SIZE, SLICES_PER_SEGMENT};

/// What a segment holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SegmentKind {
    /// Sliced segment holding small/medium/large pages.
    Normal = 0xA110C,
    /// Dedicated reservation for one huge block (> 16 MiB).
    Huge = 0x40BE,
}

/// Segment header — occupies slice 0 of the reservation.
pub struct Segment {
    /// Kind tag (doubles as a magic for `debug_checks`).
    pub kind: SegmentKind,
    /// Total reserved bytes (== SEGMENT_SIZE for Normal; the whole reservation
    /// for Huge — needed to free it).
    pub total_size: usize,
    /// Bump cursor: slices at/after this index have never been carved.
    pub next_free_slice: u32,
    /// Live pages carved from this segment.
    pub used_pages: u32,
    /// Owning thread id (`prim::thread_id`); 0 = abandoned. Gates the
    /// local-vs-remote routing in `alloc::free`.
    pub thread_id: AtomicUsize,
    /// The mapping's never-carved region is known zero (fresh OS memory).
    /// False for recycled arena chunks — bump-fresh spans then are NOT zero.
    pub mem_is_zero: bool,
    /// Any span of this segment was purged (decommitted) at some point. The
    /// segment must be RE-COMMITTED in full before it goes back to an arena:
    /// the next tenant bump-allocates from it and would otherwise touch
    /// decommitted pages (Windows access violation — the M8 defect).
    pub purged_any: bool,
    /// This segment carries a PROT_NONE guard page (guarded allocation). The
    /// protection MUST be lifted before the memory is recycled through an
    /// arena, or the next tenant faults on a page it legitimately owns —
    /// the M8 P0. (OS-released memory is unmapped, so this only bites the
    /// arena path, which is exactly why "arenas off" was the one clean arm.)
    pub guarded: bool,
    /// Next segment in the owning heap's list (reused as the global
    /// abandoned-list link while abandoned).
    pub next: *mut Segment,
    /// Head of the free-span list (slots inside `pages`).
    pub free_spans: *mut Page,
    /// One metadata slot per slice; a span's slot is its first slice.
    pub pages: [Page; SLICES_PER_SEGMENT],
}

const _: () = assert!(
    core::mem::size_of::<Segment>() <= SEGMENT_SLICE_SIZE,
    "segment header must fit in slice 0"
);

/// Slices reserved for the header.
pub const HEADER_SLICES: usize = 1;

/// Usable slices in a Normal segment.
pub const USABLE_SLICES: usize = SLICES_PER_SEGMENT - HEADER_SLICES;

/// Recover the owning segment from any pointer into it. `with_addr` keeps
/// provenance so the mask trick is miri-clean.
#[inline]
pub fn segment_of(p: *mut u8) -> *mut Segment {
    p.with_addr(p.addr() & !(SEGMENT_SIZE - 1)).cast()
}

/// Resolve a block pointer to its page slot (follows `slice_offset` back to
/// the span start).
///
/// # Safety
/// `p` must point into a live segment owned by this allocator.
#[inline]
pub unsafe fn page_of(seg: *mut Segment, p: *mut u8) -> *mut Page {
    let idx = (p.addr() - seg.addr()) / SEGMENT_SLICE_SIZE;
    debug_assert!(
        idx < SLICES_PER_SEGMENT,
        "page_of: slice index out of range"
    );
    // SAFETY: `p` lies inside this 32 MiB segment (caller contract), so
    // `idx < SLICES_PER_SEGMENT` BY CONSTRUCTION — the offset cannot exceed
    // SEGMENT_SIZE and the divisor is SEGMENT_SLICE_SIZE. Indexing the array
    // with `[idx]` instead makes LLVM emit a bounds check it cannot discharge
    // (the bound is a property of the caller's contract, not of the
    // arithmetic), and this is the hottest function in the allocator: two
    // resolutions per free. `add` on the base pointer keeps the same
    // provenance and the same address, with the check kept under
    // `debug_checks`.
    unsafe {
        let base: *mut Page = (&raw mut (*seg).pages).cast();
        let slot = base.add(idx);
        // `slice_offset` is already in BYTES, so the follow-back is a single
        // subtraction — no scaling by size_of::<Page>() on this path.
        let off = (*slot).slice_offset as usize;
        debug_assert!(
            off <= idx * core::mem::size_of::<Page>(),
            "page_of: slice_offset points before the segment"
        );
        slot.cast::<u8>().sub(off).cast::<Page>()
    }
}

/// Byte distance between adjacent slice slots — the unit of
/// [`Page::slice_offset`].
#[inline]
pub const fn slot_stride() -> usize {
    core::mem::size_of::<Page>()
}

// A span can start at most SLICES_PER_SEGMENT-1 slots before an interior slot,
// so the byte offset must fit the u16 it is stored in.
const _: () = assert!(
    (SLICES_PER_SEGMENT - 1) * core::mem::size_of::<Page>() <= u16::MAX as usize,
    "slice_offset (bytes) must fit in u16"
);

/// Payload start of the page whose slot index is `idx`.
///
/// # Safety
/// `idx` must be a span-start slice of a live segment.
#[inline]
pub unsafe fn page_area(seg: *mut Segment, idx: usize) -> *mut u8 {
    // SAFETY: stays within the segment reservation by the idx contract.
    unsafe { seg.cast::<u8>().add(idx * SEGMENT_SLICE_SIZE) }
}

/// Slot index of a page within its segment.
///
/// # Safety
/// `page` must be a slot inside `seg`'s table.
#[inline]
pub unsafe fn page_index(seg: *mut Segment, page: *mut Page) -> usize {
    // SAFETY: both point into the same header per the contract.
    let base: *mut Page = unsafe { (&raw mut (*seg).pages).cast() };
    (page.addr() - base.addr()) / core::mem::size_of::<Page>()
}

/// Spin until no page of `seg` has a remote free IN FLIGHT.
///
/// **This closes a use-after-free.** `remote_free` claims `XFLAG_FREEING`
/// BEFORE pushing a block onto the owner's delayed list and holds it until it
/// restores `XFLAG_DELAYED` afterwards. The owner, draining that very push,
/// can free the block, retire the page, empty the segment and release it to an
/// arena — where the next tenant `memset`s the whole header, including the
/// `xthread_free` atomic the remote is about to CAS. Miri caught exactly that:
/// an atomic store in `remote_free` racing `huge_alloc`'s header scrub.
///
/// The window is bounded, which is why a barrier suffices rather than an epoch
/// scheme: before the remote sets FREEING it has not pushed yet, so the owner
/// cannot have drained it, so `used > 0` and no retire is possible. Every
/// dangerous instant therefore has FREEING observably set.
///
/// Cost is a 512-slot scan, paid only when a segment is actually released.
///
/// # Safety
/// `seg` must be a live segment header.
unsafe fn wait_no_remote_in_flight(seg: *mut Segment) {
    use crate::page::{XFLAG_FREEING, XMASK};
    // SAFETY: the page table is inside the live header; atomics only.
    unsafe {
        let base: *mut Page = (&raw mut (*seg).pages).cast();
        for i in 0..SLICES_PER_SEGMENT {
            let pg = base.add(i);
            while (*pg)
                .xthread_free
                .load(core::sync::atomic::Ordering::Acquire)
                & XMASK
                == XFLAG_FREEING
            {
                core::hint::spin_loop();
            }
        }
    }
}

/// Reserve a Normal segment — from an arena when one qualifies (respecting a
/// heap's `arena_id` restriction), else eagerly-committed fresh OS memory —
/// and register it in the segment map.
pub fn segment_alloc(arena_id: i32) -> Result<*mut Segment, PrimError> {
    let (ptr_, size, mem_zero) = match crate::arena::chunk_alloc(arena_id) {
        Some((p, zero)) => (p, SEGMENT_SIZE, zero),
        None => {
            if arena_id >= 0 {
                return Err(0); // exclusive-arena heap and its arena is full
            }
            let b = os::alloc_aligned(SEGMENT_SIZE, SEGMENT_SIZE, true, false)?;
            (b.ptr, b.size, b.is_zero)
        }
    };
    let seg: *mut Segment = ptr_.cast();
    // SAFETY: 32 MiB mapping (fresh or recycled arena chunk); header written
    // in full. A recycled chunk's stale bytes are all overwritten here and
    // page slots are re-initialized by span_mark on every carve.
    unsafe {
        // Recycled chunks carry stale page slots — scrub the header region.
        if !mem_zero {
            core::ptr::write_bytes(seg.cast::<u8>(), 0, core::mem::size_of::<Segment>());
        }
        (*seg).kind = SegmentKind::Normal;
        (*seg).total_size = size;
        (*seg).next_free_slice = HEADER_SLICES as u32;
        (*seg).used_pages = 0;
        (*seg).thread_id = AtomicUsize::new(crate::init::thread_id());
        (*seg).mem_is_zero = mem_zero;
        (*seg).purged_any = false;
        (*seg).guarded = false;
        (*seg).next = ptr::null_mut();
        (*seg).free_spans = ptr::null_mut();
    }
    segment_map::register(seg);
    Ok(seg)
}

/// Release an empty Normal segment (caller has unlinked it from the heap):
/// back to its arena when it came from one, else to the OS.
///
/// # Safety
/// `seg` must be a live Normal segment with `used_pages == 0` and no live
/// blocks or references into it.
pub unsafe fn segment_free(seg: *mut Segment) -> Result<(), PrimError> {
    // SAFETY: seg is live per the contract; this only reads atomics.
    unsafe { wait_no_remote_in_flight(seg) };
    segment_map::unregister(seg);
    // SAFETY: seg is live and empty per the contract.
    unsafe {
        if (*seg).purged_any || (*seg).guarded {
            // Restore full commitment AND accessibility before the memory can
            // be re-tenanted from an arena (see purged_any / guarded).
            let base = seg.cast::<u8>().add(HEADER_SLICES * SEGMENT_SLICE_SIZE);
            let bytes = (*seg).total_size - HEADER_SLICES * SEGMENT_SLICE_SIZE;
            let _ = os::protect(base, bytes, false);
            let _ = os::commit(base, bytes);
            (*seg).purged_any = false;
            (*seg).guarded = false;
        }
    }
    if crate::arena::chunk_free(seg.cast()) {
        return Ok(());
    }
    // SAFETY: per contract; reconstruct the OsBlock we allocated with.
    unsafe {
        let block = os::OsBlock {
            ptr: seg.cast(),
            size: (*seg).total_size,
            is_large: false,
            is_zero: false,
        };
        os::free(block)
    }
}

/// Write the span markers for a span at `idx` of `len` slices: first slot
/// offset 0, interior+last offsets pointing back.
///
/// # Safety
/// `[idx, idx+len)` must lie in the carved region of `seg` under the heap lock.
unsafe fn span_mark(seg: *mut Segment, idx: usize, len: usize) {
    // SAFETY: caller contract keeps every index in bounds.
    unsafe {
        (*seg).pages[idx].slice_offset = 0;
        (*seg).pages[idx].slice_count = len as u16;
        let mut j = 1;
        while j < len {
            // BYTES back to the span start, not slices — see Page::slice_offset.
            (*seg).pages[idx + j].slice_offset = (j * slot_stride()) as u16;
            j += 1;
        }
    }
}

/// Mark `[idx, idx+len)` as a FREE span and push it on the free list.
///
/// # Safety
/// As [`span_mark`]; the span must not be in the free list already.
unsafe fn span_mark_free(seg: *mut Segment, idx: usize, len: usize) {
    // SAFETY: caller contract.
    unsafe {
        span_mark(seg, idx, len);
        let slot: *mut Page = &raw mut (*seg).pages[idx];
        (*slot).block_size = 0; // free marker
        (*slot).prev = ptr::null_mut();
        (*slot).next = (*seg).free_spans;
        if !(*seg).free_spans.is_null() {
            (*(*seg).free_spans).prev = slot;
        }
        (*seg).free_spans = slot;
    }
}

/// Unlink a free span from the free list.
///
/// # Safety
/// `span` must be a free-span start slot currently linked in `seg`'s list.
unsafe fn span_list_remove(seg: *mut Segment, span: *mut Page) {
    // SAFETY: caller contract.
    unsafe {
        if !(*span).prev.is_null() {
            (*(*span).prev).next = (*span).next;
        } else {
            (*seg).free_spans = (*span).next;
        }
        if !(*span).next.is_null() {
            (*(*span).next).prev = (*span).prev;
        }
        (*span).next = ptr::null_mut();
        (*span).prev = ptr::null_mut();
    }
}

/// `debug_checks`: verify the carved region is exactly TILED by spans, every
/// span start is marked, and the free-span list is consistent. Layout
/// corruption shows up here instead of as a wild pointer later.
///
/// # Safety
/// `seg` must be a live segment the caller is already operating on.
pub unsafe fn debug_validate_segment(seg: *mut Segment, where_: &str) {
    #[cfg(feature = "debug_checks")]
    {
        // SAFETY: caller is operating on this live segment already.
        unsafe {
            let end = (*seg).next_free_slice as usize;
            let mut idx = HEADER_SLICES;
            let mut spans = 0usize;
            while idx < end {
                let slot: *mut Page = &raw mut (*seg).pages[idx];
                assert_eq!(
                    (*slot).slice_offset,
                    0,
                    "{where_}: slice {idx} is not a span start (layout not tiled)"
                );
                let len = (*slot).slice_count as usize;
                assert!(
                    len > 0 && idx + len <= end,
                    "{where_}: slice {idx} has bad slice_count {len} (end {end})"
                );
                // Interior slices must point back to this start.
                for j in 1..len {
                    assert_eq!(
                        (*seg).pages[idx + j].slice_offset as usize,
                        j * slot_stride(),
                        "{where_}: slice {} lost its back-pointer",
                        idx + j
                    );
                }
                spans += 1;
                assert!(
                    spans <= SLICES_PER_SEGMENT,
                    "{where_}: span walk did not terminate"
                );
                idx += len;
            }
            assert_eq!(idx, end, "{where_}: spans do not tile the carved region");
            // Free-span list: every node is a free span start inside the region.
            let mut s = (*seg).free_spans;
            let mut n = 0usize;
            while !s.is_null() {
                let i = page_index(seg, s);
                assert!(
                    i >= HEADER_SLICES && i < end,
                    "{where_}: free span {i} out of region"
                );
                assert_eq!((*s).slice_offset, 0, "{where_}: free span {i} not a start");
                assert_eq!((*s).block_size, 0, "{where_}: free span {i} marked live");
                n += 1;
                assert!(
                    n <= SLICES_PER_SEGMENT,
                    "{where_}: free-span list is cyclic"
                );
                s = (*s).next;
            }
        }
    }
    #[cfg(not(feature = "debug_checks"))]
    {
        let _ = (seg, where_);
    }
}

/// Allocate a span of `slices` from the segment: first-fit over reclaimed
/// spans, else bump. Returns (span-start slot, span-is-fresh-zero) or null.
///
/// # Safety
/// `seg` must be a live Normal segment under the heap lock.
pub unsafe fn span_alloc(seg: *mut Segment, slices: usize) -> (*mut Page, bool) {
    // SAFETY: caller contract — seg is live under the owner.
    unsafe { debug_validate_segment(seg, "span_alloc:enter") };
    // SAFETY: heap lock held; list and indices maintained by the invariants
    // in the module docs.
    unsafe {
        // First fit over reclaimed spans (recycled memory — NOT zero).
        let mut s = (*seg).free_spans;
        while !s.is_null() {
            let len = (*s).slice_count as usize;
            if len >= slices {
                span_list_remove(seg, s);
                let idx = page_index(seg, s);
                // Re-commit BEFORE splitting so both halves are backed; the
                // remainder inherits the (now cleared) purged state.
                span_recommit(seg, idx, len);
                if len > slices {
                    span_mark_free(seg, idx + slices, len - slices);
                }
                span_mark(seg, idx, slices);
                (*seg).used_pages += 1;
                return (s, false);
            }
            s = (*s).next;
        }
        // Bump (never-carved region — zero iff the mapping came fresh from
        // the OS; recycled arena chunks are dirty).
        let idx = (*seg).next_free_slice as usize;
        if idx + slices > SLICES_PER_SEGMENT {
            return (ptr::null_mut(), false);
        }
        (*seg).next_free_slice = (idx + slices) as u32;
        (*seg).used_pages += 1;
        let start: *mut Page = &raw mut (*seg).pages[idx];
        span_mark(seg, idx, slices);
        (start, (*seg).mem_is_zero)
    }
}

/// Return a page's span to the segment, coalescing with free neighbors.
/// Freed spans NEVER rejoin the bump frontier: `[next_free_slice, …)` is the
/// virgin-zero region and bump allocations report `fresh = true` — giving
/// recycled slices back to it would let dirty memory skip zalloc's memset
/// (caught by the spans G1c gate, 2026-08-05).
/// The page must already be unlinked from heap queues.
///
/// Returns whether the freed span was purged back to the OS.
///
/// # Safety
/// `page` must be a live span-start of `seg` with no live blocks, under the
/// heap lock.
pub unsafe fn span_free(seg: *mut Segment, page: *mut Page) -> bool {
    // SAFETY: caller contract — seg is live under the owner.
    unsafe { debug_validate_segment(seg, "span_free:enter") };
    // SAFETY: heap lock held; boundary reads follow the span invariants.
    unsafe {
        let mut idx = page_index(seg, page);
        let mut len = (*page).slice_count as usize;
        (*seg).used_pages -= 1;
        // Scrub page state so a stale slot can't masquerade as live.
        (*page).block_size = 0;
        (*page).free = ptr::null_mut();
        (*page).local_free = ptr::null_mut();
        (*page).used = 0;
        (*page).capacity = 0;
        (*page).reserved = 0;
        (*page).flags = 0;
        (*page).free_is_zero = false;

        // Merge right: the slot at idx+len (if carved) is a span START.
        let right = idx + len;
        if right < (*seg).next_free_slice as usize {
            let rslot: *mut Page = &raw mut (*seg).pages[right];
            debug_assert_eq!((*rslot).slice_offset, 0, "right neighbor not a span start");
            if (*rslot).block_size == 0 {
                span_list_remove(seg, rslot);
                len += (*rslot).slice_count as usize;
            }
        }
        // Merge left: follow the left slot back to its span start.
        if idx > HEADER_SLICES {
            let lslot_idx = idx - 1;
            // slice_offset is in bytes; convert back to a slot index here (cold
            // path — coalescing, not the free fast path).
            let lstart_idx =
                lslot_idx - (*seg).pages[lslot_idx].slice_offset as usize / slot_stride();
            let lstart: *mut Page = &raw mut (*seg).pages[lstart_idx];
            if (*lstart).block_size == 0 {
                span_list_remove(seg, lstart);
                len += idx - lstart_idx;
                idx = lstart_idx;
            }
        }
        span_mark_free(seg, idx, len);
        // PURGE the coalesced free span (the RSS lever): return its pages to
        // the OS. Only worthwhile for multi-slice spans — a syscall costs
        // more than the pages a single 64 KiB slice returns. The span is
        // marked `purged` so reuse re-commits it; skipping that recommit is
        // an access violation on Windows (MEM_DECOMMIT), which is how this
        // was caught (2026-08-05).
        let purge_delay = crate::options::get(15);
        if purge_delay >= 0 && len >= crate::types::MEDIUM_PAGE_SLICES {
            let area = page_area(seg, idx);
            let bytes = len * SEGMENT_SLICE_SIZE;
            let decommits = crate::options::is_enabled(5); // purge_decommits
            if os::purge(area, bytes, decommits).is_ok() {
                (*seg).pages[idx].purged = true;
                (*seg).purged_any = true;
                return true;
            }
        }
        false
    }
}

/// Purge every FREE span of `seg` — used when a dying thread abandons it.
///
/// An abandoned segment is orphaned until some other thread happens to adopt
/// it, and until then it holds every page it ever touched RESIDENT. Measured:
/// 25 orphans accumulated across 8 waves of thread churn and a full 2048-block
/// allocation burst adopted only 4 of them. At 32 MiB each, that is the RSS
/// tail FFAI saw — max 403 MB against mimalloc's 134, with a 4.4x run-to-run
/// spread because whether anything adopts is pure scheduling.
///
/// Deliberately NOT gated on `purge_delay`. That option governs a LIVE heap's
/// own free spans, where the pages are likely to be reused shortly and a
/// syscall would be wasted. An abandoned segment has no owner to reuse them —
/// holding them costs memory for an unbounded time and buys nothing. Upstream
/// draws the same distinction with `abandoned_page_purge`.
///
/// Each purged span is marked so reuse re-commits it (skipping that is an
/// access violation on Windows), exactly as `span_free` does.
///
/// # Safety
/// `seg` must be a live Normal segment whose free-span list is stable — i.e.
/// the caller still owns it, which is true right up to the abandon publish.
pub unsafe fn purge_free_spans(seg: *mut Segment) {
    // SAFETY: caller still owns the segment; free spans have no live blocks.
    unsafe {
        let decommits = crate::options::is_enabled(5); // purge_decommits
        let mut s = (*seg).free_spans;
        while !s.is_null() {
            let next = (*s).next;
            if !(*s).purged {
                let idx = page_index(seg, s);
                let len = (*s).slice_count as usize;
                if len > 0 {
                    let area = page_area(seg, idx);
                    let bytes = len * SEGMENT_SLICE_SIZE;
                    if os::purge(area, bytes, decommits).is_ok() {
                        (*s).purged = true;
                        (*seg).purged_any = true;
                    }
                }
            }
            s = next;
        }
    }
}

/// Re-commit a span that was purged while free (no-op otherwise).
///
/// # Safety
/// `[idx, idx+len)` is a span of `seg` being handed to a caller.
unsafe fn span_recommit(seg: *mut Segment, idx: usize, len: usize) {
    // SAFETY: caller contract; range lies inside the segment reservation.
    unsafe {
        if !(*seg).pages[idx].purged {
            return;
        }
        (*seg).pages[idx].purged = false;
        let area = page_area(seg, idx);
        let _ = os::commit(area, len * SEGMENT_SLICE_SIZE);
    }
}

/// Allocate a dedicated huge segment for one block of `size` bytes, placed so
/// `(block + offset) % align == 0` (align ≤ SEGMENT_SIZE/2). Returns
/// (segment, block ptr).
pub fn huge_alloc(
    size: usize,
    align: usize,
    offset: usize,
) -> Result<(*mut Segment, *mut u8), PrimError> {
    debug_assert!(align.is_power_of_two() && align <= SEGMENT_SIZE / 2);
    let header = SEGMENT_SLICE_SIZE;
    // Worst-case room for placing the block within the reservation. The area
    // is 64 KiB-aligned, so only larger alignments — or offsets that shift
    // the placement off the natural boundary — need slack.
    let extra = if align > SEGMENT_SLICE_SIZE || !offset.is_multiple_of(align) {
        align
    } else {
        0
    };
    let want = os::page_align_up(header + size + extra);
    // Huge blocks recycle through arenas too (contiguous chunks) — without
    // this, every huge alloc/free cycle is an OS round-trip (the Tier-A
    // malloc-large gate measured 3–4× slower before this path).
    let chunks = want.div_ceil(SEGMENT_SIZE);
    let (bptr, total, mem_zero) = match crate::arena::chunk_alloc_n(-1, chunks) {
        Some((p, zero)) => (p, chunks * SEGMENT_SIZE, zero),
        None => {
            let b = os::alloc_aligned(want, SEGMENT_SIZE, true, false)?;
            (b.ptr, b.size, b.is_zero)
        }
    };
    let seg: *mut Segment = bptr.cast();
    // SAFETY: header region fully written below; recycled chunks scrubbed.
    unsafe {
        if !mem_zero {
            core::ptr::write_bytes(seg.cast::<u8>(), 0, core::mem::size_of::<Segment>());
        }
    }
    let b = os::OsBlock {
        ptr: bptr,
        size: total,
        is_large: false,
        is_zero: mem_zero,
    };
    segment_map::register_range(seg.addr(), b.size);
    // SAFETY: fresh zeroed reservation ≥ header + size + extra.
    unsafe {
        (*seg).kind = SegmentKind::Huge;
        (*seg).total_size = b.size;
        (*seg).next_free_slice = SLICES_PER_SEGMENT as u32;
        (*seg).used_pages = 1;
        (*seg).thread_id = AtomicUsize::new(crate::init::thread_id());
        (*seg).mem_is_zero = b.is_zero;
        (*seg).purged_any = false;
        (*seg).guarded = false;
        (*seg).next = ptr::null_mut();
        (*seg).free_spans = ptr::null_mut();
        let area = b.ptr.add(header);
        // (block + offset) aligned: round (area + offset) up, subtract offset.
        let block = area.with_addr(((area.addr() + offset + align - 1) & !(align - 1)) - offset);
        debug_assert!(block.addr() >= area.addr() && (block.addr() + offset).is_multiple_of(align));
        // The single page lives in slot 1; every reachable interior slice
        // offsets back to it (only slices 1..512 are addressable via the mask
        // trick, and aligned offsets stay < SEGMENT_SIZE/2 by the contract).
        let page: *mut Page = &raw mut (*seg).pages[1];
        (*page).block_size = b.size - (block.addr() - seg.addr());
        (*page).used = 1;
        (*page).capacity = 1;
        (*page).reserved = 1;
        (*page).slice_count = (SLICES_PER_SEGMENT - 1) as u16;
        (*page).slice_offset = 0;
        (*page).flags = crate::page::pflags::HUGE_SEGMENT | crate::page::pflags::SINGLE_BLOCK;
        (*page).free_is_zero = b.is_zero;
        let mut j = 2;
        while j < SLICES_PER_SEGMENT {
            // Bytes back to slot 1, which holds this huge block's page data.
            (*seg).pages[j].slice_offset = ((j - 1) * slot_stride()) as u16;
            j += 1;
        }
        Ok((seg, block))
    }
}

/// Free a huge segment (the whole reservation).
///
/// # Safety
/// `seg` must be a live Huge segment with no live references into it.
pub unsafe fn huge_free(seg: *mut Segment) -> Result<(), PrimError> {
    // SAFETY: per contract; reconstruct the OsBlock we allocated with.
    unsafe {
        // SAME BARRIER AS `segment_free`, and this is the path that actually
        // raced: a huge segment recycles through `chunk_free_n` WITHOUT going
        // through `segment_free`, so guarding only there left the hole open.
        // Every route by which memory can reach an arena needs it.
        wait_no_remote_in_flight(seg);
        segment_map::unregister_range(seg.addr(), (*seg).total_size);
        // Arena-backed huge blocks recycle their contiguous chunks — but the
        // memory must be handed back in a USABLE state: lift any guard-page
        // protection and restore commitment first. Skipping this recycles an
        // inaccessible page into the next tenant (the M8 P0).
        if (*seg).total_size.is_multiple_of(SEGMENT_SIZE) {
            if (*seg).guarded || (*seg).purged_any {
                let base = seg.cast::<u8>().add(HEADER_SLICES * SEGMENT_SLICE_SIZE);
                let bytes = (*seg).total_size - HEADER_SLICES * SEGMENT_SLICE_SIZE;
                let _ = os::protect(base, bytes, false);
                let _ = os::commit(base, bytes);
                (*seg).guarded = false;
                (*seg).purged_any = false;
            }
            if crate::arena::chunk_free_n(seg.cast(), (*seg).total_size / SEGMENT_SIZE) {
                return Ok(());
            }
        }
        let block = os::OsBlock {
            ptr: seg.cast(),
            size: (*seg).total_size,
            is_large: false,
            is_zero: false,
        };
        os::free(block)
    }
}
