# Phase 0 — The Engine (MVP)

**Timeline:** Weeks 1-6
**Goal:** Ship a working engine. 1 developer can deploy 1 real app, roll it back, diagnose failures, and update the binary. Single binary, single host, SQLite, no agent, no web UI, no TUI. Doctor is shipped at the **basic** level in this phase.
**Audience:** The lead engineer and the first 5 contributors.
**Definition of Done:** All 10 features shipped, all tests pass, the `5-command quickstart` works on a fresh Hetzner CX22, `sovereign doctor --level basic` returns 0 issues.

---

## 0. The 5-command quickstart (the spec we ship against)

```text
$ curl -sSf sovereignruntime.dev/install.sh | sh       # 1 — install (10s)
$ sovereign init                                        # 2 — detect framework, write app.yaml (5s)
$ sovereign login                                       # 3 — device-code flow, browser opens (15s)
$ sovereign deploy                                      # 4 — build, TLS, URL (90s)
$ open https://<id>.srvr.so                             # 5 — you are live
```

**Total target:** 2 min 30 sec end-to-end on a fresh CX22. **The phase is not done until this works.**

---

## 1. The 10 features in order

```text
Week 1-2: F1 (binary) + F2 (CLI skeleton) + F3 (SQLite + migrations)
Week 3-4: F4 (git push deploy) + F5 (rollback) + F6 (Caddy auto-TLS)
Week 5:   F7 (encrypted secrets) + F8a (Postgres backup)
Week 6:   F8b (health check + auto-rollback) + F9 (sovereign doctor basic) + F10 (self-update) + Show HN prep
```

Each feature has: **Goal → Why it matters → Sub-tasks → Code stubs → Tests → Acceptance criteria → Definition of done**.

---

## F1. Single Rust static binary (10-25 MB)

**Goal:** A statically-linked musl binary that runs on any Linux x86_64 without dependencies, with a working `--version`, `--help`, and a no-op `sovereign` subcommand.

**Why it matters:** The first 5 minutes of the user experience are governed by how easy it is to install. A 30 MB binary that needs glibc 2.32+ is a 5-minute debugging session on an older distro. A 12 MB static musl binary is `chmod +x && ./sovereign --help` and done.

### Sub-tasks

1. **Initialize the Cargo workspace** with the 10 V0 crates listed in [`tech-stack.md` §1](#).
   ```bash
   cargo new --bin crates/sovereign
   cargo new --lib crates/sovereign-core
   cargo new --lib crates/sovereign-runtime-docker
   cargo new --lib crates/sovereign-proxy-caddy
   cargo new --lib crates/sovereign-secrets-age
   cargo new --lib crates/sovereign-storage-sqlite
   cargo new --lib crates/sovereign-backup
   cargo new --lib crates/sovereign-notify
   cargo new --lib crates/sovereign-observability
   cargo new --lib crates/sovereign-proto
   ```
2. **Wire the workspace** `Cargo.toml` with the version pins from [`tech-stack.md` §5](#). Verify `cargo build` works.
3. **Add `.cargo/config.toml`** with the musl target settings from [`tech-stack.md` §6](#).
4. **Add the global allocator** in `crates/sovereign/src/main.rs` per [`tech-stack.md` §3](#).
5. **Set the release profile** in workspace `Cargo.toml` per [`tech-stack.md` §2](#).
6. **Verify musl build**:
   ```bash
   rustup target add x86_64-unknown-linux-musl
   cargo build --release --target x86_64-unknown-linux-musl
   ls -lh target/x86_64-unknown-linux-musl/release/sovereign
   file target/x86_64-unknown-linux-musl/release/sovereign
   # Output: ELF 64-bit LSB executable, x86-64, version 1 (SYSV), statically linked, ...
   ```
7. **Verify the binary runs on a stock Ubuntu 22.04** Docker image:
   ```bash
   docker run --rm -v $(pwd)/target:/target ubuntu:22.04 /target/x86_64-unknown-linux-musl/release/sovereign --version
   # Output: sovereign 0.1.0
   ```
8. **Add `Cargo.lock` to git**, add `.gitignore` for `target/`, `*.swp`, `*.bak`, `Cargo.lock.bak`.

### Code stub

```rust
// crates/sovereign/src/main.rs
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about = "Sovereign Application Runtime", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// Print version and exit
    Version,
}

#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(not(target_env = "musl"))]
#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Some(Cmd::Version) => println!("sovereign {}", env!("CARGO_PKG_VERSION")),
        None => println!("sovereign {} — run `sovereign --help` to get started", env!("CARGO_PKG_VERSION")),
    }
    Ok(())
}
```

### Tests

- `cargo build --release --target x86_64-unknown-linux-musl` succeeds and produces a binary ≤ 25 MB.
- The binary runs on `ubuntu:22.04`, `debian:12`, `alpine:3.20` Docker images with zero additional dependencies.
- `sovereign --version` prints the correct version (from `Cargo.toml`).
- `sovereign --help` prints the full help text, with no panic, no warning.

### Acceptance criteria

- [ ] Binary ≤ 25 MB
- [ ] Statically linked (no `GLIBC_X.Y` requirement)
- [ ] Runs on at least 3 distros (Ubuntu 22.04, Debian 12, Alpine 3.20)
- [ ] `--version` and `--help` work
- [ ] CI builds and uploads the binary as a release artifact

### Definition of done

The `install.sh` script can be pointed at the binary's URL, download it, place it in `/usr/local/bin/sovereign`, and `sovereign --version` works on a fresh VM.

---

## F2. CLI skeleton (clap 4.6, subcommands, `--json`, `--dry-run`)

**Goal:** The full top-level command tree is defined in `clap`, with subcommand stubs that print "not yet implemented" and return exit code 0. `--json` is detected via `std::io::IsTerminal`. `--dry-run` is wired but no-op.

**Why it matters:** The CLI is the product surface. Defining the command tree first lets us ship the help text, the man pages, the shell completions, and the OpenAPI spec from day 1. Every later feature is just filling in a subcommand.

### Sub-tasks

1. **Define the top-level `Cli` struct** in `crates/sovereign/src/cli.rs` with the global flags:
   - `--url <URL>` (env: `SOVEREIGN_URL`, default: `http://127.0.0.1:7878`)
   - `--token <TOKEN>` (env: `SOVEREIGN_TOKEN`)
   - `--format <TEXT|JSON>` (auto-detected by default)
   - `--no-color`
2. **Define the subcommand tree** per the V1 endpoint catalog in [`architecture.md` §4.1](#) — for V0, only these are non-stub:
   - `init`, `login`, `deploy`, `rollback`, `logs`, `status`, `secret set`, `secret list`, `backup create`, `backup list`
3. **Implement the global `Output` formatter** that prints `text` for humans, `JSON` for agents, color the right way, respect `NO_COLOR`.
4. **Implement `--dry-run`** as a no-op for now: every subcommand accepts it, prints "would do X", exits 0.
5. **Wire shell completions** for bash, zsh, fish, nushell, powershell via `clap_complete` (build-time generated, not runtime).
6. **Wire man pages** via `clap_mangen` (build-time generated).
7. **Add `after_long_help`** to the top-level CLI with 3-5 example commands (the "what to try first" hint).
8. **Add exit codes** (0 success, 1 generic, 2 usage, 3 partial, 4 upstream).

### Code stub

```rust
// crates/sovereign/src/cli.rs
use clap::{Parser, Subcommand, ValueEnum};
use std::io::IsTerminal;

#[derive(Parser, Debug)]
#[command(
    name = "sovereign",
    version,
    about = "Sovereign Application Runtime",
    long_about = "Run your own cloud. Single binary. No DevOps team.",
    after_long_help = "Quickstart:\n  sovereign init\n  sovereign login\n  sovereign deploy\n  sovereign logs\n  sovereign tui"
)]
pub struct Cli {
    /// Control plane URL
    #[arg(long, env = "SOVEREIGN_URL", global = true, default_value = "http://127.0.0.1:7878")]
    pub url: String,

    /// Auth token
    #[arg(long, env = "SOVEREIGN_TOKEN", global = true)]
    pub token: Option<String>,

    /// Output format (auto-detected by default)
    #[arg(long, value_enum, global = true, default_value_t = OutputFormat::Auto)]
    pub format: OutputFormat,

    /// Disable color output
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Show what would be done, but do not do it
    #[arg(long, global = true)]
    pub dry_run: bool,

    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Initialize a new app in the current directory
    Init {
        #[arg(long, value_enum, default_value_t = Framework::Auto)]
        framework: Framework,
    },
    /// Log in to the control plane
    Login,
    /// Deploy an app
    Deploy {
        #[arg(long)]
        app: String,
        #[arg(long)]
        image: Option<String>,
        #[arg(long, value_enum, default_value_t = Strategy::BlueGreen)]
        strategy: Strategy,
        #[arg(long)]
        wait: bool,
    },
    /// Roll back an app to a previous deployment
    Rollback {
        app: String,
        #[arg(long)]
        to: Option<String>,
    },
    /// Stream logs
    Logs {
        app: String,
        #[arg(long, default_value_t = 100)]
        tail: usize,
        #[arg(long)]
        follow: bool,
    },
    /// Show app status
    Status {
        #[arg(long)]
        app: Option<String>,
    },
    /// Open the interactive dashboard
    #[cfg(feature = "tui")]
    Tui,
    /// Manage secrets
    Secret {
        #[command(subcommand)]
        cmd: SecretCmd,
    },
    /// Manage backups
    Backup {
        #[command(subcommand)]
        cmd: BackupCmd,
    },
    // ... (more stubs for V1 features)
}

#[derive(ValueEnum, Clone, Debug)]
pub enum OutputFormat { Auto, Text, Json }

#[derive(ValueEnum, Clone, Debug)]
pub enum Framework { Auto, Fastapi, Nextjs, Laravel, Go, Rails, Astro }

#[derive(ValueEnum, Clone, Debug)]
pub enum Strategy { Recreate, Rolling, BlueGreen }

#[derive(Subcommand, Debug)]
pub enum SecretCmd {
    /// Set a secret (reads value from stdin)
    Set {
        app: String,
        key: String,
    },
    /// List secret keys (never values)
    List {
        app: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum BackupCmd {
    Create { #[arg(long)] app: String },
    List,
}

impl Cli {
    pub fn effective_format(&self) -> OutputFormat {
        match self.format {
            OutputFormat::Auto => {
                if std::io::stdout().is_terminal() { OutputFormat::Text } else { OutputFormat::Json }
            }
            f => f,
        }
    }
}
```

### Tests

- `cargo run -- --help` prints the full help text including `after_long_help`.
- `cargo run -- deploy --help` prints deploy-specific help.
- `cargo run -- --json deploy --app foo` (with `SOVEREIGN_URL` set to a no-op server) returns a JSON error envelope.
- `cargo run -- --dry-run deploy --app foo` prints "would do X" and exits 0.
- `NO_COLOR=1 cargo run -- --no-color status` produces no ANSI escape codes.
- `cargo run -- completions bash > /tmp/c.bash && bash -n /tmp/c.bash` succeeds.
- `cargo run -- man > /tmp/sovereign.1 && man --warnings -l /tmp/sovereign.1 >/dev/null` succeeds.

### Acceptance criteria

- [ ] All 60+ subcommands parse correctly
- [ ] `--help` is a teaching surface (3-5 tips + 3-5 example commands)
- [ ] `--json` is auto-detected via `IsTerminal`
- [ ] `--dry-run` accepted on every mutating subcommand
- [ ] Shell completions build for bash, zsh, fish, nushell, powershell
- [ ] Man pages build and validate

### Definition of done

`sovereign --help` is a 200-line reference that reads like documentation. `sovereign completions install` works. `sovereign man /usr/local/share/man/man1/` works.

---

## F3. SQLite + migrations + 7 core tables + append-only audit

**Goal:** `sovereign-core` defines the domain types and ports. The `sovereign-storage-sqlite` adapter implements `StoragePort` against SQLite with WAL mode, 7 core tables, 2 migrations, and the append-only audit log with triggers.

**Why it matters:** The state is the platform. Get this wrong and you rewrite everything. Get it right and every later feature is a use case that calls a port.

### Sub-tasks

1. **Define the domain types** in `crates/sovereign-core/src/domain/`:
   - `app.rs`: `App`, `AppId(Uuid)`, `NewApp`, `AppUpdate`
   - `deployment.rs`: `Deployment`, `DeploymentId(Uuid)`, `DeploymentStatus` (state machine)
   - `domain.rs`: `Domain`, `DomainId(Uuid)`, `TlsStatus`
   - `secret.rs`: `Secret`, `SecretId(Uuid)`, `SecretKey(String)`
   - `server.rs`: `Server`, `ServerId(Uuid)`, `ServerRole`, `ServerStatus`
   - `backup.rs`: `Backup`, `BackupId(Uuid)`, `BackupStatus`
   - `user.rs`: `User`, `UserId(Uuid)`, `UserRole`
   - `audit.rs`: `AuditEvent`, `AuditKind` (enum), `AuditActor`
2. **Define the ports** in `crates/sovereign-core/src/ports/`:
   - `storage.rs`: `StoragePort` (one big trait, with sub-traits if it grows)
   - `runtime.rs`: `RuntimePort` (`deploy`, `stop`, `logs`, `health`)
   - `proxy.rs`: `ProxyPort` (`add_route`, `remove_route`, `reload`)
   - `secrets.rs`: `SecretsPort` (`encrypt`, `decrypt`, `rotate`)
   - `backup.rs`: `BackupPort` (`snapshot`, `restore`, `verify`)
   - `audit.rs`: `AuditPort` (`append`, `query`)
3. **Write migration `0001_init.sql`** with the 7 core tables from [`architecture.md` §2.1](#). Add a uniqueness test in `migrations/0001_init_test.sql` (sqlx migration test pattern).
4. **Write migration `0002_audit.sql`** with `audit_event` and the `audit_no_update` / `audit_no_delete` triggers.
5. **Implement `SqliteState`** in `crates/sovereign-storage-sqlite/`:
   - Open the DB with `sqlx::SqlitePoolOptions::new().max_connections(1).connect_with(...)` — single writer, multiple readers.
   - Enable WAL mode: `PRAGMA journal_mode = WAL`.
   - Set `PRAGMA synchronous = NORMAL` (faster, still safe with WAL).
   - Run migrations on first open: `sqlx::migrate!()`.
   - Implement all 7 table CRUDs as async methods.
   - Implement the audit append + query.
6. **Define the error types** in `crates/sovereign-core/src/error.rs`:
   - `AppError::NotFound`, `Conflict`, `Validation`, `Upstream`, `Internal`, `Auth`
   - Implement `From<sqlx::Error>` for `AppError`.
7. **Add `sqlx::query!` compile-time checks** for every query. Set `DATABASE_URL` env var in dev so `cargo check` works.
8. **Write integration tests** in `crates/sovereign-storage-sqlite/tests/`:
   - Insert app, list apps, update app (with version check), get app, delete app (soft)
   - Insert deployment, transition through all 7 states (proptest)
   - Append audit event, query audit events, attempt UPDATE/DELETE (must fail)
9. **Write `audit_no_update` and `audit_no_delete` trigger tests** — these are the trust anchors.

### Code stub

```rust
// crates/sovereign-core/src/domain/deployment.rs
use uuid::Uuid;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type, serde::Serialize, serde::Deserialize)]
#[sqlx(transparent)]
#[serde(transparent)]
pub struct DeploymentId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    Pending,
    Building,
    Pushing,
    Starting,
    Healthy,
    Failed,
    RolledBack,
}

impl DeploymentStatus {
    pub fn can_transition_to(self, other: Self) -> bool {
        use DeploymentStatus::*;
        matches!((self, other),
            (Pending, Building) | (Building, Pushing) | (Pushing, Starting) |
            (Starting, Healthy) | (Starting, Failed) |
            (_, Failed) | (Healthy, RolledBack) | (Failed, RolledBack) |
            (RolledBack, Building)
        )
    }
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Deployment {
    pub id: DeploymentId,
    pub app_id: AppId,        // from domain/app.rs
    pub image_ref: String,
    pub strategy: Strategy,
    pub status: DeploymentStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub triggered_by: String,
    pub risk_score: Option<i32>,
    pub policy_decision: Option<serde_json::Value>,
    pub error: Option<String>,
    pub version: i64,
}
```

```rust
// crates/sovereign-core/src/ports/storage.rs
use async_trait::async_trait;
use crate::domain::*;

#[async_trait]
pub trait StoragePort: Send + Sync {
    async fn get_app(&self, id: AppId) -> Result<Option<App>, AppError>;
    async fn list_apps(&self, owner: Option<&str>) -> Result<Vec<App>, AppError>;
    async fn create_app(&self, new: NewApp, actor: &str) -> Result<App, AppError>;
    async fn update_app(&self, id: AppId, update: AppUpdate, expected_version: i64, actor: &str) -> Result<App, AppError>;
    async fn delete_app(&self, id: AppId, actor: &str) -> Result<(), AppError>;

    async fn begin_deployment(&self, new: NewDeployment, actor: &str) -> Result<Deployment, AppError>;
    async fn record_deployment_event(&self, id: DeploymentId, event: DeploymentEvent, actor: &str) -> Result<(), AppError>;
    async fn list_deployments(&self, app: AppId, limit: u32) -> Result<Vec<Deployment>, AppError>;

    async fn append_audit(&self, event: AuditEvent) -> Result<(), AppError>;
    async fn query_audit(&self, q: AuditQuery) -> Result<Vec<AuditEvent>, AppError>;

    // ... secrets, backups, servers
}
```

### Tests

- `cargo test -p sovereign-storage-sqlite` passes (≥ 30 tests).
- A property test verifies that for any (state_a, state_b), `a.can_transition_to(b)` matches the documented state machine.
- A test attempts `UPDATE audit_event SET ...` and asserts `sqlx::Error::Database` is returned with the trigger message.
- A test attempts `DELETE FROM audit_event WHERE ...` and asserts the same.
- `cargo llvm-cov -p sovereign-storage-sqlite` reports ≥ 80% line coverage.
- `cargo bench -p sovereign-storage-sqlite --bench state_transitions` reports < 1µs per transition.

### Acceptance criteria

- [ ] 7 core tables created by migration
- [ ] `audit_event` is append-only (triggers fire)
- [ ] All CRUD methods on `StoragePort` are implemented for `SqliteState`
- [ ] `cargo test` passes
- [ ] `cargo llvm-cov` ≥ 80% on the storage adapter
- [ ] No `unsafe` in `sovereign-core` or `sovereign-storage-sqlite`

### Definition of done

A test that creates an app, deploys it (in-memory), rolls it back, and queries the audit log returns the expected state. The state machine property test passes 100k cases.

---

## F4. `sovereign deploy` — git push to live URL

**Goal:** A `git push` (or `sovereign deploy`) builds the app, runs the container, health-checks it, and returns the live URL. The deploy is atomic (image-tagged, immutable history), and the previous version is still running until the new one is healthy.

**Why it matters:** This is the **5-minute moment**. If this is slow, broken, or surprising, the user churns in 10 minutes. If this is fast and predictable, the user stays for 5 years.

### Sub-tasks

1. **Implement the deploy state machine** in `sovereign-core/src/use_cases/deploy.rs`:
   - `start_deployment(app_id, image_ref, strategy, actor)` — creates `Deployment` row in `Pending` state, appends audit.
   - `mark_building(deployment_id)` — transitions to `Building`, appends audit.
   - `mark_pushing(deployment_id)` — transitions to `Pushing`, appends audit.
   - `mark_starting(deployment_id)` — transitions to `Starting`, appends audit.
   - `mark_healthy(deployment_id)` — transitions to `Healthy`, appends audit, returns the URL.
   - `mark_failed(deployment_id, error)` — transitions to `Failed`, appends audit with error.
2. **Implement `DockerRuntime`** in `sovereign-runtime-docker/`:
   - `connect()` connects to `/var/run/docker.sock` (Unix) or `tcp://docker:2375` (env var).
   - `pull_image(ref)` uses `bollard::Docker::create_image`.
   - `create_container(spec)` uses `create_container`.
   - `start_container(id)` uses `start_container`.
   - `stop_container(id, timeout)` uses `stop_container` with a 30s grace period.
   - `remove_container(id)` uses `remove_container`.
   - `healthcheck(id, path, timeout)` does a TCP probe + HTTP GET on `path`, returns `Ok(())` on 2xx within timeout.
   - `logs(id, tail, follow)` returns a `LogStream` (a `futures::Stream<Item = Result<Bytes>>`).
3. **Implement the `deploy` use case** in `sovereign-core/src/use_cases/deploy.rs`:
   - If `image_ref` is `None` and the app has a `git_repo`, build via BuildKit (out of scope for V0, defer to V1.5).
   - If `image_ref` is `None` and no `git_repo`, error: "either image or git_repo required".
   - If `image_ref` is `Some(_)`, pull the image, create a container, start it, healthcheck it.
   - On healthcheck pass: mark `Healthy`, return the URL.
   - On healthcheck fail: mark `Failed`, stop and remove the container, return the error.
4. **Implement the `health` use case** in `sovereign-core/src/use_cases/health.rs`:
   - TCP probe: 1s timeout, 3 retries, success = port open.
   - HTTP probe: 5s timeout, 3 retries, success = 2xx.
   - Returns `HealthResult { ok: bool, latency_ms: u64, error: Option<String> }`.
5. **Wire the `deploy` CLI subcommand** to the use case:
   - `sovereign deploy --app api --image=ghcr.io/me/api:v1` → pulls, runs, health-checks, prints URL.
   - `sovereign deploy --app api --strategy bluegreen` (default) → keeps old version running until new is healthy, then traffic-shifts.
   - `sovereign deploy --app api --strategy recreate` → stops old, starts new.
   - `sovereign deploy --app api --wait` → blocks until healthy, exits 0/1.
6. **Wire the `sovereign init` framework scanner** (simplified for V0):
   - Detects `pyproject.toml` + `fastapi` → emits `app.yaml` with `framework: fastapi`.
   - Detects `package.json` + `next` → emits `app.yaml` with `framework: nextjs`.
   - Default: emits `app.yaml` with `framework: generic` and asks the user for the build command.
7. **Wire `sovereign login`** with the device-code flow (V0: simple password, no browser, for the single-tenant case; real OIDC in V2).

### Code stub

```rust
// crates/sovereign-core/src/use_cases/deploy.rs
use crate::ports::{RuntimePort, StoragePort};
use crate::domain::*;
use crate::error::AppError;
use std::sync::Arc;
use std::time::Duration;

pub struct DeployRequest {
    pub app_id: AppId,
    pub image_ref: Option<String>,
    pub strategy: Strategy,
    pub wait: bool,
    pub actor: String,
}

pub struct DeployResult {
    pub deployment: Deployment,
    pub url: String,
}

pub async fn start(
    state: &AppState,
    req: DeployRequest,
) -> Result<DeployResult, AppError> {
    // 1. Begin deployment (Pending state)
    let dep = state.storage.begin_deployment(
        NewDeployment {
            app_id: req.app_id,
            image_ref: req.image_ref.clone().ok_or(AppError::Validation(
                "either --image or git_repo required".into()
            ))?,
            strategy: req.strategy,
            triggered_by: format!("user:{}", req.actor),
        },
        &req.actor,
    ).await?;
    
    let deployment_id = dep.id;
    
    // 2. Pull image
    state.storage.record_deployment_event(
        deployment_id, DeploymentEvent::Building, &req.actor
    ).await?;
    let image = req.image_ref.as_ref().unwrap();
    state.runtime.pull_image(image).await
        .map_err(|e| AppError::Upstream(e.into()))?;
    
    // 3. Start container
    state.storage.record_deployment_event(
        deployment_id, DeploymentEvent::Pushing, &req.actor
    ).await?;
    let container_id = state.runtime.start_container(
        image, &format!("app-{}", dep.app_id.0)
    ).await.map_err(|e| AppError::Upstream(e.into()))?;
    
    // 4. Health check
    state.storage.record_deployment_event(
        deployment_id, DeploymentEvent::Starting, &req.actor
    ).await?;
    let health = state.runtime.healthcheck(&container_id, "/health", Duration::from_secs(30)).await;
    
    match health {
        Ok(()) => {
            state.storage.record_deployment_event(
                deployment_id, DeploymentEvent::Healthy, &req.actor
            ).await?;
            Ok(DeployResult {
                deployment: state.storage.get_deployment(deployment_id).await?.unwrap(),
                url: state.proxy.url_for(&dep.app_id).await?,
            })
        }
        Err(e) => {
            // Auto-rollback: stop and remove the bad container
            let _ = state.runtime.stop_container(&container_id, Duration::from_secs(5)).await;
            let _ = state.runtime.remove_container(&container_id).await;
            state.storage.record_deployment_event(
                deployment_id,
                DeploymentEvent::Failed(e.to_string()),
                &req.actor,
            ).await?;
            Err(AppError::Upstream(e.into()))
        }
    }
}
```

### Tests

- Unit test: `start_deployment` creates a `Deployment` row in `Pending` state with `triggered_by = "user:alice"`.
- Unit test: a state transition from `Pending → Building` is allowed; `Pending → Healthy` is rejected.
- Integration test (with `testcontainers`): a real Docker daemon, a real image (`nginx:alpine`), and a real healthcheck path. `sovereign deploy` succeeds and the URL is reachable.
- Integration test: deploy with a bad image (non-existent) returns 502 with the upstream error, and the `Deployment` row is in `Failed` state.
- Integration test: deploy with a healthcheck that always returns 500 → 502, auto-rollback runs, old container is still serving.
- Property test (proptest): for any sequence of valid state transitions, the final state is reachable.

### Acceptance criteria

- [ ] `sovereign deploy --app foo --image=nginx:alpine --wait` returns 0 with a URL within 30s on a fresh Hetzner CX22.
- [ ] The previous version is still running until the new version is healthy (bluegreen default).
- [ ] On healthcheck fail, the previous version is still serving and the new container is removed.
- [ ] The `Deployment` row reflects the actual outcome (no "successful" but actually failed).
- [ ] An audit event is appended for every state transition.

### Sub-tasks (clarifications — connection drain, build cache, what-next, cancel)

8. **Implement persistent BuildKit cache** (eliminates the "14-min CI for a typo fix" pain from `user-pain-research.md` §1.1):
   - On first deploy, the binary creates a dedicated `buildkit-cache` volume: `docker volume create sovereign-buildkit-cache`.
   - The build command mounts the cache: `docker buildx build --mount=type=cache,target=/root/.cache/{npm,pip,cargo,go-mod,gem} ...`.
   - On every subsequent deploy, the same cache volume is reused; `npm ci` / `pip install` / `cargo fetch` are no-ops when the lockfile is unchanged.
   - Target: a deploy with an unchanged lockfile completes in < 30s on a CX22; a deploy with a changed lockfile completes in < 90s.
   - The cache is opt-out: `sovereign deploy --no-cache` skips it.
   - The cache is exposed for inspection: `sovereign deploy cache size` reports the volume's `du -sh`.
9. **Implement real zero-downtime with explicit connection drain** (eliminates the "Coolify drops in-flight requests" complaint from `competitive-landscape.md` §1):
   - The shutdown sequence is documented and tested: `SIGTERM → wait 10s for graceful drain → SIGKILL`.
   - The wait period is configurable per app: `app.yaml` has `shutdown_grace_period: 30s` (default 10s).
   - Caddy is configured with `SO_REUSEPORT` so the new process can bind the port while the old process is still draining.
   - The healthcheck is HTTP-only on the `health_path`; the `healthcheck` directive in `app.yaml` defaults to `http://127.0.0.1:<port><health_path>` with 2xx = healthy.
   - A doctor check `deploy_drain_spec` (standard level, G16) verifies every app has a `shutdown_grace_period` set; apps without one are warned.
10. **Implement the "what next?" 5-line post-deploy output** (Lesson 8 from `persona-pm.md`):
    - After a successful deploy, the binary prints exactly:
      ```text
      ✓ https://<id>.srvr.so is live
      ✓ Health check passed (200 OK, 12ms p50)
      ✓ TLS issued (Let's Encrypt, auto-renews in 60d)

      Next, you probably want to:
        → sovereign domain add api.mirasaas.com        # map a real domain
        → sovereign secret set DATABASE_URL --from-stdin
        → sovereign preview --pr 42                    # PR preview environments
        → sovereign backup verify --app postgres       # first restore drill
        → sovereign tui                                # see your fleet at a glance
      ```
    - The 5 next-step commands are templated; the binary checks which features are available (e.g., `preview` is hidden if the app has no `git_repo`) and only shows the relevant ones.
    - The output goes to stderr (so it doesn't pollute `--json`); the JSON output is unchanged.
11. **Implement `sovereign deploy --cancel <deployment-id>` and `sovereign deploy list --stuck`** (closes the "Dokploy #4461 — stuck deployments" gap):
    - `sovereign deploy list --stuck` lists deployments in `Building` / `Pushing` / `Starting` for > 5 min.
    - `sovereign deploy --cancel <id>` sends SIGTERM to the BuildKit / docker build / container start process, waits 5s, then SIGKILL. The `Deployment` row transitions to `Cancelled` (a new state, distinct from `Failed`). The audit log records the cancellation.
    - `--cancel` requires a 5s confirm in interactive mode; in `--non-interactive` (CI) it requires `--confirm`.
    - A cron (`/etc/cron.d/sovereign-deploy-cleanup`) runs every 15 min and auto-cancels deployments stuck for > 30 min.
12. **Add a CI test**: a fixture deploys a real app with the persistent cache; a 2nd deploy with the same lockfile completes in < 30s; a 3rd deploy with a changed lockfile completes in < 90s; the `--cancel` path kills a stuck build.

### Definition of done

A test that deploys `nginx:alpine` to a real Docker daemon, verifies the URL is reachable, rolls back, and verifies the previous version is still serving — passes in CI on every PR.

---

## F5. One-command rollback

**Goal:** `sovereign rollback <app>` returns to the previous version in < 10 seconds. `sovereign rollback <app> --to=<deployment_id>` returns to a specific historical version.

**Why it matters:** Rollback is the **highest-leverage feature for trust**. A user who knows they can roll back in 10 seconds is willing to ship 10x more often. A user who can't roll back ships once a week, in fear.

### Sub-tasks

1. **Implement the `rollback` use case** in `sovereign-core/src/use_cases/rollback.rs`:
   - List `Deployment` rows for the app in `Healthy` state, ordered by `started_at DESC`.
   - Pick the target: if `--to=<id>`, use that; else use the most recent `Healthy` before the current.
   - Stop the current container.
   - Start the target container.
   - Healthcheck; on pass, mark `RolledBack` (with `target_deployment_id` in the audit payload).
   - On fail, start the *original* container (the one that was running before this rollback attempt).
2. **Wire the `rollback` CLI subcommand**:
   - `sovereign rollback api` → rolls back to the previous healthy version.
   - `sovereign rollback api --to=<id>` → rolls back to a specific deployment.
   - `sovereign rollback api --dry-run` → prints "would roll back to deployment v1.4.2", exits 0.
   - **`sovereign rollback api --list`** (added in this clarification, addresses the "discoverability of versions" gap from `user-pain-research.md` §1.3): prints a table of the last 20 healthy deployments with id, image, started_at, age, and a marker for the current. Example:
     ```text
     $ sovereign rollback api --list
     ID        IMAGE                       STARTED              AGE     CURRENT
     dep-009   ghcr.io/me/api:v1.4.2       2026-06-03 14:22:01  2d      ←
     dep-008   ghcr.io/me/api:v1.4.1       2026-06-02 09:15:00  3d
     dep-007   ghcr.io/me/api:v1.4.0       2026-05-30 11:42:30  6d
     dep-006   ghcr.io/me/api:v1.3.3       2026-05-28 16:01:00  8d
     dep-005   ghcr.io/me/api:v1.3.2       2026-05-25 10:30:00  11d
     ```
   - **`sovereign rollback api --list --limit=50`** and **`--list --json`** are supported.
3. **Wire the `history` CLI subcommand** (V0.5 bonus):
   - `sovereign history api` → lists all deployments for the app with status, image, started_at.
4. **Add the `target_deployment_id` column to `deployment`** (V0.1 migration): nullable, references `deployment(id)`. Used for audit.
5. **Write a `sovereign.lock` file in the repo on every successful deploy** (the "deploy receipt" — addresses the "I deployed at midnight, the server forgot" pain from `user-pain-research.md` §8.3):
   - The file is a single-line JSON in the git repo: `{"deployment_id": "dep-009", "image": "ghcr.io/me/api:v1.4.2", "deployed_at": "2026-06-03T14:22:01Z", "actor": "user:alice", "sovereign_version": "1.0.0"}`.
   - The file is written via a post-deploy hook (the `sovereign deploy` command runs `git -C <repo> add sovereign.lock && git -C <repo> commit -m "deploy: <deployment_id>" && git -C <repo> push` after a healthy deploy).
   - The file is the **single source of truth** for "what is running in production" — when the user opens the repo, they see the last deploy at the top of the working tree.
   - The file is also useful for forensics: `git log sovereign.lock` shows the entire deploy history.
   - The file is opt-out: `sovereign deploy --no-lock`.
   - The doctor `audit_log_append_only` check (G16 standard level) is augmented to also verify the `sovereign.lock` file in the repo matches the latest `Healthy` deployment in the SQLite state.

### Code stub

```rust
// crates/sovereign-core/src/use_cases/rollback.rs
pub struct RollbackRequest {
    pub app_id: AppId,
    pub to: Option<DeploymentId>,
    pub actor: String,
}

pub async fn start(state: &AppState, req: RollbackRequest) -> Result<Deployment, AppError> {
    // 1. Find the current running deployment (Healthy)
    let current = state.storage.get_current_deployment(req.app_id).await?
        .ok_or(AppError::NotFound("no current deployment".into()))?;
    
    // 2. Find the target deployment
    let target = match req.to {
        Some(id) => state.storage.get_deployment(id).await?
            .ok_or(AppError::NotFound("target deployment not found".into()))?,
        None => {
            let history = state.storage.list_healthy_deployments_before(req.app_id, current.started_at, 1).await?;
            history.into_iter().next()
                .ok_or(AppError::NotFound("no previous healthy deployment".into()))?
        }
    };
    
    // 3. Stop the current container
    let current_container = format!("app-{}-{}", req.app_id.0, current.id.0);
    state.runtime.stop_container(&current_container, Duration::from_secs(30)).await?;
    state.runtime.remove_container(&current_container).await?;
    
    // 4. Start the target container
    let target_container = format!("app-{}-{}", req.app_id.0, target.id.0);
    state.runtime.start_container(&target.image_ref, &target_container).await?;
    let health = state.runtime.healthcheck(&target_container, "/health", Duration::from_secs(30)).await;
    
    match health {
        Ok(()) => {
            // 5. Record the rollback as a new deployment in RolledBack state, with target = current
            let new_dep = state.storage.begin_deployment(
                NewDeployment {
                    app_id: req.app_id,
                    image_ref: target.image_ref.clone(),
                    strategy: Strategy::Recreate,
                    triggered_by: format!("user:{}:rollback", req.actor),
                },
                &req.actor,
            ).await?;
            state.storage.record_deployment_event(
                new_dep.id, DeploymentEvent::Healthy, &req.actor,
            ).await?;
            state.storage.set_rollback_target(new_dep.id, current.id, &req.actor).await?;
            Ok(state.storage.get_deployment(new_dep.id).await?.unwrap())
        }
        Err(e) => {
            // Auto-rollback-failure: try to start the original container
            let _ = state.runtime.start_container(&current.image_ref, &current_container).await;
            Err(AppError::Upstream(e.into()))
        }
    }
}
```

### Tests

- Integration test: deploy v1, deploy v2, rollback → v1 is running, audit log shows the rollback, history shows v1, v2, v2-rollback.
- Integration test: rollback to a specific deployment id works.
- Integration test: rollback with no previous healthy version returns `AppError::NotFound`.
- Integration test: rollback to a deployment whose image no longer pulls → fail, original is restored.

### Acceptance criteria

- [ ] `sovereign rollback api` returns 0 in < 10s on a Hetzner CX22.
- [ ] The previous version is serving traffic within 10s.
- [ ] The audit log records the rollback with `target_deployment_id`.
- [ ] `--dry-run` prints the target and exits 0.

### Definition of done

A test that deploys v1, deploys v2, rolls back, and asserts that the URL is now serving v1 — passes in CI on every PR.

---

## F6. Caddy auto-TLS (HTTP-01 ACME, auto-renew, hot reload)

**Goal:** Every public hostname is automatically issued a Let's Encrypt TLS certificate via the Caddy admin API, auto-renewed, and hot-reloaded without dropping traffic.

**Why it matters:** TLS is the difference between "demo" and "production." Manual certbot is the #1 user pain in the competitive analysis. Auto-TLS via Caddy is the single biggest UX win for zero configuration.

### Sub-tasks

1. **Implement `CaddyProxy`** in `sovereign-proxy-caddy/`:
   - `connect()` starts the Caddy admin API (default `http://127.0.0.1:2019`) or connects to an existing Caddy.
   - `add_route(host, upstream_port)` calls `PUT /config/apps/http/servers/srv0/routes/...` with the new route.
   - `remove_route(host)` removes the route.
   - `reload()` calls `POST /load` with the full config (V0: not used; V1: for fail-over to a fresh Caddy).
   - `url_for(app_id)` returns `https://<random>.srvr.so` (V0) or `https://<app_id>.srvr.so` (V0.5).
2. **Configure the default Caddyfile** at `crates/sovereign/assets/Caddyfile`:
   ```
   {
     admin localhost:2019
     auto_https on
   }
   :443 {
     tls internal  # V0: use Caddy's internal CA, not Let's Encrypt
     reverse_proxy {args.0}  # placeholder
   }
   ```
3. **Wire the Caddy admin API** for `add_route` and `remove_route`:
   ```json
   {
     "@id": "<host>",
     "match": [{"host": ["<host>"]}],
     "handle": [{
       "handler": "reverse_proxy",
       "upstreams": [{"dial": "127.0.0.1:<port>"}]
     }],
     "terminal": true
   }
   ```
4. **Spawn Caddy as a child process** in the composition root (V0: from a system-installed Caddy; V1: bundled binary).
5. **Wire `domain add <hostname>` CLI** (V0.5 bonus):
   - For the wildcard `srvr.so`, Caddy uses HTTP-01 ACME via the Caddy ACME server (no manual cert management).
   - For custom domains, use DNS-01 ACME (V1: provider-specific, V0.5: HTTP-01 only).
6. **Add health check for Caddy** in `sovereign-observability/`:
   - `GET http://127.0.0.1:2019/config/` every 30s.
   - 200 OK = healthy; non-200 = degraded, alert (Sev2).
7. **Document the Caddy fallback to Nginx** in [`tech-stack.md` §1](#) — out of scope for V0, V1.1 adds the Nginx adapter.

### Code stub

```rust
// crates/sovereign-proxy-caddy/src/lib.rs
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

pub struct CaddyProxy {
    client: Client,
    admin_url: String,
}

impl CaddyProxy {
    pub async fn connect(admin_url: &str) -> anyhow::Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        // Health check
        let resp = client.get(format!("{}/config/", admin_url)).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("caddy admin API not reachable at {}", admin_url);
        }
        Ok(Self { client, admin_url: admin_url.to_string() })
    }
    
    pub async fn add_route(&self, host: &str, upstream_port: u16) -> anyhow::Result<()> {
        let route_id = format!("route-{}", host);
        let route = json!({
            "@id": route_id,
            "match": [{"host": [host]}],
            "handle": [{
                "handler": "reverse_proxy",
                "upstreams": [{"dial": format!("127.0.0.1:{}", upstream_port)}]
            }],
            "terminal": true
        });
        let path = format!("/config/apps/http/servers/srv0/routes/{}", route_id);
        let resp = self.client.put(format!("{}{}", self.admin_url, path))
            .json(&route)
            .send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("caddy add_route failed: {} {}", resp.status(), resp.text().await?);
        }
        Ok(())
    }
    
    pub async fn remove_route(&self, host: &str) -> anyhow::Result<()> {
        let route_id = format!("route-{}", host);
        let path = format!("/config/apps/http/servers/srv0/routes/{}", route_id);
        let resp = self.client.delete(format!("{}{}", self.admin_url, path))
            .send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("caddy remove_route failed: {}", resp.status());
        }
        Ok(())
    }
}
```

### Tests

- Integration test (with `testcontainers` Caddy): `add_route("test.example.com", 8080)` and a curl to the route succeeds.
- Integration test: `remove_route` and a curl to the route fails with 404.
- Integration test: `CaddyProxy::connect` to a non-existent Caddy returns an error.
- Unit test: the JSON path is correct (`/config/apps/http/servers/srv0/routes/<id>`).

### Acceptance criteria

- [ ] A deployed app with a hostname is reachable at `https://<hostname>` within 60s.
- [ ] The TLS cert is auto-issued (V0: internal CA; V1: Let's Encrypt).
- [ ] Removing the route makes the hostname unreachable.

### Definition of done

A test that deploys `nginx:alpine`, adds a route for `test.local`, curls the route, removes the route, curls again (must 404) — passes in CI.

---

## F7. Encrypted secret store (age + envelope encryption, zero-disk injection)

**Goal:** `sovereign secret set DATABASE_URL` reads the value from stdin, encrypts it with `age`, stores the ciphertext in the `secret` table, and injects it into the container as an env var at process start (never on disk).

**Why it matters:** Secrets are the #1 security failure mode for self-hosted platforms. Zero-disk injection (the secret never touches the container's filesystem) is the design that prevents the 2018-Tesla-cloud-credentials-leak and similar incidents.

### Sub-tasks

1. **Implement `AgeSecrets`** in `sovereign-secrets-age/`:
   - `init_master_key()` generates a 32-byte X25519 key, encrypted at rest with an `argon2id` passphrase (config), stored at `/var/lib/sovereign/master.key.age`.
   - `encrypt(plaintext)` returns age ciphertext + nonce, using the master public key.
   - `decrypt(ciphertext)` returns plaintext, using the master private key.
   - `decrypt_to_memory(ciphertext, &mut [u8])` is a **zero-disk** variant: decrypts directly into a caller-provided buffer, never touches a temp file.
2. **Wire `secret set <APP> <KEY>` CLI**:
   - Reads value from stdin (not as a CLI arg, to avoid `/proc/<pid>/cmdline` leak).
   - Calls `state.secrets.encrypt(value)`.
   - Stores the ciphertext in the `secret` table with `(app_id, key)` unique.
   - Appends an audit event: `kind: "secret.set"`, `actor`, `target: "app:<app_id>"`, `payload: {"key": "<key>"}` (never the value).
3. **Wire `secret list <APP>` CLI**:
   - Lists `key` + `rotated_at` + `version`, never the value.
4. **Implement zero-disk injection** in `RuntimePort::start_container`:
   - When starting a container, fetch the secrets for the app, decrypt to a `Vec<u8>`, pass to the runtime adapter as an env var map.
   - The runtime adapter (Docker) sets the env vars on the container; they are *not* written to a `.env` file on disk.
5. **Implement secret rotation** (V0.5 bonus):
   - `sovereign secret rotate <APP> <KEY>` reads a new value, encrypts, updates the row, redeploys the app.
   - Old value is overwritten in memory; the audit log retains the rotation event.
6. **Document the "never log secrets" rule** in [`architecture.md` §8.1](#) — no log line may contain a secret value, ever. Add a `tracing` filter that redacts known secret keys.
7. **Implement `sovereign secret exec -- <cmd>` (AI-agent safe mode)** — addresses the "Claude Code / Cursor reads `.env` and sends secrets to LLM providers" pain from `user-pain-research.md` §3.2:
   - Subcommand signature: `sovereign secret exec --app <name> [--secret KEY=ref] [-- <cmd> [args...]]`.
   - When `-- <cmd>` is given, the binary:
     - Fetches the secrets for `--app <name>` (or a comma-separated list of `--secret KEY=ref`).
     - Decrypts each to a `Vec<u8>` in memory.
     - Forks the child process with `execve`, passing the secrets as env vars.
     - **Never** writes a `.env` file, a temp file, a shell variable export, or a wrapper script.
   - The child process inherits the secrets as env vars only. The parent process waits for the child to exit and forwards the exit code.
   - This is the canonical "run my app with its secrets, but don't leave a trace" mode. The intended users are AI coding agents (Claude Code, Cursor, Copilot) and CI runners, both of which historically read `.env` files from disk.
   - Privacy guarantees:
     - The secret values are not echoed to stdout/stderr (the binary suppresses the `Command::spawn` debug output).
     - The secret values are not logged.
     - The secret values are not written to `/proc/<pid>/environ` of the parent (only the child).
     - On Unix, the child process's `/proc/<pid>/environ` is mode 0400 (the default); the AI agent cannot read its own parent's env, only its own.
   - The `exec` subcommand works with both `DockerRuntime` (exec inside a container) and the host process (exec a local binary). The default is the host process.
   - Example:
     ```text
     $ sovereign secret exec --app api -- python manage.py migrate
     ✓ Decrypting 4 secrets for app "api" into the child process
     ✓ Running: python manage.py migrate
     Operations to perform:
       Apply all migrations: admin, auth, contenttypes, sessions
     Running migrations:
       No migrations to apply.
     ✓ Exit code 0
     ```
   - The binary also exposes `sovereign secret exec --print` (for scripts that want a key=value stream, e.g., `eval $(sovereign secret exec --print --app api)` — **discouraged** in docs but available). The default is `--no-print` (the values never reach the parent's stdout).
8. **Add a doctor check** `secret_no_plaintext_on_disk` (basic level, F9): scans `/var/lib/sovereign/`, `/tmp/`, `/var/log/` for any file containing a known secret key name (DATABASE_URL, STRIPE_SECRET_KEY, etc.) and reports a `Fail` with the file path. Catches the "secret was written to a file by accident" case.
9. **Add a CI test**: `sovereign secret exec --app test -- echo $DATABASE_URL` prints the decrypted value (since `echo` exposes its env) but does not write a `.env` file. The filesystem scan post-exec finds 0 files containing the value.

### Code stub

```rust
// crates/sovereign-secrets-age/src/lib.rs
use age::{x25519, Encryptor, Decryptor};
use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
use std::io::{Read, Write};

pub struct AgeSecrets {
    master_secret_key: x25519::SecretKey,
}

impl AgeSecrets {
    pub async fn init(passphrase: &str) -> anyhow::Result<Self> {
        // Derive key from passphrase using argon2id
        let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
        let argon2 = Argon2::default();
        let key_material = argon2.hash_password(passphrase.as_bytes(), &salt)?.hash.ok_or(...)?;
        
        // Use the derived key as the age passphrase
        let identity = age::scrypt::Identity::new(key_material.to_string())?;
        // ... derive x25519::SecretKey from this
        Ok(Self { master_secret_key })
    }
    
    pub fn encrypt(&self, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let pubkey = self.master_secret_key.to_public();
        let encryptor = Encryptor::with_recipients(std::iter::once(&pubkey as &dyn age::Recipient))?;
        let mut ciphertext = vec![];
        let mut writer = encryptor.wrap_output(&mut ciphertext)?;
        writer.write_all(plaintext)?;
        writer.finish()?;
        Ok(ciphertext)
    }
    
    /// Zero-disk decryption: writes plaintext directly into the caller's buffer.
    /// The plaintext never touches a heap allocation that may be paged to disk.
    pub fn decrypt_to_memory(&self, ciphertext: &[u8], out: &mut [u8]) -> anyhow::Result<usize> {
        let identity = age::x25519::Identity::from(self.master_secret_key.clone());
        let decryptor = Decryptor::new(ciphertext)?;
        let mut reader = decryptor.decrypt(std::iter::once(&identity as &dyn age::Identity))?;
        let n = reader.read(out)?;
        Ok(n)
    }
}
```

### Tests

- Unit test: `encrypt` → `decrypt_to_memory` roundtrip succeeds.
- Unit test: `decrypt_to_memory` with a wrong master key returns `DecryptError`.
- Integration test: `secret set` → `secret list` shows the key but not the value.
- Integration test: a container started with secrets has the env vars set, but a `docker exec` into the container does not see any secret file on disk.
- Unit test: the `tracing` redactor strips known secret keys from log output.

### Acceptance criteria

- [ ] `sovereign secret set <APP> <KEY>` reads from stdin, never from a CLI arg.
- [ ] The value is never logged, never echoed, never written to a file.
- [ ] `sovereign secret list` shows keys but not values.
- [ ] A container with secrets has them in env vars, not on disk.

### Definition of done

A test that sets a secret, starts a container, `docker exec` into it, runs `env | grep <KEY>` (sees it), runs `find / -name "*<KEY>*"` (does not see any file) — passes in CI.

---

## F8. Backup + health check + auto-rollback

**Goal (split into F8a and F8b for the 6-week timeline):**
- **F8a:** `sovereign backup create` dumps a Postgres database to S3-compatible storage. The dump is verified by size sanity check.
- **F8b:** A deployed app with a `health_path` is continuously health-checked. On 3 consecutive 5xx, auto-rollback runs.

### F8a — Postgres backup

**Sub-tasks:**

1. **Implement `PostgresBackup`** in `sovereign-backup/`:
   - `snapshot(target)` runs `pg_dump --format=custom --file=<tmpfile>` and uploads to S3-compatible.
   - `restore(backup_id, target)` downloads from S3, runs `pg_restore --clean --if-exists --dbname=<target>`.
   - `verify(backup_id)` downloads the dump, runs `pg_restore --list` to enumerate tables, returns the count.
2. **Use `restic`** as the S3 backend (encrypted, deduplicated, supports S3-compatible, Rclone, B2, etc.).
3. **Wire `backup create --app postgres` CLI**:
   - Calls `state.backup.snapshot(target)`, stores metadata in the `backup` table.
   - `sovereign backup list` shows all backups with status, size, location, last verify.
4. **Implement `backup verify` CLI** (V1.2, not V0 — but the **infrastructure** is V0):
   - Restores the dump to a scratch database (`sovereign_verify_<timestamp>`), counts rows in each table, compares to source, drops the scratch DB, returns the diff.
5. **Schedule daily backups** (V1.2, not V0): cron entry in `/etc/cron.d/sovereign-backup` for `0 3 * * *`.
6. **Implement size sanity check on `snapshot()`** (addresses the "8 months of 812-byte empty backups" pain from `user-pain-research.md` §4.1):
   - After `pg_dump` writes the temp file, the binary checks `tempfile.size() > MIN_SIZE_BYTES` where `MIN_SIZE_BYTES` is computed from the source DB size: `min(10 MB, max(1 KB, source_size / 100))`. An empty / near-empty source produces a 1 KB floor; a 1 GB source expects ≥ 10 MB.
   - If the size is below the floor, the snapshot is **discarded** (not uploaded) and the operation returns `BackupError::SuspiciousSize { expected_min, actual }`. The previous successful backup is left in place.
   - The `backup` row records `status: "failed_size_check"`, the actual size, and the expected minimum. The doctor basic level (F9) gets a `backup_size_sane` check.
7. **Schedule automatic daily verification** (V0 — addresses the "8 months of empty backups" pain directly):
   - On first install, the binary creates `/etc/cron.d/sovereign-backup-verify` running at `0 4 * * *` (1 hour after the daily backup): `sovereign backup verify --auto`.
   - `sovereign backup verify --auto` picks the last 3 successful backups (not just the most recent), runs `verify` on each, and:
     - On all-pass: writes an audit event `kind: "backup.verify.auto"`, exits 0.
     - On any fail: writes an audit event `kind: "backup.verify.auto_fail"`, sends a Telegram/email alert, and exits 2.
   - The auto-verify is opt-out: `[backup] auto_verify = false` in `sovereign.toml`.
   - The auto-verify uses the same `PostgresBackup::verify` as the on-demand path; no parallel implementation.
8. **Add Prometheus metrics**:
   - `sovereign_backup_size_bytes{app,backup_id}` — gauge, set after each `snapshot`.
   - `sovereign_backup_expected_min_bytes{app}` — gauge, the computed floor.
   - `sovereign_backup_verify_duration_seconds{app,backup_id,result}` — histogram.
   - `sovereign_backup_last_auto_verify_timestamp{app}` — gauge, the last successful auto-verify.
9. **Add a doctor check** `backup_size_sane` (basic level, F9): the last backup's size is ≥ the computed floor.
10. **Add a doctor check** `backup_auto_verify_recent` (standard level, G16): the last successful auto-verify was within 26h.
11. **Add a CI test**: a fixture where the Postgres is deliberately misconfigured to time out; the resulting backup is 812 bytes; the size check rejects it; the doctor reports `backup_size_sane` as `Fail`; the previous successful backup is unchanged.

**Acceptance criteria (F8a):**
- [ ] `sovereign backup create --app postgres` produces a dump in S3.
- [ ] The dump can be downloaded and `pg_restore`'d to a fresh DB.
- [ ] The `backup` table records the size, location, status.
- [ ] A dump that fails the size check is discarded, not uploaded.
- [ ] The auto-verify cron runs daily and alerts on any failure.

### F8b — Health check + auto-rollback

**Sub-tasks:**

1. **Implement `health` use case** (already partially done in F4): continuous probing.
2. **Spawn a background health-checker** in the binary crate:
   - Every 30s, for every `Healthy` deployment, run the health probe.
   - On 3 consecutive failures, transition the deployment to `Failed` and trigger `rollback::start` to the previous `Healthy` version.
   - Append an audit event: `kind: "auto.rollback"`, `payload: {"reason": "health_failure", "deployment_id": "..."}`.
3. **Implement the health-check config** in `app.yaml`:
   ```yaml
   health:
     path: /health
     interval: 30s
     timeout: 5s
     threshold: 3
   ```
4. **Wire the auto-rollback** to be visible: `sovereign status` shows "auto-rolled back at 14:22:03 due to health failure."

**Acceptance criteria (F8b):**
- [ ] A deployment whose healthcheck returns 500 for 3 consecutive probes is auto-rolled-back.
- [ ] The previous version is serving within 30s of the third failure.
- [ ] An audit event records the auto-rollback.

### Definition of done (F8)

A test that:
1. Deploys v1 (a healthy `nginx:alpine`).
2. Deploys v2 (a broken image that returns 500 on `/health`).
3. Waits for the auto-rollback (≤ 90s).
4. Verifies v1 is serving.
5. Verifies the audit log shows the auto-rollback.
Passes in CI on every PR.

---

## F9. `sovereign doctor` — basic diagnostic (the on-call's first command)

**Goal:** Ship the `sovereign doctor` command at `--level basic` so the very first time the engine breaks (3am pager), the on-call can run one command and see exactly which of the 6 load-bearing subsystems is unhealthy. The full spec is in [`doctor.md`](./doctor.md); this feature ships the basic level only.

**Why it matters:** The 3am test is the first invariant: *one operator can run it for 5 years.* If the operator has to remember 7 different `journalctl`, `docker`, `curl`, and `sqlite3` invocations to figure out why the deploy failed, the system has already lost. `sovereign doctor` makes diagnosis one command. This is also the foundation: every later check (V1 standard, V1.5 fleet, V2 paranoid) is an additional check behind the same CLI.

### Sub-tasks

1. **Create the `sovereign-doctor` crate** (the 11th V0 crate, the only new one in this phase):
   ```bash
   cargo new --lib crates/sovereign-doctor
   ```
2. **Define the `DoctorLevel` enum** in `sovereign-doctor/src/lib.rs`:
   ```rust
   #[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq, PartialOrd, Ord)]
   pub enum DoctorLevel { Basic, Standard, Full, Paranoid, Custom }
   ```
3. **Define the `Check` trait** in `sovereign-doctor/src/check.rs`:
   ```rust
   #[async_trait]
   pub trait Check: Send + Sync {
       fn name(&self) -> &'static str;
       fn category(&self) -> CheckCategory;
       async fn run(&self, ctx: &DoctorContext) -> CheckResult;
   }
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum CheckStatus { Pass, Warn, Fail, Skip }
   pub struct CheckResult {
       pub status: CheckStatus,
       pub message: String,
       pub suggestion: Option<String>,
       pub fix: Option<Box<dyn DoctorFix>>,
   }
   ```
4. **Define the `DoctorFix` trait** in `sovereign-doctor/src/fix.rs` (the safe automatable fixes only — never destructive):
   ```rust
   #[async_trait]
   pub trait DoctorFix: Send + Sync {
       fn id(&self) -> &'static str;
       fn description(&self) -> &'static str;
       fn is_destructive(&self) -> bool;
       async fn apply(&self, ctx: &DoctorContext) -> Result<FixOutcome, FixError>;
   }
   ```
5. **Implement the 6 V0 check categories** (basic level only — see [`doctor.md` §3](./doctor.md) for the full 14 categories). Each check is a struct in `sovereign-doctor/src/checks/<category>.rs`:
   - **System** — `kernel` (Linux ≥ 5.10), `cgroups_v2` (mounted at `/sys/fs/cgroup`), `memory` (≥ 512 MB free), `disk` (≥ 5 GB free on `/var/lib/sovereign`), `capabilities` (CAP_NET_BIND_SERVICE for the binary, no CAP_SYS_ADMIN).
   - **Binary** — `binary_self` (the running binary's path, version, mtime, sha256 vs. the published manifest), `binary_stripped` (no debug symbols, no path leaks).
   - **Storage** — `sqlite_openable` (the data dir is writable, the DB file opens), `sqlite_writable` (a write succeeds, then a read back), `sqlite_wal_mode` (WAL is enabled), `migrations_current` (the `migration` table shows the current version).
   - **Runtime** — `docker_socket` (the user is in the `docker` group, the socket is reachable, version ≥ 24), `docker_pull` (a test pull of `alpine:3.20` succeeds), `docker_run` (a test `docker run --rm alpine:3.20 echo ok` returns 0).
   - **Proxy** — `caddy_installed` (binary on PATH), `caddy_config_valid` (`caddy validate` exits 0 on the generated config), `caddy_running` (Caddy is up and the admin API responds on `:2019`), `port_80_443_free` (no other process is bound).
   - **Secrets** — `sops_age_installed` (V0: optional, warn if missing), `master_key_present` (`/var/lib/sovereign/master.key` exists, 0600, owned by `sovereign` user), `master_key_decrypts` (a test ciphertext from `sovereign secret set --selftest` decrypts).
6. **Implement the CLI** `crates/sovereign/src/cmd/doctor.rs`:
   - `sovereign doctor [--level <LEVEL>] [--explain] [--fix] [--report <path>] [--watch] [--json]`
   - V0 supports `--level basic` only; `--level standard|full|paranoid` returns `Err("doctor level X is V1+; upgrade with `sovereign update`")`.
   - `--json` outputs the same data as `--report`, to stdout.
   - Default exit codes: 0 = all pass, 1 = any warn, 2 = any fail.
7. **Implement the 3 V0 fixes** (one per category that has a non-destructive remediation):
   - `caddy_config_reload` — runs `caddy reload --config /etc/caddy/Caddyfile`.
   - `disk_cleanup` — runs `journalctl --vacuum-size=100M` and `docker system prune -f` (destructive but only of cache).
   - `master_key_repair_perms` — `chmod 0600 /var/lib/sovereign/master.key` and `chown sovereign:sovereign`. Non-destructive.
   - All other categories' `CheckResult.fix` is `None` in V0.
8. **Implement `--report`** — writes a markdown report to the given path (default: `/var/log/sovereign/doctor-<timestamp>.md`). Contains every check, its result, the message, the suggestion, and a "share this safely" disclaimer.
9. **Implement `--watch`** — runs basic level every 30s, prints a diff if any check changes state. `Ctrl+C` exits cleanly. V0 is single-shot only; V1+ adds the continuous event log.
10. **Implement `sovereign doctor --fix`** (basic level only, opt-in): for every check with a fix available, run it in the order: cheapest first, non-destructive before destructive, with a 5s confirmation prompt per destructive fix.
11. **Wire doctor into the auto-rollback path (F8b)**: when a deployment auto-rolls back, the binary appends a `sovereign doctor --level basic` snapshot to the audit log. The on-call can then read the doctor output in the audit.
12. **Add Prometheus metrics** to `sovereign-observability`:
    - `sovereign_doctor_runs_total{level,status}` — counter, one per invocation.
    - `sovereign_doctor_check_duration_seconds{category,name}` — histogram.
    - `sovereign_doctor_last_status{level}` — gauge, 1 = pass, 0 = warn, -1 = fail.
13. **Add a CI smoke test**: a GitHub Actions job runs `sovereign doctor --level basic` on every PR against a fresh Ubuntu 22.04 runner. The job fails if any check is `Fail` (Warns allowed).

### Code stub

```rust
// crates/sovereign-doctor/src/lib.rs
use async_trait::async_trait;
use clap::ValueEnum;

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq, PartialOrd, Ord)]
pub enum DoctorLevel { Basic, Standard, Full, Paranoid, Custom }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus { Pass, Warn, Fail, Skip }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckCategory {
    System, Binary, Storage, Runtime, Proxy, Secrets,
    Backup, Network, Agents, Observability, Security,
    Sovereignty, Performance, Cost,
}

pub struct CheckResult {
    pub status: CheckStatus,
    pub message: String,
    pub suggestion: Option<String>,
    pub fix: Option<Box<dyn DoctorFix>>,
}

#[async_trait]
pub trait Check: Send + Sync {
    fn name(&self) -> &'static str;
    fn category(&self) -> CheckCategory;
    async fn run(&self, ctx: &DoctorContext) -> CheckResult;
}

pub struct DoctorContext {
    pub data_dir: std::path::PathBuf,
    pub config: std::sync::Arc<sovereign_core::Config>,
    pub runtime: std::sync::Arc<dyn sovereign_core::ports::RuntimePort>,
    // ... ports the checks need
}

pub struct Doctor {
    level: DoctorLevel,
    checks: Vec<Box<dyn Check>>,
    fixes: Vec<Box<dyn DoctorFix>>,
}

impl Doctor {
    pub fn for_level(level: DoctorLevel) -> Self {
        let mut checks: Vec<Box<dyn Check>> = vec![];
        if level >= DoctorLevel::Basic {
            checks.extend_from_slice(&[
                Box::new(checks::system::KernelCheck),
                Box::new(checks::system::CgroupsV2Check),
                Box::new(checks::binary::BinarySelfCheck),
                Box::new(checks::storage::SqliteOpenableCheck),
                Box::new(checks::storage::MigrationsCurrentCheck),
                Box::new(checks::runtime::DockerSocketCheck),
                Box::new(checks::runtime::DockerPullCheck),
                Box::new(checks::proxy::CaddyInstalledCheck),
                Box::new(checks::proxy::CaddyConfigValidCheck),
                Box::new(checks::secrets::MasterKeyPresentCheck),
            ]);
        }
        // Standard/Full/Paranoid checks added in V1+ via `extension_modules` pattern.
        Self { level, checks, fixes: vec![] }
    }

    pub async fn run(&self, ctx: &DoctorContext) -> DoctorReport {
        let mut report = DoctorReport::new(self.level);
        for check in &self.checks {
            let start = std::time::Instant::now();
            let result = check.run(ctx).await;
            report.add(check.name(), check.category(), result, start.elapsed());
        }
        report
    }
}
```

### Tests

- Unit: each of the 10 V0 basic checks has a unit test that constructs a fake `DoctorContext` and asserts the expected `CheckStatus`.
- Integration: a fresh `ubuntu:22.04` Docker container with the binary installed passes all 10 checks.
- Integration: deliberately breaking one subsystem (e.g., `chmod 0644 /var/lib/sovereign/master.key`) makes the corresponding check return `Fail` with a clear message and the `master_key_repair_perms` fix suggestion.
- Integration: `sovereign doctor --fix` repairs the broken permission and exits 0.
- Property test (`proptest`): running doctor 1000 times in a row is idempotent (no side effects, no log spam).
- CI gate: the GitHub Actions job runs doctor on every PR and fails on any `Fail`.

### Acceptance criteria

- [ ] `sovereign doctor --level basic` runs 10 checks in < 10s on a CX22.
- [ ] The output is colored: green ✓, yellow ⚠, red ✗. `--json` for machines.
- [ ] `--level standard|full|paranoid` returns a clear "not in V0" error pointing to the upgrade.
- [ ] `--fix` repairs the 3 repairable V0 issues non-interactively for non-destructive, with a 5s confirm for destructive.
- [ ] The CI job fails the build on any `Fail` check.

### Definition of done

A Show HN demo where the binary is deliberately broken (kill Docker, delete the master key, fill the disk), `sovereign doctor` is run, and the output shows 3 fails with clear messages and 2 fix suggestions. The on-call's first command is `sovereign doctor`. This is the **single most important CLI command in V0 after `sovereign deploy`.**

---

## F10. `sovereign update` — self-update with rollback

**Goal:** The binary can update itself to the latest release from the official channel, with a mandatory backup of the running binary, atomic symlink swap, and one-command rollback if the new version fails the smoke test.

**Why it matters:** Every other V0+ feature is in this binary. If a security CVE is disclosed (CVE in axum, rustls, sqlite, age, or any of the 90+ V0 crates), the only way to ship a fix is `sovereign update`. Without self-update, every CVE is a `ssh` + `scp` + `systemctl restart` cycle. With self-update, it's `sovereign update && sovereign doctor --level basic && done`. Pairs directly with F9: a successful `sovereign update` is verified by `sovereign doctor` on the new binary.

### Sub-tasks

1. **Define the `UpdateChannel` enum** in `sovereign-core/src/ports/update.rs`:
   ```rust
   #[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
   pub enum UpdateChannel { Stable, Rc, Nightly }
   ```
2. **Implement `UpdatePort`** trait (the abstract interface) and the in-house implementation `sovereign-update` (in-tree, no external dependency):
   ```rust
   #[async_trait]
   pub trait UpdatePort: Send + Sync {
       async fn current_version(&self) -> Result<Version, UpdateError>;
       async fn latest(&self, channel: UpdateChannel) -> Result<Release, UpdateError>;
       async fn fetch(&self, release: &Release, dest: &Path) -> Result<Sha256, UpdateError>;
       async fn apply(&self, new_binary: &Path) -> Result<UpdateRecord, UpdateError>;
       async fn rollback(&self, record: &UpdateRecord) -> Result<(), UpdateError>;
   }
   ```
3. **Implement the release manifest** — the update server hosts `https://releases.sovereignruntime.dev/stable.json` with:
   ```json
   {
     "version": "0.1.0",
     "channel": "stable",
     "released_at": "2025-12-01T00:00:00Z",
     "binaries": {
       "x86_64-unknown-linux-musl": {
         "url": "https://releases.sovereignruntime.dev/0.1.0/sovereign-x86_64-unknown-linux-musl.tar.gz",
         "sha256": "abc123...",
         "size": 12582912
       }
     },
     "min_supported_downgrade": "0.0.9"
   }
   ```
4. **Implement the atomic apply**:
   - Step 1: download to `/var/lib/sovereign/cache/sovereign-<version>`.
   - Step 2: verify SHA-256 against the manifest.
   - Step 3: `cp` the running binary to `/var/lib/sovereign/backups/sovereign-<prev-version>`.
   - Step 4: write an `UpdateRecord` (version, prev version, timestamp, channel) to the `update_history` table.
   - Step 5: `chmod 0755` the new binary, `rename(2)` it over `/usr/local/bin/sovereign` (atomic on the same filesystem).
   - Step 6: send `SIGHUP` to the running process (graceful restart).
   - Step 7: run `sovereign doctor --level basic` on the new binary. If any check fails, **auto-rollback** the binary and exit 2.
5. **Implement `sovereign update`** CLI:
   - `sovereign update` — applies the latest stable.
   - `sovereign update --channel rc` — applies the latest RC.
   - `sovereign update --version 0.1.0` — pins a version.
   - `sovereign update --check` — dry-run, prints what's available, exits 0 if up-to-date, 1 if newer available.
   - `sovereign update --no-verify` — skips the post-update doctor (V0: requires an interactive confirm, V1: deprecated).
6. **Implement `sovereign update rollback`** — reads the last `UpdateRecord`, restores the previous binary from the backup path, sends SIGHUP, runs doctor.
7. **Implement `sovereign update history`** — lists the last 20 updates with version, channel, timestamp, who (which admin user, or "self" if CLI).
8. **Wire the update check into the morning report (G11 in V1, but the **infrastructure** is V0)**: a background task that, on startup and once per day, hits `releases.sovereignruntime.dev/stable.json`. If newer, the binary logs `INFO: sovereign 0.1.1 available; run sovereign update`. The operator chooses when.
9. **Add a `update_check` config** in `sovereign.toml`:
   ```toml
   [update]
   channel = "stable"
   check_on_startup = true
   check_daily = true
   ```
10. **Add Prometheus metrics**:
    - `sovereign_update_check_total{channel,result}` — counter.
    - `sovereign_update_applied_total{channel}` — counter.
    - `sovereign_update_last_applied_timestamp{channel}` — gauge.
    - `sovereign_update_last_rolled_back_total` — counter.
11. **Add a CI test**: a GitHub Actions job runs `cargo build --release`, then a second job runs the binary, calls `update.apply()` against a test release, asserts the new binary is on disk, calls `rollback()`, asserts the old binary is back.

### Code stub

```rust
// crates/sovereign/src/cmd/update.rs
use clap::{Args, Subcommand};
use sovereign_update::{UpdatePort, UpdateChannel, Version};

#[derive(Args, Debug)]
pub struct UpdateCmd {
    #[command(subcommand)]
    pub action: UpdateAction,
}

#[derive(Subcommand, Debug)]
pub enum UpdateAction {
    /// Apply the latest release from the configured channel
    Apply {
        #[arg(long, value_enum, default_value_t = UpdateChannel::Stable)]
        channel: UpdateChannel,
        #[arg(long)] version: Option<String>,
        #[arg(long)] no_verify: bool,
    },
    /// Roll back to the previous binary
    Rollback,
    /// Print available updates without applying
    Check {
        #[arg(long, value_enum, default_value_t = UpdateChannel::Stable)]
        channel: UpdateChannel,
    },
    /// Show the last 20 update records
    History,
}

pub async fn run(cmd: UpdateCmd, ctx: AppContext) -> anyhow::Result<()> {
    match cmd.action {
        UpdateAction::Apply { channel, version, no_verify } => {
            let update = ctx.update.clone();
            let current = update.current_version().await?;
            let latest = update.latest(channel).await?;
            let target = version.map(Version::parse).transpose()?
                .unwrap_or(latest.version);
            if target <= current {
                println!("Already on {} (latest is {}). Nothing to do.", current, latest.version);
                return Ok(());
            }
            let tmp = ctx.config.data_dir.join("cache").join(format!("sovereign-{target}"));
            let sha = update.fetch(&latest.binaries[ctx.target_triple], &tmp).await?;
            let record = update.apply(&tmp).await?;
            if !no_verify {
                // Re-exec and run doctor
                let status = std::process::Command::new("/usr/local/bin/sovereign")
                    .args(["doctor", "--level", "basic"])
                    .status()?;
                if !status.success() {
                    eprintln!("Doctor failed after update; rolling back.");
                    update.rollback(&record).await?;
                    std::process::exit(2);
                }
            }
            println!("Updated {} → {} (sha256: {})", current, target, sha);
            Ok(())
        }
        UpdateAction::Rollback => { /* ... */ Ok(()) }
        UpdateAction::Check { channel } => { /* ... */ Ok(()) }
        UpdateAction::History => { /* ... */ Ok(()) }
    }
}
```

### Tests

- Unit: `apply` then `rollback` returns the binary to the original SHA-256.
- Unit: a manifest with a wrong SHA-256 makes `fetch` return `UpdateError::ChecksumMismatch` and does not modify disk.
- Integration: in a Docker container, `sovereign update --check` reports "up to date" against a fake `stable.json` that matches the running version, "0.1.1 available" against a newer one.
- Integration: `sovereign update --version <new>` against a local fake server swaps the binary; `sovereign update rollback` restores it.
- Integration: after a fake "broken" update (binary that segfaults on `--version`), the post-update doctor catches it and auto-rolls back.
- Property test: applying the same update twice in a row is idempotent (the second call is a no-op).
- CI gate: the GitHub Actions release job publishes a release artifact and runs the apply/rollback test in a clean container.

### Acceptance criteria

- [ ] `sovereign update` applies the latest stable and exits 0 if doctor passes.
- [ ] `sovereign update rollback` restores the previous binary in < 5s.
- [ ] A bad SHA-256 in the manifest fails fast, does not modify the running binary.
- [ ] A broken new binary is auto-rolled-back within 30s.
- [ ] `sovereign update history` shows the last 20 records with version, channel, timestamp.

### Definition of done

A test in CI that:
1. Builds a "v0.1.0" binary, installs it.
2. Publishes a "v0.1.1" with a fixed feature.
3. Calls `sovereign update` and asserts the new binary is on disk, the old is in the backup dir, and the `update_history` table has one row.
4. Breaks the "v0.1.1" binary (replaces the symlink with a shell script that exits 1).
5. Calls `sovereign update` again, asserts the auto-rollback path fires, the "v0.1.0" binary is back, exit code 2.
Passes on every release PR.

---

## 2. The 5-command quickstart (revisited)

After all 8 features ship, the 5-command quickstart must work end-to-end on a fresh Hetzner CX22:

```text
$ curl -sSf sovereignruntime.dev/install.sh | sh
# Downloads sovereign-0.1.0-x86_64-unknown-linux-musl.tar.gz, extracts to /usr/local/bin/sovereign

$ sovereign init
# Detects: pyproject.toml, fastapi
# Writes: app.yaml with framework: fastapi, build: dockerfile, deploy: bluegreen
# Asks: "ready to deploy? (y/n)"

$ sovereign login
# V0: prompts for a one-time token (set by `sovereign-admin create-user` on the server side)
# V1: device-code flow, browser opens to https://auth.sovereignruntime.dev

$ sovereign deploy
# Detects no --image, no git_repo in app.yaml
# V0.1: prompts for image; V0.5: detects Dockerfile, builds via BuildKit
# Pulls, starts, health-checks, returns URL
# Output:
#   ✓ Pulling image: nginx:alpine (1.2s)
#   ✓ Starting container app-api (0.3s)
#   ✓ Health check passed (200 OK, 12ms p50)
#   ✓ Live at: https://<id>.srvr.so
#   Next: sovereign domain add api.mirasaas.com

$ open https://<id>.srvr.so
# TLS cert auto-issued (V0: internal CA, V1: Let's Encrypt)
# 200 OK
```

**Phase 0 is done when this 5-command flow works on a fresh Hetzner CX22, end-to-end, in < 5 minutes.**

---

## 3. Definition of Done (the phase gate)

Phase 0 closes when **all** of these are true:

- [ ] All 10 features (F1-F10) are merged to `main` with passing CI.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo deny check` passes.
- [ ] `cargo audit` reports no new advisories.
- [ ] The 5-command quickstart works on a fresh Hetzner CX22 in < 5 minutes.
- [ ] The auto-rollback test (F8) passes in CI on every PR.
- [ ] `sovereign doctor --level basic` passes on a fresh Hetzner CX22 and fails on a deliberately-broken one.
- [ ] `sovereign update` applies a test release and auto-rolls-back on a broken one.
- [ ] A Show HN post is drafted, ready to publish on the day after the phase closes.
- [ ] The LICENSE is Apache 2.0, unmodified, committed at the repo root.
- [ ] The README has a 1-paragraph description, a "Quickstart" section, and a "Status" section.

**If any of these is false, the phase is not done, regardless of how much code is written.**

---

## 4. What we explicitly do not build in Phase 0

To make the 6-week timeline realistic, the following are **deferred** to later phases and are NOT in scope for Phase 0:

- TUI (V1)
- Multi-server / agent pattern (V1.5)
- Audit log query UI (V1)
- Backup verify (V1.2)
- Web UI (V2)
- OPA/Rego policy (V2)
- ML scorer (V2)
- rqlite (V2)
- OIDC, MFA, SSO (V2)
- Nginx low-mem mode (V1.1)
- Podman runtime (V1.5)
- Service catalog, drift detection (V1.5)
- Pro tier billing (V2)
- EU incorporation (V1.5)
- BSI C5 mapping (V2)
- 10-point sovereignty test in CI (V2)
- Doctor `--level standard|full|paranoid` (V1, V1.5, V2 respectively)
- Doctor fleet/remote mode (V1.5)
- Doctor `--watch` continuous event log (V1)
- Doctor incident toolkit (`sovereign incident declare/note/resolve`, `sovereign retro`, `sovereign game-day`) (V1.5)
- Doctor compliance scan, canary, auto-tune (V2)

If a stakeholder asks for any of these in Phase 0, the answer is in [`negative-prompt.md`](./negative-prompt.md) §3. The full doctor spec is in [`doctor.md`](./doctor.md).

---

**Next: read [`doctor.md`](./doctor.md) for the full diagnostic spec, then [`phase-01-v1.md`](./phase-01-v1.md) for the next 12 weeks of work (which add the standard level, --fix, --explain, and benchmark/cost subcommands).**
