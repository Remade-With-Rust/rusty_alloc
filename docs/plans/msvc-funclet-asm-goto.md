# 2.2.1–2.2.4 do not build in an unwinding MSVC consumer with LTO

**Status:** fixed differently from §3, on branch `fix/msvc-funclet-asm-goto`, for 2.2.5. See §8.
**Date:** 2026-10-07 · **Affects:** rusty_alloc-api 2.2.1, 2.2.2, 2.2.3, 2.2.4 ·
**Platform:** `x86_64-pc-windows-msvc`, consumer built with `panic = "unwind"` (the default),
`lto = "thin"`, `codegen-units = 1` ·
**From:** the rusty_sloth session (rusty_sloth's `crates/rusty_sloth-alloc` seam, upgrading 2.2.0
to 2.2.4).

---

## 1. What happened

rusty_sloth bumped `rusty_alloc-api` 2.2.0 → 2.2.4 (`cargo add`, nothing else changed) and its
release binary stopped building:

```
   Compiling rusty_alloc v2.2.4
   Compiling rusty_alloc-api v2.2.4
   Compiling rusty_sloth-alloc v0.0.0
   Compiling rusty_sloth-cli v0.0.0
Bogus funclet pad use
  callbr void asm sideeffect inteldialect "sub dword ptr [${0:q} + 64], 1\0Ajle ${1:l}", "r,!i,~{dirflag},~{fpsr},~{flags},~{memory}"(ptr %2698) #45 [ "funclet"(token %1453) ]
          to label %2724 [label %2710], !dbg !55279, !noalias !50705
  (three more, the same instruction in other functions)
rustc-LLVM ERROR: Broken module found, compilation aborted!
error: could not compile `rusty_sloth-cli` (bin "sloth")
```

The consumer's release profile is `opt-level = 3`, `lto = "thin"`, `codegen-units = 1`, and the
default `panic = "unwind"`. 2.2.0 builds in the same profile.

### Reproduce (re-run 2026-10-07 from a clean rusty_sloth `main`, no patch)

The consumer's `Cargo.lock` pins **2.2.0**. Cargo keeps a locked version until it is told to
move. Without step 2 the build compiles 2.2.0 from crates.io and tests nothing; a `[patch]`
entry is likewise left unused (cargo only warns "Patch … was not used in the crate graph").

```sh
cd rusty_sloth                                    # F:/coding/rusty_sloth
cargo add rusty_alloc-api@2.2.4 -p rusty_sloth-alloc
cargo update -p rusty_alloc-api -p rusty_alloc    # REQUIRED: moves the lock off 2.2.0
cargo tree -i rusty_alloc-api -e normal           # must print "rusty_alloc-api v2.2.4"
cargo build --release -p rusty_sloth-cli --features cuda
```

**Expected:** `Compiling rusty_alloc v2.2.4`, then four "Bogus funclet pad use" and `rustc-LLVM
ERROR: Broken module found`. Reproduced with the published 2.2.4 (`source =
"registry+…crates.io-index"` in the lock). The LTO link of the `sloth` binary is where it fails;
building only the library crates passes.

**To test a rusty_alloc tree instead** (the fix, or `main`), put the two entries in rusty_sloth's
**existing** `[patch.crates-io]` table. It already holds `serde_json` and `tokenizers`, and a
second `[patch.crates-io]` header is a duplicate-key error:

```toml
rusty_alloc = { path = "<tree>/crates/rusty_alloc" }
rusty_alloc-api = { path = "<tree>/crates/rusty_alloc_api" }
```

Then the same `cargo add` (the requirement must admit the tree's version), `cargo update -p
rusty_alloc-api -p rusty_alloc`, and check that the lock's two entries have **no `source =`
line** (a path dependency) before building. Restore rusty_sloth afterwards (`git checkout
Cargo.toml Cargo.lock crates/rusty_sloth-alloc/Cargo.toml`).

## 2. Mechanism

- `free_inline` (`crates/rusty_alloc/src/alloc.rs`, `#[inline(always)]`) has the free fast path
  as an `asm!` goto: `sub dword ptr [{pg} + {off}], 1` / `jle {cold}` with `cold = label { … }`.
  LLVM emits that as `callbr`.
- **Since 2.2.1** (`fdda396`, "a Rust GlobalAlloc fast path"), `GlobalAlloc::dealloc` in
  `crates/rusty_alloc_api/src/lib.rs` calls `free_inline` directly. Through 2.2.0 it called
  `free`, a separate function. Now the `callbr` inlines into every Rust deallocation.
- That includes the `drop`s a panic runs. On MSVC targets those are EH **cleanup funclets**, and
  every call inside one carries a `"funclet"` operand bundle. LLVM cannot keep a `callbr` in a
  funclet: its successor blocks lose the funclet colouring. The verifier rejects the module
  ("Bogus funclet pad use").
- **Why nothing in rusty_alloc's own CI caught it.** The workspace release profile is
  `panic = "abort"` (`Cargo.toml`), so no cleanup funclets exist. The Windows CI job builds with
  that profile, and the cross `cargo check --target x86_64-pc-windows-msvc` job does no codegen.
  Only a consumer that unwinds, on MSVC, with enough inlining to pull `dealloc` into a cleanup
  pad, hits it.
- **Other targets.** Itanium EH (Linux, macOS) uses landing pads, not funclets. `callbr` in a
  cleanup block is allowed there; rusty_sloth's Linux build was not tried.
- **`realloc` is not affected.** The live-pointer path goes through `realloc_live`
  (`#[inline(never)]`); `realloc_move`'s inlined goto stays in a function with no drops.

## 3. The fix (written, verified, unmerged)

Branch **`fix/msvc-funclet-dealloc`**, commit **`7037d8d`** (on `rusty_alloc-v2.2.4`). It lives
in a worktree of this repo outside the tree; `git log fix/msvc-funclet-dealloc` shows it.

On MSVC targets `dealloc` calls `free_outlined`, an `#[inline(never)]` wrapper of `free_inline`.
It has no drops, so it has no funclets, and the goto stays inside it. Other targets are
unchanged.

```rust
// GlobalAlloc::dealloc
unsafe {
    core::hint::assert_unchecked(!ptr.is_null());
    #[cfg(not(target_env = "msvc"))]
    rusty_alloc::alloc::free_inline(ptr);
    #[cfg(target_env = "msvc")]
    free_outlined(ptr);
}

#[cfg(target_env = "msvc")]
#[inline(never)]
unsafe fn free_outlined(ptr: *mut u8) {
    unsafe {
        core::hint::assert_unchecked(!ptr.is_null());
        rusty_alloc::alloc::free_inline(ptr)
    }
}
```

**Cost on MSVC:** every Rust deallocation pays one `call` into `free_outlined` and its return;
the fast path inside is unchanged. That is the `jmp` 2.2.1 removed, now a `call`. Not measured
yet: §6 step 3.

**Alternative considered:** gate on `cfg(panic = "unwind")` instead of `target_env = "msvc"`, so
an aborting MSVC build keeps the inlined path. It is not chosen: rusty_alloc-api is compiled with
the consumer's panic strategy, so this would work, but it is not where the defect is. The defect
is MSVC funclets; `panic = "abort"` only hides them. Revisit if step 3 shows the call costs
something measurable.

## 4. Verified in the consumer

rusty_sloth was built on `7037d8d` through a temporary `[patch.crates-io]`; that patch is not
committed, and rusty_sloth is back on 2.2.0.

- **Build:** the release binary builds (`cargo build --release --features cuda -p rusty_sloth-cli`).
- **Every rusty_sloth gate the same:**
  - Llama / Qwen3 SFT, DPO and two-turn census outputs;
  - a 200-token and a 3-turn chat;
  - an SFT and a DPO resume;
  - GLM-4-MoE training (5-step losses to the bit) and decode text.
- **rusty_alloc-api's own tests pass** (`cargo test --release -p rusty_alloc-api`), clippy and
  fmt are clean.
- **What 2.2.4 brings this consumer** — the stranded-span fix. rusty_sloth quantises a model's
  weights on worker threads that exit, and frees the results on the main thread. A 2.8B GLM
  load plus a 129-token chat, the process sampled every 50 ms, two runs each, identical:

| | 2.2.0 | 2.2.4 + `7037d8d` |
|---|---:|---:|
| private bytes at the end | 5,103 MB | **4,077 MB** |
| working set at the end | 2,133 / 2,149 MB | **1,062 / 1,058 MB** |
| peak working set | 7,551 / 7,568 MB | 6,397 / 6,294 MB |

## 5. A regression test that would have caught it

Add a CI job on **`windows-latest`** that builds a small consumer, a workspace member or an
`examples/` binary, with:

```toml
[profile.release]   # its own profile, NOT the workspace's panic = "abort"
lto = "thin"
codegen-units = 1
panic = "unwind"
```

It should install `rusty_alloc_api::RustyAlloc` as `#[global_allocator]`, and have a function that owns a `Vec`/`String` and calls something
that can panic. That puts a `drop` (a `dealloc`) into a cleanup funclet. Build it with
`cargo build --release`; the job fails if LLVM rejects the module. Building is the whole test,
and running it is optional.

Check that it **fails on 2.2.4 (`410ae26`) and passes on `7037d8d`** before trusting it.
Workspaces don't allow one member its own `[profile]`, so the consumer is probably its own tiny
workspace (`tests/msvc-unwind-consumer/`, with `path` dependencies) built with
`--manifest-path`.

## 6. To do (rusty_alloc session)

1. **Review and land the fix.** Cherry-pick `7037d8d` onto `main` (or merge
   `fix/msvc-funclet-dealloc`).
2. **Add the regression job** (§5). Confirm it fails on `410ae26` and passes with the fix.
3. **Price the call on MSVC** with the usual Windows instruments (the perl / sqlite allocator
   instruction counts the `deferred_free` fix was priced with, or the bench suite). Record the
   number in the changelog. If it is measurable, consider the §3 alternative.
4. **Search for other `asm!` gotos** that reach a consumer through an inlinable path:
   `rg "label \{" crates/`. Today they are the three in `alloc.rs`: two in `free_inline`, one in
   `free_general` (`#[inline(never)]`, safe).
5. **Changelog** (`crates/rusty_alloc_api/CHANGELOG.md` and `crates/rusty_alloc/CHANGELOG.md` as
   release-plz expects). Under `### Fixed`: "2.2.1–2.2.4 failed to build in an MSVC consumer
   that unwinds, with LTO: `GlobalAlloc::dealloc` inlined `free_inline`'s `asm!` goto into EH
   cleanup funclets (LLVM: 'Bogus funclet pad use'). On MSVC `dealloc` now calls it through a
   non-inlined wrapper." Add the measured cost from step 3.
6. **Release 2.2.5** through the usual release-plz PR.
7. **Tell the consumer.** rusty_sloth then runs `cargo add rusty_alloc-api@2.2.5 -p
   rusty_sloth-alloc` and its gates. Other MSVC consumers that unwind with LTO, if any (spacedb,
   rusty_zstd and rusty_alloc_default were named in 2.2.4's semver note), are affected the same
   way from 2.2.1.

## 7. Open questions

- Is 2.2.1–2.2.4 worth a yank on crates.io? The failure is a build error, not a miscompile:
  nothing ships broken, so a release note pointing at 2.2.5 may be enough.
- Would an LLVM / rustc issue be worth filing? A `callbr` inlined into a funclet should either be
  handled or refused at inline time, not left for the verifier. A minimal reproducer is an
  `asm!` goto in an `#[inline(always)]` function called from a `Drop` on MSVC with LTO.

## 8. Resolution (2026-10-07, rusty_alloc session)

**The fix is in `free_inline`, not in `dealloc`.** `7037d8d` outlined only
`GlobalAlloc::dealloc`, so the other callers were still exposed:
`rusty_alloc_api::Heap::dealloc` calls `rusty_alloc::alloc::free`, whose body
is `free_inline`, and LTO is free to inline it into a consumer's drop. Instead,
the asm goto's cfg in `free_inline` now excludes
`all(target_env = "msvc", panic = "unwind")`. Those builds take the
plain-Rust decrement that aarch64 and Miri already use. MSVC builds with
`panic = "abort"`, which have no funclets, keep the asm. `7037d8d` is not
merged; its branch is left in place.

**Cost** (§6 step 3). Callgrind cannot run an MSVC build, so the same code was
forced on Linux and counted there: +3 Ir per local free, exactly opps #6's
figure. Opscan small 52.26 -> 55.26, big 94 -> 97, mixed 94.26 -> 97.26. On the
Rust `GlobalAlloc` workloads it cost maps +0.30 %, threads +0.40 %,
overaligned +0.62 % and trees +3.56 % (free-heavy), with identical checksums.
Outlining would have cost about +2 (call and return) and covered one caller.

**Regression gate** (§5). A synthetic consumer did NOT reproduce. Four
shapes, thin and fat LTO, all built clean on the unfixed code: a `Vec` owned
across a panicking call, a `Heap::dealloc` in a `Drop`, a direct
`GlobalAlloc::dealloc` in a `Drop`, and an unwind-only guard with an
`#[inline(always)]` drop. The IR shows why: rustc marks calls in cleanup blocks
cold, so the inliner leaves the drop out of line and the `callbr` stays on the
normal path. Whatever rusty_sloth does to defeat that is not small. So the gate
is **rusty_sloth itself**, as a corpus row built `--release`
(`tools/corpus/corpus.toml`, `build = "release"`). It is a local gate, not a CI
job, because rusty_sloth is not on CI's machines.
