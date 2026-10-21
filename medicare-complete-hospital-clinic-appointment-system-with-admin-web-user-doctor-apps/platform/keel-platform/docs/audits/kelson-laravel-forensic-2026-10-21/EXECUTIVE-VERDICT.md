# EXECUTIVE VERDICT — Kelson × Laravel forensic competitor audit

**Audit date:** 2026-10-21
**Baseline:** commit `722a8931794cbfae5f95265c7a26d44e4ec6a71f`, branch `p0-runtime-safety`
**Auditor role:** cross-functional review board (framework architecture, Laravel/package specialism,
application & product security, data/ORM & distributed systems, SRE, test architecture, TPM)
**Audit mode:** read-only first pass. No product code was modified by this audit; the only files written
are under this directory.

---

## 1. Verdict

# `NOT PRODUCTION READY`

Three reasons, in order of weight:

1. **Two S0 supply-chain findings sit in the sandbox that runs untrusted code.**
   `wasmtime 36.0.17` carries RUSTSEC-2026-0379 and RUSTSEC-2026-0380 — both **Cranelift
   miscompilation** defects (severity 9.9) — and RUSTSEC-2026-0363 (handle tables capped per
   instance and charged to **no limit**). The path is confirmed: `kelson-wasm` → `kelson-demo` /
   `kelson-worker` / `kelson-framework`. A miscompiled guest can violate the sandbox the whole
   module model depends on.
2. **The reference host does not run.** `kelson-demo` aborts with
   `thread 'main' has overflowed its stack` during boot, reproduced twice (the server path and the
   one-shot `--call` path), while `kelson graph`, `kelson route:list` and `kelson check` all succeed —
   so the graph, the route plan and the architecture gate are not implicated. The harness the test
   suite uses (`build_app_state`) is a **different** path from the real boot, which is why 102 green
   HTTP cases coexist with a crashing host.
3. **The flagship product promise has no evidence.** The no-rebuild contract — hot config/route/
   policy changes without a kernel rebuild or restart — is recorded as `NOT RUN` in the project's own
   P0 execution report. The machinery is present (a 500 ms config-file poller, the route reloader,
   the flag reloader, a boot that refuses a stale config cache) but the process-level experiment has
   never been performed, and per finding KFA-005 it cannot be until the host boots.

`INSUFFICIENT EVIDENCE` was considered and rejected: the PostgreSQL profile is genuinely blocked
(no PostgreSQL on this host), but the failures above are **code and dependency** facts, not
infrastructure gaps.

---

## 2. What was actually executed (this pass)

| Check | Command | Result |
|---|---|---|
| Supply chain | `cargo audit --json` | **10 advisories, 5 crates, 3 critical (9.9)** — wasmtime ×3, rustls ×4, webpki-roots, rustls-webpki, rsa |
| Licence/advisory policy | `cargo deny check` | `advisories FAILED, bans FAILED, licenses ok, sources ok` |
| Architecture gate | `cargo xtask dep-rules` | clean |
| Lint gate | `cargo xtask lint` | clean |
| Graph build | `kelson graph` | 6 modules, 14 operations, 3 events — exit 0 |
| Route plan | `kelson route:list` | 14 routes, parity columns — exit 0 |
| Architecture self-check | `kelson check` | dependency matrix + budgets clean (84 members) — exit 0 |
| Real host boot | `kelson-demo` (KEEL_CONFIG=config/dev.toml) | **CRASH** — `thread 'main' has overflowed its stack` |
| One-shot dispatch | `kelson-demo --call echo.echo --input {..}` | **CRASH** — same overflow |
| Outbox semantics (live MySQL) | `cargo test -p kelson-postgres --features pg-tests --test pg_integration outbox` | 4 passed (the MySQL half real, the pg half self-skipped) |
| Live Redis | Redis re-started on 6379 during this audit | reachable |
| Live MySQL | probe 3307 | **not reachable** — the portable instance's binary is gone; the system instance on 3306 denies the `keel` user |
| PostgreSQL | probe 5433 | **not reachable** — the known native fault (0xC0000142) |
| Container build | `docker` | **absent** |
| Browser journey | chromedriver / playwright | **absent** |
| Coverage | `cargo llvm-cov` (0.9.1 present) | the whole-workspace run **FAILED** with LNK1318 (the disk was at 0 bytes free; the instrumented build needs a second target tree). A scoped run over 4 of the 84 members measured **89.07 % regions / 86.23 % functions / 88.38 % lines** |

Raw artefacts: `RAW-EVIDENCE/`.

---

## 3. The metrics — separate, never merged

Scope frozen before scoring: the **111 Laravel Feature Atlas rows** (`P0-FEATURE-VERIFICATION.md`),
each with its executed evidence. The numerator counts a row only when the matrix itself names the
evidence; the report is never the evidence.

| # | Metric | Numerator | Denominator | Value | Data source |
|---|---|---|---|---|---|
| 1 | `surface_coverage` (implementation exists) | 92 | 111 | **82.9 %** | the matrix's status column (111 − 19 absent) |
| 2 | `behavioral_verification` (every named behaviour proven) | 40 | 111 | **36.0 %** | the VERIFIED rows |
| 3 | `integration_verified` (a real dependency exercised) | 23 | 111 | **20.7 %** | rows naming a live MySQL / Redis / conformance / PostgreSQL case |
| 4 | `end_to_end_verified` (a real user path) | 10 | 111 | **9.0 %** | rows naming a demo / http journey |
| 5 | `security_hardened` (security-negative evidence) | 40 | 111 | **36.0 %** | rows naming a negative/isolation/redaction case |
| 6 | `production_hardened` (production-like evidence) | **0** | 111 | **0.0 %** | no container, no clean-room deploy, and the host does not boot |
| 7 | `selection_coverage` (required-profile capabilities done) | 40 | 111 | **36.0 %** | the VERIFIED rows against the declared profiles |
| 8 | `test_evidence_coverage` (recorded tests or raw artefacts) | 92 | 111 | **82.9 %** | every implemented row names a case |

**The gap between metric 1 (82.9 %) and metric 2 (36.0 %) is the finding that matters**: Kelson
implements most of what the atlas names, and *proves* about a third of it. The gap between metric 6
(0 %) and the others is the second: nothing here has been shown to survive a clean, containerised,
production-like deployment — and the one attempt to start the host failed.

---

## 4. The hard gates (non-compensating)

| Gate | Status | Evidence |
|---|---|---|
| Actual route dispatch end-to-end | **FAIL** | the host aborts on boot (KFA-005); the in-process router is proven but the real server path is not |
| Authentication / authorisation correctness | **PASS** (scoped) | 40 security-named rows, including the PAT ability ceiling (KFA-010) |
| Least-privileged database + RLS tenant isolation | **BLOCKED** | no PostgreSQL (KFA-022) and the RLS assertion is unnamed (LF-089) |
| Durable stores / sessions / queues for the declared profile | **PARTIAL** | MySQL and Redis proven; PostgreSQL unexecuted |
| Atomic runtime activation and failed-activation safety | **NOT RUN** | the host does not boot (KFA-005) |
| The no-rebuild contract for advertised hot changes | **NOT RUN** | never demonstrated (KFA-009) |
| Secret handling and safe error redaction | **PASS** | the `Secret` type (validated UTF-8, redacted `Debug`, wiped drop, constant-time compare) + the redacted error surfaces |
| Reproducible clean build / artifact provenance | **PARTIAL** | the digests and the drift detector are real; the SBOM/provenance generators are untracked (KFA-021) |
| Production container / process startup / health / shutdown | **FAIL** | no Docker daemon; the host crashes (KFA-005) |
| Recovery / rollback and data-migration compatibility | **PARTIAL** | the promotion lock and the rollback are proven; the outbox's PostgreSQL half is unexecuted (KFA-013) |
| Observability and incident diagnostics | **PARTIAL** | the metrics, the traces and the incidents exist; the cardinality and the sensitive labels are unproven (LF-100) |
| No unresolved S0/S1 finding in the release scope | **FAIL** | KFA-001, KFA-002 (S0), KFA-003, KFA-004, KFA-005, KFA-009, KFA-022 (S1) |

**A deliberately unsupported backend does not block a profile** — but PostgreSQL is the engine the
atlas names, so it blocks the PostgreSQL profile, and the release scope cannot quietly exclude it.

---

## 5. The findings register and the plan

- **23 findings** - `ACTIONABLE-FINDINGS.csv`, each with the severity, the type, the source paths, the
  exact command and result, the expected vs actual contract, the root cause (labelled confirmed or
  hypothesis), the impact, the smallest safe fix, the regression tests, the acceptance criteria, the
  owner layer, the effort and the state.
- **The dependency-ordered plan** — `REMEDIATION-ROADMAP.md`, in five stages with entry/exit
  criteria: Stage 0 (the S0 bumps, the boot fix, the no-rebuild proof, PostgreSQL), Stage 1 (the
  conformance suites, the blocking IO, the panic gate), Stage 2 (the parity floor and the CI gates),
  Stage 3 (the adapters and the package decisions), Stage 4 (the differentiators).
- **The critical path** is short and cheap: the dependency bumps → the host boots → the no-rebuild
  promise is proven → PostgreSQL is available → the fakes are contract-bound. The expensive work sits
  deliberately after it.

---

## 6. Prior artefacts — revalidated, not copied

| Prior artefact | Path | Git | Disposition |
|---|---|---|---|
| The capability comparison | `docs/KELSON-VS-LARAVEL-COMPARISON.md` | **untracked** | **stale** — still lists the read/write split as missing (it shipped in F-9) and claims factories that do not exist (KFA-021) |
| The claims verdict | `docs/CLAIMS-VERIFICATION-VERDICT.md` | **untracked** | present; the claims it carries were rechecked individually below |
| The P0 execution report | `docs/P0-EXECUTION-REPORT.md` | tracked @ `722a893` | current; its Phase 1 (`NOT RUN`) is confirmed and now has a **cause** (KFA-005) |
| The forensic audit | `docs/DEEP-AUDIT.md` | tracked @ `e39d80b` | current; its 21 findings were revalidated — 5 now VERIFIED FIX (F-5, F-7, F-8, F-10, F-16), F-9 partially fixed (the MySQL half), 9 still open, and its supply-chain blind spot is now KFA-001…004 |
| The competitive assessment | `docs/LARAVEL-COMPETITIVE-ASSESSMENT.md` | tracked @ `48717a1` | current as an executive abstract; this audit supersedes it as the deep record |
| The feature atlas | `docs/LARAVEL-FEATURE-ATLAS-COMPLETE.md` | tracked @ `722a893` | current; the 111 rows are the frozen scoring scope |
| The prior forensic gap audit | — | — | **not found** as a separate artefact (the DEEP-AUDIT is it) |
| The RuntimeSnapshot / atomic activation plan | — | — | **not found** as a file; the mechanism is in `crates/extension/wasm` and `runtime/kelson-demo/src/hot_reload.rs` |
| The CI workflows / hard-gate register | `.github/workflows/keel.yml` | tracked | current; the architecture, semver, conformance, scaffold and coverage steps are present |
| A delta verification suite | — | — | **not found** |

**Claims rechecked (not copied):**

| Prior claim | Now |
|---|---|
| "The PAT's abilities are carried but not enforced" | **FALSE** — enforced at the policy boundary; verified live (KFA-010) |
| "The egress policy is TOCTOU-vulnerable" | **FALSE** — the client installs a pinning resolver (KFA-011) |
| "A promotion has no concurrency lock" | **FALSE** — a `PromotionLock` port with a leased file lock (KFA-012) |
| "The outbox's rollback/commit-then-crash are unnamed" | **PARTLY FALSE** — named and green on MySQL; the pg half is unexecuted (KFA-013) |
| "The testkit holds a std Mutex across an await" | **FALSE** — fixed (KFA-016) |
| "A full disk breaks the build as a linker error" | **FALSE** — the CI checks the disk first (KFA-017) |
| "The workspace is dependency-clean" | **FALSE** — 10 advisories, 3 critical, on the runtime's own path (KFA-001…004) |
| "The reference host serves" | **FALSE** — it aborts on boot (KFA-005) |

---

## 7. What this audit did not settle

- The **PostgreSQL profile** — no engine on this host (KFA-022). Every pg-only case is a silent
  self-skip, which reads like coverage.
- The **browser journey** — no browser; the Playwright specs in the starter template have never run.
- The **container / clean-room deployment** — no Docker daemon; no SBOM, signature or provenance
  artefact was produced.
- The **performance baseline** — no load harness, no comparable Laravel baseline on this host, so no
  performance claim is published in either direction.
- The **Laravel 13.x surface** was reconciled against the bundle's catalogue and the official-domain
  register (133 rows) as **historical input**; no live documentation fetch was performed in this
  environment, so the baseline's currency is **unverified** and must be re-checked against the 13.x
  docs index before the catalogue is treated as complete.

**The findings register is the deliverable, not a score.** The score is a summary of it; the
register is where the work is.
