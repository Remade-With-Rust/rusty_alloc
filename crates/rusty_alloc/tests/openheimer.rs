//! Openheimer standing pack for `rusty_alloc` (campaign F-IDs).
//!
//! These are fail-closed assertions on the public allocation surface. They
//! exist so the campaign has one named file; several already lived in
//! `alloc_core.rs` / `double_free.rs` / `corruption.rs`. A duplicate here is
//! deliberate: if someone deletes the original, this pack still fails.
//!
//! Source of truth: `f:/coding/openheimer/docs/plans/openheimer.md` §5.6.
//! Campaign-only hostile cases (stale realloc, legal interior free, …) live
//! in the openheimer repo: `probes/rusty_alloc/tests/hostile.rs`.

use rusty_alloc::alloc::{
    calloc, calloc_aligned, free, malloc, malloc_aligned, malloc_aligned_at, mallocn, recalloc,
};
use rusty_alloc::bins::{self, is_aligned_to};
use rusty_alloc::init;

/// F-04: count×size overflow is null, never a wrapped small allocation.
#[test]
fn oh_f04_calloc_overflow_returns_null() {
    assert!(calloc(usize::MAX / 2, 3).is_null());
    assert!(mallocn(usize::MAX, 2).is_null());
    // SAFETY: null input is a documented no-op/overflow-null path (same as alloc_core).
    assert!(unsafe { recalloc(core::ptr::null_mut(), usize::MAX / 2, 3) }.is_null());
}

/// F-04: a non-power-of-two alignment is refused, not silently masked.
#[test]
fn oh_f04_non_power_of_two_align_returns_null() {
    assert!(malloc_aligned(64, 3).is_null());
    assert!(malloc_aligned(64, 0).is_null());
    assert!(malloc_aligned(64, 12).is_null());
}

/// F-01 / F-03: a size that does not wrap `count×size` but cannot be a
/// reservation (`malloc(usize::MAX)`) is null, not `header + size` overflow
/// in `huge_alloc` (OH-rusty_alloc-9).
#[test]
fn oh_f01_huge_nonwrapping_size_returns_null() {
    let r = std::panic::catch_unwind(|| malloc(usize::MAX));
    assert!(r.is_ok(), "malloc(usize::MAX) panicked in huge_alloc");
    assert!(r.unwrap().is_null());
    let c = std::panic::catch_unwind(|| calloc(usize::MAX, 1));
    assert!(c.is_ok(), "calloc(usize::MAX, 1) panicked in huge_alloc");
    assert!(c.unwrap().is_null());
}

/// F-01 / F-04: `good_size(usize::MAX)` must not panic (debug `page_align_up`
/// add) and must not wrap to 0 in release (OH-rusty_alloc-10).
#[test]
fn oh_f04_good_size_huge_does_not_shrink() {
    let r = std::panic::catch_unwind(|| rusty_alloc::bins::good_size(usize::MAX));
    assert!(r.is_ok(), "good_size(usize::MAX) panicked in page_align_up");
    let g = r.unwrap();
    assert_eq!(g, usize::MAX, "good_size(usize::MAX) shrank");
}

/// F-01: align 0 on a *warm* heap (free list non-empty) is still null, not a
/// debug `align - 1` overflow. A cold-heap probe short-circuits on a null
/// free-list head and never evaluates the mask (OH-rusty_alloc-8).
#[test]
fn oh_f01_align_zero_on_warm_heap_returns_null() {
    let p = malloc(64);
    assert!(!p.is_null());
    // SAFETY: `p` is live and ours; freed once, to warm the free list.
    unsafe { free(p) };
    let r = std::panic::catch_unwind(|| malloc_aligned(64, 0));
    assert!(r.is_ok(), "malloc_aligned(64, 0) panicked on a warm heap");
    assert!(r.unwrap().is_null());
    let c = std::panic::catch_unwind(|| calloc_aligned(4, 16, 0));
    assert!(c.is_ok(), "calloc_aligned(..., 0) panicked on a warm heap");
    assert!(c.unwrap().is_null());
}

/// F-01 / F-04: a huge aligned-at offset is null, not a debug overflow panic
/// and not a wrapped interior pointer (OH-rusty_alloc-6).
#[test]
fn oh_f04_aligned_at_huge_offset_returns_null() {
    assert!(malloc_aligned_at(16, 8, usize::MAX).is_null());
}

/// F-04: `is_aligned_to` is total — a non-power-of-two align is `false`,
/// never a `div` on a runtime divisor and never "aligned to 3".
#[test]
fn oh_f04_is_aligned_to_is_total() {
    assert!(
        !is_aligned_to(4, 3),
        "4 is not aligned to 3; a mask would lie"
    );
    assert!(!is_aligned_to(6, 3));
    assert!(!is_aligned_to(8, 0));
    assert!(is_aligned_to(8, 8));
    assert!(is_aligned_to(16, 8));
    assert!(!is_aligned_to(17, 8));
}

/// F-04: arena reserve of an unsatisfiable size is Err, not
/// `chunks * SEGMENT_SIZE` overflow (OH-rusty_alloc-12).
#[test]
fn oh_f04_arena_reserve_huge_is_err() {
    let r = std::panic::catch_unwind(|| {
        rusty_alloc::arena::reserve_os_memory_ex(usize::MAX, true, false, true)
    });
    assert!(r.is_ok(), "reserve_os_memory_ex(usize::MAX) panicked");
    assert!(r.unwrap().is_err(), "huge arena reserve succeeded");
}

/// F-04: adopting a null / overflowing OS range is Err, not a fake arena
/// covering the address space (OH-rusty_alloc-15).
#[test]
fn oh_f04_manage_os_memory_null_is_err() {
    assert!(
        rusty_alloc::arena::manage_os_memory_ex(
            core::ptr::null_mut(),
            usize::MAX,
            true,
            false,
            false,
            -1,
            false,
        )
        .is_err()
    );
    assert!(
        rusty_alloc::arena::manage_os_memory_ex(
            core::ptr::with_exposed_provenance_mut(usize::MAX - 8),
            64,
            true,
            false,
            false,
            -1,
            false,
        )
        .is_err()
    );
}

/// F-03: malloc(0) is a unique freeable pointer, not null (C contract this
/// crate documents). Complements overflow-null: zero is valid, wrap is not.
#[test]
fn oh_f03_malloc_zero_is_freeable_and_unique() {
    let a = malloc(0);
    let b = malloc(0);
    assert!(!a.is_null() && !b.is_null());
    assert_ne!(a, b);
    // SAFETY: both live, freed once.
    unsafe {
        rusty_alloc::alloc::free(a);
        rusty_alloc::alloc::free(b);
    }
}

/// F-01: inverted `get_clamp` bounds are swapped, not `i64::clamp`'s assert
/// (OH-rusty_alloc-18).
#[test]
fn oh_f01_option_get_clamp_inverted_is_total() {
    let r = std::panic::catch_unwind(|| rusty_alloc::options::get_clamp(0, 10, 1));
    assert!(r.is_ok(), "get_clamp(min>max) panicked");
    let c = r.unwrap();
    assert!((1..=10).contains(&c), "inverted clamp produced {c}");
}

/// F-04: KiB-scaled `get_size` must not wrap `v * 1024` (OH-rusty_alloc-19).
#[test]
fn oh_f04_option_get_size_huge_does_not_wrap() {
    let old = rusty_alloc::options::get(23);
    rusty_alloc::options::set(23, i64::MAX);
    let r = std::panic::catch_unwind(|| rusty_alloc::options::get_size(23));
    rusty_alloc::options::set(23, old);
    assert!(r.is_ok(), "get_size(MAX KiB) panicked");
    assert!(r.unwrap() >= 1024, "get_size wrapped to a small byte count");
}

/// F-01: `perc = usize::MAX` is a bool, not `capacity * perc` overflow
/// (OH-rusty_alloc-20).
#[test]
fn oh_f01_page_under_utilized_huge_perc_is_bool() {
    let p = malloc(64);
    assert!(!p.is_null());
    // SAFETY: `hb` is this thread's live heap box and `p` its live block.
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let hb = rusty_alloc::init::heap_box();
        (*(*hb).heap.get()).page_under_utilized(p, usize::MAX)
    }));
    // SAFETY: `p` is live and ours; freed once.
    unsafe { free(p) };
    assert!(r.is_ok(), "page_under_utilized(MAX perc) panicked");
}

/// F-01: a lying subprocess id is a no-op, not an assert (OH-rusty_alloc-21).
#[test]
fn oh_f01_subproc_add_oob_is_noop() {
    let r = std::panic::catch_unwind(|| rusty_alloc::init::subproc_add_current_thread(usize::MAX));
    assert!(r.is_ok(), "subproc_add_current_thread(MAX) panicked");
}

/// F-04: `chunk_alloc_n(..., 0)` is None, not a pointer with no bits claimed
/// (OH-rusty_alloc-22).
#[test]
fn oh_f04_chunk_alloc_n_zero_is_none() {
    let bytes = rusty_alloc::types::SEGMENT_SIZE.saturating_mul(2);
    let Ok(id) = rusty_alloc::arena::reserve_os_memory_ex(bytes, true, false, true) else {
        return;
    };
    assert!(rusty_alloc::arena::chunk_alloc_n(id, 1).is_some());
    assert!(rusty_alloc::arena::chunk_alloc_n(id, 0).is_none());
}

/// F-01: a deferred-free hook that allocates must not recurse until the stack
/// dies (OH-rusty_alloc-17).
#[test]
fn oh_f01_deferred_free_hook_that_mallocs_is_contained() {
    unsafe extern "C" fn hook(_force: bool, _hb: u64, _arg: *mut core::ffi::c_void) {
        let q = malloc(32);
        if !q.is_null() {
            // SAFETY: `q` was just allocated here; freed once.
            unsafe { free(q) };
        }
    }
    rusty_alloc::options::register_deferred_free(Some(hook), core::ptr::null_mut());
    let r = std::panic::catch_unwind(|| malloc(2 * 1024 * 1024));
    rusty_alloc::options::register_deferred_free(None, core::ptr::null_mut());
    assert!(r.is_ok(), "malloc with deferred-free hook panicked");
    if let Ok(p) = r
        && !p.is_null()
    {
        // SAFETY: `p` is the block `malloc` returned above; freed once.
        unsafe { free(p) };
    }
}

/// F-01: an error hook that calls `error` again must not overflow the stack
/// (OH-rusty_alloc-23).
#[test]
fn oh_f01_error_hook_reentry_is_contained() {
    unsafe extern "C" fn hook(err: i32, _arg: *mut core::ffi::c_void) {
        rusty_alloc::options::error(err);
    }
    rusty_alloc::options::register_error(Some(hook), core::ptr::null_mut());
    rusty_alloc::options::error(12);
    rusty_alloc::options::register_error(None, core::ptr::null_mut());
}

/// F-01: an output hook that calls `out_fmt` again must not overflow the stack
/// (OH-rusty_alloc-24).
#[test]
fn oh_f01_output_hook_reentry_is_contained() {
    unsafe extern "C" fn hook(_msg: *const i8, _arg: *mut core::ffi::c_void) {
        rusty_alloc::options::out_fmt("nested");
    }
    rusty_alloc::options::register_output(Some(hook), core::ptr::null_mut());
    rusty_alloc::options::out_fmt("hi");
    rusty_alloc::options::register_output(None, core::ptr::null_mut());
}

/// F-04 / F-05: `chunk_free_n` of 0 or `usize::MAX` is false, not a bitmap OOB
/// (OH-rusty_alloc-25).
#[test]
fn oh_f04_chunk_free_n_zero_and_huge_are_false() {
    let bytes = rusty_alloc::types::SEGMENT_SIZE.saturating_mul(2);
    let Ok(id) = rusty_alloc::arena::reserve_os_memory_ex(bytes, true, false, true) else {
        return;
    };
    let Some((p, _)) = rusty_alloc::arena::chunk_alloc_n(id, 1) else {
        return;
    };
    assert!(!rusty_alloc::arena::chunk_free_n(p, 0));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rusty_alloc::arena::chunk_free_n(p, usize::MAX)
    }));
    assert!(r.is_ok(), "chunk_free_n(MAX) panicked");
    assert!(!r.unwrap());
    assert!(rusty_alloc::arena::chunk_free(p));
}

/// F-05: an interior pointer into a live chunk is not a successful free
/// (OH-rusty_alloc-27). The chunk start must still be reclaimable afterwards.
#[test]
fn oh_f05_chunk_free_interior_is_false() {
    let bytes = rusty_alloc::types::SEGMENT_SIZE.saturating_mul(2);
    let Ok(id) = rusty_alloc::arena::reserve_os_memory_ex(bytes, true, false, true) else {
        return;
    };
    let Some((p, _)) = rusty_alloc::arena::chunk_alloc_n(id, 1) else {
        return;
    };
    let interior = p.wrapping_add(4096);
    assert!(!rusty_alloc::arena::chunk_free(interior));
    assert!(!rusty_alloc::arena::chunk_free_n(interior, 1));
    assert!(rusty_alloc::arena::chunk_free(p));
}

/// F-01: `os::alloc_aligned` with a non-power-of-two align is `Err`, not
/// `assert!` (OH-rusty_alloc-28).
#[test]
fn oh_f01_os_alloc_aligned_bad_align_is_err() {
    for align in [0usize, 3, 12] {
        let r =
            std::panic::catch_unwind(|| rusty_alloc::os::alloc_aligned(4096, align, true, false));
        assert!(r.is_ok(), "os::alloc_aligned(align={align}) panicked");
        assert!(
            r.unwrap().is_err(),
            "os::alloc_aligned(align={align}) succeeded"
        );
    }
}

/// F-01 / F-04: `os::alloc_aligned(usize::MAX, page)` is Err, not
/// `size + alignment` overflow in the prim aligned-reserve dance
/// (OH-rusty_alloc-29).
#[test]
fn oh_f01_os_alloc_aligned_max_is_err() {
    let ps = rusty_alloc::os::page_size();
    let r =
        std::panic::catch_unwind(|| rusty_alloc::os::alloc_aligned(usize::MAX, ps, true, false));
    assert!(r.is_ok(), "os::alloc_aligned(MAX, page) panicked");
    assert!(
        r.unwrap().is_err(),
        "os::alloc_aligned(MAX, page) succeeded"
    );
    let r = std::panic::catch_unwind(|| {
        rusty_alloc::os::alloc_aligned(usize::MAX, rusty_alloc::types::SEGMENT_SIZE, true, false)
    });
    assert!(r.is_ok(), "os::alloc_aligned(MAX, SEGMENT) panicked");
    assert!(
        r.unwrap().is_err(),
        "os::alloc_aligned(MAX, SEGMENT) succeeded"
    );
}

/// F-05: `arena_area` past the 32-slot table is null, not `ARENAS[id]` OOB.
#[test]
fn oh_f05_arena_area_past_max_is_null() {
    let (p, sz) = rusty_alloc::arena::arena_area(32);
    assert!(p.is_null());
    assert_eq!(sz, 0);
    let (p, sz) = rusty_alloc::arena::arena_area(i32::MAX);
    assert!(p.is_null());
    assert_eq!(sz, 0);
    let (p, sz) = rusty_alloc::arena::arena_area(-1);
    assert!(p.is_null());
    assert_eq!(sz, 0);
}

/// F-01 / F-04: with guarded sampling on, `malloc(MAX)` is null, not
/// `payload + page` overflow (OH-rusty_alloc-31). Restores rate 0 so later
/// tests in this binary are not all guarded.
#[test]
fn oh_f01_guarded_malloc_max_is_null() {
    let hb = init::heap_box();
    // SAFETY: `hb` is this thread's live heap box; the setters take the owner.
    unsafe {
        (*(*hb).heap.get()).guarded_set_size_bound(0, usize::MAX);
        (*(*hb).heap.get()).guarded_set_sample_rate(1, 1);
    }
    let r = std::panic::catch_unwind(|| malloc(usize::MAX));
    // SAFETY: as above.
    unsafe {
        (*(*hb).heap.get()).guarded_set_sample_rate(0, 0);
    }
    assert!(r.is_ok(), "guarded malloc(MAX) panicked");
    assert!(
        r.unwrap().is_null(),
        "guarded malloc(MAX) wrapped to a pointer"
    );
}

/// F-01 / F-04: public `bin_size` of a garbage index is 0, not `bin + 3`
/// overflow (OH-rusty_alloc-32).
#[test]
fn oh_f04_bin_size_garbage_index_is_total() {
    let r = std::panic::catch_unwind(|| bins::bin_size(usize::MAX));
    assert!(r.is_ok(), "bin_size(MAX) panicked");
    assert_eq!(r.unwrap(), 0);
    assert_eq!(bins::bin_size(0), 0);
}

/// F-01 / F-04: `dedicated_segments(MAX)` is unrepresentable (`usize::MAX`),
/// not `SLICE + size` overflow, and not 0 (which already means "shares")
/// (OH-rusty_alloc-33).
#[test]
fn oh_f04_dedicated_segments_max_is_unrepresentable() {
    use rusty_alloc::prim::fixed::{dedicated_segments, shape_of};
    let r = std::panic::catch_unwind(|| dedicated_segments(usize::MAX));
    assert!(r.is_ok(), "dedicated_segments(MAX) panicked");
    let n = r.unwrap();
    assert_ne!(n, 0, "dedicated_segments(MAX) = 0 would lie as 'shares'");
    assert_eq!(n, usize::MAX);
    assert_eq!(dedicated_segments(0), 0);
    assert_eq!(
        dedicated_segments(rusty_alloc::types::LARGE_OBJ_SIZE_MAX),
        0
    );
    let r = std::panic::catch_unwind(|| shape_of(usize::MAX));
    assert!(r.is_ok(), "shape_of(MAX) panicked");
    assert_eq!(r.unwrap().dedicated_segments, n);
}

/// F-01 / F-04: `region_for(MAX)` is 0, not `segments * SEGMENT_SIZE`
/// overflow (OH-rusty_alloc-34).
#[test]
fn oh_f04_region_for_max_is_zero() {
    use rusty_alloc::prim::fixed::{region_for, region_for_allocs};
    let r = std::panic::catch_unwind(|| region_for(usize::MAX));
    assert!(r.is_ok(), "region_for(MAX) panicked");
    assert_eq!(r.unwrap(), 0);
    assert_eq!(region_for(0), rusty_alloc::types::SEGMENT_SIZE);
    let r = std::panic::catch_unwind(|| region_for_allocs(usize::MAX, 1));
    assert!(r.is_ok(), "region_for_allocs(MAX, 1) panicked");
    assert_eq!(r.unwrap(), 0);
}

/// F-01: `prim::alloc` garbage align is Err, not `align_up` debug_assert
/// (OH-rusty_alloc-38).
#[test]
fn oh_f01_prim_alloc_bad_align_is_err() {
    // SAFETY: the OS wrapper is handed a garbage alignment; the assertion is
    // that it refuses BEFORE any syscall, so nothing is mapped or leaked.
    let r = std::panic::catch_unwind(|| unsafe { rusty_alloc::prim::alloc(64, 0, true, false) });
    assert!(r.is_ok(), "prim::alloc(align=0) panicked");
    assert!(r.unwrap().is_err());
    // SAFETY: as above, with a non-power-of-two alignment.
    let r = std::panic::catch_unwind(|| unsafe { rusty_alloc::prim::alloc(64, 3, true, false) });
    assert!(r.is_ok(), "prim::alloc(align=3) panicked");
    assert!(r.unwrap().is_err());
}

/// F-01: `huge_alloc` garbage align is Err, not `debug_assert!`
/// (OH-rusty_alloc-39).
#[test]
fn oh_f01_huge_alloc_bad_align_is_err() {
    let r = std::panic::catch_unwind(|| rusty_alloc::segment::huge_alloc(64, 0, 0, -1));
    assert!(r.is_ok(), "huge_alloc(align=0) panicked");
    assert!(r.unwrap().is_err());
    let r = std::panic::catch_unwind(|| {
        rusty_alloc::segment::huge_alloc(64, rusty_alloc::types::SEGMENT_SIZE, 0, -1)
    });
    assert!(r.is_ok(), "huge_alloc(align=SEGMENT) panicked");
    assert!(r.unwrap().is_err());
}

/// F-01 / F-04: in-place `realloc_aligned_at` huge offset is null; original
/// lives (OH-rusty_alloc-42).
#[test]
fn oh_f04_realloc_inplace_huge_offset_is_null() {
    use rusty_alloc::alloc::{realloc_aligned_at, usable_size};
    let p = malloc(64);
    assert!(!p.is_null());
    // SAFETY: `p` is a live 64-byte block of ours; the realloc keeps it live
    // (it must refuse), and it is freed once at the end.
    unsafe {
        p.write(0x42);
        let u = usable_size(p);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            realloc_aligned_at(p, u, 8, usize::MAX)
        }));
        assert!(r.is_ok(), "realloc_aligned_at in-place MAX offset panicked");
        assert!(r.unwrap().is_null());
        assert_eq!(p.read(), 0x42);
        free(p);
    }
}

/// F-01 / F-04: in-place `rezalloc_aligned_at` huge offset is null
/// (OH-rusty_alloc-43).
#[test]
fn oh_f04_rezalloc_inplace_huge_offset_is_null() {
    use rusty_alloc::alloc::{rezalloc_aligned_at, usable_size, zalloc};
    let p = zalloc(64);
    assert!(!p.is_null());
    // SAFETY: as the realloc twin above.
    unsafe {
        let u = usable_size(p);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rezalloc_aligned_at(p, u, 8, usize::MAX)
        }));
        assert!(
            r.is_ok(),
            "rezalloc_aligned_at in-place MAX offset panicked"
        );
        assert!(r.unwrap().is_null());
        free(p);
    }
}

/// F-01: `prim::commit(null)` is Err, not a fresh OS mapping
/// (OH-rusty_alloc-60).
#[test]
fn oh_f01_prim_commit_null_is_err() {
    let ps = rusty_alloc::os::page_size();
    // SAFETY: the OS wrapper is handed null; the assertion is that it refuses
    // before any syscall (it used to hand Windows an untracked mapping).
    let r = std::panic::catch_unwind(|| unsafe {
        rusty_alloc::prim::commit(core::ptr::null_mut(), ps)
    });
    assert!(r.is_ok(), "prim::commit(null) panicked");
    assert!(r.unwrap().is_err(), "prim::commit(null) succeeded");
}

/// F-01: `malloc_small(MAX)` is null, not a debug_assert (OH-rusty_alloc-62).
#[test]
fn oh_f01_malloc_small_max_is_null() {
    let r = std::panic::catch_unwind(|| rusty_alloc::alloc::malloc_small(usize::MAX));
    assert!(r.is_ok(), "malloc_small(MAX) panicked");
    assert!(r.unwrap().is_null());
    let r = std::panic::catch_unwind(|| rusty_alloc::alloc::zalloc_small(usize::MAX));
    assert!(r.is_ok(), "zalloc_small(MAX) panicked");
    assert!(r.unwrap().is_null());
}

/// F-05: `manage_os_memory_ex(0x8, 2*SEGMENT)` is Err (OH-rusty_alloc-115).
#[test]
fn oh_f05_manage_os_memory_garbage_base_is_err() {
    let r = rusty_alloc::arena::manage_os_memory_ex(
        8usize as *mut u8,
        rusty_alloc::types::SEGMENT_SIZE.saturating_mul(2),
        true,
        false,
        false,
        -1,
        false,
    );
    assert!(r.is_err());
}

/// F-05: a high unmapped start is not an arena (OH-rusty_alloc-201).
#[test]
fn oh_f05_manage_os_memory_unmapped_is_err() {
    let unmapped = 1usize << 40;
    let r = rusty_alloc::arena::manage_os_memory_ex(
        unmapped as *mut u8,
        rusty_alloc::types::SEGMENT_SIZE.saturating_mul(2),
        true,
        false,
        false,
        -1,
        false,
    );
    assert!(r.is_err());
    let r = rusty_alloc::arena::manage_os_memory_ex(
        (unmapped + 8) as *mut u8,
        rusty_alloc::types::SEGMENT_SIZE.saturating_mul(2),
        true,
        false,
        false,
        -1,
        false,
    );
    assert!(r.is_err());
}

/// F-05: a live segment window is not caller memory (OH-rusty_alloc-202).
#[test]
fn oh_f05_manage_os_memory_live_window_is_err() {
    let p = malloc(16);
    assert!(!p.is_null());
    let seg = rusty_alloc::segment::segment_of(p);
    let r = rusty_alloc::arena::manage_os_memory_ex(
        seg.cast(),
        rusty_alloc::types::SEGMENT_SIZE,
        true,
        false,
        false,
        -1,
        false,
    );
    assert!(r.is_err());
    // SAFETY: `p` is a live 16-byte block of ours, untouched by the refused
    // adoption; freed once.
    unsafe {
        p.write(0x62);
        assert_eq!(p.read(), 0x62);
        free(p);
    }
}

/// F-05: a real OS reservation is still adoptable (OH-rusty_alloc-201/202).
#[test]
fn oh_f05_manage_os_memory_real_block_still_works() {
    let b = rusty_alloc::os::alloc_aligned(
        rusty_alloc::types::SEGMENT_SIZE.saturating_mul(2),
        rusty_alloc::types::SEGMENT_SIZE,
        true,
        false,
    )
    .expect("os block");
    let id = rusty_alloc::arena::manage_os_memory_ex(b.ptr, b.size, true, false, false, -1, false)
        .expect("manage real block");
    let (base, sz) = rusty_alloc::arena::arena_area(id);
    assert_eq!(base, b.ptr);
    assert!(sz >= rusty_alloc::types::SEGMENT_SIZE);
}
