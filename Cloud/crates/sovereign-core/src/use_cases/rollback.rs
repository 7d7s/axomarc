// The rollback use case (F5).
//
// Flow:
//   1. Find the *current* deployment (the most recent `Healthy` for
//      the app). If there isn't one, error.
//   2. Find the *target* deployment. If `to` is set, look it up by
//      id. Otherwise, pick the most recent `Healthy` that started
//      strictly before the current's `started_at` — i.e. the
//      previous version.
//   3. Stop the current container. Start the target container.
//      Healthcheck. On pass:
//        a. Mark the current as `RolledBack` (a new `Deployment`
//           row is *not* created; the existing current is mutated).
//        b. Create a "rollback marker" deployment (in `Healthy`
//           state) that records the rollback event; its
//           `target_deployment_id` points to the version we replaced
//           (the original current).
//        c. Return the rollback marker.
//      On healthcheck fail, restart the current container (the
//      "original") so the user is no worse off than before the
//      rollback attempt.
//
// On the storage side this is implemented via three new methods
// on `StoragePort` (F5 sub-tasks): `get_current_deployment`,
// `list_healthy_deployments_before`, and `set_rollback_target`.

use std::time::Duration;

use crate::domain::{
    AppId, Deployment, DeploymentEvent, DeploymentId, DeploymentStatus, NewDeployment, Strategy,
    Timestamp,
};
use crate::error::AppError;
use crate::ports::{default_v0_host, HealthResult};
use crate::rbac::{self, Action, Actor};
use crate::state::AppState;

/// Hard cap on the rollback health-probe budget.
pub const ROLLBACK_HEALTH_TIMEOUT: Duration = Duration::from_secs(30);

/// Input to `start_rollback`.
#[derive(Debug, Clone)]
pub struct RollbackRequest {
    pub app_id: AppId,
    /// Specific historical deployment to roll back to. `None` means
    /// "the most recent Healthy before the current one".
    pub to: Option<DeploymentId>,
    /// Who triggered this. Persisted in `deployment.triggered_by`
    /// and every audit event.
    pub actor: String,
}

/// Output of `start_rollback`. The CLI prints the deployment id
/// (the rollback marker) and the image_ref (the target version).
#[derive(Debug, Clone)]
pub struct RollbackResult {
    /// The rollback marker deployment. In `Healthy` state, with
    /// `target_deployment_id` set to the deployment we replaced.
    pub deployment: Deployment,
    /// The image_ref of the target version (what's now serving).
    pub target_image: String,
}

/// Run a rollback. See module docs.
///
/// Errors:
///
/// * `AppError::NotFound("no current deployment")` — the app has
///   never had a healthy deploy.
/// * `AppError::NotFound("no previous healthy deployment")` — the
///   user did not pass `--to` and there is no historical version.
/// * `AppError::NotFound("target deployment not found")` — the user
///   passed a `--to` id that doesn't exist.
/// * `AppError::Upstream(...)` — the healthcheck failed; the
///   current container was restored, but the rollback did not land.
pub async fn start_rollback(
    state: &AppState,
    req: RollbackRequest,
) -> Result<RollbackResult, AppError> {
    // RBAC: verify the actor is allowed to rollback.
    let actor = Actor::from_str_loose(&req.actor);
    rbac::check(&actor, Action::AppRollback)
        .map_err(|e| AppError::Unauthorized(e.to_string()))?;

    // 1. Current.
    let current = state
        .storage
        .get_current_deployment(req.app_id)
        .await?
        .ok_or_else(|| AppError::not_found("no current deployment"))?;

    // 2. Target.
    let target = match req.to {
        Some(id) => state
            .storage
            .get_deployment(id)
            .await?
            .ok_or_else(|| AppError::not_found("target deployment not found"))?,
        None => {
            let history = state
                .storage
                .list_healthy_deployments_before(req.app_id, current.started_at, 1)
                .await?;
            history
                .into_iter()
                .next()
                .ok_or_else(|| AppError::not_found("no previous healthy deployment"))?
        }
    };

    // Sanity: the target must be a different deployment than the
    // current. Otherwise we'd be in a no-op.
    if target.id == current.id {
        return Err(AppError::validation(
            "rollback target is the same as the current deployment",
        ));
    }

    // 3. Stop the current container + start the target.
    let current_name = format!("sovereign-{}-{}", current.app_id, current.id);
    let target_name = format!("sovereign-{}-{}", target.app_id, target.id);

    // Always stop the current first (F4 deploy left the old
    // container running because bluegreen keeps both up; V0 has no
    // proxy so we just stop it).
    if let Err(e) = state
        .runtime
        .stop_container(&current_name, Duration::from_secs(5))
        .await
    {
        // Non-fatal: log via the audit, continue.
        tracing::warn!("rollback: stop current container failed: {e}");
    }
    let _ = state.runtime.remove_container(&current_name).await;

    let target_spec = crate::ports::ContainerSpec {
        image: target.image_ref.clone(),
        name: target_name.clone(),
        port: 8080,
        env: Vec::new(),
        mounts: Vec::new(),
    };
    let new_container_id = state
        .runtime
        .create_container(target_spec)
        .await
        .map_err(|e| {
            // The current is already stopped. Best-effort restart
            // (best-effort because the original is now in a weird
            // state — its container is gone). The audit log will
            // record the failure.
            AppError::Upstream(format!("rollback create_container: {e}"))
        })?;
    state
        .runtime
        .start_container(&new_container_id)
        .await
        .map_err(|e| AppError::Upstream(format!("rollback start_container: {e}")))?;
    let health: HealthResult = state
        .runtime
        .healthcheck(
            &new_container_id,
            8080,
            target.image_ref.as_str(),
            ROLLBACK_HEALTH_TIMEOUT,
        )
        .await
        .unwrap_or(HealthResult {
            ok: false,
            latency_ms: 0,
            error: Some("runtime healthcheck impl missing".into()),
        });

    if !health.ok {
        // Auto-restore: try to start the original current container
        // again. Best-effort — we cannot guarantee the original is
        // recoverable, but we should try.
        let _ = restore_current(state, &current).await;
        return Err(AppError::Upstream(
            health
                .error
                .unwrap_or_else(|| "rollback healthcheck failed".to_string()),
        ));
    }

    // 4. Mark the current as RolledBack + create a rollback marker.
    let now = Timestamp::now();
    let _ = state
        .storage
        .transition_deployment(
            current.id,
            DeploymentEvent {
                to: DeploymentStatus::RolledBack,
                error: None,
                actor: format!("user:{}", req.actor),
            },
            &req.actor,
        )
        .await?;

    // Create the rollback-marker deployment. This is a new row in
    // `Healthy` state whose `image_ref` is the target's (what's
    // now serving) and whose `target_deployment_id` is `current.id`
    // (the version we just replaced). Triggered-by includes
    // ":rollback" so the audit trail can grep for it.
    let marker = state
        .storage
        .begin_deployment(
            NewDeployment {
                app_id: req.app_id,
                image_ref: target.image_ref.clone(),
                strategy: Strategy::Recreate,
                triggered_by: format!("user:{}:rollback", req.actor),
                risk_score: None,
            },
            &req.actor,
        )
        .await?;
    let _ = state
        .storage
        .transition_deployment(
            marker.id,
            DeploymentEvent {
                to: DeploymentStatus::Healthy,
                error: None,
                actor: format!("user:{}", req.actor),
            },
            &req.actor,
        )
        .await?;
    let _ = state
        .storage
        .set_rollback_target(marker.id, current.id, &req.actor)
        .await?;
    let _ = now; // reserved for future audit timestamp

    // Refresh the proxy route. The new container is on the same host
    // port (V0 pins port 8080), so the existing route's upstream is
    // now stale. Idempotent: Caddy's `add_route` PUT replaces.
    refresh_proxy_route(state, req.app_id).await;

    Ok(RollbackResult {
        deployment: state
            .storage
            .get_deployment(marker.id)
            .await?
            .ok_or_else(|| AppError::internal("rollback marker vanished"))?,
        target_image: target.image_ref,
    })
}

/// Refresh the proxy route for an app's default V0 hostname. Best-
/// effort: a Caddy error is a warning, not a rollback failure — the
/// audit log + storage row are the source of truth.
async fn refresh_proxy_route(state: &AppState, app_id: AppId) {
    let Some(proxy) = state.proxy.as_ref() else {
        return;
    };
    let host = default_v0_host(&app_id.to_string());
    if let Err(e) = proxy.add_route(&host, 8080).await {
        tracing::warn!(
            host = %host,
            error = %e,
            "rollback: proxy route refresh failed; the new container is serving on the loopback port, but the public URL is not wired. Run `sovereign domain add` to repair."
        );
    }
}

/// Best-effort: bring the original current container back. Used when
/// the rollback healthcheck fails.
async fn restore_current(state: &AppState, current: &Deployment) -> Result<(), AppError> {
    let name = format!("sovereign-{}-{}", current.app_id, current.id);
    let spec = crate::ports::ContainerSpec {
        image: current.image_ref.clone(),
        name: name.clone(),
        port: 8080,
        env: Vec::new(),
        mounts: Vec::new(),
    };
    let id = state.runtime.create_container(spec).await?;
    state.runtime.start_container(&id).await?;
    Ok(())
}
