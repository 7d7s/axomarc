# 15 — Database Schema (Full ERD, Migration Order, Conventions)

> The single source of truth for "what tables exist, what columns they have, and in what order we created them". This is what the migrations implement.

---

## 1. Conventions

### 1.1 Naming

- Tables: **snake_case plural** (`appointments`, `patient_files`).
- Pivot tables: **singular_singular alphabetical** (`clinic_doctor`, `doctor_department`).
- Columns: **snake_case** (`first_name`, `tenant_id`, `is_active`).
- Foreign keys: **singular_id** (`doctor_id`, `appointment_id`).
- Primary keys: **`id`** bigint auto-increment.
- Soft deletes: **`deleted_at`** timestamp nullable.
- Tenant column: **`tenant_id`** bigint nullable (NULL = global or super-admin context).
- Created/updated: **`created_at`, `updated_at`** timestamps.
- UUID for public-facing: **`uuid`** char(36) unique.

### 1.2 Types

- `id` → `bigIncrements` (or `ulid`/`uuid` V2)
- Foreign keys → `unsignedBigInteger` + foreign key constraint
- Money → `bigInteger` (minor units)
- Booleans → `boolean` (TINYINT(1))
- Enums → MySQL `ENUM(...)` (we accept the schema-friction for clarity)
- JSON → MySQL `JSON` (validated at app level)
- Text → `text` (no length limit) vs `varchar(N)` (with limit)

### 1.3 Indexes

Every FK is indexed. Composite indexes are explicitly named. `EXPLAIN` is checked on every query in the slow log.

### 1.4 Charset

- `utf8mb4_unicode_ci` for everything.
- `InnoDB` engine (transactions, FKs).
- `ROW_FORMAT=DYNAMIC` for JSON-heavy tables.

### 1.5 Time

- All timestamps stored in **UTC** (`timestamp` type, no timezone).
- Conversion to clinic timezone happens in the model cast or resource layer.

### 1.6 Soft Deletes

Used on every business entity. Hard deletes are reserved for:
- GDPR erasure requests (after 30-day grace period)
- Anonymized test data
- Super-admin tools

---

## 2. Complete Table Catalog

Roughly 80 tables. Grouped by module.

### 2.1 Identity & Access (10)

| Table | Tenant-Scoped | Notes |
|---|---|---|
| `users` | ✅ | All login identities |
| `password_reset_tokens` | — | Laravel default |
| `personal_access_tokens` | — | Sanctum |
| `otp_verifications` | — | Realm A logs |
| `login_audits` | — | Per-request log |
| `roles` | ✅ | Spatie |
| `permissions` | ❌ (global) | Spatie registry |
| `role_has_permissions` | — | Spatie pivot |
| `model_has_roles` | — | Spatie pivot |
| `model_has_permissions` | — | Spatie pivot |
| `fcm_tokens` | ✅ | Per-device |
| `idempotency_keys` | — | API dedup |

### 2.2 Tenancy & Platform (4)

| Table | Tenant-Scoped |
|---|---|
| `tenants` | (self) |
| `tenant_domains` | ✅ |
| `tenant_invitations` | ✅ |
| `settings` | ✅ + global |

### 2.3 Geography & Structure (5)

| Table | Tenant-Scoped |
|---|---|
| `cities` | ❌ (global) |
| `clinics` | ✅ |
| `departments` | ✅ |
| `specializations` | ✅ |
| `services` | ✅ |

### 2.4 Doctors (8)

| Table | Tenant-Scoped |
|---|---|
| `doctors` | ✅ |
| `doctor_experiences` | ✅ |
| `doctor_educations` | ✅ |
| `doctor_socials` | ✅ |
| `doctor_schedules` | ✅ |
| `doctor_time_slots` | ✅ |
| `doctor_reviews` | ✅ |
| `clinic_doctor` (pivot) | ✅ |
| `doctor_department` (pivot) | ✅ |
| `doctor_specialization` (pivot) | ✅ |
| `doctor_service` (pivot) | ✅ |
| `clinic_service` (pivot) | ✅ |

### 2.5 Patients (7)

| Table | Tenant-Scoped |
|---|---|
| `patients` | ✅ |
| `user_patients` (pivot) | ✅ |
| `vitals` | ✅ |
| `patient_files` | ✅ |
| `patient_mrn_sequences` | ✅ |
| `medicines` | ❌ (global master) |
| `medicine_favorites` | ✅ (per-doctor recent picks) |

### 2.6 Appointments & Queue (6)

| Table | Tenant-Scoped |
|---|---|
| `appointments` | ✅ |
| `appointment_status_logs` | ✅ |
| `appointment_cancellations` | ✅ |
| `queue_tokens` | ✅ |
| `doctor_time_slots` (see above) | ✅ |
| `appointment_reschedules` (optional V1) | ✅ |

### 2.7 Clinical (3)

| Table | Tenant-Scoped |
|---|---|
| `prescriptions` | ✅ |
| `prescription_medicines` | ✅ |
| `prescription_templates` | ✅ |
| `prescription_share_log` | ✅ |
| `prescription_sequences` | ✅ |

### 2.8 Billing & Payments (8)

| Table | Tenant-Scoped |
|---|---|
| `invoices` | ✅ |
| `invoice_lines` | ✅ |
| `invoice_sequences` | ✅ |
| `transactions` | ✅ |
| `payments` | ✅ |
| `refunds` | ✅ |
| `coupons` | ✅ |
| `coupon_usages` | ✅ |
| `taxes` | ❌ (global, tenant-overridable) |

### 2.9 Engagement & i18n (10)

| Table | Tenant-Scoped |
|---|---|
| `testimonials` | ✅ |
| `banners` | ✅ |
| `login_slides` | ✅ (with global fallback) |
| `languages` | ❌ (global) |
| `currencies` | ❌ (global) |
| `translations` | ✅ + global |
| `banner_clicks` (analytics) | ✅ |

### 2.10 Communication (5)

| Table | Tenant-Scoped |
|---|---|
| `notification_templates` | ✅ + global |
| `notification_template_translations` | ✅ |
| `notifications` | ✅ |
| `notification_preferences` | (per user) |
| `email_suppression_list` | — |
| `processed_webhooks` | ✅ |
| `zoom_meetings` | ✅ |

### 2.11 Audit & System (4)

| Table | Tenant-Scoped |
|---|---|
| `audit_logs` | ✅ |
| `cron_jobs` | ✅ |
| `scheduled_reports` | ✅ |
| `waitlist` | ✅ |
| `idempotency_keys` | — |
| `jobs` / `failed_jobs` | — (Laravel) |

### 2.12 V2 Stubs (8)

| Table | Tenant-Scoped | V1 Status |
|---|---|---|
| `pathology_labs` | ✅ | schema only |
| `pathology_tests` | ❌ (global) | schema only |
| `pathology_orders` | ✅ | schema only |
| `pathology_order_items` | ✅ | schema only |
| `blog_categories` | ✅ | schema only |
| `blog_posts` | ✅ | schema only |
| `blog_post_revisions` | ✅ | schema only |
| `blog_comments` | ✅ | schema only |
| `subscription_plans` | ❌ + ✅ override | schema only |
| `subscription_plan_inclusions` | ✅ | schema only |
| `subscription_plan_features` | ✅ | schema only |
| `patient_subscriptions` | ✅ | schema only |
| `subscription_usages` | ✅ | schema only |
| `subscription_invoices` | ✅ | schema only |

---

## 3. ERD (Simplified)

```
┌─────────────┐ 1   M ┌──────────────┐
│   tenants   │───────│  settings    │
└──────┬──────┘       └──────────────┘
       │ 1
       │ M
       │      ┌──────────┐ 1   M ┌──────────┐
       ├──────│  users   │───────│  fcm_tokens│
       │      └─────┬────┘       └──────────┘
       │            │ 1
       │            │ M
       │            │       ┌────────────────┐
       │            ├───────│  user_patients │ M─M
       │            │       └────────┬───────┘
       │            │                │
       │ M          │ M              │ M
       │ ┌──────────┴─┐         ┌────▼─────┐
       │ │   patients │ 1───M   │appointments│
       │ └──────┬─────┘         └─────┬─────┘
       │        │ 1                    │ 1
       │        │ M                    │ M
       │  ┌─────▼──────┐         ┌─────▼──────┐
       │  │  vitals    │         │queue_tokens│
       │  └────────────┘         └────────────┘
       │  ┌────────────┐
       │  │patient_files│
       │  └────────────┘
       │
       │ M   ┌──────────┐ 1   M ┌──────────────┐
       ├─────│  clinics │───────│clinic_doctor │
       │     └──────────┘       └──────┬───────┘
       │                               │ M
       │                               │ 1
       │                          ┌────▼─────┐
       │                          │ doctors  │
       │                          └────┬─────┘
       │                               │ 1
       │                               │ M
       │   ┌──────────────────────┐   │
       │   │doctor_schedules (7/wk)│   │
       │   └──────────┬───────────┘   │
       │              │ 1             │
       │              │ M             │
       │   ┌──────────▼───────────┐   │
       │   │ doctor_time_slots    │   │
       │   └──────────────────────┘   │
       │                              │
       │   ┌──────────────────────┐   │
       │   │  prescriptions       │◄──┘
       │   └──────────┬───────────┘
       │              │ 1
       │              │ M
       │   ┌──────────▼───────────┐
       │   │prescription_medicines│
       │   └──────────────────────┘
       │
       │   ┌──────────────┐ 1   M ┌──────────┐
       │   │  invoices    │───────│invoice_lines│
       │   └──────┬───────┘       └──────────┘
       │          │ 1
       │          │ M
       │   ┌──────▼───────┐ 1   M ┌──────────┐
       │   │ transactions │───────│ payments │──────► (gateway)
       │   └──────────────┘       └──────────┘
       │                          ┌──────────┐
       │                          │ refunds  │
       │                          └──────────┘
       │
       │  ┌─────────────────┐
       │  │ notification_*  │  (per-user send log)
       │  └─────────────────┘
       │  ┌─────────────────┐
       │  │  audit_logs     │  (append-only)
       │  └─────────────────┘
       │
       │  M   ┌─────────────┐
       └──────│ departments │
              └─────────────┘
       M   ┌────────────────┐
       └───│specializations │
           └────────────────┘
       M   ┌────────────┐
       └───│  services  │
           └────────────┘
```

---

## 4. Migration Order (the order to run them)

Migrations are append-only. Never edit a shipped migration. To change a column, write a new migration.

```
Phase 0: Foundation
  0001_01_01_000000_create_users_table                    (Laravel default)
  0001_01_01_000001_create_cache_table                    (Laravel default)
  0001_01_01_000002_create_jobs_table                     (Laravel default)
  0001_01_01_000003_create_personal_access_tokens_table   (Sanctum)
  0001_01_01_000004_create_password_reset_tokens_table    (Laravel default)
  0001_01_01_000005_create_failed_jobs_table              (Laravel default)

Phase 1: Tenancy
  2026_01_01_000010_create_tenants_table
  2026_01_01_000011_create_tenant_domains_table
  2026_01_01_000012_create_tenant_invitations_table
  2026_01_01_000013_add_tenant_id_to_users_table
  2026_01_01_000014_create_settings_table

Phase 2: Auth & RBAC
  2026_01_01_000020_create_permission_tables               (Spatie)
  2026_01_01_000021_create_otp_verifications_table
  2026_01_01_000022_create_login_audits_table
  2026_01_01_000023_create_fcm_tokens_table
  2026_01_01_000024_create_idempotency_keys_table

Phase 3: Geography & Structure
  2026_01_01_000030_create_cities_table
  2026_01_01_000031_create_clinics_table
  2026_01_01_000032_create_departments_table
  2026_01_01_000033_create_specializations_table
  2026_01_01_000034_create_services_table
  2026_01_01_000035_create_clinic_service_table

Phase 4: Doctors
  2026_01_01_000040_create_doctors_table
  2026_01_01_000041_create_doctor_experiences_table
  2026_01_01_000042_create_doctor_educations_table
  2026_01_01_000043_create_doctor_socials_table
  2026_01_01_000044_create_clinic_doctor_table
  2026_01_01_000045_create_doctor_department_table
  2026_01_01_000046_create_doctor_specialization_table
  2026_01_01_000047_create_doctor_service_table
  2026_01_01_000048_create_doctor_schedules_table
  2026_01_01_000049_create_doctor_time_slots_table
  2026_01_01_000050_create_doctor_reviews_table

Phase 5: Patients & Family
  2026_01_01_000060_create_patients_table
  2026_01_01_000061_create_user_patients_table
  2026_01_01_000062_create_patient_mrn_sequences_table
  2026_01_01_000063_create_vitals_table
  2026_01_01_000064_create_patient_files_table

Phase 6: Medicines (global master)
  2026_01_01_000070_create_medicines_table
  2026_01_01_000071_create_medicine_favorites_table
  2026_01_01_000072_seed_medicines_table                   (seeder-as-migration)

Phase 7: Appointments
  2026_01_01_000080_create_appointments_table
  2026_01_01_000081_create_appointment_status_logs_table
  2026_01_01_000082_create_appointment_cancellations_table
  2026_01_01_000083_create_queue_tokens_table

Phase 8: Prescriptions
  2026_01_01_000090_create_prescriptions_table
  2026_01_01_000091_create_prescription_medicines_table
  2026_01_01_000092_create_prescription_templates_table
  2026_01_01_000093_create_prescription_share_log_table
  2026_01_01_000094_create_prescription_sequences_table

Phase 9: Billing
  2026_01_01_000100_create_taxes_table
  2026_01_01_000101_create_coupons_table
  2026_01_01_000102_create_coupon_usages_table
  2026_01_01_000103_create_invoices_table
  2026_01_01_000104_create_invoice_lines_table
  2026_01_01_000105_create_invoice_sequences_table
  2026_01_01_000106_create_transactions_table
  2026_01_01_000107_create_payments_table
  2026_01_01_000108_create_refunds_table

Phase 10: Engagement & i18n
  2026_01_01_000110_create_languages_table
  2026_01_01_000111_create_currencies_table
  2026_01_01_000112_create_translations_table
  2026_01_01_000113_create_testimonials_table
  2026_01_01_000114_create_banners_table
  2026_01_01_000115_create_login_slides_table
  2026_01_01_000116_seed_languages_and_currencies           (seeder-as-migration)

Phase 11: Notifications
  2026_01_01_000120_create_notification_templates_table
  2026_01_01_000121_create_notification_template_translations_table
  2026_01_01_000122_create_notifications_table
  2026_01_01_000123_create_notification_preferences_table
  2026_01_01_000124_create_email_suppression_list_table
  2026_01_01_000125_seed_notification_templates             (seeder-as-migration)

Phase 12: Communication Integrations
  2026_01_01_000130_create_zoom_meetings_table
  2026_01_01_000131_create_processed_webhooks_table

Phase 13: Audit & Reports
  2026_01_01_000140_create_audit_logs_table
  2026_01_01_000141_create_cron_jobs_table
  2026_01_01_000142_create_scheduled_reports_table

Phase 14: V2 Stubs (schema only)
  2026_01_01_000150_create_pathology_tables
  2026_01_01_000151_create_blog_tables
  2026_01_01_000152_create_subscription_tables
  2026_01_01_000153_create_waitlist_table

Phase 15: Installer Final
  2026_01_01_000200_install_default_roles
  2026_01_01_000201_install_default_permissions
  2026_01_01_000202_create_append_only_audit_grants
```

**Why this order matters:**

- Phase 0 must run first (Laravel needs users + cache + jobs).
- Phase 1 (tenants) before anything else because `tenant_id` FK is added to many tables.
- Phase 2 (auth) before user-facing tables.
- Phase 3 (geography) before doctors (clinics + departments needed).
- Phase 7 (appointments) needs doctors + patients.
- Phase 9 (billing) needs appointments (invoice FK).
- Phase 14 (V2 stubs) is schema-only, can be last.
- Phase 15 is the installer's post-migration hook.

---

## 5. Per-Table ERD Highlights

### 5.1 `users` — the central identity

```
users (1) ──< (M) patients        via user_patients (M:M)
      (1) ──< (M) fcm_tokens
      (1) ──< (M) personal_access_tokens
      (1) ──< (M) login_audits
      (1) ──< (M) notifications
      (1) ──< (M) audit_logs (as actor)
      (1) ──< (M) idempotency_keys
      (1) ──< (M) refresh_tokens (V2)
      (1) ──< (M) doctors        (1:1, only if role=doctor)
```

### 5.2 `appointments` — the busiest table

```
appointments (M) ──> (1) patients
           (M) ──> (1) doctors
           (M) ──> (1) clinics
           (M) ──> (1) services (nullable)
           (M) ──> (1) doctor_time_slots (nullable)
           (M) ──> (1) users (as booked_by)
           (M) ──> (1) queue_tokens (1:1, nullable)
           (M) ──> (1) invoices (1:1)
           (M) ──< (M) appointment_status_logs
           (M) ──< (M) appointment_cancellations
           (M) ──< (M) prescriptions
           (M) ──< (1) parent_appointment (self-ref, for reschedule)
```

### 5.3 `payments` / `transactions` — the financial spine

```
payments (M) ──> (1) invoices
         (M) ──> (1) appointments (nullable)
         (M) ──> (1) users (as payer)
         (M) ──> (1) patients (nullable)
         (1) ──< (1) transactions    -- 1:1 for online, 0:1 for cash
transactions (M) ──> (1) invoices
            (M) ──> (1) patients
            (M) ──> (1) payments
            (M) ──< (M) refunds (1:1)
```

---

## 6. Foreign Key Strategy

All FKs use `restrict` on delete (you can't delete a parent that has children). Soft-delete is the only way to "remove" a record. Exceptions (cascade):

- `user_patients.user_id` → `users.id` ON DELETE CASCADE (user deletion removes family links)
- `fcm_tokens.user_id` → `users.id` ON DELETE CASCADE
- `personal_access_tokens.user_id` → `users.id` ON DELETE CASCADE
- `notification_preferences.user_id` → `users.id` ON DELETE CASCADE
- `prescription_medicines.prescription_id` → `prescriptions.id` ON DELETE CASCADE
- `invoice_lines.invoice_id` → `invoices.id` ON DELETE CASCADE
- `appointment_status_logs.appointment_id` → `appointments.id` ON DELETE CASCADE
- `queue_tokens.appointment_id` → `appointments.id` ON DELETE CASCADE

For all others: `ON DELETE RESTRICT`. This forces a deliberate unmapping process.

---

## 7. Index Catalog (Hot Paths)

The 20 most-queried indexes (in order of importance):

| # | Table | Index | Used by |
|---|---|---|---|
| 1 | appointments | (tenant_id, status, start_at) | Calendar, today list |
| 2 | appointments | (doctor_id, start_at) | Doctor day view |
| 3 | appointments | (patient_id, start_at desc) | Patient history |
| 4 | doctor_time_slots | (doctor_id, start_at, status) | Slot listing |
| 5 | doctor_time_slots | (start_at, status) | Slot cleanup cron |
| 6 | notifications | (user_id, created_at desc) | In-app list |
| 7 | notifications | (status, created_at) | Retry queue |
| 8 | audit_logs | (tenant_id, created_at desc) | Compliance query |
| 9 | audit_logs | (auditable_type, auditable_id) | Entity history |
| 10 | audit_logs | (user_id, created_at desc) | Per-user report |
| 11 | transactions | (tenant_id, processed_at desc) | Reports |
| 12 | transactions | (invoice_id) | Invoice detail |
| 13 | payments | (gateway_order_id) | Webhook lookup |
| 14 | patients | (tenant_id, mrn) UNIQUE | MRN search |
| 15 | patients | (tenant_id, phone) | Search |
| 16 | patients | (tenant_id, last_name, first_name) | Search |
| 17 | doctors | (tenant_id, is_active, avg_rating) | Public search |
| 18 | doctor_schedules | (doctor_id, clinic_id, weekday) UNIQUE | Schedule save |
| 19 | vitals | (patient_id, metric, recorded_at) | Chart query |
| 20 | prescription_medicines | (prescription_id, sort_order) | Rx detail |

---

## 8. Data Retention

| Data | Online | Archive | Hard Delete |
|---|---|---|---|
| Appointments | 7 years | After 1 year to S3 Glacier | Never (legal record) |
| Prescriptions | 7 years | After 1 year to S3 Glacier | Never (legal record) |
| Patient files | 7 years | After 1 year to S3 Glacier | Never (legal record) |
| Invoices | 10 years | After 2 years to S3 Glacier | Never (tax) |
| Transactions | 10 years | After 2 years to S3 Glacier | Never (tax) |
| Audit logs | 7 years | After 1 year to S3 Glacier | Never |
| Notifications (in-app, read) | 90 days | — | After 90 days |
| Login audits | 1 year | — | After 1 year |
| OTP verifications | 30 days | — | After 30 days |
| Idempotency keys | 24 hours | — | After 24 hours |
| FCM tokens (inactive) | 60 days | — | After 60 days |
| Cron job records | Forever | — | Never |
| Processed webhooks | 90 days | — | After 90 days |

Implemented as `php artisan data:archive` and `php artisan data:purge` cron jobs.

---

## 9. Schema Migrations Workflow

### 9.1 Generating

```bash
php artisan make:migration add_xyz_to_appointments_table --table=appointments
```

### 9.2 Naming

`YYYY_MM_DD_HHMMSS_verb_noun_table.php` — Laravel's default is fine.

### 9.3 Review

Every migration PR is reviewed for:
- Adds an index if a new FK
- Default value is explicit (not relying on column type default)
- Down migration is correct (or `down() { /* irreversible */ }` if destructive)
- Tenant scope is preserved (don't drop tenant_id)

### 9.4 Testing

- Migrations run on a copy of production data in CI.
- `migrate:fresh` runs cleanly in dev.
- `migrate:rollback` then `migrate` is round-trip safe.

---

## 10. Schema Versioning

The schema is versioned in `app/Modules/Schema/SchemaVersion.php`:

```php
class SchemaVersion {
    public const CURRENT = '1.0.0';
    
    public static function upgradeFrom(string $from): array {
        // returns array of migrations to run
    }
}
```

A mobile app sends `X-Schema-Version: 1.0.0` header. The API returns `409` if the client's known schema is too old.

---

## 11. Backup & Restore

- **Daily mysqldump** to S3 via Forge (retained 30 days).
- **Per-tenant backup** via `php artisan tenant:backup {slug}` (see 03-TENANCY).
- **Restore** creates a new tenant slug; never overwrites live data.
- **Test restore** quarterly in CI.

---

## 12. SQL Style Guide (for raw queries)

When you must write raw SQL (last resort):

```sql
-- Good
SELECT id, uuid, first_name, last_name, mrn
FROM patients
WHERE tenant_id = ?
  AND deleted_at IS NULL
  AND (last_name LIKE ? OR first_name LIKE ?)
ORDER BY last_name ASC, first_name ASC
LIMIT 20;

-- Bad: missing tenant scope, no index hint, ambiguous order
SELECT * FROM patients WHERE last_name LIKE '%smith%';
```

Use `EXPLAIN` for any query touching > 10k rows.

---

## 13. Testing the Schema

| Test | Assertion |
|---|---|
| `MigrationRoundTripTest` | `migrate:fresh` + `migrate:rollback` + `migrate` works |
| `TenantScopeTest` | No model can be saved with wrong `tenant_id` |
| `SoftDeleteCascadeTest` | Soft-deleted parent still has accessible children |
| `FKRestrictTest` | Cannot hard-delete a user with appointments |
| `UniqueConstraintTest` | Duplicate MRN throws at DB level |
| `IndexUsageTest` | EXPLAIN on hot queries uses the documented index |
| `MoneyPrecisionTest` | No float anywhere; minor unit math is exact |
| `JsonCastTest` | `allergies` roundtrips as array |
| `TimestampTimezoneTest` | `created_at` always UTC |

---

## 14. Future Schema Work (V2)

- Move from `bigint` PKs to `ulid` (sortable, less guessable).
- Move money from `bigint` minor to `decimal(20,4)` (same precision, more readable).
- Add `pg_trgm` indexes for fuzzy search (V2 PG migration).
- Partition `audit_logs` by month (V3).
- Partition `notifications` by quarter (V3).
- Move `patient_files` to a separate database (V2; file metadata stays, blobs go to object storage).

---

## 15. V2 Schema Additions (the full V2 catalog)

> V1 ships ~80 tables. V2 adds ~40 more (pathology, telemedicine, AI, subscriptions, insurance, resources, chat, plugins, custom domain, EHR, beds/OT, OIDC), taking the platform to ~120 tables by V3, ~150 by V4.

### 15.1 V2 table catalog (full list)

| # | Table | Module | Doc | Notes |
|---|---|---|---|---|
| 1 | `user_identities` | Auth (V2) | `02-Auth` §17.1 | Federated login (phone, ABHA, OIDC) |
| 2 | `mfa_credentials` | Auth (V2) | `02-Auth` §17.2 | TOTP, WebAuthn, backup codes |
| 3 | `identity_providers` | Auth (V2) | `02-Auth` §17.3 | OIDC per-tenant config |
| 4 | `tenant_domains` | Tenancy (V2) | `03-Tenancy` §17.2 | Custom domains |
| 5 | `tenant_data_exports` | Tenancy (V2) | `03-Tenancy` §17.3 | GDPR export jobs |
| 6 | `resources` | Clinic (V2) | `04-Clinic` §14.2 | Bookable resources |
| 7 | `resource_schedules` | Clinic (V2) | `04-Clinic` §14.2 | Weekly resource schedule |
| 8 | `resource_bookings` | Clinic (V2) | `04-Clinic` §14.2 | Active bookings |
| 9 | `wards` | Clinic (V3) | `04-Clinic` §14.3 | Hospital wards |
| 10 | `beds` | Clinic (V3) | `04-Clinic` §14.3 | Beds |
| 11 | `bed_assignments` | Clinic (V3) | `04-Clinic` §14.3 | Patient↔Bed |
| 12 | `operation_theatres` | Clinic (V3) | `04-Clinic` §14.4 | OTs |
| 13 | `ot_slots` | Clinic (V3) | `04-Clinic` §14.4 | OT calendar |
| 14 | `equipment` | Clinic (V3) | `04-Clinic` §14.5 | Hospital equipment |
| 15 | `doctor_rotation_groups` | Doctors (V1.1) | `05-Doctors` §16.4 | On-call rotations |
| 16 | `doctor_rotations` | Doctors (V1.1) | `05-Doctors` §16.4 | Per-day rotation |
| 17 | `patient_problem_list` | Patients (V2) | `06-Patients` §14.2 | ICD-10 problems |
| 18 | `patient_family_history` | Patients (V2) | `06-Patients` §14.2 | Hereditary conditions |
| 19 | `patient_lab_results` | Patients (V2) | `06-Patients` §14.2 | LOINC-coded results |
| 20 | `patient_vaccinations` | Patients (V2) | `06-Patients` §14.2 | Immunizations |
| 21 | `patient_surgical_history` | Patients (V2) | `06-Patients` §14.2 | Past procedures |
| 22 | `patient_attachments` | Patients (V2) | `06-Patients` §14.3 | S3-backed files |
| 23 | `patient_encounters` | Patients (V2) | `06-Patients` §14.6 | The "consultation" anchor |
| 24 | `appointment_series` | Appts (V1.1) | `07-Appts` §19.1 | Recurring definitions |
| 25 | `group_appointments` | Appts (V1.1) | `07-Appts` §19.2 | Group bookings |
| 26 | `video_rooms` | Appts (V2) | `07-Appts` §19.3 | LiveKit rooms |
| 27 | `appointment_waitlist` | Appts (V1.1) | `07-Appts` §19.5 | Smart waitlist |
| 28 | `refund_policies` | Appts (V1.1) | `07-Appts` §19.6 | Per-clinic refund rules |
| 29 | `drug_interactions` | Rx (V2) | `08-Rx` §16.1 | Drug-drug interactions |
| 30 | `lab_panels` | Rx (V2) | `08-Rx` §16.3 | Curated lab bundles |
| 31 | `lab_partners` | Rx (V2) | `08-Rx` §16.3 | 1mg, Thyrocare, Quest... |
| 32 | `lab_orders` | Rx (V2) | `08-Rx` §16.3 | Lab orders |
| 33 | `lab_order_items` | Rx (V2) | `08-Rx` §16.3 | Individual tests |
| 34 | `lab_samples` | Rx (V2) | `08-Rx` §16.3 | Barcoded samples |
| 35 | `lab_results` | Rx (V2) | `08-Rx` §16.3 | Numeric/text results |
| 36 | `lab_reports` | Rx (V2) | `08-Rx` §16.3 | Generated PDFs |
| 37 | `prescription_refills` | Rx (V2.1) | `08-Rx` §16.5 | Refill requests |
| 38 | `subscription_plans` | Billing (V2) | `09-Billing` §21.1 | Plan catalog |
| 39 | `subscriptions` | Billing (V2) | `09-Billing` §21.1 | Tenant subscriptions |
| 40 | `subscription_items` | Billing (V2) | `09-Billing` §21.1 | Plan add-ons |
| 41 | `subscription_invoices` | Billing (V2) | `09-Billing` §21.1 | Recurring invoices |
| 42 | `subscription_events` | Billing (V2) | `09-Billing` §21.1 | Lifecycle audit |
| 43 | `packages` | Billing (V2) | `09-Billing` §21.2 | Patient packages |
| 44 | `patient_packages` | Billing (V2) | `09-Billing` §21.2 | Active patient packages |
| 45 | `insurance_payers` | Billing (V2) | `09-Billing` §21.3 | Insurance companies |
| 46 | `insurance_policies` | Billing (V2) | `09-Billing` §21.3 | Patient policies |
| 47 | `insurance_preauth_requests` | Billing (V2) | `09-Billing` §21.3 | Pre-auth |
| 48 | `insurance_claims` | Billing (V2) | `09-Billing` §21.3 | Submitted claims |
| 49 | `denials` | Billing (V2.1) | `09-Billing` §21.4 | Denied claims |
| 50 | `dunning_letters` | Billing (V2.1) | `09-Billing` §21.4 | Overdue reminders |
| 51 | `credit_notes` | Billing (V2.1) | `09-Billing` §21.4 | Credits against invoices |
| 52 | `payment_plans` | Billing (V2) | `09-Billing` §21.4 | Installment plans |
| 53 | `payment_plan_installments` | Billing (V2) | `09-Billing` §21.4 | Individual installments |
| 54 | `chain_revenue_rules` | Billing (V2) | `09-Billing` §21.5 | HQ↔Branch split |
| 55 | `revenue_distributions` | Billing (V2) | `09-Billing` §21.5 | Per-invoice split record |
| 56 | `usage_records` | Billing (V2) | `09-Billing` §21.6 | AI token usage |
| 57 | `e_invoices` | Billing (V2) | `09-Billing` §21.7 | GSTN/Peppol e-invoices |
| 58 | `payout_accounts` | Payments (V2) | `10-Payments` §18.6 | Connected accounts |
| 59 | `payout_runs` | Payments (V2) | `10-Payments` §18.7 | Scheduled payouts |
| 60 | `payout_line_items` | Payments (V2) | `10-Payments` §18.7 | Per-doctor payout |
| 61 | `terminal_readers` | Payments (V2) | `10-Payments` §18.3 | Stripe Terminal |
| 62 | `fx_rates` | Payments (V2) | `10-Payments` §18.5 | Currency exchange |
| 63 | `currencies` (was in V1 as JSON) | Payments (V2) | `10-Payments` §18.5 | Currencies table |
| 64 | `bnpl_providers` | Payments (V2) | `10-Payments` §18.2 | Klarna, LazyPay, etc. |
| 65 | `whatsapp_conversations` | Notif (V1.1) | `11-Notif` §20.1 | WA threads |
| 66 | `whatsapp_messages` | Notif (V1.1) | `11-Notif` §20.1 | WA messages |
| 67 | `sms_messages` | Notif (V1.2) | `11-Notif` §20.2 | Twilio SMS |
| 68 | `in_app_chats` | Notif (V2) | `11-Notif` §20.4 | Chat threads |
| 69 | `in_app_messages` | Notif (V2) | `11-Notif` §20.4 | Chat messages |
| 70 | `push_subscriptions` | Notif (V2) | `11-Notif` §20.5 | Topic subscriptions |
| 71 | `blog_categories` | Engage (V2) | `12-Engage` §12.1 | Blog categories |
| 72 | `blog_posts` | Engage (V2) | `12-Engage` §12.1 | Blog posts |
| 73 | `blog_revisions` | Engage (V2) | `12-Engage` §12.1 | Post revisions |
| 74 | `blog_post_reactions` | Engage (V2) | `12-Engage` §12.1 | Likes/love |
| 75 | `blog_comments` | Engage (V2) | `12-Engage` §12.1 | Comments |
| 76 | `report_definitions` | Reports (V3) | `13-Reports` §13.2 | Custom reports |
| 77 | `report_runs` | Reports (V3) | `13-Reports` §13.2 | Run history |
| 78 | `ai_conversations` | AI (V2) | `14-V2-Stubs` §4.18 | Symptom checker convos |
| 79 | `ai_messages` | AI (V2) | `14-V2-Stubs` §4.18 | AI chat messages |
| 80 | `ai_usage_log` | AI (V2) | `14-V2-Stubs` §4.18 | Token usage |
| 81 | `ai_prompts` | AI (V2) | `14-V2-Stubs` §4.18 | Versioned prompts |
| 82 | `plugins` | Platform (V2) | `14-V2-Stubs` §4.7 | Plugin catalog |
| 83 | `plugin_installs` | Platform (V2) | `14-V2-Stubs` §4.7 | Per-tenant installs |
| 84 | `plugin_versions` | Platform (V2) | `14-V2-Stubs` §4.7 | Version history |
| 85 | `plugin_licenses` | Platform (V2) | `14-V2-Stubs` §4.7 | License keys |
| 86 | `fhir_resources` | Interop (V3) | `14-V2-Stubs` §4.19 | Cached FHIR projections |
| 87 | `data_access_log` | Security (V2) | `20-Security` §8.5 | PHI read log |

**Total V2 tables: 87 new. Total V1+V2: 167 tables.**

### 15.2 V2 extension migrations

V2 extends (not breaks) V1 tables. Key extensions:

```php
// users (V1) gets V2 columns
Schema::table('users', function (Blueprint $t) {
    $t->string('mfa_required', 16)->default('off');
    $t->timestamp('last_mfa_at')->nullable();
    $t->json('federated_identities_summary')->nullable();   // ['phone', 'abha', 'google']
});

// patients (V1) gets V2 columns
Schema::table('patients', function (Blueprint $t) {
    $t->string('abha_id', 16)->nullable()->index();
    $t->string('blood_group', 4)->nullable();
    $t->json('allergies')->nullable();
    $t->json('chronic_conditions')->nullable();
    $t->json('current_medications')->nullable();
    $t->string('emergency_contact_phone', 32)->nullable();
    $t->json('insurance_policies')->nullable();
});

// doctors (V1) gets V2 columns
Schema::table('doctors', function (Blueprint $t) {
    $t->string('slug', 128)->nullable()->unique();
    $t->string('geo')->nullable();                           // PostGIS
    $t->string('rotation_group_id', 26)->nullable()->index();
    $t->string('payout_currency', 3)->default('INR');
    $t->string('payout_method', 16)->default('bank');
    $t->json('payout_details')->nullable();
});

// appointments (V1) gets V2 columns
Schema::table('appointments', function (Blueprint $t) {
    $t->ulid('parent_appointment_id')->nullable()->index();
    $t->string('rrule', 512)->nullable();
    $t->ulid('group_appointment_id')->nullable()->index();
    $t->ulid('video_room_id')->nullable();
    $t->json('resources')->nullable();
});

// tenants (V1) gets V2 columns
Schema::table('tenants', function (Blueprint $t) {
    $t->string('plan_id', 32)->nullable()->index();
    $t->string('subscription_status', 32)->default('trial');
    $t->string('region', 32)->default('ap-south-1')->index();
    $t->string('kms_key_id', 128)->nullable();
    $t->string('parent_tenant_id', 26)->nullable()->index();
    $t->json('feature_flags')->nullable();
});
```

### 15.3 V2 indexes (additions to V1's hot-path indexes)

```sql
-- PostGIS geo
CREATE INDEX doctors_geo_gist ON doctors USING GIST (geo);
CREATE INDEX clinics_geo_gist ON clinics USING GIST (geo);

-- pgvector
CREATE INDEX doctor_reviews_embedding_ivf ON doctor_reviews USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);
CREATE INDEX blog_posts_embedding_ivf ON blog_posts USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);

-- Recurring appointments
CREATE INDEX appointments_parent_idx ON appointments (parent_appointment_id, start_at) WHERE parent_appointment_id IS NOT NULL;
CREATE INDEX appointments_series_active ON appointment_series (status, series_end_date) WHERE status = 'active';

-- Resource booking
CREATE UNIQUE INDEX resource_bookings_no_overlap ON resource_bookings (resource_id, start_at) WHERE status IN ('confirmed', 'checked_in');
CREATE INDEX resource_bookings_by_date ON resource_bookings (resource_id, start_at, end_at);

-- Insurance claims
CREATE INDEX insurance_claims_status_idx ON insurance_claims (status, submitted_at);
CREATE INDEX insurance_preauth_status_idx ON insurance_preauth_requests (status, submitted_at);

-- Subscription
CREATE INDEX subscriptions_active_idx ON subscriptions (tenant_id, current_period_end) WHERE status = 'active';
CREATE INDEX subscription_invoices_due_idx ON subscription_invoices (status, period_end) WHERE status = 'open';

-- Chat
CREATE INDEX in_app_chats_by_last_message ON in_app_chats (last_message_at DESC);
CREATE INDEX in_app_messages_by_chat ON in_app_messages (chat_id, created_at DESC);

-- Audit / data access
CREATE INDEX data_access_log_user_time ON data_access_log (user_id, accessed_at);
CREATE INDEX data_access_log_patient_time ON data_access_log (patient_id, accessed_at);
CREATE INDEX audit_logs_actor_time ON audit_logs (actor_user_id, created_at);
CREATE INDEX audit_logs_entity ON audit_logs (entity_type, entity_id, created_at);

-- Plugin
CREATE UNIQUE INDEX plugin_installs_unique ON plugin_installs (tenant_id, plugin_id) WHERE uninstalled_at IS NULL;

-- Plugin tables (per 14-V2-Stubs §4.15)
CREATE TABLE plugins (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  tenant_id BIGINT NULL,                       -- NULL for catalog-wide (Platform); set for tenant-private plugins
  slug VARCHAR(64) NOT NULL,                   -- 'lab-connector-thyrocare', 'ai-scribe-v2', etc.
  name VARCHAR(128) NOT NULL,
  short_description VARCHAR(255),
  long_description TEXT,
  category VARCHAR(32),                        -- 'lab' | 'ai' | 'billing' | 'messaging' | 'analytics' | 'integration'
  icon_url VARCHAR(255),
  homepage_url VARCHAR(255),
  developer_id BIGINT NOT NULL,                -- FK to developers (V2.1 SaaS Plan Marketplace)
  developer_name VARCHAR(128),                 -- denormalised for listing display
  current_version VARCHAR(32) NOT NULL,        -- semver
  latest_version_id BIGINT,                    -- FK to plugin_versions
  visibility ENUM('catalog','tenant_private') NOT NULL DEFAULT 'catalog',
  status ENUM('draft','review','published','deprecated','removed') NOT NULL DEFAULT 'draft',
  is_paid TINYINT(1) NOT NULL DEFAULT 0,
  price_cents INT UNSIGNED NULL,               -- per-month listing price (NULL = free)
  revenue_share_pct TINYINT UNSIGNED NOT NULL DEFAULT 70,  -- developer share (per 14-V2-Stubs §10 Q10)
  install_count INT UNSIGNED NOT NULL DEFAULT 0,
  avg_rating DECIMAL(3,2) NULL,
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
  UNIQUE KEY plugins_slug_unique (slug, COALESCE(tenant_id, 0)),
  KEY plugins_status_category (status, category),
  KEY plugins_developer (developer_id)
);

CREATE TABLE plugin_versions (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  plugin_id BIGINT NOT NULL,
  version VARCHAR(32) NOT NULL,                -- semver
  artifact_url VARCHAR(255) NOT NULL,          -- S3 signed URL, valid 5 min
  artifact_sha256 CHAR(64) NOT NULL,           -- integrity check
  artifact_size_bytes INT UNSIGNED NOT NULL,
  min_medicare_version VARCHAR(16) NOT NULL,   -- '2.0.0'
  max_medicare_version VARCHAR(16) NULL,
  signature TEXT NOT NULL,                     -- detached signature (developer public key verified at install)
  manifest JSON NOT NULL,                      -- { "hooks": [...], "permissions": [...], "ui": {...} }
  release_notes TEXT,
  status ENUM('draft','published','yanked') NOT NULL DEFAULT 'draft',
  published_at TIMESTAMP NULL,
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  UNIQUE KEY plugin_versions_unique (plugin_id, version),
  KEY plugin_versions_status (status)
);

CREATE TABLE plugin_installs (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  tenant_id BIGINT NOT NULL,
  plugin_id BIGINT NOT NULL,
  version_id BIGINT NOT NULL,                  -- FK to plugin_versions
  status ENUM('installed','enabled','disabled','errored') NOT NULL DEFAULT 'installed',
  installed_by_user_id BIGINT NOT NULL,
  installed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  enabled_at TIMESTAMP NULL,
  uninstalled_at TIMESTAMP NULL,
  config JSON NULL,                            -- tenant-specific config blob
  last_health_check_at TIMESTAMP NULL,
  last_health_check_status ENUM('ok','degraded','unreachable') NULL,
  error_message TEXT NULL,
  UNIQUE KEY plugin_installs_unique (tenant_id, plugin_id),
  KEY plugin_installs_tenant_status (tenant_id, status)
);

CREATE TABLE plugin_licenses (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  tenant_id BIGINT NOT NULL,
  plugin_id BIGINT NOT NULL,
  license_key CHAR(32) NOT NULL,               -- public-safe random key
  status ENUM('active','expired','revoked') NOT NULL DEFAULT 'active',
  issued_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  expires_at TIMESTAMP NULL,
  seats INT UNSIGNED NULL,                     -- NULL = unlimited
  seats_used INT UNSIGNED NOT NULL DEFAULT 0,
  gateway_subscription_id VARCHAR(64) NULL,    -- Razorpay/Stripe subscription id (V3 monetization)
  UNIQUE KEY plugin_licenses_key_unique (license_key),
  KEY plugin_licenses_tenant_plugin (tenant_id, plugin_id)
);

-- Blog
CREATE FULLTEXT INDEX blog_posts_fulltext ON blog_posts (title, excerpt, content);
CREATE INDEX blog_posts_status_published ON blog_posts (status, published_at DESC) WHERE status = 'published';

-- Telemedicine
CREATE UNIQUE INDEX video_rooms_appointment ON video_rooms (appointment_id);

-- Lab
CREATE INDEX lab_orders_status_priority ON lab_orders (status, priority, scheduled_at);
CREATE INDEX lab_results_by_order ON lab_results (lab_order_id, verified_at);
CREATE INDEX lab_results_patient_time ON patient_lab_results (patient_id, collected_at DESC);

-- AI usage
CREATE INDEX ai_usage_log_tenant_time ON ai_usage_log (tenant_id, recorded_at);
CREATE INDEX ai_usage_log_feature_time ON ai_usage_log (feature, recorded_at);

-- HL7 v2 (V3, per 19-Integrations §15.10)
CREATE TABLE hl7v2_messages (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  tenant_id BIGINT NOT NULL,
  message_type VARCHAR(16) NOT NULL,            -- 'ADT^A01', 'ADT^A03', 'ADT^A04', 'ADT^A08', 'ORU^R01'
  trigger_event VARCHAR(8) NOT NULL,
  control_id VARCHAR(32) NOT NULL,             -- MSH.10
  sending_facility VARCHAR(64) NOT NULL,       -- MSH.4
  sending_application VARCHAR(64) NULL,         -- MSH.3
  received_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  processed_at TIMESTAMP NULL,
  processing_status ENUM('received','processing','processed','failed','quarantined') NOT NULL DEFAULT 'received',
  processing_duration_ms INT UNSIGNED NULL,
  raw_message MEDIUMTEXT NOT NULL,              -- original HL7 v2 message (for replay)
  parsed_payload JSON NULL,                     -- Mirth's parsed JSON
  affected_entity_type VARCHAR(32) NULL,        -- 'patient', 'patient_admission', 'lab_result'
  affected_entity_id BIGINT NULL,
  retry_count TINYINT UNSIGNED NOT NULL DEFAULT 0,
  last_error TEXT NULL,
  UNIQUE KEY hl7v2_messages_control_unique (tenant_id, control_id),
  KEY hl7v2_messages_tenant_time (tenant_id, received_at DESC),
  KEY hl7v2_messages_status (tenant_id, processing_status, received_at)
);

CREATE TABLE hl7v2_quarantine (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  tenant_id BIGINT NULL,                        -- NULL if sender could not be resolved
  raw_message MEDIUMTEXT NOT NULL,
  received_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  error_code VARCHAR(32) NOT NULL,              -- 'schema_invalid', 'unknown_sender', 'tenant_disabled', 'mllp_timeout'
  error_message TEXT NOT NULL,
  control_id VARCHAR(32) NULL,
  sending_facility VARCHAR(64) NULL,
  resolved_at TIMESTAMP NULL,
  resolved_by_user_id BIGINT NULL,
  resolution ENUM('replayed','rejected','pending') NOT NULL DEFAULT 'pending',
  KEY hl7v2_quarantine_tenant_time (tenant_id, received_at DESC),
  KEY hl7v2_quarantine_resolution (resolution, received_at)
);

CREATE INDEX hl7v2_messages_affected ON hl7v2_messages (tenant_id, affected_entity_type, affected_entity_id);

-- FHIR R4 Write (V3, per 19-Integrations §15.10a)
CREATE TABLE smart_apps (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  tenant_id BIGINT NOT NULL,
  name VARCHAR(128) NOT NULL,
  client_id VARCHAR(64) NOT NULL,
  client_secret_hash CHAR(64) NOT NULL,         -- SHA-256 of secret; raw secret shown only at create
  launch_type ENUM('standalone','ehr-launch') NOT NULL DEFAULT 'standalone',
  redirect_uris JSON NULL,                      -- ehr-launch only
  scopes JSON NOT NULL,                         -- e.g., ["patient/Patient.read", "patient/Observation.write"]
  status ENUM('active','revoked') NOT NULL DEFAULT 'active',
  created_by_user_id BIGINT NOT NULL,
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  revoked_at TIMESTAMP NULL,
  UNIQUE KEY smart_apps_client_id_unique (client_id),
  KEY smart_apps_tenant_status (tenant_id, status)
);

CREATE TABLE fhir_write_log (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  tenant_id BIGINT NOT NULL,
  smart_app_id BIGINT NOT NULL,
  resource_type VARCHAR(32) NOT NULL,           -- 'Patient', 'Observation', etc.
  resource_id VARCHAR(64) NULL,                 -- FHIR id; NULL for failed pre-validation
  operation ENUM('create','update','delete') NOT NULL,
  idempotency_key_hash CHAR(64) NULL,           -- SHA-256 of header value
  projection_status ENUM('synced','pending_projection','projection_failed') NOT NULL,
  http_status SMALLINT UNSIGNED NOT NULL,       -- final response status
  latency_ms INT UNSIGNED NOT NULL,
  payload_size_bytes INT UNSIGNED NOT NULL,
  conflict_resolution ENUM('none','lww') NULL,
  error_code VARCHAR(32) NULL,
  error_message TEXT NULL,
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  KEY fhir_write_log_tenant_time (tenant_id, created_at DESC),
  KEY fhir_write_log_app_time (smart_app_id, created_at DESC),
  KEY fhir_write_log_resource (tenant_id, resource_type, created_at DESC),
  KEY fhir_write_log_idempotency (tenant_id, idempotency_key_hash, created_at DESC),
  KEY fhir_write_log_projection (tenant_id, projection_status, created_at)
);

-- Extend fhir_resources (V2 stub) with V3 columns
ALTER TABLE fhir_resources
  ADD COLUMN projection_status ENUM('synced','pending_projection','projection_failed') NOT NULL DEFAULT 'synced' AFTER last_projected_at,
  ADD COLUMN idempotency_key_hash CHAR(64) NULL AFTER projection_status,
  ADD COLUMN conflict_log JSON NULL AFTER idempotency_key_hash,
  ADD KEY fhir_resources_projection (tenant_id, projection_status, last_projected_at);

CREATE TABLE fhir_smart_tokens (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  smart_app_id BIGINT NOT NULL,
  tenant_id BIGINT NOT NULL,
  access_token_hash CHAR(64) NOT NULL,          -- SHA-256 of opaque token
  refresh_token_hash CHAR(64) NULL,
  scopes JSON NOT NULL,
  expires_at TIMESTAMP NOT NULL,
  refresh_expires_at TIMESTAMP NULL,
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  revoked_at TIMESTAMP NULL,
  KEY fhir_smart_tokens_app (smart_app_id, expires_at DESC),
  KEY fhir_smart_tokens_access (access_token_hash)
);

-- DICOM (V3, per 19-Integrations §15.9)
CREATE TABLE dicom_studies (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  tenant_id BIGINT NOT NULL,
  patient_id BIGINT NOT NULL,
  imaging_order_id BIGINT NULL,                 -- FK to imaging_orders (V2 pathology) or null for ad-hoc
  study_uid VARCHAR(64) NOT NULL,               -- DICOM StudyInstanceUID (0020,000D)
  study_date DATE NULL,                         -- DICOM StudyDate (0008,0020)
  study_description VARCHAR(255) NULL,
  modality VARCHAR(16) NOT NULL,                -- 'CT', 'MR', 'XR', 'US', 'PT', 'MG'
  body_part VARCHAR(64) NULL,
  accession_number VARCHAR(64) NULL,            -- DICOM AccessionNumber (0008,0050)
  referring_doctor_id BIGINT NULL,
  pacs_source ENUM('self_hosted_orthanc','aws_healthimaging','partner_wado_rs') NOT NULL,
  pacs_endpoint VARCHAR(255) NULL,              -- Orthanc URL or HealthImaging datastore id
  orthanc_study_id VARCHAR(64) NULL,            -- Orthanc's internal study id
  instance_count INT UNSIGNED NOT NULL DEFAULT 0,
  series_count INT UNSIGNED NOT NULL DEFAULT 0,
  size_bytes BIGINT UNSIGNED NOT NULL DEFAULT 0,
  s3_bucket VARCHAR(64) NULL,
  s3_prefix VARCHAR(255) NULL,                  -- tenant_id/study_uid/
  status ENUM('ordered','ordered_pacs_unavailable','uploading','received','reported','pacs_upload_failed','cancelled') NOT NULL DEFAULT 'ordered',
  received_at TIMESTAMP NULL,
  reported_at TIMESTAMP NULL,
  retention_until DATE NOT NULL,                -- 7 years per HIPAA
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  UNIQUE KEY dicom_studies_study_uid (tenant_id, study_uid),
  KEY dicom_studies_patient (tenant_id, patient_id, study_date DESC),
  KEY dicom_studies_status (tenant_id, status, received_at),
  KEY dicom_studies_modality_date (tenant_id, modality, study_date DESC)
) PARTITION BY HASH(tenant_id) PARTITIONS 16;

CREATE TABLE dicom_reports (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  dicom_study_id BIGINT NOT NULL,
  reported_by_user_id BIGINT NOT NULL,
  report_text TEXT NOT NULL,
  measurements JSON NULL,                       -- structured measurements (key-value: modality-specific)
  impression TEXT NULL,                         -- the "summary" section
  recommendations TEXT NULL,
  signed_at TIMESTAMP NULL,
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  UNIQUE KEY dicom_reports_study_unique (dicom_study_id),
  KEY dicom_reports_user_time (reported_by_user_id, created_at DESC)
);

-- White-Label Mobile (V3, per 19-Integrations §15.10b)
CREATE TABLE tenant_mobile_config (
  tenant_id BIGINT PRIMARY KEY,
  app_name VARCHAR(64) NOT NULL,
  bundle_id VARCHAR(128) NULL,                  -- iOS, e.g., 'com.acme.health'
  application_id VARCHAR(128) NULL,             -- Android, e.g., 'com.acme.health'
  icon_url VARCHAR(255) NULL,
  splash_image_url VARCHAR(255) NULL,
  primary_color CHAR(7) NULL,                   -- '#RRGGBB'
  secondary_color CHAR(7) NULL,
  api_base_url VARCHAR(255) NULL,
  support_email VARCHAR(128) NULL,
  support_url VARCHAR(255) NULL,
  privacy_url VARCHAR(255) NULL,
  app_store_id VARCHAR(32) NULL,                -- numeric id from App Store Connect
  play_store_package VARCHAR(128) NULL,
  updated_by_user_id BIGINT NOT NULL,
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
);

CREATE TABLE tenant_mobile_builds (
  id BIGINT PRIMARY KEY AUTO_INCREMENT,
  tenant_id BIGINT NOT NULL,
  eas_build_id VARCHAR(64) NULL,                -- EAS-assigned id
  platform ENUM('ios','android') NOT NULL,
  status ENUM('queued','building','completed','failed','cancelled','submitted') NOT NULL DEFAULT 'queued',
  triggered_by_user_id BIGINT NOT NULL,
  git_sha CHAR(40) NOT NULL,                    -- the commit SHA that triggered the build
  config_snapshot JSON NOT NULL,                -- the tenant_mobile_config at build time
  artifact_s3_key VARCHAR(255) NULL,            -- .ipa or .aab in S3
  artifact_size_bytes INT UNSIGNED NULL,
  store_url VARCHAR(255) NULL,                  -- App Store or Play Store URL after submit
  submitted_at TIMESTAMP NULL,
  started_at TIMESTAMP NULL,
  completed_at TIMESTAMP NULL,
  error_message TEXT NULL,
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  KEY tenant_mobile_builds_tenant_time (tenant_id, created_at DESC),
  KEY tenant_mobile_builds_status (tenant_id, status, created_at)
);
```

### 15.4 V2 partitioning (large tables)

```sql
-- audit_logs: partition by month
ALTER TABLE audit_logs PARTITION BY RANGE (YEAR(created_at) * 100 + MONTH(created_at)) (
    PARTITION p202601 VALUES LESS THAN (202602),
    PARTITION p202602 VALUES LESS THAN (202603),
    ...
);

-- data_access_log: partition by quarter (similar)
-- notifications: partition by quarter
-- in_app_messages: partition by month (active chats only)
-- ai_usage_log: partition by month
```

### 15.5 V2 retention policies (additions)

| Table | Retention | Archive | Delete |
|---|---|---|---|
| `audit_logs` | 7 years (legal) | S3 Glacier after 1 year | Never (immutable) |
| `data_access_log` | 7 years | S3 Glacier after 1 year | Never |
| `in_app_messages` | 7 years (medical record) | S3 Glacier after 2 years | Never |
| `notifications` | 2 years | S3 after 1 year | After 2 years |
| `login_audits` | 1 year | S3 after 90 days | After 1 year |
| `tenant_data_exports` | 7 days | — | After 7 days |
| `whatsapp_messages` | 1 year | S3 after 6 months | After 1 year |
| `sms_messages` | 6 months | — | After 6 months |
| `video_recordings` | 90 days (configurable) | — | After retention |
| `ai_usage_log` | 3 years (billing) | S3 after 1 year | After 3 years |
| `appointment_reminders_log` | 1 year | — | After 1 year |
| `blog_revisions` | Forever (until post deleted) | — | On post delete |
| `processed_webhooks` | 90 days | — | After 90 days |
| `failed_jobs` | 30 days | — | After 30 days |
| `cdn_access_logs` | 90 days | S3 after 30 days | After 90 days |

### 15.6 Migration ordering (V1 + V2, single deploy)

V2 migrations run in a separate directory and are opt-in. A V1 tenant that doesn't enable any V2 feature never runs V2 migrations.

```bash
# V1 (always)
php artisan migrate --path=database/migrations/v1

# V2 (opt-in, per feature)
php artisan migrate --path=database/migrations/v2/2024_08_01_lab_orders
php artisan migrate --path=database/migrations/v2/2024_08_02_video_rooms
# etc.
```

The `migrate:v2` command reads `tenants.feature_flags` and only runs the migrations for enabled features. The `migrate:v2 --all` command runs every V2 migration (used in fresh install for V2 SaaS).

### 15.7 Storage estimates (V2 per-tenant, 1 year)

| Table | Rows | Avg size | Total |
|---|---|---|---|
| `appointments` | 50,000 | 2 KB | 100 MB |
| `prescriptions` | 20,000 | 5 KB | 100 MB |
| `invoices` | 50,000 | 1 KB | 50 MB |
| `transactions` | 50,000 | 1 KB | 50 MB |
| `audit_logs` | 5,000,000 | 0.5 KB | 2.5 GB |
| `data_access_log` | 50,000,000 | 0.2 KB | 10 GB |
| `in_app_messages` | 5,000,000 | 0.5 KB | 2.5 GB |
| `ai_usage_log` | 1,000,000 | 0.3 KB | 300 MB |
| `patient_attachments` (S3) | 50,000 | 500 KB | 25 GB |
| `video_recordings` (S3) | 5,000 | 50 MB | 250 GB |
| `lab_reports` (S3) | 5,000 | 200 KB | 1 GB |

Per-tenant DB size after 1 year: ~20 GB. With 100 tenants: 2 TB. RDS MySQL `db.r6g.4xlarge` (1 TB storage) handles 50 tenants comfortably.
