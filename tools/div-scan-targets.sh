#!/usr/bin/env bash
# Division audit for the targets no scan in this campaign has read: macOS
# (both arches) and wasm32. `--emit=asm` needs no linker, so these
# cross-compile fine from Linux.
#
# Instruction names differ per ISA, which is the whole reason a Linux-x86 grep
# could never have found them:
#   x86-64   div idiv divsd divss
#   aarch64  udiv sdiv fdiv
#   wasm32   i32.div_u i64.rem_s f64.div ...
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$HOME/ra_xt}

scan() {
  local target="$1" pat="$2"
  echo "########## $target"
  rustup target add "$target" >/dev/null 2>&1
  rm -f "$CARGO_TARGET_DIR/$target/release/deps/"*.s 2>/dev/null
  # Cargo skips codegen for a crate it considers fresh, which yields NO .s at
  # all — so force it. Without this the scan reports "no asm" rather than a
  # count, which is at least loud, but it has to actually run.
  touch crates/rusty_alloc/src/lib.rs
  local out
  out=$(cargo rustc --release -p rusty_alloc --target "$target" -- --emit=asm 2>&1)
  if echo "$out" | grep -qE '^error'; then
    echo "  BUILD FAILED:"; echo "$out" | grep -E '^error' -A3 | head -6; echo; return
  fi
  local f
  f=$(find "$HOME/ra_xt/$target/release/deps" -name 'rusty_alloc-*.s' 2>/dev/null | head -1)
  [ -z "$f" ] && { echo "  no .s emitted"; echo; return; }
  echo "  asm lines: $(wc -l < "$f")"
  local n
  n=$(grep -cE "$pat" "$f")
  echo "  division instructions: $n"
  if [ "$n" -gt 0 ]; then
    awk -v pat="$pat" '
      /^(_?[A-Za-z_$.][A-Za-z0-9_$.@]*):[ \t]*$/ { s=$0; sub(/:[ \t]*$/,"",s); if (s !~ /^[.]/) sym=s }
      $0 ~ pat { c[sym]++ }
      END { for (k in c) printf "    %4d  %s\n", c[k], k }
    ' "$f" | sort -rn | head -12
  fi
  echo
}

scan x86_64-apple-darwin  '^[[:space:]]+(idiv|div)[qlbw]?[[:space:]]|^[[:space:]]+v?div[sp][sd][[:space:]]'
scan aarch64-apple-darwin '^[[:space:]]+(udiv|sdiv|fdiv)[[:space:]]'
scan wasm32-unknown-unknown '(i32|i64|f32|f64)\.(div_[us]|rem_[us]|div)([[:space:]]|$)'
