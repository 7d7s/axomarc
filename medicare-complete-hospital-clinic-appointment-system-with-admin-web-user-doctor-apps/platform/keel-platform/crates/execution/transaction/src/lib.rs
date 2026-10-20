//! # kelson-transaction
//! Transaction contracts (S1 split, STAGE-0-SPEC §8/S1 item 14): execution-neutral
//! traits + deterministic lock helpers. The SQL implementations live in
//! `kelson-postgres` (data layer); this crate carries no SQL client.
//!
//! Consumers: the execution backend (plan-driven options), and later the outbox
//! pump / job store / migration runners — all open transactions through the same
//! contract.

use std::future::Future;
use std::pin::Pin;

use kelson_foundation::AppError;

/// Local alias: contracts stay independent of the engine's BoxFuture.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Isolation levels understood by the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IsolationLevel {
    /// Kernel default.
    #[default]
    ReadCommitted,
    RepeatableRead,
    /// Snapshot isolation for Serialized plans (combine with advisory sequencing).
    Serializable,
}

/// Transaction opening options (compiled from the execution plan by callers).
#[derive(Debug, Clone, Default)]
pub struct TxOptions {
    pub isolation: IsolationLevel,
    pub readonly: bool,
    /// Statement budget (ms); backends derive it from `plan.timeout_secs`.
    pub statement_timeout_ms: Option<u64>,
}

impl TxOptions {
    /// Plan-driven options: Serialized plans get serializable isolation; the
    /// plan timeout becomes the statement budget.
    pub fn from_plan_fields(
        strategy_is_serialized: bool,
        timeout_secs: u64,
        readonly: bool,
    ) -> Self {
        Self {
            isolation: if strategy_is_serialized {
                IsolationLevel::Serializable
            } else {
                IsolationLevel::ReadCommitted
            },
            readonly,
            statement_timeout_ms: Some(timeout_secs.saturating_mul(1000)),
        }
    }
}

/// Minimal transaction handle: exec + explicit commit/rollback. Data layers add
/// their typed query surfaces on top (the execution backend keeps its own unit
/// contract for delta/audit/outbox writes).
pub trait TxHandle: Send {
    /// Execute one statement; returns rows affected.
    fn exec<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<u64, AppError>>;
    /// Commit; consumes the handle.
    fn commit(self: Box<Self>) -> BoxFuture<'static, Result<(), AppError>>;
    /// Rollback; consumes the handle (dropping without commit also rolls back).
    fn rollback(self: Box<Self>) -> BoxFuture<'static, Result<(), AppError>>;
}

/// Port for opening plain transactions (no execution-unit semantics).
pub trait TransactionPort: Send + Sync {
    fn begin<'a>(
        &'a self,
        options: &'a TxOptions,
    ) -> BoxFuture<'a, Result<Box<dyn TxHandle>, AppError>>;
}

/// Whether a failed transaction attempt may be run again (F-9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionFailure {
    /// The attempt lost a race — a deadlock or a serialization failure. The
    /// whole transaction can be run again and may succeed.
    Retryable,
    /// The attempt failed fatally; running it again cannot help.
    Fatal,
}

/// Classify an [`AppError`] for [`retry_transaction`].
///
/// A **persistence** error that the driver marked retryable is a lost race
/// (the SQL adapters mark a deadlock or a serialization failure that way);
/// everything else — a validation error, a denied call, a fatal database
/// error — is fatal.
pub fn classify_app_error(err: &AppError) -> TransactionFailure {
    use kelson_foundation::{ErrorCategory, Retryability};
    if err.category == ErrorCategory::Persistence && err.retryable == Retryability::Retryable {
        TransactionFailure::Retryable
    } else {
        TransactionFailure::Fatal
    }
}

/// Run a transaction body, re-running it while the failure is retryable.
///
/// The body must be **the whole transaction** (open, work, commit) — a retry
/// re-opens the transaction, so a partially applied body is rolled back with
/// it. `attempts` is the total number of tries (at least 1), and `delay` is
/// the backoff between them.
///
/// A failure that is not retryable is returned immediately, and the last
/// failure is returned when the attempts run out.
pub fn retry_transaction<T, F>(attempts: u32, delay: std::time::Duration, mut body: F) -> Result<T, AppError>
where
    F: FnMut() -> Result<T, AppError>,
{
    let attempts = attempts.max(1);
    let mut last: Option<AppError> = None;
    for attempt in 0..attempts {
        match body() {
            Ok(value) => return Ok(value),
            Err(err) => {
                if classify_app_error(&err) == TransactionFailure::Fatal {
                    return Err(err);
                }
                last = Some(err);
                if attempt + 1 < attempts && !delay.is_zero() {
                    std::thread::sleep(delay);
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| AppError::internal("transaction retry failed")))
}

// ---------------------------------------------------------------------------
// Lock helpers (S1): deterministic, injection-safe builders shared by all data
// layer implementations.
// ---------------------------------------------------------------------------

/// FNV-1a 64-bit: stable across runs/platforms (deliberately not std's hasher,
/// whose seed is process-local).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Deterministic advisory-lock key: positive 63-bit integer from `partition|key`.
/// `partition` is typically the tenant id keeps lock spaces collision-free.
pub fn advisory_lock_key(partition: &str, key: &str) -> i64 {
    (fnv1a64(format!("{partition}|{key}").as_bytes()) & 0x7fff_ffff_ffff_ffff) as i64
}

fn validate_ident(ident: &str) -> Result<(), AppError> {
    if ident.is_empty()
        || ident.len() > 64
        || !ident
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(AppError::validation(format!(
            "invalid SQL identifier {ident:?}: expected [a-z0-9_] (max 64)"
        )));
    }
    Ok(())
}

/// `SELECT 1 ... FOR UPDATE` existence + row lock (pessimistic plans).
pub fn row_lock(table: &str, id_column: &str) -> Result<String, AppError> {
    validate_ident(table)?;
    validate_ident(id_column)?;
    Ok(format!(
        "SELECT 1 FROM {table} WHERE {id_column} = $1 FOR UPDATE"
    ))
}

/// Row-lock select returning specific columns.
pub fn row_lock_select(table: &str, columns: &[&str], id_column: &str) -> Result<String, AppError> {
    validate_ident(table)?;
    validate_ident(id_column)?;
    for c in columns {
        validate_ident(c)?;
    }
    if columns.is_empty() {
        return Err(AppError::validation(
            "row_lock_select requires at least one column",
        ));
    }
    Ok(format!(
        "SELECT {} FROM {table} WHERE {id_column} = $1 FOR UPDATE",
        columns.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advisory_key_is_deterministic_and_positive() {
        let a = advisory_lock_key("t1", "finance.close");
        let b = advisory_lock_key("t1", "finance.close");
        let c = advisory_lock_key("t2", "finance.close");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a >= 0);
    }

    #[test]
    fn row_lock_builders_validate_identifiers() {
        assert_eq!(
            row_lock("appointments", "id").unwrap(),
            "SELECT 1 FROM appointments WHERE id = $1 FOR UPDATE"
        );
        assert!(row_lock("patients; DROP TABLE x", "id").is_err());
        assert!(row_lock("patients", "id; --").is_err());
        assert_eq!(
            row_lock_select("invoices", &["id", "status"], "id").unwrap(),
            "SELECT id, status FROM invoices WHERE id = $1 FOR UPDATE"
        );
        assert!(row_lock_select("t", &[], "id").is_err());
    }

    #[test]
    fn plan_driven_options() {
        let opts = TxOptions::from_plan_fields(true, 30, false);
        assert_eq!(opts.isolation, IsolationLevel::Serializable);
        assert_eq!(opts.statement_timeout_ms, Some(30_000));
        let opts = TxOptions::from_plan_fields(false, 15, true);
        assert_eq!(opts.isolation, IsolationLevel::ReadCommitted);
        assert!(opts.readonly);
    }

    /// A persistence error the driver marked retryable is a lost race.
    fn lost_race() -> AppError {
        AppError::persistence("deadlock detected")
    }

    /// A fatal database error (the driver marked it not retryable).
    fn fatal_db() -> AppError {
        let mut err = AppError::persistence("duplicate key");
        err.retryable = kelson_foundation::Retryability::NotRetryable;
        err
    }

    #[test]
    fn classification_separates_lost_races_from_fatal_failures() {
        assert_eq!(
            classify_app_error(&lost_race()),
            TransactionFailure::Retryable
        );
        assert_eq!(classify_app_error(&fatal_db()), TransactionFailure::Fatal);
        // A validation error is never a lost race, even if it claims to be.
        let mut validation = AppError::validation("bad input");
        validation.retryable = kelson_foundation::Retryability::Retryable;
        assert_eq!(classify_app_error(&validation), TransactionFailure::Fatal);
    }

    #[test]
    fn retry_reruns_a_lost_race_until_it_succeeds() {
        let mut calls = 0u32;
        let out = retry_transaction(5, std::time::Duration::ZERO, || {
            calls += 1;
            if calls < 3 {
                Err::<&str, _>(lost_race())
            } else {
                Ok("committed")
            }
        })
        .expect("the third attempt wins");
        assert_eq!(out, "committed");
        assert_eq!(calls, 3, "the body was re-run until it succeeded");
    }

    #[test]
    fn retry_stops_at_the_attempt_cap_and_returns_the_last_failure() {
        let mut calls = 0u32;
        let err = retry_transaction(3, std::time::Duration::ZERO, || {
            calls += 1;
            Err::<(), _>(lost_race())
        })
        .expect_err("every attempt lost the race");
        assert_eq!(calls, 3, "the attempt cap is respected");
        assert!(
            err.internal.as_deref().unwrap_or_default().contains("deadlock"),
            "the last failure is returned: {err:?}"
        );
    }

    #[test]
    fn retry_does_not_rerun_a_fatal_failure() {
        let mut calls = 0u32;
        let err = retry_transaction(5, std::time::Duration::ZERO, || {
            calls += 1;
            Err::<(), _>(fatal_db())
        })
        .expect_err("a fatal failure is returned as-is");
        assert_eq!(calls, 1, "a fatal failure is not re-run");
        assert!(
            err.internal.as_deref().unwrap_or_default().contains("duplicate key"),
            "the fatal failure is returned as-is: {err:?}"
        );
    }

    #[test]
    fn retry_runs_the_body_once_when_it_succeeds() {
        let mut calls = 0u32;
        let out = retry_transaction(5, std::time::Duration::ZERO, || {
            calls += 1;
            Ok(7u32)
        })
        .expect("the first attempt wins");
        assert_eq!(out, 7);
        assert_eq!(calls, 1);
    }
}

