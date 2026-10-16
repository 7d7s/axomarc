//! An `ExecutionUnit` a guest can own.
//!
//! The engine's own `MemoryUnit` is deliberately private — it is part of the
//! commit path a guest does not have. What a guest needs is the recording
//! half of the trait: handlers call `record_mutation` / `record_audit` /
//! `queue_outbox` as they work, and something has to hold what they said.

use kelson_context::ExecutionContext;
use kelson_execution::{AuditRecord, ExecutionUnit, MutationRecord, OutboxMessage};
use kelson_foundation::TenantId;

/// Buffers what a handler records for the duration of one call.
///
/// Nothing here commits. A guest has no delta store and no transaction, so
/// the buffered records are the module's *declaration* of what it did, which
/// the host is free to reconcile against the writes it already accepted
/// through `model_write`. Keeping the distinction sharp matters: a unit that
/// pretended to commit would be a guest lying about durability.
#[derive(Default)]
pub struct GuestUnit {
    tenant_id: Option<TenantId>,
    mutations: Vec<MutationRecord>,
    audits: Vec<AuditRecord>,
    outbox: Vec<OutboxMessage>,
}

impl GuestUnit {
    /// A unit carrying the tenant of the execution it serves.
    pub fn for_context(ctx: &ExecutionContext) -> Self {
        Self {
            tenant_id: ctx.tenant.as_ref().map(|tenant| tenant.id),
            ..Self::default()
        }
    }

    /// Mutations the handler recorded, in order.
    pub fn mutations(&self) -> &[MutationRecord] {
        &self.mutations
    }

    /// Audit records the handler recorded, in order.
    pub fn audits(&self) -> &[AuditRecord] {
        &self.audits
    }

    /// Outbox messages the handler queued, in order.
    pub fn outbox(&self) -> &[OutboxMessage] {
        &self.outbox
    }
}

impl ExecutionUnit for GuestUnit {
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
