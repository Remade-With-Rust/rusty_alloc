# The small-path step — a 16% cliff at 512 bytes that only exists on bare metal

**Status:** RESOLVED 2026-09-10 — see §8, and §8.7 for the DEFECT the
report led to: the small profile extended every page one block at a time. The attribution was wrong and the
correction is the finding. Originally: measured, nothing implemented · **Date:** 2026-09-10 · **Build:**
`=2.1.0` from crates.io, unmodified · **Cfgs:** `ra_single_threaded`,
`ra_small_profile` · **Boxes:** ESP32-S3 DevKit (Xtensa LX7, 32-bit,
`prim::fixed`) with `CCOUNT`; and x86-64 Windows (`prim::windows`) with
`rdtsc`, pinned to one core at High priority

Filed by the Kairos project (FreeRTOS remade in Rust), which uses
`rusty_alloc` behind its `heap_3` seam. Everything here is reproducible from
two cells and one standalone binary, all listed in §6.

This is a **map, not a claim**. It answers one question — *why does one
`alloc`+`free` cost 16% more below 512 bytes than above it* — reports what
that costs against the incumbent, and is explicit about the part it could
not answer and the run that would.

---

## 0. The answer in one paragraph

On a microcontroller, one `alloc` + one `free` steps **314 → 271 cycles/op
between a 512-byte request and a 513-byte one**, and **271 → 933 between
2,048 and 2,049**. Both boundaries are your own constants under
`ra_small_profile` (`SMALL_OBJ_SIZE_MAX` = 512, `MEDIUM_OBJ_SIZE_MAX` =
2,048); the totals are byte-identical *within* each range and nothing steps
anywhere else, so this is **routing**, not a gradual effect and not
fragmentation. **The lower step does not reproduce on a 64-bit host**, where
the same sweep is flat across 512 *and* across 1,024 and runs at **13
cycles/op against the device's 314**. So it is not a generic geometry effect,
and it is not the `direct[]` table (whose top is 1,024 on 64-bit and shows no
step either) — it is specific to the configuration a firmware actually uses,
which points at **`prim::fixed`**. It costs something concrete: measured
against FreeRTOS's `heap_4` on the same part, `rusty_alloc` is **2.4× faster
at 16 bytes and 1.27× slower at 256–512** — and 256–512 is *the only range it
loses*, so closing this step would turn the one loss into a win.

---

## 1. Method, and what makes a number admissible here

**The counter is real.** Xtensa `CCOUNT` increments once per CPU cycle. This
matters because the obvious alternative does not exist: QEMU was measured six
ways to supply no cycle, latency or work counter at all (DWT unimplemented,
SysTick tracking host wall time and *shrinking* as work grows, nothing under
`-icount`), so every number here is from silicon.

**A null arm is subtracted.** An identical empty round — same loop, same
`black_box`, same counter reads — is measured the same way and taken off:
16 cycles/op on the device, printed so a reader can see how much of a small
figure is floor.

**Best of 32 rounds of 256 ops, after 8 warm-up rounds.** The floor is what
survives an interrupt landing mid-round; the warm-up pulls the code from
flash into the instruction cache, and measuring that would measure the flash
controller.

**Work is checked, not assumed.** Every round keeps a checksum, so a compiler
that removed the allocation would show as a changed checksum rather than as a
suspiciously good number.

**The table is a function of size, and that is proven rather than hoped.**
Every size is measured twice, ascending and descending, and the cell fails if
one does not reproduce its own total. All seventeen sizes up to 2,048
reproduce **to the cycle**. (The one exception is quarantined: 4,096 —
`FIXED_PAGE`, so it cannot come from a page — read 789, 861 and 933 cycles/op
in three runs differing only in what ran before it. It is not a function of
size, is declared an expected asymmetry, and is not quoted anywhere.)

---

## 2. The step, tested by one byte

The plateaus are **not** the bin geometry — 160, 192, 224, 256, 288, 320, 384
and 512 are eight distinct bins and every one of them produces the identical
total of 84,498 cycles. They land instead on the page-kind boundaries, which
makes a prediction that can be wrong by one byte, so it was tested that way:

```text
     size    total      per op   route
      256    84498         314   small
      511    84498         314   small
      512    84498         314   small
      513    73490         271   MEDIUM   <- steps here
      640    73490         271   medium
     1024    73490         271   medium
     2047    73490         271   medium
     2048    73490         271   medium
     2049   242979         933   own large span   <- and here
     2560   242979         933   own large span
```

**43 cycles for one byte at 512; 662 for one byte at 2,048.** Requests one
byte apart cannot differ for any other reason — same alignment, no bin
between them, and the whole table already shown to be a function of size.

---

## 3. It does not reproduce on a host, and that is the useful part

`tools/alloc-route-probe` (§6) runs the same sweep on x86-64 against the same
crate version and the same two cfgs. On 64-bit the two candidate constants
come apart — `SMALL_SIZE_MAX` is `128 × 8` = 1,024 while `ra_small_profile`
keeps `SMALL_OBJ_SIZE_MAX` at `4096/8` = 512 — so a step would name its own
cause. Neither does:

```text
     size   cycles/op
      511          13
      512          13     == SMALL_OBJ_SIZE_MAX
      513          13
     1023          12
     1024          12     == SMALL_SIZE_MAX (direct[] top)
     1025          11
     2048          11     == MEDIUM_OBJ_SIZE_MAX
     2049          91                      <- the only step
```

Three things follow.

1. **`MEDIUM_OBJ_SIZE_MAX` routes on both platforms.** Confirmed twice,
   independently. That one is not in question.
2. **The 512 step is not `SMALL_SIZE_MAX`.** On this host that constant is
   1,024 and there is no step there.
3. **The whole host sweep runs at 11–13 cycles/op against the device's
   271–314.** Even allowing for a slower core, 24× is not a core-speed
   ratio — it is a different path. The host is hitting a fast path the
   device is not.

The variable is the **prim**: a firmware gets `prim::fixed`, a host gets
`prim::windows`/`prim::unix`, and the prim is selected by target OS rather
than by a feature, so it cannot be swapped on a host to isolate it. That is
where we ran out of road, and §5 says what would finish it.

**A hypothesis, labelled as one.** `alloc.rs` already records that "a tight
alloc/free loop frees into `local_free`, so the queue front's `free` list is
ALWAYS dry when the next allocation arrives" — and this harness *is* that
loop. If the device is taking `malloc_generic` on most operations while the
host is not, the 24× gap and the step would be the same fact seen twice. We
could not confirm it: nothing on the seam exposes a slow-path counter.

---

## 4. Four models written down first, all four refuted

Recorded so nobody re-runs them.

| model | refuted by |
|---|---|
| bin geometry | the plateaus span **eight** distinct bins with identical totals |
| collect frequency (`FIXED_PAGE / good_size`) | monotone in size; the cost is not |
| a per-page cost amortised over blocks | fits 16…256 at ~0.875 cycles/byte, then breaks completely at 512 |
| blocks-per-page *within* a route | 256 and 512 share a page size, have 16 and 8 blocks, and cost the **same** 314 |

---

## 5. What we could not measure, and what would

**We could not see a page kind from outside.** The natural footprint answer —
a small page is one slice (4 KiB), a medium page four (16 KiB), so a 513-byte
allocation should claim 4× the region a 512-byte one does — is not observable
through the seam. `region_stats()` answers over region **extents**, so it
moves when a whole segment is claimed and not when a page is; the probe that
asked read 0 for all six sizes and was deleted rather than shipped.

Two gaps, both of which a firmware would need to tune around this at all:

- **no per-allocation usable size** on the fixed-region API (`usable_size`
  exists in `alloc.rs`, but a firmware that reaches past its seam to get it
  has defeated the seam), and
- **no way to observe which page kind, or how many region bytes, a request
  costs.**

**The run that would finish it** is inside your tree, not ours: the same
sweep against `prim::fixed` on a host, which would isolate the prim from the
architecture in one step. From outside the crate it cannot be built, because
the prim is chosen by `cfg(unix)`/`cfg(windows)`.

---

## 6. Reproducers

| what | where | needs |
|---|---|---|
| the one-byte step, on silicon | Kairos `rusty_rtos_core/firmware/esp32s3-devkit-alloc-cycles` | an ESP32-S3 on USB |
| the host sweep, self-contained | Kairos `tools/alloc-route-probe` — one binary, `cargo run --release`, no Kairos dependencies | nothing |
| the `heap_4` A/B | Kairos `rusty_rtos_core/firmware/esp32s3-devkit-alloc-ab` | an ESP32-S3 on USB |

---

## 7. Why 16% is worth your time

Measured on the same part against FreeRTOS's `heap_4` — compiled **verbatim**
from `FreeRTOS-Kernel/portable/MemMang/heap_4.c`, run under the identical
harness, both allocators given the same 64 KiB, both called directly at the
same 8-byte alignment, with `vTaskSuspendAll` stubbed to a no-op so the C arm
pays no lock the `ra_single_threaded` Rust arm does not:

| request | rusty_alloc | heap_4 | |
|---|---:|---:|---|
| 16 B | 99 | 236 | **2.38× faster** |
| 32 B | 109 | 236 | **2.17× faster** |
| 64 B | 141 | 236 | **1.67× faster** |
| 128 B | 193 | 236 | 1.22× faster |
| 256–512 B | 298 | 236 | **1.27× slower** |
| 1024–2048 B | 255 | 236 | 1.08× slower |

The null A/B — the Rust arm run against *itself*, presented to the harness as
two different allocators — came back **37,963 vs 37,963**, so the resolution
floor is **zero cycles** and every difference above it counts. (These are the
64 KiB, direct-call numbers; they differ from §2's by a **constant 16
cycles** at every size, which is the harness difference — `Vec` versus a raw
call — with the plateau structure identical.)

**The only range `rusty_alloc` loses is the expensive route.** Put 256–512 on
the 255-cycle path and the loss becomes a win.

**And for completeness, because a flat `heap_4` number is its best case and
should not be quoted alone:** that 236 is flat because the workload keeps one
block live, so heap_4's free list is one entry and first-fit answers in one
step. At 512-byte requests against a free list of 16-byte holes it degrades
**linearly at ~27 cycles per entry walked** — 235 → 466 → 1,114 → 3,706 at 0,
8, 32 and 128 holes — while `rusty_alloc` stays flat at 297. Different floor
functions; the crossover is about two holes. On a heap that fragments, which
is every long-running RTOS heap, `rusty_alloc` wins by 12.5× at 128 holes.
**That is the headline, and this document is about the one place it does
not.**

*(Our own first fragmentation probe was refuted and it is worth passing on:
holes of the **same** size as the request made heap_4 **faster**, 236 → 218,
because first-fit stops at the first block that fits. The holes have to be
smaller than the request.)*

---

## 8. Resolution (2026-09-10) — it is not the prim, it is the POINTER WIDTH

Taken seriously, reproduced, and re-attributed. The report is right that there
is a real step, right that it is routing, and right about where it hurts. The
one thing it gets wrong is the cause, and the reason is worth more than the
fix.

### 8.1 The step reproduces on a HOST — a 32-bit one

`SMALL_SIZE_MAX` is not a profile constant. It is

```rust
pub const SMALL_SIZE_MAX: usize = SMALL_WSIZE_MAX * INTPTR_SIZE;   // 128 * size_of::<usize>()
```

so it is **1,024 on a 64-bit host and 512 on a 32-bit chip**, where it lands on
the same byte as `SMALL_OBJ_SIZE_MAX`. §3's refutation — "on this host that
constant is 1,024 and there is no step there" — is testing a machine on which
the suspect is not at the scene. Re-run with the pointer width as the only
variable, same crate, same cfgs, `prim::windows` in **both** arms:

| arm | 256 vs 264 (control) | **512 vs 513** | 2048 vs 2049 |
|---|---:|---:|---:|
| x86-64 host | −0.9 % | **+0.1 %** | −81.9 % |
| **i686 host** | −0.4 % | **+8.6 %** | −80.6 % |

ABBA-interleaved, 50,000 ops per arm, min of 40 blocks. The control says the
instrument resolves well under 1 %, so +8.6 % is twenty times the floor.

**`prim::fixed` is exonerated.** The step appears on the OS prim as soon as the
pointer is 32 bits, and disappears on the same binary at 64. §5's "the run that
would finish it is inside your tree" turned out to be a run *outside* it:
`cargo build --target i686-pc-windows-msvc`, no bare-metal prim required.

### 8.2 The mechanism, counted rather than timed

`alloc::stats()` is public and carries the slow-path counter §3 wanted. Per
100,000 alloc+free pairs, one live block:

| route | `generic`/op | pages carved | extends | retires |
|---|---:|---:|---:|---:|
| `direct[]` (`size <= SMALL_SIZE_MAX`) | 1.0000 | **195** | 195 | 195 |
| bin peek (just above it) | 1.0000 | **0** | 0 | 0 |

So the slow path is **not** the difference — both routes take it on every
operation, which also settles §3's hypothesis: `malloc_generic` is universal
here, not a device effect. The difference is that the `direct[]` route
**retires its page and carves a fresh one every ~513 operations** and the bin
route never does. `100_000 / 512 = 195`: that is
`GENERIC_COLLECT_DEFAULT`, the periodic `collect_inner` sweep, which is 512 at
the small profile. In a loop that keeps one block live the page is empty at
every sweep, so every sweep costs a carve, an extend and a retire.

Note what this also explains: the boundary is the **route**, not the page kind.
On 64-bit, 513–1024 are medium PAGES on the `direct[]` route and they churn
pages too (195); it is crossing `SMALL_SIZE_MAX` that stops it.

### 8.3 It is a deliberate trade, and the defect was that you could not move it

The 512 is not an oversight. Its own doc records why: at 10,000 the small
profile starved — "512 B capacity decaying 168 → 8 blocks and 22,533 of 50,000
churn allocations returning null with 61,440 bytes of the region free". Short
sweeps buy that back and cost page churn. **Neither direction is free, and
which is right depends on the workload** — a bench that keeps one block live
cannot decay, so it sees only the cost.

The real gap is that a firmware had **no way to choose**. `options::set` is a
no-op under `ONE_REGION` (options are compile-time constants there, which is
what lets the option table leave the image), and the doc told firmwares to
"change the default it is built with" when there was no cfg to do it.

**Shipped:** `--cfg ra_generic_collect="64" | "4096" | "65536"`.

### 8.4 The observability §5 asked for

- **`prim::fixed::shape_of(size) -> Shape`** — page bytes, dedicated segments,
  and `direct_route`, all `const`, all derived from the active geometry. That
  is "which page kind, and how many region bytes" as a compile-time answer;
  `region_stats()` could never answer it because it reports over extents.
- **`Shape::direct_route` names the pointer-width boundary**, so the next
  consumer meets it in a `const` assertion instead of on silicon.
- A unit test pins `SMALL_SIZE_MAX == 128 * size_of::<usize>()` and the two
  width-specific values.

Two of the §5 gaps were already reachable and are worth knowing rather than
building: **`alloc::usable_size`** is public, and **`alloc::stats()`** is the
slow-path counter. Both are in `alloc`, not `prim::fixed`, so a seam
re-exporting the fixed-region API in one `pub use` cannot see them — the same
shape as the `PrimError` gap closed in 2.1.0. Re-export them from `alloc` in
the seam and the "nothing exposes a counter" note goes away.

### 8.5 What is NOT established, and whose run finishes it

**How much of the 16 % the page churn accounts for is still open.** Raising the
heartbeat on a host takes the churn to a measured zero, but the timing arm of
that experiment was inadmissible: its control re-run flipped from +8.0 % to
+0.6 %, so the instrument was deciding the answer and the number is withdrawn
rather than quoted. The device, with `CCOUNT` and a quiet core, is the right
box for it — `--cfg ra_generic_collect="65536"` against the same sweep on the
same part is the one-line experiment, and the counter half of it
(`stats().pages_fresh`) is deterministic and needs no quiet box at all.

Nothing here changes the default. A firmware that has measured its own workload
can now move it; one that has not should not.

### 8.6 A lead worth more than the knob: every reclaimed page has `capacity = 1`

Tracing the reclaim itself (a temporary `eprintln` in `collect_inner`'s
`page_all_free` branch, 100,000 pairs per size on x86-64) gives the churn a
face, and one column of it was not expected:

```text
    200 RECLAIM bin=24 block_size=1024 used=0 capacity=1
    200 RECLAIM bin=21 block_size=640  used=0 capacity=1
    199 RECLAIM bin=20 block_size=512  used=0 capacity=1
      1 RECLAIM bin=28 block_size=2048 used=0 capacity=1
      1 RECLAIM bin=25 block_size=1280 used=0 capacity=1
```

Two things. The route boundary is exact — bins 20/21/24 are the `direct[]`
sizes (512, 640, 1024 at `SMALL_SIZE_MAX = 1024` on this host) and churn ~200
times each; bins 25 and 28 are just above it and churn **once**, at the
transition between probe rows. And **`capacity = 1` on every one of them.**

`page_extend` links a 4 KiB *payload* bound per extend, so a 512-byte class
should come back with 8 blocks and a 1,024-byte class with 4. A page holding
ONE block is exhausted by the allocation that follows it, which is the missing
half of why `generic` reads exactly **1.0000 per op** in §8.2 — the fast path
cannot hit a page that has no second block. Whether `reserved` is being
clamped to 1 for these classes, or the extend is not running at all, is not
established here.

**This is the lead to pull next, and it is not the heartbeat.** The knob in
§8.3 changes how often an empty page is reclaimed; this asks why the page was
worth so little in the first place. If a `direct[]`-route page came back with
its full block count, the fast path would hit, `generic` would fall well below
1.0/op, and the sweep would find the page in use rather than empty — the step
would close from the other side, with no knob and no default moved.

Recorded rather than chased because it is a hot-path change (`page_extend` and
`page_fresh`) that needs the full instruction-count battery, not a session's
tail. The reproducer is three lines of `eprintln` in `collect_inner` and the
counter probe in §8.2.

---

## 8.7 The defect, found by pulling §8.6 — the extend bound was sixteen times too small

`page_extend` links a batch of blocks and bounds it by 4 KiB of payload
("one OS page seems to work well"). It computed that bound as

```rust
let span_shift = 4 + (*page).slice_count.trailing_zeros();
let take = ((reserved >> span_shift).max(1)).min(reserved - capacity);
```

The identity behind it is `4096 / bsize == reserved / (slice_count * 16)`, and
**the `16` is `SEGMENT_SLICE_SIZE / 4096`** — so the literal `4` is `log2(16)`
and is correct only for the shipped 64 KiB slice. Under `ra_small_profile` the
slice is 4 KiB, the true factor is 1, and the bound this computed was **256
bytes of payload instead of 4 KiB**.

For a 512-byte class `reserved >> shift` is then `8 >> 4 == 0`, `.max(1)`
rescues it to one, and **every page on the profile firmware actually uses was
extended ONE BLOCK AT A TIME**. That is why §8.6 saw `capacity = 1` on every
reclaimed page, and why `generic` read exactly 1.0000 per op in §8.2: a page
with one block has no second block for the fast path to find, so every single
allocation took the slow path.

**Fixed** by deriving the constant term from the geometry
(`SEGMENT_SLICE_SIZE.trailing_zeros() - 12`), which is 4 at the default slice
and 0 at the small profile. Counted, 100,000 alloc+free pairs, small profile:

| size | `generic`/op before | after | blocks per extend |
|---|---:|---:|---:|
| 512 | 1.0000 | **0.1250** | 8 |
| 513 | 1.0000 | **0.1667** | 6 |
| 1024 | 1.0000 | **0.2500** | 4 |

Page churn falls with it, 195 carve-and-retire cycles per 100,000 becoming 24,
32 and 49 respectively.

**The step inverts.** On the 32-bit host, ABBA-interleaved, reproduced three
times: 512 against 513 goes from **+8.6 % (slower)** to **−36 % (faster)**, and
512 in absolute terms from 390,200 ns to ~200,000 ns for the same 50,000 pairs
— about **1.9× faster**. The `direct[]` route is now the fast route, which is
what its name always claimed.

**The default geometry does not move.** `65536.trailing_zeros() - 12 == 4`, the
old literal, so the constant is identical there; the all-features x86-64
assembly diff against `main` changes exactly one symbol, the debug-record blob,
with no executable function touched.

### Measured on silicon after all (2026-09-10)

The "not measured on silicon" caveat above is withdrawn: it is measured, on our
own XIAO ESP32-S3, `main` against this fix in one session on one board. Same
192 KiB region, same 166 ns/op no-allocator floor, and **identical checksums on
every row**, so the two arms did identical work.

| workload | `main` | with the fix | |
|---|---:|---:|---|
| pingpong, 32 B | 595 | **518** | **13.0 % faster** |
| batch, 64 mixed 8-512 B | 833 | **702** | **15.7 % faster** |
| churn, 64 live 8-512 B | 1,011 | **856** | **15.3 % faster** |
| large, 2,048 B | 1,134 | 1,121 | 1.1 % |

ns per alloc/free pair, net of the floor. **13-16 % on every binned workload**,
which is the same magnitude as the 16 % step this report opened with — and
2,048 B barely moving is the tell that it is the same mechanism, because that
size is on the bin route, which still enters `malloc_generic` on every
operation and is untouched by this fix.

What this does NOT settle is §7's `heap_4` row. That is a different harness
(direct calls, 64 KiB, one live block) and a different question; whether
256-512 stops being the only range `rusty_alloc` loses is still the consumer's
run to make.

### What this does and does not settle for the report

It closes the 512-byte step and it should take a large bite out of §0's 24×
host-versus-device gap, because the device was paying `malloc_generic` on 100 %
of allocations where it should pay it on one in eight. **It is not measured on
silicon** — that rig is Kairos's, and §7's `heap_4` A/B at 256–512 is the row
to re-run. The prediction to falsify: the 256–512 range stops being the only
one `rusty_alloc` loses.

What it does NOT explain is why the bin route enters `malloc_generic` on every
op even now (`generic` still 1.0000/op at 1025 and 2048, unchanged by this
fix). That is a separate thread, and the trace in §8.6 is where to pick it up.
