#!/usr/bin/env bash
# Prove the cross-target scan is not vacuous. A crate that DOES divide must
# make each pattern fire; if it does not, a zero from the real scan means
# nothing. This is the check the fast-trans postscript demands.
set -uo pipefail
D=$HOME/divprobe
rm -rf "$D"; mkdir -p "$D/src"
cat > "$D/Cargo.toml" <<'TOML'
[package]
name = "divprobe"
version = "0.0.0"
edition = "2021"
[lib]
crate-type = ["lib"]
[profile.release]
opt-level = 3
TOML
cat > "$D/src/lib.rs" <<'RS'
// Runtime divisors, so nothing can be strength-reduced away.
#[inline(never)]
pub fn u64_div(a: u64, b: u64) -> u64 { a / b }
#[inline(never)]
pub fn i32_rem(a: i32, b: i32) -> i32 { a % b }
#[inline(never)]
pub fn f64_div(a: f64, b: f64) -> f64 { a / b }
RS

cd "$D" || exit 1
export CARGO_TARGET_DIR=$D/target

probe() {
  local target="$1" pat="$2"
  rustup target add "$target" >/dev/null 2>&1
  cargo rustc --release --target "$target" -- --emit=asm >/dev/null 2>&1
  local f
  f=$(find "$D/target/$target/release/deps" -name 'divprobe-*.s' 2>/dev/null | head -1)
  if [ -z "$f" ]; then echo "  $target: NO ASM EMITTED (scan would be vacuous)"; return; fi
  local n
  n=$(grep -cE "$pat" "$f")
  printf "  %-28s pattern hits in a KNOWN-dividing crate: %s\n" "$target" "$n"
  if [ "$n" -gt 0 ]; then
    grep -oE "$pat" "$f" | sort | uniq -c | sed 's/^/      /' | head -6
  else
    echo "      *** PATTERN IS BLIND — the real scan's zero is meaningless ***"
    echo "      sample of what the file actually contains:"
    grep -iE 'div|rem' "$f" | head -5 | sed 's/^/        /'
  fi
}

echo "=== instrument self-test ==="
probe x86_64-apple-darwin  '^[[:space:]]+(idiv|div)[qlbw]?[[:space:]]|^[[:space:]]+v?div[sp][sd][[:space:]]'
probe aarch64-apple-darwin '^[[:space:]]+(udiv|sdiv|fdiv)[[:space:]]'
probe wasm32-unknown-unknown '(i32|i64|f32|f64)\.(div_[us]|rem_[us]|div)([[:space:]]|$)'
