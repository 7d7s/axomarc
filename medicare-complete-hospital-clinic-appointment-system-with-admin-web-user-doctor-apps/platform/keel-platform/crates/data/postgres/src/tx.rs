//! PostgreSQL execution backend: BEGIN → SET LOCAL kelson.tenant_id → handler →
//! guards → DeltaStone + Audit + Outbox writes → COMMIT (one transaction, D-53).

use kelson_context::ExecutionContext;
use kelson_foundation::{AppError, ConcurrencyStrategy, DeltaId, StreamRef};
use kelson_execution::concurrency::expand_key;
use kelson_execution::plan::ExecutionPlan;
use kelson_execution::unit::{
    guard, AuditRecord, BoxFuture, CommitReceipt, ExecutionBackend, ExecutionUnit, HandlerFn,
    MutationRecord, OutboxMessage, RunOutcome,
};
use kelson_transaction::{advisory_lock_key, IsolationLevel, TxOptions};
use sqlx::postgres::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// Apply plan-driven transaction options (S1): isolation before any statement,
/// then the statement budget from `plan.timeout_secs`.
async fn apply_tx_options(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    opts: &TxOptions,
) -> Result<(), AppError> {
    match opts.isolation {
        IsolationLevel::Serializable => {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                .execute(&mut **tx)
                .await
                .map_err(|e| AppError::persistence(format!("isolation: {e}")))?;
        }
        IsolationLevel::RepeatableRead => {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                .execute(&mut **tx)
                .await
                .map_err(|e| AppError::persistence(format!("isolation: {e}")))?;
        }
        IsolationLevel::ReadCommitted => {} // postgres default
    }
    if opts.readonly {
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut **tx)
            .await
            .map_err(|e| AppError::persistence(format!("readonly: {e}")))?;
    }
    if let Some(ms) = opts.statement_timeout_ms {
        sqlx::query(&format!("SET LOCAL statement_timeout = {ms}"))
            .execute(&mut **tx)
            .await
            .map_err(|e| AppError::persistence(format!("statement_timeout: {e}")))?;
    }
    Ok(())
}

struct PgUnit<'c> {
    tx: sqlx::Transaction<'c, sqlx::Postgres>,
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

impl ExecutionUnit for PgUnit<'_> {
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

pub struct PostgresBackend {
    pool: PgPool,
}

impl PostgresBackend {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl ExecutionBackend for PostgresBackend {
    fn run<'a>(
        &'a self,
        ctx: &'a ExecutionContext,
        plan: &'a ExecutionPlan,
        handler: HandlerFn<'a>,
    ) -> BoxFuture<'a, Result<RunOutcome, AppError>> {
        Box::pin(async move {
            let t_begin = std::time::Instant::now();
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|e| AppError::persistence(format!("begin: {e}")))?;
            kelson_execution::QueryTap::record("pg:tx.begin", "BEGIN", t_begin.elapsed());

            // Plan-driven transaction options (S1): isolation + statement budget.
            let opts = TxOptions::from_plan_fields(
                plan.concurrency.strategy == ConcurrencyStrategy::Serialized,
                plan.timeout_secs,
                plan.profile.is_readonly(),
            );
            apply_tx_options(&mut tx, &opts).await?;

            // RLS anchor: tenant GUC for the whole transaction scope.
            if let Some(tenant) = ctx.tenant_id() {
                sqlx::query("SELECT set_config('kelson.tenant_id', $1, true)")
                    .bind(tenant.to_string())
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| AppError::persistence(format!("set tenant guc: {e}")))?;
            }

            // Centralized sequencing (Serialized plans): transaction-scoped
            // advisory lock keyed by tenant + plan key template (S1 locks).
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
                sqlx::query("SELECT pg_advisory_xact_lock($1)")
                    .bind(advisory_lock_key(&partition, &key))
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| AppError::persistence(format!("advisory lock: {e}")))?;
            }

            let scope_paths = ctx
                .scope
                .as_ref()
                .map(|s| s.nodes.iter().map(|n| n.path.clone()).collect())
                .unwrap_or_default();

            let mut unit = PgUnit {
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
                let seq: i64 = sqlx::query_scalar(
                    "INSERT INTO delta_streams (stream_key, next_seq) VALUES ($1, 1)
                     ON CONFLICT (stream_key) DO UPDATE SET next_seq = delta_streams.next_seq + 1
                     RETURNING next_seq",
                )
                .bind(&key)
                .fetch_one(&mut *unit.tx)
                .await
                .map_err(|e| AppError::persistence(format!("stream sequence: {e}")))?;

                sqlx::query(
                    r"INSERT INTO delta_stones
                        (id, stream_key, sequence, resource_type, resource_id, resource_version,
                         execution_id, principal_id, tenant_id, scope_snapshot, ownership_snapshot,
                         before, command, after, causation_id, correlation_id, at)
                      VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)",
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
                    "pg:delta.write",
                    "INSERT delta_stones",
                    t_delta.elapsed(),
                );
            }

            let deltas_written = unit.mutations.len();
            let audit_written = unit.audits.len();
            let outbox_queued = unit.outbox.len();

            // Sentinel (S5): audit hash-chaining (D-74). Each row hashes
            // prev_hash || canonical(row) inside this transaction — no side
            // channels. Segments anchor per (tenant, day); legacy rows stay
            // NULL and are skipped by verification.
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
                                      WHERE tenant_id = $1 AND chain_id = $2
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
                                      WHERE tenant_id IS NULL AND chain_id = $1
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
                      VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
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
                    "pg:audit.write",
                    "INSERT audit_records",
                    t_audit.elapsed(),
                );
            }

            for o in unit.outbox.iter() {
                let t_outbox = std::time::Instant::now();
                // §5.2: key computed at APPEND time inside the commit so
                // the row and its key are atomic (KEEL-006).
                let key = kelson_event::idempotency_key(&o.event, unit.tenant_id, &o.payload);
                sqlx::query(
                    r"INSERT INTO outbox (id, tenant_id, event, topic, payload, idempotency_key, published, created_at)
                      VALUES ($1,$2,$3,$4,$5,$6,false,now())",
                )
                .bind(Uuid::now_v7())
                .bind(unit.tenant_id.map(|t| *t.as_uuid()))
                .bind(&o.event)
                .bind(&o.topic)
                .bind(&o.payload)
                .bind(key.0)
                .execute(&mut *unit.tx)
                .await
                .map_err(|e| AppError::persistence(format!("outbox: {e}")))?;
                kelson_execution::QueryTap::record(
                    "pg:outbox.write",
                    "INSERT outbox",
                    t_outbox.elapsed(),
                );
            }

            let t_commit = std::time::Instant::now();
            unit.tx
                .commit()
                .await
                .map_err(|e| AppError::persistence(format!("commit: {e}")))?;
            kelson_execution::QueryTap::record("pg:tx.commit", "COMMIT", t_commit.elapsed());

            Ok(RunOutcome {
                result,
                receipt: CommitReceipt {
                    deltas_written,
                    audit_written,
                    outbox_queued,
                    replayed: false,
                },
            })
        })
    }
}
/// Truncate to microsecond precision so the canonical RFC3339 string survives
/// the Postgres `timestamptz` roundtrip unchanged (S5 audit chain contract).
fn truncate_to_micros(at: OffsetDateTime) -> OffsetDateTime {
    let micros = at.unix_timestamp_nanos() / 1_000;
    OffsetDateTime::from_unix_timestamp_nanos(micros * 1_000).unwrap_or(at)
}