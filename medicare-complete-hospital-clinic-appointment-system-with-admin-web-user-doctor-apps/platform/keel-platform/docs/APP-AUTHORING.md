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
