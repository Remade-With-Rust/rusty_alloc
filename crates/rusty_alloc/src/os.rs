//! OS memory layer over [`crate::prim`] (mirrors upstream `src/os.c`).
//!
//! Adds to prim: cached configuration, page-size rounding, alignment
//! validation, and the purge policy (decommit vs reset — `mi_option_purge_decommits`,
//! wired to the options table in M7; until then callers pass the policy).

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::prim::{self, PrimError};

/// A block of OS memory returned by [`alloc_aligned`].
#[derive(Debug, Clone, Copy)]
pub struct OsBlock {
    /// Base address — aligned as requested; also the free handle.
    pub ptr: *mut u8,
    /// Rounded (page-multiple) size actually reserved.
    pub size: usize,
    /// Backed by large pages.
    pub is_large: bool,
    /// Contents guaranteed zero.
    pub is_zero: bool,
}

static PAGE_SIZE: AtomicUsize = AtomicUsize::new(0);
static ALLOC_GRANULARITY: AtomicUsize = AtomicUsize::new(0);
static LARGE_PAGE_SIZE: AtomicUsize = AtomicUsize::new(usize::MAX); // MAX = uninit (0 is a valid value)

fn config_init() {
    let cfg = prim::mem_init();
    // Benign race: idempotent stores of identical values.
    PAGE_SIZE.store(cfg.page_size, Ordering::Relaxed);
    ALLOC_GRANULARITY.store(cfg.alloc_granularity, Ordering::Relaxed);
    LARGE_PAGE_SIZE.store(cfg.large_page_size, Ordering::Relaxed);
}

/// OS page size (cached).
pub fn page_size() -> usize {
    let v = PAGE_SIZE.load(Ordering::Relaxed);
    if v != 0 {
        return v;
    }
    config_init();
    PAGE_SIZE.load(Ordering::Relaxed)
}

/// Reservation granularity (64 KiB on Windows, page size elsewhere; cached).
pub fn alloc_granularity() -> usize {
    let v = ALLOC_GRANULARITY.load(Ordering::Relaxed);
    if v != 0 {
        return v;
    }
    config_init();
    ALLOC_GRANULARITY.load(Ordering::Relaxed)
}

/// Large-page size, 0 when unavailable (cached).
pub fn large_page_size() -> usize {
    let v = LARGE_PAGE_SIZE.load(Ordering::Relaxed);
    if v != usize::MAX {
        return v;
    }
    config_init();
    LARGE_PAGE_SIZE.load(Ordering::Relaxed)
}

/// Round `size` up to a whole number of OS pages (never 0).
pub fn page_align_up(size: usize) -> usize {
    let ps = page_size();
    size.max(1).div_ceil(ps) * ps
}

/// Reserve (and optionally commit) an aligned block of OS memory.
///
/// `alignment` must be a power of two ≥ page size; `size` is rounded up to
/// pages. This is what segments (M3) and arenas (M6) sit on.
pub fn alloc_aligned(
    size: usize,
    alignment: usize,
    commit: bool,
    allow_large: bool,
) -> Result<OsBlock, PrimError> {
    assert!(
        alignment.is_power_of_two(),
        "alignment must be a power of two"
    );
    let alignment = alignment.max(page_size());
    let size = page_align_up(size);
    // SAFETY: size is page-multiple and > 0, alignment a power of two; the
    // returned mapping is owned by the OsBlock we hand out.
    let a = unsafe { prim::alloc(size, alignment, commit, allow_large)? };
    debug_assert_eq!(
        (a.ptr as usize) % alignment,
        0,
        "prim returned misaligned block"
    );
    Ok(OsBlock {
        ptr: a.ptr,
        size,
        is_large: a.is_large,
        is_zero: a.is_zero,
    })
}

/// Release a block from [`alloc_aligned`].
///
/// # Safety
/// `block` must come from [`alloc_aligned`], be unfreed, and have no live
/// references into it.
pub unsafe fn free(block: OsBlock) -> Result<(), PrimError> {
    // SAFETY: forwarded contract (whole mapping base + size).
    unsafe { prim::free(block.ptr, block.size) }
}

/// Commit a page-aligned sub-range of a reserved block. Returns known-zero.
///
/// # Safety
/// Range must lie within a live block from [`alloc_aligned`], page-aligned.
pub unsafe fn commit(ptr: *mut u8, size: usize) -> Result<bool, PrimError> {
    // SAFETY: forwarded contract.
    unsafe { prim::commit(ptr, page_align_up(size)) }
}

/// Decommit a page-aligned sub-range. Returns whether recommit is required.
///
/// # Safety
/// As [`commit`]; contents are lost.
pub unsafe fn decommit(ptr: *mut u8, size: usize) -> Result<bool, PrimError> {
    // SAFETY: forwarded contract.
    unsafe { prim::decommit(ptr, page_align_up(size)) }
}

/// Purge a range per policy: `purge_decommits` → decommit (returns whether
/// recommit is needed); otherwise reset (stays committed, contents undefined).
///
/// # Safety
/// As [`commit`]; contents are lost either way.
pub unsafe fn purge(ptr: *mut u8, size: usize, purge_decommits: bool) -> Result<bool, PrimError> {
    if purge_decommits {
        // SAFETY: forwarded contract.
        unsafe { decommit(ptr, size) }
    } else {
        // SAFETY: forwarded contract.
        unsafe { prim::reset(ptr, page_align_up(size))? };
        Ok(false)
    }
}

/// Toggle no-access protection on a page-aligned range (guard pages).
///
/// # Safety
/// As [`commit`]; caller must not touch a protected range.
pub unsafe fn protect(ptr: *mut u8, size: usize, on: bool) -> Result<(), PrimError> {
    // SAFETY: forwarded contract.
    unsafe { prim::protect(ptr, page_align_up(size), on) }
}
