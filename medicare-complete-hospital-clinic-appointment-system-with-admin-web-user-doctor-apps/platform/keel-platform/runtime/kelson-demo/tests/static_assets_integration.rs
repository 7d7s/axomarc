//! Wave 2 static assets edge integration: `public/` answers real files
//! through the router fallback (immutable headers for fingerprinted build
//! assets, `index.html` at `/`), declared routes keep winning over files,
//! traversal attempts and unknown paths render the not-found path, and
//! `@asset("...")` resolves through the Vite build manifest. Fixtures live
//! under `<temp>/public` + `<temp>/views` (see `lib.rs::build_app_state`).

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

/// One web-plane HTML route whose view exercises the asset helper.
const EXTRA_ROUTES: &str = r#"
[[routes]]
method = "GET"
path = "/web/assets"
operation = "availability"
capability = "availability"
plane = "web"
response = "html"
view = "pages.assets"
public = true
"#;

const ASSETS_VIEW: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Assets</title>
  <link rel="stylesheet" href="@asset("resources/css/app.css")">
  <script type="module" src="@asset("resources/js/app.js")"></script>
  <link rel="icon" href="@asset("favicon.ico")">
  <img src='@asset("logo.svg?x=1&y=2")'>
</head>
<body class="assets">shell</body>
</html>
"#;

const ERROR_404_VIEW: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>{{ status }} {{ title }}</title></head>
<body>
  <main class="error-404">
    <h1>{{ status }}</h1>
    <p class="title">{{ title }}</p>
    <p class="message">{{ message }}</p>
    <p class="path">{{ path }}</p>
  </main>
</body>
</html>
"#;

const CSS: &str = "body { color: #333; }\n";
const MANIFEST: &str = r#"{
  "resources/css/app.css": {
    "file": "assets/app-B7f3k9Jh.css",
    "src": "resources/css/app.css",
    "isEntry": true
  },
  "resources/js/app.js": {
    "file": "assets/app-9xK2pQ4r.js",
    "src": "resources/js/app.js",
    "isEntry": true,
    "css": ["assets/app-B7f3k9Jh.css"]
  }
}"#;

struct Fixture {
    state: kelson_demo::routes::AppState,
    graph: kelson_registry::PlatformGraph,
}

/// Boot the harness with a real `public/` tree, fixture templates and (when
/// `with_manifest` is set) a Vite build manifest under `public/build/`.
fn fixture(with_manifest: bool) -> Fixture {
    let dir = std::env::temp_dir().join(format!("kelson-static-it-{}", uuid::Uuid::now_v7()));
    let public = dir.join("public");
    std::fs::create_dir_all(public.join("assets")).expect("public dirs");
    std::fs::create_dir_all(public.join("css")).expect("public dirs");
    std::fs::create_dir_all(public.join("build")).expect("public dirs");
    std::fs::create_dir_all(dir.join("views").join("pages")).expect("views dirs");
    std::fs::create_dir_all(dir.join("views").join("errors")).expect("views dirs");

    std::fs::write(public.join("assets").join("app-B7f3k9Jh.css"), CSS).expect("css writes");
    std::fs::write(public.join("css").join("site.css"), "html{zoom:1}").expect("css writes");
    std::fs::write(
        public.join("index.html"),
        "<!doctype html><title>app shell</title>",
    )
    .expect("shell writes");
    std::fs::write(public.join("favicon.ico"), b"\x00\x00icon").expect("icon writes");
    if with_manifest {
        std::fs::write(public.join("build").join("manifest.json"), MANIFEST)
            .expect("manifest writes");
    }
    // Files that collide with declared routes: the routes must still win.
    std::fs::write(public.join("healthz"), "FILE-AT-HEALTHZ").expect("decoy writes");
    std::fs::create_dir_all(public.join("web")).expect("decoy dirs");
    std::fs::write(public.join("web").join("assets"), "FILE-AT-WEB-ASSETS").expect("decoy writes");
    // A secret one level ABOVE the root: only reachable via traversal.
    std::fs::write(
        dir.parent()
            .expect("temp parent")
            .join("kelson-static-it-secret.txt"),
        "hunter2",
    )
    .expect("secret writes");

    std::fs::write(
        dir.join("views").join("pages").join("assets.html"),
        ASSETS_VIEW,
    )
    .expect("view writes");
    std::fs::write(
        dir.join("views").join("errors").join("404.html"),
        ERROR_404_VIEW,
    )
    .expect("error view writes");

    let (state, _, _) = kelson_demo::build_app_state(&dir, true, true);
    let raw = format!(
        "{}{EXTRA_ROUTES}",
        include_str!("../../../applications/appointments/module.toml")
    );
    let manifest =
        kelson_registry::ModuleManifest::from_toml(&raw).expect("fixture manifest parses");
    let graph =
        kelson_registry::PlatformGraph::compile(vec![manifest]).expect("fixture graph compiles");
    Fixture { state, graph }
}

fn router(fx: &Fixture) -> axum::Router {
    let state = fx.state.clone();
    let router = kelson_demo::routes::router(state.clone())
        .merge(kelson_demo::graph_routes::graph_router(
            state.clone(),
            &fx.graph,
        ))
        .merge(kelson_demo::auth_routes::router(state.clone()));
    let router = state
        .sessions
        .as_ref()
        .expect("sessions enabled")
        .as_ref()
        .clone()
        .apply(router);
    // Production wiring: the fallback is attached LAST, after every route.
    kelson_demo::static_assets::with_static_files(router, &state)
}

struct Reply {
    status: StatusCode,
    content_type: String,
    cache_control: String,
    content_length: Option<String>,
    body: Vec<u8>,
}

impl Reply {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

async fn send(router: &axum::Router, req: Request<Body>) -> Reply {
    let res = router.clone().oneshot(req).await.expect("request serves");
    let hdr = |name: header::HeaderName| {
        res.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let status = res.status();
    let content_type = hdr(header::CONTENT_TYPE);
    let cache_control = hdr(header::CACHE_CONTROL);
    let content_length = res
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body reads");
    Reply {
        status,
        content_type,
        cache_control,
        content_length,
        body: body.to_vec(),
    }
}

async fn request(router: &axum::Router, method: Method, uri: &str, accept: Option<&str>) -> Reply {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(accept) = accept {
        builder = builder.header(header::ACCEPT, accept);
    }
    send(router, builder.body(Body::empty()).expect("request builds")).await
}

async fn get(router: &axum::Router, uri: &str) -> Reply {
    request(router, Method::GET, uri, None).await
}

// ------------------------------------------------------------ file serving

#[tokio::test]
async fn public_files_serve_with_their_type_and_cache_posture() {
    let fx = fixture(true);
    let router = router(&fx);

    let hashed = get(&router, "/assets/app-B7f3k9Jh.css").await;
    assert_eq!(hashed.status, StatusCode::OK, "{}", hashed.text());
    assert_eq!(hashed.content_type, "text/css; charset=utf-8");
    assert_eq!(
        hashed.cache_control, "public, max-age=31536000, immutable",
        "fingerprinted asset is cacheable forever"
    );
    assert_eq!(hashed.text(), CSS);

    let plain = get(&router, "/css/site.css").await;
    assert_eq!(plain.status, StatusCode::OK, "{}", plain.text());
    assert_eq!(plain.cache_control, "no-cache");
    assert_eq!(plain.text(), "html{zoom:1}");

    // The SPA shell answers "/" through public/index.html.
    let shell = get(&router, "/").await;
    assert_eq!(shell.status, StatusCode::OK, "{}", shell.text());
    assert!(
        shell.content_type.starts_with("text/html"),
        "{}",
        shell.content_type
    );
    assert!(shell.text().contains("app shell"), "{}", shell.text());
}

#[tokio::test]
async fn declared_routes_still_win_over_files_with_the_same_path() {
    let fx = fixture(true);
    let router = router(&fx);

    // A file at /healthz must not shadow the platform route.
    let health = get(&router, "/healthz").await;
    assert_eq!(health.status, StatusCode::OK, "{}", health.text());
    assert_eq!(health.json()["data"]["ok"], true, "{}", health.text());
    assert!(
        !health.text().contains("FILE-AT-HEALTHZ"),
        "the file did not shadow the route: {}",
        health.text()
    );

    // Nor may one shadow a graph route: /web/assets still renders the view.
    let page = get(&router, "/web/assets").await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text());
    assert!(
        page.content_type.starts_with("text/html"),
        "{}",
        page.content_type
    );
    assert!(page.text().contains("class=\"assets\""), "{}", page.text());
    assert!(!page.text().contains("FILE-AT-WEB-ASSETS"));
}

#[tokio::test]
async fn head_reports_the_entity_length_without_a_body() {
    let fx = fixture(true);
    let router = router(&fx);

    let head = request(&router, Method::HEAD, "/assets/app-B7f3k9Jh.css", None).await;
    assert_eq!(head.status, StatusCode::OK, "{}", head.text());
    assert_eq!(
        head.content_length.as_deref(),
        Some(CSS.len().to_string().as_str())
    );
    assert_eq!(
        head.cache_control, "public, max-age=31536000, immutable",
        "HEAD carries the same cache posture"
    );
    assert!(head.body.is_empty(), "HEAD must not ship a body");

    // The GET twin proves the length header matches the real entity.
    let get = get(&router, "/assets/app-B7f3k9Jh.css").await;
    assert_eq!(
        get.content_length.as_deref(),
        Some(CSS.len().to_string().as_str())
    );
    assert_eq!(get.body.len(), CSS.len());
}

// ------------------------------------------------------- not-found rendering

#[tokio::test]
async fn traversal_attempts_and_unknown_paths_render_the_not_found_view() {
    let fx = fixture(true);
    let router = router(&fx);

    for attempt in [
        "/../kelson-static-it-secret.txt",
        "/..%2Fkelson-static-it-secret.txt",
        "/%2e%2e/kelson-static-it-secret.txt",
        "/missing.txt",
        "/no/such/place",
    ] {
        let reply = get(&router, attempt).await;
        assert_eq!(
            reply.status,
            StatusCode::NOT_FOUND,
            "{attempt}: {}",
            reply.text()
        );
        assert!(
            reply.content_type.starts_with("text/html"),
            "{attempt}: browser gets the error view, not bare text"
        );
        assert!(
            reply.text().contains(r#"class="error-404""#),
            "{attempt}: {}",
            reply.text()
        );
        assert!(
            !reply.text().contains("hunter2"),
            "{attempt} leaked the file above the root"
        );
    }

    // A machine client on the same miss keeps the Envelope shape.
    let machine = request(
        &router,
        Method::GET,
        "/missing.txt",
        Some("application/json"),
    )
    .await;
    assert_eq!(machine.status, StatusCode::NOT_FOUND);
    assert_eq!(machine.json()["error"]["code"], "KSN_NOT_FOUND");

    // A wrong verb against a real file is a route miss too, never a serve.
    let post = request(&router, Method::POST, "/css/site.css", None).await;
    assert_eq!(post.status, StatusCode::NOT_FOUND, "{}", post.text());
}

// ----------------------------------------------------------- @asset helper

#[tokio::test]
async fn asset_directive_resolves_through_the_vite_manifest() {
    let fx = fixture(true);
    let router = router(&fx);

    let page = get(&router, "/web/assets").await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text());
    let html = page.text();
    // Source keys map to their hashed build artifacts...
    assert!(
        html.contains(r#"href="/assets/app-B7f3k9Jh.css""#),
        "manifest css resolved:\n{html}"
    );
    assert!(
        html.contains(r#"src="/assets/app-9xK2pQ4r.js""#),
        "manifest js resolved:\n{html}"
    );
    // ...keys the manifest does not know stay literal public/ URLs...
    assert!(
        html.contains(r#"href="/favicon.ico""#),
        "literal asset resolved:\n{html}"
    );
    // ...and the URL leaves the template HTML-escaped.
    assert!(
        html.contains("/logo.svg?x=1&amp;y=2"),
        "helper output is escaped:\n{html}"
    );
    // The raw manifest source key must never leak into the page.
    assert!(!html.contains("resources/css/app.css"), "{html}");
}

#[tokio::test]
async fn without_a_build_manifest_the_helper_emits_literal_urls() {
    let fx = fixture(false);
    assert!(
        fx.state.assets.is_none(),
        "no manifest on disk means no manifest at boot"
    );
    let router = router(&fx);

    let page = get(&router, "/web/assets").await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text());
    let html = page.text();
    assert!(
        html.contains(r#"href="/resources/css/app.css""#),
        "degraded helper falls back to the literal path:\n{html}"
    );
    // Serving still works without a build: files are independent of it.
    assert_eq!(get(&router, "/css/site.css").await.status, StatusCode::OK);
}
