# Let's win — where the corpus actually spends its time

**Status:** measured, nothing implemented · **Date:** 2026-08-21 · **Build:** HEAD
(`18ff26b`, 1.0.0), override cdylib rebuilt from source before every arm
· **Box:** WSL2 Ubuntu on x86-64, 24 threads, `/mnt/c` filesystem

This is a **map, not a claim**. It answers one question — *across the whole
corpus, where do our milliseconds and our instructions go* — and then ranks
what to do about it. Every number below has a method line; the four that are
not verdicts are labelled as such rather than quietly averaged in.

---

## 0. The answer in one paragraph

The corpus does **not** have one problem, it has four, and the biggest one is
not the one the release notes are about. **`alloc-test` — the corpus's only
realistic random-size/random-lifetime workload — runs 1.49× the instructions
and 1.37× the wall time of mimalloc, because our allocator takes its generic
slow path on 70% of allocations where mimalloc takes it on 1.1%.** That is a
**path-frequency** defect, localised to one function, whose mechanism upstream
fixed on purpose — and it **predates the security work entirely**. The hardening campaign did cost
real performance — 1–6 Ir/op, all of it in a single commit — but that is the
*second*-order story, and the README's "exactly one instruction" is true only
of the one op it was measured on. Two further findings: we get **zero
transparent huge pages** where mimalloc gets ~97% of its heap on them (priced
here at 0–7%, and 0% on the workload where we lose most — so real, but not the
fix), and **thread-churn workloads cost us 2.8× mimalloc's instructions**,
concentrated in `adopt_segment`.

---

## 1. Method, and what makes a number admissible here

Two instruments, used for different questions, per `codec-measurement` and this
repo's standing rule that the counter is primary and the clock confirmatory.

**Instructions (verdicts).** callgrind, `--cache-sim=no --branch-sim=no`. Same
binary in every arm, only `LD_PRELOAD` differs, so work parity is structural
rather than argued — and on `alloc-test` it was confirmed exactly: both arms
made **50,130,729** allocations, to the unit. `PERL_HASH_SEED`/
`PERL_PERTURB_KEYS` pinned.

**Attribution is exact, and it is checked.** Per-symbol cost comes from parsing
the **raw** callgrind file (`ob=`/`fn=` with name decompression), never from
`callgrind_annotate`'s `[object]` suffix — that suffix is elided on
continuation lines and under-counted this allocator ~4× in a previous campaign
(`docs/plans/finished/opscan_v1.md`, estimator 2, DISQUALIFIED). Every run
cross-checks `sum(self costs) == the file's own summary:` line and is printed
`[OK]` or `[MISMATCH]`. One row came back MISMATCH (`glibc-thread`) and is
**excluded from every total below** rather than quoted.

> A parser bug caught by this discipline, recorded so it is not re-introduced:
> callgrind assigns a name-compression id the **first** time a name appears,
> and that first appearance can be on a *call* line (`cfn=`/`cob=`/`cfi=`).
> Skipping call lines — the obvious reading of the format — leaves the largest
> symbols nameless while the totals still reconcile. Totals agreeing is not
> evidence that attribution is right.

**Milliseconds (indicative, and bounded).** Medians of 6, ABBA-interleaved
(ra,mi,mi,ra per repeat), min–max spread printed beside every row. This box
cannot resolve a few-percent wall-clock effect — that is why this repository
makes no wall-clock claim — so **a ms row is a verdict only when the ra/mi gap
is far outside its own spread.** Rows where it is not are marked. Wall clock
appears here because "where does the time go" is a *sizing* question, not a
1%-A/B question, and at 1.37× the instrument resolves fine.

**Four rows in the corpus cannot be ranked by wall clock at all.** `larson`,
`larson-sized`, `rptest` and `xmalloc-test` run for a **fixed duration** — the
5 in `larson 5 8 …` is seconds. Their wall time is an *input*; the ra/mi ratio
of 1.000 they produce is arithmetic, not a result. Their real metric is the
throughput they print, reported separately in §2.1.

**Reproduce:**

```sh
bash bench/opscan.sh              # per-op Ir vs mimalloc (existing)
bash bench/icount-arms.sh         # real programs, Ir (existing)
# the passes this document adds are in §8
```

---

## 2. The corpus in milliseconds

Every mimalloc-bench binary, at the invocation `corpus/sweep-all.sh` uses
(`procs=8`), medians of 6, ABBA. `spread` is (max−min)/median over the ra runs
— the row's own noise floor.

| benchmark               | ra (ms) | mi (ms) |     ra/mi | ra spread | reads as                                                                  |
| ----------------------- | ------: | ------: | --------: | --------: | ------------------------------------------------------------------------- |
| **mleak** ×5            |     334 |     218 | **1.532** |     19.8% | **loss — gap ≫ spread**                                                   |
| **alloc-test** 1 thread |    4515 |    3287 | **1.374** |      5.1% | **loss — gap ≫ spread**                                                   |
| mstress                 |     482 |     425 |     1.134 |    138.4% | not a verdict (spread ≫ gap)                                              |
| sh8bench                |     385 |     367 |     1.049 |     25.7% | not a verdict                                                             |
| barnes                  |    2440 |    2333 |     1.046 |     18.7% | not a verdict                                                             |
| cache-thrash            |     226 |     220 |     1.027 |     21.7% | not a verdict                                                             |
| cfrac                   |    3526 |    3439 |     1.025 |      7.1% | marginal loss                                                             |
| sh6bench                |     122 |     120 |     1.017 |     26.2% | not a verdict                                                             |
| cache-scratch           |     215 |     214 |     1.005 |     12.6% | tie                                                                       |
| glibc-simple            |    1564 |    1564 |     1.000 |     22.6% | tie                                                                       |
| alloc-test 8 threads    |    3558 |    3566 |     0.998 |    101.5% | not a verdict                                                             |
| glibc-thread            |    2009 |    2017 |     0.996 |      0.5% | tie                                                                       |
| espresso                |    5182 |    5206 |     0.995 |      3.3% | tie                                                                       |
| **malloc-large**        |    1684 |    2610 | **0.645** |     13.4% | **win — but unstable, see below**                                         |
| larson                  |    7036 |    7039 |         — |      0.4% | fixed duration — see §2.1                                                 |
| larson-sized            |    7037 |    7044 |         — |      1.6% | fixed duration — see §2.1                                                 |
| rptest                  |   16019 |   16027 |         — |      0.1% | fixed duration — see §2.1                                                 |
| xmalloc-test            |    5017 |    5022 |         — |      0.2% | fixed duration — see §2.1                                                 |

Two rows deserve their caveat stated rather than buried:

- **`malloc-large` is not stable.** The table says 1684 ms; a single run taken
  minutes later under `/usr/bin/time` read **5.45 s for us against 2.87 s for
  mimalloc** — the ratio inverted. Its user/sys split moves with it (ra 4.81 s
  user / 0.61 s sys, mi 1.47 s / 1.38 s) and its fault counts differ by 3×
  (ra 169,877 minor faults, mi 513,544). This benchmark is dominated by
  purge/decommit policy and by whatever the machine's free-page state happens
  to be. **Do not quote either direction until it has its own controlled
  experiment** (§7, W5).
- **`alloc-test` at 8 threads reads 0.998 with a 101% spread** while at 1
  thread it reads 1.374 with a 5% spread. The 8-thread row is noise, not a
  contradiction of the 1-thread row.

### 2.1 The four fixed-duration benchmarks — attempted, and NOT resolved

`larson`, `larson-sized`, `rptest` and `xmalloc-test` each print a throughput,
which is the only number of theirs worth ranking. It was measured twice, three
ABBA repeats each, and **the two passes do not agree**:

| benchmark    | pass A ra/mi | pass B ra/mi | ra absolute, A → B                                                                | verdict        |
| ------------ | -----------: | -----------: | --------------------------------------------------------------------------------- | -------------- |
| larson       |        0.499 |        0.423 | 120.2 M → 89.0 M ops/s                                                            | unstable       |
| larson-sized |        0.456 |        0.510 | 48.8 M → 120.9 M ops/s                                                            | **2.5× swing** |
| rptest       |        0.429 |    **1.102** | 3.25 M → 11.06 M ops/s                                                            | **sign flip**  |
| xmalloc-test |        0.491 |        0.546 | 110.1 M → 183.2 M ops/s                                                           | unstable       |

**No verdict is taken from these rows.** Both passes lean the same way for
three of the four, which is suggestive — but `rptest` inverted between passes
and one arm's absolute throughput moved 2.5×, so the instrument is not
resolving the effect. These are 8-thread benchmarks on an unpinned 24-thread
WSL2 VM, which is exactly the configuration this repository already declines to
quote (`corpus/WSL2.md`, risk R5).

That leaves the four highest-allocation-rate benchmarks in the corpus with **no
admissible time-domain measurement at all** — the wall clock cannot rank them
because their duration is an input, and their throughput needs a quiet pinned
box. What we do have for them is deterministic: `larson` spends **63.05%** of
its instructions in the allocator with `malloc_generic` as its top symbol, and
`xmalloc-test` **72.33%** with `free_general` as its top symbol — the two
signatures of §5.1. Getting them onto a pinned box is §7, W6.

### 2.2 How much of the corpus's wall time is inside the allocator

Wall clock re-measured at **exactly** the configurations the instruction
profile used, so the two columns describe the same run shape and can be
multiplied. `allocator ms ≈ wall × allocator instruction share` — an
approximation that assumes the allocator retires instructions at roughly the
program's average rate. Treat it as sizing, not as a measurement.

| workload                                                                     | ra wall (ms) | ra spread | allocator share |  **≈ allocator ms** |
| ---------------------------------------------------------------------------- | -----------: | --------: | --------------: | ------------------: |
| rptest                                                                       |        16022 |      1.1% |          32.62% |            **5226** |
| alloc-test (1 thr)                                                           |         4750 |     39.7% |          45.52% |            **2162** |
| larson-sized                                                                 |         3022 |      0.3% |          63.81% |            **1928** |
| larson                                                                       |         3021 |      0.2% |          63.05% |            **1905** |
| xmalloc-test                                                                 |         1016 |      1.0% |          72.33% |             **735** |
| sh8bench                                                                     |          939 |     13.1% |          66.14% |             **621** |
| cfrac                                                                        |         3260 |    154.3% |          12.42% |                 405 |
| espresso                                                                     |         5447 |      8.8% |           5.04% |                 275 |
| glibc-simple                                                                 |         1476 |      7.9% |          17.50% |                 258 |
| sh6bench                                                                     |          271 |     11.8% |          64.62% |                 175 |
| mleak (×2)                                                                   |          113 |     55.8% |          93.07% |                 105 |
| perl                                                                         |          257 |     16.3% |           3.72% |                  10 |
| malloc-large                                                                 |         1597 |     11.0% |           0.43% |                   7 |
| lua                                                                          |           94 |     16.0% |           5.33% |                   5 |
| mstress                                                                      |           57 |      8.8% |           3.37% |                   2 |
| sqlite                                                                       |           35 |     37.1% |           1.74% |                   1 |
| barnes                                                                       |         2601 |    142.6% |         0.0004% |                  ~0 |
| cache-thrash                                                                 |           37 |     54.1% |          0.003% |                  ~0 |
| cache-scratch                                                                |           37 |     18.9% |          0.003% |                  ~0 |
| **corpus total**                                                             |   **44,052** |           |                 | **≈13,819 (31.4%)** |
| **excluding fixed-duration rows**                                            |   **20,971** |           |                 |  **≈4,025 (19.2%)** |

**Roughly a third of the corpus's wall time is spent inside this allocator** —
and once the four fixed-duration benchmarks are removed (their wall time being
an input, they contribute a fixed 23 s regardless of how fast anything is),
about a fifth. Either way it is a large enough slice that the §5 findings are
worth the work.

The high-spread rows (`cfrac` 154%, `barnes` 143%, `mleak` 56%, `cache-thrash`
54%, `alloc-test` 40%) had one slow first run each in this pass; their medians
match the §2 pass to within a few percent, and the *share* column — which is
deterministic — is what the product is actually built on.


---

## 3. The corpus in instructions — and how much of it is us

callgrind, ra arm, exact per-symbol attribution, `[OK]` cross-check on every
row. "allocator" = every symbol in the preloaded `.so`. Sorted by the share
that is **ours**, because that is the column that says where work is worth
doing at all.

| workload                                                                |     program Ir |   allocator Ir |                 **allocator share** |
| ----------------------------------------------------------------------- | -------------: | -------------: | ----------------------------------: |
| mleak                                                                   |    125,631,382 |    116,924,914 |                          **93.07%** |
| xmalloc-test                                                            |    300,597,850 |    217,407,814 |                          **72.33%** |
| sh8bench                                                                | 31,262,566,431 | 20,676,162,379 |                          **66.14%** |
| sh6bench                                                                | 12,425,134,295 |  8,029,252,506 |                          **64.62%** |
| larson-sized                                                            |    581,125,876 |    370,826,170 |                          **63.81%** |
| larson                                                                  |  1,033,406,783 |    651,521,373 |                          **63.05%** |
| alloc-test (1 thr)                                                      | 19,392,469,781 |  8,826,948,375 |                          **45.52%** |
| rptest                                                                  |    267,155,784 |     87,145,845 |                              32.62% |
| glibc-simple                                                            | 23,497,657,405 |  4,111,894,159 |                              17.50% |
| cfrac                                                                   | 45,392,802,948 |  5,635,676,135 |                              12.42% |
| lua                                                                     |    610,640,128 |     32,518,002 |                               5.33% |
| espresso                                                                | 30,974,740,199 |  1,562,374,654 |                               5.04% |
| perl                                                                    |    776,706,670 |     28,879,745 |                               3.72% |
| mstress                                                                 |    524,310,914 |     17,688,279 |                               3.37% |
| sqlite                                                                  |    316,959,701 |      5,513,092 |                               1.74% |
| malloc-large                                                            |  1,797,393,607 |      7,688,192 |                               0.43% |
| barnes                                                                  | 20,669,792,210 |         81,101 |                         **0.0004%** |
| cache-thrash                                                            |  2,602,149,767 |         70,979 |                              0.003% |
| cache-scratch                                                           |  2,602,153,307 |         72,823 |                              0.003% |
| ~~glibc-thread~~                                                        |              — |              — | **row VOID (attribution MISMATCH)** |

Threaded rows are profiled at reduced thread counts and durations so a
callgrind run stays bounded (`larson 1 8 …` not `5 8 …`, `sh8bench 4` not `16`,
`mleak 2` not `5`); the share is a property of the mix, not of the count.

**Read the bottom of that table first.** `barnes` spends **0.0004%** of its
instructions in the allocator and still reads 1.046 on the clock; `cache-thrash`
and `cache-scratch` are the same shape. **No change to allocator code can move
those benchmarks.** Whatever their wall difference is, it is memory-system
behaviour — and their fault counts say so out loud (barnes: ra 16,582 minor
faults, mi 2,398). That is §5.4's territory, not the code's.

### 3.1 Head to head, where both arms were profiled

| workload                                |          ra Ir |          mi Ir | whole-program |   ra alloc Ir |   mi alloc Ir | **allocator ratio** |
| --------------------------------------- | -------------: | -------------: | ------------: | ------------: | ------------: | ------------------: |
| **alloc-test**                          | 19,392,469,781 | 12,992,314,160 |     **1.493** | 8,826,948,375 | 2,326,560,430 |           **3.794** |
| **mleak**                               |    125,631,382 |     44,733,626 |     **2.809** |   116,924,914 |    20,796,211 |           **5.622** |
| cfrac                                   | 45,392,802,948 | 43,795,500,200 |         1.036 | 5,635,676,135 | 4,038,228,702 |               1.396 |
| sqlite                                  |    316,959,701 |    316,886,021 |         1.000 |     5,513,092 |     5,523,078 |               0.998 |
| perl                                    |    776,706,670 |    778,153,668 |         0.998 |    28,879,745 |    30,336,466 |               0.952 |
| lua                                     |    610,640,128 |    622,528,866 |         0.981 |    32,518,002 |    44,688,963 |           **0.728** |

The three workloads the README quotes — lua, perl, sqlite — are exactly where
we are at or ahead. They are also, per §3, the workloads where the allocator is
**1.7–5.3%** of the program. **The corpus's verdict trio cannot see a problem
in a component that is 3% of it.** The two workloads where the allocator is
45% and 93% of the program are the two that lose, and neither is in the
README's table.

---

## 4. Inside the allocator: where our instructions actually go

Sum of self cost per symbol over every ra-arm workload above. This is
corpus-weighted, so it is dominated by the biggest benchmarks — the per-workload
view underneath it is the one to act on.

| symbol                                                                                                    |             Ir | share of allocator |
| --------------------------------------------------------------------------------------------------------- | -------------: | -----------------: |
| `free` (exported; carries `free_inline`'s fast path)                                                      | 20,471,113,429 |         **45.35%** |
| `malloc` (exported; carries the `direct[]` fast path)                                                     | 11,544,090,723 |         **25.58%** |
| `Heap::malloc_generic`                                                                                    |  6,847,106,937 |         **15.17%** |
| `Heap::free_local_at`                                                                                     |  1,921,169,347 |              4.26% |
| `alloc::free` (outlined, internal callers)                                                                |  1,408,206,894 |              3.12% |
| `alloc::free_general`                                                                                     |  1,221,094,828 |              2.71% |
| `mi_new`                                                                                                  |    999,650,543 |              2.21% |
| `alloc::malloc_slow`                                                                                      |    172,231,536 |              0.38% |
| `alloc::retire_or_abort`                                                                                  |    111,940,883 |              0.25% |
| `Heap::adopt_segment`                                                                                     |     67,763,115 |              0.15% |
| `segment::span_alloc` / `span_free`                                                                       |    101,987,941 |              0.23% |
| `alloc::realloc`                                                                                          |     34,497,768 |              0.08% |
| `init::thread_done`                                                                                       |     30,084,726 |              0.07% |
| `Heap::collect_inner`                                                                                     |     29,653,465 |              0.07% |
| `Heap::malloc_aligned_at`                                                                                 |     20,445,314 |              0.05% |

**`malloc_generic` at 15.17% of the allocator is the anomaly.** It is the slow
path. On a healthy allocator it should be a rounding error — on mimalloc, over
the same workloads, its counterpart is. §5.1 is that number's explanation.

### 4.1 The same map, per workload — the top symbol tells you the workload's disease

| workload      | top allocator symbol      |                     Ir | what it means                                                              |
| ------------- | ------------------------- | ---------------------: | -------------------------------------------------------------------------- |
| alloc-test    | `Heap::malloc_generic`    |          4,287,418,321 | **slow path taken 70% of the time**                                        |
| mleak         | `Heap::adopt_segment`     |             67,256,323 | **thread churn: 57.5% of our cost**                                        |
| xmalloc-test  | `alloc::free_general`     |             84,093,168 | cross-thread frees miss the fast path                                      |
| larson        | `Heap::malloc_generic`    |            329,681,182 | same disease as alloc-test                                                 |
| rptest        | `Heap::malloc_aligned_at` |             20,445,314 | the aligned path                                                           |
| sh8bench      | `free`                    |         11,690,486,978 | healthy — fast path, in proportion                                         |
| sh6bench      | `free`                    |          4,962,044,967 | healthy                                                                    |
| glibc-simple  | `free`                    |          2,591,958,279 | healthy                                                                    |
| espresso      | `free`                    |            896,052,078 | healthy                                                                    |
| lua           | `alloc::realloc`          |             15,109,242 | realloc-shaped workload                                                    |
| perl / sqlite | `free`                    | 16,208,457 / 3,274,812 | healthy                                                                    |

A workload whose top symbol is `free` or `malloc` is spending its time on the
fast path, which is what should happen. A workload whose top symbol is
`malloc_generic`, `free_general` or `adopt_segment` is one where we are missing
the fast path — and those are exactly the workloads that lose.

---

## 5. The four findings

### 5.1 `alloc-test`: we take the slow path 70% of the time. mimalloc takes it 1.1%.

**This is the largest loss in the corpus and it has a named mechanism.**

`alloc-test` is the only benchmark here that models a *realistic* load —
100 M operations, **262,144 live objects** (`maxItems = 1 << 18` — the
benchmark's own comment beside that line says "512k objects" and is wrong),
random sizes 5–1024 B on a Pareto-ish distribution, every block written, seed pinned at 41
(`bench/alloc-test/allocator_tester.cpp`). Everything else in the corpus is
either a churn loop over one size class or a memory-behaviour test.

**Work parity is exact, not argued:** both arms performed **50,130,729**
allocations.

|                                                                                                   |    rusty_alloc |       mimalloc |     ratio |
| ------------------------------------------------------------------------------------------------- | -------------: | -------------: | --------: |
| program instructions                                                                              | 19,392,469,781 | 12,992,314,160 | **1.493** |
| allocator instructions                                                                            |  8,826,948,375 |  2,326,560,430 | **3.794** |
| allocator share of program                                                                        |         45.52% |         17.91% |           |
| wall clock (median of 6)                                                                          |        4515 ms |        3287 ms | **1.374** |
| **generic/slow path entries**                                                                     | **35,035,882** |    **564,856** | **62.0×** |
| … as a share of allocations                                                                       |      **69.9%** |      **1.13%** |           |
| cost *per* generic call                                                                           |       122.4 Ir |       113.3 Ir |      1.08 |
| generic-path frees (`free_general`)                                                               |     19,091,816 |         20,682 |  **923×** |

**It is a frequency problem, not a cost problem.** Our generic path costs 122
instructions per call against mimalloc's 113 — near parity. We simply enter it
62 times more often. All the tuning in `docs/opps.md` sharpened the *cost* of
paths; none of it could have found this, because nothing in the existing
instrument set enters this regime.

**It is not a cache problem either — the obvious hypothesis, refuted.**
Cachegrind on the same run:

|                                                                                                                     | rusty_alloc |    mimalloc |
| ------------------------------------------------------------------------------------------------------------------- | ----------: | ----------: |
| D1 misses                                                                                                           |  91,807,291 | 126,049,455 |
| D1 miss rate                                                                                                        |        1.7% |        4.6% |
| LL misses                                                                                                           |     150,475 |     160,223 |

We take **fewer** absolute cache misses than mimalloc and still lose. The extra
wall time is the extra instructions, and nothing else.

**It is not a regression.** The same benchmark under the pre-audit build
(`5eb1ceb`, 0.7.0, before any hardening) reads **19,562,149,691** program /
**8,996,630,032** allocator instructions — HEAD is **0.87% better**. This loss
has been there the whole time; it has simply never been measured, because the
corpus's per-op scan cannot reach it (see §5.1.2).

#### 5.1.1 The mechanism, line by line

The line-level profile inside `Heap::malloc_generic`
([heap.rs:296-370](../../crates/rusty_alloc/src/heap.rs#L296-L370)), per
35,035,882 calls:

| line                                                                                 |     executions | reading                                 |
| ------------------------------------------------------------------------------------ | -------------: | --------------------------------------- |
| `let mut p = (*q).first;`                                                            |     35,035,887 | one queue walk per call                 |
| `while !p.is_null()`                                                                 |    108,255,404 | **3.09 pages walked per call**          |
| `let next = (*p).next;` (the park branch)                                            | **19,091,815** | **19.1 M pages parked as full**         |
| `page_extend(p, (*p).area)`                                                          |      **1,059** | extension essentially never happens     |
| `self.stats.pages_fresh += 1`                                                        |        **124** | only 124 pages carved in the entire run |

19,091,815 park events, and 19,091,816 `free_general` calls — the same number,
because they are the same event seen from both ends. **The allocator is
thrashing a page across the full/not-full boundary nineteen million times.**

The cycle, and it is one function:
[`Heap::free_local_at`](../../crates/rusty_alloc/src/heap.rs#L1094-L1104) —

```rust
if (*pg).flags.load(Ordering::Relaxed) & pflags::IN_FULL != 0 {
    (*pg).flags.fetch_and(!pflags::IN_FULL, Ordering::Relaxed);
    queue_remove(&raw mut self.pages[BIN_FULL], pg);
    page_set_flag(pg, XFLAG_NORMAL);
    let bin = (*pg).bin as usize;
    queue_push_front(&raw mut self.pages[bin], pg);   // <-- FRONT
    self.update_direct(bin);                          // <-- and the fast path
}
```

A page that was full receives **one** freed block, and we (a) push it to the
**front** of its bin queue and (b) point `direct[]` — the malloc fast path —
straight at it. The next allocation of that size class pops that single block,
the page is full again, the allocation after it misses, `malloc_generic` walks
the queue and parks the page again. Park → free → unpark-to-front → one alloc →
park.

**Upstream solved this deliberately, and the comment is in their source.**
`_mi_page_unfull` calls `mi_page_queue_enqueue_from_full(pq, pqfull, page)`,
which passes **`enqueue_at_end = true`** — and the alternative branch in
`mi_page_queue_enqueue_from_ex` is labelled *"enqueue at 2nd place"*
(`oracle/mimalloc/src/page-queue.c:317-380`). Upstream will put a just-unfulled
page **at the end**, or at second place, but **never at the front** — precisely
so the queue head, and therefore `pages_free_direct`, keeps pointing at a page
with blocks to spare. We are the only one of the two that promotes a page with
one free block to be the fast path.

#### 5.1.2 Why every existing instrument is blind to this

`bench/opscan.sh` measures ops that are steady-state on **one page of one size
class** with a tiny live set: `op_pair` allocates and frees the same block;
`op_batch` cycles 64 blocks. In that regime the direct page always has blocks,
the generic path is never entered, and the un-park cycle cannot occur. That is
why every op of the scan reads at-or-below mimalloc except batch, while the workload with 262 k live
objects reads 3.79×. **The scan is not wrong; its coverage has a hole exactly
where realistic allocation lives.**

---

### 5.2 `mleak`: thread churn costs us 2.8× — and it is `adopt_segment`

`mleak` creates and destroys ten OS threads per round, each doing one
`calloc`+`free`, for `ITER*100` rounds. It is a pure thread-lifecycle test, and
the allocator is **93.07%** of its instructions — the highest share in the
corpus.

**Note the two configurations:** the instruction rows below are `mleak 2`
(2,000 threads, so a callgrind run stays bounded); the wall-clock and resource
rows are `mleak 5` (5,000 threads), which is what `sweep-all.sh` runs. They are
different runs of the same shape and are labelled as such rather than mixed
into one column.

|                                                                                                          | rusty_alloc |   mimalloc |     ratio |
| -------------------------------------------------------------------------------------------------------- | ----------: | ---------: | --------: |
| program instructions (`mleak 2`)                                                                         | 125,631,382 | 44,733,626 | **2.809** |
| allocator instructions (`mleak 2`)                                                                       | 116,924,914 | 20,796,211 | **5.622** |
| wall clock (`mleak 5`)                                                                                   |      334 ms |     218 ms | **1.532** |
| system time (`mleak 5`)                                                                                  |      0.34 s |     0.18 s |      1.89 |
| voluntary context switches (`mleak 5`)                                                                   |       4,242 |        632 |  **6.71** |
| minor faults (`mleak 5`)                                                                                 |      15,547 |      6,127 |      2.54 |
| peak RSS (`mleak 5`)                                                                                     |   7,440 KiB | 14,428 KiB |      0.52 |

Where it goes, on our side:

| symbol                                                                                               |         Ir | share of our allocator cost |
| ---------------------------------------------------------------------------------------------------- | ---------: | --------------------------: |
| `Heap::adopt_segment`                                                                                | 67,256,323 |                   **57.5%** |
| `init::thread_done`                                                                                  | 29,700,510 |                   **25.4%** |
| `Heap::collect_inner`                                                                                | 15,747,530 |                       13.5% |
| `init::create_heap`                                                                                  |  1,590,804 |                        1.4% |

> **Superseded in part by §9** — these are the numbers that opened the
> campaign; §9 records what twenty measured primitives did to them.

**83% of the cost is in two functions on the thread create/destroy path**, and
the context-switch count says a meaningful part of the remainder is kernel
work we are asking for and mimalloc is not. We use **half** mimalloc's RSS
here, so this is a speed/footprint trade currently taken at the wrong end.
`adopt_segment` is the abandon/adopt protocol — the same machinery the loom
model covers — so this is delicate work rather than a quick win, but 5.6× on
the allocator's own share is a large prize.

---

### 5.3 The hardening tax: real, and it is **one commit**

The question behind this document was "we just did a big security update and
lost performance". Answer: **yes — 1 to 6 instructions per operation, and 100%
of it lands in a single commit.** Three builds of the same crate, same release
profile (`lto=thin`, `codegen-units=1`, verified identical at both revisions),
measured in one session with the same driver binary:

| op                                    | 0.7.0 `5eb1ceb` | post-hardening `757c2a1` | HEAD 1.0.0 | mimalloc | **hardening cost** | perf campaign |
| ------------------------------------- | --------------: | -----------------------: | ---------: | -------: | -----------------: | ------------: |
| small                                 |           77.39 |                    79.38 |      79.37 |   111.41 |          **+1.99** |         −0.01 |
| med                                   |           85.75 |                    87.62 |      87.56 |   123.01 |          **+1.87** |         −0.06 |
| big                                   |          171.00 |                   171.00 |     170.00 |   222.02 |               0.00 |         −1.00 |
| calloc                                |          151.50 |                   152.62 |     131.94 |   160.89 |              +1.12 |    **−20.68** |
| batch_lifo                            |           59.19 |                    60.17 |      60.16 |    59.70 |          **+0.98** |         −0.01 |
| realloc                               |          388.03 |                   393.69 |     379.36 |   499.82 |          **+5.66** |    **−14.33** |
| aligned                               |          163.25 |                   169.25 |     169.19 |   187.89 |          **+6.00** |         −0.06 |
| mixed                                 |          140.26 |                   140.07 |     139.35 |   157.70 |              −0.19 |         −0.72 |

Walking the campaign commit by commit isolates it completely:

| build    | commit        |     small |    aligned | batch_lifo |                                                                                |
| -------- | ------------- | --------: | ---------: | ---------: | ------------------------------------------------------------------------------ |
| pre      | `5eb1ceb`     |     77.39 |     163.25 |      59.19 | 0.7.0, before the audit                                                        |
| **tsan** | **`161717a`** | **79.38** | **169.25** |  **60.17** | **TSan atomic page flags**                                                     |
|          | *delta*       | **+1.99** |  **+6.00** |  **+0.98** | ← the entire tax                                                               |
| link     | `e8f8902`     |     79.38 |     169.25 |      60.17 | +0.00 / +0.00 / +0.00                                                          |
| bmap     | `69f894a`     |     79.38 |     169.25 |      60.17 | +0.00 / +0.00 / +0.00                                                          |
| sec      | `757c2a1`     |     79.38 |     169.25 |      60.17 | +0.00 / +0.00 / +0.00                                                          |

Every hardening commit after the ThreadSanitizer fix is **exactly free** —
byte-for-byte zero across all three ops. The link check, the blockmap, the
side-channel work, the four-crate audit: no cost at all in the default build,
as designed.

**Two things follow, and one of them is a correction to the README.**

1. `README.md` says the atomic-flags fix "costs **exactly one instruction**".
   That is true of `batch_lifo` (+0.98) and of nothing else. It costs
   **+1.99 on small, +1.87 on med, +5.66 on realloc, +6.00 on aligned** — the
   flags byte is read more than once on those paths, and each read LLVM can no
   longer fold into a `test`'s memory operand costs its own instruction. The
   claim was measured on one op and generalised to all of them.
2. **The tax is what flipped `batch` from a win to a loss.** At 0.7.0 batch_lifo
   was **59.19 against mimalloc's 59.70 — we were ahead.** The +0.98 put us at
   60.16, behind. The repository explains this as an accepted correctness
   trade, which it is; what it does not say is that part of the trade is
   avoidable, because the same byte is loaded repeatedly (§7, W2).

The 2026-08-20 perf campaign gave back **calloc (−20.68)** and **realloc
(−14.33)** — both large wins — but it never touched small, med, aligned or
batch, which is where the tax actually sits.

---

### 5.4 Transparent huge pages: we get none. Priced honestly, it is 0–7%.

On this box `/sys/kernel/mm/transparent_hugepage/enabled` is **`[madvise]`** —
a mapping gets 2 MiB pages only if the allocator asks with
`madvise(MADV_HUGEPAGE)`. mimalloc asks (`mi_option_allow_thp`, default **on**,
`oracle/mimalloc/src/prim/unix/prim.c:398-406`). **We never call `madvise` with
`MADV_HUGEPAGE` anywhere** — `crates/rusty_alloc/src/prim/unix.rs` uses it only
for `MADV_DONTNEED` / `MADV_FREE`.

Sampled from `/proc/<pid>/smaps_rollup` mid-run:

| workload   | arm                                                                                  |       Rss | Anonymous |   **AnonHugePages** |
| ---------- | ------------------------------------------------------------------------------------ | --------: | --------: | ------------------: |
| alloc-test | rusty_alloc                                                                          | 12,984 kB |  8,872 kB |            **0 kB** |
| alloc-test | mimalloc                                                                             | 16,496 kB | 12,636 kB | **12,288 kB (97%)** |
| larson     | rusty_alloc                                                                          | 64,576 kB | 60,312 kB |            **0 kB** |
| larson     | mimalloc                                                                             | 76,912 kB | 72,840 kB |       **28,672 kB** |

This is also why our minor-fault counts run 2–150× mimalloc's across the corpus
(sh8bench: ra 40,548 vs mi 266; barnes: 16,582 vs 2,398; alloc-test: 2,338 vs
203) — 4 KiB pages against 2 MiB ones.

**And then it was priced, which is the part that matters.** mimalloc has its
own switch, so turning THP *off* in the arm that has it prices exactly the
thing we are missing — with no system setting touched:

| workload                                            | mi with THP | mi without THP |   **what THP is worth to mimalloc** | ra vs mi-without-THP |
| --------------------------------------------------- | ----------: | -------------: | ----------------------------------: | -------------------: |
| alloc-test                                          |     3161 ms |        3073 ms | **0.972 — none; slightly negative** |            **1.416** |
| cfrac                                               |     3241 ms |        3282 ms |                               1.013 |                1.038 |
| sh8bench                                            |      321 ms |         339 ms |                               1.056 |                1.027 |
| mstress                                             |      378 ms |         404 ms |                               1.069 |                1.062 |

**The beautiful hypothesis is refuted.** Take huge pages away from mimalloc
entirely and it is *still* 1.42× faster than us on alloc-test. THP is worth
**5–7% on thread-heavy workloads and nothing at all on the one where we lose
most.** It is a real, cheap, bounded win (§7, W3) and it must not be sold as
the answer to §5.1.

---

## 6. Found on the way, and not a performance issue: `linkcheck` does not compile

```
$ cargo check -p rusty_alloc --features linkcheck
error[E0061]: this function takes 2 arguments but 3 arguments were supplied
   --> crates/rusty_alloc/src/page.rs:249:37
```

[page.rs:249](../../crates/rusty_alloc/src/page.rs#L249) calls
`link_is_plausible(n.addr(), b.addr(), extent)`; the function at
[page.rs:221](../../crates/rusty_alloc/src/page.rs#L221) takes two arguments.
The arm was introduced in `69f894a` and has never built.

**Why CI is green anyway, which is the transferable part.** CI runs
`--all-features`. With `--all-features`, `secure` is on, so the entire
`#[cfg(not(feature = "secure"))]` block containing the broken call is compiled
out. **`--all-features` is not a feature-combination test** — it is one point in
the lattice, and it is the point that hides this. The feature ships documented
in `Cargo.toml` as the way to "isolate the bound so its cost can be measured on
its own"; that measurement cannot have been taken at this revision.

---

## 7. The plan

Ranked by measured prize per unit of risk. Nothing here is implemented; every
item states the number it must move and the gate that decides it, per the house
rule that a change which reads flat or negative gets reverted **and its
refutation recorded**.

### W0 (do first — it is the gate for W1) · An `opscan` op that has a live set — **EXECUTED, see §12**

**Why first.** §5.1.2: the entire existing instrument set is structurally
blind to the largest loss in the corpus. Landing W1 against no standing gate
means the next campaign re-loses it silently.

Add to `bench/opscan.c` an op in the regime that actually breaks us:

```c
/* op_liveset: L live objects, random size, random-position replace.
   The regime opscan has never had: many pages per bin, long queues,
   pages crossing the full/not-full boundary constantly. */
static void op_liveset(long iters, size_t live, size_t maxsz)
```

with `L` around 2^18 and sizes 8–1024 to mirror `alloc-test`. Expect it to read
**~3.8× mimalloc on day one** — that is the point. Cost: an afternoon.

### W1 · Do not promote a one-block page to the fast path — **EXECUTED, see §12**

- **Prize** — slow-path entry rate **69.9% → target ~1%**; up to **6.5 G Ir**
  on `alloc-test` (≈33% of that program's total instructions); the **1.374×**
  wall gap
- **Where** —
  [`Heap::free_local_at`](../../crates/rusty_alloc/src/heap.rs#L1094-L1104)
- **Change** — on un-park, enqueue the page at the **back** of its bin queue
  (mimalloc's `enqueue_at_end = true`) instead of `queue_push_front`, and drop
  the `update_direct(bin)` that repoints the malloc fast path at it
- **Risk** — `direct[]` invariants. `update_direct` exists partly to stop
  `direct[]` pointing at a page that has left the queue; removing the call
  needs the sentinel/`direct[]` contract re-read and, if necessary, the entry
  pointed at the empty-page sentinel instead of at this page. This is a
  correctness-sensitive edit on the hottest structure in the allocator — it is
  not a one-liner even though it looks like one.

**Ceiling probe before anything else**, in a throwaway worktree, exactly the
discipline `codec-experimental` calls for — prove the prize offline before
integrating:

```sh
git worktree add --detach ~/w1 HEAD     # edit free_local_at there, never in the tree
CARGO_TARGET_DIR=~/t_w1 cargo build --release -p rusty_alloc-override
LD_PRELOAD=~/t_w1/release/librusty_alloc_override.so \
  valgrind --tool=callgrind --cache-sim=no --branch-sim=no \
  corpus/mimalloc-bench/out/bench/alloc-test 1
# target: allocator Ir 8.83 G -> ~2.3 G, generic entries 35.0 M -> ~0.6 M
```

**Gates:** `bench/opscan.sh` all 13 ops — `small` / `batch_lifo` / `mixed`
should come back **byte-identical** (they never enter this path, so a change
there means the edit did something else); the new W0 op; `corpus/sweep-all.sh`
19/19; `corpus/realworld.sh` checksums unchanged; loom cross-thread model;
Miri; `stress_mt` 30/30.

### W2 · Recover the hardening tax that is recoverable

- **Status** — **EXECUTED 2026-08-21 — see §10**, though not by the route
  planned here. The atomic-flags reads were never hoisted; instead the
  keep-one-warm decision moved off the heap and onto the page, which took
  realloc −94.95 Ir/op and small −20.05, and put **every op of the scan ahead
  of mimalloc — batch_lifo included**, at 59.53 against 59.70.
- **Prize (original)** — up to **+6.00 aligned, +5.66 realloc, +1.99 small,
  +1.87 med, +0.98 batch_lifo** Ir/op
- **Where** — the four reads of the atomic flags byte:
  [alloc.rs:26](../../crates/rusty_alloc/src/alloc.rs#L26),
  [alloc.rs:557](../../crates/rusty_alloc/src/alloc.rs#L557),
  [alloc.rs:676](../../crates/rusty_alloc/src/alloc.rs#L676),
  [heap.rs:1095](../../crates/rusty_alloc/src/heap.rs#L1095)
- **Change** — read the byte **once** per operation and thread the value
  through, instead of re-loading it at each decision point. The `+6.00` on
  `aligned` is the shape of a path that loads it several times.

**This does not weaken the ThreadSanitizer fix.** What TSan required is that the
byte be *atomic*; the cost is that LLVM will not fold an atomic load into a
`test`'s memory operand, so each additional read costs its own instruction.
Fewer reads of the same atomic byte is strictly less racy, not more. Gate:
re-run ThreadSanitizer (`tsan-full.log` shape) plus opscan; a TSan report of any
kind reverts the change.

### W3 · Ask for transparent huge pages

- **Prize** — **5–7% wall on thread-heavy workloads** (sh8bench 1.056, mstress
  1.069), **0% on alloc-test** — priced in §5.4, do not oversell it. Also
  2–150× fewer minor faults, which is a real operational property in its own
  right.
- **Where** — `crates/rusty_alloc/src/prim/unix.rs`, after commit
- **Change** — `madvise(ptr, len, MADV_HUGEPAGE)` on segment/arena reservations
  of ≥2 MiB, behind an option mirroring upstream's `mi_option_allow_thp`
  (default on, off for Android as upstream does). Verify with `AnonHugePages`
  in `smaps_rollup` — that is the acceptance test, not the clock.
- **Risk** — RSS. Huge pages round allocation up; mimalloc's RSS is 20–30%
  above ours on several rows and this is part of why. Gate on
  `bench/rss-scaling.sh` and the 6-minute thread-churn soak as well as on
  speed.

### W4 · Thread churn: `adopt_segment`

- **Prize** — `mleak` allocator instructions **5.62×** mimalloc's; wall
  **1.53×**; `adopt_segment` alone is **57.5%** of our cost there
- **Status** — **EXECUTED 2026-08-21 — see §9.** The allocator's share of
  `mleak` fell 24.4% (116.9 M -> 88.4 M Ir) and the ratio to mimalloc 5.62x ->
  4.25x. Root cause was NOT one thing: it was a redundant second slice walk,
  two CAS loops where one suffices, index arithmetic in three walks, and a
  per-thread `bin_size` loop. What remains is genuinely per-live-page work.
  opps #7 already bounded the 512-slot release scan; that landed and this
  remains.
- **First question** — is the cost per adoption, or the number of adoptions?
  The same "how often vs how much" split that decided §5.1 in one measurement —
  `bench/callcount.sh` against `bench/opprofile.sh`. Answer that before
  proposing anything.

Secondary signal to carry into it: 6.71× the voluntary context switches and
1.89× the system time, so part of this is syscalls, not instructions.

### W5 · `malloc-large` is not measurable as it stands

It read **0.645× (a large win)** in the ABBA pass and **1.90× (a large loss)**
minutes later. Its allocator share is **0.43%**, so almost none of its time is
our code — it is purge/decommit policy against the machine's free-page state.
Until it has a controlled experiment (fixed `purge_delay`, drop-caches or
warm-state control, ≥31 repeats) **no direction should be quoted from it**,
including the flattering one.

### W6 · Repair the instruments

1. **Fixed-duration benchmarks:** `larson`, `larson-sized`, `rptest`,
   `xmalloc-test` are timed by the harness but their wall time is an *input*.
   Capture the throughput they print (§2.1) in the standing harness so those
   four corpus rows mean something.
2. **Attribution cross-check:** the `sum(self) == summary:` check that voided
   `glibc-thread` here belongs in the harness permanently. A profile that does
   not reconcile is not a profile.
3. **Feature-combination CI.** `--all-features` hid a feature that has never
   compiled (§6). Add `--no-default-features`, each feature alone, and the
   documented pairs (`secure+blockmap`, `linkcheck` alone).
4. **Fix `linkcheck`** — either restore the extent-aware three-argument
   predicate or drop the third argument; then take the measurement its
   `Cargo.toml` comment promises.

### What NOT to do

- **Do not chase `batch_lifo`'s remaining gap through codegen.** `docs/opps.md`
  #6 closed that as a Rust-vs-C instruction-selection floor and the refutation
  holds. W2 attacks a different instruction — the redundant atomic read — and
  is worth more than the fold ever was.
- **Do not sell THP as the fix for §5.1.** It was priced at 0.972 there. It
  buys thread-heavy workloads 5–7% and that is all it buys.
- **Do not re-run `opscan` expecting §5.1 to appear.** It structurally cannot
  (§5.1.2). That is what W0 is for.
- **Do not read `barnes`, `cache-thrash` or `cache-scratch` as allocator
  results.** The allocator is 0.003% or less of their instructions.

---

## 8. Reproducing this

Every pass here is a script; the ones this document adds are listed so the next
person re-runs rather than re-derives.

| pass                   | what it produces                                                                                                       |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `bench/opscan.sh`      | per-op Ir vs mimalloc (existing)                                                                                       |
| `bench/icount-arms.sh` | lua / perl / sqlite Ir (existing)                                                                                      |
| `bench/opprofile.sh`   | per-function Ir for one op, both arms (existing)                                                                       |
| **corpus wall pass**   | §2 — every benchmark, ABBA, medians of 6, spread printed                                                               |
| **corpus Ir pass**     | §3/§4 — raw-callgrind attribution with the `sum(self)==summary` cross-check                                            |
| **call-count pass**    | §5.1 — `calls=` per callee: the "how often" half                                                                       |
| **line pass**          | §5.1.1 — `callgrind_annotate --auto=yes` inside `malloc_generic`                                                       |
| **tax bisect**         | §5.3 — three/five builds from git worktrees, one session, same driver                                                  |
| **THP price**          | §5.4 — `MIMALLOC_ALLOW_THP=0` against `=1`, plus `smaps_rollup`                                                        |

Two method notes worth keeping:

- **Wall-clock passes must not overlap a callgrind run.** One pass in this
  campaign was discarded and re-taken for exactly that reason; instruction
  counts do not care about load, medians of six do.
- **Rebuild the override `.so` from HEAD before every arm.** The one on this
  box was stale when the campaign opened.

## 9. Campaign executed: 20 primitives against the thread-lifecycle path

**Date:** 2026-08-21 · **Target:** §5.2's finding — `mleak` spends 93% of its
instructions in the allocator and 83% of that in two functions · **Instrument:**
callgrind Ir on `mleak 2`, rebuilt from source before every arm, exact
attribution with the `sum(self) == summary:` cross-check on every reading.

Twenty primitives were identified from the per-line profile and **all twenty
were executed and measured**. Eleven won, four were refuted (two of them
regressions, and both are now recorded in the code so they are not retried),
four read flat because LLVM had already done them, and one was assessed and
declined before implementation.

### Result

|                                                                                       |      before |          after |                   change |
| ------------------------------------------------------------------------------------- | ----------: | -------------: | -----------------------: |
| `mleak 2` program instructions                                                        | 125,633,259 | **95,902,328** | **−29,730,931 (−23.7%)** |
| `mleak 2` allocator instructions                                                      | 116,924,914 | **88,391,659** | **−28,533,255 (−24.4%)** |
| `Heap::adopt_segment`                                                                 |  67,256,323 | **43,243,986** | **−24,012,337 (−35.7%)** |
| `init::thread_done`                                                                   |  29,706,510 | **26,757,533** |   **−2,948,977 (−9.9%)** |
| `Heap::collect_inner`                                                                 |  15,747,530 | **15,504,655** |         −242,875 (−1.5%) |
| `init::create_heap`                                                                   |   1,591,599 |    **318,327** |  **−1,273,272 (−80.0%)** |
| allocator ratio vs mimalloc                                                           |      5.622× |     **4.251×** |                          |
| whole-program ratio vs mimalloc                                                       |      2.809× |     **2.144×** |                          |

Wall clock on `mleak 5` moved the same way — 334 → 322 ms, ratio 1.532 → 1.406
— but its spread is 302–376 ms, **wider than the change**, so the clock
confirms the direction and does not measure the size. The instruction counts do.

### The twenty, in the order they were executed

| #   | primitive                                                                           |              Ir | verdict                           |
| --- | ----------------------------------------------------------------------------------- | --------------: | --------------------------------- |
| 1   | `adopt_segment`: skip walk 2 unless walk 1 saw a large span                         | **−16,724,748** | **win (−24.9% of adopt)**         |
| 2   | `adopt_segment`: walk by running pointer, not `pages[idx]`                          |  **−1,965,141** | **win**                           |
| 3   | `adopt_segment`: drop two dead `next`/`prev` stores                                 |    **−985,868** | **win**                           |
| 4   | `page_collect` + `page_set_flag` fused into ONE CAS (adopt)                         |    **−978,580** | **win**                           |
| 5   | `thread_done`: walk by running pointer                                              |  **−1,966,850** | **win**                           |
| 6   | `thread_done`: the same fused CAS                                                   |    **−982,127** | **win**                           |
| 7   | `Heap::new`: page-queue table becomes a `const`                                     |  **−1,273,272** | **win (−80% of create_heap)**     |
| 8   | `adopt_segment`: hoist the doubled `(*slot).bin` load                               |  **−1,966,544** | **win**                           |
| 8b  | the running pointers advance with `wrapping_add`                                    |               0 | correctness repair, free          |
| 9   | `update_direct`: read `PageQueue::block_size`, don't recompute                      |    **−746,257** | **win**                           |
| 10  | `update_direct`: same for the `w_lo` bound                                          |    **−317,740** | **win**                           |
| 11  | `adopt_segment`: rebuild only the bins the segment touched                          |  **+3,745,534** | **REFUTED — regression**          |
| 12  | `adopt_segment`: bound the rebuild to `direct[]`-reaching bins                      |    **−436,128** | **win**                           |
| 13  | `adopt_segment`: hoist `self.delayed` out of the walk                               |               0 | flat — LLVM already hoisted       |
| 14  | `Heap::new`: `direct[]` table as a `const` blob                                     |    **+128,999** | **REFUTED — regression**          |
| 15  | `collect_inner`: walk bins by running queue pointer                                 |    **−190,000** | **win**                           |
| 16  | `adopt_segment`: load-guard the locked `fetch_and(!IN_FULL)`                        |  **+1,966,544** | **REFUTED — regression**          |
| 17  | `page_collect`: `#[inline]` on the forwarding wrapper                               |               0 | flat — already inlined            |
| 18  | fused loop: mask `x` once and reuse it                                              |               0 | flat — already CSE'd              |
| 19  | `collect_inner`: hoist the queue-end loads out of the test                          |        **+125** | **REFUTED — regression**          |
| 20  | the sibling walk in `heap_destroy` gets 2 and 5's fix                               |    0 on `mleak` | applied — path not exercised here |
| —   | `.max(1)` on `slice_count` (2.95 M in adopt, 2.96 M in `thread_done`)               |               — | **assessed, declined**            |

### What the refutations teach

**Per-page work never pays for per-segment work (#11, #16, #19).** Three of the
four regressions are the same mistake in different clothes: adding a test to a
loop that runs once per PAGE in order to save work that runs once per SEGMENT.
#11 added two compares per queued page (~1.97 M of them) to skip a rebuild loop
costing ~444 k. #16 added a load and a branch per page to skip one `lock and`
— **+1,966,544 Ir, exactly one instruction per page**, reproducing the
`fetch_or(HAS_ALIGNED)` refutation already banked in `docs/opps.md` #9 for a
different leaf op. #19 loaded both queue ends eagerly and defeated a
short-circuit. The direction that *does* pay is bounding a loop's END (#12),
which is per-segment work removed by per-segment reasoning.

**Measure the program, not your symbol (#14).** Hoisting `direct[]` into a
`const` blob cut **126 k Ir out of this crate's symbols** and added **129 k to
libc's `memcpy`**. The allocator's own profile improved and the process got
slower. `EMPTY_QUEUES` (#7) wins precisely because its elements are *computed*
rather than repeated — there is real arithmetic to hoist to compile time, not
just a fill LLVM already turns into a tight store loop.

**Four of twenty were already done by the compiler (#13, #17, #18, and the
`.max(1)` variants).** Hoisting an invariant, inlining a forwarder, and
common-subexpression-eliminating a mask are all things LLVM does unprompted.
Reading the emitted profile before proposing the fix is what separates the
eleven that paid from these four.

**`.max(1)` — assessed and declined, so it is not re-proposed.** It costs 2.95 M
Ir in `adopt_segment` and 2.96 M in `thread_done` — the third-largest line in
both — and it is at its floor. Every cheaper formulation ties on instruction
count (`cmp; cmov` vs `test; sete; add` vs `test; jz`), and the only form that
is genuinely cheaper — trusting `slice_count >= 1` and dropping the guard —
turns a corrupt `slice_count` into an **infinite loop** in an allocator whose
stated property is that it aborts on corruption rather than misbehaving. A hang
is a worse failure mode than the two instructions are worth.

### Gates run

- `cargo clippy --workspace --all-targets --all-features` — clean
- `cargo test --workspace --all-features` — all suites pass, including
  `stress_mt`, `teardown_reclaim`, `abandon_rss`, `corruption`, `double_free`
- `corpus/sweep-all.sh 8` — **19/19** benchmarks complete under rusty_alloc
- `bench/opscan.sh` ops, against the pre-campaign HEAD numbers:

| op                                                                     | before |      after |                                            delta |
| ---------------------------------------------------------------------- | -----: | ---------: | -----------------------------------------------: |
| small                                                                  |  79.37 |      79.37 |         +0.00 (fast path untouched, as intended) |
| med                                                                    |  87.56 |  **86.50** |                                        **−1.06** |
| big                                                                    | 170.00 | **161.00** |                                        **−9.00** |
| calloc                                                                 | 131.94 | **130.88** |                                        **−1.06** |
| batch_lifo                                                             |  60.16 |      60.17 | +0.01 — the two-point estimator's own resolution |
| realloc                                                                | 379.36 | **376.72** |                                        **−2.64** |
| aligned                                                                | 169.19 | **168.12** |                                        **−1.07** |
| mixed                                                                  | 139.35 | **132.38** |                                        **−6.97** |

- real programs: **perl −71,048 Ir**, **sqlite −10,615 Ir** (both deterministic,
  both verdicts). lua read +275,991 — but three repeats of the *same* binary
  span 609,217,167 … 610,794,210, a spread of 1.58 M, so that number is noise
  and lua stays what its harness already calls it: indicative, not a verdict.

The work reached beyond its target: six of the eight scanned ops improved
because `page_collect`, `update_direct` and the fused CAS sit on the generic
allocation path too, not only on thread teardown.

### Still open on this path

`adopt_segment` is still **43.2 M Ir over 2,596 adoptions — ~16,650 per
adoption**, and the per-line profile now shows that cost is genuinely
per-live-page work (the fused CAS, `queue_push_front`, and the walk itself),
not waste. The remaining lever is not shaving those instructions but asking
**why an adopted segment carries so many live pages**, which is the same
architectural question §5.1 raises from the other end. That is the next
investigation, not the next micro-optimisation.

## 10. Campaign executed: realloc, and the hardening tax it was meant to repay

**Date:** 2026-08-21 · **Target:** §5.3's finding — the hardening cost realloc
**+5.66 Ir/op** · **Instrument:** `bench/opscan.sh`'s two-point estimator on the
`realloc` op, rebuilt from source before every arm.

**First, the framing correction.** Realloc's tax had already been repaid before
this campaign started: the opps campaign returned −14.33 and §9's work another
−2.64, leaving realloc at **376.72 against a pre-hardening 0.7.0 baseline of
388.03**. So this was new ground, not recovery. Sixteen primitives were
identified and **all sixteen were executed and measured**: seven won, five were
refuted (all five are recorded in the code so they are not retried), three read
flat, and one was measured and declined.

### Result

| op                                                                           | before |      after |               delta | mimalloc |     ra/mi |
| ---------------------------------------------------------------------------- | -----: | ---------: | ------------------: | -------: | --------: |
| **realloc**                                                                  | 376.72 | **281.77** | **−94.95 (−25.2%)** |   499.82 | **0.564** |
| small                                                                        |  79.37 |  **59.32** | **−20.05 (−25.3%)** |   111.41 |     0.532 |
| med                                                                          |  86.50 |  **64.69** |          **−21.81** |   123.02 |     0.526 |
| big                                                                          | 161.00 | **140.00** |          **−21.00** |   222.04 |     0.630 |
| calloc                                                                       | 130.88 | **109.62** |          **−21.26** |   160.89 |     0.681 |
| aligned                                                                      | 168.12 | **146.31** |          **−21.81** |   187.89 |     0.779 |
| mixed                                                                        | 132.38 | **124.22** |           **−8.16** |   157.74 |     0.788 |
| **batch_lifo**                                                               |  60.16 |  **59.53** |           **−0.63** |    59.70 | **0.997** |
| batch_fifo                                                                   |      — |      59.52 |                   — |    59.68 |     0.997 |
| small_touch                                                                  |      — |      65.32 |                   — |   117.41 |     0.556 |
| large                                                                        |      — |     140.00 |                   — |   222.02 |     0.631 |
| usable                                                                       |      — |      28.00 |                   — |    30.00 |     0.933 |

**−94.95 Ir/op on realloc is 16.8× the +5.66 the hardening cost it**, and puts
realloc **106.26 Ir/op below the pre-hardening 0.7.0 baseline**.

**Every op in the scan improved, and every op is now ahead of mimalloc.**
That includes `batch_lifo` — the one operation this repository has recorded as
a loss since the ThreadSanitizer fix, explained as a safe-Rust codegen floor
(`docs/opps.md` #6). The floor was real, but it was not the whole gap: at
**59.53 against mimalloc's 59.70** the op is now a win, without touching the
`used--` instruction selection that #6 correctly refuted.

Real programs, both deterministic verdicts: **perl 776,635,622 → 776,563,346**
(−72,276) and **sqlite 316,949,086 → 316,906,273** (−42,813).

### The sixteen, in the order they were executed

| #   | primitive                                                                       |                    Ir/op | verdict                      |
| --- | ------------------------------------------------------------------------------- | -----------------------: | ---------------------------- |
| 1   | `retire_or_abort`: keep-one-warm from the PAGE's own links                      |               **−33.00** | **win**                      |
| 2   | split `free_inline` to pass realloc's already-resolved segment                  |       **+1.00 per free** | **REFUTED**                  |
| 3   | `usable_size`: one flags load, not kind + `unalign` + subtract                  |               **−10.00** | **win**                      |
| 4   | fold the keep-warm test into `free`'s rarely-taken branch                       |               **−29.00** | **win**                      |
| 5   | realloc calls `free_inline`; LLVM CSEs the segment resolution                   |               **−16.00** | **win**                      |
| 6   | a `realloc_inline` export twin, as `free` has it                                |                **+6.31** | **REFUTED**                  |
| 7   | the in-place test as a single unsigned range check                              |                        0 | flat — LLVM emits it already |
| 8   | the two link tests as one OR                                                    |                        0 | flat — already merged        |
| 9   | `update_direct`: early-out when the range is already correct                    |                **−5.10** | **win**                      |
| 10  | gate the always-on `generic`/`extends`/`pages_fresh`                            |                    −0.17 | **declined — see below**     |
| 11  | `deferred_free`: peek the fn pointer before its argument                        |                **−0.17** | **win**                      |
| 12  | hand-rolled 32-byte-chunk copy instead of `memcpy`                              |               **+31.47** | **REFUTED**                  |
| 13  | outline `usable_size`'s interior-pointer tail as `#[cold]`                      |                **−1.68** | **win**                      |
| 14  | outline realloc's move path to shrink the in-place frame                        |               **+12.00** | **REFUTED**                  |
| 15  | `segment_of` by pointer arithmetic, not `with_addr`                             | **+1.00 per alloc/free** | **REFUTED**                  |
| 16  | drop the keep-warm test now dead in `retire_emptied`                            |                   0 here | flat (helps multi-page bins) |

### The one that carried the campaign, and why it was hiding

`retire_or_abort` fired on **every free** in this benchmark — 3 per op, 75.03
Ir/op, 20% of realloc — because a workload that cycles one live block through a
size class empties its page every single time. All it did was decide *"this
page is its queue's only member, keep it warm"* — and it spent a segment mask,
an `Acquire` load of `xheap` plus a container-of to reach the owning heap, a
`bin` load and a queue address to reach the two compares that decided nothing
needed doing.

A queued page is its queue's sole member exactly when **both of its own links
are null**. Two loads, two tests, no heap. That is #1 (−33.00). Then #4 moved
the same test up into `free`'s emptied branch, so the cold call, its prologue
and its return vanish too on that outcome (−29.00) — and because it sits inside
a branch that a free which does not empty its page never enters, `batch_lifo`
paid nothing for it.

Those two are also why `med`, `big`, `calloc` and `aligned` each fell ~21
Ir/op without being touched: they are all `op_pair` shapes that empty a page
per iteration.

### What the refutations teach

**Do not pay the hot path to help a caller (#2, #15).** Splitting `free_inline`
so realloc could hand it a pre-resolved segment saved realloc 12 Ir and cost
**+1 Ir on every free in the program** — and `free` is 45% of this corpus's
allocator instructions (§4). #15 was the same shape: a `segment_of` written as
pointer arithmetic rather than `with_addr`, on the theory that the integer
round-trip blocks CSE, cost +1 on every malloc *and* every free. The sharing
#2 wanted was real — #5 got all of it by simply letting realloc call
`free_inline` and having the inliner find the common subexpression, which costs
no other caller anything because only that one call site opts in.

**A trick that pays for one function does not pay for another (#6, #14).**
`free_inline` exists because inlining `free`'s small body into the export
removes a GOT thunk and needs no stack frame. Doing the same for `realloc` cost
+6.31: its body is far larger, so the export gains a prologue worth more than
the thunk. #14 was the mirror image — outlining realloc's move path to shrink
its frame cost +12.00, because this scan is *all* moves and the argument setup,
call and return are then paid every time.

**The obvious explanation for a cost is often not the cost (#12).** Replacing
`memcpy` with an inline chunk loop — on the theory that a PLT call dominates a
64-byte move — cost **+31.47 Ir/op**. glibc's AVX `memcpy` moves 64 bytes in a
couple of wide load/store pairs; the chunk loop pays an increment, a compare
and a branch per chunk. The call was never the expensive part.

**Three of sixteen were already done by the compiler (#7, #8, and the range
check).** The readable form of the in-place test already compiles to the
one-compare unsigned range check; the two null tests were already merged.

**#10 was declined on its measurement, not on principle.** The always-on
`generic` / `extends` / `pages_fresh` counters are this project's work-parity
instrument for every A/B. Gating them behind `debug_assertions` — as
`allocs`/`frees` and, in opps #2, `realloc_in_place`/`realloc_moved` already are
— is worth **0.17 Ir/op, 0.06%**. They are effectively free, so the diagnostic
stays. Measuring it is what turned "should we?" into "no, and here is why".

### Gates run

- `cargo clippy --workspace --all-targets --all-features` — clean
- `cargo test --workspace --all-features` — **32/32 suites pass**, including the
  new `debug_assert` in `retire_emptied` asserting the invariant #16 relies on
- `corpus/sweep-all.sh 8` — **19/19** benchmarks complete under rusty_alloc
- the full `opscan` table above — no op regressed
- perl and sqlite, the two deterministic real-program verdicts — both improved

## 11. Campaign executed: aligned

**Date:** 2026-08-21 · **Target:** §5.3's finding — the hardening cost `aligned`
**+6.00 Ir/op**, the largest per-op tax of the campaign · **Instrument:**
`bench/opscan.sh`'s two-point estimator on the `aligned` op
(`posix_memalign(&p, 64, 256)` + `free`), rebuilt from source before every arm.

**The framing correction again, because it keeps mattering.** Aligned's tax was
already repaid: §10's work left it at **146.31 against a pre-hardening 0.7.0
baseline of 163.25**. Thirteen primitives were identified and **all thirteen
were executed and measured**: ten won, one was refuted, and two read flat (one
of those also changed observable behaviour, which is why it went back).

### Result

| op                                                                            | before |     after |               delta | mimalloc |     ra/mi |
| ----------------------------------------------------------------------------- | -----: | --------: | ------------------: | -------: | --------: |
| **aligned**                                                                   | 146.31 | **99.50** | **−46.81 (−32.0%)** |   187.89 | **0.529** |

**−46.81 Ir/op is 7.8× the +6.00 the hardening cost it**, and leaves aligned
**63.75 Ir/op below the pre-hardening 0.7.0 baseline**.

**Every other op in the scan came back byte-identical** — `small`, `med`,
`big`, `large`, `calloc`, `batch_lifo`, `batch_fifo`, `realloc`, `usable`,
`mixed`, `small_touch` all at +0.00. This campaign is surgical where §9 and §10
were broad, and that is the right shape: the wins are in the aligned entry
chain, and nothing else routes through it.

### Where the time was, and what it cost to find out

The op resolves to *glue, a malloc, and an ordinary free*. The aligned fast
path peeks the bin's next free block and tests its actual address against the
alignment mask — for a 256-byte bin and a 64-byte request that hits **93.75%**
of the time, so almost nothing aligned-specific happens at all. Yet the entry
chain cost more than the allocation:

| layer                                                                                                |   before |                         after |
| ---------------------------------------------------------------------------------------------------- | -------: | ----------------------------: |
| `Heap::malloc_aligned_at`                                                                            | 50.12/op |        inlined into the entry |
| `free`                                                                                               | 32.02/op |          32.02/op (untouched) |
| `mi_posix_memalign`                                                                                  | 21.00/op |        folded into the export |
| `alloc::malloc_aligned`                                                                              | 20.00/op | ~21/op (now carries the peek) |

**Prologue and epilogue alone were 20.00 Ir/op — 40% of
`Heap::malloc_aligned_at`.** The peek uses almost no registers; it was sharing
a frame with huge placement and oversize-and-adjust, and paying their spills on
every call. That is #1, and it is the same lesson §10's #13 taught on
`usable_size`, applied to a much larger function.

### The thirteen, in the order they were executed

| #   | primitive                                                                                         |      Ir/op | verdict                  |
| --- | ------------------------------------------------------------------------------------------------- | ---------: | ------------------------ |
| 1   | split `Heap::malloc_aligned_at`: peek hot, everything else `#[cold]`                              | **−30.93** | **win (−21.1%)**         |
| 1b  | restore the `align <= SEGMENT_SIZE/2` bound to the hot guard                                      |          0 | correctness repair, free |
| 2   | the entry uses `heap_box_fast`, not `my_heap`'s heap-creating path                                |  **−6.13** | **win**                  |
| 3   | `mi_posix_memalign`: bound test first, so power-of-two drops its zero half                        |  **−2.00** | **win**                  |
| 4   | the same bare `align & (align - 1)` in the hot guard                                              |  **−1.00** | **win**                  |
| 5   | collapse the `posix_memalign` → `mi_posix_memalign` export hop                                    |  **−1.00** | **win**                  |
| 6   | `#[inline]` the glue so the shim's validation propagates                                          |  **+4.56** | **REFUTED**              |
| 7   | raw-read peek off the TLS box, mirroring `malloc`'s sentinel handling                             |  **−0.81** | **win**                  |
| 8   | `#[cold]` EINVAL/ENOMEM helpers instead of inline `return 22`                                     |  **−2.00** | **win**                  |
| 9   | one `align - 1` for the bound, the power-of-two test and the mask                                 |  **−1.00** | **win**                  |
| 10  | on a peek miss, enter the cold half directly, skipping its twin peek                              |  **−1.19** | **win**                  |
| 11  | `page_pop_known`: pop the block the peek already loaded                                           |          0 | flat — already CSE'd     |
| 12  | write `*memptr` before the failure test (straight-line success)                                   |          0 | flat — **and reverted**  |
| 13  | delete the third copy of the peek, left inside the cold half                                      |  **−0.75** | **win**                  |

### What this one teaches

**One redundant peek became three.** #1 split the function by copying the peek
into the new hot half; #7 then put a peek in the outer entry too, so the guard
existed in `alloc::malloc_aligned_at`, in `Heap::malloc_aligned_at`, and still
in `Heap::malloc_aligned_at_slow`. #10 and #13 are the cleanup — and both were
*wins*, because every copy after the first runs only when an identical copy has
already failed against the same heap, the same page and the same free list.
**A split is not finished when the fast path is fast; it is finished when the
slow path stops repeating it.**

**The same trick, twice refuted, in the same place.** #6 — `#[inline]` on the
aligned glue so the C shim's already-proven `align` facts would let LLVM drop
the peek's re-derivation — cost **+4.56**, exactly as §10's `realloc_inline`
twin cost +6.31. Both times the export gained a frame worth more than the work
it removed. That is now two independent measurements of the same rule:
**`free_inline` works because `free`'s body is small; it does not generalise to
a bigger body.**

**A flat result is not neutral if it changes behaviour (#12).** Writing
`*memptr` before the failure test makes the success path straight-line, and
POSIX does leave that value unspecified on failure — so it was conformant. It
measured **0.00**. A behaviour change that buys nothing is strictly worse than
no change, so it went back; only the measurement makes that call obvious rather
than arguable.

**`is_power_of_two` is not one instruction (#3, #4, #9).** It is
`count_ones() == 1`, which LLVM expands to a non-zero test *and* an
`x & (x - 1)` test. Establishing the lower bound first makes the non-zero half
redundant, and the alignment mask the peek needs anyway *is* `align - 1`. Three
separate wins came out of noticing that one expression was being computed three
ways in two crates.

### Gates run

- `cargo clippy --workspace --all-targets --all-features` — clean
- `cargo test --workspace --all-features` — **32/32 suites, 0 failed**
- `corpus/sweep-all.sh 8` — **19/19** benchmarks complete under rusty_alloc
- the full `opscan` table — **aligned −46.81, every other op +0.00**
- perl and sqlite, the deterministic real-program verdicts — both improved
  slightly (−566 Ir each), neither regressed

## 12. Campaign executed: alloc-test — §5.1 closed

**Date:** 2026-08-21 · **Target:** §5.1, the largest loss in the corpus ·
**Instruments:** a new `opscan` op (`liveset`, plan item W0) for iteration, and
`alloc-test 1` under callgrind for the verdict.

### Result

|                                                                        |             before |               after |        mimalloc |     ra/mi |
| ---------------------------------------------------------------------- | -----------------: | ------------------: | --------------: | --------: |
| **alloc-test program Ir**                                              |     19,183,776,321 |  **12,777,782,088** |  12,992,502,658 | **0.983** |
| **alloc-test allocator Ir**                                            |      8,618,257,221 |   **2,212,264,809** |   2,326,748,826 | **0.951** |
| allocator share of program                                             |             44.92% |          **17.31%** |          17.91% |           |
| generic-path entries                                                   | 35,035,889 (69.9%) | **583,986 (1.17%)** | 566,745 (1.13%) |           |
| `free_general` entries                                                 | 19,091,816 (38.1%) |  **23,912 (0.05%)** |  20,538 (0.04%) |           |
| `liveset` Ir/op                                                        |             200.08 |           **78.55** |           78.22 | **1.004** |

**−6,405,994,233 program instructions (−33.4%); the allocator's own share fell
74.3%.** alloc-test went from **1.477× mimalloc to 0.983×** on the whole
program, and from **3.704× to 0.951×** on allocator instructions — it is now
the faster of the two on the corpus's only realistic-load benchmark.

**Every other op in the scan is byte-identical** (+0.00 across all twelve), and
both deterministic real-program verdicts improved: perl −832 Ir,
sqlite −1,607 Ir.

### W0 first: an instrument that can see the disease

§5.1 recorded that every existing instrument is structurally blind here —
`op_pair` cycles one block, `op_mixed` cycles 64, and neither ever leaves a
page's fast list dry. The first thing built was therefore the `liveset` op:
**65,536 live objects**, random sizes on alloc-test's own distribution, each
step freeing a random slot and replacing it. It reproduced the disease
immediately — **200.08 Ir/op against mimalloc's 78.22, a 2.56× gap** — at about
a minute a measurement instead of five. Everything below was found on it and
confirmed on alloc-test.

### The eight, in the order they were executed

| #   | primitive                                                       |                                result | verdict                         |
| --- | --------------------------------------------------------------- | ------------------------------------: | ------------------------------- |
| 1   | un-park to the queue BACK, not the front                        |                     **−121.53 Ir/op** | **win (−60.7%)**                |
| 2   | `page_extend`: running pointer instead of `i * bsize`           |                                     0 | flat — LLVM strength-reduces it |
| 3   | `page_of`: derive the offset by mask, not by subtracting `seg`  |                                     0 | flat — already folded           |
| 4   | `free_inline` in the four C++ `operator delete` exports         |                   **−150,368,332 Ir** | **win (−6.2%)**                 |
| 5   | `new_impl` inlined into the `operator new` exports              |                    **−50,130,729 Ir** | **win (1.00/alloc)**            |
| 6a  | outline `malloc_generic`'s large/huge tail                      |                                +28 Ir | flat                            |
| 6b  | outline `malloc_generic`'s fresh-page tail                      | −0.58 M alloc-test, **+145,129 perl** | **REFUTED**                     |
| 7   | the same export-hop fix for sized delete and aligned new        |                    not exercised here | applied (sibling check)         |

Three wins. They were enough because the first one was the whole finding.

### #1 — the whole campaign in one line

§5.1 traced alloc-test's loss to a **park/unpark thrash**: a page that had
filled received one freed block, and `free_local_at` pushed it to the **front**
of its bin queue and pointed `direct[]` — the malloc fast path — straight at
it. The next allocation of that size took that single block, the page was full
again, the one after it missed, and `malloc_generic` walked the queue and
parked it once more. That cycle ran **19.1 million times** and drove the
generic path to **70% of all allocations**.

Upstream never does this, and says so in its own source: `_mi_page_unfull`
calls `mi_page_queue_enqueue_from_full` with **`enqueue_at_end = true`**, and
the alternative branch beside it is labelled *"enqueue at 2nd place"*. Either
way the queue **head** is left alone, so it keeps pointing at a page with
blocks to spare.

Changing `queue_push_front` to a new `queue_push_back` at that one call site
took the generic-path rate from **69.9% to 1.17%** — mimalloc's is 1.13% — and
`free_general` from **38.1% to 0.05%**, against mimalloc's 0.04%. `update_direct`
stays, because when the queue was empty this page really does become the head;
it early-outs when the head did not move.

### #4 and #5 — the C++ entry points were paying a hop the C ones were not

`free`'s export has used `alloc::free_inline` since M16, so the C `free` IS the
body rather than a call through the outlined one. **The four C++
`operator delete` exports still called `alloc::free`** — and a C++ benchmark
reaches the allocator through `_ZdaPv`, not through `free`. Inlining them was
**−150.4 M Ir, 3.0 per delete**. The same hop existed on the `operator new`
side, through the cross-crate `extern "C" mi_new` that nothing can inline
through; giving it a `#[inline] new_impl` twin with the OOM retry outlined was
**−50.1 M, exactly 1.00 per allocation**.

This is the third campaign in which the `free_inline` arrangement paid and the
second in which its *converse* was refuted — §10 #6 and §11 #6 both showed that
inlining a **large** body into an export costs a frame worth more than the
thunk. `operator delete` is small; `realloc` and the aligned entry were not.
The rule that survives all five measurements: **inline an export's body when
the body is small enough not to need a frame, and never on the strength of the
pattern alone.**

### #6b — the refutation that mattered most

Outlining `malloc_generic`'s fresh-page tail as `#[cold]` is the same split
that won −30.93 Ir/op on `aligned` (§11 #1). It gained alloc-test 0.58 M Ir —
and cost **perl +145,129**. perl carves fresh pages constantly; alloc-test
carves a few hundred against 100 million operations. The outlined arm is only
*cold* in one of the two workloads, and where it is warm the call and argument
setup are pure loss.

It was caught only because the gate measures perl and sqlite as deterministic
verdicts, not just the benchmark under attack. **A split is worth it where the
outlined arm is genuinely rare — and "rare" is a property of the workload, not
of the code.** Both halves went back; the whole of #6's alloc-test gain was
this half, and it was not worth the trade.

### Gates run

- `cargo clippy --workspace --all-targets --all-features` — clean (it caught an
  undocumented `unsafe` in #7 before the tests did)
- `cargo test --workspace --all-features` — **32/32 suites, 0 failed**
- `corpus/sweep-all.sh 8` — **19/19** benchmarks complete under rusty_alloc
- full `opscan` — **all twelve pre-existing ops +0.00**; `liveset` 200.08 → 78.55
- perl **−832 Ir**, sqlite **−1,607 Ir** — both deterministic, both improved

### What this opens

`larson` (63.05% allocator, top symbol `malloc_generic`) and `xmalloc-test`
(72.33%, top symbol `free_general`) carry the same two signatures §5.1 named,
and §2.1 could not rank either because their throughput does not resolve on
this box. They are the natural next targets, and they now need the pinned
machine that §7 W6 asks for more than they need another primitive.

## 13. Campaign executed: sh8bench — and what its instrument can and cannot say

**Date:** 2026-08-21 · **Target:** sh8bench, 66% allocator share, the corpus's
heaviest allocator workload · **Outcome:** one win, three refutations, one
decline — and three findings about the instrument that matter more than any of
them.

### Finding 1: sh8bench's instruction count is NOT deterministic

Three runs of the **same binary**, before touching anything:

| run                                                                                                           |     program Ir |   allocator Ir |
| ------------------------------------------------------------------------------------------------------------- | -------------: | -------------: |
| 1                                                                                                             | 31,318,898,689 | 20,732,556,479 |
| 2                                                                                                             | 31,249,639,841 | 20,663,476,559 |
| 3                                                                                                             | 31,355,942,456 | 20,769,970,619 |

**Spread 106.3 M — ±0.34% of the program, ±0.52% of the allocator.** It is four
threads doing cross-thread frees; the interleaving varies, so the *amount of
work* varies. This is not measurement jitter in a counter, it is a different
program each time.

Every other campaign in this document rested on instruction counts being exact
to the unit. On sh8bench they are not, and **nothing worth less than ~100 M Ir
can be adjudicated on it at all.** The first thing this campaign produced was
therefore the knowledge that its headline benchmark could not referee it — and
a −0.18% reading against §3's figure, which looked like a regression from the
earlier campaigns, is comfortably inside that band and means nothing.

### Finding 2: sh8bench is already won

|                                                                                                   |    rusty_alloc |       mimalloc |     ra/mi |
| ------------------------------------------------------------------------------------------------- | -------------: | -------------: | --------: |
| program Ir                                                                                        | 31,318,898,689 | 32,130,779,125 | **0.975** |
| allocator Ir                                                                                      | 20,732,556,479 | 21,532,820,039 | **0.963** |
| `malloc`                                                                                          |  14.97 Ir/call |          15.99 | **0.936** |
| `free`                                                                                            |  26.79 Ir/call |          24.84 |     1.078 |
| generic path                                                                                      |    530 Ir/call |    896 Ir/call | **0.592** |

We are ahead on the whole program, on the allocator, on `malloc`, and on the
slow path by a factor of nearly two. The one place we lose is `free`, by
**+1.95 Ir/call** — which is the safe-Rust codegen floor `docs/opps.md` #6
measured and closed: the memory-destination decrement and the `fs`-relative
compare that LLVM will not emit. Confirmed again here from a third workload.

### Finding 3: the per-call instrument

An aggregate that varies 0.5% can still contain per-call costs that do not.
`span_from_segments` read **31.00 Ir/call** and `free_general` **47.57 Ir/call**
across two runs with different interleavings — identical to the last decimal.
That is what made the one win below measurable: **divide the noise out.**

### The five, in the order they were executed

| #   | primitive                                                             |                                 result | verdict                  |
| --- | --------------------------------------------------------------------- | -------------------------------------: | ------------------------ |
| 1   | `page_extend`: build the free list forward, dropping the carried head |                        **+0.33 Ir/op** | **REFUTED**              |
| 2   | raise the extend batch bound 4 KiB → 8 KiB                            | −0.16…−8.39 Ir/op, **+2.2% LL misses** | **REFUTED**              |
| 3   | …→ 16 KiB                                                             |           more Ir, **+6.4% LL misses** | **REFUTED**              |
| 4   | `span_mark`: running pointer instead of two multiplies per slice      |            **−39.11 / −31.83 Ir/call** | **win**                  |
| 5   | mark only a free span's first and last slot                           |                              ~−20 M Ir | **DECLINED — see below** |

### #4 — the win, and why it needed a different instrument

`span_mark` writes a back-pointer into every interior slice of a span, and did
it as `(*seg).pages[idx + j].slice_offset = (j * slot_stride())` — a multiply by
`size_of::<Page>()` (88, not a power of two) *and* a multiply for the offset,
per slice. Carrying the slot pointer and the offset as running values makes
both adds:

|                                                                                             |         before |      after |               delta |
| ------------------------------------------------------------------------------------------- | -------------: | ---------: | ------------------: |
| `span_alloc`                                                                                | 187.68 Ir/call | **148.57** | **−39.11 (−20.8%)** |
| `span_free`                                                                                 | 177.72 Ir/call | **145.89** | **−31.83 (−17.9%)** |
| `span_from_segments` (control)                                                              |          31.00 |      31.00 |                0.00 |
| `free_general` (control)                                                                    |          47.57 |      47.57 |                0.00 |

≈ **−16.7 M Ir on sh8bench** — real, and 0.08%, which is precisely why the
aggregate could not see it and the two unchanged controls could.

### #2 and #3 — an instruction win that is a cache loss

Raising the extend bound means a draining size class re-enters the slow path
half as often, and it reads like a broad win: **−8.39 on realloc, −5.75 on
aligned, −2.85 on med, −0.97 on batch_lifo** at 8 KiB, better still at 16 KiB.
It was reverted anyway, because each extend WALKS its whole batch:

| bound                                                                             |               I refs |       D1 misses |          LL misses |
| --------------------------------------------------------------------------------- | -------------------: | --------------: | -----------------: |
| 4 KiB                                                                             |           11,567,403 |         144,514 |             20,750 |
| 8 KiB                                                                             | 11,542,248 (−25,155) | 146,106 (+1.1%) | 21,207 (**+2.2%**) |
| 16 KiB                                                                            |           11,544,360 | 149,413 (+3.4%) | 22,079 (**+6.4%**) |

8 KiB buys 25,155 instructions for **457 extra last-level misses** — at a few
hundred cycles each, ~114 k cycles spent to save perhaps 13 k. This repository
counts instructions *because its clock cannot resolve small effects*, not
because instructions are the goal, and its own README says so: *"fewer
instructions is not automatically less TIME (cache behaviour and syscalls do
not show up here)."* A change with a **measured** cost in the blind spot and an
**unverifiable** gain in the domain that matters is not a win. Upstream's
comment on the same 4 KiB bound reads only *"one OS page seems to work well"* —
it is one OS page for a reason.

### #5 — declined, and the reason is the point

Free spans get long once they coalesce (~16 interior slices per call here), and
`span_mark_free` was writing every one of those back-pointers. Interior offsets
are read by exactly one function, `page_of`, which only ever runs on a pointer
to a **live** block; coalescing follows back only from a neighbour's **last**
slot. So marking only the first and last slot of a free span is, on the
reasoning, sound — and worth another ~20 M Ir.

`debug_validate_segment` disagreed, immediately, by aborting the debug test
run: it asserts the **stronger** invariant that every span, free or live, has
every interior slice pointing back. That walk is the net which caught the M8
parallel-gate race `adopt_segment` still documents.

Landing #5 meant weakening a validated layout invariant of an allocator to buy
~20 M Ir on a benchmark whose own noise floor is ~100 M — a change we could not
measure where it matters, paid for in the property this code most needs to
keep. Declined, with the reasoning recorded at the site so it can be
re-specified deliberately if it is ever wanted, rather than weakened as a side
effect.

### Gates run

- `cargo clippy --workspace --all-targets --all-features` — clean
- `cargo test --workspace --all-features` — **32/32 suites, 0 failed**
- `cargo test -p rusty_alloc --features debug_checks` — 0 failed (**this is the
  gate that refused #5**)
- `corpus/sweep-all.sh 8` — **19/19**
- full `opscan` — **all thirteen ops +0.00** (#4 touches span carving, which the
  two-point estimator cancels by design)
- perl **−7,934 Ir**, sqlite **−6,263 Ir** — both improved

### What sh8bench needs next

Not another primitive. It needs the pinned, quiet machine §7 W6 asks for, so
its 0.5% noise band closes far enough to referee the work — and, before that, a
`bench/` harness entry that records this benchmark's noise floor beside its
result, so the next person does not read a 0.2% move as a finding. `larson` and
`xmalloc-test` are threaded too and almost certainly share the problem.

## 14. Campaign executed: rptest

**Date:** 2026-08-21 · **Target:** rptest — 32.62% allocator share, and the only
corpus benchmark whose top symbols are the ALIGNED and calloc paths ·
**Instrument:** `rptest 1`, where the allocator's instruction count is exact.

### Finding first: the instrument, again

sh8bench taught this campaign to check determinism before reading a delta.
rptest is worse — three runs of the same binary at four threads:

| run                                                                                                                |  program Ir | allocator Ir |
| ------------------------------------------------------------------------------------------------------------------ | ----------: | -----------: |
| 1                                                                                                                  | 259,740,658 |   65,478,068 |
| 2                                                                                                                  | 265,921,787 |   68,389,305 |
| 3                                                                                                                  | 254,515,166 |   65,549,676 |

**±4.4%** — eight times sh8bench's band. But dropping to **one thread** makes
the allocator's count *exact*: **12,291,622 three times, to the unit** (the
small residual movement in the program total is thread startup, outside our
code). So the campaign ran on `rptest 1` and used `rptest 4` only for the
verdict, quoted against its measured floor.

### Result

|                                                                                         |     before |          after |                  change |
| --------------------------------------------------------------------------------------- | ---------: | -------------: | ----------------------: |
| **`rptest 1` allocator Ir** (exact)                                                     | 12,291,622 | **10,642,166** | **−1,649,456 (−13.4%)** |
| vs mimalloc (13,968,309)                                                                |     0.880× |     **0.762×** |                         |
| **`rptest 4` allocator Ir** (median of 3)                                               | 65,549,676 | **59,093,353** |              **−9.85%** |
| vs mimalloc (87,350,348)                                                                |     0.744× |     **0.677×** |                         |

The four-thread ranges do not overlap — baseline min 65.48 M against final max
59.70 M — so the improvement clears that benchmark's ±4.4% floor outright.

**No op in the scan regressed**; `calloc` gained −2.06 Ir/op as a side effect,
and perl (−7,329) and sqlite (−3,322) both improved.

### The root cause: a hole between the size classes

`direct[]` is indexed by word size and stops at `SMALL_WSIZE_MAX`, so the fast
peek covers allocations up to 1 KiB. rptest allocates **8..4000 bytes**. Every
request above 1 KiB — **29% of them** — skipped the peek entirely and fell into
`malloc_generic`: the heartbeat, the queue walk, full-page parking,
`update_direct`, all of it, even when the bin's front page had a block ready.
Upstream has the same hole.

The bin's queue front is one indirection further than a `direct[]` entry and
answers the same question.

### The seven, in the order they were executed

| #   | primitive                                                                    |                        Ir (rptest 1) | verdict             |
| --- | ---------------------------------------------------------------------------- | -----------------------------------: | ------------------- |
| 1   | `Heap::malloc`: peek the bin queue for MEDIUM sizes                          |                         **−823,544** | **win (−6.7%)**     |
| 2   | the same peek on the plain-malloc path (`alloc::malloc_slow`)                |           big/large **+25.00 Ir/op** | **REFUTED**         |
| 3   | derive the bin ONCE across `good_size` and the malloc it picks               |                         **−414,165** | **win (−3.6%)**     |
| 4   | `zalloc`: `heap_box_fast`, not `my_heap`'s heap-creating path                |                          **−99,505** | **win**             |
| 5   | medium aligned peek — three placements measured                              |                         **−312,242** | **win (see below)** |
| 6   | thread the derived bin into `malloc_generic`                                 | −36,918, small/batch **+0.02/+0.06** | **REFUTED**         |
| 7   | the same `my_heap` fix for `zalloc_aligned_at`                               |                   not exercised here | applied (sibling)   |

### #2 — the same idea, and it is a large regression

Putting the medium peek on the **plain-malloc** path costs `big` and `large`
**+25.00 Ir/op each**, against `mixed` −30.22. The reason is which list the
block is on: a tight alloc/free loop frees into `local_free`, so the queue
front's `free` list is **always dry** when the next allocation arrives. The
peek can never hit, and every one of them pays the bin computation, the queue
read and a failed `page_pop` before falling through anyway.

**A fast path is worth its cost only where the thing it looks for is actually
there.** It pays in `Heap::malloc` (#1) and in the aligned entry (#5) because
those workloads leave populated free lists behind; it does not pay one call
earlier.

### #5 — the same win, three placements, only one of them free

The medium aligned peek is worth ~300 k either way. Where it goes decides what
it costs the `aligned` scan, which gates the small aligned path:

| placement                                                                                                    |       rptest 1 |       `aligned` |
| ------------------------------------------------------------------------------------------------------------ | -------------: | --------------: |
| folded into the small guard as a select                                                                      |     10,850,437 | **+6.19 Ir/op** |
| nested arms under a shared guard                                                                             |     10,637,597 |       **+2.50** |
| duplicated `else if` chain                                                                                   |     10,816,717 |       **+2.50** |
| **in the COLD fallback (`malloc_aligned_slow`)**                                                             | **10,642,166** |       **+0.00** |

The first forced a null test that `direct[]` never needs (its empty slots hold
the immortal sentinel); the second and third merely reordered the guard chain
and still cost 2.50. Moving the peek into the cold function keeps the hot entry
**byte-for-byte unchanged** and still catches the case one call earlier than
before — within 5 k Ir of the best variant, for nothing.

### #6 — a win on the target, a regression on the gates

`malloc_in_bin` derives `bins::bin(size)` to pick the queue it peeks, and on a
miss `malloc_generic` derived the same bin again. Passing it through is worth
−36,918 on rptest and costs `small` +0.02 and `batch_lifo` +0.06 — the "bin not
yet known" marker is a compare every generic call pays, including the small
ones that never had a bin to pass. The const-generic form that would fold the
check away duplicates the whole function. Declined at 0.35% of one benchmark.

### Gates run

- `cargo clippy --workspace --all-targets --all-features` — clean
- `cargo test --workspace --all-features` — **32/32 suites, 0 failed**
- `cargo test -p rusty_alloc --features debug_checks` — 0 failed
- `corpus/sweep-all.sh 8` — **19/19**
- full `opscan` — no op regressed; `calloc` **−2.06**
- perl **−7,329 Ir**, sqlite **−3,322 Ir**

## 15. Campaign executed: xmalloc-test

**Date:** 2026-08-21 · **Target:** xmalloc-test — 72.33% allocator share and the
only corpus benchmark whose top symbol is `free_general`, the cross-thread free
path · **Outcome:** one win, two refutations, two flats, and the clearest
instrument failure in this document.

### Finding 1: xmalloc-test measures the machine, not the allocator

Its main loop runs `while (elapsed_time(&begin) > run_time)` — it is bounded by
**wall time**, so the amount of work it does depends on how fast the box is at
that moment. Under callgrind's ~50× slowdown that is whatever the machine felt
like. Three runs of one binary:

| run                                                                                                                              | allocator Ir |
| -------------------------------------------------------------------------------------------------------------------------------- | -----------: |
| 1                                                                                                                                |  109,098,035 |
| 2                                                                                                                                |  109,216,130 |
| 3                                                                                                                                |  124,928,406 |

**±13.6%.** And the decisive evidence came at the end of the campaign, from the
arm containing **no code of ours at all**:

|                                                                                                                    | mimalloc arm, allocator Ir |
| ------------------------------------------------------------------------------------------------------------------ | -------------------------: |
| session start                                                                                                      |                146,496,195 |
| session end                                                                                                        |                176,792,246 |

**The unchanged mimalloc binary moved 21% between two runs an hour apart.** No
verdict of any kind can be taken from this benchmark on this box — not for us,
not against upstream. The three ra runs at the end spread 19.6%.

### Finding 2: the deterministic stand-in

xmalloc-test's shape is "one thread allocates, another frees" — `remote_free`,
the delayed list, and full pages a non-owner cannot un-park. That is worth
optimising; the benchmark just cannot referee it. So the campaign added an
`opscan` op, **`xthread`**, that does the same allocator work without the
interleaving: the main thread allocates a batch, a spawned thread frees the
whole batch and is joined before the next begins. Every free is remote, and the
two threads never run at once.

**It reads 123.55 Ir/op three times, to the hundredth.**

### Result

|                                                                                | before |              after |                      vs mimalloc |
| ------------------------------------------------------------------------------ | -----: | -----------------: | -------------------------------: |
| `xthread` Ir/op (exact)                                                        | 123.55 | **116.54 (−5.7%)** | 130.87 → **0.891×** (was 0.944×) |

No op in the scan regressed; perl (−9,672) and sqlite (−4,151) both improved.

### What the proxy showed

**74.6% of frees take `free_general`** — 45,056 of 60,382. A remote thread
cannot un-park a page it does not own, so every remote free to a full page
lands on the general path and stays there until the owner's heartbeat runs.
That is the design; the cost is what it does when it gets there.

### The four, in the order they were executed

| #   | primitive                                                                                       |     Ir/op | verdict                     |
| --- | ----------------------------------------------------------------------------------------------- | --------: | --------------------------- |
| 1   | pass the resolved `seg` and `pg` into `free_general`                                            | **−7.01** | **win (−5.7%)**             |
| 2   | pass the `flags` byte into `free_local_at` as well                                              | **+6.05** | **REFUTED**                 |
| 3   | drop `#[cold]` from `free_general`                                                              |         0 | flat — Ir cannot see layout |
| 4   | pass the `flags` byte into `free_general` too                                                   |         0 | flat                        |

### #1 — a documented decision that a change in frequency reversed

`free_general` took only `p` and re-derived the segment, the page and the flags
that `free` had **just computed**. Its doc explains why, and the reasoning was
sound: at **1.6% of frees** (`batch_lifo`, where it was measured) ten
instructions of re-derivation beat keeping values live across the fast path,
and threading all five through measured worse.

The frequency is what changed. At **74.6%** the same ten instructions are the
dominant cost of the dominant path. Passing the two *derivations* — a segment
mask and the `page_of` follow-back — is −7.01 Ir/op, and `small` and
`batch_lifo` are **byte-identical**, because those two values are already live
in registers at the call.

**A cost/benefit that was measured correctly can still expire.** What made this
findable was measuring a workload the original decision never saw.

### #2 and #4 — where the same idea stops paying

#4 passes one more value into the same function and reads **exactly flat**: an
atomic load is one instruction, and so is the argument that replaces it. #2
passes one into `free_local_at` and costs **+6.05** — a fifth argument pushes
that larger function over a register-allocation cliff, reproducing the M9
result at a different arity.

So the rule the three measurements together give is sharper than "pass things
in": **pass a DERIVATION the caller already has (a mask, a follow-back); do not
pass a LOAD, and do not pass anything into a function that is already near its
register budget.**

### Identified, not landed: O(1) cross-thread collect

`page_collect` walks the whole stolen chain — `block_next` per element — to
find its tail and count its length for `used`. On this workload that walk is
most of `malloc_generic`'s 1,355 Ir per call. When `free` is empty (the case
that brought us here) the *tail* is not needed at all; only the count is. A
per-page `AtomicU32` of pending remote frees, maintained by `remote_free`,
would make the whole collect O(1) — at the cost of a `lock add` on every remote
free, four bytes per page, and a new invariant (the count must match the chain)
sitting next to the loom-modelled protocol. Estimated ~6% of this workload.
Recorded rather than attempted: it is a protocol change, and this campaign had
no admissible way to verify one.

### Gates run

- `cargo clippy --workspace --all-targets --all-features` — clean
- `cargo test --workspace --all-features` — **32/32 suites, 0 failed**
- `cargo test -p rusty_alloc --features debug_checks` — 0 failed
- `corpus/sweep-all.sh 8` — **19/19**
- full `opscan` — no op regressed
- perl **−9,672 Ir**, sqlite **−4,151 Ir**

## 16. Campaign executed: sh6bench — no wins, and the reason is worth more

**Date:** 2026-08-21 · **Target:** sh6bench, 64.6% allocator share ·
**Outcome:** **zero primitives landed.** What the campaign produced instead is
an exact instrument, a precise account of the only place in the corpus where we
are behind mimalloc, and a much harder refutation of the one thing that would
close it.

### The instrument: sh6bench at one thread is EXACT

Unlike sh8bench (±0.52%), xmalloc-test (±13.6%) and rptest at four threads
(±4.4%), sh6bench run with `1` gives an allocator count exact to the unit:

| run                                                                                                        |     program Ir |      allocator Ir |
| ---------------------------------------------------------------------------------------------------------- | -------------: | ----------------: |
| 1                                                                                                          | 12,400,115,344 | **8,005,465,906** |
| 2                                                                                                          | 12,400,115,325 | **8,005,465,906** |
| 3                                                                                                          | 12,400,115,337 | **8,005,465,906** |

(The ~19 Ir of movement in the program total is thread startup, outside our
code.) It is the only threaded benchmark in the corpus that can referee a
single instruction, and that is what made the rest of this section possible.

### The finding: sh6bench is where we are behind, and it is one thing

|                                                                                     |       rusty_alloc |      mimalloc |                 delta |
| ----------------------------------------------------------------------------------- | ----------------: | ------------: | --------------------: |
| allocator Ir                                                                        |     8,005,465,906 | 7,960,789,811 | **+44.7 M (1.0056×)** |
| `malloc`                                                                            |     14.99 Ir/call |         16.00 |    **−1.01 → −195 M** |
| `free`                                                                              | **26.99 Ir/call** |     **25.00** |    **+1.99 → +366 M** |
| generic machinery                                                                   |             124 M |        ~210 M |                 −86 M |

193,000,001 allocations and 183,808,001 frees, identical in both arms.
**98% of this benchmark is `malloc` and `free`.** We are ahead on malloc, ahead
on the slow path, and behind on free by exactly the `docs/opps.md` #6 codegen
floor — the memory-destination decrement and the `fs`-relative compare. There
is nothing else in the profile: everything below `malloc_generic` (1.6%) is
under 0.05%.

### Re-refuting the floor, harder than before

`opps.md` #6 measured the gap and tried **splitting the list-push from the
`used` decrement**, which produced byte-identical asm. But its own explanation
names a different culprit: LLVM will not emit `sub [mem], 1` when the
decremented value must *also* live in a register — and #6 never removed that
register dependency, because `page_push_local` still RETURNED the count.

So that was tried here: the return value deleted, the decrement written
straight to memory, the caller made to re-read the same field — the exact shape
Clang folds into `subw $1, used; je` for mimalloc. On an instrument exact to
the unit:

|                                                                                                                             |      allocator Ir |
| --------------------------------------------------------------------------------------------------------------------------- | ----------------: |
| before                                                                                                                      |     8,005,465,906 |
| after                                                                                                                       | **8,005,465,906** |

**Not one instruction different.** The fold is not reachable from safe Rust by
any arrangement of this code, and the `#[must_use]` return that was removed to
test it is worth keeping anyway — it makes ignoring the double-free signal a
compile error.

The other half of the floor was analysed rather than measured, because the
analysis settles it: mimalloc's `cmp %rcx, %fs:0` is one instruction because
`fs:0` is a memory operand. Rust can read `fs:0` only through inline `asm!`,
which forces a register (`mov` + `cmp`, two), and cannot export flags to a
Rust-level branch. Storing the id in this crate's own initial-exec TLS block
instead is strictly *worse* — that read is itself two instructions (GOTTPOFF
then `fs:[reg]`), so three with the compare. Only putting the compare AND the
branch in asm would close it, on the hottest function in the allocator.

### What this means for the number

The floor is now measured on four workloads — sh6bench +366 M, sh8bench
+852 M, alloc-test +200 M, rptest — and refuted twice, the second time with the
mechanism its own analysis pointed at. It is the single largest remaining item
in the allocator and it is **not a Rust-level problem to solve**. Closing it is
an inline-asm decision about `free`, which is a policy call for the owner, not
a primitive for a campaign. Recorded as such.

### Gates

Nothing landed, so nothing changed: clippy clean, **32/32 suites**, the full
`opscan` identical, perl and sqlite unmoved from §14's figures. The tree
carries exactly the six previous campaigns.

## 17. Campaign executed: cfrac — the frame was a fifth of the slow path

**Date:** 2026-08-21 · **Target:** cfrac, the corpus's purest single-threaded
allocator workload (91,530,284 allocations, 9.3% allocator share) ·
**Instrument:** cfrac is exact to the unit, in both arms.

### Result

|                                                                          |    campaign start |              after |      mimalloc |       ra/mi |
| ------------------------------------------------------------------------ | ----------------: | -----------------: | ------------: | ----------: |
| **cfrac allocator Ir**                                                   |     4,060,606,216 |  **3,994,824,217** | 4,038,242,733 | **0.98925** |
| cfrac program Ir                                                         |    43,817,729,406 | **43,751,947,531** |             — |             |
| `malloc_generic`                                                         |      82.9 Ir/call |    **~59 Ir/call** |         107.3 |             |
| — its frame (push+pop)                                                   | **17.01 Ir/call** |           **8.01** |             — |             |
| — calls reaching the queue walk                                          |       (not split) |          **0.53%** |             — |             |

**−65,781,999 instructions (−1.62%).** cfrac entered this campaign at
**1.0055× mimalloc** and leaves it at **0.98925×** — it is now the faster of
the two, by 43.4 M instructions.

To anchor that against something built in the same session rather than a
recorded figure, v1.0.0 was checked out into a git worktree and measured fresh:

|                                                                                             |         v1.0.0 |                now |      change |
| ------------------------------------------------------------------------------------------- | -------------: | -----------------: | ----------: |
| cfrac allocator Ir                                                                          |  5,635,676,135 |  **3,994,824,217** | **−29.11%** |
| cfrac program Ir                                                                            | 45,392,802,847 | **43,751,947,531** |      −3.61% |
| cfrac vs mimalloc                                                                           |        1.3956× |       **0.98925×** |             |

That 1.3956× reproduces §3's recorded 1.396× exactly, which is the check that
the anchor itself is sound.

### The finding: 20% of the slow path was prologue and epilogue

Line-mapping `malloc_generic` put **17.00 Ir/call in its frame** — eight
instructions of `push` and nine of `pop`, on every one of 2.56 M generic
allocations, 43.5 M in total and twice cfrac's entire remaining deficit. A
function only needs that frame if some value must survive a call, and the
mechanism turned out to be very specific:

> **A frame is created by a call with values LIVE ACROSS IT — not by a call.**
> Everything on this fast path either returns or tail-calls, so nothing of ours
> outlives it. The single exception was `page_extend`, which had `self`, `bin`
> and `p` live across it and on its own held two callee-saved registers hostage
> for the whole function. It fires on 0.1% of generic allocations.

Removing the live-across calls one at a time took the frame from 17.01 to
**8.01**, and the function from 82.9 to about 59 Ir/call against mimalloc's
107.3.

### The ten, in the order they were executed

| #   | primitive                                                              |                     Ir (cfrac) | verdict                         |
| --- | ---------------------------------------------------------------------- | -----------------------------: | ------------------------------- |
| S1  | outline the page-park branch (`park_full`)                             | −2,415,021, later **+117,465** | **EXPIRED — folded back**       |
| S2  | outline the guarded-sampling arm (`try_guarded`)                       |                 **−2,556,965** | **win** (1.00/call)             |
| S3  | outline the delayed-list drain                                         |                              0 | flat — retried as S6            |
| S4  | split the queue walk out into `malloc_generic_walk`                    |                 **−7,053,354** | **win** (−25,249,105 re-priced) |
| S5  | outline the page-grow arm (`grow_front`)                               |                **−20,464,651** | **win — accepted trade**        |
| S6  | S3 again, once S5 had removed the rival register holder                |                **−10,227,860** | **win**                         |
| S7  | outline the deferred-free hook's indirect call                         |                 **−2,570,824** | **win** (1.00/call)             |
| S8  | check `direct[]` with the request's own word index                     |                **−15,258,610** | **win** (6.00/call)             |
| S9  | straight-line peek before `page_collect`'s exchange loop               |                **−10,230,234** | **win** (4.00/call)             |
| S10 | immortal empty delayed-list sentinel                                   |                 **−5,113,930** | **win** (2.00/call)             |

**Eight wins, one flat that became a win, and one win that expired.** Every
`opscan` op improved, and no gated verdict regressed except as recorded under
S5.

### S4 — the split that made the rest possible

`malloc_generic` was one function: the heartbeat, the bin derivation, a queue
walk that parks every page that cannot serve, a fresh-page carve and an OOM
arm. Splitting it so the common outcome — *the queue's front page has a block,
or grows one* — keeps its own small frame, with everything else moved to an
`#[inline(never)]` `malloc_generic_walk`, is worth **−25.2 M** in the final
configuration.

The call counts say why:

|                                                                                                     |      calls | share of generic allocations |
| --------------------------------------------------------------------------------------------------- | ---------: | ---------------------------: |
| `malloc_generic`                                                                                    |  2,556,965 |                         100% |
| `malloc_generic_walk`                                                                               | **13,670** |                    **0.53%** |
| `page_extend`                                                                                       |      2,458 |                        0.10% |

The walk re-collects the first queue page rather than having it threaded in.
The collect is idempotent, and re-doing it is what keeps the two frames
independent of each other's register needs — which is the entire point of the
split.

### S1 — a WIN that expired, which is the finding of this campaign

S1 outlined the page-park branch as a `#[cold]` helper and measured
**−2,415,021** on the first day of the campaign, when `malloc_generic` was
still one large function. After S4 the same code sat inside
`malloc_generic_walk` — already out of line, already entered on 0.53% of calls
— so outlining it merely nested a second call inside the rare path. Re-measured
at the end:

|                                                                                            |       with S1 |        without S1 | S1's real cost |
| ------------------------------------------------------------------------------------------ | ------------: | ----------------: | -------------: |
| cfrac allocator                                                                            | 4,010,285,846 | **4,010,168,381** |   **+117,465** |
| perl                                                                                       |   776,690,628 |   **776,667,107** |    **+23,521** |
| sh6bench allocator                                                                         | 8,008,651,177 | **8,007,097,315** | **+1,553,862** |

It was folded back inline. This document already records refutations that
expired — §15 #1, where a correct cost/benefit was reversed by a change in
frequency. S1 is the mirror image: **a win can expire too.** The frequency that
justifies outlining something is a property of the ENCLOSING FRAME, not of the
code being outlined, so every outlining decision has to be re-measured when the
function around it is restructured — not only when the workload changes.

### S6 — the same lesson running the other way

S3 outlined the delayed-list drain and read **exactly flat**. The obvious
reading is "no frame pressure here". The real reason was that `page_extend` was
*also* live-across in the same frame and held the callee-saved registers on its
own, so removing one holder changed nothing. Once S5 removed that one, the
identical change was worth **−10,227,860**.

**Two live-across calls in one frame hide each other.** A flat result from
removing one is not evidence that it was free — it is evidence that something
else is still paying, and the measurement has to be repeated once that
something else is gone.

### S5 — a measured trade, taken deliberately

Outlining the page-grow arm is the largest single win here and the only change
in the campaign that costs anything:

|                                                                                           |        S5 off |             S5 on |          change |
| ----------------------------------------------------------------------------------------- | ------------: | ----------------: | --------------: |
| cfrac allocator                                                                           | 4,030,751,631 | **4,010,286,980** | **−20,464,651** |
| `opscan` big / large                                                                      |   −6.00 Ir/op |  **−14.00 Ir/op** |           −8.00 |
| `opscan` mixed                                                                            |   −4.03 Ir/op |   **−9.81 Ir/op** |           −5.78 |
| perl                                                                                      |   776,511,765 |       776,690,628 |    **+178,863** |

perl pays one call per extend and it carves constantly, so the arm this marks
`#[cold]` is genuinely warm in that one program — the same shape as the
fresh-page split refuted on alloc-test at **+145,129 perl**, a near-identical
figure. On precedent alone it would have gone back. It was kept because the
ratio is **114:1 in instructions** and the gain is corpus-wide, where the loss
is 0.023% of a single program. The numbers above sit at the call site so the
decision can be reversed deliberately rather than rediscovered.

### S8 — the cheapest kind of win: a fact about the data structure

`update_direct` re-derives the bin's block size, its top word index and the
front page before discovering that nothing moved — ten instructions to confirm
a no-op, on the path 99.5% of generic allocations take.

But `direct[]` has exactly **one writer**, and it writes a whole bin-range at a
time. So every slot of a bin's range always holds the same value, and **any one
of them answers for all of them** — including the request's own word size,
which `bins::bin` has already computed and which therefore costs nothing to
reuse. Ten instructions become four: **−15,258,610**, and it is a win on
sh6bench (**−2,319,579**) as well.

### Where the remaining deficit is, and where it is not

| cfrac, per call                                                                             |  rusty_alloc |  mimalloc |  effect on the program |
| ------------------------------------------------------------------------------------------- | -----------: | --------: | ---------------------: |
| `malloc`                                                                                    |     14.92 Ir |     16.00 |     **−99 M** (we win) |
| `malloc_generic`                                                                            |       ~59 Ir |     107.3 |    **−124 M** (we win) |
| `free`                                                                                      | **27.00 Ir** | **25.00** | **+183 M** (the floor) |

`malloc` was checked instruction by instruction against `_mi_page_malloc` and
is at parity: the block pop is six instructions in both, and the TLS heap read
is the same GOTTPOFF + `fs:[reg]` pair mimalloc emits under `LD_PRELOAD` — the
one-instruction form is only available to a main-executable local-exec TLS
model, which a preloaded allocator does not get. **There is nothing left in
`malloc`.**

`free`'s +2.00 Ir/call is the `docs/opps.md` #6 codegen floor, now measured on
a fifth workload and already refuted twice. It was not re-litigated.

### Instrument findings

**perl is exactly deterministic — but its ENVIRONMENT is an input.** Three runs
of one binary gave 776,690,628 to the unit. A harness edit that added a `cd`
moved it 2,561, because perl allocates its environment block. Deltas below a
few thousand mean nothing unless the invocation is byte-identical.

**mimalloc's sh6bench arm is NOT exact.** §16 established that `sh6bench 1`
reads the same to the unit and used it as an exact instrument — but it only
ever measured the ra side. Two runs of the *unchanged mimalloc* binary here:
7,961,242,320 and 7,960,607,621, **a spread of 634,699.** The ra arm is still
exact; the comparison against upstream on that benchmark is not.

Against v1.0.0, sh6bench is **8,043,394,341 → 8,007,097,315, −36,297,026.** It
reads **+1,631,409** against §16's recorded figure, and that residual is **not
attributable to anything in this campaign**: every primitive here was toggled
individually and each one *improves* sh6bench (S8 −2,319,579, S4 −1,857,601,
S5 −714,646, S2+S6+S7 −4,168,064 together). The one that hurt it was S1, which
was folded back. Recorded unexplained rather than blamed on the nearest change.

### Gates run

- `cargo clippy --workspace --all-targets --all-features` — clean
- `cargo test --workspace --all-features` — all suites, 0 failed
- `corpus/sweep-all.sh 8` — **19/19**, sweep passed
- v1.0.0 rebuilt in a git worktree as an independent anchor

Every `opscan` op improved and none regressed:

| op                                                                                                | before |      after |      delta | mimalloc |
| ------------------------------------------------------------------------------------------------- | -----: | ---------: | ---------: | -------: |
| small                                                                                             |  59.32 |  **59.22** |      −0.10 |   111.41 |
| small_touch                                                                                       |  65.32 |  **65.22** |      −0.10 |   117.41 |
| med                                                                                               |  64.69 |  **63.12** |      −1.57 |   123.02 |
| big                                                                                               | 140.00 | **122.00** | **−18.00** |   222.05 |
| large                                                                                             | 140.00 | **122.00** | **−18.00** |   222.09 |
| calloc                                                                                            | 109.62 | **106.00** |      −3.62 |   160.89 |
| batch_lifo                                                                                        |  59.53 |  **59.12** |      −0.41 |    59.70 |
| batch_fifo                                                                                        |  59.52 |  **59.11** |      −0.41 |    59.68 |
| realloc                                                                                           | 281.77 | **277.45** |      −4.32 |   499.82 |
| aligned                                                                                           |  99.50 |  **97.94** |      −1.56 |   187.89 |
| usable                                                                                            |  28.00 |      28.00 |      +0.00 |    30.00 |
| mixed                                                                                             | 124.22 | **111.40** | **−12.82** |   157.85 |
| liveset                                                                                           |  78.55 |  **76.44** |      −2.11 |    78.22 |

Real programs: **sqlite −47,006**; perl **+35,540** (+0.0046%) — S5's recorded
trade, most of which S9 and S10 gave back.

## 18. Campaign executed: larson-sized — a benchmark that cannot referee, and the driver that can

**Date:** 2026-08-21 · **Target:** larson-sized — 63.81% allocator share, and the
only corpus benchmark that exercises C++ **sized** deallocation ·
**Outcome:** four wins, one flat, two refutations, and a new deterministic
driver in `bench/`.

### Finding 1: larson-sized measures the allocator's own speed back at itself

Its workers run a fixed block quota and then **respawn themselves** —
`exercise_heap` ends with `if (!stopflag) _beginthread(exercise_heap, 0, pdea)`
— so the loop only ends when a timer fires. The `runloops` phase before it
breaks on `duration >= sleep_cnt`. Both halves are wall-clock bounded, which
means a FASTER allocator is handed MORE work. In one second under callgrind:

|                                                                                                           | operations completed | allocator Ir |
| --------------------------------------------------------------------------------------------------------- | -------------------: | -----------: |
| rusty_alloc                                                                                               |        **2,525,001** |  130,100,519 |
| mimalloc                                                                                                  |            1,745,458 |  107,200,100 |

**We did 45% more work, so our total is 21% higher.** Read as an aggregate this
benchmark reports our win as a loss. Per operation the same run says 51.52 Ir
against mimalloc's 61.42 — **0.839×** — which is the opposite verdict from the
same data.

Unlike `xmalloc-test` (§15, ±13.6%), larson-sized IS reproducible for a fixed
binary: 130,092,471 / 130,100,529 / 130,100,519, a spread of 8,058 on 130 M
(**±0.006%**). So it can measure one binary precisely and still cannot compare
two. That is a distinct instrument failure from the one §15 recorded, and it is
the more dangerous of the pair, because the numbers look trustworthy.

### Finding 2: the deterministic driver — `bench/sizedchurn.cpp`

`liveset`, `shbench` and `xthread` are C, and none of them reaches
`operator delete[](void*, size_t)` — the export that IS larson-sized. So the
campaign added a C++ driver instead: 5,000 live blocks, a random victim per
step released through sized delete and replaced at a random size from larson's
own 8..1000 range, one byte written to each new block, and a **fixed iteration
count** taken from `argv[1]` so the repo's two-point estimator applies.

It reads **exact to the instruction**: 11,479,732 at n and 22,100,637 at 2n,
twice over. `bench/sizedchurn.sh` wraps it.

### Result

|                                                                                     |        before |      after |         mimalloc |     ra/mi |
| ----------------------------------------------------------------------------------- | ------------: | ---------: | ---------------: | --------: |
| **`sizedchurn` Ir/op** (exact)                                                      |        53.637 | **52.920** | 62.3 (61.5–63.5) | **0.849** |
| `operator new[]` self                                                               | 16.17 Ir/call |  **14.84** |            20.49 |     0.724 |
| `operator delete[]` self                                                            |         26.70 |      26.70 |            26.89 |     0.993 |
| generic machinery                                                                   |          11.4 |   **10.6** |            16.85 |     0.629 |

**−0.717 Ir/op (−1.34%)**, and cfrac improved a further **88,262** to
3,994,735,955 as a side effect. No `opscan` op moved.

Split by phase, on the exact driver, we are ahead of upstream everywhere:

| per operation                                                                                                          | rusty_alloc | mimalloc |
| ---------------------------------------------------------------------------------------------------------------------- | ----------: | -------: |
| allocation side                                                                                                        |   **26.24** |    30.90 |
| free side                                                                                                              |   **29.39** |    33.45 |

### Finding 3: mimalloc's arm is not deterministic here either

The driver is exact for us — 11,479,732 and 22,100,637, repeatedly. Three runs
of the **unchanged** mimalloc binary against that same fixed iteration count:

| run                                                                                                                                |   mi Ir/op |
| ---------------------------------------------------------------------------------------------------------------------------------- | ---------: |
| 1                                                                                                                                  |     62.069 |
| 2                                                                                                                                  |     61.469 |
| 3                                                                                                                                  | **63.478** |

**A spread of 2.009 Ir/op — 3.3%.** The work is fixed and the binary is
unchanged, so the variation is entirely upstream's own: mimalloc schedules
segment purging against a clock (`_mi_clock_now` sits in its profile), so how
much purge work a run does depends on how long the run happened to take. This
is the third benchmark in this document where the ra arm is exact and the mi
arm is not — see §17 on sh6bench, where the same effect is ±635 k.

The practical consequence is that **our margin here can only be stated as a
range: 0.834× to 0.861×.** The per-phase comparisons in the result table above
come from one mi run apiece and carry that same 3.3% uncertainty; the
rusty_alloc side of every number in this section does not.

### Finding 4: the corpus estimator mis-attributes C++ workloads

`bench/opscan2.sh` is this repo's canonical per-op estimator, and it attributes
cost by matching the SYMBOL NAME — `rusty_alloc` for our side, `mi_` for
upstream's. Its header explains why it does that rather than match the object:
`callgrind_annotate` elides the `[object]` suffix on continuation lines, which
under-counted this allocator ~4x once already.

**Name matching does not work for a C++ benchmark.** The hot symbols here are
`operator new[](unsigned long)` and `operator delete[](void*, unsigned long)`,
which carry NEITHER allocator's name — they are 74% of our allocator's cost on
this driver, and the pattern misses every one of them while picking up source
paths outside the allocator on our side. Measured both ways on the same run:

| attribution                                                                                      | rusty_alloc |   mimalloc | verdict it gives  |
| ------------------------------------------------------------------------------------------------ | ----------: | ---------: | ----------------- |
| by symbol name (`opscan2.sh`'s rule)                                                             |      78.563 |     63.628 | we lose by 23%    |
| **by object, from the raw file**                                                                 |  **52.920** | **61.489** | **we win by 14%** |

Same callgrind output, opposite conclusions. `bench/sizedchurn.sh` therefore
parses the raw file and keys on the shared OBJECT, re-deriving compressed names
including the ids callgrind defines on CALL lines, and it asserts its own
arithmetic — the sum of self costs must equal callgrind's `summary:`, or it
refuses to print a number at all. Anything measuring a C++ path in this corpus
needs the same treatment.

### The seven, in the order they were executed

| #   | primitive                                                                                   |                     Ir/op | verdict         |
| --- | ------------------------------------------------------------------------------------------- | ------------------------: | --------------- |
| W1  | `malloc_or`: OOM handled by a tail-called cold arm, so `operator new` needs no null test    |                **−0.532** | **win**         |
| W2  | fold `malloc_slow`'s body into that cold arm to avoid nesting two cold frames               |            0 (4 Ir total) | flat — reverted |
| W3  | `update_direct`'s range fill as `slice::fill`                                               |                **+0.255** | **REFUTED**     |
| W3b | …as a slice iterator instead                                                                |                **+0.270** | **REFUTED**     |
| W4  | un-park: call `update_direct` only when the queue was empty                                 |                **−0.154** | **win**         |
| W5  | retire: reuse the `first == pg` the guard above already computed                            | 0 here, **−81,936 cfrac** | **win**         |
| W6  | walk: collect a page only when its `free` list is actually dry                              |                **−0.031** | **win**         |
| W7  | walk: delete the reorder branch, which is provably dead                                     |                **+0.025** | **REFUTED**     |

### W1 — a null test in the caller costs the callee's whole fast path

`operator new` must abort on OOM, so `new_impl` called `malloc` and tested the
result. `malloc`'s own doc explains why that is expensive: its miss is a **tail
call** precisely so the fast path needs no callee-saved registers. A caller
that inspects the result and may then need `size` again breaks that — `size`
has to survive the slow-path call, and that one live value gave every C++
`operator new` export a frame: **3.00 Ir on every allocation**, 5.8% of the
allocator, for an arm that never runs.

`alloc::malloc_or(size, on_oom)` has the identical fast path but hands the OOM
handler to the cold arm, so the miss is a tail call again and the export tests
nothing. The wrapper fell from **3.00 to 1.92 Ir/call**, `operator new[]` from
16.17 to 14.84, and the driver by **−0.532 Ir/op**. It applies to every C++
`operator new` export, so it is a win on every C++ benchmark in the corpus, not
just this one.

### W4 and W5 — ask the question you were going to answer anyway

`update_direct` re-derives the bin's block size, its top word index and the
front page before it can decide it has nothing to do — 4.00 Ir to discover a
no-op. Both call sites in `free_local_at` already knew the answer:

- **W4** pushes an un-parked page to the queue's BACK. That can only change the
  head the table tracks if the queue was **empty**, which is one null load.
- **W5** removes a retiring page. That can only change the head if the page
  **was** the head — and `first == pg` is literally the first half of the
  guard on the line above.

This is the same shape as §17's S8: the cheapest optimisations are not clever
code, they are a fact about the data structure that the code was re-deriving.

### W3 and W7 — two refutations that both say "measure, don't reason"

**W3.** `update_direct`'s range fill is **32.17 Ir per queue walk, 21% of the
walk**, because the wide bins span up to 32 word sizes and it writes every
slot. An indexed `while` loop with a bounds check per store looks like the
textbook case for a slice fill. Both slice forms are **worse** — `fill` +0.255,
an iterator +0.270 — because LLVM already compiles the indexed form without a
per-store check, and what the slice forms add is the range's own bounds
computation. The cost here is the WIDTH of a bin's word-size range, which is a
data-structure property; no rewrite of the loop can touch it.

**W7.** The walk's `if p != (*q).first { reorder }` is **dead**: every
iteration removes `p` from the queue before advancing, so the loop is always
looking at the front. A `debug_assert_eq!` in its place ran the entire
`debug_checks` suite without firing. Deleting it measured **worse** — 52.920 →
52.945 on an instrument exact to the unit — because the compare is free in
practice and removing it perturbs register allocation around the return. It was
kept, with the invariant recorded as a comment and a warning not to "clean it
up" without re-measuring.

### W6 — and the reason it is small on purpose

The walk called `page_collect` on every page it visited, including pages whose
free list was already populated. Collecting only when `(*p).free` is dry is
**−0.031 Ir/op**. It is deliberately not more: the guard defers a page's
cross-thread drain, and deferring is already the protocol's contract, but only
because each served block moves that page closer to a dry list and therefore to
a collect. The bound is what makes it safe, and the bound is what keeps it
small.

### Gates run

- `cargo clippy --workspace --all-targets --all-features` — clean
- `cargo test --workspace --all-features` — 32 suites, 0 failed
- `cargo test -p rusty_alloc --features debug_checks` — 0 failed (it is what
  confirmed W7's branch is unreachable)
- `corpus/sweep-all.sh 8` — **19/19**, sweep passed
- full `opscan` — **no op moved** (small 59.22, batch_lifo 59.12, big/large
  122.00, mixed 111.40 — all unchanged from §17's closing figures)
- cfrac **−88,262** as a side effect, to 3,994,735,955
- sqlite **−45,215**; perl unmoved at the level §17 left it

## 19. Campaign executed: `free` — 27 instructions to 21, and the floor that wasn't

**Date:** 2026-08-21/22 · **Target:** `free`, because it is where a real program
spends its allocator time — 55.3% of the allocator on a steady-state working
set, 56% on cfrac · **Outcome:** six wins, seven refutations, one bug of my own
found by a gate I had been reading wrong.

### Result

|                                                                                       |              before |             after |      mimalloc |
| ------------------------------------------------------------------------------------- | ------------------: | ----------------: | ------------: |
| **`free` fast path**                                                                  | **27 instructions** |            **21** |        **25** |
| `free` Ir/call (cfrac, exact)                                                         |              26.999 |        **21.000** |        25.000 |
| `free_general` Ir/call                                                                |                39.0 |          **36.0** |             — |
| cfrac allocator Ir                                                                    |       3,994,735,955 | **3,445,613,016** | 4,038,242,733 |

**−549,122,939 instructions on cfrac (−13.7%)**, and `free` went from two
instructions *above* upstream to four *below*. The corpus followed: `small`
59.22 → **54.22**, `batch_lifo` 59.12 → **53.14**, `usable` 28.00 → **22.00**,
and the allocator-only ratio against mimalloc on real programs moved lua
0.75 → **0.66**, perl 0.95 → **0.83**, sqlite 0.98 → **0.85**.

### The thirteen, in the order they were executed

| #   | primitive                                                                                |             result | verdict                   |
| --- | ---------------------------------------------------------------------------------------- | -----------------: | ------------------------- |
| F1  | `used--` + retire branch as one memory-destination `sub`, cold arm as an `asm!` label    | **−1.999 Ir/call** | **win**                   |
| F2  | field offset as a `const` operand, so addressing carries the displacement                |         **−1.000** | **win**                   |
| F3  | null test folded into the segment mask, in Rust                                          |             +1.000 | **REFUTED**               |
| F4  | fuse the `fs:0` thread-id read into the compare                                          |         **−1.000** | **win**                   |
| G1  | per-slice owner table: pointer&rarr;page in 2 instructions, not 4                        |         **−2.000** | **win**                   |
| G2  | the same null fold, in asm                                                               |                  — | **BLOCKED (rust#119364)** |
| G3  | outline `free_general`'s interior-pointer arm                                            |                  0 | flat                      |
| G4  | outline `free_local_at`'s rare arms                                                      |           +215,880 | **REFUTED**               |
| H1  | reuse `used_now` instead of re-reading for `page_all_free`                               |           +147,456 | **REFUTED**               |
| H2  | skip `free_local`'s null test when draining the delayed list                             |                  0 | flat                      |
| H3  | fuse the thread compare in `free_general` too                                            |       **−149,504** | **win**                   |
| H4  | outline `remote_free` to kill `free_general`'s frame                                     |           +373,760 | **REFUTED**               |
| H5  | hand `owner_tid` into `free_general` instead of re-loading it                            |        **−74,752** | **win**                   |

### F1 — the floor `opps.md` #6 called unreachable

`opps.md` #6 measured this gap on four workloads and refuted it twice from safe
Rust, the second time by deleting the return value entirely — which produced
byte-identical code, because the caller then had to re-read the field. Its
conclusion was that the instruction was not reachable. **The analysis was right
and the conclusion was wrong.** LLVM will not emit a memory-destination
read-modify-write when the decremented value must also drive a branch, and it
will even re-test a value `dec` has already set the flags for:

```text
    mov 0x40(%rdx),%eax ; dec %eax ; mov %eax,0x40(%rdx) ; test %eax,%eax ; jg     5
    subl $0x1,0x40(%rdx) ; jle                                                      2
```

`asm_goto` — `asm!` with a `label` block, stable since Rust 1.87 — is what makes
the second form expressible: the `sub`'s own flags drive a jump into a Rust
block holding the retire arm. `used` is a plain owner-only `u32`, so a
non-atomic RMW is exactly right. `opps.md` #6 is now marked CLOSED, with the
analysis kept, because it is the reason the fix has the shape it does.

### G1 — the biggest single win, and it is a data-structure change

Resolving a pointer to its page was four dependent steps: multiply the slice
index by `size_of::<Page>()` — 88, a real `imul` — add it to the slot array,
load that slot's `slice_offset`, subtract it. Four steps to reach a page whose
address is a pure function of the slice index.

`Segment::page_off` is a `[u32; 512]` recording, per slice, the byte offset from
the segment base to the owning page. The whole resolution becomes
`mov off(%seg,%idx,4)` and an `lea`. It costs **2 KiB in a 32 MiB segment
header that had ~20 KiB spare** and is written only when a span is carved.

It duplicates state, which is a real hazard, so `page_of` carries a
`debug_assert` that the table and `slice_offset` agree on every resolution —
the check that makes the duplication safe to keep, and free in release.

The cost is honest and recorded: carving now writes the table, which is
**+109 Ir/op on `opscan huge`** (535 → 644). On an operation we win by 83×
(644 against mimalloc's 53,366) that is a trade worth taking, and vectorising
the fill measured flat, so it is the carve loop and not the stores.

### H4 and G4 — "cold" is a property of the workload, for the third and fourth time

`free_general` carries an eight-instruction frame, and the disassembly names the
cause exactly: `remote_free` is inlined into it, spin loop and all — there is a
`pause` in the body — and its CAS protocol holds the callee-saved registers.
Outlining it removes the frame and **loses**: `free_general` −1,495,040 but
`remote_free` +1,868,800, net **+373,760**. A workload where this function
matters is one where every free reaches it, so the call is paid every time and
the callee grows a frame of its own.

G4 is the same shape one level down. Splitting `free_local_at`'s huge-segment,
un-park and retire arms out removes its frame — a third of the function — and
still costs **+215,880**, because a cross-thread free lands on the owner's
delayed list, and the pages the owner touches when it drains are exactly the
parked and just-emptied ones.

### H1 — the redundant load that was cheaper than not doing it

`page_all_free(pg)` is `(*page).used == 0`, and `free_local_at` already holds
that value in `used_now` from the push it just performed; the un-park arm
between them cannot change it. Removing the re-read is **+147,456**: keeping
`used_now` live across the un-park's queue calls costs a register, and
re-loading a field already hot in L1 costs less than spilling one.

### The one that is available and was declined

`free`'s page-flags test is three instructions where upstream spends two,
because the byte is an `AtomicU8` and LLVM will not fold an atomic load into a
test's memory operand. Written as `test byte ptr [seg+idx+0x55], 0xf` in asm it
is one — the read stays atomic in hardware, and nothing about the protocol
changes.

It was declined. That byte is atomic because ThreadSanitizer found a genuine
race on it — a thread adopting an abandoned segment rewrites page flags while
another reads them to route a free — and the README publishes the instruction
as a deliberate correctness trade. An `asm!` read is invisible to TSan, so the
tool that caught the bug would go quiet without being satisfied. Buying one
instruction by blinding the sanitizer on the exact byte it found a bug in is
the wrong trade, and it is recorded here as a decision rather than an
oversight.

### The bug this campaign introduced, and the gate that was not looking

Splitting `page_link_local` out of `page_push_local` (F1) moved the blockmap
liveness transition into the new function **and left the original in place**, so
the bit was flipped twice and the second flip read as "already free". The
blockmap aborted, exactly as designed.

It survived several gates because those gates were wrong: they grepped output
for `FAILED|panicked`, and **a `SIGABRT` produces neither**. `tests/abandon_rss`
had been dying silently while the harness reported "32/32, 0 failed". It
surfaced only when cargo's exit code was finally checked (101), and was then
bisected — v1.0.0 passes, so it was mine; then by file to `page.rs`; then by
tagging each abort site with a distinct exit code, which named
`blockmap_abort` in one run.

The harness now reports the exit code. **A gate that greps for words cannot see
a process that dies without saying anything**, and this repository's own
allocator is full of things that abort deliberately and silently.

### Gates run

- `cargo clippy --workspace --all-targets --all-features` — clean
- `cargo test --workspace --all-features` — **exit 0**, 32 suites, 82 tests
- `tests/abandon_rss` — 6/6 (the test that caught the blockmap bug)
- `bench/datasweep.sh 2` — 573,640 checks × 6 arms, 0 failures
- `corpus/sweep-all.sh 8` — 19/19
- full `opscan` — every op improved; none regressed except `huge`, recorded above
