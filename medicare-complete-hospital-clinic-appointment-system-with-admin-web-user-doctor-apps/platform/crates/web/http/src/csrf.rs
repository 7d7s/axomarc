//! Opt-in CSRF protection (blueprint L26, S3). Double-submit-cookie pattern:
//! the server issues a random token as a cookie; state-changing requests must
//! echo it in the `X-CSRF-TOKEN` header. A cross-origin attacker can make the
//! browser SEND the cookie but cannot read it to set the header.
//!
//! Opt-in by construction: `CsrfGuard::layer()` is attached by the runtime
//! only when the app enables it; the fixed Admission pipeline never includes
//! it implicitly. Session-bound tokens (stronger, revocable with the
//! session) are available through keel-session `Session::csrf_token`.

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Method, Request, Response, StatusCode};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::Json;
use keel_foundation::AppError;
use rand::RngCore;

use crate::{Envelope, ErrorBody};

/// Cookie carrying the double-submit token. NOT HttpOnly: browser JS must
/// read it to set the header (that is the whole point of double-submit).
pub const CSRF_COOKIE: &str = "keel_csrf";
/// Header state-changing requests must echo the token in.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// Opt-in CSRF posture.
#[derive(Debug, Clone, Default)]
pub struct CsrfGuard {
    /// Paths that skip CSRF (webhook receivers signed by provider HMAC, etc.).
    pub bypass_paths: Vec<String>,
}

impl CsrfGuard {
    /// Wrap a router with the CSRF check (attach AFTER Admission so the
    /// request-id/trace envelope context exists when rejections render).
    pub fn apply(self, router: axum::Router) -> axum::Router {
        router.layer(axum::middleware::from_fn_with_state(self, csrf_fn))
    }
}

/// Session-bound CSRF token (stronger than double-submit: revocable with
/// the session, forged only with the server secret). Delegate to
/// keel-session, kept here so web edges have one import.
pub fn session_csrf_token(session: &keel_session::Session, secret: &[u8]) -> String {
    session.csrf_token(secret)
}

/// 256-bit random token, hex-encoded.
pub fn issue_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    const NIBBLES: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push(NIBBLES[(b >> 4) as usize] as char);
        out.push(NIBBLES[(b & 0xf) as usize] as char);
    }
    out
}

fn cookie_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers.get_all(header::COOKIE).iter().find_map(|value| {
        value.to_str().ok()?.split(';').find_map(|pair| {
            let (name, val) = pair.split_once('=')?;
            if name.trim() == CSRF_COOKIE {
                Some(val.trim().to_string())
            } else {
                None
            }
        })
    })
}

fn method_is_safe(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

pub(crate) async fn csrf_fn(
    State(guard): State<CsrfGuard>,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    if method_is_safe(req.method()) {
        return next.run(req).await;
    }
    if guard.bypass_paths.iter().any(|p| req.uri().path() == p) {
        return next.run(req).await;
    }
    let cookie = cookie_token(req.headers());
    let header_token = req
        .headers()
        .get(header::HeaderName::from_static(CSRF_HEADER))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string());
    let ok = matches!((cookie, header_token), (Some(c), Some(h)) if !c.is_empty()
        && crate::signed::constant_time_eq(c.as_bytes(), h.as_bytes()));
    if !ok {
        let err = AppError::denied("CSRF token missing or mismatched");
        let body = Envelope::<serde_json::Value> {
            ok: false,
            data: None,
            error: Some(ErrorBody::from_app_error(&err)),
            meta: None,
        };
        return (StatusCode::FORBIDDEN, Json(body)).into_response();
    }
    let mut res = next.run(req).await;
    ensure_cookie(&mut res);
    res
}

/// Set/refresh the CSRF cookie on responses that passed the check (keeps the
/// token fresh without an extra endpoint).
fn ensure_cookie(res: &mut Response<Body>) {
    if res.headers().contains_key(header::SET_COOKIE) {
        return;
    }
    let token = issue_token();
    let cookie = format!("{CSRF_COOKIE}={token}; Path=/; SameSite=Strict");
    if let Ok(value) = cookie.parse() {
        res.headers_mut().insert(header::SET_COOKIE, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use axum::Router;
    use tower::ServiceExt;

    fn app() -> Router {
        let router = Router::new()
            .route("/submit", post(|| async { "accepted" }))
            .route("/read", get(|| async { "read" }))
            .route("/hook", post(|| async { "hooked" }));
        CsrfGuard {
            bypass_paths: vec!["/hook".into()],
        }
        .apply(router)
    }

    fn post_req(uri: &str, cookie: Option<&str>, header: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method(Method::POST).uri(uri);
        if let Some(c) = cookie {
            builder = builder.header(header::COOKIE, format!("{CSRF_COOKIE}={c}"));
        }
        if let Some(h) = header {
            builder = builder.header(CSRF_HEADER, h);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn safe_methods_skip_csrf() {
        let res = app()
            .oneshot(Request::builder().uri("/read").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_token_rejected() {
        let res = app()
            .oneshot(post_req("/submit", None, None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn mismatched_token_rejected() {
        let res = app()
            .oneshot(post_req("/submit", Some("aaa"), Some("bbb")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn matching_double_submit_passes_and_refreshes_cookie() {
        let res = app()
            .oneshot(post_req("/submit", Some("tok123"), Some("tok123")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let set = res
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            set.starts_with("keel_csrf="),
            "refresh cookie issued: {set}"
        );
        assert!(set.contains("SameSite=Strict"));
    }

    #[tokio::test]
    async fn bypass_paths_skip_csrf() {
        let res = app().oneshot(post_req("/hook", None, None)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[test]
    fn tokens_are_256bit_and_unique() {
        let a = issue_token();
        let b = issue_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }
}
