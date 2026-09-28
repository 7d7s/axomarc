//! # keel-transaction
//! Transaction contracts (S1 split, STAGE-0-SPEC §8/S1 item 14): execution-neutral
//! traits + deterministic lock helpers. The SQL implementations live in
//! `keel-postgres` (data layer); this crate carries no SQL client.
//!
//! Consumers: the execution backend (plan-driven options), and later the outbox
//! pump / job store / migration runners — all open transactions through the same
//! contract.

use std::future::Future;
use std::pin::Pin;

use keel_foundation::AppError;

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
}
