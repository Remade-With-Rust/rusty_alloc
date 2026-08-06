# Builds the three C mimalloc oracle arms (Windows / MSVC). Dev-only — the
# oracle is never a runtime dependency (plan §1 principle 2).
#   mi  — release            (the performance oracle)
#   dmi — debug, MI_DEBUG_FULL (the correctness oracle: padding, internal asserts)
#   smi — release, MI_SECURE   (the hardening oracle for the M8 `ras` comparison)
# Requires: cmake + an MSVC toolchain (VS Build Tools). Output: oracle/out/<arm>/

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$src = Join-Path $root "mimalloc"
if (-not (Test-Path (Join-Path $src "CMakeLists.txt"))) {
    throw "oracle/mimalloc is empty - run: git submodule update --init oracle/mimalloc"
}

$arms = @(
    @{ name = "mi";  flags = @("-DCMAKE_BUILD_TYPE=Release") },
    @{ name = "dmi"; flags = @("-DCMAKE_BUILD_TYPE=Debug", "-DMI_DEBUG_FULL=ON") },
    @{ name = "smi"; flags = @("-DCMAKE_BUILD_TYPE=Release", "-DMI_SECURE=ON") }
)

foreach ($arm in $arms) {
    # OS-namespaced: the repo is shared with WSL2 — mixed-OS cmake caches in
    # one dir corrupt both builds (learned 2026-08-05).
    $out = Join-Path $root "out\win\$($arm.name)"
    Write-Host "== building oracle arm '$($arm.name)' -> $out"
    $cfg = "Release"; if ($arm.name -eq "dmi") { $cfg = "Debug" }
    cmake -S $src -B $out @($arm.flags) -DMI_BUILD_TESTS=OFF
    if ($LASTEXITCODE -ne 0) { throw "cmake configure failed for $($arm.name)" }
    cmake --build $out --config $cfg --parallel
    if ($LASTEXITCODE -ne 0) { throw "cmake build failed for $($arm.name)" }
}
Write-Host "== oracle arms built. Version check:"
Write-Host "   (mi_version should report 20405 = v2.4.5)"
