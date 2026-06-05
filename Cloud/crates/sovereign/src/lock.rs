// F5 sub-task 5: write the `sovereign.lock` "deploy receipt" file.
//
// The lock file is the single source of truth for "what is running
// in production" — when the user opens their repo, the latest healthy
// deployment is at the top of the working tree. `git log sovereign.lock`
// shows the entire deploy history.
//
// Format (per `docs/phase-00-mvp.md` line 690):
//
//     {"deployment_id": "dep-009", "image": "ghcr.io/me/api:v1.4.2",
//      "deployed_at": "2026-06-03T14:22:01Z", "actor": "user:alice",
//      "sovereign_version": "1.0.0"}
//
// Spec also requires a post-write git commit (`git add sovereign.lock
// && git commit -m "deploy: <id>" && git push`). We run it best-effort:
// a missing `git`, a detached HEAD, a failed `push`, or no `git_repo`
// at all is logged as a warning — the file itself is the receipt, the
// commit is just a nice-to-have for `git log` forensics.

use std::path::{Path, PathBuf};
use std::process::Command;

use sovereign_core::domain::Deployment;
use sovereign_core::error::AppError;
use tracing::warn;

/// The exact JSON shape mandated by F5 sub-task 5. Rendered as a
/// single line so `git diff` shows the whole change in one row.
#[derive(serde::Serialize)]
pub(crate) struct LockFile<'a> {
    pub deployment_id: &'a str,
    pub image: &'a str,
    /// ISO-8601 UTC, second precision. We hand-roll the formatter
    /// to avoid a chrono dep in the binary (chrono is the
    /// observability adapter's concern).
    pub deployed_at: String,
    pub actor: &'a str,
    pub sovereign_version: &'static str,
}

/// Result of a `write` call. The CLI surfaces this as a one-line
/// status message so the user can tell whether the file was
/// written and whether the git commit/push succeeded.
pub(crate) struct LockWriteOutcome {
    pub path: PathBuf,
    pub committed: bool,
    pub pushed: bool,
}

/// Write the `sovereign.lock` file. Returns the absolute path used
/// and whether the optional git commit / push succeeded.
///
/// `git_repo` is the app's `git_repo` field (V0.5 sets it; V0.0
/// may not have it). The file itself is always written to the CWD
/// — the deploy command always runs from the repo root in practice.
pub(crate) fn write(
    app_name: &str,
    dep: &Deployment,
    actor: &str,
    git_repo: Option<&str>,
) -> Result<LockWriteOutcome, AppError> {
    let path = lock_path();
    let body = render(app_name, dep, actor);

    std::fs::write(&path, body.as_bytes())
        .map_err(|e| AppError::Upstream(format!("write {}: {e}", path.display())))?;

    // Best-effort git commit + push. We never fail the deploy over
    // a missing git — the file is the receipt, the commit is the
    // `git log` view.
    let (committed, pushed) = match git_repo {
        Some(repo) if has_git() => {
            let c = git_commit(repo, dep);
            let p = if c { git_push(repo) } else { false };
            (c, p)
        }
        _ => (false, false),
    };

    Ok(LockWriteOutcome {
        path,
        committed,
        pushed,
    })
}

/// Resolve the absolute path of the lock file. CWD-relative
/// `sovereign.lock` is the spec's contract; the operator's repo
/// root is always CWD in practice.
fn lock_path() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.join("sovereign.lock")
}

fn render(app_name: &str, dep: &Deployment, actor: &str) -> String {
    let entry = LockFile {
        deployment_id: &format!("{}/{}", app_name, dep.id),
        image: &dep.image_ref,
        deployed_at: format_iso8601_utc(dep.started_at),
        actor,
        sovereign_version: env!("CARGO_PKG_VERSION"),
    };
    serde_json::to_string(&entry).expect("LockFile is always serializable")
}

fn format_iso8601_utc(ts: sovereign_core::domain::Timestamp) -> String {
    let (year, month, day, hour, min, sec) = epoch_to_ymdhms(ts.as_secs());
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Howard Hinnant's `date.h` algorithm (public domain). Same as the
/// copy in `commands_rollback.rs` and `output.rs`.
fn epoch_to_ymdhms(epoch_secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = epoch_secs / 86400;
    let secs_of_day = (epoch_secs % 86400).rem_euclid(86400) as u32;
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y } as i32;
    (year, m, d, hour, min, sec)
}

fn has_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git_commit(repo: &str, dep: &Deployment) -> bool {
    let status = Command::new("git")
        .args(["-C", repo, "add", "sovereign.lock"])
        .status();
    match status {
        Ok(s) if s.success() => {}
        _ => {
            warn!("git add sovereign.lock failed in {repo}; skipping commit");
            return false;
        }
    }
    let msg = format!("deploy: {}", dep.id);
    let commit = Command::new("git")
        .args(["-C", repo, "commit", "-m", &msg])
        .status();
    match commit {
        Ok(s) if s.success() => true,
        Ok(s) => {
            // "nothing to commit" is fine — the file may be identical.
            if s.code() == Some(1) {
                true
            } else {
                warn!("git commit failed in {repo}");
                false
            }
        }
        Err(e) => {
            warn!("git commit spawn error: {e}");
            false
        }
    }
}

fn git_push(repo: &str) -> bool {
    match Command::new("git")
        .args(["-C", repo, "push"])
        .status()
    {
        Ok(s) if s.success() => true,
        Ok(_) => {
            warn!("git push failed in {repo} (no upstream? auth?)");
            false
        }
        Err(e) => {
            warn!("git push spawn error: {e}");
            false
        }
    }
}

/// Lightweight read of an existing `sovereign.lock` — used by
/// `sovereign doctor` in a later feature. We expose it now so the
/// public shape is stable. Returns `None` if the file doesn't exist
/// or isn't valid JSON; never panics.
pub(crate) fn read(path: &Path) -> Option<serde_json::Value> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_is_single_line_json() {
        let dep = Deployment {
            id: sovereign_core::domain::DeploymentId::generate(),
            app_id: sovereign_core::domain::AppId::generate(),
            image_ref: "ghcr.io/me/api:v1".to_string(),
            strategy: sovereign_core::domain::Strategy::BlueGreen,
            status: sovereign_core::domain::DeploymentStatus::Healthy,
            started_at: sovereign_core::domain::Timestamp(1_780_582_920),
            finished_at: Some(sovereign_core::domain::Timestamp(1_780_583_020)),
            triggered_by: "user:alice".to_string(),
            risk_score: None,
            policy_decision: None,
            error: None,
            target_deployment_id: None,
            version: 1,
        };
        let body = render("api", &dep, "user:alice");
        // F5 spec: single line, no trailing newline.
        assert!(!body.contains('\n'), "lock must be a single line: {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["image"], "ghcr.io/me/api:v1");
        assert_eq!(parsed["deployed_at"], "2026-06-04T14:22:00Z");
        assert_eq!(parsed["actor"], "user:alice");
        assert!(parsed["deployment_id"].as_str().unwrap().starts_with("api/"));
        assert!(parsed["sovereign_version"].is_string());
    }

    #[test]
    fn format_iso8601_matches_spec_example() {
        // The F5 spec example: "2026-06-03T14:22:01Z"
        let ts = sovereign_core::domain::Timestamp(1_780_496_521);
        assert_eq!(format_iso8601_utc(ts), "2026-06-03T14:22:01Z");
    }
}
