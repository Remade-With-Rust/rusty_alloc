//! Stats reporting (mirrors `stats.c`): per-heap counters live in
//! [`crate::heap::Stats`]; this module aggregates them across the global heap
//! registry, formats reports through the output hook, and queries process
//! metrics (`mi_process_info`).

use crate::heap::Stats;
use crate::options::out_fmt;

/// Sum the counters of every registered heap (`mi_stats_merge` semantics —
/// our counters are always per-heap, so the merged view is computed on read).
pub fn merged() -> Stats {
    let mut total = Stats::new();
    crate::init::for_each_heap(&mut |h| {
        let s = h.stats;
        total.allocs += s.allocs;
        total.frees += s.frees;
        total.generic += s.generic;
        total.pages_fresh += s.pages_fresh;
        total.segments += s.segments;
        total.huge_allocs += s.huge_allocs;
        total.extends += s.extends;
        total.large_allocs += s.large_allocs;
        total.pages_retired += s.pages_retired;
        total.segments_freed += s.segments_freed;
        total.realloc_in_place += s.realloc_in_place;
        total.realloc_moved += s.realloc_moved;
        total.delayed_frees += s.delayed_frees;
        total.reclaims += s.reclaims;
    });
    total
}

fn print_one(label: &str, s: &Stats) {
    out_fmt(&format!(
        "{label}: allocs {} frees {} (generic {}), pages fresh {} retired {}, \
         segments {} freed {}, large {} huge {}, realloc {}/{} (in-place/moved), \
         delayed {} reclaims {}\n",
        s.allocs,
        s.frees,
        s.generic,
        s.pages_fresh,
        s.pages_retired,
        s.segments,
        s.segments_freed,
        s.large_allocs,
        s.huge_allocs,
        s.realloc_in_place,
        s.realloc_moved,
        s.delayed_frees,
        s.reclaims,
    ));
}

/// `mi_stats_print` / `mi_stats_print_out`: process-wide (merged) stats.
pub fn print_process() {
    let m = merged();
    print_one("heap stats (process)", &m);
    let (elapsed, user, sys, rss, peak_rss, commit, peak_commit, faults) = process_info();
    out_fmt(&format!(
        "process: elapsed {elapsed} ms, user {user} ms, sys {sys} ms, rss {} KiB (peak {}), \
         commit {} KiB (peak {}), faults {faults}\n",
        rss / 1024,
        peak_rss / 1024,
        commit / 1024,
        peak_commit / 1024,
    ));
}

/// `mi_thread_stats_print_out`: the calling thread's heap only.
pub fn print_thread() {
    let s = crate::alloc::stats();
    print_one("heap stats (thread)", &s);
}

/// `mi_stats_reset`: zero the CALLING thread's counters (per-heap model).
pub fn reset() {
    // SAFETY: own heap.
    unsafe {
        let hb = crate::init::heap_box();
        (*(*hb).heap.get()).stats = Stats::new();
    }
}

/// `mi_process_info`: (elapsed_ms, user_ms, system_ms, current_rss, peak_rss,
/// current_commit, peak_commit, page_faults). Best-effort per platform;
/// unknown fields read 0.
pub fn process_info() -> (usize, usize, usize, usize, usize, usize, usize, usize) {
    #[cfg(all(windows, not(miri)))]
    {
        win_process_info()
    }
    #[cfg(all(unix, not(miri)))]
    {
        unix_process_info()
    }
    // Miri and wasm: no process accounting to report. Wasm has no RSS concept
    // distinct from the size of linear memory, and no host time.
    #[cfg(any(miri, all(target_arch = "wasm32", not(miri))))]
    {
        (0, 0, 0, 0, 0, 0, 0, 0)
    }
}

#[cfg(all(windows, not(miri)))]
fn win_process_info() -> (usize, usize, usize, usize, usize, usize, usize, usize) {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    // SAFETY: out-params are valid locals; pseudo-handle needs no rights.
    unsafe {
        let proc = GetCurrentProcess();
        let (mut c, mut e, mut k, mut u) = ([0u32; 2], [0u32; 2], [0u32; 2], [0u32; 2]);
        GetProcessTimes(
            proc,
            c.as_mut_ptr().cast(),
            e.as_mut_ptr().cast(),
            k.as_mut_ptr().cast(),
            u.as_mut_ptr().cast(),
        );
        let ft = |t: [u32; 2]| ((t[1] as u64) << 32 | t[0] as u64) / 10_000; // 100ns → ms
        let mut pmc: PROCESS_MEMORY_COUNTERS = core::mem::zeroed();
        pmc.cb = core::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        K32GetProcessMemoryInfo(proc, &mut pmc, pmc.cb);
        let now = crate::prim::clock_now() / 1_000_000;
        (
            now as usize,
            ft(u) as usize,
            ft(k) as usize,
            pmc.WorkingSetSize,
            pmc.PeakWorkingSetSize,
            pmc.PagefileUsage,
            pmc.PeakPagefileUsage,
            pmc.PageFaultCount as usize,
        )
    }
}

#[cfg(all(unix, not(miri)))]
fn unix_process_info() -> (usize, usize, usize, usize, usize, usize, usize, usize) {
    // SAFETY: out-param is a valid local.
    unsafe {
        let mut ru: libc::rusage = core::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        let ms = |tv: libc::timeval| (tv.tv_sec as usize) * 1000 + (tv.tv_usec as usize) / 1000;
        // Linux ru_maxrss is KiB.
        let peak_rss = (ru.ru_maxrss as usize) * 1024;
        let current_rss = std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|s| {
                s.split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse::<usize>().ok())
            })
            .map(|pages| pages * crate::os::page_size())
            .unwrap_or(0);
        let now = crate::prim::clock_now() / 1_000_000;
        (
            now as usize,
            ms(ru.ru_utime),
            ms(ru.ru_stime),
            current_rss,
            peak_rss,
            0,
            0,
            (ru.ru_majflt + ru.ru_minflt) as usize,
        )
    }
}
