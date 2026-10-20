//! `TransactionPort` over `MySqlPool` (S1 split): plain transactions through
//! the kelson-transaction contracts - no execution-unit semantics here. The
//! execution backend (tx.rs) keeps its unit contract; the outbox pump / job
//! store / maintenance flows use this port directly.
//!
//! MySQL port notes (divergences from `kelson_postgres::txport`):
//! - Transaction characteristics are set with ONE
//!   `SET TRANSACTION ISOLATION LEVEL ..., READ WRITE|READ ONLY` statement
//!   BEFORE `BEGIN`: MySQL rejects `SET TRANSACTION` inside a transaction
//!   (ERROR 1568), and `SET SESSION TRANSACTION` would not affect the current
//!   transaction, so the statement moves ahead of the begin (the pg twin
//!   issues BEGIN first). The access mode is always spelled out so a stale
//!   next-transaction value on a pooled connection can never leak in.
//! - `IsolationLevel::ReadCommitted` (the kernel default) is set EXPLICITLY:
//!   MySQL's own default is REPEATABLE READ, so without this statement every
//!   plain transaction would silently run at a stronger level than on pg.
//! - The pg `SET LOCAL statement_timeout` budget is dropped: MySQL has no
//!   per-statement timeout (`MAX_EXECUTION_TIME` is session-scoped and
//!   SELECT-only; setting it would leak a kill timer onto every pooled
//!   connection).
//! - The `SET TRANSACTION` statement and `TxHandle::exec` run over the simple
//!   query path (COM_QUERY) via `sqlx::raw_sql`, not `sqlx::query`: MySQL's
//!   prepared-statement protocol only accepts a documented subset of
//!   statement types (error 1295 "not supported in the prepared statement
//!   protocol") and `SET TRANSACTION` is not in it, while `sqlx::query`
//!   prepares even a zero-bind statement. `exec` must accept whatever single
//!   statement a caller passes, and the pg twin's extended protocol has no
//!   such restriction. (Side effect: a multi-statement string would run here,
//!   where pg rejects it.)
//! - Those `raw_sql` calls are written in RECEIVER form -
//!   `conn.execute(sqlx::raw_sql(stmt))` through `sqlx::Executor`, never
//!   `sqlx::raw_sql(stmt).execute(conn)`: the inherent `RawSql::execute`
//!   folds the query borrow and the executor lifetime into one bound
//!   (`'q: 'e, E: Executor<'e>`), and that region goal fails to generalize
//!   when the future is boxed into the trait `BoxFuture`s (rustc: "implementation
//!   of `Executor` is not general enough"). `Executor::execute` keeps the two
//!   lifetimes apart (`'e, 'q: 'e`, `E: Execute<'q, ..>`) and compiles.
//! - `TxHandle::exec` counts `rows_affected()` the same as pg: sqlx-mysql
//!   requests CLIENT_FOUND_ROWS, so UPDATE counts matched rows (rule 16).

use kelson_foundation::AppError;
use kelson_foundation::Retryability;
use kelson_transaction::{BoxFuture, IsolationLevel, TransactionPort, TxHandle, TxOptions};
use sqlx::mysql::MySqlPool;
use sqlx::Executor;

/// MySQL SQLSTATEs that mean "this transaction lost a race; run it again":
/// `40001` is `ER_LOCK_DEADLOCK` (1213). MySQL's lock-wait timeout (1205) reports the
/// generic `HY000`, so it is NOT distinguishable through the SQLSTATE and is treated
/// as fatal (fail fast) — the lock wait is a symptom the caller should see, not hide
/// behind a silent retry.
pub fn is_retryable_transaction_error(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Database(db) => matches!(db.code().as_deref(), Some("40001")),
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

pub struct MySqlTransactions {
    pool: MySqlPool,
}

impl MySqlTransactions {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

pub struct MySqlTxHandle {
    tx: Option<sqlx::Transaction<'static, sqlx::MySql>>,
}

/// `TxHandle::exec` body: run one caller statement over COM_QUERY
/// (`sqlx::raw_sql` binds nothing, so sqlx takes the simple path - the
/// prepared-statement protocol would refuse any caller statement outside
/// MySQL's preparable list, module docs) on the transaction connection and
/// return `rows_affected` (CLIENT_FOUND_ROWS counts matched rows - the same
/// contract as the pg twin, negotiated at connect time, independent of
/// protocol).
async fn raw_exec(
    conn: &mut sqlx::mysql::MySqlConnection,
    sql: &str,
) -> Result<u64, AppError> {
    // Receiver form via `sqlx::Executor` - see the module docs note on why
    // `raw_sql(sql).execute(conn)` cannot be boxed into a `BoxFuture`.
    let result = conn
        .execute(sqlx::raw_sql(sql))
        .await
        .map_err(|e| tx_error("exec", &e))?;
    Ok(result.rows_affected())
}

/// Whole body of [`TxHandle::exec`] as one async fn, so the boxed trait
/// method is a single opaque instantiation (the `raw_sql` receiver-form
/// constraint lives in the module docs).
async fn exec_impl(handle: &mut MySqlTxHandle, sql: &str) -> Result<u64, AppError> {
    let tx = handle
        .tx
        .as_mut()
        .ok_or_else(|| AppError::internal("tx already completed"))?;
    raw_exec(tx, sql).await
}

impl TxHandle for MySqlTxHandle {
    fn exec<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<u64, AppError>> {
        Box::pin(exec_impl(self, sql))
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

/// Acquire a pooled connection, set the transaction characteristics BEFORE
/// BEGIN (MySQL requirement - ERROR 1568, module docs) on the same
/// connection the transaction will run on, then start the transaction.
/// `options.statement_timeout_ms` is dropped: MySQL has no per-statement
/// timeout (the session-scoped SELECT-only `MAX_EXECUTION_TIME` would leak
/// a kill timer onto every pooled connection) - documented divergence.
async fn begin_tx(
    pool: &MySqlPool,
    options: &TxOptions,
) -> Result<sqlx::Transaction<'static, sqlx::MySql>, AppError> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| tx_error("begin", &e))?;

    // Isolation must be set before the first statement of the transaction -
    // on MySQL before BEGIN itself, on the same connection the transaction
    // will run on.
    let level = match options.isolation {
        IsolationLevel::Serializable => "SERIALIZABLE",
        IsolationLevel::RepeatableRead => "REPEATABLE READ",
        // Explicit: MySQL's own default is REPEATABLE READ while the
        // kernel default is READ COMMITTED.
        IsolationLevel::ReadCommitted => "READ COMMITTED",
    };
    let access = if options.readonly {
        "READ ONLY"
    } else {
        "READ WRITE"
    };
    // `raw_sql` binds nothing, so sqlx takes the COM_QUERY (simple)
    // path - `SET TRANSACTION` is not preparable on MySQL (module
    // docs), and this must run on the same connection the BEGIN uses.
    // Receiver form via `sqlx::Executor` - see the module docs note on
    // why `raw_sql(...).execute(conn)` cannot be boxed into a `BoxFuture`.
    let set = format!("SET TRANSACTION ISOLATION LEVEL {level}, {access}");
    (&mut *conn)
        .execute(sqlx::raw_sql(&set))
        .await
        .map_err(|e| tx_error("isolation", &e))?;

    let tx: sqlx::Transaction<'static, sqlx::MySql> =
        sqlx::Transaction::begin(conn, None)
            .await
            .map_err(|e| tx_error("begin", &e))?;
    Ok(tx)
}

/// Whole body of [`TransactionPort::begin`] as one async fn: the boxed
/// trait method awaits it with ZERO awaits of its own (see `exec_impl`).
async fn begin_impl(
    pool: &MySqlPool,
    options: &TxOptions,
) -> Result<Box<dyn TxHandle>, AppError> {
    let tx = begin_tx(pool, options).await?;
    let handle: Box<dyn TxHandle> = Box::new(MySqlTxHandle { tx: Some(tx) });
    Ok(handle)
}

impl TransactionPort for MySqlTransactions {
    fn begin<'a>(
        &'a self,
        options: &'a TxOptions,
    ) -> BoxFuture<'a, Result<Box<dyn TxHandle>, AppError>> {
        Box::pin(begin_impl(&self.pool, options))
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

    /// A deadlock is a lost race: the whole transaction can be run again.
    #[test]
    fn deadlock_is_retryable() {
        assert!(is_retryable_transaction_error(&db_error("40001")));
    }

    /// Everything else is fatal — re-running it cannot help.
    #[test]
    fn other_database_failures_are_fatal() {
        assert!(!is_retryable_transaction_error(&db_error("23000")));
        assert!(!is_retryable_transaction_error(&db_error("HY000")));
        assert!(!is_retryable_transaction_error(&sqlx::Error::RowNotFound));
    }

    /// The mapping carries the classification into the AppError, so the retry
    /// helper can act on it.
    #[test]
    fn a_lost_race_maps_to_a_retryable_app_error() {
        let lost = tx_error("exec", &db_error("40001"));
        assert_eq!(lost.retryable, Retryability::Retryable);
        let fatal = tx_error("exec", &db_error("23000"));
        assert_eq!(fatal.retryable, Retryability::NotRetryable);
    }
}
