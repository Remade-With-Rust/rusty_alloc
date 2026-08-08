# rusty_alloc LEDGER

One entry per milestone/brick: what landed, the numbers with their method lines,
what was reverted and **which kind** of revert (measured-worse vs within-noise).
Newest first.

## 0.4.0 — the three unmeasured things, measured (2026-08-08)

Perf, RSS and the `secure` feature had all been *recommended* without numbers.
All three measured before cutting 0.4.0. Method throughout: deterministic
COUNTS wherever a count exists (callgrind instructions retired, RSS bytes), and
where a duration was unavoidable, ABBA interleaving with a **null arm**.

**1. Did the seven fixes cost performance? No — and this one is exact.**
Wall/CPU time in Docker could not resolve it (null arm read **1.0253** on means:
the environment's floor is ~2.5%, wider than any effect). So the verdict comes
from instructions retired, which has no noise floor:

| kernel | pristine 0.3.2 | fixed | ratio |
|---|---:|---:|---:|
| malloc-small (single-threaded, fully repeatable) | 229,924,005 | 229,921,893 | **0.99999** |
| larson-4t | 129.6–130.9 M | 129.7–130.9 M | ranges overlap |
| xmalloc-4p | 203.3 M | 203.3 M | **1.0000** |

2,112 instructions in 230 M. Expected: the fixes add predictable early returns
on the adopt/retire COLD paths and touch the malloc/free fast path not at all.
Parity with mimalloc is preserved transitively — the fast path does identical
work to the version that measured 0.99–1.01 against mimalloc — though that arm
was not independently re-run here.

**A surprising number that was WRONG, kept as a warning.** The first xmalloc
reading was **4.86×**. Re-run three times per arm it is 203.3 M both ways: the
outlier was a one-off scheduling artifact under callgrind. Work parity was
confirmed independently (both arms print `blocks=600000`, same seed). Re-verify
a surprising number before acting on it.

**2. RSS — one clear win, one open question.**

The decommit fix, measured directly (reserve 512 MiB, commit, touch every page,
decommit, read RSS):

| decommit impl | returned to OS | contents after |
|---|---:|---|
| pristine `MADV_DONTNEED` | **6.4%** (27.4 of 427.9 MiB) | `165` — STALE, contract violated |
| fixed `mmap MAP_FIXED` | **100.1%** (457.8 of 457.3 MiB) | `0` — zeroed, contract honoured |

Soaks (daemon-shaped: thread waves that exit holding live blocks, forcing
abandonment, against a bounded live set):

- **purge ENABLED** (`purge_delay = 0`), 6 min: RSS flat at **9.4 MiB**, slope
  **−0.02 MiB/min**, peak 14.8. Clean.
- **shipped default** (`purge_delay = -1`, purging opt-in), 25 min, 299 samples:
  ~650 MiB RSS against a ~175 MiB mean live set, drifting **+1.45 ± 0.70
  MiB/min** (least-squares, 95% CI) over the full run. The drift DECELERATES —
  first half +2.42 ± 2.06, second half **+1.19 ± 1.87, no longer
  distinguishable from zero** — and RSS does not track the live set
  (corr = **+0.034**), so this is retention approaching a plateau, or a slow
  leak, and **25 minutes cannot separate those two**. NOT claimed as settled.
  The naive two-endpoint slope this harness printed first (+2.69) is not a
  sound estimator on data with 307 MiB peak-to-peak oscillation; the regression
  above supersedes it.

Actionable consequence: **long-lived services should set `purge_delay >= 0`**
rather than rely on the opt-in default. That is the configuration with flat,
measured RSS.

**3. `secure` — works, and costs 4–7%.** Full suite green with
`--features secure`; `stress_mt` 30/30 in release. Cost, instructions retired:

| kernel | default | secure | ratio |
|---|---:|---:|---:|
| malloc-small | 229,921,829 | 245,439,205 | **1.0675×** |
| larson-4t | 130,150,515 | 136,816,187 | 1.0512× |
| xmalloc-4p | 203,333,568 | 211,720,930 | 1.0412× |

Throughput on the alloc-heaviest kernel: 84.0 → 73.6 Mops/s. A real but modest
price for guard pages + encrypted free lists on anything facing untrusted input.

**Also executed for the first time this round:** `wasm32-unknown-unknown` via
`bench/wasm-selftest.mjs` in a Node VM — **PASSED** (linear memory grew
2.06 → 64.00 MiB). The platform table said "tested in a VM self-test" on faith;
it has now actually been run.

## The abandon/adopt UAF family — `stress_mt` CLOSED, suite fully green (2026-08-08)

The open P0 from the entry below is fixed. **It was not a weak-memory-ordering
bug, and it was not aarch64-specific** — that hypothesis (recorded below, from
the fact that the same source passed 19/20 under Rosetta's TSO) was WRONG. It is
a family of three plain use-after-frees on the abandon → adopt → reuse path,
present on every platform. x86-64 survived them because the just-`munmap`ped
region there usually stayed mapped; native aarch64 unmaps a 32 MiB segment for
real and faults on the next touch. The correction matters: **these are latent
memory-safety bugs on x86-64 Linux and Windows too**, and the x86-64 arm of the
gate below improves as well.

**The shape, in one sentence: three functions can RELEASE the segment they were
handed, and each returned `()` — so every caller kept using the pointer.**

1. **`span_from_segments` used a segment that `adopt_segment` had released.**
   `adopt_segment` frees `seg` when it arrives empty and an empty one is already
   cached, or when a Huge segment's block had already died — then the caller read
   `(*aseg).used_pages` and called `span_alloc(aseg, …)`. Proof before fixing: a
   probe that `_exit(42)`s when the just-adopted pointer is the one adoption
   freed **fired in 19 of 20 runs, with SIGSEGV dropping to 0**.
2. **`adopt_segment` used a segment its own `retire_span` had released.** The
   tail's dead-large-span retire can empty the segment and release it; the very
   next line reads `(*seg).used_pages`. This was the residual crash — lldb put it
   in `adopt_segment` itself, at that read.
3. **`retire_span` freed a segment it had failed to unlink.** `remove_segment`
   ended in a bare `debug_assert!(false, "segment not in heap list")`, which is
   **compiled out in release**: the caller fell through and `segment_free`d a
   segment still linked in another list, leaving a dangling `h.segments` head
   that later crashed `thread_done`'s walk. A probe that `_exit(43)`s on the
   not-found branch fired in **4 of 30 runs**.

**Fix: put the outcome in the type, not in a comment.** `adopt_segment`,
`retire_span` and `remove_segment` now return `bool` and are `#[must_use]` with
a message naming the consequence. That is what makes this class non-recurring —
adding `#[must_use]` immediately surfaced all four remaining `retire_span` call
sites for audit (all four proved terminal and are annotated as such). Callers
that legitimately ignore it now say why in one line.

One premise checked rather than assumed while fixing: releasing a segment cannot
strand queued pages, because `used_pages` counts CARVED spans and a page is only
queued while carved — so `used_pages == 0` implies none of its pages are in a bin
queue. An earlier "park it instead of releasing" attempt was built on the
opposite assumption, measured no better (36/40), and was reverted rather than
kept as a belt-and-braces change.

**Gate, all on aarch64-apple-darwin unless noted, exit-code classified:**

| arm | `stress_mt` |
|---|---|
| pristine 0.3.2 | 0 / 20 |
| + the 4 platform fixes (entry below) | 17 / 30 ABBA |
| **+ this UAF family fix** | **30 / 30 ABBA · 100/100 release soak · 40/40 debug** |
| x86-64 (Rosetta), before | 19 / 20 |
| **x86-64 (Rosetta), after** | **30 / 30** |

`cargo test --workspace` is now **fully green on aarch64-apple-darwin — no
failures, no ignores beyond the pre-existing doctest** — and green on
x86_64-apple-darwin; wasm32 still builds. The `#[global_allocator]` smoke app
(Vec/String/HashMap/BTreeMap churn, 40 MiB allocations, cross-thread frees,
8-thread waves) passes 30/30.

`stress_mt` IS the regression test for this family: it failed 65–100% of runs
before and is now 100/100, so a reintroduction shows up immediately.

## aarch64-apple-darwin FIRST EXECUTION — 4 bugs fixed, 1 still open (2026-08-08)

The README's platform table said aarch64 "compiles; **never executed**". It was
executed, on macOS 26 / Apple Silicon (16 KiB pages), rustc 1.95.0. **Four
defects, two of them memory-safety class. Baseline 0.3.2 could not run a
realistic `#[global_allocator]` workload on this platform at all.**

Method for every rate below: the built binary run N times by a script that
classifies by EXIT CODE (0 pass / 101 panic / 134 SIGABRT / 139 SIGSEGV / 137
killed-at-timeout), arms interleaved ABBA so machine-load drift hits both
equally, binaries fingerprinted by sha256 before each arm — a stale binary
produced one bogus reading before that check was added.

**P0 — `thread_id()` read the wrong register (memory-safety).** The aarch64 arm
read `tpidr_el0`, which is the thread pointer on Linux/Android/BSD but NOT on
Darwin: Apple puts the thread pointer in `tpidrRO_el0` and uses `tpidr_el0` for
the CPU/cluster id. Measured directly: `tpidr_el0` returned small non-pointer
values (0x1002, 0x2005…), took **5 distinct values within ONE thread** over 3M
reads as it migrated cores, and **8 live threads produced only 5 distinct values
— distinct threads collided**. `thread_id()` is the ownership identity behind
`segment.thread_id`, so a collision routes one thread's `free` down the owner
(unsynchronised) path into another thread's segment. Fixed to
`tpidrro_el0 & !0b111` (Apple documents the low 3 bits as the CPU number — which
is exactly the observed drift). New standing gate `tests/thread_identity.rs`
asserts stability-within-thread and uniqueness-across-live-threads; verified to
FAIL against the old register before being accepted.

**P1 — subprocess isolation silently lost at teardown.** `abandoned_push` read
`my_subproc()` from a Rust `thread_local!` while running inside the
`pthread_key_create` destructor. Destruction order between a platform TLS
destructor and Rust's own TLS is unspecified everywhere, and is observably wrong
here: it read back 0, so a thread tagged into subproc N abandoned its segments
into the MAIN subproc. Probe: `ABANDONED_COUNT` was 1 (the abandon fired
correctly) but the segment sat on list 0, not list 1. Fixed by mirroring the tag
onto `HeapBox::subproc` — the box is alive for all of teardown because it is the
value passed *to* the destructor — and passing it into `abandoned_push`. Now
correct on every target rather than accidentally correct on one.

**P2 — `decommit` never returned memory to the OS.** `MADV_DONTNEED` is only
advisory for private anonymous memory on Darwin: it neither frees the physical
pages nor zeroes them, so purge was a no-op and RSS only ever grew (the
abandonment purge path is on by default, so this was live). It also violated the
documented "contents are lost" contract — caught by `prim.rs` reading back 42.
Nothing trusted that yet (`free_is_zero` is conservatively cleared on purge) so
it was not a disclosure bug, but it was a landmine. Fixed with a `MAP_FIXED`
anonymous re-map over our own range, which drops the physical pages and installs
zero-fill-on-demand ones, matching the Linux contract exactly.

**P3 — `process_info` RSS wrong on Darwin, in both directions.** `current_rss`
read `/proc/self/statm` (no procfs on macOS → 0), and `ru_maxrss` was scaled
×1024 as KiB when macOS/BSD report it in BYTES — a plausible-looking number
1024× too large, in the exact field the README lists as unmeasured. Both replaced
with one `task_info(MACH_TASK_BASIC_INFO)` call, which reports `resident_size`
and `resident_size_max` in bytes.

**Result.** Whole suite green on this platform except `stress_mt`. Realistic
`#[global_allocator]` workload (Vec/String/HashMap/BTreeMap churn, 40 MiB
allocations, cross-thread frees, 8-thread waves), ABBA-interleaved, n=20/arm:

| arm | pass |
|---|---|
| pristine 0.3.2 | **1 / 20** (19 × SIGSEGV, dies at the cross-thread-free stage) |
| with these four fixes | **20 / 20** |

**[CLOSED by the entry above — and the diagnosis below was WRONG: it is a plain
use-after-free family, present on every platform, not a weak-memory bug.]**

**STILL OPEN (P0-class, aarch64-native only).** `stress_mt`'s abandon → adopt →
reuse storm still crashes: release 7/20 pass, 11 × SIGSEGV, 1 × SIGABRT, 1 hang.
Consistent crash site `Heap::span_from_segments`, EXC_BAD_ACCESS on addresses
sharing a low offset with differing high bits (a walk off a stale/unmapped
segment), plus `debug_assert!(false, "segment not in heap list")` at heap.rs:524
and :977. Two experiments narrow it:

- **Not the thread-pointer path.** Forcing apple-aarch64 onto the safe
  `pthread_self` cached-TLS id leaves the rate unchanged (7/20 pass), so P0's fix
  is necessary but not sufficient — the remaining bug is elsewhere.
- **It passes under Rosetta.** The same source built for `x86_64-apple-darwin`
  passes **19/20**. Rosetta emulates x86 **TSO**, which is the signature of a
  WEAK-MEMORY-ORDERING bug in the lock-free abandon/adopt protocol — invisible on
  every platform tested so far (x86-64 Linux and Windows are both TSO), and only
  ever visible on genuinely weakly-ordered hardware. The alternative hypothesis,
  a 16 KiB-page geometry assumption, is NOT excluded: Rosetta also uses 4 KiB
  pages, so that variable moved too.

Note `tests/loom_xthread.rs` models the delayed-free/abandon PROTOCOL but not the
segment adopt path or the heap segment list, which is where the crash sits —
extending the model there is the obvious next probe.

**Also fixed: a test that hardcoded a 4 KiB page.** `bins::known_size_classes`
asserted `good_size(65537) == 69632`. Above `MEDIUM_OBJ_SIZE_MAX` good_size is
page-rounded, so the correct value is 81920 on a 16 KiB-page host. Split into a
property test deriving the expectation from `os::page_size()`.

## M9b — the free fast path, folded to one flags byte (2026-08-05)

Continuing the M9 win. The probe had already priced `prim::thread_id()`; the
remaining per-free work was a pile of separate loads answering one question —
"is this a plain binned page I can just push onto?":
`has_aligned`, `bin == BIN_HUGE`, `in_full`, and the segment's `kind` (a u32
compared against two magics, with a two-arm match on BOTH the page lookup and
the block recovery).

**Brick #3:** one `Page::flags` byte (`HAS_ALIGNED | SINGLE_BLOCK | IN_FULL |
HUGE_SEGMENT`). The free path now does ONE page resolution and ONE flags load;
the `SegmentKind` match is gone from the hot path entirely (a huge segment's
interior slices already offset back to slot 1, so `page_of` covers both kinds).
`in_full`/`has_aligned` bools were removed — the byte replaces them, so the
Page struct did not grow.

**Work parity proven before any timing was read** (§4): baseline and new
binaries report **byte-identical counters** on the same workload —
allocs 10 002 036 = frees, generic 604 676, pages_fresh 132, segments 1,
extends 553. Both arms do exactly the same work, so the comparison is valid.

**Gates:** Windows all-features green, Linux 21 suites / 0 failures, clippy
`-D warnings`, fmt.

**Harness defect fixed along the way:** `bench/pinvs.ps1` would not parse —
Windows PowerShell 5.1 reads a UTF-8 file as Windows-1252, and the em-dashes
in the comments turned into a parse error several lines later. THE timing
harness is now ASCII-only with a banner saying why. A harness that does not
run is a discipline that does not exist (§13).

**Numbers (pinned, CPU time, ABBA, same workload, work-parity verified) —
and the honest conclusion: THE CLOCK CANNOT RESOLVE THIS ON THIS BOX.**

| run | arms | pairs | median B/A | min B/A | win rate | z |
|---|---|---:|---:|---:|---:|---:|
| short | ~1 s | 21 | 0.908 (B faster) | 0.943 | 12/21 | +0.65 |
| long | ~7 s | 31 | **1.069 (B slower)** | 1.129 | 11/31 | −1.62 |

**The sign flipped.** Neither run resolves (|z| < 2 both times), and the
within-arm spread gives the game away: arm A's own median was 7234 ms against
its own minimum of 5812 ms — a **24% swing inside a single arm**, on a box
running two VS Code instances, a browser and Task Manager (checked, per the
go-find-the-process rule). A 5-10% effect is simply not measurable through
24% of noise, and a result that changes sign with arm length is not a result.

**NULL ARM — the session's noise floor, measured, not assumed** (§3). The
SAME binary against ITSELF, 21 pairs, ~7 s arms, identical method:

```
A: median 7046.9 ms  min 5703.1 ms
B: median 6828.1 ms  min 5875.0 ms
ratio of medians 0.9690 | ratio of mins 1.0301 | 11/21 | z = 0.22
```

Identical code measured **3.1% "faster" by median and 3.0% "slower" by min —
the two statistics disagree in SIGN on a null comparison.** That is the
resolution limit of this machine, and it retires the earlier readings on the
spot: a floor of ±3% cannot adjudicate a 5-10% claim whose own two runs
disagree by 16 percentage points (0.908 vs 1.069). The between-run conditions
moved, not the code. Every number in the table above is hereby marked
inadmissible; the null arm is why we run it before believing anything.

**Decision: KEEP the bricks, label them below instrument resolution.** This is
the §15 rule, not a rationalisation: for effects the clock cannot resolve, the
deterministic evidence is primary and the clock is confirmatory.
The deterministic evidence here is strong and independent of this box:
- `thread_id` measured 1.41 ns vs 0.25 ns cached, **reproduced to within
  0.02 ns on Windows AND Linux** — a mechanism, not a reading.
- Bricks #2 and #3 remove work by construction: a mask, a shift and two loads
  (brick #2) and three loads plus a two-arm match (brick #3) per free, with
  **byte-identical counters** proving the same work is performed either way.
- Nothing was added; the Page struct did not grow.

## P0 — 0.3.1 SEGFAULTED: a dying thread adopted orphans (2026-08-06)

**I shipped a segfault.** FFAI bisected it in one pass: 0.3.0 clean 0/8,
**0.3.1 crashing 6/8**, identical build and workload, allocator version the only
variable. Reproducible with **five JPEG decodes**, single-threaded.

### Cause — my one functional change in 0.3.1

`collect(force)` began reclaiming abandoned segments. But **both teardown paths
call a forced collect**:

```
init.rs:509  thread_done  -> h.collect(true)
init.rs:597  heap_delete  -> h.collect(true)
```

So a thread on its way out adopted every orphan from previously-dead threads
into **the heap it was about to destroy**, re-homing their pages onto a
`DelayedList` freed moments later. Use-after-free.

### Their data identified it before I read a line of code

The decisive detail: **more threads made it LESS frequent** — 1 thread 5/6,
2→3/6, 4→4/6, 16→1/6. Backwards for a race, exactly right for "a dying thread
swallows the orphan pool": with more live heaps, orphans get adopted by a
LIVING thread first. Everything else follows — every crash needs a thread exit
(60 detects on a preloaded image: 0/6), PNG is clean because of its thread
lifecycle not its allocation shape, and buffer size is irrelevant (640x480 and
1920x1080 both 6/6).

### Fix (0.3.2)

`collect_for_teardown()` — forced collect that does **not** reclaim. Both
teardown sites use it; `mi_collect(true)` on a LIVE heap still reclaims.

### The regression test DOES NOT reproduce it — do not trust it yet

`tests/teardown_reclaim.rs` passes 4/4 **with the bug deliberately
reintroduced**. It is not a guard, and it is labelled as such in its own header.
The missing ingredient is almost certainly CROSS-THREAD frees: every thread in
it frees its own blocks, so nothing remote is pushing onto the dying heap's
`DelayedList` — which is what makes a re-homed `xheap` a live target. **The fix
is reasoned and matches every observation, but it is not test-proven.**
FFAI's repro is the only thing that has reproduced this.

### Process failures

1. **A caret requirement made this automatic.** FFAI had
   `rusty_alloc = "0.3.0"`, which silently resolved to 0.3.1 on publish,
   mid-session, under an already-validated lockfile. They shipped a
   segfaulting default for several commits without touching a version string.
2. **Nothing gated `collect(force)`.** Not one test called it, which is exactly
   why the earlier `_force`-is-ignored bug survived to be found by a reader
   rather than a gate. I fixed the ignored parameter and added no test for it.
3. **The teardown call sites were never checked.** I searched for who called
   `collect` when diagnosing FFAI's report, saw both teardown paths, and still
   changed the shared implementation.

**Recommendation: YANK 0.3.1.**

## collect(force) ACTUALLY DOES SOMETHING — reported by FFAI (2026-08-06)

FFAI reported two things against 0.3.0. Both were real.

**1. Stale M2 comments.** `rusty_alloc_api/src/lib.rs:129` said
*"(M2: one global locked heap)"* and `heap.rs:3` said *"M2: ONE global heap
behind a lock"* — describing an architecture removed in M4. Anyone reading
either would conclude `free` serialises on a global lock. Corrected.

**2. `collect` ignored `force` — the signature was literally `_force: bool`.**
So `mi_collect(true)` was a per-heap page sweep and nothing else: it never
reclaimed an abandoned segment. That is a large part of why a caller's "trim"
measures ~0% — there was nothing in the forced path to reclaim WITH. `force`
now adopts every orphan first, so the bin sweep retires their dead pages in the
same pass.

### The purge half CRASHED and was reverted

A forced collect should also return pages to the OS, so the first version
purged every free span. **It crashed the test suite with an access violation
(0xC0000005).** Cause: `span_free` purges only spans of
`len >= MEDIUM_PAGE_SLICES`, so purging smaller ones reaches spans whose reuse
path does not re-commit them — the M8 defect exactly (Windows `MEM_DECOMMIT`
faults on touch; Linux `MADV_DONTNEED` does not, which is why this class keeps
being Windows-first). Reverted. A forced purge needs the recommit path audited
before it can ship.

Kept: reclaim-on-force. Gates green — Windows tests + clippy, Linux GATE
PASSED, churn 3/3, wasm, and speed unchanged (lua 0.9883, perl 1.0060,
sqlite 1.0037).

## RSS TAIL — abandoned segments; two fixes, IMPLEMENTED not yet VALIDATED (2026-08-06)

**FFAI, N=5, one program, trim the only variable:**

| arm | RSS med | RSS min | RSS max | latency |
|---|---:|---:|---:|---:|
| mimalloc | 111.1 | 106.7 | 134.2 | 31.97 ms |
| rusty no-trim | 195.8 | **92.2** | **403.3** | 30.63 ms |
| rusty trim200 | 195.8 | 91.2 | 402.5 | 30.49 ms |

**The finding is the SPREAD, not the median.** Our MINIMUM (92.2) beats
mimalloc's (106.7); our max is 3x theirs; spread 4.4x against their 1.26x. A
retention-policy difference shifts a distribution — it does not stretch one. And
92->403 MB is roughly ten 32 MiB segments, so something timing-dependent decides
how many the process holds. Latency is FINE (we are 1.4 ms faster).

Trim reclaiming 0.1 MB (0%) is not a sampling artifact: **trim walks a heap's
own free spans and cannot see the abandoned list at all**, so it would read 0%
at any N.

### Localised

`tests/abandon_rss.rs`: 8 waves x 8 threads that allocate spans and exit.
**25 segments abandoned; a following 2048-block allocation burst adopted 4.**
At 32 MiB each that is the tail, and whether anything adopts is pure
scheduling — exactly the shape of a 4.4x run-to-run spread.

### Fix 1 — purge on abandon (`abandoned_page_purge`, now default ON)

The option existed **in the table by name only, defaulted to 0, and nothing
read it.** So an orphan kept every page it had ever touched, resident,
indefinitely. `segment::purge_free_spans` now decommits a segment's free spans
at the last instant the dying thread still owns it. Deliberately NOT gated on
`purge_delay`: that governs a LIVE heap's spans, which are likely to be reused
shortly, whereas an orphan has no owner to reuse anything.

### Fix 2 — adopt until satisfied, not twice

`span_from_segments` capped adoption at `tries < 2`, then took a fresh 32 MiB
segment while orphans sat unclaimed. Now each adopted segment is tried
immediately and the loop runs until the request is met or the list is empty
(cap 32, a stall guard rather than a reclaim budget).

### HONEST STATUS: implemented and correctness-gated, NOT validated for RSS

Neither fix is shown to reduce RSS yet, because **no probe here reproduces the
shape**. The count probe measures orphans, not bytes; `bench/churn.c` has a
4.9 MiB working set (where we already use 7x LESS than mimalloc); the
long-lived-thread sweeps never abandon anything. Fix 2 also did not move the
orphan count, and the reason is instructive: **adoption only triggers on a
segment MISS**, so a thread with room in its own heap never reaches the
abandoned list however generous the cap.

Speed is unaffected — lua 0.9908, perl 1.0060, sqlite 1.0037, unchanged, and
neither fix touches the fast path.

**Validation must come from FFAI's workload**, which is the only thing that has
reproduced the tail. The A/B is free: `MIMALLOC_ABANDONED_PAGE_PURGE=0` vs `1`
on the same binary, N=5, comparing max and spread rather than median.

## RSS INVESTIGATION — mechanism confirmed, our retention NOT implicated (2026-08-06)

**External measurement (FFAI/Diana, their harness, their null arm):** speed at
exact parity (1.015x wall, CPU 1.000x), **peak RSS 91.4 MB vs mimalloc's 77.5 MB
= +17.9%**, null arm 77.4 so the gap is real. First independent numbers the
project has ever had, and they land on the gap the audit called most likely to
surprise us — we have no RSS gate at all.

### The instrument failure that came first

`bench/rss.sh` initially had NO NULL ARM. It returned 62.6, 62.8 and **51.6
MiB for the same binary** — an 11 MiB swing that silently invalidated three
conclusions drawn from it. Cause: **perl randomises its hash seed per process**,
so every run allocated a different pattern. Pinning `PERL_HASH_SEED=0` collapsed
the spread to **0.2 MiB**.

**Everything measured before that fix was retracted**, including a confident
"purging is the cause" claim. The rule this project already applies to time
applies to memory: *no null arm, no result.*

### What is now established

| probe | result |
|---|---|
| single thread, perl cycling | rusty_alloc **51.8** vs mimalloc 52.9 — we are BETTER |
| thread sweep 1 -> 28 | RSS scales LINEARLY with threads for both; at 28, ours **122.8** vs **228.2 MiB** |
| size sweep 32 KiB -> 4 MiB | −44% at every size |
| alignment 0 / 32 / 64 / 4096 B | −44% at every alignment |

**Per-thread heap retention IS the dominant RSS term** — confirmed
independently, and it matches FFAI's `scaling.rs` finding (28 heaps retaining
174 MiB against 26.3 MiB live). But **on every synthetic form of that mechanism
we retain roughly HALF of mimalloc**, so our retention is not yet implicated in
their +17.9%.

### Not reproduced, and what is left

Nothing synthetic reproduced the gap. Untested, in order of suspicion:
1. **Mixed lifetimes** — model weights held for the session while activations
   cycle. Every probe here frees everything each round, so none of them
   exercises fragmentation.
2. **Thread lifecycle churn** — probes use long-lived concurrent threads;
   candle's pool may create/destroy, routing through abandonment/adoption.
3. **Rust `dealloc` passing a `Layout`** (size AND align) where C `free` passes
   only a pointer.

**Next step is not more guessing:** run FFAI's own `scaling.rs` with
rusty_alloc as the arm. It already produced the 174-vs-26.3 number, so it
measures per-thread retention under Diana's real behaviour — exactly what these
probes failed to synthesise.

### Also refuted here (both on the FIXED instrument)

- **Purging** (`purge_delay: -1`, off by default) recovers ~2 MiB of ~10
  single-threaded and **nothing** multi-threaded (122.8 vs 122.4). It is worth
  enabling but it is not the gap.
- **Deferred retire** — keeping emptied pages queued so the next round reuses
  the same memory instead of first-fitting elsewhere. Span re-carve churn is
  REAL (504 pages retired and re-carved per round, 3,050 carves for a ~530-page
  working set, segment count flat) but changing it moved RSS not at all.
  Reverted.

New probes, all reproducible: `bench/rss.sh` (null arm + pinned seed),
`bench/rss-threads.{c,sh}`, `bench/rss-sizes.sh`,
`crates/rusty_alloc/tests/rss_churn.rs`.

## P0 FIXED in 0.1.0-alpha.2 — use-after-free race (2026-08-06)

**Fix:** `wait_no_remote_in_flight(seg)` — spin until no page of the segment
has `XFLAG_FREEING` set — called on **every** path by which memory can reach an
arena.

The first attempt guarded only `segment_free` and **Miri still failed,
identically**. That refutation was the useful part: it proved the racing path
was elsewhere. `huge_free` recycles a huge segment through `chunk_free_n`
WITHOUT passing through `segment_free`, so guarding one choke point left the
real hole open. Both are guarded now.

Why a barrier is sufficient rather than an epoch scheme: before a remote sets
FREEING it has not yet pushed to the delayed list, so the owner cannot have
drained it, so `used > 0` and no retire is possible. Every dangerous instant
therefore has FREEING observably set.

Verified: `cargo +nightly miri test -p rusty_alloc` (isolation ON, the whole
target) exits 0 — `stress_mt::abandon_adopt_reuse_storm` included, which is the
test that caught it.

**LESSON — the one that matters most here:** the audit's `corpus/miri-gate.sh`
ENUMERATED suites (`alloc_core spans heaps secure prim`) and therefore silently
omitted `stress_mt`, the only multi-threaded one. It then recorded "Miri clean"
on that basis. CI ran the whole target and found a use-after-free on the first
green-field run. **Never let a gate enumerate what it should sweep.**

### Original diagnosis (kept for the record)


**Found by CI, minutes after publishing 0.1.0-alpha.1.** Miri's data-race
detector on `stress_mt::abandon_adopt_reuse_storm`:

```
Undefined Behavior: Data race detected between
  (1) atomic store        page.rs:430   thread `abandon_adopt_reuse_storm`
  (2) non-atomic write    segment.rs:535 thread `unnamed-8`
  at alloc57912+0x8000060
```

- **(2)** is `huge_alloc` scrubbing a recycled arena chunk:
  `write_bytes(seg, 0, size_of::<Segment>())` — which zeroes the whole header,
  including every page slot's `xthread_free` atomic.
- **(1)** is `remote_free`'s restore-DELAYED loop doing a
  `compare_exchange_weak` on `(*page).xthread_free` — **a page inside that very
  segment**.

So a segment was released, recycled through the arena, and re-tenanted as a
huge allocation **while another thread was still mid-`remote_free` on one of
its pages**. That is a use-after-free, and the write that lands on it is a
`memset` of the whole header.

This is the same FAMILY as the M8 P0 (guard pages recycled while still
PROT_NONE): a segment reaching an arena while something still references it.
The four-state protocol has FREEING precisely to stop teardown racing a remote
free — `page_set_flag` spins it out — so the gap is a teardown path that
reaches `segment_free` WITHOUT passing that gate. Not yet localised.

### Why the gates missed it

Miri was never run against `stress_mt`. The audit added `corpus/miri-gate.sh`
with the suite list `alloc_core spans heaps secure prim` — **the multi-threaded
suite was not in it**, and the audit entry even recorded "Miri clean" on that
basis. CI runs `cargo +nightly miri test -p rusty_alloc`, which runs
*everything*, and caught it on the first green-field run. The lesson is exact:
**a Miri gate that enumerates suites will silently omit the one that matters;
run the whole target.**

### Status

- **`0.1.0-alpha.1` is published on crates.io with this defect.**
  Recommendation: **yank** (`cargo yank --version 0.1.0-alpha.1`) for both
  `rusty_alloc` and `rusty_alloc-api`. Yanking blocks new dependents while
  leaving existing builds working; it is reversible.
- Reproduce: `cargo +nightly miri test -p rusty_alloc --test stress_mt`
  (isolation ON, i.e. no `-Zmiri-disable-isolation`).
- Blast radius: multithreaded programs that abandon threads AND allocate huge
  blocks. Single-threaded use is unaffected.
- Fix will need the loom model that built the protocol, not a point patch.

### CI fixes landed alongside (all real, all pre-existing)

1. **Clippy never ran on Linux.** `c_long` is `i64` on LP64 unix and `i32` on
   Windows, so four `as c_long` casts are "unnecessary" on Linux and
   load-bearing on Windows. `corpus/linux-gates.sh` now runs clippy too —
   running it only on Windows was a genuine hole.
2. **`double_free.rs` cannot run under Miri** — `current_exe()` needs
   `readlink`, blocked by isolation, and Miri cannot spawn the child anyway.
   Now `#[cfg_attr(miri, ignore)]`.
3. **Stale oracle path** in `ci.yml` (`out/mi` vs the OS-namespaced
   `out/linux/mi`), plus a wasm job that executes rather than only compiles.

## RELEASE PREP — 0.1.0-alpha.1 (2026-08-06)

### Double free: silent corruption -> clean abort

The known limitation recorded in the audit is fixed. `page_push_local` did
`(*page).used -= 1` with no guard, so freeing a block twice wrapped `used` to
`u32::MAX`: the page never retired and the same block sat on the free list
twice, so a later pair of `malloc` calls handed the SAME memory to two owners.
Release builds accepted this silently; upstream mimalloc does too.

Now detected via a sign test on the post-decrement value — legitimate `used` is
always far below `i32::MAX`, so a negative reading can only be the wrap — and
the process aborts. Proven by `tests/double_free.rs`, which re-executes its own
binary in a child so the abort can be observed rather than assumed.

**It costs real performance and that was the deliberate call:**

| | perl | sqlite |
|---|---:|---:|
| M16 (no detection) | 1.0021 | 1.0018 |
| **with detection (shipped)** | **1.0062** | **1.0037** |

~4 Ir per free. Two forms were tried; both cost the same, and the disassembly
confirms the ideal `dec eax; js` sequence — the cost is the load/store around
it, because reading `used` back after the store prevents LLVM from keeping the
whole thing as a single `dec [mem]`. Kept anyway: an allocator whose premise is
memory safety should not hand the same block to two owners to save 0.4%.

### Carved out for release

- **`publish = false`** on `_ffi`, `_override`, `_bench`, `_wasm` — harnesses,
  fixtures and native artifacts, not libraries. Only `rusty_alloc` and
  `rusty_alloc_api` go to crates.io.
- **LICENSE added** (MIT), naming the dev-only vendored trees explicitly:
  `oracle/mimalloc` and `corpus/mimalloc-bench` are outside every published
  package directory and never ship.
- **Per-crate READMEs** for the two published crates; `cargo package` verified
  at 32 files / 311.9 KiB — no oracle, no corpus, no target.
- **A stray zero-byte file named U+F03A** (an unprintable private-use
  character) was sitting at the repo root, an artifact of an earlier shell
  redirect. Removed before it could be committed.
- Version set to **`0.1.0-alpha.1`**, with the reason in the manifest: the
  allocator is done, the evidence is not.

### Wall-clock: the instrument, and what it can honestly say

`bench/wallclock.sh` — pinned, ABBA-interleaved, N=31, medians AND minima, with
a **null arm** (the same allocator against itself) as the floor.

The first version was WRONG and said so loudly: every `min` came back 0.0 ms,
because `/usr/bin/time` reports at 10 ms granularity — on a 300 ms workload
that is ~3%, coarser than the ~0.5% effect being measured. Replaced with
microsecond `EPOCHREALTIME` and workloads scaled to >1 s so per-run fixed costs
fall below the noise. (A SECOND harness bug survived that: `min` was still
0.0 ms because the accumulators start empty and grow with `" $x"`, so the list
had a leading space that `sort -g` ranked as zero. `awk NF` now drops it —
a whole statistic had been silently dead across two consecutive runs.)

**The result, N=31, pinned, microsecond timer:**

| arm | median ratio |
|---|---:|
| **null (rusty_alloc vs ITSELF)** | **1.0117** |
| perl, ra vs mi | 1.0009 |
| sqlite, ra vs mi | 1.0091 |

**The null arm is 1.17% — wider than either effect.** The same allocator
compared against itself differs by more than the difference we are trying to
detect. The only conclusion this instrument supports is *"at parity, below
measurement resolution"*, and that is what the README says. The wall-clock debt
carried since M9 is now paid in the only currency available: we ran it, and it
says the question cannot be answered on this machine.

### Publish order

`cargo package -p rusty_alloc` succeeds (32 files, 312.6 KiB — no oracle, no
corpus, no target). `rusty_alloc_api` fails with *"no matching package named
`rusty_alloc`"* until the core is actually on crates.io — expected, not a
defect. **Publish `rusty_alloc` first, then `rusty_alloc_api`.**

## M16 — the prologue is GONE (fourth attempt) (2026-08-06)

`free` opened with `push r15; push r14; push rbx` and closed with the pops —
six instructions of callee-saved traffic on a fast path that uses none of it.
Three previous attempts failed:

1. `#[inline]` + 5-arg cold split — worse.
2. 5-arg cold split alone — worse.
3. `#[cold]` on `retire_emptied` — worse, and it ADDED back a push.

All three attacked the same thing: where the code lives. **The fourth attacked
what stays LIVE.** Registers get saved because a value must survive a call, and
the only call on the fast path is the retire branch — which took
`(seg, pg)`. `seg` is dead the moment `page_of` finishes; it was being kept
alive across the entire fast path purely to serve a branch taken on 1.6% of
frees. Passing only `pg` and re-deriving `seg` inside the cold function (one
mask) lets it die immediately:

```
- 25b20: push r15 / push r14 / push rbx ...
+ 25c60: test rdi,rdi          <- no prologue at all
```

The fast path now fits entirely in caller-saved registers.

| | perl | sqlite | lua |
|---|---:|---:|---:|
| after M15 | 1.0044 | 1.0029 | 0.9865 |
| **after M16** | **1.0021** | **1.0018** | **0.9841** |

**perl is 0.21% from parity, sqlite 0.18%** — and both now beat glibc
(0.822 / 0.995).

The transferable rule, which cost four attempts to learn: *to remove a
prologue, shorten LIVE RANGES, not function bodies.* Splitting code out does
nothing if the split still threads hot values through its signature — and both
M13 and M16 landed only once the cold function's argument list was cut to the
single value it could not re-derive.

Gates: Windows tests exit 0, clippy `-D warnings` exit 0, Linux GATE PASSED
(23 suites), churn 5/5 clean, **Miri clean** — `alloc_core` 11 passed, `spans`
and `heaps` 1 each, zero UB and zero leaks. That run specifically clears the
new `segment_of(pg.cast())` derivation: `pg` points into `(*seg).pages`, so
masking it back to `seg` stays inside the same allocation and `with_addr`
preserves the provenance.

## M15 — the empty-page sentinel: one branch instead of two (2026-08-06)

Worked the four sized levers. The **malloc side** paid, twice.

M14's re-split showed the deficit had gone even (free +5.1 Ir/op, malloc +5.1)
while every recent brick had targeted free. Reading our malloc fast path
against upstream's found a structural difference:

```rust
let p = self.direct[w];
if !p.is_null() {            // <- upstream has NO such test
    let b = page_pop(p);
    if !b.is_null() { ... }
}
```

Two tests — "is there a page?" then "did it yield a block?" — where mimalloc
has one. Its `pages_free_direct` slots never hold null; an empty slot points at
a shared **empty page** (`_mi_page_empty`) whose free list is permanently null,
so popping from it returns null and falls through to the generic path exactly
as an exhausted real page does. **The two questions collapse into one.**

Ported as `Page::empty_sentinel()` + `page::EMPTY_PAGE`, published by
`update_direct` whenever a bin's queue is empty. `block_size`/`slice_count` are
1-ish rather than 0 only so the `debug_checks` validator accepts it. Sound as a
shared immortal `static` because `page_pop` returns BEFORE its first store when
`free` is null — nothing ever writes it.

`heap.rs:malloc` fell 6.03 -> **4.03** Ir/op, exactly the two deleted
instructions.

| | perl | sqlite | lua |
|---|---:|---:|---:|
| after M14 | 1.0060 | 1.0037 | 0.9883 |
| **after M15** | **1.0044** | **1.0029** | 0.9865 |

**sqlite is now 0.29% from parity — about 920K instructions of 317.8M.**

### Also retested and refuted (third time)

`#[inline]` on `alloc::free`, retested because M13 moved the general path out of
line and the body is now much smaller — the exact condition that was blamed the
first two times. Still a loss: perl 1.0044 -> 1.0055, sqlite 1.0029 -> 1.0033.
**Three attempts, three refutations; treat it as settled** and do not try a
fourth time without a genuinely new mechanism.

### Lever status after this pass

| lever | before | now |
|---|---|---|
| malloc-side deficit | +5.1 Ir/op | **+2.1** (M14 + M15) |
| free-side remainder | +5.1 Ir/op | +5.1 — now the larger half |
| `Page` 80->64 | ~1 Ir/op | confirmed ~1: the follow-back multiply is already gone, only the forward `idx*80` lea-chain remains. Large refactor, small prize — deprioritised. |
| aligned fast path (P2) | +21 Ir/op | unchanged; rare in the verdict workloads, ~0 whole-program |

Gates: Windows tests exit 0, clippy `-D warnings` exit 0, Linux GATE PASSED
(23 suites), churn 3/3 clean.

## M14 — the heap pointer in TWO instructions, not four (2026-08-06)

Post-M13 the remaining gap was split EVENLY — free +5.1 Ir/op, malloc +5.1 —
and malloc had barely been examined. The breakdown named the culprit
immediately: `init.rs:malloc` cost **4.00 Ir/op**, purely locating the heap.

M10c's TLS slot resolved the address the obvious way: load the offset from the
GOT, read the thread pointer from `fs:0`, add, dereference. Four instructions.
But **x86 does that addition in the addressing mode** — `fs:[reg]` is a
segment-relative load, so the explicit `fs:0` read and the add both vanish:

```
  mov {t}, qword ptr [rip + __ra_tls_heap@GOTTPOFF]
  mov {o}, qword ptr fs:[{t}]
```

M10c chose the four-instruction form deliberately, to keep the address
computation `pure` and CSE-able. That reasoning was wrong in practice: the
profile shows `heap_box` is called once per malloc, so there was never anything
to CSE — the optimisation paid for a benefit that could not occur.

| | perl | sqlite | lua |
|---|---:|---:|---:|
| before | 1.0067 | 1.0041 | 0.9917 |
| **after** | **1.0060** | **1.0037** | **0.9883** |

Gates: Windows tests exit 0, clippy `-D warnings` exit 0, Linux GATE PASSED
(23 suites), churn 3/3 clean.

## M13 — the six prologue instructions: a 1-ARG cold split (2026-08-06)

M12 left six instructions of callee-saved traffic (`push r15/r14/rbx` + pops)
at the top of `free`, on a fast path that needs none of it. A cold-split had
ALREADY been tried and measured worse, so the question was whether the idea was
wrong or the implementation was.

**It was the implementation, and the signature was the whole difference.** The
failed attempt threaded all five already-computed values
(`seg, pg, p, flags, local`) into the cold function — putting five registers of
argument setup ON THE HOT PATH to serve 1.6% of frees. The version that works
passes **only `p`**, which is already in the argument register, and re-derives
segment/page/flags inside. The ~10 instructions of re-derivation are paid on
1.6% of frees; the hot path pays nothing.

| | perl | sqlite | batch_lifo |
|---|---:|---:|---:|
| before | 1.0077 | 1.0045 | 70.98 (+11.28) |
| 5-arg split (earlier) | 1.0106 | 1.0060 | — |
| **1-arg split (kept)** | **1.0067** | **1.0041** | **69.98 (+10.28)** |

`push r15` is gone from the prologue (the remaining third push is stack
alignment, not a register save). `batch_lifo` is down to 1.172x from 1.256x
two bricks ago.

**Also tried and REVERTED: `#[cold]` on `Heap::retire_emptied`.** The theory was
sound — it is `#[inline]`, it runs only when a page empties, and its tree
(`retire_span` -> `span_free` -> `segment_free`) is large, so it looked like the
reason the fast path was provisioned for so many registers. Measured: perl
1.0067 -> 1.0084, sqlite 1.0041 -> 1.0048, **and `free`'s prologue gained back
the `push r15` the change was meant to remove.** Reverted; baseline restored
bit-exactly (sqlite 318,176,956).

That is three attempts at these six instructions: two refuted, one kept. The
transferable part is that "split the cold path out" is not one idea — its
signature decides whether the cost lands on the hot path or the cold one.

Gates: Windows tests exit 0, clippy `-D warnings` exit 0, Linux GATE PASSED
(23 suites), churn 3/3 clean.

## M12 — slice_offset in BYTES: the live-working-set brick lands (2026-08-06)

The previous entry located the biggest single item in our free path: `Page` is
80 bytes, not a power of two, so `page_of`'s span follow-back scaled a slice
count by 80 — `neg; lea; shl` before the subtract, 11.02 Ir/op of pointer
arithmetic, 32% of our whole free cost.

The obvious fix was to shrink `Page` 80 -> 64, which needs a POINTER to
disappear and is a large, risky refactor. **Reading upstream first found a much
cheaper route to the same instructions.** mimalloc's field is documented as
*"the `slice_offset` is the byte offset back to the first slice"* and
`mi_slice_first` is a plain byte subtract. Ours stored SLICES and paid the
scale on every free. Storing bytes deletes the multiply without touching the
struct's size at all.

Verified in the shipped artifact, not assumed — the follow-back went 7
instructions -> 4:

```
- lea rcx,[rsi+rax*1] ; movzx ; neg rax ; lea rax,[rax+rax*4] ; shl rax,0x4 ; lea rbx,[rcx+rax*1] ; movzx
+ lea rbx,[rsi+rax*1] ; movzx ; sub rbx,rax ; movzx
```

**Results — every workload improved, and sqlite now beats glibc too:**

| workload | before | after | vs glibc |
|---|---:|---:|---:|
| lua | 0.9926 | **0.9898** | 0.833 |
| perl | 1.0098 | **1.0077** | 0.827 |
| sqlite | 1.0056 | **1.0045** | **0.997** |
| batch_lifo | 73.98 (+14.28) | **70.98 (+11.28)** | — |
| mixed | 150.18 (−7.59) | **146.18 (−11.60)** | — |

3 Ir/op off `batch_lifo` — exactly the three deleted instructions. The
`batch` deficit is down from 1.256x to 1.189x.

The `slice_offset` range is now guarded by a const assert:
`(SLICES_PER_SEGMENT-1) * size_of::<Page>() <= u16::MAX` (40,880 of 65,535),
so a future `Page` growth fails the build rather than silently truncating.

Gates: Windows tests exit 0, Linux GATE PASSED (23 suites), churn 3/3 clean.

**Still open on this path:** the six instructions of callee-saved traffic
(`push r15/r14/rbx` + pops) at the top of `free`. Moving the general path out of
line to relieve it was tried and measured WORSE (see previous entry) — the
argument setup costs more than the saves. The `Page` 80->64 shrink also remains
available and is now worth less, since the follow-back multiply — its main
prize — is already gone.

## P1–P3 EXECUTED — three refutations, and the live-working-set answer (2026-08-06)

Worked the `docs/plans/opscan_v1.md` plan. **Net code change: none. Everything
proposed was refuted, and the refutations are the result.**

**P1 died on its own count, before a line was written.** Generic-path entries
per 100,000 allocations: **ours 1,566, mimalloc 1,562** (`batch_lifo`); on
`aligned`, 6,254 vs 6,250. We do NOT leave the fast path more often than
upstream, so the extend-policy change P1 proposed was wasted work that would
have traded RSS for nothing. This is the count-before-code rule paying for
itself for the second time this campaign.

**Two follow-on bricks measured worse and were reverted.** The count did show
our exported `free` making a real call into `alloc::free` (100,082 per 100,000)
where upstream's is one flat symbol, and `alloc::malloc` had `#[inline]` while
`alloc::free` did not — a tidy-looking asymmetry.

| | batch_lifo | perl | sqlite |
|---|---:|---:|---:|
| baseline | 73.98 | 1.0100 | 1.0056 |
| `#[inline]` + cold-split | 74.98 | 1.0105 | 1.0060 |
| cold-split ALONE | — | 1.0106 | 1.0060 |

The first brick changed TWO things at once; isolating the second run showed the
cold-split was the harmful half, which **refuted** the register-pressure theory
rather than leaving it plausible. Both reverted; baseline restored bit-exactly
(sqlite 1.0056).

**WHY WE LOSE ON A LIVE WORKING SET — answered.** It is a fast-path COST
problem, not a slow-path FREQUENCY one (the counts above prove the frequency is
identical). Per-operation on `batch_lifo`: our free **34.1** Ir/op vs 25.0, our
malloc **21.0** vs 16.9. The disassembly names the biggest single contributor:

**`Page` is 80 bytes, and 80 is not a power of two.** Slice indexing emits a
`lea`/`lea`/`shl` chain for `idx * 80` and again for the `slice_offset`
follow-back, measuring **11.02 Ir/op — 32% of our entire free cost**. Upstream
pads `mi_page_t` deliberately, commented *"improve page index calculation"*; we
never did.

**Next brick, sized but NOT built: shrink `Page` 80 -> 64 bytes.** 128 is
impossible (512 x 128 = the whole 64 KiB slice, no room for the segment header).
Cutting 16 is the difficulty: `block_size`->u32 (−4), `heap_tag`->i16 (−2),
`free_is_zero`+`purged` into the `flags` byte (−2) gets 8; the other 8 needs a
POINTER to go, and `next`/`prev` are load-bearing for cross-segment page queues.

**P2 `aligned`: mechanism found, not built.** The plan's guess (we lack a
natural-fit fast path) was WRONG — we have one, and both sides fast-path 93.75%.
The real cost is that ours proves alignment via `bins::good_size(size)` and then
`malloc(size)` recomputes the same bin, where upstream tests the actual next
free block with one AND. Real, but `posix_memalign` is rare in the verdict
workloads so whole-program value is ~0. **P3 `usable`:** 32 vs 30 Ir, correctly
last, not attempted.

Gates: Windows tests exit 0, clippy `-D warnings` exit 0, Linux GATE PASSED (23
suites), perl 1.0098 / sqlite 1.0056 / lua 0.9926 — unchanged.

## OPSCAN — per-operation scan vs mimalloc (2026-08-06)

Built and ran a side-by-side per-operation comparison. Full method, table and
ranked plan: **`docs/plans/opscan_v1.md`**. Two things belong in the ledger.

**A symbol-by-symbol diff is not possible against release mimalloc.** It inlines
the whole allocator into three symbols (`free`, `malloc`,
`mi_page_free_list_extend`). There is no `mi_free_block_local` or
`mi_segment_page_of` to line up against ours. So the scan compares
**operations**, via one C driver run under each allocator by `LD_PRELOAD`.

**Two of three estimators were disqualified, and the reasons are reusable.**
Per-object attribution under-counted us ~4x, because `callgrind_annotate`
ELIDES the `[object]` suffix on continuation lines — mimalloc's three fat
symbols each keep it, our cost is spread over many `file:function` lines that
lose it. Caught by a SIGN disagreement with the attribution-free estimator.
The repaired version then reported our allocator at 115.79 Ir/op on an op where
the whole process spends 82.37 — impossible, rejected on arithmetic alone.
**The admissible estimator is the one with no attribution step:**
`(Ir(2N) − Ir(N))/N` on process totals. Deltas exact; ratios diluted toward 1
by the constant caller overhead, so read the delta column.

**Result shape (ra−mi Ir/op, positive = we lose):**

| we lose | | we win | |
|---|---:|---|---:|
| aligned | +21.4 | huge | −52,517 |
| batch_fifo | +14.3 | realloc | −97.9 |
| batch_lifo | +14.3 | big / large | −51.0 |
| usable | +2.0 | med / small | −32.5 / −29.0 |

We win the simple ops (one block in flight) and lose the ops with a **live
working set**. That is exactly why perl sits at 1.0099 while a ping-pong
microbenchmark flatters us: **real programs look like `batch`/`mixed`, and the
microbenchmark where we look best is the least representative one.** `huge` is
a structural win — mimalloc pays mmap/munmap per 2 MiB cycle, our arena serves
from cache.

Plan ranked P1 `batch_*` (most representative), P2 `aligned`, P3 `usable`, each
with the COUNT that must be taken before any code changes. Not yet executed.

## WASM — we now run in a WebAssembly VM (2026-08-06)

Asked to validate wasm. **Starting point: we did not compile for wasm at all** —
`cargo check --target wasm32-unknown-unknown` gave 18 errors, because the prim
layer has arms for `windows`, `unix` and `miri`, and wasm is none of those.

**A correction to the competitive premise.** mimalloc ALREADY supports wasm: it
ships `src/prim/wasi/prim.c` built on `__builtin_wasm_memory_grow`, and its
readme lists WASM among supported platforms. So wasm is not a place we win by
default. The honest differentiator is narrower and still real: a **pure-Rust**
allocator needs no C toolchain, no emscripten, and targets
`wasm32-unknown-unknown` directly rather than only WASI.

### What was built

`crates/rusty_alloc/src/prim/wasm.rs` — one linear memory that only grows.
Every consequence is a genuine semantic difference, documented in the module:

- **`free` is a no-op.** Linear memory cannot shrink, so nothing returns to the
  host and our own segment/page caches become load-bearing rather than an
  optimisation. (Upstream documents the same for wasi.)
- **Alignment costs a ONE-TIME pad.** `memory.grow` yields 64 KiB alignment but
  a segment needs 32 MiB. We read the current end, grow `pad + size`, and
  return the aligned base — and because a 32 MiB-aligned 32 MiB block leaves
  the end 32 MiB-aligned, only the FIRST segment ever pays.
- **`protect` returns an error rather than succeeding.** wasm has no page
  protection, and a guard page that cannot trap would let a `secure` build
  claim a hardening it does not have.
- **No clock** (`clock_now` is a counter — purge *ordering* survives, duration
  does not) and **one thread** (constant id, static TLS table, destructors
  never fire because there is no thread exit).

### Two real defects the wasm build exposed

**1. A 32-bit arithmetic overflow.** `Random::next_usize` did `(hi << 32) | lo`
— a constant shift past the width when `usize` is 32 bits, which rustc rejects
outright. Now width-aware: two draws on 64-bit, one on 32-bit.

**2. The default arena cost 1 GiB of REAL memory.** `ensure_default_arena`
reserves 1 GiB, which on a native OS is a cheap *virtual* reservation committed
lazily. Wasm has no virtual reservation — `memory.grow` backs every byte
immediately — so the reservation was fully materialised before the first
`malloc` returned. And it bought nothing: wasm memory is never returned to the
host, so every segment is already permanently cached, which is exactly what the
arena was for. Now skipped on wasm via `DEFAULT_ARENA_PAYS`. Upstream reaches
for the same lever more mildly (`arena.c` divides the reserve by 4 "if virtual
reserve is not supported (for WASM for example)"); with grow-only memory,
skipping entirely is strictly better.

| | linear memory | selftest |
|---|---:|---:|
| with the default arena | 1056.06 MiB | 6.79 ms |
| **without (shipped)** | **64.00 MiB** | **1.75 ms** |

### How it is gated

`cargo test` cannot execute `wasm32-unknown-unknown`, so proof of EXECUTION
comes from `crates/rusty_alloc_wasm` — a cdylib exporting `ra_selftest` —
instantiated under Node by `bench/wasm-selftest.mjs`, driven by
`corpus/wasm-gate.ps1`. Ten checks with distinct failure codes: cross-bin
patterns verified only after ALL allocations (so overlapping live blocks are
caught rather than overwritten), `usable_size` floor, zalloc zeroing, realloc
prefix preservation across a moving growth, a 600 KB span, 200 rounds x 32
blocks of churn (the check that matters most on wasm, since unbounded growth is
the failure mode when page recycling breaks), and word alignment. **The same
self-test also runs natively under `cargo test`**, so any failure that is not
wasm-specific is caught by the ordinary gates instead of only by the runner.

### Honest limitations

- 64 MiB for a trivial workload is coarse. It is one 32 MiB segment plus the
  one-time alignment pad, and the 32 MiB segment granularity — inherited from
  the mask-based `segment_of` addressing — is simply large for wasm contexts
  where memory is the scarce resource. A wasm-tuned `SEGMENT_SIZE` is the
  obvious follow-up and is NOT done.
- `secure` guard pages are unavailable (no page protection), and wasm entropy
  is much weaker: no host RNG, a counter clock and a constant thread id leave
  the stack address and a global counter as the only varying seed inputs.
  Free-list encoding there is corruption detection, not exploit mitigation.
- Single-threaded only. The atomics+threads proposal would need the
  read-then-grow pair in `alloc` to take a lock, as upstream's wasi backend
  does around `sbrk`.
- Not benchmarked against mimalloc on wasm. Correctness is proven; **no
  performance claim is made.**

Gates: Windows all-features exit 0, clippy `-D warnings` exit 0, Linux GATE
PASSED (23 suites), WASM GATE PASSED. Native performance unchanged — lua
0.9930, perl 1.0099, sqlite 1.0056.

## AUDIT — loops, unsafe quarantining, and a gate that lied (2026-08-06)

A deliberate hunt for looping hazards and unsafe that no caller actually
quarantines. **The worst thing found was not in the allocator — it was in the
harness that certifies it.**

### Fixed

**1. `corpus/linux-gates.sh` reported success on a BROKEN BUILD.** It counted
`test result: ok` lines and grepped for `FAILED|panicked`. Compile errors print
neither word, so a build break yielded `failures: 0`. This is not hypothetical:
earlier the same day it printed `ok-suites: 1 / failures: 0` while the tree did
not compile, and that was briefly read as a pass. Now checks cargo's exit code,
fails on any test failure, and fails if the suite count collapses below 15.
**Verified by deliberately breaking the build** — it correctly reported
`GATE FAILED (cargo exit 101)` where the old script said `failures: 0`.

**2. The new `corpus/miri-gate.sh` shipped with the SAME bug, briefly.** Piping
`cargo miri` into `tail` makes the pipeline's status `tail`'s — always 0 — so
it printed "MIRI FAILED" and exited 0. Fixed with `${PIPESTATUS[0]}`. Worth
recording precisely because it shows the failure mode is easy to re-create the
moment you stop looking for it.

**3. `init::done_slot` could hang the whole process, forever.** The winner of
the `INIT` CAS is the ONLY thread that ever publishes `RAW`; every other thread
spins in a bare `loop` waiting for it. `TlsSlot::new(...).expect(...)` on the
winner therefore turned a rare resource failure into a permanent process-wide
hang — and panicking there also unwinds into C callers, which is why the
release profile is `panic=abort` in the first place. Replaced with an explicit
`std::process::abort()`, identical in debug and release.

**4. `Heap::free_fast` was dead code holding the only cross-checks.** M11
inlined its body into `alloc::free` and left the original behind with no
callers. Its two `debug_assert`s were the ONLY places verifying that the flags
byte agrees with independent representations — `HUGE_SEGMENT` vs the segment's
`kind` tag, `SINGLE_BLOCK` vs `bin == BIN_HUGE`. Since M9b routes the entire
free on that one byte, a desync would silently send a huge or unqueued span
down the binned path. Deleted the dead `pub unsafe fn` (less unsafe surface)
and moved both checks to the live decision point.

**5. Miri was in NO gate**, and is not installed in WSL — despite having caught
two real defects in this project (the M4 registry and M7 arena base). Added
`corpus/miri-gate.sh`; run on Windows nightly: `alloc_core` 11 passed, `spans`,
`heaps`, `secure`, `prim` all clean, zero UB and zero leaks.

### Audited and cleared, with the reasoning

- **`page_of`'s removed bounds check is sound.** All eight call sites derive
  `seg = segment_of(p)` immediately before the call, so
  `p.addr() - seg.addr() == p.addr() & (SEGMENT_SIZE-1)` and `idx < 512`
  **by construction**. The contract is discharged at every site.
- **The `cfg(debug_assertions)` counter gating cannot change behaviour.** A
  search for any comparison or branch reading a `stats` field returns nothing —
  the counters are write-only in the allocator.
- **The 4-state xthread loops terminate.** All are CAS-retry (lock-free) or a
  bounded spin on the short FREEING window; `page_set_flag` spinning out
  FREEING is the designed handshake, not a hazard.
- **The arena claim-and-verify loop terminates.** On conflict `idx = c + 1` can
  move BACKWARDS for n >= 3, but `run` resets to 0, so re-triggering requires
  rescanning n free chunks and each conflict consumed a competitor's claim.
  Theoretical livelock only under adversarial single-chunk churn.

### Known limitations, recorded not fixed (all at upstream parity)

- **`page_push_local` does `used -= 1` with no underflow guard.** A double free
  wraps `used` to `u32::MAX`, so the page never retires and corruption
  continues silently in release. Debug builds catch it — Rust's overflow check
  panics at the subtraction.
- **`page_of` trusts `slice_offset` read from the pointer's own segment.** A
  pointer that is not ours yields an arbitrary `slot.sub(off)`. Upstream's
  `mi_slice_first` has the identical shape and release mimalloc likewise does
  not validate. This is the failure mode behind the jemalloc/redis
  mixed-allocator crashes recorded in M10c.
- **MIRI BLIND SPOT, and it covers the newest unsafe code.** The x86-64 Linux
  inline-asm paths — `init::thread_id`'s `fs:0` read and `init::heap_tls`'s
  initial-exec slot — are `cfg(not(miri))`, so Miri exercises their
  `thread_local!` fallbacks instead. **The TLS fast path we actually ship has
  no Miri coverage at all**; its only gates are hardware ones
  (`bench/churn.sh`, the corpus sweep). Any future change there must be
  hardware-gated, not Miri-gated.

Gates after the fixes: Windows all-features exit 0, clippy `-D warnings` exit 0,
Linux GATE PASSED (21 suites, exit 0), Miri clean, churn 3/3. Release
performance unchanged — lua 0.9929, perl 1.0100, sqlite 1.0056 — the new
assertions are debug-only.

## M11 — the benchmark itself was unfair: MI_STAT (2026-08-05)

Asked for one more win on perl and sqlite. The profile said `free` runs
**600,567 times** on perl at **~35 Ir/call** against mimalloc's ~25, and that
the last structural difference from upstream's `mi_free_block_local` was that
ours touches the owning HEAP (xheap load -> `box_of_xheap` -> heap pointer, a
dependent load chain) while upstream's touches none — the PAGE owns
`local_free`.

**Two ceiling probes, and the second one refuted the first's explanation.**

| probe | perl | sqlite |
|---|---:|---:|
| baseline (heap chain + counter) | 1.0145 | 1.0079 |
| #1 drop chain AND counter | 1.0114 | 1.0064 |
| #2 keep counter via cheap TLS, chain only on retire | **1.0163** | **1.0087** |

Probe #2 came back WORSE than baseline. That inverted the diagnosis: the heap
chain was never the cost — in the baseline ONE resolution served both the
counter and the retire, so splitting it into a TLS read plus a later chain
*added* work. **The cost was the counter.**

Which led to the finding that matters more than the brick. Upstream:

```c
#if (MI_DEBUG>0)
#define MI_STAT 2
#else
#define MI_STAT 0     // <-- the release oracle has NO counters at all
#endif
```

Our counters were unconditional. So every ratio this campaign has published
measured a **counters-on rusty_alloc against a counters-off mimalloc**. The
change is therefore not only an optimisation, it is a correction to the
comparison: hot-path counters now live behind `#[cfg(debug_assertions)]`,
exactly upstream's rule, keyed off debug rather than a new feature flag so
there is no manifest plumbing and `cargo test` (a debug profile) keeps the
instrument that proves two binaries do identical work.

The free fast path is now push + decrement + one zero test, with the owning
heap resolved only when a page actually empties (`retire_emptied`).

**Result:**

| workload | before | after | vs glibc |
|---|---:|---:|---:|
| lua | 0.9978 | **0.9927** | 0.837 |
| perl | 1.0145 | **1.0101** | 0.829 |
| sqlite | 1.0079 | **1.0056** | 0.998 |

perl is under 1% for the first time; sqlite now also beats glibc (0.9983).

**The campaign, end to end:**

| workload | start | now | vs glibc |
|---|---:|---:|---:|
| lua | 1.0650 | **0.9927** | 0.837 |
| perl | 1.0703 | **1.0101** | 0.829 |
| sqlite | 1.0355 | **1.0056** | 0.998 |

Gates: Windows all-features exit 0, Linux 21 suites / 0 failures, clippy
`-D warnings` exit 0, plus `bench/churn.sh` 5/5 clean (640 threads).

### Two process failures worth more than the brick

1. **Never round-trip source through PowerShell.** `Get-Content -Raw` +
   `Set-Content -Encoding utf8` decoded the file as Windows-1252 and re-encoded
   it, turning every `§ → —` into mojibake across `heap.rs`. The identical trap
   is already recorded for `pinvs.ps1`; it applies to SOURCE too. Reversed with
   a CP1252 re-encode, but the rule is: use the editor, not a shell text
   round-trip.
2. **A global regex replace hit the definitions it was meant to feed.**
   Rewriting `self.stats.allocs += 1` -> `self.stat_alloc()` also rewrote the
   body of `stat_alloc` itself, producing infinite recursion — caught as a
   Windows stack overflow (`0xC00000FD`) and clippy's "function cannot return
   without recursing". Write the accessor AFTER the sweep, or exclude it.

## M10c — PARITY WITH MIMALLOC on lua; the TLS call is gone (2026-08-05)

The item M10b sized and declined to build, built — by a different design than
the one that was declined.

**What was rejected, and why the rejection was right.** A thread-pointer-keyed
side table: hash the TCB address into a global array of `(tp, heap)` pairs.
That is P0-class, because a TCB is recycled when a thread exits, so a stale
entry hands a NEW thread a DEAD thread's heap. Clearing it in `thread_done`
only helps if `thread_done` always runs — the exact assumption that produced
the M8 access violation.

**What was built.** A real ELF TLS symbol in `.tbss`, declared via
`global_asm!`, read with the **initial-exec** relocation:

```
mov {off}, qword ptr [rip + __ra_tls_heap@GOTTPOFF]   ; linker-resolved, pure
                                                       ; + readonly => CSE-able
slot = thread_id() + off                               ; thread_id() IS the fs base
```

Two instructions and a load, replacing a `call __tls_get_addr` into `ld.so`.
Verified in the shipped artifact, not assumed: `readelf -r` shows
`R_X86_64_TPOFF64` against `__ra_tls_heap` (the M10 lesson — measure the
artifact you ship).

**Why this design is sound where the keyed table is not.** The storage IS the
thread's own TLS block. Every thread receives a fresh block initialised from
the all-zero `.tbss` image at creation, so a recycled TCB cannot expose a dead
thread's heap — the staleness question does not arise. Initial-exec's cost is a
LOAD-TIME constraint (it needs a static-TLS slot, so a very late `dlopen` could
fail to load us), which fails loudly at load rather than corrupting memory. It
is the same trade upstream ships as
`__attribute__((tls_model("initial-exec")))`. x86-64 Linux only; every other
target keeps `thread_local!` (Windows TLS has no `__tls_get_addr` to remove).

**Result — we are at parity with mimalloc on lua:**

| workload | before | after | note |
|---|---:|---:|---|
| lua | 1.0198 | **0.9978** | 4 runs: 0.9954 / 0.9977 / 0.9979 / 1.0002 |
| perl | 1.0281 | **1.0145** | 4 runs, deterministic |
| sqlite | 1.0144 | **1.0079** | bit-identical across runs |

**The campaign, end to end:**

| workload | session start | now | gap closed | vs glibc |
|---|---:|---:|---:|---:|
| lua | 1.0650 | **0.9978** | **at/under parity** | 0.844 |
| perl | 1.0703 | **1.0145** | 79% | 0.832 |
| sqlite | 1.0355 | **1.0079** | 78% | 1.001 |

Gates: Windows all-features (exit 0), Linux 21 suites / 0 failures, clippy
`-D warnings`, fmt. Plus a brick-specific hazard probe — 640 threads
(40 waves x 16), each writing and verifying a thread-unique byte pattern
across 200 alloc/free rounds, x5 runs, zero corruption. That probe targets
precisely what a broken per-thread heap slot would produce.

### A correction to the real-world sweep record

The M8 note claimed all 10 OSS programs run correctly on us. **That over-claimed
on redis**, and this session's sweep exposed it. Measured, 8 startups per arm:

| preload | ok | crashed |
|---|---:|---:|
| none | 8 | 0 |
| **mimalloc** | **0** | **8** |
| rusty_alloc | 2 | 6 |

Cause: `redis-server` here is built against jemalloc (`mem_allocator:jemalloc-5.3.0`,
linked to `libjemalloc.so.2`) and reaches allocator symbols directly, so
LD_PRELOADing *any* replacement produces a mixed-allocator process — blocks
allocated by one and freed by the other. It is an unsupportable configuration
rather than a defect in either allocator, and the ORACLE fails it harder than
we do. Not attributable to this brick (the mimalloc arm contains none of our
code). The sweep should either drop redis or build it with
`MALLOC=libc`; leaving it in as a "pass" was the actual error.

Separately, `imagemagick` shows 4 distinct output hashes across 6 runs
**including system-vs-system**, i.e. its output is nondeterministic
independent of the allocator. The other 8 programs (jq, sqlite3, git, xz,
zstd, lua, perl, python3) agree byte-for-byte across all three arms.

## M10b — the gap is the FREE path; a third of it is now closed (2026-08-05)

With `__tls_get_addr` gone from the top, the per-function profile finally
allowed the decisive comparison — **our allocator against mimalloc's, function
by function, on the deterministic perl workload**:

| | mimalloc | rusty_alloc (before these bricks) |
|---|---:|---:|
| malloc side | 9.7 M | ~11.4 M (**already at parity**) |
| free side | **15.0 M** | **~41 M (2.7x)** |
| total allocator | 27.9 M (3.6%) | ~56 M (6.8%) |

That reframed the whole campaign: **our malloc was never the problem.** The
entire deficit lives in `free`, and two bricks came straight out of reading it.

**Brick #4 — `page_of` without the bounds check.** It resolves a block to its
page and is the allocator's hottest function (twice per free). It indexed
`[Page; 512]` with a runtime index, so LLVM emitted a bounds check it cannot
discharge — the bound is a property of the CALLER's contract (p lies inside a
32 MiB segment), not of the arithmetic. Replaced with `add`/`sub` on the base
pointer, same provenance, same address, invariant kept as a `debug_assert`.
This is the case `rusty-unsafe-optimizations` says to look for: not "sprinkle
`get_unchecked`", but *one* place where a provable invariant is invisible to
the compiler.

**Brick #5 — stop resolving the page TWICE per free.** `alloc::free` resolves
the page to route ownership, then handed only the SEGMENT to `free_local_at`,
which resolved the page again. M9 threaded the segment through and missed the
page. Threading it too deletes an entire `page_of` per free.

**Brick #6 — the flags byte was already there; nothing tested it.** `SLOW_FREE`
(`HAS_ALIGNED|SINGLE_BLOCK|IN_FULL|HUGE_SEGMENT`) had been defined in M9 and
never used. Meanwhile the free path re-derived, one load at a time, exactly
what those four bits already say: a `SegmentKind` match, a `bin == BIN_HUGE`
compare, an `IN_FULL` re-test, and an `unalign` guard. The bits are exhaustive
by construction — `SINGLE_BLOCK` is set at the same statement that sets
`bin = BIN_HUGE`, `HUGE_SEGMENT` at the same statement that builds a Huge
segment — so one test against the byte `alloc::free` had ALREADY loaded proves
all four. Clear byte routes to `Heap::free_fast`: push, decrement, one
empty-page test, and nothing else. This is upstream's
`page->flags.full_aligned == 0` shape, reached from our own side.

**Deterministic results (perl and sqlite are exact to 4-6 digits):**

| workload | after TLS brick | after #4 | after #5 | after #6 |
|---|---:|---:|---:|---:|
| lua | 1.0536 | 1.0477 | 1.0402 | **1.0198** |
| perl | 1.0602 | 1.0547 | 1.0476 | **1.0281** |
| sqlite | 1.0305 | 1.0278 | 1.0244 | **1.0144** |

**Session total — roughly two thirds of the gap to mimalloc, closed:**

| workload | start | now | gap closed | vs glibc |
|---|---:|---:|---:|---:|
| lua | 1.0650 | **1.0198** | **70%** | 0.860 |
| perl | 1.0703 | **1.0281** | **60%** | 0.844 |
| sqlite | 1.0355 | **1.0144** | **59%** | 1.007 |

Gates green throughout: Windows all-features, Linux 21 suites / 0 failures,
clippy `-D warnings`, fmt.

**Where the remaining 21.9 M instructions (perl) now sit.** The free path fell
from ~39 M to **23.4 M** against mimalloc's 15.0 M, and `free_local_at`
vanished from the profile entirely (inlined). Accounting for what is left:

| | ours | mimalloc | gap |
|---|---:|---:|---:|
| free path | 23.4 M | 15.0 M | 8.4 M |
| malloc path | 14.5 M | 12.8 M | 1.7 M |
| `__tls_get_addr` | 7.3 M | **0** | 7.3 M |
| | | | **17.4 M** (of 21.9 M measured) |

**This reprices the TLS item.** It was 0.89% of the program when the gap was
4.76%; the program cost has not changed but the gap has, so those same 7.3 M
instructions are now **a third of everything still separating us from
mimalloc** — the single largest named item left.

It is NOT built, deliberately. Stable Rust cannot select `initial-exec` for a
cdylib's `thread_local!`, and the alternative — a thread-pointer-keyed cache —
carries a P0-class hazard rather than a bug-class one: a TCB is recycled when a
thread exits, so a stale slot hands a NEW thread a dead thread's heap. Clearing
the slot in `thread_done` closes it only if `thread_done` always runs, which is
exactly the assumption the M8 P0 punished us for making. A 0.9% win does not
buy that risk. The honest options are a nightly-gated build flag or a design
that makes the stale entry detectable rather than merely unlikely.

## M10 — a REAL win on mimalloc's turf: the TLS model (2026-08-05)

Six-whys descent on "why are we 6.5% of instructions behind mimalloc on
small-object churn", using callgrind's PER-FUNCTION breakdown as a
deterministic stage profiler.

**D3 — which op?** The profile named it immediately, and it was not one of
ours: **`__tls_get_addr`, 12.97 M Ir (1.96% of the whole program)** — more
than half the cost of our entire `free` (14.6 M).

**D5 — the mechanism.** The shipping artifact is a **cdylib** (LD_PRELOAD).
Rust's `thread_local!` in a shared library compiles to the general-dynamic TLS
model, so **every access is a CALL into `ld.so`**. mimalloc's `_mi_thread_id()`
is one register read. We were paying a linker round-trip per free for a value
that lives in a register.

**D6 — and the instrument was lying to me.** The M9 probe measured TLS at
0.25 ns and I built on that. It measured TLS **inside an executable**, where
the model is local-exec — a register offset, no call. The artifact we ship is
a shared library. *Measure the artifact you ship*, not a convenient stand-in.
This is the third time in this project a probe measured the wrong context.

**Ceiling first, then cost** (`bench/tls-ceiling.sh`): rebuilt with
`-Z tls-model=initial-exec` → **2.00% of our instructions, 33% of the gap**.
That sized the prize before a line of the fix was written.

**The brick, on STABLE Rust:** read the thread pointer directly —
`fs:0` (x86-64 Linux), `gs:0x30` (x86-64 Windows), `tpidr_el0` (aarch64),
with the cached-TLS path kept for every other target. Exactly mimalloc's
mechanism. Soundness of id reuse is the same argument mimalloc relies on: a
dying thread abandons its segments (id stored as 0) before its TCB can be
recycled.

**RESULT — deterministic, reproducible, gap closed by a sixth:**

| workload | ra/mi before | ra/mi after | gap closed |
|---|---:|---:|---:|
| lua | 1.0650 | **1.0536** | 17% |
| perl | 1.0703 | **1.0602** | 14% |
| sqlite | 1.0355 | **1.0305** | 14% |

`__tls_get_addr` no longer appears in the profile's top entries at all.
Windows all-features green, Linux 21 suites / 0 failures, clippy + fmt.

**One brick tried and REVERTED (measured flat, not measured worse):** the
in-place `realloc` path bumps a counter, which costs a TLS heap lookup on the
commonest realloc outcome. Removing it left perl at 1.0602 and sqlite at
1.0305 — **unchanged to four digits** — because in-place reallocs are rare in
these workloads. It cost a work-parity counter for an unmeasurable gain, so it
went back. Recorded as *flat*, not *worse*.

**Instrument refinement:** lua's per-process hash-seed randomisation makes its
instruction count vary ~0.3% run to run; **perl and sqlite are deterministic
to 4-6 digits** (sqlite repeated to within 209 instructions in 326 M). Use
perl/sqlite for verdicts; treat lua as indicative.

**Standing:** ~5.4% of instructions behind mimalloc on small-object churn
(from 6.5%), ~11% AHEAD of glibc. The remaining TLS prize (~1%) is the heap
pointer itself, which needs either nightly's TLS-model flag or a
thread-pointer-keyed lookup — both are M11 candidates, both now sizeable
before they are built.

## M9c — the clock could not answer, so we stopped using the clock
## (2026-08-05)

The null arm proved this box cannot adjudicate a 5-10% effect. Rather than
wait for a quiet machine, we changed INSTRUMENT: **instructions retired
(callgrind)** — a counter, deterministic, indifferent to an open IDE, a
browser or thermal drift. Same program, same input, same output in every arm;
the allocator is the only variable. `bench/icount-arms.sh`.

**Instrument verified first** (three runs of the same arm):
ra 662.83 M / 662.78 M / 663.04 M — **0.04% spread**, versus the clock's 24%.
That is a usable instrument on a noisy box, and it is now the project's
default A/B for allocator work.

**The answer, finally free of noise:**

| workload | ra instructions | vs mimalloc | vs glibc |
|---|---:|---:|---:|
| lua (small-object churn) | 663.0 M | **1.065×** | **0.900×** |
| perl (hash/array churn) | 834.3 M | **1.070×** | **0.878×** |
| sqlite (bulk) | 328.1 M | 1.036× | 1.028× |

**We execute 6.5-7.0% more instructions than mimalloc on small-object
interpreter churn, and 3.6% more on sqlite — while executing 10-12% FEWER
than glibc on the same interpreters.** That is the shape the real-world sweep
hinted at, now quantified to four digits and reproducible on demand.

So the M9 story is complete and honest: the mechanism was real and is fixed
(the per-free OS call is gone), we are comfortably ahead of the system
allocator, and we remain **~7% of instructions behind mimalloc on exactly the
workload class that started this investigation.** That residual is the M10
target, and for the first time it can be attacked brick-by-brick with an
instrument that gives the same answer twice.

**Standing debt, narrowed:** a quiet-box wall-clock session is still owed
before any *time* ratio is published — but no longer to know whether a change
helps. Instruction count answers that today.

**What is still owed, and it is the same debt as M9:** a pinned session on a
QUIET machine (no IDE, no browser) at N >= 31 to convert "removes work" into a
standing speed number. Until that exists, rusty_alloc claims no speed ratio.

## M9 — WHY we lose on small-object churn: the mechanism, named and fixed
## (2026-08-05)

**The question:** the real-world sweep showed us winning on bulk workloads and
losing on small-object interpreter churn (lua, perl — same shape as cfrac).
Why?

**The answer, measured not guessed** (`rabench freepath-probe`, both OSes):

| component | Windows | Linux |
|---|---:|---:|
| loop floor | 0.24 ns | 0.22 ns |
| **`prim::thread_id()` — called on EVERY free** | **1.41 ns** | **1.39 ns** |
| const-init `thread_local` cache (candidate) | 0.25 ns | 0.23 ns |
| whole malloc+free pair, 48 B | 5.77 ns | 6.47 ns |

`free` must know the calling thread's id to route local-vs-remote. We were
calling the OS/libc every time — `pthread_self` through the PLT from a cdylib,
`GetCurrentThreadId` on Windows — for a value that never changes.
**That single call was 18–20% of an entire malloc+free pair**, and it lands
squarely on the workloads that do nothing but small alloc/free: interpreters.
The two platforms agreeing to within 0.02 ns is what makes this a mechanism
rather than a reading.

**Second finding, free of charge:** `alloc::free` resolves the segment and
page to route ownership, then `free_local` **recomputed both** — a mask, a
shift and two loads per free, for nothing.

**Bricks landed:**
1. `init::thread_id()` — const-init `thread_local` cache with a `#[cold]`
   first-call path; every hot site routed through it (~1.16 ns/free removed).
2. `free_local_at(seg, p)` — the already-resolved segment threaded through
   instead of recomputed. Byte-identical behaviour; strictly less work, which
   is the counter-style argument the clock cannot dispute.

**Gates:** Windows all-features green, Linux 21 suites / 0 failures, clippy
`-D warnings` + fmt clean.

**Performance verdict: NOT RESOLVED on this box, and I am not claiming one.**
Real-workload medians walked with N — lua ra/mi **0.751 at N=5 → 1.071 at
N=15**, perl 1.104 → 1.729 — the exact §16 failure mode (the estimator itself
trends; the reference's own throughput moved 25% between sessions). The
best-of-N floors are the only stable statistic here:

| workload | ra/mi (min-of-N) before | after |
|---|---:|---:|
| lua | 1.19× | **~1.00×** |
| sqlite | 0.84× | ~0.98× |
| perl | 1.05× | ~1.26× (contradicts the median direction) |

lua moving to parity is consistent with the mechanism; perl moving the wrong
way is not, and both arms slowed in absolute terms between sessions, so the
box — not the code — is the likely author of that number. **What is
defensible today: the mechanism is identified, quantified identically on two
platforms, and removed. The ratio needs a quiet machine at N ≥ 31 before it
goes in any README.**

**Next (M9 continued):** pinned quiet-box session for the standing ratio;
then the remaining fast-path candidates already visible in the probe —
the `SegmentKind` branch and `unalign`'s two loads on every free, and the
`generic`-path rate (6% of allocs) which sets how often we leave the hot path.

## M8b — P0 CLOSED + real-world validation sweep (2026-08-05)

**The P0 is fixed. Root cause: guard pages were recycled while still
`PROT_NONE`.** A guarded allocation protects the page after the object; when
that segment was released it went back to the **arena** with the protection
still applied, so the next tenant faulted on memory it legitimately owned.
Fix: lift protection (and restore commitment) before any segment can be
re-tenanted — `Segment::guarded`, handled in `huge_free`/`segment_free`.

**How it was found — the method, not luck.** Whole-suite runs faulted ~1/10,
every test passed alone, and my first hypothesis (abandon→adopt→arena churn)
was WRONG: a purpose-built MT storm (`tests/stress_mt.rs`, dying threads +
adopters + cross-thread frees + huge allocs) stayed clean over 10 runs. The
discriminator was **per-binary bisection**: only `tests/secure.rs` faulted
(1/6), yet each of its four tests passed alone 8/8 — so it was an INTERACTION.
That named the pair: the guarded-objects test creates PROT_NONE pages, and the
other tests recycle segments. The one earlier signal that had held all along —
"0/12 with arenas disabled" — then made sense: OS-released memory is unmapped,
so only the arena path resurrects a protected page. **Lesson (ledgered):
when a defect needs several tests to appear, bisect by BINARY and then by
PAIR; a clean single-test run is evidence of interaction, not of health.**

**Verification after the fix:** previously-faulting binary 12/12 clean;
`--all-features` 10/10 + 8/8; default 8/8; secure 12/12. Linux 21 suites/0.
clippy `-D warnings` + fmt clean.

**REAL-WORLD SWEEP — 10 open-source programs on rusty_alloc via LD_PRELOAD**
(`corpus/realworld.sh`, `corpus/realworld-medians.sh`): jq, sqlite3, python3,
git, xz, zstd, lua5.4, perl, ImageMagick, redis-server (+redis-benchmark).
- **Correctness: 10/10 ran, and every deterministic workload produced a
  BYTE-IDENTICAL output checksum under `ra`, `mi` and glibc.** No crash, no
  hang, no wrong answer. (ImageMagick's PNG bytes differ run-to-run under
  every arm — embedded timestamps, not a defect.) redis-server serves its full
  benchmark under our allocator: SET/GET/LPUSH/LRANGE_300 all complete.
  **This is the strongest correctness evidence the project has: real C
  programs, unmodified, on our allocator.**
- **Performance (medians of 5 ABBA-interleaved reps, WSL2 dev-loop numbers —
  NOT standing claims):** sqlite **0.90× of mi (we are ~10% faster)**;
  perl 1.21×; lua **1.66×** (min-of-N: 1.19×) — we are slower on the
  interpreter workloads. Median-vs-min disagreement is large on this box, so
  the ratio needs a quiet machine and N≥31 before anyone acts on it.
- **The pattern is consistent and actionable:** we win on
  large/bulk-allocation workloads (sqlite, malloc-large) and lag on
  **small-object-heavy interpreter churn** (lua, perl — and the same shape as
  the cfrac regression). That points at the single-threaded small-malloc fast
  path, exactly where M4 measured us at 0.93× of glibc's tcache. That is the
  M9 perf target, and it is now backed by real workloads rather than kernels.

**Still open:** cfrac regression un-diagnosed (needs a quiet re-run first);
the fast-path perf campaign; the v1 geomean gate.

## M8 — Hardening + purge; **v1 SIGN-OFF BLOCKED by an open defect** (2026-08-05)

**Landed:** `random.rs` — self-contained ChaCha8 CSPRNG, per-heap streams,
OS-seeded (BCryptGenRandom / /dev/urandom) with a documented fallback mix.
`secure` feature — **encrypted free lists** (`enc = (next + key2) ^ key1`, fresh
per-page keys, corrupt links caught by an alignment check on decode) routed
through `block_next`/`block_set_next` at every traversal.
**Guarded objects** — dedicated segment with a PROT_NONE trailing page, object
right-aligned against the guard so an overflow faults on the first byte past
it; sampling API (`mi_heap_guarded_set_sample_rate` / `_size_bound`) wired to
the option table. **Purge/decommit** of coalesced free spans (RSS lever) with
per-span `purged` state and recommit-on-reuse. **`debug_checks` implemented**
(our `dmi`): page-invariant and whole-segment span-tiling validators on the
hot paths.

**Defects found and FIXED during M8 (each real, each caught by a gate):**
1. **Purge without recommit** — Windows `MEM_DECOMMIT`'d spans were handed back
   out; the next touch faulted. Linux `MADV_DONTNEED` keeps pages accessible,
   which is exactly why this was Windows-only. Fixed: purge inside `span_free`
   after coalescing + `span_recommit` on reuse + full recommit before a segment
   returns to an arena (`Segment::purged_any`).
2. **Visitor read encoded links raw** — `visit_segment_blocks` walked free
   lists with plain `(*b).next`, so under `secure` it indexed a stack bitmap
   with garbage. Fixed (block_next + bounds-checked marking).
3. **Multi-chunk arena claim race** — `chunk_alloc_n` scanned for a free run
   then set the bits; the lock-free single-chunk path could steal one in
   between, giving TWO segments the same address. Fixed: claim-and-verify with
   rollback and rescan.
4. **`adopt_segment` mutated the span layout it was walking** — `span_free`
   coalesces, so the iteration could land mid-span and queue a bogus page.
   Fixed: never retire during the walk.

**OPEN DEFECT — v1 CANNOT SHIP (P0):** a rare access violation survives, in
the parallel test suite only. Measured after all four fixes: **1 in 10** runs
(`--all-features`), **1 in 5** (default), 0 in 12 with `MIMALLOC_DISALLOW_
ARENA_ALLOC=1`. Ruled OUT by experiment, not by argument: the secure
encoding (identity-encoding probe still crashed), purge (still crashes with
purging opt-in/off), option/env parsing (bypass probe still crashed), and
single-threaded execution (every test passes alone, 11/11). Not reproduced on
Linux (21 suites, 0 failures) — consistent with a Windows-only commit/protect
interaction OR with timing. Strongest remaining hypothesis: a segment is
returned to an arena (or reused) while another thread still reaches it —
i.e. `used_pages` accounting across abandon → adopt → `segment_free`.
Next probes: (a) make `debug_checks` assert `used_pages` against a live-page
recount at every segment transition; (b) an arena-chunk generation counter to
catch reuse-while-referenced; (c) rebuild the Windows suite under Application
Verifier / page-heap for an exact faulting address.
**Purging ships OPT-IN (`purge_delay` default −1)** — not because purge is the
cause (it isn't), but because it widens the state space while the defect is
open. Documented divergence from the oracle's default of 10.

**Gates:** clippy `-D warnings` + fmt clean; Linux 21 suites / 0 failures;
Windows all-features 5/5 clean in the last sweep but 1/5 AV in the default
build — **that is the blocker, and it is reported as such.**

**Tier-A corpus (WSL2, arm-interleaved, /usr/bin/time):** malloc-large **ra
2.50/3.04 s vs mi 5.02/3.95 — still ahead**; espresso ra 8.14/8.93 vs mi
9.11/6.60 (RSS **3456 KiB vs mi 10448**); larson wall parity (7.03–7.09 vs
7.06–7.13); cfrac **regressed to 8.65–12.87 s vs mi 6.24–6.32** — a real
M8-era regression on the small-alloc path, not yet diagnosed (the box was also
running the Windows stress concurrently, so this number needs a quiet re-run
before it is acted on: measure-first discipline, not a fix-first reflex).

**v1 gate status:** API parity ✅ (~150 of ~157 functions), corpus runs ✅,
hardening ✅, **stability ❌ (open P0)**, perf gate **not yet assessable** —
the geomean claim cannot be made while a corruption is open and cfrac is
unexplained. M8 is therefore NOT complete; the remaining work is the defect
hunt, then the perf campaign.

## M6+M7 — First-class heaps, arenas, subprocs; options, stats, hooks, the
## override crate and the Tier-A corpus as `ra` (2026-08-05)

**M6 landed:** first-class heaps as separately-allocated HeapBoxes with
**owner routing via the page's `xheap` back-pointer** (container-of over the
box's offset-0 delayed list — `free` now finds the OWNING heap, correct with
many heaps per thread); huge segments tracked per-heap and unified with the
delayed protocol (remote huge frees ride the DELAYED path; abandonment/adopt
cover Huge kind); `heap_new/_ex/_in_arena`, `delete` (segments absorbed into
the backing heap via adopt — blocks stay valid), `destroy` (wholesale release,
NEVER-spinning teardown), `set_default`/`get_backing`; heap_* alloc family
(FFI: ~45 heap exports); visitors (`mi_heap_visit_blocks` with free-bitmap
block enumeration, `mi_abandoned_visit_blocks` under the list lock),
contains/check_owned, page_under_utilized; **arenas v1** (segment-granular
chunk pools, used+dirty bitmaps, exclusive arenas, `manage_os_memory`,
`arena_area`, huge-page reserves as large-page arenas); **subprocs**
(per-subproc abandoned lists — isolation verified); page heap tags surviving
abandonment. Rust `Heap` type (delete-on-drop / destroy-on-drop).

**M7 landed:** the full 38-slot option table (ABI index-compatible,
`MIMALLOC_*`/`RUSTY_ALLOC_*` env parsing), registered hooks
(output/error/deferred-free — the heartbeat fires it), stats
(per-heap merged-on-read across the heap registry, process/thread prints,
`mi_process_info` via GetProcessTimes+K32/getrusage+statm), realpath/dupenv/
wcsdup/mbsdup, the C++ `mi_new` family (documented divergence: no
`std::get_new_handler`), **the override crate** (unix-only exports: malloc
family + posix + Itanium-mangled C++ operator new/delete incl. sized+aligned),
`include/rusty_mimalloc.h`, and Tier-A runner scripts.

**THE GATE THAT MATTERS — real C programs on our allocator via LD_PRELOAD**
(WSL2, /usr/bin/time, arm-interleaved, 2 runs/arm):
- cfrac: ra 5.95/8.63 s vs mi 7.47/7.15 vs glibc 6.32/9.06 — parity with the
  oracle; **RSS 3456 KiB vs mi 4312**.
- espresso: user-time parity (ra 6.39–6.68 vs mi 6.44–7.03); **RSS 3264 KiB vs
  mi 10448**.
- larson (real 100-thread-class bench, 8 workers): wall parity (ra 7.04/7.09
  vs mi 7.15/7.77); RSS ra ~90–100 MB vs mi ~77–83 MB (retention policy).
- **malloc-large found a REAL defect**: 3–4× slower than mi, sys-time-bound —
  large/huge alloc-free cycles round-tripped the OS. TWO fixes, both
  mimalloc's own shape: (1) lazily reserve the DEFAULT 1 GiB arena
  (`arena_reserve`) so segments recycle through chunks; (2) serve HUGE blocks
  from arenas too (contiguous multi-chunk claim under a small lock). Result:
  **ra 2.24/2.66 s vs mi 4.45/4.18/5.68 — flipped to ~1.8× FASTER than the
  oracle.** Sys time 15.7 s → 0.5 s. RSS +20% vs mi (recycled chunks stay
  committed — purge wiring is the RSS lever, still open).

**Miri earned its keep AGAIN, same law twice**: the arena stored its base as
`usize` — the 1 GiB region became unreachable-by-pointer and the default-arena
reservation "leaked". Reachability (and provenance) follow POINTERS: base is
now `*mut u8` and chunk derivation uses `.add()`. That's the third time this
lesson fired (registry M4, arenas M7) — it is now a review checklist item.

**Gates green:** Windows full suite + Linux 19 suites/0 failures · the new
heaps gate (visitor counts exact, delete-migration contents verified, destroy,
exclusive-arena containment + recycled-chunk re-zeroing, subproc isolation,
options/env, stats/process_info) · miri clean (heaps gate included; subproc
section native-only — the mock's TLS dtors don't fire, documented) · G1/G2
unchanged and green · clippy `-D warnings` + fmt.

**Known divergences (documented, tracked):** arena chunks are
segment-granular (32 MiB; upstream is slice-granular); `_commit=false`
arena reserves still commit (eager model); NUMA recorded not enforced;
purge/decommit of free spans and arena chunks still pending (RSS);
`mi_stats_merge` is a no-op (merged-on-read); racy-by-design stats snapshot
(volatile read) pending an atomic-counters refactor; no `std::get_new_handler`.

## M5 — Aligned + POSIX + zero-preserving family (2026-08-05)

**Landed:** the API-completeness milestone, part 1 — ~34 new functions.
- **`aligned_at` with interior-pointer recovery** (the one real architecture
  piece): `(p+offset) % align == 0` via three tiers — natural fit through the
  bins (64 KiB-aligned areas ⇒ `bsize % align == 0` qualifies every block),
  oversize-and-adjust (interior pointer; page marked `has_aligned`, free and
  usable_size recover the block start by block arithmetic — works for binned,
  large-span, and adopted pages alike), and exact placement in dedicated huge
  segments (offset-aware, slack only when the offset actually shifts the
  boundary).
- Full §5.4 aligned family + §5.7 zero-preserving (`rezalloc`/`recalloc` +
  aligned/_at — resting on the invariant that a zalloc'd block is zero across
  its FULL usable extent, so moves zero exactly `[old_usable, new_usable)`),
  §5.5 `u*` block-size-returning variants, §5.11 POSIX core
  (posix_memalign with EINVAL/ENOMEM, memalign, valloc/pvalloc, aligned_alloc,
  reallocarray/reallocarr, cfree via the segment map, `_expand`,
  malloc_size/usable_size/good_size, sized frees with debug verification),
  `realloc_aligned(_at)`.
- Harness: trace gen emits ~15% aligned allocations (16 B–64 KiB) through the
  `align_log2` field the format carried since v0; the system arm allocates/
  frees through matching Layouts; realloc is restricted to natural-alignment
  blocks (the C contract). New gates: `aligned_at_offsets` (all tiers),
  `rezalloc_grows_zero`, `align_storm` (randomized aligned churn + canaries,
  interior-free recovery hammered).

**Gates green:** Windows + Linux full suites (18 result rows) · G1 CLEAN on
1M-op traces WITH aligned ops on both arms, strict leak gate 527 465 == 527 465
· miri clean over the new paths (112 s, all 11 alloc_core tests interpreted) ·
loom untouched (protocol unchanged) · clippy `-D warnings` + fmt.

**Notes:** `realloc` does not preserve >8 alignment (per the C contract) —
gen/replay encode that; `realloc_aligned_at` exists for callers who need it.
Sized-free fast-path exploitation (skip the page walk) is an M8 brick; M5
verifies the size under debug only. `mi_realpath`/`mi_dupenv_s`/wide-char
helpers remain M7 (they are I/O, not allocation).

## M4 — Per-thread heaps, lock-free cross-thread free, abandonment (2026-08-05)

**Landed:** the global lock is GONE. Per-thread heaps in os-allocated HeapBoxes
reached through a const-init `thread_local!` pointer (the R1-validated 0.3 ns
path; !Drop key ⇒ no allocation on access, no bootstrap recursion — heap
storage comes from the prim layer). `free` routes by `Segment::thread_id`:
owner → local path; else the **4-state xthread protocol**
(NORMAL/DELAYED/FREEING/NEVER packed into the page's atomic word with the list
head): full-queue and large pages sit DELAYED so remote frees nudge the owner's
delayed list (drained at heartbeat — that is what un-parks full pages whose
blocks died remotely); FREEING guards the heap deref against teardown; NEVER
covers abandonment. Thread exit (prim FLS/pthread destructor from M1): collect
→ retire → surviving pages to NEVER (spinning out FREEING) → drain delayed →
publish segments on the global abandoned list → release heap storage.
Allocation slow paths adopt abandoned segments before reserving OS memory.
Global heap registry (M7 stats walk + honest reachability). FFI: mi_collect,
mi_thread_init/done, mi_process_init/done, mi_thread_set_in_threadpool.

**Protocol verified by loom BEFORE implementation** (`tests/loom_xthread.rs`
is the spec): delayed-push vs abandon (the use-after-free the FREEING state
exists to prevent + block conservation), normal-push vs collect, park/unpark
vs remote. Preemption bound 2 locally; `LOOM_EXTENDED=1` → bound 3 in CI.
Loom immediately enforced its own hygiene: spin loops need `yield_now`, CAS
protocols need a preemption bound and branch budget.

**Defects caught by gates:** miri flagged heap boxes/segments as leaks after
the static→TLS move — root cause: the registry stored pointers as `usize`,
and REACHABILITY FOLLOWS POINTERS, NOT INTEGERS (AtomicPtr fixed it; the same
rule keeps our own provenance honest).

**Gates green:** Windows full suite + Linux full suite (18 result rows incl.
the new mleak test: 4 threads exit with 2 000 live blocks; contents survive
abandonment, frees from main route via NEVER, main's churn adopts segments) ·
G1 1M-op realloc trace with counters IDENTICAL to the locked M3 run (perfect
cross-milestone work parity) · miri clean · clippy `-D warnings` + fmt.

**MT kernels (in-process wall, quiet-box Windows):** larson 8 threads
**146 Mops/s** with 780k cross-thread frees; xmalloc (100% remote frees)
**51.8 Mops/s**. Canary-checked throughout.

**Measurement note:** the first post-M4 malloc-small readings (Win 27.9,
Linux 17.9) were taken while our own miri/loom/WSL gates saturated all 24
cores — both arms fell ~4× equally, ratios held (~2.3× Win). Discarded per the
go-find-the-process rule; quiet-box numbers below.

**Loom postscript:** the 3-thread abandon model exceeded loom's exploration
budgets twice (spin heuristics). Fix was MODEL DECOMPOSITION, not bigger
budgets: the UAF invariant needs exactly ONE remote vs the abandoner —
exhaustively explored, unbounded, in 3.7 s (4/4 models green). The 2-remote
wide-space variant is the `LOOM_EXTENDED=1` CI soak. Lesson: a protocol model
should be the smallest machine that can violate the invariant.

**Quiet-box numbers** (in-process wall, method lines printed; single-session,
pinned ABBA still owed for standing claims):
- Windows malloc-small: **ra 78.9 Mops/s** (locked M3: 67.9 → lock removal
  +16%) vs system 36.8 (2.14×). larson 8T: ra 202 Mops/s standalone;
  xmalloc all-remote: 70.1 Mops/s standalone.
- Windows cross-arm MT ratios: **NOT RESOLVED** — the box degraded mid-session
  (identical runs spread 10–87 Mops/s; likely rust-analyzer storm after the
  manifest edit — the check-what-your-edit-woke-up corollary). ABBA pairs
  taken, ratios 0.77–2.64, no verdict quoted. Needs a pinned quiet session.
- Linux (WSL, observational, 2 interleaved rounds, consistent direction):
  malloc-small ra 71.7 vs glibc 76.9 (0.93× single-threaded — glibc's tcache
  edges the still-unoptimized fast path). **larson 8T: ra 162.8/114.0 vs
  glibc 120.3/104.1 (ahead both rounds). xmalloc (100% cross-thread frees):
  ra 27.7/51.7 vs glibc 3.6/16.7 — 3–8× ahead, both rounds, the M4 protocol's
  designed win.**

**Deferred, tracked:** TSan MT fuzz (needs -Zbuild-std wiring, CI follow-up);
pinned cross-arm MT session on a quiet box; single-threaded fast-path polish
(M8 — the 0.93× vs tcache gap); `mi_stats_merge` over the heap registry (M7);
no_std profile (post-v1, needs a TLS story without std).

## M3 — Realloc, large pages, reclamation, segment map (2026-08-05)

**Landed:** span reclamation (per-segment first-fit free-span list with O(1)
left/right coalescing via first/last slot markers), page retire (empty pages
return their span; one page per queue stays warm), in-segment **large pages**
(64 KiB–16 MiB single-block spans, fresh-per-request, retire-on-free — span
reuse IS the recycle path), a **one-empty-segment cache** (without it every
large alloc/free cycle paid a 32 MiB OS round-trip), the **segment map** (1 bit
per 32 MiB window, 1 MiB BSS; `mi_is_in_heap_region`), the **realloc family**
(`realloc` in-place when still-fits-and-≥-half-used, `reallocn`, `reallocf`,
`expand`, `GlobalAlloc::realloc`), `mi_strdup`/`mi_strndup`, 8 new FFI exports,
Realloc in trace gen/replay with G1f prefix-preservation + G1g strict-leak
gates.

**Two real bugs caught by our own gates before shipping:**
1. **Bump-frontier give-back broke the zero invariant** — `span_free` merged
   freed spans back into the virgin bump region, whose allocations report
   `fresh = true`; recycled dirty memory then skipped zalloc's memset. Caught
   by the spans G1c test. Freed spans now never rejoin the frontier.
2. Freeing a segment's only page released the segment instantly, making
   free-then-alloc cycles reserve a fresh 32 MiB each time → the segment cache.

**Instrument lesson:** the recurring "exit 255 with all tests ok" ghost was the
harness, not the code — truncating cargo's output pipe (`Select-Object -First`)
breaks `$LASTEXITCODE`. Read exit codes from full pipes only.

**Gates green (both OSes):** core 8/8 + spans lifecycle (deterministic-counter
process: reuse without new segments, recycled-span re-zeroing, retire counts,
12 MiB coalesced fit, realloc in-place/move/shrink semantics, expand
never-moves, segment map yes/no) + selfhost 5/5 + G2 + **G1 on a 1M-op trace
with realloc**: 529 898 allocs == frees (strict leak gate), 9 952 spans
retired, 9 segments (3 freed — cache policy visible), 9 897 large + 1 050 huge
allocs, realloc 22 102 in-place / 77 899 moved · miri clean (coalescing
interpreted end-to-end) · clippy `-D warnings` + fmt.

**Numbers** (in-process wall, same seed, method lines printed; single-run —
pinned ABBA still owed): malloc-small **Windows ra 67.9 Mops/s vs system 28.1**
(M2 binary: 39.7 vs 17.6 — box conditions drifted too; ratio ~2.4×), **Linux
ra 71.2 vs glibc 71.1 — parity reached with the lock still in place** (M2:
57.1). Counters bit-identical across OSes (10 002 036 allocs = frees, generic
6.0%, 132 pages, 553 extends) — work parity + determinism hold. The M2→M3
speedup is plausibly retire keeping the hot page resident; treat as observed,
not confirmed until a pinned same-binary A/B.

**Deferred, tracked:** purge/decommit of free spans (RSS story, M7 options);
in-place realloc for large spans via span growth (M5-ish, currently copies);
aligned realloc (M5); `mi_realpath` (M7); huge-segment map bits cover the whole
reservation (done) but Normal segments assume ≤ 2⁴⁸ VA (LA57 = false-negative).

## M2 — Single-threaded core (2026-08-05)

**Landed:** the allocator exists. `bins.rs` (oracle-pinned geometry), `page.rs`
(three sharded free lists, lazy extension), `segment.rs` (32 MiB-aligned sliced
segments, eager commit, dedicated huge segments, ptr→page = mask + slice walk),
`heap.rs` (75 bin queues + full queue, direct table, `malloc_generic` heartbeat,
free with full-queue unpark), `alloc.rs` (global spin-locked heap — the M2
threading model, removed in M4), aligned-subset (natural-fit via bins, huge
fallback), `GlobalAlloc` impl, 12 `mi_*` FFI exports, G1 replayer + trace gen,
Tier-B `malloc-small` kernel, R1 `tls-spike`.

**G2 earned its keep twice** (differential vs oracle DLL/so, every size
1..=64 KiB, both OSes):
1. mimalloc's default is **MI_ALIGN2W**: wsizes ≤ 8 round to EVEN word counts —
   bins 24/40/56 B don't exist (that's how 16-byte max_align_t is guaranteed).
   My from-paper formula had them. 49k mismatches → 0.
2. The binned cutoff is **64 KiB** (`MEDIUM_PAGE_SIZE/8`), not 128 KiB (/4);
   above it `good_size` is page-rounded.

**Other defects caught by gates before they shipped:** zalloc returned blocks
whose first word held the free-list link (upstream zeroes exactly that word —
now we do); test-vs-test races on the shared heap (fixed test design, kept the
stress value); Windows DLL dependency resolution needs absolute paths +
redirect preload; **mixed-OS cmake caches corrupt both oracle builds** →
OS-namespaced `oracle/out/{win,linux}` (scripts + docs updated).

**Gates green:** core 7/7 + lib 5/5 on Windows AND Linux · G1 replay CLEAN on a
1M-op synthetic trace (alignment/usable/zero/canary/overlap) on both arms ·
G2 PASS both OSes · **selfhost 5/5 both OSes** (rusty_alloc as the test
binary's real `#[global_allocator]`: HashMap/BTreeMap churn, Vec grow/shrink,
cross-thread frees, 40 MiB boxes) · **miri clean over the whole core**
(segments, mask trick via `with_addr`, free lists) · clippy `-D warnings` +
fmt clean · every `unsafe` block carries its SAFETY invariant.

**First numbers** (Tier-B `malloc-small`, in-process wall, method lines in
output; standing claims await pinned ABBA vs oracle arms):
- Windows: **ra 39.7 Mops/s vs system 17.6** (2.26× ahead) — with the global lock.
- Linux: **ra 57.1 vs glibc 71.1** (0.80×) — glibc's lock-free tcache vs our
  locked fast path; this gap IS the M4 work item, not an M2 regression.
- Counters bit-identical across OSes (allocs 10 002 036 = frees; generic 6.0%;
  1 segment / 27 pages / 273 extends) — deterministic kernel, work-parity holds.

**R1 RESOLVED** (tls-spike, 100M accesses): `thread_local!` + `const` init =
**0.32 ns/access ≈ bare atomic load** (0.31) on Windows; 0.29 vs 0.26 on Linux.
OS-slot (FLS/pthread) path: 3.87 / 1.73 ns. M4 design: `thread_local! const`
for the heap pointer; prim TlsSlot only as the thread-exit destructor hook.
No nightly `#[thread_local]` needed.

**Deferred, tracked:** page retire/slice reclamation + realloc family (M3);
lock removal + xthread activation (M4); free-list encoding stays off to match
the oracle's release default (secure/debug feature, M8); rdtsc path profiler
skeleton (first optimization session).

## M1 — OS primitive layer (2026-08-05)

**Landed:** `prim/` (Windows VirtualAlloc backend incl. the aligned reserve-release-
re-reserve race-retry dance, large-page attempt + fallback, FLS-based TLS destructor,
QPC clock; unix mmap backend with over-allocate-and-trim alignment, MADV_DONTNEED
decommit, MADV_FREE reset, pthread_key TLS; miri mock with alloc registry), `os.rs`
(cached config, page rounding, `alloc_aligned`, purge policy).

**Gates:**
- Windows native: 9/9 integration tests (32 MiB segment alignment, fresh-zero pages,
  reserve→commit→write→decommit→zero cycle, reset stays accessible, protect
  round-trip, 50-case size×alignment sweep, thread ids, clock scale, **TLS dtor fires
  at thread exit with the stored value**, NUMA ≥ 1).
- Linux native (WSL2 Ubuntu, rustup stable): same suite, 9/9.
- miri (mock backend): 2/2 — caught a real defect before it ran anywhere: the mock's
  registry used a non-const `HashMap::new` in a static (cfg(miri)-only code stable
  never compiled). Fixed with `OnceLock`.
- clippy `-D warnings` clean both targets; fmt clean; `cargo check` green on
  x86_64-pc-windows-msvc and x86_64-unknown-linux-gnu.

**Notes for M2:** prim `commit` conservatively reports `is_zero = false` on both
platforms (range may span still-resident pages) — the page layer must track
per-page `is_zero` itself off fresh-mapping info, exactly as upstream does, or
`calloc` double-zeroes. Windows decommit needs recommit; Linux DONTNEED does not —
the purge accounting must carry `needs_recommit` per range.

## M0 — Scaffold + oracle + corpus (2026-08-05)

**Landed:** workspace (5 crates), oracle submodule pinned @ v2.4.5 (cde3f7a0) with
3-arm build scripts (mi/dmi/smi, built OK with cmake+MSVC), mimalloc-bench submodule,
`.ratrace` trace format v0 (round-trip tested), `bench/pinvs.ps1` compliant harness,
CI (fmt/clippy/check/test win+linux, cross-target, miri, oracle build), WSL2 doc.

**Gates:** 4/4 tests; clippy/fmt clean; **first differential gate passed** — oracle
`mimalloc.dll` and `rusty_alloc_ffi.dll` loaded in one process both report
`mi_version() == 20405`.

**Environment note:** WSL2 Ubuntu installed 2026-08-05 (24 cores). mimalloc-bench
needed `unzip` + `dos2unix` beyond the documented packages (shbench patch step) —
recorded in corpus/WSL2.md.

**Tier-A gate closed (2026-08-05):** full bench suite built in WSL2; `bench.sh mi
cfrac` runs (6.90 s wall / 6.77 s user / 7.3 MB RSS / 746 page-reclaims).

**Null-arm session, cfrac × mi (METHOD: WSL2 Ubuntu on /mnt/c, N=4 each arm):**
- unpinned: 6.80–12.26 s wall — **1.80× spread**; page-reclaims 743–750 (work
  parity holds, so it is scheduler migration, not the workload)
- `taskset -c 2 nice -n -5`: 7.34–9.31 s — 1.27× spread; minima agree (~6.8–7.3 s)

Verdict: best-of-N minima are usable for absolute floors; paired A/B on this box
needs pinning AND large N, and WSL2 numbers are dev-loop only — plan risk R5
confirmed empirically on day one. Every future WSL2 run states this method line.
