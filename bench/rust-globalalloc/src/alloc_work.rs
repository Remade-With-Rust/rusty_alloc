//! One body of collection work, generic over the allocator the collections
//! are built with, so three binaries can run the identical program:
//!
//! * `vec_global`: `std::alloc::Global`, which reaches rusty_alloc through
//!   `#[global_allocator]` and the `__rust_alloc` shims (what every Rust
//!   program on this allocator does today);
//! * `vec_direct`: `RustyAlloc` as an `Allocator`, called directly;
//! * `vec_heap`: a first-class `Heap`, by reference.
//!
//! The work is the three shapes `Vec` and `Box` put on an allocator: growth by
//! doubling (`grow`), exact shrinks and re-reserves (`shrink`, `grow` in
//! place), and same-size box churn (`allocate`/`deallocate` pairs). The
//! checksum depends on the values only, never on an address or a capacity
//! the allocator chose, so it is the same number in all three.

use core::alloc::Allocator;

pub fn work<A: Allocator + Copy>(a: A, n: u64) -> u64 {
    let mut sink = 0u64;
    let mut ring: Vec<Option<Box<[u64; 4], A>>, A> = Vec::with_capacity_in(64, a);
    ring.extend((0..64).map(|_| None));
    let mut bytes: Vec<u8, A> = Vec::new_in(a);

    for r in 0..n {
        // Growth by doubling, from empty, to a length that crosses a few
        // size classes; then gone.
        let len = (r * 7 + 3) % 200 + 1;
        let mut v: Vec<u64, A> = Vec::new_in(a);
        for i in 0..len {
            v.push(i ^ r);
        }
        sink = sink.wrapping_add(v.iter().sum::<u64>());

        // An exact shrink and a small re-reserve: `shrink`, then a `grow`
        // the size class can usually satisfy in place.
        v.truncate(len as usize * 3 / 4);
        v.shrink_to_fit();
        v.reserve_exact(2);
        v.push(r);
        sink = sink.wrapping_add(v.len() as u64);

        // Same-size boxes replacing each other: a free and an allocation of
        // one class, the pair an allocator's fast paths are for.
        let slot = (r % 64) as usize;
        ring[slot] = Some(Box::new_in([r, r + 1, r + 2, r + 3], a));
        if let Some(b) = &ring[(slot + 17) % 64] {
            sink = sink.wrapping_add(b[2]);
        }

        // A byte buffer that keeps growing and is cut back now and then.
        bytes.extend_from_slice(&r.to_le_bytes());
        if r % 97 == 96 {
            sink = sink.wrapping_add(bytes.iter().map(|&x| u64::from(x)).sum::<u64>());
            bytes.truncate(16);
            bytes.shrink_to_fit();
        }
    }
    sink
}

pub fn steps() -> u64 {
    std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20000)
}
