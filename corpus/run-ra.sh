#!/usr/bin/env bash
# Tier-A arms over the BUILT mimalloc-bench binaries — no bench.sh patching:
#   ra  = LD_PRELOAD librusty_alloc_override.so (ours)
#   mi  = LD_PRELOAD the oracle libmimalloc.so
#   sys = plain glibc
# Standard invocations mirror bench.sh. Usage:
#   run-ra.sh <ra|mi|sys> <cfrac|espresso|larson|xmalloc|malloc-large|glibc-simple> [repeat]
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
bench="$root/corpus/mimalloc-bench/out/bench"
ra_lib="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
mi_lib="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"

arm="$1"; name="$2"; rep="${3:-1}"
case "$arm" in
  ra)  pre="$ra_lib" ;;
  mi)  pre="$mi_lib" ;;
  sys) pre="" ;;
  *) echo "unknown arm $arm" >&2; exit 2 ;;
esac
[ -z "$pre" ] || [ -f "$pre" ] || { echo "missing lib: $pre" >&2; exit 2; }

case "$name" in
  cfrac)        cmd=("$bench/cfrac" 17545186520507317056371138836327483792789528) ;;
  espresso)     cmd=("$bench/espresso" "$bench/../../bench/espresso/largest.espresso") ;;
  larson)       cmd=("$bench/larson" 5 8 1000 5000 100 4141 8) ;;
  xmalloc)      cmd=("$bench/xmalloc-test" -w 4 -t 5 -s 64) ;;
  malloc-large) cmd=("$bench/malloc-large") ;;
  glibc-simple) cmd=("$bench/glibc-simple") ;;
  *) echo "unknown bench $name" >&2; exit 2 ;;
esac

for _ in $(seq "$rep"); do
  if [ -n "$pre" ]; then
    LD_PRELOAD="$pre" /usr/bin/time -f "$name $arm: wall %e s user %U s sys %S s maxrss %M KiB" "${cmd[@]}" >/dev/null
  else
    /usr/bin/time -f "$name $arm: wall %e s user %U s sys %S s maxrss %M KiB" "${cmd[@]}" >/dev/null
  fi
done
