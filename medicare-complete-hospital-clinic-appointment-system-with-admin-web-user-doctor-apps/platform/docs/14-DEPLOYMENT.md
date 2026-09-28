# 14 — Deployment

## Binaries

```
runtime/keel-api      HTTP entrypoint (healthz/readyz, router, middleware)
runtime/keel-worker   queue worker (claims jobs, executes, dead-letters)
```

Migration runs are a posture, not a binary: `keel migrate` drives the same code path.

## Build

`keel build [--profile dev|release]` wraps cargo with the documented matrix and writes the release
manifest (`RELEASE.md`): binary versions, config posture, migration head, checksums. `keel release`
prints it. The manifest is the deployable contract — if it is not in the manifest, it is not in the
release.

## Configuration

Layered: `defaults <- config/<env>.toml <- .env <- environment variables`. Parsed, validated
(secrets asserted — production refuses silent defaults), normalized, then **frozen at boot** as an
immutable `Arc<PlatformConfig>`. Environments: development, test, staging, production (+ benchmark,
migration, worker postures).

```bash
keel config:dump    # effective config with secrets redacted
keel key:generate   # rotate the application key in config
```

## Local dev

`docker-compose.yml` provides postgres (keel/keel_dev_only) and minio for S3-shaped storage.
`config/dev.toml` points at it. `keel doctor` verifies the whole local environment.

## Probes and graceful shutdown

- `GET /healthz` = liveness (200 even when degraded, envelope carries `degraded` + reason).
- `GET /readyz` = readiness (200 when capable, 503 + `BOOT` envelope when degraded). Wire the two
  distinctly so orchestrators restart only on liveness loss and drain traffic on readiness loss.
- Both runtimes cooperate with `SIGINT`/`Ctrl-C`: keel-api stops accepting and drains in-flight
  requests (axum graceful shutdown); keel-worker finishes the in-flight job, runs one final outbox
  drain, then exits. Shutdown is bounded by the longest in-flight unit of work — there is no
  forced-kill path.
- Set `KEEL_SESSION_SECRET` to a stable, per-environment secret: sessions survive restarts and are
  valid across instances only when every instance derives cookie tags from the same secret.

Full probe/shutdown semantics and the deployment-cache rationale live in `13-OPERATIONS.md`.

## Rollout discipline

- Feature flags ladder (`09-ENTERPRISE.md`) gates new behavior per tenant/percentage.
- Migrations are forward-only SQL; G08 enforces ordering. `migrate:fresh` is dev/test only.
- Releases carry the API surface delta (`keel upgrade`, G14 semver gate) so breaking changes are
  visible before they ship.
- Maintenance mode (`keel down --retry 60`) drains cleanly with a Retry-After for clients.
