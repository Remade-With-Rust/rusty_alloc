#!/usr/bin/env bash
# Per-FUNCTION instruction profile of the allocator under a real workload.
# This is codec-analyzer's stage profiler, except the numbers are deterministic
# instruction counts rather than sampled time - so on a noisy box it still
# answers "which of OUR functions burns the instructions", exactly.
#
# Usage: icount-profile.sh <ra|mi|sys> [top-N]
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
arm="${1:-ra}"; top="${2:-18}"
pre=""; case "$arm" in ra) pre="$RA";; mi) pre="$MI";; esac
out=/tmp/cg.$arm.out
rm -f "$out"

cmd=(lua5.4 -e "local t={} for i=1,120000 do t[#t+1]=tostring(i)..'x' end local m={} for i=1,60000 do m['k'..i]={i,i*2,tostring(i)} end local s=0 for k,v in pairs(m) do s=s+v[2] end print(#t,s)")

if [ -n "$pre" ]; then
  LD_PRELOAD="$pre" valgrind --tool=callgrind --callgrind-out-file="$out" \
    --cache-sim=no --branch-sim=no "${cmd[@]}" >/dev/null 2>&1
else
  valgrind --tool=callgrind --callgrind-out-file="$out" \
    --cache-sim=no --branch-sim=no "${cmd[@]}" >/dev/null 2>&1
fi

echo "== $arm : top $top functions by instructions (allocator symbols marked *)"
callgrind_annotate --threshold=95 "$out" 2>/dev/null \
  | sed -n '/Ir *file:function/,/^--/p' \
  | head -n "$((top + 4))" \
  | awk '{
      line=$0
      if (line ~ /rusty_alloc|mimalloc|malloc|free|_mi_|alloc/) printf "* %s\n", line
      else print line
    }'
