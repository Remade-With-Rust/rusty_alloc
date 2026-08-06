#!/usr/bin/env bash
# CEILING PROBE (six-whys rebuild rule 1: measure what a perfect fix is worth
# BEFORE building it).
#
# Finding that motivates it: in a cdylib loaded via LD_PRELOAD, Rust's
# thread_local! compiles to the GENERAL-DYNAMIC TLS model, so every access is a
# call into ld.so's __tls_get_addr. The per-function instruction profile put
# __tls_get_addr at 12.97M Ir (1.96% of the whole program) - comparable to our
# entire free(). The earlier ns-level TLS probe missed this because it measured
# TLS inside an EXECUTABLE (local-exec: a register offset, no call), which is
# not the artifact we ship.
#
# This builds the same code with -Z tls-model=initial-exec (no __tls_get_addr;
# needs nightly) and reports the instruction delta. That is the prize; the
# stable-Rust brick is judged against it.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
source ~/.cargo/env 2>/dev/null || true
cd "$root"

echo "building initial-exec variant..."
RUSTFLAGS='-Z tls-model=initial-exec' CARGO_TARGET_DIR="$HOME/ra_ie" \
  cargo +nightly build --release -p rusty_alloc-override 2>&1 | grep -E '^error' | head -5

IE="$HOME/ra_ie/release/librusty_alloc_override.so"
GD="$HOME/ra_target/release/librusty_alloc_override.so"
MI="$root/oracle/out/linux/mi/libmimalloc.so"
for f in "$IE" "$GD" "$MI"; do
  [ -f "$f" ] || { echo "missing: $f" >&2; exit 2; }
done

cmd=(lua5.4 -e "local t={} for i=1,120000 do t[#t+1]=tostring(i)..'x' end local m={} for i=1,60000 do m['k'..i]={i,i*2,tostring(i)} end local s=0 for k,v in pairs(m) do s=s+v[2] end print(#t,s)")

icount() {
  local pre="$1"; shift
  local err; err=$(mktemp)
  LD_PRELOAD="$pre" valgrind --tool=callgrind --callgrind-out-file=/dev/null \
    --cache-sim=no --branch-sim=no "$@" >/dev/null 2>"$err"
  grep -oP 'refs:\s+\K[0-9,]+' "$err" | tr -d ','
  rm -f "$err"
}

gd=$(icount "$GD" "${cmd[@]}")
ie=$(icount "$IE" "${cmd[@]}")
mi=$(icount "$MI" "${cmd[@]}")
awk -v g="$gd" -v i="$ie" -v m="$mi" 'BEGIN{
  printf "general-dynamic (shipping): %d Ir   ra/mi %.4f\n", g, g/m
  printf "initial-exec    (probe)   : %d Ir   ra/mi %.4f\n", i, i/m
  printf "mimalloc                  : %d Ir\n", m
  printf "PRIZE: %.2f%% of our instructions, closing %.0f%% of the gap to mimalloc\n", 100*(g-i)/g, 100*(g-i)/(g-m)
}'
