//! Global segment map (mirrors upstream `segment-map.c`): one bit per 32 MiB
//! address window, answering "does this pointer lie in memory we own?" —
//! `mi_is_in_heap_region`, and the `debug_checks` foreign-free guard.
//!
//! 48-bit user VA / 32 MiB windows → 2²³ bits = 1 MiB of BSS (zero-init,
//! touched sparsely). Addresses above 2⁴⁸ (LA57) are conservatively reported
//! as NOT ours — a false negative for `is_in_heap_region`, never a false
//! positive.

// The window bitmap is the native representation; wasm replaces it wholesale
// with the slice-granular base table below, so the bitmap items are compiled
// out there rather than left as dead weight.
#[cfg(not(all(target_arch = "wasm32", not(miri))))]
use core::sync::atomic::{AtomicU64, Ordering};

use crate::segment::Segment;
use crate::types::SEGMENT_SIZE;

#[cfg(not(all(target_arch = "wasm32", not(miri))))]
const ADDR_BITS: usize = 48;
const WINDOW_SHIFT: usize = 25; // log2(SEGMENT_SIZE)
#[cfg(not(all(target_arch = "wasm32", not(miri))))]
const MAP_BITS: usize = 1 << (ADDR_BITS - WINDOW_SHIFT);
#[cfg(not(all(target_arch = "wasm32", not(miri))))]
const MAP_WORDS: usize = MAP_BITS / 64;

const _: () = assert!(1 << WINDOW_SHIFT == SEGMENT_SIZE);

#[cfg(not(all(target_arch = "wasm32", not(miri))))]
static MAP: [AtomicU64; MAP_WORDS] = [const { AtomicU64::new(0) }; MAP_WORDS];

/// wasm: a slice-granular BASE table instead of the window bitmap.
///
/// On wasm, segments are 64 KiB-slice-aligned rather than
/// SEGMENT_SIZE-aligned (F2, docs/plans/segment-tax.md): requiring 32 MiB
/// bases on a platform whose memory can never be returned made every ragged
/// reservation strand its tail forever — the segment tax. Slice-aligned
/// bases mean `segment_of` can no longer mask; it asks this table instead.
/// One entry per 64 KiB slice of the 4 GiB address space (256 KiB of BSS,
/// wasm-only), holding `(base >> 16) + 1` so zero stays "no segment". Two
/// segments may share a 32 MiB window here, which is also why the window
/// bitmap above is not maintained on wasm — clearing a window on one
/// segment's free would lie about its neighbour.
#[cfg(all(target_arch = "wasm32", not(miri)))]
mod base_table {
    use core::sync::atomic::{AtomicU32, Ordering};

    const SLICE_SHIFT: usize = 16;
    const _: () = assert!(1 << SLICE_SHIFT == crate::types::SEGMENT_SLICE_SIZE);
    const SLOTS: usize = 1 << (32 - SLICE_SHIFT);
    static BASE: [AtomicU32; SLOTS] = [const { AtomicU32::new(0) }; SLOTS];

    fn slots(base: usize, size: usize) -> core::ops::Range<usize> {
        let start = base >> SLICE_SHIFT;
        let end = (base + size.max(1)).div_ceil(1 << SLICE_SHIFT).min(SLOTS);
        start.min(SLOTS)..end
    }

    pub(super) fn set(base: usize, size: usize) {
        let entry = ((base >> SLICE_SHIFT) + 1) as u32;
        for i in slots(base, size) {
            BASE[i].store(entry, Ordering::Relaxed);
        }
    }

    pub(super) fn clear(base: usize, size: usize) {
        for i in slots(base, size) {
            BASE[i].store(0, Ordering::Relaxed);
        }
    }

    pub(super) fn get(addr: usize) -> usize {
        let i = addr >> SLICE_SHIFT;
        if i >= SLOTS {
            return 0;
        }
        match BASE[i].load(Ordering::Relaxed) {
            0 => 0,
            e => ((e - 1) as usize) << SLICE_SHIFT,
        }
    }
}

/// The segment base covering `addr`, or 0. wasm-only: this is what
/// `segment_of` resolves through instead of the pointer mask.
#[cfg(all(target_arch = "wasm32", not(miri)))]
#[inline]
pub fn base_of(addr: usize) -> usize {
    base_table::get(addr)
}

#[cfg(not(all(target_arch = "wasm32", not(miri))))]
#[inline]
fn locate(addr: usize) -> Option<(usize, u64)> {
    let idx = addr >> WINDOW_SHIFT;
    if idx >= MAP_BITS {
        return None;
    }
    Some((idx / 64, 1u64 << (idx % 64)))
}

/// Register a segment's windows (Normal: one; Huge: every window the
/// reservation spans).
pub fn register(seg: *mut Segment) {
    register_range(seg.addr(), SEGMENT_SIZE);
}

/// Register every 32 MiB window overlapped by `[base, base+size)`
/// (on wasm: every 64 KiB slice, in the base table).
pub fn register_range(base: usize, size: usize) {
    #[cfg(all(target_arch = "wasm32", not(miri)))]
    {
        base_table::set(base, size);
    }
    #[cfg(not(all(target_arch = "wasm32", not(miri))))]
    {
        let mut a = base;
        let end = base + size.max(1);
        while a < end {
            if let Some((w, bit)) = locate(a) {
                MAP[w].fetch_or(bit, Ordering::Release);
            }
            a += SEGMENT_SIZE;
        }
    }
}

/// Unregister a segment's single window.
pub fn unregister(seg: *mut Segment) {
    unregister_range(seg.addr(), SEGMENT_SIZE);
}

/// Unregister every window of `[base, base+size)`.
pub fn unregister_range(base: usize, size: usize) {
    #[cfg(all(target_arch = "wasm32", not(miri)))]
    {
        base_table::clear(base, size);
    }
    #[cfg(not(all(target_arch = "wasm32", not(miri))))]
    {
        let mut a = base;
        let end = base + size.max(1);
        while a < end {
            if let Some((w, bit)) = locate(a) {
                MAP[w].fetch_and(!bit, Ordering::Release);
            }
            a += SEGMENT_SIZE;
        }
    }
}

/// Whether `p` lies inside a registered window (`mi_is_in_heap_region`).
/// Best-effort by design: a racing segment release can flip the answer, so
/// this is a diagnostic, not a safety oracle.
pub fn contains(p: *const u8) -> bool {
    #[cfg(all(target_arch = "wasm32", not(miri)))]
    {
        base_table::get(p.addr()) != 0
    }
    #[cfg(not(all(target_arch = "wasm32", not(miri))))]
    {
        match locate(p.addr()) {
            Some((w, bit)) => MAP[w].load(Ordering::Acquire) & bit != 0,
            None => false,
        }
    }
}
