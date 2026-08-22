//! `malloc`/`free`/`operator new` interposition (plan §5.13, mirrors upstream
//! `alloc-override.c`). This crate is how the 1:1 corpus loads us unmodified:
//! `LD_PRELOAD=librusty_alloc_override.so <benchmark>`.
//!
//! Unix-only by design: on Windows these exports would hijack every process
//! that links the workspace (redirection is post-v1), so the Windows build of
//! this crate exports nothing.

#![deny(missing_docs)]

pub use rusty_alloc_ffi::mi_version;

#[cfg(unix)]
mod unix_override {
    use core::ffi::{c_char, c_int, c_void};
    use rusty_alloc::alloc;

    /// `malloc`.
    #[unsafe(no_mangle)]
    pub extern "C" fn malloc(size: usize) -> *mut c_void {
        alloc::malloc(size).cast()
    }

    /// `calloc`.
    #[unsafe(no_mangle)]
    pub extern "C" fn calloc(count: usize, size: usize) -> *mut c_void {
        alloc::calloc(count, size).cast()
    }

    /// `realloc`.
    ///
    /// # Safety
    /// libc contract.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn realloc(p: *mut c_void, newsize: usize) -> *mut c_void {
        // SAFETY: forwarded libc contract. NOTE: exporting the body via a
        // `realloc_inline` twin — the arrangement `free` uses — was tried and
        // measured +6.31 Ir/op. `free`'s body is small enough that inlining it
        // removes a thunk and needs no frame; `realloc`'s is not, so the export
        // pays a prologue that costs more than the thunk it saved.
        unsafe { alloc::realloc(p.cast(), newsize).cast() }
    }

    /// `free`.
    ///
    /// # Safety
    /// libc contract.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn free(p: *mut c_void) {
        // SAFETY: forwarded libc contract. `free_inline` so this export IS
        // the body instead of a GOT-indirect thunk paid on every free; see
        // its docs for why the internal callers deliberately do not inline.
        unsafe { alloc::free_inline(p.cast()) }
    }

    /// `posix_memalign`.
    ///
    /// # Safety
    /// libc contract.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn posix_memalign(
        out: *mut *mut c_void,
        align: usize,
        size: usize,
    ) -> c_int {
        // SAFETY: forwarded libc contract. `posix_memalign_impl` so this
        // export IS the body instead of a cross-crate `extern "C"` call, the
        // same reason `free` uses `free_inline`.
        unsafe { rusty_alloc_ffi::posix_memalign_impl(out, align, size) }
    }

    /// `memalign`.
    #[unsafe(no_mangle)]
    pub extern "C" fn memalign(align: usize, size: usize) -> *mut c_void {
        alloc::malloc_aligned(size, align).cast()
    }

    /// `aligned_alloc`.
    #[unsafe(no_mangle)]
    pub extern "C" fn aligned_alloc(align: usize, size: usize) -> *mut c_void {
        alloc::malloc_aligned(size, align).cast()
    }

    /// `valloc`.
    #[unsafe(no_mangle)]
    pub extern "C" fn valloc(size: usize) -> *mut c_void {
        alloc::malloc_aligned(size, rusty_alloc::os::page_size()).cast()
    }

    /// `pvalloc`.
    #[unsafe(no_mangle)]
    pub extern "C" fn pvalloc(size: usize) -> *mut c_void {
        let ps = rusty_alloc::os::page_size();
        alloc::malloc_aligned(rusty_alloc::os::page_align_up(size), ps).cast()
    }

    /// `malloc_usable_size`.
    ///
    /// # Safety
    /// libc contract.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn malloc_usable_size(p: *mut c_void) -> usize {
        // SAFETY: forwarded libc contract.
        unsafe { alloc::usable_size(p.cast()) }
    }

    /// `reallocarray`.
    ///
    /// # Safety
    /// libc contract.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn reallocarray(
        p: *mut c_void,
        count: usize,
        size: usize,
    ) -> *mut c_void {
        // SAFETY: forwarded libc contract.
        unsafe { alloc::reallocn(p.cast(), count, size).cast() }
    }

    /// `strdup`.
    ///
    /// # Safety
    /// libc contract.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn strdup(s: *const c_char) -> *mut c_char {
        // SAFETY: forwarded libc contract.
        unsafe { rusty_alloc_ffi::mi_strdup(s) }
    }

    /// `strndup`.
    ///
    /// # Safety
    /// libc contract.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn strndup(s: *const c_char, n: usize) -> *mut c_char {
        // SAFETY: forwarded libc contract.
        unsafe { rusty_alloc_ffi::mi_strndup(s, n) }
    }

    // C++ operator new/delete (Itanium-mangled export names — what a C++
    // benchmark's calls resolve to under LD_PRELOAD).

    /// `operator new(size_t)`.
    #[unsafe(export_name = "_Znwm")]
    pub extern "C" fn cpp_new(size: usize) -> *mut c_void {
        // `new_impl` so this export IS the body, not a cross-crate call.
        rusty_alloc_ffi::new_impl(size)
    }

    /// `operator new[](size_t)`.
    #[unsafe(export_name = "_Znam")]
    pub extern "C" fn cpp_new_arr(size: usize) -> *mut c_void {
        // `new_impl` so this export IS the body, not a cross-crate call.
        rusty_alloc_ffi::new_impl(size)
    }

    /// `operator delete(void*)`.
    ///
    /// # Safety
    /// C++ contract.
    #[unsafe(export_name = "_ZdlPv")]
    pub unsafe extern "C" fn cpp_delete(p: *mut c_void) {
        // SAFETY: forwarded contract. `free_inline` for the same reason the C
        // `free` export uses it: this export IS a free and does nothing else,
        // so carrying the body beats a call through the outlined one. C++
        // programs reach the allocator through here, not through `free`.
        unsafe { alloc::free_inline(p.cast()) }
    }

    /// `operator delete[](void*)`.
    ///
    /// # Safety
    /// C++ contract.
    #[unsafe(export_name = "_ZdaPv")]
    pub unsafe extern "C" fn cpp_delete_arr(p: *mut c_void) {
        // SAFETY: forwarded contract. `free_inline` for the same reason the C
        // `free` export uses it: this export IS a free and does nothing else,
        // so carrying the body beats a call through the outlined one. C++
        // programs reach the allocator through here, not through `free`.
        unsafe { alloc::free_inline(p.cast()) }
    }

    /// sized `operator delete(void*, size_t)`.
    ///
    /// # Safety
    /// C++ contract.
    #[unsafe(export_name = "_ZdlPvm")]
    pub unsafe extern "C" fn cpp_delete_sized(p: *mut c_void, size: usize) {
        // SIBLING CHECK (2026-08-21): the unsized deletes above were paying a
        // cross-crate `extern "C"` hop, worth −150M Ir on alloc-test once
        // inlined. These two had the same shape. `mi_free_size` uses `size`
        // only for a debug assertion, so a sized delete IS a free — keep the
        // assertion, then carry the body.
        #[cfg(debug_assertions)]
        {
            // SAFETY: `p` is null or a live pointer from this allocator, per
            // the C++ contract, which is exactly `usable_size`'s requirement.
            let usable = unsafe { alloc::usable_size(p.cast()) };
            debug_assert!(
                p.is_null() || size <= usable,
                "sized delete: size exceeds the block's usable extent"
            );
        }
        let _ = size;
        // SAFETY: forwarded contract.
        unsafe { alloc::free_inline(p.cast()) }
    }

    /// sized `operator delete[](void*, size_t)`.
    ///
    /// # Safety
    /// C++ contract.
    #[unsafe(export_name = "_ZdaPvm")]
    pub unsafe extern "C" fn cpp_delete_arr_sized(p: *mut c_void, size: usize) {
        // SIBLING CHECK (2026-08-21): the unsized deletes above were paying a
        // cross-crate `extern "C"` hop, worth −150M Ir on alloc-test once
        // inlined. These two had the same shape. `mi_free_size` uses `size`
        // only for a debug assertion, so a sized delete IS a free — keep the
        // assertion, then carry the body.
        #[cfg(debug_assertions)]
        {
            // SAFETY: `p` is null or a live pointer from this allocator, per
            // the C++ contract, which is exactly `usable_size`'s requirement.
            let usable = unsafe { alloc::usable_size(p.cast()) };
            debug_assert!(
                p.is_null() || size <= usable,
                "sized delete: size exceeds the block's usable extent"
            );
        }
        let _ = size;
        // SAFETY: forwarded contract.
        unsafe { alloc::free_inline(p.cast()) }
    }

    /// aligned `operator new(size_t, align_val_t)`.
    #[unsafe(export_name = "_ZnwmSt11align_val_t")]
    pub extern "C" fn cpp_new_aligned(size: usize, align: usize) -> *mut c_void {
        // Sibling of the `new_impl` collapse above.
        rusty_alloc_ffi::new_aligned_impl(size, align)
    }

    /// aligned `operator new[](size_t, align_val_t)`.
    #[unsafe(export_name = "_ZnamSt11align_val_t")]
    pub extern "C" fn cpp_new_arr_aligned(size: usize, align: usize) -> *mut c_void {
        // Sibling of the `new_impl` collapse above.
        rusty_alloc_ffi::new_aligned_impl(size, align)
    }

    /// aligned `operator delete(void*, align_val_t)`.
    ///
    /// # Safety
    /// C++ contract.
    #[unsafe(export_name = "_ZdlPvSt11align_val_t")]
    pub unsafe extern "C" fn cpp_delete_aligned(p: *mut c_void, _align: usize) {
        // SAFETY: forwarded contract. `free_inline` for the same reason the C
        // `free` export uses it: this export IS a free and does nothing else,
        // so carrying the body beats a call through the outlined one. C++
        // programs reach the allocator through here, not through `free`.
        unsafe { alloc::free_inline(p.cast()) }
    }

    /// aligned `operator delete[](void*, align_val_t)`.
    ///
    /// # Safety
    /// C++ contract.
    #[unsafe(export_name = "_ZdaPvSt11align_val_t")]
    pub unsafe extern "C" fn cpp_delete_arr_aligned(p: *mut c_void, _align: usize) {
        // SAFETY: forwarded contract. `free_inline` for the same reason the C
        // `free` export uses it: this export IS a free and does nothing else,
        // so carrying the body beats a call through the outlined one. C++
        // programs reach the allocator through here, not through `free`.
        unsafe { alloc::free_inline(p.cast()) }
    }
}
