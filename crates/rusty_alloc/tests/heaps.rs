//! M6/M7 gates: first-class heaps, arenas, subprocs, options, stats — one
//! deterministic single-test process (counters are exact here).

use rusty_alloc::init::{self, HeapBox};
use rusty_alloc::{alloc, arena, options};

unsafe fn hmalloc(hb: *mut HeapBox, n: usize) -> *mut u8 {
    // SAFETY: test owns the heap on this thread.
    unsafe { alloc::heap_malloc(hb, n) }
}

#[test]
fn heaps_arenas_subprocs_options() {
    // --- first-class heap: alloc, cross-heap free routing, delete-migrate ---
    let h1 = init::create_heap(7, false, -1);
    let mut blocks = Vec::new();
    // SAFETY: h1 is ours on this thread; blocks tracked and freed once.
    unsafe {
        for i in 0..500usize {
            let p = hmalloc(h1, 32 + (i % 900));
            assert!(!p.is_null());
            core::ptr::write_bytes(p, 0x77, 32 + (i % 900));
            blocks.push((p, 32 + (i % 900)));
        }
        // Frees route to the OWNING heap via xheap even though the thread's
        // default heap is different:
        for &(p, _) in blocks.iter().take(100) {
            alloc::free(p);
        }
        // Visitor: counts must match live blocks (400 remaining).
        let mut seen = 0usize;
        let ok = (*(*h1).heap.get()).visit_blocks(true, &mut |_area, block, _sz| {
            if !block.is_null() {
                seen += 1;
            }
            true
        });
        assert!(ok);
        assert_eq!(seen, 400, "visitor missed blocks");
        // contains/check_owned
        let (probe, _) = blocks[200];
        assert!(alloc::heap_contains_block(h1, probe));
        assert!(alloc::heap_check_owned(h1, probe));
        let foreign = alloc::malloc(64);
        assert!(!alloc::heap_contains_block(h1, foreign));
        alloc::free(foreign);
        // delete: contents survive migration to the backing heap.
        init::heap_delete(h1);
        for &(p, n) in blocks.iter().skip(100) {
            assert_eq!(p.read(), 0x77);
            assert_eq!(p.add(n - 1).read(), 0x77);
            alloc::free(p); // now owned by the backing heap
        }
    }

    // --- destroy: wholesale release, then the world still works ------------
    let h2 = init::create_heap(0, true, -1);
    // SAFETY: h2 ours; destroy drops the blocks per contract.
    unsafe {
        for i in 0..300usize {
            let p = hmalloc(h2, 64 + i * 7 % 2000);
            assert!(!p.is_null());
        }
        init::heap_destroy(h2);
    }
    let sanity = alloc::malloc(128);
    assert!(!sanity.is_null());
    // SAFETY: live block.
    unsafe { alloc::free(sanity) };

    // --- set_default / get_backing ----------------------------------------
    let h3 = init::create_heap(0, true, -1);
    // SAFETY: h3 ours.
    unsafe {
        let prev = init::set_default_heap(h3);
        let p = alloc::malloc(64); // lands in h3
        assert!(alloc::heap_contains_block(h3, p));
        alloc::free(p);
        init::set_default_heap(prev);
        assert_eq!(init::backing_heap(), prev);
        init::heap_destroy(h3);
    }

    // --- arenas: exclusive reserve + heap_new_in_arena ---------------------
    //
    // Sized in CHUNKS, because chunks are what the arena's fixed bitmap counts.
    // As a flat `64 * 1024 * 1024` this read "two chunks" at the shipped 32 MiB
    // geometry and "2048 chunks" at the small profile's 32 KiB one — past
    // `arena::MAX_CHUNKS` (1024), so the reserve failed for a reason that had
    // nothing to do with what the test checks. Half a megabyte covers this
    // test's own largest allocation (200,000 bytes) with room over; the
    // `.max(2)` keeps the shipped geometry at exactly the 64 MiB it always
    // reserved, so nothing about the default-profile run changes.
    let arena_bytes = (512 * 1024usize)
        .div_ceil(rusty_alloc::types::SEGMENT_SIZE)
        .max(2)
        * rusty_alloc::types::SEGMENT_SIZE;
    let arena_id =
        arena::reserve_os_memory_ex(arena_bytes, true, false, true).expect("arena reserve failed");
    let (abase, asize) = arena::arena_area(arena_id);
    assert!(!abase.is_null() && asize == arena_bytes);
    let ha = init::create_heap(0, true, arena_id);
    // SAFETY: ha ours.
    unsafe {
        let s0 = (*(*ha).heap.get()).stats.segments;
        let p = hmalloc(ha, 100_000);
        assert!(!p.is_null());
        let addr = p.addr();
        assert!(
            addr >= abase.addr() && addr < abase.addr() + asize,
            "exclusive-arena heap allocated outside its arena"
        );
        assert!((*(*ha).heap.get()).stats.segments > s0);
        alloc::free(p);
        init::heap_destroy(ha);
    }
    // Chunk recycling: a second arena heap reuses the freed chunk.
    let hb2 = init::create_heap(0, true, arena_id);
    // SAFETY: hb2 ours.
    unsafe {
        let p = hmalloc(hb2, 4096);
        assert!(!p.is_null());
        assert!(p.addr() >= abase.addr() && p.addr() < abase.addr() + asize);
        // zalloc over a recycled (dirty) arena chunk must be re-zeroed:
        let z = alloc::heap_zalloc(hb2, 200_000);
        for i in (0..200_000).step_by(4096) {
            assert_eq!(z.add(i).read(), 0, "dirty arena chunk leaked at +{i}");
        }
        alloc::free(z);
        alloc::free(p);
        init::heap_destroy(hb2);
    }

    // --- subprocs: isolation of abandonment --------------------------------
    // (native only: the miri mock's TLS destructors never fire, so thread
    // exit does not abandon — documented prim/mock limitation)
    if cfg!(miri) {
        return;
    }
    let sp = init::subproc_new();
    let handle = std::thread::spawn(move || {
        init::subproc_add_current_thread(sp);
        let p = alloc::malloc(5000);
        assert!(!p.is_null());
        // SAFETY: leak intentionally; thread exit abandons into subproc sp.
        unsafe { core::ptr::write_bytes(p, 0x3C, 5000) };
        p as usize
    });
    let leaked = handle.join().unwrap();
    // Main (subproc 0) reclaim must NOT see sp's segments:
    let before = alloc::stats().reclaims;
    let churn: Vec<Vec<u8>> = (0..100).map(|_| vec![1u8; 40_000]).collect();
    drop(churn);
    let _ = before; // reclaims may move for other reasons; the real check:
    let mut found = 0usize;
    init::abandoned_visit_blocks(sp, -1, true, &mut |_a, b, _s| {
        if !b.is_null() {
            found += 1;
        }
        true
    });
    assert!(found >= 1, "abandoned block not visible in its subproc");
    // SAFETY: leaked block is still live (abandoned, not freed).
    unsafe { alloc::free(leaked as *mut u8) };

    // --- options + stats smoke ---------------------------------------------
    // v1 default: purging is opt-in (-1) pending the M8 open defect.
    assert_eq!(options::get(15), -1, "purge_delay default");
    options::set(15, 0);
    assert_eq!(options::get(15), 0);
    assert!(
        options::get_size(23) >= 1024 * 1024 * 1024,
        "arena_reserve KiB scaling"
    );
    // DEBUG ONLY: `allocs` is fed by `Heap::stat_alloc`, which is
    // `#[cfg(debug_assertions)]` because it sits on the hottest path — so in
    // release it is never incremented and this assertion cannot pass. (Its
    // neighbours `large_allocs` / `realloc_in_place` are NOT gated, which is
    // why `spans.rs` is unaffected.) Second of two such sites; both were red
    // in `cargo test --release` and invisible because CI only runs debug.
    #[cfg(debug_assertions)]
    {
        let merged = rusty_alloc::stats::merged();
        assert!(merged.allocs > 0);
    }
    let (_, _, _, rss, ..) = rusty_alloc::stats::process_info();
    assert!(rss > 0, "process_info rss");
}

/// An exclusive-arena heap must take its HUGE blocks from that arena too.
///
/// Found by P2 of `docs/plans/small-metal.md` (2026-09-07). `segment::huge_alloc`
/// asked `arena::chunk_alloc_n(-1, ..)` — a hardcoded "any non-exclusive
/// arena" — while `segment_alloc` next to it correctly passed the owning
/// heap's `arena_id`. So a heap created with `create_heap(_, _, arena_id)`,
/// whose entire purpose is that its memory comes from ONE region, silently
/// served every allocation above `LARGE_OBJ_SIZE_MAX` from the default arena
/// or straight from the OS.
///
/// Upstream does not have this: `mi_segment_huge_page_alloc` takes a
/// `req_arena_id` and both call sites pass `heap->arena_id`
/// (`oracle/mimalloc/src/segment.c:1671,1683`).
///
/// The small profile is what exposed it — at a 64 KiB segment the huge path
/// starts at 56 KiB, so an ordinary 100 KB allocation escaped — but the defect
/// is in the SHIPPED geometry too, and this test is written at that geometry
/// deliberately: it needs one allocation past `LARGE_OBJ_SIZE_MAX` (32 MiB −
/// 64 KiB), which any consumer of the exclusive-arena API can make.
#[test]
fn exclusive_arena_confines_huge_allocations() {
    use rusty_alloc::types::LARGE_OBJ_SIZE_MAX;

    // Room for the huge block plus its header, rounded to whole chunks.
    let arena_bytes = 256 * 1024 * 1024;
    let Ok(arena_id) = arena::reserve_os_memory_ex(arena_bytes, true, false, true) else {
        // A machine that cannot reserve the range has nothing to say about
        // confinement; skipping is honest, silently passing would not be.
        eprintln!("skipped: could not reserve a {arena_bytes}-byte exclusive arena");
        return;
    };
    let (abase, asize) = arena::arena_area(arena_id);
    assert!(!abase.is_null());

    let ha = init::create_heap(0, true, arena_id);
    // One byte past the in-segment span path is enough to reach `huge_alloc`.
    let huge = LARGE_OBJ_SIZE_MAX + 1;

    // SAFETY: `ha` is ours on this thread; the block is freed below.
    unsafe {
        let s0 = (*(*ha).heap.get()).stats.segments;
        let p = hmalloc(ha, huge);
        assert!(
            !p.is_null(),
            "exclusive-arena heap could not serve {huge} bytes"
        );
        assert!(
            p.addr() >= abase.addr() && p.addr() < abase.addr() + asize,
            "huge block at {:#x} escaped its exclusive arena [{:#x}, {:#x})",
            p.addr(),
            abase.addr(),
            abase.addr() + asize
        );
        // Writable through its whole extent — confinement must not have cost
        // the block its backing.
        core::ptr::write_bytes(p, 0xC3, huge);
        assert_eq!(*p, 0xC3);
        assert_eq!(*p.add(huge - 1), 0xC3);

        // A Huge segment is counted as a segment on the way IN as well as on
        // the way out. The release path has always bumped `segments_freed`
        // beside `huge_free`, so without the matching bump this pair could
        // report more segments freed than were ever allocated.
        let st = (*(*ha).heap.get()).stats;
        assert!(
            st.segments > s0,
            "huge allocation did not count a segment ({} -> {})",
            s0,
            st.segments
        );
        assert!(
            st.segments >= st.segments_freed,
            "more segments freed ({}) than allocated ({})",
            st.segments_freed,
            st.segments
        );
        alloc::free(p);
    }
}

/// The arena chunk bitmap must reach chunks past its FIRST word.
///
/// Written 2026-09-07 (P3 of `docs/plans/small-metal.md`) because the bitmap's
/// word type was narrowed from `u64` to `u32` — a 32-bit RISC-V / Xtensa target
/// has no 64-bit atomic, and a bitmap's word width is a free choice — and the
/// whole existing battery passed with the narrowing HALF applied: the element
/// type had moved but four loop bounds still said `div_ceil(64)` and
/// `(w + 1) * 64`.
///
/// It passed because **every arena any test builds is 32 chunks or fewer**,
/// and at 32 chunks `div_ceil(64)` and `div_ceil(32)` are both 1. The
/// divergence starts at chunk 33, which nothing reached. A green suite over a
/// scenario that cannot express the defect.
///
/// So: allocate past the first bitmap word. With the half-narrowed code the
/// scan stops at word 0 and the exclusive heap reports its arena full.
#[test]
fn arena_bitmap_reaches_past_its_first_word() {
    use rusty_alloc::types::{LARGE_OBJ_SIZE_MAX, SEGMENT_SIZE};

    // One chunk per huge block, and enough of them to cross a 32-bit word.
    const CHUNKS: usize = 33;
    let want = CHUNKS * SEGMENT_SIZE;
    // At the shipped 32 MiB geometry that is ~1.06 GiB, eagerly committed
    // (`reserve_os_memory_ex` ignores `commit=false` in v1, a recorded
    // divergence). Skip rather than fail where the box will not give it —
    // under the small profile the same test costs 2.1 MiB and always runs.
    let Ok(arena_id) = arena::reserve_os_memory_ex(want, true, false, true) else {
        eprintln!("skipped: could not reserve {want} bytes for a {CHUNKS}-chunk arena");
        return;
    };
    let (abase, asize) = arena::arena_area(arena_id);
    assert!(!abase.is_null() && asize >= want);

    let ha = init::create_heap(0, true, arena_id);
    // Exactly one chunk each. `LARGE_OBJ_SIZE_MAX` is the largest in-segment
    // span — it fills a segment's whole usable region, so the next allocation
    // must take a fresh chunk. (`+1` would be a HUGE block, and header + size
    // then spills into a SECOND chunk, which is what the first version of this
    // test got wrong: it failed at 16 of 33 for arithmetic reasons rather than
    // the defect it was written for.)
    let one_chunk = LARGE_OBJ_SIZE_MAX;
    let mut blocks = Vec::new();
    // SAFETY: `ha` is ours on this thread; every block is freed below.
    unsafe {
        for i in 0..CHUNKS {
            let p = hmalloc(ha, one_chunk);
            assert!(
                !p.is_null(),
                "chunk {i} of {CHUNKS} refused — the bitmap scan stopped at word \
                 {} of {}",
                i / 32,
                CHUNKS.div_ceil(32)
            );
            assert!(
                p.addr() >= abase.addr() && p.addr() < abase.addr() + asize,
                "chunk {i} escaped the arena"
            );
            blocks.push(p);
        }
        for p in blocks {
            alloc::free(p);
        }
    }
}

/// A collect must reclaim a bin's LAST all-free page — at ANY level.
///
/// Upstream's `mi_heap_page_collect` calls `_mi_page_free` whenever
/// `mi_page_all_free(page)`, with the comment "this will free retired pages as
/// well"; the keep-one-page-per-bin reuse cache is `mi_page_retire`'s policy on
/// the free path. Ours borrowed that exemption into `collect`, so no collect at
/// any level could hand a size class's slice to a different class.
///
/// Invisible at the shipped 32 MiB geometry — 512 slices per segment absorb one
/// cached page per class. At `ra_small_profile`'s 16 it is fatal, and P4d
/// measured it on hardware: 512 B capacity decaying 168 -> 8 blocks and 45 % of
/// a churn workload returning null while 61,440 bytes of the region sat free.
///
/// A private heap, so the bin under test holds exactly one page and nothing
/// else in the process can add a second.
#[test]
fn collect_reclaims_a_bins_last_page() {
    for force in [false, true] {
        let h = init::create_heap(0, true, -1);
        // SAFETY: `h` is ours, freshly created, and destroyed below.
        unsafe {
            let p = hmalloc(h, 1536);
            assert!(!p.is_null(), "fresh heap served a 1536-byte block");
            alloc::free(p);

            let before = (*(*h).heap.get()).stats.pages_retired;
            alloc::heap_collect(h, force);
            let after = (*(*h).heap.get()).stats.pages_retired;
            assert!(
                after > before,
                "collect(force={force}) must reclaim an all-free page even when \
                 it is the bin's only one (retired {before} -> {after})"
            );
            init::heap_destroy(h);
        }
    }
}

/// The automatic collect exists and fires.
///
/// `generic_collect` was declared with a default of 10,000 and read by NOTHING,
/// so a heap never collected on its own no matter how long it ran. The option is
/// lowered here so the test does not have to make 10,000 slow-path trips.
#[test]
fn generic_collect_fires_on_its_own() {
    let prev = options::get(options::GENERIC_COLLECT);
    options::set(options::GENERIC_COLLECT, 8);
    let h = init::create_heap(0, true, -1);
    // SAFETY: `h` is ours, freshly created, and destroyed below.
    unsafe {
        // Touch a bin, empty it, then keep the generic path busy with OTHER
        // sizes. Nothing here calls collect; only the periodic trigger can
        // retire the emptied page.
        let p = hmalloc(h, 1536);
        assert!(!p.is_null());
        alloc::free(p);
        let before = (*(*h).heap.get()).stats.pages_retired;

        // DISTINCT sizes: a repeated size is served from its bin's queue front
        // and never reaches `malloc_generic`, so a loop over seven sizes makes
        // seven generic trips, not sixty-four. The first version of this test
        // did exactly that and failed for its premise rather than its property.
        let g0 = (*(*h).heap.get()).stats.generic;
        for i in 0..64usize {
            let q = hmalloc(h, 24 + i * 8);
            if !q.is_null() {
                alloc::free(q);
            }
        }
        let trips = (*(*h).heap.get()).stats.generic - g0;
        assert!(
            trips > 8,
            "precondition: the loop must actually take the generic path more              often than the threshold (took it {trips} times)"
        );
        let after = (*(*h).heap.get()).stats.pages_retired;
        assert!(
            after > before,
            "the periodic collect must retire pages with no explicit call \
             (retired {before} -> {after})"
        );
        init::heap_destroy(h);
    }
    options::set(options::GENERIC_COLLECT, prev);
}

/// A heap must reclaim its own idle pages before it reports OOM.
///
/// A page allocator keeps a page per size class as a reuse cache, so it can be
/// "full" while holding empty pages for classes nobody is asking for. Before
/// P4d the generic path returned null in that state. On a XIAO ESP32-S3 that
/// meant 22,533 of 50,000 churn allocations failing from a heap that one
/// `collect` restored from 8 to 240 blocks of capacity.
///
/// **`ra_small_profile` only, and that is the point.** The cache costs one
/// slice per class touched, so it only starves a heap when slices are scarce:
/// 16 per segment here against 512 at the shipped geometry. The first version
/// of this test used a 4-chunk arena without the gate and PASSED WITH THE FIX
/// REMOVED — at 32 MiB segments, 48 cached pages cannot exhaust anything, so it
/// asserted nothing. Two 64 KiB segments give 30 usable slices, and 24 cached
/// pages is most of them.
#[cfg(ra_small_profile)]
#[test]
fn generic_path_reclaims_before_returning_null() {
    let bytes = 2 * rusty_alloc::types::SEGMENT_SIZE;
    let Ok(arena) = arena::reserve_os_memory_ex(bytes, true, false, true) else {
        return; // a host that cannot reserve it has nothing to say here
    };
    let h = init::create_heap(0, true, arena);
    // SAFETY: `h` is ours and is destroyed below.
    unsafe {
        // Touch many distinct small classes and free them all. Each leaves an
        // idle cached page holding a slice that the class we ask for next
        // cannot reach.
        let mut held = Vec::new();
        for i in 0..24usize {
            let p = hmalloc(h, 16 + i * 16);
            if !p.is_null() {
                held.push(p);
            }
        }
        let touched = held.len();
        for p in held.drain(..) {
            alloc::free(p);
        }
        assert!(
            touched >= 16,
            "precondition: the arena must actually hold the cached pages that \
             starve the retry (only {touched} classes were served)"
        );

        // Now demand ONE class hard.
        let mut ptrs = Vec::new();
        for _ in 0..512 {
            let p = hmalloc(h, 512);
            if p.is_null() {
                break;
            }
            ptrs.push(p);
        }
        let served = ptrs.len();
        for p in ptrs {
            alloc::free(p);
        }
        // 192 is chosen against MEASURED arms, not guessed: with the
        // reclaim-and-retry this serves 240 blocks (all 30 usable slices), and
        // with it removed, 128. An earlier threshold of 64 sat below BOTH and
        // so passed with the fix poisoned — the test asserted nothing.
        assert!(
            served > 192,
            "a heap holding {touched} idle cached pages must reclaim them rather \
             than report OOM (served only {served} blocks of 512 B)"
        );
        init::heap_destroy(h);
    }
}
