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

- **Parity with mimalloc on instructions retired**, and ~16% fewer than glibc,
  on real programs under `LD_PRELOAD`.
- **A double free aborts instead of corrupting.** Upstream mimalloc accepts it
  silently in release builds; we detect it on both the local and the
  cross-thread path and abort, for a measured ~0.4%.
- **~150 of mimalloc's ~157 `mi_*` entry points**, gated against the C
  implementation as a differential oracle on every change.
- **Runs on WebAssembly** with no C toolchain and no emscripten.

> **Status: `0.3.2` — a 0.x release. The API is not frozen, and parts of the
> performance evidence are still missing.** See
> [What is and isn't measured](#what-is-and-isnt-measured) — we count
> instructions, not seconds, and make no speed claim.

## Performance (this machine, deterministic)

Instructions retired under callgrind, x86-64 Linux, real programs via
`LD_PRELOAD`. Repeats to 4–6 significant figures.

| workload | vs mimalloc | vs glibc |
|---|---:|---:|
| lua | **0.99** | 0.84 |
| perl | **1.01** | 0.83 |
| sqlite | **1.00** | 1.00 |

**Method.** `bench/icount-arms.sh`. Instruction counts, not wall-clock — chosen
deliberately, because this machine's timing noise floor is wider than the
effect (see below). Counts are immune to scheduler, thermal and load artifacts;
they are also *not* a measure of time.

### What is and isn't measured

**Wall-clock: measured, and it cannot resolve the difference.**
`bench/wallclock.sh` runs pinned, ABBA-interleaved, N=31, microsecond timer,
with a **null arm** — the same allocator compared against itself:

| arm | median ratio |
|---|---:|
| **null (rusty_alloc vs ITSELF)** | **1.0117** |
| perl, rusty_alloc vs mimalloc | 1.0009 |
| sqlite, rusty_alloc vs mimalloc | 1.0091 |

The null arm is **1.17%** — wider than either effect. The honest reading is
**"at parity, below measurement resolution"**. Reproduce on a quiet box with
`N=31 bash bench/wallclock.sh`.

**Scope that claim to these workloads.** A 0.4.0 re-run against the same
mimalloc oracle, but on a pure allocation-CHURN microbenchmark (all arms
`LD_PRELOAD`ed into one neutral C binary, instructions retired), reads
**1.1342 × mimalloc** — 13.4% behind — while still being **0.67 ×** glibc.
Real programs dilute allocator cost among everything else they do; a loop that
does almost nothing but `malloc`/`free` does not. "At parity" is measured on
lua/perl/sqlite and should not be read as a general property.

**Not measured, and therefore not claimed:**

- The full mimalloc-bench corpus. Three workloads, not the suite — the
  project's own v1 gate (geomean within 10%, no bench >25% behind, RSS within
  15%) is **not yet demonstrated**.
- RSS. No systematic footprint sweep. Three things *were* measured in 0.4.0 on
  aarch64-apple-darwin, and one of them is a caveat, not a win:
  - The decommit primitive now returns **100.1%** of touched pages to the OS
    (it returned **6.4%** before the fix — `MADV_DONTNEED` is advisory-only for
    anonymous memory on Darwin).
  - **With purging enabled** (`purge_delay = 0`), a 6-minute thread-churn soak
    held RSS flat at **9.4 MiB**, slope −0.02 MiB/min.
  - **With the shipped default** (`purge_delay = -1`, purging OPT-IN), a
    25-minute soak against a *bounded* live set (~175 MiB mean) sat at ~650 MiB
    RSS and drifted **+1.45 ± 0.70 MiB/min** over the full run. The drift
    decelerates (first half +2.42, second half +1.19 ± 1.87 — no longer
    distinguishable from zero) and RSS does not track the live set
    (corr = +0.03), which is consistent with retention approaching a plateau
    rather than an unbounded leak — but **25 minutes cannot tell those two
    apart**, and this is not claimed to be settled.
- Long-running behaviour beyond ~25 minutes. Multi-day fragmentation is unknown.
  Long-lived services should set `purge_delay >= 0` rather than rely on the
  opt-in default.

**aarch64 is now executed** (0.4.0). It was not, through 0.3.2, and that first
execution cost seven defects — two of them memory-safety class, including a
`thread_id()` that read the wrong system register on Darwin and let distinct
threads collide onto one ownership id. See the 2026-08-08 entries in
[docs/LEDGER.md](docs/LEDGER.md). Three of the seven were platform-independent
use-after-frees that x86-64 had been surviving by luck, so **0.3.2 and earlier
should be treated as unsound on every target**, not merely on aarch64.

There is no "faster than mimalloc" claim anywhere in this repository, because
the evidence for one does not exist yet.

## What is this?

A reimplementation, not a binding. There are excellent mimalloc *bindings* for
Rust; this is not one of them. Every line of the allocator is Rust, the C
mimalloc in this repository is a development-only differential oracle, and it
is never a dependency and never published.

`unsafe` is confined to the places an allocator genuinely needs it — the OS
primitive layer, page and segment metadata, and the lock-free cross-thread
protocol — with a stated invariant on every block, `unsafe_op_in_unsafe_fn`
denied and `undocumented_unsafe_blocks` denied workspace-wide.

## The Remade With Rust ecosystem

| project | what |
|---|---|
| [rusty_alloc](https://github.com/remade-with-rust/rusty_alloc) | this — pure-Rust general-purpose allocator |
| [rusty_h264](https://github.com/remade-with-rust/rusty_h264) | pure-Rust H.264 encoder and decoder |
| [Mata Network](https://www.mata.network/) | the parent organisation |

## Features

**Allocator core** — 32 MiB segments sliced into 64 KiB spans, free-list-sharded
pages, the loom-verified four-state cross-thread free protocol, thread
abandonment and adoption, first-class heaps, arenas, huge allocations, aligned
allocation with interior-pointer recovery, and the full realloc family.

**Safety** — double-free detection on both the owner and cross-thread paths;
Miri-clean; a 640-thread churn probe; `debug_checks` for full invariant
validation; `secure` for guard pages, encrypted free lists and guarded-object
sampling.

**Portability** — x86-64 and aarch64, Linux and Windows, plus
`wasm32-unknown-unknown` via `memory.grow`.

## Install

```toml
[dependencies]
rusty_alloc-api = "0.3"
```

| crate | docs | what |
|---|---|---|
| [`rusty_alloc`](https://crates.io/crates/rusty_alloc) | [docs.rs](https://docs.rs/rusty_alloc) | allocator core |
| [`rusty_alloc-api`](https://crates.io/crates/rusty_alloc-api) | [docs.rs](https://docs.rs/rusty_alloc-api) | safe Rust surface — start here |

The FFI, LD_PRELOAD override, bench and wasm crates are `publish = false`:
harnesses, fixtures and native artifacts, not libraries.

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
docs/plans/rusty_alloc_v1.md  plan of record — API inventory, gate ladder
```

## Benchmarking

```sh
git submodule update --init oracle/mimalloc corpus/mimalloc-bench
bash oracle/build.sh                 # build the C oracle arms
bash bench/icount-arms.sh            # deterministic instruction A/B
N=31 bash bench/wallclock.sh         # wall-clock, with a null arm
bash bench/opscan.sh                 # per-operation scan vs mimalloc
```

## Gates

Every change runs Windows + Linux suites (all features), `clippy -D warnings`,
Miri, a 640-thread churn probe, a wasm VM self-test, and a deterministic
instruction A/B against the C oracle. [`docs/LEDGER.md`](docs/LEDGER.md)
records what each milestone measured — **including the changes reverted for
being flat or slower**, which is most of them.

## Platform support

| target | status |
|---|---|
| x86-64 Linux | tested; the LD_PRELOAD and measurement path |
| x86-64 Windows | compiles; last executed before the 0.4.0 fixes |
| aarch64 macOS (Apple Silicon, 16 KiB pages) | **tested** — full suite, 100/100 stress soak, `#[global_allocator]` app |
| aarch64 Linux | **tested** — full suite, 40/40 stress soak |
| wasm32-unknown-unknown | **executed** — `bench/wasm-selftest.mjs` passes in a Node VM; not exercised in a browser |

## License

MIT — see [LICENSE](LICENSE). No GPL or LGPL anywhere in the tree. The vendored
oracle and benchmark corpus are development-only, keep their own licences, and
never ship.

## About Mata Network

rusty_alloc is part of the [remade-with-rust](https://github.com/remade-with-rust)
portfolio from [Mata Network](https://www.mata.network/): foundational software
rebuilt in Rust, memory-safe by construction, measured rather than asserted.
