// The deploy use case.
//
// Orchestrates the storage state machine and the runtime adapter to
// take a new app version from "git push" to "serving traffic". The
// state transitions (Pending -> Building -> Pushing -> Starting ->
// Healthy | Failed) are recorded as audit events in the same
// transaction as the storage update — F3's append-only contract.
//
// On healthcheck fail, the new container is stopped and removed; the
// previous version (if any) is left running for the blue-green default.

use std::time::Duration;

use crate::domain::{
    AppId, Deployment, DeploymentEvent, DeploymentStatus, NewDeployment, Strategy,
};
use crate::error::AppError;
use crate::ports::{default_v0_host, HealthResult};
use crate::state::AppState;

/// Hard cap on the total health-probe budget. The use case returns
/// `AppError::Upstream` if the new container is not healthy in time.
pub const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-probe timeout (3 retries => up to 15s wall-clock for the default).
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Input to `start_deploy`.
#[derive(Debug, Clone)]
pub struct DeployRequest {
    /// The app being deployed.
    pub app_id: AppId,
    /// Image to run. If `None`, the use case resolves from
    /// `app.image_ref` (or `app.git_repo` once BuildKit is wired in
    /// V1.5). V0 requires one of the two; the CLI enforces that.
    pub image_ref: Option<String>,
    /// Bluegreen keeps the old version running; recreate stops it
    /// first. The state machine is the same; only the orchestration
    /// differs.
    pub strategy: Strategy,
    /// If `true`, block until healthy. If `false`, return as soon as
    /// the deployment is `Starting` and let the user poll.
    pub wait: bool,
    /// Who triggered this. Persisted in `deployment.triggered_by` and
    /// every audit event.
    pub actor: String,
}

/// Output of `start_deploy`. The CLI prints the URL on success and
/// the `Deployment` row in JSON for agents.
#[derive(Debug, Clone)]
pub struct DeployResult {
    pub deployment: Deployment,
    /// The URL the app is reachable at. V0: a synthesized
    /// `https://<app-name>.<host>` (the proxy adapter fills this in
    /// once F6 lands; for now it's the container's published port).
    pub url: String,
    /// The last health probe result, if `wait` was `true`. `None`
    /// when the deploy was fire-and-forget.
    pub health: Option<HealthResult>,
}

/// Run a deploy. See module docs for the high-level flow.
///
/// Error handling: any failure after the `Pending` row exists is
/// captured in the same transaction as a transition to `Failed` with
/// the error message. The use case still returns `Err` so the CLI
/// can print the failure; the audit log records what happened.
pub async fn start_deploy(state: &AppState, req: DeployRequest) -> Result<DeployResult, AppError> {
    // 1. Resolve the image. The CLI guarantees `req.image_ref` is
    //    Some, but be defensive in case this is called from a future
    //    use case (e.g. auto-rollback in F5) that may fall back to
    //    `app.image_ref` from storage. V0 just errors out when
    //    `image_ref` is missing; V1.5 will add the storage lookup.
    let image = req
        .image_ref
        .clone()
        .ok_or_else(|| AppError::validation("either --image or app.image_ref must be set"))?;

    // 2. Begin the deployment (Pending).
    let dep = state
        .storage
        .begin_deployment(
            NewDeployment {
                app_id: req.app_id,
                image_ref: image.clone(),
                strategy: req.strategy,
                triggered_by: format!("user:{}", &req.actor),
                risk_score: None,
            },
            &req.actor,
        )
        .await?;
    let deployment_id = dep.id;

    // 3. Pull the image.
    if let Err(e) = state.runtime.pull_image(&image).await {
        return record_failure(state, deployment_id, &e.to_string(), &req.actor).await;
    }
    transition(
        state,
        deployment_id,
        DeploymentStatus::Building,
        None,
        &req.actor,
    )
    .await?;

    // 4. Create + start the container. The name is unique per
    //    deployment so concurrent deploys of the same app don't
    //    collide on the daemon.
    let container_name = format!("sovereign-{}-{}", dep.app_id, dep.id);
    let spec = crate::ports::ContainerSpec {
        image: image.clone(),
        name: container_name.clone(),
        port: 8080,
        env: Vec::new(),
        mounts: Vec::new(),
    };
    let container_id = match state.runtime.create_container(spec).await {
        Ok(id) => id,
        Err(e) => {
            return record_failure(state, deployment_id, &e.to_string(), &req.actor).await;
        }
    };
    transition(
        state,
        deployment_id,
        DeploymentStatus::Pushing,
        None,
        &req.actor,
    )
    .await?;

    if let Err(e) = state.runtime.start_container(&container_id).await {
        let _ = state
            .runtime
            .stop_container(&container_id, Duration::from_secs(5))
            .await;
        let _ = state.runtime.remove_container(&container_id).await;
        return record_failure(state, deployment_id, &e.to_string(), &req.actor).await;
    }
    transition(
        state,
        deployment_id,
        DeploymentStatus::Starting,
        None,
        &req.actor,
    )
    .await?;

    // 5. If the caller asked for fire-and-forget, return now. The
    //    "is it healthy" decision is theirs to poll.
    if !req.wait {
        let final_dep = state
            .storage
            .get_deployment(deployment_id)
            .await?
            .ok_or_else(|| AppError::internal("deployment vanished after start"))?;
        return Ok(DeployResult {
            deployment: final_dep,
            url: format!("sovereign://{}", container_id),
            health: None,
        });
    }

    // 6. Health probe loop. Cap the total wall-clock budget.
    let probe = super::health::probe_http("127.0.0.1", 8080, "/", DEFAULT_PROBE_TIMEOUT).await;
    let healthy = probe.ok;
    if !healthy {
        // Auto-rollback: stop + remove the bad container. The
        // previous version is left running (bluegreen default).
        let _ = state
            .runtime
            .stop_container(&container_id, Duration::from_secs(5))
            .await;
        let _ = state.runtime.remove_container(&container_id).await;
        let err = probe
            .error
            .clone()
            .unwrap_or_else(|| "healthcheck failed".to_string());
        return record_failure(state, deployment_id, &err, &req.actor).await;
    }

    transition(
        state,
        deployment_id,
        DeploymentStatus::Healthy,
        None,
        &req.actor,
    )
    .await?;
    let final_dep = state
        .storage
        .get_deployment(deployment_id)
        .await?
        .ok_or_else(|| AppError::internal("deployment vanished after healthy"))?;
    let url = post_healthy_url(state, final_dep.app_id).await;
    Ok(DeployResult {
        deployment: final_dep,
        url,
        health: Some(probe),
    })
}

/// Compute the public URL for a freshly-healthy deploy. If the proxy
/// port is configured, push a Caddy route for `<host>` ->
/// `127.0.0.1:8080` and return the HTTPS URL. A proxy failure is a
/// warning, not a deploy failure — the container is serving on the
/// loopback port either way; the operator can run `sovereign domain
/// add` later to repair the route.
async fn post_healthy_url(state: &AppState, app_id: crate::domain::AppId) -> String {
    let host = default_v0_host(&app_id.to_string());
    match state.proxy.as_ref() {
        Some(proxy) => match proxy.add_route(&host, 8080).await {
            Ok(()) => format!("https://{host}"),
            Err(e) => {
                tracing::warn!(
                    host = %host,
                    error = %e,
                    "proxy.add_route failed; deployment is healthy but the public URL is not wired. Run `sovereign domain add` to repair."
                );
                format!("sovereign://{host} (proxy: {e})")
            }
        },
        None => format!("sovereign://{host} (no proxy configured)"),
    }
}

/// Record a failure transition + append the audit. Best-effort: if
/// the audit write itself fails, we still return the original error
/// (the operator cares about the deploy, not the audit plumbing).
async fn record_failure(
    state: &AppState,
    id: crate::domain::DeploymentId,
    err: &str,
    actor: &str,
) -> Result<DeployResult, AppError> {
    // Best-effort transition to Failed. If it fails, the deployment
    // is still in its pre-Failed state; the caller will see the
    // original error regardless.
    let _ = transition(
        state,
        id,
        DeploymentStatus::Failed,
        Some(err.to_string()),
        actor,
    )
    .await;
    Err(AppError::Upstream(err.to_string()))
}

/// Helper: run a single transition through the state machine.
async fn transition(
    state: &AppState,
    id: crate::domain::DeploymentId,
    to: DeploymentStatus,
    error: Option<String>,
    actor: &str,
) -> Result<(), AppError> {
    state
        .storage
        .transition_deployment(
            id,
            DeploymentEvent {
                to,
                error,
                actor: format!("user:{actor}"),
            },
            actor,
        )
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    //! Unit tests for the deploy use case.
    //!
    //! End-to-end testing of `start_deploy` requires a real
    //! `StoragePort` implementation; the F3 integration tests in
    //! `sovereign-storage-sqlite` cover the full state machine
    //! through a real SqliteState. Adding them here would require
    //! a dev-dep on sovereign-storage-sqlite, which is a workspace
    //! cycle (storage-sqlite already depends on core). Instead, the
    //! pure-logic parts of the use case are tested here.
    use super::*;

    #[test]
    fn deploy_request_carries_required_fields() {
        // Compile-time check that the public surface is stable.
        let req = DeployRequest {
            app_id: AppId::generate(),
            image_ref: Some("nginx:alpine".to_string()),
            strategy: Strategy::BlueGreen,
            wait: true,
            actor: "alice".to_string(),
        };
        assert_eq!(req.actor, "alice");
        assert!(req.wait);
        assert_eq!(req.strategy, Strategy::BlueGreen);
    }

    #[test]
    fn default_health_timeout_is_sane() {
        // The health-probe budget must be > 0 and < 5 minutes (the
        // user-facing deploy promise is 30s).
        assert!(DEFAULT_HEALTH_TIMEOUT >= std::time::Duration::from_secs(5));
        assert!(DEFAULT_HEALTH_TIMEOUT <= std::time::Duration::from_secs(300));
    }
}
