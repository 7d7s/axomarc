> **⚠ TRACK UPDATE (2026-08-19):** The API backend track has moved to **Rust**. For the Rust kernel PRD/SRS/design/execution plan see [`docs/rust/`](rust/README.md). This document remains the authoritative **domain reference** (modules, flows, V2 stubs); technology statements about Laravel/MySQL apply to the legacy track only.
# Medicare — System Architecture

> **Version:** V1.1 (V2-aware) · **Last updated:** 2026-06-04
> **Status:** Living document — updated as design evolves.
> **Audience:** Backend, frontend, mobile, DevOps, QA, and any reviewer.
> **Companion:** For a comprehensive V2 plan, see `14-MODULE-V2-STUBS.md` (V2 Master Integration Map).

---

## 1. Purpose of this Document

This is the **master architectural blueprint** for the Medicare Modernized Clone. Every other doc in `/docs` is a deep-dive into one slice. If a new engineer reads **only this document**, they should be able to:

- Visualize the system at a glance.
- Understand why each technology was chosen.
- Find the right doc to read next for any specific concern (DB, API, security, deployment, etc.).
- Know the non-negotiable architectural rules and how to add new features without breaking them.

---

## 2. Product in One Paragraph

Medicare is a **multi-tenant-capable, multi-platform Clinic & Hospital Management System** that lets a single doctor practice, a multi-specialty clinic, or a hospital network run:

- A **public website** where patients browse doctors and book appointments.
- A **patient mobile app** (Android + iOS + Web) for booking, prescriptions, vitals, family profiles, payments, and video consultations.
- A **doctor mobile app** (Android + iOS) for managing today's schedule, patient check-ins, prescriptions, and follow-ups.
- A **back-office admin panel** for staff to manage doctors, patients, schedules, billing, settings, and reports.

It is sold in the original CodeCanyon product in 5 bundles. We rebuild it on a modern stack and add capabilities the original lacks: **realtime WebSockets (queue + live calling), modern UI, multi-tenant SaaS option, mobile + web parity, and a domain-activation installer**.

---

## 3. High-Level System Diagram

```
┌──────────────────────────────────────────────────────────────────────────┐
│                              PUBLIC INTERNET                             │
└──────────────────────────────────────────────────────────────────────────┘
                                       │
        ┌──────────────────────────────┼──────────────────────────────┐
        │                              │                              │
        ▼                              ▼                              ▼
┌────────────────┐            ┌────────────────┐            ┌────────────────┐
│  PATIENT WEB   │            │  USER APP      │            │  DOCTOR APP    │
│  Next.js 15    │            │  Flutter 3.24  │            │  Flutter 3.24  │
│  Tailwind +    │            │  Android, iOS  │            │  Android, iOS  │
│  shadcn/ui     │            │  (Web later)   │            │                │
└──────┬─────────┘            └──────┬─────────┘            └──────┬─────────┘
       │ HTTPS                       │ HTTPS                       │ HTTPS
       │                             │                             │
       │                             │ FCM (push)                  │ FCM (push)
       │                             │ Firebase OTP                 │ Firebase OTP
       │                             │                             │
       └─────────────┬───────────────┴─────────────┬───────────────┘
                     │                             │
                     │ WebSocket (Reverb)         │ WebSocket (Reverb)
                     │                             │
                     ▼                             ▼
        ┌─────────────────────────────────────────────────────┐
        │              NGINX (TLS terminator)                │
        └──────────────┬─────────────────────┬───────────────┘
                       │                     │
                       ▼                     ▼
        ┌──────────────────────┐  ┌──────────────────────┐
        │  medicare-web (SPA)  │  │  medicare-api        │
        │  Next.js 15          │  │  Laravel 11 (PHP 8.3)│
        │  Admin + Patient Web │  │  REST + Sanctum      │
        │  Tailwind + shadcn   │  │  Reverb WS server    │
        └──────────┬───────────┘  └──────────┬───────────┘
                   │ HTTPS                    │
                   └─────────┬────────────────┘
                             │
                             ▼
        ┌──────────────────────────────────────────────────────┐
        │              DATA + INFRA LAYER                      │
        │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌─────────┐  │
        │  │ MySQL 8  │ │ Redis 7  │ │ Object   │ │ Horizon │  │
        │  │          │ │ cache +  │ │ Storage  │ │ queue   │  │
        │  │          │ │ queues   │ │ (S3-like)│ │ monitor │  │
        │  └──────────┘ └──────────┘ └──────────┘ └─────────┘  │
        └──────────────────────────────────────────────────────┘
                             │
        ┌────────────────────┼────────────────────┐
        │                    │                    │
        ▼                    ▼                    ▼
┌──────────────┐    ┌──────────────┐    ┌──────────────────┐
│ Firebase     │    │ Zoom S2S     │    │ Payment Gateways │
│ - Auth (OTP) │    │ OAuth        │    │ - Razorpay       │
│ - FCM (push) │    │ - Create     │    │ - Stripe         │
│ - Firestore  │    │ - Update     │    │ - Paystack       │
│   (chat)     │    │ - Delete     │    │ (webhooks)       │
└──────────────┘    └──────────────┘    └──────────────────┘
```

---

## 4. Codebases (4 Repos)

| Repo | Tech | Purpose | Hosting |
|---|---|---|---|
| `medicare-api` | Laravel 11 (PHP 8.3) + MySQL 8 + Redis 7 | REST API, queue worker, scheduler, WebSocket broadcaster | Hetzner VPS behind Nginx |
| `medicare-web` | Turborepo → Next.js 15 (App Router) + Tailwind + shadcn/ui | Admin panel + patient website (single Next.js monorepo with two `apps/`) | Vercel or Hetzner |
| `medicare-user-app` | Flutter 3.24+ (Riverpod, GoRouter, Dio) | Patient app for Android + iOS (Web later) | App Store + Play Store |
| `medicare-doctor-app` | Flutter 3.24+ | Doctor app for Android + iOS | App Store + Play Store |

The **single API** is the source of truth. The Next.js and Flutter apps are pure presentation + integration layers.

---

## 5. The Five Architectural Principles (Non-Negotiable)

### Principle 1: API is the Only Authority
No business logic in the frontend. The Flutter and Next.js apps **never** write directly to the database, never compute totals, never decide state transitions. They ask the API, the API responds. This is what allows the same backend to drive 4 different front-ends safely.

### Principle 2: Tenancy Is an HTTP Boundary, Not a Codebase Branch
Multi-tenant mode is **runtime configuration** (`MEDICARE_TENANCY=true`), not a separate Laravel install. Every business table has a `tenant_id` column (nullable for single-tenant mode). The tenant is resolved from a **subdomain** (`clinic-a.medicare.app`) or **path** (`/t/clinic-a/...`) or the install default. All Eloquent queries automatically scope by tenant — see `03-MODULE-TENANCY.md`.

### Principle 3: Every Long-Running Task Is a Job
Anything that could take >500 ms, or that could fail and need retry (emails, FCM pushes, Zoom API calls, payment webhooks, PDF rendering, email notifications) is dispatched to the queue. The HTTP request returns a **202 Accepted** with a tracking id; the client subscribes to a Reverb channel for completion. This is what makes the API feel instant even on bad networks.

### Principle 4: Realtime When It Matters, REST When It Doesn't
- **WebSocket (Reverb)** for: queue screen, live patient calling, per-appointment status updates, chat, dashboard live counts.
- **REST polling (5–15 s)** for: appointment list, transaction list, anything that the user actively navigates to. Polling keeps the implementation simple and falls back gracefully if Reverb is down.
- **FCM push** for: anything that should wake a sleeping phone (appointment reminders, prescription ready, payment receipt).

### Principle 5: Audit Everything That Touches PHI
Every read/write of a Patient, Appointment, Prescription, or Transaction row is logged to the `audit_logs` table (who, what, when, before/after diff). This is required for HIPAA/GDPR and is the only way to debug a complaint months later. Logs are append-only — never deleted by application code.

---

## 6. Why This Stack (Rationale)

| Decision | Why | Trade-off |
|---|---|---|
| **Laravel 11** | Mature, batteries-included, huge healthcare ecosystem (Spatie, Filament, Sanctum). PHP 8.3 is fast. | Not the trendiest; hiring can be hard. |
| **MySQL 8** | Universal, well-understood, JSON columns for flexible data, full-text search. | Newer features (window functions, CTEs) exist; we'll use them. |
| **Redis 7** | Cache + queue + Reverb broker + session + broadcast in one. | Another moving part. |
| **Reverb** | First-party Laravel WebSockets, no per-message cost, scale via Redis pub/sub. | Need to keep the worker process running. |
| **Sanctum** | Token + cookie auth; simpler than Passport for our needs. | No OAuth flows (we don't need them). |
| **Spatie Permissions** | Battle-tested RBAC with roles and granular permissions. | Couples to their package. |
| **Spatie MediaLibrary** | Handles file uploads + conversions; works with local + S3. | Tight coupling, but worth it. |
| **Next.js 15 App Router** | SSR for SEO (patient website), RSC for fast admin, Turborepo for shared components. | More concepts to learn. |
| **shadcn/ui** | Own your components, copy-paste, themeable. No upgrade churn. | Have to maintain our own component library. |
| **Tailwind v4** | Fast, consistent, design tokens enforced. | Verbose HTML. |
| **TanStack Table v8** | Headless, typesafe, handles 100k rows. | More config than MUI. |
| **Recharts** | Simple, declarative, good defaults. | Limited chart types; we don't need more. |
| **Flutter 3.24** | One codebase for Android + iOS, performance, mature healthcare templates. | 3 targets = more matrix testing. |
| **Riverpod** | Testable state, no BuildContext dependency, easy to refactor. | Learning curve. |
| **Firebase Auth + FCM** | Phone OTP "just works" worldwide, free push for our scale. | Vendor lock-in; mirrored by Twilio fallback. |
| **Hetzner VPS** | €30/mo for prod, EU/US data residency, no surprise bills. | DIY ops (we use Forge). |
| **Laravel Forge** | 5 min to a secured PHP server, deploys on git push, daily backups. | $12/mo per server. |
| **Sentry** | First-class error tracking for Laravel + Next.js + Flutter. | Free tier is enough until ~50k events/mo. |

---

## 7. Module Map (the 50+ Modules at a Glance)

A module is a self-contained vertical slice with **DB tables, API routes, business services, events, and tests**. See `01-MODULES-INDEX.md` for the full list.

```
medicare-api/
├── app/Modules/
│   ├── Auth/                  # OTP, email/password, sessions, password reset
│   ├── Tenancy/               # Subdomain/path tenant resolver
│   ├── Users/                 # Admins, staff, role assignments
│   ├── Cities/
│   ├── Clinics/
│   ├── Departments/
│   ├── Specializations/
│   ├── Services/
│   ├── Doctors/               # Profile, schedule, time slots
│   ├── Patients/              # MRN, demographics
│   ├── FamilyMembers/
│   ├── Vitals/
│   ├── PatientFiles/
│   ├── Medicines/             # Master list for prescriptions
│   ├── Appointments/          # 7-state machine, calendar, queue, check-in
│   ├── Queue/                 # Tokens, live display, live calling
│   ├── CheckIn/               # QR scanning
│   ├── Prescriptions/         # Rx CRUD, print, written-style fallback
│   ├── Billing/               # Invoices, transactions, refunds
│   ├── Payments/              # Razorpay, Stripe, Paystack, webhooks
│   ├── Coupons/               # Discount management
│   ├── Taxes/                 # Tax master
│   ├── Reviews/               # Doctor ratings & reviews
│   ├── Testimonials/          # Marketing testimonials
│   ├── Banners/               # Home page banners
│   ├── LoginSlides/           # Onboarding carousel
│   ├── Languages/             # Dynamic multilingual
│   ├── Currencies/            # Symbol, position, precision
│   ├── Notifications/         # FCM + email + in-app
│   ├── VideoCalls/            # Zoom S2S integration
│   ├── Settings/              # All tabs (basic, web, mobile, video, firebase)
│   ├── Reports/               # Dashboard, registrations, graphs
│   ├── AuditLogs/             # PHI change tracking
│   └── Subscriptions/         # V4 stubs (schema only V1)
```

Each module has a `Service` class that contains all business logic. Controllers are thin: validate → call service → return response.

---

## 8. Request Lifecycle (How a Patient Books an Appointment)

The most important flow in the system. Walk through it once to see every architectural rule in action.

```
[Flutter App]                         [Laravel API]                          [DB / Queue / Reverb]
  │                                       │                                       │
  │ 1. Tap "Book" on doctor page          │                                       │
  │  GET /api/v1/public/doctors/42        │                                       │
  │ ─────────────────────────────────────► │                                       │
  │                                       │ 2. Resolve tenant (subdomain/path)    │
  │                                       │ 3. Apply TenantScope global scope     │
  │                                       │ 4. Eloquent: Doctor::find(42)         │
  │                                       │ ────────────────────────────────────► │
  │                                       │ ◄──────────────────────────────────── │
  │ ◄─────────────────────────────────────│ 5. Return doctor + next 7 days slots  │
  │                                       │                                       │
  │ 6. User picks date+slot, taps Pay     │                                       │
  │  POST /api/v1/payments/razorpay/order │                                       │
  │ {appointment_data, slot_id}           │                                       │
  │ ─────────────────────────────────────► │                                       │
  │                                       │ 7. Validate via FormRequest           │
  │                                       │ 8. Begin DB transaction               │
  │                                       │ 9. Lock slot, insert Appointment      │
  │                                       │     status=Confirmed                  │
  │                                       │ 10. Insert Invoice (status=unpaid)    │
  │                                       │ 11. Call RazorpayService->createOrder │
  │                                       │ 12. Commit transaction                │
  │                                       │ 13. Dispatch PushNotificationJob       │
  │                                       │ ────────────────────────────────────► │
  │ ◄─────────────────────────────────────│ 14. Return { order_id, key, amount }  │
  │                                       │                                       │
  │ 15. RazorpayCheckout UI opens         │                                       │
  │ 16. User pays                         │                                       │
  │                                       │                                       │
  │                                       │ 17. Razorpay webhook fires            │
  │                                       │     POST /api/v1/webhooks/razorpay    │
  │                                       │ ◄──────────────────────────────────── │
  │                                       │ 18. Verify HMAC signature             │
  │                                       │ 19. Find appointment by order_id      │
  │                                       │ 20. Insert Transaction + Payment      │
  │                                       │ 21. Mark invoice paid                 │
  │                                       │ 22. Broadcast AppointmentBooked event │
  │                                       │ ────────────────────────────────────► │
  │                                       │ 23. Broadcast via Reverb              │
  │ 24. App hears "appointment.booked"   ◄─────────────────────────────────────── │
  │ 25. Navigate to confirmation screen   │                                       │
```

This single flow exercises: **tenancy resolution → validation → DB transaction → external API call → webhook handling → idempotency → queue job → realtime broadcast**. Every other flow is a variation of this pattern.

---

## 9. Cross-Cutting Concerns

### 9.1 Configuration
All configuration lives in `.env`, validated by `config/medicare.php` (a single config file we own). Critical keys:

```dotenv
APP_NAME=Medicare
APP_ENV=production
APP_DEBUG=false
APP_URL=https://api.medicare.app

# Tenancy
MEDICARE_TENANCY=false                # true = multi-tenant mode
MEDICARE_TENANT_RESOLVER=subdomain    # subdomain|path
MEDICARE_BASE_DOMAIN=medicare.app

# Database
DB_CONNECTION=mysql
DB_HOST=127.0.0.1
DB_DATABASE=medicare
DB_USERNAME=medicare
DB_PASSWORD=********

# Cache/Queue/Redis
REDIS_HOST=127.0.0.1
REDIS_PASSWORD=null
REDIS_PORT=6379
QUEUE_CONNECTION=redis
CACHE_STORE=redis
SESSION_DRIVER=redis

# Broadcasting (Reverb)
BROADCAST_CONNECTION=reverb
REVERB_APP_ID=medicare
REVERB_APP_KEY=********
REVERB_APP_SECRET=********
REVERB_HOST=0.0.0.0
REVERB_PORT=8080
REVERB_SCHEME=https

# Mail
MAIL_MAILER=smtp
MAIL_HOST=smtp.eu.mailgun.org
MAIL_PORT=587
MAIL_USERNAME=********
MAIL_PASSWORD=********
MAIL_FROM_ADDRESS=no-reply@medicare.app
MAIL_FROM_NAME="${APP_NAME}"

# Firebase (one project shared across all 4 apps)
FIREBASE_PROJECT_ID=medicare-prod
FIREBASE_CLIENT_ID=********
FIREBASE_CLIENT_SECRET=********
FIREBASE_SERVICE_ACCOUNT_JSON={"type":"service_account",...}

# Zoom
ZOOM_ACCOUNT_ID=********
ZOOM_CLIENT_ID=********
ZOOM_CLIENT_SECRET=********

# Payment gateways
RAZORPAY_KEY_ID=********
RAZORPAY_KEY_SECRET=********
RAZORPAY_WEBHOOK_SECRET=********
STRIPE_PUBLISHABLE_KEY=pk_live_********
STRIPE_SECRET_KEY=sk_live_********
STRIPE_WEBHOOK_SECRET=whsec_********
PAYSTACK_PUBLIC_KEY=pk_live_********
PAYSTACK_SECRET_KEY=sk_live_********
PAYSTACK_WEBHOOK_SECRET=********

# API key for mobile/web client authentication
API_KEY=********

# Sentry
SENTRY_LARAVEL_DSN=https://********@sentry.io/********
SENTRY_TRACES_SAMPLE_RATE=0.2

# Feature flags
FEATURE_PATHOLOGY=false
FEATURE_AI_CHAT=false
FEATURE_BLOGS=false
FEATURE_SUBSCRIPTIONS=false
```

### 9.2 Error Model
All API errors return a **RFC 7807 Problem Details** JSON shape:

```json
{
  "type": "https://medicare.app/errors/validation",
  "title": "Validation failed",
  "status": 422,
  "detail": "The given data was invalid.",
  "instance": "/api/v1/appointments",
  "errors": {
    "doctor_id": ["Doctor is required"],
    "slot": ["Slot is already booked"]
  },
  "trace_id": "01J7XQ8Y4HM0F8C3K2RYN3WX9V"
}
```

`trace_id` is the Sentry event id, returned to the user so support can look it up.

### 9.3 Versioning
- API: `/api/v1/...` — when breaking changes are needed, v2 is a new prefix; v1 keeps working for 12 months.
- Mobile: minimum supported app version is checked at login; old apps get a "please update" screen.
- DB: Laravel migrations are append-only; never edit a shipped migration.

### 9.4 Time & Timezones
- All timestamps in DB are `UTC` (`timestamp` type).
- Clinic has a `timezone` field (e.g., `Asia/Kolkata`). API converts UTC ↔ clinic timezone at the boundary.
- Flutter app uses `intl` and the clinic's timezone for display.
- Cron jobs run in **server timezone** but compute "today" in the **clinic's timezone**.

### 9.5 Money
- All money fields are stored as **integer minor units** (paisa/cents/kobo). Never as float.
- Currency is stored alongside each monetary record.
- Formatting uses the clinic's `currency` config (symbol, position, decimal precision).
- Tax is applied per invoice line, with a `tax_mode` of `inclusive` or `exclusive`.

### 9.6 I18n
- API responses can include a `lang` query param (`?lang=hi`).
- All user-facing strings live in `lang/{locale}.json` files, with the database-backed translation table overriding at runtime.
- Default seed languages: `en`, `hi`, `ar`, `es`. RTL supported for `ar`.

---

## 10. Environments

| Env | Purpose | URL pattern | DB |
|---|---|---|---|
| `local` | Developer machine | `localhost:8000` | Local Docker MySQL |
| `staging` | Pre-prod demos, UAT, customer reviews | `staging-api.medicare.app` | `medicare_staging` |
| `production` | Live | `api.medicare.app` (or per-tenant) | `medicare_prod` |

Each environment has its own:
- Firebase project (so push tokens don't mix).
- Razorpay/Stripe/Paystack test vs live keys.
- Sentry project.
- Zoom OAuth app (test vs live).

---

## 11. Non-Functional Requirements

| Concern | Target |
|---|---|
| **Availability** | 99.5% (single VPS) — adds 99.9% with managed Reverb on Laravel Cloud |
| **Latency p95** | <300 ms for any REST endpoint on staging |
| **WebSocket fanout** | 1,000 concurrent connections per Reverb instance |
| **Uptime monitor** | BetterUptime pinging `/health` every 60s |
| **Backup frequency** | DB daily, Object Storage daily, retained 30 days |
| **Recovery RTO** | <4 hours (rebuild + restore from Forge backup) |
| **Recovery RPO** | <24 hours (last daily backup) |
| **Audit log retention** | 7 years (HIPAA-aligned) |
| **API rate limit** | 60 req/min/IP for public, 600 req/min/user for authed |

---

## 12. Out of Scope (V1)

- **Pathology / Lab module** — V2. Schema is shipped, no UI.
- **AI Chat Doctor Recommendation** — V2. Endpoint stubbed to return 501.
- **Blog Module** — V2. Schema only.
- **Subscription Plans** — V2. Schema only.
- **iOS Push (APNs)** — V2. Android FCM works; iOS via Firebase Cloud Messaging on iOS works in V1 actually.
- **EHR / FHIR export** — V3. Internal schema is FHIR-friendly but no `/fhir/...` endpoints.
- **Patient-facing native iOS push via APNs tokens direct** — handled by Firebase, which manages APNs.
- **Offline mode for Flutter apps** — V2.

---

## 13. Where to Go Next

| You want to… | Read |
|---|---|
| Know every module in detail | `01-MODULES-INDEX.md` |
| Understand auth in depth | `02-MODULE-AUTH.md` |
| Understand multi-tenant | `03-MODULE-TENANCY.md` |
| Design DB tables | `15-DATABASE-SCHEMA.md` |
| Add a new REST endpoint | `16-API-REFERENCE.md` |
| Wire up a realtime feature | `17-REALTIME.md` |
| Walk a user through a flow | `18-USER-FLOWS.md` |
| Plug in Zoom, Firebase, or a payment gateway | `19-INTEGRATIONS.md` |
| Ship to production | `21-DEPLOYMENT.md` |
| Run the installer wizard | `22-INSTALLER.md` |
| Set up local dev | `23-DEVELOPMENT-SETUP.md` |
| Plan V2+ features | `24-ROADMAP.md` |
| Pass a security review | `20-SECURITY-COMPLIANCE.md` |
| Plan V2+ features | `14-MODULE-V2-STUBS.md` (master), `24-ROADMAP.md` (timeline) |

---

## 14. V2 Architecture (Appendix)

> This section describes what changes in V2 and how the V1 architecture evolves. For the full V2 plan, see `14-MODULE-V2-STUBS.md`.

### 14.1 V2 service decomposition

V1 is a monolith: Laravel serves HTTP, queue, scheduler, and WebSocket. V2 introduces **sidecar services** that live outside the Laravel app and are called via internal APIs (mTLS, service mesh):

| Service | Language | Purpose | Hosted |
|---|---|---|---|
| `medicare-api` (existing) | PHP 8.3 / Laravel 11 | V1 modules + V2 features that fit cleanly | Hetzner / K8s |
| `medicare-ai-service` | Python 3.12 / FastAPI / vLLM | Symptom checker, clinical scribe, semantic search, embeddings | GPU nodes in `us-east-1` and `eu-west-1` |
| `medicare-lab-gateway` | Java 21 / Spring Boot | ASTM E1394 instrument driver, lab partner adapters (1mg, Thyrocare, Quest, LabCorp) | Internal only, no public ingress |
| `medicare-fhir-gateway` | Node 22 / HAPI-FHIR | FHIR R4 read (V2) + write (V3), ATNA audit | Public, strict rate limits |
| `medicare-realtime-ice` | C (coturn) | TURN/STUN for WebRTC NAT traversal | Same region as Reverb, UDP/TCP :3478 |
| `medicare-video` (managed) | — | LiveKit Cloud (primary), Daily.co (fallback), Jitsi (self-hosted fallback) | SaaS + self-host |
| `medicare-search` | Rust / Meilisearch | Tenant-scoped search index (doctors, patients, prescriptions) | Per-region |

### 14.2 Service mesh

V2 introduces a service mesh for all sidecars:

- **Linkerd** (preferred over Istio — lighter, simpler mTLS).
- **mTLS** between every service-to-service call; certs rotated every 24h.
- **SPIFFE** IDs as service identity (`spiffe://medicare.app/ns/{namespace}/sa/{service-account}`).
- **Authorization policies** at mesh level (e.g., `medicare-ai-service` can read `doctors` and `patients` but not `transactions`).

### 14.3 Data layer additions

- **PostGIS** extension on `doctors` (geo-search).
- **pgvector** extension on `doctor_reviews` and `prescription_medicines` (embeddings).
- **Meilisearch** as a write-through cache for `GET /search` endpoints.
- **Redis Streams** for cross-service events (replacing some Reverb channels for service-to-service).
- **S3** for video recordings, lab reports, signed Rx PDFs, AI scribe audio (encrypted SSE-KMS).

### 14.4 V2 deployment topology

```
                        ┌──────────────────────────┐
                        │   Global Anycast (GSLB)  │
                        │   Route53 + Cloudflare   │
                        └────────────┬─────────────┘
                                     │
                ┌────────────────────┼────────────────────┐
                ▼                    ▼                    ▼
       ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐
       │  us-east-1      │  │  eu-west-1      │  │  ap-south-1     │
       │  (active)       │  │  (active)       │  │  (active)       │
       │  ALB → EKS      │  │  ALB → EKS      │  │  ALB → EKS      │
       │  - medicare-api │  │  - medicare-api │  │  - medicare-api │
       │  - medicare-ai  │  │  - medicare-ai  │  │  - realtime     │
       │  - realtime     │  │  - realtime     │  │  - search       │
       │  - search       │  │  - search       │  │                 │
       │  - lab-gateway  │  │                 │  │                 │
       │  - fhir-gateway │  │                 │  │                 │
       │  RDS MySQL      │  │  RDS MySQL      │  │  RDS MySQL      │
       │  ElastiCache    │  │  ElastiCache    │  │  ElastiCache    │
       └────────┬────────┘  └────────┬────────┘  └────────┬────────┘
                │                    │                    │
                └─────────── Cross-region read replica ──┘
                              (encrypted, KMS-region-locked)
```

V1 single-VPS tenants keep their existing deployment; only V2 SaaS tenants get the multi-region topology.

### 14.5 V2 API versioning

- V1 API: `/api/v1/*` (stable, no breaking changes for 24 months after V2 GA).
- V2 API: `/api/v2/*` (additive on V1, opt-in per tenant via `Accept` header or `?v=2`).
- V3 API: `/api/v3/*` (breaking changes allowed; V1/V2 sunset 12 months after V3 GA).
- **GraphQL gateway** (V2 opt-in): `/graphql` wraps `/api/v2/*` for read queries (V3 adds mutations).

### 14.6 V2 event bus

V1 broadcasts go through Reverb. V2 adds a **durable event bus** for cross-service events:

- **Redis Streams** (V2 default) — `events.{aggregate}.{action}` (e.g., `events.appointment.confirmed`).
- **Kafka** (V3, for >100k events/sec tenants) — same schema, different transport.
- **Schema Registry** — every event has a versioned JSON schema in `events/schemas/`.
- **Event sourcing** is **not** V2 — we use event-driven, not event-sourced (V4 introduces sourcing for the audit log only).

### 14.7 V2 plugin system (preview)

V2 ships a **plugin framework** (MML 3 at V2 GA):

- Plugins are **Composer packages** with a `medicare-plugin.json` manifest.
- Plugins register **service providers**, **routes** (with permission scopes), **event listeners**, **migrations**, **admin UI panels**, and **feature flags**.
- Plugins run **in-process** in V2 (faster, simpler). V3 ships **sidecar plugins** for untrusted code.
- Plugin storage is **namespaced** (every table has `plugin_id` column or lives in `plugin_*` schema).
- Plugin permissions are **capability-based** (`read:patients`, `write:prescriptions`, `send:notifications`).
- Plugin install/upgrade is **transactional**; on failure, full rollback.

### 14.8 V2 security additions

- **Field-level encryption** for `patients.aadhaar`, `patients.ssn`, `doctors.pan` (encrypted in app, never in DB).
- **Per-tenant KMS keys** (`tenants.kms_key_id` → AWS/GCP/Azure KMS).
- **BYOK** (Bring Your Own Key) for Enterprise.
- **Tenant-scoped service identities** (SPIFFE).
- **Data access log** (every PHI read recorded with reason, indexed for GDPR Article 15).
- **AI prompt firewall** (input/output filter for LLM calls; human-in-the-loop for medical advice).
- **TURN server auth** (short-lived HMAC credentials for WebRTC).

### 14.9 V2 cost & performance targets

| Resource | V1 (single VPS) | V2 (per-tenant) |
|---|---|---|
| Compute | 1× Hetzner CCX23 (€30/mo) | EKS Fargate, 0.5 vCPU / 1 GB per tenant |
| DB | 1× shared MySQL | RDS MySQL, 1 instance per 50 tenants |
| Object storage | 1× Hetzner Storage Box (€4/mo) | S3, ~$0.023/GB/mo |
| Bandwidth | 20 TB/mo included | CloudFront, $0.085/GB |
| Realtime | 1× Reverb on app server | Reverb cluster, 1000 conn per pod |
| AI | None | GPU node, $0.0001/1k tokens (markup) |
| Telemetry | BetterUptime free | Datadog, $0.10/host/day |

**Target gross margin:** 75% (V1: 90%, V2: 75%).

### 14.10 V2 migration path (for V1 tenants)

V1 tenants migrate to V2 on their own schedule:

1. **Step 1:** Install V2 schema (idempotent, additive).
2. **Step 2:** Enable V1.1 features (recurring, group, family booking, WhatsApp).
3. **Step 3:** Pilot V2 features with internal tenant.
4. **Step 4:** Enable per-tenant feature flags for V2 features.
5. **Step 5:** Subscribe to a plan (if not on perpetual).
6. **Step 6:** Move from shared DB to per-region DB (only if data residency required).

**No data loss, no downtime, no breaking API changes.**

### 14.11 V2 reading list

For each V2 service, read the relevant section of `14-MODULE-V2-STUBS.md` plus:
- `medicare-ai-service` → `19-INTEGRATIONS.md` §3 (LLM providers)
- `medicare-lab-gateway` → `19-INTEGRATIONS.md` §1 (lab partners)
- `medicare-fhir-gateway` → `16-API-REFERENCE.md` §14 (FHIR endpoints)
- `medicare-realtime-ice` → `17-REALTIME.md` §6 (WebRTC)
- `medicare-search` → `19-INTEGRATIONS.md` §9 (search providers)
