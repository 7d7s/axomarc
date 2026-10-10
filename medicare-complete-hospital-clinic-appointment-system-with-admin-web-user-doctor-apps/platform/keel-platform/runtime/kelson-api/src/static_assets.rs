//! Wave 2 static assets edge: the `public/` fallback that answers real files
//! only after every declared route has had its chance, and turns everything
//! else — traversal attempts included — into the normal not-found render.
//!
//! Ordering matters: this is a *router fallback*, so a path the graph
//! declares (platform, module, auth, telemetry) can never be shadowed by a
//! file that happens to share its name; the resolver in `kelson-http` then
//! decides whether the request is a file at all.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;

use crate::routes::AppState;

/// Attach the `public/` file fallback to the assembled router. A served file
/// carries its type and cache posture; a miss (wrong verb, traversal, no
/// file) renders through `graph_routes::not_found_response` — the web-plane
/// `errors/404` view for browsers, the Envelope/problem for machine clients.
///
/// The router is returned unchanged when the boot wired no static root, so
/// harnesses that never opt in keep axum's unmatched behaviour.
pub fn with_static_files(router: Router, state: &AppState) -> Router {
    let Some(files) = state.static_files.clone() else {
        return router;
    };
    let render = state.clone();
    router.fallback(move |req: Request<Body>| {
        let files = Arc::clone(&files);
        let render = render.clone();
        async move {
            let (parts, _body) = req.into_parts();
            let uri_path = parts.uri.path();
            let Some(asset) = files.resolve(&parts.method, uri_path) else {
                return crate::graph_routes::not_found_response(&render, &parts.headers, uri_path);
            };
            let content_type = asset.content_type;
            let cache_control = asset.cache_control;
            let content_length = asset.len;
            let head = parts.method == Method::HEAD;
            // I/O happens on the blocking pool: a large asset must never park
            // an async worker for the duration of the read.
            let bytes = if head {
                Vec::new()
            } else {
                let path = asset.path;
                match tokio::task::spawn_blocking(move || std::fs::read(path)).await {
                    Ok(Ok(bytes)) => bytes,
                    // The file vanished (or turned unreadable) between the
                    // resolve and the read: a miss, not a 500.
                    Ok(Err(error)) => {
                        tracing::warn!(%error, "static asset unreadable; rendering not-found");
                        return crate::graph_routes::not_found_response(
                            &render,
                            &parts.headers,
                            uri_path,
                        );
                    }
                    Err(error) => {
                        tracing::warn!(%error, "static asset read task failed; rendering not-found");
                        return crate::graph_routes::not_found_response(
                            &render,
                            &parts.headers,
                            uri_path,
                        );
                    }
                }
            };
            let mut response = Response::new(if head {
                Body::empty()
            } else {
                Body::from(bytes)
            });
            *response.status_mut() = StatusCode::OK;
            let headers = response.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_str(&cache_control).unwrap_or_else(|_| HeaderValue::from_static("no-cache")),
            );
            // Set explicitly so HEAD reports the entity length instead of
            // the empty body's zero.
            headers.insert(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&content_length.to_string())
                    .expect("content length is always a valid header value"),
            );
            response
        }
    })
}
