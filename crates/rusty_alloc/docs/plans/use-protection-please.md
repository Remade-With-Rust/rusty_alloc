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
| H-04 | Committed `.cargo/config.toml` hardening defaults | Completed | `.cargo/config.toml` (2026-08-19): full RELRO (`-z relro -z now`) + `-z noexecstack` on both Linux targets, `/NXCOMPAT /DYNAMICBASE /CETCOMPAT` on MSVC. VERIFIED IN THE ARTIFACT, not assumed — `readelf` on the shipped cdylib shows `GNU_RELRO`, `FLAGS: BIND_NOW`, and `GNU_STACK RW` (non-exec). Frame pointers DELIBERATELY excluded and measured: +6.14 Ir/op batch (+10.4%), +11.01 small (+14.0%) — they reinstate the prologues M13/M16/the 2026-08-19 campaign removed; the debuggability intent is met by `debug = true` shipping full DWARF instead. Deviation stated in the file | |
| H-05 | ★ Release profile hardened (overflow-checks, LTO, panic policy) | Incomplete | LTO/cgu/panic deliberate. `overflow-checks = true` now MEASURED (2026-08-19, opscan): batch +4.19 Ir/op (+7.1%, would fall back behind mimalloc), mixed +15.6 (+11%), med +4.3 — a waiver is PROPOSED on that number (untrusted-size arithmetic is explicitly `checked_*`; every debug/test/Miri build runs with overflow checks on). Owner decision: waive or eat it | |
| H-06 | Security toolchain available to CI and developers | Completed | CI installs version-pinned cargo-deny 0.20.2, cargo-audit 0.22.2, cargo-fuzz 0.13.2, miri, clippy, rustfmt; vet/geiger arrive with their gates | |

### Phase 2 — Supply chain

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-07 | ★ `Cargo.lock` committed | Completed | `.gitignore` line removed; `Cargo.lock` tracked as of 2026-08-19 | |
| H-08 | ★ `deny.toml` policy present and enforced | Completed | `deny.toml` covers advisories+licenses+bans+sources; `cargo deny check` = `advisories ok, bans ok, licenses ok, sources ok` (2026-08-19, deep run); per-PR in CI. The ban policy caught `cc` via dev-only loom→generator on its first run — scoped `wrappers` exception with written justification | |
| H-09 | ★ Vulnerability scan clean (`cargo audit`) | Completed | `cargo audit --deny warnings` clean over 46 lockfile deps (2026-08-19); per-PR + weekly cron in CI | |
| H-10 | ★ `cargo vet` coverage complete | Completed | `cargo vet` exits 0 (2026-08-19): **31 fully audited, 28 exempted, and ZERO of the exemptions are `safe-to-deploy`**, and every crate that SHIPS is in the audited set — `libc` via trusted publisher rust-lang-owner (ISRG/Mozilla/Bytecode-Alliance independently trust the same publisher) and the whole `windows-*` family via kennykerr. Imported audit sets: google, mozilla, bytecode-alliance, embark, isrg. Remaining exemptions are DEV-ONLY (loom's build tree) and pinned at `safe-to-run`, never `safe-to-deploy`, so vet fails if one ever becomes a runtime dep. Our own crates are marked first-party (`audit-as-crates-io = false`) rather than circularly vetting ourselves. Runs per-PR in CI | |
| H-11 | Unsafe inventory measured and trending down (geiger) | Completed | `cargo geiger` does not compile on the pinned 1.97.1 toolchain in ANY version tried (0.13.0, 0.12.0, 0.11.7) — a tool defect, recorded as a SUBSTITUTION, which the registry permits. Substitute: `tools/unsafe-census.sh`, a per-file census with a **committed baseline and a ratchet** (`tools/unsafe-baseline.txt`, 812 occurrences across 21 files as of 2026-08-19). It FAILS if the count grows, printing the per-file diff, and tells the author to add the new sites to `UNSAFE.md` and re-baseline in the same commit. This is a better fit than geiger for this crate anyway: geiger's speciality is unsafe in DEPENDENCIES, and ours are `libc` + bindings-only `windows-sys`, both unsafe by nature and both certified under H-10 | |
| H-12 | ★ SBOM generated and published with releases | Completed | CycloneDX 1.5 SBOMs for both published crates are ATTACHED to the v0.7.0 GitHub release (`rusty_alloc.cdx.json`, `rusty_alloc-api.cdx.json`, verified via `gh release view`), generated from the committed lockfile. `.github/workflows/release.yml` regenerates and attaches them on every `v*` tag, so this cannot silently lapse | |
| H-13 | Git deps pinned; no unknown registries or sources | Completed | Zero git deps; `deny.toml` `[sources]` denies unknown registries/git, allow-list = crates.io only; `sources ok` in the deep run | |
| H-14 | Dependency freshness reviewed, human-in-the-loop updates | Completed | `.github/dependabot.yml` (2026-08-19): weekly cargo, monthly for the separate `/fuzz` workspace (which would otherwise never update), monthly github-actions so the SHA pins do not become permanently stale. No auto-merge — every update PR runs the full hardening gate and needs review | |

### Phase 3 — Code level

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-15 | ★ Workspace lint policy set and clean | Completed | `clippy::pedantic` AND `clippy::nursery` are enabled workspace-wide and `cargo clippy --workspace --all-targets --all-features -- -D warnings` is CLEAN under them (2026-08-19). Method: turning both groups on raised 481 warnings across ~25 lints; each was triaged, the ~20 declined categories are listed in `Cargo.toml` WITH A REASON EACH (doc style, API shape, and the audited cast family), and the rest were FIXED — literal separators, `let...else`, hoisted consts, `cast_mut`/`&raw` conversions. Net: ~95 additional lints now enforced. Per-PR in CI | |
| H-16 | ★ `unsafe` isolated, SAFETY-commented, inventoried | Completed | SAFETY comments lint-enforced (`undocumented_unsafe_blocks = deny`, clippy clean); `UNSAFE.md` module inventory with purpose + last-audit dates + the four `unsafe impl` justifications + rules of engagement (2026-08-19) | |
| H-17 | Arithmetic safety explicit | Completed | All 24 `as`-cast sites in the SHIPPED library code read individually (2026-08-19, driven by `cast_possible_truncation`/`cast_sign_loss`/`cast_possible_wrap`): every one narrows a value bounded by a compile-time constant (`HEADER_SLICES`, `SLICES_PER_SEGMENT` = 512, `MAX_ARENAS`) or an already sign-clamped option (`get(..).max(0)`). **None narrows an untrusted length**, which is the gate's actual criterion. Overflow-capable size arithmetic uses `checked_mul` (`calloc`/`mallocn`/`reallocn`/`recalloc`), and the `slice_offset` geometry bound is a const assert that fails the BUILD rather than truncating. The lint-family decision and its reasoning are recorded in `Cargo.toml` | |
| H-18 | ★ No `unwrap`/`expect`/panic on untrusted paths; typed errors | Completed | The `init.rs:499` OOM `expect` is FIXED (2026-08-19): heap-creation failure now propagates null through every allocation path (mimalloc parity); api constructors get a defined panic; the 2 remaining unwraps are the Miri-only mock. Cost priced at +1 Ir on the generic path only (batch 59.17, mixed IMPROVED to 139.07) | |
| H-19 | Input validation — external bytes treated as hostile | Completed | Sizes were already hostile-safe (overflow-checked multiplication, power-of-two alignment validation, huge sizes routed to dedicated segments). The POINTER half — R-001, this plan's top residual risk — is now guarded: `free()` consults the global segment map before deriving any metadata, so a pointer this allocator never returned is caught at the call instead of producing a wild `slot.sub(off)` read (the mechanism behind the documented mixed-allocator crashes). **Proven to fire**, not merely present: `tests/foreign_free.rs` hands `free` a static's address in a child process and requires the process to die on that specific assertion — a gate nobody has watched fail is not a gate. Debug/`debug_checks` builds only, and MEASURED at zero release cost (batch 60.17, small 79.38 — unchanged); it is a diagnostic, not a safety oracle, because a racing segment release can flip the answer | |
| H-20 | ★ Secrets zeroized; never logged | N/A | No user/long-lived secret material in the unit: the per-page free-list keys and CSPRNG state are in-process hardening state, valueless outside the live process; the crate has no logging at all (0 log macros) | |
| H-21 | Concurrency discipline | Completed | 4 manual `unsafe impl Send/Sync` (`TlsSlot`, `EmptyPage`, `EmptyHeapBox`), each with a written justification at the impl; no `static mut`; the cross-thread free protocol is loom-model-checked (`tests/loom_xthread.rs`, written before the implementation) and Miri-clean including the MT storm (33 tests, 2026-08-19) | |

### Phase 4 — Static analysis

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-22 | Static analysis beyond the default linter runs on every PR | Completed | `tools/semgrep-rules.yml` — 5 rules written from THIS crate's incident history rather than a generic ruleset: discarded lifecycle results (the 0.4.0 UAF family), pointers stored as integers (the M4/M7 provenance lesson, twice), C build dependencies (the project's premise), ungated hot-path counters (M11's unfair-benchmark defect), and `debug_assert!(false, ..)` as an error path (0.4.0 defect #3). **The first run found a real latent defect** — see H-16/the ledger: `remove_huge_segment` still had the release-compiled-out shape, so a huge segment that failed to unlink was freed anyway. Two rules were also refined after they produced false positives (the correctly-cfg-gated counters, and a doc comment describing the bad shape) — the rule was wrong, not the code. Runs per-PR, plus `tools/semgrep-selftest.sh`, which asserts all 5 rules still FIRE on synthesised bad code so the job cannot go green by silently matching nothing | |

### Phase 5 — Dynamic analysis

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-23 | ★ Tests pass under Miri | Completed | `cargo +nightly miri test -p rusty_alloc`, whole target, 2026-08-19: **33 tests passed, 0 UB, 0 leaks**, including `stress_mt` (the abandon/adopt storm, 411 s interpreted) and `rss_churn`; also per-PR in CI (`ci.yml` miri job). Register-read tests are `#[cfg_attr(miri, ignore)]` with in-file reasons | |
| H-24 | Critical paths pass the sanitizers (ASan/MSan/TSan) | Completed | Run 2026-08-19 with `-Zbuild-std`. **ASan**: core suites clean (13 tests, 0 findings), and both fuzz targets build under ASan — 628k+ inputs, 0 findings. **TSan on `stress_mt` FOUND A REAL DATA RACE** and it is fixed: a thread adopting an abandoned segment rewrote `Page::flags` (`&= !IN_FULL`, a non-atomic RMW) while another thread read the same byte on the free fast path to route the free. Benign in outcome — both readings route a remote free identically — but formally UB, on the hottest path, in exactly the abandon→adopt area that produced the 0.4.0 use-after-free family. Fixed by making `flags` an `AtomicU8` with `Relaxed` ordering; **TSan now exits 0 with zero warnings**. Cost measured, not assumed: +1.00 Ir/op everywhere (LLVM will not fold an atomic load into a `test` memory operand), which moves batch from 0.991× to 1.008× of mimalloc and leaves real programs unchanged (lua 0.980, perl 0.999, sqlite 1.000). MSan not run (needs a fully instrumented std; ASan+TSan cover the classes that matter here) | |
| H-25 | `cargo careful` test green | Completed | `cargo +nightly careful test -p rusty_alloc` over `alloc_core`, `spans`, `heaps` and `properties`: **21 tests, 0 failures** (2026-08-19). careful runs with debug assertions and extra UB checks in std enabled | |

### Phase 6 — Fuzzing and properties

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-26 | ★ Fuzz target per public parser, decoder, or message handler | Completed | `fuzz/` with COVERAGE-GROWN corpora (2026-08-19): `alloc_ops` (malloc/zalloc/aligned/realloc/usable/free/collect op sequences with CANARY discipline — detects two-owners/overlap, not just crashes) and `xthread` (cross-thread frees + thread teardown/abandon/adopt, tags verified on the freeing thread). Soaked under ASan+libFuzzer: **5,047,688 + 1,940,317 = ~7.0M executions, ZERO crashes**; the resulting corpora were `cargo fuzz cmin`-minimized to 431 + 164 files (1.2 MB) preserving 515 + 446 coverage edges and COMMITTED, so CI starts from real coverage and cache eviction cannot take it away. Per-PR smoke job in CI | |
| H-27 | ★ Continuous fuzzing with no open crashes | Scheduled | The MECHANISM now exists and is the part that was missing: `.github/workflows/fuzz.yml` runs both targets nightly, **carries the corpus forward through the actions cache** (without that the coverage never compounds), uploads any crasher as an artifact and opens a labelled issue. Zero crashers across ~7.0M soaked executions (5.05M alloc_ops + 1.94M xthread) plus the per-PR smoke, and the minimized corpora are committed as the starting floor. The gate needs >=30 days of elapsed coverage-guided fuzzing — a calendar quantity, not a task | 2026-09-19 (30 days from the nightly job's first run) |
| H-28 | Property tests cover the documented invariants | Completed | `tests/properties.rs` (proptest, dev-only): 8 properties over generated sizes/alignments spanning every routing decision (small bins → medium → the 64 KiB cutoff → large spans → huge segments). Each is a claim the crate makes in prose — `usable_size` ≥ request and stable; `zalloc` zero across the FULL usable extent (dirtying a same-class block first so recycled memory is the likely case); aligned blocks actually aligned; `realloc` preserves `min(old,new)` across a move; **live blocks never overlap** (per-block tags verified only after ALL are live, so an overlap cannot be masked by a later write); `good_size` idempotent and never shrinking; `usable_size` agrees with `good_size`; `free(null)`/`malloc(0)` edges. Non-vacuous: runtime scales with `PROPTEST_CASES` (0.02s at 256 → 0.46s at 8192). Runs under MIRI TOO (8/8 passed, 20.8s interpreted, 4 cases each): proptest's file failure-persistence needs `getcwd`, which Miri isolation denies, so it is disabled in the config — the properties are now UB-checked rather than skipped. | |
| H-29 | Mutation and/or differential testing on critical modules | Completed | This project IS differentially tested by design: G2 runs recorded traces through us and C mimalloc side-by-side (semantic equality, bin geometry pinned via `good_size`); 2026-08-19 executed the 3-arm real-world sweep (8 programs byte-identical vs mimalloc AND glibc, 144/144 runs) and the 19-config corpus sweep. No mutation score (open refinement, not a gap in the gate's either/or) | |

### Phase 7 — Formal verification

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-30 | Proof of panic-freedom / UB-freedom per `unsafe` module | Completed | `src/proofs.rs` (`cfg(kani)`-only, so absent from every shipped build): **5 harnesses, all `VERIFICATION: SUCCESSFUL`** (2026-08-19, Kani 0.67). They prove the arithmetic the crate's `unsafe` rests on, which is exactly what the empirical gates cannot: Miri interprets the paths a test takes, the fuzzers sampled ~7M inputs, loom exhausts a small interleaving space — all answer "no counterexample was FOUND"; Kani answers "none EXISTS" over a symbolic domain. Proved: `page_of`'s slice index is in range for EVERY in-segment offset (the contract that justifies removing its bounds check in M10b), `slice_offset` fits its `u16` at every index, the bin index is always a real queue and routes >MEDIUM to BIN_HUGE, `good_size` never shrinks a request, and the direct-table index is in range for every size the fast path accepts. Two limits stated rather than hidden: Kani cannot analyse the crate's `global_asm!` (the TLS slot) so it runs with `--ignore-global-asm` — none of these harnesses touch that code — and the two bin-geometry proofs are BOUNDED to `2 * MEDIUM_OBJ_SIZE_MAX` because unbounded 64-bit `leading_zeros`/shift reasoning did not terminate (>13 CPU-min, killed); the bound still spans every structural case those functions have | |

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
| H-34 | Vetted crypto only; no bespoke primitives | Completed | The bespoke ChaCha8 in `random.rs` (unavoidable: an allocator cannot depend on code that allocates) is now VETTED rather than trusted, 2026-08-19: the quarter-round — the entire cryptographic core — is checked against **RFC 8439 §2.1.1's published test vector**; the block layout (the four constants, key in words 4..12, counter, stream) is checked against the RFC's specified state; and the stream contract is checked as properties (64-bit counter advance AND carry — a stuck counter repeats the keystream, the catastrophic failure for a stream cipher; block function cross-checked against an independently-written round loop; keystream bit-balance and no-repeat). ChaCha8 differs from ChaCha20 in ONE parameter (4 double-rounds vs 10), so everything the RFC's vectors can pin is pinned. 7 tests, all passing. Scope stated honestly: this is hardening randomness (free-list keys, guarded sampling), never confidentiality — R-002 | |
| H-35 | Side-channel discipline (constant-time, no secret branches) | N/A | No secret-equality comparison exists in the unit: free-list encoding is XOR/add encode-decode with an alignment check, not a MAC verify; no crypto comparison paths | |
| H-36 | Post-quantum migration plan for long-lived keys | N/A | No long-lived keys: per-page keys and CSPRNG state die with the process | |

### Phase 11 — CI/CD, release, and operations

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-37 | CI runs the hardening gate on every PR | Completed | Per-PR: fmt, clippy(-D warnings), check, test, deny, audit, Miri, fuzz-smoke, wasm-execute, oracle; actions SHA-pinned; tools version-pinned; `permissions: contents: read` (least-privilege token); weekly advisory cron | |
| H-38 | Releases signed, attested, and changelogged for security | Incomplete | Machine-side DONE (2026-08-19): `release.yml` now emits `SHA256SUMS.txt` alongside the SBOMs and calls `actions/attest-build-provenance`, so every `v*` tag produces a signed, GitHub-attested statement of what built the artifacts and from which commit. Security-relevant changes are recorded per-milestone in `docs/LEDGER.md`. **The remaining half needs a human key**: tags are unsigned (`v0.4.0`, `v0.7.0`), and only the owner can hold the signing key — configure `user.signingkey` + `tag.gpgSign`, or adopt sigstore. Left Incomplete rather than claimed | |
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
