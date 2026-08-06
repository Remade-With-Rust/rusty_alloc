# pinvs.ps1 - THE compliant paired A/B timing harness (plan section 7.4 rule 1).
# Ported from the rs_h264 shape. Every Windows perf number routes through this
# script; a second timing implementation is a second place for discipline to rot.
#
# ASCII ONLY. Windows PowerShell 5.1 reads a UTF-8 file as Windows-1252, and a
# stray non-ASCII byte turns into a parse error two lines later (learned the
# hard way, M9). Do not add smart quotes, em-dashes or arrows to this file.
#
# Shape (codec-measurement sections 1-3, 13):
#   - each invocation pinned to one core (not core 0), High priority
#   - CPU time (TotalProcessorTime) via a cached process handle, not wall
#   - arms ABBA-alternated: pair i runs A,B on even i and B,A on odd i
#   - reports per-arm medians, paired win rate, z-score, and its METHOD LINE
#   - refuses to report effects below timer resolution
#   - null arm: pass the same command as -ExeA and -ExeB to find the floor
#
# Usage:
#   powershell -File bench/pinvs.ps1 -ExeA .\a.exe -ArgsA "run x" `
#       -ExeB .\b.exe -ArgsB "run x" -Pairs 21 -Core 2 -Label "what this is"
#
# Cross-binary standing claims need -Pairs 31+ and a stable median across
# N=15/31/41 (codec-measurement section 16). Same-binary deltas resolve at N>=20.

param(
    [Parameter(Mandatory = $true)][string]$ExeA,
    [string]$ArgsA = "",
    [Parameter(Mandatory = $true)][string]$ExeB,
    [string]$ArgsB = "",
    [int]$Pairs = 20,
    [int]$Core = 2,
    [string]$Label = "unlabeled"
)

$ErrorActionPreference = "Stop"

function Invoke-Pinned {
    param([string]$Exe, [string]$Arguments, [int]$CoreIndex)
    $psi = @{ FilePath = $Exe; PassThru = $true; WindowStyle = "Hidden" }
    if ($Arguments -ne "") { $psi.ArgumentList = $Arguments }
    $p = Start-Process @psi
    $null = $p.Handle   # MUST precede WaitForExit or TotalProcessorTime reads empty
    try {
        $p.ProcessorAffinity = [IntPtr](1 -shl $CoreIndex)
        $p.PriorityClass = "High"
    } catch {}          # races process exit on very short runs; the time still reads
    $p.WaitForExit()
    if ($p.ExitCode -ne 0) {
        throw "$Exe exited $($p.ExitCode): a failing arm voids the comparison (work-parity rule)"
    }
    return $p.TotalProcessorTime.TotalMilliseconds
}

$a = New-Object System.Collections.Generic.List[double]
$b = New-Object System.Collections.Generic.List[double]
$winsB = 0
$ties = 0

for ($i = 0; $i -lt $Pairs; $i++) {
    if ($i % 2 -eq 0) {
        $ta = Invoke-Pinned -Exe $ExeA -Arguments $ArgsA -CoreIndex $Core
        $tb = Invoke-Pinned -Exe $ExeB -Arguments $ArgsB -CoreIndex $Core
    } else {
        $tb = Invoke-Pinned -Exe $ExeB -Arguments $ArgsB -CoreIndex $Core
        $ta = Invoke-Pinned -Exe $ExeA -Arguments $ArgsA -CoreIndex $Core
    }
    $a.Add($ta)
    $b.Add($tb)
    if ($tb -lt $ta) { $winsB++ } elseif ($tb -eq $ta) { $ties++ }
    $ratio = $tb / $ta
    Write-Host ("  pair {0,3}: A={1,9:f1} ms  B={2,9:f1} ms  B/A={3:f4}" -f ($i + 1), $ta, $tb, $ratio)
}

function Get-Median {
    param($xs)
    $s = $xs | Sort-Object
    $n = $s.Count
    if ($n % 2 -eq 1) { return $s[[int](($n - 1) / 2)] }
    return ($s[$n / 2 - 1] + $s[$n / 2]) / 2
}

$medA = Get-Median $a
$medB = Get-Median $b
$minA = ($a | Measure-Object -Minimum).Minimum
$minB = ($b | Measure-Object -Minimum).Minimum
$n = $Pairs
$z = ($winsB - $n / 2.0) / (0.5 * [math]::Sqrt($n))

Write-Host ""
Write-Host ("== {0} ==" -f $Label)
Write-Host ("METHOD: pinned core {0}, High priority, CPU time via cached handle, ABBA-interleaved, {1} pairs, {2} ties" -f $Core, $n, $ties)
Write-Host ("A: median {0,9:f1} ms   min {1,9:f1} ms" -f $medA, $minA)
Write-Host ("B: median {0,9:f1} ms   min {1,9:f1} ms" -f $medB, $minB)
Write-Host ("ratio B/A of medians: {0:f4}   of mins: {1:f4}" -f ($medB / $medA), ($minB / $minA))
Write-Host ("paired win rate B: {0}/{1}   z = {2:f2}   (abs(z) > 2 is a verdict)" -f $winsB, $n, $z)

if ($medA -lt 50 -or $medB -lt 50) {
    Write-Warning "arm under 50 ms CPU: below timer resolution, NOT admissible; lengthen the workload"
} elseif ($medA -lt 1000 -or $medB -lt 1000) {
    Write-Warning "arm under 1 s CPU: per-invocation overhead is a large fraction; prefer >= 15 s arms"
}
if ([math]::Abs($z) -le 2) {
    Write-Host "verdict: NOT RESOLVED at this N (abs(z) <= 2). Do not quote the ratio; raise N or use a counter."
} else {
    Write-Host "verdict: RESOLVED."
}
