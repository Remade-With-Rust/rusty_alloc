# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.7](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v1.1.6...rusty_alloc-v1.1.7) - 2026-09-04

### Other

- an In the wild block above the headline ([#13](https://github.com/Remade-With-Rust/rusty_alloc/pull/13))

## [1.1.6](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v1.1.5...rusty_alloc-v1.1.6) - 2026-08-28

### Other

- *(wasm)* dissolve the segment tax — slice-aligned segments and a free-slice pool (F2)
- *(heap)* route every span-sized allocation in-segment — the segment tax, F1+F5

## [1.1.5](https://github.com/Remade-With-Rust/rusty_alloc/compare/rusty_alloc-v1.1.4...rusty_alloc-v1.1.5) - 2026-08-28

### Fixed

- *(arena)* keep the public Arena shape, harden the adoption tests, own the census
- *(wasm)* recycle freed segments — adopt-on-free arenas replace the no-op leak
