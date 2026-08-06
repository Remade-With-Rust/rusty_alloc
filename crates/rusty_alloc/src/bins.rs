//! Size-class (bin) geometry, mirroring upstream `page-queue.c` `mi_bin`.
//!
//! The geometry is the ABI-visible contract (`mi_good_size` G2-pins it against
//! the oracle): words 1..=8 get exact byte-multiples-of-8 bins; above that,
//! four bins per power of two (25% worst-case / 12.5% mean internal
//! fragmentation — consecutive bins step by at most 5/4).
//! Bin NUMBERING is internal and need not match upstream — only the
//! size→good_size mapping must (verified by the differential gate).

use crate::types::{BIN_FULL, BIN_HUGE, INTPTR_SIZE, MEDIUM_OBJ_SIZE_MAX, wsize_from_size};

/// Total number of page queues (bins 0..=BIN_HUGE plus the full queue).
pub const BIN_COUNT: usize = BIN_FULL + 1;

/// Direct-table entries: one per small wsize 0..=128 (`MI_PAGES_DIRECT`).
pub const PAGES_DIRECT: usize = crate::types::SMALL_WSIZE_MAX + 1;

/// Map a size to its bin index (`mi_bin`). Sizes above [`MEDIUM_OBJ_SIZE_MAX`]
/// map to [`BIN_HUGE`], the dedicated-segment path.
#[inline]
pub fn bin(size: usize) -> usize {
    let wsize = wsize_from_size(size);
    if wsize <= 1 {
        1
    } else if wsize <= 8 {
        // MI_ALIGN2W (the 64-bit default, G2-verified): round to even word
        // counts so every block ≥ 16 bytes is 16-aligned (max_align_t). Bins
        // 3/5/7 (24/40/56 B) do not exist.
        (wsize + 1) & !1
    } else if size > MEDIUM_OBJ_SIZE_MAX {
        BIN_HUGE
    } else {
        // Four bins per power of two: index by the top bit and the next two.
        let w = wsize - 1;
        let b = (usize::BITS - 1 - w.leading_zeros()) as usize; // bsr(w)
        ((b << 2) + ((w >> (b - 2)) & 0x03)) - 3
    }
}

/// Block size (bytes) served by `bin` (`_mi_bin_size` inverse of [`bin`]).
/// Meaningless for [`BIN_HUGE`]/[`BIN_FULL`].
#[inline]
pub const fn bin_size(bin: usize) -> usize {
    if bin <= 8 {
        bin * INTPTR_SIZE
    } else {
        // bin = (b<<2) + m - 3  with block wsize = (5+m) << (b-2)
        let t = bin + 3;
        let b = t >> 2;
        let m = t & 3;
        ((5 + m) << (b - 2)) * INTPTR_SIZE
    }
}

/// `mi_good_size`: the size actually allocated for a request — the bin's block
/// size for binned sizes, page-rounded for huge ones.
#[inline]
pub fn good_size(size: usize) -> usize {
    if size <= MEDIUM_OBJ_SIZE_MAX {
        bin_size(bin(size))
    } else {
        crate::os::page_align_up(size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bin_size_inverts_bin() {
        // For every bin reachable from a size, bin_size must be the largest
        // size mapping to that bin, and good_size must be idempotent.
        for size in 1..=MEDIUM_OBJ_SIZE_MAX {
            let b = bin(size);
            let bs = bin_size(b);
            assert!(bs >= size, "bin_size({b}) = {bs} < size {size}");
            assert_eq!(bin(bs), b, "bin_size({b}) = {bs} maps to bin {}", bin(bs));
            assert_eq!(good_size(good_size(size)), good_size(size));
        }
    }

    #[test]
    fn known_size_classes() {
        // Spot values from the mimalloc paper / types.h (G2 pins the full range
        // against the oracle binary; these catch formula regressions offline).
        for (size, good) in [
            (1, 8),
            (8, 8),
            (9, 16),
            (17, 32), // ALIGN2W: no 24-byte bin
            (24, 32),
            (33, 48), // no 40-byte bin
            (56, 64), // no 56-byte bin
            (64, 64),
            (65, 80),
            (72, 80),
            (80, 80),
            (100, 112),
            (128, 128),
            (129, 160),
            (256, 256),
            (257, 320),
            (1024, 1024),
            (1025, 1280),
            (4097, 5120),
            (65536, 65536), // last binned size
            (65537, 69632), // page-rounded above the binned range
            (131072, 131072),
        ] {
            assert_eq!(good_size(size), good, "good_size({size})");
        }
    }

    #[test]
    fn fragmentation_bound() {
        // 4 linear bins per doubling: consecutive bin sizes step by ≤ 5/4, so
        // worst-case waste is 25% (hit just above each bin boundary).
        for size in 65..=MEDIUM_OBJ_SIZE_MAX {
            let g = good_size(size);
            assert!(g - size <= size / 4 + 16, "waste {}-{size} too large", g);
        }
    }
}
