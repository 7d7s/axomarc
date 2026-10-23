//! Form bridge: the one-definition rule for forms.
//!
//! A **form** is a single definition that works in both planes:
//!
//! - Wire plane: implements `RequestViewModel` for JSON API validation.
//! - HTML plane: a [`FormVm`] renders it into a template.
//!
//! The validation rules are the single source of truth: one `FormField`
//! list drives the wire plane's input contract and the HTML plane's
//! rendered fields, values, errors and CSRF token.

use crate::builtins::{FormField, FormVm};
use kelson_foundation::AppError;
use kelson_http::RequestViewModel;

/// A form that bridges the wire plane (`RequestViewModel`) and the HTML
/// plane (`ViewModel`).
#[derive(Debug, Clone)]
pub struct BridgedForm {
    /// The form's action URL.
    pub action: String,
    /// The form's HTTP method.
    pub method: String,
    /// The form's fields - the one shared definition.
    pub fields: Vec<FormField>,
    /// The CSRF token, present once the framework assembles it.
    pub csrf_token: Option<String>,
    /// The general form-level error, filled by a failed validation.
    pub error: Option<String>,
}

impl RequestViewModel for BridgedForm {
    type Input = serde_json::Map<String, serde_json::Value>;

    /// The wire plane's input contract: every declared field name with
    /// its submitted value (null when absent).
    fn validate(self) -> Result<Self::Input, AppError> {
        Ok(self
            .fields
            .iter()
            .map(|f| {
                (
                    f.name.clone(),
                    f.value.clone().unwrap_or(serde_json::Value::Null),
                )
            })
            .collect())
    }
}

impl BridgedForm {
    /// Create a new bridged form from a form definition.
    pub fn new(
        action: impl Into<String>,
        method: impl Into<String>,
        fields: Vec<FormField>,
    ) -> Self {
        Self {
            action: action.into(),
            method: method.into(),
            fields,
            csrf_token: None,
            error: None,
        }
    }

    /// Fill each field's value from the submission. The declared field
    /// list stays authoritative in both directions: a submission key no
    /// field declared is dropped, and a missing key resets the field to
    /// empty (the re-render shows the submission as it arrived).
    pub fn populate(&mut self, submitted: &serde_json::Map<String, serde_json::Value>) {
        for field in &mut self.fields {
            field.value = submitted.get(&field.name).cloned();
        }
    }

    /// Validate the submission **on the shared definition**: populate the
    /// fields, run the declared rules, and — on failure — fill the
    /// form-level error with the field messages the re-render reads.
    pub fn validate_with_data(&mut self, submitted: &serde_json::Map<String, serde_json::Value>) -> bool {
        self.populate(submitted);
        let form_vm = FormVm {
            action: self.action.clone(),
            method: self.method.clone(),
            fields: self.fields.clone(),
            csrf_token: self.csrf_token.clone(),
            error: None,
        };
        let errors = form_vm.validate(
            &self
                .fields
                .iter()
                .map(|f| {
                    (
                        f.name.clone(),
                        f.value.clone().unwrap_or(serde_json::Value::Null),
                    )
                })
                .collect(),
        );
        if errors.is_empty() {
            true
        } else {
            self.error = Some(
                errors
                    .iter()
                    .map(|(n, e)| format!("{n}: {e}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::ValidationRule;

    fn field(name: &str, field_type: &str, required: bool, rules: Vec<ValidationRule>) -> FormField {
        FormField {
            name: name.to_string(),
            label: name.to_string(),
            field_type: field_type.to_string(),
            required,
            validation_rules: rules,
            value: None,
            error: None,
            options: None,
            placeholder: None,
        }
    }

    fn submitted(pairs: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn a_bridged_form_validates_submitted_data() {
        let form = BridgedForm::new(
            "/submit",
            "POST",
            vec![
                field("name", "text", true, vec![ValidationRule::Required, ValidationRule::MinLength(2)]),
                field("email", "email", true, vec![ValidationRule::Required, ValidationRule::Email]),
            ],
        );

        // The valid submission passes on the same definition the render
        // reads, and carries no form-level error.
        let mut form = form.clone();
        let ok = submitted(&[("name", "John".into()), ("email", "john@example.com".into())]);
        assert!(form.validate_with_data(&ok));
        assert!(form.error.is_none());

        // The wire plane's input contract echoes the submission.
        let input = form.clone().validate().expect("the input describes the form");
        assert_eq!(input.get("name").and_then(|v| v.as_str()), Some("John"));

        // A missing required field fails and fills the form-level error
        // with the field messages.
        let mut form = form.clone();
        let missing = submitted(&[("email", "john@example.com".into())]);
        assert!(!form.validate_with_data(&missing));
        let error = form.error.clone().expect("the failure fills the error");
        assert!(error.contains("name"), "{error}");

        // A rule-violating email fails on the declared rule alone.
        let mut form = form.clone();
        let bad = submitted(&[("name", "John".into()), ("email", "not-an-email".into())]);
        assert!(!form.validate_with_data(&bad));
        assert!(
            form.error.as_ref().expect("filled").contains("email"),
            "the rule message names the field: {}",
            form.error.clone().unwrap()
        );
    }

    #[test]
    fn the_wire_plane_drops_a_key_no_field_declared() {
        let form = BridgedForm::new(
            "/submit",
            "POST",
            vec![field("name", "text", true, vec![ValidationRule::Required])],
        );
        let noisy = submitted(&[("name", "John".into()), ("extra", "drop me".into())]);
        let mut form = form.clone();
        assert!(form.validate_with_data(&noisy));
        let input = form.validate().expect("the input describes the form");
        assert!(
            input.get("extra").is_none(),
            "the declared field list is the one definition - an undeclared key does not enter"
        );
        assert_eq!(input.get("name").and_then(|v| v.as_str()), Some("John"));
    }
}
