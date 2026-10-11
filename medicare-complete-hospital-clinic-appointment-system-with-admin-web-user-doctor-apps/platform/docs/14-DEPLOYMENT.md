# 14 — Deployment

## Binaries

```
runtime/kelson-demo      HTTP entrypoint (healthz/readyz, router, middleware)
runtime/kelson-worker   queue worker (claims jobs, executes, dead-letters)
```

Migration runs are a posture, not a binary: `kelson migrate` drives the same code path.

## Build

`kelson build [--profile dev|release]` wraps cargo with the documented matrix and writes the release
manifest (`RELEASE.md`): binary versions, config posture, migration head, checksums. `kelson release`
prints it. The manifest is the deployable contract — if it is not in the manifest, it is not in the
release.

## Configuration

Layered: `defaults <- package [config] <- config/packages/<name>.toml (published) <- config/<env>.toml <- .env <- environment variables`.
The package layers (Wave 8, B-24/B-25) are the `[config]` table of every package indexed by the
root `packages.toml` plus its published, operator-editable copy at `config/packages/<name>.toml`
(`kelson config:publish`) — together they fill keys the config file omits, never override it, and
are each schema-checked on their own before merging (an error in the published copy names the
file). Parsed, validated
(secrets asserted — production refuses silent defaults), normalized, then **frozen at boot** as an
immutable `Arc<PlatformConfig>`. Environments: development, test, staging, production (+ benchmark,
migration, worker postures).

```bash
kelson config:dump    # effective config with secrets redacted
kelson key:generate   # rotate the application key in config
```

## Local dev

`docker-compose.yml` provides postgres (kelson/keel_dev_only) and minio for S3-shaped storage.
`config/dev.toml` points at it. `kelson doctor` verifies the whole local environment.

## Probes and graceful shutdown

- `GET /healthz` = liveness (200 even when degraded, envelope carries `degraded` + reason).
- `GET /readyz` = readiness (200 when capable, 503 + `BOOT` envelope when degraded). Wire the two
  distinctly so orchestrators restart only on liveness loss and drain traffic on readiness loss.
- Both runtimes cooperate with `SIGINT`/`Ctrl-C`: kelson-demo stops accepting and drains in-flight
  requests (axum graceful shutdown); kelson-worker finishes the in-flight job, runs one final outbox
  drain, then exits. Shutdown is bounded by the longest in-flight unit of work — there is no
  forced-kill path.
- Set `KEEL_SESSION_SECRET` to a stable, per-environment secret: sessions survive restarts and are
  valid across instances only when every instance derives cookie tags from the same secret.

Full probe/shutdown semantics and the deployment-cache rationale live in `13-OPERATIONS.md`.

## Rollout discipline

- Feature flags ladder (`09-ENTERPRISE.md`) gates new behavior per tenant/percentage.
- Migrations are forward-only SQL; G08 enforces ordering. `migrate:fresh` is dev/test only.
- Releases carry the API surface delta (`kelson upgrade`, G14 semver gate) so breaking changes are
  visible before they ship.
- Maintenance mode (`kelson down --retry 60`) drains cleanly with a Retry-After for clients.

## Promotion pipeline (Phase 4, §7)

`kelson promote` drives a release through four stages and auto-rolls-back on a failed health gate
(KEEL-004). The dimension ports live in `kelson-deployment` (Enterprise class, which may reach
neither `Data` nor `Extension`); `kelson-console` is the Tools-class composition root that supplies
the concrete adapters.

| Stage | Gate | Mutates |
|---|---|---|
| 1. deploy | `post-deploy` | module pins |
| 2. migrate | `post-migrate` | pair-migration batches |
| 3. activate | `post-traffic` | config |
| 4. canary | `canary` | — (final gate before success) |

**Order guarantees.** The two read-only guards (jobs, events) are hoisted **before any mutation**
so an incompatible target fails closed with *nothing* half-applied; the three mutating dimensions
keep the doc order config → migrations → modules. This is a deliberate, documented deviation from
§7.1's literal line order — §7.1 lists dimensions 1..5 as config, migrations, modules, jobs,
events, and hoisting the read-only two is the only way to get a fail-closed refusal rather than a
post-mutation one.

**Checkpoint then restore.** A checkpoint of all four targeted dimensions is captured first; any
failed gate restores it and the error carries the failing stage, the restored config version and
the batches unwound.

**Fail-closed wiring.** `PromotionPipeline::new` refuses to build a pipeline missing, or carrying
a duplicate or unknown, stage probe — validated at wiring time rather than mid-promotion, when
some dimensions would already have moved. A probe returning `Ok(false)` **or** `Err(_)` both count
as unhealthy: a health check that cannot answer is not evidence of health.

**Target semantics (`PromotionTarget`).**

- `config_version: 0` is a sentinel meaning *source-driven config* — stage 3 reads from the
  configured source rather than activating a held revision. A non-zero value activates that exact
  held revision, which is what makes rollback a version swap rather than a re-read.
- `db_version: Option<u32>` is the **pair-migration batch watermark** taken from `keel_migrations`
  via `pairs::applied`; `None` skips the database dimension entirely (see `05-DATABASE.md`).
- `module_versions` are applied to a separate artifact, `storage/framework/module-pins.json` —
  **not** a rewrite of the package index.
- `job_schema_versions` are checked against the job registry *and* the queue's in-flight rows; a
  job unlisted in the target is treated as `v1` (fail closed).

**Known limits (Phase 5 recorded, not fixed).**

- The config dimension of `kelson rollback` is **process-local**: it swaps the running revision and
  the subscribers' route/flag views, and never rewrites the source file. Another process reading
  the file sees the old value — see `13-OPERATIONS.md` §Config reload and rollback.
- `storage/framework/module-pins.json` has no runtime host reader yet; the pin state is written
  and reported by the CLI but not yet consulted at boot.
- The CLI's `JobRegistry` is empty (it does not link application handlers, same posture as
  `kelson call`), so `supported_versions` falls back to the queue's in-flight maximum.
- No live-DB integration test covers promotion or rollback (same posture as `migrate:rollback`);
  the measurement procedure is in `13-OPERATIONS.md` §Performance baseline.
