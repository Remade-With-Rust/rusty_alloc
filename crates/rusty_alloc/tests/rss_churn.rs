//! Localise the RSS gap: does an allocate/free CYCLE churn spans?
//!
//! Measured externally (FFAI, and reproduced by `bench/rss.sh`): rusty_alloc
//! peaks ~18% above mimalloc on workloads that build a large population, drop
//! it, and build another — while sitting at parity on grow-only workloads, and
//! at exact parity on speed. Purging was the first hypothesis and was REFUTED
//! (it recovers ~2 MiB of ~10).
//!
//! If freed memory is not being HELD (purging would have reclaimed it) then it
//! is not being REUSED: each round must be touching fresh pages. This prints
//! the counters that would show that — pages carved fresh versus pages retired,
//! and segments taken from the OS — across repeated identical rounds.
//!
//! Run with: cargo test -p rusty_alloc --test rss_churn -- --nocapture

use rusty_alloc::alloc::{free, malloc, stats};

const ROUND: usize = 20_000;
const ROUNDS: usize = 6;

#[test]
fn cycling_rounds_should_not_carve_fresh_pages_every_round() {
    let mut live: Vec<*mut u8> = Vec::with_capacity(ROUND);

    // Round 0 establishes the working set; later rounds should REUSE it.
    let mut prev_fresh = 0u64;
    let mut prev_segments = 0u64;

    println!(
        "{:>5}  {:>12} {:>12} {:>12} {:>10}",
        "round", "pages_fresh", "d_fresh", "d_segments", "retired"
    );

    for r in 0..ROUNDS {
        for i in 0..ROUND {
            // Mixed sizes across bins, same distribution every round.
            let n = 16 + ((i * 37) % 3000);
            let p = malloc(n);
            assert!(!p.is_null(), "round {r}: malloc failed");
            // SAFETY: n bytes we just allocated.
            unsafe { core::ptr::write_bytes(p, 0xA5, n) };
            live.push(p);
        }
        for p in live.drain(..) {
            // SAFETY: allocated above, freed exactly once.
            unsafe { free(p) };
        }

        let s = stats();
        println!(
            "{r:>5}  {:>12} {:>12} {:>12} {:>10}",
            s.pages_fresh,
            s.pages_fresh - prev_fresh,
            s.segments - prev_segments,
            s.pages_retired
        );
        prev_fresh = s.pages_fresh;
        prev_segments = s.segments;
    }

    let s = stats();
    println!(
        "\ntotals: segments={} pages_fresh={} pages_retired={} purges={} segments_freed={}",
        s.segments, s.pages_fresh, s.pages_retired, s.purges, s.segments_freed
    );

    // The diagnostic claim: after the first round the working set exists, so
    // later rounds should carve FEW fresh pages. If d_fresh stays high every
    // round, we are re-carving instead of reusing — which is the RSS gap.
    println!(
        "\nIf `d_fresh` stays high after round 0, spans are being re-carved \
         rather than reused - that is the RSS gap."
    );
}
