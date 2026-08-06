#!/usr/bin/env bash
# Builds the three C mimalloc oracle arms (Linux/WSL2/macOS). Dev-only.
#   mi  — release; dmi — debug+MI_DEBUG_FULL; smi — release+MI_SECURE
# Output: oracle/out/<arm>/ (each contains libmimalloc.so used by bench.sh arms)
set -euo pipefail
root="$(cd "$(dirname "$0")" && pwd)"
src="$root/mimalloc"
[ -f "$src/CMakeLists.txt" ] || { echo "oracle/mimalloc is empty — run: git submodule update --init oracle/mimalloc" >&2; exit 1; }

build() { # name cmake-flags...
    local name="$1"; shift
    # OS-namespaced: repo is shared with Windows via /mnt/c — mixed-OS cmake
    # caches in one dir corrupt both builds (learned 2026-08-05).
    local out="$root/out/$(uname -s | tr '[:upper:]' '[:lower:]')/$name"
    echo "== building oracle arm '$name' -> $out"
    cmake -S "$src" -B "$out" -DMI_BUILD_TESTS=OFF "$@"
    cmake --build "$out" --parallel
}

build mi  -DCMAKE_BUILD_TYPE=Release
build dmi -DCMAKE_BUILD_TYPE=Debug -DMI_DEBUG_FULL=ON
build smi -DCMAKE_BUILD_TYPE=Release -DMI_SECURE=ON
echo "== oracle arms built (mi_version should report 20405 = v2.4.5)"
