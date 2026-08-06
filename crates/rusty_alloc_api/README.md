# rusty_alloc_api

The safe Rust surface over [`rusty_alloc`](https://crates.io/crates/rusty_alloc),
a pure-Rust remake of mimalloc.

**Status: `0.3.1` — a 0.x release.** See the
[`rusty_alloc`](https://crates.io/crates/rusty_alloc) crate page for exactly
what has and has not been measured — in particular, there is no wall-clock
evidence yet and therefore no speed claim.

## What it gives you

- **`GlobalAlloc`** — drop it in as `#[global_allocator]`.
- **First-class heaps** — create independent heaps, allocate from them, destroy
  them wholesale.
- **`Allocator`** — the unstable `allocator_api` trait, behind a feature.

```rust
use rusty_alloc_api::RustyAlloc;

#[global_allocator]
static ALLOC: RustyAlloc = RustyAlloc;

fn main() {
    let v: Vec<u64> = (0..1000).collect();
    println!("{}", v.len());
}
```

## Features

`debug_checks`, `secure`, `profile` — each forwards to the identically-named
feature on `rusty_alloc`.

## License

MIT. See `LICENSE` at the repository root.
