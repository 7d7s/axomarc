# Sovereign Application Runtime

**Single-binary, self-hosted, CLI-first deployment runtime.** Apache 2.0, EU-sovereign, 80-150 MB RSS, fits in a 512 MB VPS.

```bash
curl -sSf sovereignruntime.dev/install.sh | sh   # 60 seconds
sovereign deploy                                   # 5 minutes to a live URL
```

---

## Table of Contents

- [What is Sovereign?](#what-is-sovereign)
- [Features](#features)
- [Quickstart](#quickstart)
- [CLI Reference](#cli-reference)
- [Architecture](#architecture)
- [Workspace Layout](#workspace-layout)
- [Deploy Modes](#deploy-modes)
- [Secrets Management](#secrets-management)
- [Backup & Restore](#backup--restore)
- [Telegram Chatops](#telegram-chatops)
- [User Management & RBAC](#user-management--rbac)
- [Self-Update](#self-update)
- [Diagnostics](#diagnostics)
- [Building from Source](#building-from-source)
- [Configuration](#configuration)
- [The Invariants](#the-invariants)
- [License](#license)

---

## What is Sovereign?

Sovereign is a deployment runtime that replaces Docker Compose, Traefik, Portainer, and your DevOps team with a single Rust binary. It runs on any VPS (Hetzner CX22, OVH, your basement server) and manages your apps with:

- **One binary** — no Docker, no Kubernetes, no Node.js
- **One SQLite file** — append-only audit log, WAL mode, atomic migrations
- **One CLI** — `sovereign deploy` builds, routes, health-checks, and serves
- **Zero dependencies** — runs on Ubuntu 22.04 LTS, Debian 12, Alpine 3.20

---

## Features

### Core (V0)

| Feature | Command | Description |
|---------|---------|-------------|
| **Framework Detection** | `sovereign init` | Auto-detects FastAPI, Flask, Django, Next.js, Nuxt, SvelteKit, Remix, Express, Go, Rails, Laravel, Astro, Phoenix, Deno, Static, Generic |
| **Docker Deploy** | `sovereign deploy` | Pull image, create container, health check, Caddy route — all in one command |
| **Native Deploy** | `sovereign deploy` (native mode) | Extract binary from .sov archive, install systemd unit, allocate port — no Docker required |
| **Blue-Green Rollback** | `sovereign rollback` | One-command rollback to previous healthy deployment or specific version |
| **Secret Management** | `sovereign secret set/list/rotate` | Age-encrypted, zero-disk secrets injected as env vars at deploy time |
| **Backup & Restore** | `sovereign backup create/list/verify/restore` | SQLite VACUUM INTO snapshots with integrity verification |
| **Caddy Auto-TLS** | `sovereign domain add` | Automatic HTTPS via Caddy reverse proxy with HTTP-01 ACME |
| **Self-Update** | `sovereign update check/apply/rollback` | SHA-256 verified, atomic binary swap with rollback |
| **Diagnostics** | `sovereign doctor` | 5-level diagnostic engine: binary, storage, runtime, proxy, secrets, system |
| **Log Streaming** | `sovereign logs` | Docker container log streaming with --tail and --follow |
| **Fleet Status** | `sovereign status` | Overview of all apps with deployment status |
| **App Validation** | `sovereign validate` | Schema check for app.yaml before deploy |

### V0.6 (Hosting Control Plane)

| Feature | Command | Description |
|---------|---------|-------------|
| **System Services** | `sovereign service install/remove/status/restart` | Manage nginx, MySQL, MariaDB, Redis, vsftpd, Let's Encrypt, phpMyAdmin via apt + systemd |
| **User Management** | `sovereign user add/list/disable/enable/whoami` | Multi-user with RBAC (Owner, Admin, Developer, Viewer) |
| **API Tokens** | `sovereign token create/list/revoke` | Scoped tokens with TTL, SHA-256 hashed |
| **Daemon** | `sovereign daemon start/install/status/logs` | Axum HTTP server with webhook handling, health checks, Telegram poller |
| **Telegram Chatops** | `sovereign chatops init/start/revoke/bindings` | 13 commands, confirmation keyboards, natural language intents, per-user rate limiting |
| **Webhook Deploy** | POST /webhook/{app} | HMAC-SHA256 verified, auto-deploy windows, rate limiting |
| **Native Runtime** | Deploy mode `native` | Binary extraction from .sov archives, systemd unit lifecycle, port allocation |

### Infrastructure

| Component | Crate | Description |
|-----------|-------|-------------|
| **Domain Layer** | `sovereign-core` | Use cases, domain types, RBAC, storage port trait |
| **SQLite Storage** | `sovereign-storage-sqlite` | sqlx with forward-only migrations, append-only audit |
| **Docker Runtime** | `sovereign-runtime-docker` | bollard integration, container lifecycle |
| **Caddy Proxy** | `sovereign-proxy-caddy` | Admin API, route management, TLS |
| **Secrets (age)** | `sovereign-secrets-age` | X25519 identity, Argon2id KDF, XChaCha20-Poly1305 |
| **Backup** | `sovereign-backup` | VACUUM INTO snapshots, integrity checks |
| **Auth** | `sovereign-auth` | Password hashing (argon2id), API tokens, RBAC |
| **Pack/Unpack** | `sovereign-pack` | .sov archive format (tar.zst) |
| **Git Ops** | `sovereign-git` | Clone, checkout, ls-remote via git binary |
| **Systemd** | `sovereign-systemd` | Unit lifecycle, port allocator, native deploy |
| **Doctor** | `sovereign-doctor` | 5-level diagnostic engine |
| **Update** | `sovereign-update` | Self-update with SHA-256 verification |
| **Chatops** | `sovereign-chatops` | Telegram bot, commands, SQLite stores |
| **Observability** | `sovereign-observability` | tracing + Prometheus /metrics |
| **Notifications** | `sovereign-notify` | 10 alert channels |

---

## Quickstart

### Fresh VPS (Hetzner CX22, Ubuntu 22.04)

```bash
# Install Sovereign
curl -sSf https://install.sovereignruntime.dev | sh

# Navigate to your app
cd /srv/myapp

# Initialize (auto-detects framework)
sovereign init

# Set up master key
sovereign login

# Deploy (Docker mode)
sovereign deploy --image nginx:alpine

# Check status
sovereign status

# View logs
sovereign logs --app myapp --follow
```

### Native Mode (No Docker)

```bash
# Initialize with native deploy mode
sovereign init --name myapp

# Edit app.yaml: set source.deploy_mode to "native"

# Create a .sov archive with your binary
# (tar.zst format with manifest.json, binary, config/)

# Deploy native
sovereign deploy --app myapp \
  --image ./myapp.sov \
  --native-binary ./myapp \
  --native-exec-start "./myapp --port {port}"
```

### One-Shot (CI/CD)

```bash
curl -sSf https://install.sovereignruntime.dev | sh
cd myapp
sovereign init && sovereign login && sovereign deploy
```

---

## CLI Reference

### App Management

| Command | Description |
|---------|-------------|
| `sovereign init` | Detect framework, write app.yaml |
| `sovereign init --framework fastapi` | Force specific framework |
| `sovereign init --name myapp --port 8000` | Custom name and port |
| `sovereign validate [app.yaml]` | Validate app.yaml schema |
| `sovereign status` | Show all apps |
| `sovereign status --app myapp` | Show specific app |

### Deployment

| Command | Description |
|---------|-------------|
| `sovereign deploy --app myapp` | Deploy with app.yaml settings |
| `sovereign deploy --app myapp --image nginx:alpine` | Override image |
| `sovereign deploy --app myapp --wait` | Wait for healthy |
| `sovereign deploy --app myapp --strategy rolling` | Deployment strategy |
| `sovereign deploy --app myapp --no-lock` | Skip sovereign.lock |
| `sovereign deploy --app myapp --dry-run` | Preview without executing |

### Native Deploy

| Command | Description |
|---------|-------------|
| `sovereign deploy --app myapp --image ./app.sov` | Deploy from .sov archive |
| `sovereign deploy --app myapp --native-binary ./myapp` | Binary path in archive |
| `sovereign deploy --app myapp --native-exec-start "./myapp --port {port}"` | Exec start template |

### Rollback

| Command | Description |
|---------|-------------|
| `sovereign rollback myapp` | Rollback to previous healthy |
| `sovereign rollback myapp --to <deployment-id>` | Rollback to specific version |
| `sovereign rollback myapp --list` | List healthy deployments |
| `sovereign rollback myapp --list --limit 5` | Limit list output |

### Secrets

| Command | Description |
|---------|-------------|
| `sovereign secret set DATABASE_URL --app myapp` | Set secret (reads from stdin) |
| `sovereign secret list myapp` | List secret keys (values never shown) |
| `sovereign secret rotate DATABASE_URL --app myapp` | Rotate secret value |

### Backup

| Command | Description |
|---------|-------------|
| `sovereign backup create --app myapp` | Create backup snapshot |
| `sovereign backup list` | List all backups |
| `sovereign backup verify <backup-id>` | Verify backup integrity |
| `sovereign backup restore <backup-id> --to /path` | Restore backup |

### Logs & Status

| Command | Description |
|---------|-------------|
| `sovereign logs --app myapp` | Show last 100 lines |
| `sovereign logs --app myapp --tail 50` | Show last 50 lines |
| `sovereign logs --app myapp --follow` | Follow log stream |
| `sovereign status` | Fleet status overview |

### Domain & TLS

| Command | Description |
|---------|-------------|
| `sovereign domain add api.example.com --app myapp` | Add hostname route |
| `sovereign domain list --app myapp` | List hostnames for app |

### Users & Tokens

| Command | Description |
|---------|-------------|
| `sovereign user add alice@example.com --role admin` | Add user |
| `sovereign user list` | List all users |
| `sovereign user disable alice@example.com` | Disable user |
| `sovereign user enable alice@example.com` | Enable user |
| `sovereign user whoami` | Show current user |
| `sovereign token create mytoken --scopes "app.deploy,secret.read"` | Create API token |
| `sovereign token list` | List tokens |
| `sovereign token revoke mytoken` | Revoke token |

### Telegram Chatops

| Command | Description |
|---------|-------------|
| `sovereign chatops init --user alice@example.com` | Generate binding code |
| `sovereign chatops start --token <bot-token>` | Start Telegram poller |
| `sovereign chatops revoke --user alice@example.com` | Revoke binding |
| `sovereign chatops bindings` | List active bindings |

### Daemon

| Command | Description |
|---------|-------------|
| `sovereign daemon start` | Start HTTP server |
| `sovereign daemon start --listen 0.0.0.0:8443` | Custom listen address |
| `sovereign daemon install` | Install systemd unit |
| `sovereign daemon status` | Check daemon status |
| `sovereign daemon logs --lines 100` | Show daemon logs |

### System Services

| Command | Description |
|---------|-------------|
| `sovereign service install nginx` | Install nginx |
| `sovereign service remove nginx` | Remove nginx |
| `sovereign service status nginx` | Check nginx status |
| `sovereign service restart nginx` | Restart nginx |
| `sovereign service validate nginx` | Validate config (nginx -t) |
| `sovereign service list` | List all managed services |

### Self-Update

| Command | Description |
|---------|-------------|
| `sovereign update check` | Check for updates |
| `sovereign update apply` | Download and install |
| `sovereign update rollback` | Rollback to previous version |
| `sovereign update history` | Show update history |

### Diagnostics

| Command | Description |
|---------|-------------|
| `sovereign doctor` | Run basic checks |
| `sovereign doctor --explain` | Detailed check descriptions |
| `sovereign doctor --fix` | Auto-fix failing checks |
| `sovereign doctor --report /path` | Write markdown report |
| `sovereign doctor --json` | JSON output |

### Utilities

| Command | Description |
|---------|-------------|
| `sovereign completions bash` | Generate bash completions |
| `sovereign completions zsh` | Generate zsh completions |
| `sovereign completions fish` | Generate fish completions |
| `sovereign man /usr/local/share/man/man1/` | Install man pages |
| `sovereign version` | Show version |

---

## Architecture

### Hexagonal Architecture

Sovereign uses hexagonal (ports & adapters) architecture:

```
┌─────────────────────────────────────────────────────┐
│                    CLI (sovereign)                    │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────┐  │
│  │  commands_*  │  │   daemon     │  │  chatops   │  │
│  └──────┬──────┘  └──────┬───────┘  └─────┬──────┘  │
│         │                │                │          │
│  ┌──────▼────────────────▼────────────────▼──────┐  │
│  │              sovereign-core                     │  │
│  │  ┌────────────┐  ┌────────────┐  ┌──────────┐ │  │
│  │  │ use_cases  │  │   domain   │  │   rbac   │ │  │
│  │  └────────────┘  └────────────┘  └──────────┘ │  │
│  │  ┌────────────────────────────────────────┐   │  │
│  │  │              ports (traits)             │   │  │
│  │  │  StoragePort │ RuntimePort │ ProxyPort  │   │  │
│  │  └────────────────────────────────────────┘   │  │
│  └───────────────────────────────────────────────┘  │
│                      │                               │
│  ┌───────────────────▼───────────────────────────┐  │
│  │              adapters (crates)                  │  │
│  │  sovereign-storage-sqlite │ sovereign-runtime-docker │
│  │  sovereign-proxy-caddy    │ sovereign-secrets-age    │
│  │  sovereign-backup         │ sovereign-systemd        │
│  └───────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘
```

### Data Model

**Core Tables:**

| Table | Purpose |
|-------|---------|
| `app` | App definitions with deploy mode, env, source config |
| `deployment` | Deployment history with status machine |
| `audit_event` | Append-only audit log (UPDATE/DELETE rejected by trigger) |
| `secret` | Encrypted secrets per app |
| `backup` | Backup metadata |
| `domain` | Hostname → app routing |
| `server` | Server registry |
| `service` | System service registry |
| `user` | User accounts with roles |
| `api_token` | API token hashes |
| `chatops_binding` | Telegram chat → user bindings |
| `chatops_binding_code` | One-time binding codes |

**State Machines:**

- **Deployment:** Pending → Building → Pushing → Starting → Healthy | Failed
- **App Status:** Active → Draining → Archived
- **Secret:** Active → Rotating → Archived

### Deploy Flow

```
sovereign deploy --app myapp
       │
       ├─ 1. Open SQLite storage
       ├─ 2. Resolve app by name
       ├─ 3. Check deploy_mode
       │     ├─ Pull/Build/Pack → Docker path
       │     └─ Native → systemd path
       │
       ├─ Docker path:
       │     ├─ 4. Connect Docker runtime
       │     ├─ 5. Pull image
       │     ├─ 6. Create container (port 8080 → 127.0.0.1)
       │     ├─ 7. Start container
       │     ├─ 8. Health probe (HTTP GET /health)
       │     ├─ 9. Push Caddy route
       │     └─ 10. Write sovereign.lock
       │
       └─ Native path:
             ├─ 4. Ensure app directories
             ├─ 5. Extract binary from .sov archive
             ├─ 6. Allocate port (49152-65535)
             ├─ 7. Install systemd unit
             └─ 8. Return URL
```

---

## Workspace Layout

```
crates/
  sovereign/                    Binary (clap, tokio, mimalloc/jemalloc)
  sovereign-core/               Domain types, use cases, port traits
  sovereign-storage-sqlite/     SQLite storage with migrations
  sovereign-runtime-docker/     Docker runtime (bollard)
  sovereign-proxy-caddy/        Caddy reverse proxy
  sovereign-secrets-age/        Age encryption for secrets
  sovereign-backup/             Backup & restore
  sovereign-auth/               Password hashing, tokens, RBAC
  sovereign-systemd/            Systemd unit lifecycle
  sovereign-pack/               .sov archive format
  sovereign-git/                Git operations
  sovereign-chatops/            Telegram bot
  sovereign-doctor/             Diagnostic engine
  sovereign-update/             Self-update
  sovereign-observability/      Tracing & metrics
  sovereign-notify/             Alert channels
  sovereign-proto/              REST/JSON DTOs

docs/
  architecture.md               System architecture
  tech-stack.md                 Crate versions & rationale
  phase-00-mvp.md               V0 specification
  phase-0.6.md                  V0.6 hosting control plane
  phase-01-v1.md                V1 specification
  doctor.md                     Diagnostic engine spec
  operations-runbook.md         Production operations
  product-ux.md                 CLI/TUI design
  negative-prompt.md            Anti-patterns
  decision-records.md           Architecture decisions

scripts/
  install.sh                    One-liner installer
  sovereign.service             Systemd unit file
  sovereign.toml                Default config
```

---

## Deploy Modes

### Pull Mode (Default)

```yaml
name: myapp
framework: fastapi
port: 8000
health_path: /health
image: ghcr.io/me/api:v1.2.3
source:
  deploy_mode: pull
```

CI builds the image, pushes to registry. Sovereign pulls and deploys.

### Build Mode

```yaml
source:
  deploy_mode: build
  repo: https://github.com/me/myapp
  branch: main
```

Sovereign clones the repo and builds the image locally.

### Pack Mode

```yaml
source:
  deploy_mode: pack
  repo: https://github.com/me/myapp
```

CI packs a .sov archive, sovereign unpacks and deploys.

### Native Mode

```yaml
source:
  deploy_mode: native
```

No Docker required. Sovereign extracts a binary from a .sov archive and installs it as a systemd service.

---

## Secrets Management

```bash
# Set a secret (reads from stdin)
echo "postgresql://user:pass@localhost/db" | sovereign secret set DATABASE_URL --app myapp

# List secret keys (values never shown)
sovereign secret list myapp

# Rotate a secret
sovereign secret rotate DATABASE_URL --app myapp
```

**How it works:**
1. Secrets are encrypted with age (X25519 + XChaCha20-Poly1305)
2. Master key is wrapped with Argon2id (64 MiB, t=3, p=1)
3. At deploy time, secrets are decrypted and injected as env vars
4. Plaintext never touches disk — only exists in container memory

---

## Backup & Restore

```bash
# Create backup
sovereign backup create --app myapp

# List backups
sovereign backup list

# Verify integrity
sovereign backup verify <backup-id>

# Restore
sovereign backup restore <backup-id> --to /var/lib/sovereign/myapp.db
```

**How it works:**
1. Uses SQLite's `VACUUM INTO` for atomic snapshots
2. SHA-256 checksums for integrity verification
3. Metadata stored in `backup` table
4. Live-restore rejected in V0 (safety)

---

## Telegram Chatops

```bash
# Generate binding code
sovereign chatops init --user alice@example.com
# → Binding code: 482913
# → Share: https://t.me/sovereign_bot?start=482913

# Start the bot
sovereign chatops start --token 123456:ABC-DEF...

# User sends /start 482913 to bind
```

**Available commands:**
- `/help` — List commands
- `/status` — Fleet status
- `/apps` — List apps
- `/deploy <app>` — Deploy (with confirmation)
- `/rollback <app>` — Rollback (with confirmation)
- `/backup <app>` — Backup (with confirmation)
- `/secret list <app>` — List secrets
- `/deployments <app>` — Recent deployments
- `/doctor` — Health checks
- `/watch <app>` — Monitor app
- `/unwatch` — Stop monitoring
- `/audit` — Recent audit events

**Features:**
- Confirmation keyboards for mutating commands
- Natural language intents ("deploy myapp", "rollback api")
- Per-user rate limiting (10 mutable commands/hour)
- RBAC enforcement
- Audit logging

---

## User Management & RBAC

```bash
# Add first user (becomes Owner)
sovereign user add alice@example.com --password --role admin

# Add viewer
sovereign user add bob@example.com --role viewer

# List users
sovereign user list

# Create API token
sovereign token create mytoken --scopes "app.deploy,secret.read" --ttl 30d
```

**Roles:**
- **Owner** — Full access (first user auto-assigned)
- **Admin** — Manage users, deploy, rollback
- **Developer** — Deploy, rollback, secrets
- **Readonly** — View only

---

## Self-Update

```bash
# Check for updates
sovereign update check

# Apply update
sovereign update apply

# Rollback
sovereign update rollback

# History
sovereign update history
```

**How it works:**
1. Downloads from `https://releases.sovereignruntime.dev`
2. SHA-256 verification
3. Atomic binary swap
4. Previous binary kept for rollback

---

## Diagnostics

```bash
# Basic checks
sovereign doctor

# Detailed
sovereign doctor --explain

# Auto-fix
sovereign doctor --fix

# Report
sovereign doctor --report /var/log/sovereign/doctor.md
```

**Check categories:**
- Binary — version, path, permissions
- Storage — DB exists, WAL mode, migrations
- Runtime — Docker socket, version
- Proxy — Caddy installed, config valid
- Secrets — Master key exists, permissions
- System — OS, memory, disk, network

---

## Building from Source

```bash
# Prerequisites
rustup target add x86_64-unknown-linux-musl

# Build musl static binary
cargo build --release --target x86_64-unknown-linux-musl

# Verify
file target/x86_64-unknown-linux-musl/release/sovereign
# → ELF 64-bit LSB executable, x86-64, statically linked

# Run tests
RUSTFLAGS="--cfg serde_json_orphan_optimize" cargo test

# Lint
RUSTFLAGS="--cfg serde_json_orphan_optimize" cargo clippy
```

---

## Configuration

Default config location: `/etc/sovereign/sovereign.toml`

```toml
[storage]
path = "/var/lib/sovereign/sovereign.db"

[proxy]
listen = "127.0.0.1:8443"

[secrets]
master_key_path = "/var/lib/sovereign/master.key"

[auth]
bootstrap_admin_email = "admin@example.com"
# oidc_issuer = "https://auth.example.com"
# oidc_client_id = "sovereign"
```

---

## The Invariants

1. **One operator, 5 years.** `sovereign doctor` is the on-call's first command.
2. **A product in 6 months.** Scope is the moat.
3. **Sovereign by construction.** EU-only defaults, no US sub-processors.
4. **The CLI is the product.** TUI is the daily-driver. Web UI is V2 opt-in.

---

## License

Apache 2.0, unmodified, irrevocable. The license will not change. The source is the contract. See [LICENSE](./LICENSE).
