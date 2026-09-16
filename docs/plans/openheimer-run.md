# Openheimer run — rusty_alloc

**Repo**: rusty_alloc (`f:/coding/rusty_alloc`)
**Class(es)**: alloc — the process heap. Untrusted inputs are sizes, alignments, counts, and pointers handed to `free`/`realloc`.
**Tier**: critical-path — every Remade crate that installs this as `#[global_allocator]` inherits its failure modes.
**Commit SHA**: `6a5efabd0af458fe89591e3dcce226be22aa6877`
**Date**: 2026-09-15
**Operator**: Openheimer campaign (first Wave A unit — bottom of the supply chain)
**Last use-protection-please audit**: [docs/plans/use-protection-please.md](use-protection-please.md) (workspace) · crate: `crates/rusty_alloc/docs/plans/use-protection-please.md`
**Threat model**: [docs/threat-model.md](../threat-model.md)
**SECURITY.md**: yes
**Eligibility (§3.7)**: **internal-only** — H-27 soak completes 2026-09-19; no 14-day Openheimer recert yet; public bounty fragment not enabled.

**Red log (hand-off):** [`f:/coding/openheimer/docs/results/rusty_alloc_openheimer_results.md`](../../../openheimer/docs/results/rusty_alloc_openheimer_results.md) — append-only; this run file does not replace it.

**Disposition (2026-09-16, branch `openheimer-real`):** the campaign's working tree was SPLIT, not landed. §4 lists what landed (boundary validation: values a caller can choose, on cold paths or at zero local-path cost), what was withdrawn (guards on internal `unsafe fn` contracts that taxed the hot path — 139 of the 202 IDs), and the one fix the log claimed that had never been written (OH-11). Every number in §4 is from the release assembly, main against branch, Windows and Linux.

Why this unit first: it sits under SpaceDB, Deputy, FFAI, remade_ffmpeg_rs, and every deliverable that pulls `rusty_alloc_default`. A panic, wrap, or silent double-free here is every product's bug.

---

## 1. Surfaces

| Surface | Where it lives | Untrusted input | Probe pack | Standing test / fuzz target |
|---|---|---|---|---|
| Library alloc API | `crates/rusty_alloc/src/alloc.rs` | size, count, align, `*mut u8` | alloc | `tests/openheimer.rs`; openheimer `probes/rusty_alloc/tests/hostile.rs`; `tests/alloc_core.rs`; `tests/properties.rs`; `fuzz/fuzz_targets/alloc_ops.rs` |
| Double-free / corruption | `page_push_local`, free-list | twice-freed / poisoned link | alloc | `tests/double_free.rs`; `tests/corruption.rs`; `fuzz/fuzz_targets/corruption.rs` |
| Cross-thread free | `xthread` protocol | remote `free` | alloc | `fuzz/fuzz_targets/xthread.rs`; loom (cited in threat model) |
| C ABI | `crates/rusty_alloc_ffi` | `mi_*` / malloc | alloc + bin | ffi crate tests; same core |
| `#[global_allocator]` | `rusty_alloc_api` / `_override` / `_default` | the host process | alloc | `rusty_alloc_bench/tests/selfhost.rs` |
| `secure` / `blockmap` / `linkcheck` | features, off by default | hostile in-process attacker | alloc | `tests/secure.rs`; `tests/corruption.rs` (needs `--features secure`) |
| no_std / chip | `prim::fixed`, `--cfg ra_single_threaded` | firmware region | alloc | compile_error without the cfg; Wave D overlap |
| Supply chain | `deny.toml`, lockfile | crates.io | posture | H-07–H-14 completed in UPP |

---

## 2. Failure classes in play

| F-ID | Applies? | Probe |
|---|---|---|
| F-01 panic on garbage | yes (abort on corruption is *policy*) | `double_free.rs` — abort, not continue |
| F-02 unbounded CPU | low | collect/heartbeat bounded; no regex |
| F-03 unbounded alloc | yes | OOM → null on malloc paths; R-003 heap-create OOM still aborts (accepted) |
| F-04 integer wrap | yes | `openheimer.rs` calloc/mallocn/recalloc overflow → null; `is_aligned_to` total |
| F-05 unsafe OOB | yes | `UNSAFE.md`; Miri in CI; SIMD N/A |
| F-06 forged success | n/a (no identity) | — |
| F-07 scope widening | n/a | — |
| F-08 replay / expiry | n/a | — |
| F-09 secret disclosure | residual | H-20 N/A (no user secrets); H-35 analysed; recycled blocks / zalloc |
| F-10 integrity / substitution | supply chain | deny + lockfile |
| F-11 differential lie | yes | mimalloc oracle / icount corpus (perf), not a security oracle |
| F-12 deser gadget | n/a | — |
| F-13 wasm blow-up | wasm prim | `rusty_alloc_wasm`; single-thread static TLS |
| F-14 supply-chain execute | no build.rs in core | deny policy |
| F-15 authz skip | n/a | — |

---

## 3. Live jobs (evidence)

| Cadence | Job / command | Last SHA | Result | Log URL |
|---|---|---|---|---|
| PR | `.github/workflows/ci.yml` (test, deny, audit, miri, fuzz smoke) | origin/main | assumed live; this run re-executes the Openheimer pack locally | |
| Nightly fuzz | `.github/workflows/fuzz.yml` — alloc_ops, xthread, corruption+secure; corpus cached | soak started 2026-08-19 | H-27 Scheduled until **2026-09-19**; ~7.0M execs cited, 0 crashers at last UPP | GitHub Actions `fuzz` |
| Miri / sanitizer | CI miri on api + properties; ASan on fuzz soak | cited in UPP H-23/H-26 | Completed for Miri tests that exist | |
| Differential corpus | mimalloc-bench / icount | perf, not fail-closed | N/A for Openheimer | |

This run executed locally (2026-09-15): `cargo test -p rusty_alloc --test openheimer` in this repo, and `cargo test -p oh-rusty-alloc --test hostile` in `f:/coding/openheimer` (see audit log).

---

## 4. Findings this run — as landed

### 4.1 Landed (boundary validation)

| IDs | Surface | Untrusted value | Fix | Regression |
|---|---|---|---|---|
| OH-1..4 | threat model | — | already accepted residuals / calendar (R-001, R-003, H-27) | `tests/foreign_free.rs`; threat model |
| OH-5 | `bins::is_aligned_to` | `align` | `is_aligned_to(x, 0)` was true for every `x`; now `align != 0 && align & (align-1) == 0 && x & (align-1) == 0` (spelled out: `is_power_of_two()` was a 20-instruction SWAR popcount without `popcnt`) | `oh_f04_is_aligned_to_is_total` |
| OH-6, 42..45 | `malloc_aligned_at`, `realloc_aligned_at`, `rezalloc_aligned_at`, `huge_alloc`, FFI twins | `offset`, `align` | `bins::aligned_at_from` / `is_aligned_at` checked; refuse with null and release the oversize block or huge reservation | `oh_f04_aligned_at_huge_offset_returns_null`, `oh_f04_realloc_inplace_huge_offset_is_null`, `oh_f04_rezalloc_*`, FFI `oh_f04_heap_realloc_*` |
| OH-7 | `debug_foreign_pointer_guard` | `p` | `assert!` — `debug_assert!` was compiled out of `--release --features debug_checks` | `--release --features debug_checks --test foreign_free` |
| OH-8 | `Heap::malloc_aligned_at` peek | `align == 0` | `align.wrapping_sub(1)` (identical release codegen; debug no longer panics before the slow path refuses) | `oh_f01_align_zero_on_warm_heap_returns_null` |
| OH-9, 10, 28, 29, 31 | `huge_alloc`, `os::page_align_up`, `os::alloc_aligned`, guarded alloc | `size`, `align` | checked adds / muls; garbage align is `Err(22)`; unrepresentable is `Err`/null | `oh_f01_huge_nonwrapping_size_returns_null`, `oh_f01_os_alloc_aligned_*`, `oh_f01_guarded_malloc_max_is_null`, `oh_f01_huge_alloc_bad_align_is_err` |
| OH-11 | `remote_free` (NEVER / NORMAL / DELAYED arms) | twice-freed block | **written this pass** (the log's fix did not exist): refuse a block that is already the chain head at push time; bound the collect walk by `used` so the interleaved cyclic chain aborts instead of hanging | `tests/double_free.rs` ×4, under default / `secure` / `blockmap` / both |
| OH-12, 22, 25, 27 | arena chunk API | `count`, `p` | `reserve_os_memory_ex` checked mul; `chunk_alloc_n(0)` is `None`; `chunk_free_n` bounds the bitmap; interior `chunk_free` is false | `oh_f04_arena_reserve_huge_is_err`, `oh_f04_chunk_alloc_n_zero_is_none`, `oh_f04_chunk_free_n_zero_and_huge_are_false`, `oh_f05_chunk_free_interior_is_false` |
| OH-13 | thread exit | — | `create_heap` bootstraps the thread's backing heap (and exit hook); exit abandons every heap the dying thread still owns, not only `done_slot` | `set_default_heap_abandoned_double_free_aborts`, `first_class_heap_abandoned_double_free_aborts` |
| OH-14, 16, 26 | FFI `mi_reserve_huge_os_pages*`, `mi_dupenv_s`, `mi_realpath` | `pages`, out-pointers, path | checked `pages * 1 GiB` (ENOMEM); EINVAL clears outs; `PATH_MAX` cap | FFI `oh_f04_reserve_huge_*`, `oh_f01_dupenv_*`, `oh_f05_realpath_*` |
| OH-15, 201, 202 | `manage_os_memory_ex` | `start`, `size` | non-null, non-wrapping, mapped (`prim::range_is_reserved`: `mincore` / `VirtualQuery`), not an already-registered window; `arena_register` CAS + free-on-refuse | `oh_f04_manage_os_memory_null_is_err`, `oh_f05_manage_os_memory_{garbage_base,unmapped,live_window}_is_err`, `oh_f05_manage_os_memory_real_block_still_works` |
| OH-17, 23, 24 | option hooks | hook re-entry | thread-local in-hook flags | `oh_f01_{deferred_free_hook_that_mallocs,error_hook_reentry,output_hook_reentry}_is_contained` |
| OH-18, 19, 20, 21 | `options::get_clamp` / `get_size`, `page_under_utilized`, `subproc_*` | option values, `perc`, `id` | swap inverted bounds; checked KiB; saturating percentage; exhaustion / out-of-range id refused instead of `assert!` | `oh_f01_option_get_clamp_inverted_is_total`, `oh_f04_option_get_size_huge_does_not_wrap`, `oh_f01_page_under_utilized_huge_perc_is_bool`, `oh_f01_subproc_add_oob_is_noop` |
| OH-30 | `segment_map::register_range` | `size` | `walk_windows` stops at unrepresentable addresses and on `checked_add` overflow; range/base tables use checked / saturating ends | reachable only through validated callers now; kept as defense in depth |
| OH-32, 33, 34 | `bin_size`, `prim::fixed::dedicated_segments` / `region_for` | index, `size` | checked; garbage is 0 / unrepresentable | `oh_f04_bin_size_garbage_index_is_total`, `oh_f04_dedicated_segments_max_is_unrepresentable`, `oh_f04_region_for_max_is_zero` |
| OH-38, 60 | `prim::alloc`, `prim::commit` | `align`, null | `Err` before the syscall (commit(null) handed Windows an untracked mapping) | `oh_f01_prim_alloc_bad_align_is_err`, `oh_f01_prim_commit_null_is_err` |
| OH-54, 58, 97..102 | FFI heap handles and out-pointers | `heap`, `out` | `heap_ptr`: null / misaligned / immortal-empty is "no heap"; `mi_heap_set_default` refuses such a handle; `out_mut`: null / misaligned out-pointers are never written (the 64 KiB floor is gone — it would refuse legitimate wasm32 out-pointers) | FFI `oh_f01_heap_visit_blocks_null_is_total`, `oh_f01_ffi_out_param_null_or_misaligned_is_refused`, `oh_f01_ffi_cstr_null_is_refused` |
| OH-62, 65, 66 | `malloc_small` / `zalloc_small`, FFI `mi_free_size` / `mi_free_aligned` | `size` | forward / ignore instead of `debug_assert!` on a public entry | `oh_f01_malloc_small_max_is_null` |

### 4.2 Withdrawn (internal-contract guards)

**OH-35, 36, 37, 40, 41, 46–53, 55–57, 59, 61, 63, 64, 67–200** — 139 IDs,
one probe: an internal `unsafe fn` (`page_of`, `page_index`, `page_area`,
`span_alloc`, `span_free`, `segment_free`, `huge_free`, `purge_free_spans`,
`visit_segment_blocks`, `adopt_segment`, `free_local`, `free_local_at`,
`retire_emptied`, `box_of_xheap`, `ensure_heap`, `heap_of`, `thread_done`,
`heap_delete`, `heap_destroy`, `set_default_heap`, `page_link_local`,
`page_extend`, `block_next`, `remote_free`, `segment_map::register`) handed
null, `0x1`, or `floor + 8`, chased up a ladder of floors (`0x1000`,
`0x10000`, `SEGMENT_SIZE`, 2×, 3×, 4×) and answered with a segment-map
membership lookup on every internal handle (`aligned_mut`) plus a heap
registry walk under a global lock (`live_heap_box`).

Why withdrawn:

1. **They guard contracts the caller inside this crate upholds.** Every one of
   those functions is `unsafe fn` with a stated contract, called only from
   this crate with values it derived itself. A guard there is not boundary
   validation; it is a second copy of the caller's invariant, and R-001
   already accepts that release `free` trusts its pointer's window.
2. **They taxed the hot path, and the campaign never measured it.** At its
   stopping point, release assembly against 2.2.0: `free` **+27**, `realloc`
   **+164**, `page_extend` **+68** instructions; `box_of_xheap` walked the
   heap registry under a global lock on the free path. The log mentions
   instruction counts three times and never once acknowledges these.
3. **The membership gate was circular.** `segment_map::register` was gated
   on `contains(seg)` — a fresh segment cannot be in the map yet — so a
   second `register_fresh` bypassed it; `heap_ptr` at the C ABI used the
   segment map to validate a HeapBox that lives on its own OS page and is
   only in the map by accident of address-space layout. The test that
   "proved" `register_range(0, MAX)` safe passed only because the gate made
   it a no-op; against the real function it paints every window in the map.
4. **The regressions did not pass.** Three of the log's High-severity tests
   failed on its own branch (exit 97: the second free RETURNED) because the
   OH-11 fix they depended on was never written.

What replaced them: nothing on the internal seams; the same inputs are
refused where they enter — `manage_os_memory`, the C ABI, the option table,
the OS wrappers — and the double-free arm that was actually missing (§4.1,
OH-11).

### 4.3 Cost, as landed

x86-64 release, main → branch, Windows / Linux: `free` 62 → 66 / 63 → 67
(two `cmp; je` on the cross-thread arm; the local path is byte-identical and
the function keeps no frame — see `page::remote_double_free` for the three
tries that took); `malloc` 0 / 0; `malloc_aligned_at` 0 / 0; `realloc` +7 /
+6; `realloc_aligned_at` +1 / +1; `page_extend` 0 / 0; `usable_size` 0 / 0;
`remote_free` +4 / +4; `huge_alloc` +29 / −14 (cold). Unsafe census 909 →
928, every site rowed in `crates/rusty_alloc/UNSAFE.md`.

---

## 5. Eligibility recert

- [x] H-01 threat model linked from README
- [x] H-39 SECURITY.md with contact + window
- [x] Every parser/identity/net/store/ai surface has a fuzz or property target — **alloc surfaces do** (`alloc_ops`, `xthread`, `corruption`)
- [ ] Farm ≥ 7 consecutive green days (30 before raising payouts) — soak in progress, completes 2026-09-19
- [x] H-18: no unwrap on inventoried untrusted paths (UPP Completed)
- [x] This file dated within 14 days
- [ ] Named bounty budget (if claiming public)

**Verdict**: **internal-only**

Do not paste `templates/SECURITY-bounty.md` into this repo yet.

---

## Audit log

| Date | SHA | What changed |
|---|---|---|
| 2026-09-15 | 6a5efab | First Openheimer run. Added `tests/openheimer.rs`. **OH-rusty_alloc-5 found and fixed:** `is_aligned_to(x, 0)` was universally true. |
| 2026-09-15 | 6a5efab | Campaign hostile pack lives in openheimer (`probes/rusty_alloc/tests/hostile.rs`). |
| 2026-09-15 | 6a5efab | **OH-rusty_alloc-6 found and fixed:** `malloc_aligned_at` panicked (debug) / would wrap (release) on `offset = usize::MAX`. Checked placement in `bins::aligned_at_from`; oversize block and huge reservation are released on refuse. |
| 2026-09-15 | 6a5efab | **OH-12…16 found and fixed (uncommitted):** arena reserve mul; first-class heaps not abandoned on thread exit; huge-OS-page `pages*1GiB`; `manage_os_memory` null/overflow; `dupenv` EINVAL outs. See openheimer red log. |
| 2026-09-15 | 6a5efab | **OH-17…22 found and fixed (uncommitted):** deferred-free re-entry stack overflow; `get_clamp` inverted; `get_size` KiB wrap; under-utilized perc overflow; subproc id assert; `chunk_alloc_n(0)`. |
| 2026-09-15 | 6a5efab | **OH-23…26 found and fixed (uncommitted):** error/output hook re-entry stack overflow; `chunk_free_n` bitmap OOB; `mi_realpath` PATH_MAX cap. |
| 2026-09-15 | 6a5efab | **OH-27…28 found and fixed (uncommitted):** interior `chunk_free` released a live chunk; `os::alloc_aligned` panicked on garbage align. |
| 2026-09-15 | 6a5efab | **OH-29 found and fixed (uncommitted):** `os::alloc_aligned(MAX)` overflowed `size + alignment` in the prim aligned-reserve dance. |
| 2026-09-15 | 6a5efab | House audit (OH-2 evidence, no new id): `mata-alloc` has no `blockmap` feature; named disco/console binaries ship without `secure`. `region.rs` 1/1 under `ra_small_profile`+`ra_single_threaded`. Arena register CAS + `arena_area` cap. A3: `cargo deny`/`audit` ok; `cargo vet` red on unvetted `portable-atomic 1.15.0`. |
| 2026-09-15 | 6a5efab | **OH-30 found and fixed (uncommitted):** `segment_map::register_range(0, MAX)` hung (release infinite loop / debug overflow). `walk_windows` stops at unrepresentable addresses. |
| 2026-09-15 | 6a5efab | **OH-31 / OH-32 found and fixed (uncommitted):** guarded `malloc(MAX)` overflowed `payload + page`; public `bin_size(MAX)` overflowed `bin + 3`. |
| 2026-09-15 | 6a5efab | **OH-33…37 found and fixed (uncommitted):** `dedicated_segments(MAX)` wrap-to-1; `region_for(MAX)` mul; `page_off_for(MAX)` mul; `span_alloc(MAX)` unbounded mark; `span_alloc(0)` zero-length span. |
| 2026-09-15 | 6a5efab | **OH-38…47 found and fixed (uncommitted):** `prim::alloc` / `huge_alloc` garbage align; `page_area` / `page_of` / `page_index`; in-place realloc `addr+offset` (Rust + extern-C abort); `box_of_xheap(0)`. |
| 2026-09-15 | 6a5efab | **OH-48…57 found and fixed (uncommitted):** empty/`null` heap delete; `heap_malloc(null)`; `huge_free` of Normal; `page_index(null)`; `segment_free` in-use; `set_default_heap(null)`; `span_alloc(null)`; `span_free` uncarved OOB; `thread_done(null)`. |
| 2026-09-15 | 6a5efab | **OH-58…61 found and fixed (uncommitted):** FFI `mi_heap_visit_blocks(null)` abort; public page helpers of null; `prim::commit(null)` allocated an untracked Windows mapping; `visit_segment_blocks(null)` abort. |
| 2026-09-15 | 6a5efab | **OH-62…63 found and fixed (uncommitted):** `malloc_small(MAX)` debug_assert; `page_link_local(null)` abort. |
| 2026-09-15 | 6a5efab | **OH-64…67 found and fixed (uncommitted):** `page_extend(live, null)` AV; FFI `mi_free_size`/`mi_free_aligned` debug_assert abort; `segment_map::register(null)` marked window 0. |
| 2026-09-15 | 6a5efab | **OH-68…69 found and fixed (uncommitted):** `remote_free(DELAYED page, null)` abort; `page_extend(live, lying area)` walked foreign memory. |
| 2026-09-15 | 6a5efab | **OH-70…74 found and fixed (uncommitted):** lying `0x1` handles — `page_index`, segment free/visit/purge, free-list writers, `heap_malloc`, `set_default_heap`. |
| 2026-09-15 | 6a5efab | **OH-75…80 found and fixed (uncommitted):** remaining `0x1` siblings — page helpers, heap teardown, FFI `heap_ptr`, `span_alloc`, close-range `page_of`, `page_area` forwarded lie. |
| 2026-09-15 | 6a5efab | **OH-81…85 found and fixed (uncommitted):** `page_extend(0x1, live area)`; `block_next(0x1)`; `usable_size(0x1)` null-page deref; FFI under-utilized of `0x1`; `ensure_heap(0x1)` forwarded lie. |
| 2026-09-15 | 6a5efab | **OH-86…91 found and fixed (uncommitted):** `realloc`/`heap_realloc`/`reallocf` of `0x1` called `free` on the lie; `free_local(0x1)` AV; `adopt_segment(0x1)` abort; `box_of_xheap(1)` forwarded lie. |
| 2026-09-15 | 6a5efab | **OH-92…94 found and fixed (uncommitted):** aligned first-page lie `0x8` passed `aligned_mut`; `retire_emptied(0x1)`; `free_local_at(0x1)`. |
| 2026-09-15 | 6a5efab | **OH-95…102 found and fixed (uncommitted):** `register(0x8)` / `register_range(8)` painted window 0; FFI first-page out-params (`posix_memalign` / `reallocarr` / `umalloc` / `arena_area` / `process_info` / `reserve_huge_os_pages`). |
| 2026-09-15 | 6a5efab | **OH-103…115 found and fixed (uncommitted):** `MIN_OBJECT_ADDR` 4096 let `0x1000` through; C-string / path / env name of `0x8`; `manage_os_memory_ex(0x8, 2*SEGMENT)` registered a fake arena. |
| 2026-09-15 | 6a5efab | **OH-116…130 found and fixed (uncommitted):** `0x10000` sat on the exclusive 64 KiB floor — typed-metadata AV, window-0 paint, fake arena, forwarded `ensure_heap`. `MIN_META_ADDR = SEGMENT_SIZE`; C-string / out-params stay at 64 KiB. |
| 2026-09-15 | 6a5efab | **OH-131…150 found and fixed (uncommitted):** `SEGMENT_SIZE` sat on the exclusive metadata floor — typed-metadata AV, window-1 paint, fake arena, forwarded `ensure_heap` / `box_of_xheap` / `page_area`. Floor is now inclusive (`addr <= MIN_META_ADDR`). |
| 2026-09-15 | 6a5efab | **OH-151…165 found and fixed (uncommitted):** `SEGMENT_SIZE+8` / `+1` sat on the inclusive 32 MiB floor — typed-metadata AV, window-1 paint, fake arena at `2*SEGMENT`, forwarded lies, `span_free`. `MIN_META_ADDR` is now `2 * SEGMENT_SIZE`. |
| 2026-09-15 | 6a5efab | **OH-166…180 found and fixed (uncommitted):** `2*SEGMENT+8` / `+1` sat on the two-window floor — typed-metadata AV, window-2 paint, fake arena at `3*SEGMENT`, forwarded lies, `span_free`. `MIN_META_ADDR` is now `3 * SEGMENT_SIZE`. |
| 2026-09-15 | 6a5efab | **OH-181…200 found and fixed (uncommitted):** `3*SEGMENT+8` / `+1` sat on the three-window floor — typed-metadata AV, window-3 paint, fake arena at `4*SEGMENT`, forwarded lies, `span_free`, `heap_destroy` / `thread_done` / `heap_zalloc` / `span_alloc` / `huge_free`. `MIN_META_ADDR` is now `4 * SEGMENT_SIZE`. |
| 2026-09-15 | 6a5efab | **Floor class closed (uncommitted):** `aligned_mut` is membership (`segment_map::contains`); HeapBox uses the live-heap registry; public `register`/`register_range` cannot paint a new window. `MIN_META_ADDR` is an alias of `MIN_OBJECT_ADDR` (64 KiB), not a rising window floor. Removed hew/rasp/whet packs and the per-window product copies. Standing regression: `oh_f01_meta_lie_unregistered_is_total`. |
| 2026-09-15 | 6a5efab | **OH-201…202 found and fixed (uncommitted):** `manage_os_memory_ex(1TiB)` registered a fake arena (next default malloc AVs); `manage(live segment)` adopted a named window. Refuse unless `os::range_is_reserved` and `!segment_map::contains(lo)`. Real OS blocks and same-block `MAX_ARENAS` fill (`chase.rs`) still work. |
| 2026-09-16 | `openheimer-real` | **Disposition.** The uncommitted tree was split: §4.1 landed, §4.2 withdrawn (139 IDs: internal-contract guards, `free` +27 / `realloc` +164 / `page_extend` +68 at the campaign's stopping point). `page.rs` reverted whole (its only changes were `aligned_mut` guards on internal machinery). |
| 2026-09-16 | `openheimer-real` | **OH-11 written** — the log's fix did not exist (`git diff \| grep -c xthread_free` = 0) and its three tests failed. `remote_free` head check at push time; collect walk bounded by `used` (the chain a remote double free leaves is CYCLIC; the pre-existing `n > used` check sat after the walk and could not be reached). Fourth regression: interleaved A-B-A. |
| 2026-09-16 | `openheimer-real` | `tests/openheimer.rs` 95 → 37 (every lie-input test went with its guard; `oh_f02_segment_map_huge_range_is_total` went because against the real `register_range` it paints the whole map; `oh_f05_secure_feature_is_not_vacuous` went because it asserted a `cfg!` constant — the vacuous test); FFI pack 16 → 8, two rewritten to null / misaligned inputs after the 64 KiB out-pointer floor was dropped (wasm32 static data starts at 1 KiB). |
| 2026-09-16 | `openheimer-real` | `bins::is_aligned_to`: `is_power_of_two()` emitted as a 20-instruction SWAR popcount without `popcnt` (`realloc_aligned_at` +43); spelled as `align & (align - 1)` (+1). |
