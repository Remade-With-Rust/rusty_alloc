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
| 1 | `page_index` division on the refill path | 1 | batch loss | **LANDED — whole-program win, not the batch one** |
| 2 | `stat_realloc` re-resolves the heap per realloc | 1 | realloc | **LANDED — realloc −14.16 Ir/op** |
| 3 | Emitted-bounds-check audit of the hot paths | 2 | unknown | **DONE — fast paths already bounds-check-free** |
| 4 | `update_direct` recomputes `bin_size` twice | 2 | batch loss | **ASSESSED — premise false, the cheap version is already there** |
| 5 | `zero_block` re-resolves `usable_size` (calloc) | 2 | calloc | **LANDED — calloc 152.62 → 132.00, −20.62 Ir/op** |
| 6 | Batch-op gap → traced to free's `used--` codegen | 3 | batch loss | **CLOSED as a codegen floor (refuted)** |
| 7 | `wait_no_remote_in_flight` 512-slot scan | 3 | latency | **LANDED — bounded to the carved region** |
| 8 | Collect-loop double `block_next` | — | batch loss | **banked (landed)** |
| 9 | `process_delayed` swaps an empty list every slow-path alloc | 3 | slow path | **LANDED — load-before-swap** |

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

### 6. The batch-op gap — TRACED, and it is a safe-Rust codegen floor

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
