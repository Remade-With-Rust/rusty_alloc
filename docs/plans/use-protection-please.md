# rusty_alloc (workspace) — hardening audit

**Standard**: remade-with-rust recursive hardening process — see the skill's `STANDARD.md`
**Registry**: 41 gates / 12 phases (`use-protection-please` v1)
**Unit**: `.` — virtual workspace root (repo-level posture: toolchain, supply
chain, CI/CD, release, disclosure)
**Tier**: critical-path — the workspace ships a general-purpose allocator; the
repo-level gates inherit the product's tier.
**Mirrors**: none of its own — the GitHub repo front page IS this README; the
two published crates mirror their own crate READMEs (see the member plans)
**Compliance**: none — no framework declared in scope
**Architect**: Tim — Mata Network
**Audit depth**: survey, plus the tool probes genuinely executed on 2026-08-19
**Audited**: 2026-08-19 by Claude Fable 5 (session audit) · **Next review**: 2026-11-19

> Source of truth for this unit's hardening status. The README's status table is
> **generated from this file** — edit here, then run the renderer.
> Code-level gates are audited in the member plans
> (`crates/rusty_alloc/docs/plans/use-protection-please.md`,
> `crates/rusty_alloc_api/docs/plans/use-protection-please.md`); this file owns
> everything the workspace owns.

**Status tokens**: `Completed` (evidenced pass) · `Scheduled` (owner + date in Target) ·
`Incomplete` (not done, or not evidenced) · `N/A` (out of tier — reason required in
Evidence; excluded from the totals).

---

## Threat sketch

*Assets* — the integrity of what ships to crates.io under the `rusty_alloc`
names; the supply chain of every downstream `#[global_allocator]` consumer.
*Adversaries* — supply-chain attackers (dependency or CI compromise, tag
spoofing), and reporters who need a disclosure channel that works.
*Highest-value attack path* — a poisoned release: no signed tags, no SBOM, no
lockfile in VCS means a tampered dependency tree is hard to detect after the
fact.
*Full model* — not yet written (H-01).

---

## Checklist

`★` = v1.0.0-blocking. Full probe and pass criteria per gate: the skill's `CHECKLIST.md`.

### Phase 0 — Threat modeling

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-01 | ★ Threat model documented and linked from README | Incomplete | No `docs/threat-model.md`, no `SECURITY.md` anywhere in the repo | |
| H-02 | Threat model revisited after last major change | Incomplete | Blocked on H-01 | |

### Phase 1 — Toolchain

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-03 | Toolchain pinned (`rust-toolchain.toml`) | Incomplete | Present (components + tier-1 targets) but `channel = "stable"` floats; pass needs `1.x.y` | |
| H-04 | Committed `.cargo/config.toml` hardening defaults | Incomplete | Absent | |
| H-05 | ★ Release profile hardened (overflow-checks, LTO, panic policy) | Incomplete | `[profile.release]`: `lto="thin"` + `codegen-units=1` + `panic="abort"` all deliberate and commented; **no `overflow-checks = true`** — needs the measured decision (see core plan) | |
| H-06 | Security toolchain available to CI and developers | Incomplete | `ci.yml` installs rustfmt/clippy/miri; deny/audit/vet/geiger absent from CI and the dev docs | |

### Phase 2 — Supply chain

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-07 | ★ `Cargo.lock` committed | Incomplete | **`.gitignore:2` ignores `Cargo.lock`** (`git check-ignore` confirms) | |
| H-08 | ★ `deny.toml` policy present and enforced | Incomplete | Absent. Dependency surface is deliberately tiny: `libc`, `windows-sys`, dev-only `loom` | |
| H-09 | ★ Vulnerability scan clean (`cargo audit`) | Incomplete | Never run; not in CI | |
| H-10 | ★ `cargo vet` coverage complete | Incomplete | No `supply-chain/` | |
| H-11 | Unsafe inventory measured and trending down (geiger) | Incomplete | Not run; member static baselines recorded 2026-08-19 (core 347/226, api 14) | |
| H-12 | ★ SBOM generated and published with releases | Incomplete | v0.7.0 released without an SBOM | |
| H-13 | Git deps pinned; no unknown registries or sources | Incomplete | Zero git deps and zero alternate registries across all six manifests; `[sources]` enforcement pending `deny.toml` | |
| H-14 | Dependency freshness reviewed, human-in-the-loop updates | Incomplete | No Renovate/Dependabot config | |

### Phase 3 — Code level

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-15 | ★ Workspace lint policy set and clean | Incomplete | `[workspace.lints]`: `unsafe_op_in_unsafe_fn = deny`, `clippy::undocumented_unsafe_blocks = deny`; `cargo clippy --workspace --all-targets --all-features -- -D warnings` exit 0 (2026-08-19 + per-PR CI). Pedantic/nursery posture undecided | |
| H-16 | ★ `unsafe` isolated, SAFETY-commented, inventoried | N/A | virtual workspace root — no src/; audited per member (core: lint-enforced SAFETY comments, `UNSAFE.md` missing) | |
| H-17 | Arithmetic safety explicit | N/A | virtual workspace root — audited per member | |
| H-18 | ★ No `unwrap`/`expect`/panic on untrusted paths; typed errors | N/A | virtual workspace root — audited per member (core has one real lead: `init.rs:499`) | |
| H-19 | Input validation — external bytes treated as hostile | N/A | virtual workspace root — audited per member | |
| H-20 | ★ Secrets zeroized; never logged | N/A | virtual workspace root — audited per member | |
| H-21 | Concurrency discipline | N/A | virtual workspace root — audited per member (core: Completed) | |

### Phase 4 — Static analysis

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-22 | Static analysis beyond the default linter runs on every PR | Incomplete | CI has clippy only | |

### Phase 5 — Dynamic analysis

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-23 | ★ Tests pass under Miri | Completed | CI runs `cargo +nightly miri test -p rusty_alloc` on every PR (`ci.yml` miri job); 2026-08-19 whole-target run: 33 tests, 0 UB (core plan H-23). The api member's own Miri coverage is vacuous — tracked there | |
| H-24 | Critical paths pass the sanitizers (ASan/MSan/TSan) | Incomplete | Never run (workspace CI has no sanitizer matrix) | |
| H-25 | `cargo careful test` green | Incomplete | Never run | |

### Phase 6 — Fuzzing and properties

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-26 | ★ Fuzz target per public parser, decoder, or message handler | Incomplete | No `fuzz/` anywhere in the workspace (v1 plan G5 specified 4 targets, never built) | |
| H-27 | ★ Continuous fuzzing with no open crashes | Incomplete | Blocked on H-26 | |
| H-28 | Property tests cover the documented invariants | Incomplete | Audited per member; no proptest/quickcheck anywhere | |
| H-29 | Mutation and/or differential testing on critical modules | Completed | The workspace's whole gate design is differential: the vendored C mimalloc oracle (G2 semantic trace diff), the 3-arm byte-identity real-world sweep (144/144, 2026-08-19), the 19-config corpus sweep — see core plan H-29 | |

### Phase 7 — Formal verification

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-30 | Proof of panic-freedom / UB-freedom per `unsafe` module | Incomplete | No Kani harnesses in the workspace; loom covers the xthread protocol (core) | |

### Phase 8 — Build and binary

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-31 | ★ Binary hardening applied and verified | N/A | tier `bin` — the workspace publishes two rlib crates; the only cdylib is `publish = false` dev tooling | |
| H-32 | Build is reproducible or fully auditable | N/A | tier `bin` — no shipped binary artifact | |

### Phase 9 — Runtime privilege

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-33 | Least privilege documented and tested | N/A | tier `bin` — libraries only | |

### Phase 10 — Cryptography

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-34 | Vetted crypto only; no bespoke primitives | Incomplete | The core carries a bespoke ChaCha8 (`random.rs`) — tracked in the core plan (R-002) | |
| H-35 | Side-channel discipline (constant-time, no secret branches) | N/A | audited per member (core: N/A — no secret comparisons) | |
| H-36 | Post-quantum migration plan for long-lived keys | N/A | audited per member (core: N/A — no long-lived keys) | |

### Phase 11 — CI/CD, release, and operations

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-37 | CI runs the hardening gate on every PR | Incomplete | Per-PR: fmt, clippy(-D warnings, all features), check, test(all features), Miri, wasm-EXECUTE, oracle build — a strong base; missing deny/audit, and actions are tag-pinned (`@v4`) not SHA-pinned | |
| H-38 | Releases signed, attested, and changelogged for security | Incomplete | `v0.4.0`/`v0.7.0` tags unsigned; no provenance; `docs/LEDGER.md` documents security-relevant changes in prose (a real asset) but no CHANGELOG discipline | |
| H-39 | ★ `SECURITY.md` with a coordinated disclosure process | Incomplete | Absent. Informal process demonstrably works (FFAI's 0.3.1 report → same-day fix + yank recommendation); needs a contact + window + policy in writing | |
| H-40 | Advisory monitoring and scheduled re-audit | Incomplete | No RustSec feed subscription recorded; this audit sets the first Next-review date | |
| H-41 | ★ Residual risks listed and accepted; waivers time-bounded | Incomplete | Register below; acceptance pending the owner | |

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

Proposed execution order (cheapest-first); **Owner/Target are the human step** —
nothing is Scheduled until both are filled.

| # | Gates | Work | Owner | Target | Notes |
|---|---|---|---|---|---|
| 1 | H-07 | Remove `Cargo.lock` from `.gitignore`, commit the lockfile | | | one line; unblocks H-08/09/10 |
| 2 | H-08, H-09, H-13 | `deny.toml` + `cargo audit`/`cargo deny check` in CI | | | 3 external deps — minutes |
| 3 | H-39, H-01 | `SECURITY.md` + `docs/threat-model.md` | | | contact address is the owner's call |
| 4 | H-03, H-37 | Pin toolchain `1.x.y`; SHA-pin CI actions; add deny/audit jobs | | | |
| 5 | H-05 | Measure `overflow-checks = true` (icount A/B), keep or waive with the number | | | |
| 6 | H-10 | `cargo vet init` + certify the 3 deps | | | |
| 7 | H-26, H-27 | cargo-fuzz targets over the `.ratrace` format + a 30-day soak | | | the ★ long pole — start it early |
| 8 | H-12 | SBOM in the release flow | | | |
| 9 | H-38 | Sign tags; attach auditable artifacts | | | |
| 10 | H-41 | Owner reviews + accepts the risk registers (all three plans) | | | closes the last ★ |

---

## Residual risk register

| ID | Risk | Likelihood | Impact | Mitigation status | Accepted by | Review date |
|---|---|---|---|---|---|---|
| R-001 | Unsigned releases + no lockfile in VCS = weak post-hoc tamper evidence | Low | High | crates.io checksums only | pending | 2026-11-19 |
| R-002 | No fuzzing has ever run against the published surface | Medium | High | differential oracle + Miri + loom (not coverage-guided) | pending | 2026-11-19 |

---

## Waivers

| Gate | Reason | Granted by | Expires |
|---|---|---|---|
| | | | |

---

## Audit log

| Date | Depth | Auditor | Completed / Scheduled / Incomplete | ★ met | Note |
|---|---|---|---|---|---|
| 2026-08-19 | survey+cited tools | Claude Fable 5 | 2 / 0 / 28 (25 N/A) | 1/13 | first pass; ★ blockers: H-01 H-05 H-07 H-08 H-09 H-10 H-12 H-15 H-26 H-27 H-39 H-41 |
