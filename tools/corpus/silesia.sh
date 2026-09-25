#!/usr/bin/env bash
# Silesia under rusty_zstd's CLI, published allocator (BASE) vs this tree (CAND),
# with C zstd 1.5.7 as the independent oracle. Per file and level:
#   1. CAND compresses byte-identically to BASE   (an allocator must not change output)
#   2. CAND decodes its own frame to the original
#   3. C zstd decodes CAND's frame to the original
#   4. CAND decodes C zstd's frame to the original
# plus the same with 4 worker threads (-T4), which exercises cross-thread frees.
# usage: silesia.sh <base rzstd.exe> <cand rzstd.exe> <oracle zstd.exe> <silesia dir> <scratch dir>
set -u
B="$1"; C="$2"; O="$3"; S="$4"; W="$5"; mkdir -p "$W"
pass=0; fail=0
bad() { echo "FAIL $*"; fail=$((fail + 1)); }
for f in "$S"/*; do
  n=$(basename "$f")
  for lv in 1 3 9 19; do
    "$B" -q -$lv -c "$f" > "$W/b.zst" || { bad "$n -$lv base compress"; continue; }
    "$C" -q -$lv -c "$f" > "$W/c.zst" || { bad "$n -$lv cand compress"; continue; }
    cmp -s "$W/b.zst" "$W/c.zst" || bad "$n -$lv compressed bytes differ from base"
    "$C" -q -d -c "$W/c.zst" > "$W/c.out" && cmp -s "$f" "$W/c.out" || bad "$n -$lv cand->cand roundtrip"
    "$O" -q -d -c "$W/c.zst" > "$W/o.out" && cmp -s "$f" "$W/o.out" || bad "$n -$lv cand->C decode"
    "$O" -q -$lv -c "$f" > "$W/o.zst" && "$C" -q -d -c "$W/o.zst" > "$W/co.out" && cmp -s "$f" "$W/co.out" || bad "$n -$lv C->cand decode"
    pass=$((pass + 1))
  done
  "$B" -q -3 -T4 -c "$f" > "$W/bt.zst" && "$C" -q -3 -T4 -c "$f" > "$W/ct.zst" || { bad "$n -T4 compress"; continue; }
  cmp -s "$W/bt.zst" "$W/ct.zst" || bad "$n -T4 compressed bytes differ from base"
  "$O" -q -d -c "$W/ct.zst" > "$W/ot.out" && cmp -s "$f" "$W/ot.out" || bad "$n -T4 cand->C decode"
  "$C" -q -d -c "$W/ct.zst" > "$W/ct.out" && cmp -s "$f" "$W/ct.out" || bad "$n -T4 cand->cand roundtrip"
  pass=$((pass + 1))
  echo "done $n"
done
rm -f "$W"/*.zst "$W"/*.out
echo "silesia: $pass file-level cases, $fail failures"
