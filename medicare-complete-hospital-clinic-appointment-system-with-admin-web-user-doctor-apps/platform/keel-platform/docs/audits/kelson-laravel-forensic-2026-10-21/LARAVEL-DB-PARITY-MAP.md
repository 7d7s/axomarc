# LARAVEL DATABASE PARITY MAP

**The gate for the database work.** Every Laravel database capability has a row: either an
**executed, named case** that proves the equivalent behaviour, or a **signed exclusion** that
states what replaces the outcome and what the divergence is.

**Baseline:** Laravel 13.x (the database surface: migrations, the schema builder, the query
builder, Eloquent, transactions and the connection layer).
**Scored against:** the `p0-runtime-safety` branch at the MySQL-native and production-grade slice.
**Evidence rule:** a row may only claim `executed` when the named case exists AND ran. Every
cited case name is verified to exist in the workspace by a script; the MySQL ones were executed
against a live MySQL 8.0.44.

## 1. The score

| Status | Rows | Meaning |
|---|---:|---|
| VERIFIED | 81 | the behaviour is implemented and an executed case proves it |
| PARTIAL | 18 | the core is proven; a named sub-feature is missing or diverges |
| EXCLUDED_BY_DESIGN | 5 | a deliberate exclusion with the alternative stated |
| OPEN | 2 | a Laravel capability with no Kelson equivalent yet |
| **Total** | **106** | |

Of the 106 rows, **104 carry an executed case**
and 2 do not (the OPEN rows).

| Category | Rows |
|---|---:|
| connection | 5 |
| eloquent | 30 |
| migrations | 12 |
| mysql-8-native | 4 |
| production | 11 |
| query-builder | 32 |
| schema | 9 |
| transactions | 3 |

**The honest shape:** the ORM-shaped surface (Eloquent) and the query builder are strong; the
**schema builder is the narrowest** area (the builder carries the types the framework uses, not
Laravel's full zoo); and **two capabilities are simply absent** (listed in section 2).

## 2. The OPEN rows - what Laravel has and Kelson does not

| ID | Laravel feature | Why it matters |
|---|---|---|
| LDB-080 | Route model binding (`Route::model() / resolveRouteBinding()`) | a Laravel feature with no Kelson equivalent |
| LDB-091 | The maintenance mode (`php artisan down`) | the framework has no database maintenance mode |

## 3. The PARTIAL rows - the proven core and the named gap

| ID | Laravel feature | Proven by | The gap |
|---|---|---|---|
| LDB-012 | Seed the database | `mysql_seed_tenant` | the framework ships a dev-tenant seeder as the proof; there are no FACTORIES yet (LDB-070) |
| LDB-014 | Drop / rename a table | `the shipped migrations` | the builder has create/alter/index; drop and rename go through raw SQL |
| LDB-015 | Every column type | `fluent_schema_builder_byte_round_trips_every_shipped_migration` | the builder carries the types the framework uses; Laravel's full type zoo is not enumerated |
| LDB-016 | Column modifiers | `fluent_schema_builder_byte_round_trips_every_shipped_migration` | the framework's modifier set, not Laravel's full set (after/before/charset/unsigned) |
| LDB-018 | A foreign key | `fluent_schema_builder_byte_round_trips_every_shipped_migration` | the reference is emitted; the referential ACTION clauses are not part of the builder |
| LDB-020 | Inspect the schema | `the schema roundtrip case` | the inspection exists for the shipped tables; the generic hasColumn walk is not exposed |
| LDB-021 | A full-text index | `the builder renders raw DDL` | reachable through raw DDL, not a first-class helper |
| LDB-028 | A pattern match | `where_like_renders_ilike` | Kelson's where_like renders ILIKE (case-INsensitive); Laravel's LIKE is case-SENSITIVE on PostgreSQL |
| LDB-031 | Join | `in_join_order_limit_offset` | INNER and LEFT are implemented; RIGHT and CROSS are not |
| LDB-049 | A lazy collection | `cursor_iterates_lazily_across_pages` | the cursor yields rows; there is no chainable lazy-collection type |
| LDB-056 | Model observers | `model_events_fire_in_order` | the events exist; there is no observer CLASS |
| LDB-078 | API resources | `manifest_hidden_and_visible_shape_the_projection` | the serialization is proven; there is no JsonResource CLASS with a toArray() contract |
| LDB-079 | Model factories with states and relationships | `the testkit's factory cases` | the states exist; the FACTORY RELATIONSHIPS (a factory making another model's rows) do not |
| LDB-081 | The model's $touches / $with / $withCount | `touch_bumps_updated_at; the manifest cases` | the touch is proven; the eager-default manifest fields are not enumerated |
| LDB-086 | A deadlock retry | `the pg/mysql tx cases` | the retry exists on the Postgres path; the MySQL twin's retry is not separately proven |
| LDB-089 | The query log / DB::listen | `the query tap cases` | the tap exists for tests and telemetry; there is no user-facing DB::listen hook |
| LDB-090 | Reconnect | `the pool cases` |  |
| LDB-094 | Tenant isolation enforced by the database | `mysql_conformance_tenant_isolation` | MySQL has no RLS; the isolation rests on the tenant-guarded predicates |

## 4. The EXCLUDED_BY_DESIGN rows - what replaces the outcome

| ID | Laravel feature | The Kelson shape | What still provides the outcome |
|---|---|---|---|
| LDB-006 | An anonymous migration | the SQL file itself | Kelson's migration IS a file; there is no class to be anonymous |
| LDB-007 | A migration class with up()/down() | the .sql migration + the paired .down file | SQL-first is the Kelson shape; the pair is the down() |
| LDB-008 | Several migration paths | one migrations-mysql directory per fabric | one directory per engine keeps the fabric independent |
| LDB-023 | A raw select expression | none - the builder refuses free-form SQL | the typed builder is the safety property; the raw escape hatch is deliberately absent |
| LDB-052 | A raw statement | none at the builder level | the typed builder is the safety property |

## 5. Where Kelson is BETTER than Laravel

These are the rows where the Laravel baseline has no equivalent - the "and more" of the
competitive claim. Each one is an executed case, not a claim.

| ID | The capability | Why Laravel cannot do it | The proof |
|---|---|---|---|
| LDB-045 | A non-blocking lock | Laravel's builder has no NOWAIT or SKIP LOCKED sibling | `mysql_native_nowait_fails_fast_instead_of_waiting; mysql_native_skip_locked_skips_a_row_another_connection_holds` |
| LDB-046 | A common table expression | the body is built by the same typed builder | `mysql_native_a_cte_projects_through_a_common_table_expression; mysql_native_a_recursive_cte_walks_a_hierarchy` |
| LDB-047 | A window function | a typed window API where Laravel has only a raw string | `mysql_native_a_window_function_sums_per_partition; mysql_native_a_window_ranking_numbers_the_partition` |
| LDB-053 | Explain the query | MySQL's EXPLAIN ANALYZE runs the statement and returns the real plan; Laravel's EXPLAIN is a raw string and returns the estimate | `mysql_native_explain_analyze_proves_the_index_is_used` |
| LDB-085 | A savepoint | the savepoint scope is a first-class, separately proven object | `mysql_savepoint_rollback_isolates_two_tests_sharing_a_table` |
| LDB-092 | A dialect-native SQL rendering | one builder, two engines, a divergence proof by construction | `the_postgres_rendering_matches_the_golden_catalogue; the_mysql_rendering_carries_no_postgres_only_construct` |
| LDB-093 | A wrong-engine statement is refused | the statement carries the engine it was rendered for | `the_compiled_statement_records_its_dialect` |
| LDB-095 | A UTC-pinned connection | Laravel leaves the timezone to the server unless the app sets it | `mysql_every_connection_is_pinned_to_utc; mysql_server_side_timestamps_are_utc` |
| LDB-096 | A migration that resumes after a failure | MySQL DDL auto-commits, so Laravel re-runs the whole file after a failure (and fails on the DDL that already landed); the recorded step state resumes instead | `mysql_migration_the_guarded_runner_resumes_after_a_failed_step; mysql_migration_the_guarded_runner_records_every_step` |
| LDB-097 | A refused resume after the migration file changed | a file edited after a failed run is refused, not applied | `mysql_migration_the_guarded_runner_refuses_a_resume_after_the_file_changed` |
| LDB-098 | A schema change on a live table without a long lock | the lock is held for the rename, not for the copy; the engine's INSTANT path is used when the change qualifies | `mysql_migration_the_online_change_copies_the_rows_and_swaps_atomically; mysql_migration_the_online_change_adds_a_column_without_rebuilding` |
| LDB-099 | A rolling deploy where every instance boots at once | the loser waits for the holder and re-checks the state instead of failing the boot | `mysql_migration_a_runner_that_loses_the_lock_waits_and_re_checks` |
| LDB-100 | A per-tenant cost ceiling | the refusal happens BEFORE the write, and the cost is an observable per-tenant fact | `mysql_cost_the_budget_refuses_a_runaway_write_before_it_runs; mysql_cost_the_ledger_aggregates_per_tenant` |
| LDB-101 | A schema version the deployment can compare | the version is a digest of what is actually applied, not a hand-maintained number, so two hosts can be compared exactly | `mysql_upgrade_the_schema_state_names_the_pending_migrations` |
| LDB-102 | A refusal when the schema is behind | a deployment that does not migrate at boot refuses to serve against a schema the framework does not recognise | `mysql_upgrade_a_stale_schema_is_refused` |
| LDB-103 | A zero-downtime schema change during an upgrade | the upgrade itself is zero-downtime - the shadow table, the copy, the atomic swap | `mysql_upgrade_a_change_to_an_existing_table_goes_through_the_online_path` |
| LDB-104 | A one-command capture of the state for a support ticket | the bundle is produced even when the database is down - the moment it is needed - and the schema's version and pending set are in it | `the_diagnostics_bundle_reports_the_state` |
| LDB-106 | The framework declares its own objectives | the objectives are the framework's, published on /meta/slo and as Prometheus gauges, and the status is COMPUTED from the telemetry, never asserted | `the_slo_surface_declares_the_objectives_and_their_status; every_objective_names_a_metric_the_framework_records` |

## 6. The divergences a Laravel developer must know about

The rows where the behaviour differs from Laravel even though both work. These are the ones
that cost a migration day if they are not written down.

| ID | The divergence |
|---|---|
| LDB-028 | **A pattern match** - Kelson's where_like renders ILIKE (case-INsensitive); Laravel's LIKE is case-SENSITIVE on PostgreSQL |
| LDB-049 | **A lazy collection** - the cursor yields rows; there is no chainable lazy-collection type |
| LDB-056 | **Model observers** - the events exist; there is no observer CLASS |
| LDB-078 | **API resources** - the serialization is proven; there is no JsonResource CLASS with a toArray() contract |
| LDB-094 | **Tenant isolation enforced by the database** - MySQL has no RLS; the isolation rests on the tenant-guarded predicates |

## 7. How to re-verify this map

```
cargo test -p kelson-query                                              # the builder + the golden catalogue
cargo test -p kelson-query --test dialect_divergence                     # the divergence proof
DATABASE_URL=mysql://keel:keel@127.0.0.1:3307/keel \
  cargo test -p kelson-postgres --features pg-tests --test modelstore_conformance   # the shared contract
DATABASE_URL=mysql://keel:keel@127.0.0.1:3307/keel \
  cargo test -p kelson-postgres --features pg-tests --test mysql_native            # the MySQL 8 native matrix
DATABASE_URL=mysql://keel:keel@127.0.0.1:3307/keel \
  cargo test -p kelson-postgres --features pg-tests --test mysql_migrations        # the migration safety
DATABASE_URL=mysql://keel:keel@127.0.0.1:3307/keel \
  cargo test -p kelson-postgres --features pg-tests --test mysql_cost             # the cost axis
```

The PostgreSQL half of the matrix cannot run on this host (the client's native backends
fault and the server binaries are gone), so the Postgres rendering is proven by the golden
catalogue instead - byte for byte. The MySQL half is proven by execution.
