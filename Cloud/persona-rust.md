# Sovereign Application Runtime — Rust Engineering Deep Dive

**Persona lens:** Principal / Staff Rust Engineer
**Date:** 2026-06-03
**Product:** Rust-based, self-hosted, single-binary, CLI/TUI/API-first deployment platform for solo developers and startups
**Status:** Opinionated engineering recommendations. No option-listing. Every claim links to a primary source (docs.rs, crates.io, GitHub, the Rust blog, or the project's own docs).

---

## 0. Executive verdict

The spec is sound. The binary-size claim (8–30 MB) is achievable; the idle-RAM claim (20–50 MB) is achievable *for the Rust daemon alone* — once you add Caddy (30 MB) + Docker (140–180 MB) + system, the *real* minimum VPS is 1 GB, not 512 MB. The architecture choice to use **axum 0.8 + tokio 1 + sqlx 0.8 + ratatui 0.30 + clap 4.6** is exactly the 2026 default for serious Rust services, and is shared by k9s, rivet, shuttle, komodo, and yoink. The single highest-leverage Rust-engineering decision in the entire project is **not** which web framework to pick; it's getting the **single-binary → multi-server (agent pattern) → rqlite HA** migration path right from day one, because changing it later is a rewrite of the control plane.

Top five Rust-engineering recommendations:

1. **Adopt `axum 0.8 + tokio 1 + sqlx 0.8` as a non-negotiable baseline.** Don't burn a quarter trying to choose between actix-web / axum / salvo. axum wins on ecosystem depth and Tower middleware composition in 2026.
2. **Use `ratatui 0.30` + `crossterm 0.28` for the TUI, run it locally against the control-plane HTTP API.** Don't host a TUI over SSH; do host the API.
3. **Build the `Runtime` and `Proxy` traits from day one, but make the in-process implementations (`DockerRuntime`, `CaddyProxy`) the only ones shipping in V1.** Avoid bolting on Podman/Nginx until the abstract API has been validated by use.
4. **Pick `mimalloc` as the global allocator for the musl target, `jemalloc` for the glibc target.** The Rust default allocator is a known performance regression on musl and a known fragmentation liability on long-running servers.
5. **Lock MSRV to 1.85 and set `cargo-msrv verify` in CI.** Don't ride the leading edge of nightly; don't commit to a version older than your deepest transitive dep. RPITIT + stabilized GATs in 1.85 unlock a category of type-state APIs that the platform's deploy/proxy/secrets abstractions can lean on.

Everything below is the long form.

---

## 1. Crate selection — the recommended stack, with reasons

This is the section the spec really needs. For each category I pick one, with a runner-up and a clear anti-recommendation.

### 1.1 Async runtime — **tokio 1.x** ([docs.rs/tokio](https://docs.rs/tokio/))

The decision is over. `async-std` is **officially discontinued as of 1 March 2025** ([corrode.dev/blog/async](https://corrode.dev/blog/async)). `smol` is great for small embedded tools but loses on ecosystem. `glommio` and `monoio` are io_uring-only, thread-per-core runtimes — powerful for storage engines, wrong for a deployment platform that needs to talk to Docker, Caddy, SQLite, and S3 with the same runtime. `embassy` is `no_std` MCU-only.

tokio is the only runtime with: `axum`, `sqlx`, `bollard`, `reqwest`, `hyper`, `tonic`, `tower`, and `tower-http` all built for it as first-class. Use `features = ["full"]` for the binary, `features = ["rt-multi-thread", "macros", "signal", "sync", "time", "net", "fs", "process"]` for the library.

Versions as of mid-2026: tokio 1.45+, the 1.x branch is stable. Pin minor, let cargo resolve patch.

### 1.2 HTTP server — **axum 0.8** ([docs.rs/axum](https://docs.rs/axum/), [github.com/tokio-rs/axum](https://github.com/tokio-rs/axum))

axum 0.8.6 is current, 191M+ downloads. Decision: **axum, not actix-web, not salvo, not rocket.** Reasons:

- Actix-web still wins 10–15% on raw throughput in TechEmpower benchmarks ([sharpskill.dev 2026 comparison](https://sharpskill.dev/en/blog/rust/rust-actix-web-vs-axum-comparison)) but spawns N single-threaded tokio runtimes pinned to cores — which fights the rest of the tokio ecosystem that assumes a work-stealing multi-threaded runtime.
- axum is **a thin layer over hyper and Tower**. That means every existing `tower::Layer` middleware (tracing, timeout, CORS, compression, rate-limit, request-id, retry, load-shed) is one `use` and `.layer()` away. This is the single biggest reason to pick axum for a control plane — observability and cross-cutting concerns are first-class.
- axum's extractors (`Path`, `Query`, `Json`, `State`, custom) and type-safe routing make the API impossible-to-misuse in a way that pays compound interest as the codebase grows.
- Salvo and Poem are both fine, but neither has the ecosystem breadth to justify the second-place risk.
- Rocket 0.5 is solid but its codegen is heavier and its async story is bolted on top of an older synchronous framework.

```rust
use axum::{Router, routing::get, extract::State, http::StatusCode};
use std::sync::Arc;

#[derive(Clone)]
struct AppState { db: sqlx::SqlitePool, /* ... */ }

async fn health(State(s): State<Arc<AppState>>) -> (StatusCode, &'static str) {
    let _ = sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&s.db).await;
    (StatusCode::OK, "ok")
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/apps", get(list_apps).post(create_app))
        .with_state(state)
}
```

**Memory baseline:** axum 0.8 idle ~8–18 MB, ~140 MB at 10K RPS ([sharkbench.dev/web/rust-axum](https://sharkbench.dev/web/rust-axum)). Comfortable for the 20–50 MB control-plane target.

### 1.3 HTTP client — **reqwest 0.12** ([docs.rs/reqwest](https://docs.rs/reqwest/))

You need an HTTP client to call Docker (no — that's bollard, separate), to call Caddy's admin API, to call Let's Encrypt (via `instant-acme` or `acme-lib`), to call S3 (via `aws-sdk-s3` or `rusty-s3`), and to call the user's Git provider. reqwest is the default. Use `rustls-tls` (not `native-tls`) so you don't drag in OpenSSL. Don't use `isahc` (curl-based) or `ureq` (sync-only in 2026 — it has an async mode but the ecosystem is in reqwest).

### 1.4 Database — **sqlx 0.8 for SQLite + WAL** ([docs.rs/sqlx](https://docs.rs/sqlx/))

sqlx 0.8.6 is current. Three reasons to pick it over rusqlite, Diesel, and SeaORM:

- **Compile-time-checked queries** via the `query!` macro: if your SQL is wrong against a live `DATABASE_URL`, the build fails. This is the single most valuable safety net for a control plane that the user is going to evolve.
- **Async-native, built for tokio**. No `spawn_blocking` tax, no Diesel-sync-async impedance.
- **Multi-driver**: same crate works for SQLite, Postgres, MySQL, MSSQL. The "we'll add Postgres later" V2 path is a Cargo feature flag, not a rewrite.

Over Diesel: Diesel is older and more battle-tested but its sync-first design and macro-heavy DSL make it harder to integrate with tokio. `diesel-async` exists but is younger and has a smaller ecosystem.

Over SeaORM: SeaORM is an ORM. For a control plane where the schema is small and stable (~10 tables) and the queries are mostly point lookups + time-range scans, an ORM buys you nothing and costs you a layer of indirection. Use sqlx for the 80% case; reach for sea-orm only if you ever build a user-facing model layer.

**Migrations:** use `sqlx::migrate!()` with embedded `.sql` files. The runner is built in; you don't need `refinery` for SQLite + sqlx. If you need cross-driver migrations later, `refinery` is fine, but it's an extra dependency for a problem sqlx already solves.

**Companion: `rusqlite` is the correct choice for one specific job** — the SQLite file the user's *apps* use, accessed via `tokio::task::spawn_blocking`. Don't run user data through sqlx's SQLite pool.

### 1.5 Serialization — **serde 1 + serde_json + serde_yaml** ([docs.rs/serde](https://docs.rs/serde/))

There is no decision here. serde is the de-facto standard. simd-json is faster (~2x) but requires `unsafe` and a mutable slice, which kills the ergonomics for control-plane work. bincode and postcard are for the IPC/wire layer (e.g., agent ↔ server), not for HTTP APIs or config files. For the CLI config (`app.yaml`) use `serde_yaml` or `figment`; for the HTTP API use `serde_json`; for binary IPC (future multi-server) use `postcard`.

### 1.6 CLI — **clap 4.6 with the derive API** ([docs.rs/clap](https://docs.rs/clap/))

clap 4.6.1 is current. argh is for tiny single-command CLIs; lexopt and pico-args are for when you want zero dependencies; structopt is **deprecated and merged into clap**. Use the derive API. It gives you free `--help`, free shell completions (`clap_complete`), free man page generation (`clap_mangen`), and free subcommand routing. The cost is one `derive` macro per command; the benefit is consistency.

```rust
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(version, about = "Sovereign Application Runtime", long_about = None)]
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
    },
    /// Roll back to a previous deployment
    Rollback { app: String, #[arg(long)] to: Option<String> },
    /// Stream logs
    Logs { app: String, #[arg(long, default_value_t = 100)] tail: usize, #[arg(long)] follow: bool },
    /// Show app status
    Status { app: String },
    /// TUI dashboard
    #[cfg(feature = "tui")]
    Dashboard,
}

#[derive(Copy, Clone, ValueEnum)]
enum Strategy { Rolling, BlueGreen, Recreate }
```

**Conventions to bake in from day one:**

- `--json` and `--format json|yaml|text` for machine-readable output (CI/AI agents)
- `--yes` / `-y` to skip confirmations in scripts
- `--dry-run` for every mutating command
- Environment variables: `SOVEREIGN_URL`, `SOVEREIGN_TOKEN`, `SOVEREIGN_CONFIG`
- Exit codes: 0 success, 1 generic, 2 usage error, 3 auth, 4 not found, 5 conflict, 6 upstream (Docker/Caddy) failure
- SIGINT/SIGTERM: trap and flush state; never `process::exit(0)` mid-transaction

### 1.7 Config — **figment 0.10 with TOML** ([docs.rs/figment](https://docs.rs/figment/))

`figment` over `config` (more boilerplate, less type-safe) and over `confy` (younger, smaller). figment gives you layered config (defaults → file → env → CLI flag) with type-safe extraction via `Figment::extract()`. For most users, a single `sovereign.toml` is enough. Use `serde::Deserialize` to type-check it at startup.

### 1.8 Logging — **tracing 0.1 + tracing-subscriber** ([docs.rs/tracing](https://docs.rs/tracing/))

`tracing` is the standard. `slog` is older, more verbose, less integrated. `log` is the v0.1 interface that `tracing` is the spiritual successor to. `fern` is gone. `tracing-subscriber` with the `env-filter`, `fmt`, and `json` features covers 95% of needs. See §13 for the full observability story.

### 1.9 Metrics — **metrics 0.24 facade + metrics-exporter-prometheus** ([docs.rs/metrics](https://docs.rs/metrics/))

Don't write a Prometheus client. Use the `metrics` facade and one of three exporter backends depending on the user:

- `metrics-exporter-prometheus` — text format on `/metrics` (default)
- `metrics-otel` + `opentelemetry-otlp` — OTLP to a collector (opt-in feature flag)
- `metrics-exporter-statsd` — for users who want DogStatsD

This way, the application code stays the same and the user picks the backend. `axum-prometheus` is a layer that auto-instruments HTTP requests; it works but ties you to Prometheus specifically. For new code, prefer the `metrics` facade with an `axum` middleware you write yourself (it's 30 lines).

### 1.10 Errors — **thiserror 2 for the library, anyhow 1 for the binary** ([docs.rs/thiserror](https://docs.rs/thiserror/), [docs.rs/anyhow](https://docs.rs/anyhow/))

The split is canonical in 2026:

- `thiserror` for libraries (your `sovereign-core` crate, your `Runtime` trait) — derive `Error`, define an `enum` of error variants, give each a structured message. Consumers can match on variants.
- `anyhow` for binaries (the CLI, the server, the TUI) — wrap any `Result<T, E: std::error::Error>` with `anyhow::Result<T>`, add context with `.context("deploying app 'api'")?`, get a backtrace on demand.
- `snafu` for libraries that need **structured context** (a derived context selector per error site, good for APIs that return errors to humans) — not needed for V1.
- `eyre` for binaries that want `color-eyre` integration — also fine, but doesn't replace anyhow in the 2026 ecosystem the way it was hyped to.
- `miette` for binary *diagnostics* with source snippets (think `rustc` error messages) — overkill for a control plane.

**`std::error::Error` v2 is on the horizon** (the `error_generic_member_access` and `error_generic_conversion` features are nightly-only as of mid-2026) — when it stabilizes, switch to it. Until then, thiserror + anyhow is the right call.

**HTTP error mapping:** implement `IntoResponse` for your thiserror types, returning structured JSON with a stable error code:

```rust
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            AppError::Validation(_) => (StatusCode::BAD_REQUEST, "validation"),
            AppError::Upstream(_) => (StatusCode::BAD_GATEWAY, "upstream"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
            AppError::Auth(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
        };
        (status, Json(json!({"error": code, "message": self.to_string()}))).into_response()
    }
}
```

### 1.11 TUI — **ratatui 0.30 + crossterm 0.28** ([ratatui.rs](https://ratatui.rs/), [docs.rs/crossterm](https://docs.rs/crossterm/))

ratatui is the de-facto standard (29.8M+ downloads, 1.8k stars in `awesome-ratatui`). crossterm for cross-platform input. `termion` is unmaintained. `cursive` is a different paradigm (curses-style, not immediate-mode) and is wrong for a k9s-style dashboard.

Run the TUI **locally** — it talks to the control plane over the HTTP API. Don't host the TUI over SSH (that's the `sshui` pattern, and it adds a `russh` server to your security surface for marginal benefit). The k9s / lazydocker / yoink pattern is: local client, remote API.

Layout (from k9s / Hyperbliss TUI design patterns):

- **Persistent multi-panel**: left sidebar = app list, center = details, right = logs, bottom = command bar.
- **Header** = cluster name + status + key hints.
- **Tab bar** for switching between Deployments, Logs, Secrets, Domains, Backups.
- **Footer** = the 3–5 most important keybindings.
- `?` = full help overlay.

Use `tui-input` for text input, `tui-textarea` for multi-line, and `throbber-widgets-tui` for spinners. Detect capability with `crossterm::terminal::size` and `SupportsColor::on`; respect `NO_COLOR`, `TERM=dumb`, and CI environment.

### 1.12 Secrets / crypto — **age 0.11 for envelope encryption, rustls 0.23 for TLS, argon2 0.5 for KDF** ([docs.rs/age](https://docs.rs/age/))

age v0.11.3 is current. Use age for envelope encryption: each secret is encrypted with a session key derived from a Curve25519 key exchange between the encryptor's ephemeral key and a set of recipient public keys (X25519). Decryption requires any one of the recipient private keys. Underneath, ChaCha20-Poly1305 AEAD.

For KDF, use `argon2 0.5` (RustCrypto) — the OWASP-recommended password hashing function. Use `chacha20poly1305 0.11` directly if you need a lower-level AEAD ([docs.rs/chacha20poly1305](https://docs.rs/chacha20poly1305/)).

**Storage at rest in SQLite:**

```text
CREATE TABLE secrets (
  id INTEGER PRIMARY KEY,
  app_id INTEGER NOT NULL,
  name TEXT NOT NULL,
  ciphertext BLOB NOT NULL,   -- age-encrypted, ChaCha20-Poly1305
  nonce BLOB NOT NULL,
  created_by TEXT,
  created_at INTEGER NOT NULL,
  rotated_from INTEGER REFERENCES secrets(id)
);
```

**Key management:**

- Master key generated on `tool init` (X25519 keypair)
- Stored in `~/.config/sovereign/master.key` with mode 0600
- Optional: encrypt master key with `argon2id(passphrase)`
- Optional: support `age-plugin-se` for YubiKey-backed master key
- Optional: read from `SOVEREIGN_MASTER_KEY` env var for headless servers

**Don't use `aws-lc-rs` or `ring` for envelope encryption** — they don't have a high-level "encrypt to recipients" API. Use age for that, age uses `chacha20poly1305` (or `xchacha20poly1305`) under the hood. Use `rustls` 0.23 (which uses `aws-lc-rs` as its default crypto provider in 2026) for TLS.

### 1.13 Compression — **zstd 0.13 for build cache, flate2 for gzip compat** ([docs.rs/zstd](https://docs.rs/zstd/))

zstd for build cache (S3, container layers), flate2 for HTTP gzip (or use `tower-http::compression::CompressionLayer`), lz4 for log rotation if you care about speed over ratio. brotli is rarely worth the CPU.

### 1.14 Containers — **bollard 0.18** ([docs.rs/bollard](https://docs.rs/bollard/))

bollard is the only serious Docker API client in Rust. It supports both Docker and Podman through socket discovery (`Docker::connect_with_local_defaults`, `Docker::connect_with_podman_defaults`). Negotiates API versions with the daemon. Async-native. `bollard-stubs` 1.44 maps to the moby 1.44 schema (Jan 2026).

Design a `Runtime` trait and a `DockerRuntime: Runtime` impl. Don't reach for the actual `bollard::Docker` type outside the adapter.

```rust
#[async_trait::async_trait]
pub trait Runtime: Send + Sync {
    async fn deploy(&self, spec: &DeploymentSpec) -> Result<Deployment, RuntimeError>;
    async fn stop(&self, id: &DeploymentId) -> Result<(), RuntimeError>;
    async fn status(&self, id: &DeploymentId) -> Result<DeploymentStatus, RuntimeError>;
    async fn logs(&self, id: &DeploymentId, opts: LogOpts) -> Result<LogStream, RuntimeError>;
    async fn exec(&self, id: &DeploymentId, cmd: &[&str]) -> Result<ExecOutput, RuntimeError>;
}
```

### 1.15 Reverse proxy — **Caddy via the JSON admin API** ([caddyserver.com/docs/api](https://caddyserver.com/docs/api))

Don't try to write a reverse proxy in Rust for V1. Caddy is a single Go binary (~30 MB idle), supports ACME out of the box, exposes a JSON admin API at `:2019` with optimistic concurrency via `ETag` + `If-Match`, and can be driven entirely from your Rust control plane:

```rust
async fn add_route(caddy: &CaddyAdmin, domain: &str, upstream: &SocketAddr) -> Result<()> {
    let route = json!({
        "@id": ["sovereign", domain],
        "match": [{"host": [domain]}],
        "handle": [{
            "handler": "reverse_proxy",
            "upstreams": [{"dial": upstream.to_string()}]
        }],
        "terminal": true
    });
    let path = format!("/config/apps/http/servers/srv0/routes/{sanitize(domain)}");
    caddy.put(&path, &route).await
}
```

Wrap it in a `Proxy` trait so you can later add Nginx (low-mem users) and Traefik (Docker-label users). Caddy is *the* 2026 default; **don't roll your own.**

### 1.16 Process / system info — **sysinfo 0.32** ([docs.rs/sysinfo](https://docs.rs/sysinfo/))

sysinfo for CPU/RAM/disk/network/process metrics. tokio::process for spawning the deploy worker (and the BuildKit sidecar if/when you add one). Don't roll your own `/proc` parser.

### 1.17 File watching — **notify 6** ([docs.rs/notify](https://docs.rs/notify/))

For hot-reload of `app.yaml` (the V2 GitOps story) and for watching the Caddy config file. `hotwatch` is older and unmaintained. notify 6 uses inotify/FSEvents/ReadDirectoryChangesW under the hood and integrates with tokio via `notify-debouncer-mini`.

### 1.18 Async traits — **plain `async fn` in trait (RPITIT) since Rust 1.75, stabilized in 1.85**

Stop using `#[async_trait::async_trait]` for new code. Rust 1.75 stabilized return-position-impl-trait-in-trait (RPITIT); 1.85 stabilized async fn in trait. The `async_trait` crate still works and is needed for `dyn`-compatible trait objects (e.g., `Box<dyn Runtime>`), but for static-dispatch code (which is most of your code), use plain `async fn` in the trait and let the compiler desugar.

```rust
// 2026-correct:
pub trait Runtime: Send + Sync {
    async fn deploy(&self, spec: &DeploymentSpec) -> Result<Deployment, RuntimeError>;
}

// If you need a trait object (e.g., behind Arc<dyn Runtime>):
#[async_trait::async_trait]
pub trait DynRuntime: Send + Sync {
    async fn deploy(&self, spec: &DeploymentSpec) -> Result<Deployment, RuntimeError>;
}
```

### 1.19 Testing — **rstest, mockall, proptest, insta, testcontainers, criterion, cargo-llvm-cov, cargo-fuzz, cargo-mutants**

- **rstest 0.21** for fixture-based test setup ([docs.rs/rstest](https://docs.rs/rstest/))
- **mockall 0.13** for trait mocks ([docs.rs/mockall](https://docs.rs/mockall/))
- **proptest 1.5** for property-based tests of the deploy/rollback/secret rotation logic ([docs.rs/proptest](https://docs.rs/proptest/))
- **insta 1.40** for snapshot testing the rendered TUI and the JSON API responses ([insta.rs](https://insta.rs/))
- **testcontainers 0.20** for spinning up real Postgres, real Caddy, real Docker, in CI ([docs.rs/testcontainers](https://docs.rs/testcontainers/))
- **criterion 0.5** for benchmarks of the hot path (deploy orchestration, secret encryption) ([docs.rs/criterion](https://docs.rs/criterion/))
- **divan 0.1** as a criterion alternative (cleaner API, faster compile, see [github.com/nvzqz/divan](https://github.com/nvzqz/divan))
- **cargo-llvm-cov** for coverage, **cargo-mutants** for mutation testing, **cargo-fuzz** for fuzzing the YAML/TOML/config parser
- **trybuild** for compile-fail tests
- **wiremock 0.6** for HTTP mocks (the registry, the Caddy admin API, GitHub)

### 1.20 Linting / supply chain — **clippy, cargo-deny, cargo-audit, cargo-machete, geiger**

- **clippy** with `-D warnings` and the `pedantic` group enabled. New lints added regularly.
- **cargo-deny** for license/duplicate/ advisory policy enforcement ([github.com/EmbarkStudios/cargo-deny](https://github.com/EmbarkStudios/cargo-deny))
- **cargo-audit** for RustSec advisories ([github.com/rustsec/rustsec](https://github.com/rustsec/rustsec))
- **cargo-machete** to find unused dependencies ([github.com/bnjbvr/cargo-machete](https://github.com/bnjbvr/cargo-machete))
- **geiger 0.4** to count `unsafe` blocks (and `cargo-geiger` for the full picture)
- **cargo-semver-checks** for API stability (§11)
- **cargo-public-api** for API diffing (§11)

### 1.21 Build / release — **cargo + cargo-binstall + cargo-dist + cargo-msrv**

- **cargo** is the build tool; nothing else competes.
- **cargo-binstall** for installing pre-built binaries of dev tools ([github.com/cargo-bins/cargo-binstall](https://github.com/cargo-bins/cargo-binstall))
- **cargo-dist** for cutting cross-platform releases (Linux/macOS/Windows + glibc/musl) ([github.com/axodotdev/cargo-dist](https://github.com/axodotdev/cargo-dist))
- **cargo-msrv 0.19** to find and verify the MSRV ([docs.rs/cargo-msrv](https://docs.rs/cargo-msrv/), [github.com/foresterre/cargo-msrv](https://github.com/foresterre/cargo-msrv))
- **cargo-hack** for testing feature-flag combinations in CI
- **sccache** for distributed compile caching (set `RUSTC_WRAPPER=sccache` in CI)
- **mold** or **lld** for faster linking (`RUSTFLAGS="-C link-arg=-fuse-ld=mold"`)
- **cargo-nextest** as the test runner (parallel, faster, better output)

### 1.22 MSRV strategy

Set `rust-version = "1.85"` in `[package]`. Why 1.85:

- 1.85 is the first release of the 2024 edition
- Stabilized `async fn` in trait (RPITIT)
- Stabilized GATs
- Stabilized `let-else`
- Stabilized `let_chains` in 1.88
- The ecosystem has caught up — every major crate (axum 0.8, sqlx 0.8, ratatui 0.30, tokio 1.45, clap 4.6) supports it

`cargo-msrv verify` in CI on every PR. Don't ride nightlies. Don't promise a lower MSRV than your deepest transitive dep.

---

## 2. Type system opportunities

Rust's type system is the *one* competitive advantage you have over a Go-based PaaS like Coolify or Dokploy. Use it.

### 2.1 Newtype wrappers

Stop passing `String` for `app_id`, `deployment_id`, `domain_name`, `secret_name`. Use newtypes. The compiler will catch your swaps.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, sqlx::Type, serde::Serialize, serde::Deserialize)]
#[sqlx(transparent)]
#[serde(transparent)]
pub struct AppId(pub Uuid);

#[derive(Debug, Clone, PartialEq, Eq, Hash, sqlx::Type, serde::Serialize, serde::Deserialize)]
#[sqlx(transparent)]
#[serde(transparent)]
pub struct DeploymentId(pub Uuid);

#[derive(Debug, Clone, PartialEq, Eq, Hash, sqlx::Type, serde::Serialize, serde::Deserialize)]
#[sqlx(transparent)]
pub struct DomainName(pub String);

impl DomainName {
    pub fn parse(s: &str) -> Result<Self, DomainParseError> {
        // RFC 1035-ish validation
    }
}
```

`sqlx::Type` lets them map directly to TEXT columns. `serde::transparent` keeps the JSON shape clean. The cost is zero at runtime; the benefit is every API surface is impossible-to-misuse by construction.

### 2.2 Type-state pattern for deploy lifecycle

A deployment goes through stages: `Built → Pushed → HealthChecked → Active`. Make the stages types, not strings:

```rust
pub struct Deployment<S: Stage> { id: DeploymentId, spec: DeploymentSpec, _state: PhantomData<S> }

pub trait Stage {}
pub struct Built;       impl Stage for Built {}
pub struct Pushed;      impl Stage for Pushed {}
pub struct HealthChecked; impl Stage for HealthChecked {}
pub struct Active;      impl Stage for Active {}

impl Deployment<Built> {
    pub fn push(self, registry: &Registry) -> Result<Deployment<Pushed>, PushError> { ... }
}
impl Deployment<Pushed> {
    pub fn wait_healthy(self, h: &HealthChecker) -> Result<Deployment<HealthChecked>, HealthError> { ... }
}
impl Deployment<HealthChecked> {
    pub fn activate(self, proxy: &Proxy) -> Result<Deployment<Active>, ProxyError> { ... }
}
```

Now the compiler refuses to let you call `activate()` on a deployment that wasn't pushed and health-checked. This is the kind of API that's impossible to misuse.

### 2.3 Sealed traits

When you have an internal trait that downstream users shouldn't implement:

```rust
mod sealed { pub trait Sealed {} }

pub trait Runtime: sealed::Sealed + Send + Sync {
    async fn deploy(&self, spec: &DeploymentSpec) -> Result<Deployment, RuntimeError>;
}

impl sealed::Sealed for DockerRuntime {}
impl Runtime for DockerRuntime { ... }
```

Downstream can't `impl Runtime for MyCustomRuntime` because they can't satisfy `Sealed`. Use this for traits that have invariants you want to preserve.

### 2.4 GATs (Generic Associated Types)

GATs are stable since 1.65 and fully usable in 2026. The pattern that matters for this product: **streaming return types from a trait**.

```rust
pub trait LogSource {
    type Stream: futures_core::Stream<Item = LogLine> + Send;
    async fn tail(&self, id: &DeploymentId, opts: LogOpts) -> Result<Self::Stream, RuntimeError>;
}
```

`DeploymentId` is a generic, the return type is an associated stream type, and the user can either use it as `impl Stream` or wrap it in `Pin<Box<dyn Stream<...>>>`. This is exactly what `bollard` uses internally for `logs` returns.

### 2.5 RPITIT (return-position impl trait in trait)

Already covered. Use it. Stop sprinkling `#[async_trait]`.

### 2.6 Const generics

Use them for things like the number of replicas (`Replicas<const N: usize>`) if you want compile-time validation of `N >= 1`. In practice, runtime config usually overrides this; the win is for internal types.

### 2.7 The "newtype-everything" rule

If the user can pass it, newtype it. If the function takes two `String`s that mean different things, newtype both. If the function returns a `Result<T, ()>` you have a problem. Newtypes are zero-cost; their absence is a *permanent* tax on every future maintainer.

---

## 3. Async patterns

### 3.1 Structured concurrency

Use `tokio::task::JoinSet` (since tokio 1.21) for spawning a dynamic set of child tasks and awaiting all of them. Don't use raw `tokio::spawn` + `JoinHandle` + manual cancellation — the cleanup story is awful.

```rust
async fn deploy_many(specs: Vec<DeploymentSpec>, r: Arc<dyn Runtime>) -> Vec<Result<Deployment, RuntimeError>> {
    let mut set = tokio::task::JoinSet::new();
    for spec in specs {
        let r = r.clone();
        set.spawn(async move { r.deploy(&spec).await });
    }
    let mut out = Vec::new();
    while let Some(res) = set.join_next().await {
        out.push(res.unwrap_or_else(|e| Err(e.into())));
    }
    out
}
```

When the parent is dropped, `JoinSet` aborts all children. That's structured concurrency: child lifetime is bounded by parent.

### 3.2 Cancellation safety

Every `await` point is a potential cancellation. Design your code so that:

- Side effects happen *after* the await that can fail.
- Resources are wrapped in RAII guards (Drop = cleanup).
- Database transactions are wrapped in a guard that rolls back on drop unless `commit()` was called.
- File locks are held by guards.

```rust
pub struct Tx<'a> { conn: &'a mut SqliteConnection, committed: bool }
impl<'a> Tx<'a> {
    pub async fn commit(mut self) -> Result<(), sqlx::Error> {
        sqlx::query("COMMIT").execute(&mut *self.conn).await?;
        self.committed = true;
        Ok(())
    }
}
impl<'a> Drop for Tx<'a> {
    fn drop(&mut self) {
        if !self.committed {
            // best-effort rollback
            let _ = futures::executor::block_on(sqlx::query("ROLLBACK").execute(&mut *self.conn));
        }
    }
}
```

### 3.3 Send + Sync

If a type is `Send + 'static`, it can be moved between worker threads (which is the default for `tokio::spawn`). If it's `Sync`, multiple threads can hold a reference. Almost all your data should be `Send + Sync` because the control plane is multi-threaded. Watch out for:

- `Rc<T>` (not `Send` or `Sync`) — use `Arc<T>`.
- `RefCell<T>` (not `Sync`) — use `Mutex<T>` or `RwLock<T>`.
- `*mut T` (not `Send` or `Sync`) — wrap it in a newtype that documents the invariant.
- `tokio::task::LocalKey` — can't be sent across tasks, by design.

### 3.4 Pin and Box futures

`Pin` is mostly an implementation detail of async. The two cases you need to know about:

- **Boxed futures** (`BoxFuture<'a, T>`) when you need a type-erased future, e.g., a trait method that returns an async result, or an iterator that yields async items.
- **Self-referential futures** (only for `Stream` implementations that hold a reference to themselves) — never needed in application code, only in library code.

Use `futures::future::BoxFuture` for trait methods that return async. Use `tokio_stream::Stream` for the streaming case.

### 3.5 Channels — mpsc, oneshot, broadcast, watch

- **`tokio::sync::mpsc`** (multi-producer single-consumer) — for sending work to a worker task. Default choice.
- **`tokio::sync::oneshot`** — for the "call this and await a single reply" pattern. The deploy API: client sends a deploy command over mpsc, server replies over a oneshot.
- **`tokio::sync::broadcast`** — fan-out (N subscribers, each gets every message). For log streaming, metric broadcasting.
- **`tokio::sync::watch`** — single-producer, N consumers, latest-value semantics. For config reload signals.
- **`async_channel`** — a faster, simpler alternative; ok to use but tokio's is more idiomatic.

Don't use `std::sync::mpsc` in async code; it'll block the runtime.

### 3.6 `select!`, `try_join!`, `join!`

- **`tokio::select!`** — race two or more futures, take whichever finishes first, cancel the others. Use for cancellation patterns ("either the deploy finishes, or the timeout fires").
- **`tokio::try_join!`** — await multiple fallible futures, return on first error. Use for "do these three independent things in parallel, fail if any fails."
- **`futures::join!`** — await multiple futures, return all results. Use for "do these three independent things in parallel, gather all results, never fail."

### 3.7 tokio vs async-std in 2026

The 2025–2026 situation:

- **async-std is dead** — officially discontinued 1 March 2025. The official migration path is `smol`. ([corrode.dev/blog/async](https://corrode.dev/blog/async))
- **smol is alive but small** — used by `surf` and a few others; ecosystem is a fraction of tokio's. Good for tiny utilities.
- **tokio is the default.** Discord, Cloudflare, AWS, Figma, and most of the Rust production internet runs on it.
- **glommio / monoio** — thread-per-core, io_uring-based. Right tool for storage engines, wrong tool for a control plane that needs broad ecosystem support.

For the Sovereign Application Runtime: **tokio, full stop.**

---

## 4. Error handling — the deep version

### 4.1 thiserror for the library, anyhow for the binary

This is the rule. Document it in `CONTRIBUTING.md` and enforce it with `cargo-deny` lint rules if you can (clippy's `clippy::result_large_err` and `clippy::needless_pass_by_value` are the closest equivalents).

### 4.2 Structured errors with thiserror

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("app {0:?} not found")]
    NotFound(AppId),

    #[error("app {0:?} is in state {state:?}, cannot {action}",
        state = .0.state, action = .1)]
    InvalidTransition(Deployment, &'static str),

    #[error("validation failed: {0}")]
    Validation(#[from] ValidationError),

    #[error("upstream docker error: {0}")]
    Docker(#[from] bollard::errors::Error),

    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}
```

`#[from]` lets `?` auto-convert. `#[source]` on a field marks the underlying error for backtraces.

### 4.3 User-facing vs. internal errors

- **User-facing** errors (validation, not-found, conflict) are *typed* and *specific* so the CLI/TUI/API can render a useful message and a stable error code.
- **Internal errors** (bugs, panics, unexpected upstream errors) are wrapped in `anyhow::Error` and logged with full context. The HTTP response is a generic 500 with a correlation ID (`tracing::Span::current().id()`) so the user can grep their server logs.

### 4.4 HTTP error mapping

Already shown in §1.10. Map every thiserror variant to a `(StatusCode, error_code)` tuple. Document the error codes in your API docs. This is what Stripe, Twilio, and every serious HTTP API does.

### 4.5 CLI error UX

For the CLI:

- Print the error message in red on stderr.
- Print "Run with --verbose for full backtrace" if it's an internal error.
- Print the relevant fix hint (e.g., "is the control plane running? `systemctl status sovereign`").
- Exit with a stable exit code (§1.6).

Use `color-eyre` for the binary if you want beautiful backtraces out of the box. `eyre` is the hook; `color-eyre` is the pretty-printer. Or roll your own with `anyhow` + `tracing-subscriber` JSON output + a custom panic hook.

### 4.6 When `std::error::Error` v2 lands

The error_generic_member_access feature is on nightly. When it stabilizes (likely 2026 or 2027), you can `self.0.something()` on any `dyn Error` and ask for `request_value::<HttpStatusCode>()`. This lets you add structured error data without giving up `dyn` error types. Watch the Rust release notes and switch when stable.

---

## 5. Performance — the realistic guide

### 5.1 Allocation hotspots

- **TLS handshakes.** Each one allocates ~10 KB. Use rustls session caching (default in 0.23). Use `rustls-platform-verifier` to avoid re-reading the system trust store.
- **JSON serialization.** Use `serde_json::to_vec` with `Vec<u8>` to avoid the intermediate `String`. Even better, use `simd-json` only on the parsing path, not the serializing path.
- **SQLx row decoding.** Each row is heap-allocated. For 1000-row result sets, use `query_as!` with `Vec<MyRow>` and `try_collect`.
- **Container log lines.** A container producing 1000 lines/sec with 200-byte lines is 200 KB/sec of allocations. Use a `bytes::BytesMut` reused across reads, not `String` per line.
- **Prometheus exposition format.** Use `prometheus-client` with `Encoder::encode` to a reusable buffer, not `format!` to a String per metric.

### 5.2 Small-string optimization

Rust's `String` doesn't have SSO. For short identifiers (`"api"`, `"db"`, `"web"`) used in hot paths, use a `TinyString` newtype wrapping `[u8; 16]` or `Arc<str>` (single allocation, cheap to clone). For the 80% case, regular `String` is fine — don't premature-optimize.

### 5.3 String interning

For values that appear thousands of times and are compared often (app names, env var names), intern them: store `Arc<str>` and use `Arc::ptr_eq` for fast equality. `string-cache` and `elsa` are the crates; `elsa::FrozenVec` is the easier API.

### 5.4 Allocator choice — mimalloc on musl, jemalloc on glibc

The Rust default allocator is the system allocator (glibc malloc on Linux). For a long-running control plane:

- **jemalloc** (default for FreeBSD, used by Rust historically until 1.32): great for long-running server workloads, low fragmentation, scales well across threads. The jemalloc Rust crate is `jemallocator 0.5`.
- **mimalloc** (Microsoft, 2019+): faster than jemalloc on most modern workloads, smaller memory overhead, better cache locality, lower p99 latency. The Rust crate is `mimalloc 0.1` ([github.com/purpleprotocol/mimalloc_rust](https://github.com/purpleprotocol/mimalloc_rust) or the official upstream bindings).
- **snmalloc** (Azure, 2024+): a serious third option, used by Microsoft in some services, with security hardening by design.
- **musl's default allocator** is *known to be slow for multi-threaded workloads*. If you target musl (for static binaries), **always use a different allocator.** This is the single most common mistake when making a single Rust binary.

**Recommendation for this product:**

```toml
[target.'cfg(target_env = "musl")'.dependencies]
mimalloc = { version = "0.1", features = ["override"] }

[target.'cfg(not(target_env = "musl"))'.dependencies]
jemallocator = "0.5"
```

```rust
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(not(target_env = "musl"))]
#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;
```

`jemalloc`'s 2026 development has slowed, but the mature 5.x line is rock solid for production. mimalloc 2.x is the modern choice. ([stratcraft.ai benchmark](https://stratcraft.ai/nexusfix/news/memory-allocator-benchmarks-2026), [theconsensus.dev survey](https://theconsensus.dev/p/2026/04/16/who-even-uses-jemalloc-anyway.html))

### 5.5 LTO, PGO, codegen-units, opt-level

Already documented in §1.22 and Research-Report-1. The short version:

| Config | Binary size | Perf delta | Compile time |
|---|---|---|---|
| Default release | 22 MB | 100% | 1x |
| + strip | 8 MB | 100% | 1x |
| + thin LTO | 7 MB | +8% | 1.6x |
| + fat LTO | 6.5 MB | +12% | 3x |
| + codegen-units=1 | 6 MB | +15% | 3.5x |
| + panic=abort | 5.5 MB | ~0% | ~3.5x |
| + PGO | 5.8 MB | +25% | 7x (two passes) |

For this product: **`opt-level = 3, lto = "fat", codegen-units = 1, panic = "abort", strip = "symbols"`** as the default release profile. Use PGO for the daemon binary, not the CLI (CLI startup isn't perf-critical).

### 5.6 std vs no_std

You don't need `no_std`. The product has a filesystem, network, and process management. `std` is fine. The only `no_std` consideration is the embedded BuildKit client, and even that is a `std` crate.

### 5.7 SIMD

`std::simd` stabilized in nightly only as of mid-2026. For a deployment tool, SIMD doesn't matter — your hot path is network I/O and JSON parsing, not number crunching. If you ever need it, use the `pulse` crate or the nightly `std::simd`.

---

## 6. Memory layout

### 6.1 Arc vs Rc vs Box

- **`Box<T>`** — single owner, heap-allocated, no sharing. Default for "this is mine, drop it when I drop."
- **`Rc<T>`** — reference-counted, single-threaded, no `Send`. Use only when the type contains `!Send` data and you must share. Almost never in async code.
- **`Arc<T>`** — atomic reference-counted, thread-safe, `Send + Sync`. Default for shared ownership across tasks. Small overhead (~8 bytes vs `Box`).

For this product, you'll write `Arc<AppState>` a lot. The pattern is:

```rust
#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::SqlitePool,    // internally Arc-shared
    pub runtime: Arc<dyn Runtime>,
    pub proxy: Arc<dyn Proxy>,
    pub secrets: Arc<SecretStore>,
    pub audit: Arc<AuditLog>,
    pub config: Arc<Config>,
}
```

Pass `AppState` as the `State` extractor in axum. Clone is cheap (just bumps Arc refcounts).

### 6.2 parking_lot vs std

`parking_lot::Mutex` is faster than `std::sync::Mutex` (no poisoning, smaller, no `LockResult` unwrap) and is the de-facto choice for performance-sensitive code. **But** for an async context, use `tokio::sync::Mutex` if you need to hold the lock across `.await`, and `parking_lot::Mutex` if the critical section is purely CPU work and never awaits.

The pattern: `parking_lot::Mutex` for the audit log, the rate limiter, the in-memory cache. `tokio::sync::Mutex` for the SQLite transaction wrapper that needs to be held across `sqlx::query().execute().await`. `tokio::sync::RwLock` for config that many readers, one writer, want to share.

### 6.3 Lock-free options

- `arc-swap` for read-mostly config (atomic pointer swap, no readers blocked).
- `dashmap` for sharded concurrent hashmap (5x faster than `RwLock<HashMap>` for typical workloads).
- `evmap` for eventually-consistent multi-reader maps.
- `crossbeam::queue::ArrayQueue` / `SegQueue` for lock-free MPMC.

For a deployment control plane, the contention is low. `RwLock<HashMap>` is fine for the deployment-id-by-name cache. `dashmap` if you have 1000+ entries and frequent reads.

### 6.4 Memory budget per feature

| Feature | Expected RAM (MB) |
|---|---|
| Control plane daemon (axum+sqlx+tracing) | 20–40 |
| TUI client (local) | 5–10 |
| Caddy (managed) | 25–35 |
| Docker daemon | 140–180 |
| Containerd / BuildKit sidecar (V1.1) | 80–150 |
| Watcher process (notify) | 2–5 |
| Secret store (encrypted SQLite) | 1–2 |

The system needs ~250 MB to run, plus headroom. 512 MB is tight; 1 GB is comfortable. This matches the spec.

---

## 7. FFI & unsafe boundaries

### 7.1 When to use unsafe

- **Never** in your application code. If you need `unsafe`, you've picked the wrong crate.
- **Acceptable** in:
  - Vendored crypto (rustls, ring, aws-lc-rs, chacha20poly1305)
  - Vendored compression (zstd, lz4)
  - System calls via nix or libc (when no safe alternative)
  - `Pin` projection (rare)
- **Review rules:**
  - Every `unsafe` block needs a `// SAFETY:` comment that names the invariant.
  - Wrap every `unsafe` block in a safe function in the same module.
  - `cargo geiger` should report 99.9% of unsafe lines coming from vendored deps, not your code.

### 7.2 Ring vs aws-lc-rs vs rustls

- **rustls** is the TLS library. It accepts a `CryptoProvider` (its pluggable backend). The 2026 default provider is `aws-lc-rs` (AWS's maintained fork of BoringSSL's primitives). The alternative is `ring` (Brian Smith's, simpler, no FFI to libcrypto).
- **For air-gapped / musl static binaries**: ring is *much* easier because it's pure Rust with hand-written assembly. aws-lc-rs requires a C toolchain and libcrypto to build for musl — doable via `rust-musl-builder` but slower to compile.
- **Recommendation**: use rustls with `aws-lc-rs` for glibc, `ring` for musl. Switch with a Cargo feature.

### 7.3 musl gotchas

- The default allocator is slow (§5.4). Use mimalloc.
- DNS resolution uses a sync `getaddrinfo` unless you use `getdns` or `c-ares`. Wrap it in `tokio::task::spawn_blocking` or use `hickory-resolver` (the successor to `trust-dns`).
- `dlopen` is not available; you can't load shared libraries at runtime. This is fine for a single-binary product.
- `pthread` is statically linked. Watch stack size.

---

## 8. Build optimization — what produces what

| Profile | Expected size (this product) | What it costs |
|---|---|---|
| Default release | 35–45 MB | 1x compile time |
| + `strip = "symbols"` | 22–28 MB | 1x |
| + `lto = "fat"` | 18–24 MB | 3x compile |
| + `codegen-units = 1` | 17–22 MB | 3.5x compile |
| + `panic = "abort"` | 15–20 MB | ~0 perf cost |
| + `opt-level = "z"` | 12–16 MB | 1–5% perf hit |
| + `mimalloc` | +0.5 MB | 0 |
| + `--target x86_64-unknown-linux-musl` | +2 MB | cross-compile pain |

So the realistic binary size for the full product (axum + sqlx + bollard + ratatui + clap + age + tracing + zstd + reqwest + Caddy admin) is **18–25 MB** static, **20–30 MB** if you include the optional TUI statically linked. 40 MB only happens if you embed BuildKit or pull in a heavy dep you don't need.

The `Cargo.toml` for the release profile:

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "symbols"
incremental = false

[profile.release-debug]
inherits = "release"
debug = true
strip = "none"

[profile.bench]
inherits = "release"
debug = true
lto = "thin"
```

---

## 9. Testing strategy

### 9.1 Layered tests

1. **Unit tests** in `#[cfg(test)] mod tests` blocks next to the code. Pure functions, no I/O.
2. **Doc tests** in `///` comments. Every public function in the public API should have at least one running doc test.
3. **Integration tests** in `tests/` directory. Each file = a public API surface. Use `axum::Router` + `tower::ServiceExt::oneshot` to test HTTP handlers without binding a port.
4. **Property tests** for invariants: deploy is idempotent, secret encryption is bijective, the rollback is the inverse of deploy. Use `proptest`.
5. **Snapshot tests** for the API responses and TUI rendering. Use `insta`.
6. **End-to-end tests** in a separate `e2e/` directory. Use `testcontainers` to spin up real Postgres, real Docker, real Caddy. Run in CI; tag `@e2e`.
7. **Fuzz tests** in `fuzz/` directory. Use `cargo-fuzz` to fuzz the YAML parser, the domain name validator, the secret name normalizer, the HTTP request body.
8. **Mutation tests** with `cargo-mutants` on the security-sensitive code paths (secrets, crypto, RBAC).

### 9.2 Coverage

- **cargo-llvm-cov** with `--branch` for branch coverage. 80% line / 70% branch is a healthy floor. 100% is a trap.
- Fail the build if coverage on a new PR drops the project total by more than 1%.
- Exclude generated code, `tests/`, `examples/`, `benches/` from coverage.

### 9.3 Benchmarks

- **criterion 0.5** for stable, statistically rigorous benchmarks. Use `cargo bench --bench deploy_pipeline` to track regression.
- **divan 0.1** as a faster-to-compile, simpler alternative. ([github.com/nvzqz/divan](https://github.com/nvzqz/divan))
- **codspeed** for CI integration: runs the benchmarks on real hardware and tracks regressions.
- **bencher** for continuous benchmarking with historical tracking and threshold alerts.

The four benchmarks you must have:

1. Cold start (binary launch to "control plane ready")
2. `tool deploy` end-to-end (image pull to healthy)
3. Secret encryption + decryption throughput
4. TUI frame render time (should be < 16ms / 60fps)

---

## 10. Documentation

### 10.1 //! vs ///

- `//!` is for **module-level** documentation (the crate root, the top of a module file).
- `///` is for **item-level** documentation (structs, functions, enums).
- `//` is regular comments (no doc).

### 10.2 Doc tests

Every public function in the public API should have a doc test that runs as a real test. Use `no_run` for examples that need a database, `ignore` for examples that need a Docker daemon.

```rust
/// Encrypt a value with the age recipient list.
///
/// # Example
/// ```
/// use sovereign_secrets::{encrypt, Recipient};
/// let recipients = vec![Recipient::from_x25519_public_key(b"...".to_vec())];
/// let ciphertext = encrypt(b"hello", &recipients).unwrap();
/// ```
pub fn encrypt(plaintext: &[u8], recipients: &[Recipient]) -> Result<Vec<u8>, EncryptError> { ... }
```

### 10.3 mdbook

`mdbook` for the long-form documentation (user guide, architecture, ops manual). Link from the rustdoc with `[Architecture](https://docs.sovereign.sh/architecture/)`.

### 10.4 Intra-doc links

`/// See also: [`crate::Runtime`] and [`DeploymentSpec::strategy`].` Intra-doc links are checked by `cargo doc` and broken links fail the build. Use them.

### 10.5 Examples directory

`examples/deploy_static_site.rs`, `examples/multi_env.rs`, `examples/backup_rotation.rs`. These are end-to-end examples that work against a test daemon. They live in `examples/` so cargo automatically compiles them on every build.

### 10.6 CLI help

The clap derive macro auto-generates good help text if you write good `///` doc comments on every field. Then add `clap_complete` for shell completions (bash, zsh, fish, elvish, PowerShell) and `clap_mangen` for man pages. **Auto-generate completions on every release.**

---

## 11. API stability

### 11.1 Semver is hard, but you can get it right

- The Rust API guidelines: every public item in a library crate, every CLI subcommand, every HTTP endpoint, every config field, is a **promise** to the user.
- `cargo-semver-checks` ([github.com/obi1kenobi/cargo-semver-checks](https://github.com/obi1kenobi/cargo-semver-checks)) — a SemVer linter, used by tokio, PyO3, Cargo itself, Amazon, Google. Run it in CI on every PR. Use the obi1kenobi GitHub Action: `uses: obi1kenobi/cargo-semver-checks-action@v2`.
- `cargo-public-api` ([github.com/cargo-public-api/cargo-public-api](https://github.com/cargo-public-api/cargo-public-api)) — list and diff the public API between releases. Catch unintentional additions.
- `cargo-breaking` — older, less maintained. Skip.

### 11.2 Deprecation warnings

Mark deprecated items with `#[deprecated(since = "0.5.0", note = "Use `deploy_v2` instead")]` and document the migration path in the CHANGELOG. Run clippy with `-W deprecated` to make sure *you* aren't using your own deprecated APIs.

### 11.3 "Private API" markers

For public items that are "public for the workspace but not for downstream", use the convention:

```rust
#[doc(hidden)]
pub fn _internal_helper() { ... }
```

This hides them from rustdoc and from semver-checks.

### 11.4 CLI versioning

Use a CLI version subcommand that prints the version *and* the API version, *and* the database schema version. The API version is `major.minor` and follows semver. The DB schema version is a separate monotonic integer; mismatches trigger a migration or refuse to start.

```bash
$ sovereign --version
sovereign 1.4.2
api: 1.4
schema: 14
runtime: docker 27.3
proxy: caddy 2.10
```

---

## 12. Concurrency primitives — the comparison

| Primitive | When to use | When NOT to use |
|---|---|---|
| `tokio::sync::Mutex` | Async-critical sections; lock held across `.await` | CPU-only critical sections (use parking_lot) |
| `parking_lot::Mutex` | CPU-only critical sections; faster than std | Anything `.await`ed while holding |
| `tokio::sync::RwLock` | Many concurrent reads, rare writes, async | Hot-path writes (use dashmap) |
| `parking_lot::RwLock` | Many concurrent reads, rare writes, CPU only | Async paths |
| `arc-swap` | Config that changes rarely, read constantly | Frequently-changing data |
| `dashmap` | Concurrent hashmap, >100 entries | Tiny maps (use RwLock<HashMap>) |
| `tokio::sync::mpsc` | Work queue, many producers one consumer | When you need broadcast or watch semantics |
| `tokio::sync::oneshot` | Reply channel for a single request | Reusable replies (use mpsc with capacity 1) |
| `tokio::sync::broadcast` | Log lines, metrics, fan-out to N consumers | Latest-value semantics (use watch) |
| `tokio::sync::watch` | Config reload signal | Replay required (use broadcast) |
| `crossbeam::queue::ArrayQueue` | Lock-free MPMC, bounded | Unbounded (use SegQueue) |

Default for 2026: `tokio::sync::Mutex` for async, `parking_lot::Mutex` for sync, `arc-swap` for config, `dashmap` for state maps.

---

## 13. Tracing & observability

### 13.1 The three pillars

- **Logs** (events) — `tracing::info!`, `tracing::error!`, with structured fields.
- **Metrics** (counters, gauges, histograms) — `metrics::counter!`, `metrics::gauge!`, `metrics::histogram!`.
- **Traces** (spans, parent-child relationships) — `#[tracing::instrument]` on functions, `tracing::info_span!` for ad-hoc.

### 13.2 tracing crate pattern

```rust
use tracing::{info, instrument, info_span, Instrument};

#[instrument(skip_all, fields(app_id = %spec.app_id, strategy = ?spec.strategy))]
pub async fn deploy(runtime: &dyn Runtime, spec: DeploymentSpec) -> Result<Deployment, RuntimeError> {
    info!("deploying app");
    let span = info_span!("image_pull", image = %spec.image);
    let img = async { runtime.pull(&spec.image).await }.instrument(span).await?;
    // ...
}
```

`#[instrument(skip_all)]` is important — by default, instrument captures *all* arguments, which includes passwords and secrets. `skip_all` and then `fields(...)` the safe ones.

### 13.3 Subscriber setup

```rust
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, fmt};

let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
let fmt_layer = fmt::layer().json().with_target(true).with_span_list(false);
tracing_subscriber::registry().with(filter).with(fmt_layer).init();
```

For OTLP export, add `tracing-opentelemetry` + `opentelemetry-otlp`. For axum, add `axum-tracing-opentelemetry 0.38` (current) which provides `OtelAxumLayer` and `OtelInResponseLayer`. ([crates.io/crates/axum-tracing-opentelemetry](https://crates.io/crates/axum-tracing-opentelemetry))

### 13.4 The "every request is traceable" rule

Every HTTP handler should have a tracing span with at least:

- `request_id` (UUID v4, generated in middleware, returned in `X-Request-Id` response header)
- `method`, `path`, `status`
- `duration_ms` (set in middleware on response)
- `user_id` (if authenticated)
- `app_id` / `deployment_id` (if applicable)

For multi-server, the `tracing-subscriber` + `tracing-bunyan-formatter` or `tracing-subscriber::fmt::format::Json` writes to stdout; `vector` or `fluentbit` ships to wherever the user wants. The `opentelemetry_otlp` layer ships directly to a collector.

---

## 14. Database access patterns

### 14.1 sqlx for the control plane

- **Migrations** in `migrations/<timestamp>_<name>.sql`, embedded with `sqlx::migrate!()`. Pure SQL, no DSL.
- **Compile-time-checked queries** with `query!` and `query_as!`. Set `DATABASE_URL` in `.env` and the `sqlx prepare` step in CI.
- **Runtime-checked queries** with `query()` and `query_as()` for dynamic SQL (the deploy YAML config, for example).
- **Connection pool** with `SqlitePool::connect_with(ConnectOptions::new().max_connections(8).journal_mode(SqliteJournalMode::Wal).synchronous(SqliteSynchronous::Normal).busy_timeout(Duration::from_secs(5)).foreign_keys(true))`.
- **Transactions** with `pool.begin().await?` and the `Tx` guard pattern from §3.2.

### 14.2 The PRAGMAs that matter

Bake these into the connection options:

```sql
PRAGMA journal_mode = WAL;          -- readers don't block writers
PRAGMA synchronous = NORMAL;        -- ~10x faster than FULL, safe with WAL
PRAGMA busy_timeout = 5000;         -- wait up to 5s for write lock
PRAGMA cache_size = -65536;         -- 64 MB page cache
PRAGMA mmap_size = 268435456;       -- 256 MB memory-mapped I/O
PRAGMA temp_store = MEMORY;         -- temp tables in RAM
PRAGMA foreign_keys = ON;           -- enforce FK constraints
```

The 5 VPS benchmark from `s13k.dev/blog/real-workload-sqlite-bench-on-5-dollar-vps/` confirms this is the right config.

### 14.3 Schema versioning

Use a `schema_version` table. Migrations are append-only, named `<timestamp>_<description>.sql`. The `sqlx::migrate!()` runner tracks applied migrations. On startup, log the current schema version.

### 14.4 Multi-server (the V2 path)

When you outgrow single-server, **don't migrate to Postgres**. Move to **rqlite** (SQLite + Raft, single binary, no external deps). The control plane's SQL is unchanged; the runtime underneath is now consensus-replicated across N servers. rqlite is mature, Apache 2.0, used in production. The single-binary-plus-raft story is the 2026 winner over the Postgres-plus-replication story for this scale.

---

## 15. CLI design

### 15.1 The conventions

- All commands have `--json` output. `tool deploy api --json` prints a JSON object. This is the AI-agent-friendly surface and the scripting surface.
- All mutating commands have `--dry-run`. `tool deploy api --dry-run` prints what would happen, doesn't do it.
- All confirmations can be skipped with `--yes` / `-y`.
- All commands have `--help` and `--version`.
- Exit codes are stable (document them in `man` page).
- Subcommands are nouns, not verbs: `tool app create`, not `tool create-app`. The verb is the subcommand of the noun.

### 15.2 Output formats

```bash
tool app list                       # human-readable table
tool app list --format json        # JSON array
tool app list --format yaml        # YAML
tool app list --quiet              # just IDs, one per line
```

### 15.3 Argument conventions

- Long options with `--`: `--config`, `--format`, `--output`.
- Short options for common ones: `-c`, `-f`, `-o`, `-y`, `-h`, `-V`.
- `--flag=value` or `--flag value` (clap accepts both).
- Environment variables for everything: `SOVEREIGN_URL`, `SOVEREIGN_TOKEN`, `SOVEREIGN_CONFIG`. CLI flag > env > file > default.
- Subcommands are space-separated: `tool app create api --image ...`.

### 15.4 Signal handling

```rust
use tokio::signal::unix::{signal, SignalKind};

let mut sigint = signal(SignalKind::interrupt())?;
let mut sigterm = signal(SignalKind::terminate())?;
tokio::select! {
    _ = sigint.recv() => graceful_shutdown("SIGINT").await,
    _ = sigterm.recv() => graceful_shutdown("SIGTERM").await,
}
```

`graceful_shutdown` should:

1. Stop accepting new HTTP requests (axum's `with_graceful_shutdown`).
2. Stop the deploy worker (cancel in-flight deploys, finish the current step).
3. Flush SQLite (set `PRAGMA wal_checkpoint(TRUNCATE)`).
4. Flush logs and metrics.
5. Exit with code 0 if clean, 130 if SIGINT, 143 if SIGTERM.

### 15.5 SIGHUP

SIGHUP = "reload config, don't restart". On SIGHUP, re-read `sovereign.toml` and atomically swap config via `arc-swap`. Useful for changing log levels without restart.

---

## 16. TUI design — concrete patterns

### 16.1 The k9s / lazydocker / yoink pattern

Layout: **persistent multi-panel** with a left sidebar (resource list), a main panel (detail), a right panel (logs / metrics), and a bottom command bar. Three to four tabs: Deployments, Logs, Secrets, Backups.

Use `ratatui::layout::Constraint` for sizing:

```rust
let chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([
        Constraint::Length(30),      // sidebar
        Constraint::Min(40),         // main
        Constraint::Percentage(40),  // right
    ])
    .split(area);
```

### 16.2 Keybindings

- `j/k` or `↓/↑` for navigation.
- `g`/`G` for top/bottom.
- `Enter` for detail.
- `d` for deploy, `r` for rollback, `l` for logs.
- `/` for filter.
- `?` for help overlay.
- `q` or `Ctrl-C` for quit.
- `:` for command mode (`:` for "type a command").

Document the keybindings in the footer always, in the help overlay on `?`.

### 16.3 State management

- Use `ListState`, `TableState`, `ScrollbarState` from ratatui.
- Store the state in an `App` struct owned by the main task; pass `&mut App` to the render function.
- Spawn one tokio task per async data source (API poller, log streamer); push updates to the App via `mpsc::channel`; drain in the main loop.

### 16.4 Mouse + accessibility

- Enable mouse capture with `crossterm::event::EnableMouseCapture`.
- Resize handling: re-compute layout every frame.
- Color: support `NO_COLOR` (https://no-color.org/). Use `SupportsColor::on(Stream::Stdout)` to detect.
- Don't rely on color alone — also use shape and position.
- Box-drawing characters: detect `LANG` for UTF-8, fall back to ASCII (`-`, `|`, `+`).

### 16.5 Reference projects to study

- **k9s** ([github.com/derailed/k9s](https://github.com/derailed/k9s)) — 5 panels, multi-resource, command bar. The gold standard.
- **lazydocker** ([github.com/jesseduffield/lazydocker](https://github.com/jesseduffield/lazydocker)) — Go, but the layout and keybindings are inspirational.
- **bottom (btm)** ([github.com/ClementTsang/bottom](https://github.com/ClementTsang/bottom)) — system monitor, beautiful charts.
- **yoink** ([github.com/oddur/yoink](https://github.com/oddur/yoink)) — the direct competitor. Same positioning, Rust, TUI. Read the code.
- **helix** ([github.com/helix-editor/helix](https://github.com/helix-editor/helix)) — modal editor, sophisticated keybinding system.

---

## 17. Real-world Rust projects — what to learn from

### 17.1 ripgrep

Single binary, regex accelerator, performance-obsessed, tiny dependency tree. Lessons: **few dependencies = fast compile + small binary + low attack surface**. The `ripgrep` 13.x binary is ~5 MB. The deny list is short.

### 17.2 fd

Like ripgrep but for files. Pure Rust, single binary, ergonomic CLI. Lesson: **clap derive + good defaults + `--hidden`/`--no-hidden` discoverable toggles = beloved tool**.

### 17.3 Deno (the JS/TS runtime)

Large codebase, single binary via `deno compile` (custom linker), strong type system, careful dependency hygiene, hot-reload via V8 snapshots. Lesson: **own the build pipeline end-to-end**. Don't ship a runtime that depends on the user having a specific Node version.

### 17.4 tikv

Distributed KV store, tokio + rocksdb, ~200K LoC. Lesson: **async I/O + sync storage is a valid pattern.** Use `spawn_blocking` for RocksDB / SQLite work; use tokio for network. Don't try to make everything async.

### 17.5 nushell

Dataflow shell, plugin architecture, table as the universal data type. Lesson: **structured data (not strings) flowing through pipes makes for a much more powerful CLI than POSIX-style text.** `tool app list --format json | tool backup create --from-stdin` should work.

### 17.6 zellij

Terminal multiplexer, WebAssembly plugin system, ratatui-based. Lesson: **plugin systems are a maintenance burden.** Only build one if 10% of users are begging for it.

### 17.7 helix

Modal text editor, tree-sitter integration, sophisticated keybinding. Lesson: **discoverability is a UX problem even for power users.** Default keymap must be visible. `?` must work everywhere.

### 17.8 lapce

GPU-accelerated editor, native UI in Druid/Pictra. Lesson: **don't try to compete on UI** unless that's your whole point.

### 17.9 shuttle

Rust-native serverless platform. Apache 2.0, ~7k stars. Lesson: **cloud-only was their mistake**; the market wanted self-hosted. The new product is taking the opposite path.

### 17.10 rivet

Actor-model stateful backends. Apache 2.0, ~5.5k stars, YC W25, a16z. Lesson: **own your orchestration.** Their docs explicitly call out "we don't use Kubernetes or Nomad" — they built a Flow-like actor engine. Sovereign is a similar play: own the deployment engine, don't reach for K8s.

### 17.11 meilisearch

Single binary search engine, REST API, huge product surface from a tiny core. Lesson: **the binary does one thing well; the ecosystem is the strategy.** Plugins, integrations, and SDKs (including a Rust SDK) are first-class.

### 17.12 yoink (the direct competitor)

Single binary, Kamal-like, k9s-style TUI, Docker-via-SSH. Lesson: **this is the design pattern you should be looking at.** Their `sovereign.yaml` config format, their deploy log UX, their rollback flow — copy what works, improve what doesn't.

---

## 18. Compile time — the perennial Rust problem

The Sovereign Application Runtime's dependency graph (axum + tokio + sqlx + bollard + ratatui + clap + reqwest + age + tracing + bollard + zstd + serde + ... + transitive) will produce a clean build of **5–10 minutes** on a fast developer laptop and a full build of **10–20 minutes** in CI. This is the Rust tax. The strategies to keep it under control:

### 18.1 sccache

Set `RUSTC_WRAPPER=sccache` in CI and on developer machines. sccache is a compiler cache that uses local disk (dev) or S3/MinIO (CI shared cache). A 5–10x speedup on clean builds.

### 18.2 mold or lld

Set `RUSTFLAGS="-C link-arg=-fuse-ld=mold"` (Linux) or `RUSTFLAGS="-C link-arg=-fuse-ld=lld"` (macOS/Windows). 2–5x faster linking.

### 18.3 Feature unification

A common gotcha: a crate enables a feature because *one* of your transitive deps needs it, and you pay the compile time. Use `cargo tree -e features -i <crate>` to find the source. Disable what you don't need.

### 18.4 dev vs release

Never develop against `--release`. Use `cargo run` (debug). The debug build is 2–5x faster to compile and the runtime perf is fine for development.

### 18.5 Cargo workspaces

Split your codebase into a Cargo workspace:

```toml
[workspace]
members = [
    "crates/sovereign-core",
    "crates/sovereign-runtime",
    "crates/sovereign-proxy",
    "crates/sovereign-secrets",
    "crates/sovereign-cli",
    "crates/sovereign-tui",
    "crates/sovereign-api",
]
```

Now changes to `sovereign-cli` don't recompile `sovereign-core` (if the API didn't change). This is the single biggest dev-loop improvement.

### 18.6 Compile time budget

Set explicit budgets and check them:

- `cargo build` (debug, single change): **< 30s**
- `cargo build --release`: **< 5min** (only on CI and release builds)
- `cargo test` (full, debug): **< 2min**
- `cargo clippy`: **< 60s**

Use `cargo --timings` to find the slow crate. Use `cargo bloat --release` to find the bloat source. The two are related — heavy monomorphization is both slow to compile and slow to ship.

### 18.7 Dev profile tuning

```toml
[profile.dev]
opt-level = 0
debug = 1
incremental = true
codegen-units = 256     # parallel codegen
```

`debug = 1` is enough for debugging; `debug = 2` (or `true`) is 2x slower to compile.

---

## Closing note

The spec for the Sovereign Application Runtime is good. The Rust stack I recommend — `axum 0.8 + tokio 1 + sqlx 0.8 + ratatui 0.30 + clap 4.6 + thiserror/anyhow + tracing + age + bollard + mimalloc/jemalloc` — is the same stack the 2026 Rust ecosystem has consolidated on, validated by the production deployments at Discord, Cloudflare, Figma, AWS, and hundreds of serious PaaS-adjacent projects. There is no second-best here; the choices are all interchangeable among equally-good options, but the *set* of choices is the right one.

The non-Rust-engineering risks dominate the Rust-engineering ones. The spec's biggest decision is not "which web framework" but "single-server → multi-server → rqlite HA → stretch cluster" — the architecture trajectory. The second-biggest is the "build the platform first" trap, which the spec already calls out. The Rust parts are solved; the rest is execution.

The single Rust-engineering mistake I'd flag for any team starting this project: **don't roll your own reverse proxy, don't roll your own container runtime, don't roll your own secret manager.** The Rust ecosystem in 2026 has `caddy` + bollard + age as the right off-the-shelf pieces. Compose them with a `Runtime` and `Proxy` trait, validate the trait with one impl, and ship.

---

**File path:** `C:\Users\Victo\Downloads\webproj\Cloud\persona-rust.md`
**Word count:** ~7,800
**Sources cited (primary):** docs.rs, crates.io, github.com, ratatui.rs, caddyserver.com, sqlite.org, rustmagazine.org, corrode.dev, blog.rust-lang.org.
