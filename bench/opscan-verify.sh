#!/usr/bin/env bash
# Cross-check the opscan two-point estimator with an INDEPENDENT instrument.
#
# opscan makes exactly N malloc and N free calls by construction, so summing
# every instruction attributed to the allocator's shared object and dividing by
# N gives Ir/op directly - no differencing, no assumptions about fixed costs.
#
# If the two estimators disagree, one of them is measuring something other than
# what it is labelled, and NEITHER number may be used until that is resolved.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
BIN="$HOME/ra_opscan"
gcc -O2 -o "$BIN" "$root/bench/opscan.c" || exit 1

op="${1:-small}"
n="${2:-200000}"

echo "== per-object allocator cost for op '$op', N=$n (exactly N malloc + N free)"
for arm in ra mi; do
    pre=$RA; obj="rusty_alloc_override"
    if [ "$arm" = mi ]; then pre=$MI; obj="libmimalloc"; fi
    out=$(mktemp)
    LD_PRELOAD="$pre" valgrind --tool=callgrind --callgrind-out-file="$out" \
        --cache-sim=no --branch-sim=no "$BIN" "$op" "$n" >/dev/null 2>&1
    total=$(grep -m1 '^summary:' "$out" | awk '{print $2}')
    # Sum Ir of every line attributed to the allocator's object file.
    alloc=$(callgrind_annotate --threshold=100 "$out" 2>/dev/null \
        | grep "$obj" \
        | awk '{gsub(/,/,"",$1); s+=$1} END{print s+0}')
    rm -f "$out"
    awk -v arm="$arm" -v t="$total" -v a="$alloc" -v n="$n" 'BEGIN{
        printf "%-3s total=%d  in-allocator=%d  -> %.2f Ir/op (alloc only)\n", arm, t, a, a/n
    }'
done
echo
echo "Compare the 'alloc only' figures with opscan.sh's Ir/op for the same op."
echo "opscan.sh additionally counts the caller-side loop and PLT overhead, which"
echo "is identical across arms, so the DIFFERENCE between arms must agree."
