//! Target-arch §7.2: `PromotionPipeline` — four stages, a health probe after
//! each, and automatic rollback to the pre-promotion checkpoint on any
//! failure (KEEL-004).
//!
//! The pipeline drives stages 1-3 through the SAME ports the rollback
//! restores (`RollbackExecutor` exposes them), so a promotion and its
//! compensating rollback can never be wired against different systems.
//!
//! ## Fail-closed posture
//!
//! * [`PromotionPipeline::new`] refuses to build a pipeline missing, or
//!   carrying a duplicate/unknown, stage probe — so no stage can ever run
//!   with zero verification. This is validated at WIRING time rather than
//!   mid-promotion, when some dimensions would already have moved.
//! * A probe returning `Ok(false)` **or** `Err(_)` both count as unhealthy:
//!   a health check that cannot answer is not evidence of health.

use std::collections::HashMap;
use std::sync::Arc;

use kelson_foundation::AppError;
use serde::{Deserialize, Serialize};

use crate::ports::HealthCheck;
use crate::rollback::{PromotionTarget, RollbackExecutor};
use crate::PromotionState;

/// The four §7.2 stages, in order. Each must have exactly one registered
/// probe — [`PromotionPipeline::new`] enforces it.
pub const PROMOTION_STAGES: [&str; 4] = ["post-deploy", "post-migrate", "post-traffic", "canary"];

/// What is being promoted: the release's identity plus the five-dimension
/// target it moves the system TO (§7.2's `ReleaseArtifact` argument).
///
/// Serde so `kelson promote` can read it from the promotion plan file
/// (`storage/framework/promotion.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseArtifact {
    /// Release identity — the `version` from the `kelson build` manifest, so a
    /// failure report names exactly what was attempted.
    pub release: String,
    /// Where each dimension should land.
    pub target: PromotionTarget,
}

impl ReleaseArtifact {
    pub fn new(release: impl Into<String>, target: PromotionTarget) -> Self {
        Self {
            release: release.into(),
            target,
        }
    }
}

/// §7.2 `PromotionPipeline`.
pub struct PromotionPipeline {
    executor: Arc<RollbackExecutor>,
    checks: HashMap<String, Arc<dyn HealthCheck>>,
}

impl PromotionPipeline {
    /// Build a pipeline guarding all four stages.
    ///
    /// Fails closed: a missing, duplicate, or unknown stage name is a wiring
    /// error, surfaced here rather than halfway through a promotion.
    pub fn new(
        executor: Arc<RollbackExecutor>,
        checks: Vec<Arc<dyn HealthCheck>>,
    ) -> Result<Self, AppError> {
        let mut map: HashMap<String, Arc<dyn HealthCheck>> = HashMap::new();
        for check in checks {
            let name = check.name().to_string();
            if map.insert(name.clone(), check).is_some() {
                return Err(AppError::validation(format!(
                    "duplicate health check registered for stage `{name}`"
                )));
            }
        }

        for stage in PROMOTION_STAGES {
            if !map.contains_key(stage) {
                return Err(AppError::validation(format!(
                    "no health check registered for stage `{stage}`; \
                     promotion requires a probe for each of {PROMOTION_STAGES:?}"
                )));
            }
        }

        // A probe registered under a name that no stage uses would sit there
        // silently never running — reject it instead of ignoring it.
        for name in map.keys() {
            if !PROMOTION_STAGES.contains(&name.as_str()) {
                return Err(AppError::validation(format!(
                    "health check `{name}` does not match any promotion stage; \
                     expected one of {PROMOTION_STAGES:?}"
                )));
            }
        }

        Ok(Self {
            executor,
            checks: map,
        })
    }

    pub fn executor(&self) -> &Arc<RollbackExecutor> {
        &self.executor
    }

    /// §7.2 `promote`: run the four stages against `release`, rolling every
    /// dimension back to the pre-promotion checkpoint on the first failure.
    ///
    /// Returns [`PromotionState::Deployed`] only once every gate passed.
    pub async fn promote(&self, release: ReleaseArtifact) -> Result<PromotionState, AppError> {
        let mut state = PromotionState::Built;

        // §7.2 `create_checkpoint` — taken BEFORE anything moves, so the
        // restore target is provably the pre-promotion state.
        let checkpoint = self.executor.checkpoint().await?;

        // Stage 1 — deploy artifacts (no traffic): WASM module pins.
        if let Err(e) = self
            .executor
            .modules()
            .pin_versions(&release.target.module_versions)
            .await
        {
            return Err(self.abort(&release, &checkpoint, PROMOTION_STAGES[0], e.public_message).await);
        }
        state = state.advance()?;
        if let Err(reason) = self.gate(PROMOTION_STAGES[0]).await {
            return Err(self.abort(&release, &checkpoint, PROMOTION_STAGES[0], reason).await);
        }

        // Stage 2 — migrate DB (forward only). Skipped entirely when the
        // release carries no schema change (`db_version: None`), mirroring
        // §7.1's optional DB dimension.
        if release.target.db_version.is_some() {
            if let Err(e) = self.executor.migrations().apply_pending().await {
                return Err(self.abort(&release, &checkpoint, PROMOTION_STAGES[1], e.public_message).await);
            }
            // The release must actually have landed where it claims: a
            // watermark short of the target means migrations are missing.
            match self.executor.migrations().watermark().await {
                Ok(w) => {
                    if let Some(expected) = release.target.db_version {
                        if w < expected {
                            return Err(self.abort(
                                &release,
                                &checkpoint,
                                PROMOTION_STAGES[1],
                                format!("migrations reached watermark {w}, release expects {expected}"),
                            ).await);
                        }
                    }
                }
                Err(e) => {
                    return Err(self.abort(&release, &checkpoint, PROMOTION_STAGES[1], e.public_message).await);
                }
            }
        }
        if let Err(reason) = self.gate(PROMOTION_STAGES[1]).await {
            return Err(self.abort(&release, &checkpoint, PROMOTION_STAGES[1], reason).await);
        }

        // Stage 3 — switch traffic: make the release's config live.
        //
        // `config_version == 0` means the release carries no pinned
        // revision — a forward release cannot name one in advance, because
        // revisions are minted by the reload that OBSERVES changed content.
        // So 0 = pick the source up; a non-zero value pins an exact held
        // revision (§7.2 `activate_release`). Version numbers start at 1
        // (`ConfigRuntime::boot`), so 0 is genuinely unused as a revision.
        let activated = if release.target.config_version == 0 {
            self.executor.config().reload().await
        } else {
            self.executor
                .config()
                .activate(release.target.config_version)
                .await
        };
        if let Err(e) = activated {
            return Err(self.abort(&release, &checkpoint, PROMOTION_STAGES[2], e.public_message).await);
        }
        if let Err(reason) = self.gate(PROMOTION_STAGES[2]).await {
            return Err(self.abort(&release, &checkpoint, PROMOTION_STAGES[2], reason).await);
        }

        // Stage 4 — canary verification.
        if let Err(reason) = self.gate(PROMOTION_STAGES[3]).await {
            return Err(self.abort(&release, &checkpoint, PROMOTION_STAGES[3], reason).await);
        }

        // Staged -> Deployed. Cannot fail: `state` is `Staged` by
        // construction at this point.
        state = state.advance()?;
        Ok(state)
    }

    /// Run the probe guarding `stage`.
    ///
    /// `Ok(false)` and `Err(_)` are both unhealthy — fail closed either way.
    async fn gate(&self, stage: &str) -> Result<(), String> {
        let probe = self
            .checks
            .get(stage)
            .expect("`new` validated that every stage has exactly one probe");
        match probe.run().await {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!("`{stage}` health check reported unhealthy")),
            Err(e) => Err(format!("`{stage}` health check failed: {}", e.public_message)),
        }
    }

    /// Roll every dimension back to `checkpoint` and build the failure error.
    ///
    /// The restore runs regardless of where the state machine got to — a
    /// stage-1 failure leaves `state == Built`, where `PromotionState::
    /// rollback_from` would refuse. `PromotionState` is bookkeeping; the
    /// executor is what actually restores.
    async fn abort(
        &self,
        release: &ReleaseArtifact,
        checkpoint: &PromotionTarget,
        stage: &str,
        reason: String,
    ) -> AppError {
        let mut err = AppError::workflow(format!(
            "promotion of `{}` failed at stage `{stage}`: {reason}",
            release.release
        ))
        .with_meta("stage", stage)
        .with_meta("release", release.release.as_str());

        match self.executor.rollback(checkpoint).await {
            Ok(report) => {
                err = err
                    .with_meta("rolled_back", true)
                    .with_meta("config_version", report.config_version)
                    .with_meta("reverted", report.reverted)
                    .with_meta("pins_written", report.pins_written);
            }
            Err(e) => {
                err = err
                    .with_meta("rolled_back", false)
                    .with_meta("rollback_error", e.public_message);
                if let Some(internal) = e.internal {
                    err = err.with_internal(internal);
                }
            }
        }
        err
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{healthy_probes, Harness, Outcome};
    use kelson_foundation::codes;
    use std::collections::BTreeMap;

    /// A release that moves every dimension forward from the harness's live
    /// state (config 7, watermark 5, `echo -> v1`). `config_version: 0` =
    /// source-driven config, the normal case for a forward release.
    fn release() -> ReleaseArtifact {
        ReleaseArtifact::new(
            "1.4.0",
            PromotionTarget {
                config_version: 0,
                db_version: Some(6),
                module_versions: BTreeMap::from([("echo".to_string(), "v2".to_string())]),
                job_schema_versions: BTreeMap::from([("mail.send".to_string(), 2u32)]),
            },
        )
    }

    /// Build a pipeline whose probes are all healthy except `bad`.
    fn pipeline(h: &Harness, bad: Option<(&str, Outcome)>) -> PromotionPipeline {
        let checks: Vec<Arc<dyn HealthCheck>> = ["post-deploy", "post-migrate", "post-traffic", "canary"]
            .into_iter()
            .map(|stage| {
                let outcome = match bad {
                    Some((name, o)) if name == stage => o,
                    _ => Outcome::Healthy,
                };
                Arc::new(crate::fakes::FakeHealth {
                    name: stage,
                    outcome,
                    trace: h.trace.clone(),
                }) as Arc<dyn HealthCheck>
            })
            .collect();
        PromotionPipeline::new(h.executor().into(), checks).unwrap()
    }

    /// Every stage runs, in order, and the system lands on the release.
    #[tokio::test]
    async fn promote_runs_all_four_stages_in_order_and_deploys() {
        let h = Harness::new();
        let p = pipeline(&h, None);

        let state = p.promote(release()).await.unwrap();
        assert_eq!(state, PromotionState::Deployed);

        assert_eq!(
            h.trace.calls(),
            vec![
                // checkpoint, taken before anything moves
                "config.current",
                "db.watermark",
                "modules.current_pins",
                "jobs.supported_versions",
                // stage 1: deploy (no traffic) + gate
                "modules.pin_versions(1)",
                "health.post-deploy",
                // stage 2: migrate + gate
                "db.apply_pending",
                "db.watermark",
                "health.post-migrate",
                // stage 3: switch traffic + gate
                "config.reload",
                "health.post-traffic",
                // stage 4: canary
                "health.canary",
            ]
        );

        // The release actually landed (reload mints the next revision: 7 -> 8).
        assert_eq!(*h.config.live.lock().unwrap(), 8);
        assert_eq!(*h.migrations.watermark.lock().unwrap(), 6);
        assert_eq!(
            h.modules.pins.lock().unwrap().get("echo").map(String::as_str),
            Some("v2")
        );
    }

    /// A gate failing at stage 3 restores the checkpoint — every dimension
    /// back to its pre-promotion value.
    #[tokio::test]
    async fn unhealthy_gate_restores_the_checkpoint() {
        let h = Harness::new();
        let p = pipeline(&h, Some(("post-traffic", Outcome::Unhealthy)));

        let err = p.promote(release()).await.unwrap_err();
        assert_eq!(err.code, codes::WORKFLOW);
        assert_eq!(err.meta.get("stage").and_then(|v| v.as_str()), Some("post-traffic"));
        assert_eq!(err.meta.get("release").and_then(|v| v.as_str()), Some("1.4.0"));
        assert_eq!(err.meta.get("rolled_back").and_then(|v| v.as_bool()), Some(true));

        // Nothing moved: the live state is byte-identical to the start.
        assert_eq!(*h.config.live.lock().unwrap(), 7, "config restored");
        assert_eq!(*h.migrations.watermark.lock().unwrap(), 5, "db restored");
        assert_eq!(
            h.modules.pins.lock().unwrap().get("echo").map(String::as_str),
            Some("v1"),
            "pins restored"
        );
        // The failed gate is the last probe run; the restore followed it.
        let calls = h.trace.calls();
        let failed = calls.iter().position(|c| c == "health.post-traffic").unwrap();
        assert!(
            calls[failed + 1..].contains(&"config.activate(7)".to_string()),
            "restore ran after the failed gate: {calls:?}"
        );
    }

    /// Stage 1 fails BEFORE `state` ever leaves `Built` — the restore must
    /// still happen (the state machine's own `rollback_from` would refuse
    /// from `Built`, which is exactly why the executor is what restores).
    #[tokio::test]
    async fn stage_one_failure_still_restores_the_checkpoint() {
        let h = Harness::new();
        let p = pipeline(&h, Some(("post-deploy", Outcome::Unhealthy)));

        let err = p.promote(release()).await.unwrap_err();
        assert_eq!(err.code, codes::WORKFLOW);
        assert_eq!(err.meta.get("stage").and_then(|v| v.as_str()), Some("post-deploy"));
        assert_eq!(err.meta.get("rolled_back").and_then(|v| v.as_bool()), Some(true));

        assert_eq!(
            h.modules.pins.lock().unwrap().get("echo").map(String::as_str),
            Some("v1"),
            "stage 1 pin reverted"
        );
        assert_eq!(*h.config.live.lock().unwrap(), 7);
        assert_eq!(*h.migrations.watermark.lock().unwrap(), 5);
        // Promotion never got past stage 1: no migration work, and the
        // release's config was never picked up. (`config.activate` does
        // appear once — that is the RESTORE back to 7, not stage 3.)
        assert!(h.trace.under("db.apply_pending").is_empty());
        assert!(
            !h.trace.calls().iter().any(|c| c == "config.reload"),
            "release config must never have gone live: {:?}",
            h.trace.calls()
        );
    }

    /// A probe that cannot answer is unhealthy, not "unknown, proceed".
    #[tokio::test]
    async fn probe_error_is_treated_as_unhealthy() {
        let h = Harness::new();
        let p = pipeline(&h, Some(("canary", Outcome::Error)));

        let err = p.promote(release()).await.unwrap_err();
        assert_eq!(err.code, codes::WORKFLOW);
        assert_eq!(err.meta.get("stage").and_then(|v| v.as_str()), Some("canary"));
        assert_eq!(err.meta.get("rolled_back").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(*h.config.live.lock().unwrap(), 7, "config restored");
    }

    /// Migrations that do not reach the release's claimed watermark are a
    /// stage-2 failure, not a silent partial deploy.
    #[tokio::test]
    async fn short_migrations_refuse_before_the_gate() {
        let h = Harness::new();
        // Release expects watermark 6, but nothing beyond 5 can be applied.
        *h.migrations.reachable.lock().unwrap() = 5;
        let p = pipeline(&h, None);

        let err = p.promote(release()).await.unwrap_err();
        assert_eq!(err.code, codes::WORKFLOW);
        let stage = err.meta.get("stage").and_then(|v| v.as_str()).unwrap().to_string();
        assert_eq!(stage, "post-migrate", "refused before stage 2's gate");
        assert_eq!(err.meta.get("rolled_back").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(*h.config.live.lock().unwrap(), 7, "config never activated");
        assert!(h.trace.under("health.post-traffic").is_empty(), "later stages never ran");
    }

    /// Wiring is fail-closed: a missing, duplicate, or unknown probe is a
    /// construction error rather than a gate that silently never runs.
    #[test]
    fn wiring_rejects_missing_duplicate_and_unknown_probes() {
        let h = Harness::new();

        let missing = healthy_probes(&h.trace).into_iter().take(3).collect();
        assert!(PromotionPipeline::new(h.executor().into(), missing).is_err());

        let mut duplicate = healthy_probes(&h.trace);
        duplicate.push(healthy_probes(&h.trace).remove(0));
        assert!(PromotionPipeline::new(h.executor().into(), duplicate).is_err());

        let mut unknown = healthy_probes(&h.trace);
        unknown.push(Arc::new(crate::fakes::FakeHealth {
            name: "post-whenever",
            outcome: Outcome::Healthy,
            trace: h.trace.clone(),
        }));
        assert!(PromotionPipeline::new(h.executor().into(), unknown).is_err());
    }

    /// A release carrying no schema change skips stage 2's DB work entirely
    /// (§7.1's optional `db_version`), while the rest still runs.
    #[tokio::test]
    async fn release_without_schema_change_skips_the_database() {
        let h = Harness::new();
        let p = pipeline(&h, None);

        let mut r = release();
        r.target.db_version = None;
        assert_eq!(p.promote(r).await.unwrap(), PromotionState::Deployed);

        assert!(h.trace.under("db.apply_pending").is_empty(), "DB work skipped");
        // The probe still runs: skipping a gate would quietly weaken the
        // fail-closed posture for a release that happens to carry no DDL.
        assert!(!h.trace.under("health.post-migrate").is_empty(), "gate still runs");
        assert_eq!(*h.config.live.lock().unwrap(), 8, "config still advanced");
    }

    /// Keeps the unused-import lint honest about `healthy_probes` here:
    /// `pipeline` builds probes directly so it can substitute one.
    #[test]
    fn healthy_probes_builds_exactly_the_four_stages() {
        let h = Harness::new();
        let mut names: Vec<String> = healthy_probes(&h.trace)
            .iter()
            .map(|c| c.name().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["canary", "post-deploy", "post-migrate", "post-traffic"]);
        assert!(PromotionPipeline::new(h.executor().into(), healthy_probes(&h.trace)).is_ok());
    }

    /// Chaos (Phase 5, §9 "rollback mid-promotion"): the full failure
    /// matrix — every one of the four stages, probed both ways (`Ok(false)`
    /// and `Err(_)`).
    ///
    /// Phase 4 covered stage 1 and a single unhealthy gate; this closes the
    /// matrix so no stage boundary is only tested in one failure mode. For
    /// each of the 8 cells the pipeline must:
    ///
    ///  * abort with `WORKFLOW` naming exactly that stage and release;
    ///  * restore every dimension to the checkpoint (config 7, watermark 5,
    ///    `echo -> v1`) and say so in the report;
    ///  * actually undo whatever the failed stage had already done — stage 1
    ///    only pins, stages 2-4 have moved the database too;
    ///  * never run a LATER stage's probe;
    ///  * run the restore strictly AFTER the failed probe, §7.1's read-only
    ///    guards first and the mutation after them.
    #[tokio::test]
    async fn every_stage_fails_closed_and_restores_the_checkpoint() {
        for (idx, stage) in PROMOTION_STAGES.into_iter().enumerate() {
            for outcome in [Outcome::Unhealthy, Outcome::Error] {
                let h = Harness::new();
                let p = pipeline(&h, Some((stage, outcome)));
                let at = format!("{stage}/{outcome:?}");

                let err = p.promote(release()).await.unwrap_err();

                // 1. A workflow failure that names this stage and release.
                assert_eq!(err.code, codes::WORKFLOW, "{at}: {:?}", err.meta);
                assert_eq!(
                    err.meta.get("stage").and_then(|v| v.as_str()),
                    Some(stage),
                    "{at}"
                );
                assert_eq!(
                    err.meta.get("release").and_then(|v| v.as_str()),
                    Some("1.4.0"),
                    "{at}"
                );

                // 2. The restore ran, and reported itself as having run.
                assert_eq!(
                    err.meta.get("rolled_back").and_then(|v| v.as_bool()),
                    Some(true),
                    "{at}: restore must succeed"
                );
                assert_eq!(
                    err.meta.get("config_version").and_then(|v| v.as_u64()),
                    Some(7),
                    "{at}: report names the restored revision"
                );

                // 3. Work the failed stage had already performed is undone.
                //    Stage 1 only pins; from stage 2 on the DB moved. The
                //    reverted list names the BATCHES unwound, so dropping
                //    watermark 6 -> 5 unwinds batch `m6` (batch 5 is the one
                //    the watermark already sits above).
                let expect_reverted: Vec<String> = if idx == 0 {
                    Vec::new()
                } else {
                    vec!["m6".to_string()]
                };
                let reverted: Vec<String> = err
                    .meta
                    .get("reverted")
                    .and_then(|v| v.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|i| i.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(reverted, expect_reverted, "{at}: reverted batches");
                assert_eq!(
                    err.meta.get("pins_written").and_then(|v| v.as_u64()),
                    Some(1),
                    "{at}: the pin written by stage 1 was pinned back"
                );

                // 4. Every dimension is byte-identical to the start.
                assert_eq!(*h.config.live.lock().unwrap(), 7, "{at}: config restored");
                assert_eq!(
                    *h.migrations.watermark.lock().unwrap(),
                    5,
                    "{at}: watermark restored"
                );
                assert_eq!(
                    h.modules.pins.lock().unwrap().get("echo").map(String::as_str),
                    Some("v1"),
                    "{at}: pins restored"
                );

                // 5. The failing probe ran; no later stage's probe ever did.
                let calls = h.trace.calls();
                let gate = format!("health.{stage}");
                let gate_at = calls
                    .iter()
                    .position(|c| c == &gate)
                    .unwrap_or_else(|| panic!("{at}: probe never ran: {calls:?}"));
                for later in PROMOTION_STAGES.iter().skip(idx + 1) {
                    assert!(
                        h.trace.under(&format!("health.{later}")).is_empty(),
                        "{at}: stage `{later}` ran after `{stage}` failed: {calls:?}"
                    );
                }

                // 6. Restore strictly after the failure, guards before the
                //    first mutation of that restore.
                let restore = calls
                    .iter()
                    .position(|c| c == "config.activate(7)")
                    .unwrap_or_else(|| panic!("{at}: no restore ran: {calls:?}"));
                assert!(
                    restore > gate_at,
                    "{at}: restore must follow the failed probe: {calls:?}"
                );
                let guards = calls
                    .iter()
                    .position(|c| c == "jobs.ensure_compatibility")
                    .unwrap_or_else(|| panic!("{at}: §7.1 guards missing: {calls:?}"));
                assert!(
                    guards > gate_at && guards < restore,
                    "{at}: read-only guards must run after the failure and before \
                     any mutation: {calls:?}"
                );
            }
        }
    }
}
