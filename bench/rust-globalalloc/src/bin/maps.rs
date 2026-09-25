//! `maps`: HashMap/HashSet churn (hashbrown allocates its tables 16-byte
//! aligned), formatted Strings, a BTreeMap and a growing `Vec<u128>`.
//! Argument: step count; the harness takes Ir(2n) - Ir(n).
use std::collections::{BTreeMap, HashMap, HashSet};

#[global_allocator]
static A: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;

fn main() {
    let n: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20000);
    let mut sink = 0u64;
    for r in 0..n / 1000 {
        // many small hash maps: hashbrown allocates with 16-byte alignment
        for k in 0..200u64 {
            let mut m: HashMap<u64, u64> = HashMap::new();
            for i in 0..(k % 13) { m.insert(i * 7 + r, i); }
            let s: HashSet<u32> = (0..(k % 7) as u32).collect();
            sink = sink.wrapping_add(m.len() as u64 + s.len() as u64);
        }
        // strings and vectors that grow (realloc)
        let mut v: Vec<String> = Vec::new();
        for i in 0..400u64 { v.push(format!("item-{i}-{r}")); }
        let mut b: BTreeMap<String, usize> = BTreeMap::new();
        for (i, s) in v.iter().enumerate() { b.insert(s.clone(), i); }
        let w: Vec<u128> = (0..(r % 50) as u128).collect();
        sink = sink.wrapping_add(b.len() as u64 + w.len() as u64);
    }
    println!("{sink}");
}
