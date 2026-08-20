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
and again on 2026-08-20 (clippy, Miri, the full test battery, callgrind
instruction counts, libFuzzer — cited per-row)
**Audited**: 2026-08-20 by Claude Opus 5 (adversarial pass; 2026-08-19 by Claude Fable 5 before it) · **Next review**: 2026-11-19

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
| H-05 | ★ Release profile hardened (overflow-checks, LTO, panic policy) | Completed | LTO/cgu/panic policy deliberate and set. `overflow-checks` in release is **WAIVED by the owner (Tim) 2026-08-20** on the measured cost (batch +7.1%, mixed +11% — release overflow checks would put batch back behind mimalloc), with the waiver time-bounded and recorded in the Waivers table below. The waiver is safe because untrusted-size arithmetic is explicitly `checked_*` (all 8 count×size sites, verified 2026-08-20) and every debug / test / Miri build runs WITH overflow checks on, so a wrapping bug surfaces in CI, not in production | |
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
| H-16 | ★ `unsafe` isolated, SAFETY-commented, inventoried | Completed | SAFETY comments lint-enforced (`undocumented_unsafe_blocks = deny`, clippy clean); `UNSAFE.md` inventories purpose + last-audit date per module, the four `unsafe impl` justifications, and the rules of engagement. **Extended 2026-08-20 to cover all SIX workspace crates**, not just the two published ones — the `publish = false` ffi (348 occurrences, and the untrusted C boundary), override (49), bench (29) and wasm (21) had never been inventoried by any plan file. The H-11 ratchet baselines all 21 files across all six | |
| H-17 | Arithmetic safety explicit | Completed | All 24 `as`-cast sites in the SHIPPED library code read individually (2026-08-19, driven by `cast_possible_truncation`/`cast_sign_loss`/`cast_possible_wrap`): every one narrows a value bounded by a compile-time constant (`HEADER_SLICES`, `SLICES_PER_SEGMENT` = 512, `MAX_ARENAS`) or an already sign-clamped option (`get(..).max(0)`). **None narrows an untrusted length**, which is the gate's actual criterion. Overflow-capable size arithmetic uses `checked_mul` (`calloc`/`mallocn`/`reallocn`/`recalloc`), and the `slice_offset` geometry bound is a const assert that fails the BUILD rather than truncating. The lint-family decision and its reasoning are recorded in `Cargo.toml` | |
| H-18 | ★ No `unwrap`/`expect`/panic on untrusted paths; typed errors | Completed | The `init.rs:499` OOM `expect` is FIXED (2026-08-19): heap-creation failure now propagates null through every allocation path (mimalloc parity); api constructors get a defined panic; the 2 remaining unwraps are the Miri-only mock. Cost priced at +1 Ir on the generic path only (batch 59.17, mixed IMPROVED to 139.07) | |
| H-19 | Input validation — external bytes treated as hostile | Completed | Sizes were already hostile-safe (overflow-checked multiplication, power-of-two alignment validation, huge sizes routed to dedicated segments). The POINTER half — R-001, this plan's top residual risk — is now guarded: `free()` consults the global segment map before deriving any metadata, so a pointer this allocator never returned is caught at the call instead of producing a wild `slot.sub(off)` read (the mechanism behind the documented mixed-allocator crashes). **Proven to fire**, not merely present: `tests/foreign_free.rs` hands `free` a static's address in a child process and requires the process to die on that specific assertion — a gate nobody has watched fail is not a gate. Debug/`debug_checks` builds only, and MEASURED at zero release cost (batch 60.17, small 79.38 — unchanged); it is a diagnostic, not a safety oracle, because a racing segment release can flip the answer **Extended to heap METADATA 2026-08-20.** A decoded free-list link is now required to be block-aligned AND inside the same segment as the block carrying it (`page::link_is_plausible`); it previously checked alignment only, while its own doc comment claimed "block-aligned inside a segment" — the segment half did not exist. That gap mattered because alignment alone filters ACCIDENTS (a stray ASCII overflow fails it 7 times in 8) and barely inconveniences a deliberate attacker, since every address worth steering an allocator at is already pointer-aligned. The bound is `(a ^ b) < SEGMENT_SIZE`: two ALU ops, no memory access — cheaper than asking the segment map (an atomic load) and strictly stronger, since the map accepts any segment we own and this accepts only the one the link must be in. Reporting also changed from `assert!` to a silent `#[cold]` abort: formatting a panic message can ALLOCATE, re-entering the allocator that just found its own metadata corrupted, and the unwind would cross `extern "C"` frames. Measured cost to the shipped default build: **zero** — all 13 opscan ops byte-identical (small 79.38, batch_lifo 60.17, mixed 140.07), the check being `secure`-only | |
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
| H-26 | ★ Fuzz target per public parser, decoder, or message handler | Completed | `fuzz/` with COVERAGE-GROWN corpora (2026-08-19): `alloc_ops` (malloc/zalloc/aligned/realloc/usable/free/collect op sequences with CANARY discipline — detects two-owners/overlap, not just crashes) and `xthread` (cross-thread frees + thread teardown/abandon/adopt, tags verified on the freeing thread). Soaked under ASan+libFuzzer: **5,047,688 + 1,940,317 = ~7.0M executions, ZERO crashes**; the resulting corpora were `cargo fuzz cmin`-minimized to 431 + 164 files (1.2 MB) preserving 515 + 446 coverage edges and COMMITTED, so CI starts from real coverage and cache eviction cannot take it away. Per-PR smoke job in CI **Third target added 2026-08-20: `corruption`**, which is pointed at the free-list link CHECK rather than at a workload. It exists because the true-positive path cannot be fuzzed in-process at all — the correct response to detected corruption is `abort`, which libFuzzer reports as a crash, so every input would be a finding; that path is covered by `tests/corruption.rs` instead. What it DOES fuzz is (a) the `(a ^ b) < SEGMENT_SIZE` same-segment identity, differentially against a naive `a / SEG == b / SEG` reference over arbitrary 128-bit input, and (b) FALSE POSITIVES — a bounds check on a hot path that ever rejects a GENUINE link is a denial of service we would ship, and under `secure` such a rejection IS an abort, which is what the fuzzer watches for. **456,561 executions, zero crashes.** Verified non-vacuous by widening the identity to `2 * SEGMENT_SIZE`, which it caught in seconds with an exact counterexample. Runs nightly with `--features secure` | |
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
| H-35 | Side-channel discipline (constant-time, no secret branches) | Completed | **Analysed 2026-08-20; full write-up in `docs/threat-model.md`.** This row previously read `N/A — no secret-equality comparison exists… not a MAC verify`, which answered only half the gate (it also asks for NO SECRET BRANCHES) and which stopped being true the same day, when the link check grew a branch on a key-derived value. Five channels examined. (1) `link_is_plausible` IS a secret-dependent branch — but its outcome is INVARIANT across every legitimate input, so it carries no timing signal on any non-corrupt workload and speaks only by aborting. (2) That abort is a genuine 1-bit oracle for an attacker with a write primitive, and it is self-destroying: each query costs the whole process, the heap CSPRNG reseeds from OS entropy on restart, and keys are per-page regardless, so bits never accumulate. (3) The REAL residual (**R-005**): `enc = (next + k1) ^ k0` does not survive a READ primitive — two known `(next, enc)` pairs from one page are solvable bit-by-bit, and *relative* forgery to a nearby target needs no key at all, since `(T+k1)^(n1+k1)` collapses to `T^n1` wherever the addition does not carry. That is exactly the case the same-segment bound cannot help with: intra-page type confusion. Inherited from mimalloc's scheme, not introduced here. (4) `below()`'s `%` has a secret dividend, but is reachable only with guarded sampling on and reveals only what the guard page already shows. (5) No key-derived value ever crosses the API boundary — encoding lives solely in freed memory. ChaCha8 itself is ARX: no S-boxes, no table lookups, no data-dependent branches, structurally constant-time. Noted for completeness: `k0` is forced odd, so 1 bit of 64 is known a priori | |
| H-36 | Post-quantum migration plan for long-lived keys | N/A | No long-lived keys: per-page keys and CSPRNG state die with the process | |

### Phase 11 — CI/CD, release, and operations

| ID | Gate | Status | Evidence | Target |
|---|---|---|---|---|
| H-37 | CI runs the hardening gate on every PR | Completed | Per-PR: fmt, clippy(-D warnings), check, test, deny, audit, Miri, fuzz-smoke, wasm-execute, oracle; actions SHA-pinned; tools version-pinned; `permissions: contents: read` (least-privilege token); weekly advisory cron | **Qualified 2026-08-20 — a real gap found the same day.** CI runs `cargo test --workspace --all-features` in DEBUG only and never in release, and two tests asserted on `stats.allocs`, which is `#[cfg(debug_assertions)]` because it sits on the hottest path. So `cargo test --release` failed 100% of the time on this crate, for an unknown period, with nobody watching — a permanently red suite being exactly how a real regression hides. THREE assertions across TWO crates are now gated to match the counters they read — `alloc_core.rs`, `heaps.rs`, and `rusty_alloc_bench/tests/selfhost.rs`, the last of which survived the first sweep precisely because it is in a DIFFERENT crate and every check that day ran `-p rusty_alloc`. Swept properly afterwards rather than fixing one more site and hoping: `allocs` and `frees` are the only debug-gated counters (`Heap::stat_alloc`/`stat_free`), so those three are the complete set; `large_allocs` and `realloc_in_place` are ungated, which is why `spans.rs` was never affected. **`cargo test --release --workspace` is now green.** Adding a release job to CI is the remaining half and is NOT yet done |
| H-38 | Releases signed, attested, and changelogged for security | Incomplete | Machine-side DONE (2026-08-19): `release.yml` now emits `SHA256SUMS.txt` alongside the SBOMs and calls `actions/attest-build-provenance`, so every `v*` tag produces a signed, GitHub-attested statement of what built the artifacts and from which commit. Security-relevant changes are recorded per-milestone in `docs/LEDGER.md`. **The remaining half needs a human key**: tags are unsigned (`v0.4.0`, `v0.7.0`), and only the owner can hold the signing key — configure `user.signingkey` + `tag.gpgSign`, or adopt sigstore. Left Incomplete rather than claimed | |
| H-39 | ★ `SECURITY.md` with a coordinated disclosure process | Completed | `SECURITY.md` (2026-08-19): private GitHub advisories contact, 3-business-day acknowledgement, 90-day coordinated disclosure, supported-versions and scope sections | |
| H-40 | Advisory monitoring and scheduled re-audit | Completed | RustSec monitored by `cargo audit` per-PR AND a weekly CI cron (Mondays 06:00); re-audit scheduled quarterly (Next review 2026-11-19, in the header) | |
| H-41 | ★ Residual risks listed and accepted; waivers time-bounded | Completed | **R-001..R-005 accepted by the owner (Tim) 2026-08-20** for the 1.0.0 release, each with a review date (2026-11-19); the two release waivers (H-05 overflow-checks, H-27 fuzz soak) are time-bounded in the Waivers table. The register and the waivers are the accepted, dated record the gate asks for | |

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
**Accepted by the owner (Tim) 2026-08-20** for the 1.0.0 release; each carries a review date.

| ID | Risk | Likelihood | Impact | Mitigation status | Accepted by | Review date |
|---|---|---|---|---|---|---|
| R-001 | `free()` on a foreign/forged pointer reads `slice_offset` from attacker-chosen memory → wild metadata write path (upstream mimalloc parity) | Low (requires a bug in the host) | High | `secure` feature (encrypted free lists, guard pages) mitigates; not default | Tim (owner) 2026-08-20 | 2026-11-19 |
| R-002 | Bespoke ChaCha8 in `random.rs` (zero-dep constraint) | Low | Medium (hardening randomness only, no confidentiality) | OS-seeded; used only for free-list keys + sampling; **vetted 2026-08-19** against RFC 8439 §2.1.1 quarter-round + state layout, with counter-carry and keystream properties (7 tests, H-34) — this cell previously read "not vetted" and contradicted H-34 | Tim (owner) 2026-08-20 | 2026-11-19 |
| R-003 | Heap-creation OOM aborts (`init.rs:499`) instead of returning null — availability, not corruption | Low | Low-Med (DoS under memory exhaustion) | none yet; fix proposed (Scheduled #10) | Tim (owner) 2026-08-20 | 2026-11-19 |
| R-004 | No fuzzing has ever run against the public surface | Medium | High | differential oracle + Miri + loom cover much of the space, but none are coverage-guided | Tim (owner) 2026-08-20 | 2026-11-19 |
| R-005 | Free-list encoding does not survive a READ primitive: an attacker with read AND write can recover the per-page keys from two known `(next, enc)` pairs, or forge a link to a NEARBY block with no key knowledge at all (carry-free bits of `(T+k1)^(n1+k1)` equal `T^n1`) | Low (needs a pre-existing memory-safety bug in the host, plus both primitives) | Medium (intra-page redirection → type confusion between live objects of one size class; the same-segment bound already excludes out-of-heap targets) | Inherent to the scheme and shared with upstream mimalloc; encoding is designed against BLIND overwrite, not read+write. **Mitigated as of 2026-08-20 by the `blockmap` feature, which is OFF by default on cost.** It detects what a forgery is FOR — the allocator handing out a block that is already live — rather than trying to prevent the forgery, so it covers key recovery, relative forgery and blind luck alike, and carries no key for a read primitive to recover. Verified to stop both corruption scenarios UNAIDED (no `secure`, no `linkcheck`), 637,602 fuzz executions with no false positives. Not default: +58 Ir/op and +2.2-5.0% whole-program, ~3x `secure`, which was itself declined at 1.7%. The residual therefore STANDS for anyone running the default or `secure` build — the mitigation exists and is switchable, it is not on. See the side-channel analysis in `docs/threat-model.md` | Tim (owner) 2026-08-20 | 2026-11-19 |

---

## Waivers

Time-bounded only. An expired waiver is an `Incomplete` gate, not a `Completed` one.

| Gate | Reason | Granted by | Expires |
|---|---|---|---|
| H-05 | Release `overflow-checks` left OFF: measured +7.1% batch / +11% mixed would put batch back behind mimalloc. Safe because untrusted-size arithmetic is `checked_*` and debug/test/Miri run with checks on. | Tim (owner) | 2026-11-19 (re-review with the quarterly audit) |
| H-27 | 1.0.0 ships before the 30-day continuous-fuzz soak completes. The nightly mechanism is live and the corpus is committed as a floor; the soak began 2026-08-19 and was clean at release. | Tim (owner) | 2026-09-19 (soak completion — H-27 flips to Completed if clean, else re-review) |

---

## Audit log

Append one line per pass; never rewrite history. The trend is the point.

| Date | Depth | Auditor | Completed / Scheduled / Incomplete | ★ met | Note |
|---|---|---|---|---|---|
| 2026-08-20 | 1.0.0 release | Claude Fable 5 | 34 / 1 / 1 (19 N/A) | 14/15 | **v1.0.0 cut.** Owner (Tim) accepted R-001..R-005 and granted the two time-bound release waivers (H-05 overflow-checks, H-27 fuzz soak), moving H-05 and H-41 to Completed. H-27 remains the one open ★ — a calendar gate; the soak completes 2026-09-19 and is waived until then. Perf refreshed: beats jemalloc (lua 0.85x, perl 0.90x, sqlite 0.98x), matches mimalloc. |
| 2026-08-19 | survey+cited tools | Claude Fable 5 | 3 / 0 / 32 (20 N/A) | 1/15 | first pass; ★ blockers: H-01 H-05 H-07 H-08 H-09 H-10 H-12 H-15 H-16 H-18 H-26 H-27 H-39 H-41 |
| 2026-08-20 | deep + adversarial | Claude Opus 5 | 32 / 1 / 3 (19 N/A) | 12/15 | adversarial pass: mitigations tested for EFFICACY not just function (`tests/corruption.rs` poisons a real free list, asserts SIGABRT not SIGSEGV); link check gained a same-segment bound; fuzz target 3; H-35 side-channel analysis done, N/A -> Completed, and it surfaced R-005; `blockmap` liveness map built and measured, ships OFF at +58 Ir/op; workspace Phase 3 audited (3,735 lines nobody had read). Found: 2 broken tests (release always red, CI debug-only) and 1 stale register cell (R-002 contradicted H-34). ★ blockers left: H-05(waiver) H-27(soak) H-41(owner) |
| 2026-08-19 | deep (tools executed) | Claude Fable 5 | 16 / 0 / 19 (20 N/A) | 9/15 | same-day execution pass: lockfile, deny+audit clean, SECURITY.md, threat model, UNSAFE.md, toolchain pin, CI hardening, OOM null-fix, fuzz targets live (628k execs). ★ blockers left: H-05(waiver) H-10 H-12 H-15 H-27(soak) H-41(owner) |
