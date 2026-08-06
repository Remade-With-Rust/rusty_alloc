//! G2: bin geometry pinned to the oracle. Loads the C mimalloc v2.4.5 library
//! and asserts `mi_good_size` equality over EVERY size in the binned range.
//! Skips loudly when the oracle library is absent (build it: oracle/build.ps1).

#![cfg(not(miri))]

fn oracle_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("RA_ORACLE_LIB") {
        return Some(p.into());
    }
    // OS-namespaced out dirs (the repo is shared between Windows and WSL2);
    // only this OS's artifacts are loadable.
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../oracle/out");
    #[cfg(windows)]
    let cands = ["win/mi/Release/mimalloc.dll"];
    #[cfg(target_os = "linux")]
    let cands = ["linux/mi/libmimalloc.so", "linux/mi/libmimalloc.so.2.4"];
    #[cfg(target_os = "macos")]
    let cands = ["darwin/mi/libmimalloc.dylib"];
    for cand in cands {
        let p = out.join(cand);
        if p.exists() {
            // Absolute path required: Windows resolves the DLL's own-directory
            // dependencies (mimalloc-redirect.dll) only for absolute loads.
            return p.canonicalize().ok();
        }
    }
    None
}

#[test]
fn g2_good_size_matches_oracle() {
    let Some(path) = oracle_path() else {
        eprintln!(
            "G2 SKIPPED: oracle library not found — build with oracle/build.ps1 or set RA_ORACLE_LIB"
        );
        return;
    };
    // Windows: mimalloc.dll depends on mimalloc-redirect.dll next to it;
    // pre-load it so the loader finds it regardless of search-path rules.
    // SAFETY: loading the pinned oracle build's own redirect shim.
    let _redirect = path
        .parent()
        .map(|d| d.join("mimalloc-redirect.dll"))
        .filter(|p| p.exists())
        .and_then(|p| unsafe { libloading::Library::new(p) }.ok());
    // SAFETY: loading the pinned oracle build; its only side effects are
    // mimalloc's process init.
    let lib = unsafe { libloading::Library::new(&path) }
        .unwrap_or_else(|e| panic!("load oracle {}: {e}", path.display()));
    // SAFETY: mi_good_size has this exact C signature in v2.4.5.
    let mi_good_size: libloading::Symbol<unsafe extern "C" fn(usize) -> usize> =
        unsafe { lib.get(b"mi_good_size") }.expect("mi_good_size symbol");
    // SAFETY: as above.
    let mi_version: libloading::Symbol<unsafe extern "C" fn() -> i32> =
        unsafe { lib.get(b"mi_version") }.expect("mi_version symbol");
    // SAFETY: no preconditions.
    let ver = unsafe { mi_version() };
    assert_eq!(
        ver, 20405,
        "oracle is not v2.4.5 — repin before trusting G2"
    );

    let mut mismatches = 0u32;
    for size in 1..=rusty_alloc::types::MEDIUM_OBJ_SIZE_MAX {
        // SAFETY: pure function of size.
        let oracle = unsafe { mi_good_size(size) };
        let ours = rusty_alloc::good_size(size);
        if oracle != ours {
            mismatches += 1;
            if mismatches <= 10 {
                eprintln!("G2 mismatch: size {size}: oracle {oracle}, ours {ours}");
            }
        }
    }
    assert_eq!(
        mismatches,
        0,
        "G2 FAIL: {mismatches} good_size mismatches in 1..={} — bin geometry diverges from oracle",
        rusty_alloc::types::MEDIUM_OBJ_SIZE_MAX
    );
    println!(
        "G2 PASS: good_size identical to oracle v2.4.5 for every size 1..={} ({})",
        rusty_alloc::types::MEDIUM_OBJ_SIZE_MAX,
        path.display()
    );
}
