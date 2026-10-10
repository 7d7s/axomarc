//! Target-arch §7.1: `RollbackExecutor` — all five rollback dimensions.
//!
//! Ordering note (deliberate, documented deviation from the doc's literal
//! line order): §7.1 lists the dimensions 1..5 as config, migrations,
//! modules, jobs, events. Dimensions 4 and 5 are READ-ONLY checks, so they
//! are hoisted to run BEFORE any mutation — an incompatible target then
//! fails closed with *nothing* half-rolled-back instead of after the config
//! and database have already moved. The three MUTATING dimensions keep the
//! doc's order exactly (config -> migrations -> modules).

use std::collections::BTreeMap;
use std::sync::Arc;

use kelson_foundation::AppError;
use serde::{Deserialize, Serialize};

use crate::ports::{ConfigPort, EventPort, JobPort, MigrationPort, ModulePort};

/// The target of a rollback — and the shape §7.2's checkpoint captures so a
/// failed promotion can be restored to exactly this.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionTarget {
    /// Config revision to make live (§7.1 `config_version`).
    pub config_version: u64,
    /// Pair-migration batch watermark to sit at (§7.1 `db_version`).
    /// `None` = leave the database alone (the doc's `Option`), which is the
    /// normal case when a release carried no schema change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_version: Option<u32>,
    /// WASM module name -> pinned version (§7.1 `module_versions`).
    #[serde(default)]
    pub module_versions: BTreeMap<String, String>,
    /// Job name -> schema version the target understands (§7.1
    /// `job_schema_versions`).
    #[serde(default)]
    pub job_schema_versions: BTreeMap<String, u32>,
}

/// What each dimension did during a rollback. The five booleans are the
/// doc's flags; the detail fields are runbook evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackReport {
    pub config_rolled_back: bool,
    pub migrations_rolled_back: bool,
    pub modules_pinned: bool,
    pub jobs_compatible: bool,
    pub events_compatible: bool,
    /// Config revision now live.
    pub config_version: u64,
    /// Pair migrations reverted, newest first.
    pub reverted: Vec<String>,
    /// Module pins that changed.
    pub pins_written: usize,
}

/// §7.1 `RollbackExecutor`. Holds one port per dimension; every method is
/// async because the migration and health-check adapters do real IO.
pub struct RollbackExecutor {
    config: Arc<dyn ConfigPort>,
    migrations: Arc<dyn MigrationPort>,
    modules: Arc<dyn ModulePort>,
    jobs: Arc<dyn JobPort>,
    events: Arc<dyn EventPort>,
}

impl RollbackExecutor {
    pub fn new(
        config: Arc<dyn ConfigPort>,
        migrations: Arc<dyn MigrationPort>,
        modules: Arc<dyn ModulePort>,
        jobs: Arc<dyn JobPort>,
        events: Arc<dyn EventPort>,
    ) -> Self {
        Self {
            config,
            migrations,
            modules,
            jobs,
            events,
        }
    }

    /// Dimension accessors — `PromotionPipeline` drives stages 1-3 through
    /// the same collaborators the rollback restores, so a promotion and its
    /// rollback can never be wired against different systems.
    pub fn config(&self) -> &Arc<dyn ConfigPort> {
        &self.config
    }
    pub fn migrations(&self) -> &Arc<dyn MigrationPort> {
        &self.migrations
    }
    pub fn modules(&self) -> &Arc<dyn ModulePort> {
        &self.modules
    }
    pub fn jobs(&self) -> &Arc<dyn JobPort> {
        &self.jobs
    }
    pub fn events(&self) -> &Arc<dyn EventPort> {
        &self.events
    }

    /// §7.2 `create_checkpoint`: capture every dimension's live state so a
    /// failed promotion restores exactly what was running before.
    ///
    /// A checkpoint is read-only — nothing here mutates the system.
    pub async fn checkpoint(&self) -> Result<PromotionTarget, AppError> {
        let config_version = self.config.current_version().await?;
        let db_version = self.migrations.watermark().await?;
        let module_versions = self.modules.current_pins().await?;
        let job_schema_versions = self.jobs.supported_versions().await?;
        Ok(PromotionTarget {
            config_version,
            db_version: Some(db_version),
            module_versions,
            job_schema_versions,
        })
    }

    /// §7.1 `rollback`: restore `target` across all five dimensions.
    ///
    /// Fail-fast (`?`): on the first error the report is lost and the error
    /// propagates, so the operator learns which dimension stopped. The
    /// dimensions are not transactional across one another (they span a
    /// config swap, SQL and a file) — that is why the compatibility guards
    /// run first: they are the two steps that can say "no" without having
    /// already changed something.
    pub async fn rollback(&self, target: &PromotionTarget) -> Result<RollbackReport, AppError> {
        let mut report = RollbackReport::default();

        // 4. Job compatibility (ensure in-flight jobs can complete) —
        //    read-only, hoisted ahead of every mutation.
        self.jobs
            .ensure_compatibility(&target.job_schema_versions)
            .await?;
        report.jobs_compatible = true;

        // 5. Event compatibility (idempotency keys stable across versions) —
        //    read-only, also hoisted.
        self.events.verify_idempotency_stability().await?;
        report.events_compatible = true;

        // 1. Config rollback (atomic snapshot swap).
        report.config_version = self.config.activate(target.config_version).await?;
        report.config_rolled_back = true;

        // 2. Migration rollback (pairs only — foundation tables require
        //    manual). `None` skips the database entirely.
        if let Some(db_version) = target.db_version {
            report.reverted = self.migrations.rollback_to(db_version).await?;
            report.migrations_rolled_back = true;
        }

        // 3. Module rollback (WASM plugin version pin).
        report.pins_written = self.modules.pin_versions(&target.module_versions).await?;
        report.modules_pinned = true;

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> PromotionTarget {
        crate::fakes::Harness::target()
    }

    /// The full five-dimension sweep: guards first, then §7.1's mutating
    /// order, every dimension reflected in the report.
    #[tokio::test]
    async fn rollback_runs_all_five_dimensions_with_guards_first() {
        let h = crate::fakes::Harness::new();
        let report = h.executor().rollback(&target()).await.unwrap();

        assert!(report.jobs_compatible && report.events_compatible);
        assert!(report.config_rolled_back && report.migrations_rolled_back);
        assert!(report.modules_pinned);
        assert_eq!(report.config_version, 3);
        // watermark 5 -> target 4 reverts exactly the batch above 4.
        assert_eq!(report.reverted, vec!["m5".to_string()]);
        assert_eq!(report.pins_written, 1, "echo v1 -> v2 changed");

        let calls = h.trace.calls();
        assert_eq!(calls[0], "jobs.ensure_compatibility", "guard runs first");
        assert_eq!(calls[1], "events.verify_idempotency_stability");
        // The three mutating dimensions keep §7.1's order.
        let mutating: Vec<&str> = calls[2..]
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(
            mutating,
            vec![
                "config.activate(3)",
                "db.rollback_to(4)",
                "modules.pin_versions(1)"
            ]
        );
    }

    /// An incompatible job target must refuse BEFORE the config or database
    /// move — the whole point of hoisting the read-only guards. Running code
    /// has stamped rows at schema 2; the target understands only 1.
    #[tokio::test]
    async fn incompatible_jobs_refuse_before_any_mutation() {
        let h = crate::fakes::Harness::build(Some(2), true);
        assert!(target().job_schema_versions["mail.send"] < 2);

        let err = h.executor().rollback(&target()).await.unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);

        assert_eq!(h.trace.calls(), vec!["jobs.ensure_compatibility".to_string()]);
        for prefix in ["config.", "db.", "modules."] {
            assert!(h.trace.under(prefix).is_empty(), "{prefix} must not have run");
        }
    }

    /// Unstable idempotency keys are also a pre-mutation refusal — the
    /// config, database and module pins all stay untouched.
    #[tokio::test]
    async fn unstable_idempotency_keys_refuse_before_any_mutation() {
        let h = crate::fakes::Harness::build(None, false);

        let err = h.executor().rollback(&target()).await.unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);

        assert_eq!(
            h.trace.calls(),
            vec![
                "jobs.ensure_compatibility".to_string(),
                "events.verify_idempotency_stability".to_string()
            ]
        );
        for prefix in ["config.", "db.", "modules."] {
            assert!(h.trace.under(prefix).is_empty(), "{prefix} must not have run");
        }
    }

    /// `db_version: None` skips the database dimension entirely (the doc's
    /// `Option`) while the other dimensions still run.
    #[tokio::test]
    async fn none_db_version_skips_the_database_dimension() {
        let h = crate::fakes::Harness::new();
        let mut t = target();
        t.db_version = None;

        let report = h.executor().rollback(&t).await.unwrap();
        assert!(!report.migrations_rolled_back);
        assert!(report.reverted.is_empty());
        assert!(report.config_rolled_back && report.modules_pinned);
        assert!(
            h.trace.under("db.").is_empty(),
            "migration port untouched: {:?}",
            h.trace.calls()
        );
    }

    /// The checkpoint captures every dimension's LIVE state, read-only.
    #[tokio::test]
    async fn checkpoint_captures_all_four_targeted_dimensions() {
        let h = crate::fakes::Harness::new();
        let cp = h.executor().checkpoint().await.unwrap();

        assert_eq!(cp.config_version, 7, "live config revision");
        assert_eq!(cp.db_version, Some(5), "live watermark");
        assert_eq!(cp.module_versions.get("echo").map(String::as_str), Some("v1"));
        assert_eq!(cp.job_schema_versions.get("mail.send"), Some(&2));

        // Read-only: no mutation call ever happened.
        for prefix in ["config.activate", "db.rollback_to", "modules.pin_versions"] {
            assert!(h.trace.under(prefix).is_empty(), "checkpoint mutated: {:?}", h.trace.calls());
        }
    }

    /// A round trip: checkpoint -> advance state -> rollback(checkpoint)
    /// restores exactly the captured state.
    #[tokio::test]
    async fn rollback_to_its_own_checkpoint_restores_state() {
        let h = crate::fakes::Harness::new();
        let ex = h.executor();
        let checkpoint = ex.checkpoint().await.unwrap();

        // Promotion advances every dimension past the checkpoint.
        *h.config.live.lock().unwrap() = 99;
        *h.migrations.watermark.lock().unwrap() = 6;
        *h.modules.pins.lock().unwrap() = BTreeMap::from([("echo".into(), "v9".into())]);

        let report = ex.rollback(&checkpoint).await.unwrap();
        assert_eq!(report.config_version, 7);
        assert_eq!(*h.config.live.lock().unwrap(), 7, "config restored");
        assert_eq!(*h.migrations.watermark.lock().unwrap(), 5, "db restored");
        assert_eq!(
            h.modules.pins.lock().unwrap().get("echo").map(String::as_str),
            Some("v1"),
            "pins restored"
        );
    }
}