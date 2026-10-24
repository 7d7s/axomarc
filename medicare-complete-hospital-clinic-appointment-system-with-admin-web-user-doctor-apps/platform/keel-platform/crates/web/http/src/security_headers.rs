//! Wave 7 P4 (docs/19 "Security-headers middleware"): response hardening
//! for the **web plane**. `kelson-demo` attaches this as the outermost
//! per-route layer on web-plane graph routes only, so gate rejections,
//! chain short-circuits and error views all carry the posture while the
//! api plane stays byte-identical.
//!
//! The posture itself is plain data resolved from `[security.headers]` at
//! boot (the adapter class may not depend on application config, so the
//! conversion happens in kelson-demo like `ViewsSection` → `ViewsConfig`).

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;

/// Resolved security-header posture for one deployment. `Default` is the
/// switch-off posture (everything unmounted); the documented baseline
/// values live behind `enabled` in `[security.headers]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecurityHeaders {
    /// Master switch — `apply` is a no-op while this is false (and
    /// kelson-demo does not even attach the layer).
    pub enabled: bool,
    /// HSTS `max-age` in seconds; `0` omits `Strict-Transport-Security`.
    pub hsts_max_age_secs: u64,
    /// Append `; includeSubDomains` to HSTS (opt-in).
    pub hsts_include_subdomains: bool,
    /// Baseline CSP (`default-src 'self'` by default, overridable per
    /// deployment); an empty string omits the header.
    pub csp: String,
    /// `Referrer-Policy` value; an empty string omits the header.
    pub referrer_policy: String,
}

impl SecurityHeaders {
    /// Stamp the configured posture onto a response. Handler-set headers
    /// win (the middleware only fills what the handler left unset), and a
    /// config value that cannot be a header value (control bytes) is
    /// skipped fail-closed — never a panic, never a partial write.
    pub fn apply(&self, res: &mut Response) {
        self.apply_with_nonce(res, "");
    }

    /// Stamp the posture, substituting `{nonce}` in the configured CSP with
    /// `nonce` (F-6).
    ///
    /// A strict CSP names the scripts it allows (`script-src 'nonce-…'`), and
    /// the nonce has to be **per response** — a static value in the header is
    /// exactly what a strict CSP forbids. When the configured CSP carries the
    /// `{nonce}` placeholder, the middleware generates a fresh nonce, puts it
    /// in the request extensions ([`CspNonce`]) so the view can stamp it on
    /// its `<script>` tags, and substitutes it here.
    ///
    /// A CSP without the placeholder is emitted verbatim (today's behavior).
    pub fn apply_with_nonce(&self, res: &mut Response, nonce: &str) {
        if !self.enabled {
            return;
        }
        let headers = res.headers_mut();
        insert_if_absent(headers, "x-content-type-options", "nosniff");
        if self.hsts_max_age_secs > 0 {
            let value = if self.hsts_include_subdomains {
                format!("max-age={}; includeSubDomains", self.hsts_max_age_secs)
            } else {
                format!("max-age={}", self.hsts_max_age_secs)
            };
            insert_if_absent(headers, "strict-transport-security", &value);
        }
        if !self.csp.is_empty() {
            let value = if !nonce.is_empty() && self.csp.contains("{nonce}") {
                self.csp.replace("{nonce}", nonce)
            } else {
                self.csp.clone()
            };
            insert_if_absent(headers, "content-security-policy", &value);
        }
        if !self.referrer_policy.is_empty() {
            insert_if_absent(headers, "referrer-policy", &self.referrer_policy);
        }
    }
}

/// The per-request CSP nonce, carried in the **request extensions** so the
/// view (or the handler) can stamp it on the `<script>` tags it emits — the
/// CSP header and the tags must carry the same value (F-6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CspNonce(pub String);

/// Generate a fresh CSP nonce: 16 random bytes as 32 hex characters. The hex
/// alphabet is a subset of base64, so the value is a valid CSP nonce.
fn generate_nonce() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `or_insert` semantics: an already-present header (set by the handler
/// or an inner layer) is left exactly as it is.
fn insert_if_absent(headers: &mut HeaderMap, name: &'static str, value: &str) {
    let name = HeaderName::from_static(name);
    if headers.contains_key(&name) {
        return;
    }
    if let Ok(parsed) = HeaderValue::from_str(value) {
        headers.insert(name, parsed);
    }
}

/// Axum middleware: run the route, then stamp the posture on whatever
/// answer it produced (200 view, error view, envelope, 204 …). Attached
/// outermost per web-plane route, so every in-route answer is covered;
/// pre-router short-circuits (rate limit, maintenance) stay outside the
/// route by design — they are edge posture, not web-plane responses.
pub async fn security_headers_fn(
    State(headers): State<SecurityHeaders>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    // A strict CSP needs a **per-response** nonce: a static value in the
    // header is exactly what `script-src 'nonce-…'` forbids. The nonce is
    // generated before the route runs and travels in the request extensions
    // ([`CspNonce`]) so the view can stamp it on its `<script>` tags; the
    // same value is substituted into the CSP here.
    let nonce = generate_nonce();
    req.extensions_mut().insert(CspNonce(nonce.clone()));
    let mut res = next.run(req).await;
    headers.apply_with_nonce(&mut res, &nonce);
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn baseline() -> SecurityHeaders {
        SecurityHeaders {
            enabled: true,
            hsts_max_age_secs: 31_536_000,
            hsts_include_subdomains: false,
            csp: "default-src 'self'".into(),
            referrer_policy: "strict-origin-when-cross-origin".into(),
        }
    }

    fn get<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
        headers.get(name).and_then(|v| v.to_str().ok())
    }

    #[test]
    fn off_by_default_leaves_the_response_untouched() {
        let mut res = Response::new(Body::empty());
        SecurityHeaders::default().apply(&mut res);
        for name in [
            "strict-transport-security",
            "x-content-type-options",
            "content-security-policy",
            "referrer-policy",
        ] {
            assert!(get(res.headers(), name).is_none(), "{name} must not appear");
        }
    }

    #[test]
    fn enabled_emits_the_full_documented_baseline() {
        let mut res = Response::new(Body::empty());
        baseline().apply(&mut res);
        assert_eq!(
            get(res.headers(), "x-content-type-options"),
            Some("nosniff")
        );
        assert_eq!(
            get(res.headers(), "strict-transport-security"),
            Some("max-age=31536000")
        );
        assert_eq!(
            get(res.headers(), "content-security-policy"),
            Some("default-src 'self'")
        );
        assert_eq!(
            get(res.headers(), "referrer-policy"),
            Some("strict-origin-when-cross-origin")
        );
    }

    /// F-6: the `{nonce}` placeholder in the configured CSP is substituted
    /// with the per-response nonce.
    #[test]
    fn csp_nonce_placeholder_is_substituted() {
        let headers = SecurityHeaders {
            csp: "script-src 'self' 'nonce-{nonce}'".into(),
            ..baseline()
        };
        let mut res = Response::new(Body::empty());
        headers.apply_with_nonce(&mut res, "abc123");
        assert_eq!(
            get(res.headers(), "content-security-policy"),
            Some("script-src 'self' 'nonce-abc123'")
        );
    }

    /// A CSP without the placeholder is emitted verbatim — the nonce is only
    /// substituted when the deployment asked for it.
    #[test]
    fn csp_without_the_placeholder_is_verbatim() {
        let mut res = Response::new(Body::empty());
        baseline().apply_with_nonce(&mut res, "abc123");
        assert_eq!(
            get(res.headers(), "content-security-policy"),
            Some("default-src 'self'")
        );
    }

    /// The nonce is fresh per response and long enough to be unguessable.
    #[test]
    fn nonce_is_fresh_and_long() {
        let a = generate_nonce();
        let b = generate_nonce();
        assert_ne!(a, b, "two nonces must differ");
        assert_eq!(a.len(), 32, "16 random bytes as hex: {a}");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()), "hex only: {a}");
    }

    /// The middleware hands the nonce to the route (request extensions) and
    /// stamps the same value into the CSP — the two must agree.
    #[tokio::test]
    async fn middleware_shares_the_nonce_with_the_route() {
        use axum::Router;

        let headers = SecurityHeaders {
            csp: "script-src 'nonce-{nonce}'".into(),
            ..baseline()
        };
        let app = Router::new()
            .route(
                "/",
                axum::routing::get(|req: Request<Body>| async move {
                    let nonce = req
                        .extensions()
                        .get::<CspNonce>()
                        .expect("the nonce travels in the request extensions")
                        .0
                        .clone();
                    axum::Json(serde_json::json!({ "nonce": nonce }))
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                headers,
                security_headers_fn,
            ));

        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let csp = get(res.headers(), "content-security-policy")
            .expect("the CSP header is stamped")
            .to_string();
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let nonce = body["nonce"].as_str().unwrap();
        assert_eq!(
            csp,
            format!("script-src 'nonce-{nonce}'"),
            "the header and the route see the same nonce"
        );
    }

    #[test]
    fn max_age_is_configurable_and_include_subdomains_is_an_opt() {
        let headers = SecurityHeaders {
            hsts_max_age_secs: 7777,
            hsts_include_subdomains: true,
            ..baseline()
        };
        let mut res = Response::new(Body::empty());
        headers.apply(&mut res);
        assert_eq!(
            get(res.headers(), "strict-transport-security"),
            Some("max-age=7777; includeSubDomains")
        );
    }

    #[test]
    fn zero_max_age_and_empty_values_omit_their_headers() {
        let headers = SecurityHeaders {
            hsts_max_age_secs: 0,
            csp: String::new(),
            referrer_policy: String::new(),
            ..baseline()
        };
        let mut res = Response::new(Body::empty());
        headers.apply(&mut res);
        assert!(get(res.headers(), "strict-transport-security").is_none());
        assert!(get(res.headers(), "content-security-policy").is_none());
        assert!(get(res.headers(), "referrer-policy").is_none());
        assert_eq!(
            get(res.headers(), "x-content-type-options"),
            Some("nosniff")
        );
    }

    #[test]
    fn handler_set_headers_win_over_the_middleware() {
        let mut res = Response::new(Body::empty());
        res.headers_mut().insert(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static("default-src 'self' https://strict.example"),
        );
        baseline().apply(&mut res);
        assert_eq!(
            get(res.headers(), "content-security-policy"),
            Some("default-src 'self' https://strict.example"),
            "the handler's CSP is authoritative; the middleware only fills gaps"
        );
        assert_eq!(
            get(res.headers(), "x-content-type-options"),
            Some("nosniff")
        );
    }

    #[test]
    fn unparseable_config_values_are_skipped_not_panicking() {
        let headers = SecurityHeaders {
            csp: "default-src 'self'\r\nX-Injected: 1".into(),
            ..baseline()
        };
        let mut res = Response::new(Body::empty());
        headers.apply(&mut res);
        assert!(
            get(res.headers(), "content-security-policy").is_none(),
            "a value that cannot be a header value is skipped fail-closed"
        );
        assert!(get(res.headers(), "x-injected").is_none());
        assert_eq!(
            get(res.headers(), "x-content-type-options"),
            Some("nosniff")
        );
    }

    #[tokio::test]
    async fn the_middleware_fn_stamps_a_handler_response_end_to_end() {
        let app = axum::Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                baseline(),
                security_headers_fn,
            ));
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request serves");
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        assert_eq!(
            get(res.headers(), "x-content-type-options"),
            Some("nosniff")
        );
        assert_eq!(
            get(res.headers(), "strict-transport-security"),
            Some("max-age=31536000")
        );
    }
}
