#!/usr/bin/env bash
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
arm="${1:-ra}"
pre=""; case "$arm" in ra) pre="$RA";; mi) pre="$MI";; esac
out=/tmp/cgp.$arm.out; rm -f "$out"
cmd=(perl -e 'my %h; for my $i (1..150000) { $h{"key$i"} = [ $i, "v$i" ]; } my $s=0; while (my ($k,$v)=each %h) { $s += $v->[0] } print "$s\n";')
if [ -n "$pre" ]; then
  LD_PRELOAD="$pre" valgrind --tool=callgrind --callgrind-out-file="$out" --cache-sim=no --branch-sim=no "${cmd[@]}" >/dev/null 2>&1
else
  valgrind --tool=callgrind --callgrind-out-file="$out" --cache-sim=no --branch-sim=no "${cmd[@]}" >/dev/null 2>&1
fi
echo "== $arm allocator cost:"
callgrind_annotate --threshold=99 "$out" 2>/dev/null | grep -E 'PROGRAM TOTALS|rusty_alloc|mimalloc|tls_get|libc.so.6:(malloc|free|realloc)' | head -12
