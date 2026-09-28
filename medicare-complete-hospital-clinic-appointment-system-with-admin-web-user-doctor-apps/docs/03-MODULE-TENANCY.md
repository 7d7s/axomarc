# 03 — Tenancy Module (Multi-Tenancy & Install Mode)

> Medicare runs in **one of two install modes** chosen at install time. The mode is a single boolean in `.env`. All code paths support both — there is no separate "multi-tenant build".

---

## 1. The Two Modes

| Mode | Trigger | Behavior |
|---|---|---|
| **Single-tenant** | `MEDICARE_TENANCY=false` (default) | One install serves one clinic. No subdomain routing. All rows have `tenant_id = NULL` (or `tenant_id = 1` constant). Admin URL is `/admin`. |
| **Multi-tenant SaaS** | `MEDICARE_TENANCY=true` | One install serves N clinics. Each clinic gets a subdomain (`apollo.medicare.app`) or a path prefix (`/t/apollo`). Super-admin at the apex domain (`medicare.app/admin`). |

Both modes use the **same code, the same migrations, the same APIs**. The only differences are:
- Whether the `TenantResolver` middleware is enabled.
- Whether a `tenant_id` is required on every business row (or just allowed).
- Whether the admin login is at the apex or per-clinic.

---

## 2. Why Row-Level Isolation, Not Database-per-Tenant

We evaluated three approaches (see [hafiz.dev comparison](https://hafiz.dev/blog/laravel-multi-tenancy-database-vs-subdomain-vs-path-routing-strategies)):

| Approach | Pros | Cons | Verdict |
|---|---|---|---|
| **Database-per-tenant** | Strong isolation, easy per-tenant backup/restore | Migration hell, can't query across tenants, expensive | Reject |
| **Schema-per-tenant** (Postgres) | Decent isolation | Doesn't work well on MySQL; tooling weak | Reject |
| **Row-level with `tenant_id`** | Cheap, fast, easy to add columns, easy cross-tenant analytics for super-admin | Requires discipline — every query must scope | **Chosen** |

With strict Eloquent global scopes + a custom test suite that asserts "User A cannot see rows in Tenant B", row-level is just as safe in practice.

---

## 3. Resolver Strategy

A **tenant** is identified at request-time by `TenantResolver`. Configurable via `MEDICARE_TENANT_RESOLVER`:

| Value | URL pattern | Example |
|---|---|---|
| `subdomain` | `{tenant}.medicare.app` | `apollo.medicare.app/api/v1/...` |
| `path` | `medicare.app/t/{tenant}/...` | `medicare.app/t/apollo/api/v1/...` |
| `header` (internal) | `X-Tenant: apollo` | For mobile apps |

Mobile apps **must** send `X-Tenant: {slug}` in every request when multi-tenant mode is on. The Flutter `Dio` interceptor sets this from a tenant picker shown on first launch.

### 3.1 `TenantResolver` Middleware Flow

```
Incoming request
  ↓
1. Read MEDICARE_TENANCY env
   - if false: skip resolution, set Tenant::current() = null, proceed
   - if true: continue
  ↓
2. Try resolvers in order:
   a. Header  X-Tenant       (for mobile)
   b. Subdomain  {x}.medicare.app
   c. Path  /t/{x}/...
   ↓
3. Look up tenant in DB (cache for 60s in Redis)
   - 404 if not found
   - 423 Locked if tenant.status = 'suspended'
   - 402 Payment Required if tenant.subscription_expires_at < now
   ↓
4. Set Tenant::current() to this tenant (in-process singleton + Redis)
  ↓
5. Set config('medicare.tenant_id') and config('app.timezone') = tenant.timezone
  ↓
6. Bind scoped services: current clinic, current currency, current language
  ↓
7. Pass to next middleware (auth, etc.)
```

### 3.2 `Tenant::current()` Singleton

```php
namespace App\Modules\Tenancy;

class Tenant {
    private static ?TenantModel $current = null;
    public static function set(TenantModel $t): void { self::$current = $t; }
    public static function get(): ?TenantModel { return self::$current; }
    public static function id(): ?int { return self::$current?->id; }
    public static function isMultiTenantMode(): bool { return config('medicare.tenancy'); }
}
```

This is consumed by:
- The `BelongsToTenant` global scope on every model.
- The `AuditLogger` to stamp `tenant_id`.
- The `Currency` resolver.
- The `Translation` resolver.

---

## 4. Data Model

### 4.1 `tenants` (a.k.a. `clinics`)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `uuid` | char(36) unique | |
| `slug` | varchar(60) unique | URL-safe: `apollo`, `metro-care` |
| `name` | varchar(180) | Display name |
| `legal_name` | varchar(255) nullable | For invoices |
| `logo_path` | varchar(500) nullable | |
| `favicon_path` | varchar(500) nullable | |
| `email` | varchar(180) | |
| `phone` | varchar(20) | |
| `address` | text | |
| `city_id` | bigint FK nullable | |
| `country` | varchar(2) | ISO 3166-1 alpha-2 |
| `timezone` | varchar(60) default 'UTC' | IANA |
| `currency_id` | bigint FK | |
| `default_language_id` | bigint FK | |
| `tax_id_label` | varchar(60) nullable | e.g. "GSTIN", "VAT" |
| `tax_id` | varchar(60) nullable | |
| `status` | enum('active','suspended','trial') default 'trial' | |
| `trial_ends_at` | timestamp nullable | |
| `subscription_ends_at` | timestamp nullable | (V2) |
| `settings` | json | Per-tenant overrides of global settings |
| `created_at`, `updated_at` | timestamps | |
| `deleted_at` | timestamp nullable | |

Indexes: `slug`, `uuid`, `status`, `(country, status)`.

### 4.2 `tenant_domains` (optional custom domains)

| Column | Type |
|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `domain` | varchar(180) unique | e.g. `appointments.apollo.com` |
| `verified_at` | timestamp nullable |
| `is_primary` | boolean default false |

When present, the resolver checks this table before falling back to subdomain.

### 4.3 `tenant_invitations`

| Column | Type |
|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `email` | varchar(180) |
| `role` | varchar(60) |
| `token` | varchar(60) unique |
| `expires_at` | timestamp |
| `accepted_at` | timestamp nullable |
| `invited_by` | bigint FK |

---

## 5. The `BelongsToTenant` Trait

Every business model uses this trait. It registers a global scope that **auto-filters all queries by `tenant_id = Tenant::id()`** and **auto-stamps `tenant_id`** on `creating`.

```php
namespace App\Modules\Tenancy\Concerns;

trait BelongsToTenant
{
    protected static function bootBelongsToTenant(): void
    {
        // Auto-stamp on create
        static::creating(function ($model) {
            if (Tenant::isMultiTenantMode() && Tenant::id()) {
                $model->tenant_id = Tenant::id();
            }
        });

        // Global scope: auto-filter
        static::addGlobalScope(new TenantScope);

        // Prevent cross-tenant ID assignment
        static::saving(function ($model) {
            if (Tenant::isMultiTenantMode()
                && $model->tenant_id !== null
                && Tenant::id() !== null
                && $model->tenant_id !== Tenant::id()) {
                throw new CrossTenantAssignmentException;
            }
        });
    }

    public function tenant(): BelongsTo
    {
        return $this->belongsTo(Tenant::class);
    }
}
```

```php
namespace App\Modules\Tenancy\Scopes;

class TenantScope implements Scope
{
    public function apply(Builder $builder, Model $model): void
    {
        if (!Tenant::isMultiTenantMode()) return;
        $id = Tenant::id();
        if ($id === null) return; // super-admin context
        $builder->where($model->getTable() . '.tenant_id', $id);
    }
}
```

A super-admin can disable the scope with `Model::withoutGlobalScope(TenantScope::class)` for cross-tenant analytics views.

### 5.1 Models That Are Tenant-Scoped

| Tenant-scoped | NOT tenant-scoped (global) |
|---|---|
| `User` (clinic staff) | `Tenant` (the tenant itself) |
| `Patient` | `City` (shared lookup) |
| `Doctor` | `Currency` (shared lookup) |
| `Appointment` | `Language` (shared lookup) |
| `Prescription` | `Tax` (global master) |
| `Invoice` | `Medicine` (global master, used by all tenants) |
| `Transaction` | `Service` (per-tenant — see 4) |
| `Payment` | `SubscriptionPlan` (global catalog — V2) |
| `Refund` | `Country` (global) |
| `Coupon` | `Timezone` (global) |
| `Banner` | `Permission` (global registry) |
| `Testimonial` | `Role` (per-tenant? — see below) |
| `Notification` | `Setting` (per-tenant, key-value) |
| `AuditLog` | `FirebaseServiceAccount` (global) |
| `Device` | `ClinicDomain` (per-tenant) |

### 5.2 `Role` and `Permission` per-tenant or global?

`Permission` is **global** (a fixed registry of ~150 rows). `Role` is **per-tenant** — each clinic defines their own `front-desk`, `accountant`, `nurse` roles mapped to permissions. The super-admin has a special "global" role not tied to a tenant.

---

## 6. Tenant-Aware Migrations

Standard Laravel migrations work for tenant-scoped tables — the column is just there. For tables that need a special per-tenant seed, we use `database/seeders/tenants/` and run with:

```bash
php artisan tenants:seed --class=ClinicDefaultSeeder
```

This is implemented via [Spatie's laravel-multitenancy](https://spatie.be/docs/laravel-multitenancy/v4/introduction) for the heavy lifting (queueable jobs, artisan commands, Redis prefixing).

---

## 7. Tenant-Aware Queues

Jobs inherit the current tenant when dispatched. When the job is processed later by a worker (which may not have a tenant set), we use a `TenantAware` middleware that re-establishes the tenant from the job's serialized `tenant_id`:

```php
namespace App\Modules\Tenancy\Queue;

class TenantAwareMiddleware
{
    public function handle($job, $next)
    {
        if ($job instanceof TenantAware && $job->tenantId) {
            Tenant::set(Tenant::find($job->tenantId));
        }
        $next($job);
        Tenant::set(null);
    }
}
```

The `Reverb` broadcaster also prefixes channels with `tenant.{id}` to prevent accidental cross-tenant broadcast.

---

## 8. Cache & Session Prefixing

Every cache key is prefixed with `t:{tenant_id}:` automatically by a `TenantCachePrefix` task in the `SwitchTenantTasks` array. This is critical so Tenant A's doctor list cache doesn't leak to Tenant B.

Session cookie domain is `.medicare.app` (with leading dot) so a single login carries across subdomains.

---

## 9. Multi-Tenant Admin

Two admin panels, one codebase:

| URL | Audience | Tenant context |
|---|---|---|
| `medicare.app/admin` | Super-admin (us) | None — sees all tenants, with a tenant switcher in the header |
| `{tenant}.medicare.app/admin` | Clinic admin | This tenant only |

The super-admin panel has a top-bar "Viewing as: Apollo Clinic ▾" dropdown that calls `POST /super/switch-tenant` to set a session-scoped `viewing_tenant_id`. All queries then use that tenant's scope.

---

## 10. Install Mode Selection

The installer (see `22-INSTALLER.md`) shows a step 1 question: "Install mode". This writes `MEDICARE_TENANCY` to `.env` and `config/medicare.php`. The choice is **permanent for this install** (changing it requires a migration of all data).

```
┌─────────────────────────────────────────────────┐
│  Welcome to Medicare                            │
│                                                 │
│  Choose install mode:                           │
│   ◯ Single Clinic  — one hospital/clinic        │
│   ◉ Multi-Tenant  — SaaS for many clinics       │
│                                                 │
│  [Next →]                                       │
└─────────────────────────────────────────────────┘
```

---

## 11. Tenant Signup Flow (Multi-Tenant Only)

1. Marketing site at `medicare.app` has "Start free trial" → form: name, email, clinic name, country.
2. Creates a `Tenant` (status=trial) + a `TenantInvitation` for the email.
3. User receives email → clicks → sets password → becomes `super-admin` of the new tenant.
4. Tenant DNS: user adds a CNAME `{clinic-slug}.medicare.app`. We auto-provision via wildcard cert.
5. Custom domain: user adds `appointments.clinic.com` CNAME → we verify → store in `tenant_domains`.
6. Trial is 14 days; reminder emails at day 10, 12, 13. After 14 days, tenant goes to "suspended" state (read-only).
7. Payment integration (V2) extends trial or converts to paid.

---

## 12. Cross-Tenant Data Leakage — The Forbidden Cases

These are explicitly **forbidden** and protected by code review + a CI test:

| Forbidden | How it's prevented |
|---|---|
| Querying `Doctor` without `tenant_id` filter in multi-tenant mode | `BelongsToTenant` global scope |
| Inserting a row with a `tenant_id` that doesn't match the current tenant | `BelongsToTenant::saving` throws |
| Returning a `tenant_id` field in an API response | `UserResource` + auto-hide via `Tenant::id()` check |
| Including another tenant's data in a CSV export | Export services are tenant-scoped; test asserts |
| Logging PHI to a global log | `AuditLogger` requires tenant context |
| Sending FCM to a user in another tenant | `NotificationDispatcher` checks `device.user.tenant_id` |
| Reverb broadcasting to a global channel without `t:{id}` prefix | `Broadcast::channel` regex enforces |

The CI test that runs on every PR seeds 2 tenants, runs all feature tests, and asserts no row from tenant A appears in tenant B's responses.

---

## 13. Database Backup & Restore per Tenant

Single-command backup per tenant:

```bash
php artisan tenant:backup apollo
# Creates storage/backups/apollo-2026-06-04.sql.gz
```

Restores to a **new** tenant slug (data is not overwritten in place):

```bash
php artisan tenant:restore apollo-2026-06-04.sql.gz --as=apollo-recovered
```

Implemented via a `TenantBackupService` that uses `mysqldump` with `--where='tenant_id=1'` style filters per table.

---

## 14. Testing Strategy

| Test | Assertion |
|---|---|
| `TenantScopeTest` | Model without tenant filter returns only current tenant's rows |
| `CrossTenantTest` | A user in tenant A cannot fetch a row in tenant B (returns 404) |
| `TenantResolverTest` | Subdomain, path, header all resolve correctly |
| `CacheIsolationTest` | Cache key `doctors:list` in tenant A != tenant B |
| `BroadcastIsolationTest` | Event broadcast on `t:1.queue` is not received by `t:2.queue` |
| `SeederTest` | All seeders run cleanly in single-tenant and multi-tenant modes |
| `BackupTest` | Restore round-trips without data loss |

---

## 15. Configuration Reference (`config/medicare.php`)

```php
return [
    'tenancy' => env('MEDICARE_TENANCY', false),
    'tenant_resolver' => env('MEDICARE_TENANT_RESOLVER', 'subdomain'),
    'base_domain' => env('MEDICARE_BASE_DOMAIN', 'medicare.app'),
    'session_domain' => env('MEDICARE_SESSION_DOMAIN', '.medicare.app'),
    'trial_days' => env('MEDICARE_TRIAL_DAYS', 14),
    'features' => [
        'pathology'    => env('FEATURE_PATHOLOGY', false),
        'ai_chat'      => env('FEATURE_AI_CHAT', false),
        'blogs'        => env('FEATURE_BLOGS', false),
        'subscriptions'=> env('FEATURE_SUBSCRIPTIONS', false),
    ],
];
```

---

## 16. Acceptance Criteria (V1)

- [ ] Single-tenant mode: install at `clinic.com`, all data lives in one DB, no subdomain needed.
- [ ] Multi-tenant mode: install at `medicare.app`, `apollo.medicare.app` and `metro.medicare.app` both work, data fully isolated.
- [ ] Cross-tenant fetch returns 404, not 403 (to avoid leaking existence).
- [ ] Cache keys are tenant-prefixed.
- [ ] Background jobs remember the tenant.
- [ ] Per-tenant backup works.
- [ ] Suspended tenant returns 423.
- [ ] Custom domain works (e.g. `appointments.apollo.com`).
- [ ] Super-admin can impersonate any tenant.
- [ ] All 50+ feature tests pass with both modes.

---

## 17. V2 Roadmap — Tenancy

> V1 is row-level isolation on a single MySQL. V2 adds custom domain provisioning, per-tenant region routing, per-tenant KMS keys, chain (multi-clinic) tenancy, and a tenant data export API for GDPR.

### 17.1 Schema additions

```php
// Add to tenants table (migration: 2024_08_01_000001_extend_tenants_for_v2)
Schema::table('tenants', function (Blueprint $t) {
    $t->string('plan_id', 32)->nullable()->index();              // 'starter' | 'pro' | 'clinic' | 'enterprise'
    $t->string('subscription_status', 32)->default('trial');      // trial | active | past_due | suspended | cancelled
    $t->string('region', 32)->default('ap-south-1')->index();    // 'us-east-1' | 'eu-west-1' | 'ap-south-1' | 'ap-southeast-1'
    $t->string('kms_key_id', 128)->nullable();                   // AWS/GCP/Azure KMS key ARN
    $t->string('database_shard', 32)->nullable()->index();       // for future sharding (V3)
    $t->json('feature_flags')->nullable();                       // per-tenant override of global flags
    $t->timestamp('trial_ends_at')->nullable();
    $t->timestamp('activated_at')->nullable();
    $t->string('parent_tenant_id', 26)->nullable()->index();     // chain hierarchy
});
```

### 17.2 New table: `tenant_domains` (custom domains)

```php
Schema::create('tenant_domains', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('domain', 255)->unique();                        // 'appointments.apollo.com'
    $t->string('verification_token', 64);                        // for TXT record verification
    $t->string('verification_status', 16)->default('pending');   // pending | verified | failed
    $t->timestamp('verified_at')->nullable();
    $t->string('ssl_status', 16)->default('pending');            // pending | provisioning | active | failed
    $t->string('cloudfront_id', 64)->nullable();                 // AWS CloudFront distribution ID
    $t->timestamps();
    $t->foreign('tenant_id')->references('id')->on('tenants')->cascadeOnDelete();
});
```

### 17.3 New table: `tenant_data_exports` (GDPR Article 15 & 20)

```php
Schema::create('tenant_data_exports', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('user_id')->index();                               // who requested
    $t->string('type', 32);                                      // 'gdpr_export' | 'gdpr_delete' | 'tenant_full'
    $t->string('status', 16)->default('queued');                 // queued | running | completed | failed
    $t->string('s3_key')->nullable();                            // resulting archive
    $t->json('filters')->nullable();
    $t->timestamp('expires_at')->nullable();                    // 7 days after completion
    $t->text('error')->nullable();
    $t->timestamps();
});
```

### 17.4 Custom domain provisioning — `custom-domain` (V2, MML 3)

**1. Overview.** V1 ships every tenant on a wildcard subdomain `*.medicare.app` (e.g., `apollo.medicare.app`). V2 lets Enterprise tenants bring their own domain (`appointments.apollo.com`) with a managed TLS cert and a CloudFront distribution. MML 3 GA covers: TXT-record verification, ACM cert issuance + auto-renewal, CloudFront distribution, HSTS preload, CAA-record awareness, and a "rebrand" path so the wildcard subdomain still works in parallel during migration.

**2. Domain Model.** Per §17.2: `tenant_domains (id, tenant_id, domain, verification_token, verification_status, verified_at, ssl_status, cloudfront_id, ...)`. Plus a runtime cache `tenant_domain_resolver` (Redis) mapping `host → tenant_id`, TTL 60s.

**3. Provisioning flow (the canonical happy path):**
1. Admin → Settings → Domains → "Add Domain" → enters `appointments.apollo.com`.
2. API creates `tenant_domains` row with `verification_token = Str::random(32)`.
3. UI shows: "Add this TXT record to your DNS: `medicare-verify=<token>`". Also shows a CAA-record hint: "If your zone has a CAA record, allow `amazon.com` and `amazontrust.com` to issue certs".
4. Admin adds the TXT record → clicks "Verify".
5. `CustomDomainVerifier` job does `dig TXT _medicare-verify.appointments.apollo.com` (note: subdomain-prefixed to avoid collision with email SPF) → if matches, mark `verification_status='verified'`.
6. Trigger ACM cert request via AWS Certificate Manager (DNS-01 validation, automatic renewal 30 days before expiry).
7. Create Route53 alias + CloudFront distribution pointing to `medicare-api` ALB. Set HSTS `max-age=63072000; includeSubDomains; preload`, TLS 1.3 only, OCSP stapling on.
8. SSL provisioning takes 5–30 min; `ssl_status` flips from `provisioning` → `active`.
9. Domain is live; existing wildcard subdomain still works.
10. `DomainProvisioningService` uses Laravel queues (`domains` queue, 3 retries with exponential backoff).

**4. API Surface (V2).**
```
GET    /api/v2/tenants/me/custom-domains
POST   /api/v2/tenants/me/custom-domains             # { domain }
GET    /api/v2/tenants/me/custom-domains/{id}/verify # triggers verification job
POST   /api/v2/tenants/me/custom-domains/{id}/reissue-cert  # manual ACM reissue
DELETE /api/v2/tenants/me/custom-domains/{id}        # remove; keeps cert in ACM (orphan) for 30d
POST   /api/v2/tenants/me/custom-domains/{id}/set-primary
GET    /api/v2/tenants/me/custom-domains/{id}/health # CloudFront health + cert expiry
```

**5. Frontend.** `/admin/settings/domains` is a single page with three sections: **Subdomain** (read-only, the auto-issued `apollo.medicare.app`), **Custom Domains** (table with status pills, "Add Domain" button, "Verify" CTA, "Remove" action), **Branding** (per-tenant logo, favicon, primary color, HSTS banner preview). A help link explains the DNS records step-by-step with copy-paste values.

**6. Realtime.**
- `domain.verification.attempted.{tenant_id}` — fires when "Verify" is clicked.
- `domain.verification.succeeded.{tenant_id}` / `domain.verification.failed.{tenant_id}` — surfaces a toast.
- `domain.ssl.active.{tenant_id}` — toast + email when the cert is live.
- `domain.cert.expiring.{tenant_id}.{days}` — 30/14/7-day warnings.

**7. Security & RBAC.** Roles: `tenant_admin` (add, verify, remove), `super_admin` (force-remove, re-key). DNS-01 verification uses a per-tenant `_medicare-verify` TXT prefix so a takeover of the zone's apex TXT cannot grant a cert for the apex. CAA-awareness: if the zone has a CAA that does not allow AWS, the system surfaces a clear error before the ACM request is made (avoids a 5–30 min wait for a guaranteed-failure). HSTS preload is opt-in per tenant (the form warns that preload-list submission is one-way). A `Host` header is mapped to a tenant via the Redis cache; a request with an unknown `Host` (i.e., not in `tenant_domains` and not matching the wildcard) is rejected with a 404 and an audit row `action='host.unknown'`. Per `20-Security §23.x`, the custom domain inherits all tenant security settings (MFA required, IP allowlist, etc.) — there is no per-domain override.

**8. Acceptance Criteria.**
- [ ] Custom domain provisioning end-to-end works in <30 min (TXT → ACM → CloudFront).
- [ ] Existing `*.medicare.app` subdomain continues to work alongside the custom domain.
- [ ] A `Host` header that does not match any tenant is rejected with 404 and audit row.
- [ ] DNS-01 verification refuses to issue if the CAA record forbids AWS.
- [ ] ACM cert auto-renewal is verified by a daily cron (the cert's `NotAfter` is at least 60 days out, otherwise an alert fires).
- [ ] Wildcard subdomain `*.medicare.app` is unchanged in behavior (no regression in V1 tenants).
- [ ] Removing a custom domain does not affect the wildcard subdomain.
- [ ] HSTS preload is opt-in and the form warning is shown.

**9. Real-World Test Scenarios.**
- *Parallel domains during rebrand:* tenant keeps `apollo.medicare.app` while `appointments.apollo.com` is being verified; both work; admin flips `is_primary` to the custom domain when ready; the wildcard is no longer advertised in the patient app's login page.
- *CAA conflict:* zone has `CAA 0 issue "letsencrypt.org"`; ACM is forbidden; the UI shows the exact CAA record to add; admin updates the zone; re-verify succeeds.
- *Cert expiry drift:* the daily `cert-expiry-cron` finds a cert at 25 days remaining; ACM auto-renewal is in progress; alert at 14 days if renewal is still pending; alert at 7 days triggers a Slack/email to the on-call.
- *Takeover attempt:* a malicious actor adds a TXT record to a domain the admin lost control of; the API rejects the verification because the TXT was added to a subdomain not owned by the tenant; the prior owner is emailed.
- *CloudFront cache poisoning:* a custom domain is misconfigured to serve a stale distribution; the dashboard surfaces a `health.degraded` event with the affected paths.

**10. Dev Workflow.** `php artisan domain:verify --tenant=... --domain=...` runs the verification manually (used in dev + partner onboarding). `php artisan domain:cert-expiry-report` lists all custom-domain certs and their expiry. `php artisan domain:rebuild-resolver` flushes the Redis cache (used after a tenant rename). Code: `app-modules/custom-domain/`; tests: `app-modules/custom-domain/tests/Feature/{Verify,Provision,Reissue,Health}Test.php` and `tests/Integration/CloudFrontDistributionTest.php` (uses a stub AWS client).

### 17.5 Region routing (`region-routing`, V2)

```php
// app/Services/TenantRouter.php
class TenantRouter {
    public function resolveRegion(string $tenantId): string {
        $cached = Redis::get("tenant:{$tenantId}:region");
        if ($cached) return $cached;
        $region = DB::table('tenants')->where('id', $tenantId)->value('region');
        Redis::setex("tenant:{$tenantId}:region", 3600, $region);
        return $region ?? config('medicare.default_region');
    }
}
```

- **Edge:** Cloudflare Workers inspect the `Host` header, look up the tenant slug in a KV store, return the regional ALB URL.
- **API:** Laravel's `TenantResolver` reads `X-Region` header (set by Cloudflare) to confirm.
- **Latency-based fallback:** if a tenant's region is unavailable, route to nearest healthy region (read-only mode, with banner).

### 17.6 Per-tenant KMS (`per-tenant-kms`, V2.1)

```php
// app/Services/TenantKms.php
class TenantKms {
    public function encrypt(string $tenantId, string $plaintext): string {
        $keyId = $this->getKeyId($tenantId);
        return KmsClient::encrypt(['KeyId' => $keyId, 'Plaintext' => $plaintext])['CiphertextBlob'];
    }
    public function decrypt(string $tenantId, string $ciphertext): string {
        $keyId = $this->getKeyId($tenantId);
        return KmsClient::decrypt(['CiphertextBlob' => $ciphertext])['Plaintext'];
    }
    public function rotate(string $tenantId): void {
        // create new key version, re-wrap data keys, leave old for decrypt-only
    }
}
```

Field-level encryption (e.g., `patients.aadhaar`, `doctors.pan`) uses `TenantKms` and the value is **never** stored in plaintext in MySQL, Redis, or logs. Decryption happens only at the API boundary.

### 17.7 Chain (multi-clinic) tenancy — `multi-clinic-dashboard` (V1.2, MML 3) — deep-dive

**1. Overview.** V1 models **one tenant = one clinic**: a single sign-up creates a single clinic with departments, doctors, appointments, and revenue. Many real customers are **chains** — a hospital group with a headquarters and 5 to 50 branches, a network of dental clinics, a chain of pathology labs. The V1.2 module `multi-clinic-dashboard` adds a **chain hierarchy** to the tenancy model: a `chain` (the top of the tree) has many `branches` (child tenants), each with its own clinic(s), doctors, and patients. A **chain admin** (a user with `chain_admin` role) can see and operate across **all branches** in one consolidated dashboard: aggregate revenue, cross-branch appointments, doctor utilization, inventory, and reports. The chain admin can also configure **revenue split rules** (per `09-Billing §21.5`): for every paid invoice, the gross is split between the branch and the HQ according to the rule, the platform fee is deducted, and the remainder is routed to the branch's payout account. MML 3 GA covers: the chain hierarchy, the chain_admin role + scoped auth, the aggregated dashboard, the cross-branch reports, the revenue split rules, and the nightly payout distribution. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.2 lane module. The deep-dive is anchored here in `03-Tenancy`; the clinic-structure side of the chain hierarchy (the `parent_clinic_id` column) is in `04-Clinic-Structure §14.1`, and the revenue split is in `09-Billing §21.5`.

**2. The chain hierarchy.** A chain is a `tenants` row with `is_chain=true`; a branch is a `tenants` row with `parent_tenant_id=<chain.id>`. A branch can have its own sub-branches (a 3-level tree: chain → region → branch). The chain is a **tenant** in its own right (it has its own subscription, its own settings, its own admins), but it does not have a clinic of its own — it is a parent. Branches have clinics, doctors, and patients. The chain admin sees the chain + all branches; the branch admin sees only their branch.
```
Acme Hospitals (chain_id=acme, parent=null, is_chain=true)
├── Apollo Clinic (parent=acme, region=ap-south-1)
│   └── Indiranagar Branch (parent=apollo)
├── Metro Hospital (parent=acme, region=ap-south-1)
└── Singapore Branch (parent=acme, region=ap-southeast-1)
```

**3. The 3 levels of the chain admin role.**
- **Chain admin** (`role='chain_admin'` in `chain_users`): full access to all branches; can see aggregate metrics; can drill into any branch; can manage chain-wide settings (branding, plans, integrations); **cannot** impersonate a branch admin (a deliberate separation of duty).
- **Region admin** (`role='region_admin'` in `chain_users` with `region`): access to all branches in a region (e.g., all branches in `ap-south-1`).
- **Branch admin** (`role='tenant_admin'` in the branch's `users`): access only to the branch.

**4. The aggregated dashboard.** The chain admin's home page is a single dashboard that aggregates across branches. The widgets are:
- **Revenue** — total revenue, MTD, QoQ growth; per-branch breakdown (stacked bar); per-specialty breakdown (pie).
- **Appointments** — total appointments, completion rate, no-show rate; per-branch breakdown.
- **Doctor utilization** — % of available slots booked, average slots per doctor, top 10 doctors by revenue.
- **Patient funnel** — new patients / returning / churned; per-branch.
- **Inventory** (if V2 enabled) — pharmacy stock levels, low-stock alerts, per-branch.
- **Top issues** — most-booked specialties, longest-wait slots, doctor-bottlenecks.
The dashboard is a single SQL query (or a materialized view `chain_metrics` refreshed every 5 min by `RefreshChainMetricsJob`) that aggregates across `parent_tenant_id = chain.id`. The query respects tenant isolation: the chain admin's view is read-only across branches (they can see metrics but not modify branch data unless they drill in and have a per-branch role).

**5. Cross-branch reports.** The chain admin can run reports that span branches: revenue by branch, by specialty, by doctor, by month; appointments by branch, by specialty, by doctor, by day; patient growth by branch; etc. The reports are produced by the same `custom_report_builder` module (V3, per `13-Settings §13.2`) but with a `chain_scope` filter that expands the query to all branches. The output is a CSV/Excel download.

**6. The 3 revenue split models.** Per `09-Billing §21.5`:
- **Fixed percentage** (e.g., 70% to branch, 30% to HQ): the most common; configurable per chain.
- **Tiered** (e.g., 70% to branch for revenue < ₹10L, 75% for > ₹10L): for chains that want to incentivize growth.
- **Custom rule** (e.g., 80% to branch for the first 6 months, then 70%): for new branches in a ramp-up period.
The rules are stored in `chain_revenue_rules` and evaluated nightly by `DistributeChainRevenueJob`. The job creates `revenue_distributions` rows (one per invoice × branch), credits the branch's `payout_accounts`, and credits the HQ's account. The platform fee is deducted from the branch's share (not the HQ's), per the SaaS agreement.

**7. Domain Model.**
```php
// tenants (extended V1.2)
Schema::table('tenants', function (Blueprint $t) {
    $t->boolean('is_chain')->default(false);
    $t->ulid('parent_tenant_id')->nullable()->index();          // self-reference for branches
    $t->string('chain_role', 16)->default('standalone');        // 'head' | 'branch' | 'standalone'
    $t->string('chain_region', 32)->nullable();                  // e.g., 'ap-south-1'
    $t->timestamps();
});

// chain_users (NEW)
Schema::create('chain_users', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('chain_tenant_id')->index();                       // the chain (parent) tenant
    $t->ulid('user_id')->index();
    $t->string('role', 32);                                      // 'chain_admin' | 'region_admin' | 'chain_auditor'
    $t->string('region', 32)->nullable();                        // for region_admin
    $t->jsonb('scoped_branch_ids')->nullable();                  // for fine-grained access (optional)
    $t->boolean('is_active')->default(true);
    $t->timestamps();
    $t->unique(['chain_tenant_id', 'user_id']);
});

// chain_revenue_rules (NEW; see also 09-Billing §21.5)
Schema::create('chain_revenue_rules', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('chain_tenant_id')->index();
    $t->string('rule_type', 16);                                 // 'fixed_percentage' | 'tiered' | 'custom'
    $t->jsonb('config');                                          // { branch_pct, hq_pct, tiers[], rampup[] }
    $t->string('applies_to', 16)->default('all');                 // 'all' | 'specialty:X' | 'doctor:Y'
    $t->boolean('is_active')->default(true);
    $t->timestamp('effective_from');
    $t->timestamp('effective_until')->nullable();
    $t->timestamps();
});

// chain_metrics (materialized view, refreshed every 5 min)
Schema::create('chain_metrics', function (Blueprint $t) {
    $t->ulid('chain_tenant_id');
    $t->ulid('branch_tenant_id');
    $t->date('metric_date');
    $t->integer('appointments_count')->default(0);
    $t->integer('appointments_completed')->default(0);
    $t->integer('appointments_no_show')->default(0);
    $t->integer('revenue_minor')->default(0);                    // in paise/cents
    $t->char('revenue_currency', 3);
    $t->integer('new_patients_count')->default(0);
    $t->integer('returning_patients_count')->default(0);
    $t->integer('doctor_utilization_pct')->default(0);
    $t->timestamp('refreshed_at');
    $t->primary(['branch_tenant_id', 'metric_date']);
});

// revenue_distributions (per 09-Billing §21.5)
Schema::create('revenue_distributions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('chain_tenant_id')->index();
    $t->ulid('branch_tenant_id')->index();
    $t->ulid('invoice_id')->index();
    $t->ulid('chain_revenue_rule_id')->index();
    $t->integer('gross_minor');
    $t->integer('branch_share_minor');
    $t->integer('hq_share_minor');
    $t->integer('platform_fee_minor');
    $t->integer('branch_payout_minor');                          // = branch_share - platform_fee
    $t->char('currency', 3);
    $t->string('payout_status', 16)->default('pending');         // 'pending' | 'processed' | 'failed'
    $t->string('gateway_transfer_id', 128)->nullable();
    $t->timestamp('distributed_at');
    $t->timestamps();
});

// chain_reports (NEW)
Schema::create('chain_reports', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('chain_tenant_id')->index();
    $t->ulid('created_by_user_id')->index();
    $t->string('report_type', 32);                                // 'revenue_by_branch' | 'appointments_by_specialty' | 'patient_funnel' | 'doctor_utilization' | 'custom'
    $t->jsonb('config');                                           // the report config (filters, group_by, date_range)
    $t->string('status', 16)->default('queued');                  // 'queued' | 'running' | 'completed' | 'failed'
    $t->string('s3_url', 1024)->nullable();
    $t->integer('rows_count')->nullable();
    $t->integer('size_bytes')->nullable();
    $t->text('error')->nullable();
    $t->timestamp('completed_at')->nullable();
    $t->timestamps();
});
```

**8. API Surface.**
```
# Chain setup (tenant admin)
POST   /api/v2/chain/enroll                                    # convert a standalone tenant into a chain (creates a parent tenant)
POST   /api/v2/chain/branches                                  # { name, region?, parent_branch_id? } -> branch tenant
GET    /api/v2/chain/branches                                  -> [branch]
PATCH  /api/v2/chain/branches/{id}                             # { name?, region?, operating_hours? }
DELETE /api/v2/chain/branches/{id}                             # 204 (soft-delete; data retained per retention policy)

# Chain users (chain admin)
GET    /api/v2/chain/users                                     -> [chain_user]
POST   /api/v2/chain/users                                     # { user_id, role, region? } -> chain_user
PATCH  /api/v2/chain/users/{id}                                # { role?, region?, is_active? }
DELETE /api/v2/chain/users/{id}                                # 204

# Aggregated dashboard
GET    /api/v2/chain/dashboard                                 # { revenue, appointments, doctor_utilization, patient_funnel, inventory_alerts, kpis, period_comparison }
GET    /api/v2/chain/dashboard/revenue?from=...&to=...&group_by=branch|specialty|doctor
GET    /api/v2/chain/dashboard/appointments?from=...&to=...&group_by=branch|specialty|doctor|day
GET    /api/v2/chain/dashboard/doctor-utilization?from=...&to=...
GET    /api/v2/chain/dashboard/patient-funnel?from=...&to=...
GET    /api/v2/chain/dashboard/inventory-alerts?from=...&to=...

# Reports
POST   /api/v2/chain/reports                                   # { report_type, config } -> chain_report
GET    /api/v2/chain/reports                                   -> [chain_report]
GET    /api/v2/chain/reports/{id}                              -> chain_report (with s3_url)
GET    /api/v2/chain/reports/{id}/download                     -> 302 to s3_url (signed, 5-min TTL)

# Revenue split rules (per 09-Billing §21.5)
GET    /api/v2/chain/revenue-rules                             -> [chain_revenue_rule]
POST   /api/v2/chain/revenue-rules                             # { rule_type, config, applies_to, effective_from }
PATCH  /api/v2/chain/revenue-rules/{id}
DELETE /api/v2/chain/revenue-rules/{id}

# Revenue distributions
GET    /api/v2/chain/revenue-distributions?from=...&to=...     -> [revenue_distribution]
POST   /api/v2/chain/revenue-distributions/recompute           # re-run the nightly job (admin-only)
```

**9. Frontend.**
- **Chain admin:** `/chain` is the home page; the dashboard widgets are arranged in a 12-col grid; the chain admin can filter by branch, by region, by date range, by specialty, by doctor.
- **Branch drill-down:** clicking a branch opens a slide-over with the branch's dashboard (the same widgets, scoped to the branch). The chain admin can drill in but cannot edit branch settings unless they have a per-branch role.
- **Branches management:** `/chain/branches` is a list of branches with a "Add branch" wizard (4 steps: name + region + parent + admin user). The wizard creates the branch tenant, the branch's first clinic, and the branch admin user; the new admin receives a welcome email.
- **Users management:** `/chain/users` shows chain users with their roles; the chain admin can add a chain_admin, region_admin, or chain_auditor.
- **Revenue rules:** `/chain/revenue-rules` is a form-driven editor: pick the rule type (fixed / tiered / custom), set the percentages, the effective dates, and the applies-to scope.
- **Reports:** `/chain/reports` is a report catalog with one-click generation; the generated file is downloaded as CSV/Excel.

**10. Realtime.**
- `chain.dashboard.refreshed.{chain_tenant_id}` — the materialized view refreshed (every 5 min); the dashboard auto-updates.
- `chain.branch.added.{chain_tenant_id}` — new branch added; dashboard updates.
- `chain.revenue.distributed.{chain_tenant_id}` — nightly job completed; chain admin sees a "Today's distribution" widget.
- `chain.report.completed.{chain_tenant_id}.{chain_report_id}` — a report is ready; chain admin gets a download link.
- `chain.alert.low_stock.{chain_tenant_id}.{branch_tenant_id}.{item_id}` — pharmacy stock below threshold at a branch.

**11. Security & RBAC.**
- **Chain admin** can read everything across branches; can write chain-wide settings; cannot impersonate; cannot modify branch-specific data (e.g., cannot change a branch's doctors or pricing) without a per-branch role.
- **Region admin** can read everything in their region; cannot write chain-wide settings.
- **Chain auditor** (a read-only role) can read everything across branches; cannot write anything.
- **Branch admin** is unaffected; their access is scoped to their branch.
- **Revenue split immutability:** a revenue distribution, once processed, is immutable; corrections require a manual `recompute` by a chain admin, which is logged in `audit_logs` with `action='chain.revenue.recompute'` and the affected invoice IDs.
- **Cross-tenant PII:** the chain admin's view of a branch's patient list is allowed (the chain has a legitimate relationship with the branch) but the chain admin cannot export patient PII in bulk without a `compliance_officer` co-sign per `20-Security §11.4`.
- **Audit:** every chain-level action (branch add, user add, revenue rule change, report generation, recompute) is logged in `audit_logs` with the actor and the affected entity.

**12. Acceptance Criteria.**
- [ ] A tenant can be converted into a chain; a chain can have 1+ branches; a branch can have a parent branch (3-level tree).
- [ ] A chain admin user can be created; the user can log in to the chain dashboard and see all branches.
- [ ] The aggregated dashboard shows revenue, appointments, doctor utilization, patient funnel, and inventory alerts.
- [ ] The dashboard's data is correct: sum of branch metrics = chain metric (verified by `tests/Consistency/ChainMetricsConsistencyTest.php`).
- [ ] A region admin is scoped to a region and cannot see other regions.
- [ ] A chain auditor can read but not write.
- [ ] A branch admin is unaffected and can only see their branch.
- [ ] A revenue split rule can be created (fixed / tiered / custom); the rule is applied to subsequent invoices; the distribution is processed by the nightly job.
- [ ] A chain report can be generated; the report runs across all branches; the output is a CSV/Excel.
- [ ] The chain admin cannot impersonate a branch admin.
- [ ] A chain admin cannot export patient PII in bulk without co-sign.
- [ ] The `chain_metrics` materialized view refreshes every 5 minutes; a drift between the view and the source data opens a P2.

**13. Real-World Test Scenarios.**
- *Onboarding:* a 5-branch dental chain signs up; the chain admin creates 5 branches in the wizard; each branch has its own admin user; each branch's admin logs in and sees only their branch.
- *Aggregated revenue:* the chain admin opens the dashboard; the revenue widget shows ₹25L for the month; the per-branch breakdown shows the 5 branches' contributions; the sum matches the total.
- *Region admin:* the chain creates a region admin for "South India"; the region admin logs in; they see 3 branches (the southern ones); they cannot see the 2 northern branches.
- *Chain auditor:* the chain creates a chain auditor; the auditor sees all dashboards but the "Edit" buttons are disabled.
- *Revenue split:* the chain sets a 70/30 rule (70% branch, 30% HQ, platform fee 5% from the branch's share); an invoice of ₹1,000 is paid at a branch; the distribution is: branch ₹665, HQ ₹300, platform ₹35; the branch's payout account is credited ₹665.
- *Tiered rule:* the chain sets a tiered rule (70% < ₹10L, 75% > ₹10L); a branch crosses ₹10L in March; the next invoice is split 75/25.
- *Custom ramp-up rule:* a new branch gets 80/20 for the first 6 months; after 6 months, it reverts to 70/30; the distribution is automatic.
- *Cross-branch report:* the chain admin runs "Revenue by branch, last quarter"; the report is generated as a CSV; the chain admin downloads it.
- *Bulk PII export attempt:* the chain admin tries to export the entire patient list across all branches; the API returns 403 with a "Requires co-sign" error; the chain admin co-signs with the compliance officer; the export is allowed; the audit log shows the co-sign.
- *Impersonation attempt:* the chain admin tries to log in as a branch admin; the API returns 403 (impersonation is not allowed for chain admins).
- *Materialized view drift:* a doctor manually inserts an appointment (bypassing the API); the dashboard does not include it; the next refresh (5 min) includes it; an alert is fired.

**14. What this module is NOT:**
- Not a standalone module. It is part of the tenancy model and depends on `tenants`, `clinics`, `users`, `chain_users`, and `chain_revenue_rules`.
- Not a franchise management system. We do not manage franchise fees, marketing funds, or franchisee onboarding workflows.
- Not a multi-currency chain. Each branch has its own currency (the clinic's base currency); the chain dashboard sums in the chain's base currency with FX conversion (per `10-Payments §18.5`).
- Not a HIPAA Business Associate Agreement (BAA) chain. The chain admin is covered under the tenant's BAA; sub-tenants are not.

**15. Dev Workflow.** `php artisan chain:enroll --tenant=...` converts a tenant into a chain. `php artisan chain:add-branch --chain=... --name=... --region=...` adds a branch. `php artisan chain:refresh-metrics` refreshes the materialized view. `php artisan chain:distribute-revenue --from=...` runs the nightly job manually. `php artisan chain:add-user --chain=... --user=... --role=chain_admin` adds a chain user. Code: `app-modules/multi-clinic-dashboard/`; tests: `app-modules/multi-clinic-dashboard/tests/Feature/{Enroll,BranchAdd,UserAdd,Dashboard,RevenueSplit,Report,Audit}Test.php` and `tests/Consistency/ChainMetricsConsistencyTest.php` and `tests/Performance/chain-dashboard.k6.js`.

### 17.8 Tenant data export API (GDPR)

```
POST   /api/v2/tenant/data-exports         { type, filters? }    -> { export_id, status: 'queued' }
GET    /api/v2/tenant/data-exports/{id}                         -> { status, s3_url?, expires_at? }
GET    /api/v2/tenant/data-exports                              -> [export]
DELETE /api/v2/tenant/data-exports/{id}                         -> 204
```

`GenerateTenantDataExportJob` runs on the `exports` queue (low priority), streams a tar.gz of all tenant tables to S3 with SSE-KMS, signs a 7-day URL, notifies the requester via email. Long-running jobs (>1 hour) are chunked.

`DELETE /api/v2/tenant/data-exports/{id}` deletes the S3 object; the row stays for audit.

### 17.9 Tenant soft-delete with grace period

V2 adds `tenants.deleted_at` (soft delete). On soft-delete:
- All user logins are blocked.
- All cron jobs are paused.
- All scheduled emails are cancelled.
- Data is **retained 30 days**, then hard-deleted by `PurgeDeletedTenantsJob` (runs daily).
- Customer can request a re-activation within 30 days (`POST /api/v2/tenant/reactivate`).

### 17.10 V2 acceptance criteria

- [ ] Custom domain provisioning end-to-end works in <30 min (TXT → ACM → CloudFront).
- [ ] Wildcard subdomain (`*.medicare.app`) continues to work alongside custom domains.
- [ ] Per-tenant region routing serves requests from the correct region with p99 < 50ms overhead.
- [ ] Per-tenant KMS key rotation works without downtime; data is decryptable with both old and new key versions.
- [ ] Chain admin sees aggregated data across all branches in real-time.
- [ ] Tenant data export downloads all tables as a single archive in <30 min for a 10GB tenant.
- [ ] Soft-deleted tenant cannot be logged into; data is purged after 30 days.
- [ ] GDPR deletion request anonymizes the user in all tables (preserves referential integrity via `gdpr_anonymized_user` stub).
