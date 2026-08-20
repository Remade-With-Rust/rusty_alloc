# rusty_alloc (core) — hardening audit

**Standard**: remade-with-rust recursive hardening process — see the skill's `STANDARD.md`
**Registry**: 41 gates / 12 phases (`use-protection-please` v1)
**Unit**: `crates/rusty_alloc` — published library crate (allocator core)
**Tier**: critical-path — a general-purpose allocator interposed under arbitrary
processes: `free()`/`realloc()`/`usable_size()` receive attacker-influenceable
pointers, `malloc()` receives attacker-influenceable sizes, and `page_of`
derives metadata from the pointer itself. Anything eating bytes from outside
the process is critical-path.
**Mirrors**: the crates.io page (<https://crates.io/crates/rusty_alloc>, renders
this crate's README at each publish) — every one of these carries the generated
block and **must be re-rendered in the same pass as this file** (SKILL.md §3.1)
**Compliance**: none — a library allocator; no framework declared in scope
**Architect**: Tim — Mata Network
**Audit depth**: survey, plus the tool probes genuinely executed on 2026-08-19
(clippy, Miri, the full test battery — cited per-row)
**Audited**: 2026-08-19 by Claude Fable 5 (session audit) · **Next review**: 2026-11-19

> Source of truth for this unit's hardening status. The README's status table is
> **generated from this file** — edit here, then run:
> `python <skills>/use-protection-please/scripts/render_readme_table.py --plan docs/plans/use-protection-please.md --readme README.md`

**Status tokens**: `Completed` (evidenced pass) · `Scheduled` (owner + date in Target) ·
`Incomplete` (not done, or not evidenced) · `N/A` (out of tier — reason required in
Evidence; excluded from the totals).

---

## Threat sketch

*Assets* — heap metadata integrity (segment/page headers, free lists), the
host process's memory safety, allocator availability (an abort is a DoS).
*Adversaries* — a caller passing hostile arguments (forged/interior/foreign
pointers to `free`, extreme sizes to `malloc`), an exploit primitive author
using the allocator's structures (free-list poisoning, cross-thread races),
and a supply-chain attacker targeting the two external deps.
*Highest-value attack path* — `free(p)` with a forged pointer: `page_of`
trusts `slice_offset` read from the pointer's own segment, so a pointer that
is not ours yields an arbitrary `slot.sub(off)` (upstream mimalloc has the
identical shape; `secure` adds encrypted free lists + guard pages).
*Full model* — [docs/threat-model.md](../../../../docs/threat-model.md) (2026-08-19).

---

## Checklist

`★` = v1.0.0-blocking. Full probe and pass criteria per gate: the skill's `CHECKLIST.md`.

### Phase 0 — Threat modeling

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-01 | ★ Threat model documented and linked from README | Completed | `docs/threat-model.md` (assets, adversaries, trust boundaries, STRIDE pass, 2026-08-19); linked from the root README §Security and this crate's README | |
| H-02 | Threat model revisited after last major change | Completed | Model dated 2026-08-19, same day as the last code change; revisit triggers named in the model header | |

### Phase 1 — Toolchain

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-03 | Toolchain pinned (`rust-toolchain.toml`) | Completed | `channel = "1.97.1"` + components + tier-1 targets; CI jobs pin the same version explicitly | |
| H-04 | Committed `.cargo/config.toml` hardening defaults | Incomplete | No `.cargo/config.toml` in the repo | |
| H-05 | ★ Release profile hardened (overflow-checks, LTO, panic policy) | Incomplete | LTO/cgu/panic deliberate. `overflow-checks = true` now MEASURED (2026-08-19, opscan): batch +4.19 Ir/op (+7.1%, would fall back behind mimalloc), mixed +15.6 (+11%), med +4.3 — a waiver is PROPOSED on that number (untrusted-size arithmetic is explicitly `checked_*`; every debug/test/Miri build runs with overflow checks on). Owner decision: waive or eat it | |
| H-06 | Security toolchain available to CI and developers | Completed | CI installs version-pinned cargo-deny 0.20.2, cargo-audit 0.22.2, cargo-fuzz 0.13.2, miri, clippy, rustfmt; vet/geiger arrive with their gates | |

### Phase 2 — Supply chain

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-07 | ★ `Cargo.lock` committed | Completed | `.gitignore` line removed; `Cargo.lock` tracked as of 2026-08-19 | |
| H-08 | ★ `deny.toml` policy present and enforced | Completed | `deny.toml` covers advisories+licenses+bans+sources; `cargo deny check` = `advisories ok, bans ok, licenses ok, sources ok` (2026-08-19, deep run); per-PR in CI. The ban policy caught `cc` via dev-only loom→generator on its first run — scoped `wrappers` exception with written justification | |
| H-09 | ★ Vulnerability scan clean (`cargo audit`) | Completed | `cargo audit --deny warnings` clean over 46 lockfile deps (2026-08-19); per-PR + weekly cron in CI | |
| H-10 | ★ `cargo vet` coverage complete | Incomplete | No `supply-chain/`; vet never initialised | |
| H-11 | Unsafe inventory measured and trending down (geiger) | Incomplete | Not run; static census 2026-08-19: 347 `unsafe` occurrences / 226 `SAFETY:` comments across src/ (baseline for the trend) | |
| H-12 | ★ SBOM generated and published with releases | Incomplete | No SBOM on the v0.7.0 release; `cargo auditable`/`cyclonedx` not in the release flow | |
| H-13 | Git deps pinned; no unknown registries or sources | Completed | Zero git deps; `deny.toml` `[sources]` denies unknown registries/git, allow-list = crates.io only; `sources ok` in the deep run | |
| H-14 | Dependency freshness reviewed, human-in-the-loop updates | Incomplete | No Renovate/Dependabot config; no triaged outdated report | |

### Phase 3 — Code level

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-15 | ★ Workspace lint policy set and clean | Incomplete | `[workspace.lints]` denies `unsafe_op_in_unsafe_fn` + `clippy::undocumented_unsafe_blocks`; `cargo clippy --all-targets --all-features -- -D warnings` exit 0 (run 2026-08-19, and per-PR in CI). Below the bar only because pedantic/nursery are not configured | |
| H-16 | ★ `unsafe` isolated, SAFETY-commented, inventoried | Completed | SAFETY comments lint-enforced (`undocumented_unsafe_blocks = deny`, clippy clean); `UNSAFE.md` module inventory with purpose + last-audit dates + the four `unsafe impl` justifications + rules of engagement (2026-08-19) | |
| H-17 | Arithmetic safety explicit | Incomplete | Positive spot-checks: `calloc`/`mallocn`/`reallocn`/`recalloc` use `checked_mul`; `slice_offset` range guarded by a const assert. 19 explicit-arithmetic calls counted; the full `as`-cast review on untrusted sizes has not been done | |
| H-18 | ★ No `unwrap`/`expect`/panic on untrusted paths; typed errors | Completed | The `init.rs:499` OOM `expect` is FIXED (2026-08-19): heap-creation failure now propagates null through every allocation path (mimalloc parity); api constructors get a defined panic; the 2 remaining unwraps are the Miri-only mock. Cost priced at +1 Ir on the generic path only (batch 59.17, mixed IMPROVED to 139.07) | |
| H-19 | Input validation — external bytes treated as hostile | Incomplete | Sizes: overflow-checked, huge → dedicated segments, `align` power-of-two-validated. Pointers: `free` trusts `slice_offset` from the pointer's own segment — a FOREIGN pointer yields a wild `slot.sub(off)` (upstream parity; `secure` mitigates via encrypted free lists + guard pages; `mi_is_in_heap_region` exists but is not on the free path). Recorded as R-001 | |
| H-20 | ★ Secrets zeroized; never logged | N/A | No user/long-lived secret material in the unit: the per-page free-list keys and CSPRNG state are in-process hardening state, valueless outside the live process; the crate has no logging at all (0 log macros) | |
| H-21 | Concurrency discipline | Completed | 4 manual `unsafe impl Send/Sync` (`TlsSlot`, `EmptyPage`, `EmptyHeapBox`), each with a written justification at the impl; no `static mut`; the cross-thread free protocol is loom-model-checked (`tests/loom_xthread.rs`, written before the implementation) and Miri-clean including the MT storm (33 tests, 2026-08-19) | |

### Phase 4 — Static analysis

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-22 | Static analysis beyond the default linter runs on every PR | Incomplete | CI runs clippy only; no Semgrep/MIRAI/pattern rules | |

### Phase 5 — Dynamic analysis

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-23 | ★ Tests pass under Miri | Completed | `cargo +nightly miri test -p rusty_alloc`, whole target, 2026-08-19: **33 tests passed, 0 UB, 0 leaks**, including `stress_mt` (the abandon/adopt storm, 411 s interpreted) and `rss_churn`; also per-PR in CI (`ci.yml` miri job). Register-read tests are `#[cfg_attr(miri, ignore)]` with in-file reasons | |
| H-24 | Critical paths pass the sanitizers (ASan/MSan/TSan) | Incomplete | ASan now runs implicitly — both fuzz targets build `-Zsanitizer=address` and executed 628k inputs with 0 findings (2026-08-19) — but the TEST SUITE has not run under ASan/TSan/MSan | |
| H-25 | `cargo careful test` green | Incomplete | Never run; tool not installed | |

### Phase 6 — Fuzzing and properties

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-26 | ★ Fuzz target per public parser, decoder, or message handler | Completed | `fuzz/` with seeded corpora (2026-08-19): `alloc_ops` (malloc/zalloc/aligned/realloc/usable/free/collect op sequences with CANARY discipline — detects two-owners/overlap, not just crashes) and `xthread` (cross-thread frees + thread teardown/abandon/adopt). Both build under ASan+libFuzzer; smoke: 514k + 114k execs, 0 findings; per-PR smoke job in CI | |
| H-27 | ★ Continuous fuzzing with no open crashes | Incomplete | Targets exist and run (628k combined smoke execs under ASan, 0 crashes; per-PR CI smoke) — the ≥30-day coverage-guided soak has not elapsed yet; start the clock | |
| H-28 | Property tests cover the documented invariants | Incomplete | Hand-rolled property-style tests exist (e.g. `bins` size-class law derived from `os::page_size()`; G1 replay asserts alignment/zeroing/disjointness/canaries on 1M-op traces) but no proptest/quickcheck over arbitrary inputs | |
| H-29 | Mutation and/or differential testing on critical modules | Completed | This project IS differentially tested by design: G2 runs recorded traces through us and C mimalloc side-by-side (semantic equality, bin geometry pinned via `good_size`); 2026-08-19 executed the 3-arm real-world sweep (8 programs byte-identical vs mimalloc AND glibc, 144/144 runs) and the 19-config corpus sweep. No mutation score (open refinement, not a gap in the gate's either/or) | |

### Phase 7 — Formal verification

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-30 | Proof of panic-freedom / UB-freedom per `unsafe` module | Incomplete | No Kani/Creusot harnesses. Partial: the xthread protocol has an exhaustive loom model (bounded exhaustive ≠ proof); `page_of`'s bounds contract is discharged by argument + debug_assert, not by a checked proof | |

### Phase 8 — Build and binary

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-31 | ★ Binary hardening applied and verified | N/A | Tier `bin` — this unit ships as an rlib; the only cdylib (`rusty_alloc_override`) is `publish = false`, a dev-only measurement harness | |
| H-32 | Build is reproducible or fully auditable | N/A | Tier `bin` — no shipped binary artifact from this unit | |

### Phase 9 — Runtime privilege

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-33 | Least privilege documented and tested | N/A | Tier `bin` — a library; privilege is the host process's posture | |

### Phase 10 — Cryptography

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-34 | Vetted crypto only; no bespoke primitives | Incomplete | **`random.rs` is a bespoke, self-contained ChaCha8 implementation** (OS-seeded via BCryptGenRandom//dev/urandom). Used for hardening randomness (free-list encoding keys, guarded sampling), not confidentiality — but it is still a hand-rolled primitive. Options: vet against RustCrypto test vectors, or justify + waive under the zero-dependency constraint. R-002 | |
| H-35 | Side-channel discipline (constant-time, no secret branches) | N/A | No secret-equality comparison exists in the unit: free-list encoding is XOR/add encode-decode with an alignment check, not a MAC verify; no crypto comparison paths | |
| H-36 | Post-quantum migration plan for long-lived keys | N/A | No long-lived keys: per-page keys and CSPRNG state die with the process | |

### Phase 11 — CI/CD, release, and operations

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-37 | CI runs the hardening gate on every PR | Completed | Per-PR: fmt, clippy(-D warnings), check, test, deny, audit, Miri, fuzz-smoke, wasm-execute, oracle; actions SHA-pinned; tools version-pinned; `permissions: contents: read` (least-privilege token); weekly advisory cron | |
| H-38 | Releases signed, attested, and changelogged for security | Incomplete | `v0.7.0` is an unsigned annotated tag; no provenance/auditable artifacts; `docs/LEDGER.md` records security-relevant changes in prose but there is no CHANGELOG security section | |
| H-39 | ★ `SECURITY.md` with a coordinated disclosure process | Completed | `SECURITY.md` (2026-08-19): private GitHub advisories contact, 3-business-day acknowledgement, 90-day coordinated disclosure, supported-versions and scope sections | |
| H-40 | Advisory monitoring and scheduled re-audit | Completed | RustSec monitored by `cargo audit` per-PR AND a weekly CI cron (Mondays 06:00); re-audit scheduled quarterly (Next review 2026-11-19, in the header) | |
| H-41 | ★ Residual risks listed and accepted; waivers time-bounded | Incomplete | Register below is populated (R-001..R-004) but **acceptance is pending the owner** — an auditor cannot accept risk on the owner's behalf | |

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

In execution order. Cheapest-first is usually correct: configuration gates clear in
minutes and unblock the outcome gates behind them. **Owner/Target are the human step**
(SKILL.md §4.5) — proposed order only, nothing is Scheduled until both are filled.

| # | Gates | Work | Owner | Target | Notes |
|---|---|---|---|---|---|
| 1 | H-07 | Un-ignore + commit `Cargo.lock` | | | one line of `.gitignore`; precondition for H-08/H-09/H-10 |
| 2 | H-08, H-13, H-09 | `deny.toml` (advisories/licenses/bans/sources) + `cargo audit` in CI | | | 2 external deps — minutes |
| 3 | H-39, H-01, H-02 | `SECURITY.md` (contact + window + policy) + `docs/threat-model.md` from the sketch above | | | contact address is the owner's call |
| 4 | H-16 | `UNSAFE.md` inventory (module → purpose → last audit) | | | comments already exist; this is collation |
| 5 | H-05 | Measure `overflow-checks = true` on `bench/icount-arms.sh` + opscan; keep or waive with the number | | | a measured decision, not a flag flip |
| 6 | H-03, H-37 | Pin toolchain to `1.x.y`; pin CI actions by SHA; add deny/audit jobs | | | |
| 7 | H-26, H-27 | `cargo fuzz` targets over the existing `.ratrace` trace format (the v1 plan's G5: alloc/free/realloc/aligned, MT schedules, OOM injection) + a soak | | | the replayer is already the harness; ★-blocking pair |
| 8 | H-12 | `cargo auditable` / cyclonedx SBOM in the release flow | | | |
| 9 | H-15 | Decide the pedantic/nursery posture; fix or allow per-lint | | | |
| 10 | H-18 | `create_heap` OOM: return null through `malloc` instead of `expect` (mimalloc parity), or document the abort as policy | | | init.rs:499 |
| 11 | H-34 | Vet `random.rs` ChaCha8 against reference test vectors, or waive under the zero-dep constraint | | | R-002 |
| 12 | H-24, H-25, H-11, H-22, H-30 | Sanitizer matrix, cargo-careful, geiger baseline, Semgrep, first Kani harness | | | the long tail |

---

## Residual risk register

Every open risk carries an owner, an acceptance, and a review date (H-41).
**Acceptance pending** — listed by the auditor, not yet accepted by the owner.

| ID | Risk | Likelihood | Impact | Mitigation status | Accepted by | Review date |
|---|---|---|---|---|---|---|
| R-001 | `free()` on a foreign/forged pointer reads `slice_offset` from attacker-chosen memory → wild metadata write path (upstream mimalloc parity) | Low (requires a bug in the host) | High | `secure` feature (encrypted free lists, guard pages) mitigates; not default | pending | 2026-11-19 |
| R-002 | Bespoke ChaCha8 in `random.rs` (zero-dep constraint) | Low | Medium (hardening randomness only, no confidentiality) | OS-seeded; used only for free-list keys + sampling; not vetted against reference vectors | pending | 2026-11-19 |
| R-003 | Heap-creation OOM aborts (`init.rs:499`) instead of returning null — availability, not corruption | Low | Low-Med (DoS under memory exhaustion) | none yet; fix proposed (Scheduled #10) | pending | 2026-11-19 |
| R-004 | No fuzzing has ever run against the public surface | Medium | High | differential oracle + Miri + loom cover much of the space, but none are coverage-guided | pending | 2026-11-19 |

---

## Waivers

Time-bounded only. An expired waiver is an `Incomplete` gate, not a `Completed` one.

| Gate | Reason | Granted by | Expires |
|---|---|---|---|
| | | | |

---

## Audit log

Append one line per pass; never rewrite history. The trend is the point.

| Date | Depth | Auditor | Completed / Scheduled / Incomplete | ★ met | Note |
|---|---|---|---|---|---|
| 2026-08-19 | survey+cited tools | Claude Fable 5 | 3 / 0 / 32 (20 N/A) | 1/15 | first pass; ★ blockers: H-01 H-05 H-07 H-08 H-09 H-10 H-12 H-15 H-16 H-18 H-26 H-27 H-39 H-41 |
| 2026-08-19 | deep (tools executed) | Claude Fable 5 | 16 / 0 / 19 (20 N/A) | 9/15 | same-day execution pass: lockfile, deny+audit clean, SECURITY.md, threat model, UNSAFE.md, toolchain pin, CI hardening, OOM null-fix, fuzz targets live (628k execs). ★ blockers left: H-05(waiver) H-10 H-12 H-15 H-27(soak) H-41(owner) |
