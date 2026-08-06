# wasm32 gate: build the allocator for WebAssembly and RUN it in a VM.
#
# `cargo test` cannot execute wasm32-unknown-unknown, so correctness is proven
# by instantiating the cdylib under Node and calling its self-test export. The
# same self-test also runs natively via `cargo test -p rusty_alloc-wasm`, so a
# failure here is specifically a WASM failure.
#
# Exits non-zero on any failure, so it can be chained into the gate battery.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

Write-Host "== cargo check (wasm32-unknown-unknown)"
cargo check -p rusty_alloc --target wasm32-unknown-unknown
if ($LASTEXITCODE -ne 0) { Write-Host "WASM GATE FAILED: core crate does not compile"; exit 1 }

Write-Host "== build selftest cdylib"
cargo build -p rusty_alloc-wasm --target wasm32-unknown-unknown --release
if ($LASTEXITCODE -ne 0) { Write-Host "WASM GATE FAILED: selftest build"; exit 1 }

# NOTE: the PACKAGE is `rusty_alloc-wasm`, but cargo maps the hyphen to an
# underscore for the artifact, so the file is `rusty_alloc_wasm.wasm`.
$wasm = "target\wasm32-unknown-unknown\release\rusty_alloc_wasm.wasm"
if (-not (Test-Path $wasm)) { Write-Host "WASM GATE FAILED: $wasm not produced"; exit 1 }

Write-Host "== run in a WebAssembly VM (node)"
node bench\wasm-selftest.mjs $wasm
if ($LASTEXITCODE -ne 0) { Write-Host "WASM GATE FAILED: selftest returned $LASTEXITCODE"; exit 1 }

Write-Host "WASM GATE PASSED"
exit 0
