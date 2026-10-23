# Building an application on Kelson

Kelson is a **framework**: it provides the parts every application needs
(the data layer, the query builder, migrations, the schema builder, the
authorization engine, the tenancy model, the observability plane, the job
queue, the module host and the CLI) so that the application you build on top
of it contains only **your domain**.

This document is the contract for that: what an application gets, how to
use it, and — stated plainly — what is not there yet.

---

## 1. What you get

| Concern | What the framework provides | Where |
|---|---|---|
| The data model | `ModelDef` (the table, the fields, the types, the tenant scope, the soft-delete veil), the `ModelStore` port, the `ModelRuntime` (the lifecycle, the events, the scopes, the relations, the casts, the optimistic locking, the dirty tracking) | `kelson-models` |
| The query builder | The typed `Select` / `Insert` / `Update` / `Delete` / `Truncate` builders, per-dialect rendering, keyset pagination, the aggregates, the CTEs, the window functions, the set operations, the lock modes | `kelson-query` |
| The escape | `SqlExpr` — a **validated** raw expression whose values are **bound** | `kelson-query` |
| The schema | The fluent schema builder (the same builder that renders the shipped migrations, byte for byte), the SQL-first migrations, the rollback-capable pairs, the guarded runner (a failed migration **resumes**), the online change (a live table changes without a long lock) | `kelson-mysql`, `kelson-postgres` |
| Authorization | The capability / scope / resource engine with a **deny by default** posture, the abilities on a token as a ceiling, the policy hooks | `kelson-policy`, `kelson-scope`, `kelson-ownership`, `kelson-elevation` |
| The tenancy | The tenant scope on every query, the tenant override, the per-tenant limits, the cost budget | `kelson-tenancy`, `kelson-feature-flags` |
| The jobs | The queue (the tenant-scoped claim, the skip-locked workers, the deferred fencing, the orphaned recovery, the dead-lettering), the batches, the scheduled work, the outbox | `kelson-jobs`, `kelson-event` |
| The observability | The metrics registry (bounded cardinality), the traces, the logs, the incidents, the **declared SLOs**, the diagnostics bundle | `kelson-observability`, `kelson-telemetry` |
| The module host | The WASM guest modules (the ABI, the capability profiles, the pinned artifacts, the drift detection) | `kelson-wasm`, `kelson-module-sdk` |
| The CLI | `kelson new` (the scaffold), `migrate`, `migrate:status`, `migrate:plan`, `seed`, `doctor` (the preflight), `diagnose` (the state bundle), `graph`, `check`, `route:cache` | `kelson-console` |
| The views | The MVVC layers: the ViewModel layer (a view declares the fields it carries), the binder (`@vm`, path-by-path schema checks at render time), the command binder (`@command`, CSRF-carrying forms to named operations), the `ViewContext` assembler (the reserved keys in one place), the head store (LF-035), the read-model layer (a declared, refreshable projection), the built-in models, and the live loop (an opt-in stateful session whose state IS its view model) | `kelson-views`, `kelson-foundation`, `kelson-realtime` |

## 2. The shape of an application

```
applications/<your-module>/
    Cargo.toml            the module: its contract, its handlers
    src/lib.rs             the domain: the models, the routes, the policies
    artifact/              the built guest artifact (the .wasm)
database/migrations/       the SQL-first schema changes
database/migrations-pairs/ the rollback-capable changes
config/dev.toml           the local configuration
```

`kelson new --with-starter` scaffolds a working application (a module, a web
shell, the error views, the end-to-end kit) so you start from something that
runs rather than from an empty directory.

## 3. Defining a model

```rust
use kelson_models::{FieldDef, FieldType, ModelDef};

let appointment = ModelDef::new("Appointment", "appointments")
    .tenant_scoped()
    .field(FieldDef::new("id", FieldType::Uuid).required())
    .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
    .field(FieldDef::new("patient", FieldType::Text).required())
    .field(FieldDef::new("slot", FieldType::Timestamp).required())
    .field(FieldDef::new("version", FieldType::Int64).required().with_default("1"));
```

The model gives you, with no extra code: the CRUD lifecycle, the tenant
guard on every path, the soft-delete veil, the optimistic-locking version
column, the dirty tracking, the casts (date, decimal, enum, json, array,
encrypted, hashed), the accessors and mutators, the local and global
scopes, the model events, the relations (`has_many`, `belongs_to`,
`many_to_many`, `morph_to`, `many_through`) with their eager loaders, the
`with_count` aggregates, the existence filters, and the pagination.

## 4. Writing a query

The typed path — every identifier validated, every value bound:

```rust
let page = Select::from_table(id("appointments"))
    .select(vec![id("id"), id("patient")])
    .where_eq(id("tenant_id"), tenant)
    .where_op(id("slot"), Operator::Gt, cutoff)
    .order_by(id("slot"), Direction::Asc)
    .limit(25)
    .build(&MYSQL)?;
```

The escape — when the framework does not model what you need (a MySQL
`JSON_TABLE` expansion, a `GROUP_CONCAT`, a custom function). The text is
**validated** (no statement separator, no comment marker, no unbalanced
quote, no statement keyword) and the values are **bound**:

```rust
let doc = SqlExpr::new("doc", vec![])?;
let items = SqlExpr::json_table(&doc, "$[*]", "sku VARCHAR(32) PATH '$.sku'", vec![])?;

let rows = Select::from_table(id("orders"))
    .from_expr(items, id("jt"))
    .select(vec![id("jt.sku")])
    .build(&MYSQL)?;
```

A raw expression that tries to smuggle a statement is refused **at build
time**, so an application author cannot write an injection through it even
by accident.

And when the query goes through the **store** - the path an application
actually takes - the store applies the tenant scope and the soft-delete
veil, and refuses a statement that reads a table other than the model's:

```rust
let rows = store.query_raw(&model, Some(tenant), statement)?;
```

The application's statement does not have to mention the tenant, and the
other tenant's rows are still hidden: the scope is applied where the
guarantee lives, not where it can be forgotten.

## 4b. Configuring a deployment: the profile layer

A **profile** is a named, versioned bundle of configuration that a
deployment selects with one variable:

```bash
KEEL_APP_PROFILE=production ./kelson-demo
```

The profile lives at `config/profiles/<name>.toml`, beside the config file
it refines, and it is a **layer**, not an override:

```text
profile  <  package defaults  <  config file  <  .env  <  environment
```

The config file wins over the profile per key, so a profile is a *preset* the
deployment refines - not something that can silently override what the
application decided. Two rules keep the layer honest: every layer is
schema-checked on its own (a profile can never smuggle an unknown key past
the config schema gate, and the error names the profile at fault), and a
profile that is **named but missing** refuses the boot - a deployment that
asked for a preset and silently got the default is worse than one that
failed to start.

`kelson env` reports the active profile, so an operator can always see
which preset an instance is running.

## 4c. Choosing your infrastructure, and plugging in your own

The infrastructure a deployment uses is **chosen by configuration**, through
a **driver registry**: a named driver, a capability manifest, and an active
instance.

```toml
[cache]
driver = "redis"                 # memory | redis - a typed enum, so an
redis_url = "redis://127.0.0.1:6379"   # unknown name refuses the load
```

A driver is a **recipe**, not an instance. It is built when the
configuration selects it, so a deployment that runs on the local cache
never opens a connection to the shared one:

```rust
let mut registry = CacheDriverRegistry::new();
registry.register(CacheDriver::with_builder(
    "memory",
    CacheCapabilities::process_local(4_096),
    || Ok(Arc::new(MemoryCache::shared(4_096))),
));
registry.register(CacheDriver::with_builder(
    "redis",
    CacheCapabilities::shared(),
    || Ok(Arc::new(RedisCache::connect(&url)?)),
));
registry.activate("redis")?;   // fail-closed: the refusal names the name
                               // and lists what IS registered
```

Four properties an application can rely on:

- **fail-closed** - a name that is not registered is an error, never a
  silent fallback to a default (a deployment that asked for a shared cache
  and got a process-local one is the failure mode that costs a day);
- **the capability manifest** - `active_is_shared()` answers "is this
  cache shared across my instances?", which is the property a multi-node
  deployment needs to check and cannot assume;
- **your own driver** - an application registers its own with the same
  shape as the built-in ones, so a bespoke cache is a registration, not a
  fork;
- **built on selection** - the recipe runs once, when the configuration
  names the driver, and the instance is memoized. Nothing is opened before
  it is chosen.

## 4d. Changing your infrastructure without a restart

The selection is not frozen at boot. A configuration reload that names a
different driver **swaps it under the running service**:

```text
config file edited → the poller notices → the driver is built
                   → the runtime holder is swapped → the next cache read
                     goes to the new driver
```

The cache service reads *through* the holder, so the swap needs no
re-wiring and no restart. The deployment's infrastructure follows its
configuration the way its routes and its flags already do.

```rust
// The subscriber (what the demo's boot registers):
impl ConfigReloader for CacheDriverReloader {
    fn on_reload(&self, snapshot: &ConfigSnapshot) {
        match self.registry.activate(driver_name(snapshot.config.cache.driver)) {
            Ok(name) => tracing::info!(driver = name, swaps = self.registry.swaps(), "swapped"),
            Err(error) => tracing::warn!(%error, "refused; the previous driver stays active"),
        }
    }
}
```

Two rules keep the swap honest:

- **a refused swap changes nothing** - a driver that is not registered, or
  one whose recipe cannot build its store (the shared cache is
  unreachable), leaves the previous driver active. A deployment never
  silently runs on infrastructure it was not configured for;
- **the swap count is the evidence** - `registry.swaps()` answers "did the
  configuration change actually take effect?". Re-activating the same
  driver is not a change and does not count.

One consequence to know: a cache **lock** taken before a swap is released
against the driver that is active afterwards, so it is freed by its lease
(the TTL) rather than by its holder. The lease is what makes the swap
safe.

## 4e. The views: the three MVVC layers

Kelson's views are server-rendered and declarative: a view is a template,
and **what it may read and what actions it may take are declared, not
discovered**. The three layers are framework-provided (ADR 0021,
`specs/ADR/0021-mvvc-layers.md`); an application composes them.

### 4e.1 The ViewModel layer — the data a view may carry

A view model is a plain Rust type that declares, field by field, what it
carries:

```rust
use kelson_views::{FieldKind, FieldSchema, ViewModel, view_schema};

#[derive(serde::Serialize)]
struct DashboardVm {
    appointments_today: u64,
}

impl ViewModel for DashboardVm {
    const VIEW: &'static str = "dashboard.today";
    fn schema() -> &'static [FieldSchema] {
        view_schema![
            FieldSchema::required("appointments_today", FieldKind::Int),
        ]
    }
}
```

The declaration is the render-time contract. When the template reads
under the `vm` namespace, every path is checked against the declaration:
a path the model does not declare, or a value of the wrong kind, is a
VIEW-RENDER refusal at render time — a bound template can never render a
blank where it expected a field, and it cannot read a field it was not
given. `extra()` is the declared escape hatch for genuinely open
payloads (a map an application fills and owns); nothing else leaks in.

A view's relationship with a model is registered, not guessed. A
binding carries the model's declaration with it:

```rust
let mut registry = kelson_views::builtins::builtin_registry();
registry.register(kelson_views::ViewBinding::new::<DashboardVm>("dashboard.today"));
engine.set_registry(std::sync::Arc::new(registry));
```

Render through the bound path and the checks run for free:

```rust
let html = engine.render_bound("dashboard.today", context, &DashboardVm { appointments_today: 4 })?;
```

The reserved context keys (`_csrf`, `_auth`, `_errors`, `_locale`,
`_lang`, `_head`) are assembled in **one** place —
`kelson_foundation::ViewContext` — not scattered across the routes:

```rust
let context = kelson_foundation::ViewContext::localized(&catalog, &locale)
    .with_auth(Some(principal))
    .with_csrf(Some(token))
    .with_errors(errors)
    .into_context(json!({}));
```

(Named cases executed: `a_view_model_declares_its_fields`,
`a_view_model_serialises_to_the_context`,
`a_missing_required_field_is_refused`, `a_field_of_the_wrong_type_is_refused`,
`the_escape_hatch_merges_undeclared_json`,
`a_bound_path_resolves_through_the_model`,
`an_unknown_bound_path_is_a_render_error`,
`a_bound_path_of_the_wrong_type_is_refused`,
`a_template_without_the_directive_behaves_as_before`,
`a_template_that_binds_a_model_it_does_not_have_is_refused`,
`a_built_in_view_is_registered_by_the_framework`,
`the_lint_refuses_a_template_that_reads_an_undeclared_path`.)

### 4e.2 The command binder — an action is a declared operation

A template does not invent an action; it names one a route declared:

```html
@command("checkin", label="Check in")
```

The command registry resolves the named operation to its verb and route
(`kelson_views::CommandRegistry` keyed on the operation; a route that
declared `checkin` as a POST to `/appointments/checkin` is what the form
renders), and `@command` renders a CSRF-carrying form bound to it. The
refusals are fail-closed, at the right layer each: an operation no route
declared is refused; a template rendered where no registry is installed
is refused; a submission that spoofs a declared read-only verb is
refused; a request whose `_csrf` does not arrive is refused; a
`label=""` or an unknown option is refused at parse time, not at
runtime.

The form object is **one definition riding two planes**.
`kelson_views::BridgedForm` starts from the same `FormField` list the
template renders, fills each field from the submission, validates
against the declared rules, and carries the re-render state: a
submission key the fields did not declare is dropped, a declared key the
submission did not send resets to empty — the form the template shows
after a failed submit is the submission as it arrived, with the errors
attached, and the wire plane (`RequestViewModel`) binds to the same
field list.

(Named cases executed: `a_command_binds_to_the_declared_operation`,
`an_undeclared_command_is_refused`,
`a_command_spoofs_the_declared_verb_and_refuses_a_read_only_one`,
`a_command_without_a_csrf_token_is_refused`,
`a_command_requires_an_operation_id`, `a_command_refuses_unknown_options`,
`the_form_view_model_carries_fields_and_errors`,
`a_bridged_form_validates_submitted_data`,
`the_wire_plane_drops_a_key_no_field_declared`.)

### 4e.3 The head store — page furniture, assembled (LF-035)

The `<head>` is not a template concern an application can forget to
merge. `kelson_foundation::HeadStore` is a request-scoped store — the
shell owns one, an included view may contribute one — and the merge rule
is **the child wins over the parent**, then the result is finalised into
the `_head` context key. The store's state does not leak between
requests: it is a value the request owns.

(Named cases executed: `a_head_store_holds_title_and_metas`,
`the_builder_produces_a_head_store`,
`the_head_merges_the_child_over_the_parent`,
`the_head_state_does_not_leak_between_requests`.)

### 4e.4 The read models — a declared, refreshable projection

A read model is an **explicit** projection: a name, the materialised
table, the query that fills it, the model(s) it is derived from, and a
refresh policy (`OnWrite` against named source models, `Interval`
(seconds), or `OnDemand`), plus the staleness surface it reports:

```rust
use kelson_models::{ReadModelDef, RefreshPolicy, ReadModelRegistry};

let mut registry = ReadModelRegistry::new();
registry.register(
    ReadModelDef::new("occupancy", "occupancy_reads", "SELECT ...")
        .source_model("Appointment")
        .refresh_policy(RefreshPolicy::OnWrite(vec!["Appointment".into()])),
)?;
```

A store materialises it (`ReadModelStore`: `refresh`,
`refresh_all_tenants`, `staleness`, `query`, `find` — tenant-scoped by
signature, the way every data call is). An in-memory store ships with
the contract, and a MySQL materialised store (`kelson-mysql`'s
`MySqlReadModelStore`) materialises it in the database. The PostgreSQL
store is not built yet (§6).

(Named cases executed: `a_read_model_declares_its_sources_and_policy`,
`an_on_write_policy_triggers_for_its_sources`,
`the_registry_rejects_duplicate_names`, `the_registry_tracks_stores`.)

### 4e.5 The live loop — a stateful session, opt-in per view

A view whose binding declared itself live can host **sessions**: one
state object per browser, and the state IS the view's declared view
model — there is no second state shape to keep in sync:

```rust
// The binding opts in; nothing else does:
registry.register(kelson_views::ViewBinding::new::<MeterVm>("counter").live());

// An application starts a session and moves its state:
let session = engine.live_session("counter", MeterVm { depth: 0 })?;
session.change(|vm| vm.depth += 1);         // the revision advances, nothing renders
let render = session.rerender(&engine)?;     // the SAME schema-checked render, stamped
```

`rerender` is not a re-render of a copy: it serialises the state,
validates it against the declaration, binds it, and renders it through
the same checks the request path uses — a live state is never a looser
contract than a request-time one. The revision is the state's, not the
wire's.

The wire half (`kelson_views`, feature `live-transport`) carries what
changed: the leaf diff between the last published state and the current
one, published on the existing realtime hub — channel `global:live-<id>`,
event `live.update`, payload `{ session, revision, changes }`. The first
publish carries every leaf; a publish with nothing changed carries
nothing. An id the session registry no longer hosts is **stale** and
refused before a render is paid for.

(Named cases executed: `a_state_change_rerenders_the_view_model`,
`a_live_session_is_opt_in_per_view`,
`a_live_session_on_an_unbound_view_is_refused`,
`a_binding_declares_itself_live`,
`a_change_advances_the_revision_without_a_render`,
`the_diff_carries_only_the_changed_parts`, `a_stale_session_is_refused`,
`the_wire_carries_only_the_changed_parts`,
`the_wire_refuses_to_publish_a_stale_session`.)

## 5. The guarantees your application inherits

- **Tenant isolation** — every query and every write is tenant-scoped; a
  scoped model read without a tenant matches nothing rather than everything.
- **Fail closed** — a missing capability is denied; an unknown
  configuration driver is refused; a schema that is behind what the
  framework requires stops the instance instead of serving against it.
- **Bounded** — the metrics' label cardinality is capped, the telemetry
  queue sheds under pressure, the cost budget refuses a write that would
  exceed the tenant's ceiling before it runs.
- **Provable** — every behaviour above has a named, executed test. The
  Laravel-parity map in `docs/audits/` lists, per capability, the case that
  proves it.

## 6. What is not there yet

Stated plainly, because a framework that overstates itself is not a
framework:

- **No declarative model DSL.** A model is defined in Rust today; a
  manifest-driven definition is not built.
- **No account layer.** The tenancy is tenant-scoped; the
  account-owns-tenants hierarchy is designed but not built.
- **No runtime plugin registry.** The adapters are compile-time traits; a
  manifest-driven, hot-swappable adapter set is not built.
- **No declarative profile layer.** The configuration is TOML; a composed
  profile (the guard, the policy, the tenancy, the telemetry, the limits) is
  not built.
- **No read-model layer for PostgreSQL.** The read-model layer ships: the
  contract, the registry, the refresh policies and the staleness surface,
  with an in-memory store and a MySQL materialised store. The PostgreSQL
  materialised store is not built.
- **No PostgreSQL schema-version check** (the MySQL blade carries it).
- **Two Laravel capabilities are absent**: route model binding and the
  database maintenance mode.

Each of these is on the roadmap with its acceptance criteria; none of them
is claimed as shipped.
