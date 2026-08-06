#!/usr/bin/env bash
# Per-FUNCTION instruction profile for one opscan op, both arms side by side.
#
# callcount.sh answers "how often"; this answers "where do the instructions go".
# Together they separate a path-FREQUENCY problem (we take the slow path more)
# from a path-COST problem (the same path costs us more) - a distinction that
# has already refuted two hypotheses in this campaign.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
BIN="$HOME/ra_opscan"
gcc -O2 -o "$BIN" "$root/bench/opscan.c" || exit 1

op="${1:-batch_lifo}"
n="${2:-100000}"

for arm in ra mi; do
    pre=$RA; [ "$arm" = mi ] && pre=$MI
    out=$(mktemp)
    LD_PRELOAD="$pre" valgrind --tool=callgrind --callgrind-out-file="$out" \
        --cache-sim=no --branch-sim=no "$BIN" "$op" "$n" >/dev/null 2>&1
    echo "== $arm : op '$op', N=$n  (Ir, and Ir/op)"
    callgrind_annotate --threshold=92 "$out" 2>/dev/null \
        | grep -E 'rusty_alloc|mimalloc|PROGRAM TOTALS|:malloc|:free|mi_' \
        | head -16 \
        | awk -v n="$n" '{ ir=$1; gsub(/,/,"",ir); if (ir+0>0) printf "%14s  %8.2f/op  %s\n", $1, ir/n, substr($0, index($0,$3)) }'
    echo
    rm -f "$out"
done
