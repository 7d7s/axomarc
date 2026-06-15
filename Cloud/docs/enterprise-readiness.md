# Enterprise Readiness — SOC 2, ISO 27001, BSI C5, EUCS, and the MNC buyer's checklist

**Status:** Locked for V1+. Updated as the framework mappings and penetration test plan are published.
**Audience:** The procurement officer. The CISO. The auditor. The enterprise sales lead. The first 5 employees. The design partners who are themselves MNCs.
**Last updated:** 2026-06-04

This document is the **answer to "are you enterprise-ready?"** for any multinational corporation (MNC), EU public-sector buyer, or regulated-industry customer evaluating Sovereign. It is opinionated, sourced, and concrete. Every claim is grounded in a real framework control, a real persona-cto.md §3 ("Technology bets"), a real user-pain-research.md §6, or a real competitive-landscape.md comparison. There is no marketing copy. There are no aspirational claims. There is no "we're working on it" handwaving.

The document is structured as: **(1) the support tiers + SLAs**, **(2) the framework mappings** (SOC 2, ISO 27001, BSI C5, EUCS, GDPR), **(3) the penetration test plan**, **(4) the audit + compliance evidence bundle** (I17), **(5) the data processing addendum template**, **(6) the enterprise onboarding playbook**, and **(7) the "what we explicitly do not promise" list**.

---

## 1. The three support tiers (and what each one actually buys)

The product surface is the same Apache 2.0 binary across all three tiers. The tiers differ in *service* (managed updates, support, compliance documentation, SLAs) and *procurement* (contract structure, payment terms, data residency) — never in *features*. This is the Plausible model and the Cal.com model. The persona-cto.md §5.3 ("Pricing tiers — Community / Pro / Enterprise") is the source.

### 1.1 Community (free, Apache 2.0, self-supported)

| Item | Value |
|---|---|
| **Price** | €0 / forever |
| **License** | Apache 2.0, unmodified, no carve-outs |
| **Support** | None (GitHub Discussions, community Discord, no SLA) |
| **Updates** | Self-managed: `sovereign update --channel stable` |
| **Security advisories** | Public on GHSA; no embargo |
| **SLA on response** | None |
| **SLA on uptime** | None (you run it, you own the uptime) |
| **Compliance docs** | The Apache 2.0 LICENSE + the SBOM + the cosign signature + the source code. That's it. |
| **Data residency** | Wherever the operator runs the binary |
| **Procurement** | Self-serve download; no contract needed |
| **Target buyer** | Mira (solo founder), Karim (agency), Alex (vibe coder) |

**What this tier is good for:** a 1-person team deploying 1-12 apps on a single VPS. The full product, no support, no compliance paperwork, no contract.

### 1.2 Pro (managed updates + support, €19/server/month)

| Item | Value |
|---|---|
| **Price** | €19 / server / month, billed annually (€228/server/year) |
| **License** | Same Apache 2.0 binary; the *update channel* is the paid service |
| **Support** | Email support (`support@sovereignruntime.dev`); 1 business-day response on weekdays (Mon-Fri, 09:00-18:00 CET) |
| **Updates** | Managed: `sovereign update --channel pro` (auto-update timer available); the operator can also opt in to a 24-hour delay before patch rollout |
| **Security advisories** | Embargoed for 24 hours for Pro customers; the public GHSA is published 24 hours after the embargo |
| **SLA on response** | 1 business day for Sev1/Sev2; 3 business days for Sev3 |
| **SLA on uptime** | None (you run it, you own the uptime); the Pro tier does not host your apps |
| **Compliance docs** | The Apache 2.0 LICENSE + SBOM + cosign signature + the *Pro tier DPA* (template, see §5) + a per-release *security advisory digest* (PDF) |
| **Data residency** | Operator's choice; the Pro binary is the same one as Community (no telemetry, no phone-home, no data leaves the box) |
| **Procurement** | Self-serve via the website; credit card or SEPA; no contract negotiation for the standard Pro tier |
| **Target buyer** | Lin (startup infra, 5-50 servers), Karim's agency, an early-stage startup that wants the managed update channel |

**What this tier is good for:** a 5-50-server operation that wants the patch flow + support to be someone else's problem. The operator still owns the box, the data, the uptime, the DR.

### 1.3 Enterprise (compliance + on-prem + dedicated support, €25k-100k/year)

| Item | Value |
|---|---|
| **Price** | Starts at €25,000 / year (annual contract); scales with server count, support tier, and audit requirements |
| **License** | Same Apache 2.0 binary + an *Enterprise support contract* (signed) |
| **Support** | Dedicated EU support engineer; 24/7 on-call for Sev1; Slack Connect channel; quarterly business review |
| **Updates** | Managed, with a 30-day embargo window between public release and the Enterprise channel rollout (so the customer's change-advisory board can review the release notes before production deployment) |
| **Security advisories** | Embargoed for 30 days; private GHSA access; CVE feed in the customer's ticketing system (Jira / ServiceNow / Zendesk integration) |
| **SLA on response** | 1 hour for Sev1, 4 hours for Sev2, 1 business day for Sev3, 24/7/365 |
| **SLA on uptime** | 99.9% on the Pro update channel availability (the customer still owns their own apps' uptime); 99.99% on the Enterprise release pipeline (the signed binary delivery) |
| **Compliance docs** | All of Pro + the *BSI C5:2026 mapping* (I11) + the *EUCS Substantial controls checklist* (I12) + the *SOC 2 Type II report* (when available, year 3-4) + the *ISO 27001 Annex A mapping* (year 3-4) + a *penetration test summary* (annual, year 2+) + a *Data Processing Addendum* (signed, DPA template §5) + a *Sub-Processor List* + a *Vendor Risk Questionnaire* (CAIQ / SIG-Lite filled) |
| **Data residency** | EU-only by default; the customer's choice of EU region (Frankfurt, Dublin, Stockholm, Paris); on-prem install supported (the binary runs in the customer's air-gapped network) |
| **Procurement** | Signed contract; SOC 2 / ISO 27001 evidence package on request; redlines supported; MePA / Consip catalog (IT) for Italian PA buyers; CPV code 72260000 |
| **Target buyer** | Sara (EU public sector), an MNC's infrastructure team, a regulated-industry buyer (finance / health / defense) |

**What this tier is good for:** an MNC or public-sector buyer that needs the *paper trail* — the signed contract, the C5 mapping, the EUCS checklist, the DPA, the pen-test report, the on-prem install, the 24/7 SLA. The features are the same as Pro; the *compliance surface* is the differentiator.

### 1.4 The "what's the difference between Pro and Enterprise?" summary

In one sentence: **Pro is "we manage the update channel and answer your support tickets"; Enterprise is "we are your vendor of record for the procurement office, with the signed contract, the C5 mapping, the EUCS checklist, the pen-test, the DPA, and the 24/7 SLA."**

If the customer is a 5-server startup, they buy Pro. If the customer is a 50-server MNC, they buy Enterprise. If the customer is an EU PA, they buy Enterprise and require the C5 mapping + the EUCS checklist as a contract attachment.

---

## 2. The support SLA matrix (the contractual commitments)

These are the published SLAs. They are part of the Pro and Enterprise contracts. They are measurable, with a financial credit if missed (the credit is the customer's next month's fee, capped at 100% of the monthly fee for Pro, 100% of the annual fee for Enterprise).

### 2.1 Response time SLA (the time to first human reply)

| Severity | Definition | Pro SLA | Enterprise SLA |
|---|---|---|---|
| **Sev1 — production down** | The control plane is unreachable, the binary fails to start, or a security advisory is being actively exploited against the customer's deployment | 4 business hours | **1 hour, 24/7/365** |
| **Sev2 — major degradation** | A standard-level doctor check fails on > 1 server, a backup fails, a deploy is wedged, the audit log is not appending | 1 business day | **4 hours, business hours** |
| **Sev3 — minor / question** | A feature request, a "how do I...", a documentation gap, a non-blocking bug | 3 business days | **1 business day** |
| **Sev4 — sales / contract** | A pricing question, a contract redline, a procurement request | 5 business days | **1 business day** |

The Sev definitions match `operations-runbook.md` §3.1-3.10 (the 10 alerts).

### 2.2 Resolution time SLA (the time to incident closure, Enterprise only)

| Severity | Enterprise target | Enterprise contractual max |
|---|---|---|
| **Sev1** | 4 hours | 8 hours (credit: 1 day of fee per hour over) |
| **Sev2** | 1 business day | 3 business days (credit: 1 day of fee per day over) |
| **Sev3** | 5 business days | 10 business days (credit: 1 day of fee per day over) |

Pro does not commit to resolution time — only to response time. The rationale: Pro is a "best-effort, business-hours" support tier; the customer runs the box and can roll back via the F10 self-update. Enterprise is a "we own the incident" tier, with a 24/7 on-call rotation.

### 2.3 Uptime SLA (Enterprise only)

| Service | Uptime target | Measurement |
|---|---|---|
| **The Enterprise release pipeline** (the signed binary delivery at `releases.sovereignruntime.dev/enterprise/`) | 99.99% (52.6 min/year max downtime) | External synthetic monitor from 3 EU regions (Frankfurt, Dublin, Stockholm); the public status page (H23) reports the live status |
| **The Pro update channel** | 99.9% (8.77 h/year max) | Same |
| **The customer apps** | None | The customer runs them; we do not own their uptime |

### 2.4 The "what if we miss the SLA?" credits

If the Enterprise SLA is missed, the customer's next invoice is credited proportionally:

- Sev1 resolution: 1 day of annual fee per hour over the 8-hour max.
- Sev2 resolution: 1 day of annual fee per day over the 3-day max.
- Uptime < 99.99% on the release pipeline: 5% of annual fee per 0.1% below target.

The credits are capped at 100% of the annual fee (i.e. the customer does not get cash back, just fee credit). The credits are not the customer's exclusive remedy — the customer can still terminate the contract for material breach if the SLA is missed repeatedly (3+ Sev1s in a 12-month period, or any single Sev1 unaddressed for > 24h).

---

## 3. The framework mappings (SOC 2, ISO 27001, BSI C5, EUCS, GDPR)

The persona-cto.md §3.10 ("Compliance — Start in month 12. Year 1 is product-market fit. Year 2 is 'we have enough customers to justify the audit cost' + 'we have a security engineer.' Year 3 is the audit. Year 4 is the certificate.") is the timeline. The actual mapping work starts in V1.5 (the I11 BSI C5 and I12 EUCS features); the SOC 2 and ISO 27001 audits are year 3-4. The current doc is the *plan* for the mapping; the mapping itself is published as a separate document per framework (`/docs/compliance/{soc2,iso27001,bsi-c5,eucs}.md`).

### 3.1 SOC 2 Type II (the audit timeline)

**The criteria:** Security, Availability, Confidentiality (the 3 Trust Services Criteria we need; Processing Integrity and Privacy are out of scope for V1+; we may add them in V2+ based on customer demand).

**The audit cost:** €200k-500k (mostly auditor fees + consultant time + remediation). The persona-cto.md §3.10 says the first 1-2 EU public-sector customers pay for this; we agree.

**The audit timeline:**

| Phase | Duration | Activity |
|---|---|---|
| **Gap analysis** | 3-6 months | Hire a SOC 2 consultant; review the 3 TSCs; identify gaps; remediate |
| **Remediation** | 3-6 months | Implement missing controls (the V1.5 + V2 features cover most of them: audit log immutability G23, log shipping G22, OIDC + MFA G21, OPA/Rego I4, the security incident response runbook in `operations-runbook.md`) |
| **Type I observation period** | 1 day | The auditor verifies the control design at a point in time |
| **Type II observation period** | 6-12 months | The auditor verifies the control operating effectiveness over time |
| **Audit report** | 2-3 months | The auditor writes the report; the customer-facing "SOC 2 Type II report" is delivered |

**Total time from "hire the consultant" to "Type II report in hand":** 18-30 months. Year 3 is the consultant + gap analysis + remediation; year 4 is the observation + report.

**The first SOC 2 report is targeted for month 30 (i.e. year 3, end of V2).** Until then, the I17 `sovereign compliance scan` provides a *self-attested* control map (the binary reports which TSCs are met and which are not). The auditor accepts the self-attestation as a starting point but not as a substitute for the audit.

**The relevant TSC controls** (the full mapping is in `/docs/compliance/soc2.md` once the audit is complete):

| TSC | Control | Mapped to |
|---|---|---|
| **CC1.1** | COSO principle 1: integrity and ethical values | `LICENSE` (Apache 2.0, no carve-outs); the `code-of-conduct.md`; the `decision-records.md` (public decision log) |
| **CC2.1** | Information and communication | The `release-notes.md` per version; the `security-advisories.md`; the GHSA feed; the `sovereign status` (H23) |
| **CC3.1** | Risk assessment | The `negative-prompt.md`; the `persona-cto.md` §6 ("Risks and how we mitigate"); the quarterly risk review in the Enterprise QBR |
| **CC5.1** | Control activities | The 14 doctor categories; the `sovereign audit verify` (G23); the OPA/Rego policy packs (I4) |
| **CC6.1** | Logical and physical access | G21 (OIDC + MFA + RBAC); H3 (RBAC); F7 (encrypted secret store); the append-only audit VFS (G23) |
| **CC6.6** | Logical access controls | The local admin fallback (G21); the `enforce_mfa` option; the `sovereign auth status` audit row |
| **CC6.7** | Restriction of access to information | RBAC; the OIDC `role_map`; the audit log redaction (G22); the `--explain` doctor output (G18) |
| **CC6.8** | Prevention of unauthorized access | The age master key (F7); the SO_REUSEPORT connection drain (F4); the cosign signature on the binary (G14); the SSH hardening doctor check (G16) |
| **CC7.1** | System operations | The doctor (F9 + G16 + H16 + I16); the morning report (G11); the 5 morning commands (operations-runbook §1) |
| **CC7.2** | Monitoring of system components | G22 (log shipping to SIEM); G23 (audit log immutability); the Prometheus `/metrics` (G8) |
| **CC7.3** | Incident response | `operations-runbook.md` §1.1 (the 3am playbook); §3 (the 10 alerts); §6 (the incident toolkit); the 10 alerts map to doctor checks (§12) |
| **CC7.4** | Incident recovery | `operations-runbook.md` §5 (DR); the `sovereign recover` runbook; the vendor-disappear test (the founding promise) |
| **CC8.1** | Change management | The version control; the `cargo-deny` + `cargo-audit` in CI; the OPA/Rego policy packs (I4); the deploy receipt (`sovereign.lock`, F5) |
| **CC9.1** | Risk mitigation | The business continuity plan; the `negative-prompt.md`; the BSI C5 mapping (I11) |
| **A1.1** | Availability | The H14 V1.5 hardening; the H13 EU incorporation; the V2 rqlite HA (I1) |
| **A1.2** | Environmental protections | The append-only audit (G23); the backup-verify cadence (G12); the DR runbook (§5) |
| **A1.3** | Recovery testing | The vendor-disappear test (CI gate); the DR drill schedule (§5.4) |
| **C1.1** | Confidential information | The age encryption (F7); the audit log redaction (G22); the `--explain` doctor output (G18) |
| **C1.2** | Disposal of confidential information | `sovereign uninstall --purge-data`; the `sovereign export` (I2) |

### 3.2 ISO 27001:2022 (the audit timeline)

**The standard:** ISO/IEC 27001:2022, with the Annex A controls.

**The audit cost:** €150k-400k. Less than SOC 2 because the auditor is a single certification body (not a Big-4 firm) and the standard is more prescriptive.

**The audit timeline:** 12-18 months from "hire the consultant" to "certificate in hand." The Annex A control mapping is in `/docs/compliance/iso27001.md` (planned for V2, month 12).

**The relevant Annex A controls** (the 4-theme structure of ISO 27001:2022):

| Theme | Control | Mapped to |
|---|---|---|
| **A.5 Organizational** | A.5.1 Information security policies | `code-of-conduct.md`; the `negative-prompt.md`; the `decision-records.md` |
| **A.5 Organizational** | A.5.2 Information security roles | The on-call rotation (operations-runbook §1.1); the Enterprise CSM role; the support tier ownership |
| **A.5 Organizational** | A.5.7 Threat intelligence | The `persona-cto.md` §3.2; the GHSA feed; the `security@` mailbox; the `negative-prompt.md` |
| **A.5 Organizational** | A.5.10 Acceptable use | The Apache 2.0 license; the EULA template (Enterprise); the operator's AUP (the customer's responsibility) |
| **A.5 Organizational** | A.5.12 Classification of information | The data residency commitment (EU-only by default); the `sovereign audit export` Parquet file classification |
| **A.5 Organizational** | A.5.23 Information security for use of cloud services | The Cloud-only-EU data localization; the `sovereign log ship` config (G22) — the operator picks the EU region for the SIEM |
| **A.5 Organizational** | A.5.30 ICT readiness for business continuity | The `operations-runbook.md` §5 (DR); the `sovereign recover` runbook; the vendor-disappear test |
| **A.6 People** | A.6.1 Screening | The hiring process (V2+, when we have > 5 employees) |
| **A.6 People** | A.6.3 Information security awareness | The docs site (G14); the Show HN post (G15); the QBR for Enterprise |
| **A.6 People** | A.6.5 Responsibilities after termination | The access revocation (G21, RBAC); the audit log retains the user.sub forever |
| **A.6 People** | A.6.8 Confidentiality / non-disclosure | The EULA + the support contract (Enterprise) |
| **A.7 Physical** | A.7.1 Physical perimeters | The customer's data center (the binary runs in the customer's network; the sovereign company has no data center) |
| **A.7 Physical** | A.7.4 Physical security monitoring | The customer's data center; the EU-only data localization |
| **A.8 Technological** | A.8.2 Privileged access rights | G21 (OIDC + MFA + RBAC); the local admin fallback |
| **A.8 Technological** | A.8.3 Information access restriction | RBAC; the role_map; the OIDC groups |
| **A.8 Technological** | A.8.5 Secure authentication | OIDC + MFA; the `enforce_mfa` option |
| **A.8 Technological** | A.8.7 Protection against malware | The doctor checks (`malware_signatures`); the `cargo-audit` in CI; the cosign signature on the binary |
| **A.8 Technological** | A.8.8 Management of technical vulnerabilities | The `cargo audit` cron; the `cargo deny` CI gate; the GHSA feed; the 24-hour embargo for Pro, 30-day for Enterprise |
| **A.8 Technological** | A.8.9 Configuration management | The OPA/Rego policy packs (I4); the `sovereign policy check`; the `sovereign apply` declarative mode (H9) |
| **A.8 Technological** | A.8.12 Data leakage prevention | The audit log redaction (G22); the secret store (F7); the pre-commit hook (F7) |
| **A.8 Technological** | A.8.15 Logging | G22 (log shipping); G23 (audit log immutability); the JSON structured logs |
| **A.8 Technological** | A.8.16 Monitoring activities | G22 (SIEM); the doctor (G16, H16, I16); the morning report (G11) |
| **A.8 Technological** | A.8.20 Network security | The Caddy auto-TLS (F6); the mTLS for agent-server (H2); the SSH hardening doctor check |
| **A.8 Technological** | A.8.21 Security of network services | The Caddy config validation (doctor); the egress allowlist (G16); the Hetzner Cloud Firewall (recommended) |
| **A.8 Technological** | A.8.23 Web filtering | N/A (the binary does not browse the web; the operator configures the egress allowlist) |
| **A.8 Technological** | A.8.25 Secure development life cycle | The `cargo-deny` + `cargo-audit` + `clippy` in CI; the `cargo-fuzz`; the `cargo-mutants`; the F9 doctor in CI |
| **A.8 Technological** | A.8.27 Secure system architecture | `architecture.md` (hexagonal, ports, data model); the threat model in `architecture.md` §6 |
| **A.8 Technological** | A.8.28 Secure coding | The clippy lints; the `trybuild` compile-fail tests; the `proptest` for state machines |
| **A.8 Technological** | A.8.29 Security testing in development and acceptance | The CI matrix; the `cargo-fuzz`; the pen-test plan (§4) |
| **A.8 Technological** | A.8.30 Outsourced development | The Apache 2.0 license; the public code review; the SBOM; the cosign signature; the reproducibility build |
| **A.8 Technological** | A.8.31 Separation of development, test and production | The `env: dev / staging / prod` in `app.yaml` (G5); the preview environments (H6) |
| **A.8 Technological** | A.8.32 Change management | The CC8.1 mapping above (SOC 2); the OPA/Rego packs |
| **A.8 Technological** | A.8.33 Test information | The fixtures (`tests/fixtures/`); the anonymized production data is never used in tests |
| **A.8 Technological** | A.8.34 Protection of information systems during audit testing | The audit log is read-only; the auditor uses `sovereign audit export` (G23) |
| **A.5 Organizational** (new) | A.5.31 Legal, statutory, regulatory and contractual requirements | The Apache 2.0 license; the EULA (Enterprise); the DPA (§5); the sub-processor list |
| **A.5 Organizational** (new) | A.5.34 Privacy and protection of personal information | GDPR compliance (§3.5); the data minimization (G22 + G23) |
| **A.5 Organizational** (new) | A.5.35 Independent review of information security | The pen-test (§4); the auditor's SOC 2 report |

### 3.3 BSI C5:2026 (the German national standard)

This is the **first** certification we target (I11 in `phase-03-v2.md`). C5:2026 is the German Federal Office for Information Security (BSI) cloud computing compliance criteria catalogue, and it explicitly aligns with EUCS Substantial.

**The 17 C5:2026 control areas** (the full mapping is in I11):

| C5 area | Mapped to |
|---|---|
| **OIS — Organisation of Information Security** | The `code-of-conduct.md`; the on-call rotation; the support tier ownership |
| **SPO — Security Policies** | The `negative-prompt.md`; the `decision-records.md`; the QBR |
| **PSO — Personal Security** | The hiring process; the NDA; the offboarding checklist |
| **INF — Infrastructure** | The customer's data center; the EU-only data localization |
| **COM — Communication** | The TLS configuration (Caddy); the mTLS for agents (H2) |
| **OPS — Operations** | The doctor; the morning report; the 3am playbook |
| **DEV — Development** | The secure SDLC; the CI matrix; the fuzzing; the pen-test |
| **SAC — Service Availability** | The DR plan; the vendor-disappear test; the rqlite HA (V2) |
| **CRA — Customer Relationship** | The EULA; the DPA; the QBR; the sub-processor list |
| **MAO — Maintenance** | The `sovereign update` (F10); the 30-day embargo (Enterprise) |
| **CRY — Cryptography** | The age master key (F7); the TLS 1.3 (Caddy); the Ed25519 audit chain (G23) |
| **Identity & Access Management** | G21 (OIDC + MFA + RBAC); the local admin fallback |
| **Logging & Monitoring** | G22 (log shipping); G23 (audit log immutability) |
| **Incident Handling** | The 3am playbook; the 10 alerts; the incident toolkit |
| **Business Continuity** | The DR plan; the rqlite HA |
| **Compliance** | The SOC 2 mapping; the ISO 27001 mapping; the GDPR mapping |
| **Risk Management** | The `negative-prompt.md`; the quarterly risk review |

The C5:2026 mapping document is published at `/docs/compliance/bsi-c5.md` in V1.5. The actual C5 certification is targeted for V2, month 18 (year 2 end).

### 3.4 EUCS Substantial (the European Cybersecurity Scheme for Cloud Services)

This is the **second** certification we target (I12 in `phase-03-v2.md`). EUCS is the EU's own cloud certification scheme, and "Substantial" is the level that maps to BSI C5:2026 (the German national scheme).

**The EUCS Substantial control areas** (the full mapping is in I12):

- **CYP — Cryptography and key management** — age + Ed25519 + TLS 1.3
- **IAM — Identity and access management** — G21 (OIDC + MFA + RBAC)
- **PRV — Data protection and privacy** — GDPR (§3.5); the data minimization
- **LOG — Logging and monitoring** — G22 + G23
- **BCP — Business continuity** — the DR plan + the vendor-disappear test
- **INC — Incident handling** — the 3am playbook + the 10 alerts
- **CFG — Configuration management** — OPA/Rego (I4) + the `sovereign apply` (H9)
- **VUL — Vulnerability management** — `cargo audit` + the GHSA feed + the embargo
- **NET — Network security** — Caddy + mTLS + the egress allowlist
- **SUP — Supply chain** — the SBOM + the cosign signature + the reproducibility build
- **LEG — Legal and regulatory** — the EULA + the DPA + the sub-processor list

The EUCS Substantial certification is targeted for V2, month 24 (year 2 end + 6 months buffer for the auditor).

### 3.5 GDPR (the data protection regulation)

**The legal basis:** The binary is a deployment runtime. The operator's *data* is the operator's responsibility. The binary's *telemetry* is opt-in (G22) and default-off (product-ux.md §8.3). The audit log is the operator's data; the binary writes it to the operator's disk.

**The data the binary does NOT collect** (the negative list, important for GDPR):

- No PII (no email, no IP, no hostname — only counts)
- No content of secrets
- No deploy content (no image refs, no commit SHAs, no diffs)
- No app names (only counts: `user has 3 apps`, not `user has api, web, worker`)
- No audit log content (only counts)

**The data the binary DOES collect** (the positive list, opt-in only):

- Anonymous, aggregated counters (deploys per day, rollback rate, mean time to detect, mean time to recover)
- The TUI session length, the CLI invocations per day
- The docs page → first deploy conversion funnel
- The 30-day churn

**The DPA template** (the Data Processing Addendum that the Enterprise customer signs) is in §5. The DPA is a signed contract; it lists the sub-processors (only the EU-only release pipeline and the EU-only SIEM — there are no US sub-processors for an EU customer by default).

**The sub-processor list** (the third parties the binary contacts):

| Sub-processor | Purpose | Location | GDPR basis |
|---|---|---|---|
| **`releases.sovereignruntime.dev`** (Fastly + S3-compatible backup) | Binary + SBOM + cosign signature delivery | EU-only (Frankfurt + Stockholm POPs) | Article 28 (processor); the customer does not send PII to this endpoint |
| **`sovereignruntime.dev`** (static site) | Website + docs | EU-only | The operator's browser fetches the install script; no PII is sent |
| **`ghcr.io`** (GitHub Container Registry) | Container image distribution | US (GitHub Inc.) | Article 28; the container image is publicly auditable; the customer can mirror to a private registry (V1+) |
| **(optional) `loki.<customer-domain>`** | Log shipping destination | Customer's choice | Article 28; the customer configures the destination |
| **(optional) `http-intake.logs.datadoghq.com`** | Log shipping destination (Datadog SaaS) | US (Datadog Inc.) | Article 28 + SCC; the customer opts in explicitly |
| **(optional) `logs.betterstack.com`** | Log shipping destination (Better Stack) | EU (Frankfurt) | Article 28; the customer opts in explicitly |
| **(optional) Splunk HEC** | Log shipping destination | Customer's choice | Article 28; the customer configures |

The default config has **no third-party sub-processors except the binary delivery and the docs site** (both EU-only). The customer opts in to Datadog / Splunk / Better Stack / Loki / etc. via `sovereign log ship` (G22).

**The right to be forgotten** (GDPR Article 17): the customer's audit log is the customer's data. The customer can `sovereign audit export --format parquet` to get the full record, then `rm -rf /var/lib/sovereign/sovereign.db` to delete it. The binary does not retain a copy anywhere outside the customer's box. The Pro / Enterprise support team does not have access to the customer's data (the support team reads audit logs only when the customer explicitly shares a `sovereign audit export` for a ticket, and only for the duration of the ticket).

**The data breach notification** (GDPR Article 33): the customer is the data controller. Sovereign (the company) is the data processor for the sub-processor list above. The customer must notify their supervisory authority within 72 hours of a personal data breach. The Enterprise contract includes a clause that the company will notify the customer of any breach affecting the customer's data within 24 hours of detection (faster than the 72-hour GDPR minimum).

---

## 4. The penetration test plan (the third-party validation)

**The cadence:** Annual (year 2+). The persona-cto.md §3.10 says year 2-3, €30-50K, annual.

**The scope:** The binary (the static musl artifact), the install.sh script, the systemd unit, the default config, the 3 web endpoints (the dashboard, the `/metrics`, the OIDC callback), the 10 CLI subcommands most likely to have injection vulnerabilities (`sovereign deploy`, `sovereign secret exec`, `sovereign import`, `sovereign doctor --fix`, `sovereign policy check`, `sovereign backup`, `sovereign db pool create`, `sovereign certs renew`, `sovereign dns record add`, `sovereign update`).

**The test categories:**

1. **Web application security** (OWASP Top 10 + ASVS Level 2) — the dashboard, the API.
2. **Network security** — the TLS configuration, the Caddy auto-renew, the mTLS for agents.
3. **Cryptographic implementation** — the age master key, the Ed25519 chain (G23), the TLS cipher suites.
4. **Authentication and authorization** — the OIDC flow (G21), the RBAC enforcement (H3), the local admin fallback.
5. **Input validation** — the YAML parser (for `app.yaml`), the SQL builder (the sqlx queries), the shell exec path (`sovereign secret exec`).
6. **Supply chain** — the SBOM completeness, the cosign signature verification, the reproducibility build.
7. **Privilege escalation** — the systemd unit's `User=sovereign`, the file mode bits on `/var/lib/sovereign`, the append-only audit VFS (G23).
8. **Information disclosure** — the log redaction (G22), the `sovereign doctor --explain` output (G18), the audit log redaction.
9. **Business logic** — the deploy race conditions (F4 connection drain), the rollback ordering (F5), the OPA/Rego policy enforcement (I4).
10. **Operational security** — the backup-verify cadence (G12), the DR runbook (§5), the incident response (operations-runbook §1.1).

**The output:** A public penetration test summary (the full report is NDA'd with the customer; the summary is published at `/docs/security/pen-test-<year>.md`). The summary lists:

- The total number of findings
- The number of Critical / High / Medium / Low findings
- The status of each finding (Open / In Remediation / Resolved)
- The average time to remediation (the SLA: 30 days for Critical, 90 days for High, 180 days for Medium)
- The auditor's opinion (Pass / Pass with notes / Fail)

**The auditor selection:** A CREST-accredited or CHECK-accredited firm, EU-based, with cloud native + Rust experience. Candidates include Cure53 (Berlin), Trail of Bits (NY — but with an EU office), SEC Consult (Vienna), Nixdorf (Hamburg). The auditor is selected by the customer for an Enterprise customer-specific test; the annual general test is selected by the company.

**The budget:** €30-50K per year. The first test is in V1.5 (month 15); the test report is the artefact the Enterprise customer signs off on.

---

## 5. The Data Processing Addendum (DPA) template

The DPA is a signed contract, not a public document. The template is below (the actual contract is in the Enterprise tier's `MSA + DPA bundle`).

### 5.1 The DPA template (the 12 clauses)

```
DATA PROCESSING ADDENDUM (DPA)

This DPA is entered into between [CUSTOMER NAME] ("Controller") and
Sovereign Application Runtime GmbH ("Processor") and supplements the
Master Service Agreement (MSA) dated [DATE].

1. Subject matter and duration
   The Processor processes personal data on behalf of the Controller
   for the purpose of providing the Sovereign Application Runtime
   software and support services. The duration is the term of the MSA.

2. Nature and purpose of processing
   The Processor operates the binary delivery infrastructure
   (releases.sovereignruntime.dev), the documentation site
   (sovereignruntime.dev), and the Enterprise support channel
   (support.sovereignruntime.dev). The Processor does NOT access the
   Controller's data, applications, secrets, or audit logs except when
   the Controller explicitly shares an export for a support ticket.

3. Categories of personal data
   - Controller's end users' email addresses (if the Controller
     enables OIDC SSO with an IdP that returns email claims)
   - Controller's support contact names and email addresses
   - Aggregated, anonymous usage counters (deploys per day, etc.) —
     opt-in only, default off

4. Categories of data subjects
   - Controller's employees and contractors (end users of the binary)
   - Controller's support contacts

5. Controller's obligations
   - The Controller is the data controller for the Controller's
     application data; the Processor is NOT a processor for that data
   - The Controller must have a legal basis for processing under
     Article 6 GDPR
   - The Controller must inform their data subjects of the processing

6. Processor's obligations
   - Process personal data only on documented instructions from the
     Controller (the MSA + this DPA)
   - Ensure that persons authorized to process the personal data have
     committed themselves to confidentiality
   - Implement appropriate technical and organizational measures
     (the SOC 2 / ISO 27001 controls; the append-only audit log; the
     age encryption; the OIDC + MFA)
   - Engage sub-processors only with the Controller's prior specific
     or general written authorization
   - Assist the Controller in fulfilling their data subject rights
     (access, rectification, erasure, restriction, portability)
   - Assist the Controller in ensuring compliance with Articles 32-36
     GDPR (security, breach notification, DPIA)
   - At the Controller's choice, delete or return all personal data
     after the end of the provision of services
   - Make available to the Controller all information necessary to
     demonstrate compliance with Article 28 GDPR
   - Notify the Controller of any personal data breach within 24 hours
     of detection (faster than the GDPR Article 33 minimum of 72 hours)

7. Sub-processors
   The Processor engages the following sub-processors:
   - Fastly Inc. (binary delivery, EU-only POPs)
   - S3-compatible storage (binary backup, EU-only)
   - GitHub Inc. (container registry, US)
   - [Customer-configured log ship destination, e.g. Datadog / Better
    Stack / Splunk / Loki]

   The Processor will notify the Controller of any changes to the
   sub-processor list at least 30 days in advance, giving the
   Controller the opportunity to object.

8. International data transfers
   The Processor ensures that any transfer of personal data outside
   the EU/EEA is subject to appropriate safeguards (Standard
   Contractual Clauses, adequacy decisions, or the EU-US Data
   Privacy Framework). The default configuration has NO
   non-EU sub-processors.

9. Security measures
   The Processor implements the technical and organizational measures
   listed in Annex A of this DPA, which include:
   - TLS 1.3 for all data in transit
   - AES-256-GCM (via the age library) for all data at rest
   - Ed25519 signatures for the audit log chain
   - OIDC + MFA for all administrative access
   - Append-only audit log for all data access events
   - Annual third-party penetration test
   - SOC 2 Type II report (from year 4) and ISO 27001 certificate
     (from year 4)

10. Data subject rights
    The Processor will assist the Controller in responding to data
    subject requests (Articles 15-22 GDPR) within 5 business days of
    the Controller's request. The Processor will not respond directly
    to data subjects unless instructed by the Controller.

11. Personal data breach
    The Processor will notify the Controller of any personal data
    breach affecting the Controller's data within 24 hours of
    detection. The notification will include:
    - The nature of the breach
    - The categories and approximate number of data subjects affected
    - The likely consequences
    - The measures taken or proposed to address the breach

12. Governing law and jurisdiction
    This DPA is governed by the laws of [Germany / France / Ireland —
    the customer's choice]. The courts of [Berlin / Paris / Dublin]
    have exclusive jurisdiction.

Annex A: Technical and Organizational Measures
[See the security measures in §3.1 (SOC 2), §3.2 (ISO 27001),
§3.3 (BSI C5), §3.4 (EUCS), and the pen-test plan in §4]

Annex B: Sub-Processor List
[See the sub-processor list in §3.5]
```

### 5.2 The sub-processor list (the controlled document)

The sub-processor list is a separate document, updated at least 30 days before any change. The customer is notified by email and has a 30-day objection window. The current list is at `/docs/legal/sub-processors.md`.

---

## 6. The enterprise onboarding playbook (the first 30-60-90 days)

The first 30 days of an Enterprise customer relationship. The playbook is published in the Enterprise tier's customer portal; the summary is below.

### 6.1 The first 30 days (procurement + technical setup)

**Week 1-2: Contract + procurement**

- The customer signs the MSA + the DPA + the support tier addendum.
- The customer receives the Enterprise license key (a JSON file at `/etc/sovereign/license.json` with the channel, the support tier, the embargo window).
- The customer's procurement office receives the BSI C5 mapping (I11), the EUCS Substantial checklist (I12), the SOC 2 self-attestation (until the Type II report is available), the pen-test summary (year 2+), the sub-processor list, the SLA matrix (§2).
- The customer receives the dedicated EU support engineer's contact (email, Slack Connect, phone for Sev1).

**Week 2-3: Technical setup**

- The customer's infrastructure team runs `curl -sSf sovereignruntime.dev/install.sh | sh` on a test box (Hetzner CX22 / OVH VPS / on-prem VM).
- The customer's team configures the OIDC SSO (G21) with their IdP (Authentik / Keycloak / Okta / Entra ID / Google).
- The customer's team configures the log shipping (G22) to their SIEM (Loki / Datadog / Splunk / Better Stack).
- The customer's team runs `sovereign import --from heroku` (or Coolify / Dokploy / Render) for the first 1-2 apps (the design-partner migration).
- The customer's team runs `sovereign doctor --level standard` and shares the output with the support engineer (sanitized — no PII, no secret values, no hostnames).

**Week 3-4: Production rollout**

- The customer's team rolls out the binary to the production fleet (the first 5-10 servers).
- The customer's team configures the audit log export (G23) to the customer's S3 bucket or SIEM.
- The customer's team runs the first DR drill (the vendor-disappear test, §5.3 of operations-runbook).
- The support engineer schedules a weekly 30-minute sync (the QBR is quarterly; the weekly sync is for the first 3 months).

### 6.2 The 60-day milestone (the first quarterly business review)

The QBR covers:

- The SLA performance (the response time SLA, the resolution time SLA, the uptime SLA)
- The open support tickets (the queue, the resolution rate, the CSAT)
- The customer's usage (the number of servers, the number of apps, the audit log volume, the backup volume)
- The customer's planned growth (the next 12 months, the procurement pipeline)
- The security advisories (the embargoed advisories, the customer's patch cadence)
- The roadmap (the customer's input on the V1.5 / V2 features)

The QBR is recorded (with the customer's consent) and the minutes are shared within 5 business days.

### 6.3 The 90-day milestone (the contract renewal check)

- The customer confirms satisfaction (a 1-page survey, 10 questions, ≤ 10 minutes to complete)
- The renewal terms are confirmed (the annual contract auto-renews unless either party gives 90 days' notice)
- The expansion opportunity is identified (the customer may want more servers, more support, more certifications)

### 6.4 The SLA reporting (the monthly report)

The customer receives a monthly report (the first business day of the month) covering:

- The SLA performance for the previous month (the response time histogram, the resolution time histogram, the uptime histogram)
- The security advisories issued in the previous month (the GHSA list, with the embargoed-vs-public split)
- The audit log volume (the number of rows, the export size)
- The backup volume (the number of dumps, the storage cost)
- The doctor's findings trend (the count of fails over time)

The monthly report is the artefact the customer's CISO reads.

---

## 7. The "what we explicitly do NOT promise" list

This is the negative list. It is the antidote to the marketing-driven "we are enterprise-ready!" claims. The persona-cto.md §6.2 ("What we are not") is the source.

- **We do NOT host your apps.** The Pro and Enterprise tiers do not include managed hosting. The customer runs the binary on their own infrastructure (Hetzner / OVH / on-prem / AWS / GCP / Azure). The Enterprise SLA is on the release pipeline and the support response, not on the customer's app uptime.
- **We do NOT own your data.** The customer owns the SQLite database, the audit log, the backups, the secrets, the app data. Sovereign (the company) does not have access. The support team can read the audit log only if the customer shares an export.
- **We do NOT have a US sub-processor by default.** The default config (binary delivery, docs site) is EU-only. The customer opts in to Datadog / Splunk / Better Stack / etc. via `sovereign log ship` (G22).
- **We do NOT commit to a 99.99% uptime on the customer's apps.** The 99.99% uptime SLA is on *our* release pipeline (the signed binary delivery), not on the customer's fleet. The customer's fleet uptime is the customer's responsibility (this is the value of the doctor: it tells the operator *before* the outage, not after).
- **We do NOT have a SOC 2 Type II report in year 1.** The first report is targeted for year 3-4 (per the timeline in §3.1). Until then, the customer receives the SOC 2 self-attestation (the I17 `sovereign compliance scan` output) and the C5 mapping (I11, V1.5).
- **We do NOT have a FedRAMP authorization.** The FedRAMP process is US-specific; we target EUCS Substantial + BSI C5:2026, which are the EU equivalents. A US public-sector customer would need to sponsor our FedRAMP authorization (a 12-18 month process, €300K+).
- **We do NOT support Windows as a deployment target.** The binary is Linux-only (with macOS for dev, FreeBSD V1+). Windows containers are out of scope.
- **We do NOT support IBM Z / s390x.** The target list is x86_64 + aarch64. A customer on s390x would need to compile from source (the source is available, the cross-compilation is not in the official release pipeline).
- **We do NOT commit to a "24/7 phone support" tier for Pro.** Phone support is Enterprise-only. Pro has email support during business hours (Mon-Fri, 09:00-18:00 CET). The 24/7 phone is Enterprise.
- **We do NOT take custody of your secrets.** The age master key never leaves the box. The support team cannot decrypt your secrets. The backup-verify restore drill runs *on your hardware*, not ours.
- **We do NOT offer a "white-label" or "OEM" tier.** The binary is the binary, the name is the name, the license is Apache 2.0. There is no "Powered by Sovereign" removal option.
- **We do NOT commit to a feature roadmap timeline for individual customers.** The roadmap is published at `/docs/roadmap.md` and is the same for all customers. The Enterprise QBR is the input mechanism; the customer can influence priorities, not the timeline.
- **We do NOT have an EU AI Act compliance package.** The binary does not use AI / ML in the runtime path. The I5 ML scorer (anomaly detection) is shadow mode in V2 and opt-in. A customer who needs EU AI Act compliance for the ML scorer can use the I17 `sovereign compliance scan` to map the controls, but the ML scorer is not the product.
- **We do NOT promise "zero CVEs forever."** CVEs happen. The 24-hour (Pro) / 30-day (Enterprise) embargo is the commitment. The `cargo audit` cron + the GHSA feed + the pen-test plan (§4) are the controls.

---

## 8. The "where to go next" reading list

- **The product:** [`README.md`](./README.md) — the docs index.
- **The architecture:** [`architecture.md`](./architecture.md) — the hexagonal layout, the data model, the state machines, the failure modes.
- **The technology:** [`tech-stack.md`](./tech-stack.md) — the locked crate table, the release profile, the footprint claim, the static linking contract, the packaging.
- **The phases:** [`phase-00-mvp.md`](./phase-00-mvp.md), [`phase-01-v1.md`](./phase-01-v1.md), [`phase-02-v15.md`](./phase-02-v15.md), [`phase-03-v2.md`](./phase-03-v2.md) — the 72 features in build order.
- **The doctor:** [`doctor.md`](./doctor.md) — the diagnostic engine, the 5 levels, the 14 categories, the 4 cross-cutting checks, the incident toolkit.
- **The operations:** [`operations-runbook.md`](./operations-runbook.md) — the 5 morning commands, the 3am playbook, the 10 alerts, the backup strategy, the DR runbook, the doctor↔alert loop.
- **The product UX:** [`product-ux.md`](./product-ux.md) — the 5-minute moment, the 8 CLI non-negotiables, the 4 TUI non-negotiables, the framework scanner, the pricing, the telemetry, the end-user install/upgrade/uninstall story.
- **The sovereignty:** [`sovereignty-and-governance.md`](./sovereignty-and-governance.md) — the 10-point sovereignty test, the EU incorporation, the OPA/Rego, the ML scorer.
- **The decisions:** [`decision-records.md`](./decision-records.md) — the 15 ADRs.
- **The anti-patterns:** [`negative-prompt.md`](./negative-prompt.md) — the 5 anti-patterns, the 8 case studies, the 14 refusals, the feature-creep checklist.

---

**Next: read [`sovereignty-and-governance.md`](./sovereignty-and-governance.md) for the 10-point sovereignty test, the OPA/Rego policy packs (I4), and the ML scorer (I5).**
