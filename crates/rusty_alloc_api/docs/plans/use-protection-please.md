# rusty_alloc-api — hardening audit

**Standard**: remade-with-rust recursive hardening process — see the skill's `STANDARD.md`
**Registry**: 41 gates / 12 phases (`use-protection-please` v1)
**Unit**: `crates/rusty_alloc_api` — published library crate (safe surface)
**Tier**: standard — a thin safe shim (`GlobalAlloc`, `Heap`, `Allocator`) over
the core; it consumes no untrusted bytes itself — the hostile-input surface is
audited in the core crate's plan.
**Mirrors**: the crates.io page (<https://crates.io/crates/rusty_alloc-api>,
renders this crate's README at each publish) — must be re-rendered in the same
pass as this file (SKILL.md §3.1)
**Compliance**: none — a library; no framework declared in scope
**Architect**: Tim — Mata Network
**Audit depth**: survey, plus the tool probes genuinely executed on 2026-08-19
**Audited**: 2026-08-19 by Claude Fable 5 (session audit) · **Next review**: 2026-11-19

> Source of truth for this unit's hardening status. The README's status table is
> **generated from this file** — edit here, then run the renderer.

**Status tokens**: `Completed` (evidenced pass) · `Scheduled` (owner + date in Target) ·
`Incomplete` (not done, or not evidenced) · `N/A` (out of tier — reason required in
Evidence; excluded from the totals).

---

## Threat sketch

*Assets* — the soundness of the safe abstraction: every `unsafe` call into the
core must uphold the core's contracts from safe Rust.
*Adversaries* — a safe-Rust caller driving the API into a contract violation
(double free via `Heap` misuse, layout mismatch in `GlobalAlloc`).
*Highest-value attack path* — a safe API sequence that reaches the core with a
violated precondition; the core's double-free abort and Miri are the nets.
*Full model* — [docs/threat-model.md](../../../../docs/threat-model.md) (2026-08-19).

---

## Checklist

`★` = v1.0.0-blocking. Full probe and pass criteria per gate: the skill's `CHECKLIST.md`.

### Phase 0 — Threat modeling

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-01 | ★ Threat model documented and linked from README | Completed | Links `docs/threat-model.md` from the crate README (absolute URL for crates.io) | |
| H-02 | Threat model revisited after last major change | N/A | tier `crit` only | |

### Phase 1 — Toolchain

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-03 | Toolchain pinned (`rust-toolchain.toml`) | Completed | Workspace pin `1.97.1` | |
| H-04 | Committed `.cargo/config.toml` hardening defaults | Completed | `.cargo/config.toml` (2026-08-19): full RELRO (`-z relro -z now`) + `-z noexecstack` on both Linux targets, `/NXCOMPAT /DYNAMICBASE /CETCOMPAT` on MSVC. VERIFIED IN THE ARTIFACT, not assumed — `readelf` on the shipped cdylib shows `GNU_RELRO`, `FLAGS: BIND_NOW`, and `GNU_STACK RW` (non-exec). Frame pointers DELIBERATELY excluded and measured: +6.14 Ir/op batch (+10.4%), +11.01 small (+14.0%) — they reinstate the prologues M13/M16/the 2026-08-19 campaign removed; the debuggability intent is met by `debug = true` shipping full DWARF instead. Deviation stated in the file | |
| H-05 | ★ Release profile hardened (overflow-checks, LTO, panic policy) | Completed | Inherits the workspace profile; `overflow-checks` WAIVED by the owner (Tim) 2026-08-20 on the measured +7.1% batch / +11% mixed cost, time-bounded in the core plan Waivers table | |
| H-06 | Security toolchain available to CI and developers | Completed | Workspace CI: version-pinned deny/audit/fuzz/miri/clippy/rustfmt | |

### Phase 2 — Supply chain

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-07 | ★ `Cargo.lock` committed | Completed | Workspace lockfile tracked as of 2026-08-19 | |
| H-08 | ★ `deny.toml` policy present and enforced | Completed | Workspace `cargo deny check` all-ok (2026-08-19); per-PR in CI | |
| H-09 | ★ Vulnerability scan clean (`cargo audit`) | Completed | Clean over the workspace lockfile (2026-08-19); per-PR + weekly cron | |
| H-10 | ★ `cargo vet` coverage complete | N/A | tier `crit` only | |
| H-11 | Unsafe inventory measured and trending down (geiger) | N/A | tier `crit` only | |
| H-12 | ★ SBOM generated and published with releases | Completed | CycloneDX 1.5 SBOMs for both published crates are ATTACHED to the v0.7.0 GitHub release (`rusty_alloc.cdx.json`, `rusty_alloc-api.cdx.json`, verified via `gh release view`), generated from the committed lockfile. `.github/workflows/release.yml` regenerates and attaches them on every `v*` tag, so this cannot silently lapse | |
| H-13 | Git deps pinned; no unknown registries or sources | Completed | Path+version dep on the core only; `[sources]` enforced workspace-wide | |
| H-14 | Dependency freshness reviewed, human-in-the-loop updates | Completed | `.github/dependabot.yml` (2026-08-19): weekly cargo, monthly for the separate `/fuzz` workspace (which would otherwise never update), monthly github-actions so the SHA pins do not become permanently stale. No auto-merge — every update PR runs the full hardening gate and needs review | |

### Phase 3 — Code level

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-15 | ★ Workspace lint policy set and clean | Completed | `clippy::pedantic` AND `clippy::nursery` are enabled workspace-wide and `cargo clippy --workspace --all-targets --all-features -- -D warnings` is CLEAN under them (2026-08-19). Method: turning both groups on raised 481 warnings across ~25 lints; each was triaged, the ~20 declined categories are listed in `Cargo.toml` WITH A REASON EACH (doc style, API shape, and the audited cast family), and the rest were FIXED — literal separators, `let...else`, hoisted consts, `cast_mut`/`&raw` conversions. Net: ~95 additional lints now enforced. Per-PR in CI | |
| H-16 | ★ `unsafe` isolated, SAFETY-commented, inventoried | Completed | SAFETY comments lint-enforced; the crate's 14 sites are in the workspace `UNSAFE.md` inventory (GlobalAlloc/Allocator contract row) | |
| H-17 | Arithmetic safety explicit | Completed | No size arithmetic in the shim: layouts pass through to the core, which owns the checked math (grep: 0 arithmetic-discipline sites needed, 0 raw `as` on lengths) | |
| H-18 | ★ No `unwrap`/`expect`/panic on untrusted paths; typed errors | Completed | grep 2026-08-19: 0 `unwrap()`, 0 `expect(` in src/ | |
| H-19 | Input validation — external bytes treated as hostile | N/A | tier `crit` only — the shim consumes no external bytes | |
| H-20 | ★ Secrets zeroized; never logged | N/A | tier `crit` only; unit holds no secrets, no logging | |
| H-21 | Concurrency discipline | N/A | tier `crit` only — 0 manual Send/Sync impls, no shared state of its own | |

### Phase 4 — Static analysis

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-22 | Static analysis beyond the default linter runs on every PR | N/A | tier `crit` only | |

### Phase 5 — Dynamic analysis

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-23 | ★ Tests pass under Miri | Completed | `tests/api_miri.rs` added (GlobalAlloc round-trips incl. zeroed + realloc-prefix, first-class Heap delete-migration and destroy): `cargo +nightly miri test -p rusty_alloc-api` = **5 passed, 0 failed** (2026-08-19) — no longer vacuous | |
| H-24 | Critical paths pass the sanitizers (ASan/MSan/TSan) | N/A | tier `crit` only | |
| H-25 | `cargo careful test` green | N/A | tier `crit` only | |

### Phase 6 — Fuzzing and properties

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-26 | ★ Fuzz target per public parser, decoder, or message handler | N/A | tier `crit` only — no parser/decoder/message surface; fuzzing lands in the core | |
| H-27 | ★ Continuous fuzzing with no open crashes | N/A | tier `crit` only | |
| H-28 | Property tests cover the documented invariants | Completed | `tests/properties.rs` (proptest, dev-only): 8 properties over generated sizes/alignments spanning every routing decision (small bins → medium → the 64 KiB cutoff → large spans → huge segments). Each is a claim the crate makes in prose — `usable_size` ≥ request and stable; `zalloc` zero across the FULL usable extent (dirtying a same-class block first so recycled memory is the likely case); aligned blocks actually aligned; `realloc` preserves `min(old,new)` across a move; **live blocks never overlap** (per-block tags verified only after ALL are live, so an overlap cannot be masked by a later write); `good_size` idempotent and never shrinking; `usable_size` agrees with `good_size`; `free(null)`/`malloc(0)` edges. Non-vacuous: runtime scales with `PROPTEST_CASES` (0.02s at 256 → 0.46s at 8192). Runs under MIRI TOO (8/8 passed, 20.8s interpreted, 4 cases each): proptest's file failure-persistence needs `getcwd`, which Miri isolation denies, so it is disabled in the config — the properties are now UB-checked rather than skipped. | |
| H-29 | Mutation and/or differential testing on critical modules | N/A | tier `crit` only | |

### Phase 7 — Formal verification

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-30 | Proof of panic-freedom / UB-freedom per `unsafe` module | N/A | tier `crit` only | |

### Phase 8 — Build and binary

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-31 | ★ Binary hardening applied and verified | N/A | tier `bin` — library crate, no shipped binary | |
| H-32 | Build is reproducible or fully auditable | N/A | tier `bin` | |

### Phase 9 — Runtime privilege

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-33 | Least privilege documented and tested | N/A | tier `bin` — a library | |

### Phase 10 — Cryptography

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-34 | Vetted crypto only; no bespoke primitives | N/A | tier `crit` only — no crypto in the shim | |
| H-35 | Side-channel discipline (constant-time, no secret branches) | N/A | tier `crit` only | |
| H-36 | Post-quantum migration plan for long-lived keys | N/A | tier `crit` only | |

### Phase 11 — CI/CD, release, and operations

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-37 | CI runs the hardening gate on every PR | Completed | Workspace CI (see root plan H-37) | |
| H-38 | Releases signed, attested, and changelogged for security | Incomplete | Machine-side DONE (2026-08-19): `release.yml` now emits `SHA256SUMS.txt` alongside the SBOMs and calls `actions/attest-build-provenance`, so every `v*` tag produces a signed, GitHub-attested statement of what built the artifacts and from which commit. Security-relevant changes are recorded per-milestone in `docs/LEDGER.md`. **The remaining half needs a human key**: tags are unsigned (`v0.4.0`, `v0.7.0`), and only the owner can hold the signing key — configure `user.signingkey` + `tag.gpgSign`, or adopt sigstore. Left Incomplete rather than claimed | |
| H-39 | ★ `SECURITY.md` with a coordinated disclosure process | Completed | Repo-root `SECURITY.md` covers both published crates | |
| H-40 | Advisory monitoring and scheduled re-audit | Completed | Workspace monitoring (per-PR + weekly cron); quarterly re-audit | |
| H-41 | ★ Residual risks listed and accepted; waivers time-bounded | Completed | R-001..R-005 accepted by the owner (Tim) 2026-08-20; the two release waivers (H-05, H-27) are time-bounded in the core plan | |

### Phase 12 — Compliance controls

Only in play when a framework is declared in scope above. With none in scope, every row is
`N/A` — reason: "no compliance framework in scope". Mapping: the skill's `COMPLIANCE.md`.

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| C-01 | Data inventory — personal/health/card data touched | N/A | no compliance framework in scope | |
| C-02 | Data-flow map including third-party egress | N/A | no compliance framework in scope | |
| C-03 | Encryption in transit for all egress | N/A | no compliance framework in scope | |
| C-04 | Encryption at rest for stored sensitive data | N/A | no compliance framework in scope | |
| C-05 | Key management — generation, storage, rotation, destruction | N/A | no compliance framework in scope | |
| C-06 | Retention limits and honoured deletion | N/A | no compliance framework in scope | |
| C-07 | Audit logging of security-relevant events | N/A | no compliance framework in scope | |
| C-08 | Log hygiene — no PII, secrets, or card data in logs | N/A | no compliance framework in scope | |
| C-09 | Least-privilege access to sensitive data | N/A | no compliance framework in scope | |
| C-10 | Subprocessor and third-party inventory | N/A | no compliance framework in scope | |
| C-11 | Incident response and breach notification path | N/A | no compliance framework in scope | |
| C-12 | Change management — reviewed, approved, traceable | N/A | no compliance framework in scope | |
| C-13 | Availability commitments and their evidence | N/A | no compliance framework in scope | |
| C-14 | Machine-readable SBOM + provenance for regulators | N/A | no compliance framework in scope | |

---

## Scheduled work

Proposed order; Owner/Target are the human step.

| # | Gates | Work | Owner | Target | Notes |
|---|---|---|---|---|---|
| 1 | H-23 | Add Miri-runnable tests to this crate (GlobalAlloc round-trips, Heap create/alloc/drop) — the current run is vacuous (0 tests) | | | |
| 2 | H-16 | Cover the 14 unsafe sites in the workspace `UNSAFE.md` | | | |
| 3 | workspace | H-07/H-08/H-39/H-03/H-05 resolve at the workspace root — see the root and core plans | | | |

---

## Residual risk register

| ID | Risk | Likelihood | Impact | Mitigation status | Accepted by | Review date |
|---|---|---|---|---|---|---|
| R-001 | The safe abstraction's soundness is exercised under Miri only via the core's suites, not its own | Low | Medium | core suites + double-free abort net | pending | 2026-11-19 |

---

## Waivers

| Gate | Reason | Granted by | Expires |
|---|---|---|---|
| | | | |

---

## Audit log

| Date | Depth | Auditor | Completed / Scheduled / Incomplete | ★ met | Note |
|---|---|---|---|---|---|
| 2026-08-19 | survey+cited tools | Claude Fable 5 | 2 / 0 / 20 (33 N/A) | 1/12 | first pass; ★ blockers: H-01 H-05 H-07 H-08 H-09 H-12 H-15 H-16 H-23 H-39 H-41 |
| 2026-08-19 | deep (tools executed) | Claude Fable 5 | 15 / 0 / 7 (33 N/A) | 8/12 | same-day execution pass. ★ blockers left: H-05(waiver) H-12 H-15 H-41(owner) |
