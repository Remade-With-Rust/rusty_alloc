//! With purging on, a freed HUGE block must give its pages back to the OS.
//!
//! `docs/plans/huge-free-retention.md`, mechanism 3: `span_free` purges a
//! freed span, but `huge_free` returned a dead block's chunks to the arena
//! resident, whatever `purge_delay` said. A consumer that loaded Whisper small
//! (a 152 MiB embedding) kept ~150 MB over the system allocator with purging
//! on, for the life of the process.
//!
//! Own test binary: it reads the process's resident memory, which any test
//! running alongside would move.

use rusty_alloc::alloc::{free, malloc};
use rusty_alloc::stats::process_info;
use rusty_alloc::types::LARGE_OBJ_SIZE_MAX;

fn rss() -> usize {
    process_info().3
}

#[test]
fn a_freed_huge_block_is_purged_when_purging_is_on() {
    rusty_alloc::options::set(15, 0); // purge_delay: purge at once
    // Past LARGE_OBJ_SIZE_MAX, so a dedicated huge segment from the arena.
    let size = 3 * LARGE_OBJ_SIZE_MAX;
    let p = malloc(size);
    assert!(!p.is_null());
    // SAFETY: a live block of `size` bytes; every page made resident.
    unsafe { core::ptr::write_bytes(p, 0x5A, size) };
    let live = rss();
    // SAFETY: freed once.
    unsafe { free(p) };
    let after = rss();
    rusty_alloc::options::set(15, -1); // restore the shipped default
    if live == 0 {
        return; // no RSS reporting on this platform (miri, wasm, bare metal)
    }
    let returned = live.saturating_sub(after);
    assert!(
        returned >= size / 2,
        "freeing a {size}-byte huge block returned only {returned} bytes \
         (resident {live} -> {after}); its pages stayed in the arena"
    );
}
