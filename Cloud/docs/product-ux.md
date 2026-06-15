# Product UX

**Status:** Locked for V1. Updated as features ship.
**Audience:** Anyone building user-facing surfaces. The PM. The first design partners.
**Last updated:** 2026-06-04

This document is the **how the product should feel to use** guide. The CLI is the product. The TUI is the daily-driver. The web UI is V2 opt-in. Documentation is auto-generated from the CLI parser.

---

## 1. The 5-minute moment

The single most important product metric is **time-to-first-deploy**. From `curl -sSf sovereignruntime.dev/install.sh | sh` to a live URL on a temporary domain. Target: **2 minutes 30 seconds.**

```text
$ curl -sSf sovereignruntime.dev/install.sh | sh       # 1 — install (10s)
$ sovereign init                                        # 2 — detect framework, write app.yaml (5s)
$ sovereign login                                       # 3 — device-code flow, browser opens (15s)
$ sovereign deploy                                      # 4 — build, TLS, URL (90s)
$ open https://<id>.srvr.so                             # 5 — you are live
```

**Above 5 min, Mira (solo founder) is gone. Above 10 min, Alex (vibe coder with Claude Code) times out. Below 5 min, the product has a chance.**

### 1.1 The post-onboarding "what next?" prompt

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

**Five lines. One screen. No marketing.**

---

## 2. The 8 non-negotiables for the CLI

Every subcommand implements all 8. No exceptions.

### 2.1 `--help` is a teaching surface

Use `clap`'s `after_long_help` for 3-5 tips + 3-5 real example commands. The `--help` is the doc.

```text
$ sovereign deploy --help
Deploy an app

USAGE:
    sovereign deploy [OPTIONS] --app <APP>

OPTIONS:
        --app <APP>            App name
        --image <IMAGE>        Pre-built image (skips build)
        --strategy <STRATEGY>  Deploy strategy: recreate, rolling, bluegreen [default: bluegreen]
        --wait                 Wait for the deploy to be healthy before exiting
        --dry-run              Show what would happen
        --confirm-risk         Bypass the policy halt (use with care)

EXAMPLES:
    sovereign deploy --app api --image=ghcr.io/me/api:v1
    sovereign deploy --app api --strategy recreate
    sovereign deploy --app api --image=ghcr.io/me/api:v1 --wait --dry-run

TIPS:
    • For first-time deploys, use --wait so you see the URL.
    • For most deploys, the default bluegreen strategy is correct.
    • Use --dry-run to see the plan before applying.

See also: sovereign rollback, sovereign status, sovereign logs
```

### 2.2 `--json` everywhere, auto-detected

Humans get colored tables; agents get JSON envelopes. AI agents are first-class users.

```rust
fn effective_format(&self) -> OutputFormat {
    match self.format {
        OutputFormat::Auto => {
            if std::io::stdout().is_terminal() { OutputFormat::Text } else { OutputFormat::Json }
        }
        f => f,
    }
}
```

The JSON envelope:
```json
{
  "ok": true,
  "data": { "deployment_id": "abc-123", "url": "https://pr-48291.srvr.so" },
  "warnings": [],
  "errors": []
}
```

Errors are RFC 9457 problem+json:
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

### 2.3 Semantic exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Generic error |
| 2 | Usage error (bad args) |
| 3 | Partial success (e.g., some apps failed to deploy) |
| 4 | Upstream error (Docker, Caddy, S3) |
| 5 | Auth error |
| 6 | Not found |
| 7 | Conflict (optimistic concurrency) |
| 8 | Policy denied (V2+) |
| 9 | Risk score too high, requires `--confirm-risk` (V2+) |

CI and agents can decide whether to retry based on the code.

### 2.4 Spinner / X-of-Y / progress bar

Pick the right one. (Reference: [Evil Martians' guide to CLI UX](https://evilmartians.com/chronicles/cli-ux-best-practices-pitfalls).)

- **Spinner:** for operations of unknown duration (e.g., `sovereign deploy`).
- **X-of-Y:** for operations with discrete steps (e.g., `sovereign restore` — "Step 2 of 5").
- **Progress bar:** for operations with known progress (e.g., `sovereign backup create`).
- **No indicator:** for operations < 100ms.

### 2.5 Color the right way

- `NO_COLOR=1` → no color.
- `--no-color` flag → no color.
- `TERM=dumb` → no color.
- CI / non-TTY → no color.
- Semantic palette: red for errors, yellow for warnings, green for success, blue for info, gray for hints.

### 2.6 Errors that teach, not blame

```text
$ sovereign deploy --app api --image=nonexistent
✗ Image not found
  → Check the image name and tag
  → Try: docker pull nonexistent
  → If you meant to build, omit --image and ensure git_repo is set in app.yaml
```

The error chain is printed (cause → effect → fix), the cause is colored red, the fix is colored blue.

### 2.7 `--dry-run` on every destructive action

Default is "ask before doing." Non-interactive is `--yes` or `--confirm`.

```text
$ sovereign rollback api
? Roll back api to v1.4.2? This will replace the current version. (y/N)
```

```text
$ sovereign rollback api --dry-run
Would roll back api to v1.4.2.
  Current: v1.5.0 (started 11m ago)
  Target:  v1.4.2 (started 2d ago)
No changes made. Run without --dry-run to apply.
```

### 2.8 Shell completions for bash, zsh, fish, nushell, powershell

Free with `clap_complete`. Install via `sovereign completions install`.

```bash
# Install completions for the current shell
sovereign completions install
# Output: ✓ Installed completions for bash at ~/.local/share/bash-completion/completions/sovereign

# Or generate for a specific shell
sovereign completions bash > ~/.local/share/bash-completion/completions/sovereign
sovereign completions zsh > ~/.zsh/completions/_sovereign
sovereign completions fish > ~/.config/fish/completions/sovereign.fish
sovereign completions nushell > ~/.config/nushell/completions/sovereign.nu
sovereign completions powershell > $PROFILE/sovereign.ps1
```

---

## 3. The CLI command tree (V1)

The full tree is in [`architecture.md` §4.1](#). The most-used commands are:

```text
sovereign init <framework>              # detect, write app.yaml
sovereign login                          # device-code flow
sovereign deploy [APP]                   # the 5-minute moment
sovereign rollback [APP]                 # one-command rollback
sovereign logs [APP]                     # tail, follow, filter
sovereign status [APP]                   # fleet or app
sovereign secret set/get/list/rotate     # zero-disk injection
sovereign backup create/list/verify      # THE most important V1.2 command
sovereign server list/add/remove         # fleet
sovereign access list/add/revoke         # V1.5
sovereign audit --since 7d               # who did what
sovereign policy check                   # V2
sovereign morning-report                 # the 5-command daily
sovereign tui                            # interactive dashboard
sovereign health                         # overall health
sovereign certs list/renew               # TLS
sovereign db shell/upgrade               # drop into psql
sovereign update                         # self-update
sovereign sovereignty check              # V2: 10-point test
sovereign compliance map --standard ...  # V2: BSI C5, EUCS
sovereign export platform                # V2: DR
sovereign import platform                # V2: DR
sovereign vendor-disappear-test          # V2: the 100-year-old question
```

---

## 4. The 4 non-negotiables for the TUI

Get these wrong and your TUI leaves the user's terminal broken when something crashes. **Test them: open the TUI, kill -9 the process, check that the shell still echoes.**

1. **alt-screen mode** — `EnterAlternateScreen` + `LeaveAlternateScreen` on entry/exit.
2. **panic-safe terminal restore** — a `Drop` impl on the `App` struct that reverses all terminal changes, AND a panic hook that runs the same restoration.
3. **SIGWINCH handling** — `crossterm::event::resize()` is wired; layout reflows.
4. **SIGTSTP handling** — `Ctrl+Z` suspends; `fg` resumes; the terminal is intact.

### 4.1 The 6 views

- **Pulse** (home screen) — 5 lines: app count, healthy count, last deploy, last error, next backup.
- **Apps** (master list) — name, status, last deploy, last deploy version, response time p50.
- **App detail** (drill-down) — tabs: Logs, Deploys, Rollbacks, Metrics, Domains, Secrets, Config.
- **Servers** (multi-server in V1.5+) — fleet. CPU, RAM, disk.
- **Backups** — every database, last backup, last verify, restore button.
- **Audit** — filterable timeline.

### 4.2 The keybindings (universal conventions)

```text
q           quit
Esc         back / dismiss
j, k        down, up
h, l        left, right
/           search
n, N        next, prev match
?           help for current view
:           command mode
Enter       select
Tab         switch panel
Space       toggle
g, G        top, bottom
Ctrl+P      command palette
```

These are the k9s / lazygit / vim conventions. No learning curve.

---

## 5. The framework scanner (the highest-leverage V1 code)

**Why this matters:** A 5-minute first deploy is the price of admission. A 5-minute first deploy *that worked without me reading docs* is the moat. Coolify has 280+ templates; the scanner doesn't need to be that good for V1, but it must work for the 6 golden paths.

### 5.1 The 6 golden paths

| Framework | Marker files | Build command | Start command | Port |
|---|---|---|---|---|
| **FastAPI** | `pyproject.toml` + `fastapi` | `pip install -r requirements.txt` | `uvicorn main:app --host 0.0.0.0 --port 8000` | 8000 |
| **Next.js** | `package.json` + `next` | `npm run build` | `npm run start -- -p 3000` | 3000 |
| **Laravel** | `composer.json` + `laravel/framework` | `composer install --no-dev` | `php artisan serve --host=0.0.0.0 --port=8000` | 8000 |
| **Go** | `go.mod` | `go build -o app .` | `./app` | 8080 |
| **Rails** | `Gemfile` + `rails` | `bundle install` | `bundle exec rails server -b 0.0.0.0 -p 3000` | 3000 |
| **Astro** | `package.json` + `astro` | `npm run build` | `npm run preview -- --host 0.0.0.0 --port 4321` | 4321 |

### 5.2 The generated `app.yaml`

```yaml
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
    schedule: "0 3 * * *"
    retention: 30d
    verify: weekly
```

---

## 6. Documentation — auto-generated, never lies

**The `--help` output IS the documentation.** Auto-generate man pages (`clap_mangen`). Auto-generate the web docs from `clap` (a 200-line build.rs). Never let docs and CLI drift.

### 6.1 The `mdbook` structure

```text
docs/
├── book.toml
├── src/
│   ├── SUMMARY.md
│   ├── quickstart.md
│   ├── concepts/
│   │   ├── deploy.md
│   │   ├── rollback.md
│   │   ├── secret.md
│   │   ├── backup.md
│   │   └── server.md
│   ├── tutorials/
│   │   ├── fastapi.md
│   │   ├── nextjs.md
│   │   ├── laravel.md
│   │   ├── go.md
│   │   ├── rails.md
│   │   └── astro.md
│   ├── how-tos/
│   │   ├── custom-domain.md
│   │   ├── backup-verify.md
│   │   ├── policy-rule.md
│   │   └── migrate-from-coolify.md
│   ├── reference/
│   │   ├── cli.md           # auto-generated from --help
│   │   ├── api.md           # auto-generated from OpenAPI
│   │   ├── config.md
│   │   └── rego.md
│   ├── operations/
│   │   ├── morning-commands.md
│   │   ├── slos.md
│   │   ├── alerts.md
│   │   └── dr.md
│   └── sovereignty/
│       ├── 10-point-test.md
│       ├── bsi-c5-2026.md
│       ├── eucs-substantial.md
│       └── vendor-disappear.md
```

### 6.2 The auto-generation script

```bash
#!/usr/bin/env bash
# scripts/gen-docs.sh
set -euo pipefail

# 1. Generate the CLI reference from --help
mkdir -p docs/src/reference/cli
sovereign man --output docs/src/reference/cli/

# 2. Generate the API reference from OpenAPI
sovereign openapi > docs/src/reference/api/openapi.yaml

# 3. Generate the config reference from a schemars-derived JSON schema
sovereign config-schema > docs/src/reference/config/schema.json

# 4. Build the book
mdbook build

# 5. Check for stale docs
# (a CI job fails if the committed docs differ from the generated docs)
```

---

## 7. Pricing

### 7.1 The three tiers

| Tier | Price | Includes |
|---|---|---|
| **Community** | Free (Apache 2.0) | Unlimited servers, all features, no support |
| **Pro** | €19/server/month | Managed updates, email support, EU-region updates, official deb/rpm repos, 1-day SLA on security advisories |
| **Enterprise** | Custom (starts €5k/year) | EUCS Substantial package, BSI C5 mapping, on-prem, 24/7 SLA, dedicated CSM, training |

### 7.2 Why per-server pricing

- **Aligns with EU procurement** (per-node, not per-seat). Maps to IPCEI-CIS, EUCS, BSI C5.
- **Predictable for the buyer.** A 1-server Pro customer pays €19/mo. A 10-server customer pays €190/mo. No surprise.
- **Avoids the Vercel mistake.** No per-bandwidth, no per-deploy, no per-seat. (See [`negative-prompt.md` §3.3](#).)

### 7.3 The free tier is the funnel

Plausible has 50k+ paying sites on a 4-person team; they don't gate features. Coolify has 56k stars with no feature paywall. **The product should match.**

### 7.4 The Pro tier is for managed updates + support, not features

The Pro tier is for *managed updates + support*, not for *features*. The features are all Apache 2.0. The Pro tier is "we run the update channel, you don't have to."

### 7.5 The Enterprise tier is for sovereignty procurement

The Enterprise tier is for the BSI C5 / EUCS / IPCEI-CIS buyer. They pay for the *certification*, not for the *features*. The features are the same as Pro; the certification is the differentiator.

### 7.6 The "no overage, no bandwidth" public commitment

> "€19/server/month. No surprises. No overage. No bandwidth."

This is published on the website. It is the antidote to Riley Walz's $46k Jmail bill.

---

## 8. Telemetry — what to instrument, what to forbid

### 8.1 What to instrument (opt-in, default off)

| Metric | Why |
|---|---|
| **Time-to-first-deploy** | The headline metric |
| **Deploys per day per user** | Engagement |
| **Rollback rate** | Quality |
| **Mean time to detect** | Operational |
| **Mean time to recover** | Operational |
| **Backup verify rate** | The "did you test your backup?" metric |
| **TUI session length** | TUI stickiness |
| **CLI invocations per day** | Engagement |
| **Docs page → first deploy conversion** | Onboarding funnel |
| **Churn (per 30 days)** | The "are we keeping users?" metric |

### 8.2 What to forbid (always, no exceptions)

- **No PII.** No email, no IP, no hostname (only counts).
- **No content of secrets.** No secret keys, no secret values, no ciphertext.
- **No deploy content.** No image refs, no commit SHAs, no diffs.
- **No app names.** Only counts (`user has 3 apps`, not `user has api, web, worker`).
- **No audit log content.** Only counts.

### 8.3 The opt-in pattern

```bash
# Opt in
sovereign telemetry enable

# Opt out (default)
sovereign telemetry disable

# Show status
sovereign telemetry status
```

The binary starts with telemetry **off**. Users opt in explicitly. The opt-in is one command, not buried in a settings page.

---

## 9. The 3 onboarding personas (revisited)

| Persona | First session goal | Killer feature | Drop-off risk |
|---|---|---|---|
| **Mira (solo founder)** | Deploy 1 app, see it on the internet | `sovereign init` + `sovereign deploy` (5 min) | If deploy takes > 5 min, she's gone |
| **Karim (agency)** | Deploy 1 client's app | The scanner + multi-app in 1 TUI | If scanner doesn't detect her stack, she's back to CapRover |
| **Lin (startup infra)** | Replace Heroku | `sovereign policy check` + `sovereign audit export` | If the policy engine is opaque, she can't sell it to her CTO |
| **Sara (EU public sector)** | Pass BSI C5 procurement | `sovereign compliance map` + `sovereign sovereignty check` | If sovereignty is theatre, she's gone |
| **Alex (vibe coder)** | Deploy from Cursor via Claude Code | `sovereign --json` everywhere | If the agent can't drive it, he's back to Vercel |

---

## 10. The "3am test" for UX

Before shipping any UX change, ask: *could a user who has never seen this product before, at 3am, in an incident, figure out what to do?*

- `--help` reads like documentation, not a wall of flags.
- Errors explain the cause, the effect, and the fix.
- The TUI is panic-safe and the terminal is restored on any exit.
- Documentation is auto-generated, never stale.
- Every destructive command has `--dry-run`.
- Every command has a JSON output for agents.
- The "5-minute moment" works on a fresh VM, offline, with no docs.

If the answer to any of these is "no," the change doesn't ship.

---

## 11. What the end user actually receives (the install → upgrade → uninstall story)

The CLI is the product, but the *product* is the artifact on the operator's disk. This section is the **end-user contract** — the exact bits the user downloads, installs, runs, upgrades, and uninstalls. Every choice below is grounded in the persona-cto.md §2.5 ("Delegate entirely, do not build an IdP / log DB / observability backend") and the user-pain-research.md §6 (the LGTM stack is a part-time platform engineer for a 1-person team — we ship the bits that don't need an operator to babysit).

### 11.1 The install story (3 paths, all < 60s)

**Path 1 — the one-liner (the default for 95% of users):**

```bash
curl -sSf sovereignruntime.dev/install.sh | sh
```

What the script does (full source is in `crates/sovereign/src/scripts/install.sh`, ~120 lines of bash, audited on every release):

1. **Detects the platform** — `uname -m` → x86_64 / aarch64; `ldd --version` → musl vs glibc (falls back to glibc binary if musl is unavailable, e.g. older CentOS).
2. **Downloads the binary** from `https://releases.sovereignruntime.dev/<version>/sovereign-<version>-<target>.tar.xz`. The URL is the SHA-256-pinned release artifact; the script aborts if the checksum doesn't match.
3. **Verifies the cosign signature** (the G14 release process signs every binary). The script fetches the public key from `sovereignruntime.dev/.well-known/cosign.pub` and aborts if the signature is invalid or the key is rotated without a corresponding `security@` advisory.
4. **Installs to `/usr/local/bin/sovereign`** (mode 0755). Skips if `/usr/local/bin` is read-only; falls back to `~/.local/bin/sovereign` and prints a one-line PATH instruction.
5. **Creates the `sovereign` system user and group** (UID/GID 700) — runs the binary as a non-root user. The install script uses `useradd` / `groupadd` on systemd systems, `pw` on FreeBSD (V1+).
6. **Creates the data directory** `/var/lib/sovereign` (mode 0750, owner `sovereign:sovereign`) and the config directory `/etc/sovereign` (mode 0750).
7. **Installs the systemd unit** (or `launchd` plist on macOS V1+, or `rc.d` script on FreeBSD V1+). The unit is `Type=notify`, `Restart=on-failure`, `LimitNOFILE=65536`, `MemoryMax=512M` (configurable).
8. **Starts the service** and waits for the `READY=1` notification. The install script exits 0 only when `sovereign status` returns 0.
9. **Prints the 5-line "what next"** from §1.1 (the post-onboarding prompt).

**The install script is reproducible** — `install.sh --version 1.0.3` installs exactly that version. The latest stable is the default. Pre-releases are available at `releases.sovereignruntime.dev/pre/<version>/`.

**Path 2 — the package (the path for ops teams that have a config-management standard):**

```bash
# Debian / Ubuntu (V1+)
sudo apt install sovereign    # from packages.sovereignruntime.dev

# RHEL / Rocky / Alma (V1+)
sudo dnf install sovereign    # from packages.sovereignruntime.dev

# Alpine (V1+)
sudo apk add sovereign        # from packages.sovereignruntime.dev/alpine

# Arch (community, V1.5+)
yay -S sovereign-bin

# Homebrew (macOS V1+, dev only)
brew install sovereign
```

The package contains:
- `/usr/bin/sovereign` — the static musl binary, mode 0755.
- `/usr/lib/systemd/system/sovereign.service` — the systemd unit (Debian / RHEL).
- `/etc/sovereign/sovereign.toml` — the default config, mode 0640.
- `/usr/share/doc/sovereign/changelog.Debian.gz` or `%changelog` — the release notes.
- `/usr/share/man/man1/sovereign.1.gz` — the man page (generated by `clap_mangen` at build time).
- A `postinst` script that creates the `sovereign` user, the data dir, and runs `systemctl daemon-reload && systemctl enable --now sovereign`. The postinst is **idempotent** — re-installing does not wipe data.

The package repo is signed with a per-release GPG key; the public key is at `packages.sovereignruntime.dev/keyring.gpg` and is shipped in the `sovereign-keyring` package. Updates flow through the standard `apt upgrade` / `dnf upgrade` pipeline.

**Path 3 — the container image (the path for Kubernetes / Nomad / Docker Swarm shops that want to run the control plane in a container):**

```bash
docker run -d --name sovereign \
  -v sovereign-data:/var/lib/sovereign \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -p 80:80 -p 443:443 \
  ghcr.io/sovereignruntime/sovereign:1.0.3
```

The image is:
- **Multi-stage** — build stage uses `rust:1.85-slim` with `cargo-chef` for layer caching; runtime stage is `gcr.io/distroless/cc-debian12` (no shell, no package manager, ~20 MB base).
- **Rootless** — runs as UID 700 (`sovereign` user, created in the Dockerfile).
- **Pinned** — `:1.0.3` is immutable; `:1.0` tracks the latest 1.0.x patch; `:latest` tracks the latest stable. The `:latest` tag is **not** the default in any deploy script; the user opts in explicitly.
- **Cosign-signed** — `cosign verify --key cosign.pub ghcr.io/sovereignruntime/sovereign:1.0.3` works out of the box.
- **SBOM-attached** — `ghcr.io/sovereignruntime/sovereign:1.0.3.sbom` is a CycloneDX SBOM; the SPDX variant is in `/usr/local/share/sovereign/sbom.spdx.json` inside the image.
- **Reproducible** — `docker buildx build --provenance=true --sbom=true` produces a SLSA Level 3 provenance attestation.

**The image's only privilege is `--pid=host` (optional, for systemd-in-docker) and the Docker socket mount.** No `--privileged`. No host network. No host PID by default. The container is a single-process deployment; the operator can run it on a 512 MB VPS, on a 16 GB bare-metal box, or in a Kubernetes pod with a `hostPath` volume for the data dir.

### 11.2 The upgrade story (zero-downtime, reversible)

The binary supports three upgrade channels:

1. **Stable (`stable`)** — `sovereign update` downloads the latest patch release (e.g. 1.0.3 → 1.0.4). Patch releases are guaranteed backwards-compatible; no schema migrations, no config changes. The upgrade is `systemctl restart sovereign` — the systemd `Restart=on-failure` handles a bad binary by rolling back to the previous one (the F10 self-update with rollback).
2. **Minor (`minor`)** — `sovereign update --channel minor` downloads the latest minor release (e.g. 1.0.3 → 1.1.0). Minor releases may include schema migrations, new config keys (with defaults), new CLI subcommands. The binary runs a pre-flight check (`sovereign doctor --level standard`) and refuses to upgrade if any check fails. The migration is a forward-only SQLite migration (V0 §3); the previous binary is kept at `/usr/local/bin/sovereign.previous` for 7 days as a rollback target.
3. **Pro / Enterprise (signed channel)** — `sovereign update --channel pro` (or `--channel enterprise`) downloads from a separate release stream signed with a channel-specific cosign key. The Pro channel is for paying customers; the Enterprise channel is for SOC 2 / ISO 27001 customers who require a 30-day embargo window between the public release and the production deployment. The 30-day window is the *only* operational difference between Community and Enterprise releases — the binary is byte-identical, the source is byte-identical, the SBOM is byte-identical.

**The upgrade is always reversible.** `sovereign update --rollback` reverts to `/usr/local/bin/sovereign.previous` and restarts. The previous version is also written to the audit log (`kind: "self_update"`, with `from_version`, `to_version`, `cosign_bundle_digest`). If the new binary fails to start (e.g. the systemd `Type=notify` times out), the systemd unit's `Restart=on-failure` rolls back automatically.

**The upgrade is always opt-in.** There is no auto-update daemon. The Pro tier ships an opt-in auto-update timer (`sovereign update enable --schedule "daily 03:00"`) that runs `sovereign update --channel pro` as a systemd timer. The Enterprise tier ships the same with a default 30-day delay. The operator can disable auto-update at any time.

### 11.3 The uninstall story (clean, complete, data-preserving)

`sovereign uninstall` (built into the binary, not a separate script) does the following, in order:

1. Stops the systemd service (`systemctl stop sovereign`). Refuses to run if the service is the only thing serving production traffic and `--force` is not passed (a 5-second confirm prompt).
2. Removes the systemd unit (`systemctl disable sovereign && rm /usr/lib/systemd/system/sovereign.service`).
3. Removes the binary (`rm /usr/local/bin/sovereign /usr/local/bin/sovereign.previous`).
4. Removes the config directory (`rm -rf /etc/sovereign`) **only if `--purge-config` is passed**. The default is to leave the config in place, so a re-install picks up the same apps, secrets, domains, and audit log.
5. Removes the data directory (`rm -rf /var/lib/sovereign`) **only if `--purge-data` is passed**. The default is to leave it.
6. Removes the `sovereign` user and group (`userdel sovereign && groupdel sovereign`).
7. Prints a one-line summary: "Uninstalled. Config preserved at `/etc/sovereign`. Data preserved at `/var/lib/sovereign`. To remove them: `rm -rf /etc/sovereign /var/lib/sovereign`."

**The uninstall never touches `/var/log/sovereign/`, the journald logs, the user's apps, the Docker images, the SQLite databases owned by the apps, or the Caddy / Nginx configs.** Those are the operator's data; Sovereign is a tool that manages them, not owns them.

### 11.4 The end-user artifact checklist (what ships in the box)

| Artifact | V0 | V1 | V1.5 | V2 |
|---|---|---|---|---|
| Static musl binary (10-25 MB) | ✓ | ✓ | ✓ | ✓ |
| cosign signature + SLSA provenance | ✓ | ✓ | ✓ | ✓ |
| SPDX + CycloneDX SBOM | ✓ | ✓ | ✓ | ✓ |
| `install.sh` (one-liner) | ✓ | ✓ | ✓ | ✓ |
| `apt` / `dnf` / `apk` packages | — | ✓ | ✓ | ✓ |
| `ghcr.io/sovereignruntime/sovereign` container image | — | ✓ | ✓ | ✓ |
| Homebrew formula (macOS dev) | — | ✓ | ✓ | ✓ |
| `clap_mangen` man page | ✓ | ✓ | ✓ | ✓ |
| Shell completions (bash / zsh / fish / nushell) | ✓ | ✓ | ✓ | ✓ |
| `default.toml` config with sane defaults | ✓ | ✓ | ✓ | ✓ |
| systemd unit (with `Type=notify`, `Restart=on-failure`) | ✓ | ✓ | ✓ | ✓ |
| `sovereign update` (self-update with rollback) | ✓ | ✓ | ✓ | ✓ |
| `sovereign uninstall` (clean, complete, opt-in) | ✓ | ✓ | ✓ | ✓ |
| Per-release GPG keyring for packages | — | ✓ | ✓ | ✓ |
| Multi-arch support (x86_64, aarch64) | x86_64 only | + aarch64 | + aarch64 | + aarch64 |
| Reproducible build verification (CI gate) | — | ✓ | ✓ | ✓ |
| Public security advisory feed (GHSA) | ✓ | ✓ | ✓ | ✓ |
| `security@sovereignruntime.dev` mailbox | ✓ | ✓ | ✓ | ✓ |

### 11.5 The "what does the end user pay for?" (the no-bluff list)

If a 1-person team is the target, the answer to "do I need to pay for anything?" is **no**. The Apache 2.0 binary, the package repos, the container image, the SBOM, the cosign signatures, the shell completions, the man page, the systemd unit, the self-update, the uninstall, the audit log, the backup, the doctor, the policy engine, the compliance scan — all free, all open source, all without a "Pro feature" gate. The Pro tier is for *managed updates + support* (the operator pays €19/server/mo to not babysit the update channel and get a 1-day SLA on security advisories). The Enterprise tier is for the *audit + procurement* layer (SOC 2 mapping, BSI C5 mapping, EUCS Substantial controls, on-prem install, custom DPA, 24/7 SLA, dedicated support engineer). The features are the same; the *service* is different.

This is the Plausible model. This is the Cal.com model. This is what the persona-cto.md §5.3 says to do. The full pricing is in §7.1; the full enterprise contract is in `enterprise-readiness.md` (the new doc this turn adds).

---

**Next: read [`sovereignty-and-governance.md`](./sovereignty-and-governance.md) for the 10-point sovereignty test, OPA/Rego, and ML scorer, and [`enterprise-readiness.md`](./enterprise-readiness.md) for the SOC 2 / ISO 27001 / BSI C5 / EUCS mapping and the support tier SLAs.**
