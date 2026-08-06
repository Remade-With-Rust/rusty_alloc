#!/usr/bin/env bash
# Rebuild the override cdylib and run the deterministic A/B instrument.
# One entry point so the measurement is always taken against a freshly built
# artifact (the M10 lesson: measure the artifact you ship).
set -uo pipefail
cd "$(cd "$(dirname "$0")/.." && pwd)" || exit 1
export CARGO_TARGET_DIR="$HOME/ra_target"
echo "== build"
if ! cargo build --release -p rusty_alloc-override 2>&1 | tail -6; then
  echo "BUILD FAILED"
  exit 1
fi
echo "== measure"
exec bash bench/icount-arms.sh
