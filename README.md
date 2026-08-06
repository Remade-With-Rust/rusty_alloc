# rusty_alloc

A pure-Rust remake of [mimalloc](https://github.com/microsoft/mimalloc) — the
v2.4.5 architecture (32 MiB segments, free-list-sharded pages, lock-free
cross-thread frees), rebuilt from the design rather than transliterated from the
C. Built under the [remade-with-rust](https://github.com/remade-with-rust)
principles: memory safety first, no C in the product, general primitives,
**measured, not vibed**.

> **Status: `0.1.0-alpha.1`.** The allocator is complete and gated. The
> performance *evidence* is not. Read
> [What is and isn't measured](#what-is-and-isnt-measured) before depending on
> this — the short version is that we count instructions, not seconds, and we
> make no speed claim.

## What it is

- **~150 of mimalloc's ~157 `mi_*` entry points**, semantics-for-semantics,
  gated against the C implementation as a differential oracle.
- **A double free is detected and aborted, not silently accepted.** Upstream
  mimalloc does not detect this in release builds. We do, at a measured cost of
  ~0.4% on perl and ~0.2% on sqlite. Silently handing the same block to two
  owners is the exact failure this project exists to prevent, so the check is
  the point rather than an overhead.
- **No C anywhere in the product.** The C mimalloc in this repo is a
  development-only oracle: never a dependency, never published.
- **Runs on WebAssembly** (`wasm32-unknown-unknown`) with no C toolchain and no
  emscripten — `memory.grow`, single linear memory, gated by a self-test that
  executes inside a real VM.

## What is and isn't measured

### Measured — instructions retired

Deterministic (callgrind), x86-64 Linux, `LD_PRELOAD` against the real programs:

| workload | vs mimalloc | vs glibc |
|---|---:|---:|
| lua | 0.99 | 0.84 |
| perl | 1.01 | 0.83 |
| sqlite | 1.00 | 1.00 |

Parity with mimalloc; roughly 16% fewer instructions than glibc. These repeat to
4–6 significant figures, so they are claims we can defend.

### NOT measured — and therefore not claimed

- **Wall-clock time.** See [Timing](#timing) below. We have run it; this machine
  cannot resolve a difference this small, and we are not going to launder that
  into a speed claim.
- **The full mimalloc-bench corpus.** Three workloads, not the suite. The
  project's own v1 gate (geomean within 10%, no bench >25% behind, RSS within
  15%) is **not yet demonstrated**.
- **RSS.** No systematic footprint sweep.
- **aarch64.** The code paths exist and compile. They have never been executed.

There is no "faster than mimalloc" claim anywhere in this repository, because
the evidence for one does not exist yet.

### Timing

`bench/wallclock.sh` runs pinned, ABBA-interleaved, microsecond-resolution
comparisons at N=31 with a **null arm** — the same allocator against itself.
The null arm is the floor: any delta smaller than it is noise, not a result.

Measured on the development machine (N=31, pinned, microsecond timer):

| arm | median ratio |
|---|---:|
| **null (rusty_alloc vs itself)** | **1.0117** |
| perl, rusty_alloc vs mimalloc | 1.0009 |
| sqlite, rusty_alloc vs mimalloc | 1.0091 |

The null arm is **1.17%** — wider than either measured effect. The same
allocator compared against itself differs by more than the difference we are
trying to detect, so the only honest reading is **"at parity, below measurement
resolution"**. That is not a hedge; it is what the instrument supports.
Reproduce it on a quiet box:

```sh
N=31 bash bench/wallclock.sh
```

## Layout

| path | what |
|---|---|
| `crates/rusty_alloc` | allocator core — **published** |
| `crates/rusty_alloc_api` | safe Rust surface (`GlobalAlloc`, `Heap`, `Allocator`) — **published** |
| `crates/rusty_alloc_ffi` | `mi_*`-compatible C ABI (cdylib + staticlib) |
| `crates/rusty_alloc_override` | `malloc`/`free` interposition cdylib (LD_PRELOAD arm) |
| `crates/rusty_alloc_bench` | Tier-B harness + trace record/replay |
| `crates/rusty_alloc_wasm` | wasm self-test fixture |
| `oracle/mimalloc` | C mimalloc @ v2.4.5 — **dev-only oracle**, never a runtime dep |
| `corpus/mimalloc-bench` | the 1:1 benchmark corpus (submodule) |
| `docs/LEDGER.md` | one entry per milestone: numbers, method, and every revert |
| `docs/plans/rusty_alloc_v1.md` | plan of record — API inventory, gate ladder, roadmap |

Only the two crates marked **published** go to crates.io. Everything else is
`publish = false`: harnesses, fixtures and native artifacts, not libraries.

## Gates

Every change runs: Windows + Linux test suites (all features), `clippy -D
warnings`, Miri, a 640-thread churn probe, a wasm VM self-test, and a
deterministic instruction-count A/B against the C oracle. `docs/LEDGER.md`
records what each milestone measured — **including the changes that were
reverted for being flat or slower**, which is most of them.

## Building the oracle (contributors)

```sh
git submodule update --init oracle/mimalloc corpus/mimalloc-bench
bash oracle/build.sh          # builds mi / dmi / smi arms
```

## License

MIT — see [LICENSE](LICENSE). Vendored dev-only dependencies keep their own
licenses; none of them ship.
