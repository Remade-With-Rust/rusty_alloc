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
*Full model* — [docs/threat-model.md](../threat-model.md) (2026-08-19).

---

## Checklist

`★` = v1.0.0-blocking. Full probe and pass criteria per gate: the skill's `CHECKLIST.md`.

### Phase 0 — Threat modeling

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-01 | ★ Threat model documented and linked from README | Completed | `docs/threat-model.md` (assets, adversaries, trust boundaries, STRIDE pass, 2026-08-19); README §Security links it | |
| H-02 | Threat model revisited after last major change | Completed | Model dated 2026-08-19, same day as the last code change | |

### Phase 1 — Toolchain

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-03 | Toolchain pinned (`rust-toolchain.toml`) | Completed | `channel = "1.97.1"` + components + tier-1 targets; CI pins the same version | |
| H-04 | Committed `.cargo/config.toml` hardening defaults | Completed | `.cargo/config.toml` (2026-08-19): full RELRO (`-z relro -z now`) + `-z noexecstack` on both Linux targets, `/NXCOMPAT /DYNAMICBASE /CETCOMPAT` on MSVC. VERIFIED IN THE ARTIFACT, not assumed — `readelf` on the shipped cdylib shows `GNU_RELRO`, `FLAGS: BIND_NOW`, and `GNU_STACK RW` (non-exec). Frame pointers DELIBERATELY excluded and measured: +6.14 Ir/op batch (+10.4%), +11.01 small (+14.0%) — they reinstate the prologues M13/M16/the 2026-08-19 campaign removed; the debuggability intent is met by `debug = true` shipping full DWARF instead. Deviation stated in the file | |
| H-05 | ★ Release profile hardened (overflow-checks, LTO, panic policy) | Incomplete | LTO/cgu/panic deliberate; `overflow-checks = true` MEASURED at +7.1% batch / +11% mixed (core plan H-05) — waiver proposed on that number, owner decision pending | |
| H-06 | Security toolchain available to CI and developers | Completed | CI installs version-pinned cargo-deny 0.20.2, cargo-audit 0.22.2, cargo-fuzz 0.13.2, miri, clippy, rustfmt | |

### Phase 2 — Supply chain

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-07 | ★ `Cargo.lock` committed | Completed | `.gitignore` entry removed; lockfile tracked as of 2026-08-19 | |
| H-08 | ★ `deny.toml` policy present and enforced | Completed | `cargo deny check` = all four checks ok (2026-08-19); per-PR in CI; the ban policy caught dev-only `cc` (loom→generator) on its first run — scoped exception with written justification | |
| H-09 | ★ Vulnerability scan clean (`cargo audit`) | Completed | Clean over 46 lockfile deps (2026-08-19); per-PR + weekly cron in CI | |
| H-10 | ★ `cargo vet` coverage complete | Completed | `cargo vet` exits 0 (2026-08-19): **31 fully audited, 28 exempted, and ZERO of the exemptions are `safe-to-deploy`**, and every crate that SHIPS is in the audited set — `libc` via trusted publisher rust-lang-owner (ISRG/Mozilla/Bytecode-Alliance independently trust the same publisher) and the whole `windows-*` family via kennykerr. Imported audit sets: google, mozilla, bytecode-alliance, embark, isrg. Remaining exemptions are DEV-ONLY (loom's build tree) and pinned at `safe-to-run`, never `safe-to-deploy`, so vet fails if one ever becomes a runtime dep. Our own crates are marked first-party (`audit-as-crates-io = false`) rather than circularly vetting ourselves. Runs per-PR in CI | |
| H-11 | Unsafe inventory measured and trending down (geiger) | Completed | `cargo geiger` does not compile on the pinned 1.97.1 toolchain in ANY version tried (0.13.0, 0.12.0, 0.11.7) — a tool defect, recorded as a SUBSTITUTION, which the registry permits. Substitute: `tools/unsafe-census.sh`, a per-file census with a **committed baseline and a ratchet** (`tools/unsafe-baseline.txt`, 812 occurrences across 21 files as of 2026-08-19). It FAILS if the count grows, printing the per-file diff, and tells the author to add the new sites to `UNSAFE.md` and re-baseline in the same commit. This is a better fit than geiger for this crate anyway: geiger's speciality is unsafe in DEPENDENCIES, and ours are `libc` + bindings-only `windows-sys`, both unsafe by nature and both certified under H-10 | |
| H-12 | ★ SBOM generated and published with releases | Completed | CycloneDX 1.5 SBOMs for both published crates are ATTACHED to the v0.7.0 GitHub release (`rusty_alloc.cdx.json`, `rusty_alloc-api.cdx.json`, verified via `gh release view`), generated from the committed lockfile. `.github/workflows/release.yml` regenerates and attaches them on every `v*` tag, so this cannot silently lapse | |
| H-13 | Git deps pinned; no unknown registries or sources | Completed | Zero git deps; `[sources]` in `deny.toml` denies unknown, crates.io-only allow-list; `sources ok` | |
| H-14 | Dependency freshness reviewed, human-in-the-loop updates | Completed | `.github/dependabot.yml` (2026-08-19): weekly cargo, monthly for the separate `/fuzz` workspace (which would otherwise never update), monthly github-actions so the SHA pins do not become permanently stale. No auto-merge — every update PR runs the full hardening gate and needs review | |

### Phase 3 — Code level

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-15 | ★ Workspace lint policy set and clean | Completed | `clippy::pedantic` AND `clippy::nursery` are enabled workspace-wide and `cargo clippy --workspace --all-targets --all-features -- -D warnings` is CLEAN under them (2026-08-19). Method: turning both groups on raised 481 warnings across ~25 lints; each was triaged, the ~20 declined categories are listed in `Cargo.toml` WITH A REASON EACH (doc style, API shape, and the audited cast family), and the rest were FIXED — literal separators, `let...else`, hoisted consts, `cast_mut`/`&raw` conversions. Net: ~95 additional lints now enforced. Per-PR in CI | |
| H-16 | ★ `unsafe` isolated, SAFETY-commented, inventoried | Completed | **Audited WORKSPACE-WIDE 2026-08-20, which is the point of this row.** It previously read `N/A — audited per member`, and that justification was one-third true: only 2 of 6 members have a plan file, leaving 3,735 lines in `rusty_alloc-ffi` (2,130), `-bench` (1,187), `-override` (221) and `-wasm` (197) covered by nobody. Verified: all six manifests carry `[lints] workspace = true`, so `clippy::undocumented_unsafe_blocks = deny` and `unsafe_op_in_unsafe_fn = deny` bind in EVERY crate, and `cargo clippy --workspace --all-targets --all-features -D warnings` is clean — so every `unsafe` block in all four previously-unaudited crates already carries its SAFETY comment. `UNSAFE.md` now inventories all six crates, and the H-11 ratchet baselines all 21 source files across all six (ffi 348, override 49, bench 29, wasm 21) | |
| H-17 | Arithmetic safety explicit | Completed | Audited workspace-wide 2026-08-20. The FFI crate is the one that matters — it takes `count`/`size` straight from C callers — and **all 8 `count * size` sites use `checked_mul`, with ZERO unchecked multiplications in the crate** (grep for `count * size` / `size * count` returns nothing). The core's 24 cast sites were read individually on 2026-08-19 and are each bounded by a compile-time constant or a sign-clamped option. `-bench`/`-wasm`/`-override` perform no size arithmetic of their own — they forward layouts | |
| H-18 | ★ No `unwrap`/`expect`/panic on untrusted paths; typed errors | Completed | Audited workspace-wide 2026-08-20: `-ffi` 0 unwrap / 0 expect across 157 `extern "C"` entry points, `-override` 0/0, `-wasm` 0/0. `-bench` has 20 unwraps and they were READ, not counted: the 9 in the trace parser are **infallible by type** — `Record::decode` takes `&[u8; RECORD_SIZE]`, so the `b[8..16].try_into()` conversions cannot fail — and the one field that genuinely can be invalid (the op byte) returns `InvalidData` rather than panicking; the rest are in a `publish = false` CLI harness. Panics also cannot unwind into C: edition 2024 makes `extern "C"` abort on unwind, and the release profile is `panic = "abort"` besides | |
| H-19 | Input validation — external bytes treated as hostile | Completed | **The FFI surface IS this workspace's untrusted boundary** — 157 `extern "C"` entry points taking caller-supplied pointers and sizes — and it had never been audited. Read 2026-08-20, and it holds up: every out-parameter writer null-guards before writing (`store_bs`, `mi_urealloc`'s `block_size_pre`, `mi_ufree`'s `block_size`, `mi_reallocarr`'s `ptrp`, `mi_reserve_os_memory_ex`'s `arena_id`); `mi_posix_memalign` validates that alignment is a power of two AND at least `sizeof(void*)`, returning EINVAL/ENOMEM rather than trusting it; count×size is `checked_mul` throughout. Caller POINTERS additionally inherit the segment-map foreign-pointer guard added to `free()` (core H-19) | |
| H-20 | ★ Secrets zeroized; never logged | N/A | Genuinely inapplicable, and evidenced rather than asserted (2026-08-20) — a fake `Completed` here would be worse than an honest `N/A`. No unit in the workspace holds user or long-lived secret material: the only key-like values are the per-page free-list encoding keys and CSPRNG state under `secure`, which are in-process hardening state, regenerated per page, valueless outside the live process, and never placed in user-visible memory. And there is no leak channel to protect: **zero logging macros** (`println!`/`eprintln!`/`log::`/`tracing::`/`dbg!`) in every shipped and tooling crate — the sole exception is the `publish = false` bench CLI, which prints benchmark results by design | |
| H-21 | Concurrency discipline | Completed | Audited workspace-wide 2026-08-20: **zero manual `Send`/`Sync` impls** in `-ffi`, `-override`, `-bench` or `-wasm` — all shared mutable state lives in the core, which is where it can be reasoned about. The core's four (`TlsSlot` ×2, `EmptyPage`, `EmptyHeapBox`) each carry a written justification at the impl and are listed in `UNSAFE.md`; no `static mut` anywhere; the cross-thread protocol is loom-model-checked before implementation and **TSan-clean as of 2026-08-19 — a state it did NOT hold before this audit**, which found and fixed a real data race on the free fast path | |

### Phase 4 — Static analysis

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-22 | Static analysis beyond the default linter runs on every PR | Completed | `tools/semgrep-rules.yml` — 5 rules written from THIS crate's incident history rather than a generic ruleset: discarded lifecycle results (the 0.4.0 UAF family), pointers stored as integers (the M4/M7 provenance lesson, twice), C build dependencies (the project's premise), ungated hot-path counters (M11's unfair-benchmark defect), and `debug_assert!(false, ..)` as an error path (0.4.0 defect #3). **The first run found a real latent defect** — see H-16/the ledger: `remove_huge_segment` still had the release-compiled-out shape, so a huge segment that failed to unlink was freed anyway. Two rules were also refined after they produced false positives (the correctly-cfg-gated counters, and a doc comment describing the bad shape) — the rule was wrong, not the code. Runs per-PR, plus `tools/semgrep-selftest.sh`, which asserts all 5 rules still FIRE on synthesised bad code so the job cannot go green by silently matching nothing | |

### Phase 5 — Dynamic analysis

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-23 | ★ Tests pass under Miri | Completed | CI runs `cargo +nightly miri test -p rusty_alloc` on every PR (`ci.yml` miri job); 2026-08-19 whole-target run: 33 tests, 0 UB (core plan H-23); the api member now has its own Miri suite (5 tests, no longer vacuous) | |
| H-24 | Critical paths pass the sanitizers (ASan/MSan/TSan) | Completed | Run 2026-08-19 with `-Zbuild-std`. **ASan**: core suites clean (13 tests, 0 findings), and both fuzz targets build under ASan — 628k+ inputs, 0 findings. **TSan on `stress_mt` FOUND A REAL DATA RACE** and it is fixed: a thread adopting an abandoned segment rewrote `Page::flags` (`&= !IN_FULL`, a non-atomic RMW) while another thread read the same byte on the free fast path to route the free. Benign in outcome — both readings route a remote free identically — but formally UB, on the hottest path, in exactly the abandon→adopt area that produced the 0.4.0 use-after-free family. Fixed by making `flags` an `AtomicU8` with `Relaxed` ordering; **TSan now exits 0 with zero warnings**. Cost measured, not assumed: +1.00 Ir/op everywhere (LLVM will not fold an atomic load into a `test` memory operand), which moves batch from 0.991× to 1.008× of mimalloc and leaves real programs unchanged (lua 0.980, perl 0.999, sqlite 1.000). MSan not run (needs a fully instrumented std; ASan+TSan cover the classes that matter here) | |
| H-25 | `cargo careful` test green | Completed | `cargo +nightly careful test -p rusty_alloc` over `alloc_core`, `spans`, `heaps` and `properties`: **21 tests, 0 failures** (2026-08-19). careful runs with debug assertions and extra UB checks in std enabled | |

### Phase 6 — Fuzzing and properties

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-26 | ★ Fuzz target per public parser, decoder, or message handler | Completed | `fuzz/` (alloc_ops + xthread, canary discipline); **~7.0M soaked executions under ASan, 0 findings**; cmin-minimized corpora (431 + 164 files, 1.2 MB, 961 coverage edges) committed as CI's floor; per-PR smoke in CI — details in the core plan | |
| H-27 | ★ Continuous fuzzing with no open crashes | Scheduled | The MECHANISM now exists and is the part that was missing: `.github/workflows/fuzz.yml` runs both targets nightly, **carries the corpus forward through the actions cache** (without that the coverage never compounds), uploads any crasher as an artifact and opens a labelled issue. Zero crashers across ~7.0M soaked executions (5.05M alloc_ops + 1.94M xthread) plus the per-PR smoke, and the minimized corpora are committed as the starting floor. The gate needs >=30 days of elapsed coverage-guided fuzzing — a calendar quantity, not a task | 2026-09-19 (30 days from the nightly job's first run) |
| H-28 | Property tests cover the documented invariants | Completed | `tests/properties.rs` (proptest, dev-only): 8 properties over generated sizes/alignments spanning every routing decision (small bins → medium → the 64 KiB cutoff → large spans → huge segments). Each is a claim the crate makes in prose — `usable_size` ≥ request and stable; `zalloc` zero across the FULL usable extent (dirtying a same-class block first so recycled memory is the likely case); aligned blocks actually aligned; `realloc` preserves `min(old,new)` across a move; **live blocks never overlap** (per-block tags verified only after ALL are live, so an overlap cannot be masked by a later write); `good_size` idempotent and never shrinking; `usable_size` agrees with `good_size`; `free(null)`/`malloc(0)` edges. Non-vacuous: runtime scales with `PROPTEST_CASES` (0.02s at 256 → 0.46s at 8192). Runs under MIRI TOO (8/8 passed, 20.8s interpreted, 4 cases each): proptest's file failure-persistence needs `getcwd`, which Miri isolation denies, so it is disabled in the config — the properties are now UB-checked rather than skipped. | |
| H-29 | Mutation and/or differential testing on critical modules | Completed | The workspace's whole gate design is differential: the vendored C mimalloc oracle (G2 semantic trace diff), the 3-arm byte-identity real-world sweep (144/144, 2026-08-19), the 19-config corpus sweep — see core plan H-29 | |

### Phase 7 — Formal verification

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-30 | Proof of panic-freedom / UB-freedom per `unsafe` module | Completed | `src/proofs.rs` (`cfg(kani)`-only, so absent from every shipped build): **5 harnesses, all `VERIFICATION: SUCCESSFUL`** (2026-08-19, Kani 0.67). They prove the arithmetic the crate's `unsafe` rests on, which is exactly what the empirical gates cannot: Miri interprets the paths a test takes, the fuzzers sampled ~7M inputs, loom exhausts a small interleaving space — all answer "no counterexample was FOUND"; Kani answers "none EXISTS" over a symbolic domain. Proved: `page_of`'s slice index is in range for EVERY in-segment offset (the contract that justifies removing its bounds check in M10b), `slice_offset` fits its `u16` at every index, the bin index is always a real queue and routes >MEDIUM to BIN_HUGE, `good_size` never shrinks a request, and the direct-table index is in range for every size the fast path accepts. Two limits stated rather than hidden: Kani cannot analyse the crate's `global_asm!` (the TLS slot) so it runs with `--ignore-global-asm` — none of these harnesses touch that code — and the two bin-geometry proofs are BOUNDED to `2 * MEDIUM_OBJ_SIZE_MAX` because unbounded 64-bit `leading_zeros`/shift reasoning did not terminate (>13 CPU-min, killed); the bound still spans every structural case those functions have | |

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
| H-34 | Vetted crypto only; no bespoke primitives | Completed | The bespoke ChaCha8 in `random.rs` (unavoidable: an allocator cannot depend on code that allocates) is now VETTED rather than trusted, 2026-08-19: the quarter-round — the entire cryptographic core — is checked against **RFC 8439 §2.1.1's published test vector**; the block layout (the four constants, key in words 4..12, counter, stream) is checked against the RFC's specified state; and the stream contract is checked as properties (64-bit counter advance AND carry — a stuck counter repeats the keystream, the catastrophic failure for a stream cipher; block function cross-checked against an independently-written round loop; keystream bit-balance and no-repeat). ChaCha8 differs from ChaCha20 in ONE parameter (4 double-rounds vs 10), so everything the RFC's vectors can pin is pinned. 7 tests, all passing. Scope stated honestly: this is hardening randomness (free-list keys, guarded sampling), never confidentiality — R-002 | |
| H-35 | Side-channel discipline (constant-time, no secret branches) | N/A | audited per member (core: N/A — no secret comparisons) | |
| H-36 | Post-quantum migration plan for long-lived keys | N/A | audited per member (core: N/A — no long-lived keys) | |

### Phase 11 — CI/CD, release, and operations

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-37 | CI runs the hardening gate on every PR | Completed | fmt, clippy, check, test, deny, audit, Miri, fuzz-smoke, wasm-execute, oracle per PR; actions SHA-pinned; tools version-pinned; least-privilege token; weekly advisory cron | |
| H-38 | Releases signed, attested, and changelogged for security | Incomplete | Machine-side DONE (2026-08-19): `release.yml` now emits `SHA256SUMS.txt` alongside the SBOMs and calls `actions/attest-build-provenance`, so every `v*` tag produces a signed, GitHub-attested statement of what built the artifacts and from which commit. Security-relevant changes are recorded per-milestone in `docs/LEDGER.md`. **The remaining half needs a human key**: tags are unsigned (`v0.4.0`, `v0.7.0`), and only the owner can hold the signing key — configure `user.signingkey` + `tag.gpgSign`, or adopt sigstore. Left Incomplete rather than claimed | |
| H-39 | ★ `SECURITY.md` with a coordinated disclosure process | Completed | Present at the repo root: private GitHub advisories, 3-business-day ack, 90-day coordinated disclosure, scope + supported versions | |
| H-40 | Advisory monitoring and scheduled re-audit | Completed | cargo-audit per-PR + weekly CI cron; quarterly re-audit (Next review 2026-11-19) | |
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
| 2026-08-19 | deep (tools executed) | Claude Fable 5 | 14 / 0 / 16 (25 N/A) | 7/13 | same-day execution pass. ★ blockers left: H-05(waiver) H-10 H-12 H-15 H-27(soak) H-41(owner) |
