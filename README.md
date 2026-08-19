[![crates.io](https://img.shields.io/crates/v/rusty_alloc.svg)](https://crates.io/crates/rusty_alloc)
[![docs.rs](https://img.shields.io/docsrs/rusty_alloc)](https://docs.rs/rusty_alloc)
[![CI](https://github.com/remade-with-rust/rusty_alloc/actions/workflows/ci.yml/badge.svg)](https://github.com/remade-with-rust/rusty_alloc/actions)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![remade with rust](https://img.shields.io/badge/remade--with--rust-portfolio-orange.svg)](https://github.com/remade-with-rust)

# rusty_alloc

A ground-up, pure-**Rust** general-purpose **allocator** — the mimalloc v2.4.5
architecture rebuilt from the design rather than transliterated from the C. No
C in the dependency tree, permissive licence, and a safety property upstream
does not offer.

## ⚡ The headline

- **At-or-below mimalloc on instructions retired** — on real programs under
  `LD_PRELOAD` *and* on every operation of the per-op scan — and ~17% fewer
  than glibc.
- **A double free aborts instead of corrupting.** Upstream mimalloc accepts it
  silently in release builds; we detect it on both the local and the
  cross-thread path and abort.
- **~150 of mimalloc's ~157 `mi_*` entry points**, gated against the C
  implementation as a differential oracle on every change.
- **Runs on WebAssembly** with no C toolchain and no emscripten.

> **Status: `0.7.0` — a 0.x release; the API is not frozen.**
> Upgrading from 0.3.x or earlier is mandatory, not optional: 0.4.0 fixed
> three platform-independent use-after-frees, so **treat 0.3.2 and earlier as
> unsound on every target.**

## Performance (deterministic instruction counts)

Instructions retired under callgrind, x86-64 Linux, `LD_PRELOAD`, repeats to
4–6 significant figures. Real programs:

| workload | vs mimalloc | vs glibc |
|---|---:|---:|
| lua | **0.98** | 0.83 |
| perl | **1.00** | 0.82 |
| sqlite | **1.00** | 0.99 |

Per-operation (`bench/opscan.sh`, one neutral C driver, all arms preloaded —
**all 13 operations measure at-or-below mimalloc**):

| op | ra/mi | op | ra/mi |
|---|---:|---|---:|
| small / med | 0.70 | batch lifo/fifo | 0.99 |
| big / large | 0.77 | aligned | 0.87 |
| realloc | 0.78 | mixed | 0.89 |
| huge | 0.01 | calloc | 0.94 |

**These are counts, not seconds.** Wall-clock cannot be resolved on the
development machine (the null arm — the same allocator against itself — reads
±1.2%, wider than the effect), so no wall-clock speed claim is made anywhere
in this repository. Reproduce with `bash bench/icount-arms.sh` and
`bash bench/opscan.sh`.

**RSS:** long-lived services should set `purge_delay >= 0` — that is the
configuration with flat, measured RSS (a 6-minute thread-churn soak held
9.4 MiB, slope −0.02 MiB/min). The shipped default leaves purging opt-in.

## Correctness evidence

Every change runs Windows + Linux suites (all features), `clippy -D warnings`,
Miri over the whole target, a 640-thread churn probe, a wasm VM self-test, and
deterministic instruction A/Bs against the C oracle. On top of that, 0.7.0 was
validated against real workloads:

- **Real programs, byte-identical output:** jq, sqlite3, python3, git, xz,
  zstd, lua and perl each produce bit-for-bit identical output under
  rusty_alloc, mimalloc and glibc — 144/144 runs across three interleaved
  passes (`corpus/realworld.sh`).
- **The full mimalloc-bench corpus runs clean:** 19/19 benchmark
  configurations — including the 8–16-thread storms (larson, mstress, rptest,
  xmalloc-test, sh6/sh8bench) — complete under rusty_alloc
  (`corpus/sweep-all.sh`).
- **Release `stress_mt` soak 30/30**; Miri-clean including the multithreaded
  abandon/adopt storm.
- Tested on x86-64 and aarch64 Linux, aarch64 and x86-64 macOS, x86-64
  Windows, and executed on `wasm32-unknown-unknown`; consumed as
  `#[global_allocator]` by shipping codec projects with byte-identical output
  before and after the allocator swap.

[`docs/LEDGER.md`](docs/LEDGER.md) records what every milestone measured —
including the changes reverted for being flat or slower.

## What is this?

A reimplementation, not a binding. Every line of the allocator is Rust; the C
mimalloc in this repository is a development-only differential oracle, never a
dependency, never published.

`unsafe` is confined to the places an allocator genuinely needs it — the OS
primitive layer, page and segment metadata, and the lock-free cross-thread
protocol — with a stated invariant on every block, `unsafe_op_in_unsafe_fn`
denied and `undocumented_unsafe_blocks` denied workspace-wide.

## Features

**Allocator core** — 32 MiB segments sliced into 64 KiB spans, free-list-sharded
pages, the loom-verified four-state cross-thread free protocol, thread
abandonment and adoption, first-class heaps, arenas, huge allocations, aligned
allocation with interior-pointer recovery, and the full realloc family.

**Safety** — double-free detection on both the owner and cross-thread paths;
Miri-clean; `debug_checks` for full invariant validation; `secure` for guard
pages, encrypted free lists and guarded-object sampling (measured cost 4–7%).

**Portability** — x86-64 and aarch64, Linux, macOS and Windows, plus
`wasm32-unknown-unknown` via `memory.grow`.

## Install

```toml
[dependencies]
rusty_alloc-api = "0.7"
```

| crate | docs | what |
|---|---|---|
| [`rusty_alloc`](https://crates.io/crates/rusty_alloc) | [docs.rs](https://docs.rs/rusty_alloc) | allocator core |
| [`rusty_alloc-api`](https://crates.io/crates/rusty_alloc-api) | [docs.rs](https://docs.rs/rusty_alloc-api) | safe Rust surface — start here |

## Quick start

```rust
use rusty_alloc_api::RustyAlloc;

#[global_allocator]
static ALLOC: RustyAlloc = RustyAlloc;

fn main() {
    let v: Vec<u64> = (0..1_000).collect();
    println!("{}", v.iter().sum::<u64>());
}
```

## Architecture

```
crates/rusty_alloc            allocator core       (published)
crates/rusty_alloc_api        safe Rust surface    (published)
crates/rusty_alloc_ffi        mi_*-compatible C ABI
crates/rusty_alloc_override   malloc/free interposition cdylib
crates/rusty_alloc_bench      Tier-B harness + trace record/replay
crates/rusty_alloc_wasm       wasm self-test fixture
oracle/mimalloc               C mimalloc @ v2.4.5 — dev-only oracle
corpus/mimalloc-bench         the 1:1 benchmark corpus
docs/LEDGER.md                one entry per milestone: numbers, method, reverts
```

## Benchmarking

```sh
git submodule update --init oracle/mimalloc corpus/mimalloc-bench
bash oracle/build.sh                 # build the C oracle arms
bash bench/icount-arms.sh            # deterministic instruction A/B
bash bench/opscan.sh                 # per-operation scan vs mimalloc
bash corpus/sweep-all.sh             # full-corpus correctness sweep
bash corpus/realworld.sh             # real programs, checksummed, 3 arms
```

Note for anyone running the test suite: use a debug build — the allocation
counters behind `alloc::stats()` are `#[cfg(debug_assertions)]`, matching
upstream's `MI_STAT` rule.

## License

MIT — see [LICENSE](LICENSE). No GPL or LGPL anywhere in the tree. The vendored
oracle and benchmark corpus are development-only, keep their own licences, and
never ship.

## About Mata Network

rusty_alloc is part of the [remade-with-rust](https://github.com/remade-with-rust)
portfolio from [Mata Network](https://www.mata.network/): foundational software
rebuilt in Rust, memory-safe by construction, measured rather than asserted.

---

<!-- HARDENING-TABLE:BEGIN generated by use-protection-please — edit docs/plans/use-protection-please.md, not this block -->
## Hardening status

**Tier** critical-path · **Audited** 2026-08-19 (survey) · **v1.0.0 gates** 1/13 · [Full checklist](docs/plans/use-protection-please.md)

`█░░░░░░░░░░░░░░░░░░░` **7%** &nbsp;·&nbsp; 2 Completed · 0 Scheduled · 28 Incomplete · 25 N/A

| Phase | ✅ Completed | 🗓 Scheduled | ⬜ Incomplete | · N/A |
|---|--:|--:|--:|--:|
| 0 — Threat modeling | 0 | 0 | 2 | 0 |
| 1 — Toolchain | 0 | 0 | 4 | 0 |
| 2 — Supply chain | 0 | 0 | 8 | 0 |
| 3 — Code level | 0 | 0 | 1 | 6 |
| 4 — Static analysis | 0 | 0 | 1 | 0 |
| 5 — Dynamic analysis | 1 | 0 | 2 | 0 |
| 6 — Fuzzing and properties | 1 | 0 | 3 | 0 |
| 7 — Formal verification | 0 | 0 | 1 | 0 |
| 8 — Build and binary | 0 | 0 | 0 | 2 |
| 9 — Runtime privilege | 0 | 0 | 0 | 1 |
| 10 — Cryptography | 0 | 0 | 1 | 2 |
| 11 — CI/CD, release, and operations | 0 | 0 | 5 | 0 |
| 12 — Compliance controls | 0 | 0 | 0 | 14 |
| **Total** | **2** | **0** | **28** | **25** |

**Architect** — Tim — Mata Network
<!-- HARDENING-TABLE:END -->
