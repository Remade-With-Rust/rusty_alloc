//! H-26 target 2: the CROSS-THREAD free path — the lock-free four-state
//! protocol plus thread teardown (abandon → adopt). Each iteration spawns
//! producer work on the main thread and frees a fuzzer-chosen split of the
//! blocks on short-lived worker threads, so remote frees, delayed lists,
//! thread exits with live blocks, and adoption by later allocations all get
//! exercised under libFuzzer+ASan. Blocks carry a fill byte verified on the
//! freeing thread — corruption across the handoff is caught, not just crashes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rusty_alloc::alloc;

const MAX_BLOCKS: usize = 96;
const MAX_THREADS: usize = 4;

struct Handoff(Vec<(usize, usize, u8)>); // addr, usable, fill — Send by construction
// SAFETY: the addresses are live blocks handed off with unique ownership —
// exactly the cross-thread free contract the allocator documents; nothing
// else retains or touches them after the send.
unsafe impl Send for Handoff {}

fuzz_target!(|data: &[u8]| {
    let mut it = data.iter().copied();
    let nthreads = 1 + (it.next().unwrap_or(0) as usize % MAX_THREADS);
    let mut batches: Vec<Vec<(usize, usize, u8)>> = (0..nthreads).map(|_| Vec::new()).collect();
    let mut fill: u8 = 0x11;

    // Producer (main thread): allocate and route each block to a fuzzer-chosen
    // freeing thread; some stay local (index 0 frees on this thread).
    while let Some(op) = it.next() {
        let size = 1 + (((op as usize) << 6) | (it.next().unwrap_or(0) as usize % 64)) % 8192;
        if batches.iter().map(Vec::len).sum::<usize>() >= MAX_BLOCKS {
            break;
        }
        let p = alloc::malloc(size);
        if p.is_null() {
            continue;
        }
        // SAFETY: live block, filled across its usable extent.
        let usable = unsafe { alloc::usable_size(p) };
        fill = fill.wrapping_add(3);
        unsafe { core::ptr::write_bytes(p, fill, usable) };
        let dest = it.next().unwrap_or(0) as usize % nthreads;
        // expose_provenance (not bare addr): the freeing thread reconstructs
        // the pointer with with_exposed_provenance_mut.
        batches[dest].push((p.expose_provenance(), usable, fill));
    }

    let mut local = batches.remove(0);
    let handles: Vec<_> = batches
        .into_iter()
        .map(Handoff)
        .map(|h| {
            std::thread::spawn(move || {
                for (addr, usable, fill) in h.0 {
                    let p = core::ptr::with_exposed_provenance_mut::<u8>(addr);
                    // SAFETY: uniquely-owned live block received from the
                    // producer; verified then freed exactly once, remotely.
                    unsafe {
                        for off in [0, usable / 2, usable - 1] {
                            assert_eq!(*p.add(off), fill, "cross-thread corruption");
                        }
                        alloc::free(p);
                    }
                }
                // The thread exits here: teardown abandons its heap; a later
                // iteration's allocations adopt the orphaned segments.
            })
        })
        .collect();

    // Interleave local frees with the remote ones.
    for (addr, usable, fill) in local.drain(..) {
        let p = core::ptr::with_exposed_provenance_mut::<u8>(addr);
        // SAFETY: uniquely-owned live block, freed exactly once, locally.
        unsafe {
            for off in [0, usable / 2, usable - 1] {
                assert_eq!(*p.add(off), fill, "local corruption");
            }
            alloc::free(p);
        }
    }
    for h in handles {
        h.join().expect("worker panicked");
    }
});
