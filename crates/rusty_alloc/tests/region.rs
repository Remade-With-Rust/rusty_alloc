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
//! profile only: at the shipped 32 MiB geometry the type's alignment is
//! 32 MiB and its smallest instance is 32 MiB + 4 KiB, which is not a value a
//! test should put on a stack or through the system allocator.
#![cfg(ra_small_profile)]

use rusty_alloc::prim::fixed::{
    FERR_REGISTERED, MIN_REGION, Region, good_region_size, usable_bytes,
};

/// The report's budget: 220 KiB, which a round declaration strands 28,672
/// bytes of and `good_region_size` trims to three whole segments.
const N: usize = good_region_size(220 * 1024);

#[test]
fn a_region_given_once_serves_exactly_what_its_size_says() {
    assert_eq!(N, 196_608);
    assert_eq!(Region::<N>::USABLE, 196_608);
    assert_eq!(
        core::mem::size_of::<Region<N>>(),
        N,
        "no padding: whole segments, aligned"
    );

    // A leaked box, allocated in place, not a `static`: rustc 1.97.1 on MSVC
    // cannot compile a half-megabyte static at 64 KiB alignment, and
    // `Box::new(Region::new())` puts a 64 KiB-aligned value on the stack first,
    // which on Windows faults past the guard page. A firmware's `.bss` is not
    // what is under test here — the handoff is. Zero is a valid `Region`.
    // SAFETY: an all-zero `Region` is a valid value (bytes and nothing else).
    let heap: &'static Region<N> =
        Box::leak(unsafe { Box::<Region<N>>::new_zeroed().assume_init() });

    // Aligned by construction, so the real-base answer IS the aligned answer.
    assert_eq!(heap.usable(), Region::<N>::USABLE);
    assert_eq!(heap.usable(), N, "nothing stranded, nothing reserved");
    assert_eq!(heap.usable(), usable_bytes(0, N));

    // The handoff returns what the allocator can serve: three whole segments.
    let usable = heap.give().expect("first give of an aligned, exact region");
    assert_eq!(usable, 196_608);
    assert_eq!(usable, Region::<N>::USABLE);

    // Given once: a second call is refused without touching the bytes.
    assert_eq!(heap.give(), Err(FERR_REGISTERED));

    // And so is any other region, aligned or not: one heap per program.
    // SAFETY: as above.
    let other: &'static Region<MIN_REGION> =
        Box::leak(unsafe { Box::<Region<MIN_REGION>>::new_zeroed().assume_init() });
    assert_eq!(other.give(), Err(FERR_REGISTERED));
}
