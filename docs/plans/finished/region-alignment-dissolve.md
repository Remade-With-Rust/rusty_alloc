# The alignment gap is not a bug — but wasm already showed how to remove it

**Status: DONE 2026-09-09 — taken, measured, and routed through a knob. §7.**
Proposed 2026-09-09 by the Janus firmware. One item, with precedent inside
this crate.

The 24,148-byte gap measured on 2.0.4 is **not a defect**. It is the price of
a design decision, correctly implemented, and 2.0.4 is the best build this
consumer has run. This is a proposal to remove that price on one target,
because the crate has already removed it on another for exactly the same
reason.

---

## 1. Why the gap exists

`segment_of` recovers a block's segment header with a mask:

```rust
p.with_addr(p.addr() & !(SEGMENT_SIZE - 1)).cast()
```

That is why segments must be `SEGMENT_SIZE`-aligned, why the region holding
them must be too, and therefore why the linker pads up to the next 64 KiB
boundary. On a fixed RAM map that padding belongs to no section and is
invisible in `.bss` — it showed up only as the section sum failing to
reconcile (335,368 unaligned against 311,220 aligned).

Everything in that chain is working as designed. The mask is one instruction
on the hottest path in the allocator, and the note above it records that a
GEP form measured **+1.00 Ir on every malloc and free**, with "do not retry"
attached. This proposal takes that seriously.

## 2. The precedent: wasm already dissolved it

From `segment.rs`, the wasm arm of the same function:

> *segments are SLICE-aligned, not `SEGMENT_SIZE`-aligned (F2,
> `docs/plans/segment-tax.md`), so the mask cannot recover the base — the
> slice-granular table in `segment_map` does. One load; this platform's free
> path is plain Rust... so the table lookup is the whole cost of **dissolving
> the alignment constraint that made every ragged reservation strand its
> tail**.*

"Every ragged reservation strands its tail" is precisely what this consumer
measured on the fixed backend. The problem is identical, the argument for
paying a little on `free` is identical, and the trade was already judged worth
it once.

**The bare-metal case is stronger than the wasm case on every axis:**

| | wasm | fixed backend |
|---|---|---|
| free path | plain Rust, single-threaded | plain Rust, single-threaded |
| scarce resource | bundle size | **RAM — the binding constraint** |
| cycles available | shared with a browser | a dedicated 240 MHz core |
| waste removed | ragged reservation tails | up to `SEGMENT_SIZE - 1` per region |

## 3. The proposal, and it is cheaper than wasm's

wasm needs a table because it has many ranges. `ONE_REGION` has exactly one,
at a base the backend already stores — so no table and **no load** is needed,
only arithmetic relative to that base:

```rust
#[cfg(ONE_REGION-ish)]
pub fn segment_of(p: *mut u8) -> *mut Segment {
    let base = REGION_BASE;                       // already a static
    let off  = p.addr() - base;
    p.with_addr(base + (off & !(SEGMENT_SIZE - 1))).cast()
}
```

One subtract and one add over the existing mask. Segments are then carved at
`SEGMENT_SIZE` strides *from the region base* rather than from zero, and the
region needs no alignment at all.

The infrastructure is already in place: `segment_map` has four `ONE_REGION`
fast paths, and 2.0.3 already reduced `contains` to two compares against the
region bounds under it.

**What it buys, measured on this board:**

| | |
|---|---:|
| RAM returned | **24,148 bytes** (up to 65,535 in general) |
| `Region`'s `#[repr(align)]` | no longer needed |
| the whole misalignment failure class | gone, including `FERR_MISALIGNED` |
| `good_region_size` | becomes exact rather than conditional on a base |

That last one matters more than the bytes. Every sharp edge this consumer has
reported in four releases — the silent short region, the startup panic, the
padded container, the invisible gap — descends from the same requirement.
Removing it removes the class, rather than adding another guard to it.

## 4. What must be measured before believing any of it

**This is a proposal, not a result, and the crate's own history says why.**
The `+1.00 Ir` note on this exact function means the free path is
instruction-counted, and two ALU ops is a real change to it. So:

- Price it on the **callgrind harness**, not a board. A chip's clock cannot
  resolve two instructions on a free, and this consumer's firmware allocates
  five buffers once — the wrong shape entirely, as established before.
- Gate it on `ONE_REGION` only. Hosted builds keep the mask and must come out
  byte-identical, exactly as the `ONE_THREAD` predicate did.
- The trade is profile-dependent and should be stated that way: **two
  instructions per free, against up to 64 KiB of RAM.** On a chip where RAM
  binds and cycles do not, that is clearly worth it. On a server it clearly
  is not — which is why it belongs behind the bare-metal predicate rather
  than being offered as a general improvement.

If the instruction count moves more than the wasm arm's table lookup did, the
honest answer is to keep the mask and keep `Region`'s alignment. This
consumer's 24 KiB is not worth a regression on every other target.

## 5. What does NOT need more work

- **Code size is essentially done.** 2.0.4 is 6,170 bytes across 30 symbols,
  from 16,256 at 2.0.1. The largest item is `FIRST_HEAP_BOX` at 1,752 bytes,
  which is the heap descriptor deliberately moved out of the region in 2.0.4 —
  a design choice, not slack. There is no fourth halving here.
- **`collect_inner`** is now 1,381 bytes, down from 1,988 without being
  targeted. The question this consumer raised about it in
  `firmware-what-is-left.md` §2 is worth answering for its own sake, but it is
  no longer a large line.
- **The alignment bug itself** is fixed and its report is closed. This is a
  successor, not a reopening.

## 6. If it is not taken

That is a reasonable outcome and nothing is blocked. `Region` is correct,
`free=0` is achievable, and the gap costs 24 KiB on a part with 512 KB. The
value of writing it down is that the *reason* is now recorded: the gap is the
mask's price, the mask is one instruction, and the crate has already decided
once — on wasm — that this particular instruction is worth trading away when
the platform's scarce resource is space rather than time.

---

## 7. Resolution (2026-09-09)

**Taken.** `crate::REGION_STRIDES` (the fixed backend, unless
`--cfg ra_aligned_region`): segments stride from the region's base,
`segment_of` masks the offset from it, the backend measures every alignment
from that base, `Region<N>` is `repr(align(16))` (`REGION_ALIGN`, new), and
the sizing rules run up to 16 bytes instead of a segment. `FERR_MISALIGNED`
stays: a base off the 16-byte grid with an exact length still loses its last
segment to the run-up (the report's own `0x3fc8a1e4` is 4-aligned and loses
12 bytes, which is a segment against 196,608) — `Region` cannot produce one,
so §3's "the whole failure class" is fifteen sixteenths true. Hosted builds
are unchanged to the instruction (x86-64 asm diff: debug records and one
symbol name).

**§4, measured as it asked.**

| | 2.0.4 (mask) | strides | strides + §7.1 fix | knob + §7.1 fix |
|---|---:|---:|---:|---:|
| probe `.stack` | 110,240 | **134,384** | 134,384 | 110,240 |
| probe section sum, unaccounted | 311,220 / 24,148 | 335,364 / 4 | 335,364 / 4 | 311,220 / 24,148 |
| `HEAP` address | `0x3fc90000` | `0x3fc8a1b0` | `0x3fc8a1b0` | `0x3fc90000` |
| free path, instructions (`dealloc`) | 37 | 42 | **39** | 36 |
| board, ns/pair: pingpong | 586 | 603 | **595** | 578 |
| batch | 824 | 845 | **833** | 820 |
| churn | 1,002 | 1,023 | **1,011** | 997 |
| 2048 B | 1,133 | 1,142 | **1,134** | 1,125 |

The callgrind harness cannot run the bare-metal arm (`REGION_STRIDES` is
false on every hosted target by construction), so the count is from the
objdump of the shipped ESP32-S3 image and the price from the board, which
resolves it: +5 instructions and +17–21 ns for strides alone, against §3's
estimate of two ALU ops. The mask is `l32r 0xffff0000; and`; the stride is
`l32r &REGION_BASE; l32i; sub; l32r 0xffff; and; sub`, the first `sub`
shared with the page index. (The esp backend does not emit `extui` for the
16-bit mask; one instruction, not worth a profile-specific cast.)

**§7.1, the sibling finding.** Reading that disassembly, both arms carried a
dead `l32i` + `memw` on every free: `(*seg).thread_id.load(Acquire)`, read
before `ONE_THREAD` folded the compare it fed. LLVM keeps an unused acquire.
Gated the load itself: −3 instructions and −8 ns per pair on both arms, so
the shipped default lands within 9 ns of 2.0.4 on every row and the mask arm
under it. `alloc.rs`, hosted unchanged.

**§4's threshold, answered.** "More than the wasm arm's table lookup" — yes,
by about one instruction, and one of the three is a data load. So the trade
is not decided once: strides are the default because RAM binds on this part
(24 KiB of 512), and **`--cfg ra_aligned_region`** is the mask, the segment
alignment and the gap, for a firmware whose alloc/free rate binds instead.
Every sizing rule, the backend's placement, `Region`'s alignment and the
tests follow the flag; CI tests the knob arm on the small profile.

**What else moved.** `FIXED_REGION` (the prim is `prim::fixed`) now carries
the arena fold that `ONE_REGION` used to, since chunks on absolute segment
boundaries are not a strided region's segments; `huge_alloc`'s slack,
`malloc_aligned`'s natural-fit bound and `link_is_plausible` measure from
the base under strides; `tests/region.rs` is a plain `static Region<N>`,
the firmware shape, which a segment-aligned type could not be on every host
toolchain; the fixed-backend test carves its region 0x1f0 past a 64 KiB line
on purpose. Gate-selftest 10/10 (new: `FIXED_REGION` forced true on a host
refuses arenas). Full record: `docs/LEDGER.md`, "REGION ALIGNMENT DISSOLVED".

---

## Consumer validation of 2.0.5: the gap is gone and the RAM reconciles

Taken, shipped, and verified on the board that reported it.

RAM on this part is fixed, so the sections must sum to a constant. That sum is
the instrument the whole finding rested on, and it now closes:

| build | `.data` | `.bss` | `.stack` | sum | unaccounted |
|---|---:|---:|---:|---:|---:|
| esp-alloc @196,608 | 2,300 | 196,700 | 136,368 | 335,368 | 0 |
| 2.0.4, segment-aligned | 2,228 | 198,752 | 110,240 | 311,220 | **24,148** |
| **2.0.5** | 2,228 | 198,752 | **134,384** | **335,364** | **4** |

**24,148 bytes down to 4, `.stack` +24,144, `.bss` byte-identical** — matching
the changelog's figure exactly. Both halves of the 2.0.4 report are confirmed
by the fix: the memory was real, and no `.bss` delta could ever have shown it.

Also unchanged where it should be: `used=196608 free=0` at every stage,
`good_region_size(220 * 1024)` still 196,608 with the consumer's `const`
assert still building, 31 host tests passing. `.text` grew 72 bytes for the
stride arithmetic.

### The per-free cost is yours, and this consumer will not confirm it

The changelog prices this at three instructions per `free` (39 against 36) and
9-17 ns per alloc/free pair. **Not reproduced here, and it should not be
quoted as though it were.** This firmware allocates five buffers once and
never frees; it cannot observe a per-free cost, which is the same limitation
that made it the wrong rig to price the hot path in the first place. The
callgrind harness and your own board run are the evidence for that number.

What this firmware can say is that the trade is right for its shape: RAM binds
on an ESP32-S3 and cycles do not, `--cfg ra_aligned_region` is there for a
firmware that disagrees, and hosted builds are untouched.

### One last placement data point

The region's base moved, so every buffer did. `rgb888_to_rgb565` gained 3.6 %
and ran 16 repetitions instead of 15; `downscale2x_gray8` came back
bit-identical; the rest sat within 0.01 %. Sixth instance of the same effect
across five releases, and still no allocator call inside any measured kernel.

### Where the swap now stands on this firmware

| | esp-alloc | 2.0.1 | **2.0.5** |
|---|---:|---:|---:|
| flash delta | — | +16,584 | **+3,404** |
| stack cost | — | 3,092 | **4** |
| region stranded | — | 24,576 | **0** |

The footprint argument against adopting this on a small firmware has
essentially gone in five releases. What remains is ~3.4 KB of flash, and the
reason to pay it is the double-free abort rather than speed on a workload of
this shape — which is what the README already says.
