# `unsafe` inventory — rusty_alloc

Every `unsafe` block in the workspace carries a `// SAFETY:` invariant, and
that is **lint-enforced**, not conventional: `clippy::undocumented_unsafe_blocks
= deny` and `unsafe_op_in_unsafe_fn = deny` are workspace-level, and clippy
runs `-D warnings` on every PR. This file is the module-level map: what each
module's `unsafe` is *for*, and when it was last deliberately audited.

Census basis: `unsafe` occurrences in CODE per file (comments stripped;
`unsafe fn` signatures and the four `unsafe impl`s included).
**The baseline is now MACHINE-ENFORCED** (H-11): `tools/unsafe-census.sh`
counts `unsafe` in code (comments stripped) per file against
`tools/unsafe-baseline.txt` — **808 occurrences across 21 files, 2026-08-19**
— and FAILS if the total grows. Growth is not forbidden, it is required to be
deliberate: add the new sites here with their purpose and audit date, then
re-run with `--update` in the same commit. `cargo geiger`, the registry's
nominal probe, does not compile on the pinned toolchain in any version tried;
this is the recorded substitution, and a better fit besides — geiger measures
unsafe in DEPENDENCIES, and ours are `libc` plus bindings-only `windows-sys`.

| Module | Count | What the `unsafe` is for | Last audit |
|---|---:|---|---|
| `alloc.rs` | 82 | The public entry points: raw-pointer reads on the malloc fast path (must never form `&mut` on the shared empty-heap sentinel), pointer-derived metadata on the free path (`segment_of`/`page_of`), block-content copies in the realloc family | 2026-08-19 (fast-path campaign; every block re-reviewed) |
| `heap.rs` | 52 | Owner-thread page/queue manipulation under raw pointers (no two `&mut Page` may coexist), the aligned-allocation peek, span carving. **+3 on 2026-08-19**: `try_unlink_huge_segment` split out of `remove_huge_segment` so the unlink DECISION is testable without the diagnostic assert firing (`unlink_tests`); the split adds an `unsafe fn` signature plus its call and block, and dereferences nothing the original did not | 2026-08-19 (huge-path unlink fix + regression test) |
| `init.rs` | 36 | Thread/heap lifecycle: the initial-exec TLS slot (`global_asm!` + fs-relative asm reads), thread-pointer register reads (`fs:0`/`gs:0x30`/`tpidrro_el0`), heap-box creation/teardown, the abandonment path run inside platform TLS destructors | 2026-08-19 (`.tdata` sentinel redesign) |
| `segment.rs` | 35 | Segment/page metadata addressing: the mask trick (`segment_of`), `page_of`'s contract-based indexing — **the bound is now PROVED for every in-segment offset by `proofs.rs` (Kani), not merely asserted** — span tiling, purge/recommit | 2026-08-19 |
| `prim/windows.rs` | 30 | OS FFI: VirtualAlloc family, FLS destructors, QPC, BCryptGenRandom | 2026-08-08 (0.4.0) |
| `page.rs` | 29 | Free-list links written into dead blocks, the lock-free `xthread_free` four-state protocol (loom-modeled in `tests/loom_xthread.rs`), the immortal `EMPTY_PAGE` sentinel (`unsafe impl Sync`, justified at the impl) | 2026-08-19 (push-local return-value change) |
| `prim/unix.rs` | 27 | OS FFI: mmap family, madvise/decommit, pthread keys, /dev/urandom | 2026-08-08 (0.4.0 Darwin decommit fix) |
| `prim/mod.rs` | 18 | The `TlsSlot` abstraction (`unsafe impl Send/Sync`, justified at the impls), dispatch to the platform backends | 2026-08-08 |
| `rusty_alloc_api/src/lib.rs` | 14 | The `GlobalAlloc`/`Allocator` impls forwarding layouts to the core; `unsafe` is inherent to those traits' contracts | 2026-08-19 |
| `os.rs` | 12 | The prim-layer wrapper: commit/decommit/protect plumbing | 2026-08-08 |
| `prim/mock.rs` | 8 | Miri-only mock OS backend (never shipped; `cfg(miri)`) | 2026-08-06 |
| `arena.rs` | 8 | Lock-free chunk bitmap claim/verify, recycled-chunk scrubbing (the 0.1.0-alpha.2 UAF fix lives here: `wait_no_remote_in_flight` on every recycle path) | 2026-08-08 |
| `prim/wasm.rs` | 6 | `memory.grow` linear-memory backend | 2026-08-06 |
| `options.rs` | 6 | Env parsing at init, registered-hook invocation | 2026-08-06 |
| `stats.rs` | 3 | Volatile whole-struct snapshot of racy-by-design counters | 2026-08-06 |
| `random.rs` | 2 | OS entropy seeding via the prim layer | 2026-08-08 |
| `types.rs`, `bins.rs`, `segment_map.rs`, `lib.rs` | 0 | safe | — |

## The four `unsafe impl`s (H-21)

| Impl | Where | Justification |
|---|---|---|
| `Sync for EmptyPage` | `page.rs` | never written: `page_pop` returns before its first store when `free` is null, and the sentinel's `free` is permanently null |
| `Sync for EmptyHeapBox` | `init.rs` | never written, never remote-reachable (no page ever stores its delayed-list address; owner_tid 0 matches no thread); readers are raw-pointer reads only |
| `Send for TlsSlot` / `Sync for TlsSlot` | `prim/mod.rs` | the slot is an OS TLS key handle; per-thread storage is accessed only by its owner |

## Rules of engagement

- New `unsafe` requires: the `SAFETY:` comment (lint-enforced), a row update
  here, and Miri coverage or an explicit note that the path is
  hardware-gated (the TLS asm reads are `cfg(not(miri))` — their gates are
  `bench/churn.sh` and the corpus sweeps, never Miri alone).
- The `#[must_use]` discipline on lifecycle returns (`adopt_segment`,
  `retire_span`, `remove_segment`, `remove_huge_segment`, `page_push_local`)
  exists because ignoring those returns was the 0.4.0 use-after-free family.
  Do not `let _ =` one without a comment saying why it is terminal — and a
  semgrep rule (`tools/semgrep-rules.yml`) now enforces that in CI. It earned
  its keep on its first run: `remove_huge_segment` still had the old unsound
  shape (a `debug_assert!(false, …)` that vanishes in release, with the caller
  freeing the segment anyway) months after the normal-segment path was fixed.
