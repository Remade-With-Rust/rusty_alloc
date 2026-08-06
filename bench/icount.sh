#!/usr/bin/env bash
# Deterministic A/B by INSTRUCTION COUNT (callgrind), not by clock.
#
# Why this exists: this box's null arm reads +/-3% with the two statistics
# disagreeing in SIGN for IDENTICAL binaries, so the clock cannot adjudicate a
# 5-10% change here. Instructions retired is a COUNTER - deterministic, immune
# to scheduling, IDE noise and thermal drift - which is exactly the instrument
# the discipline prefers when the clock cannot resolve (codec-measurement: the
# counter is primary, the clock is confirmatory).
#
# Reports Ir (instructions) per arm and the ratio. Work parity must be checked
# separately (identical allocator counters) or the comparison is void.
#
# Usage: icount.sh <binA> <binB> [args...]
set -uo pipefail
A="$1"; B="$2"; shift 2
run() {
  local bin="$1"; shift
  local out; out=$(mktemp)
  valgrind --tool=callgrind --callgrind-out-file=/dev/null \
           --cache-sim=no --branch-sim=no "$bin" "$@" >/dev/null 2>"$out"
  grep -oP 'refs:\s+\K[0-9,]+' "$out" | tr -d ','
  rm -f "$out"
}
ia=$(run "$A" "$@")
ib=$(run "$B" "$@")
echo "A ($A): $ia instructions"
echo "B ($B): $ib instructions"
awk -v a="$ia" -v b="$ib" 'BEGIN{
  printf "ratio B/A: %.5f  (%+.2f%% instructions)\n", b/a, 100*(b/a-1)
}'
echo "METHOD: callgrind instruction count, deterministic; verify work parity via allocator counters"
