//! Core size constants, mirroring mimalloc v2.4.5 `include/mimalloc/types.h`.
//!
//! Only constants verified against the oracle header live here. Anything still
//! unverified is introduced in the milestone that implements it, alongside the
//! G2 differential test that pins it (plan §4). In particular the full bin
//! geometry (`bin(size)` mapping) lands in M2 pinned by `mi_good_size` equality.

/// Word size in bytes (`MI_INTPTR_SIZE`). We target 64-bit first.
pub const INTPTR_SIZE: usize = core::mem::size_of::<usize>();

/// Maximum "small" allocation in machine words (`MI_SMALL_WSIZE_MAX` = 128).
///
/// Source: `mimalloc.h` v2.4.5 — `#define MI_SMALL_WSIZE_MAX (128)`.
pub const SMALL_WSIZE_MAX: usize = 128;

/// Maximum "small" allocation in bytes (`MI_SMALL_SIZE_MAX` = 1 KiB on 64-bit).
/// `mi_malloc_small` / `mi_zalloc_small` require `size <= SMALL_SIZE_MAX`.
pub const SMALL_SIZE_MAX: usize = SMALL_WSIZE_MAX * INTPTR_SIZE;

/// Segment slice size (`MI_SEGMENT_SLICE_SIZE` = 64 KiB on 64-bit): the
/// granularity v2 segments are carved in. A small page is one slice.
#[cfg(not(ra_small_profile))]
pub const SEGMENT_SLICE_SIZE: usize = 64 * 1024;
/// Segment slice size, small profile: 4 KiB — exactly one [`crate::prim`] page.
///
/// This is the DOMINANT footprint lever, and §2.9 of docs/plans/small-metal.md
/// is why: a page serves exactly one size class and is at minimum one slice, so
/// the floor is `(distinct bins touched) x SEGMENT_SLICE_SIZE` and is
/// independent of how many bytes the workload actually wants. Measured on a
/// XIAO ESP32-S3, a 4,914-byte workload touched 10 bins; at the 8 KiB slice
/// this probe started with, that alone cost 106,496 bytes of pages — 4.6 %
/// occupancy.
///
/// **4 KiB is the floor, and a 2 KiB probe is what proved it.** `bins::good_size`
/// answers the large range with `os::page_align_up`, but the large path
/// allocates EXACT SLICES — so `usable_size >= good_size` holds only while a
/// slice is at least an OS page. At every other geometry that is free (a 64 KiB
/// slice over a 4 KiB page), which is why the assumption was never written
/// down. At a 2 KiB slice it inverts: `good_size(49_153)` promised 53,248 while
/// the 25-slice span delivered 51,200, and `properties::usable_size_agrees_with_good_size`
/// caught it. `good_size` is ABI-visible and G2-pinned against the oracle, so
/// the slice is the side that moves. See the const assert in `prim/fixed.rs`.
#[cfg(ra_small_profile)]
pub const SEGMENT_SLICE_SIZE: usize = 4 * 1024;

/// Slices per segment (`MI_SLICES_PER_SEGMENT` = 512).
#[cfg(not(ra_small_profile))]
pub const SLICES_PER_SEGMENT: usize = 512;
/// Slices per segment, small profile: 16, so a segment is 64 KiB.
///
/// Raised with the slice halving so `SEGMENT_SIZE` does NOT move. Segment size
/// is the wrong lever — it is the granule the region is carved in, and
/// shrinking it alone only trades one segment for two, with two header slices
/// instead of one. What matters is the number of PAGES a segment can hold: 16
/// slices leaves 15 usable, and the measured workload needs 13. Holding the
/// slice COUNT while shrinking the slice is what collapsed this workload from
/// two 64 KiB segments to one 32 KiB one.
#[cfg(ra_small_profile)]
pub const SLICES_PER_SEGMENT: usize = 16;

/// Segment size (`MI_SEGMENT_SIZE` = 32 MiB on 64-bit): the unit of OS/arena
/// allocation, and the shift+mask that takes any block pointer to its segment
/// (which is why segments are segment-size-aligned).
pub const SEGMENT_SIZE: usize = SEGMENT_SLICE_SIZE * SLICES_PER_SEGMENT;

/// Index of the largest size bin (`MI_BIN_HUGE` = 73). Bins `1..=BIN_HUGE`
/// hold size-class page queues; `BIN_FULL` is the queue of full pages.
pub const BIN_HUGE: usize = 73;

/// The full-page queue index (`MI_BIN_FULL` = `BIN_HUGE + 1`).
pub const BIN_FULL: usize = BIN_HUGE + 1;

/// Guaranteed natural alignment of every allocation (`MI_MAX_ALIGN_SIZE` = 16):
/// `mi_malloc(n)` for `n >= 16` returns 16-byte-aligned memory, matching
/// `max_align_t` expectations of C callers.
pub const MAX_ALIGN_SIZE: usize = 16;

/// A small page is one slice (`MI_SMALL_PAGE_SIZE` = 64 KiB).
pub const SMALL_PAGE_SIZE: usize = SEGMENT_SLICE_SIZE;

/// A medium page spans 8 slices (`MI_MEDIUM_PAGE_SIZE` = 512 KiB).
#[cfg(not(ra_small_profile))]
pub const MEDIUM_PAGE_SLICES: usize = 8;
/// A medium page, small profile: 4 slices = 16 KiB.
///
/// Held at 4 rather than lowered with the slice, and `spans.rs` is why. This
/// constant sets `MEDIUM_OBJ_SIZE_MAX = MEDIUM_PAGE_SLICES * SEGMENT_SLICE_SIZE / 8`,
/// which is the TOP of the binned range — above it every allocation gets its
/// own single-block large span. At 2 slices that ceiling falls to 1,024 bytes,
/// so a burst of 2 KiB objects stops being packed into shared pages and takes
/// a whole slice each. `span_lifecycle_and_realloc` caught exactly that. The
/// binned range has to stay wide enough to be worth having; 2 KiB is the floor
/// that keeps it so.
#[cfg(ra_small_profile)]
pub const MEDIUM_PAGE_SLICES: usize = 4;

/// Medium page size in bytes.
pub const MEDIUM_PAGE_SIZE: usize = MEDIUM_PAGE_SLICES * SEGMENT_SLICE_SIZE;

/// Largest object served from a small page (`MI_SMALL_OBJ_SIZE_MAX` = 8 KiB).
pub const SMALL_OBJ_SIZE_MAX: usize = SMALL_PAGE_SIZE / 8;

/// Largest binned object (`MI_MEDIUM_OBJ_SIZE_MAX` = 64 KiB) — G2-verified:
/// the oracle's `mi_good_size` switches to page-rounding above this.
pub const MEDIUM_OBJ_SIZE_MAX: usize = MEDIUM_PAGE_SIZE / 8;

/// Largest object served from an in-segment large span. Above this a
/// dedicated huge segment is used.
///
/// **Deliberate divergence from upstream** (2026-08-28, the segment-tax
/// report — docs/plans/segment-tax.md). Upstream's `MI_LARGE_OBJ_SIZE_MAX`
/// is `SEGMENT_SIZE/2` = 16 MiB, but our `large_alloc` allocates an
/// EXACT-SLICE span, so the true capacity of the in-segment path is
/// everything the carved region can hold: `USABLE_SLICES` = 511 slices =
/// 32 MiB − 64 KiB. Routing (16 MiB, 31.94 MiB] to dedicated huge segments
/// instead charged each such allocation a whole 32 MiB reservation — 60 %
/// waste at 20 MiB, and on wasm (where a reservation is permanent linear
/// memory) that waste was the process's floor. As a span, the same 20 MiB
/// costs 320 slices and leaves 191 slices of the segment usable by other
/// allocations.
///
/// Expressed via `SLICES_PER_SEGMENT - 1` because `segment::HEADER_SLICES`
/// is 1; a const assert in `segment.rs` keeps the two locked together.
pub const LARGE_OBJ_SIZE_MAX: usize = (SLICES_PER_SEGMENT - 1) * SEGMENT_SLICE_SIZE;

/// Size in machine words, rounded up (`_mi_wsize_from_size`).
#[inline]
pub const fn wsize_from_size(size: usize) -> usize {
    size.div_ceil(INTPTR_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_oracle_64bit() {
        // Pinned to mimalloc v2.4.5 on x86_64 / aarch64 (64-bit words).
        assert_eq!(INTPTR_SIZE, 8);
        assert_eq!(SMALL_SIZE_MAX, 1024);
        #[cfg(not(ra_small_profile))]
        assert_eq!(SEGMENT_SIZE, 32 * 1024 * 1024);
        // The small profile's geometry is a DECISION, pinned here so moving it
        // has to be meant. 4 KiB slices x 16 = a 64 KiB segment
        // (docs/plans/small-metal.md §2.10).
        #[cfg(ra_small_profile)]
        assert_eq!(SEGMENT_SIZE, 64 * 1024);
        #[cfg(ra_small_profile)]
        assert_eq!(SEGMENT_SLICE_SIZE, 4 * 1024);
        assert_eq!(BIN_FULL, 74);
    }
}
