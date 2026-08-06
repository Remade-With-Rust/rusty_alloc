# rusty_alloc

A pure-Rust remake of [mimalloc](https://github.com/microsoft/mimalloc) — the
v2.4.5 architecture (32 MiB segments, free-list-sharded pages, lock-free
cross-thread frees), rebuilt from the design rather than transliterated from
the C.

**Status: `0.3.0` — a 0.x release.** The API is not frozen; the performance
evidence is not yet complete. Read [What is and isn't
measured](#what-is-and-isnt-measured) before depending on it.

## What it is

- **~150 of mimalloc's ~157 `mi_*` entry points**, semantics-for-semantics.
- **Safe by default where it counts.** A double free is *detected and aborted*,
  not silently accepted. Upstream mimalloc does not detect this in release
  builds; we chose to, at a measured cost of ~0.4% on perl and ~0.2% on sqlite.
  Handing the same block to two owners is the failure this project exists to
  prevent, so paying for the check is the point.
- **No C anywhere in the product.** The C mimalloc in this repository is a
  development-only differential oracle; it is never a dependency and is never
  published.
- **Runs on WebAssembly** (`wasm32-unknown-unknown`) via `memory.grow`, with no
  C toolchain and no emscripten.

## What is and isn't measured

**Measured**, deterministically, via callgrind instructions retired on x86-64
Linux under `LD_PRELOAD`:

| workload | vs mimalloc | vs glibc |
|---|---:|---:|
| lua | 0.99 | 0.84 |
| perl | 1.01 | 0.83 |
| sqlite | 1.00 | 1.00 |

That is parity with mimalloc and roughly 16% fewer instructions than glibc.

**NOT measured, and therefore not claimed:**

- **Wall-clock time.** Every number above counts instructions. Instructions and
  time are not the same thing, and we have not established that this build is
  *faster* in seconds.
- **The full mimalloc-bench corpus.** Three workloads, not the suite.
- **RSS.** No systematic memory-footprint sweep.
- **aarch64.** Code paths exist and compile; they have never been executed.

There is no "faster than mimalloc" claim here, because the evidence for one
does not exist yet.

## Usage

This crate is the allocator core. For the ergonomic Rust surface
(`GlobalAlloc`, first-class `Heap`, the `Allocator` trait), use
[`rusty_alloc_api`](https://crates.io/crates/rusty_alloc_api).

## Features

| feature | what |
|---|---|
| `debug_checks` | full invariant validation: list walks, span tiling, page canaries |
| `secure` | guard pages, encrypted free lists, guarded-object sampling |
| `profile` | feature-gated path profiler |

Statistics counters follow upstream's `MI_STAT` rule: present in debug builds,
compiled out of release.

## License

MIT. See `LICENSE` at the repository root.
