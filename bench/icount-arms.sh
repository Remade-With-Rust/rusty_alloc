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
# jemalloc arm. The README quotes a vs-jemalloc column; that number used to come
# from a one-off invocation, which meant nothing in this repo could re-derive
# it. Wired in here so `bench/icount-arms.sh` produces every column the README
# prints. Override with JE_LIB=; skipped cleanly if the library is absent.
JE="${JE_LIB:-/usr/lib/x86_64-linux-gnu/libjemalloc.so.2}"

# Whole-program instructions for one arm. Also records, in ALLOC_IR, how many
# of those instructions were the allocator's OWN — because a whole-program ratio
# understates the allocator by exactly the fraction of the program that is not
# the allocator, and on these workloads that fraction is 94-98%. Attribution is
# by OBJECT from the raw callgrind file: an interpreter reaches the allocator
# through symbols that carry neither allocator's name, so matching on the symbol
# name (as bench/opscan2.sh does, correctly, for a C microbenchmark) misses them.
#
# Prints TWO numbers, "whole allocator". It has to print rather than set a
# variable: every caller uses $( ), and an assignment inside a command
# substitution dies with the subshell.
icount() { # arm cmd... -> "<whole_program_ir> <allocator_ir>"

  local arm="$1"; shift
  local pre=""; case "$arm" in ra) pre="$RA";; mi) pre="$MI";; je) pre="$JE";; esac
  local err cg alloc; err=$(mktemp); cg=$(mktemp); alloc=0
  if [ -n "$pre" ]; then
    LD_PRELOAD="$pre" valgrind --tool=callgrind --callgrind-out-file="$cg" \
      --cache-sim=no --branch-sim=no "$@" >/dev/null 2>"$err"
  else
    valgrind --tool=callgrind --callgrind-out-file="$cg" \
      --cache-sim=no --branch-sim=no "$@" >/dev/null 2>"$err"
  fi
  if [ -n "$pre" ]; then
    alloc=$(awk -v want="$(basename "$pre")" '
      function reg(kind, spec,   id, rest) {
        if (spec ~ /^\([0-9]+\)/) {
          id = spec; sub(/^\(/, "", id); sub(/\).*$/, "", id)
          rest = spec; sub(/^\([0-9]+\)[ ]?/, "", rest)
          if (rest != "") NAMES[kind "/" id] = rest
          return NAMES[kind "/" id]
        }
        return spec
      }
      /^ob=/  { ob = reg("ob", substr($0,4)); next }
      /^cob=/ {      reg("ob", substr($0,5)); next }
      /^calls=/ { skip = 1; next }
      /^[0-9+*-]/ { if (skip) { skip = 0; next }
                    if (index(ob, want) > 0) a += $2; next }
      END { print a + 0 }' "$cg")
  else
    alloc=0
  fi
  printf '%s %s
' "$(grep -oP 'refs:\s+\K[0-9,]+' "$err" | tr -d ',')" "$alloc"
  rm -f "$err" "$cg"
}


bench() { # name -- cmd...
  local name="$1"; shift; shift
  local ra mi je sys ra_al mi_al
  read -r ra ra_al   <<<"$(icount ra  "$@")"
  read -r mi mi_al   <<<"$(icount mi  "$@")"
  read -r sys _      <<<"$(icount sys "$@")"
  je=0; [ -f "$JE" ] && read -r je _ <<<"$(icount je "$@")"
  awk -v n="$name" -v a="$ra" -v m="$mi" -v j="$je" -v s="$sys" \
      -v aa="$ra_al" -v ma="$mi_al" 'BEGIN{
    rj = (j > 0) ? sprintf("%.4f", a/j) : " n/a  "
    printf "%-8s ra %13d | mi %13d | je %13d | sys %13d | ra/mi %.4f | ra/je %s | ra/sys %.4f\n", n, a, m, j, s, a/m, rj, a/s
    if (aa > 0 && ma > 0)
      printf "%-8s   allocator only: ra %10d (%5.2f%% of program) | mi %10d | ra/mi %.4f | program floor if ours were free %.4f\n", "", aa, 100*aa/a, ma, aa/ma, (a-aa)/m
  }'
}




# PIN THE INTERPRETER'S HASH SEED. Perl randomises its hash seed per PROCESS,
# so without this the same workload allocates a different pattern every run:
# measured 2026-08-19, three repeats each, unpinned vs pinned —
#
#   unpinned  ra 778844391 / 778625662 / 778855141   (spread 229,479)
#   pinned    ra 777444364 / 777444364 / 777444364   (spread 0, bit-identical)
#
# ~0.03% of noise, which is exactly the digit the ra/mi ratio is quoted to.
# This project already learned this once — bench/rss.sh pins PERL_HASH_SEED
# after an 11 MiB swing invalidated three RSS conclusions (LEDGER,
# 2026-08-06) — but the lesson was never carried across to THIS harness. A
# lesson that lives in one instrument is not a lesson.
export PERL_HASH_SEED=0
export PERL_PERTURB_KEYS=0

echo "METHOD: callgrind instructions retired, deterministic (no clock, no noise)"
echo "        PERL_HASH_SEED pinned; lua's seed is not pinnable from the"
echo "        environment, so treat lua as indicative and perl/sqlite as verdicts"
bench lua    -- lua5.4 -e "local t={} for i=1,120000 do t[#t+1]=tostring(i)..'x' end local m={} for i=1,60000 do m['k'..i]={i,i*2,tostring(i)} end local s=0 for k,v in pairs(m) do s=s+v[2] end print(#t,s)"
bench perl   -- perl -e 'my %h; for my $i (1..150000) { $h{"key$i"} = [ $i, "v$i" ]; } my $s=0; while (my ($k,$v)=each %h) { $s += $v->[0] } print "$s\n";'
bench sqlite -- sqlite3 :memory: "CREATE TABLE t(a INTEGER, b TEXT); WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<60000) INSERT INTO t SELECT x, hex(randomblob(24)) FROM c; SELECT count(*) FROM t;"
