# rust1100-upgrades — getting the whole toolkit ready for Rust 1.100

**Date opened:** 2026-11-12 (the day 1.100.0 shipped) · **Status:** P0, P1, P2
and P5 built the same day on branch `chore/rust-1100` (local, not pushed);
P4 is deferred by design · **Code surveyed:** `origin/main` at `b63cb14`
(2.5.0)

The sections from "Bottom line" down are the plan as surveyed, kept as
written. **"Outcome" below is what building it found**, including where the
plan was wrong.

Rust 1.100.0 stabilises the core of the allocator API: the `Allocator` trait,
and `Vec` and `Box` constructors that take one. This plan covers everything the
release changes for the six crates in this workspace, in the order to do it.

Because our toolchain is pinned to **1.97.1**, moving to 1.100 also crosses
1.98 and 1.99. Those releases are included here; one of them (LLVM 22 → 23)
matters more to our numbers than anything in 1.100 itself.

## Bottom line

1. **Nothing is broken for users today.** Under 1.100.0 the workspace builds,
   clippy is clean on Windows, and the `rusty_alloc` and `rusty_alloc-api`
   suites pass. Users compile us with *their* toolchain, so anyone who
   upgraded today is already running 2.5.0 under 1.100 and LLVM 23.
2. **Our own CI would fail if we bumped the pin as-is.** One new lint fires on
   the Linux `malloc` export, and CI runs clippy with `-D warnings`.
3. **Cargo 1.100's new lints found a real, pre-existing defect.** The
   workspace declares `rust-version = "1.88"` but no crate inherits it, so both
   published crates ship with no minimum Rust version at all.
4. **We can now implement `Allocator` on stable.** A prototype over the
   existing public API (about 80 lines) drives `Vec`, `Box`, a first-class
   `Heap` and `&dyn Allocator` correctly on 1.100. It should ship behind an
   opt-in cargo feature so the minimum Rust version stays 1.88.
5. **Every published instruction count is a 1.97.1 / LLVM 22 number.** The
   bump must be measured as its own step, with no code change beside it.

## Outcome (2026-11-12)

### What landed

| Phase | Commit | What |
|---|---|---|
| P0 | `bf1fd3a` | Every crate inherits `rust-version` and `homepage`; the six libc allocation exports are `unsafe extern "C"`; the division scans find their `.s` under either Cargo layout and exit non-zero when they cannot run |
| P1 | `afdfceb` | Pin 1.97.1 -> 1.100.0 in `rust-toolchain.toml`, `ci.yml`, `release.yml`, with the measurement in `docs/opps.md` ("Toolchain") |
| P2, P3 | `bb87176` | `Allocator` for `RustyAlloc` and `Heap` behind `allocator-api`; the `Heap::alloc` alignment fix; the conformance suite; the no-unwind scan; the `msrv` job and the other CI steps |
| P5 | `38d1222` | The three-day dependency cooldown. (It also denied `unused_dependencies`; that was undone the next day: under miri `libc` is legitimately unused, and the denial stopped `cargo miri test -p rusty_alloc` before it compiled) |
| Follow-up | the commit that added this section | The API crate under miri as its own CI job (`miri-api`) with trimmed sizes; the wasm size baseline; this outcome |

### Where the plan was wrong, and what building it found

1. **"A release build has no panic on these paths" was false.** The plan's
   safety table said to audit `debug_assert!`s and assumed release was clean.
   `tools/no-unwind-scan.sh` reads the machine code: the twelve trait methods
   reach **28 panic sites in 18 allocator functions** (26 bounds checks, a
   remainder by zero, an `expect`). None can fire with the allocator's state
   intact, but the contract does not say "cannot fire". So the impls run every
   method under a guard that aborts on an unwind, in every build, and the count
   is a ratchet (`tools/no-unwind-baseline.txt`).
2. **The direct route is at parity, not below.** `Vec::new_in(RustyAlloc)` is
   1.0 % cheaper than the global route without the guard (2,507.89 against
   2,533.37 instructions per step) and 0.08 % dearer with it (2,535.48). The
   plan's target of "at or below" is met only when the 28 sites are gone.
3. **`Heap::alloc` was wrong on every 32-bit target, in published 2.5.0.**
   Its alignment test was written `<= 8`. On a 32-bit target a one-word block
   is four bytes and four-aligned, so `Heap::alloc` of a 1-byte, 8-aligned
   layout returned a misaligned block. Reproduced on `i686-pc-windows-msvc`
   with the 2.5.0 routing (the conformance suite fails there) and fixed: the
   three tiers are now `GlobalAlloc::alloc`'s. This is a soundness fix for
   wasm32 and riscv32 users of `Heap`, and a reason to release.
4. **CI's miri job never ran the API crate.** `tests/api_miri.rs` existed for
   it and the hardening registry cited it, but the job runs `-p rusty_alloc`
   only. It now runs `-p rusty_alloc-api --features allocator-api` as well.
5. **Declaring the minimum version switched on a lint that enforces it.**
   With `rust-version` inherited, clippy's `incompatible_msrv` checks every
   item against 1.88. It found nothing in the existing code and all 40 uses in
   the new module, which carries `#[clippy::msrv = "1.100"]`.
6. **1.88 is real.** Both published crates build with Rust 1.88.0, hosted,
   wasm32 and bare-metal riscv32. With the feature on, 1.88 stops at
   `use of unstable library feature 'allocator_api'`, which is clear enough.
7. **The toolchain is quieter than feared.** Every fast path compiles to the
   same instructions under LLVM 22 and 23. Three cold functions grew
   (`malloc_generic_once` +3 per call, `malloc_aligned_at_slow` +5,
   `collect_inner` about +102).
8. **Older Cargo warns about `[lints.cargo]`, it does not fail.** "unused
   manifest key" per member when building this workspace on 1.88 or 1.97; it
   is silent for consumers of the published crates.

9. **Destroying a heap could unmap another thread's live blocks** (found
   2026-11-13, older than this branch). Running the conformance suite in a
   release build on Linux, which nothing had done, crashed about one run
   in three, and only when a destroyable heap and a block that outlived
   its thread were in the same process. A destroyable heap adopted the
   dead thread's abandoned segment when it needed a span, and its
   teardown released the segment with the live blocks in it. Fixed in the
   core: a heap that can be destroyed never adopts (`Heap::no_reclaim`,
   as upstream). This affects `mi_heap_destroy` through the C API as well
   as `Heap::new_destroyable`, in every release that has had them.

### Gates

Run on the branch's final tree with Rust 1.100.0, unless a row says otherwise.

| Gate | Result |
|---|---|
| Local CI mirror (WSL, Linux): fmt; clippy `-D warnings` in every CI configuration (all features, each feature alone, shared light heap, small profile, aligned region, 256k segments, `no_std`), and for the Windows and both macOS targets; both riscv32 targets in two geometries; wasm32 | all pass |
| `cargo test --workspace --all-features` on Linux; the API crate's suite on Windows | pass; conformance 38 of 38 on both |
| The conformance suite on a 32-bit target (`i686-pc-windows-msvc`) | 34 of 34; four checks are 64-bit only |
| The same suite with the 2.5.0 `Heap::alloc` routing, 32-bit | fails, as it should: "misaligned for Layout { size: 1, align: 8 }" |
| The no-unwind guard, debug and release, Linux and Windows | aborts in all four |
| Rust 1.88.0: both published crates, hosted, wasm32, bare-metal riscv32 | build |
| `no_std` with `allocator-api`, riscv32 | builds |
| Poison: twelve breakages of the impls | twelve caught, none survived; six kept in `tools/gate-selftest.sh`, all six fire |
| Miri (nightly 2026-11-09), the conformance suite and `tests/api_miri.rs` | 34 of 34 and 5 of 5: no undefined behaviour, no leak. Run twice: at larger sizes (32 minutes on a busy machine) and at the trimmed sizes the `miri-api` job uses (about a quarter of that) |
| Miri on the core crate, full (`cargo miri test -p rusty_alloc`), on the final tree after the heap fast paths and the no-adopt fix | 29 test binaries, 134 passed, 0 failed, no undefined behaviour, no leak; 3 h 26 min on a busy machine (2026-11-13) |
| Unsafe census ratchet | 1,208, baseline and `UNSAFE.md` updated |
| No-unwind ratchet (Linux) | 28 sites, baseline 28; the self-test fires |
| Division scans under 1.97.1 and 1.100.0 | the same counts under both; the self-test fires under both |
| Wasm size ratchet | 21,432 bytes gzipped; re-baselined, +178 of it the compiler |
| Instruction census, 1.97.1 against 1.100.0 | every fast path identical (`docs/opps.md`, "Toolchain") |

Miri said one more thing, about code this branch did not write: an
integer-to-pointer cast in `init::box_of_xheap` (`xheap as *mut HeapBox`), on
the path that frees a block of a first-class heap. It is a warning, not an
error: with that cast in the program miri "might miss pointer bugs" there.
The core's own miri job has the same line in its log.

Not run here, and why:

- **macOS native tests, Kani, the fuzz smoke runs, `cargo deny` / `audit` /
  `vet`, semgrep.** Not installed locally; CI runs them on the pull request.
  No dependency was added, so the supply-chain jobs have nothing new to
  judge.
- **The twelve existing cases of `tools/gate-selftest.sh`.** Their code is
  untouched; the six new cases were run.

### Open after this, in order of value

1. **Release 2.6.0.** It carries a soundness fix for 32-bit `Heap` users. The
   push is the release, so it is the owner's call.
2. **Take the 28 panic sites to zero** (`docs/opps.md`, "The `Allocator`
   route"). Then the guard goes: 27.6 instructions per step back on the direct
   route, and each bounds check removed is its own saving on
   `malloc_generic_once`, `free_local_at` and `collect_inner`. The same sites
   sit behind `GlobalAlloc`, which has the no-unwind rule and no guard.
3. ~~A heap is 6.6 % behind the global handle on the same collection work.~~
   **Closed 2026-11-13:** 2,706.95 -> 2,524.31 instructions per step, which
   is 0.36 % *ahead* of the global handle, with every existing code path
   byte-identical. `Heap` now takes the same inline fast paths
   (`docs/opps.md`, "The `Allocator` route").
4. **The three functions LLVM 23 made longer**, and a re-test of the largest
   LLVM 22 refutations.
5. **Vein 10** (`6fc177d`, branch `perf/vein10-collect`) rewrites
   `collect_inner`: it will change that function's site count and its LLVM 23
   number, so re-run the scan's `--update` when the two branches meet.
6. P4 as written: nothing in it is needed.

## What was verified today, and what was not

Rust 1.100.0 was installed side by side (the pinned default is untouched) and
run against the `perf/vein10-collect` worktree, which is `b63cb14` plus the
uncommitted vein 10 change.

| Check under 1.100.0 | Result |
|---|---|
| `clippy --workspace --all-targets --all-features` (Windows host) | clean in our code; 2 Cargo manifest warnings (see P0) |
| `clippy --workspace --all-features --target x86_64-unknown-linux-gnu` | **1 warning**: `suspicious_runtime_symbol_definitions` on `malloc` |
| `cargo test -p rusty_alloc -p rusty_alloc-api` (debug, Windows) | all pass |
| `build -p rusty_alloc --no-default-features --cfg ra_single_threaded`, `riscv32imac-unknown-none-elf` | builds |
| `check -p rusty_alloc-wasm --target wasm32-unknown-unknown` | builds |
| `rustc --release -- --emit asm` output location | moved: `release/build/<pkg>/<hash>/out/*.s`; `release/deps/` no longer exists |
| Final libraries (`release/rusty_alloc_override.dll`, `.rlib`) | same place as before |
| `Allocator` prototype (3 tests) | pass on 1.100.0; 31 × `E0658` on 1.97.1, as expected |
| crates.io metadata for `rusty_alloc` and `rusty_alloc-api` 2.5.0 | `rust_version` is empty |

**Not run under 1.100:** the Linux and macOS test suites, release-mode tests,
miri, loom, the Kani proofs, the fuzz targets, `cargo deny` / `cargo vet`, the
wasm size baseline, and every instruction-count measurement. Each is a gate in
P1 below. The WSL machine was busy with the vein 10 miri run.

## What changed, release by release, that touches us

| Release | Change | Effect here | Phase |
|---|---|---|---|
| 1.100 | `Allocator`, `AllocError`, `Global`, `System`; `Vec`/`Box` `*_in` constructors stable | We can offer `Allocator` on stable | P2 |
| 1.100 | Cargo lints (`[lints.cargo]`), including `unused_workspace_package_fields` | Exposed the missing minimum version | P0 |
| 1.100 | Cargo's new build-directory layout | `tools/div-scan-*.sh` look in `release/deps`, which is gone | P0 |
| 1.100 | "Rust" calling convention on x86 uses SSE registers | Possible instruction-count drift; we pass integers and pointers, so expected small. To be measured | P1 |
| 1.100 | Never type `!` stable; fallback is `!` on all editions | We are on edition 2024, which already had this. No change | none |
| 1.100 | `Infallible` is an alias of `!`; `box` patterns removed | Neither appears in our source | none |
| 1.100 | `DropGuard`, `conjure_zst`, `UnsafeCell::as_mut_unchecked`, `bool::toggle` | Usable only after the minimum version rises | P4 |
| 1.100 | Cargo dependency cooldown (`global-min-publish-age`) | Supply-chain hardening for CI and dev | P5 |
| 1.100 | `-Cprofile-sample-use` stable | Optional lever for the shipped `cdylib` only | P4 |
| 1.99 | **LLVM 22 → 23** | Every hand-shaped hot path may compile differently | P1 |
| 1.99 | POSIX names added to the runtime-symbol lints | Source of the `malloc` warning | P0 |
| 1.99 | Documented: allocations may grow in place, never shrink | Matches what our `realloc` already assumes; re-read the wording | P2 |
| 1.99 | `Box::into_non_null`, `Vec::into_parts`, `Layout::for_value_raw` | Convenience only | none |
| 1.99.0 | Miscompiled FFI `bool` returns on x86-64, fixed in 1.99.1 | We skip 1.99.0 entirely. Never pin to it | P1 |
| 1.98 | `invalid_runtime_symbol_definitions` (error by default), `c_void_returns` (warning) | Neither fires on our code | none |
| 1.98.0 | Vtable miscompilation, fixed in 1.98.1 | Skipped | none |

## P0 — fix what 1.100 already flags (safe on the current pin)

None of these needs the toolchain bump, and all of them should land before it.

**P0.1 Declare the minimum Rust version for real.** `Cargo.toml:141` sets
`rust-version = "1.88"` under `[workspace.package]`, but no member has
`rust-version.workspace = true`. Add it to all six crates.

- Then prove it: add a CI job that builds `rusty_alloc` and `rusty_alloc-api`
  on 1.88. There is no such job today, so 1.88 is a claim nobody has tested.
  If it fails, raise the number to what actually builds and say so in the
  changelog.
- Same warning for `homepage`: inherit it in the published crates or drop it.

**P0.2 Make `malloc` match the signature the compiler expects.**
`crates/rusty_alloc_override/src/lib.rs:20` exports `pub extern "C" fn malloc`;
1.100 expects `unsafe extern "C" fn(usize) -> *mut c_void`. Change the
signature rather than allowing the lint.

- Only `malloc` is flagged today, but `calloc`, `memalign`, `aligned_alloc`,
  `valloc` and `pvalloc` have the same shape. Change them together so the next
  release's lint does not catch the rest.
- Gate: the calling convention is unchanged, so the override library's
  disassembly and all opscan numbers must be identical before and after.

**P0.3 Repair the division scans.** `tools/div-scan-targets.sh` and
`tools/div-scan-selftest.sh` search `…/release/deps` for the `.s` file. Under
1.100 it is at `release/build/<pkg>/<hash>/out/`.

- The scan prints "no .s emitted" rather than a false zero, so it fails loudly.
  It still stops protecting the zero-divisions claim until fixed.
- Search the whole target directory for the file, or take the path from
  Cargo's JSON messages, which is the interface Cargo now recommends.
- Keep the self-test's rule: no file found is a failure, never a pass.

**P0.4 Correct the README.** `crates/rusty_alloc_api/README.md:23` says the
crate offers "`Allocator` — the unstable `allocator_api` trait, behind a
feature". No such impl or feature exists in `src/`. Either remove the line now
or land it with P2 as a true statement about the stable trait.

## P1 — move the pin from 1.97.1 to 1.100.0

The pin lives in `rust-toolchain.toml` and in the `toolchain: 1.97.1` lines of
`ci.yml` and `release.yml`. `release-plz.yml` uses floating `stable`, so that
one job is already on 1.100.

**Measure the bump alone.** This is the part that needs care.

1. On the WSL box, build the *same commit* with 1.97.1 and with 1.100.0.
2. Run the full census on both: the ten opscan ops, the allocator-only and
   whole-program counts for perl, sqlite, lua and python, and the four Rust
   workloads.
3. Record the result in `docs/opps.md` as a **toolchain** row. It is not a win
   or a loss attributed to our code.
4. From then on, every paired comparison uses one toolchain for both arms.

What to expect, stated as hypotheses to test:

- The LLVM 23 change is the larger risk. The free path's `asm!` goto blocks,
  the split-body inlining arrangements and the cold-arm outlining were all
  tuned against LLVM 22's inlining decisions. Any of them can flip.
- The x86 calling-convention change affects Rust-to-Rust calls that pass
  floats or vectors. The allocator passes integers and pointers, so the direct
  effect should be near zero.
- After the bump, re-test the three largest recorded refutations in
  `docs/opps.md`. A layout that lost on LLVM 22 may win on 23, and the reverse.

If a hot path regresses, hold the pin and find the cause. The pin is ours, and
users' builds are unaffected by when we move it.

**Gate battery for the bump** (all on 1.100.0, plus nightly where noted):

- the local CI mirror: every clippy configuration, cross targets, both
  `riscv32` targets, `wasm32`;
- full miri (nightly), loom, the Kani proofs, a fuzz build;
- `cargo deny check`, `cargo vet`, the semgrep rules, the unsafe census;
- `tools/wasm-size.sh` against its baseline, which will move with LLVM 23 and
  needs a deliberate re-baseline;
- Linux, macOS and Windows test suites in debug and release.

**After the bump:** update the README's performance section and the toolchain
it names, since the published ratios against mimalloc were taken on 1.97.1.

## P2 — implement `Allocator` in `rusty_alloc-api`

### What was stabilised

```rust
pub unsafe trait Allocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError>;
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout);
    // provided: allocate_zeroed, grow, grow_zeroed, shrink
}
```

- Stable: the trait, `AllocError`, `Global`, `System`, forwarding impls for
  `&A`, `&mut A`, `Box<A>`, `Rc<A>`, `Arc<A>`, and the `Vec` / `Box`
  constructors (`new_in`, `with_capacity_in`, `from_raw_parts_in`, …).
- The trait is dyn-compatible: `&dyn Allocator` works.
- **Still unstable** (feature `allocator_ext`, tracking issue 163177):
  `GlobalAllocator`, `StaticAllocator`, `AllocatorClone`, and `Allocator for
  Pin<P>`. One consequence: `Box::into_pin` is not available with a custom
  allocator on stable.
- Only `Vec` and `Box` take an allocator so far. `String`, `HashMap`,
  `VecDeque` and `Rc::new_in` are not in this release.
- Nothing in the release notes or the stabilisation report changes
  `#[global_allocator]`. It still takes `GlobalAlloc`, so that impl stays our
  primary surface.

### What to build

1. `unsafe impl Allocator for RustyAlloc`: the zero-sized global handle.
2. `unsafe impl Allocator for Heap`. Users write `Vec::new_in(&heap)`; the
   standard `&A` impl carries the borrow, so no block can outlive the heap in
   safe code. This is the "lifetime-carrying `Allocator` impl" that the comment
   at `crates/rusty_alloc_api/src/lib.rs:44` promised for destroyable heaps.
3. A `Heap` realloc path. `Heap` exposes no realloc today, so `grow` and
   `shrink` would fall back to allocate-copy-free. The core already has
   `alloc::heap_realloc` (`alloc.rs:2784`); wire it in.

### Gating

| Option | Verdict |
|---|---|
| Cargo feature `allocator-api`, off by default; the feature needs 1.100, the crate stays at 1.88 | **Recommended for 2.6.0** |
| Auto-detect the compiler in a build script | Rejected: adds a build script to an allocator's dependency graph |
| Raise the minimum version to 1.100 and make it unconditional | Later, at a major version or when adoption supports it |
| Also implement the `allocator-api2` polyfill trait for older compilers | Not now: a new dependency to audit, for a trait the standard one replaces |

### The safety contract, clause by clause

The stabilised trait's rules are stricter than the old nightly ones. Each has
to be argued for our impls, not assumed.

| Rule | Our position | Work |
|---|---|---|
| Zero-size layouts must be accepted | `GlobalAlloc` forbids them, so the existing paths never see one | Return an aligned dangling pointer of length 0 without calling the allocator; `deallocate` ignores size 0 |
| No method may unwind | Corruption paths abort. A `debug_assert!` can panic in a debug build | Audit every assertion reachable from the six methods; confirm how the `GlobalAlloc` impl already meets the same rule |
| Dropping the allocator must not unwind | `Heap::drop` runs delete or destroy | Confirm neither can panic |
| A live block stays valid until invalidated | Holds. Dropping a `Heap` invalidates its blocks, which the contract allows | Document it; do **not** claim `StaticAllocator` for `Heap` |
| The allocator must not read or write live user memory | Free-list links and debug fills live only in freed blocks | Re-check `secure` and `debug_checks` builds |
| `grow` / `shrink` may change the alignment | `GlobalAlloc::realloc` never does | Use realloc when the alignment is unchanged, allocate-copy-free otherwise |
| `grow_zeroed` must zero the new tail | New requirement for us | Zero from `old.size()` to `new.size()` after the realloc |
| A failed `grow` / `shrink` leaves the old block valid | Our realloc returns null without freeing | Add a test that forces the failure |
| Equivalence between allocator values | All `RustyAlloc` are equivalent; `&Heap` is equivalent to its `Heap` | Do not declare `Heap` equivalent to `RustyAlloc`, even though `free` is heap-agnostic. It keeps the destroyable case sound |
| Thread safety | `Heap` holds a raw pointer, so it is neither `Send` nor `Sync`; `Vec<T, &Heap>` is therefore not `Send` | Lock this in with a `compile_fail` doctest |

### Two design decisions already settled by the evidence

- **Return the requested size, not the usable size.** The trait lets
  `allocate` report a larger block. `Vec`'s buffer code in 1.100 discards the
  extra (`library/alloc/src/raw_vec/mod.rs`, with a comment that it "should
  change"). Reporting it today costs a size lookup per allocation and buys
  nothing. Revisit when `Vec` starts using it: our size classes always have
  slack, so that would save real reallocations.
- **Keep `GlobalAlloc` as the fast path we tune.** Almost every user reaches
  us through `#[global_allocator]`.

### Tests and measurement

- Unit tests, starting from the three in the prototype: `Vec` growth to
  100,000 elements and back, 64-byte-aligned types, zero-sized types,
  `grow_zeroed` across an alignment change, collections in a destroyable heap,
  `&dyn Allocator`.
- Add the same cases to `tests/api_miri.rs`.
- Poison each special case (zero-size, the zeroed tail, the alignment-change
  arm) and confirm a test fails. A case no test can break is a coverage gap.
- New census arm: `Vec<T, RustyAlloc>` push/grow against `Vec<T>` under
  `#[global_allocator]`. The direct path skips the `__rust_alloc` shims and
  sees the layout as a constant, so the target is at or below the global path
  per operation. Measure it; do not assume it.

### Release

Additive, so **2.6.0**. Pushing a version bump to `main` publishes to
crates.io through release-plz, so the full pre-push gate set applies.

## P3 — the other crates

| Crate | Change |
|---|---|
| `rusty_alloc` (core) | None required. Confirm the bump in P1 |
| `rusty_alloc-api` | P0.1, P0.4, P2 |
| `rusty_alloc-ffi` | None found. Re-run clippy on Linux and macOS under 1.100 |
| `rusty_alloc-override` | P0.2 |
| `rusty_alloc-wasm` | Re-baseline the size after LLVM 23. WASI SDK 33 → 34 does not affect `wasm32-unknown-unknown` |
| `rusty_alloc-bench`, `bench/rust-globalalloc`, `fuzz` | Rebuild on 1.100; add the `Allocator` census arm from P2 |
| `tools/`, `bench/*.sh` | P0.3. The `bench` scripts read only final libraries, which have not moved |

## P4 — optional, after the required work

- **New library items inside the allocator.** `DropGuard`, `conjure_zst`,
  `UnsafeCell::as_mut_unchecked` and `bool::toggle` need 1.100, so they wait
  until the minimum version rises. None removes an instruction on its own;
  adopt one only where a paired measurement or a safety argument supports it.
- **Sample-based profile-guided builds** (`-Cprofile-sample-use`). Applies only
  to the `cdylib` we link ourselves; `rlib` users compile us with their own
  flags. Treat it as a per-workload hypothesis and test it against each one.
- **Unstable marker traits.** `RustyAlloc` meets the requirements of
  `StaticAllocator` and `GlobalAllocator`. Implement them when they stabilise;
  watch tracking issue 163177.

## P5 — supply chain

- **Dependency cooldown.** Add to `.cargo/config.toml` once the pin is 1.100:

  ```toml
  [registry]
  global-min-publish-age = "3 days"
  ```

  Versions already in `Cargo.lock` are unaffected. `cargo install` ignores the
  setting, so tool installs in CI still need exact versions.
- **`[lints.cargo]`.** Promote `unused_dependencies` in the workspace lint
  table. First confirm that Cargo 1.88 accepts a manifest containing that
  table, since consumers on older toolchains read our published manifests.
- **`cargo install --locked`** is now the default behaviour. Check CI's tool
  installs still resolve the versions they did.

## Order of work

1. P0 (four small fixes, on 1.97.1), with the normal pre-push gates.
2. P1 census on both toolchains, then the pin bump as its own commit.
3. P2 behind the `allocator-api` feature, released as 2.6.0.
4. P5, then P4 as opportunities arise.

## Decisions that are the owner's

1. **Minimum Rust version policy.** Keep 1.88 with an opt-in feature
   (recommended), or move to 1.100 sooner.
2. **`allocator-api2` support.** Recommended no; say if downstream users on
   older compilers need it.
3. **When 2.6.0 ships**, since the push is the release.

## How quickly this will be adopted

An estimate, with the evidence it rests on.

- **The compiler: fast.** Application developers tend to move to a new stable
  within weeks. Our users on 1.100 are compiling 2.5.0 with it already, which
  is why P0 and P1 come first.
- **Libraries that expose `A: Allocator`: slow by design.** Widely used crates
  hold their minimum version six to twelve months back, and distribution
  toolchains lag further. Expect feature-gated support within one to three
  months in the crates that already had it on nightly, and default-on support
  during 2027.
- **Everyday use: limited until more collections arrive.** Only `Vec` and
  `Box` accept an allocator. Arena-heavy code (compilers, parsers, games,
  databases, embedded) will use it first.
- **Demand is real.** On crates.io, over the last 90 days: `allocator-api2`
  (the stable polyfill of this trait) 180 million downloads, `bumpalo` 201
  million, `hashbrown` 894 million. For scale: `mimalloc` 26 million,
  `tikv-jemallocator` 27 million, `rusty_alloc` 52 thousand.
- **For us:** most users take `#[global_allocator]`, which has not changed, so
  the `Allocator` impl will not move adoption by itself in the near term. It
  is cheap, it makes first-class heaps usable with standard collections under
  a borrow the compiler checks, and being correct on the stable trait early is
  part of being the best unified allocator.

## Sources

- Release notes, 1.98.0 – 1.100.0: <https://github.com/rust-lang/rust/blob/stable/RELEASES.md>
- Announcement: <https://blog.rust-lang.org/2026/11/12/Rust-1.100.0/>
- Stabilisation report, "alloc: stabilise `Allocator`": <https://github.com/rust-lang/rust/pull/156882>
- Box impls generalised over allocators: <https://github.com/rust-lang/rust/pull/161946>
- x86 "Rust" ABI uses SSE registers: <https://github.com/rust-lang/rust/pull/161583>
- Trait source at the release tag: `library/core/src/alloc/mod.rs`, tag `1.100.0`
- Remaining unstable surface: <https://github.com/rust-lang/rust/issues/163177>
