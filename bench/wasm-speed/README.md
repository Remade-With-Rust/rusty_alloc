# wasm-speed — rusty_alloc vs the Rust default allocator, inside a real VM

Size was measured twice before anyone asked the other half: **do the extra bytes
buy anything?** (`docs/plans/wasm-size.md` §8.)

```sh
cd bench/wasm-speed
cargo build --release --target wasm32-unknown-unknown            && cp target/wasm32-unknown-unknown/release/speedprobe.wasm dl.wasm
cargo build --release --target wasm32-unknown-unknown --features ra && cp target/wasm32-unknown-unknown/release/speedprobe.wasm ra.wasm
node run.mjs dl.wasm ra.wasm
```

One source; `--features ra` swaps the allocator and changes nothing else.

**Read the floor line first.** Both modules run identical floor code, so their
floors must agree; the harness prints a warning when they differ by more than
25 %. The first version of this benchmark reported floors of 5.2 and 0.6 ns/op
for the same code, because V8 tiers wasm up per code path and only one branch
had been warmed. Every branch is warmed now.

**Checksums must match across arms** — printed on every row. They are what
proves both allocators serviced the identical size sequence, and that the
optimiser did not delete an alloc/free pair and leave an empty loop being timed.

**Repeat before believing a row.** Between-process variance is ±25 %, so a 10 %
effect is not resolvable here. Only churn (~5x) and 2048 B (~0.7x) survive
repeats; the other two straddle 1.0 and are reported as ranges.
