#!/usr/bin/env bash
# CORRECTNESS sweep over every built mimalloc-bench binary: does each program
# RUN TO COMPLETION on rusty_alloc, with the oracle arm beside it so a failure
# attributes immediately (breaks under ra only = our defect; breaks under both
# = the benchmark/config). This is the run-everything companion to
# run-suite.sh's four-benchmark Tier-A timing gate — exit codes and (for
# deterministic programs) output hashes, NOT timing: it is valid on a loaded
# box and that is deliberate.
#
# Usage: sweep-all.sh [procs]      (default 8)
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
bench="$root/corpus/mimalloc-bench/out/bench"
benchdir="$root/corpus/mimalloc-bench/bench"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
procs="${1:-8}"
tmo="${SWEEP_TIMEOUT:-300}"

[ -f "$RA" ] || { echo "missing ra lib: $RA" >&2; exit 2; }
[ -f "$MI" ] || { echo "missing mi lib: $MI" >&2; exit 2; }

# name|invocation (standard args from mimalloc-bench's own bench.sh; procs
# substituted). stdin redirects handled per-case below.
tests=(
  "cfrac|$bench/cfrac 17545186520507317056371138836327483792789528"
  "espresso|$bench/espresso $benchdir/espresso/largest.espresso"
  "barnes|$bench/barnes"
  "larson|$bench/larson 5 8 1000 5000 100 4141 $procs"
  "larson-sized|$bench/larson-sized 5 8 1000 5000 100 4141 $procs"
  "mstress|$bench/mstress $procs 50 25"
  "rptest|$bench/rptest $procs 0 1 2 500 1000 100 8 16000"
  "alloc-test1|$bench/alloc-test 1"
  "alloc-testN|$bench/alloc-test $procs"
  "sh6bench|$bench/sh6bench $((procs * 2))"
  "sh8bench|$bench/sh8bench $((procs * 2))"
  "xmalloc-test|$bench/xmalloc-test -w $procs -t 5 -s 64"
  "cache-thrash|$bench/cache-thrash $procs 1000 1 2000000 $procs"
  "cache-scratch|$bench/cache-scratch $procs 1000 1 2000000 $procs"
  "malloc-large|$bench/malloc-large"
  "mleak10|$bench/mleak 5"
  "mleak100|$bench/mleak 50"
  "glibc-simple|$bench/glibc-simple"
  "glibc-thread|$bench/glibc-thread $procs"
)

run_arm() { # arm name cmd... -> "exitcode"
  local arm="$1" name="$2"; shift 2
  local pre=""
  case "$arm" in ra) pre="$RA";; mi) pre="$MI";; esac
  local out="/tmp/sweep-$name-$arm.out"
  if [ "$name" = barnes ]; then
    LD_PRELOAD="$pre" timeout "$tmo" "$@" <"$benchdir/barnes/input" >"$out" 2>&1
  else
    LD_PRELOAD="$pre" timeout "$tmo" "$@" >"$out" 2>&1
  fi
  echo $?
}

echo "METHOD: exit-code correctness sweep, ra vs mi arms, timeout ${tmo}s, procs=$procs"
printf "%-14s %6s %6s  %s\n" bench ra mi verdict
fails=0
for spec in "${tests[@]}"; do
  name="${spec%%|*}"; cmd="${spec#*|}"
  # shellcheck disable=SC2086
  ra_rc=$(run_arm ra "$name" $cmd)
  # shellcheck disable=SC2086
  mi_rc=$(run_arm mi "$name" $cmd)
  verdict=ok
  if [ "$ra_rc" != "0" ] && [ "$mi_rc" = "0" ]; then verdict="RA-FAIL"; fails=$((fails + 1)); fi
  if [ "$ra_rc" != "0" ] && [ "$mi_rc" != "0" ]; then verdict="both-fail (not ours alone)"; fi
  if [ "$ra_rc" = "0" ] && [ "$mi_rc" != "0" ]; then verdict="mi-fail (we pass)"; fi
  # cfrac prints the factorization deterministically — compare it outright.
  if [ "$name" = cfrac ] && [ "$verdict" = ok ]; then
    cmp -s /tmp/sweep-cfrac-ra.out /tmp/sweep-cfrac-mi.out || {
      verdict="OUTPUT-DIFF"; fails=$((fails + 1)); }
  fi
  printf "%-14s %6s %6s  %s\n" "$name" "$ra_rc" "$mi_rc" "$verdict"
done
echo
if [ "$fails" -ne 0 ]; then echo "SWEEP FAILED: $fails ra-attributable failure(s)"; exit 1; fi
echo "SWEEP PASSED: every benchmark runs to completion on rusty_alloc"
