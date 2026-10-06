# 03 — Data Model: Medicare Execution Kernel (PostgreSQL 16)

> **Version:** 0.9 draft-for-lock · **Date:** 2026-08-19
> Principles: (1) RLS on every tenant table; (2) every aggregate owned by a hierarchy node; (3) money/clinical facts immutable + delta-corrected; (4) no FKs across module boundaries — modules reference kernel tables and opaque ids.
> Supersedes `docs/15-DATABASE-SCHEMA.md` (Laravel/MySQL track) for the Rust track.

---

## 1. Kernel Tables

### 1.1 Tenancy & Platform

```sql
CREATE TABLE tenants (
  id            uuid PRIMARY KEY,
  slug          text UNIQUE NOT NULL,            -- subdomain / path key
  name          text NOT NULL,
  status        text NOT NULL,                   -- provisioning|active|suspended|archived
  plan          text NOT NULL DEFAULT 'standard',-- maps to module availability + limits
  kb            jsonb NOT NULL DEFAULT '{}',     -- branding, locale, timezone
  created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE tenant_limits (
  tenant_id     uuid REFERENCES tenants,
  metric        text,                            -- users|locations|bookings_per_day|...
  quota         bigint,
  PRIMARY KEY (tenant_id, metric)
);
```

### 1.2 Hierarchy (scope lattice)

```sql
CREATE TABLE org_nodes (
  id            uuid PRIMARY KEY,
  tenant_id     uuid NOT NULL REFERENCES tenants,
  kind          text NOT NULL,      -- legal_entity|region|business_unit|location|department|warehouse|cost_center
  parent_id     uuid REFERENCES org_nodes,
  name          text NOT NULL,
  path          text NOT NULL,      -- materialized: LE1|BU2|LOC5|DEP3  (self included)
  meta          jsonb NOT NULL DEFAULT '{}',
  active        boolean NOT NULL DEFAULT true,
  created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX org_nodes_tenant_path ON org_nodes (tenant_id, path text_pattern_ops);
-- invariant: kind tree legality enforced by kernel (e.g., department only under location)
```

### 1.3 Identity, Memberships, Grants

```sql
CREATE TABLE identities (
  id            uuid PRIMARY KEY,
  kind          text NOT NULL,                   -- phone_otp|email|platform
  subject_key   text NOT NULL,                   -- e164 number / email / platform actor id
  email         text, phone_e164 text,
  password_hash text,                            -- email kind only (argon2id)
  status        text NOT NULL DEFAULT 'active',
  created_at    timestamptz NOT NULL DEFAULT now(),
  UNIQUE (kind, subject_key)
);

CREATE TABLE users (                               -- tenant-plane person record
  id            uuid PRIMARY KEY,
  tenant_id     uuid NOT NULL REFERENCES tenants,
  identity_id   uuid NOT NULL REFERENCES identities,
  display_name  text NOT NULL,
  kind          text NOT NULL,                   -- admin|staff|doctor|patient
  status        text NOT NULL DEFAULT 'active',
  meta          jsonb NOT NULL DEFAULT '{}'
);
CREATE INDEX users_tenant_identity ON users (tenant_id, identity_id);

CREATE TABLE memberships (
  id            uuid PRIMARY KEY,
  user_id       uuid NOT NULL REFERENCES users,
  node_id       uuid NOT NULL REFERENCES org_nodes,  -- primary affiliation scope
  title         text
);

CREATE TABLE roles (
  id            uuid PRIMARY KEY,
  tenant_id     uuid REFERENCES tenants,           -- NULL = built-in template role
  name          text NOT NULL,
  built_in      boolean NOT NULL DEFAULT false
);

CREATE TABLE user_roles (
  user_id uuid REFERENCES users, role_id uuid REFERENCES roles,
  PRIMARY KEY (user_id, role_id)
);

CREATE TABLE grants (
  id            uuid PRIMARY KEY,
  tenant_id     uuid NOT NULL REFERENCES tenants,
  subject_type  text NOT NULL,                    -- user|role
  subject_id    uuid NOT NULL,
  action_class  text NOT NULL,                    -- e.g. appointment.write
  module        text,                             -- NULL = any
  node_id       uuid NOT NULL REFERENCES org_nodes, -- prefix anchor
  conditions    jsonb NOT NULL DEFAULT '{}',      -- time windows, SoD tags
  valid_from    timestamptz NOT NULL DEFAULT now(),
  valid_until   timestamptz,
  granted_by    uuid,
  delegation_of uuid REFERENCES grants            -- downward delegation chain
);
CREATE INDEX grants_subject ON grants (tenant_id, subject_type, subject_id);
CREATE INDEX grants_node    ON grants (tenant_id, node_id);
```

### 1.4 Sessions, Idempotency, Feature Flags

```sql
CREATE TABLE sessions (
  id            uuid PRIMARY KEY,
  identity_id   uuid NOT NULL REFERENCES identities,
  plane         text NOT NULL,                    -- tenant|platform|support (D-21)
  refresh_hash  text NOT NULL,                    -- rotating refresh token hash
  device        jsonb NOT NULL DEFAULT '{}',
  created_at    timestamptz NOT NULL DEFAULT now(),
  expires_at    timestamptz NOT NULL,
  revoked_at    timestamptz
);

CREATE TABLE idempotency_keys (
  tenant_id uuid, key text, route text,
  result    jsonb,                                -- cached response envelope
  created_at timestamptz NOT NULL DEFAULT now(),
  expires_at timestamptz NOT NULL,
  PRIMARY KEY (tenant_id, key)
);

CREATE TABLE otp_codes (
  id            uuid PRIMARY KEY,
  subject_key   text NOT NULL,                  -- E.164; hashed identifier at rest
  code_hash     text NOT NULL,                  -- argon2id or HMAC of 6-digit code
  attempts      int NOT NULL DEFAULT 0,
  sent_via      text NOT NULL DEFAULT 'sms',
  ip_bucket     text,                           -- coarse, for rate limiting only
  expires_at    timestamptz NOT NULL,
  consumed_at   timestamptz
);
CREATE INDEX otp_codes_subject ON otp_codes (subject_key, expires_at);

CREATE TABLE feature_flags (
  tenant_id uuid REFERENCES tenants,              -- NULL = global default
  flag      text NOT NULL,
  enabled   boolean NOT NULL DEFAULT false,
  cohort    jsonb NOT NULL DEFAULT '{}',          -- {pct: 5, overrides:[tenant...]}
  PRIMARY KEY (tenant_id, flag)
);
```

### 1.5 Deltas & Outbox

```sql
CREATE TABLE delta_partitions (partition_id smallint PRIMARY KEY, stream text UNIQUE);

CREATE TABLE deltas (
  partition_id   smallint NOT NULL,
  seq            bigint NOT NULL,                 -- nextval per-partition sequence
  tenant_id      uuid NOT NULL,
  stream         text NOT NULL,
  aggregate      text NOT NULL,
  aggregate_id   uuid NOT NULL,
  version        bigint NOT NULL,                 -- aggregate version after apply
  action         text NOT NULL,                   -- create|update|transition|compensate
  state          jsonb NOT NULL,                  -- post-state (canonical shape)
  changed        jsonb NOT NULL DEFAULT '{}',     -- minimal diff (never secrets)
  owner_path     text NOT NULL,                   -- hierarchy path for isolation
  actor_id       uuid,
  capability_id  uuid,                            -- grant that authorized this write
  causation_id   uuid,                            -- command/request that caused it
  occurred_at    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (partition_id, seq)
);
CREATE INDEX deltas_tenant_stream ON deltas (tenant_id, stream, seq);
CREATE INDEX deltas_aggregate     ON deltas (tenant_id, aggregate, aggregate_id, version);

CREATE TABLE sync_cursors (
  tenant_id uuid, consumer_id uuid, stream text,
  last_seq  bigint NOT NULL DEFAULT 0,
  PRIMARY KEY (tenant_id, consumer_id, stream)
);

CREATE TABLE outbox (
  id         bigserial PRIMARY KEY,
  tenant_id  uuid NOT NULL,
  channel    text NOT NULL,                       -- webhook|push|sms|email
  target     text NOT NULL,                       -- endpoint url / device token / address
  payload    jsonb NOT NULL,
  dedupe_key text NOT NULL,
  attempts   int NOT NULL DEFAULT 0,
  next_at    timestamptz NOT NULL DEFAULT now(),
  status     text NOT NULL DEFAULT 'pending',     -- pending|sent|failed|dead
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX outbox_due ON outbox (status, next_at) WHERE status IN ('pending','failed');
```

### 1.6 Platform Security (break-glass)

```sql
CREATE TABLE platform_grants (
  id            uuid PRIMARY KEY,
  actor_id      uuid NOT NULL,                    -- platform identity
  tenant_id     uuid NOT NULL REFERENCES tenants,
  scope_prefix  text NOT NULL,
  action_classes text[] NOT NULL,
  reason        text NOT NULL,
  approved_by   uuid,
  valid_from    timestamptz NOT NULL DEFAULT now(),
  valid_until   timestamptz NOT NULL,             -- hard cap: now()+4h
  revoked_at    timestamptz
);
```

---

## 2. RLS Enforcement

```sql
ALTER TABLE users ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation_users ON users
  USING (tenant_id = current_setting('medicare.tenant_id', true)::uuid);
-- same pattern for every tenant table
```

- `kernel-db` opens each business transaction with `SET LOCAL medicare.tenant_id = $1` inside the tx; connection pool resets GUC on release (`DISCARD ALL` on check-in).
- Platform-plane operations (provisioning, break-glass reads) run on a dedicated pool role `medicare_platform` that bypasses RLS; this role is **not** used by tenant request handlers (separate pool objects, compile-time separation of executor types).
- CI test: connection-leak probe asserts no pooled connection carries a tenant GUC after release.

---

## 3. Module Tables (canonical shapes)

> Every table below: `tenant_id` (RLS), `owner_node` (scope anchor), `version` (concurrency), created/updated stamps. FKs only to kernel tables or within the same module. Cross-module links store the foreign aggregate id as `uuid` without DB FK + a stream reference.

### 3.1 mod-clinic
`cities(id, tenant_id, name)` ·
`clinics → org_nodes(kind='location') + clinic_profiles(node_id, address, geo, hours jsonb, media)` ·
`departments → org_nodes(kind='department') + department_profiles(node_id, image, description)` ·
`specializations(id, tenant_id, name)` ·
`services(id, tenant_id, owner_node, name, base_fee numeric(12,2), tax_code, duration_min)`

### 3.2 mod-doctor
`doctors(user_id PK→users, specialization_id, fee, experience_years, bio, signature_url, rating_cache jsonb)` ·
`doctor_profiles_rep(order, degree, institution, year)` (education/experience repeaters) ·
`doctor_memberships(doctor_user_id, node_id)` (multi-clinic) ·
`schedules(doctor_user_id, node_id, weekday, start, end)` ·
`schedule_exceptions(doctor_user_id, node_id, date, reason, closed bool)` ·
`slots(id, doctor_user_id, node_id, service_id, starts_at, ends_at, state text /*open|held|booked|done|expired*/, hold_until, appointment_id)`
- Slot rows are **pre-generated** per (doctor, node, date) by the slot engine; booking takes a row-level lock and transitions `open→booked` atomically.

### 3.3 mod-patient
`patients(id, tenant_id, owner_node, user_id nullable, mrn text UNIQUE per tenant, dob, gender, blood_group, aadhaar_enc, address jsonb, consent jsonb)` ·
`family_members(id, patient_id, relation, name, dob, gender)` ·
`vitals(id, patient_id, kind, value numeric, unit, taken_at, source)` (append-only) ·
`patient_files(id, patient_id, storage_key, mime, size, uploaded_by, phi bool)`

### 3.4 mod-appointment
`appointments(id, tenant_id, owner_node, slot_id UNIQUE, patient_ref uuid /*patient or family*/, doctor_user_id, service_id, starts_at, ends_at, status text, reason, fee_snapshot jsonb, version)` ·
`queue_tokens(id, appointment_id PK, token_no int, issued_at, called_at, completed_at)` (token_no per doctor/day enforced by unique index) ·
`cancel_requests(id, appointment_id, requested_by, reason, status, decided_by, decided_at, refund_ref uuid)`

State machine: `SCHEDULED → CHECKED_IN → IN_CONSULTATION → COMPLETED`; side paths `→ CANCEL_REQUESTED → CANCELLED|SCHEDULED(declined)`, `SCHEDULED → NO_SHOW`.

### 3.5 mod-rx
`medicines(id, tenant_id null=global master, name, generic, strength, form)` ·
`prescriptions(id, appointment_id, doctor_user_id, patient_ref, status text /*draft|issued|dispensed|revoked*/, notes, issued_at, pdf_storage_key, version)` ·
`prescription_items(id, prescription_id, medicine_id nullable, free_text, dosage, frequency, duration, instructions)`

### 3.6 mod-billing
`invoices(id, tenant_id, owner_node, patient_ref, status /*draft|final|posted|void_reversed*/, invoice_no, currency, subtotal, tax_total, total numeric(14,2), period_id, version)` ·
`invoice_items(id, invoice_id, ref_type, ref_id, description, qty, unit_price, tax_code, amount)` ·
`ledger_entries(id, tenant_id, posted_at, account_code, debit numeric(14,2), credit numeric(14,2), invoice_id, payment_id, memo, period_id)` (balanced per tx; CHECK debit≠credit per row, balance enforced by trigger test suite) ·
`payments(id, tenant_id, invoice_id, gateway, gateway_ref, amount, status /*created|authorized|captured|failed|refunded*/, idempotency_key, meta jsonb)` ·
`refunds(id, payment_id, amount, status, workflow_ref, meta)` ·
`fiscal_periods(id, tenant_id, name, starts_at, ends_at, status /*open|closed*/)`.

### 3.7 mod-inventory
`stock_items(id, tenant_id, owner_node /*warehouse*/, medicine_id, uom)` ·
`stock_batches(id, stock_item_id, batch_no, expiry, qty_on_hand)` (qty updated only by movements) ·
`stock_movements(id, tenant_id, owner_node, stock_item_id, kind /*receipt|issue|adjust|transfer_in|transfer_out*/, qty signed, batch_id, ref_type, ref_id, reason)` (append-only;FEFO allocation computed at issue) ·
`purchase_requests(id, owner_node, status, requested_by, workflow_ref)` + `purchase_request_items` ·
`purchase_orders(id, request_id, vendor text, status /*draft|approved|sent|received|closed*/, workflow_ref)` + `purchase_order_items`.

### 3.8 mod-notifycfg
`notification_templates(id, tenant_id, code UNIQUE, channel, subject, body, variables jsonb, locale)` ·
`notification_sends(id, tenant_id, template_id, target_user, channel, status, outbox_id, sent_at)` ·
`user_notification_prefs(user_id, channel, enabled, quiet_from, quiet_to)`

---

## 4. Indexing & Partitioning Strategy

- Deltas: range-partition by `occurred_at` monthly once volume justifies (V1: single table + `deltas_tenant_stream` index is enough; partitioning is an ops migration, app-transparent).
- Slots: `(doctor_user_id, node_id, starts_at)` covering index for availability scans; partial index on `state='open'`.
- Appointments: `(owner_node, starts_at)` for day sheets; `(patient_ref, starts_at desc)` for patient history.
- Ledger: `(tenant_id, account_code, posted_at)`; period-close adds an exclusion check preventing late postings.
- All hot lookups by uuid PK; no text search in V1 (reports use projections).

---

## 5. Migrations

- SQL files in `migrations/<NNNN>_name.sql`, ordered, applied by kernel-db boot runner, recorded in `_migrations`.
- Module migrations are contributed by module crates but merged into the single ordered stream at composition time (xtask validates ordering + cross-module FK ban).
- Policy: additive in-place (expand → migrate → contract); no column drops or renames during a release window; destructive changes gated by a two-release cycle.