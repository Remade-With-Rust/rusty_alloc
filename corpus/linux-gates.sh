#!/usr/bin/env bash
# Full Linux gate battery, quote-safe for wsl.exe invocation.
#
# The exit code is the verdict. An earlier version reported only a count of
# "test result: ok" lines and a grep for FAILED/panicked, which meant a BUILD
# FAILURE printed "failures: 0" — compile errors say neither word. That is not
# a theoretical hole: it reported ok-suites:1 / failures:0 on a build broken by
# an infinite-recursion bug, and was briefly read as a pass.
set -uo pipefail
source ~/.cargo/env 2>/dev/null
cd /mnt/c/Users/talmo/coding/rusty_alloc || exit 1

out=$(CARGO_TARGET_DIR=~/ra_target cargo test --workspace --all-features 2>&1)
status=$?

ok=$(echo "$out" | grep -cE 'test result: ok')
bad=$(echo "$out" | grep -cE 'FAILED|panicked')
echo "ok-suites: $ok"
echo "failures : $bad"
echo "exit-code: $status"

if [ "$status" -ne 0 ]; then
  echo "GATE FAILED (cargo exit $status) — first errors:"
  echo "$out" | grep -E '^error|FAILED|panicked' | head -10
  exit 1
fi
if [ "$bad" -ne 0 ]; then
  echo "GATE FAILED (test failures):"
  echo "$out" | grep -E 'FAILED|panicked' | head -10
  exit 1
fi
# A healthy run builds and runs the whole workspace; a collapse in suite count
# means something stopped compiling even if cargo somehow returned 0.
if [ "$ok" -lt 15 ]; then
  echo "GATE FAILED: only $ok suites ran (expected >= 15) — suspect a build break"
  exit 1
fi
echo "GATE PASSED"
