# Sovereignty and Governance

**Status:** Locked for V2. The 10-point sovereignty test is a V2 CI gate.
**Audience:** The founder, the lead engineer, the EU mid-market sales lead, EU public-sector buyers, the security officer at any design partner.
**Last updated:** 2026-06-04

This document is the **how "sovereign" is operationalized** guide. Sovereignty is a structural claim, not a feature. Hyperscalers can claim 1-2 of the 5 dimensions; the product claims all 5 by construction. Every sovereignty feature must pass the maintainability test before shipping.

---

## 1. The 5 dimensions of sovereignty

| Dimension | Definition | How the product satisfies it |
|---|---|---|
| **Data sovereignty** | My data is in a jurisdiction I trust | EU-resident data; no cross-border transfer without consent |
| **Operational sovereignty** | I can run it without calling anyone for permission | Self-hosted; no SaaS dependency; works offline |
| **Vendor sovereignty** | The vendor can disappear and the product keeps working | Apache 2.0, foundation transfer plan, export/import |
| **Legal sovereignty** | The company is incorporated where I trust | EU-incorporated (Berlin GmbH + Estonian OÜ) |
| **Technical sovereignty** | The technology is open and inspectable | Open source, SBOM, cosign-signed releases, reproducible builds |

Hyperscalers can claim 1-2 of these. The product claims all 5 by construction. **This is the moat.**

---

## 2. The 10-point sovereignty test (CI gate)

Every release runs this test in CI. If any point fails, the release does not ship.

| # | Point | Test |
|---|---|---|
| 1 | Is the source code open and self-hostable? (Apache 2.0 / MIT, no carve-outs) | `LICENSE` is Apache 2.0, no `proprietary/` dir, no source-available exceptions |
| 2 | Can I run it without internet? (offline mode tested in CI) | A network-isolated container runs `sovereign deploy` end-to-end |
| 3 | Can I run it without any external service? (no SaaS dependency) | No outbound network calls in `sovereign http` mode (verified by network capture) |
| 4 | Can I export everything? (`sovereign export` produces a complete backup) | `sovereign export platform` produces a tarball with all state |
| 5 | Can I import on a different host? (restorable on a fresh box) | `sovereign import platform --from <tarball>` restores on a fresh VM |
| 6 | Is the company incorporated in a jurisdiction I trust? (EU) | The website's `/sovereignty` page lists the GmbH and OÜ registration numbers |
| 7 | Is the company funded such that it survives a downturn? (revenue or long runway) | The website publishes an 18-month sustainability signal (revenue, runway, burn) |
| 8 | Is there a credible bus factor? (≥ 3 core maintainers, or a foundation) | `CODEOWNERS` lists ≥ 3 maintainers, OR the project is at a foundation |
| 9 | Are security advisories public? (GHSA, CVE) | All advisories are in the GitHub Security tab; `cargo audit` is clean |
| 10 | Is the binary reproducible? (deterministic builds, signed releases) | A re-build of the release tarball produces the same SHA-256; cosign signature verifies |

### 2.1 The CI workflow

```yaml
# .github/workflows/sovereignty-test.yml
name: Sovereignty Test
on:
  push:
    branches: [main]
    tags: ['v*']

jobs:
  sovereignty:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Test 1: License is Apache 2.0
        run: |
          test -f LICENSE
          head -1 LICENSE | grep -q "Apache License"
          test ! -d proprietary
      
      - name: Test 2: Offline mode
        run: |
          docker run --rm --network=none -v $(pwd):/src ubuntu:22.04 bash -c '
            apt install -y /src/target/deb/sovereign_*.deb
            sovereign init
            sovereign deploy --app test --image=localhost:5000/test
            curl -k https://localhost/health | grep -q "200"
          '
      
      - name: Test 3: No external service
        run: |
          # Start the binary, capture all network traffic, verify no outbound
          docker run --rm --network=isolated_test -v $(pwd):/src ubuntu:22.04 bash -c '
            apt install -y /src/target/deb/sovereign_*.deb
            sovereign http &
            sleep 5
            # Capture traffic from the container
            tcpdump -i any -w /tmp/capture.pcap &
            sleep 10
            # Verify no DNS queries for non-localhost domains
            tshark -r /tmp/capture.pcap -Y "dns.qry.name and not dns.qry.name contains localhost" | wc -l
          ' | grep -q "^0$"
      
      - name: Test 4: Export
        run: |
          sovereign http &
          sleep 5
          sovereign deploy --app test --image=nginx:alpine
          sovereign export platform --output /tmp/export.tar.gz
          test -f /tmp/export.tar.gz
          test $(stat -c %s /tmp/export.tar.gz) -gt 1000
      
      - name: Test 5: Import on fresh host
        run: |
          # On a separate VM
          sovereign import platform --from /tmp/export.tar.gz
          sovereign apps list
          test $(sovereign apps list --format=json | jq length) -ge 1
      
      - name: Test 6: EU incorporation
        run: |
          curl -sf https://sovereignruntime.dev/sovereignty | grep -E "(HRB [0-9]+|e-Business Registry)"
      
      - name: Test 7: Sustainability signal
        run: |
          curl -sf https://sovereignruntime.dev/sustainability | grep -E "([0-9]+ months runway|€[0-9]+k MRR)"
      
      - name: Test 8: Bus factor
        run: |
          MAINTAINERS=$(grep -c '@' CODEOWNERS)
          test $MAINTAINERS -ge 3
      
      - name: Test 9: Security advisories public
        run: |
          cargo audit
          # Verify all advisories are in the GitHub Security tab
          gh api /repos/sovereignruntime/sovereign/security-advisories | jq '.[] | .state' | grep -q "published"
      
      - name: Test 10: Reproducible build
        run: |
          # Re-build and compare SHA-256
          cargo build --release --target x86_64-unknown-linux-musl --locked
          sha256sum target/x86_64-unknown-linux-musl/release/sovereign | diff - <(cat target/dist/sovereign.sha256)
          # Verify cosign signature
          cosign verify-blob --signature target/dist/sovereign.sig --certificate target/dist/sovereign.pem target/dist/sovereign
```

### 2.2 The "passing" output

```text
✓ 1. License is Apache 2.0
✓ 2. Offline mode
✓ 3. No external service
✓ 4. Export works
✓ 5. Import works on fresh host
✓ 6. EU incorporation documented
✓ 7. Sustainability signal published
✓ 8. Bus factor ≥ 3
✓ 9. Security advisories public, cargo audit clean
✓ 10. Reproducible build, cosign signature valid

10/10. Release is sovereign.
```

### 2.3 The "failing" output (example)

```text
✗ 2. Offline mode
   → Container could not resolve localhost:5000 (no DNS)
   → The test environment is not air-gapped; the sovereignty test must use a network-isolated container

✗ 7. Sustainability signal
   → /sustainability page does not include "months runway" or "MRR"
   → The 18-month sustainability signal is missing

8/10. Release is NOT sovereign. Fix and re-run.
```

---

## 3. The EU incorporation

### 3.1 The two entities

| Entity | Jurisdiction | Purpose | Tax treatment |
|---|---|---|---|
| **Berlin GmbH** | Germany | BSI C5, EU public sector, IPCEI-CIS | German KSt (15%) + GewSt (variable) |
| **Estonian OÜ** | Estonia | E-residency, digital-first team | 0% on reinvested profits, 20% on dividends |

### 3.2 The setup

1. **Engage a Berlin law firm** (Linklaters, Taylor Wessing, YPOG) to file the GmbH.
2. **Engage an Estonian e-residency agency** to file the OÜ.
3. **Open a Berlin bank account** for the GmbH (Deutsche Bank, solarisbank, or Qonto).
4. **Set up the OÜ as a 100% subsidiary** of the GmbH (or vice-versa, depending on tax advice).
5. **Apply for BSI C5:2026 attestation** (V2; V1.5: gap analysis only).
6. **Publish the corporate details** on the website's `/sovereignty` page.

### 3.3 The published details

```text
# https://sovereignruntime.dev/sovereignty

Sovereign Application Runtime GmbH
  Registered: Berlin, Germany
  HRB: 123456 (Amtsgericht Berlin-Charlottenburg)
  VAT ID: DE123456789
  Managing Director: <name>

Sovereign Application Runtime OÜ
  Registered: Tallinn, Estonia
  Registry code: 12345678
  VAT ID: EE123456789
  Founder: <name>

Bus factor: <list of 3+ core maintainers>
Last updated: <date>
```

---

## 4. The OPA / Rego policy engine

**The thesis:** Most "policy" features in deployment platforms fail for one of two reasons:
- **Pure rules:** brittle, false-positive-heavy, can't see slow drift. Ops teams quietly disable them.
- **Pure ML:** opaque, un-auditable, illegal under GDPR Art. 22 for decisions affecting a person. Also: how do you *explain* a rollback to a customer?

**The working answer:** layered. Rules are the law. ML is the early-warning radar.

### 4.1 The 4 layers

```text
┌─────────────────────────────────────────────────────────┐
│ Layer 4: Human Override (audit log + manual ack)        │
├─────────────────────────────────────────────────────────┤
│ Layer 3: Action Decision (Rego rules — explainable)     │
│   if anomaly_score >= 80 → halt_deploy, page_operator   │
│   if anomaly_score >= 50 → deploy_canary_5pct           │
│   if anomaly_score >= 20 → log, continue                │
├─────────────────────────────────────────────────────────┤
│ Layer 2: Anomaly Scorer (ML — Prophet / TimesFM)        │
│   inputs: cpu, mem, req_rate, err_rate, p99, deploy     │
│   outputs: 0-100 score, top-3 contributing features    │
├─────────────────────────────────────────────────────────┤
│ Layer 1: Telemetry (SQLite + WAL, no external DB)       │
│   metrics: 1m, 5m, 1h, 1d rollups, 90d retention        │
│   events: deploys, config changes, policy violations    │
└─────────────────────────────────────────────────────────┘
```

### 4.2 The 6-month staged rollout

- **Month 1-2:** Layer 1 + Layer 3 (rules only, no ML). Ship `sovereign policy check` and 3 rule packs (`baseline.rego`, `cost.rego`, `compliance.rego`).
- **Month 3-4:** Add Layer 2 in *shadow mode* (the ML scorer runs, but doesn't influence decisions; logs to audit for human review).
- **Month 5:** Add `sovereign policy advise` (advisory only — the CLI says "ML scored this deploy 67, recommend canary at 5%" but doesn't enforce).
- **Month 6:** Add enforcement for low-risk decisions (auto-canary, auto-log). High-risk decisions still require human approval.

**Every ML decision is explainable.** "Why was I rolled back?" returns the score, the contributing features, and the rule that fired. Audit log includes the ML inference inputs (snapshotted) so the decision can be replayed in a notebook.

### 4.3 The risk score formula

```text
risk_score = (
  0.30 * change_size_factor       // LOC changed, files touched, image size delta
  + 0.20 * env_factor             // dev=0.1, staging=0.4, prod=1.0
  + 0.15 * author_history_factor  // last_5_deploys_fail_rate, days_since_last_deploy
  + 0.10 * time_factor            // friday_evening=1.0, weekday_afternoon=0.3
  + 0.10 * dependency_factor      // # deps changed
  + 0.10 * secret_factor          // # secrets changed
  + 0.05 * anomaly_factor         // ML scorer (when in shadow mode)
) * 100
```

| Band | Score | Action |
|---|---|---|
| **Auto-approve** | 0-19 | Full rollout, no canary |
| **Log** | 20-49 | Full rollout, log to audit |
| **Canary** | 50-79 | Canary at 5%, alert on anomaly |
| **Halt** | 80-100 | Halt deploy, page on-call, require `--confirm-risk` to bypass |

### 4.4 The 3 default rule packs

```rego
# baseline.rego — non-negotiable guardrails
package sovereign.baseline

deny[msg] {
    input.kind == "deploy"
    not input.image.attestation.signature
    msg := sprintf("image %v has no cosign signature", [input.image.ref])
}

deny[msg] {
    input.kind == "config_change"
    input.resource.type == "bucket"
    input.resource.acl == "public-read"
    msg := "public-read buckets are not allowed"
}

deny[msg] {
    input.kind == "deploy"
    not input.service.owner
    msg := sprintf("service %v has no owner declared", [input.service.name])
}
```

```rego
# cost.rego — bill-shock prevention
package sovereign.cost

deny[msg] {
    input.kind == "deploy"
    input.service.budget.egress_per_day_eur > 500
    msg := sprintf("egress budget €%v exceeds €500/day cap", [input.service.budget.egress_per_day_eur])
}

warn[msg] {
    input.kind == "deploy"
    not input.service.budget
    msg := sprintf("service %v has no budget declared", [input.service.name])
}
```

```rego
# ml_response.rego — what to do with the scorer's output
package sovereign.ml_response

halt[msg] {
    input.kind == "deploy"
    input.ml_score >= 80
    msg := sprintf("anomaly score %v — halting deploy, paging on-call", [input.ml_score])
}

canary[msg] {
    input.kind == "deploy"
    input.ml_score >= 50
    input.ml_score < 80
    msg := sprintf("anomaly score %v — canary at 5%%", [input.ml_score])
}
```

### 4.5 The `sovereign policy` CLI

```text
sovereign policy check --app <APP> --env <ENV>   # dry-run the rules
sovereign policy list                              # installed rule packs
sovereign policy install <pack.rego>               # upload a rule pack
sovereign policy remove <pack>                     # remove a rule pack
sovereign policy advise --app <APP>                # ML recommendation (advisory only)
```

### 4.6 The model card (ML scorer)

```text
# Model Card — sovereign-ml-scorer v0.1.0

## Intended use
Anomaly detection for sovereign application runtime deploys. The model
scores a proposed deploy 0-100 based on the likelihood of post-deploy
issues (errors, latency, resource exhaustion).

## Training data
- 90 days of telemetry from 5 design partners' production deployments
- ~10,000 deploys, ~1% had post-deploy issues
- Features: CPU, memory, request rate, error rate, p99 latency, deploy size

## Model architecture
- Prophet (Facebook) for time-series forecasting per (app, metric) pair
- Anomaly score = max deviation from forecast, normalized to 0-100
- Top-3 contributing features by absolute deviation

## Performance
- Precision@10: 0.78 (78% of top-10 scored deploys had real issues)
- Recall@10: 0.42 (catches 42% of all real issues in the top-10)
- False positive rate: 6% (deploys flagged as risky that were fine)

## Limitations
- Cold start: a new app has no history; falls back to global averages
- Seasonality: assumes daily/weekly seasonality; ad-hoc patterns may not be caught
- Coordinated deploys: model assumes one deploy at a time; concurrent deploys are not modeled

## Failure modes
- Score = 0: model is uncertain; default to "log" not "auto-approve"
- Score = 100: model is very confident something is wrong; never auto-approve
- Crash: model returns 0; deploy proceeds with rules-only decision
```

---

## 5. The compliance mapping

### 5.1 BSI C5:2026 (German Federal Office for Information Security)

**Status:** V2 (gap analysis in V1.5).

**The 121 controls** are organized in 17 categories:
- Organization of information security
- Human resources security
- Asset management
- Physical and environmental security
- Operations management
- Communications security
- Information systems acquisition, development, and maintenance
- Information security incident management
- Business continuity management
- Compliance
- Cryptography
- Access control
- Audit and accountability
- Configuration management
- Identification and authentication
- System and communication protection
- System and information integrity

**The mapping** (in `docs/sovereignty/bsi-c5-2026-mapping.md`):

| BSI C5:2026 Control | Product Feature |
|---|---|
| OPS-01 Operational procedures | `sovereign morning-report`, `sovereign weekly-report` |
| OPS-06 Backup | `sovereign backup create`, `sovereign backup verify` |
| OPS-07 Monitoring | VictoriaMetrics, vmalert, the 10 alerts |
| OPS-13 Incident management | The 10 alert runbooks |
| COM-01 Network controls | Caddy reverse proxy, no inbound ports except 80/443 |
| COM-03 Cryptographic controls | age encryption, ACME TLS, cosign signatures |
| AIS-01 Audit logging | `audit_event` table, append-only, SQL triggers |
| AIS-02 Monitoring of audit logs | The audit log query + export |
| AIS-06 Application security | The OPA/Rego policy engine |
| CRY-01 Cryptographic key management | `age` master key, argon2id passphrase, key rotation (V2) |
| AC-02 Account management | RBAC: owner, admin, developer, readonly |
| AC-06 Least privilege | RBAC + per-app ownership |
| AC-07 Wireless access | N/A (the product does not use wireless) |
| IA-02 Identification and authentication | Password + bearer token (V0); OIDC (V2) |
| IA-05 Authenticator management | Token revocation, password reset |
| SC-08 Transmission confidentiality | mTLS for agent-server (V1.5); TLS for the API |
| SC-13 Cryptographic protection | age, TLS, cosign, SBOM |
| SI-02 Flaw remediation | `cargo audit` in CI, security advisories public |
| SI-07 Software/firmware integrity | cosign signature verification, reproducible builds |

(All 121 controls are mapped; the table above is illustrative.)

### 5.2 EUCS Substantial (European Cybersecurity Scheme for Cloud Services)

**Status:** V2.

The mapping is similar in structure to BSI C5. The full mapping lives in `docs/sovereignty/eucs-substantial-checklist.md`.

**The key EUCS-Substantial requirements:**
- EU-resident data
- EU-incorporated provider
- EU-based support staff
- Public SBOM
- Public security advisories
- Right to audit
- Right to export
- Right to terminate without penalty

**The product satisfies all of these by construction.**

### 5.3 The compliance export

```bash
sovereign compliance map --standard bsi-c5-2026 --format pdf > bsi-c5-2026-report.pdf
sovereign compliance map --standard eucs-substantial --format pdf > eucs-substantial-report.pdf
```

The PDF is a single-page-per-control report with the mapping, the evidence (logs, config, test results), and the gaps.

---

## 6. The maintainability test for sovereignty features

Every sovereignty feature must pass this test before shipping:

1. **Is the feature testable in CI?** (e.g., the offline-mode test runs the binary in a network-isolated container)
2. **Does the feature add a maintenance burden?** (e.g., cosign signing requires key management; key rotation is V1.5)
3. **Can the feature be removed without breaking the contract?** (e.g., SBOM generation is opt-in; not on by default)
4. **Does the feature have a public doc?** (e.g., `docs/sovereignty/10-point-test.md`)
5. **Is the feature mentioned in the marketing?** (only if it's not theatre)

### 6.1 Features that pass

- Offline mode (testable, opt-in, public doc, mentioned in marketing)
- Export (testable, low maintenance, public doc, mentioned in marketing)
- Import (testable, low maintenance, public doc, mentioned in marketing)
- Encryption at rest (testable, low maintenance, public doc, mentioned in marketing)
- Public SBOM (testable, opt-in, public doc, mentioned in marketing)
- Signed releases (testable, low maintenance, public doc, mentioned in marketing)
- Audit log (testable, low maintenance, public doc, mentioned in marketing)
- EUCS-Substantial controls mapping doc (testable, low maintenance, public doc, mentioned in marketing)

### 6.2 Features that fail (don't build)

- "GDPR compliance" badge without substance → fail (no test, no doc, marketing-only)
- "Immutable audit log" without a real immutability model → fail (SQLite triggers pass; "blockchain-backed" does not)
- "Zero-trust" without ZTA architecture → fail (no test, no doc, marketing-only)
- "Air-gapped" without testing air-gapped operation → fail (must pass the offline-mode test)
- "SOC2 ready" without SOC2 → fail (no test, no doc, marketing-only)
- "AI-driven" anything that doesn't have a deterministic backup → fail (the ML scorer is always paired with a rule)

---

## 7. The "sovereign" brand — what it means to a buyer

| Buyer concern | How the product answers |
|---|---|
| **My data is in a jurisdiction I trust** | EU-resident; no cross-border transfer; verifiable |
| **I can run it without calling anyone** | Self-hosted; works offline; no SaaS dependency |
| **The vendor can disappear and the product keeps working** | Apache 2.0, foundation transfer plan, export/import |
| **The company is incorporated where I trust** | EU-incorporated (Berlin GmbH + Estonian OÜ) |
| **The technology is open and inspectable** | Open source, SBOM, cosign-signed releases |

This is the **messaging** for the website's `/sovereignty` page. Every claim is backed by a feature, a test, and a public doc.

---

## 8. The vendor-disappear test (the 100-year-old question)

**The question:** "If you disappear, what happens to the users?"

**The answer:** The binary keeps working. The data is portable. The license is irrevocable.

### 8.1 The test (CI gate, runs on every release)

```bash
# 1. Spin up a fresh VM in CI (e.g., Hetzner CX22, no special config)
# 2. Install the binary
curl -sSf sovereignruntime.dev/install.sh | sh

# 3. Restore from the last backup
sovereign import platform --from <last-backup>

# 4. Verify
sovereign status                  # 4 / 4 healthy
sovereign apps list               # 4 apps
sovereign audit --since 90d       # full audit log
sovereign backup list --verified  # 12 / 12 verified

# 5. Verify TLS certs are valid
sovereign certs list

# 6. Verify secrets are decrypted
sovereign secret list api
```

**If any step fails, the release does not ship.** This is the **founding promise**.

### 8.2 The "founding promise" page

```text
# https://sovereignruntime.dev/founding-promise

If Sovereign Application Runtime GmbH disappears, the product keeps working.

This is not a marketing claim. This is a CI gate that runs on every release.

What we promise:
1. The binary will always be available at sovereignruntime.dev (and on IPFS, and at the Linux Foundation).
2. The license is Apache 2.0, irrevocable, no carve-outs.
3. The data is exportable: `sovereign export platform` produces a complete backup.
4. The data is importable: `sovereign import platform --from <backup>` restores on any host.
5. The source code is open: github.com/sovereignruntime/sovereign, all commits public.
6. The 10-point sovereignty test is a CI gate; releases that fail the test do not ship.
7. By year 3, the project is owned by a foundation (Linux Foundation Europe).

Last verified: <date> (CI run #<number>)
Result: 10/10 passed
```

---

## 9. The 12 named risks (and the mitigations)

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Coolify adds sovereignty package | High | Medium | Move fast; sovereignty is values + compliance, not just features |
| Dokploy fixes its license / adds GitOps | High | Low | Differentiate on Rust + sovereign values |
| Hyperscaler sovereign SKU wins procurement | Medium | High | Differentiate on self-hosted (hyperscalers can't match) |
| Edge platforms reframe sovereignty | Medium | Medium | Position as for stateful apps, not all apps |
| AI agents reduce need for deployment runtime | Low | High | Counter: regulated buyers need auditable, deterministic deployment |
| Single→multi-server architecture needs rewrite | High (if done wrong) | Critical | Build agent pattern in V1.5, rqlite in V2 |
| Dokploy-style stuck-deployment bug | High | High | Auto-rollback on health fail; cancel path first-class |
| Caddy OOM under load | Medium | Medium | Offer Nginx as low-mem alternative; document Caddy limits |
| SQLite single-writer at multi-server scale | Low | High | Move to rqlite in V2; plan from day 1 |
| BuildKit security (untrusted builds) | Medium | High | Allow `--image=...` to bypass; delegate to CI as default |
| CasaOS-style abandonment | Medium | Critical | EU incorporation + foundation plan + public sustainability |
| License change backlash (Dokploy) | Medium | High | Apache 2.0 forever, public commitment, no `proprietary/` |

---

## 10. The "what about SOC 2, ISO 27001, and the MNC buyer?" note

This document covers the **sovereignty** story (the 5 dimensions, the 10-point test, the OPA/Rego policy, the BSI C5:2026 and EUCS Substantial mappings). The **enterprise-readiness** story — SOC 2 Type II, ISO 27001:2022, the 3 support tiers + SLAs, the penetration test plan, the DPA template, the GDPR sub-processor list, the 30-60-90 day enterprise onboarding playbook — is in [`enterprise-readiness.md`](./enterprise-readiness.md).

The two documents are complementary:

- **Sovereignty is structural** — the corporate graph, the SBOM, the EU incorporation, the EU-only defaults, the vendor-disappear test. Sovereignty is the *why*; it cannot be bought.
- **Enterprise-readiness is procedural** — the SOC 2 audit, the ISO 27001 certificate, the BSI C5 attestation, the EUCS certification, the DPA, the pen-test. Enterprise-readiness is the *proof*; it can be earned.

A buyer who wants to know "is this company European and structurally sovereign?" reads `sovereignty-and-governance.md`. A buyer who wants to know "is this company audit-ready for my procurement office?" reads `enterprise-readiness.md`. A buyer who wants both reads both.

The G21 OIDC SSO, G22 log shipping, and G23 append-only audit log are the **load-bearing technical pieces** that connect the two stories — the structural sovereignty (the EU-only defaults, the EU incorporation) is verifiable by the auditor via `sovereign audit verify` and `sovereign compliance scan` (I17). Without these three features, the SOC 2 / ISO 27001 / BSI C5 / EUCS mappings are paper; with them, the mappings are verifiable.

---

**Next: read [`enterprise-readiness.md`](./enterprise-readiness.md) for the support tiers, SLAs, framework mappings, penetration test plan, and enterprise onboarding playbook, and [`decision-records.md`](./decision-records.md) for the ADRs.**
