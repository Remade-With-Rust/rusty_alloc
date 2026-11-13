//! Safe Rust-native surface of rusty_alloc (plan §5.14).
//!
//! From M2, [`RustyAlloc`] is a real [`core::alloc::GlobalAlloc`]:
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;
//! ```
//!
//! [`Heap`] is a first-class heap. With the `allocator-api` feature (Rust
//! 1.100 and later) both it and [`RustyAlloc`] implement
//! [`core::alloc::Allocator`], so `Vec::new_in(&heap)` and
//! `Box::new_in(x, RustyAlloc)` work; the `allocator` module's docs go through
//! the trait's contract clause by clause. This crate stays a thin veneer over
//! the same internals as the C ABI — no separate code path, so corpus numbers
//! speak for Rust users too.

#![cfg_attr(not(test), no_std)]
#![deny(missing_docs)]

use core::alloc::{GlobalAlloc, Layout};

pub use rusty_alloc::{MI_COMPAT_VERSION, VERSION, version};

// The one module above the crate's `rust-version`, which is why it is a
// feature. Telling clippy its real floor keeps `incompatible_msrv` checking
// the rest of the crate against 1.88, and this module against 1.100.
#[cfg(feature = "allocator-api")]
#[clippy::msrv = "1.100"]
mod allocator;

/// The global allocator handle (zero-sized).
///
/// It holds no state: every `RustyAlloc` is the same allocator, so copying
/// one, or making another, gives a handle that frees what any other
/// allocated.
#[derive(Clone, Copy, Debug, Default)]
pub struct RustyAlloc;

/// One machine word: every block the allocator hands out is aligned this far.
const WORD: usize = core::mem::size_of::<usize>();

/// The alignment the size classes already give every block of MORE than one
/// word: `bins::bin` rounds word counts up to even ones (upstream's
/// `MI_ALIGN2W`), and every page area starts on a slice boundary, so a block
/// of two words or more sits on a two-word boundary — 16 bytes on a 64-bit
/// target, `MAX_ALIGN_SIZE`. Only the one-word class is not, which is why a
/// request aligned between one and two words is raised to two words.
///
/// This is the alignment hashbrown asks for on every table (its SSE2 control
/// group is 16 bytes), so a Rust `HashMap` used to go through the aligned
/// path on every allocation and through allocate-copy-free on every realloc.
const NATURAL_ALIGN: usize = 2 * WORD;

/// A first-class heap (plan §5.14): `Drop` runs `mi_heap_delete` semantics
/// (blocks migrate to the thread's backing heap and stay valid) unless built
/// with [`Heap::new_destroyable`], where `Drop` releases every block at once.
/// The destroyable form inherits C's contract for the raw methods below:
/// callers must not touch its blocks after drop. With the `allocator-api`
/// feature that is unrepresentable in safe code: a collection made with
/// `Vec::new_in(&heap)` borrows the heap and cannot outlive it.
pub struct Heap {
    hb: *mut rusty_alloc::init::HeapBox,
    destroy_on_drop: bool,
}

impl Heap {
    /// New heap; dropped ⇒ blocks migrate to the backing heap.
    ///
    /// # Panics
    /// When the OS refuses the heap's backing mapping (memory exhaustion) —
    /// a defined panic, matching std's convention for infallible
    /// constructors, rather than a null pointer carried into later use.
    pub fn new() -> Heap {
        let hb = rusty_alloc::init::create_heap(0, false, -1);
        assert!(!hb.is_null(), "rusty_alloc: heap creation failed (OOM)");
        Heap {
            hb,
            destroy_on_drop: false,
        }
    }

    /// New heap; dropped ⇒ every allocation is released wholesale
    /// (arena-style teardown).
    ///
    /// # Panics
    /// As [`Heap::new`], on memory exhaustion.
    pub fn new_destroyable() -> Heap {
        let hb = rusty_alloc::init::create_heap(0, true, -1);
        assert!(!hb.is_null(), "rusty_alloc: heap creation failed (OOM)");
        Heap {
            hb,
            destroy_on_drop: true,
        }
    }

    /// Allocate `layout`, borrowing the heap (so the block cannot outlive it).
    pub fn alloc(&self, layout: core::alloc::Layout) -> Option<core::ptr::NonNull<u8>> {
        // SAFETY: hb live (we own it), called on the owning thread by the
        // !Send/!Sync nature of raw-pointer fields.
        //
        // The three tiers are `GlobalAlloc::alloc`'s. They are stated in
        // words, not as `<= 8`: on a 32-bit target a one-word block is four
        // bytes and only four-aligned, so `<= 8` handed an eight-aligned
        // layout of four bytes or fewer a block that could sit on an odd
        // word. Up to `NATURAL_ALIGN` the size classes carry the alignment
        // once the request is two words long.
        let p = unsafe {
            if layout.align() <= WORD {
                rusty_alloc::alloc::heap_malloc(self.hb, layout.size())
            } else if layout.align() <= NATURAL_ALIGN {
                rusty_alloc::alloc::heap_malloc(self.hb, layout.size().max(NATURAL_ALIGN))
            } else {
                rusty_alloc::alloc::heap_malloc_aligned_at(
                    self.hb,
                    layout.size(),
                    layout.align(),
                    0,
                )
            }
        };
        core::ptr::NonNull::new(p)
    }

    /// Zeroed variant of [`alloc`](Self::alloc).
    pub fn alloc_zeroed(&self, layout: core::alloc::Layout) -> Option<core::ptr::NonNull<u8>> {
        // SAFETY: as alloc.
        let p = unsafe {
            if layout.align() <= WORD {
                rusty_alloc::alloc::heap_zalloc(self.hb, layout.size())
            } else if layout.align() <= NATURAL_ALIGN {
                rusty_alloc::alloc::heap_zalloc(self.hb, layout.size().max(NATURAL_ALIGN))
            } else {
                rusty_alloc::alloc::heap_zalloc_aligned_at(
                    self.hb,
                    layout.size(),
                    layout.align(),
                    0,
                )
            }
        };
        core::ptr::NonNull::new(p)
    }

    /// Free a block previously allocated from this heap.
    ///
    /// # Safety
    /// `p` came from this heap's alloc methods and is freed exactly once.
    pub unsafe fn dealloc(&self, p: core::ptr::NonNull<u8>) {
        // SAFETY: forwarded contract.
        unsafe { rusty_alloc::alloc::free(p.as_ptr()) }
    }

    /// Drain cross-thread frees and retire empty pages.
    pub fn collect(&self) {
        // SAFETY: owner thread (see alloc).
        unsafe { rusty_alloc::alloc::heap_collect(self.hb, true) }
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        // SAFETY: we own hb; exactly one of delete/destroy runs, once.
        unsafe {
            if self.destroy_on_drop {
                rusty_alloc::init::heap_destroy(self.hb);
            } else {
                rusty_alloc::init::heap_delete(self.hb);
            }
        }
    }
}

// SAFETY: GlobalAlloc contract — Layout-described allocation/free delegated to
// the rusty_alloc core, which returns blocks satisfying the layout's size and
// alignment (natural bins up to `NATURAL_ALIGN`; the aligned path above it)
// and accepts any such block back in `free` regardless of which thread frees
// it (M4: per-thread heaps, no lock — `free` routes by the segment's owner and
// hands cross-thread blocks to the loom-modeled remote protocol).
//
// Every method is `#[inline]`, as the `mimalloc` crate's are. rustc generates
// `__rust_alloc` and friends in the crate that declares `#[global_allocator]`,
// and without the hint each one was a shim that loaded `&self`, shuffled the
// arguments and jumped through the GOT into an out-of-line method — on every
// Rust allocation and every drop. With it the fast paths land in the shims and
// in their callers. Measured whole-program (callgrind, same output): a boxed-
// tree/buffer/`Rc` workload −17.9 %, a HashMap/BTreeMap/String one −4.8 %, for
// +1,888 and +2,784 bytes of text.
unsafe impl GlobalAlloc for RustyAlloc {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= WORD {
            rusty_alloc::alloc::malloc(layout.size())
        } else if layout.align() > NATURAL_ALIGN {
            // SAFETY: `Layout` guarantees a power-of-two alignment, the one
            // precondition `malloc_aligned_pow2` adds over `malloc_aligned`.
            unsafe { rusty_alloc::alloc::malloc_aligned_pow2(layout.size(), layout.align()) }
        } else {
            alloc_natural(layout.size())
        }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        // `free_inline_flags_first` (the body of `free_inline`, in the test
        // order that keeps `__rust_dealloc` inlinable; see its doc), not
        // `free`: `dealloc` IS a free and does nothing
        // else, the case `free_inline` exists for (the LD_PRELOAD export is
        // the other). Through `free` every Rust deallocation paid a `jmp`
        // into it and its null test; `GlobalAlloc` never passes null, and the
        // hint lets that test fold away.
        // SAFETY: GlobalAlloc contract — ptr came from `alloc`, is non-null
        // and is freed once.
        unsafe {
            core::hint::assert_unchecked(!ptr.is_null());
            rusty_alloc::alloc::free_inline_flags_first(ptr)
        }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= WORD {
            rusty_alloc::alloc::zalloc(layout.size())
        } else if layout.align() <= NATURAL_ALIGN {
            rusty_alloc::alloc::zalloc(layout.size().max(NATURAL_ALIGN))
        } else {
            rusty_alloc::alloc::zalloc_aligned(layout.size(), layout.align())
        }
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if layout.align() <= NATURAL_ALIGN {
            // Up to `NATURAL_ALIGN` the size classes carry the alignment, so
            // the in-place arm is open to these layouts too: a block that
            // stays keeps its address, and a move lands in a class of at
            // least two words. Before this, every realloc aligned above one
            // word (a growing `Vec<u128>`, say) was an allocate-copy-free.
            //
            // `realloc_rust` takes the four values in the registers
            // `__rust_realloc` holds them in, so a word-aligned layout — all
            // but a handful of real ones — is one compare and a jump; the
            // rare two-word layout pays its `max` on its own branch. As
            // `realloc(ptr, select(new_size, max(new_size, 16)))` every
            // realloc paid the select and the register move.
            // SAFETY: GlobalAlloc contract — ptr live and non-null,
            // invalidated on move; our realloc preserves min(old, new) bytes.
            unsafe {
                let p = core::ptr::NonNull::new_unchecked(ptr);
                if layout.align() <= WORD {
                    rusty_alloc::alloc::realloc_rust(p, layout.size(), layout.align(), new_size)
                } else {
                    rusty_alloc::alloc::realloc_rust(
                        p,
                        layout.size(),
                        layout.align(),
                        new_size.max(NATURAL_ALIGN),
                    )
                }
            }
        } else {
            // Above `NATURAL_ALIGN`. The block already satisfies
            // `layout.align()` and keeps it if it stays, so when the new size
            // fits and at least half the block stays in use — `realloc`'s own
            // in-place rule, which `mi_realloc_aligned` applies too — it
            // stays. This arm used to allocate, copy and free every time,
            // shrinks and fits included (bench/rust-globalalloc
            // `overaligned`).
            //
            // "It stays" is answered HERE, with no frame, and everything
            // else is a jump with the four arguments where `__rust_realloc`
            // received them. As one body, the move's allocation, copy and
            // free had the entry push five callee-saved registers before
            // the first test, and pop them again to return the pointer it
            // was handed.
            // SAFETY: GlobalAlloc contract — ptr is a live, non-null block
            // of ours.
            unsafe {
                let p = core::ptr::NonNull::new_unchecked(ptr);
                match rusty_alloc::alloc::usable_size_plain(p) {
                    Some(usable) => {
                        if new_size <= usable && new_size >= usable / 2 {
                            return ptr;
                        }
                        realloc_overaligned_move(ptr, layout.size(), layout.align(), new_size)
                    }
                    // A flagged page (an interior aligned block, a
                    // single-block span): the general question.
                    None => realloc_overaligned(ptr, layout.size(), layout.align(), new_size),
                }
            }
        }
    }
}

/// The move of an over-aligned `realloc`: allocate through the aligned path,
/// copy the bytes the layout says are live, free. Out of line, with the
/// arguments in `__rust_realloc`'s own registers, so the in-place answer in
/// front of it carries no frame.
///
/// # Safety
/// `GlobalAlloc::realloc`'s contract, with `align` above two words.
#[inline(never)]
unsafe fn realloc_overaligned_move(
    ptr: *mut u8,
    old_size: usize,
    align: usize,
    new_size: usize,
) -> *mut u8 {
    // SAFETY: forwarded GlobalAlloc contract; `align` came from a `Layout`.
    unsafe {
        let np = GlobalAlloc::alloc(
            &RustyAlloc,
            Layout::from_size_align_unchecked(new_size, align),
        );
        if !np.is_null() {
            core::ptr::copy_nonoverlapping(ptr, np, old_size.min(new_size));
            GlobalAlloc::dealloc(
                &RustyAlloc,
                ptr,
                Layout::from_size_align_unchecked(old_size, align),
            );
        }
        np
    }
}

/// An over-aligned `realloc` of a block on a flagged page: its usable size
/// is not its page's block size, so ask the general way, then stay or move.
///
/// # Safety
/// As [`realloc_overaligned_move`].
#[cold]
#[inline(never)]
unsafe fn realloc_overaligned(
    ptr: *mut u8,
    old_size: usize,
    align: usize,
    new_size: usize,
) -> *mut u8 {
    // SAFETY: forwarded contract; `ptr` is a live block of ours.
    unsafe {
        let usable = rusty_alloc::alloc::usable_size(ptr);
        if new_size <= usable && new_size >= usable / 2 {
            return ptr;
        }
        realloc_overaligned_move(ptr, old_size, align, new_size)
    }
}

/// `GlobalAlloc::alloc` for an alignment above one word up to
/// `NATURAL_ALIGN`, out of line. Inline, this arm was a SECOND copy of the
/// malloc fast path in every `__rust_alloc`, which kept the method too big for
/// LLVM to inline into its callers. See `NATURAL_ALIGN`: a class of two words
/// or more is already aligned this far, so only a one-word request needs
/// raising.
#[inline(never)]
fn alloc_natural(size: usize) -> *mut u8 {
    rusty_alloc::alloc::malloc(size.max(NATURAL_ALIGN))
}
