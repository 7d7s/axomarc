//! PostgreSQL integration matrix (S7 B-19 foundation): durable stores are
//! exercised against a live database. Activated with `--features pg-tests`;
//! every test is a graceful skip when DATABASE_URL is unset, so local runs
//! without a database stay green and CI (service container) enforces them.

#![cfg(feature = "pg-tests")]

use keel_foundation::PrincipalId;
use keel_identity::store::{CredentialStore, PrincipalStore};
use keel_identity::{Credential, CredentialKind, Principal, PrincipalStatus, Secret};
use keel_postgres::{
    create_pool, run_migrations, PgCredentialStore, PgPool, PgPrincipalStore, PgSessionStore,
};
use keel_session::{SessionPolicy, SessionStore};
use time::OffsetDateTime;

async fn pool() -> Option<PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!("pg-tests: DATABASE_URL not set; skipping");
            return None;
        }
    };
    match create_pool(&url, 4).await {
        Ok(pool) => {
            run_migrations(&pool).await.expect("migrations apply");
            Some(pool)
        }
        Err(e) => {
            eprintln!("pg-tests: cannot connect ({e}); skipping");
            None
        }
    }
}

fn principal(email: &str) -> Principal {
    Principal {
        id: PrincipalId::new(),
        status: PrincipalStatus::Active,
        email: Some(email.into()),
        phone: None,
        email_verified_at: None,
        created_at: OffsetDateTime::now_utc(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_principal_and_credential_roundtrip() {
    let Some(pool) = pool().await else { return };
    let principals = PgPrincipalStore::new(pool.clone());
    let credentials = PgCredentialStore::new(pool.clone());
    let email = format!("pg-{}@it.test", uuid::Uuid::now_v7());

    let created = principals
        .create(principal(&email))
        .expect("principal inserts");
    assert_eq!(
        principals
            .by_id(created.id)
            .unwrap()
            .unwrap()
            .email
            .as_deref(),
        Some(email.as_str())
    );
    assert_eq!(principals.by_email(&email).unwrap().unwrap().id, created.id);
    assert!(principals.by_email(&email).is_ok());

    let dup = principals.create(principal(&email));
    assert!(dup.is_err(), "unique email enforced");

    let secret_hash = keel_identity::hash_password(&Secret::new("pg-password-99")).unwrap();
    credentials
        .create(Credential {
            id: uuid::Uuid::now_v7(),
            principal_id: created.id,
            kind: CredentialKind::PasswordArgon2,
            secret_hash: Secret::new(secret_hash.clone()),
            created_at: OffsetDateTime::now_utc(),
            expires_at: None,
        })
        .expect("credential inserts");
    let loaded = credentials
        .password_for(created.id)
        .unwrap()
        .expect("credential loads");
    assert!(keel_identity::verify_password(
        &Secret::new("pg-password-99"),
        loaded.secret_hash.expose()
    )
    .unwrap());

    let new_hash = keel_identity::hash_password(&Secret::new("rotated-password-1")).unwrap();
    credentials
        .replace_password_hash(created.id, Secret::new(new_hash.clone()))
        .unwrap();
    let rotated = credentials.password_for(created.id).unwrap().unwrap();
    assert_eq!(rotated.secret_hash.expose(), new_hash);

    principals
        .update_status(created.id, PrincipalStatus::Suspended)
        .unwrap();
    assert!(!principals.by_id(created.id).unwrap().unwrap().is_valid());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_session_store_lifecycle() {
    let Some(pool) = pool().await else { return };
    let store = PgSessionStore::new(pool.clone());
    let manager = keel_session::SessionManager::new(
        std::sync::Arc::new(store),
        SessionPolicy {
            idle_secs: 3600,
            absolute_secs: 7200,
            ..SessionPolicy::default()
        },
        b"pg-test-secret",
    );
    let now = OffsetDateTime::now_utc();
    let principal_row = principal(&format!("pg-sess-{}@it.test", uuid::Uuid::now_v7()));
    let principal = principal_row.id;
    PgPrincipalStore::new(pool.clone())
        .create(principal_row)
        .expect("session principal inserts");
    let tenant = keel_foundation::TenantId::new();

    let session = manager.start(principal, Some(tenant), now).unwrap();
    assert!(manager.store().load(&session.id).unwrap().is_some());
    assert!(manager.store().count() >= 1);

    manager
        .put(&session.id, "locale", serde_json::json!("en-IN"))
        .unwrap();
    let loaded = manager.store().load(&session.id).unwrap().unwrap();
    assert_eq!(loaded.data.get("locale"), Some(&serde_json::json!("en-IN")));
    assert_eq!(loaded.tenant, Some(tenant));
    assert_eq!(loaded.principal, principal);

    let rotated = manager.regenerate(&session.id, now).unwrap().unwrap();
    assert_ne!(rotated.id, session.id);
    assert!(
        manager.store().load(&session.id).unwrap().is_none(),
        "old id deleted"
    );

    manager.destroy(&rotated.id).unwrap();
    assert!(manager.store().load(&rotated.id).unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_session_sweep_removes_expired_rows() {
    let Some(pool) = pool().await else { return };
    let store = PgSessionStore::new(pool.clone());
    let now = OffsetDateTime::now_utc();
    let policy = SessionPolicy {
        idle_secs: 60,
        absolute_secs: 120,
        ..SessionPolicy::default()
    };

    let principals = PgPrincipalStore::new(pool.clone());
    let fresh_row = principal(&format!("pg-fresh-{}@it.test", uuid::Uuid::now_v7()));
    let fresh_principal = fresh_row.id;
    principals.create(fresh_row).expect("fresh principal inserts");
    let stale_row = principal(&format!("pg-stale-{}@it.test", uuid::Uuid::now_v7()));
    let stale_principal = stale_row.id;
    principals.create(stale_row).expect("stale principal inserts");

    let fresh = keel_session::Session::new(fresh_principal, None, now);
    store.save(&fresh).unwrap();
    let mut stale = keel_session::Session::new(
        stale_principal,
        None,
        now - time::Duration::seconds(600),
    );
    stale.last_seen = now - time::Duration::seconds(600);
    store.save(&stale).unwrap();

    let reaped = store.sweep(now, &policy).unwrap();
    assert!(reaped >= 1, "stale session reaped");
    assert!(store.load(&fresh.id).unwrap().is_some());
    assert!(store.load(&stale.id).unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_token_store_issue_verify_consume() {
    let Some(pool) = pool().await else { return };
    let service = keel_tokens::TokenService::new(std::sync::Arc::new(
        keel_postgres::PgTokenStore::new(pool.clone()),
    ));
    let principal_row = principal(&format!("pg-tokens-{}@it.test", uuid::Uuid::now_v7()));
    let principal = principal_row.id;
    PgPrincipalStore::new(pool.clone())
        .create(principal_row)
        .expect("token principal inserts");
    let issued = service
        .issue(
            principal,
            keel_tokens::TokenPurpose::Reset,
            time::Duration::minutes(10),
            None,
        )
        .unwrap();
    let found = service
        .verify(keel_tokens::TokenPurpose::Reset, &issued.plaintext)
        .unwrap();
    assert_eq!(found.principal_id, principal);
    service
        .consume(keel_tokens::TokenPurpose::Reset, &issued.plaintext)
        .unwrap();
    assert!(service
        .verify(keel_tokens::TokenPurpose::Reset, &issued.plaintext)
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_email_verification_timestamp_roundtrip() {
    let Some(pool) = pool().await else { return };
    let principals = PgPrincipalStore::new(pool.clone());
    let email = format!("pg-verify-{}@it.test", uuid::Uuid::now_v7());

    let created = principals.create(principal(&email)).expect("inserts");
    let loaded = principals.by_id(created.id).unwrap().unwrap();
    assert!(loaded.email_verified_at.is_none(), "fresh principal unverified");

    principals.mark_email_verified(created.id).expect("marks");
    let loaded = principals.by_id(created.id).unwrap().unwrap();
    assert!(
        loaded.email_verified_at.is_some(),
        "verification timestamp recorded"
    );
    let first = loaded.email_verified_at;

    // Idempotent: a second call does not move the timestamp.
    principals.mark_email_verified(created.id).expect("idempotent");
    let loaded = principals.by_id(created.id).unwrap().unwrap();
    assert_eq!(loaded.email_verified_at, first, "timestamp stable on re-mark");

    // Unknown principal is NOT_FOUND.
    assert!(principals.mark_email_verified(PrincipalId::new()).is_err());
}