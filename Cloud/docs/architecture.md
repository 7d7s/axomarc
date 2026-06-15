# Architecture

**Status:** Locked for V0/V1. Changes require an ADR.
**Audience:** Every engineer. Read end-to-end before writing code.
**Last updated:** 2026-06-04

This document is the **load-bearing wall**. Every other document in this folder assumes this architecture is in place. If a proposed change violates a section here, the change is rejected.

---

## 1. The architecture pattern: hexagonal, with `Arc<dyn Port>` DI

We use **hexagonal architecture** (a.k.a. ports and adapters). The application core depends on nothing except its own types. All I/O is abstracted behind traits ("ports") that are implemented by adapters in separate crates.

### 1.1 The dependency rule (enforced, not aspirational)

```text
Driving Adapters  ──depends on──>  Application  ──depends on──>  Domain
                                                                  ▲
                                                                  │
Driven Adapters   ───────────────────────────────────────────────┘
   (implement the ports defined in Domain)
```

- **Domain** never imports from Application or Adapters. No `sqlx`, no `reqwest`, no `anyhow`, no `tokio`. Pure Rust types and traits.
- **Application** never imports from Adapters. Owns transaction boundaries, idempotency, span emission. Calls *ports*, which are traits.
- **Driving Adapters** (CLI, TUI, HTTP) import Application, never Domain directly.
- **Driven Adapters** (Docker, Caddy, SQLite, etc.) implement the ports from Domain. They never import from each other except through the ports they share.
- **The composition root** (`main.rs` in the binary crate) is the only place that names concrete adapter types. Everything else is generic over `Arc<dyn Port>`.

**Enforcement:** `cargo-deny` + a custom `cargo-workspace-lint` script that fails CI if a domain crate's `Cargo.toml` lists an adapter crate as a dependency.

### 1.2 The crate layout

```text
sovereign/
├── Cargo.toml                      # workspace root
├── deny.toml                       # cargo-deny config
├── crates/
│   ├── sovereign/                  # binary crate (the `sovereign` you run)
│   │   └── src/
│   │       ├── main.rs             # composition root — ONLY place that names concrete types
│   │       ├── cli.rs              # clap definitions
│   │       ├── tui/                # ratatui dashboard
│   │       ├── http/               # axum router + middleware
│   │       ├── config/             # config loading + validation
│   │       └── shutdown.rs         # SIGTERM/SIGINT handling
│   ├── sovereign-core/             # domain types + use cases
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── domain/             # pure types, no I/O
│   │       ├── use_cases/          # application services
│   │       ├── ports/              # trait definitions ONLY
│   │       └── error.rs            # thiserror enums
│   ├── sovereign-runtime-docker/   # adapter
│   ├── sovereign-runtime-podman/   # adapter (V1.5)
│   ├── sovereign-proxy-caddy/      # adapter
│   ├── sovereign-proxy-nginx/      # adapter (low-mem, V1.1)
│   ├── sovereign-secrets-age/      # adapter
│   ├── sovereign-secrets-sops/     # adapter
│   ├── sovereign-storage-sqlite/   # adapter (V1)
│   ├── sovereign-storage-rqlite/   # adapter (V2)
│   ├── sovereign-buildkit/         # adapter
│   ├── sovereign-git/              # adapter (gix)
│   ├── sovereign-acme/             # adapter
│   ├── sovereign-backup/           # adapter
│   ├── sovereign-notify/           # adapter
│   ├── sovereign-policy-rego/      # adapter (V2)
│   ├── sovereign-ml-scorer/        # adapter (V2)
│   ├── sovereign-observability/    # tracing + /metrics
│   └── sovereign-proto/            # shared types
├── migrations/                     # SQL migrations (sqlx migrate)
├── docs/
│   ├── adr/                        # Architecture Decision Records (see decision-records.md)
│   ├── openapi.yaml                # generated API spec
│   └── operations/                 # runbooks (see operations-runbook.md)
└── .github/
    └── workflows/
        ├── ci.yml
        ├── release.yml
        └── sbom.yml
```

### 1.3 Why `Arc<dyn Port>` DI and not a service-locator / globals

- **Testability:** every use case is a function `fn(&AppState, req) -> Result<R, AppError>` where `AppState: Send + Sync` and contains `Arc<dyn RuntimePort>`, `Arc<dyn StoragePort>`, etc. Tests construct an `AppState` with mocks in 5 lines.
- **Swappability:** the SQLite adapter and the rqlite adapter both implement the same `RuntimeState` trait. V1 ships `SqliteState`; V2 ships `RqliteState`. The composition root changes one line; the use cases do not.
- **No hidden state:** there is no global `DATABASE` or `HTTP_CLIENT`. Every I/O is a parameter, which makes the dataflow auditable from the function signature alone.
- **No `tokio::spawn` of long-running services from inside a use case.** Use cases are short-lived; long-running services (log streaming, health probes, the TUI event loop) live in the binary crate.

### 1.4 What this forbids

- **No `lazy_static!`, `OnceCell`, or `static REGISTRY: ...` in domain or use cases.** Adapter state lives in adapter code, behind a port.
- **No `unwrap()` in domain or use cases.** All errors are `thiserror::Error` enums; the binary crate may use `anyhow::Result` only at the edge.
- **No `tokio::spawn` from inside a use case that outlives the use case.** Use cases are synchronous-ish (they `await` but do not detach).
- **No `sqlx`, `reqwest`, `bollard`, `redis`, etc. in `sovereign-core`.** If you find yourself wanting one, you are writing an adapter, not a use case.
- **No `Box<dyn Any>` or `serde_json::Value` in the domain types.** The domain is typed end-to-end.

---

## 2. The data model (SQLite + WAL, forward-only migrations)

V1 uses **SQLite with WAL mode**. V2 optionally swaps to **rqlite** via the same `RuntimeState` trait. We do not use Postgres. We do not use DynamoDB, FoundationDB, or any other store. The state is a single SQLite file, restorable on any host.

### 2.1 The 7 core tables

```sql
-- 0001_init.sql
CREATE TABLE app (
  id           TEXT PRIMARY KEY,         -- AppId(Uuid) as string
  name         TEXT NOT NULL UNIQUE,
  owner        TEXT NOT NULL,            -- user:alice | system
  env          TEXT NOT NULL CHECK (env IN ('dev','staging','prod')),
  git_repo     TEXT,
  image_ref    TEXT,                     -- last deployed image
  config_yaml  TEXT NOT NULL,            -- last app.yaml
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
  triggered_by  TEXT NOT NULL,           -- user:alice | system | web:github
  risk_score    INTEGER,                 -- 0-100, from ML scorer (V2+)
  policy_decision TEXT,                  -- JSON: {allow, deny, requires_approval, reason}
  error         TEXT,
  version       INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_deployment_app ON deployment(app_id, started_at DESC);

CREATE TABLE domain (
  id           TEXT PRIMARY KEY,
  app_id       TEXT NOT NULL REFERENCES app(id),
  hostname     TEXT NOT NULL UNIQUE,
  tls_status   TEXT NOT NULL,            -- provisioning, valid, expired
  tls_expires  INTEGER,
  created_at   INTEGER NOT NULL
);

CREATE TABLE secret (
  id           TEXT PRIMARY KEY,
  app_id       TEXT NOT NULL REFERENCES app(id),
  key          TEXT NOT NULL,
  ciphertext   BLOB NOT NULL,            -- age-encrypted
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
  status       TEXT NOT NULL,            -- healthy, unreachable, drained
  last_seen    INTEGER NOT NULL,
  cpu_cores    INTEGER,
  mem_mb       INTEGER,
  disk_gb      INTEGER,
  joined_at    INTEGER NOT NULL
);

CREATE TABLE backup (
  id           TEXT PRIMARY KEY,
  target       TEXT NOT NULL,            -- 'postgres:api'
  status       TEXT NOT NULL,            -- pending, success, failed, verified
  size_bytes   INTEGER,
  location     TEXT,                     -- s3://... | /var/lib/sovereign/backups/...
  started_at   INTEGER NOT NULL,
  finished_at  INTEGER,
  verified_at  INTEGER,                  -- last restore-drill time
  verify_result TEXT                     -- JSON: {row_count_diff: [...], tables: [...]}
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
  actor        TEXT NOT NULL,            -- 'user:alice' | 'system' | 'ml:scorer'
  kind         TEXT NOT NULL,            -- 'deploy','rollback','secret.set','access.add',...
  target       TEXT,                     -- 'app:api', 'backup:xyz'
  payload      TEXT NOT NULL,            -- JSON
  policy_decision TEXT
) WITHOUT ROWID;

-- Enforce append-only via triggers
CREATE TRIGGER audit_no_update BEFORE UPDATE ON audit_event
BEGIN SELECT RAISE(ABORT, 'audit_event is append-only'); END;
CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit_event
BEGIN SELECT RAISE(ABORT, 'audit_event is append-only'); END;
```

### 2.2 Migration rules

- **Forward-only.** No `down` migrations in production. Roll back by deploying the previous version.
- **Append-only schema.** New tables, new columns, new indexes. Never `DROP COLUMN` in production. (SQLite supports `ALTER TABLE DROP COLUMN` since 3.35, but we still avoid it for the audit story.)
- **Every migration is a transaction.** A migration that touches 3 tables is one `BEGIN; ... COMMIT;`.
- **Migrations are versioned in git.** `migrations/0001_init.sql`, `migrations/0002_audit.sql`, etc.
- **sqlx runs them at startup** via `sqlx::migrate!()` in the SQLite adapter. If a migration fails, the binary refuses to start.
- **Litestream replicates the WAL** to S3-compatible storage continuously. On restore, replay the WAL to the most recent consistent point.

### 2.3 The append-only audit log

`audit_event` is the source of truth for "who did what, when, with what policy decision." The DB triggers prevent `UPDATE` and `DELETE`. The only way to "remove" an audit event is `VACUUM INTO` a new file (rare; only for GDPR Art. 17 with a documented legal basis).

Every mutating use case calls `audit.append(AuditEvent { ... })` inside the same transaction as the mutation. If the audit append fails, the mutation rolls back. **You cannot deploy without being audited.**

### 2.4 Optimistic concurrency on every row

Every mutable row has a `version INTEGER` column. Every `UPDATE` increments it. Every mutating use case takes an `expected_version` and returns `AppError::Conflict(id, current_version)` if the row was modified in the meantime. The HTTP layer surfaces this as **HTTP 409 with RFC 9457 problem+json**.

This is the simplest way to prevent the "two operators racing on a deploy" footgun. No SELECT FOR UPDATE; no advisory locks; no row-level locking. Just an integer and a check.

---

## 3. State machines — every entity has one

Every stateful entity in the domain has an enum with a `can_transition_to(self, other) -> bool` method. State transitions are the **first code written**, not the last.

### 3.1 Deployment lifecycle

```rust
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

**Why this matters:** every state transition is tested with `proptest` (property-based: "for any two statuses A, B, A.can_transition_to(B) implies B is reachable from A via one or more valid transitions"). The state machine is the **contract** between the use case, the storage adapter, and the HTTP layer.

### 3.2 Other state machines (same pattern)

| Entity | States | Notes |
|---|---|---|
| `App` | `Active, Draining, Archived` | Draining = no new deploys, still serving traffic |
| `Secret` | `Active, Rotating, Retired` | Rotating = new version deployed, old still in-flight |
| `Backup` | `Pending, Success, Failed, Verified` | Verified = restore-drill passed |
| `Server` | `Healthy, Unreachable, Drained, Removed` | Removed is terminal; row kept for audit |
| `Domain` | `Provisioning, Valid, Expired, Failed` | Failed = ACME challenge failed; alert on this |
| `User` | `Active, Disabled` | Disabled = cannot login; existing tokens revoked |

### 3.3 The state machine is the only place transitions are defined

No use case does `db.execute("UPDATE deployment SET status = 'healthy'")` directly. It calls `transition_deployment(deployment_id, DeploymentStatus::Healthy, actor, audit_payload)`. The transition function:

1. Loads the current row.
2. Checks `current.status.can_transition_to(Healthy)`.
3. If yes, updates with `version + 1` and appends an `audit_event` in the same transaction.
4. If no, returns `AppError::Conflict` with the current state.

This is the only pattern that scales past 1 developer. The state machine is the **type-level proof** that the code is correct.

---

## 4. API design — REST + JSON, URL-prefix `/v1`

### 4.1 Endpoint catalog (V1)

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

GET    /v1/apps/{id}/secrets          list secret keys (NEVER values)
PUT    /v1/apps/{id}/secrets/{key}    set secret (zero-disk, never logged)
DELETE /v1/apps/{id}/secrets/{key}    delete secret

GET    /v1/apps/{id}/logs?tail=N&follow=true   SSE stream

GET    /v1/backups                    list backups
POST   /v1/backups                    create backup
POST   /v1/backups/{id}/verify        restore-drill to scratch
POST   /v1/backups/{id}/restore       restore from backup

GET    /v1/servers                    list servers
POST   /v1/servers                    add server
DELETE /v1/servers/{id}               remove server

GET    /v1/audit?since=...&actor=...  query audit log

GET    /v1/policy/rules               list Rego rule packs (V2)
PUT    /v1/policy/rules/{pack}        upload rule pack (V2)
POST   /v1/policy/check               dry-run policy check (V2)
```

### 4.2 Conventions (every endpoint, no exceptions)

- **URL prefix `/v1`.** When V2 ships, it is `/v2` and V1 stays supported for 18 months.
- **Idempotency-Key header** on every mutating request (POST, PUT, PATCH, DELETE). The value is a UUID v4 supplied by the client. The server caches `(idempotency_key, result)` for 24 hours. Re-sending the same key returns the cached result. (Stripe pattern.)
- **ETag / If-Match** for optimistic concurrency. Every `GET` returns an `ETag: W/"<version>"` header. Every `PATCH`/`PUT` requires `If-Match: W/"<expected_version>"`. Mismatch → 409 Conflict.
- **Cursor pagination, never offset.** `?cursor=<opaque>&limit=50`. The cursor is the last seen `id` (encoded). Offset pagination is forbidden in the codebase.
- **Sparse fieldsets.** `?fields=id,name,status` to reduce payload. The default is all fields.
- **RFC 9457 problem+json** for all errors:
  ```json
  {
    "type": "/errors/conflict",
    "title": "Concurrent modification",
    "status": 409,
    "detail": "app api was modified by user:alice at 14:22:03",
    "instance": "/v1/apps/abc",
    "current_version": 7
  }
  ```
- **OpenAPI 3.1 spec** is auto-generated from the `axum` router and committed to `docs/openapi.yaml`. CI fails if the spec is out of sync with the code.
- **Never expose stack traces in errors.** Internal errors return `{"type": "/errors/internal", "status": 500}` with no detail. The detail goes to the audit log.

### 4.3 Streaming endpoints — SSE only

Logs (`/v1/apps/{id}/logs`) and audit follow (`/v1/audit/follow`) use **Server-Sent Events**, not WebSockets. SSE is HTTP, works through every proxy, has built-in reconnect logic, and is what the user expects from a Unix tool.

WebSockets are forbidden in the codebase. Long-lived bidirectional channels are an anti-pattern for a control plane.

### 4.4 Authentication (V1)

- **Local user table** with bcrypt-hashed passwords.
- **Session cookies** for the (V2-only) web UI. HttpOnly, Secure, SameSite=Strict.
- **Bearer tokens** for the CLI and API. Tokens are 32 bytes of `rand::thread_rng()`, base64-encoded. Stored hashed (argon2id) in `user` table. Revocable. Scoped to (app, role).
- **OIDC** in V2 for SSO (consume, do not provide). The product is an OIDC *client*, never a server.

---

## 5. The data flow — a deploy, end to end

```text
User → CLI (clap) → HTTP API (axum) → use_cases::deploy::start
   │                                    │
   │                                    ├─► use_cases::policy::enforce  (Rego rules, V2+)
   │                                    ├─► use_cases::policy::score    (ML scorer, V2+)
   │                                    ├─► ports::Storage::begin_tx
   │                                    ├─► domain::Deployment::new(...)
   │                                    ├─► ports::Storage::commit (with audit append)
   │                                    └─► ports::RuntimePort::deploy (or AgentPort in V1.5)
   │                                              │
   │                                              ▼
   │                                     Container pulled, started, healthchecked
   │                                              │
   │                                              ▼
   │                                     ports::ObservabilityPort::record
   │                                     ports::AuditPort::append
   ▼
CLI prints progress bar, exit code, "what's next" hint
```

The key invariant: **the use case completes (or fails) atomically, with audit**. The container actually starting and passing health checks is a *post-commit* event. If the container fails to start, the deployment row is in `Failed` state, an audit event is appended with the failure reason, and the CLI exits non-zero. **You cannot have a deploy that is "successful" but did not actually deploy.**

---

## 6. Failure modes — what fails, how we recover

| Failure | Detection | Mitigation | Recovery |
|---|---|---|---|
| Docker daemon crashes | `RuntimePort::ping()` fails | Retry 3x with backoff, then drain node | Restart via systemd; agent re-registers |
| Caddy crashes | JSON admin `/config/` returns 502 | Failover to Nginx (V1.1) | Restart via systemd; routes reload from state |
| SQLite corruption | sqlx returns `DatabaseCorrupt` | Restore from Litestream backup | Point-in-time recovery to last good state |
| Server host dies | Heartbeat > 30s | Mark server `drained`; reschedule workloads | rqlite HA in V2; new server `tool server add` |
| TLS cert expires | Health check detects `tls_status: expired` | Auto-renew via Caddy ACME | If renew fails: alert (Sev2), manual fix |
| Secret rotation breaks deploy | Post-deploy health check fails | Auto-rollback to previous version | Operator investigates; policy suggests fix |
| Bad deploy (memory leak) | ML scorer detects RSS growth > baseline | Canary at 5% → check error rate → full or rollback | Auto-rollback; policy learns |
| Disk full | `df` returns > 90% | Alert (Sev2); auto-cleanup of old logs/backups | Operator adds disk; Litestream catches up |
| Network partition (server unreachable) | Heartbeat > 30s | Mark `unreachable`; deploy to other servers first | Partition heals; agent re-registers |
| Audit log corruption | Trigger fires `RAISE(ABORT, ...)` | The mutation that caused it rolls back | Operator investigates; no audit log mutation possible |
| Idempotency key collision | Same key, different request body | Return 422 with "idempotency key reused with different payload" | Client generates fresh key |
| Optimistic concurrency conflict | `version` mismatch on PATCH | Return 409 with `current_version` | Client re-fetches, retries |
| BuildKit OOM during build | BuildKit process exits non-zero | Mark deploy `Failed`; suggest `--image=...` bypass | Operator reduces build parallelism, retries |

**The single most important failure mode to test:** *the backup-restore path.* Backups you have not restored are hopes. (`sovereign backup verify --restore-to scratch` is the highest-leverage V1.2 command — see [`phase-01-v1.md`](./phase-01-v1.md).)

### 6.1 The diagnostic engine (`sovereign doctor`)

Every failure mode in the table above is mirrored by a **`sovereign doctor` check** in one of the 14 categories (system, binary, storage, runtime, proxy, secrets, backup, network, agents, observability, security, sovereignty, performance, cost). The doctor is the *on-call's first command* in every failure scenario: it surfaces which of the 14 categories is unhealthy, the specific check that failed, a one-line explanation, a KB page, and (in most cases) a `--fix` that resolves the issue non-interactively.

Doctor is shipped in every phase and promoted through 5 levels:
- **V0 basic** (10 checks, 6 categories) — the install gate; CI runs it on every PR.
- **V1 standard** (40+ checks, all 14 categories) — the daily-driver; `sovereign doctor --watch --level standard` runs continuously.
- **V1.5 full** (70+ checks, every category saturated) — `sovereign doctor --level full` plus `sovereign doctor fleet` and `sovereign doctor remote <host>` for multi-server.
- **V2 paranoid** (100+ checks, the 10-point sovereignty test as a check group) — `sovereign doctor --level paranoid` plus `sovereign sovereignty-test` and `sovereign compliance scan` for EU public-sector audits.
- **Custom** — operator-defined checks via the `DoctorExtension` trait.

The doctor is the **single point of truth** for "is this system healthy?" — the on-call's first command, the CI gate, the SLO tripwire (`/etc/cron.d/sovereign-doctor` runs standard level every 5 min), the auditor's command, the customer's confidence. The full spec is in [`doctor.md`](./doctor.md).

The doctor is built as a hexagonal use case (`sovereign-doctor/src/doctor.rs`); each check is a port impl (`sovereign-doctor/src/checks/<category>.rs`); each fix is a port impl (`sovereign-doctor/src/fixes/`); the levels are composed via the `DoctorExtension` trait. This matches the rest of the architecture: the doctor is not a special case, it is another use case that happens to be cross-cutting.

---

## 7. Multi-tenancy — soft isolation, one DB, `user.role` + `app.owner`

The spec covers solo developers, freelancers, agencies, and startups. The implication: **soft multi-tenancy, not hard isolation.** One SQLite, one binary, multiple users, role-based access. No "tenant ID" column.

### 7.1 Why not hard isolation

Hard isolation = separate processes per tenant, separate SQLite per tenant, separate everything. That is Coolify's per-app Docker setup. It does not scale operationally past ~30 apps without a platform team. The single-tenant case is just the trivial multi-tenant case; supporting both is two architectures. We do not do two architectures.

### 7.2 The model

- `user.role` ∈ `{owner, admin, developer, readonly}` — global role, one user.
- `app.owner` — string, the `app` belongs to one user (or a team name).
- Every mutating endpoint checks: *does the actor have the right role AND does the actor own (or have admin over) the target?*
- The agency use case (Karim with 12 clients) is "12 teams, one binary." Each `app.owner` is a client. The agency admin can deploy to any client; the client dev can only deploy to their own.

### 7.3 RBAC matrix (V1.5)

| Action | owner | admin | developer | readonly |
|---|---|---|---|---|
| List all apps | ✓ | ✓ | own only | own only |
| Create app | ✓ | ✓ | ✓ | ✗ |
| Deploy app | ✓ | ✓ | own only | ✗ |
| Rollback | ✓ | ✓ | own only | ✗ |
| Read secrets | ✓ | ✓ | own only | ✗ |
| Write secrets | ✓ | ✓ | own only | ✗ |
| Add user | ✓ | ✓ | ✗ | ✗ |
| Change user role | ✓ | ✗ | ✗ | ✗ |
| View audit log | ✓ | ✓ | own only | own only |
| Export audit log | ✓ | ✓ | ✗ | ✗ |
| `sovereign sovereignty check` | ✓ | ✓ | ✗ | ✗ |
| `sovereign compliance map` | ✓ | ✓ | ✗ | ✗ |

---

## 8. Observability — the three pillars

### 8.1 Logs (structured, JSON, stdout)

- `tracing` with `tracing-subscriber`'s JSON formatter.
- Env var `RUST_LOG` controls the filter (default: `info,sovereign=debug`).
- Every log line has: `ts`, `level`, `target`, `msg`, `span` context, plus arbitrary fields.
- No PII. No content of secrets. No deploy content. Only metadata.
- Logs are tee'd to `/var/log/sovereign/*.log` (rotated by `logrotate`) and to the VictoriaLogs adapter (V1.2+).

### 8.2 Metrics (Prometheus format, `/metrics` endpoint)

- `metrics` facade + `metrics-exporter-prometheus`.
- Endpoint: `GET /metrics` on the control plane port (default `127.0.0.1:7878`).
- Format: Prometheus text exposition. Scraped by VictoriaMetrics or any other collector.
- Mandatory metrics:
  - `deploys_total{app, env, status}` (counter)
  - `deploy_duration_seconds{app, env}` (histogram)
  - `rollback_total{app, reason}` (counter)
  - `audit_events_total{kind}` (counter)
  - `http_request_duration_seconds{path, method, status}` (histogram)
  - `container_cpu_usage{app, server}` (gauge)
  - `container_memory_usage{app, server}` (gauge)
  - `backup_duration_seconds{target}` (histogram)
  - `backup_size_bytes{target}` (gauge)
  - `policy_evaluations_total{decision, rule}` (counter, V2+)
  - `risk_score{app, env}` (gauge, V2+)

### 8.3 Traces (OpenTelemetry, opt-in)

- `tracing-opentelemetry` is feature-flagged behind `--features otel`.
- When enabled, spans are exported to an OTLP endpoint (configured via `OTEL_EXPORTER_OTLP_ENDPOINT`).
- When disabled, traces are still emitted in-process for `tracing` log correlation, but no remote export.
- The default is **off** for V1. Most users will not enable it. The V1.5+ default for Pro tier is **on**.

---

## 9. The composition root — `main.rs`

The composition root is the **only place** that names concrete adapter types. Everything above it is generic over `Arc<dyn Port>`.

```rust
// crates/sovereign/src/main.rs
use std::sync::Arc;
use sovereign_core::ports::{RuntimePort, StoragePort, ProxyPort, SecretsPort, AuditPort, ...};
use sovereign_runtime_docker::DockerRuntime;
use sovereign_storage_sqlite::SqliteState;
use sovereign_proxy_caddy::CaddyProxy;
use sovereign_secrets_age::AgeSecrets;
use sovereign_observability::init_tracing;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cfg = sovereign::config::load().await?;
    
    // Wire concrete adapters into Arc<dyn Port>
    let runtime: Arc<dyn RuntimePort> = Arc::new(DockerRuntime::connect(&cfg.docker).await?);
    let state:   Arc<dyn StoragePort> = Arc::new(SqliteState::open(&cfg.db).await?);
    let proxy:   Arc<dyn ProxyPort>   = Arc::new(CaddyProxy::connect(&cfg.caddy).await?);
    let secrets: Arc<dyn SecretsPort> = Arc::new(AgeSecrets::open(&cfg.secrets).await?);
    let audit:   Arc<dyn AuditPort>   = state.clone(); // SQLiteState implements AuditPort too
    
    // Build the AppState (lives in sovereign-core)
    let app_state = sovereign_core::AppState::new(runtime, state, proxy, secrets, audit);
    
    // Choose the driver: CLI args, TUI subcommand, or HTTP server
    match sovereign::cli::parse().cmd {
        sovereign::cli::Cmd::HttpServer => {
            sovereign::http::serve(app_state, cfg.http).await?;
        }
        sovereign::cli::Cmd::OneShot(cmd) => {
            sovereign::cli::dispatch(cmd, app_state).await?;
        }
    }
    Ok(())
}
```

**V1.5+** adds an `agent` mode that runs an HTTP server that takes deploy instructions from a remote control plane. The same composition root, a different config flag.

**V2+** optionally swaps `SqliteState` for `RqliteState`. Same `Arc<dyn StoragePort>`, one line of config. Every use case is unchanged.

---

## 10. What this architecture buys us

- **Testability:** every use case is a pure function over `AppState`. Mocks are 5 lines.
- **Swappability:** swap SQLite for rqlite (V2), Docker for Podman (V1.5), Caddy for Nginx (V1.1), age for Vault (V3) — all one line in `main.rs`.
- **Recoverability:** every mutating operation is in a transaction with an audit append. Either the change is fully recorded, or it didn't happen. There is no in-between.
- **Auditability:** the audit log is the source of truth. The binary can be killed at any time, and the state on disk is consistent.
- **Portability:** the state is a single SQLite file. The binary is a single static binary. A vendor-disappear test (`sovereign export` + fresh-install + `sovereign import`) is the founding promise.

### 10.1 The V1.5 extensions — the 4 first-class use cases

The V1.5 phase adds 4 use cases (H20-H23 in `phase-02-v15.md`) that are *not* in V0/V1 because they were not in the original persona research. They were added in response to the market-gap analysis against Coolify, Dokploy, CapRover, and the Heroku Feb 2026 sustaining-engineering announcement. They follow the same hexagonal pattern — they are use cases in `sovereign-core/`, with port traits in `sovereign-ports/`, and adapters in `sovereign-adapters-{import,dns,dbpool,statuspage}/`.

- **`ImportApp` (H20)** — `sovereign import --from {heroku,coolify,dokploy,render}` is a use case that takes a platform-specific export (tarball or JSON) and produces the same output as the `InitApp` use case in V0: a populated `app`, `service`, `domain`, `env`, and `db` table rowset. The adapter is `sovereign-adapters-import/`; the source-specific parsers (`heroku.rs`, `coolify.rs`, `dokploy.rs`, `render.rs`) are separate files. The use case logs to the audit table as `import.<source>`; the entire import is idempotent (re-running on the same archive produces the same app ID).
- **`DnsManager` (H21)** — `sovereign dns` is a use case over the `DnsPort` trait. The 7 adapters (`cloudflare.rs`, `hetzner.rs`, `ovh.rs`, `gandi.rs`, `namecheap.rs`, `porkbun.rs`, `rfc2136.rs`) all implement `DnsPort`; the use case does not know which provider it's talking to. Tokens are stored in the V0 secret store (F7), encrypted at rest, never logged.
- **`DbPoolManager` (H22)** — `sovereign db pool` is a use case that provisions a connection pool in front of an existing DB. The 3 adapters are: `pgbouncer.rs` (downloads a static binary, writes a config, runs it as a systemd unit); `proxysql.rs` (same pattern, MySQL); `redis_pool.rs` (in-Rust, no external process, uses the `deadpool-redis` crate). The use case is a no-op for apps that already have their own pool (the app is responsible for declaring it).
- **`StatusPage` (H23)** — `sovereign status page` is a use case that publishes a self-contained HTML page from the `app`, `health_check`, and `incident` tables. The output is one `.html` file, no JS framework, no external font, served by the same Caddy process. The use case is opt-in (`sovereign status page enable --domain status.<box-hostname>`).

The hexagonal pattern means each of these can be swapped (e.g., Hetzner DNS for Cloudflare, PgBouncer for the V2 native pool) without touching the use case. The tests use the same fixture pattern as V0: a `heroku-export.json` fixture produces the same `app` table rowset regardless of which `DnsPort` adapter is wired.

### 10.2 The footprint claim — the load-bearing performance promise

The 80-150 MB RSS / 512 MB VPS / 10-25 MB binary claim is **architecturally enforced**, not just a benchmark result:

- **No bundled agents** (no Datadog, no New Relic, no Prometheus node_exporter) — the sovereign binary collects its own metrics via `/proc` and `/sys` reads.
- **No background services** besides the binary itself, Caddy, Docker, and SQLite. PgBouncer (H22) and the status page (H23) are opt-in.
- **No Java, no Node, no Python in the runtime path** — the only interpreter the operator needs to install is the one for *their app's* Dockerfile.
- **Static musl binary, no glibc bloat** — the release profile is in `tech-stack.md` §2; the CI fixture `cx22-small` runs the footprint test on every release.
- **One SQLite file, no separate cache DB, no Redis for the runtime path** — the only Redis is if the operator's *app* uses it.

This is the headline on the landing page and in every Show HN post: "80-150 MB RAM. Fits in 512 MB VPS. Rust single binary. No panel."

---

## 11. What this architecture forbids

- **No `mysql`, `postgres`, `redis` directly in `sovereign-core`.** SQLite is the only embedded store. Anything else is a port.
- **No `diesel`, `sea-orm`, or any ORM in `sovereign-core`.** sqlx is the only DB layer; it lives in the storage adapter crate.
- **No `tokio::spawn` of long-running services from inside a use case.** The use case is short-lived; long-running services live in the binary.
- **No `serde_json::Value` in domain types.** The domain is typed; JSON is the wire format at the edges.
- **No `Box<dyn Any>` or type erasure in domain types.** If you cannot name the type, you are not done designing.
- **No `unsafe` outside of explicitly-justified, audited, fuzz-tested modules.** The 2026 rule: `cargo-geiger` reports zero `unsafe` in 99% of the codebase. The 1% is `age`, `sqlx`'s SQLite binding, and one or two hot-path SIMD optimizations — each with a comment explaining why and a fuzz test that runs in CI.
- **No reflection, no `lazy_static!`, no `once_cell` in domain or use cases.**
- **No global state of any kind.** If you need shared state, it is in `AppState`, which is constructed in `main.rs` and passed by reference.

### 11.1 The V1 enterprise extensions — auth, log shipping, immutable audit

The V1 phase adds 3 first-class use cases (G21, G22, G23) that are the SOC 2 / ISO 27001 / BSI C5 / EUCS foundation. They follow the same hexagonal pattern as the V0/V1 use cases — use case in `sovereign-core/`, port trait in `sovereign-ports/`, adapter in a separate crate.

- **`AuthnApp` (G21)** — `sovereign auth` is a use case over the `AuthPort` trait. The 3 adapters are `oidc.rs` (OIDC consumer via the `openidconnect` crate), `saml.rs` (SAML wrapper via the `saml2` crate, used only when the IdP mandates SAML), and `local_admin.rs` (the bcrypt-hashed local admin fallback). The use case does not own the IdP; it consumes OIDC claims and resolves the RBAC role. Tokens are stored in the V0 secret store (F7), encrypted at rest, never logged. The full role map + the SAML wrapper + the MFA enforcement live in `crates/sovereign-auth/`.
- **`LogShipApp` (G22)** — `sovereign log ship` is a use case over the `LogSinkPort` trait. The 7 adapters (`loki.rs`, `datadog.rs`, `betterstack.rs`, `splunk_hec.rs`, `sumo.rs`, `syslog.rs`, `vector.rs`) all implement the same trait. The use case does batching (1 MB / 1000 events / 5s), exponential backoff, redaction (the 6 documented secret patterns), and queue depth tracking. The full sink list + the decision matrix + the day-2 runbook are in `operations-runbook.md` §6.5.
- **`AuditChainApp` (G23)** — `sovereign audit` is a use case that writes to the `audit` table through the `AuditWriterPort` trait. The implementation is the append-only VFS (a custom SQLite VFS that rejects all non-INSERT statements) and the Ed25519 chain (each row has a `prev_hash` + a `signature`; the chain is verifiable via `sovereign audit verify`). The 3 export formats (CSV / JSONL / Parquet) are in the same crate. The 90-day hot + 1-year cold retention is enforced by a daily cron. The full append-only VFS + the Ed25519 chain + the Parquet schema are in `crates/sovereign-audit/`.

These 3 extensions are the load-bearing pieces for the enterprise tier (`enterprise-readiness.md` §1.3). Without them, the SOC 2 / ISO 27001 / BSI C5 / EUCS mappings in `enterprise-readiness.md` §3 are paper; with them, the mappings are verifiable by the auditor via `sovereign audit verify` and `sovereign compliance scan` (I17).

### 11.2 The end-user artifact (the static binary, the packages, the container)

The architecture produces **one** binary that is the source of truth, and **four** packaging formats that are convenience artifacts (the deb, the rpm, the apk, the container image). All 4 are built from the same source by the same `cargo-dist` release pipeline. The full packaging contract (the `Cargo.toml` config, the Dockerfile, the Debian policy, the verification steps) is in `tech-stack.md` §11.

The `install.sh` is in `crates/sovereign/src/scripts/install.sh`, ~120 lines of bash, audited on every release. The contract (the 9 steps, the security properties, the failure recovery matrix) is in `tech-stack.md` §12.

The user-facing install/upgrade/uninstall story is in `product-ux.md` §11.

---

**Next: read [`tech-stack.md`](./tech-stack.md) for the crate selection, the static linking contract, and the packaging format, and [`enterprise-readiness.md`](./enterprise-readiness.md) for the support tiers, SLAs, framework mappings, and the enterprise onboarding playbook.**
