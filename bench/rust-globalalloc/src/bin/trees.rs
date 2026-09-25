//! `trees`: no hashing. Boxed trees, growing
//! byte buffers, `Rc` graphs and short-lived `Vec<Vec<u32>>`, the allocation
//! pattern of a parser or an AST pass.
use std::rc::Rc;

#[global_allocator]
static A: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;

enum Node { Leaf(u64), Pair(Box<Node>, Box<Node>) }

fn build(d: u32, s: u64) -> Box<Node> {
    if d == 0 { Box::new(Node::Leaf(s)) } else { Box::new(Node::Pair(build(d - 1, s * 3), build(d - 1, s + 1))) }
}
fn sum(n: &Node) -> u64 { match n { Node::Leaf(v) => *v, Node::Pair(a, b) => sum(a).wrapping_add(sum(b)) } }

fn main() {
    let n: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20000);
    let mut sink = 0u64;
    for r in 0..n / 1000 {
        for k in 0..20 { sink = sink.wrapping_add(sum(&build(8, r + k))); }
        let mut buf: Vec<u8> = Vec::new();
        for i in 0..3000u64 { buf.extend_from_slice(&i.to_le_bytes()[..(i % 8) as usize]); }
        let shared: Vec<Rc<Vec<u32>>> = (0..300).map(|i| Rc::new(vec![i; (i % 17) as usize])).collect();
        let copies: Vec<Rc<Vec<u32>>> = shared.iter().cloned().collect();
        let rows: Vec<Vec<u32>> = (0..200u32).map(|i| (0..i % 23).collect()).collect();
        sink = sink.wrapping_add(buf.len() as u64 + copies.len() as u64 + rows.iter().map(|v| v.len() as u64).sum::<u64>());
    }
    println!("{sink}");
}
