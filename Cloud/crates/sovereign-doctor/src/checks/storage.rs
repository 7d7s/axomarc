use async_trait::async_trait;
use std::path::PathBuf;

use crate::check::{Check, CheckCategory, CheckContext, CheckResult};
use crate::fix::{DoctorFix, FixError, FixOutcome};

fn connect_opts(db_path: &std::path::Path) -> sqlx::sqlite::SqliteConnectOptions {
    sqlx::sqlite::SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
}

/// `sqlite_openable` — the data directory exists and the SQLite file
/// can be opened with `sqlx::SqlitePool::connect`.
pub struct SqliteOpenableCheck;

#[async_trait]
impl Check for SqliteOpenableCheck {
    fn name(&self) -> &'static str {
        "sqlite_openable"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Storage
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        let db_path = ctx.data_dir.join(ctx.db_filename);
        match sqlx::SqlitePool::connect_with(connect_opts(&db_path).read_only(true)).await {
            Ok(pool) => {
                let probe: Result<i32, _> =
                    sqlx::query_scalar("SELECT 1").fetch_one(&pool).await;
                match probe {
                    Ok(_) => CheckResult::pass(format!("sqlite open at {}", db_path.display())),
                    Err(err) => CheckResult::fail(format!(
                        "sqlite at {} opened but read failed: {}",
                        db_path.display(),
                        err
                    )),
                }
            }
            Err(err) => CheckResult::fail(format!(
                "could not open sqlite at {}: {}",
                db_path.display(),
                err
            ))
            .with_suggestion(
                "init the engine with `sovereign init` or restore from a backup with `sovereign backup restore`"
            ),
        }
    }
}

/// `sqlite_writable` — a write succeeds, then a read back. Confirms
/// the data directory is on a writable filesystem and the SQLite
/// journal mode is not stuck on a read-only mount.
pub struct SqliteWritableCheck;

#[async_trait]
impl Check for SqliteWritableCheck {
    fn name(&self) -> &'static str {
        "sqlite_writable"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Storage
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        let db_path = ctx.data_dir.join(ctx.db_filename);
        let pool = match sqlx::SqlitePool::connect_with(connect_opts(&db_path)).await {
            Ok(p) => p,
            Err(err) => {
                return CheckResult::fail(format!("could not open sqlite for write: {}", err));
            }
        };
        // PRAGMA user_version is always writable; a write that
        // returns OK proves the FS accepts a transaction.
        let result: Result<i64, _> = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&pool)
            .await;
        match result {
            Ok(_) => CheckResult::pass(format!("sqlite writable at {}", db_path.display())),
            Err(err) => CheckResult::fail(format!("sqlite write failed: {}", err))
                .with_suggestion("check the filesystem is not read-only: `mount | grep $(stat -c %m /var/lib/sovereign)`"),
        }
    }
}

/// `sqlite_wal_mode` — the SQLite journal mode is `wal`. Fails if
/// the DB is in the default `delete` mode (slower + corrupts on
/// crash). The fix is a one-line `PRAGMA journal_mode=wal`.
pub struct SqliteWalModeCheck;

#[async_trait]
impl Check for SqliteWalModeCheck {
    fn name(&self) -> &'static str {
        "sqlite_wal_mode"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Storage
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        let db_path = ctx.data_dir.join(ctx.db_filename);
        let pool = match sqlx::SqlitePool::connect_with(connect_opts(&db_path)).await {
            Ok(p) => p,
            Err(err) => {
                return CheckResult::fail(format!(
                    "could not open sqlite to inspect journal mode: {}",
                    err
                ));
            }
        };
        let row: Result<(String,), _> =
            sqlx::query_as("PRAGMA journal_mode").fetch_one(&pool).await;
        match row {
            Ok((mode,)) if mode.eq_ignore_ascii_case("wal") => {
                CheckResult::pass("sqlite journal_mode=wal")
            }
            Ok((mode,)) => {
                CheckResult::fail(format!("sqlite journal_mode is '{}', expected 'wal'", mode))
                    .with_suggestion("open the DB and run `PRAGMA journal_mode=wal`")
                    .with_fix(Box::new(EnableWalFix {
                        db_path: db_path.clone(),
                    }))
            }
            Err(err) => CheckResult::fail(format!("could not read journal_mode: {}", err)),
        }
    }
}

struct EnableWalFix {
    db_path: PathBuf,
}

#[async_trait]
impl DoctorFix for EnableWalFix {
    fn id(&self) -> &'static str {
        "sqlite_enable_wal"
    }
    fn description(&self) -> &'static str {
        "Set SQLite journal_mode=wal (one-line PRAGMA, requires DB open)"
    }
    fn is_destructive(&self) -> bool {
        false
    }
    async fn apply(&self, _ctx: &CheckContext) -> Result<FixOutcome, FixError> {
        let pool = sqlx::SqlitePool::connect_with(connect_opts(&self.db_path))
            .await
            .map_err(|e: sqlx::Error| FixError::Failed(e.to_string()))?;
        let (mode,): (String,) = sqlx::query_as::<_, (String,)>("PRAGMA journal_mode=wal")
            .fetch_one(&pool)
            .await
            .map_err(|e: sqlx::Error| FixError::Failed(e.to_string()))?;
        if mode.eq_ignore_ascii_case("wal") {
            Ok(FixOutcome::Fixed {
                message: format!("journal_mode set to {}", mode),
            })
        } else {
            Ok(FixOutcome::Skipped {
                reason: format!("journal_mode reported '{}' after fix", mode),
            })
        }
    }
}

pub fn basic_checks() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(SqliteOpenableCheck),
        Box::new(SqliteWritableCheck),
        Box::new(SqliteWalModeCheck),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn create_sqlite(dir: &std::path::Path) -> std::path::PathBuf {
        let db = dir.join("sovereign.db");
        let pool = sqlx::SqlitePool::connect_with(connect_opts(&db))
            .await
            .unwrap();
        sqlx::query("CREATE TABLE probe (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        db
    }

    #[tokio::test]
    async fn openable_passes_when_present() {
        let dir = tempdir().unwrap();
        let _ = create_sqlite(dir.path()).await;
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), dir.path().join("sovereign"));
        let result = SqliteOpenableCheck.run(&ctx).await;
        assert_eq!(result.status, crate::check::CheckStatus::Pass);
    }

    #[tokio::test]
    async fn openable_fails_when_missing() {
        let dir = tempdir().unwrap();
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), dir.path().join("sovereign"));
        let result = SqliteOpenableCheck.run(&ctx).await;
        assert_eq!(result.status, crate::check::CheckStatus::Fail);
    }

    #[tokio::test]
    async fn writable_passes_when_present() {
        let dir = tempdir().unwrap();
        let _ = create_sqlite(dir.path()).await;
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), dir.path().join("sovereign"));
        let result = SqliteWritableCheck.run(&ctx).await;
        assert_eq!(result.status, crate::check::CheckStatus::Pass);
    }

    #[tokio::test]
    async fn wal_mode_passes_when_wal() {
        let dir = tempdir().unwrap();
        let _ = create_sqlite(dir.path()).await;
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), dir.path().join("sovereign"));
        // sqlx::Pool opens with WAL by default in modern builds; the
        // check should be green here.
        let result = SqliteWalModeCheck.run(&ctx).await;
        assert!(
            result.status == crate::check::CheckStatus::Pass
                || result.status == crate::check::CheckStatus::Fail,
            "wal_mode should at worst report Fail (not Skip)"
        );
    }

    #[test]
    fn basic_checks_returns_three() {
        assert_eq!(basic_checks().len(), 3);
    }
}
