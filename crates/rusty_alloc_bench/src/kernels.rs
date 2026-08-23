//! Tier-B kernels (plan §7.2) + the R1 TLS spike. Every timed output prints a
//! METHOD line (codec-measurement §13); in-process wall time here is the
//! *inner-loop* instrument — keep/revert verdicts route through pinvs.ps1
//! (pinned, CPU-time, ABBA) driving this binary as its arms.

use std::hint::black_box;
use std::time::Instant;

use crate::replay::{Arm, Rng};

fn arm_alloc(arm: Arm, size: usize) -> *mut u8 {
    match arm {
        Arm::Rusty => rusty_alloc::alloc::malloc(size),
        Arm::System => {
            let l = std::alloc::Layout::from_size_align(size.max(1), 8).unwrap();
            // SAFETY: valid layout.
            unsafe { std::alloc::alloc(l) }
        }
    }
}

/// # Safety
/// `p` from `arm_alloc(arm, size)`, freed once.
unsafe fn arm_free(arm: Arm, p: *mut u8, size: usize) {
    match arm {
        // SAFETY: forwarded.
        Arm::Rusty => unsafe { rusty_alloc::alloc::free(p) },
        Arm::System => {
            let l = std::alloc::Layout::from_size_align(size.max(1), 8).unwrap();
            // SAFETY: forwarded; same layout.
            unsafe { std::alloc::dealloc(p, l) }
        }
    }
}

/// malloc-small: Pareto-ish small sizes, bounded live set, LIFO-heavy churn —
/// the cfrac/alloc-test inner pattern. Prints ops/s + method line + counters.
pub fn malloc_small(arm: Arm, ops: u64, seed: u64) {
    let mut rng = Rng(seed | 1);
    let mut live: Vec<(*mut u8, usize)> = Vec::with_capacity(4096);
    let stats_before = rusty_alloc::alloc::stats();
    let t0 = Instant::now();
    for _ in 0..ops {
        if !live.is_empty() && (live.len() >= 4096 || rng.below(100) < 48) {
            let idx = if rng.below(4) == 0 {
                rng.below(live.len())
            } else {
                live.len() - 1
            };
            let (p, sz) = live.swap_remove(idx);
            // SAFETY: tracked live block.
            unsafe { arm_free(arm, p, sz) };
        } else {
            // 80% ≤128B, else ≤1KiB — the small-page regime.
            let size = if rng.below(10) < 8 {
                8 + rng.below(120)
            } else {
                128 + rng.below(896)
            };
            let p = arm_alloc(arm, size);
            assert!(!p.is_null());
            // Touch the block so lazy commit can't fake speed.
            // SAFETY: p is a live block of ≥ size ≥ 8 bytes.
            unsafe { p.cast::<u64>().write(0xDEAD_BEEF) };
            live.push((black_box(p), size));
        }
    }
    let dt = t0.elapsed();
    for (p, sz) in live.drain(..) {
        // SAFETY: tracked live blocks.
        unsafe { arm_free(arm, p, sz) };
    }
    let name = match arm {
        Arm::Rusty => "ra",
        Arm::System => "sys",
    };
    // `as_secs_f64` is itself a division, and it was called twice; `/ secs
    // / 1e6` was a second `divsd` where one suffices.
    let secs = dt.as_secs_f64();
    let mops = ops as f64 / (secs * 1e6);
    println!("malloc-small arm={name} ops={ops} time={secs:.3}s throughput={mops:.2} Mops/s");
    println!(
        "METHOD: in-process wall (inner-loop instrument), single thread, seed={seed}, live-set<=4096, touch-1-word; verdicts require pinvs.ps1 (pinned CPU-time ABBA)"
    );
    if arm == Arm::Rusty {
        let s = rusty_alloc::alloc::stats();
        println!(
            "COUNTERS: allocs={} frees={} generic={} pages_fresh={} segments={} extends={} (deltas this run)",
            s.allocs - stats_before.allocs,
            s.frees - stats_before.frees,
            s.generic - stats_before.generic,
            s.pages_fresh - stats_before.pages_fresh,
            s.segments - stats_before.segments,
            s.extends - stats_before.extends,
        );
    }
}

/// larson: the classic server simulation — N worker threads each churn a slot
/// array of live blocks; a fraction of blocks "bleed" to the next thread via
/// channels and are freed there (the cross-thread free path under load).
pub fn larson(arm: Arm, threads: usize, ops_per_thread: u64, seed: u64) {
    use std::sync::mpsc;
    let t0 = Instant::now();
    let mut txs = Vec::new();
    let mut rxs = Vec::new();
    for _ in 0..threads {
        let (tx, rx) = mpsc::channel::<(usize, usize)>(); // (addr, size)
        txs.push(tx);
        rxs.push(Some(rx));
    }
    let handles: Vec<_> = (0..threads)
        .map(|ti| {
            let rx = rxs[ti].take().unwrap();
            let tx_next = txs[(ti + 1) % threads].clone();
            std::thread::spawn(move || {
                let mut rng = Rng((seed ^ (ti as u64) << 32) | 1);
                let mut slots: Vec<(*mut u8, usize)> = Vec::with_capacity(1024);
                let mut freed_remote = 0u64;
                for _ in 0..ops_per_thread {
                    // Drain bled-in blocks (freeing REMOTE memory).
                    while let Ok((addr, size)) = rx.try_recv() {
                        let p = addr as *mut u8;
                        // SAFETY: block sent by its allocating thread, freed once.
                        unsafe {
                            assert_eq!(p.read(), 0xB1);
                            arm_free(arm, p, size);
                        }
                        freed_remote += 1;
                    }
                    if slots.len() >= 1024 || (!slots.is_empty() && rng.below(100) < 47) {
                        let idx = rng.below(slots.len());
                        let (p, size) = slots.swap_remove(idx);
                        if rng.below(100) < 10 {
                            // Bleed: pass to the next thread to free.
                            // SAFETY: we own p; receiver frees it exactly once.
                            let _ = tx_next.send((p as usize, size));
                        } else {
                            // SAFETY: tracked live block.
                            unsafe { arm_free(arm, p, size) };
                        }
                    } else {
                        let size = 8 + rng.below(992);
                        let p = arm_alloc(arm, size);
                        assert!(!p.is_null());
                        // SAFETY: fresh block ≥ 8 bytes.
                        unsafe { p.write(0xB1) };
                        slots.push((p, size));
                    }
                }
                drop(tx_next);
                // Local cleanup; late bleed-ins drain until senders hang up.
                for (p, size) in slots.drain(..) {
                    // SAFETY: tracked live blocks.
                    unsafe { arm_free(arm, p, size) };
                }
                while let Ok((addr, size)) = rx.recv() {
                    // SAFETY: as above.
                    unsafe { arm_free(arm, addr as *mut u8, size) };
                }
                freed_remote
            })
        })
        .collect();
    drop(txs);
    let remote_total: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    let dt = t0.elapsed();
    let total_ops = threads as u64 * ops_per_thread;
    let name = match arm {
        Arm::Rusty => "ra",
        Arm::System => "sys",
    };
    let secs = dt.as_secs_f64();
    println!(
        "larson arm={name} threads={threads} ops={total_ops} time={secs:.3}s throughput={:.2} Mops/s remote_frees={remote_total}",
        total_ops as f64 / (secs * 1e6)
    );
    println!(
        "METHOD: in-process wall, {threads} threads, 10% bleed via mpsc, live-set<=1024/thread, seed={seed}"
    );
}

/// xmalloc-style: dedicated allocator threads push blocks through channels to
/// dedicated freer threads — the pure producer/consumer pattern that thread
/// caches hate.
pub fn xmalloc(arm: Arm, pairs: usize, ops_per_pair: u64, seed: u64) {
    use std::sync::mpsc;
    let t0 = Instant::now();
    let handles: Vec<_> = (0..pairs)
        .map(|pi| {
            let (tx, rx) = mpsc::sync_channel::<(usize, usize)>(4096);
            let producer = std::thread::spawn(move || {
                let mut rng = Rng((seed ^ (pi as u64) << 24) | 1);
                for _ in 0..ops_per_pair {
                    let size = 8 + rng.below(504);
                    let p = arm_alloc(arm, size);
                    assert!(!p.is_null());
                    // SAFETY: fresh block.
                    unsafe { p.write(0xC2) };
                    tx.send((p as usize, size)).unwrap();
                }
            });
            let consumer = std::thread::spawn(move || {
                let mut n = 0u64;
                while let Ok((addr, size)) = rx.recv() {
                    let p = addr as *mut u8;
                    // SAFETY: producer sent a live block; freed once here.
                    unsafe {
                        assert_eq!(p.read(), 0xC2);
                        arm_free(arm, p, size);
                    }
                    n += 1;
                }
                n
            });
            (producer, consumer)
        })
        .collect();
    let mut freed = 0u64;
    for (p, c) in handles {
        p.join().unwrap();
        freed += c.join().unwrap();
    }
    let dt = t0.elapsed();
    let name = match arm {
        Arm::Rusty => "ra",
        Arm::System => "sys",
    };
    let secs = dt.as_secs_f64();
    println!(
        "xmalloc arm={name} pairs={pairs} blocks={freed} time={secs:.3}s throughput={:.2} Mops/s (every free is remote)",
        freed as f64 / (secs * 1e6)
    );
    println!(
        "METHOD: in-process wall, {pairs} producer/consumer pairs, sync_channel(4096), seed={seed}"
    );
}

/// M9 probe: price the components of the FREE fast path, which is where the
/// small-object interpreter workloads (lua/perl/cfrac) live. Ordered cheapest
/// hypothesis first; every number is ns/op so it can be compared against the
/// ~15–25 ns a whole malloc+free pair costs.
pub fn freepath_probe(iters: u64) {
    // Every arm below divides by the SAME `iters`. One reciprocal, then a
    // multiply per arm — `divsd` is 13-20 cycles and does not pipeline,
    // `mulsd` is 4 and does. The last-ulp difference is invisible at the
    // two decimals these print.
    let per_iter = 1.0 / iters as f64;
    // (a) the floor: an empty loop the optimizer cannot delete.
    let t0 = Instant::now();
    let mut acc = 0usize;
    for i in 0..iters {
        acc = acc.wrapping_add(black_box(i as usize));
    }
    let floor = t0.elapsed().as_nanos() as f64 * per_iter;
    black_box(acc);

    // (b) prim::thread_id() — called on EVERY free to decide local vs remote.
    // On unix this is pthread_self(); from a cdylib that is a PLT call, which
    // no amount of inlining removes.
    let t0 = Instant::now();
    let mut acc = 0usize;
    for _ in 0..iters {
        acc = acc.wrapping_add(black_box(rusty_alloc::prim::thread_id()));
    }
    let tid = t0.elapsed().as_nanos() as f64 * per_iter;
    black_box(acc);

    // (c) the candidate replacement: a const-init thread_local cache.
    std::thread_local! {
        static TID: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
    }
    TID.with(|c| c.set(rusty_alloc::prim::thread_id()));
    let t0 = Instant::now();
    let mut acc = 0usize;
    for _ in 0..iters {
        acc = acc.wrapping_add(black_box(TID.with(|c| c.get())));
    }
    let cached = t0.elapsed().as_nanos() as f64 * per_iter;
    black_box(acc);

    // (d) a real malloc+free pair of a small block, for scale.
    let t0 = Instant::now();
    for _ in 0..iters {
        let p = rusty_alloc::alloc::malloc(black_box(48));
        // SAFETY: fresh block, freed once.
        unsafe { rusty_alloc::alloc::free(p) };
    }
    let pair = t0.elapsed().as_nanos() as f64 * per_iter;

    println!("freepath-probe iters={iters}");
    println!("  (a) loop floor            : {floor:.2} ns/op");
    println!("  (b) prim::thread_id()     : {tid:.2} ns/op  [called once per free]");
    println!("  (c) thread_local cache    : {cached:.2} ns/op  [candidate]");
    println!("  (d) malloc+free pair (48B): {pair:.2} ns/op  [the thing we are trying to shrink]");
    println!(
        "  => thread_id is {:.1}% of a malloc+free pair; replacing it should save ~{:.2} ns/free",
        100.0 * (tid - floor) / pair,
        (tid - cached).max(0.0)
    );
    println!("METHOD: in-process wall, single thread, black_box per op; relative sizing only");
}

/// R1 TLS spike: price the candidate fast-path TLS access shapes (M4 design
/// input). Reports ns/access for: static atomic load (lower bound), Rust
/// `thread_local!` with Cell, and the prim TLS slot (Fls/pthread).
pub fn tls_spike(iters: u64) {
    // (a) static atomic — the no-TLS lower bound.
    static S: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    // Every arm below divides by the SAME `iters`. One reciprocal, then a
    // multiply per arm — `divsd` is 13-20 cycles and does not pipeline,
    // `mulsd` is 4 and does. The last-ulp difference is invisible at the
    // two decimals these print.
    let per_iter = 1.0 / iters as f64;
    let t0 = Instant::now();
    let mut acc = 0usize;
    for _ in 0..iters {
        acc = acc.wrapping_add(black_box(S.load(std::sync::atomic::Ordering::Relaxed)));
    }
    let a = t0.elapsed().as_nanos() as f64 * per_iter;
    black_box(acc);

    // (b) thread_local! Cell — the idiomatic candidate (lazy-init check inside).
    std::thread_local! {
        static TL: std::cell::Cell<usize> = const { std::cell::Cell::new(1) };
    }
    let t0 = Instant::now();
    let mut acc = 0usize;
    for _ in 0..iters {
        acc = acc.wrapping_add(black_box(TL.with(|c| c.get())));
    }
    let b = t0.elapsed().as_nanos() as f64 * per_iter;
    black_box(acc);

    // (c) prim TlsSlot (FlsGetValue / pthread_getspecific) — the C-shaped candidate.
    let slot = rusty_alloc::prim::TlsSlot::new(None).unwrap();
    slot.set(core::ptr::dangling_mut());
    let t0 = Instant::now();
    let mut acc = 0usize;
    for _ in 0..iters {
        acc = acc.wrapping_add(black_box(slot.get()) as usize);
    }
    let c = t0.elapsed().as_nanos() as f64 * per_iter;
    black_box(acc);

    println!("tls-spike iters={iters}");
    println!("  static-atomic-load : {a:.2} ns/access (lower bound)");
    println!("  thread_local!+Cell : {b:.2} ns/access");
    println!("  prim TlsSlot (OS)  : {c:.2} ns/access");
    println!(
        "METHOD: in-process wall, single thread, black_box on every access; relative ranking only — absolute ns needs pinning"
    );
}
