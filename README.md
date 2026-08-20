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

- **At-or-below mimalloc on instructions retired** on real programs under
  `LD_PRELOAD` (lua 0.97×, perl 1.00×, sqlite 1.00×), **2–15% under jemalloc**
  (lua 0.85×, perl 0.90×, sqlite 0.98×), and ~17% under glibc.
- **A double free aborts instead of corrupting.** Upstream mimalloc accepts it
  silently in release builds; we detect it on both the local and the
  cross-thread path and abort.
- **~150 of mimalloc's ~157 `mi_*` entry points**, gated against the C
  implementation as a differential oracle on every change.
- **Runs on WebAssembly** with no C toolchain and no emscripten.

> **Status: `1.0.0` — the API is frozen; changes follow semver from here.**
> Upgrading from 0.3.x or earlier is mandatory, not optional: 0.4.0 fixed
> three platform-independent use-after-frees, so **treat 0.3.2 and earlier as
> unsound on every target.**

## Performance (deterministic instruction counts)

Instructions retired under callgrind, x86-64 Linux, `LD_PRELOAD`, repeats to
4–6 significant figures. Real programs:

| workload | vs mimalloc | vs jemalloc | vs glibc |
|---|---:|---:|---:|
| lua | **0.97** | **0.85** | 0.83 |
| perl | **1.00** | **0.90** | 0.82 |
| sqlite | **1.00** | **0.98** | 0.99 |

jemalloc is 5.3.0; all four arms are the same neutral binary under `LD_PRELOAD`,
same callgrind method. Per-operation (`bench/opscan.sh` — **11 of 13 operations
at-or-below mimalloc**, one tied, batch 0.8% behind):

| op | ra/mi | op | ra/mi |
|---|---:|---|---:|
| small / med | 0.71 | batch lifo/fifo | 1.008 |
| big / large | 0.77 | aligned | 0.90 |
| realloc | 0.76 | mixed | 0.88 |
| huge | 0.01 | calloc | 0.82 |

**Why batch went from 0.99 to 1.008**, since a number moving the wrong way
deserves its reason in the open: ThreadSanitizer found a genuine data race on
the free fast path — a thread adopting an abandoned segment rewrites page
flags while another thread reads them to route a free. Making that byte
atomic (`Relaxed`) fixes it and costs **exactly one instruction**, because
LLVM will not fold an atomic load into a test's memory operand. Upstream
mimalloc reads the same flags non-atomically, does not pay the instruction,
and has the race. This is the same trade already made for double-free
detection: correctness over 1.7% of a synthetic microbenchmark.

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
deterministic instruction A/Bs against the C oracle. On top of that, the
allocator is validated against real workloads:

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

## Security

Audited against the `use-protection-please` 41-gate hardening standard —
**14 of 15 v1.0.0 gates met**. The one open gate, H-27, is the 30-day
continuous-fuzz soak: the nightly mechanism is live and the corpus is committed
as a floor; the soak completes 2026-09-19 and ships under a time-bound owner
waiver. The residual-risk register (R-001..R-005) is owner-accepted; both
release waivers (H-05 release overflow-checks, H-27 soak) are time-bounded. The
gate-by-gate table is at the bottom of this README.

Default build:

- **A double free aborts** instead of handing one block to two owners — on the
  owner and cross-thread paths both.
- **Memory-safe core:** `unsafe` isolated with a stated invariant per block,
  `undocumented_unsafe_blocks` and `unsafe_op_in_unsafe_fn` denied
  workspace-wide, Miri-clean over the whole target, a loom-verified cross-thread
  protocol.
- **Mitigations verified for efficacy, not just presence:** `tests/corruption.rs`
  poisons a real free list and requires SIGABRT (detected-and-refused), not
  SIGSEGV (followed the poisoned link) — a mitigation nobody has watched fire is
  a claim, not a defence.

Opt-in for hostile input: **`secure`** (encrypted free-list links + a
same-segment link bound; flat ~15 instr/alloc) and **`blockmap`** (a per-page
block-liveness map that closes R-005 — a forged link handed out as a live block
— off by default on cost).

Threat model: [docs/threat-model.md](docs/threat-model.md) · `unsafe` inventory:
[crates/rusty_alloc/UNSAFE.md](crates/rusty_alloc/UNSAFE.md) · reports:
[SECURITY.md](SECURITY.md) (private GitHub advisories, 3-business-day
acknowledgement).

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
Miri-clean; `debug_checks` for full invariant validation; `secure` for encrypted
free-list links with a same-segment link bound (flat ~15 instr/alloc); `blockmap`
for a per-page block-liveness map that closes the read-primitive residual R-005
(off by default on cost). Mitigations are tested for efficacy — the corruption
suite asserts the allocator *aborts* on a poisoned free list.

**Portability** — x86-64 and aarch64, Linux, macOS and Windows, plus
`wasm32-unknown-unknown` via `memory.grow`.

## Install

```toml
[dependencies]
rusty_alloc-api = "1.0"
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

**Tier** critical-path · **Audited** 2026-08-20 (survey) · **v1.0.0 gates** 14/15 · [Full checklist](docs/plans/use-protection-please.md)

`██████████████████░░` **94%** &nbsp;·&nbsp; 33 Completed · 1 Scheduled · 1 Incomplete · 20 N/A

| Phase | ✅ Completed | 🗓 Scheduled | ⬜ Incomplete | · N/A |
|---|--:|--:|--:|--:|
| 0 — Threat modeling | 2 | 0 | 0 | 0 |
| 1 — Toolchain | 4 | 0 | 0 | 0 |
| 2 — Supply chain | 8 | 0 | 0 | 0 |
| 3 — Code level | 6 | 0 | 0 | 1 |
| 4 — Static analysis | 1 | 0 | 0 | 0 |
| 5 — Dynamic analysis | 3 | 0 | 0 | 0 |
| 6 — Fuzzing and properties | 3 | 1 | 0 | 0 |
| 7 — Formal verification | 1 | 0 | 0 | 0 |
| 8 — Build and binary | 0 | 0 | 0 | 2 |
| 9 — Runtime privilege | 0 | 0 | 0 | 1 |
| 10 — Cryptography | 1 | 0 | 0 | 2 |
| 11 — CI/CD, release, and operations | 4 | 0 | 1 | 0 |
| 12 — Compliance controls | 0 | 0 | 0 | 14 |
| **Total** | **33** | **1** | **1** | **20** |

**Next up** — H-27 Continuous fuzzing with no open crashes (2026-09-19 (30 days from the nightly job's first run))

**Architect** — Tim — Mata Network
<!-- HARDENING-TABLE:END -->
