//! End-to-end testkit smoke: TestApp over a tiny router, fakes, factories
//! and side-effect assertions.

use axum::extract::Request;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use time::{Duration, OffsetDateTime};

use keel_testkit::{
    assert_delta, assert_event, sample_event, sample_job, AuditRecorder, EventFactory, Factory,
    FakeMail, FakeQueue, FakeStorage, InMemoryBackend, InMemoryOutbox, JobFactory, StoragePort,
    TestApp,
};

async fn ping() -> Json<serde_json::Value> {
    Json(json!({ "pong": true, "service": "testkit" }))
}

async fn echo_identity(req: Request) -> Json<serde_json::Value> {
    let actor = req
        .headers()
        .get("x-keel-actor")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("anonymous")
        .to_string();
    let tenant = req
        .headers()
        .get("x-keel-tenant")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("none")
        .to_string();
    Json(json!({ "actor": actor, "tenant": tenant }))
}

fn router() -> Router {
    Router::new()
        .route("/ping", get(ping))
        .route("/identity", post(echo_identity))
}

#[tokio::test]
async fn test_app_serves_requests_with_status_and_json_asserts() {
    let app = TestApp::new(router());
    app.get("/ping")
        .send()
        .await
        .assert_status(200)
        .assert_json_contains(json!({ "pong": true }));
    app.get("/missing").send().await.assert_status(404);
}

#[tokio::test]
async fn test_app_applies_actor_and_tenant_headers() {
    let app = TestApp::new(router())
        .acting_as("principal-42")
        .tenant("tenant-dev");
    let res = app.post("/identity").json(&json!({})).send().await;
    res.assert_status(200).assert_json_eq(json!({
        "actor": "principal-42",
        "tenant": "tenant-dev"
    }));
}

#[test]
fn fake_queue_records_and_asserts_jobs() {
    let queue = FakeQueue::new();
    queue.enqueue(sample_job("mail.send"));
    queue.enqueue(sample_job("mail.send"));
    let claimed = queue.assert_enqueued("mail.send");
    assert_eq!(claimed.name, "mail.send");
    assert_eq!(queue.count("mail.send"), 2);
}

#[test]
fn outbox_assert_event_finds_by_name() {
    let outbox = InMemoryOutbox::new();
    outbox.push(sample_event("tenant.created"));
    let event = assert_event(&outbox, "tenant.created");
    assert_eq!(event.topic, "test");
    assert!(!event.published);
}

#[test]
fn fake_mail_records_messages() {
    let mail = FakeMail::new();
    mail.send("a@b.c", "Welcome", "hello");
    let message = mail.assert_sent_to("a@b.c");
    assert_eq!(message.subject, "Welcome");
    mail.assert_count(1);
}

#[test]
fn fake_storage_round_trips_objects() {
    let storage = FakeStorage::new();
    let reference = storage.put("files", "report.pdf", vec![1, 2, 3]).unwrap();
    assert_eq!(reference.bucket, "files");
    assert!(storage.contains("files", "report.pdf"));
    assert_eq!(storage.get(&reference).unwrap(), vec![1, 2, 3]);
    assert!(storage
        .presigned_url(&reference, 60)
        .unwrap()
        .starts_with("fake://"));
    storage.delete(&reference).unwrap();
    assert!(storage.get(&reference).is_err());
}

#[test]
fn audit_recorder_asserts_actions() {
    let recorder = AuditRecorder::new();
    recorder.record_success("appointment.create", "appointments");
    let record = recorder.assert_audit("appointment.create");
    assert_eq!(record.resource, "appointments");
}

#[test]
#[should_panic(expected = "expected deltas")]
fn assert_delta_panics_when_no_stones() {
    assert_delta(&InMemoryBackend::new(), "appointment");
}

#[test]
fn job_factory_overrides_apply() {
    let job = JobFactory::make_with(|f| {
        f.name = "report.build".into();
        f.priority = 9;
        f.available_in = Some(Duration::minutes(10));
    });
    assert_eq!(job.name, "report.build");
    assert_eq!(job.priority, 9);
    assert!(job.available_at > OffsetDateTime::now_utc());
}

#[test]
fn event_factory_builds_unpublished_events() {
    let event = EventFactory::default()
        .event("x.done")
        .topic("audit")
        .build();
    assert_eq!(event.event, "x.done");
    assert_eq!(event.topic, "audit");
    assert!(!event.published);
}
