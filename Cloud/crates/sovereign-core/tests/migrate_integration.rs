//! Integration tests for the V0.1.0 → V0.5 master-key
//! migration use case. We exercise the full flow against a
//! real sqlite storage + the real age secrets adapter: V0.1.0
//! identity decrypts the existing ciphertexts, V0.5 identity
//! re-encrypts them, and the master key file is swapped
//! in-place.

use std::sync::Arc;

use age::x25519::Identity;
use secrecy::ExposeSecret;
use sovereign_core::domain::{AppEnv, AuditQuery, NewApp, NewSecret, SecretStatus};
use sovereign_core::ports::{SecretsPort, StoragePort};
use sovereign_core::use_cases::migrate::re_encrypt_all_secrets;
use sovereign_secrets_age::AgeSecrets;
use sovereign_storage_sqlite::SqliteState;
use tempfile::tempdir;

fn sample_app(name: &str) -> NewApp {
    NewApp {
        name: name.to_string(),
        owner: "ci".to_string(),
        env: AppEnv::Dev,
        git_repo: None,
        image_ref: None,
        config_yaml: "kind: app\nname: api\n".to_string(),
        health_path: Some("/healthz".to_string()),
    }
}

async fn make_storage_with_secrets(
    secrets: Arc<dyn SecretsPort>,
    plaintexts: &[(&str, &str)],
) -> (tempfile::TempDir, Arc<SqliteState>, Arc<dyn StoragePort>) {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("sovereign.db");
    let state = Arc::new(SqliteState::open(&db_path).await.expect("open"));
    let storage: Arc<dyn StoragePort> = state.clone();

    let app = storage
        .create_app(sample_app("test-app"), "ci")
        .await
        .expect("create_app");

    for (key, value) in plaintexts {
        let ct = secrets.encrypt(value.as_bytes()).expect("encrypt");
        let _ = storage
            .put_secret(
                NewSecret {
                    app_id: app.id,
                    key: (*key).to_string(),
                    ciphertext: ct,
                },
                "ci",
            )
            .await
            .expect("put_secret");
    }
    (dir, state, storage)
}

#[tokio::test]
async fn migrate_round_trip_re_encrypts_plaintext_unchanged() {
    let dir = tempdir().expect("tempdir");
    let key_path = dir.path().join("master.key");
    let v0_id = Identity::generate();
    let v0_bech = v0_id.to_string();
    let v0_pp = v0_bech.expose_secret().to_string();
    std::fs::write(&key_path, format!("{v0_pp}\n").as_bytes()).expect("write v0");

    // 1. Encrypt two secrets under the V0.1.0 identity.
    let old_secrets: Arc<dyn SecretsPort> =
        Arc::new(AgeSecrets::with_identity(key_path.clone(), Arc::new(v0_id)));
    let (_d, _s, storage) = make_storage_with_secrets(
        old_secrets.clone(),
        &[
            ("DATABASE_URL", "postgres://u:p@db:5432/x"),
            ("API_KEY", "sk-very-secret"),
        ],
    )
    .await;

    // 2. Build a fresh V0.5 identity (not yet on disk; the
    //    migration use case writes the wrapped file as part
    //    of the flow, so we can build a second in-memory
    //    AgeSecrets for it).
    let new_id = Identity::generate();
    let new_secrets: Arc<dyn SecretsPort> = Arc::new(AgeSecrets::with_identity(
        dir.path().join("new.master.key"),
        Arc::new(new_id.clone()),
    ));

    // 3. Re-encrypt.
    let report = re_encrypt_all_secrets(
        storage.clone(),
        old_secrets.clone(),
        new_secrets.clone(),
        "ci",
    )
    .await
    .expect("migrate");
    assert_eq!(report.secrets_re_encrypted, 2);
    assert_eq!(report.apps_touched, 1);

    // 4. The new identity can decrypt the re-encrypted
    //    secrets back to the original plaintext.
    let app = storage
        .list_apps(None)
        .await
        .expect("list")
        .into_iter()
        .find(|a| a.name == "test-app")
        .expect("app");
    let rows = storage.list_secrets(app.id).await.expect("list_secrets");
    let active: Vec<_> = rows
        .iter()
        .filter(|s| s.status == SecretStatus::Active)
        .collect();
    assert_eq!(active.len(), 2, "two new active rows");
    let mut got: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for s in &active {
        let pt = new_secrets
            .decrypt(&s.ciphertext)
            .expect("decrypt with new");
        got.insert(s.key.clone(), String::from_utf8(pt).expect("utf8"));
    }
    assert_eq!(
        got.get("DATABASE_URL").map(|s| s.as_str()),
        Some("postgres://u:p@db:5432/x")
    );
    assert_eq!(
        got.get("API_KEY").map(|s| s.as_str()),
        Some("sk-very-secret")
    );

    // 5. The old V0.1.0 identity can NOT decrypt the new
    //    ciphertexts (regression guard for accidental reuse
    //    of the old key).
    for s in &active {
        let r = old_secrets.decrypt(&s.ciphertext);
        assert!(
            r.is_err(),
            "old identity must not be able to decrypt re-encrypted rows"
        );
    }
}

#[tokio::test]
async fn migrate_no_secrets_is_empty_report() {
    let dir = tempdir().expect("tempdir");
    let v0_id = Identity::generate();
    let old_secrets: Arc<dyn SecretsPort> = Arc::new(AgeSecrets::with_identity(
        dir.path().join("master.key"),
        Arc::new(v0_id),
    ));
    let (_d, _s, storage) = make_storage_with_secrets(old_secrets.clone(), &[]).await;

    let new_id = Identity::generate();
    let new_secrets: Arc<dyn SecretsPort> = Arc::new(AgeSecrets::with_identity(
        dir.path().join("new.master.key"),
        Arc::new(new_id),
    ));

    let report = re_encrypt_all_secrets(storage.clone(), old_secrets, new_secrets, "ci")
        .await
        .expect("migrate");
    assert_eq!(report.secrets_re_encrypted, 0);
    assert_eq!(report.apps_touched, 0);
    assert!(report.secrets.is_empty());
}

#[tokio::test]
async fn migrate_writes_master_key_migration_audit_event() {
    let dir = tempdir().expect("tempdir");
    let v0_id = Identity::generate();
    let old_secrets: Arc<dyn SecretsPort> = Arc::new(AgeSecrets::with_identity(
        dir.path().join("master.key"),
        Arc::new(v0_id),
    ));
    let (_d, _s, storage) = make_storage_with_secrets(old_secrets.clone(), &[("K1", "v1")]).await;

    let new_id = Identity::generate();
    let new_secrets: Arc<dyn SecretsPort> = Arc::new(AgeSecrets::with_identity(
        dir.path().join("new.master.key"),
        Arc::new(new_id),
    ));

    let _ = re_encrypt_all_secrets(storage.clone(), old_secrets, new_secrets, "ci")
        .await
        .expect("migrate");

    // Audit query: the migration event must be there.
    let q = AuditQuery {
        target: Some("master-key-migration".to_string()),
        ..Default::default()
    };
    let events = storage.query_audit(q).await.expect("query_audit");
    assert_eq!(events.len(), 1, "exactly one migration audit event");
    let e = &events[0];
    assert_eq!(e.actor, "ci");
    assert_eq!(e.target.as_deref(), Some("master-key-migration"));
    let payload = &e.payload;
    assert_eq!(
        payload.get("event").and_then(|v| v.as_str()),
        Some("v01_to_v05_master_key_migration")
    );
    assert_eq!(
        payload.get("secrets_re_encrypted").and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        payload.get("apps_touched").and_then(|v| v.as_u64()),
        Some(1)
    );
    // kdf object present
    assert!(payload.get("kdf").is_some());
    let kdf = payload.get("kdf").unwrap();
    assert_eq!(
        kdf.get("algorithm").and_then(|v| v.as_str()),
        Some("Argon2id")
    );
    assert_eq!(
        kdf.get("memory_kib").and_then(|v| v.as_u64()),
        Some(64 * 1024)
    );
    assert_eq!(kdf.get("iterations").and_then(|v| v.as_u64()), Some(3));
    assert_eq!(kdf.get("parallelism").and_then(|v| v.as_u64()), Some(1));
}

#[tokio::test]
async fn migrate_retires_old_rows_and_keeps_audit_trail() {
    let dir = tempdir().expect("tempdir");
    let v0_id = Identity::generate();
    let old_secrets: Arc<dyn SecretsPort> = Arc::new(AgeSecrets::with_identity(
        dir.path().join("master.key"),
        Arc::new(v0_id),
    ));
    let (_d, _s, storage) = make_storage_with_secrets(old_secrets.clone(), &[("K1", "v1")]).await;
    let app = storage
        .list_apps(None)
        .await
        .expect("list")
        .into_iter()
        .next()
        .expect("app");

    // Pre-migration: 1 active row.
    let pre = storage.list_secrets(app.id).await.expect("list");
    assert_eq!(pre.len(), 1);
    assert_eq!(pre[0].status, SecretStatus::Active);

    let new_id = Identity::generate();
    let new_secrets: Arc<dyn SecretsPort> = Arc::new(AgeSecrets::with_identity(
        dir.path().join("new.master.key"),
        Arc::new(new_id),
    ));

    let _ = re_encrypt_all_secrets(storage.clone(), old_secrets, new_secrets, "ci")
        .await
        .expect("migrate");

    // Post-migration: 2 rows total (1 retired + 1 new active).
    let post = storage.list_secrets(app.id).await.expect("list");
    assert_eq!(post.len(), 2, "old retired + new active");
    let retired = post
        .iter()
        .filter(|s| s.status == SecretStatus::Retired)
        .count();
    let active = post
        .iter()
        .filter(|s| s.status == SecretStatus::Active)
        .count();
    assert_eq!(retired, 1);
    assert_eq!(active, 1);
    // The audit log should have at least: 1 per-old-row status
    // change, 1 per-new-row insert, 1 migration event.
    let q = AuditQuery::default();
    let events = storage.query_audit(q).await.expect("query_audit");
    assert!(
        events.len() >= 3,
        "at least 3 audit rows, got {}",
        events.len()
    );
}
