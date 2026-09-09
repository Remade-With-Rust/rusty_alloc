//! `prim::fixed::Region`, given once, in its own process.
//!
//! The fixed backend registers exactly one region per process, and the unit
//! tests in `prim::fixed` already own that registration. This file is a
//! separate test binary, so `give` can succeed here and the outcome — the
//! usable bytes it returns, the refusal of a second call, the refusal of a
//! second region — is asserted rather than order-dependent.
//!
//! On the host the fixed backend is inert (the OS backend serves the
//! allocator), so registering a region here changes nothing else. Small
//! profile only: at the shipped 32 MiB geometry the type's smallest instance
//! is 32 MiB, which is not a value a test should put in its image.
#![cfg(ra_small_profile)]

use rusty_alloc::prim::fixed::{
    FERR_REGISTERED, MIN_REGION, REGION_ALIGN, Region, good_region_size, usable_bytes,
};

/// The report's budget: 220 KiB, which a round declaration strands 28,672
/// bytes of and `good_region_size` trims to three whole segments.
const N: usize = good_region_size(220 * 1024);

/// A plain `static`, exactly as a firmware declares it: 16-byte aligned, so
/// the linker owes it no gap, and `size_of` is the heap it serves.
#[cfg(not(ra_aligned_region))]
static HEAP: Region<N> = Region::new();
#[cfg(not(ra_aligned_region))]
static OTHER: Region<MIN_REGION> = Region::new();

#[test]
fn a_region_given_once_serves_exactly_what_its_size_says() {
    assert_eq!(N, 196_608);
    assert_eq!(Region::<N>::USABLE, 196_608);
    assert_eq!(
        core::mem::size_of::<Region<N>>(),
        N,
        "no padding: whole segments"
    );
    assert_eq!(
        core::mem::align_of::<Region<N>>(),
        REGION_ALIGN,
        "16 bytes, not a segment: segments stride from the base (a segment under ra_aligned_region)"
    );

    #[cfg(not(ra_aligned_region))]
    let (heap, other): (&'static Region<N>, &'static Region<MIN_REGION>) = (&HEAP, &OTHER);
    // Under `ra_aligned_region` the type is segment-aligned again: rustc
    // 1.97.1 on MSVC cannot compile a half-megabyte static at 64 KiB
    // alignment, and `Box::new(Region::new())` puts a 64 KiB-aligned value on
    // the stack first, which on Windows faults past the guard page. Allocate
    // in place instead. Zero is a valid `Region`.
    // SAFETY: an all-zero `Region` is a valid value (bytes and nothing else).
    #[cfg(ra_aligned_region)]
    let (heap, other): (&'static Region<N>, &'static Region<MIN_REGION>) = unsafe {
        (
            Box::leak(Box::<Region<N>>::new_zeroed().assume_init()),
            Box::leak(Box::<Region<MIN_REGION>>::new_zeroed().assume_init()),
        )
    };

    // On the grid by construction, so the real-base answer IS the aligned
    // answer, wherever the linker put it -- and this test does not get to
    // choose where that is, which is the point.
    let base = core::ptr::from_ref(heap).addr();
    assert_eq!(base % REGION_ALIGN, 0);
    assert_eq!(heap.usable(), Region::<N>::USABLE);
    assert_eq!(heap.usable(), N, "nothing stranded, nothing reserved");
    assert_eq!(heap.usable(), usable_bytes(0, N));
    assert_eq!(usable_bytes(base, N), N, "the real base serves every byte");

    // The handoff returns what the allocator can serve: three whole segments.
    let usable = heap.give().expect("first give of an exact region");
    assert_eq!(usable, 196_608);
    assert_eq!(usable, Region::<N>::USABLE);

    // Given once: a second call is refused without touching the bytes.
    assert_eq!(heap.give(), Err(FERR_REGISTERED));

    // And so is any other region: one heap per program.
    assert_eq!(other.give(), Err(FERR_REGISTERED));
}
