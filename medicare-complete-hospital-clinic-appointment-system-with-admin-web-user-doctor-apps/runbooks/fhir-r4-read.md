# Runbook: FHIR R4 Read (`fhir-r4-read`)

> **Module owner:** Integrations guild · **Last tested:** 2026-06-05 · **Drill cadence:** quarterly
> **On-call PagerDuty service:** `medicare-fhir-r4-prod`
> **Datadog dashboard:** https://app.datadoghq.com/dashboard/medicare-fhir-r4
> **Sentry project:** `medicare-api` filter `module:fhir-r4`

## 1. When to use

Page this runbook when any of the following fire:

- **PagerDuty alert:** `fhir-r4-p99-latency-spike` (p99 > 500 ms over 5 min), `fhir-r4-error-rate-spike` (> 1% over 5 min), `fhir-r4-sidecar-down` (HAPI-FHIR sidecar health check fails), `fhir-r4-bulk-export-stuck` (bulk export job > 1 h).
- **Sentry:** error rate > 0.5% over 5 min in the `fhir-r4` module, or any SEV-1.
- **Customer report:** hospital integration partner says "FHIR endpoint is returning 500", "OAuth token expired", or "capability statement is stale".

## 2. Detection

- **Datadog:** **FHIR R4** dashboard panels:
  - `fhir_r4.request.rate` (target: stable; spikes may indicate partner integration ramp)
  - `fhir_r4.request.p99_latency_ms` (target: ≤ 300 ms)
  - `fhir_r4.error_rate` (target: ≤ 0.5%)
  - `fhir_r4.sidecar.up` (target: 1; HAPI-FHIR sidecar healthy)
  - `fhir_r4.oauth.token.exchange.success_rate` (target: ≥ 99%)
  - `fhir_r4.bulk_export.active_count` (target: ≤ 5 concurrent)
  - `fhir_r4.bulk_export.avg_duration_min` (target: ≤ 30 min per 100k resources)
- **Sentry:** `module:fhir-r4` — common errors: `HapiFhirSidecarUnreachable`, `OAuthTokenExchangeFailed`, `CapabilityStatementStale`, `BulkExportTimeout`, `SmartTokenScopeInvalid`.
- **Logs:** `kubectl logs -l app=medicare-fhir-gateway -c hapi-fhir` — look for FHIR validation errors, DB connection pool exhaustion, or JVM OOM.

## 3. Diagnosis

1. **Check the HAPI-FHIR sidecar health.**
   ```bash
   kubectl exec -it deploy/medicare-fhir-gateway -- curl -s http://localhost:8080/fhir/metadata | jq .
   ```
   If the sidecar is down, restart it: `kubectl rollout restart deployment/medicare-fhir-gateway`.
2. **Check the OAuth token exchange.** If partners report 401:
   ```sql
   SELECT client_id, COUNT(*) AS failed_exchanges
   FROM fhir_smart_tokens WHERE status = 'expired' AND created_at > NOW() - INTERVAL 1 HOUR
   GROUP BY client_id ORDER BY failed_exchanges DESC;
   ```
3. **Check the FHIR resource validation.** If partners report 422:
   ```bash
   kubectl logs -l app=medicare-fhir-gateway -c hapi-fhir | grep "Validation failed" | tail -50
   ```
4. **Check the DB connection pool.** If p99 latency is high:
   ```bash
   kubectl exec -it deploy/medicare-fhir-gateway -- curl -s http://localhost:8080/actuator/metrics/hikaricp.connections.active
   ```
5. **Check the bulk export jobs.** If a job is stuck:
   ```sql
   SELECT id, tenant_id, status, started_at, resources_count
   FROM fhir_bulk_export_jobs WHERE status = 'running' AND started_at < NOW() - INTERVAL 1 HOUR
   ORDER BY started_at ASC;
   ```

## 4. Mitigation

Pick the smallest hammer first.

- **Option A — Restart the HAPI-FHIR sidecar** (if the sidecar is unhealthy or OOM):
  ```bash
  kubectl rollout restart deployment/medicare-fhir-gateway
  ```
  The sidecar takes ~30 s to come back up; in-flight requests will get 503.

- **Option B — Increase the HAPI-FHIR JVM heap** (if OOM is the root cause):
  ```bash
  kubectl set env deployment/medicare-fhir-gateway JAVA_OPTS="-Xmx4g"
  ```
  Then restart: `kubectl rollout restart deployment/medicare-fhir-gateway`.

- **Option C — Clear the OAuth token cache** (if a partner's token is stuck):
  ```bash
  php artisan tinker
  > Cache::tags('fhir_smart_tokens')->flush();
  ```
  Partners will need to re-exchange their tokens.

- **Option D — Kill a stuck bulk export** (if a job is hung):
  ```sql
  UPDATE fhir_bulk_export_jobs SET status = 'cancelled', cancelled_at = NOW() WHERE id = ?;
  ```
  The partner will be notified; they can re-initiate the export.

- **Option E — Roll back to the previous version** (last resort, if a recent deploy broke a FHIR resource mapping):
  ```bash
  kubectl rollout undo deployment/medicare-api
  kubectl rollout undo deployment/medicare-fhir-gateway
  ```

## 5. Resolution

1. Document the root cause in `#incident-fhir-r4-{date}`.
2. Apply the fix: code change, config change, or infra change.
3. Re-enable any disabled feature flag (none in this runbook).
4. Deploy via the standard pipeline: PR → CI → staging → production.
5. Monitor the Datadog dashboard for 30 min.
6. Notify the affected partners: a post-mortem summary + the next steps.

## 6. Post-mortem template

```yaml
Incident: INC-2026-06-05-002
Duration: 09:15 → 10:42 UTC (87 min)
Detection: Datadog alert fhir-r4-p99-latency-spike (P3 → P2 in 10 min)
Mitigation time: 22 min (Options A + B applied)
Resolution time: 87 min (had to roll back a bad migration that added an unindexed column on `fhir_patient`)
Customer impact: 4 hospital partners; p99 latency 480ms → 4,200ms → 180ms (back to normal)
Root cause: 5-why:
  1. Why did p99 latency spike? — The `fhir_patient` table scan went from 50ms to 3,800ms.
  2. Why did the scan slow down? — A recent migration added a `custom_fields` JSONB column without an index.
  3. Why was no index added? — The migration was approved without a DB review.
  4. Why was the migration not reviewed? — The DB review was a soft requirement, not enforced in CI.
  5. Why was the soft requirement not enforced? — Migration review was owned by an individual, not a process.
Action items:
  - owner: integrations-eng | due: 2026-06-12 | item: Add a GIN index on `fhir_patient.custom_fields`
  - owner: platform-eng | due: 2026-06-12 | item: Add a CI check that flags migrations without an index on new columns > 1MB
  - owner: platform-eng | due: 2026-06-19 | item: Move migration review to a CODEOWNERS-enforced process
```
