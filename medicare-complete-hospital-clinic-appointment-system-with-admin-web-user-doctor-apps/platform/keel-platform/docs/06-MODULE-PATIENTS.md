# 06 — Patients Module (Patient Master, MRN, Family, Vitals, Files)

> The patient is the unit of care. Every other clinical module (appointments, prescriptions, vitals) hangs off this entity. This document covers the **patient master record**, **MRN generation**, **family members**, **vitals tracking**, and **patient file uploads**.

---

## 1. The Patient as Three Distinct Things

Medicare distinguishes carefully between three concepts that other systems confuse:

| Concept | What | Why |
|---|---|---|
| **User** | The login identity (phone/email) | A single human who can log in |
| **Patient** | A medical record subject | The clinical identity — what the doctor treats |
| **Appointment Patient** | The link between a User and a Patient for one appointment | Allows booking for a family member |

**Example:** Aarav (User) books an appointment for his father (Patient, MRN-0042). Aarav's User is the actor; the Patient row is the clinical subject; the Appointment has `booked_by_user_id = Aarav.id` and `patient_id = Patient.id`.

This is what enables:
- Family booking ("book for my son").
- Caregiver view ("my daughter's appointments").
- Audit clarity (who did what on whose behalf).
- HIPAA-style access controls.

---

## 2. Patient Master Record

### 2.1 Data Model

`patients` (per-tenant, soft-deletable)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `uuid` | char(36) unique | |
| `mrn` | varchar(40) unique per tenant | **Medical Record Number** — see §3 |
| `first_name` | varchar(60) | |
| `middle_name` | varchar(60) nullable | |
| `last_name` | varchar(60) | |
| `display_name` | varchar(120) generated | `first_name middle_name last_name` |
| `gender` | enum('m','f','o','u') | 'u' = unknown/undisclosed |
| `dob` | date | |
| `age_years` | int generated | Computed from `dob`, stored for fast filter |
| `blood_group` | enum('A+','A-','B+','B-','AB+','AB-','O+','O-','unknown') default 'unknown' | |
| `phone` | varchar(20) nullable | May differ from User phone |
| `email` | varchar(180) nullable | |
| `address_line1` | varchar(255) nullable | |
| `address_line2` | varchar(255) nullable | |
| `city_id` | bigint FK nullable | |
| `state` | varchar(120) nullable | |
| `postal_code` | varchar(20) nullable | |
| `country` | varchar(2) nullable | |
| `emergency_contact_name` | varchar(120) nullable | |
| `emergency_contact_phone` | varchar(20) nullable | |
| `emergency_contact_relation` | varchar(40) nullable | |
| `allergies` | json | `["penicillin","shellfish"]` |
| `chronic_conditions` | json | `["diabetes","hypertension"]` |
| `current_medications` | text nullable | Free text |
| `insurance_provider` | varchar(180) nullable | V2 |
| `insurance_policy_number` | varchar(60) nullable | V2 |
| `photo_path` | varchar(500) nullable | |
| `preferred_language_id` | bigint FK nullable | |
| `is_self` | boolean default false | True when this patient row == the user's own record |
| `owner_user_id` | bigint FK nullable | The User who "owns" this patient (e.g., parent of a minor) |
| `is_minor` | boolean default false | Computed: `dob > 18 years ago` is false |
| `guardian_user_id` | bigint FK nullable | For minors |
| `is_deceased` | boolean default false | |
| `deceased_at` | timestamp nullable | |
| `created_by_user_id` | bigint FK | Who created this patient row |
| `created_at`, `updated_at` | timestamps | |
| `deleted_at` | timestamp nullable | |

Indexes:
- `(tenant_id, mrn)` UNIQUE
- `(tenant_id, phone)` (for search)
- `(tenant_id, last_name, first_name)` (for search)
- `(owner_user_id)` (for "my family" lookup)
- `(dob)` (for age filters)

### 2.2 Pivot: `user_patients` (M-to-M)

A user can have multiple patients (themselves + family). A patient can be linked to multiple users (e.g., both parents).

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `user_id` | bigint FK |
| `patient_id` | bigint FK |
| `relation` | enum('self','spouse','child','parent','sibling','other') | |
| `relation_other` | varchar(60) nullable | When `relation = other` |
| `can_book_for` | boolean default true | Authorization to book appointments |
| `can_view_records` | boolean default true | Authorization to see history |
| `can_edit_records` | boolean default false | |
| `created_at`, `updated_at` | timestamps |

Unique: `(user_id, patient_id)`.

### 2.3 Auto-Create on First Login

When a user logs in for the first time via OTP, the system auto-creates a `patient` row with `is_self=true`, `relation=self`, and a placeholder MRN. The user can complete demographics in the profile flow.

This means **the system never has a User without at least one Patient row** (unless explicitly admin-disabled). It also means MRN is generated immediately on registration.

---

## 3. MRN (Medical Record Number)

### 3.1 What It Is
A unique, persistent, **non-PHI** identifier that links all clinical data for a patient across visits, episodes, and even across facilities in a multi-clinic tenant. It is **not** the patient's name or any direct identifier; it's a code.

### 3.2 Why It Matters
- **Patient safety** — prevents mix-ups between patients with similar names.
- **Cross-facility** — same MRN works at all clinics of one tenant.
- **Audit/legal** — required for medical record-keeping regulations in most countries.
- **Future-proofing** — V2 plans to link to pathology labs via MRN.

### 3.3 Generation Algorithm

Configurable per tenant in `settings.mrn_format` JSON:

```json
{
  "prefix": "MRN",
  "separator": "-",
  "segments": [
    { "type": "static", "value": "" },
    { "type": "dynamic", "kind": "year", "format": "YYYY" },
    { "type": "static", "value": "-" },
    { "type": "dynamic", "kind": "sequential", "pad": 6 }
  ]
}
```

This produces `MRN-2026-000001`, `MRN-2026-000002`, ...

**Validation rules** (enforced by the UI and the API):
- At least one static or one dynamic segment.
- At least one dynamic segment of at least 4 characters.
- No two consecutive dynamic segments.
- Max 5 segments, max 50 total MRN characters.

### 3.4 Generation Service

```php
namespace App\Modules\Patients\Services;

class MrnGenerator {
    public function __construct(private Tenant $tenant) {}
    
    public function next(): string {
        $config = $this->tenant->settings['mrn_format'] ?? $this->defaultConfig();
        $seq = $this->getNextSequence(); // uses a sequence table, atomic
        return $this->render($config, $seq, now());
    }
    
    private function getNextSequence(): int {
        return DB::transaction(function () {
            $row = DB::table('patient_mrn_sequences')
                ->where('tenant_id', Tenant::id())
                ->where('year', now()->year)
                ->lockForUpdate()
                ->first();
            if ($row) {
                DB::table('patient_mrn_sequences')
                    ->where('id', $row->id)
                    ->increment('current');
                return $row->current + 1;
            }
            DB::table('patient_mrn_sequences')->insert([
                'tenant_id' => Tenant::id(),
                'year' => now()->year,
                'current' => 1,
                'created_at' => now(),
            ]);
            return 1;
        });
    }
}
```

`patient_mrn_sequences` table tracks the per-tenant per-year counter atomically. Sequence can be reset by admin if needed (rare; risky).

### 3.5 Lookup by MRN

- Patient search by MRN returns the patient in <50ms.
- Doctor app has a "scan MRN barcode" option that uses the device camera + ML Kit to read the printed MRN.
- Reception staff can also manually type.

### 3.6 Edge Cases
- Two patients with the same name + DOB — system warns the admin; requires explicit "yes, create duplicate" or merge.
- Imported patients from another system — admin sets the MRN explicitly during import; if it conflicts, the import aborts.

---

## 4. Family Members

### 4.1 Concept
A family member is simply **another `Patient` row** that is linked to the current user via `user_patients` with `relation != self`. The family member:
- May or may not have their own login (typically no — minor child, elderly parent).
- Has the same fields as a self patient (name, DOB, etc.).
- Can be booked-for, viewed, edited (per pivot permissions).

### 4.2 API

```
GET    /api/v1/patient/family          (list all patients linked to current user)
POST   /api/v1/patient/family          (create new family member patient)
GET    /api/v1/patient/family/{id}     (single patient)
PUT    /api/v1/patient/family/{id}     (update)
DELETE /api/v1/patient/family/{id}     (soft delete; unmap from appointments)
POST   /api/v1/patient/family/{id}/link-existing   (link an existing patient via MRN)
```

### 4.3 Mobile UX
The "Family" tab in the user app shows:
- Self (with "self" badge, can't be removed)
- Spouse, child, parent, sibling cards
- "Add family member" CTA → form (name, relation, DOB, gender, blood group, allergies, photo)
- Tap a card → "Book for {name}", "View history for {name}", "Edit"

### 4.4 Admin-Created Family
Front-desk staff can add a family member on behalf of a patient (e.g., when the parent brings the child for the first time). This is allowed even if the parent has not yet linked the child.

---

## 5. Vitals

### 5.1 Concept
A vital is a **time-series measurement** of a specific health metric for a specific patient. Tracked metrics:

| Metric | Unit | Range | Notes |
|---|---|---|---|
| `blood_pressure_systolic` | mmHg | 50-250 | |
| `blood_pressure_diastolic` | mmHg | 30-150 | |
| `sugar_fasting` | mg/dL | 30-500 | |
| `sugar_random` | mg/dL | 30-500 | |
| `sugar_hba1c` | % | 3-15 | |
| `spo2` | % | 50-100 | |
| `temperature_c` | °C | 30-45 | |
| `temperature_f` | °F | 86-113 | Auto-converted to C |
| `weight_kg` | kg | 1-500 | |
| `height_cm` | cm | 30-250 | |
| `bmi` | kg/m² | computed | `weight / (height/100)^2` |
| `pulse_bpm` | bpm | 30-250 | |
| `respiratory_rate` | breaths/min | 5-80 | |
| `oxygen_flow` | L/min | 0-15 | |
| `pain_score` | 0-10 | subjective | |

### 5.2 Data Model

`vitals` (per-tenant, append-only)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `patient_id` | bigint FK |
| `recorded_by_user_id` | bigint FK | Who entered it (patient or staff) |
| `recorded_at` | timestamp | When the measurement was actually taken |
| `source` | enum('self','home_care','clinic','device_import') | |
| `metric` | varchar(40) | See list above |
| `value_numeric` | decimal(10,2) | |
| `value_text` | varchar(40) nullable | For non-numeric (rare) |
| `unit` | varchar(10) | |
| `notes` | varchar(500) nullable | |
| `device_serial` | varchar(60) nullable | For Bluetooth BP monitor etc. (V2) |
| `created_at`, `updated_at` | timestamps |

Indexes: `(patient_id, metric, recorded_at)` (range queries for graphs).

**Append-only**: vitals are never updated. A correction creates a new row with a note. The history is sacred.

### 5.3 API

```
GET    /api/v1/patient/{id}/vitals?metric=blood_pressure_systolic&from=2026-01-01&to=2026-12-31
POST   /api/v1/patient/{id}/vitals   { metric, value_numeric, unit, recorded_at, source, notes }
DELETE /api/v1/patient/vitals/{id}   (admin only, with audit)
GET    /api/v1/patient/{id}/vitals/chart?metric=weight_kg&period=30d
```

### 5.4 Chart Endpoint

Returns a downsampled series for graphing:
```json
{
  "metric": "weight_kg",
  "unit": "kg",
  "data": [
    { "date": "2026-05-01", "value": 75.2, "sample_count": 3 },
    { "date": "2026-05-02", "value": 75.0, "sample_count": 1 },
    ...
  ],
  "stats": {
    "min": 74.5,
    "max": 76.1,
    "avg": 75.3,
    "latest": 75.0,
    "trend": "stable"  // "up" | "down" | "stable" — based on last 7 vs prev 7
  }
}
```

Downsampling: if >100 points, aggregate to weekly (avg/min/max).

### 5.5 Mobile UX
- "Vitals" tab in patient/family detail.
- Big "+" button → modal with metric picker, value input, date/time, notes.
- Tabs for each metric; each tab shows the latest value big, a 30-day chart, and a history list.
- BP is special: single "+" creates a `systolic` + `diastolic` pair in one form.

---

## 6. Patient Files

### 6.1 Concept
A patient file is any **non-structured document** attached to a patient — lab reports, imaging, insurance cards, external prescriptions, photos of skin conditions, etc.

### 6.2 Data Model

`patient_files` (per-tenant)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `patient_id` | bigint FK |
| `uploaded_by_user_id` | bigint FK | |
| `category` | enum('lab_report','imaging','prescription_external','discharge_summary','insurance','id_document','photo','other') | |
| `title` | varchar(180) | |
| `description` | text nullable | |
| `file_path` | varchar(500) | Spatie Media path |
| `file_name` | varchar(255) | Original filename |
| `mime_type` | varchar(120) | |
| `file_size_bytes` | bigint | |
| `sha256_hash` | char(64) | For dedup |
| `taken_at` | date nullable | When the report was issued (vs uploaded) |
| `is_shared_with_doctor` | boolean default true | If false, only patient can see |
| `metadata` | json | Lab-specific: test name, results; V2 lab linkage |
| `created_at`, `updated_at` | timestamps |
| `deleted_at` | timestamp nullable |

Indexes: `(patient_id, category)`, `sha256_hash`.

### 6.3 Allowed File Types & Size

- **Images**: jpg, jpeg, png, webp, heic — max 10 MB each
- **PDF**: max 25 MB
- **DICOM** (V2): .dcm — max 50 MB
- **Other**: doc, docx, txt — max 5 MB

Enforced via `Spatie\MediaLibrary\FileAdder\FileAdder::addMedia()` validators. All uploads scanned by ClamAV via `MEDICARE_CLAMAV_HOST` (optional in V1, mandatory in V2).

### 6.4 Storage

Files go to S3-compatible object storage (`MEDICARE_STORAGE_DISK=s3` in prod, `local` in dev). Configuration:
- AWS S3 (default prod)
- Cloudflare R2
- DigitalOcean Spaces
- MinIO (self-hosted)

Bucket per tenant: `medicare-{tenant-slug}-{env}`. Cross-tenant access prevented by IAM policy + Laravel's signed-URL validation.

### 6.5 API

```
GET    /api/v1/patient/{id}/files?category=lab_report&page=1
POST   /api/v1/patient/{id}/files           (multipart, max 25MB)
GET    /api/v1/patient/files/{id}           (returns signed download URL)
GET    /api/v1/patient/files/{id}/download  (302 redirect to signed URL)
DELETE /api/v1/patient/files/{id}
PUT    /api/v1/patient/files/{id}           (metadata only; cannot re-upload)
```

### 6.6 Doctor's View
Doctors see a patient's files:
- On the patient detail page in the doctor app.
- Inline preview (PDFs render in-app; images display; DICOMs open in viewer in V2).
- Doctor can also add files (e.g., photo of a skin lesion taken during consultation).

---

## 7. Patient Service Methods

```php
namespace App\Modules\Patients\Services;

class PatientService {
    public function createSelfPatient(User $user, array $data): Patient;
    public function createFamilyMember(User $owner, array $data): Patient;
    public function linkExisting(User $user, string $mrn, string $relation): Patient;
    public function unlink(User $user, Patient $patient): void;
    public function recordVital(Patient $patient, array $data, User $recordedBy): Vital;
    public function vitalsChart(Patient $patient, string $metric, string $period): array;
    public function uploadFile(Patient $patient, UploadedFile $file, array $meta, User $uploader): PatientFile;
    public function softDelete(Patient $patient, User $actor): void; // unmap first
    public function mergeDuplicates(Patient $keep, Patient $merge, User $actor): void; // V2
    public function searchForDoctor(User $doctor, string $q): Collection;
}
```

---

## 8. Patient Search (Doctor-Facing)

When a doctor needs to find a patient, they can search by:
- MRN (exact)
- Phone (last 4 digits, prefix, or full)
- Name (first, last, or full)
- DOB (exact)
- Email

Endpoint:
```
GET /api/v1/doctor/patients?q=&limit=20
```

Returns paginated list with MRN, name, age, gender, last visit date. Doctor can tap to see full record.

### 8.1 Patient Header
Every patient detail screen shows a **Patient Header** (sticky on mobile):
```
┌────────────────────────────────────────────────────┐
│  [Photo]  Aarav Sharma                             │
│           ♂  Age 8 · A+                           │
│           MRN-2026-000042                         │
│           [Allergies: Penicillin]                 │
│           [View History]  [Book]                  │
└────────────────────────────────────────────────────┘
```

---

## 9. Validation

```php
public function rules(): array {
    return [
        'first_name' => ['required', 'string', 'max:60'],
        'last_name'  => ['required', 'string', 'max:60'],
        'gender'     => ['required', Rule::in(['m','f','o','u'])],
        'dob'        => ['required', 'date', 'before_or_equal:today', 'after:1900-01-01'],
        'blood_group'=> ['nullable', Rule::in(['A+','A-','B+','B-','AB+','AB-','O+','O-','unknown'])],
        'phone'      => ['nullable', 'string', 'regex:/^\+?[0-9]{7,15}$/'],
        'email'      => ['nullable', 'email'],
        'allergies'  => ['nullable', 'array', 'max:50'],
        'allergies.*'=> ['string', 'max:60'],
        'chronic_conditions' => ['nullable', 'array', 'max:50'],
        'photo'      => ['nullable', 'image', 'max:2048'],
        'relation'   => ['required_if:is_family,true', Rule::in(['spouse','child','parent','sibling','other'])],
        'relation_other' => ['required_if:relation,other', 'string', 'max:60'],
    ];
}
```

## 10. Realtime Events

| Event | Channel | Use |
|---|---|---|
| `PatientCreated` | `clinic.{tenant}.staff` | Staff dashboard updates |
| `VitalRecorded` | `patient.{id}` (user+authorized family) | Chart updates without refresh |
| `FileUploaded` | `patient.{id}` | New file notification |
| `PatientLinked` | `user.{id}` | Family member added |

---

## 11. Audit

| Model | Audited Events |
|---|---|
| Patient | created, updated, deleted, linked, unlinked |
| Vital | created, deleted (updates are not allowed) |
| PatientFile | uploaded, deleted, downloaded (download is `PatientFileAccessed` log) |

**PHI access** (any read of a patient record by a staff user) is logged in `audit_logs` with `action=view`. This is what enables HIPAA-style "who looked at this patient's chart?" reporting.

---

## 12. Testing Strategy

| Test | Assertion |
|---|---|
| `MrnGeneratorTest` | Concurrency: 100 parallel calls produce 100 unique MRNs |
| `PatientCreateTest` | Self patient created on first login; MRN assigned |
| `FamilyLinkTest` | Linking by MRN requires correct verification |
| `VitalAppendOnlyTest` | Cannot update a vital; correction creates new row |
| `VitalsChartTest` | Returns downsampled series with correct min/max/avg |
| `FileUploadTest` | Max size enforced; mime validated; ClamAV triggers (mock) |
| `FileDedupTest` | Same SHA-256 → same file (linked, not duplicated) |
| `DoctorSearchTest` | Doctor sees only patients they have a relationship with |
| `MinorAccessTest` | Parent can book for child; non-guardian cannot |
| `CrossTenantTest` | Patient in tenant A invisible to tenant B |

---

## 13. Acceptance Criteria (V1)

- [ ] First-time OTP login auto-creates a self patient with MRN.
- [ ] MRN is unique per tenant, sequential, never re-used.
- [ ] Family member can be added, booked for, and have their own history.
- [ ] Vitals can be added by patient and by staff; chart renders correctly.
- [ ] Vitals cannot be edited; corrections append with note.
- [ ] File upload accepts allowed types; rejects others; virus scanned.
- [ ] Doctor can search and find a patient by MRN, name, or phone.
- [ ] Patient header shows key info at a glance.
- [ ] All PHI reads and writes are audited.
- [ ] Soft-deleting a patient with past appointments is blocked.

---

## 14. V2 Roadmap — Patients

> V1 models a patient with profile, vitals, family links, and files. V2 turns the patient into a **portable EHR** with problem list, allergies, lab trends, ABDM sync, AI clinical notes, and an in-app chat history attached to the chart.

### 14.1 Schema additions

```php
Schema::table('patients', function (Blueprint $t) {
    $t->string('abha_id', 16)->nullable()->index();        // 14-digit ABHA; field-level encrypted
    $t->string('abha_address', 64)->nullable();             // abha@abdm
    $t->string('blood_group', 4)->nullable();
    $t->json('allergies')->nullable();                       // [{substance, severity, reaction, recorded_at}]
    $t->json('chronic_conditions')->nullable();              // [{icd10_code, label, diagnosed_at, status}]
    $t->json('current_medications')->nullable();             // [{drug, dose, started_at}]
    $t->string('emergency_contact_name', 128)->nullable();
    $t->string('emergency_contact_phone', 32)->nullable();
    $t->string('emergency_contact_relation', 32)->nullable();
    $t->json('insurance_policies')->nullable();              // [{payer_id, policy_number, valid_till, primary: bool}]
    $t->date('date_of_death')->nullable();
    $t->string('cause_of_death', 255)->nullable();
});
```

### 14.2 New tables: EHR core

```php
Schema::create('patient_problem_list', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->string('icd10_code', 16)->nullable()->index();
    $t->string('snomed_code', 32)->nullable()->index();
    $t->string('label', 255);
    $t->string('status', 16);                                // 'active' | 'resolved' | 'inactive'
    $t->date('onset_date')->nullable();
    $t->date('resolved_date')->nullable();
    $t->ulid('recorded_by_doctor_id')->nullable();
    $t->text('notes')->nullable();
    $t->timestamps();
});

Schema::create('patient_family_history', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->string('relation', 32);                              // 'father' | 'mother' | 'sibling' | 'child' | 'grandparent'
    $t->string('condition', 255);
    $t->integer('age_at_onset')->nullable();
    $t->string('icd10_code', 16)->nullable();
    $t->text('notes')->nullable();
    $t->timestamps();
});

Schema::create('patient_lab_results', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->ulid('lab_order_id')->nullable()->index();
    $t->string('test_code', 32)->index();                    // LOINC code
    $t->string('test_name', 128);
    $t->string('value_type', 16);                            // 'numeric' | 'text' | 'coded' | 'range'
    $t->string('value_numeric', 32)->nullable();             // store as string to preserve precision
    $t->string('value_text', 255)->nullable();
    $t->string('value_unit', 32)->nullable();
    $t->string('reference_range_low', 32)->nullable();
    $t->string('reference_range_high', 32)->nullable();
    $t->string('abnormal_flag', 16)->nullable();             // 'L' | 'H' | 'HH' | 'LL' | 'A' | 'N'
    $t->timestamp('collected_at')->index();
    $t->timestamp('resulted_at')->nullable();
    $t->json('metadata')->nullable();
    $t->timestamps();
});

Schema::create('patient_vaccinations', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->string('vaccine', 128);
    $t->string('cvx_code', 16)->nullable();
    $t->date('administered_at');
    $t->string('lot_number', 64)->nullable();
    $t->string('manufacturer', 64)->nullable();
    $t->ulid('administered_by_doctor_id')->nullable();
    $t->date('next_due_at')->nullable()->index();
    $t->timestamps();
});

Schema::create('patient_surgical_history', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->string('procedure', 255);
    $t->string('cpt_code', 16)->nullable();
    $t->date('date');
    $t->ulid('performed_by_doctor_id')->nullable();
    $t->text('complications')->nullable();
    $t->timestamps();
});
```

### 14.3 New table: `patient_attachments` (S3-backed files)

```php
Schema::create('patient_attachments', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->ulid('uploaded_by_user_id')->index();
    $t->string('category', 32);                              // 'lab_report' | 'imaging' | 'discharge_summary' | 'prescription_scan' | 'insurance' | 'other'
    $t->string('mime_type', 64);
    $t->string('original_filename', 255);
    $t->string('s3_key', 255);
    $t->string('s3_bucket', 128);
    $t->integer('size_bytes');
    $t->string('sha256', 64)->index();
    $t->string('virus_scan_status', 16)->default('pending'); // 'pending' | 'clean' | 'infected' | 'error'
    $t->json('metadata')->nullable();
    $t->timestamp('taken_at')->nullable();
    $t->timestamps();
});
```

V1 uses inline `patient_files` (V1 doc). V2 keeps the V1 table for backwards compat and adds `patient_attachments` with the new categories. Migration copies existing rows to the new table, mapping `category` based on MIME type heuristic.

### 14.4 ABDM/ABHA integration (`abdm-bridge`, V1.2)

The full deep-dive lives at `02-Auth §17.4`. The patient-side summary:

When the patient links their ABHA:

1. **Profile sync:** `PATCH /api/v1.2/auth/abha/verify` after OTP confirms the ABDM profile.
2. **Consent screen:** explain what data we'll fetch and store; patient grants via the HIE-CM (ABDM app or our integrated consent screen).
3. **ABDM HIU pull:** NHA returns 24-month `health_records` for the patient: past visits, conditions, medications, lab reports.
4. **Map to local tables:** `patient_problem_list`, `patient_lab_results`, `patient_surgical_history`, `patient_vaccinations`, `patient_encounters`. Each mapped row has a `source='abdm'` flag and a `source_id` (the FHIR resource id).
5. **Refresh cadence:** every 30 days, with patient consent, re-pull the records (incremental).
6. **HIP push:** every visit completion / Rx sign / lab result publish triggers a push to the patient's ABHA locker.

ABDM data is **read-only** in our system: a `BEFORE UPDATE` trigger on each ABDM-sourced table blocks edits; corrections create a new local row with `source='local'` and `corrects_abdm_id=<row.id>`. Consent artifacts are stored in `abdm_consent_artifacts` and can be revoked by the patient at any time. ABDM M1/M2/M3 certification is required for production; the technical integration is complete.

### 14.5 Family booking — `family-booking` (V1.1, MML 4) — deep-dive

**1. Overview.** Many real-world appointments are not for the user but for a **dependent**: a mother books for her child, a son books for his elderly father, a spouse books for their partner, a caregiver books for a patient with dementia. V1 already supports a `patient_patient` pivot (self → family link) and a `guardian_user_id` column on `patients` (for minors). V1.1 turns this into a **first-class booking experience** in the patient app: the user sees a "Book for:" picker at the top of every booking flow with the user + their dependents; each dependent has their own medical record (MRN, vitals, history); the booking is created with `patient_id=dependent.id` and `booked_by_user_id=parent.user_id`; consent is recorded and revocable. MML 4 GA covers: dependent linking (with OTP verification), the "Book for" picker, the consent scopes (view records, book, pay, receive notifications), minor-mode (parent-as-user), the 4 dependent types (spouse, child, parent, sibling, caregiver, power-of-attorney — extended from the V3 §17.5 stub), and the audit trail. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.1 lane module. The deeper "Family & Caregiver Management" V3 module (`family-caregivers`, per `06-Patients §17.5`) extends this with proxy access, healthcare proxy, family history auto-aggregation, and the age-18 transfer flow.

**2. The 6 dependent types.** (V1.1 ships the 4 most common; V3 adds 2 more.)
- **Self** (`relationship_type='self'`): the user is the patient.
- **Spouse** (`relationship_type='spouse'`): married or domestic partner; both are adults; mutual consent.
- **Child** (`relationship_type='child'`): the dependent is a minor; the user is the parent / legal guardian; the parent's account is the primary.
- **Parent** (`relationship_type='parent'`): the dependent is an adult; the user is the adult child / caregiver.
- **Sibling** (`relationship_type='sibling'`): adult sibling; mutual consent.
- **Caregiver** (`relationship_type='caregiver'`): non-family professional caregiver; explicit consent; may be time-limited.
- **Power of Attorney** (V3+): legal authority to act on the patient's behalf; highest consent; rare; requires document upload.

**3. The link flow (adding a dependent).**
1. User → `/profile/family` → "Add a dependent" → picks the relationship type.
2. The system asks for the dependent's **basic info** (name, date of birth, gender, phone number) — **without** creating the dependent as a user. The dependent may or may not be a Medicare user; the link is between the user (booker) and the patient (dependent).
3. **Verification:** for **minors** (the dependent is < 18), the system sends an OTP to the dependent's phone (if any) or to the user's own phone (as a fallback). For **adults** (spouse, parent, sibling, caregiver), the system sends an OTP to the **dependent's own phone**; the dependent enters the OTP; the link is confirmed. If the dependent is not a user, the OTP proves the dependent consented to be linked.
4. The dependent is created as a `patients` row with `created_by_user_id=parent.user_id`, `guardian_user_id=parent.user_id` (for minors), and a `family_relationships` row is created with the relationship type and the consent scopes.
5. The dependent appears in the user's "Book for:" picker.

**4. The 6 consent scopes.** Per `family_relationships`:
- `can_access_records` — view the dependent's appointments, prescriptions, lab results, files.
- `can_book_appointments` — book on behalf of the dependent.
- `can_make_payments` — pay for the dependent's appointments and bills.
- `can_receive_notifications` — receive the dependent's reminders, lab-ready, payment-received, etc.
- `is_emergency_contact` — receive critical alerts (per `20-Security §11.4`).
- `is_healthcare_proxy` (V3+) — authorize procedures, consent to treatment.
Default: a parent linking a minor has all 4 scopes enabled; an adult linking a parent has only `can_book_appointments` and `can_make_payments` (no records access without explicit consent).

**5. The "Book for" picker.** The user opens the booking flow → at the top of the screen, a dropdown or chip row shows the user + their dependents (with name, age, and a small avatar): "Book for: [me (Priya, 35)] / [son (Arjun, 8)] / [mother (Lakshmi, 62)]". Picking a dependent switches the booking context:
- The doctor's specialty list is filtered to those appropriate for the dependent's age / gender (e.g., a pediatric specialty for an 8-year-old).
- The price is shown for the dependent's service (e.g., a pediatric consultation may have a different price).
- The booking is created with `patient_id=dependent.id` and `booked_by_user_id=user.id`.
- The confirmation screen shows "Booked for [dependent name] with [doctor] at [time]".
- The receipt is sent to the user's email (the booker), with a copy to the dependent's email (if any).
- The notification "Appointment reminder" goes to the user's phone (per the consent scope `can_receive_notifications`).

**6. The minor-mode UX.** When the dependent is a minor:
- The parent's account is the primary; the child does not have a separate login.
- The parent's profile shows the child as a sub-profile.
- The child "ages out" at 18 (per V3 §17.5): on the dependent's 18th birthday, the system sends a "Transfer to adult account" prompt; the dependent creates their own user (or claims an existing one); the parent's link is set to `relationship_type='parent'` (not `'guardian'`); the consent scopes default to "view records only with consent".
- Some jurisdictions (e.g., California, EU) allow teens 13-17 to consent to certain treatments (e.g., reproductive health, mental health) without parental consent. The `family_relationships.consent` JSON stores the per-treatment consent scope; a teen can have a private sub-account with a different consent scope (per the tenant's policy).

**7. The 5 link operations.**
- **Add link:** the user adds a dependent; OTP verification; `family_relationships` row is created; default consent scopes are applied.
- **Modify consent:** the user (or the dependent, if adult) modifies the consent scopes; the change is logged in `audit_logs`.
- **Remove link:** the user removes a dependent; the `family_relationships` row is soft-deleted; the dependent's records are unaffected (the dependent may still be a patient in the system); the next booking for the dependent must be by the dependent themselves.
- **Transfer (V3+):** the dependent is transferred from one user to another (e.g., parents divorce; child moves to the other parent); both parents consent; the link is changed.
- **Age-out (V3+):** the dependent turns 18; the system prompts for transfer; the parent's link becomes read-only.

**8. Domain Model.**
```php
// family_relationships (V1.1, extended in V3 §17.5)
Schema::create('family_relationships', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('primary_user_id')->index();                       // the booker / parent
    $t->ulid('related_patient_id')->index();                     // the dependent
    $t->string('relationship_type', 32);                         // 'self' | 'spouse' | 'child' | 'parent' | 'sibling' | 'caregiver' | 'power_of_attorney'
    $t->boolean('is_emergency_contact')->default(false);
    $t->boolean('can_access_records')->default(false);
    $t->boolean('can_book_appointments')->default(true);
    $t->boolean('can_make_payments')->default(false);
    $t->boolean('can_receive_notifications')->default(true);
    $t->boolean('is_minor')->default(false);
    $t->date('minor_until')->nullable();                          // the dependent's 18th birthday
    $t->jsonb('consent')->nullable();                              // per-treatment consent scope
    $t->jsonb('verification')->nullable();                        // OTP attempts, IP, device
    $t->timestamp('verified_at')->nullable();
    $t->timestamp('unlinked_at')->nullable();
    $t->timestamps();
    $t->unique(['primary_user_id', 'related_patient_id', 'relationship_type']);
});

// patients (extended)
Schema::table('patients', function (Blueprint $t) {
    $t->ulid('created_by_user_id')->nullable()->index();         // the user who created the patient
    $t->ulid('guardian_user_id')->nullable()->index();           // for minors
    $t->boolean('is_self_patient')->default(true);               // false for dependents
});

// appointments (extended)
Schema::table('appointments', function (Blueprint $t) {
    $t->ulid('booked_by_user_id')->nullable()->index();         // the booker (may differ from patient)
});
```

**9. API Surface.**
```
# Family management
GET    /api/v1.1/me/family                                     -> [relationship] (the user's dependents)
POST   /api/v1.1/me/family                                     # { related_patient_id, relationship_type, dependent_basic_info?, scopes } -> relationship (sends OTP to dependent)
POST   /api/v1.1/me/family/{id}/verify                         # { otp } -> relationship (verified)
PATCH  /api/v1.1/me/family/{id}                                # { scopes?, is_emergency_contact? }
DELETE /api/v1.1/me/family/{id}                                # 204 (soft-delete; the patient is not deleted)

# Booking
POST   /api/v1.1/appointments                                  # { patient_id (may be a dependent), ... } -> appointment
# The "Book for" picker is a UI concept; the API takes patient_id directly.

# Dependent consent (the dependent themselves, if adult)
GET    /api/v1.1/me/consents                                   -> [consent] (where the user is the dependent)
PATCH  /api/v1.1/me/consents/{relationship_id}                 # { scopes? } -> consent (the dependent modifies the booker's scopes)

# Admin
GET    /api/v1.1/admin/family-links                            -> [relationship] (filter by relationship_type, is_minor, verified)
POST   /api/v1.1/admin/family-links/{id}/revoke                # 204 (admin override; the link is removed)
```

**10. Frontend.**
- **User:** `/profile/family` lists the user's dependents with the relationship, age, consent scopes, and a "Manage" button. The "Add dependent" wizard is a 3-step flow: pick relationship → enter basic info → verify via OTP.
- **Booking flow:** the "Book for" picker is a chip row at the top of every booking screen. The chips show the name, age, and a small avatar (the user's own photo, or a generic avatar for the dependent).
- **Dependent view (V3+):** if the dependent is an adult, they can log in to their own account and see "Linked accounts" — the list of users who have linked them; they can modify the consent scopes for each link.

**11. Realtime.**
- `family_link.added.{tenant_id}.{primary_user_id}` — a new dependent is added; the UI updates.
- `family_link.verified.{tenant_id}.{relationship_id}` — the OTP is verified; the link is active.
- `family_link.consent_changed.{tenant_id}.{relationship_id}` — consent scopes changed; both parties are notified.
- `family_link.removed.{tenant_id}.{relationship_id}` — link removed; the booker can no longer book for the dependent.

**12. Security & RBAC.**
- **OTP verification is mandatory** for all link types (except self); the OTP is sent to the dependent's phone (or the parent's phone for a minor).
- **Minor consent:** for a minor, the parent's consent is sufficient; the minor's consent is not required (but the minor's assent is logged if they are 7+).
- **Adult consent:** for an adult, the adult's OTP is required; the booker cannot add an adult without the adult's explicit consent.
- **Scope minimization:** the default consent scopes are the minimum needed for the link type; a spouse does not get `is_emergency_contact` by default; a child gets all 4 scopes but not `is_healthcare_proxy` (V3+).
- **Audit:** every link, verification, scope change, and unlink is logged in `audit_logs` with the actor, the patient, and the consent scopes.
- **Booking attribution:** every booking has `booked_by_user_id`; the audit log shows who booked for whom; the dependent sees the booking in their own chart.
- **Data isolation:** the booker sees only the data the consent scopes allow; e.g., a parent with `can_access_records=true` sees the child's full chart; a sibling with `can_book_appointments=true` only sees the sibling's upcoming appointments.
- **PHI access logging:** every PHI access by a booker on behalf of a dependent is logged with the booker, the dependent, and the resource (per `20-Security §11.4`).

**13. Acceptance Criteria.**
- [ ] A user can add a dependent (spouse, child, parent, sibling, caregiver); the OTP is sent to the dependent's phone; the link is verified.
- [ ] A minor link does not require the minor's OTP (the parent's OTP is sufficient); the link is verified.
- [ ] An adult link requires the adult's OTP; without it, the link is in `pending_verification` and cannot book.
- [ ] The "Book for" picker shows the user + their verified dependents; the picker filters by age-appropriate specialties.
- [ ] A booking for a dependent is created with `patient_id=dependent.id` and `booked_by_user_id=user.id`.
- [ ] The booking confirmation is sent to the booker; the receipt is sent to the booker + the dependent (if email exists).
- [ ] The consent scopes are enforced: a booker with `can_access_records=false` cannot see the dependent's chart; a booker with `can_make_payments=false` cannot pay for the dependent.
- [ ] A scope change is logged; the booker and the dependent are notified.
- [ ] An unlink removes the link; the dependent's records are unaffected; subsequent bookings for the dependent must be by the dependent themselves.
- [ ] The age-out prompt is sent on the dependent's 18th birthday (V3+); the link is converted to read-only until transfer.
- [ ] Every action is logged in `audit_logs` with the actor, the patient, and the consent scopes.

**14. Real-World Test Scenarios.**
- *Mother books for child:* mother adds her 8-year-old son as a dependent; the OTP is sent to the mother's phone (since the child has no phone); the link is verified; the mother books a pediatric consultation; the booking is created with `patient_id=son.id` and `booked_by_user_id=mother.id`.
- *Son books for elderly father:* son adds his 75-year-old father as a dependent; the OTP is sent to the father's phone; the father enters the OTP; the link is verified; the son books a cardiology consultation; the father receives the appointment reminder (because the consent scope `can_receive_notifications` is enabled by default for parent relationship).
- *Spouse with limited consent:* wife adds husband; the husband modifies the scopes to disable `can_access_records`; the wife can book and pay for the husband but cannot see his lab results.
- *Adult without consent:* a user tries to add an adult as a dependent without the adult's OTP; the link is in `pending_verification`; the user cannot book for the adult; the adult is notified that someone is trying to add them.
- *Age-out:* a 17-year-old turns 18; the system sends a "Transfer to adult account" prompt; the child creates their own user; the parent's link is converted to read-only.
- *Unlink and re-link:* a mother unlinks her child; the child is still a patient in the system (the records are retained); the mother re-links the child; the consent scopes are reset to the defaults.
- *Multi-clinic chain:* a chain admin sees a family link between a mother in the Bangalore branch and a child in the Singapore branch; the link is per-tenant; the chain admin cannot see the link across branches (different tenants).

**15. What this module is NOT:**
- Not a CRM. We do not track family medical history (auto-aggregation is V3+); we track the link only.
- Not a healthcare proxy. `is_healthcare_proxy` is V3+; V1.1 does not allow the booker to consent to procedures on the dependent's behalf.
- Not a power-of-attorney system. The V1.1 link is a "soft" link; for legal authority, the tenant must use the V3+ `power_of_attorney` flow with document upload.
- Not a global link. The link is per-tenant; cross-tenant links (e.g., mother in India, child in Singapore) are not supported (each tenant has its own patient base).

**16. Dev Workflow.** `php artisan family:add --user=... --patient=... --relationship=child` adds a dependent (test mode; no OTP). `php artisan family:verify --relationship=... --otp=...` verifies. `php artisan family:scope-set --relationship=... --scope=can_access_records --value=true` modifies a scope. `php artisan family:age-out --dry-run` shows the dependents who will age out in the next 30 days. Code: `app-modules/family-booking/`; tests: `app-modules/family-booking/tests/Feature/{Add,Verify,BookFor,ScopeEnforce,Unlink,AgeOut,MinorMode}Test.php`.

### 14.6 AI clinical scribe (`ai-clinical-scribe`, V2)

Doctor starts a recording in the app (or web). The audio is uploaded to `medicare-ai-service`:

1. **STT:** Whisper Large V3 transcribes the audio.
2. **Diarization:** pyannote.audio separates doctor vs patient vs third party.
3. **Summarization:** GPT-4o or Claude 3.5 generates a SOAP note (Subjective, Objective, Assessment, Plan).
4. **Structuring:** extracts diagnoses (ICD-10), medications (RxNorm), orders (lab, imaging), follow-up.
5. **Doctor review:** doctor opens the draft, edits, signs.
6. **Storage:** signed note attaches to `patient_encounters` table (new V2 table), audio is stored in S3 with SSE-KMS, encrypted in-transit, deleted after 90 days (configurable per tenant).

`patient_encounters` is the new V2 "consultation" entity that ties together: appointment + vitals + problems + medications + orders + attachments + AI-generated note + chat history + invoice.

```php
Schema::create('patient_encounters', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('appointment_id')->nullable()->unique();
    $t->ulid('patient_id')->index();
    $t->ulid('doctor_id')->index();
    $t->timestamp('started_at');
    $t->timestamp('ended_at')->nullable();
    $t->string('type', 16);                                  // 'in_person' | 'telemedicine' | 'phone' | 'home_visit'
    $t->text('subjective')->nullable();
    $t->text('objective')->nullable();
    $t->text('assessment')->nullable();
    $t->text('plan')->nullable();
    $t->json('icd10_codes')->nullable();
    $t->json('rxnorm_codes')->nullable();
    $t->string('audio_s3_key')->nullable();
    $t->string('transcript_s3_key')->nullable();
    $t->boolean('ai_generated')->default(false);
    $t->boolean('doctor_signed')->default(false);
    $t->timestamp('signed_at')->nullable();
    $t->timestamps();
});
```

### 14.7 Vitals trend analysis (`ehr-lab-trends`, V2)

`patient_vitals` is append-only (V1). V2 adds a materialized view `patient_vitals_trends` refreshed every hour:

```sql
CREATE MATERIALIZED VIEW patient_vitals_trends AS
SELECT
    patient_id,
    DATE(recorded_at) AS day,
    vital_type,
    AVG(value_numeric) AS avg_value,
    MIN(value_numeric) AS min_value,
    MAX(value_numeric) AS max_value,
    COUNT(*) AS readings
FROM patient_vitals
WHERE value_numeric IS NOT NULL
GROUP BY patient_id, DATE(recorded_at), vital_type;

CREATE INDEX ON patient_vitals_trends (patient_id, vital_type, day);
```

Doctor dashboard shows 30-day, 90-day, 1-year rolling charts.

### 14.8 Patient app V2 features

- **Health timeline:** chronological view of visits, prescriptions, lab results, vitals.
- **Share my record:** generate a 7-day signed URL (already V1, extended in V2 to include entire chart).
- **Family overview:** switch between family members with one tap.
- **Vaccination reminders:** push notification when a vaccine is due.
- **Symptom check:** opens the AI symptom checker (V2.1).
- **Lab results explained:** AI plain-language explanation of a lab report (opt-in).

### 14.9 V2 API additions

```
GET    /api/v2/patients/{id}/problem-list                 -> [problem]
POST   /api/v2/patients/{id}/problem-list                 -> problem
PATCH  /api/v2/patients/{id}/problem-list/{pid}           -> problem
GET    /api/v2/patients/{id}/family-history               -> [history]
POST   /api/v2/patients/{id}/family-history               -> history
GET    /api/v2/patients/{id}/lab-results?from&to          -> [lab_result] (with abnormal flag)
GET    /api/v2/patients/{id}/lab-results/trend?test_code  -> [series]
GET    /api/v2/patients/{id}/vaccinations                 -> [vaccination]
POST   /api/v2/patients/{id}/vaccinations                 -> vaccination
GET    /api/v2/patients/{id}/surgical-history             -> [surgery]
GET    /api/v2/patients/{id}/encounters                   -> [encounter]
GET    /api/v2/patients/{id}/encounters/{eid}             -> encounter (with SOAP note)
POST   /api/v2/patients/{id}/encounters/{eid}/sign        -> { signed_at }
POST   /api/v2/patients/{id}/attachments                  -> attachment (multipart)
GET    /api/v2/patients/{id}/attachments                  -> [attachment]
GET    /api/v2/patients/{id}/attachments/{aid}/download   -> 302 to signed S3 URL
POST   /api/v2/patients/me/link-abha                      -> { abha_id } (starts linking)
POST   /api/v2/patients/me/link-abha/verify               -> { otp }  -> ok
POST   /api/v2/patients/me/link-abha/callback             -> { auth_code } from ABDM
DELETE /api/v2/patients/me/link-abha                      -> ok
POST   /api/v2/encounters/{id}/scribe/audio               -> multipart audio
GET    /api/v2/encounters/{id}/scribe/status              -> { status, draft? }
```

### 14.10 V2 acceptance criteria

- [ ] ABHA linking round-trips against NHA sandbox; profile syncs.
- [ ] Family booking creates a separate MRN for the dependent.
- [ ] AI scribe produces a SOAP note from a 10-minute sample audio; doctor edits and signs.
- [ ] Lab trend chart shows HbA1c over 6 months correctly.
- [ ] Vaccination reminder push fires 7 days before `next_due_at`.
- [ ] Patient can download a complete chart export (PDF) for second-opinion.
- [ ] GDPR delete anonymizes the patient in all tables.
- [ ] All V2 PHI reads logged in `data_access_log`.

---

## 17. V3+ Modules — From Patient Record to Full Digital Health Platform

> V1 stores demographics + appointments. V2 adds EHR (problem list, allergies, vitals, lab results, family history), ABHA sync, AI scribe. V3+ turns the patients module into a **complete digital health platform** with genomics, behavioral health, social determinants, full PROMs/care plans, wearable integration, family management, and population health.

### 17.1 Complete EHR (V3+)

The full longitudinal electronic health record, with deep structured and unstructured data.

**V2 had:** Problem list, allergies, vitals, lab results, family history, surgical history, vaccinations.

**V3+ adds:**

- **Genomics & Pharmacogenomics (`genomics`, V3)**
  ```php
  Schema::create('patient_genetic_variants', function (Blueprint $t) {
      $t->ulid('id')->primary();
      $t->ulid('tenant_id')->index();
      $t->ulid('patient_id')->index();
      $t->string('gene', 32);                  // 'CYP2C19' | 'TPMT' | 'DPYD' | 'SLCO1B1'
      $t->string('variant', 32);                // '*2' | '*3' | rs1234
      $t->string('zygosity', 16);               // 'homozygous' | 'heterozygous' | 'hemizygous'
      $t->string('clinical_significance', 32);  // 'pathogenic' | 'likely_pathogenic' | 'vus' | 'likely_benign' | 'benign'
      $t->string('source', 32);                 // '23andme' | 'color' | 'labcorp' | 'self_reported'
      $t->json('phenotype')->nullable();        // {trait: 'decreased_metabolism', drug: 'clopidogrel'}
      $t->date('reported_at');
      $t->foreignId('lab_result_id')->nullable();
      $t->timestamps();
  });

  Schema::create('pharmacogenomic_drug_alerts', function (Blueprint $t) {
      $t->ulid('id')->primary();
      $t->ulid('patient_id')->index();
      $t->string('drug_rxnorm', 16);            // 'clopidogrel' (RxNorm CUI)
      $t->string('alert_level', 16);            // 'critical' | 'major' | 'moderate' | 'minor'
      $t->text('recommendation');
      $t->json('cpic_guideline');
      $t->timestamps();
  });
  ```
  - Integration with `ClinVar`, `CPIC`, `PharmGKB`.
  - At prescription time, alert doctor to PGx-relevant drugs.
  - Test for: CYP2C19 → clopidogrel effectiveness, TPMT → mercaptopurine toxicity, DPYD → fluorouracil toxicity, SLCO1B1 → simvastatin myopathy, VKORC1 → warfarin dosing.

- **Behavioral Health & SDOH (V3)**
  ```php
  Schema::create('social_determinants', function (Blueprint $t) {
      $t->ulid('id')->primary();
      $t->ulid('tenant_id')->index();
      $t->ulid('patient_id')->index();
      $t->string('category', 32);              // 'housing' | 'food' | 'transportation' | 'financial' | 'social_isolation' | 'education' | 'employment' | 'safety'
      $t->string('z_code', 16)->nullable();     // ICD-10 Z55-Z65
      $t->string('status', 16);                // 'identified' | 'addressed' | 'resolved'
      $t->text('description');
      $t->text('intervention')->nullable();
      $t->ulid('referred_to_resource_id')->nullable();
      $t->date('assessed_at');
      $t->ulid('assessed_by_user_id');
      $t->timestamps();
  });

  Schema::create('behavioral_health_screenings', function (Blueprint $t) {
      $t->ulid('id')->primary();
      $t->ulid('patient_id')->index();
      $t->string('screening_type', 32);         // 'phq9' | 'gad7' | 'cssrs' | 'audit_c' | 'dast' | 'pcl5' | 'pss10'
      $t->json('responses');                    // question responses
      $t->integer('total_score');
      $t->string('severity', 16);               // 'minimal' | 'mild' | 'moderate' | 'severe'
      $t->boolean('suicidal_ideation_flagged')->default(false);
      $t->boolean('referred_to_mental_health')->default(false);
      $t->date('administered_at');
      $t->ulid('administered_by_user_id')->nullable();  // null if self-administered
      $t->timestamps();
  });
  ```
  - SDOH categories per CMS: housing instability, food insecurity, transportation, financial strain, social isolation, education, employment, safety.
  - Behavioral health screenings: PHQ-9 (depression), GAD-7 (anxiety), C-SSRS (suicide risk), AUDIT-C (alcohol), DAST (drugs), PCL-5 (PTSD), PSS-10 (stress).
  - Auto-flag for suicidal ideation → instant crisis alert.
  - Closed-loop referrals: track if patient connected to referred resource.

- **Reproductive & Pregnancy Health (V3)**
  ```php
  Schema::create('pregnancies', function (Blueprint $t) {
      $t->ulid('id')->primary();
      $t->ulid('patient_id')->index();
      $t->date('lmp_date')->nullable();         // Last menstrual period
      $t->date('edd_date')->nullable();         // Estimated due date
      $t->date('confirmed_at')->nullable();
      $t->decimal('gravida', 4, 0)->default(0); // number of pregnancies
      $t->decimal('para', 4, 0)->default(0);   // number of births
      $t->string('risk_level', 16);             // 'low' | 'medium' | 'high'
      $t->date('delivery_date')->nullable();
      $t->string('delivery_outcome', 32)->nullable();  // 'live_birth' | 'stillbirth' | 'miscarriage' | 'abortion'
      $t->string('delivery_method', 32)->nullable();  // 'vaginal' | 'cesarean' | 'vacuum' | 'forceps'
      $t->json('complications')->nullable();
      $t->timestamps();
  });
  ```

- **Pediatric Growth Charts (V3)**
  ```php
  Schema::create('growth_measurements', function (Blueprint $t) {
      $t->ulid('id')->primary();
      $t->ulid('patient_id')->index();
      $t->date('measurement_date');
      $t->decimal('age_months', 6, 2);
      $t->decimal('weight_kg', 5, 2)->nullable();
      $t->decimal('height_cm', 5, 2)->nullable();
      $t->decimal('head_circumference_cm', 5, 2)->nullable();
      $t->decimal('bmi', 5, 2)->nullable();
      $t->integer('weight_percentile')->nullable();
      $t->integer('height_percentile')->nullable();
      $t->integer('bmi_percentile')->nullable();
      $t->decimal('weight_zscore', 4, 2)->nullable();
      $t->string('source', 32);                 // 'visit' | 'parent_reported' | 'wearable'
      $t->timestamps();
  });
  ```
  - WHO Z-score growth charts (0-5y) and CDC growth charts (2-20y).
  - Flag: BMI > 95th percentile (obese), < 5th (underweight), weight z-score drop > 2 (failure to thrive).

- **Occupational Health (V3)**
  - Work history, exposures (asbestos, silica, chemicals), PPE use.
  - OSHA recordable injuries.
  - Surveillance exams (pulmonary function, audiometry, vision).

- **Travel Medicine (V3)**
  - Upcoming trips, destination-specific vaccines (Yellow fever, Typhoid, Japanese encephalitis).
  - Malaria prophylaxis prescriptions.
  - Traveler's diarrhea kit.
  - CDC travel health notices integration.

- **Implant & Device Registry (V3)**
  - Cardiac devices: pacemaker (model, lead placement), ICD, CRT-D, LVAD.
  - Orthopedic: joint replacements (implant UDI, date, surgeon).
  - Other: cochlear implant, insulin pump, neurostimulator.
  - Recalls (FDA MAUDE database) auto-monitored.

- **Imaging Deep Dive (V3)**
  - Full DICOM integration (see `19-INTEGRATIONS` §V3+).
  - Image comparison across time.
  - AI image analysis (lung nodule detection, mammogram screening).

### 17.2 Wearables & Remote Patient Monitoring (`wearables-rpm`, V3)

Continuous data from wearables feeds into the EHR for proactive care.

**Supported devices (30+):**
- **Apple Watch**: heart rate, HRV, ECG, SpO2, irregular rhythm, fall detection, activity, sleep, blood oxygen.
- **Fitbit**: heart rate, sleep, activity, SpO2 (Sense), ECG (Sense), stress, skin temperature (Sense 2).
- **Garmin**: heart rate, sleep, stress, body battery, fitness age.
- **Samsung Galaxy Watch**: heart rate, ECG, blood pressure, SpO2, sleep, body composition.
- **Oura Ring**: sleep, HRV, body temp, activity, readiness.
- **Whoop**: HRV, sleep, strain, recovery.
- **Polar**: heart rate, sleep, training load.
- **Withings**: BPM Core (blood pressure + ECG), Sleep Analyzer, Body+ (scale), Thermo (thermometer).
- **Omron**: blood pressure monitors.
- **Dexcom G6/G7**: continuous glucose monitor (CGM).
- **FreeStyle Libre 3**: CGM.
- **Medtronic Guardian**: CGM.
- **Eversense**: implantable CGM.
- **Abbott FreeStyle Libre**: CGM.
- **iHealth**: glucose meter, blood pressure, weight.
- **Accu-Chek**: glucose meter.
- **ResMed AirSense/AirCurve**: CPAP.
- **Philips DreamStation**: CPAP.
- **Inogen**: portable oxygen concentrator.
- **Spirometers**: MIR Spirodoc, NuvoAir.

**`WearableIntegrationService`**:
- OAuth flow to device manufacturer.
- Webhook receiver for data push.
- Scheduled polling for older APIs.
- Data normalization (different formats → FHIR Observation).
- Storage in `wearable_observations` table (high-volume, partitioned by month).

**`wearable_observations` table (partitioned monthly, 1B+ rows over 5 years):**
```php
Schema::create('wearable_observations', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id');
    $t->ulid('patient_id')->index();
    $t->ulid('device_id');
    $t->string('metric_type', 32);       // 'heart_rate' | 'systolic_bp' | 'glucose' | 'sleep_duration' | 'steps'
    $t->decimal('value', 10, 3)->nullable();
    $t->string('unit', 16);
    $t->dateTime('measured_at')->index();
    $t->json('context')->nullable();     // activity, position, meal context
    $t->boolean('is_abnormal')->default(false);
    $t->timestamps();
})->partitionBy('rangeColumns(measured_at)');
```

**`RpmThresholdService::evaluate($patient, $observation)`**:
- For CGM: hypo < 70 mg/dL → alert; hyper > 250 mg/dL → alert; trend (rapid rise/fall).
- For BP: systolic > 180 or < 90 → alert.
- For HR: bradycardia < 40, tachycardia > 130 (resting) → alert.
- For SpO2: < 92% sustained 30s → alert.
- For sleep: < 4 hours → check in.
- For activity: 0 steps for 7 days → outreach.

**Alerts trigger:**
- In-app notification (immediate).
- SMS to care team (if configured).
- Voice call to patient (if critical).
- Care plan task auto-created.

**RPM billing (CPT codes):**
- 99453: Initial setup (one-time).
- 99454: Device supply with daily recording (monthly).
- 99457: First 20 min/month of interactive communication.
- 99458: Additional 20 min/month.
- 99091: 30 min/month of data review.

### 17.3 Patient-Reported Outcome Measures (PROMs) (`proms`, V3)

PROMs are validated questionnaires that measure patient outcomes from the patient's perspective.

**60+ pre-built PROMs:**
- **Pain**: VAS (Visual Analog Scale), NRS (Numeric Rating), McGill Pain Questionnaire, Brief Pain Inventory, WOMAC (osteoarthritis), Oswestry Disability Index.
- **Function**: SF-36, SF-12, EQ-5D-5L, PROMIS-29, KOOS (knee), HOOS (hip), DASH (arm/shoulder).
- **Mental health**: PHQ-9, GAD-7, BDI-II, BAI, EPDS (postpartum), PCL-5, MDQ (bipolar).
- **Disease-specific**: A1c self-report, CD activity index (Crohn's), PASI (psoriasis), DAS-28 (RA), Eczema Area Severity Index.
- **Quality of life**: FACT-G (cancer), EORTC QLQ-C30, KIDSCREEN (pediatric), WHO-5 Well-Being.
- **Sexual health**: FSFI, IIEF, Female Sexual Function, ASEX.
- **Sleep**: PSQI, ISI, ESS, STOP-BANG.
- **Substance use**: AUDIT, DAST, Fagerstrom (nicotine).
- **Adherence**: MARS, MMAS-8 (Morisky).

**`PromService::deliver($patient, $prom, $context)`**:
1. Patient receives notification (in-app + email + SMS).
2. Opens the survey in the app (web or mobile).
3. Answers questions (multi-choice, slider, free text).
4. Score auto-calculated per validated algorithm.
5. Results stored in `prom_responses`.
6. Trending shown to patient (improving/stable/worsening).
7. Doctor gets alert if score worsens beyond threshold.
8. Patient gets encouragement if improving.

**`prom_responses`:**
```php
Schema::create('prom_responses', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id');
    $t->ulid('patient_id')->index();
    $t->ulid('prom_id')->index();
    $t->json('responses');
    $t->decimal('total_score', 6, 2);
    $t->json('subscale_scores')->nullable();
    $t->string('severity', 32)->nullable();
    $t->boolean('is_baseline')->default(false);
    $t->decimal('change_from_baseline', 6, 2)->nullable();
    $t->string('trend', 16)->nullable();         // 'improving' | 'stable' | 'worsening'
    $t->dateTime('administered_at');
    $t->dateTime('completed_at');
    $t->string('channel', 16);                    // 'in_app' | 'phone' | 'in_clinic' | 'email'
    $t->timestamps();
    $t->index(['patient_id', 'prom_id', 'administered_at']);
});
```

**Clinical decision support:**
- PHQ-9 score 15+ → moderate-severe depression → suggest therapy referral.
- WOMAC pain subscale increased > 2 points → consider intervention.
- A1c self-report > 9% → schedule CDE visit.
- EPDS score 13+ → postpartum depression → urgent referral.

### 17.4 Care Plans (`care-plans`, V3, deep)

A care plan is a structured set of goals, interventions, and tasks for a patient over time.

**`care_plans`:**
```php
Schema::create('care_plans', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id');
    $t->ulid('patient_id')->index();
    $t->ulid('author_user_id');                 // care team member who created
    $t->string('title', 128);                   // 'Diabetes Management', 'Post-MI Recovery', 'Pregnancy Care'
    $t->text('description')->nullable();
    $t->string('category', 32);                 // 'chronic_disease' | 'post_acute' | 'preventive' | 'maternal' | 'behavioral' | 'palliative'
    $t->string('status', 16);                   // 'draft' | 'active' | 'on_hold' | 'completed' | 'cancelled'
    $t->date('start_date');
    $t->date('end_date')->nullable();
    $t->date('review_date')->nullable();
    $t->json('goals')->nullable();              // [{description, target_date, status, measure}]
    $t->json('interventions')->nullable();      // [{type, description, frequency, owner}]
    $t->json('tasks')->nullable();              // [{description, due_date, assignee, status}]
    $t->timestamps();
});

Schema::create('care_plan_tasks', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('care_plan_id')->index();
    $t->ulid('patient_id')->index();
    $t->string('title', 128);
    $t->text('description')->nullable();
    $t->string('category', 32);
    $t->date('due_date');
    $t->string('recurrence', 32)->nullable();   // 'daily' | 'weekly' | 'monthly' | 'as_needed'
    $t->string('assignee_type', 16);            // 'patient' | 'care_team' | 'family'
    $t->ulid('assignee_id')->nullable();
    $t->string('status', 16)->default('pending'); // 'pending' | 'in_progress' | 'completed' | 'skipped' | 'overdue'
    $t->dateTime('completed_at')->nullable();
    $t->text('notes')->nullable();
    $t->timestamps();
});

Schema::create('care_plan_goals', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('care_plan_id')->index();
    $t->string('description', 255);
    $t->string('metric_type', 32);              // 'numeric' | 'binary' | 'qualitative'
    $t->decimal('target_value', 10, 2)->nullable();
    $t->string('target_unit', 16)->nullable();
    $t->date('target_date');
    $t->decimal('current_value', 10, 2)->nullable();
    $t->dateTime('last_measured_at')->nullable();
    $t->string('status', 16)->default('on_track');  // 'on_track' | 'at_risk' | 'off_track' | 'achieved' | 'abandoned'
    $t->timestamps();
});
```

**Templates by condition:**
- **Diabetes**: A1c target, BP target, LDL target, daily glucose checks, foot exam, eye exam, kidney check.
- **Hypertension**: BP target, sodium restriction, exercise, weight, follow-up cadence.
- **Post-MI**: dual antiplatelet, beta-blocker, ACE/ARB, statin, cardiac rehab, smoking cessation, weight.
- **Pregnancy**: prenatal visits, ultrasound schedule, glucose screen, GBS screen, vaccines.
- **CHF**: weight monitoring, daily weights, diuretic titration, sodium restriction, follow-up.
- **COPD**: pulmonary rehab, smoking cessation, vaccinations (flu, pneumococcal), inhaler adherence.
- **Chronic kidney disease**: BP control, diabetes control, anemia check, nephrology referral, dialysis planning.
- **Cancer (active treatment)**: chemo cycle schedule, lab checks, imaging, side effect management.
- **Post-op**: wound care, activity restrictions, follow-up cadence, red flag symptoms.
- **Weight loss**: caloric target, exercise target, weekly weigh-in, behavioral check-in.
- **Smoking cessation**: nicotine replacement, counseling, quit date, milestones.
- **Maternal postpartum**: depression screen (EPDS), breastfeeding support, contraception, infant vaccines.
- **Behavioral health**: medication adherence, therapy sessions, safety plan, support system.

**Task generation & tracking:**
- Tasks auto-generated from templates.
- Patient can mark complete in app.
- Care team gets dashboard of overdue tasks.
- Missed task → escalation (call, message, red flag).
- Trend over time: adherence rate per patient, per care plan.

**Goal tracking:**
- A1c measured → updates goal `current_value`.
- Status auto-updated: on_track (within 80% of target by date), at_risk, off_track.
- At-risk: outreach to patient. Off-track: provider alert.
- Goal achieved: celebration notification.

**Care plan review:**
- Periodic review (default 90 days).
- At review: provider assesses goals, updates interventions, extends/closes plan.
- Audit-logged.

### 17.5 Family & Caregiver Management (`family-caregivers`, V3)

Manage relationships between patients (family members, caregivers, guardians).

**`family_relationships`:**
```php
Schema::create('family_relationships', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id');
    $t->ulid('primary_patient_id')->index();   // the index patient
    $t->ulid('related_patient_id')->nullable()->index();  // the related person (if also a patient)
    $t->ulid('related_user_id')->nullable()->index();     // if not a patient
    $t->string('relationship_type', 32);       // 'self' | 'spouse' | 'child' | 'parent' | 'sibling' | 'guardian' | 'caregiver' | 'power_of_attorney'
    $t->boolean('is_emergency_contact')->default(false);
    $t->boolean('is_healthcare_proxy')->default(false);
    $t->boolean('can_access_records')->default(false);
    $t->boolean('can_book_appointments')->default(false);
    $t->boolean('can_make_payments')->default(false);
    $t->boolean('can_receive_notifications')->default(false);
    $t->json('consent')->nullable();           // scope of consent
    $t->timestamps();
});
```

**Use cases:**
- Mother books appointment for child → relationship allows.
- Spouse views lab results of partner (with consent).
- Caregiver gets prescription refill notifications.
- Power of attorney can authorize procedures.
- Grandparent's emergency contact gets critical alerts.
- Family history auto-aggregated from relationships.

**Proxy access (HIPAA-compliant):**
- Patient (or authorized user) grants proxy access.
- Proxy can view: appointments, prescriptions, lab results, billing.
- Proxy cannot: delete account, change demographics (unless power of attorney).
- Audit log: every proxy access logged with proxy user ID.

**Pediatric mode:**
- Patient is child (under 18).
- Parent/guardian is the primary user.
- All actions taken via parent's account.
- At age 18, transfer to adult account (with consent).

### 17.6 Patient Portal & Engagement (`patient-portal`, V3)

A full patient portal — desktop + mobile + tablet.

**Features:**
- Dashboard: upcoming appointments, recent results, active medications, care plan progress.
- Appointments: book, reschedule, cancel, join video visit.
- Prescriptions: view, request refill, transfer.
- Lab results: view, trend, download.
- Billing: view, pay, set up payment plan, view statements.
- Documents: upload insurance card, ID, prior records.
- Messaging: chat with care team.
- Care plans: view, complete tasks, track goals.
- Health metrics: log weight, BP, glucose.
- Wearables: connect, view data.
- Family: manage relationships, view family records.
- Profile: update demographics, insurance, preferences.
- Security: 2FA, sessions, login history.

**Patient-facing AI features:**
- **Symptom checker**: pre-visit, helps patient decide if visit needed.
- **Medication reminder**: scheduled push.
- **Health insights**: "Your average BP this month is 135/85, slightly elevated."
- **Risk predictions**: "Based on your data, you may be at risk for X. Talk to your doctor."
- **Care gap alerts**: "You're due for a flu shot. Schedule here."
- **Personalized education**: condition-specific articles, videos.

### 17.7 Population Health (`population-health`, V5, MML 1)

**1. Overview.** V1–V3 of the platform is per-patient: it answers "what is this patient's A1c?" V5 answers "what fraction of my diabetic panel has A1c > 9%?". This is the **cohort tool** that makes the platform useful for value-based care, ACO contracts, payer negotiations, and public health reporting. MML 1 at V5 GA means the data model, cohort DSL, and de-identification pipeline are designed and partially built; full GA (MML 5) trails by ~6 months.

**2. Domain Model.**
```php
Schema::create('cohort_definitions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 128);
    $t->text('description')->nullable();
    $t->json('criteria_dsl');                              // {type: 'and', clauses: [...]}
    $t->string('version', 16)->default('1.0.0');           // immutable history
    $t->ulid('previous_version_id')->nullable();
    $t->boolean('is_active')->default(true);
    $t->ulid('created_by_user_id');
    $t->timestamps();
});

Schema::create('cohort_runs', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('cohort_definition_id')->index();
    $t->string('cohort_definition_version', 16);
    $t->timestamp('started_at')->index();
    $t->timestamp('finished_at')->nullable();
    $t->integer('patient_count')->nullable();
    $t->string('status', 16)->default('running');          // 'running' | 'completed' | 'failed'
    $t->text('error')->nullable();
    $t->json('performance_stats')->nullable();             // {sql_duration_ms, scan_rows}
    $t->timestamps();
});

Schema::create('cohort_memberships', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('cohort_run_id')->index();
    $t->ulid('patient_id')->index();
    $t->json('matched_clauses');                           // which DSL clauses matched
    $t->timestamp('included_at')->index();
});

Schema::create('quality_measure_runs', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('measure_code', 32);                        // 'HEDIS-CDC', 'MIPS-Q001'
    $t->string('measure_set', 16);                         // 'HEDIS' | 'MIPS' | 'MACRA' | 'CUSTOM'
    $t->string('reporting_year', 4);
    $t->timestamp('run_at')->index();
    $t->integer('denominator');
    $t->integer('numerator');
    $t->decimal('rate', 6, 4);                             // 0.0000–1.0000
    $t->json('gap_list')->nullable();                      // patients who should have but didn't
    $t->timestamps();
});
```

**3. Cohort DSL (designed in MML 1, target GA MML 5):**
The DSL is a JSON tree, not SQL. Each leaf is a condition; internal nodes are `and`/`or`/`not`. Example for "Diabetic, A1c > 9, no endo visit in 6 months":
```json
{
  "type": "and",
  "clauses": [
    {"type": "has_diagnosis", "icd10": "E11.*"},
    {"type": "lab_value", "loinc": "4548-4", "comparator": ">", "value": 9.0, "within_days": 365},
    {"type": "no_visit_with_specialty", "specialty": "endocrinology", "within_days": 180}
  ]
}
```
The DSL is compiled to a parameterized SQL query at runtime; SQL is logged to `cohort_runs.performance_stats` for tuning. Patient identifiers are returned only to authorized users; the de-identified variant is used for V5 cross-tenant research-cohort-tooling (§24-ROADMAP §19.7).

**4. API Surface (MML 1 stub → MML 5 GA):**
```
GET    /api/v5/population/cohorts
POST   /api/v5/population/cohorts                          # create from DSL JSON
PATCH  /api/v5/population/cohorts/{id}                    # new version
POST   /api/v5/population/cohorts/{id}/run                # ad-hoc run
POST   /api/v5/population/cohorts/{id}/schedule           # cron schedule
GET    /api/v5/population/cohorts/{id}/runs
GET    /api/v5/population/cohorts/{id}/runs/{run_id}/patients
GET    /api/v5/population/measures                        # HEDIS/MIPS runs
POST   /api/v5/population/measures/run                    # compute a measure
GET    /api/v5/population/measures/{id}/gap-list
POST   /api/v5/population/outreach                        # trigger patient outreach for a cohort
GET    /api/v5/population/outcomes?cohort=...             # longitudinal metric
```

**5. Frontend.** `/admin/population/cohorts` is a drag-and-drop DSL builder with a visual canvas. Each leaf is a card; the user drags cards into a tree. Below the canvas: a "test run" button that returns the first 50 matched patients (de-identified by default; toggle "show identifiable" requires a re-auth step). `/admin/population/measures` shows the HEDIS/MIPS dashboard with green/yellow/red gauges.

**6. Realtime.**
- `cohort.run.completed.{tenant}.{cohort_id}` — webhook + UI toast.
- `measure.rate.computed.{tenant}.{measure_code}` — pushed to the dashboard every time a new measure run finishes.
- `cohort.outreach.dispatched.{tenant}.{cohort_run_id}` — fires when an outreach campaign is sent.

**7. Security & RBAC.** Roles: `population_analyst` (build cohorts, view identifiable lists), `care_manager` (view + outreach, no DSL editing), `quality_lead` (run measures, export gap lists), `tenant_admin` (approve new cohort versions, audit DSL changes). Cohort DSL changes are versioned immutably; every edit creates a new version row. Identifiable patient lists are masked on screen and cannot be copy-pasted (DRM-style). All runs are audit-logged with `action='cohort.run'` and the row count; this is a HIPAA audit trigger. Cross-tenant research use is gated by §19.7 of `24-ROADMAP` and §23.3.2 of `20-Security` (homomorphic encryption + Safe Harbor de-identification, k-anonymity ≥ 5).

**8. Acceptance Criteria (V5 GA MML 5):**
- [ ] A cohort with 1M patients and 8 clauses runs in <60s.
- [ ] The DSL rejects SQL injection attempts and unknown clause types.
- [ ] HEDIS CDC (comprehensive diabetes care) measure returns the same numerator as the manual chart audit for a 100-patient sample (κ ≥ 0.95).
- [ ] A cohort run that touches a patient outside the tenant's `tenant_id` is hard-rejected.
- [ ] A de-identified export for cross-tenant research passes k-anonymity ≥ 5 (no row with fewer than 5 patients per quasi-identifier combination).
- [ ] Cohort definitions are immutable; corrections require a new version, not in-place edit.

**9. Real-World Test Scenarios.**
- *Diabetic A1c > 9 cohort:* ACO with 12,000 attributed lives; cohort finds 1,340 patients; care manager triggers SMS outreach to half; 3-month re-run shows 28% improvement to A1c ≤ 9.
- *HEDIS breast cancer screening:* 50,000-patient tenant; measure run completes in 22s; gap list of 4,200 women is exported to a navigators' CSV; navigator calls each one.
- *MIPS submission:* 6 measure runs are batched, packaged in the CMS QPP JSON format, and submitted to qpp.cms.gov.
- *Cross-tenant research (V5 MML 5):* a researcher requests "all patients with stage III colon cancer who received adjuvant chemo within 8 weeks of surgery, 2020–2024"; the system returns 12,400 patients across 18 tenants, de-identified, with k-anonymity ≥ 5 enforced.

**10. What this module is NOT:**
- Not a research data lake. That is V5+ `research-cohort-tooling` (§24-ROADMAP §19.7), which is de-identified and cross-tenant.
- Not a public health reporting system. That is V5+ `public-health-reporting` (`13`, `19`) for state/federal disease registries and CDC PHINMS.
- Not a free-form SQL console. The DSL is the only query interface; raw SQL is never exposed.

### 17.8 Health Information Exchange (HIE) (`hie`, V3)

Connect to regional/national HIE networks for interoperable records.

**Networks:**
- **US**: Carequality, CommonWell Health Alliance, eHealth Exchange, regional HIEs.
- **India**: ABDM (Ayushman Bharat Digital Mission), HIE-CM (Health Information Exchange & Consent Manager).
- **UK**: NHS Spine, GP Connect.
- **EU**: eHealth Digital Service Infrastructure (eHDSI), MyHealth@EU.
- **Canada**: Alberta Netcare, Ontario's ClinicalConnect.
- **Australia**: My Health Record.

**Capabilities:**
- Patient lookup (MPI - Master Patient Index).
- Document retrieval (C-CDA, FHIR DocumentReference).
- Document submission.
- Consent management per HIE.
- IHE profiles: XCA (Cross-Community Access), XCP (Cross-Community Patient Discovery), XDR (Document Repository).

**FHIR-based HIE gateway** (Java/Spring, see `00-ARCHITECTURE` V2 service decomposition).

### 17.9 Telehealth (V3, deep)

Already in V2 for video visits. V3 makes telehealth first-class.

**Capabilities:**
- **Video visits**: LiveKit (preferred), Daily.co, Jitsi fallback.
- **Phone visits**: Twilio voice.
- **Chat visits**: async chat with care team.
- **Asynchronous visits**: patient submits question + photo/video, doctor responds within 24h.
- **Store-and-forward**: patient uploads (image, document), doctor reviews later (e.g., dermatology, radiology second opinion).
- **Remote patient monitoring**: continuous data, periodic check-ins.
- **Virtual triage**: symptom checker → right level of care.
- **e-Consults**: specialist consult via EMR, without patient visit.
- **Group visits**: 5-10 patients with same condition meet together (coaching, education, peer support).

**`TelemedicineService::startVisit($appointment, $modality)`**:
- Generates LiveKit room.
- Issues tokens for patient and provider.
- Starts recording (with consent).
- Begins transcription.
- AI scribe generates SOAP note (V2/V3 AI).
- On visit end: recording, transcript, SOAP note saved.

**`TelemedicineService::endVisit($appointment)`**:
- Stop recording.
- Generate patient-friendly summary.
- Trigger billing (CPT 99421-99423 for online digital E/M, 99213-99215 for video, etc.).
- Send follow-up care plan, prescription, lab orders.

**Integration with EHR**: every telemedicine visit = one encounter, with full documentation.

### 17.10 Clinical Decision Support (CDS) (`cds`, V3)

Real-time alerts and recommendations at the point of care.

**Alert types:**
- **Drug-allergy**: prescribing a drug the patient is allergic to.
- **Drug-drug**: known interaction.
- **Drug-condition**: contraindicated by condition.
- **Drug-dose**: out of range for age/weight/kidney function.
- **Duplicate therapy**: two drugs of same class.
- **Guideline-based**: "Patient with HF and EF<40%, not on beta-blocker. Consider initiating."
- **Preventive care**: due for screening/vaccine.
- **Care gap**: open problem without treatment.
- **Lab critical**: critical value (K+ 6.5).
- **Sepsis screening**: SIRS criteria + qSOFA → page rapid response.
- **Falls risk**: positive falls screen.
- **Suicide risk**: positive PHQ-9 item 9 → safety plan.

**`CdsRulesEngine::evaluate($context)`**:
- Real-time at order entry, at chart open, at vital sign entry.
- Returns list of alerts with severity (info, warn, critical).
- Critical: hard stop (requires override with reason).
- Warn: confirm with reason.
- Info: just shown, no action.

**Per-rule configuration:**
- Toggle on/off per tenant.
- Severity override (e.g., suppress certain drug-allergy in chemo context).
- Override requires reason, audit-logged.

### 17.11 V3+ API Additions (Selected Highlights)

```
GET    /api/v3/patients/{id}/genomics
POST   /api/v3/patients/{id}/genomics
GET    /api/v3/patients/{id}/pgx/alerts
GET    /api/v3/patients/{id}/sdoh
POST   /api/v3/patients/{id}/sdoh
GET    /api/v3/patients/{id}/screenings
POST   /api/v3/patients/{id}/screenings
GET    /api/v3/patients/{id}/pregnancies
POST   /api/v3/patients/{id}/pregnancies
GET    /api/v3/patients/{id}/growth
POST   /api/v3/patients/{id}/growth
GET    /api/v3/patients/{id}/devices
POST   /api/v3/patients/{id}/devices
GET    /api/v3/patients/{id}/devices/{device_id}/sync
GET    /api/v3/patients/{id}/observations
POST   /api/v3/patients/{id}/observations
GET    /api/v3/patients/{id}/observations/{metric}/trend
GET    /api/v3/patients/{id}/proms/templates
POST   /api/v3/patients/{id}/proms/deliver
GET    /api/v3/patients/{id}/proms/responses
GET    /api/v3/patients/{id}/proms/{prom_id}/trend
GET    /api/v3/patients/{id}/care-plans
POST   /api/v3/patients/{id}/care-plans
GET    /api/v3/patients/{id}/care-plans/{id}
PATCH  /api/v3/patients/{id}/care-plans/{id}
GET    /api/v3/patients/{id}/care-plans/{id}/tasks
POST   /api/v3/patients/{id}/care-plans/{id}/tasks
PATCH  /api/v3/patients/{id}/care-plans/{id}/tasks/{task_id}
GET    /api/v3/patients/{id}/care-plans/{id}/goals
PATCH  /api/v3/patients/{id}/care-plans/{id}/goals/{goal_id}
GET    /api/v3/patients/{id}/family
POST   /api/v3/patients/{id}/family
PATCH  /api/v3/patients/{id}/family/{id}
GET    /api/v3/population/cohorts
POST   /api/v3/population/cohorts
GET    /api/v3/population/cohorts/{id}/patients
GET    /api/v3/population/measures
GET    /api/v3/population/measures/{id}
GET    /api/v3/population/measures/{id}/patients
POST   /api/v3/hie/lookup
POST   /api/v3/hie/fetch
POST   /api/v3/hie/submit
GET    /api/v3/telemedicine/visits/{id}
POST   /api/v3/telemedicine/visits/{id}/start
POST   /api/v3/telemedicine/visits/{id}/end
GET    /api/v3/cds/alerts
POST   /api/v3/cds/evaluate
GET    /api/v3/cds/rules
PATCH  /api/v3/cds/rules/{id}
```

---

## 18. V3+ Test Matrix — Patients (Summary)

> Total: **~580 tests** across all levels. Same pyramid as other modules.

### 18.1 Unit Tests (~280)
- **EHR modules** (50 tests): genomics, PGx, SDOH, behavioral, reproductive, pediatric, occupational, travel, implants, imaging.
- **Wearables** (30 tests): device integration, data normalization, threshold evaluation, alerts, RPM billing codes.
- **PROMs** (25 tests): delivery, scoring, trending, alerts, 60+ templates.
- **Care plans** (30 tests): template instantiation, task generation, goal tracking, status updates, review workflow.
- **Family/caregivers** (20 tests): relationships, proxy access, pediatric mode, consent.
- **Population health** (25 tests): cohort definitions, measures, risk stratification, HEDIS/MIPS.
- **HIE** (15 tests): patient lookup, document retrieval, consent, IHE profiles.
- **Telemedicine** (20 tests): room creation, recording, transcription, billing, async visits.
- **CDS** (25 tests): drug-allergy, drug-drug, drug-condition, drug-dose, guideline, sepsis, falls, suicide.
- **Patient portal** (15 tests): dashboard, preferences, documents, security.
- **Patient-facing AI** (15 tests): symptom checker, insights, risk prediction, care gaps.
- **Other services** (10 tests): DICOM, lab trends, family history, allergies.

### 18.2 Integration Tests (~100)
- **Database (25)**: genomics, wearables (partitioned), PROMs, care plans, family, cohorts, HIE, telemedicine, CDS, etc.
- **Queue/cache (15)**: wearable sync, PROM reminder, care plan review, cohort refresh, HIE sync, etc.
- **Third-party (40)**: Apple HealthKit, Google Fit, Fitbit, Garmin, Withings, Oura, Dexcom, Libre, Omron, ResMed, MyChart, ABDM, HIE networks, LiveKit, Whisper, OpenAI, ClinVar, CPIC, PharmGKB.
- **E2E flows (20)**: diabetic with CGM, hypertensive with cuff, pregnant with growth, post-MI care plan, family with proxy, suicidal flag, drug allergy, sepsis alert.

### 18.3 HTTP / API Tests (~120)
- All V3 endpoints (genomics, SDOH, behavioral, reproductive, pediatric, devices, observations, PROMs, care plans, family, population, HIE, telemedicine, CDS).
- RBAC, tenant scope, validation, error paths.

### 18.4 E2E Tests (~25, Playwright)
- Patient onboards + connects wearable.
- Diabetic reviews glucose trends.
- Hypertensive logs BP.
- Pregnant tracks pregnancy.
- Care plan created → tasks generated → patient completes.
- Family proxy accesses child record.
- Cohort built → patients identified → outreach triggered.
- Telemedicine visit started → completed → billed.
- CDS alert at order entry.

### 18.5 Performance (~10, k6)
- Genomics query → p95 < 200ms.
- Wearable sync (1M observations/day) → p95 < 1s.
- PROMs delivery → p95 < 200ms.
- Care plan task generation (1000 patients) → p95 < 30s.
- Cohort query (100k patients) → p95 < 1s.
- HIE document fetch → p95 < 2s.
- Telemedicine start → p95 < 1s.
- CDS evaluate (1000 rules) → p95 < 300ms.

### 18.6 Security (~20, OWASP)
- SQL injection, XSS, CSRF, auth bypass, tenant isolation, IDOR, mass assignment, privilege escalation, integer overflow, replay attack, PHI in logs, audit log integrity.

### 18.7 Compatibility Matrix
- PHP 8.2/8.3/8.4, Laravel 11/12, MySQL 8/MariaDB/PostgreSQL 16, iOS 16+/Android 10+, Flutter 3.24/3.27, FHIR R4/R5, IHE profiles.

### 18.8 Real-World (~30, with real services in test mode)
- Apple HealthKit test, Google Fit test, Fitbit test, Garmin test, Withings test, Oura test, Dexcom Share test, Libre test, Omron test, ABDM sandbox, HIE sandbox, LiveKit test, Whisper test, OpenAI test, ClinVar test, CPIC test, etc.

---

## 19. V3+ Deep Dives — Patients

### 19.1 Deep Dive: Pharmacogenomic Drug Alerting

PGx (pharmacogenomics) integration: at prescription time, alert based on patient's genetic variants.

```php
namespace App\Modules\Patients\Services;

use App\Modules\Patients\Models\Patient;
use App\Modules\Patients\Models\PatientGeneticVariant;
use App\Modules\Prescriptions\Models\Prescription;

class PgxAlertService
{
    public function __construct(
        private readonly CpicGuidelineRepository $cpicRepo,
    ) {}

    public function evaluate(Patient $patient, string $drugRxNorm): ?PgxAlert
    {
        // 1. Get patient's variants
        $variants = $patient->geneticVariants()
            ->whereIn('gene', ['CYP2C19', 'CYP2D6', 'TPMT', 'DPYD', 'SLCO1B1', 'VKORC1', 'CYP2C9', 'HLA-B', 'HLA-A'])
            ->get();

        if ($variants->isEmpty()) return null;

        // 2. Look up CPIC guidelines for this drug
        $guidelines = $this->cpicRepo->getByDrug($drugRxNorm);
        if ($guidelines->isEmpty()) return null;

        // 3. For each guideline, check if patient has a relevant variant
        foreach ($guidelines as $guideline) {
            foreach ($variants as $variant) {
                if ($variant->gene === $guideline->gene) {
                    $alert = $this->buildAlert($patient, $drugRxNorm, $variant, $guideline);
                    if ($alert) return $alert;
                }
            }
        }

        return null;
    }

    private function buildAlert(Patient $patient, string $drug, $variant, $guideline): ?PgxAlert
    {
        // CPIC levels: A (strong), B (moderate), C (optional), D (no recommendation)
        if (!in_array($guideline->cpic_level, ['A', 'B'])) return null;

        $phenotype = $this->lookupPhenotype($variant, $guideline);
        if (!$phenotype) return null;

        return new PgxAlert(
            patient: $patient,
            drug: $drug,
            alertLevel: $guideline->alert_level,
            recommendation: $this->formatRecommendation($guideline, $phenotype, $drug),
            cpicGuideline: $guideline->toArray(),
        );
    }

    private function formatRecommendation($guideline, $phenotype, $drug): string
    {
        return match($guideline->alert_level) {
            'critical' => "⚠️ CRITICAL: Patient has {$phenotype}. {$drug} is contraindicated. Consider alternative: {$guideline->alternative_drug}.",
            'major' => "MAJOR: Patient has {$phenotype}. {$drug} may be ineffective or cause adverse effects. Recommend: {$guideline->recommendation}.",
            'moderate' => "MODERATE: Patient has {$phenotype}. Consider dose adjustment: {$guideline->dose_adjustment}.",
            default => null,
        };
    }
}
```

**Common PGx alerts:**

| Drug | Gene | Phenotype | Recommendation |
|---|---|---|---|
| Clopidogrel | CYP2C19 | Poor metabolizer | Use prasugrel or ticagrelor instead |
| Codeine | CYP2D6 | Ultra-rapid metabolizer | Avoid; use non-tramadol opioid |
| Warfarin | VKORC1/CYP2C9 | Sensitive | Reduce dose 30-50% |
| Simvastatin | SLCO1B1 | Poor transporter | Limit to 20mg/day |
| Fluorouracil | DPYD | Deficient | Avoid; use alternative |
| Mercaptopurine | TPMT | Deficient | Reduce dose to 10% |
| Abacavir | HLA-B*57:01 | Positive | Avoid; hypersensitivity risk |
| Carbamazepine | HLA-B*15:02 | Positive (Asian) | Avoid; SJS risk |
| Allopurinol | HLA-B*58:01 | Positive | Avoid; SJS risk |

**Test scenarios:**

| Test | Patient Variant | Drug | Expected Alert |
|---|---|---|---|
| 1 | CYP2C19 *2/*2 (poor) | Clopidogrel | CRITICAL: use prasugrel |
| 2 | CYP2C19 *1/*1 (normal) | Clopidogrel | No alert |
| 3 | TPMT *3A/*3A (deficient) | Mercaptopurine | CRITICAL: 10% dose |
| 4 | DPYD *2A/*2A (deficient) | Fluorouracil | CRITICAL: avoid |
| 5 | SLCO1B1 *5/*5 (poor) | Simvastatin | MAJOR: 20mg max |
| 6 | No variants | Any | No alert |

### 19.2 Deep Dive: Wearable Data Ingestion

Ingesting 1M+ observations/day from various wearable devices.

```php
namespace App\Modules\Patients\Wearable;

use App\Modules\Patients\Models\Patient;
use App\Modules\Patients\Models\WearableDevice;
use App\Modules\Patients\Models\WearableObservation;

class WearableIngestionService
{
    public function __construct(
        private readonly WearableProviderResolver $providers,
        private readonly FhirObservationNormalizer $normalizer,
        private readonly RpmThresholdService $thresholds,
        private readonly AlertDispatcher $alerts,
    ) {}

    public function ingestFromProvider(WearableDevice $device, \DateTimeInterface $since): IngestionResult
    {
        $provider = $this->providers->for($device->provider);
        $rawData = $provider->fetchObservations($device, $since);
        $ingested = 0;
        $alerts = 0;

        foreach ($rawData as $raw) {
            $normalized = $this->normalizer->normalize($raw, $device);
            $obs = WearableObservation::create([
                'tenant_id' => $device->tenant_id,
                'patient_id' => $device->patient_id,
                'device_id' => $device->id,
                'metric_type' => $normalized->metricType,
                'value' => $normalized->value,
                'unit' => $normalized->unit,
                'measured_at' => $normalized->measuredAt,
                'context' => $normalized->context,
            ]);
            $ingested++;

            // Threshold check
            $alert = $this->thresholds->evaluate($device->patient, $obs);
            if ($alert) {
                $this->alerts->dispatch($alert);
                $alerts++;
            }
        }

        $device->update(['last_synced_at' => now()]);

        return new IngestionResult(
            deviceId: $device->id,
            ingested: $ingested,
            alerts: $alerts,
        );
    }
}
```

**FHIR mapping for wearable data:**

| Metric Type | LOINC Code | FHIR Code |
|---|---|---|
| Heart rate | 8867-4 | `http://loinc.org\|8867-4` |
| Systolic BP | 8480-6 | `http://loinc.org\|8480-6` |
| Diastolic BP | 8462-4 | `http://loinc.org\|8462-4` |
| Body weight | 29463-7 | `http://loinc.org\|29463-7` |
| Body height | 8302-2 | `http://loinc.org\|8302-2` |
| BMI | 39156-5 | `http://loinc.org\|39156-5` |
| Glucose | 2339-0 | `http://loinc.org\|2339-0` |
| SpO2 | 2708-6 | `http://loinc.org\|2708-6` |
| Steps | 55423-8 | `http://loinc.org\|55423-8` |
| Sleep duration | 93832-4 | `http://loinc.org\|93832-4` |
| Body temperature | 8310-5 | `http://loinc.org\|8310-5` |
| Respiratory rate | 9279-1 | `http://loinc.org\|9279-1` |
| HRV (RMSSD) | 80404-7 | `http://loinc.org\|80404-7` |

**Threshold evaluation:**

```php
class RpmThresholdService
{
    public function evaluate(Patient $patient, WearableObservation $obs): ?RpmAlert
    {
        return match($obs->metric_type) {
            'glucose' => $this->evalGlucose($patient, $obs),
            'systolic_bp' => $this->evalSystolicBp($patient, $obs),
            'heart_rate' => $this->evalHeartRate($patient, $obs),
            'spo2' => $this->evalSpO2($patient, $obs),
            default => null,
        };
    }

    private function evalGlucose(Patient $patient, WearableObservation $obs): ?RpmAlert
    {
        $value = (float)$obs->value;
        if ($value < 54) {
            return new RpmAlert('critical', 'Severe hypoglycemia', "Glucose {$value} mg/dL. Patient may be in crisis. Contact immediately.");
        }
        if ($value < 70) {
            return new RpmAlert('major', 'Hypoglycemia', "Glucose {$value} mg/dL. Patient should consume fast-acting carbs.");
        }
        if ($value > 250) {
            return new RpmAlert('major', 'Hyperglycemia', "Glucose {$value} mg/dL. Check ketones if T1DM.");
        }
        if ($value > 400) {
            return new RpmAlert('critical', 'Severe hyperglycemia', "Glucose {$value} mg/dL. DKA risk. Contact immediately.");
        }
        return null;
    }
}
```

### 19.3 Deep Dive: AI-Powered Care Plan Generation

Generate a care plan from a patient encounter using AI.

```php
namespace App\Modules\Patients\CarePlan;

use App\Modules\Patients\Models\Patient;
use App\Modules\Encounters\Models\Encounter;

class AiCarePlanService
{
    public function __construct(
        private readonly LlmProviderResolver $llm,
    ) {}

    public function generate(Patient $patient, Encounter $encounter, string $condition): CarePlanDraft
    {
        $prompt = $this->buildPrompt($patient, $encounter, $condition);
        $response = $this->llm->for('gpt-4o')->completeJson([
            'system' => 'You are a clinical care plan generator. Output valid JSON.',
            'user' => $prompt,
        ]);

        return CarePlanDraft::fromAiResponse($response);
    }

    private function buildPrompt(Patient $patient, Encounter $encounter, string $condition): string
    {
        $age = $patient->age;
        $sex = $patient->sex;
        $diagnoses = $patient->problems->pluck('icd10_code')->all();
        $meds = $patient->activeMedications->pluck('name')->all();
        $allergies = $patient->allergies->pluck('substance')->all();
        $vitals = $patient->latestVitals;
        $labs = $patient->recentLabResults;

        $encounterNote = $encounter->ai_scribe_note;

        return <<<PROMPT
Generate a care plan for this patient.

PATIENT:
- Age: {$age}
- Sex: {$sex}
- Active diagnoses: {implode(', ', $diagnoses)}
- Active medications: {implode(', ', $meds)}
- Allergies: {implode(', ', $allergies)}
- Latest vitals: BP {$vitals->bp_systolic}/{$vitals->bp_diastolic}, HR {$vitals->hr}, BMI {$vitals->bmi}
- Recent labs: {json_encode($labs)}

ENCOUNTER NOTE:
{$encounterNote}

CONDITION: {$condition}

Generate a structured care plan with:
- 3-5 SMART goals (specific, measurable, achievable, relevant, time-bound)
- 5-8 interventions
- 10-15 tasks (mix of patient and care team)
- Review cadence

Output JSON.
PROMPT;
    }
}
```

**Output schema:**

```json
{
  "title": "Type 2 Diabetes Management",
  "category": "chronic_disease",
  "goals": [
    {
      "description": "Achieve HbA1c < 7.0%",
      "metric_type": "numeric",
      "target_value": 7.0,
      "target_unit": "%",
      "target_date": "2026-12-31"
    },
    {
      "description": "Maintain BP < 130/80",
      "metric_type": "numeric",
      "target_value": 130,
      "target_unit": "mmHg",
      "target_date": "2026-12-31"
    }
  ],
  "interventions": [
    {
      "type": "medication",
      "description": "Metformin 1000mg BID",
      "frequency": "twice_daily"
    },
    {
      "type": "lifestyle",
      "description": "Mediterranean diet, 150 min/week exercise",
      "frequency": "daily"
    }
  ],
  "tasks": [
    {
      "title": "Check fasting glucose",
      "assignee_type": "patient",
      "recurrence": "daily"
    },
    {
      "title": "Endocrinology follow-up",
      "assignee_type": "patient",
      "due_date": "+90 days"
    }
  ],
  "review_date": "+90 days"
}
```

**Critical correctness:**

1. **AI is a draft, not final**: Care team reviews and edits before activating.
2. **Validation**: Generated codes (ICD-10, CPT) must be valid.
3. **Safety check**: Drug-allergy, drug-condition, dose-range.
4. **Audit log**: AI generation logged; edits tracked.
5. **Versioning**: Each revision creates a new version; rollback possible.
6. **Patient can decline**: Patient can opt out of specific tasks/goals.

### 19.4 Deep Dive: HIE Document Exchange

Connect to external HIE networks for interoperable records.

```php
namespace App\Modules\Patients\Hie;

use App\Modules\Patients\Models\Patient;
use IHE\XCA\InitiatingGateway;

class HieService
{
    public function __construct(
        private readonly InitiatingGateway $gateway,
        private readonly PatientIdentityService $identity,
    ) {}

    public function fetchDocuments(Patient $patient, \DateTimeInterface $fromDate, ?string $docType = null): array
    {
        // 1. Patient Identity Feed (XCP)
        $ids = $this->identity->resolveAcrossNetworks($patient);

        // 2. Stored Query (XCA)
        $query = new StoredQuery(
            patientId: $ids,
            creationTime: new TimeRange($fromDate, now()),
            classCode: $docType ? new Code($docType) : null,
        );
        $response = $this->gateway->storedQuery($query);

        // 3. Document Retrieve (XCA)
        $docs = [];
        foreach ($response->documents as $docRef) {
            $doc = $this->gateway->retrieve($docRef);
            $docs[] = $this->convertCcdaToInternal($doc);
        }

        return $docs;
    }

    private function convertCcdaToInternal($ccda): InternalDocument
    {
        $parser = new CcdaParser($ccda->content);
        return new InternalDocument(
            type: $parser->getDocumentType(),
            title: $parser->getTitle(),
            author: $parser->getAuthor(),
            custodian: $parser->getCustodian(),
            createdAt: $parser->getEffectiveTime(),
            sections: $parser->getSections(),
            source: 'hie',
            externalId: $ccda->id,
            rawXml: $ccda->content,
        );
    }
}
```

**C-CDA sections parsed:**
- Allergies and Adverse Reactions
- Medications
- Problems
- Results (lab, imaging)
- Vital Signs
- Immunizations
- Procedures
- Encounters
- Family History
- Social History
- Medical Equipment
- Functional Status
- Plan of Care
- Discharge Instructions

**HIE consent (per-patient):**
- Patient opts in/out per HIE network.
- Granular: can opt out of specific data types (e.g., mental health, HIV).
- Time-bound: can revoke anytime.
- Audit log of all access.

### 19.5 Deep Dive: Cohort Builder

Define and run patient cohorts.

```php
namespace App\Modules\Patients\Population;

class CohortBuilder
{
    public function buildFromDefinition(CohortDefinition $def): PatientList
    {
        $query = Patient::query();
        foreach ($def->conditions as $condition) {
            $query = $this->applyCondition($query, $condition);
        }
        return $query->get();
    }

    private function applyCondition($query, Condition $condition): QueryBuilder
    {
        return match($condition->type) {
            'has_diagnosis' => $query->whereHas('problems', fn($q) => $q->whereIn('icd10_code', $condition->values)),
            'has_medication' => $query->whereHas('prescriptions', fn($q) => $q->whereIn('medication_name', $condition->values)->where('status', 'active')),
            'has_lab_abnormal' => $query->whereHas('labResults', fn($q) => $q->whereIn('test_code', $condition->values)->where('is_abnormal', true)->where('measured_at', '>=', now()->subDays($condition->withinDays ?? 365))),
            'has_vital_outside' => $query->whereHas('vitalSigns', fn($q) => $q->where('measured_at', '>=', now()->subDays($condition->withinDays ?? 365))->where(function ($q) use ($condition) {
                if ($condition->metric === 'systolic_bp') {
                    $q->where('value', '>', $condition->threshold);
                }
            })),
            'no_visit_in_days' => $query->whereDoesntHave('appointments', fn($q) => $q->where('completed_at', '>=', now()->subDays($condition->days))),
            'no_procedure' => $query->whereDoesntHave('procedures', fn($q) => $q->whereIn('procedure_code', $condition->values)->where('performed_at', '>=', now()->subDays($condition->withinDays ?? 365))),
            'age_range' => $query->whereRaw('TIMESTAMPDIFF(YEAR, dob, CURDATE()) BETWEEN ? AND ?', [$condition->min, $condition->max]),
            'sex' => $query->where('sex', $condition->value),
            'sdoh_positive' => $query->whereHas('sdoh', fn($q) => $q->whereIn('category', $condition->categories)->where('status', 'identified')),
            'high_utilizer' => $query->whereHas('encounters', fn($q) => $q->where('encounter_class', 'emergency')->where('start_at', '>=', now()->subDays($condition->days)), '>=', $condition->count),
            'risk_score_above' => $query->where('risk_score', '>=', $condition->threshold),
            'engagement_low' => $query->where('last_engagement_at', '<=', now()->subDays($condition->days)),
            'consent_for_outreach' => $query->whereHas('consents', fn($q) => $q->where('purpose', 'outreach')->where('granted', true)),
            default => $query,
        };
    }
}
```

**Example cohort definitions:**

```json
{
  "name": "Diabetic Poor Control",
  "description": "Diabetics with A1c > 9% in last 90 days, no endo visit in 6mo",
  "conditions": [
    { "type": "has_diagnosis", "values": ["E11.9", "E11.65"] },
    { "type": "has_lab_abnormal", "values": ["4548-4"], "withinDays": 90 }
  ]
}
```

```json
{
  "name": "Mammogram Due",
  "description": "Women 50-74, last mammogram > 27 months ago",
  "conditions": [
    { "type": "sex", "value": "female" },
    { "type": "age_range", "min": 50, "max": 74 },
    { "type": "no_procedure", "values": ["77067"], "withinDays": 810 }
  ]
}
```

### 19.6 Deep Dive: CDS Rules Engine

Real-time clinical decision support.

```php
namespace App\Modules\Patients\Cds;

class CdsRulesEngine
{
    private array $rules = [];

    public function __construct(
        private readonly AllergyRepository $allergies,
        private readonly MedicationRepository $meds,
        private readonly ProblemRepository $problems,
        private readonly LabRepository $labs,
        private readonly VitalRepository $vitals,
        private readonly CpicGuidelineRepository $cpic,
    ) {
        $this->rules = $this->loadRules();
    }

    public function evaluate(CdsContext $context): CdsResult
    {
        $alerts = [];
        foreach ($this->rules as $rule) {
            $alert = $rule->evaluate($context, $this);
            if ($alert) $alerts[] = $alert;
        }
        return new CdsResult($alerts);
    }

    private function loadRules(): array
    {
        return [
            new DrugAllergyRule($this->allergies),
            new DrugDrugInteractionRule(),
            new DrugConditionContraindicationRule($this->problems),
            new DrugDoseRangeRule($this->meds),
            new DuplicateTherapyRule(),
            new GuidelineBasedRule(),
            new PreventiveCareRule(),
            new LabCriticalRule($this->labs),
            new SepsisScreeningRule($this->vitals),
            new FallsRiskRule(),
            new SuicideRiskRule(),
        ];
    }
}

class DrugAllergyRule implements CdsRule
{
    public function evaluate(CdsContext $ctx, CdsRulesEngine $engine): ?CdsAlert
    {
        if (!$ctx->drugBeingPrescribed) return null;
        $rxNorm = $ctx->drugBeingPrescribed;
        $allergies = $engine->allergies->forPatient($ctx->patient);
        foreach ($allergies as $allergy) {
            $match = $this->isMatch($allergy, $rxNorm);
            if ($match) {
                return new CdsAlert(
                    severity: 'critical',
                    rule: 'drug_allergy',
                    title: "Allergy: {$allergy->substance}",
                    message: "Patient is allergic to {$allergy->substance}. Reaction: {$allergy->reaction}. Severity: {$allergy->severity}.",
                    action: 'block',
                );
            }
        }
        return null;
    }
}
```

---

## 20. V3+ Precise Plan — Patients

### 20.1 Phase 1: V1 Patient Records (Months 0-3, complete)
**Status:** ✅ Complete (sections 1-13)

### 20.2 Phase 2: V2 EHR + ABHA + AI Scribe (Months 4-9, complete)
**Status:** ✅ Complete (sections 14-16)

### 20.3 Phase 3: V3 Genomics + Wearables + PROMs (Months 10-15)
**Goal:** Add genomics + PGx, wearable integration, PROMs.

**Deliverables:**
- 5 new tables (genetic variants, PGx alerts, wearable devices, wearable observations, PROM responses)
- 10 pre-built PROMs (PHQ-9, GAD-7, SF-12, etc.)
- 10 wearable integrations (Apple Health, Fitbit, Garmin, Dexcom, etc.)
- 20 new V3 endpoints
- 10 acceptance criteria

**Team:** 3 backend engineers, 1 bioinformatics consultant, 1 ML engineer, 1 QA

**Dependencies:** Phase 2 complete; ClinVar/CPIC data refreshed quarterly; 6 wearable sandbox accounts.

**Risks:** PGx data privacy (HSA/FSA-like). Wearable data volume (1M+ obs/day → partitioning). PROMs validation per disease.

**Metrics:** 30% of chronic patients with active care plan. 50% of eligible patients with connected wearable. PGx alerts at 100% of relevant prescriptions.

**Cost:** $380k (3 backend × 6 × $10k + 1 ML × 6 × $15k + bioinformatician $40k)

### 20.4 Phase 4: V3 Care Plans + Family + Population (Months 16-21)
**Goal:** Comprehensive care plans, family/caregiver, population health, HEDIS.

**Deliverables:**
- 4 new tables (care plans, tasks, goals, family relationships)
- 20+ care plan templates
- 30+ HEDIS measures
- Cohort builder UI
- Family/proxy access
- 25 new V3 endpoints
- 12 acceptance criteria

**Team:** 4 backend engineers, 1 designer, 1 QA, 1 clinical informaticist

**Dependencies:** Phase 3 complete; HEDIS specifications (annual); CMS quality measure definitions.

**Risks:** Care plan complexity (provider overload). HEDIS measures are nuanced.

**Metrics:** 80% of chronic patients with active care plan. 50% of care plan tasks completed on time. HEDIS measures computable.

**Cost:** $400k (4 backend × 6 × $10k + designer × 4 × $8k + informaticist $40k)

### 20.5 Phase 5: V3 HIE + Telemedicine + CDS (Months 22-27)
**Goal:** HIE integration, full telemedicine, clinical decision support.

**Deliverables:**
- HIE gateway (Java/Spring, FHIR-based)
- LiveKit integration for telemedicine
- CDS rules engine with 50+ rules
- 30 new V3 endpoints
- 15 acceptance criteria

**Team:** 5 engineers (2 Java/HIE, 1 PHP/telemedicine, 2 PHP/CDS), 1 QA

**Dependencies:** Phase 4 complete; HIE network setup (3-6 months per network); LiveKit or self-hosted.

**Risks:** HIE certification is multi-month. CDS alert fatigue.

**Metrics:** 30% of records fetched from HIE. 50% of visits via telemedicine. 80% of prescriptions trigger CDS.

**Cost:** $480k (5 engineers × 6 × $10k + HIE certification $30k)

### 20.6 Cross-Cutting
- HIPAA + GDPR + DPDP compliance for all data.
- 99.95% uptime.
- p95 < 200ms for any API call.
- 1M observations/day sustained.
- 0 PHI breaches.

### 20.7 Success Metrics
- 70% of patients have at least 1 connected wearable by V3+ stable.
- 90% of chronic patients have active care plan.
- 80% of eligible patients receive preventive care reminders.
- 50% of visits via telemedicine.
- HEDIS measures: > 80% on diabetes, hypertension, cancer screening.
- 0 GDPR/CCPA/DPDP fines.

(End of file)
