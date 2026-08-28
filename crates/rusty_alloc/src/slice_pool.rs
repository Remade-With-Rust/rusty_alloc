//! Free-slice pool for platforms where memory can never return to the host
//! (F2 of docs/plans/segment-tax.md).
//!
//! On wasm, `prim::free` is a no-op and linear memory only grows, so the
//! allocator's only recycling is what it keeps for itself. v1.1.5's
//! adopt-on-free arenas recycled at SEGMENT_SIZE (32 MiB) granularity, which
//! left the segment tax standing: every huge reservation was rounded up to
//! whole 32 MiB chunks, and a 33 MiB block permanently cost 64 MiB. The
//! rounding existed because every segment base had to be SEGMENT_SIZE-aligned
//! for `segment_of`'s pointer mask — and once wasm's `segment_of` resolves
//! through the slice-granular base table instead (`segment_map::base_of`),
//! that constraint is gone and this pool can hand memory around at 64 KiB
//! slices.
//!
//! The pool is a bitmap over the wasm address space: one bit per
//! `SEGMENT_SLICE_SIZE` slice, set = the slice is free and pool-owned.
//! `alloc_run` is first-fit; freeing sets bits, so adjacent ranges coalesce
//! by construction. wasm32 addresses are < 4 GiB, so the whole map is
//! 65,536 bits = 8 KiB.
//!
//! Bookkeeping only: nothing here dereferences the memory it tracks, which
//! is also why the module compiles and unit-tests on every target even
//! though only the wasm segment paths are wired to it. Single-threaded by
//! platform (wasm32 without the threads proposal — the standing assumption
//! of `prim/wasm.rs`); the atomics are for `static` soundness, not for
//! concurrency, and are `Relaxed` throughout.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::types::SEGMENT_SLICE_SIZE;

const SLICE_SHIFT: usize = 16;
const _: () = assert!(1 << SLICE_SHIFT == SEGMENT_SLICE_SIZE);
/// 4 GiB of address space in 64 KiB slices.
const SLOTS: usize = 1 << (32 - SLICE_SHIFT);
const WORDS: usize = SLOTS / 64;

static FREE: [AtomicU64; WORDS] = [const { AtomicU64::new(0) }; WORDS];

#[inline]
fn bit(idx: usize) -> (usize, u64) {
    (idx / 64, 1u64 << (idx % 64))
}

/// Return `[base, base + size)` to the pool. `false` (and no state change)
/// when the range is not slice-granular or not addressable by the map —
/// the caller falls through to `os::free`, exactly as adoption does.
///
/// Slice 0 is never accepted: address 0 is the null page and the module's
/// own data lives in the low slices, so a zero base is a caller bug, and
/// refusing it keeps `alloc_run`'s `base == 0` distinct from "no run".
pub fn free_range(base: usize, size: usize) -> bool {
    if base < SEGMENT_SLICE_SIZE
        || size == 0
        || !base.is_multiple_of(SEGMENT_SLICE_SIZE)
        || !size.is_multiple_of(SEGMENT_SLICE_SIZE)
    {
        return false;
    }
    let start = base >> SLICE_SHIFT;
    let n = size >> SLICE_SHIFT;
    let Some(end) = start.checked_add(n) else {
        return false;
    };
    if end > SLOTS {
        return false;
    }
    for idx in start..end {
        let (w, b) = bit(idx);
        let prev = FREE[w].fetch_or(b, Ordering::Relaxed);
        debug_assert_eq!(prev & b, 0, "slice pool: double free of slice {idx}");
    }
    true
}

/// First-fit run of `slices` free slices. Returns the run's base address and
/// clears its bits, or `None` — the caller reserves fresh memory instead.
pub fn alloc_run(slices: usize) -> Option<usize> {
    if slices == 0 || slices > SLOTS {
        return None;
    }
    let mut run = 0usize;
    let mut idx = 0usize;
    while idx < SLOTS {
        let (w, _) = bit(idx);
        let word = FREE[w].load(Ordering::Relaxed);
        if word == 0 && idx.is_multiple_of(64) {
            // Whole word empty: skip it. Resetting the run is correct, not
            // merely convenient — a run cannot cross a zero word.
            run = 0;
            idx += 64;
            continue;
        }
        if word & (1 << (idx % 64)) != 0 {
            run += 1;
            if run == slices {
                let start = idx + 1 - slices;
                for j in start..=idx {
                    let (jw, jb) = bit(j);
                    FREE[jw].fetch_and(!jb, Ordering::Relaxed);
                }
                return Some(start << SLICE_SHIFT);
            }
        } else {
            run = 0;
        }
        idx += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One lock, and a drain discipline: `alloc_run` is GLOBAL first-fit, so
    /// a test that leaves bits set hands the lowest-address run to whichever
    /// test allocates next — the same cross-test aliasing the adoption tests
    /// hit through VirtualAlloc adjacency. Every test therefore runs under
    /// the lock and exits with its window drained (all its freed slices
    /// allocated back out).
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    const MIB: usize = 1024 * 1024;

    #[test]
    fn round_trips_and_coalesces() {
        let _g = lock();
        let base = 256 * MIB;
        assert!(free_range(base, 2 * MIB));
        assert!(free_range(base + 2 * MIB, MIB)); // adjacent: coalesces by construction
        // 3 MiB = 48 slices, spanning the two freed ranges as one run.
        assert_eq!(alloc_run(48), Some(base), "coalesced run");
        // Pool drained: the same run is not served twice.
        assert!(alloc_run(1).is_none());
        assert!(free_range(base, 3 * MIB));
        assert_eq!(alloc_run(48), Some(base));
    }

    #[test]
    fn first_fit_skips_too_small_holes() {
        let _g = lock();
        let base = 512 * MIB;
        assert!(free_range(base, MIB)); // 16 slices
        assert!(free_range(base + 8 * MIB, 4 * MIB)); // 64 slices, disjoint
        assert_eq!(
            alloc_run(32),
            Some(base + 8 * MIB),
            "a 32-slice run must skip the 16-slice hole"
        );
        // The small hole is intact; drain everything on the way out.
        assert_eq!(alloc_run(16), Some(base));
        assert_eq!(alloc_run(32), Some(base + 10 * MIB));
        assert!(alloc_run(1).is_none());
    }

    #[test]
    fn runs_cross_word_boundaries() {
        let _g = lock();
        // Slice index 1000..1100 straddles the u64 word boundary at 1024.
        let base = 1000 * 64 * 1024;
        assert!(free_range(base, 100 * 64 * 1024));
        assert_eq!(alloc_run(100), Some(base));
        assert!(alloc_run(1).is_none());
    }

    #[test]
    fn rejects_what_it_cannot_track() {
        let _g = lock();
        assert!(!free_range(0, MIB), "slice 0 must be refused");
        assert!(!free_range(64 * 1024, 0), "empty range");
        assert!(!free_range(MIB + 4096, MIB), "misaligned base");
        assert!(!free_range(MIB, MIB + 4096), "ragged size");
        assert!(!free_range(usize::MAX - MIB, 2 * MIB), "unaddressable");
        assert!(alloc_run(0).is_none());
        assert!(alloc_run(SLOTS + 1).is_none());
    }
}
