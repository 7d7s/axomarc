// Deployment writers: `begin`, `transition`. The state machine is
// enforced in `transition`; the SQL is a dumb setter. Per
// `docs/architecture.md` §3.3 the transition is the *only* code that
// writes `deployment.status`, and the audit append is in the same
// transaction as the status update.

use sqlx::{Pool, Sqlite};

use sovereign_core::domain::{
    kind, AuditEvent, Deployment, DeploymentEvent, DeploymentId, DeploymentStatus,
    NewDeployment, Timestamp,
};
use sovereign_core::error::AppError;

use crate::app::append_audit;
use crate::row;

const DEPLOYMENT_COLUMNS: &str = "id, app_id, image_ref, strategy, status, started_at, \
                                   finished_at, triggered_by, risk_score, policy_decision, \
                                   error, version";

pub(crate) async fn begin(
    pool: &Pool<Sqlite>,
    new: NewDeployment,
    actor: &str,
) -> Result<Deployment, AppError> {
    if new.image_ref.trim().is_empty() {
        return Err(AppError::validation("deployment image_ref must not be empty"));
    }
    let id = DeploymentId::generate();
    let now = Timestamp::now();
    let mut tx = pool.begin().await?;

    sqlx::query(&format!(
        "INSERT INTO deployment ({DEPLOYMENT_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?, NULL, NULL, 1)"
    ))
    .bind(id.as_uuid())
    .bind(new.app_id.as_uuid())
    .bind(&new.image_ref)
    .bind(new.strategy.as_str())
    .bind(DeploymentStatus::Pending.to_string())
    .bind(now.as_secs())
    .bind(&new.triggered_by)
    .bind(new.risk_score)
    .execute(&mut *tx)
    .await?;

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::DEPLOY_START,
            target: Some(format!("deployment:{id}")),
            payload: serde_json::json!({
                "app_id": new.app_id.to_string(),
                "image_ref": new.image_ref,
                "strategy": new.strategy.as_str(),
                "triggered_by": new.triggered_by,
            }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_deployment_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("deployment vanished after insert"))
}

pub(crate) async fn transition(
    pool: &Pool<Sqlite>,
    id: DeploymentId,
    event: DeploymentEvent,
    actor: &str,
) -> Result<Deployment, AppError> {
    let mut tx = pool.begin().await?;
    let now = Timestamp::now();

    // Load current row inside the transaction so we observe a
    // consistent state for the state-machine check.
    let current = row::select_deployment_by_id(&mut *tx, id).await?;
    let current = current.ok_or(AppError::not_found("deployment"))?;

    if !current.status.can_transition_to(event.to) {
        return Err(AppError::InvalidTransition(
            "deployment",
            current.status.to_string(),
            event.to.to_string(),
        ));
    }

    // Failed deployments need an error message. All other transitions
    // ignore whatever was passed.
    let error_value: Option<String> = if event.to == DeploymentStatus::Failed {
        event.error.clone()
    } else {
        None
    };

    // The terminal-ish states set `finished_at` so the doctor can show
    // deploy duration. The "post-commit" events (Healthy, Failed,
    // RolledBack) all set it.
    let finished_at = if matches!(
        event.to,
        DeploymentStatus::Healthy
            | DeploymentStatus::Failed
            | DeploymentStatus::RolledBack
    ) {
        Some(now)
    } else {
        None
    };

    let rows = sqlx::query(
        "UPDATE deployment SET status = ?, finished_at = ?, error = ?, \
         version = version + 1 WHERE id = ? AND version = ?",
    )
    .bind(event.to.to_string())
    .bind(finished_at.map(|t| t.as_secs()))
    .bind(error_value.as_deref())
    .bind(id.as_uuid())
    .bind(current.version)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if rows == 0 {
        // Should be unreachable since we just read the row, but be safe.
        return Err(AppError::Conflict("deployment", current.version));
    }

    // On Healthy, also bump `app.image_ref` so `app` reflects "the
    // last successfully deployed image." This is the only place
    // `app.image_ref` is mutated from a deployment.
    if event.to == DeploymentStatus::Healthy {
        sqlx::query("UPDATE app SET image_ref = ?, updated_at = ?, version = version + 1 \
                     WHERE id = ?")
            .bind(&current.image_ref)
            .bind(now.as_secs())
            .bind(current.app_id.as_uuid())
            .execute(&mut *tx)
            .await?;
    }

    let audit_kind = if event.to == DeploymentStatus::RolledBack {
        kind::ROLLBACK
    } else {
        kind::DEPLOY_TRANSITION
    };

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: audit_kind,
            target: Some(format!("deployment:{id}")),
            payload: serde_json::json!({
                "from": current.status.to_string(),
                "to": event.to.to_string(),
                "error": event.error,
            }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_deployment_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("deployment vanished after transition"))
}
