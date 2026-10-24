//! Execution-level idempotency on PostgreSQL (D-51):
//! (tenant, operation, key) + payload hash → stored result.

use kelson_execution::ports::{IdempotencyCheck, IdempotencyKey, IdempotencyPort};
use kelson_execution::BoxFuture;
use kelson_foundation::{AppError, OperationId, TenantId};
use serde_json::Value;
use sqlx::postgres::PgPool;
use uuid::Uuid;

pub struct PgIdempotency {
    pool: PgPool,
}

impl PgIdempotency {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl IdempotencyPort for PgIdempotency {
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
                 WHERE tenant_id IS NOT DISTINCT FROM $1 AND operation = $2 AND key = $3",
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
                r"INSERT INTO idempotency_keys (id, tenant_id, operation, key, payload_hash, result, created_at)
                  VALUES ($1,$2,$3,$4,$5,$6,now())
                  ON CONFLICT (tenant_id, operation, key) DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(tenant_id)
            .bind(operation.as_str())
            .bind(&key.0)
            .bind(payload_hash)
            .bind(result)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::persistence(format!("idempotency store: {e}")))?;
            Ok(())
        })
    }
}
