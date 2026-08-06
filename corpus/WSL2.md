# Tier-A corpus under WSL2 (plan §7.1)

mimalloc-bench is Unix-only, so the 1:1 corpus runs under WSL2 on the dev box (dev
loop) or a quiet Linux box (numbers of record — WSL2 numbers are never quoted as
standing, risk R5).

## One-time setup

```powershell
wsl --install -d Ubuntu       # then reboot / open Ubuntu once
```

Inside Ubuntu:

```bash
sudo apt-get update && sudo apt-get install -y build-essential cmake ninja-build git \
    unzip dos2unix   # unzip + dos2unix: undocumented needs of the shbench patch step (2026-08-05)
cd /mnt/c/Users/talmo/coding/rusty_alloc          # or a native-FS clone (faster I/O)
git submodule update --init corpus/mimalloc-bench oracle/mimalloc
cd corpus/mimalloc-bench
./build-bench-env.sh bench mi                     # benchmarks + C mimalloc arm
```

> Prefer cloning to `~/rusty_alloc` inside WSL for corpus runs — /mnt/c file I/O
> skews gs/lean-style benchmarks. The repo is the same either way; provenance
> lines must say which filesystem the run used.

## Running

```bash
cd out/bench
../../bench.sh mi cfrac                            # M0 gate: oracle arm runs
../../bench.sh mi mi cfrac                         # null arm: same allocator twice = session floor
```

Every result row records: wall, CPU (user+sys), peak RSS, page faults — plus the
method line (machine, filesystem, arm order, N).

## The `ra` arm (M7)

At M7 `rusty_alloc_override` registers as allocator `ra` (plus `rad` = debug_checks,
`ras` = secure) in `build-bench-env.sh`, loaded via LD_PRELOAD exactly like `mi`.
Until then the corpus only exercises the oracle side.
