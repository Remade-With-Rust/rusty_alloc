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

# rusty_alloc

A ground-up, pure-**Rust** general-purpose **allocator** — the mimalloc v2.4.5
architecture rebuilt from the design rather than transliterated from the C. No
C in the dependency tree, permissive licence, and a safety property upstream
does not offer.

## The headline

- **At-or-below mimalloc on instructions retired** on real programs under
  `LD_PRELOAD` (lua 0.97×, perl 0.99×, sqlite 1.00×), **2–16% under jemalloc**
  (lua 0.84×, perl 0.89×, sqlite 0.98×), and ~18% under glibc. Counting only
  the allocator's own instructions, where the comparison actually lives:
  **0.66× / 0.83× / 0.85×**.
- **A double free aborts instead of corrupting.** Upstream mimalloc accepts it
  silently in release builds; we detect it on both the local and the
  cross-thread path and abort.
- **~150 of mimalloc's ~157 `mi_*` entry points**, gated against the C
  implementation as a differential oracle on every change.
- **Runs on WebAssembly** with no C toolchain and no emscripten, and **two
  releases have cut what it adds to a gzipped bundle by two thirds** (+7,760 ->
  +3,829 -> +2,435 bytes on a minimal module) — see
  [Shipping it to a browser](#shipping-it-to-a-browser).
- **Runs on a microcontroller, and is 2.0-3.7x faster than `esp-alloc` there** —
  measured on a XIAO ESP32-S3 at 240 MHz, both allocators built from one source.
  It costs more RAM to get that (68 KiB vs 8 KiB); both numbers are below.

> **Status: `2.0.2`.** The API is frozen and changes follow semver. 2.0.1 and
> 2.0.2 are patch releases: no public API moved. 2.0.2 halves the allocator's
> flash cost on a microcontroller and cuts its gzipped wasm overhead by a third
> (see [Embedded](#embedded-bare-metal-measured-on-silicon) and
> [Shipping it to a browser](#shipping-it-to-a-browser)).
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
>
> Upgrading from 0.3.x or earlier is mandatory, not optional: 0.4.0 fixed
> three platform-independent use-after-frees, so **treat 0.3.2 and earlier as
> unsound on every target.**

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

jemalloc is 5.3.0; all four arms are the same neutral binary under `LD_PRELOAD`,
same callgrind method.

> **Provenance, and a caveat.** These were measured at `v1.1.5` (2026-08-28) and
> **predate the reclamation fixes on `main`** — `collect` reclaiming a bin's last
> page, the periodic `generic_collect`, and the reclaim-and-retry before the
> generic path returns null. Those cost **2.2-7.9 % of allocation throughput**
> measured on an ESP32-S3, so the ratios above are the floor of what the current
> `main` would report, not the number. `bench/icount-arms.sh` regenerates every
> column, and the `icount` CI job runs it on a schedule; **re-run it before
> quoting these for a release.**

Those are whole-program ratios, and they understate the allocator by design,
because in a real program most instructions are not the allocator. The same
runs, decomposed:

| workload | allocator share | **allocator-only ra/mi** | whole program | floor if our allocator cost ZERO |
|---|---:|---:|---:|---:|
| lua | 4.9% | **0.66** | 0.97 | 0.93 |
| perl | 3.3% | **0.83** | 0.99 | 0.96 |
| sqlite | 1.5% | **0.85** | 1.00 | 0.98 |

The last column is the honest ceiling: 98.5% of sqlite is SQLite, so even if
`malloc` and `free` were free — zero instructions — that row could only read
0.98. It sits at 1.00 because of what the program is, not what the allocator
does. The whole-program figure is kept as the headline anyway, because it is
the conservative one and it is what a user actually experiences.

Per-operation (`bench/opscan.sh` — **all 13 operations below mimalloc**):

| op | ra/mi | op | ra/mi |
|---|---:|---|---:|
| small | 0.49 | batch lifo/fifo | 0.89 |
| small_touch | 0.51 | realloc | 0.51 |
| med | 0.47 | aligned | 0.50 |
| big / large | 0.53 | mixed | 0.67 |
| huge | 0.01 | calloc | 0.63 |
| usable | 0.73 | | |

Those are single-purpose microbenchmarks, and five of them (`small`, `med`,
`big`, `large`, `huge`) hold **one block live at a time**, so the page empties
on every free and what they measure is page retire-and-recarve. Workloads with
a real working set never let a page drain, and they are the honest number for a
program:

| workload op | ra/mi | what it does |
|---|---:|---|
| `liveset` | **0.92** | 65,536 live objects, random victim replaced each step |
| `shbench` | **0.91** | bulk batches allocated and released in waves |
| `xthread` | **0.83** | every free performed by a non-owning thread |

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

Two of those were closed by writing the instruction pair the compiler would
not. `used--` took **five** instructions from safe Rust — load, decrement,
store, test, branch — because LLVM will not emit a memory-destination
read-modify-write when the value also has to drive a branch; it is two now.
Resolving a pointer to its page took nine, including an `imul` by 88; a 2 KiB
per-slice owner table in the segment header makes it a scale-4 load and an
`lea`.

The flags test is the one row where upstream is cheaper, and it stays that way
on purpose. ThreadSanitizer found a genuine race there — a thread adopting an
abandoned segment rewrites page flags while another reads them to route a free
— and making that byte atomic costs **exactly one instruction**, because LLVM
will not fold an atomic load into a test's memory operand. Upstream reads the
same byte non-atomically, does not pay the instruction, and has the race.
Winning it back with inline assembly would keep the read atomic in hardware
while hiding it from the sanitizer that caught the bug, so it was declined.

**These are counts, not seconds.** Wall-clock cannot be resolved on the
development machine (the null arm — the same allocator against itself — reads
±1.2%, wider than the effect), so no wall-clock speed claim is made anywhere
in this repository. Reproduce with `bash bench/icount-arms.sh` and
`bash bench/opscan.sh`; `bash bench/datasweep.sh` checks the answers rather
than the cost.

**RSS:** long-lived services should set `purge_delay >= 0` — that is the
configuration with flat, measured RSS (a 6-minute thread-churn soak held
9.4 MiB, slope −0.02 MiB/min). The shipped default leaves purging opt-in.

## Embedded: bare metal, measured on silicon

`rusty_alloc` builds `no_std` and runs as the `#[global_allocator]` on an
ESP32-S3. Everything below was measured on a **Seeed XIAO ESP32-S3 Sense at
240 MHz**, against **`esp-alloc` 0.11**, the standard allocator for the
`esp-hal` bare-metal track.

### What a firmware has to set

**All three of these, not two.** Every number in this section is *of this
configuration*; the 68 KiB floor below is meaningless without the geometry flag.

```toml
rusty_alloc-api = { version = "2", default-features = false }
```

```sh
RUSTFLAGS="--cfg ra_single_threaded --cfg ra_small_profile"
```

```rust
// A region the linker owns, aligned to a segment. Hand it over once, before
// the first allocation.
#[repr(align(65536))]
struct Region([u8; 68 * 1024]);
static mut REGION: Region = Region([0; 68 * 1024]);

// SAFETY: the only reference ever taken to REGION.
rusty_alloc::prim::fixed::init_region(unsafe { &mut (*(&raw mut REGION)).0 })
    .expect("region is large enough and registered once");
```

| flag | what happens without it |
|---|---|
| `--cfg ra_single_threaded` | **build fails**, with a message telling you to set it |
| `--cfg ra_small_profile` | **builds and links clean, then nothing allocates** — `SEGMENT_SIZE` stays 32 MiB, a kilobyte-scale region yields zero segments, and the first `Vec` returns null |
| `init_region` | every allocation fails; the backend has no memory |

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

### Throughput — 2.0x to 3.7x faster

Nanoseconds per allocate/free pair, lower is better:

| workload | `esp-alloc` | `rusty_alloc` | speedup |
|---|---:|---:|---:|
| 32 B alloc/free, one size | 1,638 | **647** | **2.53x** |
| 64 mixed blocks (8-512 B), batch out then back | 1,792 | **881** | **2.03x** |
| **churn: 64 live, random sizes 8-512 B, random replacement** | 3,987 | **1,087** | **3.67x** |
| 2048 B alloc/free | 1,638 | **1,200** | **1.37x** |

Both arms measured in the same session, same floor (162 ns/op in each), with
matching checksums. These include the reclamation fixes a stress battery forced
(see below), and the 2 KiB row also carries the medium-band collect-and-retry
that took it from 1.19x to 1.37x.

The churn row is the one to read. It is the shape real code has, and the shape
that fragments a first-fit free list — which is exactly what a size-class page
allocator with per-class free lists is built not to do. The 2048 B row is the
weakest because at this geometry 2 KiB is the top of the binned range and lands
in a medium page.

**How the arms are kept honest:**

- **One binary source.** Both arms are the same firmware; `--cfg` picks the
  allocator, so the dependency graph and every other line are identical.
- **Equal budgets.** Both get the same 192 KiB for the speed run, so neither is
  advantaged by having more (or less) memory to walk.
- **The harness measures itself.** A baseline arm runs the identical loop,
  non-inlined touch and four volatile accesses with *no allocator call*. It cost
  **162 ns/op in both arms** and is subtracted from every row above. Without
  that subtraction the harness overhead sits inside both arms and compresses the
  ratio.
- **Work parity is proven, not assumed.** Every block is written and read back
  through `write_volatile`/`read_volatile` (so the optimiser cannot delete an
  alloc/free pair and time an empty loop), folded into a checksum that is
  printed. **Every checksum matches across the two arms**, so both allocators
  provably did the same work.
- **A null arm.** The same benchmark twice within one arm reproduced to the
  nanosecond (625 and 625; 1,638 and 1,638), so the resolution floor is below
  any gap claimed here. Best-of-5, spread <= 1% on every row.

### Footprint — this is the cost, not a win

| | `esp-alloc` | `rusty_alloc` |
|---|---:|---:|
| smallest heap that runs the same workload | **8 KiB** | 68 KiB *(needs `--cfg ra_small_profile`)* |
| peak live bytes (identical, the parity check) | 4,914 | 4,914 |
| flash (`.text` + `.rodata` + `.data`), one firmware, two arms | — | **+7,860 B** |
| static RAM (`.bss` + `.data`) | — | **+3,052 B** |

**Flash and static RAM are two more budgets, and the second one is a
hazard.** The rows above are from `size -A` on the linked ELF of one
`esp-hal` firmware built twice, allocator selected by a feature and nothing
else different. `+7,860 B` of flash is what is left after this release removed
half of it: `ra_single_threaded` used to prune nothing, so the cross-thread
machinery — abandoning a segment when a thread ends, adopting one back, the
delayed list a remote free lands on — was linked into a target that had
asserted a single context, and guarded-object sampling shipped on a chip with
no MMU to protect a page. Both now fold away on any single-context target,
which also took **10.7 %** off the gzipped wasm bundle.

The static RAM comes **straight out of the stack**: the linker hands `.stack`
whatever RAM is left, and in the measured firmware `.stack` shrank by exactly
`Δ.bss + Δ.data`, to the byte. A firmware sitting near its stack limit does
not get a bigger binary when it adopts `rusty_alloc` — it gets a stack
overflow, and nothing in the build says so. Check `size -A` before and after.

The two costs have different shapes, and a reader choosing an allocator wants
both curves. **Flash is roughly fixed** — about 8 KB whether the firmware is
240 KB (3.3 %) or 900 KB with a TLS stack in it (under 1 %), so it stops
mattering as the firmware grows. **The 68 KiB heap floor does not** — it
scales with the size classes a program touches, not with the program, so it
matters exactly as much on a big firmware as on a small one. The full
decomposition, the three levers taken and the one residue deliberately left
(a 1.75 KB heap sentinel in `.data`) are in
[`docs/plans/finished/firmware-code-size.md`](docs/plans/finished/firmware-code-size.md).

**`esp-alloc` wins this by 8.5x, and the reason is structural rather than a
missing optimisation.** A linked-list heap's floor is `bytes live + per-block
header`. A size-class page allocator's floor is `(size classes touched) x (page
size)` — independent of how many bytes you actually asked for. The measured
workload touches 10 classes and holds 4,914 bytes; nine of its pages hold 1,412
bytes between them.

**Read 68 KiB as this workload's floor, not a general budget.** It is the least
memory that runs *this* sketch. A stress battery that touches 24 distinct size
classes holds only 5 of them at 68 KiB — because the floor scales with the
number of classes a program uses, which is the same sentence as above read from
the other end.

That floor is roughly **fixed** for a given mix of sizes: the same pages serve a
5 KB working set or a 500 KB one. So the crossover is where live bytes approach
`classes x page size` — below it `esp-alloc` wins by construction, above it the
page allocator starts earning what it charges, and the throughput above is what
it buys.

We got from 192 KiB to 68 KiB by fixing a placement bug in the fixed-region
backend and halving the slice; the full decomposition, the levers taken and the
one deliberately left on the table are in
[`docs/plans/small-metal.md`](docs/plans/small-metal.md).

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

**The battery found a real bug and we fixed it.** `collect` was borrowing
`mi_page_retire`'s keep-one-page-per-class rule, so no collect at any level
could return a class's page to a different class, and nothing collected
automatically. On a heap with 16 slices per segment that compounded: capacity
decayed 168 -> 8 blocks and **22,533 of 50,000** churn allocations returned null
while 61,440 bytes sat free. With the fixes, capacity no longer decays at all
and the same churn returns 357.

The two remaining refusals are the size-class floor, not defects: a 64 KiB-aligned
request needs a whole free 64 KiB segment, and 21-of-24 classes is exactly what
30 slices hold once classes above 512 B cost four slices each.

**Use it on a microcontroller when** allocation throughput or fragmentation
under churn matters and you have RAM to spare. **Use `esp-alloc` when** the
budget is tight — which on many parts it is. We would rather say that than sell
you the wrong one.

## Shipping it to a browser

An integrator reported `rusty_alloc` adding ~12 % to their gzipped wasm bundle.
It was measured, and most of it is gone: half in 2.0.0, another third of what
was left in 2.0.2.

| | raw | gzip | overhead vs the Rust default |
|---|---:|---:|---:|
| dlmalloc (Rust default for wasm32) | 15,536 | 6,706 | — |
| rusty_alloc 1.1.x | 34,285 | 14,466 | +7,760 |
| rusty_alloc 2.0.0 | 25,734 | 10,535 | +3,829 |
| **rusty_alloc 2.0.2** | **22,550** | **9,141** | **+2,435** |

The 2.0.2 row is the embedded work paying off elsewhere: `wasm32-unknown-unknown`
without the atomics feature has one thread by construction, so the same
predicate that prunes the cross-thread machinery from a microcontroller image
prunes it here too, and guarded-object sampling — which needs an MMU wasm does
not have — no longer ships at all. The base row was rebuilt for this
measurement and did not move.

Measured on a minimal consumer built the way you would ship it (`opt-level="z"`,
`lto="fat"`, `panic="abort"`, `strip`), attributed by a set difference against
the same module without the allocator. The cause was an option-environment pass
that ran on `wasm32-unknown-unknown`, where `std::env::var` is a stub that always
fails: 38 iterations formatting 76 strings every startup, to read an environment
that target does not have. `tools/wasm-size.sh` is now a CI gate so it cannot
come back. The method and what was ruled out are in
[`docs/plans/wasm-size.md`](docs/plans/wasm-size.md).

### What the bytes buy

Same module, run in node — nanoseconds per allocate/free pair, net of a measured
harness floor, with checksums proving both allocators did identical work:

| workload | dlmalloc | rusty_alloc | |
|---|---:|---:|---|
| **churn: 64 live blocks, random 8-512 B** | ~71-78 ns | ~10-15 ns | **4.9-7.3x faster** |
| 2048 B tight alloc/free | ~9-14 ns | ~16-22 ns | 0.55-0.83x |
| 32 B tight alloc/free, and 64 mixed batched | — | — | within noise |

Churn is the shape real code has, and the one that fragments a free list. The
2048 B row is a tight same-size loop, where a boundary-tag allocator's
free-then-alloc is a single list push and pop; `alloc.rs` carries a dated,
measured note explaining why the obvious fix for it is a regression elsewhere.
Only the two outer rows are claims — the middle two straddle 1.0 across repeats
and are reported as such. Ranges are five runs; reproduce with
[`bench/wasm-speed/`](bench/wasm-speed/).

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

## Correctness evidence

Every change runs Windows + Linux suites (all features), `clippy -D warnings`,
Miri over the whole target, a 640-thread churn probe, a wasm VM self-test, and
deterministic instruction A/Bs against the C oracle. On top of that, the
allocator is validated against real workloads:

- **Every shape of request returns correct memory:** `bench/datasweep.sh`
  writes an identity-bearing pattern into every block and reads it back —
  every size from 1 to 4096 exhaustively, each class boundary held live
  simultaneously, an alignment matrix to 64 KiB, `calloc` zeroing of
  *recycled* (not just fresh) blocks, `realloc` chains that grow and shrink
  across class boundaries, cross-thread frees in both directions, and a
  20,000-block scan proving no two live usable extents overlap. At
  `datasweep.sh 4`: **1,117,640 checks and 7.25 GiB of pattern-verified data
  per arm, 0 failures, in all six arms** — glibc, mimalloc, jemalloc, and our
  default, `secure` and `debug_checks` builds. The other allocators are the
  control, which is the point: a failure in every arm is a bug in the driver
  (that has happened), and a failure in one arm is a bug in that allocator.
- **Real programs, byte-identical output:** jq, sqlite3, python3, git, xz,
  zstd, lua and perl each produce bit-for-bit identical output under
  rusty_alloc, mimalloc and glibc, across interleaved repeat passes
  (`corpus/realworld.sh`). Two rows in that sweep do not agree and neither is
  an allocator difference, which is the point of running three arms:
  **imagemagick** differs between repeat runs of the *same* arm (it embeds
  non-deterministic metadata), and **redis** fails under rusty_alloc and
  mimalloc alike while working under glibc — that binary links jemalloc, so
  preloading any allocator over it breaks malloc/free pairing.
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
