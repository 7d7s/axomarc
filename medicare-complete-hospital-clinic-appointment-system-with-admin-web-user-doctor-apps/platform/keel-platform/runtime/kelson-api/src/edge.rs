//! Wave 2 edge middleware (19-PARITY "Request/response layer"): the two
//! concerns that run *after* a handler produced its response — queued-cookie
//! flushing and terminating (post-response) hooks.
//!
//! Both are applied through one layer ([`edge_mw`]) so every route sees them
//! exactly once in a fixed order: cookies first (transport), then hooks
//! (observation). Neither can mutate the response the handler chose. The
//! layer lives here rather than in `kelson-http` because dropping a cookie or
//! a hook that panics is an observation concern — the web crate stays free
//! of logging.

use std::sync::{Arc, RwLock};
use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::header::HeaderValue;
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;
use kelson_http::{CookieCipher, CookieQueue};

/// Summary handed to terminating hooks (Laravel `terminating()`). The
/// response is already produced: hooks are for metrics, audit trails and
/// span close — never for changing what the client received.
#[derive(Debug, Clone)]
pub struct Termination {
    pub method: String,
    /// Path only (no query string): hooks must not capture identifiers.
    pub path: String,
    pub status: u16,
    pub elapsed_ms: u64,
}

/// One registered callback. Aliased so the registry and its snapshots stay
/// readable — the trait object is the same shape everywhere.
type Hook = Arc<dyn Fn(&Termination) + Send + Sync>;

/// Post-response callbacks. Registered by module providers at boot
/// (`Provider::init`), run once per request after the response is built.
/// `&self` registration so a shared `Arc` suffices.
pub struct TerminatingHooks {
    hooks: RwLock<Vec<Hook>>,
}

impl TerminatingHooks {
    pub fn new() -> Self {
        Self {
            hooks: RwLock::new(Vec::new()),
        }
    }

    /// Register a callback; it runs after every subsequent response.
    pub fn add(&self, hook: impl Fn(&Termination) + Send + Sync + 'static) {
        if let Ok(mut hooks) = self.hooks.write() {
            hooks.push(Arc::new(hook));
        }
    }

    /// Run every registered callback with the completed request summary. A
    /// panicking hook is caught per-hook so observation never disturbs the
    /// response path.
    pub fn run(&self, meta: &Termination) {
        let snapshot: Vec<Hook> = match self.hooks.read() {
            Ok(hooks) => hooks.clone(),
            Err(_) => return,
        };
        for hook in snapshot {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(meta)));
            if result.is_err() {
                tracing::warn!(target: "kelson::terminating", "terminating hook panicked");
            }
        }
    }

    pub fn len(&self) -> usize {
        self.hooks.read().map(|h| h.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for TerminatingHooks {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-host edge configuration captured when the layer is built: the
/// optional cookie cipher and the shared terminating-hook registry.
#[derive(Clone)]
pub struct EdgeConfig {
    /// `Some` when `[http] encrypt_cookies` is on.
    pub cipher: Option<Arc<CookieCipher>>,
    pub hooks: Arc<TerminatingHooks>,
}

impl EdgeConfig {
    pub fn new(hooks: Arc<TerminatingHooks>) -> Self {
        Self {
            cipher: None,
            hooks,
        }
    }

    pub fn with_cipher(mut self, cipher: Option<Arc<CookieCipher>>) -> Self {
        self.cipher = cipher;
        self
    }
}

/// The edge layer: installs a [`CookieQueue`] for the request, drains it
/// into `Set-Cookie` after the handler responded (sealing values when the
/// host enables encryption), then runs the terminating hooks.
pub async fn edge_mw(State(cfg): State<EdgeConfig>, req: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();

    let queue = CookieQueue::default();
    let (mut parts, body) = req.into_parts();
    parts.extensions.insert(queue.clone());
    let mut response = next.run(Request::from_parts(parts, body)).await;

    flush_cookies(&cfg, &queue, &mut response);

    cfg.hooks.run(&Termination {
        method,
        path,
        status: response.status().as_u16(),
        elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
    });
    response
}

/// Emit queued cookies as `Set-Cookie` headers, sealing each value when a
/// cipher is configured. A cookie that cannot be sealed or rendered is
/// dropped with a warning — it never fails the response.
fn flush_cookies(cfg: &EdgeConfig, queue: &CookieQueue, response: &mut Response) {
    for cookie in queue.drain() {
        let value = match &cfg.cipher {
            None => cookie.value.clone(),
            Some(cipher) => match cipher.seal(&cookie.value) {
                Ok(sealed) => sealed,
                Err(error) => {
                    tracing::warn!(
                        target: "kelson::cookies",
                        cookie = %cookie.name,
                        error = %error.public_message,
                        "queued cookie could not be sealed; dropped"
                    );
                    continue;
                }
            },
        };
        let mut sealed = cookie;
        sealed.value = value;
        let rendered = sealed.render().ok().and_then(|raw| HeaderValue::from_str(&raw).ok());
        let Some(header_value) = rendered else {
            tracing::warn!(
                target: "kelson::cookies",
                cookie = %sealed.name,
                "queued cookie could not be rendered; dropped"
            );
            continue;
        };
        response
            .headers_mut()
            .append(header::SET_COOKIE, header_value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, StatusCode};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    /// Deterministic stand-in for the AES-GCM sealer (the security class
    /// stays out of the web layer's test build).
    struct Echo;

    impl kelson_foundation::Sealer for Echo {
        fn seal(
            &self,
            key_id: &str,
            secret: &kelson_foundation::Secret,
        ) -> Result<kelson_foundation::Envelope, kelson_foundation::AppError> {
            Ok(kelson_foundation::Envelope::new(
                "TEST",
                key_id,
                vec![0u8; 12],
                format!("sealed:{}", secret.expose()).into_bytes(),
            ))
        }

        fn open(
            &self,
            envelope: &kelson_foundation::Envelope,
        ) -> Result<kelson_foundation::Secret, kelson_foundation::AppError> {
            let text = String::from_utf8(envelope.ciphertext.clone())
                .map_err(|_| kelson_foundation::AppError::validation("bad ciphertext"))?;
            Ok(kelson_foundation::Secret::new(text))
        }
    }

    /// Router: one GET route that queues a cookie inside the handler (the
    /// queue reaches handlers as a request extension installed by the layer).
    fn cookie_router(cfg: EdgeConfig) -> axum::Router {
        axum::Router::new()
            .route(
                "/queue",
                axum::routing::get(|req: Request| async move {
                    let queue = req
                        .extensions()
                        .get::<CookieQueue>()
                        .expect("edge installs the queue");
                    queue.queue(kelson_http::Cookie::new("pref", "evening").expect("cookie"));
                    StatusCode::NO_CONTENT
                }),
            )
            .layer(axum::middleware::from_fn_with_state(cfg, edge_mw))
    }

    async fn get(router: axum::Router, uri: &str) -> Response {
        router
            .oneshot(
                axum::http::Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("request serves")
    }

    #[test]
    fn hooks_see_every_registration_and_survive_a_panicking_hook() {
        let hooks = TerminatingHooks::new();
        assert!(hooks.is_empty());
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        hooks.add(move |meta| {
            assert_eq!(meta.status, 200);
            counter.fetch_add(1, Ordering::SeqCst);
        });
        hooks.add(|_| panic!("observation must not take down the edge"));
        assert_eq!(hooks.len(), 2);

        hooks.run(&Termination {
            method: "GET".into(),
            path: "/healthz".into(),
            status: 200,
            elapsed_ms: 1,
        });
        assert_eq!(seen.load(Ordering::SeqCst), 1, "first hook still ran");
    }

    #[tokio::test]
    async fn edge_layer_flushes_queued_cookies_then_runs_hooks() {
        let hooks = Arc::new(TerminatingHooks::new());
        let fired = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&fired);
        let seen_status = Arc::new(std::sync::Mutex::new(0u16));
        let status_slot = Arc::clone(&seen_status);
        hooks.add(move |meta| {
            *status_slot.lock().expect("status slot") = meta.status;
            counter.fetch_add(1, Ordering::SeqCst);
        });

        let response = get(cookie_router(EdgeConfig::new(Arc::clone(&hooks))), "/queue").await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let cookies: Vec<String> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(str::to_string)
            .collect();
        assert_eq!(cookies.len(), 1, "{cookies:?}");
        assert!(cookies[0].starts_with("pref=evening"), "{cookies:?}");
        assert_eq!(fired.load(Ordering::SeqCst), 1, "hook ran once");
        assert_eq!(
            *seen_status.lock().expect("status slot"),
            204,
            "hooks observe the final status"
        );
    }

    #[tokio::test]
    async fn edge_layer_seals_values_when_a_cipher_is_configured() {
        let cfg = EdgeConfig::new(Arc::new(TerminatingHooks::new())).with_cipher(Some(Arc::new(
            CookieCipher::new(Arc::new(Echo), "cookies-2026-01"),
        )));
        let response = get(cookie_router(cfg), "/queue").await;

        let raw = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("one Set-Cookie")
            .to_string();
        assert!(!raw.contains("evening"), "encryption hides it: {raw}");
        let value = raw
            .split('=')
            .nth(1)
            .expect("value pair")
            .split(';')
            .next()
            .expect("value");
        assert!(value.starts_with("k1."), "{value}");
    }

    /// Wave 7 P7 (19-PARITY "cookie encryption roundtrip"): with
    /// `[http] encrypt_cookies` on, the PRODUCTION edge seals a queued
    /// cookie with the real AES-GCM sealer under the app-key-derived
    /// cookie key — and a cipher derived from the same app key (what the
    /// read side / a second boot holds) opens the wire value back to the
    /// plaintext the handler queued. Another app key refuses; the toggle
    /// off (the default) keeps shipping values in the clear, pinned by
    /// `edge_layer_flushes_queued_cookies_then_runs_hooks` above.
    #[tokio::test]
    async fn edge_layer_roundtrips_a_queued_cookie_under_the_real_cipher() {
        let cipher = crate::cookie_cipher(true, "test-app-key-0123456789")
            .expect("derives")
            .expect("enabled yields a cipher");
        let cfg = EdgeConfig::new(Arc::new(TerminatingHooks::new()))
            .with_cipher(Some(Arc::clone(&cipher)));
        let response = get(cookie_router(cfg), "/queue").await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let raw = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("one Set-Cookie")
            .to_string();
        assert!(!raw.contains("evening"), "plaintext never reaches the wire: {raw}");
        let value = raw
            .split('=')
            .nth(1)
            .expect("value pair")
            .split(';')
            .next()
            .expect("value")
            .to_string();
        assert!(value.starts_with("k1."), "{value}");

        // The roundtrip: the wire value opens back to what the handler queued.
        assert_eq!(
            cipher.open(&value).expect("same key opens"),
            "evening",
            "seal → open restores the plaintext"
        );
        // A separate boot on the same app key derives the same cookie key,
        // so it opens the first boot's cookie...
        let reader = crate::cookie_cipher(true, "test-app-key-0123456789")
            .expect("derives")
            .expect("enabled yields a cipher");
        assert_eq!(reader.open(&value).expect("same key opens"), "evening");
        // ...while another deployment's key refuses (key separation).
        let foreign = crate::cookie_cipher(true, "other-app-key-9876543210")
            .expect("derives")
            .expect("enabled yields a cipher");
        assert!(
            foreign.open(&value).is_err(),
            "a foreign app key cannot open the sealed value"
        );
    }
}
