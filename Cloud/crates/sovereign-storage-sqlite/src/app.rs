// App writers: `create`, `update`, `archive`. All write paths write an
// audit event in the same transaction.

use sqlx::{Pool, Sqlite, Transaction};

use sovereign_core::domain::{
    kind, App, AppId, AppStatus, AppUpdate, AuditEvent, NewApp, Timestamp,
};
use sovereign_core::error::AppError;

use crate::row;

const APP_COLUMNS: &str = "id, name, owner, env, git_repo, image_ref, config_yaml, \
                            health_path, status, created_at, updated_at, version";

pub(crate) async fn create(
    pool: &Pool<Sqlite>,
    new: NewApp,
    actor: &str,
) -> Result<App, AppError> {
    if new.name.trim().is_empty() {
        return Err(AppError::validation("app name must not be empty"));
    }
    if new.config_yaml.is_empty() {
        return Err(AppError::validation("app config_yaml must not be empty"));
    }

    let id = AppId::generate();
    let now = Timestamp::now();
    let mut tx = pool.begin().await?;

    let result = sqlx::query(&format!(
        "INSERT INTO app ({APP_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    ))
    .bind(id.as_uuid())
    .bind(&new.name)
    .bind(&new.owner)
    .bind(new.env.as_str())
    .bind(new.git_repo.as_deref())
    .bind(new.image_ref.as_deref())
    .bind(&new.config_yaml)
    .bind(new.health_path.as_deref())
    .bind(AppStatus::Active.to_string())
    .bind(now.as_secs())
    .bind(now.as_secs())
    .bind(1_i64)
    .execute(&mut *tx)
    .await;

    if let Err(e) = result {
        return Err(map_insert_err(e, &new.name));
    }

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::APP_CREATE,
            target: Some(format!("app:{}", new.name)),
            payload: serde_json::json!({
                "name": new.name,
                "env": new.env.as_str(),
                "owner": new.owner,
            }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_app_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("app vanished after insert"))
}

pub(crate) async fn update(
    pool: &Pool<Sqlite>,
    id: AppId,
    update: AppUpdate,
    expected_version: i64,
    actor: &str,
) -> Result<App, AppError> {
    let mut tx = pool.begin().await?;
    let now = Timestamp::now();

    // Build a dynamic SET clause from the non-None fields. Each setter
    // corresponds to a field on `AppUpdate`. An `Option<Option<T>>` (the
    // "set to NULL" pattern) is encoded by passing `NULL` to the binder.
    let mut sets: Vec<&'static str> = Vec::new();
    if update.owner.is_some() {
        sets.push("owner = ?");
    }
    if update.env.is_some() {
        sets.push("env = ?");
    }
    if update.git_repo.is_some() {
        sets.push("git_repo = ?");
    }
    if update.image_ref.is_some() {
        sets.push("image_ref = ?");
    }
    if update.config_yaml.is_some() {
        sets.push("config_yaml = ?");
    }
    if update.health_path.is_some() {
        sets.push("health_path = ?");
    }
    if update.status.is_some() {
        sets.push("status = ?");
    }
    sets.push("updated_at = ?");
    sets.push("version = version + 1");

    let sql = format!(
        "UPDATE app SET {} WHERE id = ? AND version = ?",
        sets.join(", ")
    );

    let mut q = sqlx::query(&sql);
    if let Some(v) = &update.owner {
        q = q.bind(v);
    }
    if let Some(v) = update.env {
        q = q.bind(v.as_str());
    }
    if let Some(v) = &update.git_repo {
        q = q.bind(v.as_deref());
    }
    if let Some(v) = &update.image_ref {
        q = q.bind(v.as_deref());
    }
    if let Some(v) = &update.config_yaml {
        q = q.bind(v);
    }
    if let Some(v) = &update.health_path {
        q = q.bind(v.as_deref());
    }
    if let Some(v) = update.status {
        q = q.bind(v.to_string());
    }
    q = q.bind(now.as_secs());
    q = q.bind(id.as_uuid());
    q = q.bind(expected_version);

    let rows = q.execute(&mut *tx).await?.rows_affected();
    if rows == 0 {
        // Distinguish "not found" from "version mismatch".
        let current = row::select_app_by_id(&mut *tx, id).await?;
        return Err(match current {
            None => AppError::not_found("app"),
            Some(a) => AppError::Conflict("app", a.version),
        });
    }

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::APP_UPDATE,
            target: Some(format!("app:{id}")),
            payload: serde_json::json!({
                "id": id.to_string(),
                "expected_version": expected_version,
                "new_version": expected_version + 1,
                "changed": sets.iter().filter(|s| !s.starts_with("updated_at") && !s.starts_with("version")).copied().collect::<Vec<_>>(),
            }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_app_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("app vanished after update"))
}

pub(crate) async fn archive(
    pool: &Pool<Sqlite>,
    id: AppId,
    expected_version: i64,
    actor: &str,
) -> Result<App, AppError> {
    let mut tx = pool.begin().await?;
    let now = Timestamp::now();

    // Archive: status -> archived. We keep the same OCC contract.
    let rows = sqlx::query(
        "UPDATE app SET status = ?, updated_at = ?, version = version + 1 \
         WHERE id = ? AND version = ?",
    )
    .bind(AppStatus::Archived.to_string())
    .bind(now.as_secs())
    .bind(id.as_uuid())
    .bind(expected_version)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if rows == 0 {
        let current = row::select_app_by_id(&mut *tx, id).await?;
        return Err(match current {
            None => AppError::not_found("app"),
            Some(a) => AppError::Conflict("app", a.version),
        });
    }

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::APP_DELETE,
            target: Some(format!("app:{id}")),
            payload: serde_json::json!({
                "id": id.to_string(),
                "expected_version": expected_version,
            }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_app_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("app vanished after archive"))
}

fn map_insert_err(e: sqlx::Error, name: &str) -> AppError {
    if let sqlx::Error::Database(db) = &e {
        if db.message().contains("UNIQUE") && db.message().contains("name") {
            return AppError::validation(format!("app name `{name}` already exists"));
        }
    }
    AppError::from(e)
}

// Audit helper, shared by all writers in this crate. Lives here (not in
// `audit.rs`) so the writer can borrow the active transaction without
// a borrow conflict on `pool`.
pub(crate) async fn append_audit(
    tx: &mut Transaction<'_, Sqlite>,
    event: AuditEvent,
) -> Result<(), AppError> {
    let payload = serde_json::to_string(&event.payload)
        .map_err(|e| AppError::internal(format!("audit payload serialize: {e}")))?;
    let policy = match &event.policy_decision {
        Some(v) => Some(
            serde_json::to_string(v)
                .map_err(|e| AppError::internal(format!("audit policy serialize: {e}")))?,
        ),
        None => None,
    };
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
    .execute(&mut **tx)
    .await?;
    Ok(())
}
