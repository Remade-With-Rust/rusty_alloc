#!/usr/bin/env bash
# Does the RSS gap SCALE with the workload, or is it a fixed overhead?
#
# This is the cheap experiment that eliminates whole classes of cause at once:
#
#   * gap roughly CONSTANT  -> fixed overhead: segment headers, a cached empty
#                              segment, arena bookkeeping. Bounded, and it
#                              stops mattering as the heap grows.
#   * gap roughly PROPORTIONAL -> per-object or per-page waste: bin rounding,
#                              page utilisation, fragmentation. Unbounded, and
#                              it is a real design problem.
#
# Same binary, same work, only the population size changes.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
RA="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
MI="${MI_ORACLE_LIB:-$root/oracle/out/linux/mi/libmimalloc.so}"

peak_kib() { # preload, n
    local pre="$1" n="$2" out
    out=$(env ${pre:+LD_PRELOAD="$pre"} /usr/bin/time -f "%M" \
        perl -e "my %h; for my \$i (1..$n) { \$h{\"key\$i\"} = [ \$i, \"v\" x 64 ]; } print scalar keys %h, \"\n\";" \
        2>&1 >/dev/null | tail -1)
    echo "${out:-0}"
}

best() { # preload, n  -> best of 3, KiB
    local b=99999999 i v
    for i in 1 2 3; do v=$(peak_kib "$1" "$2"); [ "$v" -lt "$b" ] 2>/dev/null && b=$v; done
    echo "$b"
}

printf "%-10s %12s %12s %12s %10s\n" entries mimalloc rusty_alloc gap gap%
printf "%-10s %12s %12s %12s %10s\n" ------- -------- ----------- --- ----
for n in 50000 100000 200000 400000; do
    m=$(best "$MI" "$n")
    r=$(best "$RA" "$n")
    awk -v n="$n" -v m="$m" -v r="$r" 'BEGIN{
        printf "%-10d %9.1f MiB %9.1f MiB %9.1f MiB %9.1f%%\n",
               n, m/1024, r/1024, (r-m)/1024, (m>0? 100*(r-m)/m : 0) }'
done
echo
echo "Constant gap => fixed overhead. Growing gap => per-object/per-page waste."
