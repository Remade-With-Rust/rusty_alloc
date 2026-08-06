//! Do ABANDONED segments accumulate under thread churn?
//!
//! FFAI measured, N=5, one program, trim as the only variable:
//!
//!   arm            RSS med   RSS min   RSS max   spread
//!   mimalloc         111.1     106.7     134.2    1.26x
//!   rusty_alloc      195.8      92.2     403.3    4.40x
//!
//! Our MINIMUM beats mimalloc's minimum. That rules out a retention-policy
//! difference, which would shift the whole distribution rather than stretch
//! it. A 92->403 MB spread is roughly ten 32 MiB segments, so something
//! timing-dependent decides how many segments the process holds.
//!
//! Prime suspect: thread exit abandons a segment, and reclaim is
//! ADOPT-ON-DEMAND — nothing adopts unless some thread needs memory. Under a
//! churning pool (candle's), orphans could pile up 32 MiB at a time, and
//! whether they get adopted would depend on scheduling, which is exactly the
//! shape of a wide run-to-run spread. It also explains why trim reclaims 0%:
//! trim walks a heap's OWN free spans and never touches the abandoned list.
//!
//! Run: cargo test -p rusty_alloc --test abandon_rss -- --nocapture

use rusty_alloc::alloc::{free, malloc};
use std::sync::atomic::Ordering;

const WAVES: usize = 8;
const PER_WAVE: usize = 8;
const BLOCKS: usize = 512;

/// Allocate a working set big enough to need real spans, then exit the thread
/// WITHOUT the process draining — that is what abandons the segment.
fn churn_thread() {
    let mut v = Vec::with_capacity(BLOCKS);
    for i in 0..BLOCKS {
        let n = 4096 + (i * 997) % 262_144; // spans, not just binned blocks
        let p = malloc(n);
        assert!(!p.is_null());
        // SAFETY: n bytes we own; touch so the pages are real.
        unsafe { core::ptr::write_bytes(p, 0x5A, n) };
        v.push((p, n));
    }
    // Free MOST but not all: a segment with live blocks cannot retire, so it
    // must be abandoned at thread exit rather than freed.
    for (p, _) in v.drain(..BLOCKS - 8) {
        // SAFETY: allocated above, freed once.
        unsafe { free(p) };
    }
    // The remaining 8 leak deliberately: they are what forces abandonment.
    core::mem::forget(v);
}

// Miri-ignored: this probe LEAKS ON PURPOSE — a segment with live blocks
// cannot retire, which is the only way to force abandonment — so Miri's leak
// checker correctly objects. It is a diagnostic, not a correctness test.
#[cfg_attr(miri, ignore)]
#[test]
fn abandoned_segments_should_not_accumulate_across_thread_waves() {
    println!("{:>5}  {:>18}", "wave", "abandoned_segments");
    let mut peak = 0usize;
    for w in 0..WAVES {
        let hs: Vec<_> = (0..PER_WAVE)
            .map(|_| std::thread::spawn(churn_thread))
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        let ab = rusty_alloc::init::ABANDONED_COUNT.load(Ordering::Relaxed);
        peak = peak.max(ab);
        println!("{w:>5}  {ab:>18}");
    }

    // Allocating afterwards should ADOPT the orphans rather than take fresh
    // segments from the OS. If the count does not fall, adopt-on-demand is not
    // reclaiming and every orphan is 32 MiB of resident memory.
    let mut keep = Vec::new();
    for i in 0..2048 {
        let p = malloc(4096 + (i * 131) % 65536);
        assert!(!p.is_null());
        keep.push(p);
    }
    let after = rusty_alloc::init::ABANDONED_COUNT.load(Ordering::Relaxed);
    for p in keep {
        // SAFETY: allocated just above, freed once.
        unsafe { free(p) };
    }

    println!("\npeak abandoned = {peak}, after a fresh allocation burst = {after}");
    println!(
        "Each abandoned segment is 32 MiB of address space. If `after` stays \
         near `peak`, adoption is not reclaiming them and that is the RSS tail."
    );
}
