//! `mi_*`-compatible C ABI (plan §5). Exported symbols use mimalloc's exact
//! names so we are a drop-in library, plus `ra_*` aliases where useful.
//! Every export lands in its milestone WITH its differential gate — no stub
//! exports that lie to a linker.
//!
//! M2 exports: the standard + extended allocation family (§5.1/§5.2 subset)
//! and `mi_version`. Realloc/strdup land M3; posix/aligned family M5.
//!
//! Note: the *core* crate is no_std; this FFI shell links std (a no_std cdylib
//! would need its own `#[panic_handler]`, which then collides with std in every
//! test build). Panic across the C boundary aborts via the release profile.

#![deny(missing_docs)]

use core::ffi::{c_int, c_void};

use rusty_alloc::alloc;

/// mimalloc ABI: `int mi_version(void)` — 20405 = v2.4.5 compatibility.
#[unsafe(no_mangle)]
pub extern "C" fn mi_version() -> c_int {
    rusty_alloc::version()
}

/// rusty_alloc's own version handle: same value, our name on the door.
#[unsafe(no_mangle)]
pub extern "C" fn ra_version() -> c_int {
    rusty_alloc::version()
}

/// `void* mi_malloc(size_t size)`
#[unsafe(no_mangle)]
pub extern "C" fn mi_malloc(size: usize) -> *mut c_void {
    alloc::malloc(size).cast()
}

/// `void* mi_zalloc(size_t size)`
#[unsafe(no_mangle)]
pub extern "C" fn mi_zalloc(size: usize) -> *mut c_void {
    alloc::zalloc(size).cast()
}

/// `void* mi_calloc(size_t count, size_t size)`
#[unsafe(no_mangle)]
pub extern "C" fn mi_calloc(count: usize, size: usize) -> *mut c_void {
    alloc::calloc(count, size).cast()
}

/// `void* mi_mallocn(size_t count, size_t size)`
#[unsafe(no_mangle)]
pub extern "C" fn mi_mallocn(count: usize, size: usize) -> *mut c_void {
    alloc::mallocn(count, size).cast()
}

/// `void* mi_malloc_small(size_t size)` — caller promises size ≤ 1 KiB.
#[unsafe(no_mangle)]
pub extern "C" fn mi_malloc_small(size: usize) -> *mut c_void {
    alloc::malloc_small(size).cast()
}

/// `void* mi_zalloc_small(size_t size)`
#[unsafe(no_mangle)]
pub extern "C" fn mi_zalloc_small(size: usize) -> *mut c_void {
    alloc::zalloc_small(size).cast()
}

/// `void* mi_malloc_aligned(size_t size, size_t alignment)`
#[unsafe(no_mangle)]
pub extern "C" fn mi_malloc_aligned(size: usize, alignment: usize) -> *mut c_void {
    alloc::malloc_aligned(size, alignment).cast()
}

/// `void* mi_zalloc_aligned(size_t size, size_t alignment)`
#[unsafe(no_mangle)]
pub extern "C" fn mi_zalloc_aligned(size: usize, alignment: usize) -> *mut c_void {
    alloc::zalloc_aligned(size, alignment).cast()
}

/// `void mi_free(void* p)`
///
/// # Safety
/// C contract: `p` is null or a live pointer from this allocator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_free(p: *mut c_void) {
    // SAFETY: forwarded C contract.
    unsafe { alloc::free(p.cast()) }
}

/// `void mi_free_small(void* p)` (v3-compat alias of free)
///
/// # Safety
/// As [`mi_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_free_small(p: *mut c_void) {
    // SAFETY: forwarded C contract.
    unsafe { alloc::free(p.cast()) }
}

/// `size_t mi_usable_size(const void* p)`
///
/// # Safety
/// C contract: `p` is null or a live pointer from this allocator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_usable_size(p: *const c_void) -> usize {
    // SAFETY: forwarded C contract.
    unsafe { alloc::usable_size(p.cast()) }
}

/// `size_t mi_good_size(size_t size)`
#[unsafe(no_mangle)]
pub extern "C" fn mi_good_size(size: usize) -> usize {
    rusty_alloc::good_size(size)
}

/// `void* mi_realloc(void* p, size_t newsize)`
///
/// # Safety
/// C contract: `p` is null or a live pointer from this allocator; invalidated
/// on move.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_realloc(p: *mut c_void, newsize: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::realloc(p.cast(), newsize).cast() }
}

/// `void* mi_reallocn(void* p, size_t count, size_t size)`
///
/// # Safety
/// As [`mi_realloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_reallocn(p: *mut c_void, count: usize, size: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::reallocn(p.cast(), count, size).cast() }
}

/// `void* mi_reallocf(void* p, size_t newsize)` — frees `p` on failure.
///
/// # Safety
/// As [`mi_realloc`]; `p` is always consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_reallocf(p: *mut c_void, newsize: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::reallocf(p.cast(), newsize).cast() }
}

/// `void* mi_expand(void* p, size_t newsize)` — in-place only.
///
/// # Safety
/// As [`mi_realloc`], but never moves.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_expand(p: *mut c_void, newsize: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::expand(p.cast(), newsize).cast() }
}

/// `char* mi_strdup(const char* s)`
///
/// # Safety
/// C contract: `s` is null or a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_strdup(s: *const core::ffi::c_char) -> *mut core::ffi::c_char {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: s is NUL-terminated per the C contract.
    let len = unsafe { core::ffi::CStr::from_ptr(s) }.to_bytes().len();
    let p = alloc::malloc(len + 1);
    if !p.is_null() {
        // SAFETY: p has len+1 usable bytes; source is readable for len+1.
        unsafe { core::ptr::copy_nonoverlapping(s.cast::<u8>(), p, len + 1) };
    }
    p.cast()
}

/// `char* mi_strndup(const char* s, size_t n)` — copies at most `n` bytes and
/// NUL-terminates.
///
/// # Safety
/// C contract: `s` is null or readable up to `n` bytes or its NUL, whichever
/// comes first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_strndup(
    s: *const core::ffi::c_char,
    n: usize,
) -> *mut core::ffi::c_char {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    // Find length ≤ n without reading past the first NUL.
    let mut len = 0usize;
    // SAFETY: reads stay within the C contract's readable range.
    while len < n && unsafe { *s.add(len) } != 0 {
        len += 1;
    }
    let p = alloc::malloc(len + 1);
    if !p.is_null() {
        // SAFETY: p has len+1 usable bytes; s readable for len.
        unsafe {
            core::ptr::copy_nonoverlapping(s.cast::<u8>(), p, len);
            p.add(len).write(0);
        }
    }
    p.cast()
}

/// `bool mi_is_in_heap_region(const void* p)`
#[unsafe(no_mangle)]
pub extern "C" fn mi_is_in_heap_region(p: *const c_void) -> bool {
    alloc::is_in_heap_region(p.cast())
}

/// `void mi_collect(bool force)` — drain cross-thread frees, retire empties.
#[unsafe(no_mangle)]
pub extern "C" fn mi_collect(force: bool) {
    alloc::collect(force);
}

/// `void mi_thread_init(void)` — heaps are lazy; ensure this thread has one.
#[unsafe(no_mangle)]
pub extern "C" fn mi_thread_init() {
    let _ = rusty_alloc::init::heap_box();
}

/// `void mi_thread_done(void)` — abandon the calling thread's heap early
/// (also runs automatically at thread exit via the TLS destructor).
#[unsafe(no_mangle)]
pub extern "C" fn mi_thread_done() {
    let hb = rusty_alloc::init::heap_box();
    // SAFETY: hb is the calling thread's live box; thread_done clears the TLS
    // pointer so a later allocation re-creates a fresh heap.
    unsafe { rusty_alloc::init::thread_done(hb) };
}

/// `void mi_process_init(void)` — automatic; kept for ABI shape.
#[unsafe(no_mangle)]
pub extern "C" fn mi_process_init() {}

/// `void mi_process_done(void)` — automatic; kept for ABI shape.
#[unsafe(no_mangle)]
pub extern "C" fn mi_process_done() {}

/// `void mi_thread_set_in_threadpool(void)` — hint accepted (abandonment
/// policy tuning lands with options in M7).
#[unsafe(no_mangle)]
pub extern "C" fn mi_thread_set_in_threadpool() {}

// ---------------------------------------------------------------------------
// Aligned family (§5.4)
// ---------------------------------------------------------------------------

/// `mi_malloc_aligned_at(size, alignment, offset)`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_malloc_aligned_at(
    size: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    alloc::malloc_aligned_at(size, alignment, offset).cast()
}

/// `mi_zalloc_aligned_at`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_zalloc_aligned_at(
    size: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    alloc::zalloc_aligned_at(size, alignment, offset).cast()
}

/// `mi_calloc_aligned`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_calloc_aligned(count: usize, size: usize, alignment: usize) -> *mut c_void {
    alloc::calloc_aligned(count, size, alignment).cast()
}

/// `mi_calloc_aligned_at`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_calloc_aligned_at(
    count: usize,
    size: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    alloc::calloc_aligned_at(count, size, alignment, offset).cast()
}

/// `mi_realloc_aligned`.
///
/// # Safety
/// As [`mi_realloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_realloc_aligned(
    p: *mut c_void,
    newsize: usize,
    alignment: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::realloc_aligned(p.cast(), newsize, alignment).cast() }
}

/// `mi_realloc_aligned_at`.
///
/// # Safety
/// As [`mi_realloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_realloc_aligned_at(
    p: *mut c_void,
    newsize: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::realloc_aligned_at(p.cast(), newsize, alignment, offset).cast() }
}

// ---------------------------------------------------------------------------
// Zero-preserving reallocation (§5.7)
// ---------------------------------------------------------------------------

/// `mi_rezalloc` — grown space reads zero (zalloc-lineage blocks only).
///
/// # Safety
/// As [`mi_realloc`] + the zero-lineage contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_rezalloc(p: *mut c_void, newsize: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::rezalloc(p.cast(), newsize).cast() }
}

/// `mi_recalloc`.
///
/// # Safety
/// As [`mi_rezalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_recalloc(p: *mut c_void, newcount: usize, size: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::recalloc(p.cast(), newcount, size).cast() }
}

/// `mi_rezalloc_aligned`.
///
/// # Safety
/// As [`mi_rezalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_rezalloc_aligned(
    p: *mut c_void,
    newsize: usize,
    alignment: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::rezalloc_aligned(p.cast(), newsize, alignment).cast() }
}

/// `mi_rezalloc_aligned_at`.
///
/// # Safety
/// As [`mi_rezalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_rezalloc_aligned_at(
    p: *mut c_void,
    newsize: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::rezalloc_aligned_at(p.cast(), newsize, alignment, offset).cast() }
}

/// `mi_recalloc_aligned`.
///
/// # Safety
/// As [`mi_rezalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_recalloc_aligned(
    p: *mut c_void,
    newcount: usize,
    size: usize,
    alignment: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::recalloc_aligned(p.cast(), newcount, size, alignment).cast() }
}

/// `mi_recalloc_aligned_at`.
///
/// # Safety
/// As [`mi_rezalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_recalloc_aligned_at(
    p: *mut c_void,
    newcount: usize,
    size: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::recalloc_aligned_at(p.cast(), newcount, size, alignment, offset).cast() }
}

// ---------------------------------------------------------------------------
// `u*` block-size-returning variants (§5.5, new in v2.4.x)
// ---------------------------------------------------------------------------

unsafe fn store_bs(out: *mut usize, p: *mut u8) {
    if !out.is_null() {
        // SAFETY: caller passed a valid out-pointer (C contract); p null → 0.
        unsafe {
            out.write(if p.is_null() {
                0
            } else {
                alloc::usable_size(p)
            })
        };
    }
}

/// `mi_umalloc(size, *block_size)`.
///
/// # Safety
/// `block_size` null or valid to write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_umalloc(size: usize, block_size: *mut usize) -> *mut c_void {
    let p = alloc::malloc(size);
    // SAFETY: forwarded C contract.
    unsafe { store_bs(block_size, p) };
    p.cast()
}

/// `mi_ucalloc(count, size, *block_size)`.
///
/// # Safety
/// As [`mi_umalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_ucalloc(
    count: usize,
    size: usize,
    block_size: *mut usize,
) -> *mut c_void {
    let p = alloc::calloc(count, size);
    // SAFETY: forwarded C contract.
    unsafe { store_bs(block_size, p) };
    p.cast()
}

/// `mi_urealloc(p, newsize, *pre, *post)`.
///
/// # Safety
/// As [`mi_realloc`] + valid/null out-pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_urealloc(
    p: *mut c_void,
    newsize: usize,
    block_size_pre: *mut usize,
    block_size_post: *mut usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract throughout.
    unsafe {
        if !block_size_pre.is_null() {
            block_size_pre.write(alloc::usable_size(p.cast()));
        }
        let np = alloc::realloc(p.cast(), newsize);
        store_bs(block_size_post, np);
        np.cast()
    }
}

/// `mi_ufree(p, *block_size)`.
///
/// # Safety
/// As [`mi_free`] + valid/null out-pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_ufree(p: *mut c_void, block_size: *mut usize) {
    // SAFETY: forwarded C contract; usable read before the free.
    unsafe {
        if !block_size.is_null() {
            block_size.write(alloc::usable_size(p.cast()));
        }
        alloc::free(p.cast());
    }
}

/// `mi_umalloc_aligned`.
///
/// # Safety
/// As [`mi_umalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_umalloc_aligned(
    size: usize,
    alignment: usize,
    block_size: *mut usize,
) -> *mut c_void {
    let p = alloc::malloc_aligned(size, alignment);
    // SAFETY: forwarded C contract.
    unsafe { store_bs(block_size, p) };
    p.cast()
}

/// `mi_uzalloc_aligned`.
///
/// # Safety
/// As [`mi_umalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_uzalloc_aligned(
    size: usize,
    alignment: usize,
    block_size: *mut usize,
) -> *mut c_void {
    let p = alloc::zalloc_aligned(size, alignment);
    // SAFETY: forwarded C contract.
    unsafe { store_bs(block_size, p) };
    p.cast()
}

/// `mi_umalloc_small`.
///
/// # Safety
/// As [`mi_umalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_umalloc_small(size: usize, block_size: *mut usize) -> *mut c_void {
    let p = alloc::malloc_small(size);
    // SAFETY: forwarded C contract.
    unsafe { store_bs(block_size, p) };
    p.cast()
}

/// `mi_uzalloc_small`.
///
/// # Safety
/// As [`mi_umalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_uzalloc_small(size: usize, block_size: *mut usize) -> *mut c_void {
    let p = alloc::zalloc_small(size);
    // SAFETY: forwarded C contract.
    unsafe { store_bs(block_size, p) };
    p.cast()
}

// ---------------------------------------------------------------------------
// POSIX / Windows / C compatibility (§5.11 core)
// ---------------------------------------------------------------------------

/// `mi_cfree` — checked free: no-op unless the pointer is in our heap region.
///
/// # Safety
/// `p` null, foreign, or a live pointer from this allocator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_cfree(p: *mut c_void) {
    if alloc::is_in_heap_region(p.cast()) {
        // SAFETY: map hit ⇒ ours per the checked-free contract.
        unsafe { alloc::free(p.cast()) };
    }
}

/// `mi__expand` (MSVC `_expand`): in-place only.
///
/// # Safety
/// As [`mi_expand`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi__expand(p: *mut c_void, newsize: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::expand(p.cast(), newsize).cast() }
}

/// `mi_malloc_size`.
///
/// # Safety
/// As [`mi_usable_size`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_malloc_size(p: *const c_void) -> usize {
    // SAFETY: forwarded C contract.
    unsafe { alloc::usable_size(p.cast()) }
}

/// `mi_malloc_usable_size`.
///
/// # Safety
/// As [`mi_usable_size`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_malloc_usable_size(p: *const c_void) -> usize {
    // SAFETY: forwarded C contract.
    unsafe { alloc::usable_size(p.cast()) }
}

/// `mi_malloc_good_size`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_malloc_good_size(size: usize) -> usize {
    rusty_alloc::good_size(size)
}

/// `mi_posix_memalign(&p, alignment, size)` → 0 / EINVAL(22) / ENOMEM(12).
///
/// # Safety
/// `out` must be a valid pointer-to-pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_posix_memalign(
    out: *mut *mut c_void,
    alignment: usize,
    size: usize,
) -> c_int {
    if out.is_null()
        || !alignment.is_power_of_two()
        || alignment < core::mem::size_of::<*mut c_void>()
    {
        return 22; // EINVAL
    }
    let p = alloc::malloc_aligned(size, alignment);
    if p.is_null() {
        return 12; // ENOMEM
    }
    // SAFETY: out valid per contract.
    unsafe { out.write(p.cast()) };
    0
}

/// `mi_memalign(alignment, size)`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_memalign(alignment: usize, size: usize) -> *mut c_void {
    alloc::malloc_aligned(size, alignment).cast()
}

/// `mi_valloc(size)` — page-aligned.
#[unsafe(no_mangle)]
pub extern "C" fn mi_valloc(size: usize) -> *mut c_void {
    alloc::malloc_aligned(size, rusty_alloc::os::page_size()).cast()
}

/// `mi_pvalloc(size)` — page-aligned, size rounded to whole pages.
#[unsafe(no_mangle)]
pub extern "C" fn mi_pvalloc(size: usize) -> *mut c_void {
    let ps = rusty_alloc::os::page_size();
    alloc::malloc_aligned(rusty_alloc::os::page_align_up(size), ps).cast()
}

/// `mi_aligned_alloc(alignment, size)` (C11 argument order).
#[unsafe(no_mangle)]
pub extern "C" fn mi_aligned_alloc(alignment: usize, size: usize) -> *mut c_void {
    alloc::malloc_aligned(size, alignment).cast()
}

/// `mi_reallocarray(p, count, size)`.
///
/// # Safety
/// As [`mi_realloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_reallocarray(p: *mut c_void, count: usize, size: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::reallocn(p.cast(), count, size).cast() }
}

/// `mi_reallocarr(ptrp, count, size)` (NetBSD shape) → 0 / EINVAL / ENOMEM.
///
/// # Safety
/// `ptrp` must be a valid pointer-to-pointer; `*ptrp` as [`mi_realloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_reallocarr(ptrp: *mut c_void, count: usize, size: usize) -> c_int {
    if ptrp.is_null() {
        return 22; // EINVAL
    }
    let slot: *mut *mut c_void = ptrp.cast();
    // SAFETY: slot valid per contract; realloc contract forwarded.
    unsafe {
        let np = alloc::reallocn((*slot).cast(), count, size);
        if np.is_null() && count.checked_mul(size).map(|t| t > 0).unwrap_or(true) {
            return 12; // ENOMEM (or overflow)
        }
        slot.write(np.cast());
    }
    0
}

/// `mi_aligned_recalloc`.
///
/// # Safety
/// As [`mi_rezalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_aligned_recalloc(
    p: *mut c_void,
    newcount: usize,
    size: usize,
    alignment: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::recalloc_aligned(p.cast(), newcount, size, alignment).cast() }
}

/// `mi_aligned_offset_recalloc`.
///
/// # Safety
/// As [`mi_rezalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_aligned_offset_recalloc(
    p: *mut c_void,
    newcount: usize,
    size: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::recalloc_aligned_at(p.cast(), newcount, size, alignment, offset).cast() }
}

/// `mi_free_size(p, size)` — sized free (fast-path exploitation is an M8
/// brick; the size is verified under debug).
///
/// # Safety
/// As [`mi_free`]; `size` ≤ the block's usable size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_free_size(p: *mut c_void, size: usize) {
    // SAFETY: forwarded C contract.
    unsafe {
        debug_assert!(p.is_null() || size <= alloc::usable_size(p.cast()));
        alloc::free(p.cast());
    }
}

/// `mi_free_size_aligned`.
///
/// # Safety
/// As [`mi_free_size`]; `p` must satisfy the alignment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_free_size_aligned(p: *mut c_void, size: usize, alignment: usize) {
    debug_assert!(p.is_null() || (p as usize).is_multiple_of(alignment.max(1)));
    // SAFETY: forwarded C contract.
    unsafe { mi_free_size(p, size) };
}

/// `mi_free_aligned`.
///
/// # Safety
/// As [`mi_free`]; `p` must satisfy the alignment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_free_aligned(p: *mut c_void, alignment: usize) {
    debug_assert!(p.is_null() || (p as usize).is_multiple_of(alignment.max(1)));
    // SAFETY: forwarded C contract.
    unsafe { alloc::free(p.cast()) };
}

// ===========================================================================
// M6: first-class heaps (§5.6), analysis (§5.8), arenas + subprocs (§5.9)
// ===========================================================================

use rusty_alloc::init::HeapBox;

/// Opaque heap handle (`mi_heap_t*`).
pub type MiHeap = HeapBox;

/// `mi_heap_new`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_heap_new() -> *mut MiHeap {
    rusty_alloc::init::create_heap(0, true, -1)
}

/// `mi_heap_new_ex(heap_tag, allow_destroy, arena_id)`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_heap_new_ex(
    heap_tag: c_int,
    allow_destroy: bool,
    arena_id: c_int,
) -> *mut MiHeap {
    rusty_alloc::init::create_heap(heap_tag, allow_destroy, arena_id)
}

/// `mi_heap_new_in_arena`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_heap_new_in_arena(arena_id: c_int) -> *mut MiHeap {
    rusty_alloc::init::create_heap(0, true, arena_id)
}

/// `mi_heap_delete` — blocks migrate to the backing heap.
///
/// # Safety
/// `heap` live, owned by the calling thread, unused afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_delete(heap: *mut MiHeap) {
    if !heap.is_null() {
        // SAFETY: forwarded C contract.
        unsafe { rusty_alloc::init::heap_delete(heap) };
    }
}

/// `mi_heap_destroy` — drops every block of the heap at once.
///
/// # Safety
/// As [`mi_heap_delete`], plus no block of this heap may be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_destroy(heap: *mut MiHeap) {
    if !heap.is_null() {
        // SAFETY: forwarded C contract.
        unsafe { rusty_alloc::init::heap_destroy(heap) };
    }
}

/// `mi_heap_set_default`.
///
/// # Safety
/// `heap` live and owned by the calling thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_set_default(heap: *mut MiHeap) -> *mut MiHeap {
    // SAFETY: forwarded C contract.
    unsafe { rusty_alloc::init::set_default_heap(heap) }
}

/// `mi_heap_get_default`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_heap_get_default() -> *mut MiHeap {
    rusty_alloc::init::heap_box()
}

/// `mi_heap_get_backing`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_heap_get_backing() -> *mut MiHeap {
    rusty_alloc::init::backing_heap()
}

/// `mi_heap_collect`.
///
/// # Safety
/// `heap` live and owned by the calling thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_collect(heap: *mut MiHeap, force: bool) {
    // SAFETY: forwarded C contract.
    unsafe { alloc::heap_collect(heap, force) };
}

/// `mi_heap_malloc`.
///
/// # Safety
/// `heap` live and owned by the calling thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_malloc(heap: *mut MiHeap, size: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::heap_malloc(heap, size).cast() }
}

/// `mi_heap_zalloc`.
///
/// # Safety
/// As [`mi_heap_malloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_zalloc(heap: *mut MiHeap, size: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::heap_zalloc(heap, size).cast() }
}

/// `mi_heap_calloc`.
///
/// # Safety
/// As [`mi_heap_malloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_calloc(
    heap: *mut MiHeap,
    count: usize,
    size: usize,
) -> *mut c_void {
    match count.checked_mul(size) {
        // SAFETY: forwarded C contract.
        Some(total) => unsafe { alloc::heap_zalloc(heap, total).cast() },
        None => core::ptr::null_mut(),
    }
}

/// `mi_heap_mallocn`.
///
/// # Safety
/// As [`mi_heap_malloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_mallocn(
    heap: *mut MiHeap,
    count: usize,
    size: usize,
) -> *mut c_void {
    match count.checked_mul(size) {
        // SAFETY: forwarded C contract.
        Some(total) => unsafe { alloc::heap_malloc(heap, total).cast() },
        None => core::ptr::null_mut(),
    }
}

/// `mi_heap_malloc_small`.
///
/// # Safety
/// As [`mi_heap_malloc`]; size ≤ 1 KiB.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_malloc_small(heap: *mut MiHeap, size: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::heap_malloc(heap, size).cast() }
}

/// `mi_heap_zalloc_small`.
///
/// # Safety
/// As [`mi_heap_malloc_small`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_zalloc_small(heap: *mut MiHeap, size: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::heap_zalloc(heap, size).cast() }
}

/// `mi_heap_realloc`.
///
/// # Safety
/// As [`mi_heap_malloc`]; `p` as [`mi_realloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_realloc(
    heap: *mut MiHeap,
    p: *mut c_void,
    newsize: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::heap_realloc(heap, p.cast(), newsize).cast() }
}

/// `mi_heap_reallocn`.
///
/// # Safety
/// As [`mi_heap_realloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_reallocn(
    heap: *mut MiHeap,
    p: *mut c_void,
    count: usize,
    size: usize,
) -> *mut c_void {
    match count.checked_mul(size) {
        // SAFETY: forwarded C contract.
        Some(total) => unsafe { alloc::heap_realloc(heap, p.cast(), total).cast() },
        None => core::ptr::null_mut(),
    }
}

/// `mi_heap_reallocf`.
///
/// # Safety
/// As [`mi_heap_realloc`]; `p` always consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_reallocf(
    heap: *mut MiHeap,
    p: *mut c_void,
    newsize: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe {
        let np = alloc::heap_realloc(heap, p.cast(), newsize);
        if np.is_null() && !p.is_null() {
            alloc::free(p.cast());
        }
        np.cast()
    }
}

/// `mi_heap_strdup`.
///
/// # Safety
/// As [`mi_heap_malloc`]; `s` as [`mi_strdup`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_strdup(
    heap: *mut MiHeap,
    s: *const core::ffi::c_char,
) -> *mut core::ffi::c_char {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: forwarded C contracts.
    unsafe {
        let len = core::ffi::CStr::from_ptr(s).to_bytes().len();
        let p = alloc::heap_malloc(heap, len + 1);
        if !p.is_null() {
            core::ptr::copy_nonoverlapping(s.cast::<u8>(), p, len + 1);
        }
        p.cast()
    }
}

/// `mi_heap_strndup`.
///
/// # Safety
/// As [`mi_heap_malloc`]; `s` as [`mi_strndup`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_strndup(
    heap: *mut MiHeap,
    s: *const core::ffi::c_char,
    n: usize,
) -> *mut core::ffi::c_char {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    let mut len = 0usize;
    // SAFETY: reads within the C contract's readable range.
    while len < n && unsafe { *s.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: forwarded C contracts.
    unsafe {
        let p = alloc::heap_malloc(heap, len + 1);
        if !p.is_null() {
            core::ptr::copy_nonoverlapping(s.cast::<u8>(), p, len);
            p.add(len).write(0);
        }
        p.cast()
    }
}

/// `mi_heap_malloc_aligned`.
///
/// # Safety
/// As [`mi_heap_malloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_malloc_aligned(
    heap: *mut MiHeap,
    size: usize,
    alignment: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::heap_malloc_aligned_at(heap, size, alignment, 0).cast() }
}

/// `mi_heap_malloc_aligned_at`.
///
/// # Safety
/// As [`mi_heap_malloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_malloc_aligned_at(
    heap: *mut MiHeap,
    size: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::heap_malloc_aligned_at(heap, size, alignment, offset).cast() }
}

/// `mi_heap_zalloc_aligned`.
///
/// # Safety
/// As [`mi_heap_malloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_zalloc_aligned(
    heap: *mut MiHeap,
    size: usize,
    alignment: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::heap_zalloc_aligned_at(heap, size, alignment, 0).cast() }
}

/// `mi_heap_zalloc_aligned_at`.
///
/// # Safety
/// As [`mi_heap_malloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_zalloc_aligned_at(
    heap: *mut MiHeap,
    size: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { alloc::heap_zalloc_aligned_at(heap, size, alignment, offset).cast() }
}

/// `mi_heap_calloc_aligned`.
///
/// # Safety
/// As [`mi_heap_malloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_calloc_aligned(
    heap: *mut MiHeap,
    count: usize,
    size: usize,
    alignment: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { mi_heap_calloc_aligned_at(heap, count, size, alignment, 0) }
}

/// `mi_heap_calloc_aligned_at`.
///
/// # Safety
/// As [`mi_heap_malloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_calloc_aligned_at(
    heap: *mut MiHeap,
    count: usize,
    size: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    match count.checked_mul(size) {
        // SAFETY: forwarded C contract.
        Some(total) => unsafe {
            alloc::heap_zalloc_aligned_at(heap, total, alignment, offset).cast()
        },
        None => core::ptr::null_mut(),
    }
}

/// `mi_heap_realloc_aligned` (+`_at` below): alignment-preserving realloc on
/// a heap.
///
/// # Safety
/// As [`mi_heap_realloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_realloc_aligned(
    heap: *mut MiHeap,
    p: *mut c_void,
    newsize: usize,
    alignment: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { mi_heap_realloc_aligned_at(heap, p, newsize, alignment, 0) }
}

/// `mi_heap_realloc_aligned_at`.
///
/// # Safety
/// As [`mi_heap_realloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_realloc_aligned_at(
    heap: *mut MiHeap,
    p: *mut c_void,
    newsize: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contracts (aligned move through the heap).
    unsafe {
        if p.is_null() {
            return alloc::heap_malloc_aligned_at(heap, newsize, alignment, offset).cast();
        }
        let usable = alloc::usable_size(p.cast());
        if newsize <= usable
            && newsize >= usable / 2
            && (p as usize + offset).is_multiple_of(alignment.max(1))
        {
            return p;
        }
        let np = alloc::heap_malloc_aligned_at(heap, newsize, alignment, offset);
        if np.is_null() {
            return core::ptr::null_mut();
        }
        core::ptr::copy_nonoverlapping(p.cast::<u8>(), np, usable.min(newsize));
        alloc::free(p.cast());
        np.cast()
    }
}

/// `mi_heap_rezalloc` family — zero-preserving, heap-relative.
///
/// # Safety
/// As [`mi_heap_realloc`] + zalloc lineage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_rezalloc(
    heap: *mut MiHeap,
    p: *mut c_void,
    newsize: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { mi_heap_rezalloc_aligned_at(heap, p, newsize, 1, 0) }
}

/// `mi_heap_recalloc`.
///
/// # Safety
/// As [`mi_heap_rezalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_recalloc(
    heap: *mut MiHeap,
    p: *mut c_void,
    newcount: usize,
    size: usize,
) -> *mut c_void {
    match newcount.checked_mul(size) {
        // SAFETY: forwarded C contract.
        Some(total) => unsafe { mi_heap_rezalloc(heap, p, total) },
        None => core::ptr::null_mut(),
    }
}

/// `mi_heap_rezalloc_aligned`.
///
/// # Safety
/// As [`mi_heap_rezalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_rezalloc_aligned(
    heap: *mut MiHeap,
    p: *mut c_void,
    newsize: usize,
    alignment: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contract.
    unsafe { mi_heap_rezalloc_aligned_at(heap, p, newsize, alignment, 0) }
}

/// `mi_heap_rezalloc_aligned_at`.
///
/// # Safety
/// As [`mi_heap_rezalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_rezalloc_aligned_at(
    heap: *mut MiHeap,
    p: *mut c_void,
    newsize: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    // SAFETY: forwarded C contracts (zero-preserving, heap-relative).
    unsafe {
        if p.is_null() {
            return if alignment <= 1 {
                alloc::heap_zalloc(heap, newsize).cast()
            } else {
                alloc::heap_zalloc_aligned_at(heap, newsize, alignment, offset).cast()
            };
        }
        let usable = alloc::usable_size(p.cast());
        if newsize <= usable
            && newsize >= usable / 2
            && (p as usize + offset).is_multiple_of(alignment.max(1))
        {
            return p;
        }
        let np = if alignment <= 1 {
            alloc::heap_malloc(heap, newsize)
        } else {
            alloc::heap_malloc_aligned_at(heap, newsize, alignment, offset)
        };
        if np.is_null() {
            return core::ptr::null_mut();
        }
        let keep = usable.min(newsize);
        core::ptr::copy_nonoverlapping(p.cast::<u8>(), np, keep);
        let new_usable = alloc::usable_size(np);
        core::ptr::write_bytes(np.add(keep), 0, new_usable - keep);
        alloc::free(p.cast());
        np.cast()
    }
}

/// `mi_heap_recalloc_aligned`.
///
/// # Safety
/// As [`mi_heap_rezalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_recalloc_aligned(
    heap: *mut MiHeap,
    p: *mut c_void,
    newcount: usize,
    size: usize,
    alignment: usize,
) -> *mut c_void {
    match newcount.checked_mul(size) {
        // SAFETY: forwarded C contract.
        Some(total) => unsafe { mi_heap_rezalloc_aligned(heap, p, total, alignment) },
        None => core::ptr::null_mut(),
    }
}

/// `mi_heap_recalloc_aligned_at`.
///
/// # Safety
/// As [`mi_heap_rezalloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_recalloc_aligned_at(
    heap: *mut MiHeap,
    p: *mut c_void,
    newcount: usize,
    size: usize,
    alignment: usize,
    offset: usize,
) -> *mut c_void {
    match newcount.checked_mul(size) {
        // SAFETY: forwarded C contract.
        Some(total) => unsafe { mi_heap_rezalloc_aligned_at(heap, p, total, alignment, offset) },
        None => core::ptr::null_mut(),
    }
}

/// `mi_heap_alloc_new` / `_n` (C++ new semantics on a heap: abort on OOM —
/// documented divergence: no `std::get_new_handler` integration).
///
/// # Safety
/// As [`mi_heap_malloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_alloc_new(heap: *mut MiHeap, size: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    let p = unsafe { alloc::heap_malloc(heap, size) };
    if p.is_null() {
        rusty_alloc::options::error(12);
        std::process::abort();
    }
    p.cast()
}

/// `mi_heap_alloc_new_n`.
///
/// # Safety
/// As [`mi_heap_alloc_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_alloc_new_n(
    heap: *mut MiHeap,
    count: usize,
    size: usize,
) -> *mut c_void {
    match count.checked_mul(size) {
        // SAFETY: forwarded C contract.
        Some(total) => unsafe { mi_heap_alloc_new(heap, total) },
        None => {
            rusty_alloc::options::error(12);
            std::process::abort();
        }
    }
}

/// `mi_heap_contains_block`.
///
/// # Safety
/// `heap` live and owned by the calling thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_contains_block(heap: *mut MiHeap, p: *const c_void) -> bool {
    // SAFETY: forwarded C contract.
    unsafe { alloc::heap_contains_block(heap, p.cast()) }
}

/// `mi_heap_check_owned`.
///
/// # Safety
/// As [`mi_heap_contains_block`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_check_owned(heap: *mut MiHeap, p: *const c_void) -> bool {
    // SAFETY: forwarded C contract.
    unsafe { alloc::heap_check_owned(heap, p.cast()) }
}

/// `mi_check_owned`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_check_owned(p: *const c_void) -> bool {
    alloc::check_owned(p.cast())
}

/// `mi_heap_area_t` (ABI-compatible with the oracle header).
#[repr(C)]
pub struct MiHeapArea {
    /// Start of the area.
    pub blocks: *mut c_void,
    /// Bytes reserved.
    pub reserved: usize,
    /// Bytes committed.
    pub committed: usize,
    /// Allocated blocks.
    pub used: usize,
    /// Block size.
    pub block_size: usize,
    /// Block size incl. padding/metadata.
    pub full_block_size: usize,
    /// Heap tag.
    pub heap_tag: c_int,
}

/// `mi_block_visit_fun`.
pub type MiBlockVisitFun = unsafe extern "C" fn(
    heap: *const MiHeap,
    area: *const MiHeapArea,
    block: *mut c_void,
    block_size: usize,
    arg: *mut c_void,
) -> bool;

fn area_to_c(a: &rusty_alloc::heap::AreaInfo) -> MiHeapArea {
    MiHeapArea {
        blocks: a.blocks.cast(),
        reserved: a.reserved,
        committed: a.committed,
        used: a.used,
        block_size: a.block_size,
        full_block_size: a.full_block_size,
        heap_tag: a.heap_tag,
    }
}

/// `mi_heap_visit_blocks`.
///
/// # Safety
/// `heap` live/owned by caller; `visitor` a valid function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_visit_blocks(
    heap: *const MiHeap,
    visit_blocks: bool,
    visitor: Option<MiBlockVisitFun>,
    arg: *mut c_void,
) -> bool {
    let Some(vf) = visitor else { return true };
    // SAFETY: forwarded C contracts; adapter re-wraps areas per call.
    unsafe {
        let h = (*heap.cast_mut()).heap.get();
        (*h).visit_blocks(visit_blocks, &mut |area, block, bsize| {
            let ca = area_to_c(area);
            vf(heap, &ca, block.cast(), bsize, arg)
        })
    }
}

/// `mi_abandoned_visit_blocks`.
///
/// # Safety
/// `visitor` a valid function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_abandoned_visit_blocks(
    subproc_id: *mut c_void,
    heap_tag: c_int,
    visit_blocks: bool,
    visitor: Option<MiBlockVisitFun>,
    arg: *mut c_void,
) -> bool {
    let Some(vf) = visitor else { return true };
    rusty_alloc::init::abandoned_visit_blocks(
        subproc_id as usize,
        heap_tag,
        visit_blocks,
        &mut |area, block, bsize| {
            let ca = area_to_c(area);
            // SAFETY: forwarded C contract (null heap: blocks are ownerless).
            unsafe { vf(core::ptr::null(), &ca, block.cast(), bsize, arg) }
        },
    )
}

/// `mi_unsafe_heap_page_is_under_utilized`.
///
/// # Safety
/// Owner thread; `p` live pointer of this heap (the "unsafe" in the name).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_unsafe_heap_page_is_under_utilized(
    heap: *mut MiHeap,
    p: *mut c_void,
    perc_threshold: usize,
) -> bool {
    // SAFETY: forwarded C contract.
    unsafe { (*(*heap).heap.get()).page_under_utilized(p.cast(), perc_threshold) }
}

/// `mi_reserve_os_memory` / `_ex`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_reserve_os_memory(size: usize, commit: bool, allow_large: bool) -> c_int {
    match rusty_alloc::arena::reserve_os_memory_ex(size, commit, allow_large, false) {
        Ok(_) => 0,
        Err(()) => 12, // ENOMEM
    }
}

/// `mi_reserve_os_memory_ex`.
///
/// # Safety
/// `arena_id` null or valid to write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_reserve_os_memory_ex(
    size: usize,
    commit: bool,
    allow_large: bool,
    exclusive: bool,
    arena_id: *mut c_int,
) -> c_int {
    match rusty_alloc::arena::reserve_os_memory_ex(size, commit, allow_large, exclusive) {
        Ok(id) => {
            if !arena_id.is_null() {
                // SAFETY: out-pointer valid per contract.
                unsafe { arena_id.write(id) };
            }
            0
        }
        Err(()) => 12,
    }
}

/// `mi_manage_os_memory` / `_ex`.
///
/// # Safety
/// `start..start+size` must be valid committed memory owned by the caller for
/// the process lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_manage_os_memory(
    start: *mut c_void,
    size: usize,
    is_committed: bool,
    is_large: bool,
    is_zero: bool,
    numa_node: c_int,
) -> bool {
    rusty_alloc::arena::manage_os_memory_ex(
        start.cast(),
        size,
        is_committed,
        is_large,
        is_zero,
        numa_node,
        false,
    )
    .is_ok()
}

/// `mi_manage_os_memory_ex`.
///
/// # Safety
/// As [`mi_manage_os_memory`]; `arena_id` null or valid to write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_manage_os_memory_ex(
    start: *mut c_void,
    size: usize,
    is_committed: bool,
    is_large: bool,
    is_zero: bool,
    numa_node: c_int,
    exclusive: bool,
    arena_id: *mut c_int,
) -> bool {
    match rusty_alloc::arena::manage_os_memory_ex(
        start.cast(),
        size,
        is_committed,
        is_large,
        is_zero,
        numa_node,
        exclusive,
    ) {
        Ok(id) => {
            if !arena_id.is_null() {
                // SAFETY: out-pointer valid per contract.
                unsafe { arena_id.write(id) };
            }
            true
        }
        Err(()) => false,
    }
}

/// `mi_arena_area`.
///
/// # Safety
/// `size` null or valid to write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_arena_area(arena_id: c_int, size: *mut usize) -> *mut c_void {
    let (p, s) = rusty_alloc::arena::arena_area(arena_id);
    if !size.is_null() {
        // SAFETY: out-pointer valid per contract.
        unsafe { size.write(s) };
    }
    p.cast()
}

/// `mi_reserve_huge_os_pages_at` (+ `_ex`, `_interleave`, deprecated form):
/// large-page arena reservations; NUMA placement recorded, not enforced (v1).
#[unsafe(no_mangle)]
pub extern "C" fn mi_reserve_huge_os_pages_at(
    pages: usize,
    _numa_node: c_int,
    _timeout_msecs: usize,
) -> c_int {
    mi_reserve_os_memory(pages * (1 << 30), true, true)
}

/// `mi_reserve_huge_os_pages_at_ex`.
///
/// # Safety
/// `arena_id` null or valid to write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_reserve_huge_os_pages_at_ex(
    pages: usize,
    _numa_node: c_int,
    _timeout_msecs: usize,
    exclusive: bool,
    arena_id: *mut c_int,
) -> c_int {
    // SAFETY: forwarded out-pointer contract.
    unsafe { mi_reserve_os_memory_ex(pages * (1 << 30), true, true, exclusive, arena_id) }
}

/// `mi_reserve_huge_os_pages_interleave`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_reserve_huge_os_pages_interleave(
    pages: usize,
    _numa_nodes: usize,
    _timeout: usize,
) -> c_int {
    mi_reserve_os_memory(pages * (1 << 30), true, true)
}

/// deprecated `mi_reserve_huge_os_pages`.
///
/// # Safety
/// `pages_reserved` null or valid to write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_reserve_huge_os_pages(
    pages: usize,
    _max_secs: f64,
    pages_reserved: *mut usize,
) -> c_int {
    let r = mi_reserve_os_memory(pages * (1 << 30), true, true);
    if !pages_reserved.is_null() {
        // SAFETY: out-pointer valid per contract.
        unsafe { pages_reserved.write(if r == 0 { pages } else { 0 }) };
    }
    r
}

/// `mi_debug_show_arenas` / `mi_arenas_print`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_debug_show_arenas() {
    rusty_alloc::arena::arenas_print(&mut |s| rusty_alloc::options::out_fmt(s));
}

/// `mi_arenas_print`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_arenas_print() {
    mi_debug_show_arenas();
}

/// deprecated `mi_collect_reduce`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_collect_reduce(_target_thread_owned: usize) {
    alloc::collect(false);
}

/// `mi_subproc_main` / `new` / `delete` / `add_current_thread`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_subproc_main() -> *mut c_void {
    rusty_alloc::init::subproc_main() as *mut c_void
}

/// `mi_subproc_new`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_subproc_new() -> *mut c_void {
    rusty_alloc::init::subproc_new() as *mut c_void
}

/// `mi_subproc_delete`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_subproc_delete(subproc: *mut c_void) {
    rusty_alloc::init::subproc_delete(subproc as usize);
}

/// `mi_subproc_add_current_thread`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_subproc_add_current_thread(subproc: *mut c_void) {
    rusty_alloc::init::subproc_add_current_thread(subproc as usize);
}

// ===========================================================================
// M7: options (§5.10), stats + hooks (§5.3), misc (§5.11 tail, §5.12)
// ===========================================================================

/// `mi_option_is_enabled`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_option_is_enabled(option: c_int) -> bool {
    rusty_alloc::options::is_enabled(option.max(0) as usize)
}

/// `mi_option_enable`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_option_enable(option: c_int) {
    rusty_alloc::options::set(option.max(0) as usize, 1);
}

/// `mi_option_disable`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_option_disable(option: c_int) {
    rusty_alloc::options::set(option.max(0) as usize, 0);
}

/// `mi_option_set_enabled`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_option_set_enabled(option: c_int, enable: bool) {
    rusty_alloc::options::set(option.max(0) as usize, enable as i64);
}

/// `mi_option_set_enabled_default`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_option_set_enabled_default(option: c_int, enable: bool) {
    rusty_alloc::options::set_default(option.max(0) as usize, enable as i64);
}

/// `mi_option_get`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_option_get(option: c_int) -> core::ffi::c_long {
    rusty_alloc::options::get(option.max(0) as usize) as core::ffi::c_long
}

/// `mi_option_get_clamp`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_option_get_clamp(
    option: c_int,
    min: core::ffi::c_long,
    max: core::ffi::c_long,
) -> core::ffi::c_long {
    rusty_alloc::options::get_clamp(option.max(0) as usize, min as i64, max as i64)
        as core::ffi::c_long
}

/// `mi_option_get_size`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_option_get_size(option: c_int) -> usize {
    rusty_alloc::options::get_size(option.max(0) as usize)
}

/// `mi_option_set`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_option_set(option: c_int, value: core::ffi::c_long) {
    rusty_alloc::options::set(option.max(0) as usize, value as i64);
}

/// `mi_option_set_default`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_option_set_default(option: c_int, value: core::ffi::c_long) {
    rusty_alloc::options::set_default(option.max(0) as usize, value as i64);
}

/// `mi_options_print`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_options_print() {
    rusty_alloc::options::print();
}

/// `mi_register_output`.
///
/// # Safety
/// `out` null or a function of the documented signature, valid for the
/// process lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_register_output(
    out: Option<rusty_alloc::options::OutputFun>,
    arg: *mut c_void,
) {
    rusty_alloc::options::register_output(out, arg);
}

/// `mi_register_error`.
///
/// # Safety
/// As [`mi_register_output`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_register_error(
    fun: Option<rusty_alloc::options::ErrorFun>,
    arg: *mut c_void,
) {
    rusty_alloc::options::register_error(fun, arg);
}

/// `mi_register_deferred_free`.
///
/// # Safety
/// As [`mi_register_output`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_register_deferred_free(
    deferred_free: Option<rusty_alloc::options::DeferredFreeFun>,
    arg: *mut c_void,
) {
    rusty_alloc::options::register_deferred_free(deferred_free, arg);
}

/// `mi_stats_print` (arg ignored, per the header's compat note).
#[unsafe(no_mangle)]
pub extern "C" fn mi_stats_print(_out: *mut c_void) {
    rusty_alloc::stats::print_process();
}

/// `mi_stats_print_out`.
///
/// # Safety
/// As [`mi_register_output`] (hook installed for the duration of the call).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_stats_print_out(
    out: Option<rusty_alloc::options::OutputFun>,
    arg: *mut c_void,
) {
    if out.is_some() {
        rusty_alloc::options::register_output(out, arg);
    }
    rusty_alloc::stats::print_process();
}

/// `mi_thread_stats_print_out`.
///
/// # Safety
/// As [`mi_stats_print_out`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_thread_stats_print_out(
    out: Option<rusty_alloc::options::OutputFun>,
    arg: *mut c_void,
) {
    if out.is_some() {
        rusty_alloc::options::register_output(out, arg);
    }
    rusty_alloc::stats::print_thread();
}

/// `mi_stats_reset`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_stats_reset() {
    rusty_alloc::stats::reset();
}

/// `mi_stats_merge` (our counters are per-heap and merged on read — no-op).
#[unsafe(no_mangle)]
pub extern "C" fn mi_stats_merge() {}

/// `mi_process_info`.
///
/// # Safety
/// Every out-pointer null or valid to write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_process_info(
    elapsed_msecs: *mut usize,
    user_msecs: *mut usize,
    system_msecs: *mut usize,
    current_rss: *mut usize,
    peak_rss: *mut usize,
    current_commit: *mut usize,
    peak_commit: *mut usize,
    page_faults: *mut usize,
) {
    let (e, u, s, rss, prss, c, pc, f) = rusty_alloc::stats::process_info();
    // SAFETY: each out-pointer valid-or-null per contract.
    unsafe {
        let w = |p: *mut usize, v: usize| {
            if !p.is_null() {
                p.write(v)
            }
        };
        w(elapsed_msecs, e);
        w(user_msecs, u);
        w(system_msecs, s);
        w(current_rss, rss);
        w(peak_rss, prss);
        w(current_commit, c);
        w(peak_commit, pc);
        w(page_faults, f);
    }
}

/// `mi_heap_guarded_set_sample_rate`: guard 1-in-N eligible objects.
///
/// # Safety
/// `heap` live and owned by the calling thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_guarded_set_sample_rate(
    heap: *mut MiHeap,
    sample_rate: usize,
    seed: usize,
) {
    // SAFETY: forwarded C contract.
    unsafe { (*(*heap).heap.get()).guarded_set_sample_rate(sample_rate, seed) };
}

/// `mi_heap_guarded_set_size_bound`.
///
/// # Safety
/// `heap` live and owned by the calling thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_heap_guarded_set_size_bound(heap: *mut MiHeap, min: usize, max: usize) {
    // SAFETY: forwarded C contract.
    unsafe { (*(*heap).heap.get()).guarded_set_size_bound(min, max) };
}

/// `mi_is_redirected` (Windows redirection is post-v1).
#[unsafe(no_mangle)]
pub extern "C" fn mi_is_redirected() -> bool {
    false
}

/// `mi_new` family — C++ semantics: abort on final failure (documented
/// divergence: no `std::get_new_handler` loop).
#[unsafe(no_mangle)]
pub extern "C" fn mi_new(size: usize) -> *mut c_void {
    let p = alloc::malloc(size);
    if p.is_null() {
        alloc::collect(true);
        let p2 = alloc::malloc(size);
        if p2.is_null() {
            rusty_alloc::options::error(12);
            std::process::abort();
        }
        return p2.cast();
    }
    p.cast()
}

/// `mi_new_nothrow`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_new_nothrow(size: usize) -> *mut c_void {
    alloc::malloc(size).cast()
}

/// `mi_new_aligned`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_new_aligned(size: usize, alignment: usize) -> *mut c_void {
    let p = alloc::malloc_aligned(size, alignment);
    if p.is_null() {
        rusty_alloc::options::error(12);
        std::process::abort();
    }
    p.cast()
}

/// `mi_new_aligned_nothrow`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_new_aligned_nothrow(size: usize, alignment: usize) -> *mut c_void {
    alloc::malloc_aligned(size, alignment).cast()
}

/// `mi_new_n`.
#[unsafe(no_mangle)]
pub extern "C" fn mi_new_n(count: usize, size: usize) -> *mut c_void {
    match count.checked_mul(size) {
        Some(total) => mi_new(total),
        None => {
            rusty_alloc::options::error(12);
            std::process::abort();
        }
    }
}

/// `mi_new_realloc`.
///
/// # Safety
/// As [`mi_realloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_new_realloc(p: *mut c_void, newsize: usize) -> *mut c_void {
    // SAFETY: forwarded C contract.
    let np = unsafe { alloc::realloc(p.cast(), newsize) };
    if np.is_null() {
        rusty_alloc::options::error(12);
        std::process::abort();
    }
    np.cast()
}

/// `mi_new_reallocn`.
///
/// # Safety
/// As [`mi_realloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_new_reallocn(
    p: *mut c_void,
    newcount: usize,
    size: usize,
) -> *mut c_void {
    match newcount.checked_mul(size) {
        // SAFETY: forwarded C contract.
        Some(total) => unsafe { mi_new_realloc(p, total) },
        None => {
            rusty_alloc::options::error(12);
            std::process::abort();
        }
    }
}

/// `mi_realpath`: resolve to an absolute canonical path into allocated (or
/// caller-provided) storage.
///
/// # Safety
/// `fname` a valid NUL-terminated path; `resolved_name` null or a buffer of
/// PATH_MAX bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_realpath(
    fname: *const core::ffi::c_char,
    resolved_name: *mut core::ffi::c_char,
) -> *mut core::ffi::c_char {
    if fname.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: fname NUL-terminated per contract.
    let path = match unsafe { core::ffi::CStr::from_ptr(fname) }.to_str() {
        Ok(s) => s,
        Err(_) => return core::ptr::null_mut(),
    };
    let canon = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => return core::ptr::null_mut(),
    };
    let bytes = canon.to_string_lossy().into_owned().into_bytes();
    let out = if resolved_name.is_null() {
        alloc::malloc(bytes.len() + 1)
    } else {
        resolved_name.cast()
    };
    if out.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: out has ≥ len+1 writable bytes (allocated here, or PATH_MAX per
    // the C contract).
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
        out.add(bytes.len()).write(0);
    }
    out.cast()
}

/// `mi_dupenv_s` (MSVC shape): duplicate an env var into allocated storage.
///
/// # Safety
/// `buf`/`size` valid to write; `name` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_dupenv_s(
    buf: *mut *mut core::ffi::c_char,
    size: *mut usize,
    name: *const core::ffi::c_char,
) -> c_int {
    if buf.is_null() || name.is_null() {
        return 22;
    }
    // SAFETY: name NUL-terminated per contract; out-pointers valid.
    unsafe {
        buf.write(core::ptr::null_mut());
        if !size.is_null() {
            size.write(0);
        }
        let Ok(n) = core::ffi::CStr::from_ptr(name).to_str() else {
            return 22;
        };
        match std::env::var(n) {
            Ok(v) => {
                let b = v.into_bytes();
                let p = alloc::malloc(b.len() + 1);
                if p.is_null() {
                    return 12;
                }
                core::ptr::copy_nonoverlapping(b.as_ptr(), p, b.len());
                p.add(b.len()).write(0);
                buf.write(p.cast());
                if !size.is_null() {
                    size.write(b.len() + 1);
                }
                0
            }
            Err(_) => 0, // absent: *buf null, success (MSVC semantics)
        }
    }
}

/// `mi_wcsdup`.
///
/// # Safety
/// `s` null or NUL-terminated wide string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_wcsdup(s: *const u16) -> *mut u16 {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    let mut len = 0usize;
    // SAFETY: NUL-terminated per contract.
    while unsafe { *s.add(len) } != 0 {
        len += 1;
    }
    let p = alloc::malloc((len + 1) * 2).cast::<u16>();
    if !p.is_null() {
        // SAFETY: p has (len+1)*2 usable bytes.
        unsafe { core::ptr::copy_nonoverlapping(s, p, len + 1) };
    }
    p
}

/// `mi_mbsdup`.
///
/// # Safety
/// As [`mi_strdup`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_mbsdup(s: *const u8) -> *mut u8 {
    // SAFETY: forwarded contract (multibyte == byte string for dup purposes).
    unsafe { mi_strdup(s.cast()).cast() }
}

/// `mi_wdupenv_s` — wide variant.
///
/// # Safety
/// As [`mi_dupenv_s`] with wide strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mi_wdupenv_s(
    buf: *mut *mut u16,
    size: *mut usize,
    name: *const u16,
) -> c_int {
    if buf.is_null() || name.is_null() {
        return 22;
    }
    // SAFETY: contracts as documented.
    unsafe {
        buf.write(core::ptr::null_mut());
        if !size.is_null() {
            size.write(0);
        }
        let mut len = 0usize;
        while *name.add(len) != 0 {
            len += 1;
        }
        let n = String::from_utf16_lossy(core::slice::from_raw_parts(name, len));
        match std::env::var(&n) {
            Ok(v) => {
                let w: Vec<u16> = v.encode_utf16().chain(core::iter::once(0)).collect();
                let p = alloc::malloc(w.len() * 2).cast::<u16>();
                if p.is_null() {
                    return 12;
                }
                core::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                buf.write(p);
                if !size.is_null() {
                    size.write(w.len());
                }
                0
            }
            Err(_) => 0,
        }
    }
}
