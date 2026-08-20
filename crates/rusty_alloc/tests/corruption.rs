//! Adversarial tests: the free-list corruption mitigations must actually FIRE.
//!
//! Every other test in this crate asks whether the allocator WORKS. These ask
//! whether it RESISTS — they corrupt heap metadata the way an overflow or a
//! use-after-free write does, and require the allocator to die rather than be
//! steered. That is a different question, and until this file existed nobody
//! had asked it: the four tests in `secure.rs` all write strictly INSIDE
//! legitimately-allocated blocks, so every mitigation in the crate was
//! verified for function and never once observed to fire.
//!
//! A mitigation nobody has watched work is a claim, not a defence — and it is
//! how mitigations rot silently, because a refactor that disables one keeps
//! the whole suite green.
//!
//! # Three ways this test could lie, and what stops each
//!
//! 1. **"The child died" is not the assertion.** If the link check does NOT
//!    fire, the allocator decodes attacker bytes into a pointer, hands it out,
//!    and the process dies of SIGSEGV touching it. A test asserting only that
//!    the child died would PASS in exactly the case the mitigation failed. So
//!    the assertion is on the SIGNAL: SIGABRT means detected-and-refused,
//!    SIGSEGV means followed-the-poisoned-link. Opposite outcomes. This was
//!    confirmed by disabling the check and watching the signal flip.
//!
//! 2. **The attack has to actually reach the free list.** A free goes onto the
//!    page's `local_free`, which is only folded into the `free` list the
//!    allocator serves from once `free` runs dry. Corrupting 32 blocks after
//!    32 allocations poisons a list that is never walked — the page still has
//!    ~992 pristine blocks to hand out — and the child sails through reporting
//!    a survival that tested nothing. Hence the page-exhausting drain, and the
//!    `reused` counter that makes a vacuous run visible instead of green.
//!
//! 3. **A DEBUG build has its own net that reaches the same abort.** Rust's
//!    misaligned-pointer check fires on a garbage link before our check would,
//!    aborting identically — so `garbage_link_aborts` passes in debug even
//!    with our mitigation disabled, and is only DISCRIMINATING in release.
//!    `aligned_out_of_segment_link_aborts` exists to close that hole: it keeps
//!    the decoded pointer perfectly aligned, so no alignment net anywhere can
//!    catch it and only the segment bound can. Verified by disabling the
//!    check: that scenario fails in BOTH profiles, the garbage one only in
//!    release.
//!
//! The abort is deliberately silent (see `page::corrupt_free_list_abort`), so
//! the signal IS the diagnostic. The boundary behaviour of the check itself is
//! pinned separately and exhaustively by `page::link_tests`, in-process.

use std::process::Command;

const MARKER: &str = "RUSTY_ALLOC_CORRUPT_CHILD";
const SURVIVED: &str = "CHILD-SURVIVED-CORRUPTION";

/// SIGABRT. Hardcoded rather than taken from `libc`, which is a dependency of
/// the crate but not of its integration tests.
#[cfg(unix)]
const SIGABRT: i32 = 6;

/// Blocks per run. A 64 KiB slice holds ~1024 blocks of 64 bytes, so this
/// spans more than one page and guarantees the drain exhausts one.
const N: usize = 2048;
const SZ: usize = 64;

/// Build a genuine free list and leave the freed blocks poisonable.
///
/// Freeing every OTHER block is deliberate: freeing all of them would leave
/// whole pages empty, and an empty page is retired and re-initialised — which
/// would quietly discard the corruption before it could ever be read back.
fn build_poisonable_free_list(ps: &mut [*mut u8; N]) {
    for p in ps.iter_mut() {
        *p = rusty_alloc::alloc::malloc(SZ);
        assert!(!p.is_null(), "child: malloc failed");
    }
    for i in (0..N).step_by(2) {
        // SAFETY: each pointer came from `malloc` above and is freed once.
        unsafe { rusty_alloc::alloc::free(ps[i]) };
    }
}

/// Drain the page so the allocator MUST fold `local_free` in and walk the
/// poisoned links, then report whether the poison was ever actually reached.
fn drain_and_report(ps: &[*mut u8; N]) -> ! {
    // `reused` is the honest-instrument half: it counts allocations that came
    // back at a corrupted block's address. A survival with reused == 0 means
    // the attack never reached the free list and the run proved NOTHING — a
    // distinction "did the child die?" cannot make, and the exact way a
    // security test rots into a vacuous pass.
    let mut reused = 0usize;
    for _ in 0..N {
        let q = rusty_alloc::alloc::malloc(SZ);
        assert!(!q.is_null(), "child: malloc failed during drain");
        if (0..N).step_by(2).any(|i| ps[i] == q) {
            reused += 1;
        }
    }
    println!("{SURVIVED} reused={reused}");
    std::process::exit(0);
}

/// Scenario A — the blunt attack: overwrite the link with overflow filler.
fn child_garbage_link() -> ! {
    let mut ps = [core::ptr::null_mut::<u8>(); N];
    build_poisonable_free_list(&mut ps);

    // Overwriting the link word of a freed block is the classic heap
    // exploitation primitive: if the allocator believes it, the next
    // allocation returns an attacker-chosen address and every write through
    // that pointer is an arbitrary write. The value is the canonical overflow
    // filler ('A' repeated), trimmed to 16-alignment because an attacker's
    // target always is.
    for i in (0..N).step_by(2) {
        // SAFETY: deliberately writing to freed memory — the attack being
        // simulated, and the point of the test. The memory is still mapped
        // (the page is live and holds the free list), so this reaches the
        // allocator's metadata rather than an unmapped address.
        unsafe { ps[i].cast::<usize>().write(0x4141_4141_4141_4140) };
    }
    drain_and_report(&ps);
}

/// Scenario B — the precise attack, and the one that isolates the segment
/// bound.
///
/// Rather than replacing the link, flip a HIGH bit of the value already there.
/// Decode is `(enc ^ k0) - k1`, so flipping bit 44 of `enc` moves the decoded
/// address by exactly 2^44 (16 TiB) and leaves the low bits — and therefore
/// the ALIGNMENT — untouched. The result is a perfectly aligned pointer far
/// outside the segment.
///
/// That matters because it defeats every alignment-based defence at once: our
/// own old alignment-only check, and Rust's debug misaligned-pointer check
/// that otherwise masks scenario A in debug builds. Only the segment bound
/// stops it. It also needs no knowledge of the per-page keys, which is the
/// realistic threat model — an attacker who can write, but cannot read, the
/// encoding keys.
fn child_aligned_out_of_segment_link() -> ! {
    let mut ps = [core::ptr::null_mut::<u8>(); N];
    build_poisonable_free_list(&mut ps);

    for i in (0..N).step_by(2) {
        // SAFETY: as above — a deliberate write to freed-but-mapped memory.
        unsafe {
            let slot = ps[i].cast::<usize>();
            slot.write(slot.read() ^ (1usize << 44));
        }
    }
    drain_and_report(&ps);
}

/// Spawn the child in the named scenario and require an abort, not a segfault.
fn assert_child_aborts(scenario: &str) {
    let exe = std::env::current_exe().expect("current_exe");
    let out = Command::new(exe)
        .env(MARKER, scenario)
        .arg("--test-threads=1")
        // Without this libtest CAPTURES the child's stdout, and the child ends
        // in `process::exit`, which never gives libtest the chance to reprint
        // it — so the "I survived" marker vanishes and a survival reads as a
        // silent clean exit. Same class of instrument bug as the
        // stdout/stderr one recorded in `foreign_free.rs`: the reporting
        // channel was wrong, not the code under test.
        .arg("--nocapture")
        .output()
        .expect("spawn child");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        !combined.contains(SURVIVED),
        "[{scenario}] the child walked a POISONED free list to completion. \
         Every corrupted link was accepted and handed out as an allocation — \
         the free-list hijack primitive is live.\n{combined}"
    );
    assert!(
        !out.status.success(),
        "[{scenario}] the child exited cleanly after its free list was \
         corrupted.\n{combined}"
    );

    // The distinction that makes these tests worth having.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            out.status.signal(),
            Some(SIGABRT),
            "[{scenario}] the child died, but NOT on the corruption check. \
             SIGSEGV here means the allocator decoded the attacker's bytes \
             into a pointer and followed it — the mitigation did not fire, and \
             'the child died' would otherwise have been read as a pass.\n\
             {combined}"
        );
    }
}

/// True when this build actually has the mitigation compiled in.
///
/// `linkcheck` counts: it applies the SAME bound to an unencoded link, so both
/// attacks below must still be stopped by it. That arm is the whole point of
/// the experiment — a build with the bound and no encoding should still refuse
/// an out-of-segment target, and if it does not, the feature is worthless.
fn mitigation_present() -> bool {
    // `blockmap` counts too, and by a completely different route: it does not
    // check the link at all, it checks the BLOCK the link led to. A poisoned
    // link produces an address whose computed block index is nowhere near the
    // page, so the map rejects it on the bound. Worth asserting because it
    // means the two defences are independent — either alone must stop this.
    if cfg!(any(
        feature = "secure",
        feature = "linkcheck",
        feature = "blockmap"
    )) {
        return true;
    }
    // The encoding and the link check are both `secure`-only; the default
    // build stores links as plain pointers and follows them without checking,
    // which is deliberate parity with the oracle's release default. Saying so
    // is more honest than a vacuous pass.
    eprintln!(
        "skipped: free-list encoding and the link check are `secure`-only, \
         and this build does not enable it"
    );
    false
}

#[test]
#[cfg_attr(miri, ignore)] // spawns a child process
fn garbage_link_aborts() {
    if let Ok(s) = std::env::var(MARKER) {
        if s == "garbage" {
            child_garbage_link();
        }
        return;
    }
    if !mitigation_present() {
        return;
    }
    assert_child_aborts("garbage");
}

#[test]
#[cfg_attr(miri, ignore)] // spawns a child process
fn aligned_out_of_segment_link_aborts() {
    if let Ok(s) = std::env::var(MARKER) {
        if s == "aligned" {
            child_aligned_out_of_segment_link();
        }
        return;
    }
    if !mitigation_present() {
        return;
    }
    assert_child_aborts("aligned");
}
