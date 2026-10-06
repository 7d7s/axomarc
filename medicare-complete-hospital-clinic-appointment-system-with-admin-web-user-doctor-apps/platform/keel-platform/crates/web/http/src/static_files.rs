//! Wave 2 static assets (19-PARITY "Static assets"): path-traversal-safe
//! resolution of files under the application's `public/` directory.
//!
//! The router only claims a request here when the requested path resolves to
//! a real file INSIDE the canonicalized root:
//!
//! - the URI path is percent-decoded first, then every `.`/`..` segment is
//!   refused before it can touch the filesystem;
//! - the candidate must additionally canonicalize inside the canonicalized
//!   root, so symlink escapes and Windows `..\`/drive tricks fail closed;
//! - directories resolve through their `index.html` (the SPA shell at `/`);
//!   a directory without one is not a route and never lists;
//! - only `GET`/`HEAD` are static-file verbs — every other method, traversal
//!   attempt and miss returns `None`, which the edge renders as a 404 through
//!   the normal error path (never a file outside `public/`).
//!
//! Cache posture follows the fingerprint heuristic below: hashed build assets
//! are immutable for a year, everything else revalidates, so the shell and
//! favicons cannot pin a stale copy. File bytes are read at the edge (the
//! transport decides *what* to serve; the runtime does the I/O).

use std::path::{Path, PathBuf};

use axum::http::Method;

use crate::cache::CacheControlBuilder;

/// One resolved static file: what to serve and how to describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticAsset {
    /// Canonical path of the file — guaranteed inside the root.
    pub path: PathBuf,
    /// Byte length from metadata (drives `Content-Length`, HEAD included).
    pub len: u64,
    /// `Content-Type` for this extension.
    pub content_type: &'static str,
    /// `Cache-Control` for this file name.
    pub cache_control: String,
}

/// Path-traversal-safe `public/` resolver (see the module docs).
#[derive(Debug, Clone)]
pub struct StaticFiles {
    root: PathBuf,
}

impl StaticFiles {
    /// Serve files under `root`. A root that does not exist simply resolves
    /// nothing (every request misses), so boot never depends on the directory
    /// having been created yet.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve `uri_path` for `method`. `None` means "not a static-file
    /// answer": wrong verb, traversal, missing file, or a directory without
    /// an `index.html` — the edge turns each of those into a 404.
    pub fn resolve(&self, method: &Method, uri_path: &str) -> Option<StaticAsset> {
        if method != Method::GET && method != Method::HEAD {
            return None;
        }
        let relative = decode_relative(uri_path)?;
        // The root itself must canonicalize on every lookup: a missing or
        // removed `public/` is a miss, and everything below is compared
        // against this *real* path rather than the configured one.
        let root = std::fs::canonicalize(&self.root).ok()?;
        let mut resolved = std::fs::canonicalize(root.join(relative)).ok()?;
        if !resolved.starts_with(&root) {
            return None;
        }
        if resolved.is_dir() {
            resolved = std::fs::canonicalize(resolved.join("index.html")).ok()?;
            if !resolved.starts_with(&root) || !resolved.is_file() {
                return None;
            }
        }
        let metadata = std::fs::metadata(&resolved).ok()?;
        if !metadata.is_file() {
            return None;
        }
        let file_name = resolved.file_name()?.to_str()?.to_string();
        Some(StaticAsset {
            path: resolved,
            len: metadata.len(),
            content_type: content_type_of(&file_name),
            cache_control: cache_control_for(&file_name),
        })
    }
}

/// Percent-decode the URI path and turn it into safe relative components.
/// `None` on a bad escape, invalid UTF-8, or any `.`/`..` segment — traversal
/// is refused before the filesystem is consulted at all.
fn decode_relative(uri_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(uri_path)?;
    let trimmed = decoded.strip_prefix('/').unwrap_or(&decoded);
    let mut relative = PathBuf::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() {
            continue;
        }
        if segment == "." || segment == ".." {
            return None;
        }
        relative.push(segment);
    }
    Some(relative)
}

/// Percent-decode a URI path. An invalid escape or a non-UTF-8 result refuses
/// the request rather than guessing at a filename.
fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `Content-Type` for a `public/` file name. Zero-dep extension table:
/// unknown extensions stay binary rather than being sniffed.
fn content_type_of(file_name: &str) -> &'static str {
    let ext = file_name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Fingerprinted build assets are cached forever; everything else
/// revalidates on every use (a plain `no-cache`, since no validator is
/// emitted for a static file — the correct copy is fetched each time).
fn cache_control_for(file_name: &str) -> String {
    if is_fingerprinted(file_name) {
        CacheControlBuilder::new()
            .public()
            .max_age(31_536_000)
            .immutable()
            .build()
    } else {
        CacheControlBuilder::new().no_cache().build()
    }
}

/// True when the file name carries a content hash (Vite/Rollup style:
/// `app-B7f3k9Jh.css`, `main.a1b2c3d4.js`) — an at-least-8-character
/// alphanumeric run mixing letters and digits in the stem. Deliberately
/// conservative: an unlucky long word without a digit never pins a stale
/// copy, while every real hash does.
pub fn is_fingerprinted(file_name: &str) -> bool {
    let stem = file_name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(file_name);
    stem.split(['-', '.']).any(|part| {
        part.len() >= 8
            && part.bytes().all(|b| b.is_ascii_alphanumeric())
            && part.bytes().any(|b| b.is_ascii_digit())
            && part.bytes().any(|b| b.is_ascii_alphabetic())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, StaticFiles) {
        let dir = std::env::temp_dir().join(format!("keel-static-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(dir.join("assets")).expect("fixture dirs");
        std::fs::create_dir_all(dir.join("docs")).expect("fixture dirs");
        std::fs::write(dir.join("assets").join("app-B7f3k9Jh.css"), "body{}")
            .expect("css writes");
        std::fs::write(dir.join("index.html"), "<!doctype html><title>app</title>")
            .expect("shell writes");
        std::fs::write(dir.join("docs").join("guide.md"), "# guide").expect("doc writes");
        let files = StaticFiles::new(dir.clone());
        (dir, files)
    }

    #[test]
    fn serves_real_files_with_type_and_cache_posture() {
        let (dir, files) = fixture();
        let asset = files
            .resolve(&Method::GET, "/assets/app-B7f3k9Jh.css")
            .expect("fingerprinted asset resolves");
        assert!(asset.path.starts_with(std::fs::canonicalize(&dir).unwrap()));
        assert_eq!(asset.content_type, "text/css; charset=utf-8");
        assert_eq!(asset.cache_control, "public, max-age=31536000, immutable");
        assert_eq!(asset.len, 6);

        let plain = files
            .resolve(&Method::GET, "/docs/guide.md")
            .expect("doc resolves");
        assert_eq!(plain.content_type, "application/octet-stream");
        assert_eq!(plain.cache_control, "no-cache");

        // The SPA shell answers "/" through index.html.
        let shell = files.resolve(&Method::GET, "/").expect("shell resolves");
        assert_eq!(shell.content_type, "text/html; charset=utf-8");
        assert_eq!(shell.path.file_name().unwrap(), "index.html");
    }

    #[test]
    fn traversal_is_refused_before_the_filesystem() {
        let (dir, files) = fixture();
        // A real secret one level above the root, reachable only via `..`.
        let secret = dir.parent().expect("temp parent").join("keel-static-secret.txt");
        std::fs::write(&secret, "hunter2").expect("secret writes");

        for attempt in [
            "/../keel-static-secret.txt",
            "/..%2Fkeel-static-secret.txt",
            "/assets/../../keel-static-secret.txt",
            "/%2e%2e/keel-static-secret.txt",
            "/./../keel-static-secret.txt",
            "/..\\keel-static-secret.txt",
        ] {
            assert!(
                files.resolve(&Method::GET, attempt).is_none(),
                "traversal refused: {attempt}"
            );
        }
        std::fs::remove_file(secret).ok();
    }

    #[test]
    fn symlink_escapes_fail_closed() {
        let (dir, files) = fixture();
        let outside = dir.parent().expect("temp parent").join("keel-static-outside.txt");
        std::fs::write(&outside, "outside").expect("outside writes");
        let link = dir.join("escape.txt");
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_file(&outside, &link).is_ok();
        #[cfg(unix)]
        let created = std::os::unix::fs::symlink(&outside, &link).is_ok();
        if created {
            assert!(
                files.resolve(&Method::GET, "/escape.txt").is_none(),
                "symlink escape resolves outside the root"
            );
        }
        std::fs::remove_file(outside).ok();
    }

    #[test]
    fn wrong_verbs_misses_and_bare_directories_are_not_served() {
        let (dir, files) = fixture();
        std::fs::create_dir_all(dir.join("bare")).expect("bare dir");
        std::fs::create_dir_all(dir.join("shell-only")).expect("shell dir");
        std::fs::write(dir.join("shell-only").join("index.html"), "ok").expect("shell writes");

        assert!(files.resolve(&Method::POST, "/assets/app-B7f3k9Jh.css").is_none());
        assert!(files.resolve(&Method::PUT, "/index.html").is_none());
        assert!(files.resolve(&Method::GET, "/missing.css").is_none());
        assert!(files.resolve(&Method::GET, "/bare").is_none());
        assert!(files.resolve(&Method::GET, "/bare/").is_none());
        // A directory WITH an index.html still serves through it.
        assert!(files.resolve(&Method::GET, "/shell-only").is_some());
        // A malformed escape is a miss, never a guess.
        assert!(files.resolve(&Method::GET, "/100%.txt").is_none());
    }

    #[test]
    fn encoded_paths_resolve_after_decoding() {
        let (dir, files) = fixture();
        std::fs::write(dir.join("a b.txt"), "spaced").expect("spaced writes");
        assert!(files.resolve(&Method::GET, "/a%20b.txt").is_some());
        assert!(files.resolve(&Method::GET, "/a b.txt").is_some());
    }

    #[test]
    fn fingerprint_heuristic_matches_hashes_and_skips_plain_names() {
        for hashed in [
            "app-B7f3k9Jh.css",
            "main.a1b2c3d4.js",
            "chunk-0123abcd.svg",
            "index-7F3kQ9x2.html",
        ] {
            assert!(is_fingerprinted(hashed), "hashed: {hashed}");
        }
        for plain in [
            "index.html",
            "favicon.ico",
            "site.css",
            "robots.txt",
            "logo.svg",
            "someverylongword.js",
            "v2-styles.css",
        ] {
            assert!(!is_fingerprinted(plain), "plain: {plain}");
        }
    }

    #[test]
    fn content_type_table_covers_the_usual_build_outputs() {
        assert_eq!(content_type_of("app.js"), "text/javascript; charset=utf-8");
        assert_eq!(content_type_of("manifest.json"), "application/json; charset=utf-8");
        assert_eq!(content_type_of("site.webmanifest"), "application/manifest+json");
        assert_eq!(content_type_of("logo.svg"), "image/svg+xml");
        assert_eq!(content_type_of("font.woff2"), "font/woff2");
        assert_eq!(content_type_of("unknown.bin"), "application/octet-stream");
        assert_eq!(content_type_of("no-extension"), "application/octet-stream");
    }
}
