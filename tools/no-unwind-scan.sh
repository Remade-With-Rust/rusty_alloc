#!/usr/bin/env bash
# The panics a release build can reach from an `Allocator` method, counted
# from the machine code and held to a baseline (a ratchet, like
# `tools/unsafe-census.sh`).
#
# `core::alloc::Allocator`'s contract says none of its methods may unwind, and
# the standard library's own source calls that load-bearing. The impls in
# `rusty_alloc_api::allocator` meet it with a guard that aborts on an unwind.
# The guard is there because of what this script measures: the allocator
# indexes its own tables, and where the optimiser could not prove an index in
# range the release build keeps a bounds check, whose failure is a panic.
# None can fire while the allocator's state is intact. Each is still a path
# the guard has to cover, and an instruction or two on the way past it.
#
# So the count may only go down. At zero the guard has nothing left to guard
# and can be removed, which is worth 27.6 instructions per step of collection
# work on `Vec::new_in(RustyAlloc)` (`docs/opps.md`).
#
# How it is measured: `tools/unwind-probe` exports one function per method of
# both impls and links them with fat LTO into a single module. What is in that
# module's assembly is what the twelve methods can reach. Every instruction in
# one of the allocator's own functions that names a panic entry point is a
# site.
#
# The scan first proves it can fail: the probe's `selftest` feature adds one
# export that indexes a slice, and the pattern must fire on it. A zero from a
# pattern that cannot match would look exactly like a clean result.
#
# Usage:
#   tools/no-unwind-scan.sh            # scan, check the ratchet
#   tools/no-unwind-scan.sh --update   # rewrite the baseline (deliberate act)
#
# The baseline is for x86_64 Linux, where CI runs it; the sites differ by
# target (another OS backend, another optimiser decision). On any other host
# the scan prints its census and does not compare.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
probe="$root/tools/unwind-probe"
baseline="$root/tools/no-unwind-baseline.txt"
mode="${1:-check}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$probe/target}"

# The ways compiled Rust reaches the panic runtime: `core::panicking::*`
# (explicit panics, bounds checks, division by zero, `unreachable!`), the
# `unwrap`/`expect` failure functions, the slice and string index failures,
# and the panic entry itself. Matched on the mangled names, which carry the
# path. These are where an unwind STARTS; `_Unwind_Resume` is a frame an unwind
# passes through, which the site that started it already counts.
PAT='9panicking|13unwrap_failed|13expect_failed|_index_[a-z_]*fail|slice_error_fail|rust_begin_unwind|rust_panic'
# Not sites: the two shims that end an unwind. `panic_cannot_unwind` is where
# an `extern "C"` function stops one, `panic_in_cleanup` where a destructor
# that panics during one does (the guard, when it fires). Both abort.
NOT='panic_cannot_unwind|panic_in_cleanup'

# Emit the probe's assembly and print the path of the `.s`.
emit() { # extra cargo args...
  find "$CARGO_TARGET_DIR/release" -name 'unwind_probe*.s' -delete 2>/dev/null
  # Cargo skips codegen for a fresh crate, which would leave no `.s` at all.
  touch "$probe/src/lib.rs"
  local out
  if ! out=$(cd "$probe" && cargo rustc --release "$@" -- --emit=asm 2>&1); then
    echo "$out" | grep -E '^error' -A6 | head -30 >&2
    return 1
  fi
  # By name under the profile directory: Cargo's build-directory layout is not
  # a stable interface (it moved in 1.100), a file name is.
  find "$CARGO_TARGET_DIR/release" -name 'unwind_probe*.s' 2>/dev/null | head -1
}

# "count  function -> panic entry point", for the allocator's own functions
# and the probes (allocator code that was inlined is attributed to the export
# it landed in). Not the guard's own `Drop`: that panic is the abort, not a
# site. The crate disambiguators (`Cs<hash>_`) are dropped so the names
# survive a compiler or version change.
census() { # file
  awk -v pat="$PAT" -v not="$NOT" '
    /^[A-Za-z_$.?@][A-Za-z0-9_$.?@]*:[ \t]*(#.*)?$/ {
      s = $1; sub(/:.*$/, "", s); if (s !~ /^[.]?L/) sym = s
    }
    $0 ~ pat && $0 !~ not && $0 !~ /^[ \t]*[.]/ &&
    (sym ~ /rusty_alloc/ || sym ~ /^_?probe_(global|heap)_/) && sym !~ /AbortOnUnwind/ {
      t = $NF; sub(/@.*$/, "", t); c[sym " -> " t]++
    }
    END { for (k in c) printf "%4d  %s\n", c[k], k }
  ' "$1" | sed -E 's/Cs[0-9A-Za-z]+_//g' | sort -k2
}

echo "=== self-test: the pattern must fire on code that can panic ==="
f=$(emit --features selftest) || { echo "SELF-TEST BUILD FAILED"; exit 1; }
[ -z "$f" ] && { echo "SELF-TEST: no .s emitted (the scan would be vacuous)"; exit 1; }
n=$(awk '/^_?probe_selftest_can_panic:/ {on=1; next} on && /^[A-Za-z_]/ {on=0} on' "$f" | grep -cE "$PAT")
echo "  panic references inside the export that indexes a slice: $n"
if [ "$n" -eq 0 ]; then
  echo "  *** PATTERN IS BLIND: a clean result from the real scan would mean nothing ***"
  grep -iE 'panic|unwind' "$f" | head -5 | sed 's/^/    /'
  exit 1
fi

echo
echo "=== the twelve Allocator methods, release, fat LTO ==="
f=$(emit) || { echo "BUILD FAILED"; exit 1; }
[ -z "$f" ] && { echo "no .s emitted"; exit 1; }
# The probes themselves must be there, or an empty object would pass.
probes=$(grep -cE '^_?probe_(global|heap)_[a-z_]+:' "$f")
echo "  assembly lines: $(wc -l < "$f")   probe functions found: $probes"
if [ "$probes" -ne 12 ]; then
  echo "  expected 12 probe functions; the object is not what this scan thinks it is"
  exit 1
fi

cur=$(mktemp); census "$f" > "$cur"
cur_total=$(awk '{s+=$1} END{print s+0}' "$cur")
echo "  panic sites in the allocator's own functions: $cur_total, in $(wc -l < "$cur") places"
echo

host="$(rustc -vV | awk '/^host:/ {print $2}')"
if [ "$mode" = "--update" ]; then
  cp "$cur" "$baseline"; rm -f "$cur"
  echo "baseline updated ($host): $cur_total sites"
  exit 0
fi
if [ "$host" != "x86_64-unknown-linux-gnu" ]; then
  cat "$cur"; rm -f "$cur"
  echo
  echo "host is $host: the baseline is for x86_64-unknown-linux-gnu, not compared."
  exit 0
fi
if [ ! -f "$baseline" ]; then
  echo "no baseline at $baseline: run with --update"; rm -f "$cur"; exit 2
fi
base_total=$(awk '{s+=$1} END{print s+0}' "$baseline")
diff -u "$baseline" "$cur" | grep -E '^[+-][^+-]' && echo
if [ "$cur_total" -gt "$base_total" ]; then
  cat <<EOF
NO-UNWIND RATCHET FAILED: panic sites grew $base_total -> $cur_total.

A new site is a new path on which an Allocator method would unwind if the
guard were not there, and an index the compiler could not prove. Prove it
(mask the index, carry the bound) or make the failure an abort, as the
allocator's corruption paths are. Only if neither is possible, re-run with
--update and say why in the commit.
EOF
  rm -f "$cur"; exit 1
fi
if [ "$cur_total" -lt "$base_total" ]; then
  echo "panic sites DECREASED ($base_total -> $cur_total). Run --update to lock in the gain."
fi
rm -f "$cur"
echo "NO-UNWIND RATCHET OK ($cur_total sites, baseline $base_total)"
