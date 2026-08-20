//! rusty_alloc core — a pure-Rust remake of mimalloc v2.4.5.
//!
//! Plan of record: `docs/plans/rusty_alloc_v1.md`. Module map mirrors upstream C
//! files 1:1 (plan §6) so every diff-vs-oracle conversation has a shared map.
//!
//! Milestone status: **M4** — per-thread heaps, lock-free cross-thread frees
//! (the loom-modeled xthread/delayed protocol), thread-exit abandonment and
//! segment reclaim. No global lock anywhere on the alloc/free paths.
//!
//! std note: M4's TLS fast path uses `thread_local!` (const-init, !Drop — the
//! R1 spike measured it at atomic-load parity). A no_std profile returns
//! post-v1 with the nightly `#[thread_local]` or a platform TLS shim.

#![deny(missing_docs)]

pub mod alloc;
pub mod arena;
pub mod bins;
pub mod heap;
pub mod init;
pub mod options;
pub mod os;
pub mod page;
pub mod prim;
/// Kani proof harnesses (H-30). `cfg(kani)`-only: absent from every shipped
/// build, so it costs the crate nothing.
#[cfg(kani)]
mod proofs;
pub mod random;
pub mod segment;
pub mod segment_map;
pub mod stats;
pub mod types;

pub use bins::good_size;

/// Rebuild a pointer at `addr` keeping `p`'s provenance. Used wherever an
/// address round-trips through an integer (atomic words, encoded links) — the
/// thrice-learned law: provenance and reachability follow POINTERS.
#[inline]
pub fn ptr_with_addr<T>(p: *mut T, addr: usize) -> *mut T {
    p.with_addr(addr)
}

/// Our own semantic version, from the crate manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The mimalloc version we are API- and ABI-compatible with, in mimalloc's
/// encoding (major·10⁴ + minor·10² + patch): v2.4.5. `mi_version()` reports this.
pub const MI_COMPAT_VERSION: i32 = 20405;

/// mimalloc-encoded compat version, as reported by the C ABI `mi_version()`.
#[inline]
pub const fn version() -> i32 {
    MI_COMPAT_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_v2_4_5_compat() {
        assert_eq!(version(), 20405);
    }
}
