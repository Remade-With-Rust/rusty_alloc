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
| 3 | Emitted-bounds-check audit of the hot paths | 2 | unknown | proposed |
| 4 | `update_direct` recomputes `bin_size` twice | 2 | batch loss | proposed |
| 5 | `zero_block` re-resolves `usable_size` (calloc) | 2 | calloc | proposed |
| 6 | Double-free count walk in `page_collect` | 3 | batch_fifo | proposed |
| 7 | `wait_no_remote_in_flight` 512-slot scan | 3 | latency | proposed |
| 8 | Collect-loop double `block_next` | — | batch loss | **banked (landed)** |

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

### 3. Emitted-bounds-check audit of the hot paths

`cargo rustc --release -p rusty_alloc --lib -- --emit asm`, then attribute
`panic_bounds_check` call sites to symbols by line range. Not yet run. The
cheapest way to find a *surprise* hot-path check — the `rusty-unsafe-optimizations`
case study found the "obvious" candidates emitted zero checks while one
unglamorous site owned 13% of a codec's encode. One command; may reorder
everything below it. Deterministic, needs no quiet box.

### 4. `update_direct` recomputes `bin_size` twice per refill

**Where:** [heap.rs:1180](../crates/rusty_alloc/src/heap.rs#L1180),
[heap.rs:1197](../crates/rusty_alloc/src/heap.rs#L1197)

Computes `bin_size(bin)` and `bin_size(bin-1)` (shift/mask arithmetic) on every
refill. The `w_lo`/`w_hi` wsize bounds are a pure function of `bin` and could be
a small const table indexed by bin, turning arithmetic into a load. Heartbeat
cadence, so modest.

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

### 6. Double-free count walk in `page_collect`

**Where:** [page.rs:828-853](../crates/rusty_alloc/src/page.rs#L828-L853)

Walks the entire stolen xthread chain every collect to count its length for the
`used` decrement. On batch_fifo (the adversarial free order, one of the two
losing ops) that chain is long. Maintaining the count at push time would avoid
the walk, but the remote push is a lock-free CAS with a 2-bit flag packed in the
word — intrusive. Lower priority; flagged for completeness.

### 7. `wait_no_remote_in_flight` 512-slot scan

**Where:** [segment.rs:183](../crates/rusty_alloc/src/segment.rs#L183)

O(512) scan on every segment release. Fires only on release (rare), so it is a
latency spike, not a throughput cost — but a candidate for a bitmap if segment
churn ever surfaces in a soak.

---

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
