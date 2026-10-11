# 12 — CLI Reference

The `kelson` binary (`tools/kelson-console`). Artisan-parity operational surface; every command runs
against the same workspace/config the runtime uses.

## Environment and architecture

| Command | Purpose |
|---|---|
| `kelson doctor` | toolchain, config, database assets, compose file, member count |
| `kelson check` | G07: dependency matrix + budgets; also compiles the platform graph and resolves every route's middleware chain (Wave 1 `GRAPH-MW-UNKNOWN` gate) |
| `kelson security` | secrets hygiene, dependency policy, trace locality |
| `kelson upgrade` | upgrade report: versions, APIs, deprecations, config, migrations, deps |

## Graph and modules

| Command | Purpose |
|---|---|
| `kelson graph` | compile the platform graph from all `module.toml` |
| `kelson modules` | registered modules (version, maturity, plane, capability count) |
| `kelson pkg:list` | the root `packages.toml` index with each entry's manifest + ABI compatibility; exits non-zero when any entry is unusable (Wave 8, B-24) |
| `kelson pkg:add <path>` | validate the package's `module.toml` (the package v1 contract) and append it to `packages.toml` (stored root-relative; duplicates refused) (Wave 8, B-24) |
| `kelson publish <package> [--force]` | materialize an indexed package: migrations → `database/migrations/` (name-prefixed after the sqlx integer version), `[config]` → `config/packages/<name>.toml`, assets → `public/vendor/<name>/`; views register under `<name>::` at boot (no copy). Byte-compared idempotence: identical = skip, different = conflict (exit non-zero) unless `--force` (Wave 8, B-25) |
| `kelson route:list [--format table\|json] [--name F] [--path F] [--plane api\|web]` | declared routes with plane/group/version/deprecation columns; `--name`/`--path` are case-insensitive substring filters, `--plane` validates `api`/`web` (Wave 2) |
| `kelson route:cache` | write the pinned route artifact (Wave 1): resolved per-route middleware chains, bindings and validators → `storage/framework/routes.json`, hash-pinned to `module.toml` + `packages.toml` + indexed package manifests + `middleware.toml` + `MODULE_ABI`; boot refuses a stale artifact |
| `kelson route:clear` | delete the route artifact; boot falls back to live compilation |
| `kelson event:list` | declared events |
| `kelson capabilities` | capability catalog |

## Generators

| Command | Purpose |
|---|---|
| `kelson new [name] [--plane] [--with-starter]` | new project from the template (compiles immediately); `--with-starter` uses the SPA-first starter: auth-wired module + web shell (S9) |
| `kelson make:module <name> [--plane]` | scaffold a domain module |
| `kelson make:migration <name>` | timestamped migration file |
| `kelson make <kind> <name> --module <m>` | migration/seeder/factory/job/event/listener/command/query/policy/workflow/model/test/resource/request/controller + Wave 5's cast/channel/component/exception/mail/notification/observer/rule/scope/job-middleware/middleware/view (27 kinds; kind discoverable via `GENERATOR_KINDS`, loop-tested for substitution + placeholder-free content) |

## Database

| Command | Purpose |
|---|---|
| `kelson migrate` / `migrate:status` / `migrate:fresh` | apply (advisory-lock guarded: `pg_advisory_lock(7259)` acquired before any run, fail-closed on timeout — concurrent runs serialize; `GET_LOCK('keel_migrations')` twin on MySQL) / report (incl. `keel_migrations` pair bookkeeping) / drop+recreate (dev-test) (Wave 3/5, B-31) |
| `kelson migrate:rollback [--step n]` | revert the last `n` batches (default 1) in reverse filename order — pair migrations only; forward-only single-file migrations are not revertible (documented divergence) (Wave 5) |
| `kelson migrate:reset` / `migrate:refresh [--step n]` | revert everything / rollback `n` batches then re-apply pending (both dev/test-only: the destructive window refuses production) (Wave 5) |
| `kelson migrate:install` | create the `keel_migrations` bookkeeping table without running any files (Wave 5) |
| `kelson seed [--only a,b]` | idempotent seeders |
| `kelson db:inspect` | tables + row counts |
| `kelson db:show` | driver, server version, database + the `db:inspect` table/row inventory (Wave 5) |
| `kelson db:schema:dump` | normalized DDL snapshot of the migrated schema → `database/schema.sql` (review artifact; never executed by `migrate`) (Wave 5) |
| `kelson model:prune [--chunk n] [--tenant <uuid>]` | sweep prunable models in chunked passes (1..=200); untenanted by default — passes the tenant only with `--tenant` (Wave 3) |

**MySQL driver (B-31).** With `[database].driver = "mysql"` the whole Database table keeps
working — `migrate` applies the forward-only `database/migrations-mysql/` set, `migrate:fresh`
drops every table (FK checks off) and rebuilds, `migrate:status` lists the MySQL set, `seed`/
`db:show`/`db:inspect` run against information_schema, and (B-31 Phase C) the pair family
`migrate:rollback` / `migrate:reset` / `migrate:refresh` / `migrate:install` reads the same
`database/migrations-pairs` shipment and `keel_migrations` bookkeeping as Postgres — applied
statement by statement with fail-stop-and-report, because MySQL DDL auto-commits (docs/05
records the divergence and the four live tests that prove it). (B-31
Phase E) `db:schema:dump` runs on this driver too: it writes one
`SHOW CREATE TABLE` per base table into the same `database/schema.sql`
contract, normalized for byte-stable replay (the `AUTO_INCREMENT` counter
and column `CHARACTER SET`/`COLLATE` clauses that merely repeat the
statement's own table options are stripped — docs/05). Separately —
Operations table, `keel-platform` console only —
`kelson promote` / `kelson rollback` dispatch on the driver too as of B-31
Phase F: `wire_promotion` wires their events/migration/probe ports on the
driver-selected `DbPool` (pairs + outbox store per driver), and the last
`pg_only_on_mysql` gate is deleted — promotion was its final user, so no
console command refuses this driver anymore.

## Operations

| Command | Purpose |
|---|---|
| `kelson up` / `kelson down [--retry N] [--secret S]` | maintenance mode |
| `kelson job:list [--status s] [--limit n]` | job inventory |
| `kelson queue:work [--once]` | run the worker |
| `kelson queue:failed` / `queue:retry <id>` | dead-letter triage |
| `kelson queue:clear` | purge every pending job — refuses prod/staging (like `migrate:fresh`) and prompts unless `KEEL_FORCE=1` (Wave 5) |
| `kelson queue:flush` | delete every dead job (`flush_cutoff()` = unbounded; `queue:prune-failed` is the age-bounded variant) (Wave 5) |
| `kelson queue:forget <id>` | delete one dead job (unknown id or non-dead job = refusal) (Wave 5) |
| `kelson queue:monitor` | one-shot queue depth report: pending (+ oldest age), running, dead, succeeded counts (pure render, unit-tested) (Wave 5) |
| `kelson queue:prune-failed [--hours n]` (default 168) | delete dead jobs older than the cutoff (Wave 5) |
| `kelson queue:prune-batches [--hours n]` (default 72) | prune terminal batch records finished before the cutoff — real `BatchLedger::prune` over the durable `job_batches` rows (unfinished batches are never pruned), printing the deleted count (Wave 5 flag/default; Wave 6 P9 made it real, B-32) |
| `kelson queue:restart` | write the restart-signal file: workers that observe a signal newer than their start stop after the current job, supervisors respawn them (Wave 5) |
| `kelson queue:listen` | spawn + supervise `kelson-worker`: respawn on every exit with a crash-loop backoff (≥2s runs restart immediately, faster runs back off 1s) (Wave 5) |
| `kelson queue:retry-batch <id>` | requeue every failed/dead member of a batch — the same `BatchOps::retry` the HTTP `POST /api/v1/batches/{id}/retry` runs over the durable batch ledger; the id is validated before any connection is attempted (Wave 5 flag; Wave 6 P9 made it real, B-32) |
| `kelson schedule:list` / `schedule:run` | schedules (information stubs over the `schedule_registry` seam: they report the registry's empty posture — still empty after Wave 6, which delivered the cron/guard vocabulary (P4) but no module-declared schedules) |
| `kelson schedule:work [--every n]` (default 60s) | schedule daemon: tick loop over the registry, persists tick state (last-fired + recorded failures, Wave 6 P4), fires due items (Wave 5) |
| `kelson schedule:test` | dry-run the schedule registry: prints registered vs due-now (pure — nothing fires, nothing is written; disabled entries would show registered but never due) (Wave 5) |
| `kelson storage:link [--force]` | create the public/storage → storage/local symlink (refuses to overwrite without `--force`; `--force` replaces a link or file — a real directory at the path is never deleted automatically) (Wave 5) |
| `kelson call <name> [json] [--principal <uuid>] [--tenant <uuid>]` | dispatch one module operation through the CLI console route: pre-flights against `routes/console.toml` (resolve by name, list on miss), enforces the dev/test-only default operator (explicit `--principal` is the production path — B-33), resolves the tenant (explicit `--tenant` UUID, else the dev/test fallback tenant — absent outside dev/test, so it must be passed), then spawns `kelson-demo --call` which runs the CommandBus with the same policy engine as HTTP (Wave 5; the HTTP edge-stage validation/middleware layer is not re-run — routes are pre-validated at boot) |
| `kelson key:generate` | new application key into config |
| `kelson config:dump` | effective config, secrets redacted |
| `kelson config:cache` / `kelson config:clear` | pin the frozen effective config to `storage/framework/config.json` + content hash / drop it; boot hash-check refuses drift (Wave 4) |
| `kelson config:show <key> [--json]` | print one config section with secrets redacted (Wave 4) |
| `kelson config:publish <package> [--force]` | materialize only the package's manifest `[config]` table to `config/packages/<name>.toml` (the editable overlay boot reads); same idempotence / conflict / `--force` cycle as `kelson publish`; refuses packages without `[config]` (Wave 8, B-25) |
| `kelson config:reload` | Phase 3 §4.1 pre-flight for hot-reload: schema-gate the current config file, print its content hash and the drift state against the pinned `config.json`, with timing; exits non-zero when the config is invalid. A running server's poller picks a valid edit up within its interval and swaps the route/flag views **without a restart** (< 50 ms budget). Runbook: `13-OPERATIONS.md` §Config reload and rollback |
| `kelson promote [--plan <path>] [--dry-run]` | Phase 4 §7.2: run the four-stage pipeline — deploy (module pins) → migrate (pair batches) → activate (config) → canary — checkpointing to `storage/framework/promotion.checkpoint.json` **first** and restoring every dimension automatically if any health gate fails (KEEL-004). Default plan `storage/framework/promotion.json`. `--dry-run` prints the plan and the stages that would run, changing nothing. Runbook: `13-OPERATIONS.md` §Promotion failure |
| `kelson rollback [--checkpoint <path>] [--dry-run]` | Phase 4 §7.1: restore all five dimensions (config, migrations, modules, jobs, events) to a checkpoint. The read-only job and event compatibility guards run **before** any mutation, so an incompatible target fails closed with nothing half-applied; the three mutating dimensions then run config → migrations → modules. Default `storage/framework/promotion.checkpoint.json`. `--dry-run` prints the checkpoint without restoring |
| `kelson serve [--worker]` | one-command local loop: spawns kelson-demo (and kelson-worker), streams logs (S7) |
| `kelson shell` | interactive platform console (v1.1 B-29): REPL over the booted engine — login/whoami/ops/routes/caps/models/run under your principal (Wave 4 adds `caps` + `models`) |
| `kelson cache:clear` | clear the app cache |
| `kelson cache:forget <key>` | remove one cache key (Wave 4) |
| `kelson optimize` | warm all three boot-pinning artifacts in order — route:cache → config:cache → view:cache; aborts at the first refusal (graph gate / config / views) (Wave 5) |
| `kelson optimize:clear` | drop route + config + view artifacts; every step is attempted (a clear never leaves one cache behind because another delete failed), first error reported afterwards (Wave 5) |
| `kelson view:cache` / `kelson view:clear` | precompile every template under `[views].paths` into `[views].cache_dir` (mtime + content hash) / drop the compiled view artifacts; boot recompiles on demand (Wave 2 views engine) |
| `kelson flag:list` / `flag:set <name> <stage> [--tenant id]` | feature flag rollout |

## Diagnostics and quality

| Command | Purpose |
|---|---|
| `kelson inspect execution\|delta\|authorization\|n-plus-one [id] [--limit] [--threshold]` | inspect recorded executions |
| `kelson replay <id> [--dry-run]` | replay a recorded execution (Original vs Replay) |
| `kelson benchmark [--save-baseline] [--budget pct] [--iterations n]` | G15 benchmark gate |
| `kelson build [--profile dev\|release]` | build + release manifest |
| `kelson release` | show the release manifest |
| `kelson about` | Laravel-style environment report: framework/app version, env, debug, DB driver + server version (best-effort `SELECT version()`), cache/queue/session drivers, flag snapshot, graph module/route/operation counts, dep-budget state (Wave 4) |
| `kelson env` | environment summary: the config file the layering resolved, redacted key facts, config-cache pin state (Wave 4) |
| `kelson profile <api\|worker>` | B-27 profiling wrapper: builds the package, then `samply record target/debug/<bin>` when samply is installed; a missing samply fails closed with `cargo install samply` + the docs/13 workflow (Wave 4) |
| `kelson test [--pg]` | run `cargo test --workspace --locked` from the workspace root, streaming lines and tallying suite/test results into a final `PASS`/`FAIL` verdict; `--pg` adds `--features pg-tests` and fails closed without `DATABASE_URL` (Wave 5) |

## Telemetry (Sentinel)

`kelson observe` operates the telemetry runtime from local stores — exported JSONL events, WAL
segments, exporter health, the incident file control plane, the trace store and the postgres audit
chain. No live host is required; commands read files or the configured database only.

| Command | Purpose |
|---|---|
| `kelson observe status` | runtime status assembled from local stores |
| `kelson observe logs [--last n] [--level l]` | tail exported log events (file exporter JSONL) |
| `kelson observe errors [--last n]` | error + exception events grouped by fingerprint |
| `kelson observe traces [--trace id] [--last n]` | recorded execution traces (`storage/framework/traces`) |
| `kelson observe request <id>` | one-command request reconstruction: trace + events + context |
| `kelson observe audit [--verify] [--tenant id] [--since ts] [--limit n]` | postgres audit log; `--verify` recomputes the hash chain (D-74) |
| `kelson observe incidents` | active + historical incidents (file control plane) |
| `kelson observe incident <start\|stop> [--operator name]` | start or stop incident mode (writes the control file) |
| `kelson observe exporters` | exporter health snapshots |
| `kelson observe policies` | effective telemetry policy rules + sampling rates |
| `kelson observe buffers` | WAL buffer state (segments, bytes, watermark) |
| `kelson observe health` | health rollup: exporters + incident + WAL |
| `kelson observe cost` | cost rollup (`cost.json`) |
| `kelson observe metrics` | where metric numbers live (live registry vs local rollups) |

The same incident plane is also exposed over HTTP to session-authenticated operators
(`POST /admin/telemetry/incident`, `GET /admin/telemetry/status`), so CLI, HTTP and pollers agree.

## Ecosystem

| Command | Purpose |
|---|---|
| `kelson mcp:tools [--format table\|json]` | MCP tool manifest generated from the platform graph |
| `kelson sdk:openapi [--output file]` | OpenAPI 3.1 document from module routes (SDK generation) |

`kelson dev` remains the one gate-pending placeholder (production serve posture); `kelson test` is
implemented as of Wave 5 (see Diagnostics and quality).


Note (S7): sessions are opt-in — enable with `kelson flag:set app.sessions 100` before serving browser logins.

Note (v1.1 B-29): `kelson shell` boots the exact production engine (same config, pool, flag
snapshot and graph gate as `kelson-demo`) and gives you a typed REPL: `login <email> <password>
[tenant]`, `whoami`, `ops`, `routes`, `run <module.operation> [json]`, `help`, `exit`.
Every `run` goes through the CommandBus under the logged-in principal with the same policy
engine enforcing capability denial as the HTTP edge — there is no authorization bypass
(denial enforced and writes-nothing is covered by tests). Login verifies argon2 credentials
exactly like `POST /auth/login`. Prod refuses the shell unless `KEEL_SHELL_ALLOW=1`. The
REPL itself runs inside `kelson-demo --shell` (dep-rules keep tools from linking runtime
crates); `kelson shell` spawns it with inherited stdio so the prompt stays interactive.
