# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
