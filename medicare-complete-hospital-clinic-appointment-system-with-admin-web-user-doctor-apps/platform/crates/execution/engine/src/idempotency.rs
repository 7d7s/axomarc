//! Execution-level idempotency (D-51): engine module + port (S1 extraction).
//! Works identically over REST/CLI/jobs/webhooks/imports/AI. Replays are first-
//! class results (distinct receipt), never silent re-executions.

use std::sync::Arc;

use keel_context::ExecutionContext;
use keel_foundation::{codes, AppError, OperationId, TenantId};
use serde_json::Value;

use crate::plan::ExecutionPlan;
use crate::unit::BoxFuture;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct IdempotencyKey(pub String);

/// Deterministic, dependency-light payload fingerprint.
pub fn payload_hash(input: &Value) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(input)
        .unwrap_or_default()
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Debug, Clone)]
pub enum IdempotencyCheck {
    New,
    Replay(Value),
    /// Same key, different payload hash (spec: conflict, never silent overwrite).
    HashConflict,
}

/// Durable idempotency store (postgres implements; in-memory for dev/tests).
pub trait IdempotencyPort: Send + Sync {
    fn check<'a>(
        &'a self,
        tenant: Option<TenantId>,
        operation: &'a OperationId,
        key: &'a IdempotencyKey,
        payload_hash: &'a str,
    ) -> BoxFuture<'a, Result<IdempotencyCheck, AppError>>;

    fn store<'a>(
        &'a self,
        tenant: Option<TenantId>,
        operation: &'a OperationId,
        key: &'a IdempotencyKey,
        payload_hash: &'a str,
        result: &'a Value,
    ) -> BoxFuture<'a, Result<(), AppError>>;
}

/// Outcome of the pre-execution gate.
#[derive(Debug, Clone)]
pub enum GateOutcome {
    /// Fresh execution; carries the payload hash for the post-execution store.
    Proceed { hash: String },
    /// Key already executed with this exact payload: replay the stored result.
    Replay(Value),
}

/// The engine's idempotency boundary: one gate before execution, one store
/// after commit. Keeps engine.rs free of store mechanics.
pub struct IdempotencyGate {
    port: Arc<dyn IdempotencyPort>,
}

impl IdempotencyGate {
    pub fn new(port: Arc<dyn IdempotencyPort>) -> Self {
        Self { port }
    }

    /// Pre-execution gate (engine order: after authorization, before unit run).
    pub async fn gate(
        &self,
        ctx: &ExecutionContext,
        plan: &ExecutionPlan,
        key: Option<&IdempotencyKey>,
        payload: &Value,
    ) -> Result<GateOutcome, AppError> {
        let hash = payload_hash(payload);
        let Some(key) = key else {
            if plan.idempotency.required {
                return Err(AppError::validation(format!(
                    "operation {} requires an idempotency key",
                    plan.operation
                )));
            }
            return Ok(GateOutcome::Proceed { hash });
        };
        match self
            .port
            .check(ctx.tenant_id(), &plan.operation, key, &hash)
            .await?
        {
            IdempotencyCheck::New => Ok(GateOutcome::Proceed { hash }),
            IdempotencyCheck::Replay(result) => Ok(GateOutcome::Replay(result)),
            IdempotencyCheck::HashConflict => Err(AppError::new(
                codes::IDEMPOTENCY_CONFLICT,
                keel_foundation::ErrorCategory::Conflict,
                keel_foundation::Severity::Medium,
                keel_foundation::Retryability::NotRetryable,
                "idempotency key reused with a different payload",
            )),
        }
    }

    /// Post-execution store (only for keyed executions).
    pub async fn record(
        &self,
        tenant: Option<TenantId>,
        operation: &OperationId,
        key: &IdempotencyKey,
        hash: &str,
        result: &Value,
    ) -> Result<(), AppError> {
        self.port.store(tenant, operation, key, hash, result).await
    }
}

/// In-memory idempotency store (tests + local dev).
#[derive(Default)]
pub struct InMemoryIdempotency {
    entries: std::sync::Mutex<std::collections::HashMap<String, (String, Value)>>,
}

impl InMemoryIdempotency {
    pub fn new() -> Self {
        Self::default()
    }

    fn composite_key(
        tenant: Option<TenantId>,
        operation: &OperationId,
        key: &IdempotencyKey,
    ) -> String {
        format!(
            "{}|{}|{}",
            tenant.map(|t| t.to_string()).unwrap_or_default(),
            operation,
            key.0
        )
    }
}

impl IdempotencyPort for InMemoryIdempotency {
    fn check<'a>(
        &'a self,
        tenant: Option<TenantId>,
        operation: &'a OperationId,
        key: &'a IdempotencyKey,
        payload_hash: &'a str,
    ) -> BoxFuture<'a, Result<IdempotencyCheck, AppError>> {
        Box::pin(async move {
            let map = self.entries.lock().unwrap();
            Ok(
                match map.get(&Self::composite_key(tenant, operation, key)) {
                    None => IdempotencyCheck::New,
                    Some((hash, result)) if hash == payload_hash => {
                        IdempotencyCheck::Replay(result.clone())
                    }
                    Some(_) => IdempotencyCheck::HashConflict,
                },
            )
        })
    }

    fn store<'a>(
        &'a self,
        tenant: Option<TenantId>,
        operation: &'a OperationId,
        key: &'a IdempotencyKey,
        payload_hash: &'a str,
        result: &'a Value,
    ) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            self.entries.lock().unwrap().insert(
                Self::composite_key(tenant, operation, key),
                (payload_hash.to_string(), result.clone()),
            );
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_foundation::{ExecutionProfileKind, PrincipalId, SecurityPlane};
    use time::OffsetDateTime;

    fn ctx() -> Arc<ExecutionContext> {
        ExecutionContext::builder(ExecutionProfileKind::System, OffsetDateTime::now_utc())
            .plane(SecurityPlane::Platform)
            .principal(keel_context::PrincipalContext {
                id: PrincipalId::new(),
                display: None,
            })
            .build()
            .unwrap()
    }

    #[test]
    fn gate_requires_key_when_plan_demands_it() {
        let gate = IdempotencyGate::new(Arc::new(InMemoryIdempotency::new()));
        let plan = ExecutionPlan::new(
            OperationId::from_static("demo.op"),
            ExecutionProfileKind::Financial,
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt
            .block_on(gate.gate(&ctx(), &plan, None, &serde_json::json!({})))
            .unwrap_err();
        assert!(err.to_string().contains("requires an idempotency key"));
    }

    #[test]
    fn gate_replays_and_conflicts() {
        let gate = IdempotencyGate::new(Arc::new(InMemoryIdempotency::new()));
        let plan = ExecutionPlan::new(
            OperationId::from_static("demo.op"),
            ExecutionProfileKind::System,
        );
        let key = IdempotencyKey("k1".into());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ctx = ctx();
        match rt
            .block_on(gate.gate(&ctx, &plan, Some(&key), &serde_json::json!({"v":1})))
            .unwrap()
        {
            GateOutcome::Proceed { hash } => {
                rt.block_on(gate.record(
                    ctx.tenant_id(),
                    &plan.operation,
                    &key,
                    &hash,
                    &serde_json::json!({"done":true}),
                ))
                .unwrap();
            }
            GateOutcome::Replay(_) => panic!("first execution must not replay"),
        }
        // Same payload → replay with stored result.
        match rt
            .block_on(gate.gate(&ctx, &plan, Some(&key), &serde_json::json!({"v":1})))
            .unwrap()
        {
            GateOutcome::Replay(v) => assert_eq!(v, serde_json::json!({"done":true})),
            GateOutcome::Proceed { .. } => panic!("expected replay"),
        }
        // Different payload → conflict.
        let err = rt
            .block_on(gate.gate(&ctx, &plan, Some(&key), &serde_json::json!({"v":2})))
            .unwrap_err();
        assert_eq!(err.code, codes::IDEMPOTENCY_CONFLICT);
    }
}
