# MVVC-first platform audit — 2026-10-23

Scope: `platform/keel-platform` only (framework + runtime + applications +
config + views + database), audited as an **MVVC-layer wiring audit** —
every Laravel-shaped folder mapped to its Kelson counterpart, then checked
for what is wired, what is inert, and what is broken. Read-only except this
directory. Lanes: full sweep (MVVC depth first), scaffolded templates, live
DB/boot sanity, CLI self-checks.

Method: source reads with line-number evidence, CLI self-check runs
(`kelson doctor`, `about`, `graph`, `view:cache`, `migrate:plan`), a TCP
reachability probe of the configured infrastructure, a red-survey test run
(`cargo test -p kelson-demo --lib`), and a git-state review. Candidate
findings-register rows live beside this file
(`CANDIDATE-FINDINGS-ROWS.csv`) and are NOT merged into
`docs/audits/kelson-laravel-forensic-2026-10-21/ACTIONABLE-FINDINGS.csv`
until individually approved.

---

## 1. The folder map (Laravel → Kelson)

| Laravel | Kelson counterpart | Folder audited | Verdict |
|---|---|---|---|
| `app/Providers` + `bootstrap/app.php` | `runtime/kelson-demo/src/{host.rs, lib.rs, edge.rs}` | boot: providers, config reload, driver registries, engine build | wired (§2.1), MVVC wiring thin (F-004) |
| `app/Http/Controllers` | module handlers + `graph_routes.rs`, `routes.rs`, `auth_routes.rs`, `broadcast_routes.rs`, `notification_routes.rs`, `telemetry_routes.rs`, `upload_routes.rs`, `batch_routes.rs` | dispatch through the CommandBus | wired (§2.2) |
| `app/Models` | `crates/data/models` + module `ModelDef`s + `RouteBindingResolver` | model lifecycle, tenant veil, bindings | shipped (S-1..S-5 lanes) |
| `app/Http/Requests` (forms) | `BridgedForm`/`FormField` + `RequestViewModel` + `[[validation]]` specs | forms lane | framework shipped; **zero runtime consumers** (F-007) |
| `app/View/Components` | `kelson_views::builtins` (Error/Pagination/Table/Form/Empty/Ops VMs) + `builtin_registry()` | built-ins | shipped; bindings exist but only dashboard/ops has templates in the runtime (F-006) |
| `app/Livewire` | live spine + wire (`live-transport`) | live lane | shipped (LDB-120/121); **no runtime consumer** (F-008) |
| `resources/views` | `config [views].paths` + `runtime/kelson-demo/views` + package namespaces | 9 shipped templates total | **the configured root does not exist** (F-001) |
| `routes/` | `routes/console.toml` + `module.toml [[routes]]` + route files + `route:cache` artifact | graph: 6 modules, 15 routes, 14 ops | wired; all routes `view: null` (F-005) |
| `config/` | `config/{dev.toml, dev-mysql.toml, .env, console.toml}` + profile layer + feature flags | configuration | parses; posture gates MVVC off (F-004) |
| `database/` | `migrations` (13), `migrations-mysql` (14), `migrations-pairs` (1), `rls` (0), `seeds` (0) | schema | migrations shipped; rls+seeds **empty** (F-009) |
| `lang/` | `lang/{en,fr,ar}.toml` + `MessageCatalog` + `ViewContext::localized` | i18n | wired into the reserved context |
| `storage/` | `storage/framework/{views, flags, routes}` | caches | populated by CLI runs; view cache **compiles 0 templates** (F-002) |
| `tests/`, `examples/`, `benchmarks/`, `adapters/` | architecture tests, examples/{minimal,reference}, adapters (8 crates) | support dirs | adapters real; **examples carry zero MVVC usage** (F-005) |
| `bootstrap/providers` halves | `applications/{appointments,counter,echo,oauth2}` (module.toml manifests) | the app side | API-first; no view lane of their own (F-005) |
| `public/` + Vite | `static_assets.rs` + asset manifest + `@asset` | asset path | wired |
| knowledge/docs | `docs/`, `specs/ADR/` | ADR 0021 + plan live | recorded |

## 2. Boot and dispatch (the `bootstrap/` lane) — what is solid

- **Boot assembly** is provider-based and deterministic: Config, Kernel,
  Platform, Identity, Notifications, FeatureFlags providers install in one
  host loop; the views engine is built before providers so notification
  HTML mail can render; `install_view_registry` sets the **built-ins**
  (`lib.rs:636,1413-1417`), `install_command_registry` derives
  `CommandRoute`s from every **mutating graph route** (`lib.rs:1425-1447`
  → `engine.set_commands`), and `@asset` + package view namespaces
  register on the same engine.
- **Dispatch**: graph routes run through validation/middleware →
  `CommandBus`; mutating verbs are CSRF-guarded on the **web plane only**
  (`graph_routes.rs:499 CsrfGuard::apply_route`); API-plane responses stay
  token-authenticated byte-for-byte.
- **The reserved context is assembled once**
  (`graph_routes.rs:1343-1350`): `ViewContext::localized(...).with_auth
  .with_errors .with_csrf .with_head(HeadBuilder)` — the one-assembler
  contract from ADR 0021 is honored at the route edge.
- **Error pages are admitted as views** (`graph_routes.rs:1101-1185`
  `error_view_response`): status-name fallback (`errors.404` /
  `errors.maintenance` / `errors.500`), the built-in `ErrorVm` fills the
  declared shape, debug-detail converges, locale/head injected — fall back
  to JSON when no template exists. The contract is good; shipping is the
  gap (F-006).
- **Wave-2 HTML routes** (`graph_routes.rs:1223+`) render `route.view`
  through `engine.render(name, context)` with the reserved context applied.
- **Red survey**: `cargo test -p kelson-demo --lib` → **122 passed, 0
  failed** (the two hot-reload timing flakes stayed green this run).
- **CLI surface matches the documented contract** (`kelson help`): doctor,
  check, about, graph, new/dev/serve, make:module, migrate:* family, seed,
  view:/route:/config:/queue:/schedule:/pkg:/call/shell — the APP-AUTHORING
  §1 CLI row checked out (an earlier suspicion of missing `new/doctor/
  check/graph/seed` was a grep artifact; the commands exist as clap enum
  variants).

## 3. Findings (order: severity)

The P-scale below is this audit's; the candidate register rows
(`CANDIDATE-FINDINGS-ROWS.csv`) use the house S-scale: P0=S0, P1=S1,
P2=S2, P3=S3.

### F-001 — P0, config: the configured view root is a phantom, and missing roots are invisible

- `config/dev.toml`: `[views] paths = ["resources/views"]` —
  **`platform/keel-platform/resources/` does not exist** (confirmed
  `Test-Path` false). The demo only renders because the boot pushes
  `dashboard::builtin_views_root()` LAST (`lib.rs:632-635`).
- The engine's directory walker **silently swallows** unreadable roots
  (`engine.rs` `collect_view_names` → `let Ok(entries) = read_dir else {}`),
  and `precompile_all` returns `Ok(0)` with no warning for a dead root.
- Repro: `cargo run -p kelson-console -- view:cache` →
  `view:cache: 0 templates compiled into ...storage/framework/views`,
  `EXIT=0`.
- **Impact**: every Laravel-parity boot artifact (`kelson view:cache`,
  `kelson optimize`) optimizes an empty template set while the served
  dashboard templates are compiled lazily per request; a typo'd root would
  also be invisible forever (fail-open reporting).
- **Smallest safe fix**: point `[views].paths` at existing dirs (or create
  `resources/views` as the app-facing root), and make a nonexistent
  configured root a boot-time refusal (fail-closed) or at minimum a
  boot warning naming the root.

### F-002 — P0, wiring: the console's view engine does not reproduce the boot's roots

- The CLI builds its own engine from `[views].paths` only
  (`main.rs:3620-3655` `views_engine`/`views_config`) — it does not push
  `dashboard::builtin_views_root()` nor install package view namespaces,
  both of which the boot does (`lib.rs:632-640`).
- **Impact**: `view:cache`/`view:clear`/`optimize` operate on a
  different engine shape than the server — the "warm every boot-pinning
  artifact" contract (`optimize`) compiles 0 templates while the server
  lazily compiles the real ones. Two resolvers, one truth missing.
- **Smallest safe fix**: one shared roots resolver (config paths + shipped
  builtin root + package roots) used by both the boot and the console.

### F-003 — P0, runtime posture: the MVVC web cycle is inert at the default dev boot

- Feature flags **`app.sessions`** and **`app.realtime`** declare
  `default_stage: Disabled` (`crates/enterprise/feature-flags/src/lib.rs
  :549-565`), rules empty; the dev snapshot shows **5 registered, 0
  enabled** (`kelson about`).
- With sessions off, `csrf_token(state, session)` yields `None`
  (`graph_routes.rs:1197-1204` — "a guest — no session — has no token"),
  so `@csrf` renders no hidden field on any page, and
  `CsrfGuard::apply_route` on web-plane routes will 403 every form POST.
- **Impact**: the CSRF-carrying `@command` form cycle (ADR 0021's command
  layer, S-10) and the forms bridge (S-9) are **unusable in the reference
  runtime as configured** — a chain where the first link (sessions) and
  the last link (postback) are both disabled-by-default.
- **Smallest safe fix**: dev-profile default that enables `app.sessions`
  (rule or explicit enable in `config/dev.toml` flags config), then a
  route.view → @command → POST round-trip case as the gate.

### F-004 — P0, views: no application view anywhere uses the MVVC layers

- All 9 shipped templates (demo `views/` + starter `resources/views/`) are
  plain-echo HTML: **zero** `@vm`, `@command`, `@csrf`, `@auth`, `@lang`,
  `@error`, `@push/@stack` occurrences (grep across every shipped template).
- The demo registers **only** the built-ins (`install_view_registry`), and
  its dashboard renders through un-bound `engine.render`
  (`dashboard.rs:222`). `examples/{minimal,reference}` likewise carry zero
  MVVC usage.
- The appointments module — the actual `app/` the project is named for —
  declares 5 routes, all `view: null` (manifest: every route entry), so
  the hospital app's presentation is entirely outside MVVC today.
- **Impact**: the three-layer MVVC claim is framework-true and
  application-false: no living template exercises the binder, the command
  form, the head, or the live loop. The parity rows (LDB-112..122) rest on
  crate-level cases only.
- **Smallest safe fix**: one app view wired end-to-end in the demo (route
  with `view` + binding + `@vm` + `@command` + `@csrf` (sessions on per
  F-003) + head) with a named case each, lifting the runtime side.

### F-005 — P1, contract: manifest routes cannot declare a view model

- `module.toml [[routes]]` supports `view` (+ `response` html via the
  edge, honored in `graph_routes.rs`) but has **no view-model key**; a
  manifest HTML route therefore renders UNBOUND (`engine.render`, data
  poured straight into the context), and a template with `@vm` inside it
  would refuse unless the app separately registers the binding in Rust.
- **Impact**: the manifest contract (the app-authoring surface most apps
  use) cannot express "this page is rendered from THIS model" — the
  schema-checked path stops at the crate boundary; a graph-declared
  `view` page can never reach `render_bound`.
- **Smallest safe fix**: design decision first (ADR addendum): either
  `[[routes]] view_model = "<registered binding>"` + a boot section that
  maps graph-declared view names to Rust-registered ViewModels, or
  document manifest views as deliberately unbound with `@vm` templates
  refusing unless the app binds. (Report-only: needs owner decision.)

### F-006 — P1, runtime: error HTML pages are contracted but their templates are not shipped in the runtime

- `error_view_response` falls back JSON because the runtime view root has
  no `errors/404.html` etc. — only the **starter template** ships them
  (`templates/starter-template/resources/views/errors/{404,500,
  maintenance}.html`), while the demo runtime ships only
  `dashboard/ops*`.
- **Impact**: every 404/500/maintenance today answers JSON even on
  browser `text/html` requests in the reference deployment; the
  ErrorVm→binding→page pipeline exists and would render if templates
  existed.
- **Smallest safe fix**: copy the starter error templates into the
  runtime view root + one named test ("a 404 renders the error page") as
  the gate.

### F-007 — P1, runtime consumers: forms, read models and the head store have zero runtime consumers

- **Forms**: `BridgedForm`/`FormField` are consumed by crate tests only;
  no boot code, no template, no route builds a form from a
  `[[validation]]` spec (the module validation specs stop at the graph
  gate: `graph_routes.rs:276+`).
- **Read models** (S-6/S-7): `ReadModelRegistry`/`MySqlReadModelStore`
  are installed NOWHERE — no `ReadModelDef` ships, so the materialised
  projection layer the parity row LDB-122 declares has no living
  definition (in-memory + MySQL stores sit idle).
- **Head**: the only consumers are the route-edge `HeadBuilder` calls with
  **hard-coded page identity** ("Kelson"/"Kelson application",
  `graph_routes.rs:1347-1350` and the error page's "Error") — per-page
  head authoring is impossible because no template-side contributor
  (`@push`/`@stack`) occurs in any shipped template and the merge/leak
  machinery is only exercised in foundation tests.
- **Smallest safe fix**: after F-004's first page: a read-model-backed
  page (one `ReadModelDef` + refresh) and a form page whose fields ride
  `BridgedForm`, citing the S-6..S-9 cases as the gate.

### F-008 — P1, runtime: no live-wire edge

- Zero references to `LiveWire`/`LiveSessions`/`state_diff` outside
  `crates/web/views` (demo/http/testkit all clean of them), and
  `app.realtime` is Disabled by default, so the WS/SSE transports that
  would carry `live.update` are not even mounted. Channel authorization,
  deliberately "the runtime's job", is therefore unexercised.
- **Smallest safe fix**: after F-003/F-004, a demo route that hosts a
  session + subscribes the channel (authorization made explicit), then
  the frame preservation case at the runtime level.

### F-009 — P1, CLI drift: `about` budgets against a stale literal

- `kelson about` prints `Dependencies 26/25 direct external (OVER
  BUDGET)`: the console hardcodes `external > 25` + the literal "25"
  (`main.rs:1052-1062`) while the architecture gate's single source of
  truth is `EXTERNAL_CAP = 26` (`tools/kelson-analyzer/src/lib.rs:13`,
  raised when `regex` was added). `cargo xtask dep-rules` answers
  **clean**; the CLI's own report contradicts it.
- **Impact**: a deployment runbook that trusts `kelson about` believes the
  platform is over budget — wrong as filled; the right diagnosis is a
  hardcode-vs-constant drift.
- **Smallest safe fix**: read the cap from `kelson-analyzer`
  (`EXTERNAL_CAP`) instead of the literal.

### F-010 — P2, CLI/observability: `doctor` never probes the database it declares healthy

- `kelson doctor` checks toolchain, config parse, **file presence** of
  migrations/roles/compose and exits ok with the fabric DOWN (this run:
  all infra ports closed) — while `kelson about` honestly reports
  `Database postgres — server version unreachable` and
  `migrate:plan` fails with a bare `[DB_ERROR] a database error
  occurred` (`EXIT=1`, no remediation hint).
- **Impact**: the preflight that should catch "DB not up" passes; the two
  commands disagree with each other and give the operator no
  next-action.
- **Smallest safe fix**: doctor gains a read-only reachability probe
  (connect + `SELECT 1` + migration-table check) reporting the same
  posture `about` emits.

### F-011 — P2, environment: the live fabric is down and the default dev config points at a closed first choice

- Probe (this machine, now): **3306 OPEN**, 3307 CLOSED, 5432 CLOSED,
  6379 CLOSED, 9000 CLOSED. `config/dev.toml` (the default) targets
  `postgres://`@5432 (closed); `config/dev-mysql.toml` targets the
  3307/keel fabric (closed). The only OPEN service is 3306 — a
  different, not-configured port. `doctor` even advises "no
  docker-compose.yml".
- **Impact**: a plain `kelson serve/demo` boot fails at DB time today;
  the "MySQL is the live fabric (3307)" posture currently has no
  running counterpart, so the S-6/S-7 MySQL store cannot be round-tripped
  end-to-end right now. (Environment drift, not a code defect; recorded
  so the next boot-sanity lane isn't a surprise.)
- **Smallest safe fix**: bring the compose/fabric up, or point the dev
  config at the actually-listening instance; then `kelson migrate:plan` +
  `kelson about` as the gate.

### F-012 — P2, runtime: `database/seeds` and `database/rls` ship empty

- `database/seeds` (0 files) and `database/rls` (0 files) — the CLI
  ships a `seed` command and the RLS story cites policy SQL, but the
  reference app carries neither a seed definition nor a shipped RLS
  policy file in those folders.
- **Impact**: `kelson seed` has nothing to run in a fresh clone; the
  onboarding path (dev up → seed → demo) has an unfilled lane.
- **Smallest safe fix**: one dev seed (a demo tenant + operator + the
  appointments sample) and one RLS policy file as reference, invoked by
  the existing CLI.

### F-013 — P3, quality: the two `hot_reload` timing flakes

- Known pre-existing (50ms timing sleeps). This audit's run was green
  (122/0), but the flake class remains: `kelson-demo --lib` timing cases
  are the only red-able tests on the board.
- **Fix sketch**: replace wall-clock sleeps with an injectable clock /
  deterministic wait in those two tests.

### F-014 — P3, audit-state note: the audited tree carried pending work

- The runtime `about`/`graph` outputs above include **uncommitted** working
  tree state: `applications/counter/`, `applications/echo/` are untracked
  (the "6 modules, 15 routes" graph includes them; committed state would
  show fewer), plus 9 modified files (config/dev.toml is line-endings-only
  noise; `host.rs`, `wasm_tests.rs`, `pg_pairs.rs`, `packages.toml`,
  ADR 0020 + 5 untracked docs) — the user's parallel work. The audit's
  route/graph numbers are working-tree numbers by intent.
- Also observed: `storage/framework` artifacts were generated by this
  audit's CLI runs (untracked build state — normal for local runs).

## 4. Still-green ledger (for the record)

- **The framework layers themselves**: ViewModel declaration/refusals,
  the `@vm` binder (unknown path / wrong kind / unbound model ↛ render),
  the lint, `@command` (binds to the declared op; undeclared refused;
  read-only-verb spoof refused; CSRF absent refused; `label=""` and
  unknown options refused at parse), the forms bridge (one definition,
  two planes), the head store (merge child/parent, no leak), the live
  spine + wire (diff leaf set, stale refusal, first publish = full
  leaves) — each landed with its named case (LDB-112..122; the S-13
  citation check: 54 referenced cases, 0 missing).
- **Command/CSRF plane**: session-bound CSRF token derivation
  (`web/http/csrf.rs:67`, `session_csrf_token`), web-plane-only guard
  attachment, API-plane unchanged.
- **CLI breadth**: 60+ commands, `migrate:plan` read-only posture,
  `optimize` order (route→config→view), fail-fast semantics.

## 5. Suggested fix order (post-approval, smallest set first)

1. **F-001 + F-002** (roots: one truth; fail-closed reporting) — unblocks
   every other view edit, small diffs.
2. **F-003** (dev default: sessions on in dev profile) — unblocks the
   CSRF cycle.
3. **F-006** (ship error templates into the runtime root, one named test).
4. **F-004 + F-007** (one app page: binding + @vm + @command + BridgedForm;
   then the read-model page) — the runtime proof of MVVC.
5. **F-009** (about budget constant), **F-010** (doctor probe).
6. **F-005** (ADR addendum decision), **F-008** (live edge after 2-4).
7. **F-011/F-012** environment + seeds as the deployment lane.
8. **F-013** clock injection for the hot-reload tests.

Every fix nominally rides its parity/ledger row: LDB-115 for the built-in
templates (F-006), LDB-118 for the forms consumer (F-007), LDB-122 for
the first read-model consumer (F-007), and the live rows LDB-120..121
once F-008 lands.

## 6. Repro appendix

```text
# phantom root
cargo run -p kelson-console -- view:cache
#   view:cache: 0 templates compiled into ...\storage\framework\views   (EXIT=0)

# budget drift
cargo run -p kelson-console -- about
#   Dependencies 26/25 direct external (OVER BUDGET)
cargo xtask dep-rules
#   dep-rules: clean / lint: clean

# fabric down state
cargo run -p kelson-console -- migrate:plan       # [DB_ERROR] (EXIT=1)
cargo run -p kelson-console -- doctor             # all [ok], DB never probed
Test-NetConnection 127.0.0.1 -Port 3307           # CLOSED (3306 OPEN)

# red survey
cargo test -p kelson-demo --lib                   # 122 passed; 0 failed

# MVVC usage across every shipped template
Select-String views *.html -Pattern '@vm|@command|@csrf|@auth|@lang|@error|@push|@stack'
#   (no matches — runtime + starter + examples)
```
