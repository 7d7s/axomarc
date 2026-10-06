# 07 — Appointments Module (Core, Calendar, Check-in, Queue, Calling, Cancellation)

> The central business object. Every other module either feeds into or reads from appointments. This is the most complex module in the system — a 7-state finite state machine with realtime, queue, payment, and clinical-data dependencies.

---

## 1. Scope

| In Scope | Out of Scope |
|---|---|
| 7-state appointment lifecycle (pending → completed) | Group appointments (V2) |
| Booking from patient app, web, or admin | Recurring/series bookings (V2) |
| Calendar views (day/week/month) for staff & doctor | Resource scheduling (rooms, equipment) — V2 |
| QR-code + manual check-in | Walk-in queue (no appointment) — V2 |
| Queue token auto-generation on check-in | Multi-doctor queue (V2) |
| Live patient calling by front-desk via Reverb | Voice announcement integration (V2) |
| Cancellation requests (user → admin approval) | Refund of partially-used services (V2) |
| Reschedule (with same payment tracking) | Multi-day appointments (V2) |
| Patient + family booking | |
| Notes from front-desk and from doctor (separate fields) | |

---

## 2. The 7-State Machine

This is the single most important FSM in the system. Every status change is **atomic**, **audited**, and **broadcast**.

```
                      ┌──────────────┐
                      │   pending    │  ← created, awaiting payment OR free service
                      └──────┬───────┘
                             │
                ┌────────────┼────────────┐
                │            │            │
                ▼            ▼            ▼
        ┌──────────┐  ┌──────────┐  ┌──────────┐
        │ confirmed│  │ cancelled│  │  failed  │   (payment failed / declined)
        └────┬─────┘  └──────────┘  └──────────┘
             │
   ┌─────────┼─────────┐
   │         │         │
   ▼         ▼         ▼
┌──────┐ ┌────────┐ ┌────────┐
│checked│ │cancel │ │no_show │  (no cancellation — just past start with no check-in)
│  -in  │ │requested│└────────┘
└───┬──┘ └────┬───┘
    │        │  admin approves → cancelled
    │        │  admin rejects  → confirmed (or pending if not paid)
    ▼        ▼
┌────────┐ ┌──────────┐
│in_cons │ │cancelled │
│ultation│ └──────────┘
└───┬────┘
    │
    ▼
┌──────────┐
│ completed│
└──────────┘
```

### 2.1 The 7 States

| # | State | Description | Entered From | Exits To |
|---|---|---|---|---|
| 1 | `pending` | Created, awaiting payment (or free service) | — | `confirmed` (paid), `cancelled` (timeout 15 min), `failed` (payment error) |
| 2 | `confirmed` | Paid, scheduled | `pending` (on payment success) | `checked_in`, `cancellation_requested`, `no_show`, `cancelled` (by admin) |
| 3 | `checked_in` | Patient arrived, queue token issued | `confirmed` (manual or QR) | `in_consultation` |
| 4 | `in_consultation` | Doctor started the visit | `checked_in` | `completed` |
| 5 | `completed` | Visit finished, prescription written | `in_consultation` | — (terminal) |
| 6 | `cancelled` | Cancelled (by user, by admin, or system timeout) | `pending`, `confirmed`, `cancellation_requested` (approved) | — (terminal; refund if paid) |
| 7 | `no_show` | Past start time, never checked in | `confirmed` (cron at start+30min) | — (terminal; no refund unless policy) |
| — | `cancellation_requested` | User asked to cancel, awaiting admin | `confirmed` | `cancelled` (approved), `confirmed` (rejected) |
| — | `failed` | Payment failed or declined | `pending` | — (terminal) |

> **Note on the count:** "7 states" per the index refers to the **happy-path** states (pending, confirmed, checked_in, in_consultation, completed, cancelled, no_show). `cancellation_requested` and `failed` are auxiliary states in the FSM for finer reporting.

### 2.2 Transition Rules

```php
// app/Modules/Appointments/Services/AppointmentStateMachine.php
class AppointmentStateMachine {
    private const TRANSITIONS = [
        'pending'                 => ['confirmed', 'cancelled', 'failed'],
        'confirmed'               => ['checked_in', 'cancellation_requested', 'no_show', 'cancelled'],
        'checked_in'              => ['in_consultation', 'cancelled'],
        'in_consultation'         => ['completed'],
        'cancellation_requested'  => ['confirmed', 'cancelled'],
        'completed'               => [],
        'cancelled'               => [],
        'no_show'                 => [],
        'failed'                  => [],
    ];

    public function canTransition(string $from, string $to): bool {
        return in_array($to, self::TRANSITIONS[$from] ?? [], true);
    }

    public function transition(Appointment $a, string $to, User $actor, array $meta = []): Appointment {
        DB::transaction(function () use ($a, $to, $actor, $meta) {
            $from = $a->status;
            if (!$this->canTransition($from, $to)) {
                throw new InvalidTransitionException("$from → $to not allowed");
            }
            $a->status = $to;
            $a->save();

            AuditLog::record('appointment.transition', $a, [
                'from' => $from, 'to' => $to, 'actor_id' => $actor->id, 'meta' => $meta,
            ]);

            event(new AppointmentStatusChanged($a, $from, $to, $actor));
        });
        return $a->fresh();
    }
}
```

### 2.3 Who Can Trigger What

| From → To | Patient | Doctor | Front-Desk | Admin | System (cron/job) |
|---|---|---|---|---|---|
| `pending → confirmed` | — (webhook does it) | — | — | — | ✅ (Razorpay/Stripe webhook) |
| `pending → cancelled` | ✅ (cancel pending booking) | — | — | ✅ | ✅ (15-min payment timeout) |
| `pending → failed` | — | — | — | — | ✅ (webhook with `payment.failed`) |
| `confirmed → checked_in` | ✅ (QR scan) | — | ✅ | ✅ | — |
| `confirmed → cancellation_requested` | ✅ | — | — | ✅ | — |
| `confirmed → no_show` | — | — | — | — | ✅ (`MarkNoShowJob` 30 min after start) |
| `confirmed → cancelled` | — | — | — | ✅ | ✅ (doctor emergency, with refund) |
| `cancellation_requested → cancelled` | — | — | — | ✅ | — |
| `cancellation_requested → confirmed` | — | — | — | ✅ (rejection) | — |
| `checked_in → in_consultation` | — | ✅ | — | — | — |
| `checked_in → cancelled` | — | — | — | ✅ | — |
| `in_consultation → completed` | — | ✅ | — | — | — |

---

## 3. Data Model

### 3.1 `appointments`

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `uuid` | char(36) unique | Public ID |
| `appointment_number` | varchar(40) unique per tenant | Human-readable, e.g., `APT-2026-000001` |
| `patient_id` | bigint FK | Clinical subject |
| `booked_by_user_id` | bigint FK | Who clicked Book (may be different from patient — family) |
| `doctor_id` | bigint FK | |
| `clinic_id` | bigint FK | |
| `service_id` | bigint FK nullable | If booked as a service |
| `slot_id` | bigint FK nullable | The doctor_time_slot row |
| `start_at` | timestamp | UTC, converted to clinic TZ for display |
| `end_at` | timestamp | |
| `duration_minutes` | smallint | |
| `consultation_type` | enum('in_person','online') | |
| `status` | enum(...) | The 9 states above |
| `previous_status` | enum nullable | For `cancellation_requested` rollback |
| `queue_token_id` | bigint FK nullable | Set on check-in |
| `queue_position` | int nullable | Snapshot at check-in time |
| `chief_complaint` | text nullable | Patient's reason for visit |
| `patient_notes` | text nullable | Patient-visible, set at booking |
| `internal_notes` | text nullable | Staff-only, hidden from patient |
| `doctor_notes` | text nullable | Doctor's clinical notes, set during/after visit |
| `cancellation_reason` | text nullable | |
| `cancelled_by_user_id` | bigint FK nullable | |
| `cancelled_at` | timestamp nullable | |
| `no_show_marked_at` | timestamp nullable | |
| `checked_in_at` | timestamp nullable | |
| `started_at` | timestamp nullable | When doctor started |
| `completed_at` | timestamp nullable | |
| `video_call_id` | bigint FK nullable | If `consultation_type=online`, link to Zoom meeting |
| `is_follow_up` | boolean default false | |
| `parent_appointment_id` | bigint FK nullable | If this is a follow-up of an earlier visit |
| `metadata` | json | Extensible |
| `created_at`, `updated_at` | timestamps | |
| `deleted_at` | timestamp nullable | Soft delete (rare; only for admin GDPR erasure) |

Indexes:
- `(tenant_id, status, start_at)` — calendar queries
- `(doctor_id, start_at)` — doctor's day view
- `(patient_id, start_at desc)` — patient history
- `(clinic_id, start_at, status)` — front-desk day view
- `(queue_token_id)` — lookup
- `(slot_id)` unique — one appointment per slot

### 3.2 `appointment_status_logs`

Append-only log of every transition.

| Column | Type |
|---|---|
| `id` | bigint PK |
| `appointment_id` | bigint FK |
| `from_status` | enum |
| `to_status` | enum |
| `actor_id` | bigint FK nullable (NULL for system) |
| `actor_type` | enum('user','doctor','staff','admin','system') |
| `reason` | text nullable |
| `meta` | json |
| `created_at` | timestamp |

This is the source of truth for "what happened to this appointment". Used for timeline display in admin and for any dispute.

### 3.3 `queue_tokens`

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `clinic_id` | bigint FK |
| `doctor_id` | bigint FK |
| `appointment_id` | bigint FK unique |
| `token_number` | int | Per (clinic, doctor, day) — resets at midnight clinic-local |
| `issued_at` | timestamp |
| `called_at` | timestamp nullable | When front-desk called the patient |
| `served_at` | timestamp nullable | When `in_consultation` started |
| `cancelled_at` | timestamp nullable |
| `status` | enum('waiting','called','served','skipped','cancelled') default 'waiting' |

Indexes: `(clinic_id, doctor_id, date(issued_at), status)`, `(token_number, doctor_id, date(issued_at))`.

### 3.4 `appointment_cancellations`

| Column | Type |
|---|---|
| `id` | bigint PK |
| `appointment_id` | bigint FK |
| `requested_by_user_id` | bigint FK |
| `reason` | text |
| `requested_at` | timestamp |
| `reviewed_by_user_id` | bigint FK nullable |
| `reviewed_at` | timestamp nullable |
| `decision` | enum('approved','rejected') nullable |
| `decision_notes` | text nullable |
| `refund_id` | bigint FK nullable |

### 3.5 `appointment_reschedules`

Track history of reschedules (V1 keeps it simple — each reschedule creates a new appointment + links via `parent_appointment_id`).

---

## 4. Booking Flow (Happy Path)

### 4.1 Patient-Initiated Booking

```
1. Patient app: pick doctor → pick date → see available slots
   GET /api/v1/public/doctors/{id}/slots?date=2026-06-15

2. Patient picks slot → app calls hold endpoint (optional but recommended)
   POST /api/v1/public/doctors/{id}/slots/{slotId}/hold
   → slot.status = 'held', held_by = user.id, held_until = now()+10min
   → 409 if slot is no longer available

3. Patient fills in: who is the appointment for (self/family), chief_complaint, patient_notes
   → app calls create endpoint
   POST /api/v1/patient/appointments
   {
     slot_id, service_id?, patient_id, consultation_type,
     chief_complaint?, patient_notes?,
     coupon_code?
   }

4. API:
   a. Validate (FormRequest)
   b. Resolve amount: service.price or doctor.consultation_fee
   c. Apply coupon if any
   d. Begin transaction
   e. Lock slot row (SELECT FOR UPDATE)
   f. Verify slot is still available or held by this user
   g. Create appointment (status=pending)
   h. Create invoice (status=unpaid) — see 09-BILLING
   i. Call PaymentGateway->createOrder
   j. Commit
   k. Dispatch PushNotificationJob ("appointment.pending")
   l. Return { appointment, payment: { gateway, order_id, key, amount, currency } }

5. App opens gateway checkout (Razorpay SDK / Stripe SDK / Paystack SDK)
   User pays

6. Gateway → webhook to API
   POST /api/v1/webhooks/razorpay  (or stripe/paystack)

7. Webhook handler:
   a. Verify HMAC signature
   b. Find appointment by order_id
   c. In transaction:
      - Insert Transaction row
      - Insert Payment row (linked to Transaction + Invoice)
      - Mark invoice paid
      - Mark slot booked
      - StateMachine->transition(appointment, 'confirmed')
      - Insert appointment_status_log
   d. Dispatch PushNotificationJob to patient + doctor
   e. Broadcast AppointmentBooked on patient.{id}, doctor.{id}, clinic.{id}
   f. Return 200 to gateway

8. App receives broadcast (or polls appointment)
   → navigates to confirmation screen
```

### 4.2 Free / Walk-in Booking (No Payment)

For services with `price=0` or admin override, skip steps 5–7. After step 4 the API directly calls `transition(appointment, 'confirmed')`. The state machine supports this; the Invoice is still created for record-keeping with `amount=0`.

### 4.3 Admin-Initiated Booking

Same as patient, but with `booked_by_user_id` = admin. Patient may not have a User account yet (walk-in). Admin creates the Patient row inline.

### 4.4 Doctor-to-Patient (In-Clinic)

Doctor or front-desk creates an appointment on the spot. Status starts at `confirmed` (no payment). `consultation_type=in_person`. Used for paper-based clinics going digital.

### 4.5 Idempotency

All booking endpoints accept an `Idempotency-Key` header. Same key + same payload within 24h returns the original response. This prevents double-booking if the app retries on timeout.

---

## 5. Calendar Views

### 5.1 Admin / Front-Desk Calendar

```
GET /api/v1/admin/appointments/calendar
  ?clinic_id=4
  &doctor_id=42           (optional; omit for clinic-wide)
  &from=2026-06-10
  &to=2026-06-16
  &status=confirmed,checked_in
  &consultation_type=in_person
  &view=day|week|month
```

Returns appointments grouped by date and resource (doctor). Optimized for fast initial render.

### 5.2 Doctor's Day View

```
GET /api/v1/doctor/appointments?date=today
```

Returns today's appointments with patient header (name, MRN, age, allergies, chief complaint), status badge, queue position, "Start" button.

### 5.3 Patient's Upcoming

```
GET /api/v1/patient/appointments?filter=upcoming&page=1
```

---

## 6. Check-in

### 6.1 Manual Check-in (Front-Desk)

```
POST /api/v1/admin/appointments/{id}/check-in
{
  method: "manual",
  notes?: "..."
}
```

- `StateMachine->transition('checked_in')`
- Generate queue token (atomic per-doctor counter)
- Broadcast `AppointmentCheckedIn` on clinic channel
- Push notification to doctor

### 6.2 QR Code Check-in (Patient Self)

Each appointment has a QR code with payload `{ appointment_id, uuid, signature_hmac }`. The signature uses `HMAC-SHA256(appointment.id + appointment.uuid, APP_KEY)` to prevent forgery.

```
POST /api/v1/patient/appointments/{id}/check-in
{
  method: "qr",
  qr_payload: "...",
  qr_signature: "..."
}
```

- App scans QR (e.g., at clinic entrance kiosk or printed on bill)
- Verifies signature
- Same flow as manual

### 6.3 Self Check-in Window

A patient can self check-in **between 30 min before and 30 min after** the appointment start. Outside this window, the front-desk must do it.

### 6.4 Check-in at Wrong Clinic

If a patient scans the QR at a different clinic (multi-clinic tenant), the API returns `409 wrong_clinic` with directions to the correct one.

---

## 7. Queue Tokens

### 7.1 Generation

When `checked_in` is entered, `QueueTokenService::issue(appointment)` runs:

```php
public function issue(Appointment $a): QueueToken {
    return DB::transaction(function () use ($a) {
        $today = $a->start_at->toDateString();
        $count = QueueToken::where('doctor_id', $a->doctor_id)
            ->whereDate('issued_at', $today)
            ->lockForUpdate()
            ->count();
        return QueueToken::create([
            'clinic_id' => $a->clinic_id,
            'doctor_id' => $a->doctor_id,
            'appointment_id' => $a->id,
            'token_number' => $count + 1,
            'issued_at' => now(),
            'status' => 'waiting',
        ]);
    });
}
```

### 7.2 Live Queue Display (TV / Screen at the Clinic)

```
GET /api/v1/public/clinics/{id}/queue?doctor_id=42
```

Returns the current "now serving" and next 3 "waiting" tokens. Cached 5s; also broadcast via Reverb on `clinic.{id}.queue` for instant updates.

A separate **public display page** at `/display/{clinic-slug}` (no auth, read-only) renders this. The page auto-reconnects to Reverb and shows token number, patient name (or initials if privacy mode), and "Please proceed to Room X".

### 7.3 Live Patient Calling (Front-Desk Action)

```
POST /api/v1/admin/appointments/{id}/call
{
  consultation_room: "Room 204"
}
```

- Sets `queue_token.called_at = now()`, `status = 'called'`
- Broadcasts `PatientCalled` on `clinic.{id}.queue` and `patient.{id}`
- Display screen updates instantly
- Patient phone buzzes with FCM push
- Optional: TTS announcement (V2)

### 7.4 Skip

Front-desk can `POST /admin/appointments/{id}/skip` if patient isn't there. Token status = `skipped`, patient reverts to end of queue (V2 reissue; V1 just mark as skipped, admin manually re-calls).

### 7.5 Doctor "Start Visit" → Serves Token

When doctor taps "Start" → `StateMachine->transition('in_consultation')` → `queue_token.status='served'`, `served_at=now()`. The next patient in queue is auto-promoted to "next" (broadcast event for the display).

---

## 8. Cancellation

### 8.1 Patient-Initiated

```
POST /api/v1/patient/appointments/{id}/cancel-request
{ reason: "..." }
```

- If appointment is `pending`: directly call `transition('cancelled')`. Refund if any payment.
- If `confirmed`: create `AppointmentCancellation` row, transition to `cancellation_requested`.
- Notification to admin: new cancellation request.
- Push to patient: "Your cancellation request was submitted".

### 8.2 Admin Decision

```
POST /api/v1/admin/cancellations/{id}/approve
{ refund: true, notes?: "..." }
```

or

```
POST /api/v1/admin/cancellations/{id}/reject
{ notes: "..." }
```

- On approve: `transition('cancelled')` → if paid, dispatch `ProcessRefundJob`.
- On reject: `transition('confirmed')`, push to patient with reason.
- Audit-logged.

### 8.3 Refund Rules

Configured per tenant in `settings.cancellation_policy` JSON:

```json
{
  "free_cancellation_hours": 24,
  "partial_refund_hours": 4,
  "partial_refund_percent": 50,
  "no_refund_after_minutes": 0
}
```

- >24h before start: full refund
- 4–24h: 50% refund
- <4h: no refund (admin can override)

The `CancellationPolicyService::computeRefund($appointment)` returns the amount to refund. Refund is a separate `Refund` row linked to the original `Transaction`.

### 8.4 Admin-Direct Cancellation

Admin can cancel any non-terminal appointment immediately. Always requires a reason. Refund per policy.

### 8.5 System Cancellations

- `pending` + 15 min with no payment → auto-cancel, slot released
- `confirmed` + start_time passed + 30 min + not checked_in → mark `no_show`
- Doctor emergency → admin cancels all of doctor's appointments for the day, bulk refund

---

## 9. Reschedule

```
POST /api/v1/patient/appointments/{id}/reschedule
{ new_slot_id, reason? }
```

- Original appointment: marked `cancelled` (reason=`rescheduled`), refund per policy
- New appointment: created in `pending` (if paid) or `confirmed` (if free), with `parent_appointment_id` pointing to old
- Slot is the new slot; old slot released
- If patient paid full price and the new slot costs less, partial refund; if more, additional charge via same gateway (V2)

---

## 10. Doctor App Workflow

```
[Today tab]
  → list of today's appointments for this doctor
  → filterable by status
  → sorted by start_at

[Tap appointment]
  → Patient Header (name, age, gender, MRN, allergies, photo)
  → Chief complaint, patient notes
  → Vitals summary (latest weight, BP)
  → Files list (preview inline)
  → Previous visits (with this doctor + others in tenant)
  → "Start Visit" button → in_consultation
  → "Write Prescription" → opens Rx form
  → "Add Vitals" → form
  → "Upload Photo/Document" → file upload
  → "Start Video Call" (if online) → opens Zoom
  → "Complete" → status=completed, optional notes
  → "Mark No-Show" (admin only) → status=no_show
```

---

## 11. Service Methods (`AppointmentService`)

```php
namespace App\Modules\Appointments\Services;

class AppointmentService {
    public const VERSION = '1.0.0';

    public function book(array $data, User $booker): array;        // returns {appointment, payment}
    public function confirm(Appointment $a, Transaction $tx): void;
    public function checkIn(Appointment $a, User $actor, array $meta): QueueToken;
    public function call(Appointment $a, User $actor, ?string $room): void;
    public function skip(QueueToken $t, User $actor): void;
    public function start(Appointment $a, User $doctor): void;
    public function complete(Appointment $a, User $doctor, array $data): void;
    public function requestCancellation(Appointment $a, User $by, string $reason): AppointmentCancellation;
    public function approveCancellation(AppointmentCancellation $c, User $admin, bool $refund): void;
    public function rejectCancellation(AppointmentCancellation $c, User $admin, string $notes): void;
    public function cancel(Appointment $a, User $by, string $reason): void;  // direct cancel
    public function reschedule(Appointment $a, int $newSlotId, User $by): Appointment;
    public function markNoShow(Appointment $a): void;                         // cron
    public function expirePending(): int;                                      // cron, returns count
    public function getCalendar(Carbon $from, Carbon $to, array $filters): Collection;
    public function getQueueDisplay(Clinic $c, ?Doctor $d): array;
    public function getUpcoming(User $user, int $limit = 10): Collection;
    public function getHistory(Patient $p, int $page): LengthAwarePaginator;
}
```

---

## 12. API Endpoints

### 12.1 Public (Read-only)

```
GET  /api/v1/public/appointments/lookup/{uuid}      (for QR code; returns minimal info)
GET  /api/v1/public/clinics/{id}/queue
```

### 12.2 Patient (Mobile + Web)

```
GET    /api/v1/patient/appointments?filter=upcoming|past|cancelled&page=
GET    /api/v1/patient/appointments/{id}
POST   /api/v1/patient/appointments                     (book — see 4.1)
POST   /api/v1/patient/appointments/{id}/cancel-request
POST   /api/v1/patient/appointments/{id}/check-in        (self QR)
POST   /api/v1/patient/appointments/{id}/reschedule
GET    /api/v1/patient/appointments/{id}/payment-status   (poll)
GET    /api/v1/patient/appointments/{id}/receipt          (PDF)
POST   /api/v1/patient/appointments/{id}/review           (after completed)
```

### 12.3 Doctor App

```
GET    /api/v1/doctor/dashboard?date=today
GET    /api/v1/doctor/appointments?date=&status=
GET    /api/v1/doctor/appointments/{id}
POST   /api/v1/doctor/appointments/{id}/start
POST   /api/v1/doctor/appointments/{id}/complete
POST   /api/v1/doctor/appointments/{id}/notes             (save doctor_notes)
GET    /api/v1/doctor/appointments/{id}/patient
GET    /api/v1/doctor/schedule?from=&to=
```

### 12.4 Admin / Front-Desk

```
GET    /api/v1/admin/appointments                         (full filter)
GET    /api/v1/admin/appointments/calendar?from=&to=
GET    /api/v1/admin/appointments/{id}
POST   /api/v1/admin/appointments                         (book on behalf)
POST   /api/v1/admin/appointments/{id}/check-in           (manual)
POST   /api/v1/admin/appointments/{id}/call               (live call)
POST   /api/v1/admin/appointments/{id}/skip
POST   /api/v1/admin/appointments/{id}/cancel             (direct)
POST   /api/v1/admin/appointments/{id}/mark-no-show
POST   /api/v1/admin/appointments/{id}/mark-arrived       (alias for check-in)
POST   /api/v1/admin/appointments/{id}/reschedule
POST   /api/v1/admin/appointments/{id}/refund             (admin override)
GET    /api/v1/admin/appointments/{id}/status-log
GET    /api/v1/admin/appointments/{id}/audit-log
POST   /api/v1/admin/appointments/bulk-cancel             (doctor emergency)
```

### 12.5 Cancellation Requests

```
GET    /api/v1/admin/cancellations?status=pending
GET    /api/v1/admin/cancellations/{id}
POST   /api/v1/admin/cancellations/{id}/approve
POST   /api/v1/admin/cancellations/{id}/reject
```

---

## 13. Validation Examples

```php
// BookAppointmentRequest
public function rules(): array {
    return [
        'slot_id'           => ['required', 'integer', 'exists:doctor_time_slots,id'],
        'patient_id'        => ['required', 'integer', 'exists:patients,id'],
        'service_id'        => ['nullable', 'integer', 'exists:services,id'],
        'consultation_type' => ['required', Rule::in(['in_person', 'online'])],
        'chief_complaint'   => ['nullable', 'string', 'max:1000'],
        'patient_notes'     => ['nullable', 'string', 'max:2000'],
        'coupon_code'       => ['nullable', 'string', 'max:40'],
        'idempotency_key'   => ['nullable', 'string', 'max:80'],
    ];
}
```

```php
// CheckInRequest
public function rules(): array {
    return [
        'method'        => ['required', Rule::in(['manual', 'qr'])],
        'notes'         => ['nullable', 'string', 'max:500'],
        'qr_payload'    => ['required_if:method,qr', 'string'],
        'qr_signature'  => ['required_if:method,qr', 'string'],
    ];
}
```

```php
// CancelRequestRequest
public function rules(): array {
    return [
        'reason' => ['required', 'string', 'min:10', 'max:500'],
    ];
}
```

---

## 14. Realtime Events

| Event | Channel | Payload | Listeners |
|---|---|---|---|
| `AppointmentBooked` | `patient.{id}`, `doctor.{id}`, `clinic.{tenant}.staff` | full appointment | App navigates to confirmation; staff dashboard updates; doctor gets FCM |
| `AppointmentConfirmed` | `patient.{id}` | appointment | App updates status pill |
| `AppointmentCheckedIn` | `clinic.{id}.queue`, `doctor.{id}` | {appointment, queue_token} | Display screen shows in waiting list; doctor sees "patient arrived" |
| `AppointmentCalled` | `clinic.{id}.queue`, `patient.{id}` | {appointment, queue_token, room} | Display flashes; patient phone buzzes |
| `AppointmentStarted` | `patient.{id}`, `clinic.{id}.queue` | appointment | Display marks "served"; next auto-promoted |
| `AppointmentCompleted` | `patient.{id}`, `doctor.{id}` | appointment | App shows "completed, please review" |
| `AppointmentCancelled` | `patient.{id}`, `doctor.{id}`, `clinic.{tenant}.staff` | appointment | All UIs update; refund queued |
| `CancellationRequested` | `clinic.{tenant}.staff` | cancellation | Admin bell icon increments |
| `NoShowMarked` | `patient.{id}`, `doctor.{id}` | appointment | FCM |
| `AppointmentRescheduled` | `patient.{id}`, `doctor.{id}` | {old, new} | Both UIs update |

See `17-REALTIME.md` for full channel/payload specs.

---

## 15. Cron Jobs

| Job | Schedule | Purpose |
|---|---|---|
| `ExpirePendingAppointmentsJob` | every minute | `pending` + 15 min old → `cancelled`, release slot |
| `MarkNoShowJob` | every 5 min | `confirmed` + start+30 min + no check-in → `no_show` |
| `AppointmentReminderJob` | every 15 min | Find appointments in next 24h/2h with no reminder sent → push + email |
| `ReleaseExpiredHoldsJob` | every minute | Release slot holds where `held_until < now()` |
| `QueueTokenResetJob` | daily at 00:00 clinic-TZ | No-op (token counter is per-day count) |
| `AppointmentSettlementJob` | daily 02:00 | Mark `completed` appointments >7 days old as settled for accounting |

---

## 16. Audit

`Appointment` is in the global audit list. **Every** state transition writes to both `audit_logs` (PHI tracking) and `appointment_status_logs` (timeline). The two differ in audience:
- `audit_logs` → compliance officer, never deleted
- `appointment_status_logs` → patient/staff timeline, queryable via API

---

## 17. Testing Strategy

| Test | Assertion |
|---|---|
| `StateMachineTest` | All 9 states × all valid transitions succeed; all invalid throw |
| `BookingFlowTest` | Pending → Confirmed on webhook, slot marked booked, broadcast fired |
| `ConcurrentBookingTest` | Two patients hit the same slot — one wins, one gets 409 |
| `IdempotencyTest` | Same Idempotency-Key returns same response, no double booking |
| `CancellationRefundTest` | Full / partial / no refund based on time-to-start |
| `CheckInWindowTest` | 29 min before start → 400; 31 min before → 400; within window → 200 |
| `QRSignatureTest` | Tampered QR → 401 |
| `QueueTokenTest` | 3 patients check in → tokens 1,2,3 in order |
| `LiveCallBroadcastTest` | Reverb receives `PatientCalled` with right payload |
| `NoShowCronTest` | Appointment past start+30 → marked no_show |
| `RescheduleTest` | Old apt cancelled, new apt created, refund calculated correctly |
| `BulkCancelTest` | All of doctor's day cancelled with one click, refunds queued |
| `DoctorDayViewTest` | Only this doctor's appointments returned, sorted by start_at |
| `CrossTenantTest` | Patient in tenant A cannot book in tenant B |

---

## 18. Acceptance Criteria (V1)

- [ ] Patient can book an appointment end-to-end with payment.
- [ ] Slot is held for 10 min during checkout, auto-released on timeout.
- [ ] Two patients racing for same slot — exactly one succeeds.
- [ ] Cancellation before 24h issues full refund; between 4–24h issues 50%; <4h issues none (configurable).
- [ ] QR check-in works; tampered QR rejected.
- [ ] Queue tokens auto-increment per doctor per day; display screen shows live list.
- [ ] Front-desk can "Call" a patient; display screen flashes within 1 second; patient phone buzzes.
- [ ] Doctor taps "Start" → appointment moves to `in_consultation`; token marked served.
- [ ] Doctor taps "Complete" → appointment `completed`; patient prompted to review.
- [ ] No-show cron marks past appointments correctly.
- [ ] All transitions appear in patient and staff timelines.
- [ ] All transitions audited.
- [ ] Reschedule preserves payment, refunds/charges difference.
- [ ] Bulk cancel works for a doctor's emergency day off.
- [ ] Idempotency-Key prevents double-booking on retry.
- [ ] State machine rejects every invalid transition.

---

## 19. V2 Roadmap — Appointments

> V1 ships a 9-state appointment FSM. V2 layers on recurring, group, resource, telemedicine, and waitlist — and turns the appointment into the **anchor** of a portable patient encounter.

### 19.1 Recurring appointments (`recurring-appointments`, V1.1)

The most-requested feature post-V1. Patient says "every Tuesday 6 PM for 8 weeks" — we expand that into 8 individual appointment rows at booking time (or lazily on read).

```php
Schema::table('appointments', function (Blueprint $t) {
    $t->ulid('parent_appointment_id')->nullable()->index();
    $t->string('rrule', 512)->nullable();                   // RFC 5545: 'FREQ=WEEKLY;COUNT=8;BYDAY=TU'
    $t->boolean('is_recurring_anchor')->default(false);
});

Schema::create('appointment_series', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('doctor_id')->index();
    $t->string('rrule', 512);
    $t->date('series_start_date');
    $t->date('series_end_date')->nullable();
    $t->integer('max_occurrences')->nullable();
    $t->integer('generated_count')->default(0);
    $t->timestamp('last_generated_until')->nullable();
    $t->string('status', 16)->default('active');            // active | paused | cancelled
    $t->timestamps();
});
```

`RecurringAppointmentService::expand($series, Carbon $from, Carbon $to)` uses `rrule` library to materialize individual `appointments` rows in the DB. The expansion is **idempotent** (UNIQUE on `parent_appointment_id, occurrence_date`).

**Edge cases:**
- Patient cancels one occurrence → only that row cancels; series stays active.
- Patient cancels the series → all future occurrences cancel; past stay.
- Doctor's schedule changes → bulk-reschedule the future occurrences (V2.1).
- Holiday (tenant calendar) skips a date automatically.

### 19.2 Group appointments — `group-appointments` (V1.1, MML 4) — deep-dive

**1. Overview.** A **group appointment** is a single time slot shared by **N patients** (typically 2-20) with the same doctor. Common use cases:
- **Group therapy** (CBT group, addiction counseling, support group) — 6-12 patients, weekly, 90 minutes.
- **Prenatal class** — 8-15 expecting mothers, monthly, 2 hours.
- **Vaccination camp** — 20-50 patients, 1-shot events, 4 hours.
- **Diabetes education workshop** — 10-20 patients, monthly, 2 hours.
- **Yoga / physiotherapy batch** — 5-10 patients, weekly, 60 minutes.
- **Webinar-style doctor talk** — 50-200 patients, 1-time, 60 minutes.

V1.1 ships the first-class group appointment model: a single time slot with a capacity, a per-person price, a list of participants, and a shared resource (room). The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.1 lane module. MML 4 GA covers: group creation, capacity enforcement, participant join / leave, payment per participant, group video (per `07-Appts §19.3` Telemedicine, for online groups), per-participant outcomes (each patient can have their own notes, prescription, and follow-up), and group cancellation. MML 5 (V2) adds group series (a recurring group appointment), group chat (per `11-Notif §20.4`), and group analytics.

**2. The data model (extended).** (Builds on the existing §19.2 schema.) Adds:
```php
Schema::create('group_appointment_participants', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('group_appointment_id')->index();
    $t->ulid('appointment_id')->unique()->index();                // the participant's individual appt
    $t->ulid('patient_id')->index();
    $t->ulid('invoice_id')->nullable()->index();
    $t->string('status', 16)->default('reserved');                  // 'reserved' | 'confirmed' | 'attended' | 'no_show' | 'cancelled'
    $t->timestamp('joined_at')->nullable();
    $t->timestamp('cancelled_at')->nullable();
    $t->text('cancellation_reason')->nullable();
    $t->timestamps();
    $t->unique(['group_appointment_id', 'patient_id']);             // one row per patient per group
});

Schema::create('group_appointment_resources', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('group_appointment_id')->index();
    $t->ulid('resource_id')->index();                              // the room (V2+)
    $t->string('purpose', 32);                                     // 'primary' | 'overflow'
    $t->timestamps();
});
```

**3. The 6 group statuses.**
- `draft` — the doctor is composing the group; not yet published.
- `open` — published; patients can join (if `booked_count < capacity`).
- `full` — capacity reached; the join button is disabled (but waitlist may be enabled).
- `started` — the doctor has marked the group as started (`started_at` is set); no new participants can join.
- `completed` — the group has finished; the doctor has marked outcomes for each participant.
- `cancelled` — the group is cancelled; all participants are refunded; all individual appointments are cancelled.

**4. The 4 join / leave flows.**
- **Join (open group):** the patient taps "Join" on a group listing; the API runs `SELECT ... FOR UPDATE` on the `group_appointments` row; checks `booked_count < capacity`; if yes, increments `booked_count`, creates a `group_appointment_participants` row, creates an `appointments` row (`is_group=true`, `group_appointment_id=...`), creates an `invoices` row, sends to the patient; if no, returns 409.
- **Join (with payment):** the patient taps "Join & Pay"; the API reserves the seat, creates the invoice, and sends the patient to the gateway (Razorpay / Stripe / Paystack); the seat is held for 10 minutes; on `payment.captured` webhook, the reservation is confirmed; on timeout, the seat is released.
- **Leave (before group):** the patient taps "Leave"; the API checks the cancellation window (per `09-Billing §21.1.5`); if eligible, the participant is removed, the seat is released (`booked_count -= 1`), the invoice is refunded (per the dynamic refund policy, see Turn 55); the group status is recomputed (full → open if a seat is released).
- **Leave (after group started):** not allowed; the patient's status is `'attended'` or `'no_show'` (set by the doctor at the end).

**5. Capacity enforcement.** The `booked_count` is a denormalized counter maintained in a transaction with `SELECT ... FOR UPDATE` on the `group_appointments` row. The pattern:
```php
DB::transaction(function () use ($group) {
    $row = DB::table('group_appointments')->where('id', $group->id)->lockForUpdate()->first();
    if ($row->booked_count >= $row->capacity) {
        throw new GroupFullException();
    }
    DB::table('group_appointments')->where('id', $group->id)->update(['booked_count' => $row->booked_count + 1]);
    // ... create participant, appointment, invoice ...
});
```
A partial unique index as a belt-and-braces:
```sql
CREATE UNIQUE INDEX group_appointments_capacity_check
ON group_appointments (id, (CASE WHEN booked_count < capacity THEN 1 ELSE NULL END));
```

**6. Per-participant outcomes.** When the group is `completed`, the doctor records per-participant outcomes:
- **Attendance:** `attended` or `no_show`.
- **Clinical note:** a free-text note (per the doctor; not shared with other participants).
- **Prescription:** if the group is a "vaccination camp", the doctor records the vaccine + lot + expiry for each participant; the prescription is a per-participant `prescriptions` row.
- **Follow-up:** a per-participant `appointments` row for the follow-up visit (auto-created if the doctor checks "Schedule follow-up").

The per-participant outcomes are **isolated**: a participant sees only their own note, prescription, and follow-up. The doctor sees all participants in a grid.

**7. Group video (online group).** If `type='telemedicine'`, the group uses a **single LiveKit room** (per `07-Appts §19.3` Telemedicine). All participants join the same room; the doctor's permissions are `moderator`; the participants are `participant`. The room supports:
- Up to 50 participants (LiveKit Cloud default; more on Enterprise).
- Mute / unmute per participant (doctor controls).
- Screen sharing (doctor only by default; can be enabled per participant).
- Breakout rooms (V2+): the doctor can split the group into smaller rooms for sub-discussion.
- Recording (if all participants have consented, per `07-Appts §19.3 §10`).
- Live captions (V2+, via OpenAI Whisper).

**8. The 3 listing / discovery UIs.**
- **Patient:** `/group-appointments` lists upcoming groups in the patient's city, with filters (specialty, date, format: in-person / online, price); each card shows the group title, doctor, time, capacity (e.g., "8 / 10 seats"), price; tap → detail page with description, doctor's profile, "Join" button.
- **Doctor:** `/doctor/group-appointments` lists the doctor's own groups (draft / open / full / started / completed) with quick actions (publish, cancel, mark started, mark completed).
- **Admin:** `/admin/group-appointments` lists all groups in the tenant with filters (status, doctor, date, format); the admin can intervene (force-cancel, refund all).

**9. API Surface.**
```
# Groups
GET    /api/v1.1/group-appointments                            -> [group] (filter: specialty, doctor, city, format, from, to, status='open')
GET    /api/v1.1/group-appointments/{id}                       -> group (with doctor, clinic, capacity, participants_count, my_status)
POST   /api/v1.1/group-appointments/{id}/join                  -> { patient_id, payment_method_id? } -> { participant, appointment, invoice? }
POST   /api/v1.1/group-appointments/{id}/leave                 -> 204 (refund per policy)
GET    /api/v1.1/group-appointments/{id}/participants          -> [participant] (doctor / admin only; with per-participant status)
POST   /api/v1.1/group-appointments/{id}/participants/{pid}/mark-attended  # doctor
POST   /api/v1.1/group-appointments/{id}/participants/{pid}/mark-no-show   # doctor
POST   /api/v1.1/group-appointments/{id}/participants/{pid}/note          # { note } (private to the doctor)
POST   /api/v1.1/group-appointments/{id}/participants/{pid}/prescription  # { rx } (creates per-participant Rx)
POST   /api/v1.1/group-appointments/{id}/participants/{pid}/follow-up    # { date, doctor_id } -> follow-up appt

# Doctor (create / manage)
POST   /api/v1.1/doctor/group-appointments                     # { title, description, doctor_id, clinic_id, start_at, duration_minutes, capacity, per_person_price, currency, format, type } -> group
PATCH  /api/v1.1/doctor/group-appointments/{id}                # { title?, description?, capacity?, per_person_price?, status? }
POST   /api/v1.1/doctor/group-appointments/{id}/publish        # draft -> open
POST   /api/v1.1/doctor/group-appointments/{id}/cancel         # { reason, refund? } -> 204 (cancels group + all participants + refunds)
POST   /api/v1.1/doctor/group-appointments/{id}/start          # marks started_at = now
POST   /api/v1.1/doctor/group-appointments/{id}/complete       # marks completed_at = now (after outcomes recorded)

# Admin
GET    /api/v1.1/admin/group-appointments                      -> [group] (all in tenant)
POST   /api/v1.1/admin/group-appointments/{id}/force-cancel    # admin override
```

**10. Realtime.**
- `group_appointment.seat_released.{tenant_id}.{group_id}` — a participant left; the seat is available; UI updates the capacity.
- `group_appointment.full.{tenant_id}.{group_id}` — capacity reached; the "Join" button is disabled.
- `group_appointment.started.{tenant_id}.{group_id}` — the doctor started the group; participants are notified.
- `group_appointment.completed.{tenant_id}.{group_id}` — the group is completed; participants are notified.
- `group_appointment.cancelled.{tenant_id}.{group_id}` — the group is cancelled; participants are refunded and notified.

**11. Security & RBAC.**
- **Capacity enforcement** is row-locked; the partial unique index is a belt-and-braces; over-booking is impossible.
- **Patient scoping** — a patient can only see / join groups they are eligible for (city / format / specialty filter).
- **Doctor scoping** — a doctor can only modify their own groups.
- **Bulk cancel requires co-sign** per `20-Security §11.4` for groups with > 10 confirmed participants.
- **Per-participant data isolation** — the doctor's note, prescription, and follow-up are private to the doctor; a participant can see only their own.
- **Audit** — every join, leave, cancel, refund is logged in `audit_logs` with the actor and the group.
- **Refund policy** — the cancellation window is per the tenant's dynamic refund policy (see Turn 55); a cancel within the window triggers a full refund; a cancel outside the window triggers a credit (per `09-Billing §21.1.5`).

**12. Acceptance Criteria.**
- [ ] A doctor can create a group with title, capacity, price, time, and format; the group is in `draft`.
- [ ] The doctor can publish; the group is `open`; patients can see it in the listing.
- [ ] A patient can join; the seat is reserved; the `booked_count` is incremented in a transaction; the over-booking is impossible.
- [ ] The capacity is reached; the group is `full`; new joins are rejected with 409.
- [ ] A participant can leave before the group; the seat is released; the refund is per the policy.
- [ ] A participant cannot leave after the group is started.
- [ ] The doctor can mark attendance, no-show, and outcomes per participant; the outcomes are private to the doctor.
- [ ] The doctor can start and complete the group; the status transitions are correct.
- [ ] The doctor can cancel the group; all participants are refunded; the audit log shows the cancellation.
- [ ] An online group uses a single LiveKit room; all participants join; the doctor has moderator permissions.
- [ ] The partial unique index on `booked_count < capacity` enforces the constraint at the DB level.

**13. Real-World Test Scenarios.**
- *Group therapy creation:* therapist creates "CBT Group — Wed 6 PM" with capacity 10, per-person price ₹500; publishes; the group appears in the listing.
- *Patient joins:* 8 patients join over 2 days; the capacity is 8/10; the 9th patient sees "Full"; the 10th sees "Full".
- *Patient leaves:* one patient leaves 24h before; the seat is released; the group is 7/10; the 9th patient can now join.
- *Group starts:* at the start time, the therapist marks "Start"; no new participants can join; the status is `started`.
- *Outcomes:* at the end, the therapist records attendance (8 attended, 0 no-show), a private note for one patient ("discussed anxiety coping"), and a follow-up appointment for another patient.
- *Group cancellation:* the therapist cancels 2 days before the group; all 8 participants are refunded in full; the audit log shows the cancellation with the list of refunds.
- *Online group:* an online prenatal class is created; 12 patients join; at the start time, all 13 (12 patients + 1 doctor) join the same LiveKit room; the recording starts (with consent); the recording is auto-deleted at 90 days.
- *Over-booking attempt:* 11 patients try to join a capacity-10 group simultaneously; 10 succeed; the 11th gets 409 (with a "Group is full, join the waitlist" CTA).
- *Refund window edge case:* a patient leaves 1 hour before the group; the policy says "full refund if > 24h, 50% if > 2h, no refund if < 2h"; the patient gets a 50% refund.

**14. What this module is NOT:**
- Not a recurring group. A group is a single session; recurring groups are V2+ (group series).
- Not a chat. The participants do not chat with each other in V1.1; group chat is V2+ (per `11-Notif §20.4`).
- Not a multi-doctor group. A group has one doctor; multi-doctor groups (e.g., a panel discussion) are V2+ (panel groups).
- Not a free group. The per-person price is required (can be ₹0 for a free group, but the column is non-null in practice).

**15. Dev Workflow.** `php artisan group:create --doctor=... --title='CBT Group' --capacity=10 --price=500` creates a test group. `php artisan group:join --group=... --patient=...` simulates a join. `php artisan group:leave --group=... --patient=...` simulates a leave. `php artisan group:overbook-test --capacity=10` runs 20 concurrent joins to verify the row-lock prevents over-booking. Code: `app-modules/group-appointments/`; tests: `app-modules/group-appointments/tests/Feature/{Create,Join,Leave,Capacity,Outcomes,Cancel,Refund,OverbookAttempt,Online}Test.php` and `tests/Performance/group-overbook.k6.js`.

### 19.3 Telemedicine video (`telemedicine-video`, V2)

**1. Overview.** V1 telemedicine was a "Zoom link in the SMS" workflow — the doctor pasted a Zoom URL into a notification. V2 builds the video room into the platform: when an appointment has `type='telemedicine'`, a LiveKit room is auto-provisioned on confirmation, the patient and doctor tap a "Join" button, and the call begins with no external app. MML 4 GA covers: WebRTC media, screen sharing, in-call chat, optional recording (with consent), call-quality metrics, and provider failover (LiveKit Cloud → Daily.co → Jitsi self-hosted).

**2. Domain Model.**
```php
Schema::create('video_rooms', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('appointment_id')->unique();
    $t->string('provider', 32);                              // 'livekit' | 'daily' | 'jitsi'
    $t->string('room_sid', 128)->unique();                   // LiveKit SID
    $t->json('participants')->nullable();                    // { patient_token, doctor_token, joined_at, left_at }
    $t->boolean('recording_enabled')->default(false);
    $t->string('recording_s3_key')->nullable();
    $t->string('recording_consent_id', 26)->nullable();      // FK to consent record
    $t->timestamp('started_at')->nullable();
    $t->timestamp('ended_at')->nullable();
    $t->integer('duration_seconds')->nullable();
    $t->json('quality_metrics')->nullable();                 // {bitrate_kbps, packet_loss_pct, jitter_ms, resolution}
    $t->string('failure_reason', 255)->nullable();
    $t->timestamps();
});

Schema::create('video_call_audit', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('video_room_id')->index();
    $t->ulid('user_id')->index();
    $t->string('action', 32);                                 // 'token_issued' | 'joined' | 'left' | 'recording_started' | 'recording_stopped' | 'ended'
    $t->string('ip_address', 45)->nullable();
    $t->string('user_agent', 255)->nullable();
    $t->string('geo_country', 2)->nullable();
    $t->timestamp('occurred_at')->index();
});
```

**3. API Surface.**
```
POST   /api/v2/video/rooms                                  # provision (auto on appointment confirm)
GET    /api/v2/video/rooms/{id}                             # metadata
POST   /api/v2/video/rooms/{id}/token                       # issue LiveKit JWT (scoped, 1h, role)
POST   /api/v2/video/rooms/{id}/recording-consent           # patient+doctor consent to record
POST   /api/v2/video/rooms/{id}/start                       # mark start
POST   /api/v2/video/rooms/{id}/end                         # mark end, trigger recording upload
GET    /api/v2/video/rooms/{id}/recording                   # short-lived S3 URL
POST   /api/v2/video/webhook                                # LiveKit/Daily/Jitsi callback (room finished, recording ready)
GET    /api/v2/video/quality/{room_id}                      # post-call quality report
POST   /api/v2/video/rooms/{id}/fallback                    # manual failover to next provider
```

**4. Frontend.**
- **Patient app:** a single "Join" button in the appointment card 5 min before start. Inside the call: a centered self-view, large mute/camera toggle, "End call" button, in-call text chat, and a "Need help?" link that opens §82 in-app chat (V2) with a support agent. The patient is blocked from joining until they accept the recording consent (if `recording_enabled`).
- **Doctor web:** full call UI with picture-in-picture patient view, screen share, file attachment, in-call notes panel that auto-saves to the encounter draft, and a "Convert to in-person" button that switches the appointment to `type='in_person'` and refunds the difference.

**5. Realtime.**
- `video.room.provisioned.{tenant}.{appointment_id}` — both sides get "Join" CTA.
- `video.participant.joined.{room_id}` / `video.participant.left.{room_id}` — presence inside the room.
- `video.recording.ready.{room_id}` — webhook from LiveKit, push to patient app.
- `video.quality.degraded.{room_id}` — fires when packet loss > 5% for 10s; client shows "Switching to lower resolution" toast.

**6. Security & RBAC.** Roles: `doctor`, `patient`, `care_team_member` (chaperone, with patient consent), `tenant_admin` (audit only — cannot join). LiveKit tokens are **server-issued** (the API never returns the LiveKit API key to the client); each token is scoped to a single room, a single identity, and a 1-hour TTL. Recording consent is a co-sign: both patient and doctor must tap "I consent" before `recording_enabled` flips to true; consent is stored as `recording_consent_id` and audit-logged. Per `20-Security §23.2.7`, all media is DTLS-SRTP encrypted end-to-end; we never decrypt or transcode; the recording is stored as an opaque MP4 in S3 with SSE-KMS. The recording is auto-deleted after 90 days (configurable per-tenant: 30/60/90/365). PHI is **not** displayed in the call UI (no patient name overlay, no MRN, no DOB); the doctor opens the chart in a separate tab. IP + user-agent + geo are stored in `video_call_audit` for every token issue, join, and leave.

**7. Acceptance Criteria.**
- [ ] Telemedicine appointment confirmation auto-provisions a LiveKit room within 5s.
- [ ] Patient and doctor both see "Join" 5 min before start; tapping it issues a JWT scoped to the room and the user's role.
- [ ] Recording is blocked until both sides consent; consent record is retrievable.
- [ ] LiveKit room SID is unique per appointment; a second appointment creates a second room.
- [ ] If LiveKit Cloud is down, the API falls back to Daily.co; if Daily is down, to self-hosted Jitsi; failure is recorded in `video_rooms.failure_reason`.
- [ ] Recordings auto-delete at 90 days (verified by a daily cron that lists S3 objects past retention).
- [ ] Quality metrics (bitrate, packet loss, jitter) are persisted for every completed call.

**8. Real-World Test Scenarios.**
- *Cross-region call:* doctor in Mumbai, patient in Dubai (high latency); the client auto-selects the LiveKit edge nearest to each side; quality metrics show sub-200ms RTT.
- *Recording consent withdrawal:* patient initially consents, then taps "Stop recording" mid-call; the room continues without recording; the partial recording is deleted; audit row written.
- *Provider outage:* LiveKit Cloud has a regional outage; the call drops; the patient app auto-reconnects via the Daily.co fallback URL embedded in the JWT metadata; no manual re-join.
- *Chaperone:* the patient invites a family member; chaperone receives a separate token with `can_publish=false` (view-only); the recording audit shows 3 participants.
- *Call abandonment:* patient joins, no doctor; 2-min timeout fires; room ends; appointment auto-flags for admin follow-up; no charge to patient.

**9. Dev Workflow.** `php artisan video:simulate --provider=livekit --concurrent=50` runs a synthetic 50-room load. `php artisan video:webhook-replay --from=...` replays LiveKit webhooks to recover from a callback outage. Code: `app-modules/telemedicine-video/`; tests: `app-modules/telemedicine-video/tests/Feature/{Room,Token,Recording,Failover}Test.php` and `tests/Performance/video-load.k6.js` (with a real LiveKit Cloud dev project).

### 19.4 Resource booking (`ot-room-scheduling`, `bed-management`, V3)

V1 appointments are doctor-centric. V2 ties them to a **resource**: a room (in-person consult), an OT (surgery), a bed (admission), a piece of equipment (X-ray, ultrasound).

```php
Schema::table('appointments', function (Blueprint $t) {
    $t->json('resources')->nullable();                       // ['resource_id_1', 'resource_id_2']
});
```

When confirming an appointment, `ResourceBookingService::book($appointment, $resourceIds)` is called. If any resource is unavailable, the appointment confirmation fails. The doctor can pick a different room or the system can suggest one.

### 19.5 Smart waitlist (`smart-waitlist`, V1.1, MML 3) — deep-dive

**1. Overview.** V1.1 builds on the V1.1 stub `appointment_waitlist` table at §19.5 (above) and turns the waitlist into a **smart, auto-matching, weighted-scoring** system. The patient sets rich preferences; when a slot opens, the system auto-fills from the waitlist using a weighted match score; the top N candidates receive a multi-channel offer with a TTL; first to claim wins; if all decline, the system offers to the next batch. MML 3 GA covers: the rich preferences (per the geo-search T53 integration), the weighted score, the multi-channel offer (SMS / WhatsApp / email / push), the auto-cascade on decline, the analytics dashboard, the group waitlist, and the recurring waitlist ("any slot in the next 2 weeks"). The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.1 lane module.

**2. Patient preferences (rich).** The patient can set, per waitlist entry:
- `preferred_doctor_id` (or "any doctor of specialty X")
- `preferred_clinic_id` (or "any clinic within 10 km")
- `preferred_from_date` / `preferred_to_date` (the date window)
- `preferred_days_of_week` (e.g., Mon-Fri only)
- `preferred_time_ranges` (e.g., 9-12, 17-20)
- `max_wait_days` (auto-cancel after N days)
- `language_preference` (e.g., "Hindi", "Tamil")
- `gender_preference` (e.g., "female doctor only")
- `urgency` (`urgent` / `normal` / `low` — affects scoring and channel priority)
- `mode` (`in_person` / `video` / `any`)
- `service_id` (specific service, e.g., "consultation", "follow-up")
- `notification_channels` (SMS / WhatsApp / email / push — the patient picks)
- `quiet_hours` (e.g., 22:00-07:00 — no notifications)
- `max_offers_per_day` (e.g., 1 — to avoid spam)
- `auto_accept_score_threshold` (e.g., 0.95 — auto-accept matches with score >= 0.95, no human in the loop)

**3. The weighted match score.** When a slot opens, the system computes a match score per candidate:
- **Doctor match:** `1.0` if the slot's doctor = preferred doctor; `0.7` if same specialty; `0.3` if different specialty.
- **Clinic match:** `1.0` if the slot's clinic = preferred clinic; `0.8` if within the patient's preferred distance (per geo-search T53); `0.4` if outside.
- **Time match:** `1.0` if the slot's time is within the preferred time range; `0.6` if on a preferred day but outside the range; `0.2` if on a non-preferred day.
- **Date match:** `1.0` if the slot's date is within the preferred window; scaled by `1 - (days_into_window / window_size)`.
- **Language / gender / mode:** `1.0` if matches; `0.0` if not (hard constraints — these exclude, they don't lower the score).
- **Urgency boost:** `+0.1` for `urgent`, `+0.0` for `normal`, `-0.1` for `low`.
- **Wait time boost:** `+0.05` per day the patient has been waiting (capped at `+0.3`).
- **Final score:** `sum(weight × value)` normalized to 0-1.

**4. The offer cascade.** When a slot opens:
1. **Find candidates:** the system queries the waitlist for entries matching the slot's doctor / clinic / date (hard constraints); computes the score for each; sorts by score desc.
2. **Pick top N:** default N=3 (configurable per tenant); the top 3 receive an offer via their preferred channels (SMS + push for `urgent`, WhatsApp for `normal`, email for `low`).
3. **Offer TTL:** default 5 minutes (configurable per tenant, 3-15 min); the offer includes the slot's details, the claim link, and the TTL.
4. **First to claim wins:** the patient's `POST /waitlist/{id}/accept` creates the appointment (per the existing `OfferWaitlistSlotJob` flow); the other 2 receive a "Sorry, taken" notification.
5. **Auto-cascade on decline / expiry:** if all 3 decline (or the TTL expires), the system offers to the next 3 (candidates 4-6); this cascades until the slot is claimed or no more candidates remain; the cascade respects `max_offers_per_day` and `quiet_hours`.
6. **Slot not claimed:** after the cascade ends, the slot is returned to the available pool (or escalated to the admin for manual handling if no candidate accepted).

**5. Group + recurring waitlists.** The smart waitlist supports 2 additional modes:
- **Group waitlist:** for group classes (per the group-appointments T50 module), the patient joins a waitlist for a specific group; when a seat opens, the system auto-fills using the same weighted score; the system supports `n_seats_opened` (e.g., 3 seats open at once) and offers the top 3 × 3 candidates.
- **Recurring waitlist:** the patient joins a "any slot in the next 2 weeks" waitlist; the system uses the recurring expander (per the recurring-appointments T49 module) to evaluate every slot in the window; the top match per slot is offered; the patient can accept or decline; the waitlist auto-cancels after `max_wait_days`.

**6. Domain Model.** Builds on the V1.1 stub at §19.5. Adds:
```php
Schema::create('waitlist_preferences', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('waitlist_id')->index();
    $t->jsonb('preferred_doctor_ids')->nullable();
    $t->string('preferred_specialty', 64)->nullable();
    $t->jsonb('preferred_clinic_ids')->nullable();
    $t->jsonb('language_preference')->nullable();
    $t->string('gender_preference', 16)->nullable();
    $t->string('mode', 16)->default('any');                      // 'in_person' | 'video' | 'any'
    $t->jsonb('notification_channels')->nullable();
    $t->string('quiet_hours_start', 5)->nullable();               // '22:00'
    $t->string('quiet_hours_end', 5)->nullable();
    $t->integer('max_offers_per_day')->default(3);
    $t->decimal('auto_accept_score_threshold', 3, 2)->nullable();
    $t->integer('max_distance_km')->nullable();
    $t->decimal('home_lat', 10, 7)->nullable();
    $t->decimal('home_lng', 10, 7)->nullable();
    $t->timestamps();
});

Schema::create('waitlist_offers', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('waitlist_id')->index();
    $t->ulid('slot_id')->index();
    $t->ulid('tenant_id')->index();
    $t->decimal('match_score', 4, 3);
    $t->string('status', 16)->default('sent');                   // 'sent' | 'claimed' | 'declined' | 'expired' | 'auto_accepted'
    $t->string('channel_primary', 16);                            // 'sms' | 'whatsapp' | 'email' | 'push'
    $t->jsonb('channel_attempts')->nullable();                    // [{ channel, sent_at, delivered_at?, error? }]
    $t->timestamp('sent_at');
    $t->timestamp('expires_at');
    $t->timestamp('responded_at')->nullable();
    $t->string('response', 16)->nullable();                       // 'claim' | 'decline'
    $t->string('decline_reason', 32)->nullable();                 // 'patient_busy' | 'too_far' | 'wrong_time' | 'no_response' | 'already_booked'
    $t->timestamps();
    $t->index(['slot_id', 'status']);
    $t->index(['waitlist_id', 'status']);
});

Schema::create('waitlist_cascades', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('slot_id')->index();
    $t->ulid('tenant_id')->index();
    $t->integer('total_candidates');
    $t->integer('total_batches');                                 // number of offer rounds
    $t->integer('total_offers_sent');
    $t->integer('total_declines');
    $t->integer('total_expiries');
    $t->string('outcome', 16);                                    // 'claimed' | 'expired' | 'admin_takeover'
    $t->ulid('winning_offer_id')->nullable();
    $t->timestamp('started_at');
    $t->timestamp('ended_at')->nullable();
    $t->timestamps();
});
```

**7. API Surface.**
```
# Patient
POST   /api/v1.1/waitlist                                      # { doctor_id, clinic_id?, service_id?, preferences... } -> waitlist entry
GET    /api/v1.1/me/waitlist                                   -> [waitlist] (the patient's own)
PATCH  /api/v1.1/waitlist/{id}                                 # { preferences? } (update preferences)
DELETE /api/v1.1/waitlist/{id}                                 # 204
GET    /api/v1.1/waitlist/{id}/offers                          -> [offer] (the offers this entry received)
POST   /api/v1.1/waitlist/offers/{offer_id}/claim              # { } -> appointment
POST   /api/v1.1/waitlist/offers/{offer_id}/decline            # { reason } -> 204

# Tenant admin
GET    /api/v1.1/admin/waitlist                                -> [waitlist] (with filters: doctor, status, score, date)
GET    /api/v1.1/admin/waitlist/{id}                           -> waitlist (with preferences, offers, history)
POST   /api/v1.1/admin/waitlist/{id}/manual-offer              # { slot_id } -> offer
POST   /api/v1.1/admin/waitlist/cascade/takeover/{slot_id}      # admin takes over the cascade (e.g., manual contact)
GET    /api/v1.1/admin/waitlist/analytics                      # { total_waitlist, by_doctor, by_urgency, avg_match_score, fill_rate, avg_cascade_batches }

# Cron
POST   /api/v1.1/internal/waitlist/cascade-tick                # runs every 30s; picks up expired offers and offers to next batch
```

**8. Frontend.**
- **Patient (`/me/waitlist`):** the patient sees their active waitlist entries; each entry shows the preferences, the offers received, the claim/decline history, and the "Cancel waitlist" button. The "Join waitlist" form is a multi-step wizard (per `18-User-Flows §18.x`).
- **Admin (`/admin/waitlist`):** the admin sees all waitlist entries with filters; the cascade view shows the per-slot cascade progress (sent → claimed / declined / expired); the analytics dashboard shows the fill rate, the average cascade batches, the top 5 doctors by waitlist demand, the no-show rate of waitlist-claimed appointments.
- **Realtime:** the patient sees the offer land in the app (push notification) and in the SMS / WhatsApp; the admin sees the cascade progress live.

**9. Realtime.**
- `waitlist.offer.received.{patient_id}` — a new offer is available; the patient is notified.
- `waitlist.offer.claimed.{tenant_id}.{slot_id}` — a slot was claimed; the cascade ends.
- `waitlist.offer.expired.{tenant_id}.{slot_id}` — an offer expired; the cascade moves to the next batch.
- `waitlist.cascade.complete.{tenant_id}.{slot_id}` — the cascade ended (claimed / expired / admin_takeover).
- `waitlist.fill_rate.{tenant_id}` — the per-tenant fill rate (hourly cron).
- `waitlist.demand.spike.{tenant_id}.{doctor_id}` — the waitlist for a doctor spiked; the admin is paged.

**10. Security & RBAC.**
- **Match score is server-side:** the client cannot manipulate the score or the offer order.
- **Claim requires authenticated patient**; the offer is bound to the patient's user_id.
- **One claim per offer:** the claim endpoint uses a DB-level lock; the first successful claim wins; concurrent claims are deduplicated.
- **TTL is strict:** expired offers return 410; the client cannot bypass the TTL.
- **Auto-accept threshold:** the patient's `auto_accept_score_threshold` is enforced server-side; the system does not auto-accept below the threshold.
- **Quiet hours:** the system respects the patient's quiet hours; the cascade skips candidates in quiet hours.
- **Max offers per day:** the system enforces `max_offers_per_day`; the cascade skips candidates who have hit the cap.
- **Roles:** `patient` (own waitlist, own offers), `tenant_admin` (own waitlist, manual takeover, view analytics), `compliance_officer` (view all, audit).
- **Audit:** every waitlist create / update / offer / claim / decline / cascade outcome is logged in `audit_logs`.

**11. Acceptance Criteria.**
- [ ] A patient can join a waitlist with rich preferences; the preferences are stored in `waitlist_preferences`.
- [ ] When a slot opens, the system computes the match score for each candidate; the top N (default 3) receive an offer.
- [ ] The offer is sent via the patient's preferred channels; the offer has a TTL (default 5 min).
- [ ] First to claim wins; concurrent claims are deduplicated (DB lock).
- [ ] If all decline or expire, the cascade moves to the next batch; the cascade respects `max_offers_per_day` and `quiet_hours`.
- [ ] The auto-accept threshold is enforced; matches >= threshold are auto-accepted (no human in the loop).
- [ ] The group waitlist auto-fills `n_seats_opened` seats using the same weighted score.
- [ ] The recurring waitlist evaluates every slot in the window; the top match per slot is offered.
- [ ] The analytics dashboard shows the fill rate, the average cascade batches, the top doctors by demand, the no-show rate of waitlist-claimed appointments.
- [ ] The patient sees the offer in the app and via SMS / WhatsApp / email / push per their preference.
- [ ] The admin can manually take over a cascade (e.g., contact a specific patient).
- [ ] The waitlist auto-cancels after `max_wait_days`.

**12. Real-World Test Scenarios.**
- *Cancellation auto-fill:* patient A cancels their Tuesday 6 PM appointment; the system finds 3 waitlist candidates with score 0.92, 0.85, 0.78; the top 3 receive an offer at 6:01 PM via SMS + push; candidate 1 claims at 6:02 PM; the slot is filled; the other 2 receive a "Sorry, taken" notification.
- *Decline cascade:* candidate 1 declines (reason: "patient_busy"); the cascade moves to candidates 4-6 at 6:06 PM; candidate 5 claims at 6:08 PM; the slot is filled.
- *TTL expiry:* all 3 candidates let the TTL expire; the cascade moves to candidates 7-9 at 6:12 PM; the cascade continues until the slot is claimed or no more candidates remain.
- *Quiet hours:* candidate 1 has `quiet_hours_start=22:00`; the offer is sent at 6 PM; the offer is delayed to 7 AM the next day; the slot is offered at 7 AM; the candidate claims or declines.
- *Auto-accept:* the slot's score is 0.97; the candidate's threshold is 0.95; the system auto-accepts the match; the appointment is created; the patient is notified.
- *Group waitlist:* 3 seats open in a group class; the system finds 9 candidates; the top 9 are offered in 3 batches of 3; 7 candidates claim; 2 seats remain; the cascade ends with 2 unfilled seats (admin takeover).
- *Recurring waitlist:* the patient joins a "any slot in the next 2 weeks" waitlist for cardiology; the system evaluates 280 slots (20 days × 14 slots); the top match per slot is offered; the patient accepts 1; the waitlist continues for the next 2 weeks.
- *Distance-based:* the slot is at a clinic 5 km from the patient's home; the patient's `max_distance_km=10`; the candidate is included in the cascade; the score includes the distance factor (0.8).
- *Hard constraint:* the patient has `gender_preference='female'`; the slot is with a male doctor; the candidate is excluded from the cascade (hard constraint).
- *No-show of waitlist-claimed:* a waitlist-claimed patient does not show up; the system's no-show rate of waitlist-claimed patients is tracked in the analytics; the system may deprioritize chronic no-shows (configurable).
- *Manual takeover:* the cascade is stuck (no candidate accepts); the admin manually contacts a specific patient; the admin uses `POST /waitlist/cascade/takeover/{slot_id}` to mark the cascade as `admin_takeover`; the slot is held for 30 min for the manual contact.

**13. Dev Workflow.** `php artisan waitlist:join --patient=... --doctor=... --preferences=...` creates a waitlist entry. `php artisan waitlist:simulate-cancellation --appointment=...` simulates a cancellation and triggers the cascade. `php artisan waitlist:tick` runs the cascade tick. `php artisan waitlist:analytics` prints the analytics. Code: `app-modules/smart-waitlist/`; tests: `app-modules/smart-waitlist/tests/Feature/{Join,MatchScore,Offer,Claim,Decline,Cascade,QuietHours,Group,Recurring,Distance,HardConstraint}Test.php` and `tests/Performance/waitlist-cascade.k6.js` (asserts 1k concurrent offers, claim race condition is dedup'd).

### 19.6 Dynamic refund window (`dynamic-refund-window`, V1.1)

V1 has a global policy: full refund >24h, 50% 4–24h, none <4h. V2 makes this **per-clinic, per-service, per-doctor** with a rules engine:

```php
Schema::create('refund_policies', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('clinic_id')->nullable()->index();
    $t->ulid('service_id')->nullable()->index();
    $t->ulid('doctor_id')->nullable()->index();
    $t->string('name', 64);
    $t->json('tiers');                                       // [{hours_before_start: 24, refund_pct: 100}, {hours_before_start: 4, refund_pct: 50}, {hours_before_start: 0, refund_pct: 0}]
    $t->boolean('is_default')->default(false);
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});
```

Resolution: service-level → doctor-level → clinic-level → tenant default.

### 19.7 In-encounter features (V2)

While in `in_consultation`, the doctor can:
- Write a prescription (`08-Prescriptions`).
- Order a lab test (`pathology-lab`, V2).
- Start in-app chat with patient (`in-app-chat`, V2).
- Begin video call if telemedicine (`telemedicine-video`, V2).
- Start AI scribe (`ai-clinical-scribe`, V2).
- Bill the patient (`09-Billing`).
- Mark the patient for follow-up (V1, 7-day default).

The encounter is a single `patient_encounters` row (V2) that ties all of these together.

### 19.8 New appointment types (V2)

| Type | Description | V1? | V2 |
|---|---|---|---|
| `in_person` | Default, walk-in or scheduled | ✓ | ✓ |
| `telemedicine` | Video consult | stub | ✓ (LiveKit) |
| `phone` | Audio-only (5 min, no video) | — | ✓ |
| `home_visit` | Doctor visits patient's home | — | ✓ (V2.1) |
| `follow_up` | Free or discounted follow-up | — | ✓ |
| `group` | Group therapy/class | — | ✓ (V1.1) |
| `recurring` | Series | — | ✓ (V1.1) |
| `resource` | Tied to OT/bed/equipment | — | ✓ (V3) |

### 19.9 V2 API additions

```
POST   /api/v2/appointments/recurring                     -> series (RRULE)
GET    /api/v2/appointment-series/{id}                    -> series with occurrences
PATCH  /api/v2/appointment-series/{id}                    -> update RRULE
DELETE /api/v2/appointment-series/{id}                    -> cancel future occurrences
POST   /api/v2/appointment-series/{id}/pause              -> pause
POST   /api/v2/appointment-series/{id}/resume             -> resume

POST   /api/v2/group-appointments                         -> group appointment
GET    /api/v2/group-appointments/{id}                    -> group with participants
POST   /api/v2/group-appointments/{id}/join               -> { patient_id }
DELETE /api/v2/group-appointments/{id}/leave              -> cancel my participation

POST   /api/v2/appointments/{id}/video-room               -> video_room
GET    /api/v2/video-rooms/{id}/token                    -> { token, url }
POST   /api/v2/video-rooms/{id}/end                      -> { ended_at, duration }
GET    /api/v2/video-rooms/{id}/recording                -> 302 to signed S3 URL

POST   /api/v2/waitlist                                  -> waitlist entry
GET    /api/v2/waitlist                                  -> [entry]
DELETE /api/v2/waitlist/{id}                             -> cancel
POST   /api/v2/waitlist/{id}/accept                      -> { slot_id } creates appointment

POST   /api/v2/appointments/{id}/refund-policies         -> preview refund
POST   /api/v2/appointments/{id}/reschedule              -> new appointment + refund
```

### 19.10 V2 acceptance criteria

- [ ] Recurring appointment expands to N rows idempotently; cancelling one keeps the series.
- [ ] Group appointment enforces capacity; over-booking is impossible.
- [ ] Telemedicine video connects two clients via LiveKit; recording works.
- [ ] OT booking is atomic with appointment confirmation.
- [ ] Waitlist auto-fills a cancelled slot within 30 seconds; first to claim wins.
- [ ] Per-clinic refund policy overrides the tenant default.
- [ ] All V2 state transitions are auditable and broadcast.
- [ ] V2 endpoints are V1-compatible (V1 clients keep working without changes).

---

## 20. V3+ Features — Deep Module Expansion

> Beyond the V2 base, V3+ adds a constellation of features that turn Appointments from "the booking module" into the **operational heart of the platform**. Each feature has a `slug`, a `lane`, a `tier`, an MML target, and a precise plan.

### 20.1 Multi-doctor appointment / panel consult (`multi-doctor-appt`, V3)

A single appointment with N doctors (tumor board, second opinion, complex case). All doctors see the patient, take notes, sign off.

```php
Schema::create('appointment_doctors', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('appointment_id')->index();
    $t->ulid('doctor_id')->index();
    $t->string('role', 16)->default('consultant');            // 'primary' | 'consultant' | 'observer' | 'scribe'
    $t->boolean('is_required_for_completion')->default(false);
    $t->timestamp('joined_at')->nullable();
    $t->timestamp('signed_off_at')->nullable();
    $t->json('notes')->nullable();
    $t->timestamps();
    $t->unique(['appointment_id', 'doctor_id']);
});
```

**State machine extension:** `in_consultation` requires at least one doctor with `role='primary'` to start; `completed` requires all `is_required_for_completion=true` doctors to have `signed_off_at`.

**Billing:** each doctor gets their own invoice line; patient can pay per-doctor or bundled.

**Use cases:** tumor board (3+ oncologists), rare disease panel, complex surgery pre-op (surgeon + anesthetist + cardiologist).

### 20.2 Second opinion marketplace (`second-opinion`, V3)

Patient uploads records, specifies condition, sets a budget. Doctors bid with credentials and timeline. Patient picks.

```php
Schema::create('second_opinion_requests', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('original_doctor_id')->nullable();             // doctor's opinion being reviewed
    $t->string('condition_summary', 4096);
    $t->json('attachments');                                 // S3 keys to medical records
    $t->decimal('budget_min', 12, 2);
    $t->decimal('budget_max', 12, 2);
    $t->integer('turnaround_days');
    $t->string('status', 16)->default('open');              // open | bidding | awarded | completed | cancelled
    $t->timestamp('bidding_deadline')->nullable();
    $t->ulid('awarded_doctor_id')->nullable();
    $t->ulid('result_appointment_id')->nullable();          // the actual consult
    $t->timestamps();
});

Schema::create('second_opinion_bids', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('request_id')->index();
    $t->ulid('doctor_id')->index();
    $t->decimal('price', 12, 2);
    $t->integer('turnaround_days');
    $t->text('proposal');                                   // doctor's pitch
    $t->string('status', 16)->default('pending');           // pending | accepted | rejected | withdrawn
    $t->timestamp('responded_at')->nullable();
    $t->timestamps();
});
```

**Flow:** patient posts request → doctors get notified (matching specialty + rating) → doctors bid → patient reviews and accepts → `Appointment` is created with the winning doctor → the bid price is held in escrow → on completion, payment released.

**Conflict of interest:** if the second-opinion doctor is in the same clinic as the original doctor, system flags it (configurable: some clinics allow internal second opinions, others forbid).

### 20.3 Patient-Reported Outcomes (`proms`, V3)

Validated questionnaires (PHQ-9 for depression, GAD-7 for anxiety, EQ-5D for QoL, Oswestry for back pain) sent to patients at intervals.

```php
Schema::create('prom_templates', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->string('slug', 32)->unique();                       // 'phq9', 'gad7', 'eq5d', 'oswestry', 'dast10', 'audit-c'
    $t->string('name', 128);
    $t->string('specialization', 64)->nullable()->index();  // 'psychiatry', 'orthopedics'
    $t->json('questions');                                  // [{id, type, text, options?, scoring}]
    $t->json('scoring_rules');                              // { total: 'sum', subscales: [...], interpretation: [...] }
    $t->boolean('is_validated')->default(true);             // clinically validated vs custom
    $t->boolean('is_active')->default(true);
});

Schema::create('prom_schedules', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->ulid('doctor_id')->nullable();
    $t->ulid('prom_template_id')->index();
    $t->date('start_date');
    $t->string('frequency', 16);                            // 'daily' | 'weekly' | 'monthly' | 'quarterly' | 'pre_visit' | 'post_visit'
    $t->integer('duration_days')->nullable();               // total duration of the schedule
    $t->string('status', 16)->default('active');            // active | completed | cancelled
    $t->json('reminder_channels')->nullable();             // ['push', 'email', 'sms']
    $t->timestamps();
});

Schema::create('prom_responses', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('schedule_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('prom_template_id')->index();
    $t->timestamp('completed_at')->index();
    $t->json('answers');                                    // {q1: 2, q2: 1, ...}
    $t->decimal('total_score', 8, 2);
    $t->json('subscale_scores')->nullable();
    $t->string('severity', 32)->nullable();                 // 'minimal' | 'mild' | 'moderate' | 'severe'
    $t->boolean('flagged_clinically')->default(false);
    $t->text('flag_reason')->nullable();
});
```

**Auto-flagging:** if PHQ-9 score > 20 (severe depression) or any question on self-harm is positive, `flagged_clinically=true`, immediate push to the doctor with a "Review urgently" CTA.

**Trend chart:** doctor's dashboard shows PROM trends per patient over time; if scores worsen, suggests a follow-up.

### 20.4 Care plans (`care-plans`, V3)

For chronic disease management (diabetes, hypertension, asthma, CKD). A structured plan with goals, interventions, milestones, and outcomes.

```php
Schema::create('care_plan_templates', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 128);
    $t->string('condition_icd10', 16)->index();
    $t->json('goals');                                      // [{description, target_value, target_unit, deadline_days}]
    $t->json('interventions');                              // [{type: 'medication' | 'lifestyle' | 'lab' | 'visit', details}]
    $t->json('milestones');                                 // [{day_offset, description, criteria}]
    $t->integer('duration_days');
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});

Schema::create('patient_care_plans', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('care_plan_template_id')->index();
    $t->ulid('prescribed_by_doctor_id')->index();
    $t->date('start_date');
    $t->date('end_date')->nullable();
    $t->string('status', 16)->default('active');            // active | completed | abandoned | on_hold
    $t->json('customizations')->nullable();                // overrides from the doctor
    $t->json('progress')->nullable();                       // {goal_id: {current_value, achieved_at?}}
    $t->timestamps();
});

Schema::create('care_plan_milestones', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('care_plan_id')->index();
    $t->string('description');
    $t->date('target_date');
    $t->date('achieved_date')->nullable();
    $t->string('status', 16)->default('pending');           // pending | achieved | missed
    $t->text('notes')->nullable();
});
```

**Workflow:** doctor selects a template, customizes for the patient, sets start date. Patient app shows the plan as a checklist: "Take Metformin 500mg twice daily", "Check fasting glucose every morning", "30-min walk 5x/week". Patient marks tasks done; system tracks adherence; auto-flags low adherence (< 50% over 7 days) to the doctor.

### 20.5 Clinical trial matching (`clinical-trials`, V3.1)

Auto-match patients to active clinical trials based on their conditions, demographics, and exclusions.

```php
Schema::create('clinical_trials', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('principal_investigator_id')->nullable();
    $t->string('registry_id', 64)->unique();                // NCT number (ClinicalTrials.gov) or CTRI (India)
    $t->string('title', 512);
    $t->text('description');
    $t->string('phase', 16);                                // 'phase_1' | 'phase_2' | 'phase_3' | 'phase_4'
    $t->string('status', 16);                               // 'not_yet_recruiting' | 'recruiting' | 'active' | 'completed' | 'terminated'
    $t->json('inclusion_criteria');                         // [{type, field, operator, value, icd10?, loinc?, age_range?, gender?}]
    $t->json('exclusion_criteria');
    $t->date('start_date');
    $t->date('end_date')->nullable();
    $t->integer('target_enrollment');
    $t->integer('current_enrollment')->default(0);
    $t->ulid('sponsor_organization_id')->nullable();
    $t->string('contact_email', 128)->nullable();
    $t->json('compensation')->nullable();                   // {amount, currency, schedule}
    $t->timestamps();
});

Schema::create('clinical_trial_matches', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('trial_id')->index();
    $t->ulid('patient_id')->index();
    $t->decimal('match_score', 5, 2);                      // 0-100
    $t->json('matching_criteria');                         // what matched
    $t->json('exclusion_violations')->nullable();          // what excluded (if partial match)
    $t->string('status', 16)->default('new');              // new | contacted | interested | enrolled | declined | ineligible
    $t->timestamps();
});
```

**Matching algorithm:** `TrialMatchingService::match($patient, $trial)`:
1. **Hard exclusions:** check age, gender, pregnancy, condition interactions. If hit, return ineligible.
2. **Soft scoring:** for each inclusion criterion, +10 points; each required lab, +5.
3. **Lifestyle:** geography within X km of trial site, +5; language match, +5.
4. **Recent history:** recent visit to relevant doctor, +10.

**Sync with registries:** nightly job pulls from `ClinicalTrials.gov` v2 API, adds new trials, updates statuses, archives completed.

**Patient view:** app shows "Clinical trials you may qualify for" with a clear "This is research, not standard care" disclaimer.

### 20.6 Voice-first interface (`voice-first`, V3.1)

Alexa Skill + Google Action + Apple Shortcuts.

- "Alexa, ask Medicare to book an appointment with Dr. Jane tomorrow at 6 PM."
- "Hey Google, when is my next appointment?"
- iOS Shortcut: "Hey Siri, schedule my next appointment."

**Architecture:**
- Voice → Twilio Voice / Alexa Skills Kit / Google Assistant SDK
- Our NLU parser (or Dialogflow/Lex) extracts intent + entities
- API call to standard endpoints (auth via voice-specific token)
- TTS response

**Edge cases:**
- Ambiguity ("next week") → ask clarifying question.
- Auth → voice PIN (4 digits) or biometric via phone.
- Privacy → no PHI spoken back (no test results, no diagnosis).

### 20.7 Wearables integration (`wearables`, V3.1)

Pull continuous vitals from patient devices into the chart.

| Device | Data | Auth | V3 tier |
|---|---|---|---|
| Apple Watch | HR, HRV, ECG, SpO2, sleep | HealthKit (patient consent) | Pro |
| Fitbit | HR, steps, sleep, SpO2, temp | OAuth | Pro |
| Garmin | HR, HRV, stress, body battery | OAuth | Pro |
| Oura Ring | sleep, HR, HRV, temp | OAuth | Pro |
| Withings (BPM) | Blood pressure | OAuth | Pro |
| Dexcom G7 | Continuous glucose | OAuth + share code | Enterprise |
| Abbott Libre | Continuous glucose | OAuth | Enterprise |
| iHealth | Glucose, BP, weight | OAuth | Pro |
| KardiaMobile | ECG | OAuth | Pro |

```php
Schema::create('wearable_connections', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->string('provider', 32);                             // 'apple_health', 'fitbit', 'garmin', 'oura', 'dexcom'
    $t->string('external_user_id', 128);                    // provider-side ID
    $t->json('access_token')->nullable();                   // encrypted
    $t->json('refresh_token')->nullable();                  // encrypted
    $t->timestamp('token_expires_at')->nullable();
    $t->string('scope', 512)->nullable();                   // OAuth scopes granted
    $t->json('permissions')->nullable();                    // granular: HR, SpO2, sleep, etc.
    $t->boolean('sync_enabled')->default(true);
    $t->timestamp('last_synced_at')->nullable();
    $t->timestamps();
    $t->unique(['patient_id', 'provider']);
});

Schema::create('wearable_readings', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->ulid('connection_id')->index();
    $t->string('metric', 32)->index();                      // 'heart_rate' | 'hrv' | 'spo2' | 'glucose' | 'steps' | 'sleep_minutes' | 'blood_pressure' | 'body_temp' | 'ecg' | 'weight'
    $t->decimal('value', 12, 4)->nullable();
    $t->string('unit', 16)->nullable();
    $t->json('value_json')->nullable();                     // for complex (ECG waveform, sleep stages)
    $t->string('source', 32);                               // 'apple_health', 'fitbit'
    $t->string('device_model', 64)->nullable();
    $t->timestamp('measured_at')->index();
    $t->timestamp('synced_at')->nullable();
    $t->index(['patient_id', 'metric', 'measured_at']);
});
```

**Sync:** cron every 15 min (or webhook from provider if supported). Vitals deduplicated by `(patient_id, metric, measured_at, source)`.

**Clinical use:** doctor sees glucose trends from CGM, HRV trends from Oura, BP from Withings — chart overlays them with appointments and medications.

**Alerts:** abnormal patterns (e.g., glucose > 250 mg/dL for 2 hours, AFib detected on Kardia) → real-time push to doctor.

### 20.8 Predictive no-show ML (`predictive-no-show`, V3.1)

Predicts probability of a no-show at booking time, suggests overbooking strategy.

```python
# medicare-ai-service/predictors/no_show.py
class NoShowPredictor:
    FEATURES = [
        'patient_age', 'patient_gender',
        'appointment_hour', 'appointment_day_of_week',
        'lead_time_days',
        'patient_historical_no_show_rate',
        'patient_historical_cancellation_rate',
        'doctor_no_show_rate',
        'clinic_no_show_rate',
        'weather_forecast_rain',
        'is_rainy_season',
        'is_holiday',
        'is_followup',
        'is_first_visit',
        'service_type',
        'previous_appointment_outcome',
        'patient_has_insurance',
        'copay_amount',
        'is_paid_in_advance',
    ]
    MODEL_PATH = 'models/no_show_v1.xgb'

    def predict(self, appointment_features: dict) -> float:
        df = pd.DataFrame([appointment_features])[self.FEATURES]
        prob = self.model.predict_proba(df)[0][1]
        return prob
```

**Training:** nightly job on completed appointments (90 days window). Features include patient history, weather (OpenWeatherMap), public holidays (Nager.Date).

**Use:**
- At booking: show "Your appointment has a 22% no-show risk. Confirm now to lock your slot."
- Admin dashboard: shows predicted vs actual no-shows per doctor per day.
- Overbooking strategy: if a slot has 30% predicted no-show, allow 1.3× booking; if both show, refund automatically per policy.

### 20.9 Specialty modules (V3.1+)

**Mental health** (`mental-health`):
- Validated screeners (PHQ-9, GAD-7, Columbia Suicide Severity, AUDIT).
- Safety planning (Stanley-Brown Safety Plan).
- Crisis resources (988 in US, iCall India).
- Session notes (45-min, SOAP-D format).
- Risk flags (suicidal ideation → page on-call).
- Telehealth-only mode (HIPAA-compliant video required).

**Oncology** (`oncology`):
- TNM staging (Tumor, Node, Metastasis).
- ECOG performance status.
- Chemo regimen templates (R-CHOP, FOLFOX, etc.).
- Cycle scheduling (D1, D8, D15 with G-CSF support).
- Toxicity grading (CTCAE v5).
- Tumor board scheduling.
- Survival analytics (Kaplan-Meier on de-identified cohorts).

**Maternal care** (`maternal`):
- Pregnancy tracking (LMP, EDD, gestational age).
- Antenatal visit schedule (8 visits, WHO standard).
- Ultrasound tracking (NT, anomaly scan, growth).
- Labor & delivery plan.
- Postpartum follow-up (6 weeks).
- High-risk flags (GDM, PIH, anemia).

**Pediatric** (`pediatric`):
- WHO growth charts (height, weight, BMI, head circumference).
- Vaccination schedule by country (India NIS, US CDC, UK NHS).
- Developmental milestones.
- Age-appropriate dosing (Clarkson's rule, BSA).
- Parental consent workflow.

**Diabetes** (`diabetes`):
- CGM integration (Dexcom, Libre).
- HbA1c tracking.
- Insulin dosing calculator.
- Hypoglycemia alert.
- Annual complications screening (eye, foot, kidney).
- Patient education library.

**Cardiology** (`cardiology`):
- ECG storage and interpretation.
- Echo reports.
- Stress test scheduling.
- Chest pain protocol (HEART score).
- Anticoagulation tracking (warfarin INR, DOAC).
- BP trend analysis.

**Dental** (`dental`, separate UI):
- Tooth chart (FDI / Universal).
- Treatment planning (per tooth).
- Perio charting.
- Imaging (periapical, OPG, CBCT).
- Lab orders (crown, denture).

**Vision/optometry** (`vision`, separate UI):
- Refraction (sphere, cylinder, axis, add).
- Visual acuity (Snellen, LogMAR).
- Intraocular pressure.
- Fundus imaging.
- Rx for glasses/contacts.

**Veterinary** (`vet`, separate fork):
- Species-aware records.
- Vaccination schedule.
- Weight tracking.
- Owner as proxy.

**Ayurveda / Homeopathy** (`ayurveda`):
- Prakriti (constitution) assessment.
- Pulse diagnosis notes.
- Custom herb formulations.
- Panchakarma scheduling.

### 20.10 Hospital-at-home (`hospital-at-home`, V3.1)

Acute care delivered at the patient's home: remote monitoring, nurse visits, IV infusions, oxygen.

```php
Schema::create('home_care_plans', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->ulid('prescribing_doctor_id')->index();
    $t->string('condition', 128);
    $t->string('acuity', 16);                              // 'low' | 'medium' | 'high'
    $t->date('start_date');
    $t->date('end_date')->nullable();
    $t->json('interventions');                             // [{type: 'iv_infusion' | 'oxygen' | 'vitals_monitor' | 'nurse_visit', schedule, parameters}]
    $t->json('escalation_rules');                          // when to call ambulance
    $t->string('status', 16)->default('active');
    $t->timestamps();
});

Schema::create('home_visits', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->ulid('nurse_id')->index();
    $t->ulid('home_care_plan_id')->nullable();
    $t->timestamp('scheduled_at')->index();
    $t->timestamp('arrived_at')->nullable();
    $t->timestamp('completed_at')->nullable();
    $t->json('vitals_recorded')->nullable();
    $t->json('interventions_performed')->nullable();
    $t->text('notes')->nullable();
    $t->string('status', 16)->default('scheduled');
});
```

**Required:** wearables (continuous vitals) + nursing network (visits) + 24/7 on-call doctor + ambulance service partnership.

### 20.11 ER triage board (`er-triage`, V4)

**1. Overview.** Emergency department: walk-in patients triaged with ESI 1–5, assigned to treatment bays, picked up by doctor, dispositioned. This module is the front line of the Hospital OS; it is the only module where a patient arrives unscheduled and is seen on a "first-by-acuity, not first-come" basis. V4 GA: ESI v.4 (5-level), Karamanlis/Bayesian optional override, treatment-bay assignment, doc-pickup queue, disposition (discharge/admit/transfer/AMA/eloped/died). The board is the dashboard for the ED charge nurse, ED physician, and unit secretary.

**2. Domain Model.**
```php
Schema::create('er_triage_encounters', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('triage_nurse_id')->index();
    $t->timestamp('arrival_at')->index();
    $t->string('esi_level', 8);                             // '1' | '2' | '3' | '4' | '5'
    $t->string('chief_complaint', 255);
    $t->json('vitals');                                     // {bp, hr, rr, spo2, temp, pain, gcs}
    $t->json('allergies');
    $t->json('medications');
    $t->string('mode_of_arrival', 32);                      // 'walk_in' | 'ambulance' | 'private_vehicle' | 'transfer'
    $t->string('ambulance_agency', 128)->nullable();
    $t->ulid('assigned_bay_id')->nullable();
    $t->ulid('assigned_doctor_id')->nullable();
    $t->timestamp('doctor_seen_at')->nullable();
    $t->timestamp('discharged_at')->nullable();
    $t->string('disposition', 32);                         // 'discharged' | 'admitted' | 'transferred' | 'left_ama' | 'eloped' | 'died'
    $t->string('disposition_destination_ward_id', 26)->nullable();  // if admitted
    $t->string('disposition_diagnosis_icd10', 16)->nullable();
    $t->timestamps();
});

Schema::create('er_bays', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 32);                                 // 'Resus-1', 'Bay-12', 'Fast-Track-3'
    $t->string('bay_type', 16);                             // 'resus' | 'acute' | 'fast_track' | 'psych' | 'isolation'
    $t->boolean('is_available')->default(true);
    $t->ulid('current_encounter_id')->nullable();
    $t->timestamps();
});
```
ESI 1 (resuscitation) is always pinned to a `bay_type='resus'`; the assignment engine refuses to drop a non-ESI-1 into a resus bay unless the patient is pediatric and the only resus bay is free.

**3. API Surface.**
```
POST   /api/v4/er/triage                                  # create encounter (from arrival screen)
GET    /api/v4/er/triage?status=waiting                   # board listing
PATCH  /api/v4/er/triage/{id}/vitals                      # re-chart vitals (overrides ESI)
PATCH  /api/v4/er/triage/{id}/esi                         # change ESI level (re-pin to top of board)
POST   /api/v4/er/triage/{id}/assign-bay                  # SELECT FOR UPDATE on bay
POST   /api/v4/er/triage/{id}/pickup                      # doctor claims encounter
PATCH  /api/v4/er/triage/{id}/disposition                 # close encounter
GET    /api/v4/er/board                                   # Kanban payload (one query, 4 lanes)
POST   /api/v4/er/board/move                              # drag-drop lane move (server validates)
GET    /api/v4/er/metrics                                 # door-to-doc time, LWBS rate, ESI distribution
```

**4. Frontend.** A single full-screen Kanban page at `/admin/er/board`. Four lanes: `Arriving` (vitals not yet taken), `Waiting` (triaged, no bay), `In Treatment` (bay assigned or doctor seen), `Disposition` (closed). Each card shows ESI color bar (red/orange/yellow/green/blue), patient initials, chief complaint, age, mode of arrival, time-in-queue. ESI 1 cards are always at the top of every lane, with a heartbeat-pulse animation. Live updates via Pusher channel `er.board.{tenant}`.

**5. Realtime.** Channels: `er.arrival.{tenant}` (new walk-in), `er.esi.changed.{tenant}` (re-triaged), `er.bay.assigned.{tenant}`, `er.pickup.{tenant}`, `er.disposition.{tenant}`, `er.board.{tenant}` (full board snapshot on demand). A "shadow mode" lets new residents observe the board without affecting state.

**6. Security & RBAC.** Roles: `triage_nurse` (create + chart vitals + assign bay), `ed_physician` (pickup + dispose + write notes), `ed_charge_nurse` (re-assign bays, override ESI, manage left-without-being-seen), `unit_secretary` (board view only). All board actions are audit-logged with `action='er.{operation}'`, including ESI changes and bay overrides. PHI visible on the board follows the same masking rules as `06-Module-Patients §2.4`; non-clinicians with `unit_secretary` see masked names.

**7. Acceptance Criteria.**
- [ ] Walk-in patient is on the board within 60s of arrival.
- [ ] ESI 1 patients are visually pinned to the top of every lane.
- [ ] Two patients cannot be assigned to the same bay simultaneously.
- [ ] Door-to-doctor time is <2 min for ESI 1, <10 min for ESI 2.
- [ ] LWBS (left without being seen) rate is tracked and exported as a metric.
- [ ] Disposition → admitted creates a `bed_assignments` row (V3 bed module per 04-Clinic-Structure §14.3).

**8. Real-World Test Scenarios.**
- *Mass casualty:* 12 ESI-2 patients arrive simultaneously; board re-orders automatically; resus bays are reserved by the engine.
- *Pediatric fever at 3am:* ESI 3; `triage_nurse` assigns to `bay_type='fast_track'`; doc sees patient in 25 min; discharged with antipyretic Rx.
- *AMA:* patient signs AMA form; disposition is recorded as `left_ama`; `audit` row stores the form scan URL.
- *Transfer:* patient needs a specialist at the partner hospital; disposition=`transferred`, partner hospital code recorded, transport method (`air_ambulance`, `ground`).

**9. Dev Workflow.** `php artisan er:simulate --esi=2 --minutes=10` runs a synthetic load against a tenant (used for load testing + demos). `php artisan er:metrics --from=... --to=...` exports the daily ED metrics report (door-to-doc, LWBS, time-to-disposition by ESI). Code lives in `app-modules/er-triage/`; tests are in `app-modules/er-triage/tests/Feature/` and `tests/Performance/er-triage.k6.js`.

### 20.12 OR scheduling (`or-schedule`, V4)

Operation theatre: book surgeon + anesthetist + nurses + equipment, pre-op checklist, post-op notes.

```php
Schema::create('surgical_cases', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->ulid('primary_surgeon_id')->index();
    $t->ulid('anesthetist_id')->nullable()->index();
    $t->ulid('operation_theatre_id')->index();
    $t->date('scheduled_date')->index();
    $t->time('scheduled_start_time');
    $t->integer('estimated_duration_minutes');
    $t->string('procedure_cpt', 16);                        // CPT code
    $t->string('procedure_name', 255);
    $t->string('status', 16)->default('scheduled');        // scheduled | pre_op | in_or | post_op | completed | cancelled
    $t->json('pre_op_checklist')->nullable();              // {consent_signed, npo_confirmed, allergies_reviewed, site_marked, ...}
    $t->json('intra_op_notes')->nullable();
    $t->json('post_op_notes')->nullable();
    $t->json('implants_used')->nullable();                 // [{name, lot, serial, manufacturer}]
    $t->json('complications')->nullable();
    $t->string('asa_classification', 4);                   // 'I' | 'II' | 'III' | 'IV' | 'V' | 'VI'
    $t->timestamps();
});

Schema::create('surgical_team_members', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('surgical_case_id')->index();
    $t->ulid('user_id')->index();                          // surgeon, scrub nurse, circulating nurse, etc.
    $t->string('role', 32);                                // 'surgeon' | 'first_assistant' | 'scrub_nurse' | 'circulating_nurse' | 'anesthetist' | 'anesthetist_assistant'
    $t->boolean('is_primary')->default(false);
    $t->timestamps();
});
```

**Schedule conflict detection:** if the same surgeon is in two ORs at the same time, the system blocks booking (configurable override for emergencies).

### 20.13 Nursing station (`nursing-station`, V4)

**1. Overview.** The clinical cockpit of an inpatient unit: nurse-to-nurse handoff (SBAR), vitals charting at the bedside, MAR (medication administration record) with Five Rights barcode verification, IV line management, wound assessment, and pending-task lists. V4 GA replaces the clipboards-with-paper workflow that V1–V3 of the platform could not address. This module is the daily driver for ~70% of inpatient nurse time.

**2. Domain Model.**
```php
Schema::create('nursing_handoffs', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('from_nurse_id')->index();
    $t->ulid('to_nurse_id')->index();
    $t->timestamp('handoff_at')->index();
    $t->text('sbar');                                      // Situation, Background, Assessment, Recommendation
    $t->json('pending_tasks');
    $t->json('allergies');
    $t->json('code_status');                               // 'full_code' | 'dnr' | 'dni' | 'comfort_care'
    $t->json('iv_lines');                                  // [{site, gauge, inserted_at, solution, rate}]
    $t->json('wounds');                                     // [{location, stage, measurement, dressing}]
    $t->json('pending_labs');
    $t->timestamps();
});

Schema::create('vital_signs', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('charted_by_nurse_id')->index();
    $t->timestamp('charted_at')->index();
    $t->string('bp_systolic', 8);
    $t->string('bp_diastolic', 8);
    $t->string('heart_rate', 8);
    $t->string('respiratory_rate', 8);
    $t->string('spo2', 8);
    $t->decimal('temperature_c', 4, 1);
    $t->string('pain_score', 4);                            // 0–10 NRS
    $t->string('gcs', 8)->nullable();                       // Glasgow Coma Scale
    $t->json('source');                                     // {device: 'spot_vital_signs', reading_id, raw_payload}
    $t->timestamps();
});

Schema::create('medication_administrations', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('prescription_line_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('administered_by_nurse_id')->index();
    $t->timestamp('scheduled_at')->index();
    $t->timestamp('administered_at')->nullable();
    $t->string('status', 16)->default('pending');          // pending | administered | refused | held | missed
    $t->text('reason_for_hold')->nullable();
    $t->string('site', 32)->nullable();                    // 'left_arm', 'right_iv', etc.
    $t->string('witness_user_id', 26)->nullable();         // for narcotics (Schedule II–V)
    $t->json('five_rights_check');                         // {patient_ok, drug_ok, dose_ok, route_ok, time_ok}
    $t->boolean('barcode_scanned')->default(false);
    $t->timestamps();
});
```

**3. API Surface.**
```
GET    /api/v4/nursing/handoffs?patient_id=...&date=...         # list
POST   /api/v4/nursing/handoffs                                  # create (shift change)
PATCH  /api/v4/nursing/handoffs/{id}/accept                      # receiving nurse confirms
POST   /api/v4/nursing/vitals                                    # chart vitals (supports HL7 FHIR Observation)
GET    /api/v4/nursing/vitals?patient_id=...&since=...           # trend query
POST   /api/v4/nursing/mar/administer                           # Five Rights verification + record
PATCH  /api/v4/nursing/mar/{id}/hold                             # hold with reason
PATCH  /api/v4/nursing/mar/{id}/refuse                           # patient refused
GET    /api/v4/nursing/mar/queue?shift=...&nurse=...             # due-now MAR
GET    /api/v4/nursing/tasks?assignee=me                         # pending tasks
POST   /api/v4/nursing/tasks/{id}/complete
```

**4. Frontend.** Three primary pages:
- `/admin/nursing/handoff` — split view: my patients on left, handoff form on right (SBAR template, allergies, code status, IVs, wounds auto-populate from `nursing_handoffs` history).
- `/admin/nursing/mar` — timeline view, current shift at top, color-coded by status (pending=blue, due=amber, late=red, done=green). Click a med → Five Rights modal → scan wristband + barcode.
- `/admin/nursing/vitals` — bedside flow, single page, large input controls, auto-saves after each field.

**5. Realtime.**
- `mar.due.{tenant}.{unit}` — paged to the assigned nurse 5 min before scheduled time.
- `mar.overdue.{tenant}.{unit}` — escalates to charge nurse at 15 min overdue.
- `vitals.critical.{tenant}.{patient}` — fires when a charted vital exceeds MEWS threshold (e.g., SBP <90, HR >130, SpO2 <90); paged to assigned nurse + on-call physician.
- `handoff.assigned.{tenant}.{nurse_id}` — incoming handoff notification.

**6. Security & RBAC.** Roles: `rn` (chart, administer), `lpn` (chart only, cannot administer), `charge_nurse` (override Five Rights with co-sign), `cna` (vitals + intake/output, no MAR). Narcotic administration requires `witness_user_id` (a second licensed user, must be `rn` or higher). All Five Rights overrides are audit-logged with `action='mar.five_rights_override'`. PHI masking follows `06-Module-Patients §2.4`.

**7. Acceptance Criteria.**
- [ ] Five Rights barcode mismatch produces a hard stop; no administration record is created.
- [ ] Narcotic administration requires a witness; admin UI blocks submission without one.
- [ ] MEWS critical vital pages the on-call physician within 30s.
- [ ] Handoff SBAR template enforces all four sections (situation, background, assessment, recommendation) before submit.
- [ ] MAR queue updates in real time as the clock advances; overdue items turn red.

**8. Real-World Test Scenarios.**
- *Code Blue:* vitals charted at 03:14 show HR 0; system pages the code team; nurse initiates a `code_blue` event from the vitals screen.
- *Wrong patient scan:* nurse scans wristband of room 312 while holding morphine for room 314; Five Rights fails on Patient-Right; alert + audit log entry.
- *Shift change at 07:00:* outgoing nurse completes handoff for 6 patients in 12 min; incoming nurse accepts each; both co-sign electronically.
- *Late dose:* vancomycin scheduled for 08:00 still pending at 08:20; MAR queue flags it red; pharmacist is paged.

**9. Dev Workflow.** `php artisan mar:simulate --schedule=...` generates a synthetic MAR queue for a unit (used in demos). `php artisan vitals:ingest --source=hl7 --file=...` replays a stream of HL7 ORU observations from a vital-signs monitor. Code: `app-modules/nursing-station/`; tests: `app-modules/nursing-station/tests/Feature/{Handoff,Vitals,Mar}Test.php` and `tests/Performance/mar-load.k6.js`.

### 20.14 Bed management (`bed-management`, V3)

See `04-CLINIC-STRUCTURE.md` §14.3. The appointment side: `bed_assignments` is queried to find available beds, patient waitlist, transport orders.

### 20.15 Equipment maintenance (`equipment-maintenance`, V4)

**1. Overview.** Asset management for hospital biomedical equipment — infusion pumps, ventilators, MRI, ultrasound, anesthesia machines, sterilizers. V4 GA covers: equipment registry with serial numbers and FDA UDI, PM (preventive maintenance) scheduling by runtime-hours or calendar, corrective work orders, parts inventory, vendor service contracts, and depreciation for the asset ledger. The equipment module was first mentioned as a stub in V3 (per `14-V2-STUBS` line 180) but is GA in V4 because it depends on the V4 finance and asset accounting integration.

**2. Domain Model.**
```php
Schema::create('equipment', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('asset_tag', 32)->unique();                  // tenant-scoped: 'EQ-00123'
    $t->string('udi_di', 64)->nullable()->index();          // FDA Unique Device Identifier Device Identifier
    $t->string('udi_pi', 128)->nullable();                  // Production Identifier (lot/serial/expiry)
    $t->string('manufacturer', 128);
    $t->string('model', 128);
    $t->string('serial_number', 128);
    $t->string('category', 32);                             // 'imaging' | 'monitoring' | 'surgical' | 'lab' | 'life_support' | 'sterilization'
    $t->string('risk_class', 4);                            // 'I' | 'II' | 'III' (FDA) or 'A' | 'B' | 'C' (IEC 62304)
    $t->date('commissioned_at')->nullable();
    $t->date('warranty_expires_at')->nullable();
    $t->date('depreciation_until')->nullable();
    $t->decimal('acquisition_cost', 12, 2);
    $t->string('current_location', 128)->nullable();        // 'ICU-Bay-3', 'OR-2'
    $t->string('status', 16)->default('active');            // 'active' | 'out_of_service' | 'retired' | 'in_calibration'
    $t->json('maintenance_schedule');                       // {pm_interval_days, runtime_hours_target, last_pm_at}
    $t->timestamps();
});

Schema::create('equipment_work_orders', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('equipment_id')->index();
    $t->string('type', 16);                                // 'preventive' | 'corrective' | 'installation' | 'calibration' | 'inspection' | 'recall'
    $t->string('priority', 16);                            // 'low' | 'medium' | 'high' | 'urgent'
    $t->text('description');
    $t->string('reported_by_user_id', 26)->nullable();
    $t->json('symptoms')->nullable();
    $t->json('parts_used')->nullable();                    // [{part_id, quantity, lot, unit_cost}]
    $t->decimal('labor_hours', 6, 2);
    $t->decimal('cost', 12, 2);
    $t->ulid('assigned_technician_id')->nullable()->index();
    $t->ulid('vendor_id')->nullable()->index();
    $t->timestamp('scheduled_at')->nullable();
    $t->timestamp('started_at')->nullable();
    $t->timestamp('completed_at')->nullable();
    $t->string('status', 16)->default('open');             // 'open' | 'in_progress' | 'awaiting_parts' | 'completed' | 'cancelled'
    $t->text('resolution_notes')->nullable();
    $t->json('attachments')->nullable();                   // [{name, url, mime}]
    $t->timestamps();
});

Schema::create('equipment_parts', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('equipment_id')->nullable()->index();          // null = generic part
    $t->string('part_number', 64);
    $t->string('description', 255);
    $t->integer('quantity_on_hand');
    $t->integer('reorder_level');
    $t->decimal('unit_cost', 10, 2);
    $t->string('preferred_vendor', 128)->nullable();
    $t->timestamps();
});

Schema::create('equipment_vendors', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 128);
    $t->string('contact_email', 128)->nullable();
    $t->string('contact_phone', 32)->nullable();
    $t->json('service_contracts');                          // [{equipment_id, start, end, cost, sla_hours}]
    $t->timestamps();
});
```

**3. API Surface.**
```
GET    /api/v4/equipment                                   # registry
POST   /api/v4/equipment
PATCH  /api/v4/equipment/{id}
GET    /api/v4/equipment/{id}                             # full record incl. WO history
GET    /api/v4/equipment/{id}/runtime                     # usage hours (from instrument logs, V4.1)
GET    /api/v4/equipment/scan?udi=...                     # lookup by barcode
POST   /api/v4/equipment/work-orders
PATCH  /api/v4/equipment/work-orders/{id}
POST   /api/v4/equipment/work-orders/{id}/complete
GET    /api/v4/equipment/work-orders?status=open
GET    /api/v4/equipment/parts
POST   /api/v4/equipment/parts/{id}/receive
GET    /api/v4/equipment/vendors
GET    /api/v4/equipment/recall-check?udi=...             # FDA recall API lookup (daily cron)
GET    /api/v4/equipment/dashboard?tenant=...              # upcoming PMs, open WOs, overdue
```

**4. Frontend.** `/admin/biomed/` is a separate vertical under the admin shell. Three primary views: **Registry** (table with filter chips: status, category, risk class, location), **PM Calendar** (Gantt view of scheduled PMs), **Work Order Board** (Kanban: open → in_progress → awaiting_parts → completed). A mobile-friendly form `/admin/biomed/wo/new` lets a nurse report a broken device by scanning its asset tag with the phone camera.

**5. Realtime.**
- `equipment.wo.opened.{tenant}.{location}` — new WO broadcast to the on-call biomedical tech.
- `equipment.wo.completed.{tenant}.{equipment_id}` — equipment returns to `active`; OR/ED can resume bookings (per V3 §14.5 equipment-booking).
- `equipment.recall.flagged.{tenant}.{udi}` — FDA recall match fires a tenant-wide alert.
- `equipment.pm.due.{tenant}.{equipment_id}` — 7-day PM reminder; escalates to the manager if not assigned.

**6. Security & RBAC.** Roles: `biomed_tech` (PM, corrective, parts), `biomed_engineer` (high-risk class III work, calibration), `biomed_manager` (assign, override, vendor PO), `vendor` (read-only access to their contracted equipment via a tokenized `/api/v4/vendor/{token}/equipment`). The `vendor` role never sees PHI; the equipment record strips patient fields. A recall event triggers an audit row `action='equipment.recall.actioned'`.

**7. Acceptance Criteria.**
- [ ] PM auto-schedule: equipment with `pm_interval_days=90` produces a WO 7 days before due.
- [ ] Work order cannot be `completed` if `parts_used` references a part with `quantity_on_hand` not decremented; atomic transaction.
- [ ] UDI scan: scanning an unknown UDI returns 404 with a "register new equipment" deep link.
- [ ] FDA recall check: daily cron fetches `https://api.fda.gov/device/recall.json` filtered by the tenant's UDI DIs; matches flagged.
- [ ] Equipment set to `out_of_service` is excluded from V3 equipment-booking slot generation (per 04-Clinic-Structure §14.5).
- [ ] Asset register depreciation schedule matches the depreciation rule (straight-line or declining-balance per tenant config).

**8. Real-World Test Scenarios.**
- *Infusion pump recall:* FDA recall on Baxter Spectrum IQ lot L-9876; system flags 12 pumps; PMs auto-scheduled for inspection; nurses notified of the affected serial numbers.
- *MRI cold head replacement:* corrective WO opened by MRI tech; awaiting_parts; vendor service contract covers labor; parts arrive; WO completed with $14,500 cost; asset value depreciated accordingly.
- *Annual PM surge:* 240 pieces of equipment have PMs due in a 30-day window; the dashboard surfaces a heatmap so the manager can level-load the team.
- *Stolen unit:* equipment `EQ-00231` reported missing; status set to `retired`; police report number stored in `retired_reason`; finance writes off the asset.
- *Uptime KPI:* the dashboard shows MRI uptime at 98.6% for Q3, derived from `(total_hours - downtime_hours) / total_hours` across the WO history.

**9. Dev Workflow.** `php artisan biomed:simulate --tenant=... --pm-rate=5/day` synthetic PM load. `php artisan biomed:recall-sync` runs the FDA recall sync manually. Code: `app-modules/equipment-maintenance/`; tests: `app-modules/equipment-maintenance/tests/Feature/{Registry,WorkOrder,Parts,Recall}Test.php` and `tests/Compliance/JcahoEquipmentTest.php` (Joint Commission audit simulation).

---

## 21. Test Matrix (Unit to Real-World)

> The full test matrix for the Appointments module. We aim for 95%+ coverage on critical paths and 100% on state machine transitions, idempotency, and concurrency.

### 21.1 Unit tests (PHPUnit, ~250 tests)

#### Model tests (60 tests)

| Test class | Cases | What it covers |
|---|---|---|
| `AppointmentModelTest` | 30 | Casts, mutators, accessors, scopes, soft-delete, fillable, hidden |
| `AppointmentSeriesModelTest` | 8 | RRULE accessor, expansion, status transitions |
| `GroupAppointmentModelTest` | 8 | Capacity check, participants relation |
| `AppointmentDoctorModelTest` | 5 | Role validation, completion requirement |
| `WaitlistEntryModelTest` | 5 | Priority sort, offer expiry |
| `RefundPolicyModelTest` | 4 | Tier evaluation, fallback |

Key cases:
```php
test('appointment casts start_at to UTC', function () {
    $a = Appointment::factory()->create(['start_at' => '2026-06-04 18:00:00', 'timezone' => 'Asia/Kolkata']);
    expect($a->start_at->timezone->name)->toBe('UTC');
    expect($a->start_at->toIso8601String())->toBe('2026-06-04T12:30:00+00:00');
});

test('appointment status is enum-backed', function () {
    $a = Appointment::factory()->create();
    expect($a->status)->toBeInstanceOf(AppointmentStatus::class);
    expect($a->status)->toBe(AppointmentStatus::Pending);
});

test('soft-deleted appointment still has children accessible', function () {
    $a = Appointment::factory()->create();
    $rx = Prescription::factory()->for($a)->create();
    $a->delete();
    expect(Prescription::find($rx->id))->not->toBeNull();
});

test('appointment patient_ids is JSON cast', function () {
    $a = Appointment::factory()->create(['patient_ids' => ['p1', 'p2']]);
    expect($a->patient_ids)->toBeArray()->toHaveCount(2);
    $raw = DB::table('appointments')->where('id', $a->id)->value('patient_ids');
    expect($raw)->toBeString(); // stored as JSON
});
```

#### State machine tests (30 tests)

Every transition, valid and invalid:
```php
test('valid transition: pending -> confirmed', function () {
    $a = Appointment::factory()->inState(AppointmentStatus::Pending)->create();
    $a->transitionTo(AppointmentStatus::Confirmed);
    expect($a->fresh()->status)->toBe(AppointmentStatus::Confirmed);
});

test('invalid transition: completed -> pending throws', function () {
    $a = Appointment::factory()->inState(AppointmentStatus::Completed)->create();
    expect(fn() => $a->transitionTo(AppointmentStatus::Pending))
        ->toThrow(InvalidStateTransitionException::class);
});

// ... 28 more transitions

test('idempotent: same transition twice is a no-op', function () {
    $a = Appointment::factory()->inState(AppointmentStatus::Confirmed)->create();
    $a->transitionTo(AppointmentStatus::CheckedIn);
    $a->transitionTo(AppointmentStatus::CheckedIn); // no-op
    expect(AuditLog::where('entity_id', $a->id)->count())->toBe(1); // only one audit row
});
```

#### Service tests (60 tests)

`AppointmentService` business logic:
```php
test('book creates appointment with idempotency key', function () {...});
test('book rejects past start time', function () {...});
test('book rejects when slot is held by another user', function () {...});
test('confirm generates invoice and triggers payment', function () {...});
test('cancel before 24h refunds 100%', function () {...});
test('cancel between 4-24h refunds 50%', function () {...});
test('cancel <4h refunds 0%', function () {...});
test('reschedule creates new appointment, refunds old', function () {...});
test('bulk cancel refunds per policy', function () {...});
test('checkin updates status, records queue token', function () {...});
test('start requires checked_in state', function () {...});
test('complete triggers review request and broadcasts', function () {...});
test('no_show cron marks appointments past start+30', function () {...});
// ... 45 more
```

#### Value object tests (15 tests)
```php
test('RRule parses and expands', function () {...});
test('RRule handles EXDATE exclusions', function () {...});
test('RRule handles RDATE additions', function () {...});
test('RRule handles DST transitions correctly', function () {...});
test('SlotHold expires after 10 min', function () {...});
test('IdempotencyKey hashes payload correctly', function () {...});
// ...
```

#### Job tests (20 tests)
```php
test('SendAppointmentReminderJob sends 24h reminder', function () {...});
test('SendAppointmentReminderJob is idempotent (same appt, same template, only once)', function () {...});
test('GenerateSlotsJob creates 60 days of slots in <1s', function () {...});
test('MarkNoShowJob runs every 5 min', function () {...});
test('OfferWaitlistSlotJob picks top 3 by priority', function () {...});
test('ReleaseExpiredSlotHoldsJob runs every 1 min', function () {...});
// ...
```

#### Event/Listener tests (15 tests)
```php
test('AppointmentConfirmed broadcasts on private channel', function () {...});
test('AppointmentCancelled sends refund email', function () {...});
test('AppointmentRescheduled sends both confirmations', function () {...});
test('QueueCalled flashes display screen', function () {...});
// ...
```

#### Form Request tests (15 tests)
```php
test('BookAppointmentRequest validates all required fields', function () {...});
test('BookAppointmentRequest validates start_at is future', function () {...});
test('BookAppointmentRequest validates doctor_id belongs to tenant', function () {...});
test('BookAppointmentRequest validates patient_id is bookable', function () {...});
test('CancelAppointmentRequest validates reason min length', function () {...});
// ...
```

#### Policy tests (15 tests)
```php
test('PatientPolicy: can view own appointment', function () {...});
test('PatientPolicy: cannot view other patient appointment', function () {...});
test('DoctorPolicy: can view own schedule', function () {...});
test('DoctorPolicy: cannot view other doctor appointment', function () {...});
test('AdminPolicy: can view all tenant appointments', function () {...});
test('AdminPolicy: cannot view cross-tenant', function () {...});
// ...
```

#### Cast tests (10 tests)
```php
test('MoneyCast encrypts and decrypts', function () {...});
test('EncryptedFieldCast with tenant KMS', function () {...});
test('RRuleCast serializes to RFC 5545', function () {...});
test('TimezoneCast converts to UTC on write', function () {...});
// ...
```

### 21.2 Integration tests (Pest, ~100 tests)

#### Database integration
```php
test('concurrent booking: only one wins', function () {
    $slot = $this->createSlot();
    [$a, $b] = [Thread::start(fn() => book($slot)), Thread::start(fn() => book($slot))];
    $successes = collect([$a->join(), $b->join()])->filter()->count();
    expect($successes)->toBe(1);
});
test('referential integrity: cannot delete doctor with appointments', function () {...});
test('FK cascade: deleting patient soft-deletes appointments', function () {...});
test('tenant isolation: tenant A cannot see tenant B appointments', function () {...});
test('audit log: every state transition logged', function () {...});
test('event broadcast: appointment.confirmed reaches patient channel', function () {...});
```

#### Queue + Job integration
```php
test('reminder job idempotency via sent_log', function () {...});
test('failed job retries 3x with backoff', function () {...});
test('failed job permanently records in failed_jobs', function () {...});
test('horizon dashboard accessible to super-admin only', function () {...});
```

#### Cache integration
```php
test('available_slots cache hit returns same as fresh query', function () {...});
test('cache invalidated on schedule change', function () {...});
test('cache prefix isolated per tenant', function () {...});
```

#### Webhook integration (mocked)
```php
test('Razorpay payment.captured marks invoice paid', function () {...});
test('Stripe payment_intent.succeeded handles async', function () {...});
test('Paystack charge.success idempotency', function () {...});
test('webhook with bad signature returns 401', function () {...});
```

#### Search integration
```php
test('appointment indexed in Meilisearch on create', function () {...});
test('search returns only tenant-isolated results', function () {...});
```

### 21.3 HTTP / Feature tests (~150 tests)

Every endpoint, every method, every role, every error case.

```php
test('POST /api/v1/appointments as patient returns 201', function () {...});
test('POST /api/v1/appointments as guest returns 401', function () {...});
test('POST /api/v1/appointments as wrong role returns 403', function () {...});
test('POST /api/v1/appointments with invalid data returns 422', function () {...});
test('POST /api/v1/appointments with idempotency key is idempotent', function () {...});
test('GET /api/v1/appointments returns paginated list', function () {...});
test('GET /api/v1/appointments filters by doctor, date, status', function () {...});
test('GET /api/v1/appointments/{id} returns 200', function () {...});
test('GET /api/v1/appointments/{id} of cross-tenant returns 404', function () {...});
test('PATCH /api/v1/appointments/{id} updates notes', function () {...});
test('POST /api/v1/appointments/{id}/confirm returns 200', function () {...});
test('POST /api/v1/appointments/{id}/cancel returns 200 with refund', function () {...});
test('POST /api/v1/appointments/{id}/checkin returns 200 and triggers queue', function () {...});
test('POST /api/v1/appointments/{id}/start requires checkin', function () {...});
test('POST /api/v1/appointments/{id}/complete triggers review request', function () {...});
test('POST /api/v1/appointments/{id}/reschedule atomically', function () {...});
test('GET /api/v1/appointments/calendar returns ical feed', function () {...});
test('POST /api/v1/appointments/bulk-cancel with admin auth', function () {...});
// ... 130+ more for V2/V3 endpoints
```

### 21.4 E2E / Browser tests (Playwright, ~30 tests)

Real user flows on real browsers:

```typescript
test('patient books appointment end-to-end', async ({ page }) => {
  await page.goto('/doctors/dr-jane');
  await page.click('[data-testid="book-slot-tomorrow-6pm"]');
  await page.fill('[data-testid="phone"]', '+919876543210');
  await page.click('[data-testid="verify-otp"]');
  await page.fill('[data-testid="otp"]', '123456');
  await page.click('[data-testid="pay-now"]');
  await page.fill('[data-testid="card"]', '4242424242424242');
  await page.click('[data-testid="confirm-payment"]');
  await expect(page.locator('[data-testid="confirmation"]')).toBeVisible();
});

test('doctor day view shows today appointments sorted by time', async ({ page }) => {...});
test('front desk calls patient, display screen flashes', async ({ page, context }) => {
  const [adminPage, displayPage] = await Promise.all([context.newPage(), context.newPage()]);
  await adminPage.goto('/admin/queue');
  await displayPage.goto('/display/clinic-1');
  await adminPage.click('[data-testid="call-patient-token-3"]');
  await expect(displayPage.locator('[data-testid="called-token-3"]')).toBeVisible();
});
// ... 27 more
```

Mobile (Flutter integration tests):
```dart
testWidgets('patient books appointment', (tester) async {
  await tester.pumpWidget(MedicareApp());
  await tester.tap(find.text('Find Doctor'));
  await tester.tap(find.byKey(Key('doctor-dr-jane')));
  await tester.tap(find.text('Tomorrow 6 PM'));
  await tester.tap(find.text('Confirm'));
  await tester.pumpAndSettle();
  expect(find.text('Appointment Confirmed'), findsOneWidget);
});
```

### 21.5 Performance tests (k6, ~10 tests)

```javascript
import http from 'k6/http';
import { check, sleep } from 'k6';

export const options = {
  stages: [
    { duration: '30s', target: 100 },   // ramp to 100 users
    { duration: '5m', target: 100 },    // hold
    { duration: '30s', target: 1000 },  // spike
    { duration: '5m', target: 1000 },   // hold spike
    { duration: '1m', target: 0 },      // ramp down
  ],
  thresholds: {
    http_req_duration: ['p(95)<300', 'p(99)<500'],
    http_req_failed: ['rate<0.01'],
  },
};

export default function () {
  const res = http.get('https://staging.medicare.app/api/v1/public/doctors?city_id=1');
  check(res, {
    'status 200': (r) => r.status === 200,
    'p95 < 300ms': (r) => r.timings.duration < 300,
  });
  sleep(1);
}
```

Tests:
- `public_doctor_search.js` — 1k concurrent, p95<300ms
- `appointment_booking.js` — 100 concurrent bookings, no double-book
- `telemedicine_token.js` — 500 concurrent LiveKit token requests
- `waitlist_offer.js` — 1k waiters, race condition
- `slot_calendar_60d.js` — load calendar with 10k slots
- `reverb_broadcast.js` — 1k concurrent WebSocket connections
- `soak_test.js` — 24h sustained load
- `spike_test.js` — sudden 10x traffic
- `stress_test.js` — find breaking point
- `cache_thrash.js` — cache eviction behavior

### 21.6 Security tests (~20 tests)

OWASP-aligned:
```php
test('SQLi: search query with quote returns 400', function () {...});
test('XSS: appointment notes are sanitized on render', function () {...});
test('CSRF: state-changing requires valid token', function () {...});
test('IDOR: cannot access other tenant appointment', function () {...});
test('Mass assignment: only fillable fields saved', function () {...});
test('Auth: expired token returns 401', function () {...});
test('Authz: patient cannot confirm appointment', function () {...});
test('Rate limit: 100 reqs/min returns 429', function () {...});
test('Webhook signature: tampered returns 401', function () {...});
test('File upload: malicious file rejected', function () {...});
test('Audit log: every PHI read recorded', function () {...});
test('Headers: CSP, HSTS, X-Frame-Options present', function () {...});
test('Tenant leak: error messages never include tenant_id', function () {...});
test('Enumeration: timing attack on appointment_id mitigated', function () {...});
// ... 6 more
```

Penetration tests (quarterly, manual):
- OWASP Top 10 retest
- Auth bypass attempts
- Privilege escalation
- Injection (SQL, NoSQL, LDAP, OS)
- XXE, SSRF, SSJI
- Business logic flaws (refund abuse, double-booking, race conditions)

### 21.7 Accessibility tests (axe-core, ~5 tests)

```typescript
test('home page is WCAG 2.1 AA compliant', async ({ page }) => {
  await page.goto('/');
  const results = await new AxeBuilder({ page }).analyze();
  expect(results.violations).toEqual([]);
});
test('booking form is keyboard navigable', async ({ page }) => {...});
test('color contrast meets AA', async ({ page }) => {...});
test('screen reader can read appointment details', async ({ page }) => {...});
test('focus indicators visible on all interactive elements', async ({ page }) => {...});
```

### 21.8 Compatibility matrix

| Component | Versions tested | CI |
|---|---|---|
| **PHP** | 8.2, 8.3, 8.4 | ✓ matrix |
| **Laravel** | 11.x, 12.x | ✓ matrix |
| **MySQL** | 8.0, 8.4 | ✓ matrix |
| **MariaDB** | 10.11, 11.4 | ✓ matrix |
| **PostgreSQL** | 15, 16, 17 (V2) | ✓ matrix |
| **Redis** | 7.0, 7.2, 7.4 | ✓ matrix |
| **Node** | 20, 22 | ✓ matrix |
| **Flutter** | 3.24, 3.27 | ✓ matrix |
| **Browsers (web)** | Chrome, Firefox, Safari, Edge (latest 2) | ✓ Playwright |
| **OS (dev)** | macOS, Ubuntu 22.04, Windows 11 | ✓ |
| **Mobile devices** | iPhone 12-15, Pixel 6-8, Galaxy S22-S24 | ✓ BrowserStack |

### 21.9 Real-world scenarios (Playwright + real services)

| Scenario | Services used | Pass criteria |
|---|---|---|
| Real Firebase phone OTP | Firebase test phone | SMS received, verify works |
| Real FCM push | Firebase test device | Push received, deep-link works |
| Real Razorpay test mode | Razorpay test keys | Payment succeeds, webhook fires |
| Real Stripe test mode | Stripe test keys | PaymentIntent succeeds |
| Real Paystack test mode | Paystack test keys | Transaction succeeds |
| Real email (Mailtrap) | SMTP | Email received, links work |
| Real video (LiveKit Cloud dev) | LiveKit dev project | 2 clients connect, audio/video |
| Real ABDM (NHA sandbox) | NHA sandbox | ABHA linking works |
| Real LLM (Ollama local) | Ollama + qwen2.5:7b | Scribe produces SOAP |
| Real lab partner (mock) | lab-mock-server | Order → result flow works |
| Real insurance (mock) | payer-mock-server | Preauth → claim flow works |
| Real WhatsApp (test number) | WhatsApp test | Template sends, delivery webhook |
| Real SMS (Twilio test) | Twilio test creds | SMS received |
| Real Mapbox | Mapbox dev | Map renders, markers work |

### 21.10 Test data management

- **Factories:** `AppointmentFactory`, `AppointmentSeriesFactory`, `WaitlistFactory`, etc. Each with 3-5 states (`pending()`, `confirmed()`, `completed()`, `noShow()`, `cancelled()`).
- **Seeders:** `DevSeeder` for development, `DemoSeeder` for sales demos, `LoadTestSeeder` for k6.
- **Faker locales:** `en`, `hi`, `ar`, `es`, `fr`, `de` for i18n.
- **Anonymization:** production-data-anonymized fixture for staging tests.
- **Snapshots:** S3 snapshots of test data committed to `tests/fixtures/snapshots/`.

### 21.11 Test infrastructure

- **Pest 3.x** for PHP tests.
- **PHPUnit 11.x** for legacy.
- **Mockery** for service mocks.
- **Mockery::mock(Queue::class)** for queue assertions.
- **Carbon::setTestNow()** for time travel.
- **Spatie\LaravelRay** for debugging failing tests.
- **Playwright 1.50** for E2E.
- **MailHog** for email capture in tests.
- **WireMock** for external service mocks.
- **k6** for performance.
- **OWASP ZAP** for security.
- **axe-core** for accessibility.
- **Codecov** for coverage.
- **Qodana** for static analysis.
- **SonarCloud** for code quality.

### 21.12 Coverage targets

| Layer | Target | Block PR below |
|---|---|---|
| Models | 95% | 80% |
| Services | 95% | 85% |
| Controllers | 90% | 75% |
| Jobs | 95% | 85% |
| Events/Listeners | 90% | 75% |
| Form Requests | 100% | 90% |
| Policies | 100% | 90% |
| Casts | 100% | 80% |
| Overall | 90% | 80% |

### 21.13 Test execution

```bash
# Local
./vendor/bin/pest                    # all
./vendor/bin/pest --filter=Booking   # by name
./vendor/bin/pest --group=critical   # by group
./vendor/bin/pest --parallel --processes=8

# CI (matrix)
strategy:
  matrix:
    php: [8.2, 8.3, 8.4]
    laravel: [11.x, 12.x]
    db: [mysql8, mariadb10.11, postgres16]
steps:
  - run: ./vendor/bin/pest --ci --coverage --min=80

# Smoke (post-deploy)
./scripts/smoke-test.sh production
# - Real login
# - Real booking
# - Real payment
# - Real notification
# - Real audit log entry
```

### 21.14 Continuous testing

- **Pre-commit:** static analysis (PHPStan level 9), code style (Pint), unit tests for changed files.
- **Pre-merge:** full test suite + coverage check + SAST.
- **Pre-deploy:** smoke tests against staging.
- **Post-deploy:** smoke tests against production.
- **Scheduled:** nightly full suite + weekly E2E + monthly performance + quarterly security.

---

## 22. Deep Dive — State Machine Implementation

> The state machine is the **single most critical piece** of the appointments module. Every transition is atomic, audited, broadcast, and reversible. This deep dive walks through the full implementation.

### 22.1 The state machine, formally

States: `pending, confirmed, checked_in, in_consultation, completed, cancelled, no_show, failed, cancellation_requested`.

Valid transitions (15 in total):
```
pending → confirmed          (patient pays, system confirms)
pending → cancelled           (patient cancels before payment)
pending → failed              (payment fails)
pending → cancellation_requested (V2 refund flow)

confirmed → checked_in        (patient arrives, scans QR)
confirmed → cancelled         (patient cancels after payment → refund)
confirmed → no_show           (cron: 30 min past start, no check-in)
confirmed → cancellation_requested (V2 refund flow)

checked_in → in_consultation  (doctor taps "Start")
checked_in → cancelled        (rare, with reason)
checked_in → no_show          (rare, after long wait)

in_consultation → completed   (doctor taps "Complete")

cancellation_requested → confirmed (refund issued, appointment re-confirmed via rebook)
cancellation_requested → cancelled (refund completed, appointment cancelled)

(no other transitions allowed)
```

### 22.2 The enum

```php
namespace App\Modules\Appointments\Enums;

enum AppointmentStatus: string implements HasLabel, HasColor {
    case Pending = 'pending';
    case Confirmed = 'confirmed';
    case CheckedIn = 'checked_in';
    case InConsultation = 'in_consultation';
    case Completed = 'completed';
    case Cancelled = 'cancelled';
    case NoShow = 'no_show';
    case Failed = 'failed';
    case CancellationRequested = 'cancellation_requested';

    public function label(): string {
        return match($this) {
            self::Pending => 'Pending Payment',
            self::Confirmed => 'Confirmed',
            self::CheckedIn => 'Checked In',
            self::InConsultation => 'In Consultation',
            self::Completed => 'Completed',
            self::Cancelled => 'Cancelled',
            self::NoShow => 'No Show',
            self::Failed => 'Payment Failed',
            self::CancellationRequested => 'Cancellation Requested',
        };
    }

    public function color(): string {
        return match($this) {
            self::Pending, self::CancellationRequested => 'yellow',
            self::Confirmed, self::CheckedIn, self::InConsultation => 'blue',
            self::Completed => 'green',
            self::Cancelled, self::Failed, self::NoShow => 'red',
        };
    }

    public function isTerminal(): bool {
        return in_array($this, [self::Completed, self::Cancelled, self::NoShow, self::Failed]);
    }

    public function requiresQueueToken(): bool {
        return in_array($this, [self::CheckedIn, self::InConsultation]);
    }
}
```

### 22.3 The transition matrix

```php
namespace App\Modules\Appointments\Support;

class AppointmentStateMachine {
    private const TRANSITIONS = [
        'pending' => [
            'confirmed', 'cancelled', 'failed', 'cancellation_requested',
        ],
        'confirmed' => [
            'checked_in', 'cancelled', 'no_show', 'cancellation_requested',
        ],
        'checked_in' => [
            'in_consultation', 'cancelled', 'no_show',
        ],
        'in_consultation' => [
            'completed',
        ],
        'cancellation_requested' => [
            'confirmed', 'cancelled',
        ],
        'completed' => [],
        'cancelled' => [],
        'no_show' => [],
        'failed' => [],
    ];

    public static function canTransition(AppointmentStatus $from, AppointmentStatus $to): bool {
        return in_array($to->value, self::TRANSITIONS[$from->value] ?? []);
    }

    public static function allowedTransitionsFrom(AppointmentStatus $from): array {
        return array_map(
            fn($s) => AppointmentStatus::from($s),
            self::TRANSITIONS[$from->value] ?? []
        );
    }

    /**
     * Validate the state machine is well-formed (no unreachable states, no self-loops except idempotent).
     */
    public static function validate(): void {
        $allStates = array_map(fn($c) => $c->value, AppointmentStatus::cases());
        foreach (self::TRANSITIONS as $from => $tos) {
            foreach ($tos as $to) {
                if (!in_array($to, $allStates)) {
                    throw new \LogicException("Transition $from -> $to targets unknown state");
                }
                if ($from === $to) {
                    throw new \LogicException("Self-loop $from -> $to not allowed (use idempotent flag)");
                }
            }
        }
        // Check reachability: every non-initial state must be reachable from pending
        $reachable = ['pending'];
        $changed = true;
        while ($changed) {
            $changed = false;
            foreach ($reachable as $s) {
                $new = self::TRANSITIONS[$s] ?? [];
                foreach ($new as $n) {
                    if (!in_array($n, $reachable)) {
                        $reachable[] = $n;
                        $changed = true;
                    }
                }
            }
        }
        foreach ($allStates as $s) {
            if (!in_array($s, $reachable)) {
                throw new \LogicException("State $s is unreachable from pending");
            }
        }
    }
}
```

### 22.4 The model method

```php
namespace App\Modules\Appointments\Models;

class Appointment extends Model {
    use BelongsToTenant, HasUuids, SoftDeletes, Auditable;

    protected $fillable = [
        'tenant_id', 'patient_id', 'doctor_id', 'clinic_id',
        'start_at', 'end_at', 'timezone',
        'status', 'type', 'reason', 'notes',
        'parent_appointment_id', 'rrule', 'is_recurring_anchor',
        'group_appointment_id', 'is_group', 'group_capacity',
        'video_room_id', 'resources',
        'cancellation_reason', 'cancelled_at', 'cancelled_by',
        'checked_in_at', 'started_at', 'completed_at',
        'amount', 'currency', 'invoice_id', 'transaction_id',
        'queue_token', 'queue_called_at', 'queue_served_at',
    ];

    protected $casts = [
        'start_at' => 'immutable_datetime',
        'end_at' => 'immutable_datetime',
        'checked_in_at' => 'immutable_datetime',
        'started_at' => 'immutable_datetime',
        'completed_at' => 'immutable_datetime',
        'cancelled_at' => 'immutable_datetime',
        'queue_called_at' => 'immutable_datetime',
        'queue_served_at' => 'immutable_datetime',
        'is_recurring_anchor' => 'boolean',
        'is_group' => 'boolean',
        'resources' => 'array',
        'status' => AppointmentStatus::class,
        'amount' => Money::class,
    ];

    /**
     * Atomic state transition with side effects.
     *
     * @param AppointmentStatus $to Target state
     * @param array $context Extra context: actor, reason, payment_id, etc.
     * @return self
     * @throws InvalidStateTransitionException
     */
    public function transitionTo(AppointmentStatus $to, array $context = []): self {
        return DB::transaction(function () use ($to, $context) {
            // 1. Lock the row
            $fresh = self::lockForUpdate()->findOrFail($this->id);
            
            // 2. Check transition validity
            $from = $fresh->status;
            if ($from === $to) {
                return $fresh; // idempotent
            }
            if (!AppointmentStateMachine::canTransition($from, $to)) {
                throw new InvalidStateTransitionException(
                    "Cannot transition appointment {$this->id} from {$from->value} to {$to->value}"
                );
            }

            // 3. Apply the state change
            $fresh->status = $to;
            $this->applyTransitionTimestamps($fresh, $to);
            $fresh->save();

            // 4. Side effects
            $this->applyTransitionSideEffects($fresh, $from, $to, $context);

            // 5. Audit
            AuditLog::create([
                'tenant_id' => $fresh->tenant_id,
                'entity_type' => 'appointment',
                'entity_id' => $fresh->id,
                'action' => "transition.{$to->value}",
                'actor_user_id' => $context['actor_user_id'] ?? null,
                'old_values' => ['status' => $from->value],
                'new_values' => ['status' => $to->value],
                'context' => $context,
                'ip' => request()?->ip(),
                'user_agent' => request()?->userAgent(),
            ]);

            // 6. Broadcast
            broadcast(new AppointmentStateChanged($fresh, $from, $to))->toOthers();

            // 7. Refresh model
            $this->setRawAttributes($fresh->getAttributes());
            $this->syncOriginal();

            return $this;
        });
    }

    private function applyTransitionTimestamps(self $a, AppointmentStatus $to): void {
        $now = now();
        match($to) {
            AppointmentStatus::CheckedIn => $a->checked_in_at = $now,
            AppointmentStatus::InConsultation => $a->started_at = $now,
            AppointmentStatus::Completed => $a->completed_at = $now,
            AppointmentStatus::Cancelled => $a->cancelled_at = $now,
            default => null,
        };
    }

    private function applyTransitionSideEffects(self $a, AppointmentStatus $from, AppointmentStatus $to, array $context): void {
        match($to) {
            AppointmentStatus::Confirmed => $this->onConfirm($a, $context),
            AppointmentStatus::CheckedIn => $this->onCheckIn($a, $context),
            AppointmentStatus::InConsultation => $this->onStart($a, $context),
            AppointmentStatus::Completed => $this->onComplete($a, $context),
            AppointmentStatus::Cancelled => $this->onCancel($a, $context),
            AppointmentStatus::NoShow => $this->onNoShow($a, $context),
            AppointmentStatus::Failed => $this->onFail($a, $context),
            AppointmentStatus::CancellationRequested => $this->onCancellationRequested($a, $context),
            default => null,
        };
    }

    private function onConfirm(self $a, array $context): void {
        // Issue queue token if confirmed and start is within 24h
        if ($a->start_at->diffInHours(now()) < 24) {
            $a->queue_token = QueueService::issueToken($a);
        }
    }

    private function onCheckIn(self $a, array $context): void {
        $a->queue_called_at = null;
        $a->queue_served_at = null;
        // Trigger live update of queue display
    }

    private function onStart(self $a, array $context): void {
        $a->queue_served_at = now();
    }

    private function onComplete(self $a, array $context): void {
        // Request review
        $a->doctor->patient()?->notify(new ReviewRequestNotification($a));
        // Add to doctor's earnings
        DoctorEarningService::record($a);
    }

    private function onCancel(self $a, array $context): void {
        $a->cancellation_reason = $context['reason'] ?? null;
        $a->cancelled_by = $context['actor_user_id'] ?? null;
        // Refund per policy
        $refund = RefundService::calculate($a, $context['reason'] ?? 'user_cancelled');
        if ($refund['amount']->isPositive()) {
            RefundService::issue($a, $refund);
        }
        // Release resource
        if ($a->video_room_id) {
            VideoRoomService::release($a->video_room_id);
        }
        // Notify waitlist
        OfferWaitlistSlotJob::dispatch($a);
    }

    private function onNoShow(self $a, array $context): void {
        // Mark patient as no-show, may affect future booking privileges
        $a->patient()->increment('no_show_count');
        // Add to doctor's earnings? Depends on policy. V2 configurable.
    }

    private function onFail(self $a, array $context): void {
        $a->cancellation_reason = 'payment_failed';
        // Release slot
    }

    private function onCancellationRequested(self $a, array $context): void {
        // V2 async refund flow
        $a->cancellation_reason = $context['reason'] ?? null;
        ProcessCancellationRefundJob::dispatch($a);
    }
}
```

### 22.5 The visualizer

`/api/v1/appointments/{id}/state-machine` returns a JSON of the state graph for this appointment, used by the doctor/admin UI to show available actions:

```json
{
  "current": "confirmed",
  "allowed": ["checked_in", "cancelled", "no_show", "cancellation_requested"],
  "graph": {
    "nodes": [
      {"id": "pending", "label": "Pending"},
      {"id": "confirmed", "label": "Confirmed"},
      ...
    ],
    "edges": [
      {"from": "pending", "to": "confirmed", "label": "Pay"},
      {"from": "confirmed", "to": "checked_in", "label": "Check In"},
      ...
    ]
  }
}
```

The UI renders this as a flowchart with current state highlighted and clickable edges.

---

## 23. Deep Dive — Slot Generation Algorithm

> The slot generator is called on every doctor schedule save and on demand. It must be fast, correct across DST, and respect all rules (working hours, breaks, holidays, slot duration, buffer, max patients per slot).

### 23.1 The algorithm

```php
namespace App\Modules\Appointments\Services;

use Carbon\CarbonImmutable;
use Carbon\CarbonPeriod;
use RRule\RRule;

class SlotGenerator {
    public function __construct(
        private readonly DoctorScheduleRepository $schedules,
        private readonly HolidayRepository $holidays,
        private readonly ExistingAppointmentRepository $appointments,
        private readonly SlotConfig $config,
    ) {}

    /**
     * Generate slots for a doctor for the next N days.
     *
     * @return Collection<TimeSlot>
     */
    public function generate(Doctor $doctor, int $days = 60): Collection {
        $start = now()->startOfDay();
        $end = $start->addDays($days);
        $slots = collect();

        // 1. Get all schedule templates for the doctor
        $schedules = $this->schedules->activeFor($doctor, between: [$start, $end]);

        // 2. Get holidays in range
        $holidays = $this->holidays->between($doctor->clinic_id, $start, $end);

        // 3. Get existing appointments to mark as taken
        $existing = $this->appointments->between($doctor, $start, $end);

        // 4. Iterate day by day
        $period = CarbonPeriod::create($start, $end);
        foreach ($period as $day) {
            if ($this->isHoliday($day, $holidays)) {
                continue; // skip holidays
            }

            // Find schedule template for this day of week
            $daySchedules = $schedules->where('day_of_week', $day->dayOfWeek);
            if ($daySchedules->isEmpty()) {
                continue; // doctor doesn't work this day
            }

            foreach ($daySchedules as $schedule) {
                $slots = $slots->merge(
                    $this->generateForDay($doctor, $day, $schedule, $existing)
                );
            }
        }

        return $slots;
    }

    private function generateForDay(Doctor $doctor, CarbonImmutable $day, DoctorSchedule $schedule, Collection $existing): Collection {
        $slots = collect();
        $tz = $doctor->timezone ?? 'UTC';

        // Schedule start/end in doctor's timezone
        $scheduleStart = $day->setTimeFromTimeString($schedule->start_time)->setTimezone($tz);
        $scheduleEnd = $day->setTimeFromTimeString($schedule->end_time)->setTimezone($tz);

        // Apply breaks
        $breaks = $schedule->breaks ?? []; // [['start' => '13:00', 'end' => '14:00']]

        // Walk slot by slot
        $cursor = $scheduleStart;
        $duration = $schedule->slot_duration_minutes;
        $buffer = $schedule->buffer_minutes;
        $step = $duration + $buffer;
        $maxPerSlot = $schedule->max_patients_per_slot ?? 1;

        while ($cursor->addMinutes($duration) <= $scheduleEnd) {
            $slotStart = $cursor;
            $slotEnd = $cursor->addMinutes($duration);

            // Skip if in a break
            if ($this->isInBreak($slotStart, $slotEnd, $breaks)) {
                $cursor = $cursor->addMinutes($step);
                continue;
            }

            // Skip if in the past
            if ($slotStart <= now()) {
                $cursor = $cursor->addMinutes($step);
                continue;
            }

            // Count existing appointments in this slot
            $taken = $existing->filter(fn($a) =>
                $a->start_at >= $slotStart && $a->start_at < $slotEnd
            )->count();

            $slots->push(new TimeSlot(
                doctor_id: $doctor->id,
                start_at: $slotStart,
                end_at: $slotEnd,
                timezone: $tz,
                capacity: $maxPerSlot,
                booked: $taken,
                is_available: $taken < $maxPerSlot,
                price: $doctor->consultation_fee,
            ));

            $cursor = $cursor->addMinutes($step);
        }

        return $slots;
    }

    private function isInBreak(CarbonImmutable $slotStart, CarbonImmutable $slotEnd, array $breaks): bool {
        foreach ($breaks as $break) {
            $breakStart = $slotStart->setTimeFromTimeString($break['start']);
            $breakEnd = $slotStart->setTimeFromTimeString($break['end']);
            if ($slotStart < $breakEnd && $slotEnd > $breakStart) {
                return true;
            }
        }
        return false;
    }

    private function isHoliday(CarbonImmutable $day, Collection $holidays): bool {
        return $holidays->contains(fn($h) => $h->date->isSameDay($day));
    }
}
```

### 23.2 DST handling

```php
// Important: never use naive DateTime, always CarbonImmutable with timezone.
// Carbon handles DST transitions automatically.

$day = CarbonImmutable::parse('2026-03-08', 'America/New_York'); // DST starts Mar 8, 2 AM
$scheduleStart = $day->setTimeFromTimeString('09:00');
// On DST day, 9 AM is correctly adjusted to the new offset.

// When serializing to DB, always convert to UTC.
$utcTimestamp = $scheduleStart->utc()->toDateTimeString();
```

### 23.3 Caching strategy

```php
// Cache key per doctor per day
$cacheKey = "slots:doctor:{$doctor->id}:day:{$day->toDateString()}";
Cache::remember($cacheKey, now()->addHours(6), function () use ($doctor, $day) {
    return $this->generate($doctor, days: 1)->filter(fn($s) => $s->start_at->isSameDay($day));
});

// Invalidate on:
// - Schedule change (model observer)
// - Appointment create/update/cancel
// - Holiday change
// - Manual flush (admin button)
```

### 23.4 Performance: 60 days × 5 doctors × 30 slots = 9,000 slots

- Generation: ~50ms (in-memory, no DB hits except for existing appointments).
- Cache hit: ~1ms.
- Cache miss: ~50ms.
- Memory: ~1.5MB for 10k slots.

### 23.5 Recurring expansion

```php
class RecurringAppointmentExpander {
    public function expand(AppointmentSeries $series, CarbonImmutable $from, CarbonImmutable $to): Collection {
        $rrule = new RRule([
            'FREQ' => $series->frequency,                // 'DAILY' | 'WEEKLY' | 'MONTHLY'
            'INTERVAL' => $series->interval ?? 1,
            'COUNT' => $series->max_occurrences,
            'UNTIL' => $series->series_end_date,
            'BYDAY' => $series->by_day,                  // ['MO', 'WE', 'FR']
            'EXDATE' => $series->exdates,                // exceptions
        ]);
        $occurrences = $rrule->getOccurrencesBetween($from, $to);
        
        // Materialize as Appointment rows, idempotent
        return $occurrences->map(fn($occ) => 
            Appointment::firstOrCreate(
                ['parent_appointment_id' => $series->id, 'occurrence_date' => $occ],
                [...series->template_attributes, 'start_at' => $this->composeStartAt($series, $occ)]
            )
        );
    }
}
```

### 23.6 Edge cases tested

| Case | Expected | Test |
|---|---|---|
| DST forward (spring) | Slot at 2:30 AM doesn't exist; generator skips | ✓ |
| DST backward (fall) | Slot at 1:30 AM occurs twice; pick first | ✓ |
| Schedule crosses midnight | Slots after midnight are on next day | ✓ |
| Holiday in middle of week | Skip just that day | ✓ |
| Schedule edited mid-period | Future slots regenerated | ✓ |
| Doctor on leave (one-off) | Slots for that day removed | ✓ |
| Buffer time negative | Clamped to 0 | ✓ |
| Slot duration 0 | Generator throws | ✓ |
| Existing appointment cancelled | Slot becomes available again | ✓ |
| Concurrent generation | Cache key collisions handled | ✓ |

---

## 24. Deep Dive — Recurring Appointment Implementation

> V1.1 introduces recurring appointments. The hardest part is idempotent expansion: regenerating 100s of occurrences must not create duplicates.

### 24.1 The schema

```php
Schema::create('appointment_series', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('doctor_id')->index();
    $t->ulid('clinic_id')->nullable();
    $t->string('rrule', 1024);
    $t->dateTime('start_at');
    $t->integer('duration_minutes');
    $t->string('timezone', 32);
    $t->date('series_start_date');
    $t->date('series_end_date')->nullable();
    $t->integer('max_occurrences')->nullable();
    $t->integer('generated_count')->default(0);
    $t->dateTime('last_generated_until')->nullable();
    $t->string('status', 16)->default('active');
    $t->json('template_attributes')->nullable();           // service_id, reason, amount, etc.
    $t->timestamps();
});

Schema::table('appointments', function (Blueprint $t) {
    $t->ulid('parent_appointment_id')->nullable()->index();
    $t->date('occurrence_date')->nullable()->index();
    $t->unique(['parent_appointment_id', 'occurrence_date'], 'appt_series_occurrence_unique');
});
```

### 24.2 The expander

```php
class RecurringAppointmentExpander {
    public function expand(AppointmentSeries $series, ?CarbonImmutable $through = null): Collection {
        $through ??= now()->addDays(60);
        $rrule = RRule::createFromString($series->rrule);

        // Compute occurrences in window [last_generated_until + 1, through]
        $windowStart = $series->last_generated_until 
            ? $series->last_generated_until->addSecond() 
            : $series->series_start_date->startOfDay();
        $occurrences = $rrule->getOccurrencesBetween($windowStart, $through);

        $created = collect();
        DB::transaction(function () use ($series, $occurrences, &$created) {
            foreach ($occurrences as $occDate) {
                $start = $this->composeStartAt($series, $occDate);
                $end = $start->addMinutes($series->duration_minutes);

                $appointment = Appointment::firstOrCreate(
                    [
                        'parent_appointment_id' => $series->id,
                        'occurrence_date' => $occDate->toDateString(),
                    ],
                    [
                        'id' => Str::ulid(),
                        'tenant_id' => $series->tenant_id,
                        'patient_id' => $series->patient_id,
                        'doctor_id' => $series->doctor_id,
                        'clinic_id' => $series->clinic_id,
                        'start_at' => $start,
                        'end_at' => $end,
                        'timezone' => $series->timezone,
                        'status' => AppointmentStatus::Pending,
                        'type' => 'in_person',
                        ...$series->template_attributes,
                    ]
                );
                if ($appointment->wasRecentlyCreated) {
                    $created->push($appointment);
                }
            }
            $series->update([
                'last_generated_until' => $through,
                'generated_count' => $series->appointments()->count(),
            ]);
        });

        return $created;
    }
}
```

### 24.3 Edge cases

- **EXDATE handling:** if patient cancels one occurrence, the series adds it to `exdates` and the expander never re-creates it.
- **RDATE handling:** if doctor adds an extra occurrence, the series adds it to `rdates` and the expander creates it.
- **Patient reschedules one occurrence:** new appointment is not part of the series; the original is in `exdates`.
- **Series paused:** no new occurrences generated; existing stay.
- **Series resumed:** expander picks up from `last_generated_until`.
- **Series end date passed:** status → 'completed'; no more expansions.

### 24.4 Performance

- Expansion of 1 year weekly (52 occurrences) takes ~200ms.
- For 1000 series (52k occurrences): ~3 minutes in a queued job.
- Use `queue:work` with batched jobs to avoid memory.

#### 24.5 Full deep-dive — `recurring-appointments` (V1.1, MML 4)

**1. Overview.** Many clinical workflows are inherently **recurring**: a chronic-disease patient sees the same doctor every 4 weeks for a diabetes checkup; a prenatal patient has a 12-visit schedule across 9 months; a physio patient comes 3x a week for 6 weeks; a therapy patient has a weekly session. V1.1 ships a first-class recurring appointment module that creates the **series** once and **expands** it into individual appointments on a rolling 60-day window. MML 4 GA covers: RFC 5545 RRULE, exceptions (EXDATE) and additions (RDATE), timezone correctness, per-occurrence reschedule / cancel / no-show without breaking the series, idempotent expansion, expander lag detection, and the patient + doctor UI for managing a series. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.1 lane module. The expander pattern is reused by V2 modules (recurring prescriptions per `08-Rx §16.4` AI Scribe, recurring lab orders per `08-Rx §16.3` Pathology Lab).

**2. RFC 5545 RRULE.** The recurrence rule is stored as a string in `appointment_series.rrule`. Example: `FREQ=WEEKLY;BYDAY=MO,WE,FR;COUNT=24` means every Mon/Wed/Fri for 24 occurrences. The library is `rlanvin/php-rrule` (or `sabre/vobject`). The expander uses `RRule::getOccurrencesBetween($start, $end)` to compute the occurrences in a window. Supported FREQs: `DAILY`, `WEEKLY`, `MONTHLY`, `YEARLY`; supported BYxxx: `BYDAY` (e.g., `MO,WE,FR`), `BYMONTHDAY` (e.g., `15`), `BYMONTH` (e.g., `3,6,9,12`), `BYHOUR` / `BYMINUTE` (rare); supported modifiers: `INTERVAL` (every N), `COUNT` (max N), `UNTIL` (end date). DST handling: `BYDAY=MO` in a `WEEKLY` rule respects the timezone; if the rule says "every Monday 10:00 AM" in `America/New_York`, the 10:00 AM is preserved across DST transitions (the wall-clock time is preserved; the UTC offset shifts).

**3. Domain Model (extended).** (Builds on the existing §24.1 schema.) Adds:
```php
Schema::create('appointment_series_exdates', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('appointment_series_id')->index();
    $t->dateTime('original_start_at');
    $t->ulid('replaced_by_appointment_id')->nullable()->index();    // if rescheduled, the new appt
    $t->string('reason', 32);                                        // 'cancelled' | 'rescheduled' | 'doctor_unavailable'
    $t->timestamps();
});

Schema::create('appointment_series_rdates', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('appointment_series_id')->index();
    $t->dateTime('start_at');
    $t->integer('duration_minutes');
    $t->json('template_attributes')->nullable();                     // overrides for this one occurrence
    $t->timestamps();
});

Schema::create('appointment_series_audit', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('appointment_series_id')->index();
    $t->string('action', 32);                                         // 'created' | 'paused' | 'resumed' | 'completed' | 'cancelled' | 'occurrence_cancelled' | 'occurrence_rescheduled' | 'expanded'
    $t->jsonb('payload');
    $t->ulid('actor_user_id')->nullable();
    $t->string('actor_type', 16)->nullable();                        // 'user' | 'system' | 'job'
    $t->timestamp('occurred_at');
    $t->timestamps();
});
```

**4. The expander algorithm.** The expander is run on three triggers: (a) **on create** of a new series (initial expansion 60 days ahead); (b) **on read** of a doctor's schedule (lazy expansion if `now() + 60d > last_generated_until`); (c) **nightly cron** (`ExpandRecurringAppointmentsJob`) for all active series (expansion to `now() + 90d`). The algorithm:
1. Compute the window: `[$last_generated_until + 1s, $now + 90d]` (or `$now + 60d` for the lazy trigger).
2. Call `$rrule->getOccurrencesBetween($start, $end)` to get the list of occurrence datetimes.
3. For each occurrence datetime, **compose the start_at** by combining the date with the series's time-of-day (e.g., the series is "every Monday 10:00 AM" → each occurrence's date is Monday; start_at is that Monday at 10:00 AM in the series's timezone).
4. For each composed start_at, run `Appointment::firstOrCreate(['parent_appointment_id' => $series->id, 'occurrence_date' => $date.toDateString()], [...full appointment...])`. The unique key is `(parent_appointment_id, occurrence_date)`, so re-expansion is **idempotent**.
5. **Skip EXDATEs** (cancelled occurrences) and **add RDATEs** (extra occurrences) — the rrule library handles EXDATE/RDATE natively when given the series's exdate list.
6. Update `series.last_generated_until = max(occurrence_start_at)`.
7. Write an `appointment_series_audit` row with `action='expanded'`, `payload={occurrences_created: N, occurrences_skipped_exdate: M, window_start, window_end}`.

**5. Timezone correctness.** A patient in `Asia/Kolkata` books "every Monday 10:00 AM" with a doctor in `America/New_York`. The series is stored in **the patient's timezone** (the source of the wall-clock time); the doctor's schedule is rendered in **the doctor's timezone** (the doctor's working hours). The expander produces start_at in UTC (the canonical form); the API renders in the patient's timezone for the patient app, in the doctor's timezone for the doctor app, and in the clinic's timezone for the admin. DST: the rrule library handles DST transitions; the wall-clock time is preserved (e.g., "10:00 AM" in `America/New_York` becomes "10:00 AM EDT" in summer and "10:00 AM EST" in winter; the UTC offset shifts).

**6. The 4 per-occurrence operations.**
- **Cancel one occurrence:** the patient or doctor cancels a single occurrence; the API writes an `appointment_series_exdates` row with `original_start_at=<that occurrence>`, `reason='cancelled'`; the original `appointments` row's status is set to `'cancelled'`. The series is otherwise unchanged; future occurrences are unaffected. The expander skips the EXDATE on the next expansion.
- **Reschedule one occurrence:** the patient or doctor reschedules a single occurrence to a different time; the API writes an `appointment_series_exdates` row with `original_start_at=<that occurrence>`, `replaced_by_appointment_id=<new appointment>`, `reason='rescheduled'`. The original is `'cancelled'`. The new appointment is a **standalone** appointment (not part of the series).
- **Mark no-show:** the doctor marks one occurrence as no-show; the `appointments.status='no_show'`. The series is unchanged; the patient can still attend future occurrences.
- **Skip a date:** the patient says "skip the next one, but keep the rest"; same as cancel one occurrence.

**7. The 4 series-level operations.**
- **Pause:** the series status is set to `'paused'`; the expander stops generating new occurrences; existing occurrences remain. The patient can resume later.
- **Resume:** the series status is set to `'active'`; the expander picks up from `last_generated_until`.
- **Complete:** the series end date has passed (or the max count is reached); the series status is set to `'completed'`; no more expansions. Existing occurrences remain.
- **Cancel:** the series status is set to `'cancelled'`; **all pending occurrences are cancelled in a single batch** (with a confirmation prompt to the actor); the audit log shows the cancellation with a list of cancelled appointment IDs.

**8. API Surface.**
```
# Series (patient / doctor)
POST   /api/v1.1/appointments/series                          # { doctor_id, clinic_id, rrule, start_at, duration_minutes, template_attributes, ... } -> series
GET    /api/v1.1/appointments/series                          -> [series] (own series)
GET    /api/v1.1/appointments/series/{id}                     -> series (with occurrences, exdates, rdates, audit)
PATCH  /api/v1.1/appointments/series/{id}                     # { rrule?, start_at?, duration_minutes?, template_attributes?, end_date? } -> series
POST   /api/v1.1/appointments/series/{id}/pause
POST   /api/v1.1/appointments/series/{id}/resume
POST   /api/v1.1/appointments/series/{id}/cancel              # confirms via body { confirm: true }
POST   /api/v1.1/appointments/series/{id}/add-occurrence      # { start_at, duration_minutes?, template_attributes? } -> adds RDATE
GET    /api/v1.1/appointments/series/{id}/occurrences         # [appointment] (the expanded appointments)
GET    /api/v1.1/appointments/series/{id}/audit               # [audit]

# Per-occurrence
POST   /api/v1.1/appointments/{id}/cancel                     # adds EXDATE, cancels this appt
POST   /api/v1.1/appointments/{id}/reschedule                 # { new_start_at, new_doctor_id? } -> adds EXDATE, creates new appt
POST   /api/v1.1/appointments/{id}/no-show                    # doctor only

# Admin
GET    /api/v1.1/admin/appointments/series                    -> [series] (filter by status, doctor, patient)
POST   /api/v1.1/admin/appointments/series/{id}/force-expand  # triggers immediate expansion
GET    /api/v1.1/admin/appointments/series/lag                # { series_id, lag_seconds, last_generated_until, now } for series with lag > 1h
```

**9. Frontend.**
- **Patient:** `/appointments/series` lists the patient's series with the next 3 occurrences; clicking a series opens a detail view with a calendar (next 60 days), the RRULE in human form ("Every Monday, Wednesday, Friday at 10:00 AM for 24 visits"), the pause / resume / cancel buttons, and a "Skip next" button.
- **Doctor:** the doctor's calendar shows the expanded occurrences; a small "recurring" badge indicates series-sourced appointments; clicking one opens a side panel with "Cancel this visit" / "Reschedule this visit" / "Mark no-show" buttons; a "Manage series" link opens the full series detail.
- **Booking flow:** the patient picks a doctor, picks a date, then sees a "Make this recurring" toggle; if on, a 3-step wizard appears: frequency (daily / weekly / monthly), days of week (for weekly), end (count or date), time of day. The preview shows the next 5 occurrences with the cost; the patient confirms and books.

**10. Realtime.**
- `appointment_series.expanded.{tenant_id}.{series_id}` — expander ran; the new occurrences are created.
- `appointment_series.occurrence.cancelled.{tenant_id}.{series_id}.{occurrence_date}` — single occurrence cancelled.
- `appointment_series.paused.{tenant_id}.{series_id}` — series paused.
- `appointment_series.resumed.{tenant_id}.{series_id}` — series resumed.
- `appointment_series.completed.{tenant_id}.{series_id}` — series completed (end date reached or count reached).
- `appointment_series.lag_detected.{tenant_id}.{series_id}` — expander lag > 1h; admin paged.

**11. Security & RBAC.**
- **Idempotency:** the unique key `(parent_appointment_id, occurrence_date)` ensures the expander cannot create duplicates; the `firstOrCreate` is wrapped in a transaction with a row-level lock.
- **Tenant scoping:** every series is scoped to `tenant_id`; cross-tenant access returns 404.
- **Patient scoping:** a patient can only see / modify their own series; a doctor can see / modify series for their own appointments; a tenant_admin can see all.
- **Cancel requires reason:** a single-occurrence cancel requires a reason (`'patient_unavailable' | 'doctor_unavailable' | 'emergency' | 'other'`); the reason is stored in the EXDATE and the audit log.
- **Bulk cancel requires co-sign:** cancelling a series with > 10 pending occurrences requires co-sign per `20-Security §11.4` (the action affects many patients / doctors).
- **Audit:** every action (create, pause, resume, cancel, occurrence-cancel, occurrence-reschedule, expand) is logged in `appointment_series_audit` and mirrored to `audit_logs`.
- **No-show requires doctor role:** a no-show is set by the doctor; a patient cannot mark themselves as no-show.

**12. Acceptance Criteria.**
- [ ] A series can be created with an RRULE; the initial 60-day window is expanded; the next 5 occurrences are visible in the UI.
- [ ] The expander is idempotent: re-running the expander for the same window does not create duplicates (verified by the unique key).
- [ ] The expander is timezone-correct: a "every Monday 10:00 AM Asia/Kolkata" series produces occurrences at the correct UTC time across DST transitions.
- [ ] A single occurrence can be cancelled without affecting the rest of the series; the EXDATE is added; the expander skips it on the next run.
- [ ] A single occurrence can be rescheduled; the original is cancelled with EXDATE; the new is a standalone appointment.
- [ ] A series can be paused; new occurrences are not generated; existing remain.
- [ ] A series can be resumed; the expander picks up from `last_generated_until`.
- [ ] A series completes when the end date is reached or the count is reached; no more expansions.
- [ ] A series can be cancelled; all pending occurrences are cancelled in a batch; the audit log shows the cancellation.
- [ ] A RDATE (extra occurrence) can be added; the expander creates it on the next run.
- [ ] The expander lag is monitored; a lag > 1h opens a P2.
- [ ] The bulk cancel co-sign is enforced for series with > 10 pending occurrences.

**13. Real-World Test Scenarios.**
- *Diabetes checkup:* patient books "every 4 weeks on Monday 10:00 AM with Dr. Kumar" for 12 occurrences; the series is created; the next 12 Mondays are populated; the patient receives reminders 24h before each.
- *Prenatal schedule:* patient books a 12-visit prenatal schedule; the RRULE is `FREQ=MONTHLY;COUNT=12;BYMONTHDAY=15` (the 15th of each month); 12 appointments are created; the patient can skip a month (cancel one occurrence) and the rest stay.
- *Reschedule one occurrence:* patient has a series; one occurrence is on a holiday; the patient reschedules that one to the next day; the original is in EXDATE; the new is a standalone appointment; the rest of the series is unchanged.
- *DST transition:* a US patient books "every Monday 10:00 AM America/New_York"; a DST transition happens in March; the 10:00 AM is preserved (the UTC offset shifts from -5 to -4); the occurrence is at 14:00 UTC instead of 15:00 UTC; the doctor's schedule renders correctly.
- *Pause + resume:* patient is on vacation for 2 months; pauses the series; no new occurrences are generated; the existing ones remain; resumes after vacation; new occurrences are generated from the resume date.
- *Bulk cancel:* doctor is leaving the clinic; cancels a series with 24 pending occurrences; the API requires co-sign; the co-sign is provided; all 24 are cancelled; the audit log shows the action.
- *Expander lag:* the nightly cron is delayed by 2 hours; the admin sees a `appointment_series.lag_detected` alert; the admin triggers a manual `force-expand`; the lag is resolved.
- *Idempotency check:* the expander runs twice for the same window (e.g., a job retry); the second run creates 0 new appointments (verified by the unique key + assertion on the count).

**14. What this module is NOT:**
- Not a calendar sync. We do not push the series to Google Calendar / iCal directly; the patient can subscribe to the `medicare.app/ical/{patient_id}/{token}` URL for a read-only iCal feed (V1, per `12-Engagement`).
- Not a scheduling optimizer. The series is rigid; it does not auto-shift around conflicts. The expander respects conflicts: if an occurrence would conflict with an existing appointment, the expander **skips** it and adds an EXDATE (the patient is notified).
- Not a group series. A series is for a single patient; group appointments are a separate module (`group-appointments`, see Turn 50).
- Not a subscription. The series is a schedule definition; payments are per-occurrence (or per the patient's plan if they have a package). Recurring billing is a separate module (`razorpay-subscriptions` per `10-Payments §18.1.1`).

**15. Dev Workflow.** `php artisan recurring:create --patient=... --doctor=... --rrule='FREQ=WEEKLY;BYDAY=MO;COUNT=12'` creates a test series. `php artisan recurring:expand --series=...` expands immediately. `php artisan recurring:expire` marks completed series. `php artisan recurring:lag-report` shows series with lag > 1h. Code: `app-modules/recurring-appointments/`; tests: `app-modules/recurring-appointments/tests/Feature/{Create,Expand,Idempotency,Timezone,EXDATE,RDATE,Pause,Resume,Cancel,Reschedule,BulkCancel}Test.php` and `tests/Performance/expander-lag.k6.js`.

---

## 25. Deep Dive — Telemedicine Video Flow

> V2 telemedicine uses LiveKit Cloud. Here's the full implementation.

### 25.1 Provisioning a room

When an appointment is confirmed with `type='telemedicine'`:

```php
class VideoRoomService {
    public function provision(Appointment $appointment): VideoRoom {
        $roomName = "appt-{$appointment->id}";
        
        $livekitRoom = $this->livekit->createRoom($roomName, [
            'emptyTimeout' => 600,        // 10 min
            'maxParticipants' => 4,        // patient, doctor, +2 observers
            'metadata' => json_encode([
                'appointment_id' => $appointment->id,
                'tenant_id' => $appointment->tenant_id,
            ]),
        ]);

        $videoRoom = VideoRoom::create([
            'tenant_id' => $appointment->tenant_id,
            'appointment_id' => $appointment->id,
            'provider' => 'livekit',
            'room_sid' => $livekitRoom->sid,
            'recording_enabled' => $appointment->doctor->video_recording_enabled ?? false,
        ]);

        $appointment->update(['video_room_id' => $videoRoom->id]);
        return $videoRoom;
    }

    public function issueToken(VideoRoom $room, User $user, string $role): string {
        $token = new AccessToken(
            config('services.livekit.api_key'),
            config('services.livekit.api_secret'),
        );
        $token->setIdentity($user->id);
        $token->setName($user->name);
        $token->setMetadata(json_encode([
            'user_id' => $user->id,
            'tenant_id' => $user->tenant_id,
            'role' => $role,
        ]));
        $token->addGrant(new RoomJoinGrant());
        $token->addGrant(new RoomGrant($room->room_sid, canPublish: $role === 'doctor' || $role === 'patient', canSubscribe: true, canPublishData: true));
        if ($role === 'doctor') {
            $token->addGrant(new RoomAdminGrant(true, true, true));  // can manage recordings
        }
        return $token->toJwt();
    }
}
```

### 25.2 Client integration (Flutter)

```dart
class TelemedicineService {
  final LiveKitClient _client = LiveKitClient();
  String? _token;
  EventsListener<RoomEvent>? _listener;

  Future<void> joinRoom(VideoRoom room, User user, String role) async {
    _token = await api.issueVideoToken(room.id, user.id, role: role);
    
    _listener = _client.createListener();
    _listener?.on<RoomConnected>((e) => print('Connected'));
    _listener?.on<ParticipantConnected>((e) => print('${e.participant.identity} joined'));
    _listener?.on<TrackSubscribed>((e) => _onTrackSubscribed(e));
    _listener?.on<DataReceived>((e) => _onDataReceived(e));
    
    await _client.connect(
      config.livekitUrl,
      _token!,
      roomOptions: RoomOptions(
        adaptiveStream: true,
        dynacast: true,
        simulcast: true,
      ),
    );
    
    await _client.enableCamera();
    await _client.enableMic();
  }

  Future<void> endCall() async {
    await _client.disconnect();
    _listener?.dispose();
  }
}
```

### 25.3 Recording

If `recording_enabled`, the doctor (admin role) can start/stop:

```php
class VideoRecordingService {
    public function startRecording(VideoRoom $room): void {
        $this->livekit->startRecording($room->room_sid, [
            'output' => [
                's3' => [
                    'bucket' => config('services.livekit.s3_bucket'),
                    'region' => config('services.livekit.s3_region'),
                    'accessKey' => config('services.aws.key'),
                    'secret' => config('services.aws.secret'),
                ],
            ],
        ]);
    }

    public function onRecordingComplete(string $roomSid, string $s3Key): void {
        $room = VideoRoom::where('room_sid', $roomSid)->firstOrFail();
        $room->update(['recording_s3_key' => $s3Key]);
        broadcast(new VideoRecordingReady($room));
    }
}
```

### 25.4 Security

- LiveKit tokens are JWTs signed with our API secret.
- Token validity: 1 hour; auto-refreshed on the client.
- TURN credentials are HMAC-signed with our app key, short-lived (1h).
- Recording is encrypted at rest in S3 with SSE-KMS.
- WebRTC media is DTLS-SRTP encrypted end-to-end.
- No raw media hits our servers (only LiveKit cluster).

### 25.5 Failure modes

| Failure | Recovery |
|---|---|
| LiveKit cluster down | Fallback to Daily.co; if also down, audio-only via Twilio |
| Patient's network drops | Auto-reconnect 3x; on fail, audio-only |
| Doctor's mic fails | Prompt to retry; in-call UI shows audio level meter |
| Recording fails | Silent; call continues; show "Recording unavailable" |
| Token expired | Auto-refresh on reconnect; if expired, re-issue via API |

---

## 26. Deep Dive — Concurrency & Race Conditions

> The appointment module is **the** most concurrency-sensitive part of the platform. Here are the races we explicitly handle.

### 26.1 The double-booking race

Two patients trying to book the same slot at the same time. Solution: DB-level lock with `lockForUpdate()` inside a transaction.

```php
public function book(BookAppointmentRequest $req): Appointment {
    return DB::transaction(function () use ($req) {
        // 1. Hold the slot
        $slot = SlotHold::where('idempotency_key', $req->idempotency_key)->first();
        if (!$slot) {
            $slot = SlotHold::create([
                'idempotency_key' => $req->idempotency_key,
                'doctor_id' => $req->doctor_id,
                'start_at' => $req->start_at,
                'expires_at' => now()->addMinutes(10),
            ]);
        }
        if ($slot->expires_at < now()) {
            $slot->update(['expires_at' => now()->addMinutes(10)]);
        }

        // 2. Lock the doctor + slot
        $existing = Appointment::where('doctor_id', $req->doctor_id)
            ->where('start_at', $req->start_at)
            ->whereIn('status', ['pending', 'confirmed', 'checked_in', 'in_consultation'])
            ->lockForUpdate()
            ->first();

        if ($existing) {
            throw new SlotUnavailableException("Slot is already booked");
        }

        // 3. Create the appointment
        $appointment = Appointment::create([...]);

        // 4. Release the hold
        $slot->delete();

        return $appointment;
    });
}
```

### 26.2 The webhook vs sync race

Webhook arrives at the same time as the gateway sync job. Solution: UNIQUE index on `transactions.gateway_transaction_id`; second insert fails.

```php
DB::table('transactions')->insertOrIgnore([
    'gateway' => 'razorpay',
    'gateway_transaction_id' => $payload['payment_id'],
    'invoice_id' => $invoice->id,
    'amount' => $payload['amount'],
    'status' => 'paid',
]);
```

### 26.3 The refund race

Patient cancels while refund is being processed. Solution: `RefundService` checks `invoice.status` before processing; if already refunded, no-op.

### 26.4 The state transition race

Two requests trying to confirm the same appointment. Solution: `lockForUpdate()` in the transition method.

### 26.5 The queue token race

Three patients check in at the same time. Solution: `SELECT MAX(queue_token) ... FOR UPDATE`.

### 26.6 The recurring expansion race

Cron job and manual trigger expand at the same time. Solution: `firstOrCreate` with unique key on `(parent_appointment_id, occurrence_date)`.

### 26.7 Test infrastructure for concurrency

- **Parallel test runner:** Pest parallel mode (8 processes).
- **Threading in tests:** `paratest` or `pestphp/pest-parallel`.
- **Database isolation:** each test uses a transaction; rolled back at end.
- **Load testing:** k6 with 1000 concurrent users.

---

## 27. Precise Plan — Module Roadmap

> The full plan for the Appointments module from V1 through V5, with phases, dependencies, resources, risks, and metrics.

### 27.1 Phase breakdown

#### Phase 1: V1 GA (Month 0)
- 9-state FSM
- Slot generation
- Booking, payment, cancel, refund
- QR check-in, queue tokens
- Live calling
- Audit + broadcast

**Team:** 2 BE, 1 FE, 1 Mobile, 1 PM.
**Effort:** ~6 person-months.
**Risk:** Race conditions in booking. **Mitigation:** lockForUpdate + tests.
**Exit criteria:** All V1 acceptance criteria pass; load test 1k concurrent.

#### Phase 2: V1.1 — Polishing (Month 0–2)
- Recurring, group, family
- Geo-search
- Waitlist
- Marketing vouchers
- RTL languages
- Doctor rotation

**Team:** +1 BE, +1 FE.
**Effort:** ~3 person-months.
**Risk:** Recurring expansion idempotency. **Mitigation:** unique index + firstOrCreate.
**Exit criteria:** All V1.1 acceptance criteria pass.

#### Phase 3: V1.2 — Integrations (Month 2–4)
- ABDM, FHIR read
- Subscriptions (Razorpay, Paystack)
- Stripe Terminal
- Twilio SMS
- Multi-clinic dashboard

**Team:** +1 BE.
**Effort:** ~2 person-months.
**Risk:** ABDM certification. **Mitigation:** Start M1 process early.
**Exit criteria:** ABDM sandbox passes.

#### Phase 4: V2 — Platform (Month 4–7)
- Telemedicine video
- Pathology lab integration
- AI symptom checker
- AI clinical scribe
- In-app chat
- Plugins v1

**Team:** +2 BE, +1 AI/ML, +1 DevOps.
**Effort:** ~8 person-months.
**Risk:** AI hallucination harms patients. **Mitigation:** Human-in-the-loop, red-team, disclaimers.
**Exit criteria:** SOC 2 Type I; AI red-team 100% pass.

#### Phase 5: V2.1 — Insurance & RCM (Month 7–10)
- Insurance pre-auth
- Denial management
- Dunning
- Recurring revenue
- Usage-based pricing
- AI usage dashboard

**Team:** +1 BE.
**Effort:** ~4 person-months.
**Risk:** Payer integration complexity. **Mitigation:** Start with one payer, generalize.
**Exit criteria:** 1k insurance claims processed.

#### Phase 6: V3 — Marketplace (Month 10–15)
- Plugin marketplace
- FHIR write
- DICOM viewer
- Custom report builder
- White-label mobile
- SSO OIDC
- Bed/OT/equipment scheduling

**Team:** +2 BE, +1 FE, +1 Mobile.
**Effort:** ~10 person-months.
**Risk:** Plugin sandbox security. **Mitigation:** Sidecar plugins, capability permissions.
**Exit criteria:** 50 plugin partners.

#### Phase 7: V4 — Hospital ITOps (Month 15–22)
- LIS ASTM instrument
- HIS bridge (Epic, Cerner)
- ER triage
- Nursing station
- Pharmacy
- Equipment maintenance
- Oncology, Mental health, Maternal, Pediatric, Diabetes, Cardiology specialties

**Team:** +3 BE, +1 Clinical SME, +2 QA.
**Effort:** ~15 person-months.
**Risk:** Epic/Cerner integration scope. **Mitigation:** Phase 1: read-only; Phase 2: write.
**Exit criteria:** 100 hospital customers; $1M MRR.

#### Phase 8: V5 — Population Health (Month 22+)
- De-identification
- Cohort builder
- Public health reporting
- Homomorphic encryption
- Federated learning

**Team:** +2 BE-AI, +1 Security.
**Effort:** ongoing.
**Risk:** Privacy violations. **Mitigation:** Differential privacy, audit, ethics board.
**Exit criteria:** 5 government/NGO customers.

### 27.2 Dependencies

| Phase | Depends on |
|---|---|
| V1 | Auth, Tenancy, Clinic Structure, Doctors, Patients, Billing, Payments, Notifications, Realtime (all V1) |
| V1.1 | V1, RRULE library, Mapbox |
| V1.2 | V1, FHIR (V1.1), ABDM NHA sandbox, Stripe Terminal SDK |
| V2 | V1.1, LiveKit Cloud, OpenAI/Anthropic, 1mg/Thyrocare API, WhatsApp Cloud, Twilio |
| V2.1 | V2, Change Healthcare / Vipul, Razorpay Subscriptions |
| V3 | V2, HAPI-FHIR, OHIF, Mirth Connect, JWT libraries |
| V4 | V3, ASTM libraries, Epic/Cerner sandbox |
| V5 | V4, Microsoft SEAL / Zama Concrete, federated learning frameworks |

### 27.3 Resource estimate

| Phase | FTE | Months | Effort (PM) | Cost @ $10K/PM |
|---|---|---|---|---|
| V1 | 5 | 6 | 30 | $300K |
| V1.1 | 2 | 2 | 4 | $40K |
| V1.2 | 1 | 2 | 2 | $20K |
| V2 | 5 | 3 | 15 | $150K |
| V2.1 | 1 | 3 | 3 | $30K |
| V3 | 4 | 5 | 20 | $200K |
| V4 | 6 | 7 | 42 | $420K |
| V5 | 3 | ongoing | TBD | TBD |
| **Total** | | | **~116 PM** | **$1.16M** |

### 27.4 Risks

| Risk | Phase | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| Double-booking race | V1 | Med | Critical | lockForUpdate + extensive tests |
| Payment failure cascade | V1 | Med | High | Idempotent webhooks + sync job |
| DST bug | V1 | High | Med | CarbonImmutable + tests |
| ABDM certification delay | V1.2 | High | Med | Start M1 in V1 |
| AI hallucination | V2 | Med | Critical | Human-in-the-loop, red-team |
| LiveKit outage | V2 | Med | High | Daily.co + Jitsi fallback |
| LLM cost overruns | V2 | High | Med | Caching, smaller models |
| Insurance payer rejection | V2.1 | High | Med | 1 payer first, then generalize |
| Plugin supply chain attack | V3 | Med | Critical | Sandboxed execution, capability permissions |
| Epic/Cerner scope creep | V4 | Very High | High | Phase read-only first |
| Population health privacy | V5 | Low | Critical | Differential privacy, ethics board |

### 27.5 Metrics

| Metric | V1 | V1.1 | V2 | V3 | V4 |
|---|---|---|---|---|---|
| Bookings/day | 1k | 10k | 100k | 500k | 2M |
| Concurrent users | 100 | 1k | 10k | 50k | 200k |
| p99 latency (book) | 500ms | 400ms | 300ms | 250ms | 200ms |
| No-show rate | 20% | 15% | 12% | 10% | 8% |
| Telemedicine % | 0% | 5% | 20% | 30% | 40% |
| NPS | 40 | 50 | 60 | 65 | 70 |
| Mean time to book | 90s | 60s | 45s | 30s | 20s |
| Patient retention (90d) | 50% | 60% | 70% | 75% | 80% |

### 27.6 Key dates

| Date | Milestone |
|---|---|
| **Month 0** | V1 GA |
| **Month 2** | V1.1 GA |
| **Month 4** | V1.2 GA |
| **Month 5** | ABDM M1 certified |
| **Month 6** | SOC 2 Type I |
| **Month 7** | V2 GA (telemedicine + AI + plugins) |
| **Month 9** | First 100 telemedicine consults |
| **Month 10** | V2.1 GA (insurance + RCM) |
| **Month 12** | First 1k insurance claims |
| **Month 15** | V3 GA (FHIR + DICOM + plugins marketplace) |
| **Month 18** | Break-even |
| **Month 22** | V4 GA (hospital ITOps) |
| **Month 24** | 100 hospital customers; $1M MRR |

### 27.7 Dependencies on other modules

- **Auth:** every endpoint requires auth; user roles gate access.
- **Tenancy:** every model is tenant-scoped; multi-tenant safe.
- **Billing:** every confirmed appointment creates an invoice.
- **Payments:** every invoice needs a payment method.
- **Notifications:** every state transition triggers notifications.
- **Realtime:** every state transition broadcasts.
- **Audit:** every state transition is audited.
- **Patients:** appointments reference patients.
- **Doctors:** appointments reference doctors.
- **Clinic Structure:** appointments reference clinics.

### 27.8 Open questions for the team

1. Should we expose the state machine to clients for offline-first UX?
2. Should we support "freeze" state for insurance pending?
3. Should we have separate `no_show` and `late_cancellation` states?
4. Should we support group appointments where the doctor is shared across slots?
5. Should we have a "walk-in" flow that bypasses the slot system?

### 27.9 Definition of Done (per phase)

- [ ] All code has 90%+ test coverage.
- [ ] All P1/P2 bugs from previous phase are closed.
- [ ] Performance budget met (p99 < target).
- [ ] Security scan passes (no High/Critical).
- [ ] Accessibility AA compliant.
- [ ] Documentation updated.
- [ ] Runbook updated.
- [ ] On-call trained.
- [ ] Customer-facing changelog written.
- [ ] SOC 2 controls verified (where applicable).
- [ ] A/B test results analyzed (for UX changes).
- [ ] Stakeholder demo.

---

## 28. V3+ Acceptance Criteria (Rolled Up)

- [ ] Multi-doctor appointment works with 3+ doctors.
- [ ] Second opinion marketplace: 100 active requests, 50 doctors bidding.
- [ ] PROMs: PHQ-9 auto-flags severe depression to doctor in <5 min.
- [ ] Care plans: 70%+ patient adherence over 30 days.
- [ ] Clinical trial matching: 80%+ accuracy on test set.
- [ ] Voice-first: Alexa/Google integration works.
- [ ] Wearables: 5+ providers integrated, 10k readings/day.
- [ ] Predictive no-show: 75%+ accuracy.
- [ ] All specialty modules (mental, oncology, maternal, pediatric, diabetes, cardiology) at MML 4.
- [ ] Hospital-at-home: 50+ active patients.
- [ ] ER triage board: <2 min from arrival to triage.
- [ ] OR scheduling: no double-booking; conflict detection works.
- [ ] Nursing station: 5 Rights check enforced.
- [ ] All V2 acceptance criteria still pass.
- [ ] All V1 acceptance criteria still pass.

---

> **Total additions to this doc:** ~150 KB (deep dive sections, test matrices, V3+ features, precise plan).
> **Total test count defined:** ~600+ tests across all levels.
> **Module line count:** ~1,800 lines (was 1,017).
