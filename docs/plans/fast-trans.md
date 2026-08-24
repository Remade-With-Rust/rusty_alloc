# fast-trans — the division inventory

**Date opened:** 2026-08-22 · **Method:** `rusty-fast-transcendentals`, applied
to an allocator · **Status:** survey complete, nothing landed

## Why a transcendentals skill points at division here

The skill's premise is not "libm is slow". It is that **a scalar op with no
machine instruction behind it is a barrier** — one `exp` per element keeps a
loop scalar while eight lanes idle, and the fix is to replace it with arithmetic
the machine actually has.

rusty_alloc has no transcendentals. It has the same *shape* of problem in
integer division:

|                       | transcendental in a kernel                       | integer `div` in the allocator                                                                                                                                          |
| --------------------- | ------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| why it hurts          | no SIMD instruction; blocks vectorisation        | 20–40 cycle latency, **not pipelined**; blocks the dependency chain                                                                                                     |
| the cheap replacement | range-reduced polynomial                         | shift, or multiply by a precomputed inverse                                                                                                                             |
| the trap              | leaving a second libm call (`round`) in the loop | using the **wrong kind** of inverse and getting wrong answers                                                                                                           |

The skill's targeting rule transfers directly: **check which ops already have an
instruction before replacing anything.** For division that means asking whether
the divisor is a compile-time power of two — if it is, the compiler already
emitted a shift and there is nothing to win.

## What the survey found

**92** `/` and `%` sites in shipped source (`crates/*/src/**.rs`, comments and
string literals excluded).

|  class | sites                                                                                                                                                | verdict                                                                            |
| -----: | ---------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| **57** | divisor is a compile-time power of two (`SEGMENT_SIZE`, `SEGMENT_SLICE_SIZE`, `INTPTR_SIZE`, `64`, `8`…)                                             | **already a shift. Do not touch.** This is the skill's `sqrt`/`min`/`max` column.  |
| **27** | real division on a cold path — `stats.rs` formatting, `rusty_alloc_bench`, `rusty_alloc_wasm`, tests, `visit_segment_blocks`, `arena.rs` MiB display | **leave.** Diagnostics and harnesses; a `div` there costs nothing anyone measures. |
|  **7** | real division on a path the allocator actually runs — **5 distinct issues**, D4 being three lines of one function                                    | **the whole opportunity, listed below.**                                           |
|      1 | a `/` inside a format string (the scanner counts it twice)                                                                                           | —                                                                                  |

That ratio is the finding, and it is the reason to survey before optimising. The
instinct on seeing "34 real divisions" is to fix 34 things; **27 of them are in
code that runs once per process, or only in a test, or never in a shipped
build.** The arithmetic: 57 + 27 + 7 + 1 = 92.

## The hot five

| #   | site                               | expression                              | divisor                                           | when it runs                                                                                              |
| --- | ---------------------------------- | --------------------------------------- | ------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| D1  | `segment.rs:210` `page_index`      | `(page - base) / size_of::<Page>()`     | **88**, constant, not a power of two (`= 8 x 11`) | every `unalign`, every span free and coalesce (5 call sites)                                              |
| D2  | `alloc.rs:34` `unalign`            | `(off / bsize) * bsize`                 | `block_size`, **runtime**                         | every free of a pointer from an aligned allocation                                                        |
| D3  | `page.rs:1061` `page_extend`       | `(4096 / bsize).max(1)`                 | `block_size`, **runtime**                         | every free-list refill                                                                                    |
| D4  | `heap.rs:649,663,665` `fresh_page` | `(slices * SEGMENT_SLICE_SIZE) / bsize` | `block_size`, **runtime**                         | every page carve (×3 in one function)                                                                     |
| D5  | `segment.rs:597` coalesce          | `slice_offset / slot_stride()`          | **88**, constant, not a power of two              | every span free that merges left                                                                          |

`docs/opps.md` #1 already named D1 ("`page_index` division on the refill path")
and it is still open.

## The two techniques, and why confusing them is the trap

This is the analogue of the skill's *"you removed `exp` and left `round`"* — the
mistake does not make the code slow, it makes it **wrong**, and only on some
inputs.

### Exact division — valid only when the dividend is a known multiple

If `a` is guaranteed divisible by `d`, then `a / d` equals
`(a >> tz) * inverse(d >> tz)` in wrapping arithmetic, where `tz = d.trailing_zeros()`
and `inverse` is the modular inverse of the remaining odd factor. One shift, one
multiply, no `div`.

**This already exists in the repo.** `page::odd_mod_inverse` (page.rs:101,
Newton iteration, `const fn`) and the `Page::bs_inv` field (page.rs:459),
computed in `fresh_page` (heap.rs:667) and used at page.rs:165 — but **only by
the `blockmap` feature**. The default build carries the field and never reads
it.

**If the dividend is not a multiple, this silently returns garbage** — a
plausible-looking index, not a crash.

### True reciprocal — needed when the dividend is arbitrary

For `off / bsize` where `off` is an interior offset, exact division does not
apply. That needs a magic-number reciprocal (multiply-high then shift,
libdivide-style), which is more instructions and needs a per-divisor constant
plus a proof of the shift width.

### Which applies where

| #   | dividend always a multiple of the divisor?                                                                                                                                    | technique                                                    |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| D1  | **yes** — `page - base` is an offset into `[Page; 512]`, always `k * 88`                                                                                                      | exact division: `>> 3` then `* odd_mod_inverse(11)`          |
| D5  | **yes** — `slice_offset` is only ever written as a multiple of `slot_stride()` (segment.rs:370, :774)                                                                         | same as D1                                                   |
| D2  | **no** — `off` is an arbitrary interior offset; the `/ *` pair is a round-down                                                                                                | reciprocal, **or** restructure so the division is not needed |
| D3  | no, and it does not need to be exact (`.max(1)` then `.min(...)`)                                                                                                             | hoist, or reciprocal, or accept                              |
| D4  | no                                                                                                                                                                            | precompute once per page — see below                         |

## Plan

Ordered by confidence, not by size. Each is a separate brick, measured on its
own, reverted if flat — the house rule.

The stride is not a guess: the repo states it in two places
(`segment.rs:78`, `segment.rs:353` — "88, not a power of two, so a real
`imul`"), and `88 = 8 x 11`, so the odd factor to invert is 11.

**T1 — D1 `page_index` by exact division.** Highest confidence: the divisor is
the compile-time constant 88, the dividend is provably a multiple, and the
helper is already written and already unit-tested for the blockmap. Replace the
`/ size_of::<Page>()` with `>> 3` and a multiply by
`odd_mod_inverse(size_of::<Page>() >> 3)`, as a `const`. A `debug_assert_eq!`
against the real division keeps the proof visible. Expect one `div` removed from
five call sites.

**T2 — D5, the same substitution** at the coalesce site, sharing T1's constant.

**T3 — D4 `fresh_page`.** Three divisions by the same `bsize` in one function,
and `bs_inv` is computed in that very function (heap.rs:667). Compute
`reserved` once instead of three times, or reuse the inverse. This is the
skill's "hoist the invariant" move before any cleverness.

**T4 — D3 `page_extend`.** `4096 / bsize` is loop-invariant per page and could
live in the `Page` beside `bs_inv` — but the extend path was measured this
session at 2,458 calls against 91.5 M frees on cfrac, so **measure before
touching it**. It may be entirely cold in practice despite looking hot.

**T5 — D2 `unalign`.** Last, and possibly *never*: it only runs on frees of
aligned allocations, and a full reciprocal is real complexity. Check the
frequency first (`aligned` op, `rptest`) and leave it if the rate is low.

## Gates

Per the skill's section 5, adapted — these are integers, so bit-identity **is**
available and is therefore mandatory:

1. **`debug_assert_eq!` against the real division** at every converted site, so
   the equivalence is checked on every debug run rather than argued in a comment.
2. **An exhaustive test over every bin's block size** that the substitution
   equals `/`. One already exists for the blockmap
   (`index_matches_real_division_for_every_bin`, page.rs:1114) — extend it.
3. **`cargo test --workspace --all-features` by exit code**, not by grepping for
   `FAILED`. This allocator aborts silently (`double_free_abort`,
   `blockmap_abort`, `corrupt_free_list_abort` all call `std::process::abort()`
   with no message), and a text grep reports a passing run — that mistake hid a
   real bug earlier in this repo's history.
4. **`bench/opscan.sh` + `bench/datasweep.sh`** — no op regressed, all six
   allocator arms clean.
5. **Revert if flat.** A `div` that the branch predictor was already hiding
   costs nothing to remove and nothing to keep; the comment recording the flat
   is worth more than the change.

## What this plan does not claim

No measurement has been taken yet. The survey says where the divisions *are* and
which are arithmetically removable; it does **not** say any of them is costing
measurable time. `page_extend` is the cautionary case — it looks like a hot path
and fired 2,458 times against 91.5 M frees on cfrac.

The skill's own warning applies: *a win on one op is not a licence to rewrite
its neighbour.* Land T1, measure it, and let the number decide whether T2–T5
happen at all.

## Results — every site hammered, 2026-08-22

The plan above was written from a **source scan**. Hammering it started by
checking the same question against the **binary**, and that reversed two of its
five headline targets. Both readings are recorded; the disassembly wins.

### The measurement that reframed it

`objdump` over the shipped `librusty_alloc_override.so`: **78 divide
instructions**, attributed to their enclosing symbol.

| symbol                                                                                                                                                 | divides | what it is                                                                      |
| ------------------------------------------------------------------------------------------------------------------------------------------------------ | ------: | ------------------------------------------------------------------------------- |
| addr2line / gimli / rustc_demangle                                                                                                                     |      12 | backtrace machinery, not ours                                                   |
| `visit_segment_blocks`                                                                                                                                 |       6 | the heap-walk API                                                               |
| `Heap::try_guarded`                                                                                                                                    |       6 | `guarded` sampling, off by default                                              |
| `segment_free`, `huge_alloc`, `huge_free`, `os::purge`, `malloc_aligned_at_slow`                                                                       |  4 each |                                                                                 |
| `malloc_generic_walk`                                                                                                                                  |       3 | **`page_extend` ×2 + `fresh_page`**                                             |
| nine more (`span_alloc`, `alloc_aligned`, `usable_size_slow`, the aligned realloc family, …)                                                           |  2 each |                                                                                 |
| `free_general`                                                                                                                                         |       2 | **`unalign`'s `off / bsize`** — one division, emitted as LLVM's 32/64-bit split |
| `init::thread_done`                                                                                                                                    |       1 |                                                                                 |

**`page_index` does not appear. Neither does the coalesce follow-back.** D1 and
D5 — the two the plan ranked *highest confidence* — cost nothing: LLVM
strength-reduces division by **any** compile-time constant, not only powers of
two, so `/ 88` was already a multiply-high and a shift. The plan's premise for
them was wrong, and reading the binary is what showed it.

**Only a RUNTIME divisor emits a `div`.** That is the corrected targeting rule,
and it is the exact analogue of the skill's own: check what the machine already
does before replacing anything.

### D3 — the one real removal, measured, and refuted

`page_extend`'s `(4096 / bsize).max(1)` is a genuine `div` by a runtime value,
inlined into `malloc_generic_walk` at both call sites. The quotient depends only
on the bin, `bin_size` is a `const fn`, so it becomes a compile-time table:
`bins::EXTEND_BATCH` (kept, with tests proving it equals the division for every
bin).

Wired in, it removes **3 of the 78** divide instructions. And it measures:

|                                                                                                                                                                                                      |        before |                       after |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------: | --------------------------: |
| `small`, `med`, `big`, `mixed`, `aligned`, `shbench`                                                                                                                                                 |             — |               **identical** |
| `liveset`                                                                                                                                                                                            |         70.50 |                       70.51 |
| cfrac allocator Ir                                                                                                                                                                                   | 3,445,599,360 | **3,445,628,817** (+29,457) |

**Flat everywhere and slightly negative on the one exact instrument.** Reverted.
`page_extend` runs 2,458 times against 91.5 M frees on cfrac — the `div`'s
latency overlaps the surrounding work, and removing a slow instruction from a
cold path buys nothing. The refutation is recorded at the call site so the next
person does not re-derive it.

### D2 — LANDED. Four divides removed from the free path

`unalign`'s `(off / bsize) * bsize` is the highest-value division here, because
the function inlines into `free_general`, `usable_size_slow`,
`malloc_aligned_at_slow`, `realloc_aligned_at`, `rezalloc_aligned_at` and three
`mi_heap_*` wrappers — **one source division, emitted right across the binary.**

Two facts remove it, and the first is the one the earlier attempt missed:

1. **A `SINGLE_BLOCK` page holds one block, and it starts at `area`.** The old
   code reached that same answer through `(off / bsize) * bsize` with
   `off < bsize` — *paying a division to compute zero*. An early return is
   strictly less work, and it also excludes the only pages whose `block_size`
   is not a bin size (`slices * SEGMENT_SLICE_SIZE`, heap.rs:771, odd part
   arbitrary).
2. **Every BIN size is `odd << k` with odd in {1,3,5,7}** — bins 1..=8 are
   `bin * 8`; above that `((5+m) << (b-2)) * 8` with `m` in 0..=3. Shift the
   power of two out and the division is by one of four compile-time constants,
   each strength-reduced to a multiply. Exact for an arbitrary dividend, unlike
   the blockmap's `odd_mod_inverse`.

The first attempt failed for an instructive reason worth keeping: with a
fallback arm computing `n / odd`, **LLVM merged all five paths back into the
single `div`** the change existed to remove — divide count unmoved at 78. The
fallback must DIVERGE. It is `unreachable!()`, safe Rust rather than
`unreachable_unchecked`, and unreachable in fact because step 1 removed the
only case that could reach it.

|                                                                                                                                                                                                          |        before |                   after |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------: | ----------------------: |
| divide instructions in the `.so`                                                                                                                                                                         |            78 |                  **74** |
| `free_general`                                                                                                                                                                                           |             2 |                   **0** |
| `usable_size_slow`                                                                                                                                                                                       |             2 |                   **0** |
| `rezalloc_aligned_at`                                                                                                                                                                                    |             2 |                   **0** |
| `aligned`, `usable`, `realloc`, `small`, `med`                                                                                                                                                           |             — |          byte-identical |
| cfrac allocator Ir                                                                                                                                                                                       | 3,445,599,360 | 3,445,626,672 (+27,312) |

**Landed, and the +27,312 is stated rather than hidden.** cfrac performs no
aligned allocation, so the changed path barely executes there — that delta is
code-layout drift, not the new code running. The reason to keep it anyway,
where D3 was reverted: this instrument counts INSTRUCTIONS, and a `div` is one
instruction but 20-40 cycles that do not pipeline. Ir systematically
under-counts exactly what was removed. The change is free where it does not run
and strictly cheaper where it does.

That is a departure from revert-if-flat and is flagged as one. What justifies it
is that the substitution is not code motion: it deletes a latency hazard the
instrument cannot see, and it deletes a division that was computing a known
zero.

Correctness, because `unalign` computes a pointer: **86 tests**, and
`datasweep` at scale 2 across six allocator arms — 573,640 checks each,
including the alignment matrix from 8 bytes to 64 KiB, which is the phase that
drives this path — all clean.

### D6 — `page_align_up`: one line, 26 divides

`os::page_align_up` was `size.max(1).div_ceil(ps) * ps`, and `ps` is a runtime
value, so that is a real `div` — inlined into `purge`, `alloc_aligned`,
`segment_free`, `huge_free` and `huge_alloc`, which is why one line accounted
for a third of the binary's divisions.

A page size is a power of two on every platform this builds for. Rounding up to
a power of two is `(size + ps - 1) & !(ps - 1)`.

|                                                                                                                                                                                                                    |        before |         after |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------: | ------------: |
| divide instructions                                                                                                                                                                                                |            74 |        **48** |
| `os::purge`, `page_align_up`, `alloc_aligned`, `segment_free`, `huge_free`                                                                                                                                         |     4/2/2/4/4 |         **0** |
| `huge_alloc`                                                                                                                                                                                                       |             4 |         **2** |
| cfrac allocator Ir                                                                                                                                                                                                 | 3,445,626,672 | 3,445,626,607 |

**26 divides for one line, no cost anywhere.** The power-of-two assumption is
not new — it is asserted against the live page size by an existing test — and a
`debug_assert` now states it at the site.

### D7 — `is_aligned_to`: modulo by alignment is a mask

Five sites asked `x.is_multiple_of(align)` with a RUNTIME `align`: the two
aligned-realloc entries and three decisions inside `malloc_aligned_at_slow`.
Alignment is a power of two by the C contract, and the same function already
relied on that two lines away (`& !(align - 1)` appears twice in it), so the
test is a mask.

`bins::is_aligned_to` keeps the power-of-two check rather than assuming it,
which makes the function total: a non-power-of-two answers `false`, and that is
the **conservative** direction at every call site — each falls back to the
general path instead of taking an in-place or same-bin shortcut. A bare mask
would be unsound there (`4 & (3-1) == 0` claims 4 is aligned to 3).

|                                                                                                                                                                                                                       | before |             after |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -----: | ----------------: |
| divide instructions                                                                                                                                                                                                   |     48 |            **40** |
| `malloc_aligned_at_slow`                                                                                                                                                                                              |      4 |             **0** |
| **`aligned` Ir/op**                                                                                                                                                                                                   |  92.94 | **92.75 (-0.19)** |

The first *measurable* win of the sweep, and the only one: every other op is
byte-identical.

### Where the sweep finished — 78 divides to 18, and none of them ours

| step                                              | divides | what changed                                                        |
| ------------------------------------------------- | ------: | ------------------------------------------------------------------- |
| start                                             |  **78** |                                                                     |
| D2 `unalign` SINGLE_BLOCK early-out               |      74 | a division that computed a known zero                               |
| D6 `page_align_up` mask                           |      48 | **26 from one line** — `div_ceil(ps) * ps` is a mask                |
| D7 `is_aligned_to` mask, 5 sites                  |      44 | `aligned` **-0.19 Ir/op**                                           |
| D8 `Random::below` Lemire multiply-shift          |      40 | `% n` -> one `mul`; strictly fewer instructions                     |
| D9 `visit_segment_blocks`, SINGLE_BLOCK-guarded   |      34 | one flags load replaces a `div` per block                           |
| D10 `segment::huge_alloc` alignment test          |      32 | `huge` 644.00 -> 642.00                                             |
| D11 `div_by_block_size` in `unalign`              |      28 | 4 divides across `free_general` + `usable_size_slow`                |
| D12 `is_aligned_to` in the two FFI `*_aligned_at` |      22 | 6 divides across three `mi_heap_*` wrappers, at **zero** cost       |
| D13 `page_extend` bound as a **shift**            |      19 | 3 divides, and cfrac **-1,500**                                     |
| D14 `div_by_block_size` in `fresh_page`           |  **18** | the last one                                                        |

**60 divide instructions removed. The allocator contains no division.** The 18
that remain are addr2line / gimli / rustc_demangle / `core::slice` sorting —
backtrace machinery std links in, not code we wrote or call.

Cost, honestly: cfrac +35,619 Ir on 3.446e9 (**+0.001%**), `aligned` -0.19 vs
where the campaign started, `huge` back to its original 644.00 (D10's -2.00 was
given back by D11's code layout), every other op byte-identical.

### The mistake this campaign made, and the test that caught it

The first pass stopped at 32 and wrote four refutations. **Three of them were
wrong**, and they were wrong in the same way: an Ir delta was read as the cost
of new code *executing*, when the code in question never ran.

The test that separates the two is one question — **does this code execute on
the workload that showed the cost?**

- `unalign`'s division sits below a `SINGLE_BLOCK` early-out. Huge pages return
  above it; cfrac performs no aligned allocation at all; a callgrind profile of
  cfrac and `liveset` shows no `unalign` frame. Its +27,312 on cfrac and +2.00
  on `huge` were **code placement**, not work — those instructions cannot retire
  because control never reaches them. Confirming it: `aligned`, the one op that
  exercises the path, reads **92.75 either way, to the last digit**.
- The two FFI wrappers cost **nothing at all** — the number was never taken,
  because they were assumed to share `unalign`'s verdict.
- `page_extend` was the real finding, and both attempts on it failed for a
  reason neither recorded: they *kept the computation* and only changed its
  form, so each added a live value to `malloc_generic`'s fast path. The fix is
  to not compute it. `reserved` is `(slice_count * SEGMENT_SLICE_SIZE) / bsize`,
  so `4096 / bsize` is exactly `reserved / (slice_count * 16)`; every span that
  reaches here is 1 or `MEDIUM_PAGE_SLICES` (= 8) slices, so the divisor is a
  power of two and the bound is a **shift of `reserved`** — a value the `min` on
  that same line already holds live. Nothing arrives to replace what leaves,
  which is why it is the only substitution here that made cfrac *faster*.

Only the fourth refutation survived contact, and it inverted:

- `fresh_page` genuinely costs. +9,872 on cfrac over **1,446** fresh pages
  (`span_from_segments` call count) is **+6.8 Ir each** — the match tree's exact
  size, so this one is execution, not layout. It lands anyway, and the
  arithmetic is the argument: pay ~7 predictable instructions to delete one
  64-bit `div` at 20-40 unpipelined cycles. Ir counts a `div` as **1**, which is
  the one place this instrument is not merely noisy but systematically,
  knowably wrong.

The earlier refutation of that same site read +48,605 — it was measured
*combined* with the `page_extend` substitution that was itself the entire loss.
**Never refute two changes with one number.**

### Where the rule landed

The first pass's rule — *replace a `div` with one or two ALU ops, never four* —
was a good rule with a bad corollary. Sharpened:

> Replace a `div` with **nothing** where the algebra allows it (D6, D13: the
> value is already in a register, or the divisor divides a known multiple).
> Where it does not, count the executions before you count the instructions.
> `div` is 1 Ir and 20-40 cycles, so a substitution costing under ~10 Ir on a
> path that runs thousands — not millions — of times is a cycle win that Ir
> will always report as a loss.

The sh8bench precedent still holds, and still cuts both ways: a measured cost in
the visible domain with an unverifiable gain in the invisible one does not land.
What changed is that **"measured" now requires proving the code ran.** An Ir
delta on unexecuted code is not a measurement of that code.

### Verdict for every site

| site                                                    | verdict                                                                                         |
| ------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `ra/segment.rs:210` `page_index` ÷88                    | **no `div` emitted** — constant folds to a magic multiply. Verified absent.                     |
| `ra/segment.rs:597` coalesce ÷`slot_stride()`           | **no `div` emitted** — same.                                                                    |
| `ra/page.rs:1082` `page_extend` ÷`bsize`                | **REMOVED (D13)** — the bound is `reserved >> (4 + log2 slice_count)`. 3 divides; cfrac -1,500. |
| `ra/heap.rs:649,665` `fresh_page` ÷`bsize`              | **REMOVED (D14)** — `div_by_block_size`. +6.8 Ir per fresh page, buys a 20-40 cycle `div`.      |
| `ra/alloc.rs:34` `unalign` ÷`bsize`                     | **REMOVED (D2 + D11)** — SINGLE_BLOCK early-out, then `div_by_block_size`. 8 divides total.     |
| `ra_ffi/lib.rs:1174,1259` `*_aligned_at` ÷`alignment`   | **REMOVED (D12)** — `is_aligned_to`. 6 divides across three wrappers, at zero measured cost.    |
| `ra/heap.rs:1812` `visit_segment_blocks`                | **REMOVED (D9)** — one flags load replaces a `div` per block.                                   |
| `ra/random.rs:139` `% n`                                | **REMOVED (D8)** — Lemire multiply-shift.                                                       |
| `ra/os.rs:92`, `page_align_up`, `alloc_aligned`         | **REMOVED (D6)** — 26 divides from one mask.                                                    |
| `ra/stats.rs:121,125,145,201` ÷ constants               | **no `div` emitted** — magic multiply.                                                          |
| `ra/bins.rs:140`, `ra/page.rs:1123`, `ra/random.rs:396` | `#[cfg(test)]` — not in any shipped artifact.                                                   |
| `ra/prim/windows.rs:208`                                | Windows-only; absent from this `.so`.                                                           |
| `ra_bench/**` (12 float), `ra_wasm` (1)                 | separate crates, never linked into the shipped allocator.                                       |
| `ra/stats.rs:36`                                        | scanner false positive: a `/` inside a format string.                                           |

### What the hammer produced

Of the 34 source-level divisions in the inventory, **21 never emitted an
instruction** — the compiler had already folded them, which is the correction
worth more than any single edit — 1 was a scanner artifact, and **every one of
the remaining 12 that reached the shipped `.so` is now gone.**

Gate, on the default build: clippy 0, `blockmap` builds clean, 32 suites /
86 tests / 0 panics by exit code, datasweep 573,640 checks × 6 allocator arms,
`sweep-all` passed, and all 13 opscan ops at or ahead of mimalloc.

The two durable artifacts are the sharpened rule above and the layout-versus-
execution test — because the first pass through this list produced four
refutations from real numbers, and three of them were measuring nothing.

### One thing this campaign found but did not cause

`cargo test -p rusty_alloc --features debug_checks` fails
`foreign_pointer_is_rejected_in_debug_builds` (foreign_free.rs:89). The child
process dies — so `free` does reject the foreign pointer — but it dies with
**empty output**, meaning a hard crash rather than the named assertion the test
looks for.

**This is pre-existing.** Verified by stashing every source change from this
campaign and re-running against clean `v1.0.1` (edebae3): it fails identically.
The default, `blockmap`, and `secure` builds are unaffected — 18 suites each,
71 and 69 tests, all passing. Filed here rather than fixed, alongside issue #8,
because it is a debug-only instrument fault and not a defect in shipped code.

## Round two — the 21 sites this plan wrote off

The section above closed with *"21 never emitted an instruction"* and treated
that as the end of the inventory. **It was not**, and the error is the same
species as the three refutations corrected above: a wrong equivalence, stated
confidently.

> "No `div` emitted" is not "no division". A `/` by a non-power-of-two
> **constant** compiles to `movabs <reciprocal>; mul; shr` — a 3-instruction
> magic multiply with a 10-byte immediate. A `div` scan cannot see it, and 21
> sites were cleared by exactly that scan.

`size_of::<Page>()` is **88** — `8 * 11` — and dividing by it is the shape
above. It appeared **five** times in the shipped `.so` and had a name already:
`docs/opps.md` #1, open since it was filed.

### D15 — `slice_offset` in slices, not bytes

`span_free`'s coalesce-left recovered a slot index with
`slice_offset as usize / slot_stride()`. The field's own doc justified bytes:

> *Bytes, because this field is read on the hottest path in the allocator:
> `page_of` follows it back on every free.*

**That justification had expired.** `Segment::page_off` took over `page_of`, and
in a release build the only remaining reader of `slice_offset` that does any
arithmetic is this one line — the `page_of` reader is inside a `debug_assert`.
Bytes were buying nothing and costing a magic multiply.

In slices the read is a bare subtract, and both loops that WRITE the field drop
a multiply each — including `huge_alloc`'s 511-iteration init loop, whose
comment specifically blamed the 88-byte stride for not vectorising.

### D16 — `page_index` as a shift, with no new field

`opps.md` #1 proposed storing the slice index in a spare `Page` slot and
recorded the risk as 1 KiB of extra segment header. **Neither was needed.**
`Page::area` already holds `seg + idx * SEGMENT_SLICE_SIZE`, and
`SEGMENT_SLICE_SIZE` is 2^16:

```rust
let idx = ((*page).area.addr() - seg.addr()) / SEGMENT_SLICE_SIZE; // a shift
```

The slot-pointer form stays as the `debug_assert`. Five magic multiplies to
zero, at zero header cost.

**And it surfaced a latent trap.** `span_mark` writes `area` for every carved
span, free or allocated — but `huge_alloc` builds its one slot by hand and
**never wrote the field**, so huge pages carried a null `area`. Nothing read it
before; `unalign` and `page_index` both do now, and huge pages set
`SINGLE_BLOCK` so they do *not* take `unalign`'s early return. Caught here by
`huge` moving 644.00 -> 655.00 on opscan while the change was half-applied —
the op reported a number rather than crashing, which is the failure mode worth
remembering. Fixed in the same change, with a `debug_assert` tying it to
`page_area(seg, 1)`.

`unalign` gains separately: it called `page_area(seg, page_index(seg, pg))` to
recompute a value the page was already carrying. One load now replaces
`segment_of`'s mask, the magic divide and the scale back up.

### D17 — the Windows clock's `u128` division

`prim/windows.rs` converted QPC ticks to nanoseconds with
`(count as u128 * 1e9) / freq as u128`. A 128-bit division is **not an
instruction** on x86-64 — it is a `__udivti3` libcall.

`QueryPerformanceFrequency` is fixed for the life of the process, and every
TSC-backed Windows since 8 reports 10 MHz, which divides 1e9 exactly. Caching
that quotient once makes the conversion a single 64-bit multiply; an
inexact frequency (the pre-Win8 3.579545 MHz PIT) falls back to the `u128`
form. Three tests pin it against the `u128` reference, including that fallback
and a full day of ticks.

This one is cold — `clock_now` has three callers, all seeding or stats — and it
is on a platform this box cannot profile, so it is recorded as a structural win
(a libcall removed), not a measured one. It was verified natively on Windows:
tests pass and the function is clippy-clean there.

### Where round two finished

| change                                          | ÷88 magic multiplies |     cfrac |
| ----------------------------------------------- | -------------------: | --------: |
| start of round two                              |                    5 |         — |
| D15 `slice_offset` in slices                    |                    4 |      flat |
| D16a `unalign` reads `Page::area`               |                    3 |      flat |
| D16b `page_index` via `area`, huge `area` fixed |                **0** | **-17,057** |

Plus D17, off-box.

**Final: no `div` and no magic-multiply division by `size_of::<Page>()`
anywhere in the allocator.** What remains in our symbols is 15 constant
divisions inside `bins::div_by_block_size` (`n/3`, `n/5`, `n/7` — the
*deliberate* replacement of a runtime `div`, in `free_general`,
`usable_size_slow` and the `visit_segment_blocks` diagnostic) and 2 in
`stats::process_info`'s formatting.

Everything else in the 21: `os.rs:103`'s `% alignment` is inside a
`debug_assert_eq!`, `page.rs:1160` is `#[test]`, `heap.rs:1682` and `:1699`
divide by `INTPTR_SIZE` (8, a shift), `arena.rs:442` by `1024 * 1024` (a
shift), and the `rusty_alloc_bench` float divisions are one per printed
benchmark line. Those are genuinely nothing — but that is now a statement
about each one, not about a scan that could not see past `div`.

Gate: clippy 0 on both Linux (`--all-features`) and Windows for the code
touched, 32 suites / 86 tests / 0 panics by exit code, `blockmap` 18 suites,
`secure` 18, datasweep 573,640 checks × 6 arms, `sweep-all` passed, all 13
opscan ops at or ahead of mimalloc (`huge` +5.00 against a 53,362 mimalloc
figure), and `docs/opps.md` #1 closed.

## Round three — exact division, and the complete site-by-site ledger

Round two closed by saying the remainder was "genuinely nothing". Two of those
were still winnable, and the reason they were missed is a third variant of the
same error: **"a constant division is already optimal" is false when the
dividend is a known multiple.**

Dividing by a constant is `movabs <reciprocal>; mul; shr` — and `mul` clobbers
`rdx`. Dividing an **exact multiple** by that constant is the modular inverse:
a single `imul`, no high half, no shift, no `rdx`. This plan documented that
distinction in "Exact division — valid only when the dividend is a known
multiple" and then failed to apply it.

### D18 — `exact_div_by_block_size`, and `visit_segment_blocks` to zero

`bins::exact_div_by_block_size` derives its four inverses at compile time from
`page::odd_mod_inverse` (which had to come out from behind `#[cfg(feature =
"blockmap")]` — it is now used in every build). Derived, not transcribed: a
mistyped digit would produce plausible garbage rather than a compile error.

`visit_segment_blocks` marks free-list blocks, and a free-list pointer is a
block **start** — so `addr - area` is always an exact multiple of `bsize`. Its
nine magic multiplies went to **zero**. The `off < cap` bound that follows still
catches a corrupt link, so the narrower precondition costs no safety.

Two tests pin it: the cheap form must equal real division for every bin size
across a range of multiples, and the general form must keep working on the
interior offsets where only it is legal.

`unalign` cannot use it. Its whole purpose is interior pointers from aligned
allocations, which are *not* multiples — that is why `div_by_block_size` exists
and why its six remaining magic multiplies in `free_general` and
`usable_size_slow` **are the win**, not a residue of one: each replaced a
runtime `div` at 20-40 unpipelined cycles.

### D19 — `os.rs`'s runtime modulo

`debug_assert_eq!((a.ptr as usize) % alignment, 0, ..)` is a real `div` by a
runtime divisor. Release-free, but `debug_checks` runs the entire datasweep
corpus, so it executes a great many times in the configuration that exists to
be run. Now `bins::is_aligned_to` — the same mask as the five release sites in
D7.

### The ledger — every site in the inventory

The survey found 92 `/` and `%` sites; 57 have compile-time power-of-two
divisors and were never candidates. These are the other 35.

| site | shape | outcome |
| ---- | ----- | ------- |
| `alloc.rs:34` `unalign` | `off / bsize`, runtime | **WIN D2+D11** — SINGLE_BLOCK early-out, then `div_by_block_size`. 8 `div`s. |
| `page.rs:1082` `page_extend` | `4096 / bsize`, runtime | **WIN D13** — `reserved >> (4 + log2 slice_count)`. 3 `div`s, cfrac −1,500. |
| `heap.rs:649` `fresh_page` | `(slices*SLICE)/bsize`, runtime | **WIN D14** — `div_by_block_size`. |
| `heap.rs:665` `fresh_page` (blockmap) | same, minus bitmap | **WIN D14** — same. |
| `segment.rs:210` `page_index` | `/ size_of::<Page>()` = 88 | **WIN D16** — shift via `Page::area`. 5 magic multiplies, cfrac −17,057. |
| `segment.rs:597` coalesce-left | `slice_offset / 88` | **WIN D15** — `slice_offset` now counts slices; a bare subtract. |
| `heap.rs:1829` `visit_segment_blocks` | `off / bsize`, runtime | **WIN D9 then D18** — `div_by_block_size`, then the exact inverse. 9 magic multiplies to 0. |
| `random.rs:139` `below` | `% n`, runtime | **WIN D8** — Lemire multiply-shift. |
| `os.rs:92` `page_align_up` + `alloc_aligned` | `div_ceil(ps) * ps` | **WIN D6** — mask. **26** `div`s from one line. |
| `os.rs:103` alignment check | `% alignment`, runtime | **WIN D19** — `is_aligned_to` mask (debug builds). |
| `segment.rs` `huge_alloc` | alignment test | **WIN D10** — `is_aligned_to`. `huge` 644.00→642.00. |
| `ra_ffi:1174,1259` `*_aligned_at` | `% alignment`, runtime | **WIN D12** — `is_aligned_to`, 6 magic multiplies, zero cost. |
| 5 further `is_aligned_to` sites | `% align`, runtime | **WIN D7** — mask. `aligned` −0.19 Ir/op. |
| `prim/windows.rs:208` `clock_now` | `u128 / u128` | **WIN D17** — cached exact scale; removes a `__udivti3` **libcall**. |
| `bins.rs:85,86,87` | `n/3, n/5, n/7` | **This IS the win** (D2/D11/D14) — four constant divisions replacing one runtime `div`. Not a residue. |
| `heap.rs:1682,1699` | `/ INTPTR_SIZE` (8) | **No instruction exists.** `shr $3`, one instruction, already minimal. |
| `arena.rs:442` | `/ (1024*1024)` | **No instruction exists.** Power of two → shift; also inside a `format!` argument. |
| `heap.rs:1832` freemap | `off/64`, `off%64` | **No instruction exists.** Powers of two → shift and mask. |
| `page.rs:102` `odd_mod_inverse` | `n % 2` | **No instruction exists.** Power of two → `and $1`. |
| `stats.rs:121` FILETIME→ms | `/ 10_000` | **At the floor.** Constant, dividend arbitrary, so multiply-high + shift is minimal. Once per `mi_process_info`. |
| `stats.rs:125,201` | `/ 1_000_000` | **At the floor.** Same; two magic multiplies, once per stats print. |
| `stats.rs:145` timeval→ms | `/ 1000` | **At the floor.** Same. |
| `stats.rs:36` | — | **Not a division.** A `/` inside a format string; the scanner counted it twice. |
| `bins.rs:124` `EXTEND_BATCH` | `4096 / bs` | **DELETED in v1.1.0** — see Round six. Superseded by D13; restricting it to `pub(crate)` exposed it as dead. |
| `bins.rs:284,299`, `page.rs:1160`, `random.rs:412` | assorted | **Must stay.** `#[cfg(test)]` oracles — they are the independent reference the fast forms are checked against. Optimising an oracle to match its subject destroys the test. |
| `ra_bench/kernels.rs` × 12 (f64) | `ops / secs / 1e6` | **Declined, with reason.** One execution per printed benchmark line — unmeasurable — and re-associating to `a/(b*c)` to halve the `divsd`s perturbs reported figures in the last ulp. Changing benchmark arithmetic for no gain is a bad trade on a harness used for cross-run comparison. |
| `ra_wasm` × 1 | *(mischaracterised — see Round six)* | That crate has no float arithmetic. `BIG / 2` is a shift; `% 3000` is a constant modulo in a self-test, `publish = false`. |

Three honest categories, and they are not the same thing: **19 wins**, five
sites where *no instruction exists to remove* (verified in the disassembly, not
inferred from the source), and four where a change is possible but would make
the artifact worse — test oracles and benchmark reporting.

### Final state of the binary

- `div` instructions in our code: **0** (18 remain in std's backtrace machinery).
- Magic-multiply divisions by `size_of::<Page>()`: **0**.
- Magic multiplies remaining in our symbols: **8** — six in `div_by_block_size`
  as reached from `unalign`, which are the substitution that removed a runtime
  `div`, and two in `stats::process_info`'s display formatting.

Gate: clippy **0** on Linux `--all-features` and clean on Windows for the code
touched, 32 suites / **88** tests / 0 panics by exit code, `blockmap` 18 suites,
`secure` 18, datasweep 573,640 checks × 6 arms, `sweep-all` passed, all 13
opscan ops at or ahead of mimalloc, cfrac allocator **3,445,617,935**, `free`
21.000 Ir/call.

### The three-strike lesson

This inventory was declared finished three times, and each time the error was a
false equivalence that a scan could not see past:

1. *"An Ir delta is the cost of the new code"* — it is not, when the code never
   executes. Three refutations died on that.
2. *"No `div` emitted means no division"* — it does not; a constant divisor is a
   3-instruction magic multiply. Twenty-one sites were cleared by that scan.
3. *"A constant division is already optimal"* — it is not, when the dividend is
   a known multiple; then it is one `imul`.

Each was corrected only by reading the disassembly rather than the source, and
each time the remaining work was larger than the summary claimed.

## Round four — the benchmark harness, and measuring the floor

Round three declined the 13 `f64` divisions in `rusty_alloc_bench` /
`rusty_alloc_wasm` on the grounds that re-associating them "perturbs reported
figures in the last ulp". **That objection does not survive arithmetic.** A
last-ulp change in `f64` is a relative change of ~1e-16, on figures printed to
two decimals. It cannot alter a single character of output. The real reason
nothing had been done was that the harness had not been looked at.

### D20 — the bench harness to zero float divisions

Three distinct wins, none of which needed the ulp argument at all:

- **`as_secs_f64()` is itself a division, and it was called twice per line.**
  `larson`, `xmalloc` and `malloc-small` each computed it once for the `time=`
  field and again inside the throughput expression. Hoisted to a local: three
  divisions gone, and no rounding change whatsoever — it is the same call.
- **`X / secs / 1e6` is two `divsd` where one suffices.** Folded to
  `X / (secs * 1e6)`: three more gone, trading a `divsd` (13-20 cycles,
  unpipelined) for a `mulsd` (4, pipelined).
- **Both probe loops divide by the same `iters` in every arm.** `freepath_probe`
  does it four times and `tls_spike` three. One `let per_iter = 1.0 / iters as
  f64` per function turns seven divisions into two plus seven multiplies.

Result: **zero `divsd`/`divss` in the entire `rabench` binary.** Verified by
running it, not only by building it — `malloc-small` reports 89.42 Mops/s,
`tls-spike` still shows the ordering it exists to show (0.22 ns static atomic <
0.40 thread_local!+Cell < 1.49 OS TLS slot), `xmalloc` prints correctly.

### The floor, measured rather than asserted

The previous round listed five sites as "no instruction exists to remove" and
four as "at the floor". Those were *source-level* judgements — the exact species
of claim that was wrong three times in this document. Counted in the
disassembly instead:

| site | claimed | measured in the `.so` |
| ---- | ------- | --------------------- |
| `heap.rs:1682,1699` `bsize / INTPTR_SIZE` | "a shift" | **0 instructions.** No `shr`, no `div`, no magic constant — folded into addressing entirely. |
| `page.rs:102` `n % 2` in `odd_mod_inverse` | "an `and`" | **0 instructions.** `const fn`, fully folded; the symbol is not in the object. |
| `heap.rs:1832` `freemap[off/64] \|= 1<<(off%64)` | "shift and mask" | `shr $0x6` + `and $0x3f` — **2 instructions, the irreducible cost of a bitset index plus bit position.** |
| `arena.rs:442` `size / (1024*1024)` | "a shift" | one `shr $0x14`, inside a `format!` argument. |
| `stats.rs:121,125,145,201` display | "at the floor" | 2 magic constants in `process_info`, **0 `div`**. Constant divisor with an arbitrary dividend: multiply-high plus shift *is* the minimum. |

Two of the five turned out to cost **nothing at all** — better than claimed. The
other three are at a floor that is now a counted fact.

### What genuinely cannot be won, and why that is the right answer

- **`#[cfg(test)]` oracles** (`bins.rs:284,299`, `page.rs:1160`,
  `random.rs:412`). These are the independent references the fast forms are
  checked against — `page.rs:1160` computes `64 * 1024 / bs` by real division
  precisely so `EXTEND_BATCH` can be proven correct. Optimising an oracle into
  the shape of its subject deletes the test. Not shipped in any artifact.
- **`bins.rs:124`** builds a table that was measured and **not adopted**; it is
  a `const`-evaluated builder, so it emits nothing at runtime.
- **`stats.rs:36`** is a `/` inside a format string. It was never a division.

### Final accounting

| class | count | state |
| ----- | ----: | ----- |
| Wins landed (D2-D20) | **20** | each measured, each recorded above |
| Sites costing 0 instructions | 3 | verified absent from the object |
| Sites at a counted floor | 3 | `shr`, `shr`+`and`, magic multiply |
| Test oracles / unshipped | 6 | must stay real divisions |
| Not a division | 1 | format-string artifact |

**In the shipped allocator:** zero `div`/`idiv`, zero `divsd`/`divss`, zero
`__udivti3`-class libcalls, zero divisions by `size_of::<Page>()`. The eight
magic multiplies that remain are six inside `div_by_block_size` reached from
`unalign` — which *are* a win, each having replaced a runtime `div` — and two in
`process_info`'s display formatting.

**In the benchmark harness:** zero float division.

Gate: clippy **0** across the workspace with `--all-features`, and clean on
Windows for the code touched; 32 suites / **88** tests / 0 panics by exit code;
`blockmap` 18 suites, `secure` 18; datasweep 573,640 checks x 6 allocator arms;
`sweep-all` passed; all 13 opscan ops at or ahead of mimalloc; cfrac allocator
**3,445,617,935**; `free` **21.000** Ir/call.

### Postscript: four stops, three of them premature

This inventory was declared complete four times. The corrections, in order:

1. *"An Ir delta measures the new code"* — not when the code never executes.
2. *"No `div` emitted means no division"* — a constant divisor is a
   3-instruction magic multiply that a `div` scan cannot see.
3. *"A constant division is already optimal"* — not when the dividend is a known
   multiple; then it is a single `imul`.
4. *"An ulp of difference is a reason to decline"* — not on a figure printed to
   two decimals.

Every one was a source-level judgement that the disassembly contradicted. The
transferable rule is the cheap one: **count the instruction, do not reason about
it** — and when declining a change, make sure the reason is a measurement and
not a plausible sentence.

## Round five — the last eight

Round four ended with eight magic multiplies in our symbols and called six of
them "the win, not a residue of one". That was true of the *substitution* — each
had replaced a runtime `div` — and it quietly excused the **shape** the
substitution was written in.

### D21 — `div_by_block_size` branchless, by table

The four-arm `match` on the odd part compiled to a three-way compare tree plus
**three** `movabs; mul; shr` sequences, one per arm. Only one runs on any call,
so nine of those instructions were dead weight in `free_general` and again in
`usable_size_slow`.

Indexing a four-entry reciprocal table by `odd >> 1` removes the tree and two of
the three multiplies:

```rust
const RECIP32: [u64; 4] = [recip32(1), recip32(3), recip32(5), recip32(7)];
const fn recip32(d: u64) -> u64 { (1u64 << 32).div_ceil(d) }

((m * RECIP32[(odd >> 1) & 3]) >> 32) as usize
```

The `odd == 1` entry is exactly `2^32`, so the same multiply-high returns `m`
unchanged — no special case, and therefore no branch at all.

**Bounds, because 32-bit reciprocals are not unconditionally exact.** With
`R = ceil(2^32/d)` and `e = R*d - 2^32`, the identity holds for `m < 2^32/e`:
`e` is 0, 2, 4, 3 for d = 1, 3, 5, 7, so the tightest arm (d = 5) is exact to
2^30. Every caller is bounded by page geometry — the largest dividend any can
present is one page payload, `MEDIUM_PAGE_SLICES * SEGMENT_SLICE_SIZE` = 2^19,
and `k >= 3` shrinks it further, leaving 11 bits of headroom. Debug builds
assert it, and the existing equivalence test already spans dividends to 2^20.

| | before | after |
| --- | ---: | ---: |
| magic multiplies in `free_general` | 3 | **0** |
| magic multiplies in `usable_size_slow` | 3 | **0** |
| `free_general` instructions | 148 | **112** |
| `usable_size_slow` instructions | 93 | **55** |
| cfrac allocator Ir | 3,445,617,935 | **3,445,578,293** |

**−39,642 Ir on cfrac and 74 instructions of code**, with `aligned`, `usable`
and `realloc` byte-identical and `huge` improving 649.00 -> 647.00.

That also flips the campaign's sign. Against the pre-campaign baseline of
3,445,599,360, cfrac now sits at **-21,067 Ir** — the division work is a net Ir
*reduction*, not merely a cycle trade.

### The last two are irreducible, and that is now measured

`stats::process_info` converts nanoseconds to milliseconds and microseconds to
milliseconds, because `mi_process_info` documents millisecond fields. Two
constant divisions with arbitrary dividends; a multiply-high plus a shift is the
minimal form of each.

There was one visible redundancy worth testing: `clock_now()` builds
`sec * 1e9 + nsec` and `process_info` immediately divides by 1e6 — scaling
seconds *up* by a billion only to scale the sum back down. A `clock_now_ms()`
computing `sec * 1000 + nsec / 1_000_000` avoids the round-trip and needs no
bounds argument at all.

**It was built and counted, and the codegen is byte-for-byte identical:**

| | round-trip | direct ms |
| --- | ---: | ---: |
| `process_info` instructions | 387 | 387 |
| `movabs` | 2 | 2 |
| `mul`/`imul` | 8 | 8 |
| `shr`/`sar` | 9 | 9 |
| `div` | 0 | 0 |

LLVM already collapses it. The change was reverted rather than land new public
API surface for a measured zero — and unlike the declines earlier in this
document, that is a counted result and not a plausible sentence.

The remaining alternative would be a bounded 32-bit reciprocal for
`tv_usec / 1000` (exact, since `tv_usec < 10^6` and that arm's bound is ~6.1e6).
It saves at most one instruction, twice, in a diagnostic function called on
demand — in exchange for a new correctness precondition inside a public API's
CPU-time reporting. Declined on that trade, not on effort.

### Final state

**In the shipped allocator:** zero `div`/`idiv`, zero `divsd`/`divss`, zero
`__udivti3`-class libcalls, zero divisions by `size_of::<Page>()`, and zero
magic-multiply divisions on any allocation path. What remains is **two**, in
`process_info`'s display formatting, both proven minimal.

**In the benchmark harness:** zero float division.

Gate: clippy **0** workspace-wide with `--all-features`; 32 suites / **88**
tests / 0 panics by exit code; `blockmap` 18 suites; datasweep 573,640 checks x
6 allocator arms; `sweep-all` passed; all 13 opscan ops at or ahead of mimalloc;
cfrac allocator **3,445,578,293**; `free` **21.000** Ir/call.

## Round six — the audit, and the platform this document never looked at

Re-reading this plan against the code as it actually stands after v1.1.1 turned
up one substantial gap, one false claim, and several stale rows. The gap is the
interesting one, and it is structural rather than a missed line:

> **Every "zero divisions" claim above was measured on the Linux `.so`.**
> `crates/rusty_alloc/src/prim/windows.rs` is not compiled into that object, so
> no scan in five rounds ever read it.

### D23 — the Windows OS-allocation path

Emitting assembly for the `x86_64-pc-windows-msvc` target
(`cargo rustc -p rusty_alloc --target ... -- --emit=asm`, which needs no
disassembler and works on any host) found **10 integer division instructions**
where the Linux object has zero. Attributed:

| symbol | div instructions | source |
| ------ | ---------------: | ------ |
| `prim::alloc` | 4 | **two `is_multiple_of(try_alignment)`** on a runtime divisor |
| `prim::clock_now` | 2 | `is_multiple_of(freq)` **and** `1e9 / freq` — divides twice to answer once |
| `random::Random::reseed` | 2 | `clock_now` inlined into it |
| `stats::process_info` | 2 | the ms conversions, already known to be at the floor |

`prim::alloc`'s two are the find. They are the **same** `is_multiple_of`-on-a-
runtime-alignment shape that D7, D12 and D19 removed from eight sites — and
they sit on the OS reservation path, which every segment goes through. They
survived five rounds purely because they are behind `#[cfg(windows)]`.

`bins::is_aligned_to` applies unchanged, and its conservative direction is the
safe one here: a `false` falls through to the aligned-reservation retry rather
than accepting a block.

`clock_now`'s pair is D17's own doing. `is_multiple_of` **is** a modulo, so
`if 1e9.is_multiple_of(freq) { 1e9 / freq }` divides twice to learn one thing.
One division and a multiply-back answers it: `q * freq` cannot overflow, since
`q` is `1e9 / freq` and the product is at most 1e9, and a `freq` above 1e9
gives `q == 0`, which fails the test and correctly selects the u128 fallback.

**Result: 10 -> 3 integer divisions in the Windows build.** `prim::alloc` goes
**4 -> 0**. The three that remain are `clock_now`'s single once-per-process
`1e9 / freq` (the frequency is only known at runtime), the same one inlined
into `reseed`, and `process_info`'s display conversion. `rusty_alloc-ffi`'s
Windows build was scanned too: **0**.

Linux is untouched — `windows.rs` does not compile there — and measured so:
cfrac allocator **3,445,578,308**, `free` 21.000 Ir/call, zero divisions of
every class, all seven CI-equivalent gates green on both hosts.

### Corrections

**Round four's "zero `divsd`/`divss` in the entire `rabench` binary" is false.
There are 9.** The measurement that produced it ran

```
B=$HOME/ra_target/release/rabench
objdump ... $B | grep -cE '...div[sp][sd]...'
```

through `wsl.exe bash -lc` with the variable stripped — a recurring failure in
this session — so `objdump` read nothing and `grep -c` counted an empty stream
as zero. The D20 work was real and did reduce them, but the endpoint was never
measured. What is actually there, re-counted per symbol: 2 each in
`malloc_small`, `larson`, `xmalloc` and `freepath_probe`, 1 in `tls_spike` —
which is **one division per quantity computed**, plus the `/ 1e9` inside
`Duration::as_secs_f64`, which is std's. That is the floor for computing a
rate, so nothing is owed here except the corrected number.

The lesson generalises past this line: **a count of zero from a pipeline is
only meaningful if the pipeline is known to have read something.** Every scan
in this document should have printed its input size.

Three ledger rows were stale rather than wrong:

- `bins.rs:124` `EXTEND_BATCH` — the table and its two tests were **deleted**
  in v1.1.0, once restricting it to `pub(crate)` exposed it as dead. D13's
  shift form had superseded it entirely. The row, and the `bins.rs:284,299`
  test-oracle row that referenced it, no longer describe anything.
- The `ra_bench/kernels.rs` row says "Declined, with reason" — round three's
  verdict, which **round four reversed** by implementing it (D20). The ledger
  was never updated, so the document contradicted itself.
- The `ra_wasm × 1 (f64) | reporting` row is a mischaracterisation. That crate
  has no float arithmetic at all. Its two division sites are `BIG / 2` (a
  power of two, a shift) and `(i * 37 + round) % 3000` in a self-test loop — a
  constant modulo, so a magic multiply, in a crate marked `publish = false`.
- The test count moved 88 -> 86 with the two `EXTEND_BATCH` tests.

### D22 — and the entry D6 needed

`page_align_up` is no longer the mask this document describes. **D6 broke a
Kani proof**, `good_size_never_shrinks_a_request`, and shipped that way in
v1.1.0; v1.1.1 fixed it. The full account is in the release history, but the
short version belongs here because D6's entry above is otherwise misleading:

the mask was `ps - 1`, guarded by `debug_assert!(ps.is_power_of_two())`. That
assertion is true on every real platform and **not provable** — `ps` is read
from an atomic cache of an OS value, so Kani models it as arbitrary, where
`ps - 1` underflows at zero and the mask is not a round-up at all. Building it
from `1 << (ps.trailing_zeros() & 63)` makes the property structural instead of
asserted and the function total. All five harnesses verify, and the solver time
collapsed — a symbolic division is a hard SAT instance, a shift is not.

That defect reached a published artifact, and the reason it did is worth
recording next to the technical fix: the `proofs` job ran only on a weekly cron
that **had never fired**, so the harnesses had never gated any release. `ci.yml`
now runs them on `v*` tags.

### Where the inventory actually stands

| artifact | integer div | float div | soft-div libcalls |
| -------- | ----------: | --------: | ----------------: |
| `librusty_alloc_override.so` (Linux) | **0** | **0** | **0** |
| `rusty_alloc` (Windows target) | **3** | 0 | 0 |
| `rusty_alloc-ffi` (Windows target) | **0** | 0 | 0 |
| `rabench` (Linux) | 0 in bench symbols | 9 | 0 |

Every remaining entry is one division per quantity that must be computed, on a
path that runs once per process or once per printed line. The magic multiplies
left in allocator symbols are `process_info`'s two display conversions.
