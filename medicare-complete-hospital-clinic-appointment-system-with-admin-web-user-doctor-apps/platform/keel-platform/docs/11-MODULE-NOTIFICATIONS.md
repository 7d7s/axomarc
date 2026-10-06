# 11 — Notifications Module (FCM Push, Email, In-App, Templates, Cron)

> The voice of the system. Reaches users on every channel: phone push, email, in-app banner, SMS (V2). Every state change that affects a user funnels through this module.

---

## 1. Scope

| In Scope | Out of Scope |
|---|---|
| FCM push to mobile apps (Android + iOS via Firebase) | WhatsApp Business (V2) |
| Transactional email via SMTP | Marketing email blasts (V2) |
| In-app notification center | SMS (V2; OTP only for V1 via Firebase) |
| Notification templates (DB-driven, i18n) | Voice calls (V2) |
| Per-tenant template overrides | |
| Channel preference (user can opt out of email, not push) | |
| Cron-triggered reminders (24h, 2h before appt) | |
| Audit-logged send history | |
| Multi-channel fan-out (push + email + in-app in one go) | |

---

## 2. Channels

| Channel | Provider | Reach | Opt-out? |
|---|---|---|---|
| `push` | Firebase Cloud Messaging (FCM) | Mobile apps (Android + iOS) | No (system-critical) |
| `email` | SMTP (Mailgun/SES/Postmark) | Web + email clients | Yes (per template) |
| `in_app` | In-app notification center + Reverb | Web admin, patient web | No |
| `sms` | Twilio (V2) | Phones | Yes (V2) |
| `whatsapp` | WhatsApp Business API (V2) | WhatsApp users | Yes (V2) |

A single notification is sent on **multiple channels simultaneously** based on:
- User's channel preferences (per template).
- Tenant config (e.g., a clinic may disable email for non-critical notifications).

---

## 3. Data Model

### 3.1 `notification_templates` (per-tenant)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK nullable | NULL = global system template |
| `key` | varchar(80) | `appointment_confirmed`, `prescription_ready`, etc. — see §5 |
| `channel` | enum('push','email','in_app') | One template per channel per key |
| `language_id` | bigint FK | Translations stored separately |
| `subject` | varchar(255) nullable | For email |
| `title` | varchar(120) | For push and in-app |
| `body` | text | With `{{placeholders}}` |
| `action_url` | varchar(500) nullable | Deep link or web URL |
| `action_label` | varchar(60) nullable | "View Appointment" |
| `icon_path` | varchar(500) nullable | Push icon |
| `is_active` | boolean default true | |
| `created_at`, `updated_at` | timestamps |

Unique: `(tenant_id, key, channel, language_id)` — first non-null tenant wins; fallback to global.

### 3.2 `notification_template_translations`

For non-default languages. Same columns as above but per `(template_id, language_id)`.

### 3.3 `notifications` (per-tenant, sent log)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `uuid` | char(36) unique | |
| `user_id` | bigint FK | Recipient |
| `template_key` | varchar(80) | |
| `channel` | enum('push','email','in_app') | |
| `language_id` | bigint FK | Used language |
| `subject` | varchar(255) nullable | Rendered |
| `title` | varchar(255) | Rendered |
| `body` | text | Rendered |
| `data` | json | Placeholders used + any extras |
| `action_url` | varchar(500) nullable | |
| `status` | enum('queued','sending','sent','delivered','failed','read') | |
| `sent_at` | timestamp nullable | |
| `delivered_at` | timestamp nullable | FCM delivery receipt |
| `read_at` | timestamp nullable | When user opened the notification |
| `failed_at` | timestamp nullable | |
| `failure_reason` | varchar(255) nullable | |
| `provider_response` | json | FCM/SMTP response |
| `fcm_token_id` | bigint FK nullable | Which device |
| `email_message_id` | varchar(120) nullable | SMTP message id |
| `related_type` | varchar(60) nullable | `appointment`, `prescription`, etc. |
| `related_id` | bigint nullable | |
| `group_key` | varchar(80) nullable | For collapsing multiple into one |
| `created_at`, `updated_at` | timestamps |

Indexes: `(user_id, created_at desc)`, `(status, created_at)`, `(related_type, related_id)`, `(group_key)`.

### 3.4 `notification_preferences` (per user)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `user_id` | bigint FK | |
| `template_key` | varchar(80) | Or `*` for global default |
| `channel` | enum('push','email','in_app') | |
| `enabled` | boolean default true | |

If no row exists for `(user, template, channel)`, defaults to `enabled=true` (except email, which defaults to user's `email_opt_in` setting).

### 3.5 `fcm_tokens` (already in 02-AUTH)

| Column | Type |
|---|---|
| `id` | bigint PK |
| `user_id` | bigint FK |
| `token` | varchar(500) unique |
| `device_type` | enum('ios','android','web') |
| `device_id` | varchar(120) nullable |
| `app_version` | varchar(20) nullable |
| `last_seen_at` | timestamp |
| `is_active` | boolean default true |

When a token returns `NotRegistered` or `InvalidRegistration` from FCM, mark `is_active=false`.

### 3.6 `email_suppression_list`

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `email` | varchar(180) |
| `reason` | enum('bounce','complaint','unsubscribe','manual') |
| `suppressed_at` | timestamp |

If an email is in this list, the dispatcher skips the send.

### 3.7 `cron_jobs` (job tracking, not a real cron table)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `name` | varchar(80) unique |
| `last_run_at` | timestamp nullable |
| `last_status` | enum('success','failed') |
| `last_error` | text nullable |
| `next_run_at` | timestamp nullable |
| `is_enabled` | boolean default true |

For visibility in the admin panel.

---

## 4. Dispatcher

### 4.1 `NotificationDispatcher` Service

```php
namespace App\Modules\Notifications\Services;

class NotificationDispatcher {
    public const VERSION = '1.0.0';

    public function send(User $user, string $templateKey, array $data = [], array $opts = []): void;
    public function sendToMany(Collection $users, string $templateKey, array $data = [], array $opts = []): void;
    public function markRead(Notification $n, User $user): void;
    public function markAllRead(User $user): int;
    public function unreadCount(User $user): int;
}
```

### 4.2 The Send Pipeline

```
NotificationDispatcher::send($user, 'appointment_confirmed', [...])
    │
    ├─► Resolve user preferences
    │   └─► for each enabled channel:
    │       ├─► push: queue PushNotificationJob
    │       ├─► email: queue EmailNotificationJob
    │       └─► in_app: insert Notification row (status=queued) → broadcast via Reverb
    │
    └─► For each channel:
        ├─► Render template (per user.language)
        ├─► Create Notification row (status=queued)
        ├─► Dispatch job to queue
        └─► Job updates status to sent/failed
```

### 4.3 Send to Many (Batched)

For "all staff got a new appointment" notifications, we batch into a single queue job that iterates and calls `send()` per user, with rate limiting at 100 messages/sec to avoid FCM throttling.

### 4.4 Grouping (Collapse Multiple)

If the user receives 5 appointment confirmations in 5 minutes, we don't want 5 separate push notifications. The `group_key` field collapses them. e.g., `group_key=appointment.{id}` means subsequent notifications with the same key update the existing push (`notification.updates` API call) instead of creating a new one.

---

## 5. Template Keys (Built-in)

| Key | Default Trigger | Channels | Placeholders |
|---|---|---|---|
| `appointment_booked` | Appointment created (pending) | push, email, in_app | patient_name, doctor_name, clinic_name, date, time |
| `appointment_confirmed` | Payment success | push, email, in_app | patient_name, doctor_name, date, time, appointment_url |
| `appointment_reminder_24h` | Cron 24h before | push, email | patient_name, doctor_name, date, time, clinic_address |
| `appointment_reminder_2h` | Cron 2h before | push | patient_name, doctor_name, time, clinic_address |
| `appointment_checked_in` | Patient arrives | push, in_app | patient_name, doctor_name, queue_token |
| `appointment_called` | Front-desk calls | push | patient_name, doctor_name, room, queue_token |
| `appointment_cancelled` | Status → cancelled | push, email | patient_name, doctor_name, date, refund_amount? |
| `cancellation_requested` | User requests cancel | email, in_app | admin_name, patient_name, doctor_name, date |
| `cancellation_approved` | Admin approves | push, email | patient_name, doctor_name, refund_amount |
| `cancellation_rejected` | Admin rejects | push, email | patient_name, doctor_name, reason |
| `appointment_rescheduled` | Status reschedule | push, email | patient_name, doctor_name, old_date, new_date |
| `no_show_marked` | Cron | push | patient_name, doctor_name, date |
| `prescription_created` | Doctor saves Rx draft | in_app | patient_name, doctor_name |
| `prescription_signed` | Doctor signs | push, email, in_app | patient_name, doctor_name, rx_url |
| `prescription_shared` | Patient shares | email | sender_name, recipient, rx_url |
| `payment_received` | Webhook success | push, email, in_app | patient_name, amount, invoice_url |
| `payment_failed` | Webhook failed | push, email | patient_name, amount, retry_url |
| `refund_processed` | Refund success | push, email | patient_name, amount, refund_id |
| `invoice_overdue` | Cron | email | patient_name, amount, due_date, invoice_url |
| `video_call_started` | Doctor starts Zoom | push | patient_name, doctor_name, join_url |
| `video_call_invite` | Appointment time | push, email | patient_name, doctor_name, join_url |
| `welcome_patient` | First OTP login | email | patient_name, login_url |
| `welcome_doctor` | Admin creates doctor | email | doctor_name, temp_password, login_url |
| `password_reset` | Reset request | email | name, reset_url, expires_in |
| `doctor_new_review` | Patient leaves review | push, in_app | doctor_name, rating, review_url |
| `testimonial_approved` | Admin approves testimonial | email | name |
| `appointment_follow_up` | Doctor recommends follow-up | push, email | patient_name, doctor_name, follow_up_date, book_url |
| `vitals_abnormal` | Patient records abnormal vital | push, in_app | patient_name, metric, value, doctor_name? |
| `family_member_added` | Linked to user | push, in_app | user_name, patient_name, relation |
| `account_suspended` | Admin suspends | email | name, reason |
| `role_assigned` | Role granted | in_app | name, role, permissions_count |

---

## 6. Template Rendering

### 6.1 Placeholders

Templates use `{{variable}}` Mustache-style. Variables are escaped for HTML. Examples:

Email body for `appointment_confirmed`:
```html
<p>Hi {{patient_name}},</p>
<p>Your appointment with <strong>{{doctor_name}}</strong> at <strong>{{clinic_name}}</strong> is confirmed.</p>
<p><strong>Date:</strong> {{formatted_date}}<br>
<strong>Time:</strong> {{formatted_time}}</p>
<p><a href="{{appointment_url}}" class="btn">View Appointment</a></p>
```

Push body for same:
```
Appointment confirmed with {{doctor_name}} on {{formatted_date}} at {{formatted_time}}
```

### 6.2 Placeholder Resolution

A `PlaceholderResolver` service takes the template + data dict and returns rendered strings per channel. Missing placeholders → empty string + warn-logged.

Common helpers available in any template:
- `{{patient_name}}`, `{{patient_first_name}}`
- `{{doctor_name}}`, `{{clinic_name}}`
- `{{date}}`, `{{time}}`, `{{formatted_date}}`, `{{formatted_time}}` (in clinic timezone)
- `{{amount}}`, `{{currency}}`, `{{formatted_amount}}`
- `{{appointment_url}}`, `{{rx_url}}`, `{{invoice_url}}` — deep links
- `{{unsubscribe_url}}` — email only

### 6.3 Language Resolution

On send:
1. Try `notification_templates` where `(tenant_id, key, channel, language_id=user.language_id)`.
2. Fallback to tenant default language.
3. Fallback to global template (tenant_id=NULL) in user language.
4. Fallback to global template in English.
5. Last resort: log "template not found" + skip send.

---

## 7. FCM Integration

### 7.1 Setup

- Firebase project: `medicare-prod`, `medicare-staging`, etc.
- `google-services.json` in Flutter Android app.
- `GoogleService-Info.plist` in Flutter iOS app.
- Service account JSON in API `.env`.

### 7.2 Laravel Side

Use `kreait/laravel-firebase` package.

```php
class PushSender {
    public function __construct(private FirebaseMessaging $fm) {}
    
    public function send(FcmToken $token, string $title, string $body, array $data, array $opts = []): PushResult {
        $message = CloudMessage::withTarget('token', $token->token)
            ->withNotification(Notification::create($title, $body))
            ->withData($data);
        try {
            $report = $this->fm->send($message);
            return new PushResult(success: true, messageId: $report->messageId());
        } catch (MessagingException $e) {
            if (in_array($e->errorCode, ['NotRegistered', 'InvalidRegistration'])) {
                $token->update(['is_active' => false]);
            }
            return new PushResult(success: false, error: $e->errorCode);
        }
    }
}
```

### 7.3 Android-Specific Options

- `priority=high` for time-sensitive (reminders, calls).
- `priority=normal` for informational.
- `ttl=3600` for 1-hour window.
- `collapse_key` to dedupe at FCM level.
- `notification.sound=default`.

### 7.4 iOS-Specific Options

- `aps.alert` with title/body.
- `aps.badge` increments.
- `aps.sound=default`.
- `aps.content-available=1` for silent (background) pushes when needed.
- `aps.category=APPOINTMENT_ACTION` for interactive (Accept/Reschedule buttons).

---

## 8. Email Integration

### 8.1 Setup

SMTP via `MAIL_*` env. Recommended: Mailgun, SES, or Postmark for deliverability. We **never** use the default `log` driver in production.

### 8.2 Mailable Classes

`app/Modules/Notifications/Mail/GenericNotificationMail.php`:

```php
class GenericNotificationMail extends Mailable {
    public function __construct(
        public string $subject,
        public string $bodyHtml,
        public string $bodyText,
        public array $data,
    ) {}
    
    public function build(): self {
        return $this->subject($this->subject)
            ->view('emails.generic', ['bodyHtml' => $this->bodyHtml])
            ->text('emails.generic-text', ['bodyText' => $this->bodyText])
            ->withSwiftMessage(function ($msg) {
                $msg->getHeaders()->addTextHeader('X-Mailer', 'Medicare');
            });
    }
}
```

### 8.3 Layouts

- `emails.layouts.html` — HTML master
- `emails.layouts.text` — plaintext master
- Both include clinic logo, footer, unsubscribe link.

### 8.4 Deliverability

- SPF + DKIM + DMARC records (set up in DNS by DevOps).
- Custom `From` per tenant: `no-reply@{tenant-slug}.medicare.app`.
- Reply-to: `support@medicare.app` (centralized).
- `List-Unsubscribe` header in every email.
- Auto-handle bounces via SES/Mailgun webhooks → `email_suppression_list`.

---

## 9. In-App Notifications

### 9.1 Storage

A `Notification` row with `channel=in_app` and `status=sent`. Queryable via API.

### 9.2 Realtime

When a new in-app notification is created, broadcast on `user.{id}.notifications` private Reverb channel. Web and mobile apps listen; on receive:
- Web: toast + bell icon counter increment
- Mobile: same as push, but no sound

### 9.3 Notification Center UI

- Bell icon in admin/patient-web header.
- Dropdown shows last 10 with "View all" link.
- Full page: paginated list with filter by type, mark all as read.
- Each item: icon, title, body preview, timestamp, action button.

---

## 10. The Queue Jobs

### 10.1 `SendPushNotificationJob`

```php
class SendPushNotificationJob implements ShouldQueue {
    use TenantAware;
    
    public int $tries = 3;
    public int $backoff = 30;     // 30s, 2m, 10m
    public int $timeout = 30;
    
    public function __construct(public int $notificationId) {}
    
    public function handle(PushSender $sender): void {
        $n = Notification::findOrFail($this->notificationId);
        if ($n->status !== 'queued') return;
        $n->update(['status' => 'sending']);
        
        $tokens = $n->user->fcmTokens()->active()->get();
        foreach ($tokens as $token) {
            $result = $sender->send($token, $n->title, $n->body, $n->data, $n->opts);
            // Update $n with result; first success moves status to 'sent'
        }
        $n->update(['status' => 'sent', 'sent_at' => now()]);
    }
}
```

### 10.2 `SendEmailNotificationJob`

Same pattern, calls `Mail::send()` and updates with the SMTP message id.

### 10.3 Retry & DLQ

- `tries=3` with exponential backoff.
- After final failure, mark `status=failed` and increment a metric. A weekly job reports any templates with >5% failure rate to the team.

---

## 11. Cron Jobs

| Job | Schedule | Purpose |
|---|---|---|
| `AppointmentReminder24hJob` | hourly | Find confirmed appointments in next 24h with no 24h reminder sent → send `appointment_reminder_24h` |
| `AppointmentReminder2hJob` | every 15 min | Same for 2h window |
| `AppointmentFollowUpJob` | daily 09:00 | Send follow-up survey 1 day after `completed` (V2) |
| `InvoiceOverdueReminderJob` | daily 09:00 | Unpaid invoices past `due_at` → email |
| `SubscriptionExpiryReminderJob` | daily (V2) | V2 |
| `TrialEndingReminderJob` | daily (V2) | V2 |
| `InactiveUserNudgeJob` | weekly | V2 re-engagement (off by default) |
| `StaleNotificationCleanupJob` | daily 03:00 | Delete `read` in-app notifications older than 90 days |

---

## 12. User Preferences API

```
GET    /api/v1/me/notification-preferences
PUT    /api/v1/me/notification-preferences
       {
         "preferences": [
           { "template_key": "appointment_reminder_24h", "channel": "email", "enabled": false },
           ...
         ]
       }
POST   /api/v1/me/unsubscribe/{token}    (from email footer; one-click)
```

The default preferences are seeded on user creation. Users can mute non-critical channels per template.

---

## 13. Admin Visibility

### 13.1 Notification Log

```
GET /api/v1/admin/notifications?user_id=&template_key=&channel=&status=&from=&to=&page=
```

Shows every send, with delivery status, open/click tracking, retries. Critical for debugging "user says they didn't get the email" complaints.

### 13.2 Cron Health

```
GET /api/v1/admin/cron-jobs
```

Shows the `cron_jobs` table — last run, status, next run. A red flag here means the scheduler is broken.

### 13.3 Template Editor

Admin can edit any template via `PUT /admin/notification-templates/{id}`. Live preview with sample data. Versioning: every save creates a new version row; admins can rollback to any previous version.

---

## 14. API Endpoints Summary

```
# Send (internal, called by other modules)
POST   /internal/notifications/send                  (service-to-service)
POST   /internal/notifications/send-to-many

# User-facing
GET    /api/v1/me/notifications?unread=&page=
POST   /api/v1/me/notifications/{id}/read
POST   /api/v1/me/notifications/read-all
GET    /api/v1/me/notifications/unread-count          (Reverb-friendly)
GET    /api/v1/me/notification-preferences
PUT    /api/v1/me/notification-preferences

# Admin
GET    /api/v1/admin/notifications
GET    /api/v1/admin/notifications/{id}
GET    /api/v1/admin/notification-templates
POST   /api/v1/admin/notification-templates
PUT    /api/v1/admin/notification-templates/{id}
POST   /api/v1/admin/notification-templates/{id}/preview   { sample_data: {} }
GET    /api/v1/admin/notification-templates/{id}/versions
POST   /api/v1/admin/notification-templates/{id}/rollback { version_id }
GET    /api/v1/admin/cron-jobs
POST   /api/v1/admin/cron-jobs/{name}/run-now           (manual trigger)
GET    /api/v1/admin/email-suppressions
DELETE /api/v1/admin/email-suppressions/{id}
```

---

## 15. Validation

```php
// SendNotificationRequest (internal)
public function rules(): array {
    return [
        'user_id'      => ['required_without:user_ids', 'exists:users,id'],
        'user_ids'     => ['required_without:user_id', 'array'],
        'user_ids.*'   => ['exists:users,id'],
        'template_key' => ['required', 'string', 'max:80'],
        'data'         => ['required', 'array'],
        'channels'     => ['nullable', 'array'],
        'channels.*'   => ['in:push,email,in_app'],
        'group_key'    => ['nullable', 'string', 'max:80'],
    ];
}
```

---

## 16. Realtime Events

| Event | Channel | Payload |
|---|---|---|
| `NotificationCreated` | `user.{id}` | full notification object |
| `NotificationRead` | `user.{id}` (other devices) | { id } for badge sync |
| `UnreadCountChanged` | `user.{id}` | { count } |

---

## 17. Audit

`Notification::created/updated` is audited when status changes. Admin can see who triggered the send, who received, and the full delivery chain. Required for GDPR — user can request "all notifications you ever sent me".

---

## 18. Testing Strategy

| Test | Assertion |
|---|---|
| `TemplateRenderTest` | All placeholders substituted, HTML escaped |
| `TemplateFallbackTest` | Tenant override → global → English chain |
| `PushSendTest` | Mock FCM, verify message format, token deactivated on NotRegistered |
| `EmailSendTest` | Mock SMTP, verify headers, List-Unsubscribe present |
| `IdempotentCronTest` | 24h reminder sent once even if cron runs twice |
| `InAppBroadcastTest` | Reverb receives event with right payload |
| `PreferenceTest` | Disabled channel → no send |
| `BounceHandlerTest` | SES bounce webhook → email added to suppression list |
| `TranslationTest` | User language = 'hi' → Hindi template used |
| `GroupKeyTest` | Multiple sends with same group_key collapse into one push |
| `BulkSendRateLimitTest` | 1000 users → queued, processed at 100/sec |

---

## 19. Acceptance Criteria (V1)

- [ ] Doctor receives FCM within 5 seconds of a new appointment.
- [ ] Patient receives FCM + email on appointment confirmation.
- [ ] 24h reminder is sent exactly once (idempotent cron).
- [ ] 2h reminder is sent exactly once.
- [ ] User can disable email per template; push cannot be disabled.
- [ ] User receives in-app notification within 1 second of event (Reverb).
- [ ] Bounced email is auto-added to suppression list.
- [ ] All 31 template keys ship with at least English content.
- [ ] Template editor previews with sample data before saving.
- [ ] Admin notification log shows every send with delivery status.
- [ ] Switching user language from English to Hindi shows Hindi templates within 60 seconds.
- [ ] Email contains working unsubscribe link.

---

## 20. V2 Roadmap — Notifications

> V1 ships FCM (push) + SMTP (email) + in-app (Reverb). V2 adds WhatsApp Business, Twilio SMS, voice calls, in-app chat, push categories, and an AI-generated notification copy service.

### 20.1 WhatsApp Business (`whatsapp-business`, V1.1, MML 4) — deep-dive

**1. Overview.** V1 ships FCM (push) + SMTP (email) + in-app (Reverb) for transactional and engagement notifications. V1.1 adds **WhatsApp Business** as a first-class notification channel via the **Meta WhatsApp Cloud API** (v18.0, the post-On-Prem-API hosted variant). WhatsApp is the dominant messaging channel in India, Brazil, Mexico, Indonesia, Nigeria, and most of LATAM + South Asia; the engagement and deliverability are 2-5x better than SMS in those markets. MML 4 GA covers: **template management** (author → submit to Meta → approved → use), **outbound sends** (template + free-form within 24h window), **inbound webhook** (text, image, document, audio, location, contact, button reply, list reply), **opt-in / opt-out** management, the **24-hour customer service window** enforcement, **deliverability** (sent / delivered / read / failed), **Meta rate limits** (80 MPS default, 1k MPS Enterprise), and the **conversation pricing** (Meta charges per conversation, not per message). The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.1 lane module. Per the existing stub, V1.1 ships the template creation UI + the runtime sender; the marketing template categories and the catalog-messages path are V2+.

**2. The Meta WhatsApp Cloud API.** Meta offers two ways to use WhatsApp Business:
- **On-Premises API** (deprecated Sept 2025; not supported): businesses self-host the WhatsApp Business API client.
- **Cloud API** (the only path now): Meta hosts the API; we make HTTPS calls to `graph.facebook.com/v18.0/{phone_number_id}/messages` with a system-user access token.

The `whatsapp-business` module is Cloud API only. The setup:
- A **Meta Business Account** (one per tenant) is created or linked.
- A **WhatsApp Business Account (WABA)** is created under the Meta Business Account.
- A **phone number** is registered with the WABA (the number is dedicated to WhatsApp; cannot be used for SMS or voice).
- A **system user** is created with `whatsapp_business_management` and `whatsapp_business_messaging` permissions.
- The system user's access token is stored in AWS Secrets Manager.
- The display name, profile picture, and category are set (e.g., "Acme Hospital — Medical & Health").

**3. Templates.** WhatsApp templates are pre-approved message formats that you can send to a user **outside** the 24h customer service window. Each template has:
- **Name:** a slug, e.g., `appointment_reminder_v1`.
- **Language:** `en`, `en_US`, `hi_IN`, `pt_BR`, `es_MX`, etc. (60+ supported).
- **Category:** `marketing` (promotions, offers), `utility` (transactional: appointment confirmations, payment receipts), `authentication` (OTP).
- **Components:** header (text / image / document / video), body (text with `{{1}}`, `{{2}}` placeholders), footer (text), buttons (URL, phone, quick reply, copy code, dynamic URL with placeholder).
- **Sample values:** e.g., `{1: 'Dr. Jane', 2: 'Tue 6 PM'}` for the preview.

The template lifecycle:
1. Admin authors a template in the V1.1 UI (`/admin/notifications/templates/whatsapp`).
2. Admin submits to Meta via `POST /{waba_id}/message_templates` with the components.
3. Meta reviews (1 minute for utility, up to 24 hours for marketing).
4. Meta returns a `status` of `APPROVED`, `REJECTED`, or `PENDING` (with reason). Approved templates can be sent.
5. We mirror the status in `notification_templates.whatsapp_status` and surface it in the UI.

**4. The 5 message types.**
- **Template** — pre-approved; used outside 24h window; required for first contact.
- **Text (free-form)** — within 24h window only; plain text, up to 4096 chars.
- **Image / Video / Document / Audio** — within 24h window; media is uploaded to Meta first (`POST /{app_id}/uploads` with the binary), then sent by `media_id`.
- **Interactive** — within 24h window; button message (up to 3 quick-reply buttons), list message (up to 10 sections, 10 rows), product message, product list, CTAs (URL button, phone button, copy code button).
- **Location / Contact** — within 24h window; user shares location or contact; useful for "share your address" or "save our contact".

**5. The send flow (outbound, template).**
1. Upstream event (e.g., `appointment.reminder` 24h before) → `NotificationService::send(channel: 'whatsapp', to: phone, template: 'appointment_reminder_v1', data: {1: 'Dr. Jane', 2: 'Tue 6 PM'})`.
2. The `WhatsAppService::sendTemplate()` checks the conversation: if `opted_in=false`, the send is **skipped** (Meta rejects opt-out messages anyway, and we don't want to charge a conversation for them).
3. The service calls Meta's `POST /{phone_number_id}/messages` with `{ messaging_product: 'whatsapp', to, type: 'template', template: { name, language: { code }, components: [...] } }`. Meta returns `{ messages: [{ id: 'wamid.xxx' }], contacts: [{ wa_id, profile }] }`.
4. We write the `whatsapp_messages` row with `direction='outbound'`, `type='template'`, `whatsapp_message_id=wamid.xxx`, `status='sent'`, and the rendered payload.
5. Meta delivers the message; the status callback fires (`sent` → `delivered` → `read`); we update the row's `status` and the `whatsapp_conversations.last_message_at`.
6. A **conversation** is opened with Meta; the conversation is billed per Meta's pricing tier (see §11).

**6. The send flow (outbound, free-form within 24h).**
1. Doctor replies to a patient message → `NotificationService::send(channel: 'whatsapp', to: phone, type: 'text', body: '...')`.
2. The `WhatsAppService::sendFreeForm()` checks the conversation's `last_message_at` (the user's last inbound message). If `now() - last_message_at < 24h`, send as free-form. Otherwise, **fall back** to a template (e.g., `conversation_recovery_v1`).
3. Send as `text`; rest is the same as template.

**7. The inbound webhook.** Meta sends a webhook to `POST /api/webhooks/whatsapp` for every inbound message, status change, or template status update. The webhook payload:
```json
{
  "object": "whatsapp_business_account",
  "entry": [{
    "id": "WABA_ID",
    "changes": [{
      "value": {
        "messaging_product": "whatsapp",
        "metadata": { "display_phone_number": "...", "phone_number_id": "..." },
        "contacts": [{ "profile": { "name": "..." }, "wa_id": "91xxxxxxxxxx" }],
        "messages": [{ "from": "91xxxxxxxxxx", "id": "wamid.xxx", "timestamp": "...", "type": "text", "text": { "body": "Hi" } }]
      },
      "field": "messages"
    }]
  }]
}
```
The API:
1. Verifies the `X-Hub-Signature-256` HMAC against `META_APP_SECRET`.
2. Looks up the conversation by `phone_number_id` + `from`. If not found, creates a new one.
3. Writes the `whatsapp_messages` row with `direction='inbound'`.
4. If the message is a button reply (`type='interactive'`, `button_reply` or `list_reply`), maps it to the in-app chat's reply flow.
5. If the conversation is unlinked to a user (the phone is unknown), the message is **stored** and a "New conversation from unknown phone" alert is sent to the admin.
6. The in-app chat UI (per `11-Notif §20.4`) opens the conversation; the doctor's reply is sent back as free-form (or template, if the 24h window has closed).

**8. The 24-hour customer service window.** Meta's most confusing rule: free-form replies are only allowed within 24h of the user's **last inbound** message. Outside that window, every outbound must be a pre-approved template. The `WhatsAppService` enforces this:
- If `now() - conversations.last_inbound_at > 24h` and the send is free-form, **fall back** to a template (with an audit row explaining the fallback).
- If the send is a template, no window check needed.
- The first contact (no inbound ever) **always** requires a template (Meta rejects free-form to a user who has never messaged us).

**9. The opt-in / opt-out flow.**
- **Opt-in:** the patient must opt in to receive WhatsApp messages. The opt-in is captured in the app (`/profile/notification-preferences` → "WhatsApp" toggle), or in the WhatsApp conversation itself (the patient sends a "START" message), or via a one-time opt-in template (the first send is `optin_template_v1` with a "I agree" button; the patient taps; we receive a `button_reply`; we mark `opted_in=true`). The opt-in is stored in `whatsapp_conversations.opted_in` and `opted_in_at`.
- **Opt-out:** the patient can opt out by sending "STOP", by clicking an opt-out link, or by toggling the app preference. The opt-out is stored in `opted_out_at`; subsequent sends are skipped at the service layer.
- **Meta's quality rating:** Meta tracks the quality of our templates (user reports, blocks). A low rating (red) reduces our daily limit; a banned template is removed.

**10. The 7 message statuses (deliverability).**
- `accepted` — Meta accepted the message (queued for delivery).
- `sent` — Meta handed off to the user's device (or to the carrier).
- `delivered` — the device confirmed receipt (the user has the WhatsApp app open and online).
- `read` — the user opened the chat (requires read receipts to be on).
- `failed` — Meta could not deliver (invalid phone, opt-out, template not approved).
- `deleted` — the user deleted the message from their chat (we are notified; rare).
- `reaction` — the user reacted to a message with an emoji (we are notified; not a status of the original message).

**11. The conversation pricing.** Meta charges per **conversation**, not per message. A conversation is a 24h window during which unlimited messages can be exchanged. Categories and pricing (2026, indicative):
- **Utility** (transactional: appointment, payment, prescription): $0.0042 / conversation (India), $0.005 / conversation (US), $0.0125 / conversation (Brazil).
- **Marketing** (promotions, offers): $0.0250 / conversation (India), $0.0250 / conversation (US), $0.0450 / conversation (Brazil).
- **Authentication** (OTP): $0.0025 / conversation (India), $0.0030 / conversation (US), $0.0100 / conversation (Brazil).
- **Service** (within 24h window, free-form replies): **free** for the first 1k / month; $0.0025 / conversation after that.

The `whatsapp_messages` table is updated with the cost; a nightly job rolls up the cost per tenant into `whatsapp_costs_by_tenant`.

**12. The Meta rate limits.** Default: 80 MPS (messages per second) per phone number; Enterprise tier (per Meta's verification): 1,000 MPS. The `SendWhatsAppTemplateJob` uses a per-tenant queue; a worker pool drains at the limit; excess is retried with backoff. The queue also respects Meta's per-template quota (Meta assigns a daily quota per template; a template that exceeds the quota is paused for 24h).

**13. Domain Model.** (Builds on the existing stub at §20.1.) Adds:
```php
Schema::table('whatsapp_conversations', function (Blueprint $t) {
    $t->string('optin_template', 64)->nullable();              // the opt-in template name
    $t->timestamp('last_inbound_at')->nullable();              // for 24h window
    $t->timestamp('last_outbound_at')->nullable();
    $t->string('meta_conversation_id', 64)->nullable();        // Meta's conv_id
    $t->string('quality_rating', 16)->nullable();              // 'green' | 'yellow' | 'red' | 'banned'
    $t->integer('messages_inbound_count')->default(0);
    $t->integer('messages_outbound_count')->default(0);
    $t->jsonb('pricing_summary')->nullable();                  // last conversation cost
    $t->index(['last_inbound_at', 'opted_in']);
});

Schema::create('whatsapp_template_submissions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('notification_template_id')->nullable()->index();
    $t->string('name', 128);
    $t->string('language', 16);
    $t->string('category', 16);                                // 'utility' | 'marketing' | 'authentication'
    $t->jsonb('components');
    $t->string('meta_status', 16)->default('pending');         // 'pending' | 'approved' | 'rejected'
    $t->text('meta_rejection_reason')->nullable();
    $t->string('meta_template_id', 64)->nullable();
    $t->timestamp('submitted_at');
    $t->timestamp('approved_at')->nullable();
    $t->timestamps();
});

Schema::create('whatsapp_webhook_events', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('event_type', 32);                              // 'message' | 'status' | 'template_status' | 'account_update' | 'phone_number_update' | 'business_capability_update'
    $t->jsonb('payload');
    $t->boolean('processed')->default(false);
    $t->text('error')->nullable();
    $t->timestamp('received_at');
    $t->timestamps();
});
```

**14. API Surface.**
```
# Templates (admin)
GET    /api/v1.1/notifications/whatsapp/templates             -> [notification_template] (filtered to WA-enabled)
POST   /api/v1.1/notifications/whatsapp/templates             # { name, language, category, components, sample_values } -> notification_template (submits to Meta)
GET    /api/v1.1/notifications/whatsapp/templates/{id}/status -> { meta_status, rejection_reason, approved_at }
POST   /api/v1.1/notifications/whatsapp/templates/{id}/resubmit

# Conversations
GET    /api/v1.1/notifications/whatsapp/conversations        -> [conversation]
GET    /api/v1.1/notifications/whatsapp/conversations/{id}   -> conversation (with messages, opt-in status, 24h window status)
POST   /api/v1.1/notifications/whatsapp/conversations/{id}/opt-in
POST   /api/v1.1/notifications/whatsapp/conversations/{id}/opt-out
POST   /api/v1.1/notifications/whatsapp/conversations/{id}/send   # { template?, text?, media? } -> message

# Stats
GET    /api/v1.1/notifications/whatsapp/stats                -> { conversations_active, templates_approved, messages_sent_30d, messages_delivered_30d, messages_read_30d, cost_30d, quality_rating }

# Webhooks
POST   /api/webhooks/whatsapp                                 # inbound + status + template_status
GET    /api/webhooks/whatsapp                                 # Meta verification challenge (GET with hub.mode, hub.verify_token, hub.challenge)
```

**15. Frontend.**
- **Admin:** `/admin/notifications/whatsapp` shows: WABA + phone number status, quality rating, today's sends / delivered / read, the cost widget (today, MTD, projected), and the templates list (with status badges).
- **Template composer:** `/admin/notifications/templates/whatsapp/new` is a 4-step wizard: pick category → compose components (header / body / footer / buttons) → pick sample values → submit to Meta. The preview pane shows the rendered template with WhatsApp's exact styling. After submit, the status is polled every 30s until `approved` or `rejected`.
- **In-app chat integration:** the in-app chat UI (per `11-Notif §20.4`) shows WhatsApp conversations with a `transport='whatsapp'` badge. The doctor's reply is sent as free-form (within 24h) or template (outside 24h); the UI shows the window status.
- **Patient:** the patient's notification preferences page has a "WhatsApp" toggle (with the opt-in template preview).

**16. Realtime.**
- `whatsapp.message.received.{tenant_id}.{conversation_id}` — inbound message; in-app chat updates.
- `whatsapp.message.delivered.{tenant_id}.{message_id}` — delivery confirmed.
- `whatsapp.message.read.{tenant_id}.{message_id}` — read receipt.
- `whatsapp.template.approved.{tenant_id}.{template_id}` — template approved; can be used in sends.
- `whatsapp.template.rejected.{tenant_id}.{template_id}` — template rejected; admin notified.
- `whatsapp.quality.changed.{tenant_id}` — quality rating changed; if red, admin paged.

**17. Security & RBAC.**
- **Webhook verification:** every Meta webhook is verified with `X-Hub-Signature-256` HMAC SHA-256 against `META_APP_SECRET`; unverified webhooks return 401 and are logged in `whatsapp_webhook_events` with `processed=false`.
- **Opt-in mandatory:** a send to a non-opted-in conversation is skipped at the service layer; the audit log shows the skip.
- **PII:** the patient's phone number is stored in E.164; the conversation metadata may include the patient's WhatsApp profile name (which is set by the user, not verified).
- **Roles:** `tenant_admin` (configure WABA, submit templates, view stats), `notification_admin` (send ad-hoc messages, view conversations), `compliance_officer` (view opt-in / opt-out audit), `doctor` (reply to their patients' conversations within their scope).
- **Audit:** every template submission, every send, every opt-in / opt-out, every webhook is logged in `audit_logs`.
- **Rate limit per sender:** a doctor is limited to 100 messages / hour / patient to prevent spam; a tenant-wide limit is 10k / hour (configurable per tenant's plan).

**18. Acceptance Criteria.**
- [ ] A WABA can be linked; a phone number can be registered; the access token is stored in Secrets Manager.
- [ ] A template can be authored, submitted to Meta, approved, and used in a send.
- [ ] A template submission that is rejected is shown with the rejection reason; a resubmit creates a new submission.
- [ ] An outbound template send is delivered within 30 seconds; the status callback updates the row.
- [ ] An outbound free-form send is allowed only within 24h of the user's last inbound; outside the window, the service falls back to a template.
- [ ] An inbound message is processed; the conversation is updated; the in-app chat UI shows the message.
- [ ] The 24h window status is visible in the UI; the doctor's reply is gated accordingly.
- [ ] The opt-in / opt-out flow works: a non-opted-in patient does not receive a send; a opted-out patient does not receive a send; the audit log shows the skip.
- [ ] The cost is logged per message and per conversation; the monthly cost rollup is accurate.
- [ ] The Meta rate limit is respected (80 MPS default; the queue drains at that rate).
- [ ] The webhook HMAC verification rejects tampered payloads.
- [ ] The quality rating is displayed; a red rating opens a P2 incident.

**19. Real-World Test Scenarios.**
- *First contact:* clinic sends an `optin_template_v1` to a new patient; the patient taps "I agree"; the API records `opted_in=true`; the next send is allowed.
- *Appointment reminder:* scheduled job runs 24h before; the `appointment_reminder_v1` template is sent; the patient receives "Hi Priya, your appointment with Dr. Kumar is tomorrow at 10:00 AM. Reply YES to confirm."; Meta charges a `utility` conversation.
- *Patient replies:* patient replies "YES"; the API receives the inbound; the in-app chat opens the conversation; the doctor sees the YES; the doctor replies "Great, see you tomorrow"; the reply is sent as free-form (within 24h).
- *24h window expires:* 25 hours after the patient's last inbound, the doctor tries to reply with a free-form; the service falls back to `conversation_recovery_v1` template; the message is delivered.
- *Opt-out:* patient sends "STOP"; the API marks the conversation as opted out; subsequent sends are skipped.
- *Template rejection:* admin submits a marketing template with a "Buy now" CTA; Meta rejects (violates commerce policy); the admin edits and resubmits.
- *Quality rating drops:* the tenant's templates get a red rating (high block rate); Meta reduces the daily limit from 80 MPS to 20 MPS; the API surfaces the rating change; the admin is paged.
- *Inbound from unknown phone:* a random person sends "Hi" to the clinic's WhatsApp number; the API creates a conversation with no user; an alert is sent to the admin; the message is stored but no auto-reply.
- *Webhook tamper attempt:* attacker sends a fake inbound message with a bogus signature; the HMAC verification fails; the webhook is rejected; the audit log shows the failure.

**20. What this module is NOT:**
- Not a replacement for SMS. SMS is for OTP and for recipients without WhatsApp; WhatsApp is for engagement and rich media.
- Not a free channel. Meta charges per conversation; the cost is non-trivial at scale (especially marketing).
- Not a global one-size-fits-all. WhatsApp is dominant in 60+ countries; in the US, the penetration is lower (~30%); the channel is configured per tenant's primary market.
- Not a CRM. We do not store the full conversation history beyond 90 days (Meta's recommendation); for long-term storage, the in-app chat has its own history.

**21. Dev Workflow.** `php artisan whatsapp:send --tenant=... --to=+91xxxxxxxxxx --template=appointment_reminder_v1 --params='["Dr. Kumar","Tue 6 PM"]'` sends a test template. `php artisan whatsapp:webhook-test --payload=tests/fixtures/whatsapp-inbound.json` simulates a webhook. `php artisan whatsapp:reconcile --tenant=... --from=...` reconciles the local `whatsapp_messages` against Meta's message log. Code: `app-modules/whatsapp-business/`; tests: `app-modules/whatsapp-business/tests/Feature/{TemplateSubmit,Send,Inbound,OptIn,OptOut,Window,Webhook,Quality}Test.php` and `tests/Reconciliation/WhatsAppReconcileTest.php`. Sandbox: Meta WhatsApp Cloud API test phone number (`+1 555-0736-7174`); test templates provided by Meta; a `WhatsAppTestHelper` fakes the gateway in CI.

### 20.2 Twilio SMS (`twilio-sms`, V1.2, MML 4) — deep-dive

**1. Overview.** V1 ships FCM (push) + SMTP (email) + in-app (Reverb) for transactional and engagement notifications. V1.2 adds **Twilio Programmable SMS** as a first-class notification channel, for two distinct use cases:
- **OTP fallback** when Firebase Phone Auth is blocked (per `02-Auth §8` — e.g., India, China, parts of EU where Firebase is unreliable). The `twilio-verify` Flutter package is the drop-in; the API calls Twilio Verify Services API to send + check the OTP.
- **Transactional SMS** for patient engagement (appointment reminders, lab result ready, payment received, prescription dispatched) and for staff notifications (new appointment, cancellation, escalations).

The `twilio-sms` module ships both. MML 4 GA covers: outbound SMS (notification, OTP, marketing — opt-in), inbound SMS (RECEIVED webhook for two-way chat), short-code / long-code / toll-free number provisioning per region, A2P 10DLC registration for US, deliverability tracking (queued/sent/delivered/undelivered/failed), DLR (delivery receipt) webhooks, opt-out management (STOP / START / UNSUBSCRIBE keywords), and the `sms_messages` mirror table. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.2 lane module.

**2. Twilio phone numbers and the regulatory landscape.** Sending SMS in different countries requires local-number registration, sender-id registration, or both:
- **US:** A2P 10DLC (Application-to-Person 10-Digit Long Code). Brand + Campaign registration with The Campaign Registry (TCR); a Brand is the company (vetting, EIN, contact); a Campaign is the use case (e.g., "appointment reminders") with sample messages. Approval is 1-7 days; throughput is assigned per campaign (default 75 MPS). Mandatory since June 2024.
- **India:** DLT (Distributed Ledger Technology) registration per operator (Jio, Airtel, Vi, BSNL). Each template must be registered with a Template ID. Entity + Header + Template registration. Mandatory since 2021.
- **EU / UK:** Generally unrestricted, but some countries require sender-id registration (Germany, France, Italy). Long codes work in most; alphanumeric sender-id is restricted in some.
- **Other:** Country-specific; covered in Twilio's regulatory guidelines.

The `twilio-sms` module abstracts this: a tenant admin configures their sender-id per region; the module selects the right sender-id per recipient country. For US, the tenant goes through a one-time Brand + Campaign registration; the module stores the TCR Brand ID and Campaign ID. For India, the module stores the DLT Template IDs.

**3. The 5 SMS statuses (DLR).** Twilio's status callback webhook reports:
- `queued` — Twilio accepted the message; queued for delivery.
- `sent` — Twilio handed off to the carrier.
- `delivered` — the carrier confirmed delivery to the handset.
- `undelivered` — the carrier could not deliver (recipient phone off, no signal, blocked).
- `failed` — Twilio could not send (invalid number, sender-id not registered, suppression list).

The `sms_messages` row's `status` column is updated by the webhook. The cost column is filled by the `sms.cost` field in the Twilio response (per-message; varies by country).

**4. The send flow (outbound).**
1. `NotificationService::send(channel: 'sms', to: '+91xxxxxxxxxx', template: 'appointment_reminder', data: {...})` is called by an upstream event (e.g., a scheduled job 24h before the appointment).
2. The `SmsService::send()` resolves the template (from `notification_templates.sms_template_id`), renders the body with the data (e.g., "Hi {patient_name}, your appointment with {doctor_name} is at {appointment_time}. Reply YES to confirm or call {clinic_phone} to reschedule.").
3. The service checks the **opt-out list** (`sms_opt_outs.phone`); if the recipient has opted out, the send is **skipped** and `status='opted_out'` is set.
4. The service calls Twilio's `POST /2010-04-01/Accounts/{sid}/Messages.json` with `{ To, From, Body, StatusCallback: 'https://api.medicare.app/webhooks/twilio/sms-status' }`. Twilio returns `{ sid, status: 'queued', price, price_unit }`.
5. The service writes the `sms_messages` row with `status='queued'`, `twilio_sid`, `cost`.
6. Twilio sends; the status callback fires (sent → delivered); the row's status is updated.

**5. The OTP flow (inbound direction = outgoing only).**
1. Mobile app → `POST /auth/otp/send` with `{ phone }`. The API checks `MEDICARE_OTP_PROVIDER`: if `firebase` (default), it calls Firebase; if `twilio`, it calls Twilio Verify Services.
2. The `TwilioVerifyService` calls `POST https://verify.twilio.com/v2/Services/{verify_sid}/Verifications` with `{ To: phone, Channel: 'sms' }`. Twilio sends the OTP (e.g., "Your Medicare verification code is 123456. It expires in 10 minutes.") and returns `{ sid, status: 'pending' }`.
3. Mobile app → `POST /auth/otp/verify` with `{ phone, code }`. The API calls `POST https://verify.twilio.com/v2/Services/{verify_sid}/VerificationCheck` with `{ To: phone, Code: code }`. Twilio returns `{ status: 'approved' | 'pending' | 'canceled' }`.
4. On `approved`, the API issues a Sanctum token. On `pending` (wrong code), the API returns 401. Twilio enforces a 5-attempt cap per verification; after that, the verification is `canceled` and a new one must be requested.

**6. Inbound SMS (two-way chat).** Twilio sends a webhook `POST /webhooks/twilio/sms-received` for every inbound SMS to one of our numbers. The webhook payload is `application/x-www-form-urlencoded` per Twilio's convention: `{ From, To, Body, MessageSid, FromCity, FromState, FromCountry, NumMedia }`. The API:
1. Verifies the `X-Twilio-Signature` HMAC against the request URL and body (per Twilio's signing algorithm).
2. Looks up the recipient (the `To` number is a per-tenant Twilio number; mapped to `tenant_id` via `sms_phone_numbers`).
3. Checks the opt-out keywords (`STOP`, `STOPALL`, `UNSUBSCRIBE`, `CANCEL`, `END`, `QUIT`): if matched, adds the sender to `sms_opt_outs` with `reason='opt_out'`, and sends an opt-out confirmation ("You have been unsubscribed. Reply START to opt back in.").
4. Checks the opt-in keywords (`START`, `YES`, `UNSTOP`): if matched, removes the sender from `sms_opt_outs`.
5. Otherwise, the body is matched to an open conversation (per `11-Notif §20.4` In-App Chat; SMS is a transport): the API creates or extends an `in_app_conversation` and `in_app_message` (with `transport='sms'`); the doctor's UI shows the patient's SMS as a chat message; the doctor's reply is sent back via SMS.

**7. The 6 opt-out keywords (CTIA guidelines).** STOP, STOPALL, UNSUBSCRIBE, CANCEL, END, QUIT. These are mandated by the CTIA (Cellular Telecommunications Industry Association) for US A2P. The 2 opt-in keywords are START, YES, UNSTOP. The opt-out is **carrier-level** (Twilio forwards to all US carriers) **and** application-level (we maintain `sms_opt_outs`). The HELP keyword returns "Reply STOP to opt out. For help, contact support@medicare.app".

**8. Throughput and rate limits.**
- **US A2P 10DLC:** default 75 MPS (messages per second) per campaign; can be raised to 4,500 MPS with vetting.
- **India DLT:** 100 TPS per template (DLT-enforced).
- **EU / UK:** 1 MPS default; can be raised.
- **Other:** Twilio's per-country limits.
- The `SmsService` queues sends to a per-region queue; a worker pool drains the queue at the limit; excess is retried with backoff.

**9. Cost.** SMS is paid per send. Per-country rates (2026, indicative):
- US: $0.0079 (toll-free), $0.0085 (A2P 10DLC), $0.05 (short-code).
- India: $0.0025 (DLT-registered, transactional).
- UK: $0.04.
- Nigeria: $0.05.
The cost is logged in `sms_messages.cost`; the monthly cost rollup is in `sms_costs_by_tenant`.

**10. Domain Model.**
```php
Schema::create('sms_messages', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('user_id')->nullable()->index();
    $t->string('to_phone', 32);
    $t->string('from_phone', 32);                              // Twilio number or short code
    $t->text('body');
    $t->string('twilio_sid', 64)->unique()->nullable();
    $t->string('status', 16);                                  // 'queued' | 'sent' | 'delivered' | 'undelivered' | 'failed' | 'opted_out'
    $t->decimal('cost', 6, 4)->nullable();
    $t->char('cost_currency', 3)->nullable();
    $t->text('error')->nullable();
    $t->string('template_id', 64)->nullable()->index();
    $t->string('transport', 16)->default('outbound');          // 'outbound' | 'inbound'
    $t->string('purpose', 32);                                 // 'notification' | 'otp' | 'marketing'
    $t->jsonb('metadata')->nullable();
    $t->timestamp('queued_at')->nullable();
    $t->timestamp('sent_at')->nullable();
    $t->timestamp('delivered_at')->nullable();
    $t->timestamp('failed_at')->nullable();
    $t->timestamps();
    $t->index(['tenant_id', 'status', 'created_at']);
});

Schema::create('sms_opt_outs', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('phone', 32)->index();
    $t->string('reason', 32)->default('opt_out');             // 'opt_out' | 'complaint' | 'manual'
    $t->string('source', 16);                                  // 'sms_keyword' | 'admin' | 'web_form'
    $t->timestamp('opted_out_at');
    $t->timestamp('opted_in_at')->nullable();
    $t->timestamps();
    $t->unique(['tenant_id', 'phone']);
});

Schema::create('sms_phone_numbers', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('phone_number', 32)->unique();                 // E.164
    $t->string('twilio_phone_sid', 64)->unique();             // PNxxx
    $t->string('country', 2);                                  // ISO 3166-1 alpha-2
    $t->string('type', 16);                                    // 'long_code' | 'short_code' | 'toll_free' | 'alphanumeric'
    $t->string('purpose', 16)->default('transactional');      // 'transactional' | 'otp' | 'both'
    $t->jsonb('regulatory')->nullable();                       // TCR brand_id, campaign_id, DLT template_ids, etc.
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});
```

**11. API Surface.**
```
# Admin
GET    /api/v2/admin/sms/numbers                              -> [sms_phone_number]
POST   /api/v2/admin/sms/numbers                              -> { phone, country, type, purpose } -> sms_phone_number
PATCH  /api/v2/admin/sms/numbers/{id}                         -> { purpose?, is_active? }
DELETE /api/v2/admin/sms/numbers/{id}                         -> 204
GET    /api/v2/admin/sms/opt-outs                             -> [opt_out] (filterable)
POST   /api/v2/admin/sms/opt-outs/{id}/opt-in                 -> 204 (manual re-opt-in)
GET    /api/v2/admin/sms/stats                                -> { sent_today, delivered_today, undelivered_today, cost_today, opt_outs_total }
GET    /api/v2/admin/sms/messages                             -> [message] (filter by status, purpose, date range)

# Auth (OTP)
POST   /api/v1/auth/otp/send                                  # same path; if MEDICARE_OTP_PROVIDER=twilio, uses Twilio Verify
POST   /api/v1/auth/otp/verify                                # same path; Twilio Verify check

# Notification (internal, used by NotificationService)
POST   /api/internal/sms/send                                 # { to, body, template_id?, purpose? } -> sms_message
POST   /api/internal/sms/send-bulk                            # { recipients[], body, template_id? } -> batch

# Webhooks
POST   /api/webhooks/twilio/sms-status                        # status callback (queued/sent/delivered/undelivered/failed)
POST   /api/webhooks/twilio/sms-received                      # inbound SMS
```

**12. Frontend.**
- **Admin:** `/admin/notifications/sms` shows: phone numbers (with country, type, TCR/DLT registration status), opt-outs (with manual re-opt-in), the stats widget (sent / delivered / cost), and a "Test SMS" button (sends a single test message to a chosen number).
- **Notification composer:** the existing `notification_templates` UI gains an "SMS" tab where the admin authors the SMS body (160 chars for GSM-7, 70 chars for Unicode, segments auto-calculated), picks the DLT template ID (for India), and previews the rendered SMS with sample data.
- **Two-way chat:** the existing in-app chat UI (per §20.4) shows SMS messages as `transport='sms'`; the doctor's reply is sent via SMS; the patient sees the SMS thread.

**13. Realtime.**
- `sms.sent.{tenant_id}.{user_id}` — SMS sent (for the live notification feed).
- `sms.delivered.{tenant_id}.{sms_message_id}` — delivery confirmed.
- `sms.failed.{tenant_id}.{sms_message_id}` — delivery failed; the upstream notification logic can decide to retry (email or push fallback).
- `sms.opted_out.{tenant_id}.{phone}` — recipient opted out; UI updates the opt-out list.
- `sms.inbound.{tenant_id}.{from_phone}` — inbound SMS received; routed to the in-app chat.

**14. Security & RBAC.**
- **OTP rate limit:** Twilio Verify's built-in rate limit (5 attempts per verification) plus our rate limit (5 OTP sends per phone per hour; 1 per phone per 30s).
- **PII:** phone numbers are stored in E.164 format. The body may contain PII (patient name); the body is encrypted at rest (per V2.1 `field-level-encryption` for the body column).
- **Opt-out is immediate:** an opt-out takes effect within 1 second; subsequent sends return `status='opted_out'` and do not contact Twilio.
- **Carrier complaints:** Twilio forwards complaints (e.g., "Spam" reports via 7726) to a webhook; we mark the recipient as `sms_opt_outs.reason='complaint'` and stop sending.
- **Roles:** `tenant_admin` (configure phone numbers, view opt-outs, manual re-opt-in), `notification_admin` (view messages, retry failed, test send), `compliance_officer` (view opt-outs, view opt-out audit). Sending is internal (notification service only).
- **Audit:** every send, every opt-out, every opt-in, every test send is logged in `audit_logs` with the actor, the recipient, the body, and the template ID.
- **A2P 10DLC vetting:** for US, the tenant's Brand must be vetted by TCR (vetting level: standard, which is the default). The vetting result is stored in `sms_phone_numbers.regulatory.vetting_status`.

**15. Acceptance Criteria.**
- [ ] Outbound SMS sends in < 2 seconds; the `sms_messages` row is updated to `status='sent'` then `'delivered'` within 30 seconds.
- [ ] Twilio Verify OTP works end-to-end; wrong codes return 401; correct codes issue a Sanctum token.
- [ ] The opt-out keywords (STOP, STOPALL, UNSUBSCRIBE, CANCEL, END, QUIT) are honored; subsequent sends to the same phone return `opted_out`.
- [ ] The opt-in keywords (START, YES, UNSTOP) are honored; subsequent sends succeed.
- [ ] Inbound SMS is verified (X-Twilio-Signature HMAC); rejected if invalid.
- [ ] Inbound SMS is routed to the in-app chat; the doctor's reply is sent back via SMS.
- [ ] The DLR webhook updates the `sms_messages.status` correctly for queued/sent/delivered/undelivered/failed.
- [ ] The DLT template ID is required for Indian numbers; the body matches the template; mismatches are rejected at the API.
- [ ] The A2P 10DLC TCR Brand + Campaign registration is required for US numbers; sends fail-fast if not registered.
- [ ] The cost is logged per message; the monthly cost rollup is accurate.
- [ ] The throughput is respected (e.g., 75 MPS for A2P 10DLC; the queue drains at that rate).
- [ ] PII (the body) is encrypted at rest.

**16. Real-World Test Scenarios.**
- *OTP fallback:* India user can't receive Firebase OTP; the mobile app falls back to Twilio Verify; the user receives "Your Medicare code is 123456"; the user enters 123456; login succeeds.
- *Appointment reminder:* scheduled job runs 24h before an appointment; the SMS is "Hi Priya, your appointment with Dr. Kumar is tomorrow at 10:00 AM. Reply YES to confirm or call 080-xxxx to reschedule."; the SMS is sent; the DLR confirms delivery.
- *Opt-out:* patient replies STOP; the API marks the phone as opted out; the next appointment reminder is skipped; the audit log shows the opt-out.
- *Opt-in:* patient replies START; the API removes the opt-out; the next appointment reminder is sent.
- *Two-way chat:* patient replies YES; the API routes to the in-app chat as `transport='sms'`; the doctor sees the YES; the doctor replies "Great, see you tomorrow"; the reply is sent via SMS to the patient.
- *DLT mismatch (India):* the SMS body doesn't match a registered DLT template; Twilio rejects with error 21408; the API logs the rejection; the SMS is not sent; the admin is notified.
- *A2P 10DLC throughput:* 1,000 appointment reminders at 9 AM; the queue drains at 75 MPS; the last SMS is sent 13 seconds after the first.
- *Delivery failure:* patient's phone is off; the carrier reports `undelivered`; the API marks the row; the upstream logic can retry via email.
- *Carrier complaint:* patient reports the SMS as spam via 7726; Twilio forwards the complaint; the API marks the phone as `sms_opt_outs.reason='complaint'`; subsequent sends are blocked.
- *Inbound SMS to wrong tenant:* a patient of Tenant A sends an SMS to Tenant B's number; the API looks up the number in `sms_phone_numbers`, finds Tenant B; the conversation is created under Tenant B; Tenant B's staff is paged.

**17. What this module is NOT:**
- Not a WhatsApp replacement. WhatsApp is the `whatsapp-business` module (separate).
- Not a marketing tool. SMS is for transactional and OTP only; bulk marketing SMS requires explicit opt-in and a separate campaign registration.
- Not a free channel. SMS is paid per send; the cost is non-trivial at scale.
- Not a global one-size-fits-all. The regulatory landscape is per-country; the module abstracts the basics but edge cases (e.g., China's MIIT registration) are out of scope.

**18. Dev Workflow.** `php artisan sms:send --tenant=... --to=+91xxxxxxxxxx --body='Test'` sends a test SMS. `php artisan sms:test-inbound --tenant=... --from=+91xxxxxxxxxx --body='YES'` simulates an inbound SMS. `php artisan sms:opt-out --phone=+91xxxxxxxxxx` adds an opt-out. `php artisan sms:reconcile --from=2026-01-01 --to=2026-01-31` reconciles the Twilio message log against the local `sms_messages` table. Code: `app-modules/twilio-sms/`; tests: `app-modules/twilio-sms/tests/Feature/{Send,OTP,Inbound,OptOut,StatusCallback,TwoWayChat}Test.php` and `tests/Reconciliation/TwilioSmsReconcileTest.php`. Sandbox: Twilio test credentials (`ACxxx` test account; test phone numbers `+15005550006`); a `TwilioSmsTestHelper` fakes the gateway in CI.

### 20.3 Voice calls (V2.1)

Twilio Programmable Voice. Used for:
- Critical alerts (lab result with abnormal flag).
- No-show reminders 30 min after the appointment.
- Patient opt-in for "call me when my slot is ready" (queue management).

`VoiceCallService::call($phone, $twimlUrl)` triggers an outbound call. Twilio fetches TwiML from our URL, which reads the message via TTS. Cost ~$0.02/min.

### 20.4 In-app chat (`in-app-chat`, V2, MML 4)

**1. Overview.** V1 chat is a thin email-replacement: send a message, the other side replies, no realtime, no attachments, no chart integration. V2 turns the in-app chat into the **primary asynchronous channel** between patient and doctor: realtime, attachments (image, document, voice, prescription card, lab result card, video call offer), typing indicators, read receipts, push fallback, full-text search, and full encounter-chart integration (the entire chat is saved to the patient chart at the end of the consult, and key messages can be promoted into the encounter note). MML 4 GA covers all of the above; MML 5 (V2.1) adds billable chat.

**2. Domain Model.**
```php
Schema::create('in_app_chats', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('doctor_id')->index();
    $t->ulid('encounter_id')->nullable()->index();          // linked encounter if chat is part of a consult
    $t->string('status', 16)->default('active');            // 'active' | 'archived' | 'blocked'
    $t->ulid('last_message_id')->nullable();
    $t->timestamp('last_message_at')->nullable()->index();
    $t->boolean('is_pinned')->default(false);
    $t->boolean('is_muted_by_patient')->default(false);
    $t->boolean('is_muted_by_doctor')->default(false);
    $t->timestamps();
    $t->unique(['patient_id', 'doctor_id', 'encounter_id']);
});

Schema::create('in_app_messages', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('chat_id')->index();
    $t->ulid('sender_user_id')->index();
    $t->string('type', 16)->default('text');                // 'text' | 'image' | 'document' | 'voice' | 'prescription' | 'lab_result' | 'video_call_offer' | 'appointment_card'
    $t->text('body')->nullable();
    $t->json('attachment')->nullable();                     // {s3_key, mime, size, name, duration_s?}
    $t->string('status', 16)->default('sent');              // 'sent' | 'delivered' | 'read' | 'failed'
    $t->timestamp('read_at')->nullable();
    $t->ulid('reply_to_message_id')->nullable();
    $t->json('mentions')->nullable();                       // [{user_id, offset}]
    $t->boolean('is_promoted_to_chart')->default(false);
    $t->ulid('encounter_section_id')->nullable();           // if promoted
    $t->string('redaction_reason', 32)->nullable();
    $t->timestamps();
    $t->softDeletes();
});

Schema::create('in_app_message_reactions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('message_id')->index();
    $t->ulid('user_id')->index();
    $t->string('emoji', 16);
    $t->timestamp('reacted_at')->index();
});
```

**3. API Surface.**
```
GET    /api/v2/chat/chats?patient_id=...&doctor_id=...         # list threads
POST   /api/v2/chat/chats                                        # create or fetch existing
GET    /api/v2/chat/chats/{id}/messages?before=...&limit=50
POST   /api/v2/chat/chats/{id}/messages                         # send (multipart for attachments)
PATCH  /api/v2/chat/chats/{id}/messages/{msg_id}                # edit own message
DELETE /api/v2/chat/chats/{id}/messages/{msg_id}                # soft-delete (10-min window for sender, anytime for admin)
POST   /api/v2/chat/chats/{id}/messages/{msg_id}/read           # mark read
POST   /api/v2/chat/chats/{id}/messages/{msg_id}/react
POST   /api/v2/chat/chats/{id}/messages/{msg_id}/promote        # doctor promotes to encounter
POST   /api/v2/chat/chats/{id}/archive
POST   /api/v2/chat/chats/{id}/block
GET    /api/v2/chat/chats/{id}/search?q=...
POST   /api/v2/chat/chats/{id}/typing                            # ephemeral, no persistence
```

**4. Frontend.**
- **Patient app:** bottom-tab "Messages"; each thread shows last message preview + unread badge; the thread view is a familiar chat UI (sent bubbles right, received left), with attachment tray (image picker, document picker, voice record), voice waveform during recording, typing indicator, and read-receipt ticks (single gray = sent, double gray = delivered, double blue = read). Long-press a sent message to edit/delete (within 10 min).
- **Doctor web:** a right-rail "Patient Chat" panel inside the encounter; messages sync to the encounter draft. A "Promote to chart" button next to each message lets the doctor attach a chat excerpt to a SOAP section.

**5. Realtime.**
- `chat.{chat_id}` private channel — both participants subscribe via Reverb Pusher auth; new messages, edits, reactions, typing, and read receipts broadcast instantly. (Per `17-Realtime §18.2`.)
- `chat.unread.{tenant}.{user_id}` — fired when a message is delivered to a user with the chat not open; drives the unread badge.

**6. Security & RBAC.** Roles: `patient` (own chats only), `doctor` (own chats only), `care_team_member` (read-only on chats they're a member of, requires patient consent per `06-Patients §2.4`), `tenant_admin` (audit only). All messages are encrypted at rest (SSE-KMS, tenant-scoped key) and in transit (TLS 1.3). A sender may soft-delete their own message within 10 minutes of send (after that, only `tenant_admin` may redact, with `redaction_reason` written to the row and an audit log entry). Files uploaded as attachments pass through ClamAV (`20-Security §11.5`) and a content-type sniff; images are auto-stripped of EXIF GPS data. Mentions in the chat cannot grant access; they only notify. A patient may block a doctor (status='blocked'); blocked chats are not delivered; a row in `audit` is written.

**7. Acceptance Criteria.**
- [ ] A sent message is delivered to the recipient's device in < 2s when online.
- [ ] A message is read-receipted on the sender's device within 5s of the recipient opening the thread.
- [ ] A patient cannot read another patient's chat (RBAC tested).
- [ ] Voice messages are auto-transcribed (V2.1) so the doctor can read before listening.
- [ ] Edit/delete window is 10 min for the sender; UI hides the buttons after.
- [ ] A message promoted to chart is visible in the encounter's SOAP section within 5s.
- [ ] Files >25 MB are rejected with 413; ClamAV-flagged files are rejected with 422 and the user is notified.
- [ ] Search by keyword returns all matching messages across the user's chats, ranked by recency.

**8. Real-World Test Scenarios.**
- *Voice note:* patient records 32s of voice describing symptoms; uploaded; transcribed; doctor reads the transcript and replies by text.
- *Lab result card:* doctor sends a `type='lab_result'` card containing the HbA1c result; the card renders in the patient app with the abnormal flag badge; tapping it deep-links to the full e-Report (§16.3 pathology).
- *Promote to chart:* doctor taps a chat message where the patient said "I started having headaches 2 weeks ago"; promoted to the Subjective section of the SOAP; the chat bubble gains a "In chart" badge.
- *Blocked chat:* patient reports harassment; tenant admin blocks the chat; both sides see "This conversation has been closed"; new messages are refused.
- *EXIF strip:* patient sends a photo of a rash; the EXIF GPS is stripped before storage; the audit row stores the original (city-level only) geo for compliance but not the photo file.

**9. Dev Workflow.** `php artisan chat:simulate --tenant=... --messages=100/min` synthetic load. `php artisan chat:reindex --tenant=...` rebuilds the OpenSearch index. Code: `app-modules/in-app-chat/`; tests: `app-modules/in-app-chat/tests/Feature/{Thread,Message,Attachment,Promote}Test.php` and `tests/Performance/chat-load.k6.js`.

### 20.5 Push categories (`push-categories`, V2)

Patients can subscribe to topic-style push channels:

- `health_tips` — daily health tip from the clinic.
- `clinic_news` — new doctors, new services, offers.
- `lab_reminders` — "Your HbA1c test is due."
- `vaccination_reminders` — from `patient_vaccinations.next_due_at`.
- `appointment_tips` — pre-appointment prep.

`push_subscriptions(user_id, category, subscribed_at)`. `PushCategoryService::send($category, $template, $audience)` sends to all subscribers.

### 20.6 AI notification copy (`ai-notification-copy`, V2.1)

LLM rewrites notification copy in the user's language, with appropriate tone (formal in German, friendly in Hindi, concise in English).

```php
class AiNotificationCopyService {
    public function rewrite(string $templateKey, string $locale, array $vars): string {
        $prompt = config("prompts.notification.{$templateKey}");
        return $this->llm->complete([
            'model' => 'gpt-4o-mini',
            'messages' => [
                ['role' => 'system', 'content' => "You are a medical notification copywriter. Locale: {$locale}. Be concise, friendly, never alarmist. Avoid medical advice."],
                ['role' => 'user', 'content' => "Template: {$prompt}\nVariables: " . json_encode($vars)],
            ],
            'temperature' => 0.3,
            'max_tokens' => 200,
        ]);
    }
}
```

Used for: WhatsApp messages (no template needed if it's a session message), email subject lines, push titles. Cost: $0.0001 per rewrite.

### 20.7 Template engine upgrades (V2)

- **Liquid syntax:** `{{ patient.first_name }}`, `{{ appointment.start_at | date: '%a %e %b %Y' }}`, `{% if appointment.doctor.gender == 'f' %} Dr. (Mrs.) {% else %} Dr. {% endif %}`.
- **Multi-locale inline:** editor shows all locales side-by-side.
- **Versioning:** every save creates a new version; rollback available.
- **A/B testing:** two variants, split 50/50, measure open rate, auto-pick winner.

### 20.8 V2 API additions

```
POST   /api/v2/notifications/whatsapp/send                -> whatsapp_message
GET    /api/v2/notifications/whatsapp/conversations       -> [conversation]
POST   /api/v2/notifications/whatsapp/inbound             -> handle webhook
GET    /api/v2/notifications/sms                          -> [sms]
POST   /api/v2/notifications/sms/inbound                  -> handle webhook
POST   /api/v2/notifications/voice/call                   -> voice_call
GET    /api/v2/notifications/voice/calls                  -> [call]
POST   /api/v2/chat/chats                                 -> chat
GET    /api/v2/chat/chats                                 -> [chat]
GET    /api/v2/chat/chats/{id}                            -> chat with messages
POST   /api/v2/chat/chats/{id}/messages                   -> message
PATCH  /api/v2/chat/chats/{id}/messages/{mid}             -> read
DELETE /api/v2/chat/chats/{id}/messages/{mid}             -> 204
GET    /api/v2/push/categories                            -> [category]
POST   /api/v2/push/categories/{slug}/subscribe           -> subscribed
DELETE /api/v2/push/categories/{slug}/subscribe           -> unsubscribed
```

### 20.9 V2 acceptance criteria

- [ ] WhatsApp template sends successfully via Meta Cloud API; delivery webhook updates status.
- [ ] 24h customer service window is enforced; outside window, only templates are sent.
- [ ] Twilio SMS sends in <2s; delivery webhook updates status.
- [ ] Two-way WhatsApp/SMS: patient replies are stored in the conversation.
- [ ] In-app chat delivers messages in <500ms via Reverb.
- [ ] Read receipts update in real-time.
- [ ] Push categories: subscribe/unsubscribe, target by category.
- [ ] AI rewrite produces locale-appropriate copy in 9 languages.
- [ ] All V2 notification events are auditable.

---

## 22. V3+ Modules — From Push & SMS to Full Patient Engagement

> V1 sends FCM/SMS/email via the basic dispatcher. V2 adds WhatsApp, in-app chat, voice calls, AI copy, push categories. V3+ turns the notifications module into a full **patient engagement & communication platform** — omnichannel orchestration, conversation AI, voice-first, video messaging, smart bundling, deliverability optimization, and full compliance.

### 22.1 Omnichannel Orchestrator (`omnichannel-orchestrator`, V3)

A central engine that decides **which channel to use, when, and how** for every notification. The goal: maximize engagement while minimizing cost and respecting user preferences.

**`NotificationOrchestrator::dispatch($notification)`**:
1. **Check user preferences**: User has opted out of SMS but in for email? Honor.
2. **Check quiet hours**: It's 11 PM in user's timezone? Defer to 8 AM unless urgent.
3. **Check channel availability**: Is the device online? FCM token valid? Phone reachable?
4. **Check priority**: Urgent (medical emergency) bypass quiet hours.
5. **Select primary channel**: based on user preferences + content type.
6. **Select fallback channel(s)**: if primary fails, try next.
7. **Schedule**: if not urgent, batch with other non-urgent; if urgent, send immediately.
8. **Track**: open, click, conversion per channel per user.

**Channel priority matrix (configurable per tenant, overridable per user):**

| Content Type | Primary | Fallback 1 | Fallback 2 | Last Resort |
|---|---|---|---|---|
| Appointment confirmed | Push (mobile) | Email | SMS | In-app |
| Appointment reminder (24h) | Push + SMS | Email | Voice | In-app |
| Appointment reminder (1h) | Push + SMS | Voice | WhatsApp | In-app |
| Payment received | Push + Email | SMS | In-app | — |
| Refund processed | Push + Email | SMS | In-app | — |
| Lab results ready | Email (with PDF) | Push | SMS | — |
| Prescription expiring | Push | SMS | WhatsApp | Email |
| Critical (e.g., positive COVID test) | Voice + SMS + Email | WhatsApp | In-app | — |
| Marketing / newsletter | Email | Push | — | — |
| Family member's appointment | SMS | Email | Voice | In-app |
| Insurance claim update | Email | SMS | In-app | — |
| Post-visit follow-up | Email | Push | SMS | — |

**`NotificationOrchestrator::decideChannel($user, $contentType)`**:
```php
public function decideChannel(User $user, string $contentType): array
{
    $prefs = $user->notificationPreferences;
    $rules = config("notifications.channel_priority.{$contentType}");
    $available = $this->getAvailableChannels($user);
    $candidates = array_intersect($rules, $available);
    $honored = array_intersect($candidates, $prefs->enabled_channels);
    return array_values($honored ?: $candidates);
}
```

### 22.2 Smart Bundling & Digest (`smart-bundling`, V3)

Instead of 10 separate push notifications, bundle related ones into a digest.

**`BundlingEngine::shouldBundle($notification, $user)`**:
- If 3+ notifications of same type in 1 hour → bundle.
- If 5+ notifications of any type in 30 min → digest.
- "New appointments, prescriptions, and lab results available" → one push.
- "3 messages from your care team" → one push with deep link.

**Bundle types:**
- **Time-based**: hourly digest (if 3+ items).
- **Type-based**: same category (3 appointment reminders → 1).
- **Sender-based**: 5 messages from same doctor → "Dr. Mehta has 3 messages for you".
- **AI-driven**: GPT-4o summarizes all unread into 1-2 sentence push.

**`/api/v3/notifications/digest?period=hourly|daily`** returns the bundled feed.

### 22.3 Conversation AI (In-App Chat) (`conversation-ai`, V3)

The in-app chat is now an AI-assisted conversation between patient and care team.

**`ChatService::process($userId, $message, $context)`**:
1. **Intent classification**: GPT-4o classifies: "schedule_appointment" | "refill_request" | "lab_question" | "billing_question" | "general" | "urgent".
2. **Entity extraction**: Extract appointment date, doctor name, prescription, etc.
3. **Routing**: 
   - "schedule_appointment" → suggest available slots, deep-link to booking.
   - "refill_request" → start refill flow.
   - "lab_question" → forward to nurse triage.
   - "billing_question" → forward to billing.
   - "general" → AI responds with FAQ answers.
   - "urgent" → page on-call provider immediately.
4. **AI response**: GPT-4o generates a draft reply based on patient's history, the clinic's tone, and the intent.
5. **Human review**: Draft goes to care team member for review/edit. Auto-send for low-stakes (FAQs); require approval for medical advice.
6. **Continuous learning**: Care team accepts/edits AI drafts; corrections feed back into the model.

**`ChatThread` model:**
```php
Schema::create('chat_threads', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('care_team_id')->nullable()->index();
    $t->string('subject', 128)->nullable();
    $t->string('category', 32)->nullable();
    $t->string('status', 16)->default('open');  // 'open' | 'waiting_patient' | 'waiting_care_team' | 'resolved' | 'closed'
    $t->ulid('assigned_user_id')->nullable();
    $t->integer('unread_count_patient')->default(0);
    $t->integer('unread_count_team')->default(0);
    $t->timestamp('last_message_at')->nullable();
    $t->json('ai_context')->nullable();
    $t->timestamps();
});

Schema::create('chat_messages', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('thread_id')->index();
    $t->ulid('sender_user_id')->nullable()->index();
    $t->boolean('is_ai')->default(false);
    $t->boolean('is_from_patient')->default(false);
    $t->text('body');
    $t->json('attachments')->nullable();   // [{s3_key, mime, filename}]
    $t->json('ai_metadata')->nullable();   // intent, entities, confidence
    $t->boolean('is_edited')->default(false);
    $t->ulid('edited_from_message_id')->nullable();
    $t->timestamp('read_at')->nullable();
    $t->timestamps();
});
```

**Realtime:** WebSocket on `chat.{thread_id}` channel. Typing indicator, read receipts, presence (online/offline).

**AI capabilities:**
- Auto-summarize long threads.
- Suggest quick replies for the care team.
- Detect escalation signals ("I want to speak to a manager", "I'm going to sue") → auto-escalate.
- Sentiment analysis → flag frustrated patients.
- Language detection → translate on the fly.

### 22.4 Voice-First & Video Messages (`voice-first`, V3)

For accessibility, low-literacy populations, and elderly, voice is often better than text.

**Voice notes:**
- Patient records a voice note (mobile uses `record` package).
- Uploaded to S3, transcribed by Whisper.
- Sent to care team as text + audio.
- Care team can reply with voice (transcribed to text for log).

**Voice calls (auto-dialer):**
- `VoiceCallService::initiate($user, $template, $params)`:
  - Twilio: `client.calls.create({to, from, twiml})`.
  - Plays TTS message using the template.
  - IVR menu: "Press 1 to confirm, 2 to reschedule, 3 to speak to a representative".
  - Records responses (DTMF + voice).
  - Logs to `voice_calls` table.
- **Use cases**: appointment reminders (proven 30% reduction in no-shows), lab follow-up, payment reminders, post-op check-in.

**Video messages:**
- Care team records a 1-2 minute video explaining a diagnosis.
- Patient watches asynchronously.
- Used for: pre-op instructions, post-op care, lab result explanation, chronic disease education.
- Stored in S3 (encrypted), served via signed URLs.
- AI summary on top of video for accessibility.

**Live video consultations (V2 had this, V3 deepens):**
- WebRTC via LiveKit / Daily.co / mediasoup.
- Recording with consent.
- AI transcription + SOAP note generation (V3 AI scribe).
- Multi-party (3+ for family/team consults).
- Background blur, virtual backgrounds.
- Live captions (Web Speech API + Whisper).

### 22.5 Smart Send Time & Throttling (`smart-send-time`, V3)

**Per-user optimal send time**:
- Analyze past 90 days of engagement: when does this user open notifications?
- ML model predicts the best hour to send.
- Default: 8-9 AM local (highest baseline engagement).
- Skip Friday evening / Saturday morning (low engagement).

**Throttling:**
- Max 5 push notifications per user per day (configurable).
- Max 2 marketing SMS per user per week.
- Max 1 voice call per patient per day.
- If exceeded, batch into digest or skip with audit.

**Frequency cap per category:**
- Appointment reminders: 3 per appointment.
- Lab notifications: 1 per lab.
- Marketing: 1 per week per user.
- Patient can override per category in preferences.

**`NotificationThrottleService::canSend($user, $category, $channel)`** returns bool.

### 22.6 Deliverability & Sender Reputation (`deliverability`, V3)

**Email deliverability:**
- SPF, DKIM, DMARC properly configured (verified by `mail-tester.com` score 10/10).
- Dedicated sending IPs per tenant (for high-volume tenants).
- IP warm-up schedule for new IPs (start at 50/day, ramp to 10k/day over 4 weeks).
- Bounce management: hard bounce → suppress; soft bounce → retry 3x then suppress.
- Complaint handling: spam complaint → auto-unsubscribe + suppress.
- Engagement-based: Gmail/MSN/Apple throttling — if user hasn't opened in 90 days, move to "engagement campaign" before going dormant.
- List-unsubscribe header (RFC 8058) for one-click unsubscribe.
- BIMI (Brand Indicators for Message Identification) — show logo in inbox.

**SMS deliverability:**
- Sender ID registration per country (DLT in India, 10DLC in US, A2P in UK).
- Carrier filtering rules (avoid "STOP", "FREE", "WINNER").
- Short code vs long code: use long code for transactional, short code for mass.
- Link shortener with domain reputation monitoring (Bitly, Rebrandly).
- Opt-out keyword handling: STOP, UNSUBSCRIBE, CANCEL → auto-process.

**Push deliverability:**
- FCM: keep tokens fresh; remove invalid tokens.
- APNS: HTTP/2 keepalive, APNS auth keys rotated every 12 months.
- Use silent push for high-priority updates (foreground, no sound).

**WhatsApp deliverability:**
- **24-hour customer service window**: After user-initiated conversation, you have 24h to send free-form messages. After that, must use template.
- **Pre-approved templates**: HSM (Highly Structured Messages) registered in Meta Business Manager.
- **Quality rating**: Meta monitors; low rating (red) → message limits.
- **Opt-in flow**: User must opt-in before any marketing template.

**Voice deliverability:**
- STIR/SHAKEN attestation for US (mandatory 2021+).
- Caller ID reputation (Reputation scoring).
- Time-of-day restrictions (TCPA: 8am-9pm local).
- DNC (Do Not Call) list scrub.

### 22.7 Localization & Translation (`localization-translation`, V3, deep)

V2 had 9 languages. V3 has 50+ languages with auto-translation.

**`TranslationService::translate($text, $fromLocale, $toLocale)`**:
- For UI strings: Laravel `__()` + `lang/` files (manual translations).
- For dynamic content (e.g., AI-generated reminders): AWS Translate / Google Translate / DeepL API.
- For medical terminology: validated against `MedLex` (SNOMED CT linked).

**Per-user language preference**:
- Detected from browser `Accept-Language` header.
- Stored in `user.preferred_locale`.
- Can be changed in settings.
- All notifications honor this.

**Right-to-left (RTL) support:**
- Arabic, Hebrew, Persian, Urdu.
- Templates have `.rtl` variants.
- Email HTML uses `dir="rtl"`.
- Push notification text is mirrored.

**Cultural adaptation:**
- Date formats: en-US `MM/DD/YYYY` vs en-GB `DD/MM/YYYY` vs hi-IN `DD/MM/YYYY`.
- Time: 12-hour vs 24-hour.
- Name order: Eastern (family first) vs Western (given first).
- Holidays: don't send appointment reminders on local holidays.
- Color: white = mourning in East Asia, red = mourning in some African cultures.

### 22.8 Notification Templates & Builder (`template-builder`, V3)

**`NotificationTemplate` model**:
- Multi-locale (en, hi, ar, es, fr, de, pt, zh-CN, ja, etc.).
- Multi-channel (push, SMS, email, WhatsApp, in-app, voice).
- Variables: `{{patient_name}}`, `{{appointment_date}}`, `{{doctor_name}}`, `{{clinic_name}}`, etc.
- Layouts: branded email templates with logo, colors, footer.
- Preview: WYSIWYG editor in admin panel.
- A/B test: 2 variants of the same template, measure open rate.
- Versioning: every change creates a new version; can roll back.

**Template categories (50+ pre-built):**
- Appointment: created, confirmed, rescheduled, cancelled, reminder-24h, reminder-1h, checked-in, in-consultation, completed, no-show.
- Prescription: created, refilled, expiring, expired.
- Lab: ordered, sample-collected, results-ready, results-abnormal.
- Billing: invoice-issued, payment-received, payment-failed, refund-processed, payment-reminder, dunning.
- Insurance: preauth-submitted, preauth-approved, preauth-denied, claim-submitted, claim-paid, claim-denied.
- Family: child-appointment, dependent-update.
- Care plan: care-plan-created, milestone-achieved, due-soon.
- Marketing: newsletter, promotion, seasonal-greeting.
- Operational: password-reset, login-alert, profile-update, security-alert.
- Emergency: medical-emergency, urgent-recall.

**Template rendering:**
```php
class TemplateRenderer {
    public function render(NotificationTemplate $template, array $vars, string $locale): RenderedTemplate
    {
        $content = $template->translations[$locale] ?? $template->translations[$template->fallback_locale];
        $rendered = $this->substitute($content, $vars);
        $subject = $this->substitute($template->subject, $vars);
        return new RenderedTemplate(
            subject: $subject,
            body: $rendered,
            channel: $template->channel,
            metadata: $this->extractMetadata($rendered),  // CTAs, deep links
        );
    }
}
```

### 22.9 Compliance & Consent (`compliance-consent`, V3, deep)

**TCPA (US Telephone Consumer Protection Act):**
- Express written consent for marketing SMS.
- Express consent for marketing voice calls.
- Opt-out via STOP/UNSUBSCRIBE → immediate.
- Time-of-day: 8 AM - 9 PM local.
- Records of consent: timestamp, IP, form text, checkbox state.
- Revocation: 10 business days to honor.

**GDPR / DPDP / PDPA:**
- Explicit, granular consent per channel and per category.
- Easy withdrawal (one-click unsubscribe, in settings).
- Record of consent: who, when, what, how, IP, version.
- Right to be forgotten: hard delete notification history on request.
- Data portability: export all notifications sent.

**HIPAA:**
- PHI in notifications: minimized to the minimum necessary.
- Patient name + "Lab results are ready" → OK.
- Patient name + "Your HIV test is positive" → NEVER (must be in portal).
- Audit trail: every PHI access logged.

**CAN-SPAM (email):**
- Physical postal address in every email.
- Clear identification (no deceptive subject lines).
- One-click unsubscribe.
- Honor opt-out within 10 business days.

**WhatsApp Commerce Policy:**
- Healthcare: only transactional templates (no marketing for prescription drugs, etc.).
- Pre-approved templates required.
- Opt-in mandatory.

**`ConsentService` records:**
```php
Schema::create('user_consents', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('user_id')->index();
    $t->string('purpose', 64);       // 'marketing_sms' | 'transactional_email' | etc.
    $t->boolean('granted');
    $t->string('method', 32);        // 'web_form' | 'mobile_app' | 'phone_call'
    $t->text('consent_text');        // Exact text shown to user
    $t->string('policy_version', 16);
    $t->ipAddress('ip');
    $t->string('user_agent', 255)->nullable();
    $t->json('metadata')->nullable();
    $t->timestamp('granted_at');
    $t->timestamp('revoked_at')->nullable();
    $t->timestamps();
});
```

### 22.10 Engagement Analytics (`engagement-analytics`, V3)

**`EngagementMetricsService`**:
- **Delivery rate**: % of notifications delivered (not bounced).
- **Open rate**: % opened (email: tracking pixel, push: open event).
- **Click-through rate**: % clicked CTA.
- **Conversion rate**: % completed the target action (e.g., confirmed appointment after reminder).
- **Opt-out rate**: % who opted out per channel.
- **Best time to send**: per user, per channel, per content type.
- **Channel preference**: % of users who primarily use each channel.
- **Engagement by locale**: en vs hi vs ar.
- **Engagement by age/segment**: young prefer push; elderly prefer voice/SMS.
- **Revenue attribution**: appointment booked after notification → revenue.
- **Churn correlation**: low engagement → high churn risk.

**Dashboards:**
- Real-time: notifications sent per minute, delivery success rate.
- Hourly: bounce rate per gateway, error rate.
- Daily: open/click/conv rate per channel and template.
- Weekly: cohort analysis, A/B test results.
- Monthly: full funnel, engagement trends, opt-out trends.

**A/B test framework:**
- Define variants of subject, body, send time, channel.
- 50/50 split (or weighted).
- Measure: open rate, click rate, conversion rate.
- Auto-promote winning variant (if 95% confidence for 7 days).
- All results logged for analysis.

### 22.11 Notification Center & Inbox (`notification-center`, V3)

A user-facing unified inbox (in-app and web).

**`NotificationCenterService`**:
- All notifications (push, email, SMS, etc.) archived in `notifications` table.
- User can browse history: `GET /api/v3/notifications?type=&from=&to=&read=`.
- Filter: unread, by category, by sender.
- Search: full-text on subject and body.
- Mark as read (single, bulk, all).
- Archive (move out of inbox).
- Snooze: defer for 1h, 4h, tomorrow.
- Pin: keep at top.

**Per-type actions:**
- Appointment notification: "Add to calendar", "Reschedule", "Cancel".
- Lab result: "View", "Share with my doctor", "Print".
- Payment: "View invoice", "Pay now", "Receipt".
- Message: "Reply", "Mark unread", "Block sender".

**Notification preferences UI:**
- Per-channel: push, email, SMS, WhatsApp, voice.
- Per-category: appointments, prescriptions, labs, billing, marketing.
- Quiet hours: 10 PM - 8 AM.
- Daily digest: yes/no, time.
- Per-template override: turn off appointment reminders, keep only 1h.

### 22.12 Channel-Specific V3+ Enhancements

**Email (V3):**
- **AMP for Email**: interactive elements (accordion, carousel) in email.
- **BIMI**: brand logo in inbox (requires VMC cert + DMARC).
- **Inbox placement testing**: integration with `mail-tester.com`, `GlockApps`, `InboxAlly`.
- **Email warmup tool**: 30-day gradual warmup for new IPs.
- **Reputation monitoring**: alerts if IP/domain on any blocklist (Spamhaus, Barracuda, etc.).

**SMS (V3):**
- **RCS (Rich Communication Services)**: next-gen SMS with rich cards, buttons, media.
- **MMS support**: images, video, audio in SMS (where supported).
- **10DLC registration** (US): brand + campaign registration for A2P.
- **DLT registration** (India): entity, header, template, consent registration.
- **Two-way SMS**: receive replies, parse, route to workflows.

**Push (V3):**
- **Silent push**: data-only, no UI, for app updates.
- **Background push**: trigger background fetch.
- **Push-to-pull**: silent push, app pulls fresh data on receipt.
- **Notification actions**: reply, snooze, mark read inline.
- **Critical alerts** (iOS): bypass Do Not Disturb for emergencies (requires Apple entitlement).
- **Notification grouping** (Android): auto-group related notifications.
- **Time-sensitive notifications** (iOS 15+): higher priority, can bypass Focus modes.

**WhatsApp (V3):**
- **Catalog messages**: browse services in WhatsApp.
- **Cart messages**: items ready to checkout.
- **Flow messages**: interactive multi-step flows (button-based).
- **List messages**: scrollable list of options.
- **Location messages**: share location (e.g., for clinic directions).
- **Contact messages**: share vCard.
- **Sticker packs** (entertainment).
- **Voice messages** (record in-app, send to clinic).
- **Video notes** (60-second video).

**Voice (V3):**
- **AI voice agents**: handle inbound calls with NLU, answer FAQs, route to humans.
- **Speech analytics**: keyword spotting (e.g., "suicide" → escalate to crisis line).
- **Sentiment analysis**: detect frustrated callers, route to senior agent.
- **Voice biometrics**: verify patient by voiceprint (for high-security flows).
- **Multilingual IVR**: support 50+ languages with auto-detection.

### 22.13 Realtime Notification Routing (`realtime-routing`, V3)

Beyond V2's basic presence. V3 has full multi-device realtime.

**`NotificationPresenceService`**:
- Tracks user presence across: mobile app (foreground/background), web (focus/blur), web (active tab), FCM token, APNS token, browser WebSocket.
- Routes push to **all active devices**.
- Routes in-app to **all open tabs**.
- Routes SMS/voice if **none active** within 30s.

**Per-message routing:**
- Live message: realtime channel only (no fallback).
- Important: realtime + push.
- Marketing: email only (no realtime).

**Typing indicators** in chat.
**Read receipts** with timestamp.
**Delivery states**: `delivered` (received by service), `read` (opened).

### 22.14 Scheduled & Recurring Notifications (`scheduled-recurring`, V3)

Beyond cron-based. V3 has user-schedulable notifications.

**Recurring templates (cron expressions):**
- "Daily medication reminder at 8 AM" — daily push.
- "Weekly weigh-in reminder on Sunday at 7 AM" — weekly push.
- "Monthly blood pressure log" — monthly push.
- "Quarterly A1c checkup" — quarterly push + email.

**`RecurringNotificationService::schedule($user, $template, $cron, $params)`**:
- Validates cron expression.
- Stores in `scheduled_notifications` table.
- Cron worker (`ScheduleRunnerJob`) checks every minute, dispatches.
- User can pause, skip, edit, or delete.

**Use cases:**
- Medication adherence (chronic conditions).
- Post-op follow-up (Day 1, 3, 7, 14, 30).
- Pregnancy tracking (weekly fetal development updates).
- Chronic disease (daily glucose check, weekly blood pressure, monthly HbA1c).
- Therapy (weekly journaling prompt).
- Family: child's vaccination schedule.

### 22.15 Emergency & Mass Notification (`emergency-mass`, V3)

For medical emergencies and mass events.

**`EmergencyNotificationService::broadcast($event, $audience, $channels)`**:
- For example: drug recall → all patients who took that drug.
- Channel selection: all available, bypass quiet hours.
- Multi-channel: voice (highest reach), SMS, push, email, WhatsApp.
- Confirmation: patient must acknowledge; if no ack within 1h → retry; if still no → flag.
- Tracking: who was notified, who confirmed.

**Use cases:**
- Drug recall (FDA alert).
- Device recall (UDI).
- Outbreak (epidemic).
- Mass casualty (e.g., contamination event).
- Clinic closure (emergency).
- Appointment cancellation (provider sick, mass schedule change).

**HIPAA compliance**: For PHI-bearing emergencies, use minimum necessary; de-identify where possible.

### 22.16 V3+ API Additions (Selected Highlights)

```
POST   /api/v3/notifications/dispatch
POST   /api/v3/notifications/smart-bundle
GET    /api/v3/notifications/digest?period=hourly|daily
GET    /api/v3/notifications
PATCH  /api/v3/notifications/{id}
POST   /api/v3/notifications/{id}/snooze
POST   /api/v3/notifications/{id}/archive
POST   /api/v3/notifications/bulk-action
GET    /api/v3/notifications/preferences
PUT    /api/v3/notifications/preferences
GET    /api/v3/notifications/preferences/quiet-hours
PUT    /api/v3/notifications/preferences/quiet-hours
POST   /api/v3/chat/threads
GET    /api/v3/chat/threads
GET    /api/v3/chat/threads/{id}
POST   /api/v3/chat/threads/{id}/messages
POST   /api/v3/chat/threads/{id}/read
GET    /api/v3/chat/threads/{id}/messages
GET    /api/v3/chat/ai-suggest/{thread_id}
POST   /api/v3/chat/ai-suggest/{message_id}/accept
POST   /api/v3/chat/ai-suggest/{message_id}/edit
POST   /api/v3/voice/calls/initiate
POST   /api/v3/voice/calls/recording/{id}/transcribe
POST   /api/v3/video-messages
GET    /api/v3/video-messages/{id}
POST   /api/v3/scheduled-notifications
GET    /api/v3/scheduled-notifications
PATCH  /api/v3/scheduled-notifications/{id}
DELETE /api/v3/scheduled-notifications/{id}
POST   /api/v3/emergency/broadcast
GET    /api/v3/emergency/broadcasts/{id}/ack
GET    /api/v3/engagement/metrics
GET    /api/v3/engagement/best-time
GET    /api/v3/templates
POST   /api/v3/templates
PUT    /api/v3/templates/{id}
GET    /api/v3/templates/{id}/preview
POST   /api/v3/templates/{id}/ab-test
GET    /api/v3/ab-tests/{id}/results
POST   /api/v3/templates/{id}/promote-variant
GET    /api/v3/consent
POST   /api/v3/consent
DELETE /api/v3/consent/{id}
POST   /api/v3/translate
GET    /api/v3/reports/engagement
GET    /api/v3/reports/deliverability
GET    /api/v3/reports/opt-out
```

---

## 23. V3+ Test Matrix — Notifications

> Total: **~520 tests** across all levels.

### 23.1 Unit Tests (~250)

**NotificationOrchestrator (30 tests):**
- Selects primary channel per user preference
- Honors opt-out (channel disabled)
- Falls back to next channel on primary failure
- Defers non-urgent during quiet hours
- Bypasses quiet hours for urgent
- Respects timezone for quiet hours
- Respects DST transitions
- Bundles 3+ notifications in 1h
- Bundles by category
- Bundles by sender
- AI digest summary generated
- Throttling: max 5 push/day
- Throttling: max 2 marketing SMS/week
- Per-category throttling
- Channel availability check (offline device)
- Smart send time: optimal hour per user
- Locale-correct rendering
- Per-tenant channel priority matrix
- Per-template override
- Emergency: voice + SMS + email + push simultaneously
- Critical: bypasses all preferences
- Bounce handling: hard bounce → suppress
- Spam complaint: auto-unsubscribe
- Opt-out: stops future immediately
- Opt-in: restarts future
- Channel-level preference (push yes, SMS no)
- Category-level preference (appointment yes, marketing no)
- Frequency cap respected
- Bundle digest frequency (hourly, daily)
- Fallback: SMS if push fails within 30s

**BundlingEngine (15 tests):**
- 3 appointment reminders → 1 bundle
- 5 mixed notifications in 30min → 1 digest
- AI summary generated by GPT
- Bundle subject generated
- Bundle contents deep-linkable
- Bundle click → opens first item
- Time-based bundling
- Type-based bundling
- Sender-based bundling
- Bundle disable per user
- Bundle threshold configurable
- Bundle across channels (push summary of 3 SMS)
- Bundle retention: bundles archived
- Bundle priority: critical not bundled
- Bundle order: chronological

**ChatService (30 tests):**
- Intent classification correct per type
- Entity extraction from message
- Schedule appointment intent → suggest slots
- Refill request intent → start refill flow
- Lab question intent → forward to nurse
- Billing question intent → forward to billing
- General intent → AI responds
- Urgent intent → page on-call
- AI draft generated
- AI draft requires human review (medical advice)
- AI draft auto-sent (FAQ)
- AI draft accepted → sends
- AI draft edited → sends edited version
- AI suggestions feed back to model
- Thread created on first message
- Thread status transitions correct
- Unread count increments
- Unread count resets on read
- Typing indicator sent
- Read receipts sent
- Presence tracked
- Multi-party thread (3+ users)
- Attachments uploaded
- Voice messages transcribed
- AI escalation signal detected ("I want to speak to a manager") → escalate
- Sentiment analysis: negative → flag
- Language detection → translate on fly
- AI summary of long thread
- Thread archived
- Thread closed
- Search across threads

**VoiceCallService (20 tests):**
- Twilio call initiated
- TTS message played correctly
- IVR menu works (DTMF detected)
- Voice recording captured
- Transcription accurate
- DTMF "1" → confirm
- DTMF "2" → reschedule
- DTMF "3" → speak to rep
- Call duration logged
- Call status: completed/no-answer/busy/failed
- Retry on no-answer
- Time-of-day restriction: 8am-9pm only
- DNC scrub before call
- STIR/SHAKEN attestation
- Voicemail left
- Voice biometric match
- Multi-language TTS
- Call forwarding
- Caller ID reputation
- Cost per call tracked

**NotificationTemplate (20 tests):**
- Render with all variables
- Missing variable → error
- Extra variable → ignored
- HTML escape in email
- Plain-text fallback for email
- SMS within 160 char limit (or split for multi-part)
- Push title 50 char limit
- Push body 200 char limit
- WhatsApp template matches HSM
- Voice TTS within duration limit
- Locale fallback (ar→en if not present)
- RTL rendering for Arabic
- Multi-byte chars (Chinese, Hindi) handled
- Link shortener
- Image with alt text
- Inline image (CID) for email
- Personalization (first name)
- A/B variant selection
- Per-tenant branding
- Time-zone aware date formatting

**ConsentService (15 tests):**
- Grant consent records timestamp, IP, text
- Withdraw consent
- Channel-specific consent
- Category-specific consent
- Granular consent
- Age verification (13+, 16+, 18+ per jurisdiction)
- Consent text version recorded
- Policy version bump forces re-consent
- Bulk consent update
- Cross-border consent (EU vs US)
- TCPA express written consent required
- GDPR explicit consent required
- DPDP verifiable consent required
- Audit log on grant
- Audit log on revoke

**DeliverabilityService (20 tests):**
- Email bounce: hard → suppress
- Email bounce: soft → retry 3x → suppress
- Spam complaint → auto-unsubscribe
- Sender score monitoring
- Domain reputation (Spamhaus check)
- IP reputation (Sender Score)
- IP warmup: starts at 50/day
- IP warmup: ramps to 10k/day over 4 weeks
- DLT registration validation (India)
- 10DLC brand registration (US)
- WhatsApp quality rating check
- WhatsApp 24h window enforcement
- WhatsApp template approval status
- DNC list scrub before voice
- TCPA time window: 8am-9pm local
- BIMI logo display
- List-Unsubscribe header present
- Engagement-based: dormant user moved to re-engagement
- Engagement-based: cold user moved to dormant
- Drip campaign: 3 emails over 7 days for dormant

**SmartSendTime (10 tests):**
- Predicts optimal hour from past 90 days
- Falls back to 8-9 AM if no history
- Per-user model
- Updated daily
- Per-channel model
- Per-locale model
- Holiday skip
- Weekend skip (configurable)
- Send-time test (A/B hour)
- Manual override per user

**ScheduledNotificationService (15 tests):**
- Validates cron expression
- Daily at 8 AM
- Weekly on Sunday
- Monthly on 1st
- Quarterly (Jan, Apr, Jul, Oct)
- Custom cron `0 8 * * *`
- Timezone-aware
- DST transitions handled
- User pause → skip but don't delete
- User resume → continues
- User skip-once → skip next occurrence
- User delete → stop permanently
- Multiple schedules per user
- Per-template schedule
- Audit log on schedule

**EmergencyBroadcastService (10 tests):**
- Multi-channel broadcast
- Bypass quiet hours
- Bypass preferences
- Acknowledgment required
- Retry on no-ack after 1h
- Track who acknowledged
- Audience filter (e.g., patients on drug X)
- HIPAA minimum-necessary check
- Geographic filter (city, state)
- Audit log

**EngagementMetrics (15 tests):**
- Open rate computed correctly
- Click rate computed correctly
- Conversion rate computed correctly
- Per-channel breakdown
- Per-template breakdown
- Per-locale breakdown
- A/B test significance
- Best time per user
- Cohort analysis
- Funnel: send → delivered → opened → clicked → converted
- Revenue attribution
- Opt-out rate per channel
- Engagement by age
- Engagement by device
- Engagement by tenure

**Other services (50 tests):**
- In-app notification: create, mark read, archive, snooze
- Search: full-text on subject, body
- Filter: unread, by type, by sender
- Pin: keep at top
- Notification preferences: per channel, per category
- Quiet hours: per day
- Locale: per user, per channel
- Email rendering: HTML and text
- SMS length: 160, 306 (2-part), 459 (3-part)
- SMS split: UDH for multi-part
- Push badge count sync
- Push silent push
- Push critical alert (iOS entitlement)
- Push notification actions (Reply, Snooze)
- Push time-sensitive (iOS 15+)
- Push group notification (Android)
- WhatsApp: list message
- WhatsApp: button message
- WhatsApp: flow message
- WhatsApp: media (image, video, document)
- WhatsApp: sticker
- WhatsApp: location
- WhatsApp: contact
- Voice: AI agent NLU
- Voice: sentiment analysis
- Voice: keyword spotting
- Voice: speech-to-text
- Voice: text-to-speech
- Voice: DTMF detection
- Voice: call recording
- Voice: call transfer
- Voice: voicemail
- Translation: AWS Translate API
- Translation: Google Translate API
- Translation: DeepL API
- Translation: medical terminology (MedLex)
- Translation: RTL handling
- Translation: cache
- Translation: fallback chain
- AI summary of long thread
- AI suggestion for quick reply
- AI escalation detection
- AI sentiment analysis
- AI entity extraction
- AI intent classification
- AI hallucination check
- AI prompt firewall (no PHI leak)
- Realtime: presence
- Realtime: typing indicator
- Realtime: read receipt

### 23.2 Integration Tests (~80)

**Database (15):**
- Notification row created
- Status updates atomic
- Preferences update atomic
- Consent update atomic
- Scheduled notification creates cron entry
- Chat thread + messages atomic
- In-app notification fan-out
- Cross-tenant isolation
- Cascade delete (user delete)
- Soft delete (notification)
- Index usage
- Partition pruning
- Read replica lag
- Encrypted storage (PHI)
- Field-level encryption (consent text)

**Queue/Cache (15):**
- `DispatchNotificationJob` retries on transient
- `DispatchNotificationJob` DLQ on 3 fails
- `ScheduleRunnerJob` every minute
- `BundlingEngineJob` every 5 min
- `EngagementMetricsJob` hourly
- `DeliverabilityMonitorJob` every 15 min
- `ReputationCheckJob` daily
- `TranslationCacheWarmerJob` weekly
- `ConsentAuditJob` monthly
- `DripCampaignJob` daily
- `OptOutSweeperJob` daily
- Redis cache: preferences
- Redis cache: user presence
- Redis cache: smart send time
- Redis cache: A/B variant selection

**Third-party (30):**
- FCM: send, delivery, token refresh
- APNS: send, delivery, badge update
- Twilio: SMS send, status callback
- Twilio: Voice call, recording
- Twilio: WhatsApp send, status
- SendGrid: email send, event webhook
- Mailgun: email send, event webhook
- AWS SES: email send, event webhook
- AWS SNS: SMS send
- AWS Translate: API call
- Google Translate: API call
- DeepL: API call
- Whisper: voice transcription
- OpenAI: intent classification
- OpenAI: chat response generation
- OpenAI: thread summary
- Anthropic: chat response
- Ollama: chat response (dev)
- WhatsApp Business API: template message
- WhatsApp Business API: list message
- WhatsApp Business API: button message
- WhatsApp Business API: flow message
- WhatsApp Business API: media
- Meta Business: HSM template registration
- Meta Business: webhook verification
- LiveKit: video room, recording
- Daily.co: video room
- WebRTC: signaling
- Mail-tester: deliverability score
- GlockApps: inbox placement

**E2E flows (20):**
- Patient registers → welcome email sent
- Patient books appointment → 3 reminders (24h, 1h, on-time)
- Patient pays → receipt email + push
- Patient misses appointment → no-show notification
- Doctor schedules → admin notified
- Lab results ready → patient notified
- Refill request → patient notified
- Insurance denied → patient + staff notified
- Emergency: drug recall → all affected patients notified
- Multi-channel: push failed → SMS fallback
- Multi-device: same notification on phone and web
- Locale: en user gets English, hi user gets Hindi
- Quiet hours: notification deferred to 8 AM
- Bundle: 3 notifications in 1h → 1 digest
- AI chat: patient asks question → AI draft → doctor reviews → sends
- Voice: reminder call → DTMF 2 to reschedule → reschedule scheduled
- Consent: grant marketing → receives marketing email
- Consent: revoke marketing → no more marketing email
- A/B test: variant A vs B → winner determined
- IP warmup: new IP at 50/day → ramp up

### 23.3 HTTP / API Tests (~100)

**Send/Dispatch (15):**
- `POST /api/v3/notifications/dispatch` valid → 200
- `POST /api/v3/notifications/dispatch` invalid user → 422
- `POST /api/v3/notifications/dispatch` rate-limited → 429
- `POST /api/v3/notifications/smart-bundle` → 200
- `GET /api/v3/notifications/digest?period=hourly` → 200
- `GET /api/v3/notifications?type=&from=&to=&read=` → 200
- `PATCH /api/v3/notifications/{id}` mark read → 200
- `POST /api/v3/notifications/{id}/snooze` → 200
- `POST /api/v3/notifications/{id}/archive` → 200
- `POST /api/v3/notifications/bulk-action` → 200
- 1000 notifications → streamed export
- Date range filter fast
- Cross-tenant access → 404
- Admin role required for emergency → 403
- Audit log entry created

**Preferences (10):**
- `GET /api/v3/notifications/preferences` → 200
- `PUT /api/v3/notifications/preferences` → 200
- `GET /api/v3/notifications/preferences/quiet-hours` → 200
- `PUT /api/v3/notifications/preferences/quiet-hours` → 200
- Per-channel opt-in
- Per-category opt-in
- Per-template override
- Locale change
- DST-aware quiet hours
- Timezone validation

**Chat (15):**
- `POST /api/v3/chat/threads` → 201
- `GET /api/v3/chat/threads` → 200
- `GET /api/v3/chat/threads/{id}` → 200
- `POST /api/v3/chat/threads/{id}/messages` → 201
- `POST /api/v3/chat/threads/{id}/read` → 200
- `GET /api/v3/chat/threads/{id}/messages` → 200
- `GET /api/v3/chat/ai-suggest/{thread_id}` → 200
- `POST /api/v3/chat/ai-suggest/{message_id}/accept` → 200
- `POST /api/v3/chat/ai-suggest/{message_id}/edit` → 200
- WebSocket connect → 101
- Typing indicator broadcast
- Read receipt broadcast
- Attachment upload
- Voice message upload
- AI response time p95 < 3s

**Voice (10):**
- `POST /api/v3/voice/calls/initiate` valid → 201
- `POST /api/v3/voice/calls/initiate` no consent → 422
- `POST /api/v3/voice/calls/initiate` DNC match → 422
- `POST /api/v3/voice/calls/initiate` time window → 422
- `POST /api/v3/voice/calls/recording/{id}/transcribe` → 200
- Voice call status: completed
- Voice call status: no-answer
- Voice call status: busy
- Voice call status: failed
- Voice call cost tracked

**Video (5):**
- `POST /api/v3/video-messages` valid → 201
- `GET /api/v3/video-messages/{id}` → 200 with signed URL
- 50 MB video uploaded
- 1 GB video → rejected
- DRM-protected video (V4)

**Scheduled (5):**
- `POST /api/v3/scheduled-notifications` valid cron → 201
- `POST /api/v3/scheduled-notifications` invalid cron → 422
- `GET /api/v3/scheduled-notifications` → 200
- `PATCH /api/v3/scheduled-notifications/{id}` → 200
- `DELETE /api/v3/scheduled-notifications/{id}` → 204

**Emergency (5):**
- `POST /api/v3/emergency/broadcast` admin only → 200
- `POST /api/v3/emergency/broadcast` non-admin → 403
- `GET /api/v3/emergency/broadcasts/{id}/ack` → 200
- Multi-channel broadcast
- Acknowledgment tracked

**Engagement (10):**
- `GET /api/v3/engagement/metrics` → 200
- `GET /api/v3/engagement/best-time` → 200
- A/B test: variant selection deterministic
- A/B test: results after 7 days
- Cohort analysis correct
- Funnel analysis correct
- Revenue attribution correct
- Opt-out rate tracked
- Engagement by locale
- Engagement by device

**Templates (10):**
- `GET /api/v3/templates` → 200
- `POST /api/v3/templates` valid → 201
- `PUT /api/v3/templates/{id}` → 200
- `GET /api/v3/templates/{id}/preview?vars=...` → 200
- `POST /api/v3/templates/{id}/ab-test` → 201
- `GET /api/v3/ab-tests/{id}/results` → 200
- `POST /api/v3/templates/{id}/promote-variant` → 200
- Version rollback
- Locale-specific template
- Channel-specific template

**Consent (5):**
- `GET /api/v3/consent` → 200
- `POST /api/v3/consent` valid → 201
- `DELETE /api/v3/consent/{id}` → 200
- Consent text recorded
- Policy version tracked

**Translation (5):**
- `POST /api/v3/translate` valid → 200
- `POST /api/v3/translate` same locale → 200
- AWS Translate API call
- Google Translate API call
- DeepL API call

**Reports (5):**
- `GET /api/v3/reports/engagement` → 200
- `GET /api/v3/reports/deliverability` → 200
- `GET /api/v3/reports/opt-out` → 200
- Date range filter
- Tenant scope

### 23.4 E2E Tests (~20, Playwright)

**Patient flows (8):**
- Patient books → receives email + push
- Patient opens app → sees notification center
- Patient reads notifications
- Patient snoozes notification
- Patient updates preferences
- Patient sets quiet hours
- Patient grants marketing consent
- Patient uses in-app chat with care team

**Staff flows (6):**
- Doctor receives new appointment notification
- Doctor replies in chat → patient gets push
- Admin sends broadcast → all patients notified
- Admin reviews A/B test results
- Staff updates template
- Staff records voice call

**Admin flows (6):**
- Admin views deliverability dashboard
- Admin sees bounce rate spike → alerts
- Admin views engagement metrics
- Admin manages template library
- Admin configures throttling rules
- Admin runs emergency broadcast

### 23.5 Performance Tests (~10, k6)

- `POST /api/v3/notifications/dispatch` → p95 < 200ms
- `GET /api/v3/notifications` (1000 items) → p95 < 300ms
- WebSocket connection: 10,000 concurrent → no degradation
- Push fan-out: 1M push in < 5 min
- Email send: 100k in < 10 min
- SMS send: 50k in < 5 min
- Voice call initiation: 100 concurrent → p95 < 2s
- Chat message send: p95 < 100ms
- AI chat response: p95 < 3s
- Translation: p95 < 500ms

### 23.6 Security Tests (~20, OWASP)

- SQL injection in notification ID → sanitized
- XSS in template body → escaped
- CSRF on preference update → token
- Auth bypass: no token → 401
- Tenant isolation: tenant A notification → 404 for tenant B
- PHI in URL → blocked
- PHI in logs → sanitized
- Webhook signature verification
- IDOR: chat thread → 404 if not participant
- Mass assignment: `is_admin: true` → ignored
- Privilege escalation: patient role → cannot send broadcast → 403
- Negative score on prompt → blocked
- PHI in AI prompt → redacted
- Rate limiting: 10k notifications/sec → 429
- File upload: malicious PDF → scanned
- Voice upload: malicious audio → scanned
- Spammy consent text → flagged
- GDPR: data export → all notifications included
- GDPR: data deletion → hard delete with audit
- HIPAA: PHI access logged

### 23.7 Compatibility Matrix

- PHP 8.2/8.3/8.4
- Laravel 11/12
- iOS 16+ / Android 10+
- Flutter 3.24/3.27
- WebSocket: RFC 6455
- FCM: API v1
- APNS: HTTP/2
- Twilio: 2010-04-01
- SendGrid: v3
- AWS SES: v2
- WhatsApp Business: Cloud API v18.0
- LiveKit: 1.5+
- Whisper: API or self-hosted

### 23.8 Real-World (~30, with real services in test mode)

- FCM test: send, receive, token refresh
- APNS test: send, badge, action
- Twilio SMS test: send, status callback
- Twilio Voice test: call, DTMF, recording
- Twilio WhatsApp test: template message
- SendGrid test: send, open tracking, click tracking
- AWS SES test: send, bounce, complaint
- AWS Translate test
- Google Translate test
- DeepL test
- Whisper test
- OpenAI test: chat completion
- Anthropic test
- Ollama local test
- WhatsApp Business test
- Meta Business HSM test
- LiveKit Cloud test
- Daily.co test
- Mail-tester: score 10/10
- GlockApps: inbox placement
- Spamhaus check
- DLT registration (India)
- 10DLC brand registration (US)
- Apple Push Notification entitlement
- Android notification channels
- BIMI VMC cert
- TCPA time window test
- DNC list scrub
- STIR/SHAKEN attestation
- GDPR consent flow
- HIPAA minimum-necessary check
- CAN-SPAM footer
- DPDP India consent
- CCPA opt-out
- COPPA age gate

---

## 24. V3+ Deep Dives — Notifications

### 24.1 Deep Dive: Notification Orchestrator

The orchestrator is the brain that decides channel, timing, and content for every notification.

```php
namespace App\Modules\Notifications\Services;

use App\Modules\Notifications\Models\Notification;
use App\Modules\Notifications\Models\UserPreference;
use App\Modules\Notifications\Models\Consent;

class NotificationOrchestrator
{
    public function __construct(
        private readonly ChannelAvailabilityService $availability,
        private readonly SmartSendTimeService $sendTime,
        private readonly NotificationThrottleService $throttle,
        private readonly BundlingEngine $bundling,
        private readonly TemplateRenderer $renderer,
        private readonly AuditService $audit,
    ) {}

    public function dispatch(Notification $notification): DispatchResult
    {
        // 1. Check if user can receive this notification
        if (!$this->canNotify($notification)) {
            return DispatchResult::skipped('user_cannot_receive');
        }

        // 2. Apply throttling rules
        if (!$this->throttle->canSend($notification->user, $notification->category, $notification->channel)) {
            return DispatchResult::deferred('throttled');
        }

        // 3. Check if notification should be bundled
        if ($bundle = $this->bundling->shouldBundle($notification)) {
            return $this->bundling->addToBundle($bundle, $notification);
        }

        // 4. Decide primary + fallback channels
        $channels = $this->decideChannels($notification);

        // 5. Decide send time
        $sendAt = $this->decideSendTime($notification, $channels);

        // 6. Render template for each channel and locale
        $rendered = [];
        foreach ($channels as $channel) {
            $rendered[$channel] = $this->renderer->render(
                $notification->template,
                $notification->vars,
                $notification->user->preferred_locale,
                $channel
            );
        }

        // 7. Dispatch (or schedule)
        if ($sendAt->isPast() || $notification->isUrgent()) {
            return $this->dispatchNow($notification, $channels, $rendered);
        }
        return $this->scheduleDispatch($notification, $channels, $rendered, $sendAt);
    }

    private function canNotify(Notification $notification): bool
    {
        $user = $notification->user;
        $category = $notification->category;

        // Check consent
        $consent = Consent::where('user_id', $user->id)
            ->where('purpose', "channel:{$notification->channel}")
            ->where('granted', true)
            ->whereNull('revoked_at')
            ->first();
        if (!$consent) return false;

        // Check user preferences
        $prefs = $user->notificationPreferences;
        if (!$prefs->category_enabled[$category] ?? true) return false;
        if (!in_array($notification->channel, $prefs->enabled_channels)) return false;

        return true;
    }

    private function decideChannels(Notification $notification): array
    {
        $rules = config("notifications.channel_priority.{$notification->category}");
        $prefs = $notification->user->notificationPreferences;
        $available = $this->availability->getAvailable($notification->user);

        $candidates = collect($rules)
            ->filter(fn($ch) => in_array($ch, $available))
            ->filter(fn($ch) => in_array($ch, $prefs->enabled_channels))
            ->values()
            ->all();

        if (empty($candidates)) {
            // Fall back to all available (per emergency override)
            return $notification->isUrgent() ? $available : ['in_app'];
        }
        return $candidates;
    }

    private function decideSendTime(Notification $notification, array $channels): \Carbon\Carbon
    {
        if ($notification->isUrgent()) {
            return now();
        }

        $now = now();
        $tz = $notification->user->timezone ?? 'UTC';
        $localNow = $now->copy()->setTimezone($tz);
        $localHour = (int)$localNow->format('G');

        // Quiet hours: 10 PM - 8 AM
        $quietStart = $notification->user->preferences->quiet_hours_start ?? 22;
        $quietEnd = $notification->user->preferences->quiet_hours_end ?? 8;

        if ($localHour >= $quietStart || $localHour < $quietEnd) {
            // Defer to quiet hours end
            $sendAt = $localNow->copy()->setTime($quietEnd, 0)->setTimezone('UTC');
            if ($sendAt->isPast()) $sendAt->addDay();
            return $sendAt;
        }

        // Smart send time
        return $this->sendTime->optimalForUser($notification->user, $notification->category, $channels);
    }
}
```

**Key design points:**

1. **Channel priority matrix**: configurable per tenant and per category.
2. **Fallback chain**: if primary fails, try next automatically.
3. **Throttling**: max per channel, per category, per user.
4. **Bundling**: digest non-urgent; urgent bypasses.
5. **Smart send time**: ML-based per user, per channel.
6. **Locale-aware**: every channel respects user's preferred locale.
7. **Audit**: every dispatch decision logged.
8. **Telemetry**: dispatch metrics → Prometheus.

### 24.2 Deep Dive: AI Chat with Care Team

The chat is an AI-assisted conversation with full audit, escalation, and learning.

```php
namespace App\Modules\Notifications\Chat;

use App\Modules\Notifications\Models\ChatThread;
use App\Modules\Notifications\Models\ChatMessage;

class ChatService
{
    public function __construct(
        private readonly LlmProviderResolver $llm,
        private readonly PromptFirewallService $firewall,
        private readonly IntentClassifier $intentClassifier,
        private readonly EntityExtractor $entityExtractor,
        private readonly EscalationService $escalation,
    ) {}

    public function processPatientMessage(ChatThread $thread, string $body, ?array $attachments = null): ChatMessage
    {
        // 1. PHI check
        $phiCheck = $this->firewall->scan($body, ['name', 'dob', 'ssn', 'mrn', 'phone', 'email']);
        if ($phiCheck->hasPHI()) {
            // Redact before sending to LLM
            $redacted = $phiCheck->redact();
            $bodyForLlm = $redacted;
        } else {
            $bodyForLlm = $body;
        }

        // 2. Save patient message
        $patientMessage = $thread->messages()->create([
            'sender_user_id' => $thread->patient->user_id,
            'is_from_patient' => true,
            'body' => $body,
            'attachments' => $attachments,
        ]);

        // 3. Classify intent
        $intent = $this->intentClassifier->classify($bodyForLlm, $thread->ai_context);

        // 4. Extract entities
        $entities = $this->entityExtractor->extract($bodyForLlm);

        // 5. Handle urgent
        if ($intent->isUrgent()) {
            $this->escalation->urgent($thread, $patientMessage, $intent);
            return $patientMessage;
        }

        // 6. Route to specialist
        if ($intent->isSchedulable()) {
            $this->routeToScheduling($thread, $patientMessage, $entities);
            return $patientMessage;
        }
        if ($intent->isRefill()) {
            $this->routeToRefill($thread, $patientMessage, $entities);
            return $patientMessage;
        }

        // 7. Generate AI response
        $aiDraft = $this->llm->for('gpt-4o')->complete([
            'system' => $this->buildSystemPrompt($thread, $intent),
            'user' => $bodyForLlm,
            'context' => $this->buildContext($thread, $entities),
        ]);

        // 8. Post-process
        $cleaned = $this->firewall->unredact($aiDraft, $phiCheck);
        $aiMessage = $thread->messages()->create([
            'is_ai' => true,
            'is_from_patient' => false,
            'body' => $cleaned,
            'ai_metadata' => [
                'intent' => $intent->name,
                'confidence' => $intent->confidence,
                'entities' => $entities,
                'model' => 'gpt-4o',
                'redacted_phi' => $phiCheck->hasPHI(),
            ],
            'requires_review' => $intent->requiresHumanReview(),
        ]);

        // 9. Notify care team
        if ($aiMessage->requires_review) {
            $this->notifyCareTeamForReview($thread, $aiMessage);
        } else {
            // Auto-send after a 30s delay (gives human time to intervene)
            Bus::dispatch(new SendAiChatMessageJob($aiMessage))->delay(now()->addSeconds(30));
        }

        return $patientMessage;
    }

    private function buildSystemPrompt(ChatThread $thread, Intent $intent): string
    {
        $patient = $thread->patient;
        return <<<PROMPT
You are a healthcare AI assistant for {$patient->tenant->name}. The patient is communicating in their preferred language.
You must:
1. Be empathetic, professional, and concise.
2. Never diagnose or recommend specific treatments.
3. For medical advice, always say "I'll have a nurse or your doctor follow up with you."
4. For medication questions, link to the patient's medication list and ask if they need a refill.
5. For appointment questions, link to the patient's upcoming appointments.
6. For billing questions, link to the patient's recent invoices.
7. Never share another patient's information.
8. If the patient expresses suicidal thoughts or self-harm, immediately escalate and provide crisis hotline number.

Patient context:
- Name: {$patient->name}
- DOB: {$patient->dob}
- Preferred language: {$thread->patient->user->preferred_locale}
- Upcoming appointments: {$this->summarizeAppointments($patient)}
- Active medications: {$this->summarizeMedications($patient)}
- Recent lab results: {$this->summarizeLabs($patient)}
PROMPT;
    }

    public function acceptAiDraft(ChatMessage $aiMessage, User $reviewer): ChatMessage
    {
        $aiMessage->update([
            'is_accepted' => true,
            'accepted_by_user_id' => $reviewer->id,
            'accepted_at' => now(),
        ]);
        Bus::dispatch(new SendAiChatMessageJob($aiMessage));
        return $aiMessage;
    }

    public function editAiDraft(ChatMessage $aiMessage, string $editedBody, User $reviewer): ChatMessage
    {
        $aiMessage->update([
            'body' => $editedBody,
            'is_edited' => true,
            'edited_by_user_id' => $reviewer->id,
            'edited_at' => now(),
        ]);
        Bus::dispatch(new SendAiChatMessageJob($aiMessage));
        return $aiMessage;
    }
}
```

**Critical design points:**

1. **PHI firewall**: PII/PHI redacted before sending to LLM. LLM never sees raw patient data unless explicitly needed.
2. **Human-in-the-loop**: medical advice always requires human review. Auto-send only for FAQs.
3. **Intent classification**: routes to specialist workflows (scheduling, refill, billing).
4. **Escalation**: urgent (suicidal, chest pain) → page on-call + crisis hotline.
5. **Audit trail**: every message, every action, every AI generation logged.
6. **Model versioning**: track which model version generated each response.
7. **Feedback loop**: human edits feed back to fine-tuning.
8. **Sentiment analysis**: frustrated patient → escalate to senior care team.
9. **Multi-locale**: AI responds in patient's preferred language.
10. **Patient context**: AI has patient's appointments, medications, labs in context (de-identified).

### 24.3 Deep Dive: Voice Call System

The voice system uses Twilio for outbound calls with TTS, IVR, and recording.

```php
namespace App\Modules\Notifications\Voice;

use Twilio\Rest\Client;

class VoiceCallService
{
    public function __construct(
        private readonly Client $twilio,
        private readonly TranscriptionService $transcription,
        private readonly DncService $dnc,
        private readonly TcpaComplianceService $tcpa,
    ) {}

    public function initiate(VoiceCall $call): VoiceCallResult
    {
        // 1. TCPA: time window
        if (!$this->tcpa->isWithinTimeWindow($call->user)) {
            return VoiceCallResult::deferred('tcpa_time_window');
        }

        // 2. DNC scrub
        if ($this->dnc->isBlocked($call->user->phone)) {
            return VoiceCallResult::blocked('dnc_match');
        }

        // 3. STIR/SHAKEN: sign
        $attestation = $this->getAttestation($call->user, $call->direction);

        // 4. Initiate via Twilio
        $twiml = $this->buildTwiml($call);

        $twilioCall = $this->twilio->calls->create(
            $call->user->phone,         // to
            config('services.twilio.caller_id'),  // from
            [
                'twiml' => $twiml,
                'record' => true,
                'recording_status_callback' => route('webhooks.twilio.recording'),
                'status_callback' => route('webhooks.twilio.status'),
                'status_callback_event' => ['initiated', 'ringing', 'answered', 'completed'],
                'caller_id' => config('services.twilio.caller_id'),
            ]
        );

        $call->update([
            'twilio_call_sid' => $twilioCall->sid,
            'status' => 'initiated',
            'initiated_at' => now(),
        ]);

        return VoiceCallResult::initiated($twilioCall->sid);
    }

    private function buildTwiml(VoiceCall $call): string
    {
        $template = $call->template;
        $vars = $call->vars;
        $message = $this->substitute($template->body, $vars);

        $twiml = '<?xml version="1.0" encoding="UTF-8"?>';
        $twiml .= '<Response>';
        $twiml .= '<Say voice="alice" language="' . $call->user->preferred_locale . '">';
        $twiml .= htmlspecialchars($message);
        $twiml .= '</Say>';

        // IVR menu
        if ($template->has_ivr) {
            $twiml .= '<Gather numDigits="1" action="' . route('webhooks.twilio.ivr') . '" method="POST">';
            foreach ($template->ivr_options as $option) {
                $twiml .= '<Say>' . htmlspecialchars($option->prompt) . ' Press ' . $option->digit . '.</Say>';
            }
            $twiml .= '</Gather>';
        }

        $twiml .= '</Response>';
        return $twiml;
    }

    public function handleStatusCallback(Request $request): void
    {
        $callSid = $request->input('CallSid');
        $status = $request->input('CallStatus');
        $duration = $request->input('CallDuration', 0);

        $call = VoiceCall::where('twilio_call_sid', $callSid)->firstOrFail();
        $call->update([
            'status' => $status,
            'duration_seconds' => $duration,
            'completed_at' => in_array($status, ['completed', 'failed', 'no-answer', 'busy']) ? now() : null,
        ]);

        if ($status === 'no-answer') {
            $this->scheduleRetry($call);
        }
    }

    public function handleRecordingCallback(Request $request): void
    {
        $callSid = $request->input('CallSid');
        $recordingUrl = $request->input('RecordingUrl');
        $recordingSid = $request->input('RecordingSid');

        $call = VoiceCall::where('twilio_call_sid', $callSid)->firstOrFail();
        $call->update([
            'recording_sid' => $recordingSid,
            'recording_url' => $recordingUrl,
        ]);

        // Transcribe
        Bus::dispatch(new TranscribeCallJob($call));
    }
}
```

**Critical design points:**

1. **TCPA compliance**: time window check (8am-9pm local), DNC scrub, consent verification.
2. **STIR/SHAKEN**: attestation header for US calls.
3. **TwiML generation**: dynamic, locale-aware, IVR-capable.
4. **Recording**: optional, encrypted, transcribed.
5. **Transcription**: Whisper API for high accuracy, multi-language.
6. **Retry logic**: no-answer → retry at different time, max 3 attempts.
7. **Status callback**: real-time status updates (initiated → ringing → answered → completed).
8. **Cost tracking**: per-call cost logged.

### 24.4 Deep Dive: WhatsApp 24-Hour Window

The most confusing rule: WhatsApp Business has a 24-hour customer service window.

```php
namespace App\Modules\Notifications\WhatsApp;

class WhatsAppService
{
    public function __construct(
        private readonly WhatsAppApiClient $client,
    ) {}

    public function send(WhatsAppMessage $message): WhatsAppResult
    {
        $thread = $message->thread;
        $lastInboundAt = $thread->last_inbound_at;

        if (!$lastInboundAt || $lastInboundAt->diffInHours(now()) > 24) {
            // Outside customer service window
            // Must use pre-approved template
            return $this->sendTemplate($message);
        }

        // Within customer service window
        // Can send free-form messages
        return $this->sendFreeForm($message);
    }

    private function sendFreeForm(WhatsAppMessage $message): WhatsAppResult
    {
        // Free-form message
        $response = $this->client->sendMessage($message->user->phone, [
            'type' => $message->type,
            strtolower($message->type) => $this->buildBody($message),
        ]);
        return WhatsAppResult::sent($response['messages'][0]['id']);
    }

    private function sendTemplate(WhatsAppMessage $message): WhatsAppResult
    {
        $template = $message->template;
        $vars = $message->vars;

        // Pre-approved HSM (Highly Structured Message)
        $components = $this->buildComponents($template, $vars);
        $response = $this->client->sendTemplate(
            $message->user->phone,
            $template->name,
            $template->language,
            $components
        );
        return WhatsAppResult::sent($response['messages'][0]['id']);
    }

    public function handleInbound(InboundWebhook $webhook): void
    {
        $phone = $webhook->from;
        $user = User::where('phone', $phone)->first();
        if (!$user) return;

        $thread = ChatThread::firstOrCreate([
            'tenant_id' => $user->tenant_id,
            'patient_id' => $user->patient->id,
            'channel' => 'whatsapp',
        ], [
            'status' => 'open',
            'last_inbound_at' => now(),
        ]);

        $thread->update(['last_inbound_at' => now()]);

        $message = $thread->messages()->create([
            'is_from_patient' => true,
            'body' => $webhook->body,
            'attachments' => $webhook->media,
            'external_id' => $webhook->message_id,
        ]);

        // Trigger AI processing
        app(ChatService::class)->processPatientMessage($thread, $webhook->body);
    }
}
```

**Critical design points:**

1. **24-hour window**: after user-initiated conversation, free-form messages allowed for 24h. After that, must use templates.
2. **Template approval**: each template must be pre-approved by Meta; can take 24-48h.
3. **Quality rating**: Meta monitors for spam, low-quality content. Low rating → message limits.
4. **Opt-in mandatory**: patient must opt-in to receive WhatsApp messages.
5. **Webhook handling**: inbound messages processed by `handleInbound`; user identified by phone number.

### 24.5 Deep Dive: Email Deliverability

Email deliverability is hard. We get it right.

**Infrastructure:**
- **Dedicated sending IPs** per high-volume tenant (10k+ emails/month).
- **IP warmup**: 50/day → 100 → 250 → 500 → 1000 → 2500 → 5000 → 10k over 4 weeks.
- **Authenticated sending**: SPF, DKIM, DMARC all configured.
- **BIMI**: VMC cert + DMARC for brand logo in inbox.
- **Multiple sending services**: SendGrid primary, AWS SES backup, Mailgun tertiary.

**`EmailDeliveryService::send(Email $email)`**:
1. Verify SPF, DKIM, DMARC for sending domain.
2. Choose sending IP: dedicated for tenant if available, shared pool otherwise.
3. Set List-Unsubscribe header.
4. Set X-Mailer header.
5. Send via primary service.
6. Track delivery, opens, clicks, bounces, complaints.
7. If bounce: process bounce.
8. If complaint: process complaint.

**Bounce handling:**
- **Hard bounce**: email doesn't exist → suppress immediately.
- **Soft bounce**: mailbox full, server down → retry 3x with exponential backoff over 7 days, then suppress.
- **Block bounce**: domain blocks us → escalate to admin.

**Complaint handling:**
- Spam complaint (user clicks "Report Spam" in Gmail): immediate unsubscribe, suppress.
- Auto-feedback loop: AOL, Yahoo, Comcast, etc.

**Engagement-based:**
- Gmail/Yahoo track engagement. If user hasn't opened in 90 days, our emails go to Promotions or are filtered.
- We monitor: Gmail Postmaster Tools, Microsoft SNDS, Yahoo CFL.
- Auto-re-engagement: dormant users get a "Still want to hear from us?" email. If no engagement in 30 days, suppress.

**Reputation monitoring:**
- Daily check: `Spamhaus`, `Barracuda`, `Spamcop`, `SORBS`, `Invaluement`.
- If IP on blocklist: alert admin, switch to backup IP, request delisting.

### 24.6 Deep Dive: Bundle & Digest Engine

The bundling engine reduces notification fatigue.

```php
namespace App\Modules\Notifications\Bundling;

use App\Modules\Notifications\Models\Notification;

class BundlingEngine
{
    public function __construct(
        private readonly LlmProviderResolver $llm,
    ) {}

    public function shouldBundle(Notification $new): ?Bundle
    {
        $user = $new->user;

        // Don't bundle urgent or emergency
        if ($new->isUrgent() || $new->isEmergency()) return null;

        // Find existing bundle
        $bundle = Bundle::where('user_id', $user->id)
            ->where('status', 'open')
            ->where('expires_at', '>', now())
            ->orderBy('created_at', 'desc')
            ->first();

        // Time-based bundling
        $recentCount = Notification::where('user_id', $user->id)
            ->where('created_at', '>', now()->subHour())
            ->where('status', 'pending')
            ->count();
        if ($recentCount >= 3) {
            return $this->createOrUpdateBundle($user, 'time_based', $new);
        }

        // Type-based bundling
        $sameTypeCount = Notification::where('user_id', $user->id)
            ->where('category', $new->category)
            ->where('created_at', '>', now()->subHour())
            ->where('status', 'pending')
            ->count();
        if ($sameTypeCount >= 2) {
            return $this->createOrUpdateBundle($user, 'type_based', $new);
        }

        // Sender-based bundling
        if ($new->sender_id) {
            $sameSenderCount = Notification::where('user_id', $user->id)
                ->where('sender_id', $new->sender_id)
                ->where('created_at', '>', now()->subHour())
                ->where('status', 'pending')
                ->count();
            if ($sameSenderCount >= 3) {
                return $this->createOrUpdateBundle($user, 'sender_based', $new);
            }
        }

        return $bundle;
    }

    private function createOrUpdateBundle(User $user, string $type, Notification $new): Bundle
    {
        $bundle = Bundle::firstOrCreate([
            'user_id' => $user->id,
            'type' => $type,
            'status' => 'open',
            'created_at' => now()->startOfHour(),
        ], [
            'expires_at' => now()->addHour(),
            'item_count' => 0,
        ]);

        $bundle->increment('item_count');
        $bundle->update(['summary' => null]); // Will be regenerated
        return $bundle;
    }

    public function renderDigest(Bundle $bundle): RenderedBundle
    {
        $items = $bundle->items;
        $user = $bundle->user;

        // AI summary
        if ($bundle->item_count >= 5) {
            $summary = $this->llm->for('gpt-4o-mini')->complete([
                'system' => 'You are a notification summarizer. Summarize the following notifications into a 1-2 sentence push notification title and body. Use the patient\'s preferred language.',
                'user' => $user->preferred_locale,
                'items' => $items->map(fn($n) => ['category' => $n->category, 'subject' => $n->subject, 'body' => $n->body])->toArray(),
            ]);
            $bundle->update(['summary' => $summary, 'ai_generated' => true]);
        } else {
            $summary = [
                'title' => "{$bundle->item_count} new updates",
                'body' => $this->summarizeManually($items),
            ];
            $bundle->update(['summary' => $summary, 'ai_generated' => false]);
        }

        return $this->dispatch($bundle);
    }
}
```

**Critical design points:**

1. **Bundle triggers**: time, type, sender; configurable thresholds.
2. **AI summary**: GPT-4o-mini for cost efficiency; multi-locale.
3. **Deep linking**: each item in the bundle is individually clickable.
4. **Priority respected**: critical/emergency never bundled.
5. **Expiry**: bundle auto-closes after 1h; new items start new bundle.
6. **User override**: user can disable bundling in preferences.

---

## 25. V3+ Precise Plan — Notifications

### 25.1 Phase 1: V1 Push/SMS/Email (Months 0-3, complete)

**Status:** ✅ Complete (sections 1-19)

### 25.2 Phase 2: V2 WhatsApp/In-App/Voice/AI Copy (Months 4-9, complete)

**Status:** ✅ Complete (sections 20-21)

### 25.3 Phase 3: V3 Vault + Smart Routing + Bundling (Months 10-13)

**Goal:** Patient notification preferences, smart channel selection, smart bundling.

**Deliverables:**
- `NotificationOrchestrator` (channel priority matrix, quiet hours, throttling)
- `BundlingEngine` (time/type/sender-based)
- `NotificationCenter` (inbox UI)
- `EngagementMetrics` (per-channel, per-template)
- 15 new V3 endpoints
- 8 acceptance criteria

**Team:** 2 backend engineers, 1 designer (UI), 1 QA

**Dependencies:** Phase 2 complete; 90 days of notification history for training smart send time.

**Risks:**
- **R1**: Channel priority matrix is opinionated → make per-tenant configurable
- **R2**: Bundling may reduce urgency perception → A/B test before rollout
- **R3**: Smart send time is biased toward 8 AM → consider per-locale

**Metrics:** Open rate +15%, click rate +10%, opt-out rate -20%, notification volume -30% (fewer sends, more engagement).

**Cost:** $200k (2 backend × 4 × $10k + 1 designer × 4 × $8k)

### 25.4 Phase 4: V3 Chat + Voice + Video (Months 14-17)

**Goal:** In-app chat with AI assist, voice calls, video messages.

**Deliverables:**
- `ChatService` (threads, messages, AI suggest)
- `VoiceCallService` (Twilio, IVR, recording)
- `VideoMessageService` (S3, signed URLs, AI summary)
- AI integration (GPT-4o for chat, Whisper for transcription)
- 25 new V3 endpoints
- 10 acceptance criteria

**Team:** 3 backend engineers, 1 AI engineer, 1 designer, 1 QA

**Dependencies:** Phase 3 complete; OpenAI/Anthropic API keys; Twilio voice setup.

**Risks:**
- **R1**: AI may hallucinate medical info → always require human review for medical advice
- **R2**: PHI leak via LLM → prompt firewall + redaction
- **R3**: Voice call cost → cap per user per month
- **R4**: Twilio rate limits → batch calls

**Metrics:** Chat usage 30% of patients. Voice call completion rate 70%. AI draft acceptance rate 50% (saves 30% of staff time).

**Cost:** $320k (3 backend × 4 × $10k + 1 AI × 4 × $15k + 1 designer × 4 × $8k)

### 25.5 Phase 5: V3 Deliverability + Consent + Compliance (Months 18-21)

**Goal:** Production-grade deliverability, full consent management, full compliance.

**Deliverables:**
- Dedicated sending IPs with warmup
- `ConsentService` (TCPA, GDPR, DPDP, PDPA, CAN-SPAM, COPPA, CCPA)
- DLT registration (India)
- 10DLC brand + campaign (US)
- WhatsApp HSM template library
- `DeliverabilityMonitor`
- 15 new V3 endpoints
- 12 acceptance criteria

**Team:** 2 backend engineers, 1 deliverability specialist, 1 QA, 1 legal/compliance consultant

**Dependencies:** Phase 4 complete; IP warmup takes 4 weeks; DLT/10DLC approval takes 4-8 weeks.

**Risks:**
- **R1**: IP warmup fails → fallback to shared IP
- **R2**: DLT/10DLC delayed → defer India/US-specific features
- **R3**: GDPR/DPDP fines → exhaustive audit trail
- **R4**: Deliverability regression → continuous monitoring

**Metrics:** Inbox placement > 95%. Bounce rate < 2%. Spam complaint rate < 0.1%. Open rate > 30%.

**Cost:** $280k (2 backend × 4 × $10k + deliverability specialist × 4 × $12k + consultant $30k + IP warmup $10k)

### 25.6 Phase 6: V3 AI Personalization + Templates + Builder (Months 22-25)

**Goal:** AI-driven personalization, template builder, A/B testing.

**Deliverables:**
- AI notification copy generation (per user, per locale)
- Template builder UI (admin)
- A/B test framework (auto-promote winner)
- Smart send time per user
- Engagement analytics dashboards
- 20 new V3 endpoints
- 12 acceptance criteria

**Team:** 2 backend engineers, 1 AI engineer, 1 designer, 1 QA

**Dependencies:** Phase 5 complete; 6 months of engagement data for ML model.

**Risks:**
- **R1**: AI copy generation may be off-brand → guardrails + human review for first 6 months
- **R2**: A/B test statistical significance → wait for 95% confidence
- **R3**: Smart send time privacy → opt-in per user

**Metrics:** Personalization +10% engagement. Template reuse +30%. A/B test winner deployment 50+/month.

**Cost:** $260k (2 backend × 4 × $10k + 1 AI × 4 × $15k + 1 designer × 4 × $8k)

### 25.7 Phase 7: V3 Emergency + Scheduled + Realtime Scale (Months 26-30)

**Goal:** Emergency broadcasts, scheduled notifications, scale to 1M concurrent.

**Deliverables:**
- `EmergencyBroadcastService` (multi-channel, ack tracking)
- `ScheduledNotificationService` (cron per user)
- Realtime scale to 100k WebSocket
- Push fan-out to 1M in <5 min
- 20 new V3 endpoints
- 10 acceptance criteria

**Team:** 3 backend engineers, 1 devops, 1 QA

**Dependencies:** Phase 6 complete; load test environment with 1M users.

**Risks:**
- **R1**: Push fan-out throttle → coordinate with FCM/APNS
- **R2**: Realtime scale → Redis pub/sub + multiple Reverb instances
- **R3**: Emergency broadcast misuse → admin role + 4-eyes approval
- **R4**: Scheduled notifications at scale → distributed cron

**Metrics:** Emergency broadcast reach 100% within 5 min. Ack rate > 80%. Realtime p95 < 100ms.

**Cost:** $320k (3 backend × 4 × $10k + 1 devops × 4 × $12k + load test $20k)

### 25.8 Cross-Cutting Concerns

**Security & Compliance:**
- HIPAA-compliant PHI handling (firewall + redaction)
- GDPR Art. 17 right to be forgotten
- TCPA time window + DNC scrub
- CAN-SPAM unsubscribe
- WCAG 2.1 AA accessibility for in-app UI
- WCAG 2.1 AAA for email templates

**Performance:**
- 1M push/min via FCM/APNS
- 100k WebSocket concurrent
- p95 < 200ms for any API call
- 99.95% uptime

**Observability:**
- All sends logged to audit
- All channel failures logged
- All bounces and complaints processed
- Engagement metrics Prometheus + Grafana
- PagerDuty for critical alerts (deliverability regression, webhook down)

**Disaster Recovery:**
- Multi-region active-active
- RPO 1 min (Redis AOF)
- RTO 5 min
- 4-hour DR drill quarterly

**Data Retention:**
- Notification content: 90 days (then archive to S3 Glacier)
- Audit log: 7 years (HIPAA)
- Consent: indefinite (with revocation)
- Engagement metrics: 2 years

### 25.9 Key Milestones

| Month | Milestone |
|---|---|
| 0 | V1 push/SMS/email GA |
| 4 | V2 WhatsApp GA |
| 6 | V2 in-app chat GA |
| 9 | V2 voice + AI copy GA |
| 13 | V3 vault + smart routing GA |
| 17 | V3 chat + voice + video GA |
| 21 | V3 deliverability + consent + compliance GA |
| 25 | V3 AI personalization + templates GA |
| 30 | V3 emergency + scheduled + scale GA |
| 33 | V3 stable; V4 begins (voice biometrics, RCS universal) |

### 25.10 Success Metrics (V3+ Notifications)

- **Engagement**: open rate > 40% (industry avg 25%), click rate > 8% (industry avg 4%)
- **Reach**: 95% of users receive critical notifications within 5 min
- **Opt-out rate**: < 2% per channel per month
- **Cost per notification**: $0.001 average (multi-channel blended)
- **Compliance**: 100% of sends have consent record; 0 GDPR/TCPA fines
- **Engineering metrics:**
  - 99.95% uptime
  - p95 dispatch < 200ms
  - 1M push/min sustained
  - 0 PHI leaks in AI prompts

(End of file)
