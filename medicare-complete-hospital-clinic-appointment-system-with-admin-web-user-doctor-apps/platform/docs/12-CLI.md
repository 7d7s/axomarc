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
| `keel make <kind> <name> --module <m>` | migration/seeder/factory/job/event/listener/command/query/policy/workflow/model/test/resource/request/controller |

## Database

| Command | Purpose |
|---|---|
| `keel migrate` / `migrate:status` / `migrate:fresh` | apply / report / drop+recreate (dev-test) |
| `keel seed [--only a,b]` | idempotent seeders |
| `keel db:inspect` | tables + row counts |

## Operations

| Command | Purpose |
|---|---|
| `keel up` / `keel down [--retry N] [--secret S]` | maintenance mode |
| `keel job:list [--status s] [--limit n]` | job inventory |
| `keel queue:work [--once]` | run the worker |
| `keel queue:failed` / `queue:retry <id>` | dead-letter triage |
| `keel schedule:list` / `schedule:run` | schedules |
| `keel key:generate` | new application key into config |
| `keel config:dump` | effective config, secrets redacted |
| `keel serve [--worker]` | one-command local loop: spawns keel-api (and keel-worker), streams logs (S7) |
| `keel shell` | interactive platform console (v1.1 B-29): REPL over the booted engine, dispatches under your principal |
| `keel cache:clear` | clear the app cache |
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

`keel dev` and `keel test` are gate-pending placeholders reserved for their milestones.


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
