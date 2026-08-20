# rusty_alloc threat model

**Scope**: the published crates `rusty_alloc` (core) and `rusty_alloc-api`
(safe surface), as consumed via `#[global_allocator]`, the `mi_*` C ABI, or
`LD_PRELOAD` interposition.
**Written**: 2026-08-19 · **Revisit**: after any change to the cross-thread
protocol, the TLS design, the `secure` feature, or a dependency — and at
least every 12 months (next: 2027-08-19 or earlier per the audit cadence in
`docs/plans/use-protection-please.md`).

## Assets

1. **Heap metadata integrity** — segment headers, page slots, free lists,
   the cross-thread `xthread_free` words. Corrupting these converts a caller
   bug into arbitrary memory corruption in the host process.
2. **Host-process memory safety** — the allocator must never hand the same
   block to two owners, resurrect freed memory, or write outside its own
   structures, whatever the caller does short of UB.
3. **Availability** — the allocator aborts deliberately on detected
   corruption (double free); it must not be *forceable* into an abort or hang
   by inputs a correct program can produce (e.g. OOM must return null, not
   abort — tracked as R-003).
4. **Confidentiality of recycled memory** — freed blocks must not leak
   another allocation's contents through the zero-guarantee paths
   (`calloc`/`zalloc`'s `free_is_zero` machinery).

## Trust boundaries

- **Caller → allocator**: every argument is untrusted. Sizes and alignments
  are validated (overflow-checked multiplication, power-of-two checks, huge
  sizes routed to dedicated segments). **Pointers are trusted structurally**:
  `free(p)` masks `p` to a segment and reads metadata from it — see T1.
- **Thread → thread**: cross-thread frees ride a lock-free four-state
  protocol (loom-modeled before implementation); thread teardown abandons
  segments for adoption. The ownership identity is the thread-pointer
  register.
- **Process → OS**: the prim layer (mmap/VirtualAlloc, TLS destructors,
  decommit). The OS is trusted.
- **Supply chain**: two runtime dependencies (`libc`, bindings-only
  `windows-sys`); policy enforced by `deny.toml` (no C build scripts, no
  unknown sources).

## Adversaries

- **A1 — a buggy or hostile caller** in the same process: forged, interior,
  double-freed, or foreign (other-allocator) pointers; extreme sizes;
  allocation storms; thread create/destroy churn.
- **A2 — an exploit author** using the allocator's structures as primitives
  after gaining a separate write primitive in the host: free-list poisoning,
  metadata grooming, cross-thread races.
- **A3 — a supply-chain attacker**: dependency substitution, CI compromise,
  release tampering.

## STRIDE pass

| Threat | Instance here | Mitigation / status |
|---|---|---|
| **S**poofing | A recycled thread id impersonating a dead owner | Dying threads abandon segments (id → 0) before the TCB can be recycled; TLS storage is the thread's own block (`.tdata` image), so a recycled TCB cannot inherit a live heap pointer |
| **T**ampering | T1: `free()` on a forged/foreign pointer — `page_of` trusts `slice_offset` read from the pointer's own 32 MiB window (upstream mimalloc parity) | Partial: `secure` feature encrypts free-list links (corruption detected on decode) and adds guard pages; double frees abort on both local and remote paths; full pointer validation (segment-map membership on the free path) is a residual risk — R-001, and the reason `secure` exists for hostile-input services |
| | T2: free-list poisoning via a caller write primitive — the classic heap-exploitation move, where an overwritten link makes the next allocation return an attacker-chosen address and every write through it an arbitrary write | `secure`: per-page keyed link encoding, plus a decode check requiring the link to be block-aligned AND inside the SAME segment as the block carrying it (`page::link_is_plausible`), aborting silently rather than following it. The segment bound is what constrains a DELIBERATE attacker — alignment alone filters accidents, since every address worth steering an allocator at is already pointer-aligned. Both arms are adversarially tested (`tests/corruption.rs` poisons a real free list and asserts SIGABRT, not SIGSEGV) and fuzzed (`fuzz_targets/corruption.rs`). **The default build performs neither the encoding nor the check** — upstream parity, and the reason enabling `secure` is a security decision rather than a performance one |
| **R**epudiation | (not applicable — no authentication or audit domain of its own) | N/A |
| **I**nformation disclosure | Recycled blocks leaking prior contents through the zero guarantee | `free_is_zero` is conservatively cleared on purge/reuse; the zalloc invariant is G1-gated on 1M-op traces; the Darwin decommit contract violation that could have stale-exposed pages was found and fixed in 0.4.0 |
| **D**enial of service | Forced abort or hang from reachable inputs | Double-free abort is a deliberate policy (corruption beats availability). Known gap: heap-creation OOM aborts instead of null-returning (`init.rs:499`, R-003). The init CAS winner aborts rather than hangs on TLS-slot failure (audited 2026-08-06) |
| **E**levation of privilege | The allocator runs at the host's privilege; the escalation risk is memory corruption → see Tampering | Loom-modeled cross-thread protocol; Miri whole-target in CI; 640-thread churn probe; differential oracle |

## What the `secure` feature adds (opt-in; a flat ~15 instructions per allocation)

**Always on with the feature:** per-page encrypted free-list links, and a
same-segment + alignment bound on every decoded link.

**Available but INERT until asked for:** guarded objects and their guard pages.
The `guarded_max` option defaults to 0, so `init::create_heap` skips both
`guarded_set_*` calls and the heap keeps `guarded_rate: 0`. **Enabling
`secure` alone does not give you guard pages** — set the `guarded_max` option
above 0, or call `mi_heap_guarded_set_sample_rate`. Anyone hardening a
hostile-input service on the strength of "secure = guard pages" should read
that sentence twice; the previous wording here implied otherwise.

**Cost, measured 2026-08-20** (callgrind Ir, same binary in both arms, only the
preloaded `.so` differing) — this supersedes an earlier "4–7%" estimate that
was roughly 3–4× too pessimistic:

- **A flat ~15 instructions per allocation**, not a percentage. It lands as
  8–25% per-op purely because the base varies (batch_lifo 60.17 → 75.14 is
  25%; big 171.00 → 185.00 is 8%). `usable` (30.00) and `huge` (666.00) are
  **+0.00** — neither walks a free list, which is what confirms the cost is
  link traffic and nothing else. `realloc` is +46 ≈ 3×15, matching its three
  free-list operations.
- **+0.6% to +1.8% whole-program** on lua / perl / sqlite.

## What `blockmap` adds (opt-in, and expensive: ~+58 instructions per allocation)

A per-page liveness bit for every block, checked on allocation and cleared on
free. **It is the only thing in the crate that answers R-005**, and it works by
refusing to play the game the encoding loses: rather than trying to stop a link
being FORGED, it detects what a forgery is FOR — the allocator handing out a
block that is already live. It therefore does not care how the link was
produced, and it carries no key for a read primitive to recover.

Verified to stop both corruption scenarios **unaided** — no `secure`, no
`linkcheck` — with SIGABRT in debug and release, across 637,602 fuzz executions
with no false positives.

It is OFF by default because of the cost: **+58 Ir/op** on the small and batch
ops (batch_lifo 60.17 → 118.33, very nearly double), +171 on realloc, and
**+2.2% to +5.0% whole-program** on lua/perl/sqlite. That is roughly three
times `secure`, which was itself declined at 1.7%. Memory is ~0.8%, taken as
fewer blocks per page rather than extra allocation.

Two limits worth stating plainly. The map lives at the END of the page payload
rather than the front, so a forward overflow out of the last block can reach
it; the front is the better place on security grounds but the payload start is
not ours to move (`page_area` has eight consumers). And remote frees clear
their bit at the next collect rather than immediately, so a recently
remote-freed block reads as live for a window — the conservative direction, and
what keeps the map owner-only and free of atomics.

**Because it is off by default, R-005 still stands for anyone running the
default or `secure` build.** The mitigation exists and is one flag away; it is
not in force.

**Why the bounds stay opt-in:** that is still enough to forfeit mimalloc parity — the
headline result of the optimization campaign — on real programs, not merely on
synthetic loops. perl 0.9991 → 1.0160, sqlite 1.0003 → 1.0061, calloc 0.949 →
1.041, batch_lifo 1.008 → 1.259. Services facing untrusted input should enable
it anyway and accept ~1–2%; the default build matches upstream mimalloc's
release posture plus double-free detection.

**These mitigations are tested for EFFICACY, not merely for function.** That
distinction was a real gap until 2026-08-20: the tests in `secure.rs` all
write strictly inside legitimately-allocated blocks, so nothing in the crate
had ever been observed to fire. `tests/corruption.rs` now poisons a genuine
free list two ways — blunt overflow filler, and a high-bit flip that keeps the
decoded pointer perfectly ALIGNED so it defeats every alignment-based defence
— and asserts the process dies of SIGABRT rather than SIGSEGV. The signal is
the whole assertion: SIGSEGV would mean the allocator decoded the attacker's
bytes and followed them, and a test asserting only "the child died" would pass
in exactly the case the mitigation failed. Confirmed by disabling the check
and watching the signal flip.

## Side-channel analysis (H-35)

Performed 2026-08-20. The row previously read `N/A` on the grounds that "no
secret-equality comparison exists… not a MAC verify". That answered only half
the gate — which asks for constant-time behaviour **and no secret branches** —
and it stopped being true the same day, when the free-list link check grew a
branch on a key-derived value. The correct answer is not "inapplicable" but
"analysed, with one characterised residual".

### What is secret here

Three values, none of them user data: the per-page free-list keys
(`keys = [rng.next_usize() | 1, rng.next_usize()]`, regenerated for every
page), the per-heap ChaCha8 state (OS-seeded at heap creation), and — only
when guarded sampling is switched on — the sampling countdown. Their sole
value is to an attacker already executing inside the process. That is exactly
why H-20 is `N/A` while this row is not: H-20 asks whether user secrets are
protected (there are none), this one asks whether these values leak in a way
that defeats the mitigation they exist to power.

### Channel 1 — the link check is a secret-dependent branch

`link_is_plausible(dec, b.addr())` branches on `dec = (enc ^ k0) - k1`, which
is key-derived. That is a secret branch, and denying it would be false.

It carries no timing signal, though, and for a specific reason: **its outcome
is invariant across every legitimate input.** A valid link always passes both
conditions, so on any non-corrupt workload the branch never varies and there is
nothing to distinguish. It speaks only by aborting, which is its purpose.

### Channel 2 — the abort oracle (real, self-destroying)

An attacker with a write primitive can submit a chosen `enc` and learn one bit
from whether the process dies: did that value decode block-aligned and inside
the segment? That is a genuine oracle.

It does not accumulate. Each query costs the whole process, and on restart the
heap CSPRNG reseeds from OS entropy, so the keys it was probing no longer
exist. Even within one process the keys are per-page, so a fresh page voids
whatever was learned. An oracle that destroys its own secret on every use is
not a practical attack, and that — not "no secret branches" — is the honest
reason this is acceptable.

### Channel 3 — the encoding does not resist a READ primitive

This is the real residual (**R-005**), and it is inherited from mimalloc's
scheme rather than introduced here.

Encoding is `enc = (next + k1) ^ k0`. An attacker who can *read* a freed
block's link word and who knows or can infer `next` obtains one equation in
the two unknowns. Two such pairs from the same page are solvable bit-by-bit
from the least-significant end, as mixed XOR/add systems generally are.

Full key recovery is not even required for the practical attack. To redirect a
link to a target `T` near a known block `n1`, the attacker needs
`(T + k1) ^ (n1 + k1)`, which collapses to `T ^ n1` in every bit position
where the addition does not carry. Targets close to a known block are
therefore forgeable with high probability and no key knowledge at all. Since
the same-segment bound already confines targets to one segment, and blocks
within a page are close together, this residual is precisely the case the
bound cannot help with: **intra-page redirection, i.e. type confusion between
two live objects of the same size class.**

The posture to state plainly: free-list encoding is designed against a *blind*
overwrite. It is not designed against read-plus-write, and it does not survive
one. Also noted for completeness — `k0` is forced odd (`| 1`), so one bit of
64 is known a priori; negligible on its own, but it is not zero.

### Channel 4 — RNG modulo timing (negligible)

`Random::below(n)` is `next_usize() % n`: a hardware divide whose dividend is
secret and whose latency can vary with operand values on some cores. It is
reachable only when guarded sampling is enabled (inert by default), and the
value it computes decides *which* allocation gets a guard page — something the
attacker can already observe directly from the guard page itself. The channel
therefore reveals nothing that is not already in plain sight.

### Channel 5 — returned pointers leak nothing

The encoding exists only inside freed memory. Every pointer handed to a caller
is a real block address at a deterministic page offset, so no key-derived value
ever crosses the API boundary.

### The CSPRNG itself

ChaCha8 is an ARX construction — add, rotate, XOR. No S-boxes, no table
lookups, no data-dependent branches or memory accesses, so it is structurally
constant-time; there is no cache-timing surface of the AES-table variety to
have. Its core is pinned against RFC 8439 §2.1.1 (see H-34).

### Conclusion

No constant-time *comparison* obligation exists (no MAC verify, no secret
equality anywhere). One secret-dependent branch exists; it is invariant on
legitimate inputs and self-limiting as an oracle. The residual worth tracking
is R-005: an attacker holding both read and write primitives can forge
intra-page links, which encoding was never built to prevent.

## Residual risks

Tracked with owners and review dates in the audit plan files:
[workspace](plans/use-protection-please.md) ·
[core](../crates/rusty_alloc/docs/plans/use-protection-please.md) ·
[api](../crates/rusty_alloc_api/docs/plans/use-protection-please.md).
Headlines: R-001 foreign-pointer trust in `free` (upstream parity, `secure`
mitigates), R-002 bespoke ChaCha8 for hardening randomness (vetted against RFC
8439 vectors as of 2026-08-19 — see H-34), R-003 OOM abort in heap creation,
R-004 no coverage-guided fuzzing yet (targets landing; the soak is scheduled
work), R-005 free-list encoding does not survive a read primitive, so an
attacker holding read AND write can forge intra-page links — mitigable with
`blockmap` but NOT mitigated by default, since that costs ~3x `secure` (see
the side-channel analysis above).
