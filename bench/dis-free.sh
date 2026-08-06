#!/usr/bin/env bash
# Disassemble the shipped cdylib's exported malloc/free.
#
# The deterministic profile gives Ir/call; this gives the actual instruction
# sequence those come from. Use it when the profile blames inlined core code
# (`ptr/mut_ptr.rs`) that has no source on the box to annotate.
set -uo pipefail
L="${RA_OVERRIDE_LIB:-$HOME/ra_target/release/librusty_alloc_override.so}"
sym="${1:-free}"
n="${2:-70}"
dis="$HOME/ra_dis.txt"

if [ ! -f "$dis" ] || [ "$L" -nt "$dis" ]; then
  objdump -d -M intel --no-show-raw-insn "$L" >"$dis"
fi

if [ "$sym" = "--list" ]; then
  echo "== function labels in the disassembly:"
  grep -E '^[0-9a-f]+ <' "$dis" | head -60
  exit 0
fi

echo "== $sym (first $n lines)"
grep -A "$n" -m1 -E "^[0-9a-f]+ <${sym}>:" "$dis"
