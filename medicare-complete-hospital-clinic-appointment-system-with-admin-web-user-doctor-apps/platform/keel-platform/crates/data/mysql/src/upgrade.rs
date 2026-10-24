//! The schema-version contract.
//!
//! A framework's hardest promise is that upgrading it does not break the
//! data. This module makes that promise concrete:
//!
//! * **the schema has a version** — a content digest over the migration
//!   set that is actually applied (not a hand-maintained number that can
//!   drift from reality);
//! * **the boot refuses a stale schema** — when the deployment does not
//!   migrate at boot (the production posture), a schema that is behind the
//!   framework's requirement is a **refusal**, not a warning: the host does
//!   not serve traffic against a schema it does not understand;
//! * **the upgrade is safe** — the pending migrations are applied through
//!   the guarded runner (every statement recorded, a failed run resumes)
//!   and a step that changes an existing table goes through the **online**
//!   path (the shadow table, the copy, the atomic swap) instead of an
//!   `ALTER` that rebuilds the table under a lock.
//!
//! The version is comparable across instances: two hosts with the same
//! applied set have the same version, so a rolling deploy can assert that
//! every instance sees the same schema.

use kelson_foundation::AppError;
use sqlx::Row;

use crate::guarded;
use crate::online;

/// The sqlx migrator's bookkeeping table (the singles).
const SQLX_TABLE: &str = "_sqlx_migrations";

/// What the schema currently is, and what it must become.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaState {
    /// The migrations that are applied (the singles and the pairs).
    pub applied: Vec<String>,
    /// The migrations that are on disk but not applied.
    pub pending: Vec<String>,
    /// The digest of the applied set — the schema's version.
    pub version: String,
    /// The digest of the set the framework requires.
    pub required_version: String,
}

impl SchemaState {
    /// Whether the schema matches what the framework requires.
    pub fn is_current(&self) -> bool {
        self.pending.is_empty()
    }

    /// A one-line description for a log or an error message.
    pub fn describe(&self) -> String {
        format!(
            "schema version {} ({} applied, {} pending; the framework requires {})",
            self.version,
            self.applied.len(),
            self.pending.len(),
            self.required_version
        )
    }
}

/// The digest of a migration set — the schema's version.
///
/// Two hosts with the same applied set hash the same, and a host whose
/// schema drifted hashes differently, so the comparison is exact rather
/// than a count.
pub fn version_of(migrations: &[String]) -> String {
    let mut sorted: Vec<&String> = migrations.iter().collect();
    sorted.sort();
    let joined = sorted
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    guarded::checksum(&joined)
}

/// The migrations the framework requires: the singles plus the pairs.
pub fn required_migrations(root: &std::path::Path) -> Result<Vec<String>, AppError> {
    let mut names: Vec<String> = Vec::new();

    // The singles: the sqlx migrator's own list.
    let migrator = sqlx::migrate!("../../../database/migrations-mysql");
    for m in migrator.iter() {
        names.push(m.version.to_string());
    }

    // The pairs: the rollback-capable shipment.
    for pair in crate::pairs::scan_pairs(root)? {
        names.push(pair.migration);
    }

    names.sort();
    names.dedup();
    Ok(names)
}

/// The migrations that are actually applied.
pub async fn applied_migrations<'a, A>(acquire: A) -> Result<Vec<String>, AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::MySql>,
    A::Connection: std::ops::DerefMut<Target = sqlx::MySqlConnection>,
{
    let mut conn = acquire
        .acquire()
        .await
        .map_err(|e| AppError::persistence(format!("schema state: connect: {e}")))?;
    let mut names: Vec<String> = Vec::new();

    // The singles (the table may not exist yet - a fresh database). The
    // version column is a BIGINT, so it is cast to text to match the
    // migrator's own version spelling.
    let rows = sqlx::query(&format!(
        "SELECT CAST(version AS CHAR) FROM {SQLX_TABLE} WHERE success = TRUE"
    ))
    .fetch_all(&mut *conn)
    .await;
    if let Ok(rows) = rows {
        for row in rows {
            if let Ok(version) = row.try_get::<String, _>(0) {
                names.push(version);
            }
        }
    }

    // The pairs.
    for (migration, _) in crate::pairs::applied(&mut *conn).await.unwrap_or_default() {
        names.push(migration);
    }

    names.sort();
    names.dedup();
    Ok(names)
}

/// The schema's state against the framework's requirement.
pub async fn schema_state<'a, A>(
    acquire: A,
    root: &std::path::Path,
) -> Result<SchemaState, AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::MySql>,
    A::Connection: std::ops::DerefMut<Target = sqlx::MySqlConnection>,
{
    let applied = applied_migrations(acquire).await?;
    let required = required_migrations(root)?;
    let pending: Vec<String> = required
        .iter()
        .filter(|r| !applied.contains(r))
        .cloned()
        .collect();
    Ok(SchemaState {
        version: version_of(&applied),
        required_version: version_of(&required),
        applied,
        pending,
    })
}

/// **Refuse** when the schema is behind what the framework requires.
///
/// This is the production posture: the deployment does not migrate at boot
/// (the migrations are a separate, gated step), so a stale schema must stop
/// the host rather than let it serve traffic against a schema it does not
/// understand.
pub async fn assert_current<'a, A>(acquire: A, root: &std::path::Path) -> Result<(), AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::MySql>,
    A::Connection: std::ops::DerefMut<Target = sqlx::MySqlConnection>,
{
    let state = schema_state(acquire, root).await?;
    if state.is_current() {
        return Ok(());
    }
    Err(AppError::persistence(format!(
        "the schema is behind what this framework requires: {} - pending: {} (run the migrations before \\
         starting this instance)",
        state.describe(),
        state.pending.join(", ")
    )))
}

/// Apply the pending PAIRS, with the online path for the DDL that changes
/// an existing table.
///
/// The singles (the sqlx migrator's own list) are applied by
/// [`crate::run_migrations`], which calls this for the pairs.
///
/// Returns the names of the migrations that were applied. The bookkeeping
/// row is written per migration ✓, and every step is recorded ✓ (so a
/// failed upgrade resumes ✓).
pub async fn upgrade_pairs<'a, A>(
    acquire: A,
    root: &std::path::Path,
) -> Result<Vec<String>, AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::MySql>,
    A::Connection: std::ops::DerefMut<Target = sqlx::MySqlConnection>,
{
    let mut conn = acquire
        .acquire()
        .await
        .map_err(|e| AppError::persistence(format!("upgrade: connect: {e}")))?;
    crate::pairs::install(&mut *conn).await?;
    guarded::install_steps(&mut *conn).await?;

    let applied = crate::pairs::applied(&mut *conn).await?;
    let scanned = crate::pairs::scan_pairs(root)?;
    let queued = crate::pairs::pending_pairs(&applied, &scanned);
    if queued.is_empty() {
        return Ok(Vec::new());
    }
    let next_batch = applied.iter().map(|(_, b)| *b).max().unwrap_or(0) + 1;

    let mut done: Vec<String> = Vec::new();
    for pair in queued {
        let verdict = apply_pair(&mut conn, &pair.migration, &pair.up_sql).await?;
        sqlx::query(&format!(
            "INSERT INTO {} (migration, batch) VALUES (?, ?)",
            crate::pairs::PAIRS_TABLE
        ))
        .bind(&pair.migration)
        .bind(next_batch)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            AppError::persistence(format!(
                "{} (up): record: {e} ({})",
                pair.migration,
                guarded::NONTRANSACTIONAL_NOTE
            ))
        })?;
        let _ = verdict;
        done.push(pair.migration.clone());
    }
    Ok(done)
}

/// Apply one pair's statements: a step that changes an existing table goes
/// through the **online** path (the shadow table, the copy, the atomic
/// swap); every other step runs directly. Each step is recorded.
async fn apply_pair(
    conn: &mut sqlx::MySqlConnection,
    migration: &str,
    up_sql: &str,
) -> Result<guarded::PairVerdict, AppError> {
    let statements = guarded::split_statements(up_sql);
    let recorded = guarded::step_state(&mut *conn, migration).await?;
    let mut applied_steps = Vec::new();
    let mut resumed_steps = Vec::new();
    let mut auto_committing_steps = Vec::new();

    for (index, statement) in statements.iter().enumerate() {
        if guarded::auto_commits(statement) {
            auto_committing_steps.push(index);
        }
        let digest = guarded::checksum(statement);
        if let Some(previous) = recorded.iter().find(|r| r.step == index) {
            if previous.checksum != digest {
                return Err(AppError::persistence(format!(
                    "{migration}: step {index} changed since it was applied; refusing to resume"
                )));
            }
            resumed_steps.push(index);
            continue;
        }

        // The step that changes an existing table takes the online path.
        if let Some((table, rest)) = guarded::alter_target(statement) {
            let key = online::primary_key_of(conn, &table).await?;
            let change = online::OnlineChange::new(&table, &rest, &key);
            online::apply_online(&mut *conn, &change)
                .await
                .map_err(|e| {
                    AppError::persistence(format!(
                        "{migration}: step {index} (the online change on {table}) failed: {e}"
                    ))
                })?;
        } else {
            sqlx::raw_sql(statement)
                .execute(&mut *conn)
                .await
                .map_err(|e| {
                    AppError::persistence(format!(
                        "{migration}: step {index} failed: {e} ({})",
                        guarded::NONTRANSACTIONAL_NOTE
                    ))
                })?;
        }

        guarded::record_step(conn, migration, index, &digest).await?;
        applied_steps.push(index);
    }

    let transactional = auto_committing_steps.is_empty();
    Ok(guarded::PairVerdict {
        migration: migration.to_string(),
        applied_steps,
        resumed_steps,
        auto_committing_steps,
        transactional,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_a_digest_of_the_applied_set() {
        let a = vec![
            "20260101000000_a".to_string(),
            "20260102000000_b".to_string(),
        ];
        let b = vec![
            "20260102000000_b".to_string(),
            "20260101000000_a".to_string(),
        ];
        assert_eq!(version_of(&a), version_of(&b), "the order does not matter");
        let c = vec!["20260101000000_a".to_string()];
        assert_ne!(
            version_of(&a),
            version_of(&c),
            "a different set is a different version"
        );
    }

    #[test]
    fn the_version_changes_when_a_migration_is_added() {
        let before = version_of(&["a".to_string()]);
        let after = version_of(&["a".to_string(), "b".to_string()]);
        assert_ne!(before, after);
    }

    #[test]
    fn the_state_describes_itself() {
        let state = SchemaState {
            applied: vec!["a".into()],
            pending: vec!["b".into()],
            version: "v1".into(),
            required_version: "v2".into(),
        };
        assert!(!state.is_current());
        assert!(state.describe().contains("1 pending"));
    }
}
