# 05 — Doctors Module (Profiles, Schedules, Time Slots, Reviews)

> The heart of the public experience. Everything the patient sees and books is driven by this module.

---

## 1. Scope

| In Scope | Out of Scope |
|---|---|
| Doctor profile (name, bio, fee, experience, education, social, languages) | Doctor payroll (V2) |
| Doctor-to-clinic, department, specialization, service mappings | Doctor credential verification (V2) |
| Per-day weekly schedule (working hours) | Complex rotating/exception-based schedules (V2) |
| Per-doctor time slot generation (15/20/30/60 min) | Group appointments (V2) |
| Doctor reviews & ratings (aggregate + list) | Doctor-to-doctor referrals (V2) |
| Doctor search, filter, sort (public + admin) | Geo-search "near me" (V2) |
| Doctor dashboard (today's appointments, my patients) | Doctor-to-doctor messaging (V2) |
| Doctor app: today list, start visit, complete, write Rx | |

---

## 2. The Doctor as a User

A doctor is a `User` with:
- `role = doctor`
- A linked `Doctor` profile (1-to-1)
- One or more `clinic_doctor` mappings (M-to-M)
- One or more `doctor_department` mappings
- One or more `doctor_specialization` mappings
- One or more `doctor_service` mappings (with optional per-doctor price override)
- One `doctor_schedule` per weekday (or 0 for "unavailable that day")
- Many `doctor_time_slot` rows (generated on schedule save)

The relationship is:
```
User (role=doctor) 1───1 Doctor 1───M doctor_experiences
                              │1───M doctor_educations
                              │1───M doctor_languages_spoken
                              │M───M Clinic     (via clinic_doctor)
                              │M───M Department (via doctor_department)
                              │M───M Specialization (via doctor_specialization)
                              │M───M Service   (via doctor_service)
                              │1───7 DoctorSchedule (one per weekday)
                              │1───M DoctorTimeSlot
                              │1───M DoctorReview
                              │1───M DoctorException (date overrides, V2)
```

---

## 3. Doctor Profile — Data Model

### 3.1 `doctors`

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `user_id` | bigint FK unique | |
| `uuid` | char(36) | Public ID |
| `slug` | varchar(60) unique per tenant | |
| `first_name` | varchar(60) | |
| `last_name` | varchar(60) | |
| `display_name` | varchar(120) nullable | "Dr. Priya Mehta" — falls back to first+last |
| `gender` | enum('m','f','o') | |
| `bio` | text nullable | Long-form about the doctor |
| `tagline` | varchar(255) nullable | One-liner shown in cards |
| `avatar_path` | varchar(500) nullable | |
| `cover_image_path` | varchar(500) nullable | |
| `consultation_fee` | bigint default 0 | In minor units |
| `online_consultation_fee` | bigint default 0 | Often different from in-person |
| `follow_up_fee` | bigint default 0 | |
| `currency_id` | bigint FK | |
| `experience_years` | int default 0 | Computed from experiences; can be overridden |
| `languages_spoken` | json | `["en","hi","ta"]` for quick filter |
| `registration_number` | varchar(60) nullable | Medical council registration |
| `is_verified` | boolean default false | Admin verification flag |
| `is_featured` | boolean default false | Boost in search results |
| `is_active` | boolean default true | Soft pause without losing data |
| `accepts_online_consultation` | boolean default true | |
| `accepts_in_person_consultation` | boolean default true | |
| `avg_rating` | decimal(3,2) default 0 | Cached; recomputed on review write |
| `review_count` | int default 0 | Cached |
| `total_appointments` | int default 0 | Cached; for "popular" badge |
| `metadata` | json | Extensible: awards, press mentions, etc. |
| `created_at`, `updated_at` | timestamps | |
| `deleted_at` | timestamp nullable | |

Indexes: `(tenant_id, slug)`, `user_id`, `is_active`, `(is_featured, avg_rating)`, `(city_id, is_active, avg_rating)` (for search).

### 3.2 `doctor_experiences`

| Column | Type |
|---|---|
| `id` | bigint PK |
| `doctor_id` | bigint FK |
| `title` | varchar(180) | "Senior Cardiologist" |
| `organization` | varchar(180) | "Apollo Hospital" |
| `city_id` | bigint FK nullable |
| `start_date` | date |
| `end_date` | date nullable | NULL = current |
| `is_current` | boolean default false |
| `description` | text nullable |
| `sort_order` | int default 0 |

### 3.3 `doctor_educations`

| Column | Type |
|---|---|
| `id` | bigint PK |
| `doctor_id` | bigint FK |
| `degree` | varchar(120) | "MBBS", "MD" |
| `institution` | varchar(180) |
| `year_of_completion` | smallint |
| `sort_order` | int default 0 |

### 3.4 `doctor_socials`

| Column | Type |
|---|---|
| `id` | bigint PK |
| `doctor_id` | bigint FK |
| `platform` | enum('website','facebook','twitter','instagram','linkedin','youtube','other') |
| `url` | varchar(500) |

---

## 4. Doctor Schedules

### 4.1 Data Model

`doctor_schedules` — one row per (doctor, weekday). Stores working hours and slot duration.

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `doctor_id` | bigint FK |
| `clinic_id` | bigint FK nullable | If null, schedule applies to all clinics |
| `weekday` | tinyint | 0=Sunday, 6=Saturday (ISO compatible) |
| `start_time` | time | "09:00:00" |
| `end_time` | time | "17:00:00" |
| `slot_duration_minutes` | tinyint default 15 | |
| `buffer_minutes` | tinyint default 0 | Gap between slots |
| `max_patients_per_slot` | tinyint default 1 | If >1, group consultation |
| `is_active` | boolean default true |
| `effective_from` | date nullable | V2 — schedule changes over time |
| `effective_to` | date nullable | V2 |
| `created_at`, `updated_at` | timestamps |

Unique: `(doctor_id, clinic_id, weekday)` (where `clinic_id` can be NULL).

### 4.2 Schedule Examples

**Example 1:** Dr. Priya works Mon-Sat 9 AM – 5 PM at Apollo with 20-min slots.
- 6 rows (Mon-Sat).
- `slot_duration_minutes = 20`, `buffer_minutes = 0`.
- 24 slots/day × 6 days = 144 slots/week.

**Example 2:** Dr. Raj works Mon/Thu at Apollo, Tue/Fri at Metro, both 6-9 PM with 30-min slots, online only.
- 2 rows at Apollo, 2 rows at Metro.
- `accepts_in_person_consultation` overridden at the `clinic_doctor` level via pivot boolean.

**Example 3:** Dr. Chen does online consultations all 7 days, 24/7.
- 7 rows with `start_time=00:00`, `end_time=23:59`, `slot_duration_minutes=60`, `clinic_id=null` (online has no clinic).

### 4.3 API

```
GET    /api/v1/admin/doctors/{id}/schedules
POST   /api/v1/admin/doctors/{id}/schedules   (bulk save — replaces all 7 days)
PUT    /api/v1/admin/doctors/{id}/schedules/{scheduleId}
DELETE /api/v1/admin/doctors/{id}/schedules/{scheduleId}
GET    /api/v1/public/doctors/{id}/schedule?from=2026-06-10&to=2026-06-16
```

### 4.4 `DoctorScheduleService::bulkSave($doctorId, $clinicId, array $weekdays)`

Validates no overlapping schedules, replaces all 7 day rows atomically, regenerates time slots for the next 60 days.

---

## 5. Time Slots

### 5.1 Data Model

`doctor_time_slots`

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `doctor_id` | bigint FK |
| `clinic_id` | bigint FK nullable |
| `schedule_id` | bigint FK |
| `start_at` | timestamp indexed | |
| `end_at` | timestamp |
| `status` | enum('available','held','booked','blocked') default 'available' | |
| `held_by` | bigint FK nullable | User who started checkout |
| `held_until` | timestamp nullable | Hold expires |
| `appointment_id` | bigint FK nullable | Set when status=booked |
| `slot_type` | enum('in_person','online') | |
| `created_at`, `updated_at` | timestamps |

Indexes: `(doctor_id, start_at)`, `(clinic_id, start_at)`, `status`, `(start_at, status)`.

### 5.2 Slot Generation Algorithm

When a `DoctorSchedule` is saved, `SlotGeneratorService::generate(Doctor, Carbon $from, Carbon $to)` runs:

```
for each day D in [from, to]:
  if D.weekday matches schedule.weekday:
    cursor = D + schedule.start_time
    while cursor + slot_duration <= D + schedule.end_time:
      upsert doctor_time_slots where (doctor, clinic, start_at=cursor):
        if no existing row: create
        if existing.status=blocked: keep blocked
        if existing.status=available: refresh
      cursor += slot_duration + buffer
```

Slots are pre-generated for **the next 60 days**. A nightly job (`GenerateFutureSlotsJob`) extends the window by one day (rolling window).

### 5.3 Slot Hold (Concurrency Control)

When a patient starts checkout:
```
POST /api/v1/public/doctors/{id}/slots/{slotId}/hold
```

- API sets `status='held'`, `held_by=user_id`, `held_until=now()+10min`.
- Other patients see the slot as "Held" and can't book it.
- If the patient doesn't complete checkout in 10 min, a `ReleaseExpiredHoldsJob` (runs every minute) frees the slot.

### 5.4 Race Condition Guard
- DB transaction with `SELECT ... FOR UPDATE` on the slot row.
- If status != 'available' → return 409 with `slot_unavailable` error.
- If held by another user → same.

### 5.5 Public Slot Listing

```
GET /api/v1/public/doctors/{id}/slots?date=2026-06-15&type=in_person
```

Returns grouped by date:
```json
{
  "data": [
    {
      "date": "2026-06-15",
      "slots": [
        { "id": 12345, "start_at": "2026-06-15T09:00:00+05:30", "end_at": "2026-06-15T09:20:00+05:30", "status": "available" },
        ...
      ]
    }
  ]
}
```

Only `available` and `held_by_current_user` slots are returned; `booked` and `blocked` are filtered.

---

## 6. Doctor Reviews

See [12-MODULE-ENGAGEMENT.md §Reviews](12-MODULE-ENGAGEMENT.md) for the full review module. Doctor reviews have these specifics:

### 6.1 `doctor_reviews`

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `doctor_id` | bigint FK |
| `patient_id` | bigint FK |
| `appointment_id` | bigint FK unique | One review per appointment |
| `rating` | tinyint | 1-5 stars |
| `title` | varchar(120) nullable |
| `comment` | text nullable |
| `is_anonymous` | boolean default false | Hides patient name on display |
| `is_approved` | boolean default true | Admin moderation |
| `doctor_reply` | text nullable | Doctor can reply once |
| `replied_at` | timestamp nullable |
| `created_at`, `updated_at` | timestamps |

### 6.2 `RatingAggregator`
Listens to `DoctorReview::saved/deleted`, recomputes `doctors.avg_rating` and `doctors.review_count` in the same transaction. The doctor search endpoint uses these cached values for sorting.

### 6.3 API

```
POST   /api/v1/patient/appointments/{id}/review   { rating, title, comment, is_anonymous }
PUT    /api/v1/patient/reviews/{id}
DELETE /api/v1/patient/reviews/{id}   (within 24h)
POST   /api/v1/doctor/reviews/{id}/reply   { doctor_reply }
GET    /api/v1/public/doctors/{id}/reviews?rating=5&page=1
```

---

## 7. Doctor Search (the Most Complex Public Endpoint)

### 7.1 Request

```
GET /api/v1/public/doctors
  ?q=
  &city_id=
  &clinic_id=
  &department_id=
  &specialization_id=
  &service_id=
  &language=
  &min_rating=0
  &min_fee=
  &max_fee=
  &accepts_online=true
  &accepts_in_person=true
  &available_on=2026-06-15
  &gender=
  &sort=-rating|fee|experience|popular
  &page=1&per_page=20
```

### 7.2 SQL Strategy

Uses **Eloquent query builder** with eager-loading. Joins to `doctor_time_slots` for the `available_on` filter (a doctor with at least 1 `available` slot on that date passes). Indexes:
- `doctors(tenant_id, is_active, avg_rating)` for sort.
- `doctor_time_slots(doctor_id, start_at, status)` for availability.

### 7.3 Caching

Cache key: `public:doctors:v1:{tenant}:{sha1(query)}`
TTL: 60 seconds.
Invalidated on any `Doctor::saved`, `DoctorSchedule::saved`, or `Appointment::saved` (the last one because it changes slot availability).

### 7.4 Response Shape

```json
{
  "data": [
    {
      "id": 42,
      "uuid": "8c3a...",
      "name": "Dr. Priya Mehta",
      "slug": "dr-priya-mehta",
      "avatar_url": "...",
      "tagline": "Cardiologist with 12 years experience",
      "departments": [{ "id": 3, "name": "Cardiology", "icon": "..." }],
      "specializations": ["MBBS", "MD Cardiology"],
      "experience_years": 12,
      "languages_spoken": ["en", "hi"],
      "rating": 4.7,
      "review_count": 132,
      "fees": {
        "in_person": 800,
        "online": 600,
        "follow_up": 300,
        "currency": { "code": "INR", "symbol": "₹", "minor_unit_factor": 100 }
      },
      "clinics": [
        { "id": 4, "name": "Apollo — Bandra", "city": "Mumbai", "distance_km": 2.3 }
      ],
      "next_available_slot": {
        "date": "2026-06-15",
        "start_at": "2026-06-15T10:30:00+05:30",
        "type": "in_person"
      },
      "is_online_consultation": true,
      "is_featured": false
    }
  ],
  "meta": { "current_page": 1, "last_page": 7, "per_page": 20, "total": 134 },
  "filters_applied": { "city_id": 2, "department_id": 3, ... }
}
```

---

## 8. Doctor Public Profile Page

Route: `/{doctor-slug}` on patient web. Renders:
- Cover image, avatar, name, tagline
- Bio, languages, experience summary
- Education & experience timelines
- Fee table (in-person, online, follow-up)
- Reviews (paginated, with rating histogram)
- "Book Appointment" CTA → `/{doctor-slug}/book`
- Clinic list (paginated cards with map)
- Sticky mobile CTA bar

---

## 9. Doctor App (Flutter) — Doctor-Specific

### 9.1 Screens
- **Today** — list of today's appointments with status, patient name, queue position.
- **Schedule** — week view of the doctor's own schedule.
- **Patient** — list of patients they've seen.
- **Appointment Detail** — start visit, complete, write Rx, view history, start video.
- **Profile** — edit bio, fee, schedule, see rating.

### 9.2 Doctor App API (subset of admin)
```
GET    /api/v1/doctor/dashboard?date=today
GET    /api/v1/doctor/appointments?date=today&status=confirmed
GET    /api/v1/doctor/appointments/{id}
POST   /api/v1/doctor/appointments/{id}/start       (status -> in_consultation)
POST   /api/v1/doctor/appointments/{id}/complete    (status -> visited)
GET    /api/v1/doctor/patients?q=
GET    /api/v1/doctor/patients/{id}
GET    /api/v1/doctor/prescriptions?patient_id=
POST   /api/v1/doctor/prescriptions
GET    /api/v1/doctor/me/profile
PUT    /api/v1/doctor/me/profile
```

### 9.3 Realtime
Doctor subscribes to `doctor.{id}.appointments` private channel. New bookings, cancellations, queue calls all push in real time.

---

## 10. Doctor Service Methods

`app/Modules/Doctors/Services/DoctorService.php`

```php
class DoctorService {
    public function create(array $data): Doctor;
    public function updateProfile(Doctor $d, array $data): Doctor;
    public function bulkSaveSchedule(int $doctorId, ?int $clinicId, array $weekdays): void;
    public function generateSlots(Doctor $d, Carbon $from, Carbon $to): int;
    public function availableSlots(Doctor $d, Carbon $date, string $type): Collection;
    public function search(SearchDoctorsDTO $dto): LengthAwarePaginator;
    public function recalculateRating(Doctor $d): void;
    public function softDelete(Doctor $d): void; // unmap from appointments
    public function impersonateByAdmin(Doctor $d, User $admin): string; // returns Sanctum token
}
```

---

## 11. Validation Rules

```php
// UpdateDoctorRequest
public function rules(): array {
    return [
        'first_name' => ['required', 'string', 'max:60'],
        'last_name'  => ['required', 'string', 'max:60'],
        'slug'       => ['required', 'string', 'max:60', 'regex:/^[a-z0-9-]+$/', Rule::unique('doctors')->ignore($this->doctor)],
        'bio'        => ['nullable', 'string', 'max:5000'],
        'consultation_fee' => ['required', 'integer', 'min:0'],
        'online_consultation_fee' => ['nullable', 'integer', 'min:0'],
        'follow_up_fee'  => ['nullable', 'integer', 'min:0'],
        'currency_id' => ['required', 'exists:currencies,id'],
        'experience_years' => ['integer', 'min:0', 'max:80'],
        'languages_spoken' => ['array', 'max:10'],
        'languages_spoken.*' => ['string', 'max:10'],
        'accepts_online_consultation' => ['boolean'],
        'accepts_in_person_consultation' => ['boolean'],
        'avatar' => ['nullable', 'image', 'max:2048'],
        'cover_image' => ['nullable', 'image', 'max:5120'],
        'department_ids' => ['array'],
        'department_ids.*' => ['exists:departments,id'],
        'specialization_ids' => ['array'],
        'specialization_ids.*' => ['exists:specializations,id'],
        'service_ids' => ['array'],
        'service_ids.*' => ['exists:services,id'],
        'clinic_ids' => ['array', 'min:1'],
        'clinic_ids.*' => ['exists:clinics,id'],
    ];
}
```

---

## 12. Realtime Events

| Event | Channel | Trigger |
|---|---|---|
| `DoctorProfileUpdated` | `public.doctors` | Caches invalidate |
| `DoctorScheduleUpdated` | `public.doctors.{id}` | Slot list refreshes |
| `DoctorNewReview` | `doctor.{id}` | Dashboard live updates |

---

## 13. Audit

`Doctor` create/update/delete is audited. `DoctorSchedule` save is audited. `DoctorReview` create/update/delete is audited. `Slot` status changes (`blocked`/`unblocked`) are audited but not the auto-generated `available` transitions (too noisy).

---

## 14. Testing Strategy

| Test | Assertion |
|---|---|
| `SlotGeneratorTest` | 9 AM-5 PM with 20-min slots → 24 slots/day, no overlaps, rolling window extends |
| `SlotHoldTest` | Two users can't hold the same slot |
| `ScheduleOverlapTest` | Saving overlapping schedules throws |
| `DoctorSearchTest` | All 9 filters compose; cache hit/miss |
| `RatingAggregatorTest` | Adding 5-star review updates avg within 100ms |
| `ReviewReplyTest` | Doctor can reply once, can edit, can delete own reply |
| `SoftDeleteTest` | Deleting a doctor with appointments is blocked |
| `BulkScheduleSaveTest` | Atomic: 7 days saved or none |

---

## 15. Acceptance Criteria (V1)

- [ ] Admin can add a doctor with full profile, education, experience, languages, fees.
- [ ] Admin can save a weekly schedule; 60 days of slots are generated within 1 second.
- [ ] Patient can search doctors by all 9 filters; result in <500ms p95.
- [ ] Patient opens doctor profile, sees reviews, fees, schedule, available slots.
- [ ] Patient holds a slot for 10 min during checkout; auto-release on timeout.
- [ ] Two patients trying to book the same slot — one wins, one gets 409.
- [ ] Doctor receives a FCM push for new appointment within 5 seconds.
- [ ] Doctor updates their profile from the app; changes reflect on public site within 60 seconds.
- [ ] Doctor can post a review reply from the app; it appears on public profile.
- [ ] Soft-deleting a doctor with active appointments is blocked with clear error.
- [ ] Review aggregates are always consistent.

---

## 16. V2 Roadmap — Doctors

> V1 models a doctor with profile, schedule, and reviews. V2 adds **geo-search**, **AI-powered recommendations**, **rotating schedules** for group practices, and **multi-currency payout**.

### 16.1 Schema additions

```php
Schema::table('doctors', function (Blueprint $t) {
    $t->string('slug', 128)->nullable()->unique();         // 'dr-jane-doe-cardiology' for SEO URL
    $t->string('geo_lat', 32)->nullable();                 // DECIMAL(10,7); redundant with PostGIS point for index simplicity
    $t->string('geo_lng', 32)->nullable();
    $t->string('geo')->nullable();                         // PostGIS POINT, requires extension enabled
    $t->string('rotation_group_id', 26)->nullable()->index();
    $t->string('payout_currency', 3)->default('INR');
    $t->string('payout_method', 16)->default('bank');     // 'bank' | 'upi' | 'paypal' | 'stripe_connect'
    $t->json('payout_details')->nullable();               // encrypted; bank account / UPI ID / PayPal email
    $t->json('languages')->nullable();                    // ['en', 'hi', 'ta']
    $t->json('badges')->nullable();                       // ['top_rated', 'verified', 'quick_responder']
    $t->integer('response_time_minutes')->nullable();      // median, recomputed nightly
    $t->integer('consultation_count')->default(0);         // denormalized for sort
});
```

```php
// doctor_reviews gets an embedding column for AI search
Schema::table('doctor_reviews', function (Blueprint $t) {
    $t->text('embedding')->nullable();                    // vector(1536), generated by nightly job
});
```

### 16.2 PostGIS geo-search — `geo-search` (V1.1, MML 4) — deep-dive

**1. Overview.** The "find a doctor near me" experience is table-stakes for any modern patient app. V1 has a city-level search (the patient picks a city; the API returns doctors in that city). V1.1 adds **true geo-search** powered by **PostGIS** (per `00-Architecture §16.2` and `15-DB-Schema §432`): the patient grants location permission (or types an area / drops a pin), the API returns doctors within a configurable radius (default 5 km, max 50 km), ordered by distance, with optional filters (specialty, rating, availability, insurance accepted, language). The patient app renders the results as a **list + map** view (Mapbox GL JS in web, native maps on iOS / Android), with the doctor's pin, the distance, and the next available slot. MML 4 GA covers: PostGIS schema, the GiST index, the `DoctorGeoSearch` service, the public API, the Mapbox tile precompute (per the existing stub), the "use my location" UX, the area search, the filters, the per-tenant feature flag, and the fallback for tenants without PostGIS. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.1 lane module.

**2. PostGIS schema.** (Builds on the existing §16.2 stub.) Adds:
```sql
-- Enable PostGIS (per the installation guide)
CREATE EXTENSION IF NOT EXISTS postgis;

-- doctors.geo is a PostGIS POINT (lng, lat) in SRID 4326 (WGS84)
ALTER TABLE doctors ADD COLUMN geo GEOMETRY(Point, 4326);
ALTER TABLE doctors ADD COLUMN geo_lat DECIMAL(10, 7);  -- redundant for index simplicity
ALTER TABLE doctors ADD COLUMN geo_lng DECIMAL(10, 7);

-- GiST index for fast radius queries
CREATE INDEX doctors_geo_gist ON doctors USING GIST (geo);

-- Optional: index on (tenant_id, is_listed) for tenant-scoped searches
CREATE INDEX doctors_tenant_geo_gist ON doctors USING GIST (geo) WHERE is_listed = true;
```
The `geo` column is populated when the doctor's clinic is registered: the clinic's address is geocoded via the Mapbox Geocoding API (or Google Maps Geocoding API, per tenant preference), and the resulting `lng, lat` is stored in both `geo` (PostGIS) and `geo_lat` / `geo_lng` (decimal, for display and for non-PostGIS fallback). A re-geocode cron runs monthly to catch address corrections.

**3. The `DoctorGeoSearch` service.** (Per the existing §16.2 stub, expanded.)
```php
// app-modules/geo-search/src/Services/DoctorGeoSearch.php
namespace App\Modules\GeoSearch\Services;

class DoctorGeoSearch {
    public function nearby(float $lat, float $lng, float $radiusKm = 5.0, array $filters = []): LengthAwarePaginator {
        $point = "ST_SRID(POINT($lng, $lat), 4326)::geography";
        $query = Doctor::query()
            ->selectRaw("doctors.*, clinics.name AS clinic_name, clinics.address_line1, clinics.city,
                         ST_Distance(doctors.geo, $point) / 1000 AS distance_km,
                         (SELECT MIN(slot_start) FROM doctor_slots WHERE doctor_id = doctors.id AND slot_start > NOW() AND status = 'open') AS next_available_at")
            ->join('clinics', 'clinics.id', '=', 'doctors.primary_clinic_id')
            ->whereRaw("ST_DWithin(doctors.geo, $point, ?)", [$radiusKm * 1000])
            ->where('doctors.is_listed', true)
            ->where('doctors.status', 'active')
            ->orderBy('distance_km');

        // Filters
        if (isset($filters['specialization_id'])) $query->where('doctors.specialization_id', $filters['specialization_id']);
        if (isset($filters['language'])) $query->whereJsonContains('doctors.languages', $filters['language']);
        if (isset($filters['min_rating'])) $query->where('doctors.avg_rating', '>=', $filters['min_rating']);
        if (isset($filters['insurance'])) $query->whereJsonContains('doctors.accepted_insurances', $filters['insurance']);
        if (isset($filters['available_within_hours'])) $query->whereRaw("(SELECT MIN(slot_start) FROM doctor_slots WHERE doctor_id = doctors.id AND slot_start > NOW() AND slot_start < NOW() + INTERVAL ? HOUR AND status = 'open') IS NOT NULL", [$filters['available_within_hours']]);
        if (isset($filters['consultation_fee_max'])) $query->where('doctors.consultation_fee', '<=', $filters['consultation_fee_max']);

        return $query->paginate(min($filters['limit'] ?? 20, 100));
    }

    public function withinBounds(float $minLat, float $minLng, float $maxLat, float $maxLng, array $filters = []): Collection {
        // For map view: return all doctors within the visible map bounds
        $query = Doctor::query()
            ->selectRaw("doctors.*, clinics.name AS clinic_name,
                         ST_Y(doctors.geo::geometry) AS lat,
                         ST_X(doctors.geo::geometry) AS lng")
            ->join('clinics', 'clinics.id', '=', 'doctors.primary_clinic_id')
            ->whereRaw("ST_Within(doctors.geo, ST_MakeEnvelope(?, ?, ?, ?, 4326))", [$minLng, $minLat, $maxLng, $maxLat])
            ->where('doctors.is_listed', true)
            ->where('doctors.status', 'active');
        return $query->get();
    }
}
```
The `geography` cast (vs `geometry`) is important: `geometry` is in the SRID's unit (degrees for 4326), `geography` is in meters. `ST_DWithin(geo, point, distance_in_meters)` is what we want.

**4. The "use my location" UX.** The patient app prompts for location permission (browser geolocation API, or `geolocator` Flutter package). On grant, the app sends `{ lat, lng, radius_km: 5 }` to `/api/v1.1/public/doctors/nearby`; the API returns the doctors within 5 km, ordered by distance, with the distance in km, the next available slot, and the doctor's photo / specialty / rating. The app renders the list and the map (Mapbox GL JS) with the doctor's pin and the user's pin. Tapping a pin highlights the list item; tapping a list item centers the map on the doctor's pin. A "Search this area" button is shown when the user pans the map; on tap, the app calls `withinBounds` with the new bounds and re-renders.

**5. The area search UX (no location permission).** If the patient denies location, the app falls back to:
- A **text search** (the patient types a city, neighborhood, or landmark; the app uses Mapbox Geocoding to convert to lat/lng).
- A **pin drop** (the patient pans the map to their area; the app uses the map's center as the search point).
- A **city picker** (the patient picks a city from a list; the app uses the city's centroid).

**6. The Mapbox tile precompute.** Per the existing stub, a nightly job (`GenerateMapTilesJob`) generates a static tile JSON for each city: a low-resolution grid (e.g., 100x100 cells) where each cell contains the count of doctors and a sample doctor's `id`. The patient app loads the tile for the user's city once (no API call per scroll) and renders the dots on the map. The tile is regenerated when a doctor is added or removed (via the `geo.invalidate.{tenant}` realtime event) — the regeneration is incremental, not a full rebuild.

**7. The 8 filters.**
- **Specialty** (`specialization_id`): exact match.
- **Sub-specialty** (`sub_specialization`): partial match.
- **Language** (`language`): JSON contains.
- **Min rating** (`min_rating`): `avg_rating >= X`.
- **Insurance accepted** (`insurance`): JSON contains.
- **Available within X hours** (`available_within_hours`): subquery on `doctor_slots`.
- **Max consultation fee** (`consultation_fee_max`): `consultation_fee <= X`.
- **Min years of experience** (`min_experience`): `years_of_experience >= X`.

**8. Performance.** With the GiST index, a 5 km radius search on 100k doctors takes < 50ms (p99). The Mapbox tile precompute keeps the map view fast (no per-pan API call). The subquery for `next_available_at` is a potential bottleneck; we use a covering index on `doctor_slots (doctor_id, slot_start, status)` to keep it < 100ms. For very large tenants (> 1M doctors), the geo-search is sharded by region.

**9. Fallback for non-PostGIS tenants.** Per the existing stub, a tenant without PostGIS uses the **Haversine formula in MySQL**:
```sql
SELECT doctors.*,
  (6371 * acos(cos(radians(:lat)) * cos(radians(geo_lat)) * cos(radians(geo_lng) - radians(:lng)) + sin(radians(:lat)) * sin(radians(geo_lat)))) AS distance_km
FROM doctors
WHERE (6371 * acos(cos(radians(:lat)) * cos(radians(geo_lat)) * cos(radians(geo_lng) - radians(:lng)) + sin(radians(:lat)) * sin(radians(geo_lat)))) <= :radius_km
ORDER BY distance_km;
```
This is slower (no index; full table scan with computation) but works for tenants with < 1,000 doctors. The V1.1 module auto-detects PostGIS availability and uses the appropriate path.

**10. API Surface.**
```
# Public
GET    /api/v1.1/public/doctors/nearby                         # { lat, lng, radius_km=5, specialization_id?, language?, min_rating?, insurance?, available_within_hours?, consultation_fee_max?, min_experience?, page=1, limit=20 } -> { doctors[], total, page, last_page, user_location: { lat, lng } }
GET    /api/v1.1/public/doctors/within-bounds                  # { min_lat, min_lng, max_lat, max_lng, specialization_id? } -> { doctors[] } (for map view)
GET    /api/v1.1/public/doctors/{id}/distance                  # { lat, lng } -> { distance_km, walking_minutes, driving_minutes }
POST   /api/v1.1/public/doctors/geocode                        # { address } -> { lat, lng, formatted_address, confidence } (server-side geocoding)

# Doctor
PATCH  /api/v1.1/doctor/profile                                 # { clinic_address?, ... } -> triggers re-geocode

# Admin
POST   /api/v1.1/admin/doctors/{id}/re-geocode                 # forces a re-geocode (admin override)
GET    /api/v1.1/admin/geo-search/stats                        # { total_doctors_geocoded, total_pending_geocode, avg_search_latency_ms, tile_last_generated_at }
POST   /api/v1.1/admin/geo-search/tile-regenerate              # forces a tile regeneration
```

**11. Frontend.**
- **Patient app (web):** `/doctors` is a split view: list on the left, map on the right. The map shows the user's pin and the doctors' pins. The list shows the doctor card with name, specialty, distance, next available slot, and rating. Filters are at the top (specialty, language, rating, insurance, available within). The "Use my location" button is prominent.
- **Patient app (mobile):** the same UX as web, with native maps (Apple Maps on iOS, Google Maps on Android) and a bottom sheet for the list.
- **Doctor app:** the doctor's profile shows their location on a map and the search-radius circles (3 km, 5 km, 10 km) for visibility.
- **Admin:** the admin can see the geocoding status of all doctors; doctors with a failed geocode are highlighted; a "Re-geocode" button is per-row.

**12. Realtime.**
- `geo.doctor.added.{tenant_id}.{city_id}` — a new doctor is geocoded; the tile is regenerated; the patient app updates.
- `geo.doctor.removed.{tenant_id}.{doctor_id}` — a doctor is removed; the tile is updated.
- `geo.tile.regenerated.{tenant_id}.{city_id}` — the tile is regenerated; the admin sees a "Last regenerated" timestamp.
- `geo.search.high_demand.{tenant_id}.{area}` — many patients are searching in an area with few doctors; the admin is paged to recruit more doctors.

**13. Security & RBAC.**
- **Location data is sensitive.** The patient's location is sent over HTTPS; it is not stored server-side (the API processes the lat/lng in the request and discards). The audit log does not record the patient's exact lat/lng (only the city / area).
- **Doctor location is public** (for listed doctors); unlisted doctors are excluded from the search.
- **Geocoding quota:** the Mapbox / Google Geocoding API has a monthly quota (50k / month for Mapbox free tier); the geocoder is rate-limited; the re-geocode cron is throttled to 1k / hour.
- **Roles:** `patient` (public search), `doctor` (own location), `tenant_admin` (re-geocode, view stats, force tile regen), `platform_admin` (override quotas).
- **Audit:** every re-geocode, every tile regeneration, every search (with the area, not the exact lat/lng) is logged.

**14. Acceptance Criteria.**
- [ ] A doctor is geocoded when the clinic is registered; `doctors.geo` is populated; the GiST index is used.
- [ ] A patient searches "doctors near me" with location permission; the API returns doctors within 5 km, ordered by distance.
- [ ] A patient searches without location permission; the fallback works (text search / pin drop / city picker).
- [ ] The 8 filters work correctly; the result set is filtered and re-ordered.
- [ ] The map view renders the doctors' pins and the user's pin; tapping a pin highlights the list item.
- [ ] The "Search this area" button triggers a `withinBounds` call; the results update.
- [ ] The Mapbox tile is precomputed nightly; the tile is served from CDN; the patient app loads the tile on city change.
- [ ] A doctor added or removed triggers an incremental tile regeneration; the admin sees the timestamp.
- [ ] The fallback for non-PostGIS tenants works (Haversine); the latency is < 500ms for 1k doctors.
- [ ] The patient's exact lat/lng is not stored; the audit log records only the area.
- [ ] The Geocoding API quota is respected; an exceeded quota opens a P2.

**15. Real-World Test Scenarios.**
- *Patient in Bangalore:* patient grants location; the app shows 28 doctors within 5 km; the list is ordered by distance; the map shows the pins; the patient picks a cardiologist 1.2 km away.
- *Patient without location:* patient types "Koramangala, Bangalore"; the geocoder returns `{ lat: 12.9352, lng: 77.6245 }`; the search runs on that point.
- *Doctor moves clinic:* doctor updates the clinic address; the platform re-geocodes; the doctor's pin moves; the tile is regenerated; the patient app updates within 5 minutes (the next tile regen).
- *Filter by language:* patient filters "Hindi-speaking"; the result set is filtered; 12 of the 28 doctors match.
- *Filter by insurance:* patient filters "Star Health accepted"; the result set is filtered; 8 of the 28 doctors match.
- *Filter by availability:* patient filters "Available within 24 hours"; the subquery on `doctor_slots` filters out doctors with no slots in the next 24h; 5 of the 28 doctors match.
- *High demand area:* many patients are searching in "Whitefield, Bangalore" but only 3 doctors are in the area; the admin is paged; the admin recruits 2 more doctors.
- *Map pan:* patient pans the map to a new area; the "Search this area" button appears; the patient taps; the results update.
- *Geocoding failure:* a new clinic's address is invalid; the geocoder returns a low confidence; the doctor is in `pending_geocode`; the admin is notified; the admin corrects the address and re-geocodes.
- *Non-PostGIS tenant:* a small tenant without PostGIS uses the Havarsine fallback; a 5 km search on 200 doctors takes 200ms (acceptable).
- *Quota exceeded:* the Mapbox quota is exceeded; the geocoder returns 429; the re-geocode cron retries with backoff; a P2 is opened; the admin is notified to upgrade the plan.

**16. What this module is NOT:**
- Not a routing engine. The "walking minutes" / "driving minutes" estimates use Mapbox Directions API for a simple route; we do not provide turn-by-turn navigation.
- Not a global search. The search is per-tenant; cross-tenant search is not supported.
- Not a real-time availability check. The `next_available_at` is a snapshot; the actual availability may change before the patient books.
- Not a doctor rating system. The `avg_rating` is from the existing V1 review system; this module does not add ratings.

**17. Dev Workflow.** `php artisan geo:geocode --tenant=...` triggers a batch geocode for all doctors in the tenant. `php artisan geo:tile-regenerate --tenant=... --city=...` regenerates a tile. `php artisan geo:search --lat=12.97 --lng=77.59 --radius=5` runs a test search. `php artisan geo:quota-check` shows the Mapbox quota usage. Code: `app-modules/geo-search/`; tests: `app-modules/geo-search/tests/Feature/{Geocode,Nearby,WithinBounds,Filters,TileRegenerate,Fallback,Quota}Test.php` and `tests/Performance/geo-search.k6.js` (asserts p99 < 100ms for a 5 km search on 100k doctors).

### 16.3 AI doctor recommendation (`ai-doctor-recommendation`, V2.1)

Patient says "I have chest pain", the symptom checker returns a recommended specialization ("cardiology") and the doctor list endpoint is re-ranked by:

1. **Specialization match** (V1 already supports).
2. **Distance** (PostGIS, V1.1).
3. **Vector similarity** of doctor profile + reviews to patient's description.
4. **Historical success rate** for the condition (de-identified, opt-in).
5. **Availability** (next slot within 48h).
6. **Language match** (V1 has `languages` JSON, V2 weights it).
7. **Insurance accepted** (V2.1).

```php
// app/Services/AIDoctorRecommender.php
class AIDoctorRecommender {
    public function recommend(PatientSymptomContext $ctx, GeoLocation $loc, int $limit = 10): Collection {
        $embedding = $this->embeddings->embed($ctx->text);
        $candidates = Doctor::query()
            ->whereHas('specializations', fn($q) => $q->whereIn('id', $ctx->likelySpecializations))
            ->near($loc, 25) // 25 km
            ->whereRaw("doctor_reviews.embedding <=> ?", [$embedding])  // pgvector cosine distance
            ->orderByRaw("doctor_reviews.embedding <=> ? ASC", [$embedding])
            ->limit(50)
            ->get();
        return $this->rerank($candidates, $ctx);
    }
}
```

Embeddings are regenerated nightly (`GenerateDoctorEmbeddingsJob`) for any doctor with new reviews. Cost: $0.0001 per 1k tokens × ~500 tokens/embedding × 1000 doctors = $0.05/night.

### 16.4 Doctor rotating schedule (`doctor-rotating-schedule`, V1.1, MML 3) — deep-dive

**1. Overview.** V1 ships fixed weekly schedules (per `doctor_schedules` at §4). V1.1 extends this with a **rotating on-call schedule** for group practices and hospital departments. The platform models a **rotation group** (a pool of doctors who share an on-call roster) and a **rotation plan** (the per-doctor, per-day assignment). MML 3 GA covers: rotation groups, rotation plans (daily / weekly / custom), escalation chains (if the on-call doctor doesn't answer in N min, escalate to the next), fair distribution (equalize on-call hours per doctor), swap requests (doctors can swap their shifts), coverage gap detection (a slot is uncovered → alert the admin), patient-facing "after-hours" routing (the patient calls after hours → routed to the on-call doctor), and the analytics dashboard. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.1 lane module. The existing V1.1 stub at §16.4 is replaced with this full deep-dive.

**2. The rotation model.** A `rotation_group` is a named pool (e.g., "Cardiology On-Call", "Pediatrics Night Shift", "Hospitalist Weekend"). Each group has:
- `members` (the doctors in the pool).
- `rotation_type` (`daily` / `weekly` / `custom`).
- `rotation_period` (1-30 days, per the rotation_type).
- `start_date` (the anchor date for the rotation).
- `shift_start_time` / `shift_end_time` (e.g., 18:00-08:00 for night shift; 08:00-18:00 for day shift).
- `timezone` (the clinic's timezone).
- `escalation_timeout_minutes` (default 5; if the on-call doctor doesn't answer in 5 min, escalate).
- `escalation_chain` (ordered list of backup doctors; per §2.5 below).
- `fairness_strategy` (`equal_hours` / `equal_shifts` / `weighted`; per §2.6).
- `coverage_required` (`true` / `false`; if `true`, every day must have an on-call doctor).

**3. The rotation plan.** A `rotation_plan` is the materialized per-doctor, per-day assignment. The system generates the plan for the next 60-90 days when the group is created or updated; the plan is stored in `doctor_rotations`. The system can regenerate the plan when a swap is requested (per §2.4). The plan is deterministic: the same group + start_date + rotation_type produces the same plan. The plan is also **idempotent**: a manual edit (admin override) is preserved across regenerations.

**4. Swap requests.** A doctor can request a swap (e.g., "I can't be on call Friday; can Dr. B cover?"). The flow:
1. The doctor opens the rotation calendar in the doctor app / web.
2. The doctor picks their shift + the target doctor's shift; the doctor submits a swap request.
3. The target doctor receives a push + email; the target accepts or declines.
4. If accepted, the system updates the rotation plan (atomic transaction); the system recalculates the fairness score (per §2.6); the system notifies both doctors + the admin.
5. If declined, the doctor is notified; the request is marked `declined`.
6. If the target doesn't respond in 24 hours, the system sends a reminder; after 48 hours, the system auto-declines.
7. The admin can override: approve a swap even if the target didn't respond; cancel a swap.

**5. Escalation chain.** When a patient calls after hours, the system routes the call to the on-call doctor. The flow:
1. The system identifies the rotation group for the call's specialty.
2. The system looks up the on-call doctor for the current time.
3. The system calls the on-call doctor (via the platform's voice / SMS).
4. The on-call doctor has `escalation_timeout_minutes` (default 5) to answer.
5. If the on-call doesn't answer, the system escalates to the next doctor in `escalation_chain` (e.g., the backup on-call, then the department head, then the admin).
6. The system logs the escalation (who was called, when, whether they answered, how long it took).
7. The system notifies the admin of the escalation (in real time, via push / SMS / email).
8. If no one answers, the system routes the call to the admin's voicemail.

**6. Fairness strategy.** The system uses one of 3 strategies to ensure the on-call burden is fairly distributed:
- **`equal_hours`:** the total on-call hours per doctor over the last 90 days are equalized; the next assignment goes to the doctor with the fewest hours.
- **`equal_shifts`:** the total on-call shifts per doctor are equalized (regardless of shift length); the next assignment goes to the doctor with the fewest shifts.
- **`weighted`:** the admin assigns weights to doctors (e.g., senior doctors get 0.5 weight, juniors get 1.0 weight); the on-call burden is distributed proportional to the weight.

The fairness is recalculated when a new rotation plan is generated or a swap is approved. The admin can override (manually assign a specific doctor to a specific day, even if it breaks fairness); the override is logged.

**7. Coverage gap detection.** The system runs a daily cron at 23:00 to check the next 7 days' coverage:
- For each rotation group, for each day in the next 7 days, check if there is an on-call doctor.
- If a day is uncovered (e.g., all doctors are on leave, or the group has 0 members), the system:
  - Pages the admin (push + email + SMS).
  - Suggests a fix: "Add a doctor to the group", "Extend the shift to cover the gap", "Mark the day as 'uncovered' (the system will route the call to the admin's voicemail)".
  - Logs the gap in `coverage_gaps` for analytics.
- The patient-facing "after-hours" page shows a banner: "Cardiology on-call is currently unavailable; your call will be routed to the on-call admin."

**8. Domain Model.** Builds on the existing V1.1 stub at §16.4. Adds:
```php
Schema::create('rotation_groups', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 128);
    $t->text('description')->nullable();
    $t->jsonb('members');                                        // [{ doctor_id, weight, joined_at, role: 'primary' | 'backup' | 'lead' }]
    $t->string('rotation_type', 16);                             // 'daily' | 'weekly' | 'custom'
    $t->integer('rotation_period_days')->default(1);
    $t->date('start_date');
    $t->string('timezone', 64)->default('UTC');
    $t->string('shift_start_time', 8)->default('18:00');
    $t->string('shift_end_time', 8)->default('08:00');
    $t->integer('escalation_timeout_minutes')->default(5);
    $t->jsonb('escalation_chain')->nullable();                    // [{ doctor_id, order }]
    $t->string('fairness_strategy', 16)->default('equal_hours'); // 'equal_hours' | 'equal_shifts' | 'weighted'
    $t->boolean('coverage_required')->default(true);
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});

Schema::create('rotation_assignments', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('rotation_group_id')->index();
    $t->ulid('tenant_id')->index();
    $t->ulid('doctor_id')->index();
    $t->date('date')->index();
    $t->string('shift_start_at')->nullable();
    $t->string('shift_end_at')->nullable();
    $t->string('source', 16)->default('auto');                   // 'auto' | 'manual' | 'swap' | 'admin_override'
    $t->string('status', 16)->default('scheduled');              // 'scheduled' | 'active' | 'completed' | 'cancelled' | 'swapped'
    $t->timestamp('activated_at')->nullable();
    $t->timestamp('ended_at')->nullable();
    $t->timestamps();
    $t->unique(['rotation_group_id', 'date']);
});

Schema::create('rotation_swaps', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('rotation_group_id')->index();
    $t->ulid('requester_doctor_id')->index();
    $t->ulid('target_doctor_id')->index();
    $t->date('requester_date');
    $t->date('target_date');
    $t->string('status', 16)->default('pending');                // 'pending' | 'accepted' | 'declined' | 'auto_declined' | 'cancelled' | 'admin_approved'
    $t->text('reason')->nullable();
    $t->timestamp('responded_at')->nullable();
    $t->timestamp('responded_by_user_id')->nullable();
    $t->ulid('approved_by_user_id')->nullable();
    $t->timestamps();
});

Schema::create('rotation_escalations', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('rotation_group_id')->index();
    $t->date('date');
    $t->ulid('call_id')->nullable();
    $t->jsonb('attempts');                                       // [{ doctor_id, called_at, answered, response_time_seconds, channel }]
    $t->string('outcome', 16);                                   // 'answered' | 'escalated' | 'voicemail' | 'no_coverage'
    $t->ulid('answered_by_doctor_id')->nullable();
    $t->integer('total_response_time_seconds')->nullable();
    $t->timestamps();
});

Schema::create('coverage_gaps', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('rotation_group_id')->index();
    $t->date('date');
    $t->string('reason', 64);                                    // 'no_members' | 'all_on_leave' | 'admin_uncovered'
    $t->string('status', 16)->default('open');                   // 'open' | 'resolved' | 'accepted'
    $t->ulid('resolved_by_user_id')->nullable();
    $t->timestamp('resolved_at')->nullable();
    $t->timestamps();
});
```

**9. API Surface.**
```
# Tenant admin
GET    /api/v1.1/admin/rotation-groups                          -> [group]
POST   /api/v1.1/admin/rotation-groups                          # { name, members, rotation_type, shift_start, shift_end, ... } -> group
PATCH  /api/v1.1/admin/rotation-groups/{id}                     # { members?, shift_start?, escalation_chain? }
DELETE /api/v1.1/admin/rotation-groups/{id}                     # 204
POST   /api/v1.1/admin/rotation-groups/{id}/regenerate-plan     # { from, to } -> plan (returns the new assignments)
GET    /api/v1.1/admin/rotation-groups/{id}/plan                # { from, to } -> [assignment]
GET    /api/v1.1/admin/rotation-groups/{id}/fairness            # { doctors: [{ doctor_id, total_hours, total_shifts, score }] }
GET    /api/v1.1/admin/rotation-groups/{id}/coverage            # { from, to } -> [coverage_status]
GET    /api/v1.1/admin/coverage-gaps                            -> [gap]
POST   /api/v1.1/admin/coverage-gaps/{id}/resolve               # { resolution: 'add_member' | 'extend_shift' | 'accept_uncovered' } -> 204

# Doctor
GET    /api/v1.1/me/rotations                                   -> [rotation_group] (the doctor's own)
GET    /api/v1.1/me/rotations/{group_id}/plan                   # { from, to } -> [assignment] (the doctor's own shifts)
POST   /api/v1.1/me/rotations/{group_id}/swap                   # { my_date, target_doctor_id, target_date, reason } -> swap
GET    /api/v1.1/me/rotations/swaps                             -> [swap]
POST   /api/v1.1/me/rotations/swaps/{id}/accept                # 204
POST   /api/v1.1/me/rotations/swaps/{id}/decline                # { reason } -> 204
POST   /api/v1.1/me/rotations/swaps/{id}/cancel                # 204

# Patient-facing (after-hours)
GET    /api/v1.1/public/on-call?specialty=cardiology&at=2026-06-05T20:00 -> { rotation_group_id, on_call_doctor: { id, name, photo, languages, escalation_chain } }
POST   /api/v1.1/public/call-after-hours                        # { specialty, patient_id, callback_number } -> { call_id, status: 'routed' | 'escalating' | 'voicemail' }
```

**10. Frontend.**
- **Admin (`/admin/rotations`):** the rotation group list shows all groups with the current on-call (live); the group detail shows the 60-90 day plan in a calendar view (color-coded per doctor), the swap requests, the escalation log, the coverage gap list, the fairness score. The composer is a multi-step wizard: name + specialty + members + rotation type + shift hours + escalation chain.
- **Doctor (`/doctor/rotations`):** the doctor's own rotation calendar shows the doctor's shifts; the swap request button lets the doctor request a swap; the swap history shows pending / accepted / declined / completed swaps.
- **Patient (`/after-hours`):** the patient-facing page shows the current on-call doctor for the specialty (with photo, name, languages, "Call now" button); if the call is not answered, the system escalates and the patient sees "Calling the next on-call..." with a real-time status.

**11. Realtime.**
- `rotation.activated.{tenant_id}.{group_id}.{date}` — the on-call doctor for the date is now active (the shift started).
- `rotation.ended.{tenant_id}.{group_id}.{date}` — the on-call shift ended.
- `rotation.swap.requested.{tenant_id}.{target_doctor_id}` — a swap request was created; the target is notified.
- `rotation.swap.accepted.{tenant_id}.{group_id}.{date}` — a swap was accepted; the plan is updated.
- `rotation.escalation.triggered.{tenant_id}.{call_id}` — an escalation was triggered; the next doctor is called.
- `rotation.escalation.answered.{tenant_id}.{call_id}` — the escalated doctor answered; the call is connected.
- `rotation.coverage.gap.{tenant_id}.{group_id}.{date}` — a coverage gap was detected; the admin is paged.
- `rotation.oncall.changed.{tenant_id}.{specialty}` — the on-call doctor for a specialty changed (live update for the patient-facing page).

**12. Security & RBAC.**
- **Rotation group edit requires `tenant_admin` or `department_head`**; the changes are logged.
- **Swap request is bound to the doctor:** the doctor can only request swaps for their own shifts; the target doctor can only accept / decline swaps targeted at them.
- **Admin override is logged** with the actor and the reason.
- **Escalation chain is private:** the chain is not exposed to patients; the patient sees only the current on-call doctor's name and photo.
- **After-hours call is rate-limited:** the same patient cannot call more than 3 times in 1 hour (prevents abuse).
- **Patient call recording is opt-in** (per the tenant's policy; per the platform's consent management).
- **Roles:** `tenant_admin` (own rotation groups, override, view all), `department_head` (own department's rotation groups), `doctor` (own rotations, own swaps), `patient` (after-hours call), `compliance_officer` (view all, audit, recording access).

**13. Acceptance Criteria.**
- [ ] A tenant admin can create a rotation group with members, rotation type, shift hours, escalation chain, fairness strategy.
- [ ] The rotation plan is auto-generated for the next 60-90 days; the plan is deterministic and idempotent.
- [ ] A doctor can request a swap; the target doctor can accept / decline; the swap is atomic; the fairness score is recalculated.
- [ ] The on-call doctor for a date is correctly identified; the shift starts / ends at the configured time.
- [ ] If the on-call doctor doesn't answer in N min, the system escalates to the next in the chain; the escalation is logged.
- [ ] The fairness strategy is enforced (`equal_hours` / `equal_shifts` / `weighted`); the admin can override.
- [ ] A coverage gap is detected and the admin is paged; the gap is logged in `coverage_gaps`.
- [ ] The patient can see the current on-call doctor for a specialty and call after hours.
- [ ] The call routes to the on-call; if not answered, escalates; the patient sees the live status.
- [ ] The doctor's own rotation calendar shows the doctor's shifts; the swap request is enabled.
- [ ] The admin can regenerate the plan (e.g., after adding a new member); the plan is consistent.
- [ ] The manual override is preserved across regenerations.

**14. Real-World Test Scenarios.**
- *Cardiology on-call (3 doctors, daily rotation):* Dr. A on Monday, Dr. B on Tuesday, Dr. C on Wednesday, then repeat; the patient calls at 8 PM Tuesday; Dr. B is the on-call; the system calls Dr. B; Dr. B answers in 30 seconds; the call is connected; the system logs the answer with response time.
- *Escalation:* Dr. B is the on-call; the patient calls at 8 PM; Dr. B doesn't answer in 5 min; the system escalates to Dr. C (the backup); Dr. C answers in 20 seconds; the call is connected; the system logs the escalation with Dr. B's missed call and Dr. C's answer.
- *Voicemail:* Dr. B and Dr. C don't answer; the system routes to the admin's voicemail; the system sends the admin an SMS + email with the call details.
- *Swap request:* Dr. A is on-call Friday but has a conference; Dr. A requests a swap with Dr. B (who is on-call Saturday); Dr. B accepts; the system updates the plan (Dr. A is on Saturday, Dr. B is on Friday); both doctors are notified; the fairness score is recalculated.
- *Swap declined:* Dr. A requests a swap; Dr. B declines (reason: "I'm on leave Saturday"); Dr. A is notified; the swap is marked `declined`; Dr. A's Friday shift is preserved.
- *Coverage gap:* the cardiology group has 3 doctors; all 3 are on leave Mon-Wed; the daily cron detects the gap for the next 7 days; the admin is paged; the admin accepts the gap; the patient-facing page shows "Cardiology on-call is currently unavailable"; the call routes to the admin's voicemail.
- *Fairness:* the group uses `equal_hours`; Dr. A has 40 hours, Dr. B has 30, Dr. C has 20 over the last 90 days; the next assignment goes to Dr. C (fewest hours).
- *Patient-facing after-hours:* a patient opens the app at 9 PM; the home page shows "Cardiology on-call: Dr. B (8 PM - 8 AM)"; the patient taps "Call now"; the system calls Dr. B; Dr. B answers; the call is connected.
- *Timezone correctness:* the clinic is in IST; the doctor is in UAE (GMT+4); the rotation is in the clinic's timezone; the doctor's calendar shows the shift in UAE time; the patient sees the shift in IST.
- *Manual override:* the admin needs Dr. A on-call Friday (special case); the admin sets the assignment to Dr. A for Friday; the system logs the override; the fairness is broken; the admin is aware.

**15. Dev Workflow.** `php artisan rotation:create-group --tenant=... --name=... --members=... --type=daily` creates a group. `php artisan rotation:regenerate-plan --group=... --from=... --to=...` regenerates the plan. `php artisan rotation:swap --requester=... --target=... --requester_date=... --target_date=...` creates a swap. `php artisan rotation:tick` runs the cron (activate / end shifts, check coverage, log escalations). `php artisan rotation:simulate-call --specialty=cardiology --time=20:00` simulates an after-hours call. Code: `app-modules/doctor-rotating-schedule/`; tests: `app-modules/doctor-rotating-schedule/tests/Feature/{CreateGroup,RegeneratePlan,Swap,Escalation,Fairness,CoverageGap,AfterHours,Timezone}Test.php`.

### 16.5 Doctor payout — `multi-currency-payout` (V1.1, MML 3) — deep-dive

**1. Overview.** Doctors earn a **revenue share** of the consultation fee for every completed appointment (per the tenant's policy — typically 60-80% to the doctor, 20-40% to the platform). V1 supports payouts in the **clinic's base currency** via bank transfer. V1.1 introduces **multi-currency payout**: a doctor can set a **preferred payout currency** (any of 25 supported) and a **payout method** (bank transfer / UPI / PayPal / Stripe Connect / Wise / Payoneer); the platform converts the clinic's base currency to the doctor's preferred currency at the payout-time FX rate, deducts the gateway's transfer fee, and initiates the payout via the appropriate gateway. MML 3 GA covers: payout currency preference, payout method onboarding (per gateway), per-period payout generation (weekly / bi-weekly / monthly), FX conversion at payout time, gateway-specific transfer (Razorpay Route / Stripe Connect / PayPal Mass Pay / Wise / Payoneer), per-payout audit, payslip PDF, and tax withholding (TDS in India, 1099 in the US, VAT in EU). The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.1 lane module.

**2. The 25 supported currencies.** Per `10-Payments §18.5` (V2 multi-currency with FX), the platform supports 25 ISO 4217 currencies: INR, USD, EUR, GBP, AUD, CAD, SGD, HKD, JPY, CNY, KRW, AED, SAR, ZAR, NGN, GHS, KES, BRL, MXN, PHP, IDR, MYR, THB, VND, TRY. Each currency has a `decimal_places` (2 for most, 0 for JPY/KRW/VND/IDR), a `symbol`, a `locale` (for ICU formatting), and an `is_active` flag. The doctor's `payout_currency` is one of these; the platform converts from the clinic's base currency to the payout currency at the payout-time FX rate.

**3. The 5 payout methods.**
- **Bank transfer (India):** NEFT / IMPS / RTGS via **Razorpay Route**. The doctor onboards to Razorpay with a bank account + IFSC + PAN; payouts go to the doctor's bank account in INR. Fee: free for IMPS ≤ ₹1L, ₹5 + GST for larger.
- **Bank transfer (global):** ACH / SEPA / Faster Payments / SWIFT via **Stripe Connect Express**. The doctor onboards to Stripe Express; the platform creates a connected account; payouts go to the doctor's bank in the local currency. Fee: 0.25% capped at $5 per payout (US) / €0.50 (EU).
- **UPI (India):** direct UPI handle (`name@bank`). The doctor provides a UPI ID; the platform initiates a UPI Collect / UPI Autopay payout. Fee: ₹0 (UPI is free for payouts; the platform absorbs the gateway's fee).
- **PayPal:** **PayPal Mass Pay API**. The doctor onboards to PayPal with a verified email; payouts go to the doctor's PayPal account in the doctor's preferred currency. Fee: 2% of the payout amount, capped at $20.
- **Wise / Payoneer:** third-party cross-border payout. The doctor onboards to Wise or Payoneer; the platform initiates a transfer to the Wise / Payoneer account; Wise / Payoneer delivers to the doctor's local bank. Fee: ~1-2% FX margin + a small fixed fee.

**4. The payout flow.**
1. **Configure:** the doctor sets `payout_currency`, `payout_method`, and the gateway-specific details (bank account, UPI ID, PayPal email, etc.) in `/doctor/payout-settings`. The onboarding is per-gateway: for Stripe Connect, the doctor is redirected to Stripe Express onboarding; for Razorpay Route, the doctor enters bank details + PAN; for UPI, the doctor enters the UPI ID (verified via a 1-rupee test credit).
2. **Generate:** the `GeneratePayoutRunJob` runs on the configured schedule (per tenant: weekly / bi-weekly / monthly). The job:
   - For each active doctor, computes the gross revenue from completed appointments in the period.
   - Subtracts refunds (negative).
   - Applies the platform fee (% per doctor or per plan).
   - Applies tax withholding: TDS (India, 10% for individuals), 1099 (US, 0% unless cumulative > $600), VAT (EU, 0% for cross-border B2B services).
   - Converts the net amount to the doctor's `payout_currency` at the **payout-time FX rate** (the rate at the moment the payout is generated, not the rate at the moment of the appointment).
   - Creates a `payouts` row with the gross, the platform fee, the tax withheld, the FX rate, the net in the doctor's currency, the gateway, and the status `pending`.
3. **Initiate:** the `InitiatePayoutJob` runs after generation; for each pending payout, it calls the gateway's transfer API. The gateway returns a `transfer_id`; the payout's status is set to `processing`.
4. **Confirm:** the gateway sends a webhook (`payout.paid` or `payout.failed`); the payout's status is updated. A payslip PDF is generated and emailed to the doctor.

**5. The FX conversion at payout time.** Unlike the invoice-time FX (per `10-Payments §18.5` §6 — FX is locked at invoice creation for the patient's display currency), the payout FX is at **payout time**. The doctor bears the FX risk: if the clinic's base currency weakens against the doctor's preferred currency between the appointment and the payout, the doctor's net is higher; if it strengthens, lower. The doctor sees the **estimated payout** in their preferred currency on the dashboard (using the current FX rate) and the **actual payout** in the payslip (using the payout-time FX rate). The delta is shown in the payslip.

**6. The 6 payout statuses.**
- `pending` — generated but not initiated.
- `processing` — initiated; the gateway is processing.
- `paid` — the doctor has received the funds.
- `failed` — the gateway rejected the transfer (invalid bank, KYC pending, etc.); the doctor is notified to fix the details; the payout is retried after the fix.
- `on_hold` — the payout is paused (e.g., the doctor has an outstanding balance with the platform); admin-only action.
- `reversed` — the payout was paid but reversed (e.g., the doctor requested a clawback for a refund issued after the payout); rare.

**7. Tax handling.** The platform is **not** a tax advisor; the tenant is responsible for the tax obligations. The platform provides:
- **TDS (India):** the platform is the deductor; the payout line item shows the TDS amount; the platform files the TDS return (Form 26Q) quarterly; the doctor receives a TDS certificate (Form 16A) annually. The TDS is 10% for individuals (Section 194J); 0% if the doctor has a valid lower / nil TDS certificate.
- **1099 (US):** the platform is the payer; if a US-based doctor earns > $600 in a calendar year, the platform issues a 1099-NEC at year-end. The 1099 is generated as a PDF and mailed by Jan 31.
- **VAT (EU):** the platform is the B2B service provider; reverse charge applies (the doctor self-accounts for VAT on their side); the invoice shows "Reverse charge" for cross-border B2B.
- **GST (India, B2C):** the consultation fee includes GST; the platform collects and remits GST on the platform's share; the doctor's share is the doctor's own GST obligation (if the doctor is GST-registered).

**8. Domain Model.**
```php
Schema::create('doctor_payout_methods', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('doctor_id')->index();
    $t->string('currency', 3);                                 // ISO 4217
    $t->string('method', 32);                                  // 'razorpay_route' | 'stripe_connect' | 'upi' | 'paypal' | 'wise' | 'payoneer' | 'manual'
    $t->string('gateway_account_id', 128)->nullable();         // e.g., 'acct_xxx' for Stripe
    $t->jsonb('details')->nullable();                            // bank, IFSC, UPI ID, PayPal email, etc. (encrypted at rest)
    $t->string('kyc_status', 16)->default('pending');           // 'pending' | 'verified' | 'rejected'
    $t->jsonb('kyc_documents')->nullable();                      // uploaded docs (in S3, encrypted)
    $t->boolean('is_default')->default(true);
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});

Schema::create('payouts', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('doctor_id')->index();
    $t->ulid('payout_method_id')->nullable()->index();
    $t->ulid('payout_run_id')->nullable()->index();
    $t->date('period_start');
    $t->date('period_end');
    $t->string('currency', 3);                                  // doctor's payout currency
    $t->integer('gross_minor');                                  // gross in doctor's currency
    $t->integer('platform_fee_minor');
    $t->integer('tax_withheld_minor');
    $t->integer('net_minor');                                    // net to doctor
    $t->decimal('fx_rate_at_payout', 18, 8);                     // from clinic base currency to doctor's currency
    $t->string('status', 16)->default('pending');                // 'pending' | 'processing' | 'paid' | 'failed' | 'on_hold' | 'reversed'
    $t->string('gateway_transfer_id', 128)->nullable();
    $t->text('error')->nullable();
    $t->timestamp('initiated_at')->nullable();
    $t->timestamp('paid_at')->nullable();
    $t->string('payslip_url', 1024)->nullable();
    $t->timestamps();
    $t->index(['doctor_id', 'period_end']);
});

Schema::create('payout_runs', function (Blueprint $t) {  // (per 10-Payments §18.7)
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('period', 16);                                   // '2026-W23' | '2026-06'
    $t->date('period_start');
    $t->date('period_end');
    $t->string('status', 16)->default('pending');                // 'pending' | 'processing' | 'completed' | 'failed'
    $t->integer('total_payouts')->default(0);
    $t->integer('total_doctors')->default(0);
    $t->timestamp('processed_at')->nullable();
    $t->timestamps();
});

Schema::create('doctor_tax_forms', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('doctor_id')->index();
    $t->string('form_type', 16);                                // 'tds_certificate' | '1099_nec' | 'vat_invoice' | 'gst_invoice'
    $t->string('tax_year', 8);                                  // '2025-2026' or '2025'
    $t->string('pdf_url', 1024)->nullable();
    $t->timestamp('issued_at');
    $t->timestamps();
});
```

**9. API Surface.**
```
# Doctor
GET    /api/v1.1/doctor/payout-settings                        -> { payout_currency, methods[] }
POST   /api/v1.1/doctor/payout-settings                        # { currency, method, details } -> payout_method
PATCH  /api/v1.1/doctor/payout-settings/{method_id}            # { is_default?, details? }
DELETE /api/v1.1/doctor/payout-settings/{method_id}            # 204
POST   /api/v1.1/doctor/payout-settings/{method_id}/verify     # triggers gateway verification (e.g., 1 INR UPI test)

GET    /api/v1.1/doctor/payouts                               -> [payout] (own payouts)
GET    /api/v1.1/doctor/payouts/{id}                          -> payout (with line items)
GET    /api/v1.1/doctor/payouts/{id}/payslip                   -> PDF
GET    /api/v1.1/doctor/payout-summary                         -> { mtd_net, ytd_net, pending_payout, last_payout, next_payout_date, tax_withheld_ytd }

# Admin
GET    /api/v1.1/admin/payouts                                 -> [payout] (all in tenant)
POST   /api/v1.1/admin/payouts/generate                        # { period_start, period_end } -> payout_run
POST   /api/v1.1/admin/payouts/{id}/retry                     # retry a failed payout
POST   /api/v1.1/admin/payouts/{id}/on-hold                   # pause a payout
POST   /api/v1.1/admin/payouts/{id}/reverse                    # reverse a paid payout (rare)
GET    /api/v1.1/admin/payouts/stats                           -> { total_paid_mtd, total_pending, total_failed, fx_pnl_ytd }
GET    /api/v1.1/admin/payout-runs                             -> [payout_run]

# Tax forms
GET    /api/v1.1/doctor/tax-forms                              -> [tax_form]
GET    /api/v1.1/doctor/tax-forms/{id}/download                -> PDF
```

**10. Frontend.**
- **Doctor:** `/doctor/payouts` lists the payouts with filters (status, period, currency); each row is expandable to show the gross / fees / tax / FX rate / net. The dashboard widget shows the **next payout date**, the **estimated payout** (using the current FX rate), the **YTD net**, and the **tax withheld YTD**. The payout-settings page has the onboarding flow for each method.
- **Admin:** `/admin/payouts` lists all payouts in the tenant with filters; the admin can generate, retry, on-hold, or reverse. The payout-runs page shows the history of runs with the totals.

**11. Realtime.**
- `payout.generated.{tenant_id}.{doctor_id}` — a new payout is generated; the doctor sees the row in the list.
- `payout.initiated.{tenant_id}.{payout_id}` — the payout is initiated; the status is `processing`.
- `payout.paid.{tenant_id}.{payout_id}` — the payout is paid; the doctor is notified; the payslip is emailed.
- `payout.failed.{tenant_id}.{payout_id}` — the payout failed; the doctor is notified to fix the payout method.
- `payout.fx_rate_alert.{tenant_id}.{doctor_id}` — the FX rate moved > 2% in 24h; the doctor is notified (opt-in).

**12. Security & RBAC.**
- **PII / bank details** are encrypted at rest (per V2.1 `field-level-encryption`); the API never returns the bank account number; the UI shows a "Bank account •••• 1234" badge.
- **KYC documents** are stored in a tenant-scoped S3 prefix with SSE-KMS; the documents are accessible only to the admin and the doctor themselves.
- **On-hold requires admin role + co-sign** per `20-Security §11.4`; reversing a paid payout requires admin + co-sign + audit log entry.
- **Payout init requires the doctor's payout method to be `kyc_status='verified'`**; unverified methods cannot receive funds.
- **Roles:** `doctor` (own payouts, own payout settings, own tax forms), `tenant_admin` (all payouts, generate, retry, on-hold, reverse), `finance_admin` (view stats, export to accounting), `compliance_officer` (view tax forms, view audit).
- **Audit:** every payout event (generate, initiate, paid, failed, retry, on-hold, reverse) is logged in `audit_logs` with the actor and the payout ID.
- **PII minimization:** the doctor's tax ID (PAN, SSN, VAT) is encrypted; the platform displays the last 4 only.

**13. Acceptance Criteria.**
- [ ] A doctor can set a payout currency (any of the 25 supported) and a payout method (any of the 5 supported); the method is verified via the gateway's onboarding flow.
- [ ] A payout run is generated on the configured schedule; the per-doctor net is computed correctly (gross - platform fee - tax withholding).
- [ ] The FX conversion is at payout time; the FX rate is stored on the payout; the payslip shows the rate.
- [ ] The gateway transfer is initiated; the status transitions to `processing` then `paid`.
- [ ] A failed payout is retried; the doctor is notified to fix the method.
- [ ] The payslip PDF is generated and emailed; the PDF includes the period, the gross, the fees, the tax, the FX rate, the net, and the doctor's bank / UPI / PayPal details (last 4 only).
- [ ] The tax forms (TDS certificate, 1099-NEC) are generated at year-end and accessible to the doctor.
- [ ] The platform's FX P&L is tracked (the delta between the invoice-time FX and the payout-time FX).
- [ ] The admin can on-hold, retry, or reverse a payout; the action is logged.
- [ ] The doctor's payout method's KYC status is verified before any payout is initiated.

**14. Real-World Test Scenarios.**
- *Indian doctor, INR payout:* a Bangalore-based doctor sets payout_currency=INR, payout_method=razorpay_route with bank details; the platform generates a payout for the week; the net is ₹45,000 (after 10% platform fee and 10% TDS); the Razorpay Route transfer is initiated; the doctor's bank receives the funds; the payslip is emailed.
- *US doctor, USD payout:* a New York-based doctor on a US tenant sets payout_currency=USD, payout_method=stripe_connect; the platform generates a payout; the net is $1,200 (after 20% platform fee, no US tax); the Stripe Connect transfer is initiated; the doctor's US bank receives the funds; the doctor downloads a 1099-NEC at year-end.
- *Indian doctor working in Singapore:* the doctor is on a Singapore tenant; the clinic's base currency is SGD; the doctor sets payout_currency=INR; the platform converts SGD to INR at payout time; the doctor sees the FX rate on the payslip.
- *Bank change:* a doctor changes the bank account; the platform re-verifies; the next payout uses the new bank.
- *Failed payout:* the doctor's bank account is closed; the gateway rejects the transfer; the doctor is notified; the doctor updates the bank; the platform retries; the next attempt succeeds.
- *On-hold:* a doctor has an outstanding balance with the platform (e.g., a chargeback); the admin puts the next payout on hold; the doctor is notified; the doctor pays the balance; the admin releases the hold; the payout is processed.
- *Reverse:* a doctor requests a refund for an appointment that was already paid out; the admin reverses the payout; the doctor's account is debited; the audit log shows the reversal.
- *Year-end tax form:* on Jan 31, the platform issues a 1099-NEC to all US doctors with > $600 in payouts; the doctors download the PDF; the audit log shows the issuance.
- *Multi-currency chain:* a chain admin sees payouts across 5 branches in 3 currencies (INR, USD, EUR); the chain dashboard aggregates the totals in the chain's base currency (INR) using the current FX rate; the per-branch breakdown shows the original currency.

**15. What this module is NOT:**
- Not a payment processor. We use Razorpay / Stripe / PayPal / Wise / Payoneer; we do not hold funds.
- Not a tax advisor. The platform computes tax withholding per the published rules; the tenant is responsible for the actual filing and for any changes in the rules.
- Not a global FX engine. We use the rates from OpenExchangeRates / ECB / Fixer; we do not provide FX hedging or forward contracts.
- Not a payroll system. The platform handles doctor payouts only; staff payroll is a separate system (the tenant uses a payroll provider like Gusto / Razorpay Payroll).

**16. Dev Workflow.** `php artisan payout:generate --tenant=... --period=2026-06` generates a test payout run. `php artisan payout:initiate --run=...` initiates the transfers. `php artisan payout:retry --payout=...` retries a failed payout. `php artisan payout:fx-rate --from=USD --to=INR` shows the current FX rate. `php artisan payout:tax-form --doctor=... --year=2025` generates a test 1099-NEC. Code: `app-modules/multi-currency-payout/`; tests: `app-modules/multi-currency-payout/tests/Feature/{Onboard,Generate,Initiate,FX,Tax,Retry,OnHold,Reverse,Payslip,YearEnd}Test.php` and `tests/Reconciliation/PayoutReconcileTest.php` (asserts every paid payout has a corresponding gateway transfer).

### 16.6 Doctor AI features (V2)

- **AI profile writer:** doctor pastes a CV → AI generates a friendly bio in 5 languages.
- **AI reply to reviews:** doctor's tone + review text → suggested reply (1 click to accept).
- **Smart scheduling:** AI suggests the best slot arrangement based on doctor's historical pattern (morning surgeries, afternoon OPD) — opt-in.

### 16.7 Public API additions (V2)

```
GET    /api/v2/public/doctors/nearby?lat&lng&radius_km&specialization_id -> [doctor with distance]
GET    /api/v2/public/doctors/recommend?symptom_text&lat&lng              -> [doctor with score]
GET    /api/v2/public/doctors/{slug}                                      -> doctor profile (SEO)
POST   /api/v2/doctors/{id}/payout-methods                                -> { currency, method, details }
GET    /api/v2/doctors/{id}/payouts                                        -> [payout]
GET    /api/v2/doctors/{id}/payslips/{payout_id}                          -> PDF
```

### 16.8 V2 acceptance criteria

- [ ] Geo-search returns doctors within 25 km in <300ms p99.
- [ ] AI recommendation returns top 10 doctors for a free-text symptom.
- [ ] Doctor rotation works for a 3-doctor group; on-call status flips at midnight.
- [ ] Doctor payout in USD lands in their Stripe Connect account within 2 business days.
- [ ] AI-generated bio is editable; doctor can lock it from regeneration.
- [ ] Profile slug works for SEO (`/doctors/dr-jane-doe-cardiology`).
