# Sovereign Deployment Guide

**How to deploy apps with Sovereign — from zero to production.**

---

## Table of Contents

- [Prerequisites](#prerequisites)
- [Installation](#installation)
- [First Deploy](#first-deploy)
- [Deploy Modes](#deploy-modes)
- [Secrets Setup](#secrets-setup)
- [Custom Domains](#custom-domains)
- [Backup Strategy](#backup-strategy)
- [Telegram Chatops](#telegram-chatops)
- [Multi-User Setup](#multi-user-setup)
- [Production Hardening](#production-hardening)
- [Troubleshooting](#troubleshooting)

---

## Prerequisites

| Requirement | Minimum | Recommended |
|-------------|---------|-------------|
| OS | Ubuntu 22.04 LTS | Ubuntu 22.04 LTS |
| RAM | 512 MB | 2 GB |
| Disk | 10 GB | 50 GB |
| CPU | 1 vCPU | 2 vCPU |
| Network | Public IPv4 | Public IPv4 + domain |

**Optional:**
- Docker (for container deploys)
- Caddy (for auto-TLS)
- Git (for build/pack modes)

---

## Installation

### Quick Install

```bash
curl -sSf https://install.sovereignruntime.dev | sh
```

### Manual Install

```bash
# Download latest release
curl -sSf https://releases.sovereignruntime.dev/sovereign-linux-x86_64 -o /usr/local/bin/sovereign
chmod +x /usr/local/bin/sovereign

# Verify
sovereign --version
```

### Build from Source

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add musl target
rustup target add x86_64-unknown-linux-musl

# Build
cargo build --release --target x86_64-unknown-linux-musl

# Install
cp target/x86_64-unknown-linux-musl/release/sovereign /usr/local/bin/
```

---

## First Deploy

### 1. Initialize

```bash
cd /srv/myapp
sovereign init
```

Output:
```
detected framework: fastapi
wrote app.yaml
next: sovereign login
```

### 2. Login (Generate Master Key)

```bash
sovereign login
```

Output:
```
enter passphrase: ********
master key created at /var/lib/sovereign/master.key
public key: age1abc123...
next: sovereign deploy
```

### 3. Deploy

```bash
# Docker mode (default)
sovereign deploy --image ghcr.io/me/api:v1.0.0

# Or with --wait to block until healthy
sovereign deploy --image ghcr.io/me/api:v1.0.0 --wait
```

Output:
```
deploying myapp (strategy=bluegreen, wait=true)...
pulling ghcr.io/me/api:v1.0.0...
creating container sovereign-abc123-456...
starting container...
health check passed (200 OK)
myapp is live at https://myapp.127.0.0.1.sslip.io
```

### 4. Verify

```bash
sovereign status
sovereign logs --app myapp --tail 20
```

---

## Deploy Modes

### Pull Mode (Default)

CI builds the image, pushes to registry. Sovereign pulls and deploys.

```bash
# app.yaml
name: myapp
framework: fastapi
port: 8000
health_path: /health
image: ghcr.io/me/api:v1.2.3
source:
  deploy_mode: pull
```

```bash
sovereign deploy --app myapp
```

### Build Mode

Sovereign clones the repo and builds the image locally.

```yaml
source:
  deploy_mode: build
  repo: https://github.com/me/myapp
  branch: main
```

```bash
sovereign deploy --app myapp
```

### Pack Mode

CI packs a .sov archive, sovereign unpacks and deploys.

```yaml
source:
  deploy_mode: pack
  repo: https://github.com/me/myapp
```

```bash
# CI creates archive
tar czf myapp.sov manifest.json binary config/

# Sovereign deploys
sovereign deploy --app myapp --image ./myapp.sov
```

### Native Mode

No Docker required. Binary runs as systemd service.

```yaml
source:
  deploy_mode: native
```

```bash
# Create .sov archive with your binary
# Format: tar.zst with manifest.json, binary, config/

# Deploy
sovereign deploy --app myapp \
  --image ./myapp.sov \
  --native-binary ./myapp \
  --native-exec-start "./myapp --port {port}"
```

---

## Secrets Setup

### Set Secrets

```bash
# Set from stdin (avoids /proc/<pid>/cmdline leaks)
echo "postgresql://user:pass@localhost/db" | sovereign secret set DATABASE_URL --app myapp

# Set multiple secrets
echo "sk-1234567890" | sovereign secret set API_KEY --app myapp
echo "redis://localhost:6379" | sovereign secret set REDIS_URL --app myapp
```

### List Secrets

```bash
sovereign secret list myapp
# Output:
# DATABASE_URL
# API_KEY
# REDIS_URL
```

### Rotate Secrets

```bash
sovereign secret rotate DATABASE_URL --app myapp
```

### How It Works

1. Value encrypted with age (X25519 + XChaCha20-Poly1305)
2. Stored as ciphertext in SQLite
3. At deploy time, decrypted and injected as env vars
4. Plaintext never touches disk

---

## Custom Domains

### Add Domain

```bash
sovereign domain add api.example.com --app myapp
```

### List Domains

```bash
sovereign domain list --app myapp
```

### How It Works

1. Sovereign pushes route to Caddy admin API
2. Caddy provisions TLS certificate (HTTP-01 ACME)
3. Reverse proxy forwards to app port

### DNS Setup

```bash
# Point your domain to the server
# A record: api.example.com → 203.0.113.50

# Verify
curl https://api.example.com/health
```

---

## Backup Strategy

### Create Backups

```bash
# Manual backup
sovereign backup create --app myapp

# Cron job (daily at 2am)
0 2 * * * /usr/local/bin/sovereign backup create --app myapp
```

### List Backups

```bash
sovereign backup list
```

### Verify Integrity

```bash
sovereign backup verify <backup-id>
```

### Restore

```bash
sovereign backup restore <backup-id> --to /var/lib/sovereign/myapp.db
```

### Offsite Backup

```bash
# Copy to remote storage
scp /var/lib/sovereign/backups/* backup-server:/backups/
```

---

## Telegram Chatops

### 1. Create Bot

1. Message @BotFather on Telegram
2. Create new bot: `/newbot`
3. Get token: `123456789:ABCdefGHIjklMNOpqrsTUVwxyz`

### 2. Generate Binding

```bash
sovereign chatops init --user alice@example.com
# Output:
# Binding code: 482913
# Share: https://t.me/sovereign_bot?start=482913
```

### 3. Start Bot

```bash
sovereign chatops start --token 123456789:ABCdefGHIjklMNOpqrsTUVwxyz
```

### 4. User Binds

User sends: `/start 482913`

### 5. Use Commands

```
/status          → Fleet status
/apps            → List apps
/deploy myapp    → Deploy (with confirmation)
/rollback myapp  → Rollback (with confirmation)
/backup myapp    → Backup (with confirmation)
/secret list myapp → List secrets
/doctor          → Health checks
```

---

## Multi-User Setup

### Add Users

```bash
# First user becomes Owner
sovereign user add alice@example.com --password --role admin

# Add developer
sovereign user add bob@example.com --password --role developer

# Add viewer
sovereign user add carol@example.com --password --role readonly
```

### Create API Tokens

```bash
# Token for CI/CD
sovereign token create ci-deploy --scopes "app.deploy" --ttl 90d

# Token for monitoring
sovereign token create monitor --scopes "app.read,secret.read" --ttl 365d
```

### Use Tokens

```bash
# Set token for CLI
export SOVEREIGN_TOKEN=so_abc123...

# Or use flag
sovereign status --token so_abc123...
```

---

## Production Hardening

### 1. Install as System Service

```bash
sovereign daemon install
sudo systemctl enable sovereign-daemon
sudo systemctl start sovereign-daemon
```

### 2. Install System Services

```bash
# Web server
sovereign service install nginx

# Database
sovereign service install mysql

# Cache
sovereign service install redis

# TLS
sovereign service install letsencrypt
```

### 3. Configure Firewall

```bash
# Allow HTTP/HTTPS
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp

# Allow SSH
sudo ufw allow 22/tcp

# Enable
sudo ufw enable
```

### 4. Set Up Monitoring

```bash
# Run doctor daily
0 6 * * * /usr/local/bin/sovereign doctor --report /var/log/sovereign/doctor.md

# Check for updates weekly
0 3 * * 1 /usr/local/bin/sovereign update check
```

### 5. Backup Strategy

```bash
# Daily backups
0 2 * * * /usr/local/bin/sovereign backup create --app myapp

# Weekly verification
0 4 * * 0 /usr/local/bin/sovereign backup verify <latest-backup-id>

# Offsite sync
0 5 * * * rsync -av /var/lib/sovereign/backups/ backup-server:/backups/
```

---

## Troubleshooting

### Deploy Fails

```bash
# Check status
sovereign status --app myapp

# Check logs
sovereign logs --app myapp --tail 50

# Run diagnostics
sovereign doctor
```

### Docker Not Running

```bash
# Check Docker
sudo systemctl status docker

# Start Docker
sudo systemctl start docker

# Add user to docker group
sudo usermod -aG docker $USER
```

### Port Conflict

```bash
# Check what's using the port
sudo lsof -i :8080

# Kill the process
sudo kill <pid>
```

### Database Locked

```bash
# Check for concurrent access
fuser /var/lib/sovereign/sovereign.db

# WAL checkpoint
sqlite3 /var/lib/sovereign/sovereign.db "PRAGMA wal_checkpoint(TRUNCATE);"
```

### Master Key Lost

```bash
# If you have the passphrase
sovereign login

# If passphrase lost, re-init
sovereign init --force
sovereign login
# Note: existing secrets cannot be decrypted
```

### Rollback Stuck

```bash
# List deployments
sovereign rollback myapp --list

# Force rollback to specific version
sovereign rollback myapp --to <deployment-id>
```

---

## Reference

- [CLI Reference](../README.md#cli-reference)
- [Architecture](./architecture-v2.md)
- [Operations Runbook](./operations-runbook.md)
- [Doctor Spec](./doctor.md)
