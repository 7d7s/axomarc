//! PostgreSQL integration matrix (S7 B-19 foundation): durable stores are
//! exercised against a live database. Activated with `--features pg-tests`;
//! every test is a graceful skip when DATABASE_URL is unset, so local runs
//! without a database stay green and CI (service container) enforces them.

#![cfg(feature = "pg-tests")]

use keel_foundation::{PrincipalId, TenantId};
use keel_identity::store::{CredentialStore, PrincipalStore};
use keel_identity::{Credential, CredentialKind, Principal, PrincipalStatus, Secret};
use keel_notifications::{NotificationRow, NotificationStore};
use keel_postgres::{
    create_pool, run_migrations, PgBatchLedger, PgCredentialStore, PgNotificationStore, PgPool,
    PgPrincipalStore, PgSessionStore,
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
            run_migrations(&pool, std::path::Path::new("."))
                .await
                .expect("migrations apply");
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

/// Wave 6 P5: the notifications table round trip — insert through the FK
/// constraints (principal required, tenant recorded), newest-first
/// principal-scoped listing with feed counts, and idempotent principal-only
/// mark-read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_notification_store_roundtrip() {
    let Some(pool) = pool().await else { return };
    let principals = PgPrincipalStore::new(pool.clone());
    let notifications = PgNotificationStore::new(pool.clone());

    let owner = principals
        .create(principal(&format!(
            "pg-notif-{}@it.test",
            uuid::Uuid::now_v7()
        )))
        .expect("principal inserts");
    let noise = principals
        .create(principal(&format!(
            "pg-notif-noise-{}@it.test",
            uuid::Uuid::now_v7()
        )))
        .expect("noise principal inserts");
    let tenant_id = uuid::Uuid::now_v7();
    keel_postgres::seed_tenant(&pool, tenant_id, &format!("notif-{}", uuid::Uuid::now_v7()), "Notifications Tenant")
        .await
        .expect("tenant seeds");
    let tenant = Some(TenantId::from_uuid(tenant_id));

    let row_for = |principal_id: PrincipalId, patient: &str| NotificationRow {
        id: uuid::Uuid::now_v7(),
        principal_id,
        tenant_id: tenant,
        kind: "appointment.booked".into(),
        data: serde_json::json!({ "patient": patient }),
        read_at: None,
        created_at: OffsetDateTime::now_utc(),
    };

    notifications
        .insert(row_for(owner.id, "Ada"))
        .await
        .expect("row inserts");
    notifications
        .insert(row_for(owner.id, "Grace"))
        .await
        .expect("row inserts");
    notifications
        .insert(row_for(noise.id, "Noise"))
        .await
        .expect("foreign row inserts");

    let feed = notifications.list(owner.id, 0, 10).await.expect("lists");
    assert_eq!(feed.total, 2, "foreign principal rows never count");
    assert_eq!(feed.unread, 2);
    assert_eq!(feed.rows.len(), 2);
    assert!(
        feed.rows[0].created_at >= feed.rows[1].created_at,
        "newest first"
    );
    assert!(feed
        .rows
        .iter()
        .all(|r| r.kind == "appointment.booked" && r.tenant_id == tenant));
    assert!(feed
        .rows
        .iter()
        .any(|r| r.data["patient"].as_str() == Some("Ada")));

    // Page 2 of a limit-1 listing yields exactly the older row.
    let second = notifications.list(owner.id, 1, 1).await.expect("pages");
    assert_eq!(second.rows.len(), 1);
    assert_eq!(second.total, 2);

    // Mark-read: another principal cannot apply it; the owner's mark is
    // counted once and never again.
    let target: Vec<uuid::Uuid> = feed.rows.iter().take(1).map(|r| r.id).collect();
    let foreign = notifications
        .mark_read(noise.id, target.clone())
        .await
        .expect("attempt");
    assert_eq!(foreign, 0, "principal scoping holds");
    let updated = notifications
        .mark_read(owner.id, target.clone())
        .await
        .expect("marks");
    assert_eq!(updated, 1);
    let again = notifications
        .mark_read(owner.id, target)
        .await
        .expect("re-marks");
    assert_eq!(again, 0, "idempotent");

    let feed = notifications.list(owner.id, 0, 10).await.expect("counts");
    assert_eq!(feed.total, 2);
    assert_eq!(feed.unread, 1);
}

/// Wave 6 P9 (B-32): the durable batch ledger round trip — membership
/// written once survives across ledger instances (the restart /
/// second-process stand-in), the terminal stamp is one-shot, and prune ages
/// out only finished records strictly before the cutoff.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_batch_ledger_roundtrip() {
    use keel_jobs::{BatchLedger, BatchRecord};

    let Some(pool) = pool().await else { return };
    let ledger = PgBatchLedger::new(pool.clone());

    let record = BatchRecord {
        id: uuid::Uuid::now_v7(),
        name: "nightly".into(),
        members: vec![uuid::Uuid::now_v7(), uuid::Uuid::now_v7()],
        on_complete: Some("batch.done".into()),
        created_at: OffsetDateTime::now_utc(),
        finished_at: None,
    };
    ledger.save(&record).expect("record writes");

    // A second ledger over the same pool = another process / a restart.
    let reader = PgBatchLedger::new(pool.clone());
    let read = reader
        .get(record.id)
        .expect("reads")
        .expect("record present");
    assert_eq!(read.name, "nightly");
    assert_eq!(read.members, record.members, "membership round trips");
    assert_eq!(read.on_complete.as_deref(), Some("batch.done"));
    assert!(read.finished_at.is_none(), "open batch carries no stamp");
    assert!(
        reader
            .list()
            .expect("lists")
            .iter()
            .any(|r| r.id == record.id),
        "list sees the record"
    );

    // Upsert refreshes metadata only — the bookkeeping columns hold.
    // (Compare against the first READ: timestamptz stores microseconds, so
    // the raw `record.created_at` nanos never survive the trip verbatim.)
    let created = read.created_at;
    let mut refreshed = read.clone();
    refreshed.name = "nightly-renamed".into();
    ledger.save(&refreshed).expect("upsert");
    let read = ledger.get(record.id).expect("reads").expect("present");
    assert_eq!(read.name, "nightly-renamed");
    assert_eq!(read.created_at, created, "created_at never rewritten");
    assert!(read.finished_at.is_none());

    // Unfinished records survive every cutoff; prune says so honestly.
    let future = OffsetDateTime::now_utc() + time::Duration::hours(1);
    assert_eq!(ledger.prune(future).expect("prunes"), 0, "open batch kept");

    // The terminal stamp is one-shot (finished_at IS NULL guard)...
    let finished_at = OffsetDateTime::now_utc();
    ledger.mark_finished(record.id, finished_at).expect("stamps");
    let later = finished_at + time::Duration::seconds(5);
    ledger
        .mark_finished(record.id, later)
        .expect("second stamp is a no-op");
    let read = ledger.get(record.id).expect("reads").expect("present");
    let stamped = read.finished_at.expect("stamped");
    // timestamptz stores microseconds — compare within that truncation.
    assert!(
        stamped <= finished_at && stamped >= finished_at - time::Duration::seconds(1),
        "first stamp wins ({stamped:?} vs {finished_at:?})"
    );

    // ...and only strictly-before cutoffs age the record out.
    let past = OffsetDateTime::now_utc() - time::Duration::hours(1);
    assert_eq!(ledger.prune(past).expect("prunes"), 0, "stamp is newer");
    assert_eq!(ledger.prune(future).expect("prunes"), 1, "finished record aged out");
    assert!(
        ledger.get(record.id).expect("reads").is_none(),
        "pruned records are gone"
    );
}
/// Wave 7 P3 (docs/19 L341): transactional savepoint isolation — the
/// RefreshDatabase-equivalent suite setup. Two test bodies touch the SAME
/// table on SEPARATE connections: body A's insert lives only inside its
/// `BEGIN; SAVEPOINT keel_test` scope, `rollback()` leaves no residue for
/// body B, and no schema drops/truncation are involved. (DATABASE_URL-gated
/// like every pg test; skips cleanly without it.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_savepoint_rollback_isolates_two_tests_sharing_a_table() {
    let Some(pool) = pool().await else { return };
    let email_a = format!("savepoint-a-{}@it.test", uuid::Uuid::now_v7());
    let email_b = format!("savepoint-b-{}@it.test", uuid::Uuid::now_v7());

    // Test body A: writes inside the savepoint, sees them, rolls them back.
    {
        let mut conn = pool.acquire().await.expect("connection A");
        let mut sp = keel_postgres::test_support::Savepoint::begin(&mut conn)
            .await
            .expect("BEGIN + SAVEPOINT keel_test");
        sqlx::query("INSERT INTO principals (id, email) VALUES ($1, $2)")
            .bind(uuid::Uuid::now_v7())
            .bind(&email_a)
            .execute(sp.conn())
            .await
            .expect("insert inside the scope");
        let seen: i64 =
            sqlx::query_scalar("SELECT count(*) FROM principals WHERE email = $1")
                .bind(&email_a)
                .fetch_one(sp.conn())
                .await
                .expect("count inside the scope");
        assert_eq!(seen, 1, "the body sees its own write");
        sp.rollback().await.expect("ROLLBACK TO keel_test");
    }

    // Test body B: a separate connection — A's row never existed for it,
    // and the shared table still accepts writes through its own scope.
    {
        let mut conn = pool.acquire().await.expect("connection B");
        let residue: i64 =
            sqlx::query_scalar("SELECT count(*) FROM principals WHERE email = $1")
                .bind(&email_a)
                .fetch_one(&mut *conn)
                .await
                .expect("count after A rolled back");
        assert_eq!(residue, 0, "body A left no residue behind");

        let mut sp = keel_postgres::test_support::Savepoint::begin(&mut conn)
            .await
            .expect("second scope begins");
        sqlx::query("INSERT INTO principals (id, email) VALUES ($1, $2)")
            .bind(uuid::Uuid::now_v7())
            .bind(&email_b)
            .execute(sp.conn())
            .await
            .expect("second body writes to the same table");
        sp.rollback().await.expect("second rollback");
    }

    // Final state: the table is exactly as both bodies found it.
    let mut conn = pool.acquire().await.expect("connection C");
    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM principals WHERE email LIKE 'savepoint-%@it.test'")
            .fetch_one(&mut *conn)
            .await
            .expect("final count");
    assert_eq!(total, 0, "no savepoint row survives its scope");
}
