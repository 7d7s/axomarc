# Sovereign Application Runtime

**Single-binary, self-hosted, CLI-first deployment runtime.** Apache 2.0, EU-sovereign, 80-150 MB RSS, fits in a 512 MB VPS.

```bash
curl -sSf sovereignruntime.dev/install.sh | sh   # 60 seconds
sovereign deploy                                   # 5 minutes to a live URL
```

## Status

**V0 sprint (weeks 1-6).** The 10 features in [`docs/phase-00-mvp.md`](./docs/phase-00-mvp.md). This repository is the source of truth for the implementation.

| Feature | Status |
|---|---|
| F1 — Single Rust static binary (10-25 MB) | ✅ scaffolded (F1 §) |
| F2 — CLI skeleton (clap 4.6, `--json`, `--dry-run`) | ✅ (10 subcommands, completions, man, exit codes 0-5) |
| F3 — SQLite + 7 core tables + append-only audit | ✅ (50 tests, see `crates/sovereign-storage-sqlite/`) |
| F4 — `sovereign deploy` (git push to live URL) | ✅ (RuntimePort + DockerRuntime + deploy use case + CLI wired) |
| F5 — One-command rollback | ✅ (`sovereign rollback <app> [--to=<id>] [--list] [--limit=N] [--json]`; writes `sovereign.lock` receipt on every healthy deploy; `--no-lock` opt-out) |
| F6 — Caddy auto-TLS (HTTP-01 ACME, auto-renew) | ✅ (`CaddyProxy` adapter; `tls internal` in V0; `sovereign deploy` and `rollback` push routes; `sovereign domain add <host> --app <app>` for custom hostnames) |
| F7 — Encrypted secret store (age + Argon2) | ✅ |
| F8 — Backup + health check + auto-rollback | ✅ |
| F9 — `sovereign doctor` — basic diagnostic | ✅ |
| F10 — `sovereign update` — self-update with rollback | ✅ (`sovereign update check/apply/rollback/history`; manifest at `https://releases.sovereignruntime.dev`; `update_history` table; SHA-256 verified; atomic swap) |
| F4.6 — `sovereign init` + `sovereign login` (V0 single-tenant) | ✅ (`init` auto-detects FastAPI / Next.js / Express / Go / Rails / Laravel / Astro / Static / Generic, writes `app.yaml`; `login` provisions / loads the local age master key, prints the Bech32 public key; passphrase read from `SOVEREIGN_PASSPHRASE` or stdin) |

## Quickstart

```bash
# Build the musl static binary (Linux host)
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl

# Verify
file target/x86_64-unknown-linux-musl/release/sovereign
target/x86_64-unknown-linux-musl/release/sovereign --version
# → sovereign 0.1.0

# V0 single-tenant bootstrap (no daemon, no OIDC)
cd /srv/myapp
sovereign init                              # auto-detect framework, write app.yaml
sovereign login                             # generate the age master key, print public key
sovereign deploy                            # build, run, route, emit sovereign.lock

# Or in one shot on a fresh Hetzner CX22 (Step 9 demo, F1)
curl -sSf https://install.sovereignruntime.dev | sh
cd myapp
sovereign init && sovereign login && sovereign deploy
```

On Windows (this dev box):

```bash
cargo build --release
target\release\sovereign.exe --version
target\release\sovereign.exe init
target\release\sovereign.exe login --no-input  # requires SOVEREIGN_PASSPHRASE
```

## Workspace layout

```
crates/
  sovereign/                  # the binary (clap, tokio, mimalloc/jemalloc)
  sovereign-core/             # use cases + domain types (no adapters)
  sovereign-runtime-docker/   # bollard (Docker REST API) — F4
  sovereign-proxy-caddy/      # Caddy admin API — F6
  sovereign-secrets-age/      # age + Argon2 envelope encryption — F7
  sovereign-storage-sqlite/   # sqlx + forward-only migrations — F3
  sovereign-backup/           # snapshot + S3 offsite — F8
  sovereign-notify/           # 10 alert channels — V0.5
  sovereign-observability/    # tracing + Prometheus /metrics — F1+V1.5
  sovereign-proto/            # REST/JSON DTOs — F2
  sovereign/src/scripts/
    install.sh                # the 9-step one-liner
    sovereign.service         # the systemd unit (hardened)
    sovereign.toml            # the default config
docs/                         # 15 doc files (architecture, phases, ops, ...)
deny.toml                     # the dep policy
.cargo/config.toml            # the musl/glibc build flags
.github/workflows/ci.yml      # the CI matrix
```

## The invariants (non-negotiable)

1. **One operator, 5 years.** `sovereign doctor` is the on-call's first command.
2. **A product in 6 months.** Scope is the moat.
3. **Sovereign by construction.** EU-only defaults, no US sub-processors, vendor-disappear-safe.
4. **The CLI is the product.** TUI is the daily-driver. Web UI is V2 opt-in.

## Phase 0 close-out scripts

Two operator-runnable scripts ship in `scripts/`:

- `scripts/cx22-quickstart.sh` — the 5-command paste-and-run for a Hetzner CX22. The recipe lives in [`docs/operations/cx22-verify.md`](./docs/operations/cx22-verify.md); the script is the headlinable form. Time budget: 90s on a CX22, 5min for a first-time operator.
- `scripts/verify-distros.sh` — local equivalent of the CI `verify-distros` job. Runs the musl binary in `ubuntu:22.04`, `debian:12`, `alpine:3.20` Docker images and asserts `--version` + `--help` work without any install step.

## Release pipeline

Tag-driven GitHub Actions workflow at [`.github/workflows/release.yml`](./.github/workflows/release.yml): tag push → musl build (x86_64 + aarch64) → cosign keyless signing → CycloneDX SBOM → manifest.json → GitHub release → CDN mirror (Hetzner Storage Box). The install script and `sovereign update check` consume the same artifact layout. Full recipe: [`docs/operations/release-process.md`](./docs/operations/release-process.md). Changelog: [`CHANGELOG.md`](./CHANGELOG.md).

Full spec: [`docs/README.md`](./docs/README.md). Decision log: [`docs/decision-records.md`](./docs/decision-records.md). Anti-patterns: [`docs/negative-prompt.md`](./docs/negative-prompt.md). CX22 verification recipe: [`docs/operations/cx22-verify.md`](./docs/operations/cx22-verify.md). Release process: [`docs/operations/release-process.md`](./docs/operations/release-process.md).

## License

Apache 2.0, unmodified, irrevocable. The license will not change. The source is the contract. See [LICENSE](./LICENSE).
