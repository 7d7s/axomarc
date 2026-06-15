# Phase 0.6 — Git Deploy, Webhook, Pack, Native Runtime Integration

**Timeline:** ~8 weeks (overlaps with existing S1-S6 sub-phases)
**Goal:** Config-driven deploy modes (pull/build/pack), per-app webhook auth, `.sov` pack archive format, native runtime (no Docker), Telegram auto-notifications.
**Audience:** Lead engineer, CI/CD maintainers, VPS operators.
**Definition of Done:** Git push triggers deploy via webhook. `.sov` archives work. Native binaries deploy without Docker. All modes configurable per app. 390+ tests passing.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────┐
│                   sovereign daemon                        │
│  ┌───────────────────┐  ┌──────────────────────────────┐ │
│  │  Axum HTTP Server  │  │  Telegram long-poll loop     │ │
│  │                    │  │  (S4/S5/P7 extension)        │ │
│  │  POST /webhook/<a> │  │                              │ │
│  │  GET  /health      │  │  /status, /apps, /deploy,   │ │
│  │  GET  /metrics     │  │  /logs, /rollback, /backup  │ │
│  └────────┬──────────┘  └──────────────────────────────┘ │
└───────────┼──────────────────────────────────────────────┘
            │ dispatch based on deploy_mode
            ▼
┌─────────────────────────────────────────────────────────┐
│              sovereign-core use cases                    │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────────┐  │
│  │  pull    │ │  build   │ │  pack    │ │  native   │  │
│  │  deploy  │ │  deploy  │ │  deploy  │ │  deploy   │  │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘ └─────┬─────┘  │
└───────┼────────────┼────────────┼──────────────┼────────┘
        ▼            ▼            ▼              ▼
┌──────────┐ ┌────────────┐ ┌──────────┐ ┌──────────────┐
│  bollard │ │  sovereign- │ │  bollard │ │  systemd +   │
│  pull    │ │  git clone  │ │  load    │ │  binary exec │
│          │ │  + bollard  │ │          │ │              │
│          │ │  build      │ │          │ │              │
└──────────┘ └────────────┘ └──────────┘ └──────────────┘
```

---

## Deploy Modes

| Mode | Flow | Build happens | Use case |
|---|---|---|---|
| `pull` | git push -> CI builds image -> webhook -> sovereign pulls + deploys | GitHub Actions (off-box) | zero CPU on VPS, fast |
| `build` | git push -> webhook -> sovereign clones + docker build + deploys | VPS (on-box) | self-contained, no CI dependency |
| `pack` | CI packs .sov -> webhook (body or URL) -> sovereign unpacks + deploys | anywhere | build anywhere, ship artifact |

Default: `pull` (backward compatible with existing image-only deploys).

---

## `.sov` Archive Format

```
myapp-v1.2.3.sov  (tar.zst)
├── manifest.json          # required (first file)
├── image.tar              # present if runtime=container
├── binary                 # present if runtime=native
├── config/                # optional config overrides
│   └── nginx-vhost.conf
└── checksums.sha256       # required (last file, sha256 of all other files)
```

Manifest schema:
```json
{
  "app": "myapp",
  "version": "1.2.3",
  "runtime": "container",
  "image_ref": "ghcr.io/user/myapp:1.2.3",
  "binary_path": "binary",
  "systemd_exec": "/opt/sovereign/apps/myapp/bin/myapp --port {port}",
  "health_path": "/health",
  "port": 8000,
  "env": { "RUST_LOG": "info" },
  "created_at": "2026-06-07T14:30:00Z",
  "built_by": "ci"
}
```

---

## Webhook Endpoint

```
POST /webhook/<app-name>
Headers: X-Hub-Signature-256: sha256=<hex>
         Content-Type: application/json (GitHub) | application/octet-stream (.sov)

Body (GitHub):  {"ref": "refs/heads/main", "commits": [...]}
Body (.sov):    raw binary of .sov archive
Body (URL):     {"pack_url": "https://cdn.example.com/releases/v1.2.3.sov"}
```

Flow:
1. Look up app by name
2. Read `webhook_secret_ref` from secrets store (per-app HMAC key)
3. HMAC-SHA256 verify
4. Rate limit check (per-app max_auto_deploys_per_hour)
5. Dispatch by deploy_mode
6. Return 202 Accepted with deploy tracking URL

---

## P0 — Daemon Foundation (2 weeks, parallel with S1)

**Goal:** `sovereign daemon` starts a long-running HTTP + Telegram process.
**Purpose:** Webhooks need an HTTP server. Telegram needs a long-running poller. One daemon, one config, one systemd service.

### Sub-tasks

1. **CLI: `DaemonCmd` enum in `cli.rs`**
   - `Start`, `Install`, `Status`, `Logs` variants
   - Wire `Cmd::Daemon { cmd }` in `commands.rs`

2. **Config block in `sovereign.toml`**
   ```toml
   [daemon]
   listen_addr = "127.0.0.1:8443"
   ```

3. **Axum HTTP server skeleton**
   - `POST /webhook/<app>` — 501 Not Implemented stub (wired in P4)
   - `GET /health` — daemon version + uptime
   - Graceful shutdown on SIGTERM/SIGINT

4. **Systemd unit writer**
   - Template: `ExecStart=/usr/bin/sovereign daemon start`, `Restart=always`
   - `sovereign daemon install` writes + enables unit

5. **Telegram poller stub**
   - `TelegramPoller::run_forever()` as tokio task
   - Reads token from config
   - Stub: logs received updates

6. **Shared `connect.rs`**
   - `connect_storage()`, `connect_runtime()`, `connect_proxy()`, `connect_secrets()`, `connect_backup()`
   - Refactor `commands_deploy`, `commands_rollback` to use shared helpers

### Files Changed
| File | Change |
|---|---|
| `crates/sovereign/src/cli.rs` | Add `Daemon` variant to `Cmd`, `DaemonCmd` enum |
| `crates/sovereign/src/commands.rs` | Wire `Cmd::Daemon` |
| `crates/sovereign/src/commands_daemon.rs` | **NEW** — run_install, run_start, run_status, run_logs |
| `crates/sovereign/src/daemon.rs` | **NEW** — axum server, router, graceful shutdown |
| `crates/sovereign/src/connect.rs` | **NEW** — shared connect helpers |

### Tests
| Level | Count | What |
|---|---|---|
| Unit | 15 | Config parse, systemd template, URL routing, health response, Telegram backoff |
| Integration | 4 | Daemon boots on random port, /health returns 200, /webhook unknown-app returns 404, graceful shutdown within 5s |
| Security | 2 | Daemon refuses privileged port <1024 without sudo; logs don't include token value |

---

## P1 — `sovereign-pack` crate (2 weeks, parallel with S1/S2)

**Goal:** Create, inspect, push, validate `.sov` archives.
**Purpose:** The `.sov` archive is the universal deploy artifact.

### Sub-tasks

1. **New crate `crates/sovereign-pack/`**
   - `lib.rs`, `manifest.rs`, `archive.rs`, `checksum.rs`

2. **Manifest schema (`SovManifest`)** — see `.sov` Archive Format above

3. **Archive format** — tar.zst with manifest.json, image.tar/binary, config/, checksums.sha256

4. **CLI commands**
   - `sovereign pack create` — creates .sov from local files
   - `sovereign pack inspect` — prints manifest
   - `sovereign pack validate` — verify checksums + schema
   - `sovereign pack push` — HTTP PUT to URL

5. **Library API** (consumed by P4 webhook handler)
   - `create()`, `extract()`, `inspect()`, `validate()`

### Tests
| Level | Count | What |
|---|---|---|
| Unit | 30 | Manifest roundtrip, archive create/extract, checksum pass/fail, field validation, missing manifest, corrupted archive |
| Integration | 6 | Create .sov -> inspect matches, Create -> extract -> verify files, Push to local server, Validate valid, Validate corrupted, Large binary handling |
| Security | 3 | Checksum mismatch rejected, Tampering detected, Symlink path traversal rejected |

---

## P2 — `sovereign-git` crate (1 week, parallel with S1)

**Goal:** Clone, checkout, ls-remote via pure Rust (`gix`).
**Purpose:** `deploy_mode: build` needs VPS to clone repo locally.

### Sub-tasks

1. **New crate `crates/sovereign-git/`**
   - `clone()`, `checkout()`, `ls_remote()`, `latest_commit()`

2. **SSH key handling** — temp file with 600 perms, `GIT_SSH_COMMAND` injection, cleanup on drop

3. **Dependencies** — `gix = "0.70"` (pure Rust, no libgit2)

### Tests
| Level | Count | What |
|---|---|---|
| Unit | 10 | SSH key temp file permissions, URL parse, branch validation, shallow clone args |
| Integration | 6 | Clone public repo, checkout branch, ls-remote, HTTPS token auth, non-existent repo, latest-commit SHA |
| Security | 3 | Key file cleanup on drop, key cleanup on failure, error doesn't leak key |

---

## P3 — Source Config + Deploy Mode (2 weeks, extends S1)

**Goal:** `app.yaml` gets `source:` block. `DeployMode` in domain. Migration 0007.

### Sub-tasks

1. **Domain: `DeployMode` enum** — `Pull | Build | Pack`

2. **Domain: `SourceConfig` struct** — repo, branch, deploy_mode, webhook_secret_ref, auto_deploy, window, max_per_hour

3. **`app.yaml` source block** — optional `source:` with all fields

4. **Migration 0007** — add deploy_mode, source_repo, source_branch, auto_deploy, auto_deploy_window, max_auto_deploys_per_hour to app table

5. **Validator updates** — deploy_mode valid, window format valid, repo required when source present

### Tests
| Level | Count | What |
|---|---|---|
| Unit | 20 | DeployMode serde, AppConfig with source, missing source fallback, SQL constraint, validator valid/invalid, backward compat |
| Integration | 4 | Create app with source, read back, update deploy_mode, migration preserves data |

---

## P4 — Webhook Dispatch (3 weeks, overlaps S2-S3)

**Goal:** `POST /webhook/<app>` authenticates per-app HMAC, dispatches by deploy_mode.
**Purpose:** Git push -> CI -> webhook -> sovereign deploys. End-to-end automation.

### Sub-tasks

1. **Webhook handler** in daemon — look up app, HMAC verify, parse payload, dispatch

2. **HMAC verification** — `hmac_sha256` + constant-time compare

3. **Payload parsing** — GitHub push JSON, .sov binary, pack_url JSON

4. **Dispatch per deploy_mode** — pull/build/pack

5. **Rate limit counters** — in-memory per-app, reset hourly

6. **Audit events** — `AuditKind::Webhook` with full payload

7. **Bollard build support** — extend `RuntimePort` with `build_image()` and `load_image()`

### Tests
| Level | Count | What |
|---|---|---|
| Unit | 30 | HMAC correct/incorrect, payload parse (all 3 types), rate limit accept/reject, dispatch construction, branch matching, 401 missing secret |
| Integration | 8 | E2E pull deploy, E2E pack body deploy, E2E pack URL deploy, invalid HMAC 401, rate limited 429, non-matching ref 200, concurrent webhooks, cold start rate limit |
| Security | 5 | HMAC timing resistance, secret not logged, audit records outcome, error body not leaked, pack URL validation |

---

## P5 — Native Runtime (2 weeks, extends S1 systemd work)

**Goal:** `runtime: native` in `.sov` manifest -> extract binary -> systemd unit -> health check.
**Purpose:** CX22 with 2GB RAM runs more apps without Docker overhead.

### Sub-tasks

1. **App directory layout** — `/opt/sovereign/apps/<name>/bin/`, `bin.prev/`, `config/`, `data/`

2. **Systemd unit template** — `Type=simple`, `User=sovereign-<name>`, `Restart=on-failure`, `ProtectSystem=strict`

3. **Port allocation** — `/var/lib/sovereign/ports.lock`, ephemeral range 49152-65535

4. **User creation** — `useradd --system`, `chown` app directory

5. **Native health check** — HTTP probe, 3 retries, 30s budget

6. **Caddy proxy** — same as container: `reverse_proxy 127.0.0.1:{port}`

7. **Rollback for native** — stop unit, swap bin/bin.prev, start unit, health check

8. **New port: `SystemdNativePort`** — install_unit, remove_unit, start/stop/restart_unit, unit_status

### Tests
| Level | Count | What |
|---|---|---|
| Unit | 25 | Systemd template, port allocator, user creation, binary extraction, rollback swap, health probe, Caddy route |
| Integration | 6 | E2E deploy native -> health passes, E2E rollback, auto-recovery (kill process), port conflict, Caddy proxy, user isolation |
| Security | 2 | File permissions, no cross-app access |

---

## P6 — Auto-Deploy (1 week, after P4)

**Goal:** `auto_deploy: true` + window + rate limit.

### Sub-tasks

1. **Wire `auto_deploy` flag** in webhook dispatch

2. **Window parser** — `"HH:MM-HH:MM TZ"` format

3. **Rate limit wiring** — reuse P4 rate limiter

4. **Telegram auto-notification** — success/failure alerts to bound chat

### Tests
| Level | Count | What |
|---|---|---|
| Unit | 10 | Window parser, is_in_window, rate limiter with per-app config, auto_deploy=false skips, combined check |
| Integration | 3 | Webhook during window -> deploys, outside window -> 200 no deploy, 4th webhook in hour -> 429 |

---

## P7 — Telegram Extend (2-3 weeks, extends S4-S5)

**Goal:** New commands + auto-notifications.

### Sub-tasks

1. **`/deploy <app>`** — confirm -> start deploy

2. **`/rollback <app>`** — confirm -> select version -> rollback

3. **`/monitor <app>`** — CPU/RAM/disk one-shot

4. **`/backup <app>`** — confirm -> trigger backup

5. **`/deployments <app>`** — list last 5 deployments

6. **Auto-notifications** — deploy events on broadcast channel, Telegram task subscribes, pushes to bound chat

7. **Rate limit extension** — mutating commands share 10/hour/user; read-only unlimited

### Tests
| Level | Count | What |
|---|---|---|
| Unit | 20 | Command parse, keyboard construction, notification template, channel subscribe, rate limit counting |
| Integration | 8 | E2E /deploy, /rollback, failure notification, /backup, rate limit, multi-user isolation |
| Security | 2 | RBAC for app access, non-existent version error |

---

## New Crates Summary

| Crate | Phase | LOC | Dependencies Added |
|---|---|---|---|
| `sovereign-pack` | P1 | ~600 | `tar = "0.4"`, `zstd = "0.13"` |
| `sovereign-git` | P2 | ~400 | `gix = "0.70"` |
| `sovereign-daemon` | P0 | ~300 | (uses existing axum) |

## Extended Crates

| Crate | Change |
|---|---|
| `sovereign-core` | `DeployMode`, `SourceConfig`, `NativeUnitSpec`, `SystemdNativePort`, `Webhook` audit kind, `App` struct extension |
| `sovereign-runtime-docker` | `build_image()`, `load_image()` on `RuntimePort` |
| `sovereign-systemd` | `SystemdNativePort` impl, `NativeUnitSpec` |
| `sovereign` (CLI) | `Daemon`, `Pack`, `Git` subcommands, shared `connect.rs` |
| `sovereign-storage-sqlite` | Migration 0007 |
| `sovereign-proxy-caddy` | No changes (same Caddy routes for container+native) |

## Cumulative Test Budget

| Phase | New Unit | New Integration | New Security | Cumulative |
|---|---|---|---|---|
| V0.5 (current) | — | — | — | 215 |
| P0 Daemon | +15 | +4 | +2 | 236 |
| P1 Pack | +30 | +6 | +3 | 275 |
| P2 Git | +10 | +6 | +3 | 294 |
| P3 Source config | +20 | +4 | — | 318 |
| P4 Webhook | +30 | +8 | +5 | 361 |
| P5 Native runtime | +25 | +6 | +2 | 394 |
| P6 Auto-deploy | +10 | +3 | — | 407 |
| P7 Telegram extend | +20 | +8 | +2 | 437 |

---

## Implementation Order

```
Week 1-2:  P0 (daemon) + P2 (git)      [parallel, no dependency]
Week 3-4:  P1 (pack) + P3 (source)     [parallel, P3 needs P1 manifest types]
Week 5-7:  P4 (webhook)                 [needs P0 + P1 + P2 + P3]
Week 8-9:  P5 (native runtime)         [needs P1 + S1 systemd]
Week 10:   P6 (auto-deploy)            [needs P4]
Week 11-13: P7 (telegram extend)       [needs P4 + S4]
```

## Key Decisions

| Decision | Value |
|---|---|
| Daemon auth | Per-app HMAC only, no master key |
| Pack URL validation | No origin restriction, just fetch URL |
| Auto-deploy window | Simple `"HH:MM-HH:MM TZ"` format |
| Default deploy mode | `pull` (backward compatible) |
| Native runtime | Yes, included in V0.6 |
| Pack transport | Both direct POST body AND URL reference |
| Git clone depth | Shallow (`--depth 1`) by default |
