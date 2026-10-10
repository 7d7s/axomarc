//! End-to-end testkit smoke: TestApp over a tiny router, fakes, factories
//! and side-effect assertions.

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use time::{Duration, OffsetDateTime};

use kelson_testkit::{
    assert_delta, assert_event, sample_event, sample_job, AuditRecorder, CacheFake, CachePort,
    EventFactory, EventFake, Factory, FakeMail, FakeQueue, FakeStorage, HttpFake, HttpClientPort,
    HttpRequest, HttpResponse, InMemoryBackend, InMemoryOutbox, JobFactory, NotificationFake,
    QueueFake, StorageFake, StoragePort, TestApp,
};

async fn ping() -> Json<serde_json::Value> {
    Json(json!({ "pong": true, "service": "testkit" }))
}

async fn echo_identity(req: Request) -> Json<serde_json::Value> {
    let actor = req
        .headers()
        .get("x-kelson-actor")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("anonymous")
        .to_string();
    let tenant = req
        .headers()
        .get("x-kelson-tenant")
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

// ------------------------------------------------------------- Wave 7 P1

#[test]
fn wave7_queue_event_and_notification_fakes_surface() {
    let queue = QueueFake::new();
    queue.enqueue(sample_job("mail.send"));
    queue.assert_pushed("mail.send", |job| job.name == "mail.send");
    queue.assert_pushed_count("mail.send", 1);
    queue.assert_not_pushed("report.build");

    let events = EventFake::new();
    events.dispatch(sample_event("tenant.created"));
    events.assert_dispatched("tenant.created", |event| !event.published);
    events.assert_not_dispatched("invoice.posted");

    assert!(NotificationFake::new().records().is_empty());
}

#[test]
fn wave7_storage_and_cache_fakes_surface() {
    let storage = StorageFake::new();
    storage.put("files", "a.txt", vec![1, 2, 3]).unwrap();
    storage.assert_exists("files/a.txt");
    storage.assert_missing("files/b.txt");
    storage.assert_directory_empty("logs/");

    let cache = CacheFake::new();
    cache.put("query:products", b"[]".to_vec(), Some(30));
    cache.assert_has("query:products");
    assert_eq!(cache.ttl_secs("query:products"), Some(30));
    cache.delete("query:products");
    cache.assert_missing("query:products");
}

#[tokio::test]
async fn http_fake_reexport_records_requests() {
    let fake = HttpFake::new();
    fake.enqueue(HttpResponse {
        status: 200,
        headers: Default::default(),
        body: b"pong".to_vec(),
    });

    let request = HttpRequest::json("POST", "https://api.example.test/ping", &json!({ "x": 1 }))
        .expect("json request builds");
    let response = fake.execute(&request).await.expect("queued response");
    assert_eq!(response.status, 200);

    fake.assert_request_count(1);
    fake.assert_sent(|r| r.method == "POST" && r.url == "https://api.example.test/ping");
    fake.assert_no_pending();
}

#[test]
#[should_panic(expected = "never consumed")]
fn http_fake_assert_no_pending_panics_with_unconsumed_response() {
    let fake = HttpFake::new();
    fake.enqueue(HttpResponse {
        status: 200,
        headers: Default::default(),
        body: Vec::new(),
    });
    fake.assert_no_pending();
}

// ------------------------------------------------------------- Wave 7 P2

#[tokio::test]
async fn wave7_response_assertion_matrix_surface() {
    let app = TestApp::new(
        Router::new()
            .route("/ping", get(ping))
            .route(
                "/go",
                get(|| async { ([(header::LOCATION, "/next")], StatusCode::SEE_OTHER) }),
            )
            .route("/page", get(|| async { "<p>Tom &amp; Jerry</p>" })),
    );

    app.get("/go").send().await
        .assert_redirect("/next")
        .assert_redirect("/n");
    app.get("/page").send().await
        .assert_html_contains("Tom &amp; Jerry")
        .assert_see("Tom & Jerry");
    app.get("/ping").send().await.assert_json_path("pong", json!(true));
}

// ------------------------------------------------------------- Wave 7 P3

/// The pub surface a generated factory or a downstream test names from
/// OUTSIDE the crate — the gap class a missing `pub use` hides from
/// same-crate unit tests (the P2 ViewRecorder bug): time travel, seeded
/// sequences, state() overrides, ModelFactory persistence through the
/// ModelStore, and the view probe type.
#[test]
fn wave7_test_infrastructure_surface() {
    use kelson_foundation::{Clock, TenantId};
    use kelson_models::InMemoryModelStore;
    use kelson_testkit::{
        freeze_time, travel, ModelDef, ModelFactory, ModelStore, Record, Sequence, ViewRecorder,
    };

    // freeze_time + travel compose; the concrete FixedClock makes an
    // external crate name the Clock trait itself.
    let clock = freeze_time(OffsetDateTime::UNIX_EPOCH);
    travel(&clock, 90);
    assert_eq!(
        clock.now(),
        OffsetDateTime::UNIX_EPOCH + Duration::seconds(90)
    );

    // A seeded counter reproduces from its seed (no process-wide leak).
    let seq = Sequence::seeded(7);
    assert_eq!(seq.next(), 7);
    assert_eq!(seq.next(), 8);
    assert_eq!(Sequence::seeded(7).next(), 7);

    // state() overrides chain before build on a stock factory.
    let job = JobFactory::default()
        .state(|f| f.name = "billing.sync".into())
        .build();
    assert_eq!(job.name, "billing.sync");

    // ModelFactory the way codegen templates implement it: model +
    // to_record, with the provided create() persisting through the store
    // (stamps tenant_id on tenant_scoped models and a missing id).
    #[derive(Default)]
    struct NoteFactory;

    impl Factory for NoteFactory {
        type Item = String;

        fn build(self) -> String {
            "standalone note".into()
        }
    }

    impl ModelFactory for NoteFactory {
        fn model(&self) -> ModelDef {
            ModelDef::new("Note", "notes").tenant_scoped()
        }

        fn to_record(item: &Self::Item) -> Record {
            let mut record = Record::new();
            record.insert("body".into(), json!(item.as_str()));
            record
        }
    }

    let mem = InMemoryModelStore::new();
    let store: &dyn ModelStore = &mem;
    let tenant = TenantId::new();
    let row = NoteFactory
        .create(store, tenant)
        .expect("row persists");
    assert_eq!(row["body"], json!("standalone note"));
    assert_eq!(row["tenant_id"], json!(tenant.to_string()));
    assert!(row["id"]
        .as_str()
        .expect("create stamps the missing id")
        .parse::<uuid::Uuid>()
        .is_ok());

    // The view probe type is reachable from outside the crate too.
    let views = ViewRecorder::new();
    assert!(views.renders().is_empty());
}

/// Wave 8 (docs/19 "Helpers"): the Str/inflection helper surface is
/// re-exported from `kelson-foundation` — names and behavior callable from
/// outside the crate, the same external-surface proof the Wave 7 infra
/// smoke gives its exports.
#[test]
fn wave8_str_helpers_surface() {
    use kelson_foundation::{excerpt, plural, random, singular, to_title_case, ulid, uuid, words};

    assert_eq!(to_title_case("hello_world"), "Hello World");
    assert_eq!(words("one two three", 2, "..."), "one two...");
    assert!(excerpt("the quick brown fox", "brown", 6, 4, 100).contains("brown"));
    assert_eq!(plural("person"), "people");
    assert_eq!(singular("people"), "person");
    assert_eq!(plural("sheep"), "sheep");

    let token = random(12);
    assert_eq!(token.len(), 12);
    assert_eq!(ulid().len(), 26);
    assert_eq!(uuid().get_version(), Some(uuid::Version::Random));
}

/// Wave 8 P2 (docs/19 "Helpers"): the Number/date helper surface is
/// re-exported from `kelson-foundation` and callable from outside the crate.
#[test]
fn wave8_number_and_date_helpers_surface() {
    use kelson_foundation::{
        clamp, format_cents, format_currency, format_date_with, format_percent,
        format_precision, in_tz, now_in, parse_date_with, timezone_offset_secs, MoneyCents,
    };

    assert_eq!(format_currency(1234.56, "USD", "en"), "$1,234.56");
    assert_eq!(format_currency(1234.56, "EUR", "fr"), "1 234,56 €");
    assert_eq!(format_percent(75.0, 1, "fr"), "75,0 %");
    assert_eq!(format_precision(1234.5, 2, "en"), "1,234.50");
    assert_eq!(clamp(42.0, 0.0, 10.0), 10.0);
    let cents = MoneyCents::new(999, "USD").expect("iso code");
    assert_eq!(format_cents(&cents, "en"), "$9.99");

    let parts = parse_date_with("2024-01-02", "Y-m-d").expect("parses");
    let at = parts.to_instant().expect("full date");
    assert_eq!(format_date_with(at, "d/m/Y").unwrap(), "02/01/2024");
    assert!(in_tz(at, "Asia/Kolkata").is_some());
    assert!(now_in("UTC").is_some());
    assert_eq!(timezone_offset_secs("Asia/Kolkata"), Some(19_800));
}

/// Wave 8 P3 (docs/19 "B-24 package registry"): the package-index surface
/// is re-exported from `kelson-registry` and callable from outside the crate
/// (load/append/save/entry resolution over the root `packages.toml`).
#[test]
fn wave8_package_index_surface() {
    use kelson_registry::{PackageEntry, PackageIndex, PACKAGES_FILE};

    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "kelson-testkit-pkg-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");

    // Absent index loads empty (the normal posture for a fresh project).
    let mut index = PackageIndex::load(&root).expect("absent index is empty");
    assert!(index.package.is_empty());

    // append + save + reload round-trips, and the entry resolves
    // root-relative to its package directory.
    index.append(PackageEntry {
        name: "billing".into(),
        version: "0.1.0".into(),
        path: "packages/billing".into(),
    });
    index.save(&root).expect("index writes");
    assert!(root.join(PACKAGES_FILE).exists());
    let reloaded = PackageIndex::load(&root).expect("index parses");
    assert_eq!(reloaded.package.len(), 1);
    assert_eq!(
        PackageIndex::entry_dir(&root, &reloaded.package[0]),
        root.join("packages/billing")
    );

    std::fs::remove_dir_all(&root).ok();
}
