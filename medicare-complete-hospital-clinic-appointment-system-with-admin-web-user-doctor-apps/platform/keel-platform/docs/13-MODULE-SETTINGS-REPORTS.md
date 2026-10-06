# 13 — Settings, Reports, Audit Logs

> The cross-cutting platform module. Settings configures everything; Reports queries everything; Audit logs everything.

---

## 1. Settings

### 1.1 Concept

Two-tier settings:

1. **System settings** (`settings` table, key-value, per-tenant or global). Keyed by dotted path: `payments.default_gateway`, `features.subscriptions`, etc.
2. **Config file** (`config/medicare.php`). The single Laravel config file that holds env-derived settings. Editable via the installer wizard (writes to .env) but not at runtime.

### 1.2 Data Model

`settings` (per-tenant + global)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK nullable | NULL = global |
| `key` | varchar(120) | `payments.default_gateway` |
| `value` | json | |
| `type` | enum('string','int','bool','json','array') | Validator hint |
| `group` | varchar(60) | `payments`, `features`, `branding`, `policies` |
| `is_public` | boolean default false | Exposed to public APIs (e.g., `clinic.name`) |
| `created_at`, `updated_at` | timestamps |

Unique: `(tenant_id, key)`.

### 1.3 Helper

```php
// app/Support/Setting.php
setting('payments.default_gateway', 'razorpay');     // reads with fallback chain
setting(['payments' => ['default_gateway' => 'stripe']]);  // write multiple
```

Resolution order: per-tenant DB → global DB → `config/medicare.php` → default.

### 1.4 The Tabs

The admin settings page is organized as tabs. Each tab is a `SettingsTab` class with a `schema()` method.

| Tab | Group | Settings |
|---|---|---|
| **Basic** | `basic` | clinic_name, clinic_email, clinic_phone, clinic_address, timezone, currency, language, date_format, time_format |
| **Branding** | `branding` | logo, favicon, cover_image, primary_color, secondary_color, custom_css |
| **Policies** | `policies` | cancellation_policy, no_show_policy, follow_up_window_days, refund_policy |
| **Web** | `web` | seo_title, seo_description, og_image, custom_domain, google_analytics_id, facebook_pixel_id |
| **Mobile (User App)** | `mobile_user` | app_name, primary_color, splash_image, login_slides_active, force_update_min_version, store_url_ios, store_url_android |
| **Mobile (Doctor App)** | `mobile_doctor` | same as user, with doctor-specific extras |
| **Video** | `video` | provider (zoom), default_meeting_duration, auto_record, waiting_room_enabled |
| **Firebase** | `firebase` | project_id, api_key, auth_domain, storage_bucket, messaging_sender_id, app_id, service_account_json |
| **Payments** | `payments` | default_gateway, enabled_gateways[], razorpay_key_id, stripe_publishable_key, paystack_public_key, min_amount, max_amount, auto_capture |
| **Features** | `features` | pathology, ai_chat, blogs, subscriptions, multi_clinic, family_members, vitals, patient_files, video_calls, reviews, testimonials |
| **Notifications** | `notifications` | push_enabled, email_enabled, sms_enabled (V2), whatsapp_enabled (V2), from_name, from_email, reply_to |
| **Email** | `email` | smtp_host, smtp_port, smtp_username, smtp_password, smtp_encryption, from_address, from_name |
| **Localization** | `localization` | default_language, enabled_languages[], default_currency, enabled_currencies[], timezone |
| **Security** | `security` | session_lifetime_minutes, password_min_length, login_throttle_attempts, login_throttle_lockout_minutes, two_factor_required, ip_whitelist[] (admin) |
| **Backup** | `backup` | auto_backup_enabled, frequency (daily/weekly), retention_days, s3_bucket |
| **Cron** | `cron` | each job: enabled, schedule hint, last-run warning if >X hours |

### 1.5 API

```
GET    /api/v1/admin/settings                      (all, grouped)
GET    /api/v1/admin/settings/{group}             (one tab)
PUT    /api/v1/admin/settings/{group}              (bulk update one tab)
PUT    /api/v1/admin/settings/key/{key}            (single key)
POST   /api/v1/admin/settings/test-email           { to } → sends test email
POST   /api/v1/admin/settings/test-push            { fcm_token, template_key } → sends test push
POST   /api/v1/admin/settings/reset                { group } → resets to default (super-admin only)
GET    /api/v1/public/settings                     (public + is_public only)
```

### 1.6 Caching

Settings cached in Redis with key `settings:{tenant_id|global}`. Invalidated on update. Public settings have a separate `public_settings:{tenant}` key for hot reads (logo URL, currency symbol).

---

## 2. Dashboard

### 2.1 The Two Dashboards

1. **Admin Dashboard** — for clinic admin / front-desk. Shows clinic KPIs.
2. **Doctor Dashboard** — for doctors in the doctor app. Shows their own day + KPIs.
3. **Super-Admin Dashboard** — for us, when MEDICARE_TENANCY=true. Shows all-tenant metrics with drill-down.

### 2.2 Admin Dashboard Cards

```
┌──────────────────────┬──────────────────────┬──────────────────────┬──────────────────────┐
│ Today's Appointments │ Today's Revenue     │ New Patients (week)  │ Avg Rating (clinic)  │
│       42             │      ₹1,25,000      │       18             │      4.6 ★           │
│  +12% vs yesterday   │  +5% vs yesterday   │  +3 vs last week     │  from 132 reviews    │
└──────────────────────┴──────────────────────┴──────────────────────┴──────────────────────┘
```

### 2.3 Charts (Recharts)

- **Revenue (last 30 days)** — line chart, daily granularity.
- **Appointments by status** — stacked bar (confirmed, checked_in, completed, cancelled, no_show).
- **Top 5 doctors by appointments** — horizontal bar.
- **Top 5 services by revenue** — horizontal bar.
- **New patients (last 12 weeks)** — line chart, weekly granularity.

### 2.4 Lists

- **Today's appointment list** — sortable, filterable, with "Call next" action.
- **Pending cancellation requests** — count + quick approve/reject.
- **Pending refund requests** — same.
- **Recent transactions** — last 20.

### 2.5 Realtime

- "Today's count" updates in real time via Reverb (`clinic.{tenant}.dashboard`).
- New appointments + revenue increment on the chart without refresh.

### 2.6 API

```
GET /api/v1/admin/dashboard?date=today
GET /api/v1/admin/dashboard/charts?metric=&period=
GET /api/v1/doctor/dashboard?date=today
```

---

## 3. Reports

### 3.1 Built-in Reports

| Report | Endpoint | Description |
|---|---|---|
| Revenue | `/admin/reports/revenue` | Total, tax, discounts, refunds over a period; groupable by day/week/month/doctor/service/clinic |
| Tax | `/admin/reports/tax` | Tax collected by tax_id, period; exportable in GST-friendly format |
| Doctor earnings | `/admin/reports/doctor-earnings` | Per-doctor gross, refunds, net, platform fee, payout due |
| Appointments | `/admin/reports/appointments` | Total, by status, by type (in-person/online), by doctor; no-show rate |
| Patients | `/admin/reports/patients` | New registrations, demographics (age, gender, city), top patients by visit count |
| Transactions | `/admin/reports/transactions` | All transactions, filterable by gateway, method, status |
| Refunds | `/admin/reports/refunds` | All refunds, with reasons |
| Coupons | `/admin/reports/coupons` | Coupon usage, revenue impact |
| Cancellations | `/admin/reports/cancellations` | Cancellation rate, by reason, by doctor, by time-of-day |
| Reviews | `/admin/reports/reviews` | Doctor ratings over time, sentiment |
| Audit | `/admin/reports/audit` | (See §4) |

### 3.2 Common Query Params (Every Report)

```
?from=2026-06-01
&to=2026-06-30
&clinic_id=
&doctor_id=
&group_by=day|week|month|doctor|service|clinic
&format=json|csv|xlsx|pdf
&page=1
```

### 3.3 Export

- **CSV** — `league/csv`, RFC 4180 compliant.
- **XLSX** — `openspout/openspout`, multi-sheet for breakdowns.
- **PDF** — same PDF engine as Rx, branded with clinic header.
- Exports >1000 rows: dispatched as a background job; user gets an email when ready with a signed download link (24h expiry).

### 3.4 Scheduled Reports

```
POST /api/v1/admin/reports/schedule
{
  report: "revenue",
  frequency: "weekly",     // daily | weekly | monthly
  day_of_week?: 1,         // Monday
  day_of_month?: 1,
  recipients: ["admin@clinic.com", "cfo@clinic.com"],
  format: "xlsx"
}
```

A `SendScheduledReportJob` runs daily at 09:00, finds due schedules, generates, emails.

### 3.5 Custom Reports (V2)

A SQL-query builder for non-technical admins. Out of scope V1.

---

## 4. Audit Logs

### 4.1 The Most Important Table

`audit_logs` is the system of record for **every** action that touches PHI or business-critical data. Required for HIPAA / GDPR / SOC2.

### 4.2 Data Model

`audit_logs`

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `user_id` | bigint FK nullable | Actor (null = system) |
| `impersonated_by_user_id` | bigint FK nullable | If admin was impersonating |
| `session_id` | varchar(60) nullable | |
| `event` | varchar(80) | `patient.created`, `appointment.transition`, `payment.captured`, `phi.access` |
| `auditable_type` | varchar(60) | `App\Modules\Patients\Models\Patient` |
| `auditable_id` | bigint | |
| `old_values` | json nullable | Snapshot before |
| `new_values` | json nullable | Snapshot after |
| `changed_fields` | json nullable | For updates only |
| `url` | varchar(500) nullable | Request path |
| `method` | varchar(10) nullable | HTTP method |
| `ip` | varchar(45) nullable | |
| `user_agent` | varchar(500) nullable | |
| `metadata` | json | Extensible (request_id, etc.) |
| `created_at` | timestamp | Append-only |

Indexes: `(tenant_id, created_at desc)`, `(auditable_type, auditable_id)`, `(user_id, created_at desc)`, `(event, created_at)`.

### 4.3 What Gets Logged

| Module | Events |
|---|---|
| Auth | `user.created`, `user.updated`, `user.deleted`, `user.logged_in`, `user.logged_out`, `user.password_changed`, `user.suspended`, `user.impersonated` |
| Tenancy | `tenant.created`, `tenant.suspended`, `tenant.settings_changed` |
| Patients | `patient.created`, `patient.updated`, `patient.deleted`, `patient.linked`, `patient.unlinked`, `vital.recorded`, `vital.deleted`, `file.uploaded`, `file.downloaded`, `phi.access` |
| Doctors | `doctor.created`, `doctor.updated`, `doctor.schedule_changed`, `doctor.deleted` |
| Appointments | `appointment.created`, `appointment.transition` (every status change), `appointment.cancelled`, `appointment.rescheduled` |
| Prescriptions | `prescription.created`, `prescription.signed`, `prescription.unlocked`, `prescription.amended`, `prescription.deleted`, `prescription.shared`, `prescription.accessed` |
| Billing | `invoice.created`, `invoice.updated`, `invoice.voided`, `transaction.created`, `refund.created`, `coupon.applied` |
| Payments | `payment.created`, `payment.captured`, `payment.failed`, `refund.processed` |
| Settings | `settings.updated`, `role.assigned`, `role.revoked` |
| Admin | `admin.action` (catch-all for sensitive manual ops) |

### 4.4 The "PHI Access" Event

Critical for HIPAA: **every read** of a patient, prescription, vital, or file by a staff user is logged with `event=phi.access`. This is what enables "show me everyone who looked at Patient X's chart in the last 30 days".

```php
// app/Modules/AuditLogs/Services/PhiAccessLogger.php
class PhiAccessLogger {
    public function accessedBy(Authenticatable $user, Model $phi, string $action = 'view'): void {
        AuditLog::create([
            'tenant_id' => Tenant::id(),
            'user_id' => $user->id,
            'event' => 'phi.access',
            'auditable_type' => get_class($phi),
            'auditable_id' => $phi->id,
            'metadata' => ['action' => $action, 'request_id' => request()->header('X-Request-Id')],
            'ip' => request()->ip(),
            'user_agent' => request()->userAgent(),
            'url' => request()->path(),
        ]);
    }
}
```

Called from controllers before the PHI response is returned.

### 4.5 Retention

- Online: 7 years (HIPAA-aligned default).
- Archived to S3 (cold storage) after 1 year, with a manifest table for fast retrieval.
- Restoring an archive for a compliance audit: `php artisan audit:restore --from=2024-01-01 --to=2024-12-31`.

### 4.6 Append-Only Enforcement

- DB user for the app has `INSERT, SELECT` only on `audit_logs` — no UPDATE/DELETE.
- Migration step in installer creates the user with these grants.
- A `BEFORE UPDATE` trigger raises an error (defense in depth).

### 4.7 API

```
GET    /api/v1/admin/audit-logs?event=&user_id=&auditable_type=&auditable_id=&from=&to=&page=
GET    /api/v1/admin/audit-logs/{id}
GET    /api/v1/admin/audit-logs/by-entity?type=&id=        (all logs for one record)
GET    /api/v1/admin/audit-logs/by-user?user_id=&from=&to= (all actions by one user)
GET    /api/v1/admin/audit-logs/phi-access-report?patient_id=&from=&to=  (HIPAA report)

GET    /api/v1/admin/audit-logs/export?from=&to=&format=csv|xlsx
```

### 4.8 Test Mode

In dev/test, audit logging can be disabled via `AUDIT_LOG_ENABLED=false`. In production, **always on** and any disable attempt triggers a Sentry alert.

---

## 5. Admin Layout & Navigation

### 5.1 Sidebar Sections

```
Dashboard
  └─ Overview, Charts, Live Counts

Appointments
  └─ Calendar, Queue, All Appointments, Cancellations

Patients
  └─ All Patients, New, Reports, Import

Doctors
  └─ All Doctors, Schedules, Reviews

Clinic
  └─ Doctors, Departments, Specializations, Services, Staff

Clinical
  └─ Prescriptions, Vitals, Patient Files, Vitals Charts

Billing
  └─ Invoices, Transactions, Refunds, Coupons, Taxes, Reports

Marketing
  └─ Banners, Testimonials, Login Slides, Reviews

Engagement
  └─ Notifications, Templates, Email Log, Push Log

Reports
  └─ Dashboard, Revenue, Tax, Doctor Earnings, Appointments, etc.

System
  └─ Settings (all tabs), Audit Logs, Cron Health, Login Audits, Roles
```

### 5.2 Breadcrumbs

Every page shows breadcrumbs: `Dashboard > Patients > Aarav Sharma > Vitals`. Built from the route hierarchy.

### 5.3 Global Search

Top bar: type a patient's name, MRN, doctor's name, or invoice number. Returns matching rows across entities (with the right tenant scope). Uses a unified index table for sub-200ms response.

---

## 6. Settings Service Methods

```php
namespace App\Modules\Settings\Services;

class SettingsService {
    public const VERSION = '1.0.0';

    public function get(string $key, mixed $default = null): mixed;
    public function set(string $key, mixed $value, string $type = 'string'): void;
    public function setMany(array $kv): void;
    public function getGroup(string $group): array;
    public function resetGroup(string $group): void;          // super-admin
    public function testEmail(array $to): bool;
    public function testPush(string $fcmToken, string $templateKey): bool;
    public function getPublic(): array;                       // only is_public=true
    public function export(): array;                          // full dump
    public function import(array $dump): void;
}
```

---

## 7. Reports Service Methods

```php
class ReportService {
    public function revenue(ReportFilter $f): RevenueReport;
    public function tax(ReportFilter $f): TaxReport;
    public function doctorEarnings(ReportFilter $f): DoctorEarningsReport;
    public function appointments(ReportFilter $f): AppointmentReport;
    public function patients(ReportFilter $f): PatientReport;
    public function transactions(ReportFilter $f): TransactionReport;
    public function refunds(ReportFilter $f): RefundReport;
    public function coupons(ReportFilter $f): CouponReport;
    public function cancellations(ReportFilter $f): CancellationReport;
    public function reviews(ReportFilter $f): ReviewReport;
    public function export(string $report, string $format, ReportFilter $f): string;  // returns file path
    public function schedule(array $config): ScheduledReport;
}
```

---

## 8. Reports — the `ReportFilter` Value Object

```php
final class ReportFilter {
    public function __construct(
        public readonly Carbon $from,
        public readonly Carbon $to,
        public readonly ?int $clinicId = null,
        public readonly ?int $doctorId = null,
        public readonly ?int $serviceId = null,
        public readonly ?int $patientId = null,
        public readonly ?string $gateway = null,
        public readonly ?string $status = null,
        public readonly string $groupBy = 'day',
        public readonly int $page = 1,
        public readonly int $perPage = 50,
    ) {}
}
```

---

## 9. Realtime Events

| Event | Channel | Use |
|---|---|---|
| `DashboardCountsUpdated` | `clinic.{tenant}.dashboard` | Live KPI updates |
| `SettingsChanged` | `clinic.{tenant}.staff` | All open tabs reload settings |
| `NewAuditAlert` | `clinic.{tenant}.admins` | Suspicious activity (5+ failed logins) |

---

## 10. Audit (for the Audit Module Itself)

- `audit_log.viewed` when admin opens audit logs (so we know who's looking at the logs).
- `audit_log.exported` for any export.

---

## 11. Testing Strategy

| Test | Assertion |
|---|---|
| `SettingFallbackTest` | per-tenant → global → config → default |
| `SettingCacheInvalidationTest` | Update invalidates Redis key |
| `SettingPublicEndpointTest` | Only `is_public=true` returned |
| `DashboardTodayTest` | Counts match DB |
| `RevenueReportTest` | Sum of paid invoices in window |
| `TaxReportTest` | Sum of invoice_lines.tax_amount |
| `DoctorEarningsTest` | Per-doctor net = sum of paid - sum of refunds |
| `ScheduledReportTest` | Email sent at scheduled time |
| `ExportCSVTest` | Opens cleanly in Excel |
| `ExportXLSXTest` | Multi-sheet, formatted |
| `ExportPDFTest` | Branded, paginated |
| `PhiAccessTest` | Reading a patient logs `phi.access` |
| `AuditRetentionTest` | Old logs archived, restorable |
| `AppendOnlyTest` | Direct DB UPDATE on audit_logs fails |
| `CrossTenantTest` | Audit log in tenant A invisible to tenant B |
| `SettingsResetTest` | Only super-admin can reset |

---

## 12. Acceptance Criteria (V1)

- [ ] Admin can edit every setting via UI; changes reflect within 1 second.
- [ ] Test email button sends a real email to the entered address.
- [ ] Test push button sends a real push to the entered FCM token.
- [ ] Dashboard loads in <1s p95 with 10k appointments in DB.
- [ ] Revenue report matches the sum of paid invoices in the window.
- [ ] CSV export opens cleanly in Excel with all columns.
- [ ] XLSX export has multiple sheets when there's a breakdown.
- [ ] PDF export is branded and paginated.
- [ ] Scheduled reports email on time with a signed download link.
- [ ] Every PHI read by staff is logged in `audit_logs` with IP + user agent.
- [ ] Audit log is append-only — direct DB UPDATE fails.
- [ ] Audit logs retained 7 years, restorable from cold storage.
- [ ] Cross-tenant audit logs are inaccessible.

---

## 13. V2 Roadmap — Settings, Reports, Audit

> V1 ships a settings store, 11 reports, a dashboard, and an append-only audit log. V2 turns the admin panel into a **self-service platform**: feature flag management, custom report builder, AI usage dashboard, audit log explorer, and a multi-region operational view.

### 13.1 Feature flag admin UI (`feature-flag-admin`, V1.2)

Already defined in `14-V2-STUBS.md` §5. From the settings perspective, this is a new settings tab:

```
Settings
├── ... (V1 tabs)
└── Features (V2)
    ├── Search [_______________]
    ├── Filter: [All | Enabled | Disabled | My Plan]
    │
    ├── Pathology / Lab          [Pro]  [Enabled ✓]  [Configure]
    ├── Telemedicine Video      [Pro]  [Disabled ]  [Enable]
    ├── AI Symptom Checker      [Enterprise] [Locked 🔒]  [Upgrade]
    ├── AI Clinical Scribe      [Enterprise] [Locked 🔒]  [Upgrade]
    ├── In-App Chat             [Pro]  [Disabled ]  [Enable]
    ├── Custom Domain           [Enterprise] [Locked 🔒]  [Upgrade]
    ├── Insurance Pre-Auth      [Enterprise] [Locked 🔒]  [Upgrade]
    ├── Drug Interaction DB     [Pro]  [Enabled ✓]  [Configure]
    ├── Multi-Currency Payout   [Pro]  [Disabled ]  [Enable]
    ├── WhatsApp Business       [Pro]  [Enabled ✓]  [Configure]
    └── ... 30+ more
```

Clicking **Enable** triggers:
1. Check tenant's plan has the feature (`plan.feature_flags`).
2. If not, show upgrade modal.
3. If yes, run any required migrations (idempotent, in background).
4. Set the flag in `tenants.feature_flags`.
5. Broadcast `tenant.feature.activated` event.
6. Show success + "Go to settings for this feature" CTA.

### 13.2 Custom report builder (`custom-report-builder`, V3)

Metabase-style visual query builder. Admin can:

1. Pick a data source: `appointments`, `invoices`, `transactions`, `patients`, `prescriptions`, `lab_orders`, `audit_logs`, etc.
2. Pick columns: drag-and-drop.
3. Add filters: visual query builder (no SQL needed).
4. Add aggregations: count, sum, avg, min, max, group by.
5. Add visualizations: table, line, bar, pie, number, pivot.
6. Save the report: `report_definitions` table.
7. Schedule: email to a list, every day/week/month, with the rendered PDF/CSV.
8. Embed: iframe into the dashboard or external page.

```php
Schema::create('report_definitions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('owner_user_id')->index();
    $t->string('name', 128);
    $t->text('description')->nullable();
    $t->string('data_source', 64);                            // 'appointments' | 'invoices' | 'patients' | 'prescriptions' | 'lab_orders' | 'audit_logs' | custom SQL
    $t->json('query');                                       // { columns, filters, group_by, order_by, limit }
    $t->string('visualization', 32)->default('table');        // 'table' | 'line' | 'bar' | 'pie' | 'number' | 'pivot' | 'map'
    $t->json('viz_config')->nullable();                       // axis labels, colors, etc.
    $t->json('schedule')->nullable();                         // { frequency: 'daily', time: '09:00', recipients: [...] }
    $t->boolean('is_public')->default(false);                 // visible to other staff
    $t->timestamps();
});

Schema::create('report_runs', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('report_id')->index();
    $t->ulid('triggered_by_user_id')->nullable();             // null for scheduled
    $t->string('status', 16)->default('queued');              // 'queued' | 'running' | 'completed' | 'failed'
    $t->json('result_summary')->nullable();                   // { row_count, file_size, duration_ms }
    $t->string('s3_key')->nullable();                         // CSV/XLSX/PDF
    $t->text('error')->nullable();
    $t->timestamp('started_at')->nullable();
    $t->timestamp('completed_at')->nullable();
    $t->timestamps();
});
```

**V2 limitation:** Visual builder supports only the listed data sources. Custom SQL is **V3** and requires `report.query_raw` field + role check.

**1. API surface (V3 GA):**
```
GET    /api/v3/reports/definitions                # list (own + public)
POST   /api/v3/reports/definitions                # create
GET    /api/v3/reports/definitions/{id}          # fetch with query
PATCH  /api/v3/reports/definitions/{id}          # update
DELETE /api/v3/reports/definitions/{id}          # soft-delete
POST   /api/v3/reports/definitions/{id}/duplicate  # copy as new
POST   /api/v3/reports/definitions/{id}/run       # enqueue a run, return { run_id }
GET    /api/v3/reports/runs                       # paginated run history
GET    /api/v3/reports/runs/{id}                  # run status + result summary
GET    /api/v3/reports/runs/{id}/download         # CSV/XLSX/PDF (signed URL, 5 min)
POST   /api/v3/reports/runs/{id}/cancel           # cancel a running job
GET    /api/v3/reports/data-sources               # list available data sources + columns
POST   /api/v3/reports/preview                    # run a query without saving (max 100 rows)
GET    /api/v3/reports/embed/{public_token}       # public embed (no auth, signed token)
```

**2. Realtime events:**
- `report.run.started.{report_id}` — broadcast when the queued job starts.
- `report.run.progress.{report_id}` — every 10% milestone (100k row granularity).
- `report.run.completed.{report_id}` — with `result_summary` and download URL.
- `report.run.failed.{report_id}` — with `error` reason (sanitized: never includes raw SQL or row data).

**3. Custom SQL sandbox (V3):**
- Admin with `reports.query_raw` permission can author a `report.query_raw` field (Postgres-compatible SQL, validated against our schema introspection).
- The sandbox runs the query as a **read-only** Postgres role with `statement_timeout = 30s`, `lock_timeout = 5s`, and row limit `SET LOCAL app.row_limit = 1000000`.
- The query is parsed by `pg_parse_tree` and rejected if it contains any of: `INSERT`, `UPDATE`, `DELETE`, `DROP`, `TRUNCATE`, `GRANT`, `REVOKE`, `CREATE`, `ALTER`, `COPY`, `\\copy`, function calls to `pg_*`, access to system catalogs, or attempt to read from another tenant's data (verified by `pg_class.relacl` check).
- Every raw-SQL run is logged in `report_runs` with the full query and the planner's `EXPLAIN` output. Audit row `action = 'report.raw_sql.run'` includes the query, the user, and the row count.

**4. Run execution model:**
- All runs are queued (per `00-Arch §5 Principle 3`). The HTTP request returns `202 Accepted` with `{ run_id, status: 'queued' }`.
- A `RunReportJob` is dispatched with a backoff: 3 retries on transient failures (DB connection drop, timeout).
- The job streams the result to S3 as CSV/XLSX/PDF (rendered with the same `pdf` library used for invoices).
- Scheduled runs are owned by `scheduler:run-scheduled-reports` running every 5 min; it queries `report_definitions` where `schedule.frequency` is due and enqueues the runs.

**5. Security & RBAC:**
- Creating a custom report requires `reports.create` permission.
- Saving a custom-SQL report requires `reports.query_raw` (a higher-trust permission; granted only to `super-admin` and `admin` roles by default).
- Sharing a report (`is_public = true`) makes it visible to all staff in the tenant; the visibility is still tenant-scoped (no cross-tenant).
- Embedding (`POST /api/v3/reports/definitions/{id}/embed`) generates a `public_token` (32-char URL-safe random, sha256-hashed at rest) that is required to view the report without auth. The token can be rotated or revoked at any time.

**6. Acceptance criteria (V3 GA):**
- [ ] Admin can create a report with the visual builder, save it, run it, and download the result.
- [ ] Custom SQL report is rejected at parse time if it contains forbidden keywords.
- [ ] Scheduled report fires at the configured time and emails the configured recipients.
- [ ] Embed token can be revoked and the embedded report immediately returns 401.
- [ ] All runs are visible in `report_runs` with status, duration, and result summary.
- [ ] A run that exceeds 30s `statement_timeout` is marked `failed` with a clear error and is retried at most once (then marked `failed_permanent`).
- [ ] No PHI from one tenant is ever queryable from another tenant's report (verified by integration test that creates two tenants and attempts cross-tenant query).

**7. Real-world test scenarios:**
- *Revenue reconciliation:* admin creates a report on `invoices` with filter `created_at BETWEEN ... AND ...`, group by `payment_method`, sum `total`. Runs in < 5s for 100k rows. Downloads as XLSX, opens in Excel with correct column headers.
- *Cohort build:* admin creates a report on `patients` with filter `chronic_conditions CONTAINS 'diabetes'` and `last_visit_at > now() - 90 days`. Uses the result to send a campaign via WhatsApp (#58).
- *Capacity planning:* admin creates a scheduled daily report on `appointments` grouped by `doctor_id` and `start_at::date`, sent every morning at 7am to the operations team.
- *Anomaly detection:* admin runs the audit log explorer (#13.4) and finds a user reading 200 patient records in 1 hour; this is detected by the same ML model that powers breach detection (per 20-Security §23.4.2).

**8. What this module is NOT:**
- Not a general BI tool. No data blending across data sources, no cross-schema joins, no ad-hoc data import.
- Not a data warehouse. `report_definitions` and `report_runs` are operational metadata, not data. Results are written to S3 and never to a long-term store.

### 13.3 AI usage dashboard (`ai-usage-dashboard`, V2.1)

Already in `14-V2-STUBS.md`. From the admin perspective:

```
┌─ AI Usage ────────────────────────────────┐
│                                           │
│  This month: 2.4M tokens, $24.00          │
│  [▓▓▓▓▓▓▓▓░░░░░░░░░░] 24% of budget       │
│                                           │
│  By feature:                              │
│  • Symptom Checker   1.2M  $12.00         │
│  • Clinical Scribe   0.9M  $9.00          │
│  • Doctor Rec        0.3M  $3.00          │
│                                           │
│  By user (top 5):                         │
│  • Dr. Jane    450K  $4.50                │
│  • Dr. Patel   380K  $3.80                │
│  ...                                      │
│                                           │
│  Set budget: [$100] [Save]                │
│  Alert at:  [80%] [Save]                  │
└───────────────────────────────────────────┘
```

`ai_usage_log` (V2) is the source. Aggregated nightly by `AggregateAiUsageJob`.

### 13.4 Audit log explorer (`audit-log-explorer`, V2)

V1 has `audit_logs` table but no admin UI to search them. V2 adds a Kibana-style explorer:

- Filters: actor, action, entity_type, entity_id, date range, IP.
- Full-text search on `old_values` + `new_values` (JSON query).
- Detail view: full diff (redacted for sensitive fields), IP, user agent, geolocation.
- Export: CSV, XLSX, PDF.
- Alerts: "email me if any user reads >100 patient records in 1 hour" (anomaly detection).

```php
// data_access_log (V2) is a separate table for PHI reads
Schema::create('data_access_log', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('user_id')->index();
    $t->ulid('patient_id')->index();
    $t->string('entity_type', 64);
    $t->string('entity_id', 26);
    $t->string('action', 32);                                 // 'read' | 'export' | 'print' | 'share'
    $t->string('reason', 64)->nullable();
    $t->string('ip', 64);
    $t->string('user_agent', 512);
    $t->json('metadata')->nullable();
    $t->timestamp('accessed_at')->index();
    $t->index(['user_id', 'accessed_at']);
    $t->index(['patient_id', 'accessed_at']);
});
```

Retention: 7 years. Indexed for fast query. Replicated to cold storage (S3 Glacier) after 1 year.

### 13.5 Operational dashboard (`ops-dashboard`, V2)

For V2 multi-region SaaS tenants, a separate ops dashboard:

- **Health:** uptime per region, error rate, p99 latency.
- **Queue:** jobs waiting, jobs/sec, DLQ depth.
- **DB:** connections in use, slow queries, replication lag.
- **Cache:** hit rate, memory used, evictions.
- **AI service:** token rate, queue depth, model latency.
- **Storage:** S3 bucket size, request rate.
- **Alerts:** active incidents, recent runbook executions.

This is internal-only, accessible only to platform admins (us), not to tenants.

### 13.6 Multi-region management (V2)

Tenants in `eu-west-1` cannot see data in `us-east-1`. The admin panel respects this; the ops dashboard shows aggregate metrics only (no PHI).

### 13.7 V2 API additions

```
GET    /api/v2/features                                    -> [feature]
POST   /api/v2/features/{slug}/enable                      -> { feature_flags: [...] }
POST   /api/v2/features/{slug}/disable                     -> { feature_flags: [...] }
GET    /api/v2/reports/definitions                         -> [report_def]
POST   /api/v2/reports/definitions                         -> report_def
PATCH  /api/v2/reports/definitions/{id}                    -> report_def
POST   /api/v2/reports/definitions/{id}/run                -> report_run
GET    /api/v2/reports/runs                                -> [run]
GET    /api/v2/reports/runs/{id}                           -> run
GET    /api/v2/reports/runs/{id}/download                  -> CSV/XLSX/PDF
GET    /api/v2/ai/usage?from&to                            -> [usage_record aggregated]
POST   /api/v2/ai/usage/budget                             -> { monthly_budget_usd, alert_threshold_pct }
GET    /api/v2/audit-logs?actor&action&entity&from&to      -> [audit_log]
GET    /api/v2/audit-logs/{id}                             -> audit_log with diff
GET    /api/v2/data-access-log?user&patient&from&to       -> [access]
POST   /api/v2/data-access-log/alerts                      -> { rule } (anomaly rule)
```

### 13.8 V2 acceptance criteria

- [ ] Feature flag enable triggers migrations in background, shows progress.
- [ ] Custom report builder produces a valid report; can be saved and re-run.
- [ ] AI usage dashboard shows correct aggregates; budget alerts fire at threshold.
- [ ] Audit log explorer searches 1M rows in <2s.
- [ ] Anomaly rule: "user reads >100 patients/hour" triggers an email.
- [ ] Data access log retains 7 years; queryable in <5s for a 100M row tenant.
