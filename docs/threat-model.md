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

## What the `secure` feature adds (opt-in, measured 4–7%)

Guard pages around guarded-sampled objects, per-page encrypted free-list
links, a same-segment bound on every decoded link, and guarded-object
sampling. Services facing untrusted input should enable it; the default build
matches upstream mimalloc's release posture plus double-free detection.

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

## Residual risks

Tracked with owners and review dates in the audit plan files:
[workspace](plans/use-protection-please.md) ·
[core](../crates/rusty_alloc/docs/plans/use-protection-please.md) ·
[api](../crates/rusty_alloc_api/docs/plans/use-protection-please.md).
Headlines: R-001 foreign-pointer trust in `free` (upstream parity, `secure`
mitigates), R-002 bespoke ChaCha8 for hardening randomness, R-003 OOM abort
in heap creation, R-004 no coverage-guided fuzzing yet (targets landing; the
soak is scheduled work).
