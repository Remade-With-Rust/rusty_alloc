//! The M2 keystone gate: rusty_alloc as this binary's REAL global allocator.
//! Every Vec, String, HashMap, BTreeMap, and spawned thread in these tests —
//! plus the test harness itself — allocates through our code.

use std::collections::{BTreeMap, HashMap};

#[global_allocator]
static ALLOC: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;

#[test]
fn collections_churn() {
    let mut map: HashMap<String, Vec<u64>> = HashMap::new();
    for i in 0..50_000u64 {
        map.entry(format!("key-{}", i % 1000)).or_default().push(i);
    }
    assert_eq!(map.len(), 1000);
    let total: u64 = map.values().map(|v| v.len() as u64).sum();
    assert_eq!(total, 50_000);

    let mut tree: BTreeMap<u64, String> = BTreeMap::new();
    for i in 0..10_000u64 {
        tree.insert(i, "x".repeat((i % 200) as usize));
    }
    let mid = tree.split_off(&5000);
    assert_eq!(tree.len(), 5000);
    assert_eq!(mid.len(), 5000);
}

#[test]
fn grow_shrink_vectors() {
    // Exercises GlobalAlloc::realloc (default alloc+copy+dealloc over our fns).
    let mut vs: Vec<Vec<u8>> = Vec::new();
    for round in 0..50 {
        for i in 0..100 {
            let mut v = Vec::with_capacity(8);
            v.resize(8 + (round * 37 + i * 13) % 5000, (i % 251) as u8);
            vs.push(v);
        }
        vs.retain(|v| v.len() % 3 != 0);
    }
    let checksum: usize = vs.iter().map(|v| v.len()).sum();
    assert!(checksum > 0);
}

#[test]
fn cross_thread_free_under_lock() {
    // M2's global lock must make alloc-on-A/free-on-B safe (lock-free version
    // is M4). This is exactly what cargo test's own thread pool does too.
    let data: Vec<Vec<u8>> = (0..1000).map(|i| vec![i as u8; 100 + i % 900]).collect();
    let handles: Vec<_> = data
        .chunks(250)
        .map(|c| {
            let owned: Vec<Vec<u8>> = c.to_vec(); // allocated on main thread
            std::thread::spawn(move || {
                let sum: usize = owned.iter().map(|v| v.len()).sum();
                drop(owned); // freed on worker thread
                sum
            })
        })
        .collect();
    let total: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
    assert_eq!(total, (0..1000).map(|i| 100 + i % 900).sum::<usize>());
}

#[test]
fn big_boxes() {
    // Cross the huge-path boundary (>128 KiB) and back.
    for size in [64 * 1024, 200 * 1024, 2 * 1024 * 1024, 40 * 1024 * 1024] {
        let v = vec![0xA5u8; size];
        assert_eq!(v[size / 2], 0xA5);
        drop(v);
    }
}

#[test]
fn mleak_threads_abandon_and_memory_survives() {
    // The mleak shape: workers allocate and EXIT while their blocks live; the
    // memory must survive abandonment, be freeable from main (NEVER-flag
    // remote path), and the segments must be adoptable afterwards.
    let mut all: Vec<(usize, usize, u8)> = Vec::new();
    for round in 0..4 {
        let handle = std::thread::spawn(move || {
            let mut owned = Vec::new();
            for i in 0..500usize {
                let size = 64 + (i * 37 + round * 11) % 4000;
                let p = rusty_alloc::alloc::malloc(size);
                assert!(!p.is_null());
                let tag = ((round * 500 + i) as u8) | 1;
                // SAFETY: fresh block of ≥ size bytes.
                unsafe { core::ptr::write_bytes(p, tag, size) };
                owned.push((p as usize, size, tag));
            }
            owned // thread exits with every block still live → abandonment
        });
        all.extend(handle.join().unwrap());
    }
    // Verify contents survived the owners' deaths, then free from main.
    for &(addr, size, tag) in &all {
        let p = addr as *const u8;
        // SAFETY: blocks are live; their pages are abandoned, not freed.
        unsafe {
            assert_eq!(p.read(), tag, "abandoned block lost its contents");
            assert_eq!(p.add(size - 1).read(), tag);
        }
    }
    for (addr, _, _) in all {
        // SAFETY: live tracked block, freed once (routes via the NEVER path).
        unsafe { rusty_alloc::alloc::free(addr as *mut u8) };
    }
    // Allocation pressure from main must be able to adopt what remains.
    let before = rusty_alloc::alloc::stats().reclaims;
    let v: Vec<Vec<u8>> = (0..200).map(|i| vec![7u8; 1000 + i * 40]).collect();
    drop(v);
    let after = rusty_alloc::alloc::stats().reclaims;
    // Not asserting a count: adoption triggers only when the main heap needs
    // spans; the churn above makes it overwhelmingly likely but not certain.
    println!("mleak: reclaims delta = {}", after - before);
}

#[test]
fn work_parity_counters_move() {
    let before = rusty_alloc::alloc::stats();
    let v: Vec<Box<u64>> = (0..10_000).map(Box::new).collect();
    drop(v);
    let after = rusty_alloc::alloc::stats();
    assert!(
        after.allocs - before.allocs >= 10_000,
        "counters must observe the work"
    );
    assert!(after.frees - before.frees >= 10_000);
}
