# Runbook: ABDM/ABHA Bridge (`abdm-bridge`)

> **Module owner:** Integrations guild · **Last tested:** 2026-06-05 · **Drill cadence:** quarterly
> **On-call PagerDuty service:** `medicare-abdm-prod`
> **Datadog dashboard:** https://app.datadoghq.com/dashboard/medicare-abdm
> **Sentry project:** `medicare-api` filter `module:abdm`

## 1. When to use

Page this runbook when any of the following fire:
- **PagerDuty alert:** `abdm-nha-gateway-down` (NHA gateway health check fails), `abdm-consent-artifact-invalid` (> 1% of consent artifacts fail validation), `abdm-hip-push-failure` (HIP push error rate > 5%), `abdm-cert-expiry-warning` (M1/M2/M3 cert expires in < 30 days), `abdm-hiu-pull-failure` (HIU pull error rate > 5%).
- **Sentry:** error rate > 0.5% over 5 min in the `abdm` module, or any SEV-1.
- **Customer report:** Indian hospital says "ABHA linking is broken", "consent artifact is rejected", "we can't pull the patient's records".

## 2. Detection

- **Datadog:** **ABDM** dashboard panels:
  - `abdm.nha_gateway.up` (target: 1; `sandbox.abdm.gov.in` or `abdm.gov.in`)
  - `abdm.consent_artifact.validation.success_rate` (target: ≥ 99%)
  - `abdm.hip_push.success_rate` (target: ≥ 95%)
  - `abdm.hiu_pull.success_rate` (target: ≥ 95%)
  - `abdm.cert.expires_in_days` (target: ≥ 30 days)
  - `abdm.token.refresh.success_rate` (target: ≥ 99%)
- **Sentry:** `module:abdm` — common errors: `NhaGatewayUnreachable`, `ConsentArtifactValidationFailed`, `HipPushRejected`, `HiuPullTimeout`, `CertificateExpired`, `TokenRefreshFailed`.
- **Logs:** `kubectl logs -l app=medicare-api -c api | grep abdm` — look for NHA 4xx/5xx, mTLS handshake failures, cert verification errors.

## 3. Diagnosis

1. **Check the NHA gateway health.**
   ```bash
   # Sandbox
   curl -s -o /dev/null -w "%{http_code}\n" https://dev.abdm.gov.in/api/hiecm/certs
   # Production
   curl -s -o /dev/null -w "%{http_code}\n" https://abdm.gov.in/api/hiecm/certs
   ```
2. **Check the OAuth token.**
   ```bash
   php artisan tinker
   > Cache::get('abdm.access_token')
   ```
   If the token is missing or expired, refresh it: `php artisan abdm:token-refresh`.
3. **Check the mTLS cert.**
   ```bash
   openssl x509 -in /etc/medicare/abdm/ABHA_PRIVATE_KEY_PATH -noout -dates
   ```
   If the cert is expired, rotate it: see Option A.
4. **Check the ABDM cert level.** Run:
   ```bash
   php artisan abdm:cert-status
   ```
   Outputs the current M1/M2/M3 cert status, expiry, and the next milestone.
5. **Check the consent artifacts** for validation failures:
   ```sql
   SELECT id, patient_id, hip_id, status, validation_error, created_at
   FROM abdm_consent_artifacts WHERE status = 'invalid' AND created_at > NOW() - INTERVAL 1 HOUR
   ORDER BY created_at DESC LIMIT 20;
   ```
6. **Check the HIP push queue.** If pushes are stuck:
   ```sql
   SELECT id, patient_id, type, status, last_error, attempts
   FROM abdm_hip_push_queue WHERE status = 'failed' AND attempts >= 3 ORDER BY last_attempt_at DESC;
   ```
7. **Check the HIU pull failures.**
   ```sql
   SELECT id, patient_id, hip_id, status, error_code, error_message
   FROM abdm_hiu_pull_log WHERE status = 'failed' AND created_at > NOW() - INTERVAL 1 HOUR
   ORDER BY created_at DESC LIMIT 20;
   ```

## 4. Mitigation

Pick the smallest hammer first.

- **Option A — Rotate the mTLS cert** (if the cert is expired or expiring soon):
  1. Generate a new CSR: `php artisan abdm:cert-csr --cn=medicare-{tenant_id}`.
  2. Upload to ABDM portal: https://abdm.gov.in/cert.
  3. Wait for approval (1-3 business days for production; instant for sandbox).
  4. Download the new cert: `php artisan abdm:cert-download --request-id={id}`.
  5. Restart the API: `kubectl rollout restart deployment/medicare-api`.

- **Option B — Refresh the OAuth token** (if the token is expired or revoked):
  ```bash
  php artisan abdm:token-refresh --tenant={tenant_id}
  ```
  The new token is cached in Redis for 30 min.

- **Option C — Disable the feature flag** (last resort, if the integration is fundamentally broken):
  ```sql
  UPDATE tenants SET feature_flags = JSON_SET(feature_flags, '$.abdm_bridge', false) WHERE id = ?;
  ```
  The patient app falls back to manual ABHA entry (V1 behavior).

- **Option D — Re-queue failed HIP pushes** (if a transient gateway error caused the failure):
  ```bash
  php artisan abdm:hip-push-retry --older-than=1h
  ```
  This re-queues all failed pushes from the last 1 h; the queue worker retries with exponential backoff.

- **Option E — Restart the API deployment** (if Sentry shows widespread `NhaGatewayUnreachable` due to a bad middleware):
  ```bash
  kubectl rollout restart deployment/medicare-api
  ```

## 5. Resolution

1. Document the root cause in `#incident-abdm-{date}`.
2. Apply the fix: cert rotation, token refresh, code change, or config change.
3. Re-enable any disabled feature flag.
4. Deploy via the standard pipeline.
5. Monitor the Datadog dashboard for 30 min.
6. Notify the affected hospitals: "ABDM integration is restored. No PHI was exposed."

## 6. Post-mortem template

```yaml
Incident: INC-2026-06-05-003
Duration: 11:00 → 12:15 UTC (75 min)
Detection: Datadog alert abdm-hip-push-failure (P2 → P1 in 8 min)
Mitigation time: 35 min (Option A applied; cert was expiring in 12 days but the rotation was scheduled for next week)
Resolution time: 75 min (cert approved, downloaded, deployed, queue drained)
Customer impact: 8 Indian hospitals; 3,200 HIP pushes queued; 0 PHI loss (queue persisted)
Root cause: 5-why:
  1. Why did the HIP push fail? — The mTLS cert was rejected by the NHA gateway.
  2. Why was the cert rejected? — The cert was within the 30-day expiry window and the NHA gateway was rejecting early-rotated certs.
  3. Why did we not rotate earlier? — The cert rotation was scheduled for next week (30 days out).
  4. Why was the rotation scheduled late? — The cron `abdm:cert-rotation-tick` was set to T-30 days.
  5. Why was the cron set to T-30? — The cron was set when the ABDM gateway was more lenient about expiry.
Action items:
  - owner: integrations-eng | due: 2026-06-12 | item: Change the cert rotation cadence to T-60 days
  - owner: integrations-eng | due: 2026-06-12 | item: Add a Datadog alert for cert expiry at T-60, T-30, T-14, T-7, T-1
  - owner: integrations-eng | due: 2026-06-19 | item: Add a CI check that validates the cert is uploaded to ABDM before deploy
```
