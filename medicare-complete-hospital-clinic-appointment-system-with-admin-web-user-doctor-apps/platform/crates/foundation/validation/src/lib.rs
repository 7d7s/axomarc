//! # keel-validation
//! Boundary validation: small composable primitives PLUS the collect-all
//! Validator (S2 item 17, FormRequest role). Error-uniform (AppError).
//! Used by Request ViewModels; domain invariants stay in the domain.
//!
//! Wave 4: `validator::Rule` is the 1:1 manifest grammar (types, strings,
//! numeric incl. field refs; conditionals/cross-field/date/file/auth/DB
//! rules land in the following phases), evaluated by the private `checks`
//! module against a `ValidationContext` supplied by the transport edge.

mod checks;
pub mod validator;

pub use validator::{
    parse_rules, FieldSpec, FieldViolation, NumTarget, Rule, UploadMeta, ValidationContext,
    ValidationReport, Validator,
};

use keel_foundation::AppError;

pub fn non_empty(field: &str, value: &str) -> Result<(), AppError> {
    if value.trim().is_empty() {
        return Err(AppError::validation(format!("{field} is required")).with_meta("field", field));
    }
    Ok(())
}

pub fn max_len(field: &str, value: &str, max: usize) -> Result<(), AppError> {
    if value.chars().count() > max {
        return Err(
            AppError::validation(format!("{field} exceeds {max} characters"))
                .with_meta("field", field),
        );
    }
    Ok(())
}

pub fn one_of(field: &str, value: &str, allowed: &[&str]) -> Result<(), AppError> {
    if !allowed.contains(&value) {
        return Err(AppError::validation(format!(
            "{field} must be one of: {}",
            allowed.join(", ")
        ))
        .with_meta("field", field));
    }
    Ok(())
}

pub fn numeric_range(field: &str, value: i64, min: i64, max: i64) -> Result<(), AppError> {
    if !(min..=max).contains(&value) {
        return Err(
            AppError::validation(format!("{field} must be {min}..={max}"))
                .with_meta("field", field),
        );
    }
    Ok(())
}

pub fn phone_e164(field: &str, value: &str) -> Result<(), AppError> {
    let ok = value.starts_with('+')
        && value.len() >= 8
        && value.len() <= 16
        && value[1..].chars().all(|c| c.is_ascii_digit());
    if !ok {
        return Err(
            AppError::validation(format!("{field} must be E.164 (+919876543210)"))
                .with_meta("field", field),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives() {
        assert!(non_empty("name", "  ").is_err());
        assert!(max_len("reason", "abc", 2).is_err());
        assert!(one_of("kind", "cat", &["self", "family"]).is_err());
        assert!(one_of("kind", "self", &["self", "family"]).is_ok());
        assert!(numeric_range("age", 150, 0, 130).is_err());
        assert!(phone_e164("phone", "+919876543210").is_ok());
        assert!(phone_e164("phone", "9876543210").is_err());
    }
}
