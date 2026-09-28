# Runbook: SOC 2 Type II (`soc2-type-ii`)

> **Module owner:** Security guild · **Last tested:** 2026-06-05 · **Drill cadence:** semi-annual
> **On-call PagerDuty service:** `medicare-soc2-prod` (rarely pages; mostly for audit-prep alerts)
> **Datadog dashboard:** https://app.datadoghq.com/dashboard/medicare-soc2
> **Sentry project:** N/A (audit work, not runtime)

## 1. When to use

This runbook is for the **audit team**, not the on-call engineer. Page this runbook when any of the following fire:

- **Audit prep alert:** Big-4 firm requests evidence; quarterly evidence collection job fails; control test overdue; exception discovered mid-audit.
- **Customer report:** enterprise customer asks for the latest SOC 2 Type II report (a normal request, not an incident).
- **Internal:** the 12-month observation period is approaching; controls need to be locked for the audit window.

## 2. Detection

- **Datadog:** **SOC 2** dashboard panels:
  - `soc2.control_test.pass_rate` (target: ≥ 95%)
  - `soc2.control_test.overdue_count` (target: 0)
  - `soc2.evidence_request.fulfillment_rate` (target: ≥ 90% within SLA)
  - `soc2.exception.discovered_count` (target: 0 critical, ≤ 5 minor per quarter)
- **Internal cron:** `soc2:control-test-tick` runs daily at 06:00 UTC. Failures email the security guild and the SOC 2 lead.
- **Audit firm portal:** Big-4 firm uploads evidence requests; the team is notified via the portal's webhook.

## 3. Diagnosis

1. **Identify the failing control.** Query the `controls` table:
   ```sql
   SELECT id, code, name, owner, status, last_test_at, last_test_result
   FROM controls WHERE status = 'failing' OR last_test_result = 'fail' ORDER BY last_test_at DESC;
   ```
2. **Identify the exception.** Query the `control_exceptions` table:
   ```sql
   SELECT id, control_id, severity, discovered_at, description, remediation_status
   FROM control_exceptions WHERE severity IN ('critical', 'high') AND remediation_status != 'closed'
   ORDER BY discovered_at DESC;
   ```
3. **Check the evidence-request queue.** Query the `evidence_requests` table:
   ```sql
   SELECT id, control_id, requested_by, requested_at, fulfilled_at, sla_due_at
   FROM evidence_requests WHERE fulfilled_at IS NULL AND sla_due_at < NOW() + INTERVAL 3 DAY
   ORDER BY sla_due_at ASC;
   ```
4. **Cross-check with the audit log.** The `audit_logs` table is the source of truth for control evidence. Run:
   ```sql
   SELECT action, COUNT(*) FROM audit_logs
   WHERE created_at BETWEEN ? AND ? AND action LIKE '%.%' GROUP BY action ORDER BY 2 DESC;
   ```
5. **Check the Big-4 portal.** Look at the open evidence requests, the open findings, and the next audit milestone.

## 4. Mitigation

Pick the smallest hammer first. SOC 2 is a **process**, not a code path; mitigation is rarely "restart a pod".

- **Option A — Remediate the control failure** (most common):
  1. Identify the root cause (the control's `last_test_result='fail'` indicates which sub-test failed).
  2. Apply the fix (code change, process change, or training).
  3. Re-run the control test: `php artisan soc2:control-test --control={code} --since=30d`.
  4. Mark the control as `passing` in the `controls` table.
  5. Document the remediation in the incident channel.

- **Option B — Accept the exception** (if the fix is not feasible within the audit window):
  1. Document the exception in `control_exceptions` with a clear `description` and `remediation_plan`.
  2. Notify the SOC 2 lead and the Big-4 firm.
  3. Add a compensating control (e.g., manual review) for the duration of the observation period.

- **Option C — Extend the observation period** (if too many exceptions were discovered):
  1. Notify the Big-4 firm; the firm will issue a "bridge letter" for the customers in the meantime.
  2. Extend the observation period by 3-6 months.
  3. Continue collecting evidence; re-attest at the end of the extended period.

- **Option D — Pull the latest report for a customer request**:
  ```bash
  php artisan soc2:report-export --period=latest --format=pdf --to=customer@example.com
  ```
  The PDF is generated from the latest `controls` + `control_tests` + `control_exceptions` + `evidence_requests` data.

## 5. Resolution

1. Document the incident (or audit request) in `#soc2-audit-{period}`.
2. Apply the fix or accept the exception.
3. Notify the Big-4 firm; update the audit log.
4. Notify affected customers (if a bridge letter is needed, per Option C).
5. Schedule the next quarterly evidence collection.

## 6. Post-mortem template

```yaml
Audit period: 2026-01-01 → 2026-12-31 (12 months observation)
Incident: SOC2-2026-Q2-001 (control failure discovered in audit week 2)
Duration: 2026-06-05 09:00 → 2026-06-05 17:00 UTC (8 hours to remediate)
Detection: Big-4 firm portal evidence request (control CC6.1 "logical access controls")
Mitigation time: 4 hours (root cause: a developer had not been offboarded from a former contractor)
Resolution time: 8 hours (account disabled, audit log reviewed, exception filed)
Customer impact: 0 customers (internal finding; no PHI exposure)
Root cause: 5-why:
  1. Why was the contractor's account still active? — The offboarding script was skipped.
  2. Why was the offboarding script skipped? — The HR ticket was closed without the offboarding checklist being completed.
  3. Why was the checklist not enforced? — The offboarding workflow did not require a manager sign-off.
  4. Why was sign-off not required? — The workflow was designed for full-time employees, not contractors.
  5. Why was the workflow not extended to contractors? — Contractor management was owned by Procurement, not HR.
Action items:
  - owner: sec-eng | due: 2026-06-12 | item: Add contractor offboarding to the HR offboarding workflow
  - owner: sec-eng | due: 2026-06-12 | item: Add a daily cron that detects orphan accounts (no login in 90 days + no recent activity)
  - owner: sec-eng | due: 2026-06-19 | item: Add a control test that auto-validates the offboarding checklist is completed
```
