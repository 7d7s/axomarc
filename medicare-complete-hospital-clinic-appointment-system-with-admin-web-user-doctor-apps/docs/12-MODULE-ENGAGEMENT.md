# 12 — Engagement Module (Testimonials, Banners, Login Slides, Reviews, Languages, Currencies)

> The marketing + i18n layer. Everything that makes the public site feel alive, and everything that lets the same product speak any language and accept any currency.

---

## 1. Scope

| In Scope | Out of Scope |
|---|---|
| Testimonials (curated quotes from patients) | Marketing automation (V2) |
| Banners (homepage carousel) | A/B testing (V2) |
| Login slides (onboarding carousel) | Multi-region data residency (V2) |
| Doctor reviews (full CRUD, ratings) | Clinic reviews (V2 schema only) |
| Languages (dynamic, admin-managed) | Translation memory (V2) |
| Currencies (symbol, position, precision) | Crypto (V2) |
| Public/patient/doctor rating displays | |

---

## 2. Testimonials

### 2.1 Concept

A testimonial is a **curated, pre-approved** quote from a patient about the clinic or a specific doctor. Used on the homepage and clinic detail pages. The `is_approved` gate is the key control — only approved ones show publicly.

### 2.2 Data Model

`testimonials` (per-tenant)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `clinic_id` | bigint FK nullable | For multi-clinic filtering |
| `doctor_id` | bigint FK nullable | If about a specific doctor |
| `patient_name` | varchar(120) | |
| `patient_photo_path` | varchar(500) nullable | |
| `patient_city` | varchar(120) nullable | "Mumbai" |
| `rating` | tinyint nullable | 1-5 |
| `quote` | text | The testimonial text |
| `video_url` | varchar(500) nullable | YouTube/Vimeo embed |
| `is_featured` | boolean default false | |
| `is_approved` | boolean default false | |
| `display_order` | int default 0 | |
| `approved_by_user_id` | bigint FK nullable | |
| `approved_at` | timestamp nullable | |
| `created_at`, `updated_at` | timestamps |
| `deleted_at` | timestamp nullable | Soft |

### 2.3 API

```
GET    /api/v1/public/testimonials?clinic_id=&doctor_id=&featured=
GET    /api/v1/admin/testimonials?is_approved=
POST   /api/v1/admin/testimonials
GET    /api/v1/admin/testimonials/{id}
PUT    /api/v1/admin/testimonials/{id}
DELETE /api/v1/admin/testimonials/{id}        (soft)
POST   /api/v1/admin/testimonials/{id}/approve
POST   /api/v1/admin/testimonials/{id}/unapprove
POST   /api/v1/admin/testimonials/reorder      { ids: [...] }
```

### 2.4 Sources

1. **Admin manual entry** — most common. Reception writes what the patient said, with consent.
2. **Patient-submitted form** — public `POST /public/testimonials/submit` with name, quote, rating, optional photo. Goes into `is_approved=false` queue.
3. **Auto-extracted from reviews** — admin can "promote" a doctor review to a testimonial with one click.

### 2.5 Display

- Homepage: 3-6 featured in a carousel.
- Clinic detail page: filtered to that clinic.
- Doctor profile: filtered to that doctor.

---

## 3. Banners

### 3.1 Concept

Image banners for the homepage carousel. Clickable; target a URL, doctor, or department. Schedule-driven (start/end date).

### 3.2 Data Model

`banners` (per-tenant)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `clinic_id` | bigint FK nullable | |
| `title` | varchar(180) | For accessibility (alt text) |
| `image_path` | varchar(500) | |
| `image_mobile_path` | varchar(500) nullable | Optional mobile crop |
| `target_type` | enum('url','doctor','department','service','clinic','none') | |
| `target_id` | bigint nullable | |
| `target_url` | varchar(500) nullable | For `target_type=url` |
| `cta_label` | varchar(60) nullable | "Book Now" |
| `starts_at` | timestamp nullable | |
| `ends_at` | timestamp nullable | |
| `is_active` | boolean default true | |
| `display_order` | int default 0 | |
| `click_count` | int default 0 | Cached, atomic increment |
| `impression_count` | int default 0 | |
| `created_at`, `updated_at` | timestamps |
| `deleted_at` | timestamp nullable | |

### 3.3 Image Specs

- Desktop: 1920×640 (3:1)
- Mobile: 1080×720 (3:2)
- Max 500 KB
- WebP preferred, JPEG fallback
- Stored via Spatie Media with conversions

### 3.4 API

```
GET    /api/v1/public/banners?clinic_id=
GET    /api/v1/public/banners/{id}/click       (redirects, increments click)
POST   /api/v1/public/banners/{id}/impression  (called by client, increments)
GET    /api/v1/admin/banners
POST   /api/v1/admin/banners
GET    /api/v1/admin/banners/{id}
PUT    /api/v1/admin/banners/{id}
DELETE /api/v1/admin/banners/{id}
POST   /api/v1/admin/banners/reorder
```

---

## 4. Login Slides (Onboarding Carousel)

### 4.1 Concept

3-5 illustrated slides shown to a new user the first time they open the mobile app or visit the patient website. Skip-able, "Don't show again" option persists in local storage.

### 4.2 Data Model

`login_slides` (per-tenant, optional — falls back to global)

| Column | Type |
|---|---|
| `id` | bigint PK |
| `tenant_id` | bigint FK nullable | NULL = global default |
| `app_type` | enum('user','doctor','admin','patient_web') | Which app |
| `title` | varchar(120) |
| `subtitle` | varchar(255) |
| `image_path` | varchar(500) | |
| `cta_label` | varchar(60) nullable | "Get Started" |
| `cta_target` | enum('next','skip','url') |
| `cta_url` | varchar(500) nullable |
| `display_order` | int default 0 |
| `is_active` | boolean default true |
| `created_at`, `updated_at` | timestamps |

### 4.3 API

```
GET    /api/v1/public/login-slides?app_type=user
GET    /api/v1/admin/login-slides
POST   /api/v1/admin/login-slides
PUT    /api/v1/admin/login-slides/{id}
DELETE /api/v1/admin/login-slides/{id}
POST   /api/v1/admin/login-slides/reorder
```

---

## 5. Doctor Reviews

The full review module is technically part of `05-MODULE-DOCTORS` (data model lives there) but the engagement surface is here.

### 5.1 Concept

A patient who had a `completed` appointment can leave one review per appointment. Review has 1-5 star rating + optional title + optional comment. The patient can also choose to be anonymous. The doctor can post one reply. Admin moderates.

### 5.2 Data Model

`doctor_reviews` — see `05-MODULE-DOCTORS.md` §6.1 for full schema.

Summary:
- One review per appointment (unique constraint)
- 1-5 star rating
- Optional title, comment
- is_anonymous flag
- is_approved flag (admin moderation)
- doctor_reply + replied_at (one reply, editable)
- Reply can be edited/deleted

### 5.3 The Rating Aggregator

A `RatingAggregator` listener on `DoctorReview::saved/deleted`:

```php
class RatingAggregator {
    public function recompute(Doctor $doctor): void {
        DB::transaction(function () use ($doctor) {
            $stats = DoctorReview::where('doctor_id', $doctor->id)
                ->where('is_approved', true)
                ->selectRaw('AVG(rating) as avg, COUNT(*) as count')
                ->first();
            $doctor->update([
                'avg_rating' => round($stats->avg ?? 0, 2),
                'review_count' => $stats->count ?? 0,
            ]);
        });
    }
}
```

### 5.4 Rating Histogram (for the public profile)

```
GET /api/v1/public/doctors/{id}/reviews/histogram
→ {
  "5": 87,
  "4": 32,
  "3": 8,
  "2": 3,
  "1": 2,
  "total": 132,
  "avg": 4.7
}
```

### 5.5 API

```
POST   /api/v1/patient/appointments/{id}/review
       { rating, title?, comment?, is_anonymous? }
PUT    /api/v1/patient/reviews/{id}                (within 24h)
DELETE /api/v1/patient/reviews/{id}                (within 24h)

POST   /api/v1/doctor/reviews/{id}/reply            { doctor_reply }
PUT    /api/v1/doctor/reviews/{id}/reply
DELETE /api/v1/doctor/reviews/{id}/reply

GET    /api/v1/public/doctors/{id}/reviews?rating=&page=
GET    /api/v1/public/doctors/{id}/reviews/histogram

GET    /api/v1/admin/reviews?is_approved=&min_rating=&page=
POST   /api/v1/admin/reviews/{id}/approve
POST   /api/v1/admin/reviews/{id}/reject            { reason }
POST   /api/v1/admin/reviews/{id}/feature
```

### 5.6 Moderation Rules

- Reviews default to `is_approved=true` (no pre-moderation) unless tenant config says otherwise.
- Profanity filter on submit (auto-reject if match).
- Admin can approve/reject from queue.
- "Spam" detection (V2): 3+ reviews from same IP in 1h → flag for manual.

---

## 6. Languages (i18n)

### 6.1 Two-Layer i18n

1. **Static strings** — Laravel `lang/{locale}.json` and `lang/{locale}/*.php`. Code uses `trans()` / `__()`. Ships with English + Hindi.
2. **Dynamic strings** — DB-backed `translations` table for user-editable strings (clinic names, department descriptions, custom notification templates, etc.).

### 6.2 Data Model

`languages` (global, seeded)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `code` | varchar(10) unique | `en`, `hi`, `ar`, `es`, `fr` |
| `name` | varchar(60) | "English" |
| `native_name` | varchar(60) | "हिन्दी" |
| `direction` | enum('ltr','rtl') | Arabic is RTL |
| `flag_icon` | varchar(20) nullable | Emoji or path |
| `is_active` | boolean default true | |
| `is_rtl` | boolean default false | Mirror of `direction` for indexing |
| `is_default` | boolean default false | One row only |
| `sort_order` | int default 0 | |
| `created_at`, `updated_at` | timestamps |

Seeded: `en` (default), `hi`, `ar`, `es`, `fr`, `de`, `pt`, `zh-CN`, `ja`.

`translations` (per-tenant or global)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK nullable | NULL = global |
| `language_id` | bigint FK | |
| `group` | varchar(60) | `clinic`, `department`, `service`, `notification`, `custom` |
| `key` | varchar(180) | |
| `value` | text | |
| `created_at`, `updated_at` | timestamps |

Unique: `(tenant_id, language_id, group, key)`.

### 6.3 Translation Workflow

- Static: developer adds to `lang/{locale}.json`, ships in code.
- Dynamic: admin edits via UI; saves to DB.
- API resolution: check DB first → fall back to `lang/{locale}.json` → fall back to English.
- Cache: resolved translations cached in Redis per `(tenant, language)`, 1h TTL. Invalidated on update.

### 6.4 Per-User Language

- `users.locale` (e.g., `en`, `hi`).
- Patient app: settings → language switcher.
- Mobile app uses `MaterialApp.locale` + `intl` package for date/number formatting.
- API requests can include `?lang=hi` to override.

### 6.5 RTL Support

- Flutter app: `MaterialApp.supportedLocales` includes Arabic; `Directionality` widget auto-flips.
- Next.js: `dir="rtl"` on `<html>` when locale is RTL.
- CSS: use logical properties (`margin-inline-start` not `margin-left`).

### 6.6 API

```
GET    /api/v1/public/languages
GET    /api/v1/admin/languages
POST   /api/v1/admin/languages
PUT    /api/v1/admin/languages/{id}
DELETE /api/v1/admin/languages/{id}

GET    /api/v1/admin/translations?group=&language_id=&q=&page=
POST   /api/v1/admin/translations          (bulk)
PUT    /api/v1/admin/translations/{id}
DELETE /api/v1/admin/translations/{id}
POST   /api/v1/admin/translations/import   (JSON file)
GET    /api/v1/admin/translations/export
```

### 6.7 i18n + RTL — `i18n-rtl` (V1.1, MML 3) — deep-dive

**1. Overview.** V1 ships 2-layer i18n (the static `lang/{locale}.json` + dynamic `translations` table at §6) and basic RTL support for Arabic (§6.5). V1.1 turns this into a **production-grade localization platform** that supports:
- **10 languages** GA: English (`en`), Spanish (`es`), French (`fr`), Portuguese (`pt`), German (`de`), Arabic (`ar`, RTL), Hebrew (`he`, RTL), Hindi (`hi`), Urdu (`ur`, RTL), Bengali (`bn`). 6 more languages (Persian `fa` RTL, Indonesian `id`, Turkish `tr`, Vietnamese `vi`, Chinese `zh-CN`, Japanese `ja`) ship as `beta` with community translations.
- **Full RTL pipeline:** HTML `dir="rtl"`, CSS logical properties, Flutter `Directionality` auto-flip, mirrored icons, mirrored animations, mirrored charts, mirrored PDF reports.
- **Per-locale formatting:** currency symbol / position / grouping, date format (DD/MM/YYYY vs MM/DD/YYYY vs Hijri), time format (12h vs 24h), number system (Western `1,234.56` vs Indian `1,23,456.56` vs Arabic `١٬٢٣٤٫٥٦`), calendar (Gregorian vs Hijri for display), first day of week (Sun vs Mon).
- **Per-locale medical conventions:** Saudi Arabia uses Hijri for date display in patient-facing UI; India uses DD/MM/YYYY; USA uses MM/DD/YYYY; Egypt uses Eastern Arabic numerals in SMS / WhatsApp.
- **Translation management:** tenant admin uploads / downloads translations (JSON / XLIFF), per-locale completion %, missing-key detection, machine-translation fallback (DeepL / Google Translate), translation memory.
- **Right-to-left layout tests:** visual regression tests for RTL (e.g., `tests/Visual/rtl-snapshots/`); automated RTL check in CI for every PR.

The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.1 lane module. The 2-layer i18n foundation at §6 is unchanged; this adds the 10-language coverage, the full RTL pipeline, the per-locale formatting, the medical conventions, and the management UI.

**2. The 10 GA languages (and 6 beta).**
- **GA (MML 3):** English, Spanish, French, Portuguese, German, Arabic, Hebrew, Hindi, Urdu, Bengali.
- **Beta (MML 3, community-translated):** Persian, Indonesian, Turkish, Vietnamese, Chinese (Simplified), Japanese.
- **Planned (post-V1.1):** Tamil, Telugu, Marathi, Gujarati, Punjabi, Swahili, Russian, Italian, Dutch, Korean.

Each language has:
- `code` (BCP 47 — e.g., `en`, `ar`, `zh-CN`).
- `name` (English) + `native_name` (e.g., "हिन्दी" for Hindi).
- `direction` (`ltr` / `rtl`).
- `flag_icon` (emoji).
- `numbering_system` (`western` / `indian` / `arabic`).
- `calendar_preference` (`gregorian` / `hijri` for display only — internal storage is always Gregorian).
- `date_format` (`DD/MM/YYYY`, `MM/DD/YYYY`, `YYYY-MM-DD`, etc.).
- `time_format` (`12h` / `24h`).
- `first_day_of_week` (`sun` / `mon` / `sat`).
- `currency_symbol_position` (`before` / `after`).
- `currency_decimal_separator` (`.` / `,`).
- `currency_thousands_separator` (`,` / `.` / ` ` / `٬`).
- `translation_completion_pct` (0-100).

**3. The full RTL pipeline.** For an RTL locale (Arabic, Hebrew, Urdu, Persian):
- **HTML:** `<html dir="rtl" lang="ar">`.
- **CSS:** all layouts use **logical properties** (`margin-inline-start`, `padding-block-end`, `border-inline-start`, `inset-inline-end`) — never `margin-left` / `padding-right`. The CSS uses `dir` selectors: `:dir(rtl) { ... }`.
- **Flutter:** `MaterialApp.supportedLocales` includes all 10 GA locales; `Directionality` widget auto-flips; custom widgets use `EdgeInsetsDirectional`, `AlignmentDirectional`, `TextDirection`.
- **Mirrored icons:** arrows (← →), back buttons, chevrons, list bullets, progress bars. A `IconService::mirror($icon, $locale)` utility flips the icon for RTL.
- **Mirrored animations:** slide-in-from-left → slide-in-from-right; carousel direction reversed.
- **Mirrored charts:** bar chart left → bar chart right; legend position flipped; x-axis label alignment flipped.
- **Mirrored PDF reports:** the PDF generator (per `09-Billing §18.x` and `13-Settings §13.x`) accepts a `direction` param; the PDF is generated with the correct layout.
- **Mirrored email templates:** the email template renderer flips the layout for RTL.
- **Mirrored SMS / WhatsApp:** the SMS / WhatsApp message body is text-only (no layout), but the **line breaks** and the **emoji direction** are locale-aware (e.g., 🇸🇦 flag is read right-to-left).
- **Visual regression tests:** Playwright captures screenshots for every locale (LTR + RTL) and compares them to the baseline; CI fails on any pixel diff > 0.1%.

**4. Per-locale formatting.** Every locale has a `LocaleFormatter` service:
- **Currency:** `$100.00` (US), `100,00 €` (DE), `₹1,234.56` (IN), `100.00 د.إ` (AE), `Rp 1.234,56` (ID). The symbol, position, grouping, and decimal separator are locale-aware.
- **Date:** `06/05/2026` (US), `05/06/2026` (IN), `2026-06-05` (ISO), `٥‏/٦‏/٢٠٢٦` (AR with Arabic numerals), `05-Jun-2026` (UK long form). The system uses `IntlDateFormatter` (PHP intl extension) + `intl` (Dart) + `Intl.DateFormat` (JS).
- **Time:** `6:00 PM` (US 12h), `18:00` (DE 24h), `٦:٠٠ م` (AR 12h with AM/PM).
- **Number:** `1,234.56` (US), `1.234,56` (DE), `1,23,456.56` (IN Indian), `١٬٢٣٤٫٥٦` (AR Arabic). The system uses `NumberFormatter` + `intl.NumberFormat`.
- **Calendar:** for display only, Saudi Arabia can show dates in the Hijri calendar (e.g., "10 Dhu al-Hijjah 1447"); the internal storage is always Gregorian; the conversion is via the `islamic` calendar in ICU.
- **First day of week:** Sunday (US, IL), Monday (EU, IN), Saturday (some AR cultures for the work week).
- **Phone number formatting:** country-specific; e.g., `(555) 123-4567` (US), `555-123-4567` (IN), `٠٥٥ ١٢٣ ٤٥٦٧` (AE with Arabic numerals).
- **Address formatting:** country-specific; e.g., US: "Street, City, State ZIP"; IN: "House, Street, City, State PIN"; AE: "Building, Street, City, Country".

**5. Per-locale medical conventions.** Medical UI is locale-aware:
- **Date of birth:** for KSA, the system accepts Hijri DOB and converts to Gregorian for storage; for India, the system accepts DD/MM/YYYY; for USA, MM/DMYYYY.
- **Lab values:** unit display is locale-aware (e.g., `mg/dL` US vs `mmol/L` EU vs `mg/dl` IN); conversion is via a `unit_conversion` table.
- **Patient name order:** for AR, HE, UR, FA, the name order is "Family Name, Given Name" (or "Given Name Family Name" with explicit family name field); for IN, the name may have honorific (Shri / Smt) and father / husband name.
- **Gender field:** for most locales, `male` / `female` / `other`; for some, additional options (e.g., Hijra in IN); configurable per tenant.
- **ID number format:** Aadhaar (IN, 12 digits), SSN (US, XXX-XX-XXXX), National ID / Iqama (KSA, 10 digits), Emirates ID (UAE, 15 digits with prefix), Curp (MX, 18 chars).
- **Religious / cultural considerations:** for AR patients, the system may display prayer-time reminders (the appointment cannot be at prayer time); for Jewish patients, the system may display Shabbat / Yom Tov reminders (the appointment cannot be on Shabbat / Yom Tov).
- **Language-specific medical terms:** some languages use different terms (e.g., "diabetes" is "السكري" in Arabic, "मधुमेह" in Hindi); the system uses locale-specific medical glossary.

**6. Translation management UI.** The tenant admin can:
- **View the per-locale completion %** (e.g., "Arabic: 87% complete, 142 keys missing").
- **Search / filter translations** by group, key, value, language.
- **Bulk edit** translations (paste a CSV / XLIFF).
- **Import / export** translations (JSON, CSV, XLIFF 1.2 / 2.0).
- **Auto-translate** missing keys via DeepL / Google Translate (paid add-on; uses the platform's LLM gateway per `19-Integrations §15.x`); the auto-translation is marked as `mt_pending_review` and the admin reviews before publishing.
- **Translation memory:** the system suggests translations from previously translated strings (e.g., "Cancel appointment" was translated to "إلغاء الموعد"; the next "Cancel {x}" string suggests the same verb).
- **Glossary:** the system supports a per-tenant glossary (e.g., "Clinic Name" should always be translated to a specific term); the glossary is enforced during auto-translation.
- **Per-locale approval:** some tenants have a 2-step workflow: translator → reviewer → publish.

**7. Domain Model.** Builds on §6.2. Adds:
```php
Schema::create('locale_profiles', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->string('locale', 10)->unique();                         // BCP 47 (e.g., 'ar', 'zh-CN')
    $t->string('name', 60);
    $t->string('native_name', 60);
    $t->string('direction', 3)->default('ltr');
    $t->string('numbering_system', 16)->default('western');      // 'western' | 'indian' | 'arabic'
    $t->string('calendar_preference', 16)->default('gregorian'); // 'gregorian' | 'hijri'
    $t->string('date_format', 32)->default('YYYY-MM-DD');
    $t->string('time_format', 8)->default('24h');                 // '12h' | '24h'
    $t->string('first_day_of_week', 3)->default('mon');           // 'sun' | 'mon' | 'sat'
    $t->string('currency_symbol_position', 8)->default('before'); // 'before' | 'after'
    $t->string('currency_decimal_separator', 1)->default('.');
    $t->string('currency_thousands_separator', 1)->default(',');
    $t->integer('currency_decimals')->default(2);
    $t->string('phone_format', 32)->nullable();
    $t->string('address_format', 32)->nullable();
    $t->string('rtl_mirror_strategy', 16)->default('full');       // 'full' | 'partial' | 'none'
    $t->boolean('is_active')->default(true);
    $t->boolean('is_beta')->default(false);
    $t->integer('translation_completion_pct')->default(0);
    $t->integer('missing_keys_count')->default(0);
    $t->timestamps();
});

Schema::create('translation_memory', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('source_locale', 10);
    $t->string('target_locale', 10);
    $t->string('source_text', 1024);
    $t->string('target_text', 1024);
    $t->integer('usage_count')->default(0);
    $t->timestamp('last_used_at')->nullable();
    $t->timestamps();
    $t->index(['tenant_id', 'source_locale', 'target_locale']);
});

Schema::create('translation_glossaries', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('locale', 10);
    $t->string('term', 120);
    $t->string('do_not_translate', 512)->nullable();             // a list of source terms that should map to `term`
    $t->text('description')->nullable();
    $t->timestamps();
});

Schema::create('translation_jobs', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('source_locale', 10);
    $t->string('target_locale', 10);
    $t->string('status', 16)->default('pending');                 // 'pending' | 'running' | 'completed' | 'failed'
    $t->string('provider', 16)->nullable();                       // 'deepl' | 'google' | 'openai'
    $t->integer('total_keys');
    $t->integer('translated_keys')->default(0);
    $t->integer('failed_keys')->default(0);
    $t->timestamp('started_at')->nullable();
    $t->timestamp('completed_at')->nullable();
    $t->timestamps();
});
```

**8. API Surface.**
```
# Public
GET    /api/v1/public/locales                                  -> [locale] (active, beta; with completion %)

# Tenant admin
GET    /api/v1.1/admin/locales                                  -> [locale] (with completion %, missing keys)
POST   /api/v1.1/admin/locales/{locale}/enable                  # { } -> 204
POST   /api/v1.1/admin/locales/{locale}/disable                 # { } -> 204
GET    /api/v1.1/admin/translations/{locale}                    # all keys for a locale (with status: translated, missing, mt_pending_review)
POST   /api/v1.1/admin/translations/import                      # { locale, format: 'json'|'xliff'|'csv', data } -> { imported, skipped, errors }
GET    /api/v1.1/admin/translations/export/{locale}             # download
POST   /api/v1.1/admin/translations/auto-translate              # { source_locale, target_locale, provider } -> translation_job
GET    /api/v1.1/admin/translations/jobs/{id}                   -> job status
GET    /api/v1.1/admin/translations/memory                      -> [translation_memory]
POST   /api/v1.1/admin/translations/glossaries                  # { locale, term, do_not_translate, description } -> glossary
GET    /api/v1.1/admin/translations/glossaries                  -> [glossary]
DELETE /api/v1.1/admin/translations/glossaries/{id}             # 204

# Patient-facing (per-user locale switcher)
GET    /api/v1.1/me/locale                                      -> { locale, direction, date_format, time_format, currency, first_day_of_week }
PATCH  /api/v1.1/me/locale                                      # { locale: 'ar' } -> 204
```

**9. Frontend.**
- **Patient app (`/settings/language`):** the language switcher shows the 10 GA languages with native name + flag; switching the language reloads the app with the new direction; the user's `locale` is persisted.
- **Admin (`/admin/settings/locales`):** the locale manager shows the 10 GA + 6 beta locales with completion %; clicking a locale shows the missing keys; the auto-translate button kicks off a background job; the glossary manager is per locale.
- **Doctor web (`/doctor/settings/locale`):** the doctor can pick the locale for their web UI; the doctor also picks the locale for their calendar invites (per `07-Appointments §24.x`).
- **RTL visual diff:** the CI dashboard (`/admin/ci/visual-regressions`) shows the per-locale visual diff; failed tests block the deploy.
- **Translation UI:** the translation editor is a 2-column layout (source language | target language) with key search, group filter, status filter, and inline edit.

**10. Realtime.**
- `locale.translated.{tenant_id}.{locale}.{key_count}` — a batch of translations was imported / auto-translated; the completion % updates.
- `locale.translation_job.completed.{tenant_id}.{job_id}` — the auto-translation job is done.
- `locale.user_switched.{tenant_id}.{user_id}` — a user switched their locale; the analytics dashboard updates.
- `locale.visual_regression.failed.{locale}` — a visual regression test failed for a locale; the CI is paged.

**11. Security & RBAC.**
- **Locale switch is per-user:** the user picks their own locale; the admin cannot force a locale on a user.
- **Translation edit requires `tenant_admin` or `content_manager`**; the changes are logged in `audit_logs`.
- **Auto-translation is opt-in:** the tenant must explicitly enable auto-translation; the cost is tracked.
- **Glossary enforcement:** the glossary is enforced during auto-translation; the system cannot translate a term that is in the glossary's `do_not_translate` list.
- **Beta locales are clearly marked:** the UI shows a "Beta — community-translated" badge; the user can opt in.
- **No PHI in translation keys:** the translation keys are static identifiers; the values are not PHI.
- **Roles:** `patient` / `doctor` (own locale), `tenant_admin` / `content_manager` (own locale management, translation edit), `super_admin` (global locale management, beta promotion), `compliance_officer` (view all, audit).

**12. Acceptance Criteria.**
- [ ] The 10 GA locales are enabled; switching the locale updates the UI direction (LTR / RTL) within 1s.
- [ ] The 6 beta locales are available with a "Beta" badge; the user can opt in.
- [ ] The full RTL pipeline works: HTML dir, CSS logical properties, Flutter Directionality, mirrored icons / animations / charts / PDFs / emails.
- [ ] The per-locale currency / date / time / number formatting is correct (verified by snapshot tests for each locale).
- [ ] The Hijri calendar display works for Saudi Arabia; the internal storage is Gregorian.
- [ ] The translation management UI supports import / export (JSON, CSV, XLIFF), bulk edit, search, and filter.
- [ ] The auto-translation job uses DeepL / Google / OpenAI; the result is marked `mt_pending_review`; the admin reviews before publishing.
- [ ] The translation memory suggests previously translated strings; the suggestion rate is > 30%.
- [ ] The glossary is enforced during auto-translation; the system does not translate a glossary term.
- [ ] The visual regression tests pass for all 10 GA locales; the CI fails on any pixel diff > 0.1%.
- [ ] The per-locale completion % is accurate; the missing keys count is correct.
- [ ] The patient name order is locale-aware (family first for AR / HE / UR / FA).
- [ ] The phone / address format is locale-aware.

**13. Real-World Test Scenarios.**
- *Arabic patient in Saudi Arabia:* the patient opens the app; the locale is `ar-SA`; the direction is RTL; the currency is `SAR`; the date is displayed in Hijri (10 Dhu al-Hijjah 1447); the patient name is in family-first order; the phone format is `٠٥ XXX XXX XXX`; the SMS is in Arabic.
- *Hebrew patient in Israel:* the patient opens the app; the locale is `he-IL`; the direction is RTL; the currency is `ILS`; the date is `DD/MM/YYYY`; the first day of the week is Sunday; the patient name is in family-first order.
- *Hindi patient in India:* the patient opens the app; the locale is `hi-IN`; the direction is LTR; the currency is `INR` with Indian grouping (`1,23,456.56`); the date is `DD/MM/YYYY`; the first day of the week is Monday; the patient name has honorific + father / husband name.
- *Switching locale:* an English-speaking patient switches to Spanish; the app re-renders with LTR; the currency is now `USD` (the tenant's default); the date is `MM/DD/YYYY`; the patient's `users.locale` is updated.
- *Auto-translation:* the admin clicks "Auto-translate Arabic" for 142 missing keys; the job uses DeepL; 140 keys are translated; 2 keys fail (medical jargon); the admin reviews the 140 translated keys and publishes.
- *Glossary enforcement:* the admin has a glossary entry "Clinic Name" = "عيادة Medicare" (do not translate); the auto-translation respects the glossary; the "Clinic Name" key is always translated to "عيادة Medicare".
- *Translation memory:* the admin translates "Cancel appointment" to "إلغاء الموعد"; the next "Cancel {x}" string suggests the same verb (إلغاء).
- *Visual regression:* the CI captures screenshots for the login page in 10 locales; the Arabic version is mirrored (text right-aligned, icons flipped); the CI fails if the pixel diff > 0.1%.
- *PDF report in Arabic:* the doctor generates a PDF lab report; the PDF is in Arabic with RTL layout; the doctor's signature is on the left (mirrored from the right).
- *Email in Hebrew:* the admin sends a marketing email in Hebrew; the email layout is RTL; the CTA button is on the left; the email renders correctly in Gmail / Outlook.
- *Prayer time reminder:* a Saudi patient has a 6 PM appointment; the system detects the Maghrib prayer time (5:45 PM) and suggests a 6:30 PM or later slot.
- *Hijri DOB:* a Saudi patient enters their DOB as "10 Muharram 1400"; the system converts to Gregorian (10 October 1979) for storage; the patient sees the Hijri date in their profile.

**14. Dev Workflow.** `php artisan locale:enable --locale=ar` enables a locale. `php artisan locale:disable --locale=fr` disables a locale. `php artisan translation:auto-translate --source=en --target=ar --provider=deepl` starts an auto-translation job. `php artisan translation:import --locale=ar --file=ar.json` imports translations. `php artisan translation:export --locale=ar --format=xliff` exports translations. `php artisan translation:memory-suggest --source="Cancel appointment" --target=ar` suggests a translation from memory. `php artisan locale:visual-regression --locale=ar` runs the visual regression tests. Code: `app-modules/i18n-rtl/`; tests: `app-modules/i18n-rtl/tests/Feature/{LocaleSwitch,RtlLayout,Formatter,HijriCalendar,TranslationMemory,Glossary,AutoTranslate,VisualRegression}Test.php` and `tests/Visual/rtl-snapshots/` for Playwright snapshot tests.

---

## 7. Currencies

### 7.1 Concept

Each monetary record stores the currency at the time of the transaction. The clinic has a default currency. Admins can configure symbol, position, and precision per currency.

### 7.2 Data Model

`currencies` (global, seeded)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `code` | varchar(3) unique | ISO 4217: `INR`, `USD`, `EUR`, `GBP`, `NGN` |
| `name` | varchar(60) | "Indian Rupee" |
| `symbol` | varchar(10) | "₹", "$", "€" |
| `symbol_position` | enum('before','after') | `$100` vs `100 €` |
| `decimal_separator` | varchar(1) default '.' | |
| `thousands_separator` | varchar(1) default ',' | |
| `decimal_places` | tinyint default 2 | JPY has 0 |
| `minor_unit_factor` | int default 100 | 100 for 2-decimal currencies, 1 for 0-decimal |
| `is_active` | boolean default true | |
| `sort_order` | int default 0 | |
| `created_at`, `updated_at` | timestamps |

Seeded: ~30 most common world currencies.

### 7.3 Formatting

```php
// app/Support/Money.php
public function format(?string $locale = null): string {
    $currency = Currency::find($this->currency);
    $formatted = number_format(
        $this->minor / $currency->minor_unit_factor,
        $currency->decimal_places,
        $currency->decimal_separator,
        $currency->thousands_separator,
    );
    return $currency->symbol_position === 'before'
        ? "{$currency->symbol}{$formatted}"
        : "{$formatted} {$currency->symbol}";
}
```

For Indian rupee: `₹1,50,000.00` (lakh formatting — see extension below).

### 7.4 Indian Numbering (Lakh/Crore)

The `intl` extension in PHP 8.3 has `NumberFormatter::PATTERN_DECIMAL` with the `¤##,##,##0.00` pattern. ICU's Indian locale does this correctly. The `Money` formatter detects `currency=INR` and uses ICU's `en_IN` numbering system.

### 7.5 API

```
GET    /api/v1/public/currencies
GET    /api/v1/admin/currencies
POST   /api/v1/admin/currencies
PUT    /api/v1/admin/currencies/{id}
DELETE /api/v1/admin/currencies/{id}        (blocked if used in any record)
```

---

## 8. Engagement Realtime Events

| Event | Channel | Use |
|---|---|---|
| `TestimonialApproved` | `user.{id}` (if submitted by user) | "Your testimonial is now live" |
| `ReviewReplied` | `patient.{id}` | FCM "Doctor replied to your review" |
| `NewBannerActivated` | public broadcast | Cache invalidation |
| `LanguageAdded` | `clinic.{tenant}.staff` | Admin bell |

---

## 9. Audit

`Testimonial`, `Banner`, `LoginSlide`, `DoctorReview`, `Language`, `Currency`, `Translation` all audited on create/update/delete. Reviews are additionally audited on view (for HIPAA).

---

## 10. Testing Strategy

| Test | Assertion |
|---|---|
| `TestimonialApprovalTest` | Only approved show publicly |
| `BannerScheduleTest` | Outside start/end window, not returned |
| `LoginSlideFallbackTest` | No tenant slides → global default used |
| `ReviewOnePerAppointmentTest` | Second submit → 409 |
| `ReviewReplyTest` | Doctor can reply once, edit, delete |
| `RatingRecomputeTest` | 5-star review → avg_rating updates within 100ms |
| `RatingHistogramTest` | Returns correct distribution |
| `TranslationFallbackTest` | Missing translation → English fallback |
| `CurrencyFormatTest` | $1,000.00, ₹1,50,000.00, 100¥ all correct |
| `RTLSupportTest` | Arabic locale → `dir="rtl"` set, layout flipped |
| `CrossTenantTest` | Review in tenant A invisible to tenant B |

---

## 11. Acceptance Criteria (V1)

- [ ] Patient can leave one review per completed appointment.
- [ ] Doctor can reply once; can edit/delete own reply.
- [ ] Rating histogram and avg update within 1 second of review save.
- [ ] Anonymous reviews hide patient name on display.
- [ ] Testimonials default to unapproved; admin approves.
- [ ] Banners are clickable and click count is tracked.
- [ ] Banner scheduled outside its window doesn't show.
- [ ] Login slides shown on first launch; "Don't show again" persists.
- [ ] Hindi and Arabic translations work end-to-end (UI + notifications + emails).
- [ ] Money formats correctly per currency: ₹1,50,000.00, $1,000.00, €1.000,00.
- [ ] Admin can add a new language without code deploy.
- [ ] Admin can add a new currency without code deploy.

---

## 12. V2 Roadmap — Engagement

> V1 ships testimonials, banners, login slides, reviews, languages, and currencies. V2 turns the engagement layer into a **content + community** platform with blogs, content workflows, AI-generated content, and personalized feeds.

### 12.1 Blogs / CMS (`blogs-cms`, `content-workflow`, V2)

```php
Schema::create('blog_categories', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->nullable()->index();
    $t->string('slug', 64);
    $t->string('name', 128);
    $t->string('description', 255)->nullable();
    $t->ulid('parent_id')->nullable();
    $t->integer('sort_order')->default(0);
    $t->boolean('is_active')->default(true);
    $t->timestamps();
    $t->unique(['tenant_id', 'slug']);
});

Schema::create('blog_posts', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('category_id')->nullable()->index();
    $t->ulid('author_user_id')->index();
    $t->string('slug', 255)->unique();
    $t->string('title', 255);
    $t->string('subtitle', 255)->nullable();
    $t->longText('content');                                  // Markdown or rich-text JSON
    $t->string('content_format', 16)->default('markdown');    // 'markdown' | 'html' | 'tiptap_json'
    $t->string('featured_image_s3_key')->nullable();
    $t->string('excerpt', 512)->nullable();
    $t->string('status', 16)->default('draft');               // 'draft' | 'in_review' | 'scheduled' | 'published' | 'archived'
    $t->ulid('reviewer_user_id')->nullable();
    $t->timestamp('reviewed_at')->nullable();
    $t->timestamp('published_at')->nullable()->index();
    $t->timestamp('scheduled_for')->nullable()->index();
    $t->integer('view_count')->default(0);
    $t->integer('read_time_minutes')->nullable();
    $t->json('seo')->nullable();                              // {meta_title, meta_description, og_image, canonical_url, noindex: bool}
    $t->json('tags')->nullable();
    $t->boolean('is_featured')->default(false);
    $t->boolean('is_pinned')->default(false);
    $t->timestamps();
    $t->softDeletes();
    $t->fullText(['title', 'excerpt', 'content']);            // MySQL FULLTEXT index
});

Schema::create('blog_revisions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('post_id')->index();
    $t->ulid('editor_user_id')->index();
    $t->longText('content');
    $t->string('title', 255);
    $t->text('change_summary')->nullable();
    $t->timestamps();
});

Schema::create('blog_post_reactions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('post_id')->index();
    $t->ulid('user_id')->index();
    $t->string('reaction', 16);                              // 'like' | 'love' | 'insightful' | 'helpful'
    $t->timestamps();
    $t->unique(['post_id', 'user_id', 'reaction']);
});

Schema::create('blog_comments', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('post_id')->index();
    $t->ulid('user_id')->nullable();
    $t->string('guest_name', 128)->nullable();
    $t->string('guest_email', 128)->nullable();
    $t->text('body');
    $t->ulid('parent_comment_id')->nullable();
    $t->string('status', 16)->default('pending');            // 'pending' | 'approved' | 'spam' | 'rejected'
    $t->integer('like_count')->default(0);
    $t->timestamps();
    $t->softDeletes();
});
```

Workflow: **draft → in_review → scheduled → published → archived**.

- **Author** creates draft, saves, submits for review.
- **Reviewer** (clinic admin) approves or rejects with feedback.
- **Scheduler** cron promotes scheduled posts to published when `scheduled_for <= now()`.
- **Public** site reads published posts only, filtered by tenant.

**Rich text editor:** TipTap (already used in admin), supports headings, lists, images (uploaded to S3), embeds (YouTube, Vimeo), tables, code blocks, markdown shortcuts.

**AI assistance (`ai-blog-writer`, V2.1):**
- `BlogAssistantService::generate(topic, keywords, locale, length)` produces a full draft using GPT-4o.
- `BlogAssistantService::rewrite(post, instruction)` rewrites per an instruction.
- `BlogAssistantService::suggestSEO(post)` suggests meta title/description.
- Doctor or admin can accept, edit, or reject.

**SEO:**
- Auto-generated sitemap.xml, RSS feed.
- Open Graph + Twitter Card metadata.
- Schema.org `BlogPosting` JSON-LD.
- `robots.txt` with `noindex` for archived.
- `next-seo` integration in the patient web app.

**Performance:**
- ISR (Incremental Static Regeneration) in Next.js: revalidate every 60s.
- Image optimization via `next/image`.
- Static export for the public blog, dynamic for admin.

### 12.2 Personalized content feed (`content-personalization`, V2.1)

Patient app home tab shows:
1. **Upcoming appointment** (if any).
2. **Active prescriptions** with refill CTA.
3. **Lab results** new since last visit.
4. **Recommended articles** based on `patient.chronic_conditions` and `patient.last_visit.specialization`.
5. **Vaccination reminders** if due.
6. **Doctor messages** (if in-app chat enabled).
7. **Promotions** from clinic (if opted in).

The "Recommended articles" use a vector similarity: patient's recent diagnoses → embed → cosine similarity to blog post embeddings → top 5.

### 12.3 Doctor profile v2 (V2.1)

- **Public profile page** with SEO: `/doctors/dr-jane-doe-cardiology` (uses V2 `doctors.slug`).
- **Embedded video** (YouTube/Vimeo) of the doctor introducing themselves.
- **Press mentions** (admin adds links).
- **Awards and publications** (admin adds).
- **Verified badge** (after KYC verification).
- **Share profile** generates a referral code; if a new patient books via that link, doctor gets ₹100 credit.

### 12.4 Community (V3)

- **Patient forum** (subreddit-style): patients post, comment, react. Moderation by clinic admin.
- **Q&A with doctors**: patients ask public questions; doctors answer; upvoted answers surface.
- **Success stories**: patient consents to share anonymized before/after for a condition.

(All community features are tenant-scoped, never cross-tenant. Moderation tools: report, hide, ban. Anti-spam: rate limit, CAPTCHA, ML classifier.)

### 12.5 V2 API additions

```
GET    /api/v2/blog/categories                             -> [category]
POST   /api/v2/blog/categories                             -> category
GET    /api/v2/blog/posts?status=published&category=&q=    -> [post]
GET    /api/v2/blog/posts/{slug}                           -> post
POST   /api/v2/blog/posts                                  -> post
PATCH  /api/v2/blog/posts/{id}                             -> post
DELETE /api/v2/blog/posts/{id}                             -> 204
POST   /api/v2/blog/posts/{id}/submit-review               -> status: in_review
POST   /api/v2/blog/posts/{id}/approve                     -> status: scheduled | published
POST   /api/v2/blog/posts/{id}/reject                      -> { reason }
POST   /api/v2/blog/posts/{id}/revisions                   -> revision
GET    /api/v2/blog/posts/{id}/revisions                   -> [revision]
POST   /api/v2/blog/posts/{id}/revert/{revision_id}        -> post
POST   /api/v2/blog/posts/{id}/ai/generate                 -> { topic, keywords } -> draft
POST   /api/v2/blog/posts/{id}/ai/rewrite                  -> { instruction } -> draft
POST   /api/v2/blog/posts/{id}/ai/seo                      -> { meta_title, meta_description }
POST   /api/v2/blog/posts/{id}/reactions                   -> reaction
GET    /api/v2/blog/posts/{id}/comments                    -> [comment]
POST   /api/v2/blog/posts/{id}/comments                    -> comment
POST   /api/v2/blog/comments/{id}/approve                  -> approved
POST   /api/v2/blog/comments/{id}/spam                     -> spam
GET    /api/v2/feed/me                                     -> personalized feed items
GET    /api/v2/doctors/{slug}                              -> public doctor profile
```

### 12.6 V2 acceptance criteria

- [ ] Blog editor saves a draft, submits for review, admin approves, post goes live.
- [ ] Scheduled post is published within 1 minute of `scheduled_for`.
- [ ] Public blog has valid Open Graph tags; shares look good on Twitter/LinkedIn.
- [ ] AI-generated blog post is editable, has SEO suggestions, can be regenerated.
- [ ] Personalized feed shows relevant articles to a patient with diabetes.
- [ ] Comments require moderation if the patient is new (first-time commenter).
- [ ] All V2 content is auditable and rollback-able via revisions.
