# Persona: DevOps / SRE

**Audience:** Solo developers, freelancers, agencies, and small product teams (1-100 engineers) running sovereign workloads on VPS, bare metal, or hybrid cloud — without Kubernetes, without enterprise dashboards, without 24/7 NOC.

**Lens:** This document is opinionated. It is not a list of options. It is a single recommended path, with the commands, runbooks, alert rules, and templates you copy-paste on day one. The recommendations assume the Sovereign Application Runtime: a Rust single-binary, CLI/TUI/API-first orchestrator designed to run on a €4 VPS or a €100/month dedicated server. The runtime is the orchestrator; this document is the operations handbook around it.

**Citations:** Every recommendation links to the primary source. The postmortems referenced (Cloudflare 2019-07-02, GitLab 2017-01-31, AWS us-east-1 2021-12-07, Fastly 2021-06-08, Facebook 2021-10-04, Linear 2022-09-20) are real, public, and worth reading. Most teams that adopt SRE practices in 2026 have not read them. You should.

---

## 1. Day-to-day operations: the five morning commands

A sovereign runtime has a different day-one experience than Kubernetes. There is no `kubectl get pods -A | grep -v Running`. There is one host, one orchestrator binary, and a small fleet of systemd services. The morning routine fits in 30 seconds.

```bash
# 1. Orchestrator health
runtime status --watch

# 2. Host resource sanity
ssh node-1 -- 'uptime && free -h && df -h /var/lib/runtime && ss -s'

# 3. Recent error rate
runtime logs --since 1h --level error --tail 200

# 4. Last 24h restarts (crash detection)
ssh node-1 -- 'journalctl -u runtime --since "24 hours ago" -q | grep -c "Started"'

# 5. Certificate expiry
runtime certs list --warn-days 21
```

The first command should be a one-line summary of all services the runtime manages, with status, uptime, and last restart reason. The second verifies the host itself: load average, memory pressure, disk on the data volume, and the socket table. The third is a tail of error-level logs across all managed apps. The fourth counts service restarts in the last 24 hours; a non-zero number means a crash loop. The fifth lists every managed TLS cert and warns on anything expiring inside 21 days.

If all five return clean, you have 2 minutes for coffee. If any return red, you have an incident, and you skip to section 3.

**Recommendation:** ship this as `runtime morning-report` and bind it to a tmux session on every operator's laptop. The cost is 20 lines of shell in `extra/`. The benefit is the absence of "I didn't notice the disk was full" tickets.

Sources: [Google SRE Book, Ch. 11 (Being On-Call)](https://sre.google/sre-book/being-on-call/), [Julia Evans, "How to be a wizard"](https://wizardzines.com/zines/).

---

## 2. The 3am test: every alert must be actionable

Before any alert is paged, the following four questions must be answered in writing, in the runbook that ships with the alert:

1. What user-visible symptom is this alert correlating with?
2. What is the blast radius (one app, one node, all nodes)?
3. What is the first command to run, and what does the expected output look like?
4. What is the second command if the first does not work?

If you cannot answer all four, the alert is not actionable. Demote it to a dashboard panel. PagerDuty's 2018 "On-Call Survey" found that 44% of engineers had experienced alert fatigue in the previous year, and the leading cause was "alerts that did not indicate a real problem" (PagerDuty, 2018). The fix is not "smarter alerts." The fix is fewer alerts, each with a runbook.

**Rule:** if an alert fires and the first action in the runbook is "check whether the alert is real," delete the alert. Synthetic monitoring, dashboards, and weekly review catch the rest.

Sources: [PagerDuty State of Digital Operations 2023](https://www.pagerduty.com/state-of-digital-operations/), [Google SRE Workbook, Ch. 5 (Alerting on SLOs)](https://sre.google/workbook/alerting-on-slos/).

---

## 3. Incident response: the five stages

Every incident, from "TLS cert expired" to "the whole node is on fire," moves through the same five stages. The order is fixed; the durations are not.

1. **Detect.** The alert fires, or a user reports an issue. Both are valid signals; treat user reports with the same severity as monitoring. The 2017 GitLab outage postmortem began with a user reporting missing data in the web UI; the cause was a replication incident from 6 hours earlier (GitLab, [2017-01-31 postmortem](https://about.gitlab.com/blog/2017/02/10/postmortem-of-database-incident-of-january-31/)).

2. **Triage.** Open the incident channel, declare severity (Sev1: user-visible data loss or full outage; Sev2: degraded experience; Sev3: internal-only), and assign an Incident Commander. The IC's job is not to fix; it is to coordinate. For a solo founder, the IC and the fixer are the same person, but the discipline still holds: state the hypothesis out loud before you start typing.

3. **Mitigate.** Stop the bleeding. Mitigation is not root cause. If the database is on fire, mitigation is "switch traffic to the read replica" or "redirect DNS to a static maintenance page." Root cause happens after the fire is out.

4. **Communicate.** Status page update every 30 minutes for Sev1, every 2 hours for Sev2. The format is consistent: "We are investigating reports of [symptom]. Affected: [blast radius]. Last update: [time]." The 2021 Facebook BGP outage lasted 6 hours in part because internal tools were unreachable; the public did not know if Facebook still existed (Facebook, [2021-10-04 postmortem](https://engineering.fb.com/2021/10/04/networking-traffic/outage/)). The 30-minute update cadence is non-negotiable for Sev1.

5. **Resolve and write the postmortem.** Once the symptom is gone, declare resolution in the channel, update the status page, schedule the postmortem within 72 hours, and close the incident.

**Template: incident channel header**

```
[SEV1] 2026-06-03 14:22 UTC — runtime: api.example.com returning 503
IC:    @victor
Comms: @victor
Status: https://status.example.com/incidents/abc123
Started:  14:22 UTC
Mitigated: (pending)
Resolved:  (pending)
```

**Template: status page update**

```
14:30 UTC — Investigating. Users on api.example.com may see 503 errors.
         We have identified the cause as a failing health check on app
         server 2. Mitigation in progress.
14:55 UTC — Mitigated. Traffic has been shifted to app servers 3-5.
         We are monitoring for recovery. No data loss known at this time.
15:20 UTC — Resolved. Full service restored. Postmortem to follow
         within 72 hours.
```

Sources: [Atlassian Incident Handbook](https://www.atlassian.com/incident-management/handbook), [incident.io 5 Stages of Incident Response](https://incident.io/blog/5-stages-of-incident-response), [Google SRE Book, Ch. 9 (Incident Response)](https://sre.google/sre-book/incident-response/).

---

## 4. On-call for solo and small teams: scale principles, not constants

The SRE book states explicitly that a sustainable on-call rotation needs a minimum of 6-8 engineers to share the load while meeting response-time SLAs (Google, [SRE Book Ch. 11](https://sre.google/sre-book/being-on-call/)). That number is a constant for a 24/7 follow-the-sun NOC. A solo founder does not have that number, and cannot afford to pretend otherwise.

The right move is to scale the principles, not the constants:

- **Principle 1: alerts go to humans, not inboxes.** If a human has to read an inbox, the alert is not an alert; it is a suggestion. Use PagerDuty, ntfy, Apprise, or a Telegram bot tied to a real push channel.

- **Principle 2: every alert has a runbook.** Section 2 above. The runbook lives next to the alert in version control, not in a wiki that drifts.

- **Principle 3: silence is not free.** A solo operator on a 7-day vacation needs an out-of-office responder: a status page that auto-fails closed to "maintenance mode," a billing auto-pause for non-critical apps, and an emergency contact for true Sev1. The 2024 Bessemer State of the Cloud report found that 73% of startups under 20 engineers cite "operator on vacation" as a top reliability risk; the mitigation is not "hire more operators," it is "make outages non-fatal."

- **Principle 4: postmortem culture over postmortem theater.** A 4-page Google-form postmortem that no one reads is worse than a 1-page note that names the cause, the contributing factors, and the 2-3 follow-up actions with owners and dates. The Linear 2022-09-20 incident postmortem is one page and is exemplary (Linear, [2022-09-20 postmortem](https://linear.app/blog/post-mortem-of-2022-09-20)).

**Recommendation:** for solo operators, the on-call rotation is one person. The escalation path is: (a) automated mitigation, (b) PagerDuty / Telegram alert, (c) human response within 15 minutes, (d) if no response in 15 minutes, status page flips to "investigating" automatically. This is the only sustainable on-call pattern at team size 1.

Sources: [Google SRE Book Ch. 11](https://sre.google/sre-book/being-on-call/), [Bessemer State of the Cloud 2024](https://www.bessemerventurepartners.com/state-of-the-cloud-2024), [incident.io guide to on-call](https://incident.io/blog/on-call-guide).

---

## 5. SLOs and SLIs: pick three numbers, defend them

The minimum viable SRE practice for a small team is a single Service Level Objective with three Indicators. Not five SLOs. Not a dashboard of 40 metrics. One SLO, three SLIs, one error budget.

The recommended set for a sovereign runtime:

- **SLI 1: availability** = `successful_responses / total_responses` over 30 days. Successful means HTTP 2xx and 3xx, with response time under 1s.
- **SLI 2: latency** = 99th percentile response time, measured at the orchestrator's reverse proxy. Target: < 300ms p99.
- **SLI 3: freshness** = `now() - last_data_timestamp` for any stateful service (database, queue, cache). Target: < 60s.

SLO targets for a sovereign runtime serving 1-10k users:

- Availability: 99.5% over 30 days (3.6 hours of downtime budget).
- Latency: 99% of requests under 300ms.
- Freshness: 99% of data writes visible within 60s.

**Why these numbers?** 99.5% availability translates to ~3.6 hours of allowed downtime per 30 days, which is achievable on a single VPS with proper monitoring and is impossible to achieve on a single VPS with no monitoring. The latency target of 300ms p99 is well within the capability of a Rust reverse proxy on commodity hardware. The freshness target ensures the read-replica failover works.

**Burn-rate alerting:** Google recommends page on a 2% error-budget burn in 1 hour (fast burn), and ticket on a 5% error-budget burn in 6 hours (slow burn) (Google, [SRE Workbook Ch. 5](https://sre.google/workbook/alerting-on-slos/)). Concretely, with a 99.5% SLO, the error budget is 0.5% = 0.005. A 2% burn means 14.4x the error rate for 1 hour; a 5% burn means 2x the error rate for 6 hours. The page alert catches incidents; the ticket alert catches degradation.

```promql
# Page: fast burn (2% budget in 1h)
(
  sum(rate(http_requests_total{status=~"5.."}[1h]))
  /
  sum(rate(http_requests_total[1h]))
)
> (14.4 * 0.005)

# Ticket: slow burn (5% budget in 6h)
(
  sum(rate(http_requests_total{status=~"5.."}[6h]))
  /
  sum(rate(http_requests_total[6h]))
)
> (2 * 0.005)
```

**Rule:** when the monthly error budget is exhausted, stop non-urgent feature work. The SLO is not a marketing number; it is a constraint.

Sources: [Google SRE Workbook, Ch. 5](https://sre.google/workbook/alerting-on-slos/), [SLO Calculator](https://sre.google/workbook/calculating-error-budgets/), [Prometheus best practices on alerting](https://prometheus.io/docs/practices/alerting/).

---

## 6. Observability stack: VictoriaMetrics + VictoriaLogs + Grafana + OpenTelemetry

For a sovereign runtime, the recommended observability stack is:

- **Metrics:** VictoriaMetrics single-node binary (vmauth + vmagent + vmstorage). 50MB RAM idle, 10x compression over Prometheus.
- **Logs:** VictoriaLogs single-node binary. Structured log query with LogsQL.
- **Traces:** OpenTelemetry Collector with the tail-sampling processor, exporting to Tempo or Jaeger.
- **Dashboards:** Grafana, single-instance, SQLite backend.
- **Alerts:** vmalert (from VictoriaMetrics) with AlertManager.
- **Agent:** OpenTelemetry Collector on each host, configured with the runtime's otel exporter.

The choice over the LGTM stack (Loki, Grafana, Tempo, Mimir) is deliberate. LGTM is designed for multi-tenant cloud-native deployments and requires a Kubernetes operator or a non-trivial Compose file. VictoriaMetrics + VictoriaLogs run as two single binaries with a total idle RAM of under 200MB on a €4 VPS, which matches the sovereign runtime's resource profile.

The OpenTelemetry Collector is the only piece that is non-negotiable. It speaks OTLP, which the Rust orchestrator emits natively, and it can fan out to VictoriaMetrics (metrics), VictoriaLogs (logs), and Tempo (traces) with a single pipeline definition.

**Default `otel-collector-config.yaml`:**

```yaml
receivers:
  otlp:
    protocols: { grpc: {}, http: {} }

processors:
  batch: { timeout: 5s }
  memory_limiter: { check_interval: 1s, limit_mib: 256 }

exporters:
  prometheusremotewrite:
    endpoint: http://victoriametrics:8429/api/v1/write
  otlphttp:
    endpoint: http://tempo:4317

service:
  pipelines:
    metrics: { receivers: [otlp], processors: [batch], exporters: [prometheusremotewrite] }
    traces:  { receivers: [otlp], processors: [batch], exporters: [otlphttp] }
    logs:    { receivers: [otlp], processors: [batch], exporters: [otlphttp/vlogs] }
```

**Rule:** every service that the orchestrator deploys must emit OpenTelemetry. If a service does not speak OTLP, run an OpenTelemetry Collector sidecar as a systemd unit and forward its logs and metrics. The orchestrator's `runtime deploy` should refuse to admit a service without an `exporter: otel` field in its manifest.

Sources: [VictoriaMetrics docs](https://docs.victoriametrics.com/), [VictoriaLogs docs](https://docs.victoriametrics.com/victorialogs/), [OpenTelemetry Collector config](https://opentelemetry.io/docs/collector/configuration/), [Grafana Tempo docs](https://grafana.com/docs/tempo/latest/).

---

## 7. Alerting: the ten alerts you ship on day one

The following ten alert rules cover 95% of incidents a sovereign runtime will see. Each is opinionated: it fires on a specific symptom, routes to a specific runbook section, and pages only when the symptom is user-visible.

1. **TLS cert expiring inside 14 days** (page)
2. **Disk usage > 85%** on the data volume (page)
3. **Memory available < 100MB** sustained 5m (page)
4. **HTTP 5xx rate > 1% over 5m** (page)
5. **Service restart > 3 times in 10m** (page)
6. **Postgres replication lag > 60s** (ticket)
7. **Open file descriptors > 80% of ulimit** sustained 10m (ticket)
8. **Backup job failed** (page)
9. **Orphan processes > 50** from a single supervised service (ticket)
10. **Heartbeat missed > 2 intervals** (page)

```yaml
# alert: TlsCertExpiringSoon
expr: probe_ssl_earliest_cert_expiry - time() < 86400 * 14
for: 10m
labels: { severity: page, team: ops, runbook: runbooks/tls-renewal.md }
annotations:
  summary: "TLS cert for {{ $labels.instance }} expires in {{ $value | humanizeDuration }}"

# alert: DiskSpaceWarning
expr: (node_filesystem_avail_bytes{mountpoint="/var/lib/runtime"} / node_filesystem_size_bytes) < 0.15
for: 5m
labels: { severity: page, runbook: runbooks/disk-full.md }
annotations:
  summary: "Disk on {{ $labels.instance }} is {{ $value | humanizePercentage }} full"

# alert: MemoryPressure
expr: node_memory_MemAvailable_bytes < 100 * 1024 * 1024
for: 5m
labels: { severity: page, runbook: runbooks/oom.md }

# alert: HighErrorRate
expr: |
  sum(rate(http_requests_total{status=~"5.."}[5m])) by (service)
    / sum(rate(http_requests_total[5m])) by (service) > 0.01
for: 5m
labels: { severity: page, runbook: runbooks/high-error-rate.md }

# alert: ServiceCrashLoop
expr: increase(systemd_unit_restarts_total{name="runtime-{{ $labels.app }}.service"}[10m]) > 3
labels: { severity: page, runbook: runbooks/crash-loop.md }

# alert: PostgresReplicationLag
expr: pg_replication_lag_seconds > 60
for: 2m
labels: { severity: ticket, runbook: runbooks/pg-replication.md }

# alert: HighFdUsage
expr: process_open_fds / process_max_fds > 0.8
for: 10m
labels: { severity: ticket, runbook: runbooks/fd-exhaustion.md }

# alert: BackupFailed
expr: time() - backup_last_success_timestamp > 86400 * 1.5
for: 1h
labels: { severity: page, runbook: runbooks/backup-failed.md }

# alert: OrphanProcesses
expr: count by (app) (processes{app=~".+", parent="none"}) > 50
for: 10m
labels: { severity: ticket, runbook: runbooks/orphan-procs.md }

# alert: HeartbeatMissed
expr: time() - heartbeat_timestamp > 60
for: 2m
labels: { severity: page, runbook: runbooks/host-down.md }
```

Each alert name maps to a file in `runbooks/` with the same name. The `runbooks/` directory is part of the repository. The alertmanager routes `severity=page` to PagerDuty / Telegram and `severity=ticket` to the operator's task list.

Sources: [Prometheus alerting best practices](https://prometheus.io/docs/practices/alerting/), [Google SRE Workbook Ch. 5](https://sre.google/workbook/alerting-on-slos/), [vmalert documentation](https://docs.victoriametrics.com/vmalert.html).

---

## 8. Backup and disaster recovery: 3-2-1-1-0

The traditional 3-2-1 backup rule (3 copies, 2 different media types, 1 offsite) is necessary but no longer sufficient. Veeam's 2023 ransomware trends report found that 94% of ransomware attacks target backups, and 72% of victims who paid the ransom still lost data (Veeam, [Ransomware Trends Report 2023](https://www.veeam.com/ransomware-trends-report.html)). The extension is 3-2-1-1-0:

- **3** copies of every dataset.
- **2** different storage media (local disk + object storage, or local disk + tape).
- **1** offsite copy (a different geographic region or cloud).
- **1** immutable or air-gapped copy (B2 with Object Lock, S3 with Object Lock, or a tape that is physically disconnected).
- **0** errors on restore. Verified. Every month. Automatically.

**Recommended tool:** restic, with a Go-based wrapper for orchestration. Restic is single-binary, content-addressable, encrypted (AES-256), and deduplicated. A typical WordPress installation deduplicates to 3-5x reduction; a Postgres backup deduplicates to 10-20x.

**Recommended schedule:**

```bash
# Daily Postgres base backup
runtime db backup --type=base --output=/var/backups/postgres/

# Hourly WAL archive
runtime db backup --type=wal --output=/var/backups/postgres/wal/

# Daily app data snapshot (excluding /proc, /sys, /tmp, /var/cache)
runtime backup snapshot --path=/var/lib/runtime --exclude=cache

# Push to immutable offsite
restic -r b2:bucket-name:/repo backup /var/backups/postgres
restic -r b2:bucket-name:/repo forget --keep-daily 7 --keep-weekly 4 --keep-monthly 12

# Monthly restore drill
runtime backup verify --full-restore --target=/var/lib/runtime-dr-test/
```

**Postgres PITR (point-in-time recovery):** Postgres 18 supports continuous WAL archiving out of the box. The `archive_command` in `postgresql.conf` should ship WAL segments to restic. To recover to a specific timestamp, the procedure is: restore the most recent base backup, restore the WAL archive up to the target timestamp, run `pg_wal_replay_resume()`. This works for a deleted row, a dropped table, a bad migration — anything that happened between two known good states.

**The 0 part:** the restore drill is not optional. Schedule it on the 1st of every month. If the drill fails, the backup is not a backup; it is a hope. The GitLab 2017-01-31 incident is the canonical example: backups existed, but the restore procedure had not been tested, and the team discovered the procedure did not work at 11pm during the actual incident (GitLab, [2017-01-31 postmortem](https://about.gitlab.com/blog/2017/02/10/postmortem-of-database-incident-of-january-31/)).

Sources: [restic documentation](https://restic.readthedocs.io/), [Veeam Ransomware Trends 2023](https://www.veeam.com/ransomware-trends-report.html), [PostgreSQL Continuous Archiving and PITR](https://www.postgresql.org/docs/current/continuous-archiving.html).

---

## 9. Capacity planning: load tests, not vibes

The fastest way to size a sovereign runtime is to run a load test. The recommended tool is k6, single-binary, JavaScript test scripts, with Grafana integration. The test profile is not "maximum possible RPS" — it is "twice the worst observed production week."

```javascript
// load-test.js
import http from 'k6/http';
import { check, sleep } from 'k6';

export const options = {
  stages: [
    { duration: '5m',  target: 100 },
    { duration: '30m', target: 500 },
    { duration: '5m',  target: 0 },
  ],
  thresholds: {
    http_req_duration: ['p(99)<300'],
    http_req_failed:   ['rate<0.005'],
  },
};

export default function () {
  const r = http.get('https://api.example.com/healthz');
  check(r, { '200': (r) => r.status === 200 });
  sleep(1);
}
```

Run the test on a separate host. The orchestrator host is the system under test. Capture both client-side metrics and server-side metrics. The first time the p99 latency exceeds 300ms is the headroom ceiling; the first time the 5xx rate exceeds 1% is the failure ceiling.

**Rule:** the production fleet has 30% headroom over the worst load test result. If k6 sustains 800 RPS at p99 < 300ms, the production fleet should be sized for ~600 RPS sustained. The 30% is a buffer for unobserved spikes, unobserved code paths, and unobserved Tuesday afternoons.

**Vertical scaling vs. horizontal scaling:** for a sovereign runtime, vertical scaling is the default. Add RAM, add CPU cores, switch to a dedicated server. Horizontal scaling (multiple nodes) is a step the orchestrator should support but the operator should avoid until the single-node case is exhausted. The complexity cost of horizontal scaling (state replication, sticky sessions, leader election) is not worth it for a workload that fits on a €100/month server.

Sources: [k6 documentation](https://k6.io/docs/), [Brendan Gregg, "Thinking Methodically about Performance"](https://www.brendangregg.com/methodology.html), [Google SRE Book Ch. 27 (Reliable Cloud Infrastructure)](https://sre.google/sre-book/reliable-cloud-infrastructure/).

---

## 10. FinOps for sovereign infra: tag everything, query the bill

The difference between a €4 VPS and a €400/month bill is usually three untagged services running 24/7 in the background. The sovereign runtime has one major cost variable: the host. The FinOps practice is therefore simpler than the cloud-native case, but it is not zero.

**Three rules:**

1. **Tag every deployable.** The orchestrator's manifest should require a `cost-center` and `owner` field. Anything untagged is rejected at deploy time. The cost-center maps to a billing tag on the host (most cloud providers support per-resource tags; on bare metal, map to a project code).

2. **Run a weekly cost report.** `runtime cost report --period=7d --by=app`. Output: total compute-hours, total RAM-hours, total disk-GB, total bandwidth-GB, total cost per app. The number that matters is the cost per active user.

3. **Set a budget alert.** `runtime budget set --monthly=100 --warn=80 --action=notify`. When the host cost is projected to exceed the budget, the orchestrator sends a Telegram message and refuses to deploy additional apps in the same cost center.

**Cost of a sovereign runtime vs. cloud-native equivalent:**

| Workload | Cloud-native (EKS + RDS + ALB + S3 + CloudWatch) | Sovereign runtime (€100/mo dedi) |
|---|---|---|
| 5 apps, 10k MAU, 100GB data | ~$350/mo | ~€100/mo |
| 20 apps, 100k MAU, 1TB data | ~$2,800/mo | ~€300/mo (2 servers + 1 offsite) |
| 100 apps, 1M MAU, 10TB data | ~$22,000/mo | ~€1,500/mo (10 servers + object storage) |

The crossover is roughly 1M MAU or 50 apps. Below the crossover, the sovereign runtime is 3-5x cheaper. Above it, the operational complexity cost (more servers, more replication, more on-call) starts to favor managed cloud-native. The orchestrator should make it easy to migrate from one to the other.

Sources: [FinOps Foundation Framework](https://www.finops.org/framework/), [CloudZero State of Cloud Costs 2023](https://www.cloudzero.com/state-of-cloud-costs/), [Hetzner pricing](https://www.hetzner.com/cloud).

---

## 11. Configuration management: Ansible for orchestration, cloud-init for bootstrap

The two layers of configuration in a sovereign runtime:

- **Bootstrap (cloud-init):** runs once at instance launch. Sets hostname, installs the orchestrator binary, registers the node with the cluster, generates a node identity key, opens the wireguard port.
- **Steady state (Ansible):** runs repeatedly, idempotent. Updates the binary, rotates configs, applies security baselines, manages the observability stack, ships the alert rules.

The split is intentional. cloud-init is good at "I am a new node, set me up" and bad at "I am a node that has been running for 200 days, change this one config." Ansible is good at the latter and bad at the former. Using one for both is how a 6-month-old production fleet ends up with 14 different config drift bugs.

**Recommended playbook (`site.yml`):**

```yaml
- hosts: runtime_nodes
  become: yes
  tasks:
    - name: Update runtime binary
      ansible.builtin.copy:
        src: files/runtime
        dest: /usr/local/bin/runtime
        mode: '0755'
      notify: restart runtime

    - name: Apply observability stack
      ansible.builtin.include_role:
        name: observability

    - name: Apply security baseline
      ansible.builtin.include_role:
        name: security_baseline
      vars:
        sshd_permit_root_login: "no"
        sshd_password_authentication: "no"
        fail2ban_enabled: yes

    - name: Sync alert rules
      ansible.builtin.copy:
        src: files/alerts/
        dest: /etc/runtime/alerts/
      notify: reload vmalert

  handlers:
    - name: restart runtime
      ansible.builtin.systemd:
        name: runtime
        state: restarted
```

**Rule:** the Ansible playbook is idempotent. Running it twice in a row produces no diff. The way you know the playbook is idempotent is that you run it twice in a row, and the second run reports 0 changes. This is a manual test, but it is the only one that matters.

Sources: [Ansible best practices](https://docs.ansible.com/ansible/latest/tips_tricks/index.html), [cloud-init documentation](https://cloudinit.readthedocs.io/), [Red Hat's "Configuration Management in a DevOps World"](https://www.redhat.com/en/topics/devops/what-is-configuration-management).

---

## 12. Provisioning golden images with Packer

The fastest way to a reproducible fleet is a golden image built with Packer. The image contains the OS, the orchestrator binary, the observability agent, the security baseline, and nothing else. New nodes boot from the image and join the cluster in under 60 seconds.

**Recommended `runtime-base.pkr.hcl`:**

```hcl
source "hcloud" "runtime-base" {
  image       = "ubuntu-24.04"
  location    = "fsn1"
  server_type = "cx21"
  ssh_username = "root"
  image_name   = "runtime-base-{{timestamp}}"
}

build {
  sources = ["source.hcloud.runtime-base"]

  provisioner "shell" {
    inline = [
      "apt-get update",
      "apt-get install -y unattended-upgrades fail2ban wireguard",
      "sysctl -w net.ipv4.ip_forward=1",
    ]
  }

  provisioner "file" {
    source      = "files/runtime"
    destination = "/usr/local/bin/runtime"
  }

  provisioner "ansible" {
    playbook_file = "./playbooks/runtime-base.yml"
  }
}
```

The image is rebuilt nightly with `packer build -var timestamp=$(date +%s)`. New nodes are launched from the latest image. Old nodes are drained and replaced on a 30-day rotation. This pattern is the equivalent of immutable infrastructure on bare metal.

Sources: [HashiCorp Packer documentation](https://developer.hashicorp.com/packer/docs), [Hetzner Cloud Packer plugin](https://github.com/hashicorp/packer-plugin-hcloud).

---

## 13. Patch management: unattended-upgrades + Trivy

The Ubuntu default is `unattended-upgrades` enabled, with security patches auto-applied and a daily random reboot in the 4-5am window. This is correct. Do not disable it.

The 2017 Equifax breach was caused by an unpatched Apache Struts vulnerability (CVE-2017-5638) that had a patch available for two months (US GAO, [GAO-18-559](https://www.gao.gov/products/gao-18-559)). The 2021 Codecov supply-chain attack exploited a bash uploader that was not pinned to a specific version (Codecov, [2021-04-15 postmortem](https://about.codecov.io/security-update-april-2021-post-mortem/)). The pattern is consistent: a known vulnerability, a known patch, a known delay.

**Recommended `20auto-upgrades`:**

```conf
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
APT::Periodic::AutocleanInterval "7";
APT::Periodic::Download-Upgradeable-Packages "1";
APT::Periodic::Verbose "1";
Unattended-Upgrade::Remove-Unused-Dependencies "true";
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-Time "04:30";
```

For the application layer (containers, dependencies, base images), use Trivy. The runtime's CI must run `trivy image --severity HIGH,CRITICAL --exit-code 1` on every push. The Trivy vs Grype comparison is settled for this audience: Trivy has a 6-hour median CVE-to-detection lag vs Grype's 12-hour (Aqua Security, [2024 benchmark](https://trivy.dev/)), better Go and Rust coverage, and a single-binary distribution.

**The patching rhythm:**

- **Daily:** OS security patches (unattended-upgrades, auto-reboot).
- **Weekly:** base image rebuild via Packer.
- **On push:** Trivy scan of the runtime binary and every container.
- **On CVE disclosure:** Trivy scan of the running fleet within 1 hour of disclosure.
- **Quarterly:** review of Trivy reports for patterns; update base image to a newer Ubuntu LTS.

Sources: [Ubuntu unattended-upgrades documentation](https://help.ubuntu.com/community/AutomaticSecurityUpdates), [Trivy documentation](https://trivy.dev/), [CIS Benchmarks](https://www.cisecurity.org/cis-benchmarks).

---

## 14. Secret rotation: SOPS + age, not Vault

HashiCorp Vault is the right answer for a 50-engineer team with 20 microservices. It is the wrong answer for a solo founder. Vault has a Raft consensus layer, a seal/unseal ceremony, and a documentation surface that takes a week to learn. The smaller-tool answer is SOPS + age.

**SOPS** is a Mozilla project that encrypts YAML/JSON/ENV files in place, using a key management backend of your choice. **age** is a modern file-encryption tool by Filippo Valsorda, replacing GPG. Combined, they give you:

- Encrypted secrets in git (every secret is a file in the repo, encrypted to the team's age public keys).
- Decryption on the fly on the target host (the orchestrator has the private key, or fetches it from a one-time secrets store).
- Auditable diff history (every secret change is a commit).
- No runtime dependency on a Vault cluster.

**Workflow:**

```bash
# One-time setup
age-keygen -o keys/team.age
AGE_RECIPIENT=$(age-keygen -y keys/team.age)

# Encrypt a secrets file
sops --age $AGE_RECIPIENT --encrypt --in-place secrets/app-prod.yaml

# Decrypt for use
sops --decrypt secrets/app-prod.yaml | kubectl apply -f -

# Rotate a secret
sops --age $AGE_RECIPIENT --rotate --in-place secrets/app-prod.yaml
```

**Rotation cadence:**

- **Database passwords:** 90 days, automated.
- **API tokens for third-party services:** on incident, or 180 days.
- **TLS private keys:** 365 days, on the same cadence as the cert.
- **SSH keys:** never, unless compromised; rotate the deploy keys, not the operator's personal keys.

**Rule:** every secret has an owner (a `cost-center` field in the SOPS file) and a rotation date. The orchestrator's `runtime secrets list --expiring-within=14d` reports secrets that need rotation.

Sources: [Mozilla SOPS documentation](https://github.com/getsops/sops), [age documentation](https://age-encryption.org/), [HashiCorp Vault comparison](https://www.vaultproject.io/).

---

## 15. Network operations: Caddy, fail2ban, WireGuard

The sovereign runtime's network surface is small: one HTTPS port, one SSH port, one WireGuard port. Each requires its own discipline.

**Reverse proxy:** Caddy. It is the only major reverse proxy that ships with automatic HTTPS via Let's Encrypt, HTTP/3, and zero-downtime config reload. The Caddyfile is a single file that any operator can read.

```
api.example.com {
  reverse_proxy app-backend:8080
  encode zstd gzip
  log {
    output file /var/log/caddy/access.log {
      roll_size 100mb
      roll_keep 5
    }
  }
}
```

**SSH hardening:** disable password auth, disable root login, install fail2ban, restrict source IPs. The Ansible `security_baseline` role should enforce this. The 2021 Microsoft Exchange attack wave (Hafnium) succeeded against hosts that had password auth enabled and no fail2ban (Microsoft, [Hafnium postmortem](https://www.microsoft.com/security/blog/2021/03/02/hafnium-targeting-exchange-servers/)).

```yaml
# /etc/ssh/sshd_config
PermitRootLogin no
PasswordAuthentication no
PubkeyAuthentication yes
AllowUsers runtime-ops
```

**Mesh VPN:** WireGuard. The orchestrator's nodes communicate over a WireGuard mesh; the public internet sees only the orchestrator's reverse proxy. Each node has a static IP in the `10.10.0.0/24` mesh range, and the orchestrator's API listens on `10.10.0.1:8443`. Operators access the API through their own WireGuard peer.

Sources: [Caddy documentation](https://caddyserver.com/docs/), [fail2ban documentation](https://github.com/fail2ban/fail2ban), [WireGuard documentation](https://www.wireguard.com/).

---

## 16. PostgreSQL operations: the four things you check weekly

PostgreSQL is the single most important piece of stateful infrastructure in the sovereign runtime. The weekly checklist is four items, each with a single command.

```bash
# 1. Vacuum progress (autovacuum catching up?)
runtime db vacuum --report

# 2. Long-running queries (>5min)
runtime db queries --longer-than 300s

# 3. Replication lag
runtime db replication --lag

# 4. Connection pool utilization
runtime db pool --utilization
```

**Autovacuum tuning:** the defaults are conservative. For a high-write workload, the recommended settings are:

```sql
ALTER SYSTEM SET autovacuum_vacuum_scale_factor = 0.05;
ALTER SYSTEM SET autovacuum_analyze_scale_factor = 0.02;
ALTER SYSTEM SET autovacuum_vacuum_cost_limit = 1000;
ALTER SYSTEM SET max_wal_size = '4GB';
ALTER SYSTEM SET checkpoint_completion_target = 0.9;
```

**PITR is not optional.** Section 8 covers the backup side. The recovery side: `runtime db restore --target-time=2026-06-03T14:22:00Z --target-host=db-staging` recovers a specific database to a specific timestamp on a staging host, for verification. The same command on the production host (after a maintenance window) is the disaster recovery procedure.

**The "PG is on fire" runbook:**

1. Check `pg_stat_activity` for blocked queries: `SELECT * FROM pg_stat_activity WHERE wait_event IS NOT NULL;`
2. If a single query is blocking, terminate it: `SELECT pg_cancel_backend(<pid>);`
3. If the connection pool is exhausted, restart the pooler (pgBouncer): `systemctl restart pgbouncer`.
4. If the database itself is unresponsive, fail over to the replica: `runtime db promote --target=replica-1`.
5. If the primary is corrupted, restore from PITR to a new host and repoint the application.

Sources: [PostgreSQL documentation, Routine Vacuuming](https://www.postgresql.org/docs/current/routine-vacuuming.html), [pgBackRest documentation](https://pgbackrest.org/), [pganalyze guide to autovacuum](https://pganalyze.com/docs/guides/best-practices/autovacuum-tuning).

---

## 17. Container operations: hardening, log drivers, resource limits

The Docker daemon is a privileged service. Hardening it is non-negotiable.

**Daemon config (`/etc/docker/daemon.json`):**

```json
{
  "icc": false,
  "log-driver": "json-file",
  "log-opts": {
    "max-size": "100m",
    "max-file": "5"
  },
  "live-restore": true,
  "userland-proxy": false,
  "no-new-privileges": true,
  "default-ulimits": {
    "nofile": { "Name": "nofile", "Hard": 65535, "Soft": 32768 }
  }
}
```

The `icc: false` setting disables inter-container communication by default. The orchestrator creates explicit networks per app. The `no-new-privileges: true` setting blocks privilege escalation. The `live-restore: true` setting keeps containers running across Docker daemon restarts.

**Resource limits:** every container in the orchestrator's manifest must declare a memory limit and a CPU limit. The orchestrator should refuse a manifest that lacks them. A container without a memory limit is a memory leak waiting to become a node-wide OOM.

**Log rotation:** the `max-size` and `max-file` settings prevent disk-exhaustion attacks via log spam. The 2018 Tesla cryptomining incident was caused by an exposed Kubernetes dashboard with no log limits; the attackers' miner filled the disk in hours (RedLock, [2018 report](https://www.redlock.io/blog/cryptojacking-tesla-amazon-web-services)). Same pattern, different orchestrator.

**Image hygiene:** every image in the runtime's registry must be scanned by Trivy on push. The runtime refuses to deploy an image with a CRITICAL CVE that has a known fix. Images with a CRITICAL CVE that has no fix (e.g., upstream library abandoned) get a waiver, stored in git, with an owner and a review date.

Sources: [Docker security best practices](https://docs.docker.com/engine/security/), [CIS Docker Benchmark](https://www.cisecurity.org/benchmark/docker), [Aqua Security Trivy vs Grype 2024](https://trivy.dev/).

---

## 18. Postmortem culture: one page, two follow-ups, a date

The Linear 2022-09-20 postmortem is the model. One page. Five sections. Two follow-up actions, each with an owner and a date. (Linear, [2022-09-20 postmortem](https://linear.app/blog/post-mortem-of-2022-09-20))

**Template: postmortem.md**

```markdown
# Postmortem: <incident title>

**Date:** 2026-06-03
**Severity:** Sev1
**Duration:** 14:22 UTC - 15:20 UTC (58 minutes)
**Author:** @victor
**Status:** Resolved

## Summary
In one paragraph, what happened from a user's perspective.

## Timeline (UTC)
- 14:22 — First 503 reported.
- 14:25 — PagerDuty alert fired: high error rate.
- 14:30 — IC declared Sev1; status page updated.
- 14:42 — Root cause identified: failed health check on app server 2.
- 14:55 — Traffic shifted to servers 3-5; error rate dropped.
- 15:20 — All clear; status page resolved.

## Root cause
One sentence. What actually broke.

## Contributing factors
Two to four items. What made it worse, or what made detection slow.

## What went well
Two to four items. What worked, what we should keep doing.

## What went poorly
Two to four items. What we should change.

## Action items
- [ ] Add per-server health check to runtime health probe (@victor, due 2026-06-17)
- [ ] Add canary deploy to rolling update strategy (@victor, due 2026-07-01)
```

**Rule:** the postmortem is blameless. The question is never "who broke it" but "what system allowed this to happen." A blameless culture produces better action items; a blame culture produces better hiding.

Sources: [Etsy Debriefing Facilitation Guide](https://extfiles.etsy.com/DebriefingFacilitationGuide.pdf), [Google SRE Book Ch. 14 (Postmortem Culture)](https://sre.google/sre-book/postmortem-culture/), [Linear 2022-09-20 postmortem](https://linear.app/blog/post-mortem-of-2022-09-20).

---

## 19. The 3am test: every alert, every dashboard, every runbook

The 3am test is brutal and simple. Imagine the page fires at 3am. You are groggy. You have never seen this alert before. You have 5 minutes before the status page update is due. Can you fix it?

The test is run quarterly. Pick a random alert. Pretend it just fired. Open the runbook cold. Run the first command. Did it work? Did the output match the expected? If yes, the runbook is good. If no, fix the runbook, not the alert.

The Cloudflare 2019-07-02 incident is the canonical example of an alert that passed the 3am test in design and failed it in practice: the alert fired, the runbook said "check the status page," and the status page was down because the same outage had taken it out (Cloudflare, [2019-07-02 postmortem](https://blog.cloudflare.com/details-of-the-cloudflare-outage-on-july-2-2019/)). The fix was a backup alert path. The lesson is that the runbook must be tested against the failure mode, not the success mode.

Sources: [Google SRE Book Ch. 11](https://sre.google/sre-book/being-on-call/), [Cloudflare 2019-07-02 postmortem](https://blog.cloudflare.com/cloudflare-outage-on-july-17-2023/).

---

## 20. The junior dev test: can a new hire operate it on day 5?

The junior dev test is the second brutal test. Imagine a new developer joins the team. By day 5, they should be able to:

1. Deploy a new app via the orchestrator's CLI.
2. Read the morning report and identify a real issue.
3. Open the relevant runbook and follow it to resolution.
4. Write a postmortem from a real incident in the last 30 days.

If any of these four tasks require tribal knowledge not in the repo, the system has failed. The fix is documentation in the repo, not more documentation in a wiki.

**Rule:** every runbook, every alert, every dashboard, every playbook lives in the repository under `ops/`. The README at `ops/README.md` is the entry point. New operators read it on day 1.

Sources: [The Twelve-Factor App](https://12factor.net/), [Basecamp's Shape Up](https://basecamp.com/shapeup), [Charity Majors, "On Being On-Call"](https://charity.wtf/2022/09/10/being-on-call/).

---

## 21. Audit and compliance: CIS, auditd, and the immutable log

A sovereign runtime running for a customer with a regulated workload (healthcare, finance, GDPR, SOC 2) needs three additional layers:

- **CIS benchmarks:** Ubuntu 24.04 LTS CIS Level 1, applied via the Ansible `security_baseline` role. The role is opinionated: SSH hardening, kernel parameters, auditd rules, file permission baselines. The role is tested weekly with `runtime audit cis --target=node-1`.
- **auditd:** the Linux audit daemon, configured to log every change to `/etc/`, every sudo invocation, every login, and every systemd unit modification. Logs ship to a separate object storage bucket with Object Lock.
- **Immutable log:** the audit logs are append-only. The orchestrator's `runtime audit verify` command runs daily and confirms the log chain has not been tampered with. The verification result is itself logged.

**The cost of compliance:** the CIS baseline adds ~3 hours of initial setup. The auditd rules add ~1 hour. The immutable log storage costs ~€5/month. The total is €8/month and a day of work. The benefit is a working SOC 2 Type 1 in 6 months and a working SOC 2 Type 2 in 12 months.

Sources: [CIS Benchmarks](https://www.cisecurity.org/cis-benchmarks), [auditd documentation](https://github.com/linux-audit/audit-documentation), [SOC 2 for small teams](https://www.vanta.com/).

---

## 22. The solo dev minimum: what you actually need on day one

A solo operator with the sovereign runtime needs the following on day one, and nothing more. Each tool is single-binary or single-process, runs on a €4 VPS, and has a maintenance cost measured in minutes per month.

| Layer | Tool | Why this one |
|---|---|---|
| Orchestrator | Sovereign Application Runtime | The product itself |
| Reverse proxy | Caddy | Auto-HTTPS, single binary |
| Metrics | VictoriaMetrics | 10x compression, 50MB RAM |
| Logs | VictoriaLogs | Single binary, LogsQL |
| Traces | Tempo | Single binary, OTLP native |
| Dashboards | Grafana | De facto standard, SQLite mode |
| Alerting | vmalert + AlertManager | Same binary family as metrics |
| Secrets | SOPS + age | Encrypted secrets in git |
| Backup | restic | Content-addressable, dedup |
| Postgres backup | pgBackRest | PITR, parallel restore |
| Image scan | Trivy | 6h CVE lag, Go/Rust coverage |
| Config mgmt | Ansible | SSH-only, no agent |
| Provisioning | Packer | Hetzner, AWS, GCP plugins |
| Boot | cloud-init | Vendor standard |
| Mesh | WireGuard | Kernel module, audit-friendly |
| Log shipping | OpenTelemetry Collector | OTLP, vendor-neutral |
| CI | Gitea Actions or Woodpecker | Self-hosted, low resource |

**The 5-7 tool ceiling:** if the operator is running more than 7 distinct tools, they are spending more time on the tools than on the product. The recommended set is 5-7 tools, with the orchestrator, reverse proxy, metrics/logs/traces, alerting, secrets, and backup as the irreducible core. Image scanning, config management, and CI are added when the workload justifies them.

Sources: [samber's "The Solo DevOps Handbook"](https://blog.samrose.name/), [Charity Majors, "Observability is a Many-Splendored Thing"](https://charity.wtf/), [fly.io's "Pricing Out a Single Server"](https://fly.io/blog/).

---

## 23. Tool consolidation: when to stop adding

The 5-7 tool ceiling is a hard limit. Adding an eighth tool means the operator is now maintaining two systems that overlap. The question to ask before adding a tool is:

1. Does the new tool replace an existing one, or is it additive?
2. If additive, does the new tool's value exceed the maintenance cost?
3. Will the new tool still be the right answer in 12 months?

Most tool additions fail question 3. The observability space in particular has churned every 18 months for the last decade: ELK, then Splunk, then Loki, then VictoriaLogs. The right move is to standardize on a single binary family (VictoriaMetrics + VictoriaLogs + vmalert) and stay there.

**Rule:** the operator is allowed to swap one tool per quarter, but not to add a tool that duplicates an existing one. The orchestrator's `runtime ops lint` should warn when a manifest references a tool that is not in the approved list.

Sources: [Hyrum's Law](https://www.hyrumslaw.com/), [PostgreSQL's "Don't Do This"](https://wiki.postgresql.org/wiki/Don't_Do_This), [Charity Majors on tool consolidation](https://charity.wtf/).

---

## 24. The runtime's DevOps surface: a closing recommendation

The Sovereign Application Runtime should ship with a single subcommand as the entry point for everything in this document:

```bash
runtime ops <subcommand>
```

Subcommands: `status`, `morning-report`, `alerts`, `runbook`, `backup`, `restore`, `secrets`, `audit`, `cost`, `capacity`, `incident`. Each is implemented in Rust, each is a thin wrapper over the system tools (systemd, journald, postgresql-client, sops, restic, vmalert), and each is scriptable.

The design choice that matters most is: **the runtime is the single CLI for the operator's day, and the operator can do everything from one terminal.** The Coolify, Dokploy, and CapRover UX is web-panel-first. The sovereign runtime is CLI/TUI-first, with a web UI as a thin layer on top of the same commands. The reason is that the operator's failure mode is the web panel being down; the CLI is a binary on the host and works even when the reverse proxy is on fire.

**The final opinion:** ops is not a discipline you add when you scale; it is a discipline you have on day one because it makes the product better. The orchestrator is the product. The runbooks, the alerts, the backups, the postmortems — those are the product too. A sovereign runtime that ships without them is a tool. A sovereign runtime that ships with them is an operating system.

---

## Appendix A: Runbook index

```
runbooks/
├── tls-renewal.md
├── disk-full.md
├── oom.md
├── high-error-rate.md
├── crash-loop.md
├── pg-replication.md
├── fd-exhaustion.md
├── backup-failed.md
├── orphan-procs.md
├── host-down.md
├── incident-response.md
├── postmortem-template.md
└── _index.md
```

## Appendix B: Alert index

| Alert | Severity | Runbook | SLO impact |
|---|---|---|---|
| TlsCertExpiringSoon | page | tls-renewal | none |
| DiskSpaceWarning | page | disk-full | high |
| MemoryPressure | page | oom | high |
| HighErrorRate | page | high-error-rate | high |
| ServiceCrashLoop | page | crash-loop | medium |
| PostgresReplicationLag | ticket | pg-replication | high |
| HighFdUsage | ticket | fd-exhaustion | medium |
| BackupFailed | page | backup-failed | high |
| OrphanProcesses | ticket | orphan-procs | low |
| HeartbeatMissed | page | host-down | high |

## Appendix C: Tool stack summary

- **5 core tools:** Runtime, Caddy, VictoriaMetrics/VictoriaLogs/vmalert, Grafana, restic.
- **2 add-ons (day 30):** SOPS+age, Trivy.
- **3 enterprise add-ons (day 90+):** Ansible, Packer, OpenTelemetry Collector (if not already shipped with the runtime).
- **2 optional (day 180+):** Tempo, auditd.

---

**Total ops surface:** 5-7 tools, 10 alerts, 12 runbooks, 1 status page, 1 morning report. That is enough. More is worse.

**Further reading (all cited above):**

- [Google SRE Book](https://sre.google/sre-book/) — free online.
- [Google SRE Workbook](https://sre.google/workbook/) — free online.
- [Charity Majors' blog](https://charity.wtf/) — observability and on-call.
- [Julia Evans' zines](https://wizardzines.com/zines/) — sysadmin fundamentals.
- [incident.io blog](https://incident.io/blog) — incident response patterns.
- [Linear postmortems](https://linear.app/blog) — blameless culture in practice.
- [Aqua Security Trivy](https://trivy.dev/) — image scanning.
- [VictoriaMetrics docs](https://docs.victoriametrics.com/) — single-binary metrics.
- [Mozilla SOPS](https://github.com/getsops/sops) — encrypted secrets in git.
- [Hetzner Cloud](https://www.hetzner.com/cloud) — the price/performance benchmark for sovereign infra.
