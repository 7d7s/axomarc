//! # kelson-execution
//! Execution kernel (TCB): `ExecutionPlan`, the engine pipeline, backend ports and
//! the in-memory reference backend. The postgres backend lives in `kelson-postgres`.

pub mod concurrency;
pub mod engine;
pub mod handlers;
pub mod idempotency;
pub mod memory;
pub mod plan;
pub mod plan_compiler;
pub mod ports;
pub mod trace;
pub mod unit;

pub use concurrency::{expand_key, ConcurrencyLease, ConcurrencyPort, InMemoryConcurrency};
pub use engine::{Engine, OperationHandler, OperationOutcome};
pub use handlers::HandlerRegistry;
pub use idempotency::{
    payload_hash, GateOutcome, IdempotencyCheck, IdempotencyGate, IdempotencyKey, IdempotencyPort,
    InMemoryIdempotency,
};
pub use memory::InMemoryBackend;
pub use plan::{
    AuditPlan, AuthorizationPlan, ConcurrencyPlan, ConsistencyModel, ConsistencyPlan, DeltaPlan,
    ExecutionPlan, IdempotencyPlan, TransactionPlan,
};
pub use plan_compiler::{PlanCompiler, PlanIntent};
pub use ports::{AuthorizationPort, AuthzDecision, AuthzRequest};
pub use trace::{
    detect_n_plus_one, render_trace, ExecutionTrace, FileTraceStore, InMemoryTraceStore,
    NPlusOneFinding, QueryProbe, QueryTap, TraceStore,
};
pub use unit::{
    guard, AuditOutcome, AuditRecord, BoxFuture, CommitReceipt, ExecutionBackend, ExecutionUnit,
    HandlerFn, MutationRecord, OutboxMessage, RunOutcome,
};

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_context::{ExecutionContext, PrincipalContext, ScopeSnapshot, TenantContext};
    use kelson_foundation::AuthorizationRevision;
    use kelson_foundation::{ExecutionProfileKind, OperationId, SecurityPlane};
    use serde_json::json;
    use std::sync::Arc;

    struct AllowAll;
    impl AuthorizationPort for AllowAll {
        fn authorize(&self, _ctx: &ExecutionContext, _request: &AuthzRequest) -> AuthzDecision {
            AuthzDecision::allow("test.allow-all")
        }
    }

    struct DenyAll;
    impl AuthorizationPort for DenyAll {
        fn authorize(&self, _ctx: &ExecutionContext, _request: &AuthzRequest) -> AuthzDecision {
            AuthzDecision::deny("test.deny-all", "always denied")
        }
    }

    struct CreateNote {
        plan: ExecutionPlan,
    }

    impl CreateNote {
        fn new() -> Self {
            Self {
                plan: ExecutionPlan::new(
                    OperationId::from_static("demo.create_note"),
                    ExecutionProfileKind::Command,
                ),
            }
        }
    }

    impl OperationHandler for CreateNote {
        fn plan(&self) -> &ExecutionPlan {
            &self.plan
        }
        fn handle(
            &self,
            _ctx: Arc<ExecutionContext>,
            input: serde_json::Value,
            unit: &mut dyn ExecutionUnit,
        ) -> BoxFuture<'static, Result<serde_json::Value, kelson_foundation::AppError>> {
            unit.record_mutation(MutationRecord {
                resource_type: "note".into(),
                resource_id: Some(uuid::Uuid::now_v7()),
                resource_version: 1,
                before: None,
                after: input,
                stream: None,
            });
            Box::pin(async move { Ok(json!({"ok": true})) })
        }
    }

    struct NoopMutation {
        plan: ExecutionPlan,
    }
    impl OperationHandler for NoopMutation {
        fn plan(&self) -> &ExecutionPlan {
            &self.plan
        }
        fn handle(
            &self,
            _ctx: Arc<ExecutionContext>,
            _input: serde_json::Value,
            _unit: &mut dyn ExecutionUnit,
        ) -> BoxFuture<'static, Result<serde_json::Value, kelson_foundation::AppError>> {
            Box::pin(async move { Ok(json!(null)) })
        }
    }

    fn ctx() -> Arc<ExecutionContext> {
        ExecutionContext::builder(
            ExecutionProfileKind::Command,
            time::OffsetDateTime::now_utc(),
        )
        .principal(PrincipalContext {
            id: kelson_foundation::PrincipalId::new(),
            display: None,
        })
        .tenant(TenantContext {
            id: kelson_foundation::TenantId::new(),
            slug: "acme".into(),
            plan: "standard".into(),
        })
        .scope(ScopeSnapshot {
            nodes: vec![],
            revision: AuthorizationRevision(1),
            computed_at: time::OffsetDateTime::now_utc(),
        })
        .build()
        .unwrap()
    }

    fn engine(authz: Arc<dyn AuthorizationPort>) -> (Engine, Arc<InMemoryBackend>) {
        let backend = Arc::new(InMemoryBackend::new());
        let eng = Engine::new(authz, backend.clone(), Arc::new(InMemoryIdempotency::new()));
        (eng, backend)
    }

    #[tokio::test]
    async fn command_writes_delta_stone() {
        let (eng, backend) = engine(Arc::new(AllowAll));
        let outcome = eng
            .execute(
                ctx(),
                &CreateNote::new(),
                json!({"body": "hi"}),
                Some(IdempotencyKey("k1".into())),
            )
            .await
            .unwrap();
        assert_eq!(outcome.receipt.deltas_written, 1);
        let stones = backend.stones();
        assert_eq!(stones.len(), 1);
        assert_eq!(stones[0].sequence, Some(1));
        assert_eq!(stones[0].resource_type, "note");
    }

    #[tokio::test]
    async fn idempotent_replay_returns_same_result_without_new_stone() {
        let (eng, backend) = engine(Arc::new(AllowAll));
        let h = CreateNote::new();
        let same_ctx = ctx();
        let r1 = eng
            .execute(
                same_ctx.clone(),
                &h,
                json!({"body": "hi"}),
                Some(IdempotencyKey("k1".into())),
            )
            .await
            .unwrap();
        let r2 = eng
            .execute(
                same_ctx,
                &h,
                json!({"body": "hi"}),
                Some(IdempotencyKey("k1".into())),
            )
            .await
            .unwrap();
        assert!(r2.receipt.replayed);
        assert_eq!(r1.result, r2.result);
        assert_eq!(backend.stones().len(), 1);
    }

    #[tokio::test]
    async fn same_key_different_payload_conflicts() {
        let (eng, _b) = engine(Arc::new(AllowAll));
        let h = CreateNote::new();
        let same_ctx = ctx();
        eng.execute(
            same_ctx.clone(),
            &h,
            json!({"a": 1}),
            Some(IdempotencyKey("k".into())),
        )
        .await
        .unwrap();
        let err = eng
            .execute(
                same_ctx,
                &h,
                json!({"a": 2}),
                Some(IdempotencyKey("k".into())),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::IDEMPOTENCY_CONFLICT);
    }

    #[tokio::test]
    async fn denied_authorization_short_circuits() {
        let (eng, backend) = engine(Arc::new(DenyAll));
        let err = eng
            .execute(
                ctx(),
                &CreateNote::new(),
                json!({}),
                Some(IdempotencyKey("k".into())),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::DENIED);
        assert!(backend.stones().is_empty());
    }

    #[tokio::test]
    async fn mutation_without_effect_is_refused() {
        let (eng, _b) = engine(Arc::new(AllowAll));
        let op = NoopMutation {
            plan: ExecutionPlan::new(
                OperationId::from_static("demo.noop"),
                ExecutionProfileKind::Command,
            ),
        };
        let err = eng
            .execute(ctx(), &op, json!({}), Some(IdempotencyKey("k".into())))
            .await
            .unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::NO_EFFECT);
    }

    #[tokio::test]
    async fn plane_mismatch_is_denied() {
        let (eng, _b) = engine(Arc::new(AllowAll));
        let mut ctxb = ExecutionContext::builder(
            ExecutionProfileKind::Command,
            time::OffsetDateTime::now_utc(),
        );
        ctxb = ctxb
            .principal(PrincipalContext {
                id: kelson_foundation::PrincipalId::new(),
                display: None,
            })
            .plane(SecurityPlane::Platform);
        let cx = ctxb.build().unwrap();
        let err = eng
            .execute(
                cx,
                &CreateNote::new(),
                json!({}),
                Some(IdempotencyKey("k".into())),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::DENIED);
    }
}
