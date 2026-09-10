# rusty_alloc LEDGER

One entry per milestone/brick: what landed, the numbers with their method lines,
what was reverted and **which kind** of revert (measured-worse vs within-noise).
Newest first.

## LARGE-ALLOCATION CEILING — a 64 KiB block cost two segments; a geometry knob takes 1 block to 3 (2026-09-09)

`docs/plans/finished/esp32-large-alloc-ceiling.md`, reported from the
`rusty_zstd` bare-metal work: in a 256 KiB region `rusty_alloc` served ONE
64 KiB allocation and refused the second with 192 KiB free.

**Confirmed on the board, and their hypothesis was right.** A segment's slice 0
is its header, so `LARGE_OBJ_SIZE_MAX = SEGMENT_SIZE - SEGMENT_SLICE_SIZE` =
61,440; one byte over and `huge_alloc` reserves `4 KiB + size` on a
`SEGMENT_SIZE` stride, spanning two segments. Structural — no allocation of
`SEGMENT_SIZE` can share a segment with its own metadata. The sweep they asked
for puts the cliff at 61,440 exactly. Their 50 % estimate read 25 % on silicon
because of a cost they could not see: **the first small allocation claims a
whole segment** (`used=135168` = 69,632 + 65,536 for a `Vec` spine).

**Fix: `--cfg ra_segment_size="256k"`** (8 KiB slice x 32), which raises
`LARGEST_SHARED_ALLOC` to 253,952 so a 64 KiB request is a span three of which
pack into one segment. Same board, same 256 KiB region: **1 block -> 3**, 25 %
-> 75 % utilisation. Their kill test ("at least 3") passes, and "no region size
works on an S3" is no longer true. Opt-in: it doubles the page floor
`(classes touched) x slice` that a small-object workload pays.

Also shipped, because the report's second ask was diagnosability:
`LARGEST_SHARED_ALLOC`, `dedicated_segments(size)`, `region_for_allocs(size,
count)` (which counts the small-allocation segment), and
`region_capacity() -> (free_segments, largest_servable)` — on the failing call
`(1, 61440)` against 126,976 free bytes. The README's floor model said the cost
"amortises as the working set grows"; that is true for small objects and
inverts for segment-sized ones, and now says so.

**A rung built and withdrawn.** 4 KiB x 32 (128 KiB) buys a 64 KiB consumer
NOTHING — one 16-slice span in a 31-slice segment is still 128 KiB per block,
the default's cost by another route. The lever is
`LARGEST_SHARED_ALLOC / size`, not `SEGMENT_SIZE`. It also segfaulted 11/12
under the concurrent host battery where the default and 256k are 0/40 and 0/12;
real, geometry-specific, and NOT root-caused. Kept out of the shipped set and
written down in §9.5 in case it is latent rather than local.

**Gates.** 20 suites green at both shipped geometries; CI runs the whole suite
at 256k rather than building it; gate-selftest 11/11 (new: drop the header
slice from `dedicated_segments` and the sizing test goes red); the reproduction
is a permanent property-based test against the real extent allocator. Unsafe
+4, all `#[cfg(test)]` — the fix is arithmetic and adds none to shipped code.

## SMALL-PROFILE EXTEND — every page carved one block at a time; 100% slow path -> 12.5% (2026-09-10)

`docs/plans/finished/fixed-prim-small-step.md` §8.6-8.7, pulled out of the
Kairos step report rather than reported directly.

`page_extend` bounds its batch at 4 KiB of payload via
`span_shift = 4 + slice_count.trailing_zeros()`. The identity is
`4096/bsize == reserved / (slice_count * 16)` and the `16` is
`SEGMENT_SLICE_SIZE / 4096` -- so the literal `4` holds only at the 64 KiB
slice. Under `ra_small_profile` (4 KiB slice) the bound was **256 bytes**, the
batch for a 512 B class computed to 0, `.max(1)` clamped it to ONE, and every
page carried `capacity == 1`. A page with one block has no second block for the
fast path, which is why `generic` read exactly 1.0000/op at every binned size.

**Found by tracing `collect_inner`'s reclaim** while chasing the reported
512-byte step: every reclaimed page printed `cap=1` with `resv=8`, and reserved
being right while capacity was 1 named the extend immediately.

**Fixed** by deriving the term (`SEGMENT_SLICE_SIZE.trailing_zeros() - 12`).
Counted, 100k pairs, small profile: generic/op **1.0000 -> 0.1250** (512 B),
0.1667 (513), 0.2500 (1024); churn 195 -> 24/32/49 per 100k. Timing, 32-bit
host, ABBA, three reproductions: the 512-vs-513 step inverts +8.6% -> -36%,
and 512 absolute 390,200 -> ~200,000 ns, ~1.9x. Default geometry byte-identical
(the derived value IS the old literal; all-features asm moves one debug blob).

**Measured on silicon** (XIAO ESP32-S3, main vs fix, one board, one session,
identical floor 166 ns and identical checksums): pingpong 32 B 595 -> 518
(13.0%), batch 64-mixed 833 -> 702 (15.7%), churn 8-512 B 1,011 -> 856 (15.3%),
large 2,048 B 1,134 -> 1,121 (1.1%). 13-16% on every binned row, the same
magnitude as the reported step; 2,048 B barely moving is the tell, since it is
on the bin route this does not touch. Still open: the heap_4 A/B row is the
consumer's to re-run, and the bin route enters generic on every op even now.

## SMALL-PATH STEP — not the prim, the POINTER WIDTH; the heartbeat knob bare metal could not reach (2026-09-10)

`docs/plans/finished/fixed-prim-small-step.md`: the Kairos RTOS measured one
alloc+free stepping 314 -> 271 cycles/op across 512 bytes on a 32-bit ESP32-S3,
could not reproduce it on a 64-bit host, and concluded it "points at
`prim::fixed`".

**Re-attributed.** `SMALL_SIZE_MAX` is `128 * size_of::<usize>()` — 1,024 on
their host, **512 on the device**, where it coincides with
`SMALL_OBJ_SIZE_MAX`. Their refutation ("the 512 step is not `SMALL_SIZE_MAX`")
was run on a machine where that constant sits at 1,024. Moving ONE variable,
pointer width, with `prim::windows` in both arms (ABBA, 50k ops/arm, min of 40
blocks): x86-64 **+0.1 %** at 512 vs 513, i686 **+8.6 %**, control (256 vs 264)
under 1 % on both. The prim is exonerated; the run that settled it was
`--target i686-pc-windows-msvc`, outside the crate, not the fixed-prim-on-host
build the report asked us for.

**Mechanism, by counter not clock.** Per 100,000 pairs with one block live, the
`direct[]` route carves/extends/retires **195** pages and the bin route just
above it **0**, while `generic`/op is 1.0000 on both — so the slow path is not
the difference, page churn is. `100_000 / 512 = 195` is
`GENERIC_COLLECT_DEFAULT` at the small profile. The boundary is the ROUTE, not
the page kind: on 64-bit, 513–1024 are medium pages that still churn.

**Shipped:** `--cfg ra_generic_collect` (the trade was real and the knob was
unreachable — `options::set` is a no-op under `ONE_REGION`); and
`prim::fixed::shape_of` so the route and page kind are a `const` answer rather
than a silicon discovery. Default unchanged: the 512 exists because 10,000
starved this profile (168 -> 8 blocks, 22,533/50,000 nulls).

**Withdrawn rather than quoted:** how much of the 16 % the churn accounts for.
Raising the heartbeat took churn to a measured zero, but that experiment's
timing control flipped +8.0 % -> +0.6 % on re-run, so the instrument was
deciding it. The device is the right box; the counter half needs no quiet one.

**Gates:** 20 suites at both profiles, gate-selftest 11/11, wasm 20,168.

## REGION ALIGNMENT DISSOLVED — segments stride from the base; 24,144 B of stack back, +3 instructions per free, one knob (2026-09-09)

`docs/plans/finished/region-alignment-dissolve.md`: the Janus firmware's
proposal, after 2.0.4 — the 24,148-byte linker gap before the segment-aligned
`Region` is the address mask's price, wasm already dissolved the same
constraint with a slice table, and a fixed region has ONE base the backend
already holds, so mask the offset from it instead.

**What landed.** `crate::FIXED_REGION` (the prim is `prim::fixed`; the arena
layer folds on it) and `crate::REGION_STRIDES` (`FIXED_REGION` unless
`--cfg ra_aligned_region`). Under it `segment_of` is
`p - ((p - base) & (SEGMENT_SIZE - 1))`, the fixed backend measures every
alignment from the region's base (`place(.., origin)`, `install_region`
aligns the base up to `REGION_ALIGN` = 16 and trims), `usable_bytes` runs up
to 16 bytes instead of a segment, `Region<N>` is `repr(align(16))`,
`link_is_plausible` and `huge_alloc`'s slack and `malloc_aligned`'s
natural-fit bound follow the same predicate. Hosted builds: the x86-64 asm
diff after all edits differs in debug records and one static's symbol name
only — no instruction moved in any function a host links.

**Measured, rig (probe firmware, `Region<196_608>`):** `.stack` 110,240 →
**134,384 (+24,144)**, `.bss` and `.data` unchanged, `.data + .bss + .stack`
311,220 → 335,364 (the 24,148 unaccounted bytes of 2.0.4 are 4 now);
`xiao_s3_probe::HEAP` moved from `0x3fc90000` to `0x3fc8a1b0`. Flash +72 B
against 2.0.4 (`prim::alloc` +114 for the origin arithmetic, `insert_at` /
`remove_at` inlined away). Arm to arm against `esp-alloc` on the same rig:
flash +3,228 B, stack **+26,688 B** for the same 220 KiB budget (was +2,544
at 2.0.4: the 28,672 B `good_region_size` hands back is finally in
`.stack`). Under the knob: `.stack` 110,240 (= 2.0.4), flash +3,092.

**Measured, board (bench firmware, ns per alloc/free pair net of floor,
pingpong / batch / churn / 2048 B):** 2.0.4 586 / 824 / 1,002 / 1,133;
strides alone 603 / 845 / 1,023 / 1,142 (+17–21, 2–3 %); the knob reproduces
2.0.4 to the nanosecond. Free path in the shipped `dealloc`: 37
instructions at 2.0.4, 42 with strides — `l32r &REGION_BASE; l32i; sub;
l32r 0xffff; and; sub` where the mask was `l32r 0xffff0000; and`.

**The sibling finding, reading that disassembly.** Both arms carried
`l32i a9, a11, 0x390` followed by a `memw`, with `a9` overwritten two
instructions later: `(*seg).thread_id.load(Acquire)`, read BEFORE
`crate::ONE_THREAD || owner_tid == thread_id()` folded the compare away.
LLVM keeps an unused acquire load, so every free on every single-threaded
firmware paid a load and a barrier for nothing. The load is inside the
predicate now: 2.0.4-layout 578 / 820 / 997 / 1,125 (36 instructions),
strides **595 / 833 / 1,011 / 1,134** (39). The shipped default is within
9 ns of 2.0.4 on every row; the knob arm is 8 ns under it.

**The plan's own gate, answered honestly.** It said: if the instruction
count moves more than the wasm arm's table lookup did, keep the mask. wasm's
lookup replaced an `and` with a shift, an add and a load (about +2);
this is +3, one of them a data load — more, by one. That is why the trade
is routed through a gate rather than decided once: RAM binds on this part
(24 KiB of 512), so strides are the default; `--cfg ra_aligned_region` is
the mask for a firmware whose alloc/free rate binds instead, tested on both
geometries in CI. The two arms are the same tree, so the comparison is
within-binary-config, immune to drift.

**Gates.** clippy 0 on default / small / small+knob; `cargo test -p
rusty_alloc` green on both geometries; the fixed-backend tests carve the
region 0x1f0 past a 64 KiB line on purpose and assert a served segment is
NOT absolutely aligned; `tests/region.rs` is a plain `static Region<N>` now
(the firmware shape); gate-selftest 10/10 (new: `FIXED_REGION` forced true
on a host refuses arenas and the arena test goes red); wasm 20,169 B
unchanged; unsafe census re-baselined (test-only blocks for the knob arm;
UNSAFE.md row updated).

**Not taken, recorded:** the esp LLVM backend materialises `0xffff` through
the literal pool instead of an `extui` — one instruction of the three, not
worth a profile-specific cast in `segment_of`.

## REGION ALIGNMENT — the documented fix cost 60 KB of stack; whole segments, descriptor in a static (2026-09-09)

`docs/plans/finished/region-alignment-bug.md`: the Janus firmware copied the
`good_region_size` doc example into an alignment-1 container, the linker put
it at `0x3fc8a1e4`, the exact size served two segments instead of three, and
the board panicked 484 bytes short of the round number that had worked. Three
fixes proposed (fix the example, make `init_region` report, ship an aligned
container) and a test.

**The sibling finding that changed the fix.** Before writing fix 3 the
consumer's own workaround — `#[repr(align(65536))]` around 200,704 — was
measured on the rig: `xiao_s3_probe::HEAP` 225,281 → **262,144** (a type's
size is rounded up to its alignment), `.bss` +36,860, `.stack` **−60,952**.
The recommended fix was the most expensive of the three configurations. Root
cause: every sizing rule carried `+ FIXED_PAGE` for the first heap's
descriptor, so an exact region was never whole segments and an aligned
container of it was always padded.

**Fix, measured.** The first heap's descriptor on a one-region target is a
1,752 B static of the fixed backend's; `MIN_REGION` / `usable_bytes` /
`good_region_size` / `region_for` lose the page; `prim::fixed::Region<N>`
(aligned, `N % SEGMENT_SIZE == 0`, `size_of == N`, `give` once → usable
bytes); `init_region` refuses a base that costs a segment (`FERR_MISALIGNED`).

| probe region declaration | `.bss` | `.stack` | usable |
|---|---:|---:|---:|
| round 225,280, align 1 (2.0.2/2.0.3 shape) | 225,672 | 107,412 | 196,608 |
| consumer's `#[repr(align)]` 200,704 (the workaround) | 262,532 | 46,460 | 196,608 |
| **`Region<196_608>`** | **198,752** | **110,240** | 196,608 |

Board, footprint sketch on `Region<{ 64 * 1024 }>`: `region given: 65536
usable of 65536`, peak 4,914, ran to `[heap] end` — the floor is one segment
plus the static (was 68 KiB). The rig's `size -A` does not count the linker's
gap before an aligned static; `.stack` is the truth, and against the round
unaligned region the gain is 2,828 there, not 26,920.

**Two defects the tests caught in the fix itself.** (1) The first `Region`
had its once-flag as a field: one byte beside a 64 KiB-aligned array rounds
the type to the next segment — `Region<196_608>` was 262,144 — the exact
defect being fixed; the flag is a module static now, and a `const` assertion
pins `size_of == N`. (2) `Box::new(Region::new())` in a host test faulted
(STATUS_ACCESS_VIOLATION): the 64 KiB-aligned value is materialised on the
stack past the guard page on Windows; `Box::new_zeroed().assume_init()`
allocates in place and zero is a valid `Region`.

**Gates.** fmt; clippy host (default, small profile) and riscv32 no_std at
both geometries; full suite default + small profile (20 binaries each,
`tests/region.rs` new — its own process so `give` can succeed); wasm flat;
census 898 → 903 recorded in UNSAFE.md (two `Sync` impls, the handoff, two
test sites); gate-selftest 9/9 (the misalignment refusal removed goes red).

**Semver note.** `good_region_size` / `region_for` / `usable_bytes` /
`MIN_REGION` return different values; a consumer asserting the old literal
fails to build. Shipped as a fix (the old values were the defect); the strict
reading is a minor.

**Consumer verification of 2.0.4, and a correction to how the saving is
quoted (later the same day).** `used=196608 free=0` on the board, the seam's
own container gone, the `const` assert caught the shape change by failing to
build. And the consumer reconciled the RAM map: `.data + .bss + .stack` is
335,368 on both unaligned builds and 311,220 on the aligned 2.0.4 build —
**24,148 bytes in no section**, the linker's gap before the aligned static.
So against the round unaligned region the gain is the `.stack` number,
+2,828, not the `.bss` number, −26,920; the granule's 24,576 moved from
inside the region to the gap before it. Method rule adopted: on a fixed RAM
map, report `.stack` or the section sum, never `.bss` alone; the rig's diff
now prints the unaccounted remainder. The `Region` docs, the README recipe
and the CHANGELOG entry say it. One more placement datum: the 4 KiB shift of
every buffer when the descriptor page went moved a compute kernel **20 %**
(`yuyv_to_gray8`, 125,218 → 150,247 per unit) with no allocator call inside
it — the README's placement caution now carries that number.

## FIRMWARE, WHAT IS LEFT — one region, four folds, flash +7,860 → +3,208 B (2026-09-09)

The consumer's third report (`docs/plans/finished/firmware-what-is-left.md`)
priced the region granule at 24,576 B, three static tables at ~688 B, asked
whether `collect_inner` was reachable orphan code, and offered to build a
churn benchmark because "nobody has timed this on a chip". All four answered;
two more levers found reading the siblings of the three tables.

**Rig.** As the previous entry (scratch copy of `xiao-s3-probe`,
`[patch.crates-io]` to the working tree, `size -A` + `nm -S` on the linked
ELF; baseline = 2.0.2 as published, reproduced to the byte). Board: the
espino `blink-fs-p4` harness under `--cfg ra_bench`, both arms, before and
after (`board=xiao-esp32s3 clock=240MHz region=192KiB-both floor=158/162ns
best-of-5 checksums-matched null-arm=1ns`).

| brick | `.text` | flash | RAM (`.bss`+`.data`) | what |
|---|---:|---:|---:|---|
| `good_region_size` / `region_for` | 0 | 0 | 0 (24,576 B of region, consumer's line) | `const fn`s; tested against `usable_bytes` at both geometries |
| `ONE_REGION`: options + arenas + segment map | −2,808 | −3,500 | −960 | `VALUES` 304, `ARENAS` 128, `range_table` 512 gone; `malloc_generic_once` 1,950 → 542; `collect_inner` 1,988 → 1,381; `create_heap` 966 → 608 |
| sentinels to flash, template copied from there | −248 | −1,152 | −1,808 | `.data` −1,808, `.rodata` +904 (the second template blob gone) |
| `ra_max_extents` knob | 0 default; +24 at 8 | 0 / +24 | 0 / −192 | opt-in |
| **total (default knob)** | **−3,056** | **−4,652** | **−2,768** | attributed 8,262 → 4,313 B, 36 → 27 symbols |

Arm to arm now: flash **+3,208** (2.0.1: +16,584), static RAM **+284**
(2.0.1: +3,092), `.data` 16 bytes LESS than esp-alloc's. `.stack` identity
held on every brick.

**`collect_inner` (§2): closed.** Orphans need `abandoned_push`, whose only
caller is `thread_done`, which nothing on a bare-metal image calls;
`heap_delete` migrates through `adopt_segment` directly and `heap_destroy`
frees; since 2.0.2 `abandoned_pop` folds to null and no `abandoned_*` symbol
is in the image. The 1,988 B was the page sweep with the four freeing fns
inlined — the P4e reclamation mechanism, not a lever — and it still shrank to
1,381 because its segment-map `unregister` and arena `chunk_free` probes
folded.

**§5's premise was wrong:** the README's 2.0-3.7x rows were measured on this
board (espino harness, small profile, floor-subtracted, checksummed) — not on
a host. What was true is that they were stale: 2.0.2 already read 587 / 828 /
1,007 / 1,129 against the README's 647 / 881 / 1,087 / 1,200 (the free fold),
and this branch reads 586 / 824 / 1,002 / 1,133 — neutral to within the 1 ns
null arm, as it must be for pruning that runs on segment allocation only.
README refreshed: 2.80x / 2.17x / 3.98x / 1.45x, with the placement caution.

**Two things the self-test taught.** (1) A first `ONE_REGION` mutation
targeted the exclusive-arena test and read VACUOUS: that test skips, honestly,
when a reservation fails, so a predicate that makes reservations fail cannot
turn it red. Repointed at the broad heaps test, which `expect`s its arena and
asserts `options::set` round-trips. (2) The `good_region_size` test hardcoded
small-profile answers twice; at the shipped 32 MiB segment a 220 KiB budget is
below the floor and the right answer is 0. Now geometry-aware.

**Gates.** fmt; clippy host all-targets and `riscv32imac` no_std (with and
without the knob); full suite default + small profile; fixed tests at both
geometries; wasm ratchet flat (20,169); gate-selftest 8/8.

## FIRMWARE CODE SIZE — three levers, flash cost halved, wasm −10.7 % (2026-09-09)

`docs/plans/finished/firmware-code-size.md` decomposed what the allocator adds
to an `esp-hal` firmware (+16,584 B flash, +3,092 B static RAM) and ranked
three levers. All three taken, each its own brick, each measured on the linked
ELF before the next was written.

**Rig.** `xiao-s3-probe` (Janus `rusty_esp_dsp/firmware/`) copied to a scratch
directory with `[patch.crates-io]` pointing `rusty_alloc` and `rusty_alloc-api`
at the working tree and the git seam patched to its local checkout, so the two
arms of every A/B differ only in this branch. Method line:
`board=xiao-esp32s3 target=xtensa-esp32s3-none-elf toolchain=esp opt-level=3
lto=fat cgu=1 panic=abort cfg=ra_single_threaded+ra_small_profile
metric=size-A+nm-S-on-linked-ELF arms=one-source-two-features`. The baseline
reproduced the plan to the byte on `.text/.data/.bss/.stack` and the
16,256 B / 57-symbol attribution; `.rodata` read 312 B lower because a path
checkout's panic-location strings are ~45 B shorter than the registry's, seven
files' worth.

| brick | `.text` | flash | attributed | what left |
|---|---:|---:|---:|---|
| 1 `ONE_THREAD` (lever 1) | −3,208 | −3,328 | −3,152 | `adopt_segment` 1,154, `drain_delayed` 995 — the plan's 2,149 exactly — plus 325 off `malloc_generic_once`; four free-path fns inlined into `collect_inner`, net −649 |
| 2 `GUARD_PAGES`/`RNG_USED` (lever 3) | −4,828 | −4,924 | −4,683 | `try_guarded` 1,641 + `Random::refill` 725 — the plan's 2,366 exactly — and `init_thread_heap` 2,740 → 1,119: the seeding was 1,621 of it |
| 3 no exit hook on one context (lever 2) | −160 | −160 | −159 | `done_slot`: a TLS slot, its CAS and spin |
| **total** | **−8,196** | **−8,412** | **−7,994** | 16,256 → 8,262 |

Arm to arm now: flash **+7,860** (was +16,584), static RAM **+3,052** and
`.stack` **−3,052** — the identity held on every brick. wasm ratchet 22,574 →
**20,169** gzipped, banked.

**Lever 2 answered by subtraction, not projection:** of `init_thread_heap`'s
2,740 bytes, 1,621 was seeding a CSPRNG nothing on the target reads, 153 was the
thread-exit hook, 966 is creating a heap. The plan's suspicion (per-thread
generality) was 6 % of it.

**Two residues named.** `__ra_empty_heap_box` is 1,752 B of `.data` — flash
and RAM both — and is the plan's "unattributed 1,371" that the `rusty_alloc`
substring census could not see; left in place because removing it is a
fast-path branch decision that wants churn numbers, not a size table. And the
`esp-alloc` arm carries ~2.4 KB of `Debug` formatting to print its own stats,
so the arm-to-arm `.text` delta (+3,884) understates the allocator's code
(8,262) by that much; both are reported.

**Gates.** fmt; clippy host (default, small profile, all targets) and
`riscv32imac` `no_std` at both geometries; full suite on default/`secure`/small
profile; `rusty_alloc-api` `no_std`; wasm ratchet; `tools/gate-selftest.sh`
with a sixth mutation (`ONE_THREAD` forced true on the host must turn the
subproc test red — it does, through `abandoned_push`'s `unreachable!`).

**What CI on the PR turned up (2026-09-09).** Three things, none of them the
size work. (1) `main`'s `embedded` job was red on a `prim::fixed` test that
claimed a `SEGMENT_SIZE`-aligned page cannot come out of a 512 KiB region —
true only when the region does not straddle a boundary, a 1-in-64 ASLR roll;
made two-sided. (2) `stress_mt` has been flaky on `main` since 2026-08-24
(the run history: mostly red, occasionally green, both OSes). It is NOT
Windows-only and NOT "passes locally": with `--all-features`, the CI
configuration, it failed 1 of 40 local runs on a 24-core box, and on the
two-vCPU runner roughly two runs in three. Two failure modes seen: a silent
`abort()` (the double-free or corrupt-link detector) and
`abandoned block corrupted: left 118, right 119` — a live, abandoned block's
first byte read 0x76 where its dead owner wrote 0x77. A bounded bisection
(40 runs per single feature) found nothing at 0/40 for default, `secure`,
`blockmap`, `debug_checks`, `secure,blockmap`, `secure,linkcheck`; the race
needs the full combination or more runs, and is left as the standing open
defect. (3) The same bisection read `linkcheck` alone as 40/40 failures,
which was one rustc error: that feature without `secure` had not compiled
since the page-extent narrowing was dropped. Fixed, with a per-feature clippy
step in CI. `supply-chain` is red on `cargo vet` for `portable-atomic 1.15.0`
(no imported audit covers it) and is left for the owner: a 30k-line audit, a
publisher-trust entry, or dropping the dependency are all decisions.

**A process note.** The first clippy run failed on `assertions_on_constants`:
`debug_assert!(!ONE_THREAD)` is an assertion on a constant. Rewritten as
`if ONE_THREAD { unreachable!() }`, which is also louder in release. And the
first edit script normalised `init.rs` to LF — a 2,148-line diff for a
20-line change; the CRLF rule from the corpus work applies to this repo's own
files too.

## SMALL-METAL P5 — the six production blockers, closed (2026-09-07)

"Is this commercial ready?" produced six blockers against the post-P4e state.
This is what closing them cost and what it changed.

### 1. CI gated none of the embedded work

`ci.yml` built wasm but never set `ra_small_profile`, never passed
`--no-default-features`, and never targeted a chip. Every defect P0-P4e found —
five in shipped code — would have passed it. New `embedded` job: the
small-profile test suite, clippy on the small profile AND `no_std`, and builds
of BOTH bare-metal RISC-V targets at BOTH geometries, plus `rusty_alloc-api`.
Xtensa stays out (not a stock rustup target); the board runs are evidence, not
a gate. Every command in the job was run locally first.

### 2. The release state was incoherent

`Cargo.toml` 1.1.5, README "1.1.4", tags stopping at v1.1.4, and a commit titled
`chore: release v2.0.0`. Cause: release-plz titled the PR after
`rusty_alloc_api`'s major while the workspace went to 1.1.5. README now says
what is released and that `main` is ahead; CHANGELOG `[Unreleased]` documents
every fix and addition from this campaign, so the next release notes are true
rather than generated from nothing.

### 3. The published instruction counts were stale

Measured at v1.1.5, predating every reclamation change. `bench/icount-arms.sh`
already produces every column and nothing re-ran it. Scheduled `icount` CI job
added (valgrind + oracle + override shim, uploads the table); README carries
provenance and says the ratios are a FLOOR until re-run.

### 4. Vacuous tests

Four tests this campaign passed under the exact bug they guarded.
`tools/gate-selftest.sh` reintroduces five real defects and requires the suite to
go red for each — placement, collect-reclaims-last-page, the periodic collect,
the reclaim-before-null, and split64's ordering normalisation. It refuses to
count a mutation that did not apply or did not compile, so a moved anchor is a
failure rather than a silent pass. Wired into CI beside `semgrep-selftest.sh`.
All 5 fire; the script leaves every file byte-identical (verified with `cmp`).

### 5. `retire_expire` — replaced with a measured sweep period

Upstream ages a retired page out after ~16 generic trips. Implementing it needs
a retired-bin range on the heap, and `alloc::retire_or_abort` is deliberately
written to decide keep-one-warm from the page's own links so it never resolves
the heap — a measured optimisation this machine cannot re-profile. Since
`collect` now reclaims a bin's last page, the sweep period buys the same ageing.
Swept on the board:

| `generic_collect` | churn NULLs / 50,000 | ping | batch | churn | large |
|---:|---:|---:|---:|---:|---:|
| 10,000 (upstream) | 575 | 639 | 863 | 1,042 | 1,367 |
| **512 (shipped)** | **357** | 640 | 871 | 1,069 | 1,380 |
| 64 | 334 | 652 | 867 | **1,168** | 1,473 |

64 costs 12 % of churn throughput for 23 fewer failures; 512 costs ~1 % for 218.
Default is now geometry-aware: 10,000 shipped, 512 small profile. Upstream's
per-page countdown stays unimplemented, recorded in §6.

### 6. The `no_std` single-thread footgun

`SingleThreadCell`'s `unsafe impl Sync`, `prim::fixed`'s constant thread id and
spin lock, and `options`' split 64-bit atomics are sound only with one thread,
and all three fail quietly. `no_std` now REFUSES to compile without
`--cfg ra_single_threaded`; CI asserts the negative case.

### A defect this pass created, and the tell that caught it

Python's text-mode write emits `\r\n` on Windows, so every file rewritten by a
helper script flipped LF -> CRLF. Real content diff: 85 lines in `heap.rs`.
What git showed: 4,043. **`git diff -w` disagreeing with `git diff` by two
orders of magnitude is the signature.** Normalised back to LF across 23 files;
two files left CRLF because they are CRLF at HEAD.

### Final numbers

| workload | esp-alloc | rusty_alloc | speedup |
|---|---:|---:|---:|
| 32 B alloc/free | 1,638 | 640 | **2.56x** |
| 64 mixed, batched | 1,792 | 871 | **2.06x** |
| churn 64 live | 3,987 | 1,069 | **3.73x** |
| 2048 B | 1,638 | 1,380 | 1.19x |

Stress: capacity flat at 240 across the whole battery, 357 NULLs per 50,000
churn allocations (from 22,533 before P4d). The two remaining refusals are the
documented structural floor.

### Verification

106 tests / 33 suites / 0 failed (default); 89 / 19 / 0 (`ra_small_profile`);
clippy `-D warnings` clean on default, small profile and `no_std`; fmt clean;
unsafe census RATCHET OK; gate selftest 5/5 fire; both RISC-V targets at both
geometries and wasm32 build; board kill test green at the 68 KiB floor.

## SMALL-METAL P4e — reclamation fixed: churn NULLs 22,533 -> 575, capacity ratchet gone (2026-09-07)

P4d found the capacity ratchet and fixed one third of it. P4e closes the rest and
re-runs the whole esp-alloc comparison, stress and speed, on the XIAO ESP32-S3.

### Three changes

1. **Keep-one exemption removed from `collect` entirely.** P4d gated it on
   `force`; that was a partial port. Upstream's `mi_heap_page_collect` frees an
   all-free page at EVERY collect level ("this will free retired pages as well")
   — the keep-one cache is `mi_page_retire`'s, on the free path.
2. **`generic_collect` wired** — declared with a default of 10,000, read by
   nothing. Now a per-heap countdown in the generic path, as upstream.
3. **`malloc_generic` reclaims once before returning null.** The one that
   mattered: the battery makes ~649 generic trips total, so a 10,000 threshold
   never fires. An allocator must not report OOM while holding empty pages for
   classes nobody asked for. Free on the happy path — it runs only on failure.

### Stress, head to head at 192 KiB

| test | esp-alloc | rusty BEFORE | rusty AFTER |
|---|---|---|---|
| boundaries / realloc chain / zalloc-over-dirty | PASS | FAIL | **PASS** |
| fragmentation / exhaust-and-recover | PASS | PASS | PASS |
| distinct classes held at once | 24 | 9 | **21** |
| NULLs in 50,000 churn allocations | 0 | 22,533 | **575** |
| 512 B capacity across the battery | flat 383 | **168 -> 8** | **flat 240** |

Capacity no longer decays at all. The two remaining refusals are the documented
structural floor: a `SEGMENT_SIZE`-aligned request needs a whole free 64 KiB
segment, and 21-of-24 classes is what 30 slices hold once classes above 512 B
cost four slices each.

### Speed, and the price

| workload | esp-alloc | rusty BEFORE | rusty AFTER | speedup |
|---|---:|---:|---:|---:|
| 32 B alloc/free | 1,638 | 625 | 639 | **2.56x** |
| 64 mixed, batched | 1,792 | 844 | 863 | **2.08x** |
| churn 64 live | 3,987 | 1,012 | 1,042 | **3.83x** |
| 2048 B | 1,638 | 1,267 | 1,367 | 1.20x |

**2.2-7.9 % of throughput**, for an allocator that no longer fails while holding
reclaimable memory. esp-alloc reproduced to the nanosecond across sessions (same
162 ns floor, same checksums), so the deltas are ours and not drift. README and
crate README updated — the previously published 2.62x/2.12x/3.94x/1.29x are no
longer what the code does.

### The host test that asserted nothing, twice

The reclaim-and-retry regression test passed WITH THE FIX REMOVED, in two
versions running: (1) a 4-chunk arena is 128 MiB at the shipped geometry, where
48 cached pages starve nothing — regated to `ra_small_profile`; (2) still
passed, because the threshold was `served > 64` and the poisoned arm serves 128
— both arms above it. Fixed by measuring both arms and putting the threshold
between them: 240 with, 128 without, assert `> 192`. A threshold chosen before
the arms are known is a guess.

### Not implemented, and now on the list

`mi_page_retire`'s `retire_expire` countdown does not exist here, which is why
cached pages accumulate instead of ageing out. Also: the README's callgrind
instruction counts predate these changes and need re-running under `LD_PRELOAD`
before the next release.

### Verification

106 tests / 33 suites / 0 failed (default); 88 / 19 / 0 (`ra_small_profile`);
three new tests, each poisoned to confirm it fires.

## SMALL-METAL P4d — stress battery finds a collect defect: capacity decayed 168 -> 8 (2026-09-07)

Every phase so far measured a workload that works. P4d ran eight adversarial
tests on the board — routing boundaries, alignment to a whole segment, a realloc
chain, zalloc over dirtied pages, a fragmentation adversary, exhaustion and
recovery, a size-class sweep, and 50,000 churn ops — with every allocation
null-checked so one failure does not end the run.

### The defect

`heap.rs::collect_inner` discarded `force` and applied the keep-one-page-per-bin
exemption on every path, so `mi_collect(true)` could never reclaim a bin's last
all-free page. Upstream's `mi_heap_page_collect` frees unconditionally at
`MI_FORCE`; the keep-one cache belongs to `mi_page_retire`. Invisible at 512
slices per segment, fatal at the small profile's 16:

```
512 B capacity as the battery touched classes: 168 -> 104 -> 72 -> 56 -> 24 -> 8
collect(true):  8 before, 8 after   <- recovered nothing
churn:          22,533 of 50,000 allocations NULL, with 61,440 bytes free
```

Fix is `force || !only_page_in_bin`. Measured on hardware:

| | before | after |
|---|---:|---:|
| `collect(true)` capacity recovery, 192 KiB | 8 -> 8 | **8 -> 240** |
| `collect(true)` capacity recovery, 68 KiB | — | **8 -> 120** |
| NULLs in 50,000 churn ops, 192 KiB | 22,533 | **2,925** |
| segments ever created (i.e. releasable) | 2 | **3** |

Segments could not be released at all before: a single retained page pinned each
one. Regression test `heaps::forced_collect_reclaims_a_bins_last_page` asserts
both halves (unforced keeps the cache, forced reclaims it); poisoned it reports
`retired 0 -> 0`.

### The gap it exposed, deliberately not closed

**`generic_collect` is declared with a default of 10,000 and never read.** The
only collect callers are the two public entry points and teardown, so there is
no automatic collect and the capacity a forced collect now recovers is never
recovered on its own — which is why churn still returns NULL 2,925 times at
192 KiB and 24,853 at 68 KiB from a heap one `collect(true)` restores. Wiring it
moves behaviour on every platform including the published instruction counts, so
it belongs to the callgrind harness, not a board. Now item 1 of the plan's §6.

### Three structural limits, documented

- **A whole-segment request needs a whole free segment.** 61,439 / 61,440 /
  61,441 / 65,536 B and `align = 65,536` all return NULL at 192 KiB with 61,440
  free, because that free space is not a contiguous aligned 64 KiB. Size an
  embedded region as `k * 64 KiB + 4 KiB`.
- **The `bins x page` floor, dynamically.** 21 of 24 classes in 2 segments — 30
  usable slices, small classes 1 slice, classes above 512 B cost 4. It stops
  exactly where §2.9's arithmetic says.
- **68 KiB is workload-specific.** At that budget the battery holds 5 of 24
  classes. §2.10's floor is the least that runs the fs sketch and nothing more;
  the README now says so.

### What did not break, and the control

With reclamation between tests, `realloc_chain`, `zalloc_dirty`,
`fragmentation` and `exhaust_recover` all PASS and capacity holds flat — prefix
preservation across every routing boundary, re-zeroing of recycled dirty pages,
2 KiB requests over a holed heap and full restoration after exhaustion are
sound. Every failure was capacity, never correctness.

**esp-alloc control: every test PASS, capacity flat at 383, no decay, no churn
NULLs.** A linked-list heap has no per-class cache to starve on.

### The battery's own double free

The first run aborted in `page::double_free_abort` — a shared slot array the
harness never cleared, so a later test freed stale addresses. The allocator was
right and the harness was wrong, and the README's double-free claim is now
demonstrated on silicon.

### Verification

105 tests / 33 suites / 0 failed (default); 87 / 19 / 0 (`ra_small_profile`) —
both up one for the new regression test.

## SMALL-METAL P4c — 2.1-3.9x faster than esp-alloc on silicon, at 8.5x the RAM (2026-09-07)

Footprint was measured to death in P4b and esp-alloc wins it structurally.
Throughput is the half rusty_alloc is actually built for and it was
**unmeasured**, which made every claim about it an opinion. Measured now, on the
same XIAO ESP32-S3 Sense at 240 MHz, same one-source-two-arms harness, both arms
given the SAME 192 KiB.

Nanoseconds per allocate/free pair, best of 5, NET of the measured harness floor:

| workload | esp-alloc | rusty_alloc | speedup |
|---|---:|---:|---:|
| harness floor (no allocator call) | 162 | 162 | — |
| 32 B alloc/free | 1,638 | **625** | **2.62x** |
| 64 mixed blocks (8-512 B), batched | 1,792 | **844** | **2.12x** |
| **churn: 64 live, random 8-512 B** | 3,987 | **1,012** | **3.94x** |
| 2048 B alloc/free | 1,638 | **1,267** | 1.29x |

Churn is the row that matters — the shape real code has, and the one that
fragments a first-fit list. 2048 B is narrowest because 2 KiB is exactly
`MEDIUM_OBJ_SIZE_MAX` here, so it takes a medium page rather than the small
fast path.

### The guards

- **The harness measures itself.** A baseline arm — identical loop, identical
  non-inlined `touch`, identical four volatile accesses, no allocator call —
  cost **162 ns/op in BOTH arms** and is subtracted from every row. It is not
  cosmetic: unsubtracted, churn reads 3.53x instead of 3.94x, because a constant
  added to both arms drags any ratio toward 1.
- **The optimiser cannot delete the work.** Volatile write/read per block folded
  into a printed checksum. Without it an alloc/free pair is dead code and the
  benchmark times an empty loop.
- **Work parity proven, not assumed.** Every checksum matches across arms
  (25474400, 13944320, 9029440, 25474400); sizes come from a seeded xorshift32.
- **Null arm.** The same benchmark twice inside one arm reproduced to the
  nanosecond in both arms (625/625, 1638/1638); spread <= 1 % on every row.

### The first run was wrong and the number said so

It reported 2,361 ns/op for a 32 B alloc/free pair — ~570 cycles for a path that
should be tens. Cause: `esp_hal::Config::default()` leaves the S3 at 80 MHz.
Pinning `CpuClock::max()` moved every row by almost exactly 3x, which is what
confirmed the clock rather than the allocator had been under measurement.

### The cost, now measured instead of estimated

§2.9 estimated esp-alloc's floor at ~5.4 KiB from arithmetic. Measured by
shrinking its heap until it fails: **esp-alloc runs the same fs workload in
8 KiB**, versus rusty_alloc's 68 KiB. The honest headline is therefore
**2.1-3.9x faster at 8.5x the RAM**, and both halves went into the README and
the crate README — a speed claim published without its cost is one nobody should
believe.

### Validation re-run after the harness changed

Footprint build re-flashed and green: 68 KiB region, `free 0` at peak,
`PEAK 4,914`, app 127,088 bytes, kill test seen. No crate source changed in this
phase — the benchmark lives entirely in the Janus harness.

## SMALL-METAL P4b(iii) — 3.8 % page occupancy, and why 4 KiB is the floor (2026-09-07)

Two questions left after 68 KiB: is the 4 KiB slice *right* or merely where we
stopped, and is anything else cheap. Both answered on the same XIAO ESP32-S3,
kill test green throughout, region still 68 KiB with `free 0` and `PEAK 4,914`.

### The census that settles the geometry

Request counts cannot say how full a page gets. A page serves one size class, so
what matters is blocks live AT ONCE — a live/peak pair per bin:

```
block   4 B: PEAK live 1     block  64 B: PEAK live 1
block   8 B: PEAK live 2     block  96 B: PEAK live 1
block  16 B: PEAK live 4     block 320 B: PEAK live 1
block  24 B: PEAK live 2     block 384 B: PEAK live 2
block  32 B: PEAK live 1
```

**Nine pages, 36,864 bytes, holding 1,412 bytes at peak — 3.8 % occupancy.** No
class ever holds more than four blocks. (The other 3,502 bytes of the peak are
the 4 KiB allocations, which are large spans sized to the block and not part of
this.)

**4 KiB is a wall on both sides, and both walls were found by probing past
them.** Below: `good_size` rounds to an OS page while the large path allocates
slices, so `usable >= good_size` needs `slice >= page` (P4b(ii)). Above—
strictly, below in slice size—the two largest small classes are 320 B and 384 B
and `SMALL_OBJ_SIZE_MAX = SLICE / 8`. At 4 KiB that ceiling is 512 B and both
sit in one-slice small pages, 8 KiB for the pair. At 2 KiB the ceiling is 256 B,
both become MEDIUM, and `MEDIUM_PAGE_SIZE` cannot follow the slice down because
`spans.rs` pins `MEDIUM_OBJ_SIZE_MAX` at 2 KiB — which pins a medium page at
16 KiB. The pair would cost **32 KiB instead of 8**. Arithmetic from the measured
profile, labelled as such.

### The lever left on the table, with its number

Coarsening the small bins to power-of-two classes would collapse nine pages to
three or four, fit a 32 KiB segment, and take the region to roughly **36 KiB**.
**Not taken**, deliberately:

1. `bins.rs` says at the top of the file that the size -> `good_size` mapping IS
   the ABI-visible contract, G2-pinned against the oracle. Every existing
   small-profile divergence changes routing; none changes that mapping. This
   would be the first — a decision, not an optimisation to slip in.
2. It trades a BOUNDED cost for an UNBOUNDED one: page cost is per class
   touched, internal fragmentation is per live object. This workload holds 10
   tiny objects so it looks free — on a sample of one. Thousands of 24-byte
   nodes would pay up to 2x each.

### `portable-atomic` off the no_std path

`options.rs` was the last 64-bit-atomic user on a 32-bit target (`VALUES`, an
`i64` API frozen at v2.0.0; `HEARTBEAT`, a C-ABI `u64`). Its lock-based fallback
provides atomicity nothing can observe — the crate already serves `no_std` only
on single-threaded targets, which `SingleThreadCell`, `prim::fixed`'s constant
thread id and its never-contended spin lock all rest on. Replaced by `split64`,
two `AtomicU32` halves, **adding no unsafe** (a struct of `AtomicU32` is `Sync`).
With `std` on a 32-bit target the shim stays: there, threads are real.

| | .text | `.bss` symbols | `.bss` section |
|---|---:|---:|---:|
| `portable-atomic` | 84,907 | 136,423 | 201,996 |
| `split64` | 84,587 | 132,135 | 201,996 |
| | **−320** | **−4,288** | **0** |

**The predicted 4,288-byte SRAM saving did not happen, and the section table is
what said so.** `LOCKS` is the only differing symbol and it does leave, but
`.bss` does not shrink — esp-hal's linker anchors its end, so the space becomes
slack the application cannot claim. Kept for the dependency removal and 320
bytes of flash, not for RAM. Recorded because the first A/B I ran measured
*nothing*: a Python revert silently no-op'd on an MSYS `/f/...` path and both
arms built identically — the equal numbers were the tell.

### Two defects in the shim, both caught before shipping

- **Forwarding the caller's `Ordering` aborts the firmware.** `AtomicU32::load`
  rejects `Release`/`AcqRel`, and `options::set_default` performs
  `compare_exchange(.., AcqRel, ..)`. Orderings are normalised (loads `Acquire`,
  stores `Release`); the test passes exactly the orderings `options.rs` uses and,
  poisoned, panics in `core`'s `atomic.rs`.
- **The test could never run.** It sat inside a module `cfg`-gated to the target
  that needs it, so it would not have executed anywhere anyone runs tests. The
  module now also compiles under `test` — which is the only reason the ordering
  bug was found.

### Verification

104 tests / 33 suites / 0 failed (default); 86 / 19 / 0 (`ra_small_profile`) —
both up one, the shim's test runs in each; clippy `-D warnings` clean on default,
small profile and `no_std`; fmt clean; census RATCHET OK (unchanged at 890 — the
shim adds none); riscv32imac + riscv32imafc `no_std` at both geometries, and
wasm32. Board: 68 KiB, `free 0`, `PEAK 4,914`, app 127,056 bytes, kill test green.

## SMALL-METAL P4b(ii) — the levers, hammered: 192 KiB -> 68 KiB on the board (2026-09-07)

§2.9 ranked the footprint levers. This entry is what taking them cost and
bought, each step measured on the same XIAO ESP32-S3 Sense with the same
`espino run --expect` kill test, still green at every step.

| | region required | `PEAK` live | app image |
|---|---:|---:|---:|
| P4, as measured | 192 KiB | 4,914 | 127,328 |
| + lever 1 (two-ended placement) | **132 KiB** | 4,914 | — |
| + lever 2 (4 KiB slice) | **68 KiB** | 4,914 | 127,376 |

**−64.6 % of the region. `PEAK` identical at every step**, which is the
work-parity check: the allocator stayed the only variable. +48 bytes of flash
across both, so this came out of geometry and not out of deleted code. Board
counters: `pages_fresh` 10 -> 9, `segments` **2 -> 1**, `used` 135,168 -> 69,632
(= one 64 KiB segment + one 4 KiB heap block, `free 0` at peak — the exact
floor, confirmed the way P4 confirmed 192 KiB).

### Lever 1 — a placement bug, not a leak

The 61,440 stranded bytes were on the free list the whole time; nothing could
*use* them, because no `SEGMENT_SIZE`-aligned request can start mid-segment.
`prim::fixed` now places coarsely-aligned requests at the bottom of the lowest
extent that fits and merely page-aligned ones at the TOP of the highest —
requests that do not care about coarse alignment are the ones that can move.
Shipped cost: one pure arithmetic `fn place`. **Zero new unsafe in shipped
code**; the census went 883 -> 890 and all seven are `#[cfg(test)]`.

**The test needed poisoning twice before it meant anything.** Written against
the existing 512 KiB region it passed under the bug — a region that is an exact
multiple of `SEGMENT_SIZE` cannot tell the two policies apart, since a page off
either end costs a segment either way. `K * SEGMENT_SIZE + FIXED_PAGE` on an
aligned base is what discriminates, and is what the board actually has. Poisoned
separately, the two halves report offset 0 instead of `N - FIXED_PAGE`, and 7
segments of reach instead of 8.

### Lever 2 — the slice, and the invariant a failed probe uncovered

`SEGMENT_SLICE_SIZE` 8 KiB -> 4 KiB with `SLICES_PER_SEGMENT` 8 -> 16, so
`SEGMENT_SIZE` deliberately does not move: the quantity that mattered is
pages-per-segment, 7 -> 15.

**A 2 KiB probe then failed, and failing is what it was for.**
`properties::usable_size_agrees_with_good_size`: `good_size(49_153)` promised
53,248 while the 25-slice span delivered 51,200. `bins::good_size` answers the
large range with `os::page_align_up` while the large path allocates EXACT
SLICES, so `usable_size >= good_size` — ABI-visible — holds only while **a slice
is at least an OS page**. Free at every other geometry (64 KiB slice, 4 KiB
page), which is exactly why it was never written down. `good_size` is G2-pinned
against the oracle, so the slice is the side that moves. Now a
`const _: () = assert!` in `prim/fixed.rs`, at the one backend where the two can
be tuned into conflict.

### Three more findings, and one hypothesis killed

- **`slice_pool::rejects_what_it_cannot_track`** — the module's last
  byte-denominated test, and P2's defect shape verbatim. `MIB + 4096` was
  "misaligned" only while a slice was 8 KiB; at 4 KiB it became slice-ALIGNED,
  so the test *succeeded* at freeing two ranges it exists to refuse, and — the
  pool being global first-fit — took down three other tests instead of itself.
  Rewritten in slices, plus a drain assertion so a refusal that sets a bit fails
  here rather than next door.
- **`heaps.rs` arena** — `reserve_os_memory_ex(64 * 1024 * 1024, ...)` reads
  "two chunks" at 32 MiB segments and "2048 chunks" at 32 KiB ones, past
  `arena::MAX_CHUNKS` (1024). Now sized in chunks; the `.max(2)` keeps the
  default profile at exactly the 64 MiB it always reserved.
- **`MEDIUM_PAGE_SLICES` 4 -> 2 was a real regression, not a stale premise.** It
  lowers `MEDIUM_OBJ_SIZE_MAX` to 1,024 B, collapsing the binned range so a
  burst of 2 KiB objects takes a whole slice each instead of sharing a page.
  `spans.rs` caught it. Held at 4.
- **Killed cheaply:** `slice_pool::FREE` is a bitmap over the entire 32-bit
  address space (`1 << (32 - SLICE_SHIFT)` bits) — 64 KiB of BSS at the small
  profile, larger than the region. It costs nothing: every call site is
  `#[cfg(all(target_arch = "wasm32", not(miri)))]` and the linker drops the
  static everywhere else. Checked against the shipped firmware's symbol table,
  not argued from the source.

### What this does NOT do

esp-alloc's floor is bytes-live-plus-headers (~5.4 KiB here); rusty_alloc's is
`bins x slice`. These levers took 2.8x out of the gap and could not close it —
§2.9's conclusion is unchanged. 68 KiB is below the 96 KiB the esp-alloc arm is
*configured* with, but that is the example's number, not esp-alloc's floor, and
saying otherwise would be the dishonest version of this row.

### Verification

103 tests / 33 suites / 0 failed (default); 85 tests / 19 suites / 0 failed
(`ra_small_profile`); clippy `-D warnings` clean on default, small profile and
`no_std`; fmt clean; unsafe census RATCHET OK at 890 with `UNSAFE.md` updated;
builds for riscv32imac and riscv32imafc `no_std` at BOTH geometries, and wasm32.
Board kill test green.

## SMALL-METAL P4b — WHY it costs 192 KiB: ten bins, ten pages, thirteen slices (2026-09-07)

P4 measured the price. It did not measure the cause, and the difference decides
whether the gap to `esp-alloc` is a backlog or a floor. Asked on the board
rather than reasoned about: arm B now prints rusty_alloc's always-on counters
and a per-bin census of every `GlobalAlloc` request.

**Method.** Same XIAO ESP32-S3 Sense, same one-source two-arm harness, same
`espino run --expect` kill test (still green, 62 lines). Two additions to arm B
only: `rusty_alloc::alloc::stats()` at each stage, and an `AtomicU32[74]`
incremented at `rusty_alloc::bins::bin(size)` on every acquiring path
(`alloc`/`alloc_zeroed`/`realloc`). The census is in the wrapper, outside the
allocator, so it counts requests the workload makes, not decisions the
allocator takes — the two can then be compared.

```
[pages] end: generic 25 pages_fresh 10 extends 11 segments 2 large 0 huge 0
[bin] blocks touched: 4, 8, 16, 24, 32, 64, 96, 320, 384, 4096 B
[bin] distinct bins touched: 10
```

### The number that answers the question

**Distinct bins 10. Fresh pages 10.** An identity, not a correlation: a page
serves one size class and is at minimum one SLICE, so the floor is
`bins x slice`, independent of bytes demanded.

| | slices | bytes |
|---|---:|---:|
| 9 small pages (all blocks ≤ `SMALL_OBJ_SIZE_MAX` = 1 KiB) | 9 | 73,728 |
| 1 medium page (block 4,096 = `MEDIUM_OBJ_SIZE_MAX` exactly) | 4 | 32,768 |
| **pages needed** | **13** | **106,496** |
| usable slices per segment (8 − 1 header) | 7 | |
| **segments** | | **2** — 14 usable, one spare |

Region, to the byte: `4,096` (create_heap's one `os::alloc_aligned` page)
`+ 61,440` (first-fit alignment hole) `+ 2 x 65,536` = `196,608` = the 192 KiB
P4 found empirically by watching 128 KiB panic. The empirical number and the
arithmetic now agree, which is the check that the decomposition is real.

**Occupancy: 4,914 live bytes in 106,496 bytes of pages — 4.6 %.**

### Ranked levers, and the part that is not a lever

1. **Alignment hole — 61,440 B, 31 % of the region.** The only line that is a
   defect. Fix `prim::fixed` to return the skipped prefix; 192 KiB → 132 KiB.
   Arithmetic, not projection. Not yet done.
2. **`SEGMENT_SLICE_SIZE`** multiplies all 106,496 B. `SEGMENT_SIZE` is not the
   lever; the slice is. Coupled to `SMALL_WSIZE_MAX` via
   `SMALL_OBJ_SIZE_MAX = SMALL_PAGE_SIZE/8`, which at 8 KiB lands exactly on
   `SMALL_WSIZE_MAX * 8` = 1,024 — so the two must move together. Direction
   certain, magnitude unmeasured, and it stays unmeasured until it is measured.
3. **Retention.** At `end`, live 0 and region used still 135,168 — freed, not
   returned. mimalloc keeps retired pages as a reuse cache; on an MCU that turns
   the peak into the floor.
4. Flash +11,296 B (+9.7 %) — real, not the scarce resource.

**Not a lever:** esp-alloc's floor is `bytes live + headers` (~5.4 KiB here);
rusty_alloc's is `bins x slice` (106 KiB here) **regardless of byte demand**.
Different functions, not one function tuned differently. Levers 1 and 2 are
worth taking on their own merits; they do not close 20x, and saying they might
would be the dishonest version of this entry.

**The corollary that is actually useful:** the page cost is roughly FIXED for a
bin profile — the same 13 slices serve 5 KB or 500 KB. The crossover is where
live bytes approach `bins x slice`. Below it esp-alloc wins by construction.

**Kill test after instrumentation: still PASSED** (the counters are the
instrument's tax, and it is paid in the arm being measured, so the region
numbers are unchanged from P4: 135,168 used, 61,440 free, PEAK 4,914).

## SMALL-METAL P4 — IT RUNS ON A CHIP, and the region costs 2x esp-alloc for a 4.9 KB workload (2026-09-07)

P4 of `docs/plans/small-metal.md`, **on real hardware**: a Seeed XIAO
ESP32-S3 Sense (esp32s3 rev v0.2, 8 MB flash, MAC 68:ee:8f:51:74:64) on COM4,
Track B bare metal — `esp-hal` 1.2.0, `no_std`, `panic = "abort"`, the
`ra_small_profile` geometry, `rusty_alloc-api` as `#[global_allocator]`.

### Kill test: PASSED

The plan asks that the board print the filesystem geometry, the config file and
the page title, and blink at the file's rate, with this allocator underneath:

```
[heap] arm: rusty_alloc
littlefs 2.0: 1261 blocks of 4096
config.json: { "blink_ms": 250, "greeting": "hello from data/config.json" }
index.html: 318 bytes, title "blink-fs"
blinking GPIO21 every 250 ms
```

250 ms is the value **read from `config.json`**, not the 500 ms fallback the
sketch uses when the filesystem is missing — which is what makes the line
evidence that the whole path worked. Reproduced identically across runs.

**One thing this session cannot confirm:** the LED itself. The firmware reports
the interval it read; the espino ledger's own P1 row says "the LED confirmed by
eye", and nobody's eye is on this board from here.

### Method — and the control came first

Both arms are ONE binary source in one cargo project
(`espino/examples/blink-fs-p4`, copied from the green `blink-fs`), with the
allocator selected by `--cfg ra_arm_rusty`. One dependency graph, one
workload; the arms cannot drift.

**A control arm ran before anything was changed**, and it earned its place: the
unmodified example flashed to this board reported *"no filesystem at the
record's partition"* and blinked at the 500 ms fallback — the board had no
LittleFS image. Every later reading would have been ambiguous. `espino run`
packs and flashes the filesystem alongside the app, and the control then passed
the full kill test under `esp-alloc`.

### The instrument had to be built, because neither allocator's own stats answer the question

The first attempt printed `esp_alloc::HEAP.used()` at three stages. It read
**`used 0` at every one** — the file buffers are dropped before each sample. A
perfectly clean number measuring nothing. And esp-alloc's `max_usage` is behind
a feature `rusty_alloc` has no counterpart for, so it would have compared
against nothing.

So the peak is measured **outside both**, by the same code in both arms: a
`GlobalAlloc` wrapper tracking live bytes with `fetch_max`, forwarding
`alloc`/`alloc_zeroed`/`dealloc`/`realloc` so each allocator keeps its own
behaviour. esp-alloc's `global-allocator` feature is off so the wrapper can sit
in front of it. Its two atomics per call are the instrument's tax, paid
identically by both arms.

### The numbers

| | esp-alloc | rusty_alloc |
|---|---:|---:|
| **workload PEAK live bytes** (shared instrument) | **4,914** | **4,914** |
| region given | 96 KiB | 192 KiB |
| region consumed | 0 at every stage | **135,168** (132 KiB) |
| smallest region that runs | — | **192 KiB** (128 KiB panics) |
| app image | 116,032 B | **127,328 B** (+11,296, **+9.7 %**) |

**PEAK is identical to the byte.** That is the work-parity check
(`codec-measurement` §4) passing exactly: both arms performed the same
allocation work, so the allocator is the only variable. It also discharges the
caution P3 left for this phase — the mid ledger's `alloc`-rung measurement that
first came back byte-identical because LTO dropped a rung nothing called. Here
both allocators are provably reached: the counter moved in both arms, and
rusty_alloc's region consumption moved from 0 to 132 KiB.

### Where the 132 KiB goes, predicted before it was measured

`135,168 = 4,096 + 2 x 65,536` — an arena descriptor plus two segments. And
`free 61,440` is **60 KiB stranded by alignment**: `prim::fixed` is first-fit,
so the 4 KiB page-aligned arena descriptor takes the bottom of the region, which
pushes the first `SEGMENT_SIZE`-aligned segment to offset 64 KiB and the second
to 128 KiB — so two segments need a 192 KiB region even though they occupy 132.

That was written down as a prediction and then tested: **at a 128 KiB region the
board panics in `handle_alloc_error`**, exactly as predicted, because only one
segment can be placed and one segment's seven usable slices do not serve this
workload.

**The actionable half:** placing sub-segment allocations at the TOP of the
region (or best-fit rather than first-fit) would make 132 KiB sufficient and
recover a third of the region. That is a `prim::fixed` change, not an
architecture change, and it is the single cheapest improvement P4 found.

### The honest verdict on §5's "is the win real"

The workload's true demand is **4.9 KB**. `esp-alloc` serves it from a 96 KiB
heap that is already 20x the demand; `rusty_alloc` needs **192 KiB — 2x the
region and 37 % of the S3's entire 512 KiB of SRAM** — plus **+9.7 % of app
flash**, to serve the same 4.9 KB. The reason is structural rather than
wasteful: a segment is the allocation unit, and 64 KiB is the smallest segment
this geometry offers.

So the performance case is not merely absent, it is negative on the axis a chip
cares about, and §5's sentence stands as written: *"the safety argument has to
carry the whole weight on its own, and it may not."* P4's contribution is that
the sentence now has numbers under it instead of a suspicion — and that the
allocator demonstrably RUNS, which was never certain before today.

## SMALL-METAL P3 — it builds for a chip: 39 errors to 0, and a half-finished narrowing the whole battery could not see (2026-09-07)

P3 of `docs/plans/small-metal.md`: decide `portable-atomic` versus narrowing
**per site, with the reason recorded per site**, and add the single-heap
profile. **Kill test met on every target.**

| target | geometry | result |
|---|---|---|
| `riscv32imac-unknown-none-elf` | default + small | **builds**, debug and release |
| `riscv32imafc-unknown-none-elf` | default + small | **builds** |
| `xtensa-esp32s3-none-elf` (esp toolchain, `-Z build-std=core`) | default + small | **checks clean** |
| x86-64 host, `--no-default-features` | — | builds |
| `wasm32-unknown-unknown` | — | builds |

Host battery unchanged: **33 suites / 105 tests / 0 failed** (104 + P3's new
regression test); small profile 19 / 85 / 0; clippy `-D warnings` clean on the
default, small-profile AND `no_std` configurations; fmt clean; census
re-baselined with its entry.

### The atomics decision, per site — narrow a CHOICE, shim a CONTRACT

| site | what it is | decision |
|---|---|---|
| `arena.rs` `used` / `dirty` | the CAS'd chunk bitmaps — the **only correctness-path** 64-bit atomic in the crate | **narrow** `u64` → `u32` |
| `segment_map.rs` `MAP` | window bitmap | **narrow** |
| `slice_pool.rs` `FREE` | slice bitmap | **narrow** |
| `random.rs` `COUNTER` | seed-mixing counter | **narrow** to `usize` |
| `options.rs` `VALUES` | `options::{get,set}` are `i64` in an API **frozen at v2.0.0** | **`portable-atomic`** |
| `options.rs` `HEARTBEAT` | handed to a `DeferredFreeFun` whose **C ABI** declares it `u64` | **`portable-atomic`** |

The rule that falls out, and it decided all six: **a bitmap's word width is a
free choice — same total bits either way — so narrowing costs nothing and keeps
the claim/verify loop lock-free. A width that appears in a frozen signature or
a C ABI is a contract, and hand-rolling a 64-bit atomic out of two 32-bit
halves inside an allocator is exactly how you get a subtle bug.** Four narrowed,
two shimmed.

The dependency is `[target.'cfg(not(target_has_atomic = "64"))'.dependencies]`,
so **the crate stays dependency-free on every target it currently ships to** —
x86-64, aarch64, wasm32, Windows. It compiled on the Xtensa check and on
nothing else, which is the confirmation the gate works.

### The near-miss: a half-narrowed bitmap that 105 tests could not see

After the element type moved to `u32`, **four loop bounds still said
`div_ceil(64)` and `(w + 1) * 64`** — `arena.rs` lines 156, 157, 271, 275, 276,
281, 301 and 592, which the first regex pass had missed because it only matched
the `[idx / 64]` and `(idx % 64)` forms.

**The entire battery passed.** Not because the bug is benign — a short scan
means chunks past the first word are unreachable, and the `dirty` init
under-marks — but because **every arena any test builds is 32 chunks or fewer,
and at ≤32 chunks `div_ceil(64)` and `div_ceil(32)` are both 1.** The
divergence starts at chunk 33. The largest arena in the suite was 2 chunks.

That is `codec-measurement`'s "a green test can test the wrong scenario",
arrived at from the inside: the suite was not weak, it was *unable to express*
the defect. `tests/heaps.rs::arena_bitmap_reaches_past_its_first_word` now
allocates 33 chunks from an exclusive arena, and poisoning one bound back to
`64` reports precisely: **"chunk 32 of 33 refused — the bitmap scan stopped at
word 1 of 2"**, chunk 32 being the first in word 1.

**The test's own first version was wrong, and measured rather than reasoned.**
It sized each allocation at `LARGE_OBJ_SIZE_MAX + 1` — "the smallest size that
takes a whole chunk" — and failed at chunk 16 of 33. That is not the defect: a
huge block's header pushes `header + size` into a **second** chunk, so 33 chunks
is genuinely 16 allocations. `LARGE_OBJ_SIZE_MAX` exactly (the largest
in-segment span, which fills a segment's usable region) is one chunk. The
premise was fixed, not the allocator.

### The three things the crate used `std` for

- **`abort()`** (4 sites). `core` has none, so without `std` it panics — and a
  `no_std` consumer **must** build with `panic = "abort"`, which every Janus
  firmware profile already does. Documented at the function, because an abort
  that unwinds into a C caller is the guarantee gone.
- **`thread_local!` (4 sites) — this IS the single-heap profile.** With `std`
  the macro expands to `std::thread_local!` verbatim, so the shipped build keeps
  M10c's const-init initial-exec fast path untouched. Without it, a
  thread-local becomes a plain `static`: one heap, **no TLS lookup at all**, a
  *shorter* fast path than the threaded one. Sound because the crate serves
  `no_std` only on single-threaded targets — the same standing assumption
  `prim::fixed` already makes (constant thread id, TLS destructors that never
  fire, a spin lock that never contends), and the one new `unsafe impl Sync`
  says so and says what to revisit first if that changes.
- **The environment and the diagnostics** (§2.5). Deleted under `cfg`, not
  ported — a firmware has no environment, so every option keeps its compiled-in
  default and the pass does not exist rather than existing and returning
  nothing. **But the seam a firmware would actually use survives:**
  `options::out_fmt` takes `&str` and needs no allocation, so a firmware that
  registers an output hook still gets the allocator's messages over its serial
  log. Only the `eprint!` fallback and the `format!`-using CALLERS are std-only,
  and the error path still delivers the error *code* to a registered hook
  without one.

`std` is a **feature** (default on) where the geometry is a `--cfg`, and the
contrast is the point: a feature is additive and unifies across a dependency
graph, which is exactly right for "does this build have std" and exactly wrong
for "how big is a segment".

### A third instance of P0's bucket A

`random::os_entropy` and `stats::process_info` both select on
`windows` / `unix` / `wasm32` / `miri` — and a bare-metal target matches
**none**, so neither had an arm at all. That is the same defect shape P0 found
in `prim/mod.rs` and P1 fixed: **three instances in one crate of a four-way
platform selection with no default.** Both have a fifth arm now (no OS entropy;
no process accounting — the latter is what `process_info`'s doc already
promised, "unknown fields read 0").

## SMALL-METAL P2 — the geometry was TWO LINES, and it uncovered a real defect in the shipped allocator (2026-09-07)

P2 of `docs/plans/small-metal.md`: make the segment size a compile-time
parameter, add a small profile, and find out whether that ends in a port or in
a documented "no". **It is a port**, and on the way it found an escape from the
exclusive-arena API that is reachable in the SHIPPED configuration.

**The ceiling probe first, and it is the headline.** Rather than refactor the
~250 uses of the geometry constants across 14 files, change the constants and
see what breaks. A small profile — `SEGMENT_SLICE_SIZE` 8 KiB,
`SLICES_PER_SEGMENT` 8, so a **64 KiB segment** — behind a `--cfg`, built for
the host:

> **2 compile errors.** `segment_map.rs:27` and `slice_pool.rs:33` — both
> hardcoded shifts with a const assert pinning them to the shipped geometry.

`segment.rs`, `heap.rs`, `page.rs`, `alloc.rs`, `arena.rs` and `bins.rs`
compiled **unchanged** at a segment 512x smaller. The geometry was already
symbolic everywhere it mattered; two literals were the whole wall. Both are now
derived (`SEGMENT_SIZE.trailing_zeros()`), and `ADDR_BITS` with them — it was a
flat `48`, which on any 32-bit target sizes the map for 65,536x the memory that
can exist.

**Result: both profiles fully green.**

| | suites | tests | failed |
|---|---:|---:|---:|
| default (32 MiB segments) | 33 | **104** | 0 |
| small profile (64 KiB segments) | 19 | **84** | 0 |

clippy `-D warnings` clean on both. The shipped artifact is unchanged in
structure — dll 212,992 bytes and the same 316 exports as before P1 — though
that is a coarse instrument at 4 KiB PE alignment and is **not** a claim of
byte-identity on the fast path; the instruction counts need the Linux
callgrind harness, which did not run here.

### §2.6 confirmed, and it needed a third representation

The window bitmap is sized by the **address space**, not by the memory owned,
so shrinking the segment makes it *worse*: `WINDOW_SHIFT` 25 → 16 takes
`MAP_BITS` from 2²³ to 2³², i.e. 1 MiB of BSS to 512 MiB. Parameterising §2.1's
geometry alone would have made the crate LESS able to fit a chip.

Replaced for the small profile by an exact 64-entry range table — **1 KiB of
BSS against the bitmap's 1 MiB** — joining the wasm base table as a third
representation of one question. No allocator code path forked; only the map.

### Three defects, and only one of them was mine

**1. `huge_alloc` ignored the owning heap's arena. This is in the shipped
build.** `segment.rs` asked `arena::chunk_alloc_n(-1, chunks)` — a hardcoded
"any non-exclusive arena" — while `segment_alloc` twenty lines away correctly
passed `arena_id`. So a heap created with `create_heap(_, _, arena_id)`, whose
entire purpose is that its memory comes from ONE region, served **every**
allocation above `LARGE_OBJ_SIZE_MAX` from the default arena or straight from
the OS. Upstream does not: `mi_segment_huge_page_alloc` takes a `req_arena_id`
and both call sites pass `heap->arena_id`
(`oracle/mimalloc/src/segment.c:1671,1683`).

The small profile is what exposed it — at a 64 KiB segment the huge path starts
at 56 KiB, so an ordinary 100 KB allocation escaped — but **the defect is in
the 32 MiB geometry too**, reachable by any consumer of the exclusive-arena API
making one allocation past 32 MiB − 64 KiB. Fixed by threading the id and
refusing the OS fallback when `arena_id >= 0`, exactly as the normal path does.
The regression test is written at the **default** geometry so it guards the
shipped configuration, and it was poisoned back to the old behaviour to prove
it fires: `huge block at 0x1fdae010000 escaped its exclusive arena
[0x1fd9e000000, 0x1fdae000000)` — 64 KiB past the end.

**2. `segments` and `segments_freed` could not both be right.** The huge path
bumped `huge_allocs` and not `segments`, while the release path bumps
`segments_freed` beside `huge_free` (heap.rs:1243). A workload cycling huge
blocks therefore reports **more segments freed than allocated** — an impossible
reading, from the counters this project uses as its work-parity instrument for
every A/B. One line, and the test now asserts the pair.

**3. Mine, and the interesting half is the DIRECTION, not the size.** The range
table was sized at 32 entries; the host battery overflowed it and it dropped
ranges silently, so `contains` returned false for legitimate pointers and the
`debug_checks` guard began aborting good frees — three integration suites down.
Raising the number to 4096 made them pass, which **confirmed the cause and was
the wrong fix**: 64 KiB of BSS is not a chip-sized table.

The real defect is that the module's own doc — *"a false negative for
`is_in_heap_region`, never a false positive"* — is the wrong rule for this
consumer. `contains` backs two callers, and they want opposite things: for the
public query a false positive merely misleads; for the guard a false NEGATIVE
aborts a legitimate program. So the table now degrades **permissively** once it
can no longer decide, with a latch (`range_table_overflowed()`) that a test
reads, and the size stays chip-sized at 64 entries / 1 KiB. The one test that
asserts the negative direction now checks the degradation happened rather than
skipping blind.

Not a `debug_assert`: overflow is reachable in a VALID configuration (this
battery manages orders of magnitude more segments than any chip), and an
assertion should mean impossible, not "expected when you test off-target".

### What the small profile actually costs

- **Alignment ceiling is `SEGMENT_SIZE/2`** — 16 MiB shipped, **32 KiB** small.
  Inherent: a segment cannot promise an alignment it cannot hold. `malloc_aligned`
  returns null rather than aborting, which is the right failure.
- **`good_size` leaves the oracle.** Above `MEDIUM_OBJ_SIZE_MAX` it page-rounds,
  and that constant moves with the geometry, so the mimalloc-pinned bin table is
  a default-profile fixture. The rows the two geometries share still run.
- **Every per-segment structure scales as 1/`SEGMENT_SIZE`.** 512x smaller
  segments is up to 512x more of them for the same bytes managed. The range
  table noticed first; anything else sized by a guess will too.

### The test suite was pinning the geometry in three different ways

Nine tests failed at the small profile and **none of them was an allocator
defect** — a distinction worth making, because the raw count says otherwise:

- **A literal where the unit is slices** — `slice_pool` written in MiB ("1 MiB
  = 16 slices" became 128), `spans` allocating "1 MiB → 16-slice span" which at
  a 64 KiB segment is a HUGE block and never touches the span path at all.
  Rewritten in slices and in `SEGMENT_SLICE_SIZE` multiples.
- **A literal offset inside the segment** — the free-list link tests probed
  "1 MiB into the segment", which a 64 KiB segment does not contain, inverting
  three assertions about a predicate scoped to `SEGMENT_SIZE`.
- **Field-report fixtures** — `span_packing` reproduces named rows of the
  segment-tax report (its 60 % row at 20 MiB, its 27 % row at 25.1 MiB). A row
  is a size *against a segment size*; re-expressing them in slices would keep
  them green while testing nothing the report said, so they are gated to the
  shipped geometry and say why.

P1's §2.1 assertion behaved exactly as the plan asked: it was written one-sided
("a segment cannot come out of a 512 KiB region"), P2 inverted it, and it is
now two-sided — at the small profile it asserts that a **whole segment, at
segment alignment, IS served from a 512 KiB region**, which is P2's deliverable
demonstrated rather than described.

### The verdict on §5's first open question

*"Whether the small profile is the same allocator or a different one wearing the
name. If P2 ends with two architectures in one crate, that is worse than saying
no."*

**It is the same allocator.** No allocator code path is forked: the free path,
the page queues, the span carving, the cross-thread protocol, the arenas and
the bins are the shipped code running on different constants. The only
per-target divergence is the segment map's representation — which already had
two, for wasm — and one `--cfg` selecting three constants. A cargo *feature*
was deliberately not used: features are additive and unify across a dependency
graph, so two consumers wanting different geometries would silently get one of
them. The deliverable sets the cfg, the way a Janus firmware picks its chip.

## SMALL-METAL P1 — the memory seam: 16 of 55 errors gone, the shipped artifact provably untouched, and a defect of my own caught by the instrument (2026-09-07)

P1 of `docs/plans/small-metal.md`: introduce the primitive-memory seam and
implement it twice — the existing platform path, and a fixed-region path —
with nothing else changing.

**What landed.** `crates/rusty_alloc/src/prim/fixed.rs`, plus a fifth arm in
`prim/mod.rs`'s backend selection. P0 found that a bare-metal RISC-V target
matches none of `windows` / `unix` / `wasm32` / `miri`, so no `sys` module was
named at all; the new arm is `all(not(miri), not(windows), not(unix),
not(target_arch = "wasm32"))`. The module is **always compiled** (so it is
type-checked and unit-tested on the host) and **selected** only where no arm
above it matches, which is what makes "nothing else changes" true rather than
hoped.

The backend serves a `&'static mut [u8]` handed over once: a first-fit free
list of at most 32 extents in `AtomicUsize` arrays under a spin lock, with
splitting on alloc and coalescing on free. It cannot allocate its own
bookkeeping — it *is* the allocator's memory source — which is why the bound is
fixed and why exceeding it is reported rather than papered over.

**It adds zero unsafe dereferences to the shipped crate.** The census grew
864 → 881, and every one of the 17 is either an `unsafe fn` signature the seam
requires (6, with bodies containing no unsafe operation at all — the free list
is atomics and the pointers come from the safe `with_exposed_provenance_mut`)
or a test (11). `UNSAFE.md` carries the entry; re-baselined in the same change,
as the ratchet demands.

**The riscv debt, measured the way P0 established:**

| bucket | P0 | P1 | |
|---|---:|---:|---|
| A. `prim` has no backend for this target | 16 | **0** | **P1's job** |
| B. explicit `std::` paths | 10 | 10 | P3 |
| C. 64-bit atomics | 5 | 5 | P3 |
| D. alloc-dependent text | 13 | 13 | with P1's follow-up |
| E. cascade from A and B | 11 | 11 | free once B lands |
| **total (no_std probe)** | **55** | **39** | |

Every bucket P1 did not target moved by **+0**. The raw (non-probe) count went
224 → 237 — *up*, because with `sys` resolving, more code now reaches the
prelude cascade. That is the P0 artifact again, and it is why the probe number
is the one quoted.

**Kill test, item by item.**

1. **"The whole existing battery passes unchanged on the host."** 33 suites /
   103 tests / 0 failed; `clippy --workspace --all-targets --all-features
   -D warnings` clean; `fmt --check` clean; unsafe ratchet OK; `wasm32`
   still builds and still selects its own arm.

2. **"The benches move by less than the harness's own null-arm floor."** Met
   more strongly than asked, and the instrument had to be replaced to say so.
   The first attempt compared `sha256` of the shipped cdylib: it **differed**.
   A **null arm** — identical source, built twice — differed too
   (`c157c8e…` → `97a0ba8…`), because a PE embeds a build timestamp. **The
   hash cannot answer "unchanged" on this platform, and reading it would have
   manufactured a regression out of a clock.** On quantities that are
   deterministic, the artifact is identical: **dll size 212,992 both ways,
   delta 0**, and the exported-name set **316 vs 316, identical, zero
   differences**. There is no delta for a bench to resolve.

3. **"A new test builds an arena over a static 512 KiB region and serves
   allocations from it."** The **seam half is done and proven at 512 KiB**;
   the **arena half is blocked by §2.1** and cannot be written yet.
   `arena::arena_register` computes `chunks = size / SEGMENT_SIZE` and returns
   `Err` when `chunks == 0`, so **every region below 32 MiB is refused by
   arithmetic**. P1's kill test was written before that was known; the honest
   report is two-thirds met, with the third part deferred to P2 rather than
   redefined.

**§2.1 is executable for the first time.** P0 recorded that the wall ranked
first has no compile-time signature. It now has a runtime one: on a registered,
**entirely free** 512 KiB region, a `SEGMENT_SIZE` request fails, and so does a
one-page request at `SEGMENT_SIZE` *alignment* — the half no larger region
fixes, and P2's actual subject. Both leave the free list untouched.

**Two process findings, both from distrusting a green result.**

*A test that passed for the wrong reason.* The §2.1 assertion first lived in a
`#[test]` of its own. Run alone it **passed with no region registered** — i.e.
because `alloc` refuses before it looks at size, not because 32 MiB is too big.
Measured, not assumed: the standalone test was run and observed passing in that
state. It now sits inside the main test after the region is registered and
verified fully free, where the only thing that can refuse a segment is its size.

*The gate was poisoned to prove it fires.* Replacing
`try_alignment.max(FIXED_PAGE)` with `FIXED_PAGE` made the test fail at exactly
the alignment assertion; restoring it went green. A gate nobody has watched
fail is a claim, not a gate.

**And a defect of my own, caught by the bucket table.** The first version of
this backend used an `AtomicU64` for its monotonic tick — a 64-bit atomic, in
the backend written *for* the target that does not have them. Bucket C read
**5 → 6** and named it. It is now two `AtomicU32` widened to `u64`, under a
**separate** lock: `Guard` is not reentrant, so sharing the free-list lock
would deadlock the first allocation path that wanted a timestamp. A 32-bit
counter alone was rejected because wrapping inverts the purge *ordering* that
is the only property this clock provides.

Nothing else changed. `rust-toolchain.toml` also gained `wasm32-unknown-unknown`,
which the cross-check needed and which the repo already claims to support.

## SMALL-METAL P0 — 224 errors are 55, and the wall ranked first is invisible to this probe (2026-09-07)

P0 of `docs/plans/small-metal.md`: add `riscv32imac-unknown-none-elf` to the
toolchain file, build the core crate for it, **fix nothing**, and check the
error list against the plan's four walls. Nothing shipped but the target line.

**Method:** `rustup target add riscv32imac-unknown-none-elf --toolchain
1.97.1`; `cargo build -p rusty_alloc --target riscv32imac-unknown-none-elf
--message-format=json`, errors counted from the JSON rather than the human
output. Deterministic — a compile, not a measurement, so no pinning, no ABBA
and no noise floor. Reproducible from the target line alone.

**The raw number is not the finding.** The first build reports **224 errors**
(4 warnings). It is dominated by one root: two `E0463 can't find crate for
std`. Without `std` there is **no prelude**, so `Option`, `Some`, `None`,
`Result`, `Ok`, `Err`, `FnMut`, `Default`, `Sync`, `debug_assert`, `assert`,
`format`, `cfg` and `derive` all resolve to nothing. Classified:

| | errors |
|---|---:|
| prelude items + prelude macros (cascade) | **183** |
| everything else | 41 |

Acting on 224 would have meant "the plan is wrong, the list is materially
larger than the four walls" — the kill test's own failure condition, reached
entirely on an artifact. The rule this repository already writes down for
timings holds for compiler output: **a count dominated by one root is
measuring the root, not the program.**

**The probe that makes the list legible.** One line, applied, measured,
reverted — `#![cfg_attr(ra_p0_probe, no_std)]` at the top of `lib.rs`, built
with `RUSTFLAGS="--cfg ra_p0_probe"`. Not a fix and not kept; it removes the
single masking cause so the real debt can be read. **224 → 55.**

| # | bucket | errors | sites | plan |
|---|---|---:|---|---|
| A | `prim`: no backend selected for this target | 16 | `prim/mod.rs` (1 cause) | §2.4 |
| B | explicit `std::` paths — TLS, abort, io | 10 | `init.rs` 5, `options.rs` 3, `page.rs` 1, `random.rs` 1 | §2.3 |
| C | 64-bit atomics | 5 | `arena.rs`, `options.rs`, `random.rs`, `segment_map.rs`, `slice_pool.rs` — one each | §2.2 |
| D | alloc-dependent text | 13 | `options.rs` 7, `stats.rs` 3, `arena.rs` 2, `init.rs` 1 | **not in the plan** |
| E | cascade from A and B | 11 | `init.rs` 10, `random.rs` 1 | — |

**A is one cause, not sixteen.** `prim/mod.rs` selects its backend with four
arms — `windows`, `unix`, `target_arch = "wasm32"`, `miri`. A bare-metal
RISC-V target matches **none**, so no `sys` module is named at all and all
sixteen call sites through it fail together. §2.4 confirmed, and cheaper than
it reads.

**C is confirmed and its open question is answered.** The five sites split
one way on correctness: `arena.rs:51-52` `used`/`dirty` are the CAS'd chunk
bitmaps — the **only correctness site**, and a bitmap whose word width is a
free choice. `options.rs:195` `HEARTBEAT` and `random.rs:61` `COUNTER` are a
heartbeat and a seed counter. `segment_map.rs:30` and `slice_pool.rs:38` are
statics, discussed below. So `portable-atomic` may be needed nowhere;
narrowing covers every site. That is the same call the Janus programme made
for its own counters on 2026-09-06 (espino ledger: *"`AtomicU64` does not
exist on 32-bit RISC-V … Counters are `AtomicU32` now, `Stats` still reports
`u64`"*).

**D is a fifth wall the plan does not have, and it is the cheapest one.**
All thirteen are environment parsing and human-readable diagnostics:
`options.rs::ensure_init` reads `RUSTY_ALLOC_*` / `MIMALLOC_*` through
`std::env::var` with `to_uppercase` / `to_ascii_lowercase` / `format!`;
`arena.rs:605` returns a `String` debug dump; `stats.rs::process_info`
reports RSS, commit and page faults. **A firmware has no environment, no
process and no stdout.** The fix is `cfg`-ing the layer out, not porting it —
so D costs less than its error count suggests, and it is deletion rather than
an `alloc` dependency.

**§1 of the plan is wrong on its load-bearing claim.**
`crates/rusty_alloc/src/lib.rs` has **no `#![no_std]`** — the core is a std
crate, and its own module doc says so (*"A no_std profile returns post-v1"*).
The plan's *"The core is `no_std`"* cites
`crates/rusty_alloc_api/src/lib.rs:14`, which is the thin **API surface**, not
the core. `rusty_alloc_ffi/src/lib.rs:9` asserts *"the core crate is no_std"*
in a comment; that comment is false and should go with the P1 work. What IS
true, and is better news than the claim it replaces: **the core crate has
zero dependencies**, so nothing external can block the port.

**§2.1 produced zero errors, and cannot produce any.** The wall the plan
ranks first and calls "the wall" — a 32 MiB segment against a chip's whole
address space — is a *space* property, not a *type* property. The compiler has
no opinion on it. **P0 is structurally blind to §2.1**, exactly as `opscan`
was blind to the park/unpark thrash, and for the same reason: the instrument
does not enter the regime. Sizing it needs P2, or a link, not a check.

Two statics found while reading C, both space rather than type, and neither in
the plan: `segment_map::MAP` is `[AtomicU64; 131072]` — **1 MiB of BSS on
every non-wasm target**, against a Janus firmware heap of 64–220 KiB and an
ESP32-S3's 512 KiB of internal SRAM. `slice_pool::FREE` adds 8 KiB with no
`cfg` at all, on every target, though it is only used on wasm. wasm already
replaces the first wholesale with a 256 KiB base table, so the per-target
precedent §1 claims for the arena exists here too, and is stronger.

**Kill test — the plan stands, amended.** The list is not materially larger
than the four walls: three are confirmed (A/§2.4, B/§2.3, C/§2.2), one is
unmeasurable by this phase (§2.1), one new wall is real but cheap (D), and
§1's premise is false. Rewriting is not needed; five corrections are. Nothing
was fixed and nothing was kept except the toolchain target line.

## RE-BENCHMARK after the huge-path fix — zero cost, and the HARNESS was lying in the 4th digit (2026-08-19)

Asked to confirm the `remove_huge_segment` fix cost nothing. It costs nothing,
and looking for that answer properly turned up an instrument defect.

**The A/B, and it is a real one:** the pre-fix commit (`aca50c6`) built into
its own target dir and measured ABBA-interleaved against HEAD in the same
session, same box, same driver binary — not HEAD against a number written
down earlier.

| op | pre-fix | HEAD | delta |
|---|---:|---:|---:|
| **huge** (the ONLY path the fix touches) | 666.00 | 666.00 | **+0.00** |
| big / large | 171.00 | 171.00 | +0.00 |
| batch_lifo | 60.17 | 60.17 | +0.00 |
| small | 79.38 | 79.38 | +0.00 |
| mixed | 140.07 | 140.07 | +0.00 |

Zero, exactly, everywhere. Expected on reflection: the walk already computed
the answer, so `if self.remove_huge_segment(seg)` branches on a value already
in a register, and the found path — the only one a correct program takes —
falls straight through to the call it always made. `huge` repeated 666.00
three times.

**The instrument defect.** The real-program ratios moved in the 4th digit
between runs (perl 0.9989 → 0.9992), which should be impossible: this ledger
records perl and sqlite as "deterministic to 4-6 digits". Rather than wave it
off as noise, measured it — three repeats per arm:

| | run 1 | run 2 | run 3 | spread |
|---|---:|---:|---:|---:|
| unpinned, ra | 778,844,391 | 778,625,662 | 778,855,141 | **229,479** |
| **PERL_HASH_SEED=0, ra** | 777,444,364 | 777,444,364 | 777,444,364 | **0** |

**Perl randomises its hash seed per PROCESS**, so every run allocated a
different pattern — ~0.03% of noise sitting exactly on the digit the ra/mi
ratio is quoted to. `bench/icount-arms.sh` now pins `PERL_HASH_SEED` and
`PERL_PERTURB_KEYS`; the perl arm is bit-identical run to run and the ratio
is a stable **0.9991**.

**The part worth remembering: this project had already learned this.**
`bench/rss.sh` pins `PERL_HASH_SEED` — added 2026-08-06 after an 11 MiB swing
on the same binary invalidated three RSS conclusions, and that entry states
the rule as *"no null arm, no result"*. The lesson lived in one harness and
was never carried across to the other, so the icount harness quietly repeated
the same mistake in a different unit. **A lesson that lives in one instrument
is not a lesson.** Lua's seed is not pinnable from the environment, so it
stays labelled indicative; perl and sqlite are the verdicts.

Standing numbers after both: lua 0.9797, **perl 0.9991**, sqlite 1.0003.

## HARDENING — a semgrep rule written from our own history found the 0.4.0 bug still live on the huge path (2026-08-19)

Second finding of the day from a tool that had never been run here, and the
same shape as the first: an instrument the audit required, run for the first
time, immediately pointing at real code.

**The finding.** `tools/semgrep-rules.yml` encodes five rules taken from this
project's own incident log rather than from a generic ruleset. One of them
looks for `debug_assert!(false, …)` used as an error path — the shape that
caused 0.4.0 defect #3, where `remove_segment` failed to unlink a segment, the
assert vanished in release, and the caller freed it anyway, leaving a dangling
list head that crashed `thread_done`'s walk.

The rule fired on `Heap::remove_huge_segment`. **The normal-segment path was
fixed in 0.4.0; the HUGE path kept the unsound shape.** It returned `()`, so:

```rust
self.remove_huge_segment(seg);   // silently found nothing in release
let _ = huge_free(seg);          // ...released it regardless
```

Fixed identically to its sibling: `-> bool`, `#[must_use]` with a message
naming the consequence, and the one caller now releases only what it unlinked.
This is the same lesson the 0.4.0 entry already records — *put the outcome in
the type, not in a comment* — applied to the site that was missed.

**And this time the fix is REPRODUCED, not merely reasoned.** The standing
complaint against the 0.3.2-era repairs is in this ledger already:
`teardown_reclaim.rs` "passes 4/4 with the bug deliberately reintroduced", so
it guards nothing. To avoid repeating that, the walk was split into
`try_unlink_huge_segment` (the decision, no diagnostic) and
`remove_huge_segment` (decision + `debug_assert!`) — because with the assert
inline, a test build panics on the not-found path and the `false` return, the
entire point of the fix, can never be observed. `heap::unlink_tests` then
checks the decision directly, and was verified BOTH WAYS: reintroducing the
bug (`true` instead of `false` on the not-found path) makes it fail with the
message naming defect #3; restoring the fix makes it pass. The +3 `unsafe`
occurrences from the split were documented in `UNSAFE.md` and re-baselined,
which is the ratchet working as designed.

**Also from this pass, each an instrument run for the first time:**

- **Kani (H-30):** five harnesses, all `VERIFICATION: SUCCESSFUL`. The one
  that matters most proves `page_of`'s slice index is in range for EVERY
  in-segment offset — the contract that justifies M10b's removal of its
  bounds check. Miri, the fuzzers and loom can only say "no counterexample
  found"; this says none exists. Two limits stated, not hidden: Kani cannot
  analyse the crate's `global_asm!` (run with `--ignore-global-asm`; no
  harness touches that code), and the two bin-geometry proofs are BOUNDED to
  `2 × MEDIUM_OBJ_SIZE_MAX` because unbounded 64-bit `leading_zeros`/shift
  reasoning did not terminate (>13 CPU-minutes, killed).
- **ChaCha8 vetted (H-34):** the bespoke CSPRNG's quarter-round — its entire
  cryptographic core — now checks against **RFC 8439 §2.1.1's published test
  vector**, with the block layout checked against the RFC's state and the
  64-bit counter's advance AND carry checked (a stuck counter repeats the
  keystream). 7 tests.
- **Foreign-pointer guard (H-19, R-001):** `free()` now consults the segment
  map before deriving metadata, so a pointer this allocator never returned is
  caught at the call rather than producing a wild `slot.sub(off)`.
  Debug/`debug_checks` only, MEASURED at zero release cost, and **proven to
  fire** by `tests/foreign_free.rs` — a gate nobody has watched fail is not a
  gate.
- **Unsafe ratchet (H-11):** `cargo geiger` does not compile on 1.97.1 in any
  version tried, so the substitution is `tools/unsafe-census.sh` + a committed
  baseline that FAILS on growth. It caught its own first weakness honestly:
  version one counted the word "unsafe" in a doc comment and tripped on
  `proofs.rs`. A gate that cries wolf over prose is one people learn to
  re-baseline without reading, so the census now strips comments (808 in
  code, 21 files).

**Two rules were also REFINED after producing false positives** — the
correctly-`cfg`-gated counters, and a doc comment describing the bad shape.
The rule was wrong, not the code, and `tools/semgrep-selftest.sh` now asserts
all five still fire on synthesised bad code so the job cannot go green by
matching nothing.

## HARDENING — ThreadSanitizer found a real data race on the free fast path (2026-08-19)

The `use-protection-please` audit's H-24 (sanitizers) had never been run in
this project's life. Running it produced the session's most valuable result.

**The finding.** `cargo +nightly test -Zbuild-std -Zsanitizer=thread` on
`stress_mt` reported a data race, twice, at the same site:

| | where | what |
|---|---|---|
| **Read**, 1 byte | `alloc.rs` free fast path | `let flags = (*pg).flags` — routes the free |
| **Previous write**, same byte | `heap.rs` `adopt_segment` | `(*slot).flags &= !IN_FULL` — a non-atomic read-modify-write |

A thread ADOPTING an abandoned segment rewrites each page's flags while
another thread can be inside `free()` reading that same byte to decide the
free's shape. Both are plain, non-atomic accesses to the same location from
two threads: a data race, which is undefined behaviour regardless of what
today's codegen happens to do with it.

**It was benign in outcome, which is exactly why it survived.** `IN_FULL` is
part of `SLOW_FREE`, so a racing reader either takes the fast path or
`free_general` — and for a non-local free both routes end in `remote_free`
with the same block. Nothing was ever observed to break. But this is the same
abandon → adopt path that produced the 0.4.0 use-after-free family, and this
project has already been taught once (aarch64's first execution) what
"harmless on x86-TSO" looks like immediately before it stops being harmless
on weakly-ordered hardware.

**Fix: `Page::flags` is an `AtomicU8`, all 15 access sites `Relaxed`.**
Relaxed is correct and sufficient — the byte carries no happens-before
obligation of its own; the segment's `thread_id` Acquire load already orders
what the free path needs. **TSan now exits 0 with zero warnings.**

**The cost, measured rather than asserted: exactly +1.00 Ir/op, everywhere.**
A plain field read let LLVM fold the load into the test's memory operand
(`test BYTE PTR [pg+0x4d],0xf`); it will not fold an *atomic* load, so the
sequence becomes `movzx` + `test`.

| op | before | after | vs mimalloc |
|---|---:|---:|---:|
| batch_lifo / fifo | 59.17 | **60.17** | 0.991 → **1.008** |
| small | 78.38 | 79.38 | 0.713 |
| mixed | 139.07 | 140.07 | 0.888 |
| lua / perl / sqlite | — | — | **0.980 / 0.999 / 1.000** (unchanged) |

So the two synthetic batch ops go from 0.9% ahead of mimalloc to 0.8% behind,
and real programs do not move. **Kept.** It is the same trade already made for
double-free detection (~0.4%, kept deliberately): an allocator whose premise
is memory safety does not keep a data race on its hottest path to win 1.7% of
a microbenchmark. Upstream mimalloc reads these flags non-atomically, does not
pay the instruction, and has the race. The README now says 11-of-13 rather
than 13-of-13 and explains why in place.

**The fuzz soak that followed, and it is the number H-27 will be built on:**
5,047,688 executions of `alloc_ops` and 1,940,317 of `xthread` — **~7.0
million inputs through the full public surface under ASan, zero crashes**.
The runs added 2,951 and 883 new corpus units; `cargo fuzz cmin` minimized
those to **431 + 164 files (1.2 MB) preserving 515 + 446 coverage edges**,
now committed. Committing the minimized corpus rather than only the seeds is
the point: the nightly job carries coverage forward through the actions
cache, and a cache eviction would otherwise restart discovery from zero.

**Also landed in the same sanitizer/dynamic pass:** ASan clean over the core
suites (13 tests) and both fuzz targets;
`cargo careful test` green (21 tests); and `tests/properties.rs` — 8 proptest
properties over the documented invariants, including "live blocks never
overlap" checked only after every block is live, so an overlap cannot be
masked by a later write. Non-vacuous by construction: runtime scales with
`PROPTEST_CASES` (0.02 s at 256 → 0.46 s at 8192).

## The batch gap CLOSED — every opscan op now at-or-ahead of mimalloc (2026-08-19)

The 0.4.0 re-baseline (fresh oracle + fresh override build, sha256-matched to
the shipped 0.4.0 artifact) put the whole remaining per-op deficit in two
places: `batch_lifo/fifo` **+9.42 Ir/op (1.158×)** and `aligned` **+25.55
(1.136×)**. Method throughout: `bench/opscan.sh`'s two-point estimator
(`(Ir(2N)−Ir(N))/N`, callgrind, LD_PRELOAD into one neutral C driver) — a
deterministic COUNT, exact to ~0.01 Ir/op, so every keep below is a one-run
verdict. Five bricks, each disassembly-verified, and the ranking table now has
an empty "we lose" column:

| op | before | after | mi | ra/mi now |
|---|---:|---:|---:|---:|
| batch_lifo | 69.12 | **59.19** | 59.70 | **0.991** |
| batch_fifo | 69.11 | **59.17** | 59.68 | **0.991** |
| aligned | 213.44 | **163.25** | 187.89 | **0.869** |
| small | 86.37 | **77.39** | 111.41 | 0.695 |
| med | 94.56 | **85.75** | 123.02 | 0.697 |
| mixed | 147.84 | **140.26** | 157.75 | 0.889 |
| realloc | 406.05 | **388.03** | 499.82 | 0.776 |
| calloc | 156.50 | **151.50** | 160.89 | 0.942 |
| big / large | 177.00 | **171.00** | 222.02 | 0.770 |

**Real programs moved too** (`bench/icount-arms.sh`, deterministic): lua
0.9882 → **0.9823**, perl 1.0059 → **0.9985**, sqlite 1.0037 → **0.9999** —
all three at-or-below mimalloc's instruction count for the first time.

**Brick 1 — free: both cold outcomes on ONE branch, reached by a tail jump**
(batch 69.12 → 67.16). `page_push_local` now RETURNS the post-decrement
`used`; `alloc::free` tests `(u as i32) <= 0` once — 0 = page emptied
(retire), negative = double free (abort) — and jumps to a merged
`retire_or_abort`. With the abort no longer a `call` inside `free`'s body,
LLVM dropped the `push rax`/`pop rax` stack alignment: the fast path now has
NO prologue. The detection semantics are unchanged (same wrap test, same
SIGABRT; the general path in `free_local_at` checks its own return).
`retire_or_abort` re-reads `(*pg).used` instead of taking it as an argument —
passing the value in gave it a second use and would block the RMW fold (which
LLVM then still declined; the load/dec/store stays, priced at 1 Ir).

**Brick 2 — malloc: the empty-heap SENTINEL kills the null test** (batch →
63.34, small −3.95, med −3.25). The TLS slot moved from `.tbss` (null = not
initialised) to `.tdata` initialised to a static `EMPTY_HEAP_BOX` whose
direct table is all empty-page sentinels — upstream's `_mi_heap_empty`, and
the exact trick our own `EMPTY_PAGE` already plays one level down. A fresh
thread's first malloc falls through to the generic path like any dry page;
`malloc` never tests for initialisation at all. The fast path was rewritten
to RAW-POINTER READS ONLY (`alloc::malloc` no longer forms `&mut` before the
sentinel is ruled out — two threads taking `&mut` on the shared static would
be UB, and Miri runs this variant). The miss is a tail call, so `push rbx`
went with it: exported malloc is now 14 instructions (was 17).

**Brick 3 — malloc_slow goes STRAIGHT to malloc_generic** (mixed 147.03 →
144.52). The first cut re-ran `Heap::malloc`'s fast path inside the slow
call; the fast list was dry a moment ago on the same thread, so the repeat
can only miss. `mixed` (1/3 of its sizes > SMALL_SIZE_MAX) measured the
repeat at +0.68 Ir/op.

**Brick 4 — free: read the thread id BEFORE `page_of`** (batch 63.20 →
62.20). Read after it, LLVM assigned the id to the register holding the
just-computed page pointer and re-derived every later page access as
`slot + (−slice_offset)`. One statement moved; one instruction saved.

**Brick 5 — the exported `free` IS the body now** (batch 62.20 → **59.19**,
small −4.00, mixed −4.26). The override's `free` was a GOT-indirect `jmp`
thunk onto `alloc::free`. A narrowly-scoped `#[inline(always)] free_inline`
that ONLY the export calls gives the export the body. This is NOT the
thrice-refuted `#[inline]`-on-`free` — internal callers (realloc family)
still call the outlined symbol; only the export changed. It paid 3× the
expected 1 Ir because codegen in the export context (argument register free
to clobber) also deleted the `neg` and went to one-register addressing —
**the local free fast path is now 24 instructions vs upstream's measured
25 Ir/op.** Cost: the remote-free arm is outlined behind one extra `jmp`
(cross-thread frees only; the MT kernels run in-process and never see it).

**Brick 6 — aligned: peek the block, don't prove the geometry** (aligned
213.44 → 168.25 in isolation; **163.25 / 0.869×** with bricks 4–5
compounded — from 1.136× of mimalloc to a win). Ours proved
alignment via `bins::good_size` and then re-walked `malloc`; upstream tests
the bin's ACTUAL next free block with one AND (`_zero_aligned_at_fast`) and
pops it on a hit. Ported as a preamble in `malloc_aligned_at` (offset 0,
small sizes). Natural fits — the common case — always hit. A block can now
come from `bin(size)` where the old path picked `bin(asize)`: still
contract-conformant (aligned, usable ≥ size) and it is upstream's own
behaviour; `align_storm` + `aligned_at_offsets` hammer exactly this and pass.

**Gates:** Linux 28 suites / 0 failures, default AND `--all-features`;
clippy `-D warnings` clean; **Windows native 28 suites / 0 failures** (the
`thread_local!` sentinel variant); `bench/churn.sh` 5/5 (640 threads — the
probe aimed at exactly what a broken TLS slot would corrupt); **Miri whole
`-p rusty_alloc` target clean** — 33 tests, 0 UB, `stress_mt` (411 s
interpreted) and `rss_churn` (200 s) included; the `wasm-gate` self-test
executed in a Node VM; real-program icount arms above. Counters keep work
parity (the debug suites assert them). Housekeeping: `thread_identity`'s
three tests are now `#[cfg_attr(miri, ignore)]` with the reason in-file —
they verify hardware register reads whose asm is `cfg(not(miri))`, so under
Miri they interpreted 2M-iteration loops over the FALLBACK path (tens of
minutes proving nothing the other suites don't); same pattern as
`double_free.rs`, so the Miri gate keeps sweeping the whole target.

**Not re-tried, per the ledger's own refutations:** `#[inline]` on
`alloc::free` for internal callers (3×), cold splits with fat signatures,
`Page` 80 → 64 (the remaining `idx*80` lea chain is ~1 Ir and priced below
the refactor risk — still true).

### Real-world corpus sweep on the new build (same day) — the G6 CORRECTNESS half, demonstrated

`corpus/sweep-all.sh` is NEW: every built mimalloc-bench binary, standard
`bench.sh` arguments, ra and mi arms side by side, exit-code classified with
a timeout guard — the run-everything correctness companion to
`run-suite.sh`'s four-benchmark timing gate, valid on a loaded box because it
reads no clock. **19/19 configurations run to completion on rusty_alloc**
(cfrac · espresso · barnes · larson · larson-sized · mstress · rptest ·
alloc-test ×1/×8 · sh6bench ·sh8bench · xmalloc-test · cache-thrash ·
cache-scratch · malloc-large · mleak ×2 · glibc-simple · glibc-thread, 8–16
threads where MT), cfrac's factorization output byte-compared across arms.

`corpus/realworld.sh`, run THREE full passes (18 runs per program, arms
interleaved ra/mi/sys + sys/mi/ra per pass): **all 8 deterministic programs
byte-identical across every run of every arm** — jq, sqlite3, python3, git,
xz, zstd, lua, perl: one checksum each, 144/144 runs, zero non-zero exits.
imagemagick: 18 runs → 18 distinct hashes INCLUDING glibc-vs-glibc (embedded
PNG timestamps — a property of ImageMagick, not an allocator signal; every
run exited 0). redis: the ONLY failures in the whole sweep, and they are the
documented mixed-allocator configuration — this redis is BUILT on jemalloc
(`malloc=jemalloc-5.3.0`, links `libjemalloc.so.2`, verified via
`info memory`), so preloading ANY replacement corrupts by construction; both
preloaded arms failed (ours AND the oracle, which contains none of our code)
while the sys arm passed 3/3. In an earlier single pass our arm happened to
complete the full benchmark while the oracle segfaulted — scheduling luck
inside a broken-by-construction process, consistent with the 2026-08-06
measurement (mi 8/8 crashes, ours 6/8). The script now detects
jemalloc-linked redis and prints the note inline so this is never
re-investigated as an allocator defect.

Plus `stress_mt` release soak **30/30** on the new TLS/sentinel code.

**Scope note:** this demonstrates the v1 gate's CORRECTNESS half (every
corpus program runs, deterministic outputs identical). The PERF half — the
wall/CPU geomean-within-10% claim — still needs a pinned quiet-box session
and remains undemonstrated; instruction counts (above) are the standing
evidence.

Perf, RSS and the `secure` feature had all been *recommended* without numbers.
All three measured before cutting 0.4.0. Method throughout: deterministic
COUNTS wherever a count exists (callgrind instructions retired, RSS bytes), and
where a duration was unavoidable, ABBA interleaving with a **null arm**.

**1. Did the seven fixes cost performance? No — and this one is exact.**
Wall/CPU time in Docker could not resolve it (null arm read **1.0253** on means:
the environment's floor is ~2.5%, wider than any effect). So the verdict comes
from instructions retired, which has no noise floor:

| kernel | pristine 0.3.2 | fixed | ratio |
|---|---:|---:|---:|
| malloc-small (single-threaded, fully repeatable) | 229,924,005 | 229,921,893 | **0.99999** |
| larson-4t | 129.6–130.9 M | 129.7–130.9 M | ranges overlap |
| xmalloc-4p | 203.3 M | 203.3 M | **1.0000** |

2,112 instructions in 230 M. Expected: the fixes add predictable early returns
on the adopt/retire COLD paths and touch the malloc/free fast path not at all.

**The mimalloc arm, re-run directly** (oracle rebuilt from the vendored
submodule; all four arms LD_PRELOADed into the SAME neutral C churn binary, so
the allocator is the only variable):

| arm | instructions retired | vs mimalloc |
|---|---:|---:|
| glibc | 160,220,039 | 1.6835 |
| mimalloc v2.4.5 | 95,170,830 | 1.0000 |
| rusty_alloc 0.3.2 (pristine) | 107,943,033 | 1.1342 |
| **rusty_alloc 0.4.0 (fixed)** | **107,943,063** | **1.1342** |

**fixed / pristine = 1.00000** — thirty instructions in 108 million. That is the
definitive answer to "did seven fixes cost anything": no, and it is a count, not
an estimate.

**A caveat this measurement adds, and it is not flattering.** On this
allocation-CHURN microbenchmark rusty_alloc is **13.4% behind mimalloc** —
whereas the README's headline arms (lua/perl/sqlite under LD_PRELOAD) read
0.99–1.01. Both are true and they do not contradict: real programs dilute
allocator cost among everything else they do, while this workload is almost
nothing but malloc/free. The honest reading is that **parity is workload-
dependent, and the "at parity" claim should be read as scoped to the three real
programs it was measured on** — not as a general property. It also remains 33%
cheaper than glibc on the same workload.

**A surprising number that was WRONG, kept as a warning.** The first xmalloc
reading was **4.86×**. Re-run three times per arm it is 203.3 M both ways: the
outlier was a one-off scheduling artifact under callgrind. Work parity was
confirmed independently (both arms print `blocks=600000`, same seed). Re-verify
a surprising number before acting on it.

**2. RSS — one clear win, one open question.**

The decommit fix, measured directly (reserve 512 MiB, commit, touch every page,
decommit, read RSS):

| decommit impl | returned to OS | contents after |
|---|---:|---|
| pristine `MADV_DONTNEED` | **6.4%** (27.4 of 427.9 MiB) | `165` — STALE, contract violated |
| fixed `mmap MAP_FIXED` | **100.1%** (457.8 of 457.3 MiB) | `0` — zeroed, contract honoured |

Soaks (daemon-shaped: thread waves that exit holding live blocks, forcing
abandonment, against a bounded live set):

- **purge ENABLED** (`purge_delay = 0`), 6 min: RSS flat at **9.4 MiB**, slope
  **−0.02 MiB/min**, peak 14.8. Clean.
- **shipped default** (`purge_delay = -1`, purging opt-in), 25 min, 299 samples:
  ~650 MiB RSS against a ~175 MiB mean live set, drifting **+1.45 ± 0.70
  MiB/min** (least-squares, 95% CI) over the full run. The drift DECELERATES —
  first half +2.42 ± 2.06, second half **+1.19 ± 1.87, no longer
  distinguishable from zero** — and RSS does not track the live set
  (corr = **+0.034**), so this is retention approaching a plateau, or a slow
  leak, and **25 minutes cannot separate those two**. NOT claimed as settled.
  The naive two-endpoint slope this harness printed first (+2.69) is not a
  sound estimator on data with 307 MiB peak-to-peak oscillation; the regression
  above supersedes it.

Actionable consequence: **long-lived services should set `purge_delay >= 0`**
rather than rely on the opt-in default. That is the configuration with flat,
measured RSS.

**3. `secure` — works, and costs 4–7%.** Full suite green with
`--features secure`; `stress_mt` 30/30 in release. Cost, instructions retired:

| kernel | default | secure | ratio |
|---|---:|---:|---:|
| malloc-small | 229,921,829 | 245,439,205 | **1.0675×** |
| larson-4t | 130,150,515 | 136,816,187 | 1.0512× |
| xmalloc-4p | 203,333,568 | 211,720,930 | 1.0412× |

Throughput on the alloc-heaviest kernel: 84.0 → 73.6 Mops/s. A real but modest
price for guard pages + encrypted free lists on anything facing untrusted input.

**Also executed for the first time this round:** `wasm32-unknown-unknown` via
`bench/wasm-selftest.mjs` in a Node VM — **PASSED** (linear memory grew
2.06 → 64.00 MiB). The platform table said "tested in a VM self-test" on faith;
it has now actually been run.

## The abandon/adopt UAF family — `stress_mt` CLOSED, suite fully green (2026-08-08)

The open P0 from the entry below is fixed. **It was not a weak-memory-ordering
bug, and it was not aarch64-specific** — that hypothesis (recorded below, from
the fact that the same source passed 19/20 under Rosetta's TSO) was WRONG. It is
a family of three plain use-after-frees on the abandon → adopt → reuse path,
present on every platform. x86-64 survived them because the just-`munmap`ped
region there usually stayed mapped; native aarch64 unmaps a 32 MiB segment for
real and faults on the next touch. The correction matters: **these are latent
memory-safety bugs on x86-64 Linux and Windows too**, and the x86-64 arm of the
gate below improves as well.

**The shape, in one sentence: three functions can RELEASE the segment they were
handed, and each returned `()` — so every caller kept using the pointer.**

1. **`span_from_segments` used a segment that `adopt_segment` had released.**
   `adopt_segment` frees `seg` when it arrives empty and an empty one is already
   cached, or when a Huge segment's block had already died — then the caller read
   `(*aseg).used_pages` and called `span_alloc(aseg, …)`. Proof before fixing: a
   probe that `_exit(42)`s when the just-adopted pointer is the one adoption
   freed **fired in 19 of 20 runs, with SIGSEGV dropping to 0**.
2. **`adopt_segment` used a segment its own `retire_span` had released.** The
   tail's dead-large-span retire can empty the segment and release it; the very
   next line reads `(*seg).used_pages`. This was the residual crash — lldb put it
   in `adopt_segment` itself, at that read.
3. **`retire_span` freed a segment it had failed to unlink.** `remove_segment`
   ended in a bare `debug_assert!(false, "segment not in heap list")`, which is
   **compiled out in release**: the caller fell through and `segment_free`d a
   segment still linked in another list, leaving a dangling `h.segments` head
   that later crashed `thread_done`'s walk. A probe that `_exit(43)`s on the
   not-found branch fired in **4 of 30 runs**.

**Fix: put the outcome in the type, not in a comment.** `adopt_segment`,
`retire_span` and `remove_segment` now return `bool` and are `#[must_use]` with
a message naming the consequence. That is what makes this class non-recurring —
adding `#[must_use]` immediately surfaced all four remaining `retire_span` call
sites for audit (all four proved terminal and are annotated as such). Callers
that legitimately ignore it now say why in one line.

One premise checked rather than assumed while fixing: releasing a segment cannot
strand queued pages, because `used_pages` counts CARVED spans and a page is only
queued while carved — so `used_pages == 0` implies none of its pages are in a bin
queue. An earlier "park it instead of releasing" attempt was built on the
opposite assumption, measured no better (36/40), and was reverted rather than
kept as a belt-and-braces change.

**Gate, all on aarch64-apple-darwin unless noted, exit-code classified:**

| arm | `stress_mt` |
|---|---|
| pristine 0.3.2 | 0 / 20 |
| + the 4 platform fixes (entry below) | 17 / 30 ABBA |
| **+ this UAF family fix** | **30 / 30 ABBA · 100/100 release soak · 40/40 debug** |
| x86-64 (Rosetta), before | 19 / 20 |
| **x86-64 (Rosetta), after** | **30 / 30** |

`cargo test --workspace` is now **fully green on aarch64-apple-darwin — no
failures, no ignores beyond the pre-existing doctest** — and green on
x86_64-apple-darwin; wasm32 still builds. The `#[global_allocator]` smoke app
(Vec/String/HashMap/BTreeMap churn, 40 MiB allocations, cross-thread frees,
8-thread waves) passes 30/30.

`stress_mt` IS the regression test for this family: it failed 65–100% of runs
before and is now 100/100, so a reintroduction shows up immediately.

## aarch64-apple-darwin FIRST EXECUTION — 4 bugs fixed, 1 still open (2026-08-08)

The README's platform table said aarch64 "compiles; **never executed**". It was
executed, on macOS 26 / Apple Silicon (16 KiB pages), rustc 1.95.0. **Four
defects, two of them memory-safety class. Baseline 0.3.2 could not run a
realistic `#[global_allocator]` workload on this platform at all.**

Method for every rate below: the built binary run N times by a script that
classifies by EXIT CODE (0 pass / 101 panic / 134 SIGABRT / 139 SIGSEGV / 137
killed-at-timeout), arms interleaved ABBA so machine-load drift hits both
equally, binaries fingerprinted by sha256 before each arm — a stale binary
produced one bogus reading before that check was added.

**P0 — `thread_id()` read the wrong register (memory-safety).** The aarch64 arm
read `tpidr_el0`, which is the thread pointer on Linux/Android/BSD but NOT on
Darwin: Apple puts the thread pointer in `tpidrRO_el0` and uses `tpidr_el0` for
the CPU/cluster id. Measured directly: `tpidr_el0` returned small non-pointer
values (0x1002, 0x2005…), took **5 distinct values within ONE thread** over 3M
reads as it migrated cores, and **8 live threads produced only 5 distinct values
— distinct threads collided**. `thread_id()` is the ownership identity behind
`segment.thread_id`, so a collision routes one thread's `free` down the owner
(unsynchronised) path into another thread's segment. Fixed to
`tpidrro_el0 & !0b111` (Apple documents the low 3 bits as the CPU number — which
is exactly the observed drift). New standing gate `tests/thread_identity.rs`
asserts stability-within-thread and uniqueness-across-live-threads; verified to
FAIL against the old register before being accepted.

**P1 — subprocess isolation silently lost at teardown.** `abandoned_push` read
`my_subproc()` from a Rust `thread_local!` while running inside the
`pthread_key_create` destructor. Destruction order between a platform TLS
destructor and Rust's own TLS is unspecified everywhere, and is observably wrong
here: it read back 0, so a thread tagged into subproc N abandoned its segments
into the MAIN subproc. Probe: `ABANDONED_COUNT` was 1 (the abandon fired
correctly) but the segment sat on list 0, not list 1. Fixed by mirroring the tag
onto `HeapBox::subproc` — the box is alive for all of teardown because it is the
value passed *to* the destructor — and passing it into `abandoned_push`. Now
correct on every target rather than accidentally correct on one.

**P2 — `decommit` never returned memory to the OS.** `MADV_DONTNEED` is only
advisory for private anonymous memory on Darwin: it neither frees the physical
pages nor zeroes them, so purge was a no-op and RSS only ever grew (the
abandonment purge path is on by default, so this was live). It also violated the
documented "contents are lost" contract — caught by `prim.rs` reading back 42.
Nothing trusted that yet (`free_is_zero` is conservatively cleared on purge) so
it was not a disclosure bug, but it was a landmine. Fixed with a `MAP_FIXED`
anonymous re-map over our own range, which drops the physical pages and installs
zero-fill-on-demand ones, matching the Linux contract exactly.

**P3 — `process_info` RSS wrong on Darwin, in both directions.** `current_rss`
read `/proc/self/statm` (no procfs on macOS → 0), and `ru_maxrss` was scaled
×1024 as KiB when macOS/BSD report it in BYTES — a plausible-looking number
1024× too large, in the exact field the README lists as unmeasured. Both replaced
with one `task_info(MACH_TASK_BASIC_INFO)` call, which reports `resident_size`
and `resident_size_max` in bytes.

**Result.** Whole suite green on this platform except `stress_mt`. Realistic
`#[global_allocator]` workload (Vec/String/HashMap/BTreeMap churn, 40 MiB
allocations, cross-thread frees, 8-thread waves), ABBA-interleaved, n=20/arm:

| arm | pass |
|---|---|
| pristine 0.3.2 | **1 / 20** (19 × SIGSEGV, dies at the cross-thread-free stage) |
| with these four fixes | **20 / 20** |

**[CLOSED by the entry above — and the diagnosis below was WRONG: it is a plain
use-after-free family, present on every platform, not a weak-memory bug.]**

**STILL OPEN (P0-class, aarch64-native only).** `stress_mt`'s abandon → adopt →
reuse storm still crashes: release 7/20 pass, 11 × SIGSEGV, 1 × SIGABRT, 1 hang.
Consistent crash site `Heap::span_from_segments`, EXC_BAD_ACCESS on addresses
sharing a low offset with differing high bits (a walk off a stale/unmapped
segment), plus `debug_assert!(false, "segment not in heap list")` at heap.rs:524
and :977. Two experiments narrow it:

- **Not the thread-pointer path.** Forcing apple-aarch64 onto the safe
  `pthread_self` cached-TLS id leaves the rate unchanged (7/20 pass), so P0's fix
  is necessary but not sufficient — the remaining bug is elsewhere.
- **It passes under Rosetta.** The same source built for `x86_64-apple-darwin`
  passes **19/20**. Rosetta emulates x86 **TSO**, which is the signature of a
  WEAK-MEMORY-ORDERING bug in the lock-free abandon/adopt protocol — invisible on
  every platform tested so far (x86-64 Linux and Windows are both TSO), and only
  ever visible on genuinely weakly-ordered hardware. The alternative hypothesis,
  a 16 KiB-page geometry assumption, is NOT excluded: Rosetta also uses 4 KiB
  pages, so that variable moved too.

Note `tests/loom_xthread.rs` models the delayed-free/abandon PROTOCOL but not the
segment adopt path or the heap segment list, which is where the crash sits —
extending the model there is the obvious next probe.

**Also fixed: a test that hardcoded a 4 KiB page.** `bins::known_size_classes`
asserted `good_size(65537) == 69632`. Above `MEDIUM_OBJ_SIZE_MAX` good_size is
page-rounded, so the correct value is 81920 on a 16 KiB-page host. Split into a
property test deriving the expectation from `os::page_size()`.

## M9b — the free fast path, folded to one flags byte (2026-08-05)

Continuing the M9 win. The probe had already priced `prim::thread_id()`; the
remaining per-free work was a pile of separate loads answering one question —
"is this a plain binned page I can just push onto?":
`has_aligned`, `bin == BIN_HUGE`, `in_full`, and the segment's `kind` (a u32
compared against two magics, with a two-arm match on BOTH the page lookup and
the block recovery).

**Brick #3:** one `Page::flags` byte (`HAS_ALIGNED | SINGLE_BLOCK | IN_FULL |
HUGE_SEGMENT`). The free path now does ONE page resolution and ONE flags load;
the `SegmentKind` match is gone from the hot path entirely (a huge segment's
interior slices already offset back to slot 1, so `page_of` covers both kinds).
`in_full`/`has_aligned` bools were removed — the byte replaces them, so the
Page struct did not grow.

**Work parity proven before any timing was read** (§4): baseline and new
binaries report **byte-identical counters** on the same workload —
allocs 10 002 036 = frees, generic 604 676, pages_fresh 132, segments 1,
extends 553. Both arms do exactly the same work, so the comparison is valid.

**Gates:** Windows all-features green, Linux 21 suites / 0 failures, clippy
`-D warnings`, fmt.

**Harness defect fixed along the way:** `bench/pinvs.ps1` would not parse —
Windows PowerShell 5.1 reads a UTF-8 file as Windows-1252, and the em-dashes
in the comments turned into a parse error several lines later. THE timing
harness is now ASCII-only with a banner saying why. A harness that does not
run is a discipline that does not exist (§13).

**Numbers (pinned, CPU time, ABBA, same workload, work-parity verified) —
and the honest conclusion: THE CLOCK CANNOT RESOLVE THIS ON THIS BOX.**

| run | arms | pairs | median B/A | min B/A | win rate | z |
|---|---|---:|---:|---:|---:|---:|
| short | ~1 s | 21 | 0.908 (B faster) | 0.943 | 12/21 | +0.65 |
| long | ~7 s | 31 | **1.069 (B slower)** | 1.129 | 11/31 | −1.62 |

**The sign flipped.** Neither run resolves (|z| < 2 both times), and the
within-arm spread gives the game away: arm A's own median was 7234 ms against
its own minimum of 5812 ms — a **24% swing inside a single arm**, on a box
running two VS Code instances, a browser and Task Manager (checked, per the
go-find-the-process rule). A 5-10% effect is simply not measurable through
24% of noise, and a result that changes sign with arm length is not a result.

**NULL ARM — the session's noise floor, measured, not assumed** (§3). The
SAME binary against ITSELF, 21 pairs, ~7 s arms, identical method:

```
A: median 7046.9 ms  min 5703.1 ms
B: median 6828.1 ms  min 5875.0 ms
ratio of medians 0.9690 | ratio of mins 1.0301 | 11/21 | z = 0.22
```

Identical code measured **3.1% "faster" by median and 3.0% "slower" by min —
the two statistics disagree in SIGN on a null comparison.** That is the
resolution limit of this machine, and it retires the earlier readings on the
spot: a floor of ±3% cannot adjudicate a 5-10% claim whose own two runs
disagree by 16 percentage points (0.908 vs 1.069). The between-run conditions
moved, not the code. Every number in the table above is hereby marked
inadmissible; the null arm is why we run it before believing anything.

**Decision: KEEP the bricks, label them below instrument resolution.** This is
the §15 rule, not a rationalisation: for effects the clock cannot resolve, the
deterministic evidence is primary and the clock is confirmatory.
The deterministic evidence here is strong and independent of this box:
- `thread_id` measured 1.41 ns vs 0.25 ns cached, **reproduced to within
  0.02 ns on Windows AND Linux** — a mechanism, not a reading.
- Bricks #2 and #3 remove work by construction: a mask, a shift and two loads
  (brick #2) and three loads plus a two-arm match (brick #3) per free, with
  **byte-identical counters** proving the same work is performed either way.
- Nothing was added; the Page struct did not grow.

## P0 — 0.3.1 SEGFAULTED: a dying thread adopted orphans (2026-08-06)

**I shipped a segfault.** FFAI bisected it in one pass: 0.3.0 clean 0/8,
**0.3.1 crashing 6/8**, identical build and workload, allocator version the only
variable. Reproducible with **five JPEG decodes**, single-threaded.

### Cause — my one functional change in 0.3.1

`collect(force)` began reclaiming abandoned segments. But **both teardown paths
call a forced collect**:

```
init.rs:509  thread_done  -> h.collect(true)
init.rs:597  heap_delete  -> h.collect(true)
```

So a thread on its way out adopted every orphan from previously-dead threads
into **the heap it was about to destroy**, re-homing their pages onto a
`DelayedList` freed moments later. Use-after-free.

### Their data identified it before I read a line of code

The decisive detail: **more threads made it LESS frequent** — 1 thread 5/6,
2→3/6, 4→4/6, 16→1/6. Backwards for a race, exactly right for "a dying thread
swallows the orphan pool": with more live heaps, orphans get adopted by a
LIVING thread first. Everything else follows — every crash needs a thread exit
(60 detects on a preloaded image: 0/6), PNG is clean because of its thread
lifecycle not its allocation shape, and buffer size is irrelevant (640x480 and
1920x1080 both 6/6).

### Fix (0.3.2)

`collect_for_teardown()` — forced collect that does **not** reclaim. Both
teardown sites use it; `mi_collect(true)` on a LIVE heap still reclaims.

### The regression test DOES NOT reproduce it — do not trust it yet

`tests/teardown_reclaim.rs` passes 4/4 **with the bug deliberately
reintroduced**. It is not a guard, and it is labelled as such in its own header.
The missing ingredient is almost certainly CROSS-THREAD frees: every thread in
it frees its own blocks, so nothing remote is pushing onto the dying heap's
`DelayedList` — which is what makes a re-homed `xheap` a live target. **The fix
is reasoned and matches every observation, but it is not test-proven.**
FFAI's repro is the only thing that has reproduced this.

### Process failures

1. **A caret requirement made this automatic.** FFAI had
   `rusty_alloc = "0.3.0"`, which silently resolved to 0.3.1 on publish,
   mid-session, under an already-validated lockfile. They shipped a
   segfaulting default for several commits without touching a version string.
2. **Nothing gated `collect(force)`.** Not one test called it, which is exactly
   why the earlier `_force`-is-ignored bug survived to be found by a reader
   rather than a gate. I fixed the ignored parameter and added no test for it.
3. **The teardown call sites were never checked.** I searched for who called
   `collect` when diagnosing FFAI's report, saw both teardown paths, and still
   changed the shared implementation.

**Recommendation: YANK 0.3.1.**

## collect(force) ACTUALLY DOES SOMETHING — reported by FFAI (2026-08-06)

FFAI reported two things against 0.3.0. Both were real.

**1. Stale M2 comments.** `rusty_alloc_api/src/lib.rs:129` said
*"(M2: one global locked heap)"* and `heap.rs:3` said *"M2: ONE global heap
behind a lock"* — describing an architecture removed in M4. Anyone reading
either would conclude `free` serialises on a global lock. Corrected.

**2. `collect` ignored `force` — the signature was literally `_force: bool`.**
So `mi_collect(true)` was a per-heap page sweep and nothing else: it never
reclaimed an abandoned segment. That is a large part of why a caller's "trim"
measures ~0% — there was nothing in the forced path to reclaim WITH. `force`
now adopts every orphan first, so the bin sweep retires their dead pages in the
same pass.

### The purge half CRASHED and was reverted

A forced collect should also return pages to the OS, so the first version
purged every free span. **It crashed the test suite with an access violation
(0xC0000005).** Cause: `span_free` purges only spans of
`len >= MEDIUM_PAGE_SLICES`, so purging smaller ones reaches spans whose reuse
path does not re-commit them — the M8 defect exactly (Windows `MEM_DECOMMIT`
faults on touch; Linux `MADV_DONTNEED` does not, which is why this class keeps
being Windows-first). Reverted. A forced purge needs the recommit path audited
before it can ship.

Kept: reclaim-on-force. Gates green — Windows tests + clippy, Linux GATE
PASSED, churn 3/3, wasm, and speed unchanged (lua 0.9883, perl 1.0060,
sqlite 1.0037).

## RSS TAIL — abandoned segments; two fixes, IMPLEMENTED not yet VALIDATED (2026-08-06)

**FFAI, N=5, one program, trim the only variable:**

| arm | RSS med | RSS min | RSS max | latency |
|---|---:|---:|---:|---:|
| mimalloc | 111.1 | 106.7 | 134.2 | 31.97 ms |
| rusty no-trim | 195.8 | **92.2** | **403.3** | 30.63 ms |
| rusty trim200 | 195.8 | 91.2 | 402.5 | 30.49 ms |

**The finding is the SPREAD, not the median.** Our MINIMUM (92.2) beats
mimalloc's (106.7); our max is 3x theirs; spread 4.4x against their 1.26x. A
retention-policy difference shifts a distribution — it does not stretch one. And
92->403 MB is roughly ten 32 MiB segments, so something timing-dependent decides
how many the process holds. Latency is FINE (we are 1.4 ms faster).

Trim reclaiming 0.1 MB (0%) is not a sampling artifact: **trim walks a heap's
own free spans and cannot see the abandoned list at all**, so it would read 0%
at any N.

### Localised

`tests/abandon_rss.rs`: 8 waves x 8 threads that allocate spans and exit.
**25 segments abandoned; a following 2048-block allocation burst adopted 4.**
At 32 MiB each that is the tail, and whether anything adopts is pure
scheduling — exactly the shape of a 4.4x run-to-run spread.

### Fix 1 — purge on abandon (`abandoned_page_purge`, now default ON)

The option existed **in the table by name only, defaulted to 0, and nothing
read it.** So an orphan kept every page it had ever touched, resident,
indefinitely. `segment::purge_free_spans` now decommits a segment's free spans
at the last instant the dying thread still owns it. Deliberately NOT gated on
`purge_delay`: that governs a LIVE heap's spans, which are likely to be reused
shortly, whereas an orphan has no owner to reuse anything.

### Fix 2 — adopt until satisfied, not twice

`span_from_segments` capped adoption at `tries < 2`, then took a fresh 32 MiB
segment while orphans sat unclaimed. Now each adopted segment is tried
immediately and the loop runs until the request is met or the list is empty
(cap 32, a stall guard rather than a reclaim budget).

### HONEST STATUS: implemented and correctness-gated, NOT validated for RSS

Neither fix is shown to reduce RSS yet, because **no probe here reproduces the
shape**. The count probe measures orphans, not bytes; `bench/churn.c` has a
4.9 MiB working set (where we already use 7x LESS than mimalloc); the
long-lived-thread sweeps never abandon anything. Fix 2 also did not move the
orphan count, and the reason is instructive: **adoption only triggers on a
segment MISS**, so a thread with room in its own heap never reaches the
abandoned list however generous the cap.

Speed is unaffected — lua 0.9908, perl 1.0060, sqlite 1.0037, unchanged, and
neither fix touches the fast path.

**Validation must come from FFAI's workload**, which is the only thing that has
reproduced the tail. The A/B is free: `MIMALLOC_ABANDONED_PAGE_PURGE=0` vs `1`
on the same binary, N=5, comparing max and spread rather than median.

## RSS INVESTIGATION — mechanism confirmed, our retention NOT implicated (2026-08-06)

**External measurement (FFAI/Diana, their harness, their null arm):** speed at
exact parity (1.015x wall, CPU 1.000x), **peak RSS 91.4 MB vs mimalloc's 77.5 MB
= +17.9%**, null arm 77.4 so the gap is real. First independent numbers the
project has ever had, and they land on the gap the audit called most likely to
surprise us — we have no RSS gate at all.

### The instrument failure that came first

`bench/rss.sh` initially had NO NULL ARM. It returned 62.6, 62.8 and **51.6
MiB for the same binary** — an 11 MiB swing that silently invalidated three
conclusions drawn from it. Cause: **perl randomises its hash seed per process**,
so every run allocated a different pattern. Pinning `PERL_HASH_SEED=0` collapsed
the spread to **0.2 MiB**.

**Everything measured before that fix was retracted**, including a confident
"purging is the cause" claim. The rule this project already applies to time
applies to memory: *no null arm, no result.*

### What is now established

| probe | result |
|---|---|
| single thread, perl cycling | rusty_alloc **51.8** vs mimalloc 52.9 — we are BETTER |
| thread sweep 1 -> 28 | RSS scales LINEARLY with threads for both; at 28, ours **122.8** vs **228.2 MiB** |
| size sweep 32 KiB -> 4 MiB | −44% at every size |
| alignment 0 / 32 / 64 / 4096 B | −44% at every alignment |

**Per-thread heap retention IS the dominant RSS term** — confirmed
independently, and it matches FFAI's `scaling.rs` finding (28 heaps retaining
174 MiB against 26.3 MiB live). But **on every synthetic form of that mechanism
we retain roughly HALF of mimalloc**, so our retention is not yet implicated in
their +17.9%.

### Not reproduced, and what is left

Nothing synthetic reproduced the gap. Untested, in order of suspicion:
1. **Mixed lifetimes** — model weights held for the session while activations
   cycle. Every probe here frees everything each round, so none of them
   exercises fragmentation.
2. **Thread lifecycle churn** — probes use long-lived concurrent threads;
   candle's pool may create/destroy, routing through abandonment/adoption.
3. **Rust `dealloc` passing a `Layout`** (size AND align) where C `free` passes
   only a pointer.

**Next step is not more guessing:** run FFAI's own `scaling.rs` with
rusty_alloc as the arm. It already produced the 174-vs-26.3 number, so it
measures per-thread retention under Diana's real behaviour — exactly what these
probes failed to synthesise.

### Also refuted here (both on the FIXED instrument)

- **Purging** (`purge_delay: -1`, off by default) recovers ~2 MiB of ~10
  single-threaded and **nothing** multi-threaded (122.8 vs 122.4). It is worth
  enabling but it is not the gap.
- **Deferred retire** — keeping emptied pages queued so the next round reuses
  the same memory instead of first-fitting elsewhere. Span re-carve churn is
  REAL (504 pages retired and re-carved per round, 3,050 carves for a ~530-page
  working set, segment count flat) but changing it moved RSS not at all.
  Reverted.

New probes, all reproducible: `bench/rss.sh` (null arm + pinned seed),
`bench/rss-threads.{c,sh}`, `bench/rss-sizes.sh`,
`crates/rusty_alloc/tests/rss_churn.rs`.

## P0 FIXED in 0.1.0-alpha.2 — use-after-free race (2026-08-06)

**Fix:** `wait_no_remote_in_flight(seg)` — spin until no page of the segment
has `XFLAG_FREEING` set — called on **every** path by which memory can reach an
arena.

The first attempt guarded only `segment_free` and **Miri still failed,
identically**. That refutation was the useful part: it proved the racing path
was elsewhere. `huge_free` recycles a huge segment through `chunk_free_n`
WITHOUT passing through `segment_free`, so guarding one choke point left the
real hole open. Both are guarded now.

Why a barrier is sufficient rather than an epoch scheme: before a remote sets
FREEING it has not yet pushed to the delayed list, so the owner cannot have
drained it, so `used > 0` and no retire is possible. Every dangerous instant
therefore has FREEING observably set.

Verified: `cargo +nightly miri test -p rusty_alloc` (isolation ON, the whole
target) exits 0 — `stress_mt::abandon_adopt_reuse_storm` included, which is the
test that caught it.

**LESSON — the one that matters most here:** the audit's `corpus/miri-gate.sh`
ENUMERATED suites (`alloc_core spans heaps secure prim`) and therefore silently
omitted `stress_mt`, the only multi-threaded one. It then recorded "Miri clean"
on that basis. CI ran the whole target and found a use-after-free on the first
green-field run. **Never let a gate enumerate what it should sweep.**

### Original diagnosis (kept for the record)


**Found by CI, minutes after publishing 0.1.0-alpha.1.** Miri's data-race
detector on `stress_mt::abandon_adopt_reuse_storm`:

```
Undefined Behavior: Data race detected between
  (1) atomic store        page.rs:430   thread `abandon_adopt_reuse_storm`
  (2) non-atomic write    segment.rs:535 thread `unnamed-8`
  at alloc57912+0x8000060
```

- **(2)** is `huge_alloc` scrubbing a recycled arena chunk:
  `write_bytes(seg, 0, size_of::<Segment>())` — which zeroes the whole header,
  including every page slot's `xthread_free` atomic.
- **(1)** is `remote_free`'s restore-DELAYED loop doing a
  `compare_exchange_weak` on `(*page).xthread_free` — **a page inside that very
  segment**.

So a segment was released, recycled through the arena, and re-tenanted as a
huge allocation **while another thread was still mid-`remote_free` on one of
its pages**. That is a use-after-free, and the write that lands on it is a
`memset` of the whole header.

This is the same FAMILY as the M8 P0 (guard pages recycled while still
PROT_NONE): a segment reaching an arena while something still references it.
The four-state protocol has FREEING precisely to stop teardown racing a remote
free — `page_set_flag` spins it out — so the gap is a teardown path that
reaches `segment_free` WITHOUT passing that gate. Not yet localised.

### Why the gates missed it

Miri was never run against `stress_mt`. The audit added `corpus/miri-gate.sh`
with the suite list `alloc_core spans heaps secure prim` — **the multi-threaded
suite was not in it**, and the audit entry even recorded "Miri clean" on that
basis. CI runs `cargo +nightly miri test -p rusty_alloc`, which runs
*everything*, and caught it on the first green-field run. The lesson is exact:
**a Miri gate that enumerates suites will silently omit the one that matters;
run the whole target.**

### Status

- **`0.1.0-alpha.1` is published on crates.io with this defect.**
  Recommendation: **yank** (`cargo yank --version 0.1.0-alpha.1`) for both
  `rusty_alloc` and `rusty_alloc-api`. Yanking blocks new dependents while
  leaving existing builds working; it is reversible.
- Reproduce: `cargo +nightly miri test -p rusty_alloc --test stress_mt`
  (isolation ON, i.e. no `-Zmiri-disable-isolation`).
- Blast radius: multithreaded programs that abandon threads AND allocate huge
  blocks. Single-threaded use is unaffected.
- Fix will need the loom model that built the protocol, not a point patch.

### CI fixes landed alongside (all real, all pre-existing)

1. **Clippy never ran on Linux.** `c_long` is `i64` on LP64 unix and `i32` on
   Windows, so four `as c_long` casts are "unnecessary" on Linux and
   load-bearing on Windows. `corpus/linux-gates.sh` now runs clippy too —
   running it only on Windows was a genuine hole.
2. **`double_free.rs` cannot run under Miri** — `current_exe()` needs
   `readlink`, blocked by isolation, and Miri cannot spawn the child anyway.
   Now `#[cfg_attr(miri, ignore)]`.
3. **Stale oracle path** in `ci.yml` (`out/mi` vs the OS-namespaced
   `out/linux/mi`), plus a wasm job that executes rather than only compiles.

## RELEASE PREP — 0.1.0-alpha.1 (2026-08-06)

### Double free: silent corruption -> clean abort

The known limitation recorded in the audit is fixed. `page_push_local` did
`(*page).used -= 1` with no guard, so freeing a block twice wrapped `used` to
`u32::MAX`: the page never retired and the same block sat on the free list
twice, so a later pair of `malloc` calls handed the SAME memory to two owners.
Release builds accepted this silently; upstream mimalloc does too.

Now detected via a sign test on the post-decrement value — legitimate `used` is
always far below `i32::MAX`, so a negative reading can only be the wrap — and
the process aborts. Proven by `tests/double_free.rs`, which re-executes its own
binary in a child so the abort can be observed rather than assumed.

**It costs real performance and that was the deliberate call:**

| | perl | sqlite |
|---|---:|---:|
| M16 (no detection) | 1.0021 | 1.0018 |
| **with detection (shipped)** | **1.0062** | **1.0037** |

~4 Ir per free. Two forms were tried; both cost the same, and the disassembly
confirms the ideal `dec eax; js` sequence — the cost is the load/store around
it, because reading `used` back after the store prevents LLVM from keeping the
whole thing as a single `dec [mem]`. Kept anyway: an allocator whose premise is
memory safety should not hand the same block to two owners to save 0.4%.

### Carved out for release

- **`publish = false`** on `_ffi`, `_override`, `_bench`, `_wasm` — harnesses,
  fixtures and native artifacts, not libraries. Only `rusty_alloc` and
  `rusty_alloc_api` go to crates.io.
- **LICENSE added** (MIT), naming the dev-only vendored trees explicitly:
  `oracle/mimalloc` and `corpus/mimalloc-bench` are outside every published
  package directory and never ship.
- **Per-crate READMEs** for the two published crates; `cargo package` verified
  at 32 files / 311.9 KiB — no oracle, no corpus, no target.
- **A stray zero-byte file named U+F03A** (an unprintable private-use
  character) was sitting at the repo root, an artifact of an earlier shell
  redirect. Removed before it could be committed.
- Version set to **`0.1.0-alpha.1`**, with the reason in the manifest: the
  allocator is done, the evidence is not.

### Wall-clock: the instrument, and what it can honestly say

`bench/wallclock.sh` — pinned, ABBA-interleaved, N=31, medians AND minima, with
a **null arm** (the same allocator against itself) as the floor.

The first version was WRONG and said so loudly: every `min` came back 0.0 ms,
because `/usr/bin/time` reports at 10 ms granularity — on a 300 ms workload
that is ~3%, coarser than the ~0.5% effect being measured. Replaced with
microsecond `EPOCHREALTIME` and workloads scaled to >1 s so per-run fixed costs
fall below the noise. (A SECOND harness bug survived that: `min` was still
0.0 ms because the accumulators start empty and grow with `" $x"`, so the list
had a leading space that `sort -g` ranked as zero. `awk NF` now drops it —
a whole statistic had been silently dead across two consecutive runs.)

**The result, N=31, pinned, microsecond timer:**

| arm | median ratio |
|---|---:|
| **null (rusty_alloc vs ITSELF)** | **1.0117** |
| perl, ra vs mi | 1.0009 |
| sqlite, ra vs mi | 1.0091 |

**The null arm is 1.17% — wider than either effect.** The same allocator
compared against itself differs by more than the difference we are trying to
detect. The only conclusion this instrument supports is *"at parity, below
measurement resolution"*, and that is what the README says. The wall-clock debt
carried since M9 is now paid in the only currency available: we ran it, and it
says the question cannot be answered on this machine.

### Publish order

`cargo package -p rusty_alloc` succeeds (32 files, 312.6 KiB — no oracle, no
corpus, no target). `rusty_alloc_api` fails with *"no matching package named
`rusty_alloc`"* until the core is actually on crates.io — expected, not a
defect. **Publish `rusty_alloc` first, then `rusty_alloc_api`.**

## M16 — the prologue is GONE (fourth attempt) (2026-08-06)

`free` opened with `push r15; push r14; push rbx` and closed with the pops —
six instructions of callee-saved traffic on a fast path that uses none of it.
Three previous attempts failed:

1. `#[inline]` + 5-arg cold split — worse.
2. 5-arg cold split alone — worse.
3. `#[cold]` on `retire_emptied` — worse, and it ADDED back a push.

All three attacked the same thing: where the code lives. **The fourth attacked
what stays LIVE.** Registers get saved because a value must survive a call, and
the only call on the fast path is the retire branch — which took
`(seg, pg)`. `seg` is dead the moment `page_of` finishes; it was being kept
alive across the entire fast path purely to serve a branch taken on 1.6% of
frees. Passing only `pg` and re-deriving `seg` inside the cold function (one
mask) lets it die immediately:

```
- 25b20: push r15 / push r14 / push rbx ...
+ 25c60: test rdi,rdi          <- no prologue at all
```

The fast path now fits entirely in caller-saved registers.

| | perl | sqlite | lua |
|---|---:|---:|---:|
| after M15 | 1.0044 | 1.0029 | 0.9865 |
| **after M16** | **1.0021** | **1.0018** | **0.9841** |

**perl is 0.21% from parity, sqlite 0.18%** — and both now beat glibc
(0.822 / 0.995).

The transferable rule, which cost four attempts to learn: *to remove a
prologue, shorten LIVE RANGES, not function bodies.* Splitting code out does
nothing if the split still threads hot values through its signature — and both
M13 and M16 landed only once the cold function's argument list was cut to the
single value it could not re-derive.

Gates: Windows tests exit 0, clippy `-D warnings` exit 0, Linux GATE PASSED
(23 suites), churn 5/5 clean, **Miri clean** — `alloc_core` 11 passed, `spans`
and `heaps` 1 each, zero UB and zero leaks. That run specifically clears the
new `segment_of(pg.cast())` derivation: `pg` points into `(*seg).pages`, so
masking it back to `seg` stays inside the same allocation and `with_addr`
preserves the provenance.

## M15 — the empty-page sentinel: one branch instead of two (2026-08-06)

Worked the four sized levers. The **malloc side** paid, twice.

M14's re-split showed the deficit had gone even (free +5.1 Ir/op, malloc +5.1)
while every recent brick had targeted free. Reading our malloc fast path
against upstream's found a structural difference:

```rust
let p = self.direct[w];
if !p.is_null() {            // <- upstream has NO such test
    let b = page_pop(p);
    if !b.is_null() { ... }
}
```

Two tests — "is there a page?" then "did it yield a block?" — where mimalloc
has one. Its `pages_free_direct` slots never hold null; an empty slot points at
a shared **empty page** (`_mi_page_empty`) whose free list is permanently null,
so popping from it returns null and falls through to the generic path exactly
as an exhausted real page does. **The two questions collapse into one.**

Ported as `Page::empty_sentinel()` + `page::EMPTY_PAGE`, published by
`update_direct` whenever a bin's queue is empty. `block_size`/`slice_count` are
1-ish rather than 0 only so the `debug_checks` validator accepts it. Sound as a
shared immortal `static` because `page_pop` returns BEFORE its first store when
`free` is null — nothing ever writes it.

`heap.rs:malloc` fell 6.03 -> **4.03** Ir/op, exactly the two deleted
instructions.

| | perl | sqlite | lua |
|---|---:|---:|---:|
| after M14 | 1.0060 | 1.0037 | 0.9883 |
| **after M15** | **1.0044** | **1.0029** | 0.9865 |

**sqlite is now 0.29% from parity — about 920K instructions of 317.8M.**

### Also retested and refuted (third time)

`#[inline]` on `alloc::free`, retested because M13 moved the general path out of
line and the body is now much smaller — the exact condition that was blamed the
first two times. Still a loss: perl 1.0044 -> 1.0055, sqlite 1.0029 -> 1.0033.
**Three attempts, three refutations; treat it as settled** and do not try a
fourth time without a genuinely new mechanism.

### Lever status after this pass

| lever | before | now |
|---|---|---|
| malloc-side deficit | +5.1 Ir/op | **+2.1** (M14 + M15) |
| free-side remainder | +5.1 Ir/op | +5.1 — now the larger half |
| `Page` 80->64 | ~1 Ir/op | confirmed ~1: the follow-back multiply is already gone, only the forward `idx*80` lea-chain remains. Large refactor, small prize — deprioritised. |
| aligned fast path (P2) | +21 Ir/op | unchanged; rare in the verdict workloads, ~0 whole-program |

Gates: Windows tests exit 0, clippy `-D warnings` exit 0, Linux GATE PASSED
(23 suites), churn 3/3 clean.

## M14 — the heap pointer in TWO instructions, not four (2026-08-06)

Post-M13 the remaining gap was split EVENLY — free +5.1 Ir/op, malloc +5.1 —
and malloc had barely been examined. The breakdown named the culprit
immediately: `init.rs:malloc` cost **4.00 Ir/op**, purely locating the heap.

M10c's TLS slot resolved the address the obvious way: load the offset from the
GOT, read the thread pointer from `fs:0`, add, dereference. Four instructions.
But **x86 does that addition in the addressing mode** — `fs:[reg]` is a
segment-relative load, so the explicit `fs:0` read and the add both vanish:

```
  mov {t}, qword ptr [rip + __ra_tls_heap@GOTTPOFF]
  mov {o}, qword ptr fs:[{t}]
```

M10c chose the four-instruction form deliberately, to keep the address
computation `pure` and CSE-able. That reasoning was wrong in practice: the
profile shows `heap_box` is called once per malloc, so there was never anything
to CSE — the optimisation paid for a benefit that could not occur.

| | perl | sqlite | lua |
|---|---:|---:|---:|
| before | 1.0067 | 1.0041 | 0.9917 |
| **after** | **1.0060** | **1.0037** | **0.9883** |

Gates: Windows tests exit 0, clippy `-D warnings` exit 0, Linux GATE PASSED
(23 suites), churn 3/3 clean.

## M13 — the six prologue instructions: a 1-ARG cold split (2026-08-06)

M12 left six instructions of callee-saved traffic (`push r15/r14/rbx` + pops)
at the top of `free`, on a fast path that needs none of it. A cold-split had
ALREADY been tried and measured worse, so the question was whether the idea was
wrong or the implementation was.

**It was the implementation, and the signature was the whole difference.** The
failed attempt threaded all five already-computed values
(`seg, pg, p, flags, local`) into the cold function — putting five registers of
argument setup ON THE HOT PATH to serve 1.6% of frees. The version that works
passes **only `p`**, which is already in the argument register, and re-derives
segment/page/flags inside. The ~10 instructions of re-derivation are paid on
1.6% of frees; the hot path pays nothing.

| | perl | sqlite | batch_lifo |
|---|---:|---:|---:|
| before | 1.0077 | 1.0045 | 70.98 (+11.28) |
| 5-arg split (earlier) | 1.0106 | 1.0060 | — |
| **1-arg split (kept)** | **1.0067** | **1.0041** | **69.98 (+10.28)** |

`push r15` is gone from the prologue (the remaining third push is stack
alignment, not a register save). `batch_lifo` is down to 1.172x from 1.256x
two bricks ago.

**Also tried and REVERTED: `#[cold]` on `Heap::retire_emptied`.** The theory was
sound — it is `#[inline]`, it runs only when a page empties, and its tree
(`retire_span` -> `span_free` -> `segment_free`) is large, so it looked like the
reason the fast path was provisioned for so many registers. Measured: perl
1.0067 -> 1.0084, sqlite 1.0041 -> 1.0048, **and `free`'s prologue gained back
the `push r15` the change was meant to remove.** Reverted; baseline restored
bit-exactly (sqlite 318,176,956).

That is three attempts at these six instructions: two refuted, one kept. The
transferable part is that "split the cold path out" is not one idea — its
signature decides whether the cost lands on the hot path or the cold one.

Gates: Windows tests exit 0, clippy `-D warnings` exit 0, Linux GATE PASSED
(23 suites), churn 3/3 clean.

## M12 — slice_offset in BYTES: the live-working-set brick lands (2026-08-06)

The previous entry located the biggest single item in our free path: `Page` is
80 bytes, not a power of two, so `page_of`'s span follow-back scaled a slice
count by 80 — `neg; lea; shl` before the subtract, 11.02 Ir/op of pointer
arithmetic, 32% of our whole free cost.

The obvious fix was to shrink `Page` 80 -> 64, which needs a POINTER to
disappear and is a large, risky refactor. **Reading upstream first found a much
cheaper route to the same instructions.** mimalloc's field is documented as
*"the `slice_offset` is the byte offset back to the first slice"* and
`mi_slice_first` is a plain byte subtract. Ours stored SLICES and paid the
scale on every free. Storing bytes deletes the multiply without touching the
struct's size at all.

Verified in the shipped artifact, not assumed — the follow-back went 7
instructions -> 4:

```
- lea rcx,[rsi+rax*1] ; movzx ; neg rax ; lea rax,[rax+rax*4] ; shl rax,0x4 ; lea rbx,[rcx+rax*1] ; movzx
+ lea rbx,[rsi+rax*1] ; movzx ; sub rbx,rax ; movzx
```

**Results — every workload improved, and sqlite now beats glibc too:**

| workload | before | after | vs glibc |
|---|---:|---:|---:|
| lua | 0.9926 | **0.9898** | 0.833 |
| perl | 1.0098 | **1.0077** | 0.827 |
| sqlite | 1.0056 | **1.0045** | **0.997** |
| batch_lifo | 73.98 (+14.28) | **70.98 (+11.28)** | — |
| mixed | 150.18 (−7.59) | **146.18 (−11.60)** | — |

3 Ir/op off `batch_lifo` — exactly the three deleted instructions. The
`batch` deficit is down from 1.256x to 1.189x.

The `slice_offset` range is now guarded by a const assert:
`(SLICES_PER_SEGMENT-1) * size_of::<Page>() <= u16::MAX` (40,880 of 65,535),
so a future `Page` growth fails the build rather than silently truncating.

Gates: Windows tests exit 0, Linux GATE PASSED (23 suites), churn 3/3 clean.

**Still open on this path:** the six instructions of callee-saved traffic
(`push r15/r14/rbx` + pops) at the top of `free`. Moving the general path out of
line to relieve it was tried and measured WORSE (see previous entry) — the
argument setup costs more than the saves. The `Page` 80->64 shrink also remains
available and is now worth less, since the follow-back multiply — its main
prize — is already gone.

## P1–P3 EXECUTED — three refutations, and the live-working-set answer (2026-08-06)

Worked the `docs/plans/opscan_v1.md` plan. **Net code change: none. Everything
proposed was refuted, and the refutations are the result.**

**P1 died on its own count, before a line was written.** Generic-path entries
per 100,000 allocations: **ours 1,566, mimalloc 1,562** (`batch_lifo`); on
`aligned`, 6,254 vs 6,250. We do NOT leave the fast path more often than
upstream, so the extend-policy change P1 proposed was wasted work that would
have traded RSS for nothing. This is the count-before-code rule paying for
itself for the second time this campaign.

**Two follow-on bricks measured worse and were reverted.** The count did show
our exported `free` making a real call into `alloc::free` (100,082 per 100,000)
where upstream's is one flat symbol, and `alloc::malloc` had `#[inline]` while
`alloc::free` did not — a tidy-looking asymmetry.

| | batch_lifo | perl | sqlite |
|---|---:|---:|---:|
| baseline | 73.98 | 1.0100 | 1.0056 |
| `#[inline]` + cold-split | 74.98 | 1.0105 | 1.0060 |
| cold-split ALONE | — | 1.0106 | 1.0060 |

The first brick changed TWO things at once; isolating the second run showed the
cold-split was the harmful half, which **refuted** the register-pressure theory
rather than leaving it plausible. Both reverted; baseline restored bit-exactly
(sqlite 1.0056).

**WHY WE LOSE ON A LIVE WORKING SET — answered.** It is a fast-path COST
problem, not a slow-path FREQUENCY one (the counts above prove the frequency is
identical). Per-operation on `batch_lifo`: our free **34.1** Ir/op vs 25.0, our
malloc **21.0** vs 16.9. The disassembly names the biggest single contributor:

**`Page` is 80 bytes, and 80 is not a power of two.** Slice indexing emits a
`lea`/`lea`/`shl` chain for `idx * 80` and again for the `slice_offset`
follow-back, measuring **11.02 Ir/op — 32% of our entire free cost**. Upstream
pads `mi_page_t` deliberately, commented *"improve page index calculation"*; we
never did.

**Next brick, sized but NOT built: shrink `Page` 80 -> 64 bytes.** 128 is
impossible (512 x 128 = the whole 64 KiB slice, no room for the segment header).
Cutting 16 is the difficulty: `block_size`->u32 (−4), `heap_tag`->i16 (−2),
`free_is_zero`+`purged` into the `flags` byte (−2) gets 8; the other 8 needs a
POINTER to go, and `next`/`prev` are load-bearing for cross-segment page queues.

**P2 `aligned`: mechanism found, not built.** The plan's guess (we lack a
natural-fit fast path) was WRONG — we have one, and both sides fast-path 93.75%.
The real cost is that ours proves alignment via `bins::good_size(size)` and then
`malloc(size)` recomputes the same bin, where upstream tests the actual next
free block with one AND. Real, but `posix_memalign` is rare in the verdict
workloads so whole-program value is ~0. **P3 `usable`:** 32 vs 30 Ir, correctly
last, not attempted.

Gates: Windows tests exit 0, clippy `-D warnings` exit 0, Linux GATE PASSED (23
suites), perl 1.0098 / sqlite 1.0056 / lua 0.9926 — unchanged.

## OPSCAN — per-operation scan vs mimalloc (2026-08-06)

Built and ran a side-by-side per-operation comparison. Full method, table and
ranked plan: **`docs/plans/opscan_v1.md`**. Two things belong in the ledger.

**A symbol-by-symbol diff is not possible against release mimalloc.** It inlines
the whole allocator into three symbols (`free`, `malloc`,
`mi_page_free_list_extend`). There is no `mi_free_block_local` or
`mi_segment_page_of` to line up against ours. So the scan compares
**operations**, via one C driver run under each allocator by `LD_PRELOAD`.

**Two of three estimators were disqualified, and the reasons are reusable.**
Per-object attribution under-counted us ~4x, because `callgrind_annotate`
ELIDES the `[object]` suffix on continuation lines — mimalloc's three fat
symbols each keep it, our cost is spread over many `file:function` lines that
lose it. Caught by a SIGN disagreement with the attribution-free estimator.
The repaired version then reported our allocator at 115.79 Ir/op on an op where
the whole process spends 82.37 — impossible, rejected on arithmetic alone.
**The admissible estimator is the one with no attribution step:**
`(Ir(2N) − Ir(N))/N` on process totals. Deltas exact; ratios diluted toward 1
by the constant caller overhead, so read the delta column.

**Result shape (ra−mi Ir/op, positive = we lose):**

| we lose | | we win | |
|---|---:|---|---:|
| aligned | +21.4 | huge | −52,517 |
| batch_fifo | +14.3 | realloc | −97.9 |
| batch_lifo | +14.3 | big / large | −51.0 |
| usable | +2.0 | med / small | −32.5 / −29.0 |

We win the simple ops (one block in flight) and lose the ops with a **live
working set**. That is exactly why perl sits at 1.0099 while a ping-pong
microbenchmark flatters us: **real programs look like `batch`/`mixed`, and the
microbenchmark where we look best is the least representative one.** `huge` is
a structural win — mimalloc pays mmap/munmap per 2 MiB cycle, our arena serves
from cache.

Plan ranked P1 `batch_*` (most representative), P2 `aligned`, P3 `usable`, each
with the COUNT that must be taken before any code changes. Not yet executed.

## WASM — we now run in a WebAssembly VM (2026-08-06)

Asked to validate wasm. **Starting point: we did not compile for wasm at all** —
`cargo check --target wasm32-unknown-unknown` gave 18 errors, because the prim
layer has arms for `windows`, `unix` and `miri`, and wasm is none of those.

**A correction to the competitive premise.** mimalloc ALREADY supports wasm: it
ships `src/prim/wasi/prim.c` built on `__builtin_wasm_memory_grow`, and its
readme lists WASM among supported platforms. So wasm is not a place we win by
default. The honest differentiator is narrower and still real: a **pure-Rust**
allocator needs no C toolchain, no emscripten, and targets
`wasm32-unknown-unknown` directly rather than only WASI.

### What was built

`crates/rusty_alloc/src/prim/wasm.rs` — one linear memory that only grows.
Every consequence is a genuine semantic difference, documented in the module:

- **`free` is a no-op.** Linear memory cannot shrink, so nothing returns to the
  host and our own segment/page caches become load-bearing rather than an
  optimisation. (Upstream documents the same for wasi.)
- **Alignment costs a ONE-TIME pad.** `memory.grow` yields 64 KiB alignment but
  a segment needs 32 MiB. We read the current end, grow `pad + size`, and
  return the aligned base — and because a 32 MiB-aligned 32 MiB block leaves
  the end 32 MiB-aligned, only the FIRST segment ever pays.
- **`protect` returns an error rather than succeeding.** wasm has no page
  protection, and a guard page that cannot trap would let a `secure` build
  claim a hardening it does not have.
- **No clock** (`clock_now` is a counter — purge *ordering* survives, duration
  does not) and **one thread** (constant id, static TLS table, destructors
  never fire because there is no thread exit).

### Two real defects the wasm build exposed

**1. A 32-bit arithmetic overflow.** `Random::next_usize` did `(hi << 32) | lo`
— a constant shift past the width when `usize` is 32 bits, which rustc rejects
outright. Now width-aware: two draws on 64-bit, one on 32-bit.

**2. The default arena cost 1 GiB of REAL memory.** `ensure_default_arena`
reserves 1 GiB, which on a native OS is a cheap *virtual* reservation committed
lazily. Wasm has no virtual reservation — `memory.grow` backs every byte
immediately — so the reservation was fully materialised before the first
`malloc` returned. And it bought nothing: wasm memory is never returned to the
host, so every segment is already permanently cached, which is exactly what the
arena was for. Now skipped on wasm via `DEFAULT_ARENA_PAYS`. Upstream reaches
for the same lever more mildly (`arena.c` divides the reserve by 4 "if virtual
reserve is not supported (for WASM for example)"); with grow-only memory,
skipping entirely is strictly better.

| | linear memory | selftest |
|---|---:|---:|
| with the default arena | 1056.06 MiB | 6.79 ms |
| **without (shipped)** | **64.00 MiB** | **1.75 ms** |

### How it is gated

`cargo test` cannot execute `wasm32-unknown-unknown`, so proof of EXECUTION
comes from `crates/rusty_alloc_wasm` — a cdylib exporting `ra_selftest` —
instantiated under Node by `bench/wasm-selftest.mjs`, driven by
`corpus/wasm-gate.ps1`. Ten checks with distinct failure codes: cross-bin
patterns verified only after ALL allocations (so overlapping live blocks are
caught rather than overwritten), `usable_size` floor, zalloc zeroing, realloc
prefix preservation across a moving growth, a 600 KB span, 200 rounds x 32
blocks of churn (the check that matters most on wasm, since unbounded growth is
the failure mode when page recycling breaks), and word alignment. **The same
self-test also runs natively under `cargo test`**, so any failure that is not
wasm-specific is caught by the ordinary gates instead of only by the runner.

### Honest limitations

- 64 MiB for a trivial workload is coarse. It is one 32 MiB segment plus the
  one-time alignment pad, and the 32 MiB segment granularity — inherited from
  the mask-based `segment_of` addressing — is simply large for wasm contexts
  where memory is the scarce resource. A wasm-tuned `SEGMENT_SIZE` is the
  obvious follow-up and is NOT done.
- `secure` guard pages are unavailable (no page protection), and wasm entropy
  is much weaker: no host RNG, a counter clock and a constant thread id leave
  the stack address and a global counter as the only varying seed inputs.
  Free-list encoding there is corruption detection, not exploit mitigation.
- Single-threaded only. The atomics+threads proposal would need the
  read-then-grow pair in `alloc` to take a lock, as upstream's wasi backend
  does around `sbrk`.
- Not benchmarked against mimalloc on wasm. Correctness is proven; **no
  performance claim is made.**

Gates: Windows all-features exit 0, clippy `-D warnings` exit 0, Linux GATE
PASSED (23 suites), WASM GATE PASSED. Native performance unchanged — lua
0.9930, perl 1.0099, sqlite 1.0056.

## AUDIT — loops, unsafe quarantining, and a gate that lied (2026-08-06)

A deliberate hunt for looping hazards and unsafe that no caller actually
quarantines. **The worst thing found was not in the allocator — it was in the
harness that certifies it.**

### Fixed

**1. `corpus/linux-gates.sh` reported success on a BROKEN BUILD.** It counted
`test result: ok` lines and grepped for `FAILED|panicked`. Compile errors print
neither word, so a build break yielded `failures: 0`. This is not hypothetical:
earlier the same day it printed `ok-suites: 1 / failures: 0` while the tree did
not compile, and that was briefly read as a pass. Now checks cargo's exit code,
fails on any test failure, and fails if the suite count collapses below 15.
**Verified by deliberately breaking the build** — it correctly reported
`GATE FAILED (cargo exit 101)` where the old script said `failures: 0`.

**2. The new `corpus/miri-gate.sh` shipped with the SAME bug, briefly.** Piping
`cargo miri` into `tail` makes the pipeline's status `tail`'s — always 0 — so
it printed "MIRI FAILED" and exited 0. Fixed with `${PIPESTATUS[0]}`. Worth
recording precisely because it shows the failure mode is easy to re-create the
moment you stop looking for it.

**3. `init::done_slot` could hang the whole process, forever.** The winner of
the `INIT` CAS is the ONLY thread that ever publishes `RAW`; every other thread
spins in a bare `loop` waiting for it. `TlsSlot::new(...).expect(...)` on the
winner therefore turned a rare resource failure into a permanent process-wide
hang — and panicking there also unwinds into C callers, which is why the
release profile is `panic=abort` in the first place. Replaced with an explicit
`std::process::abort()`, identical in debug and release.

**4. `Heap::free_fast` was dead code holding the only cross-checks.** M11
inlined its body into `alloc::free` and left the original behind with no
callers. Its two `debug_assert`s were the ONLY places verifying that the flags
byte agrees with independent representations — `HUGE_SEGMENT` vs the segment's
`kind` tag, `SINGLE_BLOCK` vs `bin == BIN_HUGE`. Since M9b routes the entire
free on that one byte, a desync would silently send a huge or unqueued span
down the binned path. Deleted the dead `pub unsafe fn` (less unsafe surface)
and moved both checks to the live decision point.

**5. Miri was in NO gate**, and is not installed in WSL — despite having caught
two real defects in this project (the M4 registry and M7 arena base). Added
`corpus/miri-gate.sh`; run on Windows nightly: `alloc_core` 11 passed, `spans`,
`heaps`, `secure`, `prim` all clean, zero UB and zero leaks.

### Audited and cleared, with the reasoning

- **`page_of`'s removed bounds check is sound.** All eight call sites derive
  `seg = segment_of(p)` immediately before the call, so
  `p.addr() - seg.addr() == p.addr() & (SEGMENT_SIZE-1)` and `idx < 512`
  **by construction**. The contract is discharged at every site.
- **The `cfg(debug_assertions)` counter gating cannot change behaviour.** A
  search for any comparison or branch reading a `stats` field returns nothing —
  the counters are write-only in the allocator.
- **The 4-state xthread loops terminate.** All are CAS-retry (lock-free) or a
  bounded spin on the short FREEING window; `page_set_flag` spinning out
  FREEING is the designed handshake, not a hazard.
- **The arena claim-and-verify loop terminates.** On conflict `idx = c + 1` can
  move BACKWARDS for n >= 3, but `run` resets to 0, so re-triggering requires
  rescanning n free chunks and each conflict consumed a competitor's claim.
  Theoretical livelock only under adversarial single-chunk churn.

### Known limitations, recorded not fixed (all at upstream parity)

- **`page_push_local` does `used -= 1` with no underflow guard.** A double free
  wraps `used` to `u32::MAX`, so the page never retires and corruption
  continues silently in release. Debug builds catch it — Rust's overflow check
  panics at the subtraction.
- **`page_of` trusts `slice_offset` read from the pointer's own segment.** A
  pointer that is not ours yields an arbitrary `slot.sub(off)`. Upstream's
  `mi_slice_first` has the identical shape and release mimalloc likewise does
  not validate. This is the failure mode behind the jemalloc/redis
  mixed-allocator crashes recorded in M10c.
- **MIRI BLIND SPOT, and it covers the newest unsafe code.** The x86-64 Linux
  inline-asm paths — `init::thread_id`'s `fs:0` read and `init::heap_tls`'s
  initial-exec slot — are `cfg(not(miri))`, so Miri exercises their
  `thread_local!` fallbacks instead. **The TLS fast path we actually ship has
  no Miri coverage at all**; its only gates are hardware ones
  (`bench/churn.sh`, the corpus sweep). Any future change there must be
  hardware-gated, not Miri-gated.

Gates after the fixes: Windows all-features exit 0, clippy `-D warnings` exit 0,
Linux GATE PASSED (21 suites, exit 0), Miri clean, churn 3/3. Release
performance unchanged — lua 0.9929, perl 1.0100, sqlite 1.0056 — the new
assertions are debug-only.

## M11 — the benchmark itself was unfair: MI_STAT (2026-08-05)

Asked for one more win on perl and sqlite. The profile said `free` runs
**600,567 times** on perl at **~35 Ir/call** against mimalloc's ~25, and that
the last structural difference from upstream's `mi_free_block_local` was that
ours touches the owning HEAP (xheap load -> `box_of_xheap` -> heap pointer, a
dependent load chain) while upstream's touches none — the PAGE owns
`local_free`.

**Two ceiling probes, and the second one refuted the first's explanation.**

| probe | perl | sqlite |
|---|---:|---:|
| baseline (heap chain + counter) | 1.0145 | 1.0079 |
| #1 drop chain AND counter | 1.0114 | 1.0064 |
| #2 keep counter via cheap TLS, chain only on retire | **1.0163** | **1.0087** |

Probe #2 came back WORSE than baseline. That inverted the diagnosis: the heap
chain was never the cost — in the baseline ONE resolution served both the
counter and the retire, so splitting it into a TLS read plus a later chain
*added* work. **The cost was the counter.**

Which led to the finding that matters more than the brick. Upstream:

```c
#if (MI_DEBUG>0)
#define MI_STAT 2
#else
#define MI_STAT 0     // <-- the release oracle has NO counters at all
#endif
```

Our counters were unconditional. So every ratio this campaign has published
measured a **counters-on rusty_alloc against a counters-off mimalloc**. The
change is therefore not only an optimisation, it is a correction to the
comparison: hot-path counters now live behind `#[cfg(debug_assertions)]`,
exactly upstream's rule, keyed off debug rather than a new feature flag so
there is no manifest plumbing and `cargo test` (a debug profile) keeps the
instrument that proves two binaries do identical work.

The free fast path is now push + decrement + one zero test, with the owning
heap resolved only when a page actually empties (`retire_emptied`).

**Result:**

| workload | before | after | vs glibc |
|---|---:|---:|---:|
| lua | 0.9978 | **0.9927** | 0.837 |
| perl | 1.0145 | **1.0101** | 0.829 |
| sqlite | 1.0079 | **1.0056** | 0.998 |

perl is under 1% for the first time; sqlite now also beats glibc (0.9983).

**The campaign, end to end:**

| workload | start | now | vs glibc |
|---|---:|---:|---:|
| lua | 1.0650 | **0.9927** | 0.837 |
| perl | 1.0703 | **1.0101** | 0.829 |
| sqlite | 1.0355 | **1.0056** | 0.998 |

Gates: Windows all-features exit 0, Linux 21 suites / 0 failures, clippy
`-D warnings` exit 0, plus `bench/churn.sh` 5/5 clean (640 threads).

### Two process failures worth more than the brick

1. **Never round-trip source through PowerShell.** `Get-Content -Raw` +
   `Set-Content -Encoding utf8` decoded the file as Windows-1252 and re-encoded
   it, turning every `§ → —` into mojibake across `heap.rs`. The identical trap
   is already recorded for `pinvs.ps1`; it applies to SOURCE too. Reversed with
   a CP1252 re-encode, but the rule is: use the editor, not a shell text
   round-trip.
2. **A global regex replace hit the definitions it was meant to feed.**
   Rewriting `self.stats.allocs += 1` -> `self.stat_alloc()` also rewrote the
   body of `stat_alloc` itself, producing infinite recursion — caught as a
   Windows stack overflow (`0xC00000FD`) and clippy's "function cannot return
   without recursing". Write the accessor AFTER the sweep, or exclude it.

## M10c — PARITY WITH MIMALLOC on lua; the TLS call is gone (2026-08-05)

The item M10b sized and declined to build, built — by a different design than
the one that was declined.

**What was rejected, and why the rejection was right.** A thread-pointer-keyed
side table: hash the TCB address into a global array of `(tp, heap)` pairs.
That is P0-class, because a TCB is recycled when a thread exits, so a stale
entry hands a NEW thread a DEAD thread's heap. Clearing it in `thread_done`
only helps if `thread_done` always runs — the exact assumption that produced
the M8 access violation.

**What was built.** A real ELF TLS symbol in `.tbss`, declared via
`global_asm!`, read with the **initial-exec** relocation:

```
mov {off}, qword ptr [rip + __ra_tls_heap@GOTTPOFF]   ; linker-resolved, pure
                                                       ; + readonly => CSE-able
slot = thread_id() + off                               ; thread_id() IS the fs base
```

Two instructions and a load, replacing a `call __tls_get_addr` into `ld.so`.
Verified in the shipped artifact, not assumed: `readelf -r` shows
`R_X86_64_TPOFF64` against `__ra_tls_heap` (the M10 lesson — measure the
artifact you ship).

**Why this design is sound where the keyed table is not.** The storage IS the
thread's own TLS block. Every thread receives a fresh block initialised from
the all-zero `.tbss` image at creation, so a recycled TCB cannot expose a dead
thread's heap — the staleness question does not arise. Initial-exec's cost is a
LOAD-TIME constraint (it needs a static-TLS slot, so a very late `dlopen` could
fail to load us), which fails loudly at load rather than corrupting memory. It
is the same trade upstream ships as
`__attribute__((tls_model("initial-exec")))`. x86-64 Linux only; every other
target keeps `thread_local!` (Windows TLS has no `__tls_get_addr` to remove).

**Result — we are at parity with mimalloc on lua:**

| workload | before | after | note |
|---|---:|---:|---|
| lua | 1.0198 | **0.9978** | 4 runs: 0.9954 / 0.9977 / 0.9979 / 1.0002 |
| perl | 1.0281 | **1.0145** | 4 runs, deterministic |
| sqlite | 1.0144 | **1.0079** | bit-identical across runs |

**The campaign, end to end:**

| workload | session start | now | gap closed | vs glibc |
|---|---:|---:|---:|---:|
| lua | 1.0650 | **0.9978** | **at/under parity** | 0.844 |
| perl | 1.0703 | **1.0145** | 79% | 0.832 |
| sqlite | 1.0355 | **1.0079** | 78% | 1.001 |

Gates: Windows all-features (exit 0), Linux 21 suites / 0 failures, clippy
`-D warnings`, fmt. Plus a brick-specific hazard probe — 640 threads
(40 waves x 16), each writing and verifying a thread-unique byte pattern
across 200 alloc/free rounds, x5 runs, zero corruption. That probe targets
precisely what a broken per-thread heap slot would produce.

### A correction to the real-world sweep record

The M8 note claimed all 10 OSS programs run correctly on us. **That over-claimed
on redis**, and this session's sweep exposed it. Measured, 8 startups per arm:

| preload | ok | crashed |
|---|---:|---:|
| none | 8 | 0 |
| **mimalloc** | **0** | **8** |
| rusty_alloc | 2 | 6 |

Cause: `redis-server` here is built against jemalloc (`mem_allocator:jemalloc-5.3.0`,
linked to `libjemalloc.so.2`) and reaches allocator symbols directly, so
LD_PRELOADing *any* replacement produces a mixed-allocator process — blocks
allocated by one and freed by the other. It is an unsupportable configuration
rather than a defect in either allocator, and the ORACLE fails it harder than
we do. Not attributable to this brick (the mimalloc arm contains none of our
code). The sweep should either drop redis or build it with
`MALLOC=libc`; leaving it in as a "pass" was the actual error.

Separately, `imagemagick` shows 4 distinct output hashes across 6 runs
**including system-vs-system**, i.e. its output is nondeterministic
independent of the allocator. The other 8 programs (jq, sqlite3, git, xz,
zstd, lua, perl, python3) agree byte-for-byte across all three arms.

## M10b — the gap is the FREE path; a third of it is now closed (2026-08-05)

With `__tls_get_addr` gone from the top, the per-function profile finally
allowed the decisive comparison — **our allocator against mimalloc's, function
by function, on the deterministic perl workload**:

| | mimalloc | rusty_alloc (before these bricks) |
|---|---:|---:|
| malloc side | 9.7 M | ~11.4 M (**already at parity**) |
| free side | **15.0 M** | **~41 M (2.7x)** |
| total allocator | 27.9 M (3.6%) | ~56 M (6.8%) |

That reframed the whole campaign: **our malloc was never the problem.** The
entire deficit lives in `free`, and two bricks came straight out of reading it.

**Brick #4 — `page_of` without the bounds check.** It resolves a block to its
page and is the allocator's hottest function (twice per free). It indexed
`[Page; 512]` with a runtime index, so LLVM emitted a bounds check it cannot
discharge — the bound is a property of the CALLER's contract (p lies inside a
32 MiB segment), not of the arithmetic. Replaced with `add`/`sub` on the base
pointer, same provenance, same address, invariant kept as a `debug_assert`.
This is the case `rusty-unsafe-optimizations` says to look for: not "sprinkle
`get_unchecked`", but *one* place where a provable invariant is invisible to
the compiler.

**Brick #5 — stop resolving the page TWICE per free.** `alloc::free` resolves
the page to route ownership, then handed only the SEGMENT to `free_local_at`,
which resolved the page again. M9 threaded the segment through and missed the
page. Threading it too deletes an entire `page_of` per free.

**Brick #6 — the flags byte was already there; nothing tested it.** `SLOW_FREE`
(`HAS_ALIGNED|SINGLE_BLOCK|IN_FULL|HUGE_SEGMENT`) had been defined in M9 and
never used. Meanwhile the free path re-derived, one load at a time, exactly
what those four bits already say: a `SegmentKind` match, a `bin == BIN_HUGE`
compare, an `IN_FULL` re-test, and an `unalign` guard. The bits are exhaustive
by construction — `SINGLE_BLOCK` is set at the same statement that sets
`bin = BIN_HUGE`, `HUGE_SEGMENT` at the same statement that builds a Huge
segment — so one test against the byte `alloc::free` had ALREADY loaded proves
all four. Clear byte routes to `Heap::free_fast`: push, decrement, one
empty-page test, and nothing else. This is upstream's
`page->flags.full_aligned == 0` shape, reached from our own side.

**Deterministic results (perl and sqlite are exact to 4-6 digits):**

| workload | after TLS brick | after #4 | after #5 | after #6 |
|---|---:|---:|---:|---:|
| lua | 1.0536 | 1.0477 | 1.0402 | **1.0198** |
| perl | 1.0602 | 1.0547 | 1.0476 | **1.0281** |
| sqlite | 1.0305 | 1.0278 | 1.0244 | **1.0144** |

**Session total — roughly two thirds of the gap to mimalloc, closed:**

| workload | start | now | gap closed | vs glibc |
|---|---:|---:|---:|---:|
| lua | 1.0650 | **1.0198** | **70%** | 0.860 |
| perl | 1.0703 | **1.0281** | **60%** | 0.844 |
| sqlite | 1.0355 | **1.0144** | **59%** | 1.007 |

Gates green throughout: Windows all-features, Linux 21 suites / 0 failures,
clippy `-D warnings`, fmt.

**Where the remaining 21.9 M instructions (perl) now sit.** The free path fell
from ~39 M to **23.4 M** against mimalloc's 15.0 M, and `free_local_at`
vanished from the profile entirely (inlined). Accounting for what is left:

| | ours | mimalloc | gap |
|---|---:|---:|---:|
| free path | 23.4 M | 15.0 M | 8.4 M |
| malloc path | 14.5 M | 12.8 M | 1.7 M |
| `__tls_get_addr` | 7.3 M | **0** | 7.3 M |
| | | | **17.4 M** (of 21.9 M measured) |

**This reprices the TLS item.** It was 0.89% of the program when the gap was
4.76%; the program cost has not changed but the gap has, so those same 7.3 M
instructions are now **a third of everything still separating us from
mimalloc** — the single largest named item left.

It is NOT built, deliberately. Stable Rust cannot select `initial-exec` for a
cdylib's `thread_local!`, and the alternative — a thread-pointer-keyed cache —
carries a P0-class hazard rather than a bug-class one: a TCB is recycled when a
thread exits, so a stale slot hands a NEW thread a dead thread's heap. Clearing
the slot in `thread_done` closes it only if `thread_done` always runs, which is
exactly the assumption the M8 P0 punished us for making. A 0.9% win does not
buy that risk. The honest options are a nightly-gated build flag or a design
that makes the stale entry detectable rather than merely unlikely.

## M10 — a REAL win on mimalloc's turf: the TLS model (2026-08-05)

Six-whys descent on "why are we 6.5% of instructions behind mimalloc on
small-object churn", using callgrind's PER-FUNCTION breakdown as a
deterministic stage profiler.

**D3 — which op?** The profile named it immediately, and it was not one of
ours: **`__tls_get_addr`, 12.97 M Ir (1.96% of the whole program)** — more
than half the cost of our entire `free` (14.6 M).

**D5 — the mechanism.** The shipping artifact is a **cdylib** (LD_PRELOAD).
Rust's `thread_local!` in a shared library compiles to the general-dynamic TLS
model, so **every access is a CALL into `ld.so`**. mimalloc's `_mi_thread_id()`
is one register read. We were paying a linker round-trip per free for a value
that lives in a register.

**D6 — and the instrument was lying to me.** The M9 probe measured TLS at
0.25 ns and I built on that. It measured TLS **inside an executable**, where
the model is local-exec — a register offset, no call. The artifact we ship is
a shared library. *Measure the artifact you ship*, not a convenient stand-in.
This is the third time in this project a probe measured the wrong context.

**Ceiling first, then cost** (`bench/tls-ceiling.sh`): rebuilt with
`-Z tls-model=initial-exec` → **2.00% of our instructions, 33% of the gap**.
That sized the prize before a line of the fix was written.

**The brick, on STABLE Rust:** read the thread pointer directly —
`fs:0` (x86-64 Linux), `gs:0x30` (x86-64 Windows), `tpidr_el0` (aarch64),
with the cached-TLS path kept for every other target. Exactly mimalloc's
mechanism. Soundness of id reuse is the same argument mimalloc relies on: a
dying thread abandons its segments (id stored as 0) before its TCB can be
recycled.

**RESULT — deterministic, reproducible, gap closed by a sixth:**

| workload | ra/mi before | ra/mi after | gap closed |
|---|---:|---:|---:|
| lua | 1.0650 | **1.0536** | 17% |
| perl | 1.0703 | **1.0602** | 14% |
| sqlite | 1.0355 | **1.0305** | 14% |

`__tls_get_addr` no longer appears in the profile's top entries at all.
Windows all-features green, Linux 21 suites / 0 failures, clippy + fmt.

**One brick tried and REVERTED (measured flat, not measured worse):** the
in-place `realloc` path bumps a counter, which costs a TLS heap lookup on the
commonest realloc outcome. Removing it left perl at 1.0602 and sqlite at
1.0305 — **unchanged to four digits** — because in-place reallocs are rare in
these workloads. It cost a work-parity counter for an unmeasurable gain, so it
went back. Recorded as *flat*, not *worse*.

**Instrument refinement:** lua's per-process hash-seed randomisation makes its
instruction count vary ~0.3% run to run; **perl and sqlite are deterministic
to 4-6 digits** (sqlite repeated to within 209 instructions in 326 M). Use
perl/sqlite for verdicts; treat lua as indicative.

**Standing:** ~5.4% of instructions behind mimalloc on small-object churn
(from 6.5%), ~11% AHEAD of glibc. The remaining TLS prize (~1%) is the heap
pointer itself, which needs either nightly's TLS-model flag or a
thread-pointer-keyed lookup — both are M11 candidates, both now sizeable
before they are built.

## M9c — the clock could not answer, so we stopped using the clock
## (2026-08-05)

The null arm proved this box cannot adjudicate a 5-10% effect. Rather than
wait for a quiet machine, we changed INSTRUMENT: **instructions retired
(callgrind)** — a counter, deterministic, indifferent to an open IDE, a
browser or thermal drift. Same program, same input, same output in every arm;
the allocator is the only variable. `bench/icount-arms.sh`.

**Instrument verified first** (three runs of the same arm):
ra 662.83 M / 662.78 M / 663.04 M — **0.04% spread**, versus the clock's 24%.
That is a usable instrument on a noisy box, and it is now the project's
default A/B for allocator work.

**The answer, finally free of noise:**

| workload | ra instructions | vs mimalloc | vs glibc |
|---|---:|---:|---:|
| lua (small-object churn) | 663.0 M | **1.065×** | **0.900×** |
| perl (hash/array churn) | 834.3 M | **1.070×** | **0.878×** |
| sqlite (bulk) | 328.1 M | 1.036× | 1.028× |

**We execute 6.5-7.0% more instructions than mimalloc on small-object
interpreter churn, and 3.6% more on sqlite — while executing 10-12% FEWER
than glibc on the same interpreters.** That is the shape the real-world sweep
hinted at, now quantified to four digits and reproducible on demand.

So the M9 story is complete and honest: the mechanism was real and is fixed
(the per-free OS call is gone), we are comfortably ahead of the system
allocator, and we remain **~7% of instructions behind mimalloc on exactly the
workload class that started this investigation.** That residual is the M10
target, and for the first time it can be attacked brick-by-brick with an
instrument that gives the same answer twice.

**Standing debt, narrowed:** a quiet-box wall-clock session is still owed
before any *time* ratio is published — but no longer to know whether a change
helps. Instruction count answers that today.

**What is still owed, and it is the same debt as M9:** a pinned session on a
QUIET machine (no IDE, no browser) at N >= 31 to convert "removes work" into a
standing speed number. Until that exists, rusty_alloc claims no speed ratio.

## M9 — WHY we lose on small-object churn: the mechanism, named and fixed
## (2026-08-05)

**The question:** the real-world sweep showed us winning on bulk workloads and
losing on small-object interpreter churn (lua, perl — same shape as cfrac).
Why?

**The answer, measured not guessed** (`rabench freepath-probe`, both OSes):

| component | Windows | Linux |
|---|---:|---:|
| loop floor | 0.24 ns | 0.22 ns |
| **`prim::thread_id()` — called on EVERY free** | **1.41 ns** | **1.39 ns** |
| const-init `thread_local` cache (candidate) | 0.25 ns | 0.23 ns |
| whole malloc+free pair, 48 B | 5.77 ns | 6.47 ns |

`free` must know the calling thread's id to route local-vs-remote. We were
calling the OS/libc every time — `pthread_self` through the PLT from a cdylib,
`GetCurrentThreadId` on Windows — for a value that never changes.
**That single call was 18–20% of an entire malloc+free pair**, and it lands
squarely on the workloads that do nothing but small alloc/free: interpreters.
The two platforms agreeing to within 0.02 ns is what makes this a mechanism
rather than a reading.

**Second finding, free of charge:** `alloc::free` resolves the segment and
page to route ownership, then `free_local` **recomputed both** — a mask, a
shift and two loads per free, for nothing.

**Bricks landed:**
1. `init::thread_id()` — const-init `thread_local` cache with a `#[cold]`
   first-call path; every hot site routed through it (~1.16 ns/free removed).
2. `free_local_at(seg, p)` — the already-resolved segment threaded through
   instead of recomputed. Byte-identical behaviour; strictly less work, which
   is the counter-style argument the clock cannot dispute.

**Gates:** Windows all-features green, Linux 21 suites / 0 failures, clippy
`-D warnings` + fmt clean.

**Performance verdict: NOT RESOLVED on this box, and I am not claiming one.**
Real-workload medians walked with N — lua ra/mi **0.751 at N=5 → 1.071 at
N=15**, perl 1.104 → 1.729 — the exact §16 failure mode (the estimator itself
trends; the reference's own throughput moved 25% between sessions). The
best-of-N floors are the only stable statistic here:

| workload | ra/mi (min-of-N) before | after |
|---|---:|---:|
| lua | 1.19× | **~1.00×** |
| sqlite | 0.84× | ~0.98× |
| perl | 1.05× | ~1.26× (contradicts the median direction) |

lua moving to parity is consistent with the mechanism; perl moving the wrong
way is not, and both arms slowed in absolute terms between sessions, so the
box — not the code — is the likely author of that number. **What is
defensible today: the mechanism is identified, quantified identically on two
platforms, and removed. The ratio needs a quiet machine at N ≥ 31 before it
goes in any README.**

**Next (M9 continued):** pinned quiet-box session for the standing ratio;
then the remaining fast-path candidates already visible in the probe —
the `SegmentKind` branch and `unalign`'s two loads on every free, and the
`generic`-path rate (6% of allocs) which sets how often we leave the hot path.

## M8b — P0 CLOSED + real-world validation sweep (2026-08-05)

**The P0 is fixed. Root cause: guard pages were recycled while still
`PROT_NONE`.** A guarded allocation protects the page after the object; when
that segment was released it went back to the **arena** with the protection
still applied, so the next tenant faulted on memory it legitimately owned.
Fix: lift protection (and restore commitment) before any segment can be
re-tenanted — `Segment::guarded`, handled in `huge_free`/`segment_free`.

**How it was found — the method, not luck.** Whole-suite runs faulted ~1/10,
every test passed alone, and my first hypothesis (abandon→adopt→arena churn)
was WRONG: a purpose-built MT storm (`tests/stress_mt.rs`, dying threads +
adopters + cross-thread frees + huge allocs) stayed clean over 10 runs. The
discriminator was **per-binary bisection**: only `tests/secure.rs` faulted
(1/6), yet each of its four tests passed alone 8/8 — so it was an INTERACTION.
That named the pair: the guarded-objects test creates PROT_NONE pages, and the
other tests recycle segments. The one earlier signal that had held all along —
"0/12 with arenas disabled" — then made sense: OS-released memory is unmapped,
so only the arena path resurrects a protected page. **Lesson (ledgered):
when a defect needs several tests to appear, bisect by BINARY and then by
PAIR; a clean single-test run is evidence of interaction, not of health.**

**Verification after the fix:** previously-faulting binary 12/12 clean;
`--all-features` 10/10 + 8/8; default 8/8; secure 12/12. Linux 21 suites/0.
clippy `-D warnings` + fmt clean.

**REAL-WORLD SWEEP — 10 open-source programs on rusty_alloc via LD_PRELOAD**
(`corpus/realworld.sh`, `corpus/realworld-medians.sh`): jq, sqlite3, python3,
git, xz, zstd, lua5.4, perl, ImageMagick, redis-server (+redis-benchmark).
- **Correctness: 10/10 ran, and every deterministic workload produced a
  BYTE-IDENTICAL output checksum under `ra`, `mi` and glibc.** No crash, no
  hang, no wrong answer. (ImageMagick's PNG bytes differ run-to-run under
  every arm — embedded timestamps, not a defect.) redis-server serves its full
  benchmark under our allocator: SET/GET/LPUSH/LRANGE_300 all complete.
  **This is the strongest correctness evidence the project has: real C
  programs, unmodified, on our allocator.**
- **Performance (medians of 5 ABBA-interleaved reps, WSL2 dev-loop numbers —
  NOT standing claims):** sqlite **0.90× of mi (we are ~10% faster)**;
  perl 1.21×; lua **1.66×** (min-of-N: 1.19×) — we are slower on the
  interpreter workloads. Median-vs-min disagreement is large on this box, so
  the ratio needs a quiet machine and N≥31 before anyone acts on it.
- **The pattern is consistent and actionable:** we win on
  large/bulk-allocation workloads (sqlite, malloc-large) and lag on
  **small-object-heavy interpreter churn** (lua, perl — and the same shape as
  the cfrac regression). That points at the single-threaded small-malloc fast
  path, exactly where M4 measured us at 0.93× of glibc's tcache. That is the
  M9 perf target, and it is now backed by real workloads rather than kernels.

**Still open:** cfrac regression un-diagnosed (needs a quiet re-run first);
the fast-path perf campaign; the v1 geomean gate.

## M8 — Hardening + purge; **v1 SIGN-OFF BLOCKED by an open defect** (2026-08-05)

**Landed:** `random.rs` — self-contained ChaCha8 CSPRNG, per-heap streams,
OS-seeded (BCryptGenRandom / /dev/urandom) with a documented fallback mix.
`secure` feature — **encrypted free lists** (`enc = (next + key2) ^ key1`, fresh
per-page keys, corrupt links caught by an alignment check on decode) routed
through `block_next`/`block_set_next` at every traversal.
**Guarded objects** — dedicated segment with a PROT_NONE trailing page, object
right-aligned against the guard so an overflow faults on the first byte past
it; sampling API (`mi_heap_guarded_set_sample_rate` / `_size_bound`) wired to
the option table. **Purge/decommit** of coalesced free spans (RSS lever) with
per-span `purged` state and recommit-on-reuse. **`debug_checks` implemented**
(our `dmi`): page-invariant and whole-segment span-tiling validators on the
hot paths.

**Defects found and FIXED during M8 (each real, each caught by a gate):**
1. **Purge without recommit** — Windows `MEM_DECOMMIT`'d spans were handed back
   out; the next touch faulted. Linux `MADV_DONTNEED` keeps pages accessible,
   which is exactly why this was Windows-only. Fixed: purge inside `span_free`
   after coalescing + `span_recommit` on reuse + full recommit before a segment
   returns to an arena (`Segment::purged_any`).
2. **Visitor read encoded links raw** — `visit_segment_blocks` walked free
   lists with plain `(*b).next`, so under `secure` it indexed a stack bitmap
   with garbage. Fixed (block_next + bounds-checked marking).
3. **Multi-chunk arena claim race** — `chunk_alloc_n` scanned for a free run
   then set the bits; the lock-free single-chunk path could steal one in
   between, giving TWO segments the same address. Fixed: claim-and-verify with
   rollback and rescan.
4. **`adopt_segment` mutated the span layout it was walking** — `span_free`
   coalesces, so the iteration could land mid-span and queue a bogus page.
   Fixed: never retire during the walk.

**OPEN DEFECT — v1 CANNOT SHIP (P0):** a rare access violation survives, in
the parallel test suite only. Measured after all four fixes: **1 in 10** runs
(`--all-features`), **1 in 5** (default), 0 in 12 with `MIMALLOC_DISALLOW_
ARENA_ALLOC=1`. Ruled OUT by experiment, not by argument: the secure
encoding (identity-encoding probe still crashed), purge (still crashes with
purging opt-in/off), option/env parsing (bypass probe still crashed), and
single-threaded execution (every test passes alone, 11/11). Not reproduced on
Linux (21 suites, 0 failures) — consistent with a Windows-only commit/protect
interaction OR with timing. Strongest remaining hypothesis: a segment is
returned to an arena (or reused) while another thread still reaches it —
i.e. `used_pages` accounting across abandon → adopt → `segment_free`.
Next probes: (a) make `debug_checks` assert `used_pages` against a live-page
recount at every segment transition; (b) an arena-chunk generation counter to
catch reuse-while-referenced; (c) rebuild the Windows suite under Application
Verifier / page-heap for an exact faulting address.
**Purging ships OPT-IN (`purge_delay` default −1)** — not because purge is the
cause (it isn't), but because it widens the state space while the defect is
open. Documented divergence from the oracle's default of 10.

**Gates:** clippy `-D warnings` + fmt clean; Linux 21 suites / 0 failures;
Windows all-features 5/5 clean in the last sweep but 1/5 AV in the default
build — **that is the blocker, and it is reported as such.**

**Tier-A corpus (WSL2, arm-interleaved, /usr/bin/time):** malloc-large **ra
2.50/3.04 s vs mi 5.02/3.95 — still ahead**; espresso ra 8.14/8.93 vs mi
9.11/6.60 (RSS **3456 KiB vs mi 10448**); larson wall parity (7.03–7.09 vs
7.06–7.13); cfrac **regressed to 8.65–12.87 s vs mi 6.24–6.32** — a real
M8-era regression on the small-alloc path, not yet diagnosed (the box was also
running the Windows stress concurrently, so this number needs a quiet re-run
before it is acted on: measure-first discipline, not a fix-first reflex).

**v1 gate status:** API parity ✅ (~150 of ~157 functions), corpus runs ✅,
hardening ✅, **stability ❌ (open P0)**, perf gate **not yet assessable** —
the geomean claim cannot be made while a corruption is open and cfrac is
unexplained. M8 is therefore NOT complete; the remaining work is the defect
hunt, then the perf campaign.

## M6+M7 — First-class heaps, arenas, subprocs; options, stats, hooks, the
## override crate and the Tier-A corpus as `ra` (2026-08-05)

**M6 landed:** first-class heaps as separately-allocated HeapBoxes with
**owner routing via the page's `xheap` back-pointer** (container-of over the
box's offset-0 delayed list — `free` now finds the OWNING heap, correct with
many heaps per thread); huge segments tracked per-heap and unified with the
delayed protocol (remote huge frees ride the DELAYED path; abandonment/adopt
cover Huge kind); `heap_new/_ex/_in_arena`, `delete` (segments absorbed into
the backing heap via adopt — blocks stay valid), `destroy` (wholesale release,
NEVER-spinning teardown), `set_default`/`get_backing`; heap_* alloc family
(FFI: ~45 heap exports); visitors (`mi_heap_visit_blocks` with free-bitmap
block enumeration, `mi_abandoned_visit_blocks` under the list lock),
contains/check_owned, page_under_utilized; **arenas v1** (segment-granular
chunk pools, used+dirty bitmaps, exclusive arenas, `manage_os_memory`,
`arena_area`, huge-page reserves as large-page arenas); **subprocs**
(per-subproc abandoned lists — isolation verified); page heap tags surviving
abandonment. Rust `Heap` type (delete-on-drop / destroy-on-drop).

**M7 landed:** the full 38-slot option table (ABI index-compatible,
`MIMALLOC_*`/`RUSTY_ALLOC_*` env parsing), registered hooks
(output/error/deferred-free — the heartbeat fires it), stats
(per-heap merged-on-read across the heap registry, process/thread prints,
`mi_process_info` via GetProcessTimes+K32/getrusage+statm), realpath/dupenv/
wcsdup/mbsdup, the C++ `mi_new` family (documented divergence: no
`std::get_new_handler`), **the override crate** (unix-only exports: malloc
family + posix + Itanium-mangled C++ operator new/delete incl. sized+aligned),
`include/rusty_mimalloc.h`, and Tier-A runner scripts.

**THE GATE THAT MATTERS — real C programs on our allocator via LD_PRELOAD**
(WSL2, /usr/bin/time, arm-interleaved, 2 runs/arm):
- cfrac: ra 5.95/8.63 s vs mi 7.47/7.15 vs glibc 6.32/9.06 — parity with the
  oracle; **RSS 3456 KiB vs mi 4312**.
- espresso: user-time parity (ra 6.39–6.68 vs mi 6.44–7.03); **RSS 3264 KiB vs
  mi 10448**.
- larson (real 100-thread-class bench, 8 workers): wall parity (ra 7.04/7.09
  vs mi 7.15/7.77); RSS ra ~90–100 MB vs mi ~77–83 MB (retention policy).
- **malloc-large found a REAL defect**: 3–4× slower than mi, sys-time-bound —
  large/huge alloc-free cycles round-tripped the OS. TWO fixes, both
  mimalloc's own shape: (1) lazily reserve the DEFAULT 1 GiB arena
  (`arena_reserve`) so segments recycle through chunks; (2) serve HUGE blocks
  from arenas too (contiguous multi-chunk claim under a small lock). Result:
  **ra 2.24/2.66 s vs mi 4.45/4.18/5.68 — flipped to ~1.8× FASTER than the
  oracle.** Sys time 15.7 s → 0.5 s. RSS +20% vs mi (recycled chunks stay
  committed — purge wiring is the RSS lever, still open).

**Miri earned its keep AGAIN, same law twice**: the arena stored its base as
`usize` — the 1 GiB region became unreachable-by-pointer and the default-arena
reservation "leaked". Reachability (and provenance) follow POINTERS: base is
now `*mut u8` and chunk derivation uses `.add()`. That's the third time this
lesson fired (registry M4, arenas M7) — it is now a review checklist item.

**Gates green:** Windows full suite + Linux 19 suites/0 failures · the new
heaps gate (visitor counts exact, delete-migration contents verified, destroy,
exclusive-arena containment + recycled-chunk re-zeroing, subproc isolation,
options/env, stats/process_info) · miri clean (heaps gate included; subproc
section native-only — the mock's TLS dtors don't fire, documented) · G1/G2
unchanged and green · clippy `-D warnings` + fmt.

**Known divergences (documented, tracked):** arena chunks are
segment-granular (32 MiB; upstream is slice-granular); `_commit=false`
arena reserves still commit (eager model); NUMA recorded not enforced;
purge/decommit of free spans and arena chunks still pending (RSS);
`mi_stats_merge` is a no-op (merged-on-read); racy-by-design stats snapshot
(volatile read) pending an atomic-counters refactor; no `std::get_new_handler`.

## M5 — Aligned + POSIX + zero-preserving family (2026-08-05)

**Landed:** the API-completeness milestone, part 1 — ~34 new functions.
- **`aligned_at` with interior-pointer recovery** (the one real architecture
  piece): `(p+offset) % align == 0` via three tiers — natural fit through the
  bins (64 KiB-aligned areas ⇒ `bsize % align == 0` qualifies every block),
  oversize-and-adjust (interior pointer; page marked `has_aligned`, free and
  usable_size recover the block start by block arithmetic — works for binned,
  large-span, and adopted pages alike), and exact placement in dedicated huge
  segments (offset-aware, slack only when the offset actually shifts the
  boundary).
- Full §5.4 aligned family + §5.7 zero-preserving (`rezalloc`/`recalloc` +
  aligned/_at — resting on the invariant that a zalloc'd block is zero across
  its FULL usable extent, so moves zero exactly `[old_usable, new_usable)`),
  §5.5 `u*` block-size-returning variants, §5.11 POSIX core
  (posix_memalign with EINVAL/ENOMEM, memalign, valloc/pvalloc, aligned_alloc,
  reallocarray/reallocarr, cfree via the segment map, `_expand`,
  malloc_size/usable_size/good_size, sized frees with debug verification),
  `realloc_aligned(_at)`.
- Harness: trace gen emits ~15% aligned allocations (16 B–64 KiB) through the
  `align_log2` field the format carried since v0; the system arm allocates/
  frees through matching Layouts; realloc is restricted to natural-alignment
  blocks (the C contract). New gates: `aligned_at_offsets` (all tiers),
  `rezalloc_grows_zero`, `align_storm` (randomized aligned churn + canaries,
  interior-free recovery hammered).

**Gates green:** Windows + Linux full suites (18 result rows) · G1 CLEAN on
1M-op traces WITH aligned ops on both arms, strict leak gate 527 465 == 527 465
· miri clean over the new paths (112 s, all 11 alloc_core tests interpreted) ·
loom untouched (protocol unchanged) · clippy `-D warnings` + fmt.

**Notes:** `realloc` does not preserve >8 alignment (per the C contract) —
gen/replay encode that; `realloc_aligned_at` exists for callers who need it.
Sized-free fast-path exploitation (skip the page walk) is an M8 brick; M5
verifies the size under debug only. `mi_realpath`/`mi_dupenv_s`/wide-char
helpers remain M7 (they are I/O, not allocation).

## M4 — Per-thread heaps, lock-free cross-thread free, abandonment (2026-08-05)

**Landed:** the global lock is GONE. Per-thread heaps in os-allocated HeapBoxes
reached through a const-init `thread_local!` pointer (the R1-validated 0.3 ns
path; !Drop key ⇒ no allocation on access, no bootstrap recursion — heap
storage comes from the prim layer). `free` routes by `Segment::thread_id`:
owner → local path; else the **4-state xthread protocol**
(NORMAL/DELAYED/FREEING/NEVER packed into the page's atomic word with the list
head): full-queue and large pages sit DELAYED so remote frees nudge the owner's
delayed list (drained at heartbeat — that is what un-parks full pages whose
blocks died remotely); FREEING guards the heap deref against teardown; NEVER
covers abandonment. Thread exit (prim FLS/pthread destructor from M1): collect
→ retire → surviving pages to NEVER (spinning out FREEING) → drain delayed →
publish segments on the global abandoned list → release heap storage.
Allocation slow paths adopt abandoned segments before reserving OS memory.
Global heap registry (M7 stats walk + honest reachability). FFI: mi_collect,
mi_thread_init/done, mi_process_init/done, mi_thread_set_in_threadpool.

**Protocol verified by loom BEFORE implementation** (`tests/loom_xthread.rs`
is the spec): delayed-push vs abandon (the use-after-free the FREEING state
exists to prevent + block conservation), normal-push vs collect, park/unpark
vs remote. Preemption bound 2 locally; `LOOM_EXTENDED=1` → bound 3 in CI.
Loom immediately enforced its own hygiene: spin loops need `yield_now`, CAS
protocols need a preemption bound and branch budget.

**Defects caught by gates:** miri flagged heap boxes/segments as leaks after
the static→TLS move — root cause: the registry stored pointers as `usize`,
and REACHABILITY FOLLOWS POINTERS, NOT INTEGERS (AtomicPtr fixed it; the same
rule keeps our own provenance honest).

**Gates green:** Windows full suite + Linux full suite (18 result rows incl.
the new mleak test: 4 threads exit with 2 000 live blocks; contents survive
abandonment, frees from main route via NEVER, main's churn adopts segments) ·
G1 1M-op realloc trace with counters IDENTICAL to the locked M3 run (perfect
cross-milestone work parity) · miri clean · clippy `-D warnings` + fmt.

**MT kernels (in-process wall, quiet-box Windows):** larson 8 threads
**146 Mops/s** with 780k cross-thread frees; xmalloc (100% remote frees)
**51.8 Mops/s**. Canary-checked throughout.

**Measurement note:** the first post-M4 malloc-small readings (Win 27.9,
Linux 17.9) were taken while our own miri/loom/WSL gates saturated all 24
cores — both arms fell ~4× equally, ratios held (~2.3× Win). Discarded per the
go-find-the-process rule; quiet-box numbers below.

**Loom postscript:** the 3-thread abandon model exceeded loom's exploration
budgets twice (spin heuristics). Fix was MODEL DECOMPOSITION, not bigger
budgets: the UAF invariant needs exactly ONE remote vs the abandoner —
exhaustively explored, unbounded, in 3.7 s (4/4 models green). The 2-remote
wide-space variant is the `LOOM_EXTENDED=1` CI soak. Lesson: a protocol model
should be the smallest machine that can violate the invariant.

**Quiet-box numbers** (in-process wall, method lines printed; single-session,
pinned ABBA still owed for standing claims):
- Windows malloc-small: **ra 78.9 Mops/s** (locked M3: 67.9 → lock removal
  +16%) vs system 36.8 (2.14×). larson 8T: ra 202 Mops/s standalone;
  xmalloc all-remote: 70.1 Mops/s standalone.
- Windows cross-arm MT ratios: **NOT RESOLVED** — the box degraded mid-session
  (identical runs spread 10–87 Mops/s; likely rust-analyzer storm after the
  manifest edit — the check-what-your-edit-woke-up corollary). ABBA pairs
  taken, ratios 0.77–2.64, no verdict quoted. Needs a pinned quiet session.
- Linux (WSL, observational, 2 interleaved rounds, consistent direction):
  malloc-small ra 71.7 vs glibc 76.9 (0.93× single-threaded — glibc's tcache
  edges the still-unoptimized fast path). **larson 8T: ra 162.8/114.0 vs
  glibc 120.3/104.1 (ahead both rounds). xmalloc (100% cross-thread frees):
  ra 27.7/51.7 vs glibc 3.6/16.7 — 3–8× ahead, both rounds, the M4 protocol's
  designed win.**

**Deferred, tracked:** TSan MT fuzz (needs -Zbuild-std wiring, CI follow-up);
pinned cross-arm MT session on a quiet box; single-threaded fast-path polish
(M8 — the 0.93× vs tcache gap); `mi_stats_merge` over the heap registry (M7);
no_std profile (post-v1, needs a TLS story without std).

## M3 — Realloc, large pages, reclamation, segment map (2026-08-05)

**Landed:** span reclamation (per-segment first-fit free-span list with O(1)
left/right coalescing via first/last slot markers), page retire (empty pages
return their span; one page per queue stays warm), in-segment **large pages**
(64 KiB–16 MiB single-block spans, fresh-per-request, retire-on-free — span
reuse IS the recycle path), a **one-empty-segment cache** (without it every
large alloc/free cycle paid a 32 MiB OS round-trip), the **segment map** (1 bit
per 32 MiB window, 1 MiB BSS; `mi_is_in_heap_region`), the **realloc family**
(`realloc` in-place when still-fits-and-≥-half-used, `reallocn`, `reallocf`,
`expand`, `GlobalAlloc::realloc`), `mi_strdup`/`mi_strndup`, 8 new FFI exports,
Realloc in trace gen/replay with G1f prefix-preservation + G1g strict-leak
gates.

**Two real bugs caught by our own gates before shipping:**
1. **Bump-frontier give-back broke the zero invariant** — `span_free` merged
   freed spans back into the virgin bump region, whose allocations report
   `fresh = true`; recycled dirty memory then skipped zalloc's memset. Caught
   by the spans G1c test. Freed spans now never rejoin the frontier.
2. Freeing a segment's only page released the segment instantly, making
   free-then-alloc cycles reserve a fresh 32 MiB each time → the segment cache.

**Instrument lesson:** the recurring "exit 255 with all tests ok" ghost was the
harness, not the code — truncating cargo's output pipe (`Select-Object -First`)
breaks `$LASTEXITCODE`. Read exit codes from full pipes only.

**Gates green (both OSes):** core 8/8 + spans lifecycle (deterministic-counter
process: reuse without new segments, recycled-span re-zeroing, retire counts,
12 MiB coalesced fit, realloc in-place/move/shrink semantics, expand
never-moves, segment map yes/no) + selfhost 5/5 + G2 + **G1 on a 1M-op trace
with realloc**: 529 898 allocs == frees (strict leak gate), 9 952 spans
retired, 9 segments (3 freed — cache policy visible), 9 897 large + 1 050 huge
allocs, realloc 22 102 in-place / 77 899 moved · miri clean (coalescing
interpreted end-to-end) · clippy `-D warnings` + fmt.

**Numbers** (in-process wall, same seed, method lines printed; single-run —
pinned ABBA still owed): malloc-small **Windows ra 67.9 Mops/s vs system 28.1**
(M2 binary: 39.7 vs 17.6 — box conditions drifted too; ratio ~2.4×), **Linux
ra 71.2 vs glibc 71.1 — parity reached with the lock still in place** (M2:
57.1). Counters bit-identical across OSes (10 002 036 allocs = frees, generic
6.0%, 132 pages, 553 extends) — work parity + determinism hold. The M2→M3
speedup is plausibly retire keeping the hot page resident; treat as observed,
not confirmed until a pinned same-binary A/B.

**Deferred, tracked:** purge/decommit of free spans (RSS story, M7 options);
in-place realloc for large spans via span growth (M5-ish, currently copies);
aligned realloc (M5); `mi_realpath` (M7); huge-segment map bits cover the whole
reservation (done) but Normal segments assume ≤ 2⁴⁸ VA (LA57 = false-negative).

## M2 — Single-threaded core (2026-08-05)

**Landed:** the allocator exists. `bins.rs` (oracle-pinned geometry), `page.rs`
(three sharded free lists, lazy extension), `segment.rs` (32 MiB-aligned sliced
segments, eager commit, dedicated huge segments, ptr→page = mask + slice walk),
`heap.rs` (75 bin queues + full queue, direct table, `malloc_generic` heartbeat,
free with full-queue unpark), `alloc.rs` (global spin-locked heap — the M2
threading model, removed in M4), aligned-subset (natural-fit via bins, huge
fallback), `GlobalAlloc` impl, 12 `mi_*` FFI exports, G1 replayer + trace gen,
Tier-B `malloc-small` kernel, R1 `tls-spike`.

**G2 earned its keep twice** (differential vs oracle DLL/so, every size
1..=64 KiB, both OSes):
1. mimalloc's default is **MI_ALIGN2W**: wsizes ≤ 8 round to EVEN word counts —
   bins 24/40/56 B don't exist (that's how 16-byte max_align_t is guaranteed).
   My from-paper formula had them. 49k mismatches → 0.
2. The binned cutoff is **64 KiB** (`MEDIUM_PAGE_SIZE/8`), not 128 KiB (/4);
   above it `good_size` is page-rounded.

**Other defects caught by gates before they shipped:** zalloc returned blocks
whose first word held the free-list link (upstream zeroes exactly that word —
now we do); test-vs-test races on the shared heap (fixed test design, kept the
stress value); Windows DLL dependency resolution needs absolute paths +
redirect preload; **mixed-OS cmake caches corrupt both oracle builds** →
OS-namespaced `oracle/out/{win,linux}` (scripts + docs updated).

**Gates green:** core 7/7 + lib 5/5 on Windows AND Linux · G1 replay CLEAN on a
1M-op synthetic trace (alignment/usable/zero/canary/overlap) on both arms ·
G2 PASS both OSes · **selfhost 5/5 both OSes** (rusty_alloc as the test
binary's real `#[global_allocator]`: HashMap/BTreeMap churn, Vec grow/shrink,
cross-thread frees, 40 MiB boxes) · **miri clean over the whole core**
(segments, mask trick via `with_addr`, free lists) · clippy `-D warnings` +
fmt clean · every `unsafe` block carries its SAFETY invariant.

**First numbers** (Tier-B `malloc-small`, in-process wall, method lines in
output; standing claims await pinned ABBA vs oracle arms):
- Windows: **ra 39.7 Mops/s vs system 17.6** (2.26× ahead) — with the global lock.
- Linux: **ra 57.1 vs glibc 71.1** (0.80×) — glibc's lock-free tcache vs our
  locked fast path; this gap IS the M4 work item, not an M2 regression.
- Counters bit-identical across OSes (allocs 10 002 036 = frees; generic 6.0%;
  1 segment / 27 pages / 273 extends) — deterministic kernel, work-parity holds.

**R1 RESOLVED** (tls-spike, 100M accesses): `thread_local!` + `const` init =
**0.32 ns/access ≈ bare atomic load** (0.31) on Windows; 0.29 vs 0.26 on Linux.
OS-slot (FLS/pthread) path: 3.87 / 1.73 ns. M4 design: `thread_local! const`
for the heap pointer; prim TlsSlot only as the thread-exit destructor hook.
No nightly `#[thread_local]` needed.

**Deferred, tracked:** page retire/slice reclamation + realloc family (M3);
lock removal + xthread activation (M4); free-list encoding stays off to match
the oracle's release default (secure/debug feature, M8); rdtsc path profiler
skeleton (first optimization session).

## M1 — OS primitive layer (2026-08-05)

**Landed:** `prim/` (Windows VirtualAlloc backend incl. the aligned reserve-release-
re-reserve race-retry dance, large-page attempt + fallback, FLS-based TLS destructor,
QPC clock; unix mmap backend with over-allocate-and-trim alignment, MADV_DONTNEED
decommit, MADV_FREE reset, pthread_key TLS; miri mock with alloc registry), `os.rs`
(cached config, page rounding, `alloc_aligned`, purge policy).

**Gates:**
- Windows native: 9/9 integration tests (32 MiB segment alignment, fresh-zero pages,
  reserve→commit→write→decommit→zero cycle, reset stays accessible, protect
  round-trip, 50-case size×alignment sweep, thread ids, clock scale, **TLS dtor fires
  at thread exit with the stored value**, NUMA ≥ 1).
- Linux native (WSL2 Ubuntu, rustup stable): same suite, 9/9.
- miri (mock backend): 2/2 — caught a real defect before it ran anywhere: the mock's
  registry used a non-const `HashMap::new` in a static (cfg(miri)-only code stable
  never compiled). Fixed with `OnceLock`.
- clippy `-D warnings` clean both targets; fmt clean; `cargo check` green on
  x86_64-pc-windows-msvc and x86_64-unknown-linux-gnu.

**Notes for M2:** prim `commit` conservatively reports `is_zero = false` on both
platforms (range may span still-resident pages) — the page layer must track
per-page `is_zero` itself off fresh-mapping info, exactly as upstream does, or
`calloc` double-zeroes. Windows decommit needs recommit; Linux DONTNEED does not —
the purge accounting must carry `needs_recommit` per range.

## M0 — Scaffold + oracle + corpus (2026-08-05)

**Landed:** workspace (5 crates), oracle submodule pinned @ v2.4.5 (cde3f7a0) with
3-arm build scripts (mi/dmi/smi, built OK with cmake+MSVC), mimalloc-bench submodule,
`.ratrace` trace format v0 (round-trip tested), `bench/pinvs.ps1` compliant harness,
CI (fmt/clippy/check/test win+linux, cross-target, miri, oracle build), WSL2 doc.

**Gates:** 4/4 tests; clippy/fmt clean; **first differential gate passed** — oracle
`mimalloc.dll` and `rusty_alloc_ffi.dll` loaded in one process both report
`mi_version() == 20405`.

**Environment note:** WSL2 Ubuntu installed 2026-08-05 (24 cores). mimalloc-bench
needed `unzip` + `dos2unix` beyond the documented packages (shbench patch step) —
recorded in corpus/WSL2.md.

**Tier-A gate closed (2026-08-05):** full bench suite built in WSL2; `bench.sh mi
cfrac` runs (6.90 s wall / 6.77 s user / 7.3 MB RSS / 746 page-reclaims).

**Null-arm session, cfrac × mi (METHOD: WSL2 Ubuntu on /mnt/c, N=4 each arm):**
- unpinned: 6.80–12.26 s wall — **1.80× spread**; page-reclaims 743–750 (work
  parity holds, so it is scheduler migration, not the workload)
- `taskset -c 2 nice -n -5`: 7.34–9.31 s — 1.27× spread; minima agree (~6.8–7.3 s)

Verdict: best-of-N minima are usable for absolute floors; paired A/B on this box
needs pinning AND large N, and WSL2 numbers are dev-loop only — plan risk R5
confirmed empirically on day one. Every future WSL2 run states this method line.
