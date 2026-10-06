# 22 — Installer (Domain Activation Wizard, First-Run Setup)

> The first thing every new Medicare install sees. Inspired by the original CodeCanyon product's "domain activation" flow but modernized: works for both single-clinic and multi-tenant SaaS, and can be run multiple times safely.

---

## 1. Purpose

The installer:
1. Validates the environment (PHP version, extensions, file permissions).
2. Collects configuration (DB, tenancy mode, admin user, integrations).
3. Writes `.env` (or updates it).
4. Runs migrations + seeders.
5. Activates the "domain license" (we don't enforce a license in OSS, but the structure supports it).
6. Lands the user on the admin login.

---

## 2. Triggers

The installer shows when **all** of these are true:
- `app.installed` is `false` in the cache (set by the installer on success).
- The requested URL is `/install*` OR no admin user exists yet.

The installer is gated by a separate route file (`routes/install.php`) loaded only when not installed.

If someone hits any other route while not installed → redirect to `/install`.

---

## 3. The 8 Steps

```
┌────────────────────────────────────────────────────────┐
│  Step 1/8  Welcome & Mode                             │
│  Step 2/8  Environment Check                          │
│  Step 3/8  Database                                   │
│  Step 4/8  App URL & Tenancy                          │
│  Step 5/8  Admin Account                              │
│  Step 6/8  Email & Notifications                      │
│  Step 7/8  Integrations (optional)                    │
│  Step 8/8  Review & Install                           │
└────────────────────────────────────────────────────────┘
```

### 3.1 Step 1: Welcome & Mode

```
┌──────────────────────────────────────────────────────────────┐
│  Welcome to Medicare                                         │
│                                                              │
│  Choose how you want to install:                             │
│                                                              │
│  ◯ Single Clinic — one hospital or clinic on this install.  │
│    Best for: a single practice with one website.             │
│                                                              │
│  ● Multi-Tenant — many clinics share this install.           │
│    Each clinic gets its own subdomain (clinic-a.medicare.app)│
│    and fully isolated data. Best for SaaS.                   │
│                                                              │
│  [Next →]                                                    │
└──────────────────────────────────────────────────────────────┘
```

Saves `MEDICARE_TENANCY` to `.env`.

### 3.2 Step 2: Environment Check

Runs `EnvironmentChecker` which validates:

```php
class EnvironmentChecker {
    public function check(): array {
        return [
            'php_version'        => ['ok' => version_compare(PHP_VERSION, '8.3.0', '>='), 'required' => '8.3.0'],
            'php_extensions'     => ['ok' => $this->checkExtensions(['pdo_mysql', 'mbstring', 'intl', 'bcmath', 'gd', 'zip', 'redis', 'curl'])],
            'file_permissions'   => ['ok' => is_writable(storage_path()) && is_writable(base_path('.env'))],
            'max_upload_size'    => ['ok' => ini_get('upload_max_filesize') >= '25M'],
            'memory_limit'       => ['ok' => ini_get('memory_limit') >= '256M'],
            'composer_installed' => ['ok' => file_exists(base_path('vendor/autoload.php'))],
            'node_installed'     => ['ok' => $this->commandExists('node')],
        ];
    }
}
```

Each check shows green/red with a "How to fix" link to docs.

### 3.3 Step 3: Database

Form fields:
- DB Host (default: `127.0.0.1`)
- DB Port (default: `3306`)
- DB Name
- DB Username
- DB Password

On "Test Connection" → tries to connect with provided creds. On success, shows "OK" and the Laravel version of MySQL.

On "Save & Continue" → writes to `.env`:
```
DB_CONNECTION=mysql
DB_HOST=...
DB_PORT=3306
DB_DATABASE=...
DB_USERNAME=...
DB_PASSWORD=...
```

### 3.4 Step 4: App URL & Tenancy

- **App URL** — the public URL of the API.
- **Tenancy resolver** — `subdomain` | `path` | `header` (only if multi-tenant).
- **Base domain** — e.g., `medicare.app` (only if multi-tenant).

Writes `APP_URL`, `MEDICARE_TENANT_RESOLVER`, `MEDICARE_BASE_DOMAIN`.

### 3.5 Step 5: Admin Account

- First name, last name
- Email
- Phone (with country code)
- Password + confirm
- Timezone (auto-detected from browser)

This is the **first** user. After install, they get the `super-admin` role.

In multi-tenant mode: this user becomes super-admin of the **first** tenant. The system creates a default tenant called "Default Clinic" with this user as owner.

### 3.6 Step 6: Email & Notifications

- **From name** (e.g., "Medicare")
- **From address** (e.g., `no-reply@medicare.app`)
- **SMTP Host / Port / Username / Password** (or select a provider preset: Mailgun, SES, Postmark)
- "Send test email" button → sends to the admin's email

If skipped, the system uses `log` mailer (writes to `storage/logs/laravel.log` instead of sending). User can configure later.

### 3.7 Step 7: Integrations (Optional, Skippable)

Each is a collapsible section with "Skip" and "Configure":

- **Firebase** (for OTP + FCM) — paste service account JSON
- **Razorpay** — Key ID + Secret + Webhook Secret
- **Stripe** — Publishable + Secret + Webhook Secret
- **Paystack** — Public + Secret + Webhook Secret
- **Zoom** — Account ID + Client ID + Client Secret
- **AWS S3** — Access Key + Secret + Region + Bucket
- **Sentry** — DSN

User can install with no integrations and add them later in admin Settings.

### 3.8 Step 8: Review & Install

A summary of all choices. "Install" button triggers:

```php
class Installer {
    public function install(array $data): void {
        DB::transaction(function () use ($data) {
            $this->writeEnv($data);
            $this->runMigrations();
            $this->runSeeders();
            $this->createAdminUser($data);
            $this->createFirstTenant($data);  // if multi-tenant
            $this->installDefaultSettings();
            $this->markInstalled();
        });
    }
}
```

A progress bar shows:
- Writing environment... ✓
- Running migrations... ✓ (10/83)
- Seeding permissions... ✓
- Creating admin user... ✓
- Creating first tenant... ✓
- Installing default settings... ✓
- Done!

Then: "Welcome to Medicare. [Go to Admin →]"

---

## 4. Routes

```php
// routes/install.php
Route::prefix('install')->name('install.')->group(function () {
    Route::get('/', [InstallController::class, 'welcome'])->name('welcome');
    Route::get('/environment', [InstallController::class, 'environment'])->name('environment');
    Route::post('/environment/check', [InstallController::class, 'checkEnvironment'])->name('check');
    Route::get('/database', [InstallController::class, 'database'])->name('database');
    Route::post('/database/test', [InstallController::class, 'testDatabase'])->name('database.test');
    Route::post('/database/save', [InstallController::class, 'saveDatabase'])->name('database.save');
    Route::get('/app', [InstallController::class, 'app'])->name('app');
    Route::post('/app/save', [InstallController::class, 'saveApp'])->name('app.save');
    Route::get('/admin', [InstallController::class, 'admin'])->name('admin');
    Route::post('/admin/save', [InstallController::class, 'saveAdmin'])->name('admin.save');
    Route::get('/email', [InstallController::class, 'email'])->name('email');
    Route::post('/email/test', [InstallController::class, 'testEmail'])->name('email.test');
    Route::post('/email/save', [InstallController::class, 'saveEmail'])->name('email.save');
    Route::get('/integrations', [InstallController::class, 'integrations'])->name('integrations');
    Route::post('/integrations/save', [InstallController::class, 'saveIntegrations'])->name('integrations.save');
    Route::get('/review', [InstallController::class, 'review'])->name('review');
    Route::post('/install', [InstallController::class, 'install'])->name('run');
    Route::get('/done', [InstallController::class, 'done'])->name('done');
});
```

All routes are `🔓` and only available when `app.installed=false`.

---

## 5. The `Installed` Cache Key

A simple Redis (or file) cache key:

```php
class InstallationState {
    public function isInstalled(): bool {
        return Cache::get('app.installed', false) === true;
    }
    
    public function markInstalled(): void {
        Cache::forever('app.installed', true);
    }
    
    public function reset(): void {  // for testing
        Cache::forget('app.installed');
    }
}
```

In production, you can also write a file marker (`storage/installed`) for redundancy.

---

## 6. Re-Install / Upgrade Mode

The installer is one-shot. To re-run (e.g., to change DB):
1. CLI: `php artisan install:reset` (wipes DB and cache key).
2. Or manually: `php artisan migrate:fresh` + delete `storage/installed`.
3. Visit `/install`.

The installer also runs on every deploy as a check:
- If `app.installed=false`, the deploy script runs `php artisan install:resume` which validates state and either completes install or fails loudly.

---

## 7. The `install:resume` Artisan Command

For unattended installs (e.g., Terraform provisions a server, runs the installer via API):

```bash
php artisan install:resume \
    --db-host=db.internal \
    --db-name=medicare \
    --db-user=medicare \
    --db-pass=secret \
    --admin-email=admin@medicare.app \
    --admin-password=... \
    --tenancy=single \
    --app-url=https://api.medicare.app
```

This is the scriptable version. The web UI is a friendly wrapper around the same logic.

---

## 8. The "Domain Activation" Concept

The original CodeCanyon Medicare has a domain-activation step where you enter a purchase code. We **do not** enforce a license in the open-source build, but the structure supports it for a future commercial SKU:

```php
class LicenseActivator {
    public function activate(string $purchaseCode, string $domain): LicenseStatus {
        // Hit our license server
        $response = Http::post('https://license.medicare.app/api/activate', [
            'purchase_code' => $purchaseCode,
            'domain' => $domain,
            'app_version' => config('medicare.version'),
        ]);
        if ($response->successful()) {
            License::create([
                'purchase_code' => $purchaseCode,
                'domain' => $domain,
                'activated_at' => now(),
                'expires_at' => $response->json('expires_at'),
            ]);
            return LicenseStatus::Valid;
        }
        return LicenseStatus::from($response->json('status'));
    }
}
```

In OSS mode, the step is skipped. In commercial mode, it's mandatory.

---

## 9. After Install

The admin lands on the admin login page. First login:
- Force change password (if temp password was used).
- 2FA setup recommended.
- Lands on dashboard.
- Sees a "Welcome wizard" overlay (3-5 steps to set up clinic basics).

---

## 10. Multi-Tenant Signup (SaaS Mode)

In multi-tenant mode, the **installer** runs once on the apex domain. Subsequent tenants are created via the public signup form or by super-admin invitation.

### 10.1 Public Signup Form

```
https://medicare.app/signup
```

Fields:
- Clinic name
- Your name, email, phone
- Country
- Plan (Free trial / Pro)

Submission:
1. Create `Tenant` (status=trial).
2. Create `TenantInvitation` for the admin email.
3. Send email with "Set up your account" link.
4. Tenant DNS: instructions to point `{clinic-slug}.medicare.app` via CNAME.

### 10.2 First Login of a New Tenant Admin

Clicks email link → `https://{clinic-slug}.medicare.app/setup`:
- Forced to set password.
- Walks through the clinic setup wizard (departments, services, doctors, branding).
- Tenant status remains `trial` for 14 days, then `active` (or `suspended` if not paid in V2).

---

## 11. The Installer's Security Model

### 11.1 No Auth Required

Anyone can hit `/install` while not installed. This is **intentional** — you need to install before you have users.

### 11.2 Lock After Install

Once installed, the installer routes are 404. The cache key check is in a global middleware:

```php
class RedirectIfNotInstalled {
    public function handle($request, $next) {
        if (!InstallationState::isInstalled() && !$request->is('install*')) {
            return redirect('/install');
        }
        if (InstallationState::isInstalled() && $request->is('install*')) {
            return redirect('/');
        }
        return $next($request);
    }
}
```

### 11.3 CSRF on the Installer

The installer uses CSRF tokens on all forms (Laravel default). The CSRF middleware skips the install routes because they have no session yet, but we use a per-install token in a hidden form field.

### 11.4 Re-Install Requires File Permission

To re-run, you need write access to `.env` and `storage/installed`. On a properly-secured server, only the deploy user has this. So an attacker who can't write to those files can't re-install.

---

## 12. Testing the Installer

### 12.1 Unit Tests

- `EnvironmentCheckerTest` — each check returns correct ok/not-ok.
- `LicenseActivatorTest` — mocked license server.

### 12.2 Feature Tests

- Full install flow with fake DB.
- Multi-tenant install creates default tenant.
- Re-install resets state correctly.

### 12.3 E2E Test

In CI, spin up a fresh Hetzner server and run `install:resume` against it. Smoke test the API after. This catches "works on my machine" issues.

---

## 13. The Installer UI

The web installer is a single-page app (Vue 3 or Alpine.js for minimal bundle). State is kept in a single JSON object that gets sent to the server on each step's "Save".

The UI:
- Stepper on the left (1-8)
- Form on the right
- "Back" / "Next" buttons
- "Skip" for optional steps
- Saves to local storage on every change (resume after refresh)

---

## 14. Acceptance Criteria (V1)

- [ ] Installer completes in <5 min on a fresh server.
- [ ] All 8 steps work and validate input.
- [ ] Environment check catches all common misconfigurations with helpful messages.
- [ ] Single-tenant install: app runs at the configured URL with no subdomain.
- [ ] Multi-tenant install: app runs at apex; a new tenant can be created via signup.
- [ ] Re-install is possible (with manual reset) for development.
- [ ] Installer is locked once `app.installed=true`.
- [ ] All integration fields are skippable; defaults work for dev.
- [ ] Test email button verifies SMTP config without committing.
- [ ] License activation hook exists and is a no-op in OSS mode.

---

## 15. V2 Installer

> V1 ships an 8-step installer. V2 adds **plugin selection**, **plan selection**, **region selection**, **post-install feature flagging**, and **automated provisioning** for V2 SaaS tenants.

### 15.1 V2 installer flow (11 steps)

```
Step 1:  Welcome & mode (single-tenant / multi-tenant / multi-region SaaS)
Step 2:  License activation (V2 SaaS only)
Step 3:  Region selection (V2 SaaS only — us-east-1, eu-west-1, ap-south-1, ap-southeast-1)
Step 4:  Plan selection (V2 SaaS only — Starter, Pro, Clinic, Enterprise; trial 14 days)
Step 5:  Database (V1 step 2)
Step 6:  Cache & queue (V1 step 3)
Step 7:  Storage (V1 step 4)
Step 8:  Integrations (V1 step 5)
Step 9:  Plugin marketplace (V2 NEW — curated list of plugins, pick at install)
Step 10: Branding & locale (V1 step 6)
Step 11: Admin account (V1 step 7)
Step 12: Final review & install (V1 step 8)
```

**Steps 2, 3, 4, 9 are V2-only** and skipped in V1 self-hosted installs.

### 15.2 V2 license activation

```php
// app/Console/Commands/ActivateLicense.php
class ActivateLicense extends Command {
    protected $signature = 'medicare:license:activate {key}';
    public function handle(LicenseService $svc) {
        $result = $svc->activate($this->argument('key'));
        if ($result->valid) {
            $this->info("License activated. Plan: {$result->plan}, Seats: {$result->seats}");
            $this->setEnv('MEDICARE_LICENSE_KEY', $this->argument('key'));
            $this->setEnv('MEDICARE_PLAN', $result->plan);
        } else {
            $this->error("Invalid license: {$result->reason}");
        }
    }
}
```

- **License server:** Laravel-based, internal, 99.95% SLA.
- **License contents:** `{tenant_slug, plan, seats, features[], valid_until, signature}`.
- **Verification:** public key pinned in app, signature verified on every API call.
- **Offline grace:** 7 days; after that, app degrades to read-only.

### 15.3 V2 region selection

For V2 SaaS installs, region is chosen at install time. Data residency is **immutable** post-install.

```php
// app/Installer/Steps/RegionStep.php
class RegionStep {
    public function rules(): array {
        return [
            'region' => ['required', Rule::in(['us-east-1', 'us-west-2', 'eu-west-1', 'eu-central-1', 'ap-south-1', 'ap-southeast-1', 'ap-southeast-2'])],
        ];
    }
    public function execute(array $data): void {
        config(['medicare.region' => $data['region']]);
        // Validate the region's compliance (EU requires GDPR, India requires DPDP)
        ComplianceChecker::validateRegionForTenant($data['region'], $data['country'] ?? null);
    }
}
```

### 15.4 V2 plan selection

| Plan | Price | Includes | Trial |
|---|---|---|---|
| **Starter** | Free | 1 doctor, 100 appts/mo, V1 only | 14 days Pro |
| **Pro** | ₹2,499/mo | 5 doctors, all V1.1 features, 1 V2 feature free | 14 days |
| **Clinic** | ₹9,999/mo | 25 doctors, all V1.1 + most V2, chain, priority support | 14 days |
| **Enterprise** | Custom | Unlimited, SSO, BYOK, custom SLA, all features | 30 days |

The installer writes `tenants.plan_id`, `tenants.subscription_status='trialing'`, `trial_ends_at=now()+14d`.

### 15.5 V2 plugin marketplace

A new installer step shows a curated list of plugins:

```
┌─ Plugin Marketplace ─────────────────────────────────────┐
│                                                         │
│  ☑ Drug Interaction DB              [Free] [Pro]        │
│  ☑ WhatsApp Business                [Pro]                │
│  ☐ AI Symptom Checker               [Enterprise]  🔒    │
│  ☐ Telemedicine Video               [Pro]                │
│  ☐ Pathology Lab                    [Pro]                │
│  ☐ Insurance Pre-Auth               [Enterprise]  🔒    │
│  ☐ AI Clinical Scribe               [Enterprise]  🔒    │
│  ☐ In-App Chat                      [Pro]                │
│                                                         │
│  Your plan: Pro → 2 plugins free, others disabled.      │
│                                                         │
│  [Continue →]                                           │
└─────────────────────────────────────────────────────────┘
```

For each picked plugin:
1. Check if plan permits (Pro can use Pro plugins, locks Enterprise).
2. Add to `plugin_installs`.
3. Run the plugin's migrations (in a transaction).
4. Set the corresponding feature flag.
5. Prompt for plugin-specific config (e.g., WhatsApp needs phone number ID).

For un-picked plugins, they're still installed but disabled (can be enabled later from admin UI).

### 15.6 Post-install feature flagging

V1 ships all V1 features on by default. V2 ships V2 features **off by default** unless enabled at install or via admin UI.

`FeatureFlagService::seedForTenant($tenant)` creates the default flag set:
```php
[
    'recurring-appointments' => true,        // V1.1, free
    'group-appointments' => true,            // V1.1, Pro
    'family-booking' => true,                // V1.1, free
    'whatsapp-business' => false,            // V1.1, Pro, needs config
    'telemedicine-video' => false,           // V2, Pro
    'ai-symptom-checker' => false,           // V2.1, Enterprise
    'pathology-lab' => false,                // V2, Pro
    'in-app-chat' => true,                   // V2, Pro
    'drug-interaction-db' => false,          // V2, Pro
    'custom-domain' => false,                // V2, Enterprise
    // ... 30+ more
];
```

Admin can change at any time. Each enable triggers any required migrations + setup.

### 15.7 Automated provisioning for V2 SaaS

When a new tenant signs up at `medicare.app/start`:
1. Marketing site collects `{name, email, clinic_name, country}`.
2. API creates a tenant in `status='provisioning'`.
3. Async `ProvisionTenantJob` runs:
   a. Allocate RDS database (or schema).
   b. Create ElastiCache Redis.
   c. Create S3 bucket.
   d. Create CloudFront distribution.
   e. Issue ACM cert for `{slug}.medicare.app`.
   f. Configure Route53.
   g. Run V1 + selected V2 migrations.
   h. Seed default data (currencies, languages, admin user).
   i. Create subscription (trialing).
   j. Send welcome email.
4. Status → `active`. Tenant can log in.

Total provisioning time: **<10 min** for standard plans. **<30 min** for Enterprise (with custom domain).

### 15.8 CLI installer for Terraform / Ansible

```bash
# V2 SaaS install via CLI (no wizard)
php artisan medicare:install \
  --mode=multi-tenant \
  --region=ap-south-1 \
  --plan=pro \
  --db-host=medicare-rds.cluster-abc.ap-south-1.rds.amazonaws.com \
  --db-name=medicare \
  --redis=medicare-elasticache.abc.cache.amazonaws.com \
  --s3-bucket=medicare-prod-ap-south-1 \
  --admin-email=admin@clinic.com \
  --admin-name="Apollo Admin" \
  --plugins=recurring-appointments,whatsapp-business \
  --no-interaction

# For Terraform/Ansible: --no-interaction reads from env
MEDICARE_DB_HOST=... php artisan medicare:install --no-interaction
```

`php artisan medicare:install --resume` continues a previous incomplete install.

### 15.9 V2 installer UI sketch

Step 9 (plugin marketplace) is the most novel:

```
┌─ Medicare Installer ─ Step 9 of 12 ─────────────────────┐
│                                                         │
│  ●─●─●─●─●─●─●─●─◉─○─○─○                                │
│  1  2  3  4  5  6  7  8  9 10 11 12                     │
│                                                         │
│  ┌─ Plugin Marketplace ─────────────────────────────┐    │
│  │                                                  │    │
│  │  Search: [____________________] 🔍              │    │
│  │                                                  │    │
│  │  Category: [All ▾]   Sort: [Popular ▾]          │    │
│  │                                                  │    │
│  │  ┌────────────────────────────────────────┐     │    │
│  │  │ 📦 Drug Interaction DB        Pro  [☑] │     │    │
│  │  │ Real-time drug-drug interaction check  │     │    │
│  │  │ for safe prescriptions.                │     │    │
│  │  │ 14,250 interactions • 0 dependencies   │     │    │
│  │  └────────────────────────────────────────┘     │    │
│  │  ┌────────────────────────────────────────┐     │    │
│  │  │ 📞 WhatsApp Business           Pro  [☑] │     │    │
│  │  │ Send appointment reminders and          │     │    │
│  │  │ prescriptions via WhatsApp.             │     │    │
│  │  │ ⚠️ Requires: phone number ID,           │     │    │
│  │  │ business account, approved templates.   │     │    │
│  │  │ [Configure after install →]              │     │    │
│  │  └────────────────────────────────────────┘     │    │
│  │  ...                                             │    │
│  │                                                  │    │
│  │  Selected: 2 plugins (within your Pro plan).    │    │
│  │                                                  │    │
│  │  [← Back]              [Skip]      [Continue →]  │    │
│  └──────────────────────────────────────────────────┘    │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

### 15.10 V2 acceptance criteria

- [ ] V2 installer completes in <10 min for SaaS tenants.
- [ ] License verification online + offline (7-day grace).
- [ ] Region selection is enforced; cannot be changed post-install.
- [ ] Plan selection creates a trialing subscription.
- [ ] Plugin selection triggers migrations + feature flags.
- [ ] Automated provisioning completes in <30 min.
- [ ] CLI install supports Terraform/Ansible.
- [ ] Re-install requires explicit `medicare:install --reset`.
- [ ] All V1 acceptance criteria still pass.
