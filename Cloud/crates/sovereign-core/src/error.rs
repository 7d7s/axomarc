// `AppError` — the single error type the use cases and adapters return.
// It is `thiserror`-based, `Send + Sync + 'static` so it can cross
// async boundaries. Adapters may convert into it (e.g. `From<sqlx::Error>`),
// use cases may add contextual variants.
//
// The variants are intentionally low-cardinality; if you find yourself
// wanting a `NotEnoughFoo` variant, prefer a `Validation` with a
// structured message.

use thiserror::Error;

/// Every fallible operation in the use cases and adapters returns this.
#[derive(Debug, Error)]
pub enum AppError {
    /// A `WHERE id = ?` returned no rows.
    #[error("not found: {0}")]
    NotFound(&'static str),

    /// Optimistic-concurrency check failed. The HTTP layer maps this to
    /// 409 with `current_version` in the body (RFC 9457 problem+json).
    #[error("conflict on {0}: current version is {1}")]
    Conflict(&'static str, i64),

    /// A state-machine transition was attempted that the documented
    /// state machine forbids. The HTTP layer maps this to 409.
    #[error("invalid state transition on {0}: {1} -> {2}")]
    InvalidTransition(&'static str, String, String),

    /// Caller-provided input failed validation. The HTTP layer maps
    /// this to 400.
    #[error("validation: {0}")]
    Validation(String),

    /// A downstream system (Docker, Caddy, S3, …) returned an error
    /// that is not a programming error.
    #[error("upstream: {0}")]
    Upstream(String),

    /// Caller is not authorized for the action. The HTTP layer maps
    /// this to 403.
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// Caller is not authenticated at all. 401.
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),

    /// The audit log rejected a mutation that would have violated the
    /// append-only contract. The mutation that caused this is rolled
    /// back by the use case.
    #[error("audit: {0}")]
    Audit(String),

    /// A database call failed for an unexpected reason. Use sparingly;
    /// prefer the typed variants above.
    #[error("storage: {0}")]
    Storage(String),

    /// Catch-all for programmer errors that should be unreachable.
    #[error("internal: {0}")]
    Internal(String),
}

impl AppError {
    /// Convenience: build a `Validation` from any `Display` type.
    pub fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }

    /// Convenience: build an `Upstream` from any `Display` type.
    pub fn upstream(msg: impl Into<String>) -> Self {
        Self::Upstream(msg.into())
    }

    /// Convenience: build an `Internal` from any `Display` type.
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    /// Convenience: build a `NotFound` with the entity name.
    pub fn not_found(entity: &'static str) -> Self {
        Self::NotFound(entity)
    }

    /// The HTTP status code this error should map to, when surfaced via
    /// the API. Returns `None` for internal errors (the API returns
    /// 500 with no detail — see `docs/architecture.md` §4.2).
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::NotFound(_) => Some(404),
            Self::Conflict(_, _) | Self::InvalidTransition(_, _, _) => Some(409),
            Self::Validation(_) => Some(400),
            Self::Unauthorized(_) => Some(403),
            Self::Unauthenticated(_) => Some(401),
            Self::Upstream(_) => Some(502),
            Self::Audit(_) => Some(500),
            Self::Storage(_) | Self::Internal(_) => None,
        }
    }
}

/// Convert `sqlx` errors into `AppError`. This is the only mapping
/// from the storage adapter to the use-case error type; do not
/// leak `sqlx::Error` past the adapter boundary.
impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        match &e {
            sqlx::Error::RowNotFound => Self::not_found("row"),
            sqlx::Error::Database(db_err) => {
                // SQLite's `constraint failed` is how the `audit_no_update`
                // and `audit_no_delete` triggers surface. Map it explicitly
                // so the use case can react (the mutation is rolled back).
                let msg = db_err.message();
                eprintln!(
                    "[storage] sqlx database error: {msg} (code: {:?})",
                    db_err.code()
                );
                if msg.contains("append-only") {
                    Self::Audit(msg.to_string())
                } else if msg.contains("UNIQUE") {
                    Self::Conflict("row", 0)
                } else {
                    // CHECK, FOREIGN KEY, NOT NULL — surface as Storage so
                    // the test failure shows the actual cause.
                    Self::Storage(msg.to_string())
                }
            }
            sqlx::Error::Io(io_err) => Self::Storage(format!("io: {io_err}")),
            sqlx::Error::Tls(tls_err) => Self::Storage(format!("tls: {tls_err}")),
            sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed => {
                Self::Storage(format!("pool: {e}"))
            }
            _ => Self::Storage(e.to_string()),
        }
    }
}

/// Marker for storage-layer concurrency races that should be reported
/// back to the caller as a 409 rather than a 500. Used by the OCC path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentVersion(pub i64);

impl From<CurrentVersion> for AppError {
    fn from(c: CurrentVersion) -> Self {
        Self::Conflict("row", c.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_status_for_known_variants() {
        assert_eq!(AppError::not_found("app").http_status(), Some(404));
        assert_eq!(AppError::Conflict("app", 3).http_status(), Some(409));
        assert_eq!(
            AppError::InvalidTransition("deployment", "healthy".into(), "pending".into())
                .http_status(),
            Some(409)
        );
        assert_eq!(AppError::validation("bad").http_status(), Some(400));
        assert_eq!(AppError::Unauthorized("x".into()).http_status(), Some(403));
        assert_eq!(
            AppError::Unauthenticated("x".into()).http_status(),
            Some(401)
        );
        assert_eq!(AppError::upstream("x").http_status(), Some(502));
    }

    #[test]
    fn http_status_none_for_internal() {
        assert_eq!(AppError::internal("x").http_status(), None);
        assert_eq!(AppError::Storage("x".into()).http_status(), None);
    }

    #[test]
    fn display_messages_are_useful() {
        let e = AppError::Conflict("app", 7);
        assert_eq!(e.to_string(), "conflict on app: current version is 7");
    }
}
