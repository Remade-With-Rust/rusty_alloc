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
        // SAFETY: forwarded libc contract.
        unsafe { alloc::realloc(p.cast(), newsize).cast() }
    }

    /// `free`.
    ///
    /// # Safety
    /// libc contract.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn free(p: *mut c_void) {
        // SAFETY: forwarded libc contract.
        unsafe { alloc::free(p.cast()) }
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
        // SAFETY: forwarded libc contract.
        unsafe { rusty_alloc_ffi::mi_posix_memalign(out, align, size) }
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
        rusty_alloc_ffi::mi_new(size)
    }

    /// `operator new[](size_t)`.
    #[unsafe(export_name = "_Znam")]
    pub extern "C" fn cpp_new_arr(size: usize) -> *mut c_void {
        rusty_alloc_ffi::mi_new(size)
    }

    /// `operator delete(void*)`.
    ///
    /// # Safety
    /// C++ contract.
    #[unsafe(export_name = "_ZdlPv")]
    pub unsafe extern "C" fn cpp_delete(p: *mut c_void) {
        // SAFETY: forwarded contract.
        unsafe { alloc::free(p.cast()) }
    }

    /// `operator delete[](void*)`.
    ///
    /// # Safety
    /// C++ contract.
    #[unsafe(export_name = "_ZdaPv")]
    pub unsafe extern "C" fn cpp_delete_arr(p: *mut c_void) {
        // SAFETY: forwarded contract.
        unsafe { alloc::free(p.cast()) }
    }

    /// sized `operator delete(void*, size_t)`.
    ///
    /// # Safety
    /// C++ contract.
    #[unsafe(export_name = "_ZdlPvm")]
    pub unsafe extern "C" fn cpp_delete_sized(p: *mut c_void, size: usize) {
        // SAFETY: forwarded contract.
        unsafe { rusty_alloc_ffi::mi_free_size(p, size) }
    }

    /// sized `operator delete[](void*, size_t)`.
    ///
    /// # Safety
    /// C++ contract.
    #[unsafe(export_name = "_ZdaPvm")]
    pub unsafe extern "C" fn cpp_delete_arr_sized(p: *mut c_void, size: usize) {
        // SAFETY: forwarded contract.
        unsafe { rusty_alloc_ffi::mi_free_size(p, size) }
    }

    /// aligned `operator new(size_t, align_val_t)`.
    #[unsafe(export_name = "_ZnwmSt11align_val_t")]
    pub extern "C" fn cpp_new_aligned(size: usize, align: usize) -> *mut c_void {
        rusty_alloc_ffi::mi_new_aligned(size, align)
    }

    /// aligned `operator new[](size_t, align_val_t)`.
    #[unsafe(export_name = "_ZnamSt11align_val_t")]
    pub extern "C" fn cpp_new_arr_aligned(size: usize, align: usize) -> *mut c_void {
        rusty_alloc_ffi::mi_new_aligned(size, align)
    }

    /// aligned `operator delete(void*, align_val_t)`.
    ///
    /// # Safety
    /// C++ contract.
    #[unsafe(export_name = "_ZdlPvSt11align_val_t")]
    pub unsafe extern "C" fn cpp_delete_aligned(p: *mut c_void, _align: usize) {
        // SAFETY: forwarded contract.
        unsafe { alloc::free(p.cast()) }
    }

    /// aligned `operator delete[](void*, align_val_t)`.
    ///
    /// # Safety
    /// C++ contract.
    #[unsafe(export_name = "_ZdaPvSt11align_val_t")]
    pub unsafe extern "C" fn cpp_delete_arr_aligned(p: *mut c_void, _align: usize) {
        // SAFETY: forwarded contract.
        unsafe { alloc::free(p.cast()) }
    }
}
