//! HTTP harness: serves an axum router in-process through tower oneshot.
//! No network, no runtime binary — the router is built by the caller from
//! platform primitives.

use std::sync::{Arc, Mutex};

use axum::body::{to_bytes, Body, Bytes};
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use kelson_foundation::{AppError, Clock, FixedClock, PrincipalId, TenantId};
use kelson_http::{parse_session_cookie, CookieJar, SessionMiddleware, SESSION_COOKIE};
use kelson_jobs::{build_from_def, JobDef};
use kelson_models::{FieldFilter, ListOpts, ModelDef, ModelStore, Record};
use kelson_scheduling::{ScheduleItem, ScheduleRegistry, SchedulerRunner};
use kelson_session::Session;
use kelson_views::{ViewRender, ViewTap};
use serde::de::DeserializeOwned;
use serde::Serialize;
use time::OffsetDateTime;
use tower::ServiceExt;

/// Actor header applied by TestApp::acting_as. Module test wiring maps it
/// into whichever identity mechanism it uses (real sessions land in S3).
pub const ACTOR_HEADER: &str = "x-kelson-actor";
/// Tenant header applied by TestApp::tenant.
pub const TENANT_HEADER: &str = "x-kelson-tenant";

/// In-process HTTP harness around one axum router.
pub struct TestApp {
    router: Router,
    actor: Option<String>,
    tenant: Option<String>,
    default_headers: Vec<(String, String)>,
    /// Wave 7 P2: session resolution probe (the middleware wired into the
    /// router, handed over for response/request cookie -> Session lookup).
    session: Option<SessionMiddleware>,
    /// Wave 7 P2: view-render recorder (the handle wired into
    /// `ViewsEngine::set_tap`).
    views: Option<Arc<ViewRecorder>>,
    /// Wave 7 P2: ModelStore the database asserts probe.
    models: Option<Arc<dyn ModelStore>>,
    /// Wave 7 P3: fake queue the fake scheduler fires due items into.
    queue: Option<Arc<crate::fakes::QueueFake>>,
    /// Wave 7 P3: the schedule registry this TestApp owns (fresh per app).
    schedules: Option<Arc<ScheduleRegistry>>,
    /// Wave 7 P3: last-fired state carried across `run_due_schedules`
    /// calls (the runner is rebuilt per tick; the state is not).
    schedule_state: Vec<(String, OffsetDateTime)>,
    /// Wave 7 P3: the clock `run_due_schedules` reads (freeze_time handle).
    clock: Option<Arc<FixedClock>>,
}

impl TestApp {
    pub fn new(router: Router) -> Self {
        Self {
            router,
            actor: None,
            tenant: None,
            default_headers: Vec::new(),
            session: None,
            views: None,
            models: None,
            queue: None,
            schedules: None,
            schedule_state: Vec::new(),
            clock: None,
        }
    }

    /// Every subsequent request acts as this principal.
    pub fn acting_as(mut self, principal: impl Into<String>) -> Self {
        self.actor = Some(principal.into());
        self
    }

    /// Every subsequent request runs inside this tenant scope.
    pub fn tenant(mut self, tenant: impl Into<String>) -> Self {
        self.tenant = Some(tenant.into());
        self
    }

    /// Extra default header applied to every request.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.default_headers.push((name.into(), value.into()));
        self
    }

    /// Wave 7 P2: resolve the session for every response through this
    /// middleware — hand over the SAME `SessionMiddleware` wired into the
    /// router (`mw.clone().apply(router)`). Resolution: a `kelson_session`
    /// Set-Cookie on the RESPONSE is authoritative (login issues, logout
    /// clears), otherwise the request's own cookie carries the session.
    pub fn with_session(mut self, mw: SessionMiddleware) -> Self {
        self.session = Some(mw);
        self
    }

    /// Wave 7 P2: record view renders through this recorder — wire the same
    /// handle into `ViewsEngine::set_tap`. `send()` snapshots only the
    /// renders this request produced (an earlier response's view can never
    /// leak into a later `assert_view_has`).
    pub fn with_views(mut self, recorder: Arc<ViewRecorder>) -> Self {
        self.views = Some(recorder);
        self
    }

    /// Wave 7 P2: the ModelStore the database asserts probe; rows are read
    /// under this TestApp's tenant (the model's `tenant_scoped` posture
    /// decides whether the scope narrows).
    pub fn with_models(mut self, store: Arc<dyn ModelStore>) -> Self {
        self.models = Some(store);
        self
    }

    /// Wave 7 P3 (docs/19 L342): fake queue — `run_due_schedules` fires
    /// every due schedule item into it as a `max_attempts = 1` durable job;
    /// assert through `queue()`.
    pub fn with_fake_queue(mut self) -> Self {
        self.queue = Some(Arc::new(crate::fakes::QueueFake::new()));
        self
    }

    /// Wave 7 P3 (docs/19 L342): fake scheduler — a fresh `ScheduleRegistry`
    /// this TestApp owns. Register items with `schedule(...)`, execute due
    /// items with `run_due_schedules()`.
    pub fn with_fake_scheduler(mut self) -> Self {
        self.schedules = Some(Arc::new(ScheduleRegistry::new()));
        self
    }

    /// Wave 7 P3 (docs/19 L340): the clock `run_due_schedules` reads —
    /// wire the `freeze_time` handle, then `travel` between ticks.
    pub fn with_clock(mut self, clock: Arc<FixedClock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// The fake queue (wiring hint when unwired).
    pub fn queue(&self) -> &Arc<crate::fakes::QueueFake> {
        self.queue.as_ref().unwrap_or_else(|| {
            panic!("no queue fake wired (call TestApp::with_fake_queue)")
        })
    }

    /// Wave 7 P3: register a schedule item into the fake scheduler (before
    /// the first `run_due_schedules` tick).
    pub fn schedule(&mut self, item: ScheduleItem) -> Result<(), AppError> {
        let registry = self.schedules.as_mut().unwrap_or_else(|| {
            panic!("no scheduler wired (call TestApp::with_fake_scheduler)")
        });
        let registry = Arc::get_mut(registry)
            .expect("schedule registry is uniquely owned before the first tick");
        registry.register(item)
    }

    /// Wave 7 P3 (docs/19 L342): execute due items synchronously — the tick
    /// instant comes from the injected clock (else wall time), each fired
    /// item enqueues a `max_attempts = 1` durable job into the fake queue,
    /// and last-fired state carries across calls (an item never refires
    /// before its window, exactly like the worker tick).
    pub fn run_due_schedules(&mut self) -> Result<Vec<String>, AppError> {
        let registry = self
            .schedules
            .as_ref()
            .unwrap_or_else(|| panic!("no scheduler wired (call TestApp::with_fake_scheduler)"))
            .clone();
        let queue = self
            .queue
            .as_ref()
            .unwrap_or_else(|| panic!("no queue fake wired (call TestApp::with_fake_queue)"))
            .clone();
        let now = self
            .clock
            .as_ref()
            .map(|clock| clock.now())
            .unwrap_or_else(OffsetDateTime::now_utc);
        let runner = SchedulerRunner::with_state(registry, std::mem::take(&mut self.schedule_state));
        let fired = runner.tick(now, |item| {
            let mut def = JobDef::new(item.operation.clone());
            def.max_attempts = 1;
            let job = build_from_def(&def, item.payload.clone(), now);
            queue.enqueue(job);
            Ok(())
        })?;
        self.schedule_state = runner.state();
        Ok(fired)
    }

    pub fn router(&self) -> &Router {
        &self.router
    }

    pub fn get(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::GET, path)
    }

    pub fn post(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::POST, path)
    }

    pub fn put(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::PUT, path)
    }

    pub fn patch(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::PATCH, path)
    }

    pub fn delete(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::DELETE, path)
    }

    pub fn request(&self, method: Method, path: &str) -> TestRequest<'_> {
        TestRequest {
            app: self,
            method,
            path: path.to_string(),
            headers: HeaderMap::new(),
            body: Vec::new(),
            content_type: None,
        }
    }
}

/// One request under construction.
pub struct TestRequest<'a> {
    app: &'a TestApp,
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
    content_type: Option<&'static str>,
}

impl<'a> TestRequest<'a> {
    pub fn header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        let name = axum::http::HeaderName::try_from(name.as_ref()).expect("valid header name");
        let value = axum::http::HeaderValue::try_from(value.as_ref()).expect("valid header value");
        self.headers.insert(name, value);
        self
    }

    pub fn json<T: Serialize>(mut self, value: &T) -> Self {
        self.body = serde_json::to_vec(value).expect("json serializes");
        self.content_type = Some("application/json");
        self
    }

    pub fn body(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.body = bytes.into();
        self
    }

    /// Serve the request through the router (no network).
    pub async fn send(self) -> TestResponse {
        // Recorder mark: only renders AFTER this point belong to this
        // response (a previous response's view must never leak in).
        let view_mark = self.app.views.as_ref().map(|r| r.mark());
        let mut builder = Request::builder()
            .method(self.method.clone())
            .uri(&self.path);
        if let Some(actor) = &self.app.actor {
            builder = builder.header(ACTOR_HEADER, actor.as_str());
        }
        if let Some(tenant) = &self.app.tenant {
            builder = builder.header(TENANT_HEADER, tenant.as_str());
        }
        for (name, value) in &self.app.default_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(content_type) = self.content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type);
        }
        let mut request = builder.body(Body::from(self.body)).expect("request builds");
        let headers = request.headers_mut();
        for (name, value) in self.headers.iter() {
            headers.insert(name.clone(), value.clone());
        }
        // The session fallback reads the REQUEST cookie after the oneshot
        // consumes the builder, so keep a copy of the wire headers.
        let request_headers = request.headers().clone();
        let response = self
            .app
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("router serves the request");
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body collects");
        let session = self
            .app
            .session
            .as_ref()
            .and_then(|mw| resolve_session(mw, &request_headers, &headers));
        let view = view_mark.and_then(|mark| self.app.views.as_ref()?.since(mark));
        let db = self.app.models.as_ref().map(|store| DbProbe {
            store: store.clone(),
            tenant: self.app.tenant.as_deref().map(|raw| {
                TenantId::from_uuid(
                    uuid::Uuid::parse_str(raw)
                        .unwrap_or_else(|e| panic!("TestApp tenant {raw:?} is not a uuid: {e}")),
                )
            }),
        });
        TestResponse {
            status,
            headers,
            body,
            session,
            view,
            db,
        }
    }
}

/// One served response with assertion ergonomics.
pub struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
    /// Session resolved at send (None = guest).
    session: Option<Session>,
    /// Context of the LAST view this response rendered (None = no view).
    view: Option<ViewRender>,
    /// ModelStore probe + test tenant for the database asserts.
    db: Option<DbProbe>,
}

impl TestResponse {
    pub fn status(&self) -> u16 {
        self.status.as_u16()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn body_bytes(&self) -> &[u8] {
        &self.body
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json_value(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!("response body is not JSON: {e}; body: {}", self.body_text())
        })
    }

    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "response body does not deserialize: {e}; body: {}",
                self.body_text()
            )
        })
    }

    pub fn assert_status(self, expected: u16) -> Self {
        assert_eq!(
            self.status.as_u16(),
            expected,
            "unexpected status; body: {}",
            self.body_text()
        );
        self
    }

    pub fn assert_header(self, name: &str, expected: &str) -> Self {
        let actual = self
            .header(name)
            .unwrap_or_else(|| panic!("header {name} missing; body: {}", self.body_text()));
        assert_eq!(actual, expected, "header {name} mismatch");
        self
    }

    /// Exact JSON equality.
    pub fn assert_json_eq(self, expected: serde_json::Value) -> Self {
        let actual = self.json_value();
        assert_eq!(actual, expected, "json mismatch");
        self
    }

    /// Subset match: every key/value in expected must appear in the actual
    /// payload (objects checked recursively, arrays positionally).
    pub fn assert_json_contains(self, expected: serde_json::Value) -> Self {
        let actual = self.json_value();
        assert!(
            json_contains(&actual, &expected),
            "json subset mismatch; actual: {actual}, expected subset: {expected}"
        );
        self
    }

    // ------------------------------------------- Wave 7 P2: web flow asserts

    /// The response's `Location` header matches exactly or as a prefix
    /// (absolute-URL redirects: expected path, actual `https://host/path`).
    pub fn assert_redirect(self, expected: &str) -> Self {
        let actual = self.header("location").unwrap_or_else(|| {
            panic!(
                "no Location header (status {}); body: {}",
                self.status(),
                self.body_text()
            )
        });
        assert!(
            actual == expected || actual.starts_with(expected),
            "redirect mismatch: expected {expected:?}, got {actual:?}"
        );
        self
    }

    /// The response's session carries `key` (dot-paths walk nested objects).
    /// Requires `TestApp::with_session` plus an issued session cookie.
    pub fn assert_session_has(self, key: &str) -> Self {
        let session = self.session.as_ref().unwrap_or_else(|| {
            panic!(
                "no session resolved (wire TestApp::with_session and issue a cookie); body: {}",
                self.body_text()
            )
        });
        let bag = session_bag(session);
        assert!(
            json_path(&bag, key).is_some(),
            "session key {key:?} missing; keys: {:?}",
            session.data.keys().collect::<Vec<_>>()
        );
        self
    }

    /// The response's session does NOT carry `key`. No resolved session
    /// satisfies this (a guest response carries no keys).
    pub fn assert_session_missing(self, key: &str) -> Self {
        if let Some(session) = &self.session {
            let bag = session_bag(session);
            assert!(
                json_path(&bag, key).is_none(),
                "session key {key:?} unexpectedly present"
            );
        }
        self
    }

    /// A `Set-Cookie` named `name` exists; with `Some(value)` the value must
    /// match exactly (presence-only when `None`).
    pub fn assert_cookie(self, name: &str, value: Option<&str>) -> Self {
        let actual = self.response_cookie(name).unwrap_or_else(|| {
            panic!(
                "cookie {name} missing; set-cookies: {:?}",
                self.set_cookie_values()
            )
        });
        if let Some(expected) = value {
            assert_eq!(actual, expected, "cookie {name} value mismatch");
        }
        self
    }

    /// The last view this response rendered received `key` in its context
    /// (dot-paths walk nested objects; context = post-hook, what the
    /// template actually got).
    pub fn assert_view_has(self, key: &str) -> Self {
        let view = self.view.as_ref().unwrap_or_else(|| {
            panic!(
                "no view rendered for this response; body: {}",
                self.body_text()
            )
        });
        assert!(
            json_path(&view.context, key).is_some(),
            "view {:?} context lacks key {key:?}",
            view.view
        );
        self
    }

    /// The last view this response rendered received a **view model**
    /// whose `key` is present.
    ///
    /// The model lives in the `vm` namespace of the view's context — the
    /// namespace the binder (`@vm`) binds — so the assertion speaks the
    /// view's language rather than the context's raw keys. The rendered
    /// HTML is never consulted: the model is what the view was given.
    pub fn assert_view_model_has(self, key: &str) -> Self {
        let view = self.view.as_ref().unwrap_or_else(|| {
            panic!(
                "no view rendered for this response; body: {}",
                self.body_text()
            )
        });
        let model = view
            .context
            .get(kelson_views::VM_NAMESPACE)
            .unwrap_or_else(|| {
                panic!(
                    "the view {:?} received no view model; context: {}",
                    view.view, view.context
                )
            });
        assert!(
            json_path(model, key).is_some(),
            "the view model of {:?} lacks key {key:?}; model: {model}",
            view.view
        );
        self
    }

    /// The last view this response rendered received a view model of the
    /// model's **own type**.
    ///
    /// The assertion is typed twice over: the received model must
    /// deserialise into `M`, and `key` must be a field `M` declares — so
    /// a field the view model does not declare cannot be asserted at all,
    /// and a view that received the wrong shape fails here rather than
    /// quietly in a template.
    pub fn assert_view_model<M: kelson_views::ViewModel + DeserializeOwned>(self, key: &str) -> Self {
        let view = self.view.as_ref().unwrap_or_else(|| {
            panic!(
                "no view rendered for this response; body: {}",
                self.body_text()
            )
        });
        let model = view
            .context
            .get(kelson_views::VM_NAMESPACE)
            .unwrap_or_else(|| {
                panic!(
                    "the view {:?} received no view model; context: {}",
                    view.view, view.context
                )
            });
        serde_json::from_value::<M>(model.clone()).unwrap_or_else(|e| {
            panic!(
                "the view model of {:?} is not a {}: {e}",
                view.view,
                std::any::type_name::<M>()
            )
        });
        assert!(
            M::schema().iter().any(|field| field.path == key),
            "the view model {} does not declare {key:?}",
            std::any::type_name::<M>()
        );
        assert!(
            json_path(model, key).is_some(),
            "the view model of {:?} lacks key {key:?}; model: {model}",
            view.view
        );
        self
    }

    /// The view this response rendered **lints clean**: every `vm.*` path
    /// its template reads is declared by the view's registered view
    /// model.
    ///
    /// The lint is the binder's build-time half: the render-time check
    /// refuses an undeclared path when the page is served, the lint
    /// refuses it in the test — before the page is ever served.
    pub fn assert_view_lints_clean(self, engine: &kelson_views::ViewsEngine) -> Self {
        let view = self.view.as_ref().unwrap_or_else(|| {
            panic!(
                "no view rendered for this response; body: {}",
                self.body_text()
            )
        });
        engine.lint(&view.view).unwrap_or_else(|e| {
            panic!(
                "the view {:?} does not lint clean: {e}",
                view.view
            )
        });
        self
    }

    // ------------------------------------------ Wave 7 P2: content asserts

    /// Walk a dotted path through the JSON body (objects; numeric segments
    /// index arrays) and compare the value found there.
    pub fn assert_json_path(self, path: &str, expected: serde_json::Value) -> Self {
        let actual = self.json_value();
        let found = json_path(&actual, path)
            .unwrap_or_else(|| panic!("no JSON path {path:?} in body: {actual}"));
        assert_eq!(found, &expected, "json path {path:?} mismatch");
        self
    }

    /// The body contains `fragment` verbatim (raw HTML substring).
    pub fn assert_html_contains(self, fragment: &str) -> Self {
        let body = self.body_text();
        assert!(
            body.contains(fragment),
            "HTML fragment {fragment:?} not in body: {body}"
        );
        self
    }

    /// The body contains the HTML-ESCAPED rendering of `text` — Blade/Laravel
    /// `assertSee` parity: view output escapes, so the expectation escapes
    /// too. Pair with [`assert_html_contains`] for raw fragments.
    pub fn assert_see(self, text: &str) -> Self {
        let expected = kelson_views::html_escape(text);
        let body = self.body_text();
        assert!(
            body.contains(&expected),
            "expected to see {expected:?} (escaped) in body: {body}"
        );
        self
    }

    // ---------------------------------------- Wave 7 P2: database asserts

    /// At least one LIVE row matches every `predicates` pair (AND) in the
    /// ModelStore probe under the TestApp's tenant.
    pub fn assert_database_has(
        self,
        model: &ModelDef,
        predicates: &[(&str, serde_json::Value)],
    ) -> Self {
        let db = self.db_probe(model);
        let rows = db_rows(db, model, predicates);
        assert!(
            !rows.is_empty(),
            "expected row in {} matching {predicates:?} under tenant {:?}",
            model.table,
            db.tenant
        );
        self
    }

    /// No LIVE row matches every `predicates` pair under the TestApp's
    /// tenant (soft-deleted rows never count as present).
    pub fn assert_database_missing(
        self,
        model: &ModelDef,
        predicates: &[(&str, serde_json::Value)],
    ) -> Self {
        let db = self.db_probe(model);
        let rows = db_rows(db, model, predicates);
        assert!(
            rows.is_empty(),
            "expected no row in {} matching {predicates:?} under tenant {:?}, found {}",
            model.table,
            db.tenant,
            rows.len()
        );
        self
    }

    /// Exactly `expected` LIVE rows for the model under the TestApp's
    /// tenant (no predicates = every row in scope).
    pub fn assert_database_count(self, model: &ModelDef, expected: usize) -> Self {
        let db = self.db_probe(model);
        let count = db_rows(db, model, &[]).len();
        assert_eq!(count, expected, "database count mismatch for {}", model.table);
        self
    }

    // ------------------------------------------- Wave 7 P2: auth asserts

    /// A session resolved for this response and it belongs to `principal`.
    pub fn assert_authenticated_as(self, principal: PrincipalId) -> Self {
        let session = self.session.as_ref().unwrap_or_else(|| {
            panic!(
                "expected {principal}, got a guest response; body: {}",
                self.body_text()
            )
        });
        assert_eq!(
            session.principal, principal,
            "authenticated as a different principal"
        );
        self
    }

    /// No session resolved for this response (anonymous/guest posture).
    pub fn assert_guest(self) -> Self {
        if let Some(session) = &self.session {
            panic!(
                "expected guest, but a session for {} is authenticated",
                session.principal
            );
        }
        self
    }

    // ------------------------------------------------ private probe access

    /// The wired model store (panics with the wiring hint when absent).
    fn db_probe(&self, model: &ModelDef) -> &DbProbe {
        self.db.as_ref().unwrap_or_else(|| {
            panic!(
                "no model store wired (call TestApp::with_models) to assert on {}",
                model.table
            )
        })
    }

    /// Value of the first `Set-Cookie` naming `name` (before the first `;`).
    fn response_cookie(&self, name: &str) -> Option<String> {
        cookie_value(&self.headers, name)
    }

    /// Every raw `Set-Cookie` header value (failure context for asserts).
    fn set_cookie_values(&self) -> Vec<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect()
    }
}

fn json_contains(actual: &serde_json::Value, expected: &serde_json::Value) -> bool {
    match expected {
        serde_json::Value::Object(map) => match actual {
            serde_json::Value::Object(actual_map) => map.iter().all(|(key, value)| {
                actual_map
                    .get(key)
                    .map(|actual_value| json_contains(actual_value, value))
                    .unwrap_or(false)
            }),
            _ => false,
        },
        serde_json::Value::Array(items) => match actual {
            serde_json::Value::Array(actual_items) => {
                items.len() == actual_items.len()
                    && items
                        .iter()
                        .zip(actual_items)
                        .all(|(expected_item, actual_item)| {
                            json_contains(actual_item, expected_item)
                        })
            }
            _ => false,
        },
        other => actual == other,
    }
}

// ------------------------------------------- Wave 7 P2: probe plumbing

/// Value of the first `Set-Cookie` naming `name` (name/value before the
/// first `;`; a logout clear yields the empty string).
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    for raw in headers.get_all(header::SET_COOKIE) {
        let Ok(raw) = raw.to_str() else {
            continue;
        };
        let first = raw.split(';').next().unwrap_or_default();
        if let Some((cookie_name, value)) = first.split_once('=') {
            if cookie_name.trim() == name {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// Load + verify a session from a raw cookie value (tag check, store load,
/// policy expiry — the middleware's posture, read-only). Wave 7 P3: expiry
/// reads the middleware's injected clock, mirroring the touch gate.
fn load_session(mw: &SessionMiddleware, cookie: &str) -> Option<Session> {
    let id = parse_session_cookie(cookie, &mw.secret)?;
    let session = mw.manager.store().load(&id).ok().flatten()?;
    if session.expired(mw.now(), mw.manager.policy()) {
        None
    } else {
        Some(session)
    }
}

/// Resolve the response's session: a `kelson_session` Set-Cookie on the
/// RESPONSE is authoritative (login issues, logout clears); otherwise the
/// request's own cookie carries an existing session forward.
fn resolve_session(mw: &SessionMiddleware, request: &HeaderMap, response: &HeaderMap) -> Option<Session> {
    if let Some(cookie) = cookie_value(response, SESSION_COOKIE) {
        return load_session(mw, &cookie);
    }
    let jar = CookieJar::from_headers(request);
    let cookie = jar.get(SESSION_COOKIE)?;
    load_session(mw, cookie)
}

/// Session data as a JSON object (for the dotted-path walk).
fn session_bag(session: &Session) -> serde_json::Value {
    serde_json::Value::Object(
        session
            .data
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// Walk a dotted path through a JSON value (objects; numeric segments
/// index arrays; an empty path is the value itself).
fn json_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    if path.is_empty() {
        return Some(value);
    }
    let mut current = value;
    for segment in path.split('.') {
        current = match current {
            serde_json::Value::Object(map) => map.get(segment)?,
            serde_json::Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// Query the probe's store under its tenant: every predicate becomes an
/// equality filter (AND), unbounded list, LIVE rows only (soft deletes
/// excluded by the store).
fn db_rows(db: &DbProbe, model: &ModelDef, predicates: &[(&str, serde_json::Value)]) -> Vec<Record> {
    let filters: Vec<FieldFilter> = predicates
        .iter()
        .map(|(field, value)| FieldFilter::eq(*field, value.clone()))
        .collect();
    let page = db
        .store
        .list(model, db.tenant, &filters, &ListOpts::default())
        .unwrap_or_else(|e| panic!("database probe failed on {}: {e}", model.table));
    page.items
}

/// Wave 7 P2: records every successful top-level view render so
/// `TestResponse::assert_view_has` can read the context a response's view
/// received. Wire it twice: `ViewsEngine::set_tap(recorder.clone())` in the
/// router, `TestApp::with_views(recorder)` on the harness.
#[derive(Default)]
pub struct ViewRecorder {
    renders: Mutex<Vec<ViewRender>>,
}

impl ViewRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every render recorded so far (name + context).
    pub fn renders(&self) -> Vec<ViewRender> {
        self.renders.lock().unwrap().clone()
    }

    /// Snapshot point taken by `send()` before the request runs.
    fn mark(&self) -> usize {
        self.renders.lock().unwrap().len()
    }

    /// The LAST render since the mark (None = this response rendered no
    /// view — an earlier response's render can never satisfy the assert).
    fn since(&self, mark: usize) -> Option<ViewRender> {
        self.renders
            .lock()
            .unwrap()
            .get(mark..)
            .and_then(|slice| slice.last().cloned())
    }
}

impl ViewTap for ViewRecorder {
    fn on_render(&self, render: &ViewRender) {
        self.renders.lock().unwrap().push(render.clone());
    }
}

/// Wave 7 P2: the ModelStore probe + test tenant the database asserts run
/// through (assembled by `TestApp::with_models` at send time).
struct DbProbe {
    store: Arc<dyn ModelStore>,
    tenant: Option<TenantId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Extension;
    use axum::routing::{get, post};
    use kelson_http::ResolvedSession;
    use kelson_models::InMemoryModelStore;
    use kelson_session::{InMemorySessionStore, SessionManager, SessionPolicy};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn patient_model() -> ModelDef {
        ModelDef::new("Patient", "patients")
            .tenant_scoped()
            .hard_deletes()
    }

    fn row(tenant: TenantId, email: &str) -> Record {
        let mut row = Record::new();
        row.insert("id".into(), json!(uuid::Uuid::now_v7().to_string()));
        row.insert("tenant_id".into(), json!(tenant.to_string()));
        row.insert("email".into(), json!(email));
        row.insert("status".into(), json!("active"));
        row
    }

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    /// Temp workspace holding one template (mirrors the views-engine tests).
    fn view_engine() -> Arc<kelson_views::ViewsEngine> {
        let dir = std::env::temp_dir().join(format!(
            "kelson-testkit-views-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let views = dir.join("resources").join("views");
        std::fs::create_dir_all(&views).unwrap();
        std::fs::write(
            views.join("page.html"),
            "<h1>{{ title }}</h1><p>{{ user.name }}</p>",
        )
        .unwrap();
        Arc::new(kelson_views::ViewsEngine::new(kelson_views::ViewsConfig {
            paths: vec![views],
            cache_dir: Some(dir.join("storage").join("framework").join("views")),
            env: "testing".into(),
        }))
    }

    fn harness() -> TestApp {
        harness_with_store(InMemoryModelStore::new())
    }

    /// One router exercising every probe: session middleware applied,
    /// view recorder tapped, model store shared with the /db writer route.
    fn harness_with_store(store: Arc<InMemoryModelStore>) -> TestApp {
        let recorder = Arc::new(ViewRecorder::new());
        let engine = view_engine();
        engine.set_tap(recorder.clone());

        let manager = Arc::new(SessionManager::new(
            InMemorySessionStore::shared(),
            SessionPolicy::default(),
            b"unit-secret",
        ));
        let mw = SessionMiddleware::new(manager.clone(), b"cookie-secret");

        let login_manager = manager.clone();
        let login_mw = mw.clone();
        let logout_manager = manager.clone();
        let logout_mw = mw.clone();
        let db_store = store.clone();
        let view_engine = engine.clone();

        let router = Router::new()
            .route("/see", get(|| async { "<p>Tom &amp; Jerry</p>" }))
            .route(
                "/redirect",
                get(|| async { ([(header::LOCATION, "/dashboard")], StatusCode::SEE_OTHER) }),
            )
            .route(
                "/cookie",
                get(|| async {
                    [(
                        header::SET_COOKIE,
                        axum::http::HeaderValue::from_static("flavor=oatmeal; Path=/"),
                    )]
                }),
            )
            .route(
                "/json",
                get(|| async {
                    axum::Json(json!({ "a": { "b": 42 }, "items": [10, 20] }))
                }),
            )
            .route(
                "/view",
                get(move || {
                    let engine = view_engine.clone();
                    async move {
                        engine
                            .render("page", json!({ "title": "Hi", "user": { "name": "ada" } }))
                            .expect("template renders")
                    }
                }),
            )
            // Login: principal from the harness actor header, one bag key,
            // Set-Cookie issued through the same middleware the harness
            // resolves against.
            .route(
                "/login",
                post(move |headers: HeaderMap| {
                    let manager = login_manager.clone();
                    let mw = login_mw.clone();
                    async move {
                        let actor = headers
                            .get(ACTOR_HEADER)
                            .expect("actor header")
                            .to_str()
                            .expect("utf8 actor");
                        let principal =
                            PrincipalId::from_uuid(uuid::Uuid::parse_str(actor).expect("uuid"));
                        let mut session = manager
                            .start(principal, None, mw.now())
                            .unwrap();
                        session.data.insert("cart".into(), json!("7"));
                        manager.store().save(&session).unwrap();
                        let cookie = mw.issue_cookie(&session).unwrap();
                        ([(header::SET_COOKIE, cookie)], "ok")
                    }
                }),
            )
            .route(
                "/me",
                get(|Extension(rs): Extension<ResolvedSession>| async move {
                    rs.session.principal.to_string()
                }),
            )
            .route(
                "/logout",
                post(move |headers: HeaderMap| {
                    let manager = logout_manager.clone();
                    let mw = logout_mw.clone();
                    async move {
                        if let Some(raw) = CookieJar::from_headers(&headers).get(SESSION_COOKIE) {
                            if let Some(id) = parse_session_cookie(raw, &mw.secret) {
                                manager.destroy(&id).unwrap();
                            }
                        }
                        let clear = mw.clear_cookie().unwrap();
                        ([(header::SET_COOKIE, clear)], "logged out")
                    }
                }),
            )
            .route(
                "/db",
                post(move |headers: HeaderMap| {
                    let store = db_store.clone();
                    async move {
                        let tenant_raw = headers
                            .get(TENANT_HEADER)
                            .expect("tenant header")
                            .to_str()
                            .expect("utf8 tenant");
                        let tenant = TenantId::from_uuid(
                            uuid::Uuid::parse_str(tenant_raw).expect("uuid tenant"),
                        );
                        store
                            .insert(&patient_model(), &row(tenant, "ada@clinic.test"))
                            .unwrap();
                        "ok"
                    }
                }),
            );

        TestApp::new(mw.clone().apply(router))
            .with_session(mw)
            .with_views(recorder)
            .with_models(store)
    }

    // ------------------------------------------------- Wave 7 P2: redirects

    #[tokio::test]
    async fn redirect_asserts_exact_and_prefix() {
        let app = harness();
        app.get("/redirect").send().await.assert_redirect("/dashboard").assert_redirect("/dash");
    }

    #[tokio::test]
    #[should_panic(expected = "redirect mismatch")]
    async fn redirect_assert_panics_on_other_location() {
        let app = harness();
        app.get("/redirect").send().await.assert_redirect("/elsewhere");
    }

    // ---------------------------------------------------- Wave 7 P2: cookies

    #[tokio::test]
    async fn cookie_asserts_presence_and_value() {
        let app = harness();
        app.get("/cookie").send().await
            .assert_cookie("flavor", None)
            .assert_cookie("flavor", Some("oatmeal"));
    }

    #[tokio::test]
    #[should_panic(expected = "cookie flavor missing")]
    async fn cookie_assert_panics_when_absent() {
        let app = harness();
        app.get("/json").send().await.assert_cookie("flavor", None);
    }

    #[tokio::test]
    #[should_panic(expected = "cookie flavor value mismatch")]
    async fn cookie_assert_panics_on_value_mismatch() {
        let app = harness();
        app.get("/cookie").send().await.assert_cookie("flavor", Some("waffle"));
    }

    // ----------------------------------------------- Wave 7 P2: json content

    #[tokio::test]
    async fn json_path_walks_objects_and_arrays() {
        let app = harness();
        app.get("/json").send().await
            .assert_json_path("a.b", json!(42))
            .assert_json_path("items.1", json!(20));
    }

    #[tokio::test]
    #[should_panic(expected = "no JSON path")]
    async fn json_path_panics_when_absent() {
        let app = harness();
        app.get("/json").send().await.assert_json_path("a.ghost", json!(42));
    }

    #[tokio::test]
    #[should_panic(expected = "json path \"a.b\" mismatch")]
    async fn json_path_panics_on_mismatch() {
        let app = harness();
        app.get("/json").send().await.assert_json_path("a.b", json!(43));
    }

    // ---------------------------------------------- Wave 7 P2: html content

    #[tokio::test]
    async fn html_and_see_asserts_escape() {
        let app = harness();
        app.get("/see").send().await
            .assert_html_contains("Tom &amp; Jerry")
            .assert_see("Tom & Jerry")
            .assert_see("Jerry");
    }

    #[tokio::test]
    #[should_panic(expected = "not in body")]
    async fn html_contains_panics_when_absent() {
        let app = harness();
        app.get("/see").send().await.assert_html_contains("<b>nope</b>");
    }

    #[tokio::test]
    #[should_panic(expected = "expected to see")]
    async fn see_panics_on_text_that_never_appears() {
        let app = harness();
        app.get("/see").send().await.assert_see("Bob & Co");
    }

    // ------------------------------------------------ Wave 7 P2: session/auth

    #[tokio::test]
    async fn session_login_issues_cookie_and_authenticates() {
        let principal = PrincipalId::new();
        let app = harness().acting_as(principal.to_string());
        app.post("/login").send().await
            .assert_status(200)
            .assert_cookie("kelson_session", None)
            .assert_session_has("cart")
            .assert_authenticated_as(principal);
    }

    #[tokio::test]
    async fn session_carries_forward_from_the_request_cookie() {
        let principal = PrincipalId::new();
        let app = harness().acting_as(principal.to_string());
        let login = app.post("/login").send().await;
        let issued = login
            .header("set-cookie")
            .expect("session cookie issued")
            .split(';')
            .next()
            .unwrap()
            .to_string();
        app.get("/me").header("cookie", issued).send().await
            .assert_authenticated_as(principal)
            .assert_session_has("cart");
    }

    #[tokio::test]
    async fn logout_clear_cookie_reads_as_guest() {
        let principal = PrincipalId::new();
        let app = harness().acting_as(principal.to_string());
        let login = app.post("/login").send().await;
        let issued = login
            .header("set-cookie")
            .expect("session cookie issued")
            .split(';')
            .next()
            .unwrap()
            .to_string();
        app.post("/logout").header("cookie", issued).send().await
            .assert_guest()
            .assert_session_missing("cart");
    }

    #[tokio::test]
    #[should_panic(expected = "no session resolved")]
    async fn session_has_panics_without_a_session() {
        let app = harness();
        app.get("/json").send().await.assert_session_has("cart");
    }

    #[tokio::test]
    async fn session_missing_passes_for_guests() {
        let app = harness();
        app.get("/json").send().await.assert_session_missing("cart");
    }

    #[tokio::test]
    #[should_panic(expected = "session key \"cart\" unexpectedly present")]
    async fn session_missing_panics_when_the_key_is_there() {
        let principal = PrincipalId::new();
        let app = harness().acting_as(principal.to_string());
        app.post("/login").send().await.assert_session_missing("cart");
    }

    #[tokio::test]
    async fn guest_assert_passes_anonymously() {
        let app = harness();
        app.get("/json").send().await
            .assert_guest()
            .assert_session_missing("cart");
    }

    #[tokio::test]
    #[should_panic(expected = "expected guest")]
    async fn guest_assert_panics_when_authenticated() {
        let principal = PrincipalId::new();
        let app = harness().acting_as(principal.to_string());
        app.post("/login").send().await.assert_guest();
    }

    #[tokio::test]
    #[should_panic(expected = "got a guest response")]
    async fn authenticated_as_panics_for_a_guest() {
        let app = harness();
        app.get("/json")
            .send()
            .await
            .assert_authenticated_as(PrincipalId::new());
    }

    #[tokio::test]
    #[should_panic(expected = "different principal")]
    async fn authenticated_as_panics_on_principal_mismatch() {
        let principal = PrincipalId::new();
        let app = harness().acting_as(principal.to_string());
        app.post("/login")
            .send()
            .await
            .assert_authenticated_as(PrincipalId::new());
    }

    // -------------------------------------------------- Wave 7 P2: view data

    #[tokio::test]
    async fn view_context_asserts_walk_dotted_keys() {
        let app = harness();
        app.get("/view").send().await
            .assert_view_has("title")
            .assert_view_has("user.name");
    }

    #[tokio::test]
    #[should_panic(expected = "no view rendered")]
    async fn view_assert_panics_without_a_render() {
        let app = harness();
        app.get("/json").send().await.assert_view_has("title");
    }

    #[tokio::test]
    #[should_panic(expected = "no view rendered")]
    async fn a_previous_response_view_never_satisfies_the_assert() {
        let app = harness();
        // First response rendered a view; the second did not.
        app.get("/view").send().await.assert_view_has("title");
        app.get("/json").send().await.assert_view_has("title");
    }

    #[tokio::test]
    #[should_panic(expected = "context lacks key")]
    async fn view_assert_panics_when_key_absent() {
        let app = harness();
        app.get("/view").send().await.assert_view_has("ghost");
    }

    // ------------------------------------------------------- MVVC: the binder

    /// The view model the binder cases bind: a typed struct that declares
    /// the view it renders into and the fields that view may read.
    #[derive(Serialize, serde::Deserialize)]
    struct PageVm {
        title: String,
        depth: u64,
    }

    impl kelson_views::ViewModel for PageVm {
        const VIEW: &'static str = "page";
        fn schema() -> &'static [kelson_views::FieldSchema] {
            kelson_views::view_schema![
                kelson_views::FieldSchema::required("title", kelson_views::FieldKind::String),
                kelson_views::FieldSchema::required("depth", kelson_views::FieldKind::Int),
            ]
        }
    }

    /// An engine over a view that binds the page model, plus the recorder
    /// that watches its renders.
    fn bound_view_engine(template: &str) -> (Arc<kelson_views::ViewsEngine>, Arc<ViewRecorder>) {
        let dir = std::env::temp_dir().join(format!(
            "kelson-testkit-vm-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let views = dir.join("resources").join("views");
        std::fs::create_dir_all(&views).unwrap();
        std::fs::write(views.join("page.html"), template).unwrap();
        let engine = Arc::new(kelson_views::ViewsEngine::new(kelson_views::ViewsConfig {
            paths: vec![views],
            cache_dir: None,
            env: "testing".into(),
        }));
        let mut registry = kelson_views::ViewRegistry::new();
        registry.register(kelson_views::ViewBinding::new::<PageVm>("page"));
        engine.set_registry(Arc::new(registry));
        let recorder = Arc::new(ViewRecorder::new());
        engine.set_tap(recorder.clone());
        (engine, recorder)
    }

    /// A router serving the bound view through the engine.
    fn bound_view_router(engine: Arc<kelson_views::ViewsEngine>) -> Router {
        Router::new().route(
            "/page",
            get(move || {
                let engine = engine.clone();
                async move {
                    engine
                        .render_bound(
                            "page",
                            json!({}),
                            &PageVm {
                                title: "Queue".to_string(),
                                depth: 7,
                            },
                        )
                        .expect("the model renders")
                }
            }),
        )
    }

    #[tokio::test]
    async fn a_view_model_is_assertable_without_the_html() {
        let (engine, recorder) = bound_view_engine("@vm<h1>{{ vm.title }}</h1>");
        let app = TestApp::new(bound_view_router(engine.clone())).with_views(recorder);
        // The assertions read the MODEL the view received - never the
        // HTML it rendered.
        app.get("/page").send().await
            .assert_view_model_has("title")
            .assert_view_model::<PageVm>("depth")
            .assert_view_lints_clean(&engine)
            .assert_status(200);
    }

    #[tokio::test]
    async fn a_template_that_reads_an_undeclared_path_is_refused_by_the_lint() {
        let (engine, _recorder) = bound_view_engine("@vm{{ vm.ghost }}");
        let error = engine
            .lint("page")
            .expect_err("an undeclared path is refused by the lint");
        assert_eq!(error.code, "VIEW-RENDER");
        assert!(
            error.message.contains("ghost"),
            "the refusal names the path: {}",
            error.message
        );
    }

    #[tokio::test]
    #[should_panic(expected = "does not declare")]
    async fn a_view_model_assert_refuses_an_undeclared_field() {
        let (engine, recorder) = bound_view_engine("@vm<h1>{{ vm.title }}</h1>");
        let app = TestApp::new(bound_view_router(engine)).with_views(recorder);
        app.get("/page").send().await
            .assert_view_model::<PageVm>("ghost");
    }

    // ------------------------------------------------ Wave 7 P2: database row

    #[tokio::test]
    async fn database_asserts_run_under_the_test_tenant() {
        let app = harness().tenant(TenantId::new().to_string());
        app.post("/db").send().await.assert_status(200);
        let response = app.get("/json").send().await;
        response
            .assert_database_has(&patient_model(), &[("email", json!("ada@clinic.test"))])
            .assert_database_has(&patient_model(), &[("status", json!("active"))])
            .assert_database_count(&patient_model(), 1)
            .assert_database_missing(&patient_model(), &[("status", json!("cancelled"))]);
    }

    #[tokio::test]
    async fn database_probe_is_scoped_to_each_test_tenant() {
        let store = InMemoryModelStore::new();
        let mine = TenantId::new();
        let theirs = TenantId::new();

        let app = harness_with_store(store.clone()).tenant(mine.to_string());
        app.post("/db").send().await.assert_status(200);

        // Same store, another test tenant: the row is invisible.
        let other = harness_with_store(store).tenant(theirs.to_string());
        other
            .get("/json")
            .send()
            .await
            .assert_database_missing(&patient_model(), &[("email", json!("ada@clinic.test"))])
            .assert_database_count(&patient_model(), 0);
    }

    #[tokio::test]
    #[should_panic(expected = "expected row in")]
    async fn database_has_panics_when_no_row_matches() {
        let app = harness().tenant(TenantId::new().to_string());
        app.get("/json")
            .send()
            .await
            .assert_database_has(&patient_model(), &[("email", json!("ada@clinic.test"))]);
    }

    #[tokio::test]
    #[should_panic(expected = "database count mismatch")]
    async fn database_count_panics_on_mismatch() {
        let app = harness().tenant(TenantId::new().to_string());
        app.get("/json").send().await.assert_database_count(&patient_model(), 3);
    }

    #[tokio::test]
    #[should_panic(expected = "expected no row in")]
    async fn database_missing_panics_when_a_row_is_there() {
        let app = harness().tenant(TenantId::new().to_string());
        app.post("/db").send().await.assert_status(200);
        app.get("/json")
            .send()
            .await
            .assert_database_missing(&patient_model(), &[("email", json!("ada@clinic.test"))]);
    }

    #[tokio::test]
    #[should_panic(expected = "with_models")]
    async fn database_asserts_require_the_model_probe() {
        let app = TestApp::new(Router::new().route("/x", get(|| async { "ok" })));
        app.get("/x")
            .send()
            .await
            .assert_database_count(&patient_model(), 0);
    }

    // ------------------------------------ Wave 7 P3: fake queue/scheduler

    fn interval_item(name: &str, operation: &str, secs: u64) -> ScheduleItem {
        ScheduleItem {
            name: name.into(),
            operation: operation.into(),
            spec: kelson_scheduling::ScheduleSpec::Interval { secs },
            enabled: true,
            ..Default::default()
        }
    }

    /// Wave 7 P3 (docs/19 L340/L342): scheduler `next_run` reads the
    /// injected clock — `travel` moves due-ness, not wall time — and every
    /// fired item lands in the fake queue as a durable job.
    #[test]
    fn fake_scheduler_fires_due_items_into_the_fake_queue_under_a_frozen_clock() {
        let t0 = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let clock = crate::freeze_time(t0);
        let mut app = TestApp::new(Router::new())
            .with_fake_scheduler()
            .with_fake_queue()
            .with_clock(clock.clone());
        app.schedule(interval_item("billing.close-day", "billing.close_day", 60))
            .unwrap();

        // Never fired -> due on the first pass.
        let fired = app.run_due_schedules().unwrap();
        assert_eq!(fired, vec!["billing.close-day".to_string()]);
        app.queue().assert_pushed_count("billing.close_day", 1);

        // Same frozen instant: inside the window, nothing refires.
        assert!(app.run_due_schedules().unwrap().is_empty());
        app.queue().assert_pushed_count("billing.close_day", 1);

        // Travel to one second before the window closes: still quiet.
        crate::travel(&clock, 59);
        assert!(app.run_due_schedules().unwrap().is_empty());
        app.queue().assert_pushed_count("billing.close_day", 1);

        // Travel past the window: due again on the same frozen clock.
        crate::travel(&clock, 2);
        let fired = app.run_due_schedules().unwrap();
        assert_eq!(fired, vec!["billing.close-day".to_string()]);
        app.queue().assert_pushed_count("billing.close_day", 2);
    }

    #[test]
    #[should_panic(expected = "with_fake_scheduler")]
    fn run_due_schedules_requires_the_scheduler_probe() {
        let mut app = TestApp::new(Router::new());
        let _ = app.run_due_schedules();
    }

    #[test]
    #[should_panic(expected = "with_fake_queue")]
    fn run_due_schedules_requires_the_queue_probe() {
        let mut app = TestApp::new(Router::new()).with_fake_scheduler();
        let _ = app.run_due_schedules();
    }

    #[test]
    #[should_panic(expected = "with_fake_queue")]
    fn queue_accessor_requires_the_fake() {
        let app = TestApp::new(Router::new());
        let _ = app.queue();
    }

    #[test]
    #[should_panic(expected = "with_fake_scheduler")]
    fn schedule_registration_requires_the_scheduler_probe() {
        let mut app = TestApp::new(Router::new());
        let _ = app.schedule(interval_item("a.b", "a.b", 60));
    }
}
