#!/usr/bin/env bash
# The wasm size ratchet.
#
# An integrator reported rusty_alloc adding ~12% to their gzipped bundle, and
# the cause had been sitting in the crate for its whole life: the option
# environment pass ran on `wasm32-unknown-unknown`, where `std::env::var` is a
# stub that always fails. `options::get` was the LARGEST function in a wasm
# module at 3,708 bytes -- ahead of anything in the allocator proper -- and it
# dragged `to_uppercase`, `alloc::fmt::format` and the whole `OPTION_NAMES`
# table along with it.
#
# Nothing measured wasm size, so nothing could notice. This is what notices.
#
# GZIPPED, because that is what a browser downloads and what the report was in.
# The distribution profile (`bench-dist`), because the repo's `release` keeps
# debug symbols on purpose and a 2 MB artifact hides a 4 KB regression.
#
# Update the baseline deliberately, in the same commit as the change that moves
# it, the way `tools/unsafe-census.sh` is updated -- a size increase is allowed,
# it just has to be meant.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

BASELINE_FILE="tools/wasm-size-baseline.txt"
ART="target/wasm32-unknown-unknown/bench-dist/rusty_alloc_wasm.wasm"
# Headroom for toolchain drift: rustc's own codegen moves a little between
# patch releases, and a gate that cries wolf gets disabled.
TOLERANCE_PCT=3

update=0
[ "${1:-}" = "--update" ] && update=1

echo "building the wasm fixture (bench-dist: no debug info)"
cargo build -p rusty_alloc-wasm --target wasm32-unknown-unknown --profile bench-dist \
  >/dev/null 2>&1 || { echo "build FAILED" >&2; exit 1; }
[ -f "$ART" ] || { echo "missing $ART" >&2; exit 1; }

gz=$(python3 -c "import gzip,sys;print(len(gzip.compress(open(sys.argv[1],'rb').read(),9)))" "$ART")
raw=$(python3 -c "import os,sys;print(os.path.getsize(sys.argv[1]))" "$ART")

if [ "$update" = "1" ]; then
  printf '%s\n' "$gz" > "$BASELINE_FILE"
  echo "baseline updated: $gz bytes gzipped (raw $raw)"
  exit 0
fi

if [ ! -f "$BASELINE_FILE" ]; then
  echo "no baseline; run: bash tools/wasm-size.sh --update" >&2
  exit 1
fi
base=$(tr -d '[:space:]' < "$BASELINE_FILE")
limit=$(( base + base * TOLERANCE_PCT / 100 ))

echo "wasm size: $gz bytes gzipped (raw $raw), baseline $base, limit $limit (+${TOLERANCE_PCT}%)"
if [ "$gz" -gt "$limit" ]; then
  echo
  echo "WASM SIZE RATCHET FAILED: $((gz - base)) bytes gzipped over the baseline."
  echo
  echo "That is not automatically wrong -- an allocator legitimately grows -- but"
  echo "it must be DELIBERATE. Every byte here is downloaded by every visitor to"
  echo "every page that ships this crate. Find it with a set difference against a"
  echo "baseline module rather than guessing (docs/plans/wasm-size.md), then"
  echo "re-run with --update in the same commit."
  exit 1
fi
if [ "$gz" -lt "$((base - base * TOLERANCE_PCT / 100))" ]; then
  echo "wasm got $((base - gz)) bytes SMALLER; re-run with --update to bank it."
fi
echo "WASM SIZE OK"
