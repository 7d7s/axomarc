//! Phase G (B-31): the base/twin MySQL migration divergence is an INVARIANT,
//! not an accident.
//!
//! The `platform` workspace (the medicare product) ships the current 9-file
//! MySQL set; this `keel-platform` twin ships a 12-file SUPERSET that adds the
//! three target-architecture Phases 2/3 migrations the product has not adopted
//! yet (§3.1 job tenant scoping, §5.2 event idempotency, §5.1 job payload
//! versioning). Each console pairs with its OWN schema — Phase F's live
//! promotion gate runs against the twin's 12, the base's dump/rollback gates
//! against the base's 9 — so the two sets must stay RELATED, not identical:
//!
//!   1. every base migration is present in the twin BYTE-IDENTICAL (no silent
//!      drift in the shared foundation), and
//!   2. the twin adds EXACTLY the three target-arch migrations, nothing more.
//!
//! This test locks both directions. If the product adopts a target-arch phase
//! (base 9 -> 10+), update `TWIN_ONLY` below; if a shared file changes,
//! regenerate the twin's copy to match before this test will pass. Pure
//! filesystem — no database — so it runs in every workspace gate.

use std::collections::BTreeSet;
use std::path::Path;

/// The three target-architecture migrations the twin adds over the base, in
/// version order. `20261005000001` = §3.1 jobs carry their tenant;
/// `20261005000002` = §5.2 outbox rows carry the append-time idempotency key;
/// `20261005000003` = §5.1 durable jobs carry their payload schema version.
const TWIN_ONLY: &[&str] = &[
    "20261005000001_jobs_queue_tenant.sql",
    "20261005000002_outbox_idempotency_key.sql",
    "20261005000003_jobs_queue_schema_version.sql",
];

/// Every `*.sql` file in `dir`, sorted (BTreeSet order), by file name.
fn sql_files(dir: &Path) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    for entry in std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: unreadable: {e}", dir.display()))
    {
        let entry = entry.unwrap_or_else(|e| panic!("{}: dir entry: {e}", dir.display()));
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".sql") {
            files.insert(name);
        }
    }
    files
}

#[test]
fn twin_mysql_migrations_superset_the_base_by_exactly_the_target_arch_phases() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    // crates/data/mysql -> platform/keel-platform/database (this twin's set)
    let twin_dir = manifest.join("../../../database/migrations-mysql");
    // crates/data/mysql -> platform/database (the base product's set)
    let base_dir = manifest.join("../../../../database/migrations-mysql");

    let base = sql_files(&base_dir);
    let twin = sql_files(&twin_dir);
    assert!(
        !base.is_empty(),
        "the base migration set came back empty: {}",
        base_dir.display()
    );

    // 1. Every base file is byte-identical in the twin (no shared drift).
    for file in &base {
        assert!(
            twin.contains(file),
            "base migration {file} is missing from the twin set ({}); the \
             twin must carry every base migration",
            twin_dir.display()
        );
        let base_bytes = std::fs::read(base_dir.join(file))
            .unwrap_or_else(|e| panic!("{file}: base read failed: {e}"));
        let twin_bytes = std::fs::read(twin_dir.join(file))
            .unwrap_or_else(|e| panic!("{file}: twin read failed: {e}"));
        assert_eq!(
            base_bytes, twin_bytes,
            "{file}: the twin's copy drifted from the base; the two \
             workspaces ship byte-identical shared migrations — regenerate \
             the twin's copy"
        );
    }

    // 2. The twin adds EXACTLY the three target-arch migrations, nothing more.
    let extras: BTreeSet<String> = twin.difference(&base).cloned().collect();
    let expected: BTreeSet<String> = TWIN_ONLY.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        extras, expected,
        "the twin's migration set must be the base's plus exactly the three \
         target-arch phases (§3.1 job tenant, §5.2 outbox idempotency, §5.1 \
         job schema version); if the product adopted a phase, update TWIN_ONLY"
    );
}
