// Allocator speed inside a real WebAssembly VM. One source, both arms; `--cfg`
// feature `ra` swaps the allocator and changes nothing else.
//
// Same discipline as the ESP32 harness: a FLOOR arm that allocates nothing, a
// checksum that the optimiser cannot elide the work past, and identical size
// sequences from a seeded PRNG so both arms provably do the same work.
use std::alloc::{alloc, dealloc, Layout};

#[cfg(feature = "ra")]
#[global_allocator]
static A: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;

static mut SCRATCH: [u8; 4096] = [0; 4096];

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

#[inline(never)]
fn touch(p: *mut u8, size: usize, tag: u8) -> u32 {
    if p.is_null() {
        return 0;
    }
    unsafe {
        p.write_volatile(tag);
        let last = p.add(size - 1);
        last.write_volatile(tag ^ 0x5A);
        u32::from(p.read_volatile()) + u32::from(last.read_volatile())
    }
}

fn take(size: usize) -> *mut u8 {
    unsafe { alloc(Layout::from_size_align_unchecked(size, 8)) }
}
fn give(p: *mut u8, size: usize) {
    if !p.is_null() {
        unsafe { dealloc(p, Layout::from_size_align_unchecked(size, 8)) }
    }
}

/// 0 floor, 1 pingpong-32B, 2 batch-64-mixed, 3 churn, 4 large-2048
#[no_mangle]
pub extern "C" fn bench(kind: u32, iters: u32) -> u32 {
    let mut sum = 0u32;
    match kind {
        0 => {
            let s = &raw mut SCRATCH as *mut u8;
            for i in 0..iters {
                sum = sum.wrapping_add(touch(s, 32, i as u8));
            }
        }
        1 => {
            for i in 0..iters {
                let p = take(32);
                sum = sum.wrapping_add(touch(p, 32, i as u8));
                give(p, 32);
            }
        }
        2 => {
            const SIZES: [usize; 8] = [8, 24, 48, 96, 160, 256, 384, 512];
            let mut live: Vec<(*mut u8, usize)> = Vec::with_capacity(64);
            for r in 0..iters {
                for i in 0..64usize {
                    let sz = SIZES[i & 7];
                    let p = take(sz);
                    sum = sum.wrapping_add(touch(p, sz, r as u8));
                    live.push((p, sz));
                }
                while let Some((p, sz)) = live.pop() {
                    give(p, sz);
                }
            }
        }
        3 => {
            let mut rng = Rng(0x1234_5678);
            let mut live: Vec<(*mut u8, usize)> = Vec::with_capacity(64);
            for _ in 0..64 {
                let sz = 8 + (rng.next() % 504) as usize;
                let p = take(sz);
                sum = sum.wrapping_add(touch(p, sz, 1));
                live.push((p, sz));
            }
            for _ in 0..iters {
                let slot = (rng.next() % 64) as usize;
                let (op, osz) = live[slot];
                give(op, osz);
                let sz = 8 + (rng.next() % 504) as usize;
                let p = take(sz);
                sum = sum.wrapping_add(touch(p, sz, 2));
                live[slot] = (p, sz);
            }
            for (p, sz) in live.drain(..) {
                give(p, sz);
            }
        }
        _ => {
            for i in 0..iters {
                let p = take(2048);
                sum = sum.wrapping_add(touch(p, 2048, i as u8));
                give(p, 2048);
            }
        }
    }
    sum
}

/// Slow-path trips, so a speed difference can be attributed instead of guessed.
#[cfg(feature = "ra")]
#[no_mangle]
pub extern "C" fn generic_trips() -> u32 {
    rusty_alloc::alloc::stats().generic as u32
}

/// Page carves, so "why does this size re-enter the slow path" can be answered
/// with the counter rather than a story about it.
#[cfg(feature = "ra")]
#[no_mangle]
pub extern "C" fn extends() -> u32 {
    rusty_alloc::alloc::stats().extends as u32
}
#[cfg(feature = "ra")]
#[no_mangle]
pub extern "C" fn pages_fresh() -> u32 {
    rusty_alloc::alloc::stats().pages_fresh as u32
}
#[cfg(not(feature = "ra"))]
#[no_mangle]
pub extern "C" fn generic_trips() -> u32 {
    0
}
