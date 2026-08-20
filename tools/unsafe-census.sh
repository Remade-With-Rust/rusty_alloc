#!/usr/bin/env bash
# Unsafe inventory with a RATCHET (hardening gate H-11).
#
# The registry's probe for H-11 is `cargo geiger`, which does not build on
# this workspace's pinned toolchain (1.97.1) in any version tried — 0.13.0,
# 0.12.0 and 0.11.7 all fail to compile. Recorded as a SUBSTITUTION, which
# the registry permits, because the gate's INTENT is what matters: "unsafe
# inventory measured and trending down".
#
# geiger's real speciality is counting unsafe in DEPENDENCIES. That is close
# to worthless here — the runtime dependency set is `libc` plus bindings-only
# `windows-sys`, both of which are unsafe by nature and both certified via
# cargo-vet (H-10). What actually needs a ratchet is OUR OWN unsafe, and that
# is what this counts, per module, against a committed baseline.
#
# Usage:
#   tools/unsafe-census.sh            # print the census, check the ratchet
#   tools/unsafe-census.sh --update   # rewrite the baseline (deliberate act)
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
baseline="$root/tools/unsafe-baseline.txt"
mode="${1:-check}"

census() {
    # Count `unsafe` in CODE per shipped source file: unsafe fns, unsafe
    # blocks and unsafe impls alike.
    #
    # Comments are stripped first, and that is not cosmetic. The first
    # version counted the literal string anywhere, so adding `src/proofs.rs`
    # — whose only occurrence is the word "unsafe" in a doc comment
    # explaining what the proofs are for — tripped the ratchet. A gate that
    # cries wolf over prose is a gate people learn to re-baseline without
    # reading, which is worse than no gate at all.
    #
    # Still deliberately coarse in the other direction (one count per line,
    # no parsing): a ratchet wants a number that cannot drift DOWN by
    # accident, and over-counting real code is safe in that direction.
    for f in $(find "$root/crates" -path '*/src/*.rs' | sort); do
        rel="${f#"$root"/}"
        n=$(sed -e 's://!.*::' -e 's:///.*::' -e 's://.*::' "$f" | grep -c '\bunsafe\b')
        [ "$n" -gt 0 ] && printf '%6d  %s\n' "$n" "$rel"
    done
}

if [ "$mode" = "--update" ]; then
    census > "$baseline"
    total=$(awk '{s+=$1} END{print s}' "$baseline")
    echo "baseline updated: $total occurrences across $(wc -l < "$baseline") files"
    exit 0
fi

cur=$(mktemp); census > "$cur"
cur_total=$(awk '{s+=$1} END{print s+0}' "$cur")

if [ ! -f "$baseline" ]; then
    echo "no baseline at $baseline — run with --update"; rm -f "$cur"; exit 2
fi
base_total=$(awk '{s+=$1} END{print s+0}' "$baseline")

echo "unsafe census: $cur_total occurrences (baseline $base_total)"
echo
diff -u "$baseline" "$cur" | grep -E '^[+-][^+-]' && echo

if [ "$cur_total" -gt "$base_total" ]; then
    cat <<EOF
RATCHET FAILED: unsafe grew $base_total -> $cur_total.

That is not automatically wrong — an allocator legitimately adds unsafe — but
it must be DELIBERATE. Add the new sites to crates/rusty_alloc/UNSAFE.md with
their purpose and audit date, then re-run with --update in the same commit.
EOF
    rm -f "$cur"; exit 1
fi

if [ "$cur_total" -lt "$base_total" ]; then
    echo "unsafe DECREASED ($base_total -> $cur_total). Run --update to lock in the gain."
fi
rm -f "$cur"
echo "RATCHET OK"
