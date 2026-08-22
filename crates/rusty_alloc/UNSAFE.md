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
`tools/unsafe-baseline.txt` — **853 occurrences across 21 files, 2026-08-22**
— and FAILS if the total grows. Growth is not forbidden, it is required to be
deliberate: add the new sites here with their purpose and audit date, then
re-run with `--update` in the same commit. `cargo geiger`, the registry's
nominal probe, does not compile on the pinned toolchain in any version tried;
this is the recorded substitution, and a better fit besides — geiger measures
unsafe in DEPENDENCIES, and ours are `libc` plus bindings-only `windows-sys`.

| Module | Count | What the `unsafe` is for | Last audit |
|---|---:|---|---|
| `alloc.rs` | 94 | The public entry points: raw-pointer reads on the malloc fast path (must never form `&mut` on the shared empty-heap sentinel), pointer-derived metadata on the free path (`segment_of`/`page_of`), block-content copies in the realloc family. **+15 on 2026-08-22**: the free path's two `asm!` sites — the memory-destination `used--` whose flags drive the retire branch (its `label` block is a separate item and carries its own `unsafe`), and the fused `cmp {tid}, fs:0` in both `free_inline` and `free_general` — plus `malloc_or`/`malloc_or_slow`, which give `operator new` a fast path whose miss is a tail call. Each asm reads or writes exactly one field it already had a valid pointer to; none widens what the surrounding code could already touch | 2026-08-22 (free campaign; every new block reviewed at the site) |
| `heap.rs` | 67 | Owner-thread page/queue manipulation under raw pointers (no two `&mut Page` may coexist), the aligned-allocation peek, span carving. **+3 on 2026-08-19**: `try_unlink_huge_segment` split out of `remove_huge_segment`. **+15 on 2026-08-22**: `malloc_generic` split into a small entry plus `malloc_generic_walk`, with `grow_front`, `try_guarded` and `drain_delayed` as cold arms — each split adds an `unsafe fn` signature and its block while dereferencing nothing the single function did not — and the immortal `EMPTY_DELAYED` sentinel that lets the heartbeat read its list without a null test | 2026-08-22 (slow-path and free campaigns) |
| `init.rs` | 36 | Thread/heap lifecycle: the initial-exec TLS slot (`global_asm!` + fs-relative asm reads), thread-pointer register reads (`fs:0`/`gs:0x30`/`tpidrro_el0`), heap-box creation/teardown, the abandonment path run inside platform TLS destructors | 2026-08-19 (`.tdata` sentinel redesign) |
| `segment.rs` | 35 | Segment/page metadata addressing: the mask trick (`segment_of`), `page_of`'s contract-based indexing — **the bound is now PROVED for every in-segment offset by `proofs.rs` (Kani), not merely asserted** — span tiling, purge/recommit | 2026-08-19 |
| `prim/windows.rs` | 30 | OS FFI: VirtualAlloc family, FLS destructors, QPC, BCryptGenRandom | 2026-08-08 (0.4.0) |
| `page.rs` | 37 | Free-list links written into dead blocks, the lock-free `xthread_free` four-state protocol (loom-modeled in `tests/loom_xthread.rs`), the immortal `EMPTY_PAGE` sentinel. **+8 on 2026-08-22**: `page_link_local` split out of `page_push_local` so the caller can decrement `used` in one memory-destination RMW, `page_collect_impl` const-generic over whether it also writes the protocol flag, and `USED_OFFSET` — whose value is asserted against `offset_of!(Page, used)` by a unit test, because an asm operand is not type-checked and a field reordering would silently decrement the wrong bytes | 2026-08-22 (free campaign) |
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

### The `publish = false` crates

Not published, but not unaudited either — they were covered by nobody until
2026-08-20, when the workspace-root plan's "audited per member" claim turned
out to be true of only 2 of 6 members. All four inherit `[lints] workspace =
true`, so every `unsafe` block in them already carries a lint-enforced SAFETY
comment, and all four are in the H-11 ratchet's baseline.

| Module | Count | What the `unsafe` is for | Last audit |
|---|---:|---|---|
| `rusty_alloc_ffi/src/lib.rs` | 350 | **The workspace's untrusted boundary**: 157 `extern "C"` entry points receiving caller pointers and sizes. Every out-parameter writer null-guards; `mi_posix_memalign` validates alignment (power of two, ≥ `sizeof(void*)`) and returns EINVAL/ENOMEM; all 8 `count × size` sites use `checked_mul`. Panics cannot unwind into C (edition 2024 aborts on `extern "C"` unwind; release is `panic = "abort"`). **+2 on 2026-08-22**: `new_impl` routes through `alloc::malloc_or` so the OOM arm is a tail call rather than a null test that keeps `size` live | 2026-08-22 |
| `rusty_alloc_override/src/lib.rs` | 51 | `malloc`/`free`/`operator new` interposition for `LD_PRELOAD` — thin forwarding to `alloc::*`, plus the `free_inline` export that carries the fast-path body. No state of its own. **+2 on 2026-08-22**: the sized-delete exports follow the same `free_inline` shape as the unsized ones | 2026-08-22 |
| `rusty_alloc_bench/src/*.rs` | 29 | Tier-B kernels and the `.ratrace` replayer. The trace parser's `unwrap`s are infallible by TYPE (`Record::decode` takes `&[u8; RECORD_SIZE]`), and the one genuinely-invalid field returns `InvalidData` | 2026-08-20 (first audit) |
| `rusty_alloc_wasm/src/lib.rs` | 21 | The `ra_selftest` cdylib fixture: raw block writes and read-back checks that prove the wasm build actually allocates | 2026-08-20 (first audit) |

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
