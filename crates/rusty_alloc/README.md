> **In the wild** — [RAG Converter](https://ragconverter.com) uses `rusty_alloc` as the allocator, in wasm too.
> It makes personal and work files AI-readable without them leaving the machine:
> the whole conversion runs as WebAssembly in the browser tab, with nothing
> uploaded and nothing to install.

# rusty_alloc

A pure-Rust remake of [mimalloc](https://github.com/microsoft/mimalloc) — the
v2.4.5 architecture (32 MiB segments, free-list-sharded pages, lock-free
cross-thread frees), rebuilt from the design rather than transliterated from
the C. No C anywhere in the product; the C mimalloc is a development-only
differential oracle and is never a dependency.

**Status: `1.0.0` — the API is frozen; changes follow semver from here.**

> **Upgrade from 0.3.x or earlier — mandatory.** 0.4.0 fixed three
> platform-independent use-after-frees on the abandon → adopt → reuse path.
> **Treat 0.3.2 and earlier as unsound on every target.**

Tested on x86-64/aarch64 Linux, aarch64/x86-64 macOS and x86-64 Windows;
executed on `wasm32-unknown-unknown` (Node VM self-test, no emscripten).

## What it is

- **~150 of mimalloc's ~157 `mi_*` entry points**, semantics-for-semantics,
  gated against the C implementation as a differential oracle.
- **A double free is detected and aborted**, on both the owner and
  cross-thread paths. Upstream mimalloc accepts it silently in release builds;
  handing the same block to two owners is the failure this project exists to
  prevent.
- **Runs on WebAssembly** via `memory.grow`, with no C toolchain.

## Performance (instruction counts, not seconds)

Measured deterministically via callgrind instructions retired, x86-64 Linux,
`LD_PRELOAD`. Side-by-side against the field (ratio = our instructions ÷
theirs; **lower is better**):

| workload | vs mimalloc | vs jemalloc | vs glibc |
|---|---:|---:|---:|
| lua | **0.97** | **0.84** | 0.82 |
| perl | **0.99** | **0.89** | 0.81 |
| sqlite | **1.00** | **0.98** | 0.99 |

We match mimalloc and come in **2–15% under jemalloc** (jemalloc 5.3.0) across
all three real programs, and ~17% under glibc.

The per-operation scan (small/med/big/large/huge, calloc, realloc, aligned,
usable, batch and mixed working-set ops) measures **below mimalloc on all 13
operations**. Per-op ratios vs mimalloc: med **0.47**, small **0.49**, aligned **0.50**,
realloc **0.51**, small_touch **0.51**, big/large **0.53**, calloc **0.63**,
mixed **0.67**, usable **0.73**, batch lifo/fifo **0.89**, huge **0.01**.

Five of those hold ONE BLOCK LIVE at a time, so the page empties on every free
and what they measure is page retire-and-recarve rather than steady-state
service. On workloads with a real working set — the honest number for a
program — it is `liveset` **0.92** (65,536 live objects, random victim replaced
each step), `shbench` **0.91** (bulk batches released in waves) and `xthread`
**0.83** (every free performed by a non-owning thread). Wall-clock time is deliberately not claimed: the measurement box
cannot resolve it above its own noise floor, and instructions are not seconds.

Batch is the narrowest margin, and it is the one with a story. It sat at
**1.008** after a ThreadSanitizer fix made a page-flags byte atomic, which
costs exactly one instruction because LLVM will not fold an atomic load into a
test's memory operand. That trade was kept — upstream reads the same byte
non-atomically and has the race — and the instruction was won back elsewhere on
the same path rather than by undoing it.

`free`'s fast path is **21 instructions against mimalloc's 25**, down from 27.
The `used--` codegen floor `docs/opps.md` recorded is closed: it took five
instructions from safe Rust because LLVM will not emit a memory-destination
read-modify-write when the value must also drive a branch, and it is two now,
written directly. Resolving a pointer to its page went from nine instructions
to five on the back of a per-slice owner table in the segment header. The flags
test is the single row where upstream is still cheaper, and it stays that way
deliberately: closing it would keep the read atomic in hardware while hiding it
from the sanitizer that found the race.

Correctness on real software: jq, sqlite3, python3, git, xz, zstd, lua and
perl produce **byte-identical output** under rusty_alloc, mimalloc and glibc;
the full mimalloc-bench corpus (19 configurations, including the 8–16-thread
storms) runs clean; Miri is clean over the whole target.

## Usage

This crate is the allocator core. For the ergonomic Rust surface
(`GlobalAlloc`, first-class `Heap`, the `Allocator` trait), use
[`rusty_alloc-api`](https://crates.io/crates/rusty_alloc-api).

Long-lived services should set `purge_delay >= 0` — the configuration with
flat, measured RSS. The shipped default leaves purging opt-in.

## Security

Audited against the `use-protection-please` 41-gate hardening standard —
**14 of 15 v1.0.0 gates met** (the one open gate, H-27, is the 30-day
continuous-fuzz soak: the nightly mechanism is live and clean, the soak
completes 2026-09-19, and it ships under a time-bound owner waiver). The
gate-by-gate table is at the bottom of this README; the residual-risk register
(R-001..R-005, owner-accepted) and the two time-bound waivers are in the
[full checklist](https://github.com/remade-with-rust/rusty_alloc/blob/main/crates/rusty_alloc/docs/plans/use-protection-please.md).

What the default build gives you:

- **A double free aborts** instead of putting a block on a free list twice —
  on both the owner and the cross-thread path.
- **Foreign-pointer detection** on `free` (debug / `debug_checks` builds), and
  a memory-safe core: `unsafe` isolated with a stated invariant on every block,
  `undocumented_unsafe_blocks` denied workspace-wide, Miri-clean.
- **Mitigations tested for EFFICACY, not just function** — `tests/corruption.rs`
  poisons a real free list and requires the process to die of SIGABRT (detected)
  rather than SIGSEGV (followed the poisoned pointer); a mitigation nobody has
  watched fire is a claim, not a defence.

Opt-in hardening for hostile-input services:

- **`secure`** — per-page encrypted free-list links plus a same-segment +
  alignment bound on every decoded link, so a link overwrite cannot steer the
  allocator to an out-of-heap target. Measured cost: a flat ~15 instructions
  per allocation (+0.6–1.8% whole-program).
- **`blockmap`** — a per-page block-liveness map that catches a forged link
  landing on an already-live block, the one thing that closes R-005 (encoding
  does not survive an attacker with a *read* primitive). Off by default on cost
  (~3× `secure`); switchable independently.

Reports go through [SECURITY.md](https://github.com/remade-with-rust/rusty_alloc/blob/main/SECURITY.md)
(private GitHub advisories). See the
[threat model](https://github.com/remade-with-rust/rusty_alloc/blob/main/docs/threat-model.md)
and the [`unsafe` inventory](https://github.com/remade-with-rust/rusty_alloc/blob/main/crates/rusty_alloc/UNSAFE.md).

## Features

| feature | what |
|---|---|
| `debug_checks` | full invariant validation: list walks, span tiling, page canaries |
| `secure` | encrypted free-list links + same-segment link bound; guard pages / guarded-object sampling available (opt-in via options). Flat ~15 instr/alloc (+0.6–1.8% whole-program) |
| `blockmap` | per-page block-liveness map — catches a forged link handed out as a live block (closes R-005). Off by default on cost (~3× `secure`) |
| `profile` | feature-gated path profiler |

Statistics counters follow upstream's `MI_STAT` rule: present in debug builds,
compiled out of release.

## The Remade With Rust ecosystem

<!-- ORG BOILERPLATE — keep identical across repos -->

**Remade With Rust** is an initiative by **[Mata Network](https://www.mata.network/)**
to rebuild essential C and C++ tools in Rust — for the memory safety, the
predictable performance, and the freedom of a permissive license. Each project
is a reimplementation, not a fork: same wire protocols and file formats, new
code you can actually depend on.

We build the core to production grade and open-source it so the community can
extend it. No copyleft. No surprises. Just the tools we rely on, made faster and
safer.

| Project | What it is |
|---|---|
| 🎬 **[remade_ffmpeg_rs](https://github.com/Remade-With-Rust/remade_ffmpeg_rs)** | **Our FFmpeg alternative.** Drop-in `ffmpeg` and `ffprobe` binaries — demux → decode → filter → encode → mux, rebuilt as composable Rust crates with **zero GPL/LGPL**. Apache-2.0. `rusty_h264` is its H.264 codec. |
| 🧠 **[FFAI](https://github.com/Remade-With-Rust/FFAI)** | **Our sister project: media *for* AI.** "The AI media toolkit, remade with rust." Embedded ASR + TTS (**Mercury**), OCR (**Carmenta**) and vision-language captioning (**Argus**) behind an ffmpeg-style, swap-by-name architecture — no Python, no CUDA. MIT OR Apache-2.0. |
| 🌐 **[Mata Network](https://www.mata.network/)** | **The home page.** *"Stop sacrificing your privacy for convenience."* Sovereign, self-hostable privacy infrastructure — wallet & identity, password manager, contact manager, and a browser extension that stops information leaking as you browse. Remade With Rust is its open-source arm. |

→ All projects: **[github.com/Remade-With-Rust](https://github.com/Remade-With-Rust)**

<!-- /ORG BOILERPLATE -->

## License

MIT. See `LICENSE` at the repository root.

---

<!-- HARDENING-TABLE:BEGIN generated by use-protection-please — edit docs/plans/use-protection-please.md, not this block -->
## Hardening status

**Tier** critical-path · **Audited** 2026-08-20 (survey) · **v1.0.0 gates** 14/15 · [Full checklist](https://github.com/remade-with-rust/rusty_alloc/blob/main/crates/rusty_alloc/docs/plans/use-protection-please.md)

`██████████████████░░` **94%** &nbsp;·&nbsp; 34 Completed · 1 Scheduled · 1 Incomplete · 19 N/A

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
| 10 — Cryptography | 2 | 0 | 0 | 1 |
| 11 — CI/CD, release, and operations | 4 | 0 | 1 | 0 |
| 12 — Compliance controls | 0 | 0 | 0 | 14 |
| **Total** | **34** | **1** | **1** | **19** |

**Next up** — H-27 Continuous fuzzing with no open crashes (2026-09-19 (30 days from the nightly job's first run))

**Architect** — Tim — Mata Network
<!-- HARDENING-TABLE:END -->
