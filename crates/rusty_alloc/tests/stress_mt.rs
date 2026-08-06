//! Targeted reproducer for the M8 P0: hammer the abandon → adopt → segment
//! reuse path (the one the whole-suite AV pointed at) inside ONE process, so a
//! defect that took ~10 suite runs shows up in seconds.
//!
//! Shape, all at once and repeatedly:
//! - short-lived threads that EXIT with live blocks (forces abandonment)
//! - other threads allocating hard (forces adoption + segment/arena reuse)
//! - cross-thread frees (the xthread/delayed protocol)
//! - huge allocations (multi-chunk arena claims racing single-chunk ones)
//! - explicit collects (retire + purge paths)
//!
//! Run alone: cargo test -p rusty_alloc --test stress_mt -- --nocapture

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use rusty_alloc::alloc::{collect, free, malloc, stats};

fn churn(seed: u64, ops: usize, sink: Option<&mpsc::Sender<usize>>) {
    let mut s = seed | 1;
    let mut rng = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let mut live: Vec<(*mut u8, usize)> = Vec::new();
    for i in 0..ops {
        let r = rng() % 100;
        if !live.is_empty() && (live.len() > 300 || r < 40) {
            let idx = (rng() as usize) % live.len();
            let (p, size) = live.swap_remove(idx);
            // SAFETY: tracked live block, freed exactly once (here or by the
            // receiving thread when bled).
            unsafe {
                assert_eq!(p.read(), 0x5A, "canary lost before free");
                if let Some(tx) = sink
                    && rng() % 4 == 0
                {
                    let _ = tx.send(p as usize); // bleed: freed remotely
                    continue;
                }
                let _ = size;
                free(p);
            }
        } else {
            // Mix: small, medium, large span, and huge (multi-chunk arena).
            let size = match rng() % 1000 {
                0..=850 => 8 + (rng() as usize % 500),
                851..=960 => 1024 + (rng() as usize % 60_000),
                961..=995 => 100_000 + (rng() as usize % 2_000_000),
                _ => 17 * 1024 * 1024 + (rng() as usize % (8 * 1024 * 1024)),
            };
            let p = malloc(size);
            assert!(!p.is_null(), "OOM at op {i}");
            // SAFETY: fresh block of >= size bytes; touch both ends so any
            // decommitted or aliased page faults HERE, next to its cause.
            unsafe {
                p.write(0x5A);
                p.add(size - 1).write(0xA5);
            }
            live.push((p, size));
        }
        if i % 5000 == 4999 {
            collect(true);
        }
    }
    for (p, _) in live.drain(..) {
        // SAFETY: tracked live blocks.
        unsafe { free(p) };
    }
}

#[test]
fn abandon_adopt_reuse_storm() {
    let rounds = if cfg!(miri) { 1 } else { 40 };
    let stop = Arc::new(AtomicBool::new(false));
    let leaked = Arc::new(AtomicUsize::new(0));

    // Background adopters: allocate continuously so abandoned segments get
    // picked up (and arena chunks recycled) while producers come and go.
    let adopters: Vec<_> = (0..3)
        .map(|k| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    churn(0xA0000 + k * 7 + n, 3000, None);
                    n += 1;
                }
            })
        })
        .collect();

    for round in 0..rounds {
        // Threads that DIE holding live blocks → abandonment.
        let (tx, rx) = mpsc::channel::<usize>();
        let dying: Vec<_> = (0..4)
            .map(|t| {
                let tx = tx.clone();
                let leaked = leaked.clone();
                std::thread::spawn(move || {
                    churn(0xBEEF + round * 31 + t, 2000, Some(&tx));
                    // Keep a few blocks alive across thread death.
                    let mut kept = Vec::new();
                    for j in 0..8 {
                        let size = 1000 + j * 4096;
                        let p = malloc(size);
                        // SAFETY: fresh block.
                        unsafe { core::ptr::write_bytes(p, 0x77, size) };
                        kept.push((p as usize, size));
                    }
                    leaked.fetch_add(kept.len(), Ordering::Relaxed);
                    kept
                })
            })
            .collect();
        drop(tx);

        // Free the bled blocks remotely while the producers still run.
        let mut bled = 0usize;
        for addr in rx.iter() {
            // SAFETY: sent by its allocating thread, freed once here.
            unsafe { free(addr as *mut u8) };
            bled += 1;
        }

        // Collect the survivors and free them from THIS thread (blocks whose
        // owner is dead: the NEVER path + adopted segments).
        for h in dying {
            for (addr, size) in h.join().unwrap() {
                let p = addr as *mut u8;
                // SAFETY: block outlived its allocating thread by design.
                unsafe {
                    assert_eq!(p.read(), 0x77, "abandoned block corrupted");
                    assert_eq!(p.add(size - 1).read(), 0x77, "abandoned tail corrupted");
                    free(p);
                }
            }
        }
        assert!(bled > 0, "round {round}: no cross-thread frees happened");
        collect(true);
    }

    stop.store(true, Ordering::Relaxed);
    for a in adopters {
        a.join().unwrap();
    }
    let s = stats();
    println!(
        "storm done: rounds={rounds} leaked-then-freed={} allocs={} reclaims={} segs={}(-{})",
        leaked.load(Ordering::Relaxed),
        s.allocs,
        s.reclaims,
        s.segments,
        s.segments_freed
    );
}
