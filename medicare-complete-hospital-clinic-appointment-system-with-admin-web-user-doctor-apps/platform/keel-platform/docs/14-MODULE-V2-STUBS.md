# 14 · V2 Master Integration Map

> **Status:** Living design document. Updated every release cycle.
> **Owner:** Architecture Guild
> **Audience:** Tech leads, product managers, all module owners.
> **Goal:** One page that explains (1) what V2 is, (2) which V2 feature touches which V1 module, (3) how to enable each V2 feature, (4) release ordering and dependencies, (5) cross-cutting concerns (feature flags, data model, billing, security).

---

## Table of Contents
1. [What is V2?](#1-what-is-v2)
2. [Release Lanes](#2-release-lanes-v11-v2-v21-v3-v4)
3. [V2 Feature Catalog](#3-v2-feature-catalog)
4. [V1 → V2 Module Touchpoints](#4-v1--v2-module-touchpoints)
5. [Feature Flag Strategy](#5-feature-flag-strategy)
6. [Data Model Additions](#6-data-model-additions)
7. [Billing & Subscription](#7-billing--subscription)
8. [Security, Privacy, Compliance](#8-security-privacy-compliance)
9. [Observability & SLOs](#9-observability--slos)
10. [Open Questions](#10-open-questions)

---

## 1. What is V2?

V2 is the umbrella label for every **post-V1-GA** capability that is **not strictly required to book, see, prescribe, or bill** a patient. V1 ships a working clinic out of the box. V2 turns that clinic into a **digital health platform** with third-party integrations, intelligence, and a SaaS revenue layer.

### 1.1 Guiding principles
- **Backward compatibility is sacred.** Every V2 feature is opt-in via feature flag. No V1 tenant is forced to upgrade.
- **Idempotent migrations.** V2 tables land behind a `feature_*` flag and a separate migration directory; rolling back V2 leaves V1 data untouched.
- **Composable, not monolithic.** Each V2 feature ships as an independent module with its own `routes/`, `services/`, `events/`, and `policies/`. The monolith stays cohesive, not coupled.
- **Pay for what you use.** Plugins & Marketplace (§3.3a) ships as one runtime (V2) with two surfaces added later: public catalog (V3) and monetization (V3 on the V2.1 subscription engine). Tenants pay only for the plugins they activate.
- **Security by default.** V2 features must pass the same threat model gate as V1; PHI never leaves the tenant boundary without explicit consent.

### 1.2 Non-goals (V2 will not do)
- **Replace the EMR.** We are an OPD / appointment / prescription / billing platform. We are not a full inpatient EMR. We integrate with Epic / Cerner via FHIR (V3).
- **Build a PACS.** We integrate with third-party DICOM viewers; we do not store imaging in our database.
- **Run our own LLM.** We call OpenAI, Anthropic, or on-prem vLLM. We do not fine-tune or train medical models in V2.
- **Run a marketplace of clinics.** V2 supports multi-clinic chain; we are not a Yelp-equivalent.

---

## 2. Release Lanes (V1.1, V2, V2.1, V3, V4)

| Lane | Target Date | Theme | Key Features | Breaking? |
|---|---|---|---|---|
| **V1** | Month 0 (GA) | Working clinic | Book, see, prescribe, bill, multi-tenant | No |
| **V1.1** | Month +2 | Polishing | WhatsApp notif, recurring appts, group appts, family booking, multi-currency payout, advanced refund windows, geo-search, voucher codes for marketing | No |
| **V1.2** | Month +4 | Native integrations | ABDM/ABHA bridge (India), FHIR R4 read endpoint, Stripe Terminal (in-person card), Razorpay Subscriptions, Paystack Subscriptions | No |
| **V2** | Month +7 | Platform layer | Pathology/Lab orders, in-app video, AI symptom checker, AI clinical scribe, in-app chat, plugins v1, custom-domain provisioning, multi-region DR | No |
| **V2.1** | Month +10 | Insurance & revenue cycle | Insurance pre-auth, E&O claims, denial management, recurring revenue, usage-based pricing for AI tokens, revenue dashboards | No |
| **V3** | Month +15 | Marketplace & ecosystem | Plugin marketplace, HL7 v2 ADT feeds, FHIR write, third-party DICOM viewer, on-prem AI gateway, custom report builder | No |
| **V4** | Month +22 | Hospital ITOps | OT room scheduling, bed/ward management, lab instrument interfaces (ASTM/HL7), centralized HIS, hospital revenue dashboards | No |
| **V5+** | TBD | Population health | De-identified cohort queries (per `06-Patients §17.7`), public health reporting, claim outcome analytics | TBD |

> **Compatibility promise:** From V1 GA → V5, no breaking API changes. New versions are additive, deprecations announced 12 months in advance, removed only at the next major lane (V6).

---

## 3. V2 Feature Catalog

Each feature has a **slug**, a **lane**, a **tier** (free / pro / enterprise), a **scope** (V1 module it extends), an **owner**, and a **Maturity Model Level (MML)** 1–5:

- **MML 1:** Designed (this doc, no code)
- **MML 2:** Schema only (`migrations/v2/...`), no API
- **MML 3:** Internal API + admin UI
- **MML 4:** Patient/doctor-facing API + Flutter/Next.js clients
- **MML 5:** GA, SLA-backed, observability live, runbook published

### 3.1 V1.1 Lane Features

| Slug | Name | Tier | V1 Scope | MML@GA | Owner |
|---|---|---|---|---|---|
| `whatsapp-business` | WhatsApp Business notifications | Pro | `11-Notifications` | 4 | Notifications guild |
| `recurring-appointments` | Recurring weekly/monthly appts | Free | `07-Appointments` | 4 | Appointments guild |
| `group-appointments` | Group therapy / batch consults | Pro | `07-Appointments` | 4 | Appointments guild |
| `family-booking` | Book for dependents | Free | `06-Patients`, `07-Appointments` | 4 | Patients guild |
| `multi-currency-payout` | Doctor payout in any of 25 currencies | Pro | `09-Billing`, `10-Payments` | 3 | Payments guild |
| `geo-search` | Mapbox "find doctor near me" | Pro | `05-Doctors`, `12-Engagement` | 4 | Doctors guild |
| `marketing-vouchers` | Public promo codes, referral bonuses | Pro | `09-Billing` | 3 | Billing guild |
| `dynamic-refund-window` | Per-clinic refund policy | Pro | `07-Appointments`, `09-Billing` | 3 | Appointments guild |
| `smart-waitlist` | Auto-fill cancellations from waitlist | Pro | `07-Appointments` | 3 | Appointments guild |
| `i18n-rtl` | Right-to-left languages (Arabic, Hebrew) | Free | `12-Engagement` | 4 | Engagement guild |
| `doctor-rotating-schedule` | Cover-doctor / on-call roster | Pro | `05-Doctors`, `07-Appointments` | 3 | Doctors guild |
| `pwa-offline-mode` | Patient app offline check-in | Free | `medicare-user-app` | 4 | Mobile guild |

### 3.2 V1.2 Lane Features

| Slug | Name | Tier | V1 Scope | MML@GA | Owner |
|---|---|---|---|---|---|
| `abdm-bridge` | ABDM/ABHA health ID (India) | Pro | `02-Auth`, `06-Patients` | 3 | Integrations guild |
| `fhir-r4-read` | FHIR R4 read endpoint (Patient, Practitioner, Encounter, Observation) | Pro | `16-API-Reference` | 3 | Integrations guild |
| `razorpay-subscriptions` | Razorpay Subscription API for SaaS plans | Pro | `10-Payments` | 3 | Payments guild |
| `paystack-subscriptions` | Paystack Subscription API | Pro | `10-Payments` | 3 | Payments guild |
| `stripe-terminal` | Stripe Terminal in-person card reader | Pro | `10-Payments` | 4 | Payments guild |
| `twilio-sms` | Twilio SMS OTP and notifications | Free | `02-Auth`, `11-Notifications` | 4 | Notifications guild |
| `local-payment-methods` | UPI Autopay, Paytm, GCash | Pro | `10-Payments` | 3 | Payments guild |
| `multi-clinic-dashboard` | Chain admin sees all branches | Pro | `04-Clinic-Structure`, `13-Settings-Reports` | 3 | Admin guild |

### 3.3 V2 Lane Features (the marquee)

| Slug | Name | Tier | V1 Scope | MML@GA | Owner |
|---|---|---|---|---|---|
| `pathology-lab` | Lab orders, sample tracking, partner flow, LOINC results, e-Reports — see `08-Prescriptions §16.3` deep-dive | Pro | `06-Patients`, `08-Prescriptions`, `09-Billing`, `15-DB-Schema` | 4 | Lab guild |
| `telemedicine-video` | In-app WebRTC video consult (LiveKit primary, Daily/Jitsi fallback) — see `07-Appointments §19.3` deep-dive | Pro | `07-Appointments`, `17-Realtime` | 4 | Telemedicine guild |
| `ai-symptom-checker` | Pre-visit LLM symptom triage with red-flag detection — see `18-User-Flows §19.7` deep-dive | Enterprise | `18-User-Flows`, `19-Integrations` | 3 | AI guild |
| `ai-clinical-scribe` | Doctor–patient audio → STT → SOAP note + Rx auto-draft — see `08-Prescriptions §16.4` deep-dive | Enterprise | `06-Patients`, `08-Prescriptions` | 3 | AI guild |
| `in-app-chat` | Patient↔doctor realtime chat with attachments, voice, chart promotion — see `11-Notifications §20.4` deep-dive | Pro | `11-Notifications`, `17-Realtime` | 4 | Realtime guild |
| `plugins-v1` | Plugin Core runtime (install, lifecycle, hooks, sandbox, capability model) — see §3.3a + §3.3b runtime mechanics | Free framework | All modules | 3 | Platform guild |
| `custom-domain` | Tenant CNAME + ACM cert + CloudFront — see `03-Tenancy §17.4` deep-dive | Enterprise | `03-Tenancy`, `21-Deployment` | 3 | Platform guild |
| `multi-region-dr` | Active-passive DR in second region — see `21-Deployment §18.5` deep-dive | Enterprise | `21-Deployment`, `20-Security` | 3 | Platform guild |
| `drug-interaction-db` | RxNav/DrugBank/WHO/tenant-curated multi-source real-time interaction check — see `08-Prescriptions §16.1` deep-dive | Pro | `08-Prescriptions` | 4 | Clinical guild |
| `e-prescription` | National e-prescription formats (ABDM/SCRIPT/EPS/NHS/ADHA) with country-appropriate signing — see `08-Prescriptions §16.2` deep-dive | Pro | `08-Prescriptions` | 3 | Clinical guild |
| `insurance-preauth` | Payer integration: eligibility 270, preauth 278/NHCX, claim 837, ERA 835, denial mgmt — see `09-Billing §21.3` deep-dive | Enterprise | `09-Billing`, `18-User-Flows` | 3 | RCM guild |
| `revenue-cycle-mgmt` | Denial mgmt + appeals, dunning state machine, payment plans, underpayment detection, A/R aging — see `09-Billing §21.4` deep-dive | Enterprise | `09-Billing`, `13-Settings-Reports` | 3 | RCM guild |
| `bed-management` | Wards, beds, assignments, occupancy — see `04-Clinic-Structure §14.3` deep-dive | Enterprise | `04-Clinic-Structure`, `06-Patients` | 3 | Hospital guild |
| `ot-room-scheduling` | Operation Theatre scheduling, surgical notes, utilization — see `04-Clinic-Structure §14.4` deep-dive | Enterprise | `04-Clinic-Structure`, `07-Appointments` | 3 | Hospital guild |

### 3.3a Plugins & Marketplace — Unified Architecture

The plugin system, public marketplace, and plugin monetization are **one architecture with three release surfaces**, not three separate products.

| Surface | Lane | Slug | What it adds on top of the previous |
|---|---|---|---|
| **Plugin Core** | V2 | `plugins-v1` | Runtime only: install, lifecycle, hooks, sandbox, capability model, audit. No catalog, no billing. |
| **Public Marketplace** | V3 | `plugin-marketplace` | Tenant-facing catalog of third-party plugins; one-click install via Plugin Core; ratings and reviews. |
| **Plugin Monetization** | V3 (reuses V2.1 §7.2 subscription engine) | `plugin-billing` | Per-plugin pricing, revenue share (70/30 to developer per §10 Q10), subscription item on tenant invoice. |

**Why one architecture:** three independent code paths would yield three different permission models, three different audit-log shapes, and three different install procedures. As one runtime + three surfaces, the implementation is one `plugins` table set, one `PluginManager` service, and three UI surfaces (admin enable/disable toggle, public catalog, billing line item).

**Dependencies:**
- Plugin Core (V2) → no internal dependency; ships behind `feature_plugins` flag.
- Public Marketplace (V3) → Plugin Core + Stripe/Razorpay (already V1) for paid plugins.
- Plugin Monetization (V3) → Plugin Core + V2.1 subscription engine (§7.2).

**What is NOT a plugin** (built-in, not extensible):
- Core data models (Patient, Doctor, Appointment, Prescription, Invoice, Payment)
- Auth & RBAC
- Audit log
- Tenancy resolver
- Payment gateway integrations (Razorpay, Stripe, Paystack)

Plugins **add** features (e.g., AI Scribe, lab partner connector, WhatsApp bot flow, custom report pack). They **do not replace** the core services above. This boundary is enforced by the capability model in the Plugin Core.

### 3.3b Plugin Core runtime mechanics (V2, MML 3)

**1. Overview.** The Plugin Core is the V2 runtime that the V3 Public Marketplace (Turn 6) and V3 Plugin Monetization extend. The Marketplace is the catalog; the Monetization is the billing; the Core is the **runtime** — install, lifecycle, hooks, capability model, sandbox, audit. This section documents the V2 mechanics in detail; Marketplace and Monetization each have their own deep-dive.

**2. Domain Model.** Per Turn 6 we already defined:
```php
plugins (id, tenant_id, name, slug, version, manifest, signature, status, ...)
plugin_versions (id, plugin_id, version, artifact_s3_key, sha256, signature, ...)
plugin_installs (id, plugin_id, tenant_id, status, installed_at, enabled_at, ...)
plugin_licenses (id, plugin_install_id, license_key, expires_at, ...)
// plugin_hooks (id, plugin_id, hook_name, handler_class, priority) — runtime registry
// plugin_audit (id, tenant_id, plugin_id, hook, args_hash, result_status, duration_ms, occurred_at)
```

**3. Hook system.** A plugin declares the hooks it subscribes to in its `manifest.json`:
```json
{
  "name": "ai-scribe",
  "version": "1.0.0",
  "hooks": [
    {"name": "encounter.ended",       "handler": "AiScribePlugin\\OnEncounterEnded",       "priority": 50},
    {"name": "prescription.signing",  "handler": "AiScribePlugin\\OnPrescriptionSigning",  "priority": 50}
  ],
  "capabilities": ["read:encounter", "write:prescription.draft", "ai:complete"],
  "settings_schema": "json-schema-url"
}
```
The Core registers the hook handlers in a `plugin_hooks` table at install time. When a core event fires (e.g., `event(new EncounterEnded($encounter))`), the Core looks up all matching handlers, sorts by priority, and invokes them **synchronously** in-process (per the V2 execution model — see point 4).

**4. Execution model (V2 = in-process; per `21-Deployment §18.2a`).** A plugin is a PHP class autoloaded via the `PluginManager` service. It runs in the same PHP-FPM worker that handled the originating API request. Constraints:
- **Timeout:** 5 s per hook invocation (enforced by a `pcntl_alarm` or `set_time_limit`).
- **Memory:** 32 MB cap (verified by `memory_get_peak_usage` before/after).
- **Exception isolation:** all exceptions are caught at the hook boundary; an uncaught exception in a plugin does not 500 the request; it is logged and surfaced as a `plugin_audit.result_status='plugin_error'`.
- **Concurrency:** V2 plugins are stateless and re-entrant; multiple PHP-FPM workers may invoke the same plugin concurrently without coordination.

**5. Capability model.** Capabilities are an **allowlist of method calls** the plugin may make, not just an RBAC role. Examples:
- `read:patient.demographics` — may call `Patient::find($id)->only(['name','dob',...])`.
- `write:prescription.draft` — may call `Prescription::createDraft(...)`.
- `ai:complete` — may call `LlmService::complete(...)` (token-billable).
- `http:outbound` — may call `Http::get/post` (URL allowlist required).
- `storage:read`, `storage:write` — S3 read/write to a per-plugin prefix.

The Core wraps each `read`/`write` service with a `CapabilityGuard` that throws `CapabilityDenied` if the plugin does not hold the capability. Capabilities are declared in the manifest and **granted at install time by the tenant admin** (the install wizard shows the capability list and the admin must click "I approve" for each). Capabilities are logged on every invocation; the tenant admin can revoke a capability later (which disables the plugin on next hook call).

**6. Lifecycle.**
```
discovered → installed (manifest accepted) → enabled (hooks registered) → running → disabled (hooks unregistered) → uninstalled (artifacts removed)
```
A "kill switch" sets `plugin_installs.status='disabled'`; the Core refuses to register hooks for that install. A tenant admin may also set `status='quarantined'` while investigating; quarantined plugins keep their state but do not run.

**7. API Surface.**
```
GET    /api/v2/plugins                                     # list tenant's installed plugins
POST   /api/v2/plugins/install                             # upload manifest + artifact (signed)
POST   /api/v2/plugins/{id}/enable
POST   /api/v2/plugins/{id}/disable
POST   /api/v2/plugins/{id}/uninstall
GET    /api/v2/plugins/{id}/audit                          # hook invocation log
GET    /api/v2/plugins/hooks?hook=encounter.ended          # which plugins are subscribed
POST   /api/v2/plugins/{id}/settings                       # update tenant-scoped settings
POST   /api/v2/plugins/{id}/upgrade                        # new version
```

**8. Frontend.** A single admin page at `/admin/plugins/` with three tabs: **Installed**, **Discover** (loads from the V3 Marketplace catalog if the tenant is entitled, else shows a "Marketplace not enabled" notice), **Audit** (the `plugin_audit` log with filters by plugin/hook/result). Each installed plugin card shows version, capabilities (with a "Revoke" button per capability), status, last-run time, last-error, and a "Disable" toggle.

**9. Realtime.** Per `17-Realtime §18`, the `plugin.{tenant}` channel emits `installed`, `enabled`, `disabled`, `upgraded`, `health.degraded` events. The Marketplace and Monetization events piggyback on the same channel.

**10. Security.** Per `20-Security §23.2.3` (the 8-point plugin security model: capability, sandbox, integrity, code review, kill switch, audit, no exfil, tenancy isolation). The Core refuses to install an unsigned plugin; the signature must verify against a known developer public key (which itself was onboarded via the Marketplace's developer-portal flow in V3). The Core also refuses to load a plugin whose `manifest.json` declares a hook that the Core does not recognize (no shadow hooks); unknown hooks are an install-time error.

**11. Acceptance Criteria (V2 MML 3 GA).**
- [ ] A plugin can be installed, enabled, disabled, and uninstalled; each transition is audit-logged.
- [ ] A plugin invocation that exceeds the 5 s timeout is killed; the originating request still succeeds with a `200` and a `plugin_audit.result_status='timeout'` row.
- [ ] A plugin invocation that exceeds 32 MB is killed; same outcome as timeout.
- [ ] A plugin that throws an uncaught exception does not 500 the request; the audit row shows `plugin_error`.
- [ ] A plugin without `read:patient.demographics` cannot read patient demographics; attempting to do so throws `CapabilityDenied` and the audit row shows `denied`.
- [ ] A tenant admin can revoke a capability; the next hook call fails closed.
- [ ] Two tenants' plugin installs are isolated: tenant A cannot see, install, upgrade, or audit tenant B's plugins (RBAC tested).
- [ ] Unsigned plugin uploads are rejected with 422 and a clear error.

**12. Real-World Test Scenarios.**
- *AI Scribe plugin:* an encounter ends; the `encounter.ended` hook fires; the AI Scribe plugin drafts a SOAP section; the draft is written via `write:encounter.section`; the doctor reviews and signs. The whole flow is in-process; no IPC.
- *Timeout:* a malicious plugin enters an infinite loop; the Core's `set_time_limit(5)` kills it; the request returns 200 with a "Plugin X timed out" toast; the plugin is auto-quarantined by the kill-switch rule.
- *Capability escalation:* a plugin attempts to call `Patient::all()` without `read:patient.list`; the `CapabilityGuard` throws; the audit row shows `denied`; the tenant admin is emailed.
- *Upgrade rollback:* plugin v1.2.0 has a bug; tenant admin upgrades to v1.2.1, sees the bug, rolls back to v1.1.0; `plugin_versions` is append-only; the rollback is a new install pointing at the older version.
- *Tenancy leak attempt:* a plugin tries to query `WHERE tenant_id != ?`; the Core's query interceptor rewrites the query to scope by `tenant_id`; the plugin receives an empty result and the audit row shows `tenancy_violation`.

**13. What this module is NOT.**
- Not a code-hosting platform. The Core expects a signed `.tar.gz` artifact (per `21-Deployment §18.2a`); it does not git-clone.
- Not a marketplace. That is V3 `plugin-marketplace`. The V2 Core has no catalog; the admin uploads the artifact manually or it is provided by the V3 Marketplace.
- Not a billing engine. That is V3 `plugin-billing` (V2.1 subscription engine reuse).

### 3.3c SaaS Plan Marketplace (`saas-plan-marketplace`, V2.1, MML 4)

**1. Overview.** The Plugin Core (V2) is the runtime; the Public Marketplace (V3) is the catalog of third-party plugins; **the SaaS Plan Marketplace (V2.1) is the billing plane** for paid plugins. It defines how a developer publishes a paid plugin (monthly subscription, per-tenant seat, or usage-based), how a tenant subscribes, how the invoice line item appears, and how revenue is split (70% to developer, 30% to platform per `14-V2-Stubs §10 Q10`). MML 4 GA covers: developer portal, plan publishing, tenant subscription, prorated billing, dunning on failed payments, and revenue-share payouts via Stripe Connect / Razorpay Route.

**2. Relationship to V2/V3 modules.**
- Builds on the V2 Plugin Core (the runtime that hosts the plugin).
- Builds on the V2.1 subscription engine (`09-Billing §21.x`) for recurring billing.
- Pre-requisite for V3 Plugin Monetization (which adds the public catalog UI); the V2.1 Marketplace is the data model + billing flow; V3 adds the customer-facing storefront.

**3. Domain Model.**
```php
Schema::create('marketplace_developers', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->nullable()->index();             // null = external developer org
    $t->string('name', 128);
    $t->string('contact_email', 128);
    $t->string('support_url', 255)->nullable();
    $t->string('payout_account_id', 64)->nullable();        // Stripe Connect acct_xxx / Razorpay Route rzp_xxx
    $t->string('payout_provider', 32);                      // 'stripe_connect' | 'razorpay_route' | 'manual'
    $t->json('tax_info')->nullable();                       // W-9 / W-8BEN / GSTIN / PAN
    $t->string('status', 16)->default('pending_verification');
    $t->timestamps();
});

Schema::create('marketplace_plans', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('developer_id')->index();
    $t->string('plugin_slug', 64)->index();                 // FK to plugins (V3 Marketplace)
    $t->string('name', 128);                                // 'AI Scribe Pro', 'Lab Connector Starter'
    $t->text('description');
    $t->string('pricing_model', 32);                        // 'flat_monthly' | 'per_doctor_monthly' | 'usage_based'
    $t->decimal('flat_price_usd', 10, 2)->nullable();
    $t->decimal('per_seat_usd', 10, 2)->nullable();
    $t->decimal('usage_unit_price_usd', 10, 6)->nullable(); // per token / per request
    $t->string('usage_unit', 16)->nullable();               // 'token' | 'request' | 'minute'
    $t->integer('trial_days')->default(0);
    $t->string('revenue_share_pct_to_dev', 5, 2)->default(70.00);
    $t->boolean('is_active')->default(true);
    $t->string('visibility', 16)->default('public');        // 'public' | 'invite_only' | 'deprecated'
    $t->json('entitlements')->nullable();                   // {max_doctors: 50, max_api_calls_per_month: 100000, ...}
    $t->timestamps();
});

Schema::create('marketplace_subscriptions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('plan_id')->index();
    $t->ulid('plugin_install_id')->nullable()->index();
    $t->string('external_subscription_id', 64)->nullable(); // Stripe sub_xxx / Razorpay sub_xxx
    $t->string('status', 16)->default('trialing');          // 'trialing' | 'active' | 'past_due' | 'cancelled' | 'expired'
    $t->integer('seats')->default(1);                       // for per-seat plans
    $t->decimal('mrr_usd', 10, 2);
    $t->timestamp('trial_ends_at')->nullable();
    $t->timestamp('current_period_start')->nullable();
    $t->timestamp('current_period_end')->nullable();
    $t->timestamp('cancelled_at')->nullable();
    $t->string('cancellation_reason', 64)->nullable();
    $t->timestamps();
});

Schema::create('marketplace_usage_records', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('subscription_id')->index();
    $t->ulid('tenant_id')->index();
    $t->decimal('quantity', 12, 4);
    $t->decimal('cost_usd', 10, 4);
    $t->string('unit', 16);
    $t->timestamp('recorded_at')->index();
});

Schema::create('marketplace_payouts', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('developer_id')->index();
    $t->string('period', 7)->index();                       // '2026-05'
    $t->decimal('gross_revenue_usd', 12, 2);
    $t->decimal('platform_fee_usd', 12, 2);
    $t->decimal('developer_payout_usd', 12, 2);
    $t->string('status', 16)->default('pending');           // 'pending' | 'processing' | 'paid' | 'failed'
    $t->string('payout_transaction_id', 64)->nullable();
    $t->timestamp('paid_at')->nullable();
    $t->timestamps();
});
```

**4. Developer flow.**
1. Developer registers at `developer.medicare.app` (a separate product, not the main admin).
2. KYC: tax info (W-9 / W-8BEN / GSTIN / PAN), bank account (via Stripe Connect / Razorpay Route), identity verification.
3. Once approved (`status='active'`), developer can publish plans.
4. For each plan: name, description, pricing model, trial days, revenue share (default 70/30, negotiated up to 80/20 for high-volume devs).
5. The plan is linked to a Plugin Core install (the plugin must already exist as a signed artifact; the marketplace entry is a billing wrapper).
6. The plan is published (`visibility='public'`) and appears in the V3 Marketplace catalog.

**5. Tenant flow.**
1. Tenant browses the V3 Marketplace → finds a paid plugin → clicks "Install".
2. Tenant picks a plan (Starter / Pro / Enterprise) and starts a trial (if any).
3. A `marketplace_subscriptions` row is created; status='trialing' until `trial_ends_at`.
4. After trial, status='active'; the tenant's `invoices` table gains a line item each period (per `09-Billing §21.x` subscription engine).
5. The plugin continues to run; its hook invocations are metered if the plan is `usage_based` (each call writes to `marketplace_usage_records`).
6. Tenant can cancel any time; cancellation takes effect at the end of the current period; the plugin is then disabled (per `20-Security §23.2.3`, the kill switch fires).

**6. Dunning.** Failed payment → status='past_due'; the subscription engine's dunning flow (3 retries: 3d, 7d, 14d) applies. After 3 failures → status='cancelled'; the plugin is uninstalled (with a 7-day grace period during which re-payment re-activates).

**7. Payouts.** Monthly: `GeneratePayoutsJob` rolls up `marketplace_subscriptions` × `mrr_usd` for the period; computes `developer_payout_usd = gross × (share_pct / 100)`; creates `marketplace_payouts` rows; initiates Stripe Connect transfer / Razorpay Route transfer. Failed transfers retry 3× then flag for manual ops review.

**8. API Surface.**
```
POST   /api/v2.1/marketplace/developers                       # apply
GET    /api/v2.1/marketplace/developers/me
GET    /api/v2.1/marketplace/plans?plugin=...                  # browse
POST   /api/v2.1/marketplace/plans                              # publish (developer-only)
PATCH  /api/v2.1/marketplace/plans/{id}                        # update pricing
GET    /api/v2.1/marketplace/subscriptions/me                   # tenant's active subs
POST   /api/v2.1/marketplace/subscriptions                     # subscribe { plan_id }
POST   /api/v2.1/marketplace/subscriptions/{id}/cancel
POST   /api/v2.1/marketplace/usage                              # report usage { quantity, unit }
GET    /api/v2.1/marketplace/payouts/me                         # developer's payouts
POST   /api/v2.1/marketplace/webhook                            # gateway webhook (subscription.created, payment_failed, etc.)
```

**9. Frontend.**
- **Developer portal** (`developer.medicare.app`): dashboard (MRR, active subscriptions, churn, payout history), plan editor (form with live preview), payout settings, KYC, support tickets.
- **Tenant UI** (V3 Marketplace storefront, not the V2.1 deep-dive): plan picker on install, "Manage subscription" link in `/admin/billing/subscriptions`, "Cancel" with reason survey.

**10. Realtime.**
- `marketplace.subscription.{created|trial_ending|active|past_due|cancelled}.{tenant_id}` — tenant notifications.
- `marketplace.payout.{pending|paid|failed}.{developer_id}` — developer dashboard.
- `marketplace.plugin.{installed|uninstalled|quarantined}.{tenant_id}.{plugin_slug}` — piggybacks on the existing `plugin.{tenant}` channel.

**11. Security & RBAC.** Roles: `developer` (own plans + own payouts; cannot see other devs' data), `developer_admin` (org-wide), `platform_admin` (approve developers, override disputes), `tenant_admin` (subscribe / cancel). Per `20-Security §11.4`, a high-value payout (>$10k) requires co-sign. Tax info is encrypted at rest; bank tokens are managed by the payment gateway (Stripe Connect / Razorpay Route) — we never see the bank account number. KYC documents are stored in a tenant-scoped S3 prefix with SSE-KMS. Revenue-share percentages are immutable per plan; a change requires a new plan version (existing subscribers stay on the old % for the remainder of their period).

**12. Acceptance Criteria.**
- [ ] A developer can publish a plan; the plan appears in the V3 catalog within 5 min.
- [ ] A tenant can install + start a trial; the trial ends at `trial_ends_at` and auto-converts to `active` (or `cancelled` if no payment method).
- [ ] A failed payment triggers the dunning flow; after 3 failures, the plugin is uninstalled.
- [ ] A monthly payout matches the sum of `mrr_usd × (share / 100)` for active subscriptions in the period; the platform fee is the remainder.
- [ ] A usage-based plan's `cost_usd` for the period matches the sum of `marketplace_usage_records.quantity × unit_price`.
- [ ] The developer portal never exposes another developer's data; the tenant UI never exposes a developer's payout split.

**13. Real-World Test Scenarios.**
- *Developer onboarding:* "Acme Health Labs" applies; submits W-9; Stripe Connect account is created; KYC passes; plan "Lab Connector Pro" is published at $99/mo flat.
- *Tenant subscribe:* Apollo Clinic installs "Lab Connector Pro"; 14-day trial; usage is metered; trial ends; $99 charged; subscription active.
- *Churn:* Apollo cancels; subscription active until period end; then `cancelled`; plugin uninstalled; audit row written.
- *Dunning:* card expires; charge fails; retry 1 (3d): fails; retry 2 (7d): fails; retry 3 (14d): fails; subscription `cancelled`; plugin uninstalled; tenant notified.
- *Payout dispute:* developer disputes a payout amount; platform_admin reviews; the original payouts are summed; the dispute is resolved; a corrective payout is issued.

**14. What this module is NOT:**
- Not an app store for end-users. The tenants install plugins; patients do not.
- Not a payment processor. We use Stripe Connect / Razorpay Route; we do not hold funds longer than the dunning window.
- Not a tax service. The developer is responsible for their own tax filing; we provide 1099 / GST data export.

**15. Dev Workflow.** `php artisan marketplace:simulate --developers=5 --plans=20 --tenants=50` synthetic marketplace load. `php artisan marketplace:payout-run --period=2026-05` runs the monthly payout. `php artisan marketplace:webhook-replay --provider=stripe` replays Stripe webhooks. Code: `app-modules/saas-plan-marketplace/`; tests: `app-modules/saas-plan-marketplace/tests/Feature/{Developer,Plan,Subscription,Payout}Test.php` and `tests/Performance/marketplace-load.k6.js`.

**Execution model** (refers §10 Q2): designed to support both sidecar and in-process. V2 ships the **in-process** runtime (faster, simpler ops, single deploy). Sidecar mode is gated by `feature_plugin_sidecar` and designed in V3. Default = in-process for V2; sidecar available in V3.

### 3.4 V3 Lane Features

| Slug | Name | Tier | V1 Scope | MML@GA | Owner |
|---|---|---|---|---|---|
| `plugin-marketplace` | Public marketplace surface (catalog, ratings, one-click install) — extends Plugin Core, see §3.3a | Free | All modules | 3 | Platform guild |
| `hl7-v2-adt` | HL7 v2 ADT/ORU ingress (Mirth + HAPI-FHIR + MLLP) — see `19-Integrations §15.10` deep-dive | Enterprise | `19-Integrations` | 3 | Integrations guild |
| `fhir-r4-write` | FHIR R4 write (SMART-on-FHIR, dual-write with conflict resolution) — see `19-Integrations §15.10a` deep-dive | Enterprise | `16-API-Reference`, `19-Integrations` | 3 | Integrations guild |
| `dicom-viewer` | OHIF viewer + Orthanc/HealthImaging/PACS partner integration — see `19-Integrations §15.9` deep-dive | Pro | `08-Prescriptions`, `19-Integrations` | 3 | Lab guild |
| `custom-report-builder` | Visual report builder (Metabase-style) + custom-SQL sandbox — see `13-Settings-Reports §13.2` deep-dive | Pro | `13-Settings-Reports` | 3 | Reports guild |
| `on-prem-ai-gateway` | _(moved to V2.1 lane — see note below)_ | | | | |
| `white-label-mobile` | Tenant-branded iOS/Android apps (EAS build + Submit) — see `19-Integrations §15.10b` deep-dive | Enterprise | `medicare-user-app` | 3 | Mobile guild |
| `sso-oidc` | OIDC + SAML 2.0 SSO with JIT + SCIM v2 (V3.1) — see `02-Auth §17.5` deep-dive | Enterprise | `02-Auth` | 3 | Auth guild |
| `equipment-booking` | Equipment time-slot booking + PM tracking — see `04-Clinic-Structure §14.5` deep-dive | Enterprise | `04-Clinic-Structure`, `07-Appointments` | 3 | Hospital guild |

### 3.4a V1.1 / V1.2 / V2 / V2.1 — Cross-references to all deep-dives

> **Scope of this table:** every post-V1 module that has a full deep-dive in a module doc. The deep-dive is canonical; this row is a pointer. For the per-lane catalog (which modules are in which lane, MML@GA, owner), see `01-MODULES-INDEX.md §17.1-17.4` and `24-ROADMAP.md §19.1-19.4`. **The 6 V2.1 modules are also surfaced in `01-MODULES-INDEX §17.4` and `24-ROADMAP §19.4`; the V1.1 / V1.2 / V2 modules are in `01-MODULES-INDEX §17.1-17.3` and `24-ROADMAP §19.1-19.3`.**

| Slug | Lane | Name | MML@GA | Doc | Deep-dive § |
|---|---|---|---|---|---|
| `whatsapp-business` | V1.1 | WhatsApp Business Cloud API (templates, inbound, 24h window, opt-in) | 4 | `11-Notifications` | §20.1 |
| `recurring-appointments` | V1.1 | RFC 5545 RRULE, expander, EXDATE/RDATE, timezone correctness | 4 | `07-Appointments` | §24.5 |
| `group-appointments` | V1.1 | Group therapy / class / camp — capacity enforcement, per-participant outcomes, group video | 4 | `07-Appointments` | §19.2 |
| `family-booking` | V1.1 | Book for dependents (spouse/child/parent/sibling/caregiver) with OTP-verified consent scopes | 4 | `06-Patients` | §14.5 |
| `multi-currency-payout` | V1.1 | Doctor payout in 25 currencies via Razorpay Route / Stripe Connect / UPI / PayPal / Wise / Payoneer with TDS/1099/VAT | 3 | `05-Doctors` | §16.5 |
| `geo-search` | V1.1 | PostGIS geo-search, Mapbox tile precompute, 8 filters, "use my location" UX | 4 | `05-Doctors` | §16.2 |
| `marketing-vouchers` | V1.1 | Public promo codes, referral bonuses, partner pools, influencer commissions | 3 | `09-Billing` | §11.4a |
| `dynamic-refund-window` | V1.1 | Per-clinic / per-service / per-doctor / per-segment refund policy + 4 resolution types (refund / partial / credit / free rebook) | 3 | `09-Billing` | §11.4 |
| `smart-waitlist` | V1.1 | Auto-fill cancellations from waitlist with weighted match score, 5-min offer cascade, group + recurring modes | 3 | `07-Appointments` | §19.5 |
| `i18n-rtl` | V1.1 | 10 GA languages (en/es/fr/pt/de/ar/he/hi/ur/bn) + 6 beta, full RTL pipeline, per-locale formatting, translation management | 4 | `12-Engagement` | §6.7 |
| `doctor-rotating-schedule` | V1.1 | On-call rotation pools, swap requests, 5-min escalation chain, fairness strategies, coverage gap detection | 3 | `05-Doctors` | §16.4 |
| `pwa-offline-mode` | V1.1 | Patient PWA: 4 cache strategies, offline check-in via IndexedDB + Background Sync, conflict resolution, Web Push, install prompt, 110 MB storage budget | 4 | `18-User-Flows` | §19.17 |
| `abdm-bridge` | V1.2 | ABDM/ABHA health ID + HIU/HIP gateway (India) — ABHA 14-digit, NHA OAuth 2.0 + mTLS, ConsentArtifact, ABDM cert M1/M2/M3 | 3 | `02-Auth` | §17.4 |
| `fhir-r4-read` | V1.2 | FHIR R4 read endpoint (10 resources: Patient, Practitioner, Encounter, Observation, +6) via HAPI-FHIR sidecar, OAuth 2.0 client_credentials, capability statement at `/api/v2/fhir/metadata` | 3 | `16-API-Reference` | §FHIR R4 V2 |
| `razorpay-subscriptions` | V1.2 | Razorpay Subscription API (recurring billing, 4 mandate types: card/UPI Autopay/eNACH/PayLater, 5 webhooks, 5 dunning retries, proration credit) | 3 | `10-Payments` | §18.1.1 |
| `paystack-subscriptions` | V1.2 | Paystack Subscription API (NG/GH/ZA/KE, 3 mandate types: card/bank/mobile-money, 7 events, dunning) | 3 | `10-Payments` | §18.1.2 |
| `stripe-terminal` | V1.2 | Stripe Terminal in-person card reader (Wise Pad 3 / Verifone P400 / BBPOS WisePOS E, server-driven + mobile SDK, offline mode) | 4 | `10-Payments` | §18.3 |
| `twilio-sms` | V1.2 | Twilio Programmable SMS (OTP fallback + transactional + two-way) + Verify API, A2P 10DLC US + DLT India, 6 opt-out + 2 opt-in keywords | 4 | `11-Notifications` | §20.2 |
| `local-payment-methods` | V1.2 | 12 methods: UPI Intent/Collect/Autopay/Paytm/NetBanking/EMI/Pix/Boleto/OXXO/GCash/M-Pesa/Paystack bank+QR | 3 | `10-Payments` | §18.4 |
| `multi-clinic-dashboard` | V1.2 | Chain (HQ + branches) with aggregated dashboard, 3 admin roles (chain_admin / region_admin / chain_auditor), revenue split rules, per-clinic PII export co-sign | 3 | `03-Tenancy` | §17.7 |
| `pathology-lab` | V2 | Lab orders, sample tracking, partner flow, LOINC results, e-Reports | 4 | `08-Prescriptions` | §16.3 |
| `telemedicine-video` | V2 | In-app WebRTC video consult (LiveKit primary, Daily/Jitsi fallback), 2-party consent, 90-day auto-delete | 4 | `07-Appointments` | §19.3 |
| `ai-symptom-checker` | V2 | Pre-visit LLM symptom triage with red-flag detection (routing only, not diagnostic) | 3 | `18-User-Flows` | §19.7 |
| `ai-clinical-scribe` | V2 | Doctor–patient audio → STT → SOAP note + Rx auto-draft, 2-party consent, 60s withdraw-consent purge | 3 | `08-Prescriptions` | §16.4 |
| `in-app-chat` | V2 | Patient↔doctor realtime chat with attachments, voice, chart promotion, encryption-at-rest | 4 | `11-Notifications` | §20.4 |
| `plugins-v1` | V2 | Plugin Core runtime (install, lifecycle, hooks, sandbox, capability model) — see §3.3a + §3.3b | 3 | `14-V2-Stubs` | §3.3a / §3.3b |
| `custom-domain` | V2 | Tenant CNAME + ACM cert + CloudFront, DNS-01 with `_medicare-verify.` prefix, CAA-aware, HSTS opt-in | 3 | `03-Tenancy` | §17.4 |
| `multi-region-dr` | V2 | Active-passive DR in second region, RPO 5 min, RTO 1 h, quarterly drill | 3 | `21-Deployment` | §18.5 |
| `drug-interaction-db` | V2 | RxNav/DrugBank/WHO/tenant-curated multi-source real-time interaction check, p99 < 200ms, hard stop on `contraindicated` | 4 | `08-Prescriptions` | §16.1 |
| `e-prescription` | V2 | National e-prescription formats (ABDM/SCRIPT/EPS/NHS/ADHA) with country-appropriate signing (DSC / smart card / EPCS token), JWS ES256 | 3 | `08-Prescriptions` | §16.2 |
| `insurance-preauth` | V2 | Payer integration: eligibility 270, preauth 278/NHCX, claim 837, ERA 835, denial auto-categorized by CARC/RARC | 3 | `09-Billing` | §21.3 |
| `revenue-cycle-mgmt` | V2.1 | Denial mgmt + appeals, dunning state machine, payment plans, underpayment detection, A/R aging | 3 | `09-Billing` | §21.4 |
| `saas-plan-marketplace` | V2.1 | Plugin monetization: 70/30 revenue share (negotiable up to 80/20), developer portal, plan publishing, proration, dunning, payouts via Stripe Connect / Razorpay Route | 4 | `14-V2-Stubs` | §3.3c |
| `on-prem-ai-gateway` | V2.1 | Self-hosted vLLM for HIPAA/EU-BAIC/DPDP, no prompt/response logged (only token counts + prompt SHA-256 hash), HSM for air-gapped | 3 | `19-Integrations` | §15.2a |
| `soc2-type-ii` | V2.1 | SOC 2 Type II attestation (75 controls: 33 CC + 7 A + 12 C + 5 PI + 18 P), observation period V2.0 → V2.1, Big-4 firm | 3 | `20-Security` | §23.2.0 |
| `byok` | V2.1 | Bring Your Own Key (AWS KMS / GCP KMS / HashiCorp Vault / Azure Key Vault), 5-min DEK cache TTL, 30-day old key version grace, per-process per-tenant cache, emergency revoke test | 3 | `20-Security` | §23.2.2 |
| `field-level-encryption` | V2.1 | `EncryptedFieldCast` per column + HMAC-SHA256 searchable-token sidecar, 5 encrypted fields (aadhaar/ssn/pan/medical_license/prescription_notes), `data_classification` registry + daily drift sweep | 3 | `20-Security` | §23.2.1 |

> Note: AI Scribe and AI Symptom Checker (V2 lane) ship with **cloud LLM** (OpenAI / Anthropic) as the default. `on-prem-ai-gateway` is the V2.1 Enterprise add-on for tenants that require on-prem (HIPAA, EU-BAIC, India DPDP). See `01-MODULES-INDEX.md` §17.4 and `24-ROADMAP.md` §19.4 for the full V2.1 catalog.

### 3.5 V4 Lane Features (Hospital ITOps)

| Slug | Name | Tier | V1 Scope | MML@GA | Owner |
|---|---|---|---|---|---|
| `lis-astm` | ASTM E1394/E1394-97 LIS instrument interface — see `19-Integrations §15.10c` deep-dive | Enterprise | `19-Integrations` | 3 | Lab guild |
| `his-bridge` | Epic/Cerner/Meditech bidirectional bridge — FHIR R4 + HL7 v2 + IHE XDS.b + PIX/PDQ + C-CDA — see `19-Integrations §15.10d` deep-dive | Enterprise | `19-Integrations` | 3 | Integrations guild |
| `er-triage` | ED triage board (ESI 1–5), bay assignment, doctor pickup — see `07-Appointments §20.11` deep-dive | Enterprise | `06-Patients`, `07-Appointments` | 3 | Hospital guild |
| `nursing-station` | Nurse handoff (SBAR), vitals charting, MAR with Five Rights — see `07-Appointments §20.13` deep-dive | Enterprise | `06-Patients` | 3 | Hospital guild |
| `centralized-pharmacy` | Hospital pharmacy: ward stock, Schedule II–V vault with witness, IV admixture — see `08-Prescriptions §18.1` deep-dive | Enterprise | `08-Prescriptions` | 3 | Pharmacy guild |
| `equipment-maintenance` | Biomedical asset tracking, PM scheduling, work orders, FDA recall — see `07-Appointments §20.15` deep-dive | Enterprise | `04-Clinic-Structure` | 3 | Hospital guild |

> Anything past V4 is on the **public roadmap** but not designed yet.

---

## 4. V1 → V2 Module Touchpoints

This is the **core deliverable** of this document: for every V1 module doc, here is the list of V2 features that **modify, extend, or consume** that module, with cross-references to the V1 docs that need to be updated.

### 4.1 Touch `00-ARCHITECTURE.md`
- **V2:** Introduce `medicare-ai-service` (Python, FastAPI + vLLM), `medicare-lab-gateway` (Java/Spring for ASTM instrument driver), `medicare-realtime-ice` (coturn TURN/STUN), and a `medicare-fhir-gateway` (Node, HAPI-FHIR). Service mesh via Linkerd; mTLS between services; service-to-service auth via SPIFFE IDs.
- **Cross-ref:** See [V2 in 00-Architecture](#).

### 4.2 Touch `01-MODULES-INDEX.md`
- **V2:** Module index grows from 57 (V1) to 92+ (V2 GA), 130+ (V3), 180+ (V4). Module ownership moves from "guild" to "guild + V2 sponsor".

### 4.3 Touch `02-MODULE-AUTH.md`
- **V2 features:** `abdm-bridge` (ABHA token exchange), `twilio-sms` (SMS OTP fallback), `sso-oidc` (OIDC SSO for enterprise), `mfa-enforcement` (TOTP), `passkeys` (WebAuthn). Add `user_identities` table (one user, many federated identities), `mfa_credentials` table (TOTP + WebAuthn).
- **What changes:** `POST /auth/login` accepts a `method` enum (`password|phone_otp|abha|oidc`); Realm B grows to include Realm C (OIDC).

### 4.4 Touch `03-MODULE-TENANCY.md`
- **V2 features:** `custom-domain` (per-tenant `*.medicare.app` subdomain, plus custom CNAME), `region-routing` (latency-based tenant→region map), `per-tenant-kms` (each tenant gets a dedicated KMS key in V2.1).
- **What changes:** `tenants` table gains `custom_domain`, `region`, `kms_key_id`, `plan_id`, `subscription_status`. `TenantResolver` reads `Host` header first, then `X-Tenant`, then path. `Route53` + Vercel handle DNS; cert-manager handles Let's Encrypt for custom domains.

### 4.5 Touch `04-MODULE-CLINIC-STRUCTURE.md`
- **V2 features:** `ot-room-scheduling` (operation theatres as bookable resources), `bed-management` (wards + beds as resources), `equipment-booking` (X-ray, ultrasound slots), `multi-clinic-dashboard` (chain admin sees all branches).
- **What changes:** New `resources` table (generic bookable resource: room, bed, equipment, OT slot) + `resource_schedules` + `resource_bookings`. Clinic gains `chain_id` and `chain_role` (`head|branch`).

### 4.6 Touch `05-MODULE-DOCTORS.md`
- **V2 features:** `geo-search` (PostGIS + Mapbox tile precompute), `ai-doctor-recommendation` (vector similarity on specialties + reviews + patient history), `doctor-rotating-schedule` (on-call rotation across a group), `doctor-payout-currency` (doctor sets preferred payout currency), `doctor-team` (group practice with shared slots).
- **What changes:** `doctors` table gains `geo` (PostGIS POINT), `rotation_group_id`, `payout_currency`, `payout_method` (bank, UPI, PayPal). New `doctor_reviews` (already V1) is indexed in pgvector for similarity.

### 4.7 Touch `06-MODULE-PATIENTS.md`
- **V2 features:** `abdm-bridge` (link ABHA → patient), `family-booking` (dependents), `ehr-problem-list` (chronic conditions, ICD-10), `ehr-family-history`, `ehr-lab-trends` (chart over time), `in-app-chat` (chat history persists to patient chart), `clinical-ai-scribe` (SOAP note auto-attached).
- **What changes:** `patients` table gains `abha_id`, `blood_group`, `allergies` (JSON), `chronic_conditions` (JSON). New tables: `patient_problem_list`, `patient_family_history`, `patient_lab_results`, `patient_attachments` (S3 keys), `patient_vitals_trends` (materialized view refreshed hourly).

### 4.8 Touch `07-MODULE-APPOINTMENTS.md`
- **V2 features:** `recurring-appointments` (RRULE, expanded on read), `group-appointments` (N patients in one slot, e.g., group therapy), `telemedicine-video` (LiveKit room auto-created), `ot-room-scheduling` (resource booking on confirmed), `waitlist` (already V1 stub, fully built in V2), `dynamic-refund-window` (per-clinic policy), `smart-waitlist` (auto-fill).
- **What changes:** `appointments` table gains `parent_appointment_id` (recurring), `is_group`, `group_size`, `video_room_id` (LiveKit SID), `resources` (JSON: room/bed/equipment), `rrule` (RFC 5545). New tables: `appointment_series` (the recurring definition), `appointment_waitlist`, `appointment_reminders_log`.

### 4.9 Touch `08-MODULE-PRESCRIPTIONS.md`
- **V2 features:** `drug-interaction-db` (real-time check on Rx write), `e-prescription` (ABDM/SCRIPT/GStandard signing), `ai-clinical-scribe` (auto-generate Rx from consult audio), `lab-order` (lab tests in same Rx, sent to `pathology-lab`).
- **What changes:** `prescriptions` table gains `prescription_format` (`v1|abdm|script|gstandard`), `signature_jws` (signed JSON), `interactions_checked` (JSON). New tables: `lab_orders`, `lab_order_items`, `lab_results`, `drug_interactions_log`.

### 4.10 Touch `09-MODULE-BILLING.md`
- **V2 features:** `subscription-billing` (recurring SaaS plans via Razorpay Subscriptions / Stripe Subscriptions), `insurance-preauth` (pre-auth workflow with payer), `revenue-cycle-mgmt` (denial, E&O, dunning), `multi-clinic-revenue-share` (chain admin defines revenue split), `package-pricing` (e.g., "10-visit cardiology pack ₹9,999").
- **What changes:** `invoices` table gains `subscription_id`, `insurance_claim_id`, `package_id`. New tables: `subscriptions`, `subscription_items`, `subscription_invoices`, `insurance_payers`, `insurance_claims`, `insurance_preauth_requests`, `denials`, `dunning_letters`, `credit_notes`, `payment_plans`.

### 4.11 Touch `10-MODULE-PAYMENTS.md`
- **V2 features:** `razorpay-subscriptions` (Recurring payments), `paystack-subscriptions`, `stripe-terminal` (in-person card via Stripe Terminal SDK), `bnpl` (ZestMoney, LazyPay, Klarna), `local-payment-methods` (UPI Autopay, GCash), `crypto` (Bitcoin via OpenNode, opt-in).
- **What changes:** `PaymentGateway` interface gains `createSubscription()`, `cancelSubscription()`, `chargeAtTerminal()`, `verifyTerminal()`. New: `subscriptions` table, `subscription_events` (audit), `terminal_readers` table.

### 4.12 Touch `11-MODULE-NOTIFICATIONS.md`
- **V2 features:** `whatsapp-business` (template messages via WhatsApp Cloud API), `twilio-sms` (SMS for OTP and reminders), `in-app-chat` (real-time chat as a notification type), `push-categories` (patient can subscribe to health tips, clinic news), `in-app-banner` (admin pushes a banner to all patients).
- **What changes:** `notification_templates` table gains `whatsapp_template_id` (pre-approved by Meta), `sms_template_id` (registered with Twilio). New tables: `whatsapp_conversations`, `in_app_chats`, `in_app_messages`. New job: `SendWhatsAppTemplateJob` with 24h customer service window enforcement.

### 4.13 Touch `12-MODULE-ENGAGEMENT.md`
- **V2 features:** `blogs-cms` (publish health articles), `content-workflow` (draft → review → publish), `seo-meta`, `content-personalization` (show relevant articles by diagnosis).
- **What changes:** New tables: `blog_categories`, `blog_posts`, `blog_revisions`, `blog_comments`. New admin UI: rich text editor (TipTap), media library, scheduled publishing.

### 4.14 Touch `13-MODULE-SETTINGS-REPORTS.md`
- **V2 features:** `feature-flag-admin` (admin enables/disables per-tenant), `custom-report-builder` (Metabase-style designer), `ai-usage-dashboard` (LLM token consumption), `audit-log-explorer` (searchable UI over `audit_logs`).
- **What changes:** `settings` table gains `feature_flags` (JSON), `report_definitions` table (user-defined SQL/visual report saved as JSON), `ai_usage_log` table (every LLM call, tokens, cost).

### 4.15 Touch `15-DATABASE-SCHEMA.md`
- **V2 tables (~40 new):**
  - Pathology: `lab_panels`, `lab_partners`, `lab_orders`, `lab_order_items`, `lab_samples`, `lab_results`, `lab_reports`, `lab_partner_api_calls`.
  - Telemedicine: `video_rooms` (LiveKit), `video_recordings` (S3 keys), `video_participants`.
  - AI: `ai_conversations`, `ai_messages`, `ai_usage_log`, `ai_prompts` (versioned), `ai_tool_calls` (function calling).
  - Subscriptions: `subscription_plans`, `subscriptions`, `subscription_items`, `subscription_invoices`, `subscription_events`, `subscription_dunning`.
  - Insurance: `insurance_payers`, `insurance_policies`, `insurance_claims`, `insurance_preauth_requests`, `denials`, `dunning_letters`, `credit_notes`, `payment_plans`.
  - Resources: `resources` (rooms/beds/equipment), `resource_schedules`, `resource_bookings`.
  - Chat: `in_app_chats`, `in_app_messages`, `chat_attachments`.
  - Plugins: `plugins`, `plugin_installs`, `plugin_versions`, `plugin_licenses`.
  - Custom domain: `tenant_domains`, `domain_verifications`.
  - FHIR: `fhir_resources` (cached projection).
  - EHR: `patient_problem_list`, `patient_family_history`, `patient_lab_results`, `patient_attachments`, `patient_vitals_trends`.
  - Beds/OT: `wards`, `beds`, `bed_assignments`, `operation_theatres`, `ot_slots`, `equipment`, `equipment_bookings`.
  - OIDC: `identity_providers`, `user_identities`, `mfa_credentials`.
  - Audit additions: `data_access_log` (every PHI read recorded with reason).
- **Indexes:** Add PostGIS GiST index on `doctors.geo`, pgvector index on `doctor_reviews.embedding`, GIN on `appointments.rrule`, partial indexes on `appointments.status`.

### 4.16 Touch `16-API-REFERENCE.md`
- **V2 adds ~250 endpoints**:
  - `POST /v2/lab/orders`, `GET /v2/lab/orders/{id}`, `POST /v2/lab/orders/{id}/samples`, `POST /v2/lab/results`, `GET /v2/lab/reports/{id}/pdf`.
  - `POST /v2/video/rooms`, `GET /v2/video/rooms/{id}/token`, `POST /v2/video/rooms/{id}/end`, `GET /v2/video/rooms/{id}/recording`.
  - `POST /v2/ai/symptom-checker`, `POST /v2/ai/scribe/audio` (multipart upload), `GET /v2/ai/conversations/{id}`.
  - `POST /v2/subscriptions/plans` (admin), `POST /v2/subscriptions`, `POST /v2/subscriptions/{id}/cancel`, `POST /v2/subscriptions/{id}/pause`.
  - `POST /v2/insurance/preauth`, `GET /v2/insurance/claims`, `POST /v2/insurance/claims/{id}/submit`.
  - `POST /v2/chat/chats`, `POST /v2/chat/chats/{id}/messages`, `GET /v2/chat/chats/{id}/messages` (paginated).
  - `GET /v2/fhir/Patient/{id}`, `GET /v2/fhir/Encounter/{id}`, `GET /v2/fhir/Observation?patient={id}`.
  - `POST /v2/plugins/install`, `POST /v2/plugins/{id}/enable`, `POST /v2/plugins/{id}/disable`.
  - `POST /v2/tenants/{id}/custom-domain`, `GET /v2/tenants/{id}/custom-domain/verify`.
  - `POST /v2/beds/assignments`, `GET /v2/beds?ward={id}`.
  - `POST /v2/ot/slots`, `POST /v2/ot/bookings`.
- **GraphQL gateway:** V2 adds an opt-in GraphQL gateway (Apollo) that wraps the JSON:API. It is read-only at first; mutations land in V3.

### 4.17 Touch `17-REALTIME.md`
- **V2 features:** `in-app-chat` channels (`chat.{chat_id}`), `video-signal` channels (WebRTC signaling over Reverb until ICE connects), `presence` channels (`presence.clinic.{id}` for "who is online"), `ai-stream` channels (LLM token streaming for scribe).
- **What changes:** Reverb scales via Redis pub/sub. We add a **coturn** TURN/STUN server for WebRTC NAT traversal (do not trust public STUN). Video recording stored in S3 with server-side encryption (SSE-KMS).

### 4.18 Touch `18-USER-FLOWS.md`
- **V2 adds 14 new journeys:**
  1. Patient books recurring physiotherapy (RRULE).
  2. Patient books for spouse (family booking).
  3. Doctor orders lab test in Rx.
  4. Lab partner receives order, draws sample, uploads result.
  5. Patient sees lab result in app, doctor reviews, chat begins.
  6. Doctor starts video consult, screen shares prescription, ends call, recording saved.
  7. Patient asks symptom checker, gets triage recommendation, books.
  8. Doctor enables AI scribe, records audio, gets SOAP draft, edits, signs.
  9. Patient joins waitlist, gets auto-notified when slot opens, books in 5 min.
  10. Tenant admin subscribes to "Pro plan", pays via Razorpay Subscription, plan auto-renews.
  11. Insurance pre-auth: doctor creates request, payer approves, claim submitted, payment posted.
  12. Patient sends in-app chat to doctor, doctor replies, message stored in chart.
  13. Patient links ABHA, profile syncs from ABDM.
  14. Tenant admin enables `drug-interaction-db` flag, doctor gets real-time alert on conflict.

### 4.19 Touch `19-INTEGRATIONS.md`
- **V2 adds 18 new integrations:**
  - **Lab partners:** 1mg Labs, Thyrocare, SRL Diagnostics (India); Quest Diagnostics, LabCorp (US).
  - **LLM providers:** OpenAI (GPT-4o, o1), Anthropic (Claude 3.5), Google (Gemini 1.5), Mistral, vLLM (self-hosted).
  - **Telemedicine:** LiveKit Cloud (primary), Daily.co (fallback), Jitsi Meet (self-hosted fallback).
  - **Messaging:** WhatsApp Business Cloud API, Twilio (SMS + Voice), MessageBird.
  - **Maps:** Mapbox, Google Maps, OpenStreetMap (Nominatim for self-hosted).
  - **ABDM:** NHA Sandbox, ABDM HIP/HIU gateways.
  - **Insurance:** Vipul (India), Change Healthcare (US), Availity, Edifecs.
  - **DICOM:** OHIF viewer, DICOMweb server (Orthanc).
  - **TURN/STUN:** coturn (self-hosted), Twilio Network Traversal.
  - **FHIR:** HAPI-FHIR (self-hosted validator), Firely Terminal.
  - **Push:** FCM (existing), APNs (added for iOS), HMS (Huawei Mobile Services for China).
  - **Email:** SES, Mailgun, Postmark, Resend.
  - **Storage:** S3, GCS, Azure Blob, MinIO (self-hosted).
  - **Observability:** Sentry, Datadog, Honeycomb, OpenTelemetry Collector.
  - **Status:** BetterUptime, PagerDuty, Opsgenie.
  - **Search:** Meilisearch (primary), Typesense (fallback), pg_trgm (DB fallback).
  - **Cache:** Redis Cloud, Upstash, AWS ElastiCache, self-hosted Redis.
  - **Secrets:** AWS Secrets Manager, HashiCorp Vault, Doppler.

### 4.20 Touch `20-SECURITY-COMPLIANCE.md`
- **V2 adds compliance regimes:**
  - **SOC 2 Type II** (year 1 of V2).
  - **HIPAA** (US) — covered by existing controls; V2 adds BAA template per integration.
  - **GDPR** (EU) — already in V1; V2 adds DPIA for AI processing.
  - **DPDP Act** (India) — already in V1.
  - **ABDM** (India) — V2 adds certification requirements.
  - **HL7/FHIR** — V3 adds audit logging per IHE ATNA.
  - **PIPEDA** (Canada) — covered by GDPR controls.
  - **ISO 27001** — V3 target.
  - **HITRUST CSF** — V3 target.
- **V2 security additions:**
  - **End-to-end encryption for DICOM** (PACS-bound, key wrapped per tenant KMS).
  - **BYOK (Bring Your Own Key)** for enterprise tenants.
  - **Homomorphic encryption** (V4) for cohort analytics on encrypted data.
  - **Differential privacy** for AI training data (V3).
  - **Tenant-scoped service identities** (SPIFFE).

### 4.21 Touch `21-DEPLOYMENT.md`
- **V2 adds services:**
  - `medicare-ai-service` (Python, FastAPI, vLLM) — separate cluster, GPU nodes in us-east-1 and eu-west-1.
  - `medicare-lab-gateway` (Java/Spring) — internal only, no public ingress.
  - `medicare-realtime-ice` (coturn) — UDP/TCP on :3478, deployed in same region as Reverb.
  - `medicare-fhir-gateway` (Node, HAPI-FHIR) — public, with strict rate limits.
  - **Multi-region DR:** active-passive in second region (us-west-2 / ap-south-1), RPO 5 min, RTO 1 hour, monthly DR drill.
  - **K8s migration:** V2 moves from Hetzner+Forge to EKS (or self-hosted K8s on Hetzner for V1 budget tenants). Migration plan in this doc.

### 4.22 Touch `22-INSTALLER.md`
- **V2 adds:**
  - **Plugin marketplace** — installer shows a curated list of plugins, tenant picks at install time.
  - **Add-on activation** — tenant can enable/disable plugins post-install via admin UI.
  - **First-run wizard** — beyond the 8-step V1 wizard: pick plan, pick plugins, configure integrations.
  - **License verification** — SaaS plans validate against our license server (Laravel + Redis, 99.95% SLA).

### 4.23 Touch `23-DEVELOPMENT-SETUP.md`
- **V2 adds:**
  - **Python venv** for `medicare-ai-service` (pyenv + poetry).
  - **Local LLM** via Ollama (qwen2.5:7b for symptom checker; whisper-large-v3 for scribe STT).
  - **Lab partner sandbox** — `lab-mock-server` (Node, mocks ASTM E1394 + 1mg webhook).
  - **LiveKit local** — `docker compose up livekit` for video.
  - **coturn local** — `docker compose up coturn` for TURN.
  - **FHIR validator** — `docker compose up hapifhir`.
  - **MinIO local** — S3-compatible storage.
  - **k3d** for K8s dev cluster (V2.1+).

### 4.24 Touch `24-ROADMAP.md`
- **V2 lane dominates 24-ROADMAP** — see release table in §2.

---

## 5. Feature Flag Strategy

### 5.1 Why flags
- Tenants opt in or out per feature.
- We can dark-launch to a 1% cohort, monitor, ramp.
- We can A/B test V2 features against V1 baseline.
- We can disable a feature instantly if a regression hits production.

### 5.2 Storage
- **Per-tenant flags** live in `tenants.feature_flags` (JSON column).
- **Global flags** live in Redis (`feature_flag:{slug}`) and are updated via `php artisan tinker --execute="Feature::set('pathology-lab', true)"`.
- **Cohort flags** (e.g., "10% of Pro tenants") live in `feature_flag_cohorts` table; rules evaluated by `FeatureFlagService::evaluate($slug, $tenantId)`.

### 5.3 Resolution order
1. Explicit `tenants.feature_flags.{slug}` override.
2. Tenant's plan (`plan.feature_flags` JSON, e.g., Pro plan has `pathology-lab=true`).
3. Tenant's region override (e.g., `pathology-lab` disabled in EU for legal review).
4. Global default in `config/features.php`.

### 5.4 API
```php
// Service code
if (Feature::enabled('pathology-lab', $tenantId)) {
    return $this->labOrderService->create($data);
}
return $this->notImplemented('pathology-lab');
```

### 5.5 Admin UI
- Settings → "Features" tab (V1 ships a basic list).
- V2 ships the full feature catalog with descriptions, pricing, "Enable" button.
- Each enable triggers a webhook (`tenant.feature.activated`) so plugins (analytics, billing) can react.

### 5.6 Rollout process
1. **MML 1–2:** Feature flag exists but disabled. Code paths return 501.
2. **MML 3:** Enable for internal tenant (`medicare-internal`). Smoke test in production.
3. **MML 3.5:** Enable for 5% of Pro tenants. Watch error budget.
4. **MML 4:** Enable for 50% of Pro tenants. Watch latency, NPS.
5. **MML 5:** Enable for 100% of eligible tenants. Mark GA.

### 5.7 Telemetry
Every flag evaluation logs to `feature_flag_evaluations` (sampled 1 in 100 to keep volume down). Aggregated nightly to detect underused flags.

---

## 6. Data Model Additions

### 6.1 Migration directory layout
```
database/migrations/
├── v1/
│   ├── 2024_01_01_000001_create_tenants_table.php
│   ├── ...
│   └── 2024_01_01_000080_create_audit_logs_table.php
├── v2/
│   ├── 2024_08_01_000001_create_lab_orders_table.php
│   ├── ...
│   └── 2024_08_01_000040_create_video_rooms_table.php
└── v3/
    └── ...
```

`config/database.php` registers a `v2` connection alias (same DB, different migration directory). `php artisan migrate --path=database/migrations/v2` runs V2 migrations. V1 tenants never run V2 migrations unless they enable a V2 feature that needs them.

### 6.2 V2 schema highlights
See §4.15 for the full list. The **two most critical** V2 tables are:

#### `lab_orders` (the V2 cornerstone)
```php
Schema::connection('v2')->create('lab_orders', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('prescription_id')->nullable()->index();
    $t->ulid('patient_id')->index();
    $t->ulid('doctor_id')->index();
    $t->ulid('lab_partner_id')->index();
    $t->string('external_order_id', 64)->nullable()->index(); // partner-side ID
    $t->string('status', 32)->index(); // draft, submitted, sample_collected, in_lab, completed, cancelled, rejected
    $t->string('priority', 16)->default('normal'); // stat, urgent, normal
    $t->json('clinical_notes')->nullable();
    $t->json('patient_address'); // for home collection
    $t->timestamp('scheduled_at')->nullable()->index();
    $t->timestamp('sample_collected_at')->nullable();
    $t->timestamp('completed_at')->nullable();
    $t->decimal('total_amount', 12, 2);
    $t->char('currency', 3);
    $t->json('metadata')->nullable();
    $t->timestamps();
    $t->softDeletes();
    $t->foreign('tenant_id')->references('id')->on('tenants')->restrictOnDelete();
    $t->foreign('prescription_id')->references('id')->on('prescriptions')->nullOnDelete();
    $t->foreign('patient_id')->references('id')->on('patients')->restrictOnDelete();
    $t->foreign('doctor_id')->references('id')->on('doctors')->restrictOnDelete();
    $t->foreign('lab_partner_id')->references('id')->on('lab_partners')->restrictOnDelete();
});
```

#### `video_rooms`
```php
Schema::connection('v2')->create('video_rooms', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('appointment_id')->unique(); // 1:1 with appointment
    $t->string('provider', 32); // livekit, daily, jitsi
    $t->string('room_sid', 128)->unique(); // provider-side ID
    $t->json('participants')->nullable(); // {patient: token, doctor: token}
    $t->boolean('recording_enabled')->default(false);
    $t->string('recording_s3_key')->nullable();
    $t->timestamp('started_at')->nullable();
    $t->timestamp('ended_at')->nullable();
    $t->integer('duration_seconds')->nullable();
    $t->timestamps();
});
```

### 6.3 PostGIS extension
V2 enables PostGIS for geo-search. Migration:
```sql
CREATE EXTENSION IF NOT EXISTS postgis;
ALTER TABLE doctors ADD COLUMN geo geography(POINT, 4326);
CREATE INDEX doctors_geo_gist ON doctors USING GIST (geo);
```

### 6.4 pgvector extension
V2 enables pgvector for AI doctor recommendation and semantic search.
```sql
CREATE EXTENSION IF NOT EXISTS vector;
ALTER TABLE doctor_reviews ADD COLUMN embedding vector(1536);
CREATE INDEX doctor_reviews_embedding_ivf ON doctor_reviews USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);
```

### 6.5 Cross-tenant keys
Every V2 table has `tenant_id` as the **leading column** of the primary index (composite or covering). `BelongsToTenant` trait is mandatory.

---

## 7. Billing & Subscription

### 7.1 Pricing model
V1 is sold as a **one-time license** (per clinic, perpetual). V2 introduces a **SaaS subscription** model.

| Plan | Monthly | Includes | Target |
|---|---|---|---|
| **Starter** | Free | 1 doctor, 100 appointments/mo, basic features | Solo doctor just starting |
| **Pro** | ₹2,499 / $29 | 5 doctors, unlimited appts, V1.1 features, 1 V2 feature free | Growing clinic |
| **Clinic** | ₹9,999 / $119 | 25 doctors, chain, all V1.1 + most V2, priority support | Multi-doctor clinic |
| **Enterprise** | Custom | Unlimited doctors, SSO, BYOK, custom SLA, all features | Hospital chain, payer |
| **Marketplace plugins** | Per-plugin | Add-on features | Any plan |

### 7.2 Subscription engine
- `tenants.subscription_id` → `subscriptions.id`.
- `subscriptions.plan_id` → `subscription_plans.id`.
- `subscription_items` for add-ons (e.g., "AI Scribe: +$99/mo").
- `subscription_invoices` generated by Laravel scheduler on renewal date.
- `subscription_events` for audit (created, renewed, paused, cancelled, dunning).
- **Dunning:** 3 attempts (day 0, 3, 7), then service degrades (no new appointments can be created, but existing ones continue).

### 7.3 Payment gateway integration
- V1: One-off payments.
- V2: Subscriptions via Razorpay Subscriptions (India primary), Stripe Subscriptions (US/EU), Paystack Subscriptions (Africa).
- Webhook handlers: `subscription.activated`, `subscription.renewed`, `subscription.payment_failed`, `subscription.cancelled`.
- **Idempotency:** Same as V1 (UNIQUE on `gateway_event_id`).

### 7.4 Revenue share (chain)
Multi-clinic chain has a `revenue_split` rule (e.g., "70% to branch, 30% to HQ"). Implemented as a `chain_revenue_rules` table, evaluated nightly, payouts via Razorpay Route / Stripe Connect.

### 7.5 Usage-based pricing
AI features (`ai-symptom-checker`, `ai-clinical-scribe`) charge per token. `ai_usage_log` records every LLM call with token count and cost. Monthly invoice line: "AI tokens: 1.2M @ $0.0001/1k = $120".

### 7.6 Tax handling
- India: GST (CGST+SGST for intra-state, IGST for inter-state). GSTIN per tenant.
- US: Sales tax (Stripe Tax integration).
- EU: VAT (with reverse charge for B2B; tenant provides VAT ID, validated against VIES).
- All rates per `taxes` table (already in V1), but `invoices` get a `tax_breakdown` JSON.

### 7.7 Invoicing
- V1: PDF only.
- V2: PDF + e-invoice (India GSTN IRN, EU Peppol). E-invoice XML is signed and stored in `e_invoices` table.

---

## 8. Security, Privacy, Compliance

### 8.1 Threat model additions
V2 adds:
- **AI prompt injection.** A patient crafts a prompt that tricks the symptom checker into recommending a non-existent drug. Mitigations: input filter, output filter, human-in-the-loop, red-team testing every release.
- **Lab partner compromise.** Lab partner webhook is signed but a compromised partner could push false results. Mitigations: out-of-band verification, abnormal result flag (e.g., HbA1c 30 → auto-flag for human review).
- **Video recording leak.** S3 bucket misconfiguration. Mitigations: SSE-KMS, public access blocked, lifecycle rule expires recordings after 90 days, signed URLs only.
- **Plugin supply chain.** A malicious plugin could read all PHI. Mitigations: Plugin Core capability model (§3.3a), code review before listing on the Public Marketplace, signed plugin artifacts, kill switch (`plugin_installs.status = 'disabled'`), audit log on every hook invocation.
- **OIDC token replay.** Mitigations: short-lived tokens (5 min), refresh token rotation, PKCE for public clients.
- **Subscription fraud.** Stolen credit card signs up for Pro. Mitigations: 3DS/SCA, $1 auth+void, address verification, velocity checks.
- **WebRTC media injection.** Mitigations: LiveKit DTLS-SRTP (default), no raw RTP, room tokens scoped to appointment.

### 8.2 Compliance regimes
See §4.20 for the full list. V2 target certifications:
- **SOC 2 Type I** at V2 GA (Month 7).
- **SOC 2 Type II** at V2.1 GA (Month 10).
- **HIPAA BAA template** ready at V2 GA.
- **GDPR DPIA** for AI features (Month 6).
- **ABDM certification** for `abdm-bridge` (Month 5).

### 8.3 Data residency
V1 is single-region. V2 supports **per-tenant data residency**:
- `tenants.region` ∈ {`us-east-1`, `eu-west-1`, `ap-south-1`, `ap-southeast-1`}.
- All PHI storage and processing stays in that region.
- Cross-region replication is for **disaster recovery only** (encrypted, encrypted with a key the DR region cannot decrypt — true "you hold the key" pattern).
- Region is selected at install time and **cannot be changed** (would require full data export + re-import; V3 adds this).

### 8.4 Encryption
- **V1:** AES-256-GCM at rest, TLS 1.3 in transit.
- **V2:** Adds **per-tenant KMS keys** (`tenants.kms_key_id` → AWS KMS, GCP KMS, Azure Key Vault). BYOK supported.
- **V2:** Adds **envelope encryption** for large files (S3 SSE-KMS with tenant key).
- **V2:** Adds **field-level encryption** for highly sensitive fields (e.g., Aadhaar, SSN) — encrypted in app, decrypted only in app.

### 8.5 Audit log additions
V1 `audit_logs` records writes. V2 adds `data_access_log`:
- Every PHI **read** is logged with: who, what, when, why (`reason` field, e.g., "Doctor reviewed for consult #1234").
- 7-year retention.
- Indexed by `patient_id` so patients can request "who accessed my data?" (GDPR Article 15).

### 8.6 Breach notification
- 72-hour notification per GDPR.
- V2 adds automated detection (anomalous access patterns via ML on `data_access_log`).
- Incident response runbook in `20-SECURITY-COMPLIANCE.md`.

---

## 9. Observability & SLOs

### 9.1 SLOs
| Service | Availability SLO | Latency SLO (p99) | Error Budget (90d) |
|---|---|---|---|
| `medicare-api` (V1) | 99.95% | 200 ms read / 500 ms write | 21.6 min |
| `medicare-api` (V2) | 99.95% | 250 ms read / 600 ms write | 21.6 min |
| `medicare-realtime` | 99.9% | 100 ms event delivery | 43.2 min |
| `medicare-ai-service` | 99.5% (best-effort) | 2 s first token | 216 min |
| `medicare-lab-gateway` | 99.5% | 1 s partner round-trip | 216 min |
| `medicare-fhir-gateway` | 99.9% | 300 ms | 43.2 min |

### 9.2 Telemetry
- **Metrics:** Prometheus + Grafana. Per-service, per-tenant dashboards.
- **Logs:** Structured JSON to Loki (or CloudWatch). Sampled 1 in 100 for `info`, 100% for `warn` and above.
- **Traces:** OpenTelemetry, sampled 10% for `/api/*`, 100% for errors.
- **Errors:** Sentry, with tenant context attached to every event.
- **Synthetics:** Blackbox exporter checks `/health` every 30s from 5 regions.
- **RUM:** Real User Monitoring via `next/web-vitals` for web, Sentry Flutter for mobile.

### 9.3 Cost observability
- **AI cost** tracked per-tenant via `ai_usage_log`.
- **Bandwidth** tracked per-tenant via CloudWatch metrics.
- **Storage** tracked per-tenant via S3 inventory.
- **Cost dashboard** in admin UI: "You spent $X this month on AI, projected $Y".

### 9.4 Alerting
- **Page on-call** for SLO violation > 2x error budget consumption rate.
- **Slack** for warnings.
- **Email digest** daily for non-urgent anomalies.
- **Quarterly review** of SLOs with product.

### 9.5 Runbooks
- V1: 15 runbooks (deploy, rollback, DB fail, etc.).
- V2: 35 runbooks (add: lab partner down, LiveKit quota exhausted, KMS key compromised, etc.).
- Stored in `runbooks/` directory, rendered to internal wiki.

---

## 10. Open Questions

These are tracked but **not** blocking V2 design. They are the next round of architectural decisions.

1. **Multi-region write.** Active-active DBs (CockroachDB / Spanner) vs active-passive. **Decision (per V2 lane `multi-region-dr` row):** active-passive. Primary region serves all writes; secondary region is a warm standby promoted only on DR drill or primary failure. Active-active is re-evaluated at V5+ once we have ≥ 3 regions in production and the conflict-resolution cost is justified.
2. **Plugin execution model.** Sidecar process (safer, more ops) vs in-process (faster, riskier)? (Decision per §3.3a: V2 ships in-process; V3 adds sidecar mode behind `feature_plugin_sidecar`.)
3. **On-prem LLM or cloud?** Default to cloud; offer on-prem as enterprise add-on. (Decision: V2.1.)
4. **DICOM storage.** Do we store DICOM in our S3, or partner with a PACS vendor? (Decision: V3, partner.)
5. **In-app chat retention.** How long do we keep chat history? 7 years (medical record) or 1 year (chat)? (Decision: 7 years, part of patient chart.)
6. **AI scribe training data.** Can we use de-identified consult audio to improve our scribe model? (Decision: opt-in per tenant, per patient.)
7. **Subscription grandfathering.** V1 perpetual license holders — what do they get for free in V2? (Decision: free access to V1.1 features, 50% off Pro plan for 2 years.)
8. **Insurance pre-auth latency.** Some payers take 5 min, some 5 days. How do we model that in UX? (Decision: V2 ships async flow with webhook callback.)
9. **FHIR write semantics.** V3 write — do we own the source of truth or are we a downstream? (Decision: dual-write with conflict resolution.)
10. **Plugin revenue share.** If a third party builds a plugin, what's the cut? (Decision: 70/30 in favor of developer, modeled on App Store.)

---

## Appendix A · V2 Module Index (numbered)

| # | Module | Doc | Lane | MML@GA |
|---|---|---|---|---|
| 58 | Pathology / Lab | `06`, `08`, `15` | V2 | 4 |
| 59 | Telemedicine Video | `07`, `17` | V2 | 4 |
| 60 | AI Symptom Checker | `18`, `19` | V2 | 3 |
| 61 | AI Clinical Scribe | `06`, `08` | V2 | 3 |
| 62 | In-App Chat | `11`, `17` | V2 | 4 |
| 63 | Plugins v1 (Plugin Core) | All | V2 | 3 |
| 64 | Custom Domain | `03`, `21` | V2 | 3 |
| 65 | Multi-Region DR | `21`, `20` | V2 | 3 |
| 66 | Drug Interaction DB | `08` | V2 | 4 |
| 67 | E-Prescription | `08`, `19` | V2 | 3 |
| 68 | Insurance Pre-Auth | `09`, `18` | V2 | 3 |
| 69 | Revenue Cycle Mgmt | `09`, `13` | V2.1 | 3 |
| 70 | Bed Management | `04`, `06` | V3 | 3 |
| 71 | OT Room Scheduling | `04`, `07` | V3 | 3 |
| 72 | Equipment Booking | `04`, `07` | V3 | 3 |
| 73 | WhatsApp Business | `11` | V1.1 | 4 |
| 74 | Twilio SMS | `02`, `11` | V1.2 | 4 |
| 75 | ABDM Bridge | `02`, `06` | V1.2 | 3 |
| 76 | FHIR R4 Read | `16` | V1.2 | 3 |
| 77 | FHIR R4 Write | `16` | V3 | 3 |
| 78 | Razorpay Subscriptions | `10` | V1.2 | 3 |
| 79 | Paystack Subscriptions | `10` | V1.2 | 3 |
| 80 | Stripe Terminal | `10` | V1.2 | 4 |
| 81 | Local Payment Methods | `10` | V1.2 | 3 |
| 82 | Recurring Appointments | `07` | V1.1 | 4 |
| 83 | Group Appointments | `07` | V1.1 | 4 |
| 84 | Family Booking | `06`, `07` | V1.1 | 4 |
| 85 | Multi-Currency Payout | `09`, `10` | V1.1 | 3 |
| 86 | Geo-Search | `05`, `12` | V1.1 | 4 |
| 87 | Marketing Vouchers | `09` | V1.1 | 3 |
| 88 | Dynamic Refund Window | `07`, `09` | V1.1 | 3 |
| 89 | Smart Waitlist | `07` | V1.1 | 3 |
| 90 | i18n RTL | `12` | V1.1 | 4 |
| 91 | Doctor Rotating Schedule | `05`, `07` | V1.1 | 3 |
| 92 | PWA Offline Mode | `medicare-user-app` | V1.1 | 4 |
| 93 | Multi-Clinic Dashboard | `04`, `13` | V1.2 | 3 |
| 94 | Blogs / CMS | `12` | V2 | 3 |
| 95 | Content Workflow | `12` | V2 | 3 |
| 96 | Custom Report Builder | `13` | V3 | 3 |
| 97 | On-Prem AI Gateway | `19` | V3 | 3 |
| 98 | White-Label Mobile | `medicare-user-app` | V3 | 3 |
| 99 | SSO / OIDC | `02` | V3 | 3 |
| 100 | HL7 v2 ADT | `19` | V3 | 3 |
| 101 | DICOM Viewer | `08`, `19` | V3 | 3 |
| 102 | Lab Instrument (ASTM) | `19` | V4 | 3 |
| 103 | HIS Bridge | `19` | V4 | 3 |
| 104 | ER Triage | `06`, `07` | V4 | 3 |
| 105 | Nursing Station | `06` | V4 | 3 |
| 106 | Centralized Pharmacy | `08` | V4 | 3 |
| 107 | Equipment Maintenance | `04` | V4 | 3 |
| 108 | Population Health | `09`, `13` | V5 | 1 |
| 109 | De-identification Pipeline | `20` | V5 | 1 |
| 110 | Research Cohort Tooling | `06`, `08` | V5 | 1 |
| 111 | Public Health Reporting | `13`, `19` | V5 | 1 |

**Total V2+ modules designed: 54.**
**Total V1 modules: 57.**
**Grand total: 111 modules.**

---

## Appendix B · V2 Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                          Patient (Flutter)                          │
└────────┬──────────────────────────────────────────────┬──────────────┘
         │ HTTPS / WSS                                  │ HTTPS
         ▼                                              ▼
┌────────────────────┐                       ┌────────────────────┐
│   Vercel Edge      │                       │   CloudFront       │
│   (Next.js 15)     │                       │   (mobile API)     │
└────────┬───────────┘                       └────────┬───────────┘
         │ WSS (Reverb)                             │ HTTPS
         ▼                                          ▼
┌─────────────────────────────────────────────────────────────────────┐
│                       medicare-api (Laravel 11)                      │
│                                                                       │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐         │
│  │   Auth     │ │  Tenant    │ │  Appts     │ │   Rx       │ ...     │
│  └────────────┘ └────────────┘ └────────────┘ └────────────┘         │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐         │
│  │  Billing   │ │  Payments  │ │  Notif     │ │ Engagement │ ...     │
│  └────────────┘ └────────────┘ └────────────┘ └────────────┘         │
│                                                                       │
│  ┌─────────────────────────────────────────────────────────────────┐ │
│  │  V2 Layer:  Pathology  Telemed  AI  Chat  Plugins  Insurance   │ │
│  └─────────────────────────────────────────────────────────────────┘ │
└───────┬──────────────┬────────────────┬────────────────┬──────────────┘
        │              │                │                │
        ▼              ▼                ▼                ▼
┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐
│  MySQL 8     │ │  Redis 7     │ │  S3          │ │  Reverb      │
│  (per region)│ │  (cache+q)   │ │  (files+rec) │ │  (WSS)       │
└──────────────┘ └──────────────┘ └──────────────┘ └──────┬───────┘
                                                          │
        ┌─────────────────────────────────────────────────┘
        ▼
┌──────────────────────────────────────────────────────────────────┐
│  Sidecar services (V2)                                           │
│  ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐    │
│  │ medicare-ai-    │ │ medicare-lab-   │ │ medicare-fhir-  │    │
│  │ service (Py)    │ │ gateway (Java)  │ │ gateway (Node)  │    │
│  └─────────────────┘ └─────────────────┘ └─────────────────┘    │
│  ┌─────────────────┐ ┌─────────────────┐                        │
│  │ coturn TURN/    │ │ LiveKit Cloud   │                        │
│  │ STUN            │ │ (telemedicine)  │                        │
│  └─────────────────┘ └─────────────────┘                        │
└──────────────────────────────────────────────────────────────────┘
        │
        ▼
┌──────────────────────────────────────────────────────────────────┐
│  External integrations                                            │
│  ┌────────┐ ┌─────────┐ ┌────────┐ ┌──────┐ ┌────────┐ ┌─────┐  │
│  │ Razorpay│ │ Stripe  │ │ Paystack│ │ ABDM │ │WhatsApp│ │ ... │  │
│  └────────┘ └─────────┘ └────────┘ └──────┘ └────────┘ └─────┘  │
└──────────────────────────────────────────────────────────────────┘
```

---

## Appendix C · Reading order

If you are new to the V2 plan, read in this order:
1. §1 (what V2 is)
2. §2 (release lanes)
3. §3 (feature catalog)
4. §4 (module touchpoints — only your module)
5. §5 (feature flags — every dev needs this)
6. §6 (data model)
7. §7 (billing) — only if you touch money
8. §8 (security) — only if you touch PHI
9. §9 (observability) — only if you own a service
10. §10 (open questions)

For specific module updates, jump to §4 and find your module.

---

> **Last reviewed:** 2026-06-04 · **Next review:** 2026-07-01 (every release cycle).
> **Maintainer:** Architecture Guild. PRs welcome via `docs/14-V2-STUBS.md`.
