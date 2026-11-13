//! `vec_direct`: the collection work of `alloc_work.rs` on `RustyAlloc` as an
//! `Allocator`, called directly and not through the `__rust_alloc` shims.
//! The global allocator is rusty_alloc too, so everything outside the
//! collections is the same program as `vec_global`. Argument: steps.
#[global_allocator]
static A: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;

#[path = "../alloc_work.rs"]
mod alloc_work;

fn main() {
    println!("{}", alloc_work::work(rusty_alloc_api::RustyAlloc, alloc_work::steps()));
}
