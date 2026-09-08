# The downstream corpus

A major version is a promise that consumers can keep compiling, and **nothing
inside this repo can check it.** 2.0.0 makes two breaking changes that are
invisible from here and land on other people:

- `default-features = false` now selects `no_std` (which additionally refuses to
  build without `--cfg ra_single_threaded`). It used to be identical to the
  default, because there were no default features.
- `heap::Heap` gained a field, and every field on it is `pub`.

```sh
bash tools/corpus/run.sh          # compile gate
bash tools/corpus/run.sh --test   # + each consumer's test suite
```

Consumers are registered in `corpus.toml`. Add one the moment it takes a
dependency on this crate.

## What it does

Two arms per consumer. **BASELINE** is the consumer as it sits on disk, with its
pinned `rusty_alloc` from crates.io — it exists to prove the consumer was green
to begin with, so a red candidate can be blamed on us rather than on it.
**CANDIDATE** is a copy with every `rusty_alloc*` requirement rewritten to this
working tree.

`[patch.crates-io]` cannot do this. A patch has to satisfy the original
requirement, and `=1.1.6` is not satisfied by `2.0.0`. Simulating an upgrade
therefore means rewriting the requirement, which is done in a **copy** — this
script never writes inside a consumer's checkout.

## First run, 2026-09-08

| consumer | result |
|---|---|
| `spacedb-sdk` | PASS |
| `spacedb-sdk` (`secure`) | PASS |
| `rusty_alloc_default` | PASS |
| `rusty_zstd` | PASS |
| **`rusty_maplibre`** | **FAIL — broken by 2.0.0** |

`rusty_maplibre` takes both `rusty_alloc` and `rusty_alloc-api` with
`default-features = false`, which is exactly the redefinition. It stops at the
`compile_error!` that 2.0.0 added. **The migration is one line per dependency
and it was verified here, not guessed:**

```toml
rusty_alloc     = { version = "2", default-features = false, features = ["std"] }
rusty_alloc-api = { version = "2", default-features = false, features = ["std"] }
```

With that applied to the copy, the `rusty_alloc` error is gone.

## Four things this harness got wrong before it got anything right

Recorded because each one made it lie, and a corpus that lies is worse than none
— it gets muted.

1. **Tab is IFS whitespace.** The registry was tab-separated, bash collapses runs
   of whitespace IFS, and an empty `features` field shifted every column after
   it — so each consumer's *note* was passed to `--features`. All five baselines
   went red and the report blamed the world. The delimiter is `|` now.
2. **A missing workspace root is not a red build.** `packages/spacedb/*` inherit
   their sibling deps with `workspace = true` and mata-master carries no
   manifest above them, so cargo could not parse the manifest at all. Reported
   as SKIP with the reason, and then fixed properly: the crates inherit only
   *dependencies*, never package fields, so a root is reconstructable. The
   harness synthesises one **in the copy** and SpaceDB became testable.
3. **A red candidate is not automatically our fault.** Once `rusty_alloc`
   compiled, `rusty_maplibre` failed on an unrelated `bytes` import. A candidate
   whose error never mentions `rusty_alloc` is now reported UNRELATED, not FAIL.
4. **A resource-starved run misclassifies.** One full run hit
   `fork: retry: Resource temporarily unavailable` and maplibre's real error was
   replaced by `only metadata stub found for rlib dependency core` — which the
   UNRELATED rule then swallowed. Re-running that consumer alone restored the
   true FAIL. **If a run shows fork or cygheap errors, re-run the affected
   consumer in isolation before believing its row.**

## What it still does not do

Compile gates only, unless `--test` is passed. It does not measure downstream
*performance*, so an allocator change that compiles everywhere and slows a
consumer down would pass this and want `bench/wasm-speed`-style A/B in the
consumer's own repo.
