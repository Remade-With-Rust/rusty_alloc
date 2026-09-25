#!/usr/bin/env bash
# Whole-program Ir per `n` steps of the two Rust GlobalAlloc workloads
# (bench/rust-globalalloc), by the two-point estimator Ir(2n) - Ir(n), plus
# .text size. Linux/WSL with valgrind. usage: bench/rust-globalalloc.sh [n]
#
# Read WHOLE-program Ir here, not an allocator-only filter: with the
# GlobalAlloc methods inline, rustc places the allocator fast paths inside
# the program's own functions, where a per-file filter cannot see them.
# The workloads print a checksum; it must match between the arms.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
n=${1:-20000}
tgt=${CARGO_TARGET_DIR:-$root/target/rust-globalalloc}
CARGO_TARGET_DIR=$tgt cargo build --release -q --manifest-path "$root/bench/rust-globalalloc/Cargo.toml"
for b in maps trees threads overaligned; do
  bin=$tgt/release/$b
  ir() { local o; o=$(mktemp); valgrind --tool=callgrind --callgrind-out-file="$o" --cache-sim=no --branch-sim=no "$bin" "$1" >/dev/null 2>&1; grep -m1 '^summary:' "$o" | awk '{print $2}'; rm -f "$o"; }
  a=$(ir "$n"); c=$(ir $((n * 2)))
  printf '%-6s Ir/%d steps %12d   text %8d   checksum %s\n' "$b" "$n" $((c - a)) \
    "$(size -A "$bin" | awk '$1==".text"{print $2}')" "$("$bin" "$n")"
done
