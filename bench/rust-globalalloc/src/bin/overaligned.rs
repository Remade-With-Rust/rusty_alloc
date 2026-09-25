//! `overaligned`: buffers of cache-line-aligned elements (align 64, above
//! what the size classes guarantee) that grow, shrink to fit and are
//! re-reserved — the `GlobalAlloc::realloc` arm for alignments above two
//! words. Argument: step count.
#[global_allocator]
static A: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;

#[derive(Clone, Copy)]
#[repr(align(64))]
struct Line([u64; 8]);

fn main() {
    let n: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20000);
    let mut sink = 0u64;
    for r in 0..n / 20 {
        let mut v: Vec<Line> = Vec::with_capacity(4);
        for i in 0..(r % 29 + 3) {
            v.push(Line([i; 8]));
        }
        // Shrink to what is used (in place when at least half stays), then
        // grow back by a little (in place within the class's slack).
        v.shrink_to_fit();
        v.truncate(v.len() * 3 / 4);
        v.shrink_to_fit();
        v.reserve_exact(1);
        v.push(Line([r; 8]));
        sink = sink.wrapping_add(v.iter().map(|l| l.0[0]).sum::<u64>() + v.capacity() as u64);
    }
    println!("{sink}");
}
