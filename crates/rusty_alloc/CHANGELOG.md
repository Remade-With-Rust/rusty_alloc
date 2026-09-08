# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.0.0](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v1.1.6...rusty_alloc-v2.0.0) - 2026-09-07

### Breaking

Two changes require a major bump. Neither affects a consumer using default
features, which is the overwhelming majority.

- **`default-features = false` now selects `no_std`.** Before this release the
  crate had `default = []`, so `default-features = false` was identical to the
  default and got the full crate. It now selects the single-heap `no_std`
  profile, which additionally refuses to compile without
  `--cfg ra_single_threaded`. If you set `default-features = false` and want
  what you had, set `features = ["std"]`.
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
