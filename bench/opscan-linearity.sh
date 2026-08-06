#!/usr/bin/env bash
# Validate the opscan estimator: Ir must be LINEAR in the iteration count.
#
# The two-point estimator (Ir(2N)-Ir(N))/N is only meaningful if each extra
# iteration costs the same. This measures Ir at N, 2N and 3N and compares the
# two successive differences. If they disagree by more than a hair, the op has
# per-iteration state growth (page refills, retires, purge decisions) and its
# Ir/op number is an average over a changing workload, not a per-op cost.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
BIN="$HOME/ra_opscan"
gcc -O2 -o "$BIN" "$root/bench/opscan.c" || exit 1

op="${1:-small}"
n="${2:-100000}"

ir() {
    local pre="$1" iters="$2" out
    out=$(mktemp)
    LD_PRELOAD="$pre" valgrind --tool=callgrind --callgrind-out-file="$out" \
        --cache-sim=no --branch-sim=no "$BIN" "$op" "$iters" >/dev/null 2>&1
    grep -m1 '^summary:' "$out" | awk '{print $2}'
    rm -f "$out"
}

echo "== linearity of op '$op' at N=$n"
for arm in ra mi; do
    pre=$RA; [ "$arm" = mi ] && pre=$MI
    a=$(ir "$pre" "$n"); b=$(ir "$pre" $((n*2))); c=$(ir "$pre" $((n*3)))
    awk -v arm="$arm" -v a="$a" -v b="$b" -v c="$c" -v n="$n" 'BEGIN{
        d1=(b-a)/n; d2=(c-b)/n;
        printf "%-3s Ir(N)=%d Ir(2N)=%d Ir(3N)=%d | slope1=%.2f slope2=%.2f | drift=%.2f%%\n",
               arm, a, b, c, d1, d2, (d1>0? 100*(d2-d1)/d1 : 0)
    }'
done
