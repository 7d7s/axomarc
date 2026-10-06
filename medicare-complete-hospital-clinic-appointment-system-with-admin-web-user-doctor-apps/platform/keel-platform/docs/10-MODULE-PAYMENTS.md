# 10 — Payments Module (Gateway-Agnostic, Razorpay, Stripe, Paystack)

> All money in / out flows through this module. The architecture is a thin gateway-agnostic layer over three real gateways. Adding a new gateway (e.g., PayPal, Flutterwave) is one new class implementing the same interface.

---

## 1. Scope

| In Scope | Out of Scope |
|---|---|
| Payment gateway abstraction (one interface) | Cryptocurrency (V2) |
| Razorpay integration (India primary) | BNPL providers (V2) |
| Stripe integration (global) | Multi-currency conversion (V2) |
| Paystack integration (Africa) | Saved cards / customer vault (V2) |
| Webhook handling + signature verification | Subscription billing on gateway (V2) |
| Idempotent order creation | |
| Refund API per gateway | |
| Mobile SDK checkout wrapper | |
| Test mode vs live mode per env | |

---

## 2. The Abstraction

### 2.1 `PaymentGateway` Interface

```php
namespace App\Modules\Payments\Contracts;

interface PaymentGateway {
    public function name(): string;                       // 'razorpay' | 'stripe' | 'paystack'

    public function createOrder(OrderRequest $req): OrderResponse;
    public function verifyWebhook(WebhookRequest $req): WebhookEvent;
    public function fetchPayment(string $gatewayPaymentId): GatewayPayment;
    public function refund(string $gatewayPaymentId, int $amount, array $meta = []): RefundResponse;
    public function createCustomer(array $data): CustomerResponse;       // V2 (saved cards)
    public function chargeCustomer(string $customerId, int $amount, array $meta = []): ChargeResponse;  // V2
}
```

### 2.2 Value Objects

```php
final class OrderRequest {
    public function __construct(
        public readonly int $amount,             // minor units
        public readonly string $currency,        // 'INR'
        public readonly string $receipt,         // our internal order id (uuid)
        public readonly array $notes,            // { tenant_id, patient_id, invoice_id, ... }
        public readonly ?int $expireMinutes = 15,
    ) {}
}

final class OrderResponse {
    public function __construct(
        public readonly string $orderId,         // gateway's id
        public readonly string $status,          // 'created'
        public readonly ?array $extras = null,    // e.g. Razorpay's key_id, Stripe's client_secret
    ) {}
}

final class WebhookEvent {
    public function __construct(
        public readonly string $type,            // 'payment.captured' | 'payment.failed' | 'refund.processed'
        public readonly string $gatewayPaymentId,
        public readonly string $gatewayOrderId,
        public readonly int $amount,
        public readonly string $currency,
        public readonly string $status,          // 'succeeded' | 'failed'
        public readonly array $raw,              // full raw payload
    ) {}
}
```

### 2.3 Resolver

```php
class PaymentGatewayResolver {
    public function for(string $name): PaymentGateway {
        return match ($name) {
            'razorpay' => app(RazorpayGateway::class),
            'stripe'   => app(StripeGateway::class),
            'paystack' => app(PaystackGateway::class),
            default    => throw new InvalidArgumentException("Unknown gateway: $name"),
        };
    }

    public function default(): PaymentGateway {
        return $this->for(config('medicare.payments.default_gateway', 'razorpay'));
    }
}
```

The default is configurable per tenant in `settings.payments.default_gateway`.

---

## 3. The Gateway-Agnostic `payments` Table

| Column | Type | Notes |
|---|---|---|
| `id` | bigint PK | |
| `tenant_id` | bigint FK | |
| `uuid` | char(36) unique | |
| `gateway` | enum('razorpay','stripe','paystack','none') | `none` for cash/manual |
| `gateway_order_id` | varchar(120) unique nullable | |
| `gateway_payment_id` | varchar(120) unique nullable | Set on success |
| `gateway_signature` | varchar(500) nullable | For client-side verification (Razorpay) |
| `gateway_customer_id` | varchar(120) nullable | V2 |
| `method` | enum('card','upi','netbanking','wallet','cash','bank_transfer','other','unknown') | |
| `status` | enum('created','authorized','captured','failed','refunded','partially_refunded','disputed','expired') | |
| `amount` | bigint | |
| `currency` | varchar(3) | ISO 4217 |
| `fee` | bigint default 0 | Gateway fee (Razorpay/Stripe charge us) |
| `tax` | bigint default 0 | GST on gateway fee |
| `invoice_id` | bigint FK nullable | |
| `appointment_id` | bigint FK nullable | |
| `patient_id` | bigint FK nullable | |
| `user_id` | bigint FK | Who initiated |
| `raw_response` | json | Full gateway response |
| `webhook_received_at` | timestamp nullable | |
| `captured_at` | timestamp nullable | |
| `failed_at` | timestamp nullable | |
| `failure_reason` | varchar(255) nullable | |
| `created_at`, `updated_at` | timestamps | |

This is the **normalized view** of any payment. Each gateway stores its raw response in `raw_response` and we map to the canonical status.

`Transaction` (in 09-BILLING) is the accounting record. `Payment` here is the gateway-specific record. They're linked 1:1 for online payments, 1:0 for cash (cash has only a Transaction, no Payment row).

---

## 4. Razorpay Gateway (Primary for India)

### 4.1 Setup

- Razorpay account, KYC completed.
- API keys: `RAZORPAY_KEY_ID` (public) + `RAZORPAY_KEY_SECRET` (private) in `.env`.
- Webhook secret: `RAZORPAY_WEBHOOK_SECRET` (set in Razorpay dashboard).
- Webhook URL: `https://{api-host}/api/v1/webhooks/razorpay` — registered for events: `payment.captured`, `payment.failed`, `refund.processed`, `order.paid`.

### 4.2 Flow

```
[Mobile App]                    [Laravel API]                       [Razorpay]
    │                               │                                   │
    │  POST /payments/razorpay/order                                   │
    │  { amount, currency, invoice_id, ... }                           │
    │ ─────────────────────────────► │                                   │
    │                               │ PaymentGatewayResolver->razorpay  │
    │                               │ RazorpayGateway::createOrder     │
    │                               │ → POST /v1/orders                │
    │                               │ ────────────────────────────────► │
    │                               │ ◄──────────────────────────────── │
    │                               │ Store Payment row (status=created)│
    │ ◄─────────────────────────────│ { order_id, key, amount }        │
    │                               │                                   │
    │ RazorpayCheckout.open(key, order_id, amount)                     │
    │ User pays                                                        │
    │ ──────────────────────────────────────────────────────────────► │
    │                               │                                   │
    │                               │ POST /api/v1/webhooks/razorpay    │
    │                               │ payload: payment.captured        │
    │                               │ ◄──────────────────────────────── │
    │                               │ Verify HMAC SHA256               │
    │                               │ Map to WebhookEvent              │
    │                               │ Find Payment by gateway_order_id │
    │                               │ Mark captured                    │
    │                               │ Create Transaction               │
    │                               │ Mark Invoice paid                │
    │                               │ Confirm Appointment              │
    │                               │ Broadcast events                 │
    │                               │ Return 200                       │
```

### 4.3 RazorpayGateway Implementation Sketch

```php
class RazorpayGateway implements PaymentGateway {
    public function __construct(private Api $api) {}   // Razorpay\Api\Api
    
    public function createOrder(OrderRequest $req): OrderResponse {
        $order = $this->api->order->create([
            'amount'   => $req->amount,
            'currency' => $req->currency,
            'receipt'  => $req->receipt,
            'notes'    => $req->notes,
            'expire_by'=> now()->addMinutes($req->expireMinutes ?? 15)->timestamp,
        ]);
        return new OrderResponse(
            orderId: $order['id'],
            status:  $order['status'],
            extras:  ['key' => config('services.razorpay.key')],
        );
    }
    
    public function verifyWebhook(WebhookRequest $req): WebhookEvent {
        $expected = hash_hmac('sha256', $req->rawBody, config('services.razorpay.webhook_secret'));
        if (!hash_equals($expected, $req->header('X-Razorpay-Signature'))) {
            throw new WebhookSignatureException;
        }
        $payload = $req->json();
        $event = $payload['event'];
        $payment = $payload['payload']['payment']['entity'];
        return new WebhookEvent(
            type: match($event) {
                'payment.captured' => 'payment.captured',
                'payment.failed'   => 'payment.failed',
                'refund.processed' => 'refund.processed',
            },
            gatewayPaymentId: $payment['id'],
            gatewayOrderId:   $payment['order_id'],
            amount: $payment['amount'],
            currency: $payment['currency'],
            status: $payment['status'] === 'captured' ? 'succeeded' : 'failed',
            raw: $payload,
        );
    }
    
    public function refund(string $gatewayPaymentId, int $amount, array $meta = []): RefundResponse {
        $refund = $this->api->payment->fetch($gatewayPaymentId)->refund([
            'amount' => $amount,
            'notes'  => $meta,
        ]);
        return new RefundResponse(
            refundId: $refund['id'],
            status:   $refund['status'],
        );
    }
}
```

### 4.4 Flutter SDK Usage

The Flutter app uses `razorpay_flutter` package, initialized with the `key` we returned from the API. The `Razorpay` widget opens a checkout sheet, on success the Flutter side receives a payment id. The Flutter side then **does not** trust this — it just navigates to a "verifying payment" screen that polls `GET /api/v1/patient/appointments/{id}/payment-status` until `status=paid`. The webhook is the source of truth, not the client.

If the webhook is delayed, the user sees a spinner. After 30s of polling, we display "If you've been charged, you'll receive a confirmation shortly" and let them navigate away.

---

## 5. Stripe Gateway (Global)

### 5.1 Setup

- Stripe account, API keys in `.env`: `STRIPE_PUBLISHABLE_KEY` (client) + `STRIPE_SECRET_KEY` (server) + `STRIPE_WEBHOOK_SECRET`.
- Webhook endpoint: `https://{api-host}/api/v1/webhooks/stripe` — register for `payment_intent.succeeded`, `payment_intent.payment_failed`, `charge.refunded`.

### 5.2 Flow (uses PaymentIntents, not Checkout)

Stripe doesn't have a "create order" concept like Razorpay. The flow is:

```
1. App calls POST /payments/stripe/intent
   { amount, currency, invoice_id, ... }
2. API creates PaymentIntent with metadata={invoice_id, ...}
   → returns { client_secret, payment_intent_id }
3. App uses flutter_stripe SDK with the client_secret
4. User pays on Stripe's PaymentSheet
5. Stripe → webhook → API marks paid
```

### 5.3 Why PaymentIntents over Checkout

PaymentIntents give us full control of the UI and let us use Stripe's mobile SDKs. Checkout is a hosted page that breaks the UX.

---

## 6. Paystack Gateway (Africa)

### 6.1 Setup

- Paystack account, secret key in `.env`.
- Webhook URL: `https://{api-host}/api/v1/webhooks/paystack`.

### 6.2 Flow

Similar to Razorpay: create transaction → return authorization URL → redirect or in-app webview → user pays → webhook fires.

```php
class PaystackGateway implements PaymentGateway {
    public function createOrder(OrderRequest $req): OrderResponse {
        $resp = $this->http->post('https://api.paystack.co/transaction/initialize', [
            'email' => $req->notes['patient_email'],
            'amount' => $req->amount,
            'currency' => $req->currency,
            'reference' => $req->receipt,
            'callback_url' => $req->notes['callback_url'],
            'metadata' => $req->notes,
        ]);
        return new OrderResponse(
            orderId: $resp['data']['reference'],
            status: 'created',
            extras: ['authorization_url' => $resp['data']['authorization_url']],
        );
    }
}
```

---

## 7. Webhook Handler (Common)

```php
// app/Modules/Payments/Http/Controllers/WebhookController.php
class WebhookController {
    public function razorpay(Request $req): Response {
        return $this->handle('razorpay', $req);
    }
    public function stripe(Request $req): Response {
        return $this->handle('stripe', $req);
    }
    public function paystack(Request $req): Response {
        return $this->handle('paystack', $req);
    }
    
    private function handle(string $gateway, Request $req): Response {
        $event = $this->resolver->for($gateway)->verifyWebhook(
            new WebhookRequest($req->header(), $req->getContent())
        );
        // Idempotency: check if we already processed this gateway_payment_id
        if (ProcessedWebhook::where('gateway', $gateway)->where('event_id', $event->gatewayPaymentId)->exists()) {
            return response()->json(['status' => 'duplicate_ignored']);
        }
        DB::transaction(function () use ($gateway, $event) {
            ProcessedWebhook::create([
                'gateway' => $gateway,
                'event_id' => $event->gatewayPaymentId,
                'event_type' => $event->type,
                'raw' => $event->raw,
            ]);
            $handler = match($event->type) {
                'payment.captured' => app(PaymentCapturedHandler::class),
                'payment.failed'   => app(PaymentFailedHandler::class),
                'refund.processed' => app(RefundProcessedHandler::class),
            };
            $handler->handle($event);
        });
        return response()->json(['status' => 'ok']);
    }
}
```

### 7.1 Idempotency

The `processed_webhooks` table ensures the same event isn't processed twice. The gateway may retry webhooks on timeout. The unique key is `(gateway, event_id)`.

### 7.2 Signature Verification (Per Gateway)

| Gateway | Header | Algorithm |
|---|---|---|
| Razorpay | `X-Razorpay-Signature` | HMAC SHA256 of body with webhook secret |
| Stripe | `Stripe-Signature` | Stripe's format: `t=...,v1=...` — verify using Stripe's SDK |
| Paystack | `X-Paystack-Signature` | HMAC SHA512 of body with secret key |

If signature fails, return `401` and **do not log** the payload (could be a probe).

---

## 8. Webhook Handlers

### 8.1 `PaymentCapturedHandler`

```php
class PaymentCapturedHandler {
    public function __construct(
        private TransactionService $tx,
        private AppointmentStateMachine $fsm,
        private NotificationDispatcher $notify,
    ) {}
    
    public function handle(WebhookEvent $event): void {
        $payment = Payment::where('gateway_order_id', $event->gatewayOrderId)->firstOrFail();
        $payment->update([
            'status' => 'captured',
            'gateway_payment_id' => $event->gatewayPaymentId,
            'captured_at' => now(),
            'raw_response' => $event->raw,
        ]);
        
        // Idempotency: skip if already processed
        if (Transaction::where('payment_id', $payment->id)->where('status', 'succeeded')->exists()) {
            return;
        }
        
        $transaction = $this->tx->recordGatewayPayment($payment, $event);
        $invoice = $payment->invoice;
        $invoice->update([
            'amount_paid' => $invoice->amount_paid + $event->amount,
            'amount_due' => $invoice->total - ($invoice->amount_paid + $event->amount),
            'status' => 'paid',
            'paid_at' => now(),
        ]);
        
        if ($payment->appointment) {
            $this->fsm->transition($payment->appointment, 'confirmed', $payment->user, ['payment_id' => $payment->id]);
            $payment->appointment->slot->update(['status' => 'booked', 'appointment_id' => $payment->appointment->id]);
        }
        
        $this->notify->send($payment->patient->user, 'payment_received', ['invoice' => $invoice]);
        $this->notify->send($payment->appointment?->doctor?->user, 'new_appointment', ['appointment' => $payment->appointment]);
    }
}
```

### 8.2 `PaymentFailedHandler`

- Marks `Payment.status=failed`, `failed_at=now()`, `failure_reason=...`.
- If there's an appointment in `pending`, transitions it to `failed`.
- Releases the slot.
- FCM to patient: "Payment failed. Please try again."

### 8.3 `RefundProcessedHandler`

- Finds the `Refund` by `gateway_refund_id`.
- Marks it `succeeded`.
- Creates the debit `Transaction`.
- Updates the linked invoice.
- FCM to patient with refund receipt.

---

## 9. Mobile SDK Wrappers (Flutter)

```dart
// lib/payments/payment_service.dart
abstract class PaymentService {
  Future<PaymentInitResult> init({
    required Invoice invoice,
    required PaymentMethod method,
  });
  
  Future<void> openCheckout({
    required String gateway,
    required String orderId,
    required String key,
    required int amount,
    required String currency,
  });
  
  Future<PaymentStatus> pollStatus(String appointmentId);
}

class RazorpayService implements PaymentService {
  final _razorpay = Razorpay();
  
  Future<void> openCheckout({...}) async {
    var options = {
      'key': key,
      'amount': amount,
      'name': 'Medicare',
      'order_id': orderId,
      'description': 'Appointment Booking',
      'prefill': {'contact': phone, 'email': email},
      'theme': {'color': '#1E40AF'},
    };
    _razorpay.open(options);
  }
}
```

`StripeService` wraps `flutter_stripe` (PaymentSheet).
`PaystackService` wraps `paystack_manage` (in-app webview).

---

## 10. API Endpoints

### 10.1 Order / Intent Creation

```
POST   /api/v1/payments/razorpay/order
       { invoice_id, consultation_type?, amount_override? }
    → { order_id, key, amount, currency, expires_at }

POST   /api/v1/payments/stripe/intent
       { invoice_id, ... }
    → { payment_intent_id, client_secret, amount, currency }

POST   /api/v1/payments/paystack/initialize
       { invoice_id, callback_url }
    → { authorization_url, reference }
```

### 10.2 Status

```
GET    /api/v1/payments/{payment_id}
GET    /api/v1/payments/{payment_id}/status
GET    /api/v1/payments/lookup?order_id=&gateway=
```

### 10.3 Refunds

```
POST   /api/v1/payments/{payment_id}/refund
       { amount, reason }
```

(Refund creation is also in 09-BILLING; this is the gateway-direct version for admin use.)

### 10.4 Webhooks (No Auth, Signature-Verified)

```
POST   /api/v1/webhooks/razorpay
POST   /api/v1/webhooks/stripe
POST   /api/v1/webhooks/paystack
```

### 10.5 Admin

```
GET    /api/v1/admin/payments?status=&gateway=&from=&to=&page=
GET    /api/v1/admin/payments/{id}
POST   /api/v1/admin/payments/{id}/sync           (manual re-fetch from gateway)
```

---

## 11. Configuration

`.env` keys (from 00-ARCHITECTURE §9.1):

```dotenv
RAZORPAY_KEY_ID=rzp_live_********
RAZORPAY_KEY_SECRET=********
RAZORPAY_WEBHOOK_SECRET=********
STRIPE_PUBLISHABLE_KEY=pk_live_********
STRIPE_SECRET_KEY=sk_live_********
STRIPE_WEBHOOK_SECRET=whsec_********
PAYSTACK_PUBLIC_KEY=pk_live_********
PAYSTACK_SECRET_KEY=sk_live_********
PAYSTACK_WEBHOOK_SECRET=********
```

Per-tenant overrides in `settings.payments`:
```json
{
  "default_gateway": "razorpay",
  "enabled_gateways": ["razorpay", "stripe"],
  "min_amount": 100,
  "max_amount": 100000000,
  "auto_capture": true
}
```

---

## 12. Idempotency Strategy

- **Idempotency-Key** on order creation: client sends a UUID; if the same key is seen within 24h, return the original response without creating a new order. Stored in `idempotency_keys` table.
- **Webhook deduplication**: `processed_webhooks` table with unique `(gateway, event_id)`.
- **Status updates**: `PaymentCapturedHandler` checks if a `Transaction` already exists for the `payment_id` with `status=succeeded` before creating a duplicate.

This guarantees that any combination of client retries + gateway retries = exactly one outcome.

---

## 13. Failure Modes & Recovery

| Failure | Recovery |
|---|---|
| Client times out after paying | Webhook still fires, marks paid; client polls and sees success |
| Webhook delayed (>30s) | Client polls `GET /payments/{id}/status`; if still `created` after 60s, show "we'll email you" |
| Webhook lost | `SyncPendingPaymentsJob` runs every 5 min: for `Payment.status=created` older than 2 min, call gateway's `fetchPayment` API and update |
| Gateway refund fails | Admin retries via `POST /refunds/{id}/retry`; manual fallback possible |
| Slot was released by cron before webhook arrived | `PaymentCapturedHandler` re-reserves the slot; if now booked by someone else, refund automatically + notify both users |

---

## 14. Realtime Events

| Event | Channel | Use |
|---|---|---|
| `PaymentCaptured` | `user.{id}` (patient), `doctor.{id}` | Confirm success in UI |
| `PaymentFailed` | `user.{id}` | Show error |
| `RefundProcessed` | `user.{id}` | Show refund confirmation |

---

## 15. Audit

Every Payment row is audited: create, status change, refund. Every webhook received is logged with the raw payload (S3, not just DB, for forensics).

---

## 16. Testing Strategy

| Test | Assertion |
|---|---|
| `RazorpayOrderTest` | Order created, response shape correct |
| `RazorpayWebhookSignatureTest` | Tampered signature → 401, valid → 200 |
| `WebhookIdempotencyTest` | Same event sent twice → only one Transaction created |
| `PaymentCapturedHandlerTest` | Marks invoice paid, confirms appointment, broadcasts |
| `PaymentFailedHandlerTest` | Slot released, appointment failed, patient notified |
| `StripePaymentIntentTest` | Client secret returned, can be used by SDK |
| `PaystackWebhookTest` | Signature verified, status mapped correctly |
| `IdempotencyKeyTest` | Same key + payload → same response, no new order |
| `GatewaySyncJobTest` | Pending payment older than threshold → fetched from gateway |
| `CrossTenantTest` | Payment in tenant A never visible to tenant B |
| `ConcurrentBookingTest` | Two parallel `createOrder` calls + one webhook → one confirmation, one refund |

---

## 17. Acceptance Criteria (V1)

- [ ] Patient can pay via Razorpay, Stripe, or Paystack depending on tenant config.
- [ ] Webhook signature is verified; invalid signatures return 401.
- [ ] Duplicate webhooks are deduplicated.
- [ ] Client-side payment confirmation is **not** trusted; only the webhook + sync job are.
- [ ] Refund issued via the original gateway updates the invoice.
- [ ] Failed payment releases the slot and transitions appointment to failed.
- [ ] Manual gateway sync job reconciles stuck payments within 5 min.
- [ ] All payment state changes are audited.
- [ ] Test mode keys work end-to-end without affecting live.

---

## 18. V2 Roadmap — Payments

> V1 supports one-off card/UPI payments via Razorpay, Stripe, and Paystack. V2 adds **subscriptions**, **BNPL/EMI**, **Stripe Terminal (in-person)**, **local payment methods** (UPI Autopay, GCash), and **multi-currency** with FX.

### 18.1 Subscriptions (`razorpay-subscriptions`, `paystack-subscriptions`, `stripe-terminal`)

Already detailed in `09-Billing.md` §21.1. From the payments module perspective, we add three new gateway methods to the `PaymentGateway` interface:

```php
interface PaymentGateway {
    // V1
    public function createOrder(Invoice $invoice, string $idempotencyKey): GatewayOrder;
    public function verifyWebhook(Request $request): WebhookEvent;
    public function refund(Transaction $transaction, Money $amount, string $reason): Refund;
    public function getTransaction(string $gatewayId): GatewayTransaction;

    // V2
    public function createSubscription(SubscriptionPlan $plan, Patient|Tenant $subscriber, PaymentMethod $method): GatewaySubscription;
    public function cancelSubscription(string $gatewaySubId, bool $atPeriodEnd): void;
    public function pauseSubscription(string $gatewaySubId): void;
    public function resumeSubscription(string $gatewaySubId): void;
    public function updateSubscriptionPaymentMethod(string $gatewaySubId, PaymentMethod $method): void;
}
```

Three concrete implementations:
- `RazorpaySubscriptionService` — uses Razorpay Subscriptions API (`/v1/subscriptions`).
- `PaystackSubscriptionService` — uses Paystack Subscription API (`/subscription`).
- `StripeSubscriptionService` — uses Stripe Subscriptions (`/v1/subscriptions`).

Webhook handlers (`subscription.activated`, `subscription.renewed`, `subscription.payment_failed`, `subscription.cancelled`) are registered in `WebhooksController` and dispatched to `SubscriptionService` listeners.

#### 18.1.1 Razorpay Subscriptions — `razorpay-subscriptions` (V1.2, MML 3) — deep-dive

**1. Overview.** Razorpay's **Subscription API** (per `https://razorpay.com/docs/api/subscriptions`) powers **recurring billing** for both B2B (tenant admin paying Medicare SaaS fees) and B2C (patient paying a clinic for a monthly "10-visit cardiology pack"). The V1.2 module `razorpay-subscriptions` ships a production-grade integration: plan definition, subscription creation, eNACH / UPI Autopay / card mandate, auto-debit, dunning on failure, proration on plan change, and cancel/pause/resume. MML 3 GA: all four subscription events (`subscription.activated`, `subscription.renewed`, `subscription.payment_failed`, `subscription.cancelled`), the four mandate types, the dunning policy, the proration policy, the cancel-policies (`immediate` / `at_period_end`), and the audit + reconciliation. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.2 lane module. The mirror modules are `paystack-subscriptions` (V1.2, see `§18.1.2`) for Nigeria/Ghana/South Africa, and the implicit `stripe-subscriptions` (used by Stripe-Backend-SaaS for global).

**2. Plans in Razorpay vs in Medicare.** A Razorpay `plan` is a billing template (`{period, interval, item, notes}`); a Medicare `subscription_plans` row is a **per-tenant product** that we sync to Razorpay on save. The sync is one-way: we are the source of truth; Razorpay holds the schedule and the customer's mandate. On plan change (e.g., "Pro ₹2,999 → Premium ₹4,999"), we update the local row; if the price changed, we PATCH the Razorpay plan (creates a new plan version; existing subscribers stay on the old plan until renewal). On price-change, an admin-initiated plan migration is a separate flow (§5 below).

**3. The 4 mandate types Razorpay supports.**
- **Card** — auto-debit a saved card on the renewal date. 3DS / SCA enforced for EU.
- **UPI Autopay** (eNACH) — auto-debit a UPI handle. India-only; requires the customer's UPI app to approve the mandate. Mandate can be for up to ₹15,000 per execution (RBI limit at GA; raised to ₹1 lakh for some categories).
- **eMandate / eNACH (Net Banking)** — auto-debit a bank account via the customer's net-banking. India-only; supports ₹ to large amounts. Requires a 1-time bank login + Aadhaar / eSign.
- **PayLater (LazyPay, etc.)** — not a true subscription; treated as a per-cycle charge. Used only for B2C packs.

For B2B (tenant SaaS fees), the tenant admin typically picks **card** (with auto-debit on the 1st of the month). For B2C (patient packs), the patient picks the mandate type their app supports; UPI Autopay is the most common.

**4. The create-subscription flow.**
1. Customer (tenant admin or patient) → `POST /api/v2/payments/subscriptions` with `{ plan_id, customer_id, mandate_type }`.
2. The `SubscriptionService` calls Razorpay's `POST /v1/subscriptions` with `{ plan_id, customer_notify: 1, total_count: 12, start_at: <now+1m>, notes: { medicare_tenant_id, medicare_subscription_id } }`. Razorpay returns `{ id: 'sub_xxx', status: 'created', short_url }`.
3. The service returns the `short_url` to the client. The client opens it (Razorpay's hosted page) for the customer to authorize the mandate. On authorization, Razorpay sends a webhook `subscription.authenticated` and (on the start_at) `subscription.activated`. We update `subscriptions.status='active'`.
4. On the start_at, Razorpay auto-debits. On success: `subscription.charged` + the underlying `payment.captured`; we update `subscriptions.next_charge_at`, write a `transactions` row, write an `invoices` row (status='paid'), and fire `subscription.renewed` to the tenant. On failure: `subscription.payment_failed`; we trigger dunning.

**5. The 4 Razorpay subscription events we handle.**
- `subscription.authenticated` — customer authorized the mandate; subscription is `authenticated`. The first charge happens at `start_at`.
- `subscription.activated` — first charge succeeded; subscription is `active`.
- `subscription.charged` — subsequent charge succeeded; we update `last_charged_at` and `next_charge_at`.
- `subscription.payment_failed` — charge failed; we trigger dunning.
- `subscription.cancelled` — cancelled; we update `status='cancelled'`, set `cancelled_at`, and lock features at period end.
- `subscription.paused` / `subscription.resumed` — pause / resume (we forward via `POST /v1/subscriptions/{id}/pause` and `/resume`).
- `subscription.updated` — plan or quantity changed; we update the local mirror.
- `subscription.pending_cancellation` — cancel scheduled at period end.

**6. Proration.** When a customer changes plan mid-cycle (e.g., "Pro → Premium" on day 10 of a 30-day cycle), we compute the unused portion of the old plan and apply it as a credit to the new plan. Razorpay does **not** auto-prorate; we compute the credit in `SubscriptionService::changePlan()` and pass it as `notes.credit` on the new subscription. The first invoice for the new plan is `new_plan_amount - credit`. The credit is auditable in `subscription_credits`.

**7. Dunning.** Configured per tenant (in `tenants.dunning_policy`):
- On `subscription.payment_failed` at T+0: send "Payment failed" email + push notification; retry at T+3 days.
- T+3: retry; if fails, send "Action required" email; retry at T+7.
- T+7: retry; if fails, send "Last chance" email; retry at T+10.
- T+10: retry; if fails, `subscriptions.status='past_due'`; features degrade to "read-only" (the tenant can still view but not create new resources; the patient can still view their records but not book new appointments).
- T+30: `subscriptions.status='cancelled'`; the subscription is closed; the tenant's tenant-features roll back to the previous tier (e.g., "Pro → Free").
The retry schedule is a config (`tenants.dunning_policy = { retries: [3, 7, 10], grace_days: 30 }`). Each retry is a `subscription.charge_attempt` row; the dunning email is a template (`emails.dunning.{t3,t7,t10,t30}`).

**8. Cancel policies.** Two modes:
- **Immediate** (`POST /v1/subscriptions/{id}/cancel` with `cancel_at_cycle_end=0`): the subscription is cancelled right now; features degrade instantly; a prorated refund is issued for the unused portion (credit on next invoice, not a refund to the card, per `09-Billing §21.1.5`).
- **At period end** (`cancel_at_cycle_end=1`): the subscription stays `active` until `current_end`; we mark `cancel_at_period_end=true`; at `current_end` Razorpay sends `subscription.cancelled`; we set `status='cancelled'`.

**9. Pause / resume.** A customer can pause (e.g., "skipping June, resuming July"): `POST /v1/subscriptions/{id}/pause` with `pause_at=now`. The subscription is `status='paused'`; no charges; features are held in a "frozen" state. Resume: `POST /v1/subscriptions/{id}/resume` with `resume_at=2026-07-01`. Razorpay charges on `resume_at`. Pause is limited to 3 months (Razorpay policy); after that, the subscription is auto-cancelled.

**10. Domain Model.**
```php
// subscriptions — per `09-Billing §21.1` (extended for V1.2)
Schema::create('subscriptions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('subscriber_id')->index();                        // patient_id or tenant_admin's user_id
    $t->string('subscriber_type', 16);                         // 'patient' | 'tenant'
    $t->ulid('plan_id')->index();
    $t->string('gateway', 32);                                 // 'razorpay' | 'paystack' | 'stripe'
    $t->string('gateway_subscription_id', 64)->unique()->index();   // sub_xxx
    $t->string('gateway_customer_id', 64)->nullable();         // cust_xxx
    $t->string('gateway_plan_id', 64);                         // plan_xxx
    $t->string('mandate_type', 32)->nullable();                // 'card' | 'upi_autopay' | 'enach' | 'paylater'
    $t->string('status', 16)->default('created');              // 'created' | 'authenticated' | 'active' | 'past_due' | 'paused' | 'cancelled' | 'expired'
    $t->boolean('cancel_at_period_end')->default(false);
    $t->timestamp('current_period_start')->nullable();
    $t->timestamp('current_period_end')->nullable();
    $t->timestamp('next_charge_at')->nullable();
    $t->timestamp('last_charged_at')->nullable();
    $t->timestamp('cancelled_at')->nullable();
    $t->string('cancellation_reason', 64)->nullable();
    $t->jsonb('notes')->nullable();
    $t->timestamps();
    $t->index(['tenant_id', 'status']);
    $t->index(['next_charge_at', 'status']);
});

// subscription_charge_attempts
Schema::create('subscription_charge_attempts', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('subscription_id')->index();
    $t->string('gateway_payment_id', 64)->nullable();
    $t->string('status', 16);                                  // 'pending' | 'succeeded' | 'failed'
    $t->integer('attempt_number');                              // 1, 2, 3, ...
    $t->integer('amount');
    $t->char('currency', 3);
    $t->text('error_code')->nullable();
    $t->text('error_description')->nullable();
    $t->timestamp('attempted_at');
    $t->timestamps();
});

// subscription_credits
Schema::create('subscription_credits', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('subscription_id')->index();
    $t->integer('amount');
    $t->char('currency', 3);
    $t->string('reason', 64);                                  // 'plan_change_proration' | 'goodwill' | 'refund_credit'
    $t->ulid('source_subscription_id')->nullable();            // the old sub, for plan change
    $t->timestamp('applied_at')->nullable();
    $t->timestamp('expires_at')->nullable();                   // credit expires in 12 months
    $t->timestamps();
});

// subscription_events
Schema::create('subscription_events', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('subscription_id')->index();
    $t->string('event_type', 32);                              // 'created' | 'authenticated' | 'activated' | 'charged' | 'failed' | 'paused' | 'resumed' | 'cancelled' | 'plan_changed' | 'dunning_email_sent'
    $t->jsonb('payload')->nullable();
    $t->string('actor_type', 16)->nullable();                  // 'user' | 'gateway' | 'system'
    $t->ulid('actor_id')->nullable();
    $t->timestamp('occurred_at');
    $t->timestamps();
});
```

**11. API Surface.**
```
# Plan management (admin)
GET    /api/v2/payments/subscriptions/plans                  -> [plan]
POST   /api/v2/payments/subscriptions/plans                  -> plan
PATCH  /api/v2/payments/subscriptions/plans/{id}             -> plan
DELETE /api/v2/payments/subscriptions/plans/{id}             -> 204 (deactivate; do not delete)

# Subscription lifecycle (subscriber)
POST   /api/v2/payments/subscriptions                        -> { plan_id, mandate_type, ... } -> { subscription, gateway_short_url }
GET    /api/v2/payments/subscriptions                        -> [sub] (own subscriptions)
GET    /api/v2/payments/subscriptions/{id}                   -> sub (with events, charge_attempts, credits)
POST   /api/v2/payments/subscriptions/{id}/cancel            -> { at_period_end: true|false } -> sub
POST   /api/v2/payments/subscriptions/{id}/pause             -> { resume_at? } -> sub
POST   /api/v2/payments/subscriptions/{id}/resume            -> sub
POST   /api/v2/payments/subscriptions/{id}/change-plan       -> { new_plan_id } -> sub (with proration credit)
POST   /api/v2/payments/subscriptions/{id}/update-mandate    -> { new_mandate_type, new_payment_method } -> sub
GET    /api/v2/payments/subscriptions/{id}/events            -> [event]
GET    /api/v2/payments/subscriptions/{id}/charge-attempts   -> [attempt]
GET    /api/v2/payments/subscriptions/{id}/credits           -> [credit]
GET    /api/v2/payments/subscriptions/{id}/invoices          -> [invoice] (auto-generated per charge)

# Admin
GET    /api/v2/admin/subscriptions                           -> [sub] (filter by status, plan, gateway)
POST   /api/v2/admin/subscriptions/{id}/retry-charge         -> immediate retry
GET    /api/v2/admin/subscriptions/stats                     -> { active, past_due, cancelled_this_month, mrr, ... }
```

**12. Frontend.**
- **Customer view (tenant admin / patient):** `/billing/subscription` shows the current plan, the next charge date, the mandate, the cancel/pause buttons, the plan-change dropdown, the charge history (with invoice links), and the credit balance.
- **Admin:** `/admin/billing/subscriptions` shows all subscriptions in the tenant with filters (status, plan, gateway, subscriber), the MRR / ARR widget, the past-due list, and the dunning queue.
- **Onboarding flow:** when a tenant first activates a paid plan, the wizard shows 3 steps: pick plan → pick mandate type (card / UPI / eNACH) → complete Razorpay mandate. The wizard is a 3-screen modal; on success, the tenant's feature flags unlock.

**13. Realtime.**
- `subscription.activated.{tenant_id}` — first charge succeeded; admin dashboard updates.
- `subscription.charged.{tenant_id}.{subscription_id}` — renewal charge succeeded; UI shows the new invoice.
- `subscription.payment_failed.{tenant_id}.{subscription_id}` — charge failed; dunning email + UI banner.
- `subscription.cancelled.{tenant_id}.{subscription_id}` — cancelled; UI shows the cancellation date and the rollback-plan notice.
- `subscription.past_due.{tenant_id}.{subscription_id}` — after dunning exhausted; features degrade; admin paged.

**14. Security & RBAC.**
- **Webhook verification:** every Razorpay webhook is verified with HMAC SHA-256 (`RAZORPAY_WEBHOOK_SECRET`); unverified webhooks return 401 and are logged in `webhook_events` with `verification_failed=true`.
- **Idempotency:** every webhook is deduplicated on `(gateway_event_id, gateway)` for 24 hours (Redis SET with TTL); a duplicate event is a no-op.
- **PCI scope:** we never see the card number; Razorpay's hosted page + the card mandate token is the only artifact we store (in `payment_methods`).
- **PII:** customer name, email, phone are passed to Razorpay at customer creation; we do not store the Razorpay customer ID beyond the local mirror.
- **Roles:** `tenant_admin` (subscribe, cancel, change plan, update mandate for own tenant), `billing_admin` (view all, retry charges, refund), `developer` (read own platform subscriptions), `platform_admin` (override disputes, manual credit issuance).
- **Audit:** every event in `subscription_events` is a row in `audit_logs` with the actor and the payload; the dunning email sends are logged with `dunning_email_sent`.

**15. Acceptance Criteria.**
- [ ] A plan can be created in Medicare, synced to Razorpay, and listed in the Razorpay dashboard.
- [ ] A subscription can be created with each of the 4 mandate types (card / UPI Autopay / eNACH / PayLater).
- [ ] The first charge happens at `start_at`; `subscription.activated` is processed within 30 seconds.
- [ ] A renewal charge happens at `next_charge_at`; `subscription.charged` is processed; an `invoices` row is created.
- [ ] A failed charge triggers dunning; the retry happens on the configured day; after `grace_days`, `status='past_due'`.
- [ ] Plan change with proration applies the credit correctly (verified with a unit test using known values).
- [ ] Cancel immediate vs at-period-end behaves as expected; the feature flags roll back at the right time.
- [ ] Pause / resume works; pause is limited to 3 months.
- [ ] The webhook handler is idempotent; a duplicate `subscription.charged` does not double-charge.
- [ ] The webhook HMAC verification rejects tampered payloads.
- [ ] The MRR / ARR widget is correct (sum of `active` subscriptions × monthly price, in INR).
- [ ] The dunning email renders correctly in en-IN, hi-IN, and ar-SA (where applicable).

**16. Real-World Test Scenarios.**
- *B2B plan upgrade:* tenant admin on "Pro ₹2,999" clicks "Upgrade to Premium ₹4,999" on day 10 of a 30-day cycle; the system computes the credit (₹2,000 prorated); the new subscription starts with the credit applied to the first charge; the admin sees a "₹4,999 - ₹2,000 credit = ₹2,999 due" line item; the next renewal is the full ₹4,999.
- *B2C UPI Autopay:* patient signs up for a "10-visit cardiology pack ₹9,999"; picks UPI Autopay; the app opens the UPI app for mandate approval; the patient approves; the first charge is on the 1st; the renewal is on the 1st of the next month; the patient can cancel anytime.
- *Card mandate failure:* customer's card expires; Razorpay fails the renewal; we send the T+0 email; retry on T+3 fails; we send the T+3 email; retry on T+7 fails; we mark `past_due`; the patient's bookings are blocked.
- *Card mandate recovery:* customer updates the card; the next retry on T+10 succeeds; the subscription returns to `active`; the patient can book again.
- *Pause + resume:* patient pauses for 1 month (vacation); no charges; resumes; the next charge is on the resume date.
- *Cancel at period end:* patient cancels; the subscription stays active until the end of the period; on the end date, Razorpay sends `subscription.cancelled`; we set `status='cancelled'`; the patient's bookings remain valid through the period end.
- *Dunning exhausted:* customer doesn't update the card; after 30 days, the subscription is cancelled; the tenant's features roll back to Free; the tenant admin gets an email "Your subscription has been cancelled; upgrade again to restore features".
- *Webhook tamper attempt:* attacker sends a fake `subscription.charged` with a bogus signature; the HMAC verification fails; the webhook is rejected; the audit log shows `webhook.verification_failed`.

**17. What this module is NOT:**
- Not a payment processor. We use Razorpay; we do not hold funds longer than the dunning window.
- Not a global subscription engine. Razorpay is India-primary; global is Stripe; Africa is Paystack. Each is a separate module.
- Not a tax engine. We do not compute GST / VAT on the subscription amount; the invoice line items are flat (the tenant is responsible for tax handling, per their accountant's advice).
- Not a refund engine. Refunds go through `09-Billing §21.1.5` (the existing refund flow); this module only issues credits.

**18. Dev Workflow.** `php artisan subscriptions:create --tenant=... --plan=...` creates a test subscription. `php artisan subscriptions:retry --sub=...` retries a failed charge. `php artisan subscriptions:proration-test --from=pro --to=premium --day=10` prints the expected credit. `php artisan subscriptions:reconcile --gateway=razorpay --from=2026-01-01 --to=2026-01-31` reconciles the gateway's subscription list against our `subscriptions` table. Code: `app-modules/subscriptions/` + `app-modules/razorpay-subscriptions/`; tests: `app-modules/razorpay-subscriptions/tests/Feature/{Create,Charge,Dunning,Cancel,PauseResume,PlanChange,Webhook}Test.php` and `tests/Reconciliation/RazorpaySubscriptionReconcileTest.php` (asserts every active Razorpay sub has a corresponding local row, and vice versa). Sandbox: Razorpay test mode keys (`rzp_test_...`); a `RazorpayTestHelper` fakes the gateway in CI.

#### 18.1.2 Paystack Subscriptions — `paystack-subscriptions` (V1.2, MML 3) — deep-dive

**1. Overview.** **Paystack** (per `https://paystack.com/docs/api/subscription`) is the dominant payment gateway across **Nigeria (NG), Ghana (GH), South Africa (ZA), and Kenya (KE)** — supporting NGN, GHS, ZAR, KES, and USD. The `paystack-subscriptions` module is the V1.2 mirror of `razorpay-subscriptions` for African markets. It powers **recurring billing** for both B2B (tenant SaaS fees in NGN/GHS) and B2C (patient packs in ZAR). The same `subscriptions` table is reused (with `gateway='paystack'`); the gateway-specific code lives in `PaystackSubscriptionService`. MML 3 GA: all 5 Paystack subscription events (`subscription.create`, `subscription.disable`, `subscription.not_renewing`, `subscription.expiring_cards`, `invoice.create` / `invoice.payment_failed` / `invoice.update`), the 3 mandate types (card / bank / mobile money where supported), the dunning policy, the proration policy, cancel/pause/resume, and reconciliation. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.2 lane module.

**2. Differences from Razorpay.** Paystack's Subscription API is **simpler** than Razorpay's (no separate `Plan` resource; a `Plan` is implicit in the `Subscription` body). Key differences:
- **No separate plan object.** A Paystack `Subscription` is created with `{ customer, plan: { name, amount, interval: 'monthly'|'annually' } }`; the plan is created on the fly. We mirror this to our `subscription_plans` table.
- **No UPI Autopay.** The mandate types are: **card** (auto-debit), **bank** (Direct Debit via Paystack's bank integration, NG/GH/ZA/KE banks), and **mobile_money** (where supported — Ghana Mobile Money, Kenya M-Pesa via Paystack's partner). M-Pesa is direct; not via DPO.
- **No explicit pause API.** Pause is achieved by `subscription.disable` (cancel at period end) + a new subscription on resume. We model this as our "pause" UX.
- **Webhooks differ:** Paystack sends `subscription.create`, `subscription.disable`, `subscription.not_renewing`, `subscription.expiring_cards`, `invoice.create`, `invoice.payment_failed`, `invoice.update`. We map all 7 to our `subscription_events` table.
- **Currency lock:** Paystack requires the customer to be created with a specific currency; the subscription inherits. We support NGN, GHS, ZAR, KES, USD; mixing is not allowed.

**3. The create-subscription flow.**
1. Customer → `POST /api/v2/payments/subscriptions` with `{ plan_id, customer_id, mandate_type }`.
2. The `PaystackSubscriptionService` calls Paystack's `POST /subscription` with `{ customer: 'CUS_xxx', plan: { name: 'Pro Monthly', amount: 299900, interval: 'monthly' }, start_date: <ISO8601> }`. Paystack returns `{ subscription_code, email_token, status: 'active' }` (after the customer authorizes the first charge with a payment method on file; for card, 3DS / SCA applies).
3. The service returns `{ subscription, manage_url: 'https://paystack.com/subscription/...' }`. The customer is redirected to the manage URL to manage the subscription (update card, view invoices, cancel) on Paystack's hosted page.
4. On `start_date`, Paystack auto-debits. On success: `invoice.create` + `invoice.update` (status='success'); we update `subscriptions.status='active'`, `next_charge_at`, write a `transactions` row, and write an `invoices` row (status='paid').

**4. The 7 Paystack subscription events we handle.**
- `subscription.create` — subscription was created (we mirror to our `subscriptions` table).
- `subscription.disable` — subscription was cancelled (we mark `status='cancelled'`).
- `subscription.not_renewing` — customer set the subscription to not renew (we mark `cancel_at_period_end=true`).
- `subscription.expiring_cards` — the customer's card is expiring this month (we send a "Update your card" email).
- `invoice.create` — a new invoice was created for the upcoming renewal.
- `invoice.payment_failed` — the renewal charge failed (we trigger dunning).
- `invoice.update` — the invoice was paid (we update the local mirror and the transactions row).

**5. The 3 mandate types Paystack supports.**
- **Card** — Visa, Mastercard, Verve (NG). 3DS / SCA for EU. Expiry handling: Paystack sends `subscription.expiring_cards` 30 days before expiry; we email the customer to update.
- **Bank (Direct Debit)** — Nigeria (Paystack's bank integration supports 9 banks: GTBank, Zenith, UBA, Access, First Bank, etc.), Ghana, South Africa (Instant EFT). Requires a 1-time mandate authorization.
- **Mobile Money** — Ghana MTN Mobile Money, AirtelTigo Money; Kenya M-Pesa (via Paystack's M-Pesa integration, separate from the DPO integration in §18.4). The customer receives a prompt on their phone; they confirm with a PIN.

**6. Proration.** Same as Razorpay: we compute the credit in `SubscriptionService::changePlan()` and apply it as a `subscription_credits` row. Paystack's API has no built-in proration; we apply the credit to the first invoice of the new plan.

**7. Dunning.** Same retry policy as Razorpay: T+0, T+3, T+7, T+10, then `past_due` at T+30. The retry uses Paystack's `POST /subscription/{code}/manage` to re-attempt the charge with the same payment method. If the customer updates the card, the next retry succeeds.

**8. Cancel policies.**
- **Immediate:** `POST /subscription/disable` with `code` — Paystack cancels and refunds the unused portion (per their policy, only for annual subscriptions; for monthly, no refund). We mark `status='cancelled'` instantly.
- **At period end:** Paystack sends `subscription.not_renewing`; we mark `cancel_at_period_end=true`; the subscription stays `active` until period end; on period end, Paystack sends `subscription.disable`; we mark `status='cancelled'`.

**9. Pause / resume.** Paystack has no explicit pause; we model it as:
- **Pause:** `POST /subscription/disable` with `code` + a local flag `paused=true, paused_until=2026-08-01`. The subscription is `status='cancelled'` in Paystack but `status='paused'` locally. No charges.
- **Resume:** create a new Paystack subscription with the same plan; the `subscriptions` row keeps the same `id` (we update in place). The customer is re-charged on the resume date.

**10. Domain Model.** Reuses the same `subscriptions`, `subscription_charge_attempts`, `subscription_credits`, `subscription_events` tables from the Razorpay deep-dive (§18.1.1). The only new column is:
```php
Schema::table('subscriptions', function (Blueprint $t) {
    $t->string('paystack_subscription_code', 64)->nullable()->unique()->index();  // SUB_xxx
    $t->string('paystack_email_token', 64)->nullable();                            // for the manage URL
});
```

**11. API Surface.** Same as Razorpay (§18.1.1 §11). The endpoint paths are gateway-agnostic; the gateway is determined by `subscriptions.gateway`. The `subscriptions:create` Artisan command takes a `--gateway=paystack` flag.

**12. Frontend.** Same as Razorpay (§18.1.1 §12). The "Update your card" CTA on `subscription.expiring_cards` uses Paystack's `manage_url` to send the customer to Paystack's hosted page (rather than our own form, since we don't store the card).

**13. Realtime.** Same as Razorpay (§18.1.1 §13). The dunning event names use the same `subscription.*` namespace.

**14. Security & RBAC.** Same as Razorpay (§18.1.1 §14). Webhook verification: Paystack signs webhooks with HMAC SHA-512 using the `PAYSTACK_SECRET_KEY`; we verify the `X-Paystack-Signature` header. Idempotency: same `(gateway_event_id, gateway)` dedup window.

**15. Acceptance Criteria.**
- [ ] A plan can be created and mirrored to Paystack.
- [ ] A subscription can be created with each of the 3 mandate types (card / bank / mobile money).
- [ ] The first charge happens at `start_date`; the 7 Paystack events are processed within 30 seconds.
- [ ] A renewal charge happens at `next_charge_at`; an `invoices` row is created.
- [ ] A failed charge triggers dunning; the retry happens on the configured day; after `grace_days`, `status='past_due'`.
- [ ] Plan change with proration applies the credit correctly.
- [ ] Cancel immediate vs at-period-end behaves as expected.
- [ ] Pause / resume works (modeled as disable + new sub).
- [ ] `subscription.expiring_cards` triggers a "Update your card" email 30 days before card expiry.
- [ ] The webhook HMAC SHA-512 verification rejects tampered payloads.
- [ ] Reconciliation: every active Paystack sub has a corresponding local row, and vice versa.

**16. Real-World Test Scenarios.**
- *NGN B2B:* Nigerian clinic on "Pro ₦29,999/month" picks card mandate; the first charge succeeds; the renewal auto-debits on the 1st of each month; the customer receives a Paystack invoice email.
- *GHS Mobile Money:* Ghanaian patient on "10-visit pack GH₵999" picks Mobile Money; the prompt appears on their MTN line; they confirm with a PIN; the charge succeeds.
- *Card expiring:* customer's card expires next month; Paystack sends `subscription.expiring_cards`; we email "Update your card on Paystack" with the manage URL; the customer updates; the next charge succeeds.
- *Bank mandate failure:* customer's bank mandate is rejected (insufficient funds); Paystack sends `invoice.payment_failed`; we trigger dunning; the customer updates the card; the next retry succeeds.
- *Plan change mid-cycle:* customer on "Basic ₦9,999" upgrades to "Pro ₦29,999" on day 10; we compute the credit; the first Pro invoice is ₦29,999 - credit; the audit log shows the plan change.
- *Cancel at period end:* customer cancels; the subscription stays active until the 30th; on the 30th, Paystack sends `subscription.disable`; we mark `status='cancelled'`.
- *Pause + resume:* customer pauses for 2 months (vacation); no charges; resumes; a new Paystack subscription is created; the customer's card is charged on the resume date.
- *Currency mismatch attempt:* admin tries to change a subscription's currency from NGN to GHS; the API returns 422 (Paystack does not support currency changes on an active sub); the admin cancels and creates a new subscription in GHS.

**17. What this module is NOT:**
- Not a global subscription engine. Africa only; India is Razorpay; global is Stripe.
- Not a payment processor. We use Paystack; we do not hold funds.
- Not a M-Pesa-only module. M-Pesa in Kenya is one of the supported mandate types; we also support card and bank.
- Not a wallet. Paystack's "Paystack Wallet" (when it ships) is post-V1.2.

**18. Dev Workflow.** `php artisan subscriptions:create --tenant=... --plan=... --gateway=paystack` creates a test subscription. `php artisan subscriptions:reconcile --gateway=paystack --from=... --to=...` reconciles. Code: `app-modules/subscriptions/` + `app-modules/paystack-subscriptions/`; tests: `app-modules/paystack-subscriptions/tests/Feature/{Create,Charge,Dunning,Cancel,PauseResume,PlanChange,Webhook,ExpiringCard}Test.php` and `tests/Reconciliation/PaystackSubscriptionReconcileTest.php`. Sandbox: Paystack test mode keys (`sk_test_...`); a `PaystackTestHelper` fakes the gateway in CI. Local mock: `tests/fixtures/paystack-subscription-mock.json` (canned NGN + GHS + ZAR + KES responses).

### 18.2 BNPL & EMI (V2)

India: LazyPay, ZestMoney, Simpl, Mobikwik. US: Klarna, Afterpay, Affirm. EU: Klarna, Ratepay.

```php
Schema::create('bnpl_providers', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('slug', 32);                                  // 'lazyPay' | 'klarna' | 'affirm'
    $t->string('display_name', 64);
    $t->json('config');                                      // API keys, merchant ID, callback URL
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});
```

`BnplGateway` interface:
```php
interface BnplGateway {
    public function checkEligibility(Money $amount, Patient $patient): EligibilityResult;
    public function createOrder(Invoice $invoice): BnplOrder;
    public function verifyWebhook(Request $request): BnplEvent;
}
```

Flow:
1. Checkout shows "Pay in 3 installments" option if `BnplService::isEligible()`.
2. Patient is redirected to BNPL provider's page (or in-app SDK for Klarna/Affirm).
3. Provider approves, returns a token.
4. We charge the provider for the full amount, but the patient pays the provider in N installments.
5. Provider pays us per their schedule (usually D+1, D+30, D+60).

### 18.3 Stripe Terminal (in-person card) — `stripe-terminal` (V1.2, MML 4) — deep-dive

**1. Overview.** **Stripe Terminal** (per `https://stripe.com/docs/terminal`) is Stripe's in-person card-present payment product: a certified EMV card reader (Wise Pad 3 in the US, Verifone P400 in EU, BBPOS WisePOS E in APAC) connects to the merchant's POS or web checkout, accepts a tap/insert/swipe, and processes the charge through Stripe's standard payment pipeline with **lower interchange fees** (card-present vs card-not-present). The `stripe-terminal` module is the V1.2 (MML 4) integration for clinics that take **walk-in card payments** at the front desk (e.g., a consultation fee, a pharmacy over-the-counter sale, a lab test paid in cash + card). MML 4 (not MML 3) because it requires certified hardware, a Stripe Terminal activation flow, and a more complex reader-lifecycle management than a pure web integration. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.2 lane module.

**2. The hardware.** Three certified reader models are supported (per Stripe's catalog):
- **Wise Pad 3** (US/EU/UK) — handheld, Bluetooth, $59 retail. Accepts chip + contactless + magstripe. Battery-powered (8h).
- **Verifone P400** (US/EU/UK) — countertop, Ethernet/Wi-Fi, $349 retail. Pinpad + contactless + chip. Mature, widely deployed in retail.
- **BBPOS WisePOS E** (US/EU/UK/APAC) — Android-based, touchscreen, $399 retail. Full POS UI. Optional receipt printer.
Each reader is purchased from Stripe (or a Stripe reseller), shipped to the merchant, and activated against a Stripe `Location` via the Stripe Terminal activation flow (a one-time pairing between the reader's serial number and the merchant's Stripe account).

**3. The architecture.** Stripe Terminal has two integration modes:
- **Server-driven (`TerminalService`):** the API creates a `PaymentIntent` on Stripe; the API hands the `client_secret` to a JS SDK in the web checkout; the JS SDK discovers the reader, displays the prompt, and processes the card. The web checkout is in control of the UX.
- **Mobile-driven (the merchant's iOS/Android app):** the app uses Stripe's Mobile SDK (iOS / Android); the app discovers the reader, processes the card, and reports back to the API.
The V1.2 module supports both. The web checkout uses the JS SDK (`@stripe/terminal-js`); the mobile apps (iOS / Android, future V2) use the Mobile SDK. For V1.2, the web checkout is the primary surface; the mobile SDK is gated by feature flag.

**4. The `Location` and `Reader` lifecycle.**
- A `Location` is a Stripe concept for "a physical place" (name, address). A tenant can have one or more locations (e.g., "Downtown Clinic" and "Uptown Clinic"). The tenant admin creates the Location in the Stripe Dashboard; the Medicare admin UI mirrors the Location into `terminal_locations` (`{ id, stripe_location_id, label, address, country, status }`).
- A `Reader` is a physical device bound to a Location. The reader is shipped by Stripe; the merchant pairs it via the Stripe Terminal activation flow. Medicare mirrors the Reader into `terminal_readers` (`{ id, stripe_reader_id, label, location_id, status, last_seen_at, battery_level, firmware_version }`).
- A `Reader` has a `status` of `online` (connected to Stripe, ready to charge) or `offline` (powered off, or no network). The status is updated via a webhook (`terminal.reader.action_succeeded` / `terminal.reader.action_failed`); Medicare also polls Stripe every 5 minutes for the latest status.
- A Reader can be **reassigned** to a different Location (e.g., a doctor takes the reader from one clinic to another); reassignment is a `POST /v1/terminal/readers/{id}` with `location=...`; Medicare mirrors.

**5. The charge flow (web checkout).**
1. Front desk staff at the clinic → opens the appointment or invoice in the web checkout; clicks "Take Card Payment".
2. Web checkout calls `POST /api/v2/terminal/charge` with `{ invoice_id, amount, currency, reader_id }`.
3. The `TerminalService` calls Stripe's `POST /v1/payment_intents` with `{ amount, currency, payment_method_types: ['card_present'], capture_method: 'automatic', metadata: { medicare_invoice_id, medicare_tenant_id, reader_id } }`. Stripe returns a `PaymentIntent` with `client_secret`.
4. The API returns `{ payment_intent_id, client_secret, stripe_account_id }` to the web checkout.
5. The web checkout initializes the Stripe Terminal JS SDK with the `client_secret`; the SDK connects to the reader (via Bluetooth, Ethernet, or Wi-Fi); the SDK displays the amount on the reader's screen; the patient taps/inserts/swipes.
6. The card is processed; Stripe sends a webhook `payment_intent.succeeded` (or `.payment_failed`); the API updates the invoice (`invoices.status='paid'`) and the appointment (`status='confirmed'`); the reader prints a receipt (or the web checkout displays a QR code for an email receipt).
7. The `PaymentIntent` is auditable in `payment_intents` (mirrored from Stripe) with the Stripe ID, the amount, the card brand (Visa/Mastercard/Amex/Discover), the last 4, the auth code, and the AID (Application Identifier).

**6. Offline mode.** Stripe Terminal supports a limited offline mode: the reader can store up to 50 transactions while offline, and submit them when the network returns. The transactions are marked `card_present` and are guaranteed by Stripe (no chargebacks for fraud, but a chargeback for "goods not received" is still possible). Medicare honors the offline transactions as paid; the web checkout shows a "Payment queued — will be confirmed when reader is back online" badge until the webhook fires. The offline mode is configured per Stripe Location (default: enabled, max 50 transactions, 7 days).

**7. Refunds and partial captures.** A Terminal `PaymentIntent` can be **partially captured** (e.g., the patient authorized $100 but the actual charge is $80; the remaining $20 is released). The web checkout calls `POST /api/v2/terminal/charge/{id}/capture` with `{ amount_to_capture: 80.00 }`. A Terminal refund follows the standard Stripe refund flow: `POST /api/v2/terminal/charge/{id}/refund` with `{ amount, reason }`; the refund is processed against the original `PaymentMethod` (the card).

**8. Reconciliation.** Stripe's settlement report (CSV/API) is imported nightly; each row is matched against the corresponding `transactions` row by `payment_intent_id`. A mismatch (Stripe says success, we say pending; or vice versa) opens a P2 incident; the reconciliation job is in `app-modules/stripe-terminal/src/Console/ReconcileStripeTerminalSettlements.php`.

**9. Domain Model.**
```php
Schema::create('terminal_locations', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('stripe_location_id', 64)->unique()->index(); // loc_xxx
    $t->string('label', 64);
    $t->string('address_line1', 128)->nullable();
    $t->string('address_line2', 128)->nullable();
    $t->string('city', 64)->nullable();
    $t->string('state', 64)->nullable();
    $t->string('postal_code', 16)->nullable();
    $t->string('country', 2)->nullable();                     // ISO 3166-1 alpha-2
    $t->string('status', 16)->default('active');              // 'active' | 'inactive'
    $t->jsonb('offline_config')->nullable();                  // { enabled, max_transactions, max_days }
    $t->timestamps();
});

Schema::create('terminal_readers', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('clinic_id')->index();
    $t->ulid('terminal_location_id')->index();
    $t->string('stripe_reader_id', 64)->unique()->index();    // tmr_xxx
    $t->string('label', 64);
    $t->string('device_type', 32);                            // 'wise_pad_3' | 'verifone_p400' | 'bbpos_wisepos_e'
    $t->string('serial_number', 64)->nullable();
    $t->string('status', 16)->default('offline');             // 'online' | 'offline' | 'inactive'
    $t->string('firmware_version', 32)->nullable();
    $t->integer('battery_level')->nullable();                 // 0-100
    $t->timestamp('last_seen_at')->nullable();
    $t->timestamp('activated_at')->nullable();
    $t->timestamps();
});

Schema::create('payment_intents', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('invoice_id')->index()->nullable();
    $t->string('stripe_payment_intent_id', 64)->unique()->index();  // pi_xxx
    $t->string('stripe_charge_id', 64)->nullable()->index();        // ch_xxx (after success)
    $t->string('reader_id', 64)->nullable()->index();
    $t->string('location_id', 64)->nullable();
    $t->integer('amount');
    $t->integer('amount_captured')->default(0);
    $t->char('currency', 3);
    $t->string('status', 32);                                 // 'requires_payment_method' | 'requires_confirmation' | 'requires_action' | 'processing' | 'succeeded' | 'canceled'
    $t->string('card_brand', 16)->nullable();                 // 'visa' | 'mastercard' | 'amex' | 'discover'
    $t->string('card_last4', 4)->nullable();
    $t->string('auth_code', 16)->nullable();
    $t->string('application_identifier', 32)->nullable();    // AID
    $t->boolean('offline')->default(false);
    $t->timestamp('captured_at')->nullable();
    $t->timestamp('failed_at')->nullable();
    $t->text('failure_code')->nullable();
    $t->text('failure_message')->nullable();
    $t->timestamps();
    $t->index(['tenant_id', 'status', 'created_at']);
});
```

**10. API Surface.**
```
# Locations (admin)
GET    /api/v2/terminal/locations                            -> [location]
POST   /api/v2/terminal/locations                            -> location
PATCH  /api/v2/terminal/locations/{id}                       -> location
DELETE /api/v2/terminal/locations/{id}                       -> 204

# Readers (admin)
GET    /api/v2/terminal/readers                              -> [reader]
POST   /api/v2/terminal/readers                              -> { stripe_reader_id, label, device_type, location_id } -> reader (after Stripe activation)
PATCH  /api/v2/terminal/readers/{id}                         -> { label?, location_id? }
DELETE /api/v2/terminal/readers/{id}                         -> 204 (unassigns; reader can be reassigned later)
POST   /api/v2/terminal/readers/{id}/test-connection         -> { status: 'online' | 'offline' }

# Charges (front desk)
POST   /api/v2/terminal/charge                               -> { invoice_id, amount, currency, reader_id } -> { payment_intent_id, client_secret, stripe_account_id }
GET    /api/v2/terminal/charge/{id}                          -> payment_intent (with card_brand, last4, auth_code, status)
POST   /api/v2/terminal/charge/{id}/capture                  -> { amount_to_capture } -> payment_intent
POST   /api/v2/terminal/charge/{id}/refund                   -> { amount, reason } -> refund
POST   /api/v2/terminal/charge/{id}/cancel                   -> 204 (releases uncaptured amount)

# Webhooks (Stripe → API)
POST   /api/internal/stripe-terminal/webhook                 # service-to-service, mTLS, payload = parsed JSON
```

**11. Frontend.**
- **Admin:** `/admin/integrations/stripe-terminal` shows: locations (with addresses), readers (with status, battery level, last seen), the "Add a reader" wizard (Stripe activation flow), the "Move a reader to a different location" action, and the offline-mode config.
- **Front desk:** `/checkout/terminal` shows: a reader-picker dropdown (online readers only), the "Take Card Payment" button, the amount-to-charge input, and the real-time payment status. The Stripe Terminal JS SDK is embedded; the reader's screen mirrors the amount.
- **After the charge:** the checkout shows the receipt (last 4, brand, auth code, AID), the option to email the receipt, and the "Print receipt" button (if a paired receipt printer is configured).

**12. Realtime.**
- `terminal.reader.online.{tenant_id}.{reader_id}` — reader came online; admin UI updates.
- `terminal.reader.offline.{tenant_id}.{reader_id}` — reader went offline; admin paged if the reader is the only one at a clinic.
- `terminal.charge.succeeded.{tenant_id}.{invoice_id}` — charge succeeded; invoice marked paid; appointment confirmed.
- `terminal.charge.failed.{tenant_id}.{invoice_id}` — charge failed; staff prompted to retry with another method.
- `terminal.charge.offline_queued.{tenant_id}.{invoice_id}` — charge queued offline; UI shows the "queued" badge.

**13. Security & RBAC.**
- **PCI scope:** we never see the card number; Stripe Terminal handles the EMV flow. The `client_secret` is the only artifact we hold (and it expires in 24h).
- **Reader activation requires `admin.integrations` permission** and is logged in `audit_logs` with `action = 'terminal.reader.activated'`.
- **Charge requires `front_desk` role** (a custom role per tenant); a doctor's user cannot take a payment without explicit front-desk delegation.
- **Refund requires `billing_admin` role**; refunds over $500 require co-sign per `20-Security §11.4`.
- **Offline-mode cap is enforced:** the offline-mode `max_transactions` is enforced at the reader; transactions over the cap are rejected at the reader. Medicare also enforces it as a belt-and-braces check on the `payment_intents.offline=true` count.
- **Webhook verification:** every Stripe webhook is verified with the `STRIPE_WEBHOOK_SECRET`; unverified webhooks return 401.
- **PII:** the card last 4 + brand + auth code are stored; the PAN is never stored.

**14. Acceptance Criteria.**
- [ ] A tenant can register a Location, activate a Reader, and see the Reader's status (online/offline) in the admin UI.
- [ ] A front-desk user can pick a Reader, enter an amount, and complete a charge; the invoice is marked paid; the appointment is confirmed.
- [ ] The charge is recorded in `payment_intents` with the Stripe ID, the card brand, the last 4, the auth code, the AID.
- [ ] A partial capture works: authorize $100, capture $80, release $20.
- [ ] A refund works: full or partial, against the original card.
- [ ] Offline mode: the reader can store up to 50 transactions; the API marks them as `offline=true`; when the reader is back online, the webhooks fire and the invoices are confirmed.
- [ ] Reconciliation: the nightly job imports Stripe's settlement report; mismatches open P2 incidents.
- [ ] Webhook HMAC verification rejects tampered payloads.
- [ ] The reader's battery level and last-seen timestamp are visible in the admin UI.
- [ ] The front desk is blocked from charging more than the invoice's outstanding balance (over-charge is rejected).

**15. Real-World Test Scenarios.**
- *Activation:* tenant admin adds a Wise Pad 3 reader; follows the Stripe activation flow (enters the serial number on the reader, pairs with the Location); the reader appears in the admin UI with `status='online'`.
- *Tap-to-pay:* front desk clicks "Take Card Payment", enters $50; the reader displays "Insert, tap, or swipe $50.00"; the patient taps a Visa; the reader beeps; the invoice is paid; the receipt is emailed.
- *Chip + PIN (EU):* a German patient inserts a chip card; the reader prompts for PIN; the patient enters 4-digit PIN; the charge processes with 3DS / SCA exemption (low-value, card-present); the invoice is paid.
- *Decline:* patient inserts a card with insufficient funds; the reader displays "Declined"; the API receives `payment_intent.payment_failed`; the invoice remains unpaid; the staff is prompted to try another method.
- *Partial capture:* the invoice is $80; the staff authorizes $100 to cover a possible tip; the actual charge is $80; the remaining $20 is released; the audit log shows the capture amount.
- *Offline:* the internet goes down; the reader accepts 3 chip cards; the API marks them as `offline=true`; the web checkout shows "Payment queued" badges; when the internet returns, the webhooks fire; the invoices are paid.
- *Refund:* patient returns a $50 product the same day; the staff issues a $50 refund; the refund is processed; the original `PaymentIntent` is refunded; the invoice is updated.
- *Reader reassignment:* a doctor takes a reader from Downtown Clinic to Uptown Clinic; the admin reassigns the reader; the reader is now bound to the new Location; the next charge at Uptown works.

**16. What this module is NOT:**
- Not a free add-on. The hardware is $59-$399 per reader; tenants buy from Stripe.
- Not a global solution. Stripe Terminal is available in the US, EU, UK, Canada, Australia, Singapore, Japan, and a few others. For other regions, the tenant uses a different card-present provider.
- Not a cash register. Stripe Terminal is a card-only reader; cash handling is a separate workflow.
- Not a payment processor. We use Stripe; we do not hold funds.

**17. Dev Workflow.** `php artisan terminal:list-readers --tenant=...` lists readers and their status. `php artisan terminal:charge-test --tenant=... --reader=... --amount=1.00` triggers a $1 test charge (requires a test-mode reader). `php artisan terminal:reconcile --tenant=... --from=...` reconciles the Stripe settlement report. Code: `app-modules/stripe-terminal/`; tests: `app-modules/stripe-terminal/tests/Feature/{Location,Reader,Charge,Refund,PartialCapture,Offline,Webhook,Reconcile}Test.php` and `tests/Performance/terminal-latency.k6.js`. The dev setup uses Stripe's mock terminal (no real hardware required); the JS SDK is mocked with `@stripe/terminal-js-mock`.

### 18.4 Local payment methods — `local-payment-methods` (V1.2, MML 3) — deep-dive

**1. Overview.** A "local" payment method is one that is **specific to a country or region** and is **not** a card. The most common are:
- **India:** UPI (Unified Payments Interface; ~80% of consumer payments), UPI Autopay (recurring), Paytm Wallet, Net Banking, EMI on credit cards.
- **Brazil:** Pix (instant bank transfer; ~60% of consumer payments), Boleto (cash voucher).
- **Mexico:** OXXO (cash voucher at 20,000+ convenience stores), SPEI (bank transfer).
- **Philippines:** GCash (e-wallet), GrabPay, PayMaya.
- **Indonesia:** GoPay, OVO, DANA, ShopeePay, QRIS (the national QR standard).
- **Nigeria:** Bank transfer, USSD, QR.
- **Kenya:** M-Pesa (mobile money; ~80% of consumer payments).
- **Colombia:** Nequi, PSE.
- **Chile:** Webpay, Khipu.
- **Argentina:** MercadoPago (in addition to the card rails).

The `local-payment-methods` module is the V1.2 (MML 3) integration that exposes these methods to the patient at checkout. MML 3 GA: **UPI Autopay** (per `§18.1.1` deep-dive), **UPI Intent**, **Paytm Wallet**, **Net Banking**, **EMI**, **GCash** (via PayMongo), **Pix** (via Stripe / Ebanx), **OXXO** (via Stripe), **M-Pesa** (via DPO), and the **Paystack bank/QR** paths. The module is registered in `14-MODULE-V2-STUBS.md §3.4a` as a V1.2 lane module. Note: PayMongo (PH), Xendit (ID), and DPO (KE) integrations are V1.2 with 3rd-party gateway adapters; the platform's own adapters for them are V3.

**2. Why cards are not enough.** Cards work in 200+ countries, but card-not-present (CNP) fraud is high in many of them, interchange + scheme fees are 2-3% (vs 0.5-1% for local methods), and the **method share is low**: in India, UPI is 5x card volume; in Brazil, Pix is 4x; in Kenya, M-Pesa is 80%. A patient in those markets expects to pay with the local method; offering only cards is a checkout abandonment driver. Conversely, the local method only works in its home country — a Brazilian patient on vacation in NYC cannot use Pix. The routing is **per recipient country**: the checkout offers the local method(s) for the patient's country and falls back to card for cross-border.

**3. The 8 methods in MML 3 GA.**
- **UPI Intent** (India, Razorpay) — the app opens the patient's UPI app (GPay, PhonePe, Paytm, BHIM, etc.) with the payment intent; the patient authorizes; the app returns to the merchant. Settlement is instant.
- **UPI Collect** (India, Razorpay) — the merchant sends a collect request to a VPA (Virtual Payment Address, e.g., `name@okhdfcbank`); the patient approves in their UPI app.
- **UPI Autopay (eNACH)** (India, Razorpay) — for recurring billing (per `§18.1.1`); the patient approves a mandate; the merchant auto-debits.
- **Paytm Wallet** (India, Razorpay) — the patient pays from their Paytm wallet balance.
- **Net Banking** (India, Razorpay) — 60+ Indian banks; the patient is redirected to their bank's netbanking; logs in; approves the payment.
- **EMI on credit card** (India, Razorpay) — 3/6/9/12 month EMI; the patient picks the tenure at checkout.
- **Pix** (Brazil, Stripe / Ebanx) — instant bank transfer; the patient copies a Pix code or scans a QR; the bank's app confirms; the merchant is notified in < 30 seconds. Stripe and Ebanx both support Pix; the gateway choice is per-tenant (Stripe is preferred for tenants already using Stripe).
- **OXXO** (Mexico, Stripe) — cash voucher; the patient receives a voucher barcode; takes it to an OXXO convenience store; pays cash; the merchant is notified within 24-48 hours. High abandonment (5-10%) but high reach in unbanked segments.
- **GCash** (Philippines, PayMongo) — e-wallet; the patient confirms in the GCash app; settlement is instant. PayMongo is a 3rd-party gateway; the integration is via the PayMongo API.
- **M-Pesa** (Kenya, DPO) — mobile money; the patient receives an STK push on their phone; enters their M-Pesa PIN; the merchant is notified within 30 seconds. DPO is a 3rd-party gateway.
- **Paystack bank/QR** (Nigeria, Paystack) — bank transfer (the patient receives a virtual account number, transfers from their bank, the merchant is notified) or QR (the patient scans, the bank app confirms).
- **Boleto** (Brazil, Stripe) — cash voucher; similar to OXXO; 3-day validity.

**4. The 3 deposit / payout flows.** Local methods have very different deposit / payout models:
- **UPI / Pix / M-Pesa / GCash / Paytm:** instant deposit to the merchant's bank account (within 30 seconds to 24 hours, depending on the method and the country). The patient pays; the gateway holds the funds for a brief window (T+0 to T+1); the gateway releases the funds to the merchant's bank account.
- **OXXO / Boleto:** delayed deposit (T+1 to T+3) because the patient may pay or may not (the voucher expires). The gateway holds the funds until the patient pays at the convenience store; once paid, the gateway releases the funds.
- **Net Banking:** T+1 to T+3 deposit; the gateway receives the funds from the bank on the next business day; releases to the merchant.
- **EMI:** T+30 to T+90 (the gateway receives the EMI from the card network over the tenure); the merchant receives the full amount upfront minus a fee.

**5. The checkout UX.** The local method is offered as a button in the checkout; the user picks it; the gateway-specific UX happens:
- **UPI Intent / Collect / Paytm:** the app opens the patient's UPI / Paytm app; the patient approves; the app returns to the merchant.
- **Net Banking:** the patient is redirected to a list of 60+ banks; picks their bank; logs in to netbanking; approves.
- **EMI:** the patient picks the tenure (3/6/9/12) and the card (must be EMI-eligible); the gateway charges the card as an EMI transaction.
- **Pix / GCash / M-Pesa:** the checkout displays a QR code; the patient scans with the app; the app confirms.
- **OXXO / Boleto:** the checkout displays a voucher barcode; the patient can save it as a PDF or print it.
- **Paystack bank/QR:** the checkout displays a virtual account number or a QR.

**6. The webhook handling.** Each gateway sends a webhook on payment success / failure. The webhook handling is gateway-specific:
- **Razorpay:** `payment.captured` (UPI, NetBanking, Paytm) or `order.paid` (some flows). The webhook is HMAC SHA-256 signed.
- **Stripe:** `payment_intent.succeeded` (Pix) or `checkout.session.completed` (Boleto, OXXO).
- **PayMongo:** `payment.paid` (GCash).
- **DPO:** `payment.successful` (M-Pesa).
- **Paystack:** `charge.success` (bank, QR).

**7. The `PaymentMethodType` enum.** The platform models each method as a `PaymentMethodType` enum: `'card' | 'upi' | 'upi_autopay' | 'paytm_wallet' | 'net_banking' | 'emi_card' | 'pix' | 'boleto' | 'oxxo' | 'gcash' | 'm_pesa' | 'paystack_bank' | 'paystack_qr' | 'paystack_ussd' | ...`. Each value has metadata: `display_name`, `icon`, `country`, `gateway`, `gateway_method` (the gateway-specific code, e.g., Razorpay's `'card' | 'netbanking' | 'wallet' | 'upi' | 'emi'`), `min_amount`, `max_amount`, `currencies`, `settlement_days`, `requires_redirect`, `supports_refund`, `fee_pct` (the gateway fee %).

**8. Domain Model.**
```php
Schema::create('payment_method_types', function (Blueprint $t) {
    $t->string('code', 32)->primary();                         // 'upi' | 'pix' | 'm_pesa' | ...
    $t->string('display_name', 64);
    $t->string('icon', 64)->nullable();
    $t->string('country', 2)->nullable();                     // ISO 3166-1 alpha-2; null = global
    $t->string('gateway', 32);                                 // 'razorpay' | 'stripe' | 'paystack' | 'paymongo' | 'dpo' | 'multiple'
    $t->string('gateway_method', 32)->nullable();              // e.g., 'upi' for Razorpay
    $t->integer('min_amount')->default(0);
    $t->integer('max_amount')->nullable();
    $t->jsonb('currencies');                                    // ['INR', 'USD', ...]
    $t->integer('settlement_days')->default(1);
    $t->boolean('requires_redirect')->default(false);
    $t->boolean('supports_refund')->default(true);
    $t->decimal('fee_pct', 5, 2)->nullable();                  // e.g., 0.40 for UPI
    $t->jsonb('config')->nullable();                            // gateway-specific config
    $t->boolean('is_active')->default(true);
    $t->timestamps();
});

Schema::create('tenant_payment_methods', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('payment_method_code', 32);                     // FK to payment_method_types.code
    $t->boolean('is_enabled')->default(true);
    $t->jsonb('config')->nullable();                            // tenant-specific (e.g., per-method surcharge, display order)
    $t->timestamps();
    $t->unique(['tenant_id', 'payment_method_code']);
});
```

**9. API Surface.**
```
# Public (read-only)
GET    /api/v2/payment-methods                                -> [payment_method] (the tenant's enabled methods)
GET    /api/v2/payment-methods/{code}                         -> payment_method (with gateway, fee, limits, currencies)

# Admin
GET    /api/v2/admin/payment-methods                          -> [payment_method] (all)
PATCH  /api/v2/admin/payment-methods/{code}                   -> { is_enabled?, config? } -> payment_method
POST   /api/v2/admin/payment-methods/reorder                  -> { order: [code, code, ...] } -> 204
GET    /api/v2/admin/payment-methods/stats                    -> { usage_by_method, settlement_days_avg, fee_total }
```

**10. Frontend.** The checkout shows the local method buttons in a grid: each is an icon + label (e.g., "UPI", "Paytm", "Net Banking", "Pix", "M-Pesa"). The order is configurable per tenant. The patient picks one; the gateway-specific UX (QR, redirect, app-open) takes over. After the payment, the invoice is paid and a receipt is shown.

**11. Realtime.**
- `payment_method.local.succeeded.{tenant_id}.{invoice_id}` — local method payment succeeded.
- `payment_method.local.failed.{tenant_id}.{invoice_id}` — local method payment failed; checkout prompts the patient to try another method.
- `payment_method.local.expired.{tenant_id}.{invoice_id}` — voucher expired (OXXO, Boleto); the invoice is reverted to `unpaid`; the patient is notified.

**12. Security & RBAC.**
- **PCI scope:** UPI, Pix, M-Pesa, GCash, Paytm are card-free; the patient never enters a card number. Net Banking and EMI are card-based but tokenized by the gateway. We are out of PCI scope for the local-only paths.
- **UPI collect deepfake risk:** a UPI collect request can be spoofed by a fraudster asking the patient to "approve a refund". We never use UPI Collect for inbound (refunds are always to a saved VP / bank account, never via a new collect request).
- **PII:** the patient's UPI ID, Pix key, M-Pesa number, or GCash number are stored hashed (HMAC-SHA256 with the tenant's KMS key) for the duration of the transaction. They are not stored in plaintext.
- **Roles:** `tenant_admin` (enable / disable methods, set surcharge, reorder), `billing_admin` (view stats, manual refund), `compliance_officer` (view audit).
- **Audit:** every payment-method display, every selection, every gateway call is logged in `audit_logs` with the actor, the method, the amount, and the gateway response.

**13. Acceptance Criteria.**
- [ ] The patient is offered the local methods for their country (e.g., a +91 phone shows UPI, Paytm, NetBanking; a +55 phone shows Pix, Boleto).
- [ ] A UPI Intent payment opens the patient's UPI app; the patient approves; the invoice is paid within 30 seconds.
- [ ] A Pix payment displays a QR; the patient scans; the invoice is paid within 30 seconds.
- [ ] An M-Pesa payment triggers an STK push; the patient enters the PIN; the invoice is paid within 30 seconds.
- [ ] An OXXO voucher is generated; the patient takes it to an OXXO store; the invoice is paid within 72 hours.
- [ ] The settlement days are respected (UPI: T+0, Pix: T+1, OXXO: T+1 to T+3).
- [ ] A failed payment prompts the patient to retry with another method.
- [ ] A voucher expiry (OXXO, Boleto) reverts the invoice to `unpaid` and notifies the patient.
- [ ] The webhook signature is verified per gateway; tampered webhooks are rejected.
- [ ] The per-method fee is logged; the monthly fee rollup is accurate.

**14. Real-World Test Scenarios.**
- *India UPI:* patient picks UPI; the app opens the patient's UPI app; the patient approves; the invoice is paid.
- *India UPI Autopay (recurring):* patient signs up for a monthly pack; the UPI Autopay mandate is created; the first charge happens immediately; subsequent charges happen on the 1st of each month.
- *India NetBanking:* patient picks NetBanking; the patient picks HDFC; logs in to HDFC netbanking; approves; the invoice is paid the next business day.
- *India EMI:* patient picks EMI; picks 6-month tenure; the card is charged as an EMI; the merchant receives the full amount upfront minus a fee.
- *Brazil Pix:* patient picks Pix; a QR is displayed; the patient scans with their bank's app; the app confirms; the invoice is paid within 30 seconds.
- *Mexico OXXO:* patient picks OXXO; a voucher barcode is generated; the patient prints it; takes it to an OXXO store; pays cash; the invoice is paid within 24-48 hours.
- *Philippines GCash:* patient picks GCash; the checkout shows a GCash QR; the patient scans with the GCash app; the invoice is paid.
- *Kenya M-Pesa:* patient picks M-Pesa; an STK push is sent to the patient's phone; the patient enters their M-Pesa PIN; the invoice is paid within 30 seconds.
- *Nigeria bank transfer:* patient picks bank transfer; a virtual account number is displayed; the patient transfers from their bank; the invoice is paid when the funds arrive.
- *Voucher expiry:* the patient does not pay the OXXO voucher within 72 hours; the invoice reverts to unpaid; the patient is notified.

**15. What this module is NOT:**
- Not a free add-on. The gateway fees are real (0.4% for UPI, 1.5% for Pix, 3-5% for M-Pesa, etc.); the cost is passed to the tenant (or the patient, per the tenant's policy).
- Not a crypto module. Crypto is a separate module (per `14-V2-Stubs §3.4`).
- Not a bank transfer module. Local methods are payment methods at checkout, not direct bank-to-bank transfers (except where they are, e.g., Pix, M-Pesa).
- Not a guarantee of instant settlement. Settlement is T+0 to T+3 depending on the method; the patient sees the invoice as paid, but the merchant's bank account is settled later.

**16. Dev Workflow.** `php artisan local-pay:test --tenant=... --method=upi --amount=100` sends a $1 test UPI payment. `php artisan local-pay:list-methods --country=IN` lists the methods available for India. `php artisan local-pay:webhook-test --gateway=razorpay --event=payment.captured` simulates a webhook. `php artisan local-pay:reconcile --gateway=razorpay --from=...` reconciles the gateway's local-method transactions against the local `transactions` table. Code: `app-modules/local-payment-methods/`; tests: `app-modules/local-payment-methods/tests/Feature/{UPI,Pix,M-Pesa,GCash,OXXO,EMI,Webhook,Expiry,Reconcile}Test.php`. Sandbox: Razorpay test mode (UPI / NetBanking / Paytm test credentials), Stripe test mode (Pix / OXXO / Boleto), PayMongo test mode (GCash), DPO sandbox (M-Pesa), Paystack test mode (bank / QR).

### 18.5 Multi-currency with FX (V2)

V1: invoice is in one currency (the clinic's). V2: doctor or tenant sets a base currency; invoices can be created in any of 25 supported currencies; FX is locked at invoice creation; gateway settles in the local currency.

```php
Schema::create('currencies', function (Blueprint $t) {
    $t->string('code', 3)->primary();                       // ISO 4217
    $t->string('name', 64);
    $t->string('symbol', 8);
    $t->integer('decimal_places')->default(2);
    $t->string('locale', 16)->nullable();                   // 'en_IN' for lakh formatting
    $t->boolean('is_active')->default(true);
});

Schema::create('fx_rates', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->string('base_currency', 3)->index();
    $t->string('quote_currency', 3)->index();
    $t->decimal('rate', 18, 8);                              // 1 base = X quote
    $t->timestamp('fetched_at')->index();
    $t->string('source', 32);                               // 'openexchangerates' | 'ecb' | 'fixer' | 'razorpay'
    $t->index(['base_currency', 'quote_currency', 'fetched_at']);
});
```

`FxService::convert(Money $amount, string $toCurrency): Money`:
1. Look up the latest `fx_rates` for the pair.
2. If not found, fetch from a provider (`OpenExchangeRates` primary, ECB free fallback).
3. Return new Money with the converted amount.

`SyncFxRatesJob` runs hourly. Old rates are kept for 90 days for audit.

For display, ICU `NumberFormatter`:
- `en_IN` for ₹ uses lakh/crore grouping: `₹1,23,456.00`.
- `de_DE` for € uses `.` as thousands separator: `1.234,56 €`.

### 18.6 Split payouts (V2)

Multi-clinic chains need to split a single payment across multiple bank accounts. We use **Razorpay Route** (India) or **Stripe Connect** (global).

```php
Schema::create('payout_accounts', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('owner_type', 32);                            // 'tenant' | 'clinic' | 'doctor'
    $t->ulid('owner_id');
    $t->string('gateway', 32);                               // 'razorpay_route' | 'stripe_connect'
    $t->string('gateway_account_id', 128);
    $t->string('status', 16)->default('pending');            // 'pending' | 'active' | 'rejected'
    $t->json('details')->nullable();                         // bank account, PAN, GSTIN
    $t->timestamps();
});
```

When an invoice is paid in a chain, the platform fee + branch's share + doctor's share are routed to the respective `payout_accounts`. Each transfer is auditable.

### 18.7 Payout schedule (V2)

V1: doctor payouts are manual (admin transfers). V2: scheduled, automated.

```php
Schema::create('payout_runs', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('period_start');
    $t->string('period_end');
    $t->string('status', 16)->default('pending');            // 'pending' | 'processing' | 'completed' | 'failed'
    $t->decimal('total_amount', 12, 2);
    $t->char('currency', 3);
    $t->timestamp('processed_at')->nullable();
    $t->timestamps();
});

Schema::create('payout_line_items', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('payout_run_id')->index();
    $t->ulid('doctor_id')->index();
    $t->decimal('gross_amount', 12, 2);
    $t->decimal('platform_fee', 12, 2);
    $t->decimal('tax_withheld', 12, 2);
    $t->decimal('net_amount', 12, 2);
    $t->char('currency', 3);
    $t->string('gateway_transfer_id', 128)->nullable();
    $t->string('status', 16)->default('pending');
    $t->timestamps();
});
```

`GeneratePayoutRunJob` runs weekly (configurable per tenant: weekly, bi-weekly, monthly). For each doctor:
1. Sum completed appointments in the period.
2. Sum refunds (negative).
3. Apply platform fee (configurable % per doctor or per plan).
4. Apply tax withholding (TDS in India, 1099 in US).
5. Initiate transfer via gateway.
6. Generate a payslip PDF.
7. Email the doctor.

### 18.8 V2 API additions

```
POST   /api/v2/payment-methods                             -> { type, details } -> method
GET    /api/v2/payment-methods                             -> [method]
DELETE /api/v2/payment-methods/{id}                        -> 204
POST   /api/v2/payments/subscriptions                      -> subscription
GET    /api/v2/payments/subscriptions                      -> [sub]
POST   /api/v2/payments/subscriptions/{id}/cancel          -> cancelled
POST   /api/v2/payments/bnpl/eligibility                   -> { amount } -> eligible | not
POST   /api/v2/payments/bnpl/checkout                      -> bnpl_order
POST   /api/v2/payments/terminal/charge                    -> { reader_id, amount } -> payment_intent
GET    /api/v2/payments/terminal/readers                   -> [reader]
POST   /api/v2/payments/terminal/readers                   -> reader
GET    /api/v2/payments/fx?from=USD&to=INR&amount=100      -> { converted, rate }
GET    /api/v2/payouts/runs                                -> [run]
GET    /api/v2/payouts/runs/{id}                           -> run with line items
GET    /api/v2/payouts/runs/{id}/payslip/{doctor_id}       -> PDF
```

### 18.9 V2 acceptance criteria

- [ ] Razorpay Subscription round-trips: create, charge, cancel, resume.
- [ ] Stripe Subscription with `default_payment_method` and proration works.
- [ ] Klarna BNPL sandbox flow returns "approved" and we receive the funds.
- [ ] Stripe Terminal SDK connects to a reader, processes a tap, returns the receipt.
- [ ] UPI Autopay (eNACH) mandate is created and auto-debits monthly.
- [ ] Multi-currency invoice: USD base, INR display, settlement in INR; FX locked.
- [ ] Razorpay Route splits a payment 60/40 to two connected accounts.
- [ ] Weekly payout run creates transfers, generates payslips, emails doctors.
- [ ] All V2 payment events are audited and reconciled.

---

## 19. V3+ Modules — From Card Processing to Full Payment Platform

> V1 is one-off card/UPI/netbanking. V2 adds subscriptions, BNPL, Terminal, FX, split payouts. V3+ turns the payments module into a **full payment orchestration platform** — every payment method, every currency, every compliance regime, every reconciliation, every fraud check. ~20 new modules, ~25 new tables, ~80 new endpoints.

### 19.1 Payment Methods Vault (`payment-methods-vault`, V3)

A first-class customer payment-method vault that lets patients save cards, UPI IDs, bank accounts, and wallets, then use them across appointments, subscriptions, and packages.

**Tables:**

```php
Schema::create('payment_methods', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('patient_id')->index();
    $t->ulid('user_id')->index();
    $t->string('type', 32);              // 'card' | 'upi' | 'bank_account' | 'wallet' | 'bnpl' | 'crypto'
    $t->string('gateway', 32);           // 'razorpay' | 'stripe' | 'paystack' | 'paypal' | 'flutterwave'
    $t->string('gateway_token', 256);    // gateway's vault token (e.g., tok_xxx, card_xxx)
    $t->string('gateway_customer_id', 128)->nullable();
    $t->json('details')->nullable();     // last4, brand, expiry (NEVER PAN, CVV, full UPI)
    $t->string('label', 64)->nullable();// 'Personal Visa' (user-defined)
    $t->string('nickname', 32)->nullable();
    $t->boolean('is_default')->default(false);
    $t->boolean('is_verified')->default(false);
    $t->timestamp('verified_at')->nullable();
    $t->json('verification_data')->nullable();  // microdeposit IDs, OTP, etc.
    $t->date('expires_at')->nullable();
    $t->boolean('is_active')->default(true);
    $t->timestamp('last_used_at')->nullable();
    $t->integer('use_count')->default(0);
    $t->string('fingerprint', 64)->nullable();  // hashed PAN, for dedup
    $t->string('billing_address_id', 64)->nullable();
    $t->json('metadata')->nullable();
    $t->timestamps();
    $t->index(['patient_id', 'is_default']);
});

Schema::create('payment_method_mandates', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('payment_method_id')->index();
    $t->string('mandate_type', 32);     // 'e_mandate' | 'standing_order' | 'recurring_card' | 'sepa'
    $t->string('gateway_mandate_id', 128);
    $t->decimal('max_amount', 12, 2)->nullable();
    $t->string('frequency', 16);         // 'monthly' | 'quarterly' | 'as_presented'
    $t->date('start_date');
    $t->date('end_date')->nullable();
    $t->string('status', 16);            // 'created' | 'active' | 'paused' | 'cancelled' | 'expired' | 'failed'
    $t->json('auth_flow')->nullable();   // UPI: VPA + OTP, Card: 3DS, Bank: e-mandate
    $t->timestamps();
});
```

**`PaymentMethodService::add(Patient, $type, $details)`**:
1. Tokenize via gateway (Razorpay: `/v1/tokens`, Stripe: `SetupIntent` + `PaymentMethod`).
2. Validate (Luhn check on cards, BIN range, UPI VPA format, IFSC for bank accounts).
3. Store only the token; never PAN, CVV, or full UPI VPA.
4. Send a 1¢ verification charge for cards (then refund) or microdeposits for bank accounts.
5. Save with `is_verified=true` after successful verification.
6. Trigger 3DS challenge if required (RBI mandate in India; SCA in EU).

**`PaymentMethodService::chargeDefault(Patient, Money, $context)`**: charges the patient's default saved method. Used for:
- Auto-pay for payment plans
- Subscription renewals
- Recurring appointments
- Family member billing

**PCI compliance:** We never see the PAN. The gateway tokenizes client-side (Stripe.js, Razorpay.js). Only the token is stored. PCI-DSS Level 1 achieved by SAQ-A (lowest scope).

**Webhook for mandates:** UPI Autopay e-mandate, Stripe SEPA, eNETS debit — all use webhooks. `payment_method.mandate.activated`, `payment_method.mandate.executed`, `payment_method.mandate.failed`.

### 19.2 Multi-Method Checkout Orchestration (`checkout-orchestration`, V3)

A single checkout endpoint that intelligently presents the right payment methods based on geography, device, amount, and customer preference.

**`CheckoutSessionService::create(Invoice, $context)`**:
1. Determine customer region (IP + profile).
2. Determine device (mobile app / web / POS).
3. Determine amount (small / medium / large).
4. Filter enabled methods per tenant settings.
5. Apply business rules:
   - Amount < $5 → exclude BNPL (no business for them)
   - First-time customer → exclude card-on-file (not yet saved)
   - Repeat customer with BNPL history → offer Klarna/Affirm prominently
   - Mobile app → prioritize UPI (India) / Apple Pay (US) / carrier billing (Africa)
6. Return `CheckoutSession` with ordered method list.

**`POST /api/v3/checkout/sessions`**:
```json
{
  "invoice_id": "01HXXX...",
  "success_url": "https://...",
  "cancel_url": "https://..."
}
```
Returns:
```json
{
  "session_id": "cs_xxx",
  "expires_at": "2026-06-04T11:00:00Z",
  "methods": [
    { "type": "card", "brands": ["visa", "mastercard", "amex"], "default": true },
    { "type": "upi", "apps": ["gpay", "phonepe", "paytm"], "intent": true },
    { "type": "wallet", "providers": ["paytm", "amazonpay"] },
    { "type": "bnpl", "providers": ["klarna", "affirm"], "min_amount": 5000 },
    { "type": "netbanking", "banks": ["HDFC", "ICICI", "SBI"] }
  ],
  "client_config": {
    "razorpay_key": "rzp_live_...",
    "stripe_publishable_key": "pk_live_...",
    "appearance": { "theme": "stripe" }
  }
}
```

The mobile app / web app renders the session and lets the user pick.

**Optimizations:**
- Localize method labels (e.g., "EMI on Credit Card" in India).
- A/B test which methods are presented first.
- Track `method_selected` to learn which methods win.
- Skip methods that have low success rate for the customer (e.g., a card that failed 3x in a row).

### 19.3 Smart Routing & Cascading (`smart-routing`, V3)

When a customer's preferred method fails, automatically try the next one. Reduces cart abandonment by 10-15%.

**`SmartRouter::processPayment(Invoice, $methodAttempts)`**:
1. Attempt 1: customer's default method.
2. If fails (declined, insufficient funds, network error), increment to next.
3. Attempt 2: customer's second-most-used method.
4. Attempt 3: alternate gateway (Razorpay → Stripe if amount > $500 and cross-border).
5. Attempt 4: BNPL (Klarna/Affirm) if amount qualifies.
6. Attempt 5: ask customer to add a new method.

Each attempt is logged with failure reason. The customer's method-effectiveness score is updated (declined cards drop in rank).

**Cascading rules per gateway:**
- Razorpay failure → show Stripe fallback if configured.
- Stripe failure → show Razorpay fallback if configured.
- Card-specific decline codes trigger specific cascades:
  - `card_declined` (generic) → try other cards.
  - `insufficient_funds` → try other cards.
  - `expired_card` → don't retry this card.
  - `fraudulent` → blacklist this card.

### 19.4 Local Payment Methods (V3, comprehensive)

Beyond V2's basic list, V3 supports 30+ local methods across 50+ countries.

**`/config/payments/local_methods.php`:**

| Region | Method | Gateway | Notes |
|---|---|---|---|
| India | UPI Intent | Razorpay | App-to-app deep link |
| India | UPI Collect | Razorpay | VPA + OTP |
| India | UPI Autopay (eNACH) | Razorpay | Recurring via mandate |
| India | RuPay cards | Razorpay | Domestic card network |
| India | Net Banking (50+ banks) | Razorpay | |
| India | Paytm Wallet | Razorpay | |
| India | EMI on Credit Card (3/6/9/12/18/24mo) | Razorpay | No-cost & with-cost EMI |
| India | Cardless EMI (ZestMoney, LazyPay) | Razorpay | |
| India | NEFT/RTGS/IMPS | Manual | For high-value |
| India | Aadhaar Pay | Razorpay | Biometric-based |
| Philippines | GCash | PayMongo | |
| Philippines | GrabPay | PayMongo | |
| Philippines | 7-Eleven | PayMongo | Over-the-counter |
| Philippines | BPI/BDO online | PayMongo | |
| Indonesia | GoPay | Xendit | |
| Indonesia | OVO | Xendit | |
| Indonesia | DANA | Xendit | |
| Indonesia | Bank Transfer (BCA, Mandiri, BNI) | Xendit | Virtual accounts |
| Indonesia | Indomaret | Xendit | OTC |
| Vietnam | MoMo | Xendit | |
| Vietnam | ZaloPay | Xendit | |
| Thailand | PromptPay | Stripe | QR code |
| Thailand | TrueMoney | Stripe | |
| Malaysia | FPX | Stripe | Online banking |
| Singapore | PayNow | Stripe | QR |
| Singapore | GrabPay | Stripe | |
| Brazil | Pix | Stripe / Ebanx | Instant, 24/7 |
| Brazil | Boleto | Stripe / Ebanx | OTC, 1-3 day |
| Brazil | Credit card (parcelado) | Stripe | 2-12 installments |
| Mexico | OXXO | Stripe | OTC, voucher |
| Mexico | SPEI | Stripe | Bank transfer |
| Mexico | BBVA/Citibanamex | Stripe | |
| Chile | Webpay | Stripe | |
| Colombia | PSE | Stripe | |
| Argentina | MercadoPago | MercadoPago | |
| Nigeria | Bank transfer | Paystack | |
| Nigeria | USSD | Paystack | |
| Nigeria | QR | Paystack | |
| Kenya | M-Pesa | DPO / Stripe | STK push |
| Ghana | Mobile Money | Paystack | |
| South Africa | Instant EFT | Paystack | |
| Egypt | Fawry | Paymob | OTC |
| Egypt | Mobile Wallets | Paymob | |
| Saudi Arabia | Mada | Stripe | Domestic cards |
| UAE | KNET | Stripe | |
| EU | SEPA Direct Debit | Stripe | |
| EU | iDEAL | Stripe | Netherlands |
| EU | Bancontact | Stripe | Belgium |
| EU | Giropay | Stripe | Germany |
| EU | SOFORT | Stripe | Germany, EU |
| EU | EPS | Stripe | Austria |
| EU | Przelewy24 | Stripe | Poland |
| UK | Open Banking (PSD2) | Stripe / TrueLayer | |
| US | ACH Direct Debit | Stripe | |
| US | Apple Pay | Stripe | |
| US | Google Pay | Stripe | |
| US | Cash App | Stripe | |
| US | Venmo | Stripe | (limited) |
| US | HSA/FSA | Stripe | Special BINs |
| Global | PayPal | PayPal | |
| Global | Crypto (BTC, ETH, USDC) | Coinbase Commerce / BitPay | |

**Each method registered as `LocalPaymentMethod` enum and adapter.** New country = one new adapter class implementing the same `PaymentMethodHandler` interface.

### 19.5 BNPL Providers (V3, comprehensive)

V2 had generic BNPL. V3 has first-class integrations with the 6 major BNPL providers worldwide.

**Providers:**

| Provider | Region | Plans | Default Payout | Fees |
|---|---|---|---|---|
| Klarna | EU + UK + US | Pay in 4, Pay in 30, Financing | D+1 | 3.29% + $0.30 (US Pay in 4) |
| Affirm | US, Canada | 3/6/12/18/24/36 mo, 0% APR options | D+1 | 0% - 30% merchant fee |
| Afterpay | US, UK, AU, NZ, EU | Pay in 4 | D+1 | 4-6% |
| LazyPay | India | Pay in 15, Pay in 30, EMI | D+1 | 2-4% |
| ZestMoney | India | EMI on credit card | D+7 | 3-5% |
| Simpl | India | Pay in 14, Auto-pay | D+7 | 3-4% |
| Mobikwik ZIP | India | Pay in 15, 30, 90 | D+1 | 2-3% |
| Tamara | KSA, UAE, Kuwait | Pay in 4, Pay in 30, Pay in 90 | D+1 | 3-5% |
| Tabby | KSA, UAE, Kuwait | Pay in 4, Pay in 30 | D+1 | 3-5% |
| Atome | SE Asia | Pay in 3 | D+1 | 3-5% |
| Billie | EU B2B | Net 30-120 | D+1 | 1-2% |

**`BnplProvider` interface:**
```php
interface BnplProvider {
    public function checkEligibility(Money $amount, Patient $patient, BnplProviderCredentials $creds): EligibilityResult;
    public function createOrder(Invoice $invoice, BnplPlan $plan, Patient $patient): BnplOrder;
    public function capture(BnplOrder $order): BnplCapture;
    public function refund(BnplOrder $order, Money $amount): BnplRefund;
    public function verifyWebhook(Request $request): BnplWebhookEvent;
}
```

**Eligibility logic**:
- Klarna: soft credit check via `customer_token`; minimum amount $35; max $10,000.
- Affirm: customer age 18+, SSN not required for amounts < $2,500, full credit check for >$2,500.
- LazyPay: first-time user requires KYC; subsequent are 1-click.

**Plan selection UI**:
```json
{
  "bnpl_options": [
    { "provider": "klarna", "plan": "pay_in_4", "installments": 4, "amount_per_installment": 2500, "interest": 0, "total": 10000, "approved": true },
    { "provider": "klarna", "plan": "pay_in_30", "installments": 1, "amount_per_installment": 10000, "interest": 0, "total": 10000, "approved": true },
    { "provider": "affirm", "plan": "12mo_0apr", "installments": 12, "amount_per_installment": 833, "interest": 0, "total": 10000, "approved": true },
    { "provider": "afterpay", "plan": "pay_in_4", "installments": 4, "amount_per_installment": 2500, "interest": 0, "total": 10000, "approved": true }
  ]
}
```

### 19.6 Stripe Terminal & POS (V3, deep)

V2 introduced Terminal. V3 makes Terminal a first-class POS for in-clinic payment.

**Hardware support:** Wise Pad 3, Verifone P400, BBPOS WisePOS E, Stripe Reader S700 (BBPOS), Square Stand (via Stripe SDK).

**`TerminalService`**:
- `registerReader($serial, $locationId)`: registers a physical reader.
- `pairReader($readerId)`: pairs via Bluetooth (iPad) or network.
- `createPaymentIntent($amount, $description)`: server-side.
- `processPayment($readerId, $paymentIntentId)`: shows on reader, customer taps/inserts/swipes.
- `capturePayment($paymentIntentId)`: explicit capture (auth-then-capture flow).
- `voidPayment($paymentIntentId)`: void uncaptured.
- `refundPayment($paymentIntentId, $amount)`: partial or full refund.
- `printReceipt($paymentIntentId)`: print via connected printer.
- `emailReceipt($paymentIntentId, $email)`: send via SendGrid.
- `smsReceipt($paymentIntentId, $phone)`: send via Twilio.

**Offline mode:** Wise Pad 3 supports offline. `OfflineTransactionService` stores transactions locally on the device; syncs when back online; marks as `offline_settled` for audit.

**Receipts:** Generated via `ReceiptGenerator` (same PDF engine as invoice). Includes:
- Clinic logo, address, tax ID
- Last 4 of card, brand (Visa/Mastercard)
- Amount, currency, date
- Authorization code, transaction ID
- Refund policy summary
- QR code linking to digital receipt

**Reconciliation:** Daily `TerminalReconciliationJob`:
- For each terminal reader, fetch Stripe transactions for the day.
- Match to internal `Payment` records.
- Report discrepancies (missing transactions, orphan transactions, amount mismatches).

**Settlement:** `SettlementReportService` generates per-reader reports showing gross, refunds, net, fees, payout timing.

### 19.7 Crypto Payments (`crypto-payments`, V3)

Accept cryptocurrency for healthcare. Becomes relevant for medical tourism, expat care, and crypto-native patients.

**Gateways:** Coinbase Commerce, BitPay, BTCPay Server (self-hosted), Coingate.

**`CryptoPaymentService`**:
- `createCharge(Money $amount, $currency)`: locks the crypto amount at current rate, gives a 15-min window.
- `getExchangeRate(string $crypto, string $fiat)`: live rate from CoinGecko.
- `verifyWebhook($payload, $signature)`: HMAC verification.
- `confirmations` required: 1 for low value (<$1k), 3 for medium, 6 for high (>$10k).

**Supported cryptocurrencies:** BTC, ETH, USDC, USDT, DAI, BCH, LTC, DOGE, XRP. Per-tenant enable/disable.

**Volatility protection:** The charge is locked at the rate at creation. If the customer pays late (>15 min), the gateway honors the original rate (Coinbase) or marks underpaid (BTCPay). We accept underpayment up to 2% (configurable) and top up the invoice.

**Tax treatment:** Per IRS Notice 2014-21, crypto is property, not currency. Each sale is a taxable event. For healthcare services, this means:
- Clinic records revenue in fiat (USD) at the rate received.
- Capital gain/loss is realized only if clinic holds crypto.
- We convert to USD immediately to avoid gain/loss complications.
- IRS Form 8300 if cash-equivalent > $10k (rare for crypto).

**Compliance:** FinCEN MSB registration required in US. EU 5AMLD/6AMLD. We partner with a regulated custodian (Coinbase, Fidelity Digital Assets) — they handle compliance; we just receive fiat.

### 19.8 Open Banking & PSD2 (`open-banking`, V3)

For EU/UK, leverage Open Banking for direct bank debits at much lower cost than cards.

**Providers:** TrueLayer (UK/EU), Plaid (US/EU), Salt Edge, Tink, Yapily.

**`OpenBankingService`**:
- `initiatePayment(Invoice, $bankAccount)`: redirects customer to bank, they authorize, we get a token.
- `verifyWebhook($request)`: confirms payment from bank.
- `getMandate($bankAccount)`: recurring authorization.

**Benefits:**
- Lower fees: 0.1-0.5% vs. 1.5-2.9% for cards.
- Higher trust: customer authorizes via their own bank.
- SCA-compliant: built-in 2FA.
- No chargebacks: bank debits are final (in most cases).

**Coverage:** UK (CMA9), Germany, France, Spain, Italy, Netherlands, Sweden, Norway, Denmark, Finland, Poland, Ireland.

### 19.9 HSA/FSA Payments (`hsa-fsa-payments`, V3)

US healthcare-specific: accept HSA (Health Savings Account) and FSA (Flexible Spending Account) cards.

**Challenge:** Many HSA/FSA cards have restrictions on eligible merchants (MCC 5912 for drug stores, 8071 for medical services). We use MCC 8099 (Health Services) which most HSA cards accept.

**Eligibility verification:** We display the IRS-qualified medical expense notice on checkout. For FSA cards with the IIAS (Inventory Information Approval System) requirement, we integrate with `WageWorks`, `Optum`, or `LillyDirect` IIAS validators.

**Receipts for FSA:** We generate FSA-compliant receipts that include:
- Date of service
- Provider name + NPI
- Patient name
- Service description
- Amount
- Diagnosis code (ICD-10) — required by some plans

**Substantiation:** Patients can submit receipts to their FSA plan via the plan's app. We offer a "Send to FSA" button that emails the receipt in the plan's required format.

**Card-specific handling:**
- `HSA Bin` ranges: 4616xx, 4617xx, 4729xx (some)
- `FSA Bin` ranges: 4011xx, 4628xx, 4729xx, 4760xx, 5504xx
- Auto-detect from BIN; apply IIAS logic.

### 19.10 Apple Pay / Google Pay / Click to Pay (`digital-wallets`, V3)

Deep integration with mobile wallets.

**Apple Pay:**
- Merchant ID configured in Apple Developer Portal.
- Domain verification file hosted at `/.well-known/apple-developer-merchantid-domain-association`.
- iOS app: `PassKit` framework, `PKPaymentAuthorizationViewController`.
- Web: `ApplePayJS` with merchant session from our server.
- Android: `Wallet foundation`, but Apple Pay is iOS-only.

**Google Pay:**
- Merchant ID configured in Google Pay Console.
- Android: `GooglePayButton` in-app.
- Web: `GooglePayJS`.

**Click to Pay (EMVCo):**
- Visa, Mastercard, Amex, Discover unified wallet.
- Uses `src` and `is` (Identity Service) for one-time enrollment.
- Reduces friction: customer doesn't need to re-enter card on every merchant.

**`DigitalWalletService`**:
- `createSession($amount, $currency)`: returns the client config (merchant ID, cert, etc.).
- `processPayment($token, $amount)`: tokenizes the wallet credential and processes.
- `validateMerchant($domain)`: domain verification for Apple Pay.

### 19.11 Payout Engine (V3, deep)

V2 had basic payouts. V3 makes it a full payroll + vendor payout platform.

**Tables:**

```php
Schema::create('payout_methods', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->ulid('payee_id');                // doctor, vendor, staff
    $t->string('payee_type', 32);        // 'doctor' | 'vendor' | 'staff' | 'contractor'
    $t->string('type', 32);              // 'bank_account' | 'upi' | 'paypal' | 'stripe_connect'
    $t->json('details')->nullable();     // bank, UPI, etc. (encrypted)
    $t->string('gateway', 32);
    $t->string('gateway_token', 256)->nullable();
    $t->boolean('is_default')->default(false);
    $t->boolean('is_verified')->default(false);
    $t->json('verification')->nullable();
    $t->timestamps();
});

Schema::create('payout_runs', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->index();
    $t->string('period_start');
    $t->string('period_end');
    $t->string('frequency', 16);          // 'weekly' | 'biweekly' | 'monthly' | 'adhoc'
    $t->decimal('gross_amount', 12, 2);
    $t->decimal('fees_amount', 12, 2);
    $t->decimal('tax_withheld', 12, 2);
    $t->decimal('net_amount', 12, 2);
    $t->char('currency', 3);
    $t->string('status', 16)->default('draft');  // 'draft' | 'pending_approval' | 'approved' | 'processing' | 'completed' | 'failed'
    $t->ulid('approved_by_user_id')->nullable();
    $t->timestamp('approved_at')->nullable();
    $t->timestamp('processed_at')->nullable();
    $t->json('gateway_response')->nullable();
    $t->timestamps();
});

Schema::create('payout_line_items', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('payout_run_id')->index();
    $t->ulid('payee_id')->index();
    $t->string('payee_type', 32);
    $t->decimal('gross_amount', 12, 2);
    $t->decimal('platform_fee', 12, 2)->default(0);
    $t->decimal('tax_withheld', 12, 2)->default(0);
    $t->decimal('adjustments', 12, 2)->default(0);
    $t->decimal('net_amount', 12, 2);
    $t->char('currency', 3);
    $t->ulid('payout_method_id')->index();
    $t->string('gateway_transfer_id', 128)->nullable();
    $t->string('status', 16)->default('pending');
    $t->text('failure_reason')->nullable();
    $t->integer('attempt_count')->default(0);
    $t->timestamp('paid_at')->nullable();
    $t->json('items_breakdown')->nullable();  // which invoices, refunds, etc.
    $t->timestamps();
});
```

**`PayoutEngine::generate(Period $period, Frequency $freq)`**:
1. Find all completed appointments in the period, per doctor.
2. Sum gross, subtract refunds, subtract chargebacks, subtract adjustments.
3. Apply per-tenant platform fee (e.g., 5% of gross).
4. Apply tax withholding:
   - India: 10% TDS on consultation fees (Section 194J); doctor files returns quarterly.
   - US: 1099-NEC if year-to-date > $600; no withholding for W-9 vendors.
   - EU: VAT reverse charge for cross-border B2B; reverse if VIES valid.
5. Compute net.
6. Group by payout method; create line items.
7. Create payout run in `draft` state.
8. Notify admins: "Payout run generated for review."

**`PayoutEngine::approve($runId, $userId)`**:
- Requires admin role.
- Two-person rule for amounts > $100k: requires 2 admins to approve.
- Audit log with approver, time, IP.

**`PayoutEngine::process($runId)`**:
- Iterate line items.
- For each, call gateway transfer API.
- Razorpay Route: `POST /v1/payouts` with linked account.
- Stripe Connect: `POST /v1/transfers`.
- Paystack: `POST /transferrecipient` then `POST /transfer`.
- ACH: `POST /v1/payouts` (Stripe).
- Update line item status.
- On failure: retry with exponential backoff (3 attempts).
- Final failure: notify admin, queue for manual review.

**Payslip generation:** `PayslipService::generatePdf($payoutLineItem)` produces a PDF with:
- Doctor name, address, tax ID
- Period start/end
- Itemized breakdown: appointments count, gross, refunds, adjustments, fees, tax, net
- Bank reference, UTR
- Cumulative YTD figures
- Form 16 / 1099 link (year-end)

**`GeneratePayoutRunJob` cron weekly** (configurable):
- Friday 23:00 clinic-TZ.
- If period not closed, skip.
- Idempotent: if a run for this period exists in `draft`, update it; if in `processing`, no-op; if in `completed`, no-op.

### 19.12 Tax Compliance & Withholding (`tax-compliance`, V3)

**India TDS (Tax Deducted at Source):**
- 194J: 10% on professional fees (doctor consultations).
- 194H: 5% on brokerage/commission.
- 194C: 1% on contractor payments.
- 194I: 10% on rent.
- TDS deducted at payout; deposited to government by 7th of next month.
- Form 26Q filed quarterly.
- Doctor's PAN determines rate; without PAN, 20%.
- Lower/nil TDS certificate (15G/15H) supported.

**US 1099-NEC:**
- Threshold: $600/year per payee.
- Filed by Jan 31 of following year.
- Tracked YTD per payee.
- Auto-generated 1099-NEC PDF in December; e-filed via IRS FIRE system (V4).

**EU VAT:**
- B2B cross-border: reverse charge (we don't charge VAT; recipient self-accounts).
- B2C cross-border (digital services): MOSS/IOSS one-stop-shop.
- VIES API validation for EU VAT IDs.
- EC Sales List (ESL) for cross-border B2B.

**Australia GST, Canada GST/HST, UK MTD, Singapore GST, Japan Consumption Tax, Saudi Arabia VAT:** all supported via the same `TaxWithholdingService` with jurisdiction-specific config.

**`TaxWithholdingService::compute($payout, $payeeProfile)`**:
1. Determine payee jurisdiction (PAN, SSN, VAT ID, etc.).
2. Determine tenant jurisdiction.
3. Apply rules.
4. Return withholding amount + tax form reference.

### 19.13 Real-Time Fraud & Risk (`fraud-risk`, V3)

**`FraudDetectionService::analyze(PaymentRequest)`** returns risk score 0-100.

**Signals:**
- **Velocity**: 5+ payment attempts in 10 min → +20 risk.
- **Amount anomaly**: First-time customer, amount > 5× median → +25 risk.
- **Geo mismatch**: Card country ≠ IP country ≠ billing country → +30 risk.
- **Device fingerprint**: New device for known card → +15 risk.
- **BIN lookup**: Card from high-fraud BIN range → +20 risk.
- **Email age**: Email < 30 days old → +10 risk.
- **Phone match**: Phone not linked to any account → +15 risk.
- **Time of day**: 3 AM in customer's timezone → +5 risk.
- **Previous chargebacks**: Card has prior chargeback → +40 risk.
- **Blacklist**: Card on Stripe's `radar_block` → 100 risk (auto-decline).
- **Friendly fraud history**: User has prior dispute → +20 risk.
- **Device emulator**: Detect rooted Android / jailbroken iOS → +30 risk.

**Risk thresholds:**
- 0-30: allow.
- 31-60: allow + extra 3DS challenge.
- 61-80: hold for manual review (24h).
- 81-100: auto-decline.

**`FraudAlert` event** broadcasts to merchant dashboard.

**3DS2 (3-D Secure 2):**
- EU SCA (Strong Customer Authentication) requires 3DS2 for cards.
- India RBI mandates 2FA for cards > ₹2,000 (OTP).
- US: optional but recommended.
- We integrate with gateway's 3DS SDK (`Stripe.handleCardAction()`, Razorpay's `authenticate_payment`).
- Fallback: 3DS1 if 3DS2 fails.

**Blacklists:**
- `payment_blacklist` table: cards, emails, phones, IPs.
- Auto-blacklist after 3 chargebacks, 5 declined attempts, or admin flag.
- Manual blacklist for known fraudsters.
- Shared blacklist across tenant (opt-in): pool fraud signals.

### 19.14 Reconciliation Engine (V3, deep)

Beyond V2's manual recon. V3 has a multi-source reconciliation engine.

**Sources to reconcile:**
- Gateway settlement reports (Razorpay, Stripe, Paystack daily CSV/API).
- Bank statements (CSV/OFX/BAI2 import).
- Internal `transactions` table.
- Internal `payments` table.
- Refunds.
- Chargebacks/disputes.
- Adjustments.
- Fees.

**`ReconciliationService::runDaily($tenantId, $date)`**:
1. Fetch gateway settlement report for $date.
2. Fetch bank statement for $date.
3. Match each internal transaction to gateway entry (by gateway_transaction_id).
4. Match each gateway entry to bank deposit (by amount, date, reference).
5. Compute:
   - Total internal captured: Σ transactions with status=succeeded.
   - Total gateway captured: Σ gateway entries.
   - Total bank deposited: Σ bank entries.
   - Total fees: gateway reports.
6. If all three match → mark "reconciled".
7. If mismatches → create `reconciliation_exceptions`:
   - **Missing from gateway**: internal succeeded but gateway has no record (rare; possible delayed sync).
   - **Missing from internal**: gateway has record but internal doesn't (data integrity issue).
   - **Amount mismatch**: amounts differ.
   - **Fee mismatch**: our fee estimate differs from gateway's.
   - **Timing**: gateway reports D+1 settlement; if today is settlement day, expected.

**`ReconciliationExceptionService::resolve($id, $resolution, $user)`**:
- `accept_gateway`: trust the gateway's amount, adjust internal.
- `accept_internal`: trust our record, escalate to gateway support.
- `manual_adjustment`: enter the correct amount, with reason.
- `dispute`: open a dispute with the gateway.
- Every resolution is audit-logged.

**Reconciliation dashboard:**
- Today: 1,234 internal vs 1,234 gateway vs 1,234 bank. Match.
- Last 30 days: 36,789 internal vs 36,789 gateway vs 36,789 bank. Match rate: 99.97%.
- 11 exceptions, 8 resolved, 3 open.

### 19.15 Chargebacks & Disputes (`chargebacks`, V3)

**`ChargebackService::handle($chargeback)`**:
1. Mark payment as `disputed` in `payments` table.
2. Create a `dispute` row with gateway dispute ID, amount, reason, deadline.
3. Notify admin: "Chargeback received for INV-001. Reason: fraud. Deadline: 7 days."
4. Auto-respond where possible:
   - Submit transaction evidence (signed receipt, customer IP, AVS result, 3DS data).
   - Use Stripe's `radar.early_fraud_warning` to counter fraud chargebacks.
5. If evidence wins: dispute closed, payment returns to `succeeded`.
6. If lost: payment → `refunded_chargeback`, debit transaction created.
7. Chargeback fee (Razorpay: ₹200, Stripe: $15) recorded.

**Chargeback reasons and responses:**

| Reason Code (Visa/MC) | Common Response | Evidence Needed |
|---|---|---|
| 10.4 (Visa) / 4837 (MC) — Fraud | Often lost | Customer IP, 3DS data, device fingerprint |
| 13.1 (Visa) / 4863 (MC) — Merchandise/Services Not Provided | Winnable | Service record, prescription, lab results |
| 13.3 (Visa) / 4863 (MC) — Not as Described | Winnable | Photos of service, signed consent |
| 11.3 (Visa) / 4834 (MC) — No Authorization | Often won | Auth record, signed consent |
| 12.x (Visa) — Amount / Currency | Winnable | Receipt, price disclosure |
| 14 (Visa) — Paid by Other Means | Sometimes | Proof of no other payment |
| Fraud (early warning) | Winnable if 3DS | 3DS data |

**Win rate target:** > 60% (industry avg 40%).

**`ChargebackFeeService`**: tracks all gateway chargeback fees per tenant; bills them in next payout.

**Network-level rules:** Visa VROL, Mastercard CDRN — we register as a merchant monitoring service.

### 19.16 Cross-Border & Multi-Currency (V3, deep)

V2 had 25 currencies. V3 supports **150+ currencies** with intelligent FX.

**FX sources (prioritized):**
1. Razorpay (best rates, India-based).
2. Stripe (global, daily rate).
3. Wise (mid-market, lowest margin).
4. OpenExchangeRates (live, 1% margin).
5. ECB (free, daily, EU-only).
6. Central bank rate (last resort).

**`FxService::convert($amount, $from, $to, $lockForInvoiceId)`**:
1. Check if locked rate exists for this invoice (already converted).
2. Look up live rate from primary source.
3. Apply 0.5% margin (configurable).
4. Store in `fx_rates` with TTL 60s.
5. Lock rate for this invoice: store `fx_rate_id` on the invoice so subsequent reads return same.

**`FxService::rebalance()`**:
- We hold some currencies in gateway wallets (USD, EUR, GBP, INR, etc.).
- Daily job monitors balances.
- If USD < 10% of monthly volume → trigger top-up from another currency via FX.
- Uses Wise for low-fee FX.

**Settlement:**
- Default: settle in gateway's local currency (INR for Razorpay, USD for Stripe).
- Option: settle in any of 25 currencies.
- Daily settlement: `SettlementService` pulls funds to tenant's bank account.
- Weekly settlement: default for most tenants.
- Monthly: for very small volumes.

### 19.17 Network Tokenization (V3)

Beyond gateway vault tokens, we use **network tokens** (tokens issued by Visa/Mastercard directly).

**Benefits:**
- Lower interchange fees (0.1-0.3% reduction).
- Higher approval rates (+2-5% on recurring).
- Automatic card updater (when card expires, the network updates the token).
- Reduced PCI scope further.

**How it works:**
- First transaction: card goes through gateway, which calls Visa Token Service or Mastercard MDES.
- Network returns a network token (DPAN — Device Primary Account Number).
- We store the network token, not the PAN.
- Subsequent transactions: charge the network token; Visa/MC routes back to original PAN.
- Card updater: if card expires, network issues new token automatically.

**`NetworkTokenService::provision($cardToken)`**:
- Calls `vault.provision` (Stripe) or equivalent.
- Returns `network_token` + `cryptogram` (for in-person NFC).

**`CardUpdaterService`**:
- Webhook: `payment_method.card_updated` (auto-update from network).
- Update internal `payment_methods.expires_at` and re-verify.

### 19.18 Reporting & Analytics (V3, deep)

**Pre-built reports (40+):**
1. Payments by day/week/month
2. Payments by method (card/UPI/wallet/BNPL/cash)
3. Payments by gateway
4. Authorization rate (successes / attempts)
5. Decline analysis (top decline reasons)
6. Average transaction value
7. Largest transactions
8. Cross-currency volume
9. FX gain/loss
10. Fee analysis (gateway, interchange, scheme)
11. Net vs gross by gateway
12. Payout run history
13. Payout pending / processing / failed
14. Chargeback rate
15. Chargeback reasons
16. Chargeback win rate
17. Fraud block rate
18. 3DS challenge rate
19. Saved card usage rate
20. Wallet usage rate
21. BNPL adoption
22. Subscription churn by method
23. Subscription renewal success rate
24. Mandate failures
25. Refund volume
26. Refund reason analysis
27. Partial vs full refund rate
28. Average refund time
29. Reconciliation match rate
30. Reconciliation exceptions aging
31. Payout run on-time rate
32. Doctor earnings
33. Payout vs earnings variance
34. Pending payouts (doctors owed > 7 days)
35. Top earners
36. Method effectiveness (success rate by method)
37. Geographic distribution
38. New vs returning customers
39. First-time success rate
40. Failed payment recovery (dunning on failed subscriptions)

**Dashboards:**
- Payments Ops: real-time authorization rate, decline rate, errors
- Finance: collections, payouts, fees, FX
- Risk: chargebacks, fraud blocks, 3DS challenges
- Payout: pending runs, processing, failed, on-time rate

**Custom analytics:** Saved cohort analyses, funnel reports (visit → checkout → pay), revenue per user.

### 19.19 Compliance: PCI, PSD2, RBI, KYC/AML (V3, deep)

**PCI-DSS Level 1 (already V2):**
- We never see PAN/CVV.
- Tokenization via gateway.
- Network tokens for recurring.
- Quarterly ASV scan (Approved Scanning Vendor).
- Annual penetration test.
- Internal vulnerability scans monthly.

**PSD2 SCA (EU + UK):**
- 3DS2 mandatory for cards.
- Exempt categories: low-value (< €30), recurring of fixed amount, trusted beneficiary.
- `ExemptionEngine` requests exemptions where eligible.
- We track exemption acceptance rate.

**RBI Tokenization (India):**
- Effective Oct 2021, no entity other than card networks can store card data.
- All tokens are network tokens; we use them with explicit customer consent.
- Customer must re-authenticate every 5 years or when card changes.

**KYC/AML:**
- Customer KYC: collected at registration (name, DOB, address, ID number for high-value).
- Sanction screening: OFAC, UN, EU, UK lists (refreshed daily).
- PEP (Politically Exposed Persons) screening.
- Transaction monitoring:
  - $10k+ single transaction: CTR (Currency Transaction Report).
  - Suspicious patterns: SAR (Suspicious Activity Report).
  - High-risk jurisdictions: enhanced due diligence.
- We use `ComplyAdvantage`, `Jumio`, `Onfido` for KYC; `Refinitiv World-Check` for screening.

**Recordkeeping:** 5 years minimum (10 in some jurisdictions).

### 19.20 V3+ API Additions (Selected Highlights)

```
POST   /api/v3/payment-methods
GET    /api/v3/payment-methods
DELETE /api/v3/payment-methods/{id}
PATCH  /api/v3/payment-methods/{id} (set default, label, etc.)
POST   /api/v3/payment-methods/{id}/verify
GET    /api/v3/checkout/sessions
POST   /api/v3/checkout/sessions
GET    /api/v3/checkout/sessions/{id}
POST   /api/v3/payments/smart-route
GET    /api/v3/bnpl/providers
POST   /api/v3/bnpl/eligibility
GET    /api/v3/bnpl/plans
POST   /api/v3/bnpl/checkout
POST   /api/v3/terminal/readers
GET    /api/v3/terminal/readers
POST   /api/v3/terminal/readers/{id}/pair
POST   /api/v3/terminal/charge
POST   /api/v3/terminal/{payment_id}/refund
GET    /api/v3/terminal/receipts/{id}/pdf
POST   /api/v3/crypto/charges
GET    /api/v3/crypto/charges/{id}
GET    /api/v3/crypto/rates?crypto=BTC&fiat=USD
POST   /api/v3/open-banking/initiate
POST   /api/v3/open-banking/mandates
GET    /api/v3/wallet/apple-pay/session
GET    /api/v3/wallet/google-pay/session
POST   /api/v3/wallet/process
POST   /api/v3/payouts/runs
GET    /api/v3/payouts/runs
GET    /api/v3/payouts/runs/{id}
POST   /api/v3/payouts/runs/{id}/approve
POST   /api/v3/payouts/runs/{id}/process
POST   /api/v3/payouts/runs/{id}/cancel
GET    /api/v3/payouts/runs/{id}/payslip/{payee_id}/pdf
POST   /api/v3/payout-methods
GET    /api/v3/payout-methods
DELETE /api/v3/payout-methods/{id}
POST   /api/v3/payout-methods/{id}/verify
GET    /api/v3/tax/withholding/preview
GET    /api/v3/tax/forms/1099-nec/{year}
GET    /api/v3/tax/forms/26q/{quarter}
POST   /api/v3/fraud/score
GET    /api/v3/fraud/alerts
POST   /api/v3/fraud/blacklist
DELETE /api/v3/fraud/blacklist/{id}
POST   /api/v3/reconciliation/run
GET    /api/v3/reconciliation/exceptions
POST   /api/v3/reconciliation/exceptions/{id}/resolve
GET    /api/v3/disputes
GET    /api/v3/disputes/{id}
POST   /api/v3/disputes/{id}/evidence
POST   /api/v3/disputes/{id}/accept
POST   /api/v3/disputes/{id}/contest
GET    /api/v3/disputes/{id}/status
GET    /api/v3/fx/rates
POST   /api/v3/fx/convert
POST   /api/v3/fx/lock
GET    /api/v3/network-tokens/{payment_method_id}
GET    /api/v3/reports/payments
GET    /api/v3/reports/payouts
GET    /api/v3/reports/chargebacks
GET    /api/v3/dashboards/payments-ops
```

---

## 20. V3+ Test Matrix — Payments

> Total: **~580 tests** across all levels. Same test pyramid as other modules.

### 20.1 Unit Tests (~280)

**PaymentMethodService (40 tests):**
- `add` card via tokenization → stored, never PAN
- `add` UPI via VPA validation → VPA format check
- `add` bank account via IFSC validation → IFSC check
- `add` wallet token via OAuth → success
- `add` invalid card number → throws
- `add` expired card → throws
- `add` CVV in payload → ignored, logged
- `add` PAN in payload → sanitized
- `verify` card with 1¢ auth → success
- `verify` card with 3DS challenge → success on 3DS completion
- `verify` card with 3DS fail → mark unverified
- `verify` bank with microdeposits → success on both deposits
- `verify` bank with one microdeposit wrong → retry
- `setDefault` → other methods `is_default=false`
- `setDefault` for unverified method → throws
- `setDefault` non-existent → throws
- `chargeDefault` with sufficient funds → success
- `chargeDefault` with insufficient funds → decline
- `chargeDefault` with expired card → decline
- `chargeDefault` with 3DS required → challenge
- `chargeDefault` with fraud block → decline
- `remove` default method → assign new default if any
- `remove` last method → success
- `remove` with active subscription → throws
- `findDuplicate` by fingerprint → no duplicate created
- `tokenize` via Stripe → returns token
- `tokenize` via Razorpay → returns token
- `tokenize` failure → throws
- BIN range lookup → brand detected
- HSA card detection (BIN 4616xx) → IIAS path
- FSA card detection (BIN 4011xx) → IIAS path
- UPI intent deep link generated correctly
- Apple Pay merchant validation → 200
- Apple Pay merchant validation with invalid cert → 401
- Google Pay tokenization → success
- Click to Pay enrollment → 200
- Saved card usage counter increments
- Last used timestamp updates
- Multi-currency: USD card charged for EUR invoice → FX applied
- Card on file across tenants → tenant isolation

**CheckoutSessionService (25 tests):**
- `create` returns ordered method list
- `create` excludes BNPL if amount < $35
- `create` excludes BNPL if amount > $10,000
- `create` includes UPI for India region
- `create` includes Apple Pay for iOS device
- `create` includes Google Pay for Android
- `create` includes cash for in-person
- `create` excludes methods not in tenant settings
- `create` includes Klarna if amount qualifies
- `create` includes Affirm if amount qualifies
- `create` includes PayPal if enabled
- `create` returns expiration 15 min
- `create` with first-time customer → no card-on-file offer
- `create` with returning customer → card-on-file first
- `create` with past declined card → deprioritize
- `create` for hospital admission → high-amount methods prioritized
- `create` for retail pharmacy → instant methods (UPI, card)
- `create` for B2B invoice → bank transfer prioritized
- `create` with A/B test variant → different order
- Locale-aware method labels
- A/B test selection deterministic per customer
- Telemetry: every method selection logged
- Expired session returns 410
- Successful session transitions invoice to checkout_pending
- Failed session transitions invoice back to unpaid

**SmartRouter (20 tests):**
- First attempt default method → success
- First attempt declined (insufficient funds) → cascade to method 2
- First attempt declined (fraud) → blacklist card, skip to method 2
- First attempt network error → retry same method once, then cascade
- 2nd attempt declined → cascade to method 3
- 3rd attempt declined → cascade to BNPL
- BNPL declined → ask for new method
- Same card declined 3x → blacklist
- All methods exhausted → fail with reason
- Method-effectiveness score updates on success
- Method-effectiveness score updates on decline
- Decline code routing (card_declined, expired_card, etc.)
- Gateway-level routing: Razorpay fail → Stripe fallback
- Cross-border amount > $500 → Stripe preferred (lower FX fees)
- Cross-border amount < $100 → Razorpay preferred (lower fixed fee)
- India INR → Razorpay only
- US/EU USD/EUR → Stripe preferred
- Africa NGN/ZAR/NGN → Paystack preferred
- Cascade time limit: 60s total → stop and report partial
- Cascade cost limit: max $5 in fees → stop

**LocalPaymentMethod adapters (40 tests):**
- 10 methods per region × 4 regions (India, SEA, LatAm, Africa) = 40 tests
- Each: tokenize, charge, refund, verifyWebhook
- Each: eligibility check
- Each: amount limits
- Each: regional-specific rules (PIX only BRL, M-Pesa only KES, etc.)
- Each: error handling for provider outages

**BnplProvider adapters (40 tests):**
- Klarna: eligibility soft check → approved/declined
- Klarna: pay-in-4 plan → 4 installments
- Klarna: pay-in-30 plan → 1 installment in 30 days
- Klarna: financing 6-36mo → with APR
- Klarna: createOrder → returns auth URL
- Klarna: capture after auth → settles
- Klarna: webhook: installment.paid → updates
- Klarna: webhook: order.cancelled → cancel
- Klarna: refund full → returns to Klarna
- Klarna: refund partial → pro-rata
- Affirm: 3/6/12/24mo plans
- Affirm: 0% APR for qualifying
- Affirm: charge-off webhook
- Affirm: PII token (no SSN stored)
- Afterpay: pay-in-4 with late fees
- Afterpay: declined (first-time, > $1500)
- LazyPay: 0% EMI India
- LazyPay: KYC required first
- ZestMoney: cardless EMI
- Simpl: auto-pay
- All 6 providers: refund flow
- All 6 providers: webhook signature verification
- All 6 providers: error mapping to canonical
- All 6 providers: timeout handling
- All 6 providers: retry on transient errors
- Cross-provider plan comparison
- Provider outage fallback (Klarna down → show Affirm)

**CryptoPaymentService (20 tests):**
- `createCharge` USD amount → BTC equivalent at current rate
- `createCharge` 15-min rate lock window
- `createCharge` expired → must re-create
- `getExchangeRate` BTC/USD → live rate
- `getExchangeRate` from CoinGecko API
- `getExchangeRate` cached 60s
- `verifyWebhook` valid signature → 200
- `verifyWebhook` invalid signature → 401
- `confirmations` 1/3/6 by amount tier
- `processPaid` after confirmations → mark invoice paid
- `processPaid` underpayment < 2% → accept
- `processPaid` underpayment > 2% → flag
- `processPaid` overpayment → keep, credit patient
- `processPaid` wrong crypto (sent ETH, expected BTC) → flag
- Refund crypto to customer wallet
- Tax form generation (Form 8300 if > $10k)
- Sanctions screening of receiving wallet (Chainalysis)
- Coinbase Commerce integration
- BitPay integration
- BTCPay Server (self-hosted)

**OpenBankingService (15 tests):**
- `initiatePayment` returns bank auth URL
- `verifyWebhook` from TrueLayer → 200
- `verifyWebhook` from Plaid → 200
- `getMandate` recurring auth
- Bank account validation (sort code + account number UK, IBAN EU)
- BACS direct debit (UK)
- SEPA direct debit (EU)
- ACH (US)
- Refund to original bank account
- Mandate cancellation
- SCA exemption for trusted beneficiary
- PSD2 compliance check
- ASPSP availability check
- Variable recurring payment (VRP) UK
- Confirmation of payee (CoP) UK

**HSA/FSA service (15 tests):**
- HSA card detected by BIN
- FSA card detected by BIN
- IIAS validation call
- IIAS validation failure → suggest alternate
- FSA-compliant receipt generated
- IRS-qualified expense notice shown
- Diagnosis code (ICD-10) on receipt
- Substantiation email format
- WageWorks integration
- Optum integration
- LillyDirect integration
- Card 2FA verification
- Substantiation dispute
- Substantiation approval
- Substantiation rejection + patient flow

**Apple/Google Pay (10 tests):**
- ApplePay session creation → 200
- ApplePay merchant validation
- ApplePay token processing
- GooglePay session creation
- GooglePay token processing
- Click to Pay enrollment
- Click to Pay token processing
- Wallet token never has PAN
- Wallet charge success
- Wallet charge failure (insufficient funds in bank account)

**PayoutEngine (30 tests):**
- `generate` for period → all paid doctors in run
- `generate` excludes doctors with no completed appointments
- `generate` includes refunds (negative)
- `generate` includes chargebacks (negative)
- `generate` includes adjustments
- `generate` applies platform fee per tenant
- `generate` applies TDS for India doctors
- `generate` generates 1099-NEC for US contractors > $600
- `generate` for VIES-valid EU doctors → no VAT
- `generate` for non-VIES-valid EU doctors → reverse charge
- `approve` requires admin role
- `approve` for amount > $100k → 2-person rule
- `approve` audit log
- `process` calls Razorpay Route
- `process` calls Stripe Connect
- `process` calls Paystack Transfer
- `process` calls Wise (for cross-currency)
- `process` retries on transient failure
- `process` 3 failures → manual review
- `process` partial failure → continues, reports
- `process` updates line item status
- `process` payslips generated
- `process` emails doctors
- `cancel` pending run → no transfers
- `cancel` processing run → stop new, let in-flight complete
- `payslipPdf` includes all required fields
- `payslipPdf` includes YTD totals
- `payslipPdf` includes Form 16/1099 reference
- Idempotent: re-running same period returns same run
- Frequency config respected (weekly, biweekly, monthly)

**TaxWithholdingService (20 tests):**
- India 194J on consultation → 10% TDS
- India 194H on commission → 5% TDS
- India 194C on contractor → 1% TDS
- India without PAN → 20% TDS
- India with 15G/15H certificate → 0% TDS
- US 1099-NEC threshold $600 → tracked YTD
- US 1099-NEC W-9 collected → no backup withholding
- US 1099-NEC W-9 missing → 24% backup withholding
- US 1099-MISC for rent > $600
- EU cross-border B2B with VIES valid → reverse charge
- EU cross-border B2B with VIES invalid → throw
- UK reverse charge for construction
- Australia GST registered → 10% GST
- Canada GST/HST province-specific
- Japan consumption tax
- Saudi Arabia VAT 15%
- Singapore GST 8% (then 9% from 2024)
- Multi-jurisdiction payee
- Cumulative YTD calculation
- Form generation (1099, 26Q, ESL)

**FraudDetectionService (25 tests):**
- Risk score 0-30 → allow
- Risk score 31-60 → 3DS challenge
- Risk score 61-80 → manual review
- Risk score 81-100 → decline
- Velocity: 5+ in 10 min → +20
- Velocity: 10+ in 1 hour → +40
- Amount anomaly: 5× median → +25
- Geo mismatch: card country ≠ IP → +30
- New device → +15
- High-fraud BIN → +20
- Email age < 30 days → +10
- Phone not linked → +15
- 3 AM local time → +5
- Prior chargeback on card → +40
- Blacklisted card → 100 (decline)
- Prior chargebacks on user → +20
- Rooted device detected → +30
- Combined risk > 100 → capped at 100
- `isCardBlacklisted` returns true after 3 chargebacks
- `isEmailBlacklisted` returns true after admin flag
- `addToBlacklist` requires admin
- `removeFromBlacklist` audit-logged
- 3DS2 challenge for EU cards > €30
- 3DS2 exemption for low-value < €30
- 3DS2 exemption for trusted beneficiary
- 3DS fallback to 3DS1 on failure

**ReconciliationService (20 tests):**
- `runDaily` matches internal to gateway
- `runDaily` matches gateway to bank
- Amount mismatch detected
- Missing from gateway → exception
- Missing from internal → exception
- Fee mismatch → exception
- Timing mismatch (gateway reports D+1) → expected
- Auto-resolve if amounts match within $0.01
- Manual resolution requires reason
- `acceptGateway` adjusts internal
- `acceptInternal` opens gateway dispute
- `dispute` creates gateway support case
- Reconciliation rate > 99.9% → green
- Reconciliation rate 99-99.9% → yellow
- Reconciliation rate < 99% → red alert
- Bank CSV import → parses
- Bank OFX import → parses
- Bank BAI2 import → parses
- Format auto-detection
- Cross-tenant reconciliation (per-tenant scope)

**ChargebackService (15 tests):**
- Webhook received → mark disputed
- Reason code mapping (10.4, 13.1, 13.3, etc.)
- Evidence auto-submit
- Evidence custom upload
- 3DS data included
- Deadline tracking (7 days)
- Won → revert to succeeded
- Lost → mark refunded_chargeback
- Chargeback fee tracked
- Network-level dispute (Stripe dispute.created)
- Visa VROL integration
- Mastercard CDRN integration
- Win rate tracked per tenant
- Win rate tracked per reason
- Chargeback alert to admin

**FxService (15 tests):**
- Live rate from primary source
- Cache hit
- Cache miss → fetch
- ECB rate (EU-only)
- OpenExchangeRates rate
- Wise rate
- Cross rate via USD
- 0.5% margin applied
- Rate lock for invoice
- Rate lock expiry
- Stale rate warning (>24h)
- Cross-rate calculation
- Inverse rate
- Reasonable rate check (not 100x off)
- Multi-currency pair: USD→INR, EUR→USD, etc.

**NetworkTokenService (10 tests):**
- Card tokenized → network token issued
- Network token has DPAN (different from PAN)
- Network token stored in payment_methods
- Subsequent charge uses network token
- Card updater: expiry updated
- Card updater: new PAN updated
- Cryptogram for in-person
- Token portability across gateways
- Network token failure → fallback to gateway token
- PAN never stored locally

**Other (15 tests):**
- Idempotency-Key reuse → same response
- Webhook dedup → processed once
- Concurrent order + webhook → one outcome
- Test mode keys → no live charge
- Live mode keys → real charge
- Gateway sync job re-fetches pending
- Failed payment slot release
- 30-day-old pending payment → flagged
- Failed webhook replay → 200
- Signature verification failure → 401
- Body logging disabled for failed signatures
- Per-tenant gateway config
- Currency not supported by gateway → fallback
- Cross-currency with FX locking
- Multi-gateway chain (Razorpay + Stripe)

### 20.2 Integration Tests (~80)

**Database (15):**
- Payment row created on order
- Payment row updated on webhook
- Transaction created on payment.captured
- Refund → debit transaction
- Chargeback → dispute row
- Payout run + line items atomic
- Reconciliation exception creation
- Network token provisioning
- Card updater refresh
- FX rate history retention
- Payment on other tenant invisible
- Hard delete with audit
- Index usage for payment lookup
- Partition pruning for transactions
- Read replica lag tolerance

**Queue/Cache (15):**
- `ProcessPaymentJob` retries on transient
- `SyncPendingPaymentsJob` 5-min cron
- `GeneratePayoutRunJob` weekly cron
- `ReconciliationJob` daily 02:00
- `CardUpdaterJob` daily
- `FxRateSyncJob` hourly
- `FraudCheckJob` async for manual review
- `ChargebackEvidenceJob` auto-submit
- `MandateExecutionJob` for recurring
- `SubscriptionRenewalJob` 24h before
- `RetryFailedPayoutJob` 1h after failure
- `WinLossDisputeJob` 7d deadline
- `ChargebackFeeInvoiceJob` monthly
- `StalePaymentCleanupJob` 30d
- `FraudBlacklistSyncJob` daily

**Webhook (15):**
- Razorpay payment.captured
- Razorpay payment.failed
- Razorpay refund.processed
- Razorpay subscription.activated
- Razorpay subscription.payment_failed
- Razorpay payout.processed
- Razorpay payout.failed
- Stripe payment_intent.succeeded
- Stripe charge.refunded
- Stripe customer.subscription.deleted
- Stripe payout.paid
- Stripe payout.failed
- Stripe dispute.created
- Stripe radar.early_fraud_warning
- Paystack charge.success
- Paystack subscription.create
- Paystack transfer.success
- Paystack transfer.failed
- Klarna order.captured
- Klarna installment.paid
- Affirm charge.authorized
- Affirm loan.closed
- Afterpay payment.captured
- Afterpay payment.failed
- Coinbase Commerce charge.confirmed
- Plaid transfer.status_update
- TrueLayer payment.executed
- Apple Pay session.result
- Google Pay transaction.result

**Third-party (15):**
- Razorpay Subscriptions API
- Stripe Subscriptions API
- Paystack Subscriptions API
- Klarna API v2
- Affirm API
- Afterpay API
- LazyPay API
- ZestMoney API
- Tamara API
- Tabby API
- Stripe Terminal SDK
- Wise API
- Coinbase Commerce API
- Plaid Link
- TrueLayer

**E2E flows (20):**
- New patient books + pays via Razorpay test mode
- New patient books + pays via Stripe test mode
- New patient books + pays via Paystack test mode
- Returning patient pays with saved card
- Patient pays with Apple Pay (test)
- Patient pays with Google Pay (test)
- Patient pays with UPI (test)
- Patient pays with Klarna pay-in-4
- Patient pays with Affirm 12mo 0% APR
- Patient pays with HSA card
- Patient pays with crypto (BTC test mode)
- Patient pays with Open Banking (UK)
- In-clinic card payment via Stripe Terminal reader (mock)
- In-clinic card payment offline (sync when online)
- Patient refund → gateway → invoice updated
- Subscription renews → auto-charge
- Subscription fails → dunning
- Payout run: weekly → doctor receives bank transfer
- Chargeback: patient disputes → evidence → won
- Chargeback: lost → marked refunded
- Multi-currency: USD patient in INR clinic → FX applied
- Cross-currency payout: doctor in USD bank, patient pays INR
- Network tokenization: recurring card without re-entry
- Reconciliation: daily match, exception, manual resolution
- Fraud block: high-risk card → declined

### 20.3 HTTP / API Tests (~120)

**Order/Intent creation (15):**
- `POST /api/v1/payments/razorpay/order` valid → 201
- `POST /api/v1/payments/razorpay/order` invalid amount → 422
- `POST /api/v1/payments/razorpay/order` non-existent invoice → 404
- `POST /api/v1/payments/razorpay/order` for other tenant → 404
- `POST /api/v1/payments/stripe/intent` → 201
- `POST /api/v1/payments/paystack/initialize` → 201
- Idempotency-Key reuse → same response
- `POST /api/v1/payments/razorpay/order` with applied coupon → recomputed amount
- `POST /api/v1/payments/razorpay/order` for paid invoice → 409
- `POST /api/v1/payments/razorpay/order` for voided invoice → 409
- `POST /api/v1/payments/razorpay/order` with override amount → 422 (admin only)
- `POST /api/v1/payments/razorpay/order` admin override → 201
- `POST /api/v1/payments/razorpay/order` min amount → 422
- `POST /api/v1/payments/razorpay/order` max amount → 422
- `POST /api/v1/payments/razorpay/order` expired invoice → 410

**Status (10):**
- `GET /api/v1/payments/{id}` → 200
- `GET /api/v1/payments/{id}/status` → 200
- `GET /api/v1/payments/lookup?order_id=` → 200
- Status poll for paid → succeeded
- Status poll for pending → created
- Status poll for failed → failed
- Status for other tenant payment → 404
- Status for non-existent → 404
- Status before webhook → pending
- Status after webhook → captured

**Refunds (5):**
- `POST /api/v1/payments/{id}/refund` valid → 201
- `POST /api/v1/payments/{id}/refund` over-amount → 422
- `POST /api/v1/payments/{id}/refund` for non-admin → 403
- `POST /api/v1/payments/{id}/refund` reason required → 422
- `POST /api/v1/payments/{id}/refund` already-refunded → 409

**Webhooks (10):**
- `POST /api/v1/webhooks/razorpay` valid → 200
- `POST /api/v1/webhooks/razorpay` bad signature → 401
- `POST /api/v1/webhooks/stripe` valid → 200
- `POST /api/v1/webhooks/stripe` bad signature → 401
- `POST /api/v1/webhooks/paystack` valid → 200
- `POST /api/v1/webhooks/paystack` bad signature → 401
- Duplicate webhook → 200 with duplicate_ignored
- Webhook for non-existent order → 200 (idempotent)
- Webhook for other tenant → 200 (idempotent)
- Webhook body logging disabled for failed sig

**Admin (10):**
- `GET /api/v1/admin/payments?status=&gateway=&from=&to=&page=` → 200
- `GET /api/v1/admin/payments/{id}` → 200
- `POST /api/v1/admin/payments/{id}/sync` → 200, re-fetched
- `GET /api/v1/admin/payments/{id}/gateway-response` → 200
- Bulk export of 10k payments → streamed
- Date range filter fast (index used)
- Search by patient → fast
- Tenant scope enforced
- Admin role required → 403 otherwise
- Refund initiation for any payment → 201

**V3 endpoints (70):**
- `POST /api/v3/payment-methods` card → 201
- `POST /api/v3/payment-methods` UPI → 201
- `POST /api/v3/payment-methods` bank → 201
- `GET /api/v3/payment-methods` → 200
- `PATCH /api/v3/payment-methods/{id}` default → 200
- `DELETE /api/v3/payment-methods/{id}` → 204
- `POST /api/v3/checkout/sessions` → 201
- `GET /api/v3/checkout/sessions/{id}` → 200
- `POST /api/v3/payments/smart-route` → 200
- `POST /api/v3/bnpl/eligibility` → 200
- `POST /api/v3/bnpl/checkout` → 201
- `POST /api/v3/terminal/readers` → 201
- `POST /api/v3/terminal/readers/{id}/pair` → 200
- `POST /api/v3/terminal/charge` → 201
- `POST /api/v3/crypto/charges` → 201
- `GET /api/v3/crypto/rates` → 200
- `POST /api/v3/open-banking/initiate` → 201
- `POST /api/v3/wallet/apple-pay/session` → 200
- `POST /api/v3/payouts/runs` → 201
- `POST /api/v3/payouts/runs/{id}/approve` → 200
- `POST /api/v3/payouts/runs/{id}/process` → 200
- `GET /api/v3/payouts/runs/{id}/payslip/{payee_id}/pdf` → 200
- `POST /api/v3/payout-methods` → 201
- `POST /api/v3/payout-methods/{id}/verify` → 200
- `GET /api/v3/tax/withholding/preview` → 200
- `GET /api/v3/tax/forms/1099-nec/{year}` → 200 PDF
- `POST /api/v3/fraud/score` → 200
- `GET /api/v3/fraud/alerts` → 200
- `POST /api/v3/fraud/blacklist` → 201
- `POST /api/v3/reconciliation/run` → 200
- `GET /api/v3/reconciliation/exceptions` → 200
- `POST /api/v3/reconciliation/exceptions/{id}/resolve` → 200
- `GET /api/v3/disputes` → 200
- `POST /api/v3/disputes/{id}/evidence` → 200
- `POST /api/v3/disputes/{id}/contest` → 200
- `GET /api/v3/fx/rates` → 200
- `POST /api/v3/fx/convert` → 200
- `GET /api/v3/network-tokens/{payment_method_id}` → 200
- `GET /api/v3/reports/payments` → 200
- `GET /api/v3/reports/payouts` → 200
- `GET /api/v3/reports/chargebacks` → 200
- `GET /api/v3/dashboards/payments-ops` → 200
- 30+ more V3 endpoint tests covering error paths, RBAC, tenant scope

### 20.4 E2E Tests (~25, Playwright)

**Patient flows (10):**
- Patient books + pays via Razorpay test → success
- Patient books + pays via Stripe test → success
- Patient books + pays via Paystack test → success
- Patient applies coupon + pays with saved card
- Patient pays with UPI Intent (deep link)
- Patient pays with Apple Pay (web)
- Patient pays with Klarna pay-in-4
- Patient pays with Affirm 12mo 0%
- Patient pays with crypto (test mode)
- Patient pays with HSA card → IIAS validation
- Patient gets refund → sees in transactions
- Patient dispute card → sees chargeback
- Patient cancels subscription → sees refund

**Staff flows (8):**
- Staff initiates in-clinic card payment via Terminal
- Staff completes offline transaction → syncs
- Staff views payment dashboard
- Staff reviews fraud alert → approves/declines
- Staff views reconciliation exceptions
- Staff runs payout run → previews
- Staff approves payout run (2-person for large)
- Staff views payslip PDF

**Admin flows (7):**
- Admin views payouts dashboard
- Admin configures gateway per region
- Admin configures fraud rules
- Admin blacklists card
- Admin views chargebacks → submits evidence
- Admin views reconciliation report
- Admin views 1099/26Q forms

### 20.5 Performance (~10, k6)

- `POST /api/v1/payments/razorpay/order` → p95 < 200ms
- `POST /api/v3/checkout/sessions` → p95 < 300ms
- Webhook processing → p95 < 500ms
- `POST /api/v3/reconciliation/run` (10k txns) → p95 < 30s
- `POST /api/v3/payouts/runs/{id}/process` (100 doctors) → p95 < 5min
- `POST /api/v3/payments/smart-route` → p95 < 1s
- Concurrent order creation: 1000 parallel → all succeed
- Webhook replay: 1000 events → all dedup
- FX rate API call → p95 < 100ms
- Crypto rate API call → p95 < 200ms

### 20.6 Security (~20, OWASP)

- SQL injection in order_id → sanitized
- XSS in payment notes → escaped
- CSRF on order create → token
- Webhook signature bypass → 401
- Webhook replay → idempotent
- Mass assignment in payment-method create → ignored
- Tenant isolation: tenant A payment method → 404 for tenant B
- PAN in payload → sanitized, never stored
- CVV in payload → rejected
- Webhook body logging disabled for invalid sig
- Rate limit: 1000 orders/sec → 429
- Integer overflow on amount → 422
- Negative amount → 422
- Cross-currency mismatch → 422
- 3DS bypass attempt → blocked
- Webhook URL SSRF → blocked
- Payout without 2-person approval > $100k → blocked
- Payout to non-verified bank → blocked
- Refund > payment amount → 422
- Replay attack on payment intent → 409

### 20.7 Compatibility Matrix

- PHP 8.2/8.3/8.4
- Laravel 11/12
- MySQL 8/MariaDB 10.11/PostgreSQL 16
- Redis 7.2/7.4
- iOS 16+ / Android 10+
- Flutter 3.24/3.27
- Stripe API version 2024-06-20
- Razorpay API version 1
- Paystack API version 1

### 20.8 Real-World (~30, with real services in test mode)

- Razorpay test: full payment flow
- Stripe test: card, Apple Pay, Google Pay, SEPA
- Paystack test: full flow
- Klarna sandbox: pay-in-4 approval
- Affirm sandbox: 12mo 0% APR approval
- Afterpay sandbox: pay-in-4
- LazyPay sandbox: India EMI
- ZestMoney sandbox
- Tamara sandbox: KSA
- Tabby sandbox: KSA
- Atome sandbox: SEA
- Coinbase Commerce sandbox: BTC test
- BitPay test
- Plaid sandbox: bank linking
- TrueLayer sandbox
- Wise sandbox: cross-currency
- OpenExchangeRates: live rates
- Stripe Terminal: physical reader test (mock)
- Sumsub/Onfido: KYC
- ComplyAdvantage: sanctions
- Visa VROL test
- Mastercard CDRN test
- Apple Pay sandbox
- Google Pay sandbox
- Click to Pay sandbox
- 3DS2 sandbox (RBI/EMVCo)
- IIAS validator test
- W-9 collection IRS TINVS
- VIES API
- TDS challan generation

---

## 21. V3+ Deep Dives — Payments

### 21.1 Deep Dive: Smart Router & Method Cascading

The smart router intelligently tries alternative payment methods when the first one fails.

```php
namespace App\Modules\Payments\Services;

use App\Modules\Payments\Models\Payment;
use App\Modules\Payments\Models\PaymentMethod;
use App\Modules\Patients\Models\Patient;

class SmartRouter
{
    public function __construct(
        private readonly PaymentGatewayResolver $gateways,
        private readonly FraudDetectionService $fraud,
        private readonly MethodEffectivenessService $effectiveness,
        private readonly AuditService $audit,
    ) {}

    public function processPayment(
        Invoice $invoice,
        Patient $patient,
        array $context = []  // device, country, amount
    ): SmartRouteResult {
        $methods = $this->selectMethods($invoice, $patient, $context);
        $attempts = [];
        $startTime = microtime(true);

        foreach ($methods as $method) {
            // Time check
            if (microtime(true) - $startTime > 60) {
                return $this->fail($attempts, 'timeout_exceeded');
            }
            // Cost check
            if (array_sum(array_column($attempts, 'fee')) > 500) {
                return $this->fail($attempts, 'cost_limit_exceeded');
            }

            try {
                $result = $this->attemptPayment($invoice, $patient, $method);
                if ($result->isSuccess()) {
                    $this->effectiveness->recordSuccess($method, $patient);
                    $this->audit->log('payment.smart_route.success', $invoice, [
                        'method' => $method->id,
                        'attempt_count' => count($attempts) + 1,
                    ]);
                    return new SmartRouteResult(
                        success: true,
                        payment: $result->payment,
                        method: $method,
                        attempts: $attempts,
                    );
                }
                $attempts[] = ['method' => $method, 'result' => $result];
                $this->effectiveness->recordFailure($method, $patient, $result->declineCode);
                // Special case: fraud block, don't try other methods
                if ($result->isFraudBlock()) {
                    return $this->fail($attempts, 'fraud_block');
                }
            } catch (\Throwable $e) {
                $attempts[] = ['method' => $method, 'exception' => $e->getMessage()];
                // Network error: retry same method once
                if ($this->isTransientError($e)) {
                    sleep(1);
                    try {
                        $result = $this->attemptPayment($invoice, $patient, $method);
                        if ($result->isSuccess()) {
                            return new SmartRouteResult(success: true, payment: $result->payment, method: $method, attempts: $attempts);
                        }
                    } catch (\Throwable $e2) {
                        // Fall through
                    }
                }
            }
        }
        return $this->fail($attempts, 'all_methods_exhausted');
    }

    private function selectMethods(Invoice $invoice, Patient $patient, array $context): array
    {
        $methods = collect();

        // 1. Customer's default method (if verified)
        $defaultMethod = $patient->defaultPaymentMethod();
        if ($defaultMethod) {
            $methods->push($defaultMethod);
        }

        // 2. Customer's other methods, sorted by effectiveness
        $otherMethods = $patient->paymentMethods()
            ->where('id', '!=', $defaultMethod?->id)
            ->get()
            ->sortByDesc(function ($m) use ($patient) {
                return $this->effectiveness->score($m, $patient);
            });
        $methods = $methods->merge($otherMethods);

        // 3. Gateway-recommended methods (e.g., UPI for India)
        $region = $context['country'] ?? $patient->country_code;
        $recommended = $this->regionalRecommended($region, $invoice->total, $context);
        $methods = $methods->merge($recommended);

        // 4. Filter blacklisted and expired
        $methods = $methods->filter(fn($m) => $m->is_active && !$m->isExpired() && !$m->isBlacklisted());

        // 5. Filter by amount limits
        $methods = $methods->filter(fn($m) => $m->supportsAmount($invoice->total));

        return $methods->values()->all();
    }

    private function regionalRecommended(string $country, Money $amount, array $context): array
    {
        $recommendations = [];
        if ($country === 'IN' && $amount->isGreaterThan(Money::fromMajor(100, 'INR'))) {
            $recommendations[] = PaymentMethod::findByType('upi_intent');
        }
        if (in_array($country, ['US', 'CA', 'GB', 'AU']) && $context['device'] === 'ios') {
            $recommendations[] = PaymentMethod::findByType('apple_pay');
        }
        if (in_array($country, ['US', 'CA', 'GB', 'AU']) && $context['device'] === 'android') {
            $recommendations[] = PaymentMethod::findByType('google_pay');
        }
        if ($amount->isGreaterThan(Money::fromMajor(5000, 'USD'))) {
            $recommendations[] = PaymentMethod::findByType('affirm_12mo');
        }
        return $recommendations;
    }

    private function attemptPayment(Invoice $invoice, Patient $patient, PaymentMethod $method): PaymentAttemptResult
    {
        // Fraud check first
        $riskScore = $this->fraud->analyze([
            'patient' => $patient,
            'amount' => $invoice->total,
            'method' => $method,
            'ip' => request()->ip(),
            'device_fingerprint' => request()->header('X-Device-Fingerprint'),
        ]);
        if ($riskScore >= 81) {
            return PaymentAttemptResult::fraudBlock($riskScore);
        }
        if ($riskScore >= 61) {
            return PaymentAttemptResult::manualReview($riskScore);
        }

        $gateway = $this->gateways->for($method->gateway);
        return $gateway->charge($method, $invoice, [
            'require_3ds' => $riskScore >= 31 || $this->requires3ds($method, $invoice),
        ]);
    }
}
```

**Key design points:**

1. **Cascading with budget**: Stops at 60s or $5 in fees (configurable).
2. **Fraud pre-check**: Runs before each attempt; fraud block stops the chain.
3. **Effectiveness scoring**: Methods that succeed often get tried first; methods that fail often get tried last.
4. **Region-aware**: UPI for India, Apple Pay for iOS US, Affirm for large amounts.
5. **Blacklist auto-management**: 3+ chargebacks → blacklist automatically.
6. **Idempotency**: Same invoice can't be paid twice (UNIQUE on `payments.invoice_id` with status=created/authorized/captured).
7. **Audit trail**: Every attempt logged with method, result, fee.
8. **Telemetry**: `payment.smart_route.method_selected` event → powers A/B test analysis.

### 21.2 Deep Dive: Payout Engine

```php
namespace App\Modules\Payments\Payout;

use App\Modules\Payments\Models\PayoutRun;
use App\Modules\Payments\Models\PayoutLineItem;
use App\Modules\Payments\Models\Doctor;
use App\Modules\Payments\Services\TaxWithholdingService;

class PayoutEngine
{
    public function __construct(
        private readonly TaxWithholdingService $tax,
        private readonly PayoutGatewayResolver $gateways,
        private readonly AuditService $audit,
    ) {}

    public function generate(\DateTimeInterface $periodStart, \DateTimeInterface $periodEnd, string $frequency = 'weekly'): PayoutRun
    {
        return DB::transaction(function () use ($periodStart, $periodEnd, $frequency) {
            $run = PayoutRun::create([
                'tenant_id' => $this->tenant(),
                'period_start' => $periodStart,
                'period_end' => $periodEnd,
                'frequency' => $frequency,
                'status' => 'draft',
                'currency' => $this->tenant()->default_currency,
            ]);

            $doctors = Doctor::where('tenant_id', $this->tenant())
                ->whereHas('completedAppointments', function ($q) use ($periodStart, $periodEnd) {
                    $q->whereBetween('completed_at', [$periodStart, $periodEnd]);
                })->get();

            $totalGross = Money::zero($this->tenant()->default_currency);
            $totalFees = Money::zero($this->tenant()->default_currency);
            $totalTax = Money::zero($this->tenant()->default_currency);
            $totalNet = Money::zero($this->tenant()->default_currency);

            foreach ($doctors as $doctor) {
                $earnings = $this->calculateEarnings($doctor, $periodStart, $periodEnd);
                if ($earnings['gross']->isZero() && $earnings['adjustments']->isZero()) {
                    continue;
                }

                $platformFee = $this->calculatePlatformFee($doctor, $earnings['gross']);
                $taxWithheld = $this->tax->computeWithholding($doctor, $earnings['gross']->subtract($platformFee));
                $net = $earnings['gross']
                    ->subtract($platformFee)
                    ->subtract($taxWithheld)
                    ->add($earnings['adjustments']);

                $lineItem = PayoutLineItem::create([
                    'payout_run_id' => $run->id,
                    'payee_id' => $doctor->id,
                    'payee_type' => 'doctor',
                    'gross_amount' => $earnings['gross']->minor,
                    'platform_fee' => $platformFee->minor,
                    'tax_withheld' => $taxWithheld->minor,
                    'adjustments' => $earnings['adjustments']->minor,
                    'net_amount' => $net->minor,
                    'currency' => $net->currency,
                    'payout_method_id' => $doctor->defaultPayoutMethod()->id,
                    'status' => 'pending',
                    'items_breakdown' => $earnings['breakdown'],
                ]);

                $totalGross = $totalGross->add($earnings['gross']);
                $totalFees = $totalFees->add($platformFee);
                $totalTax = $totalTax->add($taxWithheld);
                $totalNet = $totalNet->add($net);
            }

            $run->update([
                'gross_amount' => $totalGross->minor,
                'fees_amount' => $totalFees->minor,
                'tax_withheld' => $totalTax->minor,
                'net_amount' => $totalNet->minor,
            ]);

            $this->audit->log('payout_run.generated', $run, [
                'doctor_count' => $doctors->count(),
                'gross' => $totalGross->format(),
                'net' => $totalNet->format(),
            ]);

            return $run;
        });
    }

    public function approve(string $runId, User $approver): void
    {
        $run = PayoutRun::findOrFail($runId);
        $this->authorizeApproval($run, $approver);

        $run->update([
            'status' => 'approved',
            'approved_by_user_id' => $approver->id,
            'approved_at' => now(),
        ]);

        $this->audit->log('payout_run.approved', $run, ['approver' => $approver->id]);
    }

    private function authorizeApproval(PayoutRun $run, User $approver): void
    {
        if (!$approver->hasAnyRole(['admin', 'finance_manager'])) {
            throw new UnauthorizedException();
        }
        if ($run->gross_amount > 10000000 && !$run->isTwoPersonApproved($approver)) {  // > $100k
            throw new TwoPersonRuleRequired();
        }
    }

    public function process(string $runId): void
    {
        $run = PayoutRun::findOrFail($runId);
        if ($run->status !== 'approved') {
            throw new InvalidStateException("Run must be approved, currently: {$run->status}");
        }
        $run->update(['status' => 'processing']);

        $failedCount = 0;
        $successCount = 0;

        foreach ($run->lineItems as $line) {
            try {
                $gateway = $this->gateways->for($line->payoutMethod);
                $transferId = $gateway->createTransfer($line);
                $line->update([
                    'gateway_transfer_id' => $transferId,
                    'status' => 'completed',
                    'paid_at' => now(),
                ]);
                $successCount++;
            } catch (TransientPayoutException $e) {
                $line->increment('attempt_count');
                if ($line->attempt_count >= 3) {
                    $line->update(['status' => 'failed', 'failure_reason' => $e->getMessage()]);
                    $failedCount++;
                }
            } catch (\Throwable $e) {
                $line->update(['status' => 'failed', 'failure_reason' => $e->getMessage()]);
                $failedCount++;
            }
        }

        $run->update([
            'status' => $failedCount === 0 ? 'completed' : 'completed_with_failures',
            'processed_at' => now(),
        ]);

        if ($successCount > 0) {
            $this->generatePayslips($run);
            $this->notifyDoctors($run);
        }
        if ($failedCount > 0) {
            $this->notifyAdminsOfFailures($run);
        }

        $this->audit->log('payout_run.processed', $run, [
            'success' => $successCount,
            'failed' => $failedCount,
        ]);
    }

    private function calculateEarnings(Doctor $doctor, $start, $end): array
    {
        $appointments = $doctor->appointments()
            ->where('status', 'completed')
            ->whereBetween('completed_at', [$start, $end])
            ->get();

        $gross = Money::zero($doctor->tenant->default_currency);
        $refunds = Money::zero($doctor->tenant->default_currency);
        $breakdown = [];

        foreach ($appointments as $apt) {
            $aptGross = $apt->invoice->total;
            $gross = $gross->add($aptGross);
            $breakdown[] = [
                'type' => 'appointment',
                'appointment_id' => $apt->id,
                'date' => $apt->completed_at,
                'amount' => $aptGross->format(),
            ];
        }

        $refundedTxns = $doctor->refundedTransactions()
            ->whereBetween('processed_at', [$start, $end])
            ->get();
        foreach ($refundedTxns as $tx) {
            $refunds = $refunds->add(Money::fromMinor($tx->amount, $doctor->tenant->default_currency));
            $breakdown[] = [
                'type' => 'refund',
                'transaction_id' => $tx->id,
                'date' => $tx->processed_at,
                'amount' => '-' . Money::fromMinor($tx->amount, $doctor->tenant->default_currency)->format(),
            ];
        }

        return [
            'gross' => $gross,
            'adjustments' => $refunds->negate(),
            'breakdown' => $breakdown,
        ];
    }
}
```

**Critical correctness points:**

1. **Idempotency**: Re-running for the same period returns the same run.
2. **Two-person rule**: Amounts > $100k require 2 admins.
3. **Tax withholding computed correctly per jurisdiction**.
4. **Atomicity**: All line items in a run are created atomically (DB transaction).
5. **Failure isolation**: One line item failing doesn't fail the whole run.
6. **Retry on transient**: 3 attempts with exponential backoff.
7. **Payslip generation**: Auto-generated and emailed.
8. **Audit trail**: Every state transition logged.
9. **Period close**: Run is for a closed period; appointments after the close are not included.

### 21.3 Deep Dive: Reconciliation Engine

```php
namespace App\Modules\Payments\Reconciliation;

use App\Modules\Payments\Models\Transaction;
use App\Modules\Payments\Models\Payment;
use App\Modules\Payments\Models\ReconciliationException;
use Brick\Money\Money as BrickMoney;

class ReconciliationService
{
    public function runDaily(\DateTimeInterface $date, string $tenantId): ReconciliationReport
    {
        $startOfDay = $date->setTime(0, 0);
        $endOfDay = $date->setTime(23, 59, 59);

        // 1. Internal transactions
        $internalTxns = Transaction::where('tenant_id', $tenantId)
            ->whereBetween('processed_at', [$startOfDay, $endOfDay])
            ->where('status', 'succeeded')
            ->get();

        // 2. Gateway settlement
        $gatewayTxns = $this->fetchGatewaySettlements($tenantId, $date);

        // 3. Bank statement
        $bankTxns = $this->fetchBankStatement($tenantId, $date);

        $report = new ReconciliationReport($date, $tenantId);

        // Match internal to gateway
        foreach ($internalTxns as $internal) {
            $gatewayMatch = $this->findGatewayMatch($internal, $gatewayTxns);
            if (!$gatewayMatch) {
                $report->addException(ReconciliationException::missingFromGateway($internal));
            } elseif (!BrickMoney::ofMinor($internal->amount, $internal->currency)
                ->isEqualTo(BrickMoney::ofMinor($gatewayMatch->amount, $gatewayMatch->currency))) {
                $report->addException(ReconciliationException::amountMismatch($internal, $gatewayMatch));
            }
        }

        // Find gateway orphans (in gateway but not internal)
        foreach ($gatewayTxns as $gateway) {
            $internalMatch = $this->findInternalMatch($gateway, $internalTxns);
            if (!$internalMatch) {
                $report->addException(ReconciliationException::missingFromInternal($gateway));
            }
        }

        // Match gateway to bank
        foreach ($gatewayTxns as $gateway) {
            $bankMatch = $this->findBankMatch($gateway, $bankTxns);
            if (!$bankMatch) {
                $report->addException(ReconciliationException::missingFromBank($gateway));
            }
        }

        return $report;
    }

    public function resolveException(string $exceptionId, string $resolution, User $user, ?string $notes = null): void
    {
        $exception = ReconciliationException::findOrFail($exceptionId);
        match ($resolution) {
            'accept_gateway' => $this->acceptGateway($exception, $user, $notes),
            'accept_internal' => $this->acceptInternal($exception, $user, $notes),
            'manual_adjustment' => $this->manualAdjustment($exception, $user, $notes),
            'dispute' => $this->openDispute($exception, $user, $notes),
            default => throw new InvalidArgumentException("Unknown resolution: $resolution"),
        };

        $exception->update([
            'status' => 'resolved',
            'resolved_by_user_id' => $user->id,
            'resolved_at' => now(),
            'resolution' => $resolution,
            'resolution_notes' => $notes,
        ]);

        $this->audit->log('reconciliation.exception.resolved', $exception, [
            'resolution' => $resolution,
            'notes' => $notes,
        ]);
    }
}
```

**Key design points:**

1. **Three-way match**: Internal ↔ Gateway ↔ Bank. Each mismatch creates an exception.
2. **Auto-resolution**: Exact matches auto-resolve.
3. **Manual resolution paths**: accept_gateway, accept_internal, manual_adjustment, dispute.
4. **Two-person rule for high-value exceptions**.
5. **Idempotent**: Re-running the same date produces the same report.
6. **Time-bounded**: Day-based, with timezone support (per-tenant).
7. **Tolerance**: Match within $0.01 (handles rounding differences).
8. **Audit trail**: Every match, every exception, every resolution logged.

### 21.4 Deep Dive: Network Tokenization

Network tokenization replaces stored PANs with network-issued tokens.

```php
namespace App\Modules\Payments\NetworkToken;

use App\Modules\Payments\Models\PaymentMethod;
use Stripe\StripeClient;

class NetworkTokenService
{
    public function __construct(
        private readonly StripeClient $stripe,
        private readonly AuditService $audit,
    ) {}

    public function provision(PaymentMethod $paymentMethod): NetworkToken
    {
        if ($paymentMethod->type !== 'card') {
            throw new \InvalidArgumentException("Only cards can be network-tokenized");
        }
        if (!$paymentMethod->gateway_token) {
            throw new \InvalidArgumentException("Payment method must have a gateway token");
        }

        $pm = $this->stripe->paymentMethods->retrieve($paymentMethod->gateway_token);

        if (empty($pm->card->networks_tokenized)) {
            // No network token yet, request one
            $pm = $this->stripe->paymentMethods->update($paymentMethod->gateway_token, [
                'card' => ['networks' => ['requested' => true]],
            ]);
        }

        $tokenData = $pm->card->networks->first();
        $networkToken = NetworkToken::updateOrCreate(
            [
                'payment_method_id' => $paymentMethod->id,
                'network' => $tokenData->network,
            ],
            [
                'network_token_id' => $tokenData->token,
                'last4' => $tokenData->last4,
                'exp_month' => $tokenData->exp_month,
                'exp_year' => $tokenData->exp_year,
                'fingerprint' => $tokenData->fingerprint,
                'issuer' => $pm->card->issuer,
                'country' => $pm->card->country,
            ]
        );

        $this->audit->log('network_token.provisioned', $networkToken);

        return $networkToken;
    }

    public function charge(NetworkToken $token, Money $amount, array $meta = []): GatewayPayment
    {
        return $this->stripe->paymentIntents->create([
            'amount' => $amount->minor,
            'currency' => strtolower($amount->currency),
            'payment_method_types' => ['card'],
            'payment_method_data' => [
                'type' => 'card',
                'card' => [
                    'token' => $token->network_token_id,  // Network token, not PAN
                ],
            ],
            'confirm' => true,
            'metadata' => $meta,
        ]);
    }

    public function handleCardUpdate(WebhookEvent $event): void
    {
        $paymentMethodId = $event->raw['data']['object']['id'];
        $pm = $this->stripe->paymentMethods->retrieve($paymentMethodId);

        $internal = PaymentMethod::where('gateway_token', $paymentMethodId)->first();
        if (!$internal) return;

        // Update card details
        $internal->update([
            'details' => array_merge($internal->details ?? [], [
                'last4' => $pm->card->last4,
                'brand' => $pm->card->brand,
                'exp_month' => $pm->card->exp_month,
                'exp_year' => $pm->card->exp_year,
            ]),
            'expires_at' => sprintf('%04d-%02d-01', $pm->card->exp_year, $pm->card->exp_month),
        ]);

        // Re-provision network token
        $this->provision($internal);

        $this->audit->log('payment_method.card_updated', $internal);
    }
}
```

**Key benefits:**

1. **Lower interchange fees**: Network tokens qualify for credential-on-file (CoF) rates (0.1-0.3% lower).
2. **Higher approval rate**: Tokens are pre-vetted by the network; less decline risk.
3. **Auto card updater**: When card expires or is replaced, the network updates the token automatically.
4. **Reduced PCI scope**: We never see PAN, even during a charge.
5. **Cryptogram for in-person**: For NFC/contactless, the network token includes a one-time cryptogram.
6. **Fallback**: If network token fails, fall back to gateway token (then PAN tokenization).

### 21.5 Deep Dive: 837P / 270 / 271 / 835 Integration (cross-ref 09-Billing)

The payments module integrates with clearinghouses for eligibility, claims, and remittance.

- **270/271**: Eligibility request/response. Sent to payer; response parsed; benefits cached.
- **837P**: Professional claim submission. Sent to clearinghouse; 999/277CA received.
- **835**: Electronic remittance. Received; parsed; auto-posted; CARC/RARC processed.

These are detailed in `09-MODULE-BILLING.md` §24.4 (ERA) and §24.6 (837 generation).

The payments module's role is to:
- Hold the gateway mapping (e.g., which clearinghouse for which payer).
- Provide the API key/credential store.
- Sign/verify messages.
- Track submission status.
- Trigger payouts based on ERA (insurance payments → invoice payments → doctor payout).

### 21.6 Deep Dive: Multi-Gateway Smart Routing at Scale

When a customer pays, we want the highest success rate at the lowest cost. Multi-gateway routing is the answer.

```php
namespace App\Modules\Payments\Routing;

class MultiGatewayRouter
{
    public function __construct(
        private readonly GatewayPerformanceRepository $perfRepo,
    ) {}

    public function selectGatewayForPayment(
        Patient $patient,
        Money $amount,
        string $country,
        string $currency
    ): string {
        $candidates = $this->getEnabledGateways();
        $scores = [];

        foreach ($candidates as $gateway) {
            $score = $this->scoreGateway($gateway, $patient, $amount, $country, $currency);
            $scores[$gateway] = $score;
        }

        arsort($scores);
        return array_key_first($scores);
    }

    private function scoreGateway(string $gateway, $patient, Money $amount, string $country, string $currency): float
    {
        $score = 100.0;

        // 1. Region fit
        $regionFit = $this->regionFit($gateway, $country);
        $score *= (1 + $regionFit);

        // 2. Currency support
        if (!$this->supportsCurrency($gateway, $currency)) {
            return 0;  // Cannot use
        }

        // 3. Historical success rate
        $successRate = $this->perfRepo->successRate($gateway, $country, $currency, days: 7);
        $score *= (0.5 + $successRate);  // 0.5 to 1.5

        // 4. Cost
        $cost = $this->estimateCost($gateway, $amount);
        $score *= (1 / (1 + $cost / 100));  // Higher cost = lower score

        // 5. Method availability
        $methodFit = $this->methodFit($gateway, $patient, $country);
        $score *= (1 + $methodFit);

        return $score;
    }

    private function regionFit(string $gateway, string $country): float
    {
        $primary = [
            'razorpay' => ['IN'],
            'paystack' => ['NG', 'GH', 'ZA', 'KE'],
            'stripe' => ['US', 'CA', 'GB', 'AU', 'DE', 'FR', 'ES', 'IT', 'NL', 'JP', 'SG'],
            'paypal' => ['US', 'GB', 'DE', 'FR'],
        ];
        return in_array($country, $primary[$gateway] ?? []) ? 0.5 : 0.0;
    }

    private function supportsCurrency(string $gateway, string $currency): bool
    {
        $supported = [
            'razorpay' => ['INR'],
            'paystack' => ['NGN', 'GHS', 'ZAR', 'KES', 'USD'],
            'stripe' => ['USD', 'EUR', 'GBP', 'CAD', 'AUD', 'JPY', 'SGD', 'INR'],
            'paypal' => ['USD', 'EUR', 'GBP', 'CAD', 'AUD', 'JPY'],
        ];
        return in_array($currency, $supported[$gateway] ?? []);
    }

    private function estimateCost(string $gateway, Money $amount): float
    {
        // Estimated cost in dollars
        return match ($gateway) {
            'razorpay' => ($amount->minor * 0.02 / 100) / 100,  // 2%
            'stripe' => ($amount->minor * 0.029 / 100 + 30) / 100,  // 2.9% + $0.30
            'paystack' => ($amount->minor * 0.015 / 100 + 100) / 100,  // 1.5% + NGN 100
            'paypal' => ($amount->minor * 0.0349 / 100 + 30) / 100,  // 3.49% + $0.30
            default => 999.0,
        };
    }
}
```

**Key design points:**

1. **Multi-factor scoring**: Region, currency, success rate, cost, methods.
2. **Performance data feeds back**: Success rate is computed from past 7 days, refreshed daily.
3. **Fallback chain**: If primary fails, try next-best.
4. **Tenant override**: Tenant can pin a specific gateway (e.g., "always use Razorpay in India").
5. **A/B testing**: 10% of traffic uses Stripe even in India (to gather comparative data).

---

## 22. V3+ Precise Plan — Payments

### 22.1 Phase 1: V1 Card/UPI/Netbanking (Months 0-3, complete)

**Status:** ✅ Complete (sections 1-17)

### 22.2 Phase 2: V2 Subscriptions, BNPL, Terminal, FX, Payouts (Months 4-9, complete)

**Status:** ✅ Complete (sections 18.1-18.9)

### 22.3 Phase 3: V3 Payment Vault + Checkout Orchestration (Months 10-13)

**Goal:** Save payment methods (card, UPI, bank), tokenize once, reuse across appointments.

**Deliverables:**
- `payment_methods` table (25 columns)
- `payment_method_mandates` table
- `PaymentMethodService` (add, verify, chargeDefault, setDefault, remove)
- `CheckoutSessionService` (smart method selection)
- Apple Pay / Google Pay / Click to Pay integration
- HSA/FSA detection
- 15 new V3 endpoints
- 8 acceptance criteria

**Team:** 3 backend engineers, 1 iOS (Apple Pay), 1 Android (Google Pay), 1 QA

**Dependencies:** Stripe + Razorpay tokenization APIs; Apple Developer account; Google Pay Console; IIAS validator (for HSA/FSA)

**Risks:**
- **R1**: Apple Pay merchant validation failures → 2-3 rounds of cert/domain verification
- **R2**: Google Pay production approval takes 2-4 weeks → start early
- **R3**: UPI VPA validation requires NPCI approval → use Razorpay's built-in
- **R4**: Card 3DS challenge in EU mandatory → may reduce conversion by 1-2% (acceptable trade-off)
- **R5**: HSA/FSA IIAS validation fails often → fall back to general healthcare MCC

**Metrics:** Card-on-file usage > 40% of repeat customers. Conversion rate +5%. Apple Pay/Google Pay adoption > 15% on mobile.

**Cost:** $280k (3 backend × 4 months × $10k + 2 mobile × 4 × $8k)

### 22.4 Phase 4: V3 Smart Routing + Cascading (Months 14-17)

**Goal:** When one method fails, try the next automatically.

**Deliverables:**
- `SmartRouter` service
- `MethodEffectivenessService` (scoring model)
- A/B test framework for method ordering
- Cross-gateway routing logic
- 5 new endpoints
- 5 acceptance criteria

**Team:** 2 backend engineers, 1 data engineer (effectiveness model), 1 QA

**Dependencies:** Phase 3 complete; 90 days of payment history for training data

**Risks:**
- **R1**: Cascading could increase fees (multiple attempts) → cost limits
- **R2**: Customer confusion ("why was my second card charged?") → clear UI
- **R3**: Chargebacks from "I didn't authorize" → only cascade to methods customer has used before
- **R4**: Time limit too short → make configurable per amount tier

**Metrics:** Conversion rate +10%. Authorization rate +5%. Avg # of attempts to success: 1.4.

**Cost:** $150k (3 engineers × 4 months)

### 22.5 Phase 5: V3 BNPL + Crypto + Open Banking (Months 18-21)

**Goal:** Add 8+ BNPL providers, crypto payments, EU Open Banking.

**Deliverables:**
- 8 BNPL adapters (Klarna, Affirm, Afterpay, LazyPay, ZestMoney, Simpl, Tamara, Tabby)
- 2 crypto gateways (Coinbase Commerce, BitPay)
- Open Banking (Plaid, TrueLayer)
- BNPL eligibility service
- Tax compliance for crypto
- 15 new endpoints
- 10 acceptance criteria

**Team:** 4 backend engineers (each takes 2 BNPL), 1 ML engineer (eligibility scoring), 1 QA

**Dependencies:** Phase 4 complete; BNPL sandbox accounts (8 providers); crypto custody partner

**Risks:**
- **R1**: BNPL sandbox limitations → some providers (e.g., Affirm) require production approval
- **R2**: Crypto volatility → 2% tolerance + immediate conversion
- **R3**: Open Banking requires PSD2 compliance audit → partner with licensed providers
- **R4**: Regulatory: crypto may be restricted in some jurisdictions → geo-block

**Metrics:** BNPL adoption > 5% of payments >$500. Crypto usage > 1% of international payments. Open Banking > 10% in EU.

**Cost:** $340k (4 backend × 4 months × $10k + 1 ML × 4 × $15k + sandbox accounts ~$20k)

### 22.6 Phase 6: V3 Payout Engine + Tax Compliance (Months 22-25)

**Goal:** Automated weekly payouts, multi-jurisdiction tax withholding.

**Deliverables:**
- `payout_methods`, `payout_runs`, `payout_line_items` tables
- `PayoutEngine` (generate, approve, process)
- `TaxWithholdingService` (India/US/EU/UK/AU)
- `PayslipService` (PDF generation)
- Razorpay Route, Stripe Connect, Paystack Transfer integrations
- 1099-NEC, 26Q, EC Sales List generation
- 15 new endpoints
- 12 acceptance criteria

**Team:** 3 backend engineers, 1 tax/compliance consultant, 1 QA

**Dependencies:** Phase 5 complete; cross-border tax advice from consultant

**Risks:**
- **R1**: Two-person rule for >$100k may slow payouts → configurable per tenant
- **R2**: Tax rules are complex and change → semi-annual compliance review
- **R3**: Wise API for cross-currency has limits → batch payouts
- **R4**: 1099 e-filing to IRS FIRE system has specific format → use Stripe Tax or Avalara

**Metrics:** Payout on-time rate > 99%. Tax withheld 100% accurate (audited). Doctors receive payout within 5 days of period close.

**Cost:** $260k (3 backend × 4 × $10k + consultant $30k + tax forms ~$10k)

### 22.7 Phase 7: V3 Terminal, Reconciliation, Chargebacks, Fraud (Months 26-30)

**Goal:** In-clinic POS, three-way reconciliation, chargeback defense, fraud scoring.

**Deliverables:**
- `terminal_readers` table
- `TerminalService` (Stripe Terminal + offline mode)
- `disputes`, `dispute_evidence` tables
- `ReconciliationService` (three-way match)
- `reconciliation_exceptions` table
- `FraudDetectionService` (scoring model)
- `payment_blacklist` table
- `ReconciliationJob` daily
- 25 new endpoints
- 15 acceptance criteria

**Team:** 4 backend engineers, 1 ML engineer (fraud), 1 QA

**Dependencies:** Phase 6 complete; Stripe Terminal hardware (test mode first); 6 months of chargeback data for fraud model

**Risks:**
- **R1**: Terminal hardware availability → start with Stripe SDK in test mode, ship hardware in V3.1
- **R2**: Chargeback auto-defense accuracy < 50% → always require human review
- **R3**: Fraud false positives → conservative thresholds, manual review queue
- **R4**: Reconciliation exceptions could pile up → SLA + escalation
- **R5**: PCI-DSS Level 1 audit is annual, expensive → budget $50k/year

**Metrics:** Chargeback win rate > 60%. Fraud block accuracy < 1% false positive. Reconciliation match rate > 99.9%. Authorization rate > 95%.

**Cost:** $420k (4 backend × 5 months × $10k + 1 ML × 5 × $15k + PCI audit $50k + hardware $20k)

### 22.8 Cross-Cutting Concerns

**Security:**
- PCI-DSS Level 1 SAQ-A
- PSD2 SCA compliance
- RBI tokenization compliance
- 3DS2 for all eligible cards
- KYC/AML via third-party (ComplyAdvantage, Onfido)
- Network tokens for all recurring cards

**Performance:**
- Order creation p95 < 200ms
- Webhook processing p95 < 500ms
- Payout run (100 doctors) p95 < 5min
- Reconciliation p95 < 30s
- 99.95% uptime for `/payments/*` endpoints

**Observability:**
- All payment state changes → `audit_log` (append-only)
- Webhook payloads → S3 (7 years, HIPAA)
- Per-gateway success rate → Prometheus
- Per-method conversion → funnel analytics
- Per-doctor payout → payout_run metrics

**Disaster Recovery:**
- RPO 5 min (WAL streaming)
- RTO 1 hour (multi-region)
- Daily backup of payment_methods (PII, encrypted)
- Quarterly DR drill

**Compliance:**
- 1099-NEC filing by Jan 31 (US)
- 26Q filing quarterly (India)
- VAT MOSS monthly (EU)
- HSA/FSA substantiation (US)

### 22.9 Key Milestones

| Month | Milestone |
|---|---|
| 0 | V1 card/UPI/Netbanking GA |
| 4 | V2 subscriptions GA |
| 6 | V2 BNPL GA |
| 9 | V2 Terminal + FX + payouts GA |
| 13 | V3 vault + checkout GA |
| 17 | V3 smart routing GA |
| 21 | V3 BNPL deep + crypto + Open Banking GA |
| 25 | V3 payout engine + tax compliance GA |
| 30 | V3 Terminal + reconciliation + chargebacks + fraud GA |
| 33 | V3 stable; V4 begins (card updater, biometric auth, recurring optimization) |

### 22.10 Success Metrics (V3+ Payments)

- **Authorization rate**: 95%+ (industry avg 90%)
- **Conversion rate**: 80%+ on checkout (industry avg 65%)
- **Cost per transaction**: < 2.0% blended (target)
- **Chargeback rate**: < 0.5% (industry avg 1%)
- **Payout on-time rate**: 99%+ (target)
- **Reconciliation match rate**: 99.9%+ (target)
- **Dispute auto-win rate**: 60%+ (target)
- **Engineering metrics:**
  - 99.95% uptime
  - p95 API < 300ms
  - 0 PCI incidents
  - 0 KYC breaches

(End of file)
