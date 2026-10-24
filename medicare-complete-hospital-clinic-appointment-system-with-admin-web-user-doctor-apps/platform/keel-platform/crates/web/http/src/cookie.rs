//! Typed cookies (S7 C6): builder + jar + `Set-Cookie` rendering.
//! Hand-rolled per D-62 (no cookie crate) — the same manual `Set-Cookie`
//! approach `csrf.rs` already proved, now centralized and typed.
//!
//! Parsing is deliberately tolerant of foreign cookies (skip, never fail the
//! request); RENDERING is strict (a malformed name/value never emits a
//! half-valid header — the caller gets an error instead).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::http::{header, HeaderMap, HeaderValue};
use kelson_foundation::{AppError, Secret};
use time::OffsetDateTime;

/// Cookie name: RFC 6265 token subset — printable ASCII without separators.
pub fn valid_cookie_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().all(|b| {
            b > 0x20
                && b < 0x7f
                && !matches!(
                    b,
                    b'(' | b')'
                        | b'<'
                        | b'>'
                        | b'@'
                        | b','
                        | b';'
                        | b':'
                        | b'\\'
                        | b'"'
                        | b'/'
                        | b'['
                        | b']'
                        | b'?'
                        | b'='
                        | b'{'
                        | b'}'
                )
        })
}

/// Cookie value: printable ASCII without `;`, `,`, `"` or backslash.
pub fn valid_cookie_value(value: &str) -> bool {
    value.len() <= 4096
        && value.bytes().all(|b| {
            ((0x21..0x7f).contains(&b) && !matches!(b, b';' | b',' | b'"' | b'\\')) || b == b' '
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    Lax,
    Strict,
    None,
}

impl SameSite {
    pub fn as_str(&self) -> &'static str {
        match self {
            SameSite::Lax => "Lax",
            SameSite::Strict => "Strict",
            SameSite::None => "None",
        }
    }
}

/// One outbound cookie. Defaults are the secure posture: `Path=/`,
/// `HttpOnly`, `SameSite=Lax`. `Secure` is explicit (environment-dependent).
#[derive(Debug, Clone)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub path: String,
    pub domain: Option<String>,
    pub max_age_secs: Option<i64>,
    pub expires: Option<OffsetDateTime>,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: Option<SameSite>,
}

impl Cookie {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Result<Self, AppError> {
        let name = name.into();
        let value = value.into();
        if !valid_cookie_name(&name) {
            return Err(AppError::validation(format!(
                "invalid cookie name {name:?}"
            )));
        }
        if !valid_cookie_value(&value) {
            return Err(AppError::validation(format!(
                "invalid cookie value for {name:?}"
            )));
        }
        Ok(Self {
            name,
            value,
            path: "/".into(),
            domain: None,
            max_age_secs: None,
            expires: None,
            http_only: true,
            secure: false,
            same_site: Some(SameSite::Lax),
        })
    }

    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    pub fn max_age_secs(mut self, secs: i64) -> Self {
        self.max_age_secs = Some(secs);
        self
    }

    pub fn expires(mut self, at: OffsetDateTime) -> Self {
        self.expires = Some(at);
        self
    }

    pub fn http_only(mut self, on: bool) -> Self {
        self.http_only = on;
        self
    }

    pub fn secure(mut self, on: bool) -> Self {
        self.secure = on;
        self
    }

    pub fn same_site(mut self, same_site: SameSite) -> Self {
        self.same_site = Some(same_site);
        self
    }

    /// Expired delete-cookie for `name` (Max-Age=0 + epoch Expires).
    pub fn expire(name: impl Into<String>, secure: bool) -> Result<Self, AppError> {
        Ok(Self::new(name, String::new())?
            .max_age_secs(0)
            .expires(OffsetDateTime::UNIX_EPOCH)
            .secure(secure))
    }

    /// Render as a `Set-Cookie` header value.
    pub fn render(&self) -> Result<String, AppError> {
        if !valid_cookie_name(&self.name) || !valid_cookie_value(&self.value) {
            return Err(AppError::validation(format!(
                "cookie {:?} is not renderable",
                self.name
            )));
        }
        let mut out = format!("{}={}", self.name, self.value);
        out.push_str("; Path=");
        out.push_str(&self.path);
        if let Some(domain) = &self.domain {
            out.push_str("; Domain=");
            out.push_str(domain);
        }
        if let Some(secs) = self.max_age_secs {
            out.push_str("; Max-Age=");
            out.push_str(&secs.to_string());
        }
        if let Some(at) = self.expires {
            out.push_str("; Expires=");
            out.push_str(&format_http_date(at));
        }
        if self.http_only {
            out.push_str("; HttpOnly");
        }
        if self.secure {
            out.push_str("; Secure");
        }
        if let Some(ss) = &self.same_site {
            out.push_str("; SameSite=");
            out.push_str(ss.as_str());
        }
        Ok(out)
    }

    /// Render and convert to a header value (render output is ASCII, so this
    /// cannot fail once `render` succeeded).
    pub fn header_value(&self) -> Result<HeaderValue, AppError> {
        let raw = self.render()?;
        HeaderValue::from_str(&raw).map_err(|e| AppError::internal(format!("cookie header: {e}")))
    }
}

/// RFC 1123 / IMF-fixdate: `Sun, 06 Nov 1994 08:49:37 GMT`.
/// Lookup-table rendering (infallible): the `time` format descriptors do not
/// portably express this exact shape, and a cookie expiry must never drop.
fn format_http_date(at: OffsetDateTime) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let at = at.to_offset(time::UtcOffset::UTC);
    let weekday = WEEKDAYS[at.weekday().number_days_from_sunday() as usize];
    let month = MONTHS[at.month() as usize - 1];
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        weekday,
        at.day(),
        month,
        at.year(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

/// Parsed request cookies (all `Cookie` headers merged; later duplicates win).
#[derive(Debug, Clone, Default)]
pub struct CookieJar {
    values: BTreeMap<String, String>,
}

impl CookieJar {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let mut jar = Self::default();
        for value in headers.get_all(header::COOKIE).iter() {
            let Ok(raw) = value.to_str() else { continue };
            for pair in raw.split(';') {
                let Some((name, val)) = pair.split_once('=') else {
                    continue;
                };
                let name = name.trim();
                if name.is_empty() || !valid_cookie_name(name) {
                    continue;
                }
                jar.values.insert(name.to_string(), val.trim().to_string());
            }
        }
        jar
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(|s| s.as_str())
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

// --------------------------------------------------- queued response cookies

/// Response-side cookie queue (Wave 2). Handlers push onto it (the edge
/// middleware installs one per request); the middleware drains it into
/// `Set-Cookie` headers once the handler produced its response — queued
/// cookies are a transport concern, not something every handler repeats.
#[derive(Debug, Clone, Default)]
pub struct CookieQueue {
    inner: std::sync::Arc<std::sync::Mutex<Vec<Cookie>>>,
}

impl CookieQueue {
    /// Queue a cookie for emission after the response is built.
    pub fn queue(&self, cookie: Cookie) {
        if let Ok(mut pending) = self.inner.lock() {
            pending.push(cookie);
        }
    }

    /// Drain every queued cookie (FIFO). A poisoned lock yields an empty
    /// list — a cookie never fails a request.
    pub fn drain(&self) -> Vec<Cookie> {
        self.inner
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|p| p.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// --------------------------------------------------- cookie value encryption

/// Optional cookie-value encryption (Wave 2): AES-256-GCM through the
/// foundation [`kelson_foundation::Sealer`] port, so the web layer never
/// learns which cipher a deployment picked — the security class supplies it.
///
/// Wire format: `k1.<hex(JSON Envelope)>`. Hex keeps the sealed blob inside
/// the printable-ASCII cookie value set without a base64 dependency, and the
/// `k1.` prefix marks the format so a future envelope version can coexist.
///
/// This is confidentiality for client-held values, not authentication: a
/// tampered value fails the AEAD tag check and is rejected as malformed.
pub struct CookieCipher {
    sealer: Arc<dyn kelson_foundation::Sealer>,
    key_id: String,
}

impl CookieCipher {
    pub fn new(sealer: Arc<dyn kelson_foundation::Sealer>, key_id: impl Into<String>) -> Self {
        Self {
            sealer,
            key_id: key_id.into(),
        }
    }

    /// Key identity the sealer seals under (rotation-aware open rejects a
    /// value sealed under another key).
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// Encrypt a cookie value for the wire.
    pub fn seal(&self, value: &str) -> Result<String, AppError> {
        let envelope = self.sealer.seal(&self.key_id, &Secret::new(value))?;
        let json = serde_json::to_string(&envelope)
            .map_err(|_| AppError::internal("sealed cookie value could not be serialized"))?;
        Ok(format!("k1.{}", hex_encode(json.as_bytes())))
    }

    /// Decrypt a wire value back to its plaintext. Anything that is not a
    /// well-formed `k1.` blob sealed under this key is rejected — malformed
    /// is a validation failure, never a silent passthrough.
    pub fn open(&self, token: &str) -> Result<String, AppError> {
        let malformed = || AppError::validation("sealed cookie value is malformed");
        let raw = token.strip_prefix("k1.").ok_or_else(malformed)?;
        let bytes = hex_decode(raw).ok_or_else(malformed)?;
        let json = std::str::from_utf8(&bytes).map_err(|_| malformed())?;
        let envelope: kelson_foundation::Envelope =
            serde_json::from_str(json).map_err(|_| malformed())?;
        let secret = kelson_foundation::open_for_key(&*self.sealer, &envelope, &self.key_id)
            .map_err(|_| malformed())?;
        Ok(secret.expose().to_string())
    }
}

/// Derive the 32-byte ASCII master-key material `AesGcmSealer::from_secret`
/// requires from an arbitrary deployment secret (the app key), scoped to the
/// cookie context so one derived key cannot open another context's material.
///
/// 32 hex characters of SHA-256 output = a 32-byte `Secret` carrying 128
/// bits of entropy — the exact shape the sealer demands.
pub fn derive_cookie_key(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update(b"\x00kelson.cookie.v1");
    let digest = hasher.finalize();
    hex_encode(&digest[..16])
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex digit"));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).expect("hex digit"));
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digits = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in digits.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_secure_defaults() {
        let raw = Cookie::new("kelson_session", "abc123")
            .unwrap()
            .render()
            .unwrap();
        assert_eq!(raw, "kelson_session=abc123; Path=/; HttpOnly; SameSite=Lax");
    }

    #[test]
    fn renders_full_attribute_set() {
        let at = OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap();
        let raw = Cookie::new("a", "v")
            .unwrap()
            .path("/api")
            .domain("example.com")
            .max_age_secs(3600)
            .expires(at)
            .http_only(false)
            .secure(true)
            .same_site(SameSite::Strict)
            .render()
            .unwrap();
        assert!(raw.starts_with("a=v; Path=/api; Domain=example.com; Max-Age=3600; Expires="));
        assert!(raw.contains(" GMT; Secure; SameSite=Strict"));
        assert!(!raw.contains("HttpOnly"));
    }

    #[test]
    fn expire_cookie_clears() {
        let raw = Cookie::expire("kelson_session", true)
            .unwrap()
            .render()
            .unwrap();
        assert!(raw.contains("kelson_session="));
        assert!(raw.contains("Max-Age=0"));
        assert!(raw.contains("Expires=Thu, 01 Jan 1970 00:00:00 GMT"));
        assert!(raw.contains("Secure"));
    }

    #[test]
    fn rejects_bad_name_and_value() {
        assert!(Cookie::new("", "v").is_err());
        assert!(Cookie::new("bad name", "v").is_err());
        assert!(Cookie::new("a=b", "v").is_err());
        assert!(Cookie::new("ok", "has;semicolon").is_err());
        assert!(Cookie::new("ok", "has\nnewline").is_err());
    }

    #[test]
    fn jar_parses_multiple_headers_and_skips_garbage() {
        let mut headers = HeaderMap::new();
        headers.append(header::COOKIE, "a=1; b=2".parse().unwrap());
        headers.append(header::COOKIE, "c=3; =nope; d".parse().unwrap());
        let jar = CookieJar::from_headers(&headers);
        assert_eq!(jar.get("a"), Some("1"));
        assert_eq!(jar.get("b"), Some("2"));
        assert_eq!(jar.get("c"), Some("3"));
        assert_eq!(jar.get("d"), None);
        assert_eq!(jar.len(), 3);
    }

    #[test]
    fn jar_trims_and_last_wins() {
        let mut headers = HeaderMap::new();
        headers.append(header::COOKIE, "a= 1 ".parse().unwrap());
        headers.append(header::COOKIE, "a=2".parse().unwrap());
        let jar = CookieJar::from_headers(&headers);
        assert_eq!(jar.get("a"), Some("2"));
    }

    #[test]
    fn http_date_shape() {
        let at = OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap();
        let stamp = format_http_date(at);
        // 2025-10-09 10:13:20 UTC
        assert_eq!(stamp.len(), 29);
        assert!(stamp.ends_with(" GMT"));
    }

    // ------------------------------------------------- queued + sealed cookies

    /// Deterministic stand-in for the AES-GCM sealer: same envelope plumbing
    /// (key identity recorded, ciphertext opaque) without pulling the
    /// security class into the web layer's test build.
    struct XorSealer {
        key: Vec<u8>,
    }

    impl kelson_foundation::Sealer for XorSealer {
        fn seal(
            &self,
            key_id: &str,
            secret: &Secret,
        ) -> Result<kelson_foundation::Envelope, AppError> {
            let ct = secret
                .expose()
                .bytes()
                .enumerate()
                .map(|(i, b)| b ^ self.key[i % self.key.len()] ^ 0x5a)
                .collect();
            Ok(kelson_foundation::Envelope::new(
                "TEST-XOR",
                key_id,
                vec![0u8; 12],
                ct,
            ))
        }

        fn open(&self, envelope: &kelson_foundation::Envelope) -> Result<Secret, AppError> {
            if envelope.alg != "TEST-XOR" {
                return Err(AppError::validation("unexpected algorithm"));
            }
            let pt: Vec<u8> = envelope
                .ciphertext
                .iter()
                .enumerate()
                .map(|(i, b)| b ^ self.key[i % self.key.len()] ^ 0x5a)
                .collect();
            String::from_utf8(pt)
                .map(Secret::new)
                .map_err(|_| AppError::validation("ciphertext is not utf-8"))
        }
    }

    fn cipher(key_id: &str) -> CookieCipher {
        CookieCipher::new(
            Arc::new(XorSealer {
                key: b"0123456789abcdef".to_vec(),
            }),
            key_id,
        )
    }

    #[test]
    fn queue_is_fifo_and_drains_once() {
        let queue = CookieQueue::default();
        assert!(queue.is_empty());
        queue.queue(Cookie::new("a", "1").unwrap());
        queue.queue(Cookie::new("b", "2").unwrap());
        assert_eq!(queue.len(), 2);
        let drained: Vec<String> = queue.drain().into_iter().map(|c| c.name).collect();
        assert_eq!(drained, ["a", "b"]);
        assert!(queue.is_empty(), "drain empties the queue");
    }

    #[test]
    fn sealed_cookie_values_round_trip_and_never_leak_plaintext() {
        let cipher = cipher("cookies-2026-01");
        let token = cipher.seal("patient-preference=evening").expect("seal");
        assert!(token.starts_with("k1."), "{token}");
        assert!(
            !token.contains("patient-preference"),
            "the wire value must not carry the plaintext: {token}"
        );
        assert_eq!(
            cipher.open(&token).expect("open"),
            "patient-preference=evening"
        );
        // A second seal of the same value may differ (fresh nonce upstream);
        // both must still open.
        let again = cipher.seal("patient-preference=evening").expect("seal");
        assert_eq!(
            cipher.open(&again).expect("open"),
            "patient-preference=evening"
        );
    }

    #[test]
    fn malformed_and_foreign_sealed_values_are_rejected() {
        let mine = cipher("cookies-2026-01");
        let token = mine.seal("value").expect("seal");

        assert!(mine.open("plaintext").is_err(), "no k1. prefix");
        assert!(mine.open("k1.zz").is_err(), "not hex");
        assert!(mine.open("k1.").is_err(), "empty payload");

        // Sealed under another key identity: rejected before decryption.
        let other = cipher("cookies-2026-07");
        let err = other.open(&token).expect_err("key identity differs");
        assert_eq!(err.public_message, "sealed cookie value is malformed");
    }

    #[test]
    fn cookie_key_derivation_is_stable_and_scoped() {
        let key = derive_cookie_key("base64:app-key");
        assert_eq!(key.len(), 32, "exactly 32 bytes of key material: {key}");
        assert_eq!(key, derive_cookie_key("base64:app-key"));
        assert_ne!(key, derive_cookie_key("base64:other-key"));
        assert_ne!(
            derive_cookie_key("base64:app-key"),
            derive_cookie_key("base64:app-key-other-context"),
            "context separator keeps derivation inputs unambiguous"
        );
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
