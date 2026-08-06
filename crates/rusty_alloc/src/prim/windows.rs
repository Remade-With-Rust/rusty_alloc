//! Windows prim backend (mirrors upstream `src/prim/windows/prim.c`).
//!
//! Salient Windows facts encoded here:
//! - `VirtualAlloc(NULL, …)` is 64 KiB-granularity-aligned; stronger alignment
//!   uses the reserve-oversized → release → re-reserve-at-aligned race-retry
//!   dance (`win_alloc_aligned` upstream) because Windows cannot partially free.
//! - Fresh commits are zero pages; decommitted ranges fault until recommitted.
//! - Large pages need `SeLockMemoryPrivilege`; we attempt and fall back.
//! - Thread-exit callbacks come from fiber-local storage (`FlsAlloc`).

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicU64, Ordering};

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Memory::{
    GetLargePageMinimum, MEM_COMMIT, MEM_DECOMMIT, MEM_LARGE_PAGES, MEM_RELEASE, MEM_RESERVE,
    MEM_RESET, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::{
    FlsAlloc, FlsGetValue, FlsSetValue, GetCurrentThreadId, GetNumaHighestNodeNumber,
};

use super::{Alloc, MemConfig, PrimError, TlsDtor, align_up};

pub(super) fn mem_init() -> MemConfig {
    // SAFETY: SYSTEM_INFO is plain-old-data; GetSystemInfo fully initializes it.
    let si = unsafe {
        let mut si: SYSTEM_INFO = core::mem::zeroed();
        GetSystemInfo(&mut si);
        si
    };
    // SAFETY: no preconditions; returns 0 when the privilege is unavailable.
    let large = unsafe { GetLargePageMinimum() };
    MemConfig {
        page_size: si.dwPageSize as usize,
        alloc_granularity: si.dwAllocationGranularity as usize,
        large_page_size: large,
        has_overcommit: false,
        has_partial_free: false,
    }
}

fn last_error() -> PrimError {
    // SAFETY: no preconditions.
    unsafe { GetLastError() }
}

pub(super) unsafe fn alloc(
    size: usize,
    try_alignment: usize,
    commit: bool,
    allow_large: bool,
) -> Result<Alloc, PrimError> {
    let flags = MEM_RESERVE | if commit { MEM_COMMIT } else { 0 };

    // Large-page attempt: requires commit (reserve-only large pages are not a
    // thing) and the privilege; never an error to lack it.
    if allow_large && commit {
        // SAFETY: no preconditions.
        let large_min = unsafe { GetLargePageMinimum() };
        if large_min > 0 && try_alignment <= large_min && size >= large_min {
            let lsize = align_up(size, large_min);
            // SAFETY: NULL base + valid flags; failure returns null, handled.
            let p = unsafe {
                VirtualAlloc(ptr::null(), lsize, flags | MEM_LARGE_PAGES, PAGE_READWRITE)
            };
            if !p.is_null() && (p as usize).is_multiple_of(try_alignment) {
                return Ok(Alloc {
                    ptr: p.cast(),
                    is_large: true,
                    is_zero: true,
                });
            }
            if !p.is_null() {
                // SAFETY: p is a whole mapping we just made and never exposed.
                unsafe { VirtualFree(p, 0, MEM_RELEASE) };
            }
            // fall through to normal pages
        }
    }

    // Fast path: reservations are 64 KiB aligned already.
    // SAFETY: NULL base + valid flags; failure returns null, handled.
    let p = unsafe { VirtualAlloc(ptr::null(), size, flags, PAGE_READWRITE) };
    if !p.is_null() && (p as usize).is_multiple_of(try_alignment) {
        return Ok(Alloc {
            ptr: p.cast(),
            is_large: false,
            is_zero: true,
        });
    }
    if !p.is_null() {
        // SAFETY: whole mapping we just made and never exposed.
        unsafe { VirtualFree(p, 0, MEM_RELEASE) };
    }

    // Aligned dance: reserve oversized to find an aligned address, release,
    // re-reserve exactly there. Another thread can steal the range between the
    // two calls, hence the retry loop (upstream uses 3 tries as well).
    for _ in 0..3 {
        // SAFETY: reserve-only of an oversized range; failure handled.
        let probe = unsafe {
            VirtualAlloc(
                ptr::null(),
                size + try_alignment,
                MEM_RESERVE,
                PAGE_NOACCESS,
            )
        };
        if probe.is_null() {
            return Err(last_error());
        }
        let aligned = align_up(probe as usize, try_alignment) as *mut c_void;
        // SAFETY: whole probe mapping, never exposed.
        unsafe { VirtualFree(probe, 0, MEM_RELEASE) };
        // SAFETY: explicit base inside a range we just observed free; on race
        // the call fails with null and we retry.
        let p = unsafe { VirtualAlloc(aligned, size, flags, PAGE_READWRITE) };
        if core::ptr::eq(p, aligned) {
            return Ok(Alloc {
                ptr: p.cast(),
                is_large: false,
                is_zero: true,
            });
        }
        if !p.is_null() {
            // SAFETY: mapping we just made at the wrong address, never exposed.
            unsafe { VirtualFree(p, 0, MEM_RELEASE) };
        }
    }
    Err(last_error())
}

pub(super) unsafe fn free(ptr_: *mut u8, _size: usize) -> Result<(), PrimError> {
    // SAFETY: caller passes a whole-mapping base per the prim contract;
    // MEM_RELEASE with size 0 releases the whole reservation.
    let ok = unsafe { VirtualFree(ptr_.cast(), 0, MEM_RELEASE) };
    if ok != 0 { Ok(()) } else { Err(last_error()) }
}

pub(super) unsafe fn commit(ptr_: *mut u8, size: usize) -> Result<bool, PrimError> {
    // SAFETY: caller guarantees the range lies in a live reservation.
    let p = unsafe { VirtualAlloc(ptr_.cast(), size, MEM_COMMIT, PAGE_READWRITE) };
    if p.is_null() {
        return Err(last_error());
    }
    // Conservative: the range may include already-committed pages whose
    // contents persist, so we cannot promise zero (fresh pages ARE zero).
    Ok(false)
}

pub(super) unsafe fn decommit(ptr_: *mut u8, size: usize) -> Result<bool, PrimError> {
    // SAFETY: caller guarantees the range lies in a live reservation.
    let ok = unsafe { VirtualFree(ptr_.cast(), size, MEM_DECOMMIT) };
    if ok != 0 { Ok(true) } else { Err(last_error()) }
}

pub(super) unsafe fn reset(ptr_: *mut u8, size: usize) -> Result<(), PrimError> {
    // SAFETY: caller guarantees a live committed range; MEM_RESET marks the
    // contents dead without decommitting.
    let p = unsafe { VirtualAlloc(ptr_.cast(), size, MEM_RESET, PAGE_READWRITE) };
    if !p.is_null() {
        Ok(())
    } else {
        Err(last_error())
    }
}

pub(super) unsafe fn protect(ptr_: *mut u8, size: usize, on: bool) -> Result<(), PrimError> {
    let new = if on { PAGE_NOACCESS } else { PAGE_READWRITE };
    let mut old = 0u32;
    // SAFETY: caller guarantees a live committed range; old-protection out-param
    // is a valid local.
    let ok = unsafe { VirtualProtect(ptr_.cast(), size, new, &mut old) };
    if ok != 0 { Ok(()) } else { Err(last_error()) }
}

pub(super) fn numa_node_count() -> usize {
    let mut highest = 0u32;
    // SAFETY: out-param is a valid local.
    let ok = unsafe { GetNumaHighestNodeNumber(&mut highest) };
    if ok != 0 { highest as usize + 1 } else { 1 }
}

#[inline]
pub(super) fn thread_id() -> usize {
    // SAFETY: no preconditions.
    (unsafe { GetCurrentThreadId() }) as usize
}

pub(super) fn clock_now() -> u64 {
    static FREQ: AtomicU64 = AtomicU64::new(0);
    let mut freq = FREQ.load(Ordering::Relaxed);
    if freq == 0 {
        let mut f = 0i64;
        // SAFETY: out-param is a valid local; QPF cannot fail on XP+.
        unsafe { QueryPerformanceFrequency(&mut f) };
        freq = f.max(1) as u64;
        FREQ.store(freq, Ordering::Relaxed);
    }
    let mut count = 0i64;
    // SAFETY: out-param is a valid local.
    unsafe { QueryPerformanceCounter(&mut count) };
    // counts * ns_per_s / freq, in u128 to avoid overflow (~292 years of ns).
    ((count as u128 * 1_000_000_000u128) / freq as u128) as u64
}

pub(super) struct TlsSlotImpl(u32);

pub(super) fn tls_new(dtor: Option<TlsDtor>) -> Option<TlsSlotImpl> {
    // SAFETY: FlsAlloc accepts an optional callback pointer of exactly this
    // type; the callback fires at thread exit with the slot value when non-null.
    let idx = unsafe { FlsAlloc(dtor) };
    if idx == u32::MAX {
        None
    } else {
        Some(TlsSlotImpl(idx))
    }
}

#[inline]
pub(super) fn tls_get(slot: &TlsSlotImpl) -> *mut c_void {
    // SAFETY: index came from a successful FlsAlloc and is never freed (slots
    // live for the process — mirrors upstream).
    unsafe { FlsGetValue(slot.0) }
}

#[inline]
pub(super) fn tls_set(slot: &TlsSlotImpl, value: *mut c_void) {
    // SAFETY: as tls_get.
    unsafe { FlsSetValue(slot.0, value) };
}

pub(super) fn tls_raw(slot: &TlsSlotImpl) -> usize {
    slot.0 as usize
}

pub(super) fn tls_from_raw(raw: usize) -> TlsSlotImpl {
    TlsSlotImpl(raw as u32)
}
