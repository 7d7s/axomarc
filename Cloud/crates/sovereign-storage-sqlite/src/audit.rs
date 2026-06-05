// Audit log writes and queries. The `audit_event` table is append-only;
// the `audit_no_update` and `audit_no_delete` triggers will reject any
// non-INSERT statement (the `append_audit` helper in `app.rs` is the
// only writer; the queries here are read-only).

use sqlx::{Pool, Row, Sqlite};
use tracing::instrument;

use sovereign_core::domain::{AuditEvent, AuditKind, AuditQuery, Timestamp};
use sovereign_core::error::AppError;

pub(crate) async fn append(pool: &Pool<Sqlite>, event: AuditEvent) -> Result<(), AppError> {
    let payload = serde_json::to_string(&event.payload)
        .map_err(|e| AppError::internal(format!("audit payload serialize: {e}")))?;
    let policy = match &event.policy_decision {
        Some(v) => Some(
            serde_json::to_string(v)
                .map_err(|e| AppError::internal(format!("audit policy serialize: {e}")))?,
        ),
        None => None,
    };

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO audit_event (ts, actor, kind, target, payload, policy_decision) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(event.ts.as_secs())
    .bind(&event.actor)
    .bind(event.kind.as_str())
    .bind(event.target.as_deref())
    .bind(payload)
    .bind(policy)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[instrument(skip(pool, q), fields(kind = ?q.kind, actor = ?q.actor, target = ?q.target))]
pub(crate) async fn query(
    pool: &Pool<Sqlite>,
    q: &AuditQuery,
) -> Result<Vec<AuditEvent>, AppError> {
    // Build the dynamic WHERE clause from the non-None filters.
    let mut where_clauses: Vec<String> = Vec::new();
    if q.since.is_some() {
        where_clauses.push("ts >= ?".to_string());
    }
    if q.until.is_some() {
        where_clauses.push("ts < ?".to_string());
    }
    if q.actor.is_some() {
        where_clauses.push("actor = ?".to_string());
    }
    if q.kind.is_some() {
        where_clauses.push("kind = ?".to_string());
    }
    if q.target.is_some() {
        where_clauses.push("target = ?".to_string());
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };
    let limit = q.effective_limit() as i64;
    let sql = format!(
        "SELECT id, ts, actor, kind, target, payload, policy_decision \
         FROM audit_event {where_sql} \
         ORDER BY id DESC LIMIT ?"
    );

    let mut stmt = sqlx::query(&sql);
    if let Some(s) = q.since {
        stmt = stmt.bind(s.as_secs());
    }
    if let Some(u) = q.until {
        stmt = stmt.bind(u.as_secs());
    }
    if let Some(a) = &q.actor {
        stmt = stmt.bind(a);
    }
    if let Some(k) = &q.kind {
        stmt = stmt.bind(k.as_str());
    }
    if let Some(t) = &q.target {
        stmt = stmt.bind(t);
    }
    stmt = stmt.bind(limit);

    let rows = stmt.fetch_all(pool).await?;

    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        let id: i64 = row.get(0);
        let ts: i64 = row.get(1);
        let actor: String = row.get(2);
        let kind_str: String = row.get(3);
        let target: Option<String> = row.get(4);
        let payload_str: String = row.get(5);
        let policy_str: Option<String> = row.get(6);

        let kind = kind_from_str(&kind_str);
        let payload: serde_json::Value = serde_json::from_str(&payload_str)
            .map_err(|e| AppError::internal(format!("audit payload deserialize: {e}")))?;
        let policy_decision = match policy_str {
            Some(s) => Some(
                serde_json::from_str(&s)
                    .map_err(|e| AppError::internal(format!("audit policy deserialize: {e}")))?,
            ),
            None => None,
        };

        events.push(AuditEvent {
            id: Some(id),
            ts: Timestamp(ts),
            actor,
            kind,
            target,
            payload,
            policy_decision,
        });
    }
    Ok(events)
}

pub(crate) async fn count(pool: &Pool<Sqlite>) -> Result<i64, AppError> {
    let row = sqlx::query("SELECT COUNT(*) FROM audit_event")
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>(0))
}

fn kind_from_str(s: &str) -> AuditKind {
    match s {
        "app" => AuditKind::AppLifecycle,
        "deploy" => AuditKind::Deploy,
        "rollback" => AuditKind::Rollback,
        "secret" => AuditKind::SecretChange,
        "backup" => AuditKind::Backup,
        "domain" => AuditKind::Domain,
        "server" => AuditKind::Server,
        "user" => AuditKind::User,
        _ => AuditKind::System,
    }
}
