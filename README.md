[![crates.io](https://img.shields.io/crates/v/rusty_alloc.svg)](https://crates.io/crates/rusty_alloc)
[![docs.rs](https://img.shields.io/docsrs/rusty_alloc)](https://docs.rs/rusty_alloc)
[![CI](https://github.com/remade-with-rust/rusty_alloc/actions/workflows/ci.yml/badge.svg)](https://github.com/remade-with-rust/rusty_alloc/actions)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![remade with rust](https://img.shields.io/badge/remade--with--rust-portfolio-orange.svg)](https://github.com/remade-with-rust)

### In The Wild with 12,582 Active Installs
> [RAG Converter](https://ragconverter.com) uses `rusty_alloc` as the allocator, in wasm too.
> It makes personal and work files AI-readable without them leaving the machine:
> the whole conversion runs as WebAssembly in the browser tab, with nothing
> uploaded and nothing to install.

> [Rusty_JSON_Turbo](https://github.com/Remade-With-Rust/rusty_json_turbo) is a Serde fork, and saw a 54% top speed increase by implementing
> Rusty_alloc.

# rusty_alloc

A ground-up, pure-**Rust** general-purpose **allocator** — the mimalloc v2.4.5
architecture rebuilt from the design rather than transliterated from the C. No
C in the dependency tree, permissive licence, and a safety property upstream
does not offer.

## The headline

- Counting only the allocator's own instructions, where the comparison actually lives:
  **0.66× / 0.83× / 0.85×**.
- **A double free aborts instead of corrupting.** Upstream mimalloc accepts it
  silently in release builds; we detect it on both the local and the
  cross-thread path and abort.
- **Runs on WebAssembly** with no C toolchain and no emscripten, and **two
  releases have cut what it adds to a gzipped bundle by two thirds** (+7,760 ->
  +3,829 -> +2,435 bytes on a minimal module) — see
  [Shipping it to a browser](#shipping-it-to-a-browser).
- **Runs on a microcontroller, and is 2.1-3.9x faster than `esp-alloc` there** —
  measured on a XIAO ESP32-S3 at 240 MHz, both allocators built from one source.
  It costs more RAM to get that (64 KiB vs 8 KiB); both numbers are below.

> **Status: `2.2.0`.** The API is frozen and changes follow semver. 2.0.1
> through 2.0.5 were patch releases; 2.1.0 and 2.2.0 are minors because they
> ADD public items and move none — `cargo-semver-checks` calls both
> API-compatible. **2.2.0 also carries the largest embedded speed fix in this
> series:** under `--cfg ra_small_profile` every page was being extended one
> block at a time, so `malloc_generic` ran on 100 % of allocations; on a XIAO
> ESP32-S3 fixing it is **13–16 % faster on every binned workload**.
> 2.0.2 halved the allocator's flash cost on a microcontroller and cut its
> gzipped wasm overhead by a third; 2.0.3 halved the flash cost again and took
> the static RAM cost from 3 KB to under 300 bytes; 2.0.4 makes a firmware's
> region whole segments with `prim::fixed::Region<N>` — unpadded by
> construction, the floor 64 KiB — and refuses a base that would silently cost
> a segment; 2.0.5 makes segments stride from that region's base, so it needs
> no segment alignment and the linker leaves no gap in front of it; 2.1.0 names
> the large-allocation ceiling in the API and adds
> `--cfg ra_segment_size="256k"` for a firmware whose allocation unit is tens
> of kilobytes. Three notes for embedded consumers: 2.0.4 moves what
> `good_region_size` / `region_for` / `MIN_REGION` return; 2.0.5 reduces
> `Region`'s alignment to 16 bytes, and `--cfg ra_aligned_region` restores the
> 2.0.4 layout; and if you allocate at or above
> `prim::fixed::LARGEST_SHARED_ALLOC` (61,440 bytes by default) read
> [Embedded](#embedded-bare-metal-measured-on-silicon) before sizing a region.
>
> **What breaks, and why it is a major.** Two things, neither of which touches a
> consumer on default features: `default-features = false` now selects the
> `no_std` profile (it used to be identical to the default, because there were
> no default features), and `heap::Heap` gained a field, which a struct literal
> would notice. If you set `default-features = false` and want what you had, add
> `features = ["std"]`. `CHANGELOG.md` has the detail.
>
> **What you gain**: `no_std` on bare metal, a second geometry for parts with
> kilobytes instead of gigabytes, and three reclamation fixes — one of which
> could leave a long-running heap reporting OOM while holding memory it could
> have reclaimed.

## Performance (deterministic instruction counts)

Instructions retired under callgrind, x86-64 Linux, `LD_PRELOAD`. perl and
sqlite repeat to the instruction with the hash seed pinned; **lua does not —
its seed is not pinnable from the environment and it moves ~0.26% run to run**,
so read lua as indicative and the other two as verdicts. Real programs,
WHOLE-PROGRAM instructions:

| workload | vs mimalloc | vs jemalloc | vs glibc |
|---|---:|---:|---:|
| lua | **0.97** | **0.84** | 0.82 |
| perl | **0.99** | **0.89** | 0.81 |
| sqlite | **1.00** | **0.98** | 0.99 |

Those are whole-program ratios, and they understate the allocator by design,
because in a real program most instructions are not the allocator. The same
runs, decomposed:

| workload | allocator share | **allocator-only ra/mi** | whole program | floor if our allocator cost ZERO |
|---|---:|---:|---:|---:|
| lua | 4.9% | **0.66** | 0.97 | 0.93 |
| perl | 3.3% | **0.83** | 0.99 | 0.96 |
| sqlite | 1.5% | **0.85** | 1.00 | 0.98 |

**Where this came from: `free`.** It is the function a real program spends its
allocator time in, so it is the one worth counting instruction by instruction.
Ours is now **21 instructions on the fast path against mimalloc's 25**, down
from 27:

| stage | rusty_alloc | mimalloc |
|---|---:|---:|
| null check + segment resolve | 4 | 4 |
| owner thread-id load | 1 | 1 |
| resolve pointer &rarr; page | **5** | 9 |
| page flags test | 3 | 2 |
| owner-thread compare | 2 | 2 |
| push onto `local_free` | 3 | 3 |
| `used--` and retire branch | **2** | 2 |
| CET landing pad + return | 1 | 2 |
| **total** | **21** | **25** |

## Embedded: bare metal, measured on silicon

`rusty_alloc` builds `no_std` and runs as the `#[global_allocator]` on an
ESP32-S3. Everything below was measured on a **Seeed XIAO ESP32-S3 Sense at
240 MHz**, against **`esp-alloc` 0.11**, the standard allocator for the
`esp-hal` bare-metal track.

### What a firmware has to set

**All three of these, not two.** Every number in this section is *of this
configuration*; the 64 KiB floor below is meaningless without the geometry flag.

```toml
rusty_alloc-api = { version = "2", default-features = false }
```

```sh
RUSTFLAGS="--cfg ra_single_threaded --cfg ra_small_profile"
```

```rust
use rusty_alloc::prim::fixed::{Region, good_region_size};

// The region: whole 64 KiB segments with no padding, and only 16-byte
// aligned — segments stride from its base, so the linker owes it no gap.
// Size it from a budget (or `region_for(usable)` from a need), never a
// round number, and hand it over once before the first allocation.
static HEAP: Region<{ good_region_size(220 * 1024) }> = Region::new();

let usable = HEAP.give().expect("region accepted, given once");
// usable == 196_608: three segments, nothing stranded.
```

Do not write your own container. A plain `static [u8; N]` has alignment 1,
so fifteen times in sixteen it sits off the 16-byte grid and yields a
segment fewer than its size says (`init_region` refuses that with
`FERR_MISALIGNED`); a `#[repr(align(65536))]` wrapper of your own is rounded
up to the next segment and costs more RAM than the granule it was meant to
save — 60,952 bytes of stack on the firmware that tried it. `Region<N>` is
the size it says, and nothing more.

| flag | what happens without it |
|---|---|
| `--cfg ra_single_threaded` | **build fails**, with a message telling you to set it |
| `--cfg ra_small_profile` | **builds and links clean, then nothing allocates** — `SEGMENT_SIZE` stays 32 MiB, a kilobyte-scale region yields zero segments, and the first `Vec` returns null |
| `Region::give` (or `init_region`) | every allocation fails; the backend has no memory |
| *(optional)* `--cfg ra_max_extents="8"` | the free-extent table keeps its default 32 slots (256 B of `.bss`, i.e. stack); 8 is plenty for a region of a few segments and returns 192 B — the doc on `MAX_EXTENTS` states the bound |
| *(optional)* `--cfg ra_aligned_region` | segments stride from the region's base: `Region` is 16-byte aligned, the linker leaves no gap before it, and `free` pays three instructions. With the flag, the 2.0.4 layout: the address mask on `free`, `Region` segment-aligned, up to 64 KiB of gap in front of it |
| *(optional)* `--cfg ra_segment_size="256k"` | a 64 KiB segment, so `LARGEST_SHARED_ALLOC` is 61,440 and any bigger request takes a dedicated **two** segments. With the flag, an 8 KiB slice x 32: a 64 KiB block is a span and three pack into one segment. Set it when your allocation unit is tens of KB; leave it unset for small objects, since it doubles the page floor |

Size the region with the two `const fn`s in `prim::fixed`, not a round
number: `good_region_size(220 * 1024)` is 196,608 — three whole segments,
nothing stranded — where a literal 220 KiB strands 28,672 bytes that neither
the allocator nor the firmware can use. The first heap's descriptor is a
1,752-byte static of the crate's, not a page of the region, which is what
lets the region be whole segments.

That middle row is the trap, and it was reported by the first outside firmware
to adopt 2.0.0 (`docs/plans/embedded-adoption.md`). `ra_single_threaded`
announces itself, so an integrator reasonably concludes the crate tells you what
it needs — and `ra_small_profile` did not. `init_region` now refuses a region
that cannot hold one segment at the active geometry, so the mistake is an `Err`
at startup rather than a wasted board run; **`--cfg ra_small_profile` is still
what you want to set**, because refusing early is a diagnosis, not a fix.

`ra_small_profile` is a `--cfg` and not a Cargo feature on purpose: it is
non-additive. Two crates in one graph cannot disagree about `SEGMENT_SIZE` the
way they can harmlessly disagree about `std`.

`ra_single_threaded` is more than a build permit. On a bare-metal target it is
also what lets the linker drop everything that only a second thread could ever
reach — abandon, adopt, the delayed-free list, the thread-exit hook — and on
`wasm32-unknown-unknown` without the atomics feature the same pruning happens
without any flag at all, because that target has one thread by construction.

### Throughput — 2.1x to 3.9x faster

Nanoseconds per allocate/free pair, lower is better:

| workload | `esp-alloc` | `rusty_alloc` | speedup |
|---|---:|---:|---:|
| 32 B alloc/free, one size | 1,638 | **595** | **2.75x** |
| 64 mixed blocks (8-512 B), batch out then back | 1,792 | **833** | **2.15x** |
| **churn: 64 live, random sizes 8-512 B, random replacement** | 3,987 | **1,011** | **3.94x** |
| 2048 B alloc/free | 1,638 | **1,134** | **1.44x** |

**`esp-alloc` wins this by 8.5x, and the reason is structural rather than a
missing optimisation.** A linked-list heap's floor is `bytes live + per-block
header`. A size-class page allocator's floor is `(size classes touched) x (page
size)` — independent of how many bytes you actually asked for. The measured
workload touches 10 classes and holds 4,914 bytes; nine of its pages hold 1,412
bytes between them.

### Stress: what a hostile workload does to each

Eight adversarial tests — every routing boundary, alignment up to a whole
segment, a realloc chain, `alloc_zeroed` over deliberately dirtied memory, a
fragmentation adversary, exhaustion and recovery, a 24-class sweep, and 50,000
random-replacement churn ops:

| | `esp-alloc` | `rusty_alloc` |
|---|---|---|
| routing boundaries, realloc chain, zeroing, fragmentation, exhaustion | PASS | PASS |
| 24 distinct size classes held at once | 24 | 21 |
| 64 KiB-aligned request | served | refused |
| NULLs in 50,000 churn allocations | 0 | 357 |
| 512 B capacity over the whole battery | flat 383 | flat 240 |

**Use it on a microcontroller when** allocation throughput or fragmentation
under churn matters and you have RAM to spare. **Use `esp-alloc` when** the
budget is tight — which on many parts it is. We would rather say that than sell
you the wrong one.

## Shipping it to a browser

Same module, run in node — nanoseconds per allocate/free pair, net of a measured
harness floor, with checksums proving both allocators did identical work:

| workload | dlmalloc | rusty_alloc | |
|---|---:|---:|---|
| **churn: 64 live blocks, random 8-512 B** | ~71-78 ns | ~10-15 ns | **4.9-7.3x faster** |
| 2048 B tight alloc/free | ~9-14 ns | ~16-22 ns | 0.55-0.83x |
| 32 B tight alloc/free, and 64 mixed batched | — | — | within noise |

**To ship it small:**

```toml
[profile.release]
opt-level = "z"      # "s" if you would rather have the speed
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

```sh
# Another ~16% off the RAW size (parse time and memory; gzip already
# captures most of what this does, so the download barely moves).
wasm-opt -Oz --enable-bulk-memory --strip-debug --strip-producers in.wasm -o out.wasm
```

**And check your own artifact for build paths.** Rust embeds panic locations as
absolute paths, so a published `.wasm` can carry your home directory and
username. `RUSTFLAGS="--remap-path-prefix=$PWD=."` fixes it on stable; Cargo's
`trim-paths` is still nightly.

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
rusty_alloc-api = "2.1"
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
