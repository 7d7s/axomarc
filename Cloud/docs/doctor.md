# Doctor — Self-Diagnostics and Incident Response

**Status:** V0 stub (basic checks), V1 full standard checks, V1.5 fleet + remote, V2 paranoid + sovereignty.
**Audience:** Every operator, every on-call, every support engineer, every design partner.
**Last updated:** 2026-06-04

This document is the **how to figure out what's wrong** guide. The `sovereign doctor` command is the on-call's best friend; `sovereign incident` and `sovereign retro` are the incident response toolkit. Every check has a runbook; every runbook has a fix; the report is exportable for support tickets.

---

## 1. The philosophy

When something is broken at 3 AM, the operator should not have to guess. `sovereign doctor` is the answer to:

- "Is the system healthy?"
- "What changed since the last deploy?"
- "Why is this app slow?"
- "Is the backup actually working?"
- "What does support need to debug this?"

`sovereign doctor` is **declarative**: it tells you the state of the system, not what to do about it. The `--fix` flag is opt-in (auto-fix common issues, but never destructive). The `--report` flag generates a support-ticket-ready markdown file.

The doctor command is also **privacy-first**: it never sends telemetry, never phones home, never reads user data. It runs entirely on the operator's host. The output is for the operator, not for the company.

---

## 2. The 5 levels of checks

| Level | When | What it checks | Speed | Output |
|---|---|---|---|---|
| **basic** | CI / smoke test / install verification | System, binary, DB readable | < 5s | `✓` or `✗` per check |
| **standard** *(default)* | On-call, daily check | + runtime, proxy, secrets, backup, network | < 30s | `✓` / `⚠` / `✗` per check |
| **full** | Support tickets, weekly review | + perf, security, observability, sovereignty | < 2 min | same + per-check detail |
| **paranoid** *(V2)* | Pre-release, post-incident, EU procurement | + multi-host, multi-region, full sovereignty test, supply chain | < 10 min | same + reproducible build hash |
| **custom** | User-defined subset | Specific categories | depends | depends |

```bash
# Quick sanity check
sovereign doctor

# Full check for a support ticket
sovereign doctor --level=full --report=support-2026-06-04.md

# Watch mode for continuous monitoring
sovereign doctor --level=standard --watch --interval=60

# Custom — just the backup category
sovereign doctor --category=backup --level=full
```

---

## 3. The 14 check categories

Each category is a `DoctorCheck` trait impl in `sovereign-doctor/`. The doctor orchestrator runs all 14 (or a subset) in parallel and aggregates the results.

### 3.1 System

**What it checks:**
- Kernel version (≥ 4.18 for cgroups v2, ≥ 5.10 recommended)
- cgroups v1 vs v2 (v2 required for modern resource control)
- Namespaces (`/proc/self/ns/`)
- Free disk space on `/var/lib/sovereign` (warn < 20%, fail < 10%)
- Free RAM (warn < 20%, fail < 10%)
- CPU count and load average (warn if load > nCPU * 0.8)
- `/dev/null` readable/writable
- File descriptor limits (warn if `ulimit -n` < 65536)
- Time skew vs NTP (warn if > 5s, fail if > 60s)

**Common issues:**
- "cgroups v1 detected" → upgrade kernel to 5.10+ or enable v2 in GRUB
- "Free disk 8%" → `tool log rotate --all && tool backup prune --keep-last 7`
- "Time skew 90s" → enable NTP (`timedatectl set-ntp true`)

### 3.2 Binary

**What it checks:**
- Binary version matches the latest release (optional, opt-in)
- Binary SHA-256 matches the published hash (V2, cosign-verified)
- Binary is statically linked (no missing shared libs)
- License file present and unmodified
- mtime / atime sane (not 1970, not in the future)

**Common issues:**
- "Binary outdated: 0.1.0 vs 0.1.5" → `sovereign update`
- "License file missing" → reinstall (`curl -sSf sovereignruntime.dev/install.sh | sh`)
- "Binary is not statically linked" → reinstall the musl variant

### 3.3 Storage (SQLite)

**What it checks:**
- SQLite file exists at `/var/lib/sovereign/sovereign.db`
- WAL mode is enabled (`PRAGMA journal_mode = WAL`)
- `PRAGMA integrity_check` returns `ok`
- `PRAGMA foreign_key_check` returns no violations
- Free disk on the volume (see System)
- Litestream replication lag (if configured)
- All 7 core tables present (`app`, `deployment`, `domain`, `secret`, `server`, `backup`, `user`)
- `audit_event` table has triggers (`audit_no_update`, `audit_no_delete`)
- `audit_event` row count is non-zero (a fresh install with no events is suspicious)

**Common issues:**
- "WAL mode disabled" → restart sovereign; check the SQLite adapter
- "integrity_check failed" → `sovereign recover` (V2) or restore from Litestream
- "Litestream lag 6h" → check S3 credentials, check disk on the replica

### 3.4 Runtime (Docker / Podman)

**What it checks:**
- Docker (or Podman) daemon is reachable (`/var/run/docker.sock` or `tcp://`)
- Daemon version (≥ 20.10)
- Daemon storage driver (overlay2 recommended)
- Image pull works (test with a small image like `alpine:3.20`)
- Container create + start + stop + remove works (test container)
- Network mode `bridge` is available
- Volume mount works
- `dmesg` for OOM kills in the last 24h (if accessible)

**Common issues:**
- "Docker daemon unreachable" → `systemctl restart docker`
- "Image pull failed" → check DNS, check proxy, check `~/.docker/config.json`
- "OOM killed in last 24h" → raise the cgroup limit in `app.yaml` or fix the leak

### 3.5 Proxy (Caddy / Nginx)

**What it checks:**
- Proxy admin API is reachable
- Proxy version (Caddy ≥ 2.7, Nginx ≥ 1.20)
- Number of routes loaded (should match `app` count)
- TLS certificates for all configured hostnames are valid
- TLS certs expiring in < 30 days (warn), < 14 days (fail)
- ACME account is registered (for Let's Encrypt)
- HTTP→HTTPS redirect works
- Caddy disk usage (Caddy uses ~80MB RAM, log files can grow)

**Common issues:**
- "Caddy admin unreachable" → `systemctl restart caddy`
- "Cert expiring in 5 days" → `sovereign certs renew <hostname>`
- "ACME account not registered" → `sovereign certs init` (or check Caddy logs)

### 3.6 Secrets (age)

**What it checks:**
- Master key file exists at `/var/lib/sovereign/master.key.age`
- Master key file permissions are 0600 (or 0640 with sovereign group)
- Master key is decryptable (round-trip with a known plaintext)
- Argon2id parameters match the recommended (`m=19456, t=2, p=1`)
- All `secret` rows decrypt successfully
- No secret row is older than 1 year without rotation (warn)

**Common issues:**
- "Master key permissions 0644" → `chmod 600 /var/lib/sovereign/master.key.age`
- "Secret row 3 failed to decrypt" → the key was rotated, restore from backup
- "Secret not rotated in 380 days" → `sovereign secret rotate <app> <key>`

### 3.7 Backup

**What it checks:**
- S3 (or S3-compatible) is reachable
- S3 credentials are valid (list-bucket test)
- Last successful backup is < 25h old (warn) or < 49h old (fail)
- Last successful verify is < 8 days old (warn) or < 15 days old (fail)
- Total backup size matches expected (within 20%)
- The 3-2-1-1-0 rule is satisfied (3 copies, 2 media, 1 offsite, 1 immutable, 0 errors)
- Litestream WAL position is recent

**Common issues:**
- "S3 unreachable" → check IAM credentials, check `~/.aws/config`
- "Last backup 60h ago" → check the backup cron, check the backup logs
- "Verify failed: 2 row count diffs" → investigate; could be a real data corruption

### 3.8 Network

**What it checks:**
- DNS resolution works (test with `cloudflare.com`, `github.com`, sovereign's release host)
- HTTPS to common endpoints works (test with `https://api.sovereignruntime.dev/health`)
- HTTP/3 (if configured) works
- NTP time sync (within 5s)
- Outbound port 443 is not blocked
- Local ports 80, 443 are not bound by another process
- The sovereign HTTP port (default 7878) is bound and listening

**Common issues:**
- "DNS resolution failed" → check `/etc/resolv.conf`, check the local DNS server
- "Outbound 443 blocked" → firewall rule, NAT, or proxy issue
- "Port 80 already bound" → another web server is running; stop it

### 3.9 Agents / Servers (V1.5+)

**What it checks:**
- All servers in the `server` table have a heartbeat within 30s
- All agents are reachable over mTLS
- Server `status` is `healthy` (not `unreachable` or `drained`)
- Server resources are sane (CPU, RAM, disk < 90%)
- mTLS certificates are valid (not expired, not revoked)

**Common issues:**
- "Server 2 heartbeat > 30s" → `sovereign server doctor <server-id>`
- "mTLS cert expires in 3 days" → `sovereign cert rotate` (V2)

### 3.10 Observability (V1.2+)

**What it checks:**
- `/metrics` endpoint is reachable on the control plane port
- `/metrics` returns valid Prometheus text format
- The 11 mandatory metrics are present (see [`architecture.md` §8.2](./architecture.md))
- VictoriaMetrics is reachable (if configured)
- VictoriaLogs is reachable (if configured)
- Grafana is reachable (if configured)
- Log directory `/var/log/sovereign` is writable
- Log rotation is working (no log file > 1 GB)

**Common issues:**
- "/metrics endpoint unreachable" → check the HTTP server, check the port
- "Mandatory metric missing: deploys_total" → the observability adapter is not wired; check the version
- "Log file 4.2 GB" → `sovereign log rotate` is not running; check the cron

### 3.11 Security

**What it checks:**
- `umask` is 0077 (or stricter)
- `/var/lib/sovereign` is 0700 (or 0750 with sovereign group)
- `/etc/sovereign` is 0755
- No world-readable secret files
- No world-readable config files
- systemd unit file has `User=sovereign` (not root)
- TLS for the control plane API is enabled (V1+)
- No hardcoded secrets in config files
- `cargo audit` has no known advisories in the dependency tree (V1.5+)

**Common issues:**
- "Permissions 0755 on /var/lib/sovereign" → `chmod 700 /var/lib/sovereign`
- "systemd runs as root" → `sed -i 's/User=root/User=sovereign/' /etc/systemd/system/sovereign.service`
- "cargo audit: 2 advisories" → `cargo update` or pin the affected dep

### 3.12 Sovereignty (V2+)

**What it checks:**
- License file is Apache 2.0 (not modified, not carve-out)
- No `proprietary/` directory exists
- No source-available license exceptions
- Offline mode test (network-isolated container runs `sovereign deploy` end-to-end) — V2 CI
- Export works (the export tarball is non-empty)
- Import works on a fresh host — V2 CI
- EU incorporation page is reachable and lists GmbH + OÜ
- Public sustainability signal is reachable
- Bus factor ≥ 3 (CODEOWNERS or foundation)
- Security advisories are public (GHSA tab is not empty)
- Binary is reproducible (re-build produces the same SHA-256) — V2 CI
- Cosign signature is valid — V2

**The 10-point sovereignty test** is the same as the CI gate in [`sovereignty-and-governance.md` §2](#). The doctor version runs locally; the CI version runs in GitHub Actions.

### 3.13 Performance (V1+)

**What it checks:**
- Disk I/O: sequential read/write (target: ≥ 200 MB/s)
- Disk I/O: random read/write IOPS (target: ≥ 5,000 IOPS)
- Memory bandwidth (target: ≥ 10 GB/s)
- Network throughput (target: ≥ 1 Gbps on the loopback)
- SQLite INSERT performance (target: ≥ 10,000 rows/sec)
- Audit append latency (target: < 1ms p99)
- Health check latency (target: < 30ms p99)

The benchmarks are run on demand, not on every doctor invocation. `sovereign doctor --level=full --category=performance` runs them.

### 3.14 Cost (V1+)

**What it checks:**
- Per-app resource cost (CPU + RAM + storage, attributed by cgroup)
- Per-server cost (the VPS spend, divided by app count)
- Backup cost (S3 storage + egress)
- Egress cost (bandwidth to public, attributed by app)
- Total monthly cost (estimated)

The output is a table:
```text
$ sovereign cost
app       server  vcpu    ram     storage  egress   cost/mo
api       node-1  0.5     512MB   2GB      50GB     €3.20
web       node-1  0.25    256MB   1GB      30GB     €1.80
postgres  node-1  1.0     2GB     20GB     5GB      €8.40
                                                --------
                                                Total:  €13.40/mo
```

The cost is calculated from the cgroup metrics, not from the cloud bill. It's an *estimate*, not a bill. V2 adds the cloud-bill reconciliation.

### 3.15 Cross-cutting (V1+) — the 4 "this is the failure mode that took the site down" checks

These four checks were promoted from ad-hoc G16 sub-bullets to first-class standard-level checks after the market-gap analysis in V1.5. Each one corresponds to a user-pain entry in `user-pain-research.md` (§4) that the basic+standard levels previously missed. They live in their own files (`sovereign-doctor/src/checks/cross_cutting/`) and run alongside the 14 categories above.

**`db_pool_configured` (Network-adjacent, logically Database):** For every Postgres / MySQL / Redis DB that has ≥ 1 app using it, a connection pool (`sovereign db pool`) must be configured. The check is `Fail` for an unpooled DB with ≥ 1 app. The fix is `sovereign doctor --fix=db_pool_configured`, which runs `sovereign db pool create --db <name>` with the recommended backend (PgBouncer for Postgres, ProxySQL for MySQL, in-Rust `deadpool-redis` for Redis).

**`dns_provider_configured` (Network):** If the operator has ≥ 1 domain in the `domain` table (a managed hostname, a verified custom domain, an APEX), at least one DNS provider must be configured (`sovereign dns provider add`). The fix is to add a provider; the check does **not** pick a specific provider (the operator's choice of Cloudflare vs. Hetzner DNS vs. RFC 2136 is policy, not a health check). Without this check, an operator can have a working sovereign box and a broken `sovereign dns record add` for months — the symptom only surfaces when they try to ship a new hostname.

**`deploy_drain_spec` (Deployment):** Every `app.yaml` must have a `shutdown_grace_period` set. Apps without one are `Warn` (the default is 10s, but a Rails / Django app with 30s request latency should set 60s+). The check parses every `app.yaml` in the config dir and verifies the field. The fix writes the recommended value (60s for Rails, 30s for Go, 10s for `static` apps) into the file. Catches the "we deployed, the load balancer killed in-flight requests" failure mode in `user-pain-research.md` §4.1.

**`migration_lock_timeout` (Database):** Every Postgres DB must have `lock_timeout` set on the connection pool (default: 5s). The check is `Fail` if the connection string lacks the parameter. The fix applies `ALTER SYSTEM SET lock_timeout = '5s'` and reloads (`sovereign db pool reload`). Catches the "Rails migration locks the table for 8 minutes" failure mode in `user-pain-research.md` §4.3 — the table is locked, every other connection in the pool is blocked, the deploys time out, the on-call gets paged. In V2, this check is duplicated at the *policy* layer (see `phase-03-v2.md` I4 — `migrations/lock_timeout_required.rego`) so the violation is blocked at deploy time, not just detected at morning doctor.

**Why these are first-class checks (not sub-bullets):** The market-gap analysis flagged them as the failure modes that operators *do* hit in V1.5 production — but only weeks after the fact. Promoting them to standard-level means the failure mode is caught by the morning doctor, not by a 3am page.

---

## 4. The output format

### 4.1 The default output (terminal)

```text
$ sovereign doctor
✓ system        kernel 5.15, cgroups v2, 8GB free disk, 4GB free RAM
✓ binary        v0.1.5 (2026-06-04), statically linked, license OK
✓ storage       SQLite WAL, integrity OK, 7/7 tables, audit triggers present
✓ runtime       Docker 24.0.7, daemon reachable, pull OK, container OK
✓ proxy         Caddy 2.7.6, 3 routes, 3 valid TLS certs (next expiry 47d)
✓ secrets       age key OK, 12 secrets, round-trip 0.8ms
✓ backup        S3 reachable, last backup 2h ago, last verify 5d ago
✓ network       DNS OK, HTTPS OK, NTP skew 1.2s
✓ observability /metrics OK, 11/11 mandatory metrics, log dir writable
✓ security      umask 077, /var/lib/sovereign 0700, systemd user=sovereign

12/12 passed. Run `sovereign doctor --watch` for continuous monitoring.
```

### 4.2 The warning output

```text
$ sovereign doctor
✓ system        kernel 5.15, cgroups v2, 8GB free disk, 4GB free RAM
✓ binary        v0.1.5 (2026-06-04), statically linked, license OK
⚠ storage       SQLite WAL, integrity OK, 7/7 tables, audit triggers present
                Litestream lag: 6h (acceptable, warn threshold 12h)
✓ runtime       Docker 24.0.7, daemon reachable, pull OK, container OK
⚠ proxy         Caddy 2.7.6, 3 routes
                ⚠ Cert api.example.com expires in 12 days
✓ secrets       age key OK, 12 secrets, round-trip 0.8ms
✗ backup        S3 reachable
                ✗ Last successful backup 60h ago (fail threshold 49h)
✓ network       DNS OK, HTTPS OK, NTP skew 1.2s

10/12 passed, 2 warnings, 1 failure.
Run `sovereign doctor --explain backup` for details.
Run `sovereign doctor --fix` to attempt auto-remediation.
```

### 4.3 The JSON output (for agents)

```json
{
  "ok": false,
  "level": "standard",
  "timestamp": "2026-06-04T14:22:03Z",
  "duration_ms": 12340,
  "passed": 10,
  "warnings": 2,
  "failed": 1,
  "checks": [
    {
      "category": "system",
      "name": "kernel-version",
      "status": "pass",
      "duration_ms": 50,
      "details": { "kernel": "5.15.0-91-generic", "cgroups": "v2" }
    },
    {
      "category": "backup",
      "name": "last-backup-age",
      "status": "fail",
      "duration_ms": 200,
      "details": { "last_backup": "2026-06-02T02:00:00Z", "age_hours": 60.3, "threshold_hours": 49 },
      "fix_hint": "Check the backup cron, check the backup logs, run `sovereign backup run`"
    }
  ]
}
```

### 4.4 The exit code

| Code | Meaning |
|---|---|
| 0 | All checks passed (warnings allowed) |
| 1 | One or more checks failed |
| 2 | Doctor itself could not run (e.g., binary version mismatch) |

---

## 5. The subcommands

### 5.1 `sovereign doctor --explain <check>`

Detailed explanation of a specific check, including the threshold, the common causes, and the suggested fix.

```text
$ sovereign doctor --explain backup.last-backup-age

Check: backup.last-backup-age
Category: backup
Default level: standard

What it checks:
  The age of the last successful backup. Backups older than 49 hours
  mean the user has at most 49 hours of recoverable data.

Threshold:
  Warn: 25 hours
  Fail: 49 hours

Why it matters:
  Backups you haven't taken are hopes. The GitLab 2017-01-31 postmortem
  is the canonical lesson: 8 months of "successful" backups, all empty,
  all useless.

How it's measured:
  1. Query the `backup` table for the most recent row with status = 'success'.
  2. Compare the `started_at` timestamp to `now()`.

Common causes:
  1. The backup cron is not running. Check `systemctl status sovereign-backup.timer`.
  2. The backup job is failing. Check `/var/log/sovereign/backup.log`.
  3. The S3 bucket is full or unreachable. Check `aws s3 ls s3://sovereign-backups/`.
  4. The Litestream replica is lagging. Check `litestream snapshots`.

Suggested fix:
  $ sudo systemctl status sovereign-backup.timer
  $ sudo journalctl -u sovereign-backup --since "1 hour ago"
  $ sovereign backup run --target postgres --verbose
  $ sovereign backup verify --target postgres --restore-to scratch
```

### 5.2 `sovereign doctor --fix`

Attempts to auto-fix common issues. **Never destructive.** Each fix is opt-in via a confirmation prompt (or `--yes` to skip).

```text
$ sovereign doctor --fix

Found 3 auto-fixable issues:

1. backup.last-backup-age (60h ago)
   Fix: run a manual backup now
   Action: sovereign backup run --target postgres
   [y/N]? y
   ✓ Manual backup started. Will verify when complete.

2. proxy.cert-expiring (12 days)
   Fix: renew the cert
   Action: sovereign certs renew api.example.com
   [y/N]? y
   ✓ Cert renewed. New expiry 90 days.

3. observability.log-rotation (log file 4.2 GB)
   Fix: rotate logs
   Action: sovereign log rotate --all
   [y/N]? y
   ✓ Logs rotated. 4.1 GB freed.

3/3 fixed. Run `sovereign doctor` to verify.
```

**The auto-fixable issues list (V1+):**

| Category | Check | Auto-fix |
|---|---|---|
| backup | last-backup-age | run a manual backup |
| backup | last-verify-age | run a manual verify |
| proxy | cert-expiring | renew the cert |
| observability | log-rotation | rotate the logs |
| security | permissions | chmod the file |
| security | systemd-user | edit the systemd unit |
| storage | wal-mode | restart the process (V1.5) |
| secrets | secret-rotation | (manual only — secrets are sensitive) |
| system | free-disk | rotate logs, prune backups |
| network | ntp-skew | (manual only — NTP requires root) |

**The never-auto-fixed list:**

- Secrets (rotation requires user input)
- TLS cert issues that may be DNS-related
- Database integrity failures (require backup restore)
- Sovereignty test failures (require policy decision)

### 5.3 `sovereign doctor --report=<file>`

Generates a support-ticket-ready markdown report.

```bash
sovereign doctor --level=full --report=support-2026-06-04.md
```

The report includes:
- The full doctor output (all 14 categories, all sub-checks)
- The sovereign version + commit hash
- The host's `uname -a`, `uptime`, `free -h`, `df -h`
- The recent `journalctl` for the sovereign service
- The recent `docker events`
- The audit log for the last 24 hours (V1+)
- A "what I tried" section (operator fills in)
- A redacted secrets section ("DB_PASSWORD is set", never the value)

The report is **opt-in** (the operator must explicitly generate it) and **local** (it is never uploaded anywhere).

### 5.4 `sovereign doctor --watch`

Continuous monitoring. Runs the check suite every N seconds, prints a one-line summary, alerts on transition from pass to fail.

```bash
# Watch with default 60s interval
sovereign doctor --watch

# Watch with 10s interval, only fail-level changes
sovereign doctor --watch --interval=10 --only-fail

# Watch with alert to Telegram
sovereign doctor --watch --alert-on-fail --channel=telegram
```

Output:
```text
$ sovereign doctor --watch
[14:22:03] ✓ 12/12 (12 passed)
[14:23:03] ✓ 12/12 (12 passed)
[14:24:03] ⚠ 11/12 (1 warning: proxy cert expiring)
[14:25:03] ⚠ 11/12 (1 warning)
[14:26:03] ✗ 10/12 (1 failure: backup lag > 49h)
              → sovereign doctor --explain backup.last-backup-age
              → Telegram alert sent
[14:27:03] ✗ 10/12 (1 failure)
```

### 5.5 `sovereign doctor --category=<name>`

Run only one category. Useful for focused debugging.

```bash
sovereign doctor --category=backup --level=full
sovereign doctor --category=sovereignty --level=paranoid
```

### 5.6 `sovereign doctor --json`

Output the result as a JSON envelope (for agents, CI, monitoring).

```bash
sovereign doctor --json | jq '.failed'
# 1

sovereign doctor --json | jq '.checks[] | select(.status == "fail") | .name'
# "backup.last-backup-age"
```

### 5.7 `sovereign doctor --since=<duration>`

Run only the checks that are stateful (i.e., checks that depend on a time window). Useful for catching issues that have appeared since the last check.

```bash
# Run only the checks that have changed in the last 24h
sovereign doctor --since=24h

# Useful for: cron-driven monitoring, post-deploy verification
```

---

## 6. The incident response toolkit

Beyond `doctor`, the product ships an incident response toolkit for the on-call. These are V1.5+ features; V0/V1 ships the doctor stubs.

### 6.1 `sovereign incident` — incident mode

```bash
# Declare an incident
sovereign incident declare --severity=sev1 --title="api latency p99 > 5s" --commander=alice
# Output:
#   ✓ Incident #42 declared
#   ✓ Status page flipped to "investigating"
#   ✓ Deploys frozen (except hotfixes)
#   ✓ On-call paged
#   ✓ #inc-42 Slack channel created

# Add a note
sovereign incident note --incident=42 --note="Identified the regression in v1.4.2, rolling back now"
# Output: ✓ Note added to #inc-42

# Resolve
sovereign incident resolve --incident=42 --root-cause="Bad deploy v1.4.2, rolled back to v1.4.1"
# Output:
#   ✓ Incident #42 resolved
#   ✓ Status page flipped to "resolved"
#   ✓ Deploys unfrozen
#   ✓ Postmortem template created
```

**What it does:**
- Creates a structured incident record (in the audit log, with `kind = "incident.declare"`)
- Flips the public status page to "investigating"
- Freezes deploys (configurable: by default, freezes prod only)
- Pages the on-call (via the configured channels)
- Creates a Slack channel (or similar) for the incident
- Provides a timeline of events (notes, deploys, alerts, actions)

### 6.2 `sovereign retro` — post-incident review generator

```bash
# Generate a retro for the last incident
sovereign retro --incident=42 --output=retro-42.md
```

The generated retro includes:
- The incident summary (from the incident record)
- The timeline (from the audit log + notes)
- The affected apps, the user-visible impact, the duration
- The root cause (operator fills in)
- The contributing factors (operator fills in)
- The action items (with owners and due dates)
- The "what went well" / "what went poorly" sections
- The "5 whys" template
- Links to the deploys, the alerts, the runbooks

The retro is a **markdown file** the operator can edit and share. It is not auto-published.

### 6.3 `sovereign game-day` — chaos engineering drills

```bash
# Schedule a game-day
sovereign game-day schedule --date=2026-07-15 --scenario=server-fail --duration=2h

# Run a drill now
sovereign game-day run --scenario=server-fail --target=node-2

# Available scenarios
sovereign game-day list-scenarios
# - server-fail: kill a server, verify the system recovers
# - network-partition: block traffic, verify the system recovers
# - disk-full: fill the disk, verify alerts fire
# - cpu-spike: pin a CPU, verify health checks catch it
# - backup-corrupt: corrupt a backup, verify the verify catches it
# - cert-expire: simulate a cert expiring, verify the renew works
# - db-failover: kill the DB, verify the failover works
# - rate-limit: hit a rate limit, verify the system throttles
```

**The game-day philosophy:** if you haven't tested the failure, you don't know if your runbook works. `sovereign game-day` makes the drill a CLI command, not a 2-hour meeting.

---

## 7. The doctor CI gate

The doctor runs in CI on every PR. The CI version runs `sovereign doctor --level=full --json` and fails the build on any `failed` check.

```yaml
# .github/workflows/doctor.yml
name: Doctor
on: [push, pull_request]

jobs:
  doctor:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install sovereign
        run: |
          curl -sSf sovereignruntime.dev/install.sh | sh
      - name: Start the test fleet
        run: |
          docker compose -f tests/docker-compose.yml up -d
          sleep 10
      - name: Run doctor
        run: |
          sovereign doctor --level=full --json > doctor.json
          cat doctor.json
      - name: Fail on any check failure
        run: |
          FAILED=$(jq '.failed' doctor.json)
          if [ "$FAILED" -gt 0 ]; then
            echo "Doctor failed $FAILED checks"
            jq '.checks[] | select(.status == "fail")' doctor.json
            exit 1
          fi
```

The CI version does not run the sovereignty check (that's a separate gate in [`sovereignty-and-governance.md`](./sovereignty-and-governance.md)).

---

## 8. Privacy — what doctor does NOT do

`sovereign doctor` is **strictly local**. It does not:

- Phone home. The binary has no outbound network calls in doctor mode.
- Send telemetry. The 14 categories run on the operator's host; the output is for the operator.
- Read user data. The storage check verifies integrity, not content. The secrets check verifies the key, not the values.
- Inspect container content. The runtime check verifies the daemon, not the apps.
- Read the audit log. The audit log is append-only; doctor does not read it (the V2 evidence pack reads it, separately).

**The doctor report (V1.5+) is the only exception:** the operator explicitly generates it for a support ticket. The report is a markdown file on the operator's host, never uploaded.

---

## 9. The doctor architecture

`sovereign doctor` is a **use case** in `sovereign-core/src/use_cases/doctor.rs`. The 14 check categories are **port impls** in `sovereign-doctor/src/checks/`. The composition root wires the checks; the CLI dispatches to the use case.

```rust
// crates/sovereign-core/src/ports/doctor.rs
use async_trait::async_trait;

pub struct DoctorCheckResult {
    pub category: String,
    pub name: String,
    pub status: CheckStatus,  // Pass, Warn, Fail
    pub duration_ms: u64,
    pub details: serde_json::Value,
    pub fix_hint: Option<String>,
}

#[async_trait]
pub trait DoctorCheck: Send + Sync {
    fn name(&self) -> &str;
    fn category(&self) -> &str;
    fn default_level(&self) -> DoctorLevel;
    async fn run(&self, state: &AppState) -> Result<DoctorCheckResult, AppError>;
}

// crates/sovereign-doctor/src/checks/storage.rs
pub struct StorageIntegrityCheck;

#[async_trait]
impl DoctorCheck for StorageIntegrityCheck {
    fn name(&self) -> &str { "storage.integrity" }
    fn category(&self) -> &str { "storage" }
    fn default_level(&self) -> DoctorLevel { Standard }
    
    async fn run(&self, state: &AppState) -> Result<DoctorCheckResult, AppError> {
        let result = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&state.db).await?;
        let status = if result == "ok" { CheckStatus::Pass } else { CheckStatus::Fail };
        Ok(DoctorCheckResult {
            category: "storage".into(),
            name: "storage.integrity".into(),
            status,
            duration_ms: 0,
            details: json!({ "integrity_check": result }),
            fix_hint: if status == CheckStatus::Fail {
                Some("Run `sovereign recover` or restore from Litestream".into())
            } else { None },
        })
    }
}
```

The 14 checks are added incrementally across the phases:
- **V0:** system, binary, storage
- **V1:** + runtime, proxy, secrets, backup, network, observability, security
- **V1.5:** + agents
- **V2:** + sovereignty, performance, cost

The `--fix` orchestrator maps `(category, name) -> auto_fix_action`. The mapping is in `sovereign-doctor/src/fixes/`. Each fix is a function `async fn(&AppState) -> Result<FixResult, AppError>`.

---

## 10. The doctor metrics

Doctor emits metrics for monitoring:

```text
# Counter
sovereign_doctor_runs_total{level, status}

# Histogram
sovereign_doctor_check_duration_seconds{category, name}

# Gauge
sovereign_doctor_last_run_timestamp{level}
sovereign_doctor_last_status{level}  # 0=pass, 1=warn, 2=fail
```

These are emitted regardless of the doctor level. The `--watch` mode is the only mode that emits them in real-time.

---

## 11. The doctor CLI (V1+)

The full command tree:

```text
sovereign doctor                                    # default: --level=standard
sovereign doctor --level=<basic|standard|full|paranoid|custom>
sovereign doctor --category=<name>                  # run one category
sovereign doctor --explain <check-name>             # detailed explanation
sovereign doctor --fix                              # auto-fix common issues
sovereign doctor --fix --yes                        # skip confirmations
sovereign doctor --report=<file>                    # generate support report
sovereign doctor --json                             # JSON output
sovereign doctor --watch                            # continuous monitoring
sovereign doctor --watch --interval=<seconds>       # custom interval
sovereign doctor --watch --alert-on-fail            # alert on transition
sovereign doctor --since=<duration>                 # stateful checks only
sovereign doctor --no-color                        # no ANSI
```

The incident toolkit:

```text
sovereign incident declare --severity=<sev1|sev2|sev3> --title=<text> --commander=<user>
sovereign incident note --incident=<id> --note=<text>
sovereign incident resolve --incident=<id> --root-cause=<text>
sovereign incident list --since=<duration>
sovereign incident show <id>

sovereign retro --incident=<id> --output=<file>

sovereign game-day schedule --date=<YYYY-MM-DD> --scenario=<name> --duration=<duration>
sovereign game-day run --scenario=<name> --target=<server>
sovereign game-day list-scenarios
```

---

## 12. The doctor + the morning-report

`sovereign morning-report` (V1) and `sovereign doctor` are complementary:

- **`morning-report`** is a 10-line summary: "is everything OK?"
- **`doctor`** is the 14-category check: "is everything OK, and why?"

The morning report is the first thing the operator sees. If it shows a warning, the operator runs `sovereign doctor --explain <check>` to drill in.

```bash
# In the daily routine
sovereign morning-report
# ✓ all green, 0 alerts

# Or:
sovereign morning-report
# ⚠ proxy cert expiring in 12 days

# Drill in:
sovereign doctor --category=proxy
# ...
sovereign certs renew api.example.com
```

---

## 13. The doctor + the support ticket

When a design partner files a support ticket, the first request is "please run `sovereign doctor --level=full --report=<file>` and attach the report." The report is a single markdown file with everything the support engineer needs:

- The full check output (all 14 categories)
- The sovereign version + commit hash
- The host's resource snapshot
- The recent logs (filtered, redacted)
- The audit log for the relevant time window
- A "what I tried" section (operator fills in)
- A redacted secrets section (keys, not values)

**The report is never uploaded by sovereign.** The operator attaches it to the ticket manually. This is privacy by design.

---

## 14. The doctor as the on-call's first action

The new on-call routine:

```bash
# 1. The page comes in
# 2. The on-call SSHes to the affected host
ssh api-1

# 3. Run doctor (10 seconds)
sovereign doctor

# 4. If everything green, the issue is in the deploy, not the platform
sovereign status <app>
sovereign logs <app> --since 5m

# 5. If doctor shows an issue, drill in
sovereign doctor --explain <failing-check>

# 6. Auto-fix if possible
sovereign doctor --fix

# 7. If the issue is real (not a doctor false-positive), declare an incident
sovereign incident declare --severity=sev1 --title="..." --commander=alice

# 8. Take action (rollback, scale, etc.)
sovereign rollback <app>

# 9. Resolve the incident
sovereign incident resolve --incident=<id> --root-cause="..."

# 10. Generate the retro
sovereign retro --incident=<id> --output=retro-<id>.md
```

This is the **3am test** for the ops story. The doctor is the entry point. The incident toolkit is the structure. The retro is the learning.

---

**Next: the doctor is referenced throughout. The most relevant files are [`operations-runbook.md`](./operations-runbook.md) (which uses doctor as the first command), [`phase-00-mvp.md`](./phase-00-mvp.md) (which adds the basic doctor to V0), [`phase-01-v1.md`](./phase-01-v1.md) (which adds the full doctor to V1), [`phase-02-v15.md`](./phase-02-v15.md) (which adds fleet + remote + incident + retro to V1.5), and [`phase-03-v2.md`](./phase-03-v2.md) (which adds the paranoid + sovereignty + canary + auto-tune to V2).**
