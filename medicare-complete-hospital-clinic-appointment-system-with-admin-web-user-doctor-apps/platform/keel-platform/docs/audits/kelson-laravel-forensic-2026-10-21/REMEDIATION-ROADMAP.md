# REMEDIATION ROADMAP — dependency-ordered

**Audit:** Kelson × Laravel forensic competitor audit, 2026-10-21
**Baseline:** commit `722a893`, branch `p0-runtime-safety`
**Scope:** the 23 findings in `ACTIONABLE-FINDINGS.csv`
**Rule:** a stage may not start until the previous stage's exit criteria hold. Where a finding is
blocked by infrastructure, the block is recorded with the minimum requirement, not worked around.

---

## Stage 0 — P0 security and proof infrastructure

**Why first:** two S0 findings and one S1 crash are in the release path, and the flagship product
promise has no evidence at all. Nothing else can be trusted until the host runs and the sandbox is
patched.

| # | Finding | Action | Depends on |
|---|---|---|---|
| KFA-001 | S0 | Upgrade `wasmtime` past RUSTSEC-2026-0379/0380 (Cranelift miscompilation) and re-run the ABI/drift suites | — |
| KFA-002 | S0 | Same bump closes the handle-table cap (RUSTSEC-2026-0363); then re-verify the module fuel/memory budget | KFA-001 |
| KFA-005 | S1 | Fix the reference host's stack overflow: run the boot on a thread with an explicit stack size (or split the boot future), then add a **boot smoke gate** (start the real binary, probe `/healthz`, assert the process serves) | — |
| KFA-009 | S1 | Run NR-01…NR-08 with the process identity, the executable digest, the generation ids and the before/after behaviour captured; publish the raw artefacts | KFA-005 |
| KFA-003 | S1 | Upgrade `rustls` past the four advisories (the pre-auth 1 GiB Brotli allocation is a remote DoS) | — |
| KFA-004 | S1 | Upgrade `webpki-roots` (≥1.0.10) and `rustls-webpki` past the distrusted-intermediate advisories | KFA-003 |
| KFA-006 | S2 | Default `database.required` from the environment: refuse the degraded in-memory boot outside dev/test | — |
| KFA-022 | S1 | Provision PostgreSQL for the audit host; re-run the pg matrix so the P0 "real PostgreSQL integration" claim has evidence | — |

**Exit criteria**

- `cargo audit` reports **0** critical/high advisories for the runtime's dependency set.
- The reference host starts and answers on the CI runner **and** on a Windows host.
- NR-01…NR-08 each have a signed experiment record (PID, digest, generation, before/after, latency).
- The PostgreSQL matrix has run with the artefacts kept; the pg halves are not self-skips.
- A production config without `database.required` refuses to boot.

**Regression strategy:** the boot smoke gate and the audit gate are permanent CI steps; the
dependency bumps are covered by the existing ABI/drift/client suites; the no-rebuild records are
re-run per release and diffed.

---

## Stage 1 — P1 runtime independence, dispatch and persistence

**Why second:** the runtime must exist and be provable before its semantics are extended.

| # | Finding | Action | Depends on |
|---|---|---|---|
| KFA-013 | S2 | Re-run the outbox's rollback/crash cases on PostgreSQL and publish the artefact | KFA-022 |
| KFA-014 | S2 | One conformance suite per remaining port (queue, events, mail, notifications, storage), each run by the fake **and** the real adapter | Stage 0 |
| KFA-015 | S2 | Stop the local disk's file IO from running on the async executor: `spawn_blocking` in the adapter now, the port's async-ness when the real object-store driver lands | — |
| KFA-008 | S2 | Land the panic-safety gate: a per-crate `clippy::unwrap_used` deny, driven newest-crate-first, with a count ratchet so the number can only fall | — |

**Exit criteria**

- Every fake has a conformance target the CI runs; no port is faked without a contract.
- A load case shows N concurrent uploads do not move the p99 of an unrelated endpoint.
- The per-crate unwrap/expect counts never rise; the deny is enabled crate by crate.

**Regression strategy:** the conformance suites become the contract gate; the ratchet file is the
budget; the load case is a repeatable script with its workload recorded.

---

## Stage 2 — P2 the Laravel baseline and the core DX

**Why third:** these are the parity gaps a Laravel developer notices first; they need a stable
runtime and trustworthy fakes underneath them.

| # | Finding | Action | Depends on |
|---|---|---|---|
| — | LF-089 | The **RLS** assertion against a real PostgreSQL (the tenant boundary proven by the database, not only by the store) | KFA-022 |
| — | LF-087 | The random-order and repeated-run cases (the suite's order must not change its result) | — |
| — | LF-093 | The teardown-after-failure and the resource-cap cases | — |
| — | LF-090 | The CLI's interrupt and invalid-argument cases | — |
| — | LF-106 | The auth **endpoint matrix** (one case enumerating the auth surface and its expected status) | — |
| — | LF-101 | The query-bindings privacy case (a captured query must not carry patient data) | — |
| — | LF-102 | The flag **stale-cache** and fail-open/fail-closed cases | — |
| KFA-018 | S2 | Measure the coverage, publish it, add a non-decreasing ratchet | — |
| KFA-019 | S3 | Normalise the tree (1,606 files) in one mechanical commit, then enable `cargo fmt --check` | — |
| KFA-020 | S3 | Add the doc-link checker; fix the dead `docs/19 L###` pointers | — |
| KFA-021 | S3 | Generate the competitive comparison from the parity matrix; track it | — |

**Exit criteria**

- The LF-089 RLS case passes against PostgreSQL.
- Every P1/P0 atlas row is VERIFIED or carries a written, signed exclusion.
- Coverage is published and the CI fails on a drop; the tree is fmt-clean and enforced.
- No dead code pointer and no hand-maintained competitive claim.

---

## Stage 3 — P3 first-party adapter and package compatibility

**Why fourth:** the adapters are only worth hardening once the contracts are trustworthy.

| # | Action | Depends on |
|---|---|---|
| — | A real S3-compatible object-store driver (today the S3 side is an in-memory sandbox, so LF-072's temporary URLs and the LF-096 cached-vs-uncached parity have no real backend) | Stage 1 |
| — | The remaining official-package equivalents that are in the declared competitive scope, each decided explicitly: Scout (the search engine), Socialite (the provider set), Cashier (billing), Horizon/Pulse/Telescope (the operator surfaces) | Stage 2 |
| — | The decision records for the excluded packages (facades, Livewire, Dusk, Mix, Valet, Sail) — each with the outcome, the alternative and the profile impact | Stage 2 |

**Exit criteria:** every L1 official package in the declared scope is either implemented with
evidence, adapted with evidence, or excluded with a written rationale that names the alternative.

---

## Stage 4 — P4 the differentiators and the ecosystem

**Why last:** the differentiators are the reason to choose Kelson, and they compound the foundation.

| # | Action | Depends on |
|---|---|---|
| — | The tenant-scoped runtime: the tenant's configuration, limits and flags pinned per generation | Stage 1 |
| — | The independent-artifact model: per-module digests, the drift detector as a release gate, the promotion checkpoint with the F-10 lock (done) | Stage 1 |
| — | The no-rebuild differentiator made visible: the activation latency published per release, the generation id in every response | Stage 0 (KFA-009) |
| — | The audit trail as a product surface (the sealed envelope, the digest chain, the export) | Stage 2 |

**Exit criteria:** the differentiators are demonstrated, measured and published — not described.

---

## The critical path (in one line)

`wasmtime/rustls/webpki bumps` → `the host boots` → `the no-rebuild promise is proven` → `PostgreSQL
is available` → `the fakes are contract-bound` → `the RLS/flag/CLI gaps close` → `the real object
store and the package decisions` → `the differentiators compound`.

Everything on the critical path is either a dependency bump (Stage 0), an environment prerequisite
(PostgreSQL, KFA-022) or a small, well-understood code change (KFA-005, KFA-006, KFA-015). The
expensive work (the parity floor, the adapters) sits **after** the cheap work — deliberately.

---

## The closed findings (their disposition, for completeness)

Six findings in the register are **not** in the plan above because they are already closed or are
recorded for completeness. Their verification evidence is in `TEST-AND-CI-EVIDENCE.md` and their
named cases are green.

| Finding | State | What closed it |
|---|---|---|
| KFA-010 | VERIFIED FIX | The PAT abilities are enforced as a ceiling at the policy boundary (`the_credential_abilities_narrow_the_decision`; `a_personal_access_token_is_confined_to_its_abilities` -> 200 within the grant, 403 outside it) |
| KFA-011 | VERIFIED FIX | The outbound client installs a pinning resolver, so the dialled address is the checked one (`the_pinning_resolver_refuses_a_private_answer`) |
| KFA-012 | VERIFIED FIX | A `PromotionLock` port with a leased file lock is taken before the checkpoint (three named cases) |
| KFA-016 | VERIFIED FIX | The testkit's guard is a `tokio::sync::Mutex`; the `await_holding_lock` suppressions are gone |
| KFA-017 | VERIFIED FIX | The CI checks the free disk before the build and names the disk in the failure |
| KFA-007 | OPEN (Stage 0, minor) | The `rsa` Marvin advisory is transitive through `sqlx-mysql`; it rides the dependency-bump work rather than needing its own stage |
| KFA-023 | OPEN (trivial) | The source parity matrix transposes LF-050 and LF-051; a one-line documentation fix plus a CI order check |

---

## Explicitly not in this plan

- **Anything the audit could not verify.** The PostgreSQL profile, the browser-level journey (no
  browser on this host), the container build (no Docker daemon) and the performance baselines beyond
  the coverage measurement are recorded as NOT RUN or BLOCKED, not as passed. They are release
  **evidence** blockers for the profiles that need them, not code defects.
- **The PHP-shaped surface.** Facades, Livewire, Dusk, Mix, Valet and Sail stay excluded; the
  framework's answer is typed injection, the zero-dep view engine, the testkit, the Rust toolchain and
  a compose file. Each exclusion is recorded in `LARAVEL-PACKAGE-SCOPE.md` with the outcome it still
  provides and how it is proven.
