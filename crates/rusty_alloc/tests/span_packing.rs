//! F1 of docs/plans/segment-tax.md: sizes in (16 MiB, 31.94 MiB] are
//! exact-slice spans in SHARED segments, not dedicated 32 MiB huge
//! reservations. The observable is the segment base address: allocations
//! whose slice counts sum within `USABLE_SLICES` must come from ONE segment.
//!
//! Before the routing fix, the 20 MiB allocation below took a dedicated huge
//! segment (32 MiB for 20 — the report's 60 % row) and the smaller blocks
//! lived elsewhere: two reservations minimum. These tests are the structural
//! half of the gate; the byte-accounting half is the wasm waste probe
//! (`ra_hold` / `ra_hold_mix` in `rusty_alloc-wasm`, driven by
//! `bench/wasm-selftest.mjs`), which measures the same geometry as linear
//! memory because on wasm a reservation is real, permanent memory.

use rusty_alloc::alloc::{free, malloc, usable_size};
use rusty_alloc::types::SEGMENT_SIZE;

const MIB: usize = 1024 * 1024;

fn segment_base(p: *mut u8) -> usize {
    p as usize & !(SEGMENT_SIZE - 1)
}

/// Allocate, verify usability, and prove co-tenancy in one segment.
fn assert_share_one_segment(sizes: &[usize]) {
    let blocks: Vec<*mut u8> = sizes
        .iter()
        .map(|&n| {
            let p = malloc(n);
            assert!(!p.is_null(), "malloc({n}) failed");
            // SAFETY: fresh block of at least n bytes from this allocator.
            unsafe {
                assert!(usable_size(p) >= n, "usable_size below request for {n}");
                // Touch both ends so the span is really backed.
                *p = 0xAB;
                *p.add(n - 1) = 0xCD;
            }
            p
        })
        .collect();
    let bases: Vec<usize> = blocks.iter().map(|&p| segment_base(p)).collect();
    for (i, &b) in bases.iter().enumerate() {
        assert_eq!(
            b, bases[0],
            "block {i} ({} bytes) landed in a different segment — span routing \
             regressed to dedicated reservations (the segment tax)",
            sizes[i]
        );
    }
    for (&p, &n) in blocks.iter().zip(sizes) {
        // SAFETY: allocated above, ends still hold the pattern, freed once.
        unsafe {
            assert_eq!(*p, 0xAB);
            assert_eq!(*p.add(n - 1), 0xCD);
            free(p);
        }
    }
}

/// The report's 60 % row: 20 MiB (320 slices) now shares its segment —
/// 8 MiB (128 slices) and ~3.9 MiB (62 slices) fit in the tail (510 ≤ 511).
#[test]
fn twenty_mib_span_shares_its_segment() {
    assert_share_one_segment(&[20 * MIB, 8 * MIB, 62 * 64 * 1024]);
}

/// The report's 27 % row: a 25.1 MiB detector tensor (402 slices) leaves a
/// 109-slice tail that a 6 MiB block (96 slices) fits inside.
#[test]
fn detector_tensor_span_shares_its_segment() {
    assert_share_one_segment(&[402 * 64 * 1024, 6 * MIB]);
}

/// Face B of the report, honestly stated: a 16 MiB-exact block can never
/// pair with ITSELF (2 x 256 slices > 511 usable), but its 255-slice tail is
/// live real estate — a 15 MiB block (240 slices) shares the segment. This
/// held before the routing change too; the test pins it against regression.
#[test]
fn sixteen_mib_tail_is_usable() {
    assert_share_one_segment(&[16 * MIB, 15 * MIB]);
}

/// The new routing boundary itself: the largest span a segment can carve is
/// USABLE_SLICES = 511 slices = 32 MiB - 64 KiB, and it must be served
/// in-segment (payload begins one header slice past the segment base).
#[test]
fn maximum_span_fills_one_segment_exactly() {
    let n = rusty_alloc::types::LARGE_OBJ_SIZE_MAX;
    let p = malloc(n);
    assert!(!p.is_null(), "malloc(LARGE_OBJ_SIZE_MAX) failed");
    // SAFETY: fresh block of n bytes.
    unsafe {
        assert!(usable_size(p) >= n);
        *p = 0x5A;
        *p.add(n - 1) = 0xA5;
        assert_eq!(
            p as usize - segment_base(p),
            64 * 1024,
            "maximum span did not start at the first usable slice"
        );
        assert_eq!(*p, 0x5A);
        assert_eq!(*p.add(n - 1), 0xA5);
        free(p);
    }
}

/// One past the boundary must still work (dedicated huge segment) — the
/// routing cliff exists, but nothing falls off it.
#[test]
fn one_past_the_boundary_is_huge_and_correct() {
    let n = rusty_alloc::types::LARGE_OBJ_SIZE_MAX + 1;
    let p = malloc(n);
    assert!(!p.is_null(), "malloc(LARGE_OBJ_SIZE_MAX + 1) failed");
    // SAFETY: fresh block of n bytes.
    unsafe {
        assert!(usable_size(p) >= n);
        *p = 0x11;
        *p.add(n - 1) = 0x22;
        assert_eq!(*p, 0x11);
        assert_eq!(*p.add(n - 1), 0x22);
        free(p);
    }
}
