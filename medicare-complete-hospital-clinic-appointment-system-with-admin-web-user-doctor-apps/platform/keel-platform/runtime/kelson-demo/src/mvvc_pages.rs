//! MVA-007 — the runtime's first read-model page and first bridged form.
//!
//! The audit found zero application consumers for three shipped MVVC
//! layers: no `ReadModelDef` anywhere, no `[[validation]]` spec ever
//! becoming a form, and the route edge's head built once with one
//! hard-coded page identity. This module closes all three at the runtime
//! level, against the REAL shipped declarations:
//!
//! * **a read-model page** (`GET /mvc/reports/daily`): one
//!   `ReadModelDef` (`appointments_daily`) registered in a
//!   [`kelson_models::ReadModelRegistry`], a [`DemoReadModelStore`] that
//!   projects the appointments store on demand, and a page that renders
//!   the projection with its staleness surface. A boot without a model
//!   backend installs the def but no store — the page then refuses with
//!   an honest error instead of rendering rows it never projected.
//! * **a form page built from the shipped `[[validation]]` spec**
//!   (`GET|POST /mvc/book-form`): the appointments manifest's `book`
//!   specs (patient / slot / photo) parse into
//!   [`BridgedForm`](kelson_views::BridgedForm) fields,
//!   the GET renders them, and the POST round-trips — session + minted
//!   `_token` verified in constant time, per-field errors re-rendered on
//!   failure, the accepted values echoed on success.
//! * **per-page head authoring**: both pages build their own
//!   [`kelson_foundation::HeadStore`] and the templates read it through
//!   the reserved `_head` key — the same assembler the route edge uses
//!   (whose head is now derived from the view the route declared,
//!   `graph_routes::page_head`).
//!
//! Boundary: the booking COMMAND itself is proven by the MVA-004 page
//! and `http_integration`; this form proves the bridge (spec → fields →
//! render → validation round-trip), not a second persistence path.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::{Extension, Form, State};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::{Json, Router};
use kelson_foundation::AppError;
use kelson_http::{Envelope, ResolvedSession};
use kelson_models::{
    FieldFilter, ListOpts, ReadModelDef, ReadModelRefreshResult, ReadModelRegistry,
    ReadModelStaleness, ReadModelState, ReadModelStore, Record,
};
use serde_json::{json, Value};

use crate::routes::AppState;

/// The one read model this runtime declares: appointments projected for
/// the clinic-day report (MVA-007's `ReadModelDef`).
pub const DAILY_MODEL: &str = "appointments_daily";

/// The shipped views this module serves (dots = path separators).
pub const DAILY_VIEW: &str = "pages.mvc-daily";
pub const BOOK_FORM_VIEW: &str = "pages.mvc-book-form";

/// The form's action — the browser posts back to the page that rendered
/// it, never to an endpoint the template invented.
pub const BOOK_FORM_ACTION: &str = "/mvc/book-form";

/// The report's head identity (per-page head authoring).
const DAILY_TITLE: &str = "Appointments daily report · Kelson";
const BOOK_FORM_TITLE: &str = "Book an appointment · Kelson";

// ------------------------------------------------------------- read model

/// The def the runtime registers: one projection over the appointments
/// model, refreshed on demand by the page (the staleness surface then
/// reports the age of that refresh honestly).
pub fn daily_def() -> ReadModelDef {
    ReadModelDef::new(
        DAILY_MODEL,
        "appointments_daily",
        "project a tenant's appointments (id, patient, slot, status) for the clinic-day report",
    )
    .source_model("Appointment")
    .on_demand()
}

/// The runtime's read-model registry: the def always registers; the
/// store installs only when the boot has a model backend to project
/// from. A storeless boot keeps the declaration and fails the page
/// honestly (never a fabricated row).
pub fn demo_read_models(backend: Option<Arc<dyn kelson_models::ModelStore>>) -> ReadModelRegistry {
    let mut registry = ReadModelRegistry::new();
    registry
        .register(daily_def())
        .expect("one appointments_daily registration");
    if let Some(backend) = backend {
        registry.install_store(DAILY_MODEL, Arc::new(DemoReadModelStore::new(backend)));
    }
    registry
}

/// A [`ReadModelStore`] over the host's model store: `refresh` projects
/// the source rows, `query` serves the projection tenant-scoped.
///
/// Deliberate scope: the demo store materialises in memory (the
/// database-backed stores own the SQL twin). Field filters are refused
/// rather than half-applied — the page asks for the tenant's rows and
/// paginates, and an unsupported query must never silently return the
/// wrong slice.
pub struct DemoReadModelStore {
    backend: Arc<dyn kelson_models::ModelStore>,
    /// Tenant-keyed materialised rows (uuid string = one bucket).
    rows: Mutex<HashMap<String, Vec<Record>>>,
    /// When the projection last materialised (drives staleness).
    last_refresh: Mutex<Option<time::OffsetDateTime>>,
}

impl std::fmt::Debug for DemoReadModelStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DemoReadModelStore")
            .field("backend", &"<dyn ModelStore>")
            .finish_non_exhaustive()
    }
}

/// Project one source row into the report's whitelist: the read model is
/// a projection, never a passthrough of every stored column.
fn project_row(row: &Record) -> Record {
    let mut out = Record::new();
    for field in ["id", "patient", "slot", "status"] {
        if let Some(value) = row.get(field) {
            out.insert(field.to_string(), value.clone());
        }
    }
    out
}

impl DemoReadModelStore {
    pub fn new(backend: Arc<dyn kelson_models::ModelStore>) -> Self {
        Self {
            backend,
            rows: Mutex::new(HashMap::new()),
            last_refresh: Mutex::new(None),
        }
    }

    /// Materialise one tenant (or every tenant) from the source store.
    fn project(&self, tenant: Option<kelson_foundation::TenantId>) -> Result<u64, AppError> {
        let model = kelson_appointments_contract::appointment_model();
        let page = self.backend.list(&model, tenant, &[], &ListOpts::default())?;
        let projected: Vec<Record> = page.items.iter().map(project_row).collect();
        let count = projected.len() as u64;
        let mut buckets = self.rows.lock().unwrap();
        match tenant {
            Some(tenant) => {
                buckets.insert(tenant.to_string(), projected);
            }
            None => {
                // An all-tenant refresh replaces every bucket: the
                // materialisation is the query's result, not an append.
                buckets.clear();
                for row in projected {
                    let key = row
                        .get("tenant_id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_default();
                    buckets.entry(key).or_default().push(row);
                }
            }
        }
        Ok(count)
    }

    /// The rows a query answers from, tenant-scoped.
    fn rows_for(&self, tenant: Option<kelson_foundation::TenantId>) -> Vec<Record> {
        let buckets = self.rows.lock().unwrap();
        match tenant {
            Some(tenant) => buckets.get(&tenant.to_string()).cloned().unwrap_or_default(),
            None => buckets.values().flatten().cloned().collect(),
        }
    }
}

impl ReadModelStore for DemoReadModelStore {
    fn refresh(
        &self,
        _read_model: &ReadModelDef,
        tenant: Option<kelson_foundation::TenantId>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ReadModelRefreshResult, AppError>> + Send + '_>,
    > {
        Box::pin(async move {
            let started = time::OffsetDateTime::now_utc();
            let rows_materialised = self.project(tenant)?;
            let finished = time::OffsetDateTime::now_utc();
            *self.last_refresh.lock().unwrap() = Some(finished);
            Ok(ReadModelRefreshResult {
                rows_materialised,
                tenants_refreshed: 1,
                duration_ms: (finished - started).whole_milliseconds().max(0) as u64,
                success: true,
                error: None,
            })
        })
    }

    fn refresh_all_tenants(
        &self,
        read_model: &ReadModelDef,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ReadModelRefreshResult, AppError>> + Send + '_>,
    > {
        self.refresh(read_model, None)
    }

    fn staleness(
        &self,
        read_model: &ReadModelDef,
        _tenant: Option<kelson_foundation::TenantId>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ReadModelStaleness, AppError>> + Send + '_>,
    > {
        // The trait's `+ '_` binds to `&self`, so the future may not carry
        // `read_model`'s shorter borrow — take what it needs by value.
        let name = read_model.name.clone();
        Box::pin(async move {
            let last = *self.last_refresh.lock().unwrap();
            let (state, age_seconds) = match last {
                None => (ReadModelState::Stale, None),
                Some(at) => {
                    let age = (time::OffsetDateTime::now_utc() - at)
                        .whole_seconds()
                        .max(0) as u64;
                    // On-demand projection: fresh inside the report's
                    // five-minute tolerance, stale beyond it (the page
                    // refreshes on every GET, so this reports honestly
                    // between refreshes rather than lying Fresh).
                    let state = if age <= 300 {
                        ReadModelState::Fresh
                    } else {
                        ReadModelState::Stale
                    };
                    (state, Some(age))
                }
            };
            let last_refreshed_at = last.map(|at| {
                at.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default()
            });
            Ok(ReadModelStaleness {
                name,
                state,
                age_seconds,
                last_refreshed_at,
                last_error: None,
            })
        })
    }

    fn query(
        &self,
        _read_model: &ReadModelDef,
        tenant: Option<kelson_foundation::TenantId>,
        filters: &[FieldFilter],
        opts: &ListOpts,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<kelson_query::Page<Record>, AppError>>
                + Send
                + '_,
        >,
    > {
        // Same lifetime rule as `staleness`: take the borrowed inputs by
        // value before the future, so the box is only ever tied to `&self`.
        let refuse_filters = !filters.is_empty();
        let sort = opts.sort.clone();
        let page = opts.page;
        let limit = opts.limit;
        Box::pin(async move {
            if refuse_filters {
                // Fail closed: a filter this store cannot apply must not
                // be dropped and answered with the wrong slice.
                return Err(AppError::not_implemented(
                    "the demo read-model store applies no field filters",
                ));
            }
            let mut rows = self.rows_for(tenant);
            if let Some((field, descending)) = &sort {
                rows.sort_by(|a, b| {
                    let ord = match (a.get(field), b.get(field)) {
                        (Some(x), Some(y)) => match (x.as_f64(), y.as_f64()) {
                            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                            _ => x
                                .as_str()
                                .unwrap_or_default()
                                .cmp(y.as_str().unwrap_or_default()),
                        },
                        _ => std::cmp::Ordering::Equal,
                    };
                    if *descending {
                        ord.reverse()
                    } else {
                        ord
                    }
                });
            }
            let total = rows.len() as u64;
            let (page_no, per_page) = match &page {
                Some(req) => (req.page, req.per_page),
                None => (1, rows.len().max(1) as u32),
            };
            if let Some(limit) = limit {
                rows.truncate(limit as usize);
            }
            if let Some(req) = &page {
                let start = (req.page.saturating_sub(1)) as usize * req.per_page as usize;
                let end = start.saturating_add(req.per_page as usize);
                rows = rows.into_iter().skip(start).take(end.saturating_sub(start)).collect();
            }
            Ok(kelson_query::Page::new(rows, page_no, per_page, Some(total)))
        })
    }

    fn find(
        &self,
        _read_model: &ReadModelDef,
        id: uuid::Uuid,
        tenant: Option<kelson_foundation::TenantId>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<Record>, AppError>> + Send + '_>,
    > {
        Box::pin(async move {
            let needle = id.to_string();
            Ok(self
                .rows_for(tenant)
                .into_iter()
                .find(|row| row.get("id").and_then(Value::as_str) == Some(needle.as_str())))
        })
    }
}

// ------------------------------------------------------------ form from spec

/// The shipped manifest's `book` validation specs — parsed from the
/// REAL `module.toml` at first use, never retyped here (a spec change
/// changes the form).
fn book_specs() -> &'static [kelson_registry::ValidationSpec] {
    static SPECS: OnceLock<Vec<kelson_registry::ValidationSpec>> = OnceLock::new();
    SPECS.get_or_init(|| {
        let manifest = kelson_registry::ModuleManifest::from_toml(include_str!(
            "../../../applications/appointments/module.toml"
        ))
        .expect("the shipped appointments manifest parses");
        manifest
            .validation
            .into_iter()
            .filter(|spec| spec.operation == "book")
            .collect()
    })
}

/// A human label for a field name ("patient" → "Patient").
fn titleize(field: &str) -> String {
    let mut chars = field.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Map one manifest rule onto the bridge's typed rule. Rules the HTML
/// plane models directly become typed (required/max/min/email/url); the
/// rest — date, before, unique, image, mimes, extensions, dimensions —
/// ride as named custom rules so the declared rule stays visible on the
/// field instead of vanishing in translation.
fn map_rule(raw: &str) -> kelson_views::ValidationRule {
    use kelson_views::ValidationRule as R;
    let (head, arg) = match raw.split_once(':') {
        Some((head, arg)) => (head, Some(arg)),
        None => (raw, None),
    };
    match (head, arg) {
        ("required", None) => R::Required,
        ("email", None) => R::Email,
        ("url", None) => R::Url,
        ("max", Some(arg)) => arg
            .parse()
            .map(R::MaxLength)
            .unwrap_or_else(|_| R::Custom(raw.to_string())),
        ("min", Some(arg)) => arg
            .parse()
            .map(R::MinLength)
            .unwrap_or_else(|_| R::Custom(raw.to_string())),
        _ => R::Custom(raw.to_string()),
    }
}

/// The HTML control a spec's rules ask for: attachments become `file`
/// inputs, date-shaped fields become `date`, everything else `text`.
fn field_type_of(spec: &kelson_registry::ValidationSpec) -> &'static str {
    let attachment = ["image", "mimes", "extensions", "dimensions"];
    if spec
        .rules
        .iter()
        .any(|rule| attachment.iter().any(|prefix| rule == prefix || rule.starts_with(prefix)))
    {
        "file"
    } else if spec
        .rules
        .iter()
        .any(|rule| rule == "date" || rule.starts_with("before:") || rule.starts_with("after:"))
    {
        "date"
    } else {
        "text"
    }
}

/// Build the book form from the manifest's `[[validation]]` specs: one
/// definition that renders the fields AND drives the round-trip's
/// validation (the bridge's one-definition rule).
pub fn book_form() -> kelson_views::BridgedForm {
    let fields = book_specs()
        .iter()
        .map(|spec| kelson_views::FormField {
            name: spec.field.clone(),
            label: titleize(&spec.field),
            field_type: field_type_of(spec).to_string(),
            required: spec.rules.iter().any(|rule| rule == "required"),
            validation_rules: spec.rules.iter().map(|rule| map_rule(rule)).collect(),
            value: None,
            error: None,
            options: None,
            placeholder: None,
        })
        .collect();
    kelson_views::BridgedForm::new(BOOK_FORM_ACTION, "POST", fields)
}

/// The form as the template reads it.
fn form_vm(form: &kelson_views::BridgedForm) -> kelson_views::FormVm {
    kelson_views::FormVm {
        action: form.action.clone(),
        method: form.method.clone(),
        fields: form.fields.clone(),
        csrf_token: form.csrf_token.clone(),
        error: form.error.clone(),
    }
}

/// Constant-time token comparison — the session-bound form token is
/// compared without an early exit (the MNC-003 posture at this edge).
fn token_eq(presented: &str, minted: &str) -> bool {
    let (a, b) = (presented.as_bytes(), minted.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= usize::from(x ^ y);
    }
    diff == 0
}

// ------------------------------------------------------------------ rendering

/// The request's locale, resolved exactly as the dashboard resolves it.
fn locale_of(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    session: Option<&ResolvedSession>,
) -> String {
    crate::i18n::resolve(
        &state.messages,
        crate::i18n::session_locale(session),
        None,
        headers
            .get(axum::http::header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok()),
    )
}

/// The per-page head these standalone pages author (the same
/// `HeadStore` + `_head` assembler the route edge uses).
fn page_head(title: &str) -> kelson_foundation::HeadStore {
    kelson_foundation::HeadBuilder::new()
        .title(title)
        .meta("description", "Kelson application")
        .build()
}

/// Render the page through the views engine; a boot without the engine
/// (or without this template) serves the SAME data as a JSON envelope —
/// the dashboard's never-a-500 contract, never a broken half-page.
fn render_page(
    state: &AppState,
    view: &str,
    context: Value,
    fallback: Value,
) -> axum::response::Response {
    match state.views.as_ref() {
        Some(engine) if engine.exists(view) => match engine.render(view, context) {
            Ok(html) => Html(html).into_response(),
            Err(error) => {
                tracing::warn!(%error, view, "page render failed; serving the JSON envelope");
                Json(serde_json::to_value(Envelope::ok(fallback)).unwrap_or_default())
                    .into_response()
            }
        },
        _ => Json(serde_json::to_value(Envelope::ok(fallback)).unwrap_or_default()).into_response(),
    }
}

/// The store this boot's report reads through — or the honest refusal as
/// an error the caller turns into a response (a boot without a model
/// backend never invents rows).
fn report_store(
    state: &AppState,
) -> Result<(ReadModelDef, Arc<dyn ReadModelStore>), AppError> {
    let registry = &state.read_models;
    let def = registry
        .get(DAILY_MODEL)
        .cloned()
        .ok_or_else(|| AppError::internal("the appointments_daily read model is not registered"))?;
    let store = registry
        .store(DAILY_MODEL)
        .ok_or_else(|| {
            AppError::internal("the appointments_daily read model has no store in this boot")
        })?;
    Ok((def, store))
}

// -------------------------------------------------------------------- routes

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/mvc/reports/daily", get(daily))
        .route("/mvc/book-form", get(book_form_get).post(book_form_post))
}

/// `GET /mvc/reports/daily` — refresh the read model, query the
/// projection, carry its staleness, render the report page.
pub async fn daily(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
) -> axum::response::Response {
    let (def, store) = match report_store(&state) {
        Ok(pair) => pair,
        Err(error) => return state.error_response(&error, &headers, None),
    };
    let tenant = state.default_tenant_id;
    if let Err(error) = store.refresh(&def, tenant).await {
        return state.error_response(&error, &headers, None);
    }
    let page = match store.query(&def, tenant, &[], &ListOpts::default()).await {
        Ok(page) => page,
        Err(error) => return state.error_response(&error, &headers, None),
    };
    let staleness = match store.staleness(&def, tenant).await {
        Ok(staleness) => staleness,
        Err(error) => return state.error_response(&error, &headers, None),
    };
    let payload = json!({
        "model": DAILY_MODEL,
        "rows": page.items,
        "total": page.total,
        "staleness": staleness,
    });
    let mut context = payload.clone();
    kelson_foundation::ViewContext::localized(&state.messages, &locale_of(&state, &headers, session.as_deref()))
        .with_head(page_head(DAILY_TITLE))
        .apply(&mut context);
    render_page(&state, DAILY_VIEW, context, payload)
}

/// `GET /mvc/book-form` — the form rendered from the shipped spec, with
/// the session's minted token (absent for a guest — the lane the POST
/// then refuses).
pub async fn book_form_get(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
) -> axum::response::Response {
    let mut form = book_form();
    form.csrf_token = crate::graph_routes::csrf_token(&state, session.as_deref());
    let payload = json!({ "form": form_vm(&form) });
    let mut context = payload.clone();
    kelson_foundation::ViewContext::localized(&state.messages, &locale_of(&state, &headers, session.as_deref()))
        .with_head(page_head(BOOK_FORM_TITLE))
        .apply(&mut context);
    render_page(&state, BOOK_FORM_VIEW, context, payload)
}

/// `POST /mvc/book-form` — session required, minted `_token` verified
/// in constant time, then the SHARED definition validates the
/// submission: failures re-render with per-field errors and the
/// submitted values intact (the round-trip), acceptance echoes what was
/// accepted.
pub async fn book_form_post(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
    Form(submitted): Form<HashMap<String, String>>,
) -> axum::response::Response {
    // No session: the form was minted against one, so this lane is a
    // uniform 401 — the guest's GET rendered no usable token either.
    let Some(resolved) = session.as_deref() else {
        return state.error_response(&AppError::unauthenticated(), &headers, None);
    };
    let minted = match crate::graph_routes::csrf_token(&state, Some(resolved)) {
        Some(token) => token,
        None => {
            return state.error_response(&AppError::unauthenticated(), &headers, None);
        }
    };
    let presented = submitted.get("_token").map(String::as_str).unwrap_or("");
    if !token_eq(presented, &minted) {
        return state.error_response(
            &AppError::denied("the form token does not match this session"),
            &headers,
            None,
        );
    }
    // The declared field list is authoritative: `_token` is the
    // framework's, not the form's, and an undeclared key never enters
    // (the bridge drops it on populate anyway — it never reaches the
    // validation map here).
    let submitted_map: serde_json::Map<String, Value> = submitted
        .iter()
        .filter(|(key, _)| key.as_str() != "_token" && key.as_str() != "_method")
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();

    let mut form = book_form();
    form.csrf_token = Some(minted);
    let accepted = form.validate_with_data(&submitted_map);

    if !accepted {
        // The bridge filled the form-level summary; the per-field
        // messages come from the same declared rules, so the re-render
        // points at each field that failed. Apply them to the DEFINITION
        // first — the view model the template reads is built after.
        let errors = form_vm(&form).validate(&submitted_map);
        for (name, message) in errors {
            if let Some(field) = form.fields.iter_mut().find(|field| field.name == name) {
                field.error = Some(message);
            }
        }
    }

    let extra = if accepted {
        json!({
            "accepted": {
                "patient": submitted_map.get("patient").cloned().unwrap_or(Value::Null),
                "slot": submitted_map.get("slot").cloned().unwrap_or(Value::Null),
            }
        })
    } else {
        json!({ "accepted": Value::Null })
    };
    let mut payload = json!({ "form": form_vm(&form) });
    if let (Some(target), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    let mut context = payload.clone();
    kelson_foundation::ViewContext::localized(
        &state.messages,
        &locale_of(&state, &headers, Some(resolved)),
    )
    .with_head(page_head(BOOK_FORM_TITLE))
    .apply(&mut context);
    render_page(&state, BOOK_FORM_VIEW, context, payload)
}
