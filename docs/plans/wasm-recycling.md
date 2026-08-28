# wasm-recycling — the leak both layers missed, and adopt-on-free

**Status: FIXED, 2026-08-28.** Reported from the first real wasm deployment
(FFAI/Carmenta, ~750 MB of linear memory growth per document read). The
reporter's analysis named the exact mechanism and it verified line for line.

## The mechanism (v1.1.4)

Three facts, individually reasonable, that only combine into a leak on wasm:

1. **`prim/wasm.rs::free` is a no-op** — correct and unavoidable; linear
   memory cannot shrink. Its doc said the range "is returned to our
   segment/arena caches" and called the segment cache "load-bearing on wasm".
2. **There is no segment cache.** Upstream mimalloc v2 deleted it when arenas
   replaced it (the option survives only as `deprecated_segment_cache`), and
   this remake faithfully copied that. `segment_free` tries
   `arena::chunk_free`, `huge_free` tries `arena::chunk_free_n`, and both fall
   through to `os::free`.
3. **Arenas were off on wasm** — `DEFAULT_ARENA_PAYS` gated them out, on a
   measurement (1056.06 → 64.00 MiB, 6.79 → 1.75 ms) that was real but ran a
   workload too short to reach the regime where the free list matters. The
   justifying comment — "every segment we have ever touched is already
   permanently cached, which is exactly what the arena was for" — was
   self-refuting: the arena was not for keeping pages resident, it IS the
   free list.

So every freed segment reached `os::free` → `prim::free` → `Ok(())`, and the
32 MiB mapping became permanently unreachable. The reporter's isolated proof:
20 alloc/free cycles of one 20 MiB `Vec` grew linear memory by **exactly
640 MiB = 20 × SEGMENT_SIZE**, and 4 MiB cycles by zero — the threshold is
`LARGE_OBJ_SIZE_MAX` = 16 MiB, above which `heap.rs` routes to `huge_alloc`'s
dedicated whole-segment reservations.

## The fix — three pieces

1. **`prim::FREE_RETURNS_MEMORY`** (new `const`, `prim/mod.rs`): true
   everywhere except wasm. Deliberately NOT `has_partial_free`, which is false
   on Windows too — "can free part of a mapping" and "free returns memory at
   all" are different properties, and conflating them would have put the
   recycling path on Windows.
2. **Adopt-on-free** (`arena::adopt_os_block`, called from `os::free` only
   where the const is false, folded away elsewhere): a freed block that the
   chunk maps don't know becomes arena chunks at the moment of release — all
   free, pre-marked dirty, exactly `manage_os_memory_ex`'s discipline. The
   arena grows to the workload's peak footprint and never beyond it; no
   up-front reserve, so the 1056 MiB startup cost that motivated the cfg
   never returns. **Coalescing:** consecutive `memory.grow` blocks are
   address-contiguous, so a freed block that lands at an adopted arena's end
   EXTENDS it in place (`size`/`chunks` are now atomics, dirty bits published
   before the counts) — the grow/free pattern collapses into one arena
   instead of consuming one of the 32 `MAX_ARENAS` slots per 32 MiB forever.
3. **Chunk-granular sizing** (`os::alloc_aligned`): where free cannot return
   memory, any segment-aligned reservation is rounded up to whole
   `SEGMENT_SIZE` chunks. Without this a ragged huge block (say 33 MiB)
   would adopt its 32 MiB interior and strand the tail — up to a chunk per
   cycle, an unbounded leak in its own right. The overshoot is committed
   linear memory, bounded by one chunk per LIVE huge block, fully recycled.

Also fixed while verified here: `ensure_default_arena` was one-shot behind a
`TRIED: AtomicBool` (an atomic RMW on every `chunk_alloc`, and no second
reserve ever). It is now miss-driven — `reserve_default_arena_on_miss` runs
only after a full table scan fails, reserves another default-sized arena
(as upstream does), latches off on failure, and the fast path pays nothing.

## Evidence

New steady-state arm in the wasm selftest (`rusty_alloc_wasm::selftest`,
codes 11/12): N identical alloc/free cycles at 4 MiB (medium page), 20 MiB
(the huge route — the reporter's exact shape), and 33 MiB (ragged huge, pins
the rounding), asserting linear memory does not grow after each shape's first
cycle. Run under Node by `bench/wasm-selftest.mjs`, in CI's `wasm` job.

| build | result |
|---|---|
| v1.1.4 allocator + this arm | **FAILED, code 12** (grew during cycles) |
| fixed allocator + this arm | **PASSED**, 2.06 → 192.06 MiB, flat through 52 cycles |

The control run is the point: the arm catches the exact leak, then the fix
passes it. The measurement that justified the original cfg captured only the
eager-reserve startup cost; this arm is the one that would have caught the
leak, and it now gates every CI run.

Native (nothing may move, and nothing did): clippy 0 both platforms, 32
suites / 89 tests (3 new adoption tests) / 0 panics, Kani 5/5, datasweep
573,640 checks × 6 arms, corpus sweep passed, cfrac `free` 21.000 Ir/call,
allocator 3,445,578,348 (+40 on 3.4e9 vs pre-change, cold-path shape),
all 13 opscan ops unchanged (`huge` 647.00 predates this work).

## Residual, known and accepted

- Sub-chunk OS blocks (heap-descriptor pages freed by `init.rs`) still reach
  the no-op `free` on wasm and are lost — bounded by heap create/destroy
  cycles, which a single-threaded wasm module rarely performs. Recorded, not
  fixed.
- Adoption assumes the no-free platform is single-threaded (true for
  `wasm32-unknown-unknown` without the threads proposal, and already a
  standing assumption of `prim/wasm.rs`). The atomics are ordered correctly
  anyway; a future threaded-wasm backend must revisit the lock-free scan vs
  in-place extension pairing.
- `ffai-wasm` pinned rusty_alloc as its global allocator before this fix; its
  A/B measured peak, not a growth curve. It needs the fixed release, and its
  own steady-state arm if it keeps an allocator A/B.

## Interim workaround (pre-fix builds only)

One `rusty_alloc::arena::reserve_os_memory_ex(n << 20, true, false, false)`
at init: costs n MiB of linear memory up front, then flat — the reporter
measured 20 × 20 MiB cycles at +0.0 MiB with a 256 MiB reserve. Unnecessary
from the fixed release onward.
