# 17 — Realtime (Reverb, Channels, Events, Client Subscriptions)

> The realtime layer. Uses Laravel Reverb (WebSocket server) for push-style updates to connected clients. REST is still the source of truth; Reverb is the "instant feel" layer on top.

---

## 1. Why Reverb

- First-party Laravel WebSocket server (no third-party SaaS per-message cost).
- Scales via Redis pub/sub.
- Single auth layer (Sanctum + Reverb token).
- Reuses Laravel's broadcasting events & listeners.

Alternatives considered:
- **Pusher** — per-message pricing adds up at our scale.
- **Ably** — similar.
- **Socket.io + custom** — more code to maintain.
- **Server-Sent Events (SSE)** — one-way, no good mobile support.
- **Long-polling** — works but feels sluggish.

Reverb is the best fit for our self-hosted, EU-data-residency model.

---

## 2. Channel Naming Convention

```
{scope}.{tenant|context}.{resource}.{id?}

Private channels (require auth):
  user.{user_id}                         — anything for a specific user
  doctor.{doctor_id}                     — doctor app
  patient.{patient_id}                   — patient's own channel (any device of user)
  clinic.{tenant_id}.{resource}          — clinic-wide topics
  tenant.{tenant_id}.{resource}          — tenant-level (when clinic = tenant)

Public channels (no auth):
  public.{tenant}.doctors
  public.{tenant}.banners
  public.{tenant}.queue.{clinic_id}      — queue display
```

### 2.1 Channel Catalog

| Channel | Type | Auth | Listeners |
|---|---|---|---|
| `user.{id}` | private | user must be `{id}` | The user's own devices |
| `doctor.{id}` | private | must be the doctor OR staff in same tenant | Doctor app + admin |
| `patient.{id}` | private | user must have `can_view_records` for this patient | Patient app (own) + linked family + assigned doctor + staff |
| `clinic.{tenant}.staff` | private | any staff user in tenant | Admin web |
| `clinic.{tenant}.dashboard` | private | any staff user in tenant | Admin web |
| `clinic.{tenant}.admins` | private | admin or super-admin only | Admin bell |
| `clinic.{id}.queue` | public | none | TV display screens in waiting room |
| `clinic.{tenant}.queue.{clinic_id}` | public | none | Multi-clinic display |
| `tenant.{tenant}.{resource}` | private | staff in tenant | Multi-tenant scenarios |
| `public.{tenant}.doctors` | public | none | Public website |
| `public.{tenant}.banners` | public | none | Public website |

### 2.2 Channel Authorization

Defined in `routes/channels.php`:

```php
Broadcast::channel('user.{id}', function (User $user, int $id) {
    return $user->id === $id;
});

Broadcast::channel('doctor.{id}', function (User $user, int $id) {
    $doctor = Doctor::find($id);
    if (!$doctor || $doctor->tenant_id !== Tenant::id()) return false;
    return $user->id === $doctor->user_id
        || $user->can('view-doctors');
});

Broadcast::channel('patient.{id}', function (User $user, int $id) {
    $patient = Patient::find($id);
    if (!$patient || $patient->tenant_id !== Tenant::id()) return false;
    if ($user->can('view-patients')) return true;     // staff
    return $patient->userPatients()
        ->where('user_id', $user->id)
        ->where('can_view_records', true)
        ->exists();
});

Broadcast::channel('clinic.{tenant}.staff', function (User $user, int $tenant) {
    return $user->tenant_id === $tenant;
});
```

---

## 3. Event Catalog

Every event has a:
- Class implementing `ShouldBroadcast` or `ShouldBroadcastNow`.
- Channel(s) it broadcasts on.
- Payload (always JSON-serializable, no PHP objects).
- `broadcastAs()` for the wire name (snake.case).
- `broadcastQueue()` for the queue name (default = `broadcasts`).

### 3.1 Identity & Auth Events

| Event | Class | Channel | Wire Name | Payload |
|---|---|---|---|---|
| `UserLoggedIn` | `UserLoggedIn` | `user.{id}` | `user.logged_in` | `{ device_type, ip, at }` (other-device alert) |
| `UserSuspended` | `UserSuspended` | `user.{id}` | `user.suspended` | `{ reason }` |
| `UserRoleAssigned` | `UserRoleAssigned` | `user.{id}` | `user.role_assigned` | `{ role, permissions[] }` |
| `UserPasswordChanged` | `UserPasswordChanged` | `user.{id}` | `user.password_changed` | `{ at }` |

### 3.2 Appointment Events

| Event | Class | Channel(s) | Wire Name | Payload |
|---|---|---|---|---|
| `AppointmentBooked` | `AppointmentBooked` | `patient.{id}`, `doctor.{id}`, `clinic.{tenant}.staff` | `appointment.booked` | full appointment |
| `AppointmentConfirmed` | `AppointmentConfirmed` | `patient.{id}`, `doctor.{id}` | `appointment.confirmed` | full appointment |
| `AppointmentCheckedIn` | `AppointmentCheckedIn` | `clinic.{id}.queue`, `doctor.{id}`, `patient.{id}` | `appointment.checked_in` | `{appointment, queue_token}` |
| `AppointmentCalled` | `AppointmentCalled` | `clinic.{id}.queue`, `patient.{id}` | `appointment.called` | `{appointment, queue_token, room}` |
| `AppointmentStarted` | `AppointmentStarted` | `clinic.{id}.queue`, `patient.{id}` | `appointment.started` | full appointment |
| `AppointmentCompleted` | `AppointmentCompleted` | `patient.{id}`, `doctor.{id}` | `appointment.completed` | full appointment |
| `AppointmentCancelled` | `AppointmentCancelled` | `patient.{id}`, `doctor.{id}`, `clinic.{tenant}.staff` | `appointment.cancelled` | `{appointment, refund?}` |
| `CancellationRequested` | `CancellationRequested` | `clinic.{tenant}.staff`, `clinic.{tenant}.admins` | `appointment.cancellation_requested` | full cancellation |
| `CancellationApproved` | `CancellationApproved` | `patient.{id}` | `appointment.cancellation_approved` | `{appointment, refund}` |
| `CancellationRejected` | `CancellationRejected` | `patient.{id}` | `appointment.cancellation_rejected` | `{appointment, reason}` |
| `AppointmentRescheduled` | `AppointmentRescheduled` | `patient.{id}`, `doctor.{id}` | `appointment.rescheduled` | `{old, new}` |
| `NoShowMarked` | `NoShowMarked` | `patient.{id}`, `doctor.{id}` | `appointment.no_show` | full appointment |
| `SlotHeld` | `SlotHeld` | `public.{tenant}.doctors` | `slot.held` | `{doctor_id, slot_id}` (cache invalidation) |
| `SlotReleased` | `SlotReleased` | `public.{tenant}.doctors` | `slot.released` | `{doctor_id, slot_id}` |

### 3.3 Prescription Events

| Event | Channel | Wire Name | Payload |
|---|---|---|---|
| `PrescriptionCreated` | `patient.{id}` | `prescription.created` | `{id, doctor_name}` |
| `PrescriptionSigned` | `patient.{id}` | `prescription.signed` | `{id, doctor_name, url}` |
| `PrescriptionAmended` | `patient.{id}` | `prescription.amended` | `{id, new_id}` |
| `PrescriptionShared` | `patient.{id}` | `prescription.shared` | `{id, shared_at, expires_at}` |
| `PrescriptionAccessed` | (none — just audit log) | — | — |

### 3.4 Billing & Payment Events

| Event | Channel | Wire Name | Payload |
|---|---|---|---|
| `InvoiceCreated` | `patient.{id}`, `clinic.{tenant}.staff` | `invoice.created` | `{id, amount, currency}` |
| `InvoicePaid` | `patient.{id}`, `clinic.{tenant}.staff` | `invoice.paid` | `{id, amount}` |
| `InvoiceVoided` | `patient.{id}`, `clinic.{tenant}.staff` | `invoice.voided` | `{id, reason}` |
| `PaymentCaptured` | `user.{id}` (patient) | `payment.captured` | `{id, amount, invoice_id}` |
| `PaymentFailed` | `user.{id}` (patient) | `payment.failed` | `{id, reason}` |
| `RefundProcessed` | `user.{id}` (patient) | `refund.processed` | `{id, amount}` |
| `CouponUsage` | `clinic.{tenant}.staff` | `coupon.used` | `{coupon_id, count}` |

### 3.5 Patient & Family Events

| Event | Channel | Wire Name | Payload |
|---|---|---|---|
| `PatientCreated` | `clinic.{tenant}.staff` | `patient.created` | `{id, name, mrn}` |
| `PatientUpdated` | `clinic.{tenant}.staff` | `patient.updated` | `{id, changed_fields[]}` |
| `VitalRecorded` | `patient.{id}` | `vital.recorded` | `{id, metric, value, recorded_at}` |
| `VitalsAbnormal` | `patient.{id}`, `doctor.{id}` (if assigned) | `vitals.abnormal` | `{id, metric, value, ref_range}` |
| `FileUploaded` | `patient.{id}` | `file.uploaded` | `{id, category, title}` |
| `FileDeleted` | `patient.{id}` | `file.deleted` | `{id}` |
| `PatientLinked` | `user.{id}` | `patient.linked` | `{patient_id, relation}` |
| `PatientUnlinked` | `user.{id}` | `patient.unlinked` | `{patient_id}` |

### 3.6 Doctor Events

| Event | Channel | Wire Name | Payload |
|---|---|---|---|
| `DoctorProfileUpdated` | `public.{tenant}.doctors` | `doctor.profile_updated` | `{id}` (cache invalidate) |
| `DoctorScheduleUpdated` | `public.{tenant}.doctors`, `doctor.{id}` | `doctor.schedule_updated` | `{id}` |
| `DoctorNewReview` | `doctor.{id}` | `doctor.new_review` | `{id, rating, patient_first_name}` |

### 3.7 Notification Events

| Event | Channel | Wire Name | Payload |
|---|---|---|---|
| `NotificationCreated` | `user.{id}` | `notification.created` | full notification |
| `NotificationRead` | `user.{id}` (other devices) | `notification.read` | `{id}` (for badge sync) |
| `UnreadCountChanged` | `user.{id}` | `notification.unread_count` | `{count}` |

### 3.8 System Events

| Event | Channel | Wire Name | Payload |
|---|---|---|---|
| `SettingsChanged` | `clinic.{tenant}.staff` | `settings.changed` | `{group}` |
| `DashboardCountsUpdated` | `clinic.{tenant}.dashboard` | `dashboard.updated` | `{today_count, today_revenue, ...}` |
| `NewAuditAlert` | `clinic.{tenant}.admins` | `audit.alert` | `{event, count, message}` |
| `TestimonialApproved` | `user.{id}` (if submitted) | `testimonial.approved` | `{id}` |
| `BannerActivated` | `public.{tenant}.banners` | `banner.activated` | `{id}` |

---

## 4. Event Class Anatomy

```php
namespace App\Modules\Appointments\Events;

class AppointmentBooked implements ShouldBroadcast {
    use SerializesModels;
    
    public function __construct(public Appointment $appointment) {}
    
    public function broadcastOn(): array {
        return [
            new PrivateChannel('patient.' . $this->appointment->patient_id),
            new PrivateChannel('doctor.' . $this->appointment->doctor_id),
            new PrivateChannel('clinic.' . $this->appointment->tenant_id . '.staff'),
        ];
    }
    
    public function broadcastAs(): string {
        return 'appointment.booked';
    }
    
    public function broadcastWith(): array {
        return (new AppointmentResource($this->appointment->fresh(['patient', 'doctor', 'clinic'])))->resolve();
    }
    
    public function broadcastQueue(): string {
        return 'broadcasts';
    }
}
```

**Key rules:**
- The constructor receives the **Eloquent model** (serialized via trait).
- `broadcastWith()` returns a plain array (no Models), going through an API Resource for consistency.
- `broadcastQueue` ensures the broadcast itself goes through a queue (not blocking the HTTP request).

---

## 5. Broadcasting Queue

All broadcasts dispatched on the `broadcasts` queue, which is processed by a dedicated worker:

```
# /etc/supervisor/conf.d/medicare-broadcasts.conf
[program:medicare-broadcasts]
process_name=%(program_name)s_%(process_num)02d
command=php /home/forge/medicare-api/artisan queue:work redis --queue=broadcasts --sleep=1 --tries=3 --max-time=3600
numprocs=2
autostart=true
autorestart=true
```

This decouples event publishing from request handling. A slow Reverb doesn't slow down the API.

---

## 6. Client Integration

### 6.1 Flutter (using `laravel_echo` + `pusher` packages)

```dart
// lib/core/realtime_service.dart
class RealtimeService {
  late final Echo _echo;
  
  void init(String apiBase, String authToken) {
    _echo = Echo(
      broadcaster: EchoBroadcasterType.Reverb,
      options: EchoOptions(
        broadcaster: 'reverb',
        key: 'medicare-reverb-key',
        wsHost: apiBase.replaceAll('https://', ''),
        wsPort: 8080,
        wssScheme: 'https',
        authEndpoint: '$apiBase/api/v1/broadcasting/auth',
        authHeaders: {'Authorization': 'Bearer $authToken', 'Accept': 'application/json'},
      ),
    );
  }
  
  Stream<ListenEvent> subscribeUserChannel(int userId) {
    return _echo.private('user.$userId')
        .map((e) => ListenEvent(event: e.event!, data: e.payload));
  }
}
```

**Subscriptions in the patient app** (on login):

```dart
realtime.subscribeUserChannel(currentUser.id).listen((e) {
  switch (e.event) {
    case 'appointment.booked':
      ref.read(appointmentsProvider.notifier).refresh();
      showSnackbar('Appointment booked!');
      break;
    case 'notification.created':
      ref.read(notificationsProvider.notifier).prepend(e.data);
      break;
    case 'appointment.called':
      ref.read(queueProvider.notifier).onCalled(e.data);
      showFullScreenAlert('You are being called!', e.data);
      break;
    // ...
  }
});
```

### 6.2 Next.js (using `laravel-echo` JS package)

```typescript
// lib/realtime.ts
import Echo from 'laravel-echo';
import Pusher from 'pusher-js';

window.Pusher = Pusher;

export const echo = new Echo({
  broadcaster: 'reverb',
  key: process.env.NEXT_PUBLIC_REVERB_KEY!,
  wsHost: process.env.NEXT_PUBLIC_REVERB_HOST!,
  wsPort: process.env.NEXT_PUBLIC_REVERB_PORT!,
  wssPort: process.env.NEXT_PUBLIC_REVERB_PORT!,
  forceTLS: true,
  authEndpoint: '/api/broadcasting/auth',
  auth: { headers: { Authorization: `Bearer ${getToken()}` } },
});

// In a page:
useEffect(() => {
  const channel = echo.private(`clinic.${tenantId}.dashboard`);
  channel.listen('.dashboard.updated', (e) => {
    setCounts(e);
  });
  return () => { channel.stopListening('.dashboard.updated'); };
}, []);
```

### 6.3 Public Display Page (no auth)

```javascript
// /display/{clinic-slug}
const echo = new Echo({ broadcaster: 'reverb', /* ... no auth ... */ });
echo.channel(`clinic.${clinicId}.queue`)
    .listen('.appointment.called', (e) => {
      flash(`${e.queue_token.token_number} - ${e.patient_first_name} → ${e.room}`);
    });
```

---

## 7. Connection Lifecycle

### 7.1 Mobile App

1. **App opens** → connect to Reverb with current Sanctum token.
2. **Subscribe** to `user.{id}` and any active context channels (e.g., current appointment's `doctor.{id}`).
3. **App backgrounds** → Reverb disconnects (OS kills socket).
4. **App foregrounds** → reconnect + re-subscribe.
5. **Token refresh** → on 401 from `broadcasting/auth`, refresh Sanctum token, reconnect.
6. **Logout** → disconnect, clear subscriptions.

### 7.2 Web Admin

1. **Page load** → connect.
2. **Subscribe** to all channels relevant to current page.
3. **Page navigation** → unsubscribe from old, subscribe to new.
4. **Tab inactive >5 min** → Reverb may disconnect (browsers throttle); reconnect on focus.

### 7.3 Public Display (TV)

- Always connected (the browser is dedicated).
- Auto-reconnect on disconnect with exponential backoff.
- Page is fullscreen, no other tabs.

---

## 8. Authentication for Reverb

Reverb uses the same `auth/broadcasting` endpoint Sanctum uses for SPAs. The `POST /api/v1/broadcasting/auth` endpoint accepts a `{socket_id, channel_name}` body and returns the auth signature.

For **private** channels, the channel's closure in `routes/channels.php` is called with the current user, and must return `true`/`false` (or a user array for presence).

For **presence** channels (V2 — who's online in the admin panel), the closure returns a user array `{id, name, avatar_url}`.

---

## 9. Connection Limits & Scaling

- **Per-Reverb-instance cap:** 1,000 concurrent connections.
- **Horizontal scale:** add more Reverb instances behind a load balancer; Redis pub/sub fans out events to all.
- **Memory per connection:** ~30 KB. So 1,000 connections ≈ 30 MB.
- **CPU per connection:** minimal at idle; spikes on broadcast.

### 9.1 Multi-Instance Topology

```
                   ┌─────────────┐
                   │   NGINX LB  │ (sticky session by socket id)
                   └──────┬──────┘
            ┌─────────────┼─────────────┐
            ▼             ▼             ▼
      ┌──────────┐  ┌──────────┐  ┌──────────┐
      │ Reverb 1 │  │ Reverb 2 │  │ Reverb 3 │  ... up to 5
      └────┬─────┘  └────┬─────┘  └────┬─────┘
           └─────────────┼─────────────┘
                         ▼
                   ┌──────────┐
                   │   Redis  │  (pub/sub fanout)
                   └──────────┘
```

The Laravel API publishes events to Redis; all Reverb instances receive and forward to their connected sockets.

---

## 10. Reconnection & Resync

WebSocket connections drop. When a client reconnects:

1. Client sends `connection-established` event.
2. Client should re-fetch data (REST) to fill the gap.
3. Server doesn't replay missed events (WebSocket isn't durable).

**For critical state**, the client always re-fetches on reconnect:
- Queue display: re-fetch `GET /public/clinics/{id}/queue` on reconnect.
- Patient app: re-fetch appointments on reconnect.
- Admin: re-fetch dashboard on reconnect.

This is why REST is still the source of truth; Reverb is just a "feels instant" layer.

---

## 11. Fallback: Polling

If Reverb is unavailable (server down, network issue), clients fall back to polling:

| Client | Polling Interval | When Reverb is down |
|---|---|---|
| Patient app — appointments | 30s | When no events for 60s, switch to 30s polling |
| Patient app — notifications | 60s | Same |
| Admin dashboard | 15s | Same |
| Queue display | 5s | Same |
| Doctor today | 30s | Same |

The polling is implemented in the same `RealtimeService` so the rest of the app is unaware.

### 11.1 Detection

The client uses a heartbeat ping every 30s. If 2 pings are missed → mark as disconnected → start polling. When Reverb comes back, switch back automatically.

---

## 12. Performance Considerations

### 12.1 Don't Broadcast Huge Payloads

The wire is precious. A 100 KB broadcast × 1,000 connections = 100 MB sent per event. Rules:

- Payload ≤ 4 KB.
- Use IDs + REST endpoints for detail.
- Compute display data client-side; don't send pre-rendered HTML.

### 12.2 Throttle High-Frequency Events

- `dashboard.updated` should be debounced to 1 per second max (use `Cache::throttle`).
- `vital.recorded` is throttled to 1 per 5 seconds per patient (if a patient enters 5 vitals in 30s, send 1 broadcast at the end).

### 12.3 Don't Broadcast to Everyone

`private` channels ensure only authorized users receive. Never use `public` channels for anything sensitive.

### 12.4 Compress

Reverb supports permessage-deflate. Always enabled.

---

## 13. Monitoring

### 13.1 Metrics

Export to Prometheus (via `promphp/prometheus_client_php`):

- `reverb_connections_total{tenant="..."}` — gauge
- `reverb_events_broadcast_total{event="..."}` — counter
- `reverb_broadcast_duration_seconds{event="..."}` — histogram
- `reverb_auth_failures_total` — counter (security)

### 13.2 Health

`GET /health/ready` checks Reverb:
- TCP connect to Reverb port succeeds.
- Auth endpoint returns 200 with a test token.

### 13.3 Alerts

- Reverb connection count > 800 per instance → warning.
- Reverb auth failures > 100/hour → Sentry alert.
- Reverb down > 5 min → PagerDuty.

---

## 14. Reverb Server Hardening

- Run as non-root `forge` user.
- Bind to `127.0.0.1`; NGINX fronts with TLS + WSS.
- Reverb max payload: 64 KB (reject larger).
- Connection rate limit: 10 connections per IP per minute.
- Idle timeout: 5 minutes (force reconnect).
- Allowed origins: configured per env (`REVERB_ALLOWED_ORIGINS`).

---

## 15. Testing Strategy

| Test | Assertion |
|---|---|
| `BroadcastAuthTest` | Valid token + allowed channel → 200; invalid token → 403; user trying to subscribe to other user's channel → 403 |
| `EventBroadcastTest` | Event fires → assertion via `Event::fake()` and `Socket::fake()` |
| `CrossTenantBroadcastTest` | Event in tenant A never reaches a subscriber in tenant B |
| `ChannelAuthPerformanceTest` | Auth response in <50ms p95 |
| `ReconnectResyncTest` | Client reconnects → next event received within 1s |
| `FallbackPollingTest` | Reverb down → client switches to polling in <60s |
| `BroadcastQueueTest` | Slow broadcast doesn't slow HTTP request |
| `PrivateChannelLeakTest` | User subscribed to `patient.42` cannot listen if they lose `can_view_records` |
| `PresenceChannelTest` | V2: presence events fire correctly |

---

## 16. V2 Enhancements

- **Presence channels** — show who's online in the admin team chat.
- **WebRTC for video** instead of Zoom for low-stakes calls.
- **CRDT-based collaborative editing** for shared prescriptions (V3).
- **Server-Sent Events fallback** for environments where WebSocket is blocked (corporate firewalls).

---

## 17. Acceptance Criteria (V1)

- [ ] All events in the catalog are implemented and broadcast.
- [ ] No PHI is broadcast on a public channel.
- [ ] Cross-tenant events are never received.
- [ ] Mobile app receives events within 500ms of the triggering action.
- [ ] Queue display updates within 1s of front-desk call.
- [ ] Reconnect works without manual intervention.
- [ ] Reverb down → clients fall back to polling.
- [ ] Reverb up → clients fall back to WebSocket.
- [ ] All broadcasts go through the `broadcasts` queue (never inline).
- [ ] Prometheus metrics are exported.
- [ ] Health check detects Reverb down.

---

## 18. V2 Roadmap — Realtime

> V1 ships Laravel Reverb for queue display, in-app notifications, and live calling. V2 adds **presence**, **WebRTC video signaling**, **in-app chat**, **AI streaming**, and **horizontal scale to 100k concurrent connections**.

### 18.1 Presence channels (`presence-*`)

V2 introduces presence channels for "who's online":

- `presence.clinic.{clinic_id}` — all staff/doctors in a clinic.
- `presence.doctor.{doctor_id}` — patient and doctor in a consultation.
- `presence.chat.{chat_id}` — both chat participants.
- `presence.admin.{tenant_id}` — all admin users.

```php
Broadcast::channel('presence.clinic.{clinicId}', function ($user, $clinicId) {
    return ['id' => $user->id, 'name' => $user->name, 'role' => $user->role]
        + ($user->canAccessClinic($clinicId) ? [] : false);
});
```

Clients subscribe and receive:
- `here` — initial list of current members.
- `joining` — when someone joins.
- `leaving` — when someone leaves.

Used for:
- "Doctor is online — start video call" indicator.
- Admin team chat: "3 staff online".
- Patient app: "Doctor will respond in ~5 min" (based on presence + last activity).

### 18.2 In-app chat realtime (`chat.{chat_id}`)

V2's `in_app_chats` is realtime-first:

```php
// app/Events/ChatMessageSent.php
class ChatMessageSent implements ShouldBroadcast {
    public function __construct(public InAppMessage $message) {}
    public function broadcastOn(): array {
        return [new PrivateChannel("chat.{$this->message->chat_id}")];
    }
    public function broadcastAs(): string { return 'message.sent'; }
}
```

Channel name: `chat.{chat_id}` (private). Both participants subscribe on opening the chat. Typing indicators:

```php
// app/Events/ChatTyping.php
class ChatTyping implements ShouldBroadcast {
    public function broadcastOn(): array {
        return [new PrivateChannel("chat.{$this->chatId}")];
    }
    public function broadcastAs(): string { return 'typing'; }
}
```

Read receipts: when a participant opens the chat, the client dispatches `POST /chat/chats/{id}/read` which updates the messages' `read_at` and broadcasts `ChatRead`.

### 18.3 Video signaling (WebRTC over Reverb)

For telemedicine, V2 uses **LiveKit Cloud** (primary) — it handles WebRTC, TURN, recording, and adaptive bitrate out of the box. Reverb is used for the **signaling layer** (room create, participant join/leave, offer/answer exchange for self-hosted fallback).

Flow:
1. Appointment confirmed → `VideoRoomService::provision()` creates a LiveKit room, stores `room_sid`.
2. Patient/doctor app requests `GET /video/rooms/{id}/token` → JWT scoped to room and role.
3. LiveKit SDK handles the WebRTC handshake; the call begins.
4. End: `POST /video/rooms/{id}/end` → LiveKit terminates room; recording uploaded to S3.
5. Reverb broadcasts `video.room.ended` to both participants for cleanup.

For self-hosted (V2.1), we use **mediasoup** + **coturn** TURN. Reverb is the signaling channel.

**coturn configuration:**
```
listening-port=3478
tls-listening-port=5349
realm=turn.medicare.app
auth-secret={APP_KEY_HMAC_SECRET}
lt-cred-mech
```

`turnUsername = HMAC-SHA1(appKey, expiryTimestamp)` — short-lived (1h) credentials.

### 18.4 AI streaming (`ai.stream`)

When the AI scribe or symptom checker responds, the LLM streams tokens. We bridge this to Reverb:

```php
// app/Services/AiStreamService.php
class AiStreamService {
    public function stream(Encounter $encounter, string $prompt): Generator {
        $stream = $this->llm->streamComplete($prompt);
        foreach ($stream as $token) {
            broadcast(new AiToken($encounter->id, $token))->toOthers();
            yield $token;
        }
        broadcast(new AiDone($encounter->id));
    }
}
```

The doctor's UI shows the SOAP note being typed in real time.

### 18.5 Horizontal scale

V1 Reverb is single-node (Hetzner CCX23 handles 1k connections). V2 SaaS multi-tenant needs 100k+ connections. The topology:

```
                  ┌─────────────┐
                  │   ALB / GLB │
                  └──────┬──────┘
                         │
        ┌────────────────┼────────────────┐
        ▼                ▼                ▼
   Reverb Pod 1     Reverb Pod 2     Reverb Pod N
   (1k conn)        (1k conn)        (1k conn)
        │                │                │
        └────────────────┼────────────────┘
                         ▼
                  ┌──────────────┐
                  │  Redis Pub/Sub│  (channel fanout)
                  └──────────────┘
```

- **Reverb** uses Redis as the cross-pod pub/sub.
- **Sticky sessions** in ALB (session affinity) so a client reconnects to the same pod.
- **Health checks:** `/health` on each pod; ALB removes unhealthy pods.
- **Auto-scale:** HPA on connection count (>800 per pod → add a pod).
- **Max connections per pod:** 5,000 (after tuning). 100k total = 20 pods.

### 18.6 SSE fallback

When WebSocket is blocked (corporate proxies, China firewall), we fall back to **Server-Sent Events**:

```php
// app/Http/Controllers/SseController.php
Route::get('/sse/events', [SseController::class, 'stream']);

public function stream(Request $request) {
    return response()->stream(function () use ($request) {
        $lastEventId = $request->header('Last-Event-ID', 0);
        while (!connection_aborted()) {
            $events = $this->reverb->getEventsSince($request->user()->id, $lastEventId);
            foreach ($events as $event) {
                echo "id: {$event->id}\n";
                echo "event: {$event->name}\n";
                echo "data: " . json_encode($event->payload) . "\n\n";
                $lastEventId = $event->id;
                ob_flush(); flush();
            }
            sleep(2);
        }
    }, 200, [
        'Content-Type' => 'text/event-stream',
        'Cache-Control' => 'no-cache',
        'X-Accel-Buffering' => 'no',
    ]);
}
```

Clients prefer WebSocket; on `WebSocketException`, automatically reconnect via SSE.

### 18.7 CRDT-based collaborative editing (V3)

For shared prescriptions (when a senior doctor and a junior are co-writing), V3 uses **Yjs** (CRDT library). The state is synced via Reverb (`collab.prescription.{id}`), with operational transformation handled by Yjs.

### 18.8 V2 channel catalog (additions to V1)

| Channel | Type | Purpose |
|---|---|---|
| `presence.clinic.{id}` | presence | Staff online status |
| `presence.doctor.{id}` | presence | Doctor online for chat |
| `presence.chat.{id}` | presence | Both chat participants |
| `presence.admin.{tenant}` | presence | Admin team |
| `chat.{chat_id}` | private | Chat messages, typing, read |
| `video.{room_id}` | private | Video room state (joined, left, ended) |
| `ai.encounter.{id}` | private | AI token stream for scribe |
| `lab.order.{id}` | private | Lab order status updates |
| `lab.partner.{id}` | private | Lab partner notifications |
| `insurance.claim.{id}` | private | Claim status updates |
| `subscription.{tenant}` | private | Tenant subscription events |
| `tenant.feature.{tenant}` | private | Feature flag events |
| `plugin.{tenant}` | private | Plugin install/enable/disable/upgrade/health events (V2 Plugin Core + V3 Marketplace, per 14-V2-Stubs §3.3a) |
| `hl7v2.message.received.{tenant}` | private | V3 HL7 v2 ADT/ORU processed inbound (per 19-Integrations §15.10) |
| `fhir.write.{tenant}.{resource_type}` | private | V3 FHIR R4 write succeeded (per 19-Integrations §15.10a) |
| `dicom.study.received.{tenant}` | private | V3 DICOM study received from Orthanc (per 19-Integrations §15.9) |
| `report.run.{started\|progress\|completed\|failed}.{report_id}` | private | V3 Custom Report Builder run lifecycle (per 13-Settings-Reports §13.2) |
| `mobile.build.{tenant}.{build_id}` | private | V3 White-Label Mobile EAS build lifecycle (per 19-Integrations §15.10b) |
| `bed.status.changed.{tenant}` | private | V3 Bed/Ward status change (per 04-Clinic-Structure §14.3) |
| `bed.assignment.{created\|discharged}.{tenant}` | private | V3 Bed assignment lifecycle |
| `ward.occupancy.changed.{tenant}` | private | V3 Ward occupancy counter (debounced 1/min) |
| `ot.slot.booked.{tenant}` | private | V3 Operation Theatre slot booked (per 04-Clinic-Structure §14.4) |
| `surgery.status.changed.{tenant}` | private | V3 Surgery status transition |
| `ot.theatre.status.changed.{tenant}` | private | V3 OT cleaning/maintenance/ready |
| `equipment.{status.changed\|booked\|booking.cancelled\|maintenance_due}.{tenant}` | private | V3 Equipment Booking lifecycle (per 04-Clinic-Structure §14.5) |
| `lis.analyzer.{connected\|disconnected}.{tenant}` | private | V4 LIS ASTM analyzer connection (per 19-Integrations §15.10c) |
| `lis.order.pushed.{tenant}` / `lis.result.received.{tenant}` | private | V4 LIS ASTM order/result flow |
| `lis.error.{tenant}` | private | V4 LIS ASTM analyzer error |
| `his.patient.merged.{tenant}` | private | V4 HIS Bridge PIX merge (per 19-Integrations §15.10d) |
| `his.document.{registered\|retrieved}.{tenant}` | private | V4 XDS.b C-CDA exchange |
| `his.connection.{connected\|disconnected\|error}.{vendor}.{tenant}` | private | V4 HIS Bridge vendor connection |
| `er.{arrival\|esi.changed\|bay.assigned\|pickup\|disposition}.{tenant}` | private | V4 ER Triage board events (per 07-Appointments §20.11) |
| `er.board.{tenant}` | private | V4 ER Triage board full snapshot (on demand) |
| `mar.due.{tenant}.{unit}` / `mar.overdue.{tenant}.{unit}` | private | V4 Nursing Station MAR queue (per 07-Appointments §20.13) |
| `vitals.critical.{tenant}.{patient_id}` | private | V4 Nursing Station MEWS critical vital |
| `handoff.assigned.{tenant}.{nurse_id}` | private | V4 Nursing Station incoming handoff |
| `pharmacy.stock.low.{tenant}.{drug_id}` | private | V4 Centralized Pharmacy reorder threshold (per 08-Prescriptions §18.1) |
| `pharmacy.dispense.requested.{tenant}.{pharmacy_id}` | private | V4 Centralized Pharmacy Rx waiting for dispense |
| `pharmacy.controlled.access.{tenant}.{pharmacy_id}` | private | V4 Centralized Pharmacy Schedule II–V vault event |
| `pharmacy.expiry.soon.{tenant}.{drug_id}` | private | V4 Centralized Pharmacy 30-day expiry warning |
| `equipment.wo.{opened\|completed}.{tenant}` | private | V4 Equipment Maintenance work order events (per 07-Appointments §20.15) |
| `equipment.recall.flagged.{tenant}.{udi}` | private | V4 Equipment Maintenance FDA recall match |
| `equipment.pm.due.{tenant}.{equipment_id}` | private | V4 Equipment Maintenance 7-day PM reminder |
| `cohort.run.completed.{tenant}.{cohort_id}` | private | V5 Population Health cohort run (per 06-Patients §17.7) |
| `measure.rate.computed.{tenant}.{measure_code}` | private | V5 Population Health HEDIS/MIPS measure computed |
| `cohort.outreach.dispatched.{tenant}.{cohort_run_id}` | private | V5 Population Health outreach campaign sent |
| `lab.order.{created\|status_changed\|completed}.{tenant}.{patient_id}` | private | V2 Pathology Lab order lifecycle (per 08-Prescriptions §16.3) |
| `lab.phlebotomist.{assigned\|en_route\|arrived\|collected}.{lab_order_id}` | private | V2 Pathology Lab home-collection tracking |
| `lab.result.critical.{tenant}.{doctor_id}` | private | V2 Pathology Lab critical/abnormal result |
| `lab.report.ready.{tenant}.{patient_id}` | private | V2 Pathology Lab e-Report PDF generated |
| `video.room.provisioned.{tenant}.{appointment_id}` | private | V2 Telemedicine LiveKit room ready (per 07-Appointments §19.3) |
| `video.participant.{joined\|left}.{room_id}` | private | V2 Telemedicine in-call presence |
| `video.recording.ready.{room_id}` | private | V2 Telemedicine recording uploaded |
| `video.quality.degraded.{room_id}` | private | V2 Telemedicine quality warning |
| `symptom.session.completed.{tenant}` | private | V2 AI Symptom Checker session finalized (per 18-User-Flows §19.7) |
| `scribe.transcript.token.{session_id}` | private | V2 AI Scribe STT token stream (per 08-Prescriptions §16.4) |
| `scribe.section.ready.{session_id}.{section}` | private | V2 AI Scribe SOAP section ready (subjective/objective/assessment/plan) |
| `scribe.signed.{encounter_id}` | private | V2 AI Scribe final sign |
| `scribe.failed.{session_id}` | private | V2 AI Scribe STT/LLM error |
| `chat.{chat_id}` | private | V2 In-App Chat thread channel (per 11-Notifications §20.4) — new message/edit/reaction/typing/read |
| `chat.unread.{tenant}.{user_id}` | private | V2 In-App Chat unread badge update |
| `domain.verification.{attempted\|succeeded\|failed}.{tenant_id}` | private | V2 Custom Domain DNS-01 verification (per 03-Tenancy §17.4) |
| `domain.ssl.active.{tenant_id}` | private | V2 Custom Domain ACM cert live |
| `domain.cert.expiring.{tenant_id}.{days}` | private | V2 Custom Domain cert expiry warning (30/14/7) |
| `prescription.{signed\|transmitted\|dispensed\|revoked}.{tenant}.{patient_id}` | private | V2 E-Prescription lifecycle (per 08-Prescriptions §16.2) |
| `insurance.eligibility.completed.{tenant}` | private | V2 Insurance Pre-Auth 270/271 returned (per 09-Billing §21.3) |
| `insurance.preauth.{submitted\|approved\|denied\|partial}.{tenant}.{preauth_id}` | private | V2 Insurance Pre-Auth status |
| `insurance.claim.{submitted\|paid\|denied}.{tenant}.{claim_id}` | private | V2 Insurance Pre-Auth claim lifecycle |
| `insurance.era.received.{tenant}.{payer_id}` | private | V2 Insurance Pre-Auth 835 ERA auto-posted |
| `voucher.redeemed.{tenant}.{voucher_id}` | private | V1.1 Marketing Vouchers redemption (per 09-Billing §11.4a) |
| `voucher.exhausted.{tenant}.{voucher_id}` | private | V1.1 Marketing Vouchers usage limit reached |
| `voucher.expires_soon.{tenant}.{voucher_id}` | private | V1.1 Marketing Vouchers expiry warning (T-7d) |
| `referral.credit.available.{patient_id}` | private | V1.1 Marketing Vouchers referral credit released (after refund window) |
| `partner.pool.exhausted.{partner_id}.{pool_id}` | private | V1.1 Marketing Vouchers partner pool depleted |
| `refund.computed.{tenant}.{appointment_id}` | private | V1.1 Dynamic Refund Window policy preview computed |
| `refund.resolution.chosen.{tenant}.{appointment_id}` | private | V1.1 Dynamic Refund Window patient chose resolution |
| `refund.policy.changed.{tenant}` | private | V1.1 Dynamic Refund Window policy updated |
| `refund.credit.expiring.{patient_id}.{credit_id}` | private | V1.1 Dynamic Refund Window credit expiry warning (T-7d) |
| `waitlist.offer.received.{patient_id}` | private | V1.1 Smart Waitlist offer received (per 07-Appointments §19.5) |
| `waitlist.offer.claimed.{tenant}.{slot_id}` | private | V1.1 Smart Waitlist slot claimed, cascade ends |
| `waitlist.offer.expired.{tenant}.{slot_id}` | private | V1.1 Smart Waitlist offer TTL expired, next batch |
| `waitlist.cascade.complete.{tenant}.{slot_id}` | private | V1.1 Smart Waitlist cascade ended |
| `waitlist.demand.spike.{tenant}.{doctor_id}` | private | V1.1 Smart Waitlist demand spike alert |
| `locale.translated.{tenant}.{locale}.{key_count}` | private | V1.1 i18n+RTL batch import / auto-translate completed (per 12-Engagement §6.7) |
| `locale.translation_job.completed.{tenant}.{job_id}` | private | V1.1 i18n+RTL auto-translation job done |
| `locale.user_switched.{tenant}.{user_id}` | private | V1.1 i18n+RTL user switched their locale |
| `locale.visual_regression.failed.{locale}` | private | V1.1 i18n+RTL visual regression CI failure |
| `rotation.activated.{tenant}.{group_id}.{date}` | private | V1.1 Doctor Rotating on-call shift started (per 05-Doctors §16.4) |
| `rotation.ended.{tenant}.{group_id}.{date}` | private | V1.1 Doctor Rotating on-call shift ended |
| `rotation.swap.requested.{tenant}.{target_doctor_id}` | private | V1.1 Doctor Rotating swap request created |
| `rotation.swap.accepted.{tenant}.{group_id}.{date}` | private | V1.1 Doctor Rotating swap accepted, plan updated |
| `rotation.escalation.triggered.{tenant}.{call_id}` | private | V1.1 Doctor Rotating after-hours escalation |
| `rotation.escalation.answered.{tenant}.{call_id}` | private | V1.1 Doctor Rotating escalated doctor answered |
| `rotation.coverage.gap.{tenant}.{group_id}.{date}` | private | V1.1 Doctor Rotating coverage gap detected |
| `rotation.oncall.changed.{tenant}.{specialty}` | private | V1.1 Doctor Rotating on-call doctor changed |
| `pwa.installed.{tenant}.{patient_id}` | private | V1.1 PWA Offline install completed (per 18-User-Flows §19.17) |
| `pwa.sync.completed.{patient_id}.{count}` | private | V1.1 PWA Offline background sync replayed N actions |
| `pwa.sync.conflict.{patient_id}.{action_id}` | private | V1.1 PWA Offline sync conflict surfaced to user |
| `pwa.push.delivered.{subscription_id}` | private | V1.1 PWA Offline Web Push delivered |
| `pwa.version.adopted.{version}` | private | V1.1 PWA Offline new version adopted by user |
| `pwa.storage.warning.{patient_id}` | private | V1.1 PWA Offline storage budget warning (>=90%) |
| `collab.prescription.{id}` | private | V3 collaborative Rx |
| `queue.clinic.{id}` | public | V1 queue display |
| `clinic.{id}.appointments` | private | V1 appointment updates |

### 18.9 V2 acceptance criteria

- [ ] Presence channels reflect join/leave within 1s.
- [ ] In-app chat message delivered in <500ms; typing indicator in <200ms.
- [ ] Video call connects 2 clients in <3s; Reverb signaling successful.
- [ ] AI scribe tokens stream in real time; UI shows incremental text.
- [ ] Reverb cluster handles 10k concurrent connections with p99 latency <100ms.
- [ ] SSE fallback delivers all events when WebSocket is blocked.
- [ ] All V2 events are auditable.
- [ ] No PHI on public channels.
