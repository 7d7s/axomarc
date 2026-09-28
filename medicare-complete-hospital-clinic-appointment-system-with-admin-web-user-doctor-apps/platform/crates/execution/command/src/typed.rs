//! Typed handler adapter (Wave 1): bridges typed request/response shapes
//! onto `OperationHandler` so controllers get compile-time typed
//! input/output instead of raw `Value`.
//!
//! Contract:
//! - [`TypedInput`] parses + validates the raw JSON value before the
//!   business logic runs (FormRequest role). Parse failures are
//!   validation-category errors (422 at the HTTP edge).
//! - [`TypedOutput`] serializes the business result back to wire JSON.
//! - [`Json<T>`] is the parse-only boundary (serde shape check, no extra
//!   validation). Richer validation lives in transport adapters (e.g.
//!   `keel_http::Validated` over `RequestViewModel`) or in the domain.
//! - Existing raw-`Value` handlers remain valid; typed handlers are opt-in.

use std::sync::Arc;

use keel_context::ExecutionContext;
use keel_foundation::AppError;
use keel_execution::{BoxFuture, ExecutionPlan, ExecutionUnit, OperationHandler};
use serde_json::Value;

/// Typed request boundary: raw JSON -> validated business input.
pub trait TypedInput: Sized + Send + 'static {
    fn from_value(value: Value) -> Result<Self, AppError>;
}

/// Typed response boundary: business result -> wire JSON.
pub trait TypedOutput: Sized + Send + 'static {
    fn into_value(self) -> Value;
}

/// Parse-only typed boundary: serde shape check, no extra validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Json<T>(pub T);

impl<T> TypedInput for Json<T>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    fn from_value(value: Value) -> Result<Self, AppError> {
        serde_json::from_value(value)
            .map(Json)
            .map_err(|e| AppError::validation(format!("invalid request shape: {e}")))
    }
}

impl<T> TypedOutput for Json<T>
where
    T: serde::Serialize + Send + 'static,
{
    fn into_value(self) -> Value {
        serde_json::to_value(&self.0).unwrap_or(Value::Null)
    }
}

/// Boxed typed handle function (keeps `TypedHandler<I, O>` nameable for
/// type aliases and newtype wrappers in modules).
pub type TypedHandleFn<I, O> = Arc<
    dyn Fn(Arc<ExecutionContext>, I, &mut dyn ExecutionUnit) -> BoxFuture<'static, Result<O, AppError>>
        + Send
        + Sync,
>;

/// Adapter bridging typed input/output onto [`OperationHandler`] (Wave 1).
/// The input is parsed + validated BEFORE the handler runs; the output is
/// serialized after. Mutations/outbox/audit still flow through the
/// `ExecutionUnit` exactly like raw handlers.
pub struct TypedHandler<I, O> {
    plan: ExecutionPlan,
    handler: TypedHandleFn<I, O>,
}

impl<I, O> TypedHandler<I, O>
where
    I: TypedInput,
    O: TypedOutput,
{
    pub fn new<F>(plan: ExecutionPlan, handler: F) -> Self
    where
        F: Fn(Arc<ExecutionContext>, I, &mut dyn ExecutionUnit) -> BoxFuture<'static, Result<O, AppError>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            plan,
            handler: Arc::new(handler),
        }
    }

    /// Wrap a pre-boxed handler (rare; used by bridges).
    pub fn from_boxed(plan: ExecutionPlan, handler: TypedHandleFn<I, O>) -> Self {
        Self { plan, handler }
    }
}

impl<I, O> OperationHandler for TypedHandler<I, O>
where
    I: TypedInput,
    O: TypedOutput,
{
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        ctx: Arc<ExecutionContext>,
        input: Value,
        unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<Value, AppError>> {
        let parsed = match I::from_value(input) {
            Ok(parsed) => parsed,
            Err(err) => return Box::pin(async move { Err(err) }),
        };
        let outcome = (self.handler)(ctx, parsed, unit);
        Box::pin(async move { outcome.await.map(TypedOutput::into_value) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_foundation::{codes, ExecutionProfileKind, OperationId};
    use keel_execution::{
        AuthorizationPort, AuthzDecision, AuthzRequest, Engine, InMemoryBackend,
        InMemoryIdempotency,
    };

    #[derive(serde::Deserialize)]
    struct EchoIn {
        msg: String,
    }

    #[derive(serde::Serialize, PartialEq, Debug)]
    struct EchoOut {
        echoed: String,
    }

    struct AllowAll;
    impl AuthorizationPort for AllowAll {
        fn authorize(&self, _ctx: &ExecutionContext, _request: &AuthzRequest) -> AuthzDecision {
            AuthzDecision::allow("test")
        }
    }

    fn engine() -> Arc<Engine> {
        Arc::new(Engine::new(
            Arc::new(AllowAll),
            Arc::new(InMemoryBackend::new()),
            Arc::new(InMemoryIdempotency::new()),
        ))
    }

    fn context() -> Arc<ExecutionContext> {
        use keel_context::{PrincipalContext, ScopeSnapshot, TenantContext};
        use keel_foundation::{AuthorizationRevision, PrincipalId, TenantId};
        ExecutionContext::builder(ExecutionProfileKind::Read, time::OffsetDateTime::now_utc())
            .principal(PrincipalContext {
                id: PrincipalId::new(),
                display: None,
            })
            .tenant(TenantContext {
                id: TenantId::new(),
                slug: "test".into(),
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

    fn handler() -> TypedHandler<Json<EchoIn>, Json<EchoOut>> {
        TypedHandler::new(
            ExecutionPlan::new(
                OperationId::from_static("demo.typed"),
                ExecutionProfileKind::Read,
            ),
            |_ctx, input: Json<EchoIn>, _unit| {
                Box::pin(async move { Ok(Json(EchoOut { echoed: input.0.msg })) })
            },
        )
    }

    #[tokio::test]
    async fn typed_handler_roundtrips_through_the_engine() {
        let outcome = engine()
            .execute(
                context(),
                &handler(),
                serde_json::json!({ "msg": "hi" }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(outcome.result, serde_json::json!({ "echoed": "hi" }));
    }

    #[tokio::test]
    async fn typed_handler_rejects_bad_shapes_with_validation_errors() {
        let err = engine()
            .execute(context(), &handler(), serde_json::json!({ "nope": 1 }), None)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::VALIDATION);
    }

    #[test]
    fn json_output_serializes() {
        let out = Json(EchoOut {
            echoed: "x".into(),
        });
        assert_eq!(out.into_value(), serde_json::json!({ "echoed": "x" }));
    }
}
