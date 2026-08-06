#!/usr/bin/env bash
# Repeated, ARM-INTERLEAVED real-world measurement: 5 pairs per workload,
# reporting the MEDIAN and the min (best-of-N floor) per arm. Single runs on
# this box swing 2-3x, so only medians over interleaved reps are quotable —
# and even these are dev-loop numbers (WSL2), never standing claims.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
work=/tmp/rw; cd "$work"
REPS="${REPS:-5}"

median() { printf '%s\n' "$@" | sort -n | awk '{a[NR]=$1} END{print (NR%2)?a[(NR+1)/2]:(a[NR/2]+a[NR/2+1])/2}'; }
minv()   { printf '%s\n' "$@" | sort -n | head -1; }

one() { # arm cmd...  -> prints elapsed seconds, or "FAIL:<rc>"
  local arm="$1"; shift
  local pre=""; case "$arm" in ra) pre="$RA";; mi) pre="$MI";; esac
  local t0 t1 rc
  t0=$(date +%s.%N)
  if [ -n "$pre" ]; then
    LD_PRELOAD="$pre" "$@" >/dev/null 2>&1; rc=$?
  else
    "$@" >/dev/null 2>&1; rc=$?
  fi
  t1=$(date +%s.%N)
  if [ $rc -ne 0 ]; then echo "FAIL:$rc"; else echo "$t1 $t0" | awk '{printf "%.3f", $1-$2}'; fi
}

sweep() { # name -- cmd...
  local name="$1"; shift; shift
  local -a ra=() mi=() sys=()
  for ((i=0;i<REPS;i++)); do
    if (( i % 2 == 0 )); then
      ra+=("$(one ra "$@")"); mi+=("$(one mi "$@")"); sys+=("$(one sys "$@")")
    else
      sys+=("$(one sys "$@")"); mi+=("$(one mi "$@")"); ra+=("$(one ra "$@")")
    fi
  done
  local rm mm sm rmin mmin
  rm=$(median "${ra[@]}"); mm=$(median "${mi[@]}"); sm=$(median "${sys[@]}")
  rmin=$(minv "${ra[@]}"); mmin=$(minv "${mi[@]}")
  echo "$name | ra med ${rm}s (min ${rmin}) | mi med ${mm}s (min ${mmin}) | sys med ${sm}s | ra/mi $(echo "scale=3; $rm/$mm" | bc)"
}

echo "METHOD: WSL2, LD_PRELOAD arms, ${REPS} ABBA-interleaved reps, median + min of wall time; dev-loop numbers"
sweep jq     -- jq -c '[.[] | select(.id % 7 == 0) | {id, n: (.vals | add)}] | length' big.json
sweep lua    -- lua5.4 -e "local t={} for i=1,700000 do t[#t+1]=tostring(i)..'x' end local n=0 for i=1,#t do n=n+#t[i] end local m={} for i=1,300000 do m['k'..i]={i,i*2,tostring(i)} end local s=0 for k,v in pairs(m) do s=s+v[2] end print(n,s)"
sweep perl   -- perl -e 'my %h; for my $i (1..800000) { $h{"key$i"} = [ $i, "v$i" ]; } my $s=0; while (my ($k,$v)=each %h) { $s += $v->[0] } print "$s\n";'
sweep sqlite -- sqlite3 :memory: "CREATE TABLE t(a INTEGER, b TEXT); WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<400000) INSERT INTO t SELECT x, hex(randomblob(24)) FROM c; SELECT count(*), sum(a) FROM t; CREATE INDEX i ON t(b); SELECT count(DISTINCT b) FROM t;"
sweep zstd   -- zstd -12 -T0 -c big.txt
sweep python -- python3 -c "
import json,collections
d=json.load(open('/tmp/rw/big.json'))
c=collections.Counter()
for r in d:
    for t in r['tags']: c[t]+=1
s=sorted((sum(r['vals']),r['id']) for r in d)
print(len(d), c.most_common(3), round(s[-1][0],6))"
