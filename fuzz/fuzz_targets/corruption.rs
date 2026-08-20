//! H-26 target 3: the free-list link check — the mitigation, not the workload.
//!
//! The other two targets fuzz VALID op sequences. This one is pointed at the
//! defence itself, and it is shaped around one awkward fact: **the correct
//! response to detected corruption is `abort`, and libFuzzer reports an abort
//! as a crash.** So the true-positive path — "poison a link, watch it die" —
//! cannot be fuzzed in-process at all; every input would be a finding. That
//! path is covered instead by `tests/corruption.rs`, which spawns a child per
//! scenario and asserts on the exit signal (SIGABRT, not SIGSEGV).
//!
//! What IS fuzzable is the other two thirds of the problem, and both matter:
//!
//! **Part 1 — the identity.** `link_is_plausible` decides "same segment" with
//! `(a ^ b) < SEGMENT_SIZE`, which is exact only because segments are
//! SEGMENT_SIZE-aligned and SEGMENT_SIZE is a power of two. That is the one
//! clever line in the check, it runs on the allocation hot path, and a subtle
//! error in it either lets an attacker through or aborts on valid links. So it
//! is checked differentially against the naive `a / SEGMENT_SIZE == b /
//! SEGMENT_SIZE` over fuzzer-chosen 128-bit input — a space no hand-written
//! table can cover.
//!
//! **Part 2 — false positives.** A bounds check added to a hot path carries
//! its own risk, and it is the more likely one: if the check ever rejects a
//! GENUINE link, the allocator aborts on a legitimate workload and we have
//! shipped a denial of service. There is no assertion for this and there does
//! not need to be — under `--features secure` a false positive IS an abort,
//! which is precisely what the fuzzer is watching for. Run it that way:
//!
//! ```text
//! cargo fuzz run corruption --features secure
//! ```
//!
//! Without `secure` part 2 still exercises the allocator but the check is not
//! compiled in, so only part 1 is meaningful.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rusty_alloc::alloc;
use rusty_alloc::page::link_is_plausible;
use rusty_alloc::types::SEGMENT_SIZE;

/// The naive statement of "same 32 MiB segment", written deliberately
/// differently from the implementation (`(a ^ b) < SEGMENT_SIZE`) so that
/// agreement between them means something.
fn same_segment_reference(a: usize, b: usize) -> bool {
    a / SEGMENT_SIZE == b / SEGMENT_SIZE
}

/// The alignment half, likewise restated.
fn aligned_reference(a: usize) -> bool {
    a % rusty_alloc::types::MAX_ALIGN_SIZE.min(8) == 0
}

fn take8(it: &mut impl Iterator<Item = u8>) -> usize {
    let mut v = 0usize;
    for i in 0..8 {
        v |= (it.next().unwrap_or(0) as usize) << (i * 8);
    }
    v
}

const MAX_LIVE: usize = 128;

fuzz_target!(|data: &[u8]| {
    let mut it = data.iter().copied();

    // ---- Part 1: the predicate, against an independent reference ----------
    //
    // Two rounds so one input exercises both a fully random pair and a NEAR
    // pair. Random 64-bit values almost never land in the same segment, so
    // without the second round the `true` branch would be fuzzed at a rate of
    // about 2^-39 and the interesting boundary — where the check actually
    // decides something — would never be reached.
    //
    let a = take8(&mut it);
    let b = take8(&mut it);
    for (dec, base) in [(a, b), (a, a.wrapping_add(b % (4 * SEGMENT_SIZE)))] {
        let got = link_is_plausible(dec, base);
        let want = aligned_reference(dec) && same_segment_reference(dec, base);
        assert_eq!(
            got, want,
            "link_is_plausible({dec:#x}, {base:#x}) = {got}, reference says {want} \
             — the (a ^ b) < SEGMENT_SIZE identity disagrees with a / SEG == b / SEG"
        );
    }

    // ---- Part 2: no false positive on a genuine workload ------------------
    //
    // Drive real allocation churn so real free lists get built and walked. A
    // link the check wrongly rejects aborts here, and libFuzzer records it.
    // Sizes deliberately span the bins, plus aligned and reallocated blocks,
    // because those are where a link might legitimately sit somewhere the
    // check could be too strict about.
    let mut live: Vec<*mut u8> = Vec::new();
    while let Some(op) = it.next() {
        let sz = match op >> 4 {
            0..=5 => (op as usize & 0xff) * 8 + 8, // small
            6..=9 => 1024 + (op as usize) * 64,    // medium
            10..=12 => 64 * 1024 + (op as usize),  // large
            _ => 8,
        };
        match op & 0x3 {
            0 | 1 if live.len() < MAX_LIVE => {
                let p = alloc::malloc(sz);
                if !p.is_null() {
                    live.push(p);
                }
            }
            2 if live.len() < MAX_LIVE => {
                let p = alloc::malloc_aligned(sz, 64);
                if !p.is_null() {
                    live.push(p);
                }
            }
            _ => {
                if let Some(p) = live.pop() {
                    // SAFETY: `p` came from this allocator and is freed once.
                    unsafe { alloc::free(p) };
                }
            }
        }
    }
    for p in live {
        // SAFETY: each pointer came from this allocator and is freed once.
        unsafe { alloc::free(p) };
    }
});
