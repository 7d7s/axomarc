//! MNC-005 (enumeration half) — registration answers ONE answer.
//!
//! The audit's finding: `/auth/register` told an attacker what they asked.
//! A free address landed `200` with a session; a taken one landed
//! `409 "an account with this email already exists"` — and answered in
//! microseconds where the create path paid an Argon2 hash, so the clock
//! named the account even when the body was silent. Three channels of one
//! oracle: status, body, timing.
//!
//! This suite pins the closed contract at the HTTP surface:
//!
//! * one identical `202 Accepted` — same status, same body, no cookie —
//!   whether or not the address is taken, with the old 409 gone;
//! * register mints no session: the session is what `/auth/login` issues;
//! * both sides of the branch pay the same password work (the timing
//!   channel closed by equalization, not by luck);
//! * the verification mail leaves OFF the response path for both
//!   branches, so SMTP latency and failure can never become a second
//!   oracle;
//! * caller-controlled validation (weak password, unknown tenant)
//!   answers identically before and after the address exists.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

struct Fixture {
    state: kelson_demo::routes::AppState,
    router: axum::Router,
}

/// Boot the auth surface (router + session middleware), optionally with a
/// mail port installed BEFORE the router is built — the router holds its
/// own clone of the state.
fn boot_with_mail(port: Option<Arc<dyn kelson_notifications::MailPort>>) -> Fixture {
    let dir = std::env::temp_dir().join(format!("kelson-mnc005-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("fixture root");
    let (mut state, _graph, _store) = kelson_demo::build_app_state(&dir, true, true);
    if let Some(port) = port {
        state.mail = Some(port);
    }
    let router = kelson_demo::routes::router(state.clone())
        .merge(kelson_demo::auth_routes::router(state.clone()));
    let router = state
        .sessions
        .as_ref()
        .expect("sessions enabled")
        .as_ref()
        .clone()
        .apply(router);
    Fixture { state, router }
}

fn boot() -> Fixture {
    boot_with_mail(None)
}

struct Reply {
    status: StatusCode,
    body: Value,
    set_cookies: Vec<String>,
    content_type: String,
}

async fn send(router: &axum::Router, req: Request<Body>) -> Reply {
    let res = router.clone().oneshot(req).await.expect("request serves");
    let status = res.status();
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let set_cookies: Vec<String> = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_string)
        .collect();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Reply {
        status,
        body,
        set_cookies,
        content_type,
    }
}

async fn post_json(router: &axum::Router, uri: &str, body: Value, cookie: Option<&str>) -> Reply {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(
        router,
        builder
            .body(Body::from(body.to_string()))
            .expect("request builds"),
    )
    .await
}

async fn get(router: &axum::Router, uri: &str, cookie: Option<&str>) -> Reply {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(router, builder.body(Body::empty()).expect("request builds")).await
}

fn register_body(email: &str, password: &str, tenant: &str) -> Value {
    json!({ "email": email, "password": password, "tenant": tenant })
}

async fn register(router: &axum::Router, email: &str) -> Reply {
    post_json(
        router,
        "/auth/register",
        register_body(email, "Correct-horse-99", "dev"),
        None,
    )
    .await
}

fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@clinic.test", uuid::Uuid::now_v7())
}

fn session_cookie_of(reply: &Reply) -> Option<String> {
    reply
        .set_cookies
        .iter()
        .find(|c| c.starts_with("kelson_session="))
        .map(|c| c.split(';').next().expect("cookie pair").to_string())
}

// ------------------------------------------------------------- the one answer

#[tokio::test]
async fn register_answers_one_uniform_202_whether_or_not_the_address_is_taken() {
    let fx = boot();
    let email = unique_email("enum");

    // The FREE side: accepted, told nothing but "we got it", and — the
    // whole point — handed NO session (a session would be an answer the
    // address did not ask for, and would differ from the taken side).
    let free = register(&fx.router, &email).await;
    assert_eq!(free.status, StatusCode::ACCEPTED, "{}", free.body);
    assert!(
        free.content_type.starts_with("application/json"),
        "{}",
        free.content_type
    );
    assert_eq!(free.body["ok"], true, "{}", free.body);
    assert_eq!(free.body["data"]["sent"], json!(true), "{}", free.body);
    assert!(
        free.body.get("error").is_none(),
        "success carries no error: {}",
        free.body
    );
    assert!(
        free.set_cookies.is_empty(),
        "MNC-005: register mints no session: {:?}",
        free.set_cookies
    );
    let created = fx
        .state
        .principals
        .by_email(&email)
        .expect("store answers")
        .expect("the free side did create the account");
    assert!(
        created.email_verified_at.is_none(),
        "verification still pending"
    );

    // The TAKEN side: the SAME answer, byte for byte — no 409, no
    // "already exists", no cookie, no field that varies.
    let taken = register(&fx.router, &email).await;
    assert_eq!(taken.status, StatusCode::ACCEPTED, "{}", taken.body);
    assert_eq!(
        free.body, taken.body,
        "the two answers must be indistinguishable"
    );
    assert_eq!(free.status, taken.status, "same status on both sides");
    assert!(
        taken.set_cookies.is_empty(),
        "MNC-005: register mints no session: {:?}",
        taken.set_cookies
    );
    // And the taken side created nothing twice: same principal, one row.
    let again = fx
        .state
        .principals
        .by_email(&email)
        .expect("store answers")
        .expect("the account still exists");
    assert_eq!(
        again.id.to_string(),
        created.id.to_string(),
        "the taken branch must not create a second principal"
    );
}

// ---------------------------------------------------- the session's new home

#[tokio::test]
async fn register_mints_no_session_and_the_session_comes_from_sign_in() {
    let fx = boot();
    let email = unique_email("signin");

    let accepted = register(&fx.router, &email).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.body);
    assert!(accepted.set_cookies.is_empty(), "{:?}", accepted.set_cookies);

    // Registration alone leaves the caller ANONYMOUS — the cookie the old
    // contract issued right here was half the oracle.
    let anon = get(&fx.router, "/auth/me", None).await;
    assert_eq!(
        anon.status,
        StatusCode::UNAUTHORIZED,
        "no session before sign-in: {}",
        anon.body
    );

    // The explicit sign-in is what issues the session — for this
    // (already existing) address, with nothing extra revealed.
    let signed_in = post_json(
        &fx.router,
        "/auth/login",
        json!({ "identifier": email, "password": "Correct-horse-99", "tenant": "dev" }),
        None,
    )
    .await;
    assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.body);
    let cookie = session_cookie_of(&signed_in).expect("sign-in mints the session");

    let me = get(&fx.router, "/auth/me", Some(&cookie)).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.body);
    assert_eq!(
        me.body["data"]["principal"]["email"],
        json!(email),
        "{}",
        me.body
    );
}

// --------------------------------------------------------- the timing channel

#[tokio::test]
async fn both_sides_of_the_branch_pay_the_same_password_work() {
    let fx = boot();

    // Three fresh addresses, each measured through the CREATE side...
    let mut create_times = Vec::new();
    let mut addresses = Vec::new();
    for i in 0..3 {
        let email = unique_email(&format!("timing-create-{i}"));
        let started = Instant::now();
        let reply = register(&fx.router, &email).await;
        create_times.push(started.elapsed());
        assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.body);
        addresses.push(email);
    }

    // ...and the same address again three times through the TAKEN side.
    let mut taken_times = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        let reply = register(&fx.router, &addresses[0]).await;
        taken_times.push(started.elapsed());
        assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.body);
    }

    // The old contract answered the taken side in microseconds against
    // the create side's Argon2 — a ratio near ZERO, which is exactly how
    // an attacker timed the oracle out. Both sides now hash BEFORE the
    // branch, so the fastest observed samples (least scheduler noise)
    // sit together. The 0.25 floor is generous slack for a loaded CI and
    // still fails loudly if the taken side ever skips the work again.
    let fastest_create = create_times.iter().min().expect("three samples");
    let fastest_taken = taken_times.iter().min().expect("three samples");
    assert!(
        *fastest_taken >= *fastest_create / 4,
        "the taken side answered in {fastest_taken:?} against the create side's {fastest_create:?} — the timing oracle is open"
    );
}

// ------------------------------------------------------------- the mail channel

/// A mailer that takes its time: the stand-in for a real SMTP hop.
struct SlowMail {
    delay: Duration,
    delivered: Arc<Mutex<Vec<kelson_notifications::Mailable>>>,
}

impl kelson_notifications::MailPort for SlowMail {
    fn send(
        &self,
        msg: &kelson_notifications::NotifyMessage,
        subject: &str,
    ) -> Result<(), kelson_foundation::AppError> {
        // Blocking on purpose: this runs on the blocking pool, exactly
        // where a real transport would be.
        std::thread::sleep(self.delay);
        self.delivered.lock().unwrap().push(kelson_notifications::Mailable {
            to: vec![msg.to.clone()],
            cc: vec![],
            bcc: vec![],
            from: None,
            reply_to: None,
            subject: subject.to_string(),
            text: Some(msg.template.clone()),
            html: None,
            headers: vec![],
            attachments: vec![],
        });
        Ok(())
    }

    fn send_mailable(
        &self,
        mail: &kelson_notifications::Mailable,
        _attachments: &[kelson_notifications::ResolvedMailAttachment],
    ) -> Result<(), kelson_foundation::AppError> {
        std::thread::sleep(self.delay);
        self.delivered.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

#[tokio::test]
async fn the_verification_mail_leaves_the_response_path_for_both_branches() {
    let delay = Duration::from_millis(2500);
    let port = Arc::new(SlowMail {
        delay,
        delivered: Arc::default(),
    });
    let delivered = port.delivered.clone();
    let fx = boot_with_mail(Some(port));
    let email = unique_email("async-mail");

    // The FREE side: the 202 must be on the wire long before a mailer
    // this slow could have answered — the send is queued, not awaited.
    let started = Instant::now();
    let free = register(&fx.router, &email).await;
    let free_elapsed = started.elapsed();
    assert_eq!(free.status, StatusCode::ACCEPTED, "{}", free.body);
    assert!(
        free_elapsed < Duration::from_millis(1500),
        "the response waited {free_elapsed:?} on a mailer that sleeps {delay:?} — the send is on the response path"
    );

    // The TAKEN side queues the same mail, at the same cost to the
    // response: latency can never differ by whether the address existed.
    let started = Instant::now();
    let taken = register(&fx.router, &email).await;
    let taken_elapsed = started.elapsed();
    assert_eq!(taken.status, StatusCode::ACCEPTED, "{}", taken.body);
    assert!(
        taken_elapsed < Duration::from_millis(1500),
        "the response waited {taken_elapsed:?} on a mailer that sleeps {delay:?} — the send is on the response path"
    );

    // Off the path does not mean never: both queued mails DO land, both
    // addressed to the registered address, both carrying the token.
    let mut landed = Vec::new();
    for _ in 0..400 {
        landed = delivered.lock().unwrap().clone();
        if landed.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(landed.len(), 2, "both branches queue one mail: {landed:?}");
    for mail in &landed {
        assert_eq!(mail.to, vec![email.clone()], "{mail:?}");
        assert_eq!(mail.subject, "Verify your email address", "{mail:?}");
        assert!(
            !mail.text.as_deref().unwrap_or("").is_empty(),
            "the mail carries the verification token: {mail:?}"
        );
    }
}

// -------------------------------------------------------- validation parity

#[tokio::test]
async fn caller_errors_answer_identically_whether_or_not_the_address_is_taken() {
    let fx = boot();

    // (a) A password below the policy. The weak attempt is refused the
    // same way on the free side...
    let weak_email = unique_email("weak-pw");
    let weak_free = post_json(
        &fx.router,
        "/auth/register",
        register_body(&weak_email, "Ab1!", "dev"),
        None,
    )
    .await;
    assert_eq!(
        weak_free.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        weak_free.body
    );
    // ...the account is then created for real...
    let created = register(&fx.router, &weak_email).await;
    assert_eq!(created.status, StatusCode::ACCEPTED, "{}", created.body);
    // ...and the SAME weak attempt is refused IDENTICALLY: validation
    // runs before the branch, so it can never become the oracle.
    let weak_taken = post_json(
        &fx.router,
        "/auth/register",
        register_body(&weak_email, "Ab1!", "dev"),
        None,
    )
    .await;
    assert_eq!(
        weak_taken.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        weak_taken.body
    );
    assert_eq!(
        weak_free.body, weak_taken.body,
        "a weak password answers the same before and after the address exists"
    );

    // (b) A tenant this environment does not have — a caller-controlled
    // config error, hoisted ahead of the branch for the same reason.
    let tenant_email = unique_email("bad-tenant");
    let tenant_free = post_json(
        &fx.router,
        "/auth/register",
        register_body(&tenant_email, "Correct-horse-99", "no-such-tenant"),
        None,
    )
    .await;
    assert_eq!(
        tenant_free.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        tenant_free.body
    );
    let created = register(&fx.router, &tenant_email).await;
    assert_eq!(created.status, StatusCode::ACCEPTED, "{}", created.body);
    let tenant_taken = post_json(
        &fx.router,
        "/auth/register",
        register_body(&tenant_email, "Correct-horse-99", "no-such-tenant"),
        None,
    )
    .await;
    assert_eq!(
        tenant_taken.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        tenant_taken.body
    );
    assert_eq!(
        tenant_free.body, tenant_taken.body,
        "tenant resolution answers the same before and after the address exists"
    );
}
