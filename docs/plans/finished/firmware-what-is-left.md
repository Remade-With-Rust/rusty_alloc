# What is left on a firmware after 2.0.2

**Status: EXECUTED 2026-09-09 — see §7.** Every item in §6, in order, plus two
levers found beside them: flash cost **+7,860 → +3,208 B**, static RAM
**+3,052 → +284 B**, attributed code 8,262 → 4,313 B, the granule handed back
to the consumer as a `const fn`, `collect_inner` closed as structure, and the
speed claim re-measured on the board (it was already on silicon; it was stale).

Originally: the third and much shorter report from the Janus firmware. 2.0.2 took every lever the last one proposed and halved the
flash cost, so this one is mostly about what is *not* worth doing, plus one
item that is now larger than all the remaining code put together.

Same method as before: XIAO ESP32-S3 Sense, one firmware source built two
ways, `--cfg ra_single_threaded --cfg ra_small_profile`, `size -A` and
`nm --size-sort` on the linked ELF, 172,800 bytes live in both arms.

Verified position after 2.0.2:

| | esp-alloc | rusty_alloc 2.0.2 |
|---|---:|---:|
| allocator code | 1,043 B / 6 symbols | 8,297 B / 37 symbols |
| firmware flash content | 60,361 B | 68,445 B (**+13.4 %**) |
| static RAM | 0 | +3,052 B, taken from `.stack` |
| usable heap from a 220 KiB region | ~225,280 B | 196,608 B |

---

## 1. The stranded tail is now the dominant cost, by 3x

**Class: granularity. Arithmetic, not projection: 24,576 bytes.**

The remaining *code* costs 8,297 bytes. The region granule costs **24,576** on
this firmware — three times as much — and unlike the code it is not fixed, it
scales with the region.

`usable_bytes` (2.0.1) lets a consumer *observe* the loss. Nothing helps them
*avoid* it, and avoiding it is pure arithmetic:

```rust
/// The smallest region >= `want` that strands nothing.
pub const fn good_region_size(want: usize) -> usize;
```

Then a firmware writes `Region<{ good_region_size(220 * 1024) }>` and gets
196 KiB with zero waste instead of 220 KiB with 24 KiB dead. It mirrors
`bins::good_size`, which already does exactly this one level down, and it
turns a rule that currently lives in a design document into something the
compiler applies.

**Why this beats any further code pruning.** Every remaining code lever below
is worth hundreds of bytes and needs a `cfg` and a correctness argument. This
is worth 24 KiB, is a `const fn`, and cannot be wrong.

## 2. `collect_inner` is 24 % of the remaining code — a question, not a claim

**Class: unknown until a reachability question is answered. No number
offered.**

| symbol | bytes | share of what is left |
|---|---:|---:|
| `Heap::collect_inner` | 1,988 | 24 % |
| `Heap::malloc_generic_once` | 1,950 | 23 % |
| *(the other 35)* | 4,359 | 53 % |

Its own comment says what it is for: *"`force` RECLAIMS ABANDONED SEGMENTS"*,
and a segment is abandoned when a thread ends. 2.0.2 already pruned
`adopt_segment` under `ONE_THREAD` on exactly that reasoning.

**But the obvious follow-on may be wrong, and that is why this is a question.**
A first-class `Heap` that a single-threaded program destroys could also orphan
segments, so `ONE_THREAD` alone does not license removing the reclaim path.
What is true is narrower and only about *this* image: no heap-destruction
symbol survives the link here, so nothing in this firmware can produce an
orphan.

So the useful question is whether orphans are reachable under `ONE_THREAD`
**without** heap destruction, and if not, whether the two conditions together
are a gate worth having. If they are, this is the largest single symbol on the
target. If they are not, saying so closes the largest remaining line
permanently, which is just as valuable.

## 3. Static RAM is scarcer than flash here, and ~688 bytes of it is tunable

**Class: granularity. Small, but the units matter more than the size.**

The `.stack` identity from the last report means **every byte of static RAM
comes out of stack headroom**. That makes these worth more than their size
suggests:

| symbol | bytes | why it is generous here |
|---|---:|---|
| `EXT_BASE` + `EXT_LEN` | 256 | `MAX_EXTENTS = 32` free holes, in a firmware with one region |
| `options::VALUES` | 304 | runtime-tunable options, all compile-time constant on a chip |
| `arena::ARENAS` | 128 | arena slots, on a target with one region |

A `ra_max_extents` cfg, and folding the options table where nothing can set an
option at runtime, would return most of ~688 bytes **to the stack**. On a part
where the stack is the thing that overflows, that is a better trade than the
same number of flash bytes.

## 4. What is explicitly NOT a lever

Listed so nobody re-derives them:

- **`malloc_generic_once` (1,950 B)** is the generic allocation path. It is the
  allocator doing its job. Structure, not waste.
- **`create_heap` (966), `prim::alloc` (573), `span_alloc` (419),
  `chunk_alloc_inner` (377)** — likewise the working allocator. Together 2,335
  bytes and all of it earns its place.
- **`range_table::BASE` / `END` (256 each)** are the segment map. Removing them
  removes the pointer-to-segment lookup, which is the design.

Between them these are ~5.5 KB of the 8.3 KB that remains, and the honest
statement is that **the allocator is now close to its floor on this target.**
After the previous two reports halved it, there is no third halving available
in code.

---

## 5. The gap in the claims: nobody has timed this on a chip

Not an optimization, but the most useful thing left.

The README's 2.0-3.7x is measured on a hosted churn harness. The embedded
profile is a **different allocator geometry** — 4 KiB slices, 16 slices per
segment, a fixed backend with a spin lock instead of an OS — and its
allocation throughput has never been measured on silicon by anyone, including
this consumer. The Janus firmware cannot supply it either: it allocates five
buffers once and never frees, which is the wrong shape entirely.

So the position today is that the embedded story is **fully measured on
footprint and completely unmeasured on speed**, while the published speed
figure comes from a geometry that firmwares do not run.

**Offer.** This consumer can build the missing arm: a churn firmware that
allocates and frees across several size classes with a deterministic operation
count, run on a XIAO ESP32-S3 against both allocators from one source,
reported with its method line and its work-parity count. Say the word and it
lands in `rusty_esp_dsp/firmware/`. It would either confirm the hosted figure
transfers, or find that it does not — and the second outcome is worth more.

One caution carries into it from the last report, and it applies to this
repo's own published tables: **an allocator change moved a compute benchmark
by 8 % on this board without executing an instruction inside the measured
region.** Buffer placement did it. Any embedded speed comparison needs its
addresses held constant, or its noise floor established across a reflash, or
it will measure placement and call it throughput.

---

## 6. Suggested order

1. **`good_region_size`.** 24 KiB, a `const fn`, no correctness argument.
   Larger than every other item combined.
2. **Answer the `collect_inner` question**, either way. It is 24 % of the
   remaining code, and the answer either closes the largest remaining line
   permanently or opens the last real lever.
3. **The embedded churn benchmark**, if wanted — the only unmeasured half of
   the embedded story.
4. **The static-RAM cfgs.** ~688 bytes, but they are stack bytes.

---

## 7. Resolution (2026-09-09)

Same rig as the previous report and as §7 of `firmware-code-size.md`: the
`xiao-s3-probe` firmware copied to a scratch directory with `[patch.crates-io]`
pointing both crates at the working tree, one source, two arms, `size -A` and
`nm -S --size-sort` on the linked ELF, each brick built and diffed before the
next was written. The baseline is 2.0.2 as published (`.text` 54,221,
`.rodata` 9,604, `.data` 4,396, `.bss` 226,328; 8,262 attributed bytes in 36
symbols). Every item in §6 is done, in its order, and three more were found on
the way.

### 7.1 §1 — `good_region_size`, and its mirror

Two `const fn`s in `prim::fixed`:

```rust
pub const fn good_region_size(budget: usize) -> usize; // largest zero-waste region <= budget
pub const fn region_for(usable: usize) -> usize;       // smallest region serving >= usable
```

The plan's signature said "smallest region >= want" and its example rounded
DOWN (220 KiB -> 196 KiB usable); the example is the useful one — a budget is
a ceiling — so `good_region_size` rounds down and `region_for` is the other
direction, for a firmware that knows what it needs rather than what it can
spare. `good_region_size(220 * 1024)` is 200,704: three segments plus the
page, `usable_bytes` of exactly `3 * SEGMENT_SIZE`, and the 24,576 bytes the
old declaration stranded handed back to the firmware. Below `MIN_REGION` it
returns 0, which the seam's existing `const` assertion on `usable_bytes` turns
into a build error. Both are tested against `usable_bytes` over every residue
of the segment size at both geometries, and `tools/gate-selftest.sh` now
proves the test can fail (a wrong granule goes red).

No image delta, by construction: a `const fn` nobody calls at run time links
to nothing. The 24,576 bytes it recovers are the consumer's to take, with one
line: `Region<{ good_region_size(220 * 1024) }>`.

### 7.2 §2 — the `collect_inner` question, answered: closed, and then smaller anyway

**Orphans are not reachable under `ONE_THREAD`, with or without heap
destruction.** A segment is abandoned only by `abandoned_push`, whose only
caller is `thread_done`, which nothing on a bare-metal image calls; `heap_delete`
migrates a first-class heap's segments into the backing heap through
`adopt_segment` directly and never touches the abandoned list; `heap_destroy`
frees. And since 2.0.2 `abandoned_pop` folds to null under `ONE_THREAD`, so the
reclaim loop the comment describes is not in the image at all — there is no
`abandoned_*` symbol in it.

So what were the 1,988 bytes? The page sweep — retire every all-free page,
coalesce its span, release an empty segment — with the four freeing functions
(`span_free`, `segment_free`, `huge_free`, `prim::free`, 1,842 bytes as
separate symbols in 2.0.1) inlined into it once `adopt_segment` stopped being
their second caller. That sweep is the mechanism behind the P4e reclamation
fixes (the periodic collect, the reclaim-before-null); a firmware that never
frees never runs it, but a library cannot know that, exactly as §2 of the
first report said of lever 4. **Not a lever; closed permanently.**

It shrank anyway, to **1,381 bytes**, because two things it called folded
under `ONE_REGION` (§7.3): the segment map's `unregister` on segment release
and the arena's `chunk_free` probe. Structure kept, hosted machinery gone.

### 7.3 §3 — static RAM, and the predicate that turned 688 bytes into 2,768

The plan priced three tables at ~688 bytes. Reading them showed the same
shape behind all three, plus two more the plan did not list: they exist to
manage MANY OS ranges, and a chip has exactly one. One constant names that:

```rust
pub(crate) const ONE_REGION: bool = cfg!(all(ra_single_threaded, not(miri), not(windows), not(unix), not(target_arch = "wasm32")));
```

— the bare-metal arm of `ONE_THREAD`, i.e. "the `prim::fixed` backend is the
memory source". Five things fold on it, each at its entry point so the host
compiles both arms and the linker drops what a chip cannot reach:

| what | how it folds | image delta |
|---|---|---:|
| **options** (`VALUES`, 304 B `.data`; `ENV_PARSED`; the split-word atomics) | `get` returns the compiled-in default, folded at each call site; `set`/`set_default` are documented no-ops | −304 `.data` |
| **arenas** (`ARENAS`, 128 B `.bss`; `chunk_alloc_inner` 377 B; the reserve-on-miss path) | `chunk_alloc*` → `None`, `chunk_free*` → `false`, `reserve_*`/`manage_*` → `Err` | −128 `.bss`, the scan out of `malloc_generic_once` |
| **segment map** (`range_table::BASE`/`END`, 512 B `.bss`; register/unregister on every segment alloc and release) | `contains` = `prim::fixed::region_contains` (two compares); register/unregister no-ops | −512 `.bss` |
| **heap sentinel + empty page** (1,808 B `.data`: flash AND RAM) | `link_section = ".rodata.*"` on bare metal; `create_heap` copies the sentinel's heap from flash instead of materialising a second template | −1,808 `.data`, +1,808 −904 `.rodata` |
| **extent table** (`EXT_BASE`/`EXT_LEN`, 256 B `.bss`) | `--cfg ra_max_extents="8"` (or 16, 64); default 32 unchanged | −192 `.bss` at 8, opt-in |

Bricks, measured:

| brick | `.text` | `.rodata` | `.data` | `.bss` | flash | RAM (`.bss`+`.data`) | `.stack` |
|---|---:|---:|---:|---:|---:|---:|---:|
| 2.0.2 | 54,221 | 9,604 | 4,396 | 226,328 | 68,221 | | 104,644 |
| options + arenas + segment map | −2,808 | −388 | −304 | −656 | −3,500 | −960 | +960 |
| sentinels to flash | −248 | +904 | −1,808 | 0 | −1,152 | −1,808 | +1,808 |
| extents knob at default | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| **final (default knob)** | **51,165** | **10,120** | **2,284** | **225,672** | **63,569** | | **107,412** |
| **change** | **−3,056** | **+516** | **−2,112** | **−656** | **−4,652** | **−2,768** | **+2,768** |
| extents knob at 8 (opt-in, on top) | +24 | 0 | 0 | −192 | +24 | −192 | +192 |

The stack identity held on every row. The first brick alone took
`malloc_generic_once` from 1,950 to 542 bytes — the arena scan and the map
registration were most of what a chip's segment allocation did — and
`create_heap` from 966 to 363 once the option reads became immediates and the
template copy came from flash. Attributed code: **8,262 → 4,313 bytes, 36 →
27 symbols** (−47.8 %), on top of 2.0.2's −49.2 %.

The extents knob costs 24 bytes of flash at 8 because the compiler unrolls the
smaller table into `prim::alloc`; a firmware takes that trade for stack bytes
or leaves the default. Its doc states the bound the number must respect: a
free extent is bounded by live blocks or the region's ends, so the table can
never need more slots than live blocks plus one — five for this firmware's
three segments and a page, more than 64 for a 4 MiB PSRAM region.

**What a firmware loses, stated:** `arena::reserve_os_memory_ex`,
`manage_os_memory_ex` return `Err` on bare metal, and `options::set` is a
no-op there. Neither had a working meaning on a chip: an arena carved from
the one region only added a table in front of the same bytes, and an option
set at run time on a target with no environment had no known caller. Both are
in the CHANGELOG.

### 7.4 What it costs now, arm to arm

| section | `esp-alloc` | `rusty_alloc` (main) | delta | 2.0.2 | 2.0.1 |
|---|---:|---:|---:|---:|---:|
| `.text` | 50,337 | 51,165 | **+828** | +3,884 | +12,080 |
| `.rodata` | 7,724 | 10,120 | **+2,396** | +1,880 | +2,400 |
| `.data` | 2,300 | 2,284 | **−16** | +2,096 | +2,104 |
| `.bss` | 225,372 | 225,672 | **+300** | +956 | +988 |
| `.stack` | 107,696 | 107,412 | **−284** | −3,052 | −3,092 |

| budget | now | 2.0.2 | 2.0.1 |
|---|---:|---:|---:|
| **flash** | **+3,208** | +7,860 | +16,584 |
| **static RAM** | **+284** | +3,052 | +3,092 |
| **heap granule** (consumer's line) | **0** with `good_region_size` | 24,576 | 24,576 |

The `.rodata` row now carries the 1,808-byte sentinel template that used to
be counted in `.data`; the rest of it is unchanged (the bin table blob is
gone, the panic messages and source paths remain). `rusty_alloc` now uses
16 bytes LESS `.data` than `esp-alloc` and 284 bytes more static RAM in
total, against 3,092 two releases ago. The `.text` delta of 828 is still
smaller than the 4,313 bytes of attributed code for the reason the last
report gave: the `esp-alloc` arm carries ~2.4 KB of `Debug` formatting to
print its own statistics.

### 7.5 §5 — the speed claim was already on silicon; re-measured anyway

The premise of §5 is wrong, and it is worth being exact about how. The
README's 2.0–3.7x rows were never a hosted number: they come from
`espino/examples/blink-fs-p4` under `--cfg ra_bench`, on a XIAO ESP32-S3
Sense at 240 MHz, at the small-profile geometry, both arms from one source,
with the harness floor measured and subtracted, checksums compared, and a
null arm — the method line is in this repo's LEDGER (P4c/P4e) and the
README's "how the arms are kept honest". What the Janus firmware could not
supply, the espino harness already had.

What §5 was right about is that the published rows were stale. Both arms were
run again today, before and after this work:

| workload (ns per alloc/free pair, net of a 158/162 ns floor) | `esp-alloc` | 2.0.2 | this branch | README said |
|---|---:|---:|---:|---:|
| 32 B alloc/free | 1,638 | 587 | 586 | 647 |
| 64 mixed blocks, batched | 1,792 | 828 | 824 | 881 |
| churn: 64 live, random 8–512 B | 3,987 | 1,007 | 1,002 | 1,087 |
| 2048 B alloc/free | 1,638 | 1,129 | 1,133 | 1,200 |

Two readings. **2.0.2 was already 7–9 % faster than the README's rows**,
which were measured at 2.0.0 — the `ONE_THREAD` fold took a compare off
every free and the guarded probe off the generic path. **This branch is
neutral**: every row moves by at most 5 ns against a null arm that reproduced
to 1 ns, because everything it removed runs on segment allocation and
release, which a steady-state churn loop never reaches. Neutral is the right
answer and the gate against a regression. The README rows are refreshed to
today's numbers, and they carry the caution the previous report earned:
an allocator can move a compute benchmark by 8 % without executing an
instruction inside it, so kernel comparisons across allocators need their
addresses held constant.

### 7.6 Gates

fmt; clippy `-D warnings` on the host (all targets) and `riscv32imac`
`no_std` at the small profile, with and without `ra_max_extents`; the full
host suite on the default and small profile; the fixed-backend tests at both
geometries; the wasm ratchet (flat at 20,169, `ONE_REGION` excludes wasm);
`tools/gate-selftest.sh` with two more mutations (a wrong granule in
`good_region_size` goes red; `ONE_REGION` forced true on the host turns the
exclusive-arena test red). Board and rig numbers are local-only evidence, as
before.

### 7.7 Left on the table, named

- The 409-byte reentrancy message and 65-byte abort message in `.rodata`.
  Shortening them is ~350 bytes of flash at the price of the one diagnostic
  a firmware author cannot get any other way. Kept.
- The seven ~52-byte source-path strings (~370 bytes): the consumer's
  `-C remap-path-prefix`, as recorded last time.
- `collect_inner` (1,381) and `malloc_generic_once` (542) are now the two
  largest symbols and both are the allocator working. **The allocator is at
  its floor on this target for code**; what remains to a firmware is the
  granule, and §7.1 hands that back.

---

## 9. Consumer verification of 2.0.3, and one finding against `good_region_size`

Re-measured on the board, same firmware source, same five buffers.

### The numbers hold, two of them exactly

| | 2.0.1 | 2.0.2 | 2.0.3 | upstream claim |
|---|---:|---:|---:|---:|
| flash delta | +16,584 | +8,084 | **+3,392** | +3,208 |
| static RAM delta | +3,092 | +3,052 | **+284** | +284 |
| attributable code | 16,256 | 8,297 | **4,313** | 4,313 |
| symbols | 57 | 37 | **27** | — |

**Static RAM and attributable code match to the byte.** The 184-byte flash gap
is the consumer seam's own `Error` variants, the same residue as the previous
two releases, and not a disagreement. Against 2.0.1 this is -79.5 % flash and
-90.8 % static RAM, and since the `.stack` identity still holds exactly, the
stack hazard is now 284 bytes rather than 3,092.

### `good_region_size` is correct only for an aligned base, and nothing says so

`good_region_size(220 * 1024)` returns 200,704. Sizing a heap to exactly that
**killed the firmware** at startup:

```
MEM stage=boot used=0 free=200704 total=200704
handle_alloc_error / __rdl_alloc_error_handler / main
```

Segments must be `SEGMENT_SIZE`-aligned, and the natural way to hand this
crate a region on bare metal -- a `static` wrapping `[u8; N]` -- has
**alignment 1**. An unaligned base discards up to 65,535 bytes before the
first boundary, and a region sized to exactly `k * SEGMENT_SIZE + FIXED_PAGE`
has no slack to absorb it: two segments fitted where three were needed. The
220 KiB region it replaced survived only because its stranded 24,576 bytes
happened to cover the misalignment.

Aligning the region to 64 KiB fixes it, and the result is the strongest form
of the evidence:

| region, 200,704 bytes | stage=buffers |
|---|---|
| unaligned base | **panic in `handle_alloc_error`** |
| `#[repr(align(65536))]` | **`used=200704 free=0`** |

Set to the predicted floor, passing with nothing left over.

**Correction, same day.** An earlier draft of this section said the API does
not warn about alignment. That was wrong: `good_region_size`'s own doc states
the assumption plainly and points at `usable_bytes` as the exact check. The
defect is narrower and is written up separately in
`docs/plans/region-alignment-bug.md`: the **code example directly above that
paragraph** demonstrates the pattern that violates it, and `init_region`
computes the exact loss and then discards it, comparing only against zero.
Three options, cheapest first:

1. Say it in `good_region_size`'s own doc, next to the returned value.
2. Have `init_region` return `FERR_GEOMETRY` when the base is unaligned *and*
   the region is too tight to absorb it -- it already has both numbers, and it
   is the one place that sees the real address.
3. Ship the aligned container, so the consumer cannot get it wrong: a
   `#[repr(align)]` region type is four lines and removes the class.

### Kernels, across a region that both moved and shrank

| comparison | worst kernel delta |
|---|---:|
| esp-alloc to 2.0.1 | 7.698 % |
| 2.0.1 to 2.0.2 | 0.006 % |
| 2.0.3 at 220 KiB to 2.0.3 aligned at 196 KiB | **0.007 %** |

Every buffer moved and every kernel held, which refines section 8's placement
finding rather than contradicting it: the kernels are sensitive to a buffer's
offset within its page and bank -- fixed by the allocation sequence -- not to
its absolute address, which the region's base sets.
