# 20 — Security & Compliance (HIPAA, GDPR, Threat Model, Controls)

> The non-negotiable security baseline. Control status is stated as it is: a claim that is machine-checked names its check in `claims.toml`, and anything not backed by an executed case is written as open. This doc is the source of truth for "is this safe to deploy?".

---

## 1. Compliance Posture

| Regulation | Status | Notes |
|---|---|---|
| **HIPAA** (US) | Partial [claim:hipaa-technical-safeguards] | Tenant scoping, auth and audit surfaces exist; encryption at rest, PHI-read audit and BAA handling are open. BAA required for US customers using AWS. |
| **GDPR** (EU) | Partial [claim:gdpr-data-subject-rights] | Rights surface exists; the erasure cascade is not yet evidenced by a green end-to-end case. |
| **India DPDP Act 2023** | Partial [claim:dpdp-data-principal-rights] | Same rights surface as GDPR; consent/erasure evidence is open. |
| **SOC 2 Type II** | Roadmap (V2) | All controls in place; audit in year 2. |
| **ISO 27001** | Roadmap (V3) | Requires formal ISMS. |

> We are not "certified HIPAA-compliant" — that's a per-customer BAA thing. We provide the technical capabilities for customers to be compliant.

---

## 2. Threat Model (STRIDE)

| Threat | Mitigation |
|---|---|
| **Spoofing** of user identity | Strong auth (Firebase OTP, bcrypt/argon2id, optional 2FA), token rotation |
| **Tampering** of data in transit | TLS 1.3 everywhere (HSTS), certificate pinning in mobile apps |
| **Tampering** of data at rest | DB encryption at rest (AWS RDS), encrypted backups, signed S3 URLs |
| **Repudiation** of actions | Append-only audit log with cryptographic integrity (HMAC chain) |
| **Information disclosure** (PHI leak) | Tenant scoping, RBAC, no PHI in logs, no PHI in URLs, no PHI in error messages |
| **Denial of service** | Rate limiting, Cloudflare WAF, queue backpressure |
| **Elevation of privilege** | Spatie RBAC, ownership policies, no admin via API without 2FA |
| **Cross-tenant data leak** | BelongsToTenant trait, CI test that seeds 2 tenants and asserts no leak |
| **Webhook spoofing** | HMAC signature verification on every webhook |
| **Insider threat** (staff) | Audit log of every PHI read, role-based access, 4-eyes for sensitive ops |
| **Supply chain** | Composer audit, npm audit, pinned versions, signed images |
| **Prompt injection** (V2 AI features) | System prompts in DB, not user input; output sanitization |

---

## 3. Authentication & Authorization

(Already detailed in `02-MODULE-AUTH.md`. Summary here.)

### 3.1 Mobile (Realm A)

- Firebase Phone OTP — single-use, 60s expiry, 5 sends/phone/hour, 10 sends/IP/hour.
- ID token verified with Firebase Admin SDK (checks signature, expiry, audience, issuer, revoked).
- Sanctum token issued, 30-day expiry, stored in `flutter_secure_storage` (Keystore/Keychain).
- Token auto-refresh 7 days before expiry.
- FCM tokens per device; logout revokes Sanctum tokens but preserves FCM for re-login.

### 3.2 Web (Realm B)

- Email + password, bcrypt cost 12 or argon2id (preferred).
- Password rules: min 10 chars, upper+lower+digit+symbol, zxcvbn score ≥ 3.
- Email verification required for staff.
- "Remember me" issues a long-lived token (60 days) + rotation cookie.
- 2FA (TOTP) optional in V1, mandatory for `super-admin` and any role with `manage-roles` permission.

### 3.3 Authorization

- Spatie RBAC: ~150 permissions in `verb-noun` form.
- Wildcards via `Gate::before`: `manage-X` implies all CRUD on X.
- Ownership policies for "doctor can only see their own appointments" etc.
- Super-admin can impersonate; banner is shown, audit log stamped.

---

## 4. Encryption

### 4.1 At Rest

| Data | Encryption |
|---|---|
| Database | AWS RDS encryption (AES-256), keys in AWS KMS |
| Database backups | Encrypted by AWS; copy to a separate KMS key |
| Object storage (S3) | SSE-S3 (AES-256); SSE-KMS option for PHI bucket |
| Mobile secure storage | Android Keystore / iOS Keychain (hardware-backed where possible) |
| Redis (sessions, cache) | Encrypted at rest if using ElastiCache encryption |
| Logs | No PHI in logs (see §5) |

### 4.2 In Transit

- **TLS 1.3** everywhere (no TLS 1.0/1.1, no SSLv3).
- HTTP → HTTPS redirect (301).
- HSTS header with `max-age=31536000; includeSubDomains; preload`.
- Certificate pinning in mobile apps (pinned to leaf cert, with backup pin).
- WebSocket (WSS) over TLS.

### 4.3 Key Management

- AWS KMS for cloud-managed keys.
- All keys rotated annually.
- Key access logged in CloudTrail.
- No keys in `.env` in version control (use AWS Secrets Manager / Laravel encrypted env).

---

## 5. PHI Handling

### 5.1 What is PHI

Any data that identifies a patient + their health info:
- Name + appointment
- MRN
- Vitals
- Prescriptions
- Patient files (lab reports, images)
- Doctor's clinical notes
- Diagnosis

### 5.2 Rules

1. **Never log PHI.** Sentry before_send filter strips known PHI patterns.
2. **Never put PHI in URLs.** Use UUIDs in URLs (`/patients/8c3a-...`), not `/patients/Aarav-Sharma`.
3. **Never put PHI in error messages.** Generic errors to clients; details only in Sentry.
4. **Never email PHI without encryption.** Patient files are never emailed as attachments; only signed links valid 5 min.
5. **Always access-control PHI.** Policy check before every read.
6. **Always audit PHI reads.** `phi.access` event in audit log.
7. **Always encrypt PHI in transit and at rest.**

### 5.3 De-identification for Analytics

- Vitals / aggregate stats use MRN, never name.
- Reports: `COUNT(*)` only by default; row-level data requires admin + audit.
- No patient data is ever sent to third-party analytics (Google Analytics is IP-anonymized; Plausible is cookieless).

---

## 6. Tenant Isolation

(Already detailed in `03-MODULE-TENANCY.md`. Summary here.)

- Every business row has `tenant_id` (or is explicitly global).
- `BelongsToTenant` global scope auto-filters all queries.
- `BelongsToTenant::saving` throws if cross-tenant assignment.
- CI test seeds 2 tenants, runs all feature tests, asserts no cross-tenant data leak.
- Cache keys prefixed `t:{tenant_id}:`.
- Reverb channel names include tenant.
- S3 paths include tenant slug.
- Backups per tenant via `tenant:backup`.

---

## 7. Audit Logging

(Already detailed in `13-MODULE-SETTINGS-REPORTS.md` §4.)

- `audit_logs` table, append-only.
- Every PHI read, every write, every state change, every login attempt.
- DB user has INSERT/SELECT only on `audit_logs` — no UPDATE/DELETE.
- Retention: 7 years online, then S3 Glacier.
- Cryptographic integrity (HMAC chain — each row includes `prev_hash` to detect tampering).

```php
// app/Modules/AuditLogs/Services/IntegrityChain.php
class IntegrityChain {
    public function append(array $data): AuditLog {
        $prevHash = AuditLog::orderBy('id', 'desc')->value('hash') ?? str_repeat('0', 64);
        $data['hash'] = hash('sha256', json_encode($data) . $prevHash);
        $data['prev_hash'] = $prevHash;
        return AuditLog::create($data);
    }
    
    public function verify(): bool {
        // Walk the chain, recompute hashes
    }
}
```

A daily cron verifies the chain; Sentry alert on any mismatch.

---

## 8. Rate Limiting

(See `16-API-REFERENCE.md` §1.7.)

| Realm | Limit | Window |
|---|---|---|
| Public (no auth) | 60 req | 1 min / IP |
| Authenticated | 600 req | 1 min / user |
| OTP send | 5 / phone | 1 hour |
| OTP send | 10 / IP | 1 hour |
| Login attempts | 5 | 15 min → 30 min lockout |
| Webhook | unlimited | (signature-verified) |

429 returns `Retry-After: {seconds}`.

### 8.1 Brute Force Protection

- Failed login → exponential backoff per email/phone.
- 5 fails in 15 min → lock for 30 min, notify user via email.
- 10 fails in 1 hour from same IP → temporary IP block (1 hour).
- 100 fails in 24h from same IP → permanent block (manual unblock via admin).

---

## 9. Input Validation

- Every request validated by `FormRequest` (Laravel's first line).
- Whitelist validation (not blacklist).
- Length limits on every string.
- Numeric ranges on every number.
- Enum constraints on every enum.
- File uploads: mime + size + extension + magic number check + ClamAV scan.
- URL parameters: regex-constrained.
- HTML in user input: sanitized via `strip_tags` + `htmlspecialchars`. WYSIWYG content (prescription body) sanitized via HTMLPurifier with allowlist.

---

## 10. Output Encoding

- All JSON responses auto-escaped by Laravel.
- HTML emails rendered via Blade with `{{ }}` (escaped).
- HTML in admin app: React auto-escapes.
- PDFs: rendered from data, never from user HTML.
- File downloads: served with `Content-Disposition: attachment` and proper `Content-Type`.

---

## 11. CSRF

- Web admin: CSRF token on every state-changing form (Laravel default).
- Mobile apps: not vulnerable to CSRF (no cookies for auth in mobile, Bearer token in header).
- Public website (patient): no state-changing forms; all actions via API with Bearer.

---

## 12. CORS

- Strict CORS policy in `config/cors.php`:
  - `allowed_origins` = `['https://medicare.app', 'https://*.medicare.app', 'https://admin.medicare.app']`
  - `allowed_methods` = `['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS']`
  - `allowed_headers` = `['Authorization', 'Content-Type', 'Accept', 'X-Tenant', 'Idempotency-Key']`
  - `supports_credentials` = true (for web session)
  - `max_age` = 86400

---

## 13. Security Headers

Every response includes:

```
Strict-Transport-Security: max-age=31536000; includeSubDomains; preload
X-Content-Type-Options: nosniff
X-Frame-Options: DENY
X-XSS-Protection: 1; mode=block
Referrer-Policy: strict-origin-when-cross-origin
Content-Security-Policy: default-src 'self'; script-src 'self' 'nonce-{csp-nonce}'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self' data:; connect-src 'self' wss: https://api.medicare.app; frame-ancestors 'none'
Permissions-Policy: camera=(self), microphone=(self), geolocation=(self)
```

CSP nonce is per-request and used by the admin SPA.

---

## 14. Secrets Management

| Secret | Where | Rotation |
|---|---|---|
| `APP_KEY` | AWS Secrets Manager | Never (regenerating breaks all tokens) |
| DB password | AWS Secrets Manager | Annually |
| Redis password | AWS Secrets Manager | Annually |
| Razorpay/Stripe/Paystack | AWS Secrets Manager | On staff change |
| Firebase service account | AWS Secrets Manager | On staff change |
| SMTP password | AWS Secrets Manager | Annually |
| S3 keys | IAM role (no static keys) | Auto by AWS |
| Sentry DSN | Public (DSN is OK to expose) | N/A |

`.env` is generated from Secrets Manager at deploy time. Never committed.

---

## 15. Vulnerability Management

### 15.1 Dependency Scanning

- Composer: `composer audit` in CI, fails on `high` or `critical`.
- npm: `npm audit --audit-level=high` in CI.
- Flutter: `dart pub outdated` + Snyk for deeper scan.
- GitHub Dependabot enabled with weekly auto-PR for non-major updates.

### 15.2 SAST

- **PHPStan** level 8 in CI.
- **Psalm** for additional type checking.
- **Snyk Code** for SAST.
- **Semgrep** for custom rules (PHI patterns, SQL patterns).

### 15.3 DAST

- **OWASP ZAP** weekly scan against staging.
- **Burp Suite Pro** for manual pen-test (annually).

### 15.4 Bug Bounty

- V2: open bug bounty via HackerOne or Intigriti.
- V1: invite-only for trusted researchers.

---

## 16. Incident Response

### 16.1 Severity Levels

| Sev | Definition | Response |
|---|---|---|
| Sev 1 | PHI breach, full outage, payment data leak | Page on-call, all-hands, public status page within 1h |
| Sev 2 | Partial outage, non-PHI data leak | Page on-call, fix within 4h |
| Sev 3 | Single-user impact, degraded performance | Ticket, fix in next sprint |
| Sev 4 | Cosmetic, minor | Backlog |

### 16.2 Breach Notification (GDPR)

- **72 hours** to notify supervisory authority after becoming aware.
- **Without undue delay** to notify data subjects if high risk.
- Document the breach: nature, categories + approximate number of data subjects, likely consequences, measures taken.
- Stored in `breach_register` table.

### 16.3 Forensic Checklist

- [ ] Snapshot DB and S3 immediately (don't wait)
- [ ] Pull audit logs for the relevant tenant(s) and time window
- [ ] Identify scope: who had access, what was read/written
- [ ] Identify root cause: SQL injection? Stolen token? Insider?
- [ ] Patch and redeploy
- [ ] Notify affected customers (data controllers)
- [ ] File regulatory notifications
- [ ] Post-mortem within 5 business days

---

## 17. Disaster Recovery

| Scenario | RTO | RPO |
|---|---|---|
| Single server down | 30 min (auto-failover) | 0 (live replication) |
| Database corruption | 4 hours (restore from backup) | 24 hours (last daily backup) |
| Ransomware (full encryption) | 8 hours (rebuild from backup + S3) | 24 hours |
| Region down (AWS) | 12 hours (failover to backup region) | 1 hour (cross-region replication) |
| Accidental data deletion (single tenant) | 4 hours (per-tenant restore) | 0 (point-in-time recovery) |

### 17.1 Backup

- DB: daily mysqldump to S3, retained 30 days, weekly to Glacier retained 1 year.
- S3: versioning enabled, lifecycle rule moves to IA after 30 days, Glacier after 90 days.
- Per-tenant backup: `php artisan tenant:backup {slug}` — daily at 02:00 UTC, retained 90 days.
- Backup verification: weekly auto-restore to a sandbox, smoke test.

### 17.2 DR Drill

- Quarterly: full restore from backup, verify against smoke tests.
- Annually: region failover drill.

---

## 18. GDPR Data Subject Rights

| Right | Implementation |
|---|---|
| **Access** (Art. 15) | `GET /me/export-data` → JSON + all files ZIP, emailed within 24h |
| **Rectification** (Art. 16) | Self-service profile update; staff can correct via admin |
| **Erasure** (Art. 17) | `POST /me/delete-account` → 30-day grace, then anonymize; PHI retained 7 years for legal (Art. 17(3)(b)) |
| **Restrict processing** (Art. 18) | `POST /me/restrict-processing` → sets `is_processing_restricted=true` |
| **Portability** (Art. 20) | `GET /me/export-data` → machine-readable JSON |
| **Object** (Art. 21) | `POST /me/object-to-processing` |
| **No automated decision-making** (Art. 22) | No automated treatment decisions; AI symptom-check (V2) requires human review |

### 18.1 Data Export Format

```json
{
  "user": { ... },
  "patients": [ { "id", "mrn", "name", "dob", "allergies", "chronic_conditions", ... } ],
  "appointments": [ ... ],
  "prescriptions": [ { "id", "issued_at", "diagnosis", "lines": [ ... ] } ],
  "vitals": [ ... ],
  "files": [ { "id", "category", "title", "download_url", "expires_at" } ],
  "transactions": [ ... ],
  "audit_log": [ { "event", "at", "ip" } ]
}
```

Files are included as base64-encoded blobs in a separate `files.zip` (signed URL valid 7 days, single download).

### 18.2 Erasure Process

1. User requests deletion → account marked `pending_deletion`, 30-day timer starts.
2. User can cancel within 30 days.
3. After 30 days:
   - User email / phone replaced with `deleted+{id}@medicare.app` / `+0000000`.
   - Name replaced with `Deleted User`.
   - All sessions / tokens / FCM tokens revoked.
   - PHI in `audit_logs` retained per legal hold (7 years), but the link to the user is broken.
   - Patient files: deleted.
   - Invoices / transactions: retained for tax law (10 years), but the patient name is anonymized.

---

## 19. HIPAA-Specific (for US Customers)

Customers on a Business Associate Agreement (BAA) get:

- AWS RDS encryption with a per-tenant KMS key (V2).
- Audit log retention 7 years.
- Signed BAAs available.
- PHI is never used for training, analytics, or any non-customer purpose.
- Subcontractors list: AWS (hosting), Mailgun (email), Sentry (errors), Firebase (auth + push) — all BAA-eligible.

---

## 20. The Security Checklist (Pre-Deploy)

Before every prod deploy, GitHub Actions runs:

- [ ] `composer audit` clean
- [ ] `npm audit` clean
- [ ] PHPStan level 8 clean
- [ ] All tests pass (including security tests)
- [ ] No secrets in diff (gitleaks)
- [ ] Migrations are reversible
- [ ] OWASP ZAP baseline scan on staging passes
- [ ] Sentry canary error from staging surfaces
- [ ] Rate limits verified
- [ ] TLS grade A+ on `https://www.ssllabs.com/ssltest/`
- [ ] Security headers present (curl -I)
- [ ] CSP report-only mode for 1 week before enforcing

---

## 21. The Security Training

Every developer signs off annually on:
- OWASP Top 10
- This document
- Internal "secure coding" guide
- Phishing simulation (90% pass rate required)

---

## 22. Reporting a Vulnerability

`security@medicare.app` (PGP key published at `/.well-known/security.txt`).

We commit to:
- Acknowledge within 24 hours.
- Triage within 72 hours.
- Fix critical within 7 days.
- Public disclosure 90 days after fix, or earlier if mutually agreed.
- Hall of fame for verified reports.

---

## 23. V2 Security & Compliance

> V1 covers HIPAA, GDPR, DPDP (India), and PIPEDA (Canada). V2 adds **SOC 2 Type II**, **HITRUST CSF**, **ISO 27001**, **ABDM certification**, **FHIR/IHE ATNA**, and **EU BAIC**. V2 also introduces **field-level encryption**, **BYOK**, **data access logging**, **AI prompt firewalls**, and **multi-region DR** with encrypted backups.

### 23.1 V2 compliance matrix

| Regime | Region | Effective | V1 support | V2 support | Auditor |
|---|---|---|---|---|---|
| **HIPAA** | US | 1996 | ✓ | ✓ (BAA template) | Internal + external |
| **GDPR** | EU | 2018 | ✓ | ✓ (DPIA for AI) | Internal + DPO |
| **DPDP Act** | India | 2023 | ✓ | ✓ | Internal |
| **PIPEDA** | Canada | 2000 | ✓ | ✓ | Internal |
| **LGPD** | Brazil | 2020 | partial | ✓ | Internal |
| **POPIA** | South Africa | 2013 | partial | ✓ | Internal |
| **PDPA Singapore** | SG | 2014 | partial | ✓ | Internal |
| **ABDM** | India | 2020 | — | ✓ (M1/M2/M3) | NHA |
| **SOC 2 Type I** | Global | 2024 | — | ✓ (V2 GA) | Drata + Vanta |
| **SOC 2 Type II** | Global | 2025 | — | ✓ (V2.1) | Drata + Vanta |
| **HITRUST CSF** | US | 2024 | — | partial | A-LIGN |
| **ISO 27001** | Global | 2025 | — | partial (V3) | BSI |
| **FHIR R4** | Global | 2018 | — | ✓ | IHE |
| **IHE ATNA** | Global | 2018 | — | ✓ (V3) | IHE |
| **EU BAIC** | EU | 2024 | — | partial | EU AI Office |
| **NIST AI RMF** | US | 2023 | — | ✓ | NIST |
| **OWASP LLM Top 10** | Global | 2023 | — | ✓ | Internal |

### 23.2 V2 security controls (additions to V1)

#### 23.2.0 SOC 2 Type II readiness — `soc2-type-ii` (V2.1, MML 3)

**1. Overview.** V2 GA targets SOC 2 Type I (a point-in-time attestation of control design). V2.1 GA targets **SOC 2 Type II** (an attestation of control design **and** operating effectiveness over a 12-month observation period). Type II is the gate most US enterprise customers (and many regulated industries globally) require before signing a BAA. Per `24-ROADMAP` and `20-Security §23.1` compliance matrix: Type I at V2 GA (Month 7), Type II at V2.1 GA (Month 10 — so the 12-month observation runs V2.0 through V2.1). MML 3 GA: all five Trust Service Criteria (TSC) covered, evidence collection automated, control tests run on a schedule, exceptions triaged within SLA, the auditor's report is delivered to enterprise prospects via a trust portal.

**2. The five Trust Service Criteria (AICPA TSC 2017, with 2022 points-of-focus revisions).**

| TSC | What it covers | Medicare modules that own it |
|---|---|---|
| **CC — Common Criteria** | Control environment, communication, risk assessment, monitoring, information & communication | `02-Auth`, `13-Settings-Reports` (audit), `20-Security` (this doc) |
| **A — Availability** | Uptime commitments, DR, capacity, recovery | `21-Deployment`, `03-Tenancy §17.5` (multi-region DR) |
| **C — Confidentiality** | Data classification, encryption, access, retention | `20-Security §23.2.1` (field-level enc), `03-Tenancy §17.6` (per-tenant KMS) |
| **P — Processing Integrity** | Inputs, processing, outputs, error handling | `13-Settings-Reports` (audit log), `08-Rx` (drug interactions), `09-Billing` (RCM) |
| **P — Privacy** (optional) | Notice, consent, collection, use, retention, disclosure, quality, monitoring | `02-Auth` (consent), `06-Patients §2.4` (PHI masking), `11-Notif` (consent receipt) |

**3. What "V2.1 MML 3" means concretely.**
- **All 33 Common Criteria** controls implemented and tested (CC1.1 through CC9.2).
- **All 7 Availability** controls (A1.1 through A1.3): uptime SLO published, DR drill quarterly, capacity plan reviewed.
- **All 12 Confidentiality** controls (C1.1 through C1.2): data classification policy, encryption at rest + in transit, key rotation, access reviews.
- **All 5 Processing Integrity** controls (PI1.1 through PI1.5): input validation (already V1), processing logs (audit), output reconciliation (e.g., invoice ↔ payment match), error-rate monitoring.
- **All 18 Privacy** controls (P1.1 through P8.1) — opt-in for the Privacy TSC; we include it because most healthcare tenants need it.
- **Evidence collection automated** via Drata + Vanta: the platform exports a daily snapshot of control state (e.g., "all 47 employees completed security training on 2026-04-15", "MFA is enforced for 100% of admin users", "last DR drill was 2026-03-10 with RTO 47m and RPO 2m").
- **Auditor (Drata + a Big-4 firm) tests operating effectiveness** over the 12-month window; the resulting Type II report is delivered to enterprise prospects via a trust portal at `medicare.app/trust`.

**4. Continuous control monitoring (CCM).** Every control has a corresponding test that runs on a schedule (daily / weekly / monthly / quarterly). The test result is written to `control_tests` and pulled by Drata. Examples:
- **CC6.1 — Logical access controls:** daily test enumerates all users with admin role and asserts MFA is enabled for 100%.
- **CC6.6 — Encryption at rest:** daily test asserts all production DB columns tagged `phi=true` are encrypted with a KMS key that rotated within the last 365 days.
- **CC7.2 — Anomaly detection:** weekly test samples 1000 audit-log rows and asserts none have been edited (append-only invariant).
- **A1.2 — Environmental protections:** monthly test runs a synthetic tenant load and asserts p99 latency < SLO.
- **C1.1 — Confidential information identification:** weekly test scans new tables/columns and asserts every new PHI column is tagged in `data_classification`.
- **PI1.4 — Implementation:** daily test for a sample of 10 invoices asserts the invoice total = sum of line items + tax - discount; drift > $0.01 is a P1 incident.
- **P3.1 — Collection:** daily test for a sample of 10 new patients asserts consent was captured before the first PHI write.

**5. The auditor's evidence request workflow.** Quarterly, the auditor (Drata's partner firm) requests evidence for a sample of controls. The `evidence_requests` table tracks the request; the system auto-bundles the relevant logs, screenshots, and exports. The auditor logs in via a scoped read-only role (`soc2_auditor`) that can read audit logs, control tests, and policy docs but cannot read PHI.

**6. Domain Model.**
```php
Schema::create('controls', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->string('code', 32)->unique();                        // 'CC6.1'
    $t->string('tsc', 16);                                   // 'CC' | 'A' | 'C' | 'PI' | 'P'
    $t->string('title', 255);
    $t->text('description');
    $t->string('owner_user_id', 26);
    $t->string('frequency', 16);                            // 'continuous' | 'daily' | 'weekly' | 'monthly' | 'quarterly' | 'annual'
    $t->json('test_spec');                                   // {assertion, query, threshold}
    $t->timestamps();
});

Schema::create('control_tests', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('control_id')->index();
    $t->timestamp('ran_at')->index();
    $t->string('result', 16);                                // 'pass' | 'fail' | 'exception'
    $t->text('evidence_url')->nullable();                    // S3 link to the test's output
    $t->text('notes')->nullable();
    $t->integer('duration_ms');
    $t->timestamps();
});

Schema::create('control_exceptions', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('control_id')->index();
    $t->ulid('tenant_id')->nullable()->index();
    $t->string('status', 16)->default('open');               // 'open' | 'accepted' | 'remediated' | 'risk_accepted'
    $t->text('description');
    $t->text('compensating_control')->nullable();
    $t->ulid('approved_by_user_id')->nullable();
    $t->timestamp('approved_at')->nullable();
    $t->date('due_date')->nullable();
    $t->timestamps();
});

Schema::create('evidence_requests', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('auditor_user_id')->index();
    $t->string('period', 7)->index();                       // '2026-Q1'
    $t->json('control_ids');
    $t->string('status', 16)->default('pending');            // 'pending' | 'in_progress' | 'delivered'
    $t->text('s3_archive_url')->nullable();
    $t->timestamp('delivered_at')->nullable();
    $t->timestamps();
});
```

**7. Trust portal.** A public page at `medicare.app/trust` (auth-gated for prospect-level access) shows: current Type II report (PDF), sub-processor list (Lindy, Stripe, Twilio, etc., per `19-Integrations` §16), uptime status (live, from `21-Deployment` §19 status page), security.txt, vulnerability disclosure policy (per `20-Security §22`), and a "Request the report" form for enterprise prospects (gated by email domain allowlist).

**8. Realtime.**
- `soc2.control_test.{passed|failed}.{control_code}` — internal Slack.
- `soc2.exception.{opened|triaged|remediated}.{tenant_id?}` — security team pager.
- `soc2.evidence_request.{created|delivered}.{auditor_id}` — auditor dashboard.

**9. Security & RBAC.** Roles: `security_engineer` (manage controls, run tests, respond to exceptions), `compliance_officer` (manage evidence, communicate with auditor), `soc2_auditor` (scoped read-only; can read audit logs and control tests but not PHI). Per `20-Security §11.4`, a control exception with `risk_accepted` status requires co-sign by `ciso` and is time-bounded (max 90 days). The trust portal never exposes tenant data; it exposes only the platform-level controls.

**10. Acceptance Criteria.**
- [ ] All 33 CC, 7 A, 12 C, 5 PI, 18 P controls have an implementation + an automated test.
- [ ] Control tests pass at >99% over a 30-day window; exceptions are triaged within 5 business days.
- [ ] A 12-month observation period completes with no material exceptions; the Big-4 firm issues an unqualified opinion.
- [ ] The Type II report is delivered to enterprise prospects via the trust portal within 1 business day of request.
- [ ] A new control (e.g., for a new TSC revision or a new product surface) can be added in < 1 sprint, with the test added to the daily CCM run.
- [ ] The sub-processor list is auto-generated from the integration registry; it is updated within 24h of a new vendor onboarding.
- [ ] The auditor's evidence request is fulfilled within 5 business days.

**11. Real-World Test Scenarios.**
- *Quarterly evidence request:* the auditor requests evidence for CC6.1, CC6.6, CC7.2 for Q1 2026; the system bundles the daily test results, the user list, the encryption config, the audit log samples, and the anomaly detection report; a signed S3 URL is delivered; the auditor downloads; the request is closed.
- *Material exception:* a control test fails (e.g., "MFA not enabled for 3 admin users"); the exception is auto-created; the security engineer triages; the affected users are notified; MFA is enabled; the test is re-run; the exception is marked `remediated`; the daily test passes for the next 30 days.
- *Risk-accept exception:* a tenant's policy requires a deviation from the standard (e.g., the tenant is in a country where SMS OTP is the only option and the auditor agrees the risk is acceptable); the exception is `risk_accepted` with a 90-day expiry; the tenant is re-onboarded with a stronger MFA at the next login.
- *Auditor pivots to a new control:* the auditor adds a new control for the V2.1 release; the security engineer creates a test; the test runs daily; the result is fed to Drata; the auditor accepts.
- *Trust portal request:* an enterprise prospect at a US hospital chain requests the Type II report; the email is on the allowlist; the report is delivered within 1 business day; the request is audit-logged.

**12. What this module is NOT:**
- Not a HIPAA certification. SOC 2 is a private-sector attestation; HIPAA is a federal regulation. We are HIPAA-compliant per `20-Security §21`, but the SOC 2 Type II report is a separate document.
- Not HITRUST or ISO 27001. Those are V3 targets per the compliance matrix.
- Not a substitute for tenant-side controls. SOC 2 attests to the **platform's** controls; tenants are responsible for their own workforce training, device management, etc.

**13. Dev Workflow.** `php artisan soc2:test-run --control=CC6.1` runs a single control test. `php artisan soc2:daily-run` runs all daily tests. `php artisan soc2:evidence-bundle --control=... --from=... --to=...` builds an evidence archive. Code: `app-modules/soc2/`; tests: `app-modules/soc2/tests/Feature/{ControlTest,Exception,Evidence}Test.php` and `tests/Audit/ContinuousControlMonitoringTest.php` (asserts the daily cron ran for the past 30 days).

#### 23.2.1 Field-level encryption — `field-level-encryption` (V2.1, MML 3)

**1. Overview.** V1 stores PHI in plain columns in MySQL (encryption at rest is handled by RDS / ElastiCache disk-level encryption). V2.1 introduces **per-field, per-tenant application-layer encryption**: a designated subset of columns is encrypted with the tenant's KMS key (per `03-Tenancy §17.6` + `20-Security §23.2.2` BYOK) before the value is written to MySQL; decryption happens at the API boundary. The result: a database dump or a stolen snapshot is **useless** without the tenant's KMS key. MML 3 GA: the `EncryptedFieldCast`, the searchable-token sidecar (for fields that must remain queryable), the data-classification registry, the migration tool for already-encrypted data, and the audit integration.

**2. The cast.**
```php
class EncryptedFieldCast implements CastsAttributes {
    public function get($model, string $key, $value, array $attributes): ?string {
        if ($value === null) return null;
        return app(TenantKms::class)->decrypt($model->tenant_id, $value);
    }
    public function set($model, string $key, $value, array $attributes): array {
        if ($value === null) return [$key => null];
        return [$key => app(TenantKms::class)->encrypt($model->tenant_id, $value)];
    }
}
```

**3. Fields encrypted at rest (V2.1 GA).**
- `patients.aadhaar` (India national ID, 12 digits).
- `patients.ssn` (US Social Security Number).
- `doctors.pan` (India PAN, 10 chars).
- `doctors.medical_license_number`.
- `prescriptions.notes` (free-text, may contain PHI).
- `audit_logs.ip` (PII, encrypted).
- `user_identities.profile` (OIDC claims, may contain PII).
- `transactions.card_last4` (already in payment_gateway; redundant but kept for defense in depth).

**4. Searchable-token sidecar.** Some encrypted fields must be searchable (e.g., the staff needs to look up a patient by Aadhaar). For these, we maintain a **deterministic encryption token** in a separate column (`patients.aadhaar_search`) computed as `HMAC-SHA256(tenant_kms_key, aadhaar_value)`. The token is deterministic (same input → same output) but non-reversible without the key. Equality queries (`WHERE aadhaar_search = ?`) work; range queries do not (and we don't need them for ID numbers). The search token is also rotated on key rotation (the DEK changes; the search token changes; we re-compute in batch).

**5. Data classification registry.** Every column in the database is tagged in `data_classification`:
```sql
CREATE TABLE data_classification (
    table_name VARCHAR(64),
    column_name VARCHAR(64),
    classification VARCHAR(16),                            -- 'phi' | 'pii' | 'financial' | 'public'
    encryption_required BOOLEAN,                           -- if true, EncryptedFieldCast is mandatory
    retention_days INT,                                    -- 7 years for HIPAA, varies for PII
    PRIMARY KEY (table_name, column_name)
);
```
A daily cron (`classify:sweep`) scans the schema and asserts every column tagged `encryption_required=true` has a `cast='encrypted'` in the Eloquent model. A drift opens a P1 incident.

**6. Migration tool.** V2.1 ships a one-time `php artisan encryption:backfill` that:
1. For each row, encrypts the value with the tenant's key.
2. Writes the ciphertext to the column.
3. Computes the search token (if applicable) and writes it to the sidecar column.
4. Marks the row as migrated.
5. After all rows are migrated, the original column is dropped (in a separate, post-migration step) and the encrypted column is renamed.
The migration is reversible: the tenant can re-activate V1 mode and decrypt back to plain (only if the tenant's key is still accessible).

**7. Performance.** Each PHI read now incurs one KMS decrypt call (cached DEK for 5 min). A 1,000-patient search by Aadhaar: 1 KMS call to compute the search token, 1 SQL query (`WHERE aadhaar_search = ?`), no per-row decrypt. A 100-row report export: 100 KMS calls (parallel, batched 50/wave). p99 of an API call that touches 5 encrypted fields: +30ms vs V1. The DEK cache is per-process and per-tenant; cross-process consistency is fine because the key is the same.

**8. Acceptance Criteria.**
- [ ] Every column tagged `encryption_required=true` is encrypted at rest; a `mysqldump` of the table shows only ciphertext.
- [ ] Decryption happens only at the API boundary; a SQL query directly against the encrypted column returns ciphertext, not plaintext.
- [ ] The search-token sidecar works: `WHERE aadhaar_search = ?` returns the matching patient in O(log n).
- [ ] A key rotation in the tenant's KMS causes the search tokens to be re-computed within 24h (the DEK batch job).
- [ ] The `classify:sweep` cron finds 0 drift between the data_classification registry and the Eloquent casts.
- [ ] A tenant disabling BYOK (reverting to platform KMS) re-encrypts the data with the platform key without downtime (rolling re-encryption).
- [ ] Performance: p99 of an API call touching 5 encrypted fields is < 200ms (including KMS latency).

**9. Real-World Test Scenarios.**
- *Dump theft:* attacker exfiltrates a MySQL dump; the encrypted columns are useless without the tenant's KMS key; the search tokens are also useless without the key.
- *Search:* staff types "1234-5678-9012" in the patient lookup; the system computes the search token; the query matches; the patient record opens.
- *Key rotation:* tenant rotates their KMS key; the data is re-encrypted in a batch job (rolling); the search tokens are re-computed; the audit log shows `encryption:re_encrypt` and `encryption:re_tokenize` events.
- *Drift detection:* a developer adds a new PHI column without the cast; the daily `classify:sweep` flags it; the P1 is opened; the developer adds the cast; the sweep is green.
- *BYOK disabled:* tenant deactivates BYOK; the system re-encrypts the data with the platform KMS key; the audit log shows the re-encryption; the tenant can later re-activate BYOK without data loss.
- *Performance:* 100k patients exported to CSV; the export is parallelized across 8 workers; each batch reuses the DEK; total export time < 5 min.

**10. What this module is NOT:**
- Not a replacement for disk-level encryption (RDS, S3, ElastiCache). Those are still on; field-level encryption is **in addition**.
- Not searchable for all fields. Free-text search on encrypted fields requires a separate encrypted-search index (e.g., OpenSearch with encrypted doc values); that is V3+.
- Not a key management system. We rely on AWS KMS / GCP KMS / Vault for the actual key material; we just orchestrate the calls.

**11. Dev Workflow.** `php artisan encryption:backfill --tenant=...` runs the one-time migration. `php artisan classify:sweep` runs the daily drift check. `php artisan encryption:re-encrypt --tenant=...` triggers a rolling re-encryption. `php artisan encryption:audit --tenant=...` shows the decryption audit. Code: `app-modules/field-level-encryption/`; tests: `app-modules/field-level-encryption/tests/Feature/{Cast,SearchToken,Backfill,Rotate}Test.php` and `tests/Performance/encrypted-read.k6.js`.

**Fields encrypted at rest:**
- `patients.aadhaar` (India national ID, 12 digits).
- `patients.ssn` (US Social Security Number).
- `doctors.pan` (India PAN, 10 chars).
- `doctors.medical_license_number`.
- `transactions.card_last4` (already in payment_gateway; redundant).
- `prescriptions.notes` (free-text, may contain PHI).
- `audit_logs.ip` (PII, encrypted).
- `user_identities.profile` (OIDC claims).

**Fields NOT encrypted** (need to be searchable):
- `users.email`, `users.phone` (indexed).
- `appointments.start_at`, `appointments.status` (indexed).

#### 23.2.2 BYOK (Bring Your Own Key) — `byok` (V2.1, MML 3)

**1. Overview.** V2 ships a per-tenant KMS key (per `03-Tenancy §17.6`) that **we** provision and **we** hold in our AWS/GCP account. The tenant trusts us with the key. V2.1 introduces **BYOK**: the tenant provides their own KMS key, hosted in **the tenant's own** AWS/GCP account (or on-prem HSM), and we request encrypt/decrypt operations against it. If the tenant revokes our access, **we cannot decrypt** any of their data — including backups. This is the strongest possible trust guarantee and is required by a small but high-value segment (defense, certain EU hospitals, certain US health systems). MML 3 GA: AWS KMS, GCP KMS, and HashiCorp Vault Transit support; key rotation; on-tenant import; emergency-revoke flow.

**2. Configuration (per the existing stub; expanded).**
```bash
# Tenant .env (or admin UI under /admin/security/byok)
MEDICARE_TENANT_KMS_PROVIDER=aws                       # 'aws' | 'gcp' | 'hashicorp_vault' | 'azure'
MEDICARE_TENANT_KMS_KEY_ID=arn:aws:kms:us-east-1:123456789:key/abcd-...
MEDICARE_TENANT_KMS_REGION=us-east-1
# For GCP
MEDICARE_TENANT_KMS_PROVIDER=gcp
MEDICARE_TENANT_KMS_KEY_ID=projects/.../locations/.../keyRings/.../cryptoKeys/...
# For HashiCorp Vault Transit (on-prem HSM or Vault cluster)
MEDICARE_TENANT_KMS_PROVIDER=hashicorp_vault
MEDICARE_TENANT_KMS_TRANSIT_PATH=transit/keys/medicare-tenant-42
MEDICARE_TENANT_KMS_VAULT_ADDR=https://vault.tenant.example.com
# For Azure Key Vault
MEDICARE_TENANT_KMS_PROVIDER=azure
MEDICARE_TENANT_KMS_KEY_VAULT_URL=https://tenant.vault.azure.net/
MEDICARE_TENANT_KMS_KEY_NAME=medicare-tenant-42
```

**3. Trust model.** With BYOK, the trust boundary moves:
- **Before BYOK:** the tenant trusts us with the KMS key. We can decrypt their data at will (within the audit-logged access).
- **After BYOK:** the tenant holds the key. We request encrypt/decrypt operations; the tenant's KMS authorizes each. If the tenant revokes our IAM role (AWS) or revokes the Vault token (HashiCorp), **we cannot decrypt** — every PHI read fails.
- This is true customer-controlled encryption. The cost is operational: the tenant's KMS becomes a hard dependency for our service. We mitigate by caching the data-encryption-key (DEK) for a short TTL (5 min) so a brief KMS blip doesn't take down the service.

**4. Key rotation.** Tenants can rotate their key in their KMS at any time (e.g., AWS `RotateKeyOnUpdate`). Our service detects the rotation (the key's `KeyId` is stable; the `Version` changes) and re-wraps our cached DEKs with the new key version. The old key version remains for decrypt-only for 30 days (configurable); after that, the tenant is expected to have re-encrypted all data with the new version; we refuse to decrypt with a key version older than 30 days unless explicitly allowed.

**5. The `EncryptedFieldCast` (per §23.2.1)** wraps each PHI column with `TenantKms::encrypt` / `decrypt`. The cast's call is async-safe: it batches up to 100 DEK requests to the tenant's KMS to amortize latency. The DEK cache is per-process and per-tenant; it is **not** persisted (so a process restart forces a re-fetch, which is fine).

**6. Onboarding flow.**
1. Tenant admin goes to `/admin/security/byok` and picks a provider.
2. They follow the provider-specific onboarding:
   - **AWS:** create a KMS key in their account; create an IAM role with `kms:Encrypt`, `kms:Decrypt`, `kms:GenerateDataKey`, `kms:DescribeKey` permissions; provide the role ARN; we assume the role via `sts:AssumeRole` with an external ID.
   - **GCP:** create a KMS keyring + key; create a service account; grant `roles/cloudkms.cryptoKeyEncrypterDecrypter`; provide the service account key (JSON, uploaded once, stored in our Secrets Manager).
   - **HashiCorp Vault:** create a Transit key; issue a token with `transit/keys/medicare-tenant-42` policy; provide the Vault URL + token.
   - **Azure:** create a Key Vault + key; grant our app registration `Key Vault Crypto User`; provide the vault URL + key name.
3. We test the configuration with a `kms:health-check` (encrypt + decrypt a known plaintext); on success, we flip the tenant's `kms_mode='byok'`.
4. From this point on, every PHI read/write goes through the tenant's KMS. We audit every decrypt call in `audit` with `action='kms.decrypt'` and the result.

**7. Emergency revoke.** If the tenant's key is compromised (or as a test), the tenant revokes our IAM role / Vault token. Within 5 minutes (the DEK cache TTL), our service cannot decrypt any PHI; reads return 500 with a "Encryption key unavailable" error. The tenant's data is safe — we cannot read it. The recovery is: the tenant re-grants access (with a new IAM role or Vault token); we re-test; we resume.

**8. Backup implications.** Backups are encrypted at rest with the same KMS key (BYOK applies to S3, RDS, and ElastiCache encryption). The DR region replica is also encrypted with the tenant's key (the secondary region needs cross-region access; the tenant must either replicate the key (AWS `ReplicaKey`) or grant cross-account access; both are documented in the onboarding guide).

**9. Domain Model.**
```php
Schema::create('tenant_kms_configs', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('tenant_id')->unique();
    $t->string('provider', 32);                             // 'aws' | 'gcp' | 'hashicorp_vault' | 'azure'
    $t->string('key_id_or_path', 255);
    $t->string('region', 32)->nullable();
    $t->json('auth_config')->nullable();                    // role_arn, vault_token_ref, etc.
    $t->string('external_id', 64)->nullable();              // AWS external ID
    $t->string('kms_mode', 16)->default('platform');        // 'platform' | 'byok'
    $t->string('health_status', 16)->default('unknown');
    $t->timestamp('last_healthcheck_at')->nullable();
    $t->integer('healthcheck_latency_ms')->nullable();
    $t->text('last_healthcheck_error')->nullable();
    $t->timestamp('key_rotated_at')->nullable();
    $t->timestamps();
});
```

**10. API Surface.**
```
GET    /api/v2.1/security/byok                             # current config (secrets redacted)
POST   /api/v2.1/security/byok                             # { provider, key_id, auth_config }
POST   /api/v2.1/security/byok/test                        # test encrypt+decrypt
POST   /api/v2.1/security/byok/activate                    # flip mode to 'byok'
POST   /api/v2.1/security/byok/deactivate                  # revert to platform KMS
POST   /api/v2.1/security/byok/rotate                      # trigger re-wrap
GET    /api/v2.1/security/byok/audit                       # kms.decrypt audit
```

**11. Frontend.** `/admin/security/byok` is a single page with a wizard: pick provider → paste/upload config → test → activate. After activation, the page shows: provider, key ID (truncated), health status, last 30 days of decrypt audit, and a "Test emergency revoke" button (runs a dry-run that flips the role's policy to deny; runs for 5 min; flips back). The page warns the tenant that activating BYOK may briefly impact read performance (the first batch of decrypts is slow; subsequent are cached).

**12. Realtime.**
- `kms.health.degraded.{tenant_id}` — health check fails 3x in a row; tenant admin paged.
- `kms.key.rotated.{tenant_id}` — re-wrap complete.
- `kms.access.revoked.{tenant_id}` — emergency revoke detected; tenant admin notified that the platform is in degraded mode.

**13. Security & RBAC.** Roles: `tenant_admin` (configure BYOK, test, activate, deactivate, emergency revoke), `security_engineer` (view audit, run health check), `compliance_officer` (view audit). Per `20-Security §11.4`, the emergency-revoke test requires co-sign. The auth_config (Vault token, GCP service account JSON) is stored in AWS Secrets Manager and is **never** returned by the API; the UI shows a "rotate now" button that re-issues the credential. The audit log of every `kms.decrypt` is a SOC 2 control (CC6.6) and an ISO 27001 control (A.10.1).

**14. Acceptance Criteria.**
- [ ] Activating BYOK causes all subsequent PHI reads to call the tenant's KMS.
- [ ] Revoking the tenant's IAM role causes PHI reads to fail with 500 within 5 minutes; re-granting restores service.
- [ ] A key rotation in the tenant's KMS is detected within 1 hour; the DEKs are re-wrapped.
- [ ] The DEK cache is per-process and per-tenant; a process restart forces a re-fetch.
- [ ] The emergency-revoke test (5-min dry run) succeeds without data loss.
- [ ] Backups and the DR region replica are also encrypted with the tenant's key.
- [ ] The `kms.decrypt` audit shows every PHI read; the volume matches the expected read traffic.

**15. Real-World Test Scenarios.**
- *Onboarding:* US hospital chain picks AWS KMS; creates the key and IAM role; pastes the role ARN; clicks Test → green; clicks Activate → all PHI is now encrypted with their key; the page shows "BYOK active".
- *Key rotation:* AWS rotates the key automatically (annual); we detect the new version within 1 hour; DEKs are re-wrapped; the next read uses the new version; the audit log shows `kms.decrypt` calls succeeding throughout.
- *Compromise:* the tenant's KMS admin suspects a compromise; they revoke our IAM role; within 5 min, all PHI reads fail; the tenant is on the phone with us; we acknowledge; the tenant issues a new IAM role; we re-test; we re-activate; service resumes; no data loss.
- *Performance:* with BYOK active, p99 of an API call that decrypts 10 PHI fields is +50ms vs platform KMS (the DEK batch is the difference). The performance is monitored; a >200ms regression opens a P3.
- *Vault token expiry:* the tenant's Vault token expires (TTL 24h, not auto-renewed); the DEK cache is still warm for 5 min; after that, reads fail; the tenant is paged; they re-issue a new token; service resumes.
- *DR drill:* the quarterly DR drill fails over to the secondary region; the secondary region can decrypt because the tenant replicated the KMS key (AWS `ReplicaKey`); the drill's RPO and RTO are unchanged.

**16. What this module is NOT:**
- Not a free add-on. BYOK is Enterprise-tier only; the operational complexity is real.
- Not a substitute for tenant-side controls. BYOK protects the data at rest from us; it does not protect against a compromised tenant admin.
- Not a HSM. We use the tenant's KMS (which may be backed by an HSM, e.g., AWS CloudHSM); we do not provide an HSM.

**17. Dev Workflow.** `php artisan byok:test --tenant=...` runs a health check. `php artisan byok:rotate --tenant=...` triggers a re-wrap. `php artisan byok:emergency-revoke-test --tenant=... --duration=5m` runs the dry-run. Code: `app-modules/byok/`; tests: `app-modules/byok/tests/Feature/{Onboard,Rotate,EmergencyRevoke,Health}Test.php` and `tests/Performance/byok-latency.k6.js`.

#### 23.2.3 Plugin security model

Plugin Core (§3.3a in `14-MODULE-V2-STUBS`) ships in V2; Public Marketplace and Plugin Monetization land in V3. The security model is shared across all three surfaces.

**1. Capability model.** A plugin declares the capabilities it needs in its `plugin_versions.manifest.permissions` (e.g., `read:patient`, `write:appointment`, `send:notification`, `access:lab_result`). At install time, the tenant admin sees the manifest and must explicitly grant each capability. Granted capabilities are stored on `plugin_installs.config.permissions_granted`. A plugin attempting to call an API endpoint outside its granted set returns **HTTP 403** and the attempt is logged in `audit_logs` with `action = 'plugin.permission_denied'`.

**2. Sandboxed execution.** Plugins run in-process for V2 (faster, simpler ops, single deploy per 14-V2-Stubs §3.3a execution model). Each plugin invocation runs inside a PHP `try/catch` boundary that:
- Enforces a 5-second wall-clock timeout (returns 504 to plugin caller).
- Enforces a 32 MB memory cap (returns 507 to plugin caller).
- Catches all exceptions and logs them to `audit_logs` with `action = 'plugin.exception'`. The main process is not affected.

Sidecar mode (V3) is gated by `feature_plugin_sidecar`; sidecar runs as a separate PHP-FPM worker pool with stricter memory and no direct DB connection (must call our internal API).

**3. Artifact integrity.** Each `plugin_versions` row has a `signature` column. The signature is verified at install time against the developer's public key (registered when the developer account is approved). A failed signature check returns 422 and the install is aborted.

**4. Code review before listing.** Plugins in the Public Marketplace (V3) require a manual code review by a Medicare engineer before `status = 'published'`. The review covers: declared capabilities match actual API calls, no exfiltration patterns, no obfuscated code, no network calls to non-allowlisted hosts.

**5. Kill switch.** `POST /api/v2/plugins/{slug}/disable` (super-admin only) flips `plugin_installs.status` to `disabled` and emits `plugin.disabled` on `plugin.{tenant}` channel. All subsequent plugin invocations return 503. The plugin code remains on disk for forensic review.

**6. Audit trail.** Every plugin operation is logged in `audit_logs` with the action prefix `plugin.` (install, enable, disable, upgrade, uninstall, permission_denied, exception, health_check_failed). Retained 7 years per V2 §23.4.1.

**7. No PHI exfiltration.** Plugins can read PHI they have been granted access to, but **cannot** make outbound network calls (egress filter blocks all by default; explicit allowlist per plugin). Plugins also cannot write to the filesystem outside their sandbox directory.

**8. Tenancy isolation.** All plugin code runs inside the tenant's own request lifecycle, with the same `BelongsToTenant` scoping (per 14-V2-Stubs §6.5). A plugin cannot read or write another tenant's data even if it bypasses our services.

#### 23.2.5 HL7 v2 ADT/ORU (V3) — IHE ATNA audit + MLLP mutual TLS

The HL7 v2 integration (`19-Integrations §15.10`) is the integration surface with the largest attack surface (inbound from external hospital systems), so it has its own dedicated controls:

**1. IHE ATNA audit logging.** Every ADT and ORU message generates an `audit_logs` row conforming to the IHE ATNA (Audit Trail and Node Authentication) profile. The `audit_logs` row carries: `action = 'hl7v2.message.received'`, `entity_type = 'hl7v2_message'`, `event_outcome` (`success | processing_error | reject`), `source_ip` (MLLP peer), `user_id` is `NULL` (machine actor), `metadata` JSON with `message_type`, `control_id`, `sending_facility`, `sending_application`, `affected_entity_type`, `affected_entity_id`. This is the same audit row format used for V2 audit (per §23.4.1) and is what feeds Splunk / Datadog SIEM.

**2. MLLP mutual TLS.** The MLLP listener on port `6661` requires mTLS: the partner's certificate must match the fingerprint registered in `tenants.integrations.hl7v2.partner_cert_sha256`. A certificate mismatch returns TCP RST and the connection is dropped before any HL7 payload is parsed. mTLS cert rotation is a 30-day overlap window (old + new cert valid simultaneously).

**3. Schema allowlist.** The Mirth sidecar only accepts HL7 v2 messages whose `MSH.9` (message type) and `MSH.9.2` (trigger event) are in the V3 GA allowlist: `ADT^A01, ADT^A03, ADT^A04, ADT^A08, ORU^R01`. Any other message type is rejected with an HL7 `AR` (Application Reject) acknowledgment and an `audit_logs.action = 'hl7v2.unsupported_type'`.

**4. PHI redaction in error logs.** When a message is quarantined, the `hl7v2_quarantine.error_message` column contains the parser's diagnostic but **never** includes segment content past `PID.5` (name) or `PID.7` (DOB). Admins see "schema invalid at PID-3: expected ST, got CX" but never the patient name.

**5. Quarantine replay is auditable.** Replaying a quarantined message (`POST /api/v3/integrations/hl7v2/quarantine/{id}/replay`) requires the `admin.integrations` permission and writes a fresh `audit_logs` row linking the original `hl7v2_messages.id` to the replay attempt. Replays that succeed generate the same downstream `audit_logs` rows as a fresh inbound message.

**6. IHE profile compliance target.** V3 GA targets compliance with **IHE ITI-1 (ATNA)** for audit, **IHE ITI-2 (XDS.b)** for document sharing, and **IHE ITI-19 (PIX)** for patient identity cross-referencing. The first two are partial (we audit and store, but do not register as an XDS Affinity Domain in V3 — that's a V4 milestone for `#106 HIS Bridge`).

#### 23.2.4 Data access log

Every PHI read is logged with reason (already added in V1 as a feature flag; V2 makes it default-on).

```php
class AccessLog {
    public static function record(string $action, Model $subject, string $reason): void {
        DataAccessLog::create([
            'tenant_id' => app('tenant')->id,
            'user_id' => auth()->id(),
            'patient_id' => $subject instanceof Patient ? $subject->id : $subject->patient_id,
            'entity_type' => $subject->getMorphClass(),
            'entity_id' => $subject->getKey(),
            'action' => $action,                                 // 'read' | 'export' | 'print' | 'share'
            'reason' => $reason,                                 // 'Consultation #1234' | 'Patient request'
            'ip' => request()->ip(),
            'user_agent' => request()->userAgent(),
        ]);
    }
}
```

Patient app: "Who accessed my data?" → GDPR Article 15 response generated in 5 seconds.

**Anomaly detection** (V2.1): ML on `data_access_log` flags unusual patterns (e.g., user reading 100 patients in 1 hour). Email alert to user + admin.

#### 23.2.4 AI prompt firewall

```php
class PromptFirewall {
    private const INJECTION_PATTERNS = [
        '/ignore\s+(previous|all)\s+instructions/i',
        '/you\s+are\s+now\s+/i',
        '/system\s*:/i',
        '/forget\s+everything/i',
        // 30+ more patterns
    ];

    public function checkInput(string $input): FirewallResult {
        foreach (self::INJECTION_PATTERNS as $pattern) {
            if (preg_match($pattern, $input)) {
                return new FirewallResult(blocked: true, reason: 'injection_detected');
            }
        }
        // Length check
        if (strlen($input) > 5000) {
            return new FirewallResult(blocked: true, reason: 'too_long');
        }
        // PHI detection
        if ($this->containsPhi($input)) {
            return new FirewallResult(blocked: true, reason: 'phi_in_prompt');
        }
        return new FirewallResult(blocked: false);
    }

    public function checkOutput(string $output): FirewallResult {
        // Hallucination check
        if ($this->containsKnownHallucination($output)) {
            return new FirewallResult(blocked: true, reason: 'hallucination');
        }
        // Medical advice without disclaimer
        if ($this->isMedicalAdvice($output) && !$this->hasDisclaimer($output)) {
            return $output . "\n\n⚠️ This is AI-generated information. Please consult a doctor.";
        }
        return new FirewallResult(blocked: false);
    }
}
```

Every AI call goes through the firewall. Blocked requests are logged + rate-limited user is warned.

#### 23.2.5 Encrypted backups with cross-region DR

```bash
# V1 backup: mysqldump to S3 (unencrypted, lifecycle to Glacier)
# V2 backup:
mysqldump ... | gpg --symmetric --cipher-algo AES256 --passphrase-file /run/secrets/backup-pass | aws s3 cp - s3://medicare-backups/tenant-42/2026-06-04.sql.gz.gpg --sse aws:kms --sse-kms-key-id alias/medicare-backup
```

- **Per-tenant passphrase** generated at install, stored in Vault.
- **KMS encryption** on S3 (envelope).
- **Cross-region copy:** `s3 cp s3://medicare-backups-us-east-1/... s3://medicare-backups-us-west-2/... --sse aws:kms --sse-kms-key-id alias/medicare-dr-key`.
- **Restore drill:** quarterly, automated, with measurable RTO/RPO.
- **WORM** (Write Once Read Many) for legal hold: S3 Object Lock with 7-year retention.

#### 23.2.6 SPIFFE service identities

V2 sidecar services authenticate with SPIFFE IDs:

```
spiffe://medicare.app/ns/medicare-ai/sa/ai-service
spiffe://medicare.app/ns/medicare-lab/sa/lab-gateway
```

`Linkerd` mTLS issues short-lived (24h) certificates. Service-to-service calls verified via the mesh, no API keys needed.

#### 23.2.7 WebRTC DTLS-SRTP (V2 telemedicine)

V2 telemedicine uses LiveKit, which enforces:
- DTLS-SRTP for media encryption (no plain RTP).
- Room tokens scoped to the appointment and the user's role.
- Recording encryption at rest (S3 SSE-KMS).
- No raw media leaves the LiveKit cluster; relay servers decrypt but can't read (server-side encryption with LiveKit-held keys).

#### 23.2.8 SSRF / CSRF / SSRF defenses (V2 add)

- All outbound HTTP from Laravel goes through a single `HttpClient` wrapper that blocks private IPs (10/8, 172.16/12, 192.168/16, 169.254/16) unless explicitly allowlisted.
- LLM tool calls go through this wrapper; AI cannot reach internal services.
- CSRF tokens rotated on every state-changing request, even for SPA (Next.js Server Actions).

### 23.3 V2 privacy features

#### 23.3.1 Differential privacy for analytics (V3)

When training internal ML models (e.g., "average no-show rate by city"), we add Laplace noise so individual records are not re-identifiable. ε=1.0 budget per query.

#### 23.3.2 Homomorphic encryption for cohort queries (V4)

Enterprise tenants can run "find all patients with HbA1c > 7 in last 90 days" without us seeing the underlying values. Uses Microsoft SEAL or Zama Concrete.

#### 23.3.3 Patient consent management

```php
Schema::create('patient_consents', function (Blueprint $t) {
    $t->ulid('id')->primary();
    $t->ulid('patient_id')->index();
    $t->string('purpose', 64);                                // 'research' | 'marketing' | 'data-sharing' | 'ai-training' | 'abdm-sync'
    $t->boolean('granted');
    $t->timestamp('granted_at')->nullable();
    $t->timestamp('revoked_at')->nullable();
    $t->json('scope')->nullable();                            // specific data types, specific recipients
    $t->timestamp('expires_at')->nullable();
    $t->timestamps();
    $t->index(['patient_id', 'purpose', 'granted']);
});
```

Every PHI operation checks consent. Withdrawal is immediate and propagated to all downstream services.

### 23.4 V2 audit & logging

#### 23.4.1 Audit log additions

- **Action types expanded:** V1 had 30+ actions. V2 adds 50+ more for: lab orders, video calls, AI calls, ABHA fetches, insurance claims, plugin operations.
- **Per-action retention:** some actions (login) retained 1y, some (PHI access) 7y, some (financial) forever.
- **Append-only enforcement:** same as V1 (DB trigger blocks UPDATE/DELETE).
- **Cryptographic chain:** each row has `prev_hash = SHA256(this_row - this_hash)`, forming an immutable chain (already V1, hardened in V2 with HMAC-SHA256 + KMS key).
- **Real-time streaming:** audit events also stream to S3 via Kinesis Firehose for SIEM ingestion (Splunk, Datadog).

#### 23.4.2 Breach detection automation

- ML on `data_access_log`: anomalous access patterns → alert.
- ML on `audit_logs`: unusual state transitions → alert.
- ML on `login_audits`: brute force, impossible travel → alert.
- 72-hour notification timer starts automatically on confirmed breach.

### 23.5 V2 secure SDLC

| Phase | V1 | V2 addition |
|---|---|---|
| Design | Threat model | STRIDE + AI red-team |
| Code | Code review | SAST (Semgrep), secrets scan (TruffleHog) |
| Build | CI tests | SCA (Snyk), license check (FOSSA) |
| Test | Unit + feature | DAST (ZAP), IAST (Contrast) in staging |
| Deploy | Manual approval | SBOM attestation, signed images (cosign) |
| Run | Logs | Runtime protection (Datadog ASM, Falco) |
| Incident | Manual | PagerDuty, runbook automation, post-mortem template |

### 23.6 V2 third-party risk

Every V2 integration has a vendor risk assessment:
- Security questionnaire (CAIQ, SIG).
- SOC 2 Type II report.
- Penetration test report (annual).
- BAA / DPA in place before PHI exchange.
- Listed in `vendors/{vendor}.md` with risk rating, contract renewal date.

### 23.7 V2 acceptance criteria

- [ ] SOC 2 Type I audit passes (V2 GA).
- [ ] SOC 2 Type II audit passes (V2.1).
- [ ] All V2 PHI fields encrypted at rest; CI verifies.
- [ ] BYOK tenants' data is unreadable without their key.
- [ ] Data access log captures every PHI read; queryable in <5s.
- [ ] AI prompt firewall blocks 100% of red-team attacks.
- [ ] All V2 services use mTLS via SPIFFE.
- [ ] Cross-region DR drill completes in <1h with RPO <5min.
- [ ] GDPR DPIA for AI features reviewed by DPO.
- [ ] ABDM M1/M2/M3 certification achieved.
- [ ] All V2 vendors have a current SOC 2 report on file.
