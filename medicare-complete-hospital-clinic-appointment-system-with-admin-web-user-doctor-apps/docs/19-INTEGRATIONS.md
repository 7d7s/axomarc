# 19 — Integrations (Firebase, Zoom, Payment Gateways, Email, Storage, Sentry, ClamAV)

> Every external system we depend on. What it does, how we connect, what we configure, how it fails, and how to test.

---

## 1. Firebase (Auth + FCM + Firestore V2)

### 1.1 What We Use It For

- **Firebase Phone Authentication** — for Realm A (patient/doctor mobile).
- **FCM (Firebase Cloud Messaging)** — push notifications.
- **Firestore** (V2) — chat / realtime features if needed.

### 1.2 Project Structure

| Env | Project | Purpose |
|---|---|---|
| dev | `medicare-dev` | Local development |
| staging | `medicare-staging` | QA / customer demos |
| prod | `medicare-prod` | Live |

**One Firebase project per environment** is critical so push tokens don't cross-contaminate.

### 1.3 Setup (One-Time per Project)

1. Create Firebase project in console.
2. Enable **Phone** sign-in provider.
3. Add Android app:
   - Package name: `app.medicare.user` (patient), `app.medicare.doctor` (doctor)
   - SHA-256 of debug + release keystore
4. Add iOS app:
   - Bundle ID: `app.medicare.user`, `app.medicare.doctor`
   - Download `GoogleService-Info.plist`
5. Add Web app (for V2 web push):
   - Download config JSON
6. **Authorized Domains** in Phone Auth settings:
   - `localhost`
   - `medicare.app`, `*.medicare.app`
   - All customer domains (in multi-tenant mode)
7. Create **Service Account** JSON (Project Settings → Service Accounts → Generate New Private Key).
8. Store service account JSON in API `.env` as `FIREBASE_SERVICE_ACCOUNT_JSON` (single line, escaped).

### 1.4 Server-Side Integration

Package: `kreait/laravel-firebase` (community wrapper around `kreait/firebase-php`).

```php
// config/firebase.php
return [
    'credentials' => json_decode(env('FIREBASE_SERVICE_ACCOUNT_JSON'), true),
    'project_id' => env('FIREBASE_PROJECT_ID'),
];
```

```php
// app/Modules/Auth/Services/FirebaseVerifier.php
use Kreait\Firebase\Factory;
use Kreait\Firebase\Auth as FirebaseAuth;

class FirebaseVerifier {
    private FirebaseAuth $auth;
    public function __construct() {
        $this->auth = (new Factory)
            ->withServiceAccount(config('firebase.credentials'))
            ->createAuth();
    }
    
    public function verifyIdToken(string $idToken): VerifiedIdToken {
        try {
            return $this->auth->verifyIdToken($idToken, true);  // check revoked
        } catch (\Throwable $e) {
            throw new OtpInvalidException('Invalid or expired token', previous: $e);
        }
    }
}
```

### 1.5 FCM (Push)

Already covered in `11-MODULE-NOTIFICATIONS.md`. The server uses the Firebase Admin SDK to send:

```php
use Kreait\Firebase\Messaging\CloudMessage;
use Kreait\Firebase\Messaging\Notification;

$message = CloudMessage::withTarget('token', $fcmToken)
    ->withNotification(Notification::create($title, $body))
    ->withData($data);

$report = $messaging->send($message);
```

### 1.6 Client-Side (Flutter)

Packages: `firebase_core`, `firebase_auth`, `firebase_messaging`.

```dart
// lib/firebase_setup.dart
class FirebaseSetup {
  static Future<void> init() async {
    await Firebase.initializeApp();
    FirebaseMessaging.onBackgroundMessage(_backgroundHandler);
  }
  
  static Future<String?> getFcmToken() async {
    return await FirebaseMessaging.instance.getToken();
  }
}
```

**Background handler** (must be a top-level function, not a class method):
```dart
@pragma('vm:entry-point')
Future<void> _backgroundHandler(RemoteMessage message) async {
  await Firebase.initializeApp();
  // We don't handle the data here — let the system show the notification
  // and the user tap will open the app and our handlers take over
}
```

### 1.7 Failure Modes

| Failure | Recovery |
|---|---|
| Firebase service down | App falls back to Twilio OTP (if `MEDICARE_OTP_PROVIDER=twilio`) |
| ID token expired (>1 hour) | App re-authenticates silently via Firebase SDK |
| `NotRegistered` from FCM | Token marked inactive; user re-registers on next login |
| iOS push certificate expired | Apple auto-renews; we monitor via Firebase |
| Service account leaked | Rotate immediately; old tokens still verify until expiry |

### 1.8 Cost

- Phone Auth: 10K verifications/month free; then $0.06/verification.
- FCM: unlimited, free.
- We're under 10K/month for V1 launch.

### 1.9 Testing

- **Test phone numbers** in Firebase Console (e.g., `+91 99999 00001` always receives code `123456`). Used in E2E tests.
- **FCM test tool** in Firebase Console for sending test pushes.
- **Local mock** for unit tests: `FirebaseAuth::shouldReceive('verifyIdToken')`.

---

## 2. Zoom (Video Calls)

### 2.1 What We Use It For

S2S OAuth integration to create / manage Zoom meetings for online consultations.

### 2.2 Setup

1. Create a Zoom **Marketplace** app (Server-to-Server OAuth).
2. Add scopes: `meeting:write:meeting:admin`, `meeting:read:meeting:admin`, `user:read:user:admin`.
3. Get Account ID, Client ID, Client Secret.
4. Configure redirect URL: `https://api.medicare.app/api/v1/webhooks/zoom` (for event subscriptions).
5. Subscribe to events: `meeting.started`, `meeting.ended`, `participant.joined`, `participant.left`.
6. Set `.env`:
   ```
   ZOOM_ACCOUNT_ID=...
   ZOOM_CLIENT_ID=...
   ZOOM_CLIENT_SECRET=...
   ```

### 2.3 OAuth Token

Server-to-Server OAuth: we exchange client credentials for a short-lived access token (1 hour). We cache the token in Redis, refresh 5 min before expiry.

```php
class ZoomClient {
    private string $baseUrl = 'https://api.zoom.us/v2';
    
    public function __construct(private TokenCache $cache) {}
    
    public function getAccessToken(): string {
        return $this->cache->remember('zoom:token', 3000, function () {
            $resp = Http::asForm()
                ->withBasicAuth(config('zoom.client_id'), config('zoom.client_secret'))
                ->post("https://zoom.us/oauth/token", [
                    'grant_type' => 'account_credentials',
                    'account_id' => config('zoom.account_id'),
                ]);
            return $resp->json('access_token');
        });
    }
    
    public function createMeeting(array $params): array {
        return Http::withToken($this->getAccessToken())
            ->post("{$this->baseUrl}/users/me/meetings", $params)
            ->json();
    }
}
```

### 2.4 Create Meeting

When doctor taps "Start Video Call" for an appointment:

```php
$meeting = $this->zoom->createMeeting([
    'topic' => "Consultation: {$patient->display_name} with Dr. {$doctor->display_name}",
    'type' => 2,  // scheduled
    'start_time' => $appointment->start_at->toIso8601String(),
    'duration' => $appointment->duration_minutes,
    'timezone' => Tenant::current()->timezone,
    'settings' => [
        'host_video' => true,
        'participant_video' => true,
        'join_before_host' => false,
        'mute_upon_entry' => true,
        'waiting_room' => true,
        'approval_type' => 0,  // automatically approve
    ],
]);
```

Store the response in `zoom_meetings`:
```php
$zoomMeeting = ZoomMeeting::create([
    'appointment_id' => $appointment->id,
    'zoom_meeting_id' => $meeting['id'],
    'join_url' => $meeting['join_url'],       // patient uses this
    'start_url' => $meeting['start_url'],     // doctor uses this
    'password' => $meeting['password'],
    'start_time' => $meeting['start_time'],
    'duration' => $meeting['duration'],
]);
```

### 2.5 Webhook Handler

```
POST /webhooks/zoom
```

Verifies Zoom's signature (`X-Zm-Signature` header = HMAC-SHA256 of body with webhook secret). Handles:
- `meeting.started` → mark appointment `in_consultation` if not already.
- `meeting.ended` → auto-end the video call record.
- `participant.joined` → audit log.
- `participant.left` → audit log.

### 2.6 Failure Modes

| Failure | Recovery |
|---|---|
| OAuth token expired | Auto-refresh (handled in cache) |
| Zoom API down | Show "Video unavailable, we'll call you" in app |
| Meeting not created (rare) | Fallback to phone call (V2) or just text-based consult |
| Patient can't open Zoom | Show web URL (works in any browser) |
| Recording fails | User notified; manual retry |

### 2.7 Cost

- Pro plan: $15/host/month, 100 participants max.
- For a clinic with 5 doctors: $75/month, acceptable.

### 2.8 Testing

- Zoom's test meeting endpoint.
- Mock `ZoomClient` in unit tests.
- E2E: book an online appointment, verify meeting URL works in browser.

---

## 3. Payment Gateways

See `10-MODULE-PAYMENTS.md` for full architecture. Quick setup here.

### 3.1 Razorpay

```
RAZORPAY_KEY_ID=rzp_live_...
RAZORPAY_KEY_SECRET=...
RAZORPAY_WEBHOOK_SECRET=...
```

- Dashboard: https://dashboard.razorpay.com
- API docs: https://razorpay.com/docs/api
- Webhook URL: register at https://dashboard.razorpay.com/app/webhooks

### 3.2 Stripe

```
STRIPE_PUBLISHABLE_KEY=pk_live_...
STRIPE_SECRET_KEY=sk_live_...
STRIPE_WEBHOOK_SECRET=whsec_...
```

- Dashboard: https://dashboard.stripe.com
- Webhook URL: register at https://dashboard.stripe.com/webhooks

### 3.3 Paystack

```
PAYSTACK_PUBLIC_KEY=pk_live_...
PAYSTACK_SECRET_KEY=sk_live_...
PAYSTACK_WEBHOOK_SECRET=...
```

- Dashboard: https://dashboard.paystack.com
- Webhook URL: register at https://dashboard.paystack.com/#/settings/developer

### 3.4 Test Mode

Each gateway has separate test/live keys. We use:
- `dev` env → all test keys
- `staging` env → gateway test mode (not "live" test, but actual sandbox)
- `prod` env → live keys

The `PaymentGatewayResolver` reads `config('medicare.payments.default_gateway')` and instantiates the right gateway.

### 3.5 Adding a New Gateway (e.g., PayPal)

1. Create `App\Modules\Payments\Gateways\PayPalGateway implements PaymentGateway`.
2. Add config in `config/services.php`.
3. Register in `PaymentGatewayResolver::for()`.
4. Add `PayPalService` Flutter wrapper.
5. Add webhook handler in `WebhookController::paypal()`.
6. Add to admin payment settings UI.
7. Add tests.

---

## 4. Email (SMTP)

### 4.1 Provider Choice

| Provider | When to use | Cost |
|---|---|---|
| **Mailgun** | Default for prod. EU + US regions. Best deliverability for transactional. | $0.80 / 1K emails |
| **Postmark** | Great for transactional. US + EU. | $1.25 / 10K |
| **Amazon SES** | Cheapest at scale. Requires careful bounce handling. | $0.10 / 1K |
| **SMTP generic** | For on-prem / air-gapped deployments. | Variable |

### 4.2 Configuration

```dotenv
MAIL_MAILER=smtp
MAIL_HOST=smtp.eu.mailgun.org
MAIL_PORT=587
MAIL_USERNAME=postmaster@medicare.app
MAIL_PASSWORD=...
MAIL_ENCRYPTION=tls
MAIL_FROM_ADDRESS=no-reply@medicare.app
MAIL_FROM_NAME="${APP_NAME}"
```

Per-tenant override in `settings.email`:
```json
{
  "from_address": "no-reply@apollo.medicare.app",
  "from_name": "Apollo Clinic",
  "reply_to": "support@medicare.app"
}
```

### 4.3 DNS Setup (Deliverability)

Required DNS records (set by DevOps at deployment):
- **SPF**: `v=spf1 include:mailgun.org ~all`
- **DKIM**: from Mailgun/SES dashboard
- **DMARC**: `v=DMARC1; p=quarantine; rua=mailto:dmarc@medicare.app`
- **Return-Path**: `bounces@medicare.app`

### 4.4 Bounce Handling

Webhooks from Mailgun/SES for:
- `bounce` (hard) → add to `email_suppression_list` permanently
- `complaint` (user marked spam) → same
- `unsubscribe` → same
- `soft_bounce` → retry up to 3 times then suppress

### 4.5 Templates

Laravel Markdown mailables for system emails; DB-stored templates for user-editable ones (see 11-NOTIFICATIONS).

---

## 5. Object Storage (S3-Compatible)

### 5.1 What We Store

- Patient files (lab reports, photos)
- Doctor avatars / covers
- Clinic logos / covers
- Banners
- Generated PDFs (invoices, prescriptions)
- Testimonial photos
- Handwritten Rx images
- Doctor signatures / stamps

### 5.2 Provider

| Provider | When to use |
|---|---|
| **AWS S3** | Default prod |
| **Cloudflare R2** | No egress fees — great for read-heavy (banners, avatars) |
| **DigitalOcean Spaces** | Simpler, S3-compatible |
| **MinIO** | Self-hosted for compliance-sensitive installs |
| **Local disk** | Dev only |

### 5.3 Configuration

```dotenv
FILESYSTEM_DISK=s3
AWS_ACCESS_KEY_ID=...
AWS_SECRET_ACCESS_KEY=...
AWS_DEFAULT_REGION=eu-central-1
AWS_BUCKET=medicare-prod-eu
AWS_URL=https://medicare-prod-eu.s3.eu-central-1.amazonaws.com
```

### 5.4 Per-Tenant Bucket

In multi-tenant mode, files are namespaced:
- Bucket: `medicare-prod-eu`
- Path: `tenants/{tenant_slug}/{resource}/{id}/...`

This lets us enforce per-tenant IAM policies if needed (V2 with per-tenant keys).

### 5.5 Upload Pipeline

- **Direct upload** (default for large files): API returns a pre-signed S3 PUT URL; client uploads directly. Avoids sending large files through the Laravel server.
- **Server upload** (for small files): API accepts multipart, uses `Spatie\MediaLibrary`.

### 5.6 Signed Download URLs

All file access is via short-lived signed URLs (5 min). API:
```
GET /patient/files/{id}/download
→ 302 to https://bucket.s3.amazonaws.com/path?X-Amz-Signature=...&X-Amz-Expires=300
```

### 5.7 ClamAV (Antivirus)

Patient files are scanned by ClamAV before being made available.

- **Local ClamAV** for self-hosted: `clamav-daemon` package.
- **Cloud scanner** for managed: `FILESCAN_API_KEY` in `.env`.

A `ScanFileJob` runs after upload. Until scanned, `patient_files.is_scanned=false` and download is blocked. The job is async, so the upload response is fast.

```php
class ScanFileJob {
    public function handle(ClamavScanner $scanner): void {
        $file = PatientFile::find($this->fileId);
        $result = $scanner->scan($file->getFullPath());
        if ($result->isInfected) {
            $file->update(['scan_status' => 'infected', 'scan_result' => $result->virus]);
            event(new FileInfected($file));  // notify admin
            $file->delete();
        } else {
            $file->update(['scan_status' => 'clean', 'scanned_at' => now()]);
        }
    }
}
```

---

## 6. Sentry (Error Tracking)

### 6.1 Setup

- Organization: `medicare`
- Projects: `medicare-api`, `medicare-web`, `medicare-user-app`, `medicare-doctor-app`
- DSN in each app's env.
- Release tracking via git SHA (`SENTRY_RELEASE=$(git rev-parse HEAD)`).

### 6.2 What We Capture

- All 500-level errors
- Unhandled exceptions
- Failed jobs (after all retries)
- Custom messages: `$sentry->captureMessage('Critical: payment gateway down')`
- Performance: 20% sample of transactions, 100% of slow ones (>1s)

### 6.3 What We DON'T Capture

- 4xx errors (except 401 floods which might indicate attack)
- PHI (Sentry scrubber configured to strip patient name, MRN, etc.)
- Auth tokens, API keys

### 6.4 Configuration

```php
// config/sentry.php
return [
    'dsn' => env('SENTRY_LARAVEL_DSN'),
    'traces_sample_rate' => env('SENTRY_TRACES_SAMPLE_RATE', 0.2),
    'profiles_sample_rate' => 0.1,
    'before_send' => function (Event $event) {
        // Strip PHI from breadcrumbs
        return $event;
    },
];
```

---

## 7. Redis (Cache + Queue + Reverb Broker)

### 7.1 Single Redis, Multiple Databases

| DB | Use |
|---|---|
| 0 | Default cache |
| 1 | Sessions |
| 2 | Queue (default) |
| 3 | Queue (broadcasts) |
| 4 | Reverb pub/sub |
| 5 | Horizon (queue monitoring) |

### 7.2 Configuration

```dotenv
REDIS_HOST=127.0.0.1
REDIS_PORT=6379
REDIS_PASSWORD=...
REDIS_CLIENT=phpredis    # or 'predis'
REDIS_PREFIX=medicare:    # all keys auto-prefixed
```

### 7.3 Failure Modes

- Redis down → app falls back to database cache (slow) and `database` queue (very slow). 503 only if all fails.
- Monitor with BetterUptime: ping `/health/ready` which checks Redis.

---

## 8. SMS (Twilio V2)

### 8.1 When V1

Realm A uses Firebase Phone Auth, which sends SMS via Firebase. We don't use Twilio in V1.

### 8.2 When V2 (and as OTP Fallback)

`MEDICARE_OTP_PROVIDER=twilio` switches the flow:
- App uses `twilio_verify` Flutter package.
- API calls Twilio Verify Services API to send + check codes.

```php
class TwilioVerifyService {
    public function send(string $phone): void {
        Http::withBasicAuth(config('services.twilio.sid'), config('services.twilio.token'))
            ->post("https://verify.twilio.com/v2/Services/{config('services.twilio.verify_sid')}/Verifications", [
                'To' => $phone,
                'Channel' => 'sms',
            ]);
    }
    
    public function check(string $phone, string $code): bool {
        $resp = Http::withBasicAuth(...)
            ->post(".../VerificationCheck", [
                'To' => $phone,
                'Code' => $code,
            ]);
        return $resp->json('status') === 'approved';
    }
}
```

---

## 9. BetterUptime (Uptime Monitoring)

### 9.1 Setup

- Monitor: `https://api.medicare.app/health`
- Interval: 60s
- Alerts: email + PagerDuty for downtime > 5 min.

### 9.2 Health Endpoints

```
GET /health       → 200 always (liveness, for k8s)
GET /health/ready → 200 only if DB + Redis + Reverb are all up (readiness)
```

---

## 10. PagerDuty (On-Call Alerts)

### 10.1 What Pages the On-Call

- API down > 5 min
- Error rate > 1% for 5 min
- Queue depth > 10K for 10 min
- Failed payment rate > 5% for 5 min
- Disk usage > 90%
- Memory usage > 90%

### 10.2 What Does NOT Page (Sentry warn instead)

- Single user error
- 4xx spike
- Degraded performance without user impact

---

## 11. PostHog / Plausible (V2 Analytics)

We start with Plausible (privacy-friendly, EU-hosted) for the patient web. PostHog for product analytics in the admin.

V1: Plausible only. PostHog in V2.

---

## 12. GitHub Actions (CI/CD)

Already covered in 21-DEPLOYMENT, but summarized:

- **CI** on every PR: lint, type-check, tests, security scan.
- **CD** on `main` merge: deploy to staging → run smoke tests → manual approval → deploy to prod.
- **Secrets** stored in GitHub Encrypted Secrets.
- **Docker images** pushed to GitHub Container Registry.

---

## 13. Environment Variable Reference (Master List)

All `.env` keys, grouped by integration:

```dotenv
# Core
APP_NAME=Medicare
APP_ENV=production
APP_KEY=base64:...
APP_DEBUG=false
APP_URL=https://api.medicare.app

# Database
DB_CONNECTION=mysql
DB_HOST=...
DB_DATABASE=medicare
DB_USERNAME=...
DB_PASSWORD=...

# Redis
REDIS_HOST=...
REDIS_PORT=6379
REDIS_PASSWORD=...
REDIS_PREFIX=medicare:

# Cache / Queue / Session
CACHE_STORE=redis
QUEUE_CONNECTION=redis
SESSION_DRIVER=redis

# Broadcasting (Reverb)
BROADCAST_CONNECTION=reverb
REVERB_APP_ID=medicare
REVERB_APP_KEY=...
REVERB_APP_SECRET=...
REVERB_HOST=0.0.0.0
REVERB_PORT=8080
REVERB_SCHEME=https

# Mail
MAIL_MAILER=smtp
MAIL_HOST=...
MAIL_PORT=587
MAIL_USERNAME=...
MAIL_PASSWORD=...
MAIL_ENCRYPTION=tls
MAIL_FROM_ADDRESS=no-reply@medicare.app
MAIL_FROM_NAME="${APP_NAME}"

# Firebase
FIREBASE_PROJECT_ID=medicare-prod
FIREBASE_SERVICE_ACCOUNT_JSON={"type":"service_account",...}

# Twilio (OTP fallback)
MEDICARE_OTP_PROVIDER=firebase     # firebase | twilio
TWILIO_SID=...
TWILIO_TOKEN=...
TWILIO_VERIFY_SID=...

# Zoom
ZOOM_ACCOUNT_ID=...
ZOOM_CLIENT_ID=...
ZOOM_CLIENT_SECRET=...
ZOOM_WEBHOOK_SECRET=...

# Razorpay
RAZORPAY_KEY_ID=...
RAZORPAY_KEY_SECRET=...
RAZORPAY_WEBHOOK_SECRET=...

# Stripe
STRIPE_PUBLISHABLE_KEY=...
STRIPE_SECRET_KEY=...
STRIPE_WEBHOOK_SECRET=...

# Paystack
PAYSTACK_PUBLIC_KEY=...
PAYSTACK_SECRET_KEY=...
PAYSTACK_WEBHOOK_SECRET=...

# AWS S3
AWS_ACCESS_KEY_ID=...
AWS_SECRET_ACCESS_KEY=...
AWS_DEFAULT_REGION=eu-central-1
AWS_BUCKET=medicare-prod-eu
AWS_URL=...

# ClamAV
CLAMAV_HOST=127.0.0.1
CLAMAV_PORT=3310
# OR
FILESCAN_API_KEY=...

# Sentry
SENTRY_LARAVEL_DSN=...
SENTRY_TRACES_SAMPLE_RATE=0.2
SENTRY_RELEASE=...

# Tenancy
MEDICARE_TENANCY=false
MEDICARE_TENANT_RESOLVER=subdomain
MEDICARE_BASE_DOMAIN=medicare.app

# Features
FEATURE_PATHOLOGY=false
FEATURE_AI_CHAT=false
FEATURE_BLOGS=false
FEATURE_SUBSCRIPTIONS=false

# API
API_KEY=...

# Cron monitoring
HORIZON_DOMAIN=horizon.medicare.app
HORIZON_PATH=horizon
```

---

## 14. Integration Testing Checklist

Before each release, verify every integration end-to-end in staging:

- [ ] Firebase OTP: real phone receives real SMS
- [ ] Firebase FCM: real device receives real push
- [ ] Razorpay: real test payment, real webhook fires, real refund works
- [ ] Stripe: same
- [ ] Paystack: same
- [ ] Zoom: real meeting created, both parties can join
- [ ] SMTP: real email arrives in inbox (not spam)
- [ ] S3: real upload + signed download works
- [ ] ClamAV: upload an EICAR test file → rejected
- [ ] Sentry: error from staging surfaces in Sentry dashboard
- [ ] Reverb: real-time event reaches connected client
- [ ] BetterUptime: down/up alerts work

---

## 15. V2 Integrations

> V1 ships 12 integrations. V2 adds 18 more, taking the platform to 30+ integrations by V3. This section is the **single source of truth** for every external service we depend on, with credentials, webhooks, rate limits, and a runbook link.

### 15.1 Lab partner integrations (V2)

| Partner | Region | API | Auth | Latency | Webhook |
|---|---|---|---|---|---|
| 1mg Labs | India | REST | API key | <2s | `1mg.order.status` |
| Thyrocare | India | REST | API key + HMAC | <3s | `thyrocare.order.status` |
| SRL Diagnostics | India | REST | OAuth 2.0 | <2s | `srl.order.status` |
| Quest Diagnostics | US | REST | OAuth 2.0 | <3s | `quest.order.status` |
| LabCorp | US | HL7 ORU | SFTP + cert | <5s | HL7 over MLLP |
| Metropolis Healthcare | India | REST | API key | <2s | `metropolis.order.status` |
| Suburban Diagnostics | India | REST | API key | <2s | `suburban.order.status` |
| Pathkind Labs | India | REST | API key | <2s | `pathkind.order.status` |

**Mediator pattern:** all partners go through `medicare-lab-gateway` (Java/Spring), which normalizes their different APIs into a single internal interface. V1 doesn't have this; V2 adds it.

**Adapter interface:**
```java
interface LabPartnerAdapter {
    LabOrderResponse submit(LabOrder order);
    Optional<LabResult> fetchResult(String externalOrderId);
    void acknowledgeResult(String externalResultId);
}
```

**Webhook handling:** every partner sends a webhook on status change. We dedupe via `external_event_id` UNIQUE index. `medicare-lab-gateway` exposes a single internal webhook for the Laravel app to consume.

### 15.2 LLM providers (V2)

| Provider | Models | Use case | Auth | Cost |
|---|---|---|---|---|
| **OpenAI** | gpt-4o, gpt-4o-mini, o1, o1-mini, whisper-1, dall-e-3 | Default for scribe, symptom checker, blog writer | API key (org-scoped) | $2.50-$15 / 1M tokens |
| **Anthropic** | claude-3.5-sonnet, claude-3-haiku | Long-context scribe, doctor rec | API key | $3-$15 / 1M tokens |
| **Google** | gemini-1.5-pro, gemini-1.5-flash | Multimodal (image + text for lab reports) | Service account JSON | $1.25-$7 / 1M tokens |
| **Mistral** | mistral-large, mixtral-8x22b | EU data residency, GDPR | API key | $2-$8 / 1M tokens |
| **vLLM (self-hosted)** | Qwen2.5-72B, Llama-3.1-70B | HIPAA/EU-BAIC, on-prem | None (local) | GPU cost |
| **Ollama (dev)** | qwen2.5:7b, llama3.1:8b | Local dev only | None | Free |

**Provider abstraction:** `LlmProvider` interface in `medicare-ai-service`:
```python
class LlmProvider(Protocol):
    async def complete(self, messages: list[dict], model: str, **kwargs) -> LlmResponse: ...
    async def stream(self, messages: list[dict], model: str, **kwargs) -> AsyncIterator[str]: ...
    async def embed(self, texts: list[dict], model: str) -> list[list[float]]: ...
```

**Per-tenant model selection:** `tenants.llm_provider` and `tenants.llm_model`. Enterprise tenants in EU can force Mistral; HIPAA tenants can force vLLM.

**Cost tracking:** every LLM call goes through `AiUsageService::record()` which logs to `ai_usage_log` (tenant, user, model, prompt_tokens, completion_tokens, cost_usd).

**Rate limits:** OpenAI 10k RPM (tier 5), Anthropic 4k RPM. `LlmRateLimiter` queues requests with priority; user-facing (symptom checker) > admin-facing (blog writer).

**Prompt injection defense:**
- Input filter: detect known injection patterns ("ignore previous instructions").
- Output filter: detect hallucinations, medical advice violations.
- Red-team test set: 50 known attacks, run nightly.
- Human-in-the-loop: AI output is always draft; doctor must approve.

### 15.2a On-Prem AI Gateway (`on-prem-ai-gateway`, V2.1, MML 3)

**1. Overview.** V2's AI Scribe and AI Symptom Checker (Turns 25-26) use a cloud LLM (OpenAI / Anthropic) as the default, with the PHI carefully minimized before the call. For Enterprise tenants with regulatory constraints — HIPAA (US), EU-BAIC (EU AI Act for high-risk medical AI), India DPDP Act 2023, or customer-mandated data residency — the default is **not acceptable**: PHI must not leave the tenant's trust boundary. The On-Prem AI Gateway is the V2.1 Enterprise add-on that swaps the cloud LLM for a **self-hosted, tenant-controlled inference server** (vLLM with Qwen2.5-72B or Llama-3.1-70B), running in the tenant's own infrastructure or in a private region we provision. MML 3 GA: vLLM deployment, model registry, per-tenant isolation, prompt-injection defense retained, no observability of PHI from our side.

**2. Topology.**
```
Tenant's VPC (or private region)
┌──────────────────────────────────────────────────────────────┐
│  Tenant's medicare-api (existing)                            │
│       │                                                       │
│       ▼ (HTTPS, mTLS)                                        │
│  medicare-ai-gateway  ◀─── runs in tenant VPC                │
│       │                                                       │
│       ▼ (HTTP, internal)                                     │
│  vLLM server (Qwen2.5-72B, GPU p4d.24xlarge or H100)         │
│       │                                                       │
│       ▼                                                       │
│  Whisper-large-v3 (STT for scribe)                           │
└──────────────────────────────────────────────────────────────┘
       ▲
       │ (only anonymized usage metrics)
       │
   Our SaaS control plane (sees token counts, NOT content)
```

**3. Deployment options.**
- **Tenant-hosted (BYO GPU):** tenant provisions 1+ `p4d.24xlarge` (8× A100) or `p5.48xlarge` (8× H100) in their AWS/GCP account; we provide a Terraform module + Helm chart that deploys `medicare-ai-gateway` + vLLM + Whisper; the gateway speaks our internal LlmProvider API to our `medicare-ai-service`. We do not have SSH or kubectl access to the tenant's cluster; we ship logs to the tenant's S3 bucket.
- **Private-region SaaS:** for tenants that don't want to operate GPUs, we provision a private region in our infrastructure, dedicated to that tenant, with a per-tenant KMS key (per `03-Tenancy §17.6`) and a per-tenant VPC peering. The cost is on the tenant's invoice.
- **Air-gapped:** for the most regulated tenants (defense, certain EU hospitals), we provide an offline Helm chart and a signed Docker image bundle. The tenant runs updates manually; we have zero connectivity.

**4. Domain Model.**
```php
Schema::create('ai_gateway_deployments', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->unique();
    $t->string('mode', 16);                                  // 'byo_gpu' | 'private_region' | 'air_gapped'
    $t->string('region', 32)->nullable();                    // 'tenant-vpc' | 'aws-eu-west-1-priv' | 'on-prem'
    $t->string('model_name', 64);                            // 'Qwen2.5-72B-Instruct' | 'Llama-3.1-70B-Instruct'
    $t->string('model_version', 32);
    $t->string('model_sha256', 64);
    $t->string('endpoint', 255);                             // internal HTTPS URL
    $t->string('auth_type', 16);                             // 'mtls' | 'bearer_token'
    $t->json('auth_config')->nullable();                     // cert SHA-256 fingerprints
    $t->string('status', 16)->default('provisioning');       // 'provisioning' | 'active' | 'draining' | 'failed'
    $t->timestamp('last_healthcheck_at')->nullable();
    $t->text('last_healthcheck_error')->nullable();
    $t->timestamps();
});

Schema::create('ai_model_versions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->string('model_family', 32);                          // 'Qwen2.5' | 'Llama-3.1' | 'Whisper'
    $t->string('version', 32);
    $t->string('hf_repo', 255);                              // HuggingFace repo
    $t->string('sha256', 64);
    $t->string('license', 64);                               // 'apache-2.0' | 'llama3-community' | 'qwen-research'
    $t->json('safety_eval_results')->nullable();             // {mmlu, medqa, usmle, toxicity, ...}
    $t->date('released_at');
    $t->boolean('is_recommended')->default(false);
});
```

**5. The `LlmProvider` interface** is the same; only the implementation differs. The on-prem implementation calls the tenant's vLLM endpoint; the cloud implementation calls OpenAI/Anthropic. The `medicare-ai-service` resolves the implementation per `tenants.llm_provider`.

**6. PHI handling.** No PHI leaves the tenant's trust boundary. The gateway accepts a request with the prompt + structured PHI; calls the local vLLM; returns the response. The gateway **does not** log prompt or response content — only token counts, latency, model version, and a SHA-256 of the prompt hash (for dedup). Our SaaS control plane sees the usage metrics for billing; an auditor on our side cannot read the prompt.

**7. Security & RBAC.** Per `20-Security §11.4`: the gateway runs as a non-root user, in a container with read-only root filesystem, with a strict seccomp profile, and with `cap_drop=ALL`. The vLLM model is loaded from a signed Docker image; the model weights are checksummed at boot; a tampered model refuses to start. The gateway's HTTPS endpoint is gated by mTLS (client cert pinned to the tenant's `medicare-api` CA). Air-gapped deployments additionally require a hardware security module (HSM) for the model-license signing key. Per `14-V2-Stubs §10 Q3` (the on-prem decision), the SLA is **best-effort** — if the tenant's GPU is down, AI features degrade gracefully (scribe falls back to manual; symptom checker shows "AI temporarily unavailable").

**8. Acceptance Criteria.**
- [ ] The on-prem gateway accepts a request from the tenant's `medicare-api` and returns a response in < 2x the cloud latency.
- [ ] No prompt or response content is logged by the gateway; an audit of the gateway's logs shows only token counts + hashes.
- [ ] The model's SHA-256 is verified at boot; a tampered model refuses to start.
- [ ] The gateway's CPU/memory is bounded (no runaway from a malformed prompt).
- [ ] A cloud-mode tenant's request to the on-prem gateway is rejected (cross-tenant isolation).
- [ ] Air-gapped mode: a 30-day offline test (no internet from the gateway) completes with all AI features functional.

**9. Real-World Test Scenarios.**
- *HIPAA tenant:* large US hospital chain provisions on-prem in their AWS GovCloud; the gateway passes an internal HIPAA audit; no PHI leaves the VPC; the AI Scribe runs as well as in cloud mode.
- *EU-BAIC tenant:* German hospital deploys on-prem; model license is Qwen2.5-72B (Apache 2.0, no US export-control concern); the gateway is in `eu-central-1`; no data leaves the EU.
- *Air-gapped:* military hospital has no internet; signed Docker images are delivered on a USB drive; the gateway runs offline for 30 days; an internal S3 bucket is the only persistence.
- *GPU failure:* tenant's GPU node crashes; the gateway returns 503 with "AI temporarily unavailable"; the Scribe falls back to manual; the symptom checker shows a banner; the patient is not blocked.
- *Model upgrade:* Qwen2.5-72B is replaced by Qwen3-72B; the new model is checksummed; the gateway is restarted; a canary test (5% of traffic) runs for 24h; full rollout if quality metrics hold.

**10. What this module is NOT:**
- Not a training platform. We ship pre-trained weights; we do not fine-tune per tenant in V2.1 (fine-tuning is a V5+ service).
- Not a general AI gateway. It serves the Medicare AI features only; it is not a multi-tenant LLM router.
- Not a free add-on. On-prem is Enterprise-tier only; the GPU cost (whether tenant's or ours) is on the tenant's invoice.

**11. Dev Workflow.** `php artisan ai:gateway-deploy --tenant=... --mode=byo_gpu --model=Qwen2.5-72B` runs the Terraform + Helm. `php artisan ai:gateway-health --tenant=...` reports the last health check. `php artisan ai:model-eval --model=Qwen2.5-72B` runs the safety eval suite. Code: `app-modules/on-prem-ai-gateway/` (Helm chart, Terraform module, gateway Python service); tests: `app-modules/on-prem-ai-gateway/tests/Integration/{Mtls,ModelVerify,PhiLogging}Test.php` and `tests/Performance/gateway-latency.k6.js`.

### 15.3 Telemedicine (V2)

| Provider | Self-host? | Use case | Auth | Recording | TURN |
|---|---|---|---|---|---|
| **LiveKit Cloud** | No | Primary for V2 | API key + secret | Built-in S3 | Built-in |
| **Daily.co** | No | Fallback for V2.1 | API key | Built-in | Built-in |
| **Jitsi Meet** | Yes (Docker) | Self-hosted fallback | JWT | Jibri + S3 | Coturn |
| **mediasoup** | Yes (Node) | V2.1 fully self-hosted | Custom | Custom | Coturn |

**Primary: LiveKit Cloud.** Reasons:
- WebRTC SDK in Flutter, React, iOS, Android, Web.
- Adaptive bitrate, simulcast, SVC.
- Built-in TURN.
- Recording to S3 out of the box.
- 99.95% SLA, $0.004/participant-minute.

**Coturn (self-hosted for mediasoup/Jitsi fallback):** runs on UDP/TCP :3478, TURN+STUN, HMAC-auth credentials, deploy 1 per region.

### 15.4 WhatsApp Business Cloud API (V1.1)

- **Endpoint:** `graph.facebook.com/v18.0/{phone_number_id}/messages`
- **Auth:** System user access token (long-lived).
- **Templates:** must be pre-approved by Meta. Admin uploads via WhatsApp Manager; we poll approval status.
- **24h customer service window:** enforced server-side.
- **Rate limits:** 80 messages/second per phone number, 1k/second per WABA.
- **Costs:** utility template $0.004, marketing $0.025, auth $0.0035.

**Webhook:** `https://api.medicare.app/webhooks/whatsapp` (POST, signed).

### 15.5 Twilio (V1.2)

- **Auth:** Account SID + Auth Token.
- **Verify API:** for OTP.
- **Programmable Messaging:** for SMS.
- **Programmable Voice:** for V2.1 voice calls.
- **Network Traversal:** for V2.1 TURN.
- **Cost:** SMS $0.0079 (US), $0.04 (India); Voice $0.013/min; Verify $0.05/verification.
- **Webhook:** signed with `X-Twilio-Signature`.

### 15.6 Maps & geo (V1.1)

| Provider | Use | Auth | Cost |
|---|---|---|---|
| **Mapbox** | Map tiles, geocoding | API key | $5/1k map loads |
| **Google Maps** | Backup | API key | $7/1k map loads |
| **OpenStreetMap (Nominatim)** | Self-hosted, free | None | Free, with rate limits |
| **PostGIS** | DB-level geo queries | Built-in | Free |

**Per-tenant choice:** tenant can pick Mapbox or Google in settings. Default: Mapbox (cheaper, better privacy).

### 15.7 ABDM (India national health ID) (V1.2)

- **Sandbox:** `dev.abdm.gov.in`
- **Production:** NHA gateway, requires M1/M2/M3 certification.
- **APIs:** ABHA creation, consent management, health record fetch (HIP/HIU), e-prescription upload.
- **Auth:** OAuth 2.0 client credentials with public key pinning.
- **Rate limits:** 100 RPM per client.
- **Cost:** free (government).

**HIP/HIU registration:** we register as a Health Information User (HIU) to fetch records and as a Health Information Provider (HIP) to push records.

### 15.8 Insurance payer integrations (V2)

| Region | Payer | Standard | Adapter |
|---|---|---|---|
| US | Change Healthcare | EDI 837/835 | Custom Java adapter |
| US | Availity | X12 270/271, 837 | Custom |
| US | Waystar | API + EDI | Custom |
| US | Edifecs | EDI | Custom |
| India | Vipul Mediclaim | API + PDF | Custom |
| India | MD India | API | Custom |
| India | Medi Assist | API | Custom |
| India | FHIR (NHCX) | FHIR R4 | Custom |
| EU | Various | Various | Generic HL7 |

**Each adapter is a separate class** in `app/Services/Insurance/{Payer}Adapter.php`. The `InsuranceService` dispatches to the right adapter based on `insurance_payers.payer_code`.

### 15.9 DICOM Viewer (`dicom-viewer`, V3)

We do not own a PACS (per `14-V2-Stubs §1.2` non-goal). We embed a viewer and integrate with a PACS partner for storage.

**1. Components:**
- **OHIF Viewer** (open source, MIT, embeddable as a React app) hosted at `/admin/dicom/{study_id}` in the Next.js admin web. Uses the Cornerstone.js rendering library.
- **DICOMweb server:** **Orthanc** (self-hosted) for the V3 GA path. Optional **AWS HealthImaging** adapter (V3.1) for tenants that already have a HealthImaging dataset.
- **Storage:** S3 with object versioning enabled (immutability required for DICOM). Bucket policy: `s3:PutObject` only via a designated IAM role; bucket has lifecycle rule to Glacier after 90 days, expire after 7 years (per HIPAA).
- **Per 14-V2-Stubs §10 Q4 (DICOM storage decision):** default = partner. Tenants can choose: (a) self-hosted Orthanc in their own VPC, (b) AWS HealthImaging, (c) existing PACS accessed via WADO-RS.

**2. Imaging study lifecycle:**
1. Doctor orders an imaging study (X-ray, CT, MRI, US) — creates a row in `lab_orders` (or new `imaging_orders`) with `modality`, `body_part`, `clinical_indication`.
2. Patient goes to the imaging center. The center pushes DICOM instances to our WADO-RS endpoint (`POST /wado-rs/studies`) via Orthanc's REST API.
3. Orthanc stores the DICOM, indexes it, and notifies our API via webhook (`POST /api/internal/dicom/study-received`).
4. The API creates a `dicom_studies` row and links it to the `imaging_orders` row.
5. Doctor (or radiologist) opens the study in OHIF — `GET /admin/dicom/{study_id}` proxies the OHIF viewer with our study URL.
6. Doctor's report is saved as a `prescriptions`-style note on the `imaging_orders` row (V3.1: full DICOM-SR structured report).

**3. DICOMweb endpoints exposed:**
```
GET    /wado-rs/studies                              # search
GET    /wado-rs/studies/{study_uid}                  # retrieve metadata
GET    /wado-rs/studies/{study_uid}/metadata         # DICOM JSON metadata
GET    /wado-rs/studies/{study_uid}/series            # list series
GET    /wado-rs/studies/{study_uid}/thumbnail         # preview
POST   /wado-rs/studies                              # STOW-RS upload (Orthanc)
DELETE /wado-rs/studies/{study_uid}                  # soft-delete (audit row, no hard delete per HIPAA)
QIDO-RS: /qido-rs/studies?PatientID={mrn}&Modality=CT&StudyDate=20240101-20241231
```

**4. OHIF authentication:** the viewer does OAuth 2.0 against our API. The user signs in to admin, then `GET /admin/dicom/{study_id}` issues a short-lived (5 min) OHIF session token scoped to that study. The token is a signed JWT in an httpOnly cookie. The viewer then makes WADO-RS calls with that token; Orthanc validates via a Lua script on every request.

**5. Endpoints (admin / internal):**
```
GET    /api/v3/dicom/studies                         # paginated list, filter by patient_id, modality, date
GET    /api/v3/dicom/studies/{id}                    # single study
GET    /api/v3/dicom/studies/{id}/metadata           # DICOM JSON
GET    /api/v3/dicom/studies/{id}/thumbnail          # 256x256 JPEG
POST   /api/v3/dicom/studies/{id}/report             # doctor report (free text + measurements)
GET    /api/v3/dicom/studies/{id}/report             # fetch report
POST   /api/internal/dicom/study-received            # Orthanc webhook (mTLS, service token)
GET    /api/v3/dicom/orthanc/health                  # Orthanc connection check
```

**6. Schema additions (per 15-DB-Schema):** `dicom_studies` (one row per study, FHIR `ImagingStudy` projection), `dicom_series` (one row per series, FHIR `ImagingSeries` projection), `dicom_instances` (one row per instance, FHIR `ImagingInstance` projection — may be very large, partition by tenant + month), `dicom_reports` (doctor reports).

**7. Realtime events:** `dicom.study.received.{tenant}` (private, broadcast when Orthanc webhook fires).

**8. Storage strategy:** V3 GA uses Orthanc's default filesystem + S3. AWS HealthImaging adapter is V3.1. Self-hosted Orthanc in tenant VPC is the Enterprise path — the tenant provides the Orthanc URL, we connect via their private peering.

**9. IHE profile compliance:** V3 GA targets **IHE RAD-1 (Scheduled Workflow)** for study ordering and **IHE RAD-3 (PWF)** for presentation. Full IHE RAD (XDS-I.b) for cross-enterprise image sharing is a V4 milestone for `#106 HIS Bridge`.

**10. Failure handling:** if Orthanc is unreachable, the imaging order is still created; status = `ordered_pacs_unavailable`. A Slack alert fires after 5 min. The order is retried by `RetryDicomUploadJob` (every 5 min, max 10 retries). After 10 failures, status = `pacs_upload_failed` and the doctor is notified via in-app notification.

### 15.10 HL7 v2 ADT (`hl7-v2-adt`, V3)

HL7 v2 is the lingua franca of hospital information systems. We accept inbound ADT and ORU messages from a tenant's existing HIS / EHR / LIS and project them into our domain.

**1. Components:**
- **Mirth Connect** (open source, self-hosted in `medicare-realtime` sidecar) for HL7 v2 parsing, validation, and routing. Runs as a sidecar pod (per 21-Deployment §18.1).
- **HAPI-FHIR** (the `medicare-fhir-gateway` sidecar) as the FHIR R4 server, populated by Mirth's HL7 v2 → FHIR transforms.
- **MLLP listener** on port `6661` (TLS) and `6662` (plain, dev only) — not exposed to the public internet, reachable only via the tenant's VPN or private peering.

**2. Supported message types (V3 GA):**
- **ADT^A01** — Admit / Inpatient. Creates a `patient_admissions` row and a `bed_assignment` (links to #102 Bed/Ward).
- **ADT^A03** — Discharge. Closes the `patient_admissions` row.
- **ADT^A04** — Register / Outpatient. Creates or matches a `patients` row by `MRN` + `tenant_id`.
- **ADT^A08** — Update patient demographics. Updates `patients` row, writes to `audit_logs` with `action = 'hl7.patient.updated'`.
- **ORU^R01** — Lab result. Creates `lab_results` row (links to #78 Pathology/Lab).

**3. Inbound endpoint (admin-configured per tenant):**
```
mllp://mirth.medicare-{tenant}.internal:6661
```
Tenant admin registers their HIS to send MLLP to this address (over the tenant's private network). On receipt, Mirth:
1. Validates the message against the HL7 v2.5 schema.
2. Resolves `tenant_id` from the sending facility code (`MSH.4`).
3. Calls `POST /api/internal/hl7v2/receive` on the API with the parsed JSON.
4. The API applies the same business logic as a normal API call (RBAC via `service_hl7v2` token, tenant scope, audit log).

**4. Outbound (V3.1, stub):** if a tenant enables `feature_hl7v2_outbound`, our appointment / admission events are formatted to ADT and pushed to the tenant's Mirth outbound channel.

**5. Configuration UI (admin):**
- `Settings → Integrations → HL7 v2`: enable, set `sending_facility_code`, map `MSH.3` sender to tenant, paste the partner's certificate fingerprint for MLLP mutual TLS.

**6. Audit per IHE ATNA** (per 20-SECURITY §23.4 V2 audit additions): every ADT and ORU message generates an `audit_logs` row with `action = 'hl7v2.message.received'`, `entity_type = 'hl7v2_message'`, capturing `message_type`, `control_id`, `sending_facility`, `event_outcome` (`success | processing_error | reject`). Required for SOC 2 and HIPAA.

**7. Failure handling:**
- Malformed message → Mirth quarantines to `hl7v2_quarantine` table with raw payload, error reason, original `MSH.10` (control id). Admin UI lists quarantined messages; admin can replay after fix or reject.
- Unmatched `sending_facility` → 403, message dropped, `audit_logs.action = 'hl7v2.unknown_sender'`. Triggers a Slack alert (V3.1: PagerDuty if > 10 in 5 min).
- Successful processing → `hl7v2_messages` row (for replay), `audit_logs` row, downstream `audit_logs` rows for the affected entity (e.g., `patient.created`).

**8. Realtime event:** on successful processing, broadcast on `hl7v2.message.received.{tenant}` (private channel, subscribed by admin web).

**9. Schema additions (per 15-DB-Schema):** two new tables in V3: `hl7v2_messages` (every received message, payload, control_id, message_type, processing_status, retry_count, last_error) and `hl7v2_quarantine` (messages rejected before processing).

### 15.10a FHIR R4 Write (`fhir-r4-write`, V3)

The HAPI-FHIR sidecar introduced in §15.10 is read-only at V2 (`GET /api/v2/fhir/*`). V3 adds the write surface so external EHR systems can push Patient, Observation, Encounter, and other resources into Medicare.

**1. Write surface (V3 GA):**
- `Patient` — create / update.
- `Encounter` — create / update / close (links to `appointments` row or `patient_admissions` row).
- `Observation` — create (links to `patient_vitals`, `lab_results`).
- `MedicationRequest` — create (links to `prescriptions`).
- `AllergyIntolerance` — create (links to `patient_allergies`).
- `Condition` — create (links to `patient_problem_list`).
- `Immunization` — create (links to `patient_immunizations`).

**2. Authentication:** SMART-on-FHIR (OAuth 2.0 + OpenID Connect). External systems register a SMART app via `POST /api/v3/fhir/smart-apps`; receive `client_id` and `client_secret`. Token endpoint at `POST /oauth2/token` returns access token (1 hour) + refresh token (24 hour). SMART launch contexts: standalone (client_credentials) and EHR-launch (authorization_code with PKCE). Required scopes per resource type: `patient/Patient.read`, `patient/Patient.write`, `patient/Observation.write`, etc.

**3. Endpoints (V3 GA):**
```
POST   /api/v3/fhir/Patient                       # create
PUT    /api/v3/fhir/Patient/{id}                  # update
DELETE /api/v3/fhir/Patient/{id}                  # soft-delete (audit row, no hard delete)
POST   /api/v3/fhir/Encounter                     # create
PUT    /api/v3/fhir/Encounter/{id}                # update
POST   /api/v3/fhir/Observation                   # create
POST   /api/v3/fhir/MedicationRequest             # create
POST   /api/v3/fhir/AllergyIntolerance            # create
POST   /api/v3/fhir/Condition                     # create
POST   /api/v3/fhir/Immunization                  # create
POST   /api/v3/fhir/{resourceType}                # generic create (for resource types added after V3 GA)
PUT    /api/v3/fhir/{resourceType}/{id}           # generic update
POST   /api/v3/fhir/smart-apps                    # register SMART app (admin)
DELETE /api/v3/fhir/smart-apps/{id}               # revoke
GET    /api/v3/fhir/smart-apps                    # list registered apps
```

**4. Write semantics (per 14-V2-Stubs §10 Q9):** **dual-write with conflict resolution**.
- The HAPI-FHIR sidecar writes to its own `fhir_resources` row first (the FHIR-native projection).
- A Laravel job `FhirResourceProjectJob` then projects the FHIR resource into our domain tables (`patients`, `observations`, `prescriptions`, etc.).
- If both sides succeed, the resource is `status = 'synced'`.
- If the projection job fails, the FHIR row is still saved, but `status = 'pending_projection'`. The job retries 3× with exponential backoff. After 3 failures, `status = 'projection_failed'`, an `audit_logs` row is written, and a Slack alert is fired.
- If our domain row already has a newer `updated_at` than the FHIR write, we apply **last-writer-wins** and emit a `fhir.conflict.lww` audit event with both timestamps and the chosen winner. The losing payload is stored in `fhir_resources.conflict_log` for 30 days for manual review.
- Versioning via FHIR `meta.versionId` is honored: a write with `If-Match: W/"3"` and current version 4 returns **HTTP 412 Precondition Failed**.

**5. Idempotency:** every write accepts an `Idempotency-Key` header (per FHIR spec). The key is hashed and stored in `fhir_write_log.idempotency_key_hash` with a 24-hour TTL. A duplicate key returns the original response.

**6. Audit:** every write generates an `audit_logs` row with `action = 'fhir.write'`, `entity_type` = FHIR resource type, `entity_id` = FHIR id, `metadata` JSON with `client_id`, `version_id`, `projection_status`, `conflict_resolution` (if any).

**7. Realtime:** on successful projection, broadcast on `fhir.write.{tenant}.{resource_type}` (private channel). Subscribed by the relevant admin module (e.g., `fhir.write.{tenant}.Patient` → admin patient list view).

**8. Rate limits per SMART app:** 1,000 writes/minute for the `patient/*` scope, 10,000 writes/minute for the `system/*` scope. Exceeded → HTTP 429 with `Retry-After`.

**9. Schema additions (per 15-DB-Schema):** extend `fhir_resources` (V2 stub) with `projection_status` enum, `idempotency_key_hash`, `conflict_log` JSON. New table `fhir_write_log` (every write attempt, success/failure, latency, payload size). New table `smart_apps` (registered SMART apps, client_id, hashed client_secret, scopes, status).

### 15.10b White-Label Mobile (`white-label-mobile`, V3)

A tenant can publish their own branded iOS and Android apps to the App Store and Play Store. The apps share the same `medicare-user-app` codebase (per `00-Architecture §4`) with per-tenant configuration applied at build time.

**1. Conflict note:** `00-Architecture §4` lists `medicare-user-app` as **Flutter 3.24+**. The V3 catalog row in `14-MODULE-V2-STUBS §3.4` says **EAS (Expo Application Services)**. EAS supports Flutter builds as of 2023, so the two are compatible; the V3 GA plan is to use EAS for the build pipeline (managed iOS/Android signing, store submission, OTA updates) while keeping the Flutter codebase. The two reference points are not contradictory.

**2. Per-tenant configuration:** a `tenant_mobile_config` row in the API (see `15-DB-Schema` below) carries:
- `app_name` (e.g., "Acme Health" — shown on home screen).
- `bundle_id` (iOS) and `application_id` (Android) — must be unique per tenant on each store.
- `icon_url` (PNG, 1024×1024 base + Android adaptive layer).
- `splash_image_url`.
- `primary_color`, `secondary_color` (hex) — drives the app's theme.
- `api_base_url` (the tenant's API endpoint; defaults to the tenant's custom domain if set, per V2 #84 Custom Domain).
- `support_email`, `support_url`, `privacy_url`.
- `app_store_id` (Apple) and `play_store_package` (set after first publish).

**3. Build pipeline:**
- The Flutter repo (`medicare-user-app`) has a single `app.config.ts` (or `app.config.json` for Flutter) that reads the tenant config at build time via `eas build --env TENANT_ID=...`.
- EAS Build runs on EAS's hosted infrastructure. We do not run our own iOS/Android build farm.
- Per-tenant signing: the tenant provides their own Apple Developer Team and Google Play Console access; we do not co-sign. For tenants without their own Apple/Google accounts, we use a shared signing identity (a paid "MFi/Play reseller" arrangement, V3.1).
- Build artifacts: `.ipa` (iOS) and `.aab` (Android App Bundle). EAS Submit uploads to the stores with the tenant's credentials.

**4. Update mechanism:**
- **OTA (Over-The-Air) updates** for non-native code: EAS Update allows JavaScript/asset updates without an app store release. Native code changes (Flutter plugin updates, native modules) still require a store release.
- We do **not** allow OTA updates that change the API base URL or the bundle ID — these would be security risks.

**5. Admin UI (V3):**
- `Settings → Mobile App → Branded App`: form for the 9 fields above, with live preview (icon + splash on a phone frame).
- "Build" button triggers an EAS build. Build status is streamed via `mobile.build.{tenant}.{build_id}` channel (added in §17 Realtime).
- Build history is at `GET /api/v3/mobile/builds` with download links for `.ipa` and `.aab`.

**6. API surface (V3):**
```
GET    /api/v3/mobile/config                  # read tenant_mobile_config
PATCH  /api/v3/mobile/config                  # update config (admin only)
POST   /api/v3/mobile/builds                  # trigger EAS build { platform: 'ios'|'android'|'both' }
GET    /api/v3/mobile/builds                  # paginated build history
GET    /api/v3/mobile/builds/{id}             # single build + status + EAS link
GET    /api/v3/mobile/builds/{id}/download    # signed S3 URL for .ipa or .aab (10 min TTL)
POST   /api/v3/mobile/builds/{id}/submit      # submit to App Store / Play Store
POST   /api/v3/mobile/builds/{id}/cancel      # cancel a running build
GET    /api/v3/mobile/ota-channels            # list EAS Update channels (production, staging, etc.)
POST   /api/v3/mobile/ota-channels/{channel}/promote  # promote a build to a channel
```

**7. Schema additions (per 15-DB-Schema):** `tenant_mobile_config` (one row per tenant, the 9 fields above + 2 timestamps), `tenant_mobile_builds` (every EAS build, status, artifact S3 key, EAS build id, submitted_to_store bool, store_url).

**8. Realtime events:**
- `mobile.build.{tenant}.{build_id}` — private channel, broadcasts on `started`, `progress` (every 10%), `completed`, `failed`. Subscribed by the admin "Mobile App" page.

**9. Pricing / cost:** EAS Build is $99/month per Apple developer team (waived for Pro and Enterprise tenants, included in tier per `01-MODULES-INDEX §17.5`). Free tier tenants pay for EAS directly and we provide a setup wizard.

**10. What this module is NOT:**
- Not a native iOS/Android development kit. The codebase is one Flutter app; per-tenant customization is build-time only.
- Not a code-signing service. The tenant owns the signing identity (or uses our shared one).
- Not a store-publisher intermediary. We submit to the stores but the tenant owns the listing (description, screenshots, reviews).
- Not a mobile BaaS. No push, no analytics, no crash reporting outside what EAS + Sentry already provide.

### 15.10c LIS ASTM E1394 (`lis-astm`, V4)

V3 has HL7 v2 ADT/ORU (per §15.10) for partner HIS connections. V4 adds the **bidirectional ASTM E1394/E1394-97 protocol** to talk directly to lab analyzers (hematology, chemistry, immunoassay, microbiology) without going through a partner middleware.

**1. Components:**
- `medicare-lab-gateway` sidecar (per `00-Architecture §18.1`), written in **Java/Spring** (not PHP, per the architecture decision in `14-V2-STUBS §4.1` because the only mature ASTM library is `clintrials/LIS2A2` in Java). It speaks the ASTM framing protocol.
- ASTM E1394 (serial framing, RS-232) and ASTM LIS2-A2 (TCP/IP framing) are both supported. The gateway terminates the wire protocol, parses records, and posts JSON to our internal API.
- The gateway runs as a sidecar pod per tenant (not per analyzer) and handles N concurrent analyzer connections.

**2. Supported record types (V4 GA):**
- `O` (Order) record — outbound (Medicare → analyzer), for orders created in `lab_orders` (§78 Pathology).
- `R` (Result) record — inbound (analyzer → Medicare), populates `lab_results`.
- `C` (Comment) record — both directions.
- `P` (Patient) record — for analyzers that maintain a patient roster.
- `Q` (Query) record — Medicare asks the analyzer "do you have result for order X?" (V4.1).

**3. Bidirectional flow:**
1. Lab tech creates an order in Medicare: `POST /api/v2/lab/orders` (V2 Pathology, per `08-Prescriptions`).
2. A `PushOrderToAnalyzerJob` dispatches the order to the gateway. The gateway opens (or reuses) the analyzer connection, sends the `O` record with patient + order details.
3. Analyzer acknowledges (`ACK` record) and runs the test.
4. When the analyzer produces a result, it pushes a `R` record. The gateway parses it, calls `POST /api/internal/lis/result` on the API, which creates a `lab_results` row.
5. If the analyzer is offline, the order is queued. A retry job (every 30s) attempts re-delivery for up to 1 hour.

**4. Analyzer connection lifecycle:**
- TCP connection (LIS2-A2): keep-alive, ping every 60s. If ping fails 3 times, close and reopen.
- Serial (E1394 RS-232): the gateway is given the device path (e.g., `/dev/ttyUSB0`). It opens the port at 9600/19200 baud, 8N1. Flow control is RTS/CTS or XON/XOFF depending on analyzer.
- Each connection is logged in `lis_connections` table with `started_at`, `ended_at`, `bytes_sent`, `bytes_received`, `last_error`.

**5. API surface (V4):**
```
GET    /api/v4/lis/analyzers                          # list analyzers in this tenant
POST   /api/v4/lis/analyzers                          # register analyzer
PATCH  /api/v4/lis/analyzers/{id}                     # update config (host, port, protocol)
DELETE /api/v4/lis/analyzers/{id}                     # soft-delete
GET    /api/v4/lis/analyzers/{id}/health              # current connection state + last error
POST   /api/v4/lis/analyzers/{id}/test                # send a test query
GET    /api/v4/lis/connections                         # connection history
GET    /api/v4/lis/orders/{id}/push-status            # did the order reach the analyzer?
POST   /api/v4/lis/orders/{id}/resend                 # manually resend a stuck order
GET    /api/v4/lis/results                            # list results received from analyzers
```

**6. Schema additions (per 15-DB-Schema):** `lis_analyzers` (one row per analyzer, host, port, protocol enum, manufacturer, model, last_connected_at, last_error), `lis_connections` (per-connection audit), `lis_outbound_queue` (orders waiting to be pushed, with retry_count and next_retry_at).

**7. Realtime events:**
- `lis.analyzer.connected.{tenant}` and `lis.analyzer.disconnected.{tenant}`.
- `lis.order.pushed.{tenant}` and `lis.result.received.{tenant}`.
- `lis.error.{tenant}` — broadcast on any analyzer error (parse, timeout, protocol).

**8. Security:**
- The gateway sidecar does **not** have direct DB access. It only calls our internal API (mTLS, service token).
- All analyzer communication is on the tenant's private network (VPN or private peering, same model as HL7 v2 §15.10).
- The `medicare-lab-gateway` runs as the `lis-gateway` Kubernetes ServiceAccount, with NetworkPolicy restricting egress to only the API and the analyzer IPs.

**9. Acceptance criteria (V4 GA):**
- [ ] An order created in Medicare is pushed to a connected analyzer within 30s.
- [ ] A result pushed by the analyzer appears in `lab_results` within 5s.
- [ ] If the analyzer is offline, the order is queued and retried automatically; manual resend works.
- [ ] Connection state (`connected`, `disconnected`, `error`) is visible in admin UI in real time.
- [ ] No analyzer traffic is exposed to the public internet.

**10. What this module is NOT:**
- Not a universal LIS. We support ASTM E1394/E1394-97; analyzers speaking only HL7 v2 ORU use the §15.10 path instead.
- Not a middleware replacement. If a tenant already has a middleware (e.g., Data Innovations Instrument Manager), we connect via the middleware's HL7 v2 output rather than directly to the analyzer.

### 15.10d HIS Bridge — Epic, Cerner, Meditech (`his-bridge`, V4)

V3 has HL7 v2 ADT/ORU for ad-hoc partner HIS connections. V3 also has FHIR R4 Write for SMART-on-FHIR apps (§15.10a). V4 promotes Medicare from "system that can receive HIS data" to "system that can act as a peer HIS" — that is, we register as a participant in the tenant's existing IHE Affinity Domain and exchange documents, images, and identity data bidirectionally with Epic, Cerner, or Meditech.

**1. Why this is its own module (not just a configuration of §15.10):**
- Vendor-specific onboarding: Epic requires App Orchard/Showroom registration, Cerner requires Open Developer Experience, Meditech requires NExT API access. Each vendor has a different sandbox-vs-prod gate, conformance review, and certification process.
- XDS Affinity Domain registration requires a registry+repository role; we run a `medicare-xds-registry` and `medicare-xds-repository` (per `00-Architecture §18.1` sidecar list).
- CCDA generation/consumption requires a separate library (MDHT) and conformance to HL7 v3 C-CDA 2.1.
- PIX/PDQ for cross-enterprise patient identity requires running an MPI (Master Person Index) in addition to the local PID.

**2. Components:**
- `medicare-his-bridge` sidecar (Java/Spring) per tenant, talks HL7 v2 + FHIR R4 + IHE XDS.b to the partner HIS.
- `medicare-xds-registry` PostgreSQL-backed registry, registered with the tenant's Affinity Domain.
- `medicare-xds-repository` object store (S3-compatible), holds C-CDA documents, KOS manifests, and imaging manifest objects.
- `medicare-pix-pdq` HL7 v3-based PIX Manager + PDQ Supplier.

**3. Patient identity (PIX/PDQ) flow (V4 GA):**
1. Patient presents at Medicare. Front desk types MRN or DOB; we send a `QBP^Q23` (PIX Query) and `QBP^Q22` (PDQ Query) to the partner HIS.
2. Partner returns the canonical enterprise ID (e.g., Epic `EPI`).
3. We store the cross-reference in `pix_domain` (`local_mrn`, `enterprise_id`, `assigning_authority`, `source_his`).
4. When the partner sends an `ADT^A40` (Merge), we update our `pix_domain` row, and trigger a fan-out to update our `patients` table's identifiers list.

**4. Document exchange (XDS.b + C-CDA) flow (V4 GA):**
1. After a visit, we generate a C-CDA R2.1 document (using MDHT) with sections: History, Medications, Allergies, Problems, Vitals, Results, Plan of Care.
2. We wrap it in an XDS `ProvideAndRegisterDocumentSet-b` request, sign it with the tenant's S/MIME cert, and POST to the partner's XDS Repository.
3. The partner's XDS Registry indexes it. Cross-enterprise clinicians can now query for and retrieve it via `RetrieveDocumentSet` calls.
4. Conversely, when a C-CDA arrives via `RetrieveDocumentSet`, we parse it, dedupe by `documentOid`+`version`, and attach to the matching patient.

**5. Imaging exchange (XDS-I.b) flow (V4 GA):**
- Per the V4 milestone mentioned in §15.9 line 948: we publish a **KOS (Key Object Selection) manifest** to the XDS Registry pointing at the original imaging study, and store the actual DICOM objects in our repository (or in the partner's, whichever is the affinity domain's repository). This lets cross-enterprise radiologists pull the study without us needing to become a DICOM Q/R provider per partner.
- The DICOM Viewer (§15.9) gains a "fetch from partner" mode that issues a `RetrieveImagingDocumentSet` XDS-I.b call.

**6. Vendor-specific connectors (V4 GA):**
- **Epic:** InterConnect (R4), Bulk Data Access ($export), App Orchard (sandbox) → Showroom (prod). 3-year EHR data export.
- **Cerner:** Open Developer Experience (CODE), Cerner Ignite APIs, Bulk Data Access, DSTU2/R4. HealtheIntent population export for §111 Population Health.
- **Meditech:** NExT API (R4), NExT Export (FHIR Bulk Data), MAGIC/C/S/NExT series support. App Orchard-equivalent: Meditech Greenfield.

**7. API surface (V4):**
```
POST   /api/v4/his/{vendor}/connect               # OAuth2 + sandbox validation
GET    /api/v4/his/{vendor}/connection            # current state
POST   /api/v4/his/{vendor}/sync/pix              # manual PIX resync
POST   /api/v4/his/{vendor}/sync/ccda             # manual C-CDA resend
GET    /api/v4/his/{vendor}/xds/affinity-domain   # registered AD info
POST   /api/v4/his/{vendor}/xds/retrieve          # pull a document by OID
GET    /api/v4/his/audit                          # IHE ATNA audit log (§23.2.5)
```

**8. Schema additions (per 15-DB-Schema):** `his_vendor_connections` (vendor, oauth2_state, last_sync_at, sandbox_or_prod), `pix_domain` (cross-ref table), `xds_documents` (OID, repository_unique_id, mime_type, size, hash, availability_status, patient_id, signing_cert_sha256), `xds_folders` and `xds_submission_sets` (XDS metadata).

**9. Realtime events:**
- `his.patient.merged.{tenant}` — PIX merge received.
- `his.document.registered.{tenant}` and `his.document.retrieved.{tenant}`.
- `his.connection.{connected|disconnected|error}.{vendor}.{tenant}`.

**10. Security:**
- mTLS to the partner HIS on the partner's preferred port (typically 6661 for HL7 v2, 9443 for FHIR).
- Document signing uses the tenant's S/MIME cert (issued by their CA, stored in our KMS).
- IHE ATNA audit logging per `20-Security §23.2.5`: every PIX/PDQ/XDS transaction writes an `audit` row with `action = 'his.{vendor}.{operation}'`.
- PHI minimization: we only request the specific document OIDs the clinician needs, not bulk pull.

**11. Acceptance criteria (V4 GA):**
- [ ] A patient registered in Epic is searchable by MRN in Medicare (PIX) within 2s.
- [ ] A C-CDA generated in Medicare is retrievable in Epic, Cerner, and Meditech within 60s.
- [ ] A KOS manifest from a Medicare study is retrievable by a partner radiologist.
- [ ] An `ADT^A40` merge received from Epic updates the local patient identifiers.
- [ ] XDS Affinity Domain registration is verified quarterly with the partner's IHE test bed (e.g., IHE North America Connectathon).

**12. What this module is NOT:**
- Not a generic EHR replacement. The partner HIS remains the system of record for the tenant; we exchange data, not replace.
- Not certified for IHE PCD (Patient Care Devices), XCA (cross-Community Access), or XDS-SD (Scanned Documents) in V4 GA. Those are V5+ roadmap items.
- Not free. Each vendor connection has a one-time integration fee + annual support fee, because the vendor's certification and conformance review is real work.

### 15.11 Local payment methods (V1.2)

| Method | Region | Gateway | Flow |
|---|---|---|---|
| UPI Intent | India | Razorpay | Opens UPI app, returns |
| UPI Collect | India | Razorpay | Sends collect request to VPA |
| UPI Autopay (eNACH) | India | Razorpay | Mandate for recurring |
| Paytm Wallet | India | Razorpay | Wallet debit |
| Net Banking (60+ banks) | India | Razorpay | Redirect to bank |
| EMI (credit card) | India | Razorpay | 3/6/9/12 month EMI |
| GCash | Philippines | PayMongo | E-wallet |
| PayMaya | Philippines | PayMongo | Card/e-wallet |
| Pix | Brazil | Ebanx / Stripe | Instant transfer |
| OXXO | Mexico | Stripe | Cash voucher |
| M-Pesa | Kenya | DPO / Flutterwave | USSD push |
| Bank Transfer | Nigeria | Paystack | Manual verify |

### 15.12 BNPL providers (V2)

| Provider | Region | Auth | Cost |
|---|---|---|---|
| LazyPay | India | OAuth | 2-3% per txn |
| Simpl | India | OAuth | 2-3% |
| ZestMoney | India | OAuth | 3-5% |
| Klarna | EU/US | API key | 2-4% |
| Afterpay | US/AU | API key | 4-6% |
| Affirm | US | API key | 0-8% |
| Mobikwik | India | API key | 2% |

### 15.13 Observability (V2)

| Tool | Purpose | Cost |
|---|---|---|
| Sentry | Error tracking | $26/mo (Team) |
| Datadog | Metrics + APM + logs | $0.10/host/day |
| Honeycomb | Tracing | $0.001/event |
| OpenTelemetry Collector | Pipeline | Free (self-hosted) |
| BetterUptime | Status page + synthetics | $20/mo |
| PagerDuty | On-call | $21/user/mo |
| Opsgenie | On-call (alt) | $9/user/mo |
| Loki | Logs (self-hosted) | Free |
| Grafana | Dashboards | Free (self-hosted) |

**V1:** BetterUptime + Sentry. **V2:** adds Datadog (or self-hosted Prometheus+Grafana+Loki+Tempo) + PagerDuty.

### 15.14 Search (V2)

| Tool | Use | Cost |
|---|---|---|
| Meilisearch | Primary search (doctors, patients, prescriptions) | $30/mo (Cloud) or self-hosted |
| Typesense | Fallback | $0.50/1k queries |
| pg_trgm | DB-level fuzzy | Free |
| OpenSearch | Self-hosted alt | Free (ops) |

### 15.15 Email (V2)

V1: SES. V2 adds:
- **Mailgun** for transactional (better deliverability).
- **Postmark** for marketing.
- **Resend** (developer-friendly, React Email templates).

V2 template engine: **React Email** for HTML emails (rendered server-side to MJML → HTML).

### 15.16 Storage (V2)

V1: Hetzner Storage Box or S3. V2:
- **AWS S3** (primary).
- **Google Cloud Storage** (for GCP tenants).
- **Azure Blob Storage** (for Azure tenants).
- **MinIO** (self-hosted, S3-compatible).

V2 adds S3 Object Lock for legal hold (immutable for X years), used for `audit_logs` archive.

### 15.17 Secrets management (V2)

V1: `.env` files. V2:
- **AWS Secrets Manager** (auto-rotation, audit).
- **HashiCorp Vault** (self-hosted, dynamic secrets).
- **Doppler** (developer-friendly, sync to env).

### 15.18 V2 credentials matrix (additions)

```bash
# AI
OPENAI_API_KEY=sk-...
ANTHROPIC_API_KEY=sk-ant-...
GOOGLE_AI_KEY=...
MISTRAL_API_KEY=...

# Telemedicine
LIVEKIT_API_KEY=API...
LIVEKIT_API_SECRET=...
DAILY_API_KEY=...

# WhatsApp
WHATSAPP_PHONE_NUMBER_ID=...
WHATSAPP_BUSINESS_ACCOUNT_ID=...
WHATSAPP_SYSTEM_USER_TOKEN=...
WHATSAPP_WEBHOOK_VERIFY_TOKEN=...

# Twilio
TWILIO_ACCOUNT_SID=AC...
TWILIO_AUTH_TOKEN=...
TWILIO_VERIFY_SERVICE_SID=VA...
TWILIO_MESSAGING_SERVICE_SID=MG...

# Maps
MAPBOX_ACCESS_TOKEN=pk....
GOOGLE_MAPS_API_KEY=AIza...

# ABDM
ABDM_CLIENT_ID=...
ABDM_CLIENT_SECRET=...
ABDM_PUBLIC_KEY_PATH=/path/to/abdm.pub
ABDM_PRIVATE_KEY_PATH=/path/to/abdm.key
ABDM_ENV=sandbox|production

# Insurance
INSURANCE_CHANGE_HC_API_KEY=...
INSURANCE_VIPUL_API_KEY=...

# DICOM
ORTHANC_URL=...
ORTHANC_USER=...
ORTHANC_PASSWORD=...

# BNPL
KLARNA_API_KEY=...
AFFIRM_API_KEY=...
LAZYPAY_API_KEY=...

# Observability
DATADOG_API_KEY=...
DATADOG_APP_KEY=...
PAGERDUTY_INTEGRATION_KEY=...
HONEYCOMB_API_KEY=...

# Search
MEILISEARCH_HOST=...
MEILISEARCH_KEY=...

# Email
MAILGUN_DOMAIN=...
MAILGUN_API_KEY=...
POSTMARK_SERVER_TOKEN=...
RESEND_API_KEY=...

# Storage (multi-cloud)
S3_PRIMARY_BUCKET=...
GCS_BUCKET=...
AZURE_BLOB_CONTAINER=...

# Secrets
VAULT_ADDR=...
VAULT_TOKEN=...
DOPPLER_TOKEN=...
```

### 15.19 V2 acceptance criteria

- [ ] Every V2 integration has a working staging test.
- [ ] Every V2 integration has a runbook in `runbooks/`.
- [ ] Every V2 integration has a fallback (degraded mode if down).
- [ ] Cost per integration is tracked in the cost dashboard.
- [ ] Webhook signatures are verified.
- [ ] Rate limits are respected (backoff, queue).
