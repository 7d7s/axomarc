//! Collect-all field validation (S2 item 17, L08/L11): the FormRequest-role engine.
//!
//! Rules: required / min / max / email / url / uuid / e164 / one_of / range / date.
//! All violations are collected (never fail-fast) and reported as ONE AppError with
//! a \`fields\` meta map (field -> messages[]) so transports get a stable wire shape.
//! Domain invariants stay in the domain; these rules check input shape only.

use keel_foundation::AppError;
use serde_json::Value;

/// One validation rule applied to a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// Present, non-null, non-empty (whitespace-only strings and empty arrays fail).
    Required,
    /// Minimum size: string char count, array length, or numeric value.
    Min(u64),
    /// Maximum size: string char count, array length, or numeric value.
    Max(u64),
    /// Structural email: one '@', non-empty local, dotted domain.
    Email,
    /// http(s) URL with a non-empty host.
    Url,
    /// Canonical UUID (any version).
    Uuid,
    /// E.164 phone: '+' followed by 8..=15 digits.
    E164,
    /// String must equal one of the allowed values.
    OneOf(Vec<String>),
    /// Numeric value in an inclusive range.
    Range(i64, i64),
    /// RFC-3339 timestamp or plain YYYY-MM-DD date.
    Date,
}

impl Rule {
    pub fn name(&self) -> &'static str {
        match self {
            Rule::Required => "required",
            Rule::Min(_) => "min",
            Rule::Max(_) => "max",
            Rule::Email => "email",
            Rule::Url => "url",
            Rule::Uuid => "uuid",
            Rule::E164 => "e164",
            Rule::OneOf(_) => "one_of",
            Rule::Range(_, _) => "range",
            Rule::Date => "date",
        }
    }
}

impl Rule {
    /// Parse one manifest rule token into a [`Rule`]. Grammar (1:1 with the
    /// enum, Wave 1 FormRequest wiring): "required", "min:<n>", "max:<n>",
    /// "email", "url", "uuid", "e164", "one_of:<a,b,c>", "range:<lo,hi>",
    /// "date". Used by the registry GRAPH-VALIDATION-RULE gate and by the
    /// HTTP edge to compile manifest rules into a collect-all Validator.
    pub fn from_token(token: &str) -> Result<Rule, String> {
        let token = token.trim();
        let (head, arg) = match token.split_once(':') {
            Some((h, a)) => (h.trim(), Some(a.trim())),
            None => (token, None),
        };
        Ok(match head {
            "required" => Rule::Required,
            "email" => Rule::Email,
            "url" => Rule::Url,
            "uuid" => Rule::Uuid,
            "e164" => Rule::E164,
            "date" => Rule::Date,
            "min" => {
                let n = arg.ok_or_else(|| format!("min requires a value: {token}"))?;
                Rule::Min(
                    n.parse::<u64>()
                        .map_err(|_| format!("min value must be an unsigned integer: {token}"))?,
                )
            }
            "max" => {
                let n = arg.ok_or_else(|| format!("max requires a value: {token}"))?;
                Rule::Max(
                    n.parse::<u64>()
                        .map_err(|_| format!("max value must be an unsigned integer: {token}"))?,
                )
            }
            "one_of" => {
                let a = arg.ok_or_else(|| format!("one_of requires values: {token}"))?;
                let values: Vec<String> = a
                    .split(',')
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
                    .collect();
                if values.is_empty() {
                    return Err(format!("one_of requires at least one value: {token}"));
                }
                Rule::OneOf(values)
            }
            "range" => {
                let a = arg.ok_or_else(|| format!("range requires lo,hi: {token}"))?;
                let mut parts = a.split(',');
                let lo = parts.next().and_then(|v| v.trim().parse::<i64>().ok());
                let hi = parts.next().and_then(|v| v.trim().parse::<i64>().ok());
                match (lo, hi) {
                    (Some(lo), Some(hi)) => Rule::Range(lo, hi),
                    _ => return Err(format!("range must be range:<lo>,<hi>: {token}")),
                }
            }
            other => return Err(format!("unknown validation rule: {other}")),
        })
    }
}

/// Parse manifest rule tokens (collect-all): returns every parse error so the
/// registry gate can report them together (GRAPH-VALIDATION-RULE).
pub fn parse_rules(tokens: &[String]) -> Result<Vec<Rule>, Vec<String>> {
    let mut rules = Vec::new();
    let mut errors = Vec::new();
    for token in tokens {
        match Rule::from_token(token) {
            Ok(rule) => rules.push(rule),
            Err(err) => errors.push(err),
        }
    }
    if errors.is_empty() {
        Ok(rules)
    } else {
        Err(errors)
    }
}
/// Rules bound to one field (dot paths allowed, e.g. address.city).
#[derive(Debug, Clone)]
pub struct FieldSpec {
    pub field: String,
    pub rules: Vec<Rule>,
}

/// Collect-all validator: binds rules to fields, validates a JSON object.
#[derive(Debug, Clone, Default)]
pub struct Validator {
    specs: Vec<FieldSpec>,
}

impl Validator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: bind rules to a field. Later calls on the same field merge.
    pub fn field(mut self, name: impl Into<String>, rules: impl Into<Vec<Rule>>) -> Self {
        let name = name.into();
        let rules = rules.into();
        if let Some(spec) = self.specs.iter_mut().find(|s| s.field == name) {
            spec.rules.extend(rules);
        } else {
            self.specs.push(FieldSpec { field: name, rules });
        }
        self
    }

    pub fn field_names(&self) -> Vec<&str> {
        self.specs.iter().map(|s| s.field.as_str()).collect()
    }

    /// Validate input (must be a JSON object). Collects every violation.
    pub fn validate(&self, input: &Value) -> Result<(), ValidationReport> {
        let mut report = ValidationReport::default();
        if !input.is_object() {
            report.push("_", "input must be a JSON object");
            return Err(report);
        }
        for spec in &self.specs {
            let value = lookup(input, &spec.field);
            let required = spec.rules.iter().any(|r| matches!(r, Rule::Required));
            match value {
                None | Some(Value::Null) => {
                    if required {
                        report.push(&spec.field, format!("{} is required", spec.field));
                    }
                    continue;
                }
                Some(v) => {
                    if required && is_blank(v) {
                        report.push(&spec.field, format!("{} is required", spec.field));
                        continue;
                    }
                    for rule in &spec.rules {
                        if matches!(rule, Rule::Required) {
                            continue;
                        }
                        if let Some(msg) = check(rule, v) {
                            report.push(&spec.field, msg);
                        }
                    }
                }
            }
        }
        if report.is_empty() {
            Ok(())
        } else {
            Err(report)
        }
    }
}

/// One failed rule on one field (public-safe wording only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldViolation {
    pub field: String,
    pub message: String,
}

/// Collect-all report: empty == valid.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    pub violations: Vec<FieldViolation>,
}

impl ValidationReport {
    pub fn push(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.violations.push(FieldViolation {
            field: field.into(),
            message: message.into(),
        });
    }

    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.violations.len()
    }

    pub fn fields(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.violations.iter().map(|v| v.field.as_str()).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Uniform error map: one validation AppError carrying a
    /// fields meta object ({field}: [messages...]).
    pub fn into_app_error(self) -> AppError {
        let mut fields = serde_json::Map::new();
        for v in &self.violations {
            let entry = fields
                .entry(v.field.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(messages) = entry {
                messages.push(Value::String(v.message.clone()));
            }
        }
        AppError::validation(format!(
            "validation failed: {} field(s) invalid",
            self.violations.len()
        ))
        .with_meta("fields", fields)
    }
}

/// Dot-path lookup on nested JSON objects.
fn lookup<'a>(input: &'a Value, field: &str) -> Option<&'a Value> {
    let mut cur = input;
    for seg in field.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

fn is_blank(v: &Value) -> bool {
    match v {
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

/// Size of a value for min/max: string chars, array length, numeric value.
fn measure(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) => Some(s.chars().count() as f64),
        Value::Array(a) => Some(a.len() as f64),
        Value::Number(_) => v.as_f64(),
        _ => None,
    }
}

/// Apply one rule; returns a violation message when the value fails.
fn check(rule: &Rule, v: &Value) -> Option<String> {
    match rule {
        Rule::Required => unreachable!("handled before rule checks"),
        Rule::Min(n) => match measure(v) {
            Some(size) if size >= *n as f64 => None,
            Some(_) => Some(format!("must be at least {n}")),
            None => Some("size rules need a string, array or number".into()),
        },
        Rule::Max(n) => match measure(v) {
            Some(size) if size <= *n as f64 => None,
            Some(_) => Some(format!("must be at most {n}")),
            None => Some("size rules need a string, array or number".into()),
        },
        Rule::Email => match v.as_str() {
            Some(s) if is_email(s) => None,
            Some(_) => Some("must be a valid email".into()),
            None => Some("must be a string email".into()),
        },
        Rule::Url => match v.as_str() {
            Some(s) if is_url(s) => None,
            Some(_) => Some("must be an http(s) URL".into()),
            None => Some("must be a string URL".into()),
        },
        Rule::Uuid => match v.as_str() {
            Some(s) if uuid::Uuid::parse_str(s).is_ok() => None,
            Some(_) => Some("must be a valid UUID".into()),
            None => Some("must be a string UUID".into()),
        },
        Rule::E164 => match v.as_str() {
            Some(s) if is_e164(s) => None,
            Some(_) => Some("must be E.164 (+919876543210)".into()),
            None => Some("must be a string phone".into()),
        },
        Rule::OneOf(allowed) => match v.as_str() {
            Some(s) if allowed.iter().any(|a| a == s) => None,
            Some(_) => Some(format!("must be one of: {}", allowed.join(", "))),
            None => Some("must be a string choice".into()),
        },
        Rule::Range(lo, hi) => match v.as_f64() {
            Some(x) if x >= *lo as f64 && x <= *hi as f64 => None,
            Some(_) => Some(format!("must be {lo}..={hi}")),
            None => Some("must be a number".into()),
        },
        Rule::Date => match v.as_str() {
            Some(s) if is_date(s) => None,
            Some(_) => Some("must be RFC-3339 or YYYY-MM-DD".into()),
            None => Some("must be a string date".into()),
        },
    }
}

fn is_email(s: &str) -> bool {
    let mut parts = s.split('@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    parts.next().is_none()
        && !local.is_empty()
        && local.chars().count() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains(char::is_whitespace)
        && domain.chars().count() >= 3
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains(char::is_whitespace)
}

fn is_url(s: &str) -> bool {
    let rest = match s
        .strip_prefix("http://")
        .or_else(|| s.strip_prefix("https://"))
    {
        Some(r) => r,
        None => return false,
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !host.contains(char::is_whitespace)
}

fn is_e164(s: &str) -> bool {
    s.starts_with('+') && (8..=16).contains(&s.len()) && s[1..].chars().all(|c| c.is_ascii_digit())
}

fn is_date(s: &str) -> bool {
    if time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).is_ok() {
        return true;
    }
    // RFC 3339 strictly requires seconds; also accept minute-precision
    // timestamps (T10:00Z / T10:00+05:30), the shape appointment slots use.
    if let Ok(fmt) =
        time::format_description::parse_borrowed::<2>("[year]-[month]-[day]T[hour]:[minute]Z")
    {
        if time::PrimitiveDateTime::parse(s, &fmt).is_ok() {
            return true;
        }
    }
    if let Ok(fmt) = time::format_description::parse_borrowed::<2>(
        "[year]-[month]-[day]T[hour]:[minute][offset_hour sign:mandatory]:[offset_minute]",
    ) {
        if time::OffsetDateTime::parse(s, &fmt).is_ok() {
            return true;
        }
    }
    match time::format_description::parse_borrowed::<2>("[year]-[month]-[day]") {
        Ok(fmt) => time::Date::parse(s, &fmt).is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collect_all_reports_every_violation() {
        let v = Validator::new()
            .field("name", vec![Rule::Required, Rule::Max(3)])
            .field("email", vec![Rule::Required, Rule::Email])
            .field("age", vec![Rule::Range(0, 130)]);
        let report = v
            .validate(&json!({ "name": "toolong", "age": 200 }))
            .unwrap_err();
        assert_eq!(report.len(), 3);
        assert_eq!(report.fields(), vec!["age", "email", "name"]);
    }

    #[test]
    fn report_maps_to_app_error_with_fields_meta() {
        let mut report = ValidationReport::default();
        report.push("email", "must be a valid email");
        report.push("email", "second message");
        let err = report.into_app_error();
        assert_eq!(err.category, keel_foundation::ErrorCategory::Validation);
        let fields = err.meta.get("fields").expect("fields meta");
        assert_eq!(fields["email"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn required_semantics() {
        let v = Validator::new().field("name", vec![Rule::Required]);
        assert!(v.validate(&json!({ "name": "ok" })).is_ok());
        assert!(v.validate(&json!({ "name": "   " })).is_err());
        assert!(v.validate(&json!({ "name": null })).is_err());
        assert!(v.validate(&json!({})).is_err());
    }

    #[test]
    fn optional_absent_fields_skip_rules() {
        let v = Validator::new().field("nick", vec![Rule::Email]);
        assert!(v.validate(&json!({})).is_ok());
        assert!(v.validate(&json!({ "nick": "not-an-email" })).is_err());
    }

    #[test]
    fn size_rules_measure_by_kind() {
        let v = Validator::new()
            .field("s", vec![Rule::Min(2), Rule::Max(4)])
            .field("n", vec![Rule::Min(10)])
            .field("a", vec![Rule::Max(2)]);
        assert!(v
            .validate(&json!({ "s": "abc", "n": 11, "a": [1, 2] }))
            .is_ok());
        assert!(
            v.validate(&json!({ "s": "a", "n": 9, "a": [1, 2, 3] }))
                .unwrap_err()
                .len()
                == 3
        );
    }

    #[test]
    fn format_rules() {
        assert!(is_email("alice@example.co.in"));
        assert!(!is_email("alice@@example.com"));
        assert!(!is_email("alice@examplecom"));
        assert!(is_url("https://keel.dev/docs"));
        assert!(!is_url("ftp://keel.dev"));
        assert!(is_e164("+919876543210"));
        assert!(!is_e164("00919876543210"));
        assert!(is_date("2026-08-20"));
        assert!(is_date("2026-08-20T10:00:00+05:30"));
        assert!(is_date("2026-08-20T10:00Z"), "minute precision Z");
        assert!(is_date("2026-08-20T10:00+05:30"), "minute precision offset");
        assert!(!is_date("2026-08-20T10:00"), "no timezone is ambiguous");
        assert!(!is_date("20/08/2026"));
        assert!(uuid::Uuid::parse_str("018f1c2a-0000-7000-8000-000000000001").is_ok());
    }

    #[test]
    fn one_of_and_range() {
        let v = Validator::new()
            .field(
                "kind",
                vec![Rule::OneOf(vec!["self".into(), "family".into()])],
            )
            .field("qty", vec![Rule::Range(1, 5)]);
        assert!(v.validate(&json!({ "kind": "self", "qty": 3 })).is_ok());
        let report = v
            .validate(&json!({ "kind": "other", "qty": 9 }))
            .unwrap_err();
        assert_eq!(report.len(), 2);
    }

    #[test]
    fn dot_paths_validate_nested_fields() {
        let v = Validator::new().field("address.city", vec![Rule::Required, Rule::Max(20)]);
        assert!(v
            .validate(&json!({ "address": { "city": "Pune" } }))
            .is_ok());
        assert!(v.validate(&json!({ "address": {} })).is_err());
    }

    #[test]
    fn non_object_input_is_rejected() {
        let v = Validator::new().field("x", vec![Rule::Required]);
        assert!(v.validate(&json!([1, 2])).is_err());
    }

    #[test]
    fn rule_tokens_parse() {
        assert_eq!(Rule::from_token("required").unwrap(), Rule::Required);
        assert_eq!(Rule::from_token("min:3").unwrap(), Rule::Min(3));
        assert_eq!(Rule::from_token("max:500").unwrap(), Rule::Max(500));
        assert_eq!(Rule::from_token("email").unwrap(), Rule::Email);
        assert_eq!(Rule::from_token("url").unwrap(), Rule::Url);
        assert_eq!(Rule::from_token("uuid").unwrap(), Rule::Uuid);
        assert_eq!(Rule::from_token("e164").unwrap(), Rule::E164);
        assert_eq!(
            Rule::from_token("one_of:self,family").unwrap(),
            Rule::OneOf(vec!["self".into(), "family".into()])
        );
        assert_eq!(Rule::from_token("range:1,5").unwrap(), Rule::Range(1, 5));
        assert_eq!(Rule::from_token("date").unwrap(), Rule::Date);
        assert_eq!(Rule::from_token(" date ").unwrap(), Rule::Date);
    }

    #[test]
    fn rule_tokens_reject_malformed() {
        for bad in [
            "min", "min:x", "max:-1", "one_of", "one_of:", "one_of: ,", "range:1", "range:a,b",
            "nope", "min:3:4", "",
        ] {
            assert!(Rule::from_token(bad).is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn parse_rules_collects_all_errors() {
        let tokens: Vec<String> = vec!["required".into(), "bogus".into(), "min:x".into()];
        let errs = parse_rules(&tokens).unwrap_err();
        assert_eq!(errs.len(), 2);
        let ok = parse_rules(&["required".into(), "email".into()]).unwrap();
        assert_eq!(ok.len(), 2);
    }
}
