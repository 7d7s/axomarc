# Sovereign Application Runtime — Principal Engineer / Architect Report

**Date:** 2026-06-03
**Author persona:** Principal Engineer / Architect
**Subject:** Deep architectural research and decision record for a Rust-based, self-hosted, single-binary deployment platform
**Source corpus:** `Research-1.txt`, `Research-2.md`, `Research-Report-1.md`, `competitive-landscape.md`

This document is the architectural view that sits one layer above the consolidated research. The previous reports tell us **what to build and why**. This report tells us **how to build it so it does not collapse under its own weight at 100 servers, 1,000 apps, or 10,000 users**. Everything below is opinionated. Where the spec is silent, this document speaks.

---

## 0. Architectural North Star

Before any individual decision, three first principles govern the whole design:

1. **Local control plane is a load-bearing wall, not a product detail.** The single binary is not a marketing claim; it is a *correctness* claim. Every dependency that cannot be statically linked or self-contained in a single `musl` binary is rejected by default. No glibc. No Python. No Node. No Java. No external databases in V1. No external message brokers. The binary must run on a fresh Hetzner Ubuntu 24.04 box with `apt install docker.io` and nothing else.
2. **State is everything; code is replaceable.** Every action is a state transition. Every state transition is auditable, reversible where safe, and recoverable from a backup. The control plane's database is the system of record; everything else is a derived projection.
3. **The single-server experience must be a subset of the multi-server experience.** No "easy mode" that cannot scale. If a feature only works on one box, it is incomplete. This is the rule that makes rqlite adoption in V2 non-breaking.

The spec's "V1 single server, V1.5 agent pattern, V2 rqlite HA" roadmap is the correct one. This report adds the layer boundaries, data model, and failure modes that make that roadmap actually work.

---

## 1. System Design

### 1.1 The architecture that fits

The spec already has the right intuition: ports and adapters (a.k.a. hexagonal architecture). The question is how to instantiate it in Rust idiomatically without the ceremony that makes Java/C# hexagonal projects unreadable.

**Recommendation: Hexagonal with Rust-style trait objects, not full DDD with aggregates, value objects, and domain events.**

The 2025–2026 Rust community has converged on a pattern that works: **module-per-bounded-context, trait-for-port, struct-for-adapter, `Arc<dyn Port>` for dependency injection at the composition root**. The `howtocodeit/hexarch` reference implementation is the cleanest example ([github.com/howtocodeit/hexarch](https://github.com/howtocodeit/hexarch)). The `dev.to/guuri11` clean-architecture-in-Rust writeup ([dev.to/guuri11/an-opinionated-clean-architecture-in-rust-4jn8](https://dev.to/guuri11/an-opinionated-clean-architecture-in-rust-4jn8)) shows the layered alternative. Both are defensible; hexagonal is leaner for our case.

**Why not DDD aggregates and value objects?** A deployment control plane is fundamentally about *commands* and *their effects on long-lived state*. The domain is not rich enough to need aggregates with invariants spanning multiple entity types. A `Deployment` row has a status enum, a target, and a created_at. We do not need aggregate roots. We need *transactions*.

**Why not event sourcing?** The audit log is an event stream, but it is not the source of truth — the SQL tables are. Event sourcing doubles the cognitive load and the disk write rate. We keep the audit log as a *projection* off the same SQLite transactions, not as a source of truth.

**Why not CQRS?** For a control plane with < 100 writes/sec at peak, the read and write paths share the same SQLite. Splitting them adds a sync problem with no performance benefit. Defer to V3 if we ever run a separate read replica.

**Why not the actor model?** Rust does not have a mainstream actor framework (actix's actor model is the framework's internal model, not a separate paradigm). Tokio tasks with `Arc<RwLock<...>>` or channels are the idiomatic answer for the few shared-mutable-state cases we have (the in-memory cache of deployment status, the active deployment locks).

### 1.2 The layer diagram (final)

```text
┌────────────────────────────────────────────────────────────────────┐
│                          Driving Adapters                          │
│   cli (clap)   │   tui (ratatui)   │   http/api (axum)            │
└────────────────────────┬───────────────────────────────────────────┘
                         │ calls
                         ▼
┌────────────────────────────────────────────────────────────────────┐
│                       Application Layer                            │
│   use_cases::deploy::start   │   use_cases::secret::rotate         │
│   use_cases::rollback        │   use_cases::backup::verify         │
│   use_cases::agent::dispatch │   use_cases::audit::record          │
│                                                                    │
│   Owns: transaction boundaries, idempotency keys, span emission.   │
│   No I/O of its own — only via ports.                              │
└────────────────────────┬───────────────────────────────────────────┘
                         │ calls
                         ▼
┌────────────────────────────────────────────────────────────────────┐
│                            Domain Core                             │
│   domain::deployment   │   domain::secret   │   domain::server     │
│   domain::app          │   domain::backup   │   domain::audit      │
│                                                                    │
│   Pure Rust types (no sqlx, no reqwest, no anyhow at the edges).   │
│   `DeploymentStatus` enum with `can_transition_to(self, other)`.   │
│   Newtypes: `AppId(Uuid)`, `DeploymentId(Uuid)`, `ServerId(Uuid)`. │
└────────────────────────┬───────────────────────────────────────────┘
                         │ calls
                         ▼
┌────────────────────────────────────────────────────────────────────┐
│                       Driven Adapters (Ports impl)                 │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌────────────┐ │
│  │ runtime::    │ │ proxy::      │ │ secrets::    │ │ storage::  │ │
│  │  docker      │ │  caddy       │ │  age         │ │  sqlite    │ │
│  │ runtime::    │ │ proxy::      │ │ secrets::    │ │ storage::  │ │
│  │  podman      │ │  nginx       │ │  sops        │ │  rqlite    │ │
│  │ build::      │ │ acme::       │ │ backup::     │ │ queue::    │ │
│  │  buildkit    │ │  caddy_acme  │ │  s3          │ │  sqlite    │ │
│  │ git::        │ │ tls::        │ │ notify::     │ │            │ │
│  │  gix         │ │  rustls      │ │  webhook     │ │            │ │
│  └──────────────┘ └──────────────┘ └──────────────┘ └────────────┘ │
└────────────────────────────────────────────────────────────────────┘
```

**Dependency rule (enforced by `cargo-deny` and review):** Domain never imports from Application. Application never imports from Adapters. Adapters never import from each other except through ports. The composition root (`main.rs`) is the only place that knows which concrete adapter is wired in.

### 1.3 Cargo workspace layout

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
│   │   └── examples/
│   │       └── tool.toml           # sample config file
│   ├── sovereign-core/             # domain types + use cases
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── domain/             # pure types, no I/O
│   │   │   ├── use_cases/          # application services
│   │   │   ├── ports/              # trait definitions only
│   │   │   └── error.rs            # thiserror enums
│   │   └── tests/                  # domain unit tests
│   ├── sovereign-runtime-docker/   # adapter: Docker
│   ├── sovereign-runtime-podman/   # adapter: Podman
│   ├── sovereign-proxy-caddy/      # adapter: Caddy
│   ├── sovereign-proxy-nginx/      # adapter: Nginx (low-mem)
│   ├── sovereign-secrets-age/      # adapter: age envelope
│   ├── sovereign-storage-sqlite/   # adapter: sqlite (V1)
│   ├── sovereign-storage-rqlite/   # adapter: rqlite (V2)
│   ├── sovereign-buildkit/         # adapter: BuildKit
│   ├── sovereign-git/              # adapter: gix or git2
│   ├── sovereign-acme/             # adapter: ACME client
│   ├── sovereign-backup/           # adapter: pg_dump + S3
│   ├── sovereign-notify/           # adapter: webhook + email + tg + slack
│   ├── sovereign-observability/    # tracing setup, /metrics
│   └── sovereign-proto/            # shared types (events, snapshots)
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

This layout follows the precedent of [tikv](https://github.com/tikv/tikv), [deno](https://github.com/denoland/deno), and [fd](https://github.com/sharkdp/fd): a thin binary crate that depends on many small, single-purpose library crates. The advantage is that each adapter can be tested in isolation, swapped out at runtime via feature flags, and reasoned about independently.

### 1.4 Data flow: a deploy, end to end

```text
User → CLI (clap) → HTTP API (axum) → use_cases::deploy::start
   │
   ▼
   1. Begin SQLite transaction (busy_timeout=5000)
   2. Read App row, lock with SELECT ... FOR UPDATE? (SQLite: BEGIN IMMEDIATE)
   3. Compute next DeploymentId, version label, target image tag
   4. INSERT deployment (status=Pending, image_tag, started_at, request_id)
   5. COMMIT
   6. Spawn tokio task for the actual work
   │
   ▼
   background task: deploy::execute
   ├─ git::clone_or_pull(source)
   ├─ build::build(context, cache=s3://cache-bucket) → image_ref
   ├─ runtime::pull(image_ref)
   ├─ runtime::start_new_container(image_ref, env=resolved_secrets, ...)
   ├─ health::wait_healthy(new_container, timeout=120s)
   │    └─ on fail → runtime::stop + audit::record(Failed) + use_cases::rollback::trigger
   ├─ proxy::swap_route(app_id, from=old → to=new)
   ├─ runtime::stop_old_container(app_id, drain=30s)
   ├─ audit::record(Success) + UPDATE deployment status=Succeeded
   └─ notify::emit(deploy.succeeded webhook)
```

Every step emits a `tracing` span. Every state change writes an audit event in the *same* SQLite transaction. If the process dies mid-deploy, the next startup detects the `Pending` deployment and either resumes (idempotent steps) or marks it `Failed` and rolls back.

---

## 2. API Design

### 2.1 Transport: REST + JSON over HTTP, with UNIX-socket fast path

**Recommendation: HTTP/1.1 + JSON over the loopback interface as the primary API.** No gRPC, no MessagePack, no GraphQL.

Why:
- Every existing tool (Kubernetes, Docker Engine, Dokploy, Coolify, Fly) speaks REST + JSON. Operators have muscle memory.
- The CLI lives on the same host as the server in 90% of cases; UNIX-socket transport is an optimization, not a requirement.
- gRPC's codegen cost (`.proto` files, build script, generated stubs) is not justified at our scale.
- MessagePack-RPC has no Rust story; rmpc is a library, not a framework.
- GraphQL is the wrong abstraction for a control plane that is fundamentally command-oriented (deploy, rollback, restart), not query-oriented (the read path is small and predictable).

For V1.5, when agents connect over the network, use **HTTP/2 with mTLS** over the LAN. The same axum routes, different transport. This avoids designing two APIs.

### 2.2 Versioning strategy: URL-prefix versioning, with a strict compatibility contract

**Recommendation: URL-prefix versioning (`/v1/...`)** from day 1, modeled on the Kubernetes pattern ([kubernetes.io/docs/reference/using-api/api-concepts](https://kubernetes.io/docs/reference/using-api/api-concepts/)).

Three rules from Kubernetes that we adopt:
1. **Alpha** endpoints live under `/v1alpha1/...` and may break at any minor version.
2. **Beta** endpoints under `/v1beta1/...` — stable for 9 months or 3 minor versions after deprecation, whichever is longer.
3. **GA** endpoints under `/v1/...` — backward-compatible within the `v1` lifecycle; breaking changes require `v2`.

In practice, for the V1 launch, we ship `/v1/` only and treat it as GA. We do not advertise alpha/beta endpoints externally; they exist only as feature-flagged routes during development.

The "no breaking changes within v1" rule means:
- Adding a field to a response is OK.
- Adding an optional field to a request is OK.
- Adding a new endpoint is OK.
- Removing a field, renaming a field, changing a type, or making a field required is a breaking change and requires a new API version.

### 2.3 Idempotency: required on every mutating endpoint

**Recommendation: Stripe-style idempotency keys on all POST/PUT/PATCH/DELETE.** Every client that wants to retry safely sends `Idempotency-Key: <uuid>`. The server stores `(key, endpoint, request_body_hash, response_status, response_body)` in a `idempotency_keys` table, with TTL of 24 hours. A second request with the same key returns the cached response, regardless of whether the original succeeded or failed. This is exactly the Stripe design ([stripe.com/blog/idempotency](https://stripe.com/blog/idempotency)).

```rust
// In axum middleware
async fn idempotency_layer(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let key = headers.get("idempotency-key").and_then(|v| v.to_str().ok());
    let Some(key) = key else {
        return next.run(request).await;  // GETs pass through
    };
    if request.method() == Method::GET {
        return next.run(request).await;
    }
    let body = request.body_bytes().await.unwrap();
    let body_hash = blake3::hash(&body);
    match state.idempotency_store.lookup(&key, &body_hash).await {
        Some(cached) => cached.into_response(),
        None => {
            let response = next.run(request).await;
            state.idempotency_store.store(&key, &body_hash, &response).await;
            response
        }
    }
}
```

### 2.4 Pagination: cursor-based, never offset

**Recommendation: Cursor-based pagination for all list endpoints.** `?limit=50&cursor=eyJpZCI6IjAxSiJ9`.

Why not offset:
- Offset pagination on SQLite with `LIMIT 50 OFFSET 10000` requires the DB to scan and discard 10,050 rows. Slow at scale.
- Offset is unstable when rows are inserted/deleted between requests. Cursor is stable.
- Cursor is the industry standard (Kubernetes, Stripe, GitHub).

Cursor encoding: base64url(JSON({ id, created_at })). For audit-log endpoints where time-range queries dominate, the cursor can be `(created_at, id)` lexicographic. The token is opaque to the client; the server owns the format.

```rust
#[derive(Serialize, Deserialize)]
struct Cursor {
    id: i64,                  // monotonic rowid
    created_at: DateTime<Utc>,
}

fn encode_cursor(c: &Cursor) -> String {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(c).unwrap())
}
```

### 2.5 Filtering, sorting, sparse fieldsets

**Recommendation: A small, fixed set of filter and sort parameters per resource.** No generic `?filter[foo]=bar` DSL — that is the road to implementation complexity and security bugs. Instead, each resource documents its supported filters:

```
GET /v1/apps?environment=production&status=running&sort=-created_at&limit=50
```

Sort syntax: `sort=field` for ascending, `sort=-field` for descending. Compound sort: `sort=-priority,created_at`.

Sparse fieldsets (return only the fields the client asks for):

```
GET /v1/apps?fields=id,name,status,updated_at
```

This is the Facebook Graph API / Cloudflare API pattern. It dramatically reduces payload size for TUI clients polling the dashboard.

### 2.6 Error format: RFC 9457 Problem Details

**Recommendation: RFC 9457 (the 2023 successor to RFC 7807) for every error response.** No "always return 200 with an error field" anti-pattern.

```json
{
  "type": "https://docs.sovereign.app/errors/invalid-image-ref",
  "title": "Invalid image reference",
  "status": 422,
  "detail": "Image reference 'ghcr.io/me/api:@sha256:not-a-real-digest' is not a valid OCI reference",
  "instance": "/v1/apps/api/deployments",
  "request_id": "req_01HZX4K9...",
  "errors": [
    { "field": "image_ref", "message": "must be a valid OCI image reference" }
  ]
}
```

The `type` URI is a stable error code clients can switch on. The `request_id` ties back to the `tracing` span and the audit log row.

### 2.7 Reference API surface (V1)

```text
GET    /v1/health                              (liveness, no auth)
GET    /v1/ready                               (readiness, no auth)
GET    /v1/version                             (build info)

# Apps
GET    /v1/apps                                list
POST   /v1/apps                                create
GET    /v1/apps/{id}                           read
PATCH  /v1/apps/{id}                           update
DELETE /v1/apps/{id}                           delete (soft)

# Deployments
GET    /v1/apps/{id}/deployments               list (cursor-paginated)
POST   /v1/apps/{id}/deployments               start deploy
GET    /v1/deployments/{id}                    read status
POST   /v1/deployments/{id}/rollback           rollback to a previous version
GET    /v1/deployments/{id}/logs               stream logs (SSE)

# Domains
GET    /v1/apps/{id}/domains
POST   /v1/apps/{id}/domains
DELETE /v1/apps/{id}/domains/{domain}

# Secrets
GET    /v1/apps/{id}/secrets                   list names only, never values
PUT    /v1/apps/{id}/secrets/{key}             set
DELETE /v1/apps/{id}/secrets/{key}             delete
POST   /v1/apps/{id}/secrets/{key}/rotate      generate new value, redeploy

# Backups
GET    /v1/backups                             list
POST   /v1/backups                             create
GET    /v1/backups/{id}                        read
POST   /v1/backups/{id}/restore                trigger restore
POST   /v1/backups/{id}/verify                 restore to scratch, row-count check

# Servers (V1.5+)
GET    /v1/servers
POST   /v1/servers                             bootstrap new agent
DELETE /v1/servers/{id}

# Audit (V1.5+)
GET    /v1/audit                               filter by user/action/app/time
GET    /v1/audit/{id}                          full event with payload

# Agents (V1.5+)
GET    /v1/agents/{id}/heartbeat               agent reports
```

Every POST/PUT/DELETE accepts `Idempotency-Key`. Every error uses RFC 9457. Every list endpoint supports cursor pagination, sort, and sparse fieldsets.

---

## 3. Data Model Design

### 3.1 Storage: SQLite in V1, rqlite in V2, no Postgres ever

**Recommendation: SQLite (WAL) for V1. rqlite (SQLite + Raft) for V2. Do not write a Postgres migration path.**

The V2 architecture swap from single-node SQLite to rqlite is a drop-in replacement of the `storage` adapter, not a schema rewrite. rqlite speaks SQL; the application's query patterns are unchanged. This is the single most important architectural decision for multi-server ([rqlite.io/docs/design](https://rqlite.io/docs/design/)).

The spec already covers why SQLite is the right choice for a control plane: <100 writes/sec, <1 GB, indexed lookups, time-range queries. Forward Email runs the same shape in production. Rails 8 ships SQLite as the default. The research in `Research-Report-1.md` §5 validates the 100,000 TPS / billion-row benchmark class.

**Pragmas to set on every connection:**

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA cache_size = -65536;        -- 64 MB
PRAGMA mmap_size = 268435456;      -- 256 MB
PRAGMA temp_store = MEMORY;
PRAGMA foreign_keys = ON;
PRAGMA wal_autocheckpoint = 1000;  -- default, tune if write-heavy
```

### 3.2 Schema (V1 core)

The schema is the contract between every subsystem. Every column is justified.

```sql
-- 0001_init.sql

CREATE TABLE schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    description TEXT NOT NULL
);

CREATE TABLE apps (
    id            TEXT PRIMARY KEY,            -- ULID
    name          TEXT NOT NULL UNIQUE,        -- app name, slug
    description   TEXT,
    environment   TEXT NOT NULL DEFAULT 'production',
    -- Source
    source_type   TEXT NOT NULL CHECK (source_type IN ('git', 'image')),
    source_url    TEXT,                        -- git url or image ref
    source_ref    TEXT,                        -- branch, tag, sha, or image tag
    dockerfile    TEXT,                        -- path to Dockerfile
    build_context TEXT,                        -- path within repo
    -- Runtime
    runtime       TEXT NOT NULL DEFAULT 'docker',
    replicas      INTEGER NOT NULL DEFAULT 1,
    cpu_limit     TEXT,                        -- "0.5", "1.0", etc.
    mem_limit     TEXT,                        -- "256m", "1g"
    -- Health
    health_path   TEXT,
    health_timeout_ms INTEGER DEFAULT 30000,
    health_interval_ms INTEGER DEFAULT 5000,
    -- Strategy
    deploy_strategy TEXT NOT NULL DEFAULT 'recreate'
                       CHECK (deploy_strategy IN ('recreate', 'rolling', 'blue_green')),
    drain_timeout_ms INTEGER DEFAULT 30000,
    -- Metadata
    labels        TEXT NOT NULL DEFAULT '{}',  -- JSON
    annotations   TEXT NOT NULL DEFAULT '{}',  -- JSON
    -- Audit
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    created_by    TEXT,                        -- user id
    updated_by    TEXT,
    deleted_at    TEXT,                        -- soft delete
    version       INTEGER NOT NULL DEFAULT 1   -- optimistic concurrency
);

CREATE INDEX idx_apps_environment ON apps(environment) WHERE deleted_at IS NULL;
CREATE INDEX idx_apps_name ON apps(name) WHERE deleted_at IS NULL;

CREATE TABLE deployments (
    id            TEXT PRIMARY KEY,            -- ULID
    app_id        TEXT NOT NULL REFERENCES apps(id) ON DELETE RESTRICT,
    version       INTEGER NOT NULL,            -- monotonic per app
    image_ref     TEXT NOT NULL,
    image_digest  TEXT,                        -- sha256:...
    status        TEXT NOT NULL CHECK (status IN
                     ('pending', 'building', 'pushing', 'starting',
                      'healthy', 'succeeded', 'failed', 'rolled_back', 'cancelled')),
    status_reason TEXT,                        -- human-readable
    strategy      TEXT NOT NULL,
    started_at    TEXT NOT NULL,
    finished_at   TEXT,
    duration_ms   INTEGER,
    request_id    TEXT,                        -- idempotency key, if any
    triggered_by  TEXT NOT NULL,               -- 'cli', 'webhook', 'auto_rollback', 'agent'
    triggered_by_user TEXT,
    config_snapshot TEXT NOT NULL,             -- JSON of env, secrets ref, resources
    previous_deployment_id TEXT REFERENCES deployments(id),
    UNIQUE(app_id, version)
);

CREATE INDEX idx_deployments_app_started ON deployments(app_id, started_at DESC);
CREATE INDEX idx_deployments_status ON deployments(status) WHERE status IN ('pending', 'building', 'starting');

CREATE TABLE secrets (
    id            TEXT PRIMARY KEY,
    app_id        TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    key           TEXT NOT NULL,
    -- Encrypted blob: nonce || ciphertext || tag (XChaCha20-Poly1305)
    ciphertext    BLOB NOT NULL,
    key_fingerprint TEXT NOT NULL,             -- which age recipient encrypted
    version       INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    created_by    TEXT,
    rotated_from  TEXT,                        -- previous secret id
    deleted_at    TEXT,
    UNIQUE(app_id, key, version)
);

CREATE INDEX idx_secrets_app ON secrets(app_id) WHERE deleted_at IS NULL;

CREATE TABLE domains (
    id            TEXT PRIMARY KEY,
    app_id        TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    hostname      TEXT NOT NULL,
    tls_mode      TEXT NOT NULL DEFAULT 'auto'
                    CHECK (tls_mode IN ('auto', 'dns01', 'custom', 'off')),
    cert_path     TEXT,                        -- if tls_mode=custom
    redirect_to   TEXT,                        -- optional 301 target
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(hostname)
);

CREATE TABLE acme_accounts (
    id            TEXT PRIMARY KEY,
    provider      TEXT NOT NULL,               -- 'letsencrypt', 'zerossl'
    email         TEXT NOT NULL,
    -- Account key encrypted with cluster master key
    account_key   BLOB NOT NULL,
    directory_url TEXT NOT NULL,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE certs (
    id            TEXT PRIMARY KEY,
    hostname      TEXT NOT NULL,
    cert_pem      BLOB NOT NULL,
    key_encrypted BLOB NOT NULL,               -- encrypted with cluster key
    issuer        TEXT NOT NULL,
    issued_at     TEXT NOT NULL,
    expires_at    TEXT NOT NULL,
    auto_renew    INTEGER NOT NULL DEFAULT 1,
    UNIQUE(hostname)
);

CREATE INDEX idx_certs_expires ON certs(expires_at) WHERE auto_renew = 1;

CREATE TABLE backups (
    id            TEXT PRIMARY KEY,
    resource_type TEXT NOT NULL,               -- 'database', 'volume', 'platform'
    resource_id   TEXT,
    target        TEXT NOT NULL,               -- 'local:/var/backups', 's3://bucket/prefix'
    size_bytes    INTEGER,
    status        TEXT NOT NULL CHECK (status IN
                     ('pending', 'running', 'succeeded', 'failed', 'verified', 'restore_failed')),
    started_at    TEXT NOT NULL,
    finished_at   TEXT,
    sha256        TEXT,                        -- content hash
    storage_uri   TEXT,                        -- where it actually landed
    error_message TEXT,
    verified_at   TEXT,                        -- last successful verify
    retention_until TEXT                       -- auto-purge after this
);

CREATE INDEX idx_backups_resource ON backups(resource_type, resource_id, started_at DESC);
CREATE INDEX idx_backups_retention ON backups(retention_until) WHERE retention_until IS NOT NULL;

CREATE TABLE servers (
    id            TEXT PRIMARY KEY,
    hostname      TEXT NOT NULL UNIQUE,
    ssh_user      TEXT NOT NULL DEFAULT 'root',
    ssh_port      INTEGER NOT NULL DEFAULT 22,
    role          TEXT NOT NULL CHECK (role IN ('server', 'agent')),
    status        TEXT NOT NULL CHECK (status IN
                     ('pending', 'provisioning', 'ready', 'unreachable', 'decommissioned')),
    last_seen_at  TEXT,
    agent_version TEXT,
    os_release    TEXT,                        -- from /etc/os-release
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE idempotency_keys (
    key           TEXT PRIMARY KEY,
    endpoint      TEXT NOT NULL,
    body_hash     BLOB NOT NULL,
    response_status INTEGER NOT NULL,
    response_body BLOB NOT NULL,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    expires_at    TEXT NOT NULL                 -- 24h TTL
);

CREATE INDEX idx_idempotency_expires ON idempotency_keys(expires_at);

-- 0002_audit.sql
CREATE TABLE audit_events (
    id            INTEGER PRIMARY KEY,          -- monotonic, for cursor pagination
    occurred_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    actor_type    TEXT NOT NULL,                -- 'user', 'agent', 'system'
    actor_id      TEXT NOT NULL,                -- user id, agent id, 'control_plane'
    action        TEXT NOT NULL,                -- 'app.create', 'deployment.start', 'secret.rotate'
    resource_type TEXT NOT NULL,                -- 'app', 'deployment', 'secret', 'domain', 'backup'
    resource_id   TEXT,
    request_id    TEXT,                         -- correlates with API request
    source_ip     TEXT,
    user_agent    TEXT,
    payload       TEXT NOT NULL,                -- JSON: { before, after, metadata }
    result        TEXT NOT NULL CHECK (result IN ('success', 'failure', 'denied')),
    error_message TEXT
);

CREATE INDEX idx_audit_occurred ON audit_events(occurred_at DESC);
CREATE INDEX idx_audit_actor ON audit_events(actor_id, occurred_at DESC);
CREATE INDEX idx_audit_resource ON audit_events(resource_type, resource_id, occurred_at DESC);
CREATE INDEX idx_audit_action ON audit_events(action, occurred_at DESC);

-- An append-only table — enforced by trigger
CREATE TRIGGER audit_no_update BEFORE UPDATE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit_events is append-only');
END;
CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit_events is append-only');
END;
```

**Schema design notes:**

1. **ULIDs over UUIDs** for primary keys. ULIDs are 26 chars, lexicographically sortable by time, and have a built-in 80-bit random component. We do not need UUIDv7's complexity; ULID gets us 90% of the benefit with a 5 KB crate.
2. **Soft delete via `deleted_at` + partial indexes.** Hard delete is reserved for compliance-driven purge (GDPR right-to-be-forgotten on a user record, never on infrastructure records).
3. **Optimistic concurrency via `version INTEGER` on `apps`.** Every `UPDATE` does `WHERE id = ? AND version = ?` and increments. Conflict returns 409.
4. **No JSON columns for relational data** (`labels`, `annotations` are an exception — they are intentionally opaque metadata). For everything else, the schema is normalized.
5. **Append-only audit enforced by trigger.** This is the only real way to make an audit log immutable in SQLite.
6. **Foreign keys ON.** SQLite's foreign keys are off by default; we turn them on at connection time. CASCADE only on `secrets` and `domains` (they have no independent meaning); RESTRICT on `deployments` to prevent accidental cascade (deployments are historical record).

### 3.3 Migration strategy

**Recommendation: `sqlx migrate` with a `schema_version` table and mandatory review.**

- Every migration is a forward-only `.sql` file in `migrations/`. No down migrations in V1. If a migration is wrong, write a new migration that fixes it.
- Migrations run inside a `BEGIN IMMEDIATE` transaction (acquires the write lock upfront; safer than `BEGIN DEFERRED` for DDL).
- Migrations are part of the binary at compile time (`include_str!`) — no separate migration tool to install.
- A `--check-migrations` flag on the CLI aborts startup if the on-disk DB version is not in the embedded list. This is a hard fail, not a warning. It catches the "ran an old binary on a new DB" bug.
- Test migrations on a fresh DB *and* on a copy of a V1 production snapshot in CI.

### 3.4 JSON columns vs. relational

Use JSON columns for:
- `apps.labels`, `apps.annotations` (Kubernetes-style arbitrary metadata; the API does not need to query into them).
- `deployments.config_snapshot` (immutable historical record; the schema would diverge from current `apps`).
- `audit_events.payload` (open-ended; structured for human readers, not for queries).

Use relational for everything that the application filters, sorts, or joins on. Do not use SQLite's `json_extract` in WHERE clauses on hot paths — the optimizer will not use the index. For label-based filtering, generate a separate column at write time or use a generated column.

---

## 4. State Machines

### 4.1 Deployment lifecycle

```text
                  ┌──────────┐
        start ──▶ │ Pending  │
                  └────┬─────┘
                       │ acquire build slot
                       ▼
                  ┌──────────┐
                  │ Building │──── build fail ────┐
                  └────┬─────┘                    │
                       │ build ok                 │
                       ▼                          │
                  ┌──────────┐                    │
                  │ Pushing  │──── push fail ─────┤
                  └────┬─────┘                    │
                       │ push ok                  │
                       ▼                          ▼
                  ┌──────────┐               ┌──────────┐
                  │ Starting │               │ Failed   │──▶ terminal
                  └────┬─────┘               └──────────┘
                       │ health ok
                       ▼
                  ┌──────────┐
   swap route ──▶  │ Healthy  │──▶ terminal success
   stop old   ──▶  │          │    (status=Succeeded)
                  └────┬─────┘
                       │ drain done
                       ▼
                  ┌──────────┐
                  │Succeeded │──▶ terminal
                  └──────────┘

   from any non-terminal:  cancel/timeout  ──▶  Cancelled
   from Succeeded:         rollback  ──▶  new deployment in Pending (copies config)
   from Failed:            retry  ──▶  new deployment in Pending
```

**Implementation as a Rust type:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeploymentStatus {
    Pending,
    Building,
    Pushing,
    Starting,
    Healthy,
    Succeeded,
    Failed,
    RolledBack,
    Cancelled,
}

impl DeploymentStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled | Self::RolledBack)
    }

    /// Returns true if a transition is legal.
    /// This is the only place state machine rules live.
    pub fn can_transition_to(self, next: Self) -> bool {
        use DeploymentStatus::*;
        matches!(
            (self, next),
            (Pending, Building)
            | (Building, Pushing)
            | (Building, Failed)
            | (Building, Cancelled)
            | (Pushing, Starting)
            | (Pushing, Failed)
            | (Pushing, Cancelled)
            | (Starting, Healthy)
            | (Starting, Failed)
            | (Starting, Cancelled)
            | (Healthy, Succeeded)
            | (Healthy, RolledBack)        // swapped away by another deploy
            | (_, Cancelled)               // cancellation is always legal
        )
    }
}
```

**Atomic transitions** are enforced by SQLite. The use case writes:

```sql
UPDATE deployments
   SET status = ?2,
       status_reason = ?3,
       version = version + 1
 WHERE id = ?1
   AND status = ?4
   AND version = ?5;
```

If `affected_rows == 0`, the transition raced and we return 409 Conflict. The Rust code does not maintain a separate in-memory state machine; the database is the state machine.

### 4.2 Other state machines (succinctly)

**App lifecycle:** `Active → Draining (no new deploys) → Stopped (containers gone) → Active (resume) → Decommissioned (terminal, config snapshot preserved)`. Soft delete maps to `Decommissioned`.

**Secret lifecycle:** `Active → Rotating (new value deployed, old still valid for grace period) → Active (rotation complete) → Revoked (no longer in use)`. Rotation has a configurable grace window (default 24h) to allow running containers to drain.

**Backup lifecycle:** `Pending → Running → Succeeded/Failed`. Optional: `Succeeded → Verifying → Verified/VerifyFailed`. Optional: `Verified → Restoring → Restored/RestoreFailed`.

**Server lifecycle:** `Pending → Provisioning → Ready → Unreachable → (auto-recoverable) Ready / Decommissioned (terminal)`. Heartbeat timeout is configurable (default 90s).

**Agent lifecycle:** `Joining → Authenticating → Provisioned → Active → Degraded (TLS cert near expiry, disk filling) → Decommissioned`.

### 4.3 Concurrent requests and idempotent transitions

Two operators clicking "Deploy" simultaneously is the canonical race. Resolution:

1. The first to `INSERT INTO deployments` wins; the second gets a 409 Conflict ("another deployment is in progress").
2. If a deploy is `Pending` and a new request comes in for the same `app_id`, the server returns 409 with the current deployment id. The client is expected to poll `GET /v1/deployments/{id}` or `tool status`.
3. If a deploy is `Healthy` and a new request comes in, a new deployment is created. The default `blue_green` strategy makes the old one `RolledBack`; `recreate` kills it immediately.

**Recovery from partial transitions:** A startup hook scans for `deployments WHERE status IN ('pending', 'building', 'pushing', 'starting') AND started_at < now() - 1h` and marks them `Failed` with `status_reason='recovered_after_restart'`. A new deployment can then be triggered.

---

## 5. Concurrency Model

### 5.1 Runtime: tokio, single multi-threaded runtime

**Recommendation: A single `#[tokio::main]` multi-threaded runtime, with `tokio::spawn` for background work, `tokio::task::spawn_blocking` for I/O or CPU work that does not play well with async.**

Why not a thread per subsystem? Because we have 1,000s of concurrent in-flight requests at the API level, each of which spawns a small number of long-running tasks. A multi-threaded work-stealing runtime is the right primitive.

**Configuration:**

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(num_cpus::get())    // 1 worker per physical core
        .thread_name("sovereign-worker")
        .thread_stack_size(2 * 1024 * 1024)  // 2 MB default is too small
        .build()?;
    // ...
}
```

### 5.2 Per-task concurrency limits

Three limits, enforced in three places:

```rust
const MAX_CONCURRENT_DEPLOYS: usize = 4;       // parallel image builds/pulls
const MAX_CONCURRENT_LOG_STREAMS: usize = 64;  // SSE log subscribers
const MAX_CONCURRENT_API_REQUESTS: usize = 256; // axum concurrency limit
```

The `MAX_CONCURRENT_DEPLOYS` is implemented as a `tokio::sync::Semaphore` held in `AppState`. Every deploy acquires a permit before starting. This is *bulkheading*: even if deploys are slow, the API stays responsive.

### 5.3 Backpressure

The axum router has a global `tower::limit::ConcurrencyLimit` layer. When exceeded, the server returns `503 Service Unavailable` with `Retry-After: 5`. Log streams have their own buffer: a slow TUI client cannot back-pressure the source; instead, slow clients get dropped at the SSE layer and must reconnect.

### 5.4 Cancellation and timeouts

Every operation that touches a network resource gets a `tokio::time::timeout`. The cancel path is explicit:

```rust
async fn deploy_step<F, T>(label: &str, dur: Duration, fut: F) -> Result<T>
where F: Future<Output = Result<T>>,
{
    tokio::select! {
        result = tokio::time::timeout(dur, fut) => {
            match result {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(e)) => Err(e.into()),
                Err(_) => Err(Error::Timeout { step: label.into(), after: dur }),
            }
        }
        _ = shutdown_rx.recv() => {
            Err(Error::ShuttingDown)
        }
    }
}
```

The `shutdown_rx` is a `tokio::sync::watch` channel broadcast from the SIGTERM handler. Every long-running task subscribes. On SIGTERM, the process stops accepting new requests, drains in-flight ones (up to 30s), then exits. State on disk is consistent because every state change is in a SQLite transaction.

### 5.5 Memory bounds

The single biggest memory risk is unbounded log streaming. Mitigation:

- A bounded `tokio::sync::mpsc` channel of capacity 1024 between the log source (Docker) and the SSE handler.
- When the channel is full, the source task logs at `WARN` ("subscriber slow, dropping lines") and drops the line.
- The SSE handler stores the last 10,000 lines in a `VecDeque` for late subscribers; older lines are dropped. This is the "tail" buffer.

A second risk is SQLite prepared-statement caching. The `sqlx` pool caches compiled statements; we cap the pool at 32 connections (overkill for a single writer, but safe). Each connection's prepared-statement cache is bounded by SQLite itself (default `SQLITE_MAX_PREPARED` is high enough).

---

## 6. Failure Modes and Blast Radius

### 6.1 What fails and how

The control plane must be designed so that any single component failure degrades gracefully, not catastrophically.

| Failure | Detection | Response | Blast radius |
|---|---|---|---|
| SQLite I/O error (disk full) | `sqlx` error | Mark control plane unhealthy; serve `503` from `/v1/health`; alerts fire | Control plane down; running apps unaffected |
| Docker daemon crash | Heartbeat on `/v1/_ping` | Pause all deploys; existing containers keep running | Deploys paused; apps running |
| Docker daemon unrecoverable | 3 failed restart attempts | Enter "maintenance mode"; alert operator | Operator must intervene |
| Reverse proxy (Caddy) crash | systemd restarts it | Routes served from in-memory config; new deploys queued | Brief 5xx if config reload racing |
| Cert renewal failure | ACME renewal job 14d before expiry | Alert; fall back to existing cert | Cert may expire; pages start |
| Network partition between server and agent | Heartbeat timeout 90s | Mark agent `Unreachable`; stop dispatching deploys to it | That agent's apps keep running, but no new deploys |
| Audit log write fails | `sqlx` error on INSERT in same tx | Abort the action; return `500`; alert | Action refused; no silent loss |
| Clock skew | `chrono` clock drift check at startup | Warn; refuse to issue certs | Build cache TTL may be wrong |
| Out-of-disk on agent | `df` check before deploy | Refuse new deploy with clear error | That agent only |
| OOM in control plane | `cgroup` OOM kill | systemd restarts; in-memory state lost (SQLite survives) | Brief unavailability |
| Backup target unreachable | Network error | Mark backup `Failed`; alert; keep last good backup | No data loss; no recovery point |
| Restore fails partway | pg_dump exit code non-zero | Transaction-style rollback if possible; otherwise mark backup `VerifyFailed` | Manual cleanup |

### 6.2 Circuit breakers

For every external call (ACME, S3, Docker daemon, GitHub API), a simple circuit breaker:

```rust
struct CircuitBreaker {
    state: AtomicU8,           // 0=closed, 1=open, 2=half-open
    failures: AtomicU32,
    opened_at: Mutex<Option<Instant>>,
    config: CircuitConfig,
}

impl CircuitBreaker {
    async fn call<F, T>(&self, fut: F) -> Result<T>
    where F: Future<Output = Result<T>>,
    {
        if self.is_open() {
            return Err(Error::CircuitOpen);
        }
        match fut.await {
            Ok(v) => { self.on_success(); Ok(v) }
            Err(e) if e.is_transient() => {
                self.on_failure();
                Err(e)
            }
            Err(e) => Err(e),  // non-transient, don't trip the breaker
        }
        // After `open_duration`, transition to half-open: allow 1 probe request
    }
}
```

Each external call site has its own breaker. The threshold is per-breaker: 5 consecutive failures opens the circuit; 30s in `Open` allows one `HalfOpen` probe.

### 6.3 Bulkheads

Already covered in §5.2: separate semaphores per subsystem prevent one slow subsystem from starving others.

### 6.4 Timeouts at every layer

The cardinal rule: every `await` should have a timeout unless the operation is naturally bounded. Default timeouts:

- HTTP client (reqwest to GitHub, S3, etc.): connect 5s, request 30s.
- ACME: 60s.
- Docker daemon calls: 30s.
- SQLite transactions: 5s (`busy_timeout`).
- Agent HTTP heartbeat: 5s.

### 6.5 Chaos testing

A `tool chaos` subcommand that randomly kills the proxy, restarts Docker, and corrupts a backup file. Used in CI to verify recovery paths. The output is a report of what survived. Inspired by Netflix Chaos Monkey, scoped to dev/test runs.

### 6.6 Postmortem template

Every incident gets a Markdown file in `docs/postmortems/`:

```text
# YYYY-MM-DD: <one-line title>

## Impact
What users saw, for how long.

## Timeline (UTC)
- HH:MM — event
- HH:MM — detected
- HH:MM — mitigated
- HH:MM — resolved

## Root cause
One paragraph, plain language.

## What went well
- ...

## What went poorly
- ...

## Action items
- [ ] owner | description | due date
```

This is a Google SRE-style postmortem ([sre.google/sre-book/postmortem-culture](https://sre.google/sre-book/postmortem-culture/)). Blameless. Public if the project goes public.

---

## 7. Event-Driven Architecture

### 7.1 Internal event bus: in-process tokio channels + SQLite outbox

**Recommendation: Do not introduce Redis, NATS, or Kafka. Use in-process `tokio::sync::broadcast` for synchronous fan-out, and a SQLite outbox table for durable cross-process events.**

The control plane is small enough that in-process channels are correct. For example, when a deploy succeeds, the API code does:

```rust
let event = Event::DeploymentSucceeded { id, app_id, at: Utc::now() };
state.event_bus.emit(event.clone());
state.outbox.append(&event)?;  // same SQLite tx as the deployment UPDATE
```

Subscribers (TUI, audit forwarder, webhook dispatcher) read from the `event_bus`. The outbox is a backup in case the process dies between emit and subscriber delivery: a startup job replays the outbox to subscribers that missed events.

The outbox table:

```sql
CREATE TABLE event_outbox (
    id INTEGER PRIMARY KEY,
    occurred_at TEXT NOT NULL,
    topic TEXT NOT NULL,
    payload TEXT NOT NULL,
    delivered_at TEXT
);
CREATE INDEX idx_outbox_undelivered ON event_outbox(id) WHERE delivered_at IS NULL;
```

A background task with `tokio::time::interval(5s)` polls `WHERE delivered_at IS NULL LIMIT 100` and dispatches. Idempotent delivery by event id; subscribers handle duplicates.

### 7.2 Why not NATS / Redis Streams

- NATS is a separate process. Adding it doubles the ops surface.
- Redis Streams are in Redis, which is a separate process. Same problem.
- The volume of events is low (< 10/sec steady state, < 100/sec during a deploy storm). In-process channels are the right tool.
- If/when we have multi-server, rqlite's Raft log already gives us ordered, durable events. Use that. (See §19.)

### 7.3 Webhook delivery: outbox + signed payloads

Webhook delivery uses the outbox. A `WebhookEvent` row has the URL, payload, and a per-event HMAC. On delivery:

```rust
let sig = hmac_sha256(secret, body);
let resp = client.post(url)
    .header("X-Sovereign-Event-Id", event.id.to_string())
    .header("X-Sovereign-Signature", format!("sha256={}", sig))
    .body(body)
    .timeout(Duration::from_secs(10))
    .send().await;
```

Retries with exponential backoff (1s, 5s, 30s, 5m, 30m, 6h, 24h) and a max of 7 attempts. After that, the webhook is marked `dead` and a 4xx is returned on the audit log. Subscribers MUST verify the signature.

This is the Stripe webhook design ([stripe.com/docs/webhooks](https://docs.stripe.com/webhooks)) — verified as the de facto standard for outbound webhooks.

### 7.4 Agent-server protocol

V1.5 introduces agents. The protocol is HTTP/2 + mTLS over the LAN, with the same REST API as the control plane. Three special endpoints agents implement:

- `POST /agent/heartbeat` — every 10s, with resource usage
- `POST /agent/execute` — server pushes a deployment plan (build, pull, swap, health-check)
- `GET  /agent/tasks/{id}` — agent polls for pending tasks

The plan is sent as JSON; the agent must acknowledge receipt and then report progress via `PATCH /agent/tasks/{id}` with status updates. This is the Nomad server+client pattern ([nomadproject.io/docs/architecture](https://developer.hashicorp.com/nomad/docs/architecture)).

---

## 8. Multi-Tenancy

### 8.1 The decision: soft isolation with a `tenant_id` column (V2+)

**Recommendation: Single binary, single SQLite, single process. Add a `tenant_id` column to every row when team features ship. Hard isolation (separate processes/DBs) is never needed for this product.**

Why:
- The spec is explicit: target users are solo developers and small teams, not SaaS providers running on behalf of thousands of unrelated customers.
- A solo developer has one tenant. An agency with 10 client sites might have 10 tenants, but they all run on one box, administered by the same operator.
- Hard isolation would require per-tenant ports, per-tenant SQLite files, per-tenant proxies. Operational complexity explodes. The k8s-style per-namespace proxy is overkill.

The `tenant_id` is a UUID, defaulting to the install's primary tenant. V1 does not need it; V1.5 introduces it as part of team features. The `tenant_id` is added to every table in a single migration. All queries are scoped: `WHERE tenant_id = current_tenant()` is enforced in a `Before` middleware on the use-case layer.

### 8.2 Security model

- Every authenticated request has a verified `actor_id` (user id) and a `tenant_id`.
- Use cases are the only place that can do `INSERT` or `UPDATE` on domain tables. They check `tenant_id` against the request's `tenant_id` before any write.
- Cross-tenant access is impossible by construction. The audit log records every denied access attempt.

---

## 9. Configuration Management

### 9.1 Format: TOML primary, with a strict schema

**Recommendation: TOML. Not YAML. Not JSON. Not env vars.**

YAML has historically been a source of CVEs in Rust (CVE-2022-24713, CVE-2023-31655 — `serde_yaml` arbitrary code execution in some adjacent libraries). JSON does not support comments. Env vars are great for container secrets but terrible for structured config with 20 fields.

TOML is the Cargo native format; every Rust developer knows it. Use `figment` for layered loading (env, file, CLI flag) and `serde` for validation against a `Config` struct.

```rust
#[derive(Debug, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub data_dir: PathBuf,                  // default: /var/lib/sovereign
    pub bind: String,                       // default: 127.0.0.1:7878
    pub tls: TlsConfig,
    pub runtime: RuntimeConfig,
    pub proxy: ProxyConfig,
    pub acme: AcmeConfig,
    pub secrets: SecretsConfig,
    pub backups: BackupsConfig,
    pub auth: AuthConfig,
    pub observability: ObservabilityConfig,
}
```

### 9.2 Hierarchy and overrides

Resolution order, lowest priority first:
1. Compiled-in defaults.
2. `/etc/sovereign/config.toml` (system-wide).
3. `~/.config/sovereign/config.toml` (user).
4. Environment variables prefixed with `SOVEREIGN_` (e.g., `SOVEREIGN_BIND=0.0.0.0:7878`).
5. CLI flags (`--bind 0.0.0.0:7878`).

Each layer overrides the previous. The `figment` crate handles this natively. The config is loaded once at startup; hot-reload is not in V1.

### 9.3 Validation

`Config::validate()` runs at startup. On failure, print a friendly error with the failing field and exit non-zero. No silent fallback. The `deny_unknown_fields` attribute catches typos in TOML.

### 9.4 Schema generation and migration

The `Config` struct has a `schemars` derive. `tool config schema` emits JSON Schema. For config migrations, ship a `Config` version field:

```rust
#[derive(Deserialize)]
struct ConfigV1 { ... }

#[derive(Deserialize)]
struct ConfigV2 { /* new field added */ }
```

On load, deserialize as V2; missing fields get defaults. For breaking changes (renamed, removed), a one-shot migration step in `migrate_config_v1_to_v2`. Keep migration logic in-tree, not in user docs.

---

## 10. Plugin / Extension Model

### 10.1 Recommendation: NO plugin runtime. Templates and out-of-process hooks only.

**Do not embed WebAssembly. Do not embed Lua. Do not load dynamic libraries.** Every plugin system added in V1 is a permanent security and maintenance burden.

Three extension surfaces, all static and out-of-process:

1. **Templates**: `tool init fastapi` and friends. Pure data — a `template.toml` describing a starter repo, plus the files. No code execution. Users can write their own templates and pass `--template /path/to/dir`.
2. **Notification channels**: Email, Slack, Discord, Telegram, generic webhook. Adding a new channel is a new trait impl in the `sovereign-notify` crate. Users wanting a custom channel can write a tiny webhook receiver in their language of choice.
3. **Backup targets**: S3, local, SFTP. Adding a target is a new trait impl in `sovereign-backup`. The trait surface is small (5 methods).

A `tool plugin` subcommand in V2+ can list official and community templates / notify channels / backup targets by reading a TOML manifest. Discovery via a static registry URL. No code execution, ever.

---

## 11. Observability Primitives

### 11.1 Structured logging: `tracing` + JSON

**Recommendation: `tracing` for instrumentation, `tracing-subscriber` with a JSON formatter for the production output, and `tracing-bunyan-formatter` as an alternative.**

```rust
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

tracing_subscriber::registry()
    .with(EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sovereign=debug")))
    .with(tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(false))
    .init();
```

JSON logs are the 2026 standard. Every CI integration, every log aggregator, every grep alternative (jq, duckdb) speaks JSON.

**What to log at each level:**
- `ERROR`: action failed, requires intervention.
- `WARN`: recoverable problem (retry succeeded, cert expiring, disk 80% full).
- `INFO`: lifecycle events (app started, deploy succeeded, user logged in).
- `DEBUG`: per-request details, SQL queries, ACME negotiations.
- `TRACE`: payload-level details, only when `RUST_LOG=sovereign=trace` is set.

### 11.2 Metrics: `/metrics` in Prometheus format

**Recommendation: Expose Prometheus metrics at `GET /metrics`. Do not embed Prometheus or VictoriaMetrics.**

Use `metrics` + `metrics-exporter-prometheus` ([github.com/metrics-rs/metrics](https://github.com/metrics-rs/metrics)). Every subsystem emits counters and histograms:

```rust
use metrics::{counter, histogram, gauge};

counter!("sovereign.deploys.started", "app" => app_id).increment(1);
histogram!("sovereign.deploy.duration_ms", "strategy" => "blue_green")
    .record(elapsed.as_millis() as f64);
gauge!("sovereign.apps.running").set(active_count as f64);
```

Metric names follow the Prometheus convention: `<namespace>_<subsystem>_<noun>_<verb>`. Cardinality is bounded (no per-user labels).

Operators scrape with their own Prometheus/VictoriaMetrics. Document a sample `docker-compose.yml` for a one-liner observability stack but never make it the default.

### 11.3 Distributed tracing: OpenTelemetry, opt-in

**Recommendation: `tracing-opentelemetry` behind a feature flag, off by default.** When enabled, spans are exported via OTLP to the operator's collector. The Rust OTel SDK is mature and adds ~20-50 MB memory and <1ms latency per request ([docs.base14.io](https://docs.base14.io/instrument/apps/auto-instrumentation/axum/)).

### 11.4 Health endpoints

- `GET /v1/health` — liveness. Returns 200 if the process is alive. No DB check.
- `GET /v1/ready` — readiness. Returns 200 if SQLite is reachable, Docker is reachable, the proxy is reachable. Returns 503 with details otherwise.
- `GET /v1/version` — build info: git sha, build time, version, features enabled.

### 11.5 Profiling

`pprof-rs` is added as a feature flag. `GET /debug/pprof/heap?seconds=10` returns a heap profile; `GET /debug/pprof/profile?seconds=30` returns a CPU profile. Disabled in production by default; enabled per-request with an auth check (operator-only).

`cargo-flamegraph` is used in CI for benchmarks. Continuous benchmarking via `criterion` + GitHub Actions on PR diffs.

---

## 12. Crate Selection

Every choice below is justified against the four criteria the report requires: maintenance status, performance, ecosystem, license.

### 12.1 Web framework: **axum 0.8**

Actix-web is 10-15% faster under saturation but has an actor model that is a real cost to team onboarding ([medium.com/@abhinav.dobhal/actix-web-vs-axum-in-2026](https://medium.com/@abhinav.dobhal/actix-web-vs-axum-in-2026-stop-asking-which-is-faster-and-start-asking-which-wont-wreck-your-team-5c19b07c2a32)). For a control plane that is not in the 5% of services that need absolute throughput, axum is the right call. The Tokio team maintains it; it composes with `tower`; the ecosystem is broad. **License: MIT.**

### 12.2 Async runtime: **tokio 1.x**

The default. Every async crate we use depends on tokio. Async-std is dormant. Smol is for embedded. **License: MIT.**

### 12.3 Database driver: **sqlx 0.8** (not rusqlite, not diesel)

Sqlx gives us:
- Compile-time checked queries (the `query!` macro).
- Native async, no `spawn_blocking` tax.
- Migrations built in.
- Connection pooling.
- Supports both SQLite and (in V2) the same queries against rqlite's HTTP API.

Rusqlite is sync and would force every query to go through `spawn_blocking`. Diesel is heavy and has its own DSL, which we don't want — we want SQL. **License: MIT/Apache-2.0.**

### 12.4 Serialization: **serde 1.x** + **simd-json 0.13** (optional)

Serde is the universal standard. Simd-json is 2-4× faster for parsing large payloads (mostly relevant for the `/v1/deployments/{id}/logs` endpoint). Use simd-json as an opt-in feature flag for the hot path; serde for everything else. **License: MIT/Apache-2.0.**

### 12.5 Logging: **tracing 0.1** + **tracing-subscriber 0.3**

The Tokio ecosystem standard. The `tracing` crate gives us structured fields and spans; `tracing-subscriber` formats them. We considered slog and concluded: slog is for sync code, and its ecosystem has not kept up. **License: MIT.**

### 12.6 CLI: **clap 4.x** (with derive)

Clap is the de facto standard. The derive macro is excellent. `argh` is leaner but has worse error messages and fewer features. `lexopt` is too low-level. **License: MIT/Apache-2.0.**

### 12.7 Config: **figment 0.10** (not config-rs)

Figment supports the layered loading we need (defaults → file → env → CLI) cleanly. The `config` crate is fine but its API is more verbose. **License: MIT/Apache-2.0.**

### 12.8 Middleware: **tower** + **tower-http**

Axum is built on tower. `tower-http` provides the `TraceLayer`, `CompressionLayer`, `RequestBodyLimitLayer`, `SetResponseHeaderLayer`, etc. that we use for cross-cutting concerns. **License: MIT.**

### 12.9 TUI: **ratatui 0.29** + **crossterm 0.28**

Ratatui is the maintained successor to tui-rs. Crossterm is the cross-platform terminal library. `termion` is unmaintained. **License: MIT.**

### 12.10 Secrets encryption: **age 0.11** + **argon2 0.5**

Age is modern, simple, and audited. The `age` crate is pure Rust. Argon2id for password-derived keys. We do not use `orion` (less audited, less active). **License: MIT (age), MIT/Apache-2.0 (argon2).**

### 12.11 HTTP client: **reqwest 0.12** (with rustls)

Reqwest is the standard. We enable `rustls-tls` (not `native-tls`) for static linking. `ureq` is a sync alternative we might use for one-off scripts but not in the main binary. **License: MIT/Apache-2.0.**

### 12.12 Docker: **bollard 0.18**

The most actively maintained async Docker API client for Rust. Not as feature-complete as `docker-rs` was, but `docker-rs` is unmaintained. We wrap bollard in a `Runtime` port so we can swap to `podman` API later. **License: MIT.**

### 12.13 Git: **gix 0.70+** (gitoxide)

`gix` is the pure-Rust reimplementation of git. Faster than `git2` (libgit2) on cold clones, smaller binary, no C dependency. The `git2` crate still has bugs around partial clones. **License: MIT/Apache-2.0.**

### 12.14 UUIDs / IDs: **ulid 1.x**

ULIDs are 26-char, lex-sortable, 80-bit random, no proprietary UUID version. Used for all primary keys. **License: MIT/Apache-2.0.**

### 12.15 ACME: **instant-acme 0.7**

The Rust ACME client library used by Caddy-adjacent projects. Supports Let's Encrypt and ZeroSSL. The simpler alternative is to shell out to `certbot` — rejected because it adds an external dependency. **License: MIT/Apache-2.0.**

### 12.16 Crypto / TLS: **rustls 0.23**

For the control plane's own HTTPS termination. Not OpenSSL. Static-linked. **License: Apache-2.0/ISC.**

### 12.17 Errors: **thiserror 2.x** (library) + **anyhow 1.x** (binary)

`thiserror` for the domain and adapter error enums (typed, structured, `?`-friendly). `anyhow` for the binary glue code where we just want to bubble up. **License: MIT/Apache-2.0.**

---

## 13. Build & Release Engineering

### 13.1 CI: GitHub Actions, three jobs, full matrix

```yaml
# .github/workflows/ci.yml
on: [push, pull_request]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo deny check

  test:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
        rust: [stable, beta]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@${{ matrix.rust }}
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace --all-features
      - run: cargo test --doc --workspace

  build:
    needs: [check, test]
    runs-on: ubuntu-latest
    strategy:
      matrix:
        target:
          - x86_64-unknown-linux-musl
          - aarch64-unknown-linux-musl
          - x86_64-unknown-linux-gnu
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo build --release --target ${{ matrix.target }} --locked
      - uses: actions/upload-artifact@v4
        with:
          name: sovereign-${{ matrix.target }}
          path: target/${{ matrix.target }}/release/sovereign
```

### 13.2 Reproducible builds

Set in `[profile.release]`:

```toml
[profile.release]
opt-level = "z"            # size; switch to 3 if you need throughput
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "symbols"
incremental = false
```

Set `SOURCE_DATE_EPOCH` from the latest git commit timestamp. Build with `cargo build --locked` (pin lockfile). Document the procedure in `BUILD.md`. Verify reproducibility by diffing the output of two builds.

Target `x86_64-unknown-linux-musl` with the `tikv/jemallocator` or `mimalloc` allocator (the musl default allocator is poor under contention, per [raniz.blog/2025-02-06_rust-musl-malloc](https://raniz.blog/2025-02-06_rust-musl-malloc/)). Configure via:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
tikv-jemallocator = "0.6"
```

```rust
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
```

### 13.3 SBOM and supply chain

- `cargo-cyclonedx` generates a CycloneDX SBOM at build time.
- `cargo-audit` runs on every PR; CI fails on a known vulnerability with no fix available.
- `cargo-deny` enforces: (a) only allowlisted licenses (MIT, Apache-2.0, BSD-2/3, ISC, MPL-2.0, Zlib); (b) no yanked crates; (c) no duplicate versions; (d) no `git` sources except in `[patch]` sections we own.
- Release artifacts are signed with `cosign` (Sigstore). The signature is published alongside the binary in the GitHub release and on a public Rekor transparency log.

### 13.4 Cross-compilation

For `aarch64-unknown-linux-musl` from an `x86_64` host, use the `cross` tool. Pure-Rust crates cross-compile cleanly. C dependencies (e.g., `ring`) need a cross Docker image; `cross` ships one.

### 13.5 Dependency pinning

- `Cargo.lock` is committed. PRs that change the lockfile get a CI comment listing every version change with a link to the changelog.
- Renovate bot opens weekly PRs for minor/patch updates. Major updates are manual.
- Every dependency's MSRV (Minimum Supported Rust Version) is recorded in the README. MSRV policy: latest stable minus 6 versions, e.g., if current is 1.85, MSRV is 1.79.

### 13.6 Release process

`cargo release` (or `release-plz`) automates versioning, changelog generation, tag creation, and binary publishing. The release job in CI builds 4 targets, signs each, generates SBOMs, attaches to a GitHub release. The `main` branch is always releasable; the release tag is a pointer.

---

## 14. API Stability & Deprecation

### 14.1 Semver policy

Strict semver. The binary's version is the public contract.

- **Major bump (X.0.0)**: any breaking change to the public API (HTTP routes, CLI subcommands, file formats, state on disk that other processes read).
- **Minor bump (0.X.0)**: new features, new endpoints, new CLI subcommands, new config keys, new SQLite migrations (forward-compatible only).
- **Patch bump (0.0.X)**: bug fixes, performance improvements, dependency bumps.

### 14.2 Deprecation markers

When a feature is deprecated, three things happen simultaneously:
1. The CLI prints `WARNING: 'tool foo' is deprecated and will be removed in v2.0. Use 'tool bar' instead.` on every invocation.
2. The HTTP response includes `Deprecation: true` and `Sunset: <RFC 1123 date>` headers.
3. The `tool doctor` subcommand lists all deprecations in use on this install.

The Sunset date is set to the minor version after the deprecation announcement + 6 months, whichever is longer. This matches the Kubernetes beta-deprecation timeline ([kubernetes.io/docs/reference/using-api/api-concepts](https://kubernetes.io/docs/reference/using-api/api-concepts/)).

### 14.3 Compatibility shims

When a CLI subcommand or config key is renamed, the old name keeps working until the Sunset date, with a deprecation warning. Internal shims translate the old to the new. The shim code lives in `compatibility.rs` modules and is removed as a single PR after the Sunset date passes.

### 14.4 Migration guides

Every minor version ships a `docs/migrations/v0.X-to-v0.Y.md` if there are user-visible changes. The format is a list of one-paragraph recipes: "If you used `tool deploy api --no-cache`, use `tool deploy api --no-build-cache`." This is the Docker / Kubernetes playbook ([docs.docker.com](https://docs.docker.com/), [kubernetes.io/docs](https://kubernetes.io/docs/)).

---

## 15. Code Organization

### 15.1 Module structure principles

The workspace layout in §1.3 is the right one. Within each crate:

- `lib.rs` re-exports a small, intentional public API. The rest of the crate is private.
- Each module has a `mod.rs` (or `module.rs` in edition 2021) with the module-level doc comment, then `pub use` re-exports for the public surface, then the implementation.
- Tests live in three places: unit tests in the module, integration tests in `tests/`, doc tests in the module. The three together give 95%+ coverage of pure code.
- Examples live in `examples/`. Every public API has at least one example.

### 15.2 Library + binary split

The library crate (`sovereign-core`) is the public API for embedding. The binary crate (`sovereign`) is the CLI. Other Rust projects can `use sovereign_core::*` to build their own tooling on top. This is the `ripgrep` / `fd` / `bat` pattern: a powerful library with a thin CLI on top.

### 15.3 Doc tests

Every public function in the domain layer has a doc test. The test is the example. `cargo test --doc` runs them. This is the standard practice in `serde`, `tokio`, `axum`, `ratatui`.

### 15.4 Reference codebases to study

For navigation and readability, study:
- **[tikv](https://github.com/tikv/tikv)** — workspace layout, crate-per-subsystem, well-named modules. Heavy inspiration.
- **[deno](https://github.com/denoland/deno)** — single-binary with optional features, opinionated crate layout.
- **[rust-lang/rust](https://github.com/rust-lang/rust)** — the gold standard for doc comments and module organization.
- **[fd](https://github.com/sharkdp/fd)** — small, sharp, easy-to-navigate CLI in Rust.
- **[ripgrep](https://github.com/BurntSushi/ripgrep)** — feature-gated compilation, well-organized features.
- **[tokio](https://github.com/tokio-rs/tokio)** — workspace structure, doc organization.
- **[axum](https://github.com/tokio-rs/axum)** — extractor pattern, middleware composition, error handling.

---

## 16. Performance Engineering

### 16.1 Latency targets

| Endpoint | p50 | p95 | p99 |
|---|---|---|---|
| `GET /v1/apps` (10 apps) | 5 ms | 15 ms | 30 ms |
| `GET /v1/deployments/{id}` | 3 ms | 10 ms | 20 ms |
| `POST /v1/apps/{id}/deployments` (queue only) | 20 ms | 50 ms | 100 ms |
| `tool logs` (SSE) first byte | 50 ms | 150 ms | 300 ms |
| `tool status` (no DB) | 2 ms | 5 ms | 10 ms |

These are the targets. They are measured in CI via `criterion` benchmarks against `axum::Router` synthetic loads and recorded as a trend graph on each PR.

### 16.2 Throughput targets

The control plane API can sustain 5,000 requests/sec on a 2-core VPS for cacheable reads (status, list), 500 req/sec for writes. Beyond that, the bottleneck is SQLite single-writer; we move to rqlite at V2.

### 16.3 Memory budgets

- Idle control plane (no apps): 30 MB RSS.
- Idle with 50 apps, no active deploys: 80 MB.
- Active deploy (build + push): 200 MB transient.
- Hard ceiling: 512 MB; the process exits non-zero and systemd restarts it if RSS exceeds 90% of `cgroup` limit.

### 16.4 Allocations and hot paths

The hottest path is `GET /v1/deployments/{id}`. Optimize with:
- `#[inline]` on small accessors.
- Pre-allocated `String` capacity in response builders.
- `serde_json::to_vec` instead of `to_string` to avoid intermediate `String` allocations.
- `parking_lot::RwLock` instead of `std::sync::RwLock` (faster, smaller).

The second hottest is log streaming. Use a `Bytes` buffer reused across writes (`bytes::BytesMut` with `clear()` and `extend_from_slice`).

### 16.5 Benchmarking

- `criterion` for micro-benchmarks. Run on every PR; fail the build if a benchmark regresses by > 5%.
- `divan` is an alternative; it is faster to compile and produces nicer reports. Use for hot-path benchmarks.
- A nightly `bench-runner` GitHub Action runs the full suite, posts results to a `gh-pages` branch, and alerts on regression.

### 16.6 Profiling

- `cargo-flamegraph` for CPU hotspots (Linux only).
- `pprof-rs` for in-process heap and CPU profiling via the `/debug/pprof/*` endpoints.
- `heaptrack` for leak hunting.
- `perf` (Linux) for call-graph sampling in production-like environments.

Profile-driven development: when adding a new feature, profile before merging. If the new code adds > 5% to a hot path's latency, optimize.

---

## 17. Security Architecture

### 17.1 Authentication: API tokens + mTLS for agents + OIDC (V2+)

**V1 auth model:** API tokens. Each user gets a token (generated at first install). Tokens are stored hashed (Argon2id) in the `users` table. The control plane API requires `Authorization: Bearer <token>` on every non-`/health` endpoint. The token is 256 bits of randomness, base64url-encoded, prefixed `sov_pat_` for visibility.

**V1.5 agent auth:** Mutual TLS. Agents have a client cert signed by the server's CA. The control plane rejects any non-mTLS connection on the agent port (default `:7879`).

**V2+ OIDC:** OIDC consumer only. We accept tokens from any OIDC-compliant IdP (Authentik, Keycloak, Google, GitHub). We do not become an IdP. Library: `openidconnect` crate.

```rust
async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let token = req.headers().get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AuthError::Missing)?;
    let claims = state.token_store.verify(token).await?;
    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}
```

### 17.2 Authorization: simple RBAC, no custom policy language

**Recommendation: Three roles, hard-coded. No OPA. No custom DSL. No ReBAC.**

| Role | Can do |
|---|---|
| `owner` | Everything, including user management and destructive ops |
| `developer` | Create/deploy apps, manage secrets, view audit |
| `viewer` | Read-only on apps, deployments, logs, audit |

The use case checks the role. Example:

```rust
impl DeployApp {
    pub async fn call(&self, ctx: &AuthContext) -> Result<Deployment> {
        require_role(ctx, Role::Developer)?;
        // ... use case body
    }
}
```

That's it. If the use case complexity grows, we add a fourth role. We do not add a permission matrix. We do not add OPA. The "policy engine" is the if-statement above.

### 17.3 Secret management

The control plane's master key is generated on first run as a 256-bit X25519 key (`age`). Stored at `/var/lib/sovereign/master.key` with mode 0600, owned by the service user. The key is **never** logged, **never** displayed, and **never** transmitted. The key can be wrapped in `systemd-creds` (Linux) for hardware-bound protection.

User secrets (env vars per app) are encrypted with the master key using XChaCha20-Poly1305. Decryption happens at deploy time, in memory, and the plaintext is passed to the container via `--env-file` (Docker) which is a `tmpfs` mount. The plaintext is never written to disk.

The CLI never prints secret values. The TUI masks them. Audit logs record the key name and the actor, not the value.

### 17.4 Supply-chain security

- `cargo-deny` rejects crates with copyleft licenses beyond MPL-2.0.
- `cargo-audit` runs on every PR. CI fails on advisories with no patched version.
- SBOM generated per release; cosign-signed.
- `cargo-vet` introduced in V2 to track which crates we have personally audited.

### 17.5 Threat model (STRIDE applied)

| Threat | Mitigation |
|---|---|
| **S**poofing (forged API requests) | API tokens + Argon2id hashing; mTLS for agents |
| **T**ampering (DB on disk modified) | SQLite WAL checksums; tamper-evident audit log (append-only trigger); recommended `dm-verity` on the data partition |
| **R**epudiation (denied actions) | Append-only audit log with actor + request_id + payload |
| **I**nformation disclosure (secrets leaked) | Master key at rest, encrypted secrets, no plaintext on disk, no `cat` in CLI |
| **D**enial of service (resource exhaustion) | Per-endpoint rate limits (`tower-governor`); semaphore per subsystem; SQLite busy_timeout; max 1024 concurrent SSE subscribers |
| **E**levation of privilege (RCE) | `deny_unknown_fields` on config; no plugin runtime; least-privilege systemd unit (`NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome=true`); secrets only readable by the service user |

### 17.6 CWE top 25 coverage

The Rust type system eliminates most memory-safety CWEs by construction. The OWASP Top 10 for APIs is the practical checklist:

- **API1:2023 Broken Object-Level Authorization** — every query is `WHERE tenant_id = ? AND id = ?`.
- **API3:2023 Broken Object Property-Level Authorization** — sparse fieldsets let the client choose; the server filters out fields the actor's role cannot see.
- **API5:2023 Broken Function-Level Authorization** — use-case-level `require_role()`.
- **API8:2023 Security Misconfiguration** — `deny_unknown_fields`, secure defaults, `--check-migrations` on startup.

---

## 18. Compliance & Governance

### 18.1 Policy engine: NOT YAGNI, but a real audit log

**Recommendation: The audit log is the compliance story. Do not build a policy engine in V1.**

The spec is correct: most governance features are compliance theatre. The audit log is the one that matters because:
- It is the "who did what when" answer that every EU public-sector RFP requires.
- It is the source of truth for incident postmortems.
- It enables `BSI C5` control AC-04 (Audit Review) and EUCS Substantial control SOP-04 (Operations Monitoring).

The audit log is the policy engine, in the same way that git history is a policy engine for solo developers. Don't build OPA; build good audit.

### 18.2 Approval flows (V2)

A simple, single-approver gate. The policy:

```toml
[policy.production]
require_approval = true
approvers = ["alice", "bob"]
```

When a deploy to `environment=production` starts, a `pending_approval` row is created instead. A user with role `developer` cannot approve their own deploy. An `admin` or `owner` approves via `tool approve <deployment_id>`. The approver is recorded in the audit log.

We do not build 2-of-N approval in V1.5; defer to V3 if customer demand.

### 18.3 Anomaly detection

**Recommendation: Do not build ML-based anomaly detection.** It is theatre at our scale.

What we *do* build:
- **Rate-based alerts:** "More than 5 failed login attempts in 5 minutes from the same IP" — basic, no ML, real.
- **Sequence-based alerts:** "Secret rotation followed by immediate deletion" — basic, real.
- **Drift detection:** "The expected deployment count for app X is 3, but we observe 1" — basic, real.

If the user wants anomaly detection, they ship their events to Datadog or Elastic and use *their* ML. We expose structured logs and metrics; we do not embed an ML model.

### 18.4 Strict governance that is actually usable

The trap: making governance so heavy that operators route around it. The discipline:

- **Audit must be free.** A failed deploy to staging that nobody audits is a free learning opportunity. Audit is for production-sensitive operations only.
- **Approval must be one command.** `tool approve <id>`. Not a web form, not a Slack bot, not a Jira ticket.
- **Rollback must always be possible.** If a deploy fails, the next deploy is a rollback. The operator never has to argue with the policy engine to recover.

---

## 19. Multi-Region & Disaster Recovery Architecture

### 19.1 The path: V1 single → V1.5 agent → V2 rqlite HA → V3 stretch

| Phase | Topology | State location | Failure tolerance |
|---|---|---|---|
| V1 | 1 server, 1 binary | Local SQLite | 0 — single point of failure |
| V1.5 | 1 server + N agents | Server's SQLite, agents stateless | N−1 of agents; 0 for server |
| V2 | 3 server nodes + M agents | rqlite cluster (SQLite + Raft) | 1 of 3 servers; N−1 of agents |
| V3 | 3 servers in 2 regions | rqlite stretch cluster; CRDTs for audit | 1 region offline |

### 19.2 Why rqlite, not FoundationDB, not CockroachDB, not etcd+Postgres

- **FoundationDB** is a real distributed KV, but the order-of-magnitude operational complexity (6+ processes, 4 GB minimum) is wrong for a 1–20 server install. rqlite is 1 binary.
- **CockroachDB / YugabyteDB** are full SQL; we do not need full SQL, we need a single SQLite file replicated.
- **etcd + Postgres** is two systems; we want one.
- **rqlite** is one binary, speaks SQL (we keep our queries), uses Raft (proven consensus), has 10 years of production hardening ([philipotoole.com/rqlite-turns-10](https://philipotoole.com/rqlite-turns-10-lessons-from-a-decade-of-building-distributed-systems/)).

From the rqlite design page ([rqlite.io/docs/design](https://rqlite.io/docs/design/)): "rqlite prioritizes data consistency and high availability over write throughput — every write goes through the Raft log." This is exactly the trade-off a control plane wants.

### 19.3 V2 transition

The V1 binary stores in SQLite via `sovereign-storage-sqlite`. The V2 binary stores in rqlite via `sovereign-storage-rqlite`. The two crates implement the same `Storage` port trait:

```rust
#[async_trait]
pub trait Storage: Send + Sync {
    async fn apps(&self) -> Result<Vec<App>>;
    async fn app_create(&self, app: NewApp) -> Result<App>;
    async fn app_get(&self, id: AppId) -> Result<Option<App>>;
    async fn app_update(&self, id: AppId, patch: AppPatch) -> Result<App>;
    // ... etc
}
```

The V1 single-binary swaps the SQLite adapter for the rqlite adapter via a feature flag. No use-case code changes. The HTTP, CLI, and TUI do not know which adapter is in use. This is the test of a good port: the implementation is swappable.

### 19.4 Disaster recovery

`tool recover` is the most important command in the V2 sovereignty story. It:

1. Reads the most recent verified backup from S3.
2. Spins up a scratch container with the same DB image.
3. Restores the SQLite file.
4. Runs `PRAGMA integrity_check` and a row-count sanity check.
5. Reports a summary; the operator confirms.
6. Atomically swaps the restored DB into place.

This is end-to-end tested in CI. The test: take a snapshot of a 50-app install, kill the install, run `tool recover --from-backup-id <id>` on a fresh box, assert that all 50 apps are present and healthy.

The "vendor disappear" test: take a Hetzner VPS, install only the binary, no other services, point it at the backup, and run `tool recover`. If it works, the sovereignty promise is real.

### 19.5 When NOT to scale further

The hard ceiling is operator cognitive load, not the binary. At 100 servers:
- The TUI fleet view (k9s-style) is the primary interface.
- Bulk operations become first-class (`tool deploy api --all-servers`).
- Per-server health matrices replace single-server health checks.
- Aggregated logs across servers are the norm.

All achievable in V2 architecture. The V3 multi-region is only justified if customers are paying for it.

---

## 20. Decision Records (ADRs)

The architecture is captured in 12 ADRs. Each is a 1-page Markdown file in `docs/adr/`.

```text
ADR-0001: Hexagonal architecture with Rust traits
ADR-0002: SQLite for V1, rqlite for V2, no Postgres
ADR-0003: axum 0.8 as the web framework
ADR-0004: REST + JSON over HTTP, no gRPC
ADR-0005: URL-prefix versioning, Kubernetes-style
ADR-0006: Cursor pagination, never offset
ADR-0007: Stripe-style idempotency keys
ADR-0008: age + Argon2id for secret encryption
ADR-0009: tracing + JSON logs, no slog
ADR-0010: Agent pattern for multi-server (V1.5)
ADR-0011: rqlite for HA control plane (V2)
ADR-0012: No plugin runtime, templates and out-of-process hooks only
```

The format follows the [adr.github.io](https://adr.github.io/) template: Context, Decision, Consequences. Each ADR is updated when a decision is reversed.

---

## 21. Top 5 Architectural Recommendations

1. **Hexagonal architecture in Rust, with `Arc<dyn Port>` DI at the composition root.** The cleanest way to keep the control plane testable and the storage/runtime/proxy adapters swappable from V1 through V3. Domain is pure Rust, no `sqlx` or `reqwest`. Use cases own transaction boundaries. Adapters are the only place that touches the outside world.
2. **REST + JSON, URL-prefix versioning, RFC 9457 errors, Stripe-style idempotency keys, cursor pagination.** Adopted wholesale from the Kubernetes / Stripe / GitHub playbook. No gRPC, no GraphQL. The `/v1/` prefix is in the binary on day 1; we never have to break a deployed user.
3. **SQLite (WAL) in V1, rqlite in V2, never Postgres.** The same `Storage` trait port, two implementations, zero use-case-code change. Optimistic concurrency via a `version` column on `apps`. Append-only audit enforced by SQLite trigger. Migrations as forward-only `.sql` files compiled into the binary.
4. **Agent pattern in V1.5, rqlite HA in V2.** The single most important architectural decision. The control plane is always a small Raft cluster of 3 nodes; agents are stateless and pull work. Every state change is in a Raft-replicated transaction. Build the agent path in V1.5; do not try to scale single-server past 10 servers.
5. **`age` + Argon2id for secrets, zero-disk injection into containers, append-only audit log as the compliance story.** No Vault, no Infisical server, no plugin runtime. The `master.key` lives at `/var/lib/sovereign/master.key` (0600), optionally wrapped by `systemd-creds`. The audit log table is the policy engine.

---

## 22. References

All claims trace to one of the following primary sources:

- **rqlite design and 10-year retrospective:** [rqlite.io/docs/design](https://rqlite.io/docs/design/), [philipotoole.com/rqlite-turns-10](https://philipotoole.com/rqlite-turns-10-lessons-from-a-decade-of-building-distributed-systems/)
- **SQLite WAL and checkpoint internals:** [sqlite.org/wal.html](https://sqlite.org/wal.html)
- **Kubernetes API conventions and versioning:** [kubernetes.io/docs/reference/using-api/api-concepts](https://kubernetes.io/docs/reference/using-api/api-concepts/), [github.com/kubernetes/community/blob/master/contributors/devel/sig-architecture/api-conventions.md](https://github.com/kubernetes/community/blob/master/contributors/devel/sig-architecture/api-conventions.md)
- **Kubernetes release versioning:** [github.com/kubernetes/sig-release/blob/master/release-engineering/versioning.md](https://github.com/kubernetes/sig-release/blob/master/release-engineering/versioning.md)
- **Stripe idempotency:** [stripe.com/blog/idempotency](https://stripe.com/blog/idempotency)
- **Stripe webhooks:** [docs.stripe.com/webhooks](https://docs.stripe.com/webhooks)
- **RFC 9457 Problem Details:** [datatracker.ietf.org/doc/rfc9457](https://datatracker.ietf.org/doc/rfc9457/)
- **Hexagonal architecture in Rust:** [howtocodeit.com/guides/master-hexagonal-architecture-in-rust](https://www.howtocodeit.com/guides/master-hexagonal-architecture-in-rust), [github.com/howtocodeit/hexarch](https://github.com/howtocodeit/hexarch), [dev.to/guuri11/an-opinionated-clean-architecture-in-rust-4jn8](https://dev.to/guuri11/an-opinionated-clean-architecture-in-rust-4jn8)
- **axum 0.8 vs actix-web 4.13:** [rustify.rs/articles/rust-axum-vs-actix-web-2026](https://rustify.rs/articles/rust-axum-vs-actix-web-2026), [medium.com/@abhinav.dobhal/actix-web-vs-axum-in-2026](https://medium.com/@abhinav.dobhal/actix-web-vs-axum-in-2026-stop-asking-which-is-faster-and-start-asking-which-wont-wreck-your-team-5c19b07c2a32)
- **tracing + axum structured logging:** [techbuddies.io/2026/04/04/how-to-add-structured-logging-to-rust-http-apis-with-axum-middleware](https://www.techbuddies.io/2026/04/04/how-to-add-structured-logging-to-rust-http-apis-with-axum-middleware), [ianbull.com/posts/axum-rust-tracing](https://ianbull.com/posts/axum-rust-tracing)
- **Rust binary size optimization:** [atharvapandey.com/post/rust/rust-deploy-release-profiles](https://www.atharvapandey.com/post/rust/rust-deploy-release-profiles/)
- **Rust musl malloc:** [raniz.blog/2025-02-06_rust-musl-malloc](https://raniz.blog/2025-02-06_rust-musl-malloc/)
- **SQLite production (Forward Email, Rails 8):** [mvpfactory.io/blog/sqlite-as-your-server-database-wal-mode-pragma-tuning-and-why-litestream](https://mvpfactory.io/blog/sqlite-as-your-server-database-wal-mode-pragma-tuning-and-why-litestream/), [erikminkel.com/2025/12/31/production-sqlite-powered-by-litestream-with-rails-8](https://www.erikminkel.com/2025/12/31/production-sqlite-powered-by-litestream-with-rails-8/)
- **Caddy admin API and memory:** [caddyserver.com/docs/api](https://caddyserver.com/docs/api), [falcao.org/posts/caddy-nginx-traefik-2026](https://falcao.org/posts/caddy-nginx-traefik-2026/)
- **Podman REST API compatibility:** [man.archlinux.org/man/podman-system-service.1.en.txt](https://man.archlinux.org/man/podman-system-service.1.en.txt)
- **ratatui ecosystem:** [ratatui.rs](https://ratatui.rs/), [github.com/ratatui/awesome-ratatui](https://github.com/ratatui/awesome-ratatui)
- **OpenTelemetry in Rust:** [docs.base14.io/instrument/apps/auto-instrumentation/axum](https://docs.base14.io/instrument/apps/auto-instrumentation/axum/)
- **SRE postmortem culture:** [sre.google/sre-book/postmortem-culture](https://sre.google/sre-book/postmortem-culture/)
- **OWASP API Security Top 10 2023:** [owasp.org/API-Security/editions/2023](https://owasp.org/API-Security/editions/2023/)
- **Nomad architecture (agent pattern):** [developer.hashicorp.com/nomad/docs/architecture](https://developer.hashicorp.com/nomad/docs/architecture)
- **Web framework benchmark data:** [theeditorial.news/frameworks/actix-web-vs-axum-vs-rocket-vs-poem-which-rust-backend-framework-ships-faster-mpkt7458](https://theeditorial.news/frameworks/actix-web-vs-axum-vs-rocket-vs-poem-which-rust-backend-framework-ships-faster-mpkt7458), [markaicode.com/vs/rust-web-frameworks-in-2025-axum-vs-actix-vs-rocket-performance-benchmark](https://markaicode.com/vs/rust-web-frameworks-in-2025-axum-vs-actix-vs-rocket-performance-benchmark/)
- **API design and pagination:** [youngju.dev/blog/culture/2026-04-15-api-design-complete-guide-rest-openapi-versioning-pagination-idempotency-webhooks-deep-dive-guide-2025](https://www.youngju.dev/blog/culture/2026-04-15-api-design-complete-guide-rest-openapi-versioning-pagination-idempotency-webhooks-deep-dive-guide-2025.en)
- **Internal Research:** `C:\Users\Victo\Downloads\webproj\Cloud\Research-1.txt`, `Research-2.md`, `Research-Report-1.md`, `competitive-landscape.md`

---

**End of report.** This document is the architectural input for the next phase of implementation. Every decision above is opinionated. Every opinion is justified. Where the spec is silent, this document speaks.
