# small-metal — rusty_alloc on a microcontroller, and whether it should be

**Source:** the Janus device programme (`coding/janus`), which ships firmware
for Espressif chips and today declares `esp-alloc` on bare metal and the
ESP-IDF heap on the ESP-IDF track. Janus's own plan carries this as an
unfiled decision item: *"`rusty_alloc` on Xtensa / bare-metal RISC-V —
firmware binaries switch allocators; no library changes."* Written
2026-09-06 against `main`.

**Status: P0–P4 EXECUTED 2026-09-07 (+P4b/P4c/P4d, §2.9–§2.15: 192 KiB -> 68 KiB, 2.1-3.7x faster than esp-alloc, two reclamation defects fixed, and the six production blockers closed). It is a PORT, not a second allocator; it
BUILDS for a chip; and it RUNS on one** — the kill test passes on a Seeed XIAO
ESP32-S3 Sense with `rusty_alloc` as the global allocator. The cost is now
measured rather than suspected, and it is not flattering: **2x the region and
+9.7 % of app flash** for a workload whose true demand is 4.9 KB (§2.8). The plan stands, with the corrections folded in
below. The plan is arranged so the cheapest disqualifying answer comes first,
because this may end in a documented "no" and that is a fine outcome.

| phase | state | riscv debt after |
|---|---|---:|
| **P0** — does it compile at all | ✅ done | 55 |
| **P1** — the memory seam | ✅ done (2 of 3 kill-test items; the third was P2's) | **39** |
| **P2** — the geometry | ✅ done — **2 compile errors**, both hardcoded shifts | — |
| **P3** — atomics and threads | ✅ done — 4 atomics narrowed, 2 shimmed | **0** |
| **P4** — on a chip | ✅ **PASSED on a XIAO ESP32-S3** — and priced (§2.8) | — |
| **P5** — the ESP-IDF track | ← next; needs the IDF track, not more allocator work | — |

Numbers, methods and the bucket tables are in
[`docs/LEDGER.md`](../../LEDGER.md). Two headlines worth carrying: the first
riscv build reports **224 errors** of which **183 are one root** (no
`#![no_std]` ⇒ no prelude), so the real debt was **55**; and P1 closed the
whole of bucket A (16 → 0) while every other bucket moved **+0** and the
shipped artifact stayed byte-for-byte identical on every deterministic
quantity. **P2's headline:** the geometry was already symbolic everywhere that
mattered — changing `SEGMENT_SLICE_SIZE` and `SLICES_PER_SEGMENT` to a 64 KiB
segment produced **two compile errors**, both hardcoded shifts, and
`segment.rs` / `heap.rs` / `page.rs` / `alloc.rs` / `arena.rs` / `bins.rs`
compiled unchanged. **P3's headline:** 39 riscv errors to **0** — the crate
builds for `riscv32imac-unknown-none-elf`, `riscv32imafc-unknown-none-elf` and
`xtensa-esp32s3-none-elf` in both geometries, with the host battery unchanged
(105 tests). Both P2 and P3 found real defects in the SHIPPED allocator along
the way — see §2.7. **P4's headline:** the sketch runs on silicon under this
allocator, and the price is §2.8.

---

## 0. Why anyone wants this

The house rule is that every deliverable declares `rusty_alloc` as its global
allocator, through a one-crate seam, never from a library. A firmware **is**
a deliverable. So the rule already points here, and the only reason the
portfolio's devices do not follow it is that nobody has tried.

The reason to want it is the safety posture, not speed. On this allocator a
double free aborts instead of putting a block on a free list twice and
handing identical memory to two owners. A device that parses bytes off a
radio, a UART and a flash partition is exactly where that matters, and it is
the one class of bug the rest of the Janus doctrine cannot design away.

The reason to be sceptical is that `esp-alloc` is small, simple, and
already correct for what it does, and this allocator's architecture was
built for machines with virtual memory. That tension is the whole plan.

---

## 1. What is already true (verified against source)

**~~The core is `no_std`.~~ CORRECTED BY P0 — it is not.**
`crates/rusty_alloc/src/lib.rs` carries **no `#![no_std]`**, and its own
module doc says so outright: *"A no_std profile returns post-v1 with the
nightly `#[thread_local]` or a platform TLS shim."* The
`#![cfg_attr(not(test), no_std)]` this plan cited is in
`crates/rusty_alloc_api/src/lib.rs:14` — the thin **API surface**, not the
core that holds the code. `rusty_alloc_ffi/src/lib.rs:9` asserted *"the core
crate is no_std"* in a comment, and that comment was false.

So the starting position was a **std-first allocator**, and this section
claimed the opposite. What was true, and is worth more than the claim it
replaces: **the core crate has zero dependencies**, so nothing external could
block the port and every error was ours to fix.

> **P3 (2026-09-07): the claim is TRUE NOW, and the comment has been
> corrected to say what is actually the case.** `rusty_alloc` has a default-on
> `std` feature; `--no-default-features` is `#![no_std]` and selects the
> single-heap profile. The zero-dependency property survives on every target
> the crate ships to today — the one dependency P3 added
> (`portable-atomic`) is gated to targets without a 64-bit atomic, which is
> none of them.

**There is precedent for a target without an OS.** `rusty_alloc-wasm` and
the arena's wasm path already deal with a world where memory only grows and
there is no `munmap`. A chip is a harsher version of the same shape, and the
wasm work is the closest thing to a map.

**The repository has the discipline this needs.** `docs/LEDGER.md`, a pinned
toolchain, `deny.toml`, a fuzz corpus and an oracle directory. Nothing below
proposes loosening any of it.

---

## 2. The walls, with evidence

Four when this was written; **six after P0**, and the two it added (§2.5,
§2.6) are the cheap ones. Each block quoted `> **P0:**` is what the compiler
said on 2026-09-07 — see [`docs/LEDGER.md`](../../LEDGER.md) for the method.

These are ordered by how likely they are to end the project. Each names the
file that establishes it.

### 2.1 Segment geometry — the wall

`crates/rusty_alloc/src/alloc.rs:115`: *"`free` masks the pointer to a 32 MiB
segment base and reads `slice_offset`"*. `crates/rusty_alloc/src/arena.rs:18`:
`MAX_CHUNKS = 1024 // 32 GiB per arena at 32 MiB chunks`.

The free fast path is O(1) because it can find a block's metadata by masking
its address down to a 32 MiB-aligned segment header. That trick is the
architecture, and it requires a 32 MiB aligned reservation to exist.

An ESP32-S3 has 512 KiB of internal SRAM. The XIAO board Janus runs adds
8 MiB of external PSRAM. An ESP32-C6 has no PSRAM at all. **A single segment
is larger than the entire address space we can allocate from**, by a factor
of 64 on internal memory.

So this is not a port. Either `SEGMENT_SIZE` becomes a compile-time
parameter that the mask, the slice count and the size classes all follow, or
a small-memory profile replaces the segment scheme with something else. The
first is a large but bounded change; the second is a second allocator
wearing the same name, which is worse.

> **P0: zero errors, and P0 cannot ever produce one.** This is a *space*
> property, not a *type* property, so the compiler has no opinion on it. The
> wall this plan ranks first is **structurally invisible to its own first
> phase** — the same shape as `opscan` being blind to the park/unpark thrash
> (`lets_win.md` §5.1.2): the instrument never enters the regime. Sizing it
> needs P2 or a link map, not a check.
>
> **The number is worse than the datasheet argument above.** Janus's
> firmwares declare their heaps explicitly, and the range is **64–220 KiB**
> (`xiao-s3-keys` 64, `c6-lora-p2p` 72, `blink-fs` and `c6-mesh-node` and
> `c6-ble-provision` 96, `xiao-s3-probe` 220). P4's own target, `blink-fs`,
> runs on **96 KiB** — a 32 MiB segment is **350×** that, not 64×.
> Corroborated from the chip rather than the source: `esp_alloc::HEAP` on
> the XIAO S3 reports 225,280 total / 172,800 used, unchanged across eight
> kernels (dsp ledger, 2026-09-06).
>
> **P2: RESOLVED, and it cost two lines.** The paragraph above says "this is
> not a port" and offers a large-but-bounded change or a second allocator. It
> was neither: the geometry was already symbolic everywhere that mattered.
> Changing `SEGMENT_SLICE_SIZE` to 8 KiB and `SLICES_PER_SEGMENT` to 8 — a
> **64 KiB segment**, 512x smaller — produced **two compile errors**, both
> hardcoded shifts (`segment_map`'s `WINDOW_SHIFT = 25`, `slice_pool`'s
> `SLICE_SHIFT = 16`), each with a const assert pinning it. `segment.rs`,
> `heap.rs`, `page.rs`, `alloc.rs`, `arena.rs` and `bins.rs` compiled
> unchanged. Both are derived now, and the small profile passes its full
> battery (19 suites, 84 tests).
>
> **What it costs, measured:** the alignment ceiling is `SEGMENT_SIZE/2`
> (alloc.rs:451, heap.rs:986) — 16 MiB shipped, **32 KiB** small — because a
> segment cannot promise an alignment it cannot hold; and `good_size` leaves
> the mimalloc oracle above `MEDIUM_OBJ_SIZE_MAX`, which moves with the
> geometry. Both are inherent, neither is a bug.

### 2.2 Sixty-four-bit atomics — a hard, cheap blocker

`crates/rusty_alloc/src/*.rs` constructs `AtomicU64` in four places and
imports it in five. Neither 32-bit RISC-V (ESP32-C3, C6) nor Xtensa
(ESP32-S3) has a 64-bit atomic. This is not theoretical: the Janus facade hit
exactly this on 2026-09-06 and the compiler's message was
`no AtomicU64 in sync::atomic`, on a C6 firmware.

Two answers: `portable-atomic`, which supplies the type on targets that lack
it, or 32-bit counters where the width was never load-bearing. Janus took the
second for its own counters and said so in the code. Which is right here
depends on whether any of those five are on a correctness path rather than a
statistics path, and that is a reading job, not a design job.

> **P0: confirmed — 5 errors, one per file, and the reading job is done.**
>
> | site | what it is | correctness? |
> |---|---|---|
> | `arena.rs:51-52` `used` / `dirty` | the CAS'd chunk bitmaps | **yes — the only one** |
> | `options.rs:195` `HEARTBEAT` | heartbeat counter | no |
> | `random.rs:61` `COUNTER` | seed counter | no |
> | `segment_map.rs:30` `MAP` | the window bitmap — see §2.5 | static, see below |
> | `slice_pool.rs:38` `FREE` | the wasm free-slice bitmap | static, wasm-only |
>
> The one correctness site is a **bitmap**, whose word width is a free
> choice. So `portable-atomic` may be needed **nowhere** — narrowing covers
> every site, which is the call Janus already made (espino ledger,
> 2026-09-06: *"Counters are `AtomicU32` now, `Stats` still reports `u64`"*).
> Note the cause recorded beside it there — *"The facade had never been
> compiled for the C6 or the C3"* — which is this crate's position exactly.
>
> **P3: RESOLVED, and "nowhere" was ALMOST right — four of six.** The rule
> that decided every site: **narrow a width that is a CHOICE, shim one that is
> a CONTRACT.**
>
> | site | decision | why |
> |---|---|---|
> | `arena.rs` `used`/`dirty` | narrow `u64`→`u32` | a bitmap's word width is free; stays lock-free |
> | `segment_map.rs` `MAP` | narrow | bitmap |
> | `slice_pool.rs` `FREE` | narrow | bitmap |
> | `random.rs` `COUNTER` | narrow to `usize` | a seed counter has no width contract |
> | `options.rs` `VALUES` | **`portable-atomic`** | `get`/`set` are `i64` in an API frozen at v2.0.0 |
> | `options.rs` `HEARTBEAT` | **`portable-atomic`** | `DeferredFreeFun`'s C ABI declares it `u64` |
>
> The shim is `[target.'cfg(not(target_has_atomic = "64"))'.dependencies]`, so
> **the crate stays dependency-free on every target it currently ships to.**
>
> **And narrowing a bitmap is not one edit.** After the element type moved,
> four loop bounds still said `div_ceil(64)`, and the whole 105-test battery
> passed — because every arena any test builds is ≤32 chunks, where
> `div_ceil(64)` and `div_ceil(32)` are both 1. See §2.7.

### 2.3 Thread-local storage — needs a seam, not a fix

`crates/rusty_alloc/src/init.rs` gives every thread a lazily-created heap
reached through a const-init `thread_local!` cell, and discusses choosing the
initial-exec TLS model over general-dynamic. A bare-metal firmware has no
`std::thread_local!` and, on the executors Janus runs, one thread that
matters.

The honest shape is a profile with exactly one heap and no TLS lookup at all,
selected at compile time. That is a simplification rather than a port, and it
should make the fast path shorter, not longer.

> **P0: confirmed — 10 errors**, and they are wider than TLS alone. Explicit
> `std::` paths in `init.rs` (5), `options.rs` (3), `page.rs` (1) and
> `random.rs` (1) — `thread_local!`, `process::abort`, and the io used by the
> option layer. A further **11 errors cascade from this and §2.4** (`init.rs`
> 10, `random.rs` 1: `HEAP_PTR`, `TID`, `SUBPROC`, `BACKING_PTR`,
> `os_entropy`), so they cost nothing extra to fix and should not be counted
> as separate work.
>
> **P3: RESOLVED — and the paragraph above was right that this is a
> SIMPLIFICATION.** `ra_thread_local!` is `std::thread_local!` verbatim with
> `std` (so the shipped build keeps M10c's initial-exec fast path untouched)
> and a plain `static` without it: one heap, **no TLS lookup at all**, a
> shorter fast path than the threaded one. Sound because the crate serves
> `no_std` only on single-threaded targets — the same assumption `prim::fixed`
> already makes — and the one new `unsafe impl Sync` names what to revisit
> first if that ever changes. `abort()` panics without `std`, so a `no_std`
> consumer **must** build with `panic = "abort"`; every Janus firmware profile
> already does.

### 2.4 Where memory comes from — a seam that does not exist yet

`crates/rusty_alloc/src/arena.rs` speaks in OS pages and reserves through the
platform. A chip has no OS: it has a region the linker gave it, and on the
S3 a second region behind a cache that must be initialised first.

The arena needs a primitive-memory seam it can be handed a fixed
`&'static mut [u8]` through, with no growth and no unmapping. The wasm path
is the nearest existing case and should be read first.

> **P0: confirmed, and it is ONE cause, not sixteen.** `prim/mod.rs` picks
> its backend with four arms — `windows`, `unix`, `target_arch = "wasm32"`,
> `miri`. A bare-metal RISC-V target matches **none**, so no `sys` module is
> named at all and all sixteen call sites through it fail together. Adding a
> fifth arm is the whole of P1's compile half. The seam size is set by §2.1's
> real number: a fixed region of **64–220 KiB**, not 512 KiB.

### 2.5 The diagnostics layer — a fifth wall, and the cheapest one (added by P0)

**13 errors**, in `options.rs` (7), `stats.rs` (3), `arena.rs` (2) and
`init.rs` (1), and none of them is an allocator problem:

- `options.rs::ensure_init` reads `RUSTY_ALLOC_*` / `MIMALLOC_*` through
  `std::env::var`, with `to_uppercase`, `to_ascii_lowercase` and `format!`;
- `arena.rs:605` returns a `String` debug dump of arena state;
- `stats.rs::process_info` reports RSS, commit and page faults.

**A firmware has no environment, no process and no stdout.** So this layer is
not ported, it is `cfg`-ed out, and the option table falls back to its
compiled `DEFAULTS`. That makes D the cheapest bucket in P0 despite having
the second-largest error count — and it means the port needs **no `alloc`
dependency**, which a naive reading of "13 errors wanting `String` and
`format!`" would have concluded.

> **P3: RESOLVED, and one seam was kept alive on purpose.** The environment
> pass does not exist without `std` rather than existing and returning nothing,
> and the `format!`-based printers are gated. **But `options::out_fmt` takes
> `&str` and needs no allocation, so it survives**: a firmware that registers
> an output hook still gets the allocator's messages over its serial log, and
> `options::error` still delivers the error CODE to a registered hook without a
> formatter. Deleting the layer wholesale would have thrown that away. A
> `no_std` consumer wanting the stats dump formats into a stack buffer and
> calls `out_fmt`.

### 2.6 Two static arrays sized for a 48-bit machine (added by P0)

Found while reading §2.2, and both are space rather than type, so like §2.1
the compiler will not raise them:

- `segment_map::MAP` is `[AtomicU64; 131072]` — **1 MiB of BSS on every
  non-wasm target**. Against a 64–220 KiB heap and the S3's 512 KiB of
  internal SRAM, this static alone is twice the entire address space the
  allocator would serve from.
- `slice_pool::FREE` adds **8 KiB with no `cfg` at all**, on every target,
  though it is only used on wasm.

The good news is that the precedent §1 claims for the arena is stronger here:
**wasm already replaces `MAP` wholesale** with a 256 KiB slice-granular base
table (`segment_map::base_of`, F2 of `segment-tax.md`). A third
representation for a small-address-space target is a shape the file already
has, not a new idea.

---

### 2.7 What the small profile FOUND in the shipped allocator (added by P2)

Not a wall — the opposite. Shrinking the segment moves the huge-path boundary
from 32 MiB down to 56 KiB, which made an ordinary 100 KB allocation take a
path that is nearly unreachable at the shipped geometry. It escaped its arena.

`segment::huge_alloc` asked `arena::chunk_alloc_n(-1, chunks)` — a hardcoded
"any non-exclusive arena" — while `segment_alloc` twenty lines away correctly
passed the owning heap's `arena_id`. So a heap created with
`create_heap(_, _, arena_id)`, whose entire purpose is that its memory comes
from ONE region, served **every** allocation above `LARGE_OBJ_SIZE_MAX` from
the default arena or straight from the OS. Upstream does not
(`mi_segment_huge_page_alloc` takes a `req_arena_id`; oracle
`segment.c:1671,1683`).

**This is reachable in the 32 MiB build** by any consumer of the
exclusive-arena API making one allocation past 32 MiB − 64 KiB. Fixed, with the
regression test written at the DEFAULT geometry
(`tests/heaps.rs::exclusive_arena_confines_huge_allocations`) and poisoned back
to the old behaviour to prove it fires.

A second, smaller one alongside it: the huge path bumped `huge_allocs` but not
`stats.segments`, while the release path bumps `segments_freed` beside
`huge_free`. The pair could report more segments freed than allocated — from
the counters this project uses as its work-parity instrument.

**P3 added a second, of the same species.** Narrowing the arena bitmap's word
type from `u64` to `u32` left four loop bounds saying `div_ceil(64)` — a
half-narrowed bitmap, which cannot reach chunks past its first word. **The
whole 105-test battery passed**, because every arena any test builds is 32
chunks or fewer and at ≤32 chunks `div_ceil(64)` and `div_ceil(32)` are both 1.
The suite was not weak; it was *unable to express* the defect.
`tests/heaps.rs::arena_bitmap_reaches_past_its_first_word` closes that, and
under the small profile it costs 2.1 MiB instead of the default geometry's
1.06 GiB — the reachability point again, from the other side.

**The transferable point:** a geometry parameter is not only a portability
lever, it is a *reachability* lever. Shrinking the segment moved three code
paths from "needs a 32 MiB allocation to reach" to "reached by a 100 KB one",
and the first thing down there was a real defect. That is the same shape as
`lets_win.md` §5.1.2 — an instrument that never enters the regime cannot see
what lives in it.

### 2.8 What it costs on silicon (added by P4, measured on a XIAO ESP32-S3)

The kill test passes. This is the price, from the board — the same workload
under both allocators, one binary source, the peak measured by a `GlobalAlloc`
wrapper present in BOTH arms:

| | esp-alloc | rusty_alloc |
|---|---:|---:|
| **workload PEAK live bytes** | **4,914** | **4,914** |
| region given | 96 KiB | 192 KiB |
| region consumed | 0 at every stage | 135,168 (132 KiB) |
| smallest region that runs | — | **192 KiB** (128 KiB panics) |
| app image | 116,032 B | 127,328 B (**+9.7 %**) |

PEAK identical to the byte is the work-parity check: the allocator is the only
variable, and both are provably reached.

**Where 132 KiB goes, and the cheapest fix P4 found.** It is `4,096 + 2 x
65,536` — an arena descriptor plus two segments — and the 60 KiB reported free
is **stranded by alignment**: `prim::fixed` is first-fit, so the 4 KiB
page-aligned descriptor takes the bottom of the region and pushes the first
`SEGMENT_SIZE`-aligned segment to 64 KiB and the second to 128 KiB. Predicted,
then confirmed by dropping the region to 128 KiB and watching the board panic in
`handle_alloc_error`. **Placing sub-segment allocations at the top of the region
(or best-fit) would make 132 KiB sufficient** — a `prim::fixed` change, not an
architectural one, and a third of the region back.

**Read this against §0.** The reason to want this was never speed; §0 says so.
P4 establishes that it is not free either: 2x the region on a part with 512 KiB
of SRAM, for a workload needing 4.9 KB. See §5.

### 2.9 WHY it costs that — the decomposition, measured (added by P4b)

§2.8 said *what* the 192 KiB is. It did not say what drives it, and the
difference decides whether this is an optimisation backlog or an architectural
floor. So the region was decomposed on the board rather than argued about: arm B
now also prints rusty_alloc's own always-on counters and a per-bin census of
every request the workload makes.

```
[pages] end: generic 25 pages_fresh 10 extends 11 segments 2 large 0 huge 0
[bin]  1: block    4 B, reqs  2      [bin] 14: block   96 B, reqs  2
[bin]  2: block    8 B, reqs  6      [bin] 21: block  320 B, reqs  1
[bin]  4: block   16 B, reqs 12      [bin] 22: block  384 B, reqs 12
[bin]  6: block   24 B, reqs  6      [bin] 36: block 4096 B, reqs  7
[bin]  8: block   32 B, reqs  2
[bin] 12: block   64 B, reqs  1      [bin] distinct bins touched: 10
```

**Ten distinct bins, ten fresh pages.** Not a correlation — an identity. A page
serves exactly one size class, and a page is at minimum a whole SLICE. So the
floor is *(distinct bins touched) x slice size*, and it is independent of how
many bytes the workload actually wants.

The slice budget closes the segment count exactly:

| | slices | bytes |
|---|---:|---:|
| 9 small pages (bins 1..22, all blocks ≤ `SMALL_OBJ_SIZE_MAX` = 1 KiB) | 9 x 1 | 73,728 |
| 1 medium page (bin 36, block 4,096 = `MEDIUM_OBJ_SIZE_MAX` exactly) | 1 x 4 | 32,768 |
| **pages needed** | **13** | **106,496** |
| usable per segment (`SLICES_PER_SEGMENT` 8 − `HEADER_SLICES` 1) | 7 | |
| **segments** | | **2** (14 usable slices — one to spare) |

And the region closes to the byte:

| | bytes |
|---|---:|
| heap+tld block (`create_heap`'s one `os::alloc_aligned` page) | 4,096 |
| **alignment hole** (first-fit skip to the next 64 KiB boundary) | **61,440** |
| segment 1 | 65,536 |
| segment 2 | 65,536 |
| **= region required** | **196,608 (192 KiB)** |

**Occupancy: 4,914 live bytes in 106,496 bytes of pages — 4.6 %.**

**The levers, ranked by measured contribution.**

1. **The alignment hole — 61,440 B, 31 % of the region, no design change.**
   Already named in §2.8; the decomposition confirms it is the single largest
   line and the only one that is a defect rather than a consequence. Fix
   `prim::fixed` to return the skipped prefix to the free list instead of
   consuming it (or place aligned requests from the tail) and the region
   requirement goes 192 KiB → 132 KiB. This is arithmetic, not a projection.

2. **`SEGMENT_SLICE_SIZE` — the multiplier on all 106,496 bytes.** Every number
   in the slice table is linear in it; `SEGMENT_SIZE` is not the lever, the
   slice is. **Done — see §2.10.**

   > **Correction (§2.10).** This entry first said the slice was coupled to
   > `SMALL_WSIZE_MAX` and that the two had to move together. They do not.
   > `SMALL_SIZE_MAX` gates the `direct[]` table, which is only a cache of "the
   > page currently serving this word size" and is indifferent to what KIND of
   > page that is; `SMALL_OBJ_SIZE_MAX` separately decides page size at
   > `heap.rs`'s `fresh_page`. The slice moved without `SMALL_WSIZE_MAX`
   > moving at all. The real constraint was somewhere else entirely, and §2.10
   > is where it turned up.

3. **Retention — the peak IS the floor.** At `end`, live is 0 and region used is
   still 135,168: everything was freed and nothing was returned. That is
   mimalloc's design (a retired page is a reuse cache), and on a part where
   nothing else can claim the SRAM it converts a transient peak into a permanent
   cost. esp-alloc's free list coalesces back to one block.

4. **Code size — +11,296 B of flash (+9.7 %).** Real, but flash is not the
   scarce resource here.

**What is NOT on this list, and why that is the answer.** esp-alloc is a
linked-list heap: its floor is *bytes live + per-allocation headers*, about
5.4 KiB here. rusty_alloc is a size-class page allocator: its floor is *bins
touched x slice size*, 106 KiB here, **whatever the workload's byte demand**.
Those are different functions, not the same function tuned differently. Lever 1
is a genuine 31 % win and lever 2 a real geometric one, but closing a 20x gap is
not what they do — after both, the residue is the architecture, and the
architecture is buying page-local O(1) free lists and lock-free cross-thread
frees that a 4.9 KB single-threaded workload never spends.

The corollary is the useful one: **rusty_alloc's page cost is roughly FIXED for
a given bin profile.** The same 13 slices serve 5 KB or 500 KB. The crossover is
where live bytes approach `bins x slice`; below it esp-alloc wins by
construction, above it the page allocator starts earning its keep. §0's reason
for wanting this on metal has to survive that sentence, or it does not survive.

### 2.10 Hammering the levers — 192 KiB to 68 KiB (added by P4b)

§2.9 ranked the levers. This is what happened when they were taken, each one
measured on the same XIAO ESP32-S3 with the same kill test.

| | region required | on the board |
|---|---:|---|
| P4, as measured | 192 KiB | 128 KiB panics in `handle_alloc_error` |
| + lever 1, two-ended placement | **132 KiB** | passes, `free 0` at peak |
| + lever 2, 4 KiB slice | **68 KiB** | passes, `free 0` at peak |

**−64.6 %.** `PEAK live bytes` is 4,914 at every step — the work-parity check
holds, so the allocator is still the only variable. The app image moved +48
bytes across both changes (127,328 -> 127,376), which is to say the footprint
came out of geometry, not out of code that was deleted.

**Lever 1 — two-ended placement in `prim::fixed`.** The 61,440 bytes were never
leaked; they sat on the free list, unusable because no `SEGMENT_SIZE`-aligned
request could start there. That makes it a PLACEMENT bug, not a leak. The rule
is now: a request that needs coarse alignment takes the bottom of the lowest
extent that fits, and a merely page-aligned one takes the TOP of the highest —
because requests that do not care about coarse alignment are the ones that can
move, so they are the ones that move. Shipped-code cost: one pure arithmetic
`fn place`, zero new unsafe.

The test for it is in `prim::fixed`, and it took two goes to make it mean
anything. Written against a 512 KiB region it passed under the bug, because a
region that is an exact multiple of `SEGMENT_SIZE` cannot tell the policies
apart — a page off either end costs a segment either way. `K * SEGMENT_SIZE +
FIXED_PAGE` on an aligned base is the shape that discriminates, and it is the
shape the board actually has. Both halves were then poisoned separately: the
placement half reports offset 0 instead of `N - FIXED_PAGE`, and the reach half
reports 7 segments instead of 8.

**Lever 2 — `SEGMENT_SLICE_SIZE` 8 KiB -> 4 KiB, `SLICES_PER_SEGMENT` 8 -> 16.**
`SEGMENT_SIZE` deliberately does NOT move: what mattered was pages-per-segment,
which went 7 -> 15. The measured effect was exactly the §2.9 arithmetic —
`pages_fresh` 10 -> 9, `segments` 2 -> **1**, `used` 135,168 -> 69,632. The
ninth-to-tenth page disappeared because bin 36 (4,096 B blocks) crossed
`MEDIUM_OBJ_SIZE_MAX` and became a large span, which for a workload holding one
at a time is strictly cheaper than a dedicated page.

**Why 4 KiB is the floor, and how that was established.** A 2 KiB probe built
and passed 35 of 36 unit tests. What it failed was
`properties::usable_size_agrees_with_good_size`: `good_size(49_153)` promised
53,248 bytes while the 25-slice span delivered 51,200. `bins::good_size` answers
the large range with `os::page_align_up`, but the large path allocates EXACT
SLICES — so `usable_size >= good_size`, an ABI-visible promise, holds only while
**a slice is at least an OS page**. Every other geometry gets that free (a
64 KiB slice over a 4 KiB page), which is precisely why nothing wrote it down.
`good_size` is G2-pinned against the oracle, so the slice is the side that
moves. The invariant is now a `const _: () = assert!` in `prim/fixed.rs`, at the
one backend where the two can be tuned into conflict.

**Two more the probes turned up, both from the same defect shape as P2's.**
`slice_pool::rejects_what_it_cannot_track` was the module's last
byte-denominated test: `MIB + 4096` was "misaligned" only while a slice was
8 KiB, and at 4 KiB it became slice-ALIGNED, so the test quietly *succeeded* in
freeing two ranges it exists to refuse — and, the pool being global first-fit,
took down three other tests instead of itself. `heaps.rs` reserved an arena of
`64 * 1024 * 1024`, which reads "two chunks" at 32 MiB segments and "2048
chunks" — past `arena::MAX_CHUNKS` — at 32 KiB ones. Both are now written in the
unit the code actually counts.

And one that was a genuine regression rather than a stale premise: dropping
`MEDIUM_PAGE_SLICES` to 2 alongside the slice lowered `MEDIUM_OBJ_SIZE_MAX` to
1,024 B, collapsing the binned range so that a burst of 2 KiB objects took a
whole slice each instead of sharing a page. `spans.rs` caught it. It is held
at 4.

**Where this leaves the comparison.** 68 KiB is now *below* the 96 KiB the
esp-alloc arm is configured with — but that is the example's chosen number, not
esp-alloc's floor, and the honest comparison in §2.9 is unchanged: esp-alloc's
floor is bytes-live-plus-headers (~5.4 KiB here) and rusty_alloc's is
bins x slice. Levers 1 and 2 took 2.8x out of the gap. They did not, and could
not, close it.

**Not taken.** Lever 3 (retention) does not move the number that sizes the
region — `used` is still the peak, and the peak is what the region must hold —
so it is worth doing only if something else on the part wants the SRAM back.
Lever 4 (flash) stands at +9.8 % over esp-alloc.

**A hypothesis that died cheaply, recorded so it is not re-run.**
`slice_pool::FREE` is a bitmap over the whole 32-bit address space —
`1 << (32 - SLICE_SHIFT)` bits — which at the small profile's slice is 64 KiB of
BSS, bigger than the region. It costs nothing: every call site is
`#[cfg(all(target_arch = "wasm32", not(miri)))]`, so the linker drops the static
on every other target. Confirmed against the symbol table of the shipped
firmware, not argued from the source.

### 2.11 Where the floor actually is (added by P4b, per-bin peak-live census)

§2.10 stopped at 68 KiB. This section establishes that 4 KiB is the right slice
for a measured reason rather than a lucky one, and names the only lever left.

`BINS` counted REQUESTS, which cannot say how much of a page is ever in use. A
page is sized for a whole size class, so the question is how many blocks of that
class are live **at once**. Adding a live/peak pair per bin answers it:

```
[bin]  1: block   4 B, PEAK live 1 =   4 B in a 4096 B page
[bin]  2: block   8 B, PEAK live 2 =  16 B in a 4096 B page
[bin]  4: block  16 B, PEAK live 4 =  64 B in a 4096 B page
[bin]  6: block  24 B, PEAK live 2 =  48 B in a 4096 B page
[bin]  8: block  32 B, PEAK live 1 =  32 B in a 4096 B page
[bin] 12: block  64 B, PEAK live 1 =  64 B in a 4096 B page
[bin] 14: block  96 B, PEAK live 1 =  96 B in a 4096 B page
[bin] 21: block 320 B, PEAK live 1 = 320 B in a 4096 B page
[bin] 22: block 384 B, PEAK live 2 = 768 B in a 4096 B page
```

**Nine pages — 36,864 bytes — holding 1,412 bytes at peak. 3.8 % occupancy.**
Not one class ever holds more than four blocks. The remaining 3,502 bytes of the
4,914-byte peak are the 4 KiB allocations, which are large spans sized to the
block and therefore not part of this waste at all.

**Why 4 KiB is the floor and not merely where §2.10 stopped.** The two largest
small classes are 320 B and 384 B, and `SMALL_OBJ_SIZE_MAX = SEGMENT_SLICE_SIZE / 8`.
At a 4 KiB slice that ceiling is 512 B, so both land in *small* pages of one
slice — 8 KiB for the pair. Halve the slice and the ceiling falls to 256 B, so
both become MEDIUM allocations; and `MEDIUM_PAGE_SIZE` cannot fall with them,
because §2.10 already established (via `spans.rs`) that `MEDIUM_OBJ_SIZE_MAX`
must stay at 2 KiB, which pins a medium page at 16 KiB. Two medium classes
touched would cost **32 KiB** where the pair currently costs 8. Arithmetic from
the measured profile, not a measurement — but the direction is not in doubt, and
it is the same trap in the opposite direction from the one `spans.rs` caught.

So 4 KiB is the largest slice at which this workload's biggest small class still
fits a small page, and the smallest at which `good_size` stays honest (§2.10).
Both walls were found by probing past them.

**The only lever left, and why it was not taken.** Nine classes hold 1,412
bytes. Coarsening the small bins — power-of-two classes instead of mimalloc's
four-per-octave — would collapse those nine pages to three or four, fit the
workload in a 32 KiB segment, and take the region to roughly **36 KiB**. It is
not taken because:

1. `bins.rs` states, at the top of the file, that the size -> `good_size`
   mapping **is** the ABI-visible contract and is G2-pinned against the oracle
   binary. Every existing small-profile divergence (`SEGMENT_SIZE`,
   `LARGE_OBJ_SIZE_MAX`) changes *routing*; none changes that mapping. This
   would be the first, and that is a decision to take deliberately, not a
   footprint optimisation to slip in.
2. It trades a **bounded** cost for an **unbounded** one. Page cost is fixed per
   class touched; internal fragmentation is paid per live object. This workload
   holds 10 tiny objects, so coarsening looks free — on a sample of one. A
   workload with thousands of 24-byte nodes would pay up to 2x on every one.

Recorded here with its number so the choice can be made on evidence.

### 2.12 One dependency removed: `portable-atomic` off the no_std path (P4b)

`options.rs` was the crate's last 64-bit-atomic user on a 32-bit target —
`VALUES` (an `i64` API frozen at v2.0.0) and `HEARTBEAT` (a C-ABI `u64`). Both
pulled `portable_atomic`'s **lock-based** fallback, whose `LOCKS` table measured
4,288 bytes in the shipped firmware. That is atomicity nothing can observe: the
crate already serves `no_std` only on single-threaded targets, which is what
`lib.rs`'s `SingleThreadCell`, `prim::fixed`'s constant thread id, and its
never-contended spin lock all rest on.

Replaced with `split64`, two `AtomicU32` halves. **It adds no unsafe** — a struct
of `AtomicU32` is already `Sync`. With `std` on a 32-bit target the shim stays,
because there threads are real.

| | .text | `.bss` symbols | `.bss` section |
|---|---:|---:|---:|
| `portable-atomic` | 84,907 | 136,423 | 201,996 |
| `split64` | 84,587 | 132,135 | 201,996 |
| | **−320** | **−4,288** | **0** |

**The BSS win is smaller than it looks, and the honest number is zero.** The
4,288 bytes leave the symbol table — `LOCKS` is the *only* differing symbol —
but the `.bss` SECTION does not shrink, because esp-hal's linker script anchors
its end. The space becomes slack the application cannot claim. This was
predicted as a 4,288-byte SRAM saving and the prediction was wrong; the section
table said so. Keep the change for the dependency and the 320 bytes of flash,
not for RAM.

**Two defects in the shim, both caught before it shipped.** Forwarding the
caller's `Ordering` to the halves aborts the firmware: `AtomicU32::load` rejects
`Release`/`AcqRel`, and `options::set_default` performs
`compare_exchange(.., AcqRel, ..)`. Orderings are now normalised (loads
`Acquire`, stores `Release`) with a test that passes exactly the orderings
`options.rs` uses; poisoned, it panics in `core`'s `atomic.rs`. And the test
itself first sat inside a module `cfg`-gated to the target that needs it, so it
could never run anywhere it would be run — the module now also compiles under
`test`, which is the only reason the ordering bug was caught at all.

---

### 2.13 The speed comparison — what rusty_alloc is actually FOR (added by P4c)

§2.9 through §2.12 measured footprint, where esp-alloc wins structurally.
Throughput is the other half, it is what a size-class page allocator with
per-class free lists exists to buy, and until P4c it was **unmeasured** — which
means every claim about it, in either direction, was an opinion.

Same board, same one-source-two-arms harness, both arms given the SAME 192 KiB
(footprint and speed are different experiments: one asks for the least an
allocator can live on, the other must not let a budget difference masquerade as
a speed difference). Nanoseconds per allocate/free pair, best of 5:

| workload | esp-alloc | rusty_alloc | speedup |
|---|---:|---:|---:|
| harness floor, no allocator call | 162 | 162 | — |
| 32 B alloc/free, one size | 1,638 | 625 | 2.62x |
| 64 mixed blocks (8-512 B), batch out then back | 1,792 | **844** | **2.12x** |
| **churn: 64 live, random 8-512 B, random replace** | 3,987 | **1,012** | **3.94x** |
| 2048 B alloc/free | 1,638 | **1,267** | 1.29x |

All rows are NET of the 162 ns harness floor. **Superseded by §2.15**, which
re-measured them after P4e's reclamation fixes: the fixes cost 2.2-7.9 %, so the
shipping numbers are 2.56x / 2.08x / 3.83x / 1.20x. The churn row is the one that
matters: it is the shape real code has and the shape that fragments a first-fit
list, and it is where the gap is widest. The 2048 B row is the narrowest because
2 KiB is exactly `MEDIUM_OBJ_SIZE_MAX` at this geometry, so it lands in a medium
page rather than the small fast path.

**Four guards, because an allocator benchmark is unusually easy to fake.**

1. **The harness measures itself.** A baseline arm runs the identical loop, the
   identical non-inlined `touch`, the identical four volatile accesses, and
   never calls the allocator. It reported **162 ns/op in BOTH arms** — equal, as
   it must be, since it shares every line. Subtracting it matters: unsubtracted,
   the churn ratio reads 3.53x instead of 3.94x, because a constant added to
   both arms always drags a ratio toward 1.
2. **The optimiser cannot delete the work.** Each block is written and read back
   with `write_volatile`/`read_volatile` and folded into a checksum. Without
   this an alloc/free pair is dead code and the benchmark times an empty loop —
   the classic way to measure an allocator as infinitely fast.
3. **Work parity is proven.** Every checksum is printed and **every one matches
   across the two arms** (25474400, 13944320, 9029440, 25474400). Both
   allocators provably serviced the identical size sequence — the sizes come
   from a seeded xorshift32, never from a clock.
4. **A null arm.** The same benchmark twice inside one arm reproduced to the
   nanosecond in both arms (625/625, 1638/1638). Spread over 5 runs was <= 1 %
   on every row. So the instrument's resolution is far below any gap claimed.

**The first run of this benchmark was wrong, and the number said so.** It
reported 2,361 ns/op for a 32-byte alloc/free pair — about 570 cycles for a fast
path that should be tens. The cause was `esp_hal::Config::default()` leaving the
CPU at 80 MHz rather than 240; pinning `CpuClock::max()` moved every row by
almost exactly 3x, which is the confirmation that the clock, and not the
allocator, was what had been measured. An impossible number is the instrument
asking for help.

**And the footprint side, measured rather than estimated.** §2.9 asserted
esp-alloc's floor as "~5.4 KiB (bytes live + headers)" from arithmetic. It has
now been measured by shrinking its heap until it fails: **esp-alloc runs the
same fs workload in 8 KiB**, against rusty_alloc's 68 KiB. So the honest pair is
**2.1-3.9x faster, at 8.5x the RAM** — and both halves belong in the README,
because a speed claim published without its cost is the kind of claim nobody
should believe.

---

### 2.14 The stress battery — what actually breaks (added by P4d)

Everything up to here measured a workload that WORKS. P4d asks the opposite
question: given adversarial content types, where does this thing fail? Eight
tests, on the board, both arms, every allocation null-checked so one failure
does not end the run.

**It found a real defect, a real gap, and three structural limits — and it also
found a bug in itself first.**

#### 0. The battery's own double free, caught by the allocator

The first run aborted in `page::double_free_abort`. The cause was the harness:
a shared `PTRS` slot array that `exhaust_recover` populated and never cleared,
which `class_sweep` then iterated further than it had written, freeing stale
addresses. **rusty_alloc was right and the harness was wrong** — and the
README's "a double free aborts instead of corrupting" is now demonstrated on
silicon rather than asserted.

#### 1. DEFECT (fixed): a forced collect could not reclaim a bin's last page

`heap.rs`'s `collect_inner` discarded `force` (`let _ = force;`) and applied the
keep-one-page-per-bin exemption on EVERY path:

```rust
if page_all_free(p) && !((*q).first == p && (*q).last == p) {
```

Upstream's `mi_heap_page_collect` frees an all-free page **unconditionally** at
`MI_FORCE`; the keep-one cache is `mi_page_retire`'s policy, not collect's. So
`mi_collect(true)` could never return a size class's slice to a different class.

Invisible at the shipped 32 MiB geometry — 512 slices per segment absorb one
cached page per class. **Fatal at the small profile's 16.** Measured, at
192 KiB, as the battery touched more classes:

```
512 B capacity:  168 -> 104 -> 72 -> 56 -> 24 -> 24 -> 24 -> 8
collect(true):   8 before, 8 after        <- recovered NOTHING
churn:           22,533 of 50,000 allocations returned NULL
                 ... with 61,440 bytes of the region still free
```

With the fix (`force || !only_page_in_bin`):

| | before | after |
|---|---:|---:|
| capacity recovered by `collect(true)`, 192 KiB | 8 -> 8 | **8 -> 240** |
| capacity recovered by `collect(true)`, 68 KiB | — | **8 -> 120** |
| NULLs in 50,000 churn ops, 192 KiB | 22,533 | **2,925** |
| segments ever created (i.e. releasable) | 2 | **3** |

That last row matters on its own: segments could not be RELEASED before, because
a retained page pinned every one of them.

Regression test: `heaps::forced_collect_reclaims_a_bins_last_page` asserts both
halves — an unforced collect keeps the cache, a forced one reclaims it. Poisoned,
it reports `retired 0 -> 0`.

#### 2. GAP (found, not fixed): there is no automatic collect at all

`generic_collect` is declared in `OPTION_NAMES` with a default of 10,000 and is
**never read anywhere in the crate**. The only `collect` callers are the two
public entry points and teardown. Upstream runs a collect every
`generic_collect` trips of the generic path.

So the capacity that `collect` can now recover is never recovered on its own,
which is exactly why churn still fails: **2,925 NULLs of 50,000 at 192 KiB, and
24,853 at 68 KiB**, from a heap that a single `collect(true)` restores.

Wiring it is the obvious fix and it is deliberately NOT taken here: it changes
behaviour on every platform, including the instruction counts this crate
publishes in its README, so it needs the callgrind harness rather than a board.
It is the top item in §6.

#### 3. Structural: a whole-segment request needs a whole free segment

Sizes 61,439 / 61,440 / 61,441 / 65,536 and `align = 65,536` return NULL at a
192 KiB budget with 61,440 bytes free, because none of that free space is a
contiguous `SEGMENT_SIZE`-aligned 64 KiB. A region yields
`floor((N - FIXED_PAGE) / SEGMENT_SIZE)` segments and strands the remainder.
**Size an embedded region as `k * 64 KiB + 4 KiB`**, or the tail is dead to
large allocations.

#### 4. Structural: the `bins x page` floor, confirmed dynamically

`class_sweep` held 21 of 24 distinct classes in 2 segments — and that is
arithmetic, not a defect: 30 usable slices, small classes cost 1 slice, classes
above `SMALL_OBJ_SIZE_MAX` (512 B) cost `MEDIUM_PAGE_SLICES` = 4. Nineteen small
plus two medium is 27 slices; a third medium needs 4 more and there are 3. It
stops exactly where §2.9's formula says it must.

#### 5. **68 KiB is a workload-specific floor, not a general budget**

At 68 KiB the battery holds **5 of 24 classes** and starts refusing 1 KiB
allocations at any alignment. §2.10's 68 KiB is the least memory that runs *the
fs sketch*, and nothing more should be read into it. The README says "smallest
heap that runs the same workload", which is precise, but the caveat is worth
stating out loud.

#### What did NOT break

With reclamation between tests (`--cfg ra_isolate`), `realloc_chain`,
`zalloc_dirty`, `fragmentation` and `exhaust_recover` all **PASS**, and capacity
holds flat at 240. Prefix preservation across a realloc chain that crosses every
routing boundary, re-zeroing of recycled dirty pages, 2 KiB requests over a
holed heap, and full capacity restoration after exhaustion are all sound. Every
failure above is capacity, not correctness.

#### The esp-alloc control

Same battery, same budget, esp-alloc: **every test PASS, capacity flat at 383,
no decay, no NULLs in churn.** A linked-list heap has no per-class page cache to
starve on. This is the honest counterpart to §2.13's speed table, and the two
belong together.

---

### 2.15 The retest — both reclamation gaps closed, head to head (added by P4e)

§2.14 fixed one defect and named two more gaps. P4e closes them and re-runs the
whole comparison against esp-alloc, stress and speed, on the board.

#### What changed

1. **The keep-one exemption is gone from `collect` entirely.** §2.14 gated it on
   `force`, which was a partial port. Upstream's `mi_heap_page_collect` calls
   `_mi_page_free` whenever `mi_page_all_free(page)` at EVERY collect level —
   the comment is "this will free retired pages as well" — and the
   keep-one-page-per-bin cache is `mi_page_retire`'s policy on the free path.
2. **`generic_collect` is wired.** Declared with a default of 10,000 and read by
   nothing; now a per-heap countdown in the generic path, exactly as upstream.
3. **`malloc_generic` reclaims once before returning null.** This is the one
   that actually mattered here, and measuring said so: the battery makes only
   ~649 generic trips in total, so a 10,000 threshold never fires. A page
   allocator can be "full" while holding empty pages for classes nobody is
   asking for, and reporting OOM in that state is wrong. Costs nothing on the
   happy path — it runs only when the allocation was about to fail.

#### Stress, head to head at 192 KiB

| test | esp-alloc | rusty BEFORE | rusty AFTER |
|---|---|---|---|
| boundaries (14 sizes, every routing edge) | PASS | FAIL | **PASS** |
| alignments (8 B .. 64 KiB) | PASS | FAIL | FAIL (64 KiB only) |
| realloc chain 8 B -> 32 KiB, prefix held | PASS | FAIL | **PASS** |
| zalloc over dirtied pages | PASS | FAIL | **PASS** |
| fragmentation adversary | PASS | PASS | PASS |
| exhaust and recover | PASS | PASS | PASS |
| distinct classes held at once | 24 | 9 | **21** |
| NULLs in 50,000 churn allocations | 0 | 22,533 | **575** |
| 512 B capacity across the battery | flat 383 | **168 -> 8** | **flat 240** |

**The capacity ratchet is gone.** It no longer decays at all. Churn failures are
down 97.5 %.

The two remaining refusals are the documented structural floor, not defects:
a `SEGMENT_SIZE`-aligned request needs a whole free 64 KiB segment and only
61,440 bytes remain contiguous (§2.14.3); and 21-of-24 classes is exactly what
30 usable slices hold once classes above `SMALL_OBJ_SIZE_MAX` cost
`MEDIUM_PAGE_SLICES` = 4 each (§2.14.4).

#### Speed, and what the fixes cost

| workload | esp-alloc | rusty BEFORE | rusty AFTER | speedup now |
|---|---:|---:|---:|---:|
| harness floor | 162 | 162 | 162 | — |
| 32 B alloc/free | 1,638 | 625 | 639 | **2.56x** |
| 64 mixed, batched | 1,792 | 844 | 863 | **2.08x** |
| churn 64 live, 8-512 B | 3,987 | 1,012 | 1,042 | **3.83x** |
| 2048 B alloc/free | 1,638 | 1,267 | 1,367 | 1.20x |

**The fixes cost 2.2-7.9 % of throughput.** Worth it: the alternative is an
allocator that reports OOM while hoarding reclaimable memory. The esp-alloc arm
reproduced to the nanosecond across sessions (same floor, same checksums), so
the deltas are the allocator and not drift. README and crate README are updated
to these numbers — the old ones are no longer what the code does.

#### The host test that asserted nothing, twice

The regression test for the reclaim-and-retry passed **with the fix removed**,
in two successive versions:

1. A 4-chunk arena at the shipped geometry is 128 MiB; 48 cached pages cannot
   starve it. Gated to `ra_small_profile`, where the cache is scarce.
2. Still passed: the threshold was `served > 64`, and the poisoned arm serves
   **128**. Both arms sat above it.

Fixed by measuring both arms first and putting the threshold between them —
240 with the fix, 128 without, assert `> 192`. A threshold picked before the
arms are known is a guess, and a guess that lands outside the interval asserts
nothing. `parameterizing-a-constant` §4 in one sitting, twice.

#### One more upstream mechanism we do not have

`mi_page_retire`'s `retire_expire` countdown — a retired page freed after N
further generic trips — is **not implemented**. It is why cached pages
accumulate rather than ageing out. Not needed now that collect and the
failure-path reclaim work, but it is the principled version and belongs on the
list.

---

### 2.16 The production-readiness pass (added by P5)

Six blockers were named when the question "is this commercial ready?" was asked
against the state after P4e. This is what closing them cost.

**1. CI gated none of this work.** `ci.yml` built wasm but never set
`ra_small_profile`, never passed `--no-default-features`, and never targeted a
chip — so every defect P0-P4e found would have sailed through. A new `embedded`
job now runs the small-profile suite, clippy on both the small profile and
`no_std`, and builds BOTH bare-metal RISC-V targets at BOTH geometries, plus
`rusty_alloc-api`. Xtensa stays out because it is not a stock rustup target; the
board runs are evidence, not a gate.

**2. The release state was incoherent.** `Cargo.toml` said `1.1.5`, the README
said `1.1.4`, tags stopped at `v1.1.4`, and a commit was titled
`chore: release v2.0.0`. The truth: release-plz titled the PR after
`rusty_alloc_api`'s major bump while the workspace went to 1.1.5. README now
states what is released and that `main` is ahead of it, and the CHANGELOG's
`[Unreleased]` section documents every fix and addition from this campaign so
the next release notes are true.

**3. The published instruction counts were stale.** They were measured at
`v1.1.5` and predate every reclamation change. `bench/icount-arms.sh` already
regenerates every column; nothing ran it. A scheduled `icount` CI job now does,
and the README carries provenance plus the explicit warning that the ratios are
a floor rather than the number until it is re-run.

**4. Vacuous tests.** Four tests in this campaign passed under the exact bug
they existed to catch. `tools/gate-selftest.sh` now reintroduces five defects
and requires the suite to go red for each; it runs in CI beside the semgrep
selftest and the unsafe census. A gate that stays green with its defect present
is reported as VACUOUS by name.

**5. `retire_expire`.** Upstream gives each retired page a countdown so a sole
empty page ages out after ~16 generic trips. Implementing it needs a
retired-bin range on the heap, and `alloc::retire_or_abort` is deliberately
written to decide keep-one-warm from the page's own links so it never resolves
the heap — a measured optimisation this machine cannot re-profile. Since
`collect` now reclaims a bin's last page, a shorter sweep period buys the same
ageing without touching that path. Swept on the board:

| `generic_collect` | churn NULLs / 50,000 | ping | batch | churn | large |
|---:|---:|---:|---:|---:|---:|
| 10,000 (upstream) | 575 | 639 | 863 | 1,042 | 1,367 |
| **512 (shipped)** | **357** | 640 | 871 | 1,069 | 1,380 |
| 64 | 334 | 652 | 867 | **1,168** | 1,473 |

64 costs 12 % of churn throughput to buy 23 fewer failures; 512 costs ~1 % and
buys 218. The default is now geometry-aware — 10,000 at the shipped geometry,
512 at the small profile — and upstream's per-page countdown stays unimplemented
and recorded in §6.

**6. The `no_std` single-thread footgun.** Three things were sound only because
there is one thread — `SingleThreadCell`'s `unsafe impl Sync`, `prim::fixed`'s
constant thread id and spin lock, and `options`' split 64-bit atomics — and all
three fail quietly rather than loudly. A doc comment is not a guard: `no_std`
now refuses to compile without `--cfg ra_single_threaded`, and CI asserts the
gate's negative case.

**And one defect the pass created and caught.** Python's text-mode write
converts `\n` to `\r\n` on Windows, so every file rewritten by a helper script
this campaign flipped LF -> CRLF. The content diff was 85 lines; the diff git
showed was 4,043. Normalised back to LF across 23 files, and the tell was
`git diff -w` disagreeing with `git diff` by two orders of magnitude.

---

## 3. The phases, cheapest disqualifier first

Each phase ends in a kill test. A phase that fails its kill test ends the
plan with a ledger row, and that is a result.

### P0 — does it compile at all? (host, hours) — ✅ **DONE 2026-09-07**

Add `riscv32imac-unknown-none-elf` to the toolchain file and build the core
crate for it. Do not fix anything; collect the error list.

**Kill test:** a complete, categorised list of what fails, checked against
§2. If the list is materially larger than the four walls above, this plan is
wrong and needs rewriting before any code moves.

**Verdict: PASSED — the plan stands, amended.** Full numbers and method in
[`docs/LEDGER.md`](../../LEDGER.md). 224 errors reported, **183 of them one
root** (no `#![no_std]` ⇒ no prelude); the real debt is **55 in four
buckets**:

| bucket | errors | wall |
|---|---:|---|
| `prim` has no backend arm for this target | 16 (1 cause) | §2.4 ✅ |
| explicit `std::` paths | 10 | §2.3 ✅ |
| 64-bit atomics | 5 | §2.2 ✅ |
| alloc-dependent text | 13 | **§2.5, new** |
| cascade from the first two | 11 | — |

Three walls confirmed, one (§2.1) shown to be **unmeasurable by this phase**,
one new wall found and it is cheap, and §1's central premise found false.
That is five corrections, not a rewrite. **Nothing was fixed**; the only
change kept is the target line in `rust-toolchain.toml`.

Reproduce, including the one-line probe that separates the root from the
cascade (applied, measured, reverted):

```sh
rustup target add riscv32imac-unknown-none-elf --toolchain 1.97.1
cargo build -p rusty_alloc --target riscv32imac-unknown-none-elf   # 224
# then, temporarily, `#![cfg_attr(ra_p0_probe, no_std)]` atop lib.rs:
RUSTFLAGS="--cfg ra_p0_probe" \
  cargo build -p rusty_alloc --target riscv32imac-unknown-none-elf # 55
```

**Read this before P1:** the 224 was within one step of tripping this
phase's own kill test ("materially larger than the four walls") on an
artifact. A count dominated by a single root measures the root, not the
program — the same rule this repository already applies to timings.

### P1 — the memory seam (host) — ✅ **DONE 2026-09-07** (2 of 3 kill-test items; the third is blocked on P2)

Introduce the primitive-memory seam and implement it twice: the existing
platform path, and a fixed-region path. Nothing else changes.

**Kill test:** the whole existing battery passes unchanged on the host, the
benches move by less than the harness's own null-arm floor, and a new test
builds an arena over a static 512 KiB region and serves allocations from it.

**What landed:** `crates/rusty_alloc/src/prim/fixed.rs` — a first-fit,
coalescing free list of at most 32 extents over a `&'static mut [u8]` handed
over once — and the fifth arm in `prim/mod.rs`. Always compiled, selected only
where no platform arm matches. **Zero unsafe dereferences added to the shipped
crate** (6 `unsafe fn` signatures with safe bodies, 11 in tests; `UNSAFE.md`
updated, ratchet re-baselined 864 → 881). Full numbers in
[`docs/LEDGER.md`](../../LEDGER.md).

| bucket | P0 | P1 |
|---|---:|---:|
| A. `prim` has no backend for this target | 16 | **0** |
| B / C / D / E | 39 | 39 (**+0** each) |
| **total** | **55** | **39** |

**Kill test, item by item:**

1. ✅ **Battery unchanged** — 33 suites / 103 tests / 0 failed, clippy
   `-D warnings` clean, `fmt --check` clean, unsafe ratchet OK, `wasm32` still
   builds and still picks its own arm.
2. ✅ **Benches** — met more strongly than asked. The shipped cdylib is
   identical on every deterministic quantity: **size delta 0** (212,992 both
   ways) and **export set 316/316 identical**. There is no delta for a bench to
   resolve. Note for whoever tries this next: `sha256` of the artifact is
   **inadmissible** on Windows — a null arm (same source, built twice) produced
   different hashes, because a PE embeds a build timestamp.
3. ⚠️ **BLOCKED on P2, and the mechanism is exact.**
   `arena::arena_register` computes `chunks = size / SEGMENT_SIZE` and returns
   `Err` when `chunks == 0` — so **every region below 32 MiB is refused by
   arithmetic**, and no arena can be built over 512 KiB until the geometry is a
   parameter. The *seam* half is done and proven at 512 KiB; the *arena* half
   moves to P2's kill test. This item was written before §2.1's shape was
   known; it is deferred, not redefined.

**§2.1 is executable now.** P0 recorded that the wall ranked first has no
compile-time signature. It has a runtime one: on a registered, entirely free
512 KiB region, both a `SEGMENT_SIZE`-sized request and a one-page request at
`SEGMENT_SIZE` *alignment* are refused, and both leave the free list untouched.
The alignment half is the one no larger region fixes — it is P2's real subject.

**Carry into P2:** fold §2.5 in (it is deletion, not a port), and delete
`rusty_alloc_ffi/src/lib.rs:9`'s false "the core crate is no_std" comment.

### P2 — the geometry (host) — ✅ **DONE 2026-09-07**

Make the segment size a compile-time parameter and follow it everywhere the
mask, the slice count, `LARGE_OBJ_SIZE_MAX` and the size classes assume 32
MiB. Add a small profile sized for a chip.

**Kill test:** the default profile stays byte-identical where the doctrine
demands it, and the small profile passes the full battery, the fuzz corpus
and the oracle on the host at 512 KiB of arena. This is the phase most likely
to be abandoned, and abandoning it here costs nothing on a board.

**Inherited from P1** — the item P1 could not reach: *an arena built over a
static 512 KiB region, serving allocations.* P1 proved the seam at that size;
`arena::arena_register`'s `chunks = size / SEGMENT_SIZE` is what refuses it,
and that line is this phase's subject. The two assertions at the end of
`prim::fixed::tests::serves_and_recycles_a_static_region` are the standing
before-picture: when P2 lands, the *alignment* one is what has to change
behaviour, and it should be re-read rather than deleted.

**Sizing, from the chip rather than the datasheet** (§2.1): the small profile
is aimed at **64–220 KiB**, with 96 KiB the number to design against — that is
what `blink-fs`, P4's own target, declares. Note also §2.6: `segment_map::MAP`
is 1 MiB of BSS independent of `SEGMENT_SIZE`, so parameterising the geometry
alone does not make the crate fit. Both have to move in this phase.

**Verdict: PASSED.** Full numbers in [`docs/LEDGER.md`](../../LEDGER.md).

- **The geometry is a `--cfg`.** `ra_small_profile` selects
  `SEGMENT_SLICE_SIZE` 8 KiB / `SLICES_PER_SEGMENT` 8 / `MEDIUM_PAGE_SLICES` 4
  — a **64 KiB segment**. A `--cfg` and not a cargo feature on purpose:
  features are additive and unify across a dependency graph, so two consumers
  wanting different geometries would silently get one of them. The
  DELIVERABLE sets it, the way a Janus firmware picks its chip.
- **Default profile:** 33 suites / **104 tests** / 0 failed; the shipped
  artifact unchanged in structure (212,992 bytes, 316 exports). Not a
  byte-identity claim — PE size at 4 KiB alignment is coarse, and the
  instruction counts need the Linux callgrind harness.
- **Small profile:** 19 suites / **84 tests** / 0 failed. clippy
  `-D warnings` clean on both.
- **§2.6 needed a third representation, not a smaller one.** The bitmap is
  sized by ADDRESS SPACE, so a smaller segment makes it *worse* (2²³ → 2³²
  bits). Replaced for this profile by an exact 64-entry range table — **1 KiB
  of BSS against 1 MiB** — beside wasm's base table. `ADDR_BITS` is derived
  now too, which shrinks the map on any 32-bit target.
- **P1's inherited item is DONE**: `prim::fixed`'s region test is two-sided,
  and at the small profile it asserts a whole segment, at segment alignment,
  served from a 512 KiB region.
- **Found on the way:** §2.7 — a real escape from the exclusive-arena API,
  present in the shipped build. Fixed, with a default-geometry regression test.

**What did NOT come free:** nine tests were pinning the geometry, in three
flavours — a literal where the unit is slices, a literal offset inside the
segment, and field-report fixtures that name specific MiB rows. The first two
were derived; the third is gated to the shipped geometry, because
re-expressing a report's rows in slices keeps them green while testing nothing
the report said.

### P3 — atomics and threads (host) — ✅ **DONE 2026-09-07**

Decide `portable-atomic` versus narrowing, per site, with the reason recorded
per site. Add the single-heap profile.

**Kill test:** `cargo build` succeeds for `riscv32imac-unknown-none-elf` and
for the Xtensa target the Janus toolchain provides, and the host battery is
unchanged.

**Verdict: PASSED, on every target and both geometries.** Full numbers in
[`docs/LEDGER.md`](../../LEDGER.md).

| target | geometry | result |
|---|---|---|
| `riscv32imac-unknown-none-elf` | default + small | **builds**, debug and release |
| `riscv32imafc-unknown-none-elf` | default + small | **builds** |
| `xtensa-esp32s3-none-elf` (esp toolchain, `-Z build-std=core`) | default + small | **checks clean** |
| x86-64 host `--no-default-features` | — | builds |
| `wasm32-unknown-unknown` | — | builds |

Host battery **unchanged and larger**: 33 suites / **105 tests** / 0 failed;
small profile 19 / 85 / 0; clippy `-D warnings` clean on the default,
small-profile and `no_std` configurations.

- **Atomics, per site with the reason** — §2.2's table. Four narrowed (the
  bitmaps and the seed counter, whose widths are choices), two shimmed with
  `portable-atomic` (the frozen `i64` option API and the `u64` C-ABI
  heartbeat, whose widths are contracts). The shim is target-gated, so the
  crate remains **dependency-free on every target it ships to today**.
- **The single-heap profile** — §2.3. `ra_thread_local!` is
  `std::thread_local!` with `std` and a plain `static` without: one heap, no
  TLS lookup, a *shorter* fast path.
- **§2.5 folded in**, as §6 asked, keeping the `out_fmt` hook seam alive.
- **A third instance of P0's bucket A**: `random::os_entropy` and
  `stats::process_info` also selected four ways with no default arm. Both fixed.
- **`rusty_alloc_ffi/src/lib.rs:9`'s false comment is now true** — the core
  crate really is `no_std` (behind the default `std` feature).

### P4 — on a chip (bench) — ✅ **PASSED 2026-09-07, on a XIAO ESP32-S3 Sense**

Declare it in the Janus `blink-fs` example, which is bare metal, has no
radio, and already passes a kill test on a board: it mounts a LittleFS
partition, reads a config file, and blinks at the rate that file sets.
Swap `esp-alloc` for this allocator and run that same test.

**Kill test:** the board prints the filesystem geometry, the config file and
the page title, and blinks at the file's rate — with this allocator
underneath. Then a number: heap high-water for the same workload, against
`esp-alloc` as the baseline, by the measurement rules (pinned, interleaved,
counters before clocks).

**Verdict: PASSED.** Board: esp32s3 rev v0.2, 8 MB flash, MAC
68:ee:8f:51:74:64, on COM4. Harness: `espino run --board xiao-esp32s3-sense
--expect`, which packs the filesystem, images, fits, flashes and monitors.

```
[heap] arm: rusty_alloc
littlefs 2.0: 1261 blocks of 4096
config.json: { "blink_ms": 250, "greeting": "hello from data/config.json" }
index.html: 318 bytes, title "blink-fs"
blinking GPIO21 every 250 ms
```

250 ms is the value read FROM the file, not the 500 ms fallback — which is what
makes the line evidence rather than decoration. **Not confirmed here: the LED
itself.** The firmware reports the interval it read; nobody's eye is on the
board from this session, and the espino ledger's own P1 row says "confirmed by
eye" for a reason.

The numbers are §2.8. Three things about the method are worth carrying:

- **The control ran first, and caught a stale board.** The unmodified example
  flashed to this XIAO reported "no filesystem at the record's partition" and
  blinked at the fallback — no LittleFS image was on it. Every later reading
  would have been ambiguous.
- **Neither allocator's own stats could answer the question.**
  `esp_alloc::HEAP.used()` read **0 at every stage** because the file buffers
  drop before each sample — a clean number measuring nothing — and its
  `max_usage` is behind a feature `rusty_alloc` has no counterpart for. The
  peak is therefore measured OUTSIDE both, by one wrapper present in both arms.
- **P3's LTO caution is discharged, not assumed.** Both arms report an
  identical PEAK and rusty_alloc's region consumption moves 0 → 132 KiB, so
  both allocators are provably reached.

Artifacts: `janus/espino/examples/blink-fs-p4` — one source, both arms, the arm
chosen by `--cfg ra_arm_rusty`. It is a copy, so the pristine `blink-fs`
example is untouched.

### P5 — the ESP-IDF track, honestly halved (bench)

On the ESP-IDF track the framework's C calls its own capability-aware
allocator directly, because it must ask for memory that is internal, or in
external RAM, or reachable by DMA. A Rust global allocator governs Rust's
allocations and nothing else.

So the claim here can only ever be "the Rust half is ours". Declare it in a
generated firmware beside the IDF heap and prove the device still boots,
joins a network and streams.

**Kill test:** a generated camera firmware runs its existing cell kill test
with this allocator serving the Rust half, and the ledger row says plainly
which half that is.

---

## 4. Non-goals

Replacing ESP-IDF's C heap. It is capability-aware in ways a Rust global
allocator has no vocabulary for, and the framework calls it directly.

DMA-capable or PSRAM-placement policy in v1. A chip allocator that must
answer "give me memory the DMA engine can reach" is a different contract, and
`esp-alloc` does not answer it either.

Any claim that a device has no C in it. It does: on the ESP-IDF track a
firmware compiles over a thousand C translation units, and on both tracks the
second-stage bootloader is C and the radio is a closed binary. This plan
changes who hands out heap memory, nothing more.

Interrupt-safety guarantees beyond what the current lock discipline gives.
If allocation from an interrupt handler is wanted, that is its own plan.

---

## 5. Open questions

~~Whether the small profile is the same allocator or a different one wearing
the name.~~ **ANSWERED by P2 — it is the SAME allocator.** No allocator code
path is forked: the free path, the page queues, span carving, the cross-thread
protocol, the arenas and the bins are the shipped code running on different
constants. The only per-target divergence is the segment map's representation,
which already had two (native bitmap, wasm base table) and now has three. One
`--cfg` selects three constants. There are not two architectures in this crate.

Whether the win is real. `esp-alloc` is simple and correct; the case for
replacing it rests on the double-free abort and on one codebase across the
portfolio. If P4's numbers show a materially larger footprint on a part with
512 KiB, the safety argument has to carry the whole weight on its own, and it
may not.

> **P4 (2026-09-07) SETTLES the measurement half, on silicon.** The workload's
> true demand is **4.9 KB**. esp-alloc serves it from 96 KiB; rusty_alloc needs
> **192 KiB — 2x the region, 37 % of the S3's entire SRAM — and +9.7 % of app
> flash** (§2.8). So the answer to "if P4's numbers show a materially larger
> footprint, the safety argument has to carry the whole weight" is: **they do,
> and it must.** What is now decidable is whether a double-free abort on a
> device that parses a radio, a UART and a flash partition is worth 96 KiB of
> SRAM and 11 KB of flash — a judgement, not an unknown. Two things move the
> price if it is close: the first-fit alignment fix in §2.8 (a third of the
> region, cheap) and a smaller segment than 64 KiB (P2's geometry is a
> parameter now, and 64 KiB was chosen to keep `SMALL_SIZE_MAX` consistent,
> not because it is a floor).

**P2 sharpens this rather than settling it, and honestly in both directions.**
Against: the dsp ledger's four-stage table shows a firmware whose heap is fully
committed at buffer time and never moves across eight kernels — a workload with
no allocation churn, where nothing this allocator's architecture optimises
applies, so the performance case is *nil* and the footprint case is uphill.
For: P2 found a real arena escape in a shipped safety-relevant API (§2.7) that
had been invisible for want of a workload that reached it, which is exactly the
argument for one codebase across the portfolio rather than a second small
allocator nobody exercises.

~~Which of the five `AtomicU64` sites are correctness and which are
statistics. Unknown until read.~~ **ANSWERED by P0 — one correctness site
(`arena.rs`'s `used`/`dirty` chunk bitmaps), and it is a bitmap whose word
width is a free choice. See §2.2.**

Whether the Xtensa fork toolchain can build this crate at all. The pinned
toolchain here is 1.97.1 with two x86_64 targets; Xtensa needs Espressif's
fork, installed separately, and its LLVM has bitten the Janus programme
before.

---

## 6. What to do first

~~P0, and nothing else.~~ **P0 through P3 are done (2026-09-07). The question
this plan exists to answer is answered: it is a PORT, and the code half is
finished.** P0 confirmed three walls, found a fourth invisible to itself, added
a fifth and falsified §1; P1 built the seam; P2 made the geometry a parameter
for two lines; P3 took the remaining 39 errors to **zero** and the crate now
builds for `riscv32imac`, `riscv32imafc` and Xtensa `esp32s3-none-elf`.

**P4 is done and passed on a XIAO ESP32-S3** (§2.8), and **P4b took the two
footprint levers** (§2.9, §2.10): the region went 192 KiB -> 132 KiB -> **68 KiB**
for the same 4,914-byte workload, with `PEAK` identical at every step and the
kill test green throughout. Flash is unchanged (+48 B), so it still costs
+9.8 % of app image over esp-alloc. The plan's own open question in §5 is a
judgement rather than an unknown.

**What is worth doing next, in order:**

1. **Re-measure the host instruction counts** (§2.15). P4e wired
   `generic_collect`, removed collect's keep-one exemption and added a
   reclaim-and-retry on the generic path. On the board that cost 2.2-7.9 % of
   throughput; the README's callgrind figures were taken before it and need
   re-running under `LD_PRELOAD` before the next release.
2. **`retire_expire`** (§2.15) — upstream ages a retired page out after N
   further generic trips. Not implemented. It is the principled version of what
   the failure-path reclaim now does reactively.
3. **P5**, the ESP-IDF track's honest half — and note it can only ever claim
   "the Rust half is ours". It is now the largest untouched thing in the plan.
4. **Retention** (§2.9 lever 3). `used` never falls: freed, not returned, by
   mimalloc's design. It does NOT shrink the region — the peak is what the
   region must hold — so this is worth doing only if something else on the part
   wants the SRAM back. Scope it against a workload that has a second consumer,
   or it is measuring nothing.
5. **Flash** (§2.9 lever 4), the only line where the gap has not moved.

**Closed, do not re-open:** the alignment fix (done, §2.10) and "a smaller
segment" (done differently, and the old framing here was wrong). Segment size
was never the lever; the SLICE was, and it moved without `SMALL_WSIZE_MAX`
moving at all — `direct[]` is a page-pointer cache indifferent to page kind.
The real constraint is `SEGMENT_SLICE_SIZE >= os::page_size()`, which
`bins::good_size` needs and nothing had written down; it is now a const assert
in `prim/fixed.rs` and it puts the floor at 4 KiB.

**Do not skip when re-measuring:** P3's carried caution is discharged for P4
(both arms reported an identical PEAK, so both allocators were provably
reached) but it applies again to every new arm.

**Do not carry the 224 into any conversation about scope** — it was 55 after
the prelude cascade, 39 after P1, and it is **0** now.
