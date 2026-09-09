# Firmware code size: what the swap costs, and where the levers are

**Status: EXECUTED 2026-09-09 -- see section 7.** Levers 1, 2 and 3 taken,
each measured on the linked ELF as a separate brick: the allocator's flash cost
fell from **+16,584 B to +7,860 B** (-52.6 %) and its attributable code from
16,256 B to 8,262 B, with no change to any hosted build. The same predicate
took **10.7 %** off the gzipped wasm bundle. The stack identity and the
crossover are in the README. Two residues the plan could not name are now
named (section 7.5).

Originally: a byte-exact decomposition of what `rusty_alloc 2.0.1` adds to an
`esp-hal` firmware compared with `esp-alloc` 0.11, and a ranked lever list.
Measured, not projected, on the linked artifacts.

This is the sibling of `small-metal.md`, which decomposed the *heap region*.
Nothing here is about RAM the allocator serves from; it is about the flash and
the static RAM the allocator itself occupies.

Consumer and method: the Janus family's `xiao-s3-probe` firmware, XIAO
ESP32-S3 Sense, `--cfg ra_single_threaded --cfg ra_small_profile`,
`opt-level=3 lto=fat codegen-units=1 panic=abort`. **One binary source, two
arms**, selected by a cargo feature, so the dependency graph and every other
line are identical. Sizes from `xtensa-esp32s3-elf-size -A` and
`nm -S --size-sort` on the actually-linked ELF, per §9 of the
footprint-decomposition discipline — not from the source, and not from
`cargo bloat`.

**Work parity.** Both arms allocate the same five buffers at startup:
`19,200 x 2`, `38,400 x 2`, `57,600` = **172,800 bytes live**, and neither
frees before the end. Identical in both arms and unchanged across the runs
below. That is the null arm: the workload never moved, so the allocator is
the only variable.

---

## 1. The decomposition

| section | `esp-alloc` | `rusty_alloc` | delta |
|---|---:|---:|---:|
| `.text` | 50,337 | 62,417 | **+12,080** |
| `.rodata` | 7,724 | 10,124 | **+2,400** |
| `.data` | 2,300 | 4,404 | **+2,104** |
| `.bss` | 225,372 | 226,360 | **+988** |
| `.stack` | 107,696 | 104,604 | **-3,092** |
| `.rwtext`, `.rwdata_dummy`, `.vectors`, `.rotext_dummy`, `.flash.appdesc` | — | — | 0 |

Read as two budgets:

| budget | bytes | made of |
|---|---:|---|
| **flash** | **+16,584** | `.text` + `.rodata` + `.data` |
| **static RAM** | **+3,092** | `.bss` + `.data`'s RAM copy |

### The identity worth having

`Δ.bss + Δ.data = 988 + 2,104 = 3,092`, and `Δ.stack = -3,092`. **Exact, to the
byte.** The linker script hands the stack whatever RAM is left, so every byte
of static growth comes straight out of stack headroom.

That is not a size number, it is a **hazard**: a firmware sitting near its
stack limit does not get a bigger binary when it adopts rusty_alloc, it gets a
stack overflow, and nothing in the build says so. Worth a line in the README
beside the 68 KiB figure.

### Attribution

| | bytes | symbols |
|---|---:|---:|
| `rusty_alloc::*` in the rusty arm | 16,256 | 57 |
| `esp_alloc::*` in the esp arm | 1,043 | 6 |
| **net, attributable** | **+15,213** | |
| measured flash delta | +16,584 | |
| **unattributed** | **1,371** | |

The residue is ~8% and is not chased here: it is new panic-message formatting,
the seam's own code, and generic machinery pulled in behind the allocator. It
is named rather than hidden, per §2 — the lines do not perfectly reconcile and
saying so is the point.

---

## 2. Levers, ranked, each classified

### Lever 1 — `ra_single_threaded` prunes nothing

**Class: defect. Arithmetic, not projection: 2,149 bytes.**

The cfg exists, the consumer sets it, and it currently changes only
*correctness assumptions* — the cell's `Sync`, the spin lock, the split
atomics. It does not remove code that **cannot execute** under it:

| symbol | bytes | why it cannot run on one context |
|---|---:|---|
| `Heap::adopt_segment` | 1,154 | adopting a segment abandoned by another thread |
| `Heap::drain_delayed` | 995 | the cross-thread delayed-free list |
| | **2,149** | |

Neither carries a `cfg` today; both are linked unconditionally. On a target
that has asserted there is exactly one context, a segment can never be
abandoned by anyone and the delayed list can never receive a cross-thread
free.

This is the highest-confidence item on the list because the number is a
subtraction, not a model. It is also the one with a correctness argument
attached: if `adopt_segment` *can* be reached on a single-context target, then
the `ra_single_threaded` assumption is already unsound and that is worth
knowing for its own sake.

### Lever 2 — thread-heap initialisation on a one-heap target

**Class: granularity. Direction certain; magnitude unmeasured.**

`init::init_thread_heap` is **2,740 bytes**, the largest single symbol in the
arm. On a single-context target there is one heap for the life of the program.
How much of that 2,740 is per-thread generality versus the one-time setup any
heap needs is not established here, and no number is offered — a plausible
projection in a ranked list gets quoted later as a measurement (§7).

What would settle it: build the arm with the function's per-thread paths
`cfg`-ed out and diff `.text`. That is a rusty_alloc-side experiment, not a
consumer-side one.

### Lever 3 — guarded sampling and its generator, with `secure` off

**Class: possible reachability defect. 2,366 bytes if unreachable.**

| symbol | bytes |
|---|---:|
| `Heap::try_guarded` | 1,641 |
| `Random::refill` | 725 |
| | **2,366** |

Both are present with `secure` **not** enabled. If guarded-object sampling is
off in this build, this is 2,366 bytes of unreachable code that LTO did not
drop — which is the reachability question this repo already takes seriously
elsewhere.

**Explicitly not a claim yet.** `try_guarded` may be runtime-gated on an
option rather than a feature, in which case LTO cannot prove it dead and the
fix is a `cfg`, not a bug. Settle it with a byte census — does any allocation
in this workload enter `try_guarded`? — rather than by reading the call graph.

### Lever 4 — the free paths a never-freeing firmware cannot reach

**Class: structure. Not a lever.**

`segment::huge_free` (420), `segment_free` (381), `span_free` (634) and the
rest of the teardown machinery are ~1.4 KB that this particular workload never
executes. It is listed to be dismissed: a library cannot know the consumer
never frees, `panic = "abort"` does not remove `Drop` paths reachable in
principle, and gating this on anything would be a footgun. Recorded so nobody
re-derives it.

---

## 3. What is structurally unreachable

The two floors are different functions, not one tuned differently:

```
esp-alloc:    1,043 bytes  = a free-list walk, a header, a coalesce
rusty_alloc: 16,256 bytes  = segments + arenas + bins + a segment map
                             + an RNG + delayed frees + adopt/abandon
```

**Parity is not the goal and should not be offered.** Even taking every lever
above — 2,149 certain, plus whatever levers 2 and 3 yield — lands somewhere
near 10-12 KB, still an order of magnitude above a boundary-tag allocator.
That is the price of a size-class page allocator's structure, and it is the
same sentence `small-metal.md` reached about the heap region from the other
direction.

**The crossover, which is the useful output.** Flash cost is roughly *fixed*:
+16.6 KB whether the firmware is 240 KB or 900 KB. So it is 6.9% of this
firmware and under 2% of a mesh node with a TLS stack. The README's advice
already says use `esp-alloc` when the budget is tight; the honest addition is
that **the flash cost stops mattering as the firmware grows, while the 68 KiB
heap floor does not** — that one scales with size classes touched, not with
the program. Two different curves, and a reader choosing an allocator wants
both.

---

## 4. Dead hypotheses

- **"It will be the RNG and the segment map tables."** `options::VALUES` is
  304 bytes and `segment_map::range_table::BASE` is 256. Together 560 bytes,
  3% of the delta. Killed by `nm --size-sort` in one command; the cost is in
  code, not tables.
- **"`.rotext_dummy` and `.rwtext` will grow."** Both are byte-identical
  across arms. The allocator adds nothing to IRAM.
- **"The stack shrank, so something got smaller."** No — the stack is a filler
  region and its shrinkage is exactly the static growth (§1). Reading it as a
  saving would have been the wrong sign entirely.

---

## 5. Suggested order

1. **Lever 1.** A subtraction with a correctness argument attached, and the
   only item whose number is arithmetic. Two `cfg`s.
2. **The stack identity in the README.** Free, and it is the finding most
   likely to bite an adopter without warning.
3. **Lever 3's census.** Cheap to settle and either 2,366 bytes or a note that
   it is runtime-gated by design.
4. **Lever 2.** Largest single symbol, but needs its own experiment before
   anyone should quote a figure for it.

---

## 6. Runtime, measured after this plan was written

Both arms have now run on the board, and two things are worth adding.

**The geometry behaves exactly as predicted.** A 220 KiB region reports
`used=200704 free=24576` and never moves: three 64 KiB segments plus the
4 KiB page, with 24,576 bytes stranded. Predicted from the granule before the
run and confirmed to the byte, which is the check `usable_bytes` now lets any
consumer make at compile time.

**The swap is invisible to compute, and it moves buffer placement by up to
8 %.** Four of eight kernels reproduced across the two arms to within **six
parts per million** — a free null arm, and this instrument's floor. The other
four moved 3.3 %, 4.8 %, 3.6 % and **-7.7 %**. Those four are exactly the ones
touching the 57,600- and 38,400-byte buffers; the four that did not move touch
only the two 19,200-byte ones. The heap does not move once the buffers exist,
so no allocator call is inside any of these numbers, and the **mixed sign**
rules out overhead: one kernel got faster.

So the effect is alignment and bank placement, not allocator speed. Worth
knowing for anyone benchmarking a kernel across an allocator change — the
allocator can shift a compute number by 8 % without executing a single
instruction inside the measured region.

No allocation-throughput number is offered from this firmware, because it
allocates five buffers once and never frees. It is the wrong shape to price an
allocator's hot path, which is what this repo's own churn harness is for.

Nor is any of this a complaint. The swap was adopted for the double-free abort
rather than for speed, on a workload that allocates five buffers once and
never frees — the shape the README correctly says gains nothing from a page
allocator. The cost is being decomposed because it is being paid knowingly.

---

## 7. Resolution (2026-09-09)

Everything below is from the same rig as §1: the `xiao-s3-probe` firmware,
one source, two arms, `opt-level=3 lto=fat codegen-units=1 panic=abort`,
`--cfg ra_single_threaded --cfg ra_small_profile`, sizes from
`xtensa-esp32s3-elf-size -A` and `nm -S --size-sort` on the linked ELF. One
difference: the firmware was copied to a scratch directory with a
`[patch.crates-io]` pointing both `rusty_alloc` crates at the working tree, so
the two arms of every A/B differ only in this branch's edits and not in
crates.io-versus-path. The baseline rebuilt from that rig reproduced §1 to the
byte on `.text`, `.data`, `.bss`, `.stack` and the 16,256/57 attribution;
`.rodata` came out **312 bytes lower**, and §7.5 says why.

### 7.1 The predicate, not the flag

The three levers share one root: code that cannot run on a single-context
target was linked because nothing folded the branch that reached it. The fix
is one `const`, not three `cfg`s:

```rust
pub(crate) const ONE_THREAD: bool = cfg!(any(
    all(ra_single_threaded, not(miri), not(windows), not(unix), not(target_arch = "wasm32")),
    all(target_arch = "wasm32", target_os = "unknown", not(target_feature = "atomics")),
));
```

`true` exactly where `prim::thread_id()` is a compile-time constant. It is
deliberately **not** `ra_single_threaded` alone: on a hosted target that cfg
only unlocks the fixed backend's unit tests, the OS prim still hands out real
thread ids, and the suite spawns threads. A `const` rather than a `cfg` so
every site reads `if ONE_THREAD`, a host build compiles both arms, and the
pruned code stays type-checked and unit-tested everywhere while being linked
only where it can run. Four consumers: `abandoned_pop` returns null,
`process_delayed` returns before the peek, `free`'s owner test becomes
`ONE_THREAD || owner_tid == thread_id()`, and `remote_free` and
`abandoned_push` say `unreachable!` if they are ever reached — which is the
correctness argument §2 asked for, made executable.

The same predicate holds for `wasm32-unknown-unknown` without atomics, which
is why the wasm ratchet moved (§7.4). That is the gate-routing the campaign
has been watching for: one condition, a win on every single-threaded target,
byte-identical everywhere else.

Lever 3 got its own constant, `GUARD_PAGES = cfg!(any(unix, windows, miri))`:
true exactly where `prim::protect` can protect. `prim::fixed` and `prim::wasm`
return `Err`, and the sampler used to run anyway and hand out a dedicated
segment with an **unprotected** trailing page — the whole cost of a guarded
object and none of the protection. `RNG_USED = GUARD_PAGES || secure` then
says whether anything draws from the heap's CSPRNG at all.

### 7.2 The bricks, each measured on the linked ELF

| brick | `.text` | `.rodata` | `.data` | `.bss` | flash | attributed |
|---|---:|---:|---:|---:|---:|---:|
| baseline (2.0.1 from the working tree) | 62,417 | 9,812 | 4,404 | 226,360 | 76,633 | 16,256 |
| 1 — `ONE_THREAD` (lever 1) | −3,208 | −120 | 0 | −12 | −3,328 | −3,152 |
| 2 — `GUARD_PAGES` + `RNG_USED` (lever 3) | −4,828 | −88 | −8 | −8 | −4,924 | −4,683 |
| 3 — no exit hook on one context (lever 2) | −160 | 0 | 0 | −12 | −160 | −159 |
| **final** | **54,221** | **9,604** | **4,396** | **226,328** | **68,221** | **8,262** |
| **change** | **−8,196** | **−208** | **−8** | **−32** | **−8,412** | **−7,994** |

The stack identity held on every brick: `.stack` grew by exactly what
`.bss + .data` shrank (+12, +16, +12).

**Lever 1, predicted 2,149, measured as predicted and then some.**
`adopt_segment` (1,154) and `drain_delayed` (995) are gone from the image —
the subtraction was right. `malloc_generic_once` also lost 325 bytes (its
adopt-until-satisfied loop), and `span_free`, `huge_free`, `segment_free` and
`prim::free` disappeared *as symbols* while `collect_inner` grew by 1,193. That
is inlining, not removal: with `adopt_segment` gone each of them had one caller
left. Net of the inlining, 649 more bytes; **lever 4 is still linked, as §2
said it must be.**

**Lever 3, predicted 2,366 if unreachable, measured exactly that and then
1,621 more.** `try_guarded` (1,641) and `Random::refill` (725) left as the
census predicted. What the plan did not predict: `init_thread_heap` fell from
2,740 to 1,119 in the same brick. The seeding — splitmix64 over a 64-bit
accumulator, which on a 32-bit Xtensa is a run of `__muldi3`, plus the ChaCha
key schedule — was 1,621 bytes of the largest symbol in the arm, and it seeded
a generator nothing on this target ever read. `reserve_default_arena_on_miss`
and `chunk_alloc_n_inner` also folded into `malloc_generic_once` (−1,417 as
symbols, +735 growth): `guarded_alloc`'s huge-segment path was their second
caller.

**Lever 2, the one the plan refused to put a number on, now has one.** The
2,740 bytes of `init_thread_heap` were:

| part | bytes | share |
|---|---:|---:|
| seeding a CSPRNG with no consumer (moved by lever 3) | 1,621 | 59 % |
| the thread-exit hook: a TLS slot, its CAS, the spin (brick 3) | 153 | 6 % |
| creating a heap: one page from the region, the `HeapBox` write, the registry | 966 | 35 % |

So the plan's suspicion — "per-thread generality versus the one-time setup any
heap needs" — was the wrong split. Per-thread generality was 153 bytes. The
bulk was an RNG. The remaining 966 is what any heap costs to create and is not
a lever; the exit hook is skipped under `ONE_THREAD` because `prim::fixed`'s
TLS never runs a destructor anyway (`tls_new` discards it), so the slot, its
compare-and-swap and its table entry were paid once for nothing.

### 7.3 What it costs now, arm to arm

| section | `esp-alloc` | `rusty_alloc` | delta | was (§1) |
|---|---:|---:|---:|---:|
| `.text` | 50,337 | 54,221 | **+3,884** | +12,080 |
| `.rodata` | 7,724 | 9,604 | **+1,880** | +2,400 |
| `.data` | 2,300 | 4,396 | **+2,096** | +2,104 |
| `.bss` | 225,372 | 226,328 | **+956** | +988 |
| `.stack` | 107,696 | 104,644 | **−3,052** | −3,092 |

| budget | now | was | |
|---|---:|---:|---|
| **flash** | **+7,860** | +16,584 | −52.6 % |
| **static RAM** | **+3,052** | +3,092 | comes out of `.stack`, still to the byte |

Add ~312 to the flash figure for a build from the crates.io registry rather
than a path checkout (§7.5); the honest round number is **about +8 KB**.

Two things to read carefully in the `.text` row. First, the arm-to-arm delta
of 3,884 is smaller than the allocator's 8,262 bytes of attributed code
because the `esp-alloc` arm carries roughly 2.4 KB of `core::fmt::Debug`
machinery that exists only to print `esp_alloc::HEAP.stats()` — the rusty arm
prints a tuple. Both numbers are true; they answer different questions
("what does the swap cost this firmware" versus "how big is the allocator").
Second, `esp_alloc::*` itself is 1,043 bytes, and the floor argument of §3
stands: 8,262 against 1,043 is the price of segments, bins and a segment map,
and no lever on this list changes that sentence.

### 7.4 The gate-routing win: wasm

`tools/wasm-size.sh`, the ratchet the wasm campaign left behind:

| | gzipped | raw |
|---|---:|---:|
| before | 22,574 | — |
| after brick 1 | 21,528 | 65,137 |
| after brick 2 | 20,269 | 61,788 |
| after brick 3 | **20,169** | 61,588 |

**−2,405 bytes gzipped, −10.7 %**, from a change written for an ESP32. The
baseline is banked in the same commit, as the script asks.

### 7.5 Two residues, now named

**The `.rodata` gap between this rig and §1 is source paths.** Seven
`rusty_alloc` files (`arena.rs`, `heap.rs`, `lib.rs`, `os.rs`, `prim/fixed.rs`,
`random.rs`, `segment.rs`) each contribute a panic-location path string. From
a path checkout those are ~52 bytes each; from
`~/.cargo/registry/src/index.crates.io-…/rusty_alloc-2.0.1/…` they are ~97,
and 7 × 45 ≈ 312. It is small here and it is not ours alone — the same ELF
carries nineteen ~100-byte `esp-hal`/`esp32s3`/`xtensa-lx-rt` paths — but a
firmware that wants them gone gets all of them with one line in its
`.cargo/config.toml`: `-C remap-path-prefix=<registry>=`. Recorded so the next
size comparison across a registry/path boundary is not read as a regression.

**The plan's "unattributed 1,371 bytes" was mostly one symbol the filter could
not see.** `__ra_empty_heap_box` — the `EMPTY_HEAP_BOX` sentinel, a whole
`HeapBox` whose `pages` table carries every bin's block size and whose `direct`
table points at `EMPTY_PAGE` so the fast path can read through it before any
heap exists — is **1,752 bytes of `.data`**: paid once in flash for the image
and again in RAM for the copy the loader makes. Its name has no `rusty_alloc`
in it, so the §1 substring census attributed it to nobody. The rest of the
`.data` delta is `options::VALUES` (304) and `EMPTY_PAGE` (56), and the `.bss`
delta is the segment map's two 256-byte range tables, the two 128-byte extent
arrays of `prim::fixed`, and `ARENAS` (128). `.rodata`'s +1,880 has no symbol
names to give, but its parts are known: the `EMPTY_QUEUES` blob every heap
copies (~900), the 409-byte reentrancy message and 65-byte abort message, and
the ~370 bytes of paths above.

The sentinel is the largest single item left and it is **not taken here**: it
exists so `malloc`'s fast path has no "is there a heap yet" branch, and
trading 1.75 KB of flash and RAM for one predictable branch per allocation is
a hot-path decision that wants the board's churn numbers beside it, not a size
table alone. A single-context target could instead create its heap eagerly at
`init_region` and let the sentinel go — that is the shape of the next lever,
and it is recorded here rather than done.

### 7.6 Gates

- `cargo fmt --check`; clippy `-D warnings` on the host (default and small
  profile, all targets) and on `riscv32imac-unknown-none-elf` `no_std` at both
  geometries; the full host suite on the default, `secure` and small-profile
  arms; `rusty_alloc-api` `no_std`; the wasm ratchet.
- `tools/gate-selftest.sh` gained a sixth mutation: flip `ONE_THREAD` to
  `true` on the host and require the subproc test in `heaps.rs` — which exits
  a thread holding a live block — to go red. It does, through the
  `unreachable!` in `abandoned_push`, which is the executable form of §2's
  "if `adopt_segment` can be reached on a single-context target, the
  assumption is unsound".
- The ESP32-S3 numbers are local-only evidence, as the embedded CI job's
  comment says: no stock runner carries the Xtensa toolchain. The exact rig is
  in the ledger.
