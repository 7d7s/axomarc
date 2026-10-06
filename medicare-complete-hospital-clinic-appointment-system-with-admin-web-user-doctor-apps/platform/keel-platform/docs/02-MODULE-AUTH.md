# 02 — Auth Module (Authentication, Authorization, Users)

> The foundation module. Every other module depends on knowing **who is calling, what they can do, and which tenant they belong to**.

---

## 1. Scope

| In Scope | Out of Scope |
|---|---|
| Firebase Phone OTP for patient/doctor mobile apps | SSO / SAML / OIDC (V2) |
| Email + password for admin/staff web | Magic link login (V2) |
| Sanctum token issuance + refresh | OAuth2 (we don't need it) |
| Password reset (email link + in-app for staff) | 2FA (V2, but infra is here) |
| RBAC via Spatie Permissions | Per-resource ownership rules (use Policies) |
| User CRUD for admins | Self-service signup (admin creates doctors; patients self-register) |
| Login audit log | Anomaly detection (Sentry handles basic) |
| Logout / token revoke | Token rotation (V2) |

---

## 2. Two Auth Realms

Medicare has **two completely separate auth flows** because the security model is different.

### 2.1 Realm A — Mobile Patient & Doctor (Firebase Phone OTP)

- Patient/doctor enters phone → Firebase Auth SDK sends SMS OTP → user enters code → Firebase returns ID token → app sends ID token to `/api/v1/auth/otp/verify` → API **verifies the ID token with Firebase Admin SDK** → issues a **Sanctum API token** (long-lived, 30 days) → app stores in `flutter_secure_storage`.
- The **API never sends an SMS**. It only verifies the ID token. This means SMS rate limits and reCAPTCHA are Firebase's problem, not ours.
- A **backup OTP via Twilio** is plumbed for environments where Firebase phone auth is blocked (e.g., India, China). Selected by `MEDICARE_OTP_PROVIDER` env.

### 2.2 Realm B — Admin Panel & Patient Web (Email + Password)

- Standard Laravel `Auth\LoginController` + `RegisteredUserController` flow.
- Password hashed with `bcrypt` (cost 12) or `argon2id` (preferred).
- Email verification required for staff (Laravel's `MustVerifyEmail`).
- "Remember me" issues a long-lived Sanctum token + refresh cookie.
- Admin panel may enforce 2FA (TOTP) starting V2.

```
┌─────────────────────────────────────────────────────────────┐
│                      MOBILE (Realm A)                       │
│  ┌──────────┐  Firebase OTP   ┌──────────┐  Verify ID tok   │
│  │   App    │────────────────►│ Firebase │─────┐            │
│  │          │◄─── ID token ───│   Auth   │     │            │
│  │          │                                    ▼            │
│  │          │  Sanctum token   ┌──────────────────────┐      │
│  │          │◄────────────────│  /auth/otp/verify    │      │
│  └──────────┘                  └──────────────────────┘      │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│                ADMIN + PATIENT WEB (Realm B)                │
│  ┌──────────┐  Email+Pass  ┌──────────────────────┐        │
│  │ Browser  │─────────────►│  /auth/login         │        │
│  │          │◄── Cookie ───│  (Sanctum SPA)       │        │
│  └──────────┘              └──────────────────────┘        │
└─────────────────────────────────────────────────────────────┘
```

---

## 3. Data Model

### 3.1 `users` (Laravel default + extensions)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK nullable | NULL = super-admin; otherwise scoped |
| `uuid` | char(36) unique | Public-facing ID (used in URLs) |
| `first_name` | varchar(60) | |
| `last_name` | varchar(60) | |
| `email` | varchar(180) unique per tenant | Nullable for OTP-only patients |
| `phone` | varchar(20) unique per tenant | E.164 format `+91XXXXXXXXXX` |
| `phone_verified_at` | timestamp nullable | |
| `email_verified_at` | timestamp nullable | |
| `password` | varchar(255) nullable | NULL until set; OTP users may never have one |
| `avatar_path` | varchar(500) nullable | Spatie Media |
| `gender` | enum('m','f','o') nullable | |
| `dob` | date nullable | |
| `last_login_at` | timestamp nullable | |
| `last_login_ip` | varchar(45) nullable | |
| `status` | enum('active','inactive','suspended') default 'active' | |
| `locale` | varchar(10) default 'en' | |
| `timezone` | varchar(60) nullable | If null, use clinic timezone |
| `fcm_token` | varchar(500) nullable | Most recent device token |
| `remember_token` | varchar(100) | |
| `created_at`, `updated_at` | timestamps | |
| `deleted_at` | timestamp nullable | Soft delete |

Indexes: `(tenant_id, email)`, `(tenant_id, phone)`, `(uuid)`, `(deleted_at)`.

### 3.2 `roles` (Spatie)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK nullable | Per-tenant roles |
| `name` | varchar(60) | `super-admin`, `admin`, `doctor`, `front-desk`, `accountant`, `nurse` |
| `guard_name` | varchar(30) default 'web' | Spatie convention |
| `description` | varchar(255) nullable | |

### 3.3 `permissions` (Spatie)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `name` | varchar(80) | `view-appointments`, `edit-doctors`, etc. |
| `guard_name` | varchar(30) default 'web' | |
| `group` | varchar(60) | For UI grouping: `appointments`, `doctors`, ... |
| `description` | varchar(255) nullable | |

Seeded permissions follow `verb-noun` pattern. About 150 seed rows.

### 3.4 `role_has_permissions`, `model_has_roles`, `model_has_permissions`
Standard Spatie pivot tables.

### 3.5 `password_reset_tokens`
Standard Laravel.

### 3.6 `otp_verifications` (Realm A only — server-side throttling/logging)

| Column | Type |
|---|---|
| `id` | bigint PK |
| `phone` | varchar(20) indexed |
| `firebase_uid` | varchar(60) nullable |
| `purpose` | enum('login','register','reset') |
| `ip` | varchar(45) |
| `user_agent` | varchar(255) |
| `verified_at` | timestamp nullable |
| `created_at` | timestamp |
| `expires_at` | timestamp |

### 3.7 `login_audits`

| Column | Type |
|---|---|
| `id` | bigint PK |
| `user_id` | bigint FK nullable |
| `email_or_phone` | varchar(180) |
| `ip` | varchar(45) |
| `user_agent` | varchar(255) |
| `success` | boolean |
| `failure_reason` | varchar(120) nullable |
| `realm` | enum('mobile','web','admin') |
| `created_at` | timestamp |

### 3.8 `personal_access_tokens` (Sanctum)
Standard.

### 3.9 `fcm_tokens`

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `user_id` | bigint FK |
| `token` | varchar(500) unique | FCM registration token |
| `device_type` | enum('ios','android','web') |
| `device_id` | varchar(120) nullable | IDFA / Android ID |
| `app_version` | varchar(20) nullable |
| `last_seen_at` | timestamp |
| `created_at`, `updated_at` | timestamps |

A user can have multiple FCM tokens (one per device). When a notification is sent, we fan out to all active tokens.

---

## 4. RBAC: Roles & Permissions

### 4.1 Built-in Roles

| Role | Default Permissions |
|---|---|
| `super-admin` | ALL (bypasses Gate) |
| `admin` (clinic admin) | All except `manage-tenant`, `manage-billing-plan` |
| `doctor` | `view-own-appointments`, `edit-own-appointments`, `write-prescriptions`, `view-own-patients`, `start-video-call` |
| `front-desk` | `view-appointments`, `checkin-appointments`, `call-patient`, `view-patients`, `create-patient`, `create-appointment` |
| `accountant` | `view-transactions`, `view-invoices`, `create-invoice`, `process-refund` |
| `nurse` | `view-vitals`, `edit-vitals`, `view-appointments` |
| `lab-technician` | (V2) |
| `pathologist` | (V2) |

### 4.2 Permission Naming Convention

`<verb>-<resource>` where verb ∈ {view, create, update, delete, export, manage} and resource is lowercase.

Examples:
- `view-appointments`, `create-appointments`, `update-appointments`, `delete-appointments`, `manage-appointments`
- `view-doctors`, `create-doctors`, ...
- `view-transactions`, `export-transactions`
- `view-reports`
- `manage-settings`, `manage-roles`, `manage-billing`

### 4.3 Wildcards

`manage-X` is a wildcard for all CRUD on resource X. Implemented as a `Gate::before` check:

```php
Gate::before(function ($user, $ability) {
    return $user->hasPermissionTo('manage-' . Str::before($ability, '-'), 'web')
        ? true : null;
});
```

### 4.4 Ownership-Aware Authorization (Policies)

Even with `update-appointments` permission, a doctor can only update **their own** appointments. This is enforced by a Policy:

```php
class AppointmentPolicy {
    public function update(User $user, Appointment $appointment): bool {
        return $user->hasRole('doctor')
            ? $appointment->doctor_id === $user->doctor?->id
            : $user->can('update-appointments');
    }
}
```

### 4.5 Permission Seeding
`database/seeders/PermissionSeeder.php` runs in the installer. It creates ~150 permissions and 7 built-in roles. The super-admin role gets `*` (all).

---

## 5. API Endpoints

### 5.1 Mobile Auth (Realm A — Patient/Doctor Apps)

```
POST   /api/v1/auth/otp/send         { phone, purpose }   -> { sent: true }
POST   /api/v1/auth/otp/verify       { id_token, fcm_token?, device_type? }
                                                            -> { token, user, roles, permissions, tenant }
POST   /api/v1/auth/logout           (Bearer)             -> 204
POST   /api/v1/auth/refresh          (Bearer)             -> { token (rotated) }
GET    /api/v1/auth/me               (Bearer)             -> { user, roles[], permissions[] }
PUT    /api/v1/auth/me               (Bearer)             -> 200
POST   /api/v1/auth/fcm/register     (Bearer) { token, device_type, app_version }
DELETE /api/v1/auth/fcm/{id}         (Bearer)
```

#### `POST /auth/otp/verify` — example request/response

Request:
```json
{
  "id_token": "eyJhbGciOiJSUzI1NiIs...",
  "fcm_token": "fK3x...9a",
  "device_type": "android"
}
```

Response (200):
```json
{
  "data": {
    "token": "1|abc123def456...",
    "expires_at": "2026-07-04T12:00:00Z",
    "user": {
      "id": 42,
      "uuid": "8c3a...",
      "first_name": "Aarav",
      "last_name": "Sharma",
      "email": null,
      "phone": "+919876543210",
      "phone_verified_at": "2026-06-04T11:30:00Z",
      "avatar_url": null,
      "locale": "en",
      "tenant": { "id": 1, "name": "Apollo Clinic", "slug": "apollo" }
    },
    "roles": ["patient"],
    "permissions": ["view-own-appointments", "create-appointment", "view-own-prescriptions"]
  }
}
```

If phone is **not yet registered**:
```json
{ "data": { "status": "new_user", "phone": "+919876543210" } }
```
The app then shows a registration form to complete the profile.

#### Validation Rules

```php
// app/Modules/Auth/Http/Requests/OtpVerifyRequest.php
public function rules(): array {
    return [
        'id_token'    => ['required', 'string', 'min:100'],
        'fcm_token'   => ['nullable', 'string', 'max:500'],
        'device_type' => ['nullable', Rule::in(['ios', 'android', 'web'])],
    ];
}
```

### 5.2 Web Auth (Realm B — Admin/Patient Web)

```
POST   /api/v1/auth/login            { email, password, device_name? } -> { token|cookie }
POST   /api/v1/auth/logout           (Bearer|Cookie)
POST   /api/v1/auth/forgot-password  { email }      -> 202 (always, no user-enum)
POST   /api/v1/auth/reset-password   { token, email, password }
GET    /api/v1/auth/email/verify/{id}/{hash}       -> redirect to /login?verified=1
POST   /api/v1/auth/email/resend     (Bearer)
POST   /api/v1/auth/2fa/enable       (Bearer)       -> TOTP QR
POST   /api/v1/auth/2fa/confirm      (Bearer) { code }
POST   /api/v1/auth/2fa/verify       { code }       (during login)
```

### 5.3 Users (Admin only)

```
GET    /api/v1/users                 ?search=&role=&status=&page=
POST   /api/v1/users                 { first_name, last_name, email, phone, role, password }
GET    /api/v1/users/{id}
PUT    /api/v1/users/{id}
DELETE /api/v1/users/{id}            (soft delete; must unmap appointments first)
POST   /api/v1/users/{id}/reset-password  (admin force)
POST   /api/v1/users/{id}/impersonate     (super-admin only; audit-logged)
POST   /api/v1/users/{id}/suspend
POST   /api/v1/users/{id}/activate
GET    /api/v1/users/{id}/audit-logs
```

### 5.4 Roles (Admin only)

```
GET    /api/v1/roles
POST   /api/v1/roles                 { name, description, permissions[] }
GET    /api/v1/roles/{id}
PUT    /api/v1/roles/{id}
DELETE /api/v1/roles/{id}
POST   /api/v1/roles/{id}/assign     { user_ids[] }
POST   /api/v1/roles/{id}/revoke     { user_ids[] }
POST   /api/v1/roles/{id}/sync-permissions  { permissions[] }
```

### 5.5 Permissions (Admin only — read-mostly)

```
GET    /api/v1/permissions           (returns full registry grouped by resource)
GET    /api/v1/permissions/groups
```

---

## 6. Services (Business Logic)

All auth flows are routed through services, not in controllers. This makes them testable in isolation.

```
app/Modules/Auth/
├── Services/
│   ├── OtpAuthService.php           # Realm A
│   ├── PasswordAuthService.php      # Realm B
│   ├── FirebaseVerifier.php         # Wraps Kreait\Firebase\JWT
│   ├── TokenIssuer.php              # Sanctum token + claims
│   ├── PasswordResetter.php
│   ├── RoleService.php              # Spatie wrapper
│   ├── UserService.php
│   ├── LoginAuditor.php
│   └── DeviceRegistrar.php          # FCM tokens
├── Events/
│   ├── UserRegistered.php
│   ├── UserLoggedIn.php
│   ├── UserPasswordChanged.php
│   ├── UserRoleAssigned.php
│   └── UserSuspended.php
├── Listeners/
│   ├── SendWelcomeEmail.php
│   ├── LogSuccessfulLogin.php
│   ├── RevokeTokensOnPasswordChange.php
│   └── BroadcastRoleChange.php
└── Http/
    ├── Controllers/
    │   ├── OtpAuthController.php
    │   ├── PasswordAuthController.php
    │   ├── MeController.php
    │   ├── UserController.php
    │   ├── RoleController.php
    │   └── PermissionController.php
    ├── Requests/
    │   ├── OtpSendRequest.php
    │   ├── OtpVerifyRequest.php
    │   ├── LoginRequest.php
    │   ├── RegisterUserRequest.php
    │   └── UpdateUserRequest.php
    ├── Resources/
    │   ├── UserResource.php
    │   ├── RoleResource.php
    │   └── AuthResource.php
    └── Middleware/
        ├── EnsureTenantResolved.php
        ├── EnsureEmailVerified.php
        └── AuditLogin.php
```

### 6.1 `OtpAuthService::verifyAndIssueToken($idToken, $deviceType)`

```php
public function verifyAndIssueToken(string $idToken, ?string $deviceType = null): array
{
    // 1. Verify Firebase ID token
    $verified = $this->firebase->verifyIdToken($idToken);
    $phone = $verified->claims()['phone_number'] ?? null;
    $firebaseUid = $verified->claims()['sub'];
    if (!$phone) throw new OtpInvalidException('No phone in token');

    // 2. Resolve or create user (auto-register)
    $user = $this->resolveOrCreateUser($phone, $firebaseUid);

    // 3. Issue Sanctum token with tenant + roles + permissions claims
    $token = $this->tokenIssuer->issue($user, [
        'tenant_id' => $user->tenant_id,
        'roles' => $user->getRoleNames(),
    ]);

    // 4. Register FCM if provided
    if ($fcmToken = request()->input('fcm_token')) {
        $this->deviceRegistrar->register($user, $fcmToken, $deviceType);
    }

    // 5. Audit
    $this->loginAuditor->success($user, 'mobile');

    // 6. Event
    event(new UserLoggedIn($user, 'mobile'));

    return [
        'token' => $token->plainTextToken,
        'expires_at' => $token->accessToken->expires_at,
        'user' => UserResource::make($user)->resolve(),
        'roles' => $user->getRoleNames(),
        'permissions' => $user->getAllPermissions()->pluck('name'),
    ];
}
```

### 6.2 `RoleService::assign($user, $role, $tenantId)`

Atomic role assignment with audit. Throws if the role is `super-admin` and the actor is not.

### 6.3 `LoginAuditor`
Writes one row to `login_audits` for every attempt. IPs, user agents, success/failure, latency. Exposed via `/api/v1/admin/login-audits`.

---

## 7. Firebase Integration (Realm A)

### 7.1 Setup

1. Create one Firebase project shared by all 4 clients (admin, patient-web, user-app, doctor-app).
2. Enable **Phone** sign-in provider in Firebase Console.
3. Add SHA-256 fingerprints for Android (debug + release keystores).
4. Add `GoogleService-Info.plist` for iOS.
5. Add authorized domains in Firebase Console: `localhost`, `medicare.app`, `*.medicare.app`, all customer subdomains.
6. Generate a **Service Account** JSON (Firebase Admin SDK) and store in API `.env` as `FIREBASE_SERVICE_ACCOUNT_JSON` (single line).

### 7.2 Verifying ID Tokens (Laravel)

```php
// app/Modules/Auth/Services/FirebaseVerifier.php
use Kreait\Firebase\Factory;

class FirebaseVerifier {
    private Auth $auth;
    public function __construct() {
        $this->auth = (new Factory)
            ->withServiceAccount(json_decode(env('FIREBASE_SERVICE_ACCOUNT_JSON'), true))
            ->createAuth();
    }

    public function verifyIdToken(string $idToken): VerifiedIdToken {
        try {
            return $this->auth->verifyIdToken($idToken);
        } catch (\Throwable $e) {
            throw new OtpInvalidException('Invalid or expired token', previous: $e);
        }
    }
}
```

The Firebase Admin SDK verifies the JWT signature, expiry, audience, and issuer.

### 7.3 Auto-registration

On a new phone, we **do not** silently create a user. Instead:
1. API returns `{ "status": "new_user", "phone": "..." }`.
2. App shows a "complete your profile" screen.
3. App calls `POST /api/v1/auth/register/patient` with `{ phone, first_name, last_name, dob?, gender?, email? }`.
4. API creates the user, assigns the `patient` role, returns the token.

This avoids the GDPR/privacy issue of orphan accounts.

---

## 8. Twilio Fallback (Optional)

If `MEDICARE_OTP_PROVIDER=twilio`, the mobile app uses Twilio Verify SDK instead of Firebase. The API then exposes:

```
POST /auth/otp/send     { phone }      -> calls Twilio Verify Services API
POST /auth/otp/verify   { phone, code } -> checks via Twilio
```

`TwilioVerifyService` is the drop-in replacement for `FirebaseVerifier`. Same controller, different service binding.

> **V1.2 — full Twilio deep-dive:** see **`11-MODULE-NOTIFICATIONS.md §20.2`** for the 21-section treatment covering Programmable Messaging (5 delivery statuses `queued`/`sent`/`delivered`/`undelivered`/`failed`, A2P 10DLC US, DLT India), Verify API (OTP cost $0.05/verification), 6 opt-out keywords (`STOP`/`STOPALL`/`UNSUBSCRIBE`/`CANCEL`/`END`/`QUIT`) + 2 opt-in (`START`/`YES`/`UNSTOP`), 2-way SMS chat binding to in-app chat thread, regulatory bundle metadata in `sms_phone_numbers.regulatory`, and the 21-section acceptance criteria. This §8 is the V1 OTP fallback; the deep-dive at 11-Notifications is canonical.

---

## 9. Sessions, Tokens, and Devices

### 9.1 Mobile Tokens
- Sanctum personal access tokens, hashed in DB, with `expires_at = now() + 30 days`.
- Stored in `flutter_secure_storage` (Keystore on Android, Keychain on iOS).
- Auto-refreshed silently 7 days before expiry by the app on the next foreground.
- Revoked on logout, password change, or admin "Force logout".

### 9.2 Web Cookies
- `web` guard uses Sanctum's `EnsureFrontendRequestsAreStateful` middleware.
- Cookie name: `medicare_session`. Domain: `.medicare.app` for tenant sharing.
- SameSite=Lax. Secure. HttpOnly.

### 9.3 Admin Impersonation
A super-admin can "log in as" any user. The impersonator's `user_id` is stamped on every audit row. UI shows a yellow banner "You are impersonating {user}" with a "Stop" button.

---

## 10. Security Rules (Enforced)

1. **Passwords** — minimum 10 characters, must contain upper/lower/digit/symbol. zxcvbn score ≥ 3.
2. **OTP rate limits** — 5 sends per phone per hour, 10 per IP per hour. Returns `429`.
3. **Login throttling** — 5 failed attempts per email/phone in 15 min → 30-min lockout. `Illuminate\Cache\RateLimiter` with custom key.
4. **Token fingerprinting** — token is bound to the device's `user_agent` hash. If it changes, all tokens are revoked.
5. **Audit retention** — `login_audits` retained 1 year; rolled up to monthly counts after 90 days.
6. **No passwords in logs** — Laravel's `Password::broker()` is wrapped in a `RedactingLogger`.
7. **No PHI in JWT claims** — only `user_id`, `tenant_id`, `roles`, `exp`. Resource data fetched fresh per request.

---

## 11. Web (Next.js) Implementation Notes

- `medicare-web/apps/admin` and `apps/patient-web` use the same auth flow.
- Use `nuxt-auth`-style: server component reads session via `auth()` helper, redirects on 401.
- Form library: `react-hook-form` + `zod` for client-side validation matching server rules.
- 2FA enrollment page uses `otplib` (or `@epic-web/totp`) to render the QR.

### Admin login screen (sketch)

```
┌──────────────────────────────────────────────┐
│                  [Logo]                      │
│                                              │
│           Sign in to Medicare                │
│                                              │
│   ┌────────────────────────────────────┐     │
│   │ Email                              │     │
│   └────────────────────────────────────┘     │
│   ┌────────────────────────────────────┐     │
│   │ Password                       👁  │     │
│   └────────────────────────────────────┘     │
│   [ Remember me ]              [Forgot?]     │
│                                              │
│   [            Sign in            ]          │
│                                              │
│   © 2026 Your Clinic                          │
└──────────────────────────────────────────────┘
```

## 12. Mobile (Flutter) Implementation Notes

### 12.1 Auth State Machine (Riverpod)

```dart
sealed class AuthState {}
class AuthInitial extends AuthState {}
class AuthLoading extends AuthState {}
class AuthNewUser extends AuthState { final String phone; }
class AuthAuthenticated extends AuthState { final User user; final String token; }
class AuthUnauthenticated extends AuthState { final String? error; }

class AuthNotifier extends Notifier<AuthState> {
  @override
  AuthState build() => AuthInitial();

  Future<void> verifyOtp(String idToken) async {
    state = AuthLoading();
    final res = await ref.read(authRepoProvider).verifyOtp(idToken, fcmToken: ...);
    state = switch (res.status) {
      'new_user' => AuthNewUser(res.phone!),
      _ => AuthAuthenticated(res.user!, res.token!),
    };
  }
  ...
}
```

### 12.2 Auto-routing
`GoRouter` `redirect` watches the auth state:
- `AuthInitial` → splash
- `AuthNewUser` → `/register`
- `AuthAuthenticated` → role-based home
- `AuthUnauthenticated` → `/login`

### 12.3 Token Refresh Interceptor (Dio)

```dart
class AuthInterceptor extends Interceptor {
  @override
  void onRequest(options, handler) {
    final token = ref.read(secureStorageProvider).getToken();
    if (token != null) options.headers['Authorization'] = 'Bearer $token';
    handler.next(options);
  }

  @override
  void onError(err, handler) async {
    if (err.response?.statusCode == 401) {
      await ref.read(authRepoProvider).refresh();
      // retry once
    }
    handler.next(err);
  }
}
```

---

## 13. Realtime Events

- `UserLoggedIn` (own devices, multi-device alert) → broadcast on `user.{id}` private channel.
- `UserSuspended` → broadcast on `user.{id}`; app clears secure storage and routes to login.
- `UserRoleAssigned` → broadcast; app refreshes its permission cache.
- `Appointment` events from other modules broadcast on `user.{id}`.

---

## 14. Audit Hooks

`User::created/updated/deleted` is in the global audit list. The `audit_logs` row stores: `event`, `user_id`, `actor_id`, `old_values` (diff), `new_values`, `ip`, `user_agent`, `tenant_id`, `url`.

---

## 15. Testing Strategy

| Test | Type | Covers |
|---|---|---|
| `OtpAuthServiceTest` | Unit | Firebase verification, user creation, token issuance |
| `PasswordAuthServiceTest` | Unit | bcrypt, throttling, audit |
| `RoleServiceTest` | Unit | Spatie interactions, super-admin check |
| `OtpAuthControllerTest` | Feature | HTTP, validation, error responses |
| `LoginThrottleTest` | Feature | 5 failed → 6th 429 |
| `RoleMiddlewareTest` | Feature | 403 for missing permission |
| `TenantScopeTest` | Feature | User A cannot see User B in another tenant |
| `E2E: Patient login` | Browserstack | Real Firebase test phone → real SMS |
| `E2E: Admin login` | Playwright | Real email + password → dashboard |

---

## 16. Acceptance Criteria (V1)

- [ ] Patient can register and log in with a real phone, receiving a real SMS, on a real device, on a real network.
- [ ] Same patient can log in on 2 devices; both work; both get FCM pushes.
- [ ] Logging out on one device does **not** log out the other.
- [ ] Admin can create a doctor with email + temp password; doctor receives an email; doctor logs in and is forced to set a new password.
- [ ] 6th wrong password in 15 min → locked out for 30 min; admin can unlock.
- [ ] Super-admin can impersonate a doctor; all writes during impersonation are marked in audit.
- [ ] Suspended user cannot log in even with correct creds.
- [ ] Token refresh works silently; user never sees the refresh happen.
- [ ] `audit_logs` is populated for every user create/update/delete.
- [ ] `login_audits` is populated for every login attempt.

---

## 17. V2 Roadmap — Auth

> V1 auth is phone-OTP (mobile) + email-password (web). V2 adds OIDC SSO, ABDM/ABHA, MFA enforcement, passkeys, and a federated identity model.

### 17.1 New table: `user_identities` (federated login)

```php
Schema::create('user_identities', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('user_id')->index();
    $t->ulid('tenant_id')->index();
    $t->string('provider', 32);             // 'password' | 'phone' | 'abha' | 'google' | 'apple' | 'oidc:{slug}'
    $t->string('provider_subject', 255);     // sub claim from OIDC, ABHA, etc.
    $t->string('provider_issuer', 255)->nullable();
    $t->json('profile')->nullable();         // raw claims, encrypted
    $t->timestamp('last_used_at')->nullable();
    $t->timestamps();
    $t->unique(['provider', 'provider_subject']); // one federated identity maps to one user globally
    $t->unique(['user_id', 'provider']);           // but a user can have multiple providers
    $t->foreign('user_id')->references('id')->on('users')->cascadeOnDelete();
});
```

A user with `user_id=42` can have:
- `provider=phone, subject=+919876543210` (V1)
- `provider=abha, subject=14-3344-5566-7788` (V2)
- `provider=google, subject=108...` (V2)
- `provider=oidc:acme-corp, subject=acme|alice` (V2 SSO)

### 17.2 New table: `mfa_credentials` (TOTP + WebAuthn)

```php
Schema::create('mfa_credentials', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('user_id')->index();
    $t->ulid('tenant_id')->index();
    $t->string('type', 16);                  // 'totp' | 'webauthn' | 'sms' | 'backup_codes'
    $t->string('name', 64)->nullable();      // 'YubiKey 5', 'iPhone 15 Pro', etc.
    $t->text('credential');                  // encrypted: TOTP secret / WebAuthn credential / hashed backup codes
    $t->json('metadata')->nullable();         // WebAuthn challenge, counter, transports
    $t->boolean('is_primary')->default(false);
    $t->timestamp('last_used_at')->nullable();
    $t->timestamps();
    $t->foreign('user_id')->references('id')->on('users')->cascadeOnDelete();
});
```

### 17.3 New table: `identity_providers` (OIDC config per tenant)

```php
Schema::create('identity_providers', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('slug', 64);                  // 'acme-corp-okta'
    $t->string('display_name', 128);
    $t->string('type', 16);                  // 'oidc' | 'saml'
    $t->json('config');                      // issuer URL, client_id, client_secret (encrypted), scopes, claims_map
    $t->string('default_role', 32)->default('staff');
    $t->boolean('jit_provisioning')->default(true);
    $t->boolean('is_enabled')->default(true);
    $t->timestamps();
    $t->unique(['tenant_id', 'slug']);
});
```

### 17.4 ABDM/ABHA integration (`abdm-bridge`, V1.2, MML 3) — deep-dive

**1. Overview.** India's **Ayushman Bharat Digital Mission (ABDM)** is the National Health Authority's (NHA) program to give every Indian a portable, consent-based health record. The identity is the **ABHA number** (14 digits) or an **ABHA address** (e.g., `name@abdm`, `name@ndhm`). The Medicare `abdm-bridge` module (V1.2, MML 3, Pro tier) lets a patient **link** their ABHA to their Medicare profile, **verify** the link via NHA's OTP flow, **pull** their 24-month health-record history from the NHA HIU (Health Information User) gateway (with explicit consent), and **push** new records (visits, prescriptions, lab reports) back to the patient's ABHA locker via the NHA HIP (Health Information Provider) gateway. MML 3 GA: ABHA number + ABHA address linking, NHA sandbox + production, profile sync, HIU pull (24-month), HIP push (visit + Rx + lab), consent artifact storage, ABDM M1/M2 certification-ready. ABDM M1/M3 (HIP/HIU) certification is a NHA requirement to operate in production; the platform supports the technical integration, the certification process itself is run by the tenant and NHA jointly. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.2 lane module.

**2. The ABDM gateway architecture.** NHA runs a **gateway** of three roles:
- **HIP (Health Information Provider):** pushes records. Medicare acts as a HIP for our patients' visits, prescriptions, lab reports.
- **HIU (Health Information User):** pulls records. Medicare acts as a HIU to fetch a patient's 24-month history from other HIPs (e.g., other hospitals, labs, pharmacies) they have linked to their ABHA.
- **HIE-CM (Consent Manager):** manages consent artifacts. Every push or pull is gated by a `ConsentArtifact` JSON object issued by the patient via an HIE-CM (e.g., the ABDM mobile app, or our integrated consent screen). We store the consent artifact in `abdm_consent_artifacts` and verify it on every push/pull.
The gateway itself is OAuth 2.0 + client-cert (mTLS): Medicare holds a client cert issued by NHA, signed by NCA (National Certification Authority), and is identified by a `client_id` (e.g., `medicare-prod-001`).

**3. Configuration.**
```bash
# In config/abdm.php
ABDM_ENV=sandbox|production
ABDM_CLIENT_ID=medicare-prod-001                    # issued by NHA per tenant
ABDM_CLIENT_SECRET=...                                # issued by NHA per tenant
ABDM_PUBLIC_KEY_PATH=/etc/medicare/abdm/nha-sandbox.pub   # NHA's public key
ABDM_PRIVATE_KEY_PATH=/etc/medicare/abdm/medicare.key    # our signing key
ABDM_CERT_PATH=/etc/medicare/abdm/medicare.crt.pem       # mTLS cert
ABDM_GATEWAY_URL=https://gateway.abdm.gov.in             # production
# ABDM_GATEWAY_URL=https://sandbox.abdm.gov.in            # sandbox
ABDM_HIU_FETCH_MONTHS=24                                # history depth
ABDM_HIP_PUSH_ON_VISIT=true
ABDM_HIP_PUSH_ON_RX=true
ABDM_HIP_PUSH_ON_LAB=true
ABDM_FEATURE_ABDM=true                                  # gates the module
```

**4. The link flow (patient → Medicare).**
1. User opens the mobile app → `Profile → Link ABHA`.
2. App calls `POST /api/v1.2/auth/abha/initiate` with `{ abha_number | abha_address }`. The API forwards to NHA's `POST /abdm/api/v3/registration/auth/init` (the auth-init endpoint), which sends an OTP to the Aadhaar-linked mobile. The API returns `{ transaction_id, otp_sent_to: 'aadhaar-linked-mobile' }`.
3. User enters OTP in the app. App calls `POST /api/v1.2/auth/abha/verify` with `{ transaction_id, otp }`. The API forwards to NHA's `POST /abdm/api/v3/registration/auth/confirm`. NHA returns `{ abha_number, abha_address, name, gender, dob, mobile, email, photo_url }` (the verified profile).
4. The API finds-or-creates the user: it first looks up `user_identities` for `(provider='abha', subject=abha_number)`; if found, attaches to the existing user; if not, looks up the user by mobile (and a `link_token` is generated and sent to the mobile to confirm linking); if still not found, creates a new patient profile pre-populated with the ABDM profile.
5. The API upserts the ABHA identity in `user_identities` with `provider='abha'`, `subject=abha_number`, `metadata={abha_address, name, gender, dob, ...}`.
6. The API issues a Sanctum token; the patient is logged in.

**5. The HIU pull flow (fetch 24-month history).** After linking, the patient can grant consent to pull their history from other HIPs:
1. App calls `POST /api/v1.2/auth/abha/consent/initiate` with `{ purpose: 'care-management', hip_ids?: [] }`. The API calls NHA's `POST /abdm/api/v3/consent/request/init`. NHA's HIE-CM (which the patient has installed, e.g., the official ABDM app) prompts the patient to grant consent to specific HIPs and a date range. The patient grants; the HIE-CM returns a `ConsentArtifact` JSON: `{ consent_id, patient_abha, hip_id, hi_types: ['OPConsultation', 'Prescription', 'DiagnosticReport', 'DischargeSummary'], date_range: {...}, purpose, expiry, signature }`. The API stores this in `abdm_consent_artifacts`.
2. The API then calls NHA's `POST /abdm/api/v3/health-information/fetch` with the `consent_id` and our HIP list. NHA returns a `Bundle` of FHIR R4 `Composition` + `MedicationRequest` + `Condition` + `DiagnosticReport` + `Encounter` resources for the requested `hi_types`. The API maps the Bundle to local tables:
   - `Composition` → `patient_visits` (with `source='abdm'`, `source_id=composition.id`).
   - `MedicationRequest` → `prescriptions` (read-only; `source='abdm'`).
   - `Condition` → `patient_problem_list` (read-only; `source='abdm'`).
   - `DiagnosticReport` → `patient_lab_results` (read-only; `source='abdm'`).
   - `Encounter` → `patient_encounters` (read-only; `source='abdm'`).
3. Each mapped row has `source='abdm'`, `source_id=<resource.id>`, `pulled_at`, `consent_id`. The data is **read-only** in our system: a `BEFORE UPDATE` trigger on each table raises an exception if the row is `source='abdm'`; corrections create a new local row with `source='local'` and `corrects_abdm_id=<row.id>`. This preserves the ABDM provenance.
4. **Refresh cadence:** every 30 days, if the patient re-grants consent, we re-pull the records (incremental: NHA returns only records new since the last `pulled_at`). The consent artifact is renewed at the same time.

**6. The HIP push flow (push visit/Rx/lab to ABHA locker).** When a patient is treated at a Medicare facility, we push the record to the patient's ABHA locker so other providers can see it:
1. On `appointments` transition to `completed`, the API calls NHA's `POST /abdm/api/v3/health-information/notify` with the new `Encounter` and its child `MedicationRequest` / `DiagnosticReport` / `Composition` resources. NHA returns a `notification_id`; the API stores it in `abdm_push_log`.
2. The push is gated by **implicit consent** (the patient linked their ABHA and the visit is for them) — NHA does not require a fresh consent artifact for push, only for pull. The push is **fire-and-forget**; failures are retried with exponential backoff (1h, 6h, 24h, 72h, 7d), then dead-lettered to `abdm_push_failed` for manual review.
3. The push is **idempotent** on the ABDM side: each resource has a `medicare_resource_id` (`appointments.id`, `prescriptions.id`, `lab_results.id`) that ABDM uses for dedup. Re-pushing the same resource does not create a duplicate.

**7. Domain Model.**
```php
// user_identities — extended (V1.2)
Schema::table('user_identities', function (Blueprint $t) {
    $t->string('abha_address', 64)->nullable()->index();   // e.g., 'name@abdm'
    $t->jsonb('abdm_profile')->nullable();                  // {name, gender, dob, mobile, email, photo_url}
    $t->timestamp('abdm_linked_at')->nullable();
});

// abdm_consent_artifacts
Schema::create('abdm_consent_artifacts', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id');
    $t->ulid('patient_id');
    $t->string('consent_id', 64)->unique();                // NHA's consent_id
    $t->jsonb('artifact');                                  // the full ConsentArtifact JSON
    $t->jsonb('hi_types');                                  // ['OPConsultation', 'Prescription', ...]
    $t->jsonb('hip_ids')->nullable();                       // list of HIPs (e.g., ['hosp-001', 'lab-002'])
    $t->timestamp('granted_at');
    $t->timestamp('expires_at');
    $t->string('status', 16)->default('active');           // 'active' | 'expired' | 'revoked'
    $t->timestamps();
    $t->index(['tenant_id', 'patient_id', 'status']);
});

// abdm_push_log
Schema::create('abdm_push_log', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id');
    $t->ulid('patient_id');
    $t->string('resource_type', 32);                        // 'appointment' | 'prescription' | 'lab_result'
    $t->ulid('medicare_resource_id');                       // idempotency key
    $t->string('notification_id', 64)->nullable();          // NHA's notification_id
    $t->string('status', 16)->default('pending');           // 'pending' | 'pushed' | 'failed'
    $t->integer('attempts')->default(0);
    $t->text('last_error')->nullable();
    $t->timestamp('next_retry_at')->nullable();
    $t->timestamp('pushed_at')->nullable();
    $t->timestamps();
    $t->unique(['tenant_id', 'resource_type', 'medicare_resource_id'], 'abdm_push_idempotency');
    $t->index(['status', 'next_retry_at']);
});

// abdm_pull_log
Schema::create('abdm_pull_log', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id');
    $t->ulid('patient_id');
    $t->ulid('consent_artifact_id');
    $t->integer('records_pulled')->default(0);
    $t->jsonb('resource_types_summary')->nullable();        // {Composition: 12, MedicationRequest: 5, ...}
    $t->string('status', 16)->default('pending');           // 'pending' | 'completed' | 'failed'
    $t->text('last_error')->nullable();
    $t->timestamp('pulled_at');
    $t->timestamps();
});
```
The ABDM-pulled data lives in existing tables (`patient_problem_list`, `patient_lab_results`, `patient_encounters`, `prescriptions`) with `source='abdm'` and `source_id` set. The `BEFORE UPDATE` trigger enforces read-only on ABDM rows.

**8. API Surface.**
```
# Link / unlink
POST   /api/v1.2/auth/abha/initiate          { abha_number | abha_address }  -> { transaction_id, otp_sent_to }
POST   /api/v1.2/auth/abha/verify            { transaction_id, otp }        -> { user, token, identities[] }
POST   /api/v1.2/auth/abha/unlink                                          -> 204
GET    /api/v1.2/auth/abha/profile                                         -> { abha_number, abha_address, name, ..., linked_at }

# Consent + pull
POST   /api/v1.2/auth/abha/consent/initiate  { purpose, hip_ids?, hi_types } -> { consent_request_id, redirect_to_hiecm }
POST   /api/v1.2/auth/abha/consent/callback  { consent_id }                 -> { consent_artifact, status }
GET    /api/v1.2/auth/abha/consents                                         -> [consent_artifact]
DELETE /api/v1.2/auth/abha/consents/{id}                                    -> revoke + 204
POST   /api/v1.2/auth/abha/pull              { consent_artifact_id }        -> { pull_log_id, job_queued: true }
GET    /api/v1.2/auth/abha/pull/{pull_log_id}                               -> { status, records_pulled, ... }

# Push (admin)
GET    /api/v1.2/admin/abdm/push-log                                        -> [push_log] (paginated, filterable)
POST   /api/v1.2/admin/abdm/push-retry/{push_log_id}                        -> { status, next_retry_at }
GET    /api/v1.2/admin/abdm/status                                          -> { gateway_reachable, last_sync, push_pending, push_failed, m1_cert }
```

**9. Frontend.**
- **Mobile (patient app):** `/profile/link-abha` is a 3-step wizard: enter ABHA → enter OTP → confirm. The same screen shows the linked ABHA, the consents granted, and a "Refresh history" button. The patient's locker is accessible at the official ABDM app (link provided in Medicare app after linking).
- **Web (doctor app):** `/patient/{id}/abdm` shows the ABDM-sourced rows (read-only) with a clear "Source: ABDM" badge. Clicking a row opens the original FHIR resource in a modal (read-only).
- **Admin (web):** `/admin/integrations/abdm` shows: gateway status (green/red), M1 certification status, push-pending count, push-failed count (with a retry-all button), pull log (last 100), and a "Test connection" button (runs `POST /abdm/api/v3/registration/auth/init` with a known test ABHA and verifies the response).

**10. Realtime.**
- `abdm.link.completed.{tenant_id}` — patient linked ABHA.
- `abdm.consent.granted.{tenant_id}.{patient_id}` — patient granted a consent artifact; UI prompts the doctor.
- `abdm.consent.revoked.{tenant_id}.{patient_id}` — patient revoked a consent; ABDM-sourced rows become read-only-and-stale (UI shows "ABDM access revoked" badge).
- `abdm.pull.completed.{tenant_id}.{patient_id}` — pull finished; UI shows a toast and a "View new records" link.
- `abdm.push.failed.{tenant_id}` — push dead-lettered; admin paged.

**11. Security & RBAC.**
- **mTLS** for every NHA gateway call (per the gateway's own requirement). Our cert is stored in AWS Secrets Manager; the private key is HSM-backed (CloudHSM or KMS CMK with `EXTERNAL` origin).
- **Patient scoping** — a patient's ABHA can only be linked to their own Medicare profile; the mobile-OTP link step (step 4 above) prevents the case where patient A enters patient B's ABHA by mistake. A patient can unlink at any time (`DELETE /api/v1.2/auth/abha/unlink`).
- **ABDM rows are read-only** — enforced by a `BEFORE UPDATE` trigger on each ABDM-sourced table. Corrections create a new local row with `source='local'`, `corrects_abdm_id=<row.id>`, preserving the ABDM provenance for the audit trail. This satisfies NHA's "ABDM data is authoritative and cannot be modified" requirement.
- **Consent artifacts** are stored with the patient's `patient_id`; they expire automatically (the `expires_at` is honored; expired artifacts are not used for pull). A patient can revoke at any time via `DELETE /api/v1.2/auth/abha/consents/{id}`.
- **Roles:** `patient` (link, view own data, grant/revoke consent), `doctor` (view ABDM-sourced data for own patients; cannot push — push is system-driven), `tenant_admin` (configure ABDM integration, retry push, view push log), `compliance_officer` (view consent audit, view pull log).
- **Audit:** every link/unlink, every consent grant/revoke, every pull, every push is logged in `audit_logs` with `action ∈ {'abdm.link', 'abdm.unlink', 'abdm.consent.grant', 'abdm.consent.revoke', 'abdm.pull', 'abdm.push'}` and the resource ID.
- **PII minimization:** the ABDM profile is stored in `user_identities.abdm_profile` (JSONB) and is **not** copied to `users` or `patients` automatically. The patient is asked: "Update Medicare profile from ABDM?" on first link; if yes, fields like name/dob are copied.
- **Encryption:** the ABDM pull FHIR Bundles are encrypted at rest (per V2.1 `field-level-encryption`) when stored in `abdm_pull_log.resource_types_summary`; the FHIR resources themselves (mapped to local tables) are protected by the local column encryption.

**12. Acceptance Criteria.**
- [ ] Patient can link ABHA via ABHA number or ABHA address; the OTP round-trips against NHA sandbox.
- [ ] Linked patient sees their ABDM profile (name, gender, dob, mobile, email) populated; unlinking removes the ABHA identity.
- [ ] Patient can grant consent; the consent artifact is stored; pull is queued; the FHIR Bundle is fetched and mapped to local tables with `source='abdm'`.
- [ ] ABDM-sourced rows are read-only in the doctor app (no edit/delete).
- [ ] A correction in Medicare creates a new local row with `corrects_abdm_id` set; the original ABDM row is untouched.
- [ ] A visit completion triggers a push; the push log shows `status='pushed'`; the ABHA locker shows the visit (verified in NHA's test locker).
- [ ] Push failures retry with backoff; after 5 attempts, the row is dead-lettered; admin can retry manually.
- [ ] Consent revocation marks all ABDM-sourced rows as stale (UI badge); the rows are not deleted.
- [ ] The 30-day refresh re-pulls only the incremental records; consent artifacts are re-issued.
- [ ] Every action emits the right `audit_logs` row.
- [ ] mTLS is enforced: a call to NHA with a wrong cert fails fast with a clear error.
- [ ] Performance: link takes < 5s end-to-end (sandbox latency); pull of 50 records takes < 30s; push is fire-and-forget (< 1s API response).
- [ ] ABDM M1 (sandbox certification) is signed off; M2/M3 (HIP/HIU production certification) is a tenant-driven process, but the technical integration is complete.

**13. Real-World Test Scenarios.**
- *Link via ABHA number:* patient types their 14-digit ABHA; receives OTP on Aadhaar-linked mobile; enters OTP; sees their profile populated; the audit log shows `abdm.link`.
- *Link via ABHA address:* patient types `name@abdm`; same flow, lookup by address.
- *Wrong OTP:* patient types wrong OTP 3x; NHA blocks the transaction; API returns 401; patient retries; after 5 wrong attempts, the ABHA is locked for 24h (NHA's policy).
- *Pull history:* patient grants consent for last 24 months; the FHIR Bundle is fetched; 30 records (10 visits, 12 Rx, 8 labs) are mapped to local tables; the doctor sees them with "Source: ABDM" badge.
- *Correction:* doctor realizes the ABDM-sourced problem list has an error; instead of editing, they add a new local entry "Corrects ABDM #abc123"; the ABDM row is untouched; the local row links back via `corrects_abdm_id`.
- *Push on visit completion:* patient visits doctor; visit is completed; the API pushes the encounter to NHA; the push log shows `status='pushed'`; the patient's ABDM app shows the new visit within 1 hour.
- *Push failure + retry:* NHA is down; push fails; the retry runs at +1h, +6h, +24h; on the 4th attempt it succeeds; the push log shows 4 attempts; no data is lost.
- *Consent revocation:* patient revokes a consent artifact; the ABDM-sourced rows show the "Access revoked" badge; the doctor cannot pull new data; the existing data is still visible (read-only) but marked stale.
- *30-day refresh:* 30 days after the initial pull, the system auto-pulls; only 3 new records (1 visit, 2 Rx) are fetched; the consent artifact is renewed; the patient gets a push notification "We've updated your history from ABDM".
- *Production cert:* tenant goes through NHA M1/M2/M3 certification; the integration is unchanged; only the gateway URL and the cert change from sandbox to production.

**14. What this module is NOT:**
- Not a replacement for the ABDM mobile app. Patients manage their ABHA, consents, and locker via the official ABDM app (or the HIE-CM of their choice); Medicare only **uses** the ABHA identity and gateway.
- Not a write authority for ABDM data. ABDM-sourced data is read-only in Medicare; corrections are local and traceable.
- Not available globally. ABDM is India-only; the module is feature-gated by `ABDM_FEATURE_ABDM=true` per tenant (and, optionally, per tenant region).
- Not a silver bullet for interoperability with Indian hospitals. ABDM is the foundation; specific HIPs may have edge cases (e.g., a lab that pushes only `DiagnosticReport.summary` and not the full `Observation`). We log and surface these gaps.

**15. Dev Workflow.** `php artisan abdm:test --tenant=...` runs a smoke test (link a sandbox ABHA, pull, push, unlink). `php artisan abdm:push-retry --tenant=...` retries dead-lettered pushes. `php artisan abdm:consent-expire` marks expired consents. `php artisan abdm:re-pull --patient=...` triggers a manual re-pull. Code: `app-modules/abdm-bridge/`; tests: `app-modules/abdm-bridge/tests/Feature/{Link,Pull,Push,ConsentRevoke,Trigger,Correction,Retr}yTest.php` and `tests/Compliance/AbdmReadOnlyTriggerTest.php` (asserts the trigger blocks UPDATE on `source='abdm'` rows). Sandbox: `MOCK_ABDM_URL=http://localhost:9002` in dev (per `23-Development-Setup §11`); the mock ABDM gateway returns canned FHIR Bundles.

### 17.5 OIDC / SAML SSO (`sso-oidc`, V3)

Enterprise tenants can configure their own identity provider (IdP) — OIDC (Okta, Azure AD, Auth0, Keycloak) or SAML 2.0 (Azure AD, OneLogin, Ping, Shibboleth).

#### 17.5.1 OIDC flow

1. Admin: `Settings → SSO → Add Provider → OIDC` enters `{issuer, client_id, client_secret, redirect_uri, default_role, claim_mappings}`.
2. API validates the discovery document at `{issuer}/.well-known/openid-configuration` (issuer must match `iss` claim, JWKS endpoint reachable, required scopes available).
3. User clicks "Sign in with SSO" on the login page. The API generates a PKCE challenge (`code_verifier` 43-128 chars, `code_challenge` SHA-256), generates a `state` (CSRF, 32-byte random, stored in Redis with 10-min TTL), and `302`s to `{issuer}/authorize?...`.
4. IdP returns to `GET /api/v3/auth/oidc/{slug}/callback?code&state`. The API:
   - Verifies `state` matches the Redis value.
   - Exchanges `code` for tokens at `{issuer}/token` (with `code_verifier`).
   - Verifies the ID token signature against the JWKS.
   - Verifies `iss`, `aud`, `exp`, `nonce` (if used).
   - Maps claims: `email → users.email`, `sub → user_identities.subject`, `name → users.name`, `groups → roles` (per `claim_mappings.group_claim` and `claim_mappings.role_map`).
5. **JIT provisioning** (per 14-V2-Stubs §4.3): if `user_identities` has no row for `(provider, subject)`, the API finds the user by `email` (must match an existing tenant user) or creates a new user with `default_role`. New users get a random password and a `must_reset_password = true` flag; they are expected to log in via SSO thereafter.
6. The API issues a Sanctum token (same as a normal login) and returns it to the client. The client stores it in httpOnly cookie (web) or secure storage (mobile).

#### 17.5.2 SAML 2.0 flow

1. Admin: `Settings → SSO → Add Provider → SAML` enters `{idp_entity_id, sso_url, slo_url, x509_cert, default_role, claim_mappings}`.
2. The API exposes SP metadata at `GET /api/v3/auth/saml/{slug}/metadata` (XML, signed).
3. User clicks "Sign in with SSO" → `GET /api/v3/auth/saml/{slug}/sso?RelayState=...` → 302 to `{sso_url}?SAMLRequest=...` (AuthnRequest, signed with our SP private key, signed with SHA-256 + RSA).
4. IdP posts a SAML Response to `POST /api/v3/auth/saml/{slug}/acs` (Assertion Consumer Service). The API:
   - Verifies the signature against the IdP's `x509_cert`.
   - Verifies `Destination` matches our ACS URL.
   - Verifies `NotBefore`, `NotOnOrAfter`, `Audience` matches our entity id.
   - Extracts the `NameID` and the configured attributes (e.g., `email`, `displayName`, `group`).
   - Same JIT provisioning as OIDC.
5. **SLO (Single Logout):** the API exposes `GET /api/v3/auth/saml/{slug}/slo` for IdP-initiated logout. On receipt, the API invalidates the Sanctum token, redirects to the IdP's SLO endpoint with a signed LogoutRequest.

#### 17.5.3 API surface (V3 GA)

The existing OIDC endpoints at §17.8 (line 790-791) are extended:
```
POST   /api/v3/auth/oidc                              # register OIDC provider (admin)
GET    /api/v3/auth/oidc                              # list configured providers
GET    /api/v3/auth/oidc/{slug}                       # fetch one provider
PATCH  /api/v3/auth/oidc/{slug}                       # update (admin)
DELETE /api/v3/auth/oidc/{slug}                       # remove (admin; revokes all sessions)
POST   /api/v3/auth/oidc/{slug}/test                  # test connection (admin): does discovery, returns IdP info
POST   /api/v3/auth/oidc/{slug}/initiate              # 302 to IdP (was /api/v2 in V2 stub)
GET    /api/v3/auth/oidc/{slug}/callback              # was /api/v2 in V2 stub
POST   /api/v3/auth/saml                              # register SAML provider (admin)
GET    /api/v3/auth/saml                              # list SAML providers
PATCH  /api/v3/auth/saml/{slug}
DELETE /api/v3/auth/saml/{slug}
GET    /api/v3/auth/saml/{slug}/metadata              # SP metadata XML
GET    /api/v3/auth/saml/{slug}/sso                   # initiate SSO
POST   /api/v3/auth/saml/{slug}/acs                   # Assertion Consumer Service
GET    /api/v3/auth/saml/{slug}/slo                   # Single Logout
POST   /api/v3/auth/scim/v2/{slug}/Users              # SCIM v2 create user (V3.1)
PATCH  /api/v3/auth/scim/v2/{slug}/Users/{id}         # SCIM v2 update
DELETE /api/v3/auth/scim/v2/{slug}/Users/{id}         # SCIM v2 delete
GET    /api/v3/auth/scim/v2/{slug}/Users              # SCIM v2 list
GET    /api/v3/auth/scim/v2/{slug}/ServiceProviderConfig
```

#### 17.5.4 Schema additions (per 15-DB-Schema)

`identity_providers` (extended from V2 stub) — per-tenant OIDC and SAML config, including the discovery URL, JWKS, client_id, hashed client_secret, claim mappings, status, default role, JIT provisioning toggle, SCIM endpoint URL and token. New table `user_identities` (V2 stub extended) — `(tenant_id, provider_slug, subject) → user_id`. New table `oidc_states` (transient, Redis-backed per `00-Arch §6`). New table `saml_assertions` (transient, 1-hour TTL for replay protection).

#### 17.5.5 Security model

- **OIDC token replay:** mitigated by `nonce` (random 32-byte, stored in Redis with 10-min TTL, verified on callback) and short token lifetime (1 hour access + 24 hour refresh, per 20-Security §8.1 mitigation "OIDC token replay").
- **PKCE required for all public clients** (mobile, SPA). Confidential clients (server-side admin web) may omit PKCE.
- **State parameter required** — 32-byte random, stored in Redis, single-use.
- **JWKS rotation:** cached JWKS refreshed every 1 hour; if a new `kid` appears, fetched on demand.
- **SAML signature required** — unsigned SAML Responses are rejected.
- **SAML NotOnOrAfter enforced** with 60-second clock-skew tolerance.
- **Provider registration requires `admin.integrations` permission** and is logged in `audit_logs` with `action = 'sso.provider.registered'`.
- **Provider removal revokes all sessions** for users who logged in via that provider.

#### 17.5.6 Acceptance criteria (V3 GA)

- [ ] OIDC login works end-to-end with Okta, Azure AD, Auth0, Keycloak (per the original §17.5 acceptance).
- [ ] SAML 2.0 login works end-to-end with Azure AD, OneLogin, Ping.
- [ ] JIT provisioning creates a new user with the `default_role` on first login; subsequent logins link to the same `user_identities` row.
- [ ] Group claim mapping correctly assigns roles (e.g., `groups: ['medicare-doctors']` → `role: 'doctor'`).
- [ ] Provider removal revokes all sessions in < 1 minute.
- [ ] SCIM v2 (V3.1) creates, updates, and deletes users in Medicare when the IdP sends a SCIM request.
- [ ] Clock skew of ±5 minutes is tolerated on `exp` and `NotOnOrAfter`.
- [ ] All SSO events emit `audit_logs` rows (`sso.login`, `sso.logout`, `sso.jit_provision`, `sso.provider.registered`).

#### 17.5.7 What this module is NOT

- Not an IdP. We do not issue identities to other apps.
- Not a federation hub. Each tenant has its own IdP; we do not broker trust between tenants.
- Not an OIDC client library. We use `lcobucci/jwt` for token verification and `web-token/jwt-framework` for SAML XML processing.

### 17.6 MFA enforcement (`mfa-enforcement`, V2)

Per-tenant policy: `tenants.mfa_policy` enum:
- `off` — MFA not required (V1 default).
- `optional` — user can opt in.
- `required_for_staff` — staff/admin must enroll; patients optional.
- `required_for_all` — every user must enroll before first login.

Enforcement on login:
1. Password/OTP verified → issue a short-lived (5 min) `mfa_pending_token`.
2. Client posts to `POST /auth/mfa/verify` with `{mfa_pending_token, totp_code}` or WebAuthn assertion.
3. If valid, issue the real Sanctum token.

Backup codes: 10 single-use codes generated at MFA enrollment, hashed in `mfa_credentials.credential`.

### 17.7 Passkeys / WebAuthn (`passkeys`, V2)

`/auth/passkey/register` returns a challenge; client signs with platform authenticator (TouchID, Windows Hello, hardware key); API stores the public key in `mfa_credentials` with `type='webauthn'`. Login: same challenge-response, passwordless.

### 17.8 API additions (V2)

```
POST   /api/v2/auth/abha/initiate       { abha_number }                 -> { transaction_id, otp_sent }
POST   /api/v2/auth/abha/verify         { transaction_id, otp }         -> { user, token, identities[] }
POST   /api/v2/auth/oidc/{slug}/initiate                                 -> 302 to IdP
GET    /api/v2/auth/oidc/{slug}/callback  ?code&state                   -> { user, token }
POST   /api/v2/auth/mfa/enroll           { type, credential }            -> { mfa_credential }
POST   /api/v2/auth/mfa/verify           { mfa_pending_token, code }     -> { user, token }
POST   /api/v2/auth/passkey/register-challenge                          -> { challenge, rp_id }
POST   /api/v2/auth/passkey/register      { attestation }                -> { mfa_credential }
POST   /api/v2/auth/passkey/auth-challenge                              -> { challenge }
POST   /api/v2/auth/passkey/authenticate  { assertion }                  -> { user, token }
GET    /api/v2/auth/identities                                          -> [identity]
DELETE /api/v2/auth/identities/{id}                                     -> 204
POST   /api/v2/auth/identities/link       { provider, code }             -> identity (account merge)
```

### 17.9 V2 acceptance criteria

- [ ] ABDM/ABHA login round-trips end-to-end against NHA sandbox.
- [ ] OIDC login works with Okta, Azure AD, Keycloak, Auth0.
- [ ] User can have ≥ 3 federated identities linked; can unlink any.
- [ ] MFA enrollment produces valid TOTP and WebAuthn credentials; backup codes work; can re-enroll.
- [ ] Required-for-staff tenants cannot have staff log in without MFA.
- [ ] Passkey login is passwordless, < 2s end-to-end.
- [ ] Account merge (linking a second identity) is auditable and reversible.
- [ ] All V2 auth flows emit `audit_logs` rows.
- [ ] Tokens issued via V2 flows are still Sanctum tokens, same `Authorization: Bearer` scheme, no client change needed.
