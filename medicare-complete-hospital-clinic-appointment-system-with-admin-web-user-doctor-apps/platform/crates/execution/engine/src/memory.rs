//! In-memory execution backend + idempotency store: tests, examples, dry runs.
//! Assigns monotonic per-stream sequences exactly like the real delta store.

use std::collections::HashMap;
use std::sync::Mutex;

use kelson_foundation::{AppError, CorrelationId, DeltaId, DeltaStone, ExecutionId, PrincipalId, TenantId};
use time::OffsetDateTime;

use crate::plan::ExecutionPlan;
use crate::unit::{
    guard, AuditRecord, BoxFuture, CommitReceipt, ExecutionBackend, ExecutionUnit, HandlerFn,
    MutationRecord, OutboxMessage, RunOutcome,
};
use kelson_context::ExecutionContext;

struct MemoryUnit {
    tenant_id: Option<TenantId>,
    execution_id: ExecutionId,
    principal_id: Option<PrincipalId>,
    correlation_id: CorrelationId,
    command: String,
    default_stream: Option<kelson_foundation::StreamRef>,
    scope_paths: Vec<String>,
    mutations: Vec<MutationRecord>,
    audits: Vec<AuditRecord>,
    outbox: Vec<OutboxMessage>,
}

impl ExecutionUnit for MemoryUnit {
    fn tenant_id(&self) -> Option<TenantId> {
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

#[derive(Default)]
pub struct InMemoryBackend {
    stones: Mutex<Vec<DeltaStone>>,
    sequences: Mutex<HashMap<String, u64>>,
}

impl InMemoryBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// All stones committed so far (test assertions, replay inspection).
    pub fn stones(&self) -> Vec<DeltaStone> {
        self.stones.lock().unwrap().clone()
    }

    pub fn stones_for(&self, partition: &str) -> Vec<DeltaStone> {
        self.stones()
            .into_iter()
            .filter(|s| s.stream.partition == partition)
            .collect()
    }
}

impl ExecutionBackend for InMemoryBackend {
    fn run<'a>(
        &'a self,
        ctx: &'a ExecutionContext,
        plan: &'a ExecutionPlan,
        handler: HandlerFn<'a>,
    ) -> BoxFuture<'a, Result<RunOutcome, AppError>> {
        Box::pin(async move {
            let scope_paths = ctx
                .scope
                .as_ref()
                .map(|s| s.nodes.iter().map(|n| n.path.clone()).collect())
                .unwrap_or_default();
            let mut unit = MemoryUnit {
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

            // S4 diagnostics: record store ops on the query tap using the same
            // shapes the postgres backend emits, so assert_queries and the N+1
            // detector work against the in-memory backend too.
            for _ in &unit.mutations {
                crate::trace::QueryTap::record(
                    "memory:delta.write",
                    "INSERT delta_stones",
                    std::time::Duration::ZERO,
                );
            }
            for _ in &unit.audits {
                crate::trace::QueryTap::record(
                    "memory:audit.write",
                    "INSERT audit_records",
                    std::time::Duration::ZERO,
                );
            }
            for _ in &unit.outbox {
                crate::trace::QueryTap::record(
                    "memory:outbox.write",
                    "INSERT outbox",
                    std::time::Duration::ZERO,
                );
            }
            crate::trace::QueryTap::record("memory:commit", "COMMIT", std::time::Duration::ZERO);

            // "commit": convert mutations to stones with monotonic per-stream sequences.
            let mut seqs = self.sequences.lock().unwrap();
            let mut new_stones = Vec::new();
            for m in unit.mutations.iter() {
                let stream = m
                    .stream
                    .clone()
                    .or_else(|| unit.default_stream.clone())
                    .unwrap_or_else(|| {
                        kelson_foundation::StreamRef::data(
                            unit.tenant_id
                                .map(|t| t.to_string())
                                .unwrap_or_else(|| "platform".into()),
                        )
                    });
                let seq = seqs.entry(stream.to_string()).or_insert(0);
                *seq += 1;
                new_stones.push(DeltaStone {
                    id: DeltaId::new(),
                    stream,
                    sequence: Some(*seq),
                    resource_type: m.resource_type.clone(),
                    resource_id: m.resource_id,
                    resource_version: m.resource_version,
                    execution_id: unit.execution_id,
                    principal_id: unit.principal_id,
                    tenant_id: unit.tenant_id,
                    scope_snapshot: unit.scope_paths.clone(),
                    ownership_snapshot: Vec::new(),
                    before: m.before.clone(),
                    command: unit.command.clone(),
                    after: m.after.clone(),
                    causation_id: None,
                    correlation_id: Some(unit.correlation_id),
                    at: OffsetDateTime::now_utc(),
                });
            }
            drop(seqs);
            let receipt = CommitReceipt {
                deltas_written: new_stones.len(),
                audit_written: unit.audits.len(),
                outbox_queued: unit.outbox.len(),
                replayed: false,
            };
            self.stones.lock().unwrap().extend(new_stones);
            Ok(RunOutcome { result, receipt })
        })
    }
}

// Idempotency store lives in crate::idempotency (S1 extraction).
