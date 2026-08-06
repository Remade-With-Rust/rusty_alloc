#!/usr/bin/env bash
# At a FIXED thread count, sweep allocation SIZE.
#
# rss-threads.sh confirmed per-thread retention is the mechanism (RSS scales
# linearly with threads) but found rusty_alloc retaining HALF what mimalloc
# does — the opposite of FFAI's +17.9%. The remaining variable is size: that
# sweep capped at 32 KB (binned path), while Diana allocates tensors in the
# hundreds-of-KB..MB range, which take the span/huge path — arena-backed and
# aggressively cached, the thing M6/M7 tuned FOR SPEED.
#
# If the sign flips as size grows, large-allocation retention is the gap.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
BIN="$HOME/ra_rss_threads"
THREADS="${THREADS:-8}"
ROUNDS="${ROUNDS:-12}"
REPS="${REPS:-3}"

gcc -O2 -o "$BIN" "$root/bench/rss-threads.c" -lpthread || exit 1

peak() { # preload, maxsize
    local pre="$1" sz="$2" best=99999999 i v
    for ((i=0; i<REPS; i++)); do
        v=$(env ${pre:+LD_PRELOAD="$pre"} /usr/bin/time -f "%M" \
            "$BIN" "$THREADS" "$ROUNDS" "$sz" 2>&1 >/dev/null | tail -1)
        [ "${v:-0}" -lt "$best" ] 2>/dev/null && best=$v
    done
    echo "$best"
}

echo "peak RSS at $THREADS threads, sweeping max allocation size"
echo
printf "%-12s %12s %12s %12s\n" max-size mimalloc rusty_alloc "ra-mi"
printf "%-12s %12s %12s %12s\n" -------- -------- ----------- -----
for sz in 32768 262144 1048576 4194304; do
    m=$(peak "$MI" "$sz")
    r=$(peak "$RA" "$sz")
    awk -v s="$sz" -v m="$m" -v r="$r" 'BEGIN{
        printf "%-12s %9.1f MiB %9.1f MiB %+9.1f MiB (%+.1f%%)\n",
               (s>=1048576? sprintf("%d MiB", s/1048576) : sprintf("%d KiB", s/1024)),
               m/1024, r/1024, (r-m)/1024, (m>0? 100*(r-m)/m : 0) }'
done
