# Free-slot purge — return the touched free slots of a page that stays live

Status: MEASURED, not built; next in the owner's order (2026-10-31). The
purge-default flip it was ordered against has landed (`purge_delay` is a real
delay, default 10 ms; see the last section for why it could not be `0`).
Neither the flip nor `collect(true)` reaches the slots of a pinned page.

## The gap

Requests from 8 KiB to 64 KiB share 512 KiB medium pages (default geometry:
`MEDIUM_OBJ_SIZE_MAX = MEDIUM_PAGE_SIZE / 8`, so 8 slots of 64 KiB or 16 of
32 KiB). One long-lived block keeps its page from ever emptying, so the page
never retires and every free slot that was ever written stays resident. Span
purging cannot reach it: only freed spans are purged, and this page is never
freed.

## Probe (`pin-probe.c`, x86_64 glibc under WSL2, flag-off build of `f782f94`)

One thread, 50 rounds of N transient buffers written in full and freed, then
`mi_collect(true)`; growth in smaps Anonymous, KiB:

| case | purging off (default) | `purge_delay = 0` | glibc |
|---|---|---|---|
| nothing pinned, 7 × 64 KiB | +512 | +0 | +128 |
| one 64 KiB block pinned | +452 | +452 | +132 |
| nothing pinned, 15 × 32 KiB | +512 | +0 | +128 |
| one 32 KiB block pinned | +484 | +484 | +132 |

The first row also shows a separate effect: purging defaults to OFF
(`purge_delay = -1`, `options.rs`), so a default build returns nothing at all.

## Real case: rusty_WRT odhcpd (purge_delay = 0 throughout)

A single-threaded daemon allocating ~10 transient zeroed 32/64 KiB netlink
receive buffers across startup. Two whole-system boots per arm, anonymous PSS
KiB:

| build | odhcpd | system PSS | large requests |
|---|---|---|---|
| per-thread buffer reuse | 248–256 | 6940–6974 | 1 × 32 KiB, 1 × 64 KiB |
| fresh buffer per read | 432 | 7132–7180 | 3 × 32 KiB, 7–8 × 64 KiB |

Heap census `[allocs, frees, pages_fresh, pages_retired, segments,
segments_freed, large, huge]` is `[0,0,28,0,1,0,0,0]` in both: the page is
pinned under reuse too. Reuse wins only by touching one slot per class.

## Real-world gate

rusty_WRT keeps the churning behaviour behind `--cfg netlink_no_lend`
(byte-identical binary without it). Build against a rusty_alloc commit with
`[patch.crates-io]` pointing `rusty_alloc` and `rusty_alloc-api` at the
extracted source, then

    RUSTFLAGS="--cfg netlink_no_lend -C relocation-model=static" \
      cargo build --release --target x86_64-unknown-linux-musl -p wrt-multicall

**Pass:** odhcpd near 248 KiB with the flag on (432 today).

## Sketch, not decided

Reset the OS pages of a free block in a medium page, leaving the first 4 KiB
of the block (it holds the free-list link). When: on collect, or when a page's
used count falls below a fill level. Gated on `purge_delay >= 0`. Costs to
measure: one madvise per block reset, and the refaults when a reset slot is
reused. Windows needs the reset form that keeps the range committed
(`MEM_RESET`/`DiscardVirtualMemory`) or the recommit path for slots; the two
2026-10 Windows access violations (`recommit-failure-ignored.md`) say which is
riskier.

## Cost of turning span purging on (for the default question)

Same library, `purge_delay` unset vs 0: user-space Ir unchanged within noise
(lua +151 K on a 1.7 M noise, perl +4.4 K, sqlite +4.3 K); madvise calls 146 /
6 / 43; minor faults +768 (+14 %) / +147 / +10; peak RSS −896 KiB / −2.6 MiB /
0.

### The cliff the program runs missed (2026-10-31): purging is not delayed

`purge_delay` is sign-only, so `>= 0` purges a freed span of 512 KiB or more
the moment it is freed, and it also turns off the large-span cache
(`heap.rs`, `get(15) < 0`). A loop that allocates a buffer, touches every
page and frees it then refaults every page on every iteration. WSL2, 4000
iterations, `bigloop.c`, per iteration:

| buffer | glibc | purging off | `purge_delay = 0` |
|---|---|---|---|
| 256 KiB | 0.19 µs | 0.19 µs | 0.19 µs (under the purge threshold) |
| 1 MiB | 0.73 µs | 0.70 µs | 207 µs (1.02 M minor faults) |
| 2 MiB | 1.6 µs | 1.5 µs | 444 µs (2.05 M) |
| 8 MiB | 7.6 µs | 7.0 µs | 2,098 µs (8.2 M) |

So the default cannot flip to 0 as the option stands. Flipping it needs a real
delay: purge a freed span only after it has stayed free for `purge_delay` ms
(upstream's default is 10), so a span reused within the window is never
purged. That is the precondition for item 2 of the order.
