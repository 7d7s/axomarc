# Keel — Enterprise Execution Platform Foundation

[![keel ci](https://github.com/7d7s/axomarc/actions/workflows/keel.yml/badge.svg)](https://github.com/7d7s/axomarc/actions/workflows/keel.yml)
[![keel publish](https://github.com/7d7s/axomarc/actions/workflows/keel-publish.yml/badge.svg)](https://github.com/7d7s/axomarc/actions/workflows/keel-publish.yml)
[![crates.io](https://img.shields.io/crates/v/keel-analyzer.svg?label=keel-analyzer)](https://crates.io/crates/keel-analyzer)
[![docs.rs](https://docs.rs/keel-analyzer/badge.svg)](https://docs.rs/keel-analyzer)

Keel is a tenant-anchored execution platform: one engine, one error protocol, one
delta/audit fabric, serving REST, CLI, jobs, webhooks, workflows and AI tools
through identical execution semantics.

This repository contains the **frozen foundation** (Stage 0/1) — the topology,
kernel, security fabric, framework services, adapters, hosts and gates — before
any domain module is built. Domain modules arrive on top as plug-and-play
applications (see `keel make:module`).

## Publishing

Keel crates are published to crates.io in release waves (Wave 1:
`keel-analyzer`). Everything about the release process - gates, token
hygiene, version bumps, wave status - lives in
[PUBLISHING.md](PUBLISHING.md); the release checklist is
[`runbooks/keel-release.md`](../runbooks/keel-release.md). Releases are
lockstep-versioned, MIT OR Apache-2.0 dual-licensed, and gated by fmt,
clippy, tests, the architecture analyzer, cargo-deny, MSRV and semver checks.

## Layout (STAGE-0-SPEC §2 blueprint topology)
- `crates/foundation/` — trusted computing base: ids, AppError, envelope, context, contracts, lifecycle, validation
- `crates/execution/` — execution engine + plan compiler, command/query buses
- `crates/application/` — registry (provider graph), configuration
- `crates/web/` — HTTP edge: envelope, viewmodels, resources
- `crates/security/` — identity, policies, scopes, ownership, elevation, sessions, tokens, integrity
- `crates/data/` — postgres adapter, embedded migrations
- `crates/async/` — events, jobs, workflows, scheduler
- `crates/services/` — cache, storage, notifications
- `crates/observability/` — Sentinel telemetry runtime (logging, telemetry, tracing, metrics, audit, pipeline, buffer, export)
- `adapters/` — cache-memory, sms(msg91), mail(smtp), storage(s3), push(fcm)
- `runtime/` — `keel-api` and `keel-worker` hosts
- `applications/` — generated apps (empty until first `keel new`)
- `tools/` — `keel` CLI, analyzer, codegen · `xtask/` — workspace tasks
- `tests/architecture/` — CI-enforced architecture gates
- `examples/` — the golden path
- `database/` — migrations + role separation

## Quick start
```bash
just db-up       # postgres + minio via docker compose
just check       # cargo check + test + architecture dep-rules
just dev         # db-up + keel-api host
cargo run -p keel-console -- doctor     # keel doctor
cargo run -p keel-example-minimal   # golden path
```

## Non-negotiables
- Every mutation is a DeltaStone inside the same transaction (no side channels).
- Tenant isolation = context + policy + RLS (`keel.tenant_id` GUC), never string filters in business code.
- Events publish post-commit only.
- Dependency matrix enforced by `keel check` (gate G07) — CI fails on violation.

Authoritative specs: `docs/rust/11-FOUNDATION-MASTER-SPEC.md` (foundation) and
`docs/rust/13-FRAMEWORK-BLUEPRINT.md` (frozen constitution, L00–L80); freeze
details in `specs/STAGE-0-FOUNDATION-SPEC.md`.