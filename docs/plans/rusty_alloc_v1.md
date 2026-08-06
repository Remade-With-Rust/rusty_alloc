# rusty_alloc v1 — remaking mimalloc in pure Rust

**Status:** planning · **Owner:** remade-with-rust / mata.network · **Created:** 2026-08-05
**Reference oracle:** microsoft/mimalloc `master` @ **v2.4.5** (`MI_MALLOC_VERSION 20405`)

---

## 1. Mission

Remake mimalloc — the segment/page, free-list-sharded, lock-free-cross-thread-free
allocator — in pure Rust, under the remade-with-rust principles:

1. **Memory safety at the boundary.** `unsafe` is unavoidable in an allocator's core,
   but it is confined to named modules, each `unsafe` block carries a stated SAFETY
   invariant, and the public surface (Rust-native *and* C ABI) is a safe interface over
   a small hand-checked core. The discipline is the one we already use for
   `Pin`/free-list style code: *one small unsafe line behind a fully safe interface*.
2. **No C in the product.** The C mimalloc is vendored as a **dev-only oracle** for
   differential testing and benchmarking (same role openh264/dav1d play for our codecs)
   — never a runtime dependency.
3. **General primitive, thin consumers.** rusty_alloc knows bytes, sizes, alignments
   and heaps. It never learns spacedb's or FFAI's types. Products consume it via
   `#[global_allocator]` or the `Allocator` trait, nothing more.
4. **Measured, not vibed.** Every performance claim follows the codec-measurement
   discipline (§7.4): pinned CPU time, ABBA-interleaved arms, null arm, paired win-rate
   with z-score, N ≥ 31 for cross-binary ratios, counters before clocks.

**v1 definition of done:** full mi_* API parity (every function in §5 implemented and
differential-tested), the 1:1 corpus (§7) running against C mimalloc, and the v1 perf
gate met: **geomean wall/CPU time within 10% of C mimalloc across the corpus, no single
benchmark > 25% slower, peak RSS geomean within 15%.** Beating mimalloc is the post-v1
campaign, run with the codec-optimize discipline; v1 is *conformance + credibility*.

---

## 2. Why mimalloc (architecture recap)

The design we are remaking, so the module map in §6 has referents:

- **Free-list sharding.** Every mimalloc *page* (a 64 KiB slice for small objects, up
  to a whole segment for huge ones) has its **own** free list. Allocation is a pop from
  the current page's list — no size-class lookup contention, natural temporal locality,
  bounded list-corruption blast radius.
- **Three free lists per page.**
  - `free` — the fast-path allocation list.
  - `local_free` — frees by the owning thread. Kept separate so the fast path's
    `free == NULL` check doubles as a **heartbeat**: when the fast list runs dry the
    slow path (`malloc_generic`) runs at a regular cadence and does deferred work
    (collect, purge, deferred-free callbacks).
  - `xthread_free` — frees from *other* threads, pushed with an atomic CAS; the owning
    thread drains it lock-free. This is the entire cross-thread story: no locks on
    alloc or free.
- **Heaps → pages → segments.** Each thread has a default heap; heaps keep pages in
  size-class queues (`pages[74]` bins + a `full` queue). Pages live in 32 MiB
  **segments** (v2: sliced segments) carved from OS memory / **arenas**; a global
  **segment map** lets `mi_free` find the segment of any pointer with two shifts and a
  mask. Metadata lives at the segment start — `ptr → page` is pure arithmetic.
- **Abandonment.** A dying thread abandons its live segments; other threads adopt them
  on demand. No thread-death leaks (`mleak` tests exactly this).
- **Security features** (the `smi` build): guard pages, encrypted free lists,
  randomized order — we inherit some of these "for free" from Rust, and implement the
  rest behind a `secure` feature in M8.

Non-goal for v1: mimalloc v3's arena-direct (segment-free) redesign. We remake **v2.4.5**
— it is the line the corpus numbers in the literature refer to, `master` today, and its
architecture is the proven one. v3 becomes a post-v1 experiment (§9).

---

## 3. Repo layout

```
rusty_alloc/
├── Cargo.toml                  # workspace
├── crates/
│   ├── rusty_alloc/            # the allocator library (lib, no_std + extern crate alloc-free core)
│   │   └── src/                # module map in §6
│   ├── rusty_alloc_api/        # safe Rust-native surface: GlobalAlloc, Allocator, Heap (thin)
│   ├── rusty_alloc_ffi/        # cdylib+staticlib: full mi_*-compatible C ABI (ra_* + mi_* aliases)
│   ├── rusty_alloc_override/   # cdylib: exports malloc/free/… (alloc-override.c equivalent)
│   │                           #   → this is what LD_PRELOADs into the 1:1 corpus
│   └── rusty_alloc_bench/      # Tier-B native harness + trace replayer (§7)
├── oracle/
│   ├── mimalloc/               # git submodule pinned at v2.4.5 tag; built by dev scripts only
│   └── build.ps1 / build.sh    # builds mi / smi / dmi shared libs for diff + bench
├── corpus/
│   ├── mimalloc-bench/         # git submodule (daanx/mimalloc-bench, pinned)
│   ├── traces/                 # recorded alloc traces (§7.3) — real workloads, versioned by hash
│   └── run/                    # bench.sh drivers + results (json, one file per run, method line included)
├── bench/
│   └── pinvs.ps1               # ported compliant timing harness (ONE implementation; everything calls it)
└── docs/plans/rusty_alloc_v1.md
```

Toolchain: stable Rust, `cargo check` gated on **x86_64-pc-windows-msvc** and
**x86_64-unknown-linux-gnu** (+ `aarch64-apple-darwin` once a Mac runner exists) before
every push — the compile-gate rule. Tier-A corpus runs need Linux (mimalloc-bench is
Unix-only): **WSL2 on the dev box** is the standing Tier-A environment; a quiet Linux
box is the numbers-of-record environment.

---

## 4. The oracle and the gates

An allocator has no "byte-identical output" to gate on — addresses are not
deterministic across implementations. The gate ladder replacing it:

| gate | what it checks | tool |
|---|---|---|
| **G1 invariants** | every returned block: correctly aligned, `usable_size(p) ≥ size`, disjoint from all live blocks, zeroed when promised (`zalloc`/`calloc`/`rezalloc`), stable until freed (canary fill + check) | `rusty_alloc_bench replay --check` |
| **G2 differential vs oracle** | same recorded trace through us and C mimalloc: identical *semantic* results — success/failure, `usable_size` **bin equality** (same size-class geometry), `good_size` equality, stats counters within tolerance | trace replayer, both `.so`s loaded side by side |
| **G3 model tests** | cross-thread free protocol, abandonment/adoption, delayed-free flags — exhaustively interleaved | `loom` |
| **G4 miri** | UB-freedom of the safe-Rust majority + selected unsafe paths (miri can't run the whole allocator, but page/bin/queue logic runs under it with a mock OS layer) | `cargo miri test` |
| **G5 fuzz** | arbitrary alloc/free/realloc/aligned traces, multi-thread schedules, OOM injection | `cargo fuzz`, 4 targets |
| **G6 corpus** | the 1:1 suite of §7, security suite included | mimalloc-bench |

G1+G2 run in CI on every PR (seconds); G3–G5 nightly; G6 on demand + weekly.
**The slow honest path stays in the tree forever:** a `debug_checks` feature keeps full
invariant checking (list walking, canaries, double-free detection) compiled in — that is
our `dmi` equivalent.

---

## 5. Public API — exact function inventory

The complete exported surface of mimalloc v2.4.5, from `include/mimalloc.h` @ master.
This is the 1:1 contract: **v1 ships when every row is implemented and G2-tested.**
Naming: the C ABI exports `mi_*` names verbatim (drop-in; required for the corpus and
for `mimalloc-bench` to load us as an allocator arm). Internally each maps to a
`snake_case` Rust function of the same name minus prefix.

Milestone column = when it lands (roadmap §8).

### 5.1 Standard malloc interface (M2–M3)

| # | function | notes | MS |
|---|---|---|---|
| 1 | `mi_malloc(size)` | the fast path; ≤ `MI_SMALL_SIZE_MAX` (128 words = 1 KiB) goes via small path | M2 |
| 2 | `mi_calloc(count, size)` | overflow-checked `count*size`, zeroed | M2 |
| 3 | `mi_realloc(p, newsize)` | in-place when bin permits, else alloc+copy+free | M3 |
| 4 | `mi_expand(p, newsize)` | in-place only, NULL if it can't | M3 |
| 5 | `mi_free(p)` | local fast path / xthread CAS push | M2 |
| 6 | `mi_strdup(s)` | | M3 |
| 7 | `mi_strndup(s, n)` | | M3 |
| 8 | `mi_realpath(fname, resolved)` | OS-dependent; Windows `GetFullPathName`, POSIX `realpath` | M7 |

### 5.2 Extended (M2–M3)

| # | function | notes | MS |
|---|---|---|---|
| 9 | `mi_malloc_small(size)` | caller guarantees ≤ `MI_SMALL_SIZE_MAX` | M2 |
| 10 | `mi_zalloc_small(size)` | | M2 |
| 11 | `mi_zalloc(size)` | | M2 |
| 12 | `mi_mallocn(count, size)` | overflow-checked, not zeroed | M2 |
| 13 | `mi_reallocn(p, count, size)` | | M3 |
| 14 | `mi_reallocf(p, newsize)` | frees `p` on failure | M3 |
| 15 | `mi_usable_size(p)` | | M2 |
| 16 | `mi_good_size(size)` | must equal oracle exactly (G2) — pins our bin geometry to mimalloc's | M2 |
| 17 | `mi_free_small(p)` | v3-compat alias | M2 |

### 5.3 Runtime, stats, hooks (M7)

| # | function | notes | MS |
|---|---|---|---|
| 18 | `mi_register_deferred_free(fun, arg)` | called from `malloc_generic` heartbeat | M7 |
| 19 | `mi_register_output(fun, arg)` | all messages route here | M7 |
| 20 | `mi_register_error(fun, arg)` | EINVAL/ENOMEM/EFAULT/EAGAIN codes as mimalloc | M7 |
| 21 | `mi_collect(force)` | drain thread-free lists, return pages, purge | M4 |
| 22 | `mi_version()` | returns our version; `mi_`-compat reports 20405-compatible | M0 |
| 23 | `mi_stats_reset()` | | M7 |
| 24 | `mi_stats_merge()` | | M7 |
| 25 | `mi_stats_print(out)` | legacy, `out` ignored | M7 |
| 26 | `mi_stats_print_out(fun, arg)` | | M7 |
| 27 | `mi_thread_stats_print_out(fun, arg)` | | M7 |
| 28 | `mi_options_print()` | | M7 |
| 29 | `mi_process_info(…8 out-params)` | elapsed/user/sys msecs, rss, commit, faults | M7 |
| 30 | `mi_process_init()` | usually automatic | M4 |
| 31 | `mi_process_done()` | | M4 |
| 32 | `mi_thread_init()` | | M4 |
| 33 | `mi_thread_done()` | triggers abandonment | M4 |

### 5.4 Aligned allocation (M5)

| # | function | MS |
|---|---|---|
| 34 | `mi_malloc_aligned(size, alignment)` | M5 |
| 35 | `mi_malloc_aligned_at(size, alignment, offset)` | M5 |
| 36 | `mi_zalloc_aligned(size, alignment)` | M5 |
| 37 | `mi_zalloc_aligned_at(size, alignment, offset)` | M5 |
| 38 | `mi_calloc_aligned(count, size, alignment)` | M5 |
| 39 | `mi_calloc_aligned_at(count, size, alignment, offset)` | M5 |
| 40 | `mi_realloc_aligned(p, newsize, alignment)` | M5 |
| 41 | `mi_realloc_aligned_at(p, newsize, alignment, offset)` | M5 |

Fast case: alignment ≤ 16 falls through to normal path; ≤ `MI_BLOCK_ALIGNMENT_MAX`
handled inside a page by over-allocating one bin; larger goes to aligned segments.

### 5.5 `u*` block-size-returning variants (new in 2.4.x) (M5)

| # | function | MS |
|---|---|---|
| 42 | `mi_umalloc(size, *block_size)` | M5 |
| 43 | `mi_ucalloc(count, size, *block_size)` | M5 |
| 44 | `mi_urealloc(p, newsize, *pre, *post)` | M5 |
| 45 | `mi_ufree(p, *block_size)` | M5 |
| 46 | `mi_umalloc_aligned(size, alignment, *block_size)` | M5 |
| 47 | `mi_uzalloc_aligned(size, alignment, *block_size)` | M5 |
| 48 | `mi_umalloc_small(size, *block_size)` | M5 |
| 49 | `mi_uzalloc_small(size, *block_size)` | M5 |

### 5.6 First-class heaps (M6)

| # | function | MS |
|---|---|---|
| 50–56 | `mi_heap_new` / `mi_heap_delete` / `mi_heap_destroy` / `mi_heap_set_default` / `mi_heap_get_default` / `mi_heap_get_backing` / `mi_heap_collect` | M6 |
| 57–68 | `mi_heap_malloc` / `zalloc` / `calloc` / `mallocn` / `malloc_small` / `zalloc_small` / `realloc` / `reallocn` / `reallocf` / `strdup` / `strndup` / `realpath` | M6 |
| 69–76 | `mi_heap_{malloc,zalloc,calloc,realloc}_aligned` + the four `_aligned_at` variants | M6 |

Note: internally every allocation is heap-relative from M2 (`heap_malloc(default_heap, …)`)
— M6 only *exposes* heaps; it does not retrofit them. `mi_heap_destroy` (free-everything-
at-once) requires per-heap page ownership tags from day one.

### 5.7 Zero-preserving reallocation (M5)

| # | function | MS |
|---|---|---|
| 77 | `mi_rezalloc(p, newsize)` | M5 |
| 78 | `mi_recalloc(p, newcount, size)` | M5 |
| 79–82 | `mi_rezalloc_aligned[/_at]`, `mi_recalloc_aligned[/_at]` | M5 |
| 83–84 | `mi_heap_rezalloc`, `mi_heap_recalloc` | M6 |
| 85–88 | heap aligned variants of the above | M6 |

### 5.8 Analysis & introspection (M6)

| # | function | MS |
|---|---|---|
| 89 | `mi_heap_contains_block(heap, p)` | M6 |
| 90 | `mi_heap_check_owned(heap, p)` | M6 |
| 91 | `mi_check_owned(p)` | M6 |
| 92 | `mi_heap_visit_blocks(heap, visit_blocks, visitor, arg)` + `mi_heap_area_t` | M6 |
| 93 | `mi_is_in_heap_region(p)` | M3 (needs segment map) |
| 94 | `mi_is_redirected()` | M7 (override crate) |

### 5.9 Arenas, OS memory, subprocesses (M6)

| # | function | MS |
|---|---|---|
| 95 | `mi_reserve_huge_os_pages_interleave(pages, numa_nodes, timeout)` | M6 |
| 96 | `mi_reserve_huge_os_pages_at(pages, numa_node, timeout)` | M6 |
| 97 | `mi_reserve_os_memory(size, commit, allow_large)` | M6 |
| 98 | `mi_manage_os_memory(start, size, committed, large, zero, numa)` | M6 |
| 99 | `mi_debug_show_arenas()` / 100 `mi_arenas_print()` | M6 |
| 101 | `mi_arena_area(arena_id, *size)` | M6 |
| 102–104 | `mi_reserve_huge_os_pages_at_ex` / `mi_reserve_os_memory_ex` / `mi_manage_os_memory_ex` (exclusive + `*arena_id` out) | M6 |
| 105 | `mi_heap_new_in_arena(arena_id)` | M6 |
| 106–109 | `mi_subproc_main` / `mi_subproc_new` / `mi_subproc_delete` / `mi_subproc_add_current_thread` | M6 |
| 110 | `mi_abandoned_visit_blocks(subproc, heap_tag, visit_blocks, visitor, arg)` | M6 |
| 111–112 | `mi_heap_guarded_set_sample_rate` / `mi_heap_guarded_set_size_bound` | M8 |
| 113 | `mi_thread_set_in_threadpool()` | M4 |
| 114 | `mi_heap_new_ex(heap_tag, allow_destroy, arena_id)` | M6 |
| 115 | `mi_unsafe_heap_page_is_under_utilized(heap, p, perc)` | M6 |
| 116 | `mi_reserve_huge_os_pages(pages, max_secs, *reserved)` (deprecated) | M6 |
| 117 | `mi_collect_reduce(target)` (deprecated) | M6 |

### 5.10 Options (M7; the enum lands M0 as a stub)

| # | function | MS |
|---|---|---|
| 118–122 | `mi_option_is_enabled` / `enable` / `disable` / `set_enabled` / `set_enabled_default` | M7 |
| 123–127 | `mi_option_get` / `get_clamp` / `get_size` / `set` / `set_default` | M7 |

Full `mi_option_t` enum (37 active options incl. `eager_commit`, `purge_decommits`,
`purge_delay`, `arena_reserve`, `abandoned_reclaim_on_free`, `guarded_*`,
`generic_collect`, `allow_thp`, legacy aliases) is copied verbatim — including
deprecated slots, so option *indices* stay ABI-compatible. Environment parsing
(`MIMALLOC_…` and `RUSTY_ALLOC_…` prefixes both accepted) in M7.

### 5.11 POSIX / Windows / C compatibility layer (M5, `realpath`-likes M7)

| # | function | MS |
|---|---|---|
| 128 | `mi_cfree(p)` — checked free | M5 |
| 129 | `mi__expand(p, newsize)` | M5 |
| 130–132 | `mi_malloc_size` / `mi_malloc_good_size` / `mi_malloc_usable_size` | M5 |
| 133 | `mi_posix_memalign(*p, alignment, size)` | M5 |
| 134 | `mi_memalign(alignment, size)` | M5 |
| 135 | `mi_valloc(size)` / 136 `mi_pvalloc(size)` | M5 |
| 137 | `mi_aligned_alloc(alignment, size)` | M5 |
| 138 | `mi_reallocarray(p, count, size)` | M5 |
| 139 | `mi_reallocarr(ptrp, count, size)` | M5 |
| 140 | `mi_aligned_recalloc(p, newcount, size, alignment)` | M5 |
| 141 | `mi_aligned_offset_recalloc(p, newcount, size, alignment, offset)` | M5 |
| 142–144 | `mi_free_size` / `mi_free_size_aligned` / `mi_free_aligned` (sized-free fast paths — `larsonN-sized` exercises these) | M5 |
| 145 | `mi_dupenv_s` / 146 `mi_wdupenv_s` | M7 |
| 147 | `mi_wcsdup(s)` / 148 `mi_mbsdup(s)` | M7 |

### 5.12 C++ `new` semantics (M7 — needed by corpus programs built as C++)

| # | function | MS |
|---|---|---|
| 149–152 | `mi_new` / `mi_new_aligned` / `mi_new_nothrow` / `mi_new_aligned_nothrow` (OOM → retry via error handler; nothrow → NULL) | M7 |
| 153 | `mi_new_n(count, size)` | M7 |
| 154–155 | `mi_new_realloc` / `mi_new_reallocn` | M7 |
| 156–157 | `mi_heap_alloc_new` / `mi_heap_alloc_new_n` | M7 |

Convenience macros (`mi_malloc_tp` etc.), the STL allocators, and the `mi_theap_*` v3
shims are header-only C/C++ — they ship in our installed `mimalloc.h`-compatible header
(generated by cbindgen + hand-carried macro block), no Rust work beyond the functions
above.

### 5.13 Override surface (`rusty_alloc_override`, M7)

Mirrors `alloc-override.c`: exports `malloc`, `calloc`, `realloc`, `free`,
`posix_memalign`, `memalign`, `aligned_alloc`, `valloc`, `pvalloc`,
`malloc_usable_size`, `reallocarray`, `reallocf`, `strdup`, `strndup`, C++
`operator new/delete` (all 14 variants incl. sized + aligned), plus macOS zone/interpose
tables when we add a Mac target. **This crate is how the 1:1 corpus runs unmodified.**
Linux (LD_PRELOAD) first; Windows redirection (mimalloc-redirect equivalent) is
explicitly **post-v1** — on Windows we bench via the Rust-native tier and static linking.

### 5.14 Rust-native API (`rusty_alloc_api`, grows M2 → M8)

```rust
// #[global_allocator] — the one-liner every remade-with-rust crate adopts
pub struct RustyAlloc;                       // unsafe impl GlobalAlloc  (M2)
pub struct Heap { .. }                       // first-class heap, !Send by design (M6)
impl Heap {
    pub fn new() -> Heap;                    // delete-on-drop
    pub fn new_destroyable() -> Heap;        // destroy-on-drop (arena-style teardown)
    pub fn alloc / zalloc / realloc / …      // safe, NonNull returns, Layout-based
}
unsafe impl Allocator for &Heap { .. }       // nightly/allocator_api feature-gated (M6)
pub fn collect(force: bool);                 // + stats(), process_info() typed structs (M7)
```

Design rule: the Rust API is *thin* over the same internals as the C ABI — no separate
code path, so the corpus numbers speak for Rust users too.

---

## 6. Internal architecture — module map (C file → Rust module)

Same decomposition as upstream so every diff-vs-oracle conversation has a shared map.
Key internal functions listed are the load-bearing ones; statics fall where they fall.

| mimalloc C | rusty_alloc module | key contents | MS |
|---|---|---|---|
| `include/mimalloc/types.h` | `types.rs` | `Page`, `Segment`, `Heap`, `Tld`, `Block`, size constants (`MI_SMALL_SIZE_MAX`=1 KiB, `MI_SEGMENT_SIZE`=32 MiB, 74 bins), encoded free-list pointers | M1 |
| `prim/*` (unix/windows/osx) | `prim/{windows,unix}.rs` | `alloc/free/commit/decommit/reset/protect`, huge pages, NUMA node count, clock, thread-id, TLS slot | M1 |
| `os.c` | `os.rs` | alignment-satisfying OS alloc, overcommit detection, purge policy (decommit vs reset) | M1 |
| `arena.c`, `arena-abandon.c` | `arena.rs` | arena reserve/manage, block bitmap alloc, abandoned-segment lists per subproc | M6 (minimal stub M3) |
| `bitmap.{c,h}` | `bitmap.rs` | atomic field bitmap: `try_find_claim`, cross-field claim, unclaim | M6 |
| `segment.c` | `segment.rs` | segment alloc/free, slice spans, page alloc within segment, abandonment: `segment_abandon`, `abandoned_reclaim` | M3–M4 |
| `segment-map.c` | `segment_map.rs` | global 1-bit-per-32MiB map answering `mi_is_in_heap_region` / validating foreign frees | M3 |
| `page.c` | `page.rs` | `page_fresh`, `page_free_collect` (drain local+xthread), `page_retire`, `malloc_generic` (the heartbeat slow path) | M2 |
| `page-queue.c` | `page_queue.rs` | 74 size bins + full-queue, `bin(size)` mapping (must match oracle exactly — G2 pins it via `good_size`) | M2 |
| `alloc.c` | `alloc.rs` | `mi_malloc`/`zalloc`/small fast paths, block init, canaries under `debug_checks` | M2 |
| `free.c` | `free.rs` | `mi_free` fast/local/xthread paths, `usable_size`, sized-free | M2 |
| `alloc-aligned.c` | `alloc_aligned.rs` | §5.4 + §5.5 | M5 |
| `alloc-posix.c` | `posix.rs` | §5.11 | M5/M7 |
| `heap.c` | `heap.rs` | heap lifecycle, `heap_collect`, `heap_destroy` page-tag sweep, visitor | M4/M6 |
| `init.c` | `init.rs` | static empty-heap bootstrap, main-heap init, thread init/done hooks (platform TLS dtor), process init/done | M2/M4 |
| `options.c` | `options.rs` | option table, env parsing, message routing (out/error hooks, buffered pre-init) | M7 (stub M0) |
| `stats.c` | `stats.rs` | per-heap counters merged to process stats, printing | M7 (counters from M2 — they are also our bench instruments) |
| `random.c` | `random.rs` | ChaCha8 CSPRNG for free-list encoding & guarded sampling (`rand_core`-free, self-contained) | M2 (weak), M8 (full) |
| `libc.c` | — | not needed; `core`/`std` provide | — |
| `alloc-override*.c` | `rusty_alloc_override` crate | §5.13 | M7 |
| `static.c` | `rusty_alloc_ffi` staticlib config | single-object static build | M7 |

**Unsafe policy per module:** `prim/*`, `alloc.rs`/`free.rs` block-pointer ops,
`segment.rs` metadata casts, and `bitmap.rs` atomics are the *only* modules allowed
`unsafe`. Each unsafe fn states its invariant; `debug_checks` asserts them dynamically;
G3/G4/G5 hunt the seams. Everything above (queues, bins, options, stats, heap logic)
is written in safe Rust over typed handles — the codec campaigns proved the safe/`unsafe`
boundary tax is ~0 when the layout is right; we expect the same here (validated in M2,
see risk R2).

**Before any `unsafe`-for-speed change:** read the emitted assembly and count the
bounds checks the compiler actually kept (`cargo rustc --release -- --emit asm`, then
attribute `panic_bounds_check` sites). The audited pattern from the codec work: most
"hot bounds checks" either never execute on the shipped path or were already elided
because a mask/length proof exists — and when a real one survives, the fix is usually
*restructuring so the compiler can prove the bound* (hoist the edge test), not
`get_unchecked`. Where genuine, the applicable patterns are the standard five:
unchecked indexing over proven bins, uninitialized page memory (never zero what
`malloc` doesn't promise — only `zalloc` paths pay for zeroing, and fresh-committed OS
pages are zero *by the OS*, a fact mimalloc tracks per page with `is_zero` and we must
too or `calloc` pays double), raw-pointer sharing with documented partitioning
(xthread lists), and `repr(C)` reinterpretation at the segment-metadata boundary.

---

## 7. The 1:1 benchmark corpus

### 7.1 Tier A — mimalloc-bench, unmodified (the number of record)

`corpus/mimalloc-bench` submodule, pinned. Our arm is `rusty_alloc_override.so` via
LD_PRELOAD, registered in `build-bench-env.sh` as allocator `ra` (plus `ras` = secure
build, `rad` = debug — mirroring `mi`/`smi`/`dmi`). Environment: WSL2 for the dev loop,
quiet Linux box for record runs. Arms of record: **ra vs mi** (primary), with
`je`/`tc`/`sn`/`sys` context arms quarterly.

The full suite, 1:1 with upstream:

| class | benchmark | what it stresses |
|---|---|---|
| real-world | `cfrac` | many small short-lived allocs, single thread |
| real-world | `espresso` | PLA analyzer, cache-aware alloc pattern |
| real-world | `barnes` | n-body, few allocs, multithreaded |
| real-world | `gs` | ghostscript over the ~5000-page Intel SDM PDF |
| real-world | `leanN` | Lean 3.4.1 compiling its stdlib, N threads — the big one |
| real-world | `redis` | redis-benchmark 1M requests, rps metric |
| real-world | `larsonN` | 100-thread server sim, cross-thread frees ("bleeding") |
| real-world | `larsonN-sized` | same + sized deallocation fast path |
| real-world | `lua` | compiling the Lua interpreter |
| real-world | `z3` | z3 computations |
| stress | `alloc-testN` | 100M allocs/thread, Pareto sizes ≤ 1 KiB, N ∈ {1, cores} |
| stress | `cache-scratch` | passive false sharing (Hoard) |
| stress | `cache-thrash` | active false sharing / heap cache locality (Hoard) |
| stress | `glibc-simple`, `glibc-thread` | glibc benchtests |
| stress | `malloc-large` | multi-MiB allocations |
| stress | `mleak` | thread-termination leaks (tests abandonment) |
| stress | `rptest` | rpmalloc-benchmark suite |
| stress | `mstressN` | phased server sim, thread churn, surviving objects |
| stress | `rbstress` | ruby allocator_bench chunks |
| stress | `sh6bench` | LIFO + reverse-order frees |
| stress | `sh8benchN` | multithreaded shbench, cross-thread frees |
| stress | `xmalloc-testN` | 100 pure-alloc + 100 pure-free threads — the thread-cache killer |
| security | `bench/security` suite | double-free, overflow, UAF behavior — run against `ras` |

Metrics per run: wall time, **CPU time (user+sys)**, peak RSS, page faults — exactly
what `bench.sh` + `mi_process_info` already report. Plus our own counters (§7.4).

### 7.2 Tier B — Rust-native harness (`rusty_alloc_bench`, the inner loop)

Native ports of the *kernels* that drive keep/revert decisions daily, runnable on the
Windows dev box without WSL, calling `rusty_alloc_api` (and the oracle through its C
ABI — same driver, both arms in-process or split-process):

- `bench malloc-small` — the cfrac/alloc-test inner pattern (Pareto sizes, LIFO+FIFO mixes)
- `bench larson` — faithful port of the larson loop incl. bleeding + sized variant
- `bench xmalloc` — asymmetric producer/consumer threads
- `bench mstress` — phased churn
- `bench malloc-large`, `bench cache-scratch`, `bench cache-thrash`
- `bench replay <trace>` — see §7.3

These are for *deltas*, not standing claims (§7.4 rule 3).

### 7.3 Trace corpus — our own real workloads

mimalloc-bench covers the literature; it does not cover *our* products. We record
alloc/free/realloc traces (op, size, alignment, thread-id, ptr-identity) with a shim
allocator from:

- **spacedb** ingest + query soak (CRDT merge churn)
- **rusty_h264 / remade_ffmpeg_rs** encode+decode runs (frame-buffer lifecycle)
- **FFAI** inference session (tensor arena pattern)
- **dioxus desktop app** startup + interaction (mata-master)

Traces are versioned by content hash in `corpus/traces/`, replayed by
`bench replay` for both G2 (correctness diff) and timing. **The corpus the code has
never seen is also a conformance test** — reference-workload traces have caught real
defects in every codec bring-up; expect the same here. (Real-content law: a cost
calibrated on synthetic patterns can be off by an order of magnitude — the trace tier
is the guard against tuning to `alloc-test`'s Pareto distribution.)

### 7.4 Measurement discipline (binding, from codec-measurement)

1. **One compliant timing harness** — `bench/pinvs.ps1` ported from rs_h264 (pinned
   core, High priority, CPU time via cached handle, ABBA interleaving, paired win-rate
   + z-score, refuses sub-resolution reports). Tier-A gets the same shape in a
   `bench-ra.sh` wrapper (taskset + `/usr/bin/time`, interleaved arm order). Every
   result prints its **method line**.
2. **Null arm per session** — two identical `mi` arms establish the session's floor
   before any ra-vs-mi number is read.
3. **Same-binary delta = progress; cross-binary ratio = standing.** Keep/revert
   decisions use same-binary A/B. The ra-vs-mi ratio is quoted only at **N ≥ 31**,
   watching the trend across N=15/31/41 — cross-binary medians walk.
4. **Counters before clocks.** `stats.rs` counters (allocs by bin, page fresh/retire,
   segment alloc, xthread frees, commit/purge syscalls, generic-path entries) are the
   primary evidence for sub-1% bricks; the clock confirms batches. Work-count parity
   (identical op counts both arms) is checked by the replayer before any timing is read.
5. **Both arms same work** — corpus runs verify benchmark-reported op counts/rps match
   between arms; divergence voids the run.
6. **The oracle's defaults are configuration.** Bench `mi` with its stock options; any
   option we set on one arm is set on both; `MIMALLOC_SHOW_STATS` etc. off for timing.

### 7.5 The allocator's own instruments (codec-analyzer shape, built in M2)

Optimization is downstream of measurement; the spine gets built with the allocator,
not after it:

1. **Path profiler** — feature-gated (`profile`), rdtsc-based, zero-cost-when-off
   (ZST guard, shipped build byte-identical): buckets over *paths*, not functions —
   `malloc_small_fast`, `malloc_generic`, `free_local`, `free_xthread_push`,
   `page_free_collect`, `page_fresh`, `segment_alloc`, `os_commit`, `os_purge`.
   Residue decomposed until every line is named; residue vs `calls × scope-cost`
   checked before believing it (the profiler is part of the system under test).
2. **Counter suite** (always on, plain `u64` per-heap, merged at stats) — allocs/frees
   per bin, generic-path entries per 10k allocs, pages fresh/retired, segments
   allocated/abandoned/reclaimed, xthread frees, commit/decommit/purge syscall counts,
   zero-init skips. These are the *primary* evidence for every sub-1% brick and the
   work-parity check for every A/B.
3. **Deterministic benchmark** — Tier-B kernels (§7.2) at fixed seeds, best-of-N,
   profiler OFF: the honest number and the per-brick A/B gate.
4. **Cache/bounds probes on demand** — the frame-size-sweep analogue (working-set
   sweep across L2/L3 by varying live-heap size) before any locality refactor, and the
   throwaway `get_unchecked` ceiling probe before believing any "bounds checks are the
   gap" hypothesis (see §6 unsafe policy).
5. **Profile the deployment config** — `debug_checks` off, `secure` off (and separately
   on, for the `ras` story), real traces (§7.3) not just Pareto synthetics: synthetic
   content mis-ranks stages 2–3× in every codec campaign; assume allocation patterns
   behave the same until proven otherwise.

---

## 8. Roadmap

Each milestone ends with its gate green in CI and a one-page LEDGER entry (what
landed, numbers with method lines, what was reverted and which kind of revert).
Sequencing is single-threaded-correct → multithreaded-correct → complete → fast → hard.

### M0 — Scaffold + oracle + harness (the week-one brick)
Workspace, crates, CI (fmt/clippy/check both targets/test), oracle submodule pinned
@ v2.4.5 + build scripts producing `mi`/`dmi`/`smi` libs, mimalloc-bench submodule +
WSL2 bootstrap doc, `pinvs.ps1` port, trace format + recording shim, `mi_version`.
**Gate:** oracle builds; `bench.sh mi cfrac` runs in WSL2; null-arm session recorded.

### M1 — OS primitive layer (`prim/`, `os.rs`)
VirtualAlloc/mmap with alignment trick, commit/decommit/purge/reset, large/huge page
reservation, NUMA detection, thread-id, monotonic clock, TLS slot with destructor
(Windows `FlsAlloc` + fallback; Linux `pthread_key` + `#[thread_local]` fast path).
**Gate:** prim unit tests both OSes; miri-clean mock; alignment/commit invariants fuzzed.

### M2 — Single-threaded core (the allocator exists)
`types.rs`, bins + page queues (bin geometry pinned to oracle via `good_size` G2),
pages with three free lists (xthread list present but single-threaded-only drained),
`malloc`/`zalloc`/`calloc`/`mallocn`/`malloc_small`/`zalloc_small`/`free`/
`usable_size`/`good_size`, `malloc_generic` heartbeat, page retire, `init.rs` static
bootstrap (no allocation before main, works as `#[global_allocator]` from this
milestone), free-list encoding + ChaCha8, stats counters, `debug_checks`, and the
instrument spine of §7.5 (path profiler + counter suite).
**Gate:** G1+G2 on single-thread traces (cfrac/espresso recorded); `RustyAlloc` as
global allocator survives `cargo test` of a real crate (spacedb unit suite); Tier-B
`malloc-small` first numbers (context only, no claims).

### M3 — Segments, large objects, realloc
`segment.rs` slice-based segments, `segment_map.rs`, large (≤ segment) + huge
(dedicated segment) paths, `realloc` family (`realloc`/`reallocn`/`reallocf`/
`expand`), `strdup`/`strndup`, `mi_is_in_heap_region`.
**Gate:** G2 over full-size-spectrum traces incl. `malloc-large` pattern; fuzz target
`realloc-storm`; gs/lua traces replay clean.

### M4 — Multithreading (the hard milestone)
Thread-local heaps via TLS, xthread free CAS push + owner drain, delayed-free flag
protocol (`use_delayed_free`/`delayed_freeing`/`never_delayed`), `mi_collect`,
thread init/done, segment abandonment + reclaim (`abandoned_reclaim_on_free` path),
`mi_thread_set_in_threadpool`, process init/done.
**Gate:** **G3 loom models of the xthread-free and abandonment protocols** (written
*before* the implementation, TLA+-style); G5 multithread fuzz under TSan; `mleak`,
`larson`, `xmalloc-test` Tier-B ports run clean for hours; G2 on multithreaded traces.

### M5 — Aligned + POSIX + zero-preserving + sized (API completeness, part 1)
§5.4, §5.5, §5.7 (non-heap), §5.11 core. Sized-free fast path (`larsonN-sized`).
**Gate:** G2 aligned/offset traces (incl. `aligned_at` edge geometry); fuzz target
`align-storm`; posix layer passes glibc benchtest correctness mode.

### M6 — Heaps + arenas + subprocs (API completeness, part 2)
§5.6, §5.7 heap variants, §5.8, §5.9. `bitmap.rs`, arena reserve/manage, huge-page
arenas, `heap_destroy` tag sweep, visitors, `Heap`/`Allocator` Rust API.
**Gate:** G2 heap-API differential suite (dedicated driver — traces don't exercise
heaps); heap lifecycle fuzz (new/delete/destroy interleaved with cross-heap frees);
`mi_heap_destroy`-vs-`Vec` UAF impossible by construction in Rust API (compile tests).

### M7 — Options, stats, hooks, override, C header (drop-in complete)
§5.3, §5.10, remaining §5.11–5.12, `rusty_alloc_ffi` (cdylib/staticlib + generated
`mimalloc.h`-compatible header), `rusty_alloc_override` LD_PRELOAD crate, `ra` arm
registered in mimalloc-bench.
**Gate:** **full Tier-A corpus runs green as `ra`** — every §7.1 benchmark completes,
security suite behavior documented; first corpus-wide ra-vs-mi table published at
N ≥ 31 with method lines. This is the *conformance* ship gate.

### M8 — Hardening + v1 perf gate
Secure feature (`guard pages, encrypted+randomized free lists, guarded-object
sampling` — §5.9 #111–112), `ras` arm vs `smi` arm on corpus + security suite.
Then the v1 perf campaign: profile → the biggest gaps only, using the
codec-optimize brick discipline (one change, gated, revert-if-not-faster; batch
sub-1% bricks behind one switch with counters as primary evidence).
**Gate (= v1 release):** geomean within 10% of `mi`, no benchmark > 25% behind, RSS
geomean within 15%, security suite ≥ `smi` behavior, all G1–G6 green, LEDGER complete.

Dependency notes: M2 needs M1; M4 needs M3 (abandonment moves segments); M6 needs M4
(heap lifecycle interacts with abandonment); M7 needs M5+M6 (override forwards
everything); M8 last. M5 can proceed in parallel with M4 after M3.

---

## 9. Post-v1 (parked, explicitly out of scope)

- **Beat-mimalloc campaign** — target the workloads where our products live (trace
  tier), not the geomean; content-adaptive ideas (per-thread heap policies) go through
  the ledgered-experiment discipline.
- **mimalloc v3 architecture** (segment-free, arena-direct pages) as an experiment
  branch once the v2-shape corpus baseline exists.
- **Windows override/redirection** (mimalloc-redirect equivalent).
- **macOS zone interposition** + aarch64 numbers.
- **no_std embedded profile** (prim layer behind a trait, static arena backend).
- Adoption rollout across remade-with-rust repos (spacedb first — it has the trace).

## 10. Risks

| # | risk | mitigation |
|---|---|---|
| R1 | **TLS fast-path cost in Rust** — C mimalloc leans on `__thread` + init tricks; Rust `thread_local!` adds a lazy-init branch and `#[thread_local]` is nightly/platform-dependent. This is *the* fast-path risk. | M2 spike: measure `thread_local!` vs `#[thread_local]` vs FFI-style TLS on both OSes **before** committing the design; the heartbeat design tolerates a slightly costlier slow path, not a costlier fast path. |
| R2 | Bounds/`NonNull` checks taxing the ~5-instruction fast path | counters + `.s` inspection in M2; layout-first (the codec lesson: safe Rust reaches C speed via layout, not via `unsafe` sprinkling); `get_unchecked` only with a named invariant |
| R3 | loom state-space explosion on the delayed-free protocol | model the *protocol* (3 flags, 2 threads) not the allocator; bounded exploration; TSan fuzz as the wide net |
| R4 | Trace replay diverges because addresses inform mimalloc's behavior (first-fit in bitmap etc.) — G2 flakiness | G2 compares *semantic* results and bin geometry, never addresses; tolerance bands on stats; flaky comparisons get investigated, never widened silently |
| R5 | WSL2 numbers drift vs bare-metal Linux | WSL2 = dev loop only; numbers of record from the quiet Linux box; both print method lines so provenance is never ambiguous |
| R6 | Corpus programs (gs, lean, redis, z3) are heavy to build and flaky in CI | Tier-A weekly + on-demand, not per-PR; per-PR gate is G1+G2+Tier-B |
| R7 | `mi_heap_destroy` + Rust aliasing rules (freeing blocks the program still references is UB we can't make safe) | C ABI: documented as inheriting C's contract; Rust API: `new_destroyable()` ties destroy to `Drop` and the borrow checker makes outliving references unrepresentable — the illegal-state-unrepresentable pattern |

## 11. Open questions (decide during M0/M1, none block the plan)

1. **Symbol strategy:** export `mi_*` only from `rusty_alloc_ffi`, or also `ra_*`
   aliases? (Leaning: `mi_*` verbatim + `ra_*` aliases; costs nothing, keeps our name
   on the door.)
2. Rust edition/MSRV policy and whether `allocator_api` (nightly) is feature-gated or
   we ship our own `Allocator` trait until stabilization.
3. Whether spacedb adopts rusty_alloc behind a feature flag at M2 (early real-world
   soak) or waits for M4 (thread safety) — leaning M4, with the M2 soak done in a
   single-threaded harness instead.

---

*Sources: `mimalloc.h` v2.4.5 (master, MI_MALLOC_VERSION 20405), mimalloc src listing,
daanx/mimalloc-bench README (master), "Mimalloc: Free List Sharding in Action" (Leijen,
Zorn, de Moura, APLAS'19). Discipline: codec-measurement, codec-optimize,
rusty-blazing-fast, rusty-unsafe-optimizations skills.*
