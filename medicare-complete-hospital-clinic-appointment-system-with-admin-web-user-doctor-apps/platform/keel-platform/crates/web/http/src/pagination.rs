//! Pagination (S2 item 26): Page<T> public wire shape + query parsing.
//! Deterministic caps: page >= 1, per_page 1..=MAX_PER_PAGE (default 25).
//!
//! Two pagination modes ship side by side:
//! - **offset** (the default): `page` + `per_page`, with an optional `total`.
//! - **cursor** (keyset, for large changing datasets): a `cursor` carries the
//!   anchor the next page starts *after*, so a concurrent insert cannot shift
//!   the window. The cursor is opaque and signed — see [`Cursor`].

use kelson_foundation::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

pub const DEFAULT_PER_PAGE: u64 = 25;
pub const MAX_PER_PAGE: u64 = 100;

/// Parsed page query: 1-based page, capped per_page, plus an optional cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageQuery {
    pub page: u64,
    pub per_page: u64,
    /// The opaque cursor from a previous page (keyset mode). `None` = offset
    /// mode. A cursor and a `page` together are refused by the caller that
    /// resolves them (a cursor already encodes its position).
    pub cursor: Option<String>,
}

impl Default for PageQuery {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: DEFAULT_PER_PAGE,
            cursor: None,
        }
    }
}

impl PageQuery {
    /// Parse from (key, value) query pairs; unknown keys are ignored.
    /// Errors are validation-category with precise public messages.
    pub fn from_pairs<'a, I>(pairs: I) -> Result<Self, AppError>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut q = Self::default();
        for (key, raw) in pairs {
            match key {
                "page" => {
                    q.page = raw
                        .parse::<u64>()
                        .map_err(|_| AppError::validation("page must be a positive integer"))?;
                }
                "per_page" => {
                    q.per_page = raw
                        .parse::<u64>()
                        .map_err(|_| AppError::validation("per_page must be a positive integer"))?;
                }
                "cursor" => {
                    if !raw.is_empty() {
                        q.cursor = Some(raw.to_string());
                    }
                }
                _ => {}
            }
        }
        if q.page == 0 {
            return Err(AppError::validation("page starts at 1"));
        }
        if q.per_page == 0 {
            q.per_page = DEFAULT_PER_PAGE;
        }
        q.per_page = q.per_page.min(MAX_PER_PAGE);
        Ok(q)
    }

    /// Offset/limit for stores that paginate with OFFSET.
    pub fn offset(&self) -> u64 {
        (self.page - 1) * self.per_page
    }

    pub fn limit(&self) -> u64 {
        self.per_page
    }
}

/// An opaque, tamper-evident pagination cursor.
///
/// The cursor carries the keyset anchor — the value the next page starts
/// *after* — plus an HMAC-SHA256 over it, so a client can neither forge nor
/// edit one. It is base64url, so it survives a query string untouched.
///
/// Decoding **fails closed**: a cursor that is not base64url, is too short,
/// was signed with another secret, or anchors another column is refused with a
/// validation error rather than silently treated as "no cursor".
pub struct Cursor;

impl Cursor {
    /// Encode the anchor value for `column` under `secret`.
    pub fn encode(column: &str, value: &Json, secret: &[u8]) -> String {
        let payload = format!("{column}\u{1f}{value}");
        let mac = crate::signed::hmac_sha256(secret, payload.as_bytes());
        let mut bytes = payload.into_bytes();
        bytes.extend_from_slice(&mac);
        crate::jwt::b64url_encode(&bytes)
    }

    /// Decode a cursor into the anchor value for `column`.
    pub fn decode(cursor: &str, column: &str, secret: &[u8]) -> Result<Json, AppError> {
        let bytes = crate::jwt::b64url_decode(cursor)
            .ok_or_else(|| AppError::validation("cursor is malformed"))?;
        if bytes.len() < 33 {
            return Err(AppError::validation("cursor is malformed"));
        }
        let (payload, mac) = bytes.split_at(bytes.len() - 32);
        let expected = crate::signed::hmac_sha256(secret, payload);
        if !constant_time_eq(mac, &expected) {
            return Err(AppError::validation("cursor is not valid for this request"));
        }
        let payload = String::from_utf8(payload.to_vec())
            .map_err(|_| AppError::validation("cursor is malformed"))?;
        let (col, value) = payload
            .split_once('\u{1f}')
            .ok_or_else(|| AppError::validation("cursor is malformed"))?;
        if col != column {
            return Err(AppError::validation("cursor anchors another column"));
        }
        serde_json::from_str(value).map_err(|_| AppError::validation("cursor is malformed"))
    }
}

/// Constant-time byte comparison (the cursor's MAC check must not leak where
/// the signatures differ).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// One page of results. The total is optional: count-free scans may omit it,
/// in which case has_more falls back to a full-page heuristic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u64,
    pub per_page: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// The cursor for the next page (keyset mode). `None` when the page is the
    /// last one, or in offset mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, page: u64, per_page: u64, total: Option<u64>) -> Self {
        Self {
            items,
            page,
            per_page,
            total,
            next_cursor: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn has_more(&self) -> bool {
        match self.total {
            Some(total) => self.page.saturating_mul(self.per_page) < total,
            None => self.items.len() as u64 >= self.per_page,
        }
    }

    /// Meta block for Envelope.with_meta (stable wire shape).
    pub fn meta(&self) -> serde_json::Value {
        serde_json::json!({
            "page": self.page,
            "per_page": self.per_page,
            "total": self.total,
            "has_more": self.has_more(),
        })
    }

    /// Map items, preserving pagination fields.
    pub fn map<U>(self, f: impl FnMut(T) -> U) -> Page<U> {
        let mut mapped = Page::new(
            self.items.into_iter().map(f).collect(),
            self.page,
            self.per_page,
            self.total,
        );
        mapped.next_cursor = self.next_cursor;
        mapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_caps_page_query() {
        let q = PageQuery::from_pairs([("page", "3"), ("per_page", "5000")]).unwrap();
        assert_eq!((q.page, q.per_page), (3, MAX_PER_PAGE));
        assert_eq!(q.offset(), 2 * MAX_PER_PAGE);
    }

    #[test]
    fn defaults_and_zero_per_page() {
        assert_eq!(PageQuery::from_pairs([]).unwrap(), PageQuery::default());
        let q = PageQuery::from_pairs([("per_page", "0")]).unwrap();
        assert_eq!(q.per_page, DEFAULT_PER_PAGE);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(PageQuery::from_pairs([("page", "0")]).is_err());
        assert!(PageQuery::from_pairs([("page", "-2")]).is_err());
        assert!(PageQuery::from_pairs([("per_page", "ten")]).is_err());
    }

    #[test]
    fn parses_the_cursor_query_param() {
        let q = PageQuery::from_pairs([("cursor", "abc"), ("per_page", "10")]).unwrap();
        assert_eq!(q.cursor.as_deref(), Some("abc"));
        assert_eq!(q.per_page, 10);
        // An empty cursor is no cursor.
        let q = PageQuery::from_pairs([("cursor", "")]).unwrap();
        assert_eq!(q.cursor, None);
        assert_eq!(q, PageQuery::default());
    }

    #[test]
    fn cursor_round_trips_the_anchor_value() {
        let secret = b"a-test-secret";
        let encoded = Cursor::encode("slot", &serde_json::json!("2026-10-20"), secret);
        // The cursor is opaque: the raw anchor is not readable from it.
        assert!(!encoded.contains("slot"));
        assert!(!encoded.contains("2026-10-20"));
        let decoded = Cursor::decode(&encoded, "slot", secret).unwrap();
        assert_eq!(decoded, serde_json::json!("2026-10-20"));
    }

    #[test]
    fn cursor_rejects_a_tampered_value() {
        let secret = b"a-test-secret";
        let encoded = Cursor::encode("slot", &serde_json::json!(7), secret);
        // Flip one character of the base64url payload: the MAC must catch it.
        let mut bytes = encoded.into_bytes();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(bytes).unwrap();
        let err =
            Cursor::decode(&tampered, "slot", secret).expect_err("tampered cursor is refused");
        assert_eq!(err.category, kelson_foundation::ErrorCategory::Validation);
    }

    #[test]
    fn cursor_rejects_another_secret_and_another_column() {
        let encoded = Cursor::encode("slot", &serde_json::json!(7), b"secret-one");
        assert!(
            Cursor::decode(&encoded, "slot", b"secret-two").is_err(),
            "a cursor signed with another secret is refused"
        );
        assert!(
            Cursor::decode(&encoded, "other_column", b"secret-one").is_err(),
            "a cursor anchored on another column is refused"
        );
    }

    #[test]
    fn cursor_rejects_malformed_input() {
        let secret = b"a-test-secret";
        assert!(Cursor::decode("not base64url!!", "slot", secret).is_err());
        assert!(Cursor::decode("", "slot", secret).is_err());
        // Short enough to carry no MAC at all.
        assert!(Cursor::decode("aGVsbG8", "slot", secret).is_err());
    }

    #[test]
    fn has_more_with_and_without_total() {
        let with_total: Page<u32> = Page::new(vec![1, 2], 1, 2, Some(5));
        assert!(with_total.has_more());
        let last: Page<u32> = Page::new(vec![1], 3, 2, Some(5));
        assert!(!last.has_more());
        let heuristic: Page<u32> = Page::new(vec![1, 2], 1, 2, None);
        assert!(heuristic.has_more());
        let short: Page<u32> = Page::new(vec![1], 1, 2, None);
        assert!(!short.has_more());
    }

    #[test]
    fn meta_wire_shape() {
        let p: Page<u32> = Page::new(vec![1], 2, 25, Some(26));
        let meta = p.meta();
        assert_eq!(meta["page"], 2);
        assert_eq!(meta["has_more"], false);
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"items\""));
    }
}
