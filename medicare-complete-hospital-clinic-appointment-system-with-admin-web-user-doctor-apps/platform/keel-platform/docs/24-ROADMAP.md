# 24 — Roadmap (V1, V2, V3, Beyond)

> Where we are, where we're going, and how we get there. Each release has a theme, a list of modules, and a target date (loose).

---

## 1. V1 — "Ship the Foundation" (Now → +6 months)

**Theme:** A complete, secure, multi-tenant appointment system that can be deployed for any single clinic or SaaS for many. Modern stack. Realtime where it matters.

### 1.1 Status

🔨 **In Progress** (target: GA in 6 months from start)

### 1.2 What's In

All 53 active modules in `01-MODULES-INDEX.md` (sections 1-15). The full architecture from `00-ARCHITECTURE.md` through `23-DEVELOPMENT-SETUP.md`.

Highlights:
- 4 codebases (Laravel API, Next.js web admin+patient, 2 Flutter apps)
- Multi-tenant with subdomain/path resolver
- Firebase Phone OTP + Sanctum
- 9-state appointment machine with realtime queue
- 3 payment gateways (Razorpay, Stripe, Paystack)
- FCM push + email + in-app notifications
- 31 notification templates, 8 languages
- HIPAA-grade audit logging
- Installer wizard
- Full CI/CD to Hetzner via Forge
- Sentry, Prometheus, BetterUptime, PagerDuty

### 1.3 What's Stubbed (V2 Schema Only)

- Pathology / Labs
- AI Chat Doctor Recommendation
- Blog Module
- Subscription Plans

These ship with tables but return 501. Feature flags hidden in UI.

### 1.4 Definition of Done for V1

- [ ] All 53 modules shipped, with feature parity to the original CodeCanyon Medicare.
- [ ] All acceptance criteria in module docs pass.
- [ ] 1 successful production deployment to a paying customer.
- [ ] 30 days of zero Sev 1 incidents in production.
- [ ] Documentation in `/docs` is up to date.
- [ ] Test coverage ≥ 80% for new code.
- [ ] Sonar quality gate passing.
- [ ] Third-party security review passed.

---

## 2. Post-V1 Release Lanes (V1.1, V1.2, V2, V2.1, V3, V4, V5+)

> **Canonical reference:** see **§19 (detailed module plan)** for the per-lane module list with MML@GA, team sizing, and outcome targets. This §2 is the **high-level summary** for executives and PMs.

V1 ships 57 modules. The 7 post-V1 release lanes add **54 more** (12 V1.1 + 8 V1.2 + 11 V2 + 6 V2.1 + 10 V3 + 6 V4 + 1 V5+), taking the platform to **111 modules by V4**. Each lane ships **opt-in** via feature flag; no V1 tenant is forced to upgrade. Backward compatibility is sacred: no breaking API changes V1→V5.

### 2.1 V1.1 — "Polish & performance" (Month 0 → +2)

**Theme:** the 12 most-requested "small" features that make V1 great. Patient-experience improvements (recurring / group / family / waitlist), marketing reach (WhatsApp / vouchers), and per-tenant policy (refund window / i18n / payout / rotating doctors / PWA offline).

**12 modules:** `whatsapp-business` (MML 4), `recurring-appointments` (4), `group-appointments` (4), `family-booking` (4), `multi-currency-payout` (3), `geo-search` (4), `marketing-vouchers` (3), `dynamic-refund-window` (3), `smart-waitlist` (3), `i18n-rtl` (4), `doctor-rotating-schedule` (3), `pwa-offline-mode` (4). See **§19.1** for the per-module plan, team sizing (2 BE / 1 FE / 1 Mobile / 1 Designer / 1 PM), and outcome (50 → 200 paying customers, NPS > 50).

### 2.2 V1.2 — "Native integrations" (Month +2 → +4)

**Theme:** wire up the regional / national integrations that block enterprise deals. India (ABDM), US (FHIR R4), Africa (Paystack), global (Stripe Terminal, Twilio), chains (multi-clinic dashboard).

**8 modules:** `abdm-bridge` (3), `fhir-r4-read` (3), `razorpay-subscriptions` (3), `paystack-subscriptions` (3), `stripe-terminal` (4), `twilio-sms` (4), `local-payment-methods` (3), `multi-clinic-dashboard` (3). See **§19.2** for the per-module plan, team sizing (3 BE / 1 FE / 1 Mobile / 1 DevOps / 1 PM), and outcome (200 → 500 customers, first enterprise hospital).

### 2.3 V2 — "Platform layer" (Month +4 → +7)

**Theme:** turn the clinic app into a platform with intelligent features. Pathology (the marquee), telemedicine (LiveKit), AI scribe + symptom checker, in-app chat, plugins v1, custom domain, multi-region DR, drug-interaction DB, e-prescription, insurance pre-auth.

**11 modules:** `pathology-lab` (4), `telemedicine-video` (4), `ai-symptom-checker` (3), `ai-clinical-scribe` (3), `in-app-chat` (4), `plugins-v1` (3), `custom-domain` (3), `multi-region-dr` (3), `drug-interaction-db` (4), `e-prescription` (3), `insurance-preauth` (3). See **§19.3** for the per-module plan, team sizing (5 BE / 2 FE / 2 Mobile / 1 AI-ML / 2 DevOps / 1 Security / 1 Designer / 1 PM / 1 QA), and outcome (500 → 1,000 customers, $100K MRR, first hospital chain).

### 2.4 V2.1 — "Insurance & revenue cycle" (Month +7 → +10)

**Theme:** help hospitals get paid faster. RCM (denial mgmt, dunning, A/R aging), plugin monetization, on-prem LLM, SOC 2 Type II start, BYOK, field-level encryption.

**6 modules** (per `14-MODULE-V2-STUBS §3.4a`): `revenue-cycle-mgmt` (3), `saas-plan-marketplace` (4), `on-prem-llm-gateway` (3), `soc2-type-ii` (3), `byok-encryption` (3), `field-level-encryption` (3). See **§19.4** for the detailed plan including 3 aspirational V2.1 extras (`usage-based-pricing`, `differential-privacy`, `audio-transcription`) not in the 6-module V2.1 catalog, team sizing (4 BE / 1 FE / 1 DevOps / 1 Security / 1 PM / 1 QA), and outcome (1,000 → 1,500 customers, $200K MRR).

### 2.5 V3 — "Marketplace & ecosystem" (Month +10 → +15)

**Theme:** open the platform for third-party innovation. Public plugin marketplace, HL7 v2 ADT feeds, FHIR R4 write, DICOM viewer, on-prem AI, custom report builder, white-label mobile, SSO, bed / OT / equipment booking for hospital clinics.

**10 modules:** `plugin-marketplace` (3), `hl7-v2-adt` (3), `fhir-r4-write` (3), `dicom-viewer` (3), `custom-report-builder` (3), `white-label-mobile` (3), `sso-oidc-saml` (3), `bed-management` (3), `ot-room-scheduling` (3), `equipment-booking` (3). See **§19.5** for the per-module plan, team sizing (6 BE / 2 FE / 2 Mobile / 2 DevOps / 1 Security / 1 PM / 2 QA / 1 GTM), and outcome (1,500 → 3,000 customers, $500K MRR, first plugin partners).

### 2.6 V4 — "Hospital ITOps" (Month +15 → +22)

**Theme:** become a credible hospital information system. LIS ASTM instrument interface, HIS bridge (Epic / Cerner / Meditech), ER triage board, nursing station (vitals / MAR), centralized pharmacy + narcotic logs, equipment maintenance.

**6 modules:** `lis-astm` (3), `his-bridge` (3), `er-triage` (3), `nursing-station` (3), `centralized-pharmacy` (3), `equipment-maintenance` (3). Plus 2 compliance milestones: ISO 27001 + HITRUST CSF certification start. See **§19.6** for the per-module plan, team sizing (8 BE / 3 FE / 3 Mobile / 3 DevOps / 1 Security / 1 PM / 3 QA / 1 Clinical SME / 1 GTM), and outcome (3,000 → 5,000 customers, $1M MRR).

### 2.7 V5+ — "Population health" (Month +22+, designed not scheduled)

**Theme:** aggregate, de-identify, analyze. HIPAA Safe Harbor de-identification, research cohort tooling, public health reporting, homomorphic encryption for encrypted cohort queries, federated learning.

**1 module in catalog + 4 aspirational:** `population-health` (MML 1, per `14-MODULE-V2-STUBS §3.7` and `06-MODULE-PATIENTS §17.7`). Plus 4 aspirational per **§19.7**: `de-identification-pipeline`, `public-health-reporting`, `homomorphic-encryption`, `federated-learning` (all MML 1, designed only).

### 2.8 What this means in product terms

By V3, the platform is no longer just "Medicare Clinic" — it is "Medicare Hospital" + "Medicare Platform" with a third-party plugin ecosystem. The shared core (tenancy, auth, RBAC, audit, payments) is V1-stable; everything above is additive. From V1 GA → V5, no breaking API changes: deprecations are announced 12 months in advance and removed only at the next major lane (V6).

---

## 6. The Tech Debt Backlog (Continuous)

Things we want to fix but never have time:

| Item | Severity | Notes |
|---|---|---|
| Move from bigint to ULID | low | ID churn is annoying |
| Move money to decimal | low | Cosmetic |
| Replace Spatie MediaLibrary with custom | low | Spatie is fine, just custom for our use case |
| Add OpenTelemetry tracing | medium | We have logs, need traces |
| Switch Reverb to Centrifugo | low | Benchmarking |
| Self-host Sentry (GlitchTip) | low | Save $26/mo |
| Self-host email (Postal) | low | For on-prem customers |
| Replace Pinia with Zustand in admin | low | Personal preference |
| Move from Vite to Turbopack | low | Stability |
| Auto-generate TypeScript types from OpenAPI | medium | Currently hand-typed |

---

## 7. The Product Wishlist (Customer-Driven)

A live doc of "things customers have asked for, ranked by votes":

- Multi-doctor group video call (1)
- Native iPad app for doctor (3)
- Apple Watch app for "next patient" alert (0)
- Family member health summary report (5)
- Pharmacy delivery integration (8)
- "Repeat last prescription" button (12) — already built, but expose
- Custom patient intake forms (7)
- "Annual health checkup" package (2)
- Pre-visit questionnaire (15) — high demand
- WhatsApp booking (25) — high demand
- Direct bank transfer for payment (V2 — Razorpay handles)
- Loyalty / referral program (4)
- Patient app widget for iOS home screen (1)

---

## 8. The Non-Goals (Things We Will Not Build)

Disciplined focus:

- **Telemedicine hardware** (we integrate with Zoom/Daily, don't build cameras).
- **Pharmacy operations** (we integrate with pharmacies, don't manage inventory).
- **Hospital ERP** (V3 maybe; not core).
- **Medical devices** (BP monitors, etc. — we accept data via API).
- **Custom AI models** (we use OpenAI/Anthropic APIs).
- **Patient social network** (no forums, no "find a friend with diabetes").
- **Doctor-to-doctor chat** (out of scope V1-V3; V4 maybe).
- **Mental health therapy marketplace** (different product).

---

## 9. The Pricing Model (Likely)

| Tier | Price | Includes |
|---|---|---|
| **Free** | $0 | 1 doctor, 50 appointments/month, watermarked "Powered by Medicare" |
| **Starter** | $49/mo | 3 doctors, 500 appointments/month, no branding |
| **Pro** | $149/mo | 10 doctors, unlimited appointments, custom domain, priority support |
| **Enterprise** | Custom | Unlimited, dedicated instance, SLA, on-prem option |

For multi-tenant SaaS, the clinic pays us; they handle their own patient billing.

---

## 10. The Go-To-Market (Likely)

- **Phase 1 (V1)**: Direct sales to small clinics in India + US. 10 customers.
- **Phase 2 (V1.1)**: Reseller partnerships with local IT vendors. 100 customers.
- **Phase 3 (V2)**: Self-serve signup. Marketing site with video walkthroughs.
- **Phase 4 (V2.1)**: Hospital chains. Enterprise sales.

---

## 11. The Team Roadmap

| Phase | Size | Roles |
|---|---|---|
| V1 | 4 | 1 backend, 1 web, 1 mobile, 1 DevOps/QA |
| V1.1 | 6 | + 1 designer, 1 product |
| V2 | 10 | + 1 AI/ML, 1 integrations, 1 sales |
| V3 | 20 | Hospital specialists, sales engineers, customer success |

---

## 12. The Risks

| Risk | Impact | Mitigation |
|---|---|---|
| HIPAA breach | Company-ending | All controls in place, daily drills, bug bounty |
| Payment gateway de-platforms us | Revenue stops | 3 gateways, fallbacks |
| Firebase Phone Auth blocked in a country | Login broken in that country | Twilio fallback ready |
| Zoom raises prices | Costs go up | Daily.co is a 1-week swap |
| AWS outage | All customers down | Cross-region failover (deferred to V2.1) |
| Key developer leaves | Velocity drops | Documentation, pair programming, bus factor ≥ 2 per module |
| Original CodeCanyon product ships a feature we don't have | Lose deals | We have 5+ differentiators (realtime, multi-tenant, modern stack, mobile+web parity, installer) |
| Competitor (e.g., Practo, Zocdoc) raises big round | Price war | We're 10x cheaper, open-source-friendly, white-label |

---

## 13. The Metrics We Track

### 13.1 Product

- Daily active clinics
- Bookings / day
- Revenue / month
- Churn rate
- NPS

### 13.2 Engineering

- Deployment frequency (target: daily)
- Lead time for changes (target: <1 day)
- Mean time to recovery (target: <1 hour)
- Change failure rate (target: <5%)
- Test coverage (target: >80%)
- Sev 1 incidents / month (target: <2)

### 13.3 Business

- Customer acquisition cost
- Lifetime value
- Payback period
- Gross margin

---

## 14. Quarterly Themes (Likely)

| Quarter | Theme |
|---|---|
| Q1 (V1 GA) | Ship V1, on-board 10 customers |
| Q2 (V1.1) | Polish based on feedback, performance, multi-language |
| Q3 (V2 alpha) | Pathology + AI symptoms internal alpha |
| Q4 (V2 beta) | Pathology + AI symptoms beta with 3 customers |
| Q1 next year (V2 GA) | Ship V2, on-board 50 customers |
| Q2 (V2.1) | Compliance, FHIR, ePrescription |
| Q3 (V3 alpha) | IPD / OT / Pharmacy internal alpha |
| Q4 (V3 beta) | Hospital beta with 1 customer |

---

## 15. How to Read This

- "V1" / "V2" / "V3" are major releases, each 6-12 months apart.
- "V1.1" / "V2.1" are minor releases, 3 months after the major.
- "V1.1.x" are patches, as needed.
- Items in the **Wishlist** are not committed; they get prioritized quarterly.
- Items in **Tech Debt** are continuous, picked up between features.

---

## 16. What Success Looks Like

**V1:** 10 paying customers, $5K MRR, 0 Sev 1 incidents per quarter.

**V2:** 100 paying customers, $50K MRR, 1.5 FTE customer support.

**V3:** 500 paying customers (mix of clinics + hospitals), $500K MRR, 10 FTE team, 24/7 on-call, HIPAA + SOC 2 + ISO 27001.

**Beyond:** Acquisition by a larger healthcare tech company, or IPO as the "open-source Epic alternative". Both are real outcomes we plan for.

---

## 17. Open Questions

- Do we want a mobile-first admin (or web-only)?
- Do we want to support on-prem / air-gapped installs in V1? (We have the infra, but the docs/support burden is real.)
- Do we want to build our own payment gateway aggregation (like Stripe Connect for healthcare)?
- Do we want to enter the US market in V1 or wait for V2?

These get answered in the next architecture review (monthly).

---

## 18. The Promise

We will:
- Ship V1 on time.
- Keep the documentation up to date.
- Never knowingly ship insecure code.
- Listen to customers, but not be led by the loudest one.
- Stay focused on the small-clinic market until we've won it, then expand.
- Keep the team small and happy.
- Have fun.

This roadmap is a living document. Updated monthly.

---

## 19. V2 Roadmap (the detailed plan)

> V1 is shipped (Month 0). The following lanes detail what's coming, in what order, with what resources.

### 19.1 V1.1 — Polishing lane (Month 0 to Month +2)

**Theme:** Ship the most-requested "small" features.

**Features:**
- `whatsapp-business` (MML 4)
- `recurring-appointments` (MML 4)
- `group-appointments` (MML 4)
- `family-booking` (MML 4)
- `multi-currency-payout` (MML 3)
- `geo-search` (MML 4)
- `marketing-vouchers` (MML 3)
- `dynamic-refund-window` (MML 3)
- `smart-waitlist` (MML 3)
- `i18n-rtl` (MML 4)
- `doctor-rotating-schedule` (MML 3)
- `pwa-offline-mode` (MML 4)

**Team:** 2 BE, 1 FE, 1 Mobile, 1 Designer, 1 PM.
**Outcome:** 50 → 200 paying customers. NPS >50.

### 19.2 V1.2 — Native integrations lane (Month +2 to Month +4)

**Theme:** Wire up the regional / national integrations that block enterprise deals.

**Features:**
- `abdm-bridge` (MML 3) — needed for Indian enterprise hospitals.
- `fhir-r4-read` (MML 3) — needed for US hospital integrations.
- `razorpay-subscriptions` (MML 3) — recurring billing for SaaS.
- `paystack-subscriptions` (MML 3) — recurring for Africa.
- `stripe-terminal` (MML 4) — in-person card at the front desk.
- `twilio-sms` (MML 4) — SMS fallback and reminders.
- `local-payment-methods` (MML 3) — UPI Autopay, GCash, M-Pesa.
- `multi-clinic-dashboard` (MML 3) — chain admin.

**Team:** 3 BE, 1 FE, 1 Mobile, 1 DevOps, 1 PM.
**Outcome:** 200 → 500 customers. First enterprise hospital.

### 19.3 V2 — Platform lane (Month +4 to Month +7)

**Theme:** Turn the clinic app into a platform with intelligent features.

**Features:**
- `pathology-lab` (MML 4) — the marquee feature.
- `telemedicine-video` (MML 4) — LiveKit integration.
- `ai-symptom-checker` (MML 3) — OpenAI/Anthropic integration.
- `ai-clinical-scribe` (MML 3) — Whisper + GPT-4o.
- `in-app-chat` (MML 4) — patient↔doctor chat.
- `plugins-v1` (MML 3) — first-class plugin system.
- `custom-domain` (MML 3) — tenant branding.
- `multi-region-dr` (MML 3) — DR drill.
- `drug-interaction-db` (MML 4) — RxNav integration.
- `e-prescription` (MML 3) — ABDM signing.
- `insurance-preauth` (MML 3) — first payer integration.

**Team:** 5 BE, 2 FE, 2 Mobile, 1 AI/ML, 2 DevOps, 1 Security, 1 Designer, 1 PM, 1 QA.
**Outcome:** 500 → 1,000 customers. First hospital chain. $100K MRR.

**Milestones:**
- M+5: ABDM M1 certification.
- M+5.5: SOC 2 Type I audit complete.
- M+6: HIPAA BAA template.
- M+6.5: First pathology lab partner live.
- M+7: V2 GA, public launch.

### 19.4 V2.1 — Insurance & revenue cycle lane (Month +7 to Month +10)

**Theme:** Help hospitals get paid faster.

**Features:**
- `revenue-cycle-mgmt` (MML 3) — denial management, dunning, E&O.
- `saas-plan-marketplace` (MML 4) — plugin billing.
- `on-prem-llm-gateway` (MML 3) — vLLM for HIPAA/EU-BAIC.
- `soc2-type-ii` (MML 3) — start the 12-month observation period.
- `byok-encryption` (MML 3) — enterprise data sovereignty.
- `field-level-encryption` (MML 3) — Aadhaar, SSN, PAN.
- `usage-based-pricing` (MML 4) — AI tokens, add-ons.
- `differential-privacy` (MML 3) — for analytics.
- `audio-transcription` (MML 3) — for consult audio.

**Team:** 4 BE, 1 FE, 1 DevOps, 1 Security, 1 PM, 1 QA.
**Outcome:** 1,000 → 1,500 customers. $200K MRR.

### 19.5 V3 — Marketplace & ecosystem lane (Month +10 to Month +15)

**Theme:** Open the platform for third-party innovation.

**Features:**
- `plugin-marketplace` (MML 3) — public marketplace with revenue share.
- `hl7-v2-adt` (MML 3) — hospital integration.
- `fhir-r4-write` (MML 3) — bi-directional EHR sync.
- `dicom-viewer` (MML 3) — embedded OHIF viewer.
- `custom-report-builder` (MML 3) — Metabase-style.
- `white-label-mobile` (MML 3) — tenant-branded apps.
- `sso-oidc-saml` (MML 3) — enterprise auth.
- `bed-management` (MML 3) — hospital wards.
- `ot-room-scheduling` (MML 3) — operation theatres.
- `equipment-booking` (MML 3) — X-ray, ultrasound slots.

**Team:** 6 BE, 2 FE, 2 Mobile, 2 DevOps, 1 Security, 1 PM, 2 QA, 1 GTM.
**Outcome:** 1,500 → 3,000 customers. $500K MRR. First plugin partners.

### 19.6 V4 — Hospital ITOps lane (Month +15 to Month +22)

**Theme:** Become a credible hospital information system.

**Features:**
- `lis-astm` (MML 3) — lab instrument integration.
- `his-bridge` (MML 3) — Epic, Cerner, Meditech bridge.
- `er-triage` (MML 3) — emergency department board.
- `nursing-station` (MML 3) — vitals charting, MAR.
- `centralized-pharmacy` (MML 3) — narcotic logs.
- `equipment-maintenance` (MML 3) — PM tracking.
- `iso-27001` — start certification.
- `hitrust-csf` — start certification.

**Team:** 8 BE, 3 FE, 3 Mobile, 3 DevOps, 1 Security, 1 PM, 3 QA, 1 Clinical SME, 1 GTM.
**Outcome:** 3,000 → 5,000 customers. $1M MRR.

### 19.7 V5 — Population health lane (Month +22+)

**Theme:** Aggregate, de-identify, analyze.

**Features:**
- `de-identification-pipeline` (MML 1) — HIPAA Safe Harbor.
- `research-cohort-tooling` (MML 1) — find patients matching criteria.
- `public-health-reporting` (MML 1) — automated disease surveillance.
- `homomorphic-encryption` (MML 1) — encrypted cohort queries.
- `federated-learning` (MML 1) — train models without sharing data.

**Team:** 4 BE, 2 BE-AI, 1 DevOps, 1 Security, 1 PM, 1 Clinical SME.
**Outcome:** 5,000+ customers. $2M MRR. Population health partnerships.

### 19.8 Resource & cost plan

| Lane | Duration | Team | Burn rate | Cumulative cost |
|---|---|---|---|---|
| V1 | 0–6 mo | 8 | $80K/mo | $480K |
| V1.1 | 0–2 mo (parallel) | +3 | +$30K/mo | +$60K |
| V1.2 | +2–4 mo (parallel) | +3 | +$30K/mo | +$60K |
| V2 | +4–7 mo | +6 | +$60K/mo | +$180K |
| V2.1 | +7–10 mo | +3 | +$30K/mo | +$90K |
| V3 | +10–15 mo | +8 | +$80K/mo | +$400K |
| V4 | +15–22 mo | +10 | +$100K/mo | +$700K |
| V5 | +22+ mo | +5 | +$50K/mo | ongoing |

**Total team by V4:** 30 FTE.
**Total burn by V4:** ~$2M.
**Revenue by V4:** $1M MRR ($12M ARR).
**Gross margin by V4:** 75%.
**Break-even:** Month +18 (estimated).

### 19.9 Risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| ABDM certification delayed | High | Med | Start early, M1 by M+5 |
| LLM cost overruns | High | Med | Caching, smaller models, on-prem option |
| Stripe/Paystack policy change | Low | High | Multi-gateway, no lock-in |
| LiveKit outage | Med | High | Daily.co + Jitsi fallback |
| Customer churn from feature gap | Med | High | Monthly NPS, quarterly customer interviews |
| Security breach | Low | Very high | SOC 2, regular pen tests, $5M cyber insurance |
| Key person dependency | Med | High | Pair programming, runbooks, on-call rotation |
| AWS outage | Med | Med | Multi-region, auto-failover |
| AI hallucination harms patient | Med | Very high | Human-in-the-loop, red-team, disclaimers |
| Competitor (Practo, Zocdoc) | Med | Med | Differentiate on depth + OSS + multi-tenant |

### 19.10 Success metrics per lane

| Lane | Metric | Target |
|---|---|---|
| V1 | NPS | >50 |
| V1.1 | DAU/MAU | >40% |
| V1.2 | Enterprise customers | 10 |
| V2 | Pathology orders | 1k/mo |
| V2 | Telemedicine consults | 10k/mo |
| V2 | AI scribe usage | 1k doctors |
| V2.1 | Insurance claims processed | 100k |
| V2.1 | ARR | $2.4M |
| V3 | Plugin partners | 50 |
| V3 | Marketplace revenue | $100K/mo |
| V4 | Hospital customers | 100 |
| V4 | ARR | $12M |
| V5 | Population health customers | 5 (govts, NGOs) |

### 19.11 The "no" list (we will NOT build these)

- **Inpatient EMR.** We integrate with Epic/Cerner/Meditech; we don't replace them.
- **PACS.** We integrate with DICOM viewers; we don't store imaging.
- **Pharmacy dispensing.** We send Rx to pharmacies; we don't dispense.
- **Lab analyzers.** We integrate via ASTM; we don't manufacture.
- **Hospital revenue cycle beyond insurance.** We do claims + denials; we don't do general accounting.
- **Medical device manufacturing.** Out of scope.
- **Patient community / social network.** Reddit, Facebook exist.
- **Health insurance.** We're not a payer.
- **Telemedicine to third-party doctors.** We support patient↔doctor in-tenant; not cross-tenant.

These are tempting; saying no keeps us focused.

### 19.12 The "yes, but later" list

- **Voice-first interface (Alexa, Google Home):** V4+ once core platform is stable.
- **Wearables integration (Apple Watch, Fitbit):** V3 if customer demand materializes.
- **Mental health specialty features:** V3, after V2 is GA.
- **Veterinary:** Out of scope; someone else can fork.
- **Dental:** V3, separate UI variant.
- **Ayurveda / homeopathy:** V3, separate module.
- **Clinical trials module:** V4+ if research demand grows.

### 19.13 V2 acceptance criteria (rolled up)

- [ ] All V2 features reach MML ≥ 3 within their target lane.
- [ ] No V1 customer is broken by a V2 upgrade.
- [ ] V2 SaaS can be provisioned in <30 min via the installer.
- [ ] V2 gross margin is ≥ 70%.
- [ ] V2 NPS is ≥ 60.
- [ ] V2 SLA is 99.95% for paid plans, 99.5% for free.
- [ ] V2 is SOC 2 Type I certified.
- [ ] V2 is HIPAA, GDPR, DPDP compliant.
- [ ] V2 supports English, Hindi, Arabic, Spanish, French, German, Portuguese, Mandarin, Japanese.
- [ ] V2 supports INR, USD, EUR, GBP, AED, SGD, AUD, CAD, JPY, BRL currencies.

---

## 20. Per-Module MML 1→5 Build Plans

> The 59 post-V1 modules are at **MML 1 (designed)** as of this doc. This section is the **per-module build plan** that takes each module through MML 2 (schema) → MML 3 (internal API + admin UI) → MML 4 (patient/doctor API + clients) → MML 5 (GA, SLA-backed, observability live, runbook published). Each level has a concrete deliverable, exit criteria, and dependencies. Owners are the guild leads. ETAs are working days from the start of the lane (per `§2.1-§2.7`).
>
> **MML definitions** (per `14-MODULE-V2-STUBS §3`):
> - **MML 1 (designed):** this doc + the deep-dive is the only artifact.
> - **MML 2 (schema):** migrations landed behind `feature_*` flag; no API yet.
> - **MML 3 (internal API + admin UI):** tenant-admin can configure / view; no patient/doctor API.
> - **MML 4 (patient/doctor API + Flutter/Next.js):** full E2E working; behind feature flag; not SLA-backed.
> - **MML 5 (GA):** feature flag removed; SLA-backed; observability live; runbook published; PagerDuty service exists; load-tested; security-reviewed; on-call rotation set.

### 20.1 V1.1 lane build plan (12 modules, target MML@GA per `§2.1`)

| # | Module | MML@GA | MML 1→2 (Schema) | MML 2→3 (Internal API) | MML 3→4 (Patient/Doctor API) | MML 4→5 (GA) | Owner | Total ETA |
|---|---|---|---|---|---|---|---|---|
| 58 | `whatsapp-business` | 4 | 3d: `whatsapp_conversations` + `whatsapp_template_submissions` + `whatsapp_webhook_events` migrations | 5d: `WhatsappTemplateService` + `SendWhatsAppTemplateJob` + 21 template categories; admin composer at `/admin/notifications/whatsapp` | 4d: patient inbound webhook → in-app chat; quality rating dashboard; 24h window enforcement | 3d: Meta Cloud API production onboarding; quality rating SLO ≥ green; cost tracking live | Notifications guild | 15d |
| 59 | `recurring-appointments` | 4 | 2d: `appointment_series` + add `parent_appointment_id` + `rrule` to `appointments` | 4d: `RecurringAppointmentService::expand()` with RFC 5545 RRULE via `rlanvin/php-rrule`; 60-day lazy window + 90-day nightly expander | 4d: patient web UI for "make this recurring"; admin calendar RRULE view; iCal export of series | 2d: load-test 1k expansions; idempotency dedup race; DST correctness verified | Appointments guild | 12d |
| 60 | `group-appointments` | 4 | 2d: add `is_group` + `group_size` + `booked_count` to `appointments`; partial unique index | 4d: `GroupAppointmentService` with `SELECT FOR UPDATE` row lock; per-participant outcomes; single LiveKit room | 3d: patient "join group" UI; admin per-participant outcome entry | 2d: load-test 100 concurrent joins | Appointments guild | 11d |
| 61 | `family-booking` | 4 | 2d: `family_relationships` + `family_consent_scopes` migrations | 3d: `FamilyBookingService` with OTP verification per dependent; minor-mode + age-out at 18 | 3d: patient "book for family" UI; consent scope picker | 2d: minor age-out test; OTP fail test | Patients guild | 10d |
| 62 | `multi-currency-payout` | 3 | 2d: `doctor_payout_methods` + `payouts` + `payout_runs` + `doctor_tax_forms` | 5d: `GeneratePayoutRunJob` (weekly/bi-weekly/monthly); FX at payout time; 5 gateway methods; TDS/1099/VAT/GST | — (MML 3 is GA; patient/doctor API not needed beyond read-only `GET /payouts/me`) | 2d: 25-currency FX test; gateway failover test | Payments guild | 9d |
| 63 | `geo-search` | 4 | 2d: PostGIS extension; `doctors.geo` POINT(4326) + GiST index; `geo_lat`/`geo_lng` decimals | 3d: `DoctorGeoSearch` service; 8 filters; Mapbox tile precompute nightly; Haversine fallback | 4d: patient "find doctor near me" UI; map view with markers; "use my location" prompt | 2d: load-test 10k doctors; non-PostGIS tenant fallback test | Doctors guild | 11d |
| 64 | `marketing-vouchers` | 3 | 3d: `vouchers` + `voucher_redemptions` + `referral_credits` + `partner_pools` + `influencer_commissions` | 5d: `VoucherService::validate()`; `ApplyVoucher` job; referral attribution; partner billing; influencer KYC + payout | — (MML 3 is GA; admin UI only) | 2d: fraud detection test; multi-account test | Billing guild | 10d |
| 65 | `dynamic-refund-window` | 3 | 2d: `refund_policies` + `refund_window_calculations` | 4d: `RefundService::computeAndCreate` extended with per-service / per-doctor / per-segment lookup; sliding scale eval; 4 resolution types | — (MML 3 is GA; admin UI only) | 2d: priority resolution test; force majeure test | Appointments guild | 8d |
| 66 | `smart-waitlist` | 3 | 2d: `waitlist_preferences` + `waitlist_offers` + `waitlist_cascades` | 5d: `MatchScoreCalculator`; `OfferWaitlistSlotJob` (extended); cascade tick; group + recurring waitlist | — (MML 3 is GA; patient-facing claim/decline API exists; admin UI only) | 2d: cascade race test; quiet hours test | Appointments guild | 9d |
| 67 | `i18n-rtl` | 4 | 2d: `locale_profiles` + `translation_memory` + `translation_glossaries` + `translation_jobs` | 3d: `LocaleFormatter` service; per-locale ICU patterns; translation management UI; auto-translate via DeepL/Google/OpenAI | 3d: patient language switcher; 10 GA locales shipped; RTL pipeline (HTML dir, CSS logical properties, Flutter Directionality, mirrored icons/charts/PDFs) | 3d: visual regression tests for all 10 locales; load-test translation memory | Engagement guild | 11d |
| 68 | `doctor-rotating-schedule` | 3 | 2d: `rotation_groups` + `rotation_assignments` + `rotation_swaps` + `rotation_escalations` + `coverage_gaps` | 5d: `RotationPlanGenerator` (deterministic, idempotent); `EscalationService` (5-min timeout); fairness calculator; coverage gap detector | — (MML 3 is GA; admin UI for rotation groups; patient API for after-hours call) | 2d: escalation chain test; fairness drift test; coverage gap test | Doctors guild | 9d |
| 69 | `pwa-offline-mode` | 4 | 2d: `pwa_subscriptions` + `pwa_queued_actions` + `pwa_app_versions` + `pwa_storage_stats` | 3d: Workbox 7 service worker; 4 cache strategies; IndexedDB queue; Background Sync; Web Push via VAPID | 4d: patient PWA install prompt; offline check-in UI; conflict resolution card; new version banner | 3d: storage budget enforcement; load-test 1k concurrent syncs; visual regression | Mobile guild | 12d |

**V1.1 lane total:** 12 modules × ~10d avg = **~120 engineer-days**. 6-person team: 2 BE / 1 FE / 1 Mobile / 1 Designer / 1 PM = 12 calendar weeks (per `§19.1` team sizing + outcome).

### 20.2 V1.2 lane build plan (8 modules, target MML@GA per `§2.2`)

| # | Module | MML@GA | MML 1→2 (Schema) | MML 2→3 (Internal API) | MML 3→4 (Patient/Doctor API) | MML 4→5 (GA) | Owner | Total ETA |
|---|---|---|---|---|---|---|---|---|
| 70 | `abdm-bridge` | 3 | 3d: `abha_links` + `abdm_consent_artifacts` + `abdm_hip_push_queue` + `abdm_hiu_pull_log` + `abdm_cert_status` | 6d: `AbdmService` (NHA OAuth 2.0 + mTLS); ABHA 14-digit verification; HIU pull (24-month); HIP push (visit/Rx/lab); ConsentArtifact; ABDM cert M1/M2/M3 tracking | — (MML 3 is GA; admin UI + patient PHR link) | 3d: ABDM M1 certification; mTLS cert rotation; failover to sandbox | Integrations guild | 12d |
| 71 | `fhir-r4-read` | 3 | 2d: HAPI-FHIR sidecar schema; `fhir_resource_cache` + `fhir_bulk_export_jobs` | 5d: HAPI-FHIR Java sidecar (port 8080, mTLS); OAuth 2.0 `client_credentials`; 10 resources; capability statement at `/api/v2/fhir/metadata`; bulk export (Premium) | — (MML 3 is GA; partner-facing API) | 3d: HAPI-FHIR load-test 1k req/s; OAuth token rotation; partner sandbox | Integrations guild | 10d |
| 72 | `razorpay-subscriptions` | 3 | 2d: `subscriptions` + `subscription_charge_attempts` + `subscription_credits` + `subscription_events` | 5d: `RazorpaySubscriptionService` (4 mandate types: card/UPI Autopay/eNACH/PayLater); 5 webhooks; dunning T+0/T+3/T+7/T+10/T+30; pause ≤ 3 mo; proration | — (MML 3 is GA; admin UI + tenant API) | 2d: 4 mandate type tests; dunning retry test; pause limit test | Payments guild | 9d |
| 73 | `paystack-subscriptions` | 3 | 2d: `paystack_subscriptions` (with `paystack_subscription_code` + `paystack_email_token`) | 4d: `PaystackSubscriptionService` (3 mandate types: card/bank/mobile-money); 7 webhooks; MML 3 GA for NG/GH/ZA/KE; disable-and-new-sub as pause | — (MML 3 is GA; admin UI + tenant API) | 2d: 3 mandate type tests; region-specific tests | Payments guild | 8d |
| 74 | `stripe-terminal` | 4 | 2d: `terminal_locations` + `terminal_readers` + `payment_intents` (extend) | 4d: `StripeTerminalService` (3 reader models: Wise Pad 3/Verifone P400/BBPOS WisePOS E); server-driven + mobile SDK; offline mode (50 txns/7d) | 3d: patient "pay at counter" UI; doctor "collect at desk" UI | 3d: reader hardware integration test; offline mode test; partial capture test | Payments guild | 12d |
| 75 | `twilio-sms` | 4 | 2d: `sms_phone_numbers` (with `regulatory` JSON) + `sms_messages` + `sms_opt_outs` | 3d: `TwilioService` (Verify API for OTP + Programmable Messaging); A2P 10DLC US + DLT India; 6 opt-out + 2 opt-in keywords; 2-way SMS chat | 3d: patient "verify phone" UI; admin "send SMS" UI; 2-way SMS thread in in-app chat | 2d: regulatory bundle test; A2P 10DLC TCR test; DLT template test | Notifications guild | 10d |
| 76 | `local-payment-methods` | 3 | 2d: `payment_method_types` + `tenant_payment_methods` | 5d: `LocalPaymentService` (12 methods: UPI Intent/Collect/Autopay/Paytm/NetBanking/EMI/Pix/Boleto/OXXO/GCash/M-Pesa/Paystack bank+QR); per-country routing; settlement T+0 to T+3 | — (MML 3 is GA; admin UI + tenant API) | 3d: 12 method tests; per-country routing test; settlement delay test | Payments guild | 10d |
| 77 | `multi-clinic-dashboard` | 3 | 3d: `tenants.parent_tenant_id` (chain hierarchy); `chain_users` + `chain_revenue_rules` + `chain_metrics` (materialized view) + `revenue_distributions` + `chain_reports` | 5d: `ChainDashboardService`; 3 admin roles (`chain_admin` / `region_admin` / `chain_auditor`); chain_admin cannot impersonate; per-clinic PII export requires co-sign | — (MML 3 is GA; admin UI only) | 3d: chain hierarchy test; impersonation block test; co-sign test; revenue split test | Admin guild | 11d |

**V1.2 lane total:** 8 modules × ~10d avg = **~80 engineer-days**. 6-person team: 3 BE / 1 FE / 1 Mobile / 1 DevOps / 1 PM = 8 calendar weeks (per `§19.2`).

### 20.3 V2 lane build plan (11 modules, target MML@GA per `§2.3`)

| # | Module | MML@GA | MML 1→2 (Schema) | MML 2→3 (Internal API) | MML 3→4 (Patient/Doctor API) | MML 4→5 (GA) | Owner | Total ETA |
|---|---|---|---|---|---|---|---|---|
| 78 | `pathology-lab` | 4 | 3d: `lab_orders` + `lab_order_items` + `lab_results` + `lab_reports` + `lab_partners` | 5d: `LabService` (LOINC-required order items; partner HMAC-SHA256; sample tracking; critical result page; e-Report PDF) | 4d: patient "view lab results" UI; doctor "order lab" UI in encounter; partner web portal | 3d: 3 lab partner integration; LOINC unmapped handling; critical result paging SLO 60s | Lab guild | 15d |
| 79 | `telemedicine-video` | 4 | 2d: `video_rooms` + `video_recordings` + `video_consents` | 5d: `VideoService` (LiveKit Cloud primary → Daily.co → Jitsi fallback); 2-party consent enforced; 90-day auto-delete; quality monitor | 3d: patient "join video" UI; doctor "start video" UI in encounter; in-call chat | 3d: LiveKit cluster failover; 2-party consent revoke test; recording consent | Telemedicine guild | 13d |
| 80 | `ai-symptom-checker` | 3 | 1d: `symptom_sessions` + `symptom_responses` + `symptom_triages` | 4d: `SymptomCheckerService` (OpenAI / Anthropic; red-flag detection; routing only — not diagnostic; privacy: no PHI in prompt) | 2d: patient pre-visit intake UI; doctor dashboard of recent sessions | 3d: red-flag detection accuracy test; rate-limit test; cost tracking | AI guild | 10d |
| 81 | `ai-clinical-scribe` | 3 | 2d: `scribe_sessions` + `scribe_transcripts` + `scribe_notes` + `scribe_consents` | 6d: `ScribeService` (Whisper STT + GPT-4o SOAP; 4 sections; 2-party consent; 60s withdraw-consent purge; doctor must edit ≥ 1 char/section before sign) | 3d: doctor "start scribe" UI in encounter; review + edit + sign | 3d: STT accuracy test; consent revoke test; sign-off audit | AI guild | 14d |
| 82 | `in-app-chat` | 4 | 2d: `in_app_chats` + `in_app_messages` + `chat_attachments` + `chart_promotions` | 4d: `ChatService` (attachments, voice, chart promotion to encounter; encryption-at-rest; rate-limit 30 msg/min) | 3d: patient + doctor chat UI; thread view; attachment preview; voice record | 2d: encryption test; rate-limit test; chart promotion race test | Realtime guild | 11d |
| 83 | `plugins-v1` | 3 | 2d: `plugins` + `plugin_versions` + `plugin_installs` + `plugin_licenses` + `plugin_hooks` + `plugin_audit` | 6d: `PluginManager` (in-process execution: 5s timeout, 32MB cap, try/catch boundary); manifest signature verify; capability model (`CapabilityGuard`); lifecycle FSM; kill switch | — (MML 3 is GA; admin UI; no public marketplace yet) | 3d: 8-point plugin security test (per `20-Security §23.2.3`); kill switch test; tenancy isolation test | Platform guild | 11d |
| 84 | `custom-domain` | 3 | 1d: `tenant_domains` + `domain_verifications` + `domain_certificates` | 4d: `CustomDomainService` (DNS-01 ACME with `_medicare-verify.` prefix; CAA-aware; HSTS opt-in; CloudFront distribution) | — (MML 3 is GA; admin UI; tenant-facing setup wizard) | 2d: ACM cert renewal test; CAA reject test; HSTS preload test | Platform guild | 7d |
| 85 | `multi-region-dr` | 3 | 1d: `dr_drill_log` + `region_status` (no new tables; uses existing infra) | 5d: Active-passive failover automation (RDS promote + Route 53 update + worker scale-up); RPO 5 min / RTO 1 h; quarterly drill | — (MML 3 is GA; ops-facing; no patient API) | 4d: full failover drill (1 per quarter); replication lag SLO; DNS health check | Platform guild | 10d |
| 86 | `drug-interaction-db` | 4 | 2d: `drug_interactions` + `drug_interaction_log` + `drug_sources` | 4d: `DrugInteractionService` (RxNav NIH free + DrugBank paid + WHO + tenant-curated; p99 < 200ms; `severity='contraindicated'` hard stop) | 3d: doctor "interaction warning" UI in Rx write; patient "interaction history" view | 2d: 4-source merge test; hard-stop override test; latency test | Clinical guild | 11d |
| 87 | `e-prescription` | 3 | 2d: `prescription_signatures` + `prescription_transmissions` (per-country) | 6d: `EPrescriptionService` (5 country formats: ABDM/SCRIPT/EPS/NHS/ADHA; per-country signing: DSC IN / smart card EU / EPCS token US; JWS ES256; transmission to partner pharmacy) | — (MML 3 is GA; admin UI; doctor signs in Rx write UI) | 3d: 5 country format tests; signing cert rotation; transmission retry | Clinical guild | 11d |
| 88 | `insurance-preauth` | 3 | 3d: `insurance_payers` + `insurance_eligibility_checks` + `insurance_preauth_requests` + `insurance_claims` + `insurance_eras` + `denials` | 6d: `InsuranceService` (270/271 eligibility; 278/NHCX preauth; 837 claim; 835 ERA auto-post; CARC/RARC denial auto-categorization) | 3d: patient "check coverage" UI; admin "submit preauth" UI; doctor "see denial reason" UI | 3d: 4 transaction type tests; CARC/RARC mapping test; NHCX gateway test | RCM guild | 15d |

**V2 lane total:** 11 modules × ~12d avg = **~130 engineer-days**. 15-person team: 5 BE / 2 FE / 2 Mobile / 1 AI-ML / 2 DevOps / 1 Security / 1 Designer / 1 PM / 1 QA = 7 calendar weeks (per `§19.3`).

### 20.4 V2.1 lane build plan (6 modules, target MML@GA per `§2.4`)

| # | Module | MML@GA | MML 1→2 (Schema) | MML 2→3 (Internal API) | MML 3→4 (Patient/Doctor API) | MML 4→5 (GA) | Owner | Total ETA |
|---|---|---|---|---|---|---|---|---|
| 89 | `revenue-cycle-mgmt` | 3 | 3d: `denial_appeals` + `dunning_policies` + `collections_agency_assignments` | 5d: `RcmService` (worklist by `due_date`: denial 15d, underpayment 30d, no_response 45d, request_info 10d, appeal 21d; dunning state machine; payment plans; underpayment detection) | 2d: admin "RCM worklist" UI; per-denial outcome tracking | 2d: worklist ordering test; dunning retry test; appeal SLA test | RCM guild | 12d |
| 90 | `saas-plan-marketplace` | 4 | 3d: `marketplace_developers` + `marketplace_plans` + `marketplace_subscriptions` + `marketplace_usage_records` + `marketplace_payouts` | 5d: `MarketplaceService` (70/30 default; dev portal at `developer.medicare.app`; KYC via Stripe Connect / Razorpay Route; proration; dunning) | 2d: developer "publish plan" UI; tenant "browse + install" UI | 2d: KYC fail test; dunning retry test; payout calc test | Platform guild | 12d |
| 91 | `on-prem-ai-gateway` | 3 | 1d: `on_prem_ai_configs` + `ai_token_usage` (per-tenant) | 5d: `OnPremAiService` (vLLM integration; air-gapped HSM mode; no prompt/response logged — only token counts + prompt SHA-256 hash) | — (MML 3 is GA; admin UI for config) | 3d: air-gapped license test; HSM unreachable test; audit log leak test | AI guild | 9d |
| 92 | `soc2-type-ii` | 3 | 3d: `controls` + `control_tests` + `control_exceptions` + `evidence_requests` | 4d: `Soc2Service` (75 controls: 33 CC + 7 A + 12 C + 5 PI + 18 P; quarterly evidence collection cron; control test scheduler; exception workflow) | — (MML 3 is GA; internal/audit-facing; no patient API) | 4d: Big-4 firm onboarding; observation period V2.0 → V2.1 start; control test coverage | Security guild | 11d |
| 93 | `byok` | 3 | 2d: `tenant_kms_configs` + `key_versions` + `dek_cache` | 5d: `ByokService` (5 providers: AWS KMS / GCP KMS / HashiCorp Vault / Azure Key Vault; DEK cache 5 min TTL; 30-day old key version grace; emergency revoke test) | — (MML 3 is GA; admin UI for KMS config) | 3d: 5 provider test; old key version grace test; emergency revoke test | Security guild | 10d |
| 94 | `field-level-encryption` | 3 | 2d: `data_classification` + `EncryptedFieldCast` migration for 5 fields (aadhaar / ssn / pan / medical_license / prescription_notes) + search-token sidecar columns | 4d: `FieldLevelEncryptionService` (`EncryptedFieldCast` per column; HMAC-SHA256 searchable-token sidecar; `data_classification` registry; daily drift sweep cron) | — (MML 3 is GA; transparent to users) | 2d: encryption test; search-token drift test; classification sweep test | Security guild | 8d |

**V2.1 lane total:** 6 modules × ~10d avg = **~60 engineer-days**. 8-person team: 4 BE / 1 FE / 1 DevOps / 1 Security / 1 PM / 1 QA = 5 calendar weeks (per `§19.4`).

### 20.5 V3 lane build plan (10 modules, target MML@GA per `§2.5`)

| # | Module | MML@GA | MML 1→2 (Schema) | MML 2→3 (Internal API) | MML 3→4 (Patient/Doctor API) | MML 4→5 (GA) | Owner | Total ETA |
|---|---|---|---|---|---|---|---|---|
| 95 | `plugin-marketplace` | 3 | 2d: `marketplace_listings` + `marketplace_ratings` + `marketplace_installs` (extends §90 catalog) | 5d: `MarketplaceCatalogService` (extends §90 with public storefront; rating + review; one-click install; revenue share) | 3d: developer "publish listing" UI; tenant "browse + install" storefront | 2d: catalog sync test; rating spam test; one-click race test | Platform guild | 12d |
| 96 | `hl7-v2-adt` | 3 | 2d: `hl7v2_messages` + `hl7v2_quarantine` (per `15-DB`); `hl7v2_audit` (IHE ATNA) | 6d: `Hl7v2Service` (Mirth sidecar; MLLP listener port 6661 mTLS / 6662 dev; ADT/ORU transforms → HAPI-FHIR; quarantine for malformed; ATNA audit) | — (MML 3 is GA; partner-facing MLLP; admin UI for routing) | 3d: Mirth transform test; quarantine test; ATNA audit log test | Integrations guild | 11d |
| 97 | `fhir-r4-write` | 3 | 2d: `smart_apps` + `fhir_write_log` + `fhir_smart_tokens` | 5d: `FhirR4WriteService` (SMART-on-FHIR launch; dual-write with conflict resolution; idempotency-key; resource validation) | 2d: SMART app launch UI; partner write API | 3d: dual-write race test; conflict resolution test; idempotency test | Integrations guild | 12d |
| 98 | `dicom-viewer` | 3 | 2d: `dicom_studies` + `dicom_reports` | 5d: `DicomService` (OHIF + Cornerstone.js at `/admin/dicom/{study_id}`; WADO-RS partner integration; 5-min short-lived JWT; 7-year HIPAA retention) | 2d: doctor "view DICOM" UI; patient "view my scan" UI | 3d: OHIF load test; JWT expiry test; retention purge test | Lab guild | 12d |
| 99 | `custom-report-builder` | 3 | 2d: `report_definitions` + `report_schedules` + `report_runs` | 5d: `ReportBuilderService` (visual builder; raw SQL via `pg_parse_tree` sandbox; read-only role; 30s `statement_timeout`; 1M row cap; CSV/Excel/PDF export) | 3d: admin "report composer" UI; doctor "run report" UI | 2d: SQL sandbox escape test; timeout test; row cap test | Reports guild | 12d |
| 100 | `white-label-mobile` | 3 | 2d: `tenant_mobile_config` (9 fields) + `tenant_mobile_builds` | 5d: `WhiteLabelMobileService` (EAS Build pipeline on hosted infra; asset override; per-tenant push cert; App Store / Play Submit automation) | 3d: tenant "upload logo" UI; "build my app" UI | 3d: EAS Build test; Submit rejection test; push cert mismatch test | Mobile guild | 13d |
| 101 | `sso-oidc-saml` | 3 | 1d: `identity_providers` + `sso_user_links` + `scim_endpoints` | 5d: `SsoService` (OIDC PKCE required for public clients; SAML signed Responses; SCIM v2 endpoint at `/api/v3/auth/scim/v2/{slug}/...`; JIT provisioning with `default_role`) | 2d: admin "configure IdP" UI; user "SSO login" UI | 2d: PKCE test; SAML signature test; SCIM v2 test; JIT race test | Auth guild | 10d |
| 102 | `bed-management` | 3 | 2d: `wards` + `beds` + `bed_assignments` (per `15-DB`) | 4d: `BedService` (assignment with `SELECT FOR UPDATE`; discharged bed → `cleaning`; transport orders; occupancy recalc) | 2d: nurse "assign bed" UI; admin "bed board" UI | 2d: row-lock deadlock test; cleaning state test; occupancy drift test | Hospital guild | 10d |
| 103 | `ot-room-scheduling` | 3 | 2d: `ot_rooms` + `ot_schedules` + `ot_bookings` (per `15-DB`) | 4d: `OtService` (booking with equipment check; surgical note; utilization recalc) | 2d: doctor "book OT" UI; admin "OT board" UI | 2d: equipment conflict test; surgical note test; utilization drift test | Hospital guild | 10d |
| 104 | `equipment-booking` | 3 | 2d: `equipment_slots` + `equipment_bookings` (per `15-DB`) | 4d: `EquipmentService` (UDI DI/PI from FDA; daily FDA recall API sync; FEFO enforcement) | 2d: doctor "book equipment" UI; admin "equipment board" UI | 2d: UDI lookup test; FDA recall test; FEFO test | Hospital guild | 10d |

**V3 lane total:** 10 modules × ~11d avg = **~110 engineer-days**. 17-person team: 6 BE / 2 FE / 2 Mobile / 2 DevOps / 1 Security / 1 PM / 2 QA / 1 GTM = 9 calendar weeks (per `§19.5`).

### 20.6 V4 lane build plan (6 modules, target MML@GA per `§2.6`)

| # | Module | MML@GA | MML 1→2 (Schema) | MML 2→3 (Internal API) | MML 3→4 (Patient/Doctor API) | MML 4→5 (GA) | Owner | Total ETA |
|---|---|---|---|---|---|---|---|---|
| 105 | `lis-astm` | 3 | 2d: `lis_messages` + `lis_results` + `lis_quarantine` (extends §96) | 6d: `LisService` (Mirth sidecar for ASTM E1394/E1394-97 + LIS-2A2; result import; partner-specific record mapping) | — (MML 3 is GA; partner-facing ASTM; admin UI for routing) | 3d: ASTM framing test; LIS-2A2 test; result import test; quarantine test | Lab guild | 11d |
| 106 | `his-bridge` | 3 | 2d: `his_partners` + `his_documents` + `his_identities` (PIX/PDQ) | 6d: `HisBridgeService` (`medicare-his-bridge` Java/Spring sidecar per tenant; HL7 v2 + FHIR R4 + IHE XDS.b + PIX/PDQ + C-CDA; bidirectional with Epic/Cerner/Meditech) | — (MML 3 is GA; partner-facing; admin UI for IHE Affinity Domain registration) | 3d: PIX/PDQ test; XDS.b registry test; C-CDA parse test | Integrations guild | 11d |
| 107 | `er-triage` | 3 | 2d: `er_bays` + `er_triage_assessments` (per `15-DB`) | 4d: `ErTriageService` (ESI 1-5 sorting; ESI 1 pinned to top of every lane; bay assignment; doctor pickup; board refresh) | 2d: nurse "triage" UI; doctor "pickup patient" UI; admin "ER board" UI | 2d: ESI sort test; bay assignment test; pickup test; board refresh test | Hospital guild | 10d |
| 108 | `nursing-station` | 3 | 2d: `vital_signs` + `mar_entries` + `handoff_sbars` (per `15-DB`) | 5d: `NursingService` (SBAR handoff template enforced; vitals with MEWS threshold page; MAR with Five Rights barcode; narcotic witness required) | 2d: nurse "vitals" UI; "MAR" UI; "handoff" UI; doctor "vitals review" UI | 2d: Five Rights test; narcotic witness test; MEWS test; handoff test | Hospital guild | 11d |
| 109 | `centralized-pharmacy` | 3 | 2d: `ward_stocks` + `schedule_vaults` + `witness_records` + `iv_admixtures` (per `15-DB`) | 5d: `PharmacyService` (ward stock reorder; Schedule II-V vault with witness; IV admixture calculation; FEFO) | 2d: pharmacist "vault access" UI; "IV admixture" UI; admin "ward stock" UI | 2d: vault access test; witness test; IV calc test; FEFO test | Pharmacy guild | 11d |
| 110 | `equipment-maintenance` | 3 | 2d: `biomedical_assets` + `pm_schedules` + `work_orders` + `fda_recalls` (per `15-DB`) | 4d: `EquipmentMaintenanceService` (biomedical asset tracking; PM scheduling; work orders; FDA recall API sync daily) | 2d: technician "work order" UI; admin "PM schedule" UI | 2d: PM overdue test; work order stuck test; FDA recall test | Hospital guild | 10d |

**V4 lane total:** 6 modules × ~11d avg = **~65 engineer-days**. 24-person team: 8 BE / 3 FE / 3 Mobile / 3 DevOps / 1 Security / 1 PM / 3 QA / 1 Clinical SME / 1 GTM = 9 calendar weeks (per `§19.6`).

### 20.7 V5+ lane build plan (1 module, target MML 1 per `§2.7`)

| # | Module | MML@GA | MML 1→2 (Schema) | MML 2→3 (Internal API) | MML 3→4 (Patient/Doctor API) | MML 4→5 (GA) | Owner | Total ETA |
|---|---|---|---|---|---|---|---|---|
| 111 | `population-health` | 1 (designed only) | TBD: de-identification pipeline (HIPAA Safe Harbor); cohort DSL syntax | TBD: cohort query engine with k-anonymity ≥ 5; HEDIS / MIPS measure calculator; opt-in research cohort tooling | TBD: clinician "cohort query" UI; public health "report" UI | TBD | Clinical guild | TBD (post-V4) |

**V5+ lane total:** 1 module at MML 1; full build plan pending V4 GA + funding.

### 20.8 Roll-up summary

| Lane | Modules | Total engineer-days | Team size | Calendar weeks | Per-module avg |
|---|---|---|---|---|---|
| V1.1 | 12 | ~120 | 6 | 12 | ~10d |
| V1.2 | 8 | ~80 | 6 | 8 | ~10d |
| V2 | 11 | ~130 | 15 | 7 | ~12d |
| V2.1 | 6 | ~60 | 8 | 5 | ~10d |
| V3 | 10 | ~110 | 17 | 9 | ~11d |
| V4 | 6 | ~65 | 24 | 9 | ~11d |
| V5+ | 1 | TBD | TBD | TBD | TBD |
| **Total** | **54** | **~565** | — | — | **~10.5d avg** |

(Plus 53 V1 active modules at MML 5 = 57 total V1 modules.)

### 20.9 MML 5 checklist (every module must hit all 11 gates)

For every module to reach **MML 5 (GA)**, the following 11 gates must be checked off in the PR description:

- [ ] **1. Schema** — migrations landed behind `feature_*` flag, no breaking changes, `down()` works, tested on MySQL + PostgreSQL.
- [ ] **2. Internal API** — endpoints documented in OpenAPI; RBAC enforced; tenant isolation verified; `tenants.feature_flags.${slug} = true` enables.
- [ ] **3. Patient/Doctor API** — endpoints documented; rate-limited; idempotency keys enforced where applicable; PII masked in responses.
- [ ] **4. Flutter / Next.js clients** — admin UI in `medicare-web/apps/admin`; patient UI in `medicare-web/apps/patient-web`; mobile UI in `medicare-user-app` and/or `medicare-doctor-app`.
- [ ] **5. Realtime** — events broadcast per `17-REALTIME.md` channel naming; private channels authenticated; no PHI on public channels.
- [ ] **6. Security review** — threat model signed off by Security guild; no OWASP Top 10; no secrets in logs; PHI encrypted at rest and in transit.
- [ ] **7. Tests** — unit + integration + E2E + performance per `23-DEVELOPMENT-SETUP §16`; coverage ≥ 80% on new code; CI green.
- [ ] **8. Observability** — Datadog dashboard live; PagerDuty service created; Sentry project created; status page component created; SLO defined and alertable.
- [ ] **9. Runbook** — `runbooks/{slug}.md` exists per `23-DEVELOPMENT-SETUP §17.1` template; quarterly drill scheduled; on-call rotation set.
- [ ] **10. Documentation** — deep-dive in module doc; `01-MODULES-INDEX §17` row updated; `14-MODULE-V2-STUBS §3.x` row updated; `17-REALTIME` channel entries; `15-DB-SCHEMA` tables; `16-API-REFERENCE` endpoints.
- [ ] **11. Feature flag removal** — `feature_${slug}` flag removed from `tenants.feature_flags` default; module is unconditionally available; the migration is committed; the changelog is updated; the customer-facing release notes are published.

---

## 21. Release Schedule & Risk Register

### 21.1 Per-quarter release calendar (next 24 months)

> Forward-looking schedule for the 54 V1.1-V4 modules. Per-quarter themes + the modules expected to reach MML 5 (GA) in that quarter. **MML 3 / MML 4 milestones** are tracked in the per-module build plans at §20; only MML 5 GA dates are listed here.

| Quarter | Lane | Modules reaching MML 5 (GA) | Headline |
|---|---|---|---|
| **Q1 2026 (M0)** | V1 GA | (V1 53 active modules ship at MML 5 as the platform baseline) | "Ship the Foundation" — 10 paying customers, $5K MRR |
| **Q2 2026 (M+2)** | V1.1 GA | 12 modules: `whatsapp-business`, `recurring-appointments`, `group-appointments`, `family-booking`, `multi-currency-payout`, `geo-search`, `marketing-vouchers`, `dynamic-refund-window`, `smart-waitlist`, `i18n-rtl`, `doctor-rotating-schedule`, `pwa-offline-mode` | "Polish & Performance" — 50 → 200 customers, NPS > 50 |
| **Q3 2026 (M+4)** | V1.2 GA | 8 modules: `abdm-bridge` (M1 cert), `fhir-r4-read`, `razorpay-subscriptions`, `paystack-subscriptions`, `stripe-terminal`, `twilio-sms`, `local-payment-methods`, `multi-clinic-dashboard` | "Native Integrations" — 200 → 500 customers, first enterprise hospital |
| **Q4 2026 (M+7)** | V2 GA | 11 modules: `pathology-lab` (marquee), `telemedicine-video`, `ai-symptom-checker`, `ai-clinical-scribe`, `in-app-chat`, `plugins-v1`, `custom-domain`, `multi-region-dr`, `drug-interaction-db`, `e-prescription`, `insurance-preauth` | "Platform Layer" — 500 → 1,000 customers, $100K MRR, first hospital chain |
| **Q1 2027 (M+10)** | V2.1 GA | 6 modules: `revenue-cycle-mgmt`, `saas-plan-marketplace`, `on-prem-ai-gateway`, `soc2-type-ii`, `byok`, `field-level-encryption` | "Insurance & Revenue Cycle" — 1,000 → 1,500 customers, $200K MRR |
| **Q2 2027 (M+13)** | V2.1 hardening | (no new GA; SOC 2 observation period mid-point; first external payer integration) | "Compliance & Hardening" — SOC 2 Type II mid-audit, first ISO 27001 stage-1 |
| **Q3 2027 (M+15)** | V3 GA | 10 modules: `plugin-marketplace`, `hl7-v2-adt`, `fhir-r4-write`, `dicom-viewer`, `custom-report-builder`, `white-label-mobile`, `sso-oidc-saml`, `bed-management`, `ot-room-scheduling`, `equipment-booking` | "Marketplace & Ecosystem" — 1,500 → 3,000 customers, $500K MRR, first plugin partners |
| **Q4 2027 (M+18)** | V3 hardening | (no new GA; SSO + HIS integration hardening; first EHR partner) | "Hardening & First EHR Partner" — SOC 2 Type II final audit |
| **Q1 2028 (M+22)** | V4 GA | 6 modules: `lis-astm`, `his-bridge`, `er-triage`, `nursing-station`, `centralized-pharmacy`, `equipment-maintenance` | "Hospital ITOps" — 3,000 → 5,000 customers, $1M MRR |
| **Q2 2028 (M+25)** | V4 hardening | (no new GA; HIS bridge hardening; first hospital chain reference) | "Hospital Reference" — first chain of 5+ hospitals live |
| **Q3 2028+ (M+28+)** | V5+ | `population-health` reaches MML 2 (schema) | "Population Health Design" — research cohort tooling under design |

### 21.2 Cumulative module count per quarter

| Quarter | Cumulative modules at MML 5 | New MML 5 modules in quarter | Cumulative % of 111 |
|---|---|---|---|
| Q1 2026 (V1 GA) | 57 (V1 53 active + 4 stubs still at MML 2) | 53 | 51% |
| Q2 2026 (V1.1) | 69 (53 V1 + 12 V1.1 + 4 V1 stubs) | 12 | 62% |
| Q3 2026 (V1.2) | 77 (53 V1 + 12 V1.1 + 8 V1.2 + 4 V1 stubs) | 8 | 69% |
| Q4 2026 (V2) | 88 (53 V1 + 12 V1.1 + 8 V1.2 + 11 V2 + 4 V1 stubs) | 11 | 79% |
| Q1 2027 (V2.1) | 94 (53 V1 + 12 V1.1 + 8 V1.2 + 11 V2 + 6 V2.1 + 4 V1 stubs) | 6 | 85% |
| Q3 2027 (V3) | 104 (+ 10 V3) | 10 | 94% |
| Q1 2028 (V4) | 110 (+ 6 V4) | 6 | 99% |
| Q3 2028+ (V5+) | 111 (+ 1 V5+) | 1 | 100% |

**Note:** the 4 V1 stubs (`pathology`, `ai-recommendation`, `blogs`, `subscription-plans-stub`) are deprecated in V2-V3 as the V2 replacements reach MML 5. They never reach MML 5 themselves; they are removed at V3 GA.

### 21.3 Top 10 risks (for the 24-month plan)

| # | Risk | Likelihood | Impact | Owner | Mitigation | Status |
|---|---|---|---|---|---|---|
| 1 | **ABDM M1 certification delayed** (the ABDM regulator is slow) | High | High (blocks V1.2) | Integrations guild | Apply for M1 cert at V1 GA (Q1 2026); maintain sandbox integration; have a fallback (manual ABHA entry) ready | Open |
| 2 | **SOC 2 Type II observation period finds an exception** | Medium | Critical (blocks V2.1) | Security guild | Run continuous controls monitoring; quarterly internal audits; engage Big-4 firm Q1 2026 for pre-audit readiness | Open |
| 3 | **LiveKit / Daily.co pricing changes** | Medium | Medium (impacts V2) | Telemedicine guild | Lock in 12-month contracts at V2 GA; have Jitsi as the open-source fallback | Open |
| 4 | **OpenAI / Anthropic rate limit at scale** | High | High (impacts V2 AI features) | AI guild | Multi-provider routing; per-tenant rate limits; pre-purchase enterprise capacity; on-prem LLM as V2.1 fallback | Open |
| 5 | **Razorpay / Stripe / Paystack policy change in target markets** | Medium | Medium | Payments guild | Multi-gateway abstraction; per-country fallback; legal review at Q1 2026 | Open |
| 6 | **Pathology lab partner integration drifts** (partner API change) | Medium | High (marquee feature) | Lab guild | Lock in 12-month API contracts with 3 partners at V2 GA; have a "manual lab result upload" fallback | Open |
| 7 | **Multi-region DR failover fails in real test** | Low | Critical (blocks V2 MML 5) | Platform guild | Quarterly drills starting Q2 2026; runbook + on-call rotation; AWS Route 53 health check from 3+ regions | Open |
| 8 | **Plugin Core security incident** (a plugin bypasses the sandbox) | Low | Critical (kills the marketplace) | Platform guild | Defense in depth: capability model + sandbox + signature verify + kill switch + audit; quarterly pen-test | Open |
| 9 | **Hiring lag** (cannot find BE / FE / Mobile / DevOps in target markets) | Medium | High (delays all lanes) | CTO | Remote-first hiring; competitive comp; equity; quarterly hiring plan; backup plan: outsource non-core modules | Open |
| 10 | **Customer concentration risk** (top 5 customers = 50% of MRR) | Medium | High (single churn kills quarter) | GTM | Diversify to 20+ customers by V1.1 GA; track concentration in monthly board report | Open |

### 21.4 Gating items (per lane)

> These items must be **checked off** before the lane can ship. Failure to clear a gating item defers the lane by 1 quarter (or until the item is resolved).

**V1.1 gating items (must clear by Q2 2026 GA):**
- [ ] All 12 modules reach MML 4 in CI (PR-open on `main`).
- [ ] 3 production tenants beta-testing for 4+ weeks with no SEV-1.
- [ ] 5 runbooks written and tested (`pwa-offline`, `smart-waitlist`, `i18n-rtl`, `doctor-rotating-schedule`, `recurring-appointments`).
- [ ] Datadog dashboards live for all 12 modules; PagerDuty services created.
- [ ] NPS ≥ 50 from 3 beta tenants.

**V1.2 gating items (must clear by Q3 2026 GA):**
- [ ] All 8 modules reach MML 4 in CI.
- [ ] ABDM M1 certification received.
- [ ] FHIR R4 Read: 1 US hospital partner validates the integration.
- [ ] Razorpay / Paystack / Stripe: 10 paying tenants in production.
- [ ] 3 runbooks written and tested (`abdm-bridge`, `fhir-r4-read`, `razorpay-subscriptions`).
- [ ] NPS ≥ 55 from 5 beta tenants.

**V2 gating items (must clear by Q4 2026 GA):**
- [ ] All 11 modules reach MML 4 in CI.
- [ ] 1 pathology lab partner in production with 100+ orders.
- [ ] 1 telemedicine video call completes end-to-end (LiveKit + consent + recording) in production.
- [ ] AI Scribe: 10 doctors using it for 4+ weeks with 0 PHI leak.
- [ ] Plugin Core: 3 third-party plugins built, installed, and audited.
- [ ] DR drill: 1 successful failover in production (with RTO < 1 h).
- [ ] 5 runbooks written and tested.
- [ ] NPS ≥ 60 from 10 beta tenants.

**V2.1 gating items (must clear by Q1 2027 GA):**
- [ ] All 6 modules reach MML 4 in CI.
- [ ] 1 enterprise customer using BYOK in production.
- [ ] SOC 2 Type II observation period: 6 months complete with no critical exceptions.
- [ ] Field-level encryption: 5 fields encrypted with no application-level regression.
- [ ] 3 runbooks written and tested.
- [ ] NPS ≥ 60 from 15 beta tenants.

**V3 gating items (must clear by Q3 2027 GA):**
- [ ] All 10 modules reach MML 4 in CI.
- [ ] 1 public plugin installed by 10+ tenants.
- [ ] 1 hospital using HL7 v2 ADT in production.
- [ ] SSO: 1 enterprise customer using OIDC + SCIM in production.
- [ ] 5 runbooks written and tested.
- [ ] NPS ≥ 65 from 20 beta tenants.

**V4 gating items (must clear by Q1 2028 GA):**
- [ ] All 6 modules reach MML 4 in CI.
- [ ] 1 hospital using ER Triage + Nursing Station in production.
- [ ] 1 hospital using HIS Bridge (Epic or Cerner) in production.
- [ ] 5 runbooks written and tested.
- [ ] SOC 2 Type II final audit passed (no exceptions).
- [ ] NPS ≥ 65 from 25 beta tenants.

### 21.5 Capacity plan (per lane)

| Lane | Engineers needed | Source | Hire plan |
|---|---|---|---|
| V1.1 | 6 (2 BE / 1 FE / 1 Mobile / 1 Designer / 1 PM) | Existing V1 team | No hires needed |
| V1.2 | 6 (+1 DevOps) | Existing + 1 hire | Hire DevOps Q1 2026 |
| V2 | 15 (+9: 3 BE / 1 FE / 1 Mobile / 1 AI-ML / 1 DevOps / 1 Security / 1 Designer / 1 QA) | Existing + 9 hires | Hires spread Q1-Q2 2026 |
| V2.1 | 8 (+2: 1 BE / 1 Security) | Existing + 2 hires | Hire Q4 2026 |
| V3 | 17 (+9: 2 BE / 1 FE / 1 Mobile / 1 DevOps / 1 Security / 1 PM / 1 QA / 1 GTM) | Existing + 9 hires | Hires spread Q2-Q3 2026 |
| V4 | 24 (+17: 2 BE / 2 FE / 1 Mobile / 2 DevOps / 1 Security / 1 PM / 1 QA / 1 Clinical SME / 1 GTM, with overlap from V3) | Existing + 17 hires | Hires spread Q1-Q3 2027 |

**Total headcount growth:** 53 (V1) → 24 (V4) = 77 people by Q1 2028.

### 21.6 Per-quarter OKR sketch

| Quarter | Objective | Key Results |
|---|---|---|
| Q1 2026 (V1) | Ship V1 GA | KR1: All 53 V1 modules at MML 5; KR2: 10 paying customers onboarded; KR3: $5K MRR; KR4: 0 SEV-1 incidents in production |
| Q2 2026 (V1.1) | Ship V1.1 | KR1: All 12 V1.1 modules at MML 5; KR2: 200 customers; KR3: NPS ≥ 50; KR4: 5 runbooks published |
| Q3 2026 (V1.2) | Ship V1.2 | KR1: All 8 V1.2 modules at MML 5; KR2: 500 customers; KR3: 1 enterprise hospital signed; KR4: ABDM M1 cert received |
| Q4 2026 (V2) | Ship V2 | KR1: All 11 V2 modules at MML 5; KR2: $100K MRR; KR3: 1,000 customers; KR4: 1 hospital chain signed |
| Q1 2027 (V2.1) | Ship V2.1 | KR1: All 6 V2.1 modules at MML 5; KR2: $200K MRR; KR3: SOC 2 Type II observation period 6 months complete; KR4: 1 enterprise using BYOK |
| Q3 2027 (V3) | Ship V3 | KR1: All 10 V3 modules at MML 5; KR2: $500K MRR; KR3: 1 public plugin in production; KR4: 1 hospital using SSO |
| Q1 2028 (V4) | Ship V4 | KR1: All 6 V4 modules at MML 5; KR2: $1M MRR; KR3: SOC 2 Type II final audit passed; KR4: 1 hospital using HIS Bridge (Epic or Cerner) |

### 21.7 Plan re-baseline cadence

This plan is **re-baselined quarterly** at the start of each new quarter:
- The previous quarter's KRs are graded (Green / Yellow / Red).
- The next quarter's KRs are set based on the actuals.
- The risk register is re-prioritized.
- The per-module MML progression in §20 is updated to reflect actual ETAs.
- The capacity plan is adjusted for hiring lag.

The re-baseline is owned by the CTO + the PM guild; it is published in `#eng-leadership` and added to the board deck.
