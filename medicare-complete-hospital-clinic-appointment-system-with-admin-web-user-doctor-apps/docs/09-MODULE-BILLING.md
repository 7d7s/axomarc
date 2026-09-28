# 09 — Billing Module (Invoices, Transactions, Coupons, Taxes)

> The money module. Tracks every cent the clinic earns, every discount given, every tax collected. Designed to be accountant-friendly and tax-compliant across multiple countries.

---

## 1. Scope

| In Scope | Out of Scope |
|---|---|
| Invoices (auto-created on booking, also manual) | Full accounting ledger / Tally export (V2) |
| Invoice line items (services, products) | Inventory (V2) |
| Tax calculation (inclusive/exclusive) per line | Multi-currency conversion (V2 spot rate) |
| Discounts (coupons, manual) | Loyalty points (V2) |
| Refunds (full, partial) | EMI / installment plans (V2) |
| Transaction records (every payment, refund) | Bank reconciliation (V2) |
| Tax masters (GST, VAT, etc.) | Country-specific tax filing exports (V2) |
| Receipts (PDF) | Credit notes (V2 — we have a `refund` workaround) |
| Reports: revenue, tax, doctor-wise | |
| Export to CSV / Excel | |

---

## 2. Money Conventions (Re-stated)

- All amounts stored as **integer minor units** (paisa for INR, cents for USD, kobo for NGN).
- A single `currency_id` on each monetary record (denormalized for historical accuracy even if clinic currency changes).
- `Money` value object wraps amount + currency + display formatting.
- Money math is `bcmath`, never float.

```php
namespace App\Support\Money;

final class Money {
    public function __construct(
        public readonly int $minor,            // 50000 = ₹500.00
        public readonly string $currency,       // 'INR'
    ) {}
    public function add(Money $other): Money { /* validate same currency */ }
    public function subtract(Money $other): Money {}
    public function multiply(int|float $factor): Money {}
    public function applyPercent(float $pct): Money {}   // discount or tax
    public function format(?string $locale = null): string {} // "₹500.00"
    public function toArray(): array { return ['amount' => $this->minor, 'currency' => $this->currency, 'formatted' => $this->format()]; }
}
```

---

## 3. Data Model

### 3.1 `invoices` (per-tenant)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `uuid` | char(36) unique | |
| `invoice_number` | varchar(40) unique per tenant | `INV-2026-000001` |
| `type` | enum('appointment','manual','subscription','other') | |
| `patient_id` | bigint FK nullable | Null for misc income |
| `appointment_id` | bigint FK unique nullable | For appointment invoices |
| `issued_at` | timestamp | |
| `due_at` | timestamp nullable | For credit invoices |
| `currency_id` | bigint FK | |
| `subtotal` | bigint | Sum of line amounts before tax/discount |
| `discount_total` | bigint default 0 | |
| `tax_total` | bigint default 0 | |
| `total` | bigint | Final amount payable |
| `amount_paid` | bigint default 0 | |
| `amount_due` | bigint | `total - amount_paid` |
| `status` | enum('draft','unpaid','partially_paid','paid','void','refunded') default 'unpaid' | |
| `paid_at` | timestamp nullable | |
| `voided_at` | timestamp nullable | |
| `voided_by_user_id` | bigint FK nullable | |
| `notes` | text nullable | Internal |
| `notes_to_patient` | text nullable | Shown on the PDF |
| `metadata` | json | |
| `created_by_user_id` | bigint FK | |
| `created_at`, `updated_at` | timestamps | |
| `deleted_at` | timestamp nullable | Soft |

Indexes: `(tenant_id, invoice_number)` UNIQUE, `(patient_id, issued_at desc)`, `(status, issued_at)`, `(appointment_id)` unique.

### 3.2 `invoice_lines` (per-tenant)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `invoice_id` | bigint FK | |
| `service_id` | bigint FK nullable | If a service line |
| `description` | varchar(255) | Free text fallback |
| `quantity` | decimal(8,2) default 1 | Can be fractional for hourly services |
| `unit_price` | bigint | Minor units per unit |
| `discount_percent` | decimal(5,2) default 0 | Line-level discount |
| `discount_amount` | bigint default 0 | Resolved |
| `tax_id` | bigint FK nullable | Tax master applied |
| `tax_mode` | enum('inclusive','exclusive') | |
| `tax_percent` | decimal(5,2) | Snapshot at line time |
| `tax_amount` | bigint | Resolved |
| `line_total` | bigint | Final line amount (after discount, after tax) |
| `sort_order` | int default 0 | |
| `metadata` | json | |

### 3.3 `transactions` (per-tenant, append-only)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `uuid` | char(36) unique | |
| `transaction_number` | varchar(40) unique per tenant | `TXN-2026-000001` |
| `type` | enum('payment','refund','adjustment') | |
| `direction` | enum('credit','debit') | Credit = money in; Debit = money out (refund) |
| `patient_id` | bigint FK nullable | |
| `invoice_id` | bigint FK nullable | |
| `appointment_id` | bigint FK nullable | |
| `payment_id` | bigint FK nullable | Link to gateway-agnostic Payment (see 10-PAYMENTS) |
| `refund_id` | bigint FK nullable | |
| `amount` | bigint | Positive |
| `currency_id` | bigint FK | |
| `status` | enum('pending','succeeded','failed','reversed') | |
| `payment_method` | enum('razorpay','stripe','paystack','cash','manual_bank','wallet','other') | |
| `gateway` | enum('razorpay','stripe','paystack','none') | |
| `gateway_transaction_id` | varchar(120) nullable | Gateway's ID |
| `gateway_order_id` | varchar(120) nullable | |
| `gateway_response` | json | Full raw response, for debugging |
| `processed_at` | timestamp | |
| `notes` | text nullable | |
| `created_by_user_id` | bigint FK | |
| `created_at`, `updated_at` | timestamps | |

Indexes: `(tenant_id, transaction_number)` UNIQUE, `(invoice_id)`, `(patient_id, processed_at desc)`, `(gateway, gateway_transaction_id)`.

### 3.4 `coupons` (per-tenant)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `code` | varchar(40) unique per tenant | Case-insensitive lookup via lowercase index |
| `name` | varchar(120) | Display name |
| `description` | text nullable | |
| `type` | enum('percentage','fixed') | |
| `value` | decimal(10,2) | Percent (0-100) or minor units |
| `max_discount_amount` | bigint nullable | Cap for percentage coupons |
| `min_invoice_amount` | bigint default 0 | |
| `currency_id` | bigint FK | For fixed-amount coupons |
| `applies_to` | enum('all','service','doctor','clinic') | Scope |
| `service_ids` | json | If applies_to=service |
| `doctor_ids` | json | If applies_to=doctor |
| `clinic_ids` | json | If applies_to=clinic |
| `usage_limit_total` | int nullable | Total across all users |
| `usage_limit_per_user` | int default 1 | |
| `used_count` | int default 0 | Cached, atomic increment |
| `starts_at` | timestamp nullable | |
| `expires_at` | timestamp nullable | |
| `is_active` | boolean default true | |
| `first_time_only` | boolean default false | New patients only |
| `created_at`, `updated_at` | timestamps |

Indexes: `(tenant_id, code)` UNIQUE, `(is_active, starts_at, expires_at)`.

### 3.5 `coupon_usages` (per-tenant)

| Column | Type |
|---|---|
| `id` | bigint PK |
| `coupon_id` | bigint FK |
| `invoice_id` | bigint FK |
| `patient_id` | bigint FK |
| `discount_amount` | bigint |
| `used_at` | timestamp |

Unique: `(coupon_id, invoice_id)`.

### 3.6 `taxes` (global master; can also be tenant-overridden)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK nullable | NULL = global default |
| `name` | varchar(60) | "GST 18%", "CGST 9%", "SGST 9%", "VAT 20%" |
| `code` | varchar(20) | "GST", "CGST", "SGST", "VAT" |
| `percent` | decimal(5,2) | 18.00 |
| `is_compound` | boolean default false | Tax on tax |
| `is_active` | boolean default true | |
| `sort_order` | int default 0 | |
| `metadata` | json | HSN/SAC code, country, etc. |
| `created_at`, `updated_at` | timestamps |

### 3.7 `refunds` (per-tenant)

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `transaction_id` | bigint FK | Original transaction |
| `invoice_id` | bigint FK nullable | |
| `amount` | bigint | |
| `currency_id` | bigint FK | |
| `reason` | text | |
| `status` | enum('pending','succeeded','failed') | |
| `gateway_refund_id` | varchar(120) nullable | |
| `gateway_response` | json | |
| `processed_at` | timestamp nullable | |
| `processed_by_user_id` | bigint FK | |
| `created_at`, `updated_at` | timestamps |

### 3.8 `invoice_sequences` (per-tenant per-year)

Same pattern as MRN and Rx sequences. Holds the counter for invoice numbers.

---

## 4. Invoice Numbering

Configurable per tenant in `settings.invoice_number_format`:
```json
{ "prefix": "INV", "separator": "-", "segments": [...same shape as MRN...] }
```

---

## 5. Money Flow (the Big Picture)

```
Patient books → Invoice created (unpaid)
            ↓
       Payment gateway (Razorpay/Stripe/Paystack)
            ↓
       Webhook: payment.captured
            ↓
       Transaction (succeeded) created
            ↓
       Invoice updated: amount_paid += tx.amount, status → paid
            ↓
       Slot marked booked, Appointment → confirmed
            ↓
       [Later] Cancellation request
            ↓
       Refund created
            ↓
       Transaction (refund, debit) created
            ↓
       Invoice updated: amount_paid -= refund.amount
            ↓
       If amount_paid == 0 → status = refunded
       If amount_paid > 0  → status = partially_refunded
```

---

## 6. Tax Calculation

### 6.1 Algorithm

For each line, given `unit_price × quantity` = `gross`:
- If `tax_mode = exclusive`:
  - `line_discount = applyDiscount(gross)`
  - `line_tax_base = line_discount`
  - `tax_amount = line_tax_base × tax_percent / 100`
  - `line_total = line_tax_base + tax_amount`
- If `tax_mode = inclusive`:
  - `line_discount = applyDiscount(gross)`
  - `tax_amount = line_discount × tax_percent / (100 + tax_percent)`
  - `line_total = line_discount`  (tax is inside)

### 6.2 Compound Tax

If a tax is `is_compound`, it's applied on top of the previous tax amount (e.g., service tax → GST on top).

### 6.3 Example

Two lines, GST 18% exclusive, line discount 10% on line 1:
```
Line 1: 1 × ₹1000 = ₹1000
  discount 10%   = -₹100
  taxable base   = ₹900
  GST 18%        = +₹162
  line_total     = ₹1062

Line 2: 1 × ₹500 = ₹500
  no discount
  GST 18%        = +₹90
  line_total     = ₹590

Subtotal    = ₹1500
Discount    = -₹100
Tax         = +₹252
Total       = ₹1652
```

---

## 7. Coupon Application

`CouponService::apply(Invoice $invoice, string $code): CouponApplication`

Validates:
1. Code exists and `is_active=true`
2. Within `starts_at` and `expires_at` window
3. Patient hasn't exceeded `usage_limit_per_user`
4. `usage_limit_total` not exceeded
5. `min_invoice_amount` met
6. `first_time_only` → no previous invoices for this patient
7. `applies_to` scope matches the line items (at least one match)

Computes:
- For `type=percentage`: discount = `min(subtotal × value / 100, max_discount_amount)`
- For `type=fixed`: discount = `min(value, subtotal)`

Recomputes tax base, tax amount, and line_total for affected lines. Writes a `coupon_usages` row. Increments `coupon.used_count` atomically.

### 7.1 Marketing Vouchers — `marketing-vouchers` (V1.1, MML 3) — deep-dive

**1. Overview.** V1 ships **per-tenant coupons** (the `coupons` table at §3.4): a tenant admin creates a code, sets a discount, and patients can apply it to their invoice. V1.1 extends this with **marketing vouchers** — a richer, multi-tenant-aware discount primitive that supports 4 distinct use cases:
- **Public promo codes** — the tenant publishes a code on social media, email, or a landing page; new patients redeem the code at first booking (e.g., "FIRSTVISIT20" for 20% off the first consultation).
- **Referral bonuses** — a patient (the referrer) shares a unique link or code; a new patient (the referee) signs up and books; both get a credit (e.g., ₹100 credit each).
- **Cross-tenant partner vouchers** — a marketing partner (e.g., a health insurance company, an employer, a corporate wellness program) buys a pool of voucher codes that work across multiple Medicare tenants; the partner is billed per redemption.
- **Influencer / affiliate codes** — an influencer (per `12-Engagement §3.4`) shares a code; redemptions are tracked; the influencer earns a commission.

MML 3 GA covers: the 4 voucher types, the redemption flow, the referral attribution, the cross-tenant pool, the influencer commission, the audit trail, and the analytics dashboard. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.1 lane module. The per-tenant coupon system is unchanged; the new functionality is additive.

**2. The 4 voucher types.**
- **Public promo (`voucher_type='public_promo'`):** a tenant publishes a code on its marketing channels; new patients (and sometimes existing) can apply it; single-use per patient (by default); can have a total usage cap and a date range.
- **Referral (`voucher_type='referral'`):** every patient has a unique referral code (auto-generated at signup, e.g., `PRIYA-7K3`); sharing the code or link tracks the referral; the referrer earns a credit when the referee completes a qualifying action (typically a first paid booking); the referee gets a one-time discount.
- **Partner pool (`voucher_type='partner_pool'`):** a partner (e.g., an insurance company) buys a pool of codes (e.g., 10,000 codes for ₹100 off); the codes work across all participating tenants; the partner is invoiced per redemption (typically monthly); the partner sees a redemption dashboard.
- **Influencer (`voucher_type='influencer'`):** an influencer is onboarded to the platform; they get a code (e.g., `DRSMILE20`); their followers use the code at booking; the influencer earns a commission (e.g., 10% of the booking value) tracked in `influencer_commissions`.

**3. The redemption flow.**
1. **Discovery:** the patient sees the code on a marketing channel (social media ad, email, friend's referral link) or in the app (the app shows a "Have a promo code?" input at checkout).
2. **Apply:** the patient types the code at the checkout / payment step. The `VoucherService::validate()` checks: code exists, is active, within date range, usage not exceeded, scope matches the line items, etc. The service returns the discount.
3. **Confirm:** the patient sees the discount applied to the invoice; the patient confirms and pays; the `voucher_redemptions` row is written; the `vouchers.used_count` is incremented atomically.
4. **Attribute (for referral / influencer):** the redemption is attributed to the referrer / influencer; the credit / commission is calculated and stored in `referral_credits` / `influencer_commissions`.
5. **Settle (for partner pool / influencer):** the partner / influencer is invoiced / paid per the settlement schedule (per `09-Billing §21.4` RCM).

**4. Referral attribution.** The referral flow is the most complex:
- Every patient has a `referral_code` (auto-generated; ULID-based with a human prefix).
- The patient shares the code or a link like `medicare.app/r/PRIYA-7K3`.
- The referee clicks the link; the app sets a `ref_code` cookie (30-day TTL).
- The referee signs up; the `users.referred_by_user_id` is set to the referrer.
- The referee completes the qualifying action (first paid booking); the `referral_credits` row is created for the referrer (e.g., ₹100) and a `referral_discount` is applied to the referee's invoice.
- The credit is a **wallet balance** (per `09-Billing §21.x`) usable on future bookings; it does not expire (or expires after 12 months, per the tenant's policy).
- **Fraud prevention:** self-referral (referrer = referee) is blocked; multi-account referral is detected (same device fingerprint, same payment method, same IP); the credit is held for 14 days (the refund window) and released if the referee's first booking is not refunded.

**5. Cross-tenant partner pool.** A partner (e.g., Star Health Insurance) buys a pool of 10,000 codes; the codes work across all tenants that opt in to the partner program; the codes are pre-validated at signup; the partner is invoiced per redemption (monthly, in arrears). The `partner_pools` table tracks the pool's total codes, used codes, billing rate, and settlement status. The partner has a dashboard at `medicare.app/partner/{partner_slug}` showing: codes redeemed, total spend, per-tenant breakdown, the next invoice, and the CSV export.

**6. Influencer commissions.** An influencer is onboarded (KYC, bank details for payout); they get a code; redemptions are tracked; commissions are calculated as a % of the booking value (or a flat fee, per the contract); commissions are settled monthly via the influencer's payout method (per Turn 52's `multi-currency-payout` module). The influencer dashboard at `medicare.app/influencer/{slug}` shows: total redemptions, total commission, the top 3 referring tenants, the trend, and the payout history.

**7. Domain Model.** (Builds on the existing `coupons` table at §3.4.) Adds:
```php
Schema::create('vouchers', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->nullable()->index();                // null = global (e.g., partner pool)
    $t->ulid('partner_id')->nullable()->index();               // for partner pool vouchers
    $t->ulid('influencer_id')->nullable()->index();             // for influencer vouchers
    $t->string('voucher_type', 32);                              // 'public_promo' | 'referral' | 'partner_pool' | 'influencer'
    $t->string('code', 64)->unique();                            // human-friendly code; case-insensitive lookup
    $t->string('name', 128);
    $t->text('description')->nullable();
    $t->string('discount_type', 16);                             // 'percentage' | 'fixed' | 'credit'
    $t->integer('discount_value');                                // percent or minor units
    $t->integer('max_discount_minor')->nullable();                // cap for percentage
    $t->integer('min_invoice_minor')->default(0);
    $t->char('currency', 3)->default('INR');
    $t->jsonb('applies_to')->nullable();                          // { service_ids, doctor_ids, clinic_ids, specialties, patient_segments }
    $t->jsonb('applies_to_patient')->nullable();                  // { first_time_only, min_age, max_age, gender, insurance }
    $t->integer('usage_limit_total')->nullable();                 // null = unlimited
    $t->integer('usage_limit_per_user')->default(1);
    $t->integer('used_count')->default(0);
    $t->integer('referral_credit_minor')->default(0);             // for the referrer
    $t->integer('referee_discount_minor')->default(0);            // for the referee
    $t->integer('commission_pct')->default(0);                    // for influencer commissions
    $t->timestamp('starts_at')->nullable();
    $t->timestamp('expires_at')->nullable();
    $t->boolean('is_active')->default(true);
    $t->timestamps();
    $t->index(['tenant_id', 'is_active', 'starts_at', 'expires_at']);
    $t->index(['voucher_type', 'code']);
});

Schema::create('voucher_redemptions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('voucher_id')->index();
    $t->ulid('tenant_id')->index();
    $t->ulid('invoice_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('referrer_patient_id')->nullable()->index();        // for referral
    $t->ulid('partner_id')->nullable()->index();                  // for partner pool
    $t->ulid('influencer_id')->nullable()->index();               // for influencer
    $t->integer('discount_minor');
    $t->char('currency', 3);
    $t->integer('referral_credit_minor')->default(0);             // the credit given to the referrer
    $t->integer('commission_minor')->default(0);                  // the commission for the influencer
    $t->string('status', 16)->default('confirmed');               // 'confirmed' | 'held' | 'released' | 'reversed'
    $t->timestamp('released_at')->nullable();
    $t->timestamps();
    $t->unique(['voucher_id', 'invoice_id']);
});

Schema::create('referral_credits', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('referrer_patient_id')->index();
    $t->ulid('referee_patient_id')->nullable()->index();
    $t->ulid('voucher_redemption_id')->nullable()->index();
    $t->integer('amount_minor');
    $t->char('currency', 3);
    $t->string('status', 16)->default('pending');                // 'pending' | 'available' | 'used' | 'expired' | 'reversed'
    $t->timestamp('available_at')->nullable();                    // after the refund window
    $t->timestamp('expires_at')->nullable();
    $t->timestamps();
});

Schema::create('partner_pools', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();                              // the Medicare-side coordinator
    $t->ulid('partner_id')->index();
    $t->string('partner_name', 128);
    $t->integer('total_codes');
    $t->integer('used_codes')->default(0);
    $t->integer('billing_rate_minor');                            // per redemption, in partner's currency
    $t->char('currency', 3);
    $t->date('pool_start_date');
    $t->date('pool_end_date')->nullable();
    $t->string('status', 16)->default('active');                  // 'active' | 'exhausted' | 'expired' | 'cancelled'
    $t->timestamps();
});

Schema::create('influencer_commissions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('influencer_id')->index();
    $t->ulid('voucher_redemption_id')->nullable()->index();
    $t->integer('gross_minor');
    $t->integer('commission_minor');
    $t->char('currency', 3);
    $t->string('status', 16)->default('pending');                // 'pending' | 'approved' | 'paid' | 'reversed'
    $t->string('payout_id')->nullable();
    $t->timestamps();
});
```

**8. API Surface.**
```
# Tenant admin
GET    /api/v1.1/admin/vouchers                                -> [voucher]
POST   /api/v1.1/admin/vouchers                                # { voucher_type, code, discount_type, value, ... } -> voucher
PATCH  /api/v1.1/admin/vouchers/{id}                           # { is_active?, usage_limit_total?, expires_at? }
DELETE /api/v1.1/admin/vouchers/{id}                           # 204
GET    /api/v1.1/admin/vouchers/{id}/redemptions               -> [redemption]
GET    /api/v1.1/admin/vouchers/stats                          # { total_vouchers, active, total_redemptions_mtd, total_discount_mtd, top_codes }

# Patient
POST   /api/v1.1/vouchers/validate                             # { code, invoice_id, patient_id } -> { valid, discount_minor, message }
POST   /api/v1.1/vouchers/apply                                # { code, invoice_id } -> { voucher_application_id, discount_minor }
GET    /api/v1.1/me/referral-code                              -> { code, link, credits_available, credits_pending, credits_used }
GET    /api/v1.1/me/referral-credits                           -> [referral_credit]
GET    /api/v1.1/me/voucher-redemptions                        -> [redemption]

# Partner
GET    /api/v1.1/partner/pools                                 -> [partner_pool]
GET    /api/v1.1/partner/pools/{id}                            -> pool (with codes, redemptions, billing)
GET    /api/v1.1/partner/pools/{id}/codes                      -> [voucher] (the codes in the pool)
GET    /api/v1.1/partner/pools/{id}/redemptions                -> [redemption] (per tenant, per code)
GET    /api/v1.1/partner/pools/{id}/invoice                    -> PDF (monthly invoice for the partner)

# Influencer
GET    /api/v1.1/influencer/dashboard                          -> { total_redemptions, total_commission, top_tenants, payout_history }
GET    /api/v1.1/influencer/redemptions                        -> [redemption] (per tenant, per code)
GET    /api/v1.1/influencer/commissions                         -> [commission]
POST   /api/v1.1/influencer/payout-request                     # { amount_minor, payout_method_id } -> payout
```

**9. Frontend.**
- **Patient:** at checkout, a "Have a promo code?" input is shown below the invoice total. Typing the code shows the discount applied. The "My referrals" page shows the patient's referral code, link, the credits available / pending / used, and the referees (anonymized).
- **Admin:** `/admin/marketing/vouchers` lists all vouchers with filters (type, status, date range). The voucher composer is a 4-step wizard: pick type → set discount → set scope / limits → publish. The redemptions view shows per-voucher usage with the per-tenant breakdown.
- **Partner:** `medicare.app/partner/{slug}` is a self-serve dashboard for the partner: pools, codes, redemptions, invoices, exports.
- **Influencer:** `medicare.app/influencer/{slug}` is a self-serve dashboard: redemptions, commissions, payout history, payout request button.

**10. Realtime.**
- `voucher.redeemed.{tenant_id}.{voucher_id}` — a voucher was redeemed; the admin dashboard updates.
- `voucher.exhausted.{tenant_id}.{voucher_id}` — the voucher's usage limit was reached; the voucher is auto-deactivated.
- `voucher.expires_soon.{tenant_id}.{voucher_id}` — the voucher expires in 7 days; the admin is notified.
- `referral.credit.available.{patient_id}` — a referral credit is now available (after the refund window).
- `partner.pool.exhausted.{partner_id}.{pool_id}` — a partner pool is exhausted; the partner is paged.

**11. Security & RBAC.**
- **Code uniqueness** is enforced at the DB level; the lookup is case-insensitive.
- **Self-referral is blocked** at the API layer.
- **Multi-account fraud detection:** the referral system uses a device fingerprint + IP + payment method hash to detect multi-account abuse; suspicious referrals are held (not auto-approved) for manual review.
- **Voucher tampering:** the discount is computed server-side; the client cannot modify the discount.
- **Partner pool billing:** the partner is invoiced per redemption; the platform tracks the pool's usage in real-time; the partner cannot exceed the pool's total codes (the API returns 410 if the pool is exhausted).
- **Influencer payout:** requires KYC (PAN, GSTIN, bank details); the platform is the payer of record (per `05-Doctors §16.5` multi-currency-payout).
- **Roles:** `tenant_admin` (own vouchers), `marketing_admin` (own vouchers, referral settings, partner pools), `partner_admin` (own partner pools, invoices), `influencer` (own dashboard, payout requests), `compliance_officer` (view all, audit).
- **Audit:** every voucher create / update / delete / redemption / credit / commission is logged in `audit_logs` with the actor and the affected entity.

**12. Acceptance Criteria.**
- [ ] A tenant admin can create a public promo voucher; the voucher can be redeemed at checkout; the discount is applied correctly.
- [ ] A patient has a unique referral code; sharing the code or link tracks the referral; the referrer earns a credit after the referee's first paid booking (after the refund window).
- [ ] Self-referral is blocked; multi-account abuse is detected and held for review.
- [ ] A partner can create a pool; the codes are pre-validated; the partner is invoiced per redemption; the partner dashboard shows the usage.
- [ ] An influencer can be onboarded (KYC + bank); the influencer's code is created; redemptions are tracked; commissions are calculated and paid monthly.
- [ ] The referral credit is held for 14 days (the refund window) and released if not refunded.
- [ ] The voucher's usage limit is respected; an exceeded usage limit auto-deactivates the voucher.
- [ ] The voucher's date range is respected; an expired voucher cannot be redeemed.
- [ ] The scope filters (service / doctor / clinic / specialty / patient segment) are enforced.
- [ ] The partner pool is exhausted; subsequent redemptions are rejected.
- [ ] The influencer payout requires KYC; the payout is processed via the multi-currency-payout module.

**13. Real-World Test Scenarios.**
- *Public promo:* clinic publishes "FIRSTVISIT20" for 20% off the first consultation; new patient books; the discount is applied; the voucher is redeemed; the usage count is incremented.
- *Referral:* patient Priya shares her code "PRIYA-7K3" with a friend; the friend signs up and books; Priya earns ₹100 credit (held for 14 days); after 14 days, the credit is available; Priya uses the credit on her next booking.
- *Self-referral attempt:* patient tries to use their own code; the API returns 403 with a "Cannot refer yourself" message.
- *Multi-account fraud:* a user creates 5 fake accounts to claim ₹500 in referral credits; the fraud detection flags the device fingerprint; the credits are held for review; the admin rejects; the credits are reversed.
- *Partner pool:* Star Health buys a pool of 10,000 codes for ₹100 off; 8,000 patients redeem; the partner is invoiced ₹8,00,000 (8,000 × ₹100) at the end of the month; the partner downloads the invoice; the partner pays via bank transfer.
- *Influencer commission:* Dr. Smile shares "DRSMILE20" for 20% off; 100 patients redeem; the total booking value is ₹2,00,000; the influencer's commission (10%) is ₹20,000; the commission is paid monthly via the influencer's bank account.
- *Voucher exhausted:* a voucher with `usage_limit_total=100` is redeemed 100 times; the next redemption returns 410 with "Voucher exhausted"; the voucher is auto-deactivated.
- *Voucher expired:* a voucher with `expires_at=yesterday` is attempted; the API returns 410 with "Voucher expired".
- *Scope mismatch:* a voucher for "cardiology only" is applied to a dermatology invoice; the API returns 422 with "Voucher not applicable to this service".
- *Cross-tenant partner pool:* Star Health's pool works in 5 tenants; the redemptions are tracked per tenant; the partner's dashboard shows the per-tenant breakdown.
- *Refund reverses the credit:* the referee's first booking is refunded; the referrer's credit is reversed; the credit status is `reversed`.

**14. What this module is NOT:**
- Not a marketing automation platform. Vouchers are a discount primitive; the marketing automation (email, push, segmentation) is a separate module (per `12-Engagement §3.4`).
- Not a customer data platform. We do not aggregate customer data for marketing; we just track the voucher redemption.
- Not a global cross-currency voucher. The voucher's currency is the clinic's base currency; cross-currency conversion is per `10-Payments §18.5` multi-currency (the partner's invoice is in the partner's preferred currency).
- Not a free add-on. The partner pool billing is a B2B feature; the influencer payout is real money out; the platform's pricing is per redemption.

**15. Dev Workflow.** `php artisan voucher:create --tenant=... --type=public_promo --code=FIRSTVISIT20 --discount=20%` creates a test voucher. `php artisan voucher:redeem --voucher=... --invoice=... --patient=...` simulates a redemption. `php artisan referral:credit --referee=... --amount=100` simulates a referral credit. `php artisan partner:pool-create --partner=star --codes=10000 --rate=100` creates a partner pool. `php artisan influencer:commission --influencer=... --redemption=...` calculates a commission. Code: `app-modules/marketing-vouchers/`; tests: `app-modules/marketing-vouchers/tests/Feature/{Create,Redeem,Referral,Partner,Influencer,Scope,Limit,Fraud}Test.php` and `tests/Performance/voucher-redeem.k6.js` (asserts 1k concurrent redemptions complete in < 1s).

---

## 8. Invoice Lifecycle

```
draft → unpaid → partially_paid → paid
                 ↘
                  void (admin)
                  
paid → refunded (full)
paid → partially_refunded (partial)
```

- `draft` is created when admin is building a manual invoice; not visible to patient.
- `unpaid` is set when an appointment invoice is created at booking time.
- A failed payment doesn't change status; the previous `unpaid` remains.
- Refund transitions: `paid → refunded` (full), `paid → partially_refunded` (partial).

---

## 9. Manual Invoice Creation (Accountant)

```
POST /api/v1/admin/invoices
{
  patient_id: 42,
  type: "manual",
  lines: [
    { description: "Custom procedure", quantity: 1, unit_price: 500000, tax_id: 1 }
  ],
  notes_to_patient?: "..."
}
```

- Status starts at `unpaid`.
- Patient gets a link to pay via the gateway (same Razorpay/Stripe flow as appointments).
- Or staff marks `cash` and records manually.

---

## 10. Cash / Manual Payment Recording

When payment is collected offline (cash, bank transfer, UPI without gateway link), staff records it:

```
POST /api/v1/admin/invoices/{id}/record-payment
{
  amount: 165200,
  method: "cash",   // or "manual_bank", "upi_manual"
  reference?: "Receipt #1234",
  notes?: "..."
}
```

- Creates a `Transaction` with `gateway=none, status=succeeded`.
- Requires `accountant` or `admin` role.
- Audit-logged with who recorded and the reference number.

---

## 11. Refund Processing

### 11.1 Standard Refund (gateway-original)

```
POST /api/v1/admin/refunds
{
  transaction_id: 123,
  amount: 100000,        // partial or full
  reason: "Patient cancelled >24h before"
}
```

- `RefundService::process` calls gateway refund API.
- On success: creates `Transaction (refund, debit)`, updates Invoice, fires `RefundProcessed` event.
- On failure: marks `refund.status=failed`, notifies admin.

### 11.2 Manual Refund (cash / bank)

Same endpoint with `method=cash` — no gateway call, just a record. Useful when a gateway refund can't be done (e.g., original card expired) and the clinic pays via bank transfer.

### 11.3 Refund Policy (per appointment)

See `07-MODULE-APPOINTMENTS.md` §8.3. The cancellation flow calls `RefundService::computeAndCreate` which uses the policy to decide amount.

### 11.4 Dynamic Refund Window — `dynamic-refund-window` (V1.1, MML 3) — deep-dive

**1. Overview.** V1 ships a **fixed refund window** of 14 days from the appointment date (per `07-MODULE-APPOINTMENTS.md §8.3` and the existing `RefundService::computeAndCreate`). The 14-day window is hard-coded and applies to every appointment, every patient, every service. V1.1 replaces this with a **dynamic, per-tenant configurable refund policy** that supports:
- **Per-service refund windows** — telemedicine gets 0 hours, dental gets 48 hours, elective surgery gets 30 days.
- **Per-doctor override** — a senior consultant can set a longer window for premium services.
- **Per-clinic override** — a chain can set a uniform policy across all its clinics.
- **Per-patient-segment override** — insurance patients, corporate patients, and walk-in patients can have different windows.
- **Non-monetary resolutions** — credit, rebook, or partial refund as alternatives to a full monetary refund.
- **Patient-initiated vs clinic-initiated** — different rules for who initiated the cancellation (e.g., clinic cancelled = full refund or rebook; patient cancelled = sliding scale).

MML 3 GA covers: the per-service / per-doctor / per-clinic / per-segment windows, the non-monetary resolution options, the patient-vs-clinic-initiated logic, the policy composer in the admin UI, the policy preview in the booking flow, the audit trail, and the analytics dashboard. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.1 lane module. The 14-day default remains the fallback for tenants that do not configure a policy.

**2. Resolution types.** The dynamic policy supports 4 resolution types:
- **Full monetary refund** — the entire amount is refunded to the original payment method.
- **Partial monetary refund** — a percentage or fixed amount is refunded; the patient pays a cancellation fee.
- **Wallet credit** — the entire amount is credited to the patient's wallet (per `09-Billing §21.x`); the credit is non-refundable and usable for 12 months (per the tenant's policy).
- **Free rebook** — the patient can rebook the same service with the same doctor for free; the original payment is credited toward the new booking; if the new booking is more expensive, the patient pays the difference.

The admin can configure the resolution by case (e.g., patient-initiated = partial or credit; clinic-initiated = full refund or free rebook).

**3. The sliding scale.** A typical sliding scale is:
- **48 hours before appointment:** full refund or free rebook.
- **24–48 hours before:** 80% refund or 100% credit.
- **2–24 hours before:** 50% refund or 100% credit.
- **Less than 2 hours / no-show:** 0% refund or 50% credit (configurable per clinic; some clinics opt for "no refund, no credit" for no-shows).
- **Clinic-initiated cancellation:** always 100% refund or free rebook (per the platform's "fair cancellation" policy).
- **Force majeure / natural disaster:** always 100% refund (admin override).

The scale is configurable per service / per clinic; the admin can use a default template or compose a custom scale.

**4. Domain Model.**
```php
Schema::create('refund_policies', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 128);                                     // "Default Telemedicine Policy", "Dental Standard", etc.
    $t->string('scope', 32);                                      // 'tenant' | 'clinic' | 'service' | 'doctor' | 'patient_segment'
    $t->ulid('clinic_id')->nullable()->index();
    $t->ulid('service_id')->nullable()->index();
    $t->ulid('doctor_id')->nullable()->index();
    $t->string('patient_segment', 32)->nullable();               // 'insurance' | 'corporate' | 'walk_in' | 'self_pay' | 'all'
    $t->integer('priority')->default(0);                          // higher priority wins on overlap
    $t->jsonb('sliding_scale');                                   // [{ hours_before: 48, refund_pct: 100, credit_pct: 100 }, { hours_before: 24, refund_pct: 80, credit_pct: 100 }, ...]
    $t->jsonb('resolution_options');                              // ['full_refund', 'partial_refund', 'credit', 'free_rebook']
    $t->jsonb('initiator_rules');                                 // { 'patient': { 'default': 'partial', 'within_window': 'full' }, 'clinic': { 'default': 'full' } }
    $t->integer('credit_expiry_months')->default(12);
    $t->integer('cancellation_fee_minor')->default(0);             // flat fee for partial refunds
    $t->integer('cancellation_fee_pct')->default(0);               // % of booking for partial refunds
    $t->boolean('allow_no_show_credit')->default(false);
    $t->boolean('is_default')->default(false);                    // the tenant's default policy
    $t->boolean('is_active')->default(true);
    $t->timestamps();
    $t->index(['tenant_id', 'scope', 'is_active']);
});

Schema::create('refund_window_calculations', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('appointment_id')->index();
    $t->ulid('refund_policy_id')->index();
    $t->integer('hours_before');
    $t->string('resolved_resolution', 32);                        // the chosen resolution
    $t->integer('refund_minor');
    $t->integer('credit_minor');
    $t->integer('cancellation_fee_minor');
    $t->string('initiator', 16);                                  // 'patient' | 'clinic' | 'doctor' | 'system'
    $t->string('reason', 64)->nullable();
    $t->timestamps();
});
```

**5. API Surface.**
```
# Tenant admin
GET    /api/v1.1/admin/refund-policies                          -> [policy]
POST   /api/v1.1/admin/refund-policies                          # { name, scope, sliding_scale, resolution_options, ... } -> policy
PATCH  /api/v1.1/admin/refund-policies/{id}                     # { sliding_scale?, resolution_options?, is_active? }
DELETE /api/v1.1/admin/refund-policies/{id}                     # 204
POST   /api/v1.1/admin/refund-policies/preview                  # { service_id?, doctor_id?, clinic_id?, patient_segment?, hours_before } -> { resolved_policy, refund_pct, credit_pct, fee_pct }

# Patient (booking flow)
GET    /api/v1.1/appointments/{id}/refund-preview               # { hours_before, refund_minor, credit_minor, options: ['full_refund', 'credit', 'free_rebook'] }
POST   /api/v1.1/appointments/{id}/cancel                       # { resolution: 'full_refund' | 'partial_refund' | 'credit' | 'free_rebook', new_appointment_id? } -> refund_application

# Reports
GET    /api/v1.1/admin/reports/refund-window-effectiveness      # { policies, total_refunds, total_credits, avg_refund_pct, no_show_rate, by_initiator }
```

**6. Frontend.**
- **Admin (`/admin/policies/refund`):** the policy composer is a form with: name, scope (tenant / clinic / service / doctor / patient_segment), the sliding scale table (hours before × refund% × credit%), the resolution options checkboxes, the initiator rules, the credit expiry. The preview button shows the resolved policy for a given scenario (e.g., "Service X, 24 hours before, patient-initiated → 80% refund or 100% credit").
- **Patient (booking flow):** the "Cancel appointment" page shows the policy preview (e.g., "Cancel 24 hours before: 80% refund, or 100% credit, or free rebook"). The patient picks a resolution; the refund or credit is processed.
- **Reports:** a refund-effectiveness dashboard shows the total refunds, total credits, average refund %, no-show rate, breakdown by initiator (patient vs clinic).

**7. Realtime.**
- `refund.computed.{tenant_id}.{appointment_id}` — the refund preview was computed (for live updates in the admin preview).
- `refund.resolution.chosen.{tenant_id}.{appointment_id}` — the patient chose a resolution; the appointment is updated.
- `refund.policy.changed.{tenant_id}` — a refund policy was changed; the next appointments use the new policy.
- `refund.credit.expiring.{patient_id}.{credit_id}` — a credit expires in 7 days; the patient is notified.

**8. Security & RBAC.**
- **Policy resolution is server-side:** the client cannot manipulate the hours_before or the refund %.
- **Policy change requires `tenant_admin` or `marketing_admin`**; the change is logged in `audit_logs`.
- **Patient picks the resolution**; the resolution is enforced by the gateway (per the gateway's refund flow).
- **Free rebook requires same service + same doctor**; the system enforces this.
- **Credit expiry is strict**; expired credits cannot be redeemed.
- **Roles:** `tenant_admin` (own policies), `marketing_admin` (own policies), `compliance_officer` (view all, audit), `patient` (own bookings, own refund preview, own resolution).

**9. Acceptance Criteria.**
- [ ] A tenant admin can create a per-service refund policy (e.g., "Telemedicine: 0 hours, full refund or credit").
- [ ] A tenant admin can create a per-clinic policy that overrides the tenant default.
- [ ] A tenant admin can create a per-patient-segment policy (e.g., "Insurance: 30 days, full refund").
- [ ] The resolution priority is: doctor override > service > clinic > patient_segment > tenant default.
- [ ] The sliding scale is enforced: 48 hours = 100% refund, 24 hours = 80% refund, 2 hours = 50% refund, 0 hours = 0% refund (per the config).
- [ ] The patient sees the policy preview at cancellation; the patient picks a resolution.
- [ ] Clinic-initiated cancellation = full refund or free rebook; patient-initiated = sliding scale.
- [ ] The credit expires after the configured months (default 12).
- [ ] The no-show credit is configurable; some clinics opt for "no refund, no credit" for no-shows.
- [ ] The force majeure override refunds 100% always.
- [ ] The free rebook requires the same service + same doctor; the system enforces this.

**10. Real-World Test Scenarios.**
- *Telemedicine cancellation:* patient cancels 1 hour before; the telemedicine policy is "0 hours, 100% credit"; the patient is offered credit or no-refund; the patient picks credit; the credit is added to the wallet.
- *Dental cancellation:* patient cancels 24 hours before; the dental policy is "24 hours, 80% refund, 100% credit"; the patient picks refund; 80% is refunded; the cancellation fee is 20%.
- *No-show:* patient does not show up; the policy is "no refund, no credit"; the patient's wallet is untouched; the clinic keeps the full amount.
- *Clinic-initiated cancellation:* doctor calls in sick; the clinic cancels the appointment; the policy is "full refund or free rebook"; the patient picks free rebook; the system creates a new appointment; the original payment is credited; the new appointment is at no extra cost.
- *Per-patient-segment:* an insurance patient cancels 7 days before; the insurance policy is "30 days, full refund"; the refund is processed; the insurance patient's invoice is updated.
- *Per-doctor override:* Dr. Premium has a "30 days, full refund, no questions asked" policy; the standard policy is "14 days, 80% refund"; Dr. Premium's policy wins (higher priority).
- *Sliding scale edge case:* the policy is 24 hours = 100%, 12 hours = 50%; the patient cancels at 18 hours (between 12 and 24); the policy resolves to 50% (the closer, more restrictive bucket).
- *Credit expiry:* a credit issued on 2025-01-01 expires on 2025-12-31 (12 months); the patient tries to use it on 2026-01-01; the API returns 410 with "Credit expired".
- *Free rebook with price increase:* the new doctor charges ₹200 more; the patient's original payment is credited; the patient pays the ₹200 difference at the new booking.
- *Force majeure override:* an admin marks a cancellation as "flood" (force majeure); the policy is overridden to 100% refund; the audit log records the override.

**11. What this module is NOT:**
- Not a refund gateway. The dynamic refund window is a policy / decision engine; the actual refund is processed by the existing `RefundService` and the gateway.
- Not a fraud detection system. We do not detect fraudulent cancellation patterns; that is a separate analytics concern.
- Not a chargeback / dispute system. Chargebacks are handled by the gateway; the platform records the chargeback event but does not process the dispute.
- Not a partial service delivery refund. The service is either delivered or not; partial delivery is out of scope.

**12. Dev Workflow.** `php artisan refund-policy:create --tenant=... --scope=service --service=... --scale=...` creates a refund policy. `php artisan refund-policy:preview --appointment=... --resolution=...` previews the resolution. `php artisan refund-window:apply --appointment=... --resolution=...` applies a resolution. Code: `app-modules/dynamic-refund-window/`; tests: `app-modules/dynamic-refund-window/tests/Feature/{CreatePolicy,Priority,SlidingScale,InitiatorRules,Credit,FreeRebook,Expiry,ForceMajeure}Test.php`.

---

## 12. Reports

### 12.1 Revenue Report

```
GET /api/v1/admin/reports/revenue
  ?from=2026-06-01&to=2026-06-30
  &group_by=day|week|month|doctor|service|clinic
  &clinic_id=
  &doctor_id=
```

Returns:
```json
{
  "summary": {
    "total_revenue": 5400000,
    "total_refunds": 250000,
    "net_revenue": 5150000,
    "total_tax": 477966,
    "total_discounts": 100000,
    "transaction_count": 342,
    "currency": "INR"
  },
  "series": [
    { "period": "2026-06-01", "revenue": 180000, "refunds": 0, "net": 180000 },
    ...
  ],
  "breakdown": [
    { "label": "Dr. Priya Mehta", "revenue": 1200000, "count": 87 }
  ]
}
```

### 12.2 Tax Report

```
GET /api/v1/admin/reports/tax?from=&to=&tax_id=
```

Returns per-tax summary (collected, refunded, net). Exportable as CSV in the format expected by Indian GST portal (V2 has a specific format).

### 12.3 Doctor-wise Earning

```
GET /api/v1/admin/reports/doctor-earnings?from=&to=&doctor_id=
```

Returns gross, net of refunds, platform fee (if multi-tenant SaaS), payout due.

### 12.4 Transaction Report

```
GET /api/v1/admin/reports/transactions?from=&to=&type=&method=&gateway=&status=
```

Paginated list with all fields, exportable CSV.

### 12.5 Export

Every report endpoint supports `?format=csv|xlsx|pdf` for export. CSV is generated server-side via `league/csv`; XLSX via `openspout/openspout`; PDF via the same PDF engine as Rx.

---

## 13. API Endpoints

### 13.1 Invoices

```
GET    /api/v1/admin/invoices?status=&patient_id=&from=&to=&page=
GET    /api/v1/admin/invoices/{id}
POST   /api/v1/admin/invoices                  (create manual)
PUT    /api/v1/admin/invoices/{id}              (only if draft/unpaid)
DELETE /api/v1/admin/invoices/{id}              (only if draft)
POST   /api/v1/admin/invoices/{id}/void         { reason }
POST   /api/v1/admin/invoices/{id}/send         (emails the PDF to patient)
GET    /api/v1/admin/invoices/{id}/pdf

GET    /api/v1/patient/invoices?status=&page=
GET    /api/v1/patient/invoices/{id}
GET    /api/v1/patient/invoices/{id}/pdf
```

### 13.2 Transactions

```
GET    /api/v1/admin/transactions?type=&method=&status=&from=&to=&page=
GET    /api/v1/admin/transactions/{id}
GET    /api/v1/patient/transactions?from=&to=&page=
```

### 13.3 Coupons

```
GET    /api/v1/admin/coupons?is_active=&page=
POST   /api/v1/admin/coupons
GET    /api/v1/admin/coupons/{id}
PUT    /api/v1/admin/coupons/{id}
DELETE /api/v1/admin/coupons/{id}              (soft, blocked if used)
GET    /api/v1/admin/coupons/{id}/usages
POST   /api/v1/public/coupons/validate         { code, invoice_subtotal, service_ids? }
                                            → { valid, discount_amount, message }
```

### 13.4 Taxes

```
GET    /api/v1/admin/taxes
POST   /api/v1/admin/taxes
PUT    /api/v1/admin/taxes/{id}
DELETE /api/v1/admin/taxes/{id}                (blocked if used in any invoice)
```

### 13.5 Refunds

```
GET    /api/v1/admin/refunds?status=&from=&to=&page=
GET    /api/v1/admin/refunds/{id}
POST   /api/v1/admin/refunds
POST   /api/v1/admin/refunds/{id}/retry        (on gateway failure)
```

### 13.6 Reports

(see §12 for query params)

```
GET    /api/v1/admin/reports/revenue
GET    /api/v1/admin/reports/tax
GET    /api/v1/admin/reports/doctor-earnings
GET    /api/v1/admin/reports/transactions
GET    /api/v1/admin/reports/dashboard          (summary cards)
```

---

## 14. Service Methods

```php
namespace App\Modules\Billing\Services;

class InvoiceService {
    public const VERSION = '1.0.0';

    public function createForAppointment(Appointment $a): Invoice;
    public function createManual(User $actor, array $data): Invoice;
    public function addLine(Invoice $i, array $data): InvoiceLine;
    public function removeLine(InvoiceLine $l): void;
    public function applyCoupon(Invoice $i, string $code): CouponApplication;
    public function removeCoupon(Invoice $i): void;
    public function recompute(Invoice $i): Invoice;          // after any line change
    public function send(Invoice $i): void;                  // email PDF
    public function void(Invoice $i, User $actor, string $reason): void;
    public function markPaid(Invoice $i, Transaction $tx): void;
    public function applyRefund(Invoice $i, Refund $r): void;
    public function generatePdf(Invoice $i): string;
}

class TransactionService {
    public function recordPayment(Invoice $i, array $data, User $actor): Transaction;
    public function recordGatewayPayment(Invoice $i, PaymentGatewayResult $result): Transaction;
    public function reverse(Transaction $t, string $reason): void;
}

class CouponService {
    public function validate(Coupon $c, Invoice $i): CouponValidation;
    public function apply(Invoice $i, string $code): CouponApplication;
    public function incrementUsage(Coupon $c, Invoice $i): void;
}

class TaxService {
    public function computeLineTax(InvoiceLine $l, Money $taxableBase): Money;
    public function getActiveForLine(InvoiceLine $l): ?Tax;
}

class RefundService {
    public function computeAndCreate(Appointment $a, string $reason, User $actor): ?Refund;
    public function process(Transaction $t, int $amount, string $reason, User $actor): Refund;
    public function retry(Refund $r): void;
}
```

---

## 15. Validation

```php
// CreateManualInvoiceRequest
public function rules(): array {
    return [
        'patient_id'    => ['required', 'exists:patients,id'],
        'type'          => ['required', Rule::in(['manual', 'other'])],
        'lines'         => ['required', 'array', 'min:1', 'max:50'],
        'lines.*.description'  => ['required_without:lines.*.service_id', 'string', 'max:255'],
        'lines.*.service_id'   => ['nullable', 'exists:services,id'],
        'lines.*.quantity'     => ['required', 'numeric', 'min:0.01'],
        'lines.*.unit_price'   => ['required', 'integer', 'min:0'],
        'lines.*.tax_id'       => ['nullable', 'exists:taxes,id'],
        'lines.*.tax_mode'     => ['required_with:lines.*.tax_id', Rule::in(['inclusive', 'exclusive'])],
        'lines.*.discount_percent' => ['nullable', 'numeric', 'min:0', 'max:100'],
        'currency_id'   => ['required', 'exists:currencies,id'],
        'due_at'        => ['nullable', 'date', 'after:today'],
        'notes_to_patient' => ['nullable', 'string', 'max:1000'],
        'coupon_code'   => ['nullable', 'string', 'max:40'],
    ];
}

// CreateCouponRequest
public function rules(): array {
    return [
        'code'          => ['required', 'string', 'max:40', 'regex:/^[A-Z0-9_-]+$/i'],
        'name'          => ['required', 'string', 'max:120'],
        'type'          => ['required', Rule::in(['percentage', 'fixed'])],
        'value'         => ['required', 'numeric', 'min:0'],
        'max_discount_amount' => ['nullable', 'integer', 'min:0'],
        'min_invoice_amount'  => ['nullable', 'integer', 'min:0'],
        'usage_limit_total'   => ['nullable', 'integer', 'min:1'],
        'usage_limit_per_user'=> ['nullable', 'integer', 'min:1'],
        'starts_at'     => ['nullable', 'date'],
        'expires_at'    => ['nullable', 'date', 'after:starts_at'],
        'applies_to'    => ['required', Rule::in(['all', 'service', 'doctor', 'clinic'])],
        'first_time_only' => ['boolean'],
    ];
}
```

---

## 16. Realtime Events

| Event | Channel | Use |
|---|---|---|
| `InvoiceCreated` | `patient.{id}`, `clinic.{tenant}.staff` | Patient sees new invoice; staff dashboard updates revenue |
| `InvoicePaid` | `patient.{id}`, `clinic.{tenant}.staff` | UI updates; dashboard revenue increments |
| `RefundProcessed` | `patient.{id}`, `clinic.{tenant}.staff` | Patient gets FCM with refund receipt |
| `CouponUsage` | `clinic.{tenant}.staff` | Live coupon counter |
| `InvoiceVoided` | `patient.{id}`, `clinic.{tenant}.staff` | |

---

## 17. Cron Jobs

| Job | Schedule | Purpose |
|---|---|---|
| `InvoiceOverdueReminderJob` | daily 09:00 clinic-TZ | Find unpaid invoices past `due_at` → email patient + admin |
| `ReconcilePendingTransactionsJob` | every 30 min | Find `Transaction.status=pending` older than 1h → mark failed, release slot |
| `ResetUsageCountersJob` | monthly | V1 doesn't reset coupons; placeholder for future |

---

## 18. Audit

All `Invoice`, `Transaction`, `Refund`, `Coupon` mutations are audited. `amount` changes are diff-logged. Every cash payment is audit-logged with the staff member's ID — required for cash-handling compliance.

---

## 19. Testing Strategy

| Test | Assertion |
|---|---|
| `InvoiceCreateFromAppointmentTest` | Auto-created, status=unpaid, line item matches service |
| `TaxCalculationTest` | Inclusive/exclusive/compound math correct to the paisa |
| `CouponApplicationTest` | All validation rules; cap by max_discount; per-user limit |
| `ConcurrentCouponUseTest` | 100 parallel uses → `used_count=100`, no over-use |
| `PaymentWebhookTest` | Marks invoice paid, slot booked, appointment confirmed |
| `PartialRefundTest` | Invoice status=partially_refunded, math correct |
| `FullRefundTest` | Invoice status=refunded, transaction reversed |
| `CashPaymentTest` | Recorded without gateway, audit row exists |
| `DoctorEarningsTest` | Sum of completed appointments minus refunds |
| `TaxReportTest` | Matches sum of invoice_lines.tax_amount |
| `CrossTenantTest` | Invoice in tenant A invisible to tenant B |

---

## 20. Acceptance Criteria (V1)

- [ ] Every appointment creates a corresponding invoice.
- [ ] Coupon validation catches: expired, over-used, below minimum, scope mismatch.
- [ ] Tax math is correct for inclusive, exclusive, and compound.
- [ ] Refunds process via the original gateway and update the invoice correctly.
- [ ] Cash payments can be recorded by staff with a reference number.
- [ ] Doctor earnings report matches the sum of their completed appointments minus refunds.
- [ ] CSV export of transactions opens cleanly in Excel.
- [ ] Invoice PDF includes clinic letterhead, line items, tax breakdown, total.
- [ ] All money fields are integer minor units (no floats anywhere).
- [ ] All financial mutations are audited.

---

## 21. V2 Roadmap — Billing

> V1 bills per appointment and handles coupons, taxes, and refunds. V2 turns the billing module into a full **revenue cycle management (RCM)** platform: subscriptions, insurance claims, denials, dunning, packages, e-invoicing, and chain revenue splits.

### 21.1 Subscription billing (`subscription-billing`, V2)

SaaS tenants pay a recurring subscription. Doctors can also offer patients **packages** (e.g., "10 physiotherapy sessions for ₹9,999").

```php
Schema::create('subscription_plans', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->nullable()->index();             // null = platform-wide plan
    $t->string('slug', 32)->unique();                       // 'starter' | 'pro' | 'clinic' | 'enterprise'
    $t->string('name', 64);
    $t->text('description')->nullable();
    $t->decimal('price', 12, 2);
    $t->char('currency', 3);
    $t->string('interval', 16);                              // 'month' | 'year'
    $t->integer('interval_count')->default(1);
    $t->integer('trial_days')->default(0);
    $t->json('feature_flags')->nullable();                   // ['recurring-appointments', 'whatsapp-business']
    $t->json('limits')->nullable();                          // { max_doctors: 5, max_appointments_per_month: 1000 }
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});

Schema::create('subscriptions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->unique();                        // one subscription per tenant
    $t->ulid('plan_id')->index();
    $t->string('gateway', 32);                              // 'razorpay' | 'stripe' | 'paystack'
    $t->string('gateway_subscription_id', 128)->unique();
    $t->string('status', 32)->index();                      // 'trialing' | 'active' | 'past_due' | 'paused' | 'cancelled' | 'expired'
    $t->timestamp('trial_ends_at')->nullable();
    $t->timestamp('current_period_start');
    $t->timestamp('current_period_end')->index();
    $t->boolean('cancel_at_period_end')->default(false);
    $t->timestamp('cancelled_at')->nullable();
    $t->json('metadata')->nullable();
    $t->timestamps();
});

Schema::create('subscription_items', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('subscription_id')->index();
    $t->string('type', 32);                                  // 'plan' | 'addon' | 'usage'
    $t->ulid('plan_id')->nullable();
    $t->string('addon_slug', 64)->nullable();
    $t->decimal('quantity', 12, 2)->default(1);              // can be fractional for usage
    $t->decimal('unit_price', 12, 2);
    $t->char('currency', 3);
    $t->json('metadata')->nullable();
    $t->timestamps();
});

Schema::create('subscription_invoices', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('subscription_id')->index();
    $t->ulid('invoice_id')->nullable()->index();            // links to the regular invoices table
    $t->decimal('amount', 12, 2);
    $t->char('currency', 3);
    $t->string('status', 16);                                // 'draft' | 'open' | 'paid' | 'failed' | 'void'
    $t->timestamp('period_start');
    $t->timestamp('period_end');
    $t->timestamp('paid_at')->nullable();
    $t->timestamp('attempted_at')->nullable();
    $t->integer('attempt_count')->default(0);
    $t->timestamps();
});
```

`SubscriptionService` listens to gateway webhooks (`subscription.activated`, `subscription.renewed`, `subscription.payment_failed`, `subscription.cancelled`) and updates the local mirror. Dunning: 3 attempts (day 0, 3, 7), then service degrades (no new appointments, read-only).

### 21.2 Patient packages (V2)

A doctor can sell a **package** to a patient: "10 physiotherapy sessions + 2 free".

```php
Schema::create('packages', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('clinic_id')->nullable()->index();
    $t->ulid('doctor_id')->nullable()->index();
    $t->string('name', 128);
    $t->text('description')->nullable();
    $t->decimal('price', 12, 2);
    $t->char('currency', 3);
    $t->integer('session_count');
    $t->integer('validity_days')->default(180);
    $t->json('includes')->nullable();                        // [{service_id, count}]
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});

Schema::create('patient_packages', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('package_id')->index();
    $t->ulid('invoice_id')->nullable();                     // purchase invoice
    $t->integer('sessions_total');
    $t->integer('sessions_used')->default(0);
    $t->date('valid_from');
    $t->date('valid_until');
    $t->string('status', 16)->default('active');            // 'active' | 'expired' | 'exhausted' | 'cancelled'
    $t->timestamps();
});
```

`PackageRedemptionService::redeem($patientPackage, $appointment)` decrements `sessions_used`. When booking via package, the invoice is zero (or the patient pays only the gap if the appointment is in a higher-priced service).

### 21.3 Insurance pre-authorization — `insurance-preauth` (V2, MML 3)

**1. Overview.** V1's billing is cash-pay (patient pays the clinic; clinic may file the claim on the patient's behalf via a paper form). V2 introduces **first-class payer integration**: a pre-authorization workflow that submits to the payer's API (or generates an EDI 837 for US, or NHCX for India), tracks the response, calculates patient responsibility, and feeds the resulting claim into the revenue cycle. MML 3 GA covers: payer registry, policy capture, preauth submission + status, claim creation, denial capture, and a denial-management dashboard. MML 4 (V2.1) adds ERA (835) auto-posting; MML 5 (V3) adds appeals and underpayment detection.

**2. Domain Model.**
```php
Schema::create('insurance_payers', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 128);
    $t->string('payer_code', 32)->unique();                 // 'vipul-mediclaim', 'change-hc-claim-md'
    $t->string('country', 2);
    $t->string('api_endpoint')->nullable();
    $t->json('api_credentials')->nullable();                // encrypted; AWS Secrets Manager reference
    $t->string('api_protocol', 32);                         // 'rest' | 'edi837' | 'nhcx' | 'edi278' | 'manual'
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});

Schema::create('insurance_policies', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->ulid('payer_id')->index();
    $t->string('policy_number', 64);
    $t->string('group_number', 64)->nullable();
    $t->string('subscriber_name', 128);
    $t->string('subscriber_relation', 16);                  // 'self' | 'spouse' | 'child' | 'parent' | 'other'
    $t->date('valid_from');
    $t->date('valid_until')->index();
    $t->decimal('coverage_pct', 5, 2);                      // 80.00 = 80%
    $t->decimal('deductible', 12, 2)->default(0);
    $t->decimal('max_coverage', 12, 2)->nullable();
    $t->string('status', 16)->default('active');
    $t->string('card_image_s3_key', 255)->nullable();       // scanned insurance card
    $t->timestamps();
});

Schema::create('insurance_preauth_requests', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('policy_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('doctor_id')->index();
    $t->ulid('appointment_id')->nullable();
    $t->string('external_request_id', 64)->nullable();
    $t->string('status', 16)->default('draft');             // draft | submitted | in_review | approved | denied | partial | expired | cancelled
    $t->decimal('requested_amount', 12, 2);
    $t->decimal('approved_amount', 12, 2)->nullable();
    $t->decimal('patient_responsibility', 12, 2)->nullable();
    $t->text('denial_reason')->nullable();
    $t->json('clinical_justification');                     // diagnosis codes, prior treatments, etc.
    $t->json('attachments')->nullable();                    // [{filename, s3_key, category: 'lab'|'imaging'|'progress_note'}]
    $t->json('procedure_codes');                            // CPT/ICD/IRDA codes
    $t->string('urgency', 16)->default('elective');         // 'emergency' | 'urgent' | 'elective'
    $t->integer('expected_response_hours')->nullable();
    $t->timestamp('submitted_at')->nullable();
    $t->timestamp('responded_at')->nullable();
    $t->timestamps();
});

Schema::create('insurance_claims', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('preauth_id')->nullable()->index();
    $t->ulid('policy_id')->index();
    $t->ulid('invoice_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('doctor_id')->index();
    $t->string('claim_number', 64)->unique();
    $t->string('status', 16)->default('draft');             // draft | submitted | acknowledged | in_review | paid | denied | appealed
    $t->decimal('claim_amount', 12, 2);
    $t->decimal('approved_amount', 12, 2)->nullable();
    $t->decimal('paid_amount', 12, 2)->default(0);
    $t->decimal('patient_responsibility', 12, 2)->default(0);
    $t->text('denial_reason')->nullable();
    $t->json('cpt_codes')->nullable();
    $t->json('icd10_codes')->nullable();
    $t->string('edi837_s3_key', 255)->nullable();
    $t->string('payer_claim_id', 64)->nullable();
    $t->timestamp('submitted_at')->nullable();
    $t->timestamp('paid_at')->nullable();
    $t->timestamps();
});

Schema::create('insurance_eligibility_checks', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('policy_id')->index();
    $t->json('request_payload');
    $t->json('response_payload');
    $t->boolean('is_active')->default(false);
    $t->decimal('copay_amount', 12, 2)->nullable();
    $t->decimal('deductible_remaining', 12, 2)->nullable();
    $t->json('coverage_details')->nullable();
    $t->integer('response_ms');
    $t->timestamp('checked_at')->index();
});
```

**3. End-to-end flow.**
1. Patient has an `insurance_policies` row (added at registration or by staff). Card image uploaded; front+back OCR'd (Tesseract + a custom NN); fields auto-fill.
2. Front desk or patient self-checks **eligibility** at booking time: `POST /api/v2/insurance/eligibility-check` → 270/271 EDI (US) or payer REST → returns `is_active`, `copay`, `deductible_remaining`, `coverage_details`. Cached for 24h.
3. Doctor orders a procedure (e.g., MRI knee). If the procedure is in the tenant's `requires_preauth` list, the system prompts to open a preauth.
4. **Preauth submission:** staff or patient fills the form (auto-populated from policy + appointment + clinical notes). System submits to payer via:
   - REST API (modern payers, India NHCX): `POST {payer.api_endpoint}/preauths` with the JSON payload.
   - EDI 278 (US, older payers): `GenerateEdi278Job` produces an X12 278 file; uploaded to the clearinghouse (e.g., Change Healthcare, Availity, Waystar).
   - NHCX (India, IRDA-regulated): `POST {nhcx_endpoint}/v1/preauth` with the IRDA-mandated JSON.
   - Manual (no API): the system produces a printable form; staff faxes or uploads it; status updated manually.
5. **Status updates:** payer responds (5 min for REST/NHCX, 1–5 days for EDI 278). The response updates the row: `status='approved' | 'denied' | 'partial'`, `approved_amount`, `patient_responsibility`. A webhook or daily poll fetches late responses.
6. **Service delivery:** on appointment completion, the system auto-creates a draft `insurance_claims` row from the preauth + invoice + CPT/ICD codes.
7. **Claim submission:** the system submits the claim (EDI 837, NHCX, or REST). The payer acknowledges (277CA in US) and assigns a `payer_claim_id`.
8. **Payment posting:** payer pays via EFT or virtual card. An 835 ERA file (US) or a webhook (REST) reconciles the payment; `transactions` row written; invoice partially paid; `patient_responsibility` posted to the patient ledger.
9. **Denial management:** if denied, a `denials` row (per §21.4) is auto-created; the denial code is parsed (CARC/RARC); the system suggests the next action (resubmit with corrected code, appeal with letter, write off).

**4. API Surface.**
```
POST   /api/v2/insurance/payers                              # admin registers payer
GET    /api/v2/insurance/payers
POST   /api/v2/insurance/policies                            # capture policy
GET    /api/v2/insurance/policies?patient_id=...
POST   /api/v2/insurance/eligibility-check                   # 270
GET    /api/v2/insurance/eligibility-check/{id}
POST   /api/v2/insurance/preauth                              # create + submit
PATCH  /api/v2/insurance/preauth/{id}                        # edit while draft
POST   /api/v2/insurance/preauth/{id}/submit
GET    /api/v2/insurance/preauth/{id}
POST   /api/v2/insurance/preauth/{id}/cancel
POST   /api/v2/insurance/claims                              # create from preauth + invoice
POST   /api/v2/insurance/claims/{id}/submit
POST   /api/v2/insurance/claims/{id}/appeal
GET    /api/v2/insurance/claims/{id}/status
POST   /api/v2/insurance/era                                  # webhook for 835
GET    /api/v2/insurance/dashboard                            # KPIs: denial rate, days in AR, collection rate
```

**5. Frontend.** `/admin/insurance/` has five tabs: **Payers** (admin-managed), **Policies** (per-patient), **Preauths** (Kanban: draft → submitted → in_review → approved/denied), **Claims** (table with filter by status/payer), **Dashboard** (denial rate, A/R aging, top denial reasons). The preauth form is a single page with auto-fill, attachment upload, and a "Submit" button that opens a confirmation modal showing the request payload.

**6. Realtime.**
- `insurance.eligibility.completed.{tenant}` — eligibility check returns.
- `insurance.preauth.{submitted|approved|denied|partial}.{tenant}.{preauth_id}` — patient + staff get push.
- `insurance.claim.{submitted|paid|denied}.{tenant}.{claim_id}` — financial dashboard update.
- `insurance.era.received.{tenant}.{payer_id}` — 835 file parsed; payments auto-posted.

**7. Security & RBAC.** Roles: `front_desk` (capture policy, run eligibility), `billing_staff` (create preauth + claim), `biller_manager` (approve high-value preauths, appeal denials), `tenant_admin` (manage payers, view all), `compliance_officer` (audit). Payer API credentials live in AWS Secrets Manager; the API never returns the raw credential. All payer API calls are logged (request/response payload, latency, HTTP status) in `insurance_eligibility_checks` and an equivalent for preauth + claim submissions. PHI minimization: name + DOB + member ID are sent; full chart is not. Per `20-Security §11.4`, an attestation is required before a high-value (>tenant-defined threshold) preauth is submitted (co-sign by biller_manager).

**8. Acceptance Criteria.**
- [ ] An eligibility check returns within the payer's SLA (REST: 5s, EDI: 60s, NHCX: 10s); failures are retried with exponential backoff.
- [ ] A preauth that is `partial` shows the approved amount + patient responsibility; the patient is notified of the out-of-pocket before service.
- [ ] A claim's CPT/ICD codes are auto-derived from the encounter + lab/imaging orders; manual override is allowed but audit-logged.
- [ ] A denial is auto-categorized (eligibility / authorization / coding / documentation / timely filing) by the CARC/RARC code.
- [ ] An 835 ERA auto-posts payments: matches the claim, creates a `transactions` row, updates the invoice.
- [ ] Submission latency for an NHCX preauth: p99 < 5s.
- [ ] Two tenants' claims are not cross-visible; the `tenant_id` scope is enforced on every query.

**9. Real-World Test Scenarios.**
- *Eligibility for a deleted policy:* patient cancels insurance; eligibility check returns `is_active=false`; booking flow shows "Insurance inactive; you'll pay out-of-pocket".
- *Preauth partially approved:* preauth for ₹1,00,000 MRI is approved at ₹80,000; patient responsibility ₹20,000 is shown; patient consents in writing; service delivered; claim is for ₹1,00,000; payer pays ₹80,000; patient pays ₹20,000.
- *Denial, then appeal:* claim denied for "Missing progress note"; the system prompts to attach the note and resubmit; the appeal is auto-drafted; the biller reviews and submits; status flips to `appealed`; eventually `paid`.
- *NHCX outage:* NHCX is down; the preauth is queued with exponential backoff (1m, 5m, 30m, 2h); the UI shows "Pending payer connectivity"; a daily digest email to the biller.
- *Clearinghouse mismatch:* EDI 837 is malformed; the clearinghouse rejects with a syntax error; the system parses the error, shows the line+segment, and re-generates a corrected file.
- *Out-of-network:* payer returns "Provider not in network"; the system marks the policy `status='oon'`; the patient is informed; the claim is filed as OON with a higher patient responsibility.

**10. What this module is NOT:**
- Not a clearinghouse. We are a client of clearinghouses (US) or NHCX (India); we do not replace them.
- Not an adjudication engine. We do not decide coverage; the payer does.
- Not free. Payer API integration has a one-time setup fee per payer (paid by the tenant); the integration requires payer certification.

**11. Dev Workflow.** `php artisan insurance:eligibility-sync` replays a queued eligibility check. `php artisan insurance:era-import --file=...` parses an 835 file manually. `php artisan insurance:denial-categorize --code=CARC-16` shows the categorization for a CARC code. Code: `app-modules/insurance-preauth/`; tests: `app-modules/insurance-preauth/tests/Feature/{Policy,Preauth,Claim,Era}Test.php` and `tests/Integration/PayerSandboxTest.php` (uses the payer's sandbox).

### 21.4 Revenue cycle management — `revenue-cycle-mgmt` (V2.1, MML 3)

**1. Overview.** V2 covers the happy-path of a claim: preauth → claim → payment. V2.1 covers the unhappy paths: **denials**, **underpayments**, **A/R aging**, **dunning**, **payment plans**, **E&O (Explanation of Benefits) reconciliation**, and a worklist dashboard that tells the biller exactly what to do today. MML 3 GA: denial auto-categorization, appeal letter generation, dunning state machine, payment plan auto-charge, credit notes, and a worklist with due-date priorities.

**2. Domain Model.** Per the existing schema (denials, dunning_letters, credit_notes, payment_plans, payment_plan_installments), expanded with:
```php
Schema::create('denial_appeals', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('denial_id')->index();
    $t->ulid('claim_id')->index();
    $t->text('appeal_letter');
    $t->json('attachments')->nullable();
    $t->ulid('submitted_by_user_id')->index();
    $t->timestamp('submitted_at')->nullable();
    $t->string('status', 16)->default('draft');             // draft | submitted | acknowledged | overturned | upheld
    $t->text('payer_response')->nullable();
    $t->timestamp('responded_at')->nullable();
    $t->timestamps();
});

Schema::create('dunning_policies', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 64);
    $t->json('steps');                                        // [{days_after_due: 1, channel: 'email', template: 'friendly'}, {days: 7, channel: 'sms', ...}, {days: 30, channel: 'phone'}]
    $t->integer('max_attempts')->default(3);
    $t->string('final_action', 32)->default('write_off');   // 'write_off' | 'collections_agency' | 'legal'
    $t->boolean('is_default')->default(false);
    $t->timestamps();
});

Schema::create('collections_agency_assignments', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('invoice_id')->index();
    $t->string('agency_name', 128);
    $t->string('agency_payout_pct', 5, 2);                    // 25.00 = agency keeps 25% of collected
    $t->timestamp('assigned_at');
    $t->timestamp('recovered_at')->nullable();
    $t->decimal('recovered_amount', 12, 2)->nullable();
    $t->timestamps();
});
```

**3. Denial worklist (`ClaimFollowupService::autoCreate`).** Every denial auto-creates a worklist row with a `due_date` based on the denial type:
| Type | Default due | Reason |
|---|---|---|
| `denial` | 15 days | payer appeal window |
| `underpayment` | 30 days | investigate contract variance |
| `no_response` | 45 days | follow up with payer |
| `request_info` | 10 days | respond to payer's request |
| `appeal` | 21 days | appeal submission deadline |

The worklist is the biller's home page; it sorts by `due_date ASC` and groups by `assignee_user_id` (default = unassigned; biller_manager can re-assign).

**4. Appeal letter generation.** When a biller opens a denial of type `denial`, the system auto-drafts an appeal letter using: denial code, denial reason, claim clinical justification, attached progress notes, and the tenant's letterhead. The biller reviews, edits, and submits (electronically via payer portal API, or printed for fax/mail). The submission creates a `denial_appeals` row; the payer response is captured.

**5. Dunning state machine.** `DunningService::run()` is a daily cron:
1. Find all overdue invoices (`due_date < now() - grace_days`) not in `dunning_hold`.
2. For each, apply the tenant's `dunning_policies` step that matches `days_after_due`.
3. Generate a `dunning_letters` row + send via the channel (email/SMS/WhatsApp/letter per `11-Notifications`).
4. After `max_attempts`, set `final_action`: write off the invoice (mark `invoices.status='written_off'`) or assign to a `collections_agency_assignments` row.
5. A patient can pause dunning with a `dunning_hold` (per request, e.g., "I'm in financial hardship; please call me"); the hold is logged in `audit` with `action='dunning.hold'`.

**6. Payment plans.** `PaymentPlanService::setup($invoice, $installments)` creates N installments with due dates. Each installment auto-charges via the patient's saved payment method (Razorpay Subscriptions / Stripe Subscriptions / Paystack Subscriptions, per `10-Payments` V1.2). A failed installment retries 3× (1d, 3d, 7d); after 3 failures, the plan is flagged `defaulted` and the remaining balance is reverted to a single overdue invoice (which then enters dunning).

**7. Underpayment detection.** When an 835 ERA posts a payment that is < 90% of the expected amount (configurable per payer contract), the system auto-creates a `claim_followups` row of type `underpayment`. The biller investigates the payer's contracted rate vs. the paid amount; if underpaid, the system drafts a payer dispute letter.

**8. API Surface (V2.1 additions).**
```
GET    /api/v2.1/rcm/worklist?assignee=me&due_within=7d
POST   /api/v2.1/rcm/worklist/{id}/assign
POST   /api/v2.1/rcm/worklist/{id}/complete
POST   /api/v2.1/rcm/denials/{id}/appeal-draft
POST   /api/v2.1/rcm/denials/{id}/appeal-submit
GET    /api/v2.1/rcm/dunning-policy
PUT    /api/v2.1/rcm/dunning-policy
POST   /api/v2.1/rcm/dunning/preview?invoice_id=...
POST   /api/v2.1/rcm/dunning/run                            # manual cron trigger
POST   /api/v2.1/rcm/dunning/hold                            # { invoice_id, reason }
POST   /api/v2.1/rcm/payment-plan                            # { invoice_id, installments }
GET    /api/v2.1/rcm/ar-aging?bucket=0-30,31-60,61-90,90+
GET    /api/v2.1/rcm/underpayments?payer=...
POST   /api/v2.1/rcm/collections-assign                      # { invoice_id, agency_name, payout_pct }
GET    /api/v2.1/rcm/dashboard
```

**9. Frontend.** `/admin/rcm/` is the biller's home. Four tabs: **Worklist** (Kanban: open → in_progress → awaiting_payer → resolved), **Denials** (table with denial-code filter and "Draft appeal" button), **A/R Aging** (bucketed bar chart with drill-down), **Dunning** (policy editor + preview). The dashboard shows: net collection rate, days in A/R, denial rate by reason, top 5 unpaid claims, and the biller's personal worklist count.

**10. Realtime.**
- `rcm.worklist.item.{created|assigned|completed}.{tenant}.{user_id}` — biller gets a toast.
- `rcm.denial.{received|categorized}.{tenant}.{claim_id}` — finance dashboard.
- `rcm.dunning.{letter_sent|escalated}.{tenant}.{invoice_id}` — audit log; customer-facing notification if the channel is email/SMS.
- `rcm.era.{posted|underpayment_flagged}.{tenant}.{claim_id}` — auto-posting done; underpayment flagged.

**11. Security & RBAC.** Roles: `biller` (worklist, draft appeals, payment plans), `biller_manager` (assign, override, write-off, collections), `tenant_admin` (policy, view all), `compliance_officer` (audit, read-only). Per `20-Security §11.4`, a write-off above a tenant-defined threshold requires co-sign by `biller_manager`. All PII in dunning letters is per-tenant configurable (e.g., include MRN or abha_id). The collections-agency export is a CSV with patient_id + amount + agency; the file is S3 SSE-KMS, signed URL expires in 24h, and the export is audit-logged with `action='collections.export'`.

**12. Acceptance Criteria.**
- [ ] Every denial creates a worklist row with the correct `due_date` for its type.
- [ ] An appeal letter draft is generated in < 5s and includes denial code, clinical justification, and progress notes.
- [ ] Dunning steps fire on the schedule defined in the policy; max attempts enforced; final action (write-off or collections) applied.
- [ ] A payment plan auto-charges the saved method on the due date; failed charge retries 3× then defaults.
- [ ] Underpayment detection flags any ERA < 90% of expected; biller is notified.
- [ ] A/R aging buckets match the standard 0-30, 31-60, 61-90, 90+; dashboard p95 < 500ms.
- [ ] Dunning hold pauses all further letters; resuming the hold re-applies the policy from the current date.

**13. Real-World Test Scenarios.**
- *Denial cascade:* 30 denials in one morning (a payer policy change); worklist loads 30 items; biller triages 8 of them in 2 hours; 3 are auto-resolved (corrected CPT resubmission); 5 require appeals.
- *Patient hardship:* patient calls "I lost my job"; biller places a `dunning_hold` with reason "Financial hardship, awaiting charity screening"; no more letters sent; the hold expires after 90 days unless renewed.
- *Write-off co-sign:* $25,000 invoice is being written off; the biller requests; the manager approves; both sign; the audit row records both user IDs; the invoice `status='written_off'`.
- *Collections agency:* 4 attempts failed; system creates a `collections_agency_assignments` row; agency gets an export; 60 days later, agency recovers 40%; `recovered_amount` is recorded; the agency keeps 25% per the contract.
- *Underpayment:* contracted rate for CPT 99214 is $120; payer paid $85; underpayment auto-flagged; biller disputes with the contract; payer pays the delta; the worklist item is closed.
- *Dunning policy preview:* biller edits a policy (e.g., changes grace from 7 to 14 days); clicks "Preview"; the system simulates the next 90 days of letters; the biller confirms; the policy is updated.

**14. What this module is NOT:**
- Not a general accounting system (no GL, no AP, no balance sheet). That is V3+ `general-ledger`.
- Not a payer adjudication engine. We post payments; we do not decide coverage.
- Not a credit bureau. We do not report patients to credit bureaus (V2.1); that is a future V3+ decision.

**15. Dev Workflow.** `php artisan rcm:simulate --denials=30` synthetic denial load. `php artisan rcm:dunning-run --tenant=...` manual cron. `php artisan rcm:appeal-draft --denial=...` drafts an appeal letter. `php artisan rcm:ar-aging-report --month=2026-05` CSV export. Code: `app-modules/revenue-cycle-mgmt/`; tests: `app-modules/revenue-cycle-mgmt/tests/Feature/{Denial,Dunning,PaymentPlan,Underpayment}Test.php` and `tests/Performance/ar-aging-dashboard.k6.js`.

### 21.5 Chain revenue split (`multi-clinic-revenue-share`, V2)

```php
Schema::create('chain_revenue_rules', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('name', 64);
    $t->json('rules');                                       // [{scope: 'department', match: 'cardiology', branch_pct: 70, hq_pct: 30}, ...]
    $t->boolean('is_default')->default(false);
    $t->timestamps();
});

Schema::create('revenue_distributions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('invoice_id')->index();
    $t->ulid('branch_id')->index();
    $t->decimal('branch_amount', 12, 2);
    $t->decimal('hq_amount', 12, 2);
    $t->char('currency', 3);
    $t->timestamp('distributed_at');
    $t->timestamps();
});
```

`ChainRevenueService::distribute($invoice)` runs nightly, splits per the matching rule, credits each branch's revenue account. HQ gets the rest. Each branch sees their share in their dashboard.

### 21.6 Usage-based pricing (V2)

For AI features charged per token:

```php
Schema::create('usage_records', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('user_id')->index();
    $t->string('feature', 32);                              // 'ai-symptom-checker' | 'ai-scribe' | 'ai-doctor-rec'
    $t->string('unit', 16);                                 // 'token' | 'request' | 'minute' | 'page'
    $t->decimal('quantity', 12, 4);
    $t->decimal('cost', 12, 4);                             // in USD
    $t->json('metadata')->nullable();                        // model used, prompt id, etc.
    $t->timestamp('recorded_at')->index();
});
```

Monthly: `BillingService::aggregateUsage($tenantId, $month)` rolls up usage into a usage invoice line.

### 21.7 E-invoicing (V2)

India: GSTN IRN (Invoice Registration Number) via NIC API. EU: Peppol. Each e-invoice is signed XML stored in S3 and referenced by `e_invoices`:

```php
Schema::create('e_invoices', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('invoice_id')->unique();
    $t->string('irn', 64)->unique();                       // GSTN IRN
    $t->string('signed_qr_s3_key');                        // signed QR code image
    $t->string('signed_xml_s3_key');                       // signed XML
    $t->timestamp('generated_at');
    $t->string('status', 16);                              // 'active' | 'cancelled'
    $t->timestamps();
});
```

### 21.8 V2 API additions

```
GET    /api/v2/subscriptions/plans                         -> [plan]
POST   /api/v2/subscriptions                              -> { plan_id, payment_method_id } -> subscription
GET    /api/v2/subscriptions/me                           -> subscription
POST   /api/v2/subscriptions/{id}/cancel                  -> cancelled
POST   /api/v2/subscriptions/{id}/pause                   -> paused
POST   /api/v2/subscriptions/{id}/resume                  -> active
GET    /api/v2/subscription-invoices                      -> [sub_invoice]
GET    /api/v2/packages                                    -> [package]
POST   /api/v2/patient-packages                           -> { package_id } -> patient_package
GET    /api/v2/patient-packages                           -> [patient_package]
POST   /api/v2/insurance/payers                            -> payer
POST   /api/v2/insurance/policies                          -> policy
POST   /api/v2/insurance/preauth                           -> preauth
GET    /api/v2/insurance/preauth/{id}                      -> preauth
POST   /api/v2/insurance/claims                            -> claim
POST   /api/v2/insurance/claims/{id}/submit                -> submitted
POST   /api/v2/insurance/claims/{id}/appeal                -> { letter }
GET    /api/v2/insurance/denials                           -> [denial]
POST   /api/v2/invoices/{id}/payment-plan                  -> { installments } -> plan
POST   /api/v2/invoices/{id}/dunning-test                  -> preview dunning schedule
POST   /api/v2/invoices/{id}/credit-note                   -> credit_note
GET    /api/v2/invoices/{id}/e-invoice                     -> e_invoice (signed QR + XML)
```

### 21.9 V2 acceptance criteria

- [ ] Subscription renews via Razorpay / Stripe / Paystack Subscriptions API.
- [ ] Dunning: 3 attempts (day 0, 3, 7), then service degrades.
- [ ] Insurance preauth: 90% of tests approved in sandbox.
- [ ] Insurance claim submitted via EDI 837 (US) and NHCX (India) successfully.
- [ ] Denial appeal letter auto-generated from denial code description.
- [ ] Chain revenue split credits correct branch in nightly job.
- [ ] Usage invoice rolls up tokens used in a month correctly.
- [ ] E-invoice IRN generated via GSTN sandbox.
- [ ] All V2 financial mutations are audited and broadcast.

---

## 22. V3+ Modules — From Invoicing to Full RCM Platform

> V1 bills per appointment. V2 adds subscriptions, insurance, RCM basics, e-invoicing. V3+ turns the billing module into a **complete revenue cycle management (RCM) platform** — every claim, every code, every payer rule, every dollar accounted for. The 111-module platform becomes ~140 modules by V5 with billing alone adding ~30 new modules.

### 22.1 RCM Worklist & A/R Aging (`rcm-worklist`, V3)

Turn the billing module from "post payment" into "actively collect revenue" with a denial-management and A/R-follow-up worklist.

**Tables:**

```php
Schema::create('ar_aging_buckets', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->date('snapshot_date')->index();
    $t->string('bucket', 16)->index();   // '0-30' | '31-60' | '61-90' | '91-120' | '120+'
    $t->string('payer_type', 16);        // 'insurance' | 'patient' | 'self_pay'
    $t->ulid('invoice_id')->index();
    $t->decimal('amount', 12, 2);
    $t->timestamps();
    $t->unique(['tenant_id', 'snapshot_date', 'invoice_id', 'bucket']);
});

Schema::create('claim_followups', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('claim_id')->index();
    $t->ulid('assigned_user_id')->nullable()->index();
    $t->string('type', 32);              // 'denial' | 'underpayment' | 'no_response' | 'request_info' | 'appeal'
    $t->string('priority', 16)->default('medium');
    $t->string('status', 16)->default('open');
    $t->text('reason');
    $t->text('action_taken')->nullable();
    $t->date('due_date')->index();
    $t->timestamp('resolved_at')->nullable();
    $t->ulid('resolved_by_user_id')->nullable();
    $t->json('activity_log')->nullable();
    $t->timestamps();
});

Schema::create('collections_agency_assignments', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('agency_name', 128);
    $t->string('agency_code', 32);
    $t->json('agency_credentials')->nullable();
    $t->ulid('invoice_id')->index();
    $t->ulid('patient_id')->index();
    $t->decimal('principal_amount', 12, 2);
    $t->decimal('agency_fee_pct', 5, 2);
    $t->string('status', 16)->default('assigned');
    $t->timestamp('assigned_at');
    $t->timestamp('recovered_at')->nullable();
    $t->decimal('recovered_amount', 12, 2)->nullable();
    $t->timestamps();
});
```

**Aging buckets** are computed nightly by `ArAgingSnapshotJob` (cron 02:00 UTC). The dashboard shows the A/R pyramid (0-30d, 31-60d, 61-90d, 91-120d, 120+ days) with total dollars and claim count. Industry target: <15% of A/R in 90+ days.

**`ClaimFollowupService::autoCreate($claim, $type, $reason)`** runs after every denial/underpayment event. Each followup has a `due_date` (denial=15 days, underpayment=30 days, no_response=45 days, request_info=10 days, appeal=21 days).

**SLA escalation rules:** 2 days overdue → email assignee; 5 days → email manager; 7 days → mark `escalated`, assign to senior biller; 14 days → eligible for `WriteOffService` or `CollectionsAgencyService`.

**`DaysInArService`** computes the industry-standard KPI: `(total A/R) / (average daily charges in last 90 days)`. Target: <40 days.

### 22.2 Real-Time Eligibility (270/271) (`realtime-eligibility`, V3)

Move from "save card on file" to "verify benefits in real-time before the visit."

**Tables:**

```php
Schema::create('eligibility_requests', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('policy_id')->index();
    $t->ulid('appointment_id')->nullable()->index();
    $t->string('gateway', 32);
    $t->string('external_request_id', 64)->nullable();
    $t->string('status', 16);
    $t->json('request_payload');
    $t->json('response_payload')->nullable();
    $t->json('benefits')->nullable();
    $t->integer('response_time_ms')->nullable();
    $t->timestamp('submitted_at');
    $t->timestamp('responded_at')->nullable();
    $t->timestamps();
});
```

**`EligibilityService::check($patient, $policy, $serviceCodes, $dateOfService)`**:
1. Sends an X12 270 EDI message (or JSON for modern APIs like Availity).
2. Receives X12 271 response.
3. Parses into structured `benefits` JSON.
4. Persists to `eligibility_requests`.
5. Returns `PatientCostEstimate` (see 22.6).

**Cache:** `eligibility:{policy_id}:{dos}` is cached for 24h. Subsequent calls hit cache.

**Real-time triggers:** Patient books appointment → automatic check. Patient checks in → second check. Doctor orders procedure → re-check for that CPT code's specific benefit.

### 22.3 Electronic Claim Submission (837P/837I/837D) (`claim-submission`, V3)

End-to-end 837 EDI generation, submission, and tracking.

**Tables:**

```php
Schema::create('claim_submissions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('claim_id')->index();
    $t->ulid('clearinghouse_id')->nullable()->index();
    $t->string('gateway', 32);
    $t->string('external_batch_id', 64)->nullable();
    $t->string('external_claim_id', 64)->nullable();
    $t->string('status', 16);
    $t->text('rejection_reason')->nullable();
    $t->string('edi_payload_s3_key')->nullable();
    $t->string('acknowledgment_s3_key')->nullable();
    $t->integer('edi_byte_count')->nullable();
    $t->timestamp('submitted_at');
    $t->timestamp('acknowledged_at')->nullable();
    $t->integer('attempt_count')->default(1);
    $t->timestamps();
});

Schema::create('claim_acknowledgments', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('submission_id')->index();
    $t->string('ack_type', 16);             // '999' | 'TA1' | '277CA'
    $t->string('status', 16);
    $t->json('errors')->nullable();
    $t->string('raw_payload_s3_key');
    $t->timestamp('received_at');
    $t->timestamps();
});
```

**`ClaimSubmissionService::submit($claim)`**:
1. **Scrubbing**: Run `ClaimScrubbingService` — only clean claims get submitted.
2. **837 generation**: Use `EdiBuilder` to construct ISA/GS/ST envelope with tenant's payer ID, tax ID, NPI, taxonomy.
3. **Submit** via clearinghouse REST API (Availity, Change Healthcare, Waystar, Office Ally).
4. **Receive 999** (syntax ack) within 30s; parse and store.
5. **Receive 277CA** (claim status) within 24h; if rejected, fix and resubmit.
6. **Track** in `claim_submissions`; broadcast `ClaimSubmitted` event.

**Resubmission tracking:** if a 277CA reports rejection with a fixable error, we fix the claim, set `attempt_count += 1`, and resubmit. Max 3 attempts before manual review.

### 22.4 Claim Scrubbing Engine (`claim-scrubbing`, V3)

The #1 cause of denials is **avoidable** data errors. The scrubber catches them before submission.

**Top rules (in `config/billing/scrubber_rules.php`):**

| Rule ID | Check | Action |
|---|---|---|
| `R-001` | NPI is 10 digits, valid Luhn check | Block |
| `R-002` | EIN is 9 digits, valid IRS prefix | Block |
| `R-005` | CPT in `cpt_codes` table, valid for DOS | Block |
| `R-006` | ICD-10 in `icd10_codes` table, valid for DOS | Block |
| `R-007` | Each CPT has at least one supporting ICD | Block |
| `R-008` | Modifiers valid for CPT (e.g., 25 only with E/M) | Block |
| `R-009` | CCI edits (procedure-to-procedure) | Warn / add modifier 59 |
| `R-010` | MUE exceeded (max units per CPT per day) | Block |
| `R-011` | Procedure covered by LCD/NCD | Block |
| `R-015` | Procedure requires preauth, preauth is approved | Block |
| `R-016` | Timely filing limit not passed (typically 90-180 days) | Block |
| `R-021` | Telehealth modifier 95 or GT present | Warn |
| `R-023` | Billing/rendering provider enrolled with payer (PECOS) | Block |
| `R-025` | Rendering provider not on OIG/SAM exclusion list | Block |
| `R-028` | Bundled procedure (NCCI column 2) | Block or add modifier 59 |
| `R-030` | Procedure in global period of prior procedure (90-day global) | Block |

(30 rules total; see test matrix 23.1 for the full list.)

**`ClaimScrubbingService::scrub($claim)`** returns:
```php
[
    'is_clean' => false,
    'errors' => [
        ['rule_id' => 'R-007', 'severity' => 'block', 'message' => 'CPT 99213 not linked to any ICD-10'],
    ],
    'warnings' => [
        ['rule_id' => 'R-021', 'severity' => 'warn', 'message' => 'Telehealth visit missing modifier 95'],
    ],
]
```

Blocked claims cannot be submitted. Warned claims can be submitted with a 4-eyes confirmation (manager override).

**Rule updates:** new rules are pushed via `BillingRuleSyncJob` (cron weekly) from the central rule database. Rules are versioned; rollback is one click.

### 22.5 Electronic Remittance Advice (ERA/EOB) — 835 (`era-posting`, V3)

Auto-post payments from payer 835 files.

**Tables:**

```php
Schema::create('era_files', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('payer_name', 128);
    $t->string('payer_id', 32);
    $t->string('trace_number', 32)->unique();
    $t->date('payment_date');
    $t->decimal('total_paid', 12, 2);
    $t->decimal('total_adjustment', 12, 2);
    $t->decimal('total_patient_responsibility', 12, 2);
    $t->integer('claim_count');
    $t->string('s3_key');
    $t->string('status', 16)->default('received');
    $t->text('parse_error')->nullable();
    $t->timestamp('received_at');
    $t->timestamp('posted_at')->nullable();
    $t->timestamps();
});

Schema::create('era_claims', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('era_file_id')->index();
    $t->ulid('claim_id')->index();
    $t->string('claim_status_code', 8);
    $t->decimal('billed_amount', 12, 2);
    $t->decimal('allowed_amount', 12, 2);
    $t->decimal('paid_amount', 12, 2);
    $t->decimal('patient_responsibility', 12, 2);
    $t->json('adjustments')->nullable();
    $t->json('service_lines')->nullable();
    $t->timestamps();
});
```

**CARC/RARC codes** (the heart of ERA):
- **Group Codes**: CO (Contractual Obligation), PR (Patient Responsibility), OA (Other Adjustments), PI (Payer Initiated), CR (Correction and Reversal)
- **CARC**: 1 (Deductible), 2 (Coinsurance), 3 (Copay), 16 (Claim/service lacks information), 29 (Time limit for filing expired), 50 (Non-covered service), 96 (Non-covered charge), 97 (Bundled), 109 (Not covered by this payer), 197 (Precertification/authorization absent)
- **RARC**: N30, N290, M127, etc.

**`EraPostingService::post($eraFile)`**:
1. **Parse 835** with `Edi835Parser` → `era_claims`.
2. **Match to internal claims** by patient + date of service + billed amount.
3. **Auto-post**:
   - `paid_amount` → `transactions` (succeeded) + invoice `amount_paid`
   - `adjustment[CO]` → write-off line on invoice (contractual write-off)
   - `adjustment[PR]` → reclassify to patient responsibility
4. **Flag denied claims** (status_code 3/4) → create `claim_followups`.
5. **Flag underpayments** (paid < expected by > 10%) → create `claim_followups`.
6. **Mark era_file as posted**, generate PDF receipt.

### 22.6 Patient Cost Estimation (`patient-cost-estimate`, V3)

Show the patient an accurate cost estimate **before** the visit. Heart of US No Surprises Act compliance.

**`CostEstimationService::estimate($patient, $serviceCodes, $dateOfService)`**:
1. Run real-time eligibility (270/271) to get deductible, copay, coinsurance, OOP max, met-to-date.
2. Look up the contracted allowed amount for each CPT code.
3. Compute:
   - `deductible_remaining` = `deductible` - `deductible_met`
   - `oop_remaining` = `oop_max` - `oop_met`
   - `patient_cost_per_cpt` = `min(allowed, deductible_remaining) + (allowed - deductible_applied) * coinsurance + copay_if_applicable`
   - But never exceed `oop_remaining`
4. Cross-check with the chargemaster for non-contracted payers.
5. Return structured estimate with totals and disclaimers.

**Display rules:** Show in booking flow. Show on patient portal before visit. Show at check-in with "I agree to pay" e-signature. Generate a **Good Faith Estimate (GFE)** PDF if required (uninsured / self-pay, certain visits).

### 22.7 Good Faith Estimate (GFE) & Advanced EOB (`gfe-aeob`, V3)

**GFE** is a US federally-mandated (No Surprises Act, effective 1/1/2022) estimate for uninsured/self-pay patients listing all expected charges for a scheduled visit.

**Tables:**

```php
Schema::create('good_faith_estimates', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('appointment_id')->nullable()->index();
    $t->string('estimate_number', 32)->unique();
    $t->date('estimate_date');
    $t->date('service_date');
    $t->json('provider_ids')->nullable();
    $t->json('service_lines');
    $t->decimal('total_estimate', 12, 2);
    $t->string('status', 16)->default('draft');
    $t->text('dispute_reason')->nullable();
    $t->date('valid_until');
    $t->string('pdf_s3_key')->nullable();
    $t->timestamp('issued_at')->nullable();
    $t->timestamp('patient_signed_at')->nullable();
    $t->string('patient_signature_s3_key')->nullable();
    $t->timestamps();
});
```

**`GoodFaithEstimateService::generate($patient, $appointment, $services)`**:
1. List all expected services (current + anticipated add-ons).
2. Use chargemaster prices for uninsured.
3. Generate PDF (CMS-prescribed format).
4. Send to patient; require e-signature.
5. Store signed copy in S3 with audit trail.
6. If patient disputes, create a `dispute_reason` and route to billing.

**Advanced EOB (AEOB)**: A patient-friendly explanation of benefits, generated from an ERA, showing what insurance paid, why, and what the patient owes.

### 22.8 Price Transparency (`price-transparency`, V3)

US hospitals must publish standard charges and shoppable services (CMS-0057-F, effective 7/1/2024).

**`PriceTransparencyService`**:
- `GET /api/public/charges?tenant_id=X` → machine-readable JSON (CMS-compliant)
- `GET /api/public/shoppable?tenant_id=X` → 70 shoppable services with negotiated rates by payer
- Auto-updated nightly from `chargemaster` table
- Output format: hospital-specific `cms-pub-0057` JSON schema + a human-readable HTML page

### 22.9 Chargemaster & Cost Accounting (`chargemaster`, V3)

The **Charge Description Master (CDM)** is the canonical price list for every billable item in a hospital.

**Tables:**

```php
Schema::create('chargemaster_items', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('cdm_code', 32)->index();
    $t->string('rev_code', 16);
    $t->string('cpt_hcpcs_code', 16)->nullable();
    $t->string('description', 255);
    $t->string('category', 64)->index();
    $t->decimal('standard_charge', 12, 2);
    $t->decimal('cost', 12, 2)->nullable();
    $t->string('unit', 16)->default('each');
    $t->boolean('is_active')->default(true);
    $t->date('effective_from');
    $t->date('effective_to')->nullable();
    $t->timestamps();
    $t->unique(['tenant_id', 'cdm_code', 'effective_from']);
});

Schema::create('contracted_rates', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('payer_id')->index();
    $t->ulid('chargemaster_item_id')->index();
    $t->decimal('rate', 12, 2);
    $t->string('rate_method', 32);
    $t->decimal('rate_value', 12, 4)->nullable();
    $t->date('effective_from');
    $t->date('effective_to')->nullable();
    $t->timestamps();
});

Schema::create('cost_accounting_entries', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('chargemaster_item_id')->index();
    $t->ulid('department_id')->nullable();
    $t->string('cost_center', 16);
    $t->decimal('direct_cost', 12, 2);
    $t->decimal('overhead_cost', 12, 2);
    $t->decimal('total_cost', 12, 2);
    $t->date('period');
    $t->timestamps();
});
```

**`ChrgmasterService::lookupRate($cdmItem, $payer, $dos)`**:
1. Find active `contracted_rates` for (payer, item, dos in range).
2. Apply `rate_method`: `fee_schedule` → return `rate`; `percent_of_billed` → return `cdmItem.standard_charge * rate_value`; `case_rate` → return `rate`; `per_diem` → return `rate * days`.
3. Fallback to `cdmItem.standard_charge` if no contract.
4. Cache result 24h.

**Cost accounting integration** computes true cost per service. Enables `ContributionMarginService`:
```
Contribution margin = (revenue collected) - (direct cost) - (overhead allocated)
```

### 22.10 Value-Based Care & Risk Adjustment (`vbc-risk-adjustment`, V3)

The future of US healthcare is value-based, not fee-for-service. V3 supports the full spectrum of VBC contracts.

**Tables:**

```php
Schema::create('capitation_contracts', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('payer_id')->index();
    $t->string('contract_name', 128);
    $t->string('panel_type', 16);
    $t->integer('member_months_rate');
    $t->decimal('quality_bonus_pct', 5, 2)->default(0);
    $t->decimal('shared_savings_pct', 5, 2)->default(0);
    $t->decimal('benchmark', 12, 2)->nullable();
    $t->date('effective_from');
    $t->date('effective_to')->nullable();
    $t->json('quality_measures')->nullable();
    $t->timestamps();
});

Schema::create('capitation_payments', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('contract_id')->index();
    $t->ulid('patient_id')->index();
    $t->date('month');
    $t->decimal('amount', 12, 2);
    $t->string('status', 16);
    $t->timestamp('received_at')->nullable();
    $t->timestamps();
    $t->unique(['contract_id', 'patient_id', 'month']);
});

Schema::create('hcc_risk_scores', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->integer('year');
    $t->decimal('raf_score', 6, 3);
    $t->json('hcc_codes')->nullable();
    $t->string('model_version', 16);
    $t->timestamp('computed_at');
    $t->timestamps();
    $t->unique(['patient_id', 'year', 'model_version']);
});
```

**HCC (Hierarchical Condition Categories)** is the risk-adjustment model used by Medicare Advantage and ACA plans.

**`HccRafService::compute($patient, $year)`**:
1. Pull all diagnoses from encounters in the prior year.
2. Apply HCC mapping (HCC v24 for 2024, v28 for 2025+): each ICD-10 → HCC category.
3. Apply hierarchy: more severe HCCs override less severe ones in the same family.
4. Add interaction factors (e.g., disease interactions like DM + CHF).
5. Sum weights to get RAF score.
6. Compare to actual RAF reported by payer; surface gaps for documentation improvement.

**Bundled Payments (BPCI Advanced, CJR):** Episode = a 90-day window around a trigger event (e.g., hip replacement). All claims in the window are summed; compared to the target price. If actual < target, the provider shares in savings; if actual > target, the provider may owe money.

**MIPS / MACRA quality reporting:** Quality measures (e.g., Q001: Diabetes A1c control <9%) tracked per provider. Promoting Interoperability (PI) measures. Improvement Activities (IA) credit. Annual submission to CMS QPP portal via QPP JSON API. `MipsSubmissionService` aggregates and submits by March 31 of the following year.

### 22.11 Hospital-Specific Billing (`hospital-billing`, V3)

Hospitals bill differently from clinics. V3 adds UB-04 (institutional) claims, DRG grouping, and APC logic.

**DRG (Diagnosis-Related Group):** `DrgGrouperService::group($diagnoses, $procedures, $age, $sex, $dischargeStatus)` returns the MS-DRG (e.g., MS-470: Major joint replacement without MCC, $12,000 base payment). Use the CMS MS-DRG Grouper (in-house implementation; for commercial payers, license 3M or Optum). For each inpatient stay, compute the DRG payment = `base_rate * weight * geographic_adjustment * wage_index`.

**APC (Ambulatory Payment Classification):** For hospital outpatient claims, group by CPT → APC → payment. Status indicators (S, T, V, X, etc.) drive packaging/bundling.

**Anesthesia billing:** `AnesthesiaBillingService::compute($procedure, $start, $end, $modifiers)`:
- Base units (per CPT, 3-7 typical)
- Time units = `ceil((end - start) / 15)` minutes
- Modifiers: P1-P6 (physical status), QK/QY/QX/QZ (medical direction), AA (anesthesiologist personally)
- Total = `(base + time) * conversion_factor`

**Implant tracking (UDI):** For high-cost implants (joint, cardiac), FDA requires UDI capture. `implant_udi_di` (Device Identifier) + `implant_udi_pi` (Production Identifier: lot, serial, expiration). `ImplantTrackingService` stores UDI on the claim line.

### 22.12 AI Coding & Fraud Detection (`ai-coding-fraud`, V3)

**AI Coding Assistant:**
- `AiCodingService::suggestCpt($note, $encounterContext)` suggests CPT codes.
- `AiCodingService::suggestIcd10($note, $encounterContext)` suggests ICD-10 codes.
- Models: OpenAI GPT-4o, Anthropic Claude 3.5, fine-tuned medical coding LLM (vLLM self-hosted).
- Output: ranked list with confidence scores. Doctor accepts/edits.
- **Critical:** never auto-post codes. Human-in-the-loop mandatory for medical-legal reasons.

**AI Fraud Detection:**
- `FraudDetectionService::analyzeClaim($claim)` scores risk (0-100):
  - Unusual billing patterns (upcoding, unbundling, phantom billing, billing for deceased patients)
  - Patient-provider relationship anomalies
  - Service-location mismatches
  - Frequency anomalies (e.g., 30 E/M visits in a month for one patient)
- High-risk claims go to a manual review queue (not auto-flagged; not auto-denied).

**AI Documentation Improvement:**
- `DocImprovementService::analyzeEncounter($encounter)` flags missing specificity:
  - "CHF" → suggest "I50.22: Chronic systolic congestive heart failure"
  - "Diabetes" → suggest type, control status, complications
  - "HTN" → suggest "essential" vs. "secondary"

### 22.13 Patient Financial Experience (`patient-financial-experience`, V3)

**Patient payment portal:** Card on file (tokenized via gateway), ACH / bank draft, HSA/FSA cards (auto-detect via BIN range), Apple Pay / Google Pay, BNPL (Affirm Healthcare, Walnut, LazyPay, Klarna), saved payment methods, auto-pay for payment plans, paperless statements, text-to-pay (SMS link), email receipts, W-9 / 1099 download, charity care application (FAP) with sliding scale, payment plan self-service (3, 6, 12, 24 months).

**Sliding fee scale:** `SlidingFeeScaleService::compute($patient, $service, $income, $household_size)`:
- Federal Poverty Level (FPL) lookup for the year
- 0-100% FPL → 100% discount
- 101-200% FPL → 75% discount
- 201-300% FPL → 50% discount
- 301-400% FPL → 25% discount
- 401%+ → full price

**Charity care / Financial Assistance Program (FAP):** IRS 501(r) requires non-profit hospitals to have an FAP. `FapApplicationService::submit($patient, $application, $documents)`:
- Stores application + supporting docs
- Routes to FAP committee
- Decision (approved/denied/partial) with effective period
- If approved, automatically applies sliding scale to all open and future invoices
- Generates approval letter

**Propensity-to-pay scoring:** `PropensityScoreService::score($invoice)` returns a 0-1 likelihood of self-pay. High propensity → standard dunning. Low propensity → fast-track to charity screening.

### 22.14 Multi-Entity & Group Reporting (`multi-entity-finance`, V3)

**Intercompany billing:** For hospital systems with multiple entities. `IntercompanyInvoiceService::create($fromEntity, $toEntity, $amount, $service)`:
- Creates an intercompany invoice
- Auto-posts in both ledgers
- Eliminates on consolidation

**Consolidation:** `ConsolidationService::consolidate($reportingEntity, $period)`:
- Pulls all child entities' trial balances
- Eliminates intercompany transactions
- Produces consolidated P&L, balance sheet, cash flow
- Multi-currency: converts to reporting currency (e.g., USD) at period-end rate

**Group dashboards:** CFO-level dashboard: $X revenue across 12 hospitals, 5 ASCs, 3 imaging centers. Drill down: hospital → service line → department → provider.

### 22.15 Cash Reconciliation & Bank Integration (`cash-recon`, V3)

**Bank reconciliation:** Daily bank statement import (CSV, OFX, QBO, BAI2 formats). `BankReconciliationService::match($bankTxn, $internalTxn)`:
- Exact match: amount + date + reference
- Fuzzy match: amount ± small fee, date ± 3 days, partial reference
- Manual match: human review for unmatched items

**Lockbox integration:** Hospitals with high mail/check volume use bank lockboxes. `LockboxFileParser::parse($lockboxFile)` extracts check amount, check number, invoice number (from remittance stub OCR), and patient ID. Auto-matches to open invoices; auto-posts payments.

**Positive pay:** Bank-issued check fraud prevention. `PositivePayService::generate($date, $bankAccount)` exports check number, date, amount, payee. Bank matches issued file; flags any check not on file (fraud).

**ACH returns:** Patient pays via ACH; bank returns the payment. `AchReturnService::process($returnCode, $originalTxn)` creates reversal transaction, adds $25-35 NSF fee, disables ACH on file for 30 days, flags patient for follow-up.

### 22.16 Dunning Engine (V3, deep)

The dunning engine is a state machine for overdue invoices.

**States:** `current → reminder_1 (day 7) → reminder_2 (day 14) → final_notice (day 30) → collections (day 45) → write_off (day 90) → bad_debt (day 120)`

**Channels (cascading):**
- Day 7: email (free)
- Day 14: SMS (low cost)
- Day 21: email + SMS + WhatsApp
- Day 30: email + SMS + WhatsApp + phone call (auto-dialer)
- Day 45: certified letter (paper)
- Day 60: legal notice
- Day 90: write-off or sell to collections

**Patient preferences override defaults:** Patient can opt out of paper statements. Patient can set "do not call before 9am" etc. (TCPA compliance). Patient can request "do not contact" — system stops outreach but doesn't stop statements.

**Skip rules:** Patient has active dispute → skip. Patient is in active payment plan → skip. Patient has charity approval → skip. Patient is in active bankruptcy → skip (legal requirement).

**`DunningService::run($date, $tenantId)`**:
1. Find all overdue invoices per tenant's dunning policy.
2. For each, check skip rules.
3. Determine next step based on age and history.
4. Send via appropriate channel(s).
5. Record `dunning_letters` row.
6. Schedule next step.

### 22.17 Patient Statements (V3)

**Paperless-first, paper-on-request:** Default: email statement (PDF) + portal notification. Opt-out: physical mail ($0.50-1.00 per cycle).

**Statement content:** Branded header, account summary, open invoices, aging breakdown, payment slip with QR code, "Pay now" link, contact info, dispute instructions, FAP notice (for hospitals).

**Statement design (PDF):** 2-page default. Multi-currency support. Barcode/QR for pay-by-phone. Generated by `StatementGenerator`.

### 22.18 Reporting & Analytics (V3, deep)

**Pre-built reports (45+):** Revenue by day/week/month, by doctor/department/clinic, by payer/plan, A/R aging summary, A/R aging detail, collections by collector, denials by reason code, denials by payer, days in A/R, net/gross collection rate, adjustment analysis, write-off analysis, top unpaid/denied/underpaid claims, patient ledger, charges by CPT/ICD-10, payer mix, service line profitability, payer profitability, provider productivity, cost analysis, contribution margin, cash flow projection, forecast vs. actual, budget variance, MIPS score, HEDIS measures, RAF gap report, capitation revenue, shared savings/losses, BNPL fee analysis, payment plan performance, charity care application stats, collections agency recovery, refund volume, tax collected, multi-entity consolidated.

**Custom report builder:** Drag-and-drop dimensions and measures. Save as private or share. Schedule email delivery. Export CSV/XLSX/PDF. Embed in dashboard.

**Dashboards (live, not batch):** Executive, RCM, billing, collections, provider, payer, cash.

### 22.19 Compliance & Audit (V3, deep)

**HIPAA / GDPR / DPDP compliance:** Every PHI access logged in `data_access_log`. Patient can request data export, deletion, restriction.

**State-specific regulations:** California SB-1376, New York surprise bill, Texas out-of-network, all state-level rules in `state_billing_rules` table per tenant.

**Tax compliance:** 1099-NEC generation for vendors/contractors ($600+ threshold). 1042-S for foreign persons. W-9 collection. TIN matching (IRS TINVS). 1099-DA for digital assets (2025+).

**SOX compliance (for public companies):** Segregation of duties, audit trail, quarterly close checklist, bank reconciliation certification, period lock.

### 22.20 V3+ API Additions (Selected Highlights)

```
GET    /api/v3/rcm/dashboard
GET    /api/v3/rcm/ar-aging?as_of=
GET    /api/v3/rcm/followups?status=&assignee=&type=
POST   /api/v3/rcm/followups/{id}/claim
POST   /api/v3/rcm/followups/{id}/resolve
POST   /api/v3/rcm/followups/{id}/reassign
POST   /api/v3/rcm/claims/{id}/scrub
POST   /api/v3/rcm/claims/{id}/submit
POST   /api/v3/rcm/claims/{id}/resubmit
POST   /api/v3/rcm/claims/{id}/appeal
POST   /api/v3/rcm/claims/{id}/write-off
GET    /api/v3/rcm/denials?payer_id=&from=&to=
GET    /api/v3/rcm/denial-codes
POST   /api/v3/eligibility/check
GET    /api/v3/eligibility/{patient_id}?dos=
GET    /api/v3/cost-estimate?patient_id=&cpt_codes[]=
POST   /api/v3/gfe
GET    /api/v3/gfe/{id}/pdf
POST   /api/v3/gfe/{id}/sign
POST   /api/v3/era/{file_id}/post
GET    /api/v3/era/{file_id}
GET    /api/v3/chargemaster?cdm_code=&cpt=
POST   /api/v3/chargemaster
GET    /api/v3/contracted-rates?payer_id=&cpt=
POST   /api/v3/coding/suggest
POST   /api/v3/coding/ai-scribe-note
GET    /api/v3/raf-scores?patient_id=
GET    /api/v3/raf-gaps?patient_id=
GET    /api/v3/hedis/measures?provider_id=
POST   /api/v3/capitation/reconcile
GET    /api/v3/dunning/policy
PUT    /api/v3/dunning/policy
POST   /api/v3/dunning/preview
POST   /api/v3/charity/applications
POST   /api/v3/charity/applications/{id}/decision
GET    /api/v3/sliding-scale?income=&household_size=
GET    /api/v3/bank-statement/import
POST   /api/v3/bank-reconciliation/match
GET    /api/v3/payment-plans?patient_id=
POST   /api/v3/payment-plans
POST   /api/v3/payment-plans/{id}/cancel
GET    /api/v3/statements/{invoice_id}/pdf
POST   /api/v3/statements/send
GET    /api/v3/reports/{report_key}
POST   /api/v3/reports/{report_key}/schedule
GET    /api/v3/dashboards/{dashboard_key}
POST   /api/v3/audit/reconciliation-exceptions/{id}/resolve
POST   /api/v3/collections/agencies
POST   /api/v3/collections/assignments
GET    /api/v3/collections/assignments/{id}
```

---

## 23. V3+ Test Matrix — Billing

> Total: **~640 tests** across all levels. Tests run on every PR (unit + integration + HTTP), nightly (E2E + perf + security), and on release candidates (real-world + full a11y).

### 23.1 Unit Tests (~340)

**Money value object (28 tests):**
- Constructor with valid input → success
- Constructor with negative amount → throws
- Constructor with invalid currency code (not 3 chars) → throws
- `add()` with same currency → correct sum
- `add()` with different currency → throws InvalidArgumentException
- `subtract()` with same currency → correct difference
- `subtract()` resulting in negative → throws or allows (configurable)
- `multiply(2)` → exact doubling
- `multiply(0.5)` → half (uses bcmath)
- `multiply(1.5)` with 0.0001 fractional → no float drift
- `applyPercent(10)` → 10% discount, correct
- `applyPercent(0)` → 0
- `applyPercent(100)` → zero
- `applyPercent(-10)` → negative discount
- `format('en_IN')` → ₹500.00
- `format('en_US')` → $5.00
- `format('ja_JP')` → ¥500
- `format('ar_SA')` → Arabic numerals
- `toArray()` → all fields present
- Comparison operators (==, <, >, <=, >=)
- Zero money → isZero() true
- Immutability → operations return new instance
- `__toString()` → formatted string
- `jsonSerialize()` → round-trip safe
- Locale-aware grouping (Indian lakh format: ₹1,00,000.00)
- Rounding mode (banker's, half-up, half-down) — config
- `fromFloat(5.00)` → Money(500, 'USD')
- `fromFloat(5.005)` → rounds correctly

**TaxService (38 tests):**
- `computeLineTax` with `tax_mode=exclusive`, percent=18 → tax = 18% of base
- `computeLineTax` with `tax_mode=inclusive`, percent=18 → tax = 18/118 of gross
- `computeLineTax` with `tax_mode=inclusive` and full discount → tax recomputed
- `computeLineTax` with zero percent → tax = 0
- `computeLineTax` with `is_compound=true` → tax on (base + prior tax)
- `computeLineTax` with two compound taxes → chain correctly
- `computeLineTax` with discount applied first → tax base reduced
- `computeLineTax` with negative quantity → throws
- `computeLineTax` with null tax_id → returns zero tax
- `computeLineTax` with inactive tax → throws
- Snapshot logic: invoice line stores tax_percent at time of invoice
- B2B reverse charge (EU): customer has valid VAT ID → tax = 0
- B2B reverse charge: invalid VAT ID → throws
- B2C MOSS (EU digital): origin country tax
- GST composition (India): no ITC, no tax collected
- GST TDS (India): 1% TDS on B2B over threshold
- GST ITC: input tax credit calculation
- 1099-NEC threshold tracking per vendor
- 1042-S withholding for non-resident aliens
- Tax rounding rules: per-line vs. per-invoice
- Tax-inclusive display: shows gross with tax extracted as note
- Tax-exclusive display: shows tax as separate line
- Compound tax order: `taxes` table sort_order vs. array order
- Tax override: manual override locked with reason
- Tax exemption: patient with exemption certificate → tax = 0
- Multi-currency tax: each invoice has own tax context
- Tax audit log: every tax computation logged
- HSN/SAC code (India) on line for GST compliance
- VAT MOSS identifier (EU) on line
- US sales tax: per-state rates (FL=6%, CA varies, OR=0)
- Canadian GST/HST/PST: province-specific
- Australian GST: 10%
- Japan consumption tax: 10% (or 8% for food)
- UK VAT: 20% standard, 5% reduced, 0% zero-rated
- Indian GST slabs: 0/5/12/18/28
- Indian TDS sections: 194J, 194C, 192
- Inactive tax rejected in new invoices
- Expired tax rejected

**InvoiceService (45 tests):**
- `createForAppointment` → invoice with one line per service
- `createForAppointment` with multi-service appointment → multiple lines
- `createManual` with valid data → draft invoice
- `createManual` with no patient → invoice without patient_id
- `addLine` to draft invoice → invoice line created
- `addLine` to paid invoice → throws
- `removeLine` from draft → line removed
- `removeLine` from paid → throws
- `applyCoupon` valid → discount applied, recompute correct
- `applyCoupon` already applied → throws
- `applyCoupon` expired → throws
- `applyCoupon` over usage limit → throws
- `applyCoupon` below min invoice → throws
- `applyCoupon` first-time-only with existing patient → throws
- `applyCoupon` scope mismatch (doctor-only coupon, different doctor) → throws
- `removeCoupon` → discount removed, recompute correct
- `recompute` after line add/remove → all totals updated
- `recompute` with coupon → totals + tax recomputed
- `send` to patient with email → email queued with PDF
- `send` to patient without email → exception logged
- `void` unpaid invoice → status=void, voided_by set
- `void` paid invoice → throws (use refund instead)
- `void` with reason → audit log entry
- `markPaid` with transaction → status=paid, paid_at set
- `markPaid` with partial amount → status=partially_paid
- `applyRefund` full → status=refunded
- `applyRefund` partial → status=partially_refunded
- `applyRefund` more than paid → throws
- `generatePdf` returns PDF binary
- `generatePdf` includes clinic letterhead
- `generatePdf` includes line items with tax breakdown
- `generatePdf` includes QR code for payment
- `generatePdf` multi-page for many lines
- `generatePdf` localized (en, hi, es, fr)
- `generatePdf` right-to-left for Arabic
- `generatePdf` with logo on disk → logo embedded
- Auto-numbering: first invoice of year → `INV-2026-000001`
- Auto-numbering: concurrent creation → no duplicates
- Auto-numbering: cross-year rollover
- Auto-numbering: format from settings respected
- Custom format `INV/{YEAR}/{CLINIC}/{SEQ:5}`
- Draft → unpaid transition on first save
- Voided invoice: cannot be paid
- Voided invoice: cannot be edited
- Subscription invoice: linked to subscription

**TransactionService (22 tests):**
- `recordPayment` with cash → transaction created, status=succeeded
- `recordPayment` with manual_bank → audit log includes reference
- `recordPayment` overpays invoice → status=paid, change tracked
- `recordPayment` negative amount → throws
- `recordGatewayPayment` with Razorpay success → transaction linked
- `recordGatewayPayment` with Stripe failure → status=failed, no invoice update
- `recordGatewayPayment` with Paystack success → transaction linked
- `recordGatewayPayment` with gateway response saved to gateway_response
- `reverse` succeeded transaction → refund transaction, invoice amount_paid reduced
- `reverse` already-reversed → throws
- `reverse` pending transaction → marks as reversed
- Transaction number generation: `TXN-2026-000001`
- Transaction number: concurrent creation → no duplicates
- Idempotency: same gateway_transaction_id twice → no duplicate transaction
- `recordPayment` requires `accountant` or `admin` role
- `recordPayment` with invalid invoice_id → throws
- Gateway fee tracking: net of fees stored
- Multi-currency: transaction in USD for invoice in EUR → FX rate stored
- BNPL fee tracking: Affirm fee stored
- Stripe fee tracking
- Razorpay fee tracking
- `recordPayment` with refund → link to original transaction

**CouponService (24 tests):**
- `validate` active coupon in window → valid
- `validate` inactive coupon → invalid
- `validate` expired coupon → invalid
- `validate` not-yet-started coupon → invalid
- `validate` over total limit → invalid
- `validate` over per-user limit → invalid
- `validate` below min invoice amount → invalid
- `validate` first-time-only with prior patient invoice → invalid
- `validate` service-scoped coupon for matching service → valid
- `validate` service-scoped coupon for non-matching service → invalid
- `validate` doctor-scoped coupon for matching doctor → valid
- `validate` clinic-scoped coupon for non-matching clinic → invalid
- `apply` percentage coupon → discount = percent × subtotal
- `apply` percentage coupon with max cap → discount = min(percent × subtotal, cap)
- `apply` fixed coupon → discount = min(fixed, subtotal)
- `apply` fixed coupon exceeds subtotal → discount = subtotal
- `apply` distributes discount pro-rata across matching lines
- `apply` excludes non-matching lines from discount
- `apply` excludes tax from discount base (configurable)
- `incrementUsage` → used_count incremented atomically
- `incrementUsage` when at limit → throws (race-safe)
- `removeCoupon` reverses discount
- Coupon codes: case-insensitive
- Coupon codes: stored uppercase, normalized

**RefundService (18 tests):**
- `process` full refund via gateway → refund + transaction, invoice status=refunded
- `process` partial refund → status=partially_refunded
- `process` over-refund → throws
- `process` cash/manual refund → no gateway call
- `process` for already-refunded transaction → throws
- `process` with reason → audit log
- `retry` failed gateway refund → re-attempt
- `retry` exhausted → mark as failed permanently
- `computeAndCreate` from cancellation policy (V1)
- Refund fee: gateway fee deducted
- Refund fee: merchant absorbs vs. patient pays
- Refund without original transaction → manual entry allowed
- Refund window: within 180 days → online; beyond → offline
- Refund to original payment method (PCI: never store card)
- Refund to different method (allowed for some gateways with extra KYC)
- Refund: partial amounts sum to original
- Refund: handles currency conversion
- Refund: notifies patient and admin

**InvoiceNumberGenerator (10 tests):**
- First invoice of year → starts at 1
- Sequential increment
- Multi-tenant: tenant A and B both get 000001
- Format `{prefix}-{year}-{seq:6}`
- Format `{prefix}/{year}/{clinic_code}/{seq:4}`
- Concurrent creation: no duplicate numbers (DB sequence lock)
- Roll-over on year change
- Custom suffix for refunds: `INV-2026-000001-R`
- Manual override: admin can set custom number
- Sequence reset on tenant request (audit-logged)

**Money & rounding (15 tests):**
- 100 × ₹0.10 = ₹10.00 (no float drift)
- ₹10.005 rounds to ₹10.01 (half-up) or ₹10.00 (banker's)
- 0.1 + 0.2 = 0.3, not 0.30000000000000004
- Negative money: allowed for refunds (sign in direction)
- Currency conversion: USD → INR with rate
- Currency conversion: rate stored, audit-logged
- Currency conversion: never use float rates
- Truncation: 1.999 → 1.99, not 2.00
- Rounding applied at display, not storage
- Sum of parts = whole (line totals sum to invoice total)
- No rounding gaps: tax + subtotal = total
- Penny errors: detect ₹0.01 mismatches and alert
- Time-zone aware: invoice in IST, report in UTC
- Daylight-saving: invoice date doesn't shift
- Negative zero: -0.00 vs. 0.00 — always 0.00 in display

**Invoice line / coupon / tax interaction (20 tests):**
- Discount on line, exclusive tax → tax on discounted base
- Discount on line, inclusive tax → tax recomputed on gross
- Coupon on invoice, exclusive tax → tax on (subtotal - coupon)
- Coupon on invoice, inclusive tax → recompute
- Line discount + coupon → coupon applied after line discount
- Line tax override → snapshot at line time
- Coupon excludes tax from base → doesn't reduce tax
- Coupon includes tax in base → reduces tax proportionally
- Multiple coupons: only one per invoice (configurable)
- Stacking coupons → configured
- Coupon max discount reached → use cap
- Fixed coupon + line discount → both apply
- Compound tax + line discount → compound base
- Tax-exempt line + taxable line → mixed invoice
- Tax-inclusive: gross remains, tax extracted
- Tax rounding: per-line vs. per-invoice difference
- Add-on coupon: one coupon per item
- Tiered coupon: 5% off ≤$100, 10% off >$100
- Buy-X-get-Y coupon: line items can have negative price
- Free service coupon: 100% off

**Eligibility / cost estimation (25 tests):**
- Eligibility check: active coverage → returns benefits
- Eligibility check: inactive coverage → returns inactive status
- Eligibility check: payer error → returns error, retryable
- Eligibility check: caching → second call returns cached
- Eligibility check: cache invalidation after 24h
- Cost estimate: deductible not met → patient pays full allowed
- Cost estimate: deductible met → coinsurance applies
- Cost estimate: OOP max reached → patient pays 0
- Cost estimate: OOP max partially met → patient pays min(coinsurance, oop_remaining)
- Cost estimate: copay vs. coinsurance precedence
- Cost estimate: secondary insurance (COB) applied
- Cost estimate: out-of-network → uses UCR or chargemaster
- Cost estimate: out-of-network with OON benefit → applies OON deductible
- Cost estimate: HMO requires referral → warn
- Cost estimate: PPO doesn't require referral → no warn
- Cost estimate: Medicaid patient → no cost (in most states)
- Cost estimate: Medicare patient → cost = 20% coinsurance after deductible
- Cost estimate: Medicare Supplement (Medigap) → covers most
- Cost estimate: Medicare Advantage → uses MA contracted rate
- Cost estimate: workers' comp → no patient cost
- Cost estimate: auto accident → no patient cost (PIP first)
- Cost estimate: multi-visit service → prorates
- Cost estimate: GFE required (uninsured + ≥$400) → flag
- Cost estimate: includes self-pay discount if applicable
- Cost estimate: handles non-covered services (CARC 50)

**RCM (claim scrubbing + submission + ERA) (30 tests):**
- Scrubber: R-001 NPI Luhn check fails → block
- Scrubber: R-005 invalid CPT → block
- Scrubber: R-007 no ICD for CPT → block
- Scrubber: R-008 invalid modifier for CPT → block
- Scrubber: R-010 MUE exceeded → block
- Scrubber: R-021 telehealth without modifier 95 → warn
- Scrubber: R-030 global period violation → block
- Scrubber: clean claim → is_clean=true
- Scrubber: warns can be overridden by manager
- Scrubber: blocks cannot be overridden
- Claim submission: 837P generation correct
- Claim submission: 837I (institutional) generation correct
- Claim submission: 837D (dental) generation correct
- Claim submission: 999 ack accepted → status=accepted
- Claim submission: 999 ack rejected → fix and resubmit
- Claim submission: 277CA accepted → status=acknowledged
- Claim submission: 277CA rejected → claim_followups created
- Claim submission: ISA envelope with tenant's NPI, EIN
- Claim submission: NM1 segments for billing, rendering, referring, subscriber, patient
- Claim submission: SV1 service line with CPT, charge, units, dx pointer
- Claim submission: HI segments for ICD-10 codes
- Claim submission: clearinghouse timeout → retry
- Claim submission: clearinghouse error → status=error
- ERA posting: 835 file parsed correctly
- ERA posting: paid amount + adjustments + patient_resp = billed
- ERA posting: matched to internal claim
- ERA posting: contractual adjustment (CO) → write-off
- ERA posting: patient responsibility (PR) → reclassify
- ERA posting: denial (CARC 16) → claim_followups created
- ERA posting: underpayment (>10% below expected) → flag

**AI coding (15 tests):**
- AI suggest CPT for "chest pain" → returns 99213, 93000
- AI suggest ICD-10 for "chest pain" → returns R07.9
- AI suggest for "hypertension" → returns I10, I11.9
- AI suggest with low confidence → flag for manual review
- AI suggest rejects hallucinated CPT (not in cpt_codes table) → filtered
- AI suggest respects patient context (age, sex)
- AI suggest for surgical note → returns surgical CPT
- AI doc improvement: "diabetes" → suggests E11.9, E11.65, E11.21 with criteria
- AI fraud: high-frequency E/M visits → flag
- AI fraud: deceased patient with service → flag
- AI fraud: unusual day-of-week patterns → flag
- AI fraud: phantom provider → flag
- AI output: human must confirm before save (no auto-post)
- AI output: confidence < 0.7 → require doctor review
- AI output: every suggestion logged for audit

**Other services (50 tests):**
- ChargemasterService: lookup rate, fall back to gross
- ChargemasterService: rate_method = percent_of_billed
- ChargemasterService: rate_method = case_rate
- ChargemasterService: rate_method = per_diem
- ChargemasterService: cache hit
- HccRafService: ICD-10 → HCC mapping correct
- HccRafService: hierarchy overrides applied
- HccRafService: interaction factors
- HccRafService: compare computed vs. payer-reported
- BpcIAdvancedService: episode aggregation
- MipsSubmissionService: quality measure aggregation
- CapitationService: PMPM calculation
- CapitationService: monthly capitation invoice
- DunningService: skip rules applied
- DunningService: channel cascade correct
- DunningService: state transitions valid
- DunningService: opt-out honored
- DunningService: TCPA time window respected (8am-9pm local)
- CharityService: FPL lookup
- CharityService: sliding scale tier
- CharityService: approval workflow
- PropensityScoreService: scoring model
- PropensityScoreService: high-propensity routes to standard dunning
- PropensityScoreService: low-propensity routes to charity screening
- BankReconciliationService: exact match
- BankReconciliationService: fuzzy match (date ±3d, ref partial)
- BankReconciliationService: manual match
- BankReconciliationService: exception creation for unmatched
- LockboxParser: parses standard lockbox file
- LockboxParser: remittance stub OCR
- AchReturnService: NSF return reversal
- AchReturnService: account closed → disable ACH
- AchReturnService: NSF fee
- FraudDetectionService: upcoding pattern
- FraudDetectionService: unbundling pattern
- FraudDetectionService: phantom billing pattern
- FraudDetectionService: deceased patient service
- GoodFaithEstimateService: PDF generation
- GoodFaithEstimateService: dispute flow
- GoodFaithEstimateService: e-signature
- AdvanceEobService: generates from ERA
- AdvanceEobService: patient-friendly language
- PriceTransparencyService: 0057-F JSON schema valid
- PriceTransparencyService: 70 shoppable services
- PriceTransparencyService: HTML page generated
- MultiEntityService: intercompany invoice auto-eliminates
- MultiEntityService: currency conversion at period-end rate
- MultiEntityService: consolidated P&L
- StatementService: PDF generation with QR
- StatementService: paperless preference respected
- StatementService: cycle (monthly, quarterly, per-invoice)

### 23.2 Integration Tests (~100)

**Database integration (30):**
- Invoice with all FK relationships → cascade rules
- Soft delete invoice → related transactions preserved
- Hard delete invoice (admin) → audit log + cascade
- Concurrent invoice number generation → unique constraint enforced
- Coupon `used_count` race → atomic increment
- Transaction idempotency: same `gateway_transaction_id` → unique
- Refund cascade: refund → transaction → invoice status
- Insurance claim cascade
- ERA file → claim updates → invoice updates
- Replicated read: invoice created on primary, read from replica
- Partition pruning on `transactions` (by month)
- Index usage: `(tenant_id, status, issued_at)` for report query
- Tenant isolation: tenant A invoice invisible to tenant B
- Tenant isolation: cross-tenant gateway response rejected
- Soft delete with tenant scope → only tenant's own soft-deleted
- Foreign key on `tenant_id` → cannot delete tenant with invoices
- Bulk operations: 10k invoice insert in <10s
- Migration rollback: 1M-row table back in <60s
- Schema migration on live DB: online (no downtime)
- MySQL vs. PostgreSQL parity: same migration runs on both
- Read replica lag: invoice created, replicated in <2s
- Backup verification: 1M-row backup restored in <30min
- Encryption: `gateway_response` field encrypted at rest
- Encryption: key rotation → re-encrypts existing rows
- Field-level encryption: PHI fields like SSN, MRN
- Audit log: every mutation logged
- Audit log: append-only (DB user permissions)
- HIPAA-compliant deletion: hard delete + soft tombstone
- Time-zone handling: invoice in IST, report in UTC, correct
- Charge capture with UDI for implants

**Queue/cache/search integration (20):**
- `SendInvoiceJob` queued, sends email with PDF
- `GenerateStatementJob` for monthly cycle
- `RecomputeInvoiceJob` after line change
- `SyncChargemasterJob` nightly
- `DunningRunJob` daily at 09:00
- `BankReconciliationJob` daily at 02:00
- `EraFileParserJob` on file upload
- `CapitationReconciliationJob` monthly
- `MipsSubmissionJob` quarterly
- `MrrReportJob` for SaaS billing
- Queue retry: failed job → 3 retries with backoff
- Queue dead letter: 3 failures → moves to DLQ
- Redis cache: invoice total cached, invalidated on update
- Redis cache: tax rates cached, refreshed daily
- Redis cache: contracted rates cached per payer
- Redis cache: chargemaster lookup cached 24h
- Meilisearch: invoice index, full-text search
- Meilisearch: claim index, searchable by patient name, payer
- Webhook idempotency: same `event_id` twice → processed once
- Streamed export: 100k transactions to S3 in chunks

**Webhook integration (15):**
- Razorpay `payment.captured` → invoice paid → appointment confirmed
- Razorpay `payment.failed` → invoice status unchanged → patient notified
- Razorpay `refund.processed` → invoice amount_paid reduced
- Razorpay `subscription.activated` → subscription row updated
- Razorpay `subscription.payment_failed` → dunning triggered
- Stripe `payment_intent.succeeded` → invoice paid
- Stripe `charge.dispute.created` → claim_followup + flag
- Stripe `customer.subscription.deleted` → cancel subscription
- Paystack `charge.success` → invoice paid
- Paystack `subscription.create` → create subscription
- Paystack `invoice.payment_failed` → dunning
- Twilio status callback → SMS log updated
- SendGrid event → email log updated
- Webhook signature verification: invalid → 401
- Webhook replay: same event twice → idempotent

**Third-party integration (20):**
- Availity 270/271: parse response correctly
- Change Healthcare 270/271
- Waystar 270/271
- NHIC (India) 270/271
- Availity 837 submission: 999 ack received
- Change Healthcare 837 submission
- Waystar 837 submission
- Office Ally 837 submission
- Razorpay Subscriptions API
- Stripe Subscriptions API
- Paystack Subscriptions API
- GSTN IRN generation (India e-invoicing)
- Peppol e-invoicing (EU)
- Stripe Terminal API
- Razorpay Route (split payouts)
- Stripe Connect (split payouts)
- ABDM (India) integration
- NHCX (India) for insurance claims
- OpenAI / Anthropic / Ollama AI APIs
- Twilio Voice for auto-dialer

**End-to-end flows (15):**
- New patient books appointment → invoice created → pays via Razorpay → appointment confirmed → receipt sent
- Patient cancels 24h before → refund processed → invoice updated
- Cash payment recorded by receptionist → audit log
- Coupon applied at checkout → discount calculated → invoice recomputed
- Tax-exempt patient → tax = 0
- Multi-currency patient (USD) in INR clinic → conversion at booking
- Insurance patient: eligibility check → preauth → claim submission → ERA → payment posting → patient balance
- Insurance denial → followup created → appeal submitted → overturned → claim re-paid
- Hospital inpatient: admission → daily charges → discharge → DRG grouping → claim → payment
- Hospital outpatient: surgery → implant with UDI → APC claim → payment
- Subscription billing: monthly cycle → auto-charge → invoice → receipt
- Capitation: patient enrolled → monthly PMPM → no per-visit invoice
- VBC: RAF gap identified → provider documents → RAF increased → next year capitation up
- Bundle (BPCI): hip replacement episode → 90-day claims aggregated → savings calculated
- Dunning: invoice 90 days old → multiple letters → collections agency → recovered

### 23.3 HTTP / API Tests (~150)

**Invoice endpoints (25):**
- `GET /api/v1/admin/invoices?status=unpaid` → filtered list
- `GET /api/v1/admin/invoices?status=&patient_id=&from=&to=&page=` → compound filter
- `GET /api/v1/admin/invoices/{id}` → 200 with invoice
- `GET /api/v1/admin/invoices/{id}` for other tenant → 404 (tenant scope)
- `POST /api/v1/admin/invoices` valid → 201 with invoice
- `POST /api/v1/admin/invoices` invalid patient → 422
- `POST /api/v1/admin/invoices` no lines → 422
- `POST /api/v1/admin/invoices` invalid tax_id → 422
- `PUT /api/v1/admin/invoices/{id}` for draft → 200
- `PUT /api/v1/admin/invoices/{id}` for paid → 409
- `DELETE /api/v1/admin/invoices/{id}` for draft → 204
- `DELETE /api/v1/admin/invoices/{id}` for paid → 409
- `POST /api/v1/admin/invoices/{id}/void` valid → 200, status=void
- `POST /api/v1/admin/invoices/{id}/void` for paid → 409
- `POST /api/v1/admin/invoices/{id}/send` → 200, email queued
- `GET /api/v1/admin/invoices/{id}/pdf` → 200, application/pdf
- `GET /api/v1/patient/invoices` → only own invoices
- `GET /api/v1/patient/invoices/{id}` for other patient → 404
- `GET /api/v1/patient/invoices/{id}/pdf` → 200
- `POST /api/v1/admin/invoices/{id}/apply-coupon` valid → 200
- `POST /api/v1/admin/invoices/{id}/apply-coupon` invalid code → 422
- `POST /api/v1/admin/invoices/{id}/remove-coupon` → 200
- `POST /api/v1/admin/invoices/{id}/record-payment` cash → 201
- `POST /api/v1/admin/invoices/{id}/record-payment` overpays → 200 with change
- `POST /api/v1/admin/invoices/{id}/record-payment` non-accountant role → 403

**Transaction endpoints (15):**
- `GET /api/v1/admin/transactions?type=payment&from=&to=` → filtered
- `GET /api/v1/admin/transactions?gateway=razorpay` → filtered
- `GET /api/v1/admin/transactions/{id}` → 200
- `GET /api/v1/admin/transactions?status=pending&older=1h` → auto-flag
- `GET /api/v1/patient/transactions` → only own
- `GET /api/v1/patient/transactions/{id}` for other patient → 404
- `POST /api/v1/admin/transactions/{id}/reverse` valid → 200
- `POST /api/v1/admin/transactions/{id}/reverse` already-reversed → 409
- `POST /api/v1/admin/transactions/{id}/reverse` non-admin → 403
- `GET /api/v1/admin/transactions/export?format=csv` → 200, text/csv
- `GET /api/v1/admin/transactions/export?format=xlsx` → 200, application/vnd.openxml
- `GET /api/v1/admin/transactions/{id}/gateway-response` → 200, full JSON
- Bulk export: 10k transactions → streamed CSV
- Date range filter: 10-year range → fast
- Search by patient name → Meilisearch hit

**Coupon endpoints (15):**
- `GET /api/v1/admin/coupons?is_active=true` → 200
- `POST /api/v1/admin/coupons` valid → 201
- `POST /api/v1/admin/coupons` invalid code format → 422
- `POST /api/v1/admin/coupons` ends_at before starts_at → 422
- `GET /api/v1/admin/coupons/{id}` → 200
- `PUT /api/v1/admin/coupons/{id}` → 200
- `PUT /api/v1/admin/coupons/{id}` if used → cannot change value
- `DELETE /api/v1/admin/coupons/{id}` if used → 409
- `GET /api/v1/admin/coupons/{id}/usages` → 200
- `POST /api/v1/public/coupons/validate` valid → 200 with discount
- `POST /api/v1/public/coupons/validate` expired → 200 with valid=false
- `POST /api/v1/public/coupons/validate` scope mismatch → 200 with valid=false
- `POST /api/v1/public/coupons/validate` over limit → 200 with valid=false
- Coupon code normalization: `save10` and `SAVE10` same coupon
- Coupon analytics: usage by day, by patient, by clinic

**Tax endpoints (10):**
- `GET /api/v1/admin/taxes` → 200
- `POST /api/v1/admin/taxes` valid → 201
- `POST /api/v1/admin/taxes` percent > 100 → 422
- `PUT /api/v1/admin/taxes/{id}` → 200
- `PUT /api/v1/admin/taxes/{id}` percent change → audit log
- `DELETE /api/v1/admin/taxes/{id}` if used → 409
- Tax rate snapshot at invoice time (immutable)
- Tax rate change forward-only
- Multi-tenant: tax with `tenant_id=null` is global
- Per-tenant override: tenant's tax overrides global

**Refund endpoints (10):**
- `GET /api/v1/admin/refunds?status=pending` → 200
- `POST /api/v1/admin/refunds` valid → 201
- `POST /api/v1/admin/refunds` over-amount → 422
- `POST /api/v1/admin/refunds` for non-existent transaction → 422
- `GET /api/v1/admin/refunds/{id}` → 200
- `POST /api/v1/admin/refunds/{id}/retry` → 200
- `POST /api/v1/admin/refunds/{id}/retry` for non-failed → 409
- `POST /api/v1/admin/refunds` reason required → 422
- Refund list: 1000 items → paginated 25/page
- Refund to original method: PCI-compliant

**Reports endpoints (20):**
- `GET /api/v1/admin/reports/revenue?from=&to=&group_by=day` → 200
- `GET /api/v1/admin/reports/revenue` with invalid date range → 422
- `GET /api/v1/admin/reports/revenue?group_by=doctor` → breakdown
- `GET /api/v1/admin/reports/tax?tax_id=` → 200
- `GET /api/v1/admin/reports/doctor-earnings?from=&to=` → 200
- `GET /api/v1/admin/reports/transactions?status=&method=` → 200
- `GET /api/v1/admin/reports/dashboard` → 200 with summary
- `GET /api/v1/admin/reports/revenue?format=csv` → 200 CSV
- `GET /api/v1/admin/reports/revenue?format=xlsx` → 200 XLSX
- `GET /api/v1/admin/reports/revenue?format=pdf` → 200 PDF
- Custom date range: 5-year range → fast
- Cross-clinic: report across multiple clinics
- Doctor filter: only that doctor's data
- Service line filter: only that service line
- Payer filter: only Medicare claims
- Export streaming: 100k transactions → streamed
- Scheduled report: weekly email
- RCM dashboard endpoint (V3)
- A/R aging endpoint
- Denial rate endpoint
- Days in A/R endpoint

**Insurance endpoints (V2, 20):**
- `GET /api/v2/insurance/payers` → 200
- `POST /api/v2/insurance/payers` valid → 201
- `POST /api/v2/insurance/policies` valid → 201
- `POST /api/v2/insurance/preauth` → 201
- `POST /api/v2/insurance/preauth` submit → status=submitted
- `GET /api/v2/insurance/preauth/{id}` → 200
- `POST /api/v2/insurance/claims` → 201
- `POST /api/v2/insurance/claims/{id}/submit` → 200
- `POST /api/v2/insurance/claims/{id}/appeal` valid → 200
- `GET /api/v2/insurance/denials` → 200
- Eligibility check endpoint (V3)
- Cost estimate endpoint (V3)
- GFE generate + sign (V3)
- Chargemaster CRUD (V3)
- Contracted rates CRUD (V3)
- Coding AI suggest (V3)
- RAF score (V3)
- HEDIS measures (V3)
- Capitation reconcile (V3)
- Bank reconciliation (V3)

**Subscription endpoints (V2, 15):**
- `GET /api/v2/subscriptions/plans` → 200
- `POST /api/v2/subscriptions` → 201
- `GET /api/v2/subscriptions/me` → 200
- `POST /api/v2/subscriptions/{id}/cancel` → 200
- `POST /api/v2/subscriptions/{id}/pause` → 200
- `POST /api/v2/subscriptions/{id}/resume` → 200
- `GET /api/v2/subscription-invoices` → 200
- `POST /api/v2/patient-packages` → 201
- `GET /api/v2/patient-packages` → 200
- `POST /api/v2/patient-packages/{id}/redeem` → sessions_used++
- Webhook: subscription.activated
- Webhook: subscription.renewed
- Webhook: subscription.payment_failed
- Webhook: subscription.cancelled
- Dunning triggered on payment failure

**Dunning + statements (V3, 10):**
- `GET /api/v3/dunning/policy` → 200
- `PUT /api/v3/dunning/policy` → 200
- `POST /api/v3/dunning/preview` → schedule preview
- `POST /api/v3/statements/send` → 200
- `GET /api/v3/statements/{id}/pdf` → 200 PDF
- `GET /api/v3/payment-plans` → 200
- `POST /api/v3/payment-plans` → 201
- `POST /api/v3/payment-plans/{id}/cancel` → 200
- `POST /api/v3/charity/applications` → 201
- `POST /api/v3/charity/applications/{id}/decision` → 200

### 23.4 E2E Tests (~30, Playwright)

**Patient flows (8):**
- Patient books appointment, pays via Razorpay test mode → sees confirmation
- Patient applies coupon SAVE10 → sees discount in summary
- Patient pays via Stripe test mode → sees success
- Patient cancels appointment → sees refund amount
- Patient downloads invoice PDF
- Patient views statement → clicks "Pay now" → pays
- Patient sets up payment plan → confirms
- Patient signs GFE → sees e-signature confirmation

**Staff flows (10):**
- Staff creates manual invoice → patient receives email with link → patient pays
- Staff records cash payment → invoice marked paid
- Staff applies coupon to manual invoice
- Staff issues refund → invoice updated
- Staff runs dunning preview → sees schedule
- Staff processes insurance preauth → status=submitted
- Staff processes insurance claim → claim submitted to clearinghouse
- Staff views RCM dashboard → filters by assignee
- Staff follows up on denial → writes appeal letter
- Staff runs charge capture from mobile

**Admin flows (8):**
- Admin views revenue report → exports CSV → opens in Excel
- Admin views A/R aging dashboard
- Admin sets dunning policy → applies to tenant
- Admin creates coupon with usage limits → patient uses it → counter increments
- Admin views denial analysis → top reason codes
- Admin runs MIPS report → submits to QPP
- Admin views multi-entity consolidated P&L
- Admin performs bank reconciliation → matches transactions

**Webhook flow (4):**
- Razorpay webhook test → invoice paid in real time
- Stripe webhook test → subscription activated
- Insurance ERA auto-posting test
- BNPL Affirm approval test

### 23.5 Performance Tests (~10, k6)

- `GET /api/v1/admin/invoices?from=&to=` (10k invoices) → p95 < 200ms
- `POST /api/v1/admin/invoices` → p95 < 300ms
- `GET /api/v1/admin/reports/revenue?from=&to=` (1M transactions) → p95 < 1s
- `POST /api/v3/rcm/claims/{id}/submit` → p95 < 500ms (clearinghouse call is async)
- `POST /api/v3/era/{file_id}/post` (1000 claims in ERA) → p95 < 30s
- `POST /api/v3/dunning/run` (10k overdue invoices) → p95 < 5min
- `POST /api/v3/bank-reconciliation/match` (1000 bank txns) → p95 < 30s
- Concurrent invoice creation: 100 parallel → all succeed, no duplicate numbers
- Coupon race: 1000 parallel coupon uses → `used_count` accurate
- Report export: 100k transactions → completes in < 60s (streamed)

### 23.6 Security Tests (~20, OWASP)

- SQL injection in invoice number: `' OR 1=1--` → sanitized
- XSS in invoice notes: `<script>alert(1)</script>` → escaped
- CSRF on invoice create → token required
- Auth bypass: no token → 401
- Auth bypass: expired token → 401
- Tenant isolation: token from tenant A → 404 for tenant B invoice
- IDOR: invoice id → 404 if not in user's tenant
- Refund race: two concurrent refunds → only one succeeds (DB lock)
- Mass assignment: `is_admin: true` in invoice create → ignored
- Privilege escalation: patient role → cannot create admin invoice → 403
- Negative amount: `amount: -1000` → 422
- Integer overflow: `amount: PHP_INT_MAX` → 422
- Replay attack: webhook re-posted → idempotent
- Signature verification: tampered webhook → 401
- PHI access: patient A reads patient B's invoice → 404
- Audit log: tampering attempt → blocked (append-only)
- Backup encryption: at-rest and in-transit
- PII in logs: invoice PDF generation → no PII in logs
- Rate limiting: 1000 invoice creates/sec → 429
- File upload: malicious PDF → scanned, blocked

### 23.7 Accessibility Tests (~5, axe-core)

- Invoice PDF → screen-reader friendly (tagged)
- Web admin invoice list → keyboard navigable
- Web admin cost estimate → high contrast
- Web admin refund form → labels associated
- Web admin RCM dashboard → ARIA roles correct

### 23.8 Compatibility Matrix

- PHP 8.2 / 8.3 / 8.4
- Laravel 11.x / 12.x
- MySQL 8.0 / 8.4 / MariaDB 10.11 / PostgreSQL 16
- Redis 7.2 / 7.4
- Browser: Chrome 125+, Firefox 125+, Safari 17+, Edge 125+
- iOS 16+ / Android 10+
- Flutter 3.24 / 3.27
- Time zones: all IANA zones, DST transitions

### 23.9 Real-World Scenarios (~25, with real services in test mode)

- Razorpay test mode: full payment flow
- Stripe test mode: full payment flow
- Paystack test mode: full payment flow
- ABDM sandbox: ABHA fetch, FHIR sync
- LiveKit Cloud test: video room, recording
- Ollama local: AI scribe, coding suggest
- OpenAI test: AI scribe, coding suggest
- Stripe Terminal: physical card reader test
- Twilio test: SMS send, voice call
- WhatsApp Business test: message send
- Razorpay Route: split payout test
- Stripe Connect: split payout test
- Stripe Subscriptions: full lifecycle
- Razorpay Subscriptions: full lifecycle
- Paystack Subscriptions: full lifecycle
- Klarna BNPL test: approval, repayment
- Availity sandbox: 270/271, 837, 835
- Change Healthcare sandbox: 270/271, 837, 835
- Waystar sandbox: 270/271, 837, 835
- NHIC sandbox (India): eligibility
- GSTN sandbox: IRN generation
- Peppol test: e-invoicing
- NHCX (India): claim submission
- Bank statement import: real CSV
- Lockbox file: real lockbox file
- CAQH ProView: provider credentialing

---

## 24. V3+ Deep Dives — Billing

> Six deep dives covering the most critical and complex code paths in the V3+ billing module.

### 24.1 Deep Dive: Money Value Object

The foundation of all financial correctness. Floats are banned everywhere. Every monetary value passes through `Money`.

```php
namespace App\Modules\Billing\Support;

use App\Modules\Billing\Exceptions\CurrencyMismatchException;
use Brick\Math\BigInteger;
use Brick\Math\Exception\MathException;

final class Money implements \JsonSerializable
{
    public function __construct(
        public readonly int $minor,
        public readonly string $currency,   // ISO 4217 3-letter code
    ) {
        if (!preg_match('/^[A-Z]{3}$/', $currency)) {
            throw new \InvalidArgumentException("Invalid currency: {$currency}");
        }
    }

    public static function zero(string $currency): self
    {
        return new self(0, $currency);
    }

    public static function fromMajor(int|float|string $major, string $currency, int $scale = 2): self
    {
        if (is_float($major) && !is_finite($major)) {
            throw new \InvalidArgumentException("Money cannot be infinity or NaN");
        }
        $minor = (int) bcmul((string) $major, (string) (10 ** $scale), 0);
        return new self($minor, $currency);
    }

    public function add(self $other): self
    {
        $this->assertSameCurrency($other);
        return new self($this->minor + $other->minor, $this->currency);
    }

    public function subtract(self $other): self
    {
        $this->assertSameCurrency($other);
        return new self($this->minor - $other->minor, $this->currency);
    }

    public function multiply(int|float|string $factor): self
    {
        if (is_float($factor) && !is_finite($factor)) {
            throw new \InvalidArgumentException("Factor cannot be infinity or NaN");
        }
        $result = (int) bcmul((string) $this->minor, (string) $factor, 0);
        return new self($result, $this->currency);
    }

    public function divide(int|float|string $divisor): self
    {
        if ((float)$divisor == 0.0) {
            throw new \DivisionByZeroError("Cannot divide Money by zero");
        }
        $result = (int) bcdiv((string) $this->minor, (string) $divisor, 0);
        return new self($result, $this->currency);
    }

    public function applyPercent(float $percent, int $roundingMode = PHP_ROUND_HALF_UP): self
    {
        if ($percent < -100 || $percent > 100000) {
            throw new \InvalidArgumentException("Percent out of range: {$percent}");
        }
        $multiplier = bcdiv((string)$percent, '100', 10);
        $result = (int) bcmul((string)$this->minor, $multiplier, 0);
        $rounded = $this->round($result, $roundingMode);
        return new self($rounded, $this->currency);
    }

    public function applyDiscountPercent(float $percent, ?int $maxDiscountMinor = null): self
    {
        $discount = $this->applyPercent(-$percent);
        if ($maxDiscountMinor !== null && abs($discount->minor) > $maxDiscountMinor) {
            $discount = new self(-$maxDiscountMinor, $this->currency);
        }
        if (abs($discount->minor) > $this->minor) {
            $discount = new self(-$this->minor, $this->currency);
        }
        return $this->add($discount);
    }

    public function isZero(): bool { return $this->minor === 0; }
    public function isPositive(): bool { return $this->minor > 0; }
    public function isNegative(): bool { return $this->minor < 0; }
    public function abs(): self { return new self(abs($this->minor), $this->currency); }
    public function negate(): self { return new self(-$this->minor, $this->currency); }

    public function format(?string $locale = null, bool $includeSymbol = true): string
    {
        $locale ??= app()->getLocale();
        $formatter = new \NumberFormatter($locale, \NumberFormatter::CURRENCY);
        $symbol = $formatter->getSymbol(\NumberFormatter::CURRENCY_SYMBOL);
        $amount = $this->minor / 100;
        $formatted = $formatter->format($amount);
        return $includeSymbol ? $formatted : preg_replace('/^' . preg_quote($symbol, '/') . '\s*/', '', $formatted);
    }

    public function formatIndian(?string $locale = null): string
    {
        $locale ??= 'en_IN';
        $formatter = new \NumberFormatter($locale, \NumberFormatter::CURRENCY);
        return $formatter->format($this->minor / 100);
    }

    public function formatArabic(?string $locale = null): string
    {
        $locale ??= 'ar_SA';
        $formatter = new \NumberFormatter($locale, \NumberFormatter::CURRENCY);
        return $formatter->format($this->minor / 100);
    }

    public function toArray(): array
    {
        return [
            'amount_minor' => $this->minor,
            'amount_major' => $this->minor / 100,
            'currency'     => $this->currency,
            'formatted'    => $this->format(),
            'is_zero'      => $this->isZero(),
        ];
    }

    public function jsonSerialize(): array { return $this->toArray(); }
    public function __toString(): string { return $this->format(); }

    public function equals(self $other): bool
    {
        return $this->minor === $other->minor && $this->currency === $other->currency;
    }

    public function greaterThan(self $other): bool
    {
        $this->assertSameCurrency($other);
        return $this->minor > $other->minor;
    }

    public function lessThan(self $other): bool
    {
        $this->assertSameCurrency($other);
        return $this->minor < $other->minor;
    }

    private function assertSameCurrency(self $other): void
    {
        if ($this->currency !== $other->currency) {
            throw new CurrencyMismatchException(
                "Currency mismatch: {$this->currency} vs {$other->currency}"
            );
        }
    }

    private function round(int $value, int $mode): int
    {
        return match($mode) {
            PHP_ROUND_HALF_UP => (int)round($value, 0, PHP_ROUND_HALF_UP),
            PHP_ROUND_HALF_EVEN => $this->bankersRound($value),
            default => $value,
        };
    }

    private function bankersRound(int $value): int
    {
        $floor = (int)floor($value / 2) * 2;
        $ceil = $floor + 2;
        if (abs($value - $floor) < abs($value - $ceil)) return $floor;
        if (abs($value - $ceil) < abs($value - $floor)) return $ceil;
        return $value % 2 === 0 ? $floor : $ceil;
    }
}
```

**Critical invariants enforced:**

1. **Currency match required for arithmetic** — `add(USD)` to `EUR` throws `CurrencyMismatchException`.
2. **No float math** — all operations use `bcmath` strings internally.
3. **Discount cap** — discount can never exceed the amount (no negative totals).
4. **Immutability** — every method returns a new `Money` instance.
5. **Rounding mode** — `PHP_ROUND_HALF_UP` default, `PHP_ROUND_HALF_EVEN` (banker's) for accounting per IFRS.
6. **Locale-aware formatting** — `format('en_IN')` shows ₹1,00,000.00 (Indian lakh grouping); `format('ar_SA')` shows Arabic numerals.
7. **JSON-safe** — `jsonSerialize()` returns array suitable for API responses.
8. **`__toString()`** — implicit string conversion, used in templates.

**Database storage:**
- Always integer `bigint` column for `*_minor`.
- `currency` column: `char(3)`.
- `Money` is hydrated from DB in model casts:
  ```php
  protected function casts(): array
  {
      return [
          'total' => MoneyCast::class.':total,currency',
          'tax_total' => MoneyCast::class.':tax_total,currency',
      ];
  }
  ```
- `MoneyCast` reads both columns, constructs `Money`, exposes both `minor` and `currency` as read-only virtual attributes.

**Common pitfalls prevented:**
- `0.1 + 0.2` is `0.3` (not `0.30000000000000004`).
- `0.005` rounds to `0.01` (configurable; banker's rounds to `0.00`).
- Multiplication never causes silent loss (`$1.00 * 0.07` = `$0.07` exactly, not `$0.0699...`).
- All operations log-friendly: `toArray()` for audit trail.

### 24.2 Deep Dive: TaxService Engine

The tax engine is the most-regulated piece of code. Bugs mean fines. We get it right.

```php
namespace App\Modules\Billing\Services;

use App\Modules\Billing\Exceptions\TaxMismatchException;
use App\Modules\Billing\Models\Tax;
use App\Modules\Billing\Models\InvoiceLine;
use App\Modules\Billing\Exceptions\InvalidTaxModeException;
use App\Modules\Billing\Support\Money;

class TaxService
{
    public function __construct(
        private readonly TaxRepository $taxes,
        private readonly AuditService $audit,
    ) {}

    public function computeLineTax(Money $taxableBase, Tax $tax, string $taxMode): Money
    {
        if (!in_array($taxMode, ['inclusive', 'exclusive'], true)) {
            throw new InvalidTaxModeException("Tax mode must be inclusive or exclusive: got '{$taxMode}'");
        }
        if ($taxableBase->isZero() || $tax->percent == 0) {
            return Money::zero($taxableBase->currency);
        }

        return match($taxMode) {
            'inclusive' => $this->computeInclusiveTax($taxableBase, $tax),
            'exclusive' => $this->computeExclusiveTax($taxableBase, $tax),
        };
    }

    private function computeExclusiveTax(Money $base, Tax $tax): Money
    {
        return $base->applyPercent((float)$tax->percent, PHP_ROUND_HALF_EVEN);
    }

    private function computeInclusiveTax(Money $gross, Tax $tax): Money
    {
        // For inclusive: gross already contains tax.
        // To extract tax: tax = gross × percent / (100 + percent)
        $multiplier = bcdiv((string)$tax->percent, bcadd((string)$tax->percent, '100', 10), 10);
        $taxMinor = (int) bcmul((string)$gross->minor, $multiplier, 0);
        return new Money($this->bankersRound($taxMinor), $gross->currency);
    }

    public function computeCompoundTaxes(Money $base, array $taxes, string $taxMode): array
    {
        $running = $base;
        $results = [];
        foreach ($taxes as $tax) {
            $line = $this->computeLineTax($running, $tax, $taxMode);
            $results[] = ['tax' => $tax, 'amount' => $line];
            if ($tax->is_compound) {
                $running = $running->add($line);
            }
        }
        return $results;
    }

    public function computeReverseCharge(Money $base, Tax $tax, ?string $customerVatId, ViesClient $vies): Money
    {
        if (!$customerVatId) {
            return $this->computeExclusiveTax($base, $tax);
        }
        $isValid = $vies->validateVatId($customerVatId);
        if ($isValid) {
            return Money::zero($base->currency);
        }
        return $this->computeExclusiveTax($base, $tax);
    }

    public function computeGstWithItc(Money $base, Tax $cgst, Tax $sgst, float $itcAvailable): array
    {
        $output = $this->computeExclusiveTax($base, $cgst)->add(
            $this->computeExclusiveTax($base, $sgst)
        );
        $itc = $this->moneyFromMajor($itcAvailable, $base->currency);
        $net = $output->subtract($itc);
        return [
            'output_gst' => $output,
            'itc_claimed' => $itc,
            'net_payable' => $net,
        ];
    }

    public function snapshotTax(InvoiceLine $line): void
    {
        $tax = $this->getActiveForLine($line);
        if ($tax) {
            $line->tax_id = $tax->id;
            $line->tax_percent = $tax->percent;
            $line->tax_mode = $line->tax_mode ?? 'exclusive';
            $line->save();
            $this->audit->log('invoice_line.tax_snapshotted', $line, [
                'tax_id' => $tax->id,
                'percent' => $tax->percent,
            ]);
        }
    }
}
```

**Key edge cases handled:**

1. **Compound tax in correct order**: CGST → SGST → Cess (India), VAT → Service tax (old India), Federal → State (US). The `sort_order` field enforces.
2. **Inclusive tax back-out**: When price already includes tax, the formula `tax = gross × rate / (100 + rate)` extracts the tax portion. Used for retail B2C, MOSS, and inclusive display.
3. **Reverse charge B2B (EU)**: Validates customer VAT ID via VIES (VAT Information Exchange System) API. If valid, tax = 0; invoice says "Reverse charge — recipient liable for VAT".
4. **Snapshot at invoice time**: A tax master's percent can change (e.g., GST 18% → 20% on April 1). The invoice line stores the percent at the time of the line, so historical invoices remain correct.
5. **Half-up vs. banker's rounding**: Tax math is sensitive. Default to banker's (rounds 0.5 to even) for accounting consistency; some jurisdictions (India) require half-up.
6. **ITC for GST-registered clinics**: Tracks input tax credit (tax paid on inputs) to compute net GST payable. Critical for B2B healthcare suppliers.
7. **Multi-tenant tax overrides**: A tenant can override the global tax master (e.g., a clinic in a special economic zone with 0% GST).
8. **Tax exemption certificates**: Patient uploads a tax exemption cert; service codes linked to exemption are taxed at 0%.

**Tests demonstrating correctness:**

- `TaxCalculationTest::testInclusiveTaxExtraction` — gross 118, rate 18% → tax = 18, base = 100.
- `TaxCalculationTest::testCompoundTax` — base 100, tax1 10% compound, tax2 5% on top → tax1=10, tax2=5.50, total=115.50.
- `TaxCalculationTest::testReverseCharge` — customer VAT valid → tax=0, customer VAT invalid → tax=18.
- `TaxCalculationTest::testSnapshot` — invoice line tax_percent frozen at line time.
- `TaxCalculationTest::testItcOffset` — output 180, ITC 50 → net payable 130.

### 24.3 Deep Dive: Claim Scrubbing Engine

The scrubber catches 80% of avoidable denials before they leave the system.

```php
namespace App\Modules\Billing\Rcm;

use App\Modules\Billing\Models\Claim;
use App\Modules\Billing\Models\Patient;
use App\Modules\Billing\Models\Provider;
use App\Modules\Billing\Models\InsurancePolicy;

class ClaimScrubbingService
{
    /** @var ScrubRule[] */
    private array $rules = [];

    public function __construct(
        private readonly CptCodeRepository $cpt,
        private readonly Icd10CodeRepository $icd10,
        private readonly CciEditsRepository $cci,
        private readonly MueRepository $mue,
        private readonly LcdNcdRepository $lcdNcd,
        private readonly ProviderEnrollmentRepository $enrollment,
        private readonly OigExclusionService $oig,
    ) {
        $this->rules = $this->loadRules();
    }

    public function scrub(Claim $claim): ScrubResult
    {
        $result = new ScrubResult();
        foreach ($this->rules as $rule) {
            $outcome = $rule->check($claim, $this);
            if ($outcome->severity === ScrubOutcome::BLOCK) {
                $result->addError($outcome);
            } elseif ($outcome->severity === ScrubOutcome::WARN) {
                $result->addWarning($outcome);
            }
        }
        return $result;
    }

    private function loadRules(): array
    {
        return [
            new NpiLuhnRule(),              // R-001
            new TaxIdFormatRule(),          // R-002
            new PatientDobRule(),           // R-003
            new SubscriberRelationRule(),   // R-004
            new CptCodeValidRule($this->cpt),  // R-005
            new Icd10CodeValidRule($this->icd10),  // R-006
            new CptIcdLinkageRule(),       // R-007
            new ModifierCptRule(),         // R-008
            new CciEditsRule($this->cci),  // R-009
            new MueRule($this->mue),       // R-010
            new LcdNcdRule($this->lcdNcd), // R-011
            new FrequencyRule(),           // R-012
            new PlaceOfServiceRule(),      // R-013
            new ReferringProviderRule(),   // R-014
            new PriorAuthRule(),           // R-015
            new TimelyFilingRule(),        // R-016
            new PatientNameFormatRule(),   // R-017
            new AddressFormatRule(),       // R-018
            new DiagnosisPointerRule(),    // R-019
            new TelehealthModifierRule(),  // R-021
            new ProviderEnrollmentRule($this->enrollment),  // R-023
            new OigExclusionRule($this->oig),                // R-025
            new ServiceDateRule(),         // R-026
            new BundledProcedureRule($this->cci),  // R-028
            new AddOnCodeRule(),           // R-029
            new GlobalPeriodRule(),        // R-030
        ];
    }
}
```

**Example rule — CPT-ICD linkage (R-007):**

```php
class CptIcdLinkageRule implements ScrubRule
{
    public function check(Claim $claim, ClaimScrubbingService $ctx): ScrubOutcome
    {
        $unsupported = [];
        foreach ($claim->serviceLines as $line) {
            $cpt = $line->cpt_code;
            $icdCodes = $line->icd10Codes;
            $linkageTable = $ctx->cci->getIcd10RequiredForCpt($cpt, $claim->payerId);
            $covered = array_intersect($icdCodes, $linkageTable);
            if (empty($covered) && !empty($linkageTable)) {
                $unsupported[] = $cpt;
            }
        }
        if (!empty($unsupported)) {
            return ScrubOutcome::block(
                'R-007',
                "CPT code(s) not supported by any ICD-10: " . implode(', ', $unsupported),
                ['cpt_codes' => $unsupported]
            );
        }
        return ScrubOutcome::pass();
    }
}
```

**Example rule — MUE (Medically Unlikely Edits, R-010):**

```php
class MueRule implements ScrubRule
{
    public function check(Claim $claim, ClaimScrubbingService $ctx): ScrubOutcome
    {
        $violations = [];
        foreach ($claim->serviceLines as $line) {
            $mueLimit = $ctx->mue->getLimit($line->cpt_code, $claim->payerId);
            if ($mueLimit !== null && $line->units > $mueLimit) {
                $violations[] = [
                    'cpt' => $line->cpt_code,
                    'units' => $line->units,
                    'limit' => $mueLimit,
                ];
            }
        }
        if (!empty($violations)) {
            return ScrubOutcome::block('R-010', 'MUE violation', $violations);
        }
        return ScrubOutcome::pass();
    }
}
```

**Rule data sources:**
- `cpt_codes` table: 10,000+ CPT/HCPCS codes, refreshed monthly from AMA
- `icd10_codes` table: 95,000+ ICD-10-CM codes, refreshed annually (Oct 1)
- `cci_edits` table: NCCI procedure-to-procedure edits (~600,000 pairs)
- `mue_limits` table: CMS MUE values, refreshed quarterly
- `lcd_ncd` table: Local/National Coverage Determinations, refreshed monthly
- `provider_enrollments` table: PECOS enrollment per payer per provider
- `oig_exclusions` table: OIG/SAM exclusion list, refreshed monthly

**Override mechanism:**
- A "block" can never be overridden automatically; it requires manual fix.
- A "warn" can be overridden by the rendering provider (with reason), a billing manager (without reason), or with 4-eyes confirmation.
- Every override is audit-logged with user, time, and reason.

**Performance:**
- Scrubber runs against ~30 rules per claim.
- A claim with 5 service lines takes ~50ms to scrub (in-process).
- For 1000 claims, ~50 seconds; run in background as `BatchScrubClaimsJob`.
- Cached lookups (CPT, ICD-10, MUE) in Redis with 24h TTL.

### 24.4 Deep Dive: ERA Auto-Posting

The ERA-to-invoice pipeline is the highest-volume automated process in RCM.

```php
namespace App\Modules\Billing\Rcm;

use App\Modules\Billing\Models\EraFile;
use App\Modules\Billing\Models\EraClaim;
use App\Modules\Billing\Models\Claim;
use App\Modules\Billing\Models\Transaction;
use App\Modules\Billing\Models\Invoice;
use App\Modules\Billing\Events\EraPosted;
use App\Modules\Billing\Events\ClaimDenied;
use App\Modules\Billing\Events\ClaimUnderpaid;

class EraPostingService
{
    public function __construct(
        private readonly Edi835Parser $parser,
        private readonly ClaimMatcher $matcher,
        private readonly TransactionService $txSvc,
        private readonly InvoiceService $invSvc,
        private readonly ClaimFollowupService $followupSvc,
        private readonly AuditService $audit,
    ) {}

    public function post(EraFile $eraFile): void
    {
        $parsed = $this->parser->parse(file_get_contents($eraFile->s3_path));
        $eraFile->update(['status' => 'parsed', 'claim_count' => count($parsed->claims)]);

        $postedCount = 0;
        $exceptionCount = 0;

        foreach ($parsed->claims as $eraClaim) {
            try {
                DB::transaction(function () use ($eraClaim, $eraFile, &$postedCount) {
                    $internal = $this->matcher->match($eraClaim);
                    $this->postEraClaim($eraFile, $eraClaim, $internal);
                    $postedCount++;
                });
            } catch (EraPostingException $e) {
                $exceptionCount++;
                $eraFile->addException($e->getMessage(), $eraClaim);
            }
        }

        $eraFile->update([
            'status' => $exceptionCount === 0 ? 'posted' : 'posted_with_exceptions',
            'posted_at' => now(),
        ]);

        $this->audit->log('era.posted', $eraFile, [
            'claim_count' => count($parsed->claims),
            'posted' => $postedCount,
            'exceptions' => $exceptionCount,
        ]);
    }

    private function postEraClaim(EraFile $eraFile, $eraClaim, ?Claim $internal): void
    {
        if (!$internal) {
            throw new EraPostingException("Could not match ERA claim to internal claim");
        }

        $eraClaimModel = EraClaim::create([
            'era_file_id' => $eraFile->id,
            'claim_id' => $internal->id,
            'claim_status_code' => $eraClaim->statusCode,
            'billed_amount' => $eraClaim->billed,
            'allowed_amount' => $eraClaim->allowed,
            'paid_amount' => $eraClaim->paid,
            'patient_responsibility' => $eraClaim->patientResp,
            'adjustments' => $eraClaim->adjustments,
            'service_lines' => $eraClaim->serviceLines,
        ]);

        // Validate math: paid + adjustments + patient_resp = billed
        $totalAdjustments = array_sum(array_column($eraClaim->adjustments, 'amount'));
        $reconciled = $eraClaim->paid + $totalAdjustments + $eraClaim->patientResp;
        if (abs($reconciled - $eraClaim->billed) > 0.01) {
            throw new EraPostingException(sprintf(
                "ERA math doesn't reconcile: billed %.2f, paid %.2f, adj %.2f, pr %.2f",
                $eraClaim->billed, $eraClaim->paid, $totalAdjustments, $eraClaim->patientResp
            ));
        }

        // Post the payment
        $transaction = $this->txSvc->recordGatewayPayment(
            $internal->invoice,
            new PaymentGatewayResult(
                gateway: $eraFile->payer_name,
                gatewayTransactionId: $eraFile->trace_number . '-' . $eraClaim->claimId,
                amount: $eraClaim->paid,
                status: 'succeeded',
                response: ['era_file_id' => $eraFile->id]
            )
        );

        // Process adjustments (group_code: CO, PR, OA, PI)
        foreach ($eraClaim->adjustments as $adj) {
            $this->processAdjustment($internal, $eraClaimModel, $adj);
        }

        // Process patient responsibility
        if ($eraClaim->patientResp > 0) {
            $internal->invoice->addPatientResponsibility($eraClaim->patientResp);
        }

        // Update claim status
        $internal->update(['status' => $this->mapStatusCode($eraClaim->statusCode)]);

        // Trigger followups for denials/underpayments
        if ($eraClaim->statusCode === '3' || $eraClaim->statusCode === '4') {
            event(new ClaimDenied($internal, $eraClaim->adjustments, $eraClaimModel));
            $this->followupSvc->autoCreate($internal, 'denial', $eraClaim->firstDenialReason());
        } else {
            $expected = $internal->calculateExpectedPayment();
            if ($eraClaim->paid < $expected * 0.9) {
                event(new ClaimUnderpaid($internal, $eraClaim->paid, $expected, $eraClaimModel));
                $this->followupSvc->autoCreate($internal, 'underpayment',
                    "Paid {$eraClaim->paid} < expected {$expected}");
            }
        }
    }

    private function processAdjustment(Claim $claim, EraClaim $era, array $adj): void
    {
        match($adj['group_code']) {
            'CO' => $this->writeOffContractual($claim, $era, $adj),
            'PR' => null,
            'OA' => $this->writeOffOther($claim, $era, $adj),
            'PI' => $this->flagPayerInitiated($claim, $era, $adj),
            default => null,
        };
    }
}
```

**Critical correctness points:**

1. **Math reconciliation**: Every ERA claim is checked: `paid + adjustments + patient_resp = billed`. If not, it throws and goes to exceptions (never silently posts wrong data).
2. **Group code routing**: CO → write-off, PR → patient balance, OA → other write-off, PI → flag for review.
3. **CARC/RARC pair**: Each adjustment has a CARC (reason) and optional RARC (remark). Both are stored.
4. **Matching strategy**: Match by (patient_id, dos, billed_amount). If ambiguous, manual review.
5. **Idempotency**: Same `trace_number + claim_id` posted twice → no duplicate transaction (UNIQUE constraint on `gateway_response.trace_claim_id`).
6. **Denial auto-followup**: A denied claim automatically creates a `claim_followups` row with the denial reason and a 15-day due date.
7. **Underpayment detection**: 10% threshold (configurable per payer; some payers underpay 5% routinely).
8. **Audit trail**: Every ERA posting creates an audit log entry with the full payload.
9. **S3 storage**: Raw 835 file stored in S3 with encryption and 7-year retention (HIPAA).

**Performance:**
- 1,000-claim ERA file posts in ~30 seconds.
- 10,000-claim ERA file in ~5 minutes.
- Run in background as `PostEraFileJob` (queue: `billing`).

### 24.5 Deep Dive: Patient Cost Estimation

Showing the patient an accurate cost estimate before the visit.

```php
namespace App\Modules\Billing\Services;

use App\Modules\Billing\Models\Patient;
use App\Modules\Billing\Models\InsurancePolicy;
use App\Modules\Billing\Models\ContractedRate;
use App\Modules\Billing\Models\EligibilityRequest;
use App\Modules\Eligibility\Services\EligibilityService;

class CostEstimationService
{
    public function __construct(
        private readonly EligibilityService $eligibility,
        private readonly ContractedRateRepository $contracted,
        private readonly ChargemasterService $chargemaster,
    ) {}

    public function estimate(
        Patient $patient,
        InsurancePolicy $policy,
        array $serviceCodes,
        \DateTimeInterface $serviceDate,
    ): CostEstimate {
        $benefits = $this->eligibility->getOrFetch($patient, $policy, $serviceDate);

        $services = [];
        $totalBilled = Money::zero($policy->payer->defaultCurrency());
        $totalAllowed = Money::zero($policy->payer->defaultCurrency());
        $totalPatientPays = Money::zero($policy->payer->defaultCurrency());
        $totalInsurancePays = Money::zero($policy->payer->defaultCurrency());

        $runningDedRemaining = $benefits['deductible_individual'] - $benefits['deductible_met'];
        $runningOopRemaining = $benefits['oop_max'] - $benefits['oop_met'];

        foreach ($serviceCodes as $cpt) {
            $allowed = $this->contracted->lookup($policy->payer, $cpt, $serviceDate)
                ?? $this->chargemaster->lookup($cpt, $serviceDate);

            $billed = $this->chargemaster->lookup($cpt, $serviceDate);
            $copay = $this->getCopayForCpt($cpt, $benefits, $policy);
            $coinsurance = $benefits['coinsurance'];

            $dedApplied = min($allowed->minor, $runningDedRemaining);
            $afterDed = $allowed->minor - $dedApplied;
            $coinsAmt = (int) round($afterDed * $coinsurance);
            $totalBeforeCap = $dedApplied + $coinsAmt + $copay;
            $patientPays = min($totalBeforeCap, $runningOopRemaining);
            $insurancePays = $allowed->minor - $patientPays;

            $services[] = new ServiceLineEstimate(
                cptCode: $cpt,
                description: $this->chargemaster->describe($cpt),
                billed: $billed,
                allowed: $allowed,
                insurancePays: new Money($insurancePays, $allowed->currency),
                patientPays: new Money($patientPays, $allowed->currency),
                deductibleApplied: new Money($dedApplied, $allowed->currency),
                coinsuranceApplied: new Money($coinsAmt, $allowed->currency),
                copayApplied: new Money($copay, $allowed->currency),
            );

            $totalBilled = $totalBilled->add($billed);
            $totalAllowed = $totalAllowed->add($allowed);
            $totalPatientPays = $totalPatientPays->add(new Money($patientPays, $allowed->currency));
            $totalInsurancePays = $totalInsurancePays->add(new Money($insurancePays, $allowed->currency));

            $runningDedRemaining = max(0, $runningDedRemaining - $dedApplied);
            $runningOopRemaining = max(0, $runningOopRemaining - $patientPays);
        }

        $isGfeRequired = $this->isGfeRequired($patient, $policy, $totalPatientPays);

        return new CostEstimate(
            serviceDate: $serviceDate,
            services: $services,
            totals: new CostEstimateTotals(
                billed: $totalBilled,
                allowed: $totalAllowed,
                insurancePays: $totalInsurancePays,
                patientPays: $totalPatientPays,
            ),
            deductibleRemainingAfter: new Money($runningDedRemaining, $policy->payer->defaultCurrency()),
            oopRemainingAfter: new Money($runningOopRemaining, $policy->payer->defaultCurrency()),
            isGoodFaithEstimateRequired: $isGfeRequired,
            disclaimers: $this->buildDisclaimers($policy, $isGfeRequired),
        );
    }

    private function isGfeRequired(Patient $patient, InsurancePolicy $policy, Money $totalPatient): bool
    {
        // US No Surprises Act: GFE required for uninsured/self-pay if estimate ≥ $400
        // or for scheduled services more than 3 business days in the future
        if (!$policy || $policy->is_self_pay) {
            return $totalPatient->isPositive()
                && $totalPatient->greaterThan(Money::fromMajor(400, 'USD'));
        }
        return false;
    }

    private function buildDisclaimers(InsurancePolicy $policy, bool $gfeRequired): array
    {
        $disclaimers = [
            "Estimate based on benefits as of " . now()->format('Y-m-d') . ". Final cost may vary.",
            "Additional services during the visit may incur additional charges.",
        ];
        if ($gfeRequired) {
            $disclaimers[] = "This is a Good Faith Estimate as required by the No Surprises Act. ";
            $disclaimers[] = "If you are billed substantially more than this estimate, you may dispute the bill.";
        }
        return $disclaimers;
    }
}
```

**Critical correctness points:**

1. **OOP max cap**: A patient can never pay more than their remaining OOP max in a year. Critical for ACA/Medicare patients.
2. **Deductible ordering**: Deductible is applied first, then coinsurance. Copay is per-visit and additive.
3. **Cumulative running balance**: As we walk through services, the remaining deductible/OOP shrinks.
4. **No OON double-dipping**: Out-of-network services use UCR (Usual, Customary, Reasonable) or chargemaster, not the in-network rate.
5. **GFE threshold**: $400+ for uninsured → must issue a GFE. Required by US No Surprises Act since 1/1/2022.
6. **Cache friendly**: Eligibility cached 24h; contracted rates cached 24h; chargemaster cached 24h. Total cost of an estimate after cache: <50ms.
7. **Disclaimer required**: Every estimate includes 2-3 disclaimers. Legal requirement.
8. **Audit trail**: Every estimate is logged (no PHI, just the request and result), so we can prove the estimate was given.

**Edge cases handled:**
- Patient has both Medicare and a Medicare Supplement (Medigap): Medicare pays primary, Medigap covers the rest, patient pays $0.
- Patient has multiple insurance policies (COB - Coordination of Benefits): apply primary, then secondary.
- Patient has met OOP max: all covered services are $0.
- Service is not covered at all (CARC 50): estimate shows full charge as patient responsibility.
- Patient has sliding scale discount: applied after insurance estimate.

### 24.6 Deep Dive: 837 EDI Generation

The 837 Professional (837P) is the standard format for submitting professional medical claims electronically.

```php
namespace App\Modules\Billing\Edi;

use App\Modules\Billing\Models\Claim;
use App\Modules\Billing\Models\Provider;
use App\Modules\Billing\Models\Patient;

class Edi837Generator
{
    public const IMPLEMENTATION_GUIDE = '005010X222A1';  // ASC X12 5010

    public function generateProfessional(Claim $claim, Provider $billing, Provider $rendering, ?Provider $referring): string
    {
        $segments = [];
        $segmentCount = 0;
        $seg = function (string $name, array $elements) use (&$segments, &$segmentCount) {
            $segments[] = $name . '*' . implode('*', array_map(
                fn($e) => str_replace(['*', '~', ':'], ['\*', '\~', '\:'], (string)$e),
                $elements
            ));
            $segmentCount++;
        };

        // Envelope
        $seg('ISA', [
            '00',          // Authorization Info Qualifier
            str_repeat(' ', 10),
            '00',          // Security Info Qualifier
            str_repeat(' ', 10),
            'ZZ',          // Interchange ID Qualifier
            str_pad($billing->isa_sender_id ?? 'SUBMITTERID', 15),
            'ZZ',          // Interchange ID Qualifier
            str_pad($claim->payer->isa_receiver_id ?? 'RECEIVERID', 15),
            now()->format('ymd'),
            now()->format('Hi'),
            '^',           // Repetition Separator
            '00501',       // Version
            $this->interchangeControlNumber(),
            '0',           // Ack Requested
            'P',           // Usage Indicator (P=Production, T=Test)
            ':',           // Component Separator
        ]);

        $stCount = 0;
        $transactionCount = 0;

        // Functional Group Header
        $gsControl = $this->groupControlNumber();
        $seg('GS', [
            'HC',          // Functional ID (Health Care Claim)
            $billing->isa_sender_id ?? 'SUBMITTER',
            $claim->payer->isa_receiver_id ?? 'RECEIVER',
            now()->format('Ymd'),
            now()->format('Hi'),
            $gsControl,
            'X',           // Responsible Agency Code
            self::IMPLEMENTATION_GUIDE,
        ]);

        // Transaction Set Header
        $stControl = $this->transactionControlNumber();
        $seg('ST', ['837', $stControl, self::IMPLEMENTATION_GUIDE]);

        // Beginning of Hierarchical Transaction
        $seg('BHT', [
            '0019',        // Hierarchical Structure Code
            '00',          // Transaction Set Purpose Code
            $claim->claim_number,
            now()->format('Ymd'),
            now()->format('Hi'),
            'CH',          // Claim or Encounter (CH=Chargeable)
        ]);

        // 1000A: Submitter Name
        $seg('NM1', [
            '41',          // Submitter
            '2',           // Entity Type Qualifier (2=Non-Person)
            $billing->organization_name,
            '', '', '', '',
            '46',          // ID Code Qualifier (46=ETIN)
            $billing->isa_sender_id ?? 'SUBMITTERID',
        ]);
        $seg('PER', ['IC', $billing->contact_name, 'TE', $billing->contact_phone]);

        // 1000B: Receiver Name
        $seg('NM1', [
            '40',          // Receiver
            '2',
            $claim->payer->organization_name,
            '', '', '', '',
            '46',          // ID Code Qualifier
            $claim->payer->isa_receiver_id ?? 'RECEIVERID',
        ]);

        // 2000A: Billing Provider Hierarchical Level
        $seg('HL', ['1', '', '20', '1']);
        $seg('PRV', ['BI', 'PXC', $billing->taxonomy_code]);
        $seg('NM1', [
            '85',          // Billing Provider
            '2',
            $billing->organization_name,
            '', '', '', '',
            'XX',          // NPI Qualifier
            $billing->npi,
        ]);
        $seg('N3', [$billing->address_line_1, $billing->address_line_2]);
        $seg('N4', [$billing->city, $billing->state, $billing->zip]);
        $seg('REF', ['EI', $billing->ein]);

        // 2000B: Subscriber Hierarchical Level
        $seg('HL', ['2', '1', '22', '0']);

        $sbr = $claim->subscriberRelation === 'self' ? 'P' : ($claim->subscriberRelation === 'spouse' ? 'S' : 'C');
        $seg('SBR', [
            $sbr,          // Payer Responsibility Sequence
            '18',          // Self or Other (18=Self)
            $claim->policy->group_number ?? '',
            '', '', '',
            '', '',         // Other insuring info
            'CI',          // Claim Filing Indicator (CI=Commercial, MB=Medicare, MC=Medicaid)
        ]);

        $seg('NM1', [
            'IL',          // Insured/Subscriber
            '1',           // Person
            $claim->subscriber_last_name,
            $claim->subscriber_first_name,
            '', '', '',
            'MI',          // Member ID
            $claim->policy->policy_number,
        ]);
        $seg('N3', [$claim->subscriber_address_line_1]);
        $seg('N4', [$claim->subscriber_city, $claim->subscriber_state, $claim->subscriber_zip]);
        $seg('DMG', ['D8', $claim->subscriber_dob->format('Ymd'), $claim->subscriber_gender]);

        // 2010BA: Payer Name
        $seg('NM1', [
            'PR',          // Payer
            '2',
            $claim->payer->organization_name,
            '', '', '', '',
            'PI',          // Payer ID
            $claim->payer->payer_id,
        ]);

        // 2300: Claim Loop
        $seg('CLM', [
            $claim->claim_number,
            (string)($claim->total_charge_minor / 100),
            '',            // Place of Service (separate REF)
            '',            // Frequency (separate REF)
            '11:B:1',      // Place of Service : Provider Signature : Assignment of Benefits
            'Y',           // Benefits Assignment
            'A',           // Release of Information
            'Y',           // Provider Signature on File
        ]);

        if ($referring) {
            $seg('NM1', [
                'DN',          // Referring Provider
                '1',
                $referring->last_name,
                $referring->first_name,
                '', '', '',
                'XX',
                $referring->npi,
            ]);
        }

        $seg('NM1', [
            '82',          // Rendering Provider (if different from billing)
            $rendering->is_person ? '1' : '2',
            $rendering->last_name ?? $rendering->organization_name,
            $rendering->first_name ?? '',
            '', '', '',
            'XX',
            $rendering->npi,
        ]);

        // 2400: Service Line Loop (one per CPT)
        $dxPointers = $claim->diagnosisPointers();
        $dxIndex = 0;
        foreach ($claim->serviceLines as $line) {
            $seg('LX', [(string)++$dxIndex]);

            // SV1: Professional Service
            $seg('SV1', [
                'HC:' . $line->cpt_code . ($line->modifiers ? ':' . implode(':', $line->modifiers) : ''),
                (string)($line->charge_minor / 100),
                'UN',         // Unit of Measure (UN=Unit)
                (string)$line->units,
                '',           // Place of Service
                '',           // Diagnosis Pointer (set per claim)
                $line->modifiers && $line->modifiers[0] === '25' ? '1' : '0',
            ]);

            $seg('DTP', ['472', 'D8', $line->service_date->format('Ymd')]);

            // HI: Health Care Diagnosis Info (ICD-10)
            foreach ($claim->diagnoses as $i => $dx) {
                if ($i >= 12) break;
                $seg('HI', ['ABK:' . $dx->code]);
            }
        }

        // Transaction Set Trailer
        $seg('SE', [(string)$segmentCount, $stControl]);
        $seg('GE', ['1', $gsControl]);
        $seg('IEA', ['1', $this->interchangeControlNumber()]);

        return implode("~\n", $segments) . "~\n";
    }

    private function interchangeControlNumber(): string
    {
        // 9-digit unique per submission
        return str_pad((string)Cache::increment('edi:isa:icn', 1, 1), 9, '0', STR_PAD_LEFT);
    }
}
```

**Critical correctness points:**

1. **Element separators**: `*` for elements, `:` for components, `~` for segments. Escaped properly.
2. **ISA line length**: Exactly 106 characters (including the trailing `~`). Each element is space-padded to a fixed width.
3. **Segment count in SE**: Must match the actual segment count from ST to SE (inclusive, exclusive of ST/SE themselves? inclusive of ST, exclusive of SE; depends on the implementation guide).
4. **Hierarchical levels (HL)**: 2000A (Billing Provider) → 2000B (Subscriber) → 2300 (Claim) → 2400 (Service Line). Each HL has a parent.
5. **NM1 segments**: 1000A (Submitter), 1000B (Receiver), 2010AA (Billing Provider), 2010AB (Pay-To Provider), 2010BA (Subscriber), 2010BB (Payer), 2310A (Referring), 2310B (Rendering), 2420A (Rendering for service line), etc.
6. **NPI in NM109**: For individuals, NM108='XX' (NPI Qualifier), NM109=10-digit NPI validated with Luhn.
7. **Taxonomy in PRV**: Billing Provider's taxonomy code (e.g., 207Q00000X for family medicine).
8. **ICD-10 in HI segment**: ABK qualifier for primary diagnosis, ABF for secondary. Up to 12 diagnoses per claim.
9. **CPT in SV101**: HC:CPT[:MOD1[:MOD2[:MOD3[:MOD4]]]]. Up to 4 modifiers.
10. **Charge in SV102**: Dollar amount with 2 decimal places (no thousands separator).
11. **Service date in DTP**: 472 = Service Date, D8 = Date format CCYYMMDD.
12. **Total charge in CLM02**: Sum of all service line charges for this claim.

**Truncation and trailing chars:**
- Each line ends with `~` + line terminator (CRLF or LF or `\n`).
- Empty fields are represented by `*` next to `*` (e.g., `CLM*ABC*100***11:B:1*Y*A*Y*Y~`).

**Test verification:**
- Parse the generated 837 with `EdiParser` and verify round-trip correctness.
- Validate ISA/IEA envelope pair (control number, segment count).
- Validate GS/GE pair.
- Validate ST/SE pair (transaction set control number, segment count).
- Validate HL hierarchy (4 levels: Provider > Subscriber > Claim > Service Line).
- Validate mandatory segments: BHT, NM1*41, NM1*40, HL*1, NM1*85, SBR, NM1*IL, NM1*PR, CLM.

**Error correction:**
- A 999 (Implementation Acknowledgment) reports syntax errors (segment/element level).
- A 277CA (Claim Acknowledgment) reports claim-level errors (business rules).
- Both must be parsed and stored for audit.

---

## 25. V3+ Precise Plan — Billing

> 6-phase rollout from "invoice module" to "complete RCM platform" over 24 months. Each phase has clear deliverables, team, dependencies, risks, and metrics.

### 25.1 Phase 1: V1 Invoice Module (Months 0-3, already done)

**Status:** ✅ Complete (this document's sections 1-20)

**Deliverables:** 8 tables, ~50 endpoints, 4 services, 11 acceptance criteria met. Already supports auto-invoicing, taxes, coupons, refunds, cash recording, reports.

### 25.2 Phase 2: V2 Subscriptions + Insurance + RCM Basics (Months 4-9, already done)

**Status:** ✅ Complete (this document's sections 21.1-21.9)

**Deliverables:**
- `subscription_plans`, `subscriptions`, `subscription_invoices`, `subscription_items` (4 tables)
- `packages`, `patient_packages` (2 tables)
- `insurance_payers`, `insurance_policies`, `insurance_preauth_requests`, `insurance_claims` (4 tables)
- `denials`, `dunning_letters`, `credit_notes`, `payment_plans`, `payment_plan_installments` (5 tables)
- `chain_revenue_rules`, `revenue_distributions` (2 tables)
- `usage_records` (1 table)
- `e_invoices` (1 table)
- ~50 new V2 endpoints (this document's §21.8)
- 9 acceptance criteria (this document's §21.9)

**Team:** 3 backend engineers, 1 QA, 1 designer (mockups for insurance flows)

### 25.3 Phase 3: V3 Eligibility + Claim Submission + ERA Posting (Months 10-15)

**Goal:** Real-time eligibility, electronic claim submission, automated ERA posting — the heart of US RCM.

**Deliverables:**
- 6 new tables (`ar_aging_buckets`, `claim_followups`, `claim_submissions`, `claim_acknowledgments`, `eligibility_requests`, `era_files`, `era_claims`)
- `EligibilityService` with 270/271 parser
- `ClaimSubmissionService` with 837P/837I/837D generator
- `EraPostingService` with 835 parser and auto-posting
- `ClaimScrubbingService` with 30 rules
- Clearinghouse integrations: Availity, Change Healthcare, Waystar, Office Ally, NHIC (India)
- ~30 new V3 endpoints (this document's §22.20)
- 10 acceptance criteria (this document's §22.4-22.5)

**Team:** 5 backend engineers (2 EDI specialists, 1 PHP, 2 general), 2 QA (clearinghouse sandboxes), 1 devops (Redis tuning, queue workers)

**Dependencies:**
- Phase 1, 2 complete
- Chargemaster schema designed
- 5 clearinghouse sandbox accounts set up
- EDI 837/835 parser library chosen (in-house vs. `EDICompanion` or `simplified-php-edi`)

**Risks:**
- **R1**: Clearinghouse sandbox limitations (some don't return realistic 835s) → fallback: in-house mock 835 generator
- **R2**: 837 generation edge cases (COB, multi-payer) → multiple payer-specific test claims
- **R3**: Scrubbing rules need to be configurable per payer → config-based rule loading
- **R4**: ERA matching is fuzzy → ML-based matching for ambiguous cases
- **R5**: High volume of 999/277CA processing → queue + async handlers

**Metrics:**
- Eligibility: p95 < 2s, 99% success rate
- Claim submission: p95 < 1s (excluding clearinghouse roundtrip)
- ERA posting: 1000 claims/30s, 99.9% accuracy
- Scrubbing: 80% of denials caught pre-submission
- Days in A/R: target <40 days

**Cost:** $320k (5 backend × 6 months × $10k/mo blended) + clearinghouse fees (~$5k/mo) + sandbox setup (~$10k one-time)

### 25.4 Phase 4: V3 Chargemaster + Cost Estimation + Dunning (Months 16-21)

**Goal:** Hospital-grade chargemaster, real-time cost estimates, automated dunning.

**Deliverables:**
- 7 new tables (`chargemaster_items`, `contracted_rates`, `cost_accounting_entries`, `good_faith_estimates`, `dunning_letters` (refactored), `collections_agency_assignments`)
- `ChargemasterService` with CMS-0057-F compliance
- `CostEstimationService` with insurance + OON + GFE
- `GoodFaithEstimateService` with PDF + e-signature
- `DunningService` with multi-channel cascade
- `PropensityScoreService` (ML model)
- `BankReconciliationService` (CSV/OFX import)
- ~25 new V3 endpoints
- 12 acceptance criteria

**Team:** 4 backend engineers, 1 ML engineer (propensity model), 2 QA, 1 designer (dunning templates)

**Dependencies:**
- Phase 3 complete
- Chargemaster data import utility (CSV parser)
- ML model training data (12 months of patient payment history)
- 5+ dunning template translations (en, es, fr, hi, ar)

**Risks:**
- **R1**: Chargemaster data quality (many hospitals have messy CDMs) → data cleansing service + manual review
- **R2**: Cost estimate accuracy is hard → conservative defaults, big disclaimers
- **R3**: Dunning skip rules are complex (legal nuances per state) → legal review + state-specific config
- **R4**: ML propensity model needs labeled training data → bootstrap with rules-based scoring first
- **R5**: Bank file formats vary widely → flexible parser with format detection

**Metrics:**
- Cost estimate accuracy: 90% within 10% of actual
- GFE acceptance rate: 80% of uninsured patients
- Dunning effectiveness: 60% collected within 60 days
- Propensity model AUC: 0.75+

**Cost:** $400k (4 backend × 6 months × $10k + 1 ML × 6 × $15k)

### 25.5 Phase 5: V3 VBC + Hospital Billing + AI Coding (Months 22-27)

**Goal:** Value-based care, hospital-specific billing, AI assistance for coders.

**Deliverables:**
- 8 new tables (`capitation_contracts`, `capitation_payments`, `hcc_risk_scores`, `vbc_quality_measures`, `vbc_measure_aggregates`, `drg_assignments`, `apc_assignments`, `anesthesia_records`)
- `HccRafService` (HCC v24 + v28)
- `BpcIAdvancedService` (90-day episode aggregation)
- `MipsSubmissionService` (QPP JSON API)
- `DrgGrouperService` (CMS MS-DRG)
- `ApcGrouperService` (CMS APC)
- `AnesthesiaBillingService` (base + time units)
- `ImplantTrackingService` (UDI capture)
- `AiCodingService` (OpenAI, Anthropic, Ollama)
- `FraudDetectionService` (anomaly detection)
- `DocImprovementService` (RAF gap analysis)
- ~40 new V3 endpoints
- 15 acceptance criteria

**Team:** 5 backend engineers (2 ML/AI, 1 health policy, 2 general), 1 medical coder (consultant), 1 QA, 1 clinical informaticist (consultant)

**Dependencies:**
- Phase 4 complete
- HCC mapping data (CMS publishes annually)
- DRG grouper (CMS publishes; possibly license 3M)
- AI service ready (Python vLLM deployment, OpenAI/Anthropic keys)
- Medical coder for test cases and validation

**Risks:**
- **R1**: DRG grouping is complex and changes annually → use CMS grouper; update yearly
- **R2**: HCC model version migration (v24 → v28) → migration script + backfill
- **R3**: AI coding accuracy is bounded (60-80% top-1 accuracy) → always human-in-the-loop
- **R4**: AI fraud detection has false positives → conservative thresholds + manual review
- **R5**: VBC contract modeling is complex → start with capitation + P4P, defer bundled payments to V4

**Metrics:**
- AI coding accuracy: 75% top-1, 90% top-3
- RAF gap closure: 30% of identified gaps documented
- MIPS score: 80+ average across providers
- Capitation reconciliation: 100% accuracy
- DRG grouping: 99% accuracy on test set

**Cost:** $620k (5 backend × 6 × $10k + 1 ML × 6 × $15k + consultants ~$60k)

### 25.6 Phase 6: V3 Multi-Entity + Statements + Reports + Compliance (Months 28-33)

**Goal:** Multi-entity group reporting, full statement engine, custom report builder, full compliance suite.

**Deliverables:**
- 6 new tables (`legal_entities`, `intercompany_invoices`, `statements`, `statement_preferences`, `custom_reports`, `compliance_audits`)
- `MultiEntityService` (intercompany + consolidation)
- `StatementService` (paperless-first, QR codes, paper fallback)
- `ConsolidationService` (multi-currency, elimination)
- `CustomReportBuilder` (drag-and-drop saved reports)
- `FapApplicationService` (charity care workflow)
- `SlidingFeeScaleService` (FPL-based discount)
- `CollectionsAgencyService` (assignment + recovery)
- `PositivePayService` (check fraud prevention)
- `AchReturnService` (NSF handling)
- `LockboxParser` (mail payments)
- 45+ pre-built reports
- 8 live dashboards
- ~50 new V3 endpoints
- 18 acceptance criteria

**Team:** 4 backend engineers, 1 data engineer (ETL for consolidation), 1 designer (dashboards, statement PDF), 1 QA, 1 legal/compliance consultant

**Dependencies:**
- Phase 5 complete
- ERP integration (optional; for cost accounting data)
- Bank lockbox file format
- Federal Poverty Level data (HHS publishes annually)
- HIPAA legal review

**Risks:**
- **R1**: Multi-entity consolidation is accounting-grade → hire senior accountant consultant
- **R2**: Statement PDF design takes iterations → 3-4 design rounds
- **R3**: Custom report builder is complex → ship with 30 pre-built reports first; custom builder in iteration 2
- **R4**: FAP eligibility screening touches protected data → legal review + privacy impact assessment
- **R5**: Collections agency integration is jurisdiction-specific → start with US only

**Metrics:**
- Statement email open rate: 40%+
- Patient portal adoption: 60% of active patients
- Custom report usage: 50% of admin users
- Multi-entity consolidation: 100% accuracy, runs in <1 hour
- FAP turnaround: 14 days median

**Cost:** $450k (4 backend × 6 × $10k + 1 data × 6 × $12k + 1 designer × 4 × $8k + consultants ~$50k)

### 25.7 Cross-Cutting Concerns (All Phases)

**Security & Compliance:**
- HITRUST CSF certification by V3 GA (Month 12)
- SOC 2 Type II by Month 18
- HIPAA audit annually
- PCI-DSS Level 1 by Month 6 (already in V2)

**Performance:**
- Report queries p95 < 1s for 1M rows
- Cost estimate p95 < 200ms
- ERA posting 1000 claims/30s
- A/R aging dashboard p95 < 500ms

**Observability:**
- All financial mutations logged to `audit_log` (append-only)
- All webhook events logged
- All EDI transactions logged with S3 archive
- Prometheus metrics: invoice.created, payment.received, claim.submitted, claim.denied, era.posted
- Grafana dashboards per service
- PagerDuty integration for high-value errors

**Disaster Recovery:**
- RPO 5 minutes (continuous WAL archiving)
- RTO 1 hour (multi-region active-passive)
- Daily backup verification (restore to staging)
- Quarterly DR drill (failover to secondary region)

**Data Retention:**
- Financial transactions: 7 years (HIPAA)
- Patient PHI: 6 years post-last-visit
- Audit logs: 7 years
- Backups: 90 days
- Archive to S3 Glacier after 1 year for cost

**Localization:**
- en, hi, es, fr, de, pt, ar, zh-CN, ja
- Number formatting per locale (lakh, crore, Arabic numerals)
- Date/time per locale
- Tax rules per country
- Currency per tenant

### 25.8 Rollout & Risk Register

| Risk | Phase | Probability | Impact | Mitigation |
|---|---|---|---|---|
| Clearinghouse sandbox limitations | 3 | High | Medium | In-house mock 835 generator; manual test plan |
| DRG grouper licensing costs | 5 | Medium | High | Use CMS open-source grouper; license only if needed |
| AI coding accuracy too low | 5 | Medium | High | Always human-in-the-loop; conservative rollout |
| Multi-entity accounting errors | 6 | Medium | High | Hire senior accountant consultant; extensive UAT |
| Chargemaster data quality | 4 | High | Medium | Data cleansing utility; manual review queue |
| Cost estimate legal liability | 4 | Low | High | Conservative estimates; legal disclaimers; insurance |
| HIPAA breach | All | Low | Critical | HITRUST certification; encryption; access controls |
| Payer API changes | 3, 5 | High | Medium | Versioned adapters; automated integration tests |
| Dunning legal compliance (state) | 4 | Medium | Medium | Legal review; configurable per-state rules |
| FAP PII exposure | 6 | Low | High | Encryption at rest; access controls; audit trail |

### 25.9 Key Milestones

| Month | Milestone |
|---|---|
| 0 | V1 invoice module GA |
| 4 | V2 subscriptions GA |
| 6 | V2 insurance GA |
| 9 | V2 RCM basics GA |
| 12 | V3 eligibility + claim submission GA |
| 15 | V3 ERA auto-posting GA |
| 18 | V3 chargemaster + cost estimate GA |
| 21 | V3 dunning + bank recon GA |
| 24 | V3 VBC + hospital billing GA |
| 27 | V3 AI coding + RAF GA |
| 30 | V3 multi-entity + statements GA |
| 33 | V3 full RCM platform GA |
| 36 | V3 stable; V4 begins (claim status tracking, FTE automation) |

### 25.10 Success Metrics (V3+ Billing)

- **Revenue:** $5M+ ARR by V3 GA (Month 12); $20M+ ARR by V3+ stable (Month 33)
- **Customers:** 1,000+ clinics, 50+ hospitals by Month 33
- **Customer outcomes:**
  - Days in A/R <40 (industry avg 50)
  - Net collection rate >96% (industry avg 95%)
  - Denial rate <5% (industry avg 10%)
  - Clean claim rate >95% (industry avg 75%)
  - Cost-to-collect <4% (industry avg 6%)
- **Engineering metrics:**
  - 99.95% uptime
  - p95 API <300ms
  - 0 critical security incidents
  - <0.1% data loss in backups

(End of file)
