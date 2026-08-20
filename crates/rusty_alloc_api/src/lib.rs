//! Safe Rust-native surface of rusty_alloc (plan §5.14).
//!
//! From M2, [`RustyAlloc`] is a real [`core::alloc::GlobalAlloc`]:
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;
//! ```
//!
//! `Heap` and the `Allocator` trait impl land at M6. This crate stays a thin
//! veneer over the same internals as the C ABI — no separate code path, so
//! corpus numbers speak for Rust users too.

#![cfg_attr(not(test), no_std)]
#![deny(missing_docs)]

use core::alloc::{GlobalAlloc, Layout};

pub use rusty_alloc::{MI_COMPAT_VERSION, VERSION, version};

/// The global allocator handle (zero-sized).
pub struct RustyAlloc;

/// A first-class heap (plan §5.14): `Drop` runs `mi_heap_delete` semantics
/// (blocks migrate to the thread's backing heap and stay valid) unless built
/// with [`Heap::new_destroyable`], where `Drop` releases every block at once.
/// The destroyable form inherits C's contract: callers must not touch its
/// blocks after drop (a lifetime-carrying `Allocator` impl that makes this
/// unrepresentable is the planned follow-up once allocator_api stabilizes).
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
        let p = unsafe {
            if layout.align() <= 8 {
                rusty_alloc::alloc::heap_malloc(self.hb, layout.size())
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
            if layout.align() <= 8 {
                rusty_alloc::alloc::heap_zalloc(self.hb, layout.size())
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
// alignment (natural bins for align ≤ 8; the aligned path otherwise) and
// accepts any such block back in `free` regardless of which thread frees it
// (M4: per-thread heaps, no lock — `free` routes by the segment's owner and
// hands cross-thread blocks to the loom-modeled remote protocol).
unsafe impl GlobalAlloc for RustyAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= 8 {
            rusty_alloc::alloc::malloc(layout.size())
        } else {
            rusty_alloc::alloc::malloc_aligned(layout.size(), layout.align())
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        // SAFETY: GlobalAlloc contract — ptr came from `alloc` and is freed once.
        unsafe { rusty_alloc::alloc::free(ptr) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= 8 {
            rusty_alloc::alloc::zalloc(layout.size())
        } else {
            rusty_alloc::alloc::zalloc_aligned(layout.size(), layout.align())
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if layout.align() <= 8 {
            // SAFETY: GlobalAlloc contract — ptr live, invalidated on move;
            // our realloc preserves min(old, new) bytes.
            unsafe { rusty_alloc::alloc::realloc(ptr, new_size) }
        } else {
            // Aligned realloc lands in M5; the default alloc-copy-dealloc is
            // correct through our aligned paths meanwhile.
            // SAFETY: forwarded GlobalAlloc contract.
            unsafe {
                let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
                let np = GlobalAlloc::alloc(self, new_layout);
                if !np.is_null() {
                    core::ptr::copy_nonoverlapping(ptr, np, layout.size().min(new_size));
                    GlobalAlloc::dealloc(self, ptr, layout);
                }
                np
            }
        }
    }
}
