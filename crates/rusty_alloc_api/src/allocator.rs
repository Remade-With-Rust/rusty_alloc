//! [`core::alloc::Allocator`] for the global handle and for first-class heaps.
//!
//! The trait is stable from Rust 1.100, so this module sits behind the
//! `allocator-api` feature and the crate's own minimum Rust version does not
//! move. With it, the standard collections that take an allocator can live in
//! this allocator directly:
//!
//! ```
//! use rusty_alloc_api::{Heap, RustyAlloc};
//!
//! // One collection on rusty_alloc, whatever the program's global allocator is.
//! let mut v = Vec::new_in(RustyAlloc);
//! v.extend([1u64, 2, 3]);
//!
//! // Collections inside a first-class heap. They borrow it, so none of them
//! // can outlive it, and dropping the heap releases whatever is left.
//! let heap = Heap::new_destroyable();
//! let mut names = Vec::new_in(&heap);
//! names.push(Box::new_in(7u32, &heap));
//! assert_eq!(**names.last().unwrap(), 7);
//! ```
//!
//! A collection in a heap is tied to the heap's thread, as the heap is:
//!
//! ```compile_fail,E0277
//! use rusty_alloc_api::Heap;
//! fn must_be_send<T: Send>(_: T) {}
//! let heap = Heap::new();
//! must_be_send(Vec::<u8, &Heap>::new_in(&heap));
//! ```
//!
//! and it cannot outlive the heap it borrows:
//!
//! ```compile_fail,E0597
//! use rusty_alloc_api::Heap;
//! let v = {
//!     let heap = Heap::new_destroyable();
//!     Vec::<u8, &Heap>::new_in(&heap)
//! };
//! drop(v);
//! ```
//!
//! # How each rule of the trait's contract is met
//!
//! * **Zero-sized layouts** never reach the allocator: `allocate` answers an
//!   aligned dangling pointer of length 0 and `deallocate` ignores it, the
//!   arrangement `std::alloc::Global` uses.
//! * **The length returned is the length asked for.** The trait allows a
//!   larger one, but `Vec` in 1.100 discards it, so reporting the size class
//!   would cost a lookup on every allocation and buy nothing. A layout
//!   therefore *fits* a block exactly when it is the layout that made it.
//! * **A block stays valid until it is deallocated, or its heap is dropped.**
//!   [`RustyAlloc`] has no state, so its blocks live until they are freed.
//!   Dropping a [`Heap`] invalidates the blocks of a destroyable heap, which
//!   the contract allows of an allocator's destructor; a collection reaches a
//!   heap through `&Heap`, so the borrow checker rules that out in safe code.
//! * **Equivalence.** Every `RustyAlloc` is equivalent to every other. A
//!   `Heap` is equivalent only to itself (and, as the standard library
//!   defines, to references to it). A block is never to be handed to a
//!   different heap or to `RustyAlloc`, even where the allocator underneath
//!   would cope.
//! * **Nothing unwinds.** Allocation failure is `Err`, and the allocator's own
//!   failure paths abort the process. That leaves the panics the compiler
//!   puts in: the allocator indexes its own tables, and a release build of
//!   it has a bounds check the optimiser could not remove on some of the
//!   paths these methods reach (`tools/no-unwind-scan.sh` counts them from
//!   the machine code and holds the count to a baseline). None can fire
//!   while the allocator's state is intact, but "cannot fire" is not what
//!   the contract asks, so every method runs under a guard that turns an
//!   unwind into an abort, in every build. Measured, callgrind, the
//!   collection work of `bench/rust-globalalloc/src/alloc_work.rs`: the guard
//!   costs 27.6 instructions per step on `Vec::new_in(RustyAlloc)` (2,507.89
//!   to 2,535.48, where the same work through `#[global_allocator]` is
//!   2,533.37) and 6.7 inside a `Heap`: a call that was a jump before. It
//!   goes when the scan reads zero.
//! * **`grow` and `shrink` may change the alignment**, which
//!   `GlobalAlloc::realloc` cannot: an equal alignment goes to the in-place
//!   reallocation, a different one is an allocate, copy and free.
//! * **A failed `grow` or `shrink` leaves the old block as it was**: the new
//!   block is allocated before the old one is touched.

use core::alloc::{AllocError, Allocator, GlobalAlloc, Layout};
use core::ptr::NonNull;

use crate::{Heap, NATURAL_ALIGN, RustyAlloc, WORD};

/// Armed for the length of one trait method. It is forgotten on the way out,
/// so its destructor runs only if the method is unwinding, and a panic raised
/// during an unwind aborts the process.
struct AbortOnUnwind;

impl Drop for AbortOnUnwind {
    fn drop(&mut self) {
        panic!("rusty_alloc: an Allocator method must not unwind; aborting");
    }
}

/// Run one trait method's body such that it cannot unwind into the caller.
///
/// The contract forbids it, and the standard library relies on that. What
/// could unwind is a `debug_assert!` in a debug build and, in any build, a
/// bounds check on one of the allocator's own tables (see the module docs).
/// Either firing is a bug in the allocator or its state already destroyed,
/// and it must stop the process, not unwind through a half-updated `Vec`.
/// With `panic = "abort"` nothing unwinds and this is the body alone.
#[inline(always)]
fn no_unwind<R>(body: impl FnOnce() -> R) -> R {
    let guard = AbortOnUnwind;
    let out = body();
    core::mem::forget(guard);
    out
}

/// What a zero-sized layout is given: aligned, dangling, length 0.
#[inline]
fn zero_sized(layout: Layout) -> NonNull<[u8]> {
    let p = core::ptr::without_provenance_mut::<u8>(layout.align());
    // SAFETY: an alignment is a power of two, so it is not zero.
    let p = unsafe { NonNull::new_unchecked(p) };
    NonNull::slice_from_raw_parts(p, 0)
}

/// A raw result as the trait wants it: null is the allocation failing.
#[inline]
fn block(p: *mut u8, size: usize) -> Result<NonNull<[u8]>, AllocError> {
    match NonNull::new(p) {
        Some(p) => Ok(NonNull::slice_from_raw_parts(p, size)),
        None => Err(AllocError),
    }
}

/// Allocate `new`, copy `keep` bytes across, free the old block: the
/// reallocation of last resort, and the only one that can change an alignment.
/// The old block is untouched when the allocation fails.
///
/// # Safety
/// `ptr` is a live block of `a` made with `old`, `old.size()` is not zero and
/// `keep` is at most both sizes.
#[inline]
unsafe fn move_block<A: Allocator>(
    a: &A,
    ptr: NonNull<u8>,
    old: Layout,
    new: Layout,
    keep: usize,
) -> Result<NonNull<[u8]>, AllocError> {
    let moved = a.allocate(new)?;
    // SAFETY: both blocks are live and at least `keep` bytes long, and a
    // fresh block cannot overlap a live one; then the old block is freed
    // once, with the layout that made it.
    unsafe {
        core::ptr::copy_nonoverlapping(ptr.as_ptr(), moved.cast::<u8>().as_ptr(), keep);
        a.deallocate(ptr, old);
    }
    Ok(moved)
}

/// Zero `block[from..]`, the part of a grown block the caller has not seen.
///
/// # Safety
/// `block` is a live allocation and `from` is at most its length.
#[inline]
unsafe fn zero_tail(block: NonNull<[u8]>, from: usize) {
    // SAFETY: per contract the range is inside one live allocation.
    unsafe {
        block
            .cast::<u8>()
            .as_ptr()
            .add(from)
            .write_bytes(0, block.len() - from);
    }
}

// SAFETY: the blocks come from the same paths as `GlobalAlloc for RustyAlloc`,
// which satisfy the layout's size and alignment, are disjoint, and stay valid
// until freed: the handle has no state, so neither dropping it nor moving it
// touches a block, and every `RustyAlloc` frees what any other allocated.
// Zero-sized layouts are answered here and never reach the allocator. No
// method unwinds: allocation failure is `Err`, and the allocator's own
// failure paths abort. See the module docs for the contract clause by clause.
unsafe impl Allocator for RustyAlloc {
    #[inline]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        no_unwind(|| {
            if layout.size() == 0 {
                return Ok(zero_sized(layout));
            }
            // SAFETY: the size is not zero, `GlobalAlloc::alloc`'s one demand.
            block(unsafe { GlobalAlloc::alloc(self, layout) }, layout.size())
        })
    }

    #[inline]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        no_unwind(|| {
            if layout.size() == 0 {
                return Ok(zero_sized(layout));
            }
            // SAFETY: the size is not zero.
            block(
                unsafe { GlobalAlloc::alloc_zeroed(self, layout) },
                layout.size(),
            )
        })
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        no_unwind(|| {
            if layout.size() != 0 {
                // SAFETY: a block of a non-zero layout came from `allocate` and
                // friends, so it is a live block of this allocator, freed once.
                unsafe { GlobalAlloc::dealloc(self, ptr.as_ptr(), layout) }
            }
        })
    }

    #[inline]
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        no_unwind(|| {
            if old_layout.size() == 0 {
                return self.allocate(new_layout);
            }
            if old_layout.align() == new_layout.align() {
                // SAFETY: `ptr` is a live block made with `old_layout`, and the
                // new size is at least the old one, so it is not zero; a valid
                // `new_layout` bounds it as `realloc` requires. On failure
                // `realloc` returns null and leaves the block alone.
                let p = unsafe {
                    GlobalAlloc::realloc(self, ptr.as_ptr(), old_layout, new_layout.size())
                };
                return block(p, new_layout.size());
            }
            // SAFETY: forwarded contract; a grow keeps the whole old block.
            unsafe { move_block(self, ptr, old_layout, new_layout, old_layout.size()) }
        })
    }

    #[inline]
    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        no_unwind(|| {
            if old_layout.size() == 0 {
                return self.allocate_zeroed(new_layout);
            }
            // SAFETY: forwarded contract.
            let grown = unsafe { self.grow(ptr, old_layout, new_layout)? };
            // SAFETY: `grown` is `new_layout.size()` long, at least the old size.
            unsafe { zero_tail(grown, old_layout.size()) };
            Ok(grown)
        })
    }

    #[inline]
    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        no_unwind(|| {
            if new_layout.size() == 0 {
                // SAFETY: forwarded contract: the old block is freed once.
                unsafe { self.deallocate(ptr, old_layout) };
                return Ok(zero_sized(new_layout));
            }
            if old_layout.align() == new_layout.align() {
                // SAFETY: as `grow`; the new size was just seen to be non-zero.
                let p = unsafe {
                    GlobalAlloc::realloc(self, ptr.as_ptr(), old_layout, new_layout.size())
                };
                return block(p, new_layout.size());
            }
            // SAFETY: forwarded contract; a shrink keeps the new size's worth.
            unsafe { move_block(self, ptr, old_layout, new_layout, new_layout.size()) }
        })
    }
}

impl Heap {
    /// Reallocate inside this heap at an unchanged alignment. Null is the
    /// allocation failing, with the old block left alone.
    ///
    /// # Safety
    /// `ptr` is a live block of this heap made with a layout of `old_size`
    /// bytes and `align`; `new_size` is not zero and is a valid size for
    /// `align`.
    #[inline]
    unsafe fn realloc_same_align(
        &self,
        ptr: NonNull<u8>,
        old_size: usize,
        align: usize,
        new_size: usize,
    ) -> *mut u8 {
        if align <= NATURAL_ALIGN {
            // The size classes carry these alignments (see `NATURAL_ALIGN`),
            // provided a request above one word is at least two words long,
            // the same raise `Heap::alloc` applies.
            let size = if align <= WORD {
                new_size
            } else {
                new_size.max(NATURAL_ALIGN)
            };
            // SAFETY: the heap is live and this is its thread (`Heap` is
            // neither `Send` nor `Sync`); `ptr` is a live block.
            return unsafe { rusty_alloc::alloc::heap_realloc(self.hb, ptr.as_ptr(), size) };
        }
        // Above `NATURAL_ALIGN` only the aligned path can make the block, so a
        // move has to go back through it. The block keeps its alignment if it
        // stays, and it stays under the allocator's own in-place rule: the new
        // size fits and at least half the block remains in use.
        // SAFETY: `ptr` is a live block.
        let usable = unsafe { rusty_alloc::alloc::usable_size(ptr.as_ptr()) };
        if new_size <= usable && new_size >= usable / 2 {
            return ptr.as_ptr();
        }
        // SAFETY: heap live on its thread; `align` came from a `Layout`.
        let moved =
            unsafe { rusty_alloc::alloc::heap_malloc_aligned_at(self.hb, new_size, align, 0) };
        if !moved.is_null() {
            // SAFETY: both blocks are live and hold the bytes copied; the old
            // one is then freed once.
            unsafe {
                core::ptr::copy_nonoverlapping(ptr.as_ptr(), moved, old_size.min(new_size));
                rusty_alloc::alloc::free(ptr.as_ptr());
            }
        }
        moved
    }
}

// SAFETY: the blocks come from `Heap::alloc`, which satisfies the layout, and
// stay valid until they are freed or the heap is dropped; dropping is the
// allocator's destructor running, which the contract lets invalidate. `Heap`
// has no `&mut` API and is not `Clone`, so nothing else invalidates a block,
// and a block is only ever returned to the heap that made it. `Heap` is
// neither `Send` nor `Sync`, which keeps every call on the owning thread.
// Zero-sized layouts never reach the heap, and no method unwinds. See the
// module docs for the contract clause by clause.
unsafe impl Allocator for Heap {
    #[inline]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        no_unwind(|| {
            if layout.size() == 0 {
                return Ok(zero_sized(layout));
            }
            match self.alloc(layout) {
                Some(p) => Ok(NonNull::slice_from_raw_parts(p, layout.size())),
                None => Err(AllocError),
            }
        })
    }

    #[inline]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        no_unwind(|| {
            if layout.size() == 0 {
                return Ok(zero_sized(layout));
            }
            match self.alloc_zeroed(layout) {
                Some(p) => Ok(NonNull::slice_from_raw_parts(p, layout.size())),
                None => Err(AllocError),
            }
        })
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        no_unwind(|| {
            if layout.size() != 0 {
                // SAFETY: a block of a non-zero layout is a live block of this
                // heap, freed once.
                unsafe { self.dealloc(ptr) }
            }
        })
    }

    #[inline]
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        no_unwind(|| {
            if old_layout.size() == 0 {
                return self.allocate(new_layout);
            }
            if old_layout.align() == new_layout.align() {
                // SAFETY: `ptr` is a live block of this heap made with
                // `old_layout`; the new size is at least the old, so not zero.
                let p = unsafe {
                    self.realloc_same_align(
                        ptr,
                        old_layout.size(),
                        new_layout.align(),
                        new_layout.size(),
                    )
                };
                return block(p, new_layout.size());
            }
            // SAFETY: forwarded contract; a grow keeps the whole old block.
            unsafe { move_block(self, ptr, old_layout, new_layout, old_layout.size()) }
        })
    }

    #[inline]
    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        no_unwind(|| {
            if old_layout.size() == 0 {
                return self.allocate_zeroed(new_layout);
            }
            // SAFETY: forwarded contract.
            let grown = unsafe { self.grow(ptr, old_layout, new_layout)? };
            // SAFETY: `grown` is `new_layout.size()` long, at least the old size.
            unsafe { zero_tail(grown, old_layout.size()) };
            Ok(grown)
        })
    }

    #[inline]
    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        no_unwind(|| {
            if new_layout.size() == 0 {
                // SAFETY: forwarded contract: the old block is freed once.
                unsafe { self.deallocate(ptr, old_layout) };
                return Ok(zero_sized(new_layout));
            }
            if old_layout.align() == new_layout.align() {
                // SAFETY: as `grow`; the new size was just seen to be non-zero.
                let p = unsafe {
                    self.realloc_same_align(
                        ptr,
                        old_layout.size(),
                        new_layout.align(),
                        new_layout.size(),
                    )
                };
                return block(p, new_layout.size());
            }
            // SAFETY: forwarded contract; a shrink keeps the new size's worth.
            unsafe { move_block(self, ptr, old_layout, new_layout, new_layout.size()) }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::no_unwind;

    /// A panic inside a guarded body ends the process; it does not unwind to
    /// the caller. Run in a child, since the point is that it does not return.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a process")]
    fn an_unwind_out_of_a_method_aborts() {
        const CHILD: &str = "RA_NO_UNWIND_CHILD";
        if std::env::var_os(CHILD).is_some() {
            no_unwind(|| panic!("deliberate: the guard must abort on this"));
        }
        let exe = std::env::current_exe().expect("the test binary's own path");
        let out = std::process::Command::new(exe)
            .args([
                "--exact",
                "allocator::tests::an_unwind_out_of_a_method_aborts",
            ])
            .env(CHILD, "1")
            .output()
            .expect("spawn the child");
        assert!(
            !out.status.success(),
            "the child survived a panic in a guarded body"
        );
        // 101 is the test harness reporting a caught panic: the unwind got out.
        assert_ne!(
            out.status.code(),
            Some(101),
            "the panic unwound to the harness instead of aborting"
        );
    }
}
