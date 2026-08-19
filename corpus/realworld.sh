#!/usr/bin/env bash
# Real-world validation sweep: run REAL open-source programs on rusty_alloc via
# LD_PRELOAD (the same interposition path mimalloc uses), against the oracle
# mimalloc and glibc.
#
# Two goals, per the brief:
#   1. performance deltas on real workloads
#   2. real-world BUGS — a crash/hang/wrong-output under `ra` that does not
#      happen under `mi`/`sys` is a defect in us, and the arms make that
#      attribution immediate.
#
# Correctness is checked where the program has a natural oracle (checksums of
# output, exit status), not just timing.
#
# Usage: realworld.sh [workload ...]   (default: all)
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
work="${WORK:-/tmp/rw}"; mkdir -p "$work"; cd "$work"

[ -f "$RA" ] || { echo "missing ra lib: $RA" >&2; exit 2; }
[ -f "$MI" ] || { echo "missing mi lib: $MI" >&2; exit 2; }

# --- fixtures (built once) --------------------------------------------------
setup() {
  [ -f big.json ] || python3 - <<'EOF'
import json, random
random.seed(7)
rows=[{"id":i,"name":f"row-{i}","tags":[f"t{j}" for j in range(i%12)],
       "vals":[random.random() for _ in range(8)]} for i in range(60000)]
open("/tmp/rw/big.json","w").write(json.dumps(rows))
EOF
  [ -f big.txt ] || python3 -c "
import random; random.seed(3)
w=[''.join(random.choice('abcdefghijklmnop') for _ in range(random.randint(3,12))) for _ in range(4000)]
open('/tmp/rw/big.txt','w').write(' '.join(random.choice(w) for _ in range(3000000)))"
  [ -d gitrepo ] || { git -c init.defaultBranch=main init -q gitrepo; (cd gitrepo
      for i in $(seq 1 300); do echo "line $i $RANDOM" >> f$((i%20)).txt; done
      git add -A >/dev/null; git -c user.email=a@b -c user.name=c commit -qm init >/dev/null); }
  [ -f img.png ] || convert -size 1400x1400 plasma:fractal img.png 2>/dev/null
}

run_arm() { # arm cmd...
  local arm="$1"; shift
  local pre=""
  case "$arm" in ra) pre="$RA";; mi) pre="$MI";; sys) pre="";; esac
  local t0 t1
  if [ -n "$pre" ]; then
    /usr/bin/time -f "%e %M" -o /tmp/rw/.time env LD_PRELOAD="$pre" "$@" >/tmp/rw/.out 2>/tmp/rw/.err
  else
    /usr/bin/time -f "%e %M" -o /tmp/rw/.time "$@" >/tmp/rw/.out 2>/tmp/rw/.err
  fi
  local rc=$?
  local tm; tm=$(cat /tmp/rw/.time 2>/dev/null | tail -1)
  local sum; sum=$(md5sum < /tmp/rw/.out | cut -c1-12)
  echo "$rc $tm $sum"
}

report() { # name arm result
  local name="$1" arm="$2"; shift 2
  local rc tm rss sum; read -r rc wall rss sum <<<"$*"
  if [ "$rc" != "0" ]; then
    echo "  $name $arm: **EXIT $rc** (wall ${wall}s rss ${rss}KiB) $(head -c 120 /tmp/rw/.err | tr '\n' ' ')"
  else
    echo "  $name $arm: wall ${wall}s rss ${rss}KiB out:$sum"
  fi
}

bench() { # name -- cmd...
  local name="$1"; shift; shift
  echo "== $name"
  for arm in ra mi sys; do report "$name" "$arm" "$(run_arm "$arm" "$@")"; done
  for arm in sys mi ra; do report "$name" "$arm" "$(run_arm "$arm" "$@")"; done
}

setup
sel="${*:-jq sqlite python git xz zstd lua imagemagick perl redis}"

for w in $sel; do
case "$w" in
  jq)     bench jq -- jq -c '[.[] | select(.id % 7 == 0) | {id, n: (.vals | add)}] | length' big.json ;;
  sqlite) bench sqlite -- sqlite3 :memory: "CREATE TABLE t(a INTEGER, b TEXT); WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<400000) INSERT INTO t SELECT x, hex(randomblob(24)) FROM c; SELECT count(*), sum(a) FROM t; CREATE INDEX i ON t(b); SELECT count(DISTINCT b) FROM t;" ;;
  python) bench python -- python3 -c "
import json,collections
d=json.load(open('/tmp/rw/big.json'))
c=collections.Counter()
for r in d:
    for t in r['tags']: c[t]+=1
s=sorted((sum(r['vals']),r['id']) for r in d)
print(len(d), c.most_common(3), round(s[-1][0],6))" ;;
  git)    bench git -- git -C gitrepo log --stat --all ;;
  xz)     bench xz -- xz -3 -T0 -c big.txt ;;
  zstd)   bench zstd -- zstd -12 -T0 -c big.txt ;;
  lua)    bench lua -- lua5.4 -e "
local t={} for i=1,700000 do t[#t+1]=tostring(i)..'x' end
local n=0 for i=1,#t do n=n+#t[i] end
local m={} for i=1,300000 do m['k'..i]={i,i*2,tostring(i)} end
local s=0 for k,v in pairs(m) do s=s+v[2] end
print(n,s)" ;;
  imagemagick) bench imagemagick -- convert img.png -resize 700x700 -blur 0x3 -sharpen 0x2 -depth 8 png:- ;;
  perl)   bench perl -- perl -e '
my %h; for my $i (1..800000) { $h{"key$i"} = [ $i, "v$i" ]; }
my $s=0; while (my ($k,$v)=each %h) { $s += $v->[0] } print "$s\n";' ;;
  redis)  echo "== redis"
          # A redis built ON jemalloc (mem_allocator:jemalloc-5.3.0, links
          # libjemalloc.so.2) reaches allocator symbols directly, so
          # LD_PRELOADing ANY replacement — ours or the mimalloc oracle —
          # produces a MIXED-ALLOCATOR process: blocks allocated by one
          # allocator, freed by the other. Failures on the preloaded arms of
          # such a binary are a property of the configuration, not of either
          # allocator (measured 2026-08-06: mimalloc 8/8 crashes, ours 6/8;
          # 2026-08-19: both arms fail, sys 3/3 clean). Only the SYS arm is a
          # verdict here; rebuild redis with MALLOC=libc for a real A/B.
          if redis-server --version 2>/dev/null | grep -q jemalloc; then
            echo "  NOTE: this redis is jemalloc-linked — preloaded-arm failures are the known mixed-allocator config, not an allocator defect"
          fi
          for arm in ra mi sys; do
            pre=""; case "$arm" in ra) pre="$RA";; mi) pre="$MI";; esac
            rm -f /tmp/rw/redis.log
            if [ -n "$pre" ]; then LD_PRELOAD="$pre" redis-server --port 7777 --save '' --daemonize no --logfile /tmp/rw/redis.log & else redis-server --port 7777 --save '' --daemonize no --logfile /tmp/rw/redis.log & fi
            srv=$!; sleep 1
            out=$(redis-benchmark -p 7777 -n 120000 -c 40 -P 8 -t set,get,lpush,lrange_300 -q 2>&1)
            rc=$?
            mem=$(redis-cli -p 7777 info memory 2>/dev/null | grep used_memory_rss: | tr -d '\r')
            redis-cli -p 7777 shutdown nosave 2>/dev/null; wait $srv 2>/dev/null
            if [ $rc != 0 ] || ! echo "$out" | grep -q SET; then
              echo "  redis $arm: **FAILED** $(head -c 160 /tmp/rw/redis.log | tr '\n' ' ')"
            else
              echo "  redis $arm: $(echo "$out" | tr '\n' ' ' | sed 's/  */ /g') $mem"
            fi
          done ;;
esac
done
