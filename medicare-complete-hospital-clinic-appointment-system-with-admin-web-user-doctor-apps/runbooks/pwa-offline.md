# Runbook: PWA Offline (`pwa-offline`)

> **Module owner:** Mobile guild · **Last tested:** 2026-06-05 · **Drill cadence:** quarterly
> **On-call PagerDuty service:** `medicare-pwa-offline-prod`
> **Datadog dashboard:** https://app.datadoghq.com/dashboard/medicare-pwa-offline
> **Sentry project:** `medicare-web` filter `module:pwa-offline`

## 1. When to use

Page this runbook when any of the following fire:

- **PagerDuty alert:** `pwa-install-rate-drop` (install rate falls below 5% over 24 h), `pwa-sync-failure-rate-spike` (>5% of syncs fail over 15 min), `pwa-push-delivery-rate-drop` (delivery < 90% over 1 h), `pwa-storage-budget-violation` (a patient reports storage full + queued actions dropped).
- **Sentry:** error rate > 0.5% over 5 min in the `pwa-offline` module, or any SEV-1.
- **Customer report:** support ticket with `pwa` or `offline` tag; patient says "my PWA is broken" or "my queued action disappeared".

## 2. Detection

- **Datadog:** watch the **PWA Offline** dashboard panels:
  - `pwa.install.completed.rate` (target: ≥ 5% of weekly active users install)
  - `pwa.sync.success_rate` (target: ≥ 95%)
  - `pwa.sync.conflict_rate` (target: ≤ 2%)
  - `pwa.sync.avg_latency_ms` (target: ≤ 500 ms p99)
  - `pwa.push.delivery_rate` (target: ≥ 90%)
  - `pwa.storage.avg_bytes` (target: ≤ 50 MB per user)
  - `pwa.version.adoption_rate` (target: ≥ 80% within 7 days of release)
- **Sentry:** `module:pwa-offline` — common errors: `ServiceWorkerRegistrationFailed`, `IndexedDBQuotaExceeded`, `BackgroundSyncReplayFailed`, `PushSubscriptionExpired`.
- **Logs:** `kubectl logs -l app=medicare-api -c api | grep pwa-offline` — look for `pwa_queued_actions.status='failed'`, `pwa_subscriptions.status='expired'`, `pwa_storage_stats.total_bytes > 100MB`.

## 3. Diagnosis

1. **Check the PWA version adoption.** If the latest version is not adopted, the issue may be a bad release. Run:
   ```sql
   SELECT version, COUNT(*) AS users, MAX(reported_at) AS last_reported
   FROM pwa_app_versions v
   JOIN pwa_storage_stats s ON s.patient_id IN (SELECT id FROM users WHERE tenant_id = v.tenant_id)
   GROUP BY version ORDER BY users DESC;
   ```
2. **Check the sync queue.** If the queue is stuck, find the oldest pending action:
   ```sql
   SELECT id, patient_id, action_type, attempts, last_error, created_at
   FROM pwa_queued_actions WHERE status = 'pending' ORDER BY created_at ASC LIMIT 20;
   ```
3. **Check the push delivery rate.** If low, find the expired subscriptions:
   ```sql
   SELECT id, patient_id, endpoint, status, last_seen_at
   FROM pwa_subscriptions WHERE status = 'expired' ORDER BY last_seen_at DESC LIMIT 50;
   ```
4. **Check the conflict rate.** If high, find the most common conflict type:
   ```sql
   SELECT action_type, COUNT(*) AS conflicts
   FROM pwa_queued_actions WHERE status = 'conflict' AND created_at > NOW() - INTERVAL 1 HOUR
   GROUP BY action_type ORDER BY conflicts DESC;
   ```
5. **Check the storage budget.** If many users near the cap:
   ```sql
   SELECT patient_id, total_bytes, actions_count
   FROM pwa_storage_stats WHERE total_bytes > 90 * 1024 * 1024 ORDER BY total_bytes DESC LIMIT 50;
   ```
6. **Check external dependencies:**
   - VAPID keys still valid? (`MEDICARE_VAPID_PUBLIC_KEY` / `MEDICARE_VAPID_PRIVATE_KEY`)
   - The workbox service worker is being served? `curl -I https://patient.medicare.app/service-worker.js` returns 200.
   - The manifest.json is valid? `curl -I https://patient.medicare.app/manifest.json` returns 200 + `Content-Type: application/manifest+json`.

## 4. Mitigation

Pick the smallest hammer first.

- **Option A — Push the latest version** (if a new app version is available and the user is on a stale version):
  ```bash
  php artisan pwa:version-release --version=$(date +%Y.%m.%d) --mandatory=true --notes="Hotfix: PWA sync fix"
  ```
  Marking `mandatory=true` forces a refresh on the user's next page load.

- **Option B — Clear the failed actions** (if the queue has too many `failed` rows):
  ```bash
  php artisan pwa:clear-failed-actions --older-than=24h
  ```
  This unblocks the queue; failed actions are reported to the patient (they can manually retry).

- **Option C — Increase the storage cap** (if many users hit the 110 MB cap):
  ```sql
  UPDATE tenants SET pwa_storage_cap_bytes = 200 * 1024 * 1024 WHERE id = ?;  -- 200 MB
  ```
  The patient app reads the cap on next sync.

- **Option D — Disable the feature flag** (last resort, if the PWA is fundamentally broken):
  ```sql
  UPDATE tenants SET feature_flags = JSON_SET(feature_flags, '$.pwa_offline', false) WHERE id = ?;
  ```
  The patient app falls back to the standard web app (no offline support).

- **Option E — Restart the API deployment** (if Sentry shows widespread `ServiceWorkerRegistrationFailed` due to a bad middleware):
  ```bash
  kubectl rollout restart deployment/medicare-api
  ```

## 5. Resolution

1. Document the root cause in `#incident-pwa-offline-{date}`.
2. Apply the fix: code change, or config change, or infra change.
3. Deploy via the standard pipeline: PR → CI → staging → production (canary 10% → 50% → 100%).
4. Re-enable any disabled feature flag.
5. Monitor the Datadog dashboard for 30 min.
6. Post the resolution in `#incident-pwa-offline-{date}` with: duration, root cause, fix, customer impact.

## 6. Post-mortem template

```yaml
Incident: INC-2026-06-05-001
Duration: 14:23 → 14:58 UTC (35 min)
Detection: Datadog alert pwa-sync-failure-rate-spike (P2 → P1 in 5 min)
Mitigation time: 18 min (Options A + E applied)
Resolution time: 35 min
Customer impact: 12,400 patients (sync queue stuck for 35 min; 2,800 reported via support)
Root cause: 5-why:
  1. Why did the sync queue get stuck? — The replay loop was in a tight retry cycle.
  2. Why was the replay loop tight? — A bug in the Background Sync handler returned 500 on a 409 conflict instead of resolving.
  3. Why was 500 returned on 409? — A recent code change assumed 409 only for "appointment cancelled", but not for "patient already checked in".
  4. Why was the assumption incomplete? — Incomplete test coverage (the conflict test only covered 1 of 3 conflict types).
  5. Why was test coverage incomplete? — The conflict-test file was skipped in CI due to a flaky LLM mock.
Action items:
  - owner: mobile-eng | due: 2026-06-12 | item: Fix the 409 handling for "patient already checked in"
  - owner: mobile-eng | due: 2026-06-12 | item: Add the 3 conflict types to the test matrix
  - owner: platform-eng | due: 2026-06-19 | item: Add a flakiness detector that un-skips tests after 2 consecutive flakes
```
