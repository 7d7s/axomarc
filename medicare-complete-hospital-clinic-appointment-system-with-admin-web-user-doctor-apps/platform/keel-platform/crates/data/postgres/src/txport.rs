//! `TransactionPort` over `PgPool` (S1 split): plain transactions through the
//! kelson-transaction contracts — no execution-unit semantics here. The execution
//! backend (tx.rs) keeps its unit contract; the outbox pump / job store /
//! maintenance flows use this port directly.

use kelson_foundation::AppError;
use kelson_foundation::Retryability;
use kelson_transaction::{BoxFuture, IsolationLevel, TransactionPort, TxHandle, TxOptions};
use sqlx::postgres::PgPool;

/// Postgres SQLSTATEs that mean "this transaction lost a race; run it again":
/// `40P01` (deadlock_detected) and `40001` (serialization_failure).
///
/// The classification lives here, where the driver error is still typed: the
/// SQLSTATE does not survive the trip into an [`AppError`]'s message.
pub fn is_retryable_transaction_error(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Database(db) => {
            matches!(db.code().as_deref(), Some("40P01") | Some("40001"))
        }
        _ => false,
    }
}

/// Map a driver error to an [`AppError`], marking a lost race **retryable** so
/// the caller can re-run the whole transaction
/// ([`kelson_transaction::retry_transaction`]). Every other database failure is
/// marked not-retryable: re-running it cannot help.
fn tx_error(op: &str, e: &sqlx::Error) -> AppError {
    let mut err = AppError::persistence(format!("{op}: {e}"));
    err.retryable = if is_retryable_transaction_error(e) {
        Retryability::Retryable
    } else {
        Retryability::NotRetryable
    };
    err
}

pub struct PostgresTransactions {
    pool: PgPool,
}

impl PostgresTransactions {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

pub struct PgTxHandle {
    tx: Option<sqlx::Transaction<'static, sqlx::Postgres>>,
}

impl TxHandle for PgTxHandle {
    fn exec<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<u64, AppError>> {
        Box::pin(async move {
            let tx = self
                .tx
                .as_mut()
                .ok_or_else(|| AppError::internal("tx already completed"))?;
            let result = sqlx::query(sql)
                .execute(&mut **tx)
                .await
                .map_err(|e| tx_error("exec", &e))?;
            Ok(result.rows_affected())
        })
    }

    fn commit(mut self: Box<Self>) -> BoxFuture<'static, Result<(), AppError>> {
        Box::pin(async move {
            let tx = self
                .tx
                .take()
                .ok_or_else(|| AppError::internal("tx already completed"))?;
            tx.commit()
                .await
                .map_err(|e| tx_error("commit", &e))
        })
    }

    fn rollback(mut self: Box<Self>) -> BoxFuture<'static, Result<(), AppError>> {
        Box::pin(async move {
            let tx = self
                .tx
                .take()
                .ok_or_else(|| AppError::internal("tx already completed"))?;
            tx.rollback()
                .await
                .map_err(|e| tx_error("rollback", &e))
        })
    }
}

impl TransactionPort for PostgresTransactions {
    fn begin<'a>(
        &'a self,
        options: &'a TxOptions,
    ) -> BoxFuture<'a, Result<Box<dyn TxHandle>, AppError>> {
        Box::pin(async move {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|e| tx_error("begin", &e))?;

            // Isolation must be set before the first statement of the transaction.
            match options.isolation {
                IsolationLevel::Serializable => {
                    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| tx_error("isolation", &e))?;
                }
                IsolationLevel::RepeatableRead => {
                    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| tx_error("isolation", &e))?;
                }
                IsolationLevel::ReadCommitted => {} // postgres default
            }
            if options.readonly {
                sqlx::query("SET TRANSACTION READ ONLY")
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| tx_error("readonly", &e))?;
            }
            if let Some(ms) = options.statement_timeout_ms {
                sqlx::query(&format!("SET LOCAL statement_timeout = {ms}"))
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| tx_error("statement_timeout", &e))?;
            }

            let handle: Box<dyn TxHandle> = Box::new(PgTxHandle { tx: Some(tx) });
            Ok(handle)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;
    use std::error::Error as StdError;

    /// A database error double with a chosen SQLSTATE.
    #[derive(Debug)]
    struct FakeDbError {
        code: &'static str,
    }

    impl std::fmt::Display for FakeDbError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "fake database error ({})", self.code)
        }
    }

    impl StdError for FakeDbError {}

    impl sqlx::error::DatabaseError for FakeDbError {
        fn message(&self) -> &str {
            "fake database error"
        }
        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.code))
        }
        fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
            self
        }
        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::Other
        }
    }

    fn db_error(code: &'static str) -> sqlx::Error {
        sqlx::Error::Database(Box::new(FakeDbError { code }))
    }

    /// A deadlock or a serialization failure is a lost race: the whole
    /// transaction can be run again.
    #[test]
    fn deadlock_and_serialization_failures_are_retryable() {
        assert!(is_retryable_transaction_error(&db_error("40P01")));
        assert!(is_retryable_transaction_error(&db_error("40001")));
    }

    /// Everything else is fatal — re-running it cannot help.
    #[test]
    fn other_database_failures_are_fatal() {
        assert!(!is_retryable_transaction_error(&db_error("23505")));
        assert!(!is_retryable_transaction_error(&db_error("42P01")));
        assert!(!is_retryable_transaction_error(&sqlx::Error::RowNotFound));
    }

    /// The mapping carries the classification into the AppError, so the retry
    /// helper can act on it.
    #[test]
    fn a_lost_race_maps_to_a_retryable_app_error() {
        let lost = tx_error("exec", &db_error("40P01"));
        assert_eq!(lost.retryable, Retryability::Retryable);
        let fatal = tx_error("exec", &db_error("23505"));
        assert_eq!(fatal.retryable, Retryability::NotRetryable);
    }
}
