//! Twelve exports, one per `Allocator` method of each impl. See `Cargo.toml`
//! for why this crate exists and `tools/no-unwind-scan.sh` for what reads it.
//!
//! Every export takes its layout as raw numbers and builds it unchecked, and
//! takes the heap by reference, so the only code in the object is the method
//! under test and what it calls: no `Layout` validation that could panic, no
//! `Heap::new` (which asserts), nothing of the caller's.

use core::alloc::{Allocator, Layout};
use core::ptr::NonNull;
use rusty_alloc_api::{Heap, RustyAlloc};

/// # Safety
/// `align` is a power of two and `size` rounded up to it fits `isize`.
#[inline(always)]
unsafe fn layout(size: usize, align: usize) -> Layout {
    // SAFETY: the caller's contract is `Layout`'s.
    unsafe { Layout::from_size_align_unchecked(size, align) }
}

/// Null for `Err`, the block's address otherwise.
#[inline(always)]
fn out(r: Result<NonNull<[u8]>, core::alloc::AllocError>) -> *mut u8 {
    match r {
        Ok(b) => b.cast::<u8>().as_ptr(),
        Err(_) => core::ptr::null_mut(),
    }
}

macro_rules! probes {
    ($allocate:ident, $zeroed:ident, $deallocate:ident, $grow:ident, $grow_zeroed:ident,
     $shrink:ident; ($($param:ident: $ty:ty),*) => $a:expr) => {
        /// # Safety
        /// A valid layout.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $allocate($($param: $ty,)* size: usize, align: usize) -> *mut u8 {
            // SAFETY: forwarded contract.
            out($a.allocate(unsafe { layout(size, align) }))
        }

        /// # Safety
        /// A valid layout.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $zeroed($($param: $ty,)* size: usize, align: usize) -> *mut u8 {
            // SAFETY: forwarded contract.
            out($a.allocate_zeroed(unsafe { layout(size, align) }))
        }

        /// # Safety
        /// `p` is a live block of this allocator made with this layout.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $deallocate($($param: $ty,)* p: NonNull<u8>, size: usize, align: usize) {
            // SAFETY: forwarded contract.
            unsafe { $a.deallocate(p, layout(size, align)) }
        }

        /// # Safety
        /// `Allocator::grow`'s contract.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $grow(
            $($param: $ty,)* p: NonNull<u8>, old_size: usize, old_align: usize,
            new_size: usize, new_align: usize,
        ) -> *mut u8 {
            // SAFETY: forwarded contract.
            out(unsafe { $a.grow(p, layout(old_size, old_align), layout(new_size, new_align)) })
        }

        /// # Safety
        /// `Allocator::grow_zeroed`'s contract.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $grow_zeroed(
            $($param: $ty,)* p: NonNull<u8>, old_size: usize, old_align: usize,
            new_size: usize, new_align: usize,
        ) -> *mut u8 {
            // SAFETY: forwarded contract.
            out(unsafe {
                $a.grow_zeroed(p, layout(old_size, old_align), layout(new_size, new_align))
            })
        }

        /// # Safety
        /// `Allocator::shrink`'s contract.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $shrink(
            $($param: $ty,)* p: NonNull<u8>, old_size: usize, old_align: usize,
            new_size: usize, new_align: usize,
        ) -> *mut u8 {
            // SAFETY: forwarded contract.
            out(unsafe { $a.shrink(p, layout(old_size, old_align), layout(new_size, new_align)) })
        }
    };
}

probes!(probe_global_allocate, probe_global_allocate_zeroed, probe_global_deallocate,
        probe_global_grow, probe_global_grow_zeroed, probe_global_shrink; () => RustyAlloc);
probes!(probe_heap_allocate, probe_heap_allocate_zeroed, probe_heap_deallocate,
        probe_heap_grow, probe_heap_grow_zeroed, probe_heap_shrink; (h: &Heap) => h);

/// The scan's control arm: an index the compiler cannot prove, so this export
/// reaches `core::panicking`. Compiled only for the self-test.
#[cfg(feature = "selftest")]
#[unsafe(no_mangle)]
pub extern "C" fn probe_selftest_can_panic(v: &[u8; 16], i: usize) -> u8 {
    v[i]
}
