use std::time::Duration;

use serde::Serialize;

use crate::check::{Check, CheckCategory, CheckContext, CheckResult, CheckStatus};
use crate::level::DoctorLevel;

/// One row in the doctor report: the check's name, category, the
/// `CheckResult`, and how long it took.
#[derive(Debug, Clone, Serialize)]
pub struct ReportEntry {
    pub name: &'static str,
    pub category: CheckCategory,
    pub status: CheckStatus,
    pub message: String,
    pub suggestion: Option<String>,
    pub has_fix: bool,
    pub duration_ms: u64,
}

/// The aggregate doctor report. `summary()` collapses every check
/// to a single exit-weight so the CLI can map it to a process exit
/// code (0 = pass, 1 = warn, 2 = fail).
#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub level: DoctorLevel,
    pub entries: Vec<ReportEntry>,
}

impl DoctorReport {
    pub fn new(level: DoctorLevel) -> Self {
        Self {
            level,
            entries: Vec::new(),
        }
    }

    pub fn add(&mut self, check: &dyn Check, result: CheckResult, duration: Duration) {
        self.entries.push(ReportEntry {
            name: check.name(),
            category: check.category(),
            status: result.status,
            message: result.message,
            suggestion: result.suggestion,
            has_fix: result.fix.is_some(),
            duration_ms: duration.as_millis() as u64,
        });
    }

    /// Highest exit-weight across all entries. Skip is treated as 0
    /// so a non-Linux host does not fail on Linux-only checks.
    pub fn summary(&self) -> CheckStatus {
        let mut worst = CheckStatus::Pass;
        for entry in &self.entries {
            if entry.status.exit_weight() > worst.exit_weight() {
                worst = entry.status;
            }
        }
        worst
    }

    pub fn counts(&self) -> ReportCounts {
        let mut counts = ReportCounts::default();
        for entry in &self.entries {
            match entry.status {
                CheckStatus::Pass => counts.pass += 1,
                CheckStatus::Warn => counts.warn += 1,
                CheckStatus::Fail => counts.fail += 1,
                CheckStatus::Skip => counts.skip += 1,
            }
        }
        counts
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ReportCounts {
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
    pub skip: usize,
}

/// The doctor runner. Construct with `for_level(level)`; the returned
/// value owns the check set for that level.
pub struct Doctor {
    level: DoctorLevel,
    checks: Vec<Box<dyn Check>>,
}

impl Doctor {
    /// Build the doctor for the given level. V0 only knows
    /// `Basic`; higher levels return an empty check set so the
    /// caller surfaces the "V1+" error.
    pub fn for_level(level: DoctorLevel) -> Self {
        let checks: Vec<Box<dyn Check>> = if level.is_shipped_in_v0() {
            crate::checks::basic_level_checks()
        } else {
            Vec::new()
        };
        Self { level, checks }
    }

    pub fn level(&self) -> DoctorLevel {
        self.level
    }

    pub fn checks(&self) -> &[Box<dyn Check>] {
        &self.checks
    }

    pub async fn run(&self, ctx: &CheckContext) -> DoctorReport {
        let mut report = DoctorReport::new(self.level);
        for check in &self.checks {
            let start = std::time::Instant::now();
            let result = check.run(ctx).await;
            report.add(check.as_ref(), result, start.elapsed());
        }
        report
    }
}
