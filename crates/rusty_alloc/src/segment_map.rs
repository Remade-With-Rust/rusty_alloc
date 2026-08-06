//! Global segment map (mirrors upstream `segment-map.c`): one bit per 32 MiB
//! address window, answering "does this pointer lie in memory we own?" —
//! `mi_is_in_heap_region`, and the `debug_checks` foreign-free guard.
//!
//! 48-bit user VA / 32 MiB windows → 2²³ bits = 1 MiB of BSS (zero-init,
//! touched sparsely). Addresses above 2⁴⁸ (LA57) are conservatively reported
//! as NOT ours — a false negative for `is_in_heap_region`, never a false
//! positive.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::segment::Segment;
use crate::types::SEGMENT_SIZE;

const ADDR_BITS: usize = 48;
const WINDOW_SHIFT: usize = 25; // log2(SEGMENT_SIZE)
const MAP_BITS: usize = 1 << (ADDR_BITS - WINDOW_SHIFT);
const MAP_WORDS: usize = MAP_BITS / 64;

const _: () = assert!(1 << WINDOW_SHIFT == SEGMENT_SIZE);

static MAP: [AtomicU64; MAP_WORDS] = [const { AtomicU64::new(0) }; MAP_WORDS];

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

/// Register every 32 MiB window overlapped by `[base, base+size)`.
pub fn register_range(base: usize, size: usize) {
    let mut a = base;
    let end = base + size.max(1);
    while a < end {
        if let Some((w, bit)) = locate(a) {
            MAP[w].fetch_or(bit, Ordering::Release);
        }
        a += SEGMENT_SIZE;
    }
}

/// Unregister a segment's single window.
pub fn unregister(seg: *mut Segment) {
    unregister_range(seg.addr(), SEGMENT_SIZE);
}

/// Unregister every window of `[base, base+size)`.
pub fn unregister_range(base: usize, size: usize) {
    let mut a = base;
    let end = base + size.max(1);
    while a < end {
        if let Some((w, bit)) = locate(a) {
            MAP[w].fetch_and(!bit, Ordering::Release);
        }
        a += SEGMENT_SIZE;
    }
}

/// Whether `p` lies inside a registered window (`mi_is_in_heap_region`).
/// Best-effort by design: a racing segment release can flip the answer, so
/// this is a diagnostic, not a safety oracle.
pub fn contains(p: *const u8) -> bool {
    match locate(p.addr()) {
        Some((w, bit)) => MAP[w].load(Ordering::Acquire) & bit != 0,
        None => false,
    }
}
