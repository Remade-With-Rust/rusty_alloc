#!/usr/bin/env bash
# Does peak RSS scale with THREAD COUNT? (per-thread heap retention)
#
# The single-threaded bench/rss.sh showed rusty_alloc at or below mimalloc.
# FFAI, at 28 threads, measured +17.9%. If retention is per-heap, the gap must
# GROW with thread count — and a one-thread benchmark is structurally blind to
# it. This sweeps N and prints the per-thread cost of each allocator.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
BIN="$HOME/ra_rss_threads"
ROUNDS="${ROUNDS:-40}"
REPS="${REPS:-3}"

gcc -O2 -o "$BIN" "$root/bench/rss-threads.c" -lpthread || exit 1

peak() { # preload, threads, extra env...
    local pre="$1" n="$2"; shift 2
    local best=99999999 i v
    for ((i=0; i<REPS; i++)); do
        v=$(env "$@" ${pre:+LD_PRELOAD="$pre"} /usr/bin/time -f "%M" \
            "$BIN" "$n" "$ROUNDS" 2>&1 >/dev/null | tail -1)
        [ "${v:-0}" -lt "$best" ] 2>/dev/null && best=$v
    done
    echo "$best"
}

printf "%-8s %12s %12s %12s %12s\n" threads mimalloc rusty_alloc "ra+purge" "ra-mi"
printf "%-8s %12s %12s %12s %12s\n" ------- -------- ----------- -------- -----
for n in 1 4 8 16 28; do
    m=$(peak "$MI" "$n")
    r=$(peak "$RA" "$n")
    p=$(peak "$RA" "$n" MIMALLOC_PURGE_DELAY=10)
    awk -v n="$n" -v m="$m" -v r="$r" -v p="$p" 'BEGIN{
        printf "%-8d %9.1f MiB %9.1f MiB %9.1f MiB %+9.1f MiB\n",
               n, m/1024, r/1024, p/1024, (r-m)/1024 }'
done
echo
echo "A gap that GROWS with thread count is per-thread retention."
echo "If ra+purge flattens it, purging the per-thread cache is the fix."
