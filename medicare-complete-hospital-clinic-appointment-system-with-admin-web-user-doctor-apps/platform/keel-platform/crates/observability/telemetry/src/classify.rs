//! Classification + redaction vocabulary (Sentinel S5 §10/§20). Sensitivity is
//! attached to events by the classify processor and consumed by policy/redact.

use serde::{Deserialize, Serialize};

/// Kernel default sensitive field names (never emitted in clear; mirrors the
/// S1 redaction list so behaviour carries over).
pub const DEFAULT_SECRET_FIELDS: &[&str] = &[
    "password",
    "secret",
    "jwt_secret",
    "authorization",
    "cookie",
    "token",
    "api_key",
    "otp",
    "ssn",
    "credit_card",
    "phi",
];

/// Field names carrying personal data (redacted outside incident capture).
pub const DEFAULT_PII_FIELDS: &[&str] = &["email", "phone", "address", "date_of_birth", "name"];

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Public,
    #[default]
    Internal,
    Sensitive,
    Secret,
}

/// True when `field` matches a secret keyword (case-insensitive substring on
/// word-ish boundaries, same spirit as the S1 line redactor).
pub fn is_secret_field(field: &str) -> bool {
    let lower = field.to_ascii_lowercase();
    // Treat separators interchangeably (X-Api-Key == x_api_key == xapikey).
    let squashed: String = lower.chars().filter(|c| *c != '-' && *c != '_').collect();
    DEFAULT_SECRET_FIELDS
        .iter()
        .any(|s| lower == *s || lower.contains(s) || squashed.contains(&s.replace('_', "")))
}

pub fn is_pii_field(field: &str) -> bool {
    let lower = field.to_ascii_lowercase();
    DEFAULT_PII_FIELDS
        .iter()
        .any(|s| lower == *s || lower.ends_with(s))
}

/// Classify a field bag: presence of secret-named keys escalates to Secret,
/// PII keys to Sensitive, otherwise Internal.
pub fn classify_fields(fields: &serde_json::Map<String, serde_json::Value>) -> Sensitivity {
    let mut max = Sensitivity::Internal;
    for key in fields.keys() {
        if is_secret_field(key) {
            return Sensitivity::Secret;
        }
        if is_pii_field(key) && max < Sensitivity::Sensitive {
            max = Sensitivity::Sensitive;
        }
    }
    max
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secret_fields_detect() {
        assert!(is_secret_field("password"));
        assert!(is_secret_field("X-Api-Key"));
        assert!(!is_secret_field("user_id"));
    }

    #[test]
    fn classify_escalates() {
        let m = json!({"user": 1}).as_object().unwrap().clone();
        assert_eq!(classify_fields(&m), Sensitivity::Internal);
        let m = json!({"email": "a@b.c"}).as_object().unwrap().clone();
        assert_eq!(classify_fields(&m), Sensitivity::Sensitive);
        let m = json!({"password": "x"}).as_object().unwrap().clone();
        assert_eq!(classify_fields(&m), Sensitivity::Secret);
    }
}
