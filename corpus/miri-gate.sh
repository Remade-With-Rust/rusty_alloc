#!/usr/bin/env bash
# Miri gate — the memory-safety instrument.
#
# Miri has twice caught defects in this project that no other gate saw (the M4
# heap registry and the M7 arena base, both "reachability and provenance follow
# POINTERS, not integers"). It is slow, so it runs the core suites only.
#
# KNOWN BLIND SPOT: the x86-64 Linux fast paths that use inline asm — the
# thread-pointer read in `init::thread_id` and the initial-exec TLS slot in
# `init::heap_tls` — are `cfg(not(miri))`, so Miri exercises their
# `thread_local!` fallbacks instead. Those two need hardware gates
# (bench/churn.sh, the corpus sweep), not Miri.
set -uo pipefail
source ~/.cargo/env 2>/dev/null
cd "$(cd "$(dirname "$0")/.." && pwd)" || exit 1

export CARGO_TARGET_DIR="$HOME/ra_target_miri"
export MIRIFLAGS="${MIRIFLAGS:--Zmiri-disable-isolation}"

suites="${*:-alloc_core spans heaps secure prim}"
fail=0
for s in $suites; do
  echo "== miri: $s"
  # PIPESTATUS, not the pipeline's status: piping to `tail` would otherwise
  # report tail's exit code (always 0) and turn every failure into a pass.
  cargo +nightly miri test --test "$s" 2>&1 | tail -15
  if [ "${PIPESTATUS[0]}" -ne 0 ]; then
    echo "MIRI FAILED: $s"
    fail=1
  fi
done
if [ "$fail" -eq 0 ]; then
  echo "miri gate: all suites clean"
fi
exit "$fail"
