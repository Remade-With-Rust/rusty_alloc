//! `vec_global`: the collection work of `alloc_work.rs` on `std::alloc::Global`,
//! which reaches rusty_alloc through `#[global_allocator]`. Argument: steps.
#[global_allocator]
static A: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;

#[path = "../alloc_work.rs"]
mod alloc_work;

fn main() {
    println!("{}", alloc_work::work(std::alloc::Global, alloc_work::steps()));
}
