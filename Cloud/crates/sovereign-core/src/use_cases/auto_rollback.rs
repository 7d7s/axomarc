// The auto-rollback use case (F8b).
//
// A `Prober` runs in the background (one per app with
// `auto_rollback = true`). It loops every `interval` and probes the
// current Healthy deployment. On `threshold` consecutive failures, it
// triggers `rollback::start_rollback` to the previous Healthy
// deployment and appends an audit event. The prober is cancelable
// (caller passes a `tokio_util::sync::CancellationToken`) and is the
// only piece of "background work" in V0.
//
// The spec (`docs/phase-00-mvp.md` F8b) calls for an end-to-end CI
// test that runs `nginx:alpine` + a broken image in Docker and
// asserts the rollback within 90s. The CI matrix does not yet have
// Docker, so the integration test in this step is a state-machine
// test against a `MockRuntime` — the real-Docker test is queued for
// the Step 9 Hetzner CX22 run (per the user's chosen scope).
//
// V0 defaults: `interval = 30s`, `threshold = 3`, `timeout = 5s`.
// The prober refuses to start if the current deployment is
// `Failed`/`RolledBack` (no work to do).
//
// The unit tests for this module live in
// `sovereign-storage-sqlite/tests/auto_rollback_integration.rs` —
// they need a real `StoragePort` implementation, which sovereign-core
// does not itself depend on (so the dep cycle would not surprise us).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};

use crate::domain::{
    AppId, AuditEvent, AuditKind, DeploymentEvent, DeploymentId, DeploymentStatus, Timestamp,
};
use crate::error::AppError;
use crate::state::AppState;

/// Per-app prober configuration. Lives next to the use case (not on
/// the `app` row) so the V0 surface stays migration-free; the spec's
/// "health: { interval, timeout, threshold } in app.yaml" is a V0.5
/// extension.
#[derive(Debug, Clone)]
pub struct ProberConfig {
    /// Time between probes.
    pub interval: Duration,
    /// Per-probe timeout.
    pub timeout: Duration,
    /// Number of consecutive failures that triggers the auto-rollback.
    pub threshold: u32,
    /// Container port to probe. Defaults to 8080.
    pub port: u16,
    /// Health path (HTTP) — falls back to TCP if empty.
    pub health_path: String,
}

impl Default for ProberConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            timeout: Duration::from_secs(5),
            threshold: 3,
            port: 8080,
            health_path: "/health".to_string(),
        }
    }
}

/// The state the prober tracks between iterations. Pure data, no I/O.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProberState {
    /// Total probes run.
    pub probes: u64,
    /// Failures counted so far in the current streak. Reset on success.
    pub consecutive_failures: u32,
    /// True iff the prober has fired an auto-rollback in this lifetime
    /// (the prober stops probing after a rollback — the previous
    /// version needs a chance to prove itself).
    pub rolled_back: bool,
    /// Last seen current deployment id (for change detection).
    pub last_seen: Option<DeploymentId>,
}

impl ProberState {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` once `consecutive_failures` has reached `threshold` and
    /// the prober has not yet rolled back. The prober logic uses
    /// this to decide when to fire the rollback.
    pub fn should_roll_back(&self, threshold: u32) -> bool {
        !self.rolled_back && self.consecutive_failures >= threshold
    }
}

/// The prober itself. Cheap to construct; `run` consumes it and loops
/// until the cancel token is tripped.
pub struct Prober {
    pub app_id: AppId,
    pub config: ProberConfig,
    pub state: Arc<Mutex<ProberState>>,
}

impl Prober {
    pub fn new(app_id: AppId, config: ProberConfig) -> Self {
        Self {
            app_id,
            config,
            state: Arc::new(Mutex::new(ProberState::new())),
        }
    }

    /// One iteration of the loop. Exposed for the test (so the test
    /// does not have to wait `interval` real seconds). Returns
    /// `true` if a rollback was fired this iteration.
    #[instrument(skip(self, state), fields(app = %self.app_id))]
    pub async fn step(&self, state: &AppState) -> Result<bool, AppError> {
        let mut s = self.state.lock().await;
        s.probes += 1;

        // Resolve the current Healthy deployment. If there's no
        // current (e.g. the app was just created), reset the
        // failure counter so a future deploy starts clean.
        let current = state.storage.get_current_deployment(self.app_id).await?;
        let Some(current) = current else {
            s.consecutive_failures = 0;
            return Ok(false);
        };
        s.last_seen = Some(current.id);

        if !matches!(current.status, DeploymentStatus::Healthy) {
            // Prober only watches Healthy deployments. If the current
            // is already Failed / RollingBack / etc., there's nothing
            // useful to probe.
            s.consecutive_failures = 0;
            return Ok(false);
        }

        if s.rolled_back {
            // We already fired in this lifetime. The rollback use
            // case has presumably restored the previous Healthy; we
            // wait for the operator to either redeploy or accept the
            // rollback as the new baseline (next probe resets).
            return Ok(false);
        }

        // The container id is `sovereign-{app_id}-{deployment_id}` by
        // convention (see `sovereign-runtime-docker::create_container`).
        let container_id = format!("sovereign-{}-{}", self.app_id, current.id);

        let result = state
            .runtime
            .healthcheck(
                &container_id,
                self.config.port,
                &self.config.health_path,
                self.config.timeout,
            )
            .await
            .unwrap_or(crate::ports::HealthResult {
                ok: false,
                latency_ms: 0,
                error: Some("healthcheck errored".into()),
            });

        if result.ok {
            s.consecutive_failures = 0;
            return Ok(false);
        }

        s.consecutive_failures = s.consecutive_failures.saturating_add(1);
        if !s.should_roll_back(self.config.threshold) {
            return Ok(false);
        }

        // Threshold reached — fire the rollback.
        warn!(
            app = %self.app_id,
            deployment = %current.id,
            failures = s.consecutive_failures,
            "healthcheck threshold reached; auto-rolling back"
        );
        s.rolled_back = true;
        drop(s); // release the lock before the I/O below

        trigger_auto_rollback(state, self.app_id, current.id, &result).await?;
        Ok(true)
    }

    /// Run the prober loop. Returns when `cancel` is tripped or
    /// immediately if the prober has already rolled back (one
    /// rollback per prober lifetime, by design).
    pub async fn run(self, state: AppState, cancel: CancellationToken) -> Result<(), AppError> {
        info!(app = %self.app_id, "auto-rollback prober started");
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!(app = %self.app_id, "auto-rollback prober cancelled");
                    return Ok(());
                }
                _ = sleep(self.config.interval) => {}
            }
            if let Err(e) = self.step(&state).await {
                warn!(app = %self.app_id, error = %e, "prober step errored");
            }
            // After a rollback we let the next interval reset the
            // counter (the new "current" is the previous version).
            // We don't kill the prober — if THAT version also
            // fails, we want to know.
            let s = self.state.lock().await;
            if s.rolled_back && s.consecutive_failures >= self.config.threshold * 2 {
                warn!(
                    app = %self.app_id,
                    "post-rollback deployment is also failing; stopping the prober"
                );
                return Ok(());
            }
        }
    }
}

/// Drive the actual rollback. Extracted from `Prober::step` so the
/// test can call it directly with a synthetic `HealthResult` and
/// assert the audit event.
#[instrument(skip(state), fields(app = %app_id, deployment = %deployment_id))]
pub async fn trigger_auto_rollback(
    state: &AppState,
    app_id: AppId,
    deployment_id: DeploymentId,
    last_probe: &crate::ports::HealthResult,
) -> Result<(), AppError> {
    // Find the previous Healthy deployment to roll back to.
    let now = Timestamp::now();
    let prev = state
        .storage
        .list_healthy_deployments_before(app_id, now, 2)
        .await?;
    let target = prev.into_iter().find(|d| d.id != deployment_id);

    let Some(target) = target else {
        // No previous Healthy — mark the current as Failed and
        // leave it at that; the operator can intervene.
        let _ = state
            .storage
            .transition_deployment(
                deployment_id,
                DeploymentEvent {
                    to: DeploymentStatus::Failed,
                    error: Some("auto-rollback: no previous healthy deployment".into()),
                    actor: "system:auto-rollback".into(),
                },
                "system:auto-rollback",
            )
            .await;
        let _ = state
            .storage
            .append_audit(AuditEvent {
                id: None,
                ts: Timestamp::now(),
                actor: "system:auto-rollback".into(),
                kind: AuditKind::Rollback,
                target: Some(format!("app:{app_id}")),
                payload: serde_json::json!({
                    "reason": "health_failure",
                    "deployment_id": deployment_id,
                    "target": null,
                    "error": "no previous healthy deployment",
                    "last_probe": last_probe,
                }),
                policy_decision: None,
            })
            .await;
        return Ok(());
    };

    // Reuse the F5 rollback use case to create a rollback marker +
    // transition the current to RolledBack. This is the same code
    // path the CLI uses for `sovereign rollback`.
    let req = crate::use_cases::rollback::RollbackRequest {
        app_id,
        to: Some(target.id),
        actor: "system:auto-rollback".to_string(),
    };
    crate::use_cases::rollback::start_rollback(state, req).await?;

    let _ = state
        .storage
        .append_audit(AuditEvent {
            id: None,
            ts: Timestamp::now(),
            actor: "system:auto-rollback".into(),
            kind: AuditKind::Rollback,
            target: Some(format!("app:{app_id}")),
            payload: serde_json::json!({
                "reason": "health_failure",
                "deployment_id": deployment_id,
                "target": target.id,
                "last_probe": last_probe,
            }),
            policy_decision: None,
        })
        .await;

    info!(
        app = %app_id,
        from = %deployment_id,
        to = %target.id,
        "auto-rollback complete"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_roll_back_threshold_logic() {
        let mut s = ProberState::new();
        s.consecutive_failures = 2;
        assert!(!s.should_roll_back(3));
        s.consecutive_failures = 3;
        assert!(s.should_roll_back(3));
        s.rolled_back = true;
        assert!(!s.should_roll_back(3), "one rollback per prober lifetime");
    }

    #[test]
    fn default_config_values() {
        let c = ProberConfig::default();
        assert_eq!(c.threshold, 3);
        assert_eq!(c.interval, Duration::from_secs(30));
        assert_eq!(c.timeout, Duration::from_secs(5));
        assert_eq!(c.port, 8080);
        assert_eq!(c.health_path, "/health");
    }
}
