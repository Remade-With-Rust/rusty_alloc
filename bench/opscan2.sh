#!/usr/bin/env bash
# Per-operation allocator scan, MARGINAL and ALLOCATOR-ONLY.
#
# WHY THIS EXISTS. Two earlier estimators disagreed by 2.7x on the same op:
#   * whole-process two-point  (Ir(2N)-Ir(N))/N  - cancels startup, but counts
#     the caller's loop and PLT thunks;
#   * per-object total         alloc(N)/N        - allocator only, but averages
#     in one-time init (arena reservation, first page extends) over N.
# Neither is wrong; they answer different questions. This one answers the
# question we actually have - "what does ONE more operation cost inside the
# allocator" - by differencing the PER-OBJECT cost:
#
#       Ir/op = (alloc_Ir(2N) - alloc_Ir(N)) / N
#
# Startup cancels (differencing) AND caller-side loop/PLT is excluded
# (per-object attribution). Deterministic: no clock, no pinning, no repeats.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
BIN="$HOME/ra_opscan"
gcc -O2 -o "$BIN" "$root/bench/opscan.c" || exit 1

OPS="${OPS:-small:100000 med:100000 big:50000 large:20000 calloc:100000 batch_lifo:100000 realloc:50000 aligned:100000 mixed:100000}"

# Ir attributed to one allocator for a single run.
#
# MATCH ON THE NAME, NOT THE [object] SUFFIX. callgrind_annotate ELIDES the
# object on continuation lines, so grepping the .so path silently dropped every
# line after the first for whichever allocator had many symbols. That made our
# side look 2-4x cheaper than it is - caught because the result disagreed in
# SIGN with the whole-process estimator. Our lines all carry "rusty_alloc" in
# the source path or the demangled symbol; mimalloc's carry "mimalloc" or the
# "mi_" prefix.
alloc_ir() {
    local pre="$1" pat="$2" op="$3" iters="$4" out ir
    out=$(mktemp)
    LD_PRELOAD="$pre" valgrind --tool=callgrind --callgrind-out-file="$out" \
        --cache-sim=no --branch-sim=no "$BIN" "$op" "$iters" >/dev/null 2>&1
    ir=$(callgrind_annotate --threshold=100 "$out" 2>/dev/null \
        | grep -E "$pat" | awk '{gsub(/,/,"",$1); s+=$1} END{print s+0}')
    rm -f "$out"
    echo "${ir:-0}"
}

per_op() {
    local pre="$1" obj="$2" op="$3" n="$4" a b
    a=$(alloc_ir "$pre" "$obj" "$op" "$n")
    b=$(alloc_ir "$pre" "$obj" "$op" "$((n * 2))")
    awk -v a="$a" -v b="$b" -v n="$n" 'BEGIN{ printf "%.2f", (b-a)/n }'
}

echo "METHOD: callgrind Ir, MARGINAL per-object: (alloc_Ir(2N)-alloc_Ir(N))/N"
echo "        startup cancels; caller loop/PLT excluded; allocator work only."
echo
printf "%-12s %9s %9s %9s %8s\n" op ra mi "ra-mi" "ra/mi"
printf "%-12s %9s %9s %9s %8s\n" "---" "----" "----" "----" "----"
rows=$(mktemp)
for spec in $OPS; do
    op="${spec%%:*}"; n="${spec##*:}"
    ra=$(per_op "$RA" "rusty_alloc" "$op" "$n")
    mi=$(per_op "$MI" "mimalloc|:mi_|:(malloc|free|calloc|realloc|posix_memalign|malloc_usable_size)\b" "$op" "$n")
    awk -v op="$op" -v ra="$ra" -v mi="$mi" 'BEGIN{
        d=ra-mi; r=(mi>0? ra/mi : 0);
        printf "%-12s %9.2f %9.2f %+9.2f %8.3f\n", op, ra, mi, d, r }' | tee -a "$rows"
done
echo
echo "OPTIMIZATION ORDER (worst first):"
sort -k4 -g -r "$rows" | awk '$4+0>0 { printf "  %-12s %+8.2f Ir/op  (%sx)\n", $1, $4, $5 }'
echo "ALREADY AHEAD:"
sort -k4 -g "$rows" | awk '$4+0<=0 { printf "  %-12s %+8.2f Ir/op  (%sx)\n", $1, $4, $5 }'
rm -f "$rows"
