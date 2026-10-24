//! Execution-level idempotency on MySQL 8 (D-51):
//! (tenant, operation, key) + payload hash -> stored result.
//!
//! MySQL port notes (B-31), vs the Postgres twin
//! (`kelson_postgres::store`):
//! - `tenant_id IS NOT DISTINCT FROM $1` becomes MySQL's null-safe `<=>`
//!   (rule 12): NULL tenant matches NULL tenant exactly like pg.
//! - `ON CONFLICT (tenant_id, operation, key) DO NOTHING` (rule 6)
//!   becomes `INSERT ... SELECT ... FROM DUAL WHERE NOT EXISTS` over
//!   EXACTLY that composite (null-safe `<=>` on the nullable tenant
//!   column): a collision on the same (tenant, operation, key) skips -
//!   0 rows inserted - while a collision on ANY OTHER key of the table
//!   still errors, which is precisely pg's conflict-target semantics.
//!   `rows_affected` is not consulted by the port (pg does not branch on
//!   it either), but the statement still yields 1 = inserted / 0 =
//!   skipped (rule 16).
//! - the `key` column is written `` `key` `` (backticked) everywhere:
//!   KEY is a RESERVED word in MySQL 8 (rule 17), so the identifier must
//!   be quoted; the column name itself is unchanged.
//! - no RLS and no `SET LOCAL kelson.tenant_id` (rule 11): on pg,
//!   `idempotency_keys` is guarded by the `tenant_isolation_idempotency`
//!   policy as a final containment barrier, but both statements already
//!   carried explicit `tenant_id` / `operation` / `key` predicates (the
//!   defense in depth) - those are kept verbatim, nothing was added or
//!   dropped, and on MySQL they are the whole isolation story. DDL note
//!   (not this port): pg's `PRIMARY KEY (tenant_id, operation, key)`
//!   implies `tenant_id NOT NULL`, so a NULL-tenant row cannot exist
//!   there; the `<=> NULL` guard reproduces the same skip-vs-insert
//!   decision whatever the MySQL mirror declares - if it mirrors the
//!   primary key (also NOT NULL) behavior is identical, and if it makes
//!   the column nullable under a unique key the guard still dedupes even
//!   though unique keys permit multiple NULLs.
//! - `now()` becomes `NOW(6)` (rule 5); uuid binds stay raw `BINARY(16)`
//!   (rule 2) and `result` stays a `serde_json::Value` over `JSON`
//!   (rule 4).

use kelson_execution::ports::{IdempotencyCheck, IdempotencyKey, IdempotencyPort};
use kelson_execution::BoxFuture;
use kelson_foundation::{AppError, OperationId, TenantId};
use serde_json::Value;
use sqlx::mysql::MySqlPool;
use uuid::Uuid;

pub struct MySqlIdempotency {
    pool: MySqlPool,
}

impl MySqlIdempotency {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

impl IdempotencyPort for MySqlIdempotency {
    fn check<'a>(
        &'a self,
        tenant: Option<TenantId>,
        operation: &'a OperationId,
        key: &'a IdempotencyKey,
        payload_hash: &'a str,
    ) -> BoxFuture<'a, Result<IdempotencyCheck, AppError>> {
        Box::pin(async move {
            let tenant_id = tenant.map(|t| *t.as_uuid());
            let row: Option<(String, Value)> = sqlx::query_as(
                "SELECT payload_hash, result FROM idempotency_keys
                 WHERE tenant_id <=> ? AND operation = ? AND `key` = ?",
            )
            .bind(tenant_id)
            .bind(operation.as_str())
            .bind(&key.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::persistence(format!("idempotency check: {e}")))?;
            Ok(match row {
                None => IdempotencyCheck::New,
                Some((hash, result)) if hash == payload_hash => IdempotencyCheck::Replay(result),
                Some(_) => IdempotencyCheck::HashConflict,
            })
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
            let tenant_id = tenant.map(|t| *t.as_uuid());
            sqlx::query(
                r"INSERT INTO idempotency_keys (id, tenant_id, operation, `key`, payload_hash, result, created_at)
                  SELECT ?, ?, ?, ?, ?, ?, NOW(6)
                  FROM DUAL
                  WHERE NOT EXISTS (
                      SELECT 1 FROM idempotency_keys
                      WHERE tenant_id <=> ? AND operation = ? AND `key` = ?
                  )",
            )
            .bind(Uuid::now_v7())
            .bind(tenant_id)
            .bind(operation.as_str())
            .bind(&key.0)
            .bind(payload_hash)
            .bind(result)
            .bind(tenant_id)
            .bind(operation.as_str())
            .bind(&key.0)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::persistence(format!("idempotency store: {e}")))?;
            Ok(())
        })
    }
}
