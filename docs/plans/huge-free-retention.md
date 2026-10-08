# Freed huge allocations stay resident in a long-lived service

**Status:** measured in a shipping consumer; **three mechanisms found, two fixed in the working tree 2026-10-07 (uncommitted, see §7)**; asks 1-3 and 5 open ·
**Date:** 2026-10-07 · **Seen on:** 2.2.3, default options (`purge_delay = -1`) ·
**Platform:** measured on Windows 11 x86-64; the consumer runs on Linux (Fly.io, 4 GB VM) ·
**From:** the ragconverter session (`rag-converter`, `crates/ragconverter-server`)

---

## 1. What happened

ragconverter's API was OOM-killed twice in five minutes on 2026-10-07. Two requests were in
flight on a 4 GB machine. Every request in flight at each kill failed with a 502.

The server loads two models of about 1 GB each, built from safetensors through candle:
- Whisper small.en: 922 MB of f32;
- SmolVLM-256M: 978 MB once its bf16 weights are widened to f32.

The ragconverter side of the fix is to keep only one model resident at a time and drop the other
before loading it. That fix did not bring the peak down by the size of a model: freed model
memory stayed resident under rusty_alloc. With the system allocator it went back to the OS.

## 2. Measurement

One process converts a video (all engines), a FLAC and a PNG, one job at a time. Each job loads
the model it needs and drops the other. The measure is the process's peak working set, sampled
every 200 ms. It is a release build with `opt-level=3` and the same inputs every time.

| Allocator / option | Peak working set | Runs |
|---|---:|---|
| system allocator (no `#[global_allocator]`) | **2,722 MB** | 1 |
| rusty_alloc 2.2.3, default (`purge_delay = -1`) | **3,802 / 3,832 / 3,881 MB** | 3, interleaved with the next row |
| rusty_alloc 2.2.3, `RUSTY_ALLOC_PURGE_DELAY=0` | **2,868 / 2,885 / 2,887 MB** | 3, interleaved |
| `RUSTY_ALLOC_PURGE_DECOMMITS=1` (purge still off) | 3,472 MB | 1 |
| `PURGE_DELAY` = 10 / 250 / 1000 | 2,797 – 2,900 MB | 2 each |

- **Retention:** about **0.95 GB**, roughly one dropped model, against the same allocator with
  purging on. That is about 1.1 GB against the system allocator.
- **Consistency:** the gap is the same in all three interleaved pairs.
- **Cause:** it is the purge setting and not the workload. Any `purge_delay >= 0` closes most
  of it. The remaining ~150 MB over the system allocator has not been explained (see §4,
  ask 4).

**Time is not settled.** In the three interleaved pairs, `purge_delay = 0` took 48–50 s against
38–40 s for the default, about 25% slower every time. But a headless browser test was running
on the same machine for part of those runs, and the delay sweep's timings range from 49 s to
183 s. Read the 25% as a lead to re-measure on a quiet machine, not as a result.

## 3. Why it matters beyond this consumer

This matches what the LEDGER already says from the soak (§ "Actionable consequence"):
*long-lived services should set `purge_delay >= 0`*. The difference is the shape:
- the soak saw ~650 MiB against a ~175 MiB live set, drifting slowly;
- here **one free of a gigabyte-scale buffer** decides whether the next allocation fits in the
  machine.

Any service that loads and swaps models, or holds one large request buffer at a time, hits this
on its first swap rather than after hours. The failure is an OOM kill, not a slow climb.

The consumer cannot easily opt in either:
- `ragconverter-alloc::configure(Profile)` is a no-op. The crate exposes no global purge or
  configure, and `Heap::collect` is per-heap, which a `#[global_allocator]` cannot reach.
- So the only lever is the environment variable, set before the first allocation, which a
  deployment has to know to set.

## 4. Asks, most useful first

1. **Release huge-segment frees to the OS whatever `purge_delay` is.**
   - A single allocation of tens of megabytes or more is the case where retention costs most
     and reuse is least likely.
   - It is also the case `purge_delay = -1` was not chosen for: the M8 defect is about span
     purge and recommit, and huge segments are freed whole.
   - Worth checking against `oracle/mimalloc`, which this was not compared with, to see what
     it does with a freed huge segment.
2. **A global purge-now call a `#[global_allocator]` consumer can make**, e.g.
   `rusty_alloc_api::purge()`: collect every heap, including abandoned ones, and purge free
   spans now. A consumer knows exactly when it has dropped a gigabyte. Calling it then costs
   nothing on the hot path, unlike `purge_delay = 0` (see the timing note in §2). The
   ragconverter-alloc seam is already waiting for this.
3. **Options settable from code, not only the environment**, so a deliverable can say
   "long-lived service, purge on" in one place: `configure(Profile::LongLived)`.
4. **Explain the remaining ~150 MB** over the system allocator with purging on. A candidate:
   huge pages freed from a thread other than the one that allocated them sit DELAYED
   (`alloc.rs`, "huge pages sit DELAYED"). Candle builds tensors on one tokio blocking-pool
   thread and the model can be dropped on another. That thread may never allocate again, and
   may exit holding the delayed free. This is not verified: no counter was read.
5. **Re-measure the cost of `purge_delay = 0` on a quiet machine** with an inference-shaped
   workload (many medium alloc/free cycles around a large resident set). If the 25% is real,
   it is probably decommit/recommit churn on medium spans. That would make ask 1 or ask 2
   the right fix rather than turning eager purging on globally.

## 5. Reproduce

From the rag-converter repo, with the weights staged in the `/opt/models` layout:

```sh
cargo test -p ragconverter-server --release --no-run
# then run the ignored rig and sample the process's peak working set / VmHWM:
RAGCONVERTER_AUDIT_DIR=<corpus> RIG_FILES=video-h264.mp4,audio.flac,card.png \
CARMENTA_WEIGHTS=<ocr-assets> FFAI_MODELS=<models> HEAVY_JOBS=1 \
  target/release/deps/ragconverter_server-*.exe --ignored --exact routes::tests::concurrent_heavy_pair
```

- **Allocator arms:** set `RUSTY_ALLOC_PURGE_DELAY` for the option arms. Comment out
  `#[global_allocator]` in `crates/ragconverter-server/src/main.rs` for the system-allocator arm.
- **Measuring on Linux:** read `VmHWM` from `/proc/<pid>/status` at exit, instead of sampling.

## 6. What ragconverter does meanwhile

- **Purge on in production:** sets `RUSTY_ALLOC_PURGE_DELAY=0` on the Fly machine, following
  the LEDGER's own recommendation for long-lived services. The M8 crash is Windows-first (Linux
  `MADV_DONTNEED` does not fault on touch), and Fly is Linux.
- **One model at a time:** keeps one big model resident and runs heavy jobs one at a time.
- **Idle unload:** unloads idle models.
- **Alerts:** alerts on high memory and on an unclean restart.

If ask 1 or ask 2 lands, ragconverter can drop the environment variable and call purge exactly
when it drops a model.

## 7. Findings (2026-10-07)

A model-swap probe reproduced it without candle: a "model" is Whisper small's
tensor-size mix (one 152 MiB embedding, 2.4-9.4 MB matrices, 3-12 KB vectors),
909 MB in all, touched page by page. Load model A, drop it, load model B, drop
it; read the process's peak working set. Windows 11, release build. Four
shapes of who loads and who drops:

| shape | system | mimalloc 2.3 | rusty_alloc 2.2.3 default | 2.2.3, `PURGE_DELAY=0` | fixed tree, default | fixed tree, `PURGE_DELAY=0` |
|---|---:|---:|---:|---:|---:|---:|
| one thread loads, drops, reloads | 1.00 | 1.01 | 1.01 | 1.01 | 1.01 | 1.01 |
| T1 loads and exits, T2 drops, T3 reloads | 1.00 | 1.04 | **1.62** | **1.62** | 1.01 | 1.01 |
| T1 loads and drops, exits; T2 reloads | 1.00 | 1.04 | 1.01 | 1.01 | 1.01 | 1.01 |
| T1 loads, drops, stays alive with small leftovers; T2 reloads | 1.00 | 1.04 | **1.23** | 1.01 | **1.23** | 1.01 |

(peak working set in models; the probe is `F:/ra-corpus-tmp/huge-ret`.)
Memory still resident at exit with purging on: 2.2.3 **159 MB**, fixed tree
**7 MB**, system 4-5 MB.

**Three mechanisms, not one.**

1. **Dead large blocks in an abandoned segment were never retired** (fixed).
   A thread that exits abandons its segments with their blocks live. Blocks
   freed into them later are collected when another thread adopts the
   segment, but `adopt_segment` retired only ONE dead large span per segment
   and `break`, "leaving the rest to later collects"; a large span is never
   queued, so no collect reaches it again. The rest stayed carved and
   resident for the life of the process: +0.62 of a model, and **purging did
   not help** (nothing reached `span_free`). Upstream collects every
   concurrent free of an abandoned segment (`mi_segment_check_free` in
   `mi_segment_try_reclaim`) and scored 1.04. Now every dead large span is
   retired, rescanning after each because retiring coalesces. Fixing it
   exposed a second slip, also fixed: when those retires emptied the
   segment, `retire_span` parked it as the heap's one empty segment and the
   tail of `adopt_segment` then counted it again and released it, leaving
   `empty_segments == 1` with no empty segment behind it.
   Test: `tests/adopt_large_spans.rs`; with the old `break` it fails (the
   next load takes a fresh segment).

2. **A live, idle thread's segments strand free space** (by design; purging
   is the remedy). Small pages are carved from the newest segment, so a
   model's small tensors spread across nearly every segment the load used,
   and each emptied small page stays as its bin's reuse cache, pinning a 32
   MiB segment to a thread that may sit idle (a tokio blocking-pool thread
   does, for 10 s by default, and longer under load). Measured: 26 segments
   allocated, 19 released, 7 (224 MiB) kept; +0.23 of a model. Free spans are
   per-thread by design; with purging on they are not resident, which is why
   `PURGE_DELAY=0` closed most of the consumer's gap. mimalloc packs small
   pages more tightly and scored 1.04 with its purging on or off; changing
   span placement is a separate, measured project.

3. **A freed huge block (> 32 MiB) stayed resident whatever `purge_delay`
   said** (fixed). `span_free` purges a freed span when purging is on;
   `huge_free` returned a dead block's chunks to the arena, which holds
   memory committed and resident. This is the consumer's unexplained ~150 MB
   (ask 4): Whisper small's embedding is 51,865 x 768 x 4 = 152 MiB. With
   purging on, `huge_free` now purges the block before its chunks rejoin the
   arena, marks it `purged_any`, and `restore_for_reuse` re-commits it
   (checked since 2.2.3), so the next tenant gets demand-zero pages. Test:
   `tests/huge_purge.rs`; with the purge disabled it fails ("returned only 0
   bytes").

Verified on Windows: the whole `rusty_alloc` suite with and without `secure`,
clippy `-D warnings` (`secure`), fmt, `cargo check --target
wasm32-unknown-unknown`, unsafe census unchanged (973).

**On the asks:**

- **Ask 1 (release huge frees whatever `purge_delay` is):** done for
  `purge_delay >= 0`. At `-1` it is a policy decision for the maintainer:
  mimalloc purges huge segments by default because its purge is ON by default
  (10 ms); ours is opt-in. Retaining a dead 150 MB block at `-1` saves a
  re-fault only if the next huge allocation reuses it.
- **Ask 2 (a global purge-now call):** not done. It is what mechanisms 1 and
  2 still need at `-1`, and what memory freed into an exited thread's
  segments needs until some thread allocates: the probe's "T1 loads and
  exits, T2 drops" case ends with B's 915 MB resident until the next
  allocation adopts it.
- **Ask 3 (options from code):** already possible, undiscoverably:
  `rusty_alloc::options::set(15, 0)` sets `purge_delay` at run time, and
  `span_free` reads it on every free, so it takes effect at once. Neither the
  `rusty_alloc-api` seam nor a named constant exposes it.
- **Ask 4:** explained by mechanism 3 above; the DELAYED-huge-page candidate
  was not needed.
- **Ask 5 (the 25 % cost of `purge_delay = 0`):** not re-measured.

**For ragconverter, today:** keep `RUSTY_ALLOC_PURGE_DELAY=0` (or 10). On
2.2.3 it still loses ~150 MB per freed embedding-sized tensor and up to ~0.6
of a model when a loader thread exits before the model is dropped; both are
gone in the fixed tree.
