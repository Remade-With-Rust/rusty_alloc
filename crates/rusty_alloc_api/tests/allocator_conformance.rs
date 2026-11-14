//! Conformance suite for the [`Allocator`] impls (feature `allocator-api`).
//!
//! Every check is written against the trait's contract, generically, and then
//! run on four allocators: [`RustyAlloc`], a [`Heap`], a destroyable `Heap`,
//! and the standard library's `System`. `System` is the control arm. A check
//! that fails on the standard library's own allocator is a wrong check, so the
//! suite cannot pass by asserting something the contract does not say.
//!
//! What only holds for this crate's impls (the dangling zero-sized block, the
//! length returned) is tested separately, at the end.
//!
//! Runs under miri (`cargo +nightly miri test -p rusty_alloc-api --features
//! allocator-api`) with smaller sizes and counts: the allocator is slow to
//! interpret, and what miri adds is a verdict on each branch of the impls, not
//! volume. Every check and all four arms still run there. Measured
//! 2026-11-12: at larger sizes one arm took 14 minutes under miri on a busy
//! machine, and CI's runners interpret several times slower.

#![cfg(feature = "allocator-api")]
// The feature under test is the one part of the crate above its
// `rust-version`: everything here needs Rust 1.100.
#![allow(clippy::incompatible_msrv)]

use core::alloc::{Allocator, Layout};
use core::hint::black_box;
use core::ptr::NonNull;
use rusty_alloc_api::{Heap, RustyAlloc};
use std::alloc::System;

/// Iterations for the randomised checks: full natively, a sample under miri.
const fn scale(native: usize, miri: usize) -> usize {
    if cfg!(miri) { miri } else { native }
}

fn layout(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).expect("test layouts are valid")
}

/// xorshift64*: a fixed, seedable stream, so a failure replays exactly.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The byte a block tagged `seed` holds at offset `i`.
fn pattern(seed: u64, i: usize) -> u8 {
    (seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8 ^ (i as u8).wrapping_mul(31)
}

/// Fill `block[from..to]` with `seed`'s pattern.
///
/// # Safety
/// `p` is a live block at least `to` bytes long.
unsafe fn fill(p: NonNull<u8>, seed: u64, from: usize, to: usize) {
    for i in from..to {
        // SAFETY: per contract `i` is inside the block.
        unsafe { p.as_ptr().add(i).write(pattern(seed, i)) };
    }
}

/// Assert `block[..len]` still holds `seed`'s pattern.
///
/// # Safety
/// `p` is a live block at least `len` bytes long, initialised to `len`.
unsafe fn check(p: NonNull<u8>, seed: u64, len: usize, what: &str) {
    for i in 0..len {
        // SAFETY: per contract `i` is inside the block and initialised.
        let got = unsafe { p.as_ptr().add(i).read() };
        assert_eq!(got, pattern(seed, i), "{what}: byte {i} of {len} changed");
    }
}

/// The checks every returned block must pass: non-null by type, aligned, at
/// least as long as asked, and inside the address space.
fn check_block(b: NonNull<[u8]>, l: Layout, what: &str) -> NonNull<u8> {
    let p = b.cast::<u8>();
    assert_eq!(
        p.as_ptr().addr() % l.align(),
        0,
        "{what}: misaligned for {l:?}"
    );
    assert!(b.len() >= l.size(), "{what}: {} bytes for {l:?}", b.len());
    assert!(
        p.as_ptr().addr().checked_add(b.len()).is_some(),
        "{what}: the block wraps the address space"
    );
    p
}

const SIZES: &[usize] = &[
    1,
    2,
    3,
    7,
    8,
    9,
    15,
    16,
    17,
    24,
    31,
    32,
    33,
    63,
    64,
    65,
    127,
    128,
    129,
    255,
    256,
    511,
    512,
    1000,
    1024,
    4095,
    4096,
    4097,
    8192,
    65_535,
    65_536,
    65_537,
    131_072,
    1 << 20,
    3 << 20,
];
const ALIGNS: &[usize] = &[1, 2, 4, 8, 16, 32, 64, 128, 256, 4096, 65_536, 1 << 20];

/// Sizes and alignments worth the time under miri.
fn matrix() -> impl Iterator<Item = (usize, usize)> {
    let (max_size, max_align) = if cfg!(miri) {
        (1024, 64)
    } else {
        (usize::MAX, usize::MAX)
    };
    SIZES
        .iter()
        .filter(move |&&s| s <= max_size)
        .flat_map(move |&s| {
            ALIGNS
                .iter()
                .filter(move |&&a| a <= max_align)
                .map(move |&a| (s, a))
        })
}

// ---------------------------------------------------------------------------
// The generic checks.
// ---------------------------------------------------------------------------

/// Zero-sized layouts are legal everywhere: allocate, both grows from zero,
/// shrink to zero, and deallocate of the zero-sized result.
fn zero_sized_layouts<A: Allocator>(a: &A) {
    for &align in ALIGNS {
        if cfg!(miri) && align > 4096 {
            continue;
        }
        let z = layout(0, align);
        for zeroed in [false, true] {
            let b = if zeroed {
                a.allocate_zeroed(z)
            } else {
                a.allocate(z)
            };
            let p = check_block(b.expect("a zero-sized allocation"), z, "zero-sized");
            // SAFETY: the block was just allocated with `z`.
            unsafe { a.deallocate(p, z) };
        }

        // Grow out of nothing, plain and zeroed, then shrink back to nothing.
        let big = layout(100, align);
        for zeroed in [false, true] {
            let p = check_block(a.allocate(z).unwrap(), z, "zero-sized");
            // SAFETY: `p` is live with layout `z`, and 100 >= 0.
            let grown = unsafe {
                if zeroed {
                    a.grow_zeroed(p, z, big)
                } else {
                    a.grow(p, z, big)
                }
            };
            let g = check_block(grown.expect("grow from zero"), big, "grow from zero");
            if zeroed {
                for i in 0..100 {
                    // SAFETY: `g` is live and 100 bytes long, all zeroed.
                    let byte = unsafe { g.as_ptr().add(i).read() };
                    assert_eq!(byte, 0, "grow_zeroed from zero");
                }
            }
            // SAFETY: `g` is live and 100 bytes long.
            unsafe { fill(g, 9, 0, 100) };
            // SAFETY: `g` is live with layout `big`, and 0 <= 100.
            let s = unsafe { a.shrink(g, big, z) }.expect("shrink to zero");
            let s = check_block(s, z, "shrink to zero");
            // SAFETY: the zero-sized result is live with layout `z`.
            unsafe { a.deallocate(s, z) };
        }
    }
}

/// Every size against every alignment: the block is aligned, long enough and
/// writable end to end, and the zeroed form is zero.
fn layout_matrix<A: Allocator>(a: &A) {
    for (size, align) in matrix() {
        let l = layout(size, align);
        let p = check_block(a.allocate(l).expect("allocate"), l, "allocate");
        // SAFETY: `p` is live and `size` bytes long; freed once with `l`.
        unsafe {
            fill(p, size as u64, 0, size);
            check(p, size as u64, size, "allocate");
            a.deallocate(p, l);
        }

        let p = check_block(
            a.allocate_zeroed(l).expect("allocate_zeroed"),
            l,
            "allocate_zeroed",
        );
        // SAFETY: `p` is live, `size` bytes long and initialised to zero.
        unsafe {
            for i in 0..size {
                assert_eq!(
                    p.as_ptr().add(i).read(),
                    0,
                    "allocate_zeroed {l:?}: byte {i}"
                );
            }
            // Leave it dirty, so a later zeroed allocation that reuses this
            // memory has something to clear.
            p.as_ptr().write_bytes(0xFF, size);
            a.deallocate(p, l);
        }
    }
}

/// Live blocks never overlap, and none of them disturbs another.
fn blocks_are_disjoint<A: Allocator>(a: &A) {
    let mut rng = Rng(0x00D1_5701);
    let n = scale(600, 30);
    let mut live: Vec<(NonNull<u8>, usize, Layout, u64)> = Vec::new();
    for seed in 0..n as u64 {
        let cap = if rng.below(8) == 0 { 70_000 } else { 400 };
        let size = 1 + rng.below(cap);
        let size = if cfg!(miri) { size.min(600) } else { size };
        let l = layout(size, 1 << rng.below(7));
        let b = a.allocate(l).expect("allocate");
        let p = check_block(b, l, "disjoint");
        // SAFETY: `p` is live and `size` bytes long.
        unsafe { fill(p, seed, 0, size) };
        live.push((p, b.len(), l, seed));
    }
    let mut spans: Vec<(usize, usize)> = live
        .iter()
        .map(|&(p, len, ..)| (p.as_ptr().addr(), len))
        .collect();
    spans.sort_unstable();
    for w in spans.windows(2) {
        assert!(w[0].0 + w[0].1 <= w[1].0, "two live blocks overlap: {w:?}");
    }
    // Free in a scattered order, checking each block as it goes.
    while !live.is_empty() {
        let i = rng.below(live.len());
        let (p, _, l, seed) = live.swap_remove(i);
        // SAFETY: `p` is live, made with `l`, initialised, and freed once.
        unsafe {
            check(p, seed, l.size(), "disjoint");
            a.deallocate(p, l);
        }
    }
}

/// Size pairs that stay in a size class, cross one, and cross into the large
/// and huge paths, so both the in-place and the moving reallocation run.
const RESIZES: &[(usize, usize)] = &[
    (1, 2),
    (8, 9),
    (8, 16),
    (16, 24),
    (24, 1000),
    (100, 104),
    (1000, 1001),
    (1000, 4096),
    (4096, 65_536),
    (60_000, 70_000),
    (65_536, 1 << 20),
    (1 << 20, 3 << 20),
];
const RESIZE_ALIGNS: &[(usize, usize)] = &[
    (1, 1),
    (8, 8),
    (16, 16),
    (64, 64),
    (4096, 4096),
    (8, 64),
    (64, 8),
    (1, 4096),
    (4096, 16),
];

/// Make reused memory dirty, so a `grow_zeroed` that zeroes nothing fails.
fn dirty<A: Allocator>(a: &A, size: usize, align: usize) {
    let l = layout(size, align);
    let blocks: Vec<NonNull<u8>> = (0..4)
        .map(|_| {
            let p = a.allocate(l).expect("allocate").cast::<u8>();
            // SAFETY: `p` is live and `size` bytes long.
            unsafe { p.as_ptr().write_bytes(0xFF, size) };
            p
        })
        .collect();
    for p in blocks {
        // SAFETY: each block is live, made with `l`, freed once.
        unsafe { a.deallocate(p, l) };
    }
}

/// `grow`, `grow_zeroed` and `shrink` keep the bytes they must, zero the bytes
/// they must, and honour a new alignment.
fn resizing<A: Allocator>(a: &A) {
    for &(small, large) in RESIZES {
        if cfg!(miri) && large > 1024 {
            continue;
        }
        for &(align_a, align_b) in RESIZE_ALIGNS {
            if cfg!(miri) && align_a.max(align_b) > 64 {
                continue;
            }
            let (from, to) = (layout(small, align_a), layout(large, align_b));
            let seed = (small * 31 + align_a) as u64;

            // grow
            let p = check_block(a.allocate(from).unwrap(), from, "grow: allocate");
            // SAFETY: `p` is live, `small` bytes long; `large >= small`.
            let g = unsafe {
                fill(p, seed, 0, small);
                a.grow(p, from, to)
            };
            let g = check_block(g.expect("grow"), to, "grow");
            // SAFETY: `g` is live and `large` bytes long, its first `small`
            // bytes initialised by the grow.
            unsafe {
                check(g, seed, small, "grow");
                fill(g, seed, small, large);
            }

            // shrink back, to the first layout
            // SAFETY: `g` is live with layout `to`; `small <= large`.
            let s = unsafe { a.shrink(g, to, from) };
            let s = check_block(s.expect("shrink"), from, "shrink");
            // SAFETY: `s` is live with layout `from`, `small` bytes kept.
            unsafe {
                check(s, seed, small, "shrink");
                a.deallocate(s, from);
            }

            // grow_zeroed, over memory known to be dirty
            dirty(a, large, align_b);
            let p = check_block(a.allocate(from).unwrap(), from, "grow_zeroed: allocate");
            // SAFETY: as the grow above.
            let g = unsafe {
                fill(p, seed, 0, small);
                a.grow_zeroed(p, from, to)
            };
            let g = check_block(g.expect("grow_zeroed"), to, "grow_zeroed");
            // SAFETY: `g` is live, `large` bytes long and fully initialised.
            unsafe {
                check(g, seed, small, "grow_zeroed");
                for i in small..large {
                    assert_eq!(
                        g.as_ptr().add(i).read(),
                        0,
                        "grow_zeroed {from:?} -> {to:?}: byte {i} is not zero"
                    );
                }
                a.deallocate(g, to);
            }
        }
    }
}

/// A request no allocator can meet is `Err`, not a panic and not an abort,
/// and a failed `grow` or `shrink` leaves the old block exactly as it was.
#[cfg(target_pointer_width = "64")]
fn failure_is_an_error<A: Allocator>(a: &A) {
    // `black_box` on both sides of every call here, and it is load-bearing.
    // The optimiser knows the system allocator: an allocation whose result is
    // only compared with null may be deleted and taken to have succeeded. A
    // release build did exactly that to the control arm ("an impossible
    // allocate succeeded", 2026-11-13). An opaque size and a result that
    // escapes leave it nothing to assume.
    let impossible = black_box(layout(isize::MAX as usize - 4095, 1));
    assert!(
        black_box(a.allocate(impossible)).is_err(),
        "an impossible allocate succeeded"
    );
    assert!(
        black_box(a.allocate_zeroed(impossible)).is_err(),
        "an impossible allocate_zeroed succeeded"
    );

    for align in [1usize, 16, 4096] {
        let old = layout(96, align);
        let p = check_block(a.allocate(old).unwrap(), old, "failure: allocate");
        // SAFETY: `p` is live and 96 bytes long.
        unsafe { fill(p, 77, 0, 96) };

        let huge = black_box(layout(isize::MAX as usize - 4095, align));
        // SAFETY: `p` is live with layout `old`, and the new size is larger.
        let grown = black_box(unsafe { a.grow(p, old, huge) });
        assert!(grown.is_err(), "an impossible grow succeeded");
        // SAFETY: a failed grow leaves the block live and unchanged.
        unsafe { check(p, 77, 96, "after a failed grow") };
        // SAFETY: as above.
        let grown = black_box(unsafe { a.grow_zeroed(p, old, huge) });
        assert!(grown.is_err(), "an impossible grow_zeroed succeeded");
        // SAFETY: as above.
        unsafe { check(p, 77, 96, "after a failed grow_zeroed") };

        // A shrink can only fail by needing a move it cannot make: a new
        // alignment nothing can satisfy.
        let unalignable = black_box(layout(32, 1 << 46));
        // SAFETY: `p` is live with layout `old`, and 32 <= 96.
        if black_box(unsafe { a.shrink(p, old, unalignable) }).is_err() {
            // SAFETY: a failed shrink leaves the block live and unchanged.
            unsafe {
                check(p, 77, 96, "after a failed shrink");
                a.deallocate(p, old);
            }
        } else {
            panic!("a shrink to a 2^46 alignment succeeded");
        }
    }
}

struct Live {
    p: NonNull<u8>,
    l: Layout,
    seed: u64,
}

/// A long random walk over all six methods. Every live block carries a
/// pattern that is checked whenever the block is touched and at the end, so
/// an overlap, a lost byte or a wrong free shows up as a changed byte.
fn model<A: Allocator>(a: &A, seed: u64) {
    let mut rng = Rng(seed | 1);
    let mut live: Vec<Live> = Vec::new();
    let mut next_seed = 0u64;
    let steps = scale(20_000, 150);

    let size = |rng: &mut Rng| -> usize {
        let cap = if cfg!(miri) { 3 } else { 5 };
        match rng.below(cap) {
            0 => rng.below(9),       // tiny, zero included
            1 | 2 => rng.below(300), // small
            3 => rng.below(20_000),  // medium
            _ => rng.below(300_000), // large
        }
    };
    let align = |rng: &mut Rng| -> usize {
        if rng.below(4) == 0 {
            1 << rng.below(13)
        } else {
            1 << rng.below(5)
        }
    };

    for _ in 0..steps {
        let op = rng.below(10);
        if live.is_empty() || op < 4 {
            // allocate, plain or zeroed
            let l = layout(size(&mut rng), align(&mut rng));
            let zeroed = rng.below(3) == 0;
            let b = if zeroed {
                a.allocate_zeroed(l)
            } else {
                a.allocate(l)
            };
            let p = check_block(b.expect("model: allocate"), l, "model: allocate");
            next_seed += 1;
            // SAFETY: `p` is live and `l.size()` bytes long; zeroed when asked.
            unsafe {
                if zeroed {
                    for i in 0..l.size() {
                        assert_eq!(p.as_ptr().add(i).read(), 0, "model: allocate_zeroed");
                    }
                }
                fill(p, next_seed, 0, l.size());
            }
            live.push(Live {
                p,
                l,
                seed: next_seed,
            });
        } else if op < 7 {
            // grow or shrink, sometimes to another alignment
            let i = rng.below(live.len());
            let Live { p, l, seed } = live.swap_remove(i);
            let new_align = if rng.below(4) == 0 {
                align(&mut rng)
            } else {
                l.align()
            };
            let new = layout(size(&mut rng), new_align);
            // SAFETY: `p` is live with layout `l` and holds `seed`'s pattern;
            // each arm calls the method whose size precondition holds.
            unsafe {
                check(p, seed, l.size(), "model: before resize");
                let (b, zeroed) = if new.size() >= l.size() {
                    if rng.below(2) == 0 {
                        (a.grow_zeroed(p, l, new), true)
                    } else {
                        (a.grow(p, l, new), false)
                    }
                } else {
                    (a.shrink(p, l, new), false)
                };
                let q = check_block(b.expect("model: resize"), new, "model: resize");
                let kept = l.size().min(new.size());
                check(q, seed, kept, "model: after resize");
                if zeroed {
                    for i in kept..new.size() {
                        assert_eq!(q.as_ptr().add(i).read(), 0, "model: grow_zeroed tail");
                    }
                }
                fill(q, seed, kept, new.size());
                live.push(Live { p: q, l: new, seed });
            }
        } else {
            // deallocate
            let i = rng.below(live.len());
            let Live { p, l, seed } = live.swap_remove(i);
            // SAFETY: `p` is live with layout `l`, initialised, freed once.
            unsafe {
                check(p, seed, l.size(), "model: before deallocate");
                a.deallocate(p, l);
            }
        }
    }
    for Live { p, l, seed } in live {
        // SAFETY: as the deallocate arm.
        unsafe {
            check(p, seed, l.size(), "model: at the end");
            a.deallocate(p, l);
        }
    }
}

/// The standard collections that take an allocator, used as a program would.
fn collections<A: Allocator + Copy>(a: A) {
    #[repr(align(64))]
    #[derive(Clone, Copy)]
    struct Wide(u64);

    let mut v: Vec<u64, A> = Vec::new_in(a);
    let n = scale(100_000, 500) as u64;
    for i in 0..n {
        v.push(i);
    }
    assert_eq!(v.iter().sum::<u64>(), n * (n - 1) / 2);
    v.insert(0, 7);
    v.retain(|x| x % 3 == 0);
    v.truncate(10);
    v.shrink_to_fit();
    assert_eq!(v.capacity(), 10);
    v.clear();
    v.shrink_to_fit();
    assert_eq!(v.capacity(), 0);

    let mut w: Vec<Wide, A> = Vec::with_capacity_in(3, a);
    for i in 0..scale(1000, 40) as u64 {
        w.push(Wide(i));
    }
    for (i, x) in w.iter().enumerate() {
        assert_eq!(x.0, i as u64);
        assert_eq!(core::ptr::from_ref(x).addr() % 64, 0);
    }

    let b = Box::new_in(Wide(41), a);
    assert_eq!((*b).0 + 1, 42);
    assert_eq!(core::ptr::from_ref(&*b).addr() % 64, 0);

    // Zero-sized values and empty collections allocate nothing and free nothing.
    let unit = Box::new_in((), a);
    drop(unit);
    let mut units: Vec<(), A> = Vec::new_in(a);
    units.extend(core::iter::repeat_n((), 1000));
    assert_eq!(units.len(), 1000);

    // Nested: boxes inside a vector, all on the same allocator.
    let mut nested: Vec<Box<[u8; 100], A>, A> = Vec::new_in(a);
    for i in 0..scale(500, 20) {
        nested.push(Box::new_in([i as u8; 100], a));
    }
    assert!(
        nested
            .iter()
            .enumerate()
            .all(|(i, b)| b.iter().all(|&x| x == i as u8))
    );

    // Through raw parts and back.
    let mut raw: Vec<u32, A> = Vec::with_capacity_in(16, a);
    raw.extend(0..16);
    let (ptr, len, cap, alloc) = raw.into_raw_parts_with_allocator();
    // SAFETY: the parts are exactly what `into_raw_parts_with_allocator` gave.
    let back = unsafe { Vec::from_raw_parts_in(ptr, len, cap, alloc) };
    assert_eq!(back.iter().sum::<u32>(), 120);

    let boxed = Box::new_in(5u16, a);
    let (ptr, alloc) = Box::into_raw_with_allocator(boxed);
    // SAFETY: the pair is exactly what `into_raw_with_allocator` gave.
    assert_eq!(*unsafe { Box::from_raw_in(ptr, alloc) }, 5);
}

/// The trait is dyn-compatible, and a reference to an allocator is an
/// equivalent allocator: allocate through one, free through the other.
fn references_and_dyn<A: Allocator>(a: &A) {
    let d: &dyn Allocator = a;
    let mut v: Vec<u8, &dyn Allocator> = Vec::new_in(d);
    v.extend_from_slice(b"dyn-compatible");
    assert_eq!(v, b"dyn-compatible");

    let l = layout(48, 8);
    let by_ref = &a;
    let p = check_block(by_ref.allocate(l).unwrap(), l, "by reference");
    // SAFETY: `&&A` and `A` are equivalent allocators; the block is live.
    unsafe {
        fill(p, 3, 0, 48);
        check(p, 3, 48, "by reference");
        a.deallocate(p, l);
    }
}

// ---------------------------------------------------------------------------
// The four arms.
// ---------------------------------------------------------------------------

macro_rules! conformance {
    ($name:ident, $make:expr) => {
        mod $name {
            use super::*;

            #[test]
            fn zero_sized_layouts() {
                super::zero_sized_layouts(&$make);
            }
            #[test]
            fn layout_matrix() {
                super::layout_matrix(&$make);
            }
            #[test]
            fn blocks_are_disjoint() {
                super::blocks_are_disjoint(&$make);
            }
            #[test]
            fn resizing() {
                super::resizing(&$make);
            }
            #[test]
            #[cfg(target_pointer_width = "64")]
            #[cfg_attr(miri, ignore = "miri's own allocator aborts on an impossible request")]
            fn failure_is_an_error() {
                super::failure_is_an_error(&$make);
            }
            #[test]
            fn model() {
                for seed in 1..=scale(4, 1) as u64 {
                    super::model(&$make, seed.wrapping_mul(0x9E37_79B9));
                }
            }
            #[test]
            fn collections() {
                let a = $make;
                super::collections(&a);
            }
            #[test]
            fn references_and_dyn() {
                super::references_and_dyn(&$make);
            }
        }
    };
}

conformance!(system_control, System);
conformance!(global_handle, RustyAlloc);
conformance!(heap, Heap::new());
conformance!(destroyable_heap, Heap::new_destroyable());

// ---------------------------------------------------------------------------
// What holds for this crate's impls beyond the contract.
// ---------------------------------------------------------------------------

/// A zero-sized layout never reaches the allocator: the answer is the
/// alignment itself as an address, with length 0.
#[test]
fn a_zero_sized_block_is_dangling() {
    let heap = Heap::new();
    for &align in ALIGNS {
        let z = layout(0, align);
        for b in [
            RustyAlloc.allocate(z).unwrap(),
            RustyAlloc.allocate_zeroed(z).unwrap(),
            heap.allocate(z).unwrap(),
            heap.allocate_zeroed(z).unwrap(),
        ] {
            assert_eq!(b.cast::<u8>().as_ptr().addr(), align);
            assert_eq!(b.len(), 0);
        }
    }
}

/// The length returned is the length asked for (see the module docs for why
/// the size class is not reported).
#[test]
fn the_length_returned_is_the_length_requested() {
    let heap = Heap::new();
    for (size, align) in matrix() {
        let l = layout(size, align);
        let b = RustyAlloc.allocate(l).unwrap();
        assert_eq!(b.len(), size);
        // SAFETY: just allocated with `l`.
        unsafe { RustyAlloc.deallocate(b.cast(), l) };
        let b = heap.allocate(l).unwrap();
        assert_eq!(b.len(), size);
        // SAFETY: just allocated from `heap` with `l`.
        unsafe { heap.deallocate(b.cast(), l) };
    }
}

/// `RustyAlloc` is a handle to process-wide state: a collection built on one
/// thread can be grown and dropped on another.
#[test]
fn the_global_handle_crosses_threads() {
    fn send_sync<T: Send + Sync + Copy + Default>() {}
    send_sync::<RustyAlloc>();

    let threads = scale(8, 2);
    let per = scale(20_000, 100) as u64;
    // Collected on purpose: every worker is running before the first is
    // joined. Joining straight off the iterator would run them one by one.
    #[allow(clippy::needless_collect)]
    let handles: Vec<_> = (0..threads as u64)
        .map(|t| {
            let mut v: Vec<u64, RustyAlloc> = Vec::new_in(RustyAlloc);
            v.extend(0..per);
            std::thread::spawn(move || {
                v.extend(per..2 * per);
                assert_eq!(v.iter().sum::<u64>(), 2 * per * (2 * per - 1) / 2);
                let boxed: Vec<Box<u64, RustyAlloc>, RustyAlloc> = {
                    let mut b = Vec::new_in(RustyAlloc);
                    b.extend((0..per / 10).map(|i| Box::new_in(i + t, RustyAlloc)));
                    b
                };
                boxed
            })
        })
        .collect();
    for (t, h) in handles.into_iter().enumerate() {
        // Blocks allocated on the worker are freed here, on another thread.
        let boxed = h.join().expect("worker");
        assert_eq!(*boxed[0], t as u64);
    }
}

/// A heap can be owned by the collection, borrowed by many, and dropped with
/// blocks still outstanding.
#[test]
fn heaps_own_borrow_and_drop() {
    // Owned: the vector frees its buffer, then drops the heap.
    let mut owned: Vec<String, Heap> = Vec::new_in(Heap::new());
    owned.extend((0..scale(2000, 100)).map(|i| i.to_string()));
    assert_eq!(owned[99], "99");
    drop(owned);

    // Borrowed by several collections at once.
    let heap = Heap::new_destroyable();
    {
        let mut a: Vec<u32, &Heap> = Vec::new_in(&heap);
        let mut b: Vec<Box<u64, &Heap>, &Heap> = Vec::new_in(&heap);
        for i in 0..scale(5000, 200) {
            a.push(i as u32);
            b.push(Box::new_in(i as u64, &heap));
        }
        assert_eq!(a.len(), b.len());
        heap.collect();
        assert!(b.iter().enumerate().all(|(i, x)| **x == i as u64));
    }

    // Outstanding blocks when the heap goes: a destroyable heap releases
    // them wholesale, an ordinary one hands them to the thread's heap.
    for destroyable in [true, false] {
        let heap = if destroyable {
            Heap::new_destroyable()
        } else {
            Heap::new()
        };
        let l = layout(256, 16);
        let leaked: Vec<NonNull<u8>> = (0..50).map(|_| heap.allocate(l).unwrap().cast()).collect();
        drop(heap);
        if !destroyable {
            for p in leaked {
                // SAFETY: an ordinary heap's blocks outlive it (they migrate
                // to the thread's heap), and the global handle's `free` takes
                // any block of the allocator. This is the documented
                // `mi_heap_delete` behaviour, not something the `Allocator`
                // contract offers, which is why it is spelled with
                // `GlobalAlloc` here.
                unsafe { std::alloc::GlobalAlloc::dealloc(&RustyAlloc, p.as_ptr(), l) };
            }
        }
    }
}

/// What a heap allocates stays in that heap when it moves. This is the one
/// thing the global handle's reallocation could not be reused for: it takes a
/// moved block from the thread's own heap, and a block that left a
/// destroyable heap that way would outlive the heap's teardown.
///
/// Three ways a block moves, since each is its own code: a copy of at most 64
/// bytes into a block the size-class table can hand over, a small copy the
/// table cannot serve, and a larger one.
#[test]
#[cfg_attr(
    miri,
    ignore = "the move under test is native code; miri takes the general path"
)]
fn a_block_that_moves_stays_in_its_heap() {
    let outside = RustyAlloc.allocate(layout(64, 8)).unwrap().cast::<u8>();
    for heap in [Heap::new(), Heap::new_destroyable()] {
        assert!(
            !heap.contains(outside),
            "the check cannot tell this heap from the global allocator"
        );

        // By hand, across every size class boundary up to 64 KiB: each step
        // is a grow that has to move, and each lands where it started.
        let mut l = layout(16, 8);
        let mut p = heap.allocate(l).unwrap().cast::<u8>();
        assert!(heap.contains(p));
        let mut moves = 0;
        for size in [
            24, 40, 64, 72, 129, 300, 700, 1500, 4000, 9000, 20_000, 65_536,
        ] {
            let new = layout(size, 8);
            // SAFETY: `p` is live with layout `l`, and `size` is larger.
            let q = unsafe { heap.grow(p, l, new) }.unwrap().cast::<u8>();
            moves += usize::from(q != p);
            assert!(
                heap.contains(q),
                "a block grown to {size} bytes left its heap"
            );
            (p, l) = (q, new);
        }
        assert!(
            moves >= 8,
            "only {moves} of 12 grows moved: the test lost its subject"
        );
        // And back down far enough that the shrink moves too.
        let small = layout(16, 8);
        // SAFETY: `p` is live with layout `l`, and 16 is smaller.
        let q = unsafe { heap.shrink(p, l, small) }.unwrap().cast::<u8>();
        assert!(heap.contains(q), "a shrunk block left its heap");
        // SAFETY: `q` is live with layout `small`.
        unsafe { heap.deallocate(q, small) };

        // And as a collection does it.
        let mut v: Vec<u64, &Heap> = Vec::new_in(&heap);
        let mut cap = 0;
        for i in 0..20_000u64 {
            v.push(i);
            if v.capacity() != cap {
                cap = v.capacity();
                assert!(
                    heap.contains(NonNull::from(&v[0]).cast()),
                    "a Vec's buffer left its heap at capacity {cap}"
                );
            }
        }
    }
    // SAFETY: allocated above with this layout.
    unsafe { RustyAlloc.deallocate(outside, layout(64, 8)) };
}

/// Blocks from different heaps and from the global handle never overlap.
#[test]
fn heaps_and_the_global_handle_are_disjoint() {
    let heaps = [Heap::new(), Heap::new_destroyable(), Heap::new()];
    let l = layout(120, 8);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut blocks: Vec<(usize, NonNull<u8>)> = Vec::new();
    for round in 0..scale(400, 40) {
        let which = round % 4;
        let b = if which == 3 {
            RustyAlloc.allocate(l)
        } else {
            heaps[which].allocate(l)
        };
        let b = b.unwrap();
        spans.push((b.cast::<u8>().as_ptr().addr(), b.len()));
        blocks.push((which, b.cast()));
    }
    spans.sort_unstable();
    for w in spans.windows(2) {
        assert!(
            w[0].0 + w[0].1 <= w[1].0,
            "blocks of different allocators overlap"
        );
    }
    for (which, p) in blocks {
        // SAFETY: each block goes back to the allocator that made it.
        unsafe {
            if which == 3 {
                RustyAlloc.deallocate(p, l)
            } else {
                heaps[which].deallocate(p, l)
            }
        }
    }
}

/// An alignment above one word on a block shorter than the alignment: the
/// case where the size class alone does not carry the alignment.
#[test]
fn short_blocks_keep_a_large_alignment() {
    let heap = Heap::new();
    for align in [2usize, 4, 8, 16, 32, 64, 4096] {
        for size in 1..=align.min(40) {
            let l = layout(size, align);
            for _ in 0..scale(8, 2) {
                let a = heap.allocate(l).unwrap();
                let b = RustyAlloc.allocate(l).unwrap();
                check_block(a, l, "heap, short block");
                check_block(b, l, "global, short block");
                // SAFETY: both were just allocated with `l`.
                unsafe {
                    heap.deallocate(a.cast(), l);
                    RustyAlloc.deallocate(b.cast(), l);
                }
            }
        }
    }
}
