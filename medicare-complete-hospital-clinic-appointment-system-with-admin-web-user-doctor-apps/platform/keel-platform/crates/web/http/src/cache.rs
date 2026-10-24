//! Wave 2 HTTP caching (19-PARITY "Request/response layer"): a per-route
//! `Cache-Control` builder plus ETag generation with conditional-GET
//! (304) handling.
//!
//! ETags here are strong validators over the exact bytes we served
//! (`"<sha256 hex>"`, quoted per RFC 9110 §8.8.3). They are transport
//! validators, not integrity proofs — the response body is not signed.

use sha2::{Digest, Sha256};

/// Builder for a `Cache-Control` header value.
///
/// Directives are only ever emitted through this type, so a route's policy
/// comes out as a canonical `directive, key=value` list rather than a raw
/// string assembled by hand at the edge.
#[derive(Debug, Clone, Default)]
pub struct CacheControlBuilder {
    directives: Vec<String>,
}

impl CacheControlBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Cache-Control: public`
    pub fn public(mut self) -> Self {
        self.directives.push("public".into());
        self
    }

    /// `Cache-Control: private`
    pub fn private(mut self) -> Self {
        self.directives.push("private".into());
        self
    }

    /// `Cache-Control: no-store` (never store a copy).
    pub fn no_store(mut self) -> Self {
        self.directives.push("no-store".into());
        self
    }

    /// `Cache-Control: no-cache` (revalidate before reuse).
    pub fn no_cache(mut self) -> Self {
        self.directives.push("no-cache".into());
        self
    }

    /// `Cache-Control: immutable`.
    pub fn immutable(mut self) -> Self {
        self.directives.push("immutable".into());
        self
    }

    /// `max-age=<secs>`.
    pub fn max_age(mut self, secs: u64) -> Self {
        self.directives.push(format!("max-age={secs}"));
        self
    }

    /// `s-max-age=<secs>` (shared caches).
    pub fn s_max_age(mut self, secs: u64) -> Self {
        self.directives.push(format!("s-max-age={secs}"));
        self
    }

    /// `stale-while-revalidate=<secs>`.
    pub fn stale_while_revalidate(mut self, secs: u64) -> Self {
        self.directives
            .push(format!("stale-while-revalidate={secs}"));
        self
    }

    /// Finish the header value. An empty builder yields the empty string
    /// (the caller then simply does not set the header).
    pub fn build(self) -> String {
        self.directives.join(", ")
    }
}

/// Strong ETag over a response body: `"<sha256 hex>"`.
pub fn etag_for(body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body);
    let digest = hasher.finalize();
    let mut etag = String::with_capacity(66);
    etag.push('"');
    for byte in digest {
        etag.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex digit"));
        etag.push(char::from_digit(u32::from(byte & 0x0f), 16).expect("hex digit"));
    }
    etag.push('"');
    etag
}

/// RFC 9110 §13.1.2 `If-None-Match` evaluation against our ETag.
///
/// `*` matches any current representation; otherwise the header is a comma
/// list of entity-tags, weak (`W/"..."`) or strong, and any match is a hit.
pub fn if_none_match_hits(if_none_match: &str, etag: &str) -> bool {
    let wanted = etag.trim_start_matches("W/");
    if_none_match.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*" || candidate.trim_start_matches("W/") == wanted
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_emits_a_canonical_list() {
        let value = CacheControlBuilder::new()
            .public()
            .max_age(60)
            .stale_while_revalidate(30)
            .build();
        assert_eq!(value, "public, max-age=60, stale-while-revalidate=30");
    }

    #[test]
    fn builder_no_store_never_pairs_with_a_lifetime() {
        assert_eq!(CacheControlBuilder::new().no_store().build(), "no-store");
        assert_eq!(CacheControlBuilder::new().build(), "");
    }

    #[test]
    fn etag_is_quoted_and_stable() {
        let etag = etag_for(b"hello");
        assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
        assert_eq!(etag, etag_for(b"hello"));
        assert_ne!(etag, etag_for(b"hello!"));
        // SHA-256 hex.
        assert_eq!(etag.len(), 2 + 64, "{etag}");
    }

    #[test]
    fn if_none_match_accepts_lists_and_weak_tags() {
        let etag = etag_for(b"payload");
        assert!(if_none_match_hits(&etag, &etag));
        assert!(if_none_match_hits(&format!("W/{etag}"), &etag));
        assert!(if_none_match_hits(&format!("\"other\", {etag}"), &etag));
        assert!(if_none_match_hits("*", &etag));
        assert!(!if_none_match_hits("\"other\"", &etag));
        assert!(!if_none_match_hits("", &etag));
    }
}
