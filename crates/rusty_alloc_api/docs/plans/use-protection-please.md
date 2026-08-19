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
*Full model* — covered by the core crate's model once H-01 lands there.

---

## Checklist

`★` = v1.0.0-blocking. Full probe and pass criteria per gate: the skill's `CHECKLIST.md`.

### Phase 0 — Threat modeling

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-01 | ★ Threat model documented and linked from README | Incomplete | None; will link the core crate's model when it exists | |
| H-02 | Threat model revisited after last major change | N/A | tier `crit` only | |

### Phase 1 — Toolchain

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-03 | Toolchain pinned (`rust-toolchain.toml`) | Incomplete | Workspace file exists; `channel = "stable"` floats (needs `1.x.y`) | |
| H-04 | Committed `.cargo/config.toml` hardening defaults | Incomplete | Absent at workspace root | |
| H-05 | ★ Release profile hardened (overflow-checks, LTO, panic policy) | Incomplete | Inherits workspace `[profile.release]` — deliberate LTO/cgu/panic, no `overflow-checks = true` (see core plan H-05) | |
| H-06 | Security toolchain available to CI and developers | Incomplete | CI lacks deny/audit/vet/geiger (workspace-level) | |

### Phase 2 — Supply chain

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-07 | ★ `Cargo.lock` committed | Incomplete | Workspace `.gitignore` ignores `Cargo.lock` | |
| H-08 | ★ `deny.toml` policy present and enforced | Incomplete | None (workspace-level) | |
| H-09 | ★ Vulnerability scan clean (`cargo audit`) | Incomplete | Not run; sole dependency is the sibling core crate | |
| H-10 | ★ `cargo vet` coverage complete | N/A | tier `crit` only | |
| H-11 | Unsafe inventory measured and trending down (geiger) | N/A | tier `crit` only | |
| H-12 | ★ SBOM generated and published with releases | Incomplete | No SBOM on the v0.7.0 release | |
| H-13 | Git deps pinned; no unknown registries or sources | Incomplete | Zero git deps (path+version dep on the core only); `[sources]` enforcement pending `deny.toml` | |
| H-14 | Dependency freshness reviewed, human-in-the-loop updates | Incomplete | No update-bot config | |

### Phase 3 — Code level

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-15 | ★ Workspace lint policy set and clean | Incomplete | Same workspace lints + clippy clean (2026-08-19, per-PR CI); pedantic/nursery not configured | |
| H-16 | ★ `unsafe` isolated, SAFETY-commented, inventoried | Incomplete | 14 `unsafe` occurrences, SAFETY comments lint-enforced (`undocumented_unsafe_blocks = deny`, clippy clean); no `UNSAFE.md` | |
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
| H-23 | ★ Tests pass under Miri | Incomplete | `cargo +nightly miri test -p rusty_alloc-api` run 2026-08-19: **0 tests executed (1 ignored) — a vacuous pass, which the registry rules Incomplete.** The crate's behaviour is exercised under Miri only indirectly through the core's suites; it needs its own Miri-runnable tests | |
| H-24 | Critical paths pass the sanitizers (ASan/MSan/TSan) | N/A | tier `crit` only | |
| H-25 | `cargo careful test` green | N/A | tier `crit` only | |

### Phase 6 — Fuzzing and properties

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-26 | ★ Fuzz target per public parser, decoder, or message handler | N/A | tier `crit` only — no parser/decoder/message surface; fuzzing lands in the core | |
| H-27 | ★ Continuous fuzzing with no open crashes | N/A | tier `crit` only | |
| H-28 | Property tests cover the documented invariants | Incomplete | No property tests in the crate | |
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
| H-37 | CI runs the hardening gate on every PR | Incomplete | fmt/clippy/check/test/Miri(core)/wasm per PR; no deny/audit; actions tag-pinned not SHA-pinned | |
| H-38 | Releases signed, attested, and changelogged for security | Incomplete | Unsigned tags; no attestation | |
| H-39 | ★ `SECURITY.md` with a coordinated disclosure process | Incomplete | Absent (workspace-level fix) | |
| H-40 | Advisory monitoring and scheduled re-audit | Incomplete | None recorded | |
| H-41 | ★ Residual risks listed and accepted; waivers time-bounded | Incomplete | Register below listed; acceptance pending the owner | |

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
