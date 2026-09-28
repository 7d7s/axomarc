# 01 — Getting Started

Keel is a Rust platform framework: an application framework (Laravel-parity surface) built on an
execution runtime (context/policy/transaction/delta), a developer platform (CLI/generators/testkit/
diagnostics/AI), and an enterprise control plane (tenancy/flags/deployment).

## Prerequisites

- Rust stable, MSRV **1.85** (workspace `rust-version`), edition 2021.
- Cargo (workspace lives in `platform/`).
- Optional, for postgres-backed dev: docker compose (`docker-compose.yml` provides postgres + minio).

## First commands

```bash
cd platform
cargo test --workspace     # full gate suite (unit + architecture + examples)
cargo run -p keel-console -- doctor   # toolchain, config, database assets, compose file
cargo run -p keel-console -- check    # architecture gate G07: dependency matrix + budgets
```

Both `doctor` and `check` must be clean on a fresh clone.

## Workspace layout

```
applications/ domain applications on the tenant plane (e.g. appointments)
crates/       platform crates by layer (foundation/execution/application/web/security/
              data/async/services/observability/enterprise/ai)
adapters/     port implementations wired only by runtime binaries
runtime/      keel-api, keel-worker binaries
tools/        keel CLI, analyzer, codegen, xtask
testing/      testkit (TestApp, fakes, time control)
examples/     minimal (golden path), reference (appointments end-to-end)
tests/        architecture gate tests
templates/    project-template + starter-template used by `keel new [--with-starter]`
specs/        STAGE-0-FOUNDATION-SPEC (normative)
database/     migrations + roles.sql
config/       per-environment TOML (defaults <- file <- env)
```

## Run the examples

```bash
cargo run -p keel-example-minimal      # golden path: boot -> execute -> delta -> replay -> deny
cargo run -p keel-example-reference    # appointments domain end-to-end (manifest -> engine ->
                                       # delta+outbox -> search -> MCP tools)
```

## Create a module

```bash
cargo run -p keel-console -- make:module bookings --plane tenant
cargo check
```

`make:module` emits a compiling module tree (manifest, Cargo.toml, provider, one golden-path
operation, tests) and registers the workspace member. `keel new` scaffolds a whole project from
`templates/project-template`.

## Run the HTTP edge

```bash
cargo run -p keel-console -- serve            # keel-api with workspace config (--worker adds the queue worker)
curl http://127.0.0.1:8180/healthz        # liveness envelope
curl -X POST http://127.0.0.1:8180/auth/register \
     -H 'content-type: application/json' \
     -d '{"email":"dev@clinic.test","password":"correct-horse-99","tenant":"dev"}'
```

`serve` spawns the real runtime binaries with `config/dev.toml`. The register/login response sets
the `keel_session` cookie; send it back to call graph-declared module routes (e.g.
`POST /api/v1/appointments`). Browser sessions require the `app.sessions` flag
(`keel flag:set app.sessions 100`; this repo's dev flag store already has it on). Probe semantics,
graceful shutdown and the metrics/dashboard endpoints are covered in `13-OPERATIONS.md`.

## Where to read next

- `02-CONCEPTS.md` for the mental model (context, capability, plan, delta).
- `03-ARCHITECTURE.md` for topology, dependency rules and gates.
- `12-CLI.md` for the full command reference.
- `16-MIGRATION-FROM-LARAVEL.md` if you come from Laravel.
