// Read helpers — one `select_*` per logical query. These return either
// `Option<Entity>` or `Vec<Entity>`. The `from_row` impls live in the
// domain layer (via `sqlx::FromRow`).
//
// Every helper accepts `impl Executor<'_, Database = Sqlite>`. This lets
// the writers pass either `&pool` (for reads outside a transaction) or
// `&mut *tx` (for reads inside an in-progress transaction).

use sqlx::Executor;
use sqlx::Sqlite;

use sovereign_core::domain::{
    App, AppId, Backup, BackupId, Deployment, DeploymentId, Domain, Secret, SecretId, Server,
    ServerId, User, UserId,
};
use sovereign_core::error::AppError;

pub(crate) async fn select_app_by_id<'e, E>(exec: E, id: AppId) -> Result<Option<App>, AppError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, App>(
        "SELECT id, name, owner, env, git_repo, image_ref, config_yaml, \
         health_path, status, created_at, updated_at, version, \
         deploy_mode, source_repo, source_branch, auto_deploy, \
         auto_deploy_window, max_auto_deploys_per_hour \
         FROM app WHERE id = ?",
    )
    .bind(id.as_uuid())
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

pub(crate) async fn select_app_by_name<'e, E>(exec: E, name: &str) -> Result<Option<App>, AppError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, App>(
        "SELECT id, name, owner, env, git_repo, image_ref, config_yaml, \
         health_path, status, created_at, updated_at, version, \
         deploy_mode, source_repo, source_branch, auto_deploy, \
         auto_deploy_window, max_auto_deploys_per_hour \
         FROM app WHERE name = ?",
    )
    .bind(name)
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

pub(crate) async fn select_all_apps<'e, E>(exec: E) -> Result<Vec<App>, AppError>
where
    E: Executor<'e, Database = Sqlite> + Copy,
{
    let rows = sqlx::query_as::<_, App>(
        "SELECT id, name, owner, env, git_repo, image_ref, config_yaml, \
         health_path, status, created_at, updated_at, version, \
         deploy_mode, source_repo, source_branch, auto_deploy, \
         auto_deploy_window, max_auto_deploys_per_hour \
         FROM app ORDER BY created_at ASC, name ASC",
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

pub(crate) async fn select_apps_by_owner<'e, E>(exec: E, owner: &str) -> Result<Vec<App>, AppError>
where
    E: Executor<'e, Database = Sqlite> + Copy,
{
    let rows = sqlx::query_as::<_, App>(
        "SELECT id, name, owner, env, git_repo, image_ref, config_yaml, \
         health_path, status, created_at, updated_at, version, \
         deploy_mode, source_repo, source_branch, auto_deploy, \
         auto_deploy_window, max_auto_deploys_per_hour \
         FROM app WHERE owner = ? ORDER BY created_at ASC, name ASC",
    )
    .bind(owner)
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

pub(crate) async fn select_deployment_by_id<'e, E>(
    exec: E,
    id: DeploymentId,
) -> Result<Option<Deployment>, AppError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, Deployment>(
        "SELECT id, app_id, image_ref, strategy, status, started_at, finished_at, \
         triggered_by, risk_score, policy_decision, error, target_deployment_id, version \
         FROM deployment WHERE id = ?",
    )
    .bind(id.as_uuid())
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

pub(crate) async fn select_deployments_by_app<'e, E>(
    exec: E,
    app_id: AppId,
    limit: i64,
) -> Result<Vec<Deployment>, AppError>
where
    E: Executor<'e, Database = Sqlite> + Copy,
{
    let rows = sqlx::query_as::<_, Deployment>(
        "SELECT id, app_id, image_ref, strategy, status, started_at, finished_at, \
         triggered_by, risk_score, policy_decision, error, target_deployment_id, version \
         FROM deployment WHERE app_id = ? \
         ORDER BY started_at DESC, id DESC \
         LIMIT ?",
    )
    .bind(app_id.as_uuid())
    .bind(limit)
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Most recent `Healthy` deployment for the app, if any. This is
/// "what is currently serving" (F5's `get_current_deployment`).
pub(crate) async fn select_current_deployment<'e, E>(
    exec: E,
    app_id: AppId,
) -> Result<Option<Deployment>, AppError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, Deployment>(
        "SELECT id, app_id, image_ref, strategy, status, started_at, finished_at, \
         triggered_by, risk_score, policy_decision, error, target_deployment_id, version \
         FROM deployment \
         WHERE app_id = ? AND status = 'healthy' \
         ORDER BY started_at DESC, id DESC \
         LIMIT 1",
    )
    .bind(app_id.as_uuid())
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// `Healthy` deployments for the app that started strictly before
/// `before_ts`, newest first, capped at `limit`. Used by rollback to
/// find the previous version.
pub(crate) async fn select_healthy_before<'e, E>(
    exec: E,
    app_id: AppId,
    before_ts: i64,
    limit: i64,
) -> Result<Vec<Deployment>, AppError>
where
    E: Executor<'e, Database = Sqlite> + Copy,
{
    let rows = sqlx::query_as::<_, Deployment>(
        "SELECT id, app_id, image_ref, strategy, status, started_at, finished_at, \
         triggered_by, risk_score, policy_decision, error, target_deployment_id, version \
         FROM deployment \
         WHERE app_id = ? AND status = 'healthy' AND started_at < ? \
         ORDER BY started_at DESC, id DESC \
         LIMIT ?",
    )
    .bind(app_id.as_uuid())
    .bind(before_ts)
    .bind(limit)
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

pub(crate) async fn select_domains_by_app<'e, E>(
    exec: E,
    app_id: AppId,
) -> Result<Vec<sovereign_core::domain::Domain>, AppError>
where
    E: Executor<'e, Database = Sqlite> + Copy,
{
    let rows = sqlx::query_as::<_, Domain>(
        "SELECT id, app_id, hostname, tls_status, tls_expires, created_at \
         FROM domain WHERE app_id = ? ORDER BY created_at ASC, hostname ASC",
    )
    .bind(app_id.as_uuid())
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

pub(crate) async fn select_secret_by_id<'e, E>(
    exec: E,
    id: SecretId,
) -> Result<Option<Secret>, AppError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, Secret>(
        "SELECT id, app_id, key, ciphertext, status, created_at, rotated_at, version \
         FROM secret WHERE id = ?",
    )
    .bind(id.as_uuid())
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

pub(crate) async fn select_secrets_by_app<'e, E>(
    exec: E,
    app_id: AppId,
) -> Result<Vec<Secret>, AppError>
where
    E: Executor<'e, Database = Sqlite> + Copy,
{
    let rows = sqlx::query_as::<_, Secret>(
        "SELECT id, app_id, key, ciphertext, status, created_at, rotated_at, version \
         FROM secret WHERE app_id = ? ORDER BY key ASC",
    )
    .bind(app_id.as_uuid())
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

pub(crate) async fn select_server_by_id<'e, E>(
    exec: E,
    id: ServerId,
) -> Result<Option<Server>, AppError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, Server>(
        "SELECT id, hostname, role, api_url, status, last_seen, cpu_cores, \
         mem_mb, disk_gb, joined_at \
         FROM server WHERE id = ?",
    )
    .bind(id.as_uuid())
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

pub(crate) async fn select_all_servers<'e, E>(exec: E) -> Result<Vec<Server>, AppError>
where
    E: Executor<'e, Database = Sqlite> + Copy,
{
    let rows = sqlx::query_as::<_, Server>(
        "SELECT id, hostname, role, api_url, status, last_seen, cpu_cores, \
         mem_mb, disk_gb, joined_at \
         FROM server ORDER BY joined_at ASC, hostname ASC",
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

pub(crate) async fn select_backup_by_id<'e, E>(
    exec: E,
    id: BackupId,
) -> Result<Option<Backup>, AppError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, Backup>(
        "SELECT id, target, status, size_bytes, location, started_at, \
         finished_at, verified_at, verify_result, error \
         FROM backup WHERE id = ?",
    )
    .bind(id.as_uuid())
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

pub(crate) async fn select_recent_backups<'e, E>(
    exec: E,
    limit: i64,
) -> Result<Vec<Backup>, AppError>
where
    E: Executor<'e, Database = Sqlite> + Copy,
{
    let rows = sqlx::query_as::<_, Backup>(
        "SELECT id, target, status, size_bytes, location, started_at, \
         finished_at, verified_at, verify_result, error \
         FROM backup ORDER BY started_at DESC, id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

pub(crate) async fn select_user_by_id<'e, E>(exec: E, id: UserId) -> Result<Option<User>, AppError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, User>(
        "SELECT id, email, role, created_at, last_seen FROM user WHERE id = ?",
    )
    .bind(id.as_uuid())
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

pub(crate) async fn select_user_by_email<'e, E>(
    exec: E,
    email: &str,
) -> Result<Option<User>, AppError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, User>(
        "SELECT id, email, role, created_at, last_seen FROM user WHERE email = ?",
    )
    .bind(email)
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

pub(crate) async fn select_all_users<'e, E>(exec: E) -> Result<Vec<User>, AppError>
where
    E: Executor<'e, Database = Sqlite> + Copy,
{
    let rows = sqlx::query_as::<_, User>(
        "SELECT id, email, role, created_at, last_seen FROM user ORDER BY email ASC",
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}
