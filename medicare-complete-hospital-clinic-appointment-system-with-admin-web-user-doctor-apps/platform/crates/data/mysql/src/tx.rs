//! MySQL execution backend: SET TRANSACTION characteristics -> BEGIN ->
//! GET_LOCK (Serialized plans) -> handler -> guards -> DeltaStone + Audit +
//! Outbox writes -> COMMIT -> RELEASE_LOCK (one transaction, D-53).
//!
//! MySQL port notes (divergences from `kelson_postgres::tx`):
//! - No RLS / no `SET LOCAL kelson.tenant_id` / no `set_config` (rule 11): the
//!   pg tenant GUC statement is dropped. Tenant isolation rests on the
//!   explicit `tenant_id` binds of the delta_stones / outbox / audit_records
//!   INSERTs and the explicit `tenant_id = ?` (or `tenant_id IS NULL`)
//!   predicate on both audit-chain lookups. Every pg statement in this file
//!   already carried its tenant anchor, so no predicate had to be ADDED;
//!   `delta_streams` has no tenant column in either driver (its `stream_key`
//!   embeds the tenant), so no predicate applies there either.
//! - Transaction characteristics run as ONE
//!   `SET TRANSACTION ISOLATION LEVEL ..., READ WRITE|READ ONLY` statement
//!   BEFORE `BEGIN`: MySQL rejects `SET TRANSACTION` inside a transaction
//!   (ERROR 1568), and `SET SESSION TRANSACTION` would not affect the current
//!   transaction. `ReadCommitted` (the kernel default) is set explicitly
//!   because MySQL's own default is REPEATABLE READ - the audit hash-chain
//!   lookups depend on read-committed freshness (a stale snapshot would fork
//!   the chain). The access mode is always spelled out so a stale
//!   next-transaction value on a pooled connection can never leak in.
//! - That statement is sent over the simple query path (COM_QUERY) with
//!   `sqlx::raw_sql`, not `sqlx::query`: MySQL's prepared-statement protocol
//!   accepts only a documented list of statement types (error 1295 "not
//!   supported in the prepared statement protocol") and `SET TRANSACTION` is
//!   not on it, while `sqlx::query` prepares even a zero-bind statement. The
//!   pg twin runs it over the extended protocol, which has no such list.
//!   Every other statement here binds at least one parameter and is an
//!   INSERT or SELECT - both preparable - so they keep the normal path.
//! - `raw_sql` is therefore always invoked in RECEIVER form -
//!   `conn.execute(sqlx::raw_sql(stmt))` through `sqlx::Executor`, never
//!   `sqlx::raw_sql(stmt).execute(conn)`: the inherent `RawSql::execute`
//!   folds the query borrow and the executor lifetime into one bound
//!   (`'q: 'e, E: Executor<'e>`), whose region goal fails to generalize when
//!   the future is boxed into the trait `BoxFuture`s (rustc: "implementation
//!   of `Executor` is not general enough"). `Executor::execute` keeps the
//!   lifetimes apart (`'e, 'q: 'e`, `E: Execute<'q, ..>`) and compiles.
//! - The pg `SET LOCAL statement_timeout` budget is dropped: MySQL has no
//!   per-statement/per-transaction timeout (`MAX_EXECUTION_TIME` is
//!   session-scoped and SELECT-only; setting it would leak a kill timer onto
//!   every pooled connection).
//! - `pg_advisory_xact_lock` -> `SELECT GET_LOCK(?, -1)` (session-scoped,
//!   negative timeout = infinite wait, so it blocks like pg) inside the
//!   transaction, released with `SELECT RELEASE_LOCK(?)` AFTER COMMIT on the
//!   same connection: MySQL user locks survive commit/rollback, so the hold
//!   window is a superset of pg's (acquire after BEGIN, release after the
//!   transaction ended), never a subset. The pooled connection is therefore
//!   held outside the transaction object until the release runs. Lock NAMES
//!   are server-wide in MySQL (they span every database on the instance),
//!   whereas pg's advisory key space is per database - the name embeds the
//!   tenant partition, so cross-database collisions cannot mix tenants.
//! - Stream sequence (rule 10): pg's single
//!   `INSERT ... ON CONFLICT DO UPDATE ... RETURNING next_seq` becomes two
//!   statements on this same transaction connection: the upsert
//!   `INSERT ... ON DUPLICATE KEY UPDATE next_seq = delta_streams.next_seq + 1`
//!   takes the row X lock (held to COMMIT, serializing concurrent
//!   incrementers exactly like pg), then `SELECT next_seq ... WHERE
//!   stream_key = ?` reads our own increment (read-your-own-writes holds in
//!   every MySQL isolation level).
//! - `before` is a reserved word in MySQL and is backticked in the
//!   delta_stones column list (`after` and `at` are not reserved).
//! - `now()` becomes `NOW(6)`; `QueryTap` sources are `mysql:*` instead of
//!   `pg:*` (the same five probe points, so query budgets are unchanged).
//! - The audit hash chain hashes byte-identical inputs in the identical
//!   order as pg (same `truncate_to_micros`, `chain_id_for`,
//!   `canonical_audit_row`, `audit_hash`, `GENESIS_HASH`), so both drivers
//!   produce the same chain for the same logical rows.

use kelson_context::ExecutionContext;
use kelson_foundation::{AppError, ConcurrencyStrategy, DeltaId, StreamRef};
use kelson_execution::concurrency::expand_key;
use kelson_execution::plan::ExecutionPlan;
use kelson_execution::unit::{
    guard, AuditRecord, BoxFuture, CommitReceipt, ExecutionBackend, ExecutionUnit, HandlerFn,
    MutationRecord, OutboxMessage, RunOutcome,
};
use kelson_transaction::{advisory_lock_key, IsolationLevel, TxOptions};
use sqlx::mysql::MySqlPool;
use sqlx::Executor;
use time::OffsetDateTime;
use uuid::Uuid;

/// Apply plan-driven transaction options (S1): isolation first, before any
/// statement of the transaction. MySQL divergence: the characteristics must
/// be set BEFORE `BEGIN` (the pg twin sets them after), and the pg statement
/// budget has no equivalent, so it is dropped here (see module docs).
async fn apply_tx_options(
    conn: &mut sqlx::MySqlConnection,
    opts: &TxOptions,
) -> Result<(), AppError> {
    let level = match opts.isolation {
        IsolationLevel::Serializable => "SERIALIZABLE",
        IsolationLevel::RepeatableRead => "REPEATABLE READ",
        // Explicit: MySQL's own default is REPEATABLE READ while the kernel
        // default is READ COMMITTED - omitting it would silently change the
        // read-committed freshness the audit chain depends on.
        IsolationLevel::ReadCommitted => "READ COMMITTED",
    };
    let access = if opts.readonly { "READ ONLY" } else { "READ WRITE" };
    // `raw_sql` binds nothing, so sqlx takes the COM_QUERY (simple) path -
    // `SET TRANSACTION` is not preparable on MySQL (module docs). Receiver
    // form via `sqlx::Executor` - see the module docs note on why
    // `raw_sql(...).execute(conn)` cannot be boxed into a `BoxFuture`.
    let set = format!("SET TRANSACTION ISOLATION LEVEL {level}, {access}");
    conn.execute(sqlx::raw_sql(&set))
        .await
        .map_err(|e| AppError::persistence(format!("isolation: {e}")))?;
    // opts.statement_timeout_ms: dropped (no MySQL equivalent; the
    // session-scoped SELECT-only MAX_EXECUTION_TIME would leak a kill timer
    // onto the pooled connection) - documented divergence.
    Ok(())
}

/// Release the serialized-plan lock taken with `GET_LOCK` (session-scoped on
/// MySQL, unlike pg's transaction-scoped advisory lock). Fail-closed: a false
/// release means this connection does not hold the lock - surfaced rather than
/// swallowed (same contract as the migration lock release in `lib.rs`).
async fn release_advisory_lock(
    conn: &mut sqlx::MySqlConnection,
    name: &str,
) -> Result<(), AppError> {
    let released: Option<i64> = sqlx::query_scalar("SELECT RELEASE_LOCK(?)")
        .bind(name)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| AppError::persistence(format!("advisory lock release: {e}")))?;
    if released == Some(1) {
        Ok(())
    } else {
        Err(AppError::persistence(format!(
            "advisory lock release: RELEASE_LOCK returned {released:?} for {name} (lock not held by this connection)"
        )))
    }
}

struct MySqlUnit<'c> {
    tx: sqlx::Transaction<'c, sqlx::MySql>,
    tenant_id: Option<kelson_foundation::TenantId>,
    execution_id: kelson_foundation::ExecutionId,
    principal_id: Option<kelson_foundation::PrincipalId>,
    correlation_id: kelson_foundation::CorrelationId,
    command: String,
    default_stream: Option<StreamRef>,
    scope_paths: Vec<String>,
    mutations: Vec<MutationRecord>,
    audits: Vec<AuditRecord>,
    outbox: Vec<OutboxMessage>,
}

impl ExecutionUnit for MySqlUnit<'_> {
    fn tenant_id(&self) -> Option<kelson_foundation::TenantId> {
        self.tenant_id
    }
    fn record_mutation(&mut self, mutation: MutationRecord) {
        self.mutations.push(mutation);
    }
    fn record_audit(&mut self, audit: AuditRecord) {
        self.audits.push(audit);
    }
    fn queue_outbox(&mut self, message: OutboxMessage) {
        self.outbox.push(message);
    }
    fn mutation_count(&self) -> usize {
        self.mutations.len()
    }
}

pub struct MySqlBackend {
    pool: MySqlPool,
}

impl MySqlBackend {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

impl ExecutionBackend for MySqlBackend {
    fn run<'a>(
        &'a self,
        ctx: &'a ExecutionContext,
        plan: &'a ExecutionPlan,
        handler: HandlerFn<'a>,
    ) -> BoxFuture<'a, Result<RunOutcome, AppError>> {
        Box::pin(async move {
            let t_begin = std::time::Instant::now();
            // The pooled connection is held OUTSIDE the transaction object:
            // MySQL user locks are session-scoped, so RELEASE_LOCK must run
            // on this same connection AFTER the transaction has ended.
            let mut conn = self
                .pool
                .acquire()
                .await
                .map_err(|e| AppError::persistence(format!("begin: {e}")))?;

            // Plan-driven transaction options (S1): isolation + access mode.
            // MySQL sets them before BEGIN (see apply_tx_options); the pg
            // statement budget has no equivalent and is dropped.
            let opts = TxOptions::from_plan_fields(
                plan.concurrency.strategy == ConcurrencyStrategy::Serialized,
                plan.timeout_secs,
                plan.profile.is_readonly(),
            );
            apply_tx_options(&mut conn, &opts).await?;

            // Set when GET_LOCK was taken on this transaction; released after
            // the transaction ends, on this same connection.
            let mut lock_name: Option<String> = None;

            // The transaction body mirrors the pg twin statement-for-statement
            // (`run_body` below): any early error drops `tx`, which queues the
            // ROLLBACK on `conn` (sqlx contract), and the release below still
            // runs.
            let outcome =
                run_body(&mut conn, plan, ctx, handler, t_begin, &mut lock_name).await;

            // Release the session-scoped lock only AFTER the transaction has
            // ended (COMMIT, or the ROLLBACK sqlx queues when the unit is
            // dropped on an error path), then return the connection.
            let released = match lock_name.as_deref() {
                Some(name) => release_advisory_lock(&mut conn, name).await,
                None => Ok(()),
            };
            drop(conn);

            // The run's outcome wins over a release failure, but the release
            // is always attempted first (same contract as the migration lock
            // in `lib.rs`).
            let outcome = outcome?;
            released?;
            Ok(outcome)
        })
    }
}

/// The transaction body of `ExecutionBackend::run` - the pg twin mirrored
/// statement-for-statement: BEGIN, the serialized-plan advisory lock, unit
/// construction, handler + guard, delta/audit/outbox writes and COMMIT, all
/// on `conn`'s transaction. The caller releases the session-scoped GET_LOCK
/// after this returns, on success AND error (an early error here drops `tx`,
/// which queues the ROLLBACK on `conn` - sqlx contract).
#[allow(clippy::too_many_arguments)]
async fn run_body(
    conn: &mut sqlx::mysql::MySqlConnection,
    plan: &ExecutionPlan,
    ctx: &ExecutionContext,
    handler: HandlerFn<'_>,
    t_begin: std::time::Instant,
    lock_name: &mut Option<String>,
) -> Result<RunOutcome, AppError> {
    let mut tx: sqlx::Transaction<'_, sqlx::MySql> =
        sqlx::Transaction::begin(&mut *conn, None)
            .await
            .map_err(|e| AppError::persistence(format!("begin: {e}")))?;
                kelson_execution::QueryTap::record("mysql:tx.begin", "BEGIN", t_begin.elapsed());

                // RLS anchor: pg sets a session GUC for the whole transaction
                // scope; MySQL has no RLS (rule 11), so there is no statement
                // here - tenant isolation is the explicit tenant_id bind on
                // every delta/audit/outbox write and the explicit tenant_id
                // predicate on every audit-chain read below.

                // Centralized sequencing (Serialized plans): transaction-scoped
                // advisory lock keyed by tenant + plan key template (S1 locks).
                // MySQL has no transaction-scoped advisory lock: GET_LOCK is
                // session-scoped and survives COMMIT/ROLLBACK, so it is
                // released explicitly after the transaction ends (module docs).
                if plan.concurrency.strategy == ConcurrencyStrategy::Serialized
                    && plan.transaction.required
                {
                    let partition = ctx
                        .tenant_id()
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "platform".into());
                    let key = expand_key(
                        plan.concurrency
                            .serialized_key
                            .as_deref()
                            .unwrap_or("{operation}"),
                        ctx.tenant_id(),
                        &plan.operation,
                    );
                    let name =
                        format!("kelson.serialized:{}", advisory_lock_key(&partition, &key));
                    let acquired: Option<i64> = sqlx::query_scalar("SELECT GET_LOCK(?, -1)")
                        .bind(&name)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|e| AppError::persistence(format!("advisory lock: {e}")))?;
                    if acquired != Some(1) {
                        return Err(AppError::persistence(format!(
                            "advisory lock: GET_LOCK did not acquire (returned {acquired:?}); fail-closed"
                        )));
                    }
                    *lock_name = Some(name);
                }

                let scope_paths = ctx
                    .scope
                    .as_ref()
                    .map(|s| s.nodes.iter().map(|n| n.path.clone()).collect())
                    .unwrap_or_default();

                let mut unit = MySqlUnit {
                    tx,
                    tenant_id: ctx.tenant_id(),
                    execution_id: ctx.execution_id,
                    principal_id: Some(ctx.principal.id),
                    correlation_id: ctx.correlation_id,
                    command: plan.operation.to_string(),
                    default_stream: plan.delta.stream.clone(),
                    scope_paths,
                    mutations: Vec::new(),
                    audits: Vec::new(),
                    outbox: Vec::new(),
                };

                let result = handler(&mut unit).await?;
                guard(plan, unit.mutation_count())?;

                // DeltaStones with per-stream monotonic sequences.
                for m in unit.mutations.iter() {
                    let t_delta = std::time::Instant::now();
                    let stream = m
                        .stream
                        .clone()
                        .or_else(|| unit.default_stream.clone())
                        .unwrap_or_else(|| {
                            StreamRef::data(
                                unit.tenant_id
                                    .map(|t| t.to_string())
                                    .unwrap_or_else(|| "platform".into()),
                            )
                        });
                    let key = stream.to_string();

                    // Stream sequence (rule 10): pg does the upsert and
                    // `RETURNING next_seq` in ONE statement; MySQL has no
                    // RETURNING, so it is an upsert + select-back on this same
                    // transaction connection. The upsert holds the row X lock
                    // until COMMIT (serializing concurrent incrementers like
                    // pg's ON CONFLICT), and the select-back reads our own
                    // increment.
                    sqlx::query(
                        "INSERT INTO delta_streams (stream_key, next_seq) VALUES (?, 1)
                         ON DUPLICATE KEY UPDATE next_seq = delta_streams.next_seq + 1",
                    )
                    .bind(&key)
                    .execute(&mut *unit.tx)
                    .await
                    .map_err(|e| AppError::persistence(format!("stream sequence: {e}")))?;
                    let seq: i64 =
                        sqlx::query_scalar("SELECT next_seq FROM delta_streams WHERE stream_key = ?")
                            .bind(&key)
                            .fetch_one(&mut *unit.tx)
                            .await
                            .map_err(|e| AppError::persistence(format!("stream sequence: {e}")))?;

                    sqlx::query(
                        r"INSERT INTO delta_stones
                            (id, stream_key, sequence, resource_type, resource_id, resource_version,
                             execution_id, principal_id, tenant_id, scope_snapshot, ownership_snapshot,
                             `before`, command, after, causation_id, correlation_id, at)
                          VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    )
                    .bind(*DeltaId::new().as_uuid())
                    .bind(&key)
                    .bind(seq)
                    .bind(&m.resource_type)
                    .bind(m.resource_id)
                    .bind(m.resource_version)
                    .bind(*unit.execution_id.as_uuid())
                    .bind(unit.principal_id.map(|p| *p.as_uuid()))
                    .bind(unit.tenant_id.map(|t| *t.as_uuid()))
                    .bind(serde_json::to_value(&unit.scope_paths).unwrap_or_default())
                    .bind(serde_json::json!([]))
                    .bind(m.before.clone().unwrap_or(serde_json::Value::Null))
                    .bind(&unit.command)
                    .bind(&m.after)
                    .bind(None::<Uuid>)
                    .bind(*unit.correlation_id.as_uuid())
                    .bind(OffsetDateTime::now_utc())
                    .execute(&mut *unit.tx)
                    .await
                    .map_err(|e| AppError::persistence(format!("delta stone: {e}")))?;
                    kelson_execution::QueryTap::record(
                        "mysql:delta.write",
                        "INSERT delta_stones",
                        t_delta.elapsed(),
                    );
                }

                let deltas_written = unit.mutations.len();
                let audit_written = unit.audits.len();
                let outbox_queued = unit.outbox.len();

                // Sentinel (S5): audit hash-chaining (D-74). Each row hashes
                // prev_hash || canonical(row) inside this transaction - no side
                // channels. Segments anchor per (tenant, day); legacy rows stay
                // NULL and are skipped by verification. Chain inputs and their
                // order are byte-identical to the pg twin.
                let mut chain_prev: std::collections::HashMap<Uuid, [u8; 32]> =
                    std::collections::HashMap::new();
                for a in unit.audits.iter() {
                    let t_audit = std::time::Instant::now();
                    let at = truncate_to_micros(OffsetDateTime::now_utc());
                    let tenant_uuid = unit.tenant_id.map(|t| *t.as_uuid());
                    let tenant_str = unit.tenant_id.map(|t| t.to_string());
                    let principal_str = unit.principal_id.map(|p| p.to_string());
                    let day = at.date().to_string();
                    let chain_id = kelson_integrity::chain_id_for(tenant_str.as_deref(), &day);

                    let prev_hash: [u8; 32] = match chain_prev.get(&chain_id) {
                        Some(h) => *h,
                        None => {
                            let fetched: Option<Vec<u8>> = match tenant_uuid {
                                Some(tid) => {
                                    sqlx::query_scalar(
                                        "SELECT chain_hash FROM audit_records
                                          WHERE tenant_id = ? AND chain_id = ?
                                            AND chain_hash IS NOT NULL
                                          ORDER BY at DESC, id DESC LIMIT 1",
                                    )
                                    .bind(tid)
                                    .bind(chain_id)
                                    .fetch_optional(&mut *unit.tx)
                                    .await
                                    .map_err(|e| {
                                        AppError::persistence(format!("audit chain: {e}"))
                                    })?
                                }
                                None => {
                                    sqlx::query_scalar(
                                        "SELECT chain_hash FROM audit_records
                                          WHERE tenant_id IS NULL AND chain_id = ?
                                            AND chain_hash IS NOT NULL
                                          ORDER BY at DESC, id DESC LIMIT 1",
                                    )
                                    .bind(chain_id)
                                    .fetch_optional(&mut *unit.tx)
                                    .await
                                    .map_err(|e| {
                                        AppError::persistence(format!("audit chain: {e}"))
                                    })?
                                }
                            };
                            fetched
                                .and_then(|b| b.try_into().ok())
                                .unwrap_or(kelson_integrity::GENESIS_HASH)
                        }
                    };

                    let outcome_str = format!("{:?}", a.outcome);
                    let merged_detail =
                        serde_json::json!({ "detail": a.detail, "outcome": outcome_str });
                    let at_iso = at
                        .format(&time::format_description::well_known::Rfc3339)
                        .map_err(|e| AppError::persistence(format!("audit chain time: {e}")))?;
                    let canonical = kelson_integrity::canonical_audit_row(
                        tenant_str.as_deref(),
                        principal_str.as_deref(),
                        &a.action,
                        &a.resource,
                        &a.authority,
                        &merged_detail,
                        &at_iso,
                    );
                    let chain_hash = kelson_integrity::audit_hash(&prev_hash, &canonical);

                    sqlx::query(
                        r"INSERT INTO audit_records
                            (id, execution_id, tenant_id, principal_id, action, resource, authority, outcome, detail, at, chain_id, prev_hash, chain_hash)
                          VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    )
                    .bind(Uuid::now_v7())
                    .bind(*unit.execution_id.as_uuid())
                    .bind(tenant_uuid)
                    .bind(unit.principal_id.map(|p| *p.as_uuid()))
                    .bind(&a.action)
                    .bind(&a.resource)
                    .bind(&a.authority)
                    .bind(&outcome_str)
                    .bind(&a.detail)
                    .bind(at)
                    .bind(chain_id)
                    .bind(prev_hash.to_vec())
                    .bind(chain_hash.to_vec())
                    .execute(&mut *unit.tx)
                    .await
                    .map_err(|e| AppError::persistence(format!("audit: {e}")))?;
                    chain_prev.insert(chain_id, chain_hash);
                    kelson_execution::QueryTap::record(
                        "mysql:audit.write",
                        "INSERT audit_records",
                        t_audit.elapsed(),
                    );
                }

                for o in unit.outbox.iter() {
                    let t_outbox = std::time::Instant::now();
                    sqlx::query(
                        r"INSERT INTO outbox (id, tenant_id, event, topic, payload, published, created_at)
                          VALUES (?,?,?,?,?,false,NOW(6))",
                    )
                    .bind(Uuid::now_v7())
                    .bind(unit.tenant_id.map(|t| *t.as_uuid()))
                    .bind(&o.event)
                    .bind(&o.topic)
                    .bind(&o.payload)
                    .execute(&mut *unit.tx)
                    .await
                    .map_err(|e| AppError::persistence(format!("outbox: {e}")))?;
                    kelson_execution::QueryTap::record(
                        "mysql:outbox.write",
                        "INSERT outbox",
                        t_outbox.elapsed(),
                    );
                }

                let t_commit = std::time::Instant::now();
                unit.tx
                    .commit()
                    .await
                    .map_err(|e| AppError::persistence(format!("commit: {e}")))?;
                kelson_execution::QueryTap::record("mysql:tx.commit", "COMMIT", t_commit.elapsed());

                Ok(RunOutcome {
                    result,
                    receipt: CommitReceipt {
                        deltas_written,
                        audit_written,
                        outbox_queued,
                        replayed: false,
                    },
                })
}
/// Truncate to microsecond precision so the canonical RFC3339 string survives
/// the `DATETIME(6)` roundtrip unchanged (S5 audit chain contract).
fn truncate_to_micros(at: OffsetDateTime) -> OffsetDateTime {
    let micros = at.unix_timestamp_nanos() / 1_000;
    OffsetDateTime::from_unix_timestamp_nanos(micros * 1_000).unwrap_or(at)
}
