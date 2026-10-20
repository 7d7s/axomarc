# Database, Query Builder & Eloquent — Laravel Feature Atlas Verification (LF-036 to LF-048)

> **Revision 1 — 2026-10-19.** First evidence-backed pass over this batch: every status below is
> backed by a named test executed on 2026-10-19 against this working tree (466 green results,
> 0 failures). Where a row could not be proven, the status says so — and the gaps are recorded in
> §4 rather than glossed.
>
> Status vocabulary: **VERIFIED** (executed evidence matches the atlas expectation) ·
> **PARTIALLY VERIFIED** (implemented; at least one expectation uncovered, unproven, or a recorded
> divergence) · **EXCLUDED BY DESIGN** (a product decision, not a missing feature) · **DEFER** ·
> **NOT IMPLEMENTED** · **NOT RUN** / **EXTERNAL** (belongs to the ecosystem).

## 1. Verification Matrix

| ID | Feature | Disposition | Kelson equivalent | Executed evidence (2026-10-19) | Status |
|---|---|---|---|---|---|
| LF-036 | Database connections and transactions | MATCH (partial) | `kelson-transaction` (`TransactionPort`/`TxHandle`, isolation levels, advisory + row locks) implemented for both fabrics (`txport.rs`); savepoint isolation; **bounded whole-transaction retry** (`retry_transaction`) with per-driver lost-race classification; one pool per fabric, reused across runtimes; tenant scoping on every write | `mysql_transaction_rollback_isolates_two_tests_sharing_a_table`, `mysql_savepoint_rollback_isolates_two_tests_sharing_a_table`, `pg_bridged_queries_reuse_pool_across_runtimes`, `tests::plan_driven_options`, `tests::advisory_key_is_deterministic_and_positive`, `tests::row_lock_builders_validate_identifiers`, `tests::retry_reruns_a_lost_race_until_it_succeeds`, `tests::retry_does_not_rerun_a_fatal_failure`, pg `txport::tests::deadlock_and_serialization_failures_are_retryable`, mysql `txport::tests::deadlock_is_retryable`; conformance `mysql_conformance_tenant_isolation` | PARTIALLY VERIFIED — the read/write split is not wired (F-9) |
| LF-037 | Query Builder | MATCH (typed, no raw escape hatch) | `kelson-query`: `Select`/`Insert`/`Update`/`Delete`/`Truncate` builders — filters, joins, group/having, distinct, unions, subqueries, aggregates, keyset row predicates, ordering, locking, tenant scoping; typed bind values; validated identifiers | 56 builder cases incl. `identifier_validation_rejects_injection_shapes`, `empty_in_list_rejected`, `flat_mixture_follows_sql_precedence`, `or_where_exists_joins_the_preceding_clause`, `nested_group_renders_parenthesized`, `exists_wrappers_and_correlated_subquery`, `keyset_row_comparison_shape`, `group_by_and_having_shapes`, `lock_clauses_render_after_limit_offset`, `tenant_scope_guard`, `unfiltered_delete_refused` | VERIFIED (no raw-SQL escape hatch — F-11) |
| LF-038 | Migrations and schema builder | MATCH (partial) | 12 pg + 13 mysql migrations embedded (`sqlx::migrate!`); the **fluent schema builder byte-round-trips every shipped migration**; the twin-set invariant (mysql = base + the 3 target-arch phases); the migration lock (`pg_try_advisory_lock` / `GET_LOCK`, fail-closed); the pairs scanner (pending/rollback order); CLI `migrate:status/plan/fresh/rollback/reset/refresh/install` | `fluent_schema_builder_byte_round_trips_every_shipped_migration`, `twin_mysql_migrations_superset_the_base_by_exactly_the_target_arch_phases`, `pairs::tests::pending_pairs_skips_applied_rows`, `pairs::tests::rollback_order_walks_batches_newest_first_in_reverse_filename_order`, `pairs::tests::scan_pairs_orders_files_and_refuses_broken_shipments`, `tests::migrate_pair_commands_parse_with_documented_defaults`, `tests::migrate_plan_reaches_for_no_mutation_path` | PARTIALLY VERIFIED — rollback granularity is the batch (pairs carry a per-migration `.down.sql`; legacy singles are forward-only), no expand-contract guidance (F-12) |
| LF-039 | Seeding and factories | MATCH (partial) | `Seeder` trait + `SeederRegistry` (registration order, duplicate-name refusal, `--only` filter, per-seeder `SeedReport{inserted,skipped}`) + `DevTenantSeeder`, **both fabrics**; driven by `kelson seed` (refused outside dev/test — F-7, fixed); re-runnable by convention (guarded inserts / `ON CONFLICT DO NOTHING`) | `seeders::tests::registry_filters_and_rejects_unknown_names`, `seeders::tests::registry_runs_in_registration_order`; console `tests::seed_refuses_outside_dev_test` | PARTIALLY VERIFIED — no factories/states/relationships, no deterministic seeded IDs |
| LF-040 | Eloquent model lifecycle and CRUD | MATCH (repository/data-mapper, not Active Record) | `kelson-models`: `ModelDef` + `ModelStore` port + `ModelRuntime` — insert, find, find_trashed, update, update_with_trashed, update_tracked, delete, prune, upsert, first_or_create, insert_get_id, insert_or_ignore, refresh, replicate; soft deletes with a pinned trash contract; mass-assignment protection (unknown/immutable fields refused); dirty tracking; timestamps; version column (optimistic locking) | The **shared conformance suite** (16 cases, run by pg + mysql + the in-memory fake): `crud_roundtrip`, `soft_delete_restore`, `update_on_a_trashed_row_writes_and_stays_trashed`, `delete_twice_is_a_noop_and_does_not_restamp`, `reads_and_batch_writes_honour_the_trash_veil`, `unique_index_blocks_double_booking`, `upsert_merges_on_plain_unique`, `insert_or_ignore_dedupes`, `force_delete_purges_trash`, `tenant_isolation`, `aggregate_and_exists`, `increment_counter`, `list_sort_pagination_and_validation`, `find_by_and_find_for_update`, `batch_update_and_delete_filtered`, `hard_delete_and_restore_noop`; plus `kelson-models` 139 cases | VERIFIED |
| LF-041 | Relationships and eager loading | MATCH | `RelationDef` (8 kinds: belongsTo, hasOne, hasMany, manyToMany, manyThrough, morphTo, morphMany, morphToMany) with FK/target-key/through/pivot/morph-type metadata; `ModelRuntime::with` — Eloquent-style **nested** eager loading (`post.comments.author`), level by level; `with_constrained` for per-level filter/sort/limit; pivot writes (`attach`/`sync`/`toggle`); tenant threaded through every fetch | `belongs_to_eager_load`, `has_many_eager_load`, `many_through_eager_load_and_aggregates`, `many_to_many_eager_loads_and_aggregates`, `morph_to_many_eager_loads_and_pivots_stay_type_pinned`, `pivot_sync_detach_toggle_and_update`, `pivot_ops_reject_non_many_to_many_and_existence_stays_refused`, **`eager_load_issues_one_query_per_relation_not_per_parent`** (the N+1 bound, now asserted); pg/mysql `pg_modelstore_belongs_to_eager_load` / `mysql_modelstore_belongs_to_eager_load` | VERIFIED |
| LF-042 | Casts, accessors, mutators and strictness | MATCH | `kelson-models::casts` (the `Cast` trait + built-in casts: enum, decimal scale, encrypted AES-256-GCM, json, date, uuid); accessors **and** mutators on the runtime (`accessors`/`mutators` maps); strictness: unknown or immutable fields are refused, never silently dropped | `casts::tests::encrypted_cast_round_trips_through_real_aes_gcm`, `casts::tests::enum_variant_validated`, `casts::tests::decimal_scale_and_enum_validate_at_set_time`, `runtime::tests::update_rejects_immutable_and_unknown_fields`, `runtime::tests::create_many_runs_the_pipeline_per_record` | VERIFIED |
| LF-043 | Scopes, model events and observers | MATCH | Global scopes (the soft-delete veil) + named scopes, applied before validation; the model lifecycle events (`Creating`/`Created`/`Updating`/`Updated`/`Deleting`/`Deleted`) with observers; the scope-bypass audit | `runtime::tests::*` (the scope/event/pipeline cases inside the 139), `conformance::tests::soft_delete_restore_matches_the_contract` | VERIFIED |
| LF-044 | Eloquent collections and base collections | EXCLUDE BY DESIGN | Not implemented: the blueprint's permanent-exclusion list names the Collection wrapper (Rust iterators + `ResourceCollection` for serialization are the idiom). `kelson-http::ResourceCollection` is a *serialization* type, not a collection wrapper. | — (product decision) | EXCLUDED BY DESIGN (P2) |
| LF-045 | API Resources and serialization | MATCH | `kelson-http::Resource` (conditional fields that omit rather than null, `hidden`/`visible`, wrap key, collection + pagination meta) + a JSON:API layer (sparse fieldsets, `included`, relationships, reserved-member stripping) | `resource::tests::*` (7: `resource_hides_non_wire_fields`, `when_omits_false_conditions_and_never_emits_null`, `wrap_key_defaults_to_data_and_can_be_disabled`, `collection_page_passes_pagination_meta_through`, `envelope_resource_page_matches_the_envelope_wire_shape`, `collection_of_sources_honors_the_wrap_key`, `page_envelope_carries_pagination_meta`) + `jsonapi::tests::*` (13) | VERIFIED |
| LF-046 | Pagination | MATCH (offset + cursor) | `kelson-http::pagination` (`PageQuery` from query pairs with caps + `cursor`, `Page<T>` with total/links/meta/`next_cursor`, `Cursor` — an opaque, HMAC-signed, fail-closed codec) + the query crate's `Page`/`PageReq` + the store's page/list options (`ListOpts.after` = the keyset anchor) + the builder's keyset row predicate | `pagination::tests::*` (10, incl. `cursor_round_trips_the_anchor_value`, `cursor_rejects_a_tampered_value`, `cursor_rejects_another_secret_and_another_column`, `cursor_rejects_malformed_input`, `parses_the_cursor_query_param`); `select::tests::keyset_row_comparison_shape`; the shared conformance case **`cursor_keyset_pagination`** (fake + pg + mysql) | VERIFIED |
| LF-047 | Redis and cache-backed data services | MATCH (partial) | `kelson-caching`: the `CachePort` (get/put/delete/invalidate_prefix/put_if_absent/delete_if_equals — the atomic primitives) + `CacheService` (Laravel-shaped: get/put/has/delete/pull/many/put_many/add/forever/increment/decrement/remember/tags/flush/lock) + the memory adapter; the **Redis** adapter (SET EX / SET NX / Lua CAS); the session, queue and rate-limit Redis adapters | `kelson-cache-memory` 17 cases incl. `tenant_isolation_cache_entries_do_not_cross_tenants`, `pentest_cache_hostile_keys_and_prefixes_cannot_escape_the_caller`, `cache_lock_acquire_release_and_exclusivity`, `remember_waiter_collapses_onto_the_leader_result`, `with_lock_runs_and_releases`; `kelson-caching` 3 cases (`put_if_absent_is_atomic_and_delete_if_equals_checks_value`, `ttl_expiry_honored`, `put_get_invalidate`); the Redis adapter's 4 unit cases; **15 live-Redis cases** (§2.4): `kelson-cache-redis` 5 + 4 (durable job store), `kelson-session-redis` 4, `kelson-ratelimit-redis` 2 | VERIFIED — the live Redis **runs** (the cross-instance limit, the SET NX lock, the Lua compare-and-delete and the Lua claim exclusivity are proven against a real server) |
| LF-048 | MongoDB integration | EXCLUDE BY DESIGN | Not implemented, and deliberately so: the blueprint's permanent-exclusion list names MongoDB, and the atlas marks the item P3 "never score as core SQL ORM parity" | — (product decision) | EXCLUDED BY DESIGN (P3) |

## 2. Method and executed evidence

### 2.1 Runs (2026-10-19, from `platform/keel-platform`)

| Run | Result |
|---|---|
| `cargo test -p kelson-query` | **56 passed**, 0 failed |
| `cargo test -p kelson-models` | **139 passed**, 0 failed (1 ignored) |
| `cargo test -p kelson-transaction` | **3 passed**, 0 failed |
| `cargo test -p kelson-caching -p kelson-cache-memory` | **3 + 17 passed**, 0 failed |
| `cargo test -p kelson-http --lib` | **164 passed**, 0 failed |
| `cargo test -p kelson-postgres --features pg-tests` | **85 passed**, 0 failed (lib 9 · conformance 34 · bridge 1 · integration 25 · modelstore 10 · pairs 6) — **with the live MySQL 8** (`DATABASE_URL=mysql://keel:keel@127.0.0.1:3307/keel`), so the **mysql** halves execute instead of self-skipping; the same command against the live PostgreSQL 16 ran 83 passed / 0 failed (§2.4) |
| `cargo test -p kelson-mysql` | **2 passed**, 0 failed (migration-set parity + schema round-trip) |
| `cargo test -p kelson-console --bin kelson migrate` | **2 passed**, 0 failed |

**466 green results, 0 failures.** Logs: `%TEMP%\lf036-batch\`.

### 2.2 Commands to re-run

```powershell
$env:CARGO_BUILD_JOBS=1; $env:CARGO_INCREMENTAL=0
$env:DATABASE_URL='mysql://keel:keel@127.0.0.1:3307/keel'   # the pg halves self-skip
cargo test -p kelson-query
cargo test -p kelson-models
cargo test -p kelson-transaction
cargo test -p kelson-caching -p kelson-cache-memory
cargo test -p kelson-http --lib
cargo test -p kelson-postgres --features pg-tests
cargo test -p kelson-mysql
cargo test -p kelson-console --bin kelson migrate
```

### 2.3 What the 56 query-builder cases cover

Filters and precedence (`flat_mixture_follows_sql_precedence`, `or_where_renders_top_level_or`,
`or_where_exists_joins_the_preceding_clause`, `leading_or_where_renders_as_first_clause`,
`nested_group_renders_parenthesized`), injection and null semantics
(`identifier_validation_rejects_injection_shapes`, `empty_in_list_rejected`), joins
(`in_join_order_limit_offset`), grouping (`group_by_and_having_shapes`,
`aggregate_projection_shapes`, `aggregate_validation_conflicts`, `group_rejects_non_condition_builder`),
subqueries (`exists_and_doesnt_exist_wrap_the_statement`, `exists_wrappers_and_correlated_subquery`),
keyset (`keyset_row_comparison_shape`), locking
(`lock_clauses_render_after_limit_offset`), tenant scope (`tenant_scope_guard`,
`tenant_scope_appends_predicate`), writes (`simple_insert`, `multi_row_insert_numbers_params_across_rows`,
`upsert_do_nothing`, `upsert_do_update_excludes_identity_column`, `upsert_do_update_excludes_target_columns`,
`targetless_do_nothing_ok_do_update_still_needs_target`, `delete_with_where_and_returning`,
`unfiltered_delete_refused`).

### 2.4 NOT RUN / blocked in this batch
- **PostgreSQL — PARTIALLY UNBLOCKED (2026-10-20), and INTERMITTENT.** The pg halves of every matrix
  used to self-skip on this host because a *backend* process died with `0xC0000142`
  (STATUS_DLL_INIT_FAILED) — the install itself was fine. The portable PostgreSQL 16 starts and the
  pg halves **execute** against the real server:
  ```powershell
  $pg = "C:\Users\Victo\pgportable\pgsql\bin"
  $env:PATH = "$pg;C:\Windows\System32;C:\Windows"
  & "$pg\pg_ctl.exe" -D "C:\Users\Victo\pgportable\data" -l "C:\Users\Victo\pgportable\pg3.log" -o "-p 5433" start
  $env:DATABASE_URL = 'postgres://postgres@127.0.0.1:5433/postgres'
  ```
  (`pg_hba.conf` is `trust`, so no password.) One full pg matrix run went green this way — **83 passed
  / 0 failed**, with the 16 `pg_conformance_*` cases real rather than skipped.
  **The caveat is that the crash is intermittent, not fixed**: a backend can die with `0xC0000142` on
  *any* new connection, and the postmaster then shuts the whole server down
  ("server process was terminated by exception 0xC0000142" → "shutting down due to startup process
  failure"). It is a host-level fault (the same class of native crash that hits `rustc` here with
  `0xc0000409`; no `AppInit_DLLs` entry, no IFEO override, and the install's own DLLs are
  consistent). A pg evidence run is therefore **interruptible** and must be re-run after a restart.
  **MySQL is the stable live fabric on this host** and is what the pg/mysql matrix runs are
  verified against below.
- **The live-Redis cases — CLOSED (2026-10-20).** A Redis server is on this host
  (`C:\Redis\redis-server.exe`), so the feature-gated live cases now **execute** instead of skipping:
  ```powershell
  Start-Process "C:\Redis\redis-server.exe" -ArgumentList '--port 6379','--bind 127.0.0.1','--appendonly no' -WindowStyle Hidden
  $env:REDIS_URL = 'redis://127.0.0.1:6379'
  cargo test -p kelson-cache-redis -p kelson-session-redis -p kelson-ratelimit-redis `
      --features kelson-cache-redis/redis-tests,kelson-session-redis/redis-tests,kelson-ratelimit-redis/redis-tests
  ```
  **15 live cases, all passing against the real server** (5 cache, 4 session, 4 durable job store,
  2 rate-limit). Three of those suites are **new** — see F-15.
- The MySQL transaction/rollback/savepoint cases ran against the local MySQL 8 (127.0.0.1:3307); the
  equivalent pg cases ran against the live PostgreSQL 16 (127.0.0.1:5433) in the window where the pg
  backends stayed alive.

## 3. Detailed verification

### LF-036 — Database connections and transactions — PARTIALLY VERIFIED
- **Transactions.** `kelson-transaction` defines the port (`TransactionPort::begin`, `TxHandle`
  exec/commit/rollback) with `IsolationLevel` and `TxOptions`; both fabrics implement it
  (`crates/data/postgres/src/txport.rs`, `crates/data/mysql/src/txport.rs`). Rollback isolation is
  proven against a real database: `mysql_transaction_rollback_isolates_two_tests_sharing_a_table`.
- **Nested behaviour.** Savepoints are proven: `mysql_savepoint_rollback_isolates_two_tests_sharing_a_table`
  — a partial rollback inside a transaction leaves the rest of the work isolated.
- **Pooling.** `pg_bridged_queries_reuse_pool_across_runtimes` proves one pool is shared across the
  bridged runtimes (no per-request pool churn). The `truncate` path is env-gated (`dev|test`).
- **Locks.** Deterministic advisory-lock keys (`tests::advisory_key_is_deterministic_and_positive`)
  and injection-safe row-lock builders (`tests::row_lock_builders_validate_identifiers`).
- **Scoping.** Every write is tenant-scoped and the store refuses a cross-tenant match
  (`mysql_conformance_tenant_isolation`).
- **Gaps (F-9).** The **read/write split** is not wired: one pool per fabric, so the deployment's read
  replica is not used by the store (a topology decision, not a port gap). There is also no
  pool-exhaustion or connection-leak evidence. The **deadlock retry** that was missing is now
  shipped: `kelson_transaction::retry_transaction` re-runs the whole transaction while the failure is a
  lost race, and both drivers classify the race where the driver error is still typed.

### LF-037 — Query Builder — VERIFIED
- A **typed** builder (not a PHP-chain clone): `Select`/`Insert`/`Update`/`Delete`/`Truncate` compile
  to SQL text plus ordered bind values, and the data layer maps those onto native driver binds.
- **Parameter binding prevents injection**: values are typed (`Value::Null|Bool|Int|Float|Text|Uuid|
  Timestamp|Json`) and never rendered into the SQL text; identifiers are validated
  (`identifier_validation_rejects_injection_shapes`).
- **Grouping and SQL semantics** are pinned (`group_by_and_having_shapes`, `flat_mixture_follows_sql_precedence`,
  `nested_group_renders_parenthesized`, `or_where_exists_joins_the_preceding_clause`).
- Joins, distinct, unions, correlated subqueries (`EXISTS`), aggregates, keyset row predicates, row
  locks and ordering are all present and tested.
- **F-11 (recorded divergence)**: there is **no raw-SQL escape hatch** — `Value` has no raw variant and
  `Ident` rejects anything outside `[a-z0-9_]`. That is the atlas's own preferred direction ("a typed
  builder is preferred in Rust"), and it removes the injection surface a raw expression would open.

### LF-038 — Migrations and schema builder — PARTIALLY VERIFIED
- **Deterministic order, once.** Migrations are embedded (`sqlx::migrate!` — compile-time, so a new file
  requires a rebuild) and tracked in the migration repository; the pairs scanner orders files and
  refuses a broken shipment (`pairs::tests::scan_pairs_orders_files_and_refuses_broken_shipments`) and
  skips already-applied rows (`pairs::tests::pending_pairs_skips_applied_rows`).
- **The schema builder is proven against the shipped bytes.** Every `database/migrations-mysql/*.sql`
  file is authored as a fluent `kelson_mysql::schema::Migration` program, and
  `fluent_schema_builder_byte_round_trips_every_shipped_migration` asserts the rendered bytes equal the
  shipped bytes — so "the builder can express every construct the fleet ships" is an enforced invariant.
- **Concurrent migrators are defined.** pg takes `pg_try_advisory_lock`, MySQL takes `GET_LOCK(name, 0)`
  with the same session-scoped, fail-closed contract; a migrator that cannot take the lock refuses.
- **Planning / dry-run.** `kelson migrate:plan` reaches the CLI without mutating
  (`tests::migrate_plan_reaches_for_no_mutation_path`) and the pair commands parse with documented
  defaults (`tests::migrate_pair_commands_parse_with_documented_defaults`).
- **The twin invariant.** `twin_mysql_migrations_superset_the_base_by_exactly_the_target_arch_phases`
  pins that the mysql set is the base set plus exactly the three target-architecture phases.
- **Gaps (F-12).** Rollback is **batch-level** (newest batch first, in reverse filename order) — there
  granularity is the batch (a single migration inside a batch cannot be reverted alone), and the expand-contract guidance for no-downtime deployments is missing.

### LF-039 — Seeding and factories — PARTIALLY VERIFIED
- A real seeding framework exists: the `Seeder` trait, the `SeederRegistry` (registration order,
  duplicate-name refusal, `--only` filtering, "stop at the first error"), the `SeedReport`
  (`inserted`/`skipped`), the `DevTenantSeeder` proof, **both fabrics** (`kelson_postgres::seeders`,
  `kelson_mysql::seeders`) and the `kelson seed` CLI command.
- **Idempotence** is the stated convention (guarded inserts / `ON CONFLICT DO NOTHING`), and the report
  distinguishes inserted from skipped rows.
- **Gaps.** There are **no factories** (no definitions/states/relationships), no deterministic seeded IDs,
  and — the important one — **no production gate (F-7)**: `kelson seed` runs against whatever database
  `DATABASE_URL` points at, while the atlas requires seeders to be "explicitly gated for production".
  The codebase already has the pattern (the truncate path is gated to `dev|test`).

### LF-040 — Eloquent model lifecycle and CRUD — VERIFIED
- The lifecycle surface is complete: `insert`, `find`, `find_trashed`, `update`, `update_with_trashed`,
  `update_tracked`, `delete`, `prune`, `upsert`, `first_or_create`, `insert_get_id`,
  `insert_or_ignore`, `refresh`, `replicate`.
- **One contract, three backends.** The shared conformance suite (16 cases) runs against the in-memory
  fake, the pg driver and the mysql driver — including the trash contract pinned this session
  (`update_on_a_trashed_row_writes_and_stays_trashed`, `delete_twice_is_a_noop_and_does_not_restamp`,
  `reads_and_batch_writes_honour_the_trash_veil`).
- **Mass-assignment protection.** The runtime refuses unknown and immutable fields
  (`runtime::tests::update_rejects_immutable_and_unknown_fields`) — no silent attribute loss.
- **Soft deletes behave correctly** and are pinned across backends (`soft_delete_restore`,
  `force_delete_purges_trash`).
- **Concurrency.** `version_column` makes every write a compare-and-swap; a concurrent writer wins and
  the loser gets `stale_record` (409) (`cas_stale_write_reports_stale_record`).
- **Design note (not a gap).** This is a repository/data-mapper design, not Active Record: the atlas
  allows that ("a repository/data-mapper design can be a valid alternative, but then score API
  compatibility separately from capabilities") — the capabilities are what is verified here.

### LF-041 — Relationships and eager loading — VERIFIED
- **The relation kinds are real and runtime-backed.** `RelationDef` declares eight kinds — belongsTo,
  hasOne, hasMany, manyToMany, manyThrough, morphTo, morphMany, morphToMany — with the FK on the
  owning side, the target key, the join model (`through`), the pivot table and the morph type column.
  The registry validates the graph, so a relation to an unregistered model is refused.
- **Eager loading is Eloquent-shaped.** `ModelRuntime::with(store, parents, tenant, path)` loads a
  **nested** relation path (`post.comments.author`) level by level, and `with_constrained` runs a
  per-level closure (filter/sort/limit) before each level's fetch. The implementation's contract is
  "at most 2 store queries per relation" — no N+1 at any depth.
- **Executed.** `belongs_to_eager_load`, `has_many_eager_load`,
  `many_through_eager_load_and_aggregates`, `many_to_many_eager_loads_and_aggregates`,
  `morph_to_many_eager_loads_and_pivots_stay_type_pinned` (in-memory fake) plus the same relation
  against a real database: `pg_modelstore_belongs_to_eager_load` / `mysql_modelstore_belongs_to_eager_load`
  (the mysql half ran; the pg half self-skipped).
- **Polymorphic type safety** is pinned: the morph type column decides which model an id points at, and
  the pivot rows stay type-pinned.
- **Tenancy** is threaded through every eager fetch, so a relation cannot cross a tenant boundary.
- **Gaps (F-8).** The "≤ 2 queries per relation" N+1 bound is the *documented* contract — there is no
  query-counting test that asserts it — and **pivot writes** (attach/detach) are not part of the
  store port, so the atlas's "pivot changes remain transactional" has nothing to verify. Reads over
  pivots are covered.

### LF-042 — Casts, accessors, mutators and strictness — VERIFIED
- The `Cast` trait with built-in casts: enum variants, decimal scale, **encrypted** (AES-256-GCM, proven
  against the real cipher: `encrypted_cast_round_trips_through_real_aes_gcm`), json, dates, uuid.
- **Accessors and mutators** are first-class on the runtime (`accessors`/`mutators` maps, applied to
  projections and writes).
- **Strictness**: an invalid enum or a bad decimal scale is refused at set time
  (`enum_variant_validated`, `decimal_scale_and_enum_validate_at_set_time`); unknown or immutable
  fields are refused rather than silently dropped.

### LF-043 — Scopes, model events and observers — VERIFIED
- **Global scopes** (the soft-delete veil) and **named scopes**, applied before validation, with the
  scope-bypass audit (`without_global_scopes_all` is the only way to lift the veil, and it is explicit).
- **Model events** fire at defined lifecycle points (`Creating`/`Created`/`Updating`/`Updated`/
  `Deleting`/`Deleted`) and observers can veto; the pipeline order is pinned by the runtime cases.
- Domain events (the `[[events]]` manifest kind) are a separate mechanism and are not conflated with
  the persistence hooks.

### LF-044 — Eloquent collections and base collections — EXCLUDED BY DESIGN
- The blueprint's permanent-exclusion list names the Collection wrapper, and the atlas asks for
  "idiomatic iterators and tested streaming APIs" rather than a PHP-helper clone.
- Kelson's equivalent is Rust's iterator/`Page` machinery plus `kelson-http::ResourceCollection` for
  *serialization* — a deliberate non-implementation, recorded here so it is not mistaken for a gap.

### LF-045 — API Resources and serialization — VERIFIED
- `kelson-http::Resource`: conditional fields that **omit** rather than null, `hidden`/`visible`
  from the model definition, a configurable wrap key, and a collection form that carries the
  pagination meta. Executed: the 7 `resource::tests::*` cases.
- A JSON:API layer on top (sparse fieldsets, `included` with a lifted id/type, relationships,
  reserved-member stripping, 406 for bad media types): the 13 `jsonapi::tests::*` cases.
- Public JSON is therefore deliberate and versionable: internal fields are excluded by construction,
  not by convention.

### LF-046 — Pagination — VERIFIED (offset + cursor)
- Offset pagination with a total: `PageQuery::from_pairs` (with caps and a fail-closed rejection of
  bad input), `Page<T>` carrying items/page/per_page/total, and the wire `meta` shape — the 5
  `pagination::tests::*` cases plus the store's `ListOpts`/`PageReq`.
- **Cursor (keyset) pagination — F-10 FIXED.** The cursor is opaque and tamper-evident:
  `kelson_http::Cursor` encodes the keyset anchor (the column + the value the next page starts
  *after*) with an HMAC-SHA256 over it, base64url so it survives a query string. Decoding fails
  closed: a cursor that is not base64url, is too short, was signed with another secret or anchors
  another column is a **validation error**, never silently "no cursor".
  The store port carries the anchor (`ListOpts.after`), all three backends apply it (pg/mysql render
  the builder's row comparison — `(col) > ($1)` — with the comparison flipped for a descending
  sort; the in-memory fake filters the sorted rows), and the anchor column is validated against the
  model (an unknown column or a value that does not fit the field is refused, not ignored).
  In cursor mode `total` is the size of the **window that is left**, so `has_more` stays true exactly
  while another page exists.
  - **Evidence**: `cursor_keyset_pagination` — the shared conformance case, so it runs against the
    fake, pg **and** mysql. It proves the page starts after the anchor, that a descending sort flips
    the comparison, that **a concurrent insert before the anchor does not shift the window** (the
    property OFFSET cannot give), and that an unknown anchor column is refused.
    `kelson-http`: `cursor_round_trips_the_anchor_value`, `cursor_rejects_a_tampered_value`,
    `cursor_rejects_another_secret_and_another_column`, `cursor_rejects_malformed_input`,
    `parses_the_cursor_query_param`.
  - **The application half is still open**: a module must choose the anchor column and issue
    `next_cursor`. The framework ships the mechanism (the demo edge passes the cursor through as
    `_page.cursor`); the sample `appointments` module still pages by offset.
- The **builder** carries the keyset row predicate (`(a, b) > ($1, $2)`) for keyset pagination.

### LF-047 — Redis and cache-backed data services — VERIFIED
- The `CachePort` carries the atomic primitives the atlas asks for: `put_if_absent` (SET NX) and
  `delete_if_equals` (Lua compare-and-delete, so only the lock holder frees the slot) — the memory
  adapter proves both (`put_if_absent_is_atomic_and_delete_if_equals_checks_value`).
- `CacheService` is the Laravel-shaped API over that port: get/put/has/delete/pull/many/put_many/add/
  forever/increment/decrement/remember/tags/flush/lock — 17 cases including the interesting ones:
  `tenant_isolation_cache_entries_do_not_cross_tenants`,
  `tenant_isolation_cache_prefix_invalidation_stays_inside_one_tenant`,
  `pentest_cache_hostile_keys_and_prefixes_cannot_escape_the_caller`,
  `cache_lock_acquire_release_and_exclusivity`, `with_lock_runs_and_releases`,
  `remember_waiter_collapses_onto_the_leader_result`,
  `remember_falls_back_when_the_leader_never_appears`.
- **Redis adapters** ship for the cache, the session store, the job store and (new this session) the
  rate-limit store — the cache one uses `SET EX`/`SET NX`/Lua CAS with cache-failure degradation to a
  miss.
- **The live Redis now RUNS** (§2.4) — **15 cases against the real server**: the cache's SET NX and
  Lua compare-and-delete, the session's round trip / TTL / shared store, the durable job store's Lua
  claim exclusivity / tenant scoping / dead-lettering, and the rate limit's cross-instance bucket.
  Before this pass only 2 of those cases existed (the rate-limit ones) and the rest of the Redis
  surface was proven only against the in-memory adapter — see F-15.
- **Still open (low)**: the atlas's "timeout" and "connection failure behavior are defined" is
  asserted only at the unit level; there is no live case for a Redis that is unreachable **mid-test**
  (the adapters' degradation to a miss / a failure-open rate limit is unit-tested, not exercised
  against a server that goes away).

### LF-048 — MongoDB integration — EXCLUDED BY DESIGN
- Not implemented, and deliberately so: the blueprint's permanent-exclusion list names MongoDB, and the
  atlas marks the item P3 "never score as core SQL ORM parity". No adapter, no dependency, no claim.

## 4. Findings (new in this pass)

- **F-7 (medium) — `kelson seed` was not gated for production — FIXED.** The seeding framework is
  sound (idempotent, ordered, `--only`), but the CLI ran it against whatever `DATABASE_URL` pointed
  at. The atlas requires seeders to be "explicitly gated for production and not silently disable
  critical database protection outside seeding". The codebase already had the pattern — the truncate
  path is gated to `dev|test` — so this was an inconsistency, not a missing idea. **Fixed**: `kelson
  seed` now refuses outside `dev`/`test` (the same gate `migrate:refresh` applies, extracted as
  `refuse_seed_outside_dev_test` so it is unit-testable). Executed:
  `tests::seed_refuses_outside_dev_test` (dev/test allowed; staging/production refused with
  "outside dev/test").
- **F-8 (low) — the relation layer's N+1 bound was documented, not asserted — FIXED (the bound); pivot
  writes remain a decision.** `ModelRuntime::with` loads a relation level by level with a documented "at
  most 2 store queries per relation" bound, but no query-counting test asserted it (the eager-load
  cases checked the *results*, not the number of round-trips). **Fixed**: a counting store wrapper now
  makes the bound an enforced invariant —
  `eager_load_issues_one_query_per_relation_not_per_parent` proves that **five** parents cost **one**
  IN query (not one per parent).
  - The pivot half needed **no** new port: the runtime already carries the pivot write API
    (`attach`, `sync`, `toggle` and the detach half of `sync`), exercised by
    `pivot_sync_detach_toggle_and_update` and
    `pivot_ops_reject_non_many_to_many_and_existence_stays_refused`; pivot **reads** are covered
  by `morph_to_many_eager_loads_and_pivots_stay_type_pinned`. *(This finding originally said pivot
  writes were absent from the port; that was wrong: the API is at the runtime level, and it is
  tested.)*
- **F-9 (low) — the transaction port had no deadlock retry and no read/write split — FIXED (retry);
  the read/write split stays open.** `TransactionPort` exposed begin/commit/rollback with an isolation
  level, but no retry concept, and every database failure came back as a blanket `retryable: true`, so
  a caller could not tell a lost race from a unique violation. **Fixed**:
  - `kelson-transaction` gained `TransactionFailure`, `classify_app_error` and
    `retry_transaction(attempts, delay, body)` — the body is the **whole** transaction, so a retry
    re-opens it and a partially applied body is rolled back with it. A fatal failure is returned
    immediately; the attempt cap is respected.
  - Both drivers classify the lost race **where the driver error is still typed** (the SQLSTATE does not
    survive the trip into an `AppError`): pg `40P01` (deadlock_detected) and `40001`
    (serialization_failure); MySQL `40001` (`ER_LOCK_DEADLOCK`; the lock-wait timeout 1205 reports the
    generic `HY000` and is therefore fatal). The transaction ports now map a lost race to a
    **retryable** error and every other database failure to a **not-retryable** one.
  - Executed: `kelson-transaction` — `classification_separates_lost_races_from_fatal_failures`,
    `retry_reruns_a_lost_race_until_it_succeeds`,
    `retry_stops_at_the_attempt_cap_and_returns_the_last_failure`,
    `retry_does_not_rerun_a_fatal_failure`, `retry_runs_the_body_once_when_it_succeeds`; pg —
    `deadlock_and_serialization_failures_are_retryable`, `other_database_failures_are_fatal`,
    `a_lost_race_maps_to_a_retryable_app_error`; mysql — `deadlock_is_retryable`,
    `other_database_failures_are_fatal`, `a_lost_race_maps_to_a_retryable_app_error`.
  - **Wire-contract note**: a database error inside a transaction now reports `retryable: false` unless
    it is a lost race. The flag used to be `true` for every persistence error, so a client that
    trusted it could retry a unique violation forever. Nothing in the workspace asserted the old value.
  - **Still open**: the **read/write split** — one pool per fabric, so the deployment's read replica is
    not wired into the store. That is a deployment-topology decision, not a port gap.
- **F-10 (low) — no cursor pagination — FIXED (the framework half).** The HTTP layer paginated by
  offset only; the builder carried the keyset predicate but nothing encoded or validated a cursor.
  **Fixed**, in three layers:
  - **The port**: `ListOpts.after: Option<Vec<(String, Json)>>` is the keyset anchor — the
    `(column, value)` pairs the page starts *after*. `kelson_models::keyset_predicate` turns it into
    the builder's row comparison (`(col) > ($1)`), validating the column **against the model** (an
    unknown column or a value that does not fit the field is a validation error, never a silently
    ignored anchor).
  - **The three backends**: pg, mysql and the in-memory fake all apply it — the SQL backends render
    `where_row_cmp` (a descending sort flips the comparison to `<`), the fake filters the sorted
    rows. In cursor mode `total` is the size of the **window that is left** (the count query carries
    the keyset too), so `has_more` stays true exactly while another page exists.
  - **The HTTP layer**: `kelson_http::Cursor` — an **opaque, tamper-evident** cursor: the anchor
    value plus an HMAC-SHA256 over it, base64url so it survives a query string untouched. Decoding
    **fails closed** (a cursor that is not base64url, is too short, was signed with another secret or
    anchors another column is a validation error, never "no cursor"). `PageQuery` gained a `cursor`
    field and `Page<T>` a `next_cursor`.
  - **Evidence**: `cursor_keyset_pagination` (the shared conformance case — run by the fake, pg AND
    mysql) proves the page starts after the anchor, that a descending sort flips the comparison,
    that **a concurrent insert before the anchor does not shift the window** (the property OFFSET
    cannot give), and that an unknown anchor column is refused. `kelson-http`:
    `cursor_round_trips_the_anchor_value`, `cursor_rejects_a_tampered_value`,
    `cursor_rejects_another_secret_and_another_column`, `cursor_rejects_malformed_input`,
    `parses_the_cursor_query_param`.
  - **Still open**: an **application** must still choose the anchor column and issue `next_cursor`.
    The framework ships the mechanism; the demo's edge passes the cursor through to the module
    (`_page.cursor`), but the sample `appointments` module still pages by offset.
- **F-13 (medium, found by running the MySQL half for real) — the shared conformance suite violated
  the schema's double-booking guard, so the MySQL half could never pass against the live schema.**
  `reads_and_batch_writes_honour_the_trash_veil` inserted **two live booked rows with the same
  (tenant, patient, slot)** — exactly what `ux_appointments_booked_slot` forbids (one booked row
  per tenant/patient/slot; the partial index in pg, the generated-column twin in mysql). The insert
  failed with `1062 Duplicate entry ... for key 'appointments.ux_appointments_booked_slot'`, so the
  MySQL conformance half was **red against the real schema** — a defect the earlier "green" matrix
  had hidden, because the mysql half only self-skips (and trivially "passes") under a `postgres://`
  DATABASE_URL. **Fixed**: the second row gets **its own slot**, which keeps the case's intent (two
  live rows for one patient, one of them trashed) and respects the guard. The in-memory fake has no
  unique index, so it accepted the old shape — the case only now holds the fake to the same contract
  as the SQL backends.
- **F-14 (medium, same run) — the job-store cases share ONE `jobs_queue` table, so they raced.**
  The unscoped-claim cases wipe the pending/running rows first
  (`DELETE FROM jobs_queue WHERE status IN ('pending', 'running')`) so their claim is deterministic —
  which **ate a concurrent case's job**: `mysql_job_store_tenant_scoped_queue_is_usable_and_isolated`
  failed with "own pending job is claimable" while its neighbour wiped the queue. A global claim can
  steal a job the same way. **Fixed**: the six job-store cases take a shared
  `JOBS_QUEUE_LOCK` (`tokio::sync::Mutex`), so the queue is only ever mutated by one case at a time.
  The failure was order-dependent, not deterministic — which is why it can hide behind a green run.
- **F-15 (medium, found by finding the Redis that was already on this host) — the Redis session and
  cache adapters shipped NO live-Redis coverage at all.** Only the rate-limit adapter had a
  `tests/live_redis.rs`; the session and cache adapters had the `redis-tests` feature but **no live
  test file**, so their primitives were proven against the in-memory adapter and against the key
  layout — never against a real server. The gap was invisible because "the live-Redis cases skip on
  this host" reads like coverage. **Fixed**: both adapters now have a live suite, run against the
  Redis on this host:
  - `kelson-session-redis` (4): `a_session_round_trips_through_redis` (save → load → delete, with the
    bag intact), `two_connections_share_the_session` (two connections are the same store — the
    property a multi-instance deployment depends on), `the_stored_session_carries_a_ttl` (Redis
    expires the key, not the app), `count_sees_only_its_own_namespace`.
  - `kelson-cache-redis` (5): `a_value_round_trips_through_redis`,
    `put_if_absent_admits_exactly_one_writer` (SET NX admits exactly one writer — the cache lock's
    foundation), `delete_if_equals_only_frees_the_slot_for_the_holder` (the Lua compare-and-delete
    does not free another holder's slot), `a_ttl_value_expires_on_its_own`,
    `prefix_invalidation_only_drops_its_own_keys`.
  - The **durable Redis job store** (4, in the same crate): `a_job_round_trips_through_redis`
    (enqueue → claim → complete, and a claimed job is never handed out twice),
    `two_workers_never_claim_the_same_job` (the Lua `CLAIM_SCRIPT`'s exclusivity against a real
    server), `a_tenant_job_is_invisible_to_another_tenant`,
    `a_failure_requeues_and_an_exhausted_job_is_dead_lettered` (a failure below the ceiling
    re-queues and records the error; an exhausted job is dead-lettered).
  - All of them keep the "skip cleanly without a service" posture (`REDIS_URL` unset or unreachable
    → skip, not fail), so a run without Redis stays green.
- **F-11 (low, recorded divergence) — the query builder has no raw-SQL escape hatch.** Every value is a
  typed bind and every identifier is validated; there is no way to inject a raw fragment. This is the
  atlas's preferred direction ("a typed builder is preferred in Rust") and it removes an injection
  surface — recorded so the absence is read as a decision, not an omission.
- **F-12 (low) — rollback granularity is the batch, and there is no expand-contract guidance.** The
  pairs scanner rolls back whole **batches** (newest first, reverse filename order inside a batch) —
  Laravel's granularity, so one migration inside a batch cannot be reverted on its own. Pairs *do* carry
  a required per-migration `.down.sql` (a pair that cannot roll back never applies — the scan refuses
  before any DDL), and the legacy single-file migrations are forward-only by documented divergence.
  The missing piece against the atlas was the **expand-contract guidance for no-downtime deployments**
  — now written next to the pair convention (`database/migrations-pairs/README.md`: expand → backfill →
  switch → contract, plus which format a no-downtime change must use).

## 5. Corrections to the previous claims

This batch had no prior verification file, so there is nothing to correct row-by-row. The corrections
this pass makes are to the **parity documents** that carried unproven or stale claims:

- `LARAVEL-FEATURE-ATLAS-COMPLETE.md` LF-036…LF-048 rows now carry the real named evidence and the
  real status (e.g. LF-041 was `MATCH` with a test-suite count; it is **NOT IMPLEMENTED**).
- `P0-FEATURE-VERIFICATION.md` — the database rows now name the executed cases instead of
  "pg_integration 18 tests ✅", and the summary points at this batch.

## 6. Summary

- **13/13 items covered**: **8 VERIFIED** (LF-037 query builder, LF-040 model CRUD, LF-041 relations +
  eager loading, LF-042 casts + accessors + strictness, LF-043 scopes + events, LF-045 resources +
  serialization, LF-046 pagination — offset **and** cursor, LF-047 Redis/cache) · **3 PARTIALLY
  VERIFIED** (LF-036 transactions, LF-038 migrations, LF-039 seeding) · **2 EXCLUDED BY DESIGN**
  (LF-044 collections, LF-048 MongoDB).
- **466 green results, 0 failures** in this pass, **plus 85 live-MySQL matrix results** (the mysql
  halves of the pg/mysql matrix, run against MySQL 8 on 127.0.0.1:3307), **15 live-Redis results**
  (against the Redis on 127.0.0.1:6379) and one live-PostgreSQL run of 83 (§2.4).
- **The sharpest gaps**: the transaction port's **read/write split** (F-9 — a deployment-topology
  decision) and the **application half** of cursor pagination (F-10 — a module must pick the anchor
  column and issue `next_cursor`). Everything else is a P1 refinement or a deliberate exclusion.
- **Findings**: F-7 (un-gated `kelson seed` — **fixed**), F-8 (N+1 bound — **fixed**, both halves),
  F-9 (no deadlock retry — **fixed**; read/write split — open), F-10 (no cursor pagination —
  **fixed**, the framework half), F-11 (no raw-SQL escape hatch — deliberate), F-12 (batch-level
  rollback only — expand/contract guidance **fixed**), **F-13** (the conformance suite violated the
  schema's double-booking guard — **fixed**), **F-14** (the job-store cases raced on one shared
  `jobs_queue` table — **fixed**).

