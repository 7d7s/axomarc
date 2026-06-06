use std::collections::BTreeMap;

use sovereign_core::ports::{
    update::Binary, Release, UpdateChannel, UpdateError, UpdatePort, Version,
};
use sovereign_update::MockUpdate;
use tempfile::tempdir;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn mock_current_version_matches_constructor() {
    let m = MockUpdate::new(Version(0, 5, 0));
    assert_eq!(m.current_version().await.unwrap(), Version(0, 5, 0));
}

#[tokio::test]
async fn mock_latest_returns_what_was_set() {
    let release = Release {
        version: "0.5.0".into(),
        channel: "stable".into(),
        released_at: "2026-01-01T00:00:00Z".into(),
        binaries: BTreeMap::new(),
    };
    let m = MockUpdate::new(Version(0, 1, 0)).with_manifest(UpdateChannel::Stable, release);
    let latest = m.latest(UpdateChannel::Stable).await.unwrap();
    assert_eq!(latest.version, "0.5.0");
}

#[tokio::test]
async fn mock_latest_errors_on_unknown_channel() {
    let m = MockUpdate::new(Version(0, 1, 0));
    let err = m.latest(UpdateChannel::Rc).await.unwrap_err();
    assert!(matches!(err, UpdateError::Network(_)));
}

#[tokio::test]
async fn mock_fetch_writes_expected_bytes() {
    let dir = tempdir().unwrap();
    let dest = dir.path().join("sovereign-new");
    let mut binaries = BTreeMap::new();
    binaries.insert(
        "x86_64-unknown-linux-musl".to_string(),
        Binary {
            url: "https://example.com/sovereign".into(),
            sha256: "MOCK".into(),
            size: 17,
        },
    );
    let release = Release {
        version: "0.1.1".into(),
        channel: "stable".into(),
        released_at: "2026-01-01T00:00:00Z".into(),
        binaries,
    };
    let m = MockUpdate::new(Version(0, 1, 0)).with_manifest(UpdateChannel::Stable, release);
    let sha = m
        .fetch(
            &m.latest(UpdateChannel::Stable).await.unwrap(),
            "x86_64-unknown-linux-musl",
            &dest,
        )
        .await
        .unwrap();
    let bytes = tokio::fs::read(&dest).await.unwrap();
    assert_eq!(bytes, b"MOCK-UPDATE-BINARY");
    assert_eq!(sha.to_string().len(), 64);
}

#[tokio::test]
#[cfg_attr(
    target_os = "windows",
    ignore = "windows locks the running test binary"
)]
async fn apply_then_rollback_round_trip() {
    let dir = tempdir().unwrap();
    let current = dir.path().join("sovereign");
    let backup = dir.path().join("backups");
    tokio::fs::create_dir(&backup).await.unwrap();
    let mut f = tokio::fs::File::create(&current).await.unwrap();
    f.write_all(b"OLD-BINARY-BYTES").await.unwrap();
    f.flush().await.unwrap();
    drop(f);
    let new = dir.path().join("sovereign-new");
    tokio::fs::write(&new, b"NEW-BINARY-BYTES").await.unwrap();
    let m = MockUpdate::new(Version(0, 1, 0));
    let record = m.apply(&new, &current, &backup).await.unwrap();
    let bytes_after_apply = tokio::fs::read(&current).await.unwrap();
    assert_eq!(bytes_after_apply, b"NEW-BINARY-BYTES");
    assert!(tokio::fs::metadata(&record.backup_path).await.is_ok());
    // The mock's rollback reads the backup file we just wrote, so
    // it should restore the OLD bytes.
    m.rollback(&record).await.unwrap();
    let bytes_after_rollback = tokio::fs::read(&current).await.unwrap();
    assert_eq!(bytes_after_rollback, b"OLD-BINARY-BYTES");
}
