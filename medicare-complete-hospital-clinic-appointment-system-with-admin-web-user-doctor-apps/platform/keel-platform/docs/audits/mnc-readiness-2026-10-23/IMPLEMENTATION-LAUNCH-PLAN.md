# IMPLEMENTATION & LAUNCH PLAN — MNC-grade readiness

**Input:** `MNC-READINESS-AUDIT.md` (MNC-001..054), the MVA batch
(`../mvvc-first-2026-10-23/`), and the forensic register
(`../kelson-laravel-forensic-2026-10-21/`).
**Rule:** a stage may not start until the previous stage's exit criteria hold;
every row names its own gate. Effort: S/M/L. Nothing here is approved — the
register rows go one-by-one to the owner first, exactly as the KFA/MVA rows did.

## Stage 0 — the trust boundary (S0 blockers, ~1 week of diffs)

**Why:** seven S0 rows are all small, local, and security-shaped. Every later
stage is meaningless while any authenticated user can enter any tenant, a
forgotten SET LOCAL reveals all rows, two patients can double-book one slot,
requests have no timeout, and the shipped container serves nobody.

| Row | Fix | Effort | Gate (named case) |
|---|---|---|---|
| MNC-001 | membership check before `install_access_snapshot` | S | "a login into a tenant the principal is not a member of is refused" integration case |
| MNC-010 | drop the `OR … IS NULL` RLS branch; grants for platform roles | S | live-pg case: unset GUC reads zero rows |
| MNC-011 | partial unique index + 23505→CONFLICT; status-filtered probe | M | two-patients-one-slot race case |
| MNC-028 | TimeoutLayer in `Admission::apply` + config knob | S | a slow route answers 503 DeadlineExceeded |
| MNC-030 | statement_timeout + bridge submit returns Err, no expect | S | bridge-survives-panicking-job case |
| MNC-041 | worker SIGTERM via the demo `shutdown_signal()` | S | kill -TERM drains in-flight job (manual + CI signal test) |
| MNC-039 | prod config overlay binding 0.0.0.0; image default = prod | S | compose-up probe from the host network |
| MNC-006 | TLS: tokio-rustls acceptor OR the promised boot assert for non-loopback without proxy-TLS | M | "non-loopback bind refuses under no TLS/proxy" case |

**Exit:** S0 register empty; the `kelson-demo` prod overlay starts and answers
`/healthz` from outside the container; a negative-security integration suite
covers all eight rows.

## Stage 1 — healthcare obligations + the open S1 cluster (~2–3 weeks)

Split by dependency:

**1a. Auth/authz hardening (independent, small):** MNC-003 constant-time CSRF
compare; MNC-004 trusted-proxy XFF; MNC-005 register throttle + uniform 202;
MNC-007 webhook egress IP pinning; MNC-002 deny.toml into this workspace
(then the CI gate is real); MNC-043 admin capability on ops routes.

**1b. PHI controls (depend on Stage 0's membership fix for object authz):**
MNC-019 encrypted casts on patient/email + sealer boot; MNC-020
appointment↔principal FK + ownership policy on list/read; MNC-021 read-access
audit records; MNC-042 payload redaction in the worker + PHI vocabulary in
classify; MNC-025 rewrite `docs/20-SECURITY-COMPLIANCE.md` to describe what
IS true (kills the false-assurance liability cheapest of all).

**1c. Runtime guarantees:** MNC-032 (=KFA-006) refuse degraded boot in prod;
MNC-031 symmetric pg schema assert; MNC-029 host-default guest deadline +
concurrency; MNC-033 async+pg-backed serialized lock; MNC-036 bounded drain +
no early process::exit.

**1d. MVVC completion (the MVA batch, already underway):** MVA-004/006
verification (proof suite), then MVA-007 riding MNC-012's tx-wrapped
read-model refresh, MVA-008 live edge, MVA-013 recorded.

**Exit:** every S1 row VERIFIED FIX or explicitly dispositioned; the
compliance doc matches the code; `cargo deny check` is meaningful in this
workspace; the proof page suite is fully green.

## Stage 2 — persistence and scale hardening (S2 cluster, ~1–2 weeks)

MNC-014 indexes; MNC-015 FK actions + uniform soft-delete; MNC-016 pool
lifetimes; MNC-017 runtime-shipped, idempotent migrations; MNC-018 clamped
lists; MNC-012/013 read-model install + validation; MNC-034 spawn_blocking
dispatch; MNC-035 poison-safe unwraps; MNC-037 wasm:promote reload; MNC-038
guest error redaction; MNC-044 alert action → notification hub.
**Exit:** the pg matrix + a load probe pass on live fabric; no tautological
validation left in the readmodel registry; kelson about/doctor/graph agree.

## Stage 3 — the product wave (MNC-027: the apps the repo promises)

With the framework safe to expose: admin-web (operator console riding the
dashboard/ops VMs + read models), user app (book/cancel/list riding
MVA-004..008's proven lane), doctor app (schedule/patient-scope lanes riding
MNC-020's object authz). Each app is an applications/<name> module with its
own manifests + views — the first real consumers of the MVVC contract.
**Exit:** a Playwright journey per app (Stage 4's CI runs them); the parity
matrix's end_to_end_verified stops being 9%.

## Stage 4 — evidence infrastructure (parallel with 2/3; feeds all)

MNC-049 CI e2e job (browser on the runner) + medicare journeys;
MNC-048 always-on pg/redis service jobs + llvm-cov ratchet; MNC-050 proptest
at the parser/validation/ABI seams; MNC-051 Deprecation/Sunset headers + a
real semver tag gate; MNC-053 injectable clock (kills the F-013 flake class);
MNC-045/026 backup/restore commands + residency story; MNC-047 cosign
attestation + image push.
**Exit:** the five EXECUTIVE-VERDICT metrics (surface/behavioral/integration/
E2E/security) recomputed on THIS tree with executed evidence, not static
reading — the audit's own numbers become the ratchet.

## Stage 5 — enterprise finish (S3)

MNC-009 version pins; MNC-046 OTLP overlay; MNC-052 README/LICENSE/a11y/i18n
in templates; MNC-054 MVVC examples.

## Critical path, one line

`MNC-001 + 010 + 011 + 028` (the trust boundary) → `prod overlay binds 0.0.0.0 + TLS assert` → `PHI cast/audit/consent (1b)` → `MVVC proof green + MVA-007/008` → `the three apps exist with e2e` → `the evidence pipeline recomputes the verdict`.

**Deliberately not estimated:** consent-model legal review, BAA/DPA paperwork,
HIPAA certification — product/compliance decisions the register exposes but
engineering cannot close alone.
