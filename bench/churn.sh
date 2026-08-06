#!/usr/bin/env bash
# Run the thread-churn hazard probe (bench/churn.c) against our allocator.
# Usage: churn.sh [runs]
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
bin="$HOME/ra_churn"
runs="${1:-5}"

gcc -O2 -o "$bin" "$root/bench/churn.c" -lpthread || exit 1

fail=0
for i in $(seq 1 "$runs"); do
  if ! LD_PRELOAD="$RA" "$bin"; then
    echo "FAILED on run $i"
    fail=1
  fi
done
[ "$fail" -eq 0 ] && echo "churn probe: $runs/$runs clean"
exit "$fail"
