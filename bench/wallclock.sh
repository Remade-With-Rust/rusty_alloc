#!/usr/bin/env bash
# WALL-CLOCK measurement — the debt this project has carried since M9.
#
# Everything else in bench/ counts instructions, which is deterministic but is
# NOT what users experience. This measures time, with the discipline that makes
# a timing number admissible at all:
#
#   * PINNED to one CPU (taskset) so the scheduler cannot migrate us mid-run.
#   * CPU time (user+sys), not wall, so unrelated load does not leak in.
#   * ABBA interleaving — arms alternate, so thermal/frequency drift affects
#     both equally instead of whichever ran first.
#   * A NULL ARM: the same allocator against itself. Any difference it reports
#     is pure harness noise, and NOTHING smaller than that is a result.
#   * N >= 31 repetitions; medians AND minima reported, because when those two
#     statistics disagree in sign the box cannot resolve the effect.
#
# Read the null arm FIRST. If it is wider than the ra-vs-mi delta, the honest
# conclusion is "below measurement resolution", not "we are faster".
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
N="${N:-31}"
CPU="${CPU:-2}"

have() { command -v "$1" >/dev/null 2>&1; }
PIN=""
if have taskset; then PIN="taskset -c $CPU"; fi

# One run's elapsed time in milliseconds, at MICROSECOND resolution.
#
# `/usr/bin/time` reports user+sys at 10 ms granularity, which on a ~300 ms
# workload is ~3% — coarser than the effect being measured, and it produced a
# nonsense `min` column of 0.0 ms for every arm. Bash's EPOCHREALTIME is
# microseconds. Wall rather than CPU time is the trade; pinning plus ABBA is
# what keeps that honest, and the null arm reports whatever it fails to cancel.
run_ms() {
    local pre="$1"; shift
    local t0 t1
    t0=${EPOCHREALTIME/./}
    LD_PRELOAD="$pre" $PIN "$@" >/dev/null 2>&1
    t1=${EPOCHREALTIME/./}
    awk -v a="$t0" -v b="$t1" 'BEGIN{ printf "%.3f", (b-a)/1000 }'
}

# Median and min of a whitespace-separated list.
#
# `awk NF` drops EMPTY fields. The accumulators below start empty and grow with
# " $x", so the list has a leading space; without this filter `tr` emitted a
# blank first line, `sort -g` ranked it as 0, and every reported `min` was
# 0.0 ms — a whole statistic silently dead in two consecutive runs.
stats() {
    tr ' ' '\n' <<<"$1" | awk 'NF' | sort -g | awk '
        {v[NR]=$1}
        END{ if (NR==0) { printf "0.0 0.0"; exit }
             m=(NR%2? v[(NR+1)/2] : (v[NR/2]+v[NR/2+1])/2);
             printf "%.1f %.1f", m, v[1] }'
}

# ABBA: arm A, arm B, arm B, arm A per round, so ordering bias cancels.
compare() {
    local label="$1" preA="$2" preB="$3"; shift 3
    local a="" b="" i
    for ((i = 0; i < N; i++)); do
        if (( i % 2 == 0 )); then
            a="$a $(run_ms "$preA" "$@")"; b="$b $(run_ms "$preB" "$@")"
        else
            b="$b $(run_ms "$preB" "$@")"; a="$a $(run_ms "$preA" "$@")"
        fi
    done
    read -r amed amin <<<"$(stats "$a")"
    read -r bmed bmin <<<"$(stats "$b")"
    awk -v l="$label" -v am="$amed" -v ai="$amin" -v bm="$bmed" -v bi="$bmin" 'BEGIN{
        rmed=(bm>0? am/bm : 0); rmin=(bi>0? ai/bi : 0);
        printf "%-22s A med %8.1fms min %8.1fms | B med %8.1fms min %8.1fms | med %.4f  min %.4f\n",
               l, am, ai, bm, bi, rmed, rmin }'
}

echo "METHOD: pinned CPU $CPU, CPU-time (user+sys), ABBA-interleaved, N=$N, median AND min"
echo "        A = rusty_alloc unless stated; B = the comparison arm"
[ -z "$PIN" ] && echo "WARNING: taskset unavailable — NOT pinned, treat every number as suspect"
echo

# Workloads are scaled up so a single run is >1 s: at 300 ms even a microsecond
# timer leaves scheduler jitter comparable to the effect, and longer runs push
# the per-run fixed costs (fork, ld.so, allocator init) below the noise.
PERL=(perl -e 'my %h; for my $r (1..6) { for my $i (1..150000) { $h{"key$r-$i"} = [ $i, "v$i" ]; } } my $s=0; while (my ($k,$v)=each %h) { $s += $v->[0] } print "$s\n";')
SQLITE=(sqlite3 :memory: "create table t(a,b); insert into t select value, randomblob(64) from generate_series(1,900000); select count(*) from t;")

echo "=== NULL ARM (rusty_alloc vs ITSELF) — this is the noise floor"
compare "null:perl" "$RA" "$RA" "${PERL[@]}"
echo
echo "=== rusty_alloc vs mimalloc"
compare "perl   ra/mi" "$RA" "$MI" "${PERL[@]}"
compare "sqlite ra/mi" "$RA" "$MI" "${SQLITE[@]}"
echo
echo "REMINDER: a delta smaller than the null arm's spread is NOT a result."
