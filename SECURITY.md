# Security Policy

rusty_alloc is a memory allocator: memory-safety defects in it are, by
definition, security defects in every program that links it. Reports are
taken seriously and handled fast — the project's record is a same-day fix and
a yank recommendation for the one shipped regression reported to date.

## Reporting a vulnerability

**Please do not open a public issue for a suspected vulnerability.**

Report privately via **GitHub Security Advisories**:
<https://github.com/remade-with-rust/rusty_alloc/security/advisories/new>

What to include, if you can: the version, the target
(OS/architecture/features), a minimal reproducer or crash artifact, and
whether you believe the defect is reachable from safe Rust or from the
`mi_*`/`malloc` C surface.

## What to expect

- **Acknowledgement within 3 business days.**
- A triage verdict (accepted / needs-more-info / declined) within 7 days.
- Coordinated disclosure: we ask for up to **90 days** to ship a fix before
  public disclosure; memory-safety fixes have historically shipped much
  faster. Affected releases are yanked from crates.io where warranted, and
  the fix is credited to the reporter unless anonymity is requested.

## Supported versions

Only the **latest published 0.x release** receives fixes. Versions **0.3.2
and earlier are unsound on every target** (three platform-independent
use-after-frees, fixed in 0.4.0) — upgrade rather than report against them.

## Scope

In scope: the published crates `rusty_alloc` and `rusty_alloc-api` — heap
corruption, use-after-free, double-free acceptance, data races, information
disclosure through allocator state, and denial-of-service beyond what a
malloc contract permits.

Out of scope: the development-only vendored trees (`oracle/mimalloc`,
`corpus/mimalloc-bench` — report those upstream), the `publish = false`
harness crates, and crashes that require an already-memory-unsafe host
program (e.g. freeing a pointer the allocator never returned — see the threat
model for where that boundary sits and what the `secure` feature adds).

## Hardening posture

The current audited hardening status is at the bottom of the README (the
"Hardening status" block), with the full gate-by-gate checklist in
[docs/plans/use-protection-please.md](docs/plans/use-protection-please.md)
and the threat model in [docs/threat-model.md](docs/threat-model.md).
