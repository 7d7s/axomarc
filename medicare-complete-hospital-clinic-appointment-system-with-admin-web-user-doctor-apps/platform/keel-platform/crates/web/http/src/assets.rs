//! Wave 2 asset URL helper (19-PARITY static assets): a zero-dep reader for
//! the Vite build manifest (`public/build/manifest.json`) plus the
//! root-absolute URL helper templates call through `@asset("...")`.
//!
//! The manifest maps a *source* key (`resources/css/app.css`) to the hashed
//! artifact the build emitted (`assets/app-B7f3k9Jh.css`), which is what lets
//! a fingerprinted URL be generated from the same name the developer wrote —
//! Laravel's `asset()`/`@vite` helper pair, without a build plugin.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

/// One manifest entry: `file` is the built artifact, `css` the stylesheets
/// the entry pulls in. Extra Vite keys (`imports`, `dynamicImports`,
/// `assets`, `sourcemap`) are ignored on purpose — the helper answers URLs,
/// not a module graph, and unknown fields never fail the parse.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AssetEntry {
    pub file: String,
    #[serde(default)]
    pub css: Vec<String>,
    #[serde(default, rename = "isEntry")]
    pub is_entry: bool,
    #[serde(default)]
    pub src: Option<String>,
}

/// The parsed build manifest: source key -> entry.
#[derive(Debug, Clone, Default)]
pub struct AssetManifest {
    entries: BTreeMap<String, AssetEntry>,
}

impl AssetManifest {
    /// Parse manifest JSON. A malformed build artifact is an error the boot
    /// logs and degrades from (literal `@asset` URLs) — never a silently
    /// empty manifest that ships wrong links.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let entries: BTreeMap<String, AssetEntry> = serde_json::from_str(raw)
            .map_err(|error| format!("asset manifest is not valid JSON: {error}"))?;
        Ok(Self { entries })
    }

    /// Read + parse a manifest file (Vite 4 writes `build/manifest.json`,
    /// Vite 5+ nests it at `build/.vite/manifest.json`; the caller picks).
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Self::parse(&raw)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entry(&self, key: &str) -> Option<&AssetEntry> {
        self.entries.get(key)
    }

    /// Stylesheets an entry pulls in, as root-absolute URLs (empty when the
    /// key is unknown or carries no CSS).
    pub fn styles(&self, key: &str) -> Vec<String> {
        self.entries
            .get(key)
            .map(|entry| entry.css.iter().map(|css| asset_url(css)).collect())
            .unwrap_or_default()
    }

    /// Root-absolute URL: a known manifest key resolves to its built file,
    /// any other key is treated as a literal `public/` path (the `asset()`
    /// half of the Laravel helper pair).
    pub fn url(&self, key: &str) -> String {
        match self.entries.get(key) {
            Some(entry) => asset_url(&entry.file),
            None => asset_url(key),
        }
    }
}

/// Root-absolute URL for a `public/` path: `css/app.css` -> `/css/app.css`,
/// `/css/app.css` stays rooted, and `.`/`..` segments are dropped so a bad
/// template or manifest key can never emit `/../`.
pub fn asset_url(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    let mut url = String::with_capacity(trimmed.len() + 1);
    url.push('/');
    let mut first = true;
    for segment in trimmed.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            continue;
        }
        if !first {
            url.push('/');
        }
        url.push_str(segment);
        first = false;
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    const VITE_MANIFEST: &str = r#"{
      "resources/css/app.css": {
        "file": "assets/app-B7f3k9Jh.css",
        "src": "resources/css/app.css",
        "isEntry": true
      },
      "resources/js/app.js": {
        "file": "assets/app-9xK2pQ4r.js",
        "src": "resources/js/app.js",
        "isEntry": true,
        "css": ["assets/app-B7f3k9Jh.css"],
        "imports": ["_shared-aBc123xy.js"]
      },
      "_shared-aBc123xy.js": {
        "file": "assets/shared-aBc123xy.js",
        "dynamicImports": []
      },
      "public/logo.svg": {
        "file": "assets/logo-Zq7wE3.png",
        "src": "public/logo.svg"
      }
    }"#;

    #[test]
    fn manifest_keys_resolve_to_hashed_urls() {
        let manifest = AssetManifest::parse(VITE_MANIFEST).expect("manifest parses");
        assert_eq!(manifest.len(), 4);
        assert_eq!(
            manifest.url("resources/css/app.css"),
            "/assets/app-B7f3k9Jh.css"
        );
        assert_eq!(
            manifest.url("resources/js/app.js"),
            "/assets/app-9xK2pQ4r.js"
        );
        assert_eq!(
            manifest.styles("resources/js/app.js"),
            vec!["/assets/app-B7f3k9Jh.css".to_string()]
        );
        assert!(manifest.styles("resources/css/app.css").is_empty());
        assert!(manifest.entry("resources/js/app.js").unwrap().is_entry);
        assert_eq!(
            manifest.entry("public/logo.svg").unwrap().src,
            Some("public/logo.svg".to_string())
        );
    }

    #[test]
    fn unknown_keys_fall_back_to_literal_paths() {
        let manifest = AssetManifest::parse(VITE_MANIFEST).expect("manifest parses");
        assert_eq!(manifest.url("favicon.ico"), "/favicon.ico");
        assert_eq!(manifest.url("css/site.css"), "/css/site.css");
        assert_eq!(manifest.url("/css/site.css"), "/css/site.css");
        assert_eq!(manifest.styles("nope"), Vec::<String>::new());
        // Without a manifest the helper still answers root-absolute URLs.
        assert_eq!(asset_url("css/site.css"), "/css/site.css");
        assert_eq!(asset_url(""), "/");
    }

    #[test]
    fn traversal_in_a_key_never_escapes_the_root() {
        assert_eq!(asset_url("../../etc/passwd"), "/etc/passwd");
        assert_eq!(asset_url("/../secret"), "/secret");
        assert_eq!(asset_url("/a/./b/../c"), "/a/b/c");
    }

    #[test]
    fn malformed_manifest_is_an_error_not_an_empty_map() {
        let error = AssetManifest::parse("{ not json").expect_err("garbage refuses");
        assert!(error.contains("valid JSON"), "{error}");
        let error = AssetManifest::from_path(Path::new("/does/not/exist.json"))
            .expect_err("missing file refuses");
        assert!(error.contains("cannot read"), "{error}");
        assert!(AssetManifest::default().is_empty());
    }
}
