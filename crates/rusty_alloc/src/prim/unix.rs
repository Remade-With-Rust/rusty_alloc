//! Unix prim backend (mirrors upstream `src/prim/unix/prim.c`).
//!
//! Salient unix facts encoded here:
//! - `munmap` can free part of a mapping, so alignment is over-allocate + trim
//!   (no race-retry dance needed).
//! - `MADV_DONTNEED` (Linux) leaves the range accessible and zero-on-next-touch
//!   → decommit needs no recommit.
//! - Reserve-only memory is `PROT_NONE` (+`MAP_NORESERVE` on Linux) so commit
//!   charges appear when the allocator says so, not at reservation.

use core::ffi::c_void;
use core::ptr;

use super::{Alloc, MemConfig, PrimError, TlsDtor, align_up};

fn errno() -> PrimError {
    // SAFETY: __errno_location/__error return a valid thread-local pointer.
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location() as PrimError
    }
    #[cfg(target_os = "macos")]
    unsafe {
        *libc::__error() as PrimError
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        1 // EPERM stand-in; refine per-OS as targets are added
    }
}

pub(super) fn mem_init() -> MemConfig {
    // SAFETY: sysconf with a valid name has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = if page > 0 { page as usize } else { 4096 };
    MemConfig {
        page_size,
        alloc_granularity: page_size,
        large_page_size: 2 * 1024 * 1024, // advisory; real THP/hugetlb wiring in M6
        has_overcommit: cfg!(target_os = "linux"),
        has_partial_free: true,
    }
}

unsafe fn mmap_anon(hint: *mut c_void, size: usize, commit: bool) -> Result<*mut u8, PrimError> {
    let prot = if commit {
        libc::PROT_READ | libc::PROT_WRITE
    } else {
        libc::PROT_NONE
    };
    #[allow(unused_mut)]
    let mut flags = libc::MAP_PRIVATE | libc::MAP_ANON;
    #[cfg(target_os = "linux")]
    if !commit {
        flags |= libc::MAP_NORESERVE;
    }
    // SAFETY: anonymous private mapping; hint may be null; failure yields
    // MAP_FAILED which we translate to errno.
    let p = unsafe { libc::mmap(hint, size, prot, flags, -1, 0) };
    if p == libc::MAP_FAILED {
        Err(errno())
    } else {
        Ok(p.cast())
    }
}

pub(super) unsafe fn alloc(
    size: usize,
    try_alignment: usize,
    commit: bool,
    _allow_large: bool, // explicit huge-page mappings land in M6; THP applies transparently
) -> Result<Alloc, PrimError> {
    let cfg = mem_init();
    if try_alignment <= cfg.page_size {
        // SAFETY: forwarded contract.
        let p = unsafe { mmap_anon(ptr::null_mut(), size, commit)? };
        return Ok(Alloc {
            ptr: p,
            is_large: false,
            is_zero: true,
        });
    }

    // Over-allocate and trim the unaligned head/tail (partial free is allowed).
    let over = size + try_alignment;
    // SAFETY: forwarded contract.
    let raw = unsafe { mmap_anon(ptr::null_mut(), over, commit)? };
    let base = raw as usize;
    let aligned = align_up(base, try_alignment);
    let pre = aligned - base;
    let post = over - size - pre;
    if pre > 0 {
        // SAFETY: [raw, raw+pre) is the head of the mapping we just made.
        unsafe { libc::munmap(raw.cast(), pre) };
    }
    if post > 0 {
        // SAFETY: the tail range lies wholly inside the same fresh mapping.
        unsafe { libc::munmap((aligned + size) as *mut c_void, post) };
    }
    Ok(Alloc {
        ptr: aligned as *mut u8,
        is_large: false,
        is_zero: true,
    })
}

pub(super) unsafe fn free(ptr_: *mut u8, size: usize) -> Result<(), PrimError> {
    // SAFETY: caller passes a range from alloc per the prim contract.
    let r = unsafe { libc::munmap(ptr_.cast(), size) };
    if r == 0 { Ok(()) } else { Err(errno()) }
}

pub(super) unsafe fn commit(ptr_: *mut u8, size: usize) -> Result<bool, PrimError> {
    // SAFETY: caller guarantees the range lies in a live mapping.
    let r = unsafe { libc::mprotect(ptr_.cast(), size, libc::PROT_READ | libc::PROT_WRITE) };
    if r != 0 {
        return Err(errno());
    }
    // Pages previously touched then DONTNEED'd read zero; pages never touched
    // read zero; but a commit over still-resident pages keeps contents →
    // conservative false, same as upstream.
    Ok(false)
}

pub(super) unsafe fn decommit(ptr_: *mut u8, size: usize) -> Result<bool, PrimError> {
    // DARWIN IS NOT LINUX HERE. `MADV_DONTNEED` is only advisory for PRIVATE
    // ANONYMOUS memory on macOS/BSD: it neither frees the physical pages nor
    // zeroes them. Using it there breaks decommit in both directions —
    //
    //   * the pages stay RESIDENT, so purge never actually returns memory to
    //     the OS and RSS only ever grows in a long-running process (this is the
    //     abandonment path, which is on by default, so it was live);
    //   * and the range keeps its old CONTENTS, violating the documented
    //     "contents are lost" contract. Nothing trusts that yet — `free_is_zero`
    //     is conservatively cleared on purge — but it is a landmine for anything
    //     that later does.
    //
    // Re-mapping is the portable-on-Darwin decommit: MAP_FIXED over a range we
    // already own atomically drops the old physical pages and installs fresh
    // zero-fill-on-demand ones. The range stays readable/writable and needs no
    // recommit, which is exactly the Linux MADV_DONTNEED contract, so callers
    // see identical semantics on both.
    #[cfg(target_vendor = "apple")]
    {
        // SAFETY: caller guarantees `[ptr_, ptr_+size)` is a page-aligned range
        // inside a live mapping WE own, so replacing it cannot clobber another
        // subsystem's memory. MAP_FIXED is what makes the replacement atomic:
        // the range is never unmapped, so a concurrent reader cannot fault.
        let p = unsafe {
            libc::mmap(
                ptr_.cast(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_FIXED,
                -1,
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return Err(errno());
        }
        return Ok(false);
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        // SAFETY: caller guarantees the range lies in a live mapping; DONTNEED
        // drops the pages, next touch faults in zeros — stays accessible.
        let r = unsafe { libc::madvise(ptr_.cast(), size, libc::MADV_DONTNEED) };
        if r == 0 { Ok(false) } else { Err(errno()) }
    }
}

pub(super) unsafe fn reset(ptr_: *mut u8, size: usize) -> Result<(), PrimError> {
    // MADV_FREE is lazy (preferred); fall back to DONTNEED where missing.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        // SAFETY: caller guarantees a live committed range.
        let r = unsafe { libc::madvise(ptr_.cast(), size, libc::MADV_FREE) };
        if r == 0 {
            return Ok(());
        }
    }
    // SAFETY: as above.
    let r = unsafe { libc::madvise(ptr_.cast(), size, libc::MADV_DONTNEED) };
    if r == 0 { Ok(()) } else { Err(errno()) }
}

pub(super) unsafe fn protect(ptr_: *mut u8, size: usize, on: bool) -> Result<(), PrimError> {
    let prot = if on {
        libc::PROT_NONE
    } else {
        libc::PROT_READ | libc::PROT_WRITE
    };
    // SAFETY: caller guarantees a live mapping.
    let r = unsafe { libc::mprotect(ptr_.cast(), size, prot) };
    if r == 0 { Ok(()) } else { Err(errno()) }
}

pub(super) fn numa_node_count() -> usize {
    1 // sysfs/getcpu wiring lands with arenas (M6)
}

#[inline]
pub(super) fn thread_id() -> usize {
    // SAFETY: no preconditions; pthread_self is async-signal-safe.
    (unsafe { libc::pthread_self() }) as usize
}

pub(super) fn clock_now() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: out-param is a valid local.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

pub(super) struct TlsSlotImpl(libc::pthread_key_t);

pub(super) fn tls_new(dtor: Option<TlsDtor>) -> Option<TlsSlotImpl> {
    let mut key: libc::pthread_key_t = 0;
    // SAFETY: out-param is a valid local; dtor has the exact pthread signature
    // and fires at thread exit for non-null values.
    let r = unsafe { libc::pthread_key_create(&mut key, dtor) };
    if r == 0 { Some(TlsSlotImpl(key)) } else { None }
}

#[inline]
pub(super) fn tls_get(slot: &TlsSlotImpl) -> *mut c_void {
    // SAFETY: key came from a successful pthread_key_create, never deleted.
    unsafe { libc::pthread_getspecific(slot.0) }
}

#[inline]
pub(super) fn tls_set(slot: &TlsSlotImpl, value: *mut c_void) {
    // SAFETY: as tls_get.
    unsafe { libc::pthread_setspecific(slot.0, value) };
}

pub(super) fn tls_raw(slot: &TlsSlotImpl) -> usize {
    slot.0 as usize
}

pub(super) fn tls_from_raw(raw: usize) -> TlsSlotImpl {
    TlsSlotImpl(raw as libc::pthread_key_t)
}
