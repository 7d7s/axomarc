# 04 — Clinic Structure Module (Cities, Clinics, Departments, Specializations, Services)

> The taxonomy that organizes every doctor, patient, and appointment. Five small but critical lookup modules that the rest of the system depends on.

---

## 1. Why These Five?

| Module | Purpose | Example |
|---|---|---|
| **Cities** | Geographic filtering on the public site and admin reports | "Mumbai", "Delhi", "Bangalore" |
| **Clinics** | A physical/virtual location, owned by a tenant. **In multi-tenant mode = the tenant itself.** | "Apollo Hospital — Bandra", "Metro Care — Andheri" |
| **Departments** | Clinical specialty group | "Cardiology", "Pediatrics", "Dermatology" |
| **Specializations** | Academic/professional credential | "MBBS", "MD (Cardiology)", "FRCS" |
| **Services** | A billable line item the clinic provides | "General Consultation", "ECG", "Blood Test" |

These are the **filtering dimensions** of every search. A patient searching for a doctor filters by city + clinic + department + specialization + service + availability date.

---

## 2. Cities

### 2.1 Data Model

`cities` (global, NOT tenant-scoped — shared lookup across all tenants)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `name` | varchar(120) | |
| `state` | varchar(120) nullable | |
| `country` | varchar(2) | ISO code |
| `country_name` | varchar(120) | For display when not in i18n catalog |
| `latitude` | decimal(10,7) nullable | For "doctors near me" V2 |
| `longitude` | decimal(10,7) nullable | |
| `is_active` | boolean default true | |
| `created_at`, `updated_at` | timestamps | |

Indexes: `(country, name)`, `is_active`, `(latitude, longitude)` (V2 geo).

### 2.2 API

```
GET    /api/v1/public/cities?country=IN&q=mum
GET    /api/v1/admin/cities        (admin)
POST   /api/v1/admin/cities
PUT    /api/v1/admin/cities/{id}
DELETE /api/v1/admin/cities/{id}    (only if no clinics attached)
```

### 2.3 Service

`CityService`:
- `search($query, $country)` — debounced typeahead, 300ms.
- `importFromGooglePlaces()` — bulk import from Google Places API (V2 enhancement).

---

## 3. Clinics

### 3.1 Data Model

`clinics` (per-tenant; this is the tenant in multi-tenant mode)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | NULL in single-tenant mode |
| `uuid` | char(36) | |
| `name` | varchar(180) | "Apollo Hospital — Bandra" |
| `slug` | varchar(60) unique per tenant | URL slug |
| `tagline` | varchar(255) nullable | |
| `description` | text nullable | |
| `logo_path` | varchar(500) nullable | |
| `cover_image_path` | varchar(500) nullable | |
| `email` | varchar(180) | |
| `phone` | varchar(20) | |
| `whatsapp` | varchar(20) nullable | |
| `address_line1` | varchar(255) | |
| `address_line2` | varchar(255) nullable | |
| `city_id` | bigint FK | |
| `state` | varchar(120) | |
| `postal_code` | varchar(20) | |
| `country` | varchar(2) | |
| `latitude` | decimal(10,7) nullable | |
| `longitude` | decimal(10,7) nullable | |
| `timezone` | varchar(60) | IANA, e.g. `Asia/Kolkata` |
| `operating_hours` | json | `{"mon":{"open":"09:00","close":"21:00"}, ...}` |
| `is_24_7` | boolean default false | |
| `is_main` | boolean default false | Head office flag |
| `status` | enum('active','inactive') default 'active' | |
| `created_at`, `updated_at` | timestamps | |
| `deleted_at` | timestamp nullable | |

Indexes: `tenant_id`, `slug`, `city_id`, `status`.

### 3.2 API

```
GET    /api/v1/public/clinics?city_id=&q=
GET    /api/v1/public/clinics/{slug}
GET    /api/v1/admin/clinics
POST   /api/v1/admin/clinics        (admin only)
GET    /api/v1/admin/clinics/{id}
PUT    /api/v1/admin/clinics/{id}
DELETE /api/v1/admin/clinics/{id}    (soft delete; must unmap doctors first)
POST   /api/v1/admin/clinics/{id}/upload-logo
POST   /api/v1/admin/clinics/{id}/upload-cover
GET    /api/v1/admin/clinics/{id}/doctors
GET    /api/v1/admin/clinics/{id}/reports    (appointments, revenue, no-shows)
```

### 3.3 `ClinicService` Methods

- `create(array $data): Clinic` — validates, creates, assigns owner.
- `updateOperatingHours(Clinic, array $hours): void` — validates 24h format.
- `findNearby(Clinic, float $radiusKm): Collection<Clinic>` — V2.
- `stats(Clinic, Carbon $from, Carbon $to): array` — for the clinic dashboard.

### 3.4 Multi-Clinic Logic

In single-tenant mode, the tenant can still have multiple clinics. A doctor is mapped to one or more clinics. An appointment is at one clinic. Reports can be filtered by clinic.

### 3.5 Public Detail Page

`/{clinic-slug}` route on the patient web renders:
- Cover image
- Logo, name, tagline
- Address with map embed
- Operating hours
- "Book Appointment" CTA → `/{clinic-slug}/book`
- Doctors at this clinic (paginated)
- Testimonials filtered to this clinic
- Banners scoped to this clinic

---

## 4. Departments

### 4.1 Data Model

`departments` (per-tenant)

| Column | Type |
|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `name` | varchar(120) unique per tenant |
| `description` | text nullable |
| `icon_path` | varchar(500) nullable |
| `is_active` | boolean default true |
| `sort_order` | int default 0 |
| `created_at`, `updated_at` | timestamps |
| `deleted_at` | timestamp nullable |

Indexes: `(tenant_id, name)`, `is_active`.

### 4.2 API

```
GET    /api/v1/public/departments
GET    /api/v1/admin/departments
POST   /api/v1/admin/departments
GET    /api/v1/admin/departments/{id}
PUT    /api/v1/admin/departments/{id}
DELETE /api/v1/admin/departments/{id}  (only if no doctors attached)
POST   /api/v1/admin/departments/reorder   { ids: [3,1,2,5,...] }
```

### 4.3 Deletion Rule

A department cannot be hard-deleted if it has any `doctor_department` mappings or any past appointments. It can be soft-deleted via the `is_active=false` toggle, which hides it from the public site.

### 4.4 UI
- Admin: list view with drag-to-reorder, image upload, active toggle.
- Mobile: home screen grid of department icons, taps go to doctor list filtered by department.

---

## 5. Specializations

### 5.1 Data Model

`specializations` (per-tenant)

| Column | Type |
|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `name` | varchar(120) unique per tenant | "MBBS", "MD Cardiology" |
| `description` | text nullable |
| `is_active` | boolean default true |
| `sort_order` | int default 0 |
| `created_at`, `updated_at` | timestamps |
| `deleted_at` | timestamp nullable |

### 5.2 API

```
GET    /api/v1/public/specializations
GET    /api/v1/admin/specializations
POST   /api/v1/admin/specializations
GET    /api/v1/admin/specializations/{id}
PUT    /api/v1/admin/specializations/{id}
DELETE /api/v1/admin/specializations/{id}
```

### 5.3 Usage

Specializations are a **filter on doctor profile**, not a search primary. Each doctor has 1+ specializations. The patient web shows them as a "credential" badge on the doctor card.

---

## 6. Services (Billable Line Items)

### 6.1 Data Model

`services` (per-tenant)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `name` | varchar(180) | "General Consultation", "ECG" |
| `code` | varchar(40) unique per tenant nullable | Internal SKU/code |
| `description` | text nullable |
| `category` | varchar(60) nullable | "Consultation", "Procedure", "Lab", "Imaging" |
| `price` | bigint | In minor units (paisa/cents) |
| `currency_id` | bigint FK | |
| `duration_minutes` | int default 15 | Default slot length when this service is booked |
| `is_taxable` | boolean default true | |
| `is_active` | boolean default true |
| `is_online_bookable` | boolean default true | Hide from public booking if false |
| `image_path` | varchar(500) nullable |
| `sort_order` | int default 0 |
| `created_at`, `updated_at` | timestamps |
| `deleted_at` | timestamp nullable |

### 6.2 API

```
GET    /api/v1/public/services
GET    /api/v1/admin/services
POST   /api/v1/admin/services
GET    /api/v1/admin/services/{id}
PUT    /api/v1/admin/services/{id}
DELETE /api/v1/admin/services/{id}
POST   /api/v1/admin/services/bulk-import   (CSV upload)
GET    /api/v1/admin/services/{id}/usage     (count of times billed)
```

### 6.3 How Services Are Used

- **Appointment booking**: when a patient picks a doctor, they pick a service. The service's `price` becomes the default amount; the service's `duration_minutes` defaults the slot length. The doctor can override either.
- **Manual invoice generation** (V3.1+ original feature): accountant adds one or more services to an invoice.
- **Subscription plan inclusions** (V2): a plan includes N uses of "OPD Consultation" service.

### 6.4 Pricing & Currency

- `price` is stored in the clinic's default currency at the time of service creation.
- When displayed in a different currency (rare, since clinics are local), we convert at the spot rate (V2 with caching).
- `currency_id` lets a multi-currency clinic price in multiple currencies (V2 enhancement).

---

## 7. Pivot Tables (Many-to-Many)

### 7.1 `clinic_doctor`

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `clinic_id` | bigint FK |
| `doctor_id` | bigint FK |
| `is_primary` | boolean default false | Doctor's main clinic |
| `consultation_room` | varchar(60) nullable | E.g. "Room 204" |
| `created_at`, `updated_at` | timestamps |

Unique: `(clinic_id, doctor_id)`.

### 7.2 `doctor_department`

| Column | Type |
|---|---|
| `id` | bigint PK |
| `doctor_id` | bigint FK |
| `department_id` | bigint FK |
| `is_primary` | boolean default false |

### 7.3 `doctor_specialization`

| Column | Type |
|---|---|
| `id` | bigint PK |
| `doctor_id` | bigint FK |
| `specialization_id` | bigint FK |

### 7.4 `doctor_service`

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `doctor_id` | bigint FK |
| `service_id` | bigint FK |
| `override_price` | bigint nullable | Per-doctor override of service price |

### 7.5 `clinic_service`

Optional pivot for "this clinic offers these services". Defaults: all services are available at all clinics in the tenant; the pivot lets a clinic opt out of a service or override the price.

---

## 8. Public Search API (Composite Endpoint)

The most-called public endpoint. Returns doctors with eager-loaded department, specialization, services, reviews, next-available slot.

```
GET /api/v1/public/doctors
  ?city_id=2
  &clinic_id=4
  &department_id=3
  &specialization_id=7
  &service_id=12
  &q=cardio
  &min_rating=4
  &min_fee=0
  &max_fee=2000
  &available_on=2026-06-15
  &sort=-rating|fee|experience
  &page=1
  &per_page=20
```

Response:
```json
{
  "data": [
    {
      "id": 42,
      "name": "Dr. Priya Mehta",
      "avatar_url": "...",
      "departments": ["Cardiology"],
      "specializations": ["MBBS", "MD Cardiology"],
      "experience_years": 12,
      "rating": 4.7,
      "review_count": 132,
      "min_fee": 600,
      "max_fee": 1200,
      "currency": { "code": "INR", "symbol": "₹" },
      "clinics": [{ "id": 4, "name": "Apollo — Bandra", "distance_km": 2.3 }],
      "next_available_slot": "2026-06-15T10:30:00+05:30",
      "is_online_consultation": true
    }
  ],
  "meta": { "current_page": 1, "last_page": 7, "per_page": 20, "total": 134 }
}
```

This endpoint is the most performance-critical. Cached at the controller level with `Cache::remember('public.doctors.' . $cacheKey, 60)`. Invalidated when a doctor profile changes, or their schedule is updated.

---

## 9. Validation Rules (FormRequests)

```php
// Clinic
public function rules(): array {
    return [
        'name' => ['required', 'string', 'max:180'],
        'slug' => ['required', 'string', 'max:60', 'regex:/^[a-z0-9-]+$/', Rule::unique('clinics')->where('tenant_id', Tenant::id())->ignore($this->clinic)],
        'email' => ['required', 'email'],
        'phone' => ['required', 'string', 'regex:/^\+?[0-9]{7,15}$/'],
        'city_id' => ['required', 'exists:cities,id'],
        'timezone' => ['required', 'timezone:all'],
        'operating_hours' => ['nullable', 'array'],
        'operating_hours.*.open' => ['required_with:operating_hours.*.close', 'date_format:H:i'],
        'operating_hours.*.close' => ['required_with:operating_hours.*.open', 'date_format:H:i', 'after:operating_hours.*.open'],
        'logo' => ['nullable', 'image', 'max:2048'],
        'cover' => ['nullable', 'image', 'max:5120'],
    ];
}
```

---

## 10. Realtime Events

None of these modules broadcast. They are CRUD-only; changes invalidate caches (handled by model observers).

---

## 11. Audit Hooks

`Clinic`, `Department`, `Specialization`, `Service` are all in the audit list. Deletes are soft-deletes; the row remains for history.

---

## 12. Testing Strategy

| Test | Assertion |
|---|---|
| `ClinicServiceTest` | Slug unique per tenant, soft-delete, logo upload |
| `DepartmentServiceTest` | Cannot delete if doctor attached, reorder works |
| `PublicDoctorSearchTest` | All filters compose correctly, cache invalidates |
| `TenantScopeTest` | Clinic A cannot see Clinic B's departments |
| `BulkImportServiceTest` | CSV of 1000 services imports in <10s |

---

## 13. Acceptance Criteria (V1)

- [ ] Admin can create a clinic with logo, cover, hours, location.
- [ ] Patient web shows a clinic landing page with all data.
- [ ] Public doctor search supports all 9 filters and 3 sort options.
- [ ] Search responds in <300ms p95 with 10k doctors.
- [ ] Deleting a department that has doctors is blocked with a clear error.
- [ ] Reordering departments persists and re-renders.
- [ ] Multi-currency service pricing works (admin sees formatted price in clinic currency).
- [ ] CSV bulk import of services handles 1000 rows in <10 seconds.
- [ ] All changes audited.

---

## 14. V2 Roadmap — Clinic Structure

> V1 models the outpatient clinic: city → clinic → department → specialization → service. V2 adds **resources** (rooms, beds, equipment, OTs) and **chain** (multi-clinic) hierarchies.

### 14.1 Chain hierarchy (`multi-clinic-dashboard`, V1.2, MML 3)

V1: one tenant = one clinic. V2: a tenant can be a **chain** with branches.

> **Full deep-dive:** see **`03-MODULE-TENANCY.md §17.7`** for the 15-section treatment (`chain_users` / `chain_revenue_rules` / `chain_metrics` / `revenue_distributions` / `chain_reports` schemas, 3 admin roles `chain_admin` / `region_admin` / `chain_auditor`, per-clinic PII export co-sign, multi-tenant analytics dashboard). This §14.1 is the V1.2 stub; the deep-dive at 03-Tenancy is canonical.

```php
// Add to clinics table
Schema::table('clinics', function (Blueprint $t) {
    $t->string('chain_role', 16)->default('standalone');  // 'head' | 'branch' | 'standalone'
    $t->string('parent_clinic_id', 26)->nullable()->index();
    $t->json('operating_hours')->nullable();              // per-day override
});
```

`ChainAdminService::aggregatedMetrics($chainId)` returns revenue, appointments, doctor utilization across all branches in one query (uses `WITH ROLLUP` or a `chain_metrics` materialized view refreshed every 5 min).

### 14.2 Resources (`ot-room-scheduling`, `bed-management`, `equipment-booking`)

A new polymorphic `resources` table that models anything bookable: OT room, hospital bed, ultrasound machine, X-ray slot.

```php
Schema::create('resources', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('clinic_id')->index();
    $t->string('type', 32);                              // 'room' | 'bed' | 'equipment' | 'ot' | 'imaging' | 'lab_slot'
    $t->string('code', 32);                              // clinic-scoped unique code: 'OT-1', 'BED-ICU-12'
    $t->string('name', 128);
    $t->json('attributes')->nullable();                   // type-specific: { ward: 'ICU', bed_type: 'manual' }
    $t->integer('capacity')->default(1);                 // OT=1, Group Room=8, etc.
    $t->decimal('hourly_rate', 12, 2)->nullable();
    $t->boolean('is_active')->default(true)->index();
    $t->timestamps();
    $t->unique(['clinic_id', 'code']);
});
```

```php
Schema::create('resource_schedules', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('resource_id')->index();
    $t->unsignedTinyInteger('day_of_week');               // 0–6
    $t->time('start_time');
    $t->time('end_time');
    $t->integer('slot_duration_minutes')->default(30);
    $t->boolean('is_active')->default(true);
    $t->timestamps();
    $t->foreign('resource_id')->references('id')->on('resources')->cascadeOnDelete();
});
```

```php
Schema::create('resource_bookings', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('resource_id')->index();
    $t->ulid('appointment_id')->nullable()->index();
    $t->ulid('patient_id')->nullable()->index();
    $t->timestamp('start_at')->index();
    $t->timestamp('end_at')->index();
    $t->string('status', 16)->default('confirmed');       // pending | confirmed | checked_in | completed | cancelled | no_show
    $t->json('notes')->nullable();
    $t->timestamps();
    $t->foreign('resource_id')->references('id')->on('resources')->restrictOnDelete();
    $t->foreign('appointment_id')->references('id')->on('appointments')->nullOnDelete();
});
```

`ResourceBookingService` follows the same idempotent, locked, audited pattern as `AppointmentService` (V1, `07-Appointments.md`).

### 14.3 Wards & beds (`bed-management`, V3)

```php
Schema::create('wards', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('clinic_id')->index();
    $t->string('name', 64);                              // 'ICU', 'General Ward A', 'Pediatric'
    $t->string('type', 16);                              // 'general' | 'icu' | 'nicu' | 'isolation' | 'pediatric'
    $t->integer('floor')->nullable();
    $t->decimal('daily_rate', 12, 2);
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});

Schema::create('beds', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('ward_id')->index();
    $t->string('code', 32);                              // 'BED-ICU-12'
    $t->string('status', 16)->default('available');      // available | occupied | cleaning | maintenance | reserved
    $t->json('attributes')->nullable();                  // { has_oxygen, has_monitor, isolation: true }
    $t->timestamps();
    $t->unique(['ward_id', 'code']);
});

Schema::create('bed_assignments', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('bed_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('admission_id')->nullable()->index();
    $t->timestamp('assigned_at');
    $t->timestamp('discharged_at')->nullable();
    $t->text('discharge_reason')->nullable();
    $t->foreign('bed_id')->references('id')->on('beds')->restrictOnDelete();
});
```

**1. Lifecycle of a bed assignment:**
1. Patient admitted (via HL7 v2 ADT^A01 per `19-Integrations §15.10`, or via `POST /api/v3/admissions` for direct entry).
2. Housekeeping / floor manager finds a free bed in the appropriate ward (filter by `wards.type` matching clinical need, `beds.attributes` matching care requirements — e.g., isolation, oxygen).
3. `POST /api/v3/beds/assignments` with `{ bed_id, patient_id, admission_id }`. Server uses `SELECT ... FOR UPDATE` on the bed row to prevent double-booking; if `bed.status != 'available'`, returns 409.
4. On success: `bed.status = 'occupied'`, `bed_assignments.assigned_at = now()`, audit row written.
5. Discharge: `PATCH /api/v3/beds/assignments/{id}` with `discharged_at` and `discharge_reason`. Bed status flips to `cleaning` (not `available` — it must be cleaned first per infection control). A `CleaningTaskJob` is enqueued; housekeeping marks complete via `PATCH /api/v3/beds/{id}/status` with `status = 'available'`.

**2. API surface (V3 GA):**
```
GET    /api/v3/wards                                  # list wards
POST   /api/v3/wards                                  # create
PATCH  /api/v3/wards/{id}
DELETE /api/v3/wards/{id}                             # soft-delete (cannot delete a ward with active beds)
GET    /api/v3/wards/{id}/beds                        # list beds in a ward, with availability
GET    /api/v3/beds                                   # all beds (paginated, filter by ward_id/status)
GET    /api/v3/beds/{id}                              # single bed
PATCH  /api/v3/beds/{id}/status                       # change bed status (cleaning, maintenance, available)
POST   /api/v3/beds/assignments                       # assign a patient to a bed
GET    /api/v3/beds/assignments                       # list active assignments
GET    /api/v3/beds/assignments/{id}                  # single assignment
PATCH  /api/v3/beds/assignments/{id}                  # discharge
GET    /api/v3/wards/{id}/occupancy                   # live occupancy: total, available, occupied, cleaning, maintenance
GET    /api/v3/wards/{id}/occupancy-history           # daily occupancy for the last 30 days
```

**3. Realtime events:**
- `bed.status.changed.{tenant}` — broadcast whenever a bed status changes. Subscribed by the floor display (TV showing the ward map).
- `bed.assignment.created.{tenant}` and `bed.assignment.discharged.{tenant}`.
- `ward.occupancy.changed.{tenant}` — derived event (Redis counter, debounced 1/min).

**4. Security:**
- Assigning a bed requires `beds.assign` permission (granted to doctors, nurses, floor managers).
- Changing bed status to `maintenance` or `available` (from `cleaning`) requires `beds.housekeeping` permission.
- All state transitions are logged in `audit_logs` with before/after status.
- Bed attributes (oxygen, monitor) are visible to anyone with `beds.read`; patient identity on the bed is restricted to `beds.read` + same-tenant scope.

**5. Acceptance criteria (V3 GA):**
- [ ] Two simultaneous `POST /api/v3/beds/assignments` for the same bed result in exactly one 201 and one 409.
- [ ] Discharged bed transitions to `cleaning`, not directly to `available`.
- [ ] Ward occupancy count matches the sum of `beds.status` in that ward.
- [ ] HL7 v2 ADT^A01 (#96) creates a `bed_assignments` row when the segment includes `PV1.3` (assigned location).
- [ ] All bed status changes emit realtime events within 2 seconds.

**6. Real-world test scenarios:**
- *Mass casualty:* 30 patients arrive at once; the system assigns beds by ward type and patient acuity, returning a JSON list of assignments for the floor manager to confirm.
- *Isolation request:* doctor requests isolation bed for a TB patient; the system filters `beds.attributes.isolation = true` and returns only matching free beds.
- *Discharge rush:* 10 patients discharged in 5 minutes; the floor display updates the bed map in realtime; the housekeeping queue shows 10 cleaning tasks.

**7. What this module is NOT:**
- Not a full EMR. Admissions, vitals charting, and nursing notes are V4 modules (`#108 Nursing Station`) — for V3, the bed module only tracks bed occupancy, not the clinical content of the admission.
- Not a billing engine. Bed daily rate is stored but invoicing happens in `09-Billing`.

### 14.4 Operation theatres (`ot-room-scheduling`, V3)

```php
Schema::create('operation_theatres', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('clinic_id')->index();
    $t->string('code', 32);
    $t->string('name', 64);
    $t->string('specialization', 64)->nullable();        // 'cardiac', 'neuro', 'general'
    $t->json('equipment_list')->nullable();
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});

Schema::create('ot_slots', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('ot_id')->index();
    $t->timestamp('start_at')->index();
    $t->timestamp('end_at')->index();
    $t->string('status', 16)->default('open');           // open | reserved | booked | blocked
    $t->ulid('surgery_id')->nullable()->index();
    $t->foreign('ot_id')->references('id')->on('operation_theatres')->restrictOnDelete();
});
```

**1. Surgery scheduling flow:**
1. Surgeon books a surgery: `POST /api/v3/ot/surgeries` with `{ patient_id, procedure_code, surgeon_user_id, anesthesiologist_user_id, duration_minutes, required_specialization, required_equipment[] }`.
2. The system searches for an OT whose `specialization` matches and whose `equipment_list` covers the required equipment. Available OT slots in the next 7 days are returned.
3. Surgeon picks a slot: `POST /api/v3/ot/surgeries/{id}/book` with `{ ot_id, slot_id }`. Server uses `SELECT ... FOR UPDATE` on the slot; if `slot.status != 'open'`, returns 409.
4. On success: `ot_slots.status = 'booked'`, `ot_slots.surgery_id = surgery.id`, audit row written.
5. Equipment conflict: if any required equipment (`equipment_bookings` per §14.5) is double-booked at the same time, the surgery booking returns 409 with a list of conflicting equipment.
6. Pre-op checklist (V3.1 stub): the system emits a `surgery.pre_op_checklist.{surgery_id}` event 24h before; the nursing app renders the checklist (consent signed, labs reviewed, NPO confirmed, allergy reviewed).

**2. Surgery status machine:**
- `scheduled → pre_op → in_progress → post_op → completed` (with `cancelled` branch at any pre-op stage).
- Status transitions emit `surgery.status.changed.{tenant}` realtime events.

**3. Surgical notes (V3 GA):** the surgeon writes a structured note (`POST /api/v3/ot/surgeries/{id}/note`) with `procedure_performed`, `findings`, `complications`, `implants_used`, `estimated_blood_loss`, `post_op_orders`. Stored in `surgical_notes` table (added in V3 per `14-V2-Stubs §4.15`).

**4. OT utilization metric (admin dashboard):** `GET /api/v3/ot/utilization?from=...&to=...&ot_id=...` returns `{ ot_id, total_hours, booked_hours, utilization_pct, cancellations, no_shows }`. Computed nightly by `AggregateOtUtilizationJob`.

**5. API surface (V3 GA):**
```
GET    /api/v3/ot/theatres
POST   /api/v3/ot/theatres
PATCH  /api/v3/ot/theatres/{id}
GET    /api/v3/ot/theatres/{id}/slots?from=...&to=...        # calendar view
POST   /api/v3/ot/slots                                       # create open slot (admin)
PATCH  /api/v3/ot/slots/{id}                                  # block or release
GET    /api/v3/ot/surgeries                                   # list (filter by status, surgeon, date)
POST   /api/v3/ot/surgeries                                   # book a surgery
GET    /api/v3/ot/surgeries/{id}
PATCH  /api/v3/ot/surgeries/{id}/status
POST   /api/v3/ot/surgeries/{id}/book                         # assign to slot
POST   /api/v3/ot/surgeries/{id}/cancel                       # cancel (pre-op only)
POST   /api/v3/ot/surgeries/{id}/note                         # write surgical note
GET    /api/v3/ot/surgeries/{id}/note
GET    /api/v3/ot/utilization                                 # OT utilization metric
GET    /api/v3/ot/surgeries/{id}/checklist                    # V3.1 pre-op checklist
```

**6. Realtime events:**
- `ot.slot.booked.{tenant}` — broadcast when an OT slot is booked.
- `surgery.status.changed.{tenant}` — broadcast on every surgery status transition.
- `ot.theatre.status.changed.{tenant}` — broadcast when an OT goes into cleaning/maintenance/ready.

**7. Security:**
- Booking a surgery requires `ot.book` permission (surgeon).
- Cancelling a surgery requires `ot.cancel` (surgeon or floor manager).
- Writing a surgical note requires `ot.note` (operating surgeon only — verified by `surgeries.surgeon_user_id`).
- All status transitions and notes are logged in `audit_logs`.

**8. Acceptance criteria (V3 GA):**
- [ ] Two simultaneous `POST /api/v3/ot/surgeries/{id}/book` for the same slot return one 201 and one 409.
- [ ] Booking a surgery that requires equipment X returns 409 if X is double-booked.
- [ ] OT utilization metric matches a hand-count for a 7-day window.
- [ ] All surgery status transitions emit realtime events.
- [ ] A surgeon cannot write a surgical note for another surgeon's surgery (returns 403).

**9. What this module is NOT:**
- Not an anesthesia module. Anesthesia records are V4 (`#108 Nursing Station` + an anesthesia plugin).
- Not a sterile supply tracker. Instrument tracking is out of scope.
- Not a robotic surgery interface. We schedule the time slot; the robotic system is operated independently.

### 14.5 Equipment (`equipment-booking`, V3)

```php
Schema::create('equipment', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('clinic_id')->index();
    $t->string('code', 32);
    $t->string('name', 128);
    $t->string('type', 32);                              // 'imaging' | 'lab' | 'monitoring' | 'surgical'
    $t->string('model', 64)->nullable();
    $t->string('serial', 64)->nullable();
    $t->date('purchase_date')->nullable();
    $t->date('warranty_expires_at')->nullable();
    $t->date('next_pm_at')->nullable()->index();         // preventive maintenance
    $t->string('status', 16)->default('available');      // available | in_use | maintenance | retired
    $t->timestamps();
});
```

**1. Booking model:** equipment is a bookable resource (per `00-Architecture §5 Principle 4` resource pattern, mirroring `07-Appointments §19.4`). A piece of equipment has a `status` (available / in_use / maintenance / retired) and an `equipment_bookings` table (per `14-V2-Stubs §4.15`) that records the time window during which the equipment is reserved.

**2. Booking flow:**
1. Requester (radiologist, surgeon, lab tech) calls `POST /api/v3/equipment/{id}/bookings` with `{ start_at, end_at, purpose, linked_appointment_id? }`.
2. Server uses `SELECT ... FOR UPDATE` on the equipment row and checks for overlap with any existing `equipment_bookings` where `start_at < new_end_at AND end_at > new_start_at`.
3. If conflict → 409 with the conflicting booking id. If free → create `equipment_bookings` row, set `equipment.status = 'in_use'` (only if `start_at <= now()`).
4. At `end_at`, a `ReleaseEquipmentJob` flips `equipment.status` back to `available` (if no overlapping next booking exists).
5. Cancelling a booking: `DELETE /api/v3/equipment/bookings/{id}` — only the requester or an admin can cancel; cancellation must happen before `start_at`.

**3. Equipment types and their booking patterns:**
- **Imaging** (X-ray, CT, MRI, US): booked per-slot (typically 30-60 min). Imaging orders from the `imaging_orders` row auto-create a placeholder booking.
- **Lab** (analyzers, centrifuges): booked per-batch (whole-day or whole-shift).
- **Monitoring** (vital sign monitors, telemetry): assigned to a bed (links to `bed_assignments` from §14.3).
- **Surgical** (laparoscope, C-arm): booked per-surgery (links to `ot_surgeries` from §14.4).

**4. Preventive maintenance (PM):** a piece of equipment with `next_pm_at` past triggers a `maintenance_due` status. The system blocks bookings and emits a `equipment.maintenance_due.{tenant}` realtime event 7 days before the due date. The biomed engineer marks PM complete via `PATCH /api/v3/equipment/{id}` with `next_pm_at = new date`, `status = 'available'`.

**5. API surface (V3 GA):**
```
GET    /api/v3/equipment                                # paginated, filter by type/status
POST   /api/v3/equipment                                # create
PATCH  /api/v3/equipment/{id}
DELETE /api/v3/equipment/{id}                           # soft-delete (status = 'retired')
GET    /api/v3/equipment/{id}/bookings?from=...&to=...   # calendar of bookings for one equipment
POST   /api/v3/equipment/{id}/bookings                  # book
GET    /api/v3/equipment/bookings                       # list all bookings (paginated, filter by equipment_id, status)
GET    /api/v3/equipment/bookings/{id}
DELETE /api/v3/equipment/bookings/{id}                  # cancel
GET    /api/v3/equipment/{id}/availability?from=...&to=...  # list of free slots
GET    /api/v3/equipment/maintenance-due                # list of equipment with next_pm_at past
```

**6. Realtime events:**
- `equipment.status.changed.{tenant}` — broadcast on every status change.
- `equipment.booked.{tenant}` and `equipment.booking.cancelled.{tenant}`.
- `equipment.maintenance_due.{tenant}` — emitted 7 days before `next_pm_at` and again on the day.

**7. Security:**
- Booking equipment requires `equipment.book` permission.
- Cancelling a booking requires `equipment.cancel` (requester or admin).
- Marking PM complete requires `equipment.maintenance` (biomed engineer only).
- All bookings are linked to a tenant; no cross-tenant booking is possible.

**8. Acceptance criteria (V3 GA):**
- [ ] Two simultaneous bookings for the same equipment in the same time window result in one 201 and one 409.
- [ ] A booking for retired equipment returns 422.
- [ ] A booking that ends in the past returns 422.
- [ ] PM due list is correct for all equipment in the tenant (matches the `next_pm_at` index).
- [ ] An imaging order from a doctor auto-creates a placeholder booking on the matching equipment type.

**9. What this module is NOT:**
- Not a CMMS (Computerized Maintenance Management System). We track PM dates but do not manage work orders, parts inventory, or vendor contracts. (#110 Equipment Maintenance in V4 will add the CMMS layer.)
- Not a calibration tracking system. Calibration certificates are stored in `patient_files`-style attachments, not in this module.

### 14.6 Public API additions (V2)

```
GET    /api/v2/public/clinics/{id}/resources             -> [resource with availability]
GET    /api/v2/public/clinics/{id}/beds?ward={id}        -> [bed with status]
GET    /api/v2/public/clinics/{id}/ot-slots?from&to      -> [ot_slot]
GET    /api/v2/public/clinics/{id}/services             -> [service with online booking]
```

### 14.7 V2 acceptance criteria

- [ ] Chain admin sees a dashboard with all branches.
- [ ] Resources can be created, scheduled, booked, cancelled, audited.
- [ ] Concurrent resource booking never double-books (DB unique partial index).
- [ ] Wards, beds, OTs, equipment are all reachable from clinic settings UI.
- [ ] Equipment PM (preventive maintenance) cron emails the manager N days before `next_pm_at`.
- [ ] Bed assignment appears in the patient's chart and is billable.
- [ ] All resource transitions emit `audit_logs` and Reverb events.
