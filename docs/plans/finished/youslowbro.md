# You slow, bro? Where rusty_alloc loses, measured from a real consumer

**Status: RESOLVED 2026-09-24 — see §8.** Both findings fixed and measured on
this report's own probe: the huge `realloc` steps went from 1.41x / 1.23x
mimalloc at 0/6 to 1.01x (parity) and 0.97x (7/8), and start-up allocations
from 263 to 12, which is mimalloc's own count on the same probe. Candidate B
was the mechanism. §5 got its README line and a backlog entry.

Originally: measured, nothing implemented. · **Date:** 2026-09-24 ·
**Build:** `rusty_alloc-api` **2.2.0 from crates.io** (not this repo's HEAD,
which has uncommitted changes that were left untouched) · **Box:** Windows 11
Home, Intel i7-14650HX, 16 cores / 24 threads, 32 GB, **native Windows, not
WSL** · **Reference:** `mimalloc` 0.1.52 (`libmimalloc-sys` 0.1.49, built
from its C source) and the Windows system heap

This note comes from outside the team. `rusty_esp_sense`, a Janus host
pipeline built on candle and rayon, adopted rusty_alloc as its binary's
global allocator. Its benchmark got **7.5 % faster** (1,945 → 1,759 ms, 15/16
alternating pairs, z 3.50). While pricing that change we measured rusty_alloc
directly against mimalloc and the system heap on the allocation patterns
that pipeline produces. The short version: **you are not slow in general.**
There is one real loss, one start-up cost, and two memory notes worth
knowing.

---

## 0. The answer in one paragraph

Against mimalloc, rusty_alloc is at parity or better on 13 of the 16
workloads we ran, and clearly faster on three of them (cross-thread free of
224 B blocks 0.73×, 24-thread churn of 4 KB blocks 0.77×, repeated fresh
blocks below 32 MiB 0.81-0.85×). **The one real loss is `realloc` that
moves a HUGE block.** Growing a block that is already larger than a segment
costs **1.41×** mimalloc's time for 33 → 66 MB and **1.23×** for
64 → 128 MB, and loses every pair (0/6). The same step below the segment
size is *faster* than mimalloc (0.87×, 0.93×). A realistic `Vec` that grows
to 64 MB by `push` pays it: **1.23×, 0/6**. Three mechanisms are already
ruled out (§3). Separately, **the options pass makes 251 allocations through
the global allocator** on the first allocation of every process, where
mimalloc makes none, and its source line is identified (§4).

---

## 1. Method, and what makes a number admissible here

- **One probe source, three binaries.** A small Rust program runs each
  workload best-of-N and prints milliseconds. It is compiled three times: as
  `#[global_allocator]` it uses `rusty_alloc_api::RustyAlloc`,
  `mimalloc::MiMalloc` or `std::alloc::System`. All three sit behind the same
  thin counting wrapper, so the counts in §4 are like for like. Source:
  `F:/janus-data/raprobe/`, `cargo build --release --features rusty|mi`.
- **Alternating runs.** The binaries run in turn, 5 or 6 rounds each. The
  verdict is the median of each arm's per-round best and the **paired win
  count**. A cell below is a verdict only if one side won at least 5 of 6
  (or 5 of 5).
- **Wall clock only, no instruction counts.** This is a Windows host with no
  callgrind. Every ranking here should be re-read on your deterministic
  instrument before anything is built.
- **The box was loaded.** Another process held the CPU at up to 100 %
  during these runs. Pairing cancels drift that both arms share; it does not
  make small effects admissible. Read anything within ±5 % that lacks a
  5/6 sweep as parity.

---

## 2. The numbers

### Speed against mimalloc (ratio = rusty_alloc / mimalloc; below 1 means rusty_alloc is faster)

| workload | mimalloc | rusty_alloc | ratio | rusty faster |
|---|---:|---:|---:|---:|
| 5,900 × 224 B kept, then freed | 0.245 ms | 0.228 ms | 0.931 | 4/5 |
| 5,900 × 11.2 KB kept, then freed | 10.6 ms | 10.8 ms | 1.017 | 2/5 |
| fresh 1 MB × 50 | 2.47 ms | 2.51 ms | 1.016 | 2/5 |
| fresh 8 MB × 20 | 8.51 ms | 8.37 ms | 0.983 | 4/5 |
| Vec growth by push to 8 MB × 10 | 10.3 ms | 10.5 ms | 1.020 | 1/5 |
| cross-thread free, 24 × 20k × 224 B | 17.4 ms | 12.6 ms | **0.725** | 5/5 |
| cross-thread free, 24 × 500 × 64 KB | 18.6 ms | 18.7 ms | 1.004 | 3/5 |
| 24-thread churn, 64 B | 70.0 ms | 70.9 ms | 1.012 | 3/5 |
| 24-thread churn, 4 KB | 45.1 ms | 34.9 ms | **0.774** | 5/5 |
| fresh 16/30 MB alternating (below a segment) | 12.8 ms | 10.9 ms | **0.846** | 6/6 |
| **realloc step 8 → 16 MB** | 0.726 ms | 0.628 ms | 0.865 | 6/6 |
| **realloc step 16 → 32 MB** | 1.578 ms | 1.466 ms | 0.929 | 5/6 |
| **realloc step 33 → 66 MB** | 4.552 ms | 6.405 ms | **1.407** | **0/6** |
| **realloc step 64 → 128 MB** | 8.489 ms | 10.466 ms | **1.233** | **0/6** |
| **Vec growth by push to 64 MB × 3** | 30.2 ms | 37.2 ms | **1.231** | **0/6** |
| fresh huge 33-128 MB, per buffer | 2.06-8.83 ms | 2.14-9.08 ms | 1.01-1.04 | 1-2/6 |

Against the Windows system heap, rusty_alloc wins every workload above, from
0.05× to 0.67×. For a Windows consumer that is the headline, and it is why
`rusty_esp_sense` ships it.

### Memory (working set from `K32GetProcessMemoryInfo`)

| probe | system | mimalloc | rusty_alloc |
|---|---:|---:|---:|
| 12 × 38 MB live | 460 MB | 461 MB | 461 MB |
| …all freed | **4 MB** | 461 MB | 461 MB |
| …500 ms later | 4 MB | 461 MB | 462 MB |
| then 45 MB of 224 B blocks live | 54 MB | 491 MB | 494 MB |

---

## 3. Finding 1: realloc that MOVES a huge block is 1.23-1.41× mimalloc

**What is established:**

- It is specific to growing a block that is **already huge**, larger than a
  32 MiB segment. The same step entirely below the segment size is faster
  than mimalloc (8 → 16 MB 0.865×, 16 → 32 MB 0.929×, 6/6 and 5/6).
- It is not the policy. `alloc.rs::realloc` keeps a block in place only
  when `newsize <= usable && newsize >= usable / 2`, and otherwise mallocs,
  copies and frees. That is upstream's generic shape too.

**Ruled out**, each with its own single-variable probe at 6 pairs:

1. **Fresh huge allocation is not the cost.** Fresh 33, 38, 64 and 128 MB
   blocks, written and freed, run 1.01-1.04× (1-2/6): near parity, at most a
   few percent.
2. **Reuse of freed huge space across sizes is not the cost.** Fresh blocks
   alternating 33/66 MB run 1.013× (2/6), against 0.996× for one size
   repeated.
3. **Several live huge blocks freed together is not the cost.** Two live
   (33 + 66 MB) run 0.964× (4/6), and four live 0.979× (4/6). That is a
   realloc's footprint without the realloc, and it runs at parity.

So the gap lives inside `realloc`'s own moving path for a huge block.

**Candidates, in the order we would test them:**

- **A. The copy itself.** The move is `copy_nonoverlapping(p, np, usable.min(newsize))`
  over 33-64 MB, which on Windows is the MSVC runtime's `memcpy`. Upstream
  copies through `_mi_memcpy`. Time the two copies alone at 33 MB and 64 MB
  into a freshly committed destination. If rusty's copy is slower there, the
  fix is a copy strategy chosen by size: `rep movsb` where the CPU has ERMS
  or FSRM, and no non-temporal stores into a destination that is about to be
  written again.
- **B. How much is copied.** `usable.min(newsize)` copies the whole
  *usable* size of the old block. If a huge block's usable size is rounded
  well past what was requested (to a slice or segment granularity), the
  copy moves more bytes than the caller ever wrote. Compare
  `usable_size(p)` with the requested size for 33 MB and 64 MB blocks, and
  compare with upstream's `mi_usable_size`.
- **C. Growing a huge block where it stands.** If the huge segment's
  reservation, or the address space right after it, has room, a huge block
  can grow by committing more pages instead of moving. That removes the copy
  and the fresh destination's page faults together. Check whether upstream
  2.2.x does anything of the kind for huge pages on Windows before deciding
  it is a feature gap rather than a defect.
- **D. The huge-block paths realloc takes on the way.** `usable_size` takes
  `usable_size_slow` for huge pages, and `free_inline` frees a huge segment.
  Both are cheap in isolation (probe 3 frees huge blocks at parity), but
  count them on your instrument inside `realloc` for completeness.

**Why it matters to consumers:** a `Vec` that grows past 32 MiB by `push`
(buffered file reads, collecting a large result) crosses exactly this path
once per doubling above the segment size. That is 1.23×, 0/6, measured.

---

## 4. Finding 2: 251 start-up allocations re-enter the global allocator

On the first allocation of a process, rusty_alloc makes **251 allocations
through the global allocator** before returning. mimalloc and the system
heap make **none**. The same probe, behind the same counting wrapper, reads
261 against 10. In `rusty_esp_sense` this showed up as a constant +251
allocations (+9 KB) in every command's census, `run` included.

**Mechanism** (`crates/rusty_alloc/src/options.rs`, the `std` environment
pass, around lines 352-356):

```rust
for i in 0..OPTION_COUNT {                      // 38 options
    let name = OPTION_NAMES[i].to_uppercase();  // a String
    let val = std::env::var(std::format!("RUSTY_ALLOC_{name}"))   // a String + env lookup
        .or_else(|_| std::env::var(std::format!("MIMALLOC_{name}")))  // another pair
        ...
```

That is 38 options × about 6.6 allocations each: the uppercase name, two
formatted keys, and two `std::env::var` calls. On Windows each call converts
the key to UTF-16 and the value back, which allocates too. The total is 251.

**Why it is worth fixing, beyond the count:**

- It is allocator re-entrancy during initialisation. Every one of those
  allocations lands in the heap that is still being set up.
- It is a start-up cost paid by every process, including the short CLI runs
  that are most of a command-line tool's life.
- You already deleted this same pass on `no_std` and on
  `wasm32-unknown-unknown`, for size. This is the same problem on hosted
  targets, for time and re-entrancy.

**Fixes, cheapest first:**

1. **Precompute the uppercase names** as a `const` table beside
   `OPTION_NAMES`, and build each key in a stack buffer with no `format!`.
   That removes about three of the six or seven allocations per option.
2. **Read the environment once, not 76 times.** Walk the environment block
   a single time (on Windows `GetEnvironmentStringsW`, which the `windows`
   primitives can already reach) and match each entry against the two
   prefixes. Parse only the entries that match, usually none. That takes
   the pass from 76 lookups to one walk, and from about 251 allocations to
   zero.
3. **Or make the pass lazy.** Only the options actually read on the hot
   path need the environment at first allocation; the rest can resolve on
   their first `get`.

The deterministic check: the counting-wrapper probe should read 10 at start-up,
the same as mimalloc.

---

## 5. Two memory notes (shared with upstream, not defects against it)

- **Freed memory is not returned, by design.** `purge_delay` defaults to -1
  (purging is opt-in). After 456 MB of huge blocks are freed, the working
  set stays at 461 MB, and at 462 MB 500 ms later. mimalloc behaves the same
  on this box. The system heap returns to 4 MB. In `rusty_esp_sense` the
  peak working set rose 25-40 %: 115 → 144 MB single-threaded, 509 → 717 MB
  for the raw-window run at 24 threads. That is a fair trade, and the
  consumer ships it behind a feature flag, but a short "memory will look
  larger" line in the README would save the next adopter a surprise.
- **Freed huge segments are not recycled for small objects.** After those
  456 MB are freed, 45 MB of 224 B blocks raise the working set to 494 MB,
  where the freed space could have held them. mimalloc does the same (491 MB)
  so this is not a regression against upstream. It is the obvious next step
  if the team ever wants to beat upstream on footprint as well as speed:
  carve small-object segments out of retained huge ones.

---

## 6. Where you win, for the record

- **Cross-thread free of small blocks: 0.725× mimalloc, 5/5.** This is
  rayon's collect-then-drop pattern: workers allocate and the main thread
  frees. It is the pattern a data-parallel pipeline hits most.
- **24-thread churn of 4 KB blocks: 0.774×, 5/5.**
- **Repeated fresh blocks below a segment: 0.81-0.85×, 6/6.**
- **Against the Windows system heap: 0.05-0.67× on every workload**, and
  7.5 % off a whole candle + rayon benchmark end to end.

---

## 7. What we would do first

1. **§4, the options pass.** It is small, deterministic to verify (start-up
   allocations 261 → 10) and removes allocator re-entrancy during
   initialisation.
2. **§3, candidates A and B**, on the deterministic instrument: price the
   huge copy and the bytes it moves. Only if both come back at parity, look
   at C, growing huge blocks in place.
3. **§5**, a README line, and a backlog entry for recycling retained huge
   segments.

---

## 8. Executed (2026-09-24)

Both fixes were built against the tree at `c9631f4` and measured on this
report's own probe (`F:/janus-data/raprobe`, copied to scratch twice and
patched with `[patch.crates-io]` to link a worktree of that commit and the
fixed tree; a third copy links mimalloc 0.1.52). The full record, with the
method line and every raw per-round value, is the LEDGER entry of the same
date; this section is the report's own questions, answered in its own order.

### §3 — it was candidate B

- **A, the copy itself: no.** Both sides call the platform `memcpy`; nothing
  about the copy changed and nothing needed to.
- **B, how much is copied: yes, and it was twice what the caller wrote.**
  `segment::huge_alloc` reserves in whole 32 MiB chunks through the arena —
  a 33 MB request holds a 64 MiB chunk pair, a 64 MB one holds 96 MiB — and
  then set the page's `block_size` to that whole reservation minus the
  header. `usable_size(p)` is the length `realloc` copies when it moves, so
  growing 33 MB copied and first-touched 64 MiB on both sides, and growing
  64 MB copied 96 MiB. The report's arithmetic already fit that shape: the
  two steps below the segment size have no such slack and were faster than
  mimalloc. The page now reports `align_up(size, SEGMENT_SLICE_SIZE)`
  capped at the reservation — upstream's `psize`, and what `mi_usable_size`
  returns there. The reservation itself is unchanged. `zalloc` on a
  recycled chunk zeroed the same extent and is fixed by the same line.
- **C, growing in place: not built, recorded.** The fat usable size *was* an
  in-place grow for anything that fit inside the slack; the fix removes it,
  as upstream never had it. Keeping it deliberately costs a flags load and a
  branch on every moving `realloc` (the whole of the deterministic `realloc`
  op) for a sub-2x grow of a block already above 32 MiB, which the doublings
  a `Vec` performs never are. `docs/opps.md` #10 has the hook and the
  arithmetic.
- **D, the paths on the way: no.** `usable_size_slow` and `free_inline` on
  a huge block are unchanged; the report's probe 3 had already priced them
  at parity.

Deterministic check first (`tests/alloc_core.rs::huge_usable_size_is_the_request_not_the_reservation`):
usable of a 33 MiB block was 67,043,328 bytes (64 MiB − 64 KiB) and is now
within one slice of 34,603,008; a 64 MiB block reported 96 MiB − 64 KiB and
now reports 64 MiB. Static instruction counts from the emitted x86-64 assembly:
`realloc`, `usable_size`, `free` and `page_extend` byte-for-byte the same
length (163 / 17 / 66 / 42); `huge_alloc` 199 → 203, cold.

Then the clock, on this probe, pinned to one core at High priority, whole
processes ABBA-alternated with the leading arm swapped each round, median of
the per-run bests, paired wins, z. The box was at 62–76 % load from another
process, so a null arm (one binary in both arms, 6 rounds) set the floor:
median ratios 0.89–1.03, no |z| ≥ 2.

| workload | 2.2.0 tree | fixed | ratio | wins | z |
|---|---:|---:|---:|---:|---:|
| realloc step 33 → 66 MB | 7.707 ms | **5.243 ms** | **0.680** | **8/8** | +2.83 |
| realloc step 64 → 128 MB | 13.237 ms | **10.152 ms** | **0.767** | **8/8** | +2.83 |
| Vec growth by push to 64 MB × 3 | 45.201 ms | **36.673 ms** | **0.811** | **6/6** | +2.45 |
| realloc step 8 → 16 / 16 → 32 MB | | | 0.975 / 0.923 | 5/8, 6/8 | floor |
| fresh 16–128 MB, per buffer | | | 0.97–1.06 | 1–4/6 | floor |

And against mimalloc, which is what §2 asked:

| workload | §2 (before) | now | wins now | z |
|---|---:|---:|---:|---:|
| realloc step 33 → 66 MB | 1.407, 0/6 | **1.013** | 5/8 | +0.71 (parity) |
| realloc step 64 → 128 MB | 1.233, 0/6 | **0.969** | 7/8 | +2.12 (faster) |
| Vec growth by push to 64 MB × 3 | 1.231, 0/6 | **1.015** | 3/6 | 0.00 (parity) |
| fresh 30–64 MB, per buffer | 1.01–1.04 | 1.02–1.06 | 0–3/6 | unchanged: the fix does not touch this path (tree vs tree 3/6) |

### §4 — zero allocations, the same 76 lookups

Fixes 1 and 2 from the list, combined and without the single environment
walk: the key is built by hand in a stack buffer whose size is a `const` over
the table (`RUSTY_ALLOC_` + the longest name + NUL), the value lands in a
64-byte stack buffer through a new `prim::getenv` — `libc::getenv` on unix and
`GetEnvironmentVariableA` on Windows, the two calls upstream's own prim makes
— and the value grammar is parsed on those bytes in place. One lookup per
prefix per option as before, 76 in all, and none of them owns memory. The
Windows API converts the name to UTF-16 on the process heap, which a
counting wrapper on the global allocator does not and should not see.
wasm32-unknown-unknown keeps the pass compiled out (its size ratchet is
unmoved by construction); Miri and wasm32-wasip1 keep a `std::env` fallback.

The deterministic check this section asked for, on this probe's counting
wrapper: **`startup` read 263 on 2.2.0 and reads 12 now; mimalloc reads 12.**
Two tests keep it: `tests/options_env.rs` sets both prefixes, a precedence
conflict, `YES` and a KiB size in a child process and reads them back through
`options::get`; `rusty_alloc_api/tests/reentrancy.rs` wraps `RustyAlloc` in a
depth-counting `GlobalAlloc` and fails if the allocator ever allocates through
the global allocator while serving a request (it read 0 of 395 calls; it
would have read 251).

### §5 — the README line, and what the backlog entry says

The README's Features section has a *Memory* paragraph: freed memory stays
committed by default (`purge_delay` = -1, as upstream ships), the working set
reads near its past peak, and the two environment variables or
`options::set(15, …)` that turn purging on. This probe's `retain` reads the
same before and after — 461 MB retained, 492 MB with the 224 B blocks live —
because nothing here touched retention.

The second note is backlog `docs/opps.md` #11, with one correction to the
report's framing: the freed huge chunks DO come back through the arena bitmap
and a fresh small-object segment takes one, so a recycler is not what is
missing. What raises the working set is first-touch of the pages the huge
block never wrote (a 38 MB block leaves 26 MB of its 64 MiB pair untouched).
Preferring the touched prefix when carving is the lever, and its instrument
is this probe's working-set number, not an instruction count.

### Gates

`cargo test -p rusty_alloc` 144/0 at default features, plus `debug_checks`
and `secure`; the `rusty_alloc-api` suite with the new re-entrancy test;
clippy clean on the new code; `cargo fmt --check`; `tools/unsafe-census.sh
--update` (928 → 931: one FFI block per prim backend, one `set_var` in a
test, each rowed in `UNSAFE.md`). Not run here: callgrind (no Linux box) —
the icount farm re-reads the `realloc` and `huge` opscan ops, where the
static counts predict 0 and +4.
