//! `threads`: short-lived threads, each allocating a little and exiting —
//! a blocking pool's shape. Prices per-thread heap setup and teardown, which
//! no other workload here reaches. Argument: step count (threads = n / 100).
#[global_allocator]
static A: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;

fn main() {
    let n: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20000);
    let mut sink = 0u64;
    for t in 0..n / 100 {
        let h = std::thread::spawn(move || {
            let v: Vec<Box<u64>> = (0..16).map(|i| Box::new(i + t)).collect();
            v.iter().map(|b| **b).sum::<u64>()
        });
        sink = sink.wrapping_add(h.join().unwrap());
    }
    println!("{sink}");
}
