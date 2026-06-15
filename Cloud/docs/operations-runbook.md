# Operations Runbook

**Status:** Locked for V1+. Updated as alerts and runbooks are added.
**Audience:** The on-call operator. The first 5 employees. Every design partner's ops team.
**Last updated:** 2026-06-04

This document is the **what to do when something is broken at 3am** guide. It is opinionated, short, and actionable. Every alert has a runbook. Every runbook has 4 questions answered. If an alert doesn't have a runbook, it doesn't ship.

---

## 1. The five morning commands

Bind these to a tmux key (`Ctrl+B m` by default) on every operator's laptop. They take 20 seconds and answer 90% of "is everything OK?" questions. **`sovereign doctor` is the on-call's first command, in the morning and at 3am.** The full doctor spec is in [`doctor.md`](./doctor.md).

```bash
# 1. Doctor (V0: basic, V1: standard, V1.5: full, V2: paranoid)
sovereign doctor --level standard
# → 40+ checks, ~5-30s, the on-call's first command

# 2. Orchestrator health
sovereign status --watch

# 3. Host resource sanity
ssh node-1 -- 'uptime && free -h && df -h /var/lib/sovereign && ss -s'

# 4. Recent error rate
sovereign logs --since 1h --level error --tail 200

# 5. Certificate expiry
sovereign certs list --warn-days 21
```

`sovereign morning-report` (V1) is the same five commands wrapped as a subcommand. It prints a 10-line summary in < 5 seconds.

```text
$ sovereign morning-report
[doctor --level standard]
  ✓ system          6/6 pass
  ✓ binary          2/2 pass
  ✓ storage         4/4 pass
  ✓ runtime         3/3 pass
  ✓ proxy           4/4 pass
  ✓ secrets         3/3 pass
  ✓ backup          6/6 pass
  ✓ network         5/5 pass
  ✓ observability   3/3 pass
  ✓ security        6/6 pass
  ⚠ sovereignty     2/3 pass, 1 warn (european_owned: stub, V2)
  ✓ performance     4/4 pass
  Total: 44/45 pass, 1 warn, 0 fail. Doctor: HEALTHY.

✓ sovereign control plane (v1.4.2) — healthy, uptime 47d 3h
✓ server node-1 (Hetzner CX32) — 28% CPU, 41% RAM, 38% disk
✓ apps: 4 / 4 healthy
   api      ●  3m23s ago  v1.4.2    142ms p99
   web      ●  11m ago    v0.8.1    89ms p99
   worker   ●  47m ago    v0.8.1    22ms p99
   cron     ●  2h ago     v0.8.1    —
✓ backup: 12 / 12 verified (last drill: 2d ago)
✓ TLS certs: 3 / 3 valid (next expiry: 47d)
✗ last 24h: 1 rollback (api @ 14:22, "OOM after deploy v1.4.1")
✓ 0 alerts firing, 0 deploys failed
```

### 1.1 The 3am playbook (the on-call's first 5 minutes)

When paged, the on-call's first five minutes are scripted. **No improvisation. No "let me check the dashboard first."** The first command is `sovereign doctor`.

```bash
# T+0 (page received)
sovereign doctor --level standard

# If doctor returns 0 fails: app is fine, page is probably a stale alert.
#   → sovereign doctor --explain <check> if you saw a warn
#   → sovereign incident note inc-<id> "page was stale, doctor is clean"

# If doctor returns ≥ 1 fail:
#   T+30s: try the auto-fix
sovereign doctor --level standard --fix --non-interactive

#   T+90s: re-run doctor to confirm
sovereign doctor --level standard

#   T+2m: if a fail persists, declare an incident
sovereign incident declare --title "<page title>" --severity sev1 --apps <app>

#   T+2m: roll back if the app is unhealthy
sovereign rollback <app> --confirm

#   T+5m: write the first note
sovereign incident note inc-<id> "rolled back to v1.4.1, doctor now clean"

#   T+30m (or whenever the system is back):
sovereign incident resolve inc-<id> --summary "..." --postmortem /tmp/inc-<id>-pm.md

#   Within 5 business days: review the postmortem
sovereign incident show inc-<id>
```

**Rule of thumb:** the on-call's job is `doctor → fix → rollback → note → resolve → postmortem`. Everything else is a distraction. The doctor is the entry point; the incident toolkit is the audit trail; the postmortem is the learning loop.

---

## 2. The SLOs

### 2.1 The three SLIs

| SLI | Definition | Target |
|---|---|---|
| **Availability** | `successful_requests / total_requests` over 30 days | 99.5% |
| **Latency** | p99 response time | < 300 ms |
| **Freshness** | `now() - last_data_timestamp` | < 60 s |

### 2.2 The error budget

- **99.5% over 30 days** = 3.6 hours of downtime budget per month.
- **2% fast burn in 1 hour** = page on Sev1.
- **5% slow burn in 6 hours** = ticket, no page.

(Reference: [Google SRE Workbook, Chapter 5](https://sre.google/workbook/alerting-on-slos/).)

### 2.3 The SLO violation response

| Violation | Action |
|---|---|
| < 0.5% budget burn | Note in the weekly report; no action |
| 0.5% - 2% burn | Investigate within 1 business day |
| 2% - 5% burn | Investigate within 4 hours, ticket |
| > 5% burn | Page on-call, Sev1 |
| Full budget exhausted | Freeze deploys (except hotfixes), post-mortem within 5 business days |

---

## 3. The 10 alerts

Each alert has a name, a condition, a severity, and a runbook. **No alert ships without a runbook.**

| # | Alert | Condition | Severity | Runbook |
|---|---|---|---|---|
| 1 | **HealthCheckFail** | 3 consecutive 5xx on any app's health probe | Sev1 | [§3.1](#31-healthcheckfail) |
| 2 | **DiskFull** | `df -h /var/lib/sovereign` > 90% | Sev2 | [§3.2](#32-diskfull) |
| 3 | **CertExpiring** | TLS cert expires in < 14 days | **Sev1** | [§3.3](#33-certexpiring) |
| 4 | **BackupFailure** | A backup job exits non-zero | Sev1 | [§3.4](#34-backupfailure) |
| 5 | **ServerUnreachable** | A server's heartbeat > 30s | Sev1 | [§3.5](#35-serverunreachable) |
| 6 | **MemoryHigh** | Container RSS > 90% of cgroup limit | Sev2 | [§3.6](#36-memoryhigh) |
| 7 | **CpuHigh** | Container CPU > 95% for 5m | Sev3 | [§3.7](#37-cpuhigh) |
| 8 | **DeployFailed** | A deploy transitions to `Failed` | Sev3 | [§3.8](#38-deployfailed) |
| 9 | **ErrorRateHigh** | 5xx rate > 5% over 5m | Sev1 | [§3.9](#39-errorratehigh) |
| 10 | **LatencyHigh** | p99 response time > 1s over 5m | Sev2 | [§3.10](#310-latencyhigh) |

### 3.1 HealthCheckFail

**Symptom:** An app's `/health` endpoint returns 5xx for 3 consecutive probes.

**Blast radius:** That app is down (or about to be).

**First command:**
```bash
tool status <app> --verbose
```
Expected output: a clear indication of which probe is failing (startup / liveness / readiness).

**Second command (if first doesn't help):**
```bash
tool logs <app> --since 5m --level error --tail 50
```
Expected output: error logs that explain the failure (e.g., DB connection refused, OOM).

**Third command (rollback):**
```bash
tool rollback <app> --confirm
```
Expected output: the previous version is serving traffic within 30s.

**After:** File a postmortem. If the failure is a pattern, add a policy rule (V2) or a monitor.

### 3.2 DiskFull

**Symptom:** `/var/lib/sovereign` is > 90% full.

**Blast radius:** New deploys will fail when there's no room for the new image layer.

**First command:**
```bash
tool log rotate --all
```
Expected output: logs older than 30 days are deleted; free space increases.

**Second command (if first doesn't help):**
```bash
tool backup prune --keep-last 7
```
Expected output: old backups are deleted from S3.

**Third command (if still full):**
```bash
df -h /var/lib/sovereign
du -sh /var/lib/sovereign/* | sort -h | tail -20
```
Look for unexpected growth in `logs/` or `images/`.

**After:** Add disk to the host (LVM or volume resize).

### 3.3 CertExpiring

**Symptom:** A TLS cert will expire in < 14 days.

**Severity: Sev1 (pages on-call).** A cert expiry is a hard outage for the affected hostname within hours. A 14-day warning is a fire — it means the ACME auto-renew silently broke, and we will not get a second chance before the cert dies. Treat it like `HealthCheckFail`: page, declare an incident, fix, close. (Upgraded from Sev2 in V1.5; the upgrade was driven by `user-pain-research.md` §4.2 — "the cert silently expired at 03:00 because the DNS-01 token rotated 3 weeks ago and nobody noticed.")

**Blast radius:** That hostname will be unreachable after expiry (browser TLS error, every API call 502s, every webhook fails).

**First command (auto-fix path):**
```bash
sovereign doctor --fix=tls_renew_now --hostname <hostname>
```
This is the Sev1 auto-fix: it skips the "5s destructive confirm" because the alternative (waiting for human action on a paging alert) is worse. The fix runs `sovereign certs renew <hostname>` under the hood; logs the entire renewal to the audit table (`kind: "doctor.fix.tls_renew_now"`, with the old + new cert fingerprints).

**Second command (if the auto-fix fails):**
```bash
sovereign certs renew <hostname> --verbose
```
Look for ACME challenge failures (DNS-01 or HTTP-01). The verbose output tells you which challenge failed and why.

**Third command (if ACME is broken — DNS-01):**
```bash
sovereign dns check <zone>  # the zone's NS / DS records are correct
sovereign certs renew <hostname> --challenge=dns-01 --provider <provider>  # explicit provider
```
Common cause: the `sovereign dns` provider token rotated. Re-add the token with `sovereign dns provider add <name>`.

**Fourth command (if ACME is broken — HTTP-01):**
The HTTP-01 challenge needs port 80 to be reachable from Let's Encrypt's validation servers. Check the host's firewall / cloud security group. (If the box is behind Cloudflare or another proxy, switch to DNS-01 — `sovereign certs renew <hostname> --challenge=dns-01`.)

**Fifth command (last resort — manual cert):**
Issue a cert via `certbot` or `acme.sh` and install it with `sovereign certs install <hostname> <cert-path> <key-path>`. Caddy reloads automatically.

**Verify:**
```bash
sovereign doctor --category network  # cert check should now be PASS
curl -vI https://<hostname>  # expiry is ~90 days out
```

**Postmortem:** Even if the fix is `doctor --fix`, write a short postmortem ("why did the auto-renew break?"). Common causes: DNS provider token rotated, firewall was tightened, the hostname was added to a new vhost but not the renewal config.

**After:** File a bug. Caddy auto-renew should "just work" — if it doesn't, the bug is in our integration.

### 3.4 BackupFailure

**Symptom:** A backup job exited non-zero.

**Blast radius:** No new backup; the last good backup ages. After 3 failures, the backup is "stale" (> 7 days old).

**First command:**
```bash
tool backup list --failed --since 24h
```
Expected output: the failed backup with the error message.

**Second command:**
```bash
tool backup verify <backup-id> --restore-to scratch
```
If the previous backup verifies, the failure was transient.

**Third command (if the previous is also broken):**
```bash
tool backup run <target> --verbose
```
Run the backup manually with verbose logging.

**After:** If backups are consistently failing, the storage (S3) may be down or the credentials may have rotated.

### 3.5 ServerUnreachable

**Symptom:** A server's heartbeat has not been received in 30s.

**Blast radius:** Deploys to that server fail; health checks on apps on that server fail.

**First command:**
```bash
tool server doctor <server-id>
```
Expected output: a connectivity check, a resource check, a log check.

**Second command (if first doesn't help):**
```bash
ssh <server-host> -- 'systemctl status sovereign'
```
Check if the sovereign process is running.

**Third command (if process is down):**
```bash
ssh <server-host> -- 'systemctl restart sovereign && journalctl -u sovereign -n 100'
```

**After:** If the server is consistently unreachable, it's a hardware issue. Move apps to another server with `tool server drain <server-id>`.

### 3.6 MemoryHigh

**Symptom:** A container's RSS is > 90% of its cgroup limit.

**Blast radius:** The container is at risk of OOM-kill.

**First command:**
```bash
tool status <app> --resources
```
Expected output: current CPU, memory, network, disk I/O.

**Second command:**
```bash
tool logs <app> --since 5m --level warn
```
Look for memory growth warnings or OOM warnings.

**Third command (if sustained):**
```bash
tool deploy <app> --strategy recreate
```
Restart the container to release memory. (V2: auto-tune the cgroup limit based on history.)

**After:** If the app consistently hits the memory limit, raise the limit in `app.yaml` or fix the leak.

### 3.7 CpuHigh

**Symptom:** A container's CPU is > 95% for 5m.

**Blast radius:** The app is slow; response times may exceed the SLO.

**First command:**
```bash
tool status <app> --resources
```
Expected output: current CPU, memory, network, disk I/O.

**Second command:**
```bash
tool logs <app> --since 5m --level info
```
Look for signs of a runaway process (infinite loop, retry storm).

**Third command (if runaway):**
```bash
tool deploy <app> --strategy recreate
```

**After:** If the app consistently hits the CPU limit, raise the limit or add replicas.

### 3.8 DeployFailed

**Symptom:** A deploy transitions to `Failed`.

**Blast radius:** The new version is not serving; the previous version is still serving (bluegreen default).

**First command:**
```bash
tool deploy <deployment-id> --show
```
Expected output: the failure reason, the step that failed (build, push, start, healthcheck).

**Second command (if healthcheck failed):**
```bash
tool logs <app> --since 5m --level error
```
Look for the error.

**Third command (rollback, if needed):**
```bash
tool rollback <app> --confirm
```

**After:** If the failure is a pattern, add a policy rule (V2) or a CI check.

### 3.9 ErrorRateHigh

**Symptom:** The 5xx rate is > 5% over 5m.

**Blast radius:** Many users are affected; the SLO is at risk.

**First command:**
```bash
tool logs --since 5m --level error --tail 200
```
Look for a common error pattern.

**Second command:**
```bash
tool status --all --resources
```
Look for a server or app in `degraded` state.

**Third command (if a specific app is at fault):**
```bash
tool rollback <app> --confirm
```

**After:** File a postmortem within 24 hours.

### 3.10 LatencyHigh

**Symptom:** p99 response time > 1s over 5m.

**Blast radius:** The SLO is at risk; some users are affected.

**First command:**
```bash
tool status --all --resources
```
Look for a server or app with high CPU or memory.

**Second command:**
```bash
tool logs --since 5m --level warn --tail 100
```
Look for slow-query warnings or DB connection pool exhaustion.

**Third command (if the DB is at fault):**
```bash
tool db status <db-app> --pool
```

**After:** Add a query timeout, a connection pool size increase, or a replica.

---

## 4. The backup strategy

### 4.1 The 3-2-1-1-0 rule

- **3** copies of data
- **2** different storage media
- **1** offsite
- **1** immutable (e.g., S3 Object Lock)
- **0** errors on verification (monthly restore drill)

### 4.2 The V1 backup stack

| Component | Tool | Target |
|---|---|---|
| **Postgres** | `pg_dump --format=custom` + restic to S3 | Daily at 03:00, retain 30 |
| **SQLite** | Litestream (continuous WAL streaming) | Real-time, retain 7 |
| **App data volumes** | restic to S3 | Daily at 04:00, retain 30 |
| **Config** | git | Always |
| **Secrets** | age-encrypted, in SQLite | Always |
| **Audit log** | Append-only SQLite | Always |

### 4.3 The restore-drill command

The **single most important V1.2 command**:

```bash
sovereign backup verify --app postgres --restore-to scratch
```

What it does:
1. Downloads the latest backup from S3 to a scratch dir.
2. Creates a scratch DB (`sovereign_verify_<timestamp>`).
3. `pg_restore` into the scratch DB.
4. For each table, `SELECT COUNT(*)` and compare to source.
5. Drops the scratch DB.
6. Returns the diff (table → count_source, count_scratch, delta).

Output on success:
```text
✓ backup postgres: 1.2 GB, 47,221 rows
✓ verify: 12 / 12 tables matched
✓ restore-drill completed in 4m 12s
```

Output on failure:
```text
✗ backup postgres: 1.2 GB
✗ verify: 11 / 12 tables matched
✗ table "users" count differs: source=47221, scratch=0
✗ restore-drill FAILED
```

**Schedule:** Weekly, in cron, with an alert on failure.

**The GitLab 2017-01-31 lesson:** 8 months of "successful" backups, all empty, all useless. They recovered 40% of the data. **Backups you haven't restored are hopes.**

### 4.4 The Litestream config

```toml
# /etc/litestream.yml
dbs:
  - path: /var/lib/sovereign/sovereign.db
    replicas:
      - url: s3://sovereign-backups-eu-central-1/litestream
        access-key-id: ${LITESTREAM_ACCESS_KEY_ID}
        secret-access-key: ${LITESTREAM_SECRET_ACCESS_KEY}
        retention: 168h  # 7 days
        snapshot-interval: 24h
```

### 4.5 The retention policy

| Backup type | Retain |
|---|---|
| Postgres daily | 30 days |
| Postgres weekly | 12 weeks |
| Postgres monthly | 12 months |
| SQLite (Litestream) | 7 days |
| App data daily | 30 days |
| Audit log | Indefinitely (append-only) |

---

## 5. The disaster recovery

### 5.1 The RTO / RPO matrix

| Scenario | RTO | RPO | Test cadence |
|---|---|---|---|
| Single app crash | 30s (auto-restart) | 0 | Always on |
| Server host dies | 5 min (auto-failover in V2) | 0 (Litestream) | Quarterly |
| Datacenter dies | 1 hour (manual failover) | 5 min (S3 backup lag) | Annually |
| Vendor disappears | 1 day (fresh install + restore) | 0 (last backup) | Annually |
| Accidental `tool drop` | 1 hour (backup restore) | 5 min | Quarterly |
| Audit log corruption | Not recoverable (must prevent) | — | — |

### 5.2 The `sovereign recover` runbook (the 100-year-old question)

**Scenario:** Sovereign disappears. The binary is still available (Apache 2.0, on GitHub). The user has a backup. They need to restore on a fresh VM.

```bash
# 1. Install the binary on a fresh VM
curl -sSf sovereignruntime.dev/install.sh | sh

# 2. Stop the sovereign service
systemctl stop sovereign

# 3. Restore the SQLite backup
# (either from a sovereign export tarball, or from Litestream)
sovereign import platform --from <backup.tar.gz>
# OR
litestream restore -config /etc/litestream.yml /var/lib/sovereign/sovereign.db

# 4. Restart the service
systemctl start sovereign

# 5. Verify
sovereign status
sovereign apps list
sovereign audit --since 90d
```

### 5.2a The `sovereign import` playbook (migrating *into* Sovereign)

**Scenario:** The team is running on Heroku / Coolify / Dokploy / Render and wants to move to Sovereign (the #1 wedge in `user-pain-research.md` §2 — Heroku's Feb 2026 sustaining-engineering announcement, and the #2 wedge per `competitive-landscape.md` — teams that picked Coolify or Dokploy and hit the operator burden).

```bash
# 1. Get an export from the source platform
#    Heroku:  heroku apps:export -a <app>            # OR the platform's REST API
#    Coolify: coolify export <app> --format=json     # requires Coolify V4.0.18+
#    Dokploy: dokploy export <app> --format=json
#    Render: render services export <svc-id>         # requires Render API v1 token

# 2. Run the import
sovereign import --from heroku  --archive ./<app>-heroku.tar.gz --env prod
# OR
sovereign import --from coolify --archive ./<app>-coolify.json   --env prod
# OR
sovereign import --from dokploy --archive ./<app>-dokploy.json   --env prod
# OR
sovereign import --from render  --archive ./<app>-render.json    --env prod

# The command:
#   - detects the source (file extension + manifest)
#   - maps env vars, add-ons (DB / Redis → `sovereign db create` calls)
#   - generates app.yaml + Dockerfile (if missing) + Caddyfile entry
#   - writes a `--dry-run` plan first; applies on confirm
#   - logs every step to the audit table (`kind: "import.<source>"`)
#   - returns the deploy-ready app ID; `sovereign deploy <app>` ships it

# 3. DNS cutover (uses the new sovereign dns command)
sovereign dns provider add cloudflare --token <token>  # one-time per box
sovereign dns zone add example.com --provider cloudflare
sovereign dns record add api.example.com --type CNAME --target <box-hostname>

# 4. Verify
sovereign doctor --level standard  # the import may surface 2-3 fails (secrets, ports)
sovereign apps list
curl -vI https://api.example.com  # the new hostname is live

# 5. Decommission the old platform (operator action, not automated)
#     - Heroku:  heroku apps:destroy -a <app> --confirm <app>
#     - Coolify: coolify delete <app> --confirm
#     - Dokploy: dokploy delete <app> --confirm
#     - Render:  render services delete <svc-id> --confirm
```

**Gotchas** (from the design-partner beta feedback that drove the import feature):

- **Heroku add-ons** (Heroku Postgres, Heroku Redis, Heroku Kafka) need a one-time data export from the Heroku side *before* the import. The import command does not pull live data from Heroku's data services — it imports the *manifest* (env vars, buildpacks, add-on config) and the operator then runs a separate `heroku pg:pull` / `heroku redis:cli` to copy the data into the newly-provisioned Sovereign DB.
- **Coolify custom Docker Compose stacks** (Coolify's "Docker Compose" deployment type, not the "Native" type) are imported as a *service group* under one app; the operator has to verify the compose networking maps correctly to Sovereign's `services:` model.
- **Dokploy's pre-deploy / post-deploy hooks** are imported as `app.yaml` `hooks:` entries, but the hook shell scripts are not auto-translated — the operator must review and rewrite for the new box's filesystem.
- **Render's `render.yaml`** is converted to multiple `app.yaml` files (one per service in the blueprint); the import command writes a `render-import-plan.md` showing the mapping.

### 5.3 The vendor-disappear test (CI gate)

**A test that runs on every release:**

1. Spin up a fresh VM in CI.
2. Install the binary.
3. Restore from the last backup.
4. Verify all apps are running.
5. Verify the audit log is intact.
6. Verify TLS certs are valid.
7. Verify secrets are decrypted.

If any step fails, the release does not ship. This is the **founding promise**.

### 5.4 The backup-restore drill schedule

| Drill | Frequency | Who runs it | Pass criteria |
|---|---|---|---|
| Postgres restore-drill | Weekly (cron) | Automated | 0 row count diffs |
| SQLite restore-drill | Daily (Litestream) | Automated | last snapshot is < 24h old |
| App data restore-drill | Monthly | Operator | 0 file count diffs |
| Full platform restore | Annually | Operator + 2nd person | All apps serving traffic |
| Vendor-disappear test | On every release | CI | All 7 steps pass |

---

## 6. The observability stack

### 6.1 The locked stack (V1.2+)

| Component | Tool | Reason |
|---|---|---|
| **Metrics** | VictoriaMetrics (single-node) | 4x lower memory than Prometheus |
| **Logs** | VictoriaLogs | 70% lower memory than Loki |
| **Dashboards** | Grafana | Standard |
| **Alerting** | vmalert | Standard |
| **Tracing** | OpenTelemetry SDK (opt-in) | Standard |
| **OTel collector** | OpenTelemetry Collector (Docker) | Standard |

**Each is a single binary, < 200 MB total RAM.**

### 6.2 Out of scope (explicitly rejected)

- **Embedded Prometheus/Grafana/Loki** in the sovereign binary.
- **OTel collector in the same process** as the sovereign binary.
- **Elasticsearch** (too memory-heavy, too complex).

### 6.3 The exposed surface

- `GET /metrics` on the control plane port — Prometheus text format.
- Structured JSON logs to stdout and `/var/log/sovereign/*.log`.
- OTel SDK as opt-in feature flag (`--features otel`).
- **Centralized log shipping** (G22) — `sovereign log ship` configures JSON log export to one of 7 sinks (Loki, Datadog, Better Stack, Splunk HEC, Sumo Logic, syslog, Vector). The full runbook is in §6.5.

### 6.4 The mandatory metrics

```text
# Counters
deploys_total{app, env, status}
rollback_total{app, reason}
audit_events_total{kind}
backup_total{target, status}

# Histograms
deploy_duration_seconds{app, env}
backup_duration_seconds{target}
http_request_duration_seconds{path, method, status}

# Gauges
container_cpu_usage{app, server}
container_memory_usage{app, server}
backup_size_bytes{target}
risk_score{app, env}                    # V2+
policy_evaluations_total{decision, rule} # V2+

# Info
build_info{version, commit, target}
sovereignty_test_result{point}           # V2+
```

### 6.5 The centralized log shipping runbook (G22 — the SIEM / SOC 2 / ISO 27001 story)

**The why:** SOC 2 CC7.2 and ISO 27001 A.8.15 require centralized log collection. The 1-person team uses Datadog free tier / Better Stack free tier / self-hosted Loki; the MNC uses Splunk / Datadog / a SIEM. The binary emits structured JSON (§6.3) and ships to a configurable destination.

**The setup (3 minutes):**

```bash
$ sovereign log ship
? Where do you want to send logs?
  > Loki (self-hosted)
    Datadog (SaaS)
    Better Stack (SaaS)
    Splunk / HEC (on-prem)
    Sumo Logic (SaaS)
    Syslog (RFC 5424) — for SIEM forwarding
    Vector pipeline (BYO config)
    Disable shipping
? Endpoint: https://logs.betterstack.com/...
? Source token: <paste>
✓ Log shipping configured. Last 5 lines shipped OK.
? Enable the redactor? (strips AWS keys, GitHub tokens, JWTs, private keys, age secret keys, the binary's master key fingerprint) [Y/n] y
✓ Redactor enabled.
? Enable the doctor health check? (Warns if the sink is > 1h silent or the queue > 10 MB) [Y/n] y
✓ Health check enabled.
```

**The 7 sinks (decision matrix):**

| Sink | When to use | Cost | EU-only option | Sample config |
|---|---|---|---|---|
| **Loki** (self-hosted) | You already run Grafana / Loki / Mimir | Free (your VPS cost) | Yes (self-host in EU region) | `https://loki.mirasaas.com/loki/api/v1/push` + basic auth |
| **Datadog** (SaaS) | You want a SaaS with first-class metrics + logs + APM in one pane | $0.10/GB ingested (15-day retention free) | Yes (`datadoghq.eu` site for EU) | `https://http-intake.logs.datadoghq.com/api/v2/logs` + `DD-API-KEY` header |
| **Better Stack** (SaaS) | Cheaper than Datadog, EU-based, modern UI | $0.25/GB ingested (30-day retention free) | Yes (EU region) | `https://in.logs.betterstack.com/` + source token |
| **Splunk HEC** (on-prem) | You are an MNC with an existing Splunk deployment | Per Splunk license (existing) | Yes (your Splunk is in your DC) | `https://splunk.mnc.com:8088/services/collector/event` + HEC token |
| **Sumo Logic** (SaaS) | You have a Sumo Logic account from another tool | Per Sumo license (existing) | Yes (EU region) | `https://endpoint.sumologic.com/receiver/v1/http/...` |
| **Syslog (RFC 5424)** | You forward to a SIEM (QRadar, ArcSight, Elastic) that accepts syslog | Free (your SIEM cost) | Yes (your SIEM is in your DC) | `siem.mnc.com:601` (TCP+TLS) |
| **Vector pipeline** (BYO) | You have a Vector pipeline already | Free (your VPS cost) | Yes (your Vector is in your region) | `unix:///var/run/vector.sock` |

**The day-2 runbook:**

- **Check the shipper health:** `sovereign log ship status` — shows the sink type, last successful ship, last error, queue depth.
- **Test the sink:** `sovereign log ship --test` — sends a synthetic test event and reports the HTTP status code. Use this after the initial config and after any network change.
- **View the queue:** `sovereign log ship status --queue` — lists the events waiting to be flushed (1 MB or 1000 events, whichever first; flushed every 5s; flushed on SIGTERM).
- **Disable temporarily:** `sovereign log ship disable` (e.g. for maintenance; re-enable with `sovereign log ship enable`).
- **Change the sink:** `sovereign log ship reconfigure` — the interactive prompt again, but it preserves the redactor and doctor config.
- **View the redactor rules:** `sovereign log ship --show-redactor` — lists the 6 secret patterns being stripped.
- **Verify the redactor:** `sovereign log ship --test-redactor` — sends a synthetic log line containing a known AWS key, verifies the line that hits the sink has `***REDACTED***` in the right field.

**The doctor integration:**

The 2 standard-level checks (G22 §3.15) are wired into the doctor:

- `log_ship_configured` — `Warn` if the operator has the Pro or Enterprise license and no log ship sink is set. The check looks at `/etc/sovereign/license.json` and `/etc/sovereign/log-ship.toml`.
- `log_ship_healthy` — `Fail` if the sink is set but the last successful ship was > 1h ago, or if the queue depth is > 10 MB.

The morning doctor output (operations-runbook §1) includes a one-liner:

```text
✓ Log shipping: 12,341 events shipped to Loki (last: 2s ago, queue: 0, redactor: 6 patterns)
```

**The 3am playbook for log shipping:**

- **Sink returns 401/403:** the token rotated. The binary auto-disables the sink and writes a `log_sink_disabled` audit event. Re-add the token: `sovereign log ship reconfigure`. The new config is hot-reloaded.
- **Sink returns 503 with Retry-After:** the binary backs off exponentially (1s, 2s, 4s, ..., 60s). The queue grows. The doctor `log_ship_healthy` will warn when the queue > 10 MB. If the sink is down for > 1h, the doctor check fires Sev1; follow operations-runbook §1.1.
- **Network is partitioned (binary can reach the box, can't reach the sink):** the queue grows to 10 MB, the doctor fires, the operator checks the network. The binary does not drop events (the queue is bounded; events block the producer when full).
- **Sink rotates the schema (e.g. Datadog deprecates an API version):** the binary logs a `sink_api_version_mismatch` event and continues shipping in the legacy format. The customer has 90 days to upgrade the binary before the legacy endpoint is removed.

**The data residency note:**

The default config has **no third-party sub-processors except the binary delivery and the docs site** (both EU-only). The customer opts in to Datadog / Splunk / Better Stack / etc. via `sovereign log ship`. The DPA template (enterprise-readiness.md §5) requires the customer to consent to any non-EU sub-processor. The sub-processor list is at `/docs/legal/sub-processors.md` and is updated 30 days before any change.

---

## 7. The capacity planning

### 7.1 The right-sizing formula

| Apps | CPU | RAM | Disk | VPS |
|---|---|---|---|---|
| 1-5 | 2 vCPU | 4 GB | 40 GB | Hetzner CX22 €4.49/mo |
| 5-15 | 4 vCPU | 8 GB | 80 GB | Hetzner CX32 €8.59/mo |
| 15-30 | 8 vCPU | 16 GB | 160 GB | Hetzner CPX41 €15/mo |
| 30-50 | dedicated | 32 GB | 320 GB | Hetzner AX41-NVMe €45/mo |
| 50+ | multi-server | 64+ GB | 1+ TB | Multiple boxes, rqlite HA |

### 7.2 The right-sizing signal

- **p99 latency > 200ms consistently at < 50% CPU** → scale up (more CPU/RAM per node), not out (more nodes).
- **p99 latency > 200ms at > 80% CPU** → scale out (more nodes), not up.
- **Disk > 70%** → add disk before it becomes an incident.

### 7.3 The Hetzner-specific notes

- **Snapshots** cost €0.012/GB/month. Take weekly snapshots for free DR.
- **Backups** cost 20% of the server price. Cheaper than snapshots for full-server recovery.
- **Volumes** are detached storage; cheaper than server disk for cold data.
- **Load balancers** are €4.49/month. Use them for multi-server V1.5+ setups.

---

## 8. The patching

### 8.1 The OS patching

- **Unattended-upgrades** enabled by default (Debian/Ubuntu).
- Reboot window: **4:30 AM** (operator's time).
- Critical kernel updates: applied immediately, reboot within 24h.

### 8.2 The container image patching

- **Trivy** in CI for image scanning.
- `--exit-code 1` on HIGH or CRITICAL CVEs.
- **Renovate** (or Dependabot) for base image updates.

### 8.3 The sovereign update

- `sovereign update` checks for new versions.
- Downloads the release tarball.
- Verifies the cosign signature.
- Verifies the SBOM.
- Applies the update (with `--confirm` for major versions).
- Restarts the service.
- Verifies the service is healthy.

---

## 9. The configuration management

### 9.1 The "no Ansible, no Chef, no Puppet" rule

- **Single binary + single config file** philosophy.
- Bootstrap: `curl -sSf sovereignruntime.dev/install.sh | sh` does the whole thing.
- After install: edit `/etc/sovereign/sovereign.toml`, `systemctl restart sovereign`.
- For fleet ops (V1.5+): `tool server add <host>` bootstraps agents via SSH.

### 9.2 The config file

```toml
# /etc/sovereign/sovereign.toml
control_plane_url = "http://127.0.0.1:7878"
data_dir = "/var/lib/sovereign"
log_dir = "/var/log/sovereign"

[state]
backend = "sqlite"  # or "rqlite" (V2)
path = "/var/lib/sovereign/sovereign.db"

[runtime]
adapter = "docker"  # or "podman" (V1.5)

[proxy]
adapter = "caddy"  # or "nginx" (V1.1)
admin_url = "http://127.0.0.1:2019"

[secrets]
backend = "age"
master_key_path = "/var/lib/sovereign/master.key.age"

[backup]
s3_bucket = "s3://sovereign-backups-eu-central-1"
s3_region = "eu-central-1"
retention_days = 30

[notify]
channels = ["telegram", "email"]  # configurable

[http]
bind = "127.0.0.1:7878"
tls_cert = "/etc/sovereign/tls/cert.pem"
tls_key = "/etc/sovereign/tls/key.pem"
```

---

## 10. The on-call

### 10.1 The rotation (V0/V1: founder; V1.5: first 3 engineers)

- **1 person, 24/7, 7 days.** No rotation.
- **Alerts go to humans via Telegram**, not email.
- **Every alert has a runbook** (in git, not wiki).
- **15-minute response window**; if no response, status page auto-flips to "investigating."

### 10.2 The "junior dev" test

If a junior dev gets paged because you're on vacation, can they resolve it? Self-service runbooks, `--explain` flags, diagnostic commands, escalation paths. **If not, the on-call system is broken, not the junior dev.**

### 10.3 The quarterly "vacation test"

- Operator takes 1 week off.
- On-call is automated (alerts → backup on-call).
- Manual backup is the freelancer (or the second engineer).
- After: review the alerts, the runbooks, the resolution time.

### 10.4 The "5-7 tool ceiling"

Every ops engineer uses 20+ tools. Aim for the 5-7 that earn their place:

1. **sovereign** (this product) — deploy, ops, secrets, backups
2. **Caddy** (or Nginx) — reverse proxy, TLS
3. **VictoriaMetrics + VictoriaLogs + Grafana** — observability
4. **restic** (or pgBackRest) — backups
5. **age + SOPS** — secrets
6. **trivy** — image scanning
7. **Ansible** (for fleet ops, optional)

That's 7. The product's job is to be the *primary*, not the *only one*.

---

## 11. The "postmortem-less" weekly review

Every Friday at 5 PM, run `sovereign weekly-report`. It auto-generates:

- Deploys this week (count, success rate, rollback rate)
- Top 5 errors (by frequency)
- Top 5 slowest endpoints (p99)
- Top 5 most-deployed apps
- MTTR (mean time to recover)
- MTTD (mean time to detect)
- Cost (S3 storage, backup retention, server spend)
- Compliance status (sovereignty test, backup verify rate)

The output is a single markdown file, ready to paste into the weekly Slack channel. No postmortem; just a review.

---

## 12. The doctor & the alert system (the diagnostic → alerting → incident loop)

The 10 alerts in §3 are **reactive** — they fire when a metric crosses a threshold. **`sovereign doctor` is proactive** — it surfaces issues *before* they cross the threshold, with a fix path. The two systems are designed to be one loop, not two tools.

### 12.1 Every alert has a doctor check

| Alert | Doctor check that catches it first | Doctor level |
|---|---|---|
| HealthCheckFail | `runtime.docker_run`, `proxy.caddy_config_valid` | basic |
| DiskFull | `system.disk` | basic |
| CertExpiring | `proxy.caddy_config_valid`, `network.caddy_ocsp_stapled` | basic / full |
| BackupFailure | `backup.backup_recent`, `backup.backup_offsite` | standard |
| ServerUnreachable | `agents.agent_heartbeat_fresh` | standard |
| MemoryHigh | `performance.memory_pressure_low` | standard |
| CpuHigh | `performance.cpu_steal_low` | standard |
| DeployFailed | `runtime.docker_pull` (the new image failed to pull) | basic |
| ErrorRateHigh | `observability.slo_burn_rate` | full |
| LatencyHigh | `performance.tls_handshake_fast` | standard |

**Rule:** the operator's morning routine (§1) catches the issue *before* the alert fires. The alert fires only when the doctor was either not run, or the issue is too fast for the doctor's 30s cycle.

### 12.2 The alert → doctor handoff

When an alert pages the on-call, the on-call's first command is **never** "open the alert dashboard." It's `sovereign doctor --level standard` followed by `sovereign doctor --explain <check>`. The doctor is the entry point; the alert is the trigger.

```bash
# 1. Pager fires
sovereign doctor --level standard

# 2. Find the failing check
#   ✗ backup_recent: last backup was 27h ago

# 3. Read the KB
sovereign doctor --explain backup_recent
#   → "Why your last backup is overdue"
#   → Runbook: docs/kb/doctor/backup_recent.md
#   → Fix: sovereign backup run --app postgres

# 4. Apply the fix
sovereign doctor --fix=backup_run_now

# 5. Verify
sovereign doctor --level standard

# 6. Declare an incident (V1.5+)
sovereign incident declare --title "Backup overdue" --severity sev2

# 7. Resolve and postmortem
sovereign incident resolve inc-<id> --summary "..." --postmortem /tmp/pm.md
```

### 12.3 Doctor is the source of truth, alerts are the signal

If `sovereign doctor` says `Pass` but the alert is still firing, the alert is misconfigured. Fix the alert. If `sovereign doctor` says `Fail` but no alert has fired, the doctor caught something the alert missed. Add a Prometheus rule for the doctor's metric. The doctor's `sovereign_doctor_check_duration_seconds{category,name}` and the `sovereign_doctor_last_status{level}` gauges are the alerting substrate.

### 12.4 Doctor runs in cron as a tripwire

`/etc/cron.d/sovereign-doctor` runs `sovereign doctor --level standard` every 5 minutes. The exit code is the alert: 0 = no page, 1 = log + ticket, 2 = page. The output is appended to `/var/log/sovereign/doctor-cron.log`. The on-call never has to remember to run it; the system runs it for them.

The full doctor spec (5 levels, 14 categories, 4 subcommands, the incident toolkit, the retro and game-day workflows) is in [`doctor.md`](./doctor.md).

---

**Next: read [`doctor.md`](./doctor.md) for the full diagnostic engine spec, then [`product-ux.md`](./product-ux.md) for the CLI/TUI design and pricing.**
