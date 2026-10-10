//! `TransactionPort` over `PgPool` (S1 split): plain transactions through the
//! kelson-transaction contracts — no execution-unit semantics here. The execution
//! backend (tx.rs) keeps its unit contract; the outbox pump / job store /
//! maintenance flows use this port directly.

use kelson_foundation::AppError;
use kelson_transaction::{BoxFuture, IsolationLevel, TransactionPort, TxHandle, TxOptions};
use sqlx::postgres::PgPool;

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
                .map_err(|e| AppError::persistence(format!("exec: {e}")))?;
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
                .map_err(|e| AppError::persistence(format!("commit: {e}")))
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
                .map_err(|e| AppError::persistence(format!("rollback: {e}")))
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
                .map_err(|e| AppError::persistence(format!("begin: {e}")))?;

            // Isolation must be set before the first statement of the transaction.
            match options.isolation {
                IsolationLevel::Serializable => {
                    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| AppError::persistence(format!("isolation: {e}")))?;
                }
                IsolationLevel::RepeatableRead => {
                    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| AppError::persistence(format!("isolation: {e}")))?;
                }
                IsolationLevel::ReadCommitted => {} // postgres default
            }
            if options.readonly {
                sqlx::query("SET TRANSACTION READ ONLY")
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| AppError::persistence(format!("readonly: {e}")))?;
            }
            if let Some(ms) = options.statement_timeout_ms {
                sqlx::query(&format!("SET LOCAL statement_timeout = {ms}"))
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| AppError::persistence(format!("statement_timeout: {e}")))?;
            }

            let handle: Box<dyn TxHandle> = Box::new(PgTxHandle { tx: Some(tx) });
            Ok(handle)
        })
    }
}
