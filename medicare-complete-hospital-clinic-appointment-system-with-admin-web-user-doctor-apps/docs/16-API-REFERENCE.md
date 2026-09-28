# 16 — API Reference (Every Endpoint)

> The complete public API surface. Every endpoint grouped by module. Use this as the contract; if it's not here, it doesn't exist (yet).

---

## 1. Conventions

### 1.1 URL

- Base: `https://{api-host}/api/v1`
- Tenancy: subdomain or path prefix per `03-MODULE-TENANCY`.
- Versioned: breaking changes ship as `/v2/...` while `/v1/...` keeps working for 12 months.

### 1.2 Auth

- Mobile: `Authorization: Bearer {sanctum_token}`
- Web: `Authorization: Bearer {token}` OR session cookie (`medicare_session`, `Secure`, `HttpOnly`, `SameSite=Lax`)
- Public endpoints explicitly marked `🔓`.
- Tenant context: resolved from host (subdomain/path) or `X-Tenant: {slug}` header.

### 1.3 Request

- `Content-Type: application/json` (except file uploads: `multipart/form-data`).
- `Accept: application/json`.
- `Accept-Language: en` (optional; default from `users.locale`).
- `Idempotency-Key: {uuid}` (optional; for POST that creates).
- `X-Request-Id: {uuid}` (optional; auto-generated if missing, returned in response).
- `X-Module-Version: 1.0.0` (optional; for client schema checks).

### 1.4 Response

- JSON. Always wrapped:
  ```json
  { "data": { ... } }
  ```
  or for collections:
  ```json
  { "data": [ ... ], "meta": { "current_page": 1, "last_page": 7, "per_page": 20, "total": 134 } }
  ```

### 1.5 Errors

RFC 7807 Problem Details (see 00-ARCHITECTURE §9.2):
```json
{
  "type": "https://medicare.app/errors/validation",
  "title": "Validation failed",
  "status": 422,
  "detail": "The given data was invalid.",
  "instance": "/api/v1/appointments",
  "errors": { "doctor_id": ["Doctor is required"] },
  "trace_id": "01J7XQ8Y4HM0F8C3K2RYN3WX9V"
}
```

### 1.6 Status Codes

| Code | Meaning |
|---|---|
| 200 | OK with body |
| 201 | Created with body (Location header) |
| 202 | Accepted (async, tracking id in body) |
| 204 | No content |
| 304 | Not modified (ETag match) |
| 400 | Bad request (validation, business rule) |
| 401 | Unauthorized (no/bad token) |
| 403 | Forbidden (authenticated but not allowed) |
| 404 | Not found (or 403-by-stealth for cross-tenant) |
| 409 | Conflict (state machine violation, slot taken, duplicate) |
| 422 | Unprocessable entity (validation failed) |
| 423 | Locked (tenant suspended) |
| 429 | Too many requests (rate limit) |
| 500 | Server error (Sentry event) |
| 501 | Not implemented (V2 feature stub) |
| 502 | Bad gateway (upstream error) |
| 503 | Unavailable (maintenance) |

### 1.7 Rate Limits

| Realm | Limit | Window |
|---|---|---|
| Public (no auth) | 60 req | 1 min / IP |
| Authenticated (any role) | 600 req | 1 min / user |
| Webhooks | unlimited (signature-verified) |
| Patient mobile | 600 req | 1 min / device |
| Admin web | 1200 req | 1 min / user |

429 returns `Retry-After: {seconds}` header.

### 1.8 Pagination

All list endpoints:
- `?page=1&per_page=20` (default 20, max 100)
- Response `meta` includes `current_page`, `last_page`, `per_page`, `total`.
- `Link` header: `Link: <...>; rel="next"`, `rel="prev"`, `rel="first"`, `rel="last"`.

### 1.9 Filtering

- `?field=value` for exact match
- `?field__gt=`, `__gte`, `__lt`, `__lte`, `__in=a,b,c`, `__like=`, `__null`
- `?q=` for free-text search (where supported)
- `?sort=field` (asc) or `-field` (desc)
- `?include=relation,relation.nested` for eager-loaded data

### 1.10 Caching

- `Cache-Control: public, max-age=N` on public read endpoints.
- `ETag` and `304 Not Modified` on all GETs.
- Cache-invalidation broadcasts on every write (see 17-REALTIME).

---

## 2. Auth Module

### 2.1 Mobile (Realm A — OTP)

```
POST   /auth/otp/send         🔓    { phone, purpose }            → { sent: true, retry_after: 60 }
POST   /auth/otp/verify       🔓    { id_token, fcm_token?, device_type? }
                                          → { token, expires_at, user, roles, permissions, tenant }
                                          or { status: "new_user", phone }
POST   /auth/logout           🔐    → 204
POST   /auth/refresh          🔐    → { token (rotated) }
GET    /auth/me               🔐    → { user, roles[], permissions[] }
PUT    /auth/me               🔐    { ...profile fields }          → { user }
POST   /auth/fcm/register     🔐    { token, device_type, app_version }  → { fcm_token_id }
DELETE /auth/fcm/{id}         🔐    → 204
POST   /auth/register/patient 🔓    { phone, first_name, last_name, dob?, gender?, email? }
                                          → { token, user, ... }
```

### 2.2 Web (Realm B — Email + Password)

```
POST   /auth/login            🔓    { email, password, device_name? }  → { token|cookie, user, roles, permissions }
POST   /auth/logout           🔐    → 204
POST   /auth/forgot-password  🔓    { email }                          → 202
POST   /auth/reset-password   🔓    { token, email, password }         → 200
GET    /auth/email/verify/{id}/{hash} 🔓 → 302 → /login?verified=1
POST   /auth/email/resend     🔐                                          → 202
POST   /auth/2fa/enable       🔐                                          → { qr_code_url, secret }
POST   /auth/2fa/confirm      🔐    { code }                            → 200
POST   /auth/2fa/verify       🔓    { code }                            → { token, user }
```

### 2.3 Users (Admin)

```
GET    /users                 🔐(admin)  ?search=&role=&status=&page=
POST   /users                 🔐(admin)  { first_name, last_name, email, phone, role, password }
GET    /users/{id}            🔐(admin)
PUT    /users/{id}            🔐(admin)
DELETE /users/{id}            🔐(admin)  (soft delete)
POST   /users/{id}/reset-password    🔐(admin)  → { temp_password }
POST   /users/{id}/impersonate       🔐(super)  → { token, expires_at }
POST   /users/{id}/suspend           🔐(admin)  { reason }
POST   /users/{id}/activate          🔐(admin)
GET    /users/{id}/audit-logs        🔐(admin)
GET    /users/login-audits           🔐(admin)  ?user_id=&from=&to=&page=
```

### 2.4 Roles (Admin)

```
GET    /roles                 🔐(admin)
POST   /roles                 🔐(admin)  { name, description, permissions[] }
GET    /roles/{id}            🔐(admin)
PUT    /roles/{id}            🔐(admin)
DELETE /roles/{id}            🔐(admin)
POST   /roles/{id}/assign     🔐(admin)  { user_ids[] }
POST   /roles/{id}/revoke     🔐(admin)  { user_ids[] }
POST   /roles/{id}/sync-permissions  🔐(admin)  { permissions[] }
```

### 2.5 Permissions

```
GET    /permissions           🔐(admin)  (grouped)
GET    /permissions/groups    🔐(admin)
```

---

## 3. Tenancy

```
GET    /tenants               🔐(super)         list (super-admin)
POST   /tenants               🔐(super)         { name, slug, admin_email, ... }
GET    /tenants/{id}          🔐(super/admin)
PUT    /tenants/{id}          🔐(super/admin)
DELETE /tenants/{id}          🔐(super)         (soft + cascade suspension)
POST   /tenants/{id}/suspend  🔐(super)         { reason }
POST   /tenants/{id}/activate 🔐(super)
GET    /tenants/{id}/domains  🔐(admin)
POST   /tenants/{id}/domains  🔐(admin)         { domain }
POST   /tenants/{id}/domains/{domainId}/verify  🔐(admin)   (DNS check)
DELETE /tenants/{id}/domains/{domainId}  🔐(admin)
GET    /tenants/{id}/stats    🔐(admin)         { patients, doctors, appointments, revenue }
```

---

## 4. Clinic Structure

### 4.1 Cities

```
GET    /public/cities         🔓  ?country=&q=
GET    /admin/cities          🔐(admin)
POST   /admin/cities          🔐(admin)
PUT    /admin/cities/{id}     🔐(admin)
DELETE /admin/cities/{id}     🔐(admin)
```

### 4.2 Clinics

```
GET    /public/clinics        🔓  ?city_id=&q=
GET    /public/clinics/{slug} 🔓
GET    /admin/clinics         🔐(admin)
POST   /admin/clinics         🔐(admin)
GET    /admin/clinics/{id}    🔐(admin)
PUT    /admin/clinics/{id}    🔐(admin)
DELETE /admin/clinics/{id}    🔐(admin)
POST   /admin/clinics/{id}/upload-logo   🔐(admin)  multipart
POST   /admin/clinics/{id}/upload-cover  🔐(admin)  multipart
GET    /admin/clinics/{id}/doctors       🔐(admin)
GET    /admin/clinics/{id}/reports       🔐(admin)
```

### 4.3 Departments

```
GET    /public/departments    🔓
GET    /admin/departments     🔐(admin)
POST   /admin/departments     🔐(admin)
GET    /admin/departments/{id}🔐(admin)
PUT    /admin/departments/{id}🔐(admin)
DELETE /admin/departments/{id}🔐(admin)
POST   /admin/departments/reorder  🔐(admin)  { ids: [...] }
```

### 4.4 Specializations

```
GET    /public/specializations  🔓
GET    /admin/specializations   🔐(admin)
POST   /admin/specializations   🔐(admin)
GET    /admin/specializations/{id}  🔐(admin)
PUT    /admin/specializations/{id}  🔐(admin)
DELETE /admin/specializations/{id}  🔐(admin)
```

### 4.5 Services

```
GET    /public/services       🔓
GET    /admin/services        🔐(admin)
POST   /admin/services        🔐(admin)
GET    /admin/services/{id}   🔐(admin)
PUT    /admin/services/{id}   🔐(admin)
DELETE /admin/services/{id}   🔐(admin)
POST   /admin/services/bulk-import  🔐(admin)  multipart
GET    /admin/services/{id}/usage   🔐(admin)
```

---

## 5. Doctors

```
GET    /public/doctors        🔓  ?city_id=&clinic_id=&department_id=&specialization_id=&service_id=&q=&min_rating=&min_fee=&max_fee=&available_on=&accepts_online=&accepts_in_person=&sort=&page=
GET    /public/doctors/{id}   🔓
GET    /public/doctors/{id}/schedule   🔓  ?from=&to=
GET    /public/doctors/{id}/slots      🔓  ?date=&type=
POST   /public/doctors/{id}/slots/{slotId}/hold  🔐  { hold_minutes?: 10 }
GET    /public/doctors/{id}/reviews    🔓  ?rating=&page=
GET    /public/doctors/{id}/reviews/histogram  🔓

GET    /admin/doctors         🔐(admin)  ?q=&clinic_id=&department_id=&is_active=
POST   /admin/doctors         🔐(admin)  { user data + profile data }
GET    /admin/doctors/{id}    🔐(admin)
PUT    /admin/doctors/{id}    🔐(admin)
DELETE /admin/doctors/{id}    🔐(admin)
POST   /admin/doctors/{id}/avatar      🔐(admin)  multipart
POST   /admin/doctors/{id}/cover       🔐(admin)  multipart

# Schedules
GET    /admin/doctors/{id}/schedules   🔐(admin)
POST   /admin/doctors/{id}/schedules   🔐(admin)  bulk save
PUT    /admin/doctors/{id}/schedules/{sid}  🔐(admin)
DELETE /admin/doctors/{id}/schedules/{sid}  🔐(admin)

# Reviews
GET    /admin/reviews         🔐(admin)  ?is_approved=&min_rating=&doctor_id=
POST   /admin/reviews/{id}/approve  🔐(admin)
POST   /admin/reviews/{id}/reject   🔐(admin)  { reason }
POST   /admin/reviews/{id}/feature  🔐(admin)

# Doctor app
GET    /doctor/me/profile     🔐(doctor)
PUT    /doctor/me/profile     🔐(doctor)
GET    /doctor/dashboard      🔐(doctor)  ?date=
GET    /doctor/appointments   🔐(doctor)  ?date=&status=
GET    /doctor/appointments/{id}  🔐(doctor)
POST   /doctor/appointments/{id}/start  🔐(doctor)
POST   /doctor/appointments/{id}/complete  🔐(doctor)  { doctor_notes? }
POST   /doctor/appointments/{id}/notes  🔐(doctor)  { doctor_notes }
GET    /doctor/patients       🔐(doctor)  ?q=&limit=
GET    /doctor/patients/{id}  🔐(doctor)
GET    /doctor/schedule       🔐(doctor)  ?from=&to=
```

---

## 6. Patients

```
# Patient (self) - mobile/web
GET    /patient/me            🔐
PUT    /patient/me            🔐
GET    /patient/family        🔐
POST   /patient/family        🔐
GET    /patient/family/{id}   🔐
PUT    /patient/family/{id}   🔐
DELETE /patient/family/{id}   🔐
POST   /patient/family/{id}/link-existing  🔐  { mrn, relation }

# Vitals
GET    /patient/{id}/vitals   🔐  ?metric=&from=&to=
POST   /patient/{id}/vitals   🔐  { metric, value_numeric, unit, recorded_at, source, notes }
DELETE /patient/vitals/{id}   🔐(admin)
GET    /patient/{id}/vitals/chart  🔐  ?metric=&period=

# Files
GET    /patient/{id}/files    🔐  ?category=&page=
POST   /patient/{id}/files    🔐  multipart
GET    /patient/files/{id}    🔐
GET    /patient/files/{id}/download  🔐  → 302
DELETE /patient/files/{id}    🔐
PUT    /patient/files/{id}    🔐  metadata only

# Doctor's patient search
GET    /doctor/patients       🔐(doctor)  ?q=&limit=

# Admin
GET    /admin/patients        🔐(admin)  ?q=&mrn=&page=
GET    /admin/patients/{id}   🔐(admin)
POST   /admin/patients        🔐(admin)
PUT    /admin/patients/{id}   🔐(admin)
DELETE /admin/patients/{id}   🔐(admin)
POST   /admin/patients/{id}/merge  🔐(admin)  { keep_id, merge_id }
```

---

## 7. Appointments

(See 07-MODULE-APPOINTMENTS for full spec; the URL list is reproduced for completeness.)

### 7.1 Public
```
GET    /public/appointments/lookup/{uuid}  🔓
GET    /public/clinics/{id}/queue          🔓
```

### 7.2 Patient
```
GET    /patient/appointments           🔐  ?filter=upcoming|past|cancelled&page=
GET    /patient/appointments/{id}      🔐
POST   /patient/appointments           🔐  (book)
POST   /patient/appointments/{id}/cancel-request  🔐  { reason }
POST   /patient/appointments/{id}/check-in        🔐  (self QR)
POST   /patient/appointments/{id}/reschedule      🔐  { new_slot_id, reason? }
GET    /patient/appointments/{id}/payment-status  🔐
GET    /patient/appointments/{id}/receipt         🔐  → PDF
POST   /patient/appointments/{id}/review          🔐
```

### 7.3 Doctor
```
GET    /doctor/dashboard               🔐(doctor)  ?date=
GET    /doctor/appointments            🔐(doctor)  ?date=&status=
GET    /doctor/appointments/{id}       🔐(doctor)
POST   /doctor/appointments/{id}/start 🔐(doctor)
POST   /doctor/appointments/{id}/complete  🔐(doctor)
POST   /doctor/appointments/{id}/notes      🔐(doctor)
GET    /doctor/appointments/{id}/patient    🔐(doctor)
GET    /doctor/schedule                 🔐(doctor)
```

### 7.4 Admin / Front-Desk
```
GET    /admin/appointments             🔐(staff)  full filter
GET    /admin/appointments/calendar    🔐(staff)  ?from=&to=&doctor_id=
GET    /admin/appointments/{id}        🔐(staff)
POST   /admin/appointments             🔐(staff)
POST   /admin/appointments/{id}/check-in  🔐(staff)  manual
POST   /admin/appointments/{id}/call      🔐(staff)  { consultation_room? }
POST   /admin/appointments/{id}/skip      🔐(staff)
POST   /admin/appointments/{id}/cancel    🔐(staff)  { reason }
POST   /admin/appointments/{id}/mark-no-show  🔐(staff)
POST   /admin/appointments/{id}/mark-arrived  🔐(staff)  alias
POST   /admin/appointments/{id}/reschedule    🔐(staff)  { new_slot_id }
POST   /admin/appointments/{id}/refund        🔐(staff)  { amount, reason }
GET    /admin/appointments/{id}/status-log  🔐(staff)
GET    /admin/appointments/{id}/audit-log   🔐(staff)
POST   /admin/appointments/bulk-cancel   🔐(staff)  { doctor_id, date, reason }

# Cancellations
GET    /admin/cancellations            🔐(staff)  ?status=pending&page=
GET    /admin/cancellations/{id}       🔐(staff)
POST   /admin/cancellations/{id}/approve  🔐(staff)  { refund: bool, notes? }
POST   /admin/cancellations/{id}/reject   🔐(staff)  { notes }
```

---

## 8. Prescriptions

```
# Medicines master
GET    /medicines/search               🔐  ?q=&limit=
GET    /admin/medicines                🔐(admin)  ?q=&form=
POST   /admin/medicines                🔐(admin)
PUT    /admin/medicines/{id}           🔐(admin)
DELETE /admin/medicines/{id}           🔐(admin)
POST   /admin/medicines/import         🔐(admin)  multipart

# Patient
GET    /patient/prescriptions          🔐  ?filter=&page=
GET    /patient/prescriptions/{id}     🔐
GET    /patient/prescriptions/{id}/pdf 🔐
POST   /patient/prescriptions/{id}/share     🔐  { hours_valid?: 72 }
DELETE /patient/prescriptions/{id}/share     🔐
GET    /public/rx/{shared_token}       🔓  (no auth)

# Doctor
GET    /doctor/prescriptions           🔐(doctor)  ?patient_id=&from=&to=
POST   /doctor/prescriptions           🔐(doctor)  from appointment or standalone
GET    /doctor/prescriptions/{id}      🔐(doctor)
PUT    /doctor/prescriptions/{id}      🔐(doctor)  within 24h
POST   /doctor/prescriptions/{id}/sign       🔐(doctor)  { signature_data_url }
POST   /doctor/prescriptions/{id}/unlock     🔐(doctor)  { reason }
POST   /doctor/prescriptions/{id}/amend      🔐(doctor)
POST   /doctor/prescriptions/{id}/lines      🔐(doctor)  add
PUT    /doctor/prescriptions/{id}/lines/{lid}  🔐(doctor)
DELETE /doctor/prescriptions/{id}/lines/{lid}  🔐(doctor)
POST   /doctor/prescriptions/{id}/lines/reorder  🔐(doctor)  { ids: [...] }
POST   /doctor/prescriptions/{id}/apply-template  🔐(doctor)  { template_id }

# Templates
GET    /doctor/templates               🔐(doctor)
POST   /doctor/templates               🔐(doctor)
PUT    /doctor/templates/{id}          🔐(doctor)
DELETE /doctor/templates/{id}          🔐(doctor)

# Admin
GET    /admin/prescriptions            🔐(admin)  full filter
```

---

## 9. Billing

```
# Invoices
GET    /admin/invoices                 🔐(admin)  ?status=&from=&to=&patient_id=&page=
GET    /admin/invoices/{id}            🔐(admin)
POST   /admin/invoices                 🔐(admin)  manual
PUT    /admin/invoices/{id}            🔐(admin)  draft/unpaid only
DELETE /admin/invoices/{id}            🔐(admin)  draft only
POST   /admin/invoices/{id}/void       🔐(admin)  { reason }
POST   /admin/invoices/{id}/send       🔐(admin)  email PDF
POST   /admin/invoices/{id}/record-payment  🔐(admin)  cash/manual
GET    /admin/invoices/{id}/pdf        🔐(admin)

GET    /patient/invoices               🔐  ?status=&page=
GET    /patient/invoices/{id}          🔐
GET    /patient/invoices/{id}/pdf      🔐

# Transactions
GET    /admin/transactions             🔐(admin)  ?type=&method=&status=&from=&to=&page=
GET    /admin/transactions/{id}        🔐(admin)
GET    /patient/transactions           🔐  ?from=&to=&page=

# Coupons
GET    /admin/coupons                  🔐(admin)  ?is_active=&page=
POST   /admin/coupons                  🔐(admin)
GET    /admin/coupons/{id}             🔐(admin)
PUT    /admin/coupons/{id}             🔐(admin)
DELETE /admin/coupons/{id}             🔐(admin)
GET    /admin/coupons/{id}/usages      🔐(admin)
POST   /public/coupons/validate        🔓  { code, invoice_subtotal, service_ids? }

# Taxes
GET    /admin/taxes                    🔐(admin)
POST   /admin/taxes                    🔐(admin)
PUT    /admin/taxes/{id}               🔐(admin)
DELETE /admin/taxes/{id}               🔐(admin)

# Refunds
GET    /admin/refunds                  🔐(admin)  ?status=&from=&to=&page=
GET    /admin/refunds/{id}             🔐(admin)
POST   /admin/refunds                  🔐(admin)  { transaction_id, amount, reason }
POST   /admin/refunds/{id}/retry       🔐(admin)
```

---

## 10. Payments

```
# Mobile checkout init
POST   /payments/razorpay/order        🔐  { invoice_id, consultation_type? }
POST   /payments/stripe/intent         🔐  { invoice_id, ... }
POST   /payments/paystack/initialize   🔐  { invoice_id, callback_url }

# Status
GET    /payments/{payment_id}          🔐
GET    /payments/{payment_id}/status   🔐
GET    /payments/lookup                🔐(staff)  ?order_id=&gateway=

# Refund (admin)
POST   /payments/{payment_id}/refund   🔐(admin)  { amount, reason }

# Webhooks (no auth, signature-verified)
POST   /webhooks/razorpay              🔓
POST   /webhooks/stripe                🔓
POST   /webhooks/paystack              🔓

# Admin
GET    /admin/payments                 🔐(admin)  full filter
GET    /admin/payments/{id}            🔐(admin)
POST   /admin/payments/{id}/sync       🔐(admin)
```

---

## 11. Notifications

```
# User
GET    /me/notifications               🔐  ?unread=&page=
POST   /me/notifications/{id}/read     🔐
POST   /me/notifications/read-all      🔐
GET    /me/notifications/unread-count  🔐
GET    /me/notification-preferences    🔐
PUT    /me/notification-preferences    🔐  { preferences: [...] }
POST   /me/unsubscribe/{token}         🔓

# Internal
POST   /internal/notifications/send        (service-to-service)
POST   /internal/notifications/send-to-many

# Admin
GET    /admin/notifications            🔐(admin)  full filter
GET    /admin/notifications/{id}       🔐(admin)

GET    /admin/notification-templates    🔐(admin)
POST   /admin/notification-templates   🔐(admin)
PUT    /admin/notification-templates/{id}  🔐(admin)
POST   /admin/notification-templates/{id}/preview   🔐(admin)  { sample_data }
GET    /admin/notification-templates/{id}/versions  🔐(admin)
POST   /admin/notification-templates/{id}/rollback  🔐(admin)  { version_id }

GET    /admin/cron-jobs                🔐(admin)
POST   /admin/cron-jobs/{name}/run-now 🔐(admin)

GET    /admin/email-suppressions       🔐(admin)
DELETE /admin/email-suppressions/{id}  🔐(admin)
```

---

## 12. Engagement

### 12.1 Testimonials
```
GET    /public/testimonials             🔓
GET    /admin/testimonials             🔐(admin)
POST   /admin/testimonials             🔐(admin)
GET    /admin/testimonials/{id}        🔐(admin)
PUT    /admin/testimonials/{id}        🔐(admin)
DELETE /admin/testimonials/{id}        🔐(admin)
POST   /admin/testimonials/{id}/approve  🔐(admin)
POST   /admin/testimonials/{id}/unapprove 🔐(admin)
POST   /admin/testimonials/reorder       🔐(admin)
POST   /public/testimonials/submit       🔓  → goes to admin queue
```

### 12.2 Banners
```
GET    /public/banners                 🔓
GET    /public/banners/{id}/click      🔓  302 redirect, increments click
POST   /public/banners/{id}/impression 🔓  increments impression

GET    /admin/banners                  🔐(admin)
POST   /admin/banners                  🔐(admin)
GET    /admin/banners/{id}             🔐(admin)
PUT    /admin/banners/{id}             🔐(admin)
DELETE /admin/banners/{id}             🔐(admin)
POST   /admin/banners/reorder          🔐(admin)
```

### 12.3 Login Slides
```
GET    /public/login-slides            🔓  ?app_type=
GET    /admin/login-slides             🔐(admin)
POST   /admin/login-slides             🔐(admin)
PUT    /admin/login-slides/{id}        🔐(admin)
DELETE /admin/login-slides/{id}        🔐(admin)
POST   /admin/login-slides/reorder     🔐(admin)
```

### 12.4 Reviews
```
POST   /patient/appointments/{id}/review       🔐
PUT    /patient/reviews/{id}                    🔐  (within 24h)
DELETE /patient/reviews/{id}                    🔐  (within 24h)
POST   /doctor/reviews/{id}/reply               🔐(doctor)
PUT    /doctor/reviews/{id}/reply               🔐(doctor)
DELETE /doctor/reviews/{id}/reply               🔐(doctor)
GET    /public/doctors/{id}/reviews             🔓
GET    /public/doctors/{id}/reviews/histogram   🔓
GET    /admin/reviews                           🔐(admin)
POST   /admin/reviews/{id}/approve              🔐(admin)
POST   /admin/reviews/{id}/reject               🔐(admin)
POST   /admin/reviews/{id}/feature              🔐(admin)
```

### 12.5 Languages
```
GET    /public/languages               🔓
GET    /admin/languages                🔐(admin)
POST   /admin/languages                🔐(admin)
PUT    /admin/languages/{id}           🔐(admin)
DELETE /admin/languages/{id}           🔐(admin)
```

### 12.6 Currencies
```
GET    /public/currencies              🔓
GET    /admin/currencies               🔐(admin)
POST   /admin/currencies               🔐(admin)
PUT    /admin/currencies/{id}          🔐(admin)
DELETE /admin/currencies/{id}          🔐(admin)
```

### 12.7 Translations
```
GET    /admin/translations             🔐(admin)  ?group=&language_id=&q=
POST   /admin/translations             🔐(admin)  bulk
PUT    /admin/translations/{id}        🔐(admin)
DELETE /admin/translations/{id}        🔐(admin)
POST   /admin/translations/import      🔐(admin)  multipart JSON
GET    /admin/translations/export      🔐(admin)
```

---

## 13. Video Calls (Zoom)

```
POST   /video-calls                    🔐(doctor)  { appointment_id }
                                          → { meeting_id, join_url, host_url, password, start_url, start_time, duration }
GET    /video-calls/{id}               🔐
DELETE /video-calls/{id}               🔐(doctor)
POST   /video-calls/{id}/start         🔐(doctor)
POST   /video-calls/{id}/end           🔐(doctor)
GET    /video-calls/{id}/participants  🔐
POST   /webhooks/zoom                  🔓  (signature-verified)
```

---

## 14. Settings, Dashboard, Reports, Audit

### 14.1 Settings
```
GET    /admin/settings                 🔐(admin)  grouped
GET    /admin/settings/{group}        🔐(admin)  one tab
PUT    /admin/settings/{group}         🔐(admin)  bulk update
PUT    /admin/settings/key/{key}       🔐(admin)  single key
POST   /admin/settings/test-email      🔐(admin)  { to }
POST   /admin/settings/test-push       🔐(admin)  { fcm_token, template_key }
POST   /admin/settings/reset           🔐(super)  { group }
GET    /public/settings                🔓  is_public only
```

### 14.2 Dashboard
```
GET    /admin/dashboard                🔐(admin)  ?date=
GET    /admin/dashboard/charts         🔐(admin)  ?metric=&period=
GET    /doctor/dashboard               🔐(doctor)  ?date=
```

### 14.3 Reports
```
GET    /admin/reports/revenue          🔐(admin)  ?from=&to=&group_by=&clinic_id=&doctor_id=&format=
GET    /admin/reports/tax              🔐(admin)  ?from=&to=&tax_id=&format=
GET    /admin/reports/doctor-earnings  🔐(admin)  ?from=&to=&doctor_id=&format=
GET    /admin/reports/appointments     🔐(admin)  ?from=&to=&format=
GET    /admin/reports/patients         🔐(admin)  ?from=&to=&format=
GET    /admin/reports/transactions     🔐(admin)  ?from=&to=&gateway=&format=
GET    /admin/reports/refunds          🔐(admin)  ?from=&to=&format=
GET    /admin/reports/coupons          🔐(admin)  ?from=&to=&format=
GET    /admin/reports/cancellations    🔐(admin)  ?from=&to=&format=
GET    /admin/reports/reviews          🔐(admin)  ?from=&to=&format=

POST   /admin/reports/schedule         🔐(admin)  schedule recurring
GET    /admin/reports/schedules        🔐(admin)
PUT    /admin/reports/schedules/{id}   🔐(admin)
DELETE /admin/reports/schedules/{id}   🔐(admin)
```

### 14.4 Audit
```
GET    /admin/audit-logs               🔐(admin)  full filter
GET    /admin/audit-logs/{id}          🔐(admin)
GET    /admin/audit-logs/by-entity     🔐(admin)  ?type=&id=
GET    /admin/audit-logs/by-user       🔐(admin)  ?user_id=&from=&to=
GET    /admin/audit-logs/phi-access-report  🔐(admin)  ?patient_id=&from=&to=
GET    /admin/audit-logs/export        🔐(admin)  ?from=&to=&format=
```

---

## 15. V2 Stubs (all return 501 unless feature enabled)

```
# Pathology
GET    /admin/pathology/labs                🔐(admin)  → 501
POST   /admin/pathology/labs                🔐(admin)  → 501
... (40 endpoints)
GET    /admin/pathology/orders              🔐(admin)  → 501

# AI Chat
POST   /ai/symptom-check                    🔐  → 501

# Blogs
GET    /public/blog                         🔓  → 501
GET    /public/blog/{slug}                  🔓  → 501
... etc

# Subscriptions
GET    /admin/subscription-plans            🔐(admin)  → 501
... etc
```

---

## 16. Common Endpoints

```
GET    /me                                  🔐  (current user with roles+permissions)
GET    /me/features                         🔐  (which V2 features are enabled for this tenant)
GET    /health                              🔓  (liveness, returns 200)
GET    /health/ready                        🔓  (readiness, checks DB+Redis+queue)
GET    /version                             🔓  (API version, build hash)
GET    /openapi.json                        🔓  (OpenAPI 3.1 spec)
GET    /docs                                🔓  (Swagger UI)
```

---

## 17. Webhooks (Server-to-Server)

All webhook endpoints are `🔓` (no Bearer auth), but **signature-verified** via per-gateway HMAC. See module docs for signature details.

```
POST   /webhooks/razorpay                   (payment.captured, payment.failed, refund.processed, order.paid)
POST   /webhooks/stripe                     (payment_intent.succeeded, payment_intent.payment_failed, charge.refunded)
POST   /webhooks/paystack                   (charge.success, charge.failure, transfer.success, transfer.failure, refund.processed)
POST   /webhooks/zoom                       (meeting.started, meeting.ended, participant.joined, participant.left)
POST   /webhooks/firebase                   (auth events, V2 for anomaly detection)
POST   /webhooks/mailgun                    (bounce, complaint, unsubscribe — updates email_suppression_list)
POST   /webhooks/ses                        (bounce, complaint)
```

---

## 18. OpenAPI Specification

The full OpenAPI 3.1 spec is auto-generated from `app/Modules/*/Http/Controllers/*` annotations (using `zircote/swagger-php`) and served at:
- `GET /openapi.json` — machine-readable
- `GET /docs` — Swagger UI

Generated on every build. Published to internal API portal (Backstage / Stoplight).

---

## 19. Postman / Insomnia Collection

A `medicare.postman_collection.json` ships with the repo, generated from the OpenAPI spec. Includes:
- All endpoints pre-configured with example payloads.
- Environment templates for `local`, `staging`, `prod`.
- Auth helper scripts (login → set token → chain requests).

---

## 20. Versioning

| API version | Status | Sunset date |
|---|---|---|
| v1 | Current | TBD |
| v2 | Future (12+ months from v1 GA) | TBD |

When v2 is released, v1 endpoints keep working. The response includes `Deprecation: true` and `Sunset: <date>` headers. Mobile clients must migrate within the sunset window.

Clients can pin to a specific version:
- URL path (`/api/v1/...`) — preferred.
- Header `Accept: application/vnd.medicare.v1+json` — alternative.

---

## 21. Endpoint Count Summary

| Module | Endpoints | Auth Required |
|---|---|---|
| Auth | 18 | 14 |
| Tenancy | 10 | 9 |
| Clinic Structure | 30 | 16 |
| Doctors | 36 | 21 |
| Patients | 28 | 22 |
| Appointments | 38 | 35 |
| Prescriptions | 22 | 19 |
| Billing | 32 | 25 |
| Payments | 14 | 10 |
| Notifications | 16 | 12 |
| Engagement | 38 | 22 |
| Video | 7 | 6 |
| Settings/Reports/Audit | 22 | 18 |
| V2 Stubs | ~80 | all 501 |
| Common + Webhooks | 14 | 6 |
| **Total** | **~400** | |

---

## 22. Error Reference (Most Common)

| HTTP | type slug | When |
|---|---|---|
| 400 | `validation` | Form validation failed |
| 400 | `slot_unavailable` | Slot taken or expired |
| 400 | `invalid_state_transition` | State machine violation |
| 401 | `unauthenticated` | No/bad token |
| 403 | `forbidden` | Missing permission |
| 404 | `not_found` | Resource doesn't exist or is cross-tenant |
| 409 | `duplicate` | Unique constraint (e.g., second review on same appt) |
| 409 | `version_conflict` | Module version too old |
| 422 | `payment_required` | Invoice amount doesn't match |
| 423 | `tenant_suspended` | Tenant is suspended |
| 429 | `rate_limited` | Too many requests |
| 501 | `not_implemented` | V2 feature stub |
| 502 | `gateway_error` | Upstream gateway failed |
| 503 | `maintenance` | Scheduled maintenance |

All error responses include `trace_id` for support lookup in Sentry.

---

## 23. V2 API Reference

> V1 ships ~400 endpoints. V2 adds ~250 more, taking the platform to ~650 endpoints by V3.

### 23.1 V2 endpoint catalog (full list)

#### Auth V2 (ABDM, OIDC, MFA, Passkey) — ~30 endpoints

```
POST   /api/v2/auth/abha/initiate
POST   /api/v2/auth/abha/verify
POST   /api/v2/auth/abha/callback
DELETE /api/v2/auth/abha
GET    /api/v2/auth/identities
DELETE /api/v2/auth/identities/{id}
POST   /api/v2/auth/identities/link
POST   /api/v2/auth/oidc/{slug}/initiate
GET    /api/v2/auth/oidc/{slug}/callback
POST   /api/v2/auth/mfa/enroll
GET    /api/v2/auth/mfa/credentials
DELETE /api/v2/auth/mfa/credentials/{id}
POST   /api/v2/auth/mfa/verify
POST   /api/v2/auth/mfa/backup-codes/regenerate
POST   /api/v2/auth/passkey/register-challenge
POST   /api/v2/auth/passkey/register
POST   /api/v2/auth/passkey/auth-challenge
POST   /api/v2/auth/passkey/authenticate
POST   /api/v2/auth/sms/otp/send
POST   /api/v2/auth/sms/otp/verify
GET    /api/v2/auth/sessions
DELETE /api/v2/auth/sessions/{id}                            # force logout a device
```

#### Tenancy V2 (custom domain, exports) — ~10 endpoints

```
GET    /api/v2/tenants/me/custom-domains
POST   /api/v2/tenants/me/custom-domains
GET    /api/v2/tenants/me/custom-domains/{id}/verify
DELETE /api/v2/tenants/me/custom-domains/{id}
POST   /api/v2/tenants/me/data-exports
GET    /api/v2/tenants/me/data-exports
GET    /api/v2/tenants/me/data-exports/{id}
DELETE /api/v2/tenants/me/data-exports/{id}
POST   /api/v2/tenants/me/reactivate
GET    /api/v2/tenants/me/usage
```

#### Resources V2 — ~15 endpoints

```
GET    /api/v2/clinics/{id}/resources
POST   /api/v2/clinics/{id}/resources
PATCH  /api/v2/clinics/{id}/resources/{rid}
DELETE /api/v2/clinics/{id}/resources/{rid}
GET    /api/v2/clinics/{id}/resources/{rid}/availability?from&to
GET    /api/v2/clinics/{id}/beds
GET    /api/v2/clinics/{id}/beds/availability
POST   /api/v2/beds/assignments
GET    /api/v2/beds/assignments/{id}
POST   /api/v2/beds/assignments/{id}/discharge
GET    /api/v2/clinics/{id}/ot-slots
POST   /api/v2/clinics/{id}/ot-slots
POST   /api/v2/ot/bookings
DELETE /api/v2/ot/bookings/{id}
GET    /api/v2/equipment
POST   /api/v2/equipment
PATCH  /api/v2/equipment/{id}
GET    /api/v2/equipment/maintenance-due
```

#### Doctors V2 (geo, AI rec, rotation) — ~10 endpoints

```
GET    /api/v2/public/doctors/nearby
GET    /api/v2/public/doctors/recommend
GET    /api/v2/public/doctors/{slug}
POST   /api/v2/doctors/{id}/payout-methods
GET    /api/v2/doctors/{id}/payouts
GET    /api/v2/doctors/{id}/payslips/{payout_id}
POST   /api/v2/doctors/{id}/rotation-groups
GET    /api/v2/doctors/{id}/rotation-groups
POST   /api/v2/doctor-rotations/{group_id}/regenerate
POST   /api/v2/doctors/{id}/ai/bio
POST   /api/v2/doctors/{id}/ai/review-reply
```

#### Patients V2 (EHR, attachments, ABHA) — ~35 endpoints

```
GET    /api/v2/patients/{id}/problem-list
POST   /api/v2/patients/{id}/problem-list
PATCH  /api/v2/patients/{id}/problem-list/{pid}
DELETE /api/v2/patients/{id}/problem-list/{pid}
GET    /api/v2/patients/{id}/family-history
POST   /api/v2/patients/{id}/family-history
DELETE /api/v2/patients/{id}/family-history/{id}
GET    /api/v2/patients/{id}/lab-results
GET    /api/v2/patients/{id}/lab-results/trend
GET    /api/v2/patients/{id}/vaccinations
POST   /api/v2/patients/{id}/vaccinations
DELETE /api/v2/patients/{id}/vaccinations/{id}
GET    /api/v2/patients/{id}/surgical-history
POST   /api/v2/patients/{id}/surgical-history
GET    /api/v2/patients/{id}/encounters
GET    /api/v2/patients/{id}/encounters/{eid}
POST   /api/v2/patients/{id}/encounters/{eid}/sign
POST   /api/v2/patients/{id}/attachments
GET    /api/v2/patients/{id}/attachments
DELETE /api/v2/patients/{id}/attachments/{aid}
GET    /api/v2/patients/{id}/attachments/{aid}/download
POST   /api/v2/patients/me/link-abha
POST   /api/v2/patients/me/link-abha/verify
POST   /api/v2/patients/me/link-abha/callback
DELETE /api/v2/patients/me/link-abha
POST   /api/v2/encounters/{id}/scribe/audio
GET    /api/v2/encounters/{id}/scribe/status
GET    /api/v2/patients/{id}/share-record                      # generate signed URL
POST   /api/v2/patients/{id}/family-link                        # link dependent
GET    /api/v2/patients/{id}/dependents
GET    /api/v2/patients/{id}/timeline
GET    /api/v2/patients/{id}/ai/explain-lab                     # AI plain-language explanation
GET    /api/v2/patients/me/insurance-cards
POST   /api/v2/patients/{id}/gdpr-delete
GET    /api/v2/patients/{id}/gdpr-export
```

#### Appointments V2 (recurring, group, video, waitlist) — ~20 endpoints

```
POST   /api/v2/appointments/recurring
GET    /api/v2/appointment-series
GET    /api/v2/appointment-series/{id}
PATCH  /api/v2/appointment-series/{id}
DELETE /api/v2/appointment-series/{id}
POST   /api/v2/appointment-series/{id}/pause
POST   /api/v2/appointment-series/{id}/resume
POST   /api/v2/group-appointments
GET    /api/v2/group-appointments/{id}
POST   /api/v2/group-appointments/{id}/join
DELETE /api/v2/group-appointments/{id}/leave
POST   /api/v2/appointments/{id}/video-room
GET    /api/v2/video-rooms/{id}
GET    /api/v2/video-rooms/{id}/token
POST   /api/v2/video-rooms/{id}/end
GET    /api/v2/video-rooms/{id}/recording
POST   /api/v2/waitlist
GET    /api/v2/waitlist
DELETE /api/v2/waitlist/{id}
POST   /api/v2/waitlist/{id}/accept
GET    /api/v2/refund-policies
POST   /api/v2/refund-policies
```

#### Prescriptions V2 (e-Rx, drug interactions, lab) — ~20 endpoints

```
POST   /api/v2/prescriptions/{id}/sign
GET    /api/v2/prescriptions/{id}/fhir-bundle
POST   /api/v2/prescriptions/{id}/upload-to-abdm
GET    /api/v2/drug-interactions
POST   /api/v2/prescriptions/{id}/lab-orders
GET    /api/v2/lab-orders
GET    /api/v2/lab-orders/{id}
PATCH  /api/v2/lab-orders/{id}
POST   /api/v2/lab-orders/{id}/cancel
POST   /api/v2/lab-orders/{id}/samples
POST   /api/v2/lab-orders/{id}/results
GET    /api/v2/lab-orders/{id}/report
GET    /api/v2/lab-partners
POST   /api/v2/prescriptions/{id}/refills
POST   /api/v2/prescription-refills
POST   /api/v2/prescription-refills/{id}/approve
POST   /api/v2/prescription-refills/{id}/deny
GET    /api/v2/prescription-templates/ai-suggest
```

#### Billing V2 (subscriptions, insurance, packages) — ~30 endpoints

```
GET    /api/v2/subscription-plans
POST   /api/v2/subscriptions
GET    /api/v2/subscriptions/me
POST   /api/v2/subscriptions/{id}/cancel
POST   /api/v2/subscriptions/{id}/pause
POST   /api/v2/subscriptions/{id}/resume
POST   /api/v2/subscriptions/{id}/change-plan
GET    /api/v2/subscription-invoices
GET    /api/v2/packages
POST   /api/v2/packages
POST   /api/v2/patient-packages
GET    /api/v2/patient-packages
POST   /api/v2/insurance/payers
GET    /api/v2/insurance/payers
POST   /api/v2/insurance/policies
GET    /api/v2/insurance/policies
POST   /api/v2/insurance/preauth
GET    /api/v2/insurance/preauth
POST   /api/v2/insurance/claims
GET    /api/v2/insurance/claims
POST   /api/v2/insurance/claims/{id}/submit
POST   /api/v2/insurance/claims/{id}/appeal
GET    /api/v2/insurance/denials
POST   /api/v2/invoices/{id}/payment-plan
POST   /api/v2/invoices/{id}/dunning-test
POST   /api/v2/invoices/{id}/credit-note
GET    /api/v2/invoices/{id}/e-invoice
```

#### Payments V2 (subscriptions, BNPL, terminal, FX) — ~25 endpoints

```
POST   /api/v2/payment-methods
GET    /api/v2/payment-methods
DELETE /api/v2/payment-methods/{id}
POST   /api/v2/payments/subscriptions
GET    /api/v2/payments/subscriptions
POST   /api/v2/payments/subscriptions/{id}/cancel
POST   /api/v2/payments/bnpl/eligibility
POST   /api/v2/payments/bnpl/checkout
POST   /api/v2/payments/terminal/charge
GET    /api/v2/payments/terminal/readers
POST   /api/v2/payments/terminal/readers
DELETE /api/v2/payments/terminal/readers/{id}
GET    /api/v2/payments/fx
GET    /api/v2/payouts/runs
GET    /api/v2/payouts/runs/{id}
GET    /api/v2/payouts/runs/{id}/payslip/{doctor_id}
POST   /api/v2/payouts/runs/{id}/retry
GET    /api/v2/payout-accounts
POST   /api/v2/payout-accounts
DELETE /api/v2/payout-accounts/{id}
GET    /api/v2/payments/upi-autopay/mandates
POST   /api/v2/payments/upi-autopay/mandates
DELETE /api/v2/payments/upi-autopay/mandates/{id}
```

#### Notifications V2 (WhatsApp, SMS, chat) — ~25 endpoints

```
POST   /api/v2/notifications/whatsapp/send
GET    /api/v2/notifications/whatsapp/conversations
GET    /api/v2/notifications/whatsapp/conversations/{id}/messages
POST   /api/v2/notifications/whatsapp/inbound
POST   /api/v2/notifications/whatsapp/opt-in
POST   /api/v2/notifications/whatsapp/opt-out
GET    /api/v2/notifications/sms
POST   /api/v2/notifications/sms/inbound
POST   /api/v2/notifications/voice/call
GET    /api/v2/notifications/voice/calls
POST   /api/v2/notifications/voice/inbound
POST   /api/v2/chat/chats
GET    /api/v2/chat/chats
GET    /api/v2/chat/chats/{id}
PATCH  /api/v2/chat/chats/{id}
POST   /api/v2/chat/chats/{id}/messages
GET    /api/v2/chat/chats/{id}/messages
PATCH  /api/v2/chat/chats/{id}/messages/{mid}
DELETE /api/v2/chat/chats/{id}/messages/{mid}
POST   /api/v2/chat/chats/{id}/typing
GET    /api/v2/push/categories
POST   /api/v2/push/categories/{slug}/subscribe
DELETE /api/v2/push/categories/{slug}/subscribe
POST   /api/v2/notifications/ai/rewrite
```

#### Engagement V2 (blogs, feed) — ~25 endpoints

```
GET    /api/v2/blog/categories
POST   /api/v2/blog/categories
GET    /api/v2/blog/posts
GET    /api/v2/blog/posts/{slug}
POST   /api/v2/blog/posts
PATCH  /api/v2/blog/posts/{id}
DELETE /api/v2/blog/posts/{id}
POST   /api/v2/blog/posts/{id}/submit-review
POST   /api/v2/blog/posts/{id}/approve
POST   /api/v2/blog/posts/{id}/reject
POST   /api/v2/blog/posts/{id}/revisions
GET    /api/v2/blog/posts/{id}/revisions
POST   /api/v2/blog/posts/{id}/revert/{revision_id}
POST   /api/v2/blog/posts/{id}/ai/generate
POST   /api/v2/blog/posts/{id}/ai/rewrite
POST   /api/v2/blog/posts/{id}/ai/seo
POST   /api/v2/blog/posts/{id}/reactions
GET    /api/v2/blog/posts/{id}/comments
POST   /api/v2/blog/posts/{id}/comments
PATCH  /api/v2/blog/comments/{id}
DELETE /api/v2/blog/comments/{id}
GET    /api/v2/feed/me
GET    /api/v2/doctors/{slug}
```

#### AI V2 (symptom, scribe, rec) — ~10 endpoints

```
POST   /api/v2/ai/symptom-checker
GET    /api/v2/ai/symptom-checker/{session_id}
POST   /api/v2/ai/scribe/start
POST   /api/v2/ai/scribe/audio
GET    /api/v2/ai/scribe/{session_id}
POST   /api/v2/ai/scribe/{session_id}/finalize
GET    /api/v2/ai/conversations
GET    /api/v2/ai/conversations/{id}
GET    /api/v2/ai/usage
POST   /api/v2/ai/feedback
```

#### Telemedicine V2 — ~10 endpoints

```
POST   /api/v2/video/rooms
GET    /api/v2/video/rooms
GET    /api/v2/video/rooms/{id}
POST   /api/v2/video/rooms/{id}/participants
DELETE /api/v2/video/rooms/{id}/participants/{pid}
POST   /api/v2/video/rooms/{id}/recordings/start
POST   /api/v2/video/rooms/{id}/recordings/stop
GET    /api/v2/video/rooms/{id}/recordings
GET    /api/v2/video/rooms/{id}/recordings/{rid}/download
```

#### Lab V2 — ~15 endpoints

```
GET    /api/v2/lab/orders
GET    /api/v2/lab/orders/{id}
POST   /api/v2/lab/orders
PATCH  /api/v2/lab/orders/{id}
POST   /api/v2/lab/orders/{id}/cancel
GET    /api/v2/lab/orders/{id}/results
POST   /api/v2/lab/orders/{id}/samples
GET    /api/v2/lab/orders/{id}/samples
POST   /api/v2/lab/results/{id}/verify
GET    /api/v2/lab/results
GET    /api/v2/lab/panels
POST   /api/v2/lab/panels
GET    /api/v2/lab/partners
POST   /api/v2/lab/partners/{id}/sync
GET    /api/v2/lab/orders/{id}/tracking
```

#### Plugins V2 — ~15 endpoints

```
GET    /api/v2/plugins
GET    /api/v2/plugins/{slug}
POST   /api/v2/plugins/{slug}/install
DELETE /api/v2/plugins/{slug}/uninstall
POST   /api/v2/plugins/{slug}/enable
POST   /api/v2/plugins/{slug}/disable
POST   /api/v2/plugins/{slug}/upgrade
GET    /api/v2/plugins/{slug}/settings
PATCH  /api/v2/plugins/{slug}/settings
GET    /api/v2/plugins/{slug}/permissions
POST   /api/v2/plugins/{slug}/permissions
GET    /api/v2/plugin-licenses/me
GET    /api/v2/plugin-marketplace
GET    /api/v2/plugin-marketplace/{slug}
POST   /api/v2/plugin-marketplace/{slug}/purchase
```

#### Features / Reports V2 — ~15 endpoints

```
GET    /api/v2/features
POST   /api/v2/features/{slug}/enable
POST   /api/v2/features/{slug}/disable
GET    /api/v2/reports/definitions
POST   /api/v2/reports/definitions
PATCH  /api/v2/reports/definitions/{id}
DELETE /api/v2/reports/definitions/{id}
POST   /api/v2/reports/definitions/{id}/run
GET    /api/v2/reports/runs
GET    /api/v2/reports/runs/{id}
GET    /api/v2/reports/runs/{id}/download
GET    /api/v2/ai/usage
POST   /api/v2/ai/usage/budget
GET    /api/v2/audit-logs
GET    /api/v2/audit-logs/{id}
GET    /api/v2/data-access-log
```

#### HL7 v2 V3 (ADT/ORU, internal + admin) — ~10 endpoints

```
# Internal (Mirth sidecar → API)
POST   /api/internal/hl7v2/receive                    # service-to-service, mTLS, payload = parsed JSON
POST   /api/internal/hl7v2/{control_id}/ack            # Mirth sends HL7 ACK back

# Admin (tenant settings)
GET    /api/v3/integrations/hl7v2                     # current config: enabled, sending_facility, partner cert fp
PATCH  /api/v3/integrations/hl7v2                     # update config
GET    /api/v3/integrations/hl7v2/messages            # paginated list of received messages
GET    /api/v3/integrations/hl7v2/messages/{id}       # single message (with raw + parsed)
POST   /api/v3/integrations/hl7v2/messages/{id}/replay  # re-process a failed/quarantined message
GET    /api/v3/integrations/hl7v2/quarantine          # paginated list of quarantined messages
POST   /api/v3/integrations/hl7v2/quarantine/{id}/reject  # mark as rejected, do not retry
GET    /api/v3/integrations/hl7v2/stats               # counters: received/processed/quarantined, last 24h
```

#### FHIR R4 V2 (read-only) — `fhir-r4-read` (V1.2, MML 3) — deep-dive

**1. Overview.** V1.2 ships a **read-only FHIR R4 endpoint** at `/api/v2/fhir/...`, exposed by the `medicare-fhir-gateway` sidecar (Node/HAPI-FHIR, per `00-Architecture §15.2`). The endpoint implements the standard FHIR R4 read + search semantics for the four most commonly consumed resources: `Patient`, `Practitioner`, `Encounter`, `Observation`, plus a few higher-value extras (`MedicationRequest`, `AllergyIntolerance`, `Condition`, `DiagnosticReport`, `Procedure`, `Immunization`, `Coverage`). The intended consumer is an **external system** (a partner HIS, a research dataset, a public-health registry) that wants to query Medicare's data via a standards-based API. The endpoint is **read-only** in V1.2; FHIR R4 **write** is the V3 module `fhir-r4-write` (see `19-Integrations §15.10a` for that deep-dive). The endpoint uses **OAuth 2.0 client_credentials** (not SMART-on-FHIR launch; that's V3 write). MML 3 GA: the 10 endpoints above, the 4 extras, the capability statement, the auth flow, the rate limit, the audit, the SLA. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.2 lane module.

**2. The `medicare-fhir-gateway` sidecar.** HAPI-FHIR (Java) is the de-facto FHIR R4 server. The sidecar:
- Listens on `:8080` (internal, mTLS only).
- Holds a read-only MySQL view over the same database the API uses (the `medicare_readonly` role; per `00-Architecture §15.2`).
- Implements the FHIR R4 REST API per the spec (`[base]/[type]/[id]` for read; `[base]/[type]?params` for search).
- Holds a small in-memory cache of `Patient`, `Practitioner`, `Encounter` for hot reads (5-min TTL); the cache is invalidated on every API write that touches the resource (the API publishes a `fhir.invalidate.{tenant}.{resource_type}.{id}` Redis event; the sidecar subscribes).
- Returns the FHIR R4 JSON resource. The response is a proper FHIR `Bundle` for searches (with `link` relations for next/prev/self) and a single resource for reads.

**3. Authentication.** OAuth 2.0 client_credentials (per the FHIR spec, "SMART-on-FHIR backend services"). The external system:
1. Registers as a "FHIR consumer" via `POST /api/v2/admin/fhir/consumers` (admin endpoint). Receives `client_id` and `client_secret` (the secret is hashed in our DB; the plaintext is shown **once** at registration).
2. Posts to `POST /oauth2/token` with `grant_type=client_credentials&client_id=...&client_secret=...&scope=patient/Patient.read+patient/Encounter.read`. The token endpoint returns `{ access_token, token_type: 'Bearer', expires_in: 3600, scope }`. The token is a JWT signed with our platform key; the `iss` is `medicare-fhir-gateway`, the `sub` is the `client_id`, the `aud` is `fhir-r4`, the `scope` is the granted scopes.
3. The access token is sent as `Authorization: Bearer ...` on every FHIR request. The sidecar validates the token (signature, exp, aud) and enforces the scopes (e.g., a token with only `patient/Patient.read` cannot read `Observation`).

**4. Scopes.** Per SMART-on-FHIR scope syntax:
- `patient/Patient.read` — read Patient resources.
- `patient/Practitioner.read` — read Practitioner resources.
- `patient/Encounter.read` — read Encounter resources (visits).
- `patient/Observation.read` — read Observation resources (vitals, lab results).
- `patient/MedicationRequest.read` — read prescriptions.
- `patient/AllergyIntolerance.read` — read allergies.
- `patient/Condition.read` — read problem list.
- `patient/DiagnosticReport.read` — read lab reports.
- `patient/Procedure.read` — read procedures / surgical history.
- `patient/Immunization.read` — read vaccinations.
- `patient/Coverage.read` — read insurance coverage.
- `user/Patient.read`, `user/Encounter.read`, etc. — same as `patient/...` in V1.2 (no user-level access; the sidecar is tenant-scoped). V3 write adds real user-level (SMART launch).
- `system/Patient.read`, etc. — tenant-wide (admin only).

**5. Capability statement.** `GET /api/v2/fhir/metadata` returns a FHIR `CapabilityStatement` describing the resources, interactions, search params, and formats supported. The statement is built from a static template + a runtime-generated list of search params per resource. Example:
```json
{
  "resourceType": "CapabilityStatement",
  "status": "active",
  "date": "2026-06-01",
  "publisher": "Medicare",
  "kind": "instance",
  "software": { "name": "medicare-fhir-gateway", "version": "1.2.0" },
  "fhirVersion": "4.0.1",
  "format": ["json"],
  "rest": [{
    "mode": "server",
    "security": { "service": [{ "coding": [{ "code": "OAuth" }] }] },
    "resource": [
      { "type": "Patient", "interaction": [{"code": "read"}, {"code": "search-type"}], "searchParam": [{"name": "_id", "type": "token"}, {"name": "identifier", "type": "token"}, {"name": "name", "type": "string"}, {"name": "birthdate", "type": "date"}, {"name": "gender", "type": "token"}] },
      { "type": "Encounter", "interaction": [{"code": "read"}, {"code": "search-type"}], "searchParam": [{"name": "patient", "type": "reference"}, {"name": "date", "type": "date"}, {"name": "status", "type": "token"}] }
    ]
  }]
}
```

**6. Search.** FHIR search params translate to a SQL query against the read-only view. Examples:
- `GET /api/v2/fhir/Patient?name=John&birthdate=1990-01-01` → `SELECT * FROM patients WHERE tenant_id=? AND name ILIKE '%John%' AND dob='1990-01-01' LIMIT 20`.
- `GET /api/v2/fhir/Encounter?patient=Patient/abc&date=ge2026-01-01` → visits for that patient since Jan 1, 2026.
- `GET /api/v2/fhir/Observation?patient=Patient/abc&category=vital-signs&date=ge2026-01-01` → vitals for that patient since Jan 1, 2026.
- `_count=20` (max 100) controls page size. `_offset=20` controls offset. The response is a `Bundle` of type `searchset` with `total`, `link` (self/next/prev).

**7. Rate limits.** Per the consumer's tier:
- **Standard:** 60 req/min, 10k req/day. Per-tenant.
- **Premium:** 600 req/min, 100k req/day. Per-tenant.
- Rate-limit headers: `X-RateLimit-Limit`, `X-RateLimit-Remaining`, `X-RateLimit-Reset`, `Retry-After` (on 429).
- Bulk export (`/api/v2/fhir/$export`) is rate-limited at 1/day/tenant and is **Premium only**.

**8. Audit.** Every read and every search is logged in `audit_logs` with `action='fhir.read'` (or `fhir.search`), `resource_type`, `resource_id` (or `null` for search), `consumer_client_id`, `ip`, `response_size_bytes`, `latency_ms`. The audit is queryable by the tenant admin (per `20-Security §11.4`) and is part of the SOC 2 CC7.2 control.

**9. SLA.** p99 latency: read < 100ms, search < 500ms (with a 20-resource page). Bulk export: < 1 hour for 1M resources (Premium). Uptime: 99.9% (per `21-Deployment §11.2` V2 SLA).

**10. Acceptance Criteria.**
- [ ] All 10 endpoints + 4 extras return valid FHIR R4 resources (validate against the official FHIR R4 schema in CI).
- [ ] `GET /api/v2/fhir/metadata` returns a valid `CapabilityStatement`.
- [ ] OAuth 2.0 client_credentials flow works end-to-end; tokens are JWT-signed; scopes are enforced.
- [ ] Rate limits are enforced; 429 returns `Retry-After`.
- [ ] Every read and search is logged in `audit_logs` with the right `action` and the consumer's `client_id`.
- [ ] Search params translate correctly; `_count` and `_offset` work; `Bundle` has correct `link` relations.
- [ ] p99 of a single read is < 100ms; of a 20-resource search is < 500ms.
- [ ] Bulk export is Premium-only; rate-limited at 1/day/tenant; produces a valid NDJSON file.
- [ ] The read-only DB role prevents writes (the sidecar's DB user has no INSERT/UPDATE/DELETE grants; an attempted write from the sidecar fails fast at the DB layer).
- [ ] The capability statement is updated whenever a new resource or search param is added.

**11. Real-World Test Scenarios.**
- *Registration:* a partner HIS calls `POST /api/v2/admin/fhir/consumers`; receives `client_id` + `client_secret`; saves the secret; tests with a `curl` POST to `/oauth2/token`; gets a JWT.
- *Single read:* the partner calls `GET /api/v2/fhir/Patient/abc` with the bearer; receives a valid FHIR `Patient` resource; the audit log shows `fhir.read` with `consumer_client_id` and `resource_id='abc'`.
- *Search:* the partner calls `GET /api/v2/fhir/Encounter?patient=Patient/abc&date=ge2026-01-01`; receives a `Bundle` of 5 Encounters; the audit log shows `fhir.search` with the params.
- *Scope denied:* the partner has a token with only `patient/Patient.read`; tries `GET /api/v2/fhir/Observation/...`; receives 403 with `OperationOutcome` `diagnostics: 'insufficient_scope'`.
- *Rate limit exceeded:* the partner hits 60 req/min; the next request returns 429 with `Retry-After: 30`; the partner backs off.
- *Bulk export:* the Premium partner calls `POST /api/v2/fhir/$export` with `{"resourceType": ["Patient", "Encounter"]}`; receives a `202 Accepted` with a `Content-Location: /api/v2/fhir/$export/{job_id}`; polls the job; when done, downloads the NDJSON file.
- *Schema validation:* the test suite validates every resource against the official FHIR R4 schema (XML Schema + JSON Schema); a non-conformant resource fails the CI build.

**12. What this module is NOT:**
- Not a write endpoint. FHIR R4 write is V3 (`fhir-r4-write`).
- Not a SMART-on-FHIR launch. SMART launch contexts (standalone, EHR-launch) are V3 write.
- Not a FHIR R5 server. R4 only; R5 is post-V3.
- Not a CDS Hooks endpoint. CDS Hooks is a separate spec and is not in scope.
- Not a free public API. The consumer must register; the tenant admin approves.

**13. Dev Workflow.** `php artisan fhir:register-consumer --tenant=...` registers a test consumer. `php artisan fhir:test --tenant=... --resource=Patient --id=...` runs a smoke test. `php artisan fhir:bulk-export --tenant=...` triggers a bulk export. Code: `app-modules/fhir-r4-read/` (Laravel) + `services/fhir-gateway/` (Node/HAPI-FHIR); tests: `app-modules/fhir-r4-read/tests/Feature/{Auth,Read,Search,Scope,RateLimit,Audit,CapabilityStatement}Test.php` and `services/fhir-gateway/tests/contract/` (validates the response against the official FHIR R4 schema in CI). The dev setup runs the sidecar locally (per `23-Development-Setup §10.5`); a mock FHIR consumer is provided for local testing.

### 23.2 GraphQL gateway (V2, opt-in)

`POST /graphql` wraps the JSON:API for read queries. Powered by Apollo Server (Node) or Lighthouse (PHP, preferred).

```graphql
type Query {
  me: User
  doctors(cityId: ID, departmentId: ID, near: GeoInput, q: String): DoctorConnection!
  doctor(id: ID, slug: String): Doctor
  appointments(from: Date, to: Date, status: AppointmentStatus): AppointmentConnection!
  prescriptions: [Prescription!]!
  invoices(status: InvoiceStatus): [Invoice!]!
  patient(id: ID!): Patient
  encounters(patientId: ID!): [Encounter!]!
  labResults(patientId: ID!, from: Date, to: Date): [LabResult!]!
}

type Mutation {
  bookAppointment(input: BookAppointmentInput!): Appointment!
  cancelAppointment(id: ID!, reason: String): Appointment!
  postChatMessage(chatId: ID!, body: String!): ChatMessage!
  signPrescription(id: ID!): Prescription!
  createPreauth(input: PreauthInput!): PreauthRequest!
}
```

- Read-only in V2; mutations land in V3.
- Auth: `Authorization: Bearer {sanctum_token}`.
- Rate limit: 600 req/min/user (same as JSON:API).
- Subscriptions over WebSocket for live data (`graphql-ws`).

### 23.3 V2 webhook catalog

V1 has 6 webhooks (Razorpay, Stripe, Paystack, FCM, Email bounces, Scheduler). V2 adds:

```
# WhatsApp
whatsapp.message.status
whatsapp.message.inbound
whatsapp.template.approved
whatsapp.template.rejected

# SMS
sms.message.status
sms.message.inbound

# Voice
voice.call.status
voice.call.recording.ready
voice.call.transcript.ready

# Lab partners
lab.order.acknowledged
lab.sample.collected
lab.result.ready
lab.report.ready

# Insurance payers
insurance.preauth.approved
insurance.preauth.denied
insurance.claim.acknowledged
insurance.claim.paid
insurance.claim.denied

# Subscriptions
subscription.activated
subscription.renewed
subscription.payment_failed
subscription.cancelled
subscription.dunning.required

# Telemedicine
video.room.started
video.room.ended
video.recording.ready

# AI
ai.symptom-checker.completed
ai.scribe.ready
ai.usage.threshold.exceeded

# ABDM
abdm.health-record.fetched
abdm.consent.granted
abdm.consent.revoked
```

### 23.4 V2 endpoint totals

| Group | V1 | V2 added | V2 total |
|---|---|---|---|
| Auth | 25 | 30 | 55 |
| Tenancy | 8 | 10 | 18 |
| Clinic Structure | 30 | 15 | 45 |
| Doctors | 18 | 10 | 28 |
| Patients | 22 | 35 | 57 |
| Appointments | 24 | 20 | 44 |
| Prescriptions | 14 | 20 | 34 |
| Billing | 25 | 30 | 55 |
| Payments | 18 | 25 | 43 |
| Notifications | 16 | 25 | 41 |
| Engagement | 38 | 25 | 63 |
| Settings/Reports/Audit | 22 | 15 | 37 |
| AI | 0 | 10 | 10 |
| Telemedicine | 7 | 10 | 17 |
| Lab | 0 | 15 | 15 |
| Plugins | 0 | 15 | 15 |
| FHIR | 0 | 10 | 10 |
| Webhooks | 14 | 35 | 49 |
| **Total** | **~400** | **~325** | **~725** |
