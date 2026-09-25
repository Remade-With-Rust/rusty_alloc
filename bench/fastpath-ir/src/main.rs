//! How many INSTRUCTIONS an alloc/free pair costs, so the Xtensa CYCLE
//! figure can be read for what it is — and what a fixed-size POOL costs
//! instead.
//!
//! The A/B cell on an ESP32-S3 measures **88 cycles** for an alloc/free pair
//! at 16 bytes and **101 at 256**. Reading the fast path says it is seven
//! operations, with `free` as its mirror. Seven operations cannot cost 88
//! cycles unless something other than instruction count sets the price, so
//! the count is worth having exactly.
//!
//! Callgrind is deterministic, so these numbers have no error bars.
//!
//! # The two arms
//!
//! * `general` — `RustyAlloc` through `GlobalAlloc`, which is what the A/B
//!   cell measures and what a `Box` or a `Vec` reaches.
//! * `pool` — a fixed-size free list. **A different strategy, not a faster
//!   allocator**, and it is only applicable where the size is known at
//!   compile time and the capacity can be pre-sized. An RTOS is full of
//!   exactly that: TCBs, queue items, timer records. It answers a narrower
//!   question, which is why it can answer it in fewer instructions.
//!
//! Both do the identical surrounding work — the same write, read, `black_box`
//! and checksum — so the difference is the allocation strategy and nothing
//! else.
//!
//! # Method
//!
//! Counted at several lengths; the cost of one pair is the **slope**, so
//! process start-up, `ld.so`, allocator init and first-touch warm-up cancel
//! exactly instead of being estimated. Same shape as `bench/opscan.sh` here.
//!
//! ```sh
//! cargo build --release
//! for n in 1000000 2000000 3000000; do
//!     valgrind --tool=callgrind --callgrind-out-file=/dev/null \
//!         ./target/release/fastpath-ir "$n" 256 general
//! done
//! ```
//!
//! # What it does NOT measure
//!
//! Cycles, and so not the quantity the A/B cell reports. This is also a
//! 64-bit host, where `SMALL_SIZE_MAX` is 1 KiB rather than the 512 bytes a
//! 32-bit chip gets, so the size BOUNDARY sits elsewhere here. Only the
//! shape carries over, which is all this is asked for.

use std::alloc::{GlobalAlloc, Layout};

#[global_allocator]
static ALLOC: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;

/// A fixed-size free list over one contiguous arena.
///
/// No size classes, no bins, no page lookup: the block size is a constant of
/// the pool, so `alloc` is a pop and `free` is a push. The bounds checks are
/// left in — this is what a SAFE pool costs, not what an unchecked one
/// could.
struct Pool {
    arena: Vec<u8>,
    block: usize,
    free: Vec<usize>,
}

impl Pool {
    fn new(block: usize, blocks: usize) -> Self {
        Self {
            arena: vec![0u8; block * blocks],
            block,
            // Highest offset first, so the first pop hands out offset 0.
            free: (0..blocks).rev().map(|i| i * block).collect(),
        }
    }

    #[inline]
    fn alloc(&mut self) -> Option<usize> {
        self.free.pop()
    }

    #[inline]
    fn dealloc(&mut self, at: usize) {
        self.free.push(at);
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let rounds: usize = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(1_000_000);
    let size: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(256);
    let arm = args.next().unwrap_or_else(|| "general".to_owned());

    let mut sum: u64 = 0;

    match arm.as_str() {
        "pool" => {
            let mut pool = Pool::new(size, 64);
            for i in 0..rounds {
                let at = pool.alloc().expect("the pool is not exhausted");
                if let Some(slot) = pool.arena.get_mut(at) {
                    *slot = (i & 0xff) as u8;
                    sum = sum.wrapping_add(u64::from(*slot));
                }
                std::hint::black_box(at);
                pool.dealloc(at);
            }
        }
        _ => {
            let layout = Layout::from_size_align(size, 8).expect("a valid layout");
            for i in 0..rounds {
                // SAFETY: a non-zero layout, and the pointer is freed exactly
                // once below with the layout it was allocated with.
                unsafe {
                    let p = ALLOC.alloc(layout);
                    assert!(!p.is_null(), "out of memory");
                    p.write((i & 0xff) as u8);
                    sum = sum.wrapping_add(u64::from(p.read()));
                    std::hint::black_box(p);
                    ALLOC.dealloc(p, layout);
                }
            }
        }
    }

    println!("rounds={rounds} size={size} arm={arm} checksum={sum}");
}
