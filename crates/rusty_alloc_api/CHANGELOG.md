# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.0.0](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v1.1.6...rusty_alloc-api-v2.0.0) - 2026-09-07

### Breaking

- **`default-features = false` now selects `no_std`.** This crate's own source
  has been `no_std` since M2; only the core dependency's default features stood
  in the way. `default = ["std"]` is new, and turning it off gives a firmware
  the single-heap profile — which also requires `--cfg ra_single_threaded` on
  the core. Consumers on default features are unaffected.

### Added

- `std` feature (default-on) forwarding to `rusty_alloc/std`, so
  `RustyAlloc` can be a bare-metal `#[global_allocator]`.

## [1.1.6](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-api-v1.1.5...rusty_alloc-api-v1.1.6) - 2026-08-28

### Other

- release v1.1.6 ([#11](https://github.com/Remade-With-Rust/rusty_alloc/pull/11))
