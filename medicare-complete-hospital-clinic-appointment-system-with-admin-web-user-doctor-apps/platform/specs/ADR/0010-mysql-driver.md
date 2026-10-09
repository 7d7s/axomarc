# ADR 0010 — MySQL as a first-class SQL driver (keel-mysql)

Status: **Accepted** (2026-10-03) · Gate: Wave 9 B-31 (docs/19 parity program) ·
Implementation: lands with the B-31 change set itself — the docs/18 DoD demands the
decision doc *first* ("decision doc first; if adopted: second driver passes the full
store matrix (CRUD/tenant/transactions); migrations remain SQL-first"), so this ADR is
that decision record.

## Context

B-31 (docs/19) asks whether the platform can be fabric-portable: today
`crates/data/postgres` is the only SQL fabric, `[database].url` is assumed
`postgres://`, and boot, migrations, and every pool-backed store speak Postgres
directly (advisory locks, `ON CONFLICT`, `RETURNING`, RLS as defense-in-depth).
Constraints that bound the answer:

- **Zero new external dependencies** (the wasmtime amendment for B-30 was already
  spent; external direct count must stay 25).
- **Migrations remain SQL-first** — files are the source of truth; no ORM-generated
  schema, no builder DSL.
- **Byte-identical default path** — `MODULE_ABI` stays 1, the suite count stays 150,
  and no existing test may change behavior.
- `sqlx` is already the access layer (Postgres feature, offline lockfile), so the
  second driver has to ride the existing dependency, not add one.

## Decision

### 1. The driver is selected by config — zero test edits

- New key **`[database].driver = "postgres" | "mysql"`**, default `postgres`.
  An unknown value fails config load (fail-fast, like every other enum in the
  schema); the key is declared in `CONFIG_SCHEMA`.
- The section type becomes a wrapper `DatabaseSection { driver, #[serde(flatten)]
  DatabaseConfig }` with `Deref`/`DerefMut` into the connection config. This keeps
  every existing `db.url`/`db.max_connections` site — including the config suite's
  literal expectations — compiling and reading unchanged; `PlatformConfig` and
  `FrozenConfig` gain `database_driver()`.

### 2. A new publishable crate owns the second fabric (crate decision)

- **`keel-mysql` at `crates/data/mysql`**, mirroring the keel-postgres surface:
  `create_pool`, `run_migrations` (+ `GET_LOCK` migration lock), `seed_tenant`, and
  every pool-backed store behind the *same* `keel-*` traits (model store, session
  store, principal/credential/token stores, job store + admin, batch ledger,
  notification store, outbox store, idempotency, transaction backend, seeder
  registry). Both fabrics implement one trait set, so the store matrix tests run
  against either without changing assertions.
- **`DbPool { Postgres(PgPool), MySql(MySqlPool) }`** lives in keel-mysql. The two
  pool types both come from sqlx, so neither fabric crate depends on the other;
  boot code matches on the enum once and hands out trait objects everywhere else.
- **Counts deviation, recorded deliberately:** members 68 → 69, publishable
  61 → 62 (gated stays 7). The earlier "publish stays 61" note is overridden by
  the crate decision; `PUBLISHING.md` and the delivery notes carry the new
  numbers, historical CHANGELOG rows stay untouched.

### 3. Full switch scope

- **keel-api, keel-worker, and keel-console** all select on
  `database_driver()`: probe, migrate, seed, and every store construction site.
  Degraded boot (D-52) is untouched — no database or unreachable + not required
  still lands on the in-memory stores, on both drivers.

### 4. Migrations stay SQL-first; pairs ship one directory to both drivers

- A parallel set **`database/migrations-mysql/`** carries the same nine migration
  filenames in the same order, adapted to MySQL dialect (below). sqlx bookkeeping
  stays the shared `_sqlx_migrations` table per database; each driver embeds its
  own directory.
- **Canonical bytes — added by B-31 Phase D (2026-10-09).** Those nine files are
  builder-generated artifacts: `keel_mysql::schema` is a fluent schema builder
  (`Migration::create_table` / `alter_table` / `create_index` / `comment` /
  `section` over `Table`/`Alter` column helpers) that renders every shipped
  migration byte-for-byte, and the pure gate
  `crates/data/mysql/tests/schema_roundtrip.rs` — no database, runs in every
  workspace gate — asserts builder output ≡ file bytes, so a hand edit the
  builder cannot represent fails the gate. Phase D canonicalized the files'
  layout once (name field = longest name + two, in `CREATE TABLE` bodies and
  `ALTER` groups alike; two-space inline comments; one-line `CREATE INDEX`) and
  proved the change whitespace-only — character-stripped equality against the
  pre-canonicalization bytes plus byte-identical SQL literals and backtick
  identifiers — before re-applying all nine to MySQL 8.0.44 from a dropped
  database (25 tables, nine fresh `_sqlx_migrations` checksums,
  `migrate:status` 9 applied / 0 pending). The SQL-first decision itself is
  unchanged: the builder authors the committed bytes; it does not generate DDL
  at `keel migrate` runtime the way Laravel's `Blueprint` closures do, and the
  gateway that decision protects (`migrate:status` before anything runs) is
  untouched.
- **Pairs — amended by B-31 Phase C (2026-10-08).** The decision as accepted
  read: "`keel-mysql::run_migrations` **fails closed** if
  `database/migrations-pairs` ships any `.up.sql`/`.down.sql` pair: pairs are an
  advisory-lock/`ON CONFLICT` Postgres feature and are never half-ported"
  (enforced by `refuse_shipped_pairs()`). That clause is **superseded**:
  `keel_mysql::pairs` is a full port of `keel_postgres::pairs` — same shipment,
  same `keel_migrations` bookkeeping, same Laravel batches, same fail-closed
  scan — executed without a transaction because MySQL DDL causes an implicit
  commit: statement by statement, fail-stop-and-report. The fail-closed *intent*
  of the original decision is unchanged and still holds: a broken shipment
  (missing/orphan `.down.sql`, bad base name) refuses `keel migrate` before the
  first statement, now through the shared `pairs::scan_pairs()` instead of an
  outright refusal, because there is no longer anything to refuse. Advisory
  locks and `ON CONFLICT` remain genuinely Postgres-only and stay documented as
  such (§5, docs/05).
- Console on the mysql driver (as amended by Phases C and E):
  `migrate:rollback`, `migrate:reset`, `migrate:refresh` and
  `migrate:install` dispatch to `keel_mysql::pairs` behind
  `keel_mysql::acquire_migration_lock`. **Phase E lifted the fifth and last
  refusal: `db:schema:dump` now runs on this driver.** The fetch is one
  `SHOW CREATE TABLE` per base table — MySQL's only lossless source
  (`information_schema` cannot express generated-column expressions;
  preparable via `COM_STMT_PREPARE` on 8.0.44; the same bytes `mysqldump`
  and Laravel's `schema:dump` read) — rendered into the same
  `database/schema.sql` contract with exactly two byte-stability
  normalizations, both proven necessary on 8.0.44: strip the
  `AUTO_INCREMENT=<n>` counter, and strip column `CHARACTER SET`/`COLLATE`
  clauses that merely repeat the statement's own table options
  (`SHOW CREATE TABLE` is a one-shot non-round-trip — inherited →
  `COLLATE x` → replay marks explicit → next render expands → fixed point
  from generation 2; the stripped inherited form is semantics-preserving
  because charset and collation come from the table, and it is what the
  server renders back byte-identically: dump → replay → dump and
  reset → migrate → dump both equal the original bytes). The `platform`
  console deletes `pg_only_on_mysql` (dump was its only user). **Phase F lifted the
  last pg-only code path: the Phase 4 pair `keel promote` / `keel rollback`
  (console of the `keel-platform` workspace only) now run on this driver too.**
  `wire_promotion` no longer refuses `mysql`; its events/migration/probe ports
  (`EventAdapter`, `MigrationAdapter`, the health probes) dispatch on the
  driver-selected `DbPool` to `keel_postgres::pairs`/`PgOutboxStore` or
  `keel_mysql::pairs`/`MySqlOutboxStore` — the same enum every other console
  command selects on — and the twin's `pg_only_on_mysql` helper is deleted
  (promotion was its last user, so neither console refuses any command). `migrate`/`migrate:fresh`/`migrate:status`/`seed`/`db:show`/
  `db:inspect` all work on both drivers (status lists the MySQL set and the
  pairs applied to it).

### 5. Dialect rules that make the store matrix pass unchanged

- `$n` placeholders → ordered `?`; `IS NOT DISTINCT FROM` → `<=>`;
  `FOR UPDATE SKIP LOCKED` kept; empty `IN` lists short-circuit before the query.
- uuid columns are **`BINARY(16)`** (sqlx's MySQL `Uuid` codec writes raw 16
  bytes), timestamps **`DATETIME(6)`** (`OffsetDateTime` encodes UTC, decodes
  `assume_utc()` — round-trip exact), documents `JSON`, `NOW(6)` for the clock.
- **No `RETURNING`** → upsert/select-back on the same transaction. sqlx sets
  `CLIENT_FOUND_ROWS`, so `rows_affected()` on `UPDATE` counts matched rows —
  the pg-compatible count the stores audit against.
- **`ON CONFLICT DO NOTHING`** → `INSERT ... SELECT ... FROM DUAL WHERE NOT
  EXISTS` with the exact conflict target (same 1/0 `rows_affected`);
  **`DO UPDATE`** → `AS new ON DUPLICATE KEY UPDATE`; the delta sequence becomes
  an upsert increment plus a select of the own value on the same transaction.
- **No RLS on MySQL** → the explicit `tenant_id` predicates every statement
  already carries are the isolation story (pg keeps RLS as defense-in-depth;
  documented divergence).
- **`GET_LOCK`** replaces the Postgres advisory migration lock; partial indexes
  become plain (non-unique) indexes or, for unique semantics, STORED generated
  columns plus a unique index (multi-NULL preserved); keyed `TEXT` becomes
  sized `VARCHAR(n)`; tables use `utf8mb4_0900_ai_ci`.

### 6. Evidence posture

- Suite count stays 150: the live-MySQL matrix is **appended to the existing
  `pg-tests`-gated suites** (same `DATABASE_URL` env var, scheme-based skip —
  `postgres://` runs the pg matrix only, `mysql://` the mysql matrix only). No
  new feature, no new env var, no new test file.
- `docker-compose.yml` gains a `mysql:8.4` service; no CI workflow runs live
  fabric tests (manual posture, unchanged from B-19).
- Analyzer: `crates/data/mysql` classifies as Data with the allowed dependency
  set; sqlx's `mysql` feature adds no external crate to the direct count.

## Consequences

- The platform runs its full store matrix — CRUD, tenant scoping, transactions —
  on two first-class fabrics at **zero new external dependencies**.
- The default path is byte-identical: driver defaults to `postgres`, every
  existing test compiles untouched (the config suite passes with zero edits plus
  one new driver test).
- Publish count moves to 62 (recorded deviation); `PUBLISHING.md` carries
  69 members / 62 publishable / 7 gated.
- PostgreSQL-only surfaces are explicit refusals with docs pointers, not
  silent no-ops — plus the RLS/array/collation divergences documented in
  docs/05-DATABASE and docs/12-CLI. (No console command is pg-only as of
  B-31 Phase F — see §4's amendments: pairs as of Phase C, `db:schema:dump`
  as of Phase E, promotion as of Phase F, which also deletes the last
  `pg_only_on_mysql` helper (the `keel-platform` console's; the `platform`
  console's was dropped in Phase E). The refusal mechanism is now retired
  from both consoles: nothing remains for it to guard. The RLS/array/
  collation dialect divergences are unaffected and stay documented.)
- The base/twin MySQL migration-set divergence is an intentional, tested
  boundary as of B-31 Phase G: the product's nine-file
  `database/migrations-mysql/` set is a byte-identical subset of the
  `keel-platform` twin's twelve — the twin adds exactly the three
  target-architecture migrations the product has not adopted (§3.1 job
  tenant scoping, §5.2 event idempotency, §5.1 job payload versioning) — and
  `keel-platform/crates/data/mysql/tests/migration_set_parity.rs` enforces
  both directions (every shared migration byte-identical; twin extras
  exactly those three, enumerated in `TWIN_ONLY`). Each console therefore
  pairs with its own schema — the product's nine for the Phase C/E live
  gates, the twin's twelve for the Phase F promotion gate — while any
  silent foundation drift or unexpected extra fails the twin's workspace
  gate before it can ship. No schema change; folding the three target-arch
  phases into the product set stays a separate decision.
- Non-goals for B-31: no cross-driver migration (a Postgres dump does not apply
  to MySQL — each database runs its own migration set), no sharding/failover,
  no driver switching at runtime (selection is boot-time config only).
