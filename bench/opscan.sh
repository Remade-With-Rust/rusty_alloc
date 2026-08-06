#!/usr/bin/env bash
# Per-operation side-by-side scan: rusty_alloc vs mimalloc vs glibc.
#
# METHOD (deterministic — no clock, no noise, no pinning needed):
#   For every op and every arm, run the SAME binary under callgrind at N and 2N
#   iterations and report
#       Ir/op = (Ir(2N) - Ir(N)) / N
#   The subtraction cancels process startup, ld.so, allocator init and
#   first-touch warmup EXACTLY, so no null arm has to be estimated. Work parity
#   is structural: one binary, identical caller code, only LD_PRELOAD differs.
#
# Output: Ir/op per arm plus ra-vs-mi delta and ratio, sorted worst-first, so
# the table doubles as the optimization ranking.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
BIN="$HOME/ra_opscan"

gcc -O2 -o "$BIN" "$root/bench/opscan.c" || exit 1

# op:N  — N chosen so a callgrind run stays a few seconds.
OPS="${OPS:-small:200000 small_touch:200000 med:200000 big:100000 large:20000 huge:2000 calloc:200000 batch_lifo:200000 batch_fifo:200000 realloc:100000 aligned:200000 usable:400000 mixed:200000}"

# Ir for one (arm, op, iters).
run_ir() {
    local pre="$1" op="$2" iters="$3"
    local out; out=$(mktemp)
    if [ -z "$pre" ]; then
        valgrind --tool=callgrind --callgrind-out-file="$out" \
            --cache-sim=no --branch-sim=no "$BIN" "$op" "$iters" >/dev/null 2>&1
    else
        LD_PRELOAD="$pre" valgrind --tool=callgrind --callgrind-out-file="$out" \
            --cache-sim=no --branch-sim=no "$BIN" "$op" "$iters" >/dev/null 2>&1
    fi
    # "summary:" is the whole-run Ir total in the callgrind file.
    local ir
    ir=$(grep -m1 '^summary:' "$out" | awk '{print $2}')
    rm -f "$out"
    echo "${ir:-0}"
}

per_op() { # arm-preload, op, N  -> Ir per operation
    local pre="$1" op="$2" n="$3"
    local a b
    a=$(run_ir "$pre" "$op" "$n")
    b=$(run_ir "$pre" "$op" "$((n * 2))")
    awk -v a="$a" -v b="$b" -v n="$n" 'BEGIN{ printf "%.2f", (b-a)/n }'
}

echo "METHOD: callgrind Ir, two-point estimator (Ir(2N)-Ir(N))/N — cancels startup+warmup"
echo
printf "%-13s %10s %10s %10s %10s %8s\n" op ra mi glibc "ra-mi" "ra/mi"
printf "%-13s %10s %10s %10s %10s %8s\n" "---" "-----" "-----" "-----" "-----" "-----"

rows=$(mktemp)
for spec in $OPS; do
    op="${spec%%:*}"; n="${spec##*:}"
    ra=$(per_op "$RA" "$op" "$n")
    mi=$(per_op "$MI" "$op" "$n")
    sys=$(per_op "" "$op" "$n")
    awk -v op="$op" -v ra="$ra" -v mi="$mi" -v sys="$sys" \
        'BEGIN{ d=ra-mi; r=(mi>0? ra/mi : 0); printf "%s %s %s %s %s %s\n", op, ra, mi, sys, d, r }' >>"$rows"
    awk -v op="$op" -v ra="$ra" -v mi="$mi" -v sys="$sys" \
        'BEGIN{ d=ra-mi; r=(mi>0? ra/mi : 0);
                printf "%-13s %10.2f %10.2f %10.2f %+10.2f %8.3f\n", op, ra, mi, sys, d, r }'
done

echo
echo "RANKED BY ABSOLUTE Ir/op LOST TO MIMALLOC (optimization order):"
sort -k5 -g -r "$rows" | awk '$5>0 { printf "  %-13s %+8.2f Ir/op  (%.3fx)\n", $1, $5, $6 }'
echo
echo "OPS WHERE WE ALREADY WIN:"
sort -k5 -g "$rows" | awk '$5<=0 { printf "  %-13s %+8.2f Ir/op  (%.3fx)\n", $1, $5, $6 }'
rm -f "$rows"
