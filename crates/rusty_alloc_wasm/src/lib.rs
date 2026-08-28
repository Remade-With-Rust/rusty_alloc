//! wasm32 self-test — the allocator running inside a WebAssembly VM.
//!
//! `cargo test` cannot execute `wasm32-unknown-unknown`, so the checks live in
//! an exported function that a host (Node, via `bench/wasm-selftest.mjs`)
//! instantiates and calls. Each check returns a DISTINCT non-zero code so a
//! failure names itself instead of just saying "wasm broke".
//!
//! The same function is compiled for the host too, so `cargo test` runs it
//! natively and any failure that is NOT wasm-specific is caught by the normal
//! gates rather than only by the wasm runner.

use rusty_alloc::alloc::{free, malloc, realloc, usable_size, zalloc};

/// Result codes. 0 = success; every other value names the failing check.
pub const OK: i32 = 0;
pub const E_MALLOC_NULL: i32 = 1;
pub const E_SMALL_PATTERN: i32 = 2;
pub const E_USABLE_TOO_SMALL: i32 = 3;
pub const E_ZALLOC_NOT_ZERO: i32 = 4;
pub const E_REALLOC_NULL: i32 = 5;
pub const E_REALLOC_LOST_DATA: i32 = 6;
pub const E_LARGE_NULL: i32 = 7;
pub const E_LARGE_PATTERN: i32 = 8;
pub const E_RECYCLE_NULL: i32 = 9;
pub const E_ALIGN: i32 = 10;
pub const E_STEADY_NULL: i32 = 11;
pub const E_STEADY_LEAK: i32 = 12;

/// Sizes chosen to cross bin boundaries and the small/medium/large thresholds
/// without demanding enough linear memory to make a wasm host unhappy.
const SIZES: [usize; 12] = [1, 8, 15, 16, 24, 64, 100, 512, 1024, 4096, 20000, 100_000];

/// Run the allocator through its paces. Returns [`OK`] or the first failure.
///
/// # Safety
/// None for callers — this owns every pointer it creates and frees them all.
pub fn selftest() -> i32 {
    /// Large-block size: above the binned cutoff, so this exercises the
    /// span path rather than a bin.
    const BIG: usize = 600_000;

    // 1. Small/medium allocations across bins: write a size-derived pattern,
    //    read it back, and check the usable extent is at least what we asked.
    let mut live: [*mut u8; SIZES.len()] = [core::ptr::null_mut(); SIZES.len()];
    for (i, &n) in SIZES.iter().enumerate() {
        let p = malloc(n);
        if p.is_null() {
            return E_MALLOC_NULL;
        }
        // SAFETY: p is a live block from this allocator.
        if unsafe { usable_size(p) } < n {
            // SAFETY: allocated above, freed once.
            unsafe { free(p) };
            return E_USABLE_TOO_SMALL;
        }
        // SAFETY: p owns n bytes we just allocated.
        unsafe { core::ptr::write_bytes(p, (i as u8).wrapping_add(1), n) };
        live[i] = p;
    }
    // Verify only AFTER every allocation, so an overlap between two live
    // blocks is caught rather than being overwritten by the next write.
    for (i, &n) in SIZES.iter().enumerate() {
        let p = live[i];
        for k in 0..n {
            // SAFETY: within the block we allocated above.
            if unsafe { *p.add(k) } != (i as u8).wrapping_add(1) {
                return E_SMALL_PATTERN;
            }
        }
    }
    for &p in live.iter() {
        // SAFETY: allocated above, freed exactly once.
        unsafe { free(p) };
    }

    // 2. Zeroed allocation must be zero across the whole requested extent.
    for &n in &[8usize, 100, 4096, 33_000] {
        let p = zalloc(n);
        if p.is_null() {
            return E_MALLOC_NULL;
        }
        for k in 0..n {
            // SAFETY: within the zeroed block.
            if unsafe { *p.add(k) } != 0 {
                // SAFETY: allocated above.
                unsafe { free(p) };
                return E_ZALLOC_NOT_ZERO;
            }
        }
        // SAFETY: allocated above, freed once.
        unsafe { free(p) };
    }

    // 3. realloc must preserve contents across a growth that moves the block.
    let p = malloc(64);
    if p.is_null() {
        return E_MALLOC_NULL;
    }
    // SAFETY: 64 bytes we own.
    unsafe { core::ptr::write_bytes(p, 0xA5, 64) };
    // SAFETY: p came from malloc and is not used after this call.
    let q = unsafe { realloc(p, 9000) };
    if q.is_null() {
        return E_REALLOC_NULL;
    }
    for k in 0..64 {
        // SAFETY: the preserved prefix of the reallocated block.
        if unsafe { *q.add(k) } != 0xA5 {
            // SAFETY: q is live.
            unsafe { free(q) };
            return E_REALLOC_LOST_DATA;
        }
    }
    // SAFETY: q is live and freed once.
    unsafe { free(q) };

    // 4. A large block — crosses out of the binned path into a span.
    let b = malloc(BIG);
    if b.is_null() {
        return E_LARGE_NULL;
    }
    // SAFETY: BIG bytes we own.
    unsafe { core::ptr::write_bytes(b, 0x5A, BIG) };
    // SAFETY: within the block; check the ends and the middle.
    unsafe {
        if *b != 0x5A || *b.add(BIG / 2) != 0x5A || *b.add(BIG - 1) != 0x5A {
            free(b);
            return E_LARGE_PATTERN;
        }
    }
    // SAFETY: allocated above, freed once.
    unsafe { free(b) };

    // 5. Churn: allocate and free repeatedly so pages retire and are reused.
    //    On wasm this is the path that MATTERS - `free` never returns memory
    //    to the host, so if our own segment/page recycling did not work the
    //    linear memory would grow without bound.
    for round in 0..200 {
        let mut batch: [*mut u8; 32] = [core::ptr::null_mut(); 32];
        for (i, slot) in batch.iter_mut().enumerate() {
            let n = 16 + ((i * 37 + round) % 3000);
            let p = malloc(n);
            if p.is_null() {
                return E_RECYCLE_NULL;
            }
            // SAFETY: n bytes we own.
            unsafe { core::ptr::write_bytes(p, 0x33, n) };
            *slot = p;
        }
        for &p in batch.iter() {
            // SAFETY: allocated this round, freed once.
            unsafe { free(p) };
        }
    }

    // 6. STEADY STATE: N identical alloc/free cycles must stop growing
    //    linear memory after the first. This is the arm whose absence let
    //    v1.1.4 ship a leak of one whole 32 MiB segment per large cycle: the
    //    churn loop above never empties a segment, and the measurement that
    //    justified disabling arenas on wasm ran a short workload that never
    //    reached the steady-state regime. Three shapes, chosen to pin the
    //    routes: 4 MiB (a medium page inside a shared segment), 20 MiB
    //    (> LARGE_OBJ_SIZE_MAX = 16 MiB, the dedicated huge route — the shape
    //    that leaked +640 MiB over 20 cycles), and 33 MiB (a huge block whose
    //    ragged size exercises the chunk-granular rounding; without it the
    //    sub-chunk tail leaks per cycle even with adoption working).
    for &(size, cycles) in &[(4usize << 20, 20usize), (20 << 20, 20), (33 << 20, 12)] {
        #[cfg(target_arch = "wasm32")]
        let mut after_first = 0usize;
        // `cycle` is read only by the wasm-gated measurement below.
        #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
        for cycle in 0..cycles {
            let p = malloc(size);
            if p.is_null() {
                return E_STEADY_NULL;
            }
            // Touch first/middle/last so every cycle commits real memory.
            // SAFETY: size bytes we own.
            unsafe {
                *p = 0x7E;
                *p.add(size / 2) = 0x7E;
                *p.add(size - 1) = 0x7E;
                free(p);
            }
            #[cfg(target_arch = "wasm32")]
            {
                let now = core::arch::wasm32::memory_size::<0>() * 65536;
                if cycle == 0 {
                    after_first = now;
                } else if now > after_first {
                    return E_STEADY_LEAK;
                }
            }
        }
    }

    // 7. Every block must be at least word-aligned.
    for &n in &SIZES {
        let p = malloc(n);
        if p.is_null() {
            return E_MALLOC_NULL;
        }
        let misaligned = !(p as usize).is_multiple_of(core::mem::size_of::<usize>());
        // SAFETY: allocated above, freed once.
        unsafe { free(p) };
        if misaligned {
            return E_ALIGN;
        }
    }

    OK
}

/// C-ABI entry point for the wasm host.
///
/// # Safety
/// Safe to call; exported as `extern "C"` only so a WebAssembly host can reach
/// it without a bindings layer.
#[unsafe(no_mangle)]
pub extern "C" fn ra_selftest() -> i32 {
    selftest()
}

/// Current size of the wasm linear memory in bytes, so the host can report how
/// much the allocator actually asked the VM for.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn ra_memory_bytes() -> u32 {
    (core::arch::wasm32::memory_size::<0>() * 65536) as u32
}

#[cfg(test)]
mod tests {
    // The same checks run natively, so a non-wasm-specific break is caught by
    // the ordinary gates instead of only by the wasm runner.
    #[test]
    fn selftest_passes_natively() {
        assert_eq!(super::selftest(), super::OK);
    }
}
