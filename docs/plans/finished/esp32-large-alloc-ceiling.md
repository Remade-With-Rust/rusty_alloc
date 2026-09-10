# A 64 KiB allocation fails with 192 KiB of the region free (ESP32-S3, 2.0.5)

Reported 2026-09-09 from the `rusty_zstd` bare-metal work. Measured on real
silicon, not inferred: ESP32-S3 rev v0.2, `xtensa-esp32s3-none-elf`,
`rusty_alloc 2.0.5` + `rusty_alloc-api 2.0.5` from crates.io, built
`default-features = false` with `--cfg ra_single_threaded --cfg
ra_small_profile`, region handed over as
`prim::fixed::Region<{ good_region_size(N) }>` exactly as the README prescribes.

**Status: CONFIRMED and FIXED, 2026-09-09. See section 9.** Reproduced on the
same part, root-caused, and closed with a geometry knob that takes the reported
case from 1 block to 3. The verdict on "by design or defect" is *both*, and
section 9 says which half is which.

## The one-paragraph version

In a **256 KiB region (four 64 KiB segments, 262,144 bytes reported usable)**,
`rusty_alloc` serves **exactly one** 64 KiB allocation. The second fails. At
that moment 64 KiB is live and **192 KiB of the region is unused**.

## Minimal reproduction, no zstd involved

```rust
static REGION: Region<{ good_region_size(256 * 1024) }> = Region::new();
#[global_allocator]
static ALLOC: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;

// in main, after REGION.give().unwrap():
let mut big: Vec<Vec<u8>> = Vec::new();
for i in 0..8 {
    println!("requesting 64 KiB block #{i} ...");
    big.push(vec![0u8; 64 * 1024]);          // #1 never returns
    println!("64 KiB block #{i} OK  (live {} KiB)", (i + 1) * 64);
}
```

Observed:

```text
region            262144 bytes declared, 262144 usable
probe  requesting 64 KiB block #0 ...
probe  64 KiB block #0 OK  (live 64 KiB)
probe  requesting 64 KiB block #1 ...
memory allocation of 65536 bytes failed          <- 192 KiB still free
```

## What DOES work, so the shape is clear

Same region, same build, from a fresh heap:

| request shape | result |
|---|---|
| 16 x 656 B | OK |
| 4 x 32 KiB (128 KiB live, 50% of region) | **OK** |
| a further 656 B after those | OK |
| 1 x 64 KiB | OK |
| **2 x 64 KiB** | **fails, 192 KiB free** |

So small and medium blocks pack fine. The cliff is exactly at the **segment
size**: a request at or near 64 KiB appears to need more than one 64 KiB
segment, which leaves a four-segment region able to satisfy one of them.

## Hypothesis (yours to confirm — we have not read the internals)

A 64 KiB user request cannot fit inside a 64 KiB segment once the segment's own
metadata is accounted for, so it takes **two** segments. A 256 KiB region has
four, block #0 consumes two, and the remainder cannot serve another. If that is
right, the usable fraction for segment-sized requests is roughly 50% at best and
the failure is structural rather than a leak.

Two experiments that would discriminate, both cheap on your rig:

1. Sweep the request size across the segment boundary (60, 62, 64, 66 KiB) in a
   fixed region and record the last size that still gives two successes. If the
   cliff sits just below `SEGMENT_SIZE`, the metadata-overflow story holds.
2. Report free segments alongside free bytes at the point of failure. Free bytes
   alone (192 KiB) look like plenty and hide the real constraint.

## Why it matters to a consumer, concretely

`rusty_zstd` 0.2.5 now builds and runs `no_std + alloc` on this part (it
round-trips at levels 1, 3 and 5 on the S3). It allocates its match tables in
units of tens of kilobytes, including 64 KiB and larger. Against `esp-alloc
0.11` on **the same firmware source, one cargo feature apart**:

| | esp-alloc 0.11 | rusty_alloc 2.0.5 |
|---|---|---|
| 8 KiB payload, L1/L3/L5 round trip | **PASS** | **OOM** |
| peak heap the workload needs | 175,832 B | (never reached) |
| smallest heap that still round-trips | **176 KiB** | fails at 192 and 256 KiB |
| 320 KiB region | n/a | does not link: "Main stack is smaller than 8192 bytes" |

The 320 KiB row is the ceiling on this part: RAM is a fixed map, so a bigger
region comes straight out of `.stack` until the linker refuses. There is
therefore **no region size on an ESP32-S3 at which this consumer can use
rusty_alloc**, which is why this is worth your time even if the behaviour turns
out to be intended.

Static cost, for completeness, both arms at the same 192 KiB budget on the
linked ELF (`.data + .bss + .stack` sums to a constant, so the stack column is
the one that moves):

| | esp-alloc | rusty_alloc | delta |
|---|---|---|---|
| `.bss` | 196,728 | 198,788 | +2,060 |
| `.data` | 2,432 | 2,356 | −76 |
| `.stack` | 135,848 | 133,852 | −1,996 |
| `.text` + `.rodata` | 222,685 | 230,221 | +7,536 |

`.stack` shrank by `Δ.bss + Δ.data` to within 12 bytes, exactly as your README
says it does.

## Against the README

`README.md` states the floor as *"64 KiB for `rusty_alloc` ... against 8 KiB for
`esp-alloc`"*, with *"a size-class page allocator's [floor] is (classes touched)
x (page size), independent of bytes requested"*, and adds that the floor
*"amortises as the working set grows"*.

The model predicts a fixed additive cost, so a 112 KiB working set should fit a
256 KiB region with room to spare. It does not, and the amortisation claim
inverts for this consumer: the cost is not a fixed floor but a **granularity
tax that scales with how many segment-sized blocks are live**. Whatever the
verdict on the code, that paragraph is worth a sentence about large allocations.

## What "fixed" would look like

A kill test a stranger can run, in the shape this repo already uses:

```text
Region<{ good_region_size(256 * 1024) }>, --cfg ra_single_threaded --cfg ra_small_profile
allocate 64 KiB blocks in a loop
PASSES WHEN: at least 3 succeed before the region is exhausted
TODAY:       1
```

## Provenance

- Firmware: `rusty_zstd`'s `bare-metal/esp32s3` (two arms, one source, the
  allocator selected by a cargo feature) at commit `8941050`.
- Every number above is from the board over USB serial, not a host simulation.
- Nothing has been filed against a public repository; this note is the hand-off.

---

## 9. Resolution (2026-09-09)

**The report is correct in every particular, including its hypothesis.**
Reproduced on a XIAO ESP32-S3 with the consumer's own shape — a 256 KiB
`Region`, 64 KiB blocks in a loop:

```text
[big] geometry: SEGMENT_SIZE=65536 LARGEST_SHARED_ALLOC=61440 dedicated_segments(65536)=2
[big] before #0: used=0 free=262144 total=262144 | free_segments=4 largest_servable=258048
[big] block #0 OK (live 64 KiB)
[big] before #1: used=135168 free=126976 total=262144 | free_segments=1 largest_servable=61440
[big] block #1 REFUSED
[big] served 1 blocks of 65536
```

### 9.1 The mechanism, and their experiment 1

A segment's slice 0 holds its header, so `LARGE_OBJ_SIZE_MAX` is
`SEGMENT_SIZE - SEGMENT_SLICE_SIZE` = **61,440**. Above that a request is
routed to `huge_alloc`, which reserves `4 KiB + size` and must start on a
`SEGMENT_SIZE` stride — so it spans **two** segments. The sweep they asked for,
computed from the same constants the allocator routes on:

| request | shares a segment? | dedicated segments |
|---|---|---:|
| 61,440 | yes | 0 |
| **61,441** | no | **2** |
| 65,536 | no | 2 |
| 131,072 | no | 3 |

**The cliff is at 61,440, one slice below `SEGMENT_SIZE`, exactly as their
metadata-overflow story predicted.** It is structural, not a leak: no
allocation of `SEGMENT_SIZE` can share a segment with the metadata describing
it, at any segment size.

Their arithmetic said "roughly 50 % at best" and the board said 25 %, because
of a cost they could not see: **the first small allocation claims a whole
segment.** `used=135168` above is block #0's 69,632 plus a 65,536 segment for
the `Vec` spine. Two segments went to one 64 KiB block, one to bookkeeping, and
the fourth could not hold the second block.

### 9.2 Their experiment 2, shipped

`region_stats` reports free BYTES, and free bytes hide this. Added
`prim::fixed::region_capacity() -> (free_segments, largest_servable)`, which on
the failing call reads `(1, 61440)` against 126,976 free bytes — the question
answered in the units that decide it.

### 9.3 Verdict: by design AND a defect

- **By design:** the two-segment cost of a segment-sized allocation follows
  from mimalloc's segment/header structure and cannot be removed at a given
  `SEGMENT_SIZE`. Putting the header out of band was considered and rejected
  before (`segment-tax.md` F3): every `free` resolves its segment by masking
  the pointer, and a table lookup there is a cost on the hottest path in the
  crate for a consumer-specific win.
- **A defect:** the README taught a floor model that inverts for this workload,
  there was no way to size a region correctly in advance, nothing reported the
  real constraint, and there was no escape hatch. All four are fixed.

### 9.4 The fix, measured

**`--cfg ra_segment_size="256k"`** moves the small profile to an 8 KiB slice x
32 slices, so `LARGEST_SHARED_ALLOC` becomes 253,952 and a 64 KiB request is a
span the allocator packs three-to-a-segment. Same board, same 256 KiB region:

| geometry | blocks served | payload live | region used |
|---|---:|---:|---:|
| default (64 KiB segment) | 1 | 64 KiB | 25 % |
| `ra_segment_size="256k"` | **3** | **192 KiB** | **75 %** |

Their kill test — *"PASSES WHEN: at least 3 succeed"* — **passes**. The
claim that "there is no region size on an ESP32-S3 at which this consumer can
use rusty_alloc" no longer holds: 256 KiB at this geometry serves 3 x 64 KiB
with 56 KiB of slices left for everything smaller, inside the 320 KiB ceiling.

**What was NOT verified, stated plainly.** The kill test in section 8 was run,
and passes. `rusty_zstd`'s actual workload was not: this end measured 64 KiB
blocks in a loop, not match tables through a real round trip. The peak that
firmware reported, 175,832 bytes, is inside the 253,952 usable at this geometry
and its 64 KiB units pack three to a segment, so the arithmetic says it fits —
but packing depends on the live SET, and only that firmware can run it. If it
does not fit, `region_capacity()` and `region_for_allocs` are now there to say
why in one line rather than a bisect.

Also shipped: `LARGEST_SHARED_ALLOC`, `dedicated_segments(size)` and
`region_for_allocs(size, count)` so the ceiling is a compile-time answer rather
than a board discovery; `region_for_allocs` counts the small-allocation segment
from 9.1. The README's floor section now states the large-allocation case with
these numbers.

### 9.5 A rung that was built and withdrawn — worth recording

A 4 KiB x 32 (128 KiB) geometry was implemented first and removed, for a reason
that generalises: **it buys a 64 KiB consumer nothing.** The request becomes a
16-slice span in a 31-slice segment, so exactly one fits and the block still
costs 128 KiB — identical to the default's two-segment huge path, reached by a
different route. The lever is not `SEGMENT_SIZE` by itself but
`LARGEST_SHARED_ALLOC / size`, the number that pack into one segment, and that
only exceeds 1 when the slice grows too.

It also **segfaulted 11 runs in 12** under the concurrent host battery
(`tests/secure.rs`, default features), where the default and `"256k"` geometries
are 0/40 and 0/12. It passes single-threaded and under `debug_checks`, so it is
a concurrency-sensitive fault specific to 4 KiB x 32. Chased far enough to know
it is real and to keep the geometry out of the shipped set; **not** root-caused.
Recorded here rather than dropped, because it may be a latent fault that this
geometry merely exposes.

### 9.6 Gates

Whole suite green at the default and at `"256k"` (20 suites each), and CI now
runs the full suite at the new geometry rather than merely building it. The
mutation self-test gained an eleventh case: drop the header slice from
`dedicated_segments` and the sizing test goes red (11/11 fire). The reproduction
itself is a permanent test against the real extent allocator, expressed as a
property — a region of dedicated blocks cannot reach half utilisation — so it
does not depend on brittle placement arithmetic. Unsafe census +4, all
`#[cfg(test)]`; the fix adds no unsafe to shipped code.
