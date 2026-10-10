//! Pagination (S2 item 26): Page<T> public wire shape + query parsing.
//! Deterministic caps: page >= 1, per_page 1..=MAX_PER_PAGE (default 25).

use kelson_foundation::AppError;
use serde::{Deserialize, Serialize};

pub const DEFAULT_PER_PAGE: u64 = 25;
pub const MAX_PER_PAGE: u64 = 100;

/// Parsed page query: 1-based page, capped per_page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageQuery {
    pub page: u64,
    pub per_page: u64,
}

impl Default for PageQuery {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: DEFAULT_PER_PAGE,
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

/// One page of results. The total is optional: count-free scans may omit it,
/// in which case has_more falls back to a full-page heuristic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u64,
    pub per_page: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, page: u64, per_page: u64, total: Option<u64>) -> Self {
        Self {
            items,
            page,
            per_page,
            total,
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
        Page::new(
            self.items.into_iter().map(f).collect(),
            self.page,
            self.per_page,
            self.total,
        )
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
