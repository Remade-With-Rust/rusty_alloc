# Adopting rusty_alloc in a firmware: three sharp edges

**Status: ALL FOUR IMPLEMENTED 2026-09-08** — see §Resolution at the end. Two
of the proposals were changed on implementation, both because the proposed form
would not have worked; that is recorded there rather than quietly fixed.

**Originally: PROPOSED 2026-09-08.** Written from the outside, by the first
consumer to put 2.0.0 into an ESP32 firmware that is not this repo's own
benchmark. Everything below was hit in one afternoon of integration, and none
of it was hit by the harness in `small-metal.md` — because that harness sets
the geometry correctly, runs no interrupts, and sizes its region by hand.

That is the theme. **The allocator is right; the adoption path is where a
consumer falls over,** and all three findings are cases where a wrong
integration is accepted silently and fails somewhere far away from its cause.

Consumer: the Janus family (`rusty_esp_*`), a XIAO ESP32-S3 Sense on the
`esp-hal` bare-metal track, adopting through a one-crate seam
(`rusty_esp_alloc`) as the house standard requires.

---

## 1. A region that cannot yield one segment is accepted

**Severity: this is the one to fix.** It costs a board run and gives no clue.

`init_region` refuses a region below `FIXED_PAGE` and accepts everything
above it:

```rust
let len = region.len();
if len < FIXED_PAGE {
    return Err(FERR);
}
```

But the layers above carve the region into `SEGMENT_SIZE` granules, and
`SEGMENT_SIZE` is **32 MiB** unless `ra_small_profile` is set. So a firmware
that sets `ra_single_threaded` (which the crate demands, loudly, and which
therefore gets set) but *not* `ra_small_profile` (which nothing demands) gets:

| | |
|---|---:|
| region handed over | 225,280 B (220 KiB) |
| `SEGMENT_SIZE`, default geometry | 33,554,432 B |
| segments the region yields | **0** |
| what `init_region` returns | `Ok(())` |
| what the build says | nothing; it compiles clean |
| what happens on the board | every allocation fails |

The failure surfaces as the first `Vec` returning null, an allocation-error
handler, and a panic with a backtrace pointing at whatever happened to
allocate first. Nothing in that chain says "your segment geometry is 512x too
large for the region you gave me".

**Proposed.** `init_region` refuses a region that cannot hold a segment:

```rust
if len < FIXED_PAGE + SEGMENT_SIZE {
    return Err(FERR);   // or a distinct code; see below
}
```

Two notes on shape. First, `PrimError` is a single sentinel, so the caller
cannot tell "too small for a page" from "too small for a segment" from
"already registered" — three very different fixes. If a distinct code is too
much churn, the doc comment should at least name the third condition, because
right now `# Errors` lists two and there would be three.

Second, and more valuable than the check: **the geometry should be visible.**
A `pub const` re-export of `SEGMENT_SIZE` from `prim::fixed`, or a
`region_requirements() -> (min_bytes, granule)`, lets a seam crate assert at
compile time rather than discovering it on silicon. The consumer wants to
write:

```rust
const _: () = assert!(N >= rusty_alloc::prim::fixed::MIN_REGION);
```

and today cannot, because the relationship is spread across two modules and
one `--cfg`.

### 1b. `ra_small_profile` is discoverable only by reading the source

The crate refuses to build `no_std` without `ra_single_threaded`, with an
excellent `compile_error!` that says exactly what to do. `ra_small_profile`
has no such gate, is not mentioned in the README's embedded section, and is
the difference between a working firmware and one where nothing allocates.

The asymmetry is the problem: a consumer meets the first cfg, learns that
this crate tells you what it needs, and reasonably concludes there is nothing
else to set.

**Proposed.** Either gate it the same way (refuse a bare-metal target with the
default geometry unless the consumer has explicitly opted into 32 MiB
segments), or say it in the README beside the 68 KiB figure — that figure is
*of the small profile*, and a reader who copies the number without the cfg
gets neither.

---

## 2. An allocating interrupt hangs, and the cfg's name hides it

`prim::fixed`'s `Guard` is a plain non-reentrant spin lock, correctly
documented as such:

```rust
fn acquire(lock: &'static AtomicBool) -> Self {
    while lock.compare_exchange_weak(false, true, Acquire, Relaxed).is_err() {
        core::hint::spin_loop();
    }
    Self(lock)
}
```

On a single-threaded target, **a contended acquire is not possible from
another thread — there isn't one. It can only be reentrancy.** And on a chip
the only source of reentrancy is an interrupt handler that allocates while
the main context holds the lock.

The consequence is a **hard hang inside the ISR**: the main context is
preempted and can never release, so the spin never ends. On an ESP32 that
becomes a watchdog reset with a backtrace pointing into `spin_loop`, or no
reset at all if the ISR outranks the watchdog. Either way the cause is
invisible.

This matters more than it looks because of what the opt-in is called.
`ra_single_threaded` reads as "I have one core and no RTOS" — which is true
of every esp-hal firmware, so everyone will set it. The actual requirement is
**single *context***, and interrupt handlers are a second context on one core.
Timer, GPIO and DMA-completion handlers are ordinary in this ecosystem, and an
embassy executor makes allocating from one entirely plausible.

**Proposed.** Turn the hang into a diagnosis. Under `ra_single_threaded` the
first failed CAS is proof of reentrancy, so:

```rust
#[cfg(ra_single_threaded)]
{
    // Nothing else can hold this: there is no second thread. A contended
    // acquire means this call re-entered from an interrupt handler that
    // allocated while the main context was inside the allocator.
    panic!("rusty_alloc: allocator re-entered, almost certainly from an \
            interrupt handler that allocates. The fixed backend's lock is \
            not reentrant; do not allocate in an ISR.");
}
```

Zero cost on the happy path — the CAS already happens, only the failure arm
changes — and it converts an undiagnosable wedge into a one-line answer. This
is the same principle as `tests/corruption.rs`: a failure mode nobody has
watched fire is a claim, not a defence.

The docs should also say "single context, interrupts included" wherever the
cfg is described. A rename to `ra_single_context` would be clearer still, but
that is a breaking change to a published gate and probably not worth it alone.

---

## 3. The stranded tail is in a plan, not in the API

`small-metal.md` §3 of the stress section records the rule:

> Size an embedded region as `k * 64 KiB + 4 KiB`, or the tail is dead to
> large allocations.

Correct, and nothing surfaces it. Under the small profile a 220 KiB region
yields `floor((225,280 - 4,096) / 65,536) = 3` segments = 196,608 B, and
**24,576 bytes are dead** — 11% of the budget, silently. A firmware author
picking a round number like 220 KiB has no way to learn this except by
reading a design document.

**Proposed.** Return it, or expose it:

```rust
pub const fn usable_bytes(region_len: usize) -> usize;
```

so a firmware can log `budget=225280 usable=196608` at startup and see the
gap. `region_stats()` already exists precisely on the argument that "a
fixed-region allocator that cannot report how much of its region is out is
unmeasurable on exactly the deployment it exists for" — this is the same
argument applied one level up, to the region it was given rather than the
region it is using.

---

## What is NOT proposed here

- **Nothing about the 68 KiB floor.** It is structural, it is documented
  honestly, and the README's advice — use `esp-alloc` when the budget is
  tight — is correct and was followed. Our own firmwares mostly do not churn
  (one allocates five buffers at startup and never allocates again; its heap
  figure does not move across eight kernels), so we expect no speed win and
  the whole footprint cost, and we are adopting for the double-free abort
  rather than for throughput. That is the README working as intended.
- **Nothing about bin coarsening.** `small-metal.md` weighs it, records the
  ABI argument and the unbounded-fragmentation argument, and defers it with
  its number. Agreed, and not our call.
- **Nothing about the automatic `collect`.** Already the top item in that
  plan's §6, already understood, and it needs the callgrind harness rather
  than a board.
- **No speed or footprint claim from us.** We have built both arms of one
  firmware from one source and measured only ELF bytes (243,300 -> 265,036,
  +8.9%, close to the +9.5% app image this repo reports). Nothing has run on
  silicon on our side yet. When it does, the numbers will come with their
  method line and we will send them whether or not they flatter the swap.

---

## Suggested order

1. **§1** — the segment-size refusal, plus a `MIN_REGION` a consumer can
   `const _: () = assert!` against. Smallest change, largest saving: it turns
   a wasted board run into a build error or an `Err`.
2. **§1b** — say `ra_small_profile` where the 68 KiB figure is quoted. A
   documentation change that prevents the same wasted run.
3. **§2** — the reentrancy panic. Small, zero-cost, and the difference
   between a hang and an answer.
4. **§3** — `usable_bytes`. Nice to have; the rule is at least written down
   today, which the other two are not.

Items 1 and 2 both convert a silent wrong-integration into a loud one, which
is the same thing this crate already does well with `ra_single_threaded` and
with the double-free abort. They are that habit applied two steps further out.

---

## Resolution (2026-09-08, this repo)

All four landed. Every claim above was verified against the code first; all
three were accurate, and **§1b was worse than reported** — `ra_small_profile`
appeared **zero times** in either README, while the `68 KiB` figure appeared
five times as *the* embedded floor. That figure is only true under the cfg, so
the README did not merely omit the flag, it handed a reader a number that is
wrong without it.

### §1b — done first, not third

Promoted ahead of the code changes because it is minutes of work and prevents
the same wasted board run. Both READMEs now carry a **"What a firmware has to
set"** section: the dependency line, both cfgs, the `init_region` call, and a
table whose middle row is the trap — *builds and links clean, then nothing
allocates*. The `68 KiB` row in the footprint table now carries
`(needs --cfg ra_small_profile)` inline, because that is the number people copy.

### §1 — done, with an exact check rather than the proposed one

`init_region` now refuses. **The proposed condition was not sufficient:**

```rust
if len < FIXED_PAGE + SEGMENT_SIZE { ... }   // proposed
```

`init_region` receives the *base address*, and the first segment can only start
on a `SEGMENT_SIZE` boundary. A region satisfying that length test whose base
sits one byte past a boundary still yields nothing — the same silent failure,
one size smaller. The shipped check asks the real question,
`usable_bytes(base, len) == 0`, which is exact where a length test is
optimistic. `usable_bytes_answers_the_question_a_firmware_asks` pins that case
specifically.

`PrimError` turned out to be a plain `u32`, so the distinct codes cost nothing:
**`FERR_TOO_SMALL`, `FERR_GEOMETRY`, `FERR_REGISTERED`** are public, and the
`# Errors` doc now lists three conditions instead of two.

**`MIN_REGION` is public**, so the `const _: () = assert!(...)` the report asked
for now compiles. It documents that it assumes an aligned base and that
`init_region` is the exact one — the constant is for compile-time sizing, the
function for the truth.

One consequence worth knowing: this backend's own unit tests exercise it as a
plain extent allocator on a 512 KiB region, which cannot hold a 32 MiB segment.
Rather than weaken the check or lose that coverage, the install half was split
into a private `install_region`, and the test branches on the active geometry —
asserting the refusal where a segment does not fit and using the public entry
point where it does. The refusal has no public bypass.

### §2 — done, but the proposed trigger would have misfired

The reasoning is right and the fix is nearly free. **The proposed trigger is
not safe as written:**

```rust
while lock.compare_exchange_weak(...).is_err() {
    #[cfg(ra_single_threaded)] { panic!(...) }    // proposed
```

`compare_exchange_weak` is permitted to fail **spuriously**. A failed CAS is
therefore not proof of anything, and this would panic firmwares at random —
worse than the hang it replaces. The shipped version confirms with a load
before it accuses:

```rust
#[cfg(ra_single_threaded)]
if lock.load(Ordering::Relaxed) { reentered(); }
```

A spurious failure leaves the lock `false` and retries; a lock actually observed
held, on a target that promised one context, can only be reentrancy. The message
says so, names the ISR, and spells out that `ra_single_threaded` means single
*context* — the report's point about the name, applied where someone hits it.

Watched firing: `a_reentrant_acquire_is_diagnosed_not_hung` is `#[should_panic]`
and CI runs `prim::fixed`'s tests under `--cfg ra_single_threaded`. It is
deliberately **not** in `tools/gate-selftest.sh`: poisoning this gate removes the
detector, and the test then hangs rather than fails, which is the defect itself
and useless in CI.

### §3 — done, and one part of the review of it was wrong

`usable_bytes(base, len)` is public and `const`, so a region can be sized at
compile time or logged at startup.

The review of this report initially agreed that `region_stats()` should stop
counting the stranded tail as `free`. **That was wrong and is not implemented.**
The tail is genuinely available to page-sized allocations — two-ended placement
serves those from the top — so `free` is honest. What was missing is a
*different* number: how much can still become a segment. Changing `free` would
have replaced one incomplete answer with another. The two are now separate, and
`usable_bytes`' doc says why.

### What this did not change

The 68 KiB floor, bin coarsening and the automatic `collect` are untouched, as
the report proposed. Their ELF measurement (243,300 -> 265,036, **+8.9%**)
independently corroborates this repo's **+9.5%** app-image figure from a
different firmware — the first outside check on that number.

### Verification

110 tests / 33 suites default; 91 small profile; `prim::fixed` under
`--cfg ra_single_threaded`; clippy clean on all three configs; both RISC-V
targets at both geometries; wasm32; unsafe census; gate selftest 5/5; wasm size
ratchet. Board re-flashed: 68 KiB region still accepted, `used 69632 free 0`,
kill test green.
