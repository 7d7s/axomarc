# 01 — Master Module Index

> The complete catalog of every module in the Medicare system. Each entry links to its deep-dive document and lists the owning team, status, public API surface, and dependencies.

---

## Module Status Legend

| Symbol | Meaning |
|---|---|
| ✅ | Shipped in V1 |
| 🟡 | Schema/UI stub only in V1 (no working feature) |
| ❌ | Deferred to V2+ |
| 🔌 | Integration with external service |

---

## 1. Identity & Access (4 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 1 | **Auth** (OTP, email/password, sessions) | ✅ | [02-MODULE-AUTH.md](02-MODULE-AUTH.md) | API | Firebase Phone OTP for mobile; email/password for web/admin |
| 2 | **RBAC** (Roles & Permissions) | ✅ | [02-MODULE-AUTH.md](02-MODULE-AUTH.md) §RBAC | API | Spatie Permissions; granular per-resource |
| 3 | **Users** (admin, staff) | ✅ | [02-MODULE-AUTH.md](02-MODULE-AUTH.md) §Users | API | CRUD with soft-delete + audit |
| 4 | **Audit Logs** | ✅ | [13-MODULE-SETTINGS-REPORTS.md](13-MODULE-SETTINGS-REPORTS.md) §Audit | API | Append-only PHI change tracking |

## 2. Tenancy & Platform (3 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 5 | **Tenancy** (multi-tenant) | ✅ | [03-MODULE-TENANCY.md](03-MODULE-TENANCY.md) | API | Subdomain/path resolver; row-level isolation |
| 6 | **Installer Wizard** | ✅ | [22-INSTALLER.md](22-INSTALLER.md) | API | Domain activation like original Medicare |
| 7 | **Settings** (all tabs) | ✅ | [13-MODULE-SETTINGS-REPORTS.md](13-MODULE-SETTINGS-REPORTS.md) §Settings | API | Basic, Web, Mobile, Video, Firebase, Payments, Features |

## 3. Geography & Structure (5 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 8 | **Cities** | ✅ | [04-MODULE-CLINIC-STRUCTURE.md](04-MODULE-CLINIC-STRUCTURE.md) §Cities | API | Multi-city support |
| 9 | **Clinics** | ✅ | [04-MODULE-CLINIC-STRUCTURE.md](04-MODULE-CLINIC-STRUCTURE.md) §Clinics | API | = tenant when multi-tenant mode |
| 10 | **Departments** | ✅ | [04-MODULE-CLINIC-STRUCTURE.md](04-MODULE-CLINIC-STRUCTURE.md) §Departments | API | With image, description, active flag |
| 11 | **Specializations** | ✅ | [04-MODULE-CLINIC-STRUCTURE.md](04-MODULE-CLINIC-STRUCTURE.md) §Specializations | API | E.g., MBBS, MD |
| 12 | **Services** | ✅ | [04-MODULE-CLINIC-STRUCTURE.md](04-MODULE-CLINIC-STRUCTURE.md) §Services | API | Billable services per clinic |

## 4. Doctors (5 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 13 | **Doctors** | ✅ | [05-MODULE-DOCTORS.md](05-MODULE-DOCTORS.md) | API | Profile, fee, experience, social, address, rating |
| 14 | **Doctor Schedules** | ✅ | [05-MODULE-DOCTORS.md](05-MODULE-DOCTORS.md) §Schedules | API | Per-day working hours |
| 15 | **Doctor Time Slots** | ✅ | [05-MODULE-DOCTORS.md](05-MODULE-DOCTORS.md) §Slots | API | Slot generation engine |
| 16 | **Doctor Reviews** | ✅ | [12-MODULE-ENGAGEMENT.md](12-MODULE-ENGAGEMENT.md) §Reviews | API | Star rating + text |
| 17 | **Doctor Experiences/Education** | ✅ | [05-MODULE-DOCTORS.md](05-MODULE-DOCTORS.md) §Profile | API | Repeater JSON on doctor |

## 5. Patients & Family (5 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 18 | **Patients** | ✅ | [06-MODULE-PATIENTS.md](06-MODULE-PATIENTS.md) | API | Auto-MRN, demographics |
| 19 | **Family Members** | ✅ | [06-MODULE-PATIENTS.md](06-MODULE-PATIENTS.md) §Family | API | Per-user profiles |
| 20 | **Vitals** | ✅ | [06-MODULE-PATIENTS.md](06-MODULE-PATIENTS.md) §Vitals | API | BP/Sugar/SpO2/Temp/Weight + graphs |
| 21 | **Patient Files** | ✅ | [06-MODULE-PATIENTS.md](06-MODULE-PATIENTS.md) §Files | API | Upload reports |
| 22 | **Medicines** (master) | ✅ | [08-MODULE-PRESCRIPTIONS.md](08-MODULE-PRESCRIPTIONS.md) §Master | API | Reusable Rx autocomplete |

## 6. Appointments & Queue (6 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 23 | **Appointments** (core) | ✅ | [07-MODULE-APPOINTMENTS.md](07-MODULE-APPOINTMENTS.md) | API | 7-state machine |
| 24 | **Appointment Calendar** | ✅ | [07-MODULE-APPOINTMENTS.md](07-MODULE-APPOINTMENTS.md) §Calendar | API | Day/week/month views |
| 25 | **Appointment Check-in** | ✅ | [07-MODULE-APPOINTMENTS.md](07-MODULE-APPOINTMENTS.md) §CheckIn | API | Manual + QR scan |
| 26 | **Queue Tokens** | ✅ | [07-MODULE-APPOINTMENTS.md](07-MODULE-APPOINTMENTS.md) §Queue | API | Auto-generated on check-in |
| 27 | **Live Patient Calling** | ✅ | [07-MODULE-APPOINTMENTS.md](07-MODULE-APPOINTMENTS.md) §Calling | API | Staff triggers → Reverb event |
| 28 | **Cancellation Requests** | ✅ | [07-MODULE-APPOINTMENTS.md](07-MODULE-APPOINTMENTS.md) §Cancel | API | User-initiated, admin-approved |

## 7. Clinical Workflow (2 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 29 | **Prescriptions** | ✅ | [08-MODULE-PRESCRIPTIONS.md](08-MODULE-PRESCRIPTIONS.md) | API | CRUD, print, written-style fallback |
| 30 | **Prescription Medicines** | ✅ | [08-MODULE-PRESCRIPTIONS.md](08-MODULE-PRESCRIPTIONS.md) §Lines | API | Repeater table |

## 8. Billing & Payments (6 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 31 | **Invoices** | ✅ | [09-MODULE-BILLING.md](09-MODULE-BILLING.md) §Invoices | API | Auto + manual |
| 32 | **Transactions** | ✅ | [09-MODULE-BILLING.md](09-MODULE-BILLING.md) §Transactions | API | All/Payment/Invoice tabs |
| 33 | **Payments** | ✅ | [10-MODULE-PAYMENTS.md](10-MODULE-PAYMENTS.md) | API | Gateway-agnostic |
| 34 | **Razorpay** 🔌 | ✅ | [10-MODULE-PAYMENTS.md](10-MODULE-PAYMENTS.md) §Razorpay | API | India primary |
| 35 | **Stripe** 🔌 | ✅ | [10-MODULE-PAYMENTS.md](10-MODULE-PAYMENTS.md) §Stripe | API | Global |
| 36 | **Paystack** 🔌 | ✅ | [10-MODULE-PAYMENTS.md](10-MODULE-PAYMENTS.md) §Paystack | API | Africa |

## 9. Discounts & Tax (2 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 37 | **Coupons** | ✅ | [09-MODULE-BILLING.md](09-MODULE-BILLING.md) §Coupons | API | % discount, scheduled |
| 38 | **Taxes** | ✅ | [09-MODULE-BILLING.md](09-MODULE-BILLING.md) §Taxes | API | Included/excluded |

## 10. Engagement & Content (5 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 39 | **Testimonials** | ✅ | [12-MODULE-ENGAGEMENT.md](12-MODULE-ENGAGEMENT.md) §Testimonials | API | Marketing |
| 40 | **Banners** | ✅ | [12-MODULE-ENGAGEMENT.md](12-MODULE-ENGAGEMENT.md) §Banners | API | Homepage carousel |
| 41 | **Login Slides** | ✅ | [12-MODULE-ENGAGEMENT.md](12-MODULE-ENGAGEMENT.md) §Slides | API | Onboarding |
| 42 | **Reviews** (doctor) | ✅ | [12-MODULE-ENGAGEMENT.md](12-MODULE-ENGAGEMENT.md) §Reviews | API | Patient ratings |
| 43 | **Reviews** (clinic) | 🟡 | [12-MODULE-ENGAGEMENT.md](12-MODULE-ENGAGEMENT.md) §Reviews | API | Schema only V1 |

## 11. Communication (4 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 44 | **Notifications** (push + email) | ✅ | [11-MODULE-NOTIFICATIONS.md](11-MODULE-NOTIFICATIONS.md) | API | FCM + SMTP |
| 45 | **Notification Templates** | ✅ | [11-MODULE-NOTIFICATIONS.md](11-MODULE-NOTIFICATIONS.md) §Templates | API | DB-driven i18n |
| 46 | **FCM Tokens** | ✅ | [11-MODULE-NOTIFICATIONS.md](11-MODULE-NOTIFICATIONS.md) §FCM | API | Per-device registration |
| 47 | **Zoom Video Calls** 🔌 | ✅ | [19-INTEGRATIONS.md](19-INTEGRATIONS.md) §Zoom | API | S2S OAuth |

## 12. Internationalization (2 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 48 | **Languages** | ✅ | [12-MODULE-ENGAGEMENT.md](12-MODULE-ENGAGEMENT.md) §Languages | API | Dynamic |
| 49 | **Currencies** | ✅ | [12-MODULE-ENGAGEMENT.md](12-MODULE-ENGAGEMENT.md) §Currencies | API | Symbol/position/precision |

## 13. Reports & Analytics (2 modules)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 50 | **Dashboard** | ✅ | [13-MODULE-SETTINGS-REPORTS.md](13-MODULE-SETTINGS-REPORTS.md) §Dashboard | API + Web | Counts + graphs |
| 51 | **Reports** | ✅ | [13-MODULE-SETTINGS-REPORTS.md](13-MODULE-SETTINGS-REPORTS.md) §Reports | API | Date-range, exportable |

## 14. Cron & Scheduled Jobs (1 module)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 52 | **Scheduled Jobs** | ✅ | [11-MODULE-NOTIFICATIONS.md](11-MODULE-NOTIFICATIONS.md) §Cron | API | Reminders, daily summaries, plan expiry |

## 15. Realtime Infrastructure (1 module)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 53 | **Reverb Broadcasting** 🔌 | ✅ | [17-REALTIME.md](17-REALTIME.md) | API + All clients | Laravel Reverb |

## 16. V2 Stubs (4 modules — schema only in V1)

| # | Module | Status | Doc | Owner | Notes |
|---|---|---|---|---|---|
| 54 | **Pathology / Labs** | 🟡 | [14-MODULE-V2-STUBS.md](14-MODULE-V2-STUBS.md) §Pathology | API | V2 |
| 55 | **AI Chat Doctor Recommendation** | 🟡 | [14-MODULE-V2-STUBS.md](14-MODULE-V2-STUBS.md) §AI | API | V2 |
| 56 | **Blogs** | 🟡 | [14-MODULE-V2-STUBS.md](14-MODULE-V2-STUBS.md) §Blogs | API | V2 |
| 57 | **Subscription Plans** | 🟡 | [14-MODULE-V2-STUBS.md](14-MODULE-V2-STUBS.md) §Subscriptions | API | V2 |

---

## Total Counts

| Bucket | Count |
|---|---|
| ✅ V1 Active modules | **53** |
| 🟡 V1 schema stubs (UI behind "Coming soon" toggle) | 4 |
| ❌ V2+ deferred | 0 from above, more in `24-ROADMAP.md` |
| **Total in catalog** | **57** |

---

## 17. V2+ Module Catalog (preview, full plan in `14-MODULE-V2-STUBS.md`)

> V1 ships 57 modules. V2 adds 54 more, taking the platform to 111 modules by V4. Each V2 module has a **Maturity Model Level (MML)** 1–5: 1=Designed, 2=Schema, 3=Internal API, 4=Patient/Doctor API, 5=GA.

### 17.1 V1.1 lane (Months 0–2, ships behind flag)

| # | Module | V1 Doc | MML@GA | Tier | Owner |
|---|---|---|---|---|---|
| 58 | WhatsApp Business | `11-Notifications` | 4 | Pro | Notifications guild |
| 59 | Recurring Appointments | `07-Appointments` | 4 | Free | Appointments guild |
| 60 | Group Appointments | `07-Appointments` | 4 | Pro | Appointments guild |
| 61 | Family Booking | `06-Patients`, `07-Appointments` | 4 | Free | Patients guild |
| 62 | Multi-Currency Payout | `05-Doctors`, `09-Billing`, `10-Payments` | 3 | Pro | Payments guild |
| 63 | Geo-Search | `05-Doctors`, `12-Engagement` | 4 | Pro | Doctors guild |
| 64 | Marketing Vouchers | `09-Billing` | 3 | Pro | Billing guild |
| 65 | Dynamic Refund Window | `07-Appointments`, `09-Billing` | 3 | Pro | Appointments guild |
| 66 | Smart Waitlist | `07-Appointments` | 3 | Pro | Appointments guild |
| 67 | i18n RTL | `12-Engagement` | 4 | Free | Engagement guild |
| 68 | Doctor Rotating Schedule | `05-Doctors`, `07-Appointments` | 3 | Pro | Doctors guild |
| 69 | PWA Offline Mode | `medicare-user-app` | 4 | Free | Mobile guild |

### 17.2 V1.2 lane (Months 2–4)

| # | Module | V1 Doc | MML@GA | Tier | Owner |
|---|---|---|---|---|---|
| 70 | ABDM/ABHA Bridge | `02-Auth`, `06-Patients` | 3 | Pro | Integrations guild |
| 71 | FHIR R4 Read | `16-API-Reference` | 3 | Pro | Integrations guild |
| 72 | Razorpay Subscriptions | `10-Payments` | 3 | Pro | Payments guild |
| 73 | Paystack Subscriptions | `10-Payments` | 3 | Pro | Payments guild |
| 74 | Stripe Terminal | `10-Payments` | 4 | Pro | Payments guild |
| 75 | Twilio SMS | `02-Auth`, `11-Notifications` | 4 | Free | Notifications guild |
| 76 | Local Payment Methods (UPI Autopay, GCash) | `10-Payments` | 3 | Pro | Payments guild |
| 77 | Multi-Clinic Dashboard | `04-Clinic-Structure`, `13-Settings-Reports` | 3 | Pro | Admin guild |

### 17.3 V2 lane (Months 4–7, the marquee)

| # | Module | V1 Doc | MML@GA | Tier | Owner |
|---|---|---|---|---|---|
| 78 | Pathology / Lab | `06`, `08`, `15` | 4 | Pro | Lab guild |
| 79 | Telemedicine Video | `07`, `17` | 4 | Pro | Telemedicine guild |
| 80 | AI Symptom Checker | `18`, `19` | 3 | Enterprise | AI guild |
| 81 | AI Clinical Scribe | `06`, `08` | 3 | Enterprise | AI guild |
| 82 | In-App Chat | `11`, `17` | 4 | Pro | Realtime guild |
| 83 | Plugins v1 | All | 3 | Free framework | Platform guild |
| 84 | Custom Domain | `03`, `21` | 3 | Enterprise | Platform guild |
| 85 | Multi-Region DR | `21`, `20` | 3 | Enterprise | Platform guild |
| 86 | Drug Interaction DB | `08` | 4 | Pro | Clinical guild |
| 87 | E-Prescription (ABDM/SCRIPT/GStandard) | `08`, `19` | 3 | Pro | Clinical guild |
| 88 | Insurance Pre-Auth | `09`, `18` | 3 | Enterprise | RCM guild |

### 17.4 V2.1 lane (Months 7–10)

| # | Module | V1 Doc | MML@GA | Tier | Owner |
|---|---|---|---|---|---|
| 89 | Revenue Cycle Mgmt (denial, E&O, dunning) | `09`, `13` | 3 | Enterprise | RCM guild |
| 90 | SaaS Plan Marketplace (Plugin Monetization surface) | `09`, `10` | 4 | Free framework | Platform guild |
| 91 | On-Prem LLM Gateway (vLLM) | `19` | 3 | Enterprise | AI guild |
| 92 | SOC 2 Type II Compliance | `20` | 3 | — | Security guild |
| 93 | BYOK Encryption | `20` | 3 | Enterprise | Security guild |
| 94 | Field-Level Encryption | `20`, `06` | 3 | Enterprise | Security guild |

### 17.5 V3 lane (Months 10–15)

| # | Module | V1 Doc | MML@GA | Tier | Owner |
|---|---|---|---|---|---|
| 95 | Plugin Marketplace (public) | All | 3 | Free | Platform guild |
| 96 | HL7 v2 ADT | `19` | 3 | Enterprise | Integrations guild |
| 97 | FHIR R4 Write | `16` | 3 | Enterprise | Integrations guild |
| 98 | DICOM Viewer (OHIF) | `08`, `19` | 3 | Pro | Lab guild |
| 99 | Custom Report Builder | `13` | 3 | Pro | Reports guild |
| 100 | White-Label Mobile (EAS) | `medicare-user-app` | 3 | Enterprise | Mobile guild |
| 101 | SSO / OIDC / SAML | `02` | 3 | Enterprise | Auth guild |
| 102 | Bed / Ward Management | `04`, `06` | 3 | Enterprise | Hospital guild |
| 103 | Operation Theatre Scheduling | `04`, `07` | 3 | Enterprise | Hospital guild |
| 104 | Equipment Booking | `04`, `07` | 3 | Enterprise | Hospital guild |

### 17.6 V4 lane (Months 15–22, Hospital ITOps)

| # | Module | V1 Doc | MML@GA | Tier | Owner |
|---|---|---|---|---|---|
| 105 | LIS ASTM E1394 Instrument | `19` | 3 | Enterprise | Lab guild |
| 106 | HIS Bridge (Epic, Cerner, Meditech) | `19` | 3 | Enterprise | Integrations guild |
| 107 | ER Triage Board | `06`, `07` | 3 | Enterprise | Hospital guild |
| 108 | Nursing Station (vitals, MAR) | `06` | 3 | Enterprise | Hospital guild |
| 109 | Centralized Pharmacy + Narcotic Logs | `08` | 3 | Enterprise | Pharmacy guild |
| 110 | Equipment Maintenance | `04` | 3 | Enterprise | Hospital guild |

### 17.7 V5+ lane (designed, not scheduled)

| # | Module | V1 Doc | MML | Notes |
|---|---|---|---|---|
| 111 | Population Health / Cohort Tooling | `06`, `08` | 1 | De-identified analytics |

**Total V2+ catalog: 54 modules. Grand total: 111 modules.**

For ownership transfer rules, V2 sponsor model, and the full release table, see `14-MODULE-V2-STUBS.md` §2 and §3.

---

## Module Dependency Graph (high-level)

```
                     ┌──────────┐
                     │ Tenancy  │  ← every other module depends on this
                     └────┬─────┘
                          │
       ┌──────────────────┼──────────────────┐
       │                  │                  │
       ▼                  ▼                  ▼
   ┌────────┐       ┌──────────┐       ┌──────────┐
   │  Auth  │──────►│   RBAC   │──────►│  Users   │
   └────┬───┘       └────┬─────┘       └────┬─────┘
        │                │                  │
        └────────────────┴──────────────────┘
                          │
        ┌─────────────────┼─────────────────┐
        │                 │                 │
        ▼                 ▼                 ▼
   ┌────────┐         ┌────────┐       ┌────────┐
   │ Cities │────────►│Clinics │──────►│Depts/  │
   └────────┘         └────┬───┘       │Specs/  │
                          │           │Services│
                          │           └────────┘
                          ▼
   ┌────────┐         ┌─────────┐        ┌────────┐
   │Doctors │────────►│ Time    │───────►│Apts    │
   │        │         │ Slots   │        │        │
   └────┬───┘         └─────────┘        └───┬────┘
        │                                    │
        │   ┌────────┐  ┌────────┐  ┌──────┐ │
        └──►│  Rx    │  │Billing │◄─┤Paymts│ │
            └────────┘  └───┬────┘  └──────┘ │
                            │                │
                            ▼                │
                       ┌─────────┐           │
                       │Invoices │◄──────────┘
                       └─────────┘

   ┌────────────┐  ┌────────────┐  ┌─────────────┐
   │Notifications│ │  Realtime  │  │  Settings   │
   └────────────┘  └────────────┘  └─────────────┘
         ▲              ▲                ▲
         │              │                │
         └──────────────┴────────────────┘
              cross-cutting, no upstream dep
```

---

## Cross-Module Concerns (read alongside any module)

| Concern | Doc | Why |
|---|---|---|
| **Tenancy** | [03-MODULE-TENANCY.md](03-MODULE-TENANCY.md) | Every module inherits tenant scoping |
| **Realtime events** | [17-REALTIME.md](17-REALTIME.md) | Modules that mutate state broadcast events |
| **Audit logging** | [13-MODULE-SETTINGS-REPORTS.md](13-MODULE-SETTINGS-REPORTS.md) §Audit | All PHI mutations logged |
| **i18n** | [12-MODULE-ENGAGEMENT.md](12-MODULE-ENGAGEMENT.md) §Languages | User-facing strings are translated |
| **Currency formatting** | [12-MODULE-ENGAGEMENT.md](12-MODULE-ENGAGEMENT.md) §Currencies | Money respects clinic currency |
| **Notifications** | [11-MODULE-NOTIFICATIONS.md](11-MODULE-NOTIFICATIONS.md) | State changes often trigger push/email |

---

## Per-Team Ownership

| Team | Modules |
|---|---|
| **API team (Dev 1)** | All modules 1-57 (backend) |
| **Admin + Patient Web (Dev 2)** | All 57 (consumed via API) |
| **Mobile (Dev 3)** | Subset: 1, 2, 8-12, 13-17, 18-22, 23-28, 29-30, 31-38, 39-43, 44-47, 50-51 |
| **DevOps/QA (Dev 4)** | 6, 7, 52, 53 (platform) + CI/CD + observability |

---

## How to Add a New Module

1. **Propose** — write a 1-page RFC into `docs/proposals/`. Include the data model, the API surface, the UI changes, the realtime events, the audit hooks, and the migration plan.
2. **Schema** — add migrations under `database/migrations/` with a clear date prefix.
3. **Model + Service** — model in `app/Modules/{Name}/{Name}.php`, service in `app/Modules/{Name}/Services/{Name}Service.php`, tests in `tests/Feature/Modules/{Name}/`.
4. **API** — controllers in `app/Modules/{Name}/Http/Controllers/`, routes in `app/Modules/{Name}/routes.php`, FormRequests in `Http/Requests/`, Resources in `Http/Resources/`.
5. **Realtime** — events in `app/Modules/{Name}/Events/`, channels in `routes/channels.php`.
6. **Audit** — register the model's `auditable` events in `app/Modules/AuditLogs/`.
7. **Web** — add to `medicare-web/apps/admin` if admin-facing, else `medicare-web/apps/patient-web`.
8. **Mobile** — add to `medicare-user-app` (patient) or `medicare-doctor-app` (doctor).
9. **Docs** — append a section to the relevant module doc.
10. **Feature flag** — if the module is optional, gate it behind `config('medicare.features.{name}')`.

---

## Versioning of Modules

Each module has a `VERSION` constant in its service class:

```php
// app/Modules/Appointments/Services/AppointmentService.php
class AppointmentService {
    public const VERSION = '1.0.0';
}
```

Breaking changes bump MAJOR. Mobile clients send `Accept-Module-Version` header; the API returns `409 Conflict` with a "please update" payload if the client is too old.
