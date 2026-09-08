# wasm-size — what rusty_alloc costs a browser, and why it cost twice that

**Source:** a GitHub message from someone integrating the crate, reporting
**+12 % on their gzipped wasm bundle**. Written 2026-09-08 against `main` at
2.0.0.

**Status: MEASURED and HALVED.** The allocator's gzipped overhead on a minimal
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
