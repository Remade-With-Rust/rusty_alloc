//! Openheimer regressions for the C ABI crate (OH-14, OH-16).

use std::panic::{AssertUnwindSafe, catch_unwind};

use rusty_alloc_ffi::{
    mi_arena_area, mi_dupenv_s, mi_free, mi_heap_get_default, mi_heap_guarded_set_sample_rate,
    mi_heap_guarded_set_size_bound, mi_heap_realloc_aligned_at, mi_heap_rezalloc_aligned_at,
    mi_heap_strdup, mi_heap_strndup, mi_heap_visit_blocks, mi_heap_zalloc, mi_malloc, mi_mbsdup,
    mi_posix_memalign, mi_process_info, mi_reallocarr, mi_realpath, mi_reserve_huge_os_pages,
    mi_reserve_huge_os_pages_at, mi_reserve_huge_os_pages_interleave, mi_strdup, mi_strndup,
    mi_umalloc, mi_unsafe_heap_page_is_under_utilized, mi_usable_size, mi_wcsdup, mi_wdupenv_s,
};

/// F-04: `pages * 1GiB` must not overflow (OH-rusty_alloc-14).
#[test]
fn oh_f04_reserve_huge_os_pages_overflow_is_enomem() {
    let r = catch_unwind(|| mi_reserve_huge_os_pages_at(usize::MAX, -1, 0));
    assert!(r.is_ok(), "mi_reserve_huge_os_pages_at(MAX) panicked");
    assert_eq!(r.unwrap(), 12);
    assert_eq!(mi_reserve_huge_os_pages_interleave(usize::MAX, 0, 0), 12);
    // SAFETY: the out-pointer is a live local.
    unsafe {
        let mut reserved = 0x1111_usize;
        assert_eq!(mi_reserve_huge_os_pages(usize::MAX, 0.0, &mut reserved), 12);
        assert_eq!(reserved, 0);
    }
}

/// F-01: EINVAL on `dupenv` clears the caller's out-pointers (OH-rusty_alloc-16).
#[test]
fn oh_f01_dupenv_einval_clears_outs() {
    // SAFETY: both out-pointers are live locals; the name is null, which the
    // entry must refuse with EINVAL before reading anything.
    unsafe {
        let mut buf: *mut i8 = core::ptr::NonNull::<i8>::dangling().as_ptr();
        let mut sz = 99usize;
        assert_eq!(mi_dupenv_s(&mut buf, &mut sz, core::ptr::null()), 22);
        assert!(buf.is_null());
        assert_eq!(sz, 0);
        let mut wbuf: *mut u16 = core::ptr::NonNull::<u16>::dangling().as_ptr();
        sz = 99;
        assert_eq!(mi_wdupenv_s(&mut wbuf, &mut sz, core::ptr::null()), 22);
        assert!(wbuf.is_null());
        assert_eq!(sz, 0);
    }
}

/// F-05: `mi_realpath` must not write past PATH_MAX (OH-rusty_alloc-26).
#[test]
fn oh_f05_realpath_does_not_write_past_path_max() {
    const PATH_MAX: usize = if cfg!(windows) { 260 } else { 4096 };
    let mut buf = vec![0u8; PATH_MAX + 16];
    buf[PATH_MAX..].fill(0xA5);
    // SAFETY: a NUL-terminated path and a buffer of more than PATH_MAX bytes,
    // whose tail is checked afterwards.
    unsafe {
        let _ = mi_realpath(c".".as_ptr(), buf.as_mut_ptr().cast());
    }
    assert!(
        buf[PATH_MAX..].iter().all(|&b| b == 0xA5),
        "realpath wrote past PATH_MAX"
    );
}

/// F-01 / F-04: heap-relative in-place realloc huge offset is null; original
/// lives (OH-rusty_alloc-44). extern "C" cannot unwind, so this must not panic.
#[test]
fn oh_f04_heap_realloc_inplace_huge_offset_is_null() {
    let h = mi_heap_get_default();
    // SAFETY: `h` is this thread's live heap; `p` is a live block of it that
    // the refused realloc leaves live, freed once.
    unsafe {
        let p = mi_malloc(64);
        assert!(!p.is_null());
        p.cast::<u8>().write(0x43);
        let u = mi_usable_size(p);
        let np = mi_heap_realloc_aligned_at(h, p, u, 8, usize::MAX);
        assert!(np.is_null());
        assert_eq!(p.cast::<u8>().read(), 0x43);
        mi_free(p);
    }
}

/// F-01 / F-04: heap-relative in-place rezalloc huge offset is null
/// (OH-rusty_alloc-45).
#[test]
fn oh_f04_heap_rezalloc_inplace_huge_offset_is_null() {
    let h = mi_heap_get_default();
    // SAFETY: as the realloc twin above.
    unsafe {
        let p = mi_heap_zalloc(h, 64);
        assert!(!p.is_null());
        let u = mi_usable_size(p);
        let np = mi_heap_rezalloc_aligned_at(h, p, u, 8, usize::MAX);
        assert!(np.is_null());
        mi_free(p);
    }
}

/// F-01: `mi_heap_visit_blocks(null)` is true, not an abort
/// (OH-rusty_alloc-58). extern "C" cannot unwind.
#[test]
fn oh_f01_heap_visit_blocks_null_is_total() {
    unsafe extern "C" fn visit_ok(
        _h: *const rusty_alloc_ffi::MiHeap,
        _a: *const rusty_alloc_ffi::MiHeapArea,
        _b: *mut core::ffi::c_void,
        _n: usize,
        _arg: *mut core::ffi::c_void,
    ) -> bool {
        true
    }
    // SAFETY: every heap handle is null, which each entry must treat as "no
    // heap"; the visitor is a valid function that touches nothing.
    unsafe {
        assert!(mi_heap_visit_blocks(
            core::ptr::null(),
            true,
            Some(visit_ok),
            core::ptr::null_mut()
        ));
        assert!(!mi_unsafe_heap_page_is_under_utilized(
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            50
        ));
        mi_heap_guarded_set_sample_rate(core::ptr::null_mut(), 1, 0);
        mi_heap_guarded_set_size_bound(core::ptr::null_mut(), 0, 64);
    }
}

/// F-01: a null or misaligned out-parameter is refused, not written
/// (OH-rusty_alloc-97…102). extern "C" cannot unwind, so none of these may
/// panic either.
#[test]
fn oh_f01_ffi_out_param_null_or_misaligned_is_refused() {
    // `*mut *mut c_void` / `*mut usize` are 8-aligned on every target we
    // ship; an odd address is misaligned for all of them.
    let odd = 0x1001usize;
    // SAFETY: every out-pointer handed in is null or misaligned, which each
    // entry must refuse before writing; `p` is a block `mi_umalloc` returned,
    // freed once.
    unsafe {
        let r = catch_unwind(|| mi_posix_memalign(core::ptr::null_mut(), 16, 32));
        assert!(r.is_ok() && r.unwrap() == 22);
        let r = catch_unwind(|| mi_posix_memalign(odd as *mut _, 16, 32));
        assert!(r.is_ok() && r.unwrap() == 22);
        let r = catch_unwind(|| mi_reallocarr(core::ptr::null_mut(), 1, 8));
        assert!(r.is_ok() && r.unwrap() == 22);
        let r = catch_unwind(|| mi_reallocarr(odd as *mut _, 1, 8));
        assert!(r.is_ok() && r.unwrap() == 22);
        let r = catch_unwind(|| mi_umalloc(16, odd as *mut _));
        assert!(r.is_ok());
        let p = r.unwrap();
        assert!(!p.is_null());
        mi_free(p);
        let r = catch_unwind(|| mi_arena_area(0, odd as *mut _));
        assert!(r.is_ok());
        let r = catch_unwind(|| {
            mi_process_info(
                odd as *mut _,
                core::ptr::null_mut(),
                odd as *mut _,
                core::ptr::null_mut(),
                odd as *mut _,
                core::ptr::null_mut(),
                odd as *mut _,
                core::ptr::null_mut(),
            )
        });
        assert!(r.is_ok());
        let r = catch_unwind(|| mi_reserve_huge_os_pages(usize::MAX, 0.0, odd as *mut _));
        assert!(r.is_ok() && r.unwrap() == 12);
    }
}
/// F-01: a null C-string / path input is null / EINVAL, not a read through
/// null (OH-rusty_alloc-105…114). extern "C" cannot unwind.
#[test]
fn oh_f01_ffi_cstr_null_is_refused() {
    let h = mi_heap_get_default();
    // SAFETY: every string / path handed in is null or misaligned, which each
    // entry must refuse before reading; `h` is this thread's live heap; the
    // dupenv out-pointers are live locals.
    unsafe {
        let r = catch_unwind(|| mi_strdup(core::ptr::null()));
        assert!(r.is_ok() && r.unwrap().is_null());
        let r = catch_unwind(|| mi_strndup(core::ptr::null(), 16));
        assert!(r.is_ok() && r.unwrap().is_null());
        let r = catch_unwind(|| mi_wcsdup(core::ptr::null()));
        assert!(r.is_ok() && r.unwrap().is_null());
        let r = catch_unwind(|| mi_mbsdup(core::ptr::null()));
        assert!(r.is_ok() && r.unwrap().is_null());
        let r = catch_unwind(AssertUnwindSafe(|| mi_heap_strdup(h, core::ptr::null())));
        assert!(r.is_ok() && r.unwrap().is_null());
        let r = catch_unwind(AssertUnwindSafe(|| {
            mi_heap_strndup(h, core::ptr::null(), 16)
        }));
        assert!(r.is_ok() && r.unwrap().is_null());
        let r = catch_unwind(|| mi_realpath(core::ptr::null(), core::ptr::null_mut()));
        assert!(r.is_ok() && r.unwrap().is_null());
        let mut buf = core::ptr::null_mut();
        let mut size = 0usize;
        let r = catch_unwind(AssertUnwindSafe(|| {
            mi_dupenv_s(&mut buf, &mut size, core::ptr::null())
        }));
        assert!(r.is_ok() && r.unwrap() == 22);
        let mut wbuf = core::ptr::null_mut();
        let r = catch_unwind(AssertUnwindSafe(|| {
            mi_wdupenv_s(&mut wbuf, &mut size, core::ptr::null())
        }));
        assert!(r.is_ok() && r.unwrap() == 22);
        // A misaligned wide string is not readable input either.
        let r = catch_unwind(|| mi_wcsdup(0x1001usize as *const _));
        assert!(r.is_ok() && r.unwrap().is_null());
    }
}
