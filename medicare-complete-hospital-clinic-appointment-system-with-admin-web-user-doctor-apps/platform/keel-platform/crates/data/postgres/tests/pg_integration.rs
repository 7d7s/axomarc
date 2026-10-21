//! PostgreSQL integration matrix (S7 B-19 foundation): durable stores are
//! exercised against a live database. Activated with `--features pg-tests`;
//! every test is a graceful skip when DATABASE_URL is unset, so local runs
//! without a database stay green and CI (service container) enforces them.
//! Wave 9 B-31: the same file carries the live-MySQL driver matrix — same
//! feature, same DATABASE_URL gate, scheme-based skip (`postgres://` runs
//! the `pg_*` half, `mysql://` runs the `mysql_*` half).

#![cfg(feature = "pg-tests")]

use kelson_foundation::{PrincipalId, TenantId};
use kelson_identity::store::{CredentialStore, PrincipalStore};
use kelson_identity::{Credential, CredentialKind, Principal, PrincipalStatus, Secret};
use kelson_notifications::{NotificationRow, NotificationStore};
use kelson_postgres::{
    create_pool, run_migrations, PgBatchLedger, PgCredentialStore, PgNotificationStore, PgPool,
    PgPrincipalStore, PgSessionStore,
};
use kelson_event::OutboxStore;
use kelson_session::{SessionPolicy, SessionStore};
use time::OffsetDateTime;

/// The job-store cases share ONE `jobs_queue` table. The unscoped-claim cases
/// wipe the pending/running rows first so their claim is deterministic — which
/// would eat a concurrent case's job, and a global claim could steal one
/// instead. They take this lock so the queue is only ever mutated by one case
/// at a time. (The tenant-scoped case needs it for the same reason: its job
/// must not be wiped or claimed by an unscoped case running beside it.)
static JOBS_QUEUE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

    let secret_hash = kelson_identity::hash_password(&Secret::new("pg-password-99")).unwrap();
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
    assert!(kelson_identity::verify_password(
        &Secret::new("pg-password-99"),
        loaded.secret_hash.expose()
    )
    .unwrap());

    let new_hash = kelson_identity::hash_password(&Secret::new("rotated-password-1")).unwrap();
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
    let manager = kelson_session::SessionManager::new(
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
    let tenant = kelson_foundation::TenantId::new();

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

    let fresh = kelson_session::Session::new(fresh_principal, None, now);
    store.save(&fresh).unwrap();
    let mut stale = kelson_session::Session::new(
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
    let service = kelson_tokens::TokenService::new(std::sync::Arc::new(
        kelson_postgres::PgTokenStore::new(pool.clone()),
    ));
    let principal_row = principal(&format!("pg-tokens-{}@it.test", uuid::Uuid::now_v7()));
    let principal = principal_row.id;
    PgPrincipalStore::new(pool.clone())
        .create(principal_row)
        .expect("token principal inserts");
    let issued = service
        .issue(
            principal,
            kelson_tokens::TokenPurpose::Reset,
            time::Duration::minutes(10),
            None,
        )
        .unwrap();
    let found = service
        .verify(kelson_tokens::TokenPurpose::Reset, &issued.plaintext)
        .unwrap();
    assert_eq!(found.principal_id, principal);
    service
        .consume(kelson_tokens::TokenPurpose::Reset, &issued.plaintext)
        .unwrap();
    assert!(service
        .verify(kelson_tokens::TokenPurpose::Reset, &issued.plaintext)
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
    kelson_postgres::seed_tenant(&pool, tenant_id, &format!("notif-{}", uuid::Uuid::now_v7()), "Notifications Tenant")
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
    use kelson_jobs::{BatchLedger, BatchRecord};

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
/// `BEGIN; SAVEPOINT kelson_test` scope, `rollback()` leaves no residue for
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
        let mut sp = kelson_postgres::test_support::Savepoint::begin(&mut conn)
            .await
            .expect("BEGIN + SAVEPOINT kelson_test");
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
        sp.rollback().await.expect("ROLLBACK TO kelson_test");
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

        let mut sp = kelson_postgres::test_support::Savepoint::begin(&mut conn)
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

// ----------------------------------------------------- pg outbox store
// Durable outbox: claim unpublished, mark published, redelivery after failure.
#[tokio::test]
async fn pg_outbox_store_claim_and_mark_published() {
    let pool = match pool().await {
        Some(p) => p,
        None => return,
    };
    let store = kelson_postgres::PgOutboxStore::new(pool.clone());

    // Insert a test outbox event directly.
    let event_id = uuid::Uuid::now_v7();
    let tenant_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO outbox (id, tenant_id, event, topic, payload, idempotency_key, published, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, false, now())",
    )
    .bind(event_id)
    .bind(tenant_id)
    .bind("test.event")
    .bind("test.topic")
    .bind(serde_json::json!({"key": "value"}))
    .bind("test-key")
    .execute(&pool)
    .await
    .expect("insert outbox row");

    // Claim unpublished — should include our event.
    let claimed = store.claim_unpublished(10).await.expect("claim");
    assert!(
        claimed.iter().any(|e| e.id == event_id),
        "claimed events must include the inserted row"
    );

    // Mark published.
    store
        .mark_published(&[event_id])
        .await
        .expect("mark published");

    // Re-claim — should NOT include our event (now published).
    let reclaimed = store.claim_unpublished(10).await.expect("reclaim");
    assert!(
        !reclaimed.iter().any(|e| e.id == event_id),
        "published event must not be re-claimed"
    );

    // Cleanup.
    let _ = sqlx::query("DELETE FROM outbox WHERE id = $1")
        .bind(event_id)
        .execute(&pool)
        .await;
}

// ----------------------------------------------------- mysql outbox store
// Durable outbox: claim unpublished, mark published, redelivery after failure.
/// **F-9 (the deep audit) — the outbox's two hardest cases, named.** The
/// store's own header claims "rows only ever exist post-commit (D-53)" ✓ —
/// that was an ASSUMPTION ✓. These two cases prove it ✓:
///
/// * **rollback suppresses** ✓ — an event written in the same transaction as
///   the business write must vanish with the rollback ✓ (otherwise the
///   pump publishes work that never happened ✓ ✓);
/// * **commit-then-crash** ✓ — an event committed but not yet marked
///   published must be **redelivered** ✓ (at-least-once ✓) with the same
///   idempotency key ✓ (so the consumer can dedupe ✓ ✓).
#[tokio::test]
async fn pg_outbox_rollback_suppresses_the_event_and_a_crash_redelivers_it() {
    let pool = match pool().await {
        Some(p) => p,
        None => return,
    };
    let store = kelson_postgres::PgOutboxStore::new(pool.clone());

    // --- rollback suppresses ---------------------------------------------
    let rolled_back = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await.expect("the transaction begins");
    sqlx::query(
        "INSERT INTO outbox (id, tenant_id, event, topic, payload, idempotency_key, published, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, false, now())",
    )
    .bind(rolled_back)
    .bind(uuid::Uuid::now_v7())
    .bind("test.rolled_back")
    .bind("test.topic")
    .bind(serde_json::json!({"key": "value"}))
    .bind("rolled-back-key")
    .execute(&mut *tx)
    .await
    .expect("the row is written inside the transaction");
    // The business write fails, the transaction rolls back.
    tx.rollback().await.expect("the rollback succeeds");

    let claimed = store.claim_unpublished(50).await.expect("claim");
    assert!(
        !claimed.iter().any(|e| e.id == rolled_back),
        "a rolled-back event must never be published"
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE id = $1")
        .bind(rolled_back)
        .fetch_one(&pool)
        .await
        .expect("the count reads");
    assert_eq!(rows, 0, "the rolled-back row is gone from the table");

    // --- commit-then-crash ----------------------------------------------
    let committed = uuid::Uuid::now_v7();
    let key = format!("crash-key-{committed}");
    let mut tx = pool.begin().await.expect("the transaction begins");
    sqlx::query(
        "INSERT INTO outbox (id, tenant_id, event, topic, payload, idempotency_key, published, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, false, now())",
    )
    .bind(committed)
    .bind(uuid::Uuid::now_v7())
    .bind("test.committed")
    .bind("test.topic")
    .bind(serde_json::json!({"key": "value"}))
    .bind(&key)
    .execute(&mut *tx)
    .await
    .expect("the row is written inside the transaction");
    tx.commit().await.expect("the transaction commits");

    // The first pass claims it; the process "crashes" before marking it
    // published, so the row stays unpublished.
    let first = store.claim_unpublished(50).await.expect("claim");
    let event = first
        .iter()
        .find(|e| e.id == committed)
        .expect("the committed event is claimed");
    assert_eq!(
        event.idempotency_key.0,
        key,
        "the idempotency key is stable across the redelivery"
    );
    assert!(!event.published, "an un-marked event is still unpublished");

    // The next pass redelivers it - at-least-once, not at-most-once.
    let second = store.claim_unpublished(50).await.expect("reclaim");
    let redelivered = second
        .iter()
        .find(|e| e.id == committed)
        .expect("the event is redelivered after the crash");
    assert_eq!(
        redelivered.idempotency_key.0,
        key,
        "the redelivery carries the same key, so the consumer can dedupe"
    );

    // Cleanup.
    let _ = sqlx::query("DELETE FROM outbox WHERE id = $1")
        .bind(committed)
        .execute(&pool)
        .await;
}

/// The same two cases on MySQL 8 — the fabric that runs on this host.
#[tokio::test]
async fn mysql_outbox_rollback_suppresses_the_event_and_a_crash_redelivers_it() {
    let pool = match mysql_pool().await {
        Some(p) => p,
        None => return,
    };
    let store = kelson_mysql::MySqlOutboxStore::new(pool.clone());

    // --- rollback suppresses ---------------------------------------------
    let rolled_back = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await.expect("the transaction begins");
    sqlx::query(
        "INSERT INTO outbox (id, tenant_id, event, topic, payload, idempotency_key, published, created_at)
         VALUES (?, ?, ?, ?, ?, ?, false, NOW(6))",
    )
    .bind(rolled_back.as_bytes().to_vec())
    .bind(None::<Vec<u8>>)
    .bind("test.rolled_back")
    .bind("test.topic")
    .bind(serde_json::json!({"key": "value"}).to_string())
    .bind("rolled-back-key")
    .execute(&mut *tx)
    .await
    .expect("the row is written inside the transaction");
    tx.rollback().await.expect("the rollback succeeds");

    let claimed = store.claim_unpublished(50).await.expect("claim");
    assert!(
        !claimed.iter().any(|e| e.id == rolled_back),
        "a rolled-back event must never be published"
    );

    // --- commit-then-crash ----------------------------------------------
    let committed = uuid::Uuid::now_v7();
    let key = format!("crash-key-{committed}");
    let mut tx = pool.begin().await.expect("the transaction begins");
    sqlx::query(
        "INSERT INTO outbox (id, tenant_id, event, topic, payload, idempotency_key, published, created_at)
         VALUES (?, ?, ?, ?, ?, ?, false, NOW(6))",
    )
    .bind(committed.as_bytes().to_vec())
    .bind(None::<Vec<u8>>)
    .bind("test.committed")
    .bind("test.topic")
    .bind(serde_json::json!({"key": "value"}).to_string())
    .bind(&key)
    .execute(&mut *tx)
    .await
    .expect("the row is written inside the transaction");
    tx.commit().await.expect("the transaction commits");

    let first = store.claim_unpublished(50).await.expect("claim");
    let event = first
        .iter()
        .find(|e| e.id == committed)
        .expect("the committed event is claimed");
    assert_eq!(
        event.idempotency_key.0,
        key,
        "the idempotency key is stable across the redelivery"
    );

    let second = store.claim_unpublished(50).await.expect("reclaim");
    let redelivered = second
        .iter()
        .find(|e| e.id == committed)
        .expect("the event is redelivered after the crash");
    assert_eq!(
        redelivered.idempotency_key.0,
        key,
        "the redelivery carries the same key, so the consumer can dedupe"
    );

    let _ = sqlx::query("DELETE FROM outbox WHERE id = ?")
        .bind(committed.as_bytes().to_vec())
        .execute(&pool)
        .await;
}#[tokio::test]
async fn mysql_outbox_store_claim_and_mark_published() {
    let pool = match mysql_pool().await {
        Some(p) => p,
        None => return,
    };
    let store = kelson_mysql::MySqlOutboxStore::new(pool.clone());

    // Insert a test outbox event directly.
    let event_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO outbox (id, tenant_id, event, topic, payload, idempotency_key, published, created_at)
         VALUES (?, ?, ?, ?, ?, ?, false, NOW(6))",
    )
    .bind(event_id.as_bytes().to_vec())
    .bind(None::<Vec<u8>>)  // tenant_id nullable
    .bind("test.event")
    .bind("test.topic")
    .bind(serde_json::json!({"key": "value"}).to_string())
    .bind("test-key")
    .execute(&pool)
    .await
    .expect("insert outbox row");

    // Claim unpublished — should include our event.
    let claimed = store.claim_unpublished(10).await.expect("claim");
    assert!(
        claimed.iter().any(|e| e.id == event_id),
        "claimed events must include the inserted row"
    );

    // Mark published.
    store
        .mark_published(&[event_id])
        .await
        .expect("mark published");

    // Re-claim — should NOT include our event (now published).
    let reclaimed = store.claim_unpublished(10).await.expect("reclaim");
    assert!(
        !reclaimed.iter().any(|e| e.id == event_id),
        "published event must not be re-claimed"
    );

    // Cleanup.
    let _ = sqlx::query("DELETE FROM outbox WHERE id = ?")
        .bind(event_id.as_bytes().to_vec())
        .execute(&pool)
        .await;
}

// ----------------------------------------------------- mysql driver matrix
// Wave 9 B-31 (ADR 0010): the same store matrix over the second fabric.
// Each test skips unless DATABASE_URL points at MySQL; the pg half above
// skips symmetrically (its create_pool refuses a mysql:// URL).

async fn mysql_pool() -> Option<kelson_mysql::MySqlPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!("mysql matrix: DATABASE_URL not set; skipping");
            return None;
        }
    };
    if !url.starts_with("mysql://") {
        eprintln!("mysql matrix: DATABASE_URL is not mysql://; skipping");
        return None;
    }
    match kelson_mysql::create_pool(&url, 4).await {
        Ok(pool) => {
            kelson_mysql::run_migrations(&pool, std::path::Path::new("."))
                .await
                .expect("migrations apply");
            Some(pool)
        }
        Err(e) => {
            eprintln!("mysql matrix: cannot connect ({e}); skipping");
            None
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_principal_and_credential_roundtrip() {
    let Some(pool) = mysql_pool().await else { return };
    let principals = kelson_mysql::MySqlPrincipalStore::new(pool.clone());
    let credentials = kelson_mysql::MySqlCredentialStore::new(pool.clone());
    let email = format!("mysql-{}@it.test", uuid::Uuid::now_v7());

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

    let dup = principals.create(principal(&email));
    assert!(dup.is_err(), "unique email enforced");

    let secret_hash = kelson_identity::hash_password(&Secret::new("mysql-password-99")).unwrap();
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
    assert!(kelson_identity::verify_password(
        &Secret::new("mysql-password-99"),
        loaded.secret_hash.expose()
    )
    .unwrap());

    let new_hash = kelson_identity::hash_password(&Secret::new("rotated-mysql-1")).unwrap();
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
async fn mysql_session_store_lifecycle() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlSessionStore::new(pool.clone());
    let manager = kelson_session::SessionManager::new(
        std::sync::Arc::new(store),
        SessionPolicy {
            idle_secs: 3600,
            absolute_secs: 7200,
            ..SessionPolicy::default()
        },
        b"mysql-test-secret",
    );
    let now = OffsetDateTime::now_utc();
    let principal_row = principal(&format!("mysql-sess-{}@it.test", uuid::Uuid::now_v7()));
    let principal = principal_row.id;
    kelson_mysql::MySqlPrincipalStore::new(pool.clone())
        .create(principal_row)
        .expect("session principal inserts");
    let tenant = kelson_foundation::TenantId::new();

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
async fn mysql_session_sweep_removes_expired_rows() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlSessionStore::new(pool.clone());
    let now = OffsetDateTime::now_utc();
    let policy = SessionPolicy {
        idle_secs: 60,
        absolute_secs: 120,
        ..SessionPolicy::default()
    };

    let principals = kelson_mysql::MySqlPrincipalStore::new(pool.clone());
    let fresh_row = principal(&format!("mysql-fresh-{}@it.test", uuid::Uuid::now_v7()));
    let fresh_principal = fresh_row.id;
    principals.create(fresh_row).expect("fresh principal inserts");
    let stale_row = principal(&format!("mysql-stale-{}@it.test", uuid::Uuid::now_v7()));
    let stale_principal = stale_row.id;
    principals.create(stale_row).expect("stale principal inserts");

    let fresh = kelson_session::Session::new(fresh_principal, None, now);
    store.save(&fresh).unwrap();
    let mut stale = kelson_session::Session::new(
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
async fn mysql_token_store_issue_verify_consume() {
    let Some(pool) = mysql_pool().await else { return };
    let service = kelson_tokens::TokenService::new(std::sync::Arc::new(
        kelson_mysql::MySqlTokenStore::new(pool.clone()),
    ));
    let principal_row = principal(&format!("mysql-tokens-{}@it.test", uuid::Uuid::now_v7()));
    let principal = principal_row.id;
    kelson_mysql::MySqlPrincipalStore::new(pool.clone())
        .create(principal_row)
        .expect("token principal inserts");
    let issued = service
        .issue(
            principal,
            kelson_tokens::TokenPurpose::Reset,
            time::Duration::minutes(10),
            None,
        )
        .unwrap();
    let found = service
        .verify(kelson_tokens::TokenPurpose::Reset, &issued.plaintext)
        .unwrap();
    assert_eq!(found.principal_id, principal);
    service
        .consume(kelson_tokens::TokenPurpose::Reset, &issued.plaintext)
        .unwrap();
    assert!(service
        .verify(kelson_tokens::TokenPurpose::Reset, &issued.plaintext)
        .is_err());
}

/// Row 2, measured: the MySQL adapter's server-side timestamps must be UTC.
///
/// `DATETIME` carries no timezone, so `NOW(6)` and a
/// `DEFAULT CURRENT_TIMESTAMP(6)` land as whatever the SERVER's zone says —
/// on this host that is **+5h30m (19800s) ahead of UTC** (measured with
/// `TIMESTAMPDIFF(SECOND, UTC_TIMESTAMP(6), NOW(6))`), while the driver
/// decodes the value back as UTC. The framework's own writes are safe
/// because every connection is pinned to `time_zone='+00:00'` — and this
/// test pins that guarantee down, so a dependency default cannot change
/// under it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_every_connection_is_pinned_to_utc() {
    let Some(pool) = mysql_pool().await else { return };

    let tz: String = sqlx::query_scalar("SELECT @@session.time_zone")
        .fetch_one(&pool)
        .await
        .expect("the session timezone is readable");
    assert_eq!(
        tz, "+00:00",
        "every connection is pinned to UTC, so a server-side timestamp is a correct instant"
    );

    // And the consequence, end to end: the server's own clock, read through
    // the pinned session, equals the app's UTC clock.
    let server_now: OffsetDateTime = sqlx::query_scalar("SELECT NOW(6)")
        .fetch_one(&pool)
        .await
        .expect("the server clock is readable");
    let drift = (server_now - OffsetDateTime::now_utc()).whole_seconds() as f64;
    assert!(
        drift.abs() < 60.0,
        "the server's clock reads as UTC through the pinned session (it is {drift}s away from the app's UTC now — the raw zone offset would be 19800s here)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_server_side_timestamps_are_utc() {
    let Some(pool) = mysql_pool().await else { return };
    let principals = kelson_mysql::MySqlPrincipalStore::new(pool.clone());
    let email = format!("mysql-utc-{}@it.test", uuid::Uuid::now_v7());

    let before = OffsetDateTime::now_utc();
    let created = principals.create(principal(&email)).expect("inserts");
    let loaded = principals.by_id(created.id).unwrap().unwrap();
    let after = OffsetDateTime::now_utc();

    let delta = |stored: OffsetDateTime| -> f64 {
        let low = (stored - before).whole_seconds() as f64;
        let high = (stored - after).whole_seconds() as f64;
        // Outside the window means the server wrote something else entirely.
        if low > high {
            f64::INFINITY
        } else if low > 0.0 {
            low
        } else if high < 0.0 {
            high
        } else {
            0.0
        }
    };

    assert!(
        delta(loaded.created_at).abs() < 60.0,
        "created_at is UTC, not the server's local clock (the stored instant is {:?} away from the app's UTC now; the server's zone offset would show here)",
        delta(loaded.created_at)
    );

    principals.mark_email_verified(created.id).expect("marks");
    let loaded = principals.by_id(created.id).unwrap().unwrap();
    let verified = loaded.email_verified_at.expect("the stamp is recorded");
    let after = OffsetDateTime::now_utc();
    let drift = (verified - after).whole_seconds() as f64;
    assert!(
        drift.abs() < 60.0,
        "email_verified_at is UTC, not the server's local clock (the stored instant is {drift}s away from the app's UTC now)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_email_verification_timestamp_roundtrip() {
    let Some(pool) = mysql_pool().await else { return };
    let principals = kelson_mysql::MySqlPrincipalStore::new(pool.clone());
    let email = format!("mysql-verify-{}@it.test", uuid::Uuid::now_v7());

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

/// B-31 twin of `pg_notification_store_roundtrip`: insert through the FK
/// constraints, newest-first principal-scoped listing with feed counts, and
/// idempotent principal-only mark-read on MySQL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_notification_store_roundtrip() {
    let Some(pool) = mysql_pool().await else { return };
    let principals = kelson_mysql::MySqlPrincipalStore::new(pool.clone());
    let notifications = kelson_mysql::MySqlNotificationStore::new(pool.clone());

    let owner = principals
        .create(principal(&format!(
            "mysql-notif-{}@it.test",
            uuid::Uuid::now_v7()
        )))
        .expect("principal inserts");
    let noise = principals
        .create(principal(&format!(
            "mysql-notif-noise-{}@it.test",
            uuid::Uuid::now_v7()
        )))
        .expect("noise principal inserts");
    let tenant_id = uuid::Uuid::now_v7();
    kelson_mysql::seed_tenant(
        &pool,
        tenant_id,
        &format!("notif-{}", uuid::Uuid::now_v7()),
        "Notifications Tenant",
    )
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

/// B-31 twin of `pg_batch_ledger_roundtrip`: durable membership across
/// ledger instances, the one-shot terminal stamp, and cutoff-faithful prune
/// on MySQL (`ON DUPLICATE KEY UPDATE` refreshes metadata only).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_batch_ledger_roundtrip() {
    use kelson_jobs::{BatchLedger, BatchRecord};

    let Some(pool) = mysql_pool().await else { return };
    let ledger = kelson_mysql::MySqlBatchLedger::new(pool.clone());

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
    let reader = kelson_mysql::MySqlBatchLedger::new(pool.clone());
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

/// B-31 twin of the savepoint isolation contract (the DoD's "transactions"
/// leg): a transaction-scoped write on one connection leaves no residue for
/// a second connection, and the shared table still accepts writes. MySQL
/// scopes the same way through `BEGIN`/`ROLLBACK` (the port's own tx flow).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_transaction_rollback_isolates_two_tests_sharing_a_table() {
    let Some(pool) = mysql_pool().await else { return };
    let email_a = format!("mysql-savepoint-a-{}@it.test", uuid::Uuid::now_v7());
    let email_b = format!("mysql-savepoint-b-{}@it.test", uuid::Uuid::now_v7());

    // Test body A: writes inside the transaction, sees them, rolls back.
    {
        let mut tx = pool.begin().await.expect("BEGIN A");
        sqlx::query("INSERT INTO principals (id, email) VALUES (?, ?)")
            .bind(uuid::Uuid::now_v7())
            .bind(&email_a)
            .execute(&mut *tx)
            .await
            .expect("insert inside the scope");
        let seen: i64 =
            sqlx::query_scalar("SELECT count(*) FROM principals WHERE email = ?")
                .bind(&email_a)
                .fetch_one(&mut *tx)
                .await
                .expect("count inside the scope");
        assert_eq!(seen, 1, "the body sees its own write");
        tx.rollback().await.expect("ROLLBACK");
    }

    // Test body B: a separate connection — A's row never existed for it,
    // and the shared table still accepts writes through its own scope.
    {
        let mut conn = pool.acquire().await.expect("connection B");
        let residue: i64 =
            sqlx::query_scalar("SELECT count(*) FROM principals WHERE email = ?")
                .bind(&email_a)
                .fetch_one(&mut *conn)
                .await
                .expect("count after A rolled back");
        assert_eq!(residue, 0, "body A left no residue behind");
        drop(conn);

        let mut tx = pool.begin().await.expect("BEGIN B");
        sqlx::query("INSERT INTO principals (id, email) VALUES (?, ?)")
            .bind(uuid::Uuid::now_v7())
            .bind(&email_b)
            .execute(&mut *tx)
            .await
            .expect("second body writes to the same table");
        tx.rollback().await.expect("second rollback");
    }

    // Final state: the table is exactly as both bodies found it.
    let mut conn = pool.acquire().await.expect("connection C");
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM principals WHERE email LIKE 'mysql-savepoint-%@it.test'",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("final count");
    assert_eq!(total, 0, "no transactional row survives its scope");
}

/// Phase B: the savepoint scope on the second fabric — the exact
/// counterpart of `pg_savepoint_rollback_isolates_two_tests_sharing_a_table`,
/// now over `kelson_mysql::test_support::Savepoint`, so both drivers expose the
/// same RefreshDatabase-equivalent primitive. (The existing
/// `mysql_transaction_rollback_*` test covers whole-transaction rollback via
/// `pool.begin()`, which is a different shape: it cannot scope a body that
/// keeps its connection and its transaction open.) Body A's insert lives only
/// inside its `BEGIN; SAVEPOINT kelson_test` scope, `rollback()` leaves no
/// residue for body B, and no schema drops/truncation are involved.
/// (DATABASE_URL-gated like every mysql matrix test; skips cleanly without
/// it. DDL inside the scope is NOT rolled back on MySQL — see the module doc.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_savepoint_rollback_isolates_two_tests_sharing_a_table() {
    let Some(pool) = mysql_pool().await else { return };
    let email_a = format!("mysql-sp-a-{}@it.test", uuid::Uuid::now_v7());
    let email_b = format!("mysql-sp-b-{}@it.test", uuid::Uuid::now_v7());

    // Test body A: writes inside the savepoint, sees them, rolls them back.
    {
        let mut conn = pool.acquire().await.expect("connection A");
        let mut sp = kelson_mysql::test_support::Savepoint::begin(&mut conn)
            .await
            .expect("BEGIN + SAVEPOINT kelson_test");
        sqlx::query("INSERT INTO principals (id, email) VALUES (?, ?)")
            .bind(uuid::Uuid::now_v7())
            .bind(&email_a)
            .execute(sp.conn())
            .await
            .expect("insert inside the scope");
        let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM principals WHERE email = ?")
            .bind(&email_a)
            .fetch_one(sp.conn())
            .await
            .expect("count inside the scope");
        assert_eq!(seen, 1, "the body sees its own write");
        sp.rollback().await.expect("ROLLBACK TO kelson_test");
    }

    // Test body B: a separate connection — A's row never existed for it,
    // and the shared table still accepts writes through its own scope.
    {
        let mut conn = pool.acquire().await.expect("connection B");
        let residue: i64 = sqlx::query_scalar("SELECT count(*) FROM principals WHERE email = ?")
            .bind(&email_a)
            .fetch_one(&mut *conn)
            .await
            .expect("count after A rolled back");
        assert_eq!(residue, 0, "body A left no residue behind");

        let mut sp = kelson_mysql::test_support::Savepoint::begin(&mut conn)
            .await
            .expect("second scope begins");
        sqlx::query("INSERT INTO principals (id, email) VALUES (?, ?)")
            .bind(uuid::Uuid::now_v7())
            .bind(&email_b)
            .execute(sp.conn())
            .await
            .expect("second body writes to the same table");
        sp.rollback().await.expect("second rollback");
    }

    // Final state: the table is exactly as both bodies found it.
    let mut conn = pool.acquire().await.expect("connection C");
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM principals WHERE email LIKE 'mysql-sp-%@it.test'",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("final count");
    assert_eq!(total, 0, "no savepoint row survives its scope");
}

/// B-31 skip-locked leg: every claim runs the port's
/// `FOR UPDATE SKIP LOCKED` transaction on MySQL 8. Concurrent claims over
/// separate connections take distinct rows (never a double-claim, never a
/// block), `complete` lands Succeeded, and `fail` requeues with the error
/// recorded — the claim/complete/fail cycle the worker lives on.
/// F-7 (row 7): a durable job enqueued BECAUSE of a request carries that
/// request's correlation — and the row in the DATABASE carries it, so a worker
/// can be traced back to the request that caused the work. The claim reads
/// the row back, so the proof covers the whole write→read path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_a_traced_job_row_carries_the_request_correlation() {
    let _guard = JOBS_QUEUE_LOCK.lock().await;
    use kelson_jobs::{build_from_def, JobDef, JobStore};

    let Some(pool) = mysql_pool().await else { return };
    // The queue is shared and PERSISTS across runs: without this, the claim
    // could hand back a row an earlier run left behind and the assertion
    // below would be about someone else's job.
    sqlx::query("DELETE FROM jobs_queue WHERE status IN ('pending', 'running')")
        .execute(&pool)
        .await
        .expect("clean claimable queue");
    let store = kelson_mysql::MySqlJobStore::new(pool.clone());

    let correlation = kelson_foundation::CorrelationId::new();
    let now = OffsetDateTime::now_utc();
    let mut job = build_from_def(&JobDef::new("mysql.trace.probe"), serde_json::json!({}), now);
    job.available_at = now - time::Duration::seconds(60);
    let job = job.with_origin(Some(correlation), Some(correlation));
    let id = store.enqueue(job).expect("the job enqueues");

    let claimed = store
        .claim("mysql-trace-worker")
        .expect("the claim runs")
        .expect("the job is claimable");
    assert_eq!(claimed.id, id);
    assert_eq!(
        claimed.correlation_id,
        Some(correlation),
        "the row in the database carries the request's correlation"
    );
    assert_eq!(claimed.causation_id, Some(correlation));
}


#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_job_store_claim_completes_and_fails_with_skip_locked() {
    let _guard = JOBS_QUEUE_LOCK.lock().await;
    use kelson_jobs::{JobStatus, JobStore, JobStoreAdmin};

    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlJobStore::new(pool.clone());

    // `claim` takes the globally first pending row (`ORDER BY priority DESC,
    // available_at, enqueued_at`, jobstore.rs) and THIS test deliberately
    // strands a pending row of its own — the `fail` requeue at the end
    // leaves attempt 1 re-queued. Re-running against that row (or against
    // rows an earlier interrupted run left behind) claims THOSE instead of
    // the two rows just enqueued, and the exact-ids assertion below fails
    // for a reason that has nothing to do with SKIP LOCKED. So start from an
    // empty claimable set: `jobs_queue` has no other writer inside this test
    // binary, and non-claimable rows are left alone.
    sqlx::query("DELETE FROM jobs_queue WHERE status IN ('pending', 'running')")
        .execute(&pool)
        .await
        .expect("clean claimable queue");

    let mut def = kelson_jobs::JobDef::new("mysql.claim.probe");
    def.max_attempts = 3;
    let now = OffsetDateTime::now_utc();
    let snap0 = store.snapshot().expect("baseline snapshot");

    // Two past-dated jobs: claimable immediately regardless of server-vs-
    // client clock skew (available_at <= NOW(6) on the server).
    let mut a = kelson_jobs::build_from_def(&def, serde_json::json!({ "n": 1 }), now);
    a.available_at = now - time::Duration::seconds(60);
    let id_a = store.enqueue(a).expect("enqueue a");
    let mut b = kelson_jobs::build_from_def(&def, serde_json::json!({ "n": 2 }), now);
    b.available_at = now - time::Duration::seconds(60);
    let id_b = store.enqueue(b).expect("enqueue b");
    assert_ne!(id_a, id_b);

    let snap1 = store.snapshot().expect("enqueue snapshot");
    assert_eq!(snap1.pending, snap0.pending + 2, "both jobs are pending");

    // Concurrent claims: two spawned tasks over separate pooled connections
    // (the sync port bridges through block_in_place on this multi-thread
    // runtime), so the SKIP LOCKED selectors race and hand each worker a
    // distinct row.
    let claim_a = tokio::spawn({
        let pool = pool.clone();
        async move { kelson_mysql::MySqlJobStore::new(pool).claim("mysql-worker-a") }
    });
    let claim_b = tokio::spawn({
        let pool = pool.clone();
        async move { kelson_mysql::MySqlJobStore::new(pool).claim("mysql-worker-b") }
    });
    let first = claim_a.await.expect("join a").expect("claim a").expect("row for a");
    let second = claim_b.await.expect("join b").expect("claim b").expect("row for b");
    assert_ne!(first.id, second.id, "skip-locked never double-claims");
    assert!(
        (first.id == id_a && second.id == id_b) || (first.id == id_b && second.id == id_a),
        "the two claims took exactly the two enqueued rows"
    );
    for claimed in [&first, &second] {
        assert_eq!(claimed.status, JobStatus::Running, "claim marks running");
        assert_eq!(claimed.attempt, 1, "attempt counted on claim");
        assert!(claimed.claimed_by.is_some(), "claimer recorded");
    }

    let snap2 = store.snapshot().expect("claim snapshot");
    assert_eq!(snap2.pending, snap0.pending, "queue drained");
    assert_eq!(snap2.running, snap0.running + 2, "both running");

    // complete: Succeeded and finished.
    store.complete(first.id).expect("completes");
    let done = store.get(first.id).expect("reads").expect("row");
    assert_eq!(done.status, JobStatus::Succeeded);
    assert!(done.finished_at.is_some(), "finish stamp lands");

    // fail: attempt 1 of 3 requeues with the error recorded.
    let failed = store.fail(second.id, "boom").expect("fails");
    assert_eq!(failed, JobStatus::Pending, "attempt 1 of 3 requeues");
    let row = store.get(second.id).expect("reads").expect("row");
    assert_eq!(row.last_error.as_deref(), Some("boom"), "error recorded");
    assert_eq!(row.attempt, 1);

    // Admin surface: both rows are listed for the operator.
    let listed = store.list(None, 100).expect("lists");
    assert!(listed.iter().any(|j| j.id == first.id), "succeeded row lists");
    assert!(listed.iter().any(|j| j.id == second.id), "requeued row lists");
}

/// P0 runtime-safety acceptance (queue crash recovery, MySQL fabric).
///
/// A worker that dies mid-execution leaves a `running` row whose owner no
/// longer exists. The claim lease makes that row recyclable: once the lease
/// expires a fresh worker reclaims it, the epoch (`attempt`) advances, and the
/// dead worker's stale completion/failure is refused by the fence.
///
/// The matrix runs the physical queue tests serially (`--test-threads=1`);
/// they share `jobs_queue`, exactly as the pre-existing claim test does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_job_store_recovers_orphaned_running_job_after_worker_crash() {
    let _guard = JOBS_QUEUE_LOCK.lock().await;
    use kelson_jobs::{build_from_def, JobDef, JobStatus, JobStore, JobStoreAdmin};

    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlJobStore::new(pool.clone());

    sqlx::query("DELETE FROM jobs_queue WHERE status IN ('pending', 'running')")
        .execute(&pool)
        .await
        .expect("clean claimable queue");

    let mut def = JobDef::new("mysql.reclaim.probe");
    def.max_attempts = 3;
    let now = OffsetDateTime::now_utc();
    let mut job = build_from_def(&def, serde_json::json!({}), now);
    job.available_at = now - time::Duration::seconds(60);
    let id = store.enqueue(job).expect("enqueue");

    // Worker A claims the job, then "crashes": it never completes or fails it.
    let claimed = store.claim("mysql-worker-A").expect("claim").expect("row");
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.status, JobStatus::Running);
    assert_eq!(claimed.attempt, 1, "the claim epoch starts at 1");

    // A claim INSIDE its lease is never stolen — reclaim is lease-gated.
    assert!(
        store.claim("mysql-worker-B").expect("live claim").is_none(),
        "a healthy in-flight claim must not be taken by another worker"
    );

    // Time passes: the abandoned claim is now a full lease old.
    sqlx::query("UPDATE jobs_queue SET claimed_at = claimed_at - INTERVAL 1 HOUR WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .expect("age the abandoned claim");

    // Worker B starts. The row is `running`, its owner is gone and the lease
    // has expired, so recovery hands it back instead of orphaning it.
    let recovered = store
        .claim("mysql-worker-B")
        .expect("reclaim")
        .expect("a crashed worker's in-flight job is reclaimed, not orphaned");
    assert_eq!(recovered.id, id);
    assert_eq!(recovered.attempt, 2, "the reclaim advances the epoch");
    assert_eq!(
        recovered.claimed_by.as_deref(),
        Some("mysql-worker-B"),
        "ownership moves to the reclaiming worker"
    );

    // Fencing: the resurrected worker A cannot finish (or fail) a job whose
    // claim it lost — it presents the stale token (epoch 1, worker A).
    let stale = store
        .complete_claim(id, 1, "mysql-worker-A")
        .expect_err("a stale claim cannot complete");
    assert_eq!(stale.code, kelson_foundation::codes::CONFLICT);
    let stale = store
        .fail_claim(id, 1, "mysql-worker-A", "stale worker")
        .expect_err("a stale claim cannot fail");
    assert_eq!(stale.code, kelson_foundation::codes::CONFLICT);
    let alive = store.get(id).expect("reads").expect("row");
    assert_eq!(
        alive.status,
        JobStatus::Running,
        "the fence left the new owner's claim alone"
    );

    // The fence guards the admin transitions a claimed job can take too.
    let stale = store
        .defer_claim(id, 1, "mysql-worker-A", now + time::Duration::seconds(30))
        .expect_err("a stale claim cannot defer");
    assert_eq!(stale.code, kelson_foundation::codes::CONFLICT);
    let stale = store
        .kill_claim(id, 1, "mysql-worker-A", "stale worker")
        .expect_err("a stale claim cannot kill");
    assert_eq!(stale.code, kelson_foundation::codes::CONFLICT);

    // The real owner finishes normally.
    store
        .complete_claim(id, 2, "mysql-worker-B")
        .expect("current claim completes");
    assert_eq!(
        store.get(id).expect("reads").expect("row").status,
        JobStatus::Succeeded
    );

    sqlx::query("DELETE FROM jobs_queue WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .expect("clean up the probe row");
}

/// The claim epoch alone is not a safe fence: `defer` hands an attempt back
/// (admission `Hold` must not burn the retry budget), so a deferred job can be
/// re-claimed at the SAME epoch. The owner half of the token closes that: the
/// deferred worker shares the epoch with the new claim but not the ownership.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_job_store_fences_a_deferred_job_reclaimed_at_the_same_epoch() {
    let _guard = JOBS_QUEUE_LOCK.lock().await;
    use kelson_jobs::{build_from_def, JobDef, JobStatus, JobStore, JobStoreAdmin};

    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlJobStore::new(pool.clone());

    sqlx::query("DELETE FROM jobs_queue WHERE status IN ('pending', 'running')")
        .execute(&pool)
        .await
        .expect("clean claimable queue");

    let mut def = JobDef::new("mysql.defer.probe");
    def.max_attempts = 3;
    let now = OffsetDateTime::now_utc();
    let mut job = build_from_def(&def, serde_json::json!({}), now);
    job.available_at = now - time::Duration::seconds(60);
    let id = store.enqueue(job).expect("enqueue");

    let claimed = store.claim("mysql-worker-A").expect("claim").expect("row");
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.attempt, 1);

    // Worker A holds the job, then admission holds it: the attempt is given
    // back (pending / attempt 0) and the claim is abandoned. The hold expires
    // in the past, so the row is claimable immediately (and regardless of
    // client-vs-server clock skew).
    store
        .defer_claim(
            id,
            claimed.attempt,
            "mysql-worker-A",
            now - time::Duration::seconds(60),
        )
        .expect("defer");
    let deferred = store.get(id).expect("reads").expect("row");
    assert_eq!(deferred.status, JobStatus::Pending, "defer returns it to pending");
    assert_eq!(deferred.attempt, 0, "the attempt is given back");
    assert!(deferred.claimed_by.is_none(), "the abandoned claim is disowned");

    // Worker B claims it — and lands on the SAME epoch A held.
    let reclaimed = store.claim("mysql-worker-B").expect("reclaim").expect("row");
    assert_eq!(reclaimed.id, id);
    assert_eq!(reclaimed.attempt, 1, "the deferred attempt was given back");
    assert_eq!(reclaimed.claimed_by.as_deref(), Some("mysql-worker-B"));

    // A resurrected A presenting (epoch 1, owner A) must still be refused:
    // the row is running at epoch 1, but under worker B.
    let stale = store
        .complete_claim(id, 1, "mysql-worker-A")
        .expect_err("a deferred worker cannot finish the claim that replaced its own");
    assert_eq!(stale.code, kelson_foundation::codes::CONFLICT);
    assert_eq!(
        store.get(id).expect("reads").expect("row").status,
        JobStatus::Running,
        "the refused write left the running claim alone"
    );

    store
        .complete_claim(id, 1, "mysql-worker-B")
        .expect("the current owner completes");

    sqlx::query("DELETE FROM jobs_queue WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .expect("clean up the probe row");
}

/// The poison-job bound: an abandoned claim whose retry budget is already
/// spent is dead-lettered by the claim sweep, so it becomes terminal and
/// countable instead of sitting `running` forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_job_store_dead_letters_an_exhausted_orphan_claim() {
    let _guard = JOBS_QUEUE_LOCK.lock().await;
    use kelson_jobs::{build_from_def, JobDef, JobStatus, JobStore, JobStoreAdmin};

    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlJobStore::new(pool.clone());

    sqlx::query("DELETE FROM jobs_queue WHERE status IN ('pending', 'running')")
        .execute(&pool)
        .await
        .expect("clean claimable queue");

    let mut def = JobDef::new("mysql.exhausted.probe");
    def.max_attempts = 1;
    let now = OffsetDateTime::now_utc();
    let mut job = build_from_def(&def, serde_json::json!({}), now);
    job.available_at = now - time::Duration::seconds(60);
    let id = store.enqueue(job).expect("enqueue");

    let claimed = store.claim("mysql-worker-A").expect("claim").expect("row");
    assert_eq!(claimed.attempt, 1);
    assert_eq!(claimed.max_attempts, 1, "the retry budget is already spent");

    sqlx::query("UPDATE jobs_queue SET claimed_at = claimed_at - INTERVAL 1 HOUR WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .expect("age the abandoned claim");

    // No worker may reclaim it: the budget is gone, so the sweep makes the
    // row terminal and this claim finds nothing.
    assert!(store.claim("mysql-worker-B").expect("sweep").is_none());
    let row = store.get(id).expect("reads").expect("row");
    assert_eq!(
        row.status,
        JobStatus::Dead,
        "an exhausted orphan becomes dead, not invisible"
    );
    assert_eq!(
        row.last_error.as_deref(),
        Some("claim lease expired; retries exhausted")
    );

    sqlx::query("DELETE FROM jobs_queue WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .expect("clean up the probe row");
}

/// The tenant-scoped queue acceptance gate (P0 finding F5, fixed by migration
/// `20261005000004_jobs_queue_tenant_binary.sql`).
///
/// The column shipped as `CHAR(36)` while both data paths treat it as a 16-byte
/// UUID — `enqueue` binds `*tenant.as_uuid()` (raw bytes, which MySQL refuses
/// to store in a utf8mb4 column) and `job_from_row` decodes `Option<Uuid>`
/// (binary) — so every tenant-scoped enqueue failed server-side with
/// `1366 (HY000): Incorrect string value: '\x…' for column 'tenant_id' at row
/// 1` and only the unscoped (tenant-NULL) path worked. The fix converts the
/// column to `BINARY(16)` (the twin's UUID form everywhere else; lossless, no
/// write could ever have succeeded), which also makes the tenancy filter the
/// reclaim arm carries testable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_job_store_tenant_scoped_queue_is_usable_and_isolated() {
    let _guard = JOBS_QUEUE_LOCK.lock().await;
    use kelson_jobs::{build_from_def, JobDef, JobStatus, JobStoreAdmin, TenantJobStore};

    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlJobStore::new(pool.clone());
    let tenant_a = TenantId::from_uuid(uuid::Uuid::now_v7());
    let tenant_b = TenantId::from_uuid(uuid::Uuid::now_v7());

    let now = OffsetDateTime::now_utc();
    let mut job = build_from_def(
        &JobDef::new("mysql.tenant.probe"),
        serde_json::json!({}),
        now,
    );
    job.available_at = now - time::Duration::seconds(60);
    let id = store.enqueue_for(tenant_a, job).expect("tenant enqueue");

    // Another tenant never sees it...
    assert!(
        store
            .claim_for(tenant_b, "mysql-worker-B")
            .expect("cross-tenant claim")
            .is_none(),
        "another tenant's pending job is never visible"
    );
    // ...its owner does...
    assert_eq!(
        store
            .claim_for(tenant_a, "mysql-worker-A")
            .expect("own claim")
            .expect("own pending job is claimable")
            .id,
        id
    );
    // ...and an EXPIRED claim stays invisible across the tenant boundary.
    sqlx::query("UPDATE jobs_queue SET claimed_at = claimed_at - INTERVAL 1 HOUR WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .expect("age the claim");
    assert!(
        store
            .claim_for(tenant_b, "mysql-worker-B")
            .expect("cross-tenant reclaim")
            .is_none(),
        "tenant scoping survives the reclaim arm"
    );

    // The reclaim lands inside the owning tenant, and the fenced terminal
    // writes keep the trust boundary: a foreign tenant gets NOT_FOUND (no
    // existence oracle), a stale token gets CONFLICT, the current owner wins.
    let recovered = store
        .claim_for(tenant_a, "mysql-worker-A2")
        .expect("own reclaim")
        .expect("the expired claim is reclaimed inside its tenant");
    assert_eq!(recovered.id, id);
    assert_eq!(recovered.attempt, 2, "the reclaim advances the epoch");

    let foreign = store
        .complete_claim_for(tenant_b, id, 2, "mysql-worker-A2")
        .expect_err("a foreign tenant cannot complete the job");
    assert_eq!(foreign.code, kelson_foundation::codes::NOT_FOUND);
    let stale = store
        .fail_claim_for(tenant_a, id, 1, "mysql-worker-A", "stale worker")
        .expect_err("a stale claim cannot fail");
    assert_eq!(stale.code, kelson_foundation::codes::CONFLICT);

    store
        .complete_claim_for(tenant_a, id, 2, "mysql-worker-A2")
        .expect("the current owner completes");
    assert_eq!(
        store.get(id).expect("reads").expect("row").status,
        JobStatus::Succeeded
    );

    sqlx::query("DELETE FROM jobs_queue WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .expect("clean up the probe row");
}

/// PostgreSQL twin of the reclaim/fence acceptance. Skipped when DATABASE_URL
/// is not a reachable `postgres://` URL (see the module note).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_job_store_recovers_orphaned_running_job_after_worker_crash() {
    let _guard = JOBS_QUEUE_LOCK.lock().await;
    use kelson_jobs::{build_from_def, JobDef, JobStatus, JobStore, JobStoreAdmin};

    let Some(pool) = pool().await else { return };
    let store = kelson_postgres::PgJobStore::new(pool.clone());

    sqlx::query("DELETE FROM jobs_queue WHERE status IN ('pending', 'running')")
        .execute(&pool)
        .await
        .expect("clean claimable queue");

    let mut def = JobDef::new("pg.reclaim.probe");
    def.max_attempts = 3;
    let now = OffsetDateTime::now_utc();
    let mut job = build_from_def(&def, serde_json::json!({}), now);
    job.available_at = now - time::Duration::seconds(60);
    let id = store.enqueue(job).expect("enqueue");

    let claimed = store.claim("pg-worker-A").expect("claim").expect("row");
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.attempt, 1);
    assert!(
        store.claim("pg-worker-B").expect("live claim").is_none(),
        "claims inside the lease are not stolen"
    );

    sqlx::query("UPDATE jobs_queue SET claimed_at = claimed_at - INTERVAL '1 hour' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("age the abandoned claim");

    let recovered = store
        .claim("pg-worker-B")
        .expect("reclaim")
        .expect("a crashed worker's job is reclaimed, not orphaned");
    assert_eq!(recovered.id, id);
    assert_eq!(recovered.attempt, 2);

    let stale = store
        .complete_claim(id, 1, "pg-worker-A")
        .expect_err("a stale claim cannot complete");
    assert_eq!(stale.code, kelson_foundation::codes::CONFLICT);
    store
        .complete_claim(id, 2, "pg-worker-B")
        .expect("current claim completes");
    assert_eq!(
        store.get(id).expect("reads").expect("row").status,
        JobStatus::Succeeded
    );

    sqlx::query("DELETE FROM jobs_queue WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("clean up the probe row");
}
