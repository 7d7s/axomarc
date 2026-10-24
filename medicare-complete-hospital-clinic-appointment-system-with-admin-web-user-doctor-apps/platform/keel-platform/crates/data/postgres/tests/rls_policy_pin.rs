//! MNC-010 policy pin (no database required): no tenant policy in the
//! migration plane may end in the `IS NULL` branch.
//!
//! The branch made the final containment barrier vanish exactly when the
//! application misbehaved — a forgotten `SET LOCAL kelson.tenant_id` read
//! ALL tenants instead of failing closed (20261024000002_rls_deny_unset_tenant
//! drops it; platform access is the explicit `current_user = 'kelson_platform'`
//! arm instead). This test pins the posture: a future migration that
//! reintroduces the tautology fails here, before it reaches a database.

use std::path::{Path, PathBuf};

/// Walk up from this crate's manifest to the workspace root, then into
/// `database/migrations`.
fn migrations_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .ancestors()
        .find(|dir| dir.join("database").join("migrations").is_dir())
        .expect("the workspace carries database/migrations");
    root.join("database").join("migrations")
}

#[test]
fn no_tenant_policy_denies_by_leaving_the_guc_unset() {
    // Assembled from pieces so this gate's own source does not contain the
    // literal it bans (same technique as the xtask SQL-TAUTOLOGY gate).
    let banned_suffix = ["kelson_current_tenant()", " ", "IS NULL"].concat();
    // Applied migrations are checksummed (sqlx) — the foundation migration's
    // original policies are immutable history, superseded by the deny
    // migration. The pin therefore watches everything AFTER it: a future
    // migration that reintroduces the tautology fails here, before it reaches
    // a database. Doc-comment lines are prose, not policy.
    let deny_migration = "20261024000002";
    let dir = migrations_dir();
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("migrations are readable") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("sql") {
            continue;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if name.as_str() <= deny_migration {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("migration is utf-8");
        for (idx, line) in content.lines().enumerate() {
            let code = line.split("--").next().unwrap_or_default();
            if code.contains(&banned_suffix) {
                offenders.push(format!(
                    "{name}:{}: a tenant policy that matches every tenant when the GUC is unset (MNC-010)",
                    idx + 1
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the RLS barrier must fail closed on an unset GUC:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_deny_migration_exists_and_names_the_platform_arm() {
    let path = migrations_dir().join("20261024000002_rls_deny_unset_tenant.sql");
    let content = std::fs::read_to_string(&path)
        .expect("the MNC-010 deny migration is present and readable");
    // Eight tenant policies in the foundation migration, eight rewrites here.
    let drops = content.matches("DROP POLICY tenant_isolation_").count();
    let creates = content.matches("CREATE POLICY tenant_isolation_").count();
    assert_eq!((drops, creates), (8, 8), "every tenant policy is rewritten");
    assert!(
        content.contains("current_user = 'kelson_platform'"),
        "the break-glass platform plane keeps explicit access"
    );
}
