# Implementation Guide

**Status:** Locked for V0/V1.
**Audience:** Every engineer. Read end-to-end on day 1.
**Last updated:** 2026-06-04

This document is the **how to actually build, test, and ship** guide. It assumes you have read [`architecture.md`](./architecture.md) and [`tech-stack.md`](./tech-stack.md).

---

## 1. The dev environment

### 1.1 Required tools

| Tool | Version | Install |
|---|---|---|
| Rust | 1.85+ | `rustup default 1.85` |
| `cargo` | bundled with rustup | `rustup component add cargo` |
| `clippy` | bundled | `rustup component add clippy` |
| `rustfmt` | bundled | `rustup component add rustfmt` |
| `cargo-deny` | latest | `cargo install cargo-deny --locked` |
| `cargo-audit` | latest | `cargo install cargo-audit --locked` |
| `cargo-watch` | latest | `cargo install cargo-watch --locked` |
| `cargo-edit` | latest | `cargo install cargo-edit --locked` |
| `cargo-outdated` | latest | `cargo install cargo-outdated --locked` |
| `cargo-llvm-cov` | latest | `cargo install cargo-llvm-cov --locked` |
| `cargo-insta` | latest | `cargo install cargo-insta --locked` |
| `cargo-dist` | latest | `cargo install cargo-dist --locked` |
| `cargo-cyclonedx` | latest | `cargo install cargo-cyclonedx --locked` |
| `mold` (Linux) | latest | `apt install mold` or equivalent |
| Docker | 24+ | [docker.com](https://docker.com) |
| `cosign` | latest | [github.com/sigstore/cosign](https://github.com/sigstore/cosign) |
| `mdbook` | latest | `cargo install mdbook --locked` |
| `sccache` | latest | `cargo install sccache --locked` |

### 1.2 The setup script

```bash
#!/usr/bin/env bash
# scripts/setup.sh — run once after cloning the repo
set -euo pipefail

# Install Rust 1.85
if ! command -v rustup &>/dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain 1.85
fi

# Install the rust targets we ship
rustup target add x86_64-unknown-linux-musl
rustup target add aarch64-unknown-linux-musl
rustup target add x86_64-unknown-linux-gnu

# Install cargo subcommands
cargo install cargo-deny cargo-audit cargo-watch cargo-edit cargo-outdated cargo-llvm-cov cargo-insta cargo-dist cargo-cyclonedx --locked

# Install system deps
if command -v apt &>/dev/null; then
    sudo apt update && sudo apt install -y mold cmake pkg-config libssl-dev
fi

# Install pre-commit
cargo install pre-commit-rs --locked  # or use the Python pre-commit

# Verify
cargo --version
rustc --version
cargo deny --version
cargo audit --version
```

### 1.3 The repo layout (recap from [`architecture.md` §1.2](#))

```text
sovereign/
├── Cargo.toml                      # workspace
├── deny.toml                       # cargo-deny config
├── rust-toolchain.toml             # pinned toolchain
├── .cargo/config.toml              # linker + flags
├── crates/
│   ├── sovereign/                  # binary
│   ├── sovereign-core/             # domain + use cases + ports
│   ├── sovereign-runtime-docker/
│   ├── sovereign-proxy-caddy/
│   ├── sovereign-secrets-age/
│   ├── sovereign-storage-sqlite/
│   ├── sovereign-backup/
│   ├── sovereign-notify/
│   ├── sovereign-observability/
│   └── sovereign-proto/
├── migrations/                     # SQL migrations
├── docs/                           # this folder
├── scripts/                        # setup, release, ci helpers
└── .github/
    └── workflows/
        ├── ci.yml
        ├── release.yml
        └── sbom.yml
```

### 1.4 The `rust-toolchain.toml`

```toml
# rust-toolchain.toml
[toolchain]
channel = "1.85"
components = ["rustfmt", "clippy", "rust-src"]
profile = "minimal"
```

This pins the toolchain for everyone. CI uses the same.

---

## 2. The build

### 2.1 Local dev build

```bash
# Build everything in debug
cargo build

# Build a specific crate
cargo build -p sovereign-core

# Build with all features
cargo build --all-features

# Build for musl (the production target)
cargo build --release --target x86_64-unknown-linux-musl
```

### 2.2 Local dev run

```bash
# Run the binary
cargo run -- http
# Output: sovereign 0.1.0 starting on http://127.0.0.1:7878

# Run with the TUI feature
cargo run --features tui -- tui

# Run with the OTel feature
cargo run --features otel -- http

# Run a one-shot CLI command
cargo run -- deploy --app api --image=nginx:alpine
```

### 2.3 The watch loop

```bash
# Watch and rebuild on every change
cargo watch -x check

# Watch and run tests on every change
cargo watch -x test

# Watch and run a specific test
cargo watch -x 'test deploy::'
```

### 2.4 The compile-time discipline

- **`mold` linker** for 3-5x faster linking. Verify with `ls -l target/debug/sovereign`; if the file's mtime is recent, you're on `mold`.
- **`sccache`** in CI for distributed compilation cache. In dev, optional.
- **`target-cpu=native`** in dev for faster builds. Release uses the cross-compile target.
- **No `cargo build --release` in dev** unless you have a reason. Use `cargo run` or `cargo test`.

---

## 3. The test

### 3.1 The test stack (V0)

| Tool | Use | Command |
|---|---|---|
| `#[tokio::test]` | Async tests | `cargo test` |
| `mockall` | Mock ports | `cargo test -p sovereign-core` |
| `insta` | Snapshot tests | `cargo insta review` |
| `trybuild` | Compile-fail tests | `cargo test -p trybuild` |
| `cargo-llvm-cov` | Coverage | `cargo llvm-cov --all-features --lcov` |
| `cargo-fmt --check` | Format | `cargo fmt --all -- --check` |
| `cargo clippy` | Lints | `cargo clippy --all-targets --all-features -- -D warnings` |

### 3.2 The test pyramid

```text
         /\
        /  \      E2E: 5 tests, 1 environment (the 5-command quickstart)
       /----\
      /      \    Integration: 50 tests, testcontainers (Docker, Caddy, Postgres)
     /--------\
    /          \  Unit: 500+ tests, in-process (use cases with mocks)
   /____________\
```

- **Unit tests (500+):** every use case, every state machine, every port trait. Fast, in-process, no I/O.
- **Integration tests (50+):** every adapter against a real dependency (testcontainers). Slower, in-CI.
- **E2E tests (5+):** the 5-command quickstart against a real VM. Slow, manual or nightly.

### 3.3 The test naming convention

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_of_test() {
        // ...
    }
}
```

Or, for nested modules:

```rust
#[cfg(test)]
mod deploy_tests {
    use super::*;

    #[test]
    fn start_deployment_creates_pending_row() {
        // ...
    }

    #[test]
    fn start_deployment_fails_with_invalid_image() {
        // ...
    }
}
```

### 3.4 The snapshot test pattern

```rust
use insta::assert_snapshot;

#[test]
fn deploy_output_for_happy_path() {
    let output = render_deploy_output(scenario::happy_path());
    assert_snapshot!(output);
}
```

Run `cargo insta review` to accept/reject snapshots.

### 3.5 The property test pattern

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn deployment_state_machine_is_total(
        a in deployment_status_strategy(),
        b in deployment_status_strategy(),
    ) {
        // For any (a, b), a.can_transition_to(b) is either allowed or denied.
        // If allowed, there exists a sequence of valid transitions from a to b.
        let result = a.can_transition_to(b);
        prop_assert!(matches!(result, true | false));
    }
}
```

### 3.6 The mock pattern

```rust
use mockall::mock;

mock! {
    pub Runtime {}
    #[async_trait]
    impl RuntimePort for Runtime {
        async fn deploy(&self, spec: &DeploySpec) -> Result<DeploymentResult, RuntimeError>;
        // ... other methods
    }
}

#[tokio::test]
async fn start_deploy_uses_runtime() {
    let mut mock = MockRuntime::new();
    mock.expect_deploy()
        .times(1)
        .returning(|_| Ok(DeploymentResult { /* ... */ }));
    
    let state = AppState::new(Arc::new(mock), /* ... */);
    let result = start(&state, /* ... */).await.unwrap();
    // Assert
}
```

### 3.7 The test container pattern

```rust
use testcontainers::*;

#[tokio::test]
async fn deploy_to_real_docker() {
    let docker = clients::Cli::default();
    let container = docker.run(images::nginx::Nginx);
    let port = container.get_host_port_ipv4(80);
    
    // ... use the container
}
```

### 3.8 The CI test pipeline

```yaml
# .github/workflows/ci.yml
name: CI
on: [push, pull_request]

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
      - uses: codecov/codecov-action@v4
        with: { files: lcov.info }
      - run: cargo install cargo-deny --locked && cargo deny check
      - run: cargo install cargo-audit --locked && cargo audit
```

### 3.9 The pre-commit hook

```yaml
# .pre-commit-config.yaml
repos:
  - repo: local
    hooks:
      - id: cargo-fmt
        name: cargo fmt
        entry: cargo fmt --all --
        language: system
        pass_filenames: false
      - id: cargo-clippy
        name: cargo clippy
        entry: cargo clippy --all-targets --all-features -- -D warnings
        language: system
        pass_filenames: false
      - id: cargo-test
        name: cargo test
        entry: cargo test --all-features
        language: system
        pass_filenames: false
        stages: [manual]
```

Install: `pre-commit install`.

---

## 4. The fuzz

### 4.1 The fuzz targets (V1+)

| Target | What it fuzzes | Why |
|---|---|---|
| `fuzz_app_yaml` | The `app.yaml` parser | Untrusted user input |
| `fuzz_policy_input` | The OPA/Rego input | Untrusted + the policy engine must not panic |
| `fuzz_audit_payload` | The audit log JSON payload | Must not panic, must not overflow |
| `fuzz_secret_value` | The secret encryption (roundtrip) | No key material leak via timing |
| `fuzz_idempotency_key` | The idempotency key parser | Must not collide, must not panic |

### 4.2 The fuzz runner

```bash
# Run a fuzz target for 60s
cargo +nightly fuzz run fuzz_app_yaml -- -max_total_time=60

# Run all targets for 5 minutes
for target in fuzz_app_yaml fuzz_policy_input fuzz_audit_payload; do
    cargo +nightly fuzz run $target -- -max_total_time=300
done
```

CI runs fuzz on every PR for 60s per target. Nightly runs for 1 hour.

---

## 5. The benchmark

### 5.1 The bench targets (V1+)

| Bench | What it measures |
|---|---|
| `bench_state_transition` | Time per `DeploymentStatus::can_transition_to` |
| `bench_audit_append` | Time per audit log insert |
| `bench_secret_encrypt` | Time per `AgeSecrets::encrypt` |
| `bench_secret_decrypt` | Time per `AgeSecrets::decrypt_to_memory` |
| `bench_deploy_use_case` | End-to-end `start_deploy` with mock runtime |
| `bench_health_check` | Time per `healthcheck` against a local container |
| `bench_query_audit` | Time per audit query with various filters |

### 5.2 The bench runner

```bash
# Run a specific bench
cargo bench -p sovereign-core --bench state_transition

# Run with codspeed (CI)
cargo codspeed run -p sovereign-core --bench state_transition
```

### 5.3 The perf budget

| Operation | Target p99 |
|---|---|
| CLI startup | < 50 ms |
| `deploy` (with mock runtime) | < 100 ms |
| `status` (1 app) | < 10 ms |
| `audit` query (90 days) | < 50 ms |
| `healthcheck` (local) | < 30 ms |
| `secret encrypt` (1 KB) | < 1 ms |
| `secret decrypt_to_memory` (1 KB) | < 1 ms |

CI fails the build if any benchmark regresses by > 10% on `codspeed`.

---

## 6. The release

### 6.1 The release pipeline (V0 manual, V1 automated)

```bash
# Bump the version
cargo set-version --workspace 0.1.0  # for V0.1.0; for V1.0, 1.0.0

# Update CHANGELOG.md
$EDITOR CHANGELOG.md

# Commit
git add -A
git commit -m "release: v0.1.0"

# Tag
git tag -a v0.1.0 -m "v0.1.0"

# Build for all targets
cargo dist build

# Publish to GitHub
git push origin main
git push origin v0.1.0

# The release.yml workflow:
# 1. Builds for all targets
# 2. Generates SBOM
# 3. Signs with cosign
# 4. Verifies reproducible build
# 5. Uploads to GitHub Releases
# 6. Updates Homebrew tap
# 7. Updates apt/yum repos
```

### 6.2 The `release.yml` workflow (V1+)

```yaml
# .github/workflows/release.yml
name: Release
on:
  push:
    tags: ['v*.*.*']

jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.85
      - name: Build with cargo-dist
        run: cargo dist build
      - name: Generate SBOM
        run: cargo cyclonedx --format json --override-filename target/dist/sbom.json
      - name: Sign with cosign
        run: |
          for f in target/dist/*; do
            cosign sign-blob --yes "$f" --output-signature "$f.sig" --output-certificate "$f.pem"
          done
      - name: Verify reproducible build
        run: |
          cargo dist build
          for f in target/dist/*; do
            sha256sum "$f" | diff - <(sha256sum "target/dist/$(basename $f)")
          done
      - name: Upload to GitHub Releases
        uses: softprops/action-gh-release@v2
        with:
          files: target/dist/*
          fail_on_unmatched_files: true
```

### 6.3 The release checklist

- [ ] `CHANGELOG.md` updated
- [ ] `Cargo.toml` version bumped
- [ ] `git tag -a vX.Y.Z` created
- [ ] `cargo dist build` succeeds for all 5 targets
- [ ] SBOM generated
- [ ] cosign signatures generated
- [ ] Reproducible build verified
- [ ] GitHub release published with all artifacts
- [ ] Homebrew tap updated (V1+)
- [ ] apt/yum repos updated (V1+)
- [ ] Blog post published
- [ ] Twitter/Mastodon post published
- [ ] Design partners re-engaged

---

## 7. The debug

### 7.1 The debug log

```bash
# Set the log level via env
RUST_LOG=debug cargo run -- http
RUST_LOG=sovereign=trace,sqlx=debug cargo run -- http

# Log to a file
RUST_LOG=info cargo run -- http 2>> /tmp/sovereign.log

# JSON logs
RUST_LOG=info RUST_LOG_FORMAT=json cargo run -- http
```

### 7.2 The `tracing` cheat sheet

- `tracing::info!(?app_id, "deploying")` — info-level with structured field
- `tracing::warn!(deployment_id = %dep.id, "rollback failed")` — warn with Display
- `tracing::error!(error = ?e, "deploy failed")` — error with Debug
- `#[tracing::instrument(skip(self))]` — auto-instrument a function

### 7.3 The common-debugging scenarios

| Symptom | First check |
|---|---|
| CLI hangs | `RUST_LOG=trace` to find the blocking await |
| Deploy fails silently | `RUST_LOG=sovereign=debug,bollard=info` to see Docker API calls |
| SQLite lock error | `journal_mode = WAL`; check no long-running transaction |
| Caddy 502 | `curl -v http://127.0.0.1:2019/config/`; check the route |
| Test flaky | `cargo test -- --test-threads=1`; check for shared state |
| Binary too big | `cargo bloat --release` to find the largest crate |

### 7.4 The `cargo-bloat` use

```bash
# Find the largest crates in the release binary
cargo bloat --release -n 20

# Find the largest functions
cargo bloat --release --functions -n 20
```

---

## 8. The code style

### 8.1 The `rustfmt` config

```toml
# rustfmt.toml (workspace root)
edition = "2024"
max_width = 100
tab_spaces = 4
use_small_heuristics = "max"
imports_granularity = "Crate"
group_imports = "StdExternalCrate"
reorder_impl_items = true
trailing_comma = "Vertical"
```

### 8.2 The `clippy` config

```toml
# .clippy.toml
avoid-breaking-exported-api = false
msrv = "1.85"
```

`cargo clippy --all-targets --all-features -- -D warnings` runs in CI and fails the build on any warning.

### 8.3 The naming conventions

- **Types:** `PascalCase` (`App`, `DeploymentId`, `Strategy`)
- **Functions:** `snake_case` (`start_deployment`, `can_transition_to`)
- **Constants:** `SCREAMING_SNAKE_CASE` (`MAX_CONNECTIONS`, `DEFAULT_PORT`)
- **Modules:** `snake_case` (`use_cases`, `domain`, `ports`)
- **Newtype wrappers:** `PascalCase(Uuid)` or `PascalCase(String)` (`AppId(Uuid)`)
- **Enums:** `PascalCase` for the type, `PascalCase` for variants (`DeploymentStatus::Healthy`)
- **Error variants:** `PascalCase` (`AppError::Conflict`)

### 8.4 The "Rust idioms" we follow

- **`if let Some(x) = ... { ... } else { ... }`** over `match` for one-variant matches.
- **`.unwrap_or(default)` / `.unwrap_or_else(|| default)`** over `match` for one-default cases.
- **Iterators** over `for` loops where it improves readability.
- **`?` operator** over `match` for error propagation.
- **`thiserror` enums** for library errors; **`anyhow::Result`** for binary errors.
- **Newtype wrappers** for IDs and units of measure (`Duration`, `Bytes`).
- **`#[must_use]`** on functions that return a non-`()` value.
- **No `unsafe`** without an explicit, audited, fuzz-tested reason.
- **No `lazy_static!` or `once_cell::Lazy`** in domain or use cases.
- **No `Box<dyn Any>` or `serde_json::Value`** in domain types.

### 8.5 The comment policy

- **No comments** that describe what the code does. The code describes itself.
- **Doc comments** (`///`) on every public function, struct, enum, trait. Use `cargo doc --no-deps` to verify.
- **Module-level docs** (`//!`) on every module explaining the *why*, not the *what*.
- **TODO comments** include the GitHub issue number: `// TODO(#123): ...`.
- **No "we should" comments** that are not linked to an issue. If it's not an issue, it's not a TODO.
- **The CODEOWNERS file** is the source of truth for who reviews what.

---

## 9. The dependencies

### 9.1 Adding a new direct dep

1. Is it in [`tech-stack.md` §1](#)? Use the version pin.
2. Is it in [`tech-stack.md` §11](#)? The phase must be open.
3. **Add the dep:** `cargo add --workspace <crate>`
4. **Pin the version** in workspace `Cargo.toml` to the same minor as the existing locks.
5. **Run `cargo update -p <crate> --precise <version>`** to pin.
6. **Run `cargo deny check`**, `cargo audit`, `cargo clippy`.
7. **Open a PR** with the dep change + a 1-line justification.
8. **Lead engineer reviews.**

### 9.2 Bumping a dep

1. **Monday morning only.**
2. **One PR per dep.**
3. **PR description includes:** old version, new version, release notes link, behavior change summary.
4. **CI must pass.**
5. **Lead engineer reviews.**

### 9.3 Removing a dep

1. **Find all usages** with `cargo machete` (or `rg "<crate>:" --type rust`).
2. **Remove the usages.**
3. **Run `cargo remove --workspace <crate>`.**
4. **Run `cargo deny check`** to confirm no transitive dep brings it back.
5. **Open a PR.**

---

## 10. The first contribution

### 10.1 The "good first issue" pattern

A good first issue is:
- **Self-contained:** a single function, a single file, a single crate.
- **Has tests:** the issue includes the test cases the PR must pass.
- **Has acceptance criteria:** "the X function returns Y when Z."
- **No architecture decisions:** the issue does not require a new trait or a new port.
- **Has a mentor:** the lead engineer is assigned to the issue.

### 10.2 The "good first contribution" path

1. **Read** [`README.md`](./README.md) and [`architecture.md`](./architecture.md) §1.
2. **Set up** the dev environment (run `scripts/setup.sh`).
3. **Build** the binary (`cargo build`).
4. **Run** the test suite (`cargo test`).
5. **Pick a "good first issue"** from the issue tracker.
6. **Open a PR** with the change + the tests.
7. **Iterate** with the reviewer.
8. **Merge** after 2 approvals and CI green.

### 10.3 The "first contribution" checklist

- [ ] Signed the CLA (V1; V0: implicit via Apache 2.0)
- [ ] Set up the dev environment
- [ ] Picked a "good first issue"
- [ ] Opened a PR with tests
- [ ] Got 2 approvals
- [ ] CI is green
- [ ] Merged to `main`

---

## 11. The CI/CD matrix

### 11.1 The CI matrix

| Job | Runs on | Trigger | Fail condition |
|---|---|---|---|
| `fmt` | ubuntu-latest | every push, every PR | `cargo fmt --check` fails |
| `clippy` | ubuntu-latest | every push, every PR | any clippy warning |
| `test-linux` | ubuntu-latest | every push, every PR | any test fails |
| `test-macos` | macos-latest | every push, every PR | any test fails |
| `coverage` | ubuntu-latest | every push, every PR | coverage < 80% |
| `deny` | ubuntu-latest | every push, every PR | `cargo deny` fails |
| `audit` | ubuntu-latest | every push, every PR | new advisory |
| `doc` | ubuntu-latest | every push, every PR | any doc warning |
| `msrv` | ubuntu-latest | every PR | does not build on MSRV |
| `build-musl` | ubuntu-latest | every push, every PR | does not build for musl |
| `fuzz` | ubuntu-latest | every PR (60s per target) | any panic |
| `bench` | ubuntu-latest | every PR | > 10% regression |
| `release` | ubuntu-latest | every tag `v*` | any of the above |
| `e2e` | self-hosted | nightly | any E2E test fails |

### 11.2 The "must be green to merge" gates

- `fmt`, `clippy`, `test-linux`, `test-macos`, `coverage`, `deny`, `audit`, `doc`, `msrv`, `build-musl`
- `fuzz` and `bench` are nice-to-have for PRs; must be green for `main`.

### 11.3 The nightly jobs

- `e2e` (the 5-command quickstart on a real Hetzner CX22)
- `fuzz` (1 hour per target)
- `bench` (full benchmark suite)
- `dependency-audit` (full audit, including transitive deps)

---

## 12. The on-call

The on-call rotation lives in [`operations-runbook.md`](./operations-runbook.md). Read that for the operational side. From the engineering side:

- **One person, 24/7, 7 days.** No rotation. (V0/V1: founder. V1.5: first 3 engineers rotate.)
- **Alerts go to humans via Telegram**, not email.
- **Every alert has a runbook** in `docs/operations/`.
- **15-minute response window.** If no response, status page auto-flips to "investigating."
- **Quarterly "vacation test":** operator takes 1 week off; on-call is automated; manual backup is the freelancer.

---

**Next: read [`operations-runbook.md`](./operations-runbook.md) for the production operations side.**
