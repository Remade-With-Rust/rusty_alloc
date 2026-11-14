# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.6.0](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v2.5.0...rusty_alloc-api-v2.6.0) - 2026-11-13

### Added

- **`core::alloc::Allocator` for `RustyAlloc` and `Heap`**, behind the new
  `allocator-api` feature (Rust 1.100, where the trait became stable).
  `Vec::new_in(RustyAlloc)` puts one collection on this allocator without
  replacing the program's global one; `Vec::new_in(&heap)` and
  `Box::new_in(x, &heap)` put standard collections in a first-class heap
  with no unsafe code, and the borrow stops them outliving it, which is
  the safe form of a destroyable heap that this crate's docs promised.
  Zero-sized layouts never reach the allocator, `grow` and `shrink`
  accept a changed alignment, a failed `grow` leaves the old block as it
  was, and no method unwinds: each runs under a guard that aborts.
  `tests/allocator_conformance.rs` runs every check on the global handle,
  both kinds of heap and the standard library's `System` as a control
  arm, natively, on a 32-bit target and under miri; twelve deliberate
  breakages of the impls were each caught by it, and six of them are now
  permanent cases in `tools/gate-selftest.sh`. Measured on the same
  collection work (`docs/opps.md`, "The `Allocator` route"):
  `Vec::new_in(RustyAlloc)` costs 2,535.48 instructions per step where
  the global allocator costs 2,533.37, and a heap 2,706.95.
- `RustyAlloc` is `Clone`, `Copy`, `Debug` and `Default`. It has no state;
  a collection with an allocator parameter needs `Clone` on it to be
  cloned.
- `Heap::contains(ptr)`: whether an address lies in memory the heap owns
  (`mi_heap_contains_block`). A diagnostic that answers by segment; the
  test that a reallocated block stays in its heap is written with it.

### Changed

- **A first-class heap is as fast as the global handle.** `Heap::alloc`,
  `alloc_zeroed` and `dealloc` are `#[inline]` and take the allocator's
  inline fast paths; a `realloc` inside a heap takes the tuned body the
  global one has. They went through the general entry points before, two
  calls an operation. On the same collection work a `Vec` in a heap costs
  2,524.31 instructions per step where it cost 2,706.95, against 2,533.37
  on the global allocator: 6.9 % behind became 0.36 % ahead. Nothing else
  moves (`docs/opps.md`, "The `Allocator` route").

### Fixed

- **`Heap::alloc` could return a misaligned block on a 32-bit target.**
  The alignment test was written `<= 8`. A one-word block is four bytes
  there and only four-aligned, so an eight-aligned layout of four bytes
  or fewer could land on an odd word. The three tiers are now
  `GlobalAlloc::alloc`'s, stated in words: up to one word, up to two
  (where the request is raised to two words, which the size classes
  align), and above. 64-bit targets were correct and gain the
  size-class path for a 16-byte alignment, which went through the
  aligned allocator before.
- **`Heap::new_destroyable`: dropping the heap could unmap another thread's
  live blocks.** A destroyable heap adopted the segments dead threads left
  behind, and its teardown released them with whatever was still in use
  inside. Fixed in `rusty_alloc` 2.6.0, whose changelog has the account: a
  heap that can be destroyed never adopts.
- **The crates now declare their minimum Rust version.** The workspace
  set `rust-version = "1.88"` but no crate inherited it, so both
  published crates carried none. CI now builds on 1.88.

## [2.5.0](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v2.4.0...rusty_alloc-api-v2.5.0) - 2026-11-12

### Changed

- **`GlobalAlloc::realloc` lands on a body that takes its arguments where
  `__rust_realloc` holds them** (`alloc::realloc_rust`): a word-aligned
  layout is one compare and a jump, 8 -> 5 instructions at the entry. An
  over-aligned `realloc` that stays in place no longer pushes five
  registers: the in-place answer is frameless, and the move is an
  out-of-line jump with the four arguments in place.
- Picks up `rusty_alloc` 2.5.0: allocator instructions perl -15.0 %,
  sqlite -15.2 %, lua -12.6 % against 2.4.0; Rust workloads boxed trees
  -5.2 %, short-lived threads -7.5 %, cache-line-aligned buffers -6.1 %.
  The core's changelog has the itemised list and the new opt-in
  `--cfg ra_tls_local_exec` (one instruction fewer per allocation in an
  executable).

## [2.4.0](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v2.3.1...rusty_alloc-api-v2.4.0) - 2026-11-01

### Changed

- Picks up `rusty_alloc` 2.4.0: purging is on by default (`purge_delay`
  10 ms, now a real delay), a page pinned by one live block gives back its
  free slots on collect, and a thread no longer costs 32 MiB of Windows
  commit charge. The core's changelog has the numbers. This crate's code is
  unchanged.

## [2.3.1](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v2.3.0...rusty_alloc-api-v2.3.1) - 2026-10-31

### Fixed

- Picks up `rusty_alloc` 2.3.1: the default arena is reserved uncommitted
  and committed a 32 MiB chunk at a time, so a process no longer charges
  1 GiB of commit on Windows or fails the overcommit check on a ~1 GiB Linux
  machine. This crate's code is unchanged.

## [2.3.0](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v2.2.7...rusty_alloc-api-v2.3.0) - 2026-10-30

### Performance

- `GlobalAlloc::alloc` moves its natural-alignment arm out of line, so the
  inlined method carries one malloc fast path, not two (maps -0.33 %,
  overaligned -1.81 %); over-aligned `realloc` inlines its usable-size
  query (-2.06 %); `dealloc` uses `free_inline_flags_first`. The core's
  changelog has the rest.

## [2.2.7](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v2.2.6...rusty_alloc-api-v2.2.7) - 2026-10-24

### Fixed

- Picks up `rusty_alloc` 2.2.7: a multithreaded process no longer hangs in
  `ExitProcess` on Windows when a thread was killed holding an allocator
  lock. This crate's code is unchanged.

## [2.2.6](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v2.2.5...rusty_alloc-api-v2.2.6) - 2026-10-09

### Fixed

- Picks up `rusty_alloc` 2.2.6: a merged free span no longer hides a
  decommitted neighbour (Windows, purging on). This crate's code is
  unchanged.

## [2.2.5](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v2.2.4...rusty_alloc-api-v2.2.5) - 2026-10-08

### Fixed

- **2.2.1–2.2.4 failed to build in an MSVC consumer that unwinds, with LTO**
  ("Bogus funclet pad use"): `GlobalAlloc::dealloc` inlined the core's
  `asm!` goto into EH cleanup funclets. Fixed in `rusty_alloc` (see its
  changelog); this crate's code is unchanged.

## [2.2.1](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v2.2.0...rusty_alloc-api-v2.2.1) - 2026-09-25

### Performance

- **`GlobalAlloc` methods are `#[inline]`**, as the `mimalloc` crate's are, so
  rustc's `__rust_alloc` / `__rust_dealloc` shims carry the fast path instead
  of jumping to it, and `dealloc` is the free fast path with its null test
  folded away. Measured whole-program against 2.2.0 on two deterministic Rust
  workloads (`bench/rust-globalalloc.sh`): **-21.1 %** and **-5.8 %**.
- **Layouts aligned up to two words (16 bytes on 64-bit) come from the natural
  size classes**, which are already aligned that far, instead of the aligned
  path; every hashbrown table is such a layout. `tests/natural_align.rs` pins
  the alignment for every size class through alloc, alloc_zeroed and realloc.
- **`realloc` keeps a block in place at any alignment when it fits** instead
  of always allocating, copying and freeing above 8 bytes of alignment:
  **-27.3 %** on an over-aligned buffer workload.

## [2.2.0](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v2.1.0...rusty_alloc-api-v2.2.0) - 2026-09-10

### Other

- release v2.2.0

## [2.1.0](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v2.0.5...rusty_alloc-api-v2.1.0) - 2026-09-10

### Other

- release v2.1.0

## [2.0.5](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v2.0.4...rusty_alloc-api-v2.0.5) - 2026-09-09

### Other

- release v2.0.5

## [2.0.0](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v1.1.6...rusty_alloc-api-v2.0.0) - 2026-09-07

### Breaking

- **`default-features = false` now selects `no_std`.** This crate's own source
  has been `no_std` since M2; only the core dependency's default features stood
  in the way. `default = ["std"]` is new, and turning it off gives a firmware
  the single-heap profile — which also requires `--cfg ra_single_threaded` on
  the core. Consumers on default features are unaffected.

### Added

- `std` feature (default-on) forwarding to `rusty_alloc/std`, so
  `RustyAlloc` can be a bare-metal `#[global_allocator]`.

## [1.1.6](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v1.1.5...rusty_alloc-api-v1.1.6) - 2026-08-28

### Other

- release v1.1.6 ([#11](https://github.com/Remade-With-Rust/rusty_alloc/pull/11))
