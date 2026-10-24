//! # Wave 2 request helpers (19-PARITY "Request/response layer")
//!
//! One transport vocabulary for request intent and client identity. Every
//! helper is a pure function over the inbound `HeaderMap` — no session, no
//! state, no silent defaults: a header that is absent is reported as absent.

use axum::http::{header, HeaderMap};

/// Explicit JSON client (the `expects_json` half of the web-plane contract).
///
/// Deliberately **not** wildcard-aware: a `*/*` or `text/html` client is not
/// a JSON client, so web routes keep rendering HTML for browsers and `curl`.
/// An explicit `q=0` ("not acceptable") is rejected.
pub fn expects_json(headers: &HeaderMap) -> bool {
    accepts(headers, "application/json")
}

/// Is `mime` listed in the `Accept` header with a non-zero q-value?
///
/// Media types are compared exactly (case-insensitively); wildcards are not
/// expanded here — callers decide whether `*/*` counts for their use case
/// (see [`expects_json`] for why the web plane says no).
pub fn accepts(headers: &HeaderMap, mime: &str) -> bool {
    let Some(raw) = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let wanted = mime.trim().to_ascii_lowercase();
    raw.split(',').any(|entry| {
        let mut parts = entry.split(';');
        let name = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
        if name != wanted {
            return false;
        }
        for param in parts {
            let Some((key, value)) = param.split_once('=') else {
                continue;
            };
            if key.trim().eq_ignore_ascii_case("q") {
                let q = value.trim().parse::<f64>().unwrap_or(1.0);
                if q <= 0.0 {
                    return false;
                }
            }
        }
        true
    })
}

/// Bearer credential from `Authorization: Bearer <token>` (scheme is
/// case-insensitive, the token must be non-empty).
pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = rest.trim();
    (!token.is_empty()).then_some(token)
}

/// Best-effort client IP: the first hop of `X-Forwarded-For`, otherwise the
/// `global` sentinel. **Telemetry only** (MNC-004): this trusts the header
/// unconditionally, so it must never decide a security-relevant bucket —
/// credential throttling and rate limiting derive their client key through
/// the deployment's `TrustedProxies` set instead
/// (`middleware::client_key_with_proxies`, or the auth plane's
/// `throttle_client`), where a client-supplied header cannot rotate its own
/// bucket.
pub fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "global".into())
}

/// Stable, privacy-preserving client fingerprint: FNV-1a over the three
/// volatility headers in a fixed order, separated so concatenation cannot
/// collide (`"ab" + "c"` vs `"a" + "bc"`). Carries no identifier — it is a
/// coarse signal for fraud/throttle correlation, never an identity.
pub fn fingerprint(headers: &HeaderMap) -> String {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for name in ["user-agent", "accept-language", "accept-encoding"] {
        let value = headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        for byte in value.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
        // Separator: keeps field boundaries unambiguous in the digest.
        hash ^= 0xff;
        hash = hash.wrapping_mul(PRIME);
    }
    format!("fnv1a64:{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                header::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        map
    }

    #[test]
    fn expects_json_needs_an_explicit_accept() {
        assert!(expects_json(&headers(&[("accept", "application/json")])));
        assert!(expects_json(&headers(&[(
            "accept",
            "text/html, application/json;q=0.9"
        )])));
        // Wildcards and HTML clients are not JSON clients.
        assert!(!expects_json(&HeaderMap::new()));
        assert!(!expects_json(&headers(&[("accept", "*/*")])));
        assert!(!expects_json(&headers(&[("accept", "text/html")])));
        // problem+json is a different media type.
        assert!(!expects_json(&headers(&[(
            "accept",
            "application/problem+json"
        )])));
    }

    #[test]
    fn accepts_rejects_zero_quality_and_unknown_types() {
        let map = headers(&[("accept", "text/plain;q=0, application/json;q=0.5")]);
        assert!(accepts(&map, "application/json"));
        assert!(!accepts(&map, "text/plain"), "q=0 means unacceptable");
        assert!(!accepts(&map, "text/html"));
        // Case-insensitive on both sides.
        assert!(accepts(
            &headers(&[("accept", "Application/JSON")]),
            "application/json"
        ));
    }

    #[test]
    fn bearer_token_requires_the_scheme() {
        assert_eq!(
            bearer_token(&headers(&[("authorization", "Bearer abc.def")])),
            Some("abc.def")
        );
        assert_eq!(
            bearer_token(&headers(&[("authorization", "bearer   spaced  ")])),
            Some("spaced")
        );
        assert_eq!(
            bearer_token(&headers(&[("authorization", "Basic dXNlcjpwdw==")])),
            None
        );
        assert_eq!(
            bearer_token(&headers(&[("authorization", "Bearer ")])),
            None
        );
        assert_eq!(bearer_token(&HeaderMap::new()), None);
    }

    #[test]
    fn client_ip_takes_the_first_forwarded_hop() {
        assert_eq!(
            client_ip(&headers(&[("x-forwarded-for", "203.0.113.9, 10.0.0.1")])),
            "203.0.113.9"
        );
        assert_eq!(client_ip(&headers(&[("x-forwarded-for", "  ")])), "global");
        assert_eq!(client_ip(&HeaderMap::new()), "global");
    }

    #[test]
    fn fingerprint_is_stable_and_field_separated() {
        let a = fingerprint(&headers(&[
            ("user-agent", "kelson-test/1.0"),
            ("accept-language", "en-GB"),
        ]));
        let b = fingerprint(&headers(&[
            ("user-agent", "kelson-test/1.0"),
            ("accept-language", "en-GB"),
        ]));
        assert_eq!(a, b, "same headers fingerprint identically");

        // Field boundaries must matter: "x"+"yz" != "xy"+"z".
        let split_one = fingerprint(&headers(&[("user-agent", "xyz"), ("accept-language", "")]));
        let split_two = fingerprint(&headers(&[("user-agent", "x"), ("accept-language", "yz")]));
        assert_ne!(split_one, split_two);

        let other = fingerprint(&headers(&[("user-agent", "firefox/1.0")]));
        assert_ne!(a, other);
        assert!(a.starts_with("fnv1a64:"));
    }
}
