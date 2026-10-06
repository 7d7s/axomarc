# 05 — Security & Isolation: Two Planes, One Lattice, Delta-Level Enforcement

> **Version:** 0.9 draft-for-lock · **Date:** 2026-08-19
> This is the normative isolation spec. The isolation matrix test suite (`07-EXECUTION-PLAN.md` M2 gate) is derived 1:1 from the tables below.

---

## 1. Isolation Stack (defense in depth)

| Layer | Mechanism | Enforces |
|---|---|---|
| L0 Edge | TLS, rate limits, request size caps | network-level abuse |
| L1 Plane | Separate token issuers + separate route namespaces; token-plane mismatch = 401 | platform↔tenant bleed |
| L2 Identity | Session JWT (short TTL), rotation, revocation broadcast | impersonation, replay |
| L3 Tenant | Postgres RLS on `medicare.tenant_id` GUC + capability snapshot | cross-tenant read/write |
| L4 Hierarchy | Capability engine prefix matching on `owner_node.path` vs granted prefixes | cross-branch access WITHIN a tenant |
| L5 Field | PHI field policy: redaction in responses, write-once national IDs, delta `changed` filter | over-exposure of sensitive fields |
| L6 Stream | Delta-level isolation: consumer sessions carry allowed prefix sets; fanout filters before delivery | over-exposure in sync/realtime/webhook/audit |
| L7 Audit | Deltas immutable + capability_grant_id recorded + PHI-read log | detection, reconstruction, compliance |

Rule: **a request must pass every layer; no layer trusts the layer above.** RLS is the last blade: even a bug in a handler cannot read another tenant's rows.

---

## 2. Two Security Planes

### 2.1 Tenant plane
- Subjects: users/memberships/roles/grants inside one tenant.
- Can never enumerate or reference other tenants (API exposes only memberships of the caller).
- Session JWTs signed with the tenant-plane key.

### 2.2 Platform plane
- Subjects: PLATFORM_SUPER_ADMIN, PLATFORM_SECURITY, PLATFORM_SUPPORT, PLATFORM_SRE, PLATFORM_RELEASE, PLATFORM_BILLING (persona table in PRD §4).
- Separate signing key, separate pool role, `/platform/v1/*` only.
- **Default: zero PHI access.** PLATFORM_SUPPORT sees tenant metadata; PLATFORM_SRE sees runtime metrics; PLATFORM_BILLING sees plans/usage.

### 2.3 Break-glass (only PHI door for platform)
```
request(tenant, scope_prefix, action_classes, ttl≤4h, reason[required])
 → optional 2nd approver (per tenant policy)
 → grant row (platform_grants) + delta on audit stream
 → platform token gains scoped tenant capability for ttl
 → every action under grant double-audited (grant id + reason stamped on each delta)
 → auto-expire; revocation on demand; expiry job verified by test
```
Break-glass usage triggers notification to tenant admins (delayed disclosure allowed for security investigations, configurable).

### 2.4 Support mode (D-21 — approved 2026-08-19)

Distinct from break-glass: PLATFORM_SUPPORT may open a **read-only** tenant session for investigation:

- Session plane value `support`; capability engine hard-fails every mutating action for this plane (rejected at pipeline stage 9, before any Tx).
- PHI field policy forced to redacted (`phi` masked, `national_id` hidden); every response carries an `X-Support-Session: <actor>` watermark header.
- TTL ≤ 2h, auto-expire; row-budget per session prevents bulk scraping; tenant admin notified on start and end; all reads audited with the support actor id (AUTH-08).

---

## 3. Hierarchy Isolation (L4)

- Every aggregate has exactly one `owner_node`; capability check is path-prefix containment.
- Granularity ladder a tenant can adopt (all optional, enforced the same way):

| Depth | Example grant | Effect |
|---|---|---|
| Tenant-wide | CFO `ledger.read` on legal_entity root | sees all locations |
| Region/Business-unit | Finance manager on `BU-Mumbai` | sees Mumbai branches only |
| Location | Reception on `LOC-Andheri` | sees one clinic |
| Department | OPD supervisor on `DEP-Cardio` under a clinic | sees one department |
| Warehouse | Pharmacist on `WH-Central` | stock only in that warehouse |

- Delegation flows **downward only** (child prefixes of the delegator's prefix); upward delegation is structurally impossible.
- Org moves are privilege-sensitive: moving a node changes every aggregate's effective scope, so `org.move` requires `org.write` + emits a high-severity audit delta; isolation matrix includes move scenarios.

---

## 4. Delta-Level Isolation (L6) — the granular part

Every consumer of change data is constructed with a **filter set**:

```rust
struct StreamFilter {
    tenant: TenantId,                     // hard wall (never bypassed)
    prefixes: Vec<PathPrefix>,            // from capability snapshot (hierarchy)
    streams: Vec<StreamName>,             // subscribed families
    field_policy: FieldPolicy,            // phi: full|redacted|none per field class
}
```

Applies uniformly to:

| Consumer | Where filter is applied |
|---|---|
| WebSocket subscriptions | at subscribe time (scope check) AND at fanout (prefix re-check on every frame) |
| Delta cursor APIs (`/sync/deltas`) | query predicate `owner_path LIKE ANY(prefixes)` enforced in SQL built by kernel, not by module code |
| Webhooks | tenant webhook = whole tenant; module-scoped webhooks (e.g., location-only) inherit prefix filters |
| Reports projections | projection builders run as synthetic subjects with least-privilege grants |
| Audit replay | platform plane only, field_policy=redacted unless break-glass |

Invariants (property-tested):
1. No frame/cursor/batch ever carries a delta whose tenant ≠ consumer tenant.
2. No delta whose owner_path ∉ consumer prefixes.
3. No field of class `secret` in any delta; fields of class `phi` stripped unless consumer holds `*.phi.read`.
4. Filters are recomputed when capability snapshot version changes (grant revoked ⇒ live WS sessions re-validated within one heartbeat).

---

## 5. Field Classes & PHI Policy

| Class | Examples | Storage | Responses | Deltas |
|---|---|---|---|---|
| public | names, roles, clinic address | plain | yes | yes |
| internal | fees, stock quantities | plain | per capability | yes |
| phi | DOB, address, vitals, diagnosis, Rx | plain + column policy | per `*.phi.read` | per field_policy |
| secret | tokens, OTPs, webhook secrets | hashed/encrypted | never | never |
| national_id | Aadhaar/SSN/PAN | field-encrypted (app-level AES-GCM, key in KMS) | write-once, masked `***1234` | never raw; masked only |

---

## 6. Audit & Reconstruction

- Every mutating delta carries `actor_id` + `capability_grant_id` (+ `reason` where workflow requires).
- PHI **reads** are logged as `access` rows on the audit stream (aggregate=subject access log), enabling GDPR/DPDP Article-15-style "who saw my data" answers.
- Reconstruction: replay of a stream reproduces current aggregate state; drift detector runs nightly (replay sample vs live rows) and alerts on divergence.
- Retention: audit 7 years (configurable per tenant policy), sync streams 90 days + snapshots.

---

## 7. Isolation Matrix Test Suite (normative, CI-gated)

Matrix axes (finite, enumerable):

- **Subjects:** platform personas ×2, tenant admin, location staff (single clinic), doctor (2 clinics), patient (self), finance (BU scope), pharmacist (warehouse scope)
- **Operations:** read list, read one, create, update, transition, delete/deactivate, sync deltas, WS subscribe, webhook receive
- **Resources across:** other tenant, sibling location, parent scope, child scope, own scope, moved node

Every cell has an expected verdict (allow/deny). Suite runs:
1. As integration tests against real Postgres (RLS on) — M2 gate requires **0 unexpected verdicts, 0 leaked rows**.
2. As delta-stream tests: seed N deltas across the lattice, assert each consumer receives exactly its allowed set.
3. IDOR fuzzer: nightly job replays recorded request corpus with mutated ids across tenants/scopes; any 200 where deny expected = P0 incident.

---

## 8. Compliance Mapping (V1 baseline)

| Control | DPDP 2023 | HIPAA-style export |
|---|---|---|
| Consent ledger (patients.consent + deltas) | purpose consent recorded at collection | — |
| Right to erasure workflow | pseudo-delete + field-level crypto-shred (key per field batch) | — |
| Access control matrix | §8 reasonable security safeguards | 164.312(a) |
| Audit trail | breach investigation support | 164.312(b) |
| Breach runbook | notification timelines | 164.410 |
| Data residency | region-locked storage/DB | — |

ABDM/FHIR: explicit non-goal in V1; kernel keeps schema-agnostic stubs so a later gateway module does not force core schema changes.

---

## 9. Secrets, Keys, Hardening Checklist (GA gate)

- Rotation: JWT signing keys (kid-based, zero-downtime rotation), DB creds (via secret store), gateway webhooks.
- Boot-time hardening asserts: TLS material present for non-loopback binds, RLS policies exist on every table flagged `tenant_table` (introspection check), no permissive default grants, rate limiters configured.
- Dependency policy: `cargo deny` (advisories, licenses MIT/Apache/BSD, < 40 direct crates budget tracked in CI report).
- Headers: HSTS, nosniff, frame-deny, CSP on any server-rendered surface.