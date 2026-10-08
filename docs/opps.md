# Allocator optimization opportunities

Instruction-reduction candidates in the allocator hot paths, found by reading
the code on 2026-08-20. **These are candidates to MEASURE, not confirmed wins.**
The house rule holds: nothing here ships without a callgrind measurement that
clears the noise floor (`bench/icount-arms.sh` / `bench/opscan.sh`, ABBA, null
arm, work-count parity). A candidate that reads flat or negative gets reverted
and its entry annotated with the refutation — a refuted idea is as valuable as a
confirmed one, because it stops the next person retrying it.

## Framing

The default build is **byte-identical** to before the 2026-08-20 security work —
every `secure` / `linkcheck` / `blockmap` feature ships OFF, confirmed by opscan
(small 79.38, batch_lifo 60.17, mixed 140.07). So this is not regression
recovery; these are genuinely new opportunities.

The one measured loss vs mimalloc is **batch_lifo / batch_fifo** (+0.47 / +0.48
Ir/op, ratio 1.008) — pure alloc-then-free churn that runs through the generic
refill and `page_collect` paths. Everything else is at or ahead of mimalloc.
Candidates are graded by how directly they target that loss and how cheap they
are to try.

| # | Location | Tier | Targets | Status |
|---|---|---|---|---|
| 1 | `page_index` division on the refill path | 1 | batch loss | **CLOSED 2026-08-22 — the divide itself is gone, at zero header cost** |
| 2 | `stat_realloc` re-resolves the heap per realloc | 1 | realloc | **LANDED — realloc −14.16 Ir/op** |
| 3 | Emitted-bounds-check audit of the hot paths | 2 | unknown | **DONE — fast paths already bounds-check-free** |
| 4 | `update_direct` recomputes `bin_size` twice | 2 | batch loss | **ASSESSED — premise false, the cheap version is already there** |
| 5 | `zero_block` re-resolves `usable_size` (calloc) | 2 | calloc | **LANDED — calloc 152.62 → 132.00, −20.62 Ir/op** |
| 6 | Batch-op gap → traced to free's `used--` codegen | 3 | batch loss | **CLOSED as a codegen floor (refuted)** |
| 7 | `wait_no_remote_in_flight` 512-slot scan | 3 | latency | **LANDED — bounded to the carved region** |
| 8 | Collect-loop double `block_next` | — | batch loss | **banked (landed)** |
| 9 | `process_delayed` swaps an empty list every slow-path alloc | 3 | slow path | **LANDED — load-before-swap** |
| 10 | Huge `realloc` grows in place inside the reservation's slack | 3 | huge realloc | **NOT BUILT (2026-09-24) — no measured workload reaches it; a load and a branch on every moving realloc** |
| 11 | Recycle retained huge chunks' untouched tail for small objects | 3 | footprint | **OPEN — backlog from `docs/plans/youslowbro.md` §5** |

---

## Tier 1 — targeted at the known loss, cheap to try

### 1. `page_index` division on the refill path

**Where:** [heap.rs:281](../crates/rusty_alloc/src/heap.rs#L281),
[heap.rs:313](../crates/rusty_alloc/src/heap.rs#L313),
[heap.rs:474](../crates/rusty_alloc/src/heap.rs#L474)

`page_index(seg, page)` is `(page.addr() - base.addr()) / size_of::<Page>()` — a
real hardware division by a **non-power-of-two**. All three call sites feed the
result straight back into `page_area(seg, idx)` (`seg + idx * SLICE_SIZE`), so
each does a divide *and* a multiply to recover an address that is a function of
`page`. These sites are on the generic **refill** path, which is exactly where
batch_lifo / batch_fifo spend their time.

**Fix under test:** store the page's slice index once, at carve time, in a
spare `Page` field; `area = seg + (idx << SLICE_SHIFT)` becomes a load and a
shift. `SEGMENT_SLICE_SIZE` is 2^16, so the shift is exact.

**Foreclosed alternative, stated so nobody re-proposes it:** padding `Page` to a
power of two would turn *every* `page_index` divide into a shift, but the
segment header must fit in one 64 KiB slice (`const _: () = assert!(size_of::
<Segment>() <= SEGMENT_SLICE_SIZE)` in segment.rs), and `[Page; 512]` at a
128-byte stride would consume the whole slice with no room for the rest of the
header. The division exists *because* of that constraint.

**Risk:** header space. A `u16` index costs 512 × 2 = 1 KiB of header; must be
verified against the fit assertion in the default build AND under
`secure` + `blockmap` (the tightest config) before committing.

**Measure:** opscan batch_lifo / batch_fifo (the target), plus small / mixed as
regression guards, and the three real-program arms.

**CLOSED 2026-08-22.** The `page_off` table removed `page_index` from `page_of`
(the whole-program win already recorded above), but the division itself
survived at all five remaining call sites, emitting `movabs
$0x2e8ba2e8ba2e8ba3; mul; shr` — a 3-instruction magic multiply with a 10-byte
immediate — because 88 is `8 * 11`.

**It needed no new field.** The fix proposed above was to store the slice index
in a spare `Page` slot, and the risk recorded above was the 1 KiB of header
that would cost. Neither was necessary: `Page::area` **already** holds
`seg + idx * SEGMENT_SLICE_SIZE`, written once at carve, and
`SEGMENT_SLICE_SIZE` is 2^16. So the index comes back as

```rust
let idx = ((*page).area.addr() - seg.addr()) / SEGMENT_SLICE_SIZE; // a shift
```

with the slot-pointer form kept as a `debug_assert`. Zero header cost, and the
"foreclosed alternative" about padding `Page` to a power of two stays
foreclosed and stays irrelevant.

Two things had to be true and one of them was not:

- `span_mark` writes `area` for **every** carved span, free or allocated — so
  the free-span walkers at segment.rs:525 and :657 were already safe.
- `huge_alloc` builds its single slot by hand and **never wrote `area`**, so
  huge pages carried a null. That is fixed in the same change, and it was a
  latent trap for anything else reaching for the field: `unalign` reads it now,
  and huge pages set `SINGLE_BLOCK`, so they do not take its early return.

Result: five magic multiplies to zero, cfrac **-17,057 Ir**, `huge` +5.00 Ir/op
against mimalloc's 53,362. Full detail in `docs/plans/fast-trans.md`.

### 2. `stat_realloc` re-resolves the heap on every realloc

**Where:** [alloc.rs:125-133](../crates/rusty_alloc/src/alloc.rs#L125-L133),
six callers.

The plain `realloc` path calls `my_heap()` — a TLS load, a null check, and a
`heap.get()` — **purely to bump a counter**. The realloc decision itself needs
only `usable_size(p)`, never the owning heap.

Behind it is an inconsistency: `allocs` / `frees` are `#[cfg(debug_assertions)]`
(upstream's MI_STAT philosophy, the same gating the release-test fixes leaned
on), but `realloc_in_place` / `realloc_moved` are always-on. Gating them to
match removes the entire TLS resolution from every release-mode realloc.

**Risk:** the release `Stats` API loses two counters (they read 0 in release,
exactly as `allocs` already does). That trade was accepted for `allocs`/`frees`,
and the tests that depended on it are already fixed. `heap_realloc` (the
first-class-heap variant) bumps the counter cheaply from the `hb` it already
holds — that path is unaffected.

**Measure:** opscan realloc (the target) + a null arm; whole-program perl /
sqlite as the realloc-heavy real workloads.

---

## Tier 2 — real but smaller, or needs a probe first

### 3. Emitted-bounds-check audit of the hot paths — DONE

**Result (2026-08-20): the shipped hot paths are already bounds-check-free.**

`cargo rustc --release --emit asm` reports **39** `panic_bounds_check` sites
across the whole lib. Attributed to symbols, then cross-checked by
disassembling the shipped override `.so`:

- **`malloc`, `free`, `calloc` exported fast paths: ZERO** bounds checks / panics
  (confirmed directly on the `.so`).
- The 39 are all on slow/cold paths: `span_alloc` 6, `span_free` 5,
  `visit_segment_blocks` 3, `adopt_segment` 3, arena management 7, thread
  teardown, guarded sampling, and **2 on the `malloc_generic` refill path**.

So there is no hot-path bounds-check tax to remove — the safe-Rust allocator
runs check-free exactly where it matters, the same result the
`rusty-unsafe-optimizations` h264 case study found. The 2 refill checks resisted
a `.min(MAX_NORMAL_BIN)` clamp on `bin` (count unchanged), sit on a cold path,
and are not worth chasing further; recorded here so the dead end is not
re-explored.

### 4. `update_direct` recomputes `bin_size` twice per refill — ASSESSED, premise false

**Verdict (2026-08-20):** not worth doing; the recompute is already the cheap
option on the hot path.

The premise was that `bin_size(bin)` and `bin_size(bin-1)` are costly arithmetic
worth precomputing into a table. On inspection:

- `bin_size(bin)` for `bin <= 8` — the common small-allocation bins — is
  literally `bin * INTPTR_SIZE`, i.e. `bin << 3`, a single shift.
- `bin_size(bin-1)` runs ONLY in the `w_lo` match's `_ =>` arm, which is reached
  only for `bin > 8` (larger, rarer allocations).

So on the hot path there is no expensive recompute to remove. And the proposed
fix makes it worse: a const table `T[bin]` or `self.pages[bin].block_size`
replaces a register shift with an **array load that carries a `panic_bounds_check`**
(`bin` comes from `bins::bin`, whose bit arithmetic LLVM cannot bound — the same
reason #3's refill checks survive a `.min(MAX_NORMAL_BIN)` clamp, which was tried
and changed the count by zero). Trading a proven-safe shift for a bounds-checked
load is a net loss. Left as-is.

### 5. `zero_block` re-resolves `usable_size` for recycled blocks (calloc)

**Where:** [alloc.rs:251](../crates/rusty_alloc/src/alloc.rs#L251)

The non-zero path calls `usable_size(p)` — segment mask → page resolve →
block_size load — to get a length the allocating page already knew. On the
calloc path (152 Ir/op, and where mimalloc is closest to us at 0.949), threading
the block size out of the allocation could save the re-resolution. Needs care:
the fast path returns only a pointer, so plumbing the size through without
growing the hot path's register pressure is the whole trick.

---

## Tier 3 — deterministic-latency, not instruction-hot

### 6. The batch-op gap — CLOSED 2026-08-22 (it was a safe-Rust floor, not a hardware one)

> **STATUS: CLOSED.** Everything below was correct about the cause and correct
> that no arrangement of *safe Rust* could reach it — the two refutations it
> records both stand. It was wrong only in the conclusion drawn from that.
> `used--` is now **two instructions**, matching mimalloc, written as the
> `sub dword ptr [pg + USED_OFFSET], 1` and `jle` that the hardware actually
> needs, with the retire arm as an `asm!` label block (`asm_goto`, stable since
> Rust 1.87). `free`'s fast path went 27 → 21 instructions against mimalloc's
> 25, and `batch_lifo`/`batch_fifo` from 1.008× to **0.89×**. The remaining
> row where upstream is cheaper is the atomic flags test, and that one is
> declined on purpose — see the README.
>
> Kept in full because the analysis is the reason the fix is the shape it is:
> LLVM will not emit a memory-destination read-modify-write when the
> decremented value must also drive a branch, and it will even re-test a value
> `dec` has already set the flags for. Read it as a record of how the floor was
> established, not as an open item.



**Where:** `free`'s `used` decrement, via [page.rs:page_push_local](../crates/rusty_alloc/src/page.rs#L625)
and the retire branch in [alloc.rs:free_inline](../crates/rusty_alloc/src/alloc.rs#L598).

The +0.47 Ir/op batch_lifo/batch_fifo gap — our one measured loss to mimalloc —
was localized on 2026-08-20 with a per-function callgrind profile + a direct
disassembly of both allocators' `free`. Findings:

- **malloc is at parity** (ra 15.99/op ≈ mi 15.99/op). The entire gap is in
  `free`.
- `free` fast path, executed instruction count: **ours 27, mimalloc 24** (excl.
  its CET `endbr64`). The +3 breaks down as, instruction for instruction:
  - **`used--` decrement: +3.** mimalloc emits `subw $1, [used]; je` — one
    memory-destination RMW whose flags feed the retire branch (2 insns). We emit
    `mov used→eax; dec; mov eax→used; test; jle` (5). LLVM will not select
    `dec [mem]; jle` because the decremented value must be in memory before the
    branch (the retire tail re-reads `used`) and also drive the branch.
  - **thread compare: +1.** mimalloc does `cmp %rcx, %fs:0` — the TLS
    self-pointer as a cmp memory operand. We do `mov %fs:0,%rcx` then `cmp`,
    because `thread_id()` is an inline-asm `mov fs:0` that forces a register.
  - **idx × sizeof(Page): −1** (we use one `imul`, mimalloc a `lea`+`shl`) — a
    place we are already tighter.

**Attempted and REFUTED (2026-08-20):** split the list-push from the `used`
decrement and inlined the decrement adjacent to the branch, the shape most
likely to trigger `dec [mem]`. Result: **byte-identical asm** — same
`mov/dec/mov/test/jle` at the same addresses. Reverted; the only artifact is a
NOTE at the decrement recording this so it is not retried. C's `--page->used <= 0`
gets `subw; je` from Clang; the equivalent Rust does not, and neither fold is
reachable without inline asm on the hottest path in the allocator.

**Conclusion:** the batch gap is a **Rust-vs-C instruction-selection floor**, not
an algorithmic deficiency. Both missing folds (memory-RMW decrement, fs-relative
cmp operand) are LLVM codegen choices we cannot steer from safe Rust. Closing the
last ~0.5% on this one synthetic op would take inline asm in `free` — not worth
it against a path we already win on every other op and match on real programs.
The earlier idea here (maintain the count at push time to avoid the collect walk)
is moot: the walk is not where the gap is.

### 7. `wait_no_remote_in_flight` 512-slot scan — LANDED

**Where:** [segment.rs:183](../crates/rusty_alloc/src/segment.rs#L183)

Was a fixed O(512) scan on every segment release, checking each slot's
`xthread_free` for an in-flight remote free (`XFLAG_FREEING`). But slots at or
after `next_free_slice` were **never carved**, so their `xthread_free` is still
0 — `& XMASK == XFLAG_NORMAL`, never `XFLAG_FREEING` — and scanning them is
guaranteed-idle work.

**Fix (2026-08-20):** bound the loop to the carved region
`[HEADER_SLICES, next_free_slice)`. A segment that only ever carved 10 slices now
scans 10, not 512 — the cost is proportional to how much of the segment was
used, not a fixed 512. A Huge segment sets `next_free_slice = SLICES_PER_SEGMENT`
so it is unaffected (correct — its one page spans the whole reservation).

Deterministic-latency win: the worst case is unchanged (a segment that filled
scans ~511) but the *typical* release — a lightly-used segment — is now far
cheaper and, more importantly, **bounded by actual use**. Correctness rests on
the never-carved invariant already documented at the top of `segment.rs`
("carved region `[HEADER_SLICES, next_free_slice)`"), and is exercised by the
loom-modeled cross-thread protocol test plus `stress_mt` / `teardown_reclaim` /
`abandon_rss` — all green.

---

## Found during the Tier 3 hunt

### 9. `process_delayed` drains an empty list with a locked swap — LANDED

**Where:** [heap.rs:727](../crates/rusty_alloc/src/heap.rs#L727)

`process_delayed` is the heartbeat's first duty — it runs on **every slow-path
allocation** (`malloc_generic`) to drain the heap's cross-thread delayed-free
list. It did so with an unconditional `head.swap(0, AcqRel)` — a LOCKed
read-modify-write — even when the list was empty, which it is on any thread that
never receives a cross-thread free (the common single-threaded case, and the
steady state of most others).

**Fix (2026-08-20):** peek with a plain `Acquire` LOAD first; only pay the swap
when there is actually a block to take. A push that races in after an empty peek
is drained on the next heartbeat — these frees are processed at heartbeat
cadence by design, never synchronously, so deferring one is already the
contract. Correctness rests on the loom-modeled cross-thread protocol, which
stays green.

**Measured:** big/large **−1.00 Ir/op** each (171 → 170), mixed **−0.72**
(140.07 → 139.35), calloc/med tiny, fast-path ops (small 79.37) unchanged — no
regressions. And the Ir count *understates* it: a locked `xchg` is a full
barrier (~20 cycles, dirties the cache line) where a load is ~4 — so the
wall-clock saving on every slow-path alloc is larger than the −1 Ir suggests.
Both an instruction win and a deterministic-latency win.

**Sibling check (curiosity discipline) — one rejected.** After #9 landed, the
other unconditional locked RMWs on warm paths were audited for the same
load-guard. The `flags.fetch_or(HAS_ALIGNED)` on the aligned-alloc path
([heap.rs:716](../crates/rusty_alloc/src/heap.rs#L716)) looked identical — a
sticky bit re-set on every adjusted aligned alloc — but load-guarding it is an
**Ir REGRESSION**: unlike #9's swap (whose early return skipped the drain
loop's register setup, netting −1 Ir), `fetch_or` is a leaf op with nothing to
skip, so a peek adds `load + test + je` (2-3 Ir) to save one `lock or` (1 Ir).
It would trade latency for instruction count — and this project's metric is Ir,
so it would read as `aligned` regressing in every opscan. Not shipped;
`page_collect`'s xthread steal was already load-guarded before it CASes, so #9
was the one place the pattern actually paid.

## From a consumer report — `docs/plans/youslowbro.md` (2026-09-24)

### 10. Huge `realloc` in place inside the reservation — NOT BUILT

**Where:** `alloc.rs::realloc`'s move arm; `segment.rs::huge_alloc`.

A huge block's reservation is chunk-rounded — 64 MiB for a 33 MB request, 96
MiB for 64 MB — so up to a chunk of committed, never-touched slack sits after
the block. Until 2026-09-24 the page REPORTED that slack as its usable size,
which made a `realloc` that fit inside it free and made every `realloc` past
it copy the whole slack: the 1.41x / 1.23x loss `youslowbro.md` §3 measured.
The fix reports the request (upstream's `psize`), and the in-place growth
went with it.

It could be kept deliberately: in the move arm, if the page is `HUGE_SEGMENT`
and `newsize <= total_size - (block - seg)`, bump `block_size` and return
`p`. The arithmetic against building it now: the growth that consumers hit
is a `Vec` doubling (33 -> 66 MB, 64 -> 128 MB), which never fits the slack
(capacity 64 MiB - 64 KiB and 96 MiB - 64 KiB respectively), so the only
beneficiary is a sub-2x grow of a block already above 32 MiB — a pattern no
report has named — while the test is a flags load and a branch on EVERY
moving `realloc`, which is the whole of the `realloc` opscan op (277 Ir/op,
all moves). Not built. If a workload with that shape appears, price it on
opscan `realloc` first; the hook is a `#[cold]` arm after the in-place test,
and `total_size` already holds the capacity.

### 11. Recycle a retained huge chunk's untouched tail for small objects — OPEN

**Where:** `arena.rs` chunk bitmap; `segment.rs::segment_alloc`.

`youslowbro.md` §5: after 456 MB of huge blocks are freed, 45 MB of 224-byte
blocks raise the working set from 461 to 494 MB — mimalloc reads 491, so it
is not a regression against upstream. The chunks DO come back through the
arena bitmap and a fresh segment takes one, so the recycling exists; what
rises is first-touch of the pages inside those chunks that the huge block
never wrote (a 38 MB block leaves 26 MB of its 64 MiB pair untouched). A
segment carved from a recycled chunk could prefer the touched prefix, or the
arena could hand a 64 MiB pair's touched half out first. Backlog: needs the
`retain` probe from the consumer's harness as its instrument, and a working
set number, not an instruction count, as its verdict.

## Vein census, 2026-10-07 (where the next ten deterministic wins are)

**Why a new census.** `bench/opscan.sh` (16 ops, two-point callgrind) now has
rusty_alloc AHEAD of mimalloc on every op it covers: small 52.25, med 56.50,
big 92.00, opscan "huge" (2 MiB) 293.00, calloc 93.19, batch 53.20, realloc
231.20, aligned 83.69, xthread 77.08 Ir/op. Four curiosity rounds mined those
paths; the oracle is no longer a target source there. The veins are on paths
opscan does not drive. A side probe (`F:/ra-corpus-tmp/veins/veins.c` +
`scan.sh`, same two-point method, pointers escaped through a `volatile` sink
because GCC elides an unused malloc/free pair, which first read a flat 30 Ir
for every allocator) measured them:

| op | rusty_alloc | mimalloc 2.4.5 | glibc |
|---|---:|---:|---:|
| 64 MiB malloc + free (true huge) | **53,538** | 53,608 | 384 |
| thread life (spawn, 32 small allocs, exit) | **63,977** | 101,460 | 15,920 |
| exited thread's 8 large blocks freed elsewhere, then reloaded | **76,050** | 172,867 | 11,773 |
| calloc 1 MiB + free | 57,812 | 110,746 | 57,805 |

Note: callgrind counts `rep stosb` once per BYTE, so memset-heavy rows overstate
cycles; the BYTES are still real (and first-touch faults with them). Rank by
bytes and calls, then confirm on whole-program Ir.

**Ranked veins, biggest first:**

1. **The 45 KB segment scrub.** `segment_alloc` and `huge_alloc` zero the whole
   `Segment` (512 `Page` slots x 88 B) whenever the chunk is recycled
   (`!mem_zero`), which after warm-up is always. It is 47 k of the 53.5 k Ir of a
   huge pair (a huge segment uses ONE page slot) and 47 k of the ~48 k Ir a thread
   life costs over glibc (each new thread's first segment is a recycled chunk).
   The comment beside it already says carved slots are re-initialised by
   `span_mark`. Lever: scrub the header fields and only the slots something reads
   before a carve (huge: slots 0-1). Gate: `debug_validate_segment`, the spans and
   adopt suites, miri. Ceiling: ~47 k Ir and 11 first-touched pages per segment.
2. **Large spans are carved and retired on every alloc/free.** The 2 MiB pair is
   293 Ir (big 64 KiB: 92), spread over `span_alloc` 54, `span_free` 52,
   `malloc_generic_once` 49, `span_from_segments` 33, `free_local_at` 25; the
   slice-marking loops scale with the span. Upstream keeps a retired page for
   reuse; here "span reclamation IS the reuse mechanism". Lever: a one-entry
   per-heap cache of the last retired large span, reused when the size class
   matches. Ceiling: most of 293 -> ~100 on alloc/free-same-size loops (Vec
   growth, tensor scratch, request buffers).
3. **Huge path, excluding the scrub:** ~4.7 k Ir per pair in `huge_alloc`
   (2.1 k), `huge_free` (1.8 k) and a `range.rs` iterator inside `huge_free`
   (0.8 k): per-slice `segment_map` register/unregister loops that scale with the
   block (64 MiB = 1,024 slices). Lever: register per chunk, not per slice.
4. **Thread start/exit beyond the scrub:** `malloc_generic_walk` ~4.2 k Ir per
   thread (first allocations walk empty bins), `collect_inner` ~2 k at exit,
   `span_free` ~1.5 k, `__nptl_deallocate_tsd` 0.7 k. Thread pools (tokio's
   blocking pool, rayon) pay this per spawn.
5. **`realloc_live` self cost: 65 Ir/op** before the copy, more than a whole
   small alloc/free pair; plus 17 from inlined page.rs and 10 from segment.rs.
   Per-line census first (opps #2 shaped this path once already).
6. **calloc wrapper:** 93.19 vs malloc's 52.25 per pair at 256 B; the memset is
   26, leaving ~15 Ir of wrapper and `free_is_zero` bookkeeping. Opps #5
   (`zero_block` re-resolving `usable_size`) is still OPEN and lives here.
7. **Adoption rescan (guard for the 2026-10-07 fix).** `adopt_segment` now
   restarts its slice walk after each dead large span it retires: O(spans^2)
   per adoption. Cheap at 8 spans; measure a segment of 64+ dead spans before
   shipping, and resume from the merged span instead of slice 0 if it shows.
8. **Duplicate arena scans.** `huge_free` with purging on now calls
   `arena::owns` and then `chunk_free_n`, two linear scans of the arena table;
   `segment_free` likewise on its failure arm. Fold into one lookup returning the
   arena id. Small, but it is per huge/segment free.
9. **Real programs, re-profiled.** perl/sqlite/python/lua/jq whole-program Ir
   (baselines in `memory/wsl-icount-rig.md` were set before 2.2.x); re-run the
   exact by-object census (`objir.awk`) and rank functions, not ops. This is
   where a vein that no synthetic op shapes shows up first.
10. **Bytes, not instructions.** `tools/wasm-size.sh` and the firmware rig
    measure code size deterministically; every fix above adds code, and the
    small profile's flash budget is a currency too. Re-baseline, and look for
    monomorphised duplicates in the per-arm `GlobalAlloc` paths.

### Vein census results, 2026-10-07 (working tree, uncommitted)

**Read this first: a memset-heavy row overstates cycles under callgrind.**
glibc's memset uses `rep stosb` above 2 KB and callgrind counts it once per
BYTE. Every number below marked *honest* was taken with
`GLIBC_TUNABLES=glibc.cpu.x86_rep_stosb_threshold=0x7fffffff:glibc.cpu.x86_rep_movsb_threshold=0x7fffffff`,
which makes memset/memcpy vector loops that callgrind counts like any other
code. Quote the honest column. Also: GCC deletes an unused malloc/free pair at
`-O2`; a probe must escape the pointer (a `volatile` sink) or every allocator
reads the same flat count.

| change | probe | before | after | delta |
|---|---|---:|---:|---:|
| **vein 1** scrub only the owner table + slots read before a carve; zero a slot at its first carve | 64 MiB malloc+free (honest) | 8,959 | 884 | **-90.1 %** (with vein 3) |
| | thread life (honest) | 19,387 | 17,409 | **-10.2 %** |
| | Rust `threads` workload, 20,000 steps (honest, checksum equal) | 3,191,866 | 2,713,575 | **-15.0 %** |
| **vein 3** huge: no strided `slice_offset` writes to slots 2..511, owner table scrubbed once, `wait_no_remote_in_flight` scans slot 1 only | (inside the 64 MiB row) | 2,931 | 884 | |
| **vein 2** per-heap one-entry large-span cache (`Heap::large_cache`), newest-first, <= 4 MiB, purging off only, flushed by every collect | 2 MiB malloc+free | 293.05 | 126.07 | **-57.0 %** |
| | exited-thread reload, 8 x 2.5 MB (honest) | 30,738 | 30,946 | +0.7 % (cache bookkeeping) |
| **retention fix** adoption retires every dead large span | 64 dead spans adopted (honest) | 136,899 | 139,729 | +2.1 % (the 63 spans 2.2.3 stranded) |

Unchanged to the instruction on every opscan op (small 52.25, med, big 92.00,
calloc 93.19, realloc 231.20, aligned 83.69, mixed 92.70, xthread 77.08).
Real programs, allocator-only Ir: perl +203 of 24.6 M, sqlite +90 of 4.6 M
(mimalloc's own perl count moved +172 between the same two runs: session
pedestal, not the change). Rust `maps`/`trees`/`overaligned` flat. Code size:
+1,936 bytes `.text` on the Rust workloads (+0.6 %).

**Refuted, recorded:** keeping the OLDEST freed span in the large cache cost
+882 Ir per op on the reload probe (later frees coalesced to its right and
retiring it last re-marked the whole run); newest-first fixed it. Resuming the
adoption walk at the merged span instead of slice 0 measured flat at 64 spans
(kept to bound the worst case, not claimed). A first cache-test design proved
nothing: first-fit returns the same address without the cache, so the test now
reads `pages_retired`; the large double-free test needed `--nocapture` in its
child, or libtest swallowed the line that proves which free aborted.

**Still open from the census:** `realloc_live` frame (13 Ir/call of entry/exit),
calloc wrapper (opps #5), the multi-chunk arena scan (bit-at-a-time, ~166 Ir
per huge allocation), `segment_map` per-window RMWs, re-profiling real programs
by function, code size.

### Vein census round 2 + validation sweep, 2026-10-07

| change | probe | before | after |
|---|---|---:|---:|
| multi-chunk arena claim: bitmap word loaded once per word, a one-word run claimed with one masked `fetch_or` (and its dirty bits with another) | 64 MiB malloc+free | 884 | **824 (-6.8 %)**, 6 -> 2 locked RMWs per 3-chunk claim |
| `owns`/`chunk_free`/`chunk_free_n` share one scan (`find_arena`) | same | 824 | 824 (neutral; -3 B wasm gz) |
| large cache: cache only a LONE large free (a bool `large_freed`), retire the cached span first on every large free | same-thread model swap, purge off | 1.04 models (the first cache) | **1.01** (= no cache) |
| | 2 MiB pair | 293.05 (2.2.3) | 138.06 (-52.9 %) |
| | exited-thread reload, honest | 31,472 (2.2.3) | 30,205 (-4.0 %) |
| | thread life, honest | 19,387 (2.2.3) | 16,988 (-12.4 %) |

Final build vs 2.2.3 on opscan: small/aligned/calloc/realloc/xthread
identical, big 92 -> 91, mixed 92.70 -> 92.04. Real programs, allocator-only
Ir: perl 24,633,532 -> 24,630,214, sqlite 4,582,668 -> 4,582,578. Endless Sky
start-up: output identical, allocator Ir 57,121,237 -> 57,119,496.

**Refuted, recorded (do not retry the wrong way):**
- `segment_map::walk_windows` coalescing windows per bitmap word: +15 Ir on
  the 64 MiB pair (tracking the pending word costs more than the two RMWs).
- `chunk_free_n` clearing a one-word run with one masked `fetch_and`: +9 Ir
  for a 3-chunk free.
- Flushing the large cache on a size miss in `large_alloc`: did NOT fix the
  model-swap spill, and inlined into `malloc_generic_once` it cost every
  generic op +3 Ir (big 92 -> 95, mixed +2.2). Bisected with a per-function
  diff; two earlier guesses (the shared `free_local_at` path, a new `u32`
  field changing `Heap`'s layout) were wrong.
- calloc (opps #5) and `realloc_live`: at their floor; #5's re-resolution is
  already gone, and realloc's frame is the price of inlining malloc + memcpy +
  free (its outlining was refuted +12 Ir in 2026-08).

**Validation sweep (final code):** workspace `--all-features` 177/177 (Windows);
`corpus/linux-gates.sh` 42 suites / 0 failures; embedded matrix 15/15
(small profile, 256k segments, single-threaded, aligned region, both RISC-V
bare-metal targets x both geometries, the no_std refusal); clippy per feature;
wasm self-test + size (20,873 B gz, +361 B vs 2.2.3, inside the +3 % ratchet);
gate self-test 11/11; semgrep clean; miri gate clean (the thread-exit test is
`ignore`d under miri: the mock prim fires no TLS destructors); mimalloc-bench
sweep 19/19; churn 5/5; `corpus/realworld.sh`: jq, sqlite, python, git, xz,
zstd, lua, perl byte-identical output across ra/mi/sys (imagemagick is
nondeterministic on every arm; redis is jemalloc-linked and fails under ANY
preloaded allocator, 2.2.3 included); alloc-eval `check` on all 8 traces
(~37.5 M ops) and the 28-thread battle replay with purging on and off.

**Edge cases the sweep caught and fixed:** the large cache's same-thread
model-swap spill (above); `adopt_large_spans`/`large_cache` hard-coded sizes
that are HUGE blocks under `ra_small_profile`, so they failed (or passed
vacuously) there, now geometry-derived and poison-checked at both geometries;
a `let _ = retire_span(..)` missing its same-line terminal comment (semgrep
`discarded-lifecycle-result`, CI-blocking); `corpus/linux-gates.sh`
hard-coded `/mnt/c/...` and exited 1 before running anything.

## Vein census 3, 2026-10-08 (real programs, by function)

**Method.** Callgrind on real programs, not synthetic ops: perl, sqlite, lua,
python and the Endless Sky start-up through the override (`LD_PRELOAD` on
valgrind itself: through `env`, valgrind traces `env` and not the exec), plus
the four Rust `GlobalAlloc` workloads. Per function: self Ir, call count,
self/call and caller->callee edges (`F:/ra-corpus-tmp/veins/cgcensus.py`,
`census3.sh`). Build: `main` at 2.2.5. mimalloc 2.4.5 through the same
parser: rusty_alloc is cheaper PER CALL at every comparable site (free 21.0 vs
25.0, malloc 14.9 vs 15.9, the page carve 1.98 M vs 3.14 M total on perl, the
lua realloc path 11.2 M vs 27.7 M, the Endless Sky cross-thread free
5.5 M vs ~15.3 M). So these veins are absolute costs, not gaps to the
oracle.

Allocator share of the whole program: Endless Sky 57.4 M Ir (5.62 %), perl
24.7 M (3.19 %), lua 23.1 M (3.85 %), sqlite 4.6 M (1.45 %), python 0.17 M.
The two fast paths are most of it (ES: delete 19.8 and new 14.8 Ir/call, 41 %
and 31 % of the allocator) and four campaigns have mined them; they are not
on this list except where a census line names a specific cost.

| # | vein | evidence (census) | ceiling | first probe |
|---|---|---|---|---|
| 1 | **Carving builds a whole free list up front.** `page_extend` links every block of the extension before one is used | `grow_front` perl 1.98 M (8.0 % of allocator; 1.84 M in page.rs), ES 1.91 M, lua 1.20 M; the thread-start walk spends most of its 911 Ir/call in the same loop | most of the carve: ~1.8 M on perl, ~1.9 M on ES | a bump-pointer carve (allocate from the uncarved tail, link nothing until freed). A throwaway that only times the loop sizes it first |
| 2 | **Endless Sky: 114 k LOCAL frees take the slow path.** `free_general` 175,393 calls, only 61,643 of them `remote_free` | ES `free_general` 4.37 M self (24.9/call, 7.6 % of allocator) | ~2.8 M if the common reason stays on the fast path | a per-reason counter: which `SLOW_FREE` bit (`IN_FULL` likely, or `SINGLE_BLOCK`) |
| 3 | **Rust: `GlobalAlloc::alloc` is not inlined into `__rust_alloc`; `dealloc` is.** | maps: `__rust_alloc` 4.0 Ir/call self, then a call to `RustyAlloc::alloc` (18.5 self) on 16,026 calls; hashbrown calls it out of line too | ~4-5 Ir per Rust allocation, in every Rust consumer | `#[inline(always)]` on `GlobalAlloc::alloc` (or split body/symbol); whole-program Ir + `.text` on all four workloads |
| 4 | **The generic slow path's 50-63 Ir of self cost per trip.** | ES 70,187 trips x 63.1 = 4.43 M (7.7 %): heap.rs 2.25 M, page.rs 0.80 M, **bins.rs 0.75 M (10.7/trip: the bin recomputed from the size)**; perl 574 k, lua 405 k | the bin recompute, ~0.75 M on ES; more after a line census | pass the bin the fast path already computed; per-line census of heap.rs |
| 5 | **C++ `operator new`'s miss takes an extra hop.** new -> `malloc_or_slow` (12 Ir) -> `malloc_slow` (7) -> generic, where `malloc` goes straight to `malloc_slow` | ES `malloc_or_slow` 70,115 x 12.0 = 841 k (1.5 %) | ~0.8 M on ES; every C++ program under the override | route the miss straight to `malloc_slow`, with the null/throw test after |
| 6 | **`realloc_live` self cost, re-measured.** "At its floor" was decided in 2026-08 against another shape | lua 60,051 calls x 55 = 3.30 M (14.3 % of lua's allocator): alloc.rs 29/call, page.rs 9, **segment.rs 5 (a segment re-resolve?)**, init.rs 2 (TLS heap) | the re-resolve plus whatever a line census finds | per-line census; check whether the segment and heap are already in hand from `realloc` |
| 7 | **Thread start/exit fixed cost and one OS allocation per thread.** | rust-threads, 200 threads: walk 911 x 2, `collect_inner` 436 x 2, `create_heap_uninstalled` 163, `segment_free` 105, `thread_done_one` 94, `reseed` 67, and `os::alloc_aligned`/`prim::alloc` 203 calls (an OS call per thread) | ~3.9 k Ir and a syscall pair per thread; thread pools pay it per spawn | find what the per-thread OS allocation is, and reuse it from exited threads |
| 8 | **Over-aligned Rust realloc asks `usable_size` out of line, then re-allocates.** | overaligned: `__rust_realloc` -> `usable_size` 5,125 x 13.0 = 66.6 k (27 % of that workload's allocator Ir), then `alloc` 2,678 x 60 | most of the 66.6 k, plus the moves that fit in place | an aligned in-place realloc that resolves the page once |
| 9 | **Win back 2.2.5's +3 Ir/free on MSVC builds that unwind.** The goto is banned there, but an `asm!` with an OUTPUT operand is not | 2.2.5: plain-Rust decrement = +3 Ir per local free (small 52.26 -> 55.26 forced on Linux) | up to 3 Ir/free on affected builds (rusty_sloth) | `sub dword ptr [..], 1` + `setle` into an output, branch in Rust; forced on Linux, then the rusty_sloth corpus row |
| 10 | **A fat heap's thread-exit collect walks every page.** | ES: ONE `thread_done_one` -> `collect_inner` = 1.82 M Ir (page.rs 1.62 M); mimalloc pays the same (`_mi_page_free_collect` 1.64 M) | ~1.8 M per exit of a big heap | abandon whole segments on a dying heap without per-page collection, if adoption can collect lazily |

**Rules for working these** (from the skill and this campaign): paired,
same-session measurements only; quote at least two shapes per probe (a real
program AND opscan); the work-parity anchors are the program's output and the
call counts above; record every refutation with its number.

### Vein census 3 results, 2026-10-08

Final tree against 2.2.5, same session, allocator self Ir (`real.sh`; outputs
identical):

| program | 2.2.5 | after | delta |
|---|---:|---:|---:|
| Endless Sky start-up | 57,380,635 | 53,453,604 | **-6.85 %** |
| lua | 23,149,581 | 22,632,842 | **-2.23 %** |
| perl | 24,668,474 | 24,262,947 | **-1.64 %** |
| sqlite | 4,592,621 | 4,587,503 | -0.11 % |
| python | 168,937 | 155,977 | -7.7 % |

Rust `GlobalAlloc` workloads, whole program (checksums equal): overaligned
-4.24 %, threads -1.53 %, maps -0.36 %, trees -0.01 %. Opscan: realloc
231.89 -> 227.97, xthread 77.09 -> 74.82, aligned 83.75 -> 83.00, calloc
93.75 -> 93.00, mixed 94.26 -> 93.56, small/big unchanged; **2 MiB op 142.06 ->
144.06 and 64 MiB pair 834 -> 836 (+2: vein 2's trade, below)**. Peak RSS
within +-0.4 % (perl, lua, ES); 28-thread battle replay 1,752.6 -> 1,744.1 MB.

| vein | result | what landed |
|---|---|---|
| 1 carve | **LANDED**, the biggest | geometric extension batches (`page_extend`: at least what the page holds). ES -4.50 %, lua -1.94 %, perl -1.62 %. The link loop was already 2.75 Ir/block (unrolled x4); the slow-path TRIPS were the cost. A fixed 8 KiB batch: ES -2.92 % but thread life +10 % (refuted) |
| 2 slow frees | **LANDED** | the census premise was wrong: the 175 k were CROSS-thread frees into FULL pages (`remote_free` inlined in `free_general`). C exports test the owner first (`free_inline`); Rust keeps the 2.2.5 order (`free_inline_flags_first`), because the bigger remote arm stopped `__rust_dealloc` inlining into drop glue (`trees` +2.88 %). ES -515,705, xthread -2.9 %. Trade: a LOCAL free into a slow page (large span, huge, aligned, full) pays the owner compare first, +2 Ir (the 2 MiB and 64 MiB ops). Refuted: unalign inlined into the remote arm (+3 Ir on EVERY free); one cold call for all remote frees (-43,649 only) |
| 3 Rust alloc inlining | **LANDED** | only the natural-alignment arm out of line (it was a second inlined copy of malloc). maps -0.33 %, overaligned -1.81 %. Refuted: `#[inline(always)]` (maps +0.98 %), all non-word arms out of line (overaligned +0.93 %), one merged malloc call (maps +1.33 %) |
| 4 bin recompute | **LANDED** | `SMALL_BIN` table by word count, indexed with the word count the generic path already computes. ES -0.70 %, then -73 k more with the shared word count |
| 5 operator new hop | **LANDED** | `malloc_or_with::<H: OnOom>`: the cold arm's frame 12 -> 7 instructions (a typed handler, not a `fn` pointer). ES -419,182 |
| 6 realloc_live | **LANDED** (small) | the page resolved for the usable size is handed to the free. lua -60,025, opscan realloc -2.00 |
| 7 thread start/exit | **LANDED** | an 8-slot heap-box cache (upstream `TD_CACHE_SIZE`). Thread life -1.1 %, Rust threads -1.46 %, minor faults ~137 -> ~117 per run |
| 8 over-aligned realloc | **LANDED** | `#[inline]` on `usable_size`. overaligned -2.06 % (isolated) |
| 9 MSVC unwind | **LANDED** | `sub` + `setle` into an output on MSVC with `panic = "unwind"`: +2 over the goto where 2.2.5's plain decrement was +3 (forced on Linux). rusty_sloth builds (registry-identity corpus row), MSVC unwind IR still has 0 `callbr` |
| 10 thread-exit collect | **REFUTED, inherent** | the cost is the cross-thread drain (each remote-freed block walked once, at its collect; teardown is where the backlog is). mimalloc pays the same 1.64 M. The per-step `n > used` check also bounds a corrupted (cyclic) chain, so it stays |

Gates: workspace `--all-features` 177/177, linux-gates 42/0, embedded 15/15,
clippy (Windows + Linux targets, all features), fmt, semgrep, wasm self-test
and size (21,175 B gz, inside the +3 % ratchet), gate self-test 11/11, miri
gate + `openheimer` 37/37 under miri, mimalloc-bench sweep 19/19, churn 5/5,
real-world programs byte-identical across ra/mi/sys, alloc-eval checks on all
8 traces, rusty_sloth (crates.io identity, `--release --features cuda`).
One unexplained failure in the first of 27 Windows test runs (log lost), not
reproduced in the 26 after it.

## Banked

### 8. Collect-loop double `block_next` — LANDED 2026-08-20

The `page_collect` steal loop called `block_next` in both the loop condition and
the body, decoding (and, under `secure`, bound-checking) every element twice.
Rewritten to once per element. Helps the default build's batch path too, not
only `secure` — a down-payment on the same loss Tier 1 targets. Committed with
the blockmap work.

---

## Log

Append a dated line per candidate as it is measured. Keep the refutations — a
flat or negative result is a finding.

| Date | # | Result | Note |
|---|---|---|---|
| 2026-08-20 | 8 | landed | collect double-decode removed (banked with blockmap) |
| 2026-08-20 | 2 | **LANDED** | realloc counter gated debug-only; opscan realloc 393.69 → 379.53 (**−14.16 Ir/op**, 0.788 → 0.759), removes a `my_heap()` TLS resolution per release realloc. No memory cost. Clean A/B vs freshly-built baseline. |
| 2026-08-20 | 1 | **LANDED, with a refutation banked** | The stated target (opscan batch) was **byte-identical** — batch_lifo stayed 60.17. The two-point estimator `(Ir(2N)−Ir(N))/N` cancels the page-fill/refill where `page_extend`'s division lives, so opscan is structurally blind to this. The win is real but on WHOLE-PROGRAM page-carving: perl ra 777,445,780 → 776,731,380 (**−713k, −0.092%**), lua −413k (−0.068%), sqlite −9k, all deterministic (callgrind, exact per binary). Also a determinism win in its own right — removes 3 data-dependent-latency hardware DIVISIONS from the refill path. Cost: +8 B/page (Page 80 → 88), ≈0.01% memory; header still fits in slice 0 in every config (tightest, secure+blockmap, keeps ~4 KB headroom). |
| 2026-08-20 | 6 | **REFUTED — codegen floor, nothing shipped** | Traced the batch gap to free's `used--` (5 insns vs mimalloc's `subw; je` = 2) + the thread-compare (`mov fs:0` + cmp vs mimalloc's `cmp reg, fs:0`). malloc is at parity; free is ours 27 / mi 24 executed insns. The push/decrement split meant to trigger `dec [mem]; jle` produced BYTE-IDENTICAL asm. Both folds need inline asm on the hottest path — declined. The gap is a Rust-vs-C instruction-selection floor, not algorithmic. A NOTE at the decrement records this so it is not retried. |
| 2026-08-20 | 3 | **AUDIT — hot paths already clean** | 39 `panic_bounds_check` sites lib-wide, but **zero** reachable from the shipped `malloc` / `free` / `calloc` fast paths (confirmed on the override `.so`). All 39 are on slow/cold paths (segment alloc/free 11, visitors/adopt 6, arena 7, teardown, and 2 on the `malloc_generic` refill). The safe-Rust hot path pays NO bounds-check tax — the h264-skill lesson holds. Refill-path removal tracked below. |
| 2026-08-20 | 5 | **LANDED — calloc 152.62 → 132.00 (−20.62 Ir/op)** | `Heap::zalloc` pops the block and zeroes it with the page IN HAND, using `(*p).block_size` for the usable extent instead of `zero_block`'s `usable_size(p)` re-resolution (segment mask + page resolve + kind check + unalign) on every recycled block. calloc 0.949x → **0.820x** vs mimalloc. Every other op byte-identical (small 79.38, batch 60.17, realloc 379.53 unchanged) — plain malloc/free untouched. Full-extent zeroing contract preserved (the `zalloc_is_zero_across_the_whole_usable_extent` property still passes). Workspace release + all-features clippy clean. |
| 2026-08-20 | 4 | **ASSESSED — not worth it** | Premise false: `bin_size(bin)` is `bin << 3` for the common `bin <= 8`, and `bin_size(bin-1)` only runs for `bin > 8`. Replacing the shift with a table/stored load reintroduces a `panic_bounds_check` (bin from `bins::bin` is unbounded to LLVM) that costs more than it saves. The `.min` clamp that would bound it was tried under #3 and changed nothing. Left as-is. |
| 2026-08-20 | 7 | **LANDED — segment-release scan bounded to actual use** | Fixed O(512) sweep on every segment release → `[HEADER_SLICES, next_free_slice)`, i.e. only the carved region; never-carved slots are guaranteed-idle (`xthread_free == 0`). A segment that carved 10 slices scans 10, not 512. Deterministic-latency win, bounded by use. Loom + stress_mt + teardown_reclaim + abandon_rss green; the aligned-op profile incidentally reconfirmed we already BEAT mimalloc there (ra 172/op vs mi 190/op — mimalloc burns 63/op in `_mi_page_retire`), so no parity gap to chase in the aligned path. |
| 2026-08-20 | 9 | **LANDED — locked swap → load on the empty slow path** | `process_delayed` (runs on every `malloc_generic`) drained the cross-thread list with an unconditional locked `swap`, even when empty. Peek-with-load first. big/large −1.00 Ir/op, mixed −0.72, fast path unchanged; and it removes a ~20-cycle locked barrier from every slow-path alloc, a wall-clock win the Ir count understates. Loom protocol green. A fresh find during the Tier-3 hunt, not one of the original 8. |

### Refutation banked (do not retry the wrong way)

**Candidate 1 will NEVER show on opscan batch_lifo/batch_fifo.** Do not "re-measure
it properly on batch" expecting a number — the loss those ops carry is not in the
refill path. In a steady batch loop on one page, allocs pop `free`, frees push
`local_free`, `page_collect` swaps them; `page_extend` (and its division) only
fire while the page is still being lazily filled, which is warmup, which the
two-point estimator cancels by design. Measure candidate 1 on whole-program
carving workloads, never on the synthetic batch op. The +0.47 Ir/op batch gap is
still open and belongs to a different mechanism (see candidate 6, and the
`page_collect` steal/append path generally).
