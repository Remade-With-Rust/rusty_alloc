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

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]

// ---------------------------------------------------------------------------
// P3 of `docs/plans/small-metal.md`: the three things the crate used `std` FOR.
//
// These live here, above the `mod` lines, because `macro_rules!` is TEXTUALLY
// scoped — a macro defined after a module is invisible inside it.
// ---------------------------------------------------------------------------

/// End the process immediately, without unwinding.
///
/// A double free, a corrupted free list and a failed TLS slot all reach this:
/// the allocator's contract is that it aborts rather than continues, and
/// unwinding out of `free` into a C caller is not an option (which is why the
/// release profile is `panic = "abort"`).
///
/// Without `std` there is no `process::abort`, so this panics and relies on the
/// deliverable's panic strategy. **A `no_std` consumer MUST build with
/// `panic = "abort"`** — every Janus firmware profile already does — or an
/// abort becomes an unwind and the guarantee is gone.
#[cold]
#[inline(never)]
pub(crate) fn abort() -> ! {
    #[cfg(feature = "std")]
    {
        std::process::abort()
    }
    #[cfg(not(feature = "std"))]
    {
        panic!("rusty_alloc: abort (build a no_std consumer with panic = \"abort\")")
    }
}

/// A `thread_local!` that survives `no_std` — the single-heap profile.
///
/// With `std` this expands to `std::thread_local!` unchanged, so the shipped
/// build keeps the const-init, `!Drop`, initial-exec fast path M10c measured.
///
/// Without it there is no thread-local storage and, on the targets this crate
/// serves without `std`, no second thread either: `prim::fixed::thread_id`
/// returns a constant and its TLS is a fixed table whose destructors never run,
/// because there is no thread exit. So a "thread-local" becomes a plain
/// `static` — which is not a compromise but the point of the profile: one heap,
/// no TLS lookup at all, a SHORTER fast path than the threaded one.
/// **`wasm32-unknown-unknown` takes the single-`static` arm too, not just
/// `no_std`.** That target has exactly one thread unless the atomics+threads
/// proposal is on, which `prim/wasm.rs` has assumed since it was written. A
/// `std::thread_local!` there still links lazy initialisation, destructor
/// registration and the "accessed during or after destruction" panic — none of
/// which can ever run — and the strings for it ship in every module.
///
/// `target_feature = "atomics"` is the precise switch: it is what
/// `-C target-feature=+atomics` sets to build wasm WITH threads, and such a
/// build keeps real TLS.
macro_rules! ra_thread_local {
    ($($(#[$m:meta])* static $N:ident: $T:ty = const $init:block;)*) => {
        #[cfg(all(feature = "std", not(all(target_arch = "wasm32", target_os = "unknown", not(target_feature = "atomics")))))]
        std::thread_local! {
            $($(#[$m])* static $N: $T = const $init;)*
        }
        $(
            #[cfg(any(not(feature = "std"), all(target_arch = "wasm32", target_os = "unknown", not(target_feature = "atomics"))))]
            $(#[$m])*
            static $N: $crate::SingleThreadCell<$T> =
                $crate::SingleThreadCell::new($init);
        )*
    };
}

// The `no_std` build asserts single-threadedness, so it must be OPTED INTO.
//
// Three things in a `no_std` build are sound only because there is exactly one
// thread: [`SingleThreadCell`]'s `unsafe impl Sync`, `prim::fixed`'s constant
// thread id and never-contended spin lock, and `options`' 64-bit atomics split
// into `AtomicU32` halves. None of them is checkable at compile time, and none
// of them fails loudly if the assumption breaks — they corrupt quietly.
//
// A doc comment is not a guard. `no_std` here therefore requires
// `--cfg ra_single_threaded`, so that using this allocator on a bare-metal
// target is a decision somebody wrote down rather than a default they
// inherited. There is no cost to it and no way around it:
//
// ```text
// RUSTFLAGS="--cfg ra_single_threaded" cargo build --no-default-features
// ```
//
// If your target has more than one thread touching the allocator, do not set
// it — enable the `std` feature instead, or the port is not done.
#[cfg(all(not(feature = "std"), not(ra_single_threaded), not(doc)))]
compile_error!(
    "rusty_alloc's no_std build assumes a SINGLE THREAD (SingleThreadCell's \
     `unsafe impl Sync`, prim::fixed's constant thread id and spin lock, and \
     options' split 64-bit atomics all depend on it). Confirm that is true of \
     your target and opt in with `--cfg ra_single_threaded`, or enable the \
     `std` feature. See the crate docs on SingleThreadCell."
);

/// The single-thread half of [`ra_thread_local!`]: a `static` with a `.with()`.
#[cfg(any(
    not(feature = "std"),
    all(
        target_arch = "wasm32",
        target_os = "unknown",
        not(target_feature = "atomics")
    )
))]
pub(crate) struct SingleThreadCell<T>(T);

#[cfg(any(
    not(feature = "std"),
    all(
        target_arch = "wasm32",
        target_os = "unknown",
        not(target_feature = "atomics")
    )
))]
// SAFETY: only ever constructed by `ra_thread_local!`, and only on a target
// this crate serves single-threaded: a `no_std` build (which must opt in with
// `--cfg ra_single_threaded`), or `wasm32-unknown-unknown` without the atomics
// proposal, where `prim/wasm.rs` has assumed one thread since it was written.
// The same standing assumption as `prim::fixed` (constant thread id, TLS
// destructors that never fire, a spin lock that never contends). With one
// thread there is no other referent, so shared access cannot race. A build on a
// target that grows threads must revisit this type FIRST — which is what the
// `target_feature = "atomics"` half of the condition above is there to catch.
unsafe impl<T> Sync for SingleThreadCell<T> {}

#[cfg(any(
    not(feature = "std"),
    all(
        target_arch = "wasm32",
        target_os = "unknown",
        not(target_feature = "atomics")
    )
))]
impl<T> SingleThreadCell<T> {
    pub(crate) const fn new(v: T) -> Self {
        Self(v)
    }
    /// Mirrors `LocalKey::with`, which is the only accessor the crate uses.
    pub(crate) fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.0)
    }
}

/// `true` exactly when this build has ONE thread for the life of the program,
/// so that `prim::thread_id()` is a compile-time constant.
///
/// That is two targets: the bare-metal `prim::fixed` backend, which the crate
/// refuses to build without `--cfg ra_single_threaded`, and
/// `wasm32-unknown-unknown` without the atomics proposal, where
/// `prim/wasm.rs` has returned one id since it was written. It is **not**
/// `ra_single_threaded` alone: on a hosted target that cfg only unlocks the
/// fixed backend's unit tests, the OS prim still hands out real thread ids,
/// and the suite spawns threads.
///
/// What it buys: the cross-thread machinery — abandoning a segment when a
/// thread ends, adopting one back, the delayed-free list a remote free lands
/// on — is code that CANNOT execute here, and `ra_single_threaded` used to
/// prune none of it. The linker kept `adopt_segment` (1,154 B) and
/// `drain_delayed` (995 B) in an ESP32-S3 firmware that had asserted a single
/// context. Each consumer of this constant folds one branch so that code
/// becomes provably unreachable and the linker drops it; where a hosted build
/// would have compared thread ids, it still does
/// (`docs/plans/finished/firmware-code-size.md`, lever 1).
///
/// A `const`, not a `cfg`, so every site reads as `if ONE_THREAD` and a host
/// build compiles both arms — the pruned code is type-checked and unit-tested
/// everywhere, and only linked where it can run.
pub(crate) const ONE_THREAD: bool = cfg!(any(
    all(
        ra_single_threaded,
        not(miri),
        not(windows),
        not(unix),
        not(target_arch = "wasm32")
    ),
    all(
        target_arch = "wasm32",
        target_os = "unknown",
        not(target_feature = "atomics")
    )
));

/// `true` where `prim::protect` can actually protect a page: the OS backends
/// and the miri mock.
///
/// Guarded objects are a huge segment whose trailing page is `PROT_NONE`, so
/// an overflow faults on the first byte past the object. `prim::fixed` and
/// `prim::wasm` have no MMU and return `Err` from `protect`; there the
/// sampler used to run anyway and hand out a dedicated segment with an
/// UNPROTECTED trailing page — the whole cost of a guarded object and none of
/// the protection — while `try_guarded` (1,641 B) and the ChaCha block it
/// samples with (725 B) stayed in an ESP32-S3 image with `secure` off. The
/// runtime gate (`guarded_rate`) could not remove them: it is a field, and a
/// linker cannot prove a field is zero. Every consumer folds on this constant
/// instead (`docs/plans/finished/firmware-code-size.md`, lever 3).
pub(crate) const GUARD_PAGES: bool = cfg!(any(unix, windows, miri));

/// Whether anything draws from a heap's CSPRNG: `secure` free-list keys, or
/// guarded sampling. A build with neither never seeds it.
pub(crate) const RNG_USED: bool = GUARD_PAGES || cfg!(feature = "secure");

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
// Wired into the segment paths only on wasm (F2, docs/plans/segment-tax.md);
// native builds compile it for its unit tests, so its items are "unused"
// there by design.
#[cfg_attr(not(all(target_arch = "wasm32", not(miri))), allow(dead_code))]
pub(crate) mod slice_pool;
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
