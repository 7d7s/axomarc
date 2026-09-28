//! Delta persistence. Monotonic per (tenant, stream) once per-partition
//! sequences land (M3-05); skeleton uses one global sequence.
use kernel_core::Delta;

#[derive(Debug, thiserror::Error)]
#[error("delta write failed: {0}")]
pub struct DeltaWriteError(#[from] pub sqlx::Error);

pub async fn write<'e, E>(conn: E, d: &Delta) -> Result<i64, DeltaWriteError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let state = serde_json::to_value(&d.state).unwrap_or(serde_json::Value::Null);
    let changed = serde_json::to_value(&d.changed).unwrap_or(serde_json::Value::Null);
    let row: (i64,) = sqlx::query_as(
        r"INSERT INTO deltas
            (tenant_id, stream, aggregate, aggregate_id, version, action,
             state, changed, owner_path, actor_id, capability_grant_id, causation_id)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
          RETURNING seq",
    )
    .bind(d.tenant_id)
    .bind(d.stream)
    .bind(d.aggregate)
    .bind(d.aggregate_id)
    .bind(d.version)
    .bind(d.action.as_str())
    .bind(state)
    .bind(changed)
    .bind(&d.owner_path)
    .bind(d.actor_id)
    .bind(d.capability_grant_id)
    .bind(d.causation_id)
    .fetch_one(conn)
    .await?;
    Ok(row.0)
}