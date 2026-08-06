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
pub const SEGMENT_SLICE_SIZE: usize = 64 * 1024;

/// Slices per segment (`MI_SLICES_PER_SEGMENT` = 512).
pub const SLICES_PER_SEGMENT: usize = 512;

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
pub const MEDIUM_PAGE_SLICES: usize = 8;

/// Medium page size in bytes.
pub const MEDIUM_PAGE_SIZE: usize = MEDIUM_PAGE_SLICES * SEGMENT_SLICE_SIZE;

/// Largest object served from a small page (`MI_SMALL_OBJ_SIZE_MAX` = 8 KiB).
pub const SMALL_OBJ_SIZE_MAX: usize = SMALL_PAGE_SIZE / 8;

/// Largest binned object (`MI_MEDIUM_OBJ_SIZE_MAX` = 64 KiB) — G2-verified:
/// the oracle's `mi_good_size` switches to page-rounding above this.
pub const MEDIUM_OBJ_SIZE_MAX: usize = MEDIUM_PAGE_SIZE / 8;

/// Largest object served from an in-segment large page
/// (`MI_LARGE_OBJ_SIZE_MAX` = SEGMENT_SIZE/2 = 16 MiB). Above this a dedicated
/// huge segment is used.
pub const LARGE_OBJ_SIZE_MAX: usize = SEGMENT_SIZE / 2;

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
        assert_eq!(SEGMENT_SIZE, 32 * 1024 * 1024);
        assert_eq!(BIN_FULL, 74);
    }
}
