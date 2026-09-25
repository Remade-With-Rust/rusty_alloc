//! The environment pass still READS the environment now that it owns no
//! memory (`options::env`, `docs/plans/youslowbro.md` §4).
//!
//! The pass runs once per process, on the first option read, so the only way
//! to test it end to end is a CHILD with the variables set before it starts —
//! the same shape `foreign_free.rs` and `double_free.rs` use. The parent sets
//! both prefixes, both value grammars and a precedence conflict; the child
//! reads them back through the public `options::get`.

use std::process::Command;

use rusty_alloc::options::{get, get_size, is_enabled};

const MARKER: &str = "RUSTY_ALLOC_OPTIONS_ENV_CHILD";

// ABI indices (see `options::OPTION_NAMES`).
const MAX_ERRORS: usize = 19;
const MAX_WARNINGS: usize = 20;
const PURGE_DELAY: usize = 15;
const SHOW_STATS: usize = 1;
const ARENA_RESERVE: usize = 23;
const OS_TAG: usize = 18;
const MAX_SEGMENT_RECLAIM: usize = 21;

fn child() -> ! {
    // Touch the allocator first, as any real process does: the pass has run
    // by the time `get` is called, whichever order the harness chose.
    let v = rusty_alloc::alloc::malloc(64);
    assert!(!v.is_null());
    // SAFETY: live block, freed once.
    unsafe { rusty_alloc::alloc::free(v) };

    assert_eq!(get(MAX_ERRORS), 7, "RUSTY_ALLOC_MAX_ERRORS");
    assert_eq!(
        get(MAX_WARNINGS),
        11,
        "MIMALLOC_MAX_WARNINGS (compat prefix)"
    );
    assert_eq!(get(PURGE_DELAY), 5, "RUSTY_ALLOC_ wins over MIMALLOC_");
    assert!(is_enabled(SHOW_STATS), "MIMALLOC_SHOW_STATS=YES (any case)");
    assert_eq!(
        get_size(ARENA_RESERVE),
        4096 * 1024,
        "RUSTY_ALLOC_ARENA_RESERVE in KiB"
    );
    // The one-walk pass (Linux) must keep `getenv`'s semantics exactly.
    assert_eq!(
        get(OS_TAG),
        3,
        "a RUSTY_ALLOC_ value too long to be an option reads as unset: MIMALLOC_ applies"
    );
    assert_eq!(
        get(MAX_SEGMENT_RECLAIM),
        10,
        "a present but unparsable RUSTY_ALLOC_ value keeps the default; MIMALLOC_ is not read"
    );
    assert_eq!(get(MAX_ERRORS), 7, "a longer look-alike key does not match");
    // The exit CODE is the signal: libtest captures a test's stdout and never
    // flushes it through `process::exit`, and a plain 0 is also what a
    // harness that never reached this function would return.
    std::process::exit(CHILD_OK);
}

const CHILD_OK: i32 = 42;

#[test]
#[cfg_attr(miri, ignore)] // spawns a child process
fn environment_variables_reach_the_options_table() {
    if std::env::var(MARKER).is_ok() {
        child();
    }
    let exe = std::env::current_exe().expect("current_exe");
    let out = Command::new(exe)
        .env(MARKER, "1")
        .env("RUSTY_ALLOC_MAX_ERRORS", "7")
        .env("MIMALLOC_MAX_WARNINGS", " 11 ")
        .env("RUSTY_ALLOC_PURGE_DELAY", "5")
        .env("MIMALLOC_PURGE_DELAY", "9")
        .env("MIMALLOC_SHOW_STATS", "YES")
        .env("RUSTY_ALLOC_ARENA_RESERVE", "4096")
        .env("RUSTY_ALLOC_OS_TAG", "1".repeat(70))
        .env("MIMALLOC_OS_TAG", "3")
        .env("RUSTY_ALLOC_MAX_SEGMENT_RECLAIM", "abc")
        .env("MIMALLOC_MAX_SEGMENT_RECLAIM", "4")
        .env("RUSTY_ALLOC_MAX_ERRORSX", "99")
        .arg("--test-threads=1")
        .output()
        .expect("spawn child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(CHILD_OK),
        "child did not read its environment back\nstatus: {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status
    );
}
