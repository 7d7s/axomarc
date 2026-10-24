//! Live-Redis integration test for the Redis **session** store.
//!
//! Skipped unless `REDIS_URL` points at a reachable Redis — the same
//! "skip cleanly without a service" posture the pg/mysql matrices and the
//! rate-limit adapter's live test use, so a local run without Redis stays
//! green.
//!
//! ```text
//! REDIS_URL=redis://127.0.0.1:6379 cargo test -p kelson-session-redis --features redis-tests
//! ```
//!
//! The adapter had NO live coverage before this file: only its key format was
//! unit-tested, so "the session survives a round trip through Redis" and "the
//! TTL is actually set" were unproven against a real server.

#![cfg(feature = "redis-tests")]

use kelson_foundation::PrincipalId;
use kelson_session::{Session, SessionStore};
use kelson_session_redis::RedisSessionStore;
use time::OffsetDateTime;

fn redis_url() -> Option<String> {
    match std::env::var("REDIS_URL") {
        Ok(url) if !url.is_empty() => Some(url),
        _ => {
            eprintln!("redis-tests: REDIS_URL not set; skipping");
            None
        }
    }
}

/// A session survives the round trip, carries its bag, and is gone after the
/// delete. The prefix is unique per run, so a case never sees another's rows.
#[test]
fn a_session_round_trips_through_redis() {
    let Some(url) = redis_url() else { return };
    let Ok(store) = RedisSessionStore::connect(&url) else {
        eprintln!("redis-tests: redis unreachable; skipping");
        return;
    };
    let store = store.with_prefix(format!("keel:test:session:{}:", uuid::Uuid::now_v7()));

    let now = OffsetDateTime::now_utc();
    let mut session = Session::new(PrincipalId::new(), None, now);
    session
        .data
        .insert("cart".to_string(), serde_json::json!("cart-42"));

    store.save(&session).expect("the session saves");
    let loaded = store
        .load(&session.id)
        .expect("the session loads")
        .expect("the session is there");
    assert_eq!(loaded.id, session.id, "the id survives");
    assert_eq!(
        loaded.principal, session.principal,
        "the principal survives"
    );
    assert_eq!(
        loaded.data.get("cart"),
        Some(&serde_json::json!("cart-42")),
        "the session bag survives"
    );

    store.delete(&session.id).expect("the session deletes");
    assert!(
        store
            .load(&session.id)
            .expect("the load after delete")
            .is_none(),
        "a deleted session is gone"
    );
}

/// Two connections are the SAME session store: a session written by one is
/// read by the other. (This is the property the deployment relies on when the
/// app runs on several instances behind a load balancer.)
#[test]
fn two_connections_share_the_session() {
    let Some(url) = redis_url() else { return };
    let Ok(_store) = RedisSessionStore::connect(&url) else {
        eprintln!("redis-tests: redis unreachable; skipping");
        return;
    };
    let prefix = format!("keel:test:session:{}:", uuid::Uuid::now_v7());
    let writer = RedisSessionStore::connect(&url)
        .expect("the first connection")
        .with_prefix(prefix.clone());
    let reader = RedisSessionStore::connect(&url)
        .expect("the second connection")
        .with_prefix(prefix);

    let session = Session::new(PrincipalId::new(), None, OffsetDateTime::now_utc());
    writer.save(&session).expect("the session saves");
    let loaded = reader
        .load(&session.id)
        .expect("the session loads")
        .expect("the other connection sees it");
    assert_eq!(loaded.id, session.id);
}

/// The stored row carries a TTL — a session must not outlive its policy, and
/// Redis (not the app) has to expire it.
#[test]
fn the_stored_session_carries_a_ttl() {
    let Some(url) = redis_url() else { return };
    let Ok(store) = RedisSessionStore::connect(&url) else {
        eprintln!("redis-tests: redis unreachable; skipping");
        return;
    };
    let prefix = format!("keel:test:session:{}:", uuid::Uuid::now_v7());
    let store = store.with_prefix(prefix.clone());

    let session = Session::new(PrincipalId::new(), None, OffsetDateTime::now_utc());
    store.save(&session).expect("the session saves");

    let client = redis::Client::open(url).expect("the client opens");
    let mut conn = client.get_connection().expect("the connection opens");
    let ttl: i64 = redis::cmd("TTL")
        .arg(format!("{prefix}{}", session.id.as_str()))
        .query(&mut conn)
        .expect("the TTL is readable");
    assert!(
        ttl > 0 && ttl <= 86_400,
        "the session key expires on its own (ttl was {ttl})"
    );

    store.delete(&session.id).expect("cleanup");
}

/// The store's count sees only its own namespace.
#[test]
fn count_sees_only_its_own_namespace() {
    let Some(url) = redis_url() else { return };
    let Ok(store) = RedisSessionStore::connect(&url) else {
        eprintln!("redis-tests: redis unreachable; skipping");
        return;
    };
    let prefix = format!("keel:test:session:{}:", uuid::Uuid::now_v7());
    let store = store.with_prefix(prefix.clone());

    let session = Session::new(PrincipalId::new(), None, OffsetDateTime::now_utc());
    store.save(&session).expect("the session saves");
    assert_eq!(store.count(), 1, "the one stored session is counted");
    store.delete(&session.id).expect("cleanup");
}
