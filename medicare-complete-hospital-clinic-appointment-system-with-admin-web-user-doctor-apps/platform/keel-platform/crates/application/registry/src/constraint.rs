//! Route parameter constraints (Wave 2, Laravel `where*` parity).
//!
//! Closed zero-dep kinds: `number`, `alpha`, `alpha_num`, `uuid`, `ulid`,
//! `slug`, plus `pattern` with a hand-interpreted mini-grammar (no regex
//! crate — it is absent from `Cargo.lock` on purpose):
//! - literal characters (`\\` escapes the next character)
//! - character classes `[a-z]` / `[A-Z0-9]` including ranges
//! - repetition `{n}`, `{n,}`, `{n,m}` on the previous element
//!
//! Matching is always full-string (anchored both ends). Arbitrary regex
//! constraints stay deliberately SUPERSEDED — documented as the `where`-regex
//! divergence in docs/LF-011-TO-LF-027-VERIFICATION.md §3 (LF-013).

use serde::{Deserialize, Serialize};

/// Graph gate code: constraint kind outside the closed set, or a malformed
/// pattern (fail-closed, collect-all, boot-refusing).
pub const GRAPH_CONSTRAINT_KIND: &str = "GRAPH-CONSTRAINT-KIND";
/// Graph gate code: constrained parameter does not appear in the route path.
pub const GRAPH_CONSTRAINT_PARAM: &str = "GRAPH-CONSTRAINT-PARAM";

/// The closed constraint-kind set (Wave 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    Number,
    Alpha,
    AlphaNum,
    Uuid,
    Ulid,
    Slug,
    Pattern,
}

impl ConstraintKind {
    /// Parse a manifest kind token. Unknown tokens are a `GRAPH-CONSTRAINT-KIND`
    /// gate failure at graph compile.
    pub fn from_token(token: &str) -> Result<Self, String> {
        match token {
            "number" => Ok(Self::Number),
            "alpha" => Ok(Self::Alpha),
            "alpha_num" => Ok(Self::AlphaNum),
            "uuid" => Ok(Self::Uuid),
            "ulid" => Ok(Self::Ulid),
            "slug" => Ok(Self::Slug),
            "pattern" => Ok(Self::Pattern),
            other => Err(format!(
                "unknown constraint kind {other:?} (expected number, alpha, alpha_num, uuid, ulid, slug, pattern)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::Alpha => "alpha",
            Self::AlphaNum => "alpha_num",
            Self::Uuid => "uuid",
            Self::Ulid => "ulid",
            Self::Slug => "slug",
            Self::Pattern => "pattern",
        }
    }
}

/// Manifest shape: `constraints = [{ param = "id", kind = "uuid" }]` or
/// `{ param = "ref", kind = "pattern", pattern = "doc-[0-9]{4}" }`.
///
/// `kind` stays a string in the serialized contract so unknown kinds surface
/// as `GRAPH-CONSTRAINT-KIND` violations (collect-all) instead of a single
/// manifest parse error.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConstraintSpec {
    pub param: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub pattern: Option<String>,
}

/// A constraint resolved against the closed set (what the edge enforces and
/// what the route-cache artifact serializes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedConstraint {
    pub param: String,
    pub kind: ConstraintKind,
    /// Present iff kind == Pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<ConstraintPattern>,
}

impl ConstraintSpec {
    /// Resolve the manifest spec against the closed kind set + pattern
    /// grammar. Errors map to `GRAPH-CONSTRAINT-KIND` at graph compile.
    pub fn resolve(&self) -> Result<ResolvedConstraint, String> {
        let kind = ConstraintKind::from_token(self.kind.trim())?;
        let pattern = match kind {
            ConstraintKind::Pattern => {
                let raw = self.pattern.as_deref().filter(|p| !p.is_empty()).ok_or_else(|| {
                    format!("constraint on {{{}}}: kind = \"pattern\" requires a pattern", self.param)
                })?;
                Some(ConstraintPattern::parse(raw).map_err(|e| {
                    format!("constraint on {{{}}}: {e}", self.param)
                })?)
            }
            _ => {
                if self.pattern.is_some() {
                    return Err(format!(
                        "constraint on {{{}}}: pattern is only valid with kind = \"pattern\"",
                        self.param
                    ));
                }
                None
            }
        };
        Ok(ResolvedConstraint {
            param: self.param.clone(),
            kind,
            pattern,
        })
    }
}

/// Does a resolved constraint accept this captured path-param value?
pub fn constraint_matches(constraint: &ResolvedConstraint, value: &str) -> bool {
    match constraint.kind {
        ConstraintKind::Number => is_number(value),
        ConstraintKind::Alpha => is_alpha(value),
        ConstraintKind::AlphaNum => is_alpha_num(value),
        ConstraintKind::Uuid => is_uuid(value),
        ConstraintKind::Ulid => is_ulid(value),
        ConstraintKind::Slug => is_slug(value),
        ConstraintKind::Pattern => constraint
            .pattern
            .as_ref()
            .is_some_and(|p| p.is_match(value)),
    }
}

fn is_number(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

fn is_alpha(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphabetic())
}

fn is_alpha_num(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Canonical 8-4-4-4-12 hex with dashes (any case).
fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, b) in bytes.iter().enumerate() {
        let is_dash_pos = i == 8 || i == 13 || i == 18 || i == 23;
        if is_dash_pos {
            if *b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// Crockford base32, 26 chars, case-insensitive (I, L, O, U excluded).
fn is_ulid(value: &str) -> bool {
    const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let bytes = value.as_bytes();
    if bytes.len() != 26 {
        return false;
    }
    bytes.iter().all(|b| {
        let upper = b.to_ascii_uppercase();
        CROCKFORD.contains(&upper)
    })
}

/// `[a-z0-9]+(-[a-z0-9]+)*` — lowercase alnum runs joined by single hyphens.
fn is_slug(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let mut prev_hyphen = true; // rejects a leading hyphen
    for b in value.bytes() {
        if b == b'-' {
            if prev_hyphen {
                return false;
            }
            prev_hyphen = true;
        } else if b.is_ascii_lowercase() || b.is_ascii_digit() {
            prev_hyphen = false;
        } else {
            return false;
        }
    }
    !prev_hyphen // rejects a trailing hyphen
}

// ---------------------------------------------------------------- pattern
// Hand-interpreted mini-grammar (no regex crate). Elements: literal char,
// char class, each optionally followed by {n} / {n,} / {n,m}.

/// One repeatable element of a constraint pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PatternElement {
    /// A single literal character.
    Lit(char),
    /// A character class like [a-z] or [A-Z0-9]; stored as (lo, hi) ranges.
    Class(Vec<(char, char)>),
}

/// Repeat bounds on an element: exact {n}, at-least {n,}, or range {n,m}.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repeat {
    pub min: usize,
    /// None = unbounded.
    pub max: Option<usize>,
}

impl Repeat {
    fn once() -> Self {
        Self { min: 1, max: Some(1) }
    }
}

/// One element plus its repetition bounds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternTerm {
    pub element: PatternElement,
    pub repeat: Repeat,
}

/// A parsed constraint pattern: an ordered list of terms matched full-string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstraintPattern {
    pub source: String,
    pub terms: Vec<PatternTerm>,
}

impl ConstraintPattern {
    /// Parse the mini-grammar. Errors are stable strings mapped to
    /// `GRAPH-CONSTRAINT-KIND` at graph compile (fail closed).
    pub fn parse(src: &str) -> Result<Self, String> {
        let chars: Vec<char> = src.chars().collect();
        let mut terms = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let (element, next) = parse_element(&chars, i)?;
            i = next;
            let (repeat, next) = parse_repeat(&chars, i)?;
            i = next;
            terms.push(PatternTerm { element, repeat });
        }
        if terms.is_empty() {
            return Err("empty pattern".to_string());
        }
        Ok(Self { source: src.to_string(), terms })
    }

    /// Full-string match against the mini-grammar.
    pub fn is_match(&self, value: &str) -> bool {
        let chars: Vec<char> = value.chars().collect();
        match_terms(&self.terms, 0, &chars, 0)
    }
}

fn parse_element(chars: &[char], i: usize) -> Result<(PatternElement, usize), String> {
    let c = chars[i];
    match c {
        '\\' => {
            let nxt = chars.get(i + 1).ok_or("trailing backslash in pattern")?;
            Ok((PatternElement::Lit(*nxt), i + 2))
        }
        '[' => parse_class(chars, i),
        ']' | '}' | '{' => Err(format!("unexpected {c:?} in pattern")),
        _ => Ok((PatternElement::Lit(c), i + 1)),
    }
}

fn parse_class(chars: &[char], i: usize) -> Result<(PatternElement, usize), String> {
    debug_assert_eq!(chars[i], '[');
    let mut j = i + 1;
    let mut ranges: Vec<(char, char)> = Vec::new();
    if j < chars.len() && chars[j] == ']' {
        return Err("empty character class".to_string());
    }
    while j < chars.len() {
        let c = chars[j];
        if c == ']' {
            if ranges.is_empty() {
                return Err("empty character class".to_string());
            }
            return Ok((PatternElement::Class(ranges), j + 1));
        }
        let lo = c;
        // range?
        if j + 2 < chars.len() && chars[j + 1] == '-' && chars[j + 2] != ']' {
            let hi = chars[j + 2];
            if (lo as u32) > (hi as u32) {
                return Err(format!("inverted range {lo}-{hi} in character class"));
            }
            ranges.push((lo, hi));
            j += 3;
        } else {
            ranges.push((lo, lo));
            j += 1;
        }
    }
    Err("unterminated character class".to_string())
}

fn parse_repeat(chars: &[char], i: usize) -> Result<(Repeat, usize), String> {
    if i >= chars.len() || chars[i] != '{' {
        return Ok((Repeat::once(), i));
    }
    // parse {n} | {n,} | {n,m}
    let mut j = i + 1;
    let mut min: usize = 0;
    let mut saw_digit = false;
    while j < chars.len() && chars[j].is_ascii_digit() {
        min = min * 10 + (chars[j] as u32 - '0' as u32) as usize;
        saw_digit = true;
        j += 1;
    }
    if !saw_digit {
        return Err("repeat {n,m} needs a number".to_string());
    }
    if j < chars.len() && chars[j] == '}' {
        return Ok((Repeat { min, max: Some(min) }, j + 1));
    }
    if j < chars.len() && chars[j] == ',' {
        j += 1;
        // {n,} or {n,m}
        let mut max: Option<usize> = None;
        let mut saw_max = false;
        let mut mval: usize = 0;
        while j < chars.len() && chars[j].is_ascii_digit() {
            mval = mval * 10 + (chars[j] as u32 - '0' as u32) as usize;
            saw_max = true;
            j += 1;
        }
        if saw_max {
            max = Some(mval);
        }
        if j >= chars.len() || chars[j] != '}' {
            return Err("unterminated repeat {n,m}".to_string());
        }
        if let Some(mx) = max {
            if mx < min {
                return Err(format!("repeat max {mx} < min {min}"));
            }
        }
        return Ok((Repeat { min, max }, j + 1));
    }
    Err("malformed repeat {n,m}".to_string())
}

fn element_matches(element: &PatternElement, c: char) -> bool {
    match element {
        PatternElement::Lit(l) => *l == c,
        PatternElement::Class(ranges) => ranges.iter().any(|(lo, hi)| c >= *lo && c <= *hi),
    }
}

/// Recursive bounded matcher: terms[ti..] against chars[ci..]. Greedy with
/// backtracking over each term's repeat range.
fn match_terms(terms: &[PatternTerm], ti: usize, chars: &[char], ci: usize) -> bool {
    if ti == terms.len() {
        return ci == chars.len();
    }
    let term = &terms[ti];
    let max = term.repeat.max.unwrap_or(usize::MAX);
    // try consuming k occurrences, from max down to min (greedy).
    let mut k = 0usize;
    // first find the maximum feasible k
    let mut pos = ci;
    while k < max && pos < chars.len() && element_matches(&term.element, chars[pos]) {
        pos += 1;
        k += 1;
    }
    // now backtrack from k down to min
    let mut count = k;
    loop {
        if count >= term.repeat.min {
            let mut consumed = ci;
            for _ in 0..count {
                consumed += 1;
            }
            if match_terms(terms, ti + 1, chars, consumed) {
                return true;
            }
        }
        if count == 0 {
            break;
        }
        count -= 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(kind: &str, pattern: Option<&str>) -> ResolvedConstraint {
        ConstraintSpec {
            param: "p".into(),
            kind: kind.into(),
            pattern: pattern.map(|s| s.to_string()),
        }
        .resolve()
        .expect("spec should resolve")
    }

    // ------------------------------------------------- closed-kind matrix

    #[test]
    fn number_kind_matrix() {
        let c = resolved("number", None);
        assert!(constraint_matches(&c, "0"));
        assert!(constraint_matches(&c, "12345"));
        assert!(!constraint_matches(&c, ""));
        assert!(!constraint_matches(&c, "12a"));
        assert!(!constraint_matches(&c, "-1"));
        assert!(!constraint_matches(&c, "1.5"));
    }

    #[test]
    fn alpha_kind_matrix() {
        let c = resolved("alpha", None);
        assert!(constraint_matches(&c, "abcXYZ"));
        assert!(!constraint_matches(&c, ""));
        assert!(!constraint_matches(&c, "abc1"));
        assert!(!constraint_matches(&c, "abc-def"));
    }

    #[test]
    fn alpha_num_kind_matrix() {
        let c = resolved("alpha_num", None);
        assert!(constraint_matches(&c, "abc123XYZ"));
        assert!(!constraint_matches(&c, ""));
        assert!(!constraint_matches(&c, "a_b"));
        assert!(!constraint_matches(&c, "a b"));
    }

    #[test]
    fn uuid_kind_matrix() {
        let c = resolved("uuid", None);
        assert!(constraint_matches(&c, "6f9619ff-8b86-d011-b42d-00c04fc964ff"));
        assert!(constraint_matches(&c, "6F9619FF-8B86-D011-B42D-00C04FC964FF"));
        assert!(!constraint_matches(&c, ""));
        assert!(!constraint_matches(&c, "6f9619ff-8b86-d011-b42d-00c04fc964f")); // 35
        assert!(!constraint_matches(&c, "6f9619ff-8b86-d011-b42d-00c04fc964fff")); // 37
        assert!(!constraint_matches(&c, "6f9619ff88b86d011b42d00c04fc964ff9")); // no dashes
        assert!(!constraint_matches(&c, "6f9619ff-8b86-d011-b42d-00c04fc964fg")); // non-hex
    }

    #[test]
    fn ulid_kind_matrix() {
        let c = resolved("ulid", None);
        assert!(constraint_matches(&c, "01ARZ3NDEKTSV4RRFFQ69G5FAV"));
        assert!(constraint_matches(&c, "01arz3ndektsv4rrffq69g5fav")); // case-insensitive
        assert!(!constraint_matches(&c, ""));
        assert!(!constraint_matches(&c, "01ARZ3NDEKTSV4RRFFQ69G5FA")); // 25
        assert!(!constraint_matches(&c, "01ARZ3NDEKTSV4RRFFQ69G5FAVV")); // 27
        assert!(!constraint_matches(&c, "01ARZ3NDEKTSV4RRFFQ69G5FAU")); // U excluded
        assert!(!constraint_matches(&c, "01ARZ3NDEKTSV4RRFFQ69G5FAI")); // I excluded
    }

    #[test]
    fn slug_kind_matrix() {
        let c = resolved("slug", None);
        assert!(constraint_matches(&c, "hello"));
        assert!(constraint_matches(&c, "hello-world-2"));
        assert!(constraint_matches(&c, "a"));
        assert!(!constraint_matches(&c, ""));
        assert!(!constraint_matches(&c, "Hello"));
        assert!(!constraint_matches(&c, "hello--world"));
        assert!(!constraint_matches(&c, "-hello"));
        assert!(!constraint_matches(&c, "hello-"));
        assert!(!constraint_matches(&c, "hello_world"));
    }

    #[test]
    fn unknown_kind_rejected() {
        let err = ConstraintSpec {
            param: "p".into(),
            kind: "regex".into(),
            pattern: None,
        }
        .resolve()
        .unwrap_err();
        assert!(err.contains("unknown constraint kind"), "{err}");
    }

    #[test]
    fn pattern_kind_requires_pattern_field() {
        let err = ConstraintSpec {
            param: "p".into(),
            kind: "pattern".into(),
            pattern: None,
        }
        .resolve()
        .unwrap_err();
        assert!(err.contains("requires a pattern"), "{err}");
    }

    #[test]
    fn non_pattern_kind_rejects_pattern_field() {
        let err = ConstraintSpec {
            param: "p".into(),
            kind: "number".into(),
            pattern: Some("[0-9]".into()),
        }
        .resolve()
        .unwrap_err();
        assert!(err.contains("only valid with kind"), "{err}");
    }

    // ------------------------------------------------- pattern mini-grammar

    #[test]
    fn pattern_literals_full_string_anchored() {
        let c = resolved("pattern", Some("doc-42"));
        assert!(constraint_matches(&c, "doc-42"));
        assert!(!constraint_matches(&c, "doc-42x")); // anchored end
        assert!(!constraint_matches(&c, "xdoc-42")); // anchored start
        assert!(!constraint_matches(&c, ""));
    }

    #[test]
    fn pattern_char_classes_and_ranges() {
        let c = resolved("pattern", Some("[a-z][A-Z0-9]"));
        assert!(constraint_matches(&c, "aB"));
        assert!(constraint_matches(&c, "z9"));
        assert!(!constraint_matches(&c, "ab"));
        assert!(!constraint_matches(&c, "A9"));
        assert!(!constraint_matches(&c, "a"));
    }

    #[test]
    fn pattern_exact_repeat() {
        let c = resolved("pattern", Some("[0-9]{4}"));
        assert!(constraint_matches(&c, "2026"));
        assert!(!constraint_matches(&c, "202"));
        assert!(!constraint_matches(&c, "20266"));
    }

    #[test]
    fn pattern_range_repeat() {
        let c = resolved("pattern", Some("[a-z]{2,4}"));
        assert!(!constraint_matches(&c, "a"));
        assert!(constraint_matches(&c, "ab"));
        assert!(constraint_matches(&c, "abcd"));
        assert!(!constraint_matches(&c, "abcde"));
    }

    #[test]
    fn pattern_unbounded_repeat() {
        let c = resolved("pattern", Some("[0-9]{2,}"));
        assert!(!constraint_matches(&c, "1"));
        assert!(constraint_matches(&c, "12"));
        assert!(constraint_matches(&c, "123456789"));
    }

    #[test]
    fn pattern_literal_repeat_and_mixed_terms() {
        let c = resolved("pattern", Some("ab{2}c"));
        assert!(constraint_matches(&c, "abbc"));
        assert!(!constraint_matches(&c, "abc"));
        assert!(!constraint_matches(&c, "abbbc"));

        let mixed = resolved("pattern", Some("ref-[0-9]{3}-[a-z]{1,2}"));
        assert!(constraint_matches(&mixed, "ref-042-x"));
        assert!(constraint_matches(&mixed, "ref-042-xy"));
        assert!(!constraint_matches(&mixed, "ref-42-x"));
        assert!(!constraint_matches(&mixed, "ref-042-xyz"));
    }

    #[test]
    fn pattern_escaped_specials() {
        let c = resolved("pattern", Some("a\\{b"));
        assert!(constraint_matches(&c, "a{b"));
        assert!(!constraint_matches(&c, "ab"));

        let bracket = resolved("pattern", Some("x\\[1\\]"));
        assert!(constraint_matches(&bracket, "x[1]"));
    }

    #[test]
    fn pattern_backtracking_across_terms() {
        // [a-z]{1,3}[0-9]{1,2} over "ab12": the first term must give back
        // characters so the second term can match.
        let c = resolved("pattern", Some("[a-z]{1,3}[0-9]{1,2}"));
        assert!(constraint_matches(&c, "ab12"));
        assert!(constraint_matches(&c, "a1"));
        assert!(constraint_matches(&c, "abc99"));
        assert!(!constraint_matches(&c, "abcd1"));
        assert!(!constraint_matches(&c, "12ab"));
    }

    #[test]
    fn pattern_zero_repeat_optional_element() {
        let c = resolved("pattern", Some("a{0}b"));
        assert!(constraint_matches(&c, "b"));
        assert!(!constraint_matches(&c, "ab"));
    }

    // ------------------------------------------------- parse errors

    #[test]
    fn pattern_parse_errors() {
        for bad in [
            "",          // empty
            "a[",        // unterminated class
            "a[]",       // empty class
            "a[z-a]",    // inverted range
            "a{",        // unterminated repeat
            "a{x}",      // non-numeric repeat
            "a{2,1}",    // max < min
            "a}",        // stray close
            "a\\",     // trailing backslash
        ] {
            let err = ConstraintPattern::parse(bad).unwrap_err();
            assert!(!err.is_empty(), "pattern {bad:?} must fail with a message");
        }
    }

    #[test]
    fn pattern_roundtrips_through_serde() {
        let c = resolved("pattern", Some("doc-[0-9]{4}"));
        let json = serde_json::to_string(&c).unwrap();
        let back: ResolvedConstraint = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        assert!(constraint_matches(&back, "doc-2026"));
    }
}
