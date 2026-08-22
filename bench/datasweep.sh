#!/usr/bin/env bash
# Run bench/datasweep.c under every allocator available on this box.
#
# This is a CORRECTNESS sweep wearing a benchmark's clothes. The instruction
# harnesses beside it answer "how much work"; none of them can see a block that
# overlaps its neighbour, a calloc that hands back a recycled block still
# holding old bytes, a realloc that loses the tail, or an alignment honoured for
# 64 but not for 4096.
#
# Running every arm matters: the driver is allocator-agnostic, so glibc,
# mimalloc and jemalloc are the control. A failure in all four arms is a bug in
# the driver (it has happened); a failure in one arm is a bug in that allocator.
#
#   bench/datasweep.sh [scale]
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
scale="${1:-1}"

RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
JE="${JE_LIB:-/usr/lib/x86_64-linux-gnu/libjemalloc.so.2}"

bin=$(mktemp) || exit 1
trap 'rm -f "$bin"' EXIT
gcc -O2 -pthread -Wall -Wextra -o "$bin" "$root/bench/datasweep.c" || exit 1

rc_all=0
run_arm() {
    local name="$1" pre="$2" out rc
    if [ -n "$pre" ] && [ ! -f "$pre" ]; then
        printf "  %-8s SKIPPED (no %s)\n" "$name" "$pre"
        return
    fi
    if [ -z "$pre" ]; then
        out=$("$bin" "$scale" 2>&1); rc=$?
    else
        out=$(LD_PRELOAD="$pre" "$bin" "$scale" 2>&1); rc=$?
    fi
    printf "  %-8s %s\n" "$name" "$(echo "$out" | grep -E 'datasweep:|PASSED|FAILED' | tr '\n' ' ')"
    if [ "$rc" -ne 0 ]; then
        echo "$out" | grep '^FAIL' | head -5 | sed 's/^/           /'
        rc_all=1
    fi
}

echo "datasweep (scale=$scale) — same driver, every allocator; all must pass"
run_arm glibc    ""
run_arm rusty    "$RA"
run_arm mimalloc "$MI"
run_arm jemalloc "$JE"

# Our own hardened and invariant-checking builds, when asked for. These are the
# arms that matter most for a CORRECTNESS sweep and least for a speed one:
#   secure        — encoded free-list links; the decode is on the path every
#                   block travels, so a wrong key or a wrong bound shows up as
#                   exactly the content mismatch this driver looks for.
#   debug_checks  — full invariant checking. It aborts INSIDE the allocator on
#                   a broken list or a bad segment layout, so it can name a
#                   fault the data check would only see later, or not at all.
# Off by default because each needs its own build of the cdylib.
if [ "${DATASWEEP_RA_VARIANTS:-0}" != "0" ]; then
    for feat in secure debug_checks; do
        tgt="$HOME/ra_target_$feat"
        if CARGO_TARGET_DIR="$tgt" cargo build --release -p rusty_alloc-override \
               --features "rusty_alloc/$feat" >/dev/null 2>&1
        then
            run_arm "ra/$feat" "$tgt/release/librusty_alloc_override.so"
        else
            printf "  %-8s BUILD FAILED (features rusty_alloc/%s)\n" "ra/$feat" "$feat"
            rc_all=1
        fi
    done
fi

if [ "$rc_all" -ne 0 ]; then echo "DATASWEEP: at least one arm FAILED"; exit 1; fi
echo "DATASWEEP: all arms passed"
