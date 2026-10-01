# 12 — CLI Reference

The `keel` binary (`tools/keel-console`). Artisan-parity operational surface; every command runs
against the same workspace/config the runtime uses.

## Environment and architecture

| Command | Purpose |
|---|---|
| `keel doctor` | toolchain, config, database assets, compose file, member count |
| `keel check` | G07: dependency matrix + budgets; also compiles the platform graph and resolves every route's middleware chain (Wave 1 `GRAPH-MW-UNKNOWN` gate) |
| `keel security` | secrets hygiene, dependency policy, trace locality |
| `keel upgrade` | upgrade report: versions, APIs, deprecations, config, migrations, deps |

## Graph and modules

| Command | Purpose |
|---|---|
| `keel graph` | compile the platform graph from all `module.toml` |
| `keel modules` | registered modules (version, maturity, plane, capability count) |
| `keel route:list [--format table\|json] [--name F] [--path F] [--plane api\|web]` | declared routes with plane/group/version/deprecation columns; `--name`/`--path` are case-insensitive substring filters, `--plane` validates `api`/`web` (Wave 2) |
| `keel route:cache` | write the pinned route artifact (Wave 1): resolved per-route middleware chains, bindings and validators → `storage/framework/routes.json`, hash-pinned to all `module.toml` + `middleware.toml` + `MODULE_ABI`; boot refuses a stale artifact |
| `keel route:clear` | delete the route artifact; boot falls back to live compilation |
| `keel event:list` | declared events |
| `keel capabilities` | capability catalog |

## Generators

| Command | Purpose |
|---|---|
| `keel new [name] [--plane] [--with-starter]` | new project from the template (compiles immediately); `--with-starter` uses the SPA-first starter: auth-wired module + web shell (S9) |
| `keel make:module <name> [--plane]` | scaffold a domain module |
| `keel make:migration <name>` | timestamped migration file |
| `keel make <kind> <name> --module <m>` | migration/seeder/factory/job/event/listener/command/query/policy/workflow/model/test/resource/request/controller + Wave 5's cast/channel/component/exception/mail/notification/observer/rule/scope/job-middleware/middleware/view (27 kinds; kind discoverable via `GENERATOR_KINDS`, loop-tested for substitution + placeholder-free content) |

## Database

| Command | Purpose |
|---|---|
| `keel migrate` / `migrate:status` / `migrate:fresh` | apply (advisory-lock guarded: `pg_advisory_lock(7259)` acquired before any run, fail-closed on timeout — concurrent runs serialize) / report (incl. `keel_migrations` pair bookkeeping) / drop+recreate (dev-test) (Wave 3/5) |
| `keel migrate:rollback [--step n]` | revert the last `n` batches (default 1) in reverse filename order — pair migrations only; forward-only single-file migrations are not revertible (documented divergence) (Wave 5) |
| `keel migrate:reset` / `migrate:refresh [--step n]` | revert everything / rollback `n` batches then re-apply pending (both dev/test-only: the destructive window refuses production) (Wave 5) |
| `keel migrate:install` | create the `keel_migrations` bookkeeping table without running any files (Wave 5) |
| `keel seed [--only a,b]` | idempotent seeders |
| `keel db:inspect` | tables + row counts |
| `keel db:show` | driver, server version, database + the `db:inspect` table/row inventory (Wave 5) |
| `keel db:schema:dump` | normalized DDL snapshot of the migrated schema → `database/schema.sql` (review artifact; never executed by `migrate`) (Wave 5) |
| `keel model:prune [--chunk n] [--tenant <uuid>]` | sweep prunable models in chunked passes (1..=200); untenanted by default — passes the tenant only with `--tenant` (Wave 3) |

## Operations

| Command | Purpose |
|---|---|
| `keel up` / `keel down [--retry N] [--secret S]` | maintenance mode |
| `keel job:list [--status s] [--limit n]` | job inventory |
| `keel queue:work [--once]` | run the worker |
| `keel queue:failed` / `queue:retry <id>` | dead-letter triage |
| `keel queue:clear` | purge every pending job — refuses prod/staging (like `migrate:fresh`) and prompts unless `KEEL_FORCE=1` (Wave 5) |
| `keel queue:flush` | delete every dead job (`flush_cutoff()` = unbounded; `queue:prune-failed` is the age-bounded variant) (Wave 5) |
| `keel queue:forget <id>` | delete one dead job (unknown id or non-dead job = refusal) (Wave 5) |
| `keel queue:monitor` | one-shot queue depth report: pending (+ oldest age), running, dead, succeeded counts (pure render, unit-tested) (Wave 5) |
| `keel queue:prune-failed [--hours n]` (default 168) | delete dead jobs older than the cutoff (Wave 5) |
| `keel queue:prune-batches [--hours n]` (default 72) | prune terminal batch records finished before the cutoff — real `BatchLedger::prune` over the durable `job_batches` rows (unfinished batches are never pruned), printing the deleted count (Wave 5 flag/default; Wave 6 P9 made it real, B-32) |
| `keel queue:restart` | write the restart-signal file: workers that observe a signal newer than their start stop after the current job, supervisors respawn them (Wave 5) |
| `keel queue:listen` | spawn + supervise `keel-worker`: respawn on every exit with a crash-loop backoff (≥2s runs restart immediately, faster runs back off 1s) (Wave 5) |
| `keel queue:retry-batch <id>` | requeue every failed/dead member of a batch — the same `BatchOps::retry` the HTTP `POST /api/v1/batches/{id}/retry` runs over the durable batch ledger; the id is validated before any connection is attempted (Wave 5 flag; Wave 6 P9 made it real, B-32) |
| `keel schedule:list` / `schedule:run` | schedules (information stubs over the `schedule_registry` seam: they report the registry's empty posture until module schedules land — Wave 6) |
| `keel schedule:work [--every n]` (default 60s) | schedule daemon: tick loop over the registry, persists tick state (last-fired + recorded failures, Wave 6 P4), fires due items (Wave 5) |
| `keel schedule:test` | dry-run the schedule registry: prints registered vs due-now (pure — nothing fires, nothing is written; disabled entries would show registered but never due) (Wave 5) |
| `keel storage:link [--force]` | create the public/storage → storage/local symlink (refuses to overwrite without `--force`; `--force` replaces a link or file — a real directory at the path is never deleted automatically) (Wave 5) |
| `keel call <name> [json] [--principal <uuid>] [--tenant <uuid>]` | dispatch one module operation through the CLI console route: pre-flights against `routes/console.toml` (resolve by name, list on miss), enforces the dev/test-only default operator (explicit `--principal` is the production path — B-33), resolves the tenant (explicit `--tenant` UUID, else the dev/test fallback tenant — absent outside dev/test, so it must be passed), then spawns `keel-api --call` which runs the CommandBus with the same policy engine as HTTP (Wave 5; the HTTP edge-stage validation/middleware layer is not re-run — routes are pre-validated at boot) |
| `keel key:generate` | new application key into config |
| `keel config:dump` | effective config, secrets redacted |
| `keel config:cache` / `keel config:clear` | pin the frozen effective config to `storage/framework/config.json` + content hash / drop it; boot hash-check refuses drift (Wave 4) |
| `keel config:show <key> [--json]` | print one config section with secrets redacted (Wave 4) |
| `keel serve [--worker]` | one-command local loop: spawns keel-api (and keel-worker), streams logs (S7) |
| `keel shell` | interactive platform console (v1.1 B-29): REPL over the booted engine — login/whoami/ops/routes/caps/models/run under your principal (Wave 4 adds `caps` + `models`) |
| `keel cache:clear` | clear the app cache |
| `keel cache:forget <key>` | remove one cache key (Wave 4) |
| `keel optimize` | warm all three boot-pinning artifacts in order — route:cache → config:cache → view:cache; aborts at the first refusal (graph gate / config / views) (Wave 5) |
| `keel optimize:clear` | drop route + config + view artifacts; every step is attempted (a clear never leaves one cache behind because another delete failed), first error reported afterwards (Wave 5) |
| `keel view:cache` / `keel view:clear` | precompile every template under `[views].paths` into `[views].cache_dir` (mtime + content hash) / drop the compiled view artifacts; boot recompiles on demand (Wave 2 views engine) |
| `keel flag:list` / `flag:set <name> <stage> [--tenant id]` | feature flag rollout |

## Diagnostics and quality

| Command | Purpose |
|---|---|
| `keel inspect execution\|delta\|authorization\|n-plus-one [id] [--limit] [--threshold]` | inspect recorded executions |
| `keel replay <id> [--dry-run]` | replay a recorded execution (Original vs Replay) |
| `keel benchmark [--save-baseline] [--budget pct] [--iterations n]` | G15 benchmark gate |
| `keel build [--profile dev\|release]` | build + release manifest |
| `keel release` | show the release manifest |
| `keel about` | Laravel-style environment report: framework/app version, env, debug, DB driver + server version (best-effort `SELECT version()`), cache/queue/session drivers, flag snapshot, graph module/route/operation counts, dep-budget state (Wave 4) |
| `keel env` | environment summary: the config file the layering resolved, redacted key facts, config-cache pin state (Wave 4) |
| `keel profile <api\|worker>` | B-27 profiling wrapper: builds the package, then `samply record target/debug/<bin>` when samply is installed; a missing samply fails closed with `cargo install samply` + the docs/13 workflow (Wave 4) |
| `keel test [--pg]` | run `cargo test --workspace --locked` from the workspace root, streaming lines and tallying suite/test results into a final `PASS`/`FAIL` verdict; `--pg` adds `--features pg-tests` and fails closed without `DATABASE_URL` (Wave 5) |

## Telemetry (Sentinel)

`keel observe` operates the telemetry runtime from local stores — exported JSONL events, WAL
segments, exporter health, the incident file control plane, the trace store and the postgres audit
chain. No live host is required; commands read files or the configured database only.

| Command | Purpose |
|---|---|
| `keel observe status` | runtime status assembled from local stores |
| `keel observe logs [--last n] [--level l]` | tail exported log events (file exporter JSONL) |
| `keel observe errors [--last n]` | error + exception events grouped by fingerprint |
| `keel observe traces [--trace id] [--last n]` | recorded execution traces (`storage/framework/traces`) |
| `keel observe request <id>` | one-command request reconstruction: trace + events + context |
| `keel observe audit [--verify] [--tenant id] [--since ts] [--limit n]` | postgres audit log; `--verify` recomputes the hash chain (D-74) |
| `keel observe incidents` | active + historical incidents (file control plane) |
| `keel observe incident <start\|stop> [--operator name]` | start or stop incident mode (writes the control file) |
| `keel observe exporters` | exporter health snapshots |
| `keel observe policies` | effective telemetry policy rules + sampling rates |
| `keel observe buffers` | WAL buffer state (segments, bytes, watermark) |
| `keel observe health` | health rollup: exporters + incident + WAL |
| `keel observe cost` | cost rollup (`cost.json`) |
| `keel observe metrics` | where metric numbers live (live registry vs local rollups) |

The same incident plane is also exposed over HTTP to session-authenticated operators
(`POST /admin/telemetry/incident`, `GET /admin/telemetry/status`), so CLI, HTTP and pollers agree.

## Ecosystem

| Command | Purpose |
|---|---|
| `keel mcp:tools [--format table\|json]` | MCP tool manifest generated from the platform graph |
| `keel sdk:openapi [--output file]` | OpenAPI 3.1 document from module routes (SDK generation) |

`keel dev` remains the one gate-pending placeholder (production serve posture); `keel test` is
implemented as of Wave 5 (see Diagnostics and quality).


Note (S7): sessions are opt-in — enable with `keel flag:set app.sessions 100` before serving browser logins.

Note (v1.1 B-29): `keel shell` boots the exact production engine (same config, pool, flag
snapshot and graph gate as `keel-api`) and gives you a typed REPL: `login <email> <password>
[tenant]`, `whoami`, `ops`, `routes`, `run <module.operation> [json]`, `help`, `exit`.
Every `run` goes through the CommandBus under the logged-in principal with the same policy
engine enforcing capability denial as the HTTP edge — there is no authorization bypass
(denial enforced and writes-nothing is covered by tests). Login verifies argon2 credentials
exactly like `POST /auth/login`. Prod refuses the shell unless `KEEL_SHELL_ALLOW=1`. The
REPL itself runs inside `keel-api --shell` (dep-rules keep tools from linking runtime
crates); `keel shell` spawns it with inherited stdio so the prompt stays interactive.
