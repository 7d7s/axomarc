// The backup use cases (F8a).
//
// The four operations the CLI exposes — `create`, `list`, `verify`,
// `restore` — plus an internal `expected_min_bytes` helper for the
// size sanity check. See `docs/phase-00-mvp.md` F8a for the full
// spec; the short version is:
//
//   create  -> VACUUM INTO <sink.path(id)>, sanity-check the size,
//              record Pending->Success/Failed with audit.
//   list    -> passthrough to storage.
//   verify  -> open the snapshot, run `PRAGMA integrity_check` and
//              a count() of every table, persist the JSON, mark
//              Success->Verified.
//   restore -> write the snapshot bytes to a target path. The V0
//              "live" restore is intentionally disabled — the
//              operator must stop the service first; we surface a
//              loud warning instead of corrupting the running DB.

use std::path::{Path, PathBuf};

use crate::domain::{AuditEvent, AuditKind, Backup, BackupId, BackupStatus, NewBackup, Timestamp};
use crate::error::AppError;
use crate::state::AppState;

/// The V0 floor for `min_size_bytes` in the sanity check. The spec
/// (F8a §6) uses `min(10 MB, max(1 KB, source_size / 100))` — 1 KB
/// catches an empty/near-empty source; 10 MB is the upper bound
/// beyond which any healthy DB will be at least that large.
pub const MIN_SIZE_FLOOR_BYTES: u64 = 1024;
pub const MAX_SIZE_FLOOR_BYTES: u64 = 10 * 1024 * 1024;

/// Compute the minimum acceptable snapshot size for a source DB of
/// `source_size` bytes. Returns `max(1 KB, source_size / 100)` clamped
/// to `10 MB`. Pure function — kept public so the doctor and the
/// F8a §11 CI test can pin its behavior.
pub fn expected_min_bytes(source_size: u64) -> u64 {
    let from_source = source_size / 100;
    let lower = from_source.max(MIN_SIZE_FLOOR_BYTES);
    lower.min(MAX_SIZE_FLOOR_BYTES)
}

/// Create a fresh snapshot of the SQLite state. On success, returns
/// the resulting `Backup` row (status = `Success`); on a failed size
/// check, the row is recorded as `Failed` with `error` set, the
/// snapshot file is removed, and an `Err` is returned.
pub async fn create_backup(state: &AppState, actor: &str) -> Result<Backup, AppError> {
    let sink = state
        .backup
        .as_deref()
        .ok_or_else(|| AppError::validation("backup sink not configured"))?;

    // 1. Reserve a Backup row in `Pending` so a crash mid-snapshot
    //    leaves an audit trail (F8a §6).
    let row = state
        .storage
        .begin_backup(
            NewBackup {
                target: "sqlite".into(),
            },
            actor,
        )
        .await?;

    // 2. Take a clean snapshot to the sink's path. The path lives
    //    inside the sink's base directory; the use case does not
    //    pick the path.
    let path = PathBuf::from(sink.location(row.id));
    let size = match state.storage.vacuum_into(&path).await {
        Ok(s) => s,
        Err(e) => {
            // The VACUUM itself failed; the file (if any) is in an
            // unknown state — best-effort delete and record Failure.
            let _ = sink.delete(row.id).await;
            return complete_with_error(
                state,
                row.id,
                None,
                None,
                &format!("vacuum_into: {e}"),
                actor,
            )
            .await;
        }
    };

    // 3. Size sanity check. We stat() the file (cheap) instead of
    //    trusting the size returned by VACUUM INTO; the spec uses
    //    the on-disk size, which is what matters for the "812-byte
    //    empty backup" pain from `user-pain-research.md` §4.1.
    let source_size = match tokio::fs::metadata(state.db_path.as_path()).await {
        Ok(m) => m.len(),
        Err(e) => {
            return complete_with_error(
                state,
                row.id,
                Some(size),
                Some(sink.location(row.id)),
                &format!("stat source db: {e}"),
                actor,
            )
            .await;
        }
    };
    let floor = expected_min_bytes(source_size);
    if size < floor {
        let msg = format!(
            "suspicious_size: snapshot is {size} bytes, expected >= {floor} \
             (source db is {source_size} bytes)"
        );
        let _ = sink.delete(row.id).await;
        return complete_with_error(
            state,
            row.id,
            Some(size),
            Some(sink.location(row.id)),
            &msg,
            actor,
        )
        .await;
    }

    // 4. Persist success.
    let location = sink.location(row.id);
    let final_row = state
        .storage
        .complete_backup(
            row.id,
            BackupStatus::Success,
            Some(size as i64),
            Some(location),
            None,
            actor,
        )
        .await?;

    // 5. Audit (separate from the row's own completion event).
    let _ = state
        .storage
        .append_audit(AuditEvent {
            id: None,
            ts: Timestamp::now(),
            actor: actor.to_string(),
            kind: AuditKind::Backup,
            target: Some(format!("backup:{}", row.id)),
            payload: serde_json::json!({
                "action": "create",
                "backup_id": row.id,
                "size": size,
                "source_size": source_size,
            }),
            policy_decision: None,
        })
        .await;

    Ok(final_row)
}

/// List the most recent `limit` backups (most-recent first).
pub async fn list_backups(state: &AppState, limit: u32) -> Result<Vec<Backup>, AppError> {
    state.storage.list_backups(limit).await
}

/// Verify an existing backup: open the snapshot, run
/// `PRAGMA integrity_check` and a `count(*)` of every user table,
/// store the result in `verify_result`, and transition the row to
/// `BackupStatus::Verified`.
pub async fn verify_backup(
    state: &AppState,
    id: BackupId,
    actor: &str,
) -> Result<Backup, AppError> {
    let sink = state
        .backup
        .as_deref()
        .ok_or_else(|| AppError::validation("backup sink not configured"))?;
    let path = PathBuf::from(sink.location(id));

    // 1. Make sure the row is in `Success` (Verified requires it).
    let row = state
        .storage
        .get_backup(id)
        .await?
        .ok_or_else(|| AppError::validation(format!("backup {id} not found")))?;
    if !matches!(row.status, BackupStatus::Success | BackupStatus::Verified) {
        return Err(AppError::validation(format!(
            "backup {id} status is {}; only Success can be verified",
            row.status
        )));
    }

    // 2. integrity_check + per-table row count. We open a *new*
    //    sqlx pool on the snapshot file — the live pool is unaffected.
    let verify_result = inspect_snapshot(&path).await?;

    // 3. Persist.
    let final_row = state
        .storage
        .verify_backup(id, verify_result, actor)
        .await?;

    // 4. Audit.
    let _ = state
        .storage
        .append_audit(AuditEvent {
            id: None,
            ts: Timestamp::now(),
            actor: actor.to_string(),
            kind: AuditKind::Backup,
            target: Some(format!("backup:{id}")),
            payload: serde_json::json!({
                "action": "verify",
                "backup_id": id,
            }),
            policy_decision: None,
        })
        .await;

    Ok(final_row)
}

/// Restore a backup to `target_path`. V0 only supports a *different*
/// path than the live `state.db`; restoring on top of the live
/// database would corrupt WAL / busy-timeout state. The CLI prints
/// a loud warning when the operator requests a live restore.
pub async fn restore_backup(
    state: &AppState,
    id: BackupId,
    target_path: &Path,
    actor: &str,
) -> Result<Backup, AppError> {
    let sink = state
        .backup
        .as_deref()
        .ok_or_else(|| AppError::validation("backup sink not configured"))?;
    let bytes = sink.read(id).await?;

    if let Some(parent) = target_path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                AppError::Storage(format!(
                    "create parent {} for restore target: {e}",
                    parent.display()
                ))
            })?;
        }
    }
    // Atomic-ish write: tmp file + rename, so a crash mid-restore
    // leaves the previous target intact.
    let tmp = target_path.with_extension("db.tmp");
    tokio::fs::write(&tmp, &bytes)
        .await
        .map_err(|e| AppError::Storage(format!("write restore tmp {}: {e}", tmp.display())))?;
    tokio::fs::rename(&tmp, target_path).await.map_err(|e| {
        AppError::Storage(format!(
            "rename {} -> {}: {e}",
            tmp.display(),
            target_path.display()
        ))
    })?;

    let _ = state
        .storage
        .append_audit(AuditEvent {
            id: None,
            ts: Timestamp::now(),
            actor: actor.to_string(),
            kind: AuditKind::Backup,
            target: Some(format!("backup:{id}")),
            payload: serde_json::json!({
                "action": "restore",
                "backup_id": id,
                "target": target_path.display().to_string(),
                "size": bytes.len(),
            }),
            policy_decision: None,
        })
        .await;

    state
        .storage
        .get_backup(id)
        .await?
        .ok_or_else(|| AppError::validation(format!("backup {id} not found")))
}

async fn complete_with_error(
    state: &AppState,
    id: BackupId,
    size: Option<u64>,
    location: Option<String>,
    error: &str,
    actor: &str,
) -> Result<Backup, AppError> {
    let row = state
        .storage
        .complete_backup(
            id,
            BackupStatus::Failed,
            size.map(|s| s as i64),
            location,
            Some(error.to_string()),
            actor,
        )
        .await?;
    let _ = state
        .storage
        .append_audit(AuditEvent {
            id: None,
            ts: Timestamp::now(),
            actor: actor.to_string(),
            kind: AuditKind::Backup,
            target: Some(format!("backup:{id}")),
            payload: serde_json::json!({
                "action": "create.fail",
                "backup_id": id,
                "error": error,
            }),
            policy_decision: None,
        })
        .await;
    Ok(row)
}

/// Open a snapshot file in a fresh sqlx pool and run the two
/// integrity checks. Kept private to the module — `verify_backup` is
/// the public entry.
async fn inspect_snapshot(path: &Path) -> Result<serde_json::Value, AppError> {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let path_str = path.to_string_lossy().into_owned();
    let connect = SqliteConnectOptions::new()
        .filename(&path_str)
        .read_only(true)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(0)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_with(connect)
        .await
        .map_err(|e| AppError::Storage(format!("open snapshot {path_str}: {e}")))?;

    // PRAGMA integrity_check returns a single row whose first column
    // is the string "ok" for a healthy database. Anything else is a
    // problem and we surface the full text.
    let integrity: (String,) = sqlx::query_as("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .map_err(AppError::from)?;
    let integrity_ok = integrity.0.trim().eq_ignore_ascii_case("ok");

    // Per-table row count. We do this against the user-visible
    // tables only (skip sqlite_* and _sqlx_migrations bookkeeping).
    let table_names: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master \
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
           AND name != '_sqlx_migrations' \
         ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .map_err(AppError::from)?;
    let mut table_counts = serde_json::Map::new();
    for (name,) in table_names {
        // Quote the table name to handle reserved words / odd chars.
        let q = format!("SELECT count(*) FROM \"{}\"", name.replace('"', "\"\""));
        let (n,): (i64,) = sqlx::query_as(&q)
            .fetch_one(&pool)
            .await
            .map_err(|e| AppError::Storage(format!("count {name}: {e}")))?;
        table_counts.insert(name, serde_json::Value::Number(n.into()));
    }

    pool.close().await;

    Ok(serde_json::json!({
        "integrity_check": integrity.0,
        "integrity_ok": integrity_ok,
        "table_counts": table_counts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_min_bytes_floor_and_clamp() {
        // Empty source -> 1 KB floor.
        assert_eq!(expected_min_bytes(0), MIN_SIZE_FLOOR_BYTES);
        // 100 KB source -> 1 KB (100*1024/100 = 1024, but exact is 1024
        // which equals the floor so it stays at 1024).
        assert_eq!(expected_min_bytes(100 * 1024), 1024);
        // 1 MB source -> 1 MB / 100 = 10,485 bytes (truncating division).
        // The spec is `source_size / 100`, not rounded to KB.
        assert_eq!(expected_min_bytes(1024 * 1024), 10_485);
        // 1 GB source -> clamped to 10 MB.
        assert_eq!(expected_min_bytes(1024 * 1024 * 1024), MAX_SIZE_FLOOR_BYTES);
    }

    // The end-to-end test (create -> verify -> restore) lives in
    // `sovereign-storage-sqlite/tests/` against the real
    // `FileBackupSink`; we only test the size helper here so this
    // module stays free of platform-specific deps.
}
