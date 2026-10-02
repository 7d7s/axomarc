# 05 — Database

PostgreSQL is the reference data layer (`crates/data/postgres`, keel-postgres). It implements the
execution and service ports; nothing in the kernel imports it directly.

## Layout

```
database/
  migrations/    timestamped SQL files (keel make:migration)
  roles.sql      role separation: owner != runtime (least privilege)
config/dev.toml  [database] url, max_connections, run_migrations
```

## Migrations

```bash
keel make:migration add_appointment_status   # timestamped SQL scaffold
keel migrate                                 # apply pending (advisory-lock guarded)
keel migrate:status                          # applied vs pending (incl. pair bookkeeping)
keel migrate:fresh                           # drop + recreate (dev/test only)
keel migrate:rollback [--step n]             # revert last n batches, reverse filename order
keel migrate:reset                           # revert everything (dev/test only)
keel migrate:refresh [--step n]              # rollback n batches, then re-apply (dev/test only)
keel migrate:install                         # create the keel_migrations bookkeeping table only
keel db:inspect                              # tables + row counts
keel db:show                                 # driver, server version, database + inventory
keel db:schema:dump                          # normalized DDL snapshot → database/schema.sql
```

Gate G08 checks migrations are ordered and present. `migrate:fresh` refuses production postures.

**Advisory lock (Wave 5).** Every `keel migrate` run first takes `pg_advisory_lock(7259)` and
fails closed on timeout, so concurrent migrators serialize instead of interleaving DDL — asserted
by the pg-gated `advisory_lock_serializes_runs_and_fails_closed_on_timeout`
(`crates/data/postgres/tests/pg_pairs.rs`, skipped without `DATABASE_URL`).

**Revertible pairs (Wave 5).** Migrations that come in `NNN_name.up.sql` / `NNN_name.down.sql`
pairs are revertible: `migrate:rollback` walks batches newest-first and applies the `.down.sql`
in reverse filename order (default `--step 1`; batches group every file sharing a timestamp).
The pairs table (`keel_migrations` bookkeeping rows) tracks applied pairs — every pair applied in one
`keel migrate` run shares the next batch number — so status and rollback agree. Single-file forward-only migrations are **not** revertible — rollback skips them and the
command says so (documented divergence from Laravel, where all migrations must be reversible).
`migrate:reset`/`migrate:refresh` carry the same destructive window as `migrate:fresh`
(dev/test-only, refuse production). Directory conventions, fail-closed scan rules (missing
`.down.sql`, orphan `.down.sql`, bad base name, applied-but-deleted files all refuse) and the
singles-before-pairs run order are documented in `database/migrations-pairs/README.md`; the
pg-gated round trip (`pair_migrations_apply_rollback_and_reset_with_batches` in
`crates/data/postgres/tests/pg_pairs.rs`) writes its own pair fixtures.

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
implements the `BatchLedger` port for the worker's dispatch, keel-api's batch endpoints and the
console batch commands; degraded boots answer through `MemoryBatchLedger` on the same trait
(D-52).

**OAuth2 client registry (Wave 8 P5, B-21).** `20260826000001_oauth2.sql` creates `oauth_clients`
(`id uuid PK, name text, client_id text UNIQUE, secret_hash text, redirect_uris jsonb, scopes
jsonb, version bigint, created_at/updated_at timestamptz`) — a GLOBAL registry (no tenant
column, hard deletes): token/revoke calls resolve a client by its globally unique `client_id`,
the authorization endpoint resolves it from the session's tenant context through the same key.
Secrets are argon2-hashed through `keel_identity` (`hash_password`/`verify_password`) and the
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

Domain modules persist through a semantic CRUD contract, not raw SQL. `keel-models` defines the
port and the portable model layer; backends swap behind it:

- `ModelStore` — `insert / find / list / update / delete / upsert` over validated `Record`s.
  Records speak one canonical encoding: uuids and RFC 3339 timestamps as strings, numbers as JSON
  numbers, jsonb as nested JSON.
- `InMemoryModelStore` — TestKit and degraded boot (D-52).
- `PgModelStore` (keel-postgres) — production: typed writes through keel-query builders,
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
  per relation (the N+1 guard `keel inspect` understands).

## Write builders (S8)

`keel-query` gained typed write builders with the same discipline as `Select`:

- `Insert` — columns + rows, `ON CONFLICT DO NOTHING / DO UPDATE` upsert, `RETURNING`, and a
  tenant guard: `.tenant_scope` requires every inserted row to bind the tenant uuid.
- `Update` — `SET` + where-conditions, `RETURNING`; an unfiltered UPDATE is a build error unless
  explicitly `allow_unfiltered()`.
- `Delete` — same where-surface; unfiltered deletes are refused.

## PostgreSQL integration matrix

`cargo test -p keel-postgres --features pg-tests` runs the live-DB matrix (skipped gracefully when
`DATABASE_URL` is unset): principal/credential/token/session/notification stores plus the model
store suite —
CRUD with RETURNING, tenant isolation end-to-end, upsert merge on conflict, soft-delete veil and
trash, BelongsTo eager loading, and the appointments partial-unique index rejecting a second
BOOKED row for the same (tenant, patient, slot) while allowing rebooking after cancel.

Upsert identity rule: `ON CONFLICT ... DO UPDATE` never overwrites the primary key.
`Insert::exclude_from_conflict_update` keeps `id` out of the SET list, so a merge
returns the surviving row unchanged (Laravel `updateOrCreate` semantics).

### Sync bridge runtime contract

The durable store ports are sync; sqlx is async. `keel_postgres::bridge::block_on` runs each
pooled future on the runtime that owns the pool: `block_in_place` inside multi-threaded
runtimes (axum host, keel-worker, keel-console), a single persistent bridge runtime otherwise.
Pooled connections can only be driven by the runtime that created them, so:

- runtime hosts must be multi-threaded (`#[tokio::main]` default);
- integrations that create a pool under a single-threaded runtime and then call the sync
  ports will deadlock by construction -- pg tests therefore pin
  `#[tokio::test(flavor = "multi_thread")]`;
- `adapters/keel-search-pgvector` carries a deliberate copy of the bridge (the frozen G07
  dependency matrix forbids adapters depending on Data crates).

Local matrix runs (no docker): point `DATABASE_URL` at any reachable PostgreSQL 16+
(e.g. the portable cluster under `C:/Users/Victo/pgportable`) and run
`cargo test -p keel-postgres --features pg-tests` and
`cargo test -p keel-search-pgvector --features pg-tests`. Vector/hybrid coverage additionally
needs the `vector` extension (CI image); all other slices run on stock Postgres.

## pgvector search adapter (S10, B-14)

`adapters/keel-search-pgvector` implements the frozen `SearchEngine`
contract (keel-search) on PostgreSQL + pgvector — **no new dependency**:
embeddings travel in pgvector text form (`[0.1,0.2,...]`) cast with
`::vector`, so plain sqlx suffices.

Layout: one table per index (`keel_search_<name>`, sanitized to
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

`keel-worker` selects its `JobStore` by availability: `REDIS_URL` set and
reachable → `RedisJobStore`; else PostgreSQL; else in-memory (D-52). The
Redis store keeps one JSON item per job plus four zsets (pending/running/
done/dead); members encode priority (`{MAX-priority:020}:{id}`) so zset
ordering yields priority DESC within one score. Claim is a single Lua
round-trip (`ZRANGEBYSCORE` + `ZREM` + `ZADD running`) — exactly one worker
wins a job (SKIP-LOCKED equivalent on a shared Redis). Retries use
`keel_jobs::backoff_delay`; `requeue` revives dead jobs; single-instance
Redis only (cluster key-slot rules out of scope).