# Per-operation scan vs mimalloc — method, results, and the optimization plan

Status: **scan built and executed; optimizations NOT yet implemented.**
Instrument: `bench/opscan.c` + `bench/opscan.sh`. Date: 2026-08-06.

## Why not a function-by-function symbol diff

The obvious reading of "compare each function" does not survive contact with the
artifact: release mimalloc inlines essentially the whole allocator into two
symbols. Its entire profile on a real workload is

```
15,008,792  free
 9,695,848  malloc
 3,144,168  mi_page_free_list_extend.isra.0
```

There is no `mi_page_malloc`, no `mi_free_block_local`, no `mi_segment_page_of`
to line up against ours — they are all inside those three. A symbol-level table
would be comparing our 20 functions against their 3 and inventing the mapping.

So the comparison is done at the level of **operations**, which is both
measurable and directly actionable: one C driver (`bench/opscan.c`) exercising
one allocator behaviour per op, run under each allocator via `LD_PRELOAD`, so
every arm executes byte-identical caller code and the allocator is the only
variable.

## The instrument, and the two variants that were DISQUALIFIED

Three estimators were built. Two are wrong, and the way they were caught is the
most transferable part of this document.

**1. Whole-process two-point — ADMISSIBLE.**
`Ir/op = (Ir(2N) - Ir(N)) / N`, on total instructions retired.
Differencing cancels process startup, `ld.so`, allocator init and first-touch
warmup exactly. It is **attribution-free** — pure subtraction of two totals — so
there is no symbol-mapping step that can go wrong. Work parity is structural:
one binary, one op, same N.
Its one limitation is stated rather than hidden: each measured op also includes
the caller's loop and two PLT thunks. That overhead is *identical across arms*,
so **deltas are exact and ratios are diluted toward 1.0**. Read the delta column;
treat the ratio as a lower bound on the real difference.

**2. Per-object total — DISQUALIFIED (under-counted us ~4x).**
`alloc_Ir(N)/N`, summing every line `callgrind_annotate` attributes to the
allocator's `.so`. Broken because **`callgrind_annotate` elides the `[object]`
suffix on continuation lines**. mimalloc has three fat symbols that each carry
the suffix; our cost is spread across many `file:function` lines
(`mut_ptr.rs`, `page.rs`, `heap.rs`, `atomic.rs`, all attributed to
`alloc::free`) whose suffixes are omitted. Grepping the `.so` path therefore
kept mimalloc's cost in full and silently dropped most of ours.
**Caught because it disagreed in SIGN with estimator 1** on `aligned` and
`batch_lifo`, and claimed we beat mimalloc 2-4x on every single op — which is
irreconcilable with perl measuring 1.0099.

**3. Marginal per-object — DISQUALIFIED (over-counted us).**
Estimator 2 with the grep widened to match names instead of the object suffix,
then differenced. It reported our allocator at **115.79 Ir/op on an op where the
WHOLE PROCESS spends 82.37**. An allocator cannot cost more than the process
containing it, so the attribution double-counts somewhere.
**An impossible number outranks a plausible one**: this was rejected on the
arithmetic alone, without needing to find the exact double-count.

The lesson generalises: *the estimator that needs no symbol attribution is the
one to trust when attribution is the hard part.* Both broken variants failed in
the attribution step, and the surviving one has no attribution step.

## Validated results

`bench/opscan.sh`, N per op chosen so each callgrind run is a few seconds.
Deltas are exact; ratios are diluted by the constant caller overhead.

| op | ra | mi | glibc | ra−mi | note |
|---|---:|---:|---:|---:|---|
| aligned | 209.31 | 187.89 | 142.00 | **+21.42** | posix_memalign(64, 256) |
| batch_fifo | 73.97 | 59.68 | 301.12 | **+14.29** | 64 live, freed FIFO |
| batch_lifo | 73.98 | 59.70 | 300.72 | **+14.28** | 64 live, freed LIFO |
| usable | 32.00 | 30.00 | 21.00 | **+2.00** | malloc_usable_size |
| calloc | 155.44 | 160.89 | 130.00 | −5.45 | |
| mixed | 149.18 | 157.85 | 398.15 | −8.67 | varied sizes, 64 live |
| small | 82.37 | 111.41 | 75.00 | −29.04 | malloc(16)/free |
| med | 90.50 | 123.02 | 75.00 | −32.52 | malloc(256)/free |
| big | 171.00 | 222.04 | 294.00 | −51.04 | malloc(4096)/free |
| large | 171.00 | 222.02 | 312.00 | −51.02 | malloc(64K)/free |
| realloc | 401.92 | 499.82 | 583.00 | −97.90 | grow 64→128→512 |
| huge | 846.00 | 53362.78 | 294.00 | **−52516** | malloc(2 MiB)/free |

**`huge` is the headline win and it is structural**: mimalloc pays an
mmap/munmap round trip per 2 MiB cycle; our arena serves it from cache. This is
the M6/M7 arena-backed-huge result reproduced as a clean per-op number.

**Note the shape.** We win the *simple* ops (one block in flight) and lose the
ops with a **live working set** (`batch_*`, and `mixed` is only barely ahead).
That is exactly consistent with the whole-program results — perl 1.0099, sqlite
1.0056 — because real programs look like `batch`/`mixed`, not like a ping-pong.
**The microbenchmark where we look best is the least representative one.**

## The plan (ranked; each item states its verification BEFORE its fix)

**P1 — `batch_*`, +14.3 Ir/op at 1.24x. Highest value: most representative.**
Both orders cost the same, so this is not a free-list *ordering* effect — LIFO
and FIFO retire identical instruction counts. The candidates are (a) our page
free-list extend batch size versus upstream's `MI_MAX_EXTEND_SIZE/bsize`
(4096/bsize, so 256 blocks for 64-byte blocks), (b) the `pages_direct` small-size
fast-path lookup, (c) per-malloc bookkeeping that upstream defers.
*Verify first:* count generic-path entries per 1000 allocations on both sides
(we already have `stats.generic`; upstream needs a counter build). If our
generic rate is higher, it is (a) and the fix is the extend policy. **Do not
touch the extend policy before that count exists** — a wrong extend size trades
instructions for RSS.

**P2 — `aligned`, +21.4 Ir/op at 1.11x.** Larger per-op delta than P1 but a much
rarer operation, so it ranks second by expected total. Our `posix_memalign`
routes through the general aligned path; upstream fast-paths the common case
where the natural block alignment already satisfies the request (a 256-byte
block is already 64-byte aligned, so the whole adjustment is unnecessary).
*Verify first:* count how many aligned requests actually need adjustment.

**P3 — `usable`, +2.0 Ir/op at 1.07x.** 32 vs 30 instructions. Small and
bounded; worth doing only if it falls out of P1/P2 work. Listed for completeness
so it is not rediscovered as new.

**Explicitly NOT a target:** every op with a negative delta. Reverting a win to
chase a ratio is how campaigns lose ground.

## EXECUTION LOG — P1/P2/P3, and the answer to "why the live working set"

### P1 — REFUTED on the count, then twice more on the clock

The plan demanded a count before code, and the count killed the hypothesis:

| generic-path entries per 100,000 allocations (`batch_lifo`) | |
|---|---:|
| ours (`Heap::malloc_generic`) | **1,566** |
| mimalloc (`_mi_malloc_generic`) | **1,562** |

Identical. **We do not leave the fast path more often than upstream**, so the
extend-policy change P1 proposed would have been pure wasted work — and would
have traded RSS for nothing. Same result on the `aligned` op: 6,254 vs 6,250.

The count did surface something real, though: our exported `free` makes 100,082
calls into `alloc::free` per 100,000 frees — a whole extra frame — where
upstream's `free` is one flat symbol. `alloc::malloc` carried `#[inline]` and
`alloc::free` did not. Two bricks were built on that and **both measured worse**:

| | batch_lifo | perl | sqlite |
|---|---:|---:|---:|
| baseline | 73.98 | 1.0100 | 1.0056 |
| `#[inline]` + cold-split | 74.98 | 1.0105 | 1.0060 |
| cold-split ALONE | — | 1.0106 | 1.0060 |

Both reverted. Isolating the second run mattered: the first brick changed two
things at once, and separating them showed the cold-split — not the inlining —
was the harmful half, which refuted the register-pressure theory below rather
than leaving it plausible.

### The answer: it is a fast-path COST problem, not a slow-path FREQUENCY one

Per-function profile of `batch_lifo` (`bench/opprofile.sh`), Ir per operation:

| | ours | mimalloc | delta |
|---|---:|---:|---:|
| free path | **34.1** | 25.0 | **+9.1** |
| malloc path | **21.0** | 16.9 | **+4.1** |
| total allocator | 55.1 | 41.9 | +13.2 |

With the generic rate identical, every one of those 13.2 instructions is spent
on the path both allocators take 98.4% of the time. The disassembly of
`alloc::free` names the two largest contributors:

**1. `Page` is 80 bytes, and 80 is not a power of two (~11 Ir/op).** Resolving a
block to its page indexes the slice table, and the emitted code is

```
shr eax,0xc ; and eax,0x1ff0 ; lea rax,[rax+rax*4]     <- idx * 80
movzx eax,WORD PTR [rsi+rax*1+0x4a]                    <- slice_offset
neg rax ; lea rax,[rax+rax*4] ; shl rax,0x4            <- off * 80
```

a `lea`/`lea`/`shl` chain where a 64-byte `Page` would need one `shl`. This
matches `mut_ptr.rs` measuring 11.02 Ir/op — **32% of our entire free cost**.
Upstream pads `mi_page_t` deliberately, with the comment *"improve page index
calculation"*; we never did.

**2. Six instructions of callee-saved traffic.** `free` opens `push r15; push
r14; push rbx` and closes with the pops, on a fast path that uses none of r14/r15.
Moving the general path out of line to relieve it made things WORSE (above), so
the fix is not "split the function" — the argument setup costs more than the
saves. Left as an open item, not a plan.

### The real next brick, sized but NOT built

**Shrink `Page` from 80 bytes to 64.** Then slice indexing is a single shift on
both the forward index and the `slice_offset` follow-back, worth ~2-3 Ir/op on
every free and every malloc. 128 is not available: `512 x 128 = 65,536` is the
entire slice, leaving no room for the segment header.

Cutting 16 bytes is the whole difficulty. Fields today (`repr(Rust)` reorders to
80): six pointers (48) + `block_size: usize` + four `u32` + two `u16` + four
`u8`. Candidate savings: `block_size` to `u32` (−4, bounded by
`LARGE_OBJ_SIZE_MAX`), `heap_tag` to `i16` (−2), `free_is_zero`/`purged` into
the existing `flags` byte (−2). That is 8; **the remaining 8 needs a pointer to
go**, and both `next`/`prev` are load-bearing for the cross-segment page queues.
Not attempted — it touches every `Page` field access and the
`size_of::<Segment>() <= SEGMENT_SLICE_SIZE` assert, and is not a change to
start without room to gate it properly.

### P2 / P3 — status

**P2 `aligned` (+21.4 Ir/op): mechanism found, NOT built.** The plan guessed we
lacked a natural-fit fast path; that was wrong — `Heap::malloc_aligned_at` has
one, and the counts show both sides take a fast path ~93.75% of the time. The
real difference is what the fast path costs: ours proves alignment from the size
class via `bins::good_size(size)` and then `malloc(size)` recomputes the same
bin, while upstream tests the actual next free block with a single AND against
`align-1`. That is the classic compute-to-decide-then-recompute-to-use. Worth
doing, but `posix_memalign` is rare in the verdict workloads, so its
whole-program value is ~0 and it ranks below the `Page` shrink.

**P3 `usable` (+2.0 Ir/op): not attempted.** 32 vs 30 instructions; correctly
last.

## Harness defects found while building this (both fixed)

- **`op_pair_touch` measured nothing.** Plain stores into a block that is freed
  without being read are dead, and GCC removed them — the op then measured
  byte-identical to `small` on all three arms, which is how it was caught.
  The writes now go through a `volatile` pointer and are read back.
- **`opscan.sh` originally reported only ratios.** With constant caller overhead
  in both arms, ratios understate the real difference; the delta column is the
  one that carries meaning, and the ranking is now computed from it.
