#!/usr/bin/env bash
# Peak-RSS comparison WITH A NULL ARM.
#
# The first version of this script had no null arm and produced 62.6, 62.8 and
# 51.6 MiB for the SAME binary — an 11 MiB swing that silently invalidated
# three conclusions drawn from it. Two things fix that:
#
#   1. PERL_HASH_SEED=0. Perl randomises its hash seed per process, so every
#      run allocated a DIFFERENT pattern. Pinning it makes the workload
#      deterministic, which is the single biggest variance source here.
#   2. A NULL ARM: rusty_alloc against itself. Whatever spread that reports is
#      the floor; nothing smaller than it is a result. This is the same rule
#      bench/wallclock.sh applies to time, applied to memory.
#
# Reports median, min AND spread. If the null arm's spread is comparable to the
# ra-vs-mi delta, the honest answer is "below measurement resolution".
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"
N="${N:-11}"

# Build a large population, drop it, repeat — the alloc/free-cycling shape
# where FFAI measured a gap. Deterministic under a pinned hash seed.
WORK=(perl -e '
  my %keep;
  for my $r (1..4) {
    my %t;
    for my $i (1..120000) { $t{"k$r-$i"} = [ $i, "v" x 64 ]; }
    $keep{$r} = scalar keys %t;
  }
  print join(",", map { $keep{$_} } sort keys %keep), "\n";')

peak_kib() { # $1 = preload ("" = system), rest = extra env
    local pre="$1"; shift
    local out
    out=$(env PERL_HASH_SEED=0 PERL_PERTURB_KEYS=0 "$@" ${pre:+LD_PRELOAD="$pre"} \
        /usr/bin/time -f "%M" "${WORK[@]}" 2>&1 >/dev/null | tail -1)
    echo "${out:-0}"
}

stats_of() { # newline list of KiB -> "median min max"
    awk 'NF' | sort -g | awk '
        {v[NR]=$1}
        END{ if(NR==0){print "0 0 0"; exit}
             m=(NR%2? v[(NR+1)/2] : (v[NR/2]+v[NR/2+1])/2);
             print m, v[1], v[NR] }'
}

row() { # label, preload, extra env...
    local label="$1" pre="$2"; shift 2
    local vals="" i
    for ((i=0; i<N; i++)); do vals+="$(peak_kib "$pre" "$@")"$'\n'; done
    read -r med lo hi <<<"$(printf '%s' "$vals" | stats_of)"
    awk -v l="$label" -v m="$med" -v a="$lo" -v b="$hi" 'BEGIN{
        printf "%-32s med %8.1f  min %8.1f  max %8.1f  spread %6.1f MiB\n",
               l, m/1024, a/1024, b/1024, (b-a)/1024 }'
}

echo "PEAK RSS — N=$N per arm, PERL_HASH_SEED pinned"
echo
echo "=== NULL ARM (rusty_alloc vs ITSELF) — this is the floor"
row "null A: rusty_alloc"            "$RA"
row "null B: rusty_alloc (same)"     "$RA"
echo
echo "=== comparison"
row "mimalloc"                       "$MI"
row "glibc (system)"                 ""
row "rusty_alloc PURGE_DELAY=10"     "$RA" MIMALLOC_PURGE_DELAY=10
echo
echo "Any ra-vs-mi difference smaller than the NULL ARM's spread is NOT a result."
