# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **The small profile extended every page ONE BLOCK AT A TIME, so 100 % of
  allocations took the slow path.** `page_extend` bounds its batch at 4 KiB of
  payload and computed that bound with a hardcoded shift whose constant term is
  really `SEGMENT_SLICE_SIZE / 4096`. At the shipped 64 KiB slice the literal
  was right; under `--cfg ra_small_profile` the slice is 4 KiB, so the bound
  was **256 bytes instead of 4 KiB** — sixteen times too small. For a 512-byte
  class the batch computed to 0 and was clamped to 1, leaving every page with
  `capacity == 1` and no second block for the fast path to find. Counted over
  100,000 alloc+free pairs at the small profile, entries into `malloc_generic`
  per op: **512 B 1.0000 -> 0.1250, 513 B 1.0000 -> 0.1667, 1 KiB 1.0000 ->
  0.2500**, with page carve-and-retire churn falling from 195 per 100,000 to
  24/32/49. On a 32-bit host the 512-vs-513 step inverts from +8.6 % (slower)
  to −36 % (faster), about **1.9× faster at 512**. **Measured on silicon** too
  — a XIAO ESP32-S3, `main` against the fix on one board with identical
  checksums and floor: **13.0 % faster** on 32 B ping-pong, **15.7 %** on a
  64-block mixed batch, **15.3 %** on 8-512 B churn, and 1.1 % at 2,048 B,
  which is on the bin route this does not touch. Found by the Kairos RTOS
  report (`docs/plans/finished/fixed-prim-small-step.md` §8.7).
  **The default geometry is unchanged** — the derived constant equals the old
  literal there, and the all-features x86-64 assembly diff moves no executable
  function.

### Added

- **`--cfg ra_generic_collect="64" | "4096" | "65536"`**, so a bare-metal
  firmware can move the periodic-collect heartbeat. It is the only lever over a
  real trade — a sweep returns an empty page, and the next allocation of that
  class carves and extends a fresh one, so a short period costs page churn
  while a long one costs capacity — and it was unreachable: `options::set` is a
  no-op under `ONE_REGION`, and the option's own doc told firmwares to "change
  the default it is built with" when no cfg existed. The default does not move.
- **`prim::fixed::shape_of(size) -> Shape`** — page bytes, dedicated segments
  and `direct_route`, `const` and derived from the active geometry. Answers
  "which page kind, and how many region bytes, does this size cost", which
  `region_stats()` cannot because it reports over region extents.
  **`Shape::direct_route` names a boundary that moves with POINTER WIDTH**:
  `SMALL_SIZE_MAX` is `128 * size_of::<usize>()`, so it is 1,024 on a 64-bit
  host and 512 on a 32-bit chip. The Kairos RTOS measured a 16 % step there on
  a device and could not reproduce it on a workstation for exactly that reason
  (`docs/plans/finished/fixed-prim-small-step.md`).

## [2.1.0](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v2.0.5...rusty_alloc-v2.1.0) - 2026-09-10

### Added

- **`--cfg ra_segment_size="256k"`, for a firmware whose allocation unit is
  tens of kilobytes.** A segment's slice 0 is its header, so the largest object
  that can share a segment is `SEGMENT_SIZE - SEGMENT_SLICE_SIZE` — 61,440
  bytes at the default small profile — and one byte over that takes a dedicated
  run of segments. Since no allocation of `SEGMENT_SIZE` can share a segment
  with its own metadata, a 64 KiB request costs **two** segments and a 128 KiB
  request three. A `rusty_zstd` firmware measured the consequence on an
  ESP32-S3: one 64 KiB block served from a 256 KiB region, the second refused
  with 192 KiB unused. The flag moves the small profile to an 8 KiB slice x 32,
  raising `LARGEST_SHARED_ALLOC` to 253,952 so a 64 KiB request becomes a span
  three of which pack into one segment — **3 blocks instead of 1 in the same
  256 KiB region, measured on the board**. Opt-in, because it doubles the page
  floor `(classes touched) x slice` that a small-object workload pays.
- **`prim::fixed::LARGEST_SHARED_ALLOC`, `dedicated_segments(size)` and
  `region_for_allocs(size, count)`** — the sizing API that predicts the above at
  compile time, instead of leaving a firmware to discover it on silicon.
  `region_for_allocs` also counts the segment the first small allocation claims,
  which is what took the reporting firmware from two blocks to one.
- **`prim::fixed::PrimError`**, re-exported so the whole fixed-region recipe is
  reachable from one path. The type has always been public as
  `prim::PrimError`, but only from the parent module, so a seam re-exporting
  this API in a single `pub use` could name `Region`, `good_region_size`,
  `init_region` and the `FERR_*` values but not the type they fail with. The
  Kairos RTOS allocator seam hit exactly that and carried an "arrives with the
  next release" comment for it. Same type, one more path.
- **`prim::fixed::region_capacity() -> (free_segments, largest_servable)`.**
  `region_stats` reports free BYTES, and free bytes hide this failure: the
  refused allocation above had 126,976 bytes free and read
  `free_segments=1, largest_servable=61440`.

### Fixed

- **The README's footprint model did not cover large allocations and implied
  the opposite of the truth for them.** It described the floor as
  `(classes touched) x (page size)`, "independent of bytes requested" and
  amortising as the working set grows. That holds for small objects; for
  segment-sized ones the cost is a granularity tax that scales with how many
  are live. The section now says so, with the measured numbers and the flag.

## [2.0.5](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v2.0.4...rusty_alloc-v2.0.5) - 2026-09-09

### Semver note, read this one

**`prim::fixed::Region<N>`'s alignment is reduced**, from `SEGMENT_SIZE`
(32 MiB at the default geometry, 64 KiB under `--cfg ra_small_profile`) to
16 bytes. `cargo-semver-checks` classes a `repr(align)` change as breaking
and would have made this 3.0.0. It ships as a patch deliberately: `Region`
was introduced one release ago in 2.0.4, at the default geometry its old
alignment was 32 MiB and its own docs called that unhonourable by any
chip-sized `.bss`, and the documented use — `static HEAP: Region<N>` then
`HEAP.give()` — cannot observe the change. `size_of::<Region<N>>()`,
`Region::USABLE` and every other signature are untouched.

**If you depend on the old alignment for a reason of your own, set
`--cfg ra_aligned_region`**, which restores the 2.0.4 layout exactly: the
address mask on `free`, `Region` segment-aligned, and the linker gap in
front of it. Nothing else in the public API changed except the added
`REGION_ALIGN` const.

### Changed

- **A firmware's region no longer needs to be segment-aligned, and the
  linker's gap in front of it is gone.** On a fixed region segments are
  carved at `SEGMENT_SIZE` strides from the region's base, and `segment_of`
  masks the offset from that base instead of the address (`REGION_STRIDES`;
  wasm made the same trade with a slice table in 2.0.0). `prim::fixed::Region`
  is 16-byte aligned (`REGION_ALIGN`, new), `usable_bytes` / `MIN_REGION` /
  `good_region_size` are exact from any 16-byte-aligned base, and the
  firmware that reported the gap (`docs/plans/finished/region-alignment-dissolve.md`)
  gets **24,144 bytes of stack back** with `.bss` unchanged — the 24,148 that
  `size -A` could never show. The price is three instructions on every
  `free` on the ESP32-S3 (39 against 36), 9–17 ns per alloc/free pair on the
  board. **`--cfg ra_aligned_region`** (new) keeps the address mask and the
  segment alignment — the 2.0.4 layout, gap included — for a firmware that
  would rather have those; every sizing rule, the backend's placement and
  `Region`'s alignment follow the flag. Hosted builds are unchanged to the
  instruction. `FERR_MISALIGNED` still exists and still fires: a base off
  the 16-byte grid with an exact length loses its last segment to the
  run-up, exactly as before — `Region` cannot produce one. Strictly, a
  type's alignment moving is observable; nothing a firmware does with
  `Region` depends on it.
- The arena layer folds on `FIXED_REGION` (any fixed-region target) rather
  than on `ONE_REGION`: chunks carved on absolute segment boundaries are not
  a strided region's segments.

### Fixed

- **Every `free` on a single-threaded target paid an acquire load and a
  memory barrier for a comparison that had already folded to `true`.** The
  segment's owner thread id was read with `Acquire` before `ONE_THREAD`
  short-circuited the compare, and LLVM keeps an unused acquire; on the
  ESP32-S3 that was an `l32i` and a `memw` on the hottest path in the
  crate. The load is inside the predicate now: −8 ns per alloc/free pair on
  the board, hosted builds unchanged. Found reading the free path's
  disassembly for the change above.

## [2.0.4](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v2.0.3...rusty_alloc-v2.0.4) - 2026-09-09

### Fixed

- **A region sized by `good_region_size` at an unaligned base silently served
  a segment fewer than its size said** (reported by the Janus firmware:
  `good_region_size(220 * 1024)` at the base the linker chose served two
  segments, not three, and panicked in `handle_alloc_error` 484 bytes short of
  the round number that had happened to work). `init_region` now refuses a
  base that costs a whole segment against what the length promises, with
  `FERR_MISALIGNED`; a round region that strands as much aligned as it loses
  misaligned still passes.
- **The documented fix was worse than the bug, and every sizing rule was the
  reason.** The 2.0.3 rule `k * SEGMENT_SIZE + FIXED_PAGE` reserved a page of
  the region for the first heap's descriptor, so an exact region was never a
  whole number of segments, so a `#[repr(align(65536))]` container of it was
  rounded up to the next segment: 200,704 bytes became 262,144 in `.bss`, and
  the firmware that took the advice lost 60,952 bytes of stack. The first
  heap's descriptor on a one-region target is now a static of the fixed
  backend's (1,752 bytes), and the region is whole segments.

### Added

- `prim::fixed::Region<N>`: the region container a firmware should use.
  `SEGMENT_SIZE`-aligned by construction, `N` checked at compile time to be a
  whole number of segments, `size_of::<Region<N>>() == N` (asserted in the
  crate), `give(&'static self) -> Result<usize, PrimError>` hands it over once
  and returns the usable bytes; `Region::USABLE` for `const` assertions. On the
  rig it replaced the consumer's aligned container for **+63,780 bytes of
  stack** (both aligned, so the linker's gap cancels), and the footprint
  sketch runs on `Region<{ 64 * 1024 }>`: one segment, `65536 usable of
  65536`. Against a round *unaligned* region the honest gain is +2,828 bytes
  of stack for the same usable heap: the alignment gap the linker leaves
  before an aligned static is charged to no section, so a `.bss` delta
  overstates it — the `Region` docs say so.
- `FERR_MISALIGNED` (0xF141), `FIXED_PAGE` (now `pub`),
  `take_first_heap_box` / `is_first_heap_box`.

### Changed

- **The sizing rules lose their `+ FIXED_PAGE`**, which changes what these
  return: `MIN_REGION` is `SEGMENT_SIZE`; `usable_bytes(0, len)` is
  `⌊len / SEGMENT_SIZE⌋ · SEGMENT_SIZE`; `good_region_size(220 * 1024)` is
  196,608 (was 200,704); `region_for(192 * 1024)` is 196,608. A consumer
  `const`-asserting the old literal will fail to build, which is the point:
  the old shape is the padded one. The strict semver reading of a changed
  `const fn` result is a minor bump; it is presented as a fix because the old
  results were the defect.
- The README's recipe declares the region through `Region`, and its floor row
  reads 64 KiB plus the descriptor static (was 68 KiB).

## [2.0.3](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v2.0.2...rusty_alloc-v2.0.3) - 2026-09-09

### Added

- `prim::fixed::good_region_size(budget)` and `region_for(usable)`, `const fn`s
  that size a firmware's region so the 64 KiB granule strands nothing: the
  largest zero-waste region no bigger than a budget, and the smallest region
  serving at least `usable` bytes. `good_region_size(220 * 1024)` is 200,704
  (three segments plus the page) where a literal 220 KiB stranded 24,576.
- `prim::fixed::region_contains(addr)`, the one-region answer to "is this
  pointer ours".
- `--cfg ra_max_extents="8"` / `"16"` / `"64"` resizes the fixed backend's
  free-extent table (default 32 unchanged). Its doc states the bound: never
  more slots than live blocks plus one.

### Changed

- **Firmware code size, second pass: everything that exists to manage many OS
  ranges folds to what one linker-handed region needs.** A new internal
  predicate (`ONE_REGION`, the bare-metal arm of `ONE_THREAD`) folds arenas,
  the segment map, the runtime option table and the RAM-resident heap sentinel
  on a bare-metal target: `chunk_alloc` is `None`, `segment_map::contains` is
  two compares against the region bounds, `options::get` is the compiled-in
  default at each call site, and the heap sentinel and empty page live in
  `.rodata` (flash) with `create_heap` copying its template from there. On the
  ESP32-S3 firmware measured in `docs/plans/finished/firmware-what-is-left.md`
  the allocator's flash cost went **+7,860 → +3,208 B** and its static RAM
  **+3,052 → +284 B** (attributed code 8,262 → 4,313 B); board throughput is
  unchanged to within 5 ns per operation. Hosted builds are byte-for-byte
  unchanged; the wasm ratchet is flat.
- What a bare-metal build gives up by it, stated: `arena::reserve_os_memory_ex`
  and `manage_os_memory_ex` return `Err` there, and `options::set` /
  `set_default` are no-ops. Neither had a working meaning on a chip before.
- README embedded figures refreshed from a same-day board run of both arms:
  2.80x / 2.17x / 3.98x / 1.45x (the 2.0.0-era rows were 7-9 % stale — 2.0.2's
  free fold had already moved them), the cost table above, the region-sizing
  advice, and the placement caution for kernel comparisons across allocators.

## [2.0.2](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v2.0.1...rusty_alloc-v2.0.2) - 2026-09-09

### Changed

- **Firmware code size halved: `--cfg ra_single_threaded` now prunes what it
  promises.** The cfg used to change only correctness assumptions; the code
  that a single context can never reach — abandoning a segment when a thread
  ends, adopting one back, the delayed list a cross-thread free lands on, the
  thread-exit hook — was still linked. On an ESP32-S3 firmware built both ways
  from one source, the allocator's flash cost fell from **+16,584 B to
  +7,860 B** and its attributable code from 16,256 B to 8,262 B, measured with
  `size -A` and `nm --size-sort` on the linked ELF, one brick at a time. One
  internal predicate (`ONE_THREAD`: the fixed backend under
  `ra_single_threaded`, or `wasm32-unknown-unknown` without atomics) folds the
  branches so the linker can see it; hosted builds compare thread ids exactly
  as before and are byte-for-byte unchanged.
- The same predicate takes **10.7 % off the gzipped wasm bundle** (22,574 →
  20,169 bytes); the ratchet baseline is updated in the same commit.
- The README's embedded section now carries the flash and static-RAM cost
  beside the heap floor, and the hazard the decomposition exposed: static RAM
  comes straight out of `.stack`, to the byte, so a firmware near its stack
  limit adopts this and gets an overflow rather than a bigger binary.

### Fixed

- **Guarded-object sampling ran on targets that cannot protect a page.**
  `prim::fixed` and `prim::wasm` return `Err` from `protect`; the sampler used
  to accept a rate anyway and hand out a dedicated segment with an unprotected
  trailing page — the full cost of a guarded object and none of the protection
  — while `try_guarded` and the ChaCha block function it samples with stayed in
  a `secure`-off image. Guarded sampling is now compiled out where no guard
  page can exist (`guarded_set_sample_rate` leaves it off there), and a heap's
  CSPRNG is seeded only where something draws from it. On the ESP32-S3 that was
  2,366 bytes of unreachable code plus 1,621 bytes of seeding for a generator
  nothing read.
- **`--features linkcheck` without `secure` did not compile.** The non-secure
  link check still passed the page extent that the dropped narrowing used to
  take. CI built only the default and `--all-features`, so the combination
  rotted unseen; it now builds, and CI clippies each optional feature alone.
- A `prim::fixed` unit test asserted that a `SEGMENT_SIZE`-aligned page cannot
  come out of a region smaller than a segment, which is only true when the
  region does not straddle a boundary — the loader decides that, and on
  2026-09-08 it put CI's 512 KiB window across a 32 MiB line. The allocator was
  right; the test now decides from the address.

## [2.0.1](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v2.0.0...rusty_alloc-v2.0.1) - 2026-09-08

### Added

- `prim::fixed::MIN_REGION` and `prim::fixed::usable_bytes(base, len)` — a
  firmware can now size its heap region at COMPILE time
  (`const _: () = assert!(N >= MIN_REGION)`) instead of discovering the answer
  on silicon, and can log how much of a region can actually back segments. The
  `k * SEGMENT_SIZE + FIXED_PAGE` rule previously existed only in a design
  document; a 220 KiB region strands 24,576 bytes and nothing said so.
- Distinct `prim::fixed` error codes: `FERR_TOO_SMALL`, `FERR_GEOMETRY`,
  `FERR_REGISTERED`. One sentinel covered three conditions with three different
  fixes.

### Fixed

- **`init_region` accepted a region that could never yield a segment.** Setting
  `--cfg ra_single_threaded` (which the crate demands loudly) without
  `--cfg ra_small_profile` (which nothing demanded) left `SEGMENT_SIZE` at
  32 MiB, so a kilobyte-scale region returned `Ok(())`, linked clean, and then
  failed every allocation on the board with a backtrace pointing at whatever
  allocated first. It now returns `FERR_GEOMETRY`, checked against the real base
  address rather than the length alone — an unaligned base needs up to
  `SEGMENT_SIZE - 1` more than a length test would demand.
- **An allocating interrupt handler hung the firmware silently.**
  `prim::fixed`'s lock is not reentrant, and on a single-context target a lock
  observed held can only mean reentrancy. That is now a panic naming the ISR
  instead of an unbounded spin that surfaces as a watchdog reset. Confirmed with
  a load, because `compare_exchange_weak` may fail spuriously and a bare CAS
  failure would misfire.
- The README documents `--cfg ra_small_profile`, which it never mentioned, and
  attaches it to the 68 KiB figure that is only true under it.

  All four reported by the first outside firmware to adopt 2.0.0
  (`docs/plans/embedded-adoption.md`).

### Changed

- `portable-atomic` is `optional` and enabled by `std`, so it is pulled only
  where it is reachable — a target without 64-bit atomics that also has `std`.
  A `no_std` firmware was fetching and compiling a crate it then discarded at
  link time. Costs nothing on the device either way; it buys an accurate
  dependency graph in an SBOM or a `cargo audit`.

## [2.0.0](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v1.1.6...rusty_alloc-v2.0.0) - 2026-09-07

### Breaking

Two changes require a major bump. Neither affects a consumer using default
features, which is the overwhelming majority.

- **`default-features = false` now selects `no_std`.** Before this release the
  crate had `default = []`, so `default-features = false` was identical to the
  default and got the full crate. It now selects the single-heap `no_std`
  profile, which additionally refuses to compile without
  `--cfg ra_single_threaded`. If you set `default-features = false` and want
  what you had, set `features = ["std"]`. **Verified against the downstream
  corpus** (`tools/corpus/`): `spacedb-sdk` (plain and `secure`),
  `rusty_alloc_default` and `rusty_zstd` compile unchanged; `rusty_maplibre` is
  broken by this and is fixed by adding `features = ["std"]` to both of its
  `rusty_alloc` dependencies -- tested by applying it, not assumed.
- **`heap::Heap` gained a field** (`generic_countdown`). Every field on it is
  `pub` and it is not `#[non_exhaustive]`, so a struct literal naming all
  fields no longer compiles. Nothing constructs a `Heap` that way in practice —
  it needs raw pointers and a `PageQueue` array — but semver is semver.

### Fixed

- *(heap)* **`collect` could not reclaim a size class's last empty page.** The
  keep-one-page-per-bin reuse cache is `mi_page_retire`'s policy on the free
  path; `collect` had borrowed it, so no collect at any level could return a
  class's page to a different class. Upstream's `mi_heap_page_collect` frees an
  all-free page unconditionally ("this will free retired pages as well").
  Harmless at the default 32 MiB geometry, where 512 slices per segment absorb
  a cached page per class; severe where slices are scarce.
- *(heap)* **nothing ever collected automatically.** `generic_collect` was
  declared as an option with a default of 10,000 and read nowhere. It is now a
  per-heap countdown on the generic path, as upstream.
- *(heap)* **the generic path reported OOM while holding reclaimable memory.**
  It now reclaims once and retries before returning null. Free on the happy
  path — it runs only when the allocation was about to fail.
- *(segment)* an exclusive arena's huge-path allocations could escape to the OS
  instead of failing, diverging from `mi_segment_huge_page_alloc`.
- *(arena)* the chunk bitmap scan stopped at the first word: an arena of more
  than 32 chunks could not allocate past chunk 31.
- *(heap)* `stats.segments` was not incremented on the huge path, so it did not
  balance `segments_freed`.
- *(prim)* the fixed-region backend placed every allocation bottom-up, so one
  page-sized block below a segment boundary cost a whole segment of reach.

### Added

- `no_std` support behind a default-on `std` feature: the core allocator builds
  and runs on bare metal (`riscv32imac`/`imafc` gated in CI, ESP32-S3 measured
  on hardware).
- `ra_small_profile`: a second geometry (4 KiB slices, 64 KiB segments) for
  parts with kilobytes rather than gigabytes. Non-additive, so it is a `--cfg`
  rather than a feature.
- `prim::fixed`, a fixed-region backend for targets with no OS —
  `init_region`, `region_stats`.
- `options::GENERIC_COLLECT`, the index of the `generic_collect` option.
- `segment_map::range_table_overflowed`, an observable latch for the
  small-profile segment map.

### Changed

- 64-bit atomics on targets without them: `no_std` builds use two `AtomicU32`
  halves instead of `portable-atomic`'s lock-based fallback (sound under the
  single-threaded assumption `no_std` already carries), removing the dependency
  from that path.

### Performance

- Medium allocations (above `SMALL_SIZE_MAX`, up to `MEDIUM_OBJ_SIZE_MAX`)
  collect-and-retry the bin queue front before the slow-path heartbeat. Through
  `GlobalAlloc` those sizes reach `malloc_generic` on **1.000** of their calls,
  so the saving lands on every one: **+14-16 % on a 2 KiB tight alloc/free
  loop**, and it survives multithreading (+12-26 % with two threads each freeing
  their own blocks). The retry **turns itself off per heap** once that heap is
  seen receiving cross-thread frees: `free`, `local_free` and `xthread_free` are
  adjacent in a `#[repr(C)]` `Page`, so peeking a page another core is freeing
  into costs 20-30 %, and no variant of the peek avoids it. Measured on all
  three targets that can run it: **+14-16 % native x86-64, +1-16 % wasm32 in
  V8, +15 % on an ESP32-S3** (`no_std`, small profile), with every other
  workload unchanged. Host INSTRUCTION counts under callgrind still pending.
- **On wasm, 4.9-7.3x faster than the Rust default allocator on a churn workload**
  (64 live blocks, random 8-512 B), measured in node with a subtracted harness
  floor and checksums proving work parity. A tight 2 KiB same-size loop is
  ~0.7x, for a reason `alloc.rs` already documents as measured-and-refuted.
- **wasm modules are 8,551 bytes smaller (3,931 gzipped)** — the allocator's
  gzipped overhead on a minimal consumer falls from +7,760 to +3,829 bytes,
  roughly halving what it adds to a bundle. Two causes, both measured
  (`docs/plans/wasm-size.md`), plus a `tools/wasm-size.sh` CI ratchet so it
  cannot regress unnoticed.
- `ra_thread_local!` takes the single-`static` arm on `wasm32-unknown-unknown`
  without the atomics proposal, which `prim/wasm.rs` has assumed single-threaded
  since it was written. A `std::thread_local!` there linked lazy init,
  destructor registration and an "accessed during or after destruction" panic
  that can never run.
- **wasm modules are 8,180 bytes smaller (3,700 gzipped).** The option
  environment pass ran on `wasm32-unknown-unknown`, where `std::env::var` is a
  stub that always fails: 38 iterations formatting 76 strings and allocating 76
  `String`s at startup, to read an environment that cannot exist. It also made
  `options::get` the largest function in a wasm build at 3,708 bytes and dragged
  `str::to_uppercase`, `alloc::fmt::format` and the `OPTION_NAMES` table in with
  it. Measured against the Rust default allocator on a minimal consumer, the
  allocator's gzipped overhead falls from +7,760 to +4,060 bytes.
- `options::error` renders its code into a stack buffer instead of `format!`,
  and `out_fmt` writes bytes instead of `eprint!`. Worth ~0 on wasm (measured),
  but it removes an allocation from an error path and gives `no_std` back the
  message it used to lose.
- **On an ESP32-S3, 2.06-3.73x faster than `esp-alloc`** across four
  allocate/free workloads, measured with a subtracted harness floor and
  checksums proving work parity (`docs/plans/small-metal.md` §2.15).
- The reclamation fixes above cost 2.2-7.9 % of allocation throughput on that
  board. **The host instruction counts in the README predate them** and are
  pending re-measurement; a scheduled `icount` CI job now regenerates them.
- `generic_collect`'s default is geometry-aware: upstream's 10,000 at the
  shipped geometry, 512 under `ra_small_profile`, chosen from a measured sweep
  rather than picked.

## [1.1.6](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v1.1.5...rusty_alloc-v1.1.6) - 2026-08-28

### Other

- *(wasm)* dissolve the segment tax — slice-aligned segments and a free-slice pool (F2)
- *(heap)* route every span-sized allocation in-segment — the segment tax, F1+F5

## [1.1.5](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v1.1.4...rusty_alloc-v1.1.5) - 2026-08-28

### Fixed

- *(arena)* keep the public Arena shape, harden the adoption tests, own the census
- *(wasm)* recycle freed segments — adopt-on-free arenas replace the no-op leak
