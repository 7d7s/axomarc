# 05 — Database

PostgreSQL is the reference data layer (`crates/data/postgres`, kelson-postgres). It implements the
execution and service ports; nothing in the kernel imports it directly.

MySQL is the second first-class fabric (`crates/data/mysql`, kelson-mysql, ADR 0010): the same
store traits, selected at boot by `[database].driver`. The dialect divergences between the two
are listed in the [MySQL driver (B-31)](#mysql-driver-b-31) section.

## Layout

```
database/
  migrations/         timestamped SQL files (kelson make:migration) — PostgreSQL
  migrations-mysql/   the same migrations adapted to MySQL (B-31, forward-only)
  roles.sql      role separation: owner != runtime (least privilege)
config/dev.toml  [database] url, max_connections, run_migrations, driver
```

## Migrations

```bash
kelson make:migration add_appointment_status   # timestamped SQL scaffold
kelson migrate                                 # apply pending (advisory-lock guarded)
kelson migrate:status                          # applied vs pending (incl. pair bookkeeping)
kelson migrate:fresh                           # drop + recreate (dev/test only)
kelson migrate:rollback [--step n]             # revert last n batches, reverse filename order
kelson migrate:reset                           # revert everything (dev/test only)
kelson migrate:refresh [--step n]              # rollback n batches, then re-apply (dev/test only)
kelson migrate:install                         # create the keel_migrations bookkeeping table only
kelson db:inspect                              # tables + row counts
kelson db:show                                 # driver, server version, database + inventory
kelson db:schema:dump                          # normalized DDL snapshot → database/schema.sql
```

Gate G08 checks migrations are ordered and present. `migrate:fresh` refuses production postures.

**Advisory lock (Wave 5).** Every `kelson migrate` run first takes `pg_advisory_lock(7259)` and
fails closed on timeout, so concurrent migrators serialize instead of interleaving DDL — asserted
by the pg-gated `advisory_lock_serializes_runs_and_fails_closed_on_timeout`
(`crates/data/postgres/tests/pg_pairs.rs`, skipped without `DATABASE_URL`).

**Revertible pairs (Wave 5).** Migrations that come in `NNN_name.up.sql` / `NNN_name.down.sql`
pairs are revertible: `migrate:rollback` walks batches newest-first and applies the `.down.sql`
in reverse filename order (default `--step 1`; batches group every file sharing a timestamp).
The pairs table (`keel_migrations` bookkeeping rows) tracks applied pairs — every pair applied in one
`kelson migrate` run shares the next batch number — so status and rollback agree. Single-file forward-only migrations are **not** revertible — rollback skips them and the
command says so (documented divergence from Laravel, where all migrations must be reversible).
`migrate:reset`/`migrate:refresh` carry the same destructive window as `migrate:fresh`
(dev/test-only, refuse production). Directory conventions, fail-closed scan rules (missing
`.down.sql`, orphan `.down.sql`, bad base name, applied-but-deleted files all refuse) and the
singles-before-pairs run order are documented in `database/migrations-pairs/README.md`; the
pg-gated round trip (`pair_migrations_apply_rollback_and_reset_with_batches` in
`crates/data/postgres/tests/pg_pairs.rs`) writes its own pair fixtures.

**Promotion watermark (Phase 4, §7.1).** A promotion target's `db_version` is a **batch**
watermark, not a migration count: it is read back from `pairs::applied`, which reports the batch
each pair landed in. Rollback therefore asks `pairs::rollback` for the number of distinct batches
*strictly above* the target, derived from `applied` rather than assuming batches are contiguous
from 1 — two runs sharing a batch number count once. `db_version: None` skips the database
dimension entirely, which is how a release with no schema change promotes without touching DDL
(proven by `release_without_schema_change_skips_the_database` in `kelson-deployment`). The
deployment-side semantics and the four-stage pipeline are in `14-DEPLOYMENT.md` §Promotion
pipeline.

**Notifications inbox (Wave 6 P5).** `20261001000001_notifications.sql` creates the `notifications`
table (`id uuid PK, principal_id FK → principals, tenant_id FK → tenants NULL, type, data jsonb,
read_at timestamptz NULL, created_at timestamptz`) with `principal_id + created_at DESC` for the
feed and a partial `(principal_id) WHERE read_at IS NULL` index for the unread count; the same
shape is served by `InMemoryNotificationStore` under degraded boot through the `NotificationStore`
port.

**Batch ledger (Wave 6 P9, B-32).** `20261001000002_job_batches.sql` creates `job_batches`
(`id uuid PK, name text, members jsonb, on_complete text NULL, created_at timestamptz DEFAULT
now(), finished_at timestamptz NULL`) with a partial `finished_at` index — the strictly-before
cutoff `queue:prune-batches` deletes on (open batches keep `finished_at NULL` and are never
pruned). The membership jsonb is written once at dispatch (`ON CONFLICT` refreshes only `name`
and `on_complete`): per-member state stays in `jobs_queue`, so `batch.status` counts live queue
truth and concurrent member finishes or admin ops can never lose an update. `PgBatchLedger`
implements the `BatchLedger` port for the worker's dispatch, kelson-api's batch endpoints and the
console batch commands; degraded boots answer through `MemoryBatchLedger` on the same trait
(D-52).

**OAuth2 client registry (Wave 8 P5, B-21).** `20260826000001_oauth2.sql` creates `oauth_clients`
(`id uuid PK, name text, client_id text UNIQUE, secret_hash text, redirect_uris jsonb, scopes
jsonb, version bigint, created_at/updated_at timestamptz`) — a GLOBAL registry (no tenant
column, hard deletes): token/revoke calls resolve a client by its globally unique `client_id`,
the authorization endpoint resolves it from the session's tenant context through the same key.
Secrets are argon2-hashed through `kelson_identity` (`hash_password`/`verify_password`) and the
plaintext is shown exactly once at creation; redirect allowlists and scopes ride jsonb (data,
not string parsing) and updates bump `version` (CAS) through the model runtime. Codes and
refresh tokens never touch this table — they ride the `TokenService` hashed single-use rows in
`auth_tokens`, and access tokens are ordinary PATs.

## Roles

`roles.sql` enforces two database identities: the migration **owner** (DDL) and the **runtime**
user (DML only). The runtime connection cannot alter schema — schema drift becomes impossible at
the database level.

## What the data layer implements

- The execution backend (transactions, commit/rollback around handler units of work).
- Delta stones + streams (versioned change log), audit chain.
- Job store with `SELECT ... FOR UPDATE SKIP LOCKED` claims.
- Outbox persistence and pump.
- Locks: optimistic CAS (`WHERE id AND version`), pessimistic `FOR UPDATE`, serialized via the
  centralized concurrency port.

## Degraded boot

Every data port has an in-memory reference implementation (`InMemoryBackend`, `InMemoryJobStore`,
`InMemorySearchEngine`, …). The platform boots and passes its golden path without postgres —
postgres swaps in behind the same ports for production.
## Model store port (S8)

Domain modules persist through a semantic CRUD contract, not raw SQL. `kelson-models` defines the
port and the portable model layer; backends swap behind it:

- `ModelStore` — `insert / find / list / update / delete / upsert` over validated `Record`s.
  Records speak one canonical encoding: uuids and RFC 3339 timestamps as strings, numbers as JSON
  numbers, jsonb as nested JSON.
- `InMemoryModelStore` — TestKit and degraded boot (D-52).
- `PgModelStore` (kelson-postgres) — production: typed writes through kelson-query builders,
  `RETURNING *`, soft-delete veil via `deleted_at`, tenant scope on every path (a tenant-scoped
  model queried with no tenant matches nothing).

The Platform phase registers one `ModelStoreRef` carrier; module providers (appointments first)
consume it at boot — a module cannot start without the port.

## Model runtime (S8)

`ModelRuntime` bridges a `ModelDef` to the store port:

- Mass-assignment boundary: only declared fields are accepted; unknown keys are rejected.
- Casts: `json_to_bind` validates every value against its `FieldType` before it can reach SQL.
- Tenant injection: tenant-scoped models require a tenant on insert.
- Timestamps: `created_at`/`updated_at` are stamped when declared on the model.
- Soft deletes: `delete` stamps `deleted_at` unless the model is `.hard_deletes()`; reads veil
  deleted rows unless the trash toggle is set.
- Scopes: named filter transforms applied at `list` time.
- Accessors/mutators: computed fields on read / value transforms on write.
- Model events: `creating/created/updating/updated/deleting/deleted` fire through observers.
- Relation loading: `BelongsTo / HasMany / HasOne / ManyThrough` eager loads in at most 2 queries
  per relation (the N+1 guard `kelson inspect` understands).

## Write builders (S8)

`kelson-query` gained typed write builders with the same discipline as `Select`:

- `Insert` — columns + rows, `ON CONFLICT DO NOTHING / DO UPDATE` upsert, `RETURNING`, and a
  tenant guard: `.tenant_scope` requires every inserted row to bind the tenant uuid.
- `Update` — `SET` + where-conditions, `RETURNING`; an unfiltered UPDATE is a build error unless
  explicitly `allow_unfiltered()`.
- `Delete` — same where-surface; unfiltered deletes are refused.

## PostgreSQL integration matrix

`cargo test -p kelson-postgres --features pg-tests` runs the live-DB matrix (skipped gracefully when
`DATABASE_URL` is unset): principal/credential/token/session/notification stores plus the model
store suite —
CRUD with RETURNING, tenant isolation end-to-end, upsert merge on conflict, soft-delete veil and
trash, BelongsTo eager loading, and the appointments partial-unique index rejecting a second
BOOKED row for the same (tenant, patient, slot) while allowing rebooking after cancel.

Upsert identity rule: `ON CONFLICT ... DO UPDATE` never overwrites the primary key.
`Insert::exclude_from_conflict_update` keeps `id` out of the SET list, so a merge
returns the surviving row unchanged (Laravel `updateOrCreate` semantics).

### Sync bridge runtime contract

The durable store ports are sync; sqlx is async. `kelson_postgres::bridge::block_on` runs each
pooled future on the runtime that owns the pool: `block_in_place` inside multi-threaded
runtimes (axum host, kelson-worker, kelson-console), a single persistent bridge runtime otherwise.
Pooled connections can only be driven by the runtime that created them, so:

- runtime hosts must be multi-threaded (`#[tokio::main]` default);
- integrations that create a pool under a single-threaded runtime and then call the sync
  ports will deadlock by construction -- pg tests therefore pin
  `#[tokio::test(flavor = "multi_thread")]`;
- `adapters/kelson-search-pgvector` carries a deliberate copy of the bridge (the frozen G07
  dependency matrix forbids adapters depending on Data crates).

Local matrix runs (no docker): point `DATABASE_URL` at any reachable PostgreSQL 16+
(e.g. the portable cluster under `C:/Users/Victo/pgportable`) and run
`cargo test -p kelson-postgres --features pg-tests` and
`cargo test -p kelson-search-pgvector --features pg-tests`. Vector/hybrid coverage additionally
needs the `vector` extension (CI image); all other slices run on stock Postgres.

## MySQL driver (B-31)

`[database].driver = "mysql"` (default `postgres`, ADR 0010) selects the second fabric at boot:
`kelson-api`, `kelson-worker`, and `kelson-console` all build the MySQL pool, runner, seeders, and
stores from that key. The default path is byte-identical — with no `driver` key every probe,
migration, and store call runs exactly as before. Degraded boot (D-52) is unchanged on both
drivers: unreachable + not required lands on the in-memory stores.

**Migrations stay SQL-first.** `database/migrations-mysql/` carries the same nine filenames in
the same order as `database/migrations/`, each adapted to MySQL 8 (sqlx bookkeeping stays in
the shared `_sqlx_migrations` table per database; each driver embeds its own directory). The
set is forward-only: `kelson migrate` applies it under `GET_LOCK` (the `pg_advisory_lock(7259)`
twin), `migrate:fresh` drops every table with FK checks off instead of cascading a schema, and
`migrate:status` lists the MySQL set.

**Migrations are builder-generated artifacts (B-31 Phase D).** The authoring DX Laravel
ships — `Schema::create(...)` over a `Blueprint` — has a kelson twin in `kelson_mysql::schema`,
a fluent, byte-stable builder: `Migration::create_table` / `alter_table` / `create_index` /
`comment` / `section` over `Table`/`Alter` column helpers (`t.uuid("id").primary_key()`,
`t.varchar("slug", 128).not_null().unique()`, `.generated_stored(expr)` for the
partial-unique substitute below). The claim is byte-exact, not "equivalent SQL":
`crates/data/mysql/tests/schema_roundtrip.rs` — a pure target (no database) that runs in
every workspace gate — authors all nine migrations as builder programs and asserts the
rendered bytes equal `database/migrations-mysql/*.sql`, so a hand edit the builder cannot
represent fails the gate. Phase D canonicalized the shipped files' layout once (name field =
longest name + two, in both `CREATE TABLE` bodies and `ALTER` groups alike; two-space inline
comments; one-line `CREATE INDEX`) and proved the result whitespace-only: character-stripped
equality against the pre-canonicalization bytes, plus byte-identical SQL literals and
backtick identifiers. The canonicalized set was then re-applied to MySQL 8.0.44 from a
dropped database — 25 tables, nine fresh `_sqlx_migrations` checksums, `migrate:status` =
9 applied / 0 pending — so the builder round-trip inherits Phase A's execution proof
transitively (builder ≡ files ≡ bytes known to execute). `KEEL_SCHEMA_REGENERATE=1 cargo
test -p kelson-mysql --test schema_roundtrip` rewrites the canonical files from the programs;
`platform/` and `keel-platform/` carry byte-identical copies of the builder, the gate, and
all nine files.

**Revertible pairs work on both drivers (B-31 Phase C).** `kelson_mysql::pairs` is
`kelson_postgres::pairs` — the same `<ts>_<name>.up.sql` / `.down.sql` shipment in
`database/migrations-pairs`, the same `keel_migrations` bookkeeping table
(`migration VARCHAR(255)` PRIMARY KEY, `batch INT NOT NULL`,
`applied_at DATETIME(6) DEFAULT CURRENT_TIMESTAMP(6)` — InnoDB cannot key a TEXT column and
MySQL has no `timestamptz`), the same Laravel batch arithmetic and the same fail-closed scan
— with one execution difference: **no transaction**. MySQL DDL causes an implicit commit, so
the Postgres runner's "forward SQL + bookkeeping row commit together or not at all" cannot
hold, and a pair runs statement by statement, fail-stop-and-report: the first error stops the
run, names the pair, and states that statements before it remain applied (never a silent
half-rollback). Laravel concedes the same engine rule — `MySQLGrammar` inherits
`$transactions = false` from `Grammar`, so `Migrator`'s `supportsSchemaTransactions()` gate
runs a MySQL `up`/`down` outside any transaction too. Pair SQL travels over the text protocol
(`COM_QUERY` with the `MULTI_STATEMENTS` capability sqlx declares at handshake), the path the
nine forward files already take: probed against 8.0.44, `COM_STMT_PREPARE` accepts exactly
one statement (two `CREATE TABLE`s -> ERROR 1064 near the second) and refuses transaction
control (ERROR 1295). A broken shipment — missing `.down.sql`, orphan `.down.sql`, bad base
name — still refuses `kelson migrate` before the first statement. Evidence (pg-tests suite,
live MySQL): `mysql_pair_migrations_apply_rollback_and_reset_with_batches`,
`mysql_pair_apply_fails_stopped_and_reports_what_already_ran`,
`mysql_run_migrations_refuses_broken_shipments_then_applies_and_rolls_back`,
`mysql_pair_scan_orders_files_and_refuses_broken_shipments`.

**Console surface on `driver = "mysql"`:**

| Works | Refuses (clear message + docs/05 pointer) |
|---|---|
| `migrate`, `migrate:fresh`, `migrate:status`, `migrate:rollback`, `migrate:reset`, `migrate:refresh`, `migrate:install`, `seed`, `db:show`, `db:inspect`, `db:schema:dump` (B-31 Phase E), the `queue:*`/`schedule:*` commands | — (nothing refuses since B-31 Phase E; `db:schema:dump` was the last original refusal) |

(`kelson promote` / `kelson rollback` - Phase 4, console of the `keel-platform`
workspace only - run on this driver too as of B-31 Phase F: `wire_promotion`
builds the five-dimension wiring against the driver-selected `DbPool`, and the
events/migration/probe ports (`EventAdapter`, `MigrationAdapter`, the health
probes) dispatch to `kelson_mysql::pairs`/`MySqlOutboxStore` per driver, the same
enum every other console command selects on. Phase E had kept them behind their
own `pg_only_on_mysql("promote/rollback")` gate (`PgPool`-typed wiring); Phase F
lifted it, deleting the helper from the `keel-platform` console - promotion was
its last user, so neither console refuses any command now.)

**`db:schema:dump` on MySQL (B-31 Phase E).** The dump fetches one
`SHOW CREATE TABLE` per base table — MySQL's only source that preserves
generated-column expressions (`information_schema` cannot express them), the
same bytes `mysqldump` and Laravel's `schema:dump` read — and renders the
same `database/schema.sql` contract as Postgres (banner + dialect line,
byte-order sorted, `;`-terminated statements, blank line between, honest
empty marker). Two byte-stability normalizations make dump → replay → dump
byte-identical on 8.0.44: the `AUTO_INCREMENT=<n>` counter is stripped, and
a column's `CHARACTER SET`/`COLLATE` clause is stripped when it merely
repeats the statement's own table options (an inherited collation renders
as `COLLATE <table collation>` on first dump, replaying that marks the
column explicit, and the next dump expands it to `CHARACTER SET … COLLATE …`
— the fixed point only arrives at generation 2; the stripped inherited form
is what the server renders back unchanged, and only the two bookkeeping
tables in this `utf8mb4_unicode_ci` database hit it). Neither
normalization changes semantics: the table options own the charset and
collation. Proven live on 8.0.44: 25/25 statements cross-check the server's
bytes modulo those rules, repeat-dump determinism, replay round trip and
reset→migrate round trip are all byte-identical, and the `keel-platform`
dump matches.

**Dialect divergences (documented, not gaps):**

- **No RLS.** MySQL has no row-level security; the explicit `tenant_id` predicates every
  statement already carries are the isolation story. Postgres keeps RLS as defense-in-depth
  on top of the same predicates.
- **Types.** uuid columns are `BINARY(16)` (sqlx's MySQL `Uuid` codec writes raw 16 bytes;
  binds are unchanged), timestamps are `DATETIME(6)` (`OffsetDateTime` encodes UTC and
  decodes `assume_utc()` — round-trip exact), documents are `JSON` (MySQL has no `jsonb`), and
  the clock reads `NOW(6)`.
- **No `RETURNING`.** Upserts select back on the same transaction. sqlx sets
  `CLIENT_FOUND_ROWS`, so `rows_affected()` on `UPDATE` counts matched rows — the
  pg-compatible count the stores audit against.
- **Upsert translation.** `ON CONFLICT ... DO NOTHING` becomes
  `INSERT ... SELECT ... FROM DUAL WHERE NOT EXISTS` preserving the exact conflict target
  (same 1/0 `rows_affected`); `DO UPDATE` becomes `AS new ON DUPLICATE KEY UPDATE`.
- **Locks.** `FOR UPDATE SKIP LOCKED` is kept verbatim for the job claim (MySQL 8 supports
  it); the migration advisory lock becomes `GET_LOCK('keel_migrations', timeout)`.
- **Partial indexes.** Non-unique partial indexes drop the `WHERE` (size/selectivity only).
  The appointments partial-UNIQUE index (`WHERE deleted_at IS NULL AND status = 'booked'`)
  is reproduced exactly with STORED generated columns (`IF(pred, col, NULL)`) plus a unique
  index — non-matching rows key to all-NULL and MySQL unique indexes allow repeated NULLs.
- **No arrays.** `scope_paths text[]` becomes a comma-separated `TEXT` column (real
  divergence; nothing in the runtime reads it yet).
- **Collation.** Tables use `utf8mb4_0900_ai_ci`, so equality and unique indexes are
  case/accent-insensitive where Postgres `text` is case-sensitive (e.g. `by_email` matches a
  differently-cased address; a differently-cased duplicate email is a conflict).
- **Keyed text.** Columns that participate in keys become sized `VARCHAR(n)` (MySQL cannot
  key `TEXT`).

**Evidence.** The live-MySQL matrix rides the same `pg-tests`-gated suites as Postgres:
`cargo test -p kelson-postgres --features pg-tests` with `DATABASE_URL=mysql://...` runs the
MySQL slice (scheme-based skip — each test only runs against its own driver). `docker compose
up -d mysql` starts the `mysql:8.4` service (user/pass/db `kelson`/`keel_dev_only`/`kelson`). No
CI workflow runs live fabric tests — the posture is manual, as it is for Postgres.

## pgvector search adapter (S10, B-14)

`adapters/kelson-search-pgvector` implements the frozen `SearchEngine`
contract (kelson-search) on PostgreSQL + pgvector — **no new dependency**:
embeddings travel in pgvector text form (`[0.1,0.2,...]`) cast with
`::vector`, so plain sqlx suffices.

Layout: one table per index (`kelson_search_<name>`, sanitized to
`[a-z][a-z0-9_]{0,39}`), columns `id TEXT PK, tenant_id UUID, body TEXT,
fields JSONB, embedding vector(N)`. Every query is tenant-scoped.

- `create_table(index, dims)` / pure `create_table_sql(index, dims)` —
  migration-friendly DDL; `dims: None` builds a keyword-only table (no
  extension required), vector/hybrid ops then fail fast with validation
  errors.
- keyword: `to_tsvector('simple', body) @@ plainto_tsquery` + `ts_rank`,
  exact JSONB filters, GROUP BY facets (capped at 25 buckets), LIMIT/OFFSET.
- vector: cosine distance `embedding <=> probe`, `min_score` post-filter.
- hybrid: one windowed candidate query (top 500 by max(kw, sim)), alpha
  blend with keyword normalization in Rust (mirrors the in-memory reference;
  scores are engine-explicit as the contract allows).

Integration tests gate on `--features pg-tests` + `DATABASE_URL` and skip
gracefully when the pgvector extension is unavailable.

## Redis queue store (S10, B-13)

`kelson-worker` selects its `JobStore` by availability: `REDIS_URL` set and
reachable → `RedisJobStore`; else PostgreSQL; else in-memory (D-52). The
Redis store keeps one JSON item per job plus four zsets (pending/running/
done/dead); members encode priority (`{MAX-priority:020}:{id}`) so zset
ordering yields priority DESC within one score. Claim is a single Lua
round-trip (`ZRANGEBYSCORE` + `ZREM` + `ZADD running`) — exactly one worker
wins a job (SKIP-LOCKED equivalent on a shared Redis). Retries use
`kelson_jobs::backoff_delay`; `requeue` revives dead jobs; single-instance
Redis only (cluster key-slot rules out of scope).