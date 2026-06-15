# Sovereign Application Runtime — Deep Persona Research (Research-3)

**Date:** 2026-06-03
**Product:** Rust-based, self-hosted, single-binary, CLI/TUI/API-first deployment platform
**Scope:** Seven persona lenses synthesized into one actionable, opinionated plan
**Companion docs:** `Research-1.txt` (spec), `Research-2.md` (consolidated synthesis), `Research-Report-1.md` (tech), `competitive-landscape.md`, `user-pain-research.md`, `sovereign-runtime-market-research.md`, plus 7 persona files (`persona-pm.md`, `persona-principal.md`, `persona-rust.md`, `persona-devops.md`, `persona-platform.md`, `persona-cto.md`, `persona-crosscutting.md`)

---

## 0. The through-line

**One operator. One binary. Five years. Sovereign by construction. Governed by rules + ML, not one or the other. Dev-friendly enough that an AI agent is a first-class user.**

Every recommendation in this document is filtered through four invariants:

1. **One operator can run it for 5 years.** Not three SREs. Not a rotation. One human on a Monday morning. This kills Kubernetes, kills service mesh, kills any feature requiring tribal knowledge to operate.
2. **A product in 6 months, not a platform in 36.** Scope is the moat. Every section is opinionated about what to *cut* as much as what to *add*.
3. **"Sovereign" is a structural claim, not a feature.** A Swiss-German non-profit that publishes SBOMs, ships in-tree EU defaults, and can drop US dependencies on 30 days' notice *is* sovereign. A US LLC with encryption-at-rest is not. Sovereignty is decided by the corporate graph, not the feature list.
4. **The CLI is the product.** TUI is the daily-driver bonus. Web UI is a V2 opt-in for non-terminal users. Documentation is auto-generated from the CLI parser, so it never lies.

The seven lenses, ranked by how much they should shape the V1 code:
- **#1 Principal Engineer** (architecture, data model, failure modes) — defines the load-bearing wall
- **#2 Rust Engineer** (crate stack, perf, testing) — defines the build
- **#3 PM** (dev journey, on-ramp, time-to-first-deploy) — defines whether anyone uses it
- **#4 DevOps** (incident response, SLOs, runbooks) — defines whether it survives production
- **#5 Platform** (golden paths, service catalog, IDP philosophy) — defines whether teams adopt it
- **#6 Cross-cutting** (governance, USP, sovereignty as engineering) — defines the brand
- **#7 CTO** (positioning, funding, moat) — defines whether the company survives

---

## 1. The seven-persona verdict table

| Lens | Top recommendation | Why it matters | V1 consequence |
|---|---|---|---|
| **PM** | CLI is the product; TUI is second-act; web is third-act. Time-to-first-deploy < 5 min. | The "5-minute moment" (push → live URL) is the only thing the user opens the tool to do. Everything else is overhead. | Onboarding = `install` → `init` → `login` → `deploy` → URL. 5 commands. No Docker, no nginx, no certbot thinking. |
| **Principal** | Hexagonal architecture with `Arc<dyn Port>` DI. SQLite (V1) → rqlite (V2). URL-prefix `/v1/`. RFC 9457 errors. Stripe-style idempotency keys. | Architecture done wrong = rewrite the control plane. Done right = every other decision is recoverable. | Cargo workspace split: `sovereign-core` (domain) + `sovereign` (binary) + 14 adapter crates. The composition root is the only place that knows concrete types. |
| **Rust** | `axum 0.8 + tokio 1 + sqlx 0.8 + ratatui 0.30 + clap 4.6`. MSRV 1.85 (RPITIT + GATs). `mimalloc` for musl, `jemalloc` for glibc. `thiserror` library, `anyhow` binary. | The 2026 default for serious Rust services. axum's Tower middleware ecosystem is the single biggest reason it wins for a control plane. | `.cargo/config.toml` with `[profile.release] lto="fat" codegen-units=1 panic="abort" strip="symbols"`. CI runs `cargo-msrv verify`. |
| **DevOps** | VictoriaMetrics + VictoriaLogs + Grafana (not LGTM). SOPS+age, not Vault. Monthly restore drill. 10 max alerts, each with a runbook. `runtime morning-report` for the 5-command daily. | The GitLab 2017-01-31 "8 months of empty backups" is the lesson. Backups you haven't restored are hopes. | `tool backup verify --restore-to scratch` is the single most important V1.2 command. SLOs: 99.5% / 300ms p99 / 60s freshness. |
| **Platform** | `RuntimeState` trait (SQLite V1 → rqlite V2). 6 golden-path templates (FastAPI, Next.js, Laravel, Go, Rails, Astro). 16-field service catalog. No plugin system. | The 80% of 1-50 engineer orgs don't need an IDP. They need a deployment runtime that *feels* like one when they're small. | `tool golden-check` in V1.2. Catalog export to Backstage YAML in V1.5. Plugin system is an anti-feature. |
| **Cross-cutting** | Rego rules + Prophet/TimesFM anomaly scoring, layered. Single main USP for 2 years. Sovereignty as CI gate (10-point test in release pipeline). | Strict governance scales past 100 services only with both rules and ML. Pure rules = false positives; pure ML = un-auditable. | Layer 1 telemetry + Layer 2 ML scorer + Layer 3 Rego rules + Layer 4 human override. 6-month staged rollout. |
| **CTO** | Apache 2.0 unmodified, no carve-outs. Bootstrap to €1M ARR, then sovereign-tech EU seed. EU incorporation (Berlin or Tallinn). "Plausible of deployment" positioning. | Dokploy's license change and CasaOS's abandonment are the cautionary tales. Sovereignty is the brand; the corporate graph proves it. | `LICENSE` = Apache 2.0 only. No `proprietary/` dir. Public commit history. EU-incorporated company. 18-month sustainability signal on day 1. |

---

## 2. The day in the life — five users, five workflows

Before any code, here is what the product must enable, day to day.

### 2.1 Mira, 32, solo founder of a B2B SaaS

1 Hetzner CX32 (4 vCPU, 8 GB) in Falkenstein. 1 Postgres, 1 Redis, 4 apps. 4–6 deploys/day. Last SSH 11 days ago. On call.

| Time | Command | What happens |
|---|---|---|
| 9:00 AM | `tool deploy api --strategy bluegreen --wait` | 7-step pipeline ("pulling image…", "warming healthcheck…", "shifting traffic…"), progress bar, green ✓, URL. 38 seconds. |
| 9:14 AM | `tool status --public` | Status page auto-published at `status.mirasaas.com` shows 100% uptime. She links it in a customer-support email. |
| 11:42 AM | `tool logs api --since 30m --level error` | Three rows of error logs. She SSHs in only to grep more. Or runs `tool tui` and uses `/` to search. |
| 2:00 PM | `tool access add --app staging --role dev --email freela@x.io` | Freelancer gets a one-time link, scoped token, no SSH. |
| 5:47 PM | `tool backup verify --app postgres --restore-to scratch` | "restored 1.2 GB, 47,221 rows, 12 tables. 2 row counts differ from production (expected)." The most important 90s of her week. |
| 6:00 PM | `tool preview --pr 42` | `pr-42.mirasaas.com` with 24h TTL, URL returned, posted to Slack. |
| Saturday 9:00 AM | `tool db upgrade --check` → `tool db upgrade --apply` | Postgres major-version upgrade. 12-second downtime, auto-rollback if new version doesn't accept a connection in 5s. |

**The "10x/day vs 1x/year" map:**

| Frequency | Action | Why it matters |
|---|---|---|
| 10×/day | `deploy`, `status`, `logs` | Muscle memory; must be sub-50ms perceived |
| 5×/day | `secret set/get` (the AI agent calls this, not always Mira) | Safe to call from a script; structured, never echo plaintext |
| 2×/day | `preview` | Idempotent; TTL is critical |
| 1×/day | `backup verify`, `health` | The single highest-value feature, hidden until you need it |
| 1×/week | `rollback`, `db shell` | Rollback must be one command; db shell must drop into `psql` not a wrapper |
| 1×/month | `access add/revoke`, `audit` | Trust-builders |
| 1×/quarter | `db upgrade`, `migrate` to a new server | The "I'm not stuck" features |

### 2.2 Karim, 41, agency owner, 12 client sites

12 client sites on one Hetzner dedicated server. 1 ops engineer. 5 dev freelancers with partial access.

| Time | Command | What happens |
|---|---|---|
| 9:00 AM | `tool fleet` | TUI shows 12 client sites, status indicator per site, last deploy per site, response time p50. |
| 9:30 AM | `tool deploy client-x --all` | Deploys all 4 apps (web, api, worker, cron) for client-x simultaneously. |
| 10:00 AM | `tool access add --app client-x/api --role dev --email newdev@agency.io` | Onboards new freelancer in 30 seconds. |
| 11:00 AM | `tool audit --app client-x --since 7d` | Shows who deployed what. White-label audit report for client-x. |
| 2:00 PM | `tool backup verify --all` | Restores 12 Postgres databases to scratch, verifies row counts, 4 minutes total. |
| 4:00 PM | `tool fleet restart --status degraded` | Auto-restarts the 2 sites that have a degraded health check. |

### 2.3 Lin, 28, infra engineer at a 40-person startup

1 production cluster (3 servers), 1 staging (1 server), 1 dev (1 server). 5-week deploy cadence. Owns the on-call.

| Time | Command | What happens |
|---|---|---|
| 8:30 AM | `tool fleet --matrix` | TUI shows 3×5 matrix of apps × servers, with status, CPU, RAM. |
| 9:00 AM | `tool secrets rotate DATABASE_URL --all-envs` | Rotates DB password across dev/staging/prod, re-deploys affected apps, verifies. |
| 10:00 AM | `tool policy check --env prod` | Runs the Rego rules against current state. 0 violations. (Yesterday there were 3; fixed.) |
| 11:00 AM | `tool audit export --format csv --since 30d > audit-2026-05.csv` | Exports audit log for compliance review. |
| 2:00 PM | `tool db failover --to server-2` | Manual failover drill. 12 seconds. Auto-rolls back if new primary doesn't accept connections. |
| 4:00 PM | `tool preview --pr 247 --ttl 24h` | PR preview for the team. |
| Friday 5:00 PM | `tool weekly-report` | Auto-generated postmortem-less review: deploys, rollbacks, alerts, MTTR. |

### 2.4 Sara, 35, EU public-sector IT lead

BSI C5:2026 procurement requirement. 5 engineers, 1 security officer. Auditable everything.

| Time | Command | What happens |
|---|---|---|
| 9:00 AM | `tool sovereignty check` | Runs the 10-point sovereignty test in CI mode. 10/10. SBOM current. cosign signatures valid. Reproducible-build hash matches. |
| 10:00 AM | `tool audit export --format pdf --since 90d > audit-q2.pdf` | Quarterly audit report, ready for the security officer. |
| 11:00 AM | `tool compliance map --standard bsi-c5-2026` | Generates a BSI C5:2026 control-mapping report. 121 controls, mapped to product features, with gaps called out. |
| 2:00 PM | `tool access review --env prod` | Shows every user with prod access, last login, last action. Auto-flags anyone with no action in 90 days. |
| 4:00 PM | `tool vendor-disappear-test` | Spins up a fresh box, installs the binary from scratch, runs `tool restore --from <last-backup>`. Verifies that the platform is fully recoverable from a vendor-disappear scenario. |

### 2.5 Alex, 24, "vibe coder" shipping a weekend project

1 Hetzner CX22 (€4.49/mo). No team. AI agent (Claude Code) writes 80% of the code. Deploys via `git push`.

| Time | Command | What happens |
|---|---|---|
| Friday 10:00 PM | (Claude Code runs) `git push && tool deploy` | Auto-deploys via webhook. |
| Friday 10:01 PM | `tool status` | Green. URL printed. |
| Friday 10:05 PM | (Claude Code runs) `tool logs api --since 5m --level error` | No errors. |
| Friday 10:30 PM | `tool secret set STRIPE_KEY` | Prompts for value, encrypts, deploys. |
| Saturday 9:00 AM | (Claude Code runs) `tool rollback` | A bad deploy at 3 AM. Alex wakes up, sees the rollback, goes back to sleep. |
| Saturday 11:00 AM | (Claude Code runs) `tool backup verify` | Backup works. Restore drill passed. |

**The pattern across all five users:** the daily-driver interface is the CLI. The TUI is a bonus. The web UI is never the primary. The agent (human or AI) types commands; the tool obeys.

---

## 3. Architecture (Principal Engineer verdict)

### 3.1 The architecture that fits — hexagonal, with `Arc<dyn Port>` DI

```
┌──────────────────────────────────────────────────────┐
│                  Driving Adapters                    │
│   cli (clap)  │  tui (ratatui)  │  http/api (axum)  │
└────────────────────┬─────────────────────────────────┘
                     │ calls
                     ▼
┌──────────────────────────────────────────────────────┐
│                  Application Layer                   │
│   use_cases::deploy::start                          │
│   use_cases::rollback                               │
│   use_cases::secret::rotate                         │
│   use_cases::backup::verify                         │
│   use_cases::agent::dispatch                        │
│   use_cases::audit::record                          │
│   use_cases::policy::enforce                        │
│                                                      │
│   Owns: transaction boundaries, idempotency keys,   │
│         span emission. No I/O — only via ports.      │
└────────────────────┬─────────────────────────────────┘
                     │ calls
                     ▼
┌──────────────────────────────────────────────────────┐
│                    Domain Core                       │
│   domain::deployment, domain::secret, domain::server │
│   domain::app, domain::backup, domain::audit,       │
│   domain::policy, domain::agent                     │
│                                                      │
│   Pure Rust types (no sqlx, no reqwest, no anyhow). │
│   Enums with `can_transition_to(self, other)`.      │
│   Newtypes: AppId(Uuid), DeploymentId(Uuid).        │
└────────────────────┬─────────────────────────────────┘
                     │ calls
                     ▼
┌──────────────────────────────────────────────────────┐
│               Driven Adapters (Ports impl)          │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐    │
│  │ runtime     │ │ proxy       │ │ secrets     │    │
│  │  -docker    │ │  -caddy     │ │  -age       │    │
│  │  -podman    │ │  -nginx     │ │  -sops      │    │
│  │  -buildkit  │ │  -traefik   │ │  -vault-v2  │    │
│  │  -git       │ │  -acme      │ │             │    │
│  └─────────────┘ └─────────────┘ └─────────────┘    │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐    │
│  │ storage     │ │ backup      │ │ observability    │
│  │  -sqlite    │ │  -restic    │ │  -tracing    │   │
│  │  -rqlite    │ │  -pg_dump   │ │  -otel       │   │
│  │             │ │  -litestream│ │  -prometheus │   │
│  └─────────────┘ └─────────────┘ └─────────────┘    │
│  ┌─────────────┐ ┌─────────────┐                    │
│  │ notify      │ │ policy      │                    │
│  │  -webhook   │ │  -rego      │                    │
│  │  -email     │ │  -ml-scorer │                    │
│  │  -telegram  │ │             │                    │
│  │  -slack     │ │             │                    │
│  └─────────────┘ └─────────────┘                    │
└──────────────────────────────────────────────────────┘
```

**Dependency rule (enforced by `cargo-deny` and review):**
- Domain never imports from Application.
- Application never imports from Adapters.
- Adapters never import from each other except through ports.
- The composition root (`main.rs`) is the only place that knows which concrete adapter is wired in.

### 3.2 Cargo workspace layout

```text
sovereign/
├── Cargo.toml                      # workspace root
├── deny.toml                       # cargo-deny config
├── crates/
│   ├── sovereign/                  # binary crate (the `tool` you run)
│   │   ├── src/
│   │   │   ├── main.rs             # composition root
│   │   │   ├── cli.rs              # clap definitions
│   │   │   ├── tui/                # ratatui dashboard
│   │   │   ├── http/               # axum router + middleware
│   │   │   ├── config/             # config loading + validation
│   │   │   └── shutdown.rs         # SIGTERM/SIGINT handling
│   ├── sovereign-core/             # domain types + use cases
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── domain/             # pure types, no I/O
│   │   │   ├── use_cases/          # application services
│   │   │   ├── ports/              # trait definitions only
│   │   │   └── error.rs            # thiserror enums
│   ├── sovereign-runtime-docker/   # adapter
│   ├── sovereign-runtime-podman/   # adapter
│   ├── sovereign-proxy-caddy/      # adapter
│   ├── sovereign-proxy-nginx/      # adapter (low-mem)
│   ├── sovereign-secrets-age/      # adapter
│   ├── sovereign-storage-sqlite/   # adapter (V1)
│   ├── sovereign-storage-rqlite/   # adapter (V2)
│   ├── sovereign-buildkit/         # adapter
│   ├── sovereign-git/              # adapter (gix)
│   ├── sovereign-acme/             # adapter
│   ├── sovereign-backup/           # adapter
│   ├── sovereign-notify/           # adapter
│   ├── sovereign-policy-rego/      # adapter
│   ├── sovereign-ml-scorer/        # adapter (Prophet/TimesFM)
│   ├── sovereign-observability/    # tracing + /metrics
│   └── sovereign-proto/            # shared types
├── migrations/                     # SQL migrations (sqlx migrate)
│   ├── 0001_init.sql
│   ├── 0002_audit.sql
│   └── ...
├── docs/
│   ├── adr/                        # Architecture Decision Records
│   ├── openapi.yaml                # generated API spec
│   └── operations/                 # runbooks
└── .github/
    └── workflows/
        ├── ci.yml
        ├── release.yml
        └── sbom.yml
```

### 3.3 Data flow — a deploy, end to end

```text
User → CLI (clap) → HTTP API (axum) → use_cases::deploy::start
   │                                    │
   │                                    ├─► use_cases::policy::enforce  (Rego rules)
   │                                    ├─► use_cases::policy::score    (ML scorer)
   │                                    ├─► ports::Storage::begin_tx
   │                                    ├─► domain::Deployment::new(...)
   │                                    ├─► ports::Storage::commit
   │                                    └─► ports::AgentPort::dispatch  (or RuntimePort in V1)
                                              │
                                              ▼
                                     Agent / Docker runtime
                                              │
                                              ▼
                                     Container pulled, started, healthchecked
                                              │
                                              ▼
                                     ports::ObservabilityPort::record
                                     ports::AuditPort::append
```

### 3.4 The data model (SQLite + WAL, forward-only migrations)

```sql
-- 0001_init.sql
CREATE TABLE app (
  id           TEXT PRIMARY KEY,         -- AppId(Uuid)
  name         TEXT NOT NULL UNIQUE,
  owner        TEXT NOT NULL,
  env          TEXT NOT NULL CHECK (env IN ('dev','staging','prod')),
  git_repo     TEXT,
  image_ref    TEXT,                      -- last deployed image
  config_yaml  TEXT NOT NULL,             -- last app.yaml
  health_path  TEXT,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL,
  version      INTEGER NOT NULL DEFAULT 1 -- optimistic concurrency
);

CREATE TABLE deployment (
  id            TEXT PRIMARY KEY,
  app_id        TEXT NOT NULL REFERENCES app(id),
  image_ref     TEXT NOT NULL,
  strategy      TEXT NOT NULL CHECK (strategy IN ('recreate','rolling','bluegreen')),
  status        TEXT NOT NULL CHECK (status IN ('pending','building','pushing','starting','healthy','failed','rolled_back')),
  started_at    INTEGER NOT NULL,
  finished_at   INTEGER,
  triggered_by  TEXT NOT NULL,            -- user:alice | system | web:github
  risk_score    INTEGER,                  -- 0-100, from ML scorer
  policy_decision TEXT,                   -- JSON: {allow, deny, requires_approval, reason}
  error         TEXT,
  version       INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_deployment_app ON deployment(app_id, started_at DESC);

CREATE TABLE domain (
  id           TEXT PRIMARY KEY,
  app_id       TEXT NOT NULL REFERENCES app(id),
  hostname     TEXT NOT NULL UNIQUE,
  tls_status   TEXT NOT NULL,             -- provisioning, valid, expired
  tls_expires  INTEGER,
  created_at   INTEGER NOT NULL
);

CREATE TABLE secret (
  id           TEXT PRIMARY KEY,
  app_id       TEXT NOT NULL REFERENCES app(id),
  key          TEXT NOT NULL,
  ciphertext   BLOB NOT NULL,             -- age-encrypted
  created_at   INTEGER NOT NULL,
  rotated_at   INTEGER,
  version      INTEGER NOT NULL DEFAULT 1,
  UNIQUE (app_id, key)
);

CREATE TABLE server (
  id           TEXT PRIMARY KEY,
  hostname     TEXT NOT NULL UNIQUE,
  role         TEXT NOT NULL CHECK (role IN ('control_plane','agent')),
  api_url      TEXT NOT NULL,
  status       TEXT NOT NULL,             -- healthy, unreachable, drained
  last_seen    INTEGER NOT NULL,
  cpu_cores    INTEGER,
  mem_mb       INTEGER,
  disk_gb      INTEGER,
  joined_at    INTEGER NOT NULL
);

CREATE TABLE backup (
  id           TEXT PRIMARY KEY,
  target       TEXT NOT NULL,             -- 'postgres:api'
  status       TEXT NOT NULL,             -- pending, success, failed, verified
  size_bytes   INTEGER,
  location     TEXT,                      -- s3://..., /var/lib/sovereign/backups/...
  started_at   INTEGER NOT NULL,
  finished_at  INTEGER,
  verified_at  INTEGER,                   -- last restore-drill time
  verify_result TEXT                      -- JSON: {row_count_diff: [...], tables: [...]}
);

CREATE TABLE user (
  id           TEXT PRIMARY KEY,
  email        TEXT NOT NULL UNIQUE,
  role         TEXT NOT NULL CHECK (role IN ('owner','admin','developer','readonly')),
  created_at   INTEGER NOT NULL,
  last_seen    INTEGER
);

-- 0002_audit.sql — append-only, never updated, never deleted
CREATE TABLE audit_event (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  ts           INTEGER NOT NULL,
  actor        TEXT NOT NULL,             -- 'user:alice' | 'system' | 'ml:scorer'
  kind         TEXT NOT NULL,             -- 'deploy','rollback','secret.set','access.add',...
  target       TEXT,                      -- 'app:api', 'backup:xyz'
  payload      TEXT NOT NULL,             -- JSON
  policy_decision TEXT
) WITHOUT ROWID;

-- Enforce append-only via trigger
CREATE TRIGGER audit_no_update BEFORE UPDATE ON audit_event
BEGIN SELECT RAISE(ABORT, 'audit_event is append-only'); END;
CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit_event
BEGIN SELECT RAISE(ABORT, 'audit_event is append-only'); END;
```

### 3.5 State machines — every entity has one

```rust
// domain::deployment
pub enum DeploymentStatus {
    Pending, Building, Pushing, Starting, Healthy, Failed, RolledBack,
}

impl DeploymentStatus {
    pub fn can_transition_to(self, other: Self) -> bool {
        use DeploymentStatus::*;
        matches!((self, other),
            (Pending, Building) | (Building, Pushing) | (Pushing, Starting) |
            (Starting, Healthy) | (Starting, Failed) |
            (_, Failed) | (Healthy, RolledBack) | (Failed, RolledBack) |
            (RolledBack, Building)  // a re-deploy from a rolled-back state
        )
    }
}
```

Every other state (App, Secret, Backup, Server) has the same pattern. State transitions are the *first* code written, not the last.

### 3.6 API design — REST + JSON, URL-prefix `/v1`, RFC 9457 errors

```text
GET    /v1/apps                       list apps
POST   /v1/apps                       create app (idempotency-key required)
GET    /v1/apps/{id}                  get app
PATCH  /v1/apps/{id}                  update app (If-Match: version)
DELETE /v1/apps/{id}                  delete app (soft)
GET    /v1/apps/{id}/deployments      list deployments
POST   /v1/apps/{id}/deployments      start deploy (idempotency-key required)
GET    /v1/deployments/{id}           get deployment
POST   /v1/deployments/{id}/rollback  roll back to this version

GET    /v1/apps/{id}/secrets          list secret keys (never values)
PUT    /v1/apps/{id}/secrets/{key}    set secret (zero-disk, never logged)
GET    /v1/apps/{id}/logs?tail=N&follow=true   SSE stream

GET    /v1/backups                    list backups
POST   /v1/backups                    create backup
POST   /v1/backups/{id}/verify        restore-drill to scratch
POST   /v1/backups/{id}/restore       restore from backup

GET    /v1/servers                    list servers
POST   /v1/servers                    add server
DELETE /v1/servers/{id}               remove server

GET    /v1/audit?since=...&actor=...  query audit log

GET    /v1/policy/rules               list Rego rule packs
PUT    /v1/policy/rules/{pack}        upload rule pack
POST   /v1/policy/check               dry-run policy check
```

**Conventions (every endpoint, no exceptions):**
- Idempotency-Key header on every mutating request (Stripe-style UUIDs)
- ETag/If-Match for optimistic concurrency
- Cursor pagination (`?cursor=...&limit=50`), never offset
- RFC 9457 problem+json for errors:
  ```json
  {"type": "/errors/conflict", "title": "Concurrent modification",
   "status": 409, "detail": "app api was modified by user:alice at 14:22:03",
   "instance": "/v1/apps/abc", "current_version": 7}
  ```
- Sparse fieldsets (`?fields=id,name,status`)
- OpenAPI 3.1 spec auto-generated and committed to `/docs/openapi.yaml`

### 3.7 Failure modes — what fails, how we recover

| Failure | Detection | Mitigation | Recovery |
|---|---|---|---|
| Docker daemon crashes | RuntimePort::ping() fails | Retry 3x, then drain | Restart via systemd; agent re-registers |
| Caddy crashes | JSON admin `/config/` returns 502 | Failover to Nginx (V1.1) | Restart via systemd; routes reload from state |
| SQLite corruption | sqlx returns `DatabaseCorrupt` | Restore from Litestream backup | Point-in-time recovery to last good state |
| Server host dies | Heartbeat > 30s | Mark server `drained`; reschedule workloads | Rqlite HA (V2); new server `tool server add` |
| TLS cert expires | Health check detects `tls_status: expired` | Auto-renew via Caddy ACME | If renew fails: alert (Sev2), manual fix |
| Secret rotation breaks deploy | Post-deploy health check fails | Auto-rollback to previous version | Operator investigates; policy suggests fix |
| Bad deploy (memory leak) | ML scorer detects RSS growth > baseline | Canary at 5% → check error rate → full rollout or rollback | Auto-rollback; policy learns |
| Disk full | df returns > 90% | Alert (Sev2); auto-cleanup of old logs/backups | Operator adds disk; Litestream catches up |
| Network partition (server unreachable) | Heartbeat > 30s | Mark `unreachable`; deploy to other servers first | Partition heals; agent re-registers |

### 3.8 Multi-tenancy — soft isolation, one DB, tenant_id column

The spec says "solo developers, freelancers, agencies, startups." The implication: **soft multi-tenancy, not hard isolation.** One SQLite, one binary, multiple users, role-based access. No "tenant ID" column; users are scoped via `user.role` (owner, admin, developer, readonly) and `app.owner`. The single-tenant case is just the trivial multi-tenant case.

**Why not hard isolation?** It would mean: separate processes per tenant, separate SQLite per tenant, separate everything. That's Coolify's per-app Docker setup, and it doesn't scale operationally beyond ~30 apps without a platform team.

**Multi-team in one binary:** the agency use case (Karim with 12 clients) is "12 teams, one binary." Each `app.owner` is a client. The agency admin can deploy to any client; the client dev can only deploy to their own.

---

## 4. Rust engineering (Rust Engineer verdict)

### 4.1 The locked stack (with reasoning)

| Category | Pick | Why | Anti-pick |
|---|---|---|---|
| Async runtime | **tokio 1.45+** | Default ecosystem; axum/sqlx/bollard all built for it | async-std (discontinued Mar 2025), smol, glommio |
| HTTP server | **axum 0.8** | Tower middleware ecosystem is the single biggest reason | actix-web (fights tokio work-stealing), salvo, poem, rocket |
| HTTP client | **reqwest 0.12** (rustls-tls) | Default; no OpenSSL drag | isahc, ureq |
| Database | **sqlx 0.8** (compile-time checked queries) | Async-native, multi-driver, embeddable | rusqlite (sync, needs spawn_blocking), diesel (macro-heavy), sea-orm (ORM overhead) |
| Serialization | **serde 1 + serde_json + serde_yaml + postcard** | Standard | simd-json (unsafe), bincode (worse for IPC) |
| CLI | **clap 4.6 derive + clap_complete + clap_mangen** | Free --help, shell completions, man pages | argh, lexopt, structopt (deprecated) |
| Config | **figment 0.10 with TOML** | Layered (defaults → file → env → CLI); type-safe | config, confy, envy |
| Logging | **tracing 0.1 + tracing-subscriber (env-filter, fmt, json)** | Standard, structured, span-aware | slog, log, fern (gone) |
| Metrics | **metrics 0.24 facade + metrics-exporter-prometheus** | Backend-agnostic; user picks exporter | prometheus (tying to one backend) |
| Errors | **thiserror 2 (library) + anyhow 1 (binary)** | Canonical split | snafu, eyre, miette (overkill) |
| TUI | **ratatui 0.30 + crossterm 0.28** | De-facto standard; k9s/lazydocker/yoink pattern | cursive, termion (unmaintained) |
| Secrets | **age + argon2** | Modern, simple, no GPG | orion, ring (low-level) |
| Compression | **flate2, lz4, zstd** | Standard | brotli (slow), snap |
| Containers | **bollard 0.18** | Tokio-native Docker API | docker-rust (older) |
| Build | **cargo + cargo-dist + cargo-binstall** | Reproducible, cross-platform | cargo-make (overkill) |
| Allocator | **mimalloc (musl target) + jemalloc (glibc target)** | Recover 15-60% on alloc-heavy workloads | std (regression on musl) |
| Testing | **tokio-test, mockall, wiremock, testcontainers, proptest, criterion, divan, insta, trybuild, cargo-fuzz, cargo-mutants, cargo-llvm-cov, codspeed** | The 2026 standard stack | None of the above is "wrong" per se; the list is what works |
| Linting | **clippy, cargo-deny, cargo-audit, cargo-machete, geiger** | The discipline of release-grade Rust | None |
| MSRV | **1.85** (RPITIT + GATs stable) | Plain `async fn` in trait; type-state APIs | 1.80 (RPITIT nightly only), 1.90 (newer) |

### 4.2 The Cargo.toml release profile (10-25 MB binary, not 30+)

```toml
[profile.release]
opt-level = "z"       # or 3 for speed; "z" for size
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "symbols"
incremental = false
```

```toml
[workspace.metadata.cargo-dist]
targets = ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"]
```

```toml
[workspace.package]
rust-version = "1.85"
edition = "2024"
license = "Apache-2.0"
repository = "https://github.com/<org>/sovereign"
```

### 4.3 The global allocator (one line, 15-60% perf win)

```rust
// src/main.rs
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(not(target_env = "musl"))]
#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;
```

### 4.4 Type system opportunities — make the API impossible to misuse

```rust
// Newtype wrappers for IDs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type, serde::Serialize, serde::Deserialize)]
#[sqlx(transparent)]
pub struct AppId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type)]
#[sqlx(transparent)]
pub struct DeploymentId(pub Uuid);

// Type-state for the deploy lifecycle
pub struct Deploy<P: Phase> { id: DeploymentId, _phase: PhantomData<P> }
pub trait Phase { fn next(self) -> impl Phase; }
pub struct Pending; pub struct Building; pub struct Pushing; pub struct Starting; pub struct Healthy;
impl Phase for Pending { fn next(self) -> impl Phase { Building } }
// ... etc.

// Builder with required fields
#[derive(typed_builder::TypedBuilder)]
pub struct DeployRequest {
    app: AppId,
    #[builder(default, setter(strip_option))]
    image: Option<ImageRef>,
    #[builder(default = "Strategy::BlueGreen")]
    strategy: Strategy,
    #[builder(default)]
    wait: bool,
}

// Sealed trait for adapters (only our crates can implement)
mod sealed { pub trait Sealed {} }
pub trait Runtime: Send + Sync + sealed::Sealed {
    async fn deploy(&self, spec: &ContainerSpec) -> Result<ContainerId, RuntimeError>;
    async fn stop(&self, id: &ContainerId) -> Result<(), RuntimeError>;
    async fn logs(&self, id: &ContainerId, opts: LogOpts) -> Result<LogStream, RuntimeError>;
    // ...
}
```

### 4.5 Error handling — thiserror for the library, anyhow for the binary

```rust
// sovereign-core/src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("app {0} not found")]
    NotFound(AppId),
    #[error("app {0} was modified concurrently (current version: {1})")]
    Conflict(AppId, u64),
    #[error("validation: {0}")]
    Validation(String),
    #[error("upstream error: {0}")]
    Upstream(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("internal: {0}")]
    Internal(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("auth: {0}")]
    Auth(String),
}

impl axum::IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::Conflict(_, _) => (StatusCode::CONFLICT, "conflict"),
            AppError::Validation(_) => (StatusCode::BAD_REQUEST, "validation"),
            AppError::Upstream(_) => (StatusCode::BAD_GATEWAY, "upstream"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
            AppError::Auth(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
        };
        (status, Json(json!({"error": code, "message": self.to_string()}))).into_response()
    }
}
```

### 4.6 CLI design — clap 4.6, every flag, every convention

```rust
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(version, about = "Sovereign Application Runtime", long_about = None,
          after_long_help = "Tips:
  • Add a custom domain: tool domain add api.example.com
  • Roll back the last release: tool rollback api
  • See the diff: tool diff api
  • List apps in TUI: tool tui
  • Restore a backup: tool restore <backup-id>")]
struct Cli {
    /// Control plane URL (default: http://127.0.0.1:7878)
    #[arg(long, env = "SOVEREIGN_URL", global = true)]
    url: Option<String>,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Text, global = true)]
    format: OutputFormat,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Deploy an app from a local source or pre-built image
    Deploy {
        #[arg(long)] app: String,
        #[arg(long)] image: Option<String>,
        #[arg(long, value_enum, default_value_t = Strategy::BlueGreen)]
        strategy: Strategy,
        #[arg(long)] wait: bool,
        #[arg(long)] dry_run: bool,
    },
    /// Roll back to a previous deployment
    Rollback {
        app: String,
        #[arg(long)] to: Option<String>,
        #[arg(long)] dry_run: bool,
    },
    /// Stream logs
    Logs {
        app: String,
        #[arg(long, default_value_t = 100)] tail: usize,
        #[arg(long)] follow: bool,
        #[arg(long)] level: Option<LogLevel>,
        #[arg(long)] since: Option<String>,
    },
    /// Show app status
    Status {
        #[arg(long)] app: Option<String>,  // None = fleet
        #[arg(long)] public: bool,         // status page
    },
    /// Open TUI dashboard
    #[cfg(feature = "tui")]
    Dashboard,
    /// Verify a backup
    BackupVerify {
        #[arg(long)] app: String,
        #[arg(long)] restore_to: String,
    },
    /// ... (60+ more subcommands, all with --json, --dry-run, --yes, --format)
}
```

**Conventions baked in from day one:**
- `--json` on every command; auto-detected via `std::io::IsTerminal` (human = colored, pipe = JSON)
- `--dry-run` on every mutating command
- `--yes` / `-y` to skip confirmations
- Env vars: `SOVEREIGN_URL`, `SOVEREIGN_TOKEN`, `SOVEREIGN_CONFIG`
- Exit codes: 0 success, 1 generic, 2 usage, 3 auth, 4 not_found, 5 conflict, 6 upstream
- Shell completions: bash, zsh, fish, nushell, powershell (`clap_complete`)
- Man pages: `clap_mangen` → installed by `tool completions install`

### 4.7 Observability in Rust — tracing + metrics + /metrics endpoint

```rust
// sovereign-observability/src/lib.rs
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sovereign=debug"));
    
    let fmt_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .json();
    
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .init();
    
    // Prometheus exporter
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("install prometheus");
}

// In every async function
#[tracing::instrument(skip(self), fields(app_id = %req.app_id))]
async fn deploy(&self, req: DeployRequest) -> Result<DeploymentId, AppError> {
    // ...
    metrics::counter!("deploys_total", "app" => req.app.to_string()).increment(1);
    // ...
}
```

### 4.8 Database access — sqlx, embedded migrations, optimistic concurrency

```rust
// In a use case
async fn start_deploy(state: &AppState, req: DeployRequest) -> Result<DeploymentId, AppError> {
    let mut tx = state.db.begin().await?;
    
    // Optimistic concurrency: lock the app row
    let app = sqlx::query_as!(App, r#"
        SELECT id, name, owner, env, version FROM app WHERE id = ? AND version = ?
    "#, req.app_id, req.expected_version)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::Conflict(req.app_id, req.expected_version))?;
    
    // Insert the deployment
    let dep_id = DeploymentId(Uuid::new_v4());
    sqlx::query!(r#"
        INSERT INTO deployment (id, app_id, image_ref, strategy, status, started_at, triggered_by, version)
        VALUES (?, ?, ?, ?, 'pending', ?, ?, 1)
    "#, dep_id, app.id, req.image.as_ref().map(|i| &i.0).unwrap_or(""), req.strategy.to_string(),
         now(), format!("user:{}", req.actor))
    .execute(&mut *tx)
    .await?;
    
    // Bump the app version
    sqlx::query!("UPDATE app SET version = version + 1, updated_at = ? WHERE id = ?",
                 now(), app.id)
    .execute(&mut *tx)
    .await?;
    
    tx.commit().await?;
    Ok(dep_id)
}
```

### 4.9 Testing — the 2026 stack

| Tool | Purpose |
|---|---|
| `#[tokio::test]` | Async tests |
| `mockall` | Mock the `Runtime`, `Proxy`, `Storage` ports |
| `wiremock` | Mock HTTP servers (e.g., Docker daemon, Caddy admin) |
| `testcontainers` | Real Postgres / Caddy / S3 in tests |
| `proptest` | Property-based testing for state machines |
| `insta` | Snapshot testing for CLI output and JSON |
| `criterion` / `divan` | Benchmarks |
| `codspeed` / `bencher` | Continuous benchmarking in CI |
| `cargo-fuzz` | LibFuzzer / AFL++ for untrusted input |
| `cargo-mutants` | Mutation testing |
| `cargo-llvm-cov` | Code coverage with branch data |
| `trybuild` | Compile-fail tests for macro/derive APIs |
| `rstest` | Fixture-based test parameters |

### 4.10 Compile time — keep it manageable

- `sccache` in CI
- `mold` linker
- `lld` linker
- `RUSTFLAGS="-C debuginfo=0"` for non-debug builds
- `cargo-hakari` for workspace dependency unification
- Feature flags per adapter (`--features docker,caddy,sqlite`)

---

## 5. DevOps reality (DevOps verdict)

### 5.1 The five morning commands

```bash
# 1. Orchestrator health
tool status --watch

# 2. Host resource sanity
ssh node-1 -- 'uptime && free -h && df -h /var/lib/sovereign && ss -s'

# 3. Recent error rate
tool logs --since 1h --level error --tail 200

# 4. Last 24h restarts
ssh node-1 -- 'journalctl -u sovereign --since "24 hours ago" -q | grep -c "Started"'

# 5. Certificate expiry
tool certs list --warn-days 21
```

**Ship as `tool morning-report` and bind to tmux on every operator's laptop.** 20 lines of shell. Eliminates "I didn't notice the disk was full."

### 5.2 The 3am test — every alert must be actionable

Before any alert pages, the runbook must answer:
1. What user-visible symptom?
2. What's the blast radius?
3. What's the first command to run, and what does the expected output look like?
4. What's the second command if the first doesn't work?

If you can't answer all four, **delete the alert.** Synthetic monitoring, dashboards, and weekly review catch the rest.

### 5.3 The 10 alerts that ship in V1

| Alert | Condition | Severity | Runbook |
|---|---|---|---|
| Health check fail | 3 consecutive 5xx | Sev1 | `tool rollback`; check container logs; check upstream |
| Disk > 90% | `df -h` | Sev2 | `tool log rotate`; `tool backup prune`; expand disk |
| Cert expiring < 14d | cert expiry | Sev2 | `tool cert renew`; check Caddy |
| Backup failure | backup job exit ≠ 0 | Sev1 | `tool backup list --failed`; investigate |
| Server unreachable | heartbeat > 30s | Sev1 | `tool server ls`; SSH check |
| Memory > 90% | RSS | Sev2 | check for leak; restart if needed |
| CPU > 95% (5m) | load avg | Sev3 | check for runaway process |
| Deploy failure | deploy status = failed | Sev3 | `tool rollback`; check logs |
| Error rate > 5% | 5xx / total | Sev1 | `tool logs --level error`; ML scorer suggests |
| p99 latency > 1s | response time | Sev2 | check upstream, DB, cache |

**Burn-rate alerting (Google SRE):** page on 2% error-budget burn in 1h (fast burn), ticket on 5% in 6h (slow burn).

### 5.4 SLOs — one SLO, three SLIs, one error budget

- **SLI 1: availability** = `successful / total` over 30 days
- **SLI 2: latency** = 99th percentile response time < 300ms
- **SLI 3: freshness** = `now() - last_data_timestamp` < 60s

SLO targets for 1-10k users:
- **Availability:** 99.5% over 30 days (3.6 hours of downtime budget)
- **Latency:** 99% under 300ms
- **Freshness:** 99% of data writes visible within 60s

### 5.5 Backups — the 3-2-1-1-0 rule, with monthly drill

**3-2-1-1-0:**
- **3** copies of data
- **2** different storage media
- **1** offsite
- **1** immutable (e.g., S3 Object Lock)
- **0** errors on verification (monthly restore drill)

**V1 backup stack:**
- **restic** (or **borgbackup**) for app data, encrypted, to S3
- **pg_dump / pg_basebackup** for Postgres, to S3
- **Litestream** for SQLite, continuous WAL streaming
- **`tool backup verify --app postgres --restore-to scratch`** — the single most important V1.2 command. Restores to a scratch location, verifies row counts, returns the diff.

**The GitLab 2017-01-31 lesson:** 8 months of "successful" backups, all empty, all useless. They recovered 40% of the data. **Backups you haven't restored are hopes.**

### 5.6 Disaster recovery

| Scenario | RTO target | RPO target | Test cadence |
|---|---|---|---|
| Single app crash | 30s (auto-restart) | 0 | Always on |
| Server host dies | 5 min (auto-failover in V2) | 0 (Litestream) | Quarterly |
| Datacenter dies | 1 hour (manual failover) | 5 min (S3 backup lag) | Annually |
| Vendor disappears | 1 day (fresh install + restore) | 0 (last backup) | Annually |
| Accidental `tool drop` | 1 hour (backup restore) | 5 min | Quarterly |

### 5.7 The solo-dev on-call pattern

- 1 person, 24/7, 7 days
- Alerts go to humans via Telegram (not email)
- Every alert has a runbook (in git, not wiki)
- 15-minute response window; if no response, status page auto-flips to "investigating"
- Quarterly "vacation test" — operator takes 1 week off; on-call is automated; manual backup is the freelancer

### 5.8 Observability stack — VictoriaMetrics, not Prometheus

**V1.2 default observability stack (each a single binary, < 200 MB total RAM):**
- **VictoriaMetrics** (single-node) — Prometheus-compatible, 4× lower memory
- **VictoriaLogs** — Loki-compatible, 70% lower memory
- **Grafana** — dashboards
- **vmalert** — alerting
- **OpenTelemetry Collector** — OTLP receiver, shipped as a Docker container

**Out of scope (explicitly rejected):**
- Embedded Prometheus/Grafana/Loki in the sovereign binary
- OTel collector in the same process
- Elasticsearch

**Exposed:** `/metrics` in Prometheus format, structured JSON logs to stdout/file, OTel SDK as opt-in feature flag.

### 5.9 Capacity planning — the formula

| Apps | CPU | RAM | Disk | VPS |
|---|---|---|---|---|
| 1-5 | 2 vCPU | 4 GB | 40 GB | Hetzner CX22 €4.49/mo |
| 5-15 | 4 vCPU | 8 GB | 80 GB | Hetzner CX32 €8.59/mo |
| 15-30 | 8 vCPU | 16 GB | 160 GB | Hetzner CPX41 €15/mo |
| 30-50 | dedicated | 32 GB | 320 GB | Hetzner AX41-NVMe €45/mo |
| 50+ | multi-server | 64+ GB | 1+ TB | Multiple boxes, rqlite HA |

**Right-sizing signal:** if p99 latency > 200ms consistently at < 50% CPU, you need to scale up (more CPU/RAM per node), not out (more nodes).

### 5.10 Configuration management

- **Single binary + single config file** philosophy. No Ansible, no Chef, no Puppet.
- Bootstrap: `curl -sSf sovereign.dev/install.sh | sh` does the whole thing.
- After install: edit `/etc/sovereign/sovereign.toml`, `systemctl restart sovereign`.
- For fleet ops: `tool server add <host>` bootstraps agents via SSH.

### 5.11 Patching

- **Unattended-upgrades enabled by default** (Debian/Ubuntu), 4:30am reboot window
- **Trivy in CI** for image scanning, `--exit-code 1` on HIGH/CRITICAL
- **Packer rebuilds base images nightly** (for the agency's 12 client sites)
- **`tool update`** ships signed releases; users opt-in

### 5.12 The "junior dev" test

If a junior dev gets paged because you're on vacation, can they resolve it? Self-service runbooks, `--explain` flags, diagnostic commands, escalation paths. **If not, the on-call system is broken, not the junior dev.**

### 5.13 The 5-7 tool ceiling

Every ops engineer uses 20+ tools. Aim for the 5-7 that earn their place:
1. **sovereign** (this product) — deploy, ops, secrets, backups
2. **Caddy** (or Nginx) — reverse proxy, TLS
3. **VictoriaMetrics + VictoriaLogs + Grafana** — observability
4. **restic** (or pgBackRest) — backups
5. **age + SOPS** — secrets
6. **trivy** — image scanning
7. **Ansible** (for fleet ops, optional)

That's 7. Coolify alone replaces 4 of them. The product's job is to be the *primary*, not the *only one*.

---

## 6. Platform engineering (Platform Engineer verdict)

### 6.1 The "thinnest viable platform" principle

Per Team Topologies and the 2026 platform-engineering literature: the platform team's job is to **reduce cognitive load on stream-aligned teams**. The "thinnest viable platform" does as little as possible, as well as possible.

For 1-50 engineer orgs, an IDP is an anti-feature. **The product must be the deployment runtime whose default behavior resembles the IDP's golden-path when the user is small, and which can grow into a true IDP through templates and golden paths as the team grows.**

### 6.2 The 6 golden path templates (V1.2, not V1)

Six templates ship in V1.2:
1. **FastAPI** (`tool init fastapi`)
2. **Next.js** (`tool init nextjs`)
3. **Laravel** (`tool init laravel`)
4. **Go** (net/http, Gin, Echo) (`tool init go`)
5. **Rails** (`tool init rails`)
6. **Astro** (`tool init astro`)

Each is a 30-line Rust scanner that detects the framework, writes `app.yaml`, and suggests the next step. No forced directory structure. Escape hatches everywhere (`--dockerfile`, `--image`, `--compose`).

### 6.3 The 16-field service catalog

| Field | Source | Mandatory? |
|---|---|---|
| name | `app.yaml` | yes |
| owner | `app.yaml` | yes |
| env | `app.yaml` | yes |
| git_repo | `app.yaml` | yes |
| image_ref | runtime | yes |
| domains | `app.yaml` | optional |
| health_path | `app.yaml` | optional |
| resources | runtime | optional |
| last_deploy | runtime | auto |
| deploy_count_30d | runtime | auto |
| last_health_fail | runtime | auto |
| backup_id | runtime | optional |
| on_call | `app.yaml` | optional |
| runbook | `app.yaml` | optional |
| slo_target | `app.yaml` | optional |
| created_by/updated_at | runtime | auto |

That's the **entire schema.** Backstage is 200+ plugins; this is "Backstage in 16 fields."

### 6.4 The `tool golden-check` command (V1.2)

```text
$ tool golden-check
✓ app: api          domain: api.example.com    health: 200    TLS: valid    secrets: encrypted-at-rest
✓ backup: daily     last-restore-drill: 12d    status: ok
✗ image: not-pinned-tag    hint: use sha256:abc... or immutable tag
✗ log-rotation: not-configured    hint: `tool log rotate --help`
```

**This is not policy enforcement. This is visibility of the golden state.** Spotify's data: Backstage users "deploy software 2x as often, and their software is deployed for 3x as long." The same mechanism at 1/1000th the engineering cost.

### 6.5 The `RuntimeState` trait — staged backend

```rust
// sovereign-core/src/ports/storage.rs
#[async_trait]
pub trait RuntimeState: Send + Sync {
    async fn get_app(&self, id: AppId) -> Result<Option<App>, AppError>;
    async fn list_apps(&self, owner: Option<&str>) -> Result<Vec<App>, AppError>;
    async fn create_app(&self, app: NewApp) -> Result<App, AppError>;
    async fn update_app(&self, id: AppId, update: AppUpdate, expected_version: u64) -> Result<App, AppError>;
    
    async fn begin_deploy(&self, req: NewDeployment) -> Result<DeploymentId, AppError>;
    async fn record_deploy_event(&self, id: DeploymentId, event: DeployEvent) -> Result<(), AppError>;
    async fn list_deployments(&self, app: AppId, limit: u32) -> Result<Vec<Deployment>, AppError>;
    
    async fn append_audit(&self, event: AuditEvent) -> Result<(), AppError>;
    async fn query_audit(&self, query: AuditQuery) -> Result<Vec<AuditEvent>, AppError>;
    
    // ... secrets, backups, servers
}
```

V1 ships `SqliteState` (the only impl). V2 adds `RqliteState` (same trait). V3 adds `PostgresState` (same trait). **The use cases never change.**

### 6.6 Drift detection in V1.5, no auto-reconcile until V2

A pure GitOps model (ArgoCD-style, auto-reconcile) is too aggressive for V1. The V1.5 model is **declarative mode + read-only drift detection**:

```bash
$ tool apply -f app.yaml --dry-run
# Plan: 2 changes
+ resources.cpu: "0.5" -> "1"
+ replicas: 1 -> 2
# 0 violations of policy
# Run with --confirm to apply

$ tool apply -f app.yaml --confirm
# Applied. View at: https://app.example.com
```

Auto-reconcile (the Kubernetes/ArgoCD way) is V2+. For V1, the human approves their own diffs. This is a feature, not a bug — it prevents the "platform quietly overwrote my fix at 3am" horror story.

### 6.7 The agency use case (Karim, 12 client sites)

For agencies, the product offers:
- **White-label status pages** (`tool status --public --branding client-x`)
- **Per-client audit reports** (`tool audit --owner client-x --since 30d --format pdf`)
- **Bulk operations** (`tool deploy --all`, `tool fleet --matrix`)
- **Per-client access control** (freelancer scoped to one client)
- **Backups aggregated across clients** (`tool backup verify --all`)

### 6.8 Build vs. buy — the 2026 decision matrix

For a sub-100 engineer org:
- **Total cost of ownership** of self-hosted IDP: €1.2-2.5M over 3 years
- **Total cost of ownership** of managed IDP (Port, Humanitec): €150k-450k
- **Total cost of ownership** of a deployment runtime + 6 templates: €5-50k

**For the spec's audience, the product is the right answer.**

### 6.9 Platform team staffing

For a 50-engineer startup using the product:
- **0 dedicated platform engineers** (if the team is < 30)
- **1 part-time platform engineer** (if the team is 30-100, doing golden-path maintenance)
- **2-3 platform engineers** (if the team is 100+, doing templates + policy)

The product's job is to make the 1-engineer case sustainable.

---

## 7. Product management (PM verdict)

### 7.1 The 5-minute moment

The single most important product metric is **time-to-first-deploy**. From `curl -sSf sovereign.dev/install.sh | sh` to a live URL on a temporary domain. Target: **2 minutes 30 seconds.**

```text
$ curl -sSf sovereign.dev/install.sh | sh       # 1 — install (10s)
$ sovereign init                                # 2 — detect framework, write app.yaml (5s)
$ sovereign login                               # 3 — device-code flow, browser opens (15s)
$ sovereign deploy                              # 4 — builds, provisions TLS, gives you URL (90s)
$ open https://<id>.srvr.so                     # 5 — you are live
```

5 commands. No Dockerfile. No docker-compose.yml. No nginx. No certbot. The output of step 4 includes the URL, the deploy log, the rollback command, the next-step hint, and the URL to add a custom domain.

### 7.2 The post-onboarding "what next?" prompt

After a successful deploy, the CLI prints:

```text
✓ https://pr-48291.srvr.so is live
✓ Health check passed (200 OK, 12ms p50)
✓ TLS issued (Let's Encrypt, auto-renews in 60d)

Next, you probably want to:
  → sovereign domain add api.mirasaas.com        # map a real domain
  → sovereign secret set DATABASE_URL --from-stdin
  → sovereign preview --pr 42                    # PR preview environments
  → sovereign backup verify --app postgres       # first restore drill
  → sovereign tui                                # see your fleet at a glance
```

Five lines. One screen. No marketing.

### 7.3 The 8 non-negotiables for the CLI

1. **`--help` is a teaching surface.** Use `clap`'s `after_long_help` for 3-5 tips + 3-5 real example commands. The `--help` is the doc.
2. **`--json` everywhere, auto-detected via `std::io::IsTerminal`.** Humans get colored tables; agents get JSON envelopes. AI agents are first-class users.
3. **Semantic exit codes.** 0 success, 1 generic, 2 usage, 3 partial, 4 upstream. CI and agents can decide whether to retry.
4. **Spinner / X-of-Y / progress bar.** Pick the right one (Evil Martians' guide).
5. **Color the right way.** NO_COLOR, --color=never, semantic palette.
6. **Errors that teach, not blame.** Print the error chain, color the cause, suggest a fix.
7. **`--dry-run` on every destructive action.** Default is "ask before doing." Non-interactive is `--yes`.
8. **Shell completions for bash, zsh, fish, nushell, powershell.** Free with `clap_complete`. Install via `sovereign completions install`.

### 7.4 The 4 non-negotiables for the TUI

- alt-screen mode
- panic-safe terminal restore
- SIGWINCH handling
- SIGTSTP handling

**Get those wrong and your TUI leaves the user's terminal broken when something crashes.** Test it: open the TUI, kill -9 the process, check that the shell still echoes.

### 7.5 The TUI views (V1)

- **Pulse** (home screen). 5 lines: app count, healthy count, last deploy, last error, next backup.
- **Apps** (master list). Status, last deploy, last deploy version, response time p50.
- **App detail** (drill-down). Tabs: Logs, Deploys, Rollbacks, Metrics, Domains, Secrets, Config.
- **Servers** (multi-server in V1.5+). Fleet. CPU, RAM, disk.
- **Backups**. Every database, last backup, last verify, restore button.
- **Audit**. Filterable timeline.

### 7.6 Keybindings (universal conventions)

- `q` quit / `Esc` back
- `j`/`k` down/up, `h`/`l` left/right
- `/` search, `n`/`N` next/prev, `Esc` dismiss
- `?` help for current view
- `:` command mode
- `Enter` select, `Tab` switch panel, `Space` toggle
- `g`/`G` top/bottom
- `Ctrl+P` command palette

### 7.7 Documentation — auto-generated, never lies

**The `--help` output IS the documentation.** Auto-generate man pages (`clap_mangen`). Auto-generate the web docs from `clap` (a 200-line build.rs). Never let docs and CLI drift.

Use `mdbook` for the rest. Host on the product itself (dogfooding). Categories:
- **Quickstart** (the 5 commands)
- **Concepts** (deploy, rollback, secret, backup, server, policy)
- **Tutorials** (per golden path: FastAPI, Next.js, Laravel, Go, Rails, Astro)
- **How-tos** (per task: set up a domain, configure backup, write a policy rule)
- **Reference** (CLI, API, config, rego)
- **Operations** (runbooks, SLOs, alerts, DR)

### 7.8 Pricing — per-server, per-tier

- **Community (free, Apache 2.0):** unlimited servers, all features, no support.
- **Pro (€19/server/month):** managed updates, email support, EU-region updates, official deb/rpm repos, 1-day SLA on security advisories.
- **Enterprise (custom, starts €5k/year):** EUCS Substantial package, BSI C5 mapping, on-prem, 24/7 SLA, dedicated CSM, training.

**Per-server pricing aligns with EU procurement** (per-node, not per-seat). Maps to the IPCEI-CIS, EUCS, BSI C5 patterns.

**Free tier is the funnel.** Plausible has 50k+ paying sites on a 4-person team; they don't gate features. Coolify has 56k stars with no feature paywall. The product should match.

### 7.9 The "framework-detection scanner" is the secret sauce

The single highest-leverage code in V1 is the **framework scanner** in `sovereign init`. A 5-minute first deploy is the price of admission; a 5-minute first deploy *that worked without me reading docs* is the moat. Coolify has 280+ templates; the scanner doesn't even need to be that good for V1, but it must work for the 6 golden paths.

### 7.10 Telemetry — what to instrument

- **Time-to-first-deploy** (the headline metric)
- **Deploys per day per user** (engagement)
- **Rollback rate** (quality)
- **Mean time to detect** (operational)
- **Mean time to recover** (operational)
- **Backup verify rate** (the "did you test your backup?" metric)
- **TUI session length** (TUI stickiness)
- **CLI invocations per day** (engagement)
- **Docs page → first deploy conversion** (onboarding funnel)

**No PII. No content of secrets. No deploy content. Only metadata.**

### 7.11 The 3 onboarding personas and what they need

| Persona | First session goal | Killer feature | Drop-off risk |
|---|---|---|---|
| **Mira (solo founder)** | Deploy 1 app, see it on the internet | `sovereign init` + `sovereign deploy` (5 min) | If deploy takes > 5 min, she's gone |
| **Karim (agency)** | Deploy 1 client's app | The scanner + multi-app in 1 TUI | If scanner doesn't detect her stack, she's back to CapRover |
| **Lin (startup infra)** | Replace Heroku | `sovereign policy check` + `sovereign audit export` | If the policy engine is opaque, she can't sell it to her CTO |
| **Sara (EU public sector)** | Pass BSI C5 procurement | `sovereign compliance map` + `sovereign sovereignty check` | If sovereignty is theatre, she's gone |
| **Alex (vibe coder)** | Deploy from Cursor via Claude Code | `sovereign --json` everywhere | If the agent can't drive it, he's back to Vercel |

---

## 8. CTO strategy (CTO verdict)

### 8.1 Positioning — the "Plausible of deployment"

The product is not in the PaaS cell. Not in the IDP cell. Not in the "Kubernetes without Kubernetes" cell. **It is the Plausible of self-hosted application deployment.**

- Bootstrapped or near-bootstrapped trajectory
- EU-based, EU-incorporated
- Apache 2.0 unmodified, no carve-outs
- Operates on a €4.49 Hetzner box
- Survives the company disappearing: binary works, data is exportable, license is irrevocable
- 4-5 person team at €1M ARR
- 10-15 person team at €5-25M ARR
- Sells to "Plausible buyer" — solo devs, agencies, EU mid-market, EU public sector

**How positioning evolves:**

| Stage | Internal | External | Buyer |
|---|---|---|---|
| 0→1 (now) | A deployment engine for solo devs | "Self-hosted Vercel alternative" | Solo devs, vibe coders, agencies |
| 1→10 (year 1) | A PaaS for small teams | "Coolify but lighter, sovereign, AI-friendly" | Startups, agencies, small SaaS teams |
| 10→100 (year 2) | A sovereign runtime for EU mid-market | "The Hetzner of deployment" | EU mid-market IT, German Mittelstand, French regulated industry |
| 100→1k (year 3) | A platform for EU public procurement | "EUCS-Substantial deployment runtime" | EU public sector, defense-adjacent, finance |
| 1k→10k (year 5+) | A category-defining sovereign application platform | "Plausible of deployment" | All of the above + agencies managing 50+ client sites |

### 8.2 License — Apache 2.0, unmodified, no carve-outs

- The Dokploy #3613 thread, the open-source "false marketing" complaint, and the CasaOS abandonment are the patterns to avoid.
- Plausible: AGPLv3, but they're so trusted it doesn't matter. Supabase: Apache 2.0. Sentry: FSL (functionally Apache 2.0 after 2 years).
- **Apache 2.0 with no carve-outs is the 2026 default for sovereign PaaS.** It matches every comparable product and signals "this won't disappear."

### 8.3 Funding — bootstrap to €1M ARR, then sovereign-tech EU seed

| Stage | Funding source | Amount | Rationale |
|---|---|---|---|
| Pre-seed | Friends, family, angels | €50-100k | 6 months runway, ship V1 |
| Bootstrap | Revenue | — | 12-18 months to €100k ARR |
| Seed | EU sovereign-tech VC (OSS Capital, Cherry, Speedinvest, Notion, LocalGlobe EU) | €4-8M | 18-24 months runway, scale team to 8-12, hit €1M ARR |
| Series A | EU sovereign-tech + sovereign-fund-adjacent (EIB, KfW, Bpifrance) | €15-30M | 36 months runway, hit €5-10M ARR, EU public-sector expansion |

**Sovereignty is the brand; US VC money contradicts it.** A US VC will push for the hyperscaler "sovereign" SKU pattern (data residency but corporate US). The buyer won't buy that.

### 8.4 Build vs. buy — the locked table

| Component | Decision | Rationale |
|---|---|---|
| HTTP / reverse proxy | **Delegate to Caddy (default) + Nginx (low-mem in V1.1)** | Caddy 2.11 has built-in ACME; Nginx is 6 MB |
| Container runtime | **Delegate to Docker (V1) + Podman (V1.5) behind `Runtime` trait** | Don't rewrite the abstraction |
| Build system | **Delegate to external CI by default; embed BuildKit as opt-in** | `sovereign deploy api --image=...` bypasses build |
| State / database | **Embed SQLite in V1; rqlite in V2** | Same `RuntimeState` trait; zero use-case-code change |
| Backup / replication | **Delegate to Litestream (V1), rqlite S3 (V2), pg_dump (V1)** | We wire it; we don't write it |
| Secret manager | **Embed age for at-rest; delegate Vault only for enterprise V2** | age + SOPS is the 2026 default |
| TLS / ACME | **Delegate to Caddy's ACME for public edge; mkcert for control plane** | Don't reinvent Certbot |
| Identity / SSO | **Delegate entirely (Authentik, Keycloak, Zitadel, Auth0, GitHub OIDC)** | Be a consumer, not a provider |
| Observability | **Embed OTel SDK as opt-in; expose Prometheus `/metrics`; don't embed** | ~5 MB OTel SDK; users ship to wherever |
| Policy engine | **Delegate to OPA in V2** | Don't build a DSL |
| DNS | **Delegate to provider APIs (Cloudflare, Hetzner DNS, OVH, Route53)** | DNS-01 ACME only |
| Object storage | **Delegate to S3-compatible (R2, B2, MinIO, Garage)** | MinIO exists |
| Logging aggregation | **Delegate (export JSON logs to Loki / Datadog / Vector)** | Don't embed Elasticsearch |
| CLI / TUI | **Embed.** This is the brand. | ratatui + clap |
| Web UI | **Do not build V1. Add as opt-in V2 (HTMX, ~30 KB).** | CLI is the product |
| Documentation | **Embed (mdBook). Host on the product itself.** | Dogfooding |
| Update / package management | **Embed: signed releases, `sovereign update`, deb/rpm/AUR** | The "WordPress-plugin-maintenance feel" of Coolify is partly because they depend on apt |

### 8.5 The bet we cannot afford to be wrong about

**Single → multi-server architecture.** Period.

- **V1:** single binary, single host, SQLite, systemd. No agent. State on the same host as the apps.
- **V1.5:** add `agent` mode. Server pushes deploy instructions to agents over mTLS. State on the server's SQLite. Agents are stateless.
- **V2:** rqlite as the optional state backend. 3 rqlite nodes form a Raft consensus group. Now the control plane survives 1 node failure.
- **V3+:** per-region agent pools, CRDTs for audit log.

**Why this is the bet:** every other decision is recoverable. Wrong proxy? Swap. Wrong TUI lib? Migrate. Wrong secret scheme? Migrate. Wrong state architecture? You rewrite every command, every endpoint, every agent message, every SQLite query, every backup path. This is the one.

### 8.6 Competitive moat — what stops a competitor

| Moat | Strength | Timeline to build |
|---|---|---|
| Single-binary Rust | Small (PIER, sh0, yoink exist) | Already exists |
| Sovereign positioning | Medium (Coolify could add it) | 12-18 months |
| "Vercel UX, VPS pricing" framing | Medium (Plausible has it in analytics) | 12-18 months |
| Ecosystem / templates | Large but expensive | 24-36 months |
| Trust / credibility | Largest | 5 years |
| EU incorporation + EU public-sector reference | Large (very hard to fake) | 24-36 months |
| Apache 2.0 + commercial-OSS hybrid from day 1 | Medium (Plausible, Sentry have it) | Already exists |

**The hardest-to-replicate moat is EU public-sector reference customer + EUCS Substantial + BSI C5 mapping.** That's 3 years of work and 3 years of trust. By then, the brand is the moat.

### 8.7 The "CasaOS mistake" to avoid

CasaOS: 33k stars, Apache 2.0, one corporate sponsor (IceWhale), dev moves to closed-source ZimaOS, repo rots. **Lesson:** Apache 2.0 with one corporate sponsor is a ticking time bomb.

**Mitigation:**
- EU incorporation (Berlin or Tallinn) — not US, not China
- Foundation transfer plan by year 3 (Linux Foundation Europe, Apache, Eclipse)
- Public 18-month sustainability signal on the website
- Public funding / revenue transparency
- ≥ 3 core maintainers, not 1
- "Maintainer ladder" — anyone can become a core maintainer

### 8.8 The "Dokploy mistake" to avoid

Dokploy: 34k stars, license changed in Jan 2026 to "Apache 2.0 core + source-available for templates/multi-server/previews." Community backlash on #3613, #3477.

**Mitigation:**
- Never introduce a `proprietary/` directory
- Never introduce a source-available license for any feature
- Public commitment: "All features, all of the time, Apache 2.0. Forever."
- Trademark policy that protects the name without restricting use

### 8.9 The "Vercel mistake" to avoid

Vercel: free → Pro $20/seat → usage-based → Riley Walz's $46,485 Jmail bill → backlash.

**Mitigation:**
- Flat per-server pricing, forever. No per-bandwidth, no per-deploy, no per-seat.
- The Pro tier is for *managed updates + support*, not for *features*.
- Public commitment: "€19/server/month. No surprises. No overage. No bandwidth."

### 8.10 The 100-year-old question

**"If you disappear, what happens to the users?"**

- Apache 2.0: the binary keeps working, the data is portable
- SQLite state: the database is a single file, restorable on any host
- Litestream backups: encrypted, to any S3, restorable anywhere
- Exportable platform: `sovereign export` produces a complete backup
- Reproducible install: `curl ... | sh` works offline from a USB stick
- Public CI proof: a vendor-disappear test runs on every release
- Foundation transfer plan: by year 3, the project is owned by a foundation

**This is the founding question of an open-source product. The Plausible blog post "We chose open source and we have no regrets" is the answer.**

### 8.11 Risk management — the 12 named risks

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

## 9. Cross-cutting: governance, USP, sovereignty as engineering (Cross-cutting verdict)

### 9.1 Governance — rules + ML, layered, not competing

**The thesis:** most "policy" features in deployment platforms fail for one of two reasons:
- **Pure rules:** brittle, false-positive-heavy, can't see slow drift. Ops teams quietly disable them.
- **Pure ML:** opaque, un-auditable, illegal under GDPR Art. 22 for decisions affecting a person. Also: how do you *explain* a rollback to a customer?

**The working answer (confirmed by Google SRE 2026, Datadog Watchdog, OPA/Gatekeeper, Kyverno, Cedar):** layered. Rules are the law. ML is the early-warning radar.

```
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

**The 6-month staged rollout:**
- **Month 1-2:** Layer 1 + Layer 3 (rules only, no ML). Ship `sovereign policy check` and 3 rule packs (`baseline.rego`, `cost.rego`, `compliance.rego`).
- **Month 3-4:** Add Layer 2 in *shadow mode* (the ML scorer runs, but doesn't influence decisions; logs to audit for human review).
- **Month 5:** Add `sovereign policy advise` (advisory only — the CLI says "ML scored this deploy 67, recommend canary at 5%" but doesn't enforce).
- **Month 6:** Add enforcement for low-risk decisions (auto-canary, auto-log). High-risk decisions still require human approval.

**Every ML decision is explainable.** "Why was I rolled back?" returns the score, the contributing features, and the rule that fired. Audit log includes the ML inference inputs (snapshotted) so the decision can be replayed in a notebook.

### 9.2 The risk score — a concrete formula

```
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

Bands:
- **0-19:** auto-approve, full rollout
- **20-49:** auto-approve, log to audit
- **50-79:** canary at 5%, no auto-rollback, alert on anomaly
- **80-100:** halt, page on-call, require manual `sovereign deploy --confirm-risk`

### 9.3 The Rego rule packs that ship in V2

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

### 9.4 USP — pick one, defend for 2 years

**The single primary USP:** **"Run your own cloud. Single binary. No DevOps team."**

Why this works:
- **Specific:** "run your own cloud" is a verb, not a noun
- **Ownable:** no competitor owns "single binary + no DevOps team" today
- **Relevant:** matches the "Vercel UX, VPS pricing" gap and the EU sovereignty buyer
- **Testable:** the user can verify it in 5 minutes (install + first deploy)
- **Defensible:** combines 3 different moats (binary size, simplicity, sovereignty)

**USPs to refuse (tempting but wrong):**
- "Self-hosted Vercel" — concedes Vercel is the canonical answer
- "The deployment engine" — too generic
- "Sovereign Application Runtime" — too corporate, not a buyer-facing phrase
- "Vercel UX, VPS pricing" — used too much, loses signal
- "Coolify alternative" — concedes Coolify is the default
- "Kubernetes without Kubernetes" — concedes Kubernetes is the answer
- "The Heroku of self-hosting" — too narrow
- "AI-native deploy" — hype, not substance

**Iteration rule:** if a customer uses a phrase to describe the product, that's the new USP candidate. If 3 customers use the same phrase in 30 days, that's the new USP. Test with the "explain to your mom" test, the "explain to your CTO" test, the "explain in one tweet" test.

### 9.5 Sovereignty as engineering — not a checkbox

**The 10-point sovereignty test (run in CI on every release):**

1. Is the source code open and self-hostable? (Apache 2.0 / MIT, no carve-outs)
2. Can I run it without internet? (offline mode tested in CI)
3. Can I run it without any external service? (no SaaS dependency)
4. Can I export everything? (`sovereign export` produces a complete backup)
5. Can I import on a different host? (restorable on a fresh box)
6. Is the company incorporated in a jurisdiction I trust? (EU)
7. Is the company funded such that it survives a downturn? (revenue or long runway)
8. Is there a credible bus factor? (≥ 3 core maintainers, or a foundation)
9. Are security advisories public? (GHSA, CVE)
10. Is the binary reproducible? (deterministic builds, signed releases)

**Wire this into the release pipeline.** If the 10-point test fails, the release doesn't ship. Make it a CI gate, not a wiki page.

### 9.6 Sovereignty corporate structure (decided first, not last)

For EU public sector procurement (BSI C5:2026, EUCS Substantial, SecNumCloud), the corporate structure is a hard requirement:

- **Option A: German GmbH** (Berlin) — qualifies for BSI C5 + EU public sector + IPCEI-CIS
- **Option B: Dutch BV** (Amsterdam) — qualifies for EU public sector, faster to set up
- **Option C: Estonian OÜ** (Tallinn) — fully digital, e-residency friendly, 0% corporate tax on reinvested profits
- **Option D: French SAS** (Paris) — qualifies for SecNumCloud + "Cloud de Confiance"

**Recommended:** Berlin GmbH (for BSI C5 + EU public sector) + Estonian OÜ (for the e-residency / digital-first team). Two entities, one mission.

### 9.7 "Sovereign" brand — the messaging

**What "sovereign" means to a buyer:**
- **Data sovereignty:** my data is in a jurisdiction I trust
- **Operational sovereignty:** I can run it without calling anyone for permission
- **Vendor sovereignty:** the vendor can disappear and the product keeps working
- **Legal sovereignty:** the company is incorporated where I trust
- **Technical sovereignty:** the technology is open and inspectable

**Hyperscalers can claim 1-2 of these. The product claims all 5 by construction (self-hosted, open source, exportable, EU-incorporated, open technology).**

### 9.8 The maintainability test for sovereignty features

Every sovereignty feature must pass this test before shipping:

1. **Is the feature testable in CI?** (e.g., the offline-mode test runs the binary in a network-isolated container)
2. **Does the feature add a maintenance burden?** (e.g., cosign signing requires key management; key rotation is V1.5)
3. **Can the feature be removed without breaking the contract?** (e.g., SBOM generation is opt-in; not on by default)
4. **Does the feature have a public doc?** (e.g., `docs/sovereignty/10-point-test.md`)
5. **Is the feature mentioned in the marketing?** (only if it's not theatre)

**Features that pass:** offline mode, export, import, encryption at rest, public SBOM, signed releases, audit log, EUCS-Substantial controls mapping doc.

**Features that fail (don't build):** "GDPR compliance" badge without substance, "immutable audit log" without a real immutability model, "zero-trust" without ZTA architecture, "air-gapped" without testing air-gapped operation, "SOC2 ready" without SOC2.

---

## 10. The unified feature/architecture map (everything in one picture)

### 10.1 Module-to-persona matrix

| Module | PM | Principal | Rust | DevOps | Platform | CTO | Cross-cutting |
|---|---|---|---|---|---|---|---|
| `sovereign-core` (domain) | — | ★★ | ★★ | — | ★ | — | ★ |
| `sovereign` (binary) | ★★ | ★★ | ★★ | ★★ | ★★ | ★★ | ★★ |
| `sovereign-runtime-docker` | ★ | ★★ | ★ | ★★ | ★ | — | — |
| `sovereign-runtime-podman` | — | ★★ | ★ | ★ | — | — | — |
| `sovereign-proxy-caddy` | ★★ | ★★ | ★ | ★★ | ★ | ★ | — |
| `sovereign-proxy-nginx` | ★ | ★★ | ★ | ★★ | — | ★ | — |
| `sovereign-secrets-age` | ★★ | ★★ | ★★ | ★★ | ★ | ★ | ★★ |
| `sovereign-storage-sqlite` | ★ | ★★ | ★★ | ★★ | ★★ | ★ | — |
| `sovereign-storage-rqlite` | — | ★★ | ★★ | ★★ | ★★ | ★★ | — |
| `sovereign-backup` | ★★ | ★★ | ★ | ★★★ | ★ | ★ | — |
| `sovereign-policy-rego` | — | ★★ | ★ | ★ | ★★ | ★ | ★★ |
| `sovereign-ml-scorer` | — | ★★ | ★ | ★ | ★ | ★ | ★★ |
| `sovereign-notify` | ★ | ★ | ★ | ★★ | ★ | — | — |
| `sovereign-observability` | ★ | ★★ | ★★ | ★★★ | ★ | — | — |
| `sovereign-cli` (clap) | ★★★ | ★★ | ★★ | ★★ | ★★ | ★★ | ★★ |
| `sovereign-tui` (ratatui) | ★★★ | ★ | ★★ | ★★ | ★★ | ★★ | ★ |
| `sovereign-web` (HTMX, V2) | ★ | ★ | ★ | — | ★ | — | — |
| `sovereign-migrations` | — | ★★ | ★★ | ★★ | ★ | — | — |
| `sovereign-acme` | ★ | ★★ | ★ | ★ | — | — | — |
| `sovereign-git` | ★★ | ★ | ★ | ★ | ★ | — | — |
| `sovereign-buildkit` | ★ | ★ | ★ | ★ | ★ | — | — |
| `sovereign-proto` | — | ★★ | ★★ | — | ★ | — | — |

### 10.2 Day-to-day feature frequency (priority matrix)

| Frequency | Pain | Auto | Business | Priority | Feature |
|---|---|---|---|---|---|
| 10×/day | High | High | High | **P0** | `sovereign deploy`, `sovereign status`, `sovereign logs` |
| 5×/day | Med | High | High | **P0** | `sovereign secret set/get` (agent-safe) |
| 2×/day | Med | High | High | **P1** | `sovereign preview` (per PR, with TTL) |
| 1×/day | High | High | High | **P0** | `sovereign backup verify`, `sovereign health` |
| 1×/week | High | High | High | **P0** | `sovereign rollback`, `sovereign db shell` |
| 1×/month | Med | Med | Med | **P1** | `sovereign access add/revoke`, `sovereign audit` |
| 1×/quarter | High | Med | High | **P1** | `sovereign db upgrade`, `sovereign migrate` |
| 1×/year | Low | Low | Low | **P3** | `sovereign sovereignty check`, `sovereign compliance map` |

### 10.3 The "5-minute moment" — every feature that affects time-to-first-deploy

| Feature | Impact on time-to-first-deploy | V1 priority |
|---|---|---|
| `sovereign init` (framework scanner) | -90s | **P0** |
| Auto-TLS via Caddy | -60s | **P0** |
| Pre-built SQLite (no init script) | -30s | **P0** |
| `sovereign login` device-code flow | -30s | **P0** |
| Auto-detect GitHub repo | -45s | **P1** |
| `sovereign deploy` with progress | -20s | **P0** |
| `sovereign --json` for agents | -120s (for agent-driven deploys) | **P0** |
| `--help` as teaching surface | -60s (no docs needed) | **P0** |
| `--dry-run` everywhere | -30s (no rollback trial-and-error) | **P0** |
| Shell completions (install in 1 cmd) | -10s | **P1** |
| Post-deploy "what next?" prompt | -60s (next feature discoverable) | **P0** |

**The 5-minute moment is the product.** Every other feature is overhead until this is < 2 min 30s.

---

## 11. The 18-month roadmap (synthesized across all personas)

### 11.1 MVP (Weeks 1-6) — "the engine"

8 features, 1 developer can use it.

1. Single Rust static binary (musl, LTO, abort, strip) — 10-25 MB
2. CLI: `sovereign deploy`, `rollback`, `logs`, `status`, `secret set/get`
3. Git push deploy with atomic release (image-tagged, immutable history)
4. One-command rollback
5. Caddy auto-TLS (HTTP-01 ACME, auto-renew, hot reload)
6. Encrypted secret store (age + envelope encryption, inject at process start)
7. Postgres backup to S3-compatible (pg_dump + S3, with size sanity check)
8. Health check + auto-rollback (configurable threshold, on health fail)

### 11.2 V1 (Weeks 7-18) — "the product"

A usable product for solo developers and small teams. The killer features.

- **TUI dashboard** (ratatui) — daily-driver interface
- **Resource limits** (cgroups) — production safety
- **Structured JSON logs** (auto-emitted)
- **Log search / filter / retention**
- **Health probes** (liveness, readiness, startup)
- **Webhook on deploy events**
- **Uptime monitoring** (external probe)
- **Alert channels:** email, Telegram, Slack, Discord, webhook
- **Multi-environment** (dev / staging / prod in one config)
- **Postgres provision** (one-click)
- **Postgres connection pooling** (PgBouncer)
- **MySQL / MariaDB / Redis / SQLite provision** (one-click)
- **Volume persistence** (named volumes)
- **Scheduled backup** (cron: daily/weekly/monthly)
- **Backup retention policy** (auto-purge)
- **OpenTelemetry / Prometheus `/metrics` endpoint**
- **TLS for control plane API**
- **Config file (`app.yaml`) for declarative deploy**
- **`sovereign init fastapi|nextjs|laravel|go|rails|astro`** (scaffolding)
- **`sovereign morning-report`** (the 5-command daily)
- **`sovereign golden-check`** (visibility of the golden state)
- **`sovereign backup verify`** (restore drill)
- **systemd unit, deb/rpm package**
- **Documentation site (mdBook)**
- **Show HN post** + awesome-selfhosted inclusion

### 11.3 V1.5 (Months 5-9) — "the team"

- **Multi-server (agent pattern)** — server + N agents over mTLS
- **Team access (RBAC: owner, admin, dev, read)**
- **Audit log** (every action → event, immutable)
- **Multi-environment promotion** (dev → staging → prod)
- **Preview environments per PR** (with TTL)
- **Nginx low-memory mode** (10 MB)
- **Podman runtime support**
- **Declarative GitOps mode** (read-only drift detection, no auto-reconcile)
- **Service catalog** (16 fields, auto-derived)
- **catalog export to Backstage YAML** (escape hatch)
- **Secret rotation** (age envelope + redeploy)

### 11.4 V2 (Months 10-18) — "the platform"

- **rqlite-based HA control plane** (3-server, tolerates 1 failure)
- **Export / import platform** (full backup / restore, restorable elsewhere)
- **Disaster recovery (`sovereign recover`)**
- **Air-gapped install** (offline binary + offline docs)
- **EUCS Substantial-aligned controls checklist** (public doc)
- **BSI C5:2026 mapping** (public doc)
- **OPA / Rego policy engine** (`sovereign policy check`)
- **ML-based anomaly scorer** (Prophet in V2, TimesFM in V2.5)
- **Risk scoring** (0-100, with 4 bands)
- **Auto-canary at 5% for medium-risk deploys**
- **Migrate-from-Coolify / -Dokploy** (best-effort import)
- **Web UI (HTMX, ~30 KB)** — opt-in, never primary
- **Managed / Pro tier** (commercial offering: hosted control plane)
- **EU pricing** (per-node, €19-100/node/mo)
- **EU incorporation** (Berlin GmbH)
- **SBOM, cosign signing, reproducible builds** (CI gate)
- **Vendor-disappear test** (CI gate)
- **The 10-point sovereignty test** (CI gate)

### 11.5 V3+ (Year 2+) — "the product for the other 90%"

- True canary deployments (5/25/100% traffic split)
- Multi-region failover
- Service catalog (apps, DBs, workers, queues)
- Marketplace (1-click: Plausible, n8n, Ghost, etc.)
- Internal Developer Platform (golden paths, `sovereign create saas`)
- SSO / OIDC (consumer, not provider)
- MFA (TOTP)
- 2-of-N approvers
- EUCS High alignment
- SecNumCloud (if customer demand)
- Hosted / managed offering (Pro tier, EU region)

---

## 12. The non-negotiables — what to ship, what to reject

### 12.1 The non-negotiables (must ship)

- Single static binary (10-25 MB musl + LTO + abort + strip)
- CLI as the primary surface; TUI as daily-driver; web UI as V2 opt-in
- axum + tokio + sqlx + ratatui + clap 4.6 + tracing + thiserror + anyhow
- SQLite (WAL) for control plane; rqlite in V2
- Caddy as default proxy; Nginx 10 MB as low-mem alternative
- Docker V1; Podman V1.5; never K8s
- Encrypted secrets at rest (age + envelope); zero-disk injection
- Auto-TLS via ACME (Caddy)
- Atomic image-tagged deploys + one-command rollback
- Health check + auto-rollback
- Postgres backup with verification
- Apache 2.0 unmodified, no carve-outs
- Public CI: tests, lints, SBOM, cosign, reproducible builds
- EU incorporation from year 1
- 10-point sovereignty test in release pipeline
- Documentation auto-generated from CLI; never drifts
- `--json` everywhere for AI agents
- `--dry-run` on every destructive action
- Shell completions for bash, zsh, fish, nushell, powershell
- 6 framework scanners (FastAPI, Next.js, Laravel, Go, Rails, Astro)
- 10 alerts, each with a runbook
- SLOs: 99.5% / 300ms p99 / 60s freshness
- 3-2-1-1-0 backups with monthly restore drill
- Hexagonal architecture with `Arc<dyn Port>` DI
- Layered governance (telemetry → ML scorer → Rego rules → human override)
- Append-only audit log with SQLite triggers

### 12.2 The defer (V2+ or never)

- True canary (5/25/100% traffic split) — V2.5+
- Multi-region failover — V3+
- Kubernetes support — never
- Service mesh — never
- Multi-cloud abstraction — never
- Terraform / Pulumi replacement — never
- Custom policy DSL — never (use OPA/Rego)
- Complex RBAC (5+ tiers) — V3+
- Plugin marketplace with 3rd-party code — never
- AI Copilot for deploys — V2.5+ (after MCP demand validated)
- SOC2 reporting — V3+ (only if customer demand)
- White-label / agency edition — V3+
- OIDC provider (be the IdP) — never
- Internal developer portal (Backstage-style) — V3+
- Workflow engine (Airflow-like) — never
- Custom container runtime — never
- gRPC API — never (REST + JSON)
- GraphQL API — never
- WebSocket API for the control plane — never (SSE for log streaming)
- Mobile app — never
- Desktop app (Electron, Tauri) — never

### 12.3 The reject (do not build, even if requested)

- "GDPR compliance" without substance
- "Immutable audit log" on an editable DB
- "Air-gapped" without testing
- "SOC2 ready" without SOC2
- "Zero-trust" without ZTA architecture
- "AI-driven" anything that doesn't have a deterministic backup
- "Real-time" anything that doesn't have a backpressure plan
- "Production-ready" without a real user
- "Enterprise-grade" without a real enterprise customer
- "Mission-critical" without a real on-call rotation
- "Future-proof" abstractions that aren't solving today's problem
- "Innovative" deployment strategies (shadow, dark launch, etc.) — over-engineered
- Custom DNS server — let the user use managed DNS
- Auto-scaling — VPS doesn't auto-scale
- Anything that requires a SaaS to be useful
- Anything that requires an account to be useful (except the admin user)
- Anything that requires a credit card to be useful (except Pro tier)
- Anything that requires telemetry to be useful (always opt-in, opt-out is default)

---

## 13. The daily-driver surface — concrete commands

### 13.1 The 30 commands that matter

```bash
# Deploy
sovereign deploy [APP]                          # git push, atomic, with health check
sovereign deploy [APP] --image=<ref>           # pre-built image, skip build
sovereign deploy [APP] --strategy bluegreen    # blue-green by default
sovereign deploy [APP] --dry-run               # show what would happen
sovereign deploy [APP] --confirm-risk          # high-risk deploy, bypass policy

# Rollback
sovereign rollback [APP]                       # one command, image-tagged history
sovereign rollback [APP] --to=v1.4.2           # specific version
sovereign rollback [APP] --dry-run

# Logs
sovereign logs [APP] --tail=100 --follow        # live stream
sovereign logs [APP] --since 30m --level error # filtered
sovereign logs --since 1h --level error        # across all apps

# Status
sovereign status                               # fleet overview
sovereign status [APP]                         # single app
sovereign status --watch                       # TUI-like stream
sovereign status --public                      # status page

# Secrets (zero-disk injection)
sovereign secret set DATABASE_URL              # from stdin
sovereign secret set DATABASE_URL --from-stdin # explicit
sovereign secret list                          # keys only, never values
sovereign secret rotate DATABASE_URL --all-envs

# Backups
sovereign backup list                          # all backups, status
sovereign backup create --app postgres         # manual
sovereign backup verify --app postgres --restore-to scratch  # THE most important command
sovereign backup restore <id> --confirm        # restore from backup

# Servers
sovereign server list                          # fleet, status
sovereign server add <host>                    # bootstrap agent
sovereign server remove <id>                   # drain, remove
sovereign server doctor <id>                   # connectivity, resources, logs

# Access (V1.5)
sovereign access list                          # users
sovereign access add --app [APP] --role dev --email user@x.io
sovereign access revoke <user-id>

# Audit
sovereign audit --since 7d                     # all events
sovereign audit --app [APP] --actor user:alice
sovereign audit export --format csv --since 30d

# Policy (V2)
sovereign policy check --app [APP] --env prod  # dry-run rules
sovereign policy list                          # installed rule packs
sovereign policy install baseline.rego         # upload rule pack

# Day-to-day ops
sovereign morning-report                       # the 5-command daily
sovereign tui                                  # interactive dashboard
sovereign health                               # overall health
sovereign certs list --warn-days 21            # TLS certs
sovereign db shell [APP]                       # drop into psql
sovereign db upgrade --check                   # Postgres major-version check
sovereign update                               # self-update

# Sovereignty (V2+)
sovereign sovereignty check                    # 10-point test
sovereign compliance map --standard bsi-c5-2026
sovereign export platform                      # full backup
sovereign import platform --from <file>
sovereign vendor-disappear-test                # the 100-year-old question
```

### 13.2 The 6 framework scanners (`sovereign init`)

```bash
sovereign init fastapi   # detects: pyproject.toml, fastapi, uvicorn
sovereign init nextjs    # detects: package.json, next
sovereign init laravel   # detects: composer.json, laravel/framework
sovereign init go        # detects: go.mod, net/http | gin | echo
sovereign init rails     # detects: Gemfile, rails
sovereign init astro     # detects: package.json, astro
```

Each writes an `app.yaml`:

```yaml
# app.yaml (auto-generated by sovereign init fastapi)
app: api
source:
  github: owner/repo
build:
  dockerfile: Dockerfile  # auto-generated if missing
deploy:
  strategy: bluegreen
  replicas: 1
  resources:
    cpu: 0.5
    memory: 512M
health:
  path: /health
  interval: 10s
  timeout: 5s
  threshold: 3
domain:
  - api.example.com
env:
  - name: PYTHONUNBUFFERED
    value: "1"
secrets:
  - DATABASE_URL
  - STRIPE_KEY
backup:
  postgres:
    schedule: "0 3 * * *"  # daily at 3am
    retention: 30d
    verify: weekly
```

### 13.3 The `sovereign morning-report` output

```text
$ sovereign morning-report
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

---

## 14. Source index (consolidated across all persona files)

Every claim in this document traces to one of:

| File | Lines | Words | What it covers |
|---|---|---|---|
| `Research-1.txt` | 2,459 | ~31,000 | Original spec / master requirement gathering prompt |
| `Research-2.md` | ~700 | ~30,000 | Consolidated synthesis of competitive, market, tech, pain |
| `Research-Report-1.md` | 1,100 | ~9,500 | Tech validation (Rust, Caddy, SQLite, OCI, etc.) |
| `competitive-landscape.md` | 910 | ~9,400 | Competitor deep-dive (12+ tools, Rust wave) |
| `user-pain-research.md` | 523 | ~6,500 | 200+ user pain points with quotes, top 20 ranked |
| `sovereign-runtime-market-research.md` | 369 | ~5,000 | Market + EU sovereignty + cloud repatriation |
| `persona-pm.md` | 634 | ~7,800 | PM lens: day-to-day, dev journey, CLI/TUI UX, docs, pricing |
| `persona-principal.md` | 1,712 | ~11,300 | Principal lens: hexagonal, data model, state machines, API, failure modes |
| `persona-rust.md` | 1,283 | ~7,800 | Rust lens: crate selection, type system, async, perf, testing |
| `persona-devops.md` | 884 | ~6,900 | DevOps lens: morning commands, 3am test, SLOs, alerts, backups, DR, observability |
| `persona-platform.md` | 1,128 | ~9,200 | Platform lens: IDP philosophy, golden paths, service catalog, RBAC, drift |
| `persona-cto.md` | 1,103 | ~12,900 | CTO lens: positioning, funding, moat, build/buy, named mistakes |
| `persona-crosscutting.md` | 843 | ~6,400 | Cross-cutting: governance (rules+ML), USP, sovereignty as engineering |

**Total: ~154,000 words / ~14,500 lines of synthesized research.**

### 14.1 Key external sources (per persona, deduplicated)

**PM lens:** Vercel onboarding, Perspective AI, Skene developer-onboarding, Fly.io deep-dive, gh CLI patterns, lazygit/lazydocker, Clap conventions, mdbook, hyperb1iss/tui-design, getbeton/coolify pricing.

**Principal lens:** howtocodeit/hexarch, dev.to/guuri11 clean-architecture-in-Rust, Tikv, Deno, fd, Kubernetes API patterns, Stripe idempotency, RFC 9457, OpenTelemetry.

**Rust lens:** axum docs.rs, tokio blog, sqlx 0.8, ratatui.rs, clap docs, thiserror/anyhow, theeditorial.news benchmarks, carlriis binary size, Leapcell release optimization, age encryption.

**DevOps lens:** Google SRE Book + Workbook, Atlassian Incident Handbook, incident.io, VictoriaMetrics, Grafana, restic, pgBackRest, GitLab 2017-01-31 postmortem, Cloudflare 2019-07-02, AWS us-east-1 2021-12-07, Facebook 2021-10-04, Linear 2022-09-20, Bessemer State of the Cloud.

**Platform lens:** Backstage, OpsLevel, Cortex, Team Topologies, Frontiers in Computer Science 2026 IDP review, Cloud Magazine 2026, Spotify golden paths 2020/2024, DORA 2026, Spacelift build-vs-buy 2026, rqlite 9.0, PIER single-binary.

**CTO lens:** Plausible $1M ARR + $3.1M ARR, Supabase Series E, Bitwarden Series B, Cal.com, Pangolin YC W25, Sentry $3B+, CasaOS abandonment, Dokploy license update + #3613, Coolify pricing teardown, OpenSSF CVD guide, Rust Foundation governance, EU Commission Sovereign Cloud Framework June 2026, BSI C5:2026, CAIDA, IPCEI-CIS €1.2bn.

**Cross-cutting lens:** OPA / Rego, Facebook Prophet, Google TimesFM, Datadog Watchdog, Cloudflare 2019-07-02 postmortem, GitHub awesome-selfhosted, r/selfhosted 750k+ subs, Plausible 10-point sovereignty.

---

## 15. The final verdict (one paragraph)

Build the engine first (8 features, 6 weeks, 1 developer). Then the product (V1, 12 more weeks). Then the team (V1.5, 5 months). Then the platform (V2, 9 more months). Then the product for the other 90% (V3+). Apache 2.0 unmodified, no carve-outs, no feature gating. EU-incorporated from day 1. Bootstrap to €1M ARR, then a sovereign-tech EU seed. Position as the "Plausible of deployment" — not a PaaS, not an IDP, not "Kubernetes without Kubernetes." A Rust single-binary, GitOps-declarative, AI-agent-friendly, sovereignty-positioned deployment runtime for solo developers, agencies, and EU mid-market. CLI is the product. TUI is the bonus. Web UI is V2 opt-in. Hexagonal architecture with `Arc<dyn Port>` DI, SQLite in V1, rqlite in V2. Layered governance: telemetry + ML scorer (Prophet/TimesFM) + Rego rules + human override. 6-month staged rollout for ML. The 10-point sovereignty test in the CI pipeline, not a wiki page. The vendor-disappear test on every release. The 18-month sustainability signal on day 1. Time-to-first-deploy < 2:30. One-command rollback as the highest-leverage feature. Encrypted zero-disk secrets as the V1 security story. `sovereign backup verify` as the V1.2 reliability story. 6 framework scanners as the V1.3 moat. TUI as the V1.4 daily-driver. Audit log as the V1.5 trust signal. rqlite HA as the V2 reliability story. EUCS Substantial + BSI C5 mapping as the V2 procurement story. Foundation transfer plan by year 3. Plausible as the model, not Supabase. Dokploy + CasaOS + Vercel + Heroku as the anti-patterns. The Plausible of deployment, the Plausible ARR curve, the Plausible bus factor, the Plausible trust signal. This is the founding question of an open-source product: "if you disappear, what happens to the users?" The answer is in the binary, the export, the test, the license, the foundation, the team. Ship it.

---

**End of Research-3.**
