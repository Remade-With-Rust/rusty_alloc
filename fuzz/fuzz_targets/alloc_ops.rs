//! H-26 target 1: arbitrary single-threaded op sequences over the whole
//! public allocation surface — malloc/zalloc/aligned/realloc/usable/free —
//! with a CANARY discipline, so the fuzzer detects the failure class this
//! allocator exists to prevent (two owners on one block, overlap, contents
//! not surviving) and not merely crashes. libFuzzer runs iterations
//! in-process, so allocator state (pages, segments, arenas, retire/adopt
//! churn) accumulates realistically across inputs.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rusty_alloc::alloc;

const MAX_LIVE: usize = 256;
const MAX_TOTAL_USABLE: usize = 64 << 20; // 64 MiB cap per input

struct Block {
    p: *mut u8,
    usable: usize,
    fill: u8,
}

fn next16(it: &mut impl Iterator<Item = u8>) -> u16 {
    let a = it.next().unwrap_or(0) as u16;
    let b = it.next().unwrap_or(0) as u16;
    a | (b << 8)
}

/// Verify the canary at the block's edges and middle, then free it.
unsafe fn check_and_free(b: Block, total: &mut usize) {
    unsafe {
        if b.usable > 0 {
            for off in [0, b.usable / 2, b.usable - 1] {
                assert_eq!(
                    *b.p.add(off),
                    b.fill,
                    "canary mismatch: block contents did not survive — overlap or two owners"
                );
            }
        }
        alloc::free(b.p);
    }
    *total -= b.usable;
}

fuzz_target!(|data: &[u8]| {
    let mut it = data.iter().copied();
    let mut live: Vec<Block> = Vec::new();
    let mut total = 0usize;
    let mut fill_seq: u8 = 0xA5;

    while let Some(op) = it.next() {
        match op % 7 {
            // malloc: sizes 0..=65535 exercise every small/med bin + large spans.
            0 => {
                let size = next16(&mut it) as usize;
                if live.len() >= MAX_LIVE || total + size > MAX_TOTAL_USABLE {
                    continue;
                }
                let p = alloc::malloc(size);
                if !p.is_null() {
                    // SAFETY: p is a live block with usable_size(p) >= size bytes.
                    let usable = unsafe { alloc::usable_size(p) };
                    assert!(usable >= size, "usable_size below the request");
                    fill_seq = fill_seq.wrapping_add(1);
                    // SAFETY: writing within the usable extent of a live block.
                    unsafe { core::ptr::write_bytes(p, fill_seq, usable) };
                    total += usable;
                    live.push(Block { p, usable, fill: fill_seq });
                }
            }
            // zalloc: the zero guarantee is an invariant, not a suggestion.
            1 => {
                let size = next16(&mut it) as usize;
                if live.len() >= MAX_LIVE || total + size > MAX_TOTAL_USABLE {
                    continue;
                }
                let p = alloc::zalloc(size);
                if !p.is_null() {
                    // SAFETY: live block; zalloc promises zero across the usable extent.
                    let usable = unsafe { alloc::usable_size(p) };
                    unsafe {
                        for off in [0, size.saturating_sub(1), usable.saturating_sub(1)] {
                            if usable > 0 {
                                assert_eq!(*p.add(off), 0, "zalloc handed back a dirty block");
                            }
                        }
                        core::ptr::write_bytes(p, 0x5A, usable);
                    }
                    total += usable;
                    live.push(Block { p, usable, fill: 0x5A });
                }
            }
            // aligned: every power of two up to 4096, and the alignment is CHECKED.
            2 => {
                let size = next16(&mut it) as usize;
                let align = 1usize << (it.next().unwrap_or(0) % 13);
                if live.len() >= MAX_LIVE || total + size > MAX_TOTAL_USABLE {
                    continue;
                }
                let p = alloc::malloc_aligned(size, align);
                if !p.is_null() {
                    assert_eq!(p.addr() % align, 0, "malloc_aligned returned a misaligned block");
                    // SAFETY: live block within its usable extent.
                    let usable = unsafe { alloc::usable_size(p) };
                    assert!(usable >= size);
                    fill_seq = fill_seq.wrapping_add(1);
                    unsafe { core::ptr::write_bytes(p, fill_seq, usable) };
                    total += usable;
                    live.push(Block { p, usable, fill: fill_seq });
                }
            }
            // realloc: the prefix must survive a move; refill after.
            3 => {
                if live.is_empty() {
                    continue;
                }
                let i = next16(&mut it) as usize % live.len();
                let newsize = next16(&mut it) as usize;
                let b = live.swap_remove(i);
                total -= b.usable;
                // SAFETY: b.p is live and ours; realloc consumes it on move.
                let np = unsafe { alloc::realloc(b.p, newsize) };
                if np.is_null() {
                    // Contract: the original is untouched on failure.
                    total += b.usable;
                    live.push(b);
                } else {
                    // SAFETY: np is live; the prefix min(old, new) is preserved.
                    let usable = unsafe { alloc::usable_size(np) };
                    assert!(usable >= newsize);
                    let keep = b.usable.min(newsize);
                    unsafe {
                        for off in [0, keep / 2, keep.saturating_sub(1)] {
                            if keep > 0 {
                                assert_eq!(*np.add(off), b.fill, "realloc lost the prefix");
                            }
                        }
                        fill_seq = fill_seq.wrapping_add(1);
                        core::ptr::write_bytes(np, fill_seq, usable);
                    }
                    total += usable;
                    live.push(Block { p: np, usable, fill: fill_seq });
                }
            }
            // free (canary-checked), fuzzer-chosen victim → LIFO/FIFO/random orders.
            4 => {
                if live.is_empty() {
                    continue;
                }
                let i = next16(&mut it) as usize % live.len();
                let b = live.swap_remove(i);
                // SAFETY: b.p is live and ours exactly once.
                unsafe { check_and_free(b, &mut total) };
            }
            // usable_size stability on a live block.
            5 => {
                if live.is_empty() {
                    continue;
                }
                let i = next16(&mut it) as usize % live.len();
                let b = &live[i];
                // SAFETY: live block.
                let u = unsafe { alloc::usable_size(b.p) };
                assert_eq!(u, b.usable, "usable_size changed while the block was live");
            }
            // collect: force the heartbeat/retire machinery mid-sequence.
            _ => {
                alloc::collect(it.next().unwrap_or(0) & 1 == 1);
            }
        }
    }

    // Drain: every survivor is canary-checked on the way out.
    for b in live.drain(..) {
        // SAFETY: each block is live and freed exactly once.
        unsafe { check_and_free(b, &mut total) };
    }
    assert_eq!(total, 0);
});
