//! Live-Redis integration test for the **durable Redis job store**.
//!
//! Skipped unless `REDIS_URL` points at a reachable Redis — the same
//! "skip cleanly without a service" posture the other live suites use.
//!
//! ```text
//! REDIS_URL=redis://127.0.0.1:6379 cargo test -p kelson-cache-redis --features redis-tests
//! ```
//!
//! The durable job store had NO live coverage: its claim exclusivity comes
//! from a Lua script (`CLAIM_SCRIPT` — ZREM inside one round-trip), and a
//! Lua script's behaviour against a real Redis is exactly what a unit test
//! cannot show.

#![cfg(feature = "redis-tests")]

use kelson_cache_redis::RedisJobStore;
use kelson_foundation::TenantId;
use kelson_jobs::{build_from_def, JobDef, JobStatus, JobStore, JobStoreAdmin, TenantJobStore};
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

fn store(prefix: &str) -> Option<RedisJobStore> {
    let url = redis_url()?;
    match RedisJobStore::connect(&url) {
        Ok(store) => Some(store.with_prefix(prefix.to_string())),
        Err(_) => {
            eprintln!("redis-tests: redis unreachable; skipping");
            None
        }
    }
}

fn job(name: &str) -> kelson_jobs::DurableJob {
    let now = OffsetDateTime::now_utc();
    let mut job = build_from_def(&JobDef::new(name), serde_json::json!({}), now);
    // Available now, not in the future.
    job.available_at = now - time::Duration::seconds(60);
    job
}

/// A job survives enqueue → claim → complete, and a claimed job is gone from
/// the queue.
#[test]
fn a_job_round_trips_through_redis() {
    let Some(store) = store(&format!("keel:test:jobs:{}:", uuid::Uuid::now_v7())) else {
        return;
    };
    let id = store.enqueue(job("live.probe")).expect("the job enqueues");

    let claimed = store
        .claim("worker-a")
        .expect("the claim runs")
        .expect("the job is claimable");
    assert_eq!(claimed.id, id, "the claimed job is the enqueued one");
    assert_eq!(claimed.status, JobStatus::Running);

    assert!(
        store.claim("worker-b").expect("the second claim").is_none(),
        "a claimed job is not handed out twice"
    );

    store.complete(id).expect("the job completes");
    assert!(
        store.claim("worker-c").expect("the third claim").is_none(),
        "a completed job is not claimable again"
    );
}

/// The claim is exclusive across **workers**: the Lua claim script removes the
/// member inside one round-trip, so two workers never take the same job.
#[test]
fn two_workers_never_claim_the_same_job() {
    let Some(store) = store(&format!("keel:test:jobs:{}:", uuid::Uuid::now_v7())) else {
        return;
    };
    store
        .enqueue(job("live.exclusive"))
        .expect("the job enqueues");

    let first = store.claim("worker-a").expect("the first claim");
    let second = store.claim("worker-b").expect("the second claim");
    assert!(first.is_some(), "the job is claimed");
    assert!(
        second.is_none(),
        "the second worker finds nothing — the claim is exclusive"
    );
}

/// A tenant-scoped job is invisible to another tenant's claim.
#[test]
fn a_tenant_job_is_invisible_to_another_tenant() {
    let Some(store) = store(&format!("keel:test:jobs:{}:", uuid::Uuid::now_v7())) else {
        return;
    };
    let tenant_a = TenantId::new();
    let tenant_b = TenantId::new();

    let id = store
        .enqueue_for(tenant_a, job("live.tenant"))
        .expect("the tenant enqueue");
    assert!(
        store
            .claim_for(tenant_b, "worker-b")
            .expect("the foreign claim")
            .is_none(),
        "another tenant never sees the job"
    );
    let claimed = store
        .claim_for(tenant_a, "worker-a")
        .expect("the own claim")
        .expect("the owner claims its job");
    assert_eq!(claimed.id, id);
}

/// A failure below the attempt ceiling re-queues the job (and records the
/// error); the exhausted job is dead-lettered instead of retried forever.
#[test]
fn a_failure_requeues_and_an_exhausted_job_is_dead_lettered() {
    let Some(store) = store(&format!("keel:test:jobs:{}:", uuid::Uuid::now_v7())) else {
        return;
    };

    // A job with the default attempt ceiling: one failure re-queues it.
    let id = store.enqueue(job("live.fail")).expect("the job enqueues");
    let claimed = store
        .claim("worker-a")
        .expect("the claim runs")
        .expect("the job is claimable");
    assert_eq!(claimed.id, id);
    let status = store.fail(id, "boom").expect("the fail runs");
    assert_eq!(
        status,
        JobStatus::Pending,
        "a failure below the ceiling re-queues the job"
    );
    let listed = store.list(None, 50).expect("the admin list runs");
    let failed = listed
        .iter()
        .find(|j| j.id == id)
        .expect("the failed job is visible to the admin");
    assert_eq!(
        failed.last_error.as_deref(),
        Some("boom"),
        "the error is recorded"
    );

    // A job that may not be retried is dead-lettered.
    let mut def = JobDef::new("live.dead");
    def.max_attempts = 1;
    let now = OffsetDateTime::now_utc();
    let mut dead_job = build_from_def(&def, serde_json::json!({}), now);
    dead_job.available_at = now - time::Duration::seconds(60);
    let dead_id = store.enqueue(dead_job).expect("the job enqueues");
    store
        .claim("worker-a")
        .expect("the claim runs")
        .expect("the job is claimable");
    let status = store.fail(dead_id, "fatal").expect("the fail runs");
    assert_eq!(
        status,
        JobStatus::Dead,
        "an exhausted job is dead-lettered, not retried"
    );
}
