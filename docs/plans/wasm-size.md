# wasm-size — what rusty_alloc costs a browser, what it cost twice, and what it buys

**Source:** a GitHub message from someone integrating the crate, reporting
**+12 % on their gzipped wasm bundle**. Written 2026-09-08 against `main` at
2.0.0.

**Status: MEASURED, HALVED, and the other half of the question answered (§8).** The allocator's gzipped overhead on a minimal
consumer went **+7,760 → +3,829 bytes**. A CI ratchet (`tools/wasm-size.sh`)
now fails the build if it grows again.

---

## 1. The instrument, before any conclusion

Size claims are cheap and usually wrong, so the arms were built the way an
integrator actually ships — not the way this repo profiles:

```toml
[profile.release]
opt-level = "z"
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

A minimal consumer allocates, frees, resizes and returns a number. **Arm A** is
the Rust default for `wasm32-unknown-unknown` (dlmalloc). **Arm B** adds one
line — `#[global_allocator] static A: RustyAlloc = RustyAlloc;` — and changes
nothing else.

**The null arm matters more than usual here.** The first attribution pass
bucketed `core::fmt` as the allocator's cost. Profiling the BASELINE showed
2.5 KiB of that was the consumer's own, and only 406 bytes were ours — so a
change made on the strength of the first reading (removing `format!` from
`options`) measured **exactly zero**. Attribution is therefore a **set
difference** between the two modules' function tables, never a per-module bucket:

```
rusty_alloc ADDS    14,659 B of functions
rusty_alloc REMOVES  8,012 B of dlmalloc
net                 +6,236 B of code
```

Tooling is three short Python files in the scratchpad, not a dependency: the
wasm binary format is sections of `(id, LEB size, payload)`, the Code section is
a vector of size-prefixed bodies, and the custom `name` section maps function
index to name. That is enough to attribute every byte.

## 2. What it actually was

**`options::get` was the largest function in the whole module at 3,708 bytes —
larger than anything in the allocator proper.**

`ensure_init` runs an environment pass. On `wasm32-unknown-unknown`,
`std::env::var` is a stub that always fails. So on every startup it ran 38
iterations, built 76 `format!` strings, allocated 76 `String`s and called
`to_uppercase` on 38 option names, in order to read an environment that target
does not have. The `OPTION_NAMES` table and the formatting machinery shipped
with it.

The `no_std` arm had already deleted this pass in P3 of `small-metal.md`, for
exactly the same reason — *there is nothing to read*. wasm was simply never
added to the condition.

| | raw | gzip | overhead |
|---|---:|---:|---:|
| dlmalloc (Rust default) | 15,536 | 6,706 | — |
| rusty_alloc, as reported | 34,285 | 14,466 | **+7,760 gz** |
| + env pass gated off wasm | 26,105 | 10,766 | +4,060 gz |
| + single-`static` TLS on wasm | **25,734** | **10,535** | **+3,829 gz** |

`target_os = "unknown"` and not `target_arch` alone: **wasm32-wasip1 has a real
environment and keeps the pass.**

## 3. The second cut: TLS that can never be used

`wasm32-unknown-unknown` has one thread unless the atomics+threads proposal is
enabled — an assumption `prim/wasm.rs` has carried since it was written. But
`ra_thread_local!` still expanded to `std::thread_local!` there, linking lazy
initialisation, destructor registration and the *"cannot access a Thread Local
Storage value during or after destruction"* panic path. That string was visible
in the data section.

The macro now takes the same single-`static` arm `no_std` uses, gated on
`not(target_feature = "atomics")` — the precise switch that
`-C target-feature=+atomics` sets, so a threaded wasm build keeps real TLS.
−371 raw, −231 gzipped, and the `bench/wasm-selftest.mjs` waste gate still
passes in a real VM.

## 4. Three things that were NOT the answer

Recorded because each looked promising and each cost a measurement.

- **`core::fmt`.** ~2.5 KiB in the module, and mostly the consumer's. Removing
  `format!`/`eprint!` from `options` saved **0 bytes**. Kept anyway: it removes
  an allocation from an error path and gives `no_std` back a message it used to
  lose — but it is not a size lever and is not claimed as one.
- **The data section.** +4,124 bytes raw looks like a third of the problem, and
  is **535 bytes gzipped** — 13 %. Most of it is `init::EMPTY_HEAP_BOX`, a
  statically-initialised sentinel `Heap` whose `direct` table points at
  `EMPTY_PAGE` (129 pointers ≈ the 504 non-zero bytes in a 2,124-byte segment).
  It is what makes `malloc`'s fast path branchless; zeros compress to nothing,
  so shrinking it would trade a hot-path branch for no download.
- **`wasm-opt -Oz`.** Cuts raw hard (26,105 → 21,874) and gzip barely at all
  (10,766 → 10,598), because gzip already captures most of what it does. Worth
  recommending for parse time and memory; it is not a download lever, and it
  does not change the overhead ratio because the baseline shrinks too.

## 5. Also found: your build paths ship in the artifact

The data section carried absolute paths from panic locations —
`F:\coding\rusty_alloc\crates\...` and `C:\Users\<name>\.rustup\...` — in every
published module. `--remap-path-prefix` fixes it on stable (Cargo's `trim-paths`
is still nightly as of 1.98). Worth ~29 bytes gzipped; the point is not leaking
a developer's username and directory layout to everyone who downloads the page.

## 6. What is left, and what it is worth

- **`heap::huge_alloc`, 1,332 B.** The >32 MiB path. Cold, but reachable; cannot
  be removed.
- **`init::init_thread_heap`, 1,211 B.** Real setup work — page queues, the
  direct table, the RNG — not thread machinery.
- **`heap::adopt_segment`, 656 B.** Reclaims segments abandoned by *other*
  threads, so it is dead on single-threaded wasm. **Not taken:** if anything
  ever does abandon a segment, skipping the reclaim leaks it, and 656 raw
  (~200 gzipped) is not worth a leak.
- **Recommend to integrators**, in the README: `wasm-opt -Oz`,
  `--remap-path-prefix`, and `opt-level = "z"` with `lto = "fat"`.


## 8. And what the bytes buy — speed, in a real VM

Size was measured for two rounds before anyone asked the other half of the
question. **Is rusty_alloc faster than the allocator it replaces on wasm?** If
not, +3,829 gzipped bytes is indefensible at any size.

Same discipline as the ESP32 harness: one source, `--cfg` picks the allocator,
a FLOOR arm that allocates nothing, volatile touches folded into a checksum the
optimiser cannot elide past, seeded size sequences, best-of-7, run under node, and `bench/wasm-speed/` so it is re-derivable.
Nanoseconds per allocate/free pair, net of the floor:

| workload | dlmalloc | rusty_alloc | |
|---|---:|---:|---|
| **churn: 64 live, random 8-512 B** | ~71-78 | ~10-15 | **4.9-7.3x faster** |
| 2048 B tight alloc/free | ~9-14 | ~16-22 | 0.55-0.83x |
| 32 B tight alloc/free | 5.3-9.8 | 5.1-8.5 | within noise |
| 64 mixed, batched | 7.5-12.4 | 7.3-11.7 | within noise |

**Only two of those four rows are claims.** Five repeats put churn at 4.92,
5.26, 6.57, 5.22 and 7.30, and 2048 B at 0.70, 0.65, 0.65, 0.83 and 0.55 — wide,
but never near 1.0 from either side, which is what makes them claims at all.
"~5x" is the bottom of the churn range, not its middle. The other two straddle 1.0 run to run and are reported as ranges
rather than ratios, because a harness with +/-25 % between-process variance
cannot resolve a 10 % effect and should not pretend to.

**The floor caught a broken first measurement.** It reported 5.2 ns/op for
dlmalloc and 0.6 ns/op for rusty_alloc — for *identical* code that allocates
nothing. V8 tiers wasm up per code path, and only one branch had been warmed.
The harness now warms every branch and prints a warning if the two floors differ
by more than 25 %, because a floor that differs is a harness measuring itself.

### Why 2048 B loses, and why that is not a bug to fix

Instrumented rather than guessed: exported `alloc::stats().generic` and counted
slow-path trips per operation.

```
32 B      0.008 per op
churn     0.060 per op
64 mixed  0.034 per op
2048 B    1.000 per op     <- every single allocation
```

Every 2 KiB allocation takes the generic path. `Heap::malloc` has a medium fast
path, but the `GlobalAlloc` entry reaches `malloc_slow`, which goes straight to
`malloc_generic` — and `alloc.rs` says exactly why, dated and measured:

> **REFUTED 2026-08-21** — peeking the MEDIUM bin's queue front here is a large
> regression: `big` and `large` +25.00 Ir/op each [...] A tight alloc/free loop
> frees into `local_free`, so the queue front's `free` list is ALWAYS dry when
> the next allocation arrives — the peek can never hit.

The benchmark row is precisely that shape: one live block, freed into
`local_free`, so `free` is empty on every allocation. dlmalloc wins it because a
boundary-tag allocator's free-then-alloc of one size is a list push and pop.
**The repo had already tried the fix and measured it worse.** Recorded here so
the third person to notice the row does not try it again.

### And a second attempt at it, also reverted (2026-09-08)

The refuted experiment added a *peek*. A peek cannot hit, because `free` is dry.
So the obvious next idea is to add the **collect** — swap `local_free` into
`free` before popping, which is the same swap `malloc_generic_walk` performs a
few lines later, and is cheap in the common case (a local list swap plus one
acquire load; `page_collect` peeks before entering its exchange loop). Guarded
on `size <= MEDIUM_OBJ_SIZE_MAX`, so it cannot reproduce the refuted version's
`big`/`large` +25 Ir/op.

It works, and it is still not worth it:

| workload | effect, both orders agreeing |
|---|---|
| 2048 B tight loop | **~7 % faster** (1.08x and 1.07x) |
| **32 B tight loop** | **~3-4 % SLOWER** |
| churn, batched | orders disagree — noise |

**Reverted in that form** -- 32 B is the commonest allocation there is, and
paying the hottest path to narrow a microbenchmark that stays lost is the wrong
trade.

### Then GATED — and then reverted, on the contention it hid (2026-09-08)

A change with a `+` and a `-` outcome is an invitation to find the predicate
that separates them. The predicate was in the counters: 2 KiB reaches
`malloc_generic` on **1.000** of its calls and 32 B on **0.008**.

That ratio is ROUTING, not list state — which is what both earlier explanations
assumed, including the 2026-08-21 refutation's. `alloc::malloc` serves
`size <= SMALL_SIZE_MAX` from the direct table and tail-calls `malloc_slow`,
which goes straight to `malloc_generic`; `Heap::malloc`'s medium branch is on a
different entry point and is never reached through `GlobalAlloc`. Gating the
retry to `size > SMALL_SIZE_MAX && size <= MEDIUM_OBJ_SIZE_MAX` removed the
payer and grew the win:

| workload | ungated | band-gated |
|---|---|---|
| 2048 B tight loop | +7 % | **+15 %** (wasm), **+15-19 %** (native) |
| 32 B tight loop | -3 to -4 % | no effect |
| churn, batched | noise | no effect |

**And then it was reverted, because every one of those numbers is
single-threaded.** `page_collect` loads `xthread_free` — the line a remote
`free` pushes onto. wasm cannot show contention there (one thread by
construction) and neither could the native benchmark. So it was built: a
producer allocates a batch of 2 KiB blocks, a consumer thread frees THAT batch
while the producer is timed allocating another, so remote frees land on the
producer's own pages with no handshake anywhere in the timed loop.

| workload | retry OFF | retry ON | |
|---|---:|---:|---|
| **producer alloc, consumer freeing its pages** | ~51-75 ns | ~71-77 ns | **0.67-1.00x** |
| same, single-threaded control | ~21-32 ns | ~24-32 ns | no effect |

Never better, usually 15-33 % worse, across two harnesses and eight runs. That
killed the UNCONDITIONAL form: cross-thread frees are the producer/consumer
shape (thread pools, channels, async runtimes) and mimalloc exists to serve
them. A gain on a single-threaded tight loop does not buy a 20-30 % loss
there. What survived is the gated form below.

**The obvious fix does not work, and that is the finding worth keeping.** The
first instinct was to do only the LOCAL half of the collect — swap `local_free`
into `free`, touch no atomic. It measured **worse** (0.67-0.77x): `free`,
`local_free` and `xthread_free` are adjacent fields of a `#[repr(C)]` `Page`,
so they share a cache line. Reading *any* of them pulls the line the remote
thread is invalidating. There is no version of "peek the queue front" that is
cheap while another core is freeing into that page — which is the mechanism the
2026-08-21 note observed the effect of without naming.

### The predicate that works: ask the HEAP, not the thread count

Built, because the contended harness was already in hand. `page_collect` now
reports whether it stole a cross-thread chain, and any steal this heap observes
latches a sticky `saw_remote_free`; the retry runs only while it is clear.

**The predicate is not "is this program single-threaded".** It is "does THIS
heap receive remote frees", which is finer and more useful: a worker that owns
its allocations keeps the win however many threads the process has.

| workload | before the latch | with the latch |
|---|---|---|
| tight 2 KiB loop, 1 thread | +15-19 % | **+14-16 %** (1.08-1.23, seven runs) |
| tight 2 KiB loop, 2 threads, each freeing its own | not measured | **+12-26 %** |
| producer alloc / consumer freeing its pages | **-20 to -33 %** | **neutral** (0.84-1.15) |

The contended row straddles 1.0 with wide variance, so the claim is that the
regression is gone, not that contention got faster.

**Latching on the retry's own steal was not enough**, and a second measurement
is what found it: the frees that hurt land on OTHER pages of the same heap, so
the page being retried never sees them and the retry never switched itself off.
The latch is set from `malloc_generic_walk`'s collects as well, which is where
those pages are actually reached.

**And the harness had to be told about the latch.** Sticky is correct in
production and useless in an A/B: the contended case ran first in warmup and
left the retry off for every case after it, so the win read as 1.00x until the
latch was reset between timed runs. An instrument sometimes has to know about
the mechanism it is measuring.

### Validated on every platform that can run it (2026-09-08)

The retry was measured on native Windows and reasoned about everywhere else, so
it was run everywhere else. **The same one workload shape moves on all three
targets, and nothing else moves anywhere.**

| target | 2 KiB tight alloc/free | every other row |
|---|---|---|
| native x86-64 (Windows) | **+14-16 %** (1.08-1.23, 7 runs) | no effect |
| wasm32 in V8 (node) | **+1-16 %** (1.01-1.16, 4 runs, both orders) | no effect |
| Xtensa ESP32-S3, `no_std`, small profile | **+15 %** (1,380 -> 1,200 ns/op) | within 1-2 % |

The board is the cleanest of the three: its harness reports 0-1 % spread, and on
the small profile `SMALL_SIZE_MAX` is 512 B, so the other three rows (32 B, a
mixed batch of 8-512 B, and churn over 8-511 B) are all BELOW the band and the
retry provably cannot touch them. Their 1-2 % movement is layout, not effect.

**"A win across the board" is the wrong phrase for it.** It is a win on one
workload shape -- medium sizes cycled through a tight alloc/free loop -- that
happens to hold on every target. Everything else is unchanged, and the contended
case is neutral rather than better.

**Correctness, all green on the same build:** 109 tests / 33 suites default,
90 / 19 at the small profile, clippy `-D warnings` on default / small profile /
`no_std`, `riscv32imac` and `riscv32imafc` at both geometries, `rusty_alloc-api`
`no_std`, the wasm self-test's waste gate inside a real VM, and on hardware both
the 68 KiB footprint kill test (`used` 69,632, `PEAK` 4,914 -- unchanged) and
the eight-test stress battery (capacity flat at 240, same passes, churn nulls
458 against a 334-575 range already seen). Plus the three ratchets: unsafe
census, gate selftest 5/5, wasm size.

**Still not covered, and worth saying plainly:** host INSTRUCTION counts under
callgrind, which is the instrument the README's figures use and which no
Windows box can run -- the scheduled `icount` job is the gate for that -- and
contention with more than one consumer thread.



**Two harnesses, one lesson.** The first version of the contention benchmark
handed blocks over an SPSC ring and timed the producer loop; at ~113 ns/op it
was timing the ring's spin, not the allocator, and its numbers wandered (a
1.37x outlier beside two 0.95x). Removing the handshake from the timed path is
what made the effect readable.

## 7. The gate

`tools/wasm-size.sh` builds the fixture under `bench-dist` (the repo's `release`
keeps debug symbols, and a 2 MB artifact hides a 4 KB regression), gzips it, and
fails past a 3 % tolerance. Baseline in `tools/wasm-size-baseline.txt`, updated
with `--update` in the same commit as the change that moves it — the same
discipline as the unsafe census.

Poisoned by restoring the env pass on wasm, it fires.

**The lesson is the gate, not the bug.** Nothing measured wasm size, so a
3,700-byte-gzipped regression sat in the crate for its entire life and reached a
user before it reached us.
