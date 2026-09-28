# 08 — Prescriptions Module (Rx, Lines, Print, Written-Style, Medicines Master)

> The clinical deliverable. Every completed appointment can have one or more prescriptions attached. Prescriptions must be legally valid, beautifully printable, and resilient to doctors who can't type.

---

## 1. Scope

| In Scope | Out of Scope |
|---|---|
| Prescription CRUD (create from appointment, edit, delete) | E-prescription transmission to pharmacy networks (V2) |
| Prescription medicine lines (repeater) | Drug interaction checking (V2) |
| Medicines master (autocomplete) | Inventory / pharmacy stock (V2) |
| Printed Rx (structured layout) | Lab orders (V2) |
| Handwritten-style fallback (canvas drawing + photo) | |
| PDF export | |
| Patient-side: list + view + download | |
| Doctor-side: write + edit (within 24h of completion) | |
| Share via signed link to pharmacy | |
| Vitals snapshot inline | |
| Follow-up recommendation | |

---

## 2. Data Model

### 2.1 `medicines` (global master, NOT tenant-scoped)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `name` | varchar(180) indexed | "Paracetamol", "Amoxicillin" |
| `generic_name` | varchar(180) nullable | "Acetaminophen" |
| `manufacturer` | varchar(180) nullable | |
| `form` | enum('tablet','capsule','syrup','injection','drops','cream','ointment','inhaler','patch','other') | |
| `strength` | varchar(60) nullable | "500 mg", "250 mg/5 ml" |
| `unit` | varchar(20) nullable | "ml", "gm" |
| `atc_code` | varchar(20) nullable | WHO ATC classification (V2) |
| `tags` | json | `["analgesic","antibiotic"]` for typeahead |
| `is_active` | boolean default true | |
| `created_at`, `updated_at` | timestamps |

Indexes: `name`, `(name, strength)`, `generic_name`, full-text index on name+generic_name.

### 2.2 `prescriptions` (per-tenant)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `uuid` | char(36) unique | |
| `prescription_number` | varchar(40) unique per tenant | `RX-2026-000001` |
| `patient_id` | bigint FK | |
| `doctor_id` | bigint FK | |
| `appointment_id` | bigint FK nullable | May exist without appointment (admin-created) |
| `consultation_type` | enum('in_person','online') | |
| `issued_at` | timestamp | When doctor finalized |
| `chief_complaint` | text nullable | Snapshot from appointment |
| `diagnosis` | text nullable | |
| `clinical_notes` | text nullable | Doctor's notes |
| `advice` | text nullable | "Drink warm water, rest" |
| `follow_up_date` | date nullable | |
| `follow_up_notes` | text nullable | |
| `vitals_snapshot` | json nullable | { bp: "120/80", pulse: 72, temp: 98.6, weight: 70, ... } |
| `is_written_style` | boolean default false | True if doctor chose handwritten canvas |
| `handwritten_image_path` | varchar(500) nullable | The PNG of the drawing |
| `handwritten_transcription` | text nullable | Doctor's typed fallback alongside drawing |
| `signature_path` | varchar(500) nullable | Doctor's signature image |
| `stamp_path` | varchar(500) nullable | Clinic stamp (optional) |
| `header_template_id` | bigint FK nullable | For letterhead customization |
| `footer_text` | varchar(500) nullable | "This Rx is valid for 90 days" |
| `is_signed` | boolean default false | |
| `signed_at` | timestamp nullable | |
| `signed_ip` | varchar(45) nullable | |
| `shared_token` | varchar(60) unique nullable | For pharmacy share link |
| `shared_at` | timestamp nullable | |
| `shared_expires_at` | timestamp nullable | |
| `metadata` | json | Extensible |
| `created_by_user_id` | bigint FK | |
| `created_at`, `updated_at` | timestamps | |
| `deleted_at` | timestamp nullable | Soft delete |

Indexes:
- `(tenant_id, prescription_number)` UNIQUE
- `(patient_id, issued_at desc)` — patient history
- `(doctor_id, issued_at desc)` — doctor history
- `(appointment_id)` unique nullable
- `shared_token` unique nullable

### 2.3 `prescription_medicines` (lines, per-tenant)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `prescription_id` | bigint FK | |
| `medicine_id` | bigint FK nullable | If chosen from master |
| `medicine_name` | varchar(180) | Always stored (denormalized in case master is deleted) |
| `generic_name` | varchar(180) nullable | |
| `form` | enum(...) | |
| `strength` | varchar(60) nullable | |
| `dosage` | varchar(120) | "1 tablet" |
| `frequency` | varchar(120) | "Twice a day" / BD / 1-0-1 |
| `duration` | varchar(60) | "5 days" |
| `route` | enum('oral','topical','iv','im','subcutaneous','inhalation','other') | |
| `timing` | enum('before_food','after_food','with_food','empty_stomach','anytime') | |
| `instructions` | text nullable | "Avoid alcohol" |
| `quantity` | int nullable | "10 tablets" |
| `is_prn` | boolean default false | "As needed" |
| `prn_condition` | varchar(120) nullable | "When pain > 5" |
| `sort_order` | int default 0 | Display order |
| `created_at`, `updated_at` | timestamps |

Indexes: `(prescription_id, sort_order)`.

### 2.4 `prescription_templates` (per-tenant, doctor-personal)

Reusable Rx templates for common conditions.

| Column | Type |
|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK |
| `doctor_id` | bigint FK |
| `name` | varchar(120) | "URI Standard", "Diabetes Follow-up" |
| `diagnosis` | text nullable |
| `advice` | text nullable |
| `lines` | json | Array of medicine line shapes |
| `is_shared_with_clinic` | boolean default false | Other doctors can clone |
| `use_count` | int default 0 | |
| `created_at`, `updated_at` | timestamps |

### 2.5 `prescription_share_log`

| Column | Type |
|---|---|
| `id` | bigint PK |
| `prescription_id` | bigint FK |
| `shared_with_email` | varchar(180) nullable |
| `shared_with_phone` | varchar(20) nullable |
| `ip` | varchar(45) |
| `user_agent` | varchar(255) |
| `accessed_at` | timestamp |

---

## 3. Writing a Prescription (Doctor App / Web)

### 3.1 Sources

1. **From a completed appointment** — most common. Doctor taps "Write Rx" in the appointment detail. Chief complaint, patient header, vitals auto-populated.
2. **Standalone** — admin creates a prescription for a patient who walked in without an appointment.
3. **From a template** — doctor picks one of their own templates, edits, saves.
4. **Clone previous** — copies a past Rx for the same patient, doctor edits as needed.

### 3.2 Two Writing Modes

**Mode A — Structured form (default).**
The doctor types into a structured form. Medicine name is autocomplete from `medicines` master. Fields: name, strength, dosage, frequency, duration, route, timing, instructions, quantity, is_prn, prn_condition.

Common frequencies are preset buttons: `OD (1-0-0)`, `BD (1-0-1)`, `TDS (1-1-1)`, `QID (1-1-1-1)`, `HS (0-0-1)`, `SOS (as needed)`. Doctor can also type free text.

**Mode B — Handwritten canvas.**
Some doctors (especially older ones) prefer to write the Rx by hand. We provide an HTML5 canvas where the doctor draws. On save, the canvas is rasterized to PNG, stored, and also OCR'd (V2) into a transcription the patient can search.

### 3.3 Signing

- Doctor signs the Rx in-app (touch / mouse / trackpad → SVG → PNG).
- OR admin uploads a pre-scanned signature image once in profile, system stamps it.
- Optional: clinic stamp (uploaded once per clinic, applied to all Rx).
- Signing locks the Rx. Edits require an "unlock" with audit reason.
- IP and timestamp recorded.

### 3.4 Edit Window

- Doctor can edit a prescription freely for **24 hours** after `signed_at`.
- After 24h, edits require a new "amendment" — the original is kept, a new version is linked with `parent_prescription_id`.
- Admin can edit anytime with audit reason.

### 3.5 Service Methods

```php
namespace App\Modules\Prescriptions\Services;

class PrescriptionService {
    public const VERSION = '1.0.0';

    public function createFromAppointment(Appointment $a, array $data, User $doctor): Prescription;
    public function createStandalone(User $actor, Patient $p, array $data): Prescription;
    public function addLine(Prescription $rx, array $data): PrescriptionMedicine;
    public function updateLine(PrescriptionMedicine $line, array $data): PrescriptionMedicine;
    public function removeLine(PrescriptionMedicine $line): void;
    public function applyTemplate(Prescription $rx, PrescriptionTemplate $tpl): void;
    public function saveTemplate(User $doctor, string $name, Prescription $rx): PrescriptionTemplate;
    public function sign(Prescription $rx, User $doctor, array $signature): void;
    public function unlock(Prescription $rx, User $actor, string $reason): void;
    public function amend(Prescription $rx, User $doctor, array $data): Prescription;
    public function delete(Prescription $rx, User $actor): void;        // soft, audit
    public function getForPatient(Patient $p, int $page): LengthAwarePaginator;
    public function getForDoctor(User $doctor, int $page, array $filters): LengthAwarePaginator;
    public function generatePdf(Prescription $rx): string;            // returns PDF binary
    public function generateShareToken(Prescription $rx, ?int $hours = 72): string;
    public function revokeShare(Prescription $rx): void;
    public function renderPreview(Prescription $rx, string $style = 'structured'): string;  // HTML
}
```

---

## 4. Prescription Number Generation

`RX-{YEAR}-{SEQUENTIAL}` using a per-tenant per-year counter table `prescription_sequences` (same pattern as `patient_mrn_sequences`). Configurable format like MRN.

---

## 5. PDF Generation

Two layouts:

### 5.1 Structured Layout

```
┌────────────────────────────────────────────────┐
│  [Clinic Logo]   CLINIC NAME                   │
│                  Address · Phone · Email       │
│                  GSTIN: ...                    │
├────────────────────────────────────────────────┤
│  Dr. Priya Mehta                                │
│  MBBS, MD Cardiology                            │
│  Reg. No: ...                                   │
│                                                │
│  Date: 2026-06-04                               │
│  Patient: Aarav Sharma   MRN: MRN-2026-000042  │
│  Age/Sex: 8 / M   Blood Group: A+              │
│  Vitals: BP 110/70, Pulse 88, Temp 98.6°F, Wt 25kg │
│  Chief Complaint: Fever since 3 days           │
│  Diagnosis: Viral Fever                        │
│                                                │
│  Rx                                             │
│  ──────────────────────────────────────────    │
│  1. Paracetamol 500 mg                         │
│     1 tablet every 6 hours, after food, 3 days │
│                                                │
│  2. Cetirizine 10 mg                           │
│     1 tablet at night, 5 days                  │
│                                                │
│  Advice: Drink warm water. Rest.               │
│  Follow-up: 2026-06-10                          │
│                                                │
│  [Signature]                                    │
│  Dr. Priya Mehta                                │
│                                                │
│  This Rx is computer-generated and valid       │
│  without a physical signature.                 │
└────────────────────────────────────────────────┘
```

Implementation: `Spatie\LaravelPdf` (wrapping Chromium) or `barryvdh/laravel-dompdf` for simpler output. PDF generated on demand, cached in object storage for 30 days.

### 5.2 Handwritten Layout

Just the clinic header, the doctor's handwritten image, the typed transcription (if any), and the signature. Plain.

### 5.3 Page Size

A5 (default) for handwritten feel; A4 for structured. Configurable per tenant.

---

## 6. Patient Experience

### 6.1 Mobile App — Prescriptions Tab

- List view: thumbnail of Rx (first medicine line as title), doctor name, date, status badge (active/expired).
- Filter: by doctor, by date range, by medicine name.
- Tap → detail screen:
  - Full structured view
  - PDF view (in-app)
  - Download / Share buttons
  - "Share with pharmacy" → opens share sheet with link + QR
  - "Reorder" → V2: order from partner pharmacy
  - "Set reminder" → V2: native phone reminders per medicine

### 6.2 Share Link to Pharmacy

```
GET https://medicare.app/rx/{shared_token}
```

- No auth required (within `shared_expires_at`).
- Read-only display: patient header (name, age, DOB), clinic/doctor, medicines list.
- Pharmacy can tap "Mark as fulfilled" if integrated (V2).
- Each access logged in `prescription_share_log`.

---

## 7. Medicines Master

### 7.1 Seeding

- Seeded with ~5000 common Indian + international medicines (name, generic, form, strength, ATC).
- Source: WHO ATC + Indian national formulary + a curated open dataset.
- Admins can add local / uncommon medicines.

### 7.2 Typeahead Search

`GET /api/v1/medicines/search?q=para` returns matches ranked by:
1. Exact name match
2. Name starts with query
3. Generic name match
4. ATC code match
5. Recently used by this doctor (boost)

Response includes `recently_prescribed_count` for the doctor to favor familiar choices.

### 7.3 Drug Interaction (V2)

V2 will integrate RxNav or similar to check for interactions. V1 shows a soft warning when the doctor adds a 2nd medicine that shares an ATC class with an existing one on the same Rx.

---

## 8. Templates

### 8.1 Doctor's Personal Templates

- Doctor saves any Rx as a template with a name.
- Next time, picks from "My Templates" and edits lightly.
- `use_count` increments; sorted by recent use.

### 8.2 Clinic-Shared Templates

- Doctor with `manage-templates` permission can mark a template `is_shared_with_clinic=true`.
- All clinic doctors see it in "Clinic Templates".

### 8.3 System Templates

Seeded per clinic at install: e.g., "Annual Health Checkup Standard", "First Visit Pediatric", etc. Cannot be edited, only cloned.

---

## 9. Vitals Snapshot

When creating an Rx from an appointment, we snapshot the latest vitals (or the vitals taken during the visit if any). The doctor can edit before signing. The snapshot is preserved as a JSON field on the prescription — even if the underlying Vital rows are later "corrected" (appended), the Rx shows what the doctor saw at the time of writing.

---

## 10. API Endpoints

### 10.1 Medicines (Autocomplete)

```
GET    /api/v1/medicines/search?q=&limit=20     (auth required)
POST   /api/v1/admin/medicines                  (admin)
PUT    /api/v1/admin/medicines/{id}
DELETE /api/v1/admin/medicines/{id}              (soft; blocked if used in any Rx)
POST   /api/v1/admin/medicines/import            (CSV)
```

### 10.2 Prescriptions

```
GET    /api/v1/patient/prescriptions?filter=&page=
GET    /api/v1/patient/prescriptions/{id}
GET    /api/v1/patient/prescriptions/{id}/pdf
POST   /api/v1/patient/prescriptions/{id}/share        { hours_valid?: 72 }
DELETE /api/v1/patient/prescriptions/{id}/share
GET    /api/v1/public/rx/{shared_token}                (no auth)

GET    /api/v1/doctor/prescriptions?patient_id=&from=&to=
POST   /api/v1/doctor/prescriptions                    (from appointment or standalone)
GET    /api/v1/doctor/prescriptions/{id}
PUT    /api/v1/doctor/prescriptions/{id}                (within 24h of sign)
POST   /api/v1/doctor/prescriptions/{id}/sign           { signature_data_url }
POST   /api/v1/doctor/prescriptions/{id}/unlock         { reason }
POST   /api/v1/doctor/prescriptions/{id}/amend
POST   /api/v1/doctor/prescriptions/{id}/lines
PUT    /api/v1/doctor/prescriptions/{id}/lines/{lineId}
DELETE /api/v1/doctor/prescriptions/{id}/lines/{lineId}
POST   /api/v1/doctor/prescriptions/{id}/lines/reorder  { ids: [...] }
POST   /api/v1/doctor/prescriptions/{id}/apply-template { template_id }

GET    /api/v1/doctor/templates
POST   /api/v1/doctor/templates
PUT    /api/v1/doctor/templates/{id}
DELETE /api/v1/doctor/templates/{id}

GET    /api/v1/admin/prescriptions?doctor_id=&patient_id=&from=&to=
```

---

## 11. Validation

```php
// CreatePrescriptionRequest
public function rules(): array {
    return [
        'patient_id'        => ['required', 'exists:patients,id'],
        'appointment_id'    => ['nullable', 'exists:appointments,id'],
        'consultation_type' => ['required', Rule::in(['in_person', 'online'])],
        'diagnosis'         => ['nullable', 'string', 'max:2000'],
        'clinical_notes'    => ['nullable', 'string', 'max:5000'],
        'advice'            => ['nullable', 'string', 'max:2000'],
        'follow_up_date'    => ['nullable', 'date', 'after:today'],
        'follow_up_notes'   => ['nullable', 'string', 'max:1000'],
        'vitals_snapshot'   => ['nullable', 'array'],
        'lines'             => ['required', 'array', 'min:1', 'max:30'],
        'lines.*.medicine_id'  => ['nullable', 'exists:medicines,id'],
        'lines.*.medicine_name'=> ['required_without:lines.*.medicine_id', 'string', 'max:180'],
        'lines.*.form'         => ['required', Rule::in(['tablet','capsule','syrup','injection','drops','cream','ointment','inhaler','patch','other'])],
        'lines.*.strength'     => ['nullable', 'string', 'max:60'],
        'lines.*.dosage'       => ['required', 'string', 'max:120'],
        'lines.*.frequency'    => ['required', 'string', 'max:120'],
        'lines.*.duration'     => ['required', 'string', 'max:60'],
        'lines.*.route'        => ['required', Rule::in(['oral','topical','iv','im','subcutaneous','inhalation','other'])],
        'lines.*.timing'       => ['required', Rule::in(['before_food','after_food','with_food','empty_stomach','anytime'])],
        'lines.*.instructions' => ['nullable', 'string', 'max:500'],
        'lines.*.quantity'     => ['nullable', 'integer', 'min:1'],
        'lines.*.is_prn'       => ['boolean'],
        'lines.*.prn_condition'=> ['required_if:lines.*.is_prn,true', 'string', 'max:120'],
        'is_written_style'     => ['boolean'],
        'handwritten_image'    => ['required_if:is_written_style,true', 'image', 'max:5120'],
    ];
}
```

---

## 12. Realtime Events

| Event | Channel | Use |
|---|---|---|
| `PrescriptionCreated` | `patient.{id}` | Patient gets FCM "New prescription from Dr. X" |
| `PrescriptionSigned` | `patient.{id}` | Patient can now download |
| `PrescriptionShared` | `patient.{id}` | Confirmation |
| `PrescriptionAmended` | `patient.{id}` | Patient sees new version |

---

## 13. Audit

Every prescription is audited: created, signed, unlocked, amended, deleted, shared, downloaded, accessed via share link. Download events include IP and user agent.

---

## 14. Testing Strategy

| Test | Assertion |
|---|---|
| `PrescriptionCreateTest` | Created from appointment; lines ordered; vitals snapshotted |
| `SigningTest` | Locks the Rx, stamps signed_at + IP |
| `EditWindowTest` | Edit allowed at 23h, denied at 25h, requires amendment |
| `AmendmentTest` | Original preserved, new version linked |
| `TemplateApplyTest` | All lines copied, sort_order preserved |
| `MedicineTypeaheadTest` | Returns matches ranked correctly |
| `HandwrittenTest` | PNG stored, transcription optional |
| `ShareTokenTest` | Public link works within window, fails after, audit logged |
| `PDFGenerationTest` | PDF contains all lines, signature, header |
| `CrossTenantTest` | Doctor in tenant A cannot see prescriptions in tenant B |
| `MedicineDeletionTest` | Cannot delete a medicine used in any Rx |

---

## 15. Acceptance Criteria (V1)

- [ ] Doctor can write an Rx from a completed appointment with all common fields.
- [ ] Medicine autocomplete returns relevant results in <200ms.
- [ ] Doctor can save a prescription as a template and re-use it.
- [ ] Doctor can sign the Rx in-app; signature is embedded in the PDF.
- [ ] PDF is generated in <2s and looks like a traditional paper Rx.
- [ ] Patient receives an FCM push when a new Rx is signed.
- [ ] Patient can view, download, and share the Rx via a link valid for 72h.
- [ ] Handwritten canvas mode saves the drawing and shows it in the PDF.
- [ ] Edits within 24h are allowed; after that, an amendment is required.
- [ ] Amendments preserve the original Rx for legal record.
- [ ] Cross-tenant prescriptions are inaccessible.
- [ ] Every access to a shared link is audit-logged.

---

## 16. V2 Roadmap — Prescriptions

> V1 ships a complete prescription module with signing, PDF, share tokens, templates, and amendments. V2 layers on **drug-interaction checks**, **national e-prescription formats**, **AI-generated drafts**, and **lab orders** integrated into the Rx.

### 16.1 Drug interaction database (`drug-interaction-db`, V2, MML 4)

**1. Overview.** V1's interaction check is a soft warning: same-ATC-class duplicates on the same Rx. V2 introduces a real-time, **multi-source**, **per-pair severity-ranked** check at the moment the doctor adds a medicine, plus a final check on signing, plus a check against the patient's active medication list. The check is synchronous and must complete in <200ms p99 (per the V2 perf budget in `08-Prescriptions` §16.7). Sources: RxNav/NDF-RT (US, free, NIH), DrugBank (commercial, India/World), WHO Essential Medicines interactions (free), and a tenant-curated CSV. A `severity='contraindicated'` is a hard stop on signing; `major` requires explicit acknowledgement.

**2. Domain Model.**
```php
Schema::create('drug_interactions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->nullable()->index();             // null = global
    $t->string('drug_a_rxnorm', 16)->index();              // RxNorm concept unique identifier
    $t->string('drug_b_rxnorm', 16)->index();
    $t->string('severity', 16);                             // 'minor' | 'moderate' | 'major' | 'contraindicated'
    $t->string('mechanism', 255)->nullable();
    $t->text('description');
    $t->json('evidence')->nullable();                       // [{source, citation_url, study_ref}]
    $t->string('source', 32);                                // 'rxnav' | 'drugbank' | 'who' | 'tenant_csv'
    $t->boolean('is_active')->default(true)->index();
    $t->timestamps();
    $t->unique(['tenant_id', 'drug_a_rxnorm', 'drug_b_rxnorm']);
});

Schema::create('drug_interaction_log', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('prescription_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('doctor_id')->index();
    $t->json('medicines_checked');                          // [{name, rxnorm, dose, frequency}]
    $t->json('interactions_found');                         // [{drug_a, drug_b, severity, mechanism, evidence}]
    $t->boolean('hard_stop')->default(false);
    $t->boolean('acknowledged')->default(false);
    $t->ulid('acknowledged_by_user_id')->nullable();
    $t->timestamp('acknowledged_at')->nullable();
    $t->string('action_taken', 32);                         // 'proceeded' | 'modified_rx' | 'cancelled_rx' | 'consulted_pharmacist'
    $t->text('override_reason')->nullable();                // if action='proceeded' on a major
    $t->integer('check_duration_ms');
    $t->timestamps();
});

Schema::create('drug_interaction_datasets', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->nullable()->index();
    $t->string('source', 32);                                // 'rxnav' | 'drugbank' | 'who' | 'tenant_csv'
    $t->string('version', 32);
    $t->date('imported_at')->index();
    $t->integer('row_count');
    $t->string('s3_key', 255);
    $t->string('checksum_sha256', 64);
    $t->timestamps();
});
```

**3. Lookup flow (`DrugInteractionService::check`).**
1. Resolve each line's medicine to its RxNorm code via `Medicine::resolveRxNorm($name)` (fuzzy match on name, dose, route).
2. Query `drug_interactions` for every (a, b) pair where both codes match (single SQL with `WHERE (a IN (...) AND b IN (...)) OR (b IN (...) AND a IN (...))`); O(n²) for n lines, but bounded to 20 lines per Rx.
3. Also check the patient's `current_medications` (from `patients` JSON) and `chronic_conditions` for contraindications (e.g., beta-blocker in asthmatic).
4. Return all interactions, sorted by severity (`contraindicated` > `major` > `moderate` > `minor`).
5. The result is cached in Redis for 60 s keyed by the sorted set of RxNorm codes, so a doctor who edits and re-checks doesn't re-query.

**4. Real-time at line-add.** When the doctor adds a medicine in the Rx writer, the front-end calls `POST /api/v2/drug-interactions/check` (debounced 300ms) and renders yellow/red chips in the medicine row. The final check on `POST /api/v2/prescriptions/{id}/sign` re-runs and refuses to sign if any `contraindicated` is unresolved.

**5. API Surface.**
```
POST   /api/v2/drug-interactions/check                     # { medicines: [{name, rxnorm?, dose?}] } -> [interaction]
GET    /api/v2/drug-interactions?drugs=rxnorm1,rxnorm2     # explicit pair check
GET    /api/v2/drug-interactions/log?patient_id=...        # history for a patient
POST   /api/v2/drug-interactions/log/{id}/acknowledge      # doctor acknowledges a major
GET    /api/v2/drug-interactions/datasets                  # import history
POST   /api/v2/drug-interactions/datasets/import           # admin uploads CSV
POST   /api/v2/drug-interactions/datasets/sync/{source}    # manual sync (RxNav, DrugBank)
```

**6. Frontend.** Inside the Rx writer, a right-side panel "Interactions" shows live chips: 🟢 none, 🟡 moderate (acknowledge on sign), 🔴 major (must acknowledge with reason), ⛔ contraindicated (hard stop). On sign, a modal lists all unresolved items; the doctor can click "Acknowledge" per item (with optional override reason), or "Cancel sign". The patient does not see this UI; the doctor must close all interactions before the Rx is final.

**7. Realtime.** None at the channel level (the check is request/response); however, when an interaction check returns `severity='contraindicated'`, the patient app shows a "Doctor is reviewing your prescription" placeholder (so the patient doesn't see a half-finished Rx).

**8. Security & RBAC.** Roles: `doctor` (view + acknowledge), `pharmacist` (view + override on dispense), `tenant_admin` (manage sources + view audit), `clinical_admin` (manage tenant-curated CSV). The drug-interaction database is **read-only** at runtime; all updates are batch imports tracked in `drug_interaction_datasets` with checksum verification. The check endpoint never returns patient PII; it only returns the interaction list. Logs are written for every check, including zero-result checks, for audit. Per `20-Security §11.4`, the DrugBank API key (if used) is stored in AWS Secrets Manager and rotated quarterly.

**9. Acceptance Criteria.**
- [ ] Adding a medicine that interacts with an existing one produces a chip within 300ms.
- [ ] A `severity='contraindicated'` pair blocks signing.
- [ ] A `severity='major'` pair requires acknowledgement with a reason; the reason is audit-logged.
- [ ] The check p99 latency is < 200ms (per `08-Prescriptions` §16.7 budget).
- [ ] Tenant CSV imports are validated; duplicate (a,b) pairs across sources are deduplicated by severity (highest wins).
- [ ] A zero-result check still writes a `drug_interaction_log` row.
- [ ] A daily sync with RxNav updates the dataset; the new version is recorded; old datasets are retained for 90 days for audit.

**10. Real-World Test Scenarios.**
- *Warfarin + Aspirin:* doctor adds Aspirin to a patient already on Warfarin; chip turns 🔴; on sign, modal demands acknowledgement + reason; doctor proceeds with "patient is post-MI, risk accepted".
- *Beta-blocker + Asthma:* patient has chronic asthma; doctor prescribes Propranolol; chronic-condition check returns `contraindicated`; signing is hard-blocked; doctor must change Rx.
- *Polypharmacy:* patient is on 8 medicines; doctor adds a 9th; the system checks 8×8 = 64 pairs in a single SQL; returns 3 minor + 1 moderate; all are acknowledged in a single modal.
- *DrugBank daily sync:* 02:00 UTC the daily job pulls DrugBank's update; new interactions are merged; conflicts (same pair, different severity) are resolved by taking the max severity; an audit row is written.
- *Tenant CSV override:* a hospital adds a tenant-specific interaction "tenofovir + stavudine" with a custom mechanism; this row is `tenant_id=...`; it does not leak to other tenants.
- *Performance:* 100 concurrent checks with 5 medicines each complete in <2s on a single API node; k6 test passes.

**11. Dev Workflow.** `php artisan drug-interaction:rxnav-sync` runs the daily sync manually. `php artisan drug-interaction:import-csv --tenant=... --file=...` imports a tenant CSV. `php artisan drug-interaction:check --medicines="warfarin,aspirin"` runs an ad-hoc check from CLI. Code: `app-modules/drug-interaction-db/`; tests: `app-modules/drug-interaction-db/tests/Feature/{Check,Acknowledge,Import}Test.php` and `tests/Performance/interaction-load.k6.js`.

### 16.2 National e-prescription formats — `e-prescription` (V2, MML 3)

**1. Overview.** V1 produces a self-signed PDF. V2 produces a **structured, signed, standards-compliant** e-prescription that pharmacies and national health authorities can verify cryptographically. The country-specific formats are:

| Country | Format | Authority |
|---|---|---|
| India | ABDM Prescription (FHIR R4 Bundle: `MedicationRequest` + `Patient` + `Practitioner` + `Organization`) | NHA / ABDM HIU |
| US | SCRIPT 2017071 (NCPDP) | SureScripts |
| EU | ePrescription (HL7 CDA + PDF/A-3 with qualified e-signature per eIDAS) | National eHealth |
| UK | NHS Digital EPS (FHIR R4) | NHS |
| Australia | ePrescription (HL7 FHIR) | ADHA |

MML 3 at V2 GA covers: signing (per-country), format emission, partner pharmacy transmission, QR for patient pickup, and revocation. MML 4 (V2.1) adds controlled-substance two-factor workflow (e.g., US EPCS).

**2. Domain Model.**
```php
// Prescriptions table additions (per §16.2)
$t->string('prescription_format', 32)->default('v1');   // 'v1' | 'abdm' | 'script' | 'gstandard' | 'eps' | 'au_eps'
$t->string('signature_jws', 4096)->nullable();           // JWS compact serialization
$t->string('signature_algorithm', 32)->default('ES256'); // ECDSA P-256
$t->string('signature_cert_sha256', 64)->nullable();     // pinning the signing cert
$t->string('fhir_bundle_id', 64)->nullable()->index();   // for ABDM/SCRIPT/EPS
$t->string('fhir_bundle_s3_key')->nullable();            // full bundle, signed
$t->timestamp('signed_at')->nullable();
$t->string('signed_by_dsc_fingerprint', 128)->nullable();// India DSC
$t->string('abdm_hiu_request_id', 64)->nullable();        // ABDM upload receipt
$t->string('abdm_hiu_response', 4096)->nullable();        // raw ABDM response
$t->json('interactions_checked')->nullable();
$t->string('revocation_reason', 64)->nullable();
$t->timestamp('revoked_at')->nullable();

// prescription_partner_dispenses (NEW)
Schema::create('prescription_partner_dispenses', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('prescription_id')->index();
    $t->ulid('partner_pharmacy_id')->index();
    $t->string('external_dispense_id', 64)->nullable();
    $t->string('status', 16)->default('pending');         // pending | partial | dispensed | rejected | expired
    $t->json('lines_dispensed')->nullable();
    $t->timestamp('dispensed_at')->nullable();
    $t->timestamps();
});
```

**3. Signing flow (`ElectronicSignatureService::sign`).**
- The doctor authenticates with the country's required mechanism: India uses a Class 2 or Class 3 DSC USB token (PKCS#11); EU uses a smart card or HSM-backed qualified signature; US EPCS uses two-factor (password + token); UK/Australia use the national identity provider.
- The Rx is serialized to a canonical JWS payload (RFC 7515 compact), signed with `ES256` (ECDSA P-256 over P-256 curve; FIPS-compliant).
- The signature is stored on the `prescriptions` row; the signing cert's SHA-256 is pinned in `signature_cert_sha256` so a later compromise of the doctor's cert can be detected (cert-pinning mismatch on verify → reject).
- The full FHIR bundle is stored in S3 (SSE-KMS) at `fhir_bundle_s3_key`; the JWS references the bundle ID, not the S3 URL.

**4. Country-specific emission.**
- **India ABDM:** the signed FHIR bundle is uploaded to NHA's HIU via the `POST /api/v2/prescriptions/{id}/upload-to-abdm` endpoint; the response is stored in `abdm_hiu_request_id` and `abdm_hiu_response`. The patient receives a QR code; the pharmacist scans it, fetches the bundle from the HIU, verifies the JWS against NHA's published public key, and dispenses.
- **US SCRIPT 2017071:** the Rx is transmitted to SureScripts via their `SendMessage` endpoint; the response is the dispense-pending acknowledgment. Refills are tracked in `prescription_partner_dispenses`.
- **EU ePrescription (per country):** the FHIR bundle is wrapped in a PDF/A-3 with a qualified e-signature; the PDF is the patient-facing artifact; the bundle is the machine-readable artifact.
- **UK NHS EPS:** the FHIR bundle is sent to the EPS via Spine; the patient receives an EPS nomination token; any UK pharmacy can dispense.
- **Australia ePrescription:** the FHIR bundle is delivered to the patient's Medicare app; the patient presents a token at the pharmacy.

**5. API Surface.**
```
POST   /api/v2/prescriptions/{id}/sign                     # sign with country-appropriate mechanism
GET    /api/v2/prescriptions/{id}/fhir-bundle              # signed bundle JSON
POST   /api/v2/prescriptions/{id}/upload-to-abdm           # India: { fhir_id }
POST   /api/v2/prescriptions/{id}/transmit                 # US/EU/UK/AU partner network
POST   /api/v2/prescriptions/{id}/revoke                   # doctor or admin revokes
GET    /api/v2/prescriptions/{id}/verify                   # verify JWS + cert chain
GET    /api/v2/prescriptions?abdm_hiu_request_id=...      # audit lookup
```

**6. Frontend.** The Rx writer gains a "Format" dropdown (auto-selected by tenant country; manual override per Rx for cross-border). On sign, a signing modal appears with the country-appropriate UI: India shows "Insert DSC token + enter PIN"; US EPCS shows "Enter password + 6-digit token code"; EU shows "Insert smart card + enter PIN". The signed Rx is rendered in the patient app as a card with: QR (India/Australia), barcode (US), PDF/A-3 download (EU), and a "Send to pharmacy" button that opens a partner-pharmacy picker.

**7. Realtime.**
- `prescription.signed.{tenant}.{patient_id}` — push to patient app "Your Rx is ready".
- `prescription.transmitted.{prescription_id}.{partner_pharmacy_id}` — pharmacy receives the Rx.
- `prescription.dispensed.{prescription_id}` — webhook from the partner network; the patient is notified.
- `prescription.revoked.{prescription_id}` — pharmacy network is notified to cancel pending dispense.

**8. Security & RBAC.** Roles: `doctor` (sign own), `pharmacist` (view signed, mark dispensed; cannot re-sign), `tenant_admin` (manage certs + revoke), `compliance_officer` (audit). The signing operation is gated by the country's mechanism (DSC/smart card/EPCS token); a stolen password alone cannot sign. All signature operations are audit-logged with `action='prescription.sign'` and the cert fingerprint. A revocation is cryptographically valid only if the original signing key is still trusted; otherwise a court-order re-issue is required. The JWS verification endpoint re-fetches the cert chain from the country's trust store; a revoked cert is rejected.

**9. Acceptance Criteria.**
- [ ] A signed Rx is verifiable by the country's public key 7 years later (the cert is pinned by SHA-256, the JWS is stored on the row).
- [ ] India: a QR code presented to a partner pharmacy is scannable; the bundle is fetchable from the HIU; the JWS verifies.
- [ ] US: a SCRIPT 2017071 message is accepted by SureScripts; the response includes a `dispense_pending` ack.
- [ ] EU: a PDF/A-3 with a qualified e-signature is verifiable in Adobe Acrobat and the EU eHealth trust list.
- [ ] A revoked Rx cannot be dispensed; the partner network rejects within 60s of revocation.
- [ ] The DSC/EPCS/smart-card flow blocks signing if the device is removed mid-flow.
- [ ] A re-signed Rx (after edit) gets a new `signed_at`; the prior JWS is retained in `signature_jws_history` (separate column, not shown here).

**10. Real-World Test Scenarios.**
- *Cross-border:* doctor in India writes an Rx; the format is auto-set to ABDM; the patient travels to Singapore and presents the QR; Singapore's pharmacy cannot read ABDM directly; the doctor re-issues in `au_eps` format (or the patient requests a PDF copy with manual translation).
- *DSC revoked:* doctor's DSC is revoked by the CA; the next sign attempt fails with "Cert revoked"; the doctor's sessions are force-ended; the tenant admin is paged.
- *EPCS token drift:* doctor's EPCS token clock drifts by 90s; sign fails with "Token out of window"; the system suggests re-syncing the token.
- *SureScripts reject:* SCRIPT message has a malformed NCPDP field; SureScripts rejects; the system retries once with the corrected field; if it still fails, the Rx is marked `transmit_failed` and the doctor is notified.
- *Audit 7 years later:* regulator asks "Show me all warfarin Rxs signed by Dr. X in 2026"; the row is fetched, the JWS is verified, the cert chain is re-checked against the 2026 trust store (or, if the CA is gone, the cert pin is used).

**11. What this module is NOT:**
- Not a controlled-substance DEA compliance system. That is V2.1 (EPCS two-factor + state PMP integration).
- Not a pharmacy management system. The partner pharmacy is a separate system; we transmit the Rx; they dispense.
- Not a substitute for the country's national ePrescription infrastructure. We are a client of that infrastructure; if NHA HIU is down, ABDM uploads are queued.

**12. Dev Workflow.** `php artisan erx:sign --rx=... --format=abdm --dry-run` simulates signing without a real DSC. `php artisan erx:verify --rx=...` re-verifies a JWS. `php artisan erx:abdm-sync` replays queued ABDM uploads. Code: `app-modules/e-prescription/`; tests: `app-modules/e-prescription/tests/Feature/{Sign,Transmit,Revoke,Verify}Test.php` and `tests/Compliance/JwsLongTermVerifyTest.php` (verifies 7-year-old signatures against archived trust stores).

### 16.3 Lab orders in Rx (`pathology-lab`, V2)

**1. Overview.** V1's Rx is medicines-only. V2 lets the doctor attach **lab tests** to the same Rx, sent to a partner lab network (1mg, Thyrocare, Metropolis, SRL, Quest, LabCorp depending on region), with home-sample-collection, partner-driven result upload, and LOINC-coded result normalization. The pathology module is the first V2 marquee feature because it is the natural extension of the prescription workflow — patients expect "doctor orders tests, results come back", not "I have to walk into a lab and re-register". MML 4 GA means: full CRUD, partner flow, mobile app, e-Report PDF, in-app chart view.

**2. Domain Model.**
```php
Schema::create('lab_panels', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 128);                                 // 'Basic Metabolic Panel', 'Lipid Profile'
    $t->string('code', 32)->unique();                        // 'BMP', 'LIPID'
    $t->json('test_codes');                                  // ['2345-7', '2345-8', ...] LOINC
    $t->decimal('price', 12, 2);
    $t->string('currency', 3);
    $t->json('pre_requisites')->nullable();                  // 'fasting_8h'
    $t->text('description')->nullable();
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});

Schema::create('lab_partners', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 128);                                 // '1mg', 'Thyrocare'
    $t->string('code', 32);                                  // '1mg', 'thyrocare'
    $t->string('api_base_url', 255);
    $t->string('api_key_hash', 128);                        // bcrypt'd; raw key only at creation
    $t->json('serviceable_pincodes');                        // ['110001', '110002', ...]
    $t->string('status', 16)->default('active');
    $t->timestamps();
});

Schema::create('lab_orders', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('prescription_id')->nullable()->index();      // bundled in same Rx
    $t->ulid('patient_id')->index();
    $t->ulid('doctor_id')->index();
    $t->ulid('lab_partner_id')->index();
    $t->string('external_order_id', 64)->nullable();        // partner-side ID
    $t->string('status', 32)->default('draft');             // draft | submitted | sample_collected | in_lab | completed | cancelled | rejected
    $t->string('priority', 16)->default('normal');          // stat | urgent | normal
    $t->json('clinical_notes')->nullable();
    $t->json('patient_address')->nullable();                // for home collection
    $t->timestamp('scheduled_at')->nullable()->index();
    $t->timestamp('sample_collected_at')->nullable();
    $t->timestamp('completed_at')->nullable();
    $t->decimal('total_amount', 12, 2);
    $t->char('currency', 3);
    $t->json('metadata')->nullable();
    $t->timestamps();
    $t->softDeletes();
});

Schema::create('lab_order_items', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('lab_order_id')->index();
    $t->string('test_code', 32);                            // LOINC
    $t->string('test_name', 128);
    $t->decimal('price', 12, 2);
    $t->text('instructions')->nullable();
});

Schema::create('lab_samples', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('lab_order_id')->index();
    $t->string('barcode', 64)->unique();
    $t->string('type', 16);                                 // 'blood' | 'urine' | 'swab' | 'tissue' | 'stool' | 'sputum'
    $t->timestamp('collected_at');
    $t->ulid('collected_by_user_id')->nullable();
    $t->string('status', 16)->default('collected');         // collected | in_transit | received | processed | disposed
    $t->string('temperature_log_s3_key')->nullable();
});

Schema::create('lab_results', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('lab_order_id')->index();
    $t->ulid('lab_order_item_id')->index();
    $t->string('value_type', 16);                           // 'numeric' | 'text' | 'coded' | 'panel'
    $t->string('value_numeric', 32)->nullable();
    $t->string('value_text', 255)->nullable();
    $t->string('value_unit', 32)->nullable();
    $t->string('reference_range_low', 32)->nullable();
    $t->string('reference_range_high', 32)->nullable();
    $t->string('abnormal_flag', 16)->nullable();            // 'L' | 'H' | 'LL' | 'HH' | 'A' | 'AA' | null
    $t->text('interpretation')->nullable();
    $t->ulid('verified_by_pathologist_id')->nullable();
    $t->timestamp('verified_at')->nullable();
    $t->timestamps();
});

Schema::create('lab_reports', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('lab_order_id')->unique();
    $t->string('pdf_s3_key', 255)->nullable();
    $t->string('signed_url_expires_at')->nullable();
    $t->timestamp('generated_at')->nullable();
    $t->timestamp('sent_to_patient_at')->nullable();
    $t->timestamps();
});

Schema::create('lab_partner_api_calls', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('lab_partner_id')->index();
    $t->string('endpoint', 128);
    $t->string('method', 8);
    $t->integer('http_status');
    $t->integer('duration_ms');
    $t->json('request_payload')->nullable();
    $t->json('response_payload')->nullable();
    $t->string('error', 255)->nullable();
    $t->timestamp('called_at')->index();
});
```

**3. API Surface.**
```
GET    /api/v2/lab/panels                                   # list available panels
GET    /api/v2/lab/partners?pincode=...                    # find partners serving the patient pincode
POST   /api/v2/lab/orders                                  # create
PATCH  /api/v2/lab/orders/{id}                             # edit while still draft
POST   /api/v2/lab/orders/{id}/submit                      # lock + send to partner
POST   /api/v2/lab/orders/{id}/cancel
POST   /api/v2/lab/orders/{id}/reschedule
GET    /api/v2/lab/orders?patient_id=...&status=...
GET    /api/v2/lab/orders/{id}
POST   /api/v2/lab/orders/{id}/results                     # partner uploads
POST   /api/v2/lab/orders/{id}/verify                      # pathologist verifies
GET    /api/v2/lab/orders/{id}/report                      # PDF e-Report
GET    /api/v2/lab/results?patient_id=...&test_code=...    # trend query
GET    /api/v2/lab/samples/{barcode}                       # phlebotomist lookup
POST   /api/v2/lab/samples/{id}/temperature                # IoT logger callback
```

**4. Frontend.**
- **Doctor web (`/admin/lab/orders`):** order list with status pills; click → side panel showing each test, its result, abnormal flag, trend chart. The Rx writer has a "Add Lab Tests" tab; common panels (CBC, BMP, Lipid, Thyroid, HbA1c) are one-click; full LOINC search for uncommon tests.
- **Patient app:** `/lab/orders` shows the partner, the scheduled collection time, a tracking map when phlebotomist is en route, the e-Report PDF when ready, and a 7-day unlock countdown for the secure share link.
- **Partner dashboard** (separate product, not the main admin): `partner.mediclaim.in` style; partners see only their own orders; status update flow; PDF upload.

**5. Realtime.**
- `lab.order.{created|status_changed|completed}.{tenant}.{patient_id}` — patient app + doctor dashboard.
- `lab.phlebotomist.{assigned|en_route|arrived|collected}.{lab_order_id}` — patient tracking map.
- `lab.result.critical.{tenant}.{doctor_id}` — abnormal/critical result (e.g., HbA1c > 14, TSH < 0.01); paged immediately.
- `lab.report.ready.{tenant}.{patient_id}` — PDF generated, push to patient.

**6. Security & RBAC.** Roles: `doctor` (create + read), `phlebotomist` (collect sample + scan barcode + temperature), `pathologist` (verify + interpret), `lab_partner_admin` (partner-scoped orders only, no patient list outside their orders), `tenant_admin` (panel config, partner config, pricing). API keys for partners are bcrypt'd; raw key shown once at creation. All partner API calls logged in `lab_partner_api_calls` with request/response payloads (PHI-minimized: name+MRN, no full DOB or address unless the address is the collection address). Patient PII on e-Report PDF is per-tenant configurable (e.g., abha_id vs MRN). A webhook signature is verified on the partner callback (`X-Lab-Partner-Signature: HMAC-SHA256` over the body).

**7. Acceptance Criteria.**
- [ ] Doctor adds 3 lab tests to an Rx; `lab_orders` row is created with 3 `lab_order_items`; partner receives the order within 10s of Rx sign.
- [ ] Partner can only see orders where `lab_partner_id` matches their own; cross-partner data is forbidden (RBAC tested).
- [ ] Phlebotomist scans the barcode → status flips to `sample_collected` → patient tracking map shows "sample collected".
- [ ] Result with `abnormal_flag = 'HH'` triggers `lab.result.critical` realtime event within 5s.
- [ ] Pathologist can verify a result; without verification, the result is not visible in the patient app (stays "pending verification").
- [ ] LOINC codes are required on every `lab_order_item`; free-text test names are rejected at the API.
- [ ] Partner API failure retries with exponential backoff (3 attempts, 1s/4s/16s); persistent failure surfaces in the doctor dashboard.

**8. Real-World Test Scenarios.**
- *Courier swap:* partner A's network goes down; tenant admin reassigns pending orders to partner B; `lab_partner_id` updates and a re-submit is sent; no double-charge (atomic swap, audit row written).
- *Critical hemoglobin:* Hb 4.2 g/dL (low) — `lab.result.critical` fires; on-call hematologist paged; patient is sent to ED via `er-triage` (V4 §20.11) deep-link.
- *Home collection no-show:* phlebotomist arrives but patient is out; status moves to `missed`; partner dashboard flags; tenant can reschedule without re-charging.
- *Re-test for accuracy:* a TSH result is implausibly high (40 mIU/L); pathologist requests re-test; `lab_order_items.status='recollection'`; partner triggers free re-collection.
- *Audit:* regulator asks "which doctors ordered HbA1c tests in 2024, broken down by patient cohort"; query returns from `lab_orders` + `lab_order_items` joined to `patients`; the 30-day log shows 4,200 orders, 78% verified by 12 pathologists.

**9. Dev Workflow.** `php artisan lab:simulate --partner=thyrocare --orders=50` synthetic order load. `php artisan lab:webhook-replay --partner=1mg --from=...` replays a partner's webhook to recover from a missed callback. Code: `app-modules/pathology-lab/`; tests: `app-modules/pathology-lab/tests/Feature/{Order,Result,Partner,Report}Test.php` and `tests/Integration/PartnerWebhookTest.php` (replays a recorded 1mg webhook).

### 16.4 AI clinical scribe for Rx (`ai-clinical-scribe`, V2, MML 3)

**1. Overview.** During a consult, the doctor's phone mic or clinic room mic captures audio (with explicit two-party consent). At end of consult, the doctor taps "Stop & generate"; audio is uploaded to `medicare-ai-service` (per `00-Architecture §18.1` sidecar); STT produces a transcript; an LLM produces a structured SOAP note; the doctor reviews, edits, and signs. The signed SOAP note attaches to the encounter, and the **Plan section** auto-populates the Rx draft (§16.4 service). MML 3 at V2 GA covers: streaming STT, SOAP extraction, Rx plan-population, encounter-attached notes. MML 4 (V2.1) adds ICD-10/CPT coding suggestions. MML 5 (V3) adds speaker diarization and multi-language.

**2. Domain Model.**
```php
Schema::create('scribe_sessions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('encounter_id')->unique();
    $t->ulid('doctor_id')->index();
    $t->ulid('patient_id')->index();
    $t->string('status', 16)->default('recording');          // recording | uploading | transcribing | drafting | ready | signed | abandoned | failed
    $t->string('audio_s3_key', 255)->nullable();
    $t->integer('audio_duration_seconds')->nullable();
    $t->string('stt_model', 64)->nullable();                 // 'whisper-large-v3' | 'google-chirp' | 'azure-fast'
    $t->text('transcript')->nullable();
    $t->json('transcript_segments')->nullable();             // [{start, end, speaker, text, confidence}]
    $t->string('llm_model', 64)->nullable();                 // 'gpt-4o' | 'claude-sonnet-4.5' | 'qwen2.5:7b'
    $t->json('soap_draft')->nullable();                      // {subjective, objective, assessment, plan, confidence_per_section}
    $t->string('patient_consent_id', 26)->nullable();
    $t->string('doctor_consent_id', 26)->nullable();
    $t->integer('input_tokens')->default(0);
    $t->integer('output_tokens')->default(0);
    $t->decimal('cost_usd', 8, 6)->default(0);
    $t->string('failure_reason', 255)->nullable();
    $t->timestamp('started_at')->index();
    $t->timestamp('signed_at')->nullable();
    $t->timestamps();
    $t->softDeletes();
});

Schema::create('scribe_consents', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('scribe_session_id')->index();
    $t->string('party', 16);                                 // 'patient' | 'doctor'
    $t->ulid('user_id')->index();
    $t->boolean('consented')->default(false);
    $t->string('ip_address', 45)->nullable();
    $t->string('user_agent', 255)->nullable();
    $t->timestamp('consented_at')->nullable();
    $t->timestamps();
});
```

**3. API Surface.**
```
POST   /api/v2/ai/scribe/encounters/{encounter_id}/start     # provision session, return upload URL
POST   /api/v2/ai/scribe/encounters/{encounter_id}/upload    # multipart upload (or chunked presigned URLs)
POST   /api/v2/ai/scribe/encounters/{encounter_id}/consent   # patient consent
GET    /api/v2/ai/scribe/sessions/{id}                      # status + transcript (streamed SSE while transcribing)
PATCH  /api/v2/ai/scribe/sessions/{id}/soap                 # doctor edits
POST   /api/v2/ai/scribe/sessions/{id}/sign                 # lock SOAP to encounter
POST   /api/v2/ai/scribe/sessions/{id}/regenerate-section   # re-run LLM for one section
POST   /api/v2/ai/scribe/sessions/{id}/withdraw-consent     # delete audio + redact transcript
GET    /api/v2/ai/scribe/sessions/{id}/audio                # short-lived S3 URL (doctor audit)
```

**4. Frontend.** `/admin/encounters/{id}/scribe` is a side panel that opens when the doctor taps "AI scribe" in the encounter. The panel has: (a) consent banner (two checkboxes: patient + doctor), (b) recording controls (big red dot, pause, stop), (c) live transcript (token-streamed as STT completes), (d) SOAP editor with four text areas, (e) per-section "regenerate" + "low confidence" warning chips, (f) Rx draft auto-populated from the Plan section (per `AiScribeRxService::generateDraft`). The doctor must edit at least one character in each section before sign is enabled (forces human review).

**5. Realtime.**
- `scribe.transcript.token.{session_id}` — SSE stream of STT tokens (per `17-Realtime` §18).
- `scribe.section.ready.{session_id}.{section}` — fires when the LLM produces a section; UI unlocks the editor for that section.
- `scribe.signed.{encounter_id}` — final event; encounter is locked.
- `scribe.failed.{session_id}` — STT or LLM error; doctor can retry or fall back to manual.

**6. Security & RBAC.** Roles: `doctor` (own sessions only), `patient` (consent + view own transcript), `tenant_admin` (read-only audit), `compliance_officer` (read + export, no edit). Two-party consent is **enforced server-side**: the upload endpoint refuses chunks unless both `scribe_consents` rows are `consented=true`. The audio is stored in a tenant-scoped S3 prefix with SSE-KMS; it auto-deletes at 90 days (configurable). Withdraw-consent deletes the audio and redacts the transcript (replaces `[REDACTED]` in `transcript`, blanks `soap_draft`); a redaction audit row is written with `action='scribe.consent.withdrawn'`. Per `20-Security §11.2` and the prompt-injection mitigations, the LLM is given only the transcript and a structured patient-context (age, sex, allergies, active problems); no other chart fields. No patient PII is sent to a third-party LLM unless the tenant has explicitly configured that provider.

**7. Acceptance Criteria.**
- [ ] Recording cannot start until both patient and doctor consent.
- [ ] Withdraw-consent deletes audio and redacts transcript within 60s.
- [ ] Doctor cannot sign without editing at least one character in each SOAP section.
- [ ] STT first-token latency < 1.5s from end-of-speech.
- [ ] LLM SOAP draft per section < 8s after transcript is complete.
- [ ] Rx draft auto-populated from Plan section; each line carries `prescription_lines.source='ai'`.
- [ ] Audio auto-deletes at 90 days (verified by daily S3 lifecycle policy check).

**8. Real-World Test Scenarios.**
- *Mid-consent revocation:* doctor has consented, patient is on the line and revokes; recording stops; partial audio deleted; SOAP draft is blanked; encounter is converted to "manual" mode.
- *Hallucination guard:* transcript says "I think it's lung cancer"; LLM correctly produces `assessment: 'Possible lung malignancy — recommend CT chest'`; the doctor must edit "Possible" → "Suspected" before sign; UI forces the edit.
- *STT low quality:* doctor in a noisy ICU; STT returns `confidence < 0.6` on multiple segments; the UI marks those segments yellow; the SOAP draft is produced with low-confidence warnings on the corresponding sections.
- *Re-run a section:* doctor taps "Regenerate Assessment" with feedback "more specific, mention BNP value"; LLM re-runs; the new draft replaces the old; cost is recorded in `scribe_sessions.cost_usd`.
- *Cross-language:* patient speaks Hindi, doctor responds in English; STT is multilingual; transcript has `[hi]` and `[en]` markers; LLM produces SOAP in the doctor's selected output language.

**9. Dev Workflow.** `php artisan ai:scribe-simulate --audio=sample.wav --model=whisper-large-v3` synthetic STT. `php artisan ai:scribe-cost-report --month=2026-05` per-tenant token spend. Code: `app-modules/ai-clinical-scribe/`; tests: `app-modules/ai-clinical-scribe/tests/Feature/{Session,Consent,Soap,Rx}Test.php` and `tests/Performance/scribe-load.k6.js`.

### 16.5 Refill management (V2.1)

```php
Schema::create('prescription_refills', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('prescription_id')->index();
    $t->ulid('patient_id')->index();
    $t->integer('refill_number');                            // 1, 2, 3...
    $t->integer('refills_allowed');
    $t->string('status', 16)->default('pending');           // pending | approved | denied | dispensed
    $t->text('reason')->nullable();
    $t->ulid('approved_by_doctor_id')->nullable();
    $t->timestamp('approved_at')->nullable();
    $t->timestamps();
});
```

Patient requests a refill → doctor approves/denies → pharmacy dispenses. Pharmacy integration via SCRIPT (US) or national e-prescription network.

### 16.6 V2 API additions

```
POST   /api/v2/prescriptions/{id}/sign                     -> signed Rx with JWS
GET    /api/v2/prescriptions/{id}/fhir-bundle              -> FHIR R4 bundle
POST   /api/v2/prescriptions/{id}/upload-to-abdm           -> { fhir_id }
GET    /api/v2/drug-interactions?drugs=[...]               -> [interaction]
POST   /api/v2/prescriptions/{id}/lab-orders               -> lab_order
GET    /api/v2/lab-orders/{id}                             -> lab_order with results
POST   /api/v2/lab-orders/{id}/samples                     -> sample
POST   /api/v2/lab-orders/{id}/results                     -> [result]
GET    /api/v2/lab-orders/{id}/report                      -> PDF report
POST   /api/v2/lab-orders/{id}/cancel                      -> cancelled
POST   /api/v2/prescriptions/{id}/refills                  -> refill request
POST   /api/v2/prescription-refills/{id}/approve           -> approved
POST   /api/v2/prescription-refills/{id}/deny              -> denied
```

### 16.7 V2 acceptance criteria

- [ ] Drug interaction check returns correct results for 95%+ of common pairs (test set: 100 known interactions).
- [ ] ABDM e-prescription upload round-trips against NHA sandbox.
- [ ] Lab order flows end-to-end: doctor order → partner API → result → patient view.
- [ ] AI scribe produces a Rx draft that the doctor accepts with < 20% edit rate.
- [ ] Refill request requires doctor approval before pharmacy can dispense.
- [ ] All V2 Rx actions are auditable and broadcast.

---

## 17. V3+ Features — Specialty Rx Modules

> V1+V2 covers the generic prescription flow. V3+ adds **specialty-specific Rx modules** that turn the prescription into a **clinical decision support** platform.

### 17.1 Oncology Rx (`oncology-rx`, V3.1)

Chemo regimens, cycle scheduling, toxicity tracking, dose adjustments.

```php
Schema::create('chemo_regimens', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 128);                                // 'R-CHOP', 'FOLFOX', 'AC-T'
    $t->string('cancer_type_icd10', 16)->index();
    $t->string('phase', 16);                                 // 'induction' | 'consolidation' | 'maintenance'
    $t->integer('cycle_length_days');
    $t->integer('total_cycles');
    $t->json('drugs');                                       // [{drug, dose_per_m2, day_in_cycle, route, pre_meds?}]
    $t->json('supportive_meds');                            // anti-emetics, G-CSF, etc.
    $t->json('monitoring');                                  // [{test, day, threshold}]
    $t->string('evidence_grade', 4)->nullable();             // 'A' | 'B' | 'C' (NCCN grading)
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});

Schema::create('chemo_cycles', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('prescription_id')->index();
    $t->integer('cycle_number');
    $t->date('planned_start_date');
    $t->date('actual_start_date')->nullable();
    $t->date('planned_end_date');
    $t->date('actual_end_date')->nullable();
    $t->string('status', 16)->default('planned');            // 'planned' | 'started' | 'completed' | 'delayed' | 'reduced' | 'discontinued'
    $t->json('dose_modifications')->nullable();             // [{drug, planned_dose, actual_dose, reason}]
    $t->json('toxicity_grade')->nullable();                 // CTCAE v5: {nausea: 2, fatigue: 1, neutropenia: 0}
    $t->ulid('prescribing_oncologist_id')->index();
    $t->timestamps();
});
```

**Dose calculation:** based on BSA (Mosteller formula: `√(height_cm × weight_kg / 3600)`) or AUC for carboplatin (Calvert formula with GFR).

**Pre-medication checks:** e.g., for cisplatin, need adequate hydration and anti-emetics; for rituximab, need pre-meds to prevent infusion reaction.

**Cycle delay rules:** if ANC < 1500, platelets < 100k, or creatinine > 2× baseline, delay cycle by 1 week. System auto-flags.

**Toxicity tracking:** CTCAE v5 grading (1=mild, 5=death); entered by nurse at each cycle.

### 17.2 Mental health Rx (`mental-health-rx`, V3.1)

Validated screeners, suicide risk assessment, controlled substance monitoring.

```php
Schema::create('mental_health_screeners', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->ulid('doctor_id')->nullable();
    $t->string('screener_slug', 32);                        // 'phq9', 'gad7', 'cssrs', 'audit', 'dast10', 'mdq'
    $t->json('responses');                                  // {q1: 2, q2: 0, ...}
    $t->integer('total_score');
    $t->string('severity', 32);                             // 'minimal' | 'mild' | 'moderate' | 'severe'
    $t->boolean('suicidal_ideation_flagged')->default(false);
    $t->text('clinical_notes')->nullable();
    $t->timestamp('administered_at')->index();
    $t->timestamps();
});

Schema::create('controlled_substance_log', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->ulid('prescription_id')->index();
    $t->string('drug', 128);
    $t->string('dea_schedule', 4);                          // 'II' | 'III' | 'IV' | 'V'
    $t->decimal('dose', 8, 2);
    $t->string('unit', 16);
    $t->integer('quantity');
    $t->integer('refills');
    $t->date('issued_date');
    $t->date('expires_date');
    $t->string('state_pmp_query_id', 64)->nullable();       // prescription monitoring program
    $t->json('pmp_result')->nullable();                     // {other_prescribers: [...], pharmacy_visits: [...]}
    $t->ulid('prescriber_id');
    $t->timestamps();
});
```

**PMP (Prescription Monitoring Program) integration:** before prescribing Schedule II-V, query the state PMP (US) or equivalent. Flags if patient has multiple prescribers or early refills.

**Suicide risk:** PHQ-9 item 9 ("thoughts that you would be better off dead or of hurting yourself in some way") > 0 → automatic C-SSRS administration; positive C-SSRS → Stanley-Brown Safety Plan auto-generated, urgent referral.

**Taper schedules:** for benzodiazepines, opioids, SSRIs — automatic taper plan generation based on current dose and duration.

### 17.3 Pediatric Rx (`pediatric-rx`, V3.1)

Weight-based dosing, age-appropriate formulations, parent counseling.

```php
Schema::create('pediatric_dosing_rules', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->nullable();
    $t->string('drug', 128);
    $t->integer('min_age_months');
    $t->integer('max_age_months');
    $t->decimal('dose_per_kg', 8, 4);
    $t->decimal('max_single_dose', 8, 4);
    $t->string('dose_unit', 16);                            // 'mg' | 'ml'
    $t->integer('frequency_per_day');
    $t->integer('max_duration_days')->nullable();
    $t->string('formulation', 32);                          // 'syrup', 'chewable', 'drops'
    $t->string('evidence_source', 64)->nullable();          // 'WHO', 'Nelson', 'Harriet Lane'
    $t->timestamps();
});
```

**Dose calculator:** `Dose = min(weight_kg × dose_per_kg, max_single_dose)`. Returns hard error if age out of range or weight missing.

**Age warnings:** tetracyclines <8 years (tooth staining), aspirin <18 (Reye's), fluoroquinolones <18 (cartilage damage).

**Liquid formulation:** auto-suggest concentration based on age (e.g., amoxicillin 125mg/5mL for <5y, 250mg/5mL for older).

**Weight tracking:** recent weight from `patient_vitals`; if not recorded in last 30 days, prompt nurse to weigh before prescribing.

### 17.4 Diabetes Rx (`diabetes-rx`, V3.1)

Insulin dosing, CGM integration, hypoglycemia tracking.

```php
Schema::create('insulin_regimens', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->string('regimen_type', 32);                         // 'basal_bolus', 'premixed', 'sliding_scale', 'pump'
    $t->json('basal_insulin')->nullable();                  // {drug, total_daily_dose, schedule}
    $t->json('bolus_ratios')->nullable();                   // {carbs: 1u/10g, correction: 1u/50mg_dL_above_150}
    $t->json('sensitivity_factors')->nullable();
    $t->json('target_ranges')->nullable();                  // {pre_meal: [80,130], post_meal: [80,180], bedtime: [100,140]}
    $t->decimal('hba1c_target', 4, 1)->nullable();
    $t->date('start_date');
    $t->date('end_date')->nullable();
    $t->ulid('prescriber_id');
    $t->timestamps();
});

Schema::create('cgm_readings', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->string('source', 32);                              // 'dexcom', 'libre', 'medtronic'
    $t->integer('glucose_mg_dl')->index();
    $t->string('trend', 16);                               // 'rising_fast', 'rising', 'stable', 'falling', 'falling_fast'
    $t->timestamp('measured_at')->index();
    $t->string('sensor_id', 64);
    $t->index(['patient_id', 'measured_at']);
});

Schema::create('hypoglycemia_events', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->integer('glucose_mg_dl');
    $t->string('severity', 16);                            // 'level_1' (54-70), 'level_2' (<54), 'level_3' (altered_consciousness)
    $t->string('treatment', 64);                            // 'glucose_tablets', 'juice', 'glucagon', 'er'
    $t->string('cause', 64)->nullable();                   // 'exercise', 'missed_meal', 'insulin_dose_error', 'unknown'
    $t->boolean('required_assistance')->default(false);
    $t->text('notes')->nullable();
    $t->timestamp('occurred_at')->index();
});
```

**CGM auto-sync:** hourly cron pulls from Dexcom/Libre API. Charts: AGP (Ambulatory Glucose Profile), time-in-range, GMI (Glucose Management Indicator).

**Insulin dose calculator:** based on carbs consumed and current glucose, returns units to dose. Includes safety caps (max 10u bolus, no dose if glucose < 70).

**Hypoglycemia alert:** reading < 54 mg/dL → push to patient + designated caregiver + (if severe) doctor's on-call.

### 17.5 Cardiology Rx (`cardiology-rx`, V3.1)

Anticoagulation, ECG-aware prescribing, ICD-10-driven dosing.

```php
Schema::create('anticoagulation_log', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->ulid('prescription_id')->index();
    $t->string('drug', 64);                                // 'warfarin', 'apixaban', 'rivaroxaban', 'dabigatran'
    $t->decimal('dose', 8, 2);
    $t->string('unit', 16);
    $t->date('start_date');
    $t->date('last_inr_date')->nullable();                 // for warfarin
    $t->decimal('last_inr_value', 4, 2)->nullable();
    $t->decimal('target_inr_low', 3, 1)->default(2.0);
    $t->decimal('target_inr_high', 3, 1)->default(3.0);
    $t->json('dose_history')->nullable();                  // [{date, dose, inr, adjustment}]
    $t->string('indication', 64);                          // 'afib', 'dvt', 'mechanical_valve'
    $t->json('interactions_checked');
    $t->timestamps();
});
```

**Warfarin dose adjustment:** based on INR trend (Rosendaal method, %TTR). If INR > 3.5, hold 1 dose, recheck in 3 days. If INR < 1.5, increase by 10-20%. If 2 in a row out of range, refer to anticoag clinic.

**DOAC selection:** for non-valvular AFib, prefer apixaban 5mg BID (2.5 if 2 of 3: age ≥80, weight ≤60kg, creatinine ≥1.5).

**Drug interactions:** NSAIDs + DOAC (bleeding), amiodarone + warfarin (CYP), etc.

### 17.6 Dental Rx (`dental-rx`, V3.1)

Per-tooth treatment plans, dental-specific drug dosing.

```php
Schema::create('dental_treatment_plans', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->ulid('prescription_id')->nullable()->index();
    $t->json('teeth');                                      // FDI numbering: {11: {condition: 'caries', treatment: 'composite', status: 'planned'}}
    $t->decimal('estimated_cost', 12, 2);
    $t->string('priority', 16);                            // 'urgent' | 'high' | 'medium' | 'low'
    $t->text('notes')->nullable();
    $t->timestamps();
});
```

**Common dental Rx:**
- Amoxicillin 500mg TID × 7d (endocarditis prophylaxis: 2g 30 min before procedure).
- Metronidazole 400mg TID × 5d (anaerobic infection).
- Ibuprofen 400mg QID (pain).
- Chlorhexidine 0.12% mouthwash BID (post-surgical).

**Drug interactions:** with patient's medical history (e.g., metronidazole + warfarin → bleeding risk).

### 17.7 Compounding pharmacy Rx (`compounding-rx`, V3.1)

Custom formulations (pediatric liquids, dermatology creams, hormone therapy).

```php
Schema::create('compounding_formulas', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 128);
    $t->json('ingredients');                                // [{drug, amount, unit, role: 'active' | 'excipient'}]
    $t->string('dosage_form', 32);                          // 'cream', 'ointment', 'solution', 'suspension', 'capsule'
    $t->string('route', 32);
    $t->json('stability_data')->nullable();                 // beyond-use date, storage conditions
    $t->json('preparation_instructions')->nullable();
    $t->timestamps();
});
```

**Beyond-use dating:** USP <795> for non-sterile, <797> for sterile. Defaults: aqueous = 14d refrigerated, non-aqueous = 30d room temp, sterile = varies (7-30d).

### 17.8 Veterinary Rx (`vet-rx`, separate fork, V3.1)

Species-aware, weight-based, owner-proxy.

```php
Schema::create('vet_consultations', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();                       // patient is the animal
    $t->ulid('owner_id')->index();                          // owner (proxy)
    $t->string('species', 32);                              // 'dog', 'cat', 'horse', 'bird', 'reptile'
    $t->string('breed', 64)->nullable();
    $t->decimal('weight_kg', 6, 2);
    $t->string('age_years', 16);                            // free-text: '3y', '6mo'
    $t->boolean('is_pregnant')->default(false);
    $t->boolean('is_lactating')->default(false);
    $t->json('conditions');
    $t->json('medications');
    $t->timestamps();
});
```

**Drug clearance varies by species:** some drugs safe in dogs are toxic in cats (e.g., acetaminophen). Specific dose calculators per species.

**Owner counseling:** the owner is the proxy for consent and administration; counseling notes go to the owner, not the patient.

### 17.9 Homeopathy / Ayurveda Rx (`alt-rx`, V3.1)

Prakriti assessment, classical formulations, classical references.

```php
Schema::create('alt_med_formulations', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->string('system', 16);                              // 'ayurveda', 'homeopathy', 'siddha', 'unani'
    $t->string('name', 128);
    $t->json('ingredients');                                // for ayurveda: herbs with Sanskrit + botanical names
    $t->string('reference_text', 128);                      // 'Charaka Samhita', 'Hahnemann Materia Medica'
    $t->string('potency', 32)->nullable();                  // for homeopathy: '30C', '200C', '1M'
    $t->string('classical_use', 512)->nullable();
    $t->boolean('is_classical_reference')->default(true);
    $t->timestamps();
});
```

**Disclaimer required:** "This is a complementary therapy. Not a substitute for evidence-based medicine. Consult your primary doctor for serious conditions."

### 17.10 Clinical decision support engine (`cdse`, V3.1)

Cross-cutting rule engine that fires on every Rx write:

```php
class ClinicalDecisionSupport {
    private array $rules = [
        // Renal dosing
        new RenalDosingRule(),
        // Hepatic dosing
        new HepaticDosingRule(),
        // Age-based (geriatric, pediatric)
        new BeersCriteriaRule(),                            // potentially inappropriate meds in elderly
        new PediatricDosingRule(),
        // Pregnancy (FDA A/B/C/D/X)
        new PregnancyCategoryRule(),
        // Lactation
        new LactationSafetyRule(),                           // LactMed database
        // Allergy
        new AllergyCrossSensitivityRule(),
        // Therapeutic duplication
        new TherapeuticDuplicationRule(),                   // two drugs same class
        // Drug-drug (already V2)
        new DrugDrugInteractionRule(),
        // Drug-food
        new DrugFoodInteractionRule(),                       // grapefruit + statins
        // Pharmacogenomic (V4)
        // new PharmacogenomicRule(),
    ];

    public function check(Prescription $rx): array {
        $alerts = [];
        foreach ($this->rules as $rule) {
            $alerts = array_merge($alerts, $rule->evaluate($rx));
        }
        // Sort by severity
        usort($alerts, fn($a, $b) => $a->severity->compareTo($b->severity));
        return $alerts;
    }
}
```

**Beers Criteria:** American Geriatrics Society list of PIM (potentially inappropriate medications) for adults 65+. Examples: benzodiazepines, anticholinergics, sliding-scale insulin.

**Pharmacogenomic (V4):** based on patient's genetic profile (CYP2D6, CYP2C19, HLA-B*5701), adjust drug choice. E.g., codeine + CYP2D6 poor metabolizer → no effect, switch to morphine.

---

## 18. Test Matrix (Unit to Real-World) — Prescriptions

### 18.1 Unit tests (~280 tests)

#### Model tests (70 tests)
- `PrescriptionModelTest` (30): casts, mutators, signing lock, edit window, soft delete
- `PrescriptionLineTest` (15): dose validation, frequency parsing, route enum
- `MedicineTest` (10): global scope, typeahead
- `MedicineInteractionTest` (5): severity enum
- `RefillTest` (5): state transitions, approval
- `LabOrderTest` (5): status enum, partner validation

#### Drug interaction tests (40 tests)
```php
test('warfarin + aspirin: major bleeding', function () {...});
test('metformin + contrast: hold 48h', function () {...});
test('simvastatin + clarithromycin: rhabdomyolysis', function () {...});
test('MAOI + SSRI: serotonin syndrome', function () {...});
test('no interaction: paracetamol + ibuprofen', function () {...});
test('severity rank: contraindicated > major > moderate > minor', function () {...});
// ... 34 more pairs from FDA database
```

#### Prescription signing tests (30 tests)
```php
test('sign locks the prescription', function () {...});
test('sign records IP, user agent, timestamp', function () {...});
test('sign with expired edit window requires amendment', function () {...});
test('amendment creates new version, preserves original', function () {...});
test('amendment chain is auditable', function () {...});
// ...
```

#### AI scribe tests (40 tests)
```php
test('scribe extracts medicines from SOAP plan', function () {...});
test('scribe extracts lab tests from SOAP plan', function () {...});
test('scribe hallucinates less than 5% on test set', function () {...});
test('scribe requires doctor sign-off', function () {...});
test('scribe prompt injection blocked', function () {...});
// ...
```

#### E-prescription format tests (20 tests)
```php
test('ABDM FHIR bundle is valid', function () {...});
test('SCRIPT 2017071 NCPDP XML is valid', function () {...});
test('e-Rx signature JWS verifies', function () {...});
test('e-Rx QR code scans to FHIR URL', function () {...});
// ...
```

#### Refill tests (15 tests)
```php
test('refill requires doctor approval', function () {...});
test('refill denied if no refills remaining', function () {...});
test('refill denied if original Rx expired', function () {...});
// ...
```

#### Lab order tests (25 tests)
```php
test('lab order creates with 1+ items', function () {...});
test('lab order status transitions: draft → submitted → sample_collected → in_lab → completed', function () {...});
test('lab result abnormal flag triggers push to doctor', function () {...});
test('lab partner API key encrypted at rest', function () {...});
// ...
```

#### Specialty tests (40 tests)
- Oncology: regimen parsing, cycle scheduling, dose calculation
- Mental health: screener scoring, suicidal ideation flag, controlled substance
- Pediatric: weight-based dose, age warnings
- Diabetes: insulin calculator, hypoglycemia severity
- Cardiology: warfarin dose adjustment, INR tracking

### 18.2 Integration tests (~100 tests)

```php
test('Rx + drug interaction check is atomic', function () {...});
test('Rx + lab order create both in single transaction', function () {...});
test('AI scribe audio → STT → SOAP → Rx draft is idempotent', function () {...});
test('ABDM upload signed bundle to NHA gateway', function () {...});
test('pharmacy dispense callback updates Rx status', function () {...});
// ...
```

### 18.3 HTTP tests (~120 tests)

Every endpoint, every role, every error:
```php
test('POST /prescriptions as doctor returns 201', function () {...});
test('POST /prescriptions as patient returns 403', function () {...});
test('POST /prescriptions with interaction returns 422 with alert', function () {...});
test('POST /prescriptions/{id}/sign returns 200', function () {...});
test('PATCH /prescriptions/{id} after 24h returns 403', function () {...});
test('POST /prescriptions/{id}/amend creates new version', function () {...});
test('GET /prescriptions/{id}/pdf returns PDF stream', function () {...});
test('POST /prescriptions/{id}/share generates 72h URL', function () {...});
test('GET /share/{token} increments access count', function () {...});
test('POST /prescriptions/{id}/lab-orders creates lab order', function () {...});
test('POST /prescriptions/{id}/refills patient requests', function () {...});
test('POST /prescription-refills/{id}/approve doctor', function () {...});
// ... 100+ more
```

### 18.4 E2E tests (Playwright, ~20 tests)

```typescript
test('doctor writes Rx from completed appointment', async ({ page }) => {...});
test('patient views Rx, shares via link', async ({ page }) => {...});
test('pharmacy scans QR, verifies signature, dispenses', async ({ page }) => {...});
test('AI scribe drafts Rx from audio, doctor signs', async ({ page }) => {...});
test('drug interaction alert blocks sign', async ({ page }) => {...});
test('lab partner receives order, uploads result', async ({ page }) => {...});
// ...
```

### 18.5 Performance tests (k6, ~5 tests)
- `prescription_create.js` — 100 concurrent creates, p99<400ms
- `prescription_pdf.js` — 1000 PDFs/min generation
- `drug_interaction_check.js` — 100 concurrent checks, p99<200ms
- `ai_scribe_stream.js` — token streaming, first token <2s
- `abdm_upload.js` — 100 concurrent uploads, p99<3s

### 18.6 Security tests (~15 tests)
- SQLi in medicine name
- XSS in handwritten notes (PNG, sanitized)
- Tampered signed PDF
- Refill IDOR
- ABDM token replay
- Pharmacy auth bypass
- Allergy cross-sensitivity
- Audit log for every prescription access

### 18.7 Real-world scenarios

| Scenario | Services | Pass criteria |
|---|---|---|
| ABDM upload | NHA sandbox | FHIR bundle accepted, IRN returned |
| RxNav drug interaction | NIH RxNav | 100% match for known pairs |
| LiveKit scribe transcription | OpenAI Whisper | SOAP note readable |
| GPT-4 scribe | OpenAI API | < 20% edit rate |
| 1mg lab partner | 1mg mock | Order → result flow |
| Pharmacy dispense callback | SureScripts mock | Status updated |
| CardioWarfarin dose | Internal | INR trend + adjustment correct |
| Pediatric amoxicillin | Internal | Dose = 25-50 mg/kg/day |

### 18.8 Coverage targets

| Layer | Target |
|---|---|
| Models | 95% |
| Services | 95% |
| AI services | 80% (LLMs are non-deterministic) |
| E-prescription formatters | 100% |
| Controllers | 90% |
| Jobs | 95% |
| Overall | 90% |

### 18.9 Total test count

~580 tests defined across all levels for the Prescriptions module.

---

## 19. Deep Dive — Drug Interaction Engine

> The drug interaction engine is the **safety-critical** core of the prescription module. False negatives can kill patients. False positives cause alert fatigue. Both are unacceptable.

### 19.1 Data sources

| Source | Coverage | License | Update frequency | Size |
|---|---|---|---|---|
| **RxNav / NDF-RT** | US, free, NIH | Public | Weekly | 100K drugs, 14K interactions |
| **DrugBank** | Global, commercial | Per-institution license | Daily | 14K drugs, 1.4M interactions |
| **WHO Essential Medicines** | Global, free | Public | Annual | 500 drugs, 5K interactions |
| **FDA AERS** | US, free | Public | Quarterly | Adverse event signals |
| **EMA EudraVigilance** | EU, free | Public | Monthly | Adverse event signals |
| **Local formulary** | Tenant-specific | Internal | Manual | Per-tenant curated |

### 19.2 Schema

```php
Schema::create('drug_interactions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->string('drug_a_rxnorm', 16)->index();
    $t->string('drug_b_rxnorm', 16)->index();
    $t->string('severity', 16);                            // 'minor' | 'moderate' | 'major' | 'contraindicated'
    $t->string('mechanism', 255)->nullable();
    $t->text('description');
    $t->json('evidence')->nullable();                      // [{source, citation, study_ref, evidence_level}]
    $t->string('onset', 16)->nullable();                   // 'rapid' | 'delayed'
    $t->string('management', 512)->nullable();             // what to do
    $t->json('alternatives')->nullable();                  // [{drug_rxnorm, note}]
    $t->boolean('is_active')->default(true)->index();
    $t->timestamps();
    $t->unique(['drug_a_rxnorm', 'drug_b_rxnorm']);
});
```

### 19.3 Resolution

```php
class DrugInteractionService {
    public function check(array $medicines, ?Patient $patient = null): Collection {
        $rxnorms = collect($medicines)
            ->map(fn($m) => $m->medicine->rxnorm_code)
            ->filter()
            ->unique()
            ->values();
        
        if ($rxnorms->count() < 2) {
            return collect();
        }

        // Generate all pairs
        $pairs = [];
        for ($i = 0; $i < $rxnorms->count(); $i++) {
            for ($j = $i + 1; $j < $rxnorms->count(); $j++) {
                $pairs[] = [$rxnorms[$i], $rxnorms[$j]];
            }
        }

        // Query interactions (both A→B and B→A)
        $interactions = DrugInteraction::where(function ($q) use ($pairs) {
            foreach ($pairs as [$a, $b]) {
                $q->orWhere(function ($q2) use ($a, $b) {
                    $q2->where('drug_a_rxnorm', $a)->where('drug_b_rxnorm', $b);
                })->orWhere(function ($q2) use ($a, $b) {
                    $q2->where('drug_a_rxnorm', $b)->where('drug_b_rxnorm', $a);
                });
            }
        })->where('is_active', true)->get();

        // Add patient-context interactions
        if ($patient) {
            $interactions = $interactions->merge(
                $this->checkPatientContext($patient, $medicines)
            );
        }

        // Sort by severity
        return $interactions->sortByDesc(fn($i) => $i->severity_rank);
    }

    private function checkPatientContext(Patient $patient, Collection $medicines): Collection {
        $alerts = collect();

        // Allergies
        foreach ($patient->allergies ?? [] as $allergy) {
            foreach ($medicines as $line) {
                if ($this->crossReactive($allergy['substance'], $line->medicine)) {
                    $alerts->push(new InteractionAlert(
                        severity: 'contraindicated',
                        message: "Patient is allergic to {$allergy['substance']}; {$line->medicine->name} is cross-reactive.",
                    ));
                }
            }
        }

        // Renal dosing
        if ($patient->latest_egfr && $patient->latest_egfr < 60) {
            foreach ($medicines as $line) {
                if ($rule = $this->renalDosingRule($line->medicine, $patient->latest_egfr)) {
                    $alerts->push($rule);
                }
            }
        }

        // Pregnancy
        if ($patient->is_pregnant) {
            foreach ($medicines as $line) {
                $cat = $this->fdaCategory($line->medicine);
                if (in_array($cat, ['D', 'X'])) {
                    $alerts->push(new InteractionAlert(
                        severity: 'contraindicated',
                        message: "{$line->medicine->name} is FDA category {$cat}; contraindicated in pregnancy.",
                    ));
                }
            }
        }

        return $alerts;
    }
}
```

### 19.4 Doctor UX

```vue
<template>
  <div v-for="alert in alerts" :class="['alert', alert.severity]">
    <strong>{{ alert.severity.toUpperCase() }}:</strong> {{ alert.message }}
    <details>
      <summary>Details</summary>
      <p>{{ alert.description }}</p>
      <p>Management: {{ alert.management }}</p>
      <p v-if="alert.alternatives">Alternatives: {{ alert.alternatives.join(', ') }}</p>
    </details>
    <label>
      <input type="checkbox" v-model="acknowledged[alert.id]" />
      I have reviewed this alert and choose to proceed.
    </label>
  </div>
  <button :disabled="!allAcknowledged" @click="sign">Sign Rx</button>
</template>
```

### 19.5 Performance

- 10-line Rx: 45 pairs to check.
- With proper index on `(drug_a_rxnorm, drug_b_rxnorm)`: <50ms.
- Cached for 1 hour per (line list) — invalidates on Rx change.

### 19.6 False positive reduction

- **Tier system:** only show 'major' and 'contraindicated' by default; 'moderate' behind a "Show all" toggle.
- **Patient context:** pregnancy, renal, hepatic only shown if relevant.
- **Acknowledged:** if doctor has previously acknowledged the same alert for the same drug pair in the last 30 days, suppress.
- **Formulary preferences:** if hospital formulary has a preferred alternative, show "Consider switching to X" instead of just blocking.

### 19.7 Continuous improvement

- **Alert feedback:** doctor can mark alert as "false positive" → we track and report.
- **Override rate:** if override rate > 50% for an alert, it's a candidate for downgrading.
- **Outcome tracking:** if patient on the drug pair has an adverse event, it's a signal to upgrade.
- **Quarterly review:** by a clinical pharmacist.

---

## 20. Deep Dive — E-Prescription Signing

> National e-prescription formats are legally binding. Signing must be cryptographically secure and standards-compliant.

### 20.1 ABDM (India) FHIR Bundle

```php
class AbdmPrescriptionFormatter {
    public function format(Prescription $rx): array {
        $patient = $rx->patient;
        $doctor = $rx->doctor;
        
        return [
            'resourceType' => 'Bundle',
            'id' => "rx-{$rx->id}",
            'meta' => [
                'profile' => ['http://hl7.org/fhir/StructureDefinition/Bundle'],
                'lastUpdated' => $rx->signed_at->toIso8601String(),
            ],
            'type' => 'collection',
            'timestamp' => $rx->signed_at->toIso8601String(),
            'entry' => [
                $this->patientResource($patient),
                $this->practitionerResource($doctor),
                $this->organizationResource($doctor->clinic),
                $this->encounterResource($rx->appointment),
                ...$rx->lines->map(fn($line) => $this->medicationRequestResource($line, $patient, $doctor))->all(),
            ],
            'signature' => $this->sign($rx),
        ];
    }

    private function medicationRequestResource(PrescriptionLine $line, Patient $patient, Doctor $doctor): array {
        return [
            'resource' => [
                'resourceType' => 'MedicationRequest',
                'id' => "med-{$line->id}",
                'status' => 'active',
                'intent' => 'order',
                'medicationCodeableConcept' => [
                    'coding' => [
                        [
                            'system' => 'http://www.nlm.nih.gov/research/umls/rxnorm',
                            'code' => $line->medicine->rxnorm_code,
                            'display' => $line->medicine->name,
                        ],
                    ],
                ],
                'subject' => ['reference' => "Patient/{$patient->id}"],
                'authoredOn' => $line->prescription->signed_at->toIso8601String(),
                'requester' => ['reference' => "Practitioner/{$doctor->id}"],
                'dosageInstruction' => [
                    [
                        'text' => $line->frequency,
                        'timing' => [
                            'repeat' => [
                                'frequency' => $line->frequency_per_day,
                                'period' => 1,
                                'periodUnit' => 'd',
                            ],
                        ],
                        'doseAndRate' => [
                            [
                                'doseQuantity' => [
                                    'value' => (float)$line->dose,
                                    'unit' => $line->dose_unit,
                                ],
                            ],
                        ],
                        'duration' => $line->duration_days,
                        'route' => ['text' => $line->route],
                    ],
                ],
                'dispenseRequest' => [
                    'quantity' => ['value' => $line->quantity],
                    'numberOfRepeatsAllowed' => $line->refills,
                ],
            ],
        ];
    }

    private function sign(Prescription $rx): array {
        $payload = json_encode([
            'bundle_id' => "rx-{$rx->id}",
            'doctor_id' => $rx->doctor_id,
            'patient_id' => $rx->patient_id,
            'signed_at' => $rx->signed_at->toIso8601String(),
            'content_hash' => hash('sha256', json_encode($rx->toArray())),
        ]);
        $signature = $this->dsaService->sign($payload, $rx->doctor->signature_key);
        return [
            'type' => [['system' => 'urn:ietf:rfc:6881', 'code' => '1.2.840.10065.1.12.1.1']],
            'when' => $rx->signed_at->toIso8601String(),
            'who' => ['reference' => "Practitioner/{$rx->doctor_id}"],
            'data' => base64_encode($signature),
        ];
    }
}
```

### 20.2 Signature verification

```php
class PrescriptionSignatureVerifier {
    public function verify(Prescription $rx): bool {
        $payload = json_encode([
            'bundle_id' => "rx-{$rx->id}",
            'doctor_id' => $rx->doctor_id,
            'patient_id' => $rx->patient_id,
            'signed_at' => $rx->signed_at->toIso8601String(),
            'content_hash' => hash('sha256', json_encode($rx->toArray())),
        ]);
        $signature = base64_decode($rx->signature_jws);
        return $this->dsaService->verify($payload, $signature, $rx->doctor->signature_public_key);
    }
}
```

### 20.3 QR code generation

```php
class PrescriptionQrService {
    public function generate(Prescription $rx): string {
        $payload = json_encode([
            'rx' => $rx->id,
            'patient' => substr($rx->patient->abha_id ?? $rx->patient->mrn, 0, 4) . '****',
            'doctor' => $rx->doctor->name,
            'drugs' => $rx->lines->map(fn($l) => $l->medicine->name)->implode(', '),
            'verify' => route('verify.public', ['rx' => $rx->id, 'token' => $this->signForQr($rx)]),
        ]);
        return QrCode::format('png')->size(300)->generate($payload);
    }

    public function signForQr(Prescription $rx): string {
        return hash_hmac('sha256', "rx:{$rx->id}", config('app.key'));
    }
}
```

### 20.4 Compliance

- ABDM M2/M3 certification requires successful round-trip with NHA gateway.
- DSC (Digital Signature Certificate) of the doctor must be ABDM-registered.
- Each prescription is uploaded to NHA HIU (Health Information User).
- Patient can view all their prescriptions on the ABDM health locker.

---

## 21. Deep Dive — Refill Workflow

> Refills are a common source of medication errors. The system enforces checks at every step.

### 21.1 Patient requests refill

```php
class RefillRequestService {
    public function request(Prescription $original, Patient $patient): PrescriptionRefill {
        // 1. Verify original is refillable
        if ($original->refills_remaining <= 0) {
            throw new NoRefillsRemainingException;
        }
        if ($original->expires_at < now()) {
            throw new RxExpiredException;
        }
        if ($original->status !== 'active') {
            throw new RxNotActiveException;
        }
        if ($original->patient_id !== $patient->id) {
            throw new UnauthorizedException;
        }

        // 2. Check for changes since last dispense
        $lastDispense = $original->refills()->where('status', 'dispensed')->latest()->first();
        if ($lastDispense) {
            $newConditions = $this->getNewConditions($patient, $lastDispense);
            if ($newConditions) {
                $this->notifyDoctorOfChanges($original, $newConditions);
            }
        }

        // 3. Create refill request
        return PrescriptionRefill::create([
            'prescription_id' => $original->id,
            'patient_id' => $patient->id,
            'refill_number' => $original->refills_used + 1,
            'status' => 'pending',
        ]);
    }
}
```

### 21.2 Doctor approval

```php
class RefillApprovalService {
    public function approve(PrescriptionRefill $refill, Doctor $doctor): void {
        // 1. Verify doctor has relationship
        if (!$doctor->hasRelationshipWith($refill->patient)) {
            throw new UnauthorizedException;
        }

        // 2. Re-check drug interactions with current patient meds
        $currentMeds = $refill->patient->activeMedications;
        $refillMeds = $refill->prescription->lines;
        $interactions = $this->interactionService->check(
            array_merge($currentMeds->all(), $refillMeds->all())
        );
        if ($interactions->where('severity', 'contraindicated')->isNotEmpty()) {
            // Auto-deny with reason
            $refill->update(['status' => 'denied', 'reason' => 'contraindicated_with_current_meds']);
            $this->notifyPatient($refill);
            return;
        }

        // 3. Approve
        $refill->update([
            'status' => 'approved',
            'approved_by_doctor_id' => $doctor->id,
            'approved_at' => now(),
        ]);

        // 4. Generate new Rx (or extend original)
        $newRx = $this->createRefillRx($refill);

        // 5. Notify pharmacy (if integrated)
        $this->pharmacyService->notify($newRx);

        // 6. Notify patient
        $this->notifyPatient($refill);
    }
}
```

### 21.3 Pharmacy dispense

Pharmacy calls back when dispensed:

```php
class PharmacyCallbackController {
    public function dispenseCallback(Request $request, Prescription $rx): JsonResponse {
        // Verify pharmacy signature
        if (!$this->verifyPharmacySignature($request)) {
            return response()->json(['error' => 'invalid_signature'], 401);
        }
        
        $rx->update([
            'status' => 'dispensed',
            'dispensed_at' => now(),
            'dispensing_pharmacy_id' => $request->input('pharmacy_id'),
            'dispensing_pharmacist_id' => $request->input('pharmacist_id'),
        ]);

        $refill = $rx->refill;
        $refill->update(['status' => 'dispensed', 'dispensed_at' => now()]);

        // Decrement refills remaining
        $rx->decrement('refills_remaining');

        return response()->json(['ok' => true]);
    }
}
```

### 21.4 Edge cases

- Patient loses medication → doctor can re-issue without refill decrement (with reason).
- Patient hospitalized → all active Rx are auto-paused.
- New allergy discovered → Rx auto-flagged for review.
- Drug recall → all matching Rx flagged, patient notified, pharmacy asked to return.
- Doctor leaves practice → refills transfer to covering doctor (with patient consent).

---

## 22. Precise Plan — Prescriptions Module

### 22.1 Phase breakdown

#### V1 (Month 0)
- Rx CRUD, signing, PDF, share, templates, written-style
- Drug autocomplete (basic)
- Audit + broadcast

#### V1.1 (Month 0–2)
- Drug interaction check (RxNav)
- AI scribe (Whisper + GPT-4o)
- E-prescription ABDM
- Refill workflow
- Lab orders in Rx

#### V2 (Month 4–7)
- National e-prescription formats (SCRIPT, GStandard, EPS)
- Pharmacy integration (SureScripts)
- DICOM imaging orders
- Clinical decision support (renal, hepatic, age, pregnancy, lactation)
- Beers Criteria, lactation safety

#### V2.1 (Month 7–10)
- Pharmacogenomics (V4 actually, deferred)
- Compounding pharmacy
- Clinical trials Rx

#### V3 (Month 10–15)
- Specialty modules:
  - Oncology (chemo regimens, cycles, toxicity)
  - Mental health (PMP, controlled substance, suicide risk)
  - Pediatric (weight-based, age warnings)
  - Diabetes (insulin calculator, CGM)
  - Cardiology (anticoagulation, INR)
  - Dental
- Alternative medicine (Ayurveda, homeopathy)
- Veterinary (fork)

#### V4 (Month 15–22)
- Hospital pharmacy (ward stock, narcotic logs)
- Inpatient MAR (medication administration)
- IV admixture
- Pharmacogenomic decision support

### 18.1 Centralized pharmacy + narcotic logs (`centralized-pharmacy`, V4)

**1. Overview.** V2's prescription module treats the pharmacy as an external system (we sign the Rx, the partner pharmacy dispenses). V4 introduces a **centralized in-house pharmacy** for hospital tenants: ward stock, controlled-substance (DEA Schedule II–V) vault with witness-required dispenses, IV admixture tracking, and integration with the V4 nursing station's MAR (§20.13) so a dispense is auditable from prescription → pharmacy → nursing administration → wastage.

**2. Domain Model.**
```php
Schema::create('pharmacies', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 128);                                 // 'Central Pharmacy', 'OR Satellite'
    $t->string('license_number', 64);                        // state pharmacy license
    $t->string('dea_registration', 32)->nullable();          // required for controlled substances
    $t->boolean('is_central')->default(false);
    $t->json('operating_hours');
    $t->timestamps();
});

Schema::create('drug_stock', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('pharmacy_id')->index();
    $t->ulid('drug_id')->index();
    $t->string('lot_number', 64);
    $t->string('serial_number', 64)->nullable();             // unit-level for high-value
    $t->date('expiry_date')->index();
    $t->integer('quantity_on_hand');
    $t->integer('reorder_level');
    $t->decimal('unit_cost', 10, 2);
    $t->string('storage_location', 32);                       // 'A1-B3' bin, 'Fridge-2', 'Vault-Row3'
    $t->string('controlled_schedule', 4)->nullable();        // 'II' | 'III' | 'IV' | 'V' | null
    $t->timestamps();
    $t->index(['pharmacy_id', 'drug_id', 'expiry_date']);
});

Schema::create('drug_dispenses', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('prescription_id')->index();
    $t->ulid('dispensed_by_pharmacist_id')->index();
    $t->ulid('witness_user_id')->nullable()->index();        // required for Schedule II–V
    $t->timestamp('dispensed_at')->index();
    $t->json('lines');                                       // [{drug_stock_id, quantity, lot, expiry}]
    $t->string('dispensing_mode', 16);                       // 'in_house' | 'ward_stock_pull' | 'satellite'
    $t->string('patient_id_or_ward', 64);                    // patient_id or 'WARD-3A'
    $t->text('pharmacist_notes')->nullable();
    $t->timestamps();
});

Schema::create('narcotic_logs', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('drug_stock_id')->index();
    $t->string('action', 16);                                // 'receive' | 'dispense' | 'waste' | 'return' | 'transfer' | 'loss'
    $t->integer('quantity');
    $t->string('performed_by_user_id', 26);
    $t->string('witness_user_id', 26)->nullable();
    $t->string('patient_or_ward', 64)->nullable();
    $t->string('dea_form_number', 16)->nullable();           // e.g., '222' for Schedule II transfer
    $t->text('reason')->nullable();                          // required for 'waste', 'loss'
    $t->json('signature');                                   // {witness_name, signed_at, ip}
    $t->timestamp('occurred_at')->index();
    $t->timestamps();
});
```

**3. API Surface.**
```
GET    /api/v4/pharmacy/dashboard                           # KPI: stock value, expiring soon, controlled count
GET    /api/v4/pharmacy/stock?drug=...&expiring_within=90d  # inventory query
POST   /api/v4/pharmacy/stock/receive                       # receive from supplier; creates narcotic_log 'receive'
POST   /api/v4/pharmacy/dispense                           # dispense from Rx; witness required for controlled
POST   /api/v4/pharmacy/dispense/ward-stock                 # ward-stock pull; auto-witnessed by RN
POST   /api/v4/pharmacy/waste                              # record wastage (e.g., partial vial); witness required
POST   /api/v4/pharmacy/return                             # return to stock
POST   /api/v4/pharmacy/transfer                           # Schedule II DEA Form 222 transfer
GET    /api/v4/pharmacy/narcotic-log?date=...&drug=...      # DEA audit log
GET    /api/v4/pharmacy/admixture                          # IV admixture queue
POST   /api/v4/pharmacy/admixture                          # compound + label + record
GET    /api/v4/pharmacy/reorder-suggestions                # below-reorder drugs
```

**4. Frontend.** A single dashboard at `/admin/pharmacy/`. Five tabs: **Stock** (sortable, filterable, with expiring-soon filter), **Dispense** (Rx queue, scan-to-dispense), **Narcotic** (vault, witness-required, perpetual inventory), **Admixture** (IV compounding queue, label printer integration), **Reports** (DEA Form 222 export, expiry wastage, controlled-substance reconciliation). The vault screen requires a second-user tap on the badge reader for any action — single-pharmacist access is denied for Schedule II.

**5. Realtime.**
- `pharmacy.stock.low.{tenant}.{drug_id}` — fires when `quantity_on_hand` crosses `reorder_level`.
- `pharmacy.dispense.requested.{tenant}.{pharmacy_id}` — new Rx waiting for dispense.
- `pharmacy.controlled.access.{tenant}.{pharmacy_id}` — every Schedule II–V vault open/close.
- `pharmacy.expiry.soon.{tenant}.{drug_id}` — 30-day expiry warning.

**6. Security & RBAC.** Roles: `pharmacist` (dispense, waste, return; no Schedule II alone), `controlled_substance_pharmacist` (Schedule II access, requires dual-auth), `pharmacy_tech` (receive stock, restock bins; no dispense), `dea_pic` (Pharmacist-in-Charge: views all narcotic logs, signs DEA Form 222). All narcotic-log actions are co-signed (witness + performer) and audit-logged with `action='narcotic.{action}'` to a separate append-only table (per DEA §1304). Witness and performer must be different users. Tampering with `narcotic_logs` is a hard-blocked operation (no UPDATE, only INSERT) — corrections require a new row with a `corrects_log_id` FK.

**7. Acceptance Criteria.**
- [ ] Schedule II dispense is hard-blocked without a witness; the witness field is required server-side.
- [ ] All narcotic_log actions are append-only; UPDATE/DELETE attempts return 403 and write to the `audit` table.
- [ ] Drug stock below `reorder_level` fires the low-stock realtime event and emails the pharmacy manager.
- [ ] Expired drugs cannot be dispensed; dispense endpoint returns 422 with `expiry_passed`.
- [ ] DEA Form 222 export matches the sum of `narcotic_logs` for the period to the gram.
- [ ] FEFO (first-expiry-first-out) is enforced; the system picks the soonest-expiring lot first.

**8. Real-World Test Scenarios.**
- *DEA inspection:* auditor runs the narcotic-log report for the past 90 days; the perpetual inventory reconciles to the on-hand count within ±1 unit per drug.
- *Partial vial waste:* nurse administers 2 mL of a 10 mL morphine vial; pharmacist records 8 mL waste with witness, in compliance with §1304.
- *Drug recall:* manufacturer issues recall for lot L-12345; system flags all `drug_stock` rows matching; all Rxs against that lot are paused; patients notified per `11-Notifications`.
- *IV admixture:* pharmacist compounds vancomycin 1g in 250 mL NS; label prints with patient, drug, dose, diluent, expiry (24h), initials; nurse scans label at administration.
- *FEFO conflict:* two lots of the same drug; the older-expiring lot is dispensed first; the system refuses to dispense the newer lot until the older is exhausted.

**9. Dev Workflow.** `php artisan pharmacy:simulate --tenant=... --dispense-rate=10/min` synthetic load. `php artisan dea:reconcile --from=... --to=...` produces the DEA Form 222 report. Code: `app-modules/centralized-pharmacy/`; tests: `app-modules/centralized-pharmacy/tests/Feature/{Stock,Dispense,Narcotic,Admixture}Test.php` and `tests/Compliance/DeaReconciliationTest.php`.

#### V5 (Month 22+)
- AI de-prescribing (deprescribe inappropriate meds in elderly)
- AI adverse event prediction
- Population-level prescribing analytics

### 22.2 Dependencies

| Phase | Depends on |
|---|---|
| V1 | Patients, Doctors, Appointments, Audit, Realtime |
| V1.1 | V1, RxNav, OpenAI, ABDM, lab partner APIs |
| V2 | V1.1, SCRIPT spec, LactMed, Beers list |
| V3 | V2, chemo regimen DB, CGM providers, PMP integration |
| V4 | V3, EHR (V2), FHIR |

### 22.3 Resource estimate

| Phase | FTE | Months | Effort (PM) |
|---|---|---|---|
| V1 | 2 | 3 | 6 |
| V1.1 | 3 | 2 | 6 |
| V2 | 3 | 3 | 9 |
| V3 | 4 | 5 | 20 |
| V4 | 3 | 7 | 21 |
| **Total** | | | **~62 PM** |

### 22.4 Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Drug interaction false negative | Low | Critical | Multiple data sources, daily updates, red-team |
| AI hallucination in scribe | Med | High | Human-in-the-loop, red-team |
| ABDM certification delay | High | Med | Start M2 early |
| Wrong dose calculation | Med | Critical | Multiple tests, weight confirmation prompt |
| Allergy cross-reactivity missed | Med | High | Comprehensive allergen DB |
| Refill abuse | Med | Med | PMP check, refill interval check |
| E-Rx QR tampering | Low | High | HMAC signature |

### 22.5 Metrics

| Metric | V1 | V2 | V3 | V4 |
|---|---|---|---|---|
| Rx/day | 1k | 10k | 50k | 200k |
| Drug interaction accuracy | 90% | 95% | 98% | 99% |
| AI scribe edit rate | — | 30% | 20% | 15% |
| E-prescription upload success | — | 95% | 99% | 99.9% |
| Refill turnaround time | — | 4h | 1h | 15min |
| Pharmacy dispense callback | — | 80% | 95% | 99% |

### 22.6 Open questions

1. Should we support veterinary as a fork or as a feature flag?
2. Should dental be a separate UI or extend the medical one?
3. Should we support homeopathy given the regulatory scrutiny?
4. Should we integrate with e-Rx clearinghouses (SureScripts) ourselves or via partner?
5. How do we handle pharmacy deserts (areas with no pharmacy)?

### 22.7 Definition of Done

- [ ] 90%+ test coverage
- [ ] All P1/P2 bugs closed
- [ ] ABDM / SCRIPT / EPS round-trips pass
- [ ] Red-team: 100+ drug pairs tested, no false negatives
- [ ] Pharmacist review of alert logic
- [ ] Performance budget met
- [ ] Security scan clean
- [ ] Documentation updated
- [ ] Runbook updated

---

## 23. V3+ Acceptance Criteria

- [ ] Chemo regimen correctly schedules 6 cycles of R-CHOP.
- [ ] Insulin calculator returns correct bolus for carbs + glucose input.
- [ ] Pediatric amoxicillin dose = 25-50 mg/kg/day.
- [ ] Warfarin dose adjustment based on INR trend.
- [ ] Beers Criteria: alert for benzodiazepine in 80yo.
- [ ] Pregnancy category X drug blocked for pregnant patient.
- [ ] ABDM M2/M3 certified.
- [ ] SureScripts round-trip succeeds.
- [ ] Pharmacist review of alert logic signed off.
- [ ] All V2/V1 acceptance criteria still pass.

---

> **Total additions to Prescriptions doc:** ~80 KB.
> **Total test count defined:** ~580 tests.
> **Module line count:** ~1,500 lines.
