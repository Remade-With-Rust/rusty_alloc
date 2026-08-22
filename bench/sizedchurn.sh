#!/usr/bin/env bash
# Marginal ALLOCATOR-ONLY Ir per operation for bench/sizedchurn.cpp.
#
#   sizedchurn.sh <preload.so> [n]
#
# WHY THE DRIVER EXISTS. `larson-sized` cannot referee a change to this
# allocator. Its workers respawn themselves until a timer fires (`exercise_heap`
# ends with `if (!stopflag) _beginthread(exercise_heap, ...)`) and the
# `runloops` phase before them breaks on elapsed time, so the amount of work the
# benchmark does is an OUTPUT of how fast the allocator is. Measured: in one
# second under callgrind rusty_alloc completed 2,525,001 operations against
# mimalloc's 1,745,458 - 45% more - so the aggregate instruction count reports a
# 16% win as a 21% loss. It is nonetheless reproducible to +-0.006% for a FIXED
# binary, which makes it more dangerous than an obviously noisy benchmark, not
# less. sizedchurn.cpp does the same allocator work with a FIXED iteration
# count, so both arms do the same thing and two-point differencing applies.
#
# WHY THIS SCRIPT PARSES THE RAW FILE instead of copying `opscan2.sh`. That
# script attributes cost by matching the SYMBOL NAME ("rusty_alloc" / "mi_"),
# which works for a C benchmark because every hot symbol carries one of those.
# It does NOT work here: the hot symbols of a C++ workload are
# `operator new[](unsigned long)` and `operator delete[](void*, unsigned long)`,
# which carry NEITHER allocator's name. Measured with name-matching this driver
# reads 78.563 Ir/op for rusty_alloc against 63.628 for mimalloc - the opposite
# of the truth, because the pattern picks up source paths outside the allocator
# on our side and misses the C++ exports on both.
#
# So attribute by OBJECT, from the raw callgrind file. That also avoids
# callgrind_annotate's elided "[object]" suffix, which under-counted this
# allocator ~4x in an earlier campaign (docs/plans/finished/opscan_v1.md,
# estimator 2 - DISQUALIFIED). The parser below re-derives compressed names,
# INCLUDING ids first defined on call lines (cob=/cfn=/cfi=), and checks its own
# arithmetic: the sum of self costs must equal callgrind's `summary:`.
set -uo pipefail

d="$(cd "$(dirname "$0")" && pwd)"
pre="${1:?usage: sizedchurn.sh <preload.so> [n]}"
n="${2:-200000}"

bin=$(mktemp) || exit 1
trap 'rm -f "$bin"' EXIT
g++ -O2 -std=c++17 -fno-builtin -o "$bin" "$d/sizedchurn.cpp" || exit 1

alloc_ir() {
    local iters="$1" out
    out=$(mktemp)
    LD_PRELOAD="$pre" valgrind --tool=callgrind --callgrind-out-file="$out" \
        --cache-sim=no --branch-sim=no "$bin" "$iters" >/dev/null 2>&1
    awk -v want="$(basename "$pre")" '
      function reg(kind, spec,   id, rest) {
        if (spec ~ /^\([0-9]+\)/) {
          id = spec; sub(/^\(/, "", id); sub(/\).*$/, "", id)
          rest = spec; sub(/^\([0-9]+\)[ ]?/, "", rest)
          if (rest != "") NAMES[kind "/" id] = rest
          return NAMES[kind "/" id]
        }
        return spec
      }
      /^ob=/    { ob = reg("ob", substr($0,4)); next }
      /^cob=/   {      reg("ob", substr($0,5)); next }
      /^(fl|fi|fe)=/ { reg("fl", substr($0,4)); next }
      /^(cfi|cfl)=/  { reg("fl", substr($0,6)); next }
      /^fn=/    {      reg("fn", substr($0,4)); next }
      /^cfn=/   {      reg("fn", substr($0,5)); next }
      /^calls=/ { skip = 1; next }
      /^summary:/ { sum = $2; next }
      /^totals:/  { if (sum == "") sum = $2; next }
      /^[0-9+*-]/ {
        if (skip) { skip = 0; next }        # inclusive cost of the call above
        total += $2
        if (index(ob, want) > 0) alloc += $2
        next
      }
      END {
        if (sum != "" && total != sum) {
          printf "MISMATCH self=%d summary=%d\n", total, sum > "/dev/stderr"
          exit 1
        }
        print alloc + 0
      }' "$out"
    local rc=$?
    rm -f "$out"
    return $rc
}

a=$(alloc_ir "$n") || { echo "sizedchurn: parse failed at n" >&2; exit 1; }
b=$(alloc_ir "$((n * 2))") || { echo "sizedchurn: parse failed at 2n" >&2; exit 1; }
awk -v a="$a" -v b="$b" -v n="$n" \
    'BEGIN{ printf "sizedchurn  %.3f Ir/op   (n=%d: %d, 2n: %d)\n", (b-a)/n, n, a, b }'
