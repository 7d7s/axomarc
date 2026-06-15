# Tech Stack

**Status:** Locked for V0/V1. Changes require an ADR.
**Last updated:** 2026-06-04

This document is the **crate and version contract**. Every `Cargo.toml` in the workspace must conform. If a new crate is needed, this document is updated first and an ADR is filed.

---

## 1. The locked table (V0/V1)

| Category | Pick | Version pin | Why | Anti-pick |
|---|---|---|---|---|
| **Language edition** | Rust 2024 | `edition = "2024"` | Modern syntax, latest features | 2021 (loses RPITIT ergonomics) |
| **MSRV** | 1.85 | `rust-version = "1.85"` | RPITIT + GATs stable; plain `async fn` in trait | 1.80 (RPITIT nightly only), 1.90 (newer than 1.85 ecosystem) |
| **Async runtime** | `tokio` | `1.45+` | Default ecosystem; axum/sqlx/bollard all built for it | `async-std` (discontinued Mar 2025), `smol`, `glommio` |
| **HTTP server** | `axum` | `0.8` | Tower middleware is the single biggest reason for a control plane | `actix-web` (fights tokio work-stealing), `salvo`, `poem`, `rocket` |
| **HTTP client** | `reqwest` | `0.12` (`rustls-tls`) | Default; no OpenSSL drag | `isahc`, `ureq` |
| **Database** | `sqlx` | `0.8` (compile-time checked) | Async-native, multi-driver, embeddable | `rusqlite` (sync, needs `spawn_blocking`), `diesel` (macro-heavy), `sea-orm` (ORM tax) |
| **Serialization** | `serde` + `serde_json` + `serde_yaml` + `postcard` | `1`, `1`, `0.9`, `1` | Standard | `simd-json` (unsafe), `bincode` (worse for IPC) |
| **CLI** | `clap` (derive) + `clap_complete` + `clap_mangen` | `4.6+` | Free `--help`, shell completions, man pages | `argh`, `lexopt`, `structopt` (deprecated) |
| **Config** | `figment` | `0.10` (TOML) | Layered (defaults → file → env → CLI); type-safe | `config`, `confy`, `envy` |
| **Logging** | `tracing` + `tracing-subscriber` | `0.1`, `0.3` (env-filter, fmt, json) | Standard, structured, span-aware | `slog`, `log`, `fern` (gone) |
| **Metrics** | `metrics` + `metrics-exporter-prometheus` | `0.24`, `0.16` | Backend-agnostic facade | `prometheus` (tying to one backend) |
| **Errors (library)** | `thiserror` | `2` | Canonical for libraries | `snafu`, `eyre`, `miette` (overkill) |
| **Errors (binary)** | `anyhow` | `1` | Canonical for binaries | `snafu`, `eyre` |
| **TUI** | `ratatui` + `crossterm` | `0.30`, `0.28` | De-facto standard; k9s/lazydocker/yoink pattern | `cursive`, `termion` (unmaintained) |
| **Secrets (encryption)** | `age` + `argon2` | `0.11`, `0.5` | Modern, simple, no GPG | `orion`, `ring` (low-level) |
| **Compression** | `flate2`, `lz4`, `zstd` | `1`, `1`, `0.13` | Standard | `brotli` (slow), `snap` |
| **Containers (Docker API)** | `bollard` | `0.18` | Tokio-native Docker API | `docker-rust` (older) |
| **Build / release** | `cargo` + `cargo-dist` + `cargo-binstall` | latest | Reproducible, cross-platform | `cargo-make` (overkill) |
| **Allocator (musl)** | `mimalloc` | `0.4` | 15-60% perf win on alloc-heavy workloads | `std` (regression on musl) |
| **Allocator (glibc)** | `jemallocator` | `0.5` | Same; better for long-running services | `std` |
| **Testing** | `tokio-test`, `mockall`, `wiremock`, `testcontainers`, `proptest`, `criterion`, `divan`, `insta`, `trybuild`, `cargo-fuzz`, `cargo-mutants`, `cargo-llvm-cov`, `codspeed` | latest | The 2026 standard stack | None of the above is "wrong"; the list is what works |
| **Linting** | `clippy`, `cargo-deny`, `cargo-audit`, `cargo-machete`, `cargo-geiger` | latest | The discipline of release-grade Rust | None |
| **Git (V1.5+)** | `gix` | `0.70+` | Pure-Rust, fast, no libgit2 | `git2` (libgit2 C dep) |
| **Policy engine (V2+)** | `opa` (WASM) + `regorus` (Rust Rego) | `0.70`, `0.3` | OPA standard; regorus for in-process eval | Custom DSL (forbidden) |
| **ML scorer (V2+)** | `prophet` (Rust port) → `timesfm` (Rust bindings, V2.5) | TBD | Time-series anomaly detection | Hand-rolled (too brittle) |

---

## 2. The release profile (10-25 MB binary, not 30+)

```toml
# Cargo.toml — workspace root
[profile.release]
opt-level = "z"        # or 3 for speed; "z" for size
lto = "fat"            # link-time optimization across all crates
codegen-units = 1      # better optimization, slower compile
panic = "abort"        # no unwinding tables; smaller binary
strip = "symbols"      # strip debug symbols from release
incremental = false    # deterministic builds

[profile.release.package."*"]
opt-level = "z"        # same for every dep that we control

[profile.dist]
inherits = "release"
strip = "symbols"
lto = "fat"
codegen-units = 1

[workspace.metadata.cargo-dist]
targets = [
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-unknown-linux-gnu",  # glibc fallback for older distros
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
]
```

**Targets priority (V0):** `x86_64-unknown-linux-musl` only. We add targets as the user demand validates them. Apple Silicon and glibc come in V1.

**Reference:** [carlriis benchmark](https://carlriis.com/blog/2024-08-03-rust-binary-size.html), [Leapcell release optimization](https://leapcell.io/blog/optimizing-rust-binary-size-techniques-for-building-lightweight-applications).

### 2.1 The footprint claim (binary, RSS, VPS fit)

**Locked claim for V1+** (per the competitive analysis in `competitive-landscape.md` Gap 1 — "Coolify is too heavy, PIER is too thin, we're the middle"):

| Metric | Target | Why |
|---|---|---|
| **Binary size** | 10-25 MB static musl | One-line install: `chmod +x && ./sovereign` |
| **Idle RSS (single binary)** | 80-150 MB | Measured on a Hetzner CX22, 0 apps deployed, after 5 min warmup |
| **Total system RSS (binary + Caddy + Docker + SQLite)** | 200-350 MB | The full V0/V1 stack; 1 app running adds ~50 MB |
| **Minimum VPS** | **512 MB RAM, 1 vCPU** | Hetzner CX22 (€4.49/mo) is the canonical target; runs V0 with 1 app |
| **Recommended VPS (V0)** | 1 GB RAM, 2 vCPU | Hetzner CX22 (€4.49/mo) or CAX11 arm64 (€4.85/mo) |
| **Recommended VPS (V1.5 fleet)** | 4 GB RAM, 2 vCPU per server | Hetzner CX32 (€7.59/mo) for the control plane; agents can be CX22 |

**The claim is verified in CI** on every release:
- A fixture `cx22-small` in `tests/fixtures/footprint/` runs the binary with 0 apps, 1 app, 5 apps, 12 apps; the test asserts the RSS is within the budget.
- The output is uploaded as a release artifact: `footprint-<version>.json` with per-app-count RSS, RSS-after-1h, RSS-after-24h.
- The footprint comparison vs. Coolify / Dokploy / CapRover / PIER is published at `/docs/comparison/footprint.md` (per `competitive-landscape.md` Gap 1).

**The claim is part of the marketing message**: "80-150 MB RAM. Fits in 512 MB VPS. Rust single binary. No panel." This is the headline on the landing page and in every Show HN post.

---

## 3. The global allocator (one line, 15-60% perf win)

```rust
// src/main.rs (composition root)
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(not(target_env = "musl"))]
#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;
```

**Why split:** `mimalloc` is faster on short-lived, alloc-heavy workloads (musl static binaries with no system allocator). `jemalloc` is faster on long-running services with many concurrent allocations. We pick the right one per target.

**Reference:** [theeditorial.news allocator benchmarks](https://theeditorial.news/posts/rust-allocators-performance-comparison/) — measured 15-60% improvement on representative workloads.

---

## 4. The lockfile discipline

- `Cargo.lock` is committed to git. Always. No exceptions.
- We use `cargo update --workspace --precise <version>` to pin a specific version of a transitive dep when security demands it. The PR includes a 1-line explanation of why.
- We do not run `cargo update` on a Friday. Updates land on Monday and have the full week to be reverted.
- We do not introduce a new direct dependency without an ADR. (Transitive deps are accepted as they come.)
- `cargo-deny` runs in CI with this `deny.toml`:

```toml
# deny.toml
[advisories]
db-path = "~/.cargo/advisory-db"
db-urls = ["https://github.com/rustsec/advisory-db"]
yanked = "deny"

[licenses]
allow = [
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "MIT",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unicode-DFS-2016",
    "Unicode-3.0",
    "Zlib",
    "CC0-1.0",
    "MPL-2.0",  # acceptable for Firefox-derived crates
]
confidence-threshold = 0.8

[bans]
multiple-versions = "warn"
wildcards = "deny"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

---

## 5. Workspace-level Cargo.toml

```toml
# Cargo.toml — workspace root
[workspace]
resolver = "2"
members = [
    "crates/sovereign",
    "crates/sovereign-core",
    "crates/sovereign-runtime-docker",
    "crates/sovereign-proxy-caddy",
    "crates/sovereign-secrets-age",
    "crates/sovereign-storage-sqlite",
    "crates/sovereign-backup",
    "crates/sovereign-notify",
    "crates/sovereign-observability",
    "crates/sovereign-proto",
    # V1.5+: add "crates/sovereign-runtime-podman", "crates/sovereign-proxy-nginx"
    # V2+:    add "crates/sovereign-storage-rqlite", "crates/sovereign-policy-rego", "crates/sovereign-ml-scorer"
]

[workspace.package]
version = "0.1.0"  # bumped to 1.0.0 at end of V1
edition = "2024"
rust-version = "1.85"
license = "Apache-2.0"
repository = "https://github.com/sovereignruntime/sovereign"
homepage = "https://sovereignruntime.dev"
authors = ["Sovereign Application Runtime Contributors"]
keywords = ["paas", "deployment", "self-hosted", "sovereign", "cli"]
categories = ["command-line-utilities", "deployment", "web-programming::http-server"]

[workspace.dependencies]
# The locked versions for direct dependencies
tokio = { version = "1.45", features = ["full"] }
axum = { version = "0.8", features = ["macros", "tracing"] }
tower = { version = "0.5", features = ["util"] }
tower-http = { version = "0.6", features = ["trace", "cors", "timeout"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
sqlx = { version = "0.8", features = ["runtime-tokio-rustls", "sqlite", "macros", "migrate", "json", "uuid", "chrono"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
clap = { version = "4.6", features = ["derive", "env", "string"] }
clap_complete = "4.6"
clap_mangen = "0.2"
figment = { version = "0.10", features = ["toml", "env"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt", "json", "registry"] }
metrics = "0.24"
metrics-exporter-prometheus = { version = "0.16", default-features = false }
thiserror = "2"
anyhow = "1"
ratatui = "0.30"
crossterm = "0.28"
age = "0.11"
argon2 = "0.5"
rand = "0.8"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
bollard = { version = "0.18", default-features = false, features = ["tls_rustls"] }
# V1.5+: gix = "0.70"
# V2+:    opa-wasm = "0.2", regorus = "0.3"
```

---

## 6. The .cargo/config.toml

```toml
# .cargo/config.toml (workspace root)
[build]
rustflags = [
    "-C", "link-arg=-fuse-ld=mold",  # faster linking on Linux
    "-C", "target-cpu=native",        # build for the build host
]
target-dir = "target"

[target.x86_64-unknown-linux-musl]
linker = "rust-lld"
rustflags = [
    "-C", "link-self-contained=yes",
    "-C", "link-arg=-static",
    "-C", "panic=abort",
]

[profile.dev]
opt-level = 0
debug = true
incremental = true
```

**Why these settings:**
- `mold` linker: 3-5x faster linking on Linux.
- `target-cpu=native` for dev: faster compile, faster test runs. Release builds use the cross-compile target.
- `link-self-contained=yes` for musl: no system libc at runtime, truly static binary.
- `panic=abort` for musl: smaller binary, no unwinding tables, deterministic behavior on panic (important for a deployment tool).

---

## 7. The testing stack (V0 ships with 7 tools, V1 adds 5)

| Tool | Use | Phase |
|---|---|---|
| `#[tokio::test]` | Async tests | V0 |
| `mockall` | Mock the `Runtime`, `Proxy`, `Storage` ports | V0 |
| `insta` | Snapshot testing for CLI output and JSON | V0 |
| `trybuild` | Compile-fail tests for macro/derive APIs | V0 |
| `cargo-llvm-cov` | Code coverage with branch data | V0 |
| `cargo-fmt --check` | Formatting in CI | V0 |
| `clippy --all-targets --all-features -- -D warnings` | Lints in CI | V0 |
| `proptest` | Property-based testing for state machines | V0.1 |
| `wiremock` | Mock HTTP servers (Caddy admin, Docker daemon) | V0.2 |
| `testcontainers` | Real Postgres / Caddy / S3 in tests | V1 |
| `criterion` / `divan` | Microbenchmarks | V1 |
| `codspeed` / `bencher` | Continuous benchmarking in CI | V1 |
| `cargo-fuzz` | LibFuzzer / AFL++ for untrusted input | V1 |
| `cargo-mutants` | Mutation testing | V1.5 |
| `rstest` | Fixture-based test parameters | V1.5 |

**CI matrix (V0):**

```yaml
# .github/workflows/ci.yml (excerpt)
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.85
      - run: rustup component add clippy rustfmt
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo test --all-features
      - run: cargo llvm-cov --all-features --lcov --output-path lcov.info
      - run: cargo install cargo-deny && cargo deny check
      - run: cargo install cargo-audit && cargo audit
```

---

## 8. The compile-time discipline

- **Workspace dependency unification** via `cargo-hakari` (V1+) to avoid duplicate builds of the same crate under different feature flags.
- **Feature flags per adapter** in the binary crate: `--features docker,caddy,sqlite`. Default features = `["docker", "caddy", "sqlite"]`. V1.1 adds `nginx`, V1.5 adds `podman`, V2 adds `rqlite`, `rego`, `ml`.
- **`sccache` in CI** for distributed compilation cache.
- **Incremental compilation** in dev only. Release builds are always `incremental = false`.
- **One target per CI job** to keep build matrices parallel: linux-musl-x86_64, linux-musl-aarch64, linux-gnu-x86_64, darwin-aarch64, darwin-x86_64.

---

## 9. The forbidden list (anti-picks, explicit)

These crates, patterns, and tools are **explicitly forbidden** in the codebase. They are not "we don't use them"; they are "if you add them, your PR is rejected."

| Anti-pick | Why forbidden | When (if ever) to revisit |
|---|---|---|
| `async-std` | Discontinued Mar 2025 | Never |
| `smol`, `glommio` | Ecosystem fragments | Never |
| `actix-web` | Fights tokio work-stealing; middleware story is worse | Never |
| `diesel` | Sync, macro-heavy, doesn't fit async use cases | Never |
| `sea-orm` | ORM tax; we want SQL | Never |
| `rusqlite` directly in `sovereign-core` | Sync; needs `spawn_blocking` everywhere | Storage adapter only |
| `lazy_static!`, `once_cell::Lazy` in domain | Hidden global state | Adapter code only, behind a port |
| `serde_json::Value` in domain types | Untyped | Wire format at the edges only |
| `tokio::spawn` from inside a use case that outlives the use case | Hidden async runtime, untestable | Use cases are short-lived; long-running services live in the binary |
| `unsafe` outside audited modules | Memory safety is the point | Audited, fuzz-tested, with a comment |
| `unwrap()` in domain or use cases | Hides errors | Adapter code may use `.expect("msg")` with a reason |
| Custom policy DSL | OPA/Rego exists | V2+: regorus in-process, OPA WASM for advanced |
| Custom container runtime | Docker/Podman exist | Never |
| K8s client (`kube-rs`) | K8s is forbidden | Never |
| gRPC / tonic | We are REST + JSON + SSE | Never |
| WebSockets in the control plane | SSE exists | Never |
| `tokio::sync::broadcast` for log streaming | We use SSE per client | V1+ if SSE proves insufficient |
| `reqwest` with `default-features = true` (uses OpenSSL) | OpenSSL is a C dep that fights musl | `rustls-tls` only |
| `openssl` crate directly | Same | Never; use `rustls` |
| `chrono` without `serde` feature | Wire format breakage | Always with `serde` |
| `time` crate (without justification) | `chrono` is fine | V2+ if `chrono` proves insufficient |
| Hand-rolled time-series anomaly detection | We use Prophet/TimesFM | V2+ |
| `git2` (libgit2) | C dep; gix is pure-Rust | V1.5+ |
| `docker-rust` (the older one) | bollard is the Tokio-native successor | Never |
| `prometheus` crate directly (tying us to one backend) | We use the `metrics` facade | Never |

---

## 10. The dependency-update policy

- **Monday morning only.** Dep updates land on Monday, have the full week to be reverted, and the on-call person is awake.
- **One PR per dep.** Each dep update is its own PR. No "deps: bump 12 things" PRs.
- **The PR description must include:**
  - The new version and the old version.
  - The release notes link.
  - A "no behavior change expected" or a "behavior change summary" line.
  - The CI link.
- **The PR is reviewed by the lead engineer.** Not by a bot, not by "anyone with merge rights."
- **`cargo-deny` and `cargo-audit` must pass.** No new advisories. No yanked crates. No duplicate versions.
- **`cargo update --aggressive` is forbidden.** Pin to the latest minor, not the latest major, until we explicitly decide to bump.

---

## 11. What we will add in V1.5+ (the future list, not the locked list)

These are *candidate* additions, not yet locked. Each requires an ADR before being added to the workspace.

| Crate | Use case | Phase | ADR required |
|---|---|---|---|
| `gix` | Pure-Rust git operations for `sovereign init` and webhook deploys | V1.5 | Yes |
| `tokio-tungstenite` | Not used (no WebSockets) | — | No |
| `governor` | Rate limiting on the HTTP API | V1.5 | Yes |
| `argon2` (already locked) | Password hashing | V0 | No |
| `totp-rs` | TOTP MFA | V1.5 | Yes |
| `jsonwebtoken` | JWT for OIDC client mode | V2 | Yes |
| `oauth2` | OIDC client flows | V2 | Yes |
| `rego` / `regorus` | In-process Rego evaluation | V2 | Yes |
| `opa-wasm` | OPA bundle evaluation for advanced rules | V2 | Yes |
| `prophet-rs` (or `timesfm` bindings) | ML scorer | V2 / V2.5 | Yes |
| `rqlite` (HTTP client) | V2 state backend | V2 | Yes |
| `sea-query` (build-time only) | Type-safe SQL builder for migrations | Maybe | Yes |

---

## 12. The version-bump policy

- **MSRV:** bumped only when a new feature is essential and stable for at least 6 months. The MSRV is in `rust-version` and CI fails if a contributor is on an older toolchain.
- **Tokio:** major bumps only when forced. tokio 2.x is a multi-year project; we track it but do not chase it.
- **axum:** track the latest 0.x. We pin to a minor (e.g., `0.8`) and bump minor versions on Monday.
- **sqlx:** same. We pin to a minor and bump on Monday.
- **clap:** 4.x is stable; bump minor on Monday.
- **ratatui:** track 0.30+. The TUI rewrites (k9s, lazygit) happen every 2-3 years.
- **`cargo-dist`:** track the latest. Releases are frequent; bumps are routine.
- **Crates in the "future list" (§11):** do not bump past the version in their ADR without a new ADR.

---

## 13. The "if a new crate is needed" checklist

Before adding a new direct dependency:

1. **Is it in the locked table (§1)?** If yes, use the version pin.
2. **Is it in the future list (§11)?** If yes, the phase must be open and the ADR filed.
3. **Is it a transitive dep we're trying to elevate?** Don't. Transitive deps stay transitive.
4. **Is it a fork or a vendored copy?** No. Use the upstream crate.
5. **Does it pull in C code?** Avoid. Use the pure-Rust alternative (`rustls` not `openssl`, `gix` not `git2`).
6. **Is its license in the `deny.toml` allow list?** If not, add the license (or reject the crate).
7. **Does it compile to musl?** Run `cargo build --target x86_64-unknown-linux-musl` and confirm.
8. **Does it have tests, examples, and CI?** Check the repo.
9. **Is it actively maintained?** Last commit < 6 months for V0/V1; < 12 months for V1.5+.
10. **File the ADR.** PR description links the ADR. Lead engineer reviews.

---

**Next: read [`phase-00-mvp.md`](./phase-00-mvp.md) to see how this stack is used to ship the first 8 features in 6 weeks.**

---

## 10. Static linking — the "one binary, no system deps" contract

The binary is **truly static** on the musl target — no `libc.so`, no `libpthread.so`, no `libresolv.so`, no `libm.so`, no `ld-linux-x86-64.so.2`. The only file the operator copies is `sovereign` itself, and it runs on any Linux 4.18+ with a working kernel (no package manager needed, no `glibc` version check, no `nsswitch.conf`).

### 10.1 The musl build flags (the exact recipe)

```toml
# .cargo/config.toml (workspace root)
[target.x86_64-unknown-linux-musl]
linker = "rust-lld"
rustflags = [
    "-C", "link-self-contained=yes",  # bundle the C runtime into the binary
    "-C", "link-arg=-static",         # force static linking of all C deps
    "-C", "panic=abort",              # no unwinding tables; smaller binary
    "-C", "strip=symbols",            # strip debug symbols at link time
    "-C", "opt-level=z",              # optimize for size
    "-C", "lto=fat",                  # link-time optimization across all crates
    "-C", "codegen-units=1",          # better optimization, slower compile
]
```

The `link-self-contained=yes` is the load-bearing flag — it tells rustc to bundle `libc.a`, `libunwind.a`, `libcompiler_builtins.a` into the binary, so the resulting file is ~25 MB and has zero runtime C dependencies. The `link-arg=-static` is a belt-and-suspenders backup for any C dep that didn't pick up the rustflag automatically.

### 10.2 The verification (CI gate)

A CI test runs on every release:

```bash
# 1. Build for musl
cargo build --release --target x86_64-unknown-linux-musl --bin sovereign

# 2. Strip and pack
strip target/x86_64-unknown-linux-musl/release/sovereign
tar -cJf sovereign-$VERSION-x86_64-unknown-linux-musl.tar.xz \
    -C target/x86_64-unknown-linux-musl/release sovereign

# 3. Verify the binary is truly static
file target/x86_64-unknown-linux-musl/release/sovereign
# Expected: ELF 64-bit LSB executable, x86-64, version 1 (SYSV), \
#           static-linked, BuildID[sha1]=..., with debug_info, stripped

ldd target/x86_64-unknown-linux-musl/release/sovereign
# Expected: not a dynamic executable

# 4. Run on a clean Alpine 3.21 container with NO system packages
docker run --rm -v $PWD/sovereign:/sovereign:ro alpine:3.21 /sovereign --version
# Expected: sovereign 1.0.3

# 5. Run on a from-scratch scratch image (the ultimate test)
docker buildx build --platform linux/amd64 -f - . <<EOF
FROM scratch
COPY --from=builder /sovereign /sovereign
ENTRYPOINT ["/sovereign"]
EOF
# If the image starts and prints the version, the binary is truly static.
```

### 10.3 The "no C dependencies" audit

The `deny.toml` is extended with a `compile-check` rule that runs the musl build for every new direct dependency and fails the CI if the build adds a `NEEDED` entry to the resulting binary:

```toml
# deny.toml (excerpt)
[compile-checks]
musl_target = "x86_64-unknown-linux-musl"
max_needed_libs = 0   # zero dynamic libraries
max_binary_size_mb = 30
```

A new dep that needs `libssl.so` (e.g. an `openssl`-based crate) is **rejected** — the PR must use the `rustls` variant. A new dep that needs `libz.so` is **rejected** — the PR must use the `flate2` / `zstd` Rust crate. A new dep that needs `libc` is fine — `libc` is the rustc-provided `core` C bindings, not a shared library at runtime.

### 10.4 The exception: glibc fallback

The glibc binary (`x86_64-unknown-linux-gnu`) is shipped as a **fallback** for older distros that don't have `ld-linux-x86-64.so.2` with the musl `rust-lld` (`rust-lld` requires Linux 3.2+ and `glibc 2.17+`). The glibc binary uses `jemalloc` and is dynamically linked to the system's `libc.so.6` and `libpthread.so.0`. It is ~15% larger (because `libc` symbols are not bundled) but works on CentOS 7, RHEL 8, and similar LTS distros. The `install.sh` detects the glibc requirement automatically and downloads the glibc binary.

### 10.5 Why this matters (the "no glibc hell" promise)

The competitive landscape is full of "this works on Ubuntu 22.04 but not 20.04" complaints (the Coolify / Dokploy / CapRover GitHub issues). The musl binary **eliminates the entire class of "your glibc is too old" bugs**. The operator can copy `sovereign` from one box to another, from an Ubuntu 24.04 box to an Alpine 3.21 box, and it runs identically. The persona-cto.md §3 ("Technical bets — SQLite, Docker+Podman, Apache 2.0, musl") treats static linking as a load-bearing decision, not a nice-to-have.

---

## 11. The packaging contract (deb / rpm / apk / container)

The binary is the source of truth; the packages are convenience artifacts. The package format is decided by the operator's distro, not by us. All 4 formats are built from the same source by the same `cargo-dist` release pipeline.

### 11.1 The `cargo-dist` workspace config

```toml
# Cargo.toml — workspace.metadata.cargo-dist
[workspace.metadata.cargo-dist]
targets = [
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-unknown-linux-gnu",       # glibc fallback
    "aarch64-apple-darwin",           # V1+ for dev
    "x86_64-apple-darwin",
]
installers = [
    "shell",                          # install.sh (one-liner)
    "npm",                            # npm install -g sovereign (wraps the binary)
    "homebrew",                       # brew install sovereign
    "deb",                            # Debian / Ubuntu
    "rpm",                            # RHEL / Rocky / Alma / openSUSE
    "apk",                            # Alpine
    "msi",                            # Windows (V2+; experimental)
]
publish-jobs = ["release"]
```

### 11.2 The Debian package (`deb`)

**File layout** (`sovereign_1.0.3_amd64.deb`, ~3 MB compressed):

```
/usr/bin/sovereign                          # the static musl binary
/usr/lib/systemd/system/sovereign.service   # the systemd unit
/etc/sovereign/sovereign.toml               # the default config
/usr/share/doc/sovereign/
    ├── changelog.Debian.gz                 # release notes
    ├── copyright                           # Apache 2.0
    └── README.md                           # the project README
/usr/share/man/man1/sovereign.1.gz          # the clap_mangen man page
/usr/share/bash-completion/completions/sovereign
/usr/share/zsh/site-functions/_sovereign
/usr/share/fish/vendor_completions.d/sovereign.fish
DEBIAN/control                              # Package: sovereign, Version: 1.0.3, ...
DEBIAN/conffiles                            # /etc/sovereign/sovereign.toml
DEBIAN/postinst                             # create user, data dir, enable service
DEBIAN/prerm                                # stop service, disable unit
DEBIAN/postrm                               # clean up
```

**Key Debian policies we follow:**

- `/etc/sovereign/sovereign.toml` is a **conffile** — `apt upgrade` prompts the operator if it has been modified (standard Debian behavior; the operator can keep their changes, see the diff, or accept the new default).
- The `postinst` is **idempotent** — re-installing does not wipe data. The `useradd` call uses `--no-create-home --shell /usr/sbin/nologin`; the data dir is created with `install -d -o sovereign -g sovereign -m 0750`.
- The `changelog` uses the Debian `changelog.Debian` format (not the upstream `CHANGELOG.md`) so `apt changelog sovereign` works.
- The package is signed with `debsigs` and the metadata is signed with the repo's GPG key (the `sovereign-keyring` package).

### 11.3 The RPM package (`rpm`)

Built with `cargo dist generate-rpm` (uses `rpm-tools`). Same file layout, but:

- The unit goes to `/usr/lib/systemd/system/sovereign.service`.
- The `changelog` is in the `%changelog` section of the `.spec` file.
- The post-install scripts are `%post`, `%preun`, `%postun`.
- The package is signed with the RPM GPG key in `/etc/pki/rpm-gpg/RPM-GPG-KEY-SOVEREIGN`.

The repo is served at `packages.sovereignruntime.dev/rpm/sovereign.repo` (a one-line `dnf config-manager --add-repo`).

### 11.4 The Alpine package (`apk`)

Built with `cargo dist generate-apk`. Smaller than the deb (~1.5 MB compressed) because Alpine uses `musl` natively, so the binary is the same. The package is in the `community` repo (V1+) and the `sovereign` repo (V1.5+).

### 11.5 The container image (`ghcr.io/sovereignruntime/sovereign`)

Multi-stage build with `cargo-chef` for layer caching. The runtime image is `gcr.io/distroless/cc-debian12` (no shell, no package manager, ~20 MB base). The image runs as UID 700 (`sovereign` user). The image's entrypoint is the `sovereign` binary with `args = ["run"]`.

```dockerfile
# Dockerfile (excerpt)
FROM rust:1.85-slim AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin sovereign && \
    strip target/release/sovereign

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /app/target/release/sovereign /usr/local/bin/sovereign
COPY --from=builder /app/crates/sovereign/src/scripts/sovereign.service \
                    /etc/systemd/system/sovereign.service
COPY --from=builder /app/crates/sovereign/src/scripts/sovereign.toml \
                    /etc/sovereign/sovereign.toml
USER 700:700
VOLUME ["/var/lib/sovereign"]
EXPOSE 80 443
ENTRYPOINT ["/usr/local/bin/sovereign"]
CMD ["run"]
```

The image is **cosign-signed** (`cosign sign --key cosign.key ghcr.io/sovereignruntime/sovereign:1.0.3`), **SBOM-attached** (CycloneDX in the image metadata, SPDX at `/usr/local/share/sovereign/sbom.spdx.json`), and **reproducible** (`docker buildx build --provenance=true` produces a SLSA Level 3 attestation).

### 11.6 The version synchronization contract

The version in `Cargo.toml`, the Debian package, the RPM, the Alpine package, the container image tag, the GitHub release, and the SBOM are **all generated from a single source** — the `cargo dist build` output. There is no manual bumping; a release is one command:

```bash
cargo dist build --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl --installer deb --installer rpm --installer apk --installer shell
# Then `cargo dist publish` uploads everything to:
#   - GitHub Releases (binaries + SBOMs)
#   - packages.sovereignruntime.dev (deb/rpm/apk repos)
#   - ghcr.io/sovereignruntime/sovereign (container image)
#   - crates.io (if/when the binary is published as a crate)
```

A version skew (e.g. the Debian package is 1.0.3 but the GitHub release is 1.0.4) is a **release-blocking bug**.

---

## 12. The install.sh spec (the one-liner, audited on every release)

The full source is in `crates/sovereign/src/scripts/install.sh`. It is ~120 lines of bash (POSIX-ish, with `set -euo pipefail` at the top). The contract:

### 12.1 The contract (the 9 things the script does, in order)

1. **Detect platform** — `uname -s` (Linux / Darwin / FreeBSD), `uname -m` (x86_64 / aarch64), `ldd --version 2>/dev/null | head -1` (musl vs glibc hint).
2. **Read the requested version** — `--version X.Y.Z` flag, or `SOVEREIGN_VERSION` env var, or default to the latest stable from `https://releases.sovereignruntime.dev/stable.json`.
3. **Download the binary** — `https://releases.sovereignruntime.dev/<version>/sovereign-<version>-<target>.tar.xz`. Uses `curl -sSfL` (fail on HTTP error, follow redirects, show progress bar to a TTY only).
4. **Verify the SHA-256 checksum** — the script fetches `sha256sums.txt` and aborts if the file's hash doesn't match.
5. **Verify the cosign signature** — the script fetches the cosign bundle from `https://releases.sovereignruntime.dev/<version>/sovereign-<version>-<target>.cosign.bundle`, verifies it against the public key at `https://sovereignruntime.dev/.well-known/cosign.pub` (pinned in the script). Aborts if the signature is invalid, the key is rotated without a `security@` advisory, or the bundle is missing.
6. **Install the binary** — `install -m 0755 sovereign /usr/local/bin/sovereign` (or `~/.local/bin/sovereign` if `/usr/local/bin` is read-only).
7. **Install the systemd unit** (Linux only) — `install -m 0644 sovereign.service /etc/systemd/system/sovereign.service && systemctl daemon-reload && systemctl enable --now sovereign.service`.
8. **Wait for the service to be ready** — `systemctl is-system-running --wait` + a 30-second timeout on `systemctl status sovereign`. Aborts if the service fails to start.
9. **Print the 5-line "what next"** from product-ux.md §1.1.

### 12.2 The security properties

- **TLS only** — `curl` is invoked with `--tlsv1.2 --proto =https` (no `http://` fallback). The script aborts if the certificate is invalid or expired.
- **No `eval`** — every shell expansion is quoted; every command is checked for `set -e` propagation.
- **No network calls beyond the documented 3** — `releases.sovereignruntime.dev` (binary + checksums + cosign bundle), `sovereignruntime.dev` (cosign pubkey), and `api.github.com` (for the `--channel` version lookup, V1+). No phone-home, no analytics, no third-party CDN.
- **Idempotent** — running the script on an already-installed box is a no-op upgrade (re-runs steps 3-8 with the same version = no-op; with a newer version = upgrade).
- **Auditable** — the script is committed to git, lives at `crates/sovereign/src/scripts/install.sh`, and is reviewed on every release. A "show me the install script" button on the website links directly to the GitHub blob.

### 12.3 The "what could go wrong?" matrix

| Failure | Detection | Recovery |
|---|---|---|
| Network down | `curl --fail` exits non-zero | Script aborts with `Error: cannot reach releases.sovereignruntime.dev. Check your network or use the package path (§11.2-11.4).` |
| Checksum mismatch | `sha256sum` output differs | Script aborts with `Error: SHA-256 mismatch. Refusing to install. This is a release-blocking bug; please report to security@sovereignruntime.dev.` |
| cosign signature invalid | `cosign verify-blob` returns non-zero | Script aborts with `Error: signature verification failed. The binary may have been tampered with. Refusing to install. Please report to security@sovereignruntime.dev.` |
| `/usr/local/bin` not writable | `install` returns EPERM | Script falls back to `~/.local/bin/sovereign` and prints the one-line PATH instruction. |
| systemd not running | `systemctl` exits non-zero | Script skips steps 7-8 and prints a "to complete the install, run: systemctl enable --now sovereign" message. |
| Old glibc detected | `ldd --version` shows glibc < 2.17 | Script downloads the glibc binary instead of the musl binary. |
| Unsupported arch | `uname -m` returns something other than x86_64 / aarch64 | Script aborts with `Error: unsupported architecture. Please open an issue at github.com/sovereignruntime/sovereign/issues.` |
| Already installed at the same version | Step 3's tarball matches the existing binary's hash | Script is a no-op and prints `Already at version 1.0.3.` |

### 12.4 The "no third-party CDN" rule

The binary is served from `releases.sovereignruntime.dev` only. We do not use Cloudflare R2, jsDelivr, GitHub Releases CDN, or any third-party mirror for the binary download itself. The reason: a CDN compromise (Cloudflare's 2017 Cloudbleed, the 2024 Cloudflare KV leak) is the single most likely supply-chain attack vector, and the binary download is the highest-value target. The `packages.sovereignruntime.dev` apt/dnf/apk repos use Fastly (EU POPs only) with a backup to a self-hosted S3-compatible bucket behind Cloudflare (EU-only data localization, per the sovereignty claim). The container image is on `ghcr.io` (GitHub's registry), which is a separate trust boundary but is the de-facto industry standard.
