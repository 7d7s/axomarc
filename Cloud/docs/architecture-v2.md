# Sovereign Architecture

**Single-binary, hexagonal, CLI-first deployment runtime.**

---

## Table of Contents

- [Design Principles](#design-principles)
- [Hexagonal Layout](#hexagonal-layout)
- [Crate Dependency Graph](#crate-dependency-graph)
- [Data Model](#data-model)
- [State Machines](#state-machines)
- [API Conventions](#api-conventions)
- [Failure Modes](#failure-modes)
- [Security Model](#security-model)
- [Performance Characteristics](#performance-characteristics)

---

## Design Principles

1. **One binary, one file, one CLI** — no daemons, no sidecars, no config files required
2. **Hexagonal (ports & adapters)** — domain logic is framework-free, adapters are swappable
3. **Append-only audit** — every write is an audit event; UPDATE/DELETE rejected by trigger
4. **Optimistic concurrency** — version column on every mutable row
5. **Zero-disk secrets** — decrypted values exist only in container memory
6. **Fail-safe defaults** — doctor checks are warnings, not errors; deploy continues on proxy failure

---

## Hexagonal Layout

```
┌─────────────────────────────────────────────────────────────┐
│                         CLI Layer                            │
│  ┌───────────┐  ┌────────────┐  ┌──────────┐  ┌──────────┐ │
│  │  deploy    │  │  rollback  │  │  secret  │  │  status  │ │
│  │  logs      │  │  backup    │  │  domain  │  │  doctor  │ │
│  └─────┬─────┘  └─────┬──────┘  └────┬─────┘  └────┬─────┘ │
│        │              │              │              │        │
│  ┌─────▼──────────────▼──────────────▼──────────────▼─────┐ │
│  │                    sovereign-core                       │ │
│  │  ┌─────────────┐  ┌──────────────┐  ┌──────────────┐  │ │
│  │  │ use_cases/  │  │  domain/     │  │    rbac/     │  │ │
│  │  │  deploy.rs  │  │   app.rs     │  │  actor.rs    │  │ │
│  │  │  rollback.rs│  │   deploy.rs  │  │  action.rs   │  │ │
│  │  │  secret.rs  │  │   user.rs    │  │  check.rs    │  │ │
│  │  │  backup.rs  │  │   audit.rs   │  │              │  │ │
│  │  └─────────────┘  └──────────────┘  └──────────────┘  │ │
│  │  ┌──────────────────────────────────────────────────┐  │ │
│  │  │                    ports/                        │  │ │
│  │  │  StoragePort    RuntimePort    ProxyPort         │  │ │
│  │  │  SecretsPort    BackupPort     HealthResult      │  │ │
│  │  │  SystemdNativePort                              │  │ │
│  │  └──────────────────────────────────────────────────┘  │ │
│  └────────────────────────────────────────────────────────┘ │
│                           │                                  │
│  ┌────────────────────────▼───────────────────────────────┐ │
│  │                    Adapters                             │ │
│  │  ┌─────────────────┐  ┌─────────────────────────────┐  │ │
│  │  │ storage-sqlite  │  │ runtime-docker (bollard)    │  │ │
│  │  │  7 core tables  │  │  pull/create/start/stop     │  │ │
│  │  │  append-only    │  │  healthcheck                │  │ │
│  │  └─────────────────┘  └─────────────────────────────┘  │ │
│  │  ┌─────────────────┐  ┌─────────────────────────────┐  │ │
│  │  │ proxy-caddy     │  │ secrets-age                 │  │ │
│  │  │  admin API      │  │  X25519 + Argon2id          │  │ │
│  │  │  route mgmt     │  │  XChaCha20-Poly1305         │  │ │
│  │  └─────────────────┘  └─────────────────────────────┘  │ │
│  │  ┌─────────────────┐  ┌─────────────────────────────┐  │ │
│  │  │ backup          │  │ systemd                     │  │ │
│  │  │  VACUUM INTO    │  │  unit lifecycle             │  │ │
│  │  │  integrity      │  │  port allocator             │  │ │
│  │  └─────────────────┘  └─────────────────────────────┘  │ │
│  └────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

---

## Crate Dependency Graph

```
sovereign (binary)
├── sovereign-core
├── sovereign-storage-sqlite → sovereign-core
├── sovereign-runtime-docker → sovereign-core
├── sovereign-proxy-caddy → sovereign-core
├── sovereign-secrets-age → sovereign-core
├── sovereign-backup → sovereign-core
├── sovereign-auth → sovereign-core
├── sovereign-systemd → sovereign-core, sovereign-pack
├── sovereign-pack → sovereign-core
├── sovereign-git
├── sovereign-chatops → sovereign-core
├── sovereign-doctor
├── sovereign-update
├── sovereign-observability
├── sovereign-notify
└── sovereign-proto
```

---

## Data Model

### Core Tables

#### `app`

| Column | Type | Constraints | Description |
|--------|------|-------------|-------------|
| `id` | BLOB (UUID) | PRIMARY KEY | Application identifier |
| `name` | TEXT | UNIQUE, NOT NULL | DNS-label safe name |
| `owner` | TEXT | NOT NULL | `user:<id>` or `system` |
| `env` | TEXT | CHECK IN ('dev','staging','prod') | Target environment |
| `git_repo` | TEXT | NULLABLE | Git repository URL |
| `image_ref` | TEXT | NULLABLE | Last deployed image |
| `config_yaml` | TEXT | NOT NULL | Full app.yaml snapshot |
| `health_path` | TEXT | NULLABLE | HTTP health endpoint |
| `deploy_mode` | TEXT | CHECK IN ('pull','build','pack','native') | Deploy strategy |
| `source_repo` | TEXT | NULLABLE | Source git URL |
| `source_branch` | TEXT | NULLABLE | Source branch |
| `auto_deploy` | BOOLEAN | DEFAULT false | Auto-deploy on push |
| `auto_deploy_window` | TEXT | NULLABLE | Time window for auto-deploys |
| `status` | TEXT | CHECK IN ('active','draining','archived') | Lifecycle status |
| `version` | INTEGER | DEFAULT 1 | Optimistic concurrency |
| `created_at` | INTEGER | NOT NULL | Unix timestamp |
| `updated_at` | INTEGER | NOT NULL | Unix timestamp |

#### `deployment`

| Column | Type | Constraints | Description |
|--------|------|-------------|-------------|
| `id` | BLOB (UUID) | PRIMARY KEY | Deployment identifier |
| `app_id` | BLOB | FK → app.id | Parent app |
| `image_ref` | TEXT | NOT NULL | Image or binary reference |
| `strategy` | TEXT | CHECK IN ('bluegreen','rolling','recreate') | Deploy strategy |
| `status` | TEXT | CHECK IN ('pending','building','pushing','starting','healthy','failed','rolled_back') | Current status |
| `triggered_by` | TEXT | NOT NULL | Actor who triggered |
| `risk_score` | REAL | NULLABLE | Risk assessment |
| `target_deployment_id` | BLOB | NULLABLE | Rollback target |
| `error_message` | TEXT | NULLABLE | Failure reason |
| `started_at` | INTEGER | NOT NULL | Start time |
| `finished_at` | INTEGER | NULLABLE | End time |

#### `audit_event`

| Column | Type | Constraints | Description |
|--------|------|-------------|-------------|
| `id` | BLOB (UUID) | PRIMARY KEY | Event identifier |
| `kind` | TEXT | NOT NULL | Event type |
| `actor` | TEXT | NOT NULL | Who performed |
| `subject_kind` | TEXT | NOT NULL | Resource type |
| `subject_id` | TEXT | NULLABLE | Resource identifier |
| `metadata_json` | TEXT | NULLABLE | Additional data |
| `created_at` | INTEGER | NOT NULL | Unix timestamp |

**Trigger:** `reject_audit_mutation` — blocks UPDATE/DELETE on audit_event

#### `secret`

| Column | Type | Constraints | Description |
|--------|------|-------------|-------------|
| `id` | BLOB (UUID) | PRIMARY KEY | Secret identifier |
| `app_id` | BLOB | FK → app.id | Parent app |
| `key` | TEXT | NOT NULL | Environment variable name |
| `ciphertext` | BLOB | NOT NULL | Age-encrypted value |
| `status` | TEXT | CHECK IN ('active','rotating','archived') | Lifecycle status |
| `version` | INTEGER | DEFAULT 1 | Optimistic concurrency |
| `created_at` | INTEGER | NOT NULL | Unix timestamp |
| `updated_at` | INTEGER | NOT NULL | Unix timestamp |

---

## State Machines

### Deployment Status

```
                    ┌─────────┐
                    │ Pending │
                    └────┬────┘
                         │ pull_image()
                    ┌────▼────┐
                    │Building │
                    └────┬────┘
                         │ create_container()
                    ┌────▼────┐
                    │Pushing  │
                    └────┬────┘
                         │ start_container()
                    ┌────▼────┐
                    │Starting │
                    └────┬────┘
                         │ healthcheck()
              ┌──────────┼──────────┐
              │                     │
         ┌────▼────┐          ┌────▼────┐
         │ Healthy │          │ Failed  │
         └────┬────┘          └─────────┘
              │ rollback()
         ┌────▼────────┐
         │RolledBack   │
         └─────────────┘
```

### App Status

```
    ┌────────┐
    │ Active │
    └───┬────┘
        │ archive()
   ┌────▼────┐
   │Draining │
   └────┬────┘
        │ archive()
   ┌────▼────┐
   │Archived │
   └─────────┘
```

---

## API Conventions

### Response Envelope

```json
{
  "status": "ok",
  "exit_code": 0,
  "data": { ... },
  "ts": "2026-06-09T12:00:00Z"
}
```

### Error Envelope

```json
{
  "status": "error",
  "exit_code": 1,
  "error": {
    "message": "app not found",
    "code": "NOT_FOUND"
  },
  "ts": "2026-06-09T12:00:00Z"
}
```

### Exit Codes

| Code | Name | Meaning |
|------|------|---------|
| 0 | Success | Command completed |
| 1 | Generic | Unexpected error |
| 2 | Usage | Bad arguments |
| 3 | NoData | Resource not found |
| 4 | Upstream | External service failure |
| 5 | Doctor | Diagnostic found issues |

---

## Failure Modes

### Deploy Failures

| Failure | Behavior | Recovery |
|---------|----------|----------|
| Image pull fails | Deployment marked Failed | Fix image reference, redeploy |
| Container create fails | Deployment marked Failed | Check Docker logs |
| Container start fails | Container stopped + removed | Check port conflicts |
| Health check fails | Container stopped + removed, previous kept running | Fix app, redeploy |
| Proxy route fails | Warning logged, deploy succeeds | Run `sovereign domain add` |

### Storage Failures

| Failure | Behavior | Recovery |
|---------|----------|----------|
| DB locked | Error returned | Check for concurrent access |
| Migration fails | Open fails | Check file permissions |
| Write fails | Transaction rolled back | Check disk space |

### Runtime Failures

| Failure | Behavior | Recovery |
|---------|----------|----------|
| Docker not running | Clean error message | Start Docker |
| Docker socket perms | Error with suggestion | Add user to docker group |
| Port conflict | Port allocator picks next | Retry |

---

## Security Model

### Secret Encryption

```
Master Key (X25519 identity)
    │
    ├─ File: /var/lib/sovereign/master.key
    │  └─ Wrapped with Argon2id + XChaCha20-Poly1305
    │     └─ Passphrase: SOVEREIGN_PASSPHRASE or stdin
    │
    └─ Used to encrypt/decrypt secrets
       └─ Algorithm: age (X25519 + XChaCha20-Poly1305)
```

### RBAC

```
Owner > Admin > Developer > Readonly

Actions:
  AppDeploy      → Owner, Admin, Developer
  AppRollback    → Owner, Admin, Developer
  SecretSet      → Owner, Admin, Developer
  SecretRead     → Owner, Admin, Developer, Readonly
  BackupCreate   → Owner, Admin
  UserManage     → Owner, Admin
  ChatopsExec    → Owner, Admin, Developer
```

### Webhook Authentication

```
HMAC-SHA256
  Key: WEBHOOK_HMAC_SECRET from secrets store
  Signature: X-Hub-Signature-256 header
  Fail-open: if no secret configured, accept
```

---

## Performance Characteristics

| Metric | Target | Notes |
|--------|--------|-------|
| Binary size | 10-25 MB | musl static |
| RSS (idle) | 80-150 MB | mimalloc on musl |
| RSS (deploying) | 200-400 MB | Transient during image pull |
| Startup time | < 100ms | SQLite WAL mode |
| Deploy time | < 30s | Excluding image pull |
| DB size (100 apps) | < 10 MB | With audit log |
| Audit log growth | ~1 KB/event | Append-only |
