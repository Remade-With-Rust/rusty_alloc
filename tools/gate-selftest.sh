#!/usr/bin/env bash
# Prove the load-bearing tests are not vacuous.
#
# A green test is not evidence until you have seen it go red for the right
# reason. This campaign found FOUR tests that passed under the exact bug they
# existed to catch — a region-shape that could not tell two placement policies
# apart, an arena too large to starve, a threshold below both arms, and an
# assertion that held with no region registered at all. Every one of them looked
# like coverage and was worth nothing.
#
# So: reintroduce each defect, run the test that owns it, and require it to
# FAIL. A mutation that leaves the suite green is a gate that is gone, and this
# script exits non-zero saying so.
#
# Sibling of `tools/semgrep-selftest.sh` (same idea, applied to lint rules) and
# `tools/unsafe-census.sh`. Add an entry here whenever you fix a defect that a
# test now guards; that is the cheapest moment, because the mutation is the diff
# you just reversed.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

pass=0
fail=0
declare -a FAILED

# name | file | perl -0pe substitution | cargo test filter | extra RUSTFLAGS
run_case() {
  local name="$1" file="$2" subst="$3" filter="$4" flags="${5:-}"
  local backup
  backup="$(mktemp)"
  cp "$file" "$backup"
  # shellcheck disable=SC2064
  trap "cp '$backup' '$file'; rm -f '$backup'" RETURN

  if ! perl -0pi -e "$subst" "$file"; then
    echo "  !! $name: mutation could not be applied (source moved?)"
    FAILED+=("$name (mutation did not apply)")
    fail=$((fail + 1))
    return
  fi
  if cmp -s "$backup" "$file"; then
    echo "  !! $name: mutation matched NOTHING — the anchor has moved"
    FAILED+=("$name (anchor moved)")
    fail=$((fail + 1))
    return
  fi

  local out
  if out="$(RUSTFLAGS="$flags" cargo test -p rusty_alloc $filter 2>&1)"; then
    echo "  !! $name: VACUOUS — the suite is GREEN with the defect present"
    FAILED+=("$name")
    fail=$((fail + 1))
  else
    # A compile error is not the gate firing; it means the mutation was invalid.
    if grep -qE '^error(\[E[0-9]+\])?: ' <<<"$out" && ! grep -q 'test result: FAILED' <<<"$out"; then
      echo "  !! $name: mutation did not COMPILE — rewrite it so it is a real defect"
      FAILED+=("$name (did not compile)")
      fail=$((fail + 1))
    else
      echo "  ok $name: gate fires"
      pass=$((pass + 1))
    fi
  fi
}

echo "poisoning each load-bearing gate; every one must go red"
echo

# --- P4b: two-ended placement in the fixed-region backend -------------------
# Bottom-up first-fit puts a page-sized block below the first segment boundary
# and costs a whole segment of reach (small-metal §2.10).
run_case "prim::fixed two-ended placement" \
  "crates/rusty_alloc/src/prim/fixed.rs" \
  's/let from_top = align == FIXED_PAGE;/let from_top = false;/' \
  "--lib prim::fixed"

# --- P4d/P4e: collect must reclaim a bin's last all-free page ---------------
run_case "collect reclaims a bin's last page" \
  "crates/rusty_alloc/src/heap.rs" \
  's/if page_all_free\(p\) \{/if page_all_free(p) \&\& !((*q).first == p \&\& (*q).last == p) {/' \
  "--test heaps collect_reclaims"

# --- P4e: the periodic collect must actually be wired ----------------------
run_case "generic_collect fires on its own" \
  "crates/rusty_alloc/src/heap.rs" \
  's/unsafe \{ self\.collect_inner\(false, false\) \};/{}/' \
  "--test heaps generic_collect_fires"

# --- P4e: the generic path must reclaim before reporting OOM ---------------
# Small profile only: the cache starves a heap only where slices are scarce.
run_case "generic path reclaims before null" \
  "crates/rusty_alloc/src/heap.rs" \
  's/unsafe \{ self\.collect_inner\(true, true\) \};/{}/' \
  "--test heaps generic_path_reclaims" \
  "--cfg ra_small_profile"

# --- P3: options 64-bit atomics on a target without them -------------------
# Forwarding the caller's Ordering to 32-bit halves aborts on AcqRel.
run_case "split64 normalises orderings" \
  "crates/rusty_alloc/src/options.rs" \
  's/self\.lo\.load\(Ordering::Acquire\)/self.lo.load(_ord)/' \
  "--lib options::split64"

# --- firmware-code-size lever 1: ONE_THREAD must be FALSE on a hosted target -
# The single-context predicate folds every free to local and every adopt to
# null. If it ever read true where threads exist, a dying thread's segment
# would be abandoned into an `unreachable!` and cross-thread frees would land
# on the wrong heap. The subproc half of this test exits a thread with a live
# block, which is exactly the path the predicate prunes.
run_case "ONE_THREAD is false where threads exist"   "crates/rusty_alloc/src/lib.rs"   's/pub\(crate\) const ONE_THREAD: bool = cfg!\(any\(/pub(crate) const ONE_THREAD: bool = true || cfg!(any(/'   "--test heaps heaps_arenas_subprocs_options"

# --- firmware-what-is-left §1: good_region_size must do the arithmetic ------
# A const fn that returned a plausible number for the reported case but got the
# granule wrong would strand memory on every board that trusted it.
run_case "good_region_size strands nothing" \
  "crates/rusty_alloc/src/prim/fixed.rs" \
  's/\(budget \/ seg\) \* seg\n\}/budget\n}/' \
  "--lib prim::fixed::tests::good_region_size" \
  "--cfg ra_single_threaded --cfg ra_small_profile"

# --- firmware-what-is-left §3: ONE_REGION must be FALSE on a hosted target ----
# It folds arenas, the segment map and the option table to what one region
# needs. On a host with many OS ranges that is wrong. NOT the exclusive-arena
# test: that one skips, honestly, when a reservation fails, so it stayed green
# under this very mutation (found by this script on 2026-09-09). The broad
# heaps test `expect`s its arena reservation and asserts that `options::set`
# round-trips, and both go red.
run_case "ONE_REGION is false where arenas exist"   "crates/rusty_alloc/src/lib.rs"   's/pub\(crate\) const ONE_REGION: bool = FIXED_REGION \&\& /pub(crate) const ONE_REGION: bool = true || FIXED_REGION \&\& /'   "--test heaps heaps_arenas_subprocs_options"

# --- region-alignment-dissolve: the arena layer folds on FIXED_REGION too ---
# Forced true on a host, `segment_of` still resolves (a base of 0 is the
# mask) but every arena entry point is refused, and the same test goes red.
run_case "FIXED_REGION is false where an OS exists"   "crates/rusty_alloc/src/lib.rs"   's/pub\(crate\) const FIXED_REGION: bool = cfg!\(all\(/pub(crate) const FIXED_REGION: bool = true || cfg!(all(/'   "--test heaps heaps_arenas_subprocs_options"

# --- region-alignment-bug §3: init_region must refuse a segment-costing base -
# The refusal is one comparison; remove it and the exact-size-at-the-linker's-
# base probe is accepted and served two thirds of its heap, which is the
# startup panic the Janus firmware reported.
run_case "init_region refuses a misaligned exact region"   "crates/rusty_alloc/src/prim/fixed.rs"   's/if usable_bytes\(base, len\) < usable_bytes\(0, len\) \{/if false {/'   "--lib prim::fixed::tests::a_misaligned_exact_region"   "--cfg ra_single_threaded --cfg ra_small_profile"

echo
if ((fail > 0)); then
  echo "GATE SELFTEST FAILED: $fail of $((pass + fail)) gates did not fire."
  for f in "${FAILED[@]}"; do echo "  - $f"; done
  echo
  echo "A gate that stays green with its defect reintroduced is not protecting"
  echo "anything. Either the test needs to be able to fail, or the mutation no"
  echo "longer describes the defect and this script needs updating."
  exit 1
fi
echo "GATE SELFTEST OK: all $pass gates fire."
