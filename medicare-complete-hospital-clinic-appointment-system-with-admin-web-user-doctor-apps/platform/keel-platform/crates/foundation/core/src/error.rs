//! Canonical error protocol (spec §31/§47). One error type across the platform;
//! the domain never returns HTTP numbers — transport maps `AppError` at the edge.

use crate::id::CorrelationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ErrorCategory {
    Validation,
    Authentication,
    Authorization,
    NotFound,
    Conflict,
    Domain,
    Workflow,
    Persistence,
    External,
    ResourceExhausted,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Retryability {
    Retryable,
    NotRetryable,
    IdempotentOnly,
}

/// Stable machine-oriented codes. Public API — never renumber, only append.
pub mod codes {
    pub const VALIDATION: &str = "KER_VALIDATION";
    pub const UNAUTHENTICATED: &str = "AUTH_UNAUTHENTICATED";
    pub const DENIED: &str = "AUTH_DENIED";
    pub const STALE_REVISION: &str = "AUTH_STALE_REVISION";
    pub const NOT_FOUND: &str = "KER_NOT_FOUND";
    pub const NOT_ACCEPTABLE: &str = "KER_NOT_ACCEPTABLE";
    pub const CONFLICT: &str = "KER_CONFLICT";
    pub const WORKFLOW: &str = "KER_WORKFLOW";
    pub const MISSING_DELTA: &str = "KER_MISSING_DELTA";
    pub const READ_MUTATED: &str = "KER_READ_MUTATED";
    pub const NO_EFFECT: &str = "KER_NO_EFFECT";
    pub const CAP_LIMIT: &str = "CAP_LIMIT_EXCEEDED";
    pub const ELEVATION: &str = "SEC_ELEVATION";
    pub const PERSISTENCE: &str = "DB_ERROR";
    pub const EXTERNAL: &str = "EXT_ERROR";
    pub const INTERNAL: &str = "KER_INTERNAL";
    pub const NOT_IMPLEMENTED: &str = "KER_NOT_IMPLEMENTED";
    pub const IDEMPOTENCY_CONFLICT: &str = "KER_IDEMPOTENCY_CONFLICT";
    pub const DEADLINE: &str = "KER_DEADLINE";
    pub const BOOT: &str = "KER_BOOT";
    pub const MAINTENANCE: &str = "KER_MAINTENANCE";
    /// Wave 3 Phase 5 strict mode: `prevent_lazy_loading` tripped on
    /// `load_relation`/`load_missing` (the `with()` eager path stays legal).
    pub const LAZY_LOAD_PREVENTED: &str = "LAZY_LOAD_PREVENTED";
    /// Wave 3 Phase 6 optimistic locking: a `version_column` compare-and-swap
    /// matched zero rows — another writer modified the record between the
    /// read and the write. Conflict category (HTTP 409 via
    /// `status_for_error`).
    pub const STALE_RECORD: &str = "STALE_RECORD";
    /// Wave 4 Phase 7 password-confirm: a `confirmed`-chained route was hit
    /// without a fresh `POST /auth/confirm` stamp, or the stamp is older
    /// than `auth.confirm_timeout_secs`. Authorization category (HTTP 403
    /// via `status_for_error`).
    pub const CONFIRMATION_REQUIRED: &str = "CONFIRMATION_REQUIRED";
    /// Wave 9 B-30 (docs/19): a WASM plugin burned its per-call instruction
    /// budget (fuel metering). ResourceExhausted/Retryable; the runtime pins
    /// it to HTTP 503 at boot (`ErrorRenderRegistry::set_status`) because it
    /// behaves like a timeout, not a client-side rate limit.
    pub const WASM_FUEL: &str = "KER_WASM_FUEL";
}

/// The one error type. `meta` must stay PHI-free (safe metadata only).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppError {
    pub code: String,
    pub category: ErrorCategory,
    pub severity: Severity,
    pub retryable: Retryability,
    pub public_message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub meta: serde_json::Map<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation: Option<CorrelationId>,
}

impl AppError {
    pub fn new(
        code: impl Into<String>,
        category: ErrorCategory,
        severity: Severity,
        retryable: Retryability,
        public_message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            category,
            severity,
            retryable,
            public_message: public_message.into(),
            internal: None,
            meta: Default::default(),
            correlation: None,
        }
    }

    pub fn validation(public_message: impl Into<String>) -> Self {
        Self::new(
            codes::VALIDATION,
            ErrorCategory::Validation,
            Severity::Low,
            Retryability::NotRetryable,
            public_message,
        )
    }

    pub fn unauthenticated() -> Self {
        Self::new(
            codes::UNAUTHENTICATED,
            ErrorCategory::Authentication,
            Severity::Medium,
            Retryability::NotRetryable,
            "authentication required",
        )
    }

    pub fn denied(reason: impl Into<String>) -> Self {
        Self::new(
            codes::DENIED,
            ErrorCategory::Authorization,
            Severity::Medium,
            Retryability::NotRetryable,
            reason,
        )
    }

    pub fn not_found(resource: impl std::fmt::Display) -> Self {
        Self::new(
            codes::NOT_FOUND,
            ErrorCategory::NotFound,
            Severity::Low,
            Retryability::NotRetryable,
            format!("{resource} not found"),
        )
    }

    /// Content negotiation refused the request's media-type parameters
    /// (JSON:API `Accept` parameters, ADR 0007). Validation category keeps
    /// this a non-reportable client mistake; the error-render registry pins
    /// the wire status to HTTP 406 at every boot (the category default is
    /// 422).
    pub fn not_acceptable() -> Self {
        Self::new(
            codes::NOT_ACCEPTABLE,
            ErrorCategory::Validation,
            Severity::Low,
            Retryability::NotRetryable,
            "unsupported media type parameters",
        )
    }

    pub fn conflict(public_message: impl Into<String>) -> Self {
        Self::new(
            codes::CONFLICT,
            ErrorCategory::Conflict,
            Severity::Low,
            Retryability::NotRetryable,
            public_message,
        )
    }

    /// Wave 3 Phase 6 optimistic locking: the `version_column` compare-and-swap
    /// matched zero rows (a concurrent writer won). Conflict category, so the
    /// shared `status_for_error` mapping renders it as HTTP 409 without a
    /// per-code error-registry entry.
    pub fn stale_record(public_message: impl Into<String>) -> Self {
        Self::new(
            codes::STALE_RECORD,
            ErrorCategory::Conflict,
            Severity::Low,
            Retryability::NotRetryable,
            public_message,
        )
    }

    pub fn workflow(public_message: impl Into<String>) -> Self {
        Self::new(
            codes::WORKFLOW,
            ErrorCategory::Workflow,
            Severity::Low,
            Retryability::NotRetryable,
            public_message,
        )
    }

    pub fn persistence(internal: impl Into<String>) -> Self {
        Self::new(
            codes::PERSISTENCE,
            ErrorCategory::Persistence,
            Severity::High,
            Retryability::Retryable,
            "a database error occurred",
        )
        .with_internal(internal)
    }

    pub fn external(service: &'static str, internal: impl Into<String>) -> Self {
        Self::new(
            codes::EXTERNAL,
            ErrorCategory::External,
            Severity::Medium,
            Retryability::Retryable,
            format!("external service {service} failed"),
        )
        .with_internal(internal)
    }

    pub fn internal(internal: impl Into<String>) -> Self {
        Self::new(
            codes::INTERNAL,
            ErrorCategory::Internal,
            Severity::High,
            Retryability::Retryable,
            "internal error",
        )
        .with_internal(internal)
    }

    pub fn not_implemented(what: impl Into<String>) -> Self {
        Self::new(
            codes::NOT_IMPLEMENTED,
            ErrorCategory::Internal,
            Severity::Low,
            Retryability::NotRetryable,
            format!("{} not implemented yet", what.into()),
        )
    }

    pub fn cap_limit(public_message: impl Into<String>) -> Self {
        Self::new(
            codes::CAP_LIMIT,
            ErrorCategory::ResourceExhausted,
            Severity::Medium,
            Retryability::NotRetryable,
            public_message,
        )
    }

    pub fn with_internal(mut self, internal: impl Into<String>) -> Self {
        self.internal = Some(internal.into());
        self
    }

    pub fn with_meta(mut self, key: impl Into<String>, value: impl serde::Serialize) -> Self {
        if let Ok(v) = serde_json::to_value(value) {
            self.meta.insert(key.into(), v);
        }
        self
    }

    pub fn with_correlation(mut self, correlation: CorrelationId) -> Self {
        self.correlation = Some(correlation);
        self
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.public_message)
    }
}

impl std::error::Error for AppError {}
