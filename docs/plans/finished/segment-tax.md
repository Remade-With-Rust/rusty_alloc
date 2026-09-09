# segment-tax — the 64 KiB that costs 32 MiB, and the fixes ranked

**Source:** FFAI/Carmenta field report on 1.1.5
(claude.ai/code/artifact/e9d564af-598d-4983-8f69-1c6de65081e3), 2026-08-28.
Status: **F1 + F5 + F2 LANDED, 2026-08-28** (results at the bottom).
F3 rejected (and now moot); F4 superseded; F6 open.

## What the report establishes (verified against the source)

1.1.5's leak fix holds — their isolated repro reads +0.0 MiB and their OCR
pipeline plateaus at 1280 MiB. What remains is a measured sizing rule:

> `cost(size) = SEGMENT_SIZE × ceil((size + 64 KiB) / SEGMENT_SIZE)`

with 100 % waste spikes at exactly the power-of-two sizes tensor workloads
produce, and a 2.2–2.5× steady-state peak vs dlmalloc. Both claimed source
faces verify:

- **Huge face** — `segment.rs::huge_alloc`: `want = page_align_up(header +
  size + extra)`; the 64 KiB header is inside the round, so any size that is
  a whole number of segments tips into an extra chunk. On wasm our own
  chunk-granular rounding (needed for adoption) makes that a full +32 MiB.
- **Large face** — `LARGE_OBJ_SIZE_MAX = SEGMENT_SIZE / 2` exactly, but a
  segment offers only `USABLE_SLICES = 511` of 512 slices; two 16 MiB pages
  need 512, so they miss sharing by one slice and each takes a segment alone.

Why it bites on wasm specifically: waste is not transient RSS there — linear
memory only grows, and `memory_size` is the number that hits the 4 GB ceiling.

## The reframing discovery

`heap.rs::large_alloc` **already allocates exact-slice spans** —
`size.div_ceil(SEGMENT_SLICE_SIZE)` slices, single-block, `bin = BIN_HUGE`
marker, `SINGLE_BLOCK` flag, no bin-table involvement, packed and recycled
inside normal segments by `span_from_segments`. The report's own control row
proves it packs (16 MiB − 128 KiB: two per segment, 1 % waste).

And `LARGE_OBJ_SIZE_MAX` is used in exactly three code places: the malloc
route (heap.rs:417) and two aligned-path checks (heap.rs:1021, 1027). It is
**not** wired into bin geometry. The threshold is a routing choice, not a
structural constant.

## Fixes, ranked

### F1 — route everything that fits a segment through the span path (small, big yield)

Raise the huge-routing threshold from `SEGMENT_SIZE / 2` to what a segment
actually holds: `USABLE_SLICES × SEGMENT_SLICE_SIZE` (= 32 MiB − 64 KiB).
Everything in (16 MiB, 31.94 MiB] becomes an exact-slice span in shared
segments instead of a dedicated 32 MiB reservation:

| report row | today | with F1 |
|---|---|---|
| 20 MiB | 32 MiB (60 %) | 320-slice span + 191-slice usable tail |
| 25.1 MiB (detector tensor) | 32 MiB (27 %) | 402-slice span + 109-slice tail |
| 32 MiB and up | unchanged | unchanged (see F2) |

**Honesty amendment (found while building the F5 gate):** F1 does NOT change
the report's pure-hold marginal for any single size above ~15.97 MiB — a span
over 255 slices cannot pair with ITSELF (2 × 256 > 511), so `hold(n)` of one
such size still reads a whole segment per block. The 16 MiB-exact row in
particular is geometrically unfixable inside a 32 MiB segment, by F1 or by
anything else (its tail was already usable pre-F1 — large blocks were spans
all along; only (16 MiB, 31.94 MiB] rerouted). F1's real win is MIXED-size
packing, which is what real workloads do — the F5 gate's discriminating row
measures exactly that.

The span machinery already exists and already recycles; the change is the
constant plus the two aligned-path checks, then the full native gate (the
`huge` opscan op and `mixed` will shift — sizes ≤ 31.9 MiB reroute). Not
wasm-only: the geometry argument holds natively too (a span that fits should
be a span), but if the native gate shows a regression, the fallback is a
target-conditional threshold.

Verification the report hands us: their marginal-cost probe (`hold(n,9) −
hold(n,1)`, fresh instance per row) — adopt it as a selftest arm.

### F2 — slice-granular arena service on wasm (medium, closes the rest)

The arena module doc already names slice-granular arenas as the intended
post-v1 refinement. On wasm it is the endgame for the ≥ 32 MiB rows: adopted
memory served at 64 KiB granularity means a freed 64 MiB reservation later
serves a 48 MiB frame plus 20 MiB of spans from the same bytes, and the
chunk-rounding overshoot stops being per-block waste and becomes pool
headroom reused by the next allocation. With F1 + F2 the whole §3 table
collapses to ≈ 0 steady-state waste; first-touch waste is bounded by one
chunk per PEAK live huge block, not per allocation.

Scope: arena bitmap at slice granularity (or a second slice-level map inside
chunks), `huge_alloc`/`huge_free` asking for exact slice counts on wasm,
adoption accepting page-granular blocks. Contained in arena.rs + the two
segment.rs call sites; native paths untouched (chunk-granular service remains
the native default).

### F3 — out-of-band huge header (deep; probably unnecessary)

The report's suggestion #1. It would make `n × SEGMENT_SIZE` cost exactly
`n` chunks even at first touch — but every free resolves its segment by
masking the pointer, so a payload at chunk base with a header elsewhere
breaks pointer→segment resolution on the hottest path in the crate (`free`,
21.000 Ir/call, guarded by this campaign's entire measurement record).
With F2 in place the +1 chunk is transient pool headroom, not a leak, so the
residual value of F3 does not justify its risk. REJECTED unless F2 proves
insufficient.

### F4 — the report's #2 (shrink LARGE_OBJ_SIZE_MAX so two large pages pair)

Superseded by F1: shrinking the constant just reroutes 16 MiB to the huge
path at identical cost (32 MiB either way). The honest fix for the
one-slice miss is F1's tail-sharing, not a smaller promise. Keep the DOC fix
(the constant's comment promises a fit the geometry cannot deliver).

### F5 — waste-bound gate (their #3, do regardless)

A selftest arm that walks sizes across `SEGMENT_SIZE/2` and `SEGMENT_SIZE`
boundaries asserting `marginal ≤ request + SEGMENT_SLICE_SIZE + bound`:
native via a fresh-heap stats probe, wasm via their `hold()` diff. This is
the test that would have caught the tax at birth — same lesson as the
steady-state arm (their #4, already shipped in 1.1.5).

### F6 — occupancy census to close their open attribution

The report is careful: the rule explains perhaps a third of the 2.2×, and
they have not attributed the rest. Export a `ra_debug_census` from the wasm
build (walk segments: slices reserved vs used, per kind; walk arenas: chunks
used/dirty) so the next report can name the remainder — candidates are span
fragmentation inside segments, `chunk_alloc_n` contiguity failures, and
candle's own buffer sizing.

## Order

F1 (+F5 gate, +F4 doc) first — small, contained, kills the three worst
sub-32 MiB rows including both 100 % cliffs the report leads with. F2 next —
the wasm endgame. F6 alongside F2 to close the attribution. F3 rejected.

## Results — F1 + F5 landed, 2026-08-28

F1 is three lines of substance: the constant
(`LARGE_OBJ_SIZE_MAX = (SLICES_PER_SEGMENT - 1) * SEGMENT_SLICE_SIZE`), a
const assert in segment.rs locking it to `USABLE_SLICES` forever, and the
divergence note. The aligned paths needed no change — mid-size aligned
requests flow through oversize-and-adjust onto 64 KiB-aligned span payloads.

F5 is two probes and a gate: `ra_hold` / `ra_hold_mix` exports in
`rusty_alloc-wasm` (the report's §7 instrument, fresh module instance per
data point), driven by a waste table in `bench/wasm-selftest.mjs` whose
bounds are the segment geometry; plus five native structural tests
(`tests/span_packing.rs`) asserting co-tenancy by segment base address.

| waste-gate row | pre-F1 | with F1 | bound |
|---|---:|---:|---:|
| 8 MiB single (packs 3/segment) | 8.00 | 8.00 | 12 |
| 16 MiB − 128 KiB single (packs 2) | 16.00 | 16.00 | 20 |
| 20 MiB single (regression pin) | 32.00 | 32.00 | 34 |
| **20 MiB + 11.875 MiB pair** | **48.00 — FAIL** | **32.00 — PASS** | 34 |
| 25.1 MiB + 6 MiB pair | 32.00 | 32.00 | 34 |
| 33 MiB single (huge; F2 will lower) | 64.00 | 64.00 | 66 |

The discriminating row behaves exactly as the arithmetic predicts: a
dedicated 32 MiB huge reservation plus a 16 MiB amortized span pre-F1, one
shared segment after. The steady-state selftest's own footprint dropped
**192.06 → 128.06 MiB** as a side effect (its 20 MiB cycles now span-share).

Native, byte-identical where it must be: cfrac allocator **3,445,578,349 —
the exact pre-F1 count** (no opscan op exercises (16 MiB, 32 MiB], so nothing
was allowed to move, and nothing did), `free` 21.000 Ir/call, all 13 ops
unchanged, 33 suites / 94 tests (5 new) / 0 panics, Kani 5/5, census
re-baselined 863 (+4, the probes' and tests' SAFETY-commented blocks),
datasweep 573,640 × 6 arms, corpus sweep passed, clippy 0 on both platforms.

What FFAI should see: their 25.1 MiB detector tensors and every other
(16, 31.94] MiB buffer now pack with their neighbours instead of costing a
32 MiB floor each. The ≥ 32 MiB rows (their 48 MiB frames) wait on F2.

## Results — F2 landed, 2026-08-28

F2 shipped as something better than the plan's sketch. The plan proposed
slice-granular ARENAS; what landed dissolves the constraint that made
granularity a problem in the first place. The chunk rounding existed because
every segment base had to be SEGMENT_SIZE-aligned for `segment_of`'s pointer
mask. On wasm that mask is now a slice-granular base table
(`segment_map::base_of`, 256 KiB of wasm-only BSS, one load per resolution —
and wasm's free path is plain Rust with no asm fast path, so the swap is
cfg'd cleanly while native keeps its measured-optimal mask untouched).

With the mask gone, three small pieces finish it:

* `slice_pool` — an 8 KiB free-slice bitmap over the 4 GiB wasm address
  space: first-fit `alloc_run`, bit-set `free_range`, coalescing by
  construction. Bookkeeping only; unit-tested natively.
* `reserve_backing` — segments and huge blocks reserve the SLICE round of
  what they need (pool first, fresh `memory.grow` second), not the chunk
  round. The wasm prim's one-time 32 MiB alignment pad disappears too, since
  nothing asks for more than 64 KiB alignment any more.
* Frees go to the pool: `segment_free`, `huge_free`, and `os::free` itself —
  which also closes wasm-recycling.md's residual heap-descriptor leak, since
  descriptors are page-granular and a wasm page IS a slice.

| waste-gate row | pre-F2 | with F2 | bound |
|---|---:|---:|---:|
| 33 MiB single | 64.00 — FAIL | **33.06** | 34 |
| 32 MiB exact — the report's headline | 64.00 — FAIL | **32.06** | 33 |
| 48 MiB frame (12 MP RGBA) | 64.00 — FAIL | **48.06** | 49 |
| every F1 row | unchanged | unchanged | — |

The 64 KiB that cost 32 MiB now costs 64 KiB. The selftest's own steady
state fell 128.06 → **66.50 MiB** (192.06 at 1.1.5) and its start-up floor
2.06 → 1.31 MiB (the alignment pad). Native: cfrac allocator 3,445,578,349 —
byte-identical, all 13 opscan ops unchanged, 33 suites / 98 tests / 0
panics, Kani 5/5, datasweep × 6 arms, corpus passed, clippy 0 on native,
wasm, and Windows, census re-baselined 864.

Still open: F6 (occupancy census for the report's unattributed remainder of
the 2.2×). With F1+F2, their steady-state plateau should re-measure well
below 1280 MiB — worth asking for a re-run before investing in F6.
