//! The execution engine: plan validation → plane check → deadline → authorization →
//! idempotency → backend.run (begin → handle → guards → delta/audit/outbox → commit)
//! → store idempotent result.

use std::sync::Arc;

use kelson_foundation::{
    codes, AppError, ConcurrencyStrategy, ErrorCategory, Retryability, Severity,
};
use serde_json::Value;
use time::OffsetDateTime;

use crate::concurrency::{expand_key, ConcurrencyPort, InMemoryConcurrency};
use crate::idempotency::{GateOutcome, IdempotencyGate, IdempotencyKey, IdempotencyPort};
use crate::plan::ExecutionPlan;
use crate::ports::{AuthorizationPort, AuthzRequest};
use crate::unit::{ExecutionBackend, RunOutcome};
use kelson_context::ExecutionContext;

/// A business operation. One handler = one operation = one stable contract.
/// The same handler serves REST, CLI, jobs, webhooks, workflows, AI tools.
pub trait OperationHandler: Send + Sync {
    fn plan(&self) -> &ExecutionPlan;
    fn handle(
        &self,
        ctx: Arc<ExecutionContext>,
        input: Value,
        unit: &mut dyn crate::unit::ExecutionUnit,
    ) -> crate::unit::BoxFuture<'static, Result<Value, AppError>>;
}

#[derive(Debug, Clone)]
pub struct OperationOutcome {
    pub result: Value,
    pub receipt: crate::unit::CommitReceipt,
}

pub struct Engine {
    authz: Arc<dyn AuthorizationPort>,
    backend: Arc<dyn ExecutionBackend>,
    idem: IdempotencyGate,
    concurrency: Arc<dyn ConcurrencyPort>,
    tracer: Option<Arc<dyn crate::trace::TraceStore>>,
}

impl Engine {
    pub fn new(
        authz: Arc<dyn AuthorizationPort>,
        backend: Arc<dyn ExecutionBackend>,
        idem: Arc<dyn IdempotencyPort>,
    ) -> Self {
        Self {
            authz,
            backend,
            idem: IdempotencyGate::new(idem),
            concurrency: Arc::new(InMemoryConcurrency::new()),
            tracer: None,
        }
    }

    /// Attach an execution-trace recorder (S4 diagnostics: inspect/replay/N+1).
    /// Enabling it captures the execution input into traces — dev/test only;
    /// traces stay under storage/framework/traces and never leave the env.
    pub fn with_trace_recorder(mut self, store: Arc<dyn crate::trace::TraceStore>) -> Self {
        self.tracer = Some(store);
        self
    }

    /// Override the concurrency port (e.g. durable locks from the data layer).
    pub fn with_concurrency(mut self, port: Arc<dyn ConcurrencyPort>) -> Self {
        self.concurrency = port;
        self
    }

    pub async fn execute(
        &self,
        ctx: Arc<ExecutionContext>,
        handler: &dyn OperationHandler,
        input: Value,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Result<OperationOutcome, AppError> {
        let plan = handler.plan();
        let Some(tracer) = self.tracer.clone() else {
            return self
                .run_pipeline(ctx, plan, handler, input, idempotency_key)
                .await;
        };

        // Diagnostics path: capture tap boundary, run, then persist the trace.
        let mark = crate::trace::QueryTap::mark();
        let started_at = OffsetDateTime::now_utc();
        let t0 = std::time::Instant::now();
        let input_capture = input.clone();
        let outcome = self
            .run_pipeline(ctx.clone(), plan, handler, input, idempotency_key)
            .await;
        let duration_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let (authz, idempotent_replay, receipt, result_bytes, error) = match &outcome {
            Ok(o) => (
                "allowed".to_string(),
                o.receipt.replayed,
                Some(o.receipt),
                o.result.to_string().len(),
                None,
            ),
            Err(e) => (
                if e.code == codes::DENIED {
                    "denied".to_string()
                } else {
                    "allowed".to_string()
                },
                false,
                None,
                0,
                Some(e.public_message.clone()),
            ),
        };
        let trace = crate::trace::ExecutionTrace {
            execution_id: ctx.execution_id,
            operation: plan.operation.to_string(),
            profile: format!("{:?}", plan.profile),
            principal: ctx.principal.id,
            principal_display: ctx.principal.display.clone(),
            tenant: ctx.tenant.as_ref().map(|t| t.id),
            tenant_slug: ctx.tenant.as_ref().map(|t| t.slug.clone()),
            plane: ctx.plane.to_string(),
            scope_paths: ctx
                .scope
                .as_ref()
                .map(|s| s.nodes.iter().map(|n| n.path.clone()).collect())
                .unwrap_or_default(),
            capability: plan.authorization.capability.clone(),
            authz,
            idempotent_replay,
            queries: crate::trace::QueryTap::take_since(mark),
            deltas_written: receipt.map(|r| r.deltas_written).unwrap_or(0),
            audit_written: receipt.map(|r| r.audit_written).unwrap_or(0),
            outbox_queued: receipt.map(|r| r.outbox_queued).unwrap_or(0),
            result_bytes,
            error,
            input: input_capture,
            started_at,
            duration_ms,
        };
        // Diagnostics must never break the execution they observe.
        let _ = tracer.save(&trace);
        outcome
    }

    async fn run_pipeline(
        &self,
        ctx: Arc<ExecutionContext>,
        plan: &ExecutionPlan,
        handler: &dyn OperationHandler,
        input: Value,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Result<OperationOutcome, AppError> {
        plan.validate()?;

        // Plane invariant: an operation runs only on its declared plane (P1/P2/P3 anchor).
        if plan.plane != ctx.plane {
            return Err(AppError::denied(format!(
                "operation {} is bound to the {} plane; context is on the {} plane",
                plan.operation, plan.plane, ctx.plane
            )));
        }

        if ctx.deadline.expired(OffsetDateTime::now_utc()) {
            return Err(AppError::new(
                codes::DEADLINE,
                ErrorCategory::Internal,
                Severity::Medium,
                Retryability::Retryable,
                "execution deadline expired before start",
            ));
        }

        // Authorization boundary.
        if plan.authorization.required {
            let decision = self.authz.authorize(
                &ctx,
                &AuthzRequest {
                    capability: plan.authorization.capability.as_deref(),
                },
            );
            if !decision.allowed {
                let reason = decision
                    .reason
                    .unwrap_or_else(|| format!("operation {} denied", plan.operation));
                return Err(AppError::denied(reason).with_meta("via", decision.via));
            }
        }

        // Concurrency boundary (S1): Serialized ops take centralized exclusive
        // sequencing before any work; the lease is held until the execution
        // completes. Optimistic/Pessimistic strategies are enforced by the data
        // layer (CAS / FOR UPDATE — see kelson-transaction locks).
        let _serialized_lease = if plan.concurrency.strategy == ConcurrencyStrategy::Serialized
            && plan.transaction.required
        {
            let key = expand_key(
                plan.concurrency
                    .serialized_key
                    .as_deref()
                    .unwrap_or("{operation}"),
                ctx.tenant_id(),
                &plan.operation,
            );
            Some(self.concurrency.acquire(&key)?)
        } else {
            None
        };

        // Idempotency gate (execution-level, D-51): one gate before execution,
        // one store after commit.
        let gate = self
            .idem
            .gate(&ctx, plan, idempotency_key.as_ref(), &input)
            .await?;
        let hash = match gate {
            GateOutcome::Replay(result) => {
                return Ok(OperationOutcome {
                    result,
                    receipt: crate::unit::CommitReceipt::replayed(),
                });
            }
            GateOutcome::Proceed { hash } => hash,
        };

        // Run through the backend (single ownership: begin → handle → guards → commit).
        let ctx_for_handler = ctx.clone();
        let outcome: RunOutcome = self
            .backend
            .run(
                &ctx,
                plan,
                Box::new(move |unit| handler.handle(ctx_for_handler, input, unit)),
            )
            .await
            .map_err(|mut e| {
                e = e.with_correlation(ctx.correlation_id);
                e
            })?;

        if let Some(key) = &idempotency_key {
            self.idem
                .record(
                    ctx.tenant_id(),
                    &plan.operation,
                    key,
                    &hash,
                    &outcome.result,
                )
                .await?;
        }
        Ok(OperationOutcome {
            result: outcome.result,
            receipt: outcome.receipt,
        })
    }
}
