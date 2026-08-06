#!/usr/bin/env bash
# Deterministic allocator comparison on REAL programs: instructions retired
# (callgrind) for the same workload under ra / mi / glibc.
#
# This exists because the clock on this box cannot resolve a 5-10% effect (the
# null arm reads +/-3% with median and min disagreeing in SIGN for identical
# binaries). Instruction count is a COUNTER: deterministic, unaffected by an
# open IDE, a browser, or thermal drift. Same program, same input, same output
# in every arm, so the only variable is the allocator - which makes fewer
# instructions a meaningful "less work done".
#
# Caveat kept explicit: fewer instructions is not automatically less TIME
# (cache behaviour and syscalls do not show up here). It is evidence about
# work, which is exactly what the discipline says to gather when the clock
# cannot resolve.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"

icount() { # arm cmd...
  local arm="$1"; shift
  local pre=""; case "$arm" in ra) pre="$RA";; mi) pre="$MI";; esac
  local err; err=$(mktemp)
  if [ -n "$pre" ]; then
    LD_PRELOAD="$pre" valgrind --tool=callgrind --callgrind-out-file=/dev/null \
      --cache-sim=no --branch-sim=no "$@" >/dev/null 2>"$err"
  else
    valgrind --tool=callgrind --callgrind-out-file=/dev/null \
      --cache-sim=no --branch-sim=no "$@" >/dev/null 2>"$err"
  fi
  grep -oP 'refs:\s+\K[0-9,]+' "$err" | tr -d ','
  rm -f "$err"
}

bench() { # name -- cmd...
  local name="$1"; shift; shift
  local ra mi sys
  ra=$(icount ra "$@"); mi=$(icount mi "$@"); sys=$(icount sys "$@")
  awk -v n="$name" -v a="$ra" -v m="$mi" -v s="$sys" 'BEGIN{
    printf "%-8s ra %14d | mi %14d | sys %14d | ra/mi %.4f | ra/sys %.4f\n", n, a, m, s, a/m, a/s
  }'
}

echo "METHOD: callgrind instructions retired, deterministic (no clock, no noise)"
bench lua    -- lua5.4 -e "local t={} for i=1,120000 do t[#t+1]=tostring(i)..'x' end local m={} for i=1,60000 do m['k'..i]={i,i*2,tostring(i)} end local s=0 for k,v in pairs(m) do s=s+v[2] end print(#t,s)"
bench perl   -- perl -e 'my %h; for my $i (1..150000) { $h{"key$i"} = [ $i, "v$i" ]; } my $s=0; while (my ($k,$v)=each %h) { $s += $v->[0] } print "$s\n";'
bench sqlite -- sqlite3 :memory: "CREATE TABLE t(a INTEGER, b TEXT); WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<60000) INSERT INTO t SELECT x, hex(randomblob(24)) FROM c; SELECT count(*) FROM t;"
