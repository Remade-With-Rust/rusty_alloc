//! `GlobalAlloc` serves layouts aligned up to two words from the natural size
//! classes (`NATURAL_ALIGN` in `lib.rs`), trusting that every class of two
//! words or more is two-word aligned. This pins that trust for every small
//! size, across the medium and large bands and into huge, through `alloc`,
//! `alloc_zeroed` and every `realloc` step between them — including the
//! one-word requests that must be raised to reach an aligned class.

use rusty_alloc_api::RustyAlloc;
use std::alloc::{GlobalAlloc, Layout};

/// Two words: 16 on a 64-bit target, the alignment hashbrown asks for.
const NATURAL: usize = 2 * core::mem::size_of::<usize>();

fn sizes() -> impl Iterator<Item = usize> {
    (0..=4200usize).chain([
        8191,
        16_385,
        65_535,
        131_072,
        300_001,
        1 << 20,
        (4 << 20) + 3,
    ])
}

#[test]
fn natural_alignment_holds_for_alloc_zeroed_and_realloc() {
    let a = RustyAlloc;
    let align = NATURAL;
    {
        let mut prev: Option<(*mut u8, Layout)> = None;
        for size in sizes() {
            let l = Layout::from_size_align(size, align).unwrap();
            // SAFETY: valid layout; each block is freed exactly once below.
            unsafe {
                let p = a.alloc(l);
                assert!(!p.is_null());
                assert_eq!(p.addr() % align, 0, "alloc({size}, {align}) misaligned");
                p.write_bytes(0xA5, size);
                a.dealloc(p, l);

                let z = a.alloc_zeroed(l);
                assert!(!z.is_null());
                assert_eq!(
                    z.addr() % align,
                    0,
                    "alloc_zeroed({size}, {align}) misaligned"
                );
                assert!(std::slice::from_raw_parts(z, size).iter().all(|&b| b == 0));
                a.dealloc(z, l);

                // Walk one block through every size: grows, and (at the
                // jumps back down between bands) shrinks, in place or moved.
                let (q, ql) = match prev.take() {
                    None => {
                        let q = a.alloc(l);
                        q.write_bytes(0x5A, size);
                        (q, l)
                    }
                    Some((old, ol)) => {
                        let q = a.realloc(old, ol, size);
                        assert!(!q.is_null());
                        let keep = ol.size().min(size);
                        assert!(
                            std::slice::from_raw_parts(q, keep)
                                .iter()
                                .all(|&b| b == 0x5A),
                            "realloc {} -> {size} lost bytes",
                            ol.size()
                        );
                        q.add(keep).write_bytes(0x5A, size - keep);
                        (q, l)
                    }
                };
                assert_eq!(
                    q.addr() % align,
                    0,
                    "realloc(.., {size}) at {align} misaligned"
                );
                prev = Some((q, ql));
            }
        }
        if let Some((q, ql)) = prev {
            // SAFETY: last live block of the walk.
            unsafe { a.dealloc(q, ql) };
        }
    }
}

/// Above two words `realloc` may now keep a block in place. Walk one block
/// up and down through sizes at alignments the classes do not give for free:
/// every result must keep the alignment and the live prefix.
#[test]
fn over_aligned_realloc_keeps_alignment_and_bytes() {
    let a = RustyAlloc;
    for align in [64usize, 4096] {
        let steps = [
            64usize, 128, 100, 64, 40, 700, 350, 5000, 2600, 70_000, 36_000, 64,
        ];
        let mut l = Layout::from_size_align(steps[0], align).unwrap();
        // SAFETY: valid layout; the block is realloc'd through the walk and
        // freed once at the end.
        unsafe {
            let mut p = a.alloc(l);
            assert!(!p.is_null());
            for (i, b) in std::slice::from_raw_parts_mut(p, l.size())
                .iter_mut()
                .enumerate()
            {
                *b = i as u8;
            }
            for &next in &steps[1..] {
                let q = a.realloc(p, l, next);
                assert!(!q.is_null());
                assert_eq!(
                    q.addr() % align,
                    0,
                    "realloc {} -> {next} at {align}",
                    l.size()
                );
                let keep = l.size().min(next);
                assert!(
                    std::slice::from_raw_parts(q, keep)
                        .iter()
                        .enumerate()
                        .all(|(i, &b)| b == i as u8),
                    "realloc {} -> {next} at {align} lost bytes",
                    l.size()
                );
                for (i, b) in std::slice::from_raw_parts_mut(q, next)
                    .iter_mut()
                    .enumerate()
                {
                    *b = i as u8;
                }
                p = q;
                l = Layout::from_size_align(next, align).unwrap();
            }
            a.dealloc(p, l);
        }
    }
}
