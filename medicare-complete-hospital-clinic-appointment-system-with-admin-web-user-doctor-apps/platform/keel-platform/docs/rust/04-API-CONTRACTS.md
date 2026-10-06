# 04 — API Contracts: Medicare Execution Kernel

> **Version:** 0.9 draft-for-lock · **Date:** 2026-08-19
> The full surface emitted per release is an OpenAPI 3.1 artifact generated from the SmartRouter registry (NFR-M3). This doc fixes the conventions and the stable V1 shape.

---

## 1. Conventions

- Base: `/api/v1` (tenant plane), `/platform/v1` (platform plane), `/realtime/v1` (WS upgrade).
- Auth: `Authorization: Bearer <access JWT>`. Refresh: `POST /api/v1/auth/refresh`.
- Mutating requests SHOULD send `Idempotency-Key` (uuid); replay window 24h.
- Concurrency: `If-Match: <version>` on PATCH/state-change; mismatch → `409 conflict.version`.
- Envelope:

```json
{ "data": { }, "meta": { "request_id": "…", "etag": "…", "cursor": "…", "has_more": false } }
```

- Error:

```json
{ "error": { "code": "capability.denied", "message": "…", "details": { "action": "invoice.approve" }, "request_id": "…" } }
```

- Stable error codes: `auth.invalid_token`, `auth.expired`, `tenant.suspended`, `module.disabled`, `plan.limit`, `validation.failed`, `capability.denied`, `sod.violation`, `workflow.invalid_transition`, `conflict.version`, `conflict.slot_taken`, `idempotency.replay`, `internal`.
- Pagination: `?limit=50&cursor=…` (opaque, delta-safe). No offset pagination on mutable collections.
- Rate limits: per-token buckets; headers `X-RateLimit-Remaining/Reset`; public directory endpoints have stricter anonymous budgets.

---

## 2. Auth & Identity

| Method | Path | Capability | Notes |
|---|---|---|---|
| POST | `/api/v1/auth/otp/start` | public | Phone E.164; server issues OTP via SMS provider (D-13); rate-limited per number+IP; uniform response (AUTH-07) |
| POST | `/api/v1/auth/otp/verify` | public | Server-side code verify (hashed compare, attempt cap, expiry) → session pair |
| POST | `/api/v1/auth/login` | public | Email + password |
| POST | `/api/v1/auth/refresh` | session | Rotate refresh |
| POST | `/api/v1/auth/logout` | session | Revoke session family |
| GET | `/api/v1/me` | any | Identity, memberships, tenant list, plane |
| POST | `/api/v1/me/tenant/select` | any | Pin active tenant when multi-membership |

Session response: `{ access_token (15m), refresh_token (rotating), expires_at, user, memberships[] }`.

---

## 3. Meta & Discovery (powers client plug'n'play)

| Method | Path | Capability | Notes |
|---|---|---|---|
| GET | `/api/v1/meta/modules` | any | Modules enabled for this tenant: id, version, flags, ui_hints (routes/panels available) |
| GET | `/api/v1/meta/permissions` | any | Caller's granted action classes + scope prefixes (clients hide what can't be used) |
| GET | `/api/v1/meta/flags` | any | Feature flags for this tenant |

---

## 4. Org & Access (mod-org)

| Method | Path | Capability |
|---|---|---|
| GET/POST | `/api/v1/org/nodes`, `GET/PATCH /org/nodes/{id}`, `POST /org/nodes/{id}/move` | `org.read` / `org.write` |
| GET/POST | `/api/v1/users`, `PATCH /users/{id}`, `POST /users/{id}/deactivate` | `user.manage` |
| GET/POST | `/api/v1/roles`, `PUT /roles/{id}/grants` | `role.manage` |
| POST | `/api/v1/grants`, `DELETE /grants/{id}`, `POST /grants/{id}/delegate` | `grant.manage` |
| GET | `/api/v1/settings`, `PUT /api/v1/settings` | `settings.manage` (versioned body) |

Move endpoint recomputes subtree paths in-transaction; response includes affected descendant count.

---

## 5. Clinic Structure (mod-clinic)

| Method | Path | Capability |
|---|---|---|
| GET/POST | `/api/v1/cities` | `clinic.meta.read/write` |
| GET/POST/PATCH | `/api/v1/clinics` (+ `/clinics/{id}/profile`) | `clinic.manage` |
| GET/POST/PATCH | `/api/v1/departments`, `/api/v1/specializations`, `/api/v1/services` | `clinic.manage` |
| GET | `/api/v1/public/doctors?city=&specialization=&clinic=&q=` | public, rate-limited |

---

## 6. Doctors (mod-doctor)

| Method | Path | Capability |
|---|---|---|
| GET/POST/PATCH | `/api/v1/doctors` | `doctor.read` / `doctor.manage` |
| GET | `/api/v1/doctors/{id}/reviews` | `doctor.read` (verified-visit flag) |
| PUT | `/api/v1/doctors/{id}/schedules` | `doctor.manage` |
| POST | `/api/v1/doctors/{id}/exceptions` | `doctor.manage` |
| GET | `/api/v1/slots?doctor=&clinic=&service=&from=&to=` | `slot.read` or public-decorated |
| GET | `/api/v1/doctors/me/today` | doctor self | Today sheet: queue, appointments, counts |

Slot responses include `slot_id` + `hold` semantics: `POST /api/v1/slots/{id}/hold` (10-min TTL, patient booking flow).

---

## 7. Patients (mod-patient)

| Method | Path | Capability |
|---|---|---|
| GET/POST/PATCH | `/api/v1/patients` | `patient.read/write` (owner-path scoped) |
| GET | `/api/v1/patients/{id}/timeline` | `patient.read` | Aggregated: visits, rx, vitals |
| POST/GET | `/api/v1/patients/{id}/family` | patient self or `patient.family.manage` |
| POST/GET | `/api/v1/patients/{id}/vitals` | `vitals.write/read` |
| POST | `/api/v1/patients/{id}/files` → presigned upload | `patient.files.write` |
| GET | `/api/v1/patients/{id}/files/{fid}/access` → presigned download (5-min TTL) | `patient.files.read` |

Aadhaar/SSN fields: accept-only endpoints, never returned in GET (write-once, redacted in responses + deltas).

---

## 8. Appointments & Queue (mod-appointment)

| Method | Path | Capability | Notes |
|---|---|---|---|
| POST | `/api/v1/appointments` | `appointment.write` | Body: slot_id (or hold id), patient_ref, service_id. Idempotent. `409 conflict.slot_taken` on race |
| GET | `/api/v1/appointments?status=&doctor=&clinic=&date=` | `appointment.read` | Day sheet, calendar views |
| GET | `/api/v1/appointments/{id}` | `appointment.read` | Includes state history (deltas) |
| POST | `/api/v1/appointments/{id}/transition` | `appointment.transition` | `{event: "check_in" | "start_consult" | "complete" | "no_show"}` — workflow-validated |
| POST | `/api/v1/appointments/{id}/checkin` | `appointment.checkin` | Manual or `{qr_token}` scan |
| GET | `/api/v1/queue?doctor=&date=` | `queue.read` | Live board data |
| POST | `/api/v1/queue/{token_id}/call` | `queue.call` | Emits realtime call event |
| POST | `/api/v1/appointments/{id}/cancel-request` | patient self | `{reason}` → CANCEL_REQUESTED |
| POST | `/api/v1/appointments/{id}/cancel-decision` | `appointment.cancel.approve` | approve/decline; refund workflow trigger |

All transitions return the updated state + next allowed events so clients can render buttons without knowing the machine.

---

## 9. Prescriptions (mod-rx)

| Method | Path | Capability |
|---|---|---|
| POST | `/api/v1/prescriptions` | `rx.write` (doctor, from appointment) |
| POST | `/api/v1/prescriptions/{id}/transition` | `rx.transition` | `issue`, `revoke` |
| GET | `/api/v1/prescriptions/{id}` (+ `/pdf` → presigned) | `rx.read` |
| GET | `/api/v1/medicines?q=` | `rx.read` | Master autocomplete |

---

## 10. Billing & Payments (mod-billing)

| Method | Path | Capability | Notes |
|---|---|---|---|
| POST | `/api/v1/invoices` | `invoice.write` | Draft from appointment/services |
| POST | `/api/v1/invoices/{id}/transition` | `invoice.finalize` / `invoice.post` | post requires balanced ledger build |
| POST | `/api/v1/invoices/{id}/reverse` | `invoice.reverse` | Maker-checker above threshold |
| GET | `/api/v1/ledger?account=&from=&to=` | `ledger.read` |
| POST | `/api/v1/payments` | `payment.create` | Creates intent + gateway order |
| POST | `/api/v1/payments/{id}/capture` | `payment.capture` |
| POST | `/webhooks/v1/payments/razorpay` | signature-verified | Gateway webhook (outside tenant auth; HMAC/verify) |
| POST | `/api/v1/refunds` | `refund.request` | Workflow when above threshold |
| GET | `/api/v1/reconciliations`, `/reconciliations/{id}` | `recon.read` |

---

## 11. Inventory & Procurement (mod-inventory)

| Method | Path | Capability |
|---|---|---|
| GET | `/api/v1/stock?warehouse=&medicine=` | `stock.read` |
| POST | `/api/v1/stock/movements` | `stock.adjust` (adjust/receipt); issues flow from Rx workflow |
| POST | `/api/v1/purchase-requests` | `procure.request` |
| POST | `/api/v1/purchase-requests/{id}/transition` | `procure.approve` | Workflow events: submit/approve/reject/convert_po |
| POST | `/api/v1/purchase-orders/{id}/receive` | `procure.receive` | Goods receipt → stock movements |

---

## 12. Sync & Deltas (kernel-delta) — the offline-sync contract

```
GET /api/v1/sync/bootstrap?streams[]=appointments&streams[]=prescriptions
→ { data: { snapshot: {…minimal initial state…}, cursors: { appointments: 10422, prescriptions: 887 } } }

GET /api/v1/sync/deltas?streams=appointments,prescriptions&after=appointments:10422,prescriptions:887&limit=500
→ { data: { deltas: [ {stream, seq, aggregate, aggregate_id, action, state, changed, version} ],
     cursors: { appointments: 10922, prescriptions: 887 }, has_more: true } }
```

- Per-subject **scope filtering** applies server-side (delta-level isolation): a location-scoped user receives only deltas with matching owner_path prefix.
- Ordering: monotonic per stream; response interleaves streams but each stream is independently resumable.
- Clients apply by `(aggregate_id, version)` — skip if `version <= known`.
- `POST /api/v1/sync/register-device` binds a push token for delta nudge (client polls on nudge; server never pushes payloads over FCM).

---

## 13. Webhooks Out (tenant-facing, P4)

| Method | Path | Capability |
|---|---|---|
| GET/POST | `/api/v1/webhooks` | `webhook.manage` |
| POST | `/api/v1/webhooks/{id}/test`, `POST /webhooks/{id}/replay?from=&to=` | `webhook.manage` |

Delivery envelope: `{ id, tenant_id, stream, seq, event: "appointment.transition", data: {…delta…}, signature headers }`. HMAC-SHA256 header `X-Medicare-Signature = hex(hmac(secret, body))`; retries 1m/5m/30m/2h/12h then dead-letter.

---

## 14. Realtime (see 06 for internals)

- `GET /realtime/v1/ws?token=<short-lived socket token>` → Upgrade.
- Subscribe frames scoped by capability: `queue:{doctor_or_node}`, `calling:{appointment}`, `board:{node}`, `today:{doctor_self}`.
- Server frames: `{ channel, event, payload, seq }`; payloads are delta-shaped so clients reuse sync apply logic.

---

## 15. Platform Plane (platform plane only)

| Method | Path | Capability |
|---|---|---|
| GET/POST | `/platform/v1/tenants` | `platform.tenant.manage` |
| POST | `/platform/v1/tenants/{id}/suspend` / `/resume` / `/provision` | `platform.tenant.manage` |
| GET | `/platform/v1/tenants/{id}/health` | `platform.sre` |
| POST | `/platform/v1/break-glass` (request), `POST /platform/v1/break-glass/{id}/approve` | `platform.security` |
| POST | `/platform/v1/tenants/{id}/support-session` | `platform.support` | Read-only, redacted, watermarked; TTL ≤ 2h (D-21) |
| GET | `/platform/v1/tenants/{id}/export` | `platform.support` | JSON aggregates + files archive + PDF bundle (invoices/Rx) (D-22) |
| POST | `/platform/v1/tenants/{id}/erase` | `platform.security` | DPDP crypto-shred + soft-delete workflow; dual approval |
| GET | `/platform/v1/modules` | `platform.release` |
| PUT | `/platform/v1/modules/{module}/availability` | `platform.release` |
| GET/POST | `/platform/v1/router/canary` | `platform.release` |

Platform endpoints never accept tenant-plane tokens; tenant endpoints never accept platform tokens except with a live, unexpired, approved break-glass grant (AUTH-05, PLT-03).