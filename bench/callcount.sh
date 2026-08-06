#!/usr/bin/env bash
# Deterministic CALL COUNTS per function, for one opscan op, per allocator.
#
# The plan (docs/plans/opscan_v1.md) requires a COUNT before any code change:
# how often does each side leave the fast path? Callgrind records exact call
# counts, so this needs no counter build and works identically on both arms.
#
# usage: callcount.sh <op> <iters> [name-regex]
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
BIN="$HOME/ra_opscan"
gcc -O2 -o "$BIN" "$root/bench/opscan.c" || exit 1

op="${1:-batch_lifo}"
n="${2:-100000}"
pat="${3:-generic|extend|collect|malloc|free|page|segment|retire|queue}"

# Callgrind interns function names: `cfn=(id) name` on first sight, `cfn=(id)`
# after. Build the id->name map, then attribute each `calls=` to the most
# recent cfn id.
extract() {
    awk -v pat="$pat" '
        /^cfn=\(/ {
            id=$0; sub(/^cfn=\(/,"",id); name=id
            sub(/\).*$/,"",id)
            if (match(name, /\) /)) { nm=substr(name, RSTART+2); names[id]=nm }
            cur=id; next
        }
        /^fn=\(/ {
            id=$0; sub(/^fn=\(/,"",id); name=id
            sub(/\).*$/,"",id)
            if (match(name, /\) /)) { nm=substr(name, RSTART+2); names[id]=nm }
            next
        }
        /^calls=/ {
            c=$0; sub(/^calls=/,"",c); split(c, a, " ")
            if (cur != "") total[cur] += a[1]
            next
        }
        END {
            for (id in total) {
                nm = (id in names) ? names[id] : ("id" id)
                if (nm ~ pat) printf "%12d  %s\n", total[id], nm
            }
        }
    ' "$1" | sort -rn | head -18
}

for arm in ra mi; do
    pre=$RA; [ "$arm" = mi ] && pre=$MI
    out=$(mktemp)
    LD_PRELOAD="$pre" valgrind --tool=callgrind --callgrind-out-file="$out" \
        --cache-sim=no --branch-sim=no "$BIN" "$op" "$n" >/dev/null 2>&1
    echo "== $arm : call counts for op '$op' (N=$n)"
    extract "$out"
    echo
    rm -f "$out"
done
