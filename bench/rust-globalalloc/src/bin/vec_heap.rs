//! `vec_heap`: the collection work of `alloc_work.rs` inside a first-class
//! heap, borrowed by every collection. Argument: steps.
#[global_allocator]
static A: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;

#[path = "../alloc_work.rs"]
mod alloc_work;

fn main() {
    let heap = rusty_alloc_api::Heap::new();
    println!("{}", alloc_work::work(&heap, alloc_work::steps()));
}
