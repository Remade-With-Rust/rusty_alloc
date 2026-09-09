# Bug: the documented `good_region_size` example is the one shape that fails

**Status: FIXED 2026-09-09 — see §7. All three fixes taken; the third one
structurally, because measuring the recommended shape first showed it cost
60,952 bytes of stack. The region is whole segments now, the descriptor is a
static, `Region<N>` is unpadded by construction, and the floor is 64 KiB.**

Originally: REPORTED 2026-09-09, by the Janus firmware. Severity: startup panic,
silent, and the failure gets *more* likely as the caller does the right
thing.

**A correction first, because it changes what the fix should be.**
`good_region_size`'s doc comment already states the alignment requirement:

> *Assumes the base is `SEGMENT_SIZE`-aligned, like `MIN_REGION`; an unaligned
> base loses up to `SEGMENT_SIZE - 1` bytes to the first boundary and
> `usable_bytes` on the real base is the exact check.*

That is correct, complete, and was in the release. So this is **not** an
undocumented assumption, and an earlier note from this consumer that said
"nothing says so" was wrong and has been corrected. The defect is narrower and
more awkward: **the code example directly above that paragraph demonstrates the
pattern that violates it.**

---

## 1. The example

```rust
/// static HEAP: Region<{ good_region_size(220 * 1024) }> = Region::new();
/// // 200,704 bytes: three 64 KiB segments plus the page, nothing stranded,
/// // and 24,576 bytes handed back to the firmware's own use.
```

`Region` there is the consumer's own container, not this crate's. Copied
literally — which is what a doc example is for — it yields three segments only
if that container happens to be `SEGMENT_SIZE`-aligned. The obvious
implementation, a `static` wrapping `[u8; N]`, has **alignment 1**, so the
linker puts it wherever it likes and the example silently produces two
segments instead of three.

The Janus seam's `Region` was exactly that, written before this API existed
and reviewed against the `MIN_REGION` doc, which carries the same assumption.

## 2. Reproduction, with the real numbers

XIAO ESP32-S3 Sense, `--cfg ra_single_threaded --cfg ra_small_profile`,
firmware needing 172,800 bytes live across five buffers.

| | value |
|---|---:|
| `HEAP` base the linker chose | `0x3fc8a1e4` |
| first `SEGMENT_SIZE` boundary above it | `0x3fc90000` |
| **discarded before the first segment** | **24,092 B** |

| region size | usable | segments | result |
|---|---:|---:|---|
| `good_region_size(220 * 1024)` = **200,704** | 131,072 | 2 | **panic** |
| the round `220 * 1024` = 225,280 | 196,608 | 3 | works |

```
MEM stage=boot used=0 free=200704 total=200704
...
handle_alloc_error / __rdl_alloc_error_handler / main
```

**The margin is 484 bytes.** The round number survived only because the 24,576
bytes it was stranding happened to exceed the 24,092 bytes of misalignment. So
the hazard is inverted from the usual: *the round, wasteful number is
accidentally safe, and calling the function that removes the waste is what
breaks it.* A user adopting `good_region_size` to fix a real 24 KiB loss —
which is why it was asked for — is the user who hits this.

Aligning the consumer's container to 64 KiB fixes it, and gives the strongest
form of the evidence at the same 200,704 bytes:

| base | `stage=buffers` |
|---|---|
| `0x3fc8a1e4`, alignment 1 | **panic in `handle_alloc_error`** |
| `0x3fc90000`, `#[repr(align(65536))]` | **`used=200704 free=0`** |

## 3. Why `init_region` does not catch it, though it could

It already computes the exact answer and then throws it away:

```rust
if usable_bytes(base, len) == 0 {
    return Err(FERR_GEOMETRY);
}
```

`usable_bytes(0x3fc8a1e4, 200_704)` returns **131,072**. Not zero, so the
region is accepted, and the caller is never told that the size it carefully
computed bought two thirds of what the number implies. `init_region` is the
one place in the system that sees the real base **and** the length, and it is
the only place that can diagnose this before the first allocation fails.

## 4. Fixes, cheapest first

1. **Make the example correct.** Show the alignment in it, since a reader who
   copies the block gets the bug:
   ```rust
   #[repr(align(65536))]              // or the crate's own aligned type
   struct Heap([u8; good_region_size(220 * 1024)]);
   ```
   One edit, removes the trap for everyone who copies rather than reads on.

2. **Report the loss from `init_region`.** It has both numbers. Either return
   the usable figure so the caller can compare it against `len`, or refuse
   with a distinct code when misalignment costs a whole segment — that is
   exactly the class where the caller believed the size was exact. A warning
   channel would do; the point is that the diagnosis exists at that moment and
   is discarded.

3. **Ship the aligned container.** The strongest option, because it removes
   the class rather than documenting it. A `#[repr(align)]` region type in
   `prim::fixed` is a few lines, and then `good_region_size`'s assumption is
   guaranteed by construction rather than by the consumer reading a paragraph.
   Note the default 32 MiB geometry cannot be aligned in BSS, so this is
   naturally a `ra_small_profile` type — which is the profile every firmware
   runs anyway.

## 5. A test worth having

The existing suite proves `usable_bytes` handles unaligned bases; nothing
proves the *pair* behaves. Both halves fail today:

```rust
// good_region_size promises "nothing stranded". At an unaligned base it
// cannot deliver that, and the caller currently cannot tell.
let base = 0x3fc8_a1e4;                       // as the linker placed it
let n = good_region_size(220 * 1024);          // 200,704
assert_eq!(usable_bytes(base, n), 131_072);    // two segments, not three
assert!(usable_bytes(base, n) < n - FIXED_PAGE);
```

The property to pin, whichever fix is taken: **for any base, a region sized by
`good_region_size` either delivers the segments its name implies, or the
caller is told it did not.** Today neither holds.

## 6. What the consumer did

`Region` is now `#[repr(align(65536))]`, `HEAP_BYTES` is
`good_region_size(220 * 1024)` checked by a `const` assert, and the firmware
runs at `used=200704 free=0` — the budget set to the predicted floor and
passing with nothing spare, which also recovers the 24,576 bytes the round
number was losing.

No urgency implied. The workaround is four characters of attribute and this
consumer is unblocked; the report exists because the next firmware to adopt
`good_region_size` will copy the same example.

---

## 7. Resolution (2026-09-09)

All three fixes in §4 are taken, the test in §5 is in, and — reading the
siblings of fix 3 before writing it — the recommended shape turned out to be
the most expensive of the three configurations on the board. So the fix is
structural rather than the one this report asked for, and §7.2 is the part
worth reading twice.

Same rig as the previous three reports (the `xiao-s3-probe` firmware in a
scratch copy, `[patch.crates-io]` to the working tree, `size -A` and
`nm -S --size-sort` on the linked ELF), plus the espino `blink-fs-p4` harness
on the XIAO for the footprint floor.

### 7.1 §4.1 and §4.2, as asked

- **The example is correct now**, and it no longer shows a container of the
  reader's own at all: it shows [`Region`], which carries the alignment.
- **`init_region` reports the loss** — it refuses it. The predicate is the
  two numbers §3 said it already had: `usable_bytes(base, len)` on the real
  base against `usable_bytes(0, len)` on an aligned one. When the base costs
  a whole segment against what the length promises, the caller's model of the
  size is wrong for that address, and the answer is `FERR_MISALIGNED` (0xF141)
  before anything is registered. The round 220 KiB region at the report's
  base passes — it strands 28,672 aligned and loses 24,092 misaligned, the
  same three segments, no claim of exactness broken — and the exact size at
  that base is refused. The §5 test pins the report's numbers
  (`usable_bytes(0x3fc8_a1e4, n)` = 131,072, two segments not three) and
  drives the refusal through a real slice at the report's residue, `0x1e4`
  past a boundary; `tools/gate-selftest.sh` proves that removing the
  comparison turns it red.

### 7.2 §4.3, and what the sibling measurement said about it

The report's workaround, and its recommended fix, was `#[repr(align(65536))]`
around the exact size. A type's size is rounded up to its alignment, so that
container is not 200,704 bytes: it is 262,144. Measured on the rig, from the
consumer's own seam as it stood after §6:

| region declaration | `HEAP` in `.bss` | `.stack` left | usable |
|---|---:|---:|---:|
| round 225,280, alignment 1 (2.0.2) | 225,281 | 107,412 | 196,608 |
| **`#[repr(align(65536))]` around 200,704 (§6, the workaround)** | **262,144** | **46,460** | 196,608 |
| `Region<196_608>` from this crate (§7.3) | 196,608 | 110,240 | 196,608 |

The workaround cost **60,952 bytes of stack** against the round number it
replaced — 36,863 of padding inside the type and the rest in the linker's
alignment gap — for the same 196,608 usable bytes. The report's own §9 table,
which reads +284 bytes of static RAM for 2.0.3, predates §6's change; the
number after it is the middle row. §3 was right that the granule was the
dominant cost; the aligned exact container did not remove it, it moved it
into padding and made it larger.

The reason is the `+ FIXED_PAGE`. Every sizing rule this crate shipped —
`MIN_REGION`, `usable_bytes`, `good_region_size`, the README's
`k * 64 KiB + 4 KiB` — reserved one page of the region for the first heap's
descriptor, so an exact region was never a whole number of segments, so an
aligned container of it was always padded to the next one.

### 7.3 The fix: whole segments, and the descriptor in a static

`create_heap` on a one-region target now takes the FIRST heap's descriptor
from a static of the fixed backend's (`prim::fixed::take_first_heap_box`,
1,752 bytes of `.bss` on the ESP32-S3) instead of a page of the region. With
no page to reserve, every sizing rule becomes whole segments:

| | 2.0.3 | now |
|---|---|---|
| `MIN_REGION` | `SEGMENT_SIZE + FIXED_PAGE` | `SEGMENT_SIZE` |
| `usable_bytes(0, len)` | `⌊(len − 4096) / SEG⌋ · SEG` | `⌊len / SEG⌋ · SEG` |
| `good_region_size(220 KiB)` | 200,704 | **196,608** |
| `region_for(192 KiB)` | 200,704 | **196,608** |

And `prim::fixed::Region<N>` is the container: `SEGMENT_SIZE`-aligned, `N`
checked at compile time to be a whole number of segments, so
`size_of::<Region<N>>() == N` — a `const` assertion in the crate says so —
and `give(&'static self) -> Result<usize, PrimError>` hands the bytes over
once and returns what the allocator can serve, which for this type is `N`.
`FERR_MISALIGNED` cannot occur through it. The flag that makes `give`
single-shot is one module-wide static, not a field: a one-byte field beside
a segment-aligned array rounds the type back up to the next segment
(measured while writing it — `Region<196_608>` with the flag inside was
262,144 bytes, the exact defect of §7.2 reintroduced).

On the rig, the probe firmware declared through `Region<{ good_region_size(220 * 1024) }>`:

| | consumer's §6 declaration | `Region` | delta |
|---|---:|---:|---:|
| `.bss` | 262,532 | 198,752 | **−63,780** |
| `.stack` | 46,460 | 110,240 | **+63,780** |
| flash | 63,533 | 63,517 | −16 |
| usable | 196,608 | 196,608 | 0 |

The 1,752-byte descriptor static and its flag are inside that `.bss`; the
allocator's attributed bytes read 4,313 → 6,170 for them, which is RAM the
region no longer gives up as a page, not new cost. Against the original
round unaligned region the stack gains 2,828: the linker still places an
aligned static after an alignment gap, and `size -A` counts that gap in no
section. It is at most one segment less one byte, it depends on what else
the linker puts in `.bss`, and it is the one cost this crate cannot remove
from outside the linker script.

### 7.4 The floor, re-measured

The README's 68 KiB floor was `64 KiB + 4 KiB`: one segment and the page.
With the page gone, the espino footprint sketch (the workload the README's
floor row has always meant) was run on the board with its region declared as
`Region<{ 64 * 1024 }>`, one whole segment:

```
[heap] region given: 65536 usable of 65536
[heap] boot:     region used 0     free 65536 total 65536 | live 0 PEAK 0
[heap] after fs: region used 65536 free 0     total 65536 | live 0 PEAK 4914
[heap] end:      region used 65536 free 0     total 65536 | live 0 PEAK 4914
```

Same 4,914-byte peak as the 68 KiB run, nothing refused, ran to the end.
**The floor is 64 KiB of region plus the 1,752-byte descriptor static**, and
the container declaring it is exactly 65,536 bytes.

### 7.5 Gates

fmt; clippy host all-targets (default and small profile) and `riscv32imac`
`no_std` at both geometries; the full suite on default and small profile;
the fixed-backend tests at both geometries; `tests/region.rs`, a separate
process so `give` can succeed and be asserted; the wasm ratchet; the unsafe
census banked with the new sites in `UNSAFE.md` (two `Sync` impls, the
handoff's `&mut`, and the tests' in-place allocations); `tools/gate-selftest.sh`
with a ninth mutation. Two things the tests taught on the way, both recorded
in the test bodies: a `Region` constructed with `Box::new` faults on Windows
because the 64 KiB-aligned value is materialised on the stack past the guard
page (`Box::new_zeroed().assume_init()` allocates it in place, and zero is a
valid `Region`), and the first draft of the type had the flag as a field and
failed its own no-padding assertion at the default geometry.

### 7.6 What the consumer changes

Two lines in the Janus seam, and the workaround goes away with its padding:
drop the seam's own `Region` for `pub use rusty_alloc::prim::fixed::Region;`,
and declare `Region<{ good_region_size(220 * 1024) }>` — 196,608 now, three
whole segments, `size_of` exactly that. `give` returns the usable byte count
where the seam's returned `()`; the seam's `Error` mapping gains
`FERR_MISALIGNED` if it wants a name for a refusal its own type can never
produce.
