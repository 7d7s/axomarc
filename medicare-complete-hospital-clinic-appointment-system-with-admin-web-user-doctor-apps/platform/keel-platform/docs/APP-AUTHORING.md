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
- **No read-model layer.** There is no materialised-view registry, no
  refresh policy and no staleness surface.
- **No PostgreSQL schema-version check** (the MySQL blade carries it).
- **Two Laravel capabilities are absent**: route model binding and the
  database maintenance mode.

Each of these is on the roadmap with its acceptance criteria; none of them
is claimed as shipped.
