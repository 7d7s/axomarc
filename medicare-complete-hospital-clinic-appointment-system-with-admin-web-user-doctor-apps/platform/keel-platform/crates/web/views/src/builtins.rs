//! The framework's built-in view models.
//!
//! The framework ships the view models an application would otherwise
//! write again: the error page, the pagination window, the table, the
//! form, the empty state, the operations dashboard. Each one is an
//! ordinary [`ViewModel`] — a typed struct that declares the view it
//! serves and the fields that view may read — so an application that
//! wants a different error page registers its own model over the
//! built-in (the last registration wins).
//!
//! The error page is here first because it is the one every deployment
//! has: a failure must render a page, and that page's data requirements
//! are as much a contract as any other view's.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;
use regex::Regex;

use crate::registry::{ViewBinding, ViewRegistry};
use crate::view_schema;
use crate::viewmodel::{FieldKind, FieldSchema, ViewModel};

// Static regexes for validation (created once, reused many times)
static EMAIL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$").expect("valid email regex")
});
static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https?://[^\s/$.?#].[^\s]*$").expect("valid URL regex")
});

/// The error page's model.
///
/// The error page is a **family** of views (404, 500, maintenance), so
/// the model names the family; the registry binds each view of the family
/// to it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorVm {
    /// The HTTP status the page reports.
    pub status: u16,
    /// The status' canonical reason ("Not Found").
    pub title: String,
    /// The public message - never the internal detail.
    pub message: String,
    /// The error's stable code.
    pub code: String,
    /// The path the failure happened on.
    pub path: String,
    /// The debug detail, present only when the deployment opens the
    /// debug gate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<Value>,
}

impl ViewModel for ErrorVm {
    const VIEW: &'static str = "errors";

    fn schema() -> &'static [FieldSchema] {
        view_schema![
            FieldSchema::required("status", FieldKind::Int),
            FieldSchema::required("title", FieldKind::String),
            FieldSchema::required("message", FieldKind::String),
            FieldSchema::required("code", FieldKind::String),
            FieldSchema::required("path", FieldKind::String),
            FieldSchema::optional("debug", FieldKind::Object),
        ]
    }
}

/// The pagination view model.
///
/// Renders a page window with first/last/prev/next links and page numbers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginationVm {
    /// The current page (1-indexed).
    pub current_page: u64,
    /// The last page number.
    pub last_page: u64,
    /// Total number of items.
    pub total: u64,
    /// Items per page.
    pub per_page: u64,
    /// The page window to display (e.g., [1, 2, 3, 4, 5]).
    pub pages: Vec<u64>,
    /// The base URL for pagination links.
    pub base_url: String,
    /// Query parameters to preserve.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<serde_json::Map<String, Value>>,
}

impl ViewModel for PaginationVm {
    const VIEW: &'static str = "pagination";

    fn schema() -> &'static [FieldSchema] {
        view_schema![
            FieldSchema::required("current_page", FieldKind::Int),
            FieldSchema::required("last_page", FieldKind::Int),
            FieldSchema::required("total", FieldKind::Int),
            FieldSchema::required("per_page", FieldKind::Int),
            FieldSchema::required("pages", FieldKind::List),
            FieldSchema::required("base_url", FieldKind::String),
            FieldSchema::optional("query", FieldKind::Object),
        ]
    }
}

/// A table column definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableColumn {
    /// The column key (matches the row object's key).
    pub key: String,
    /// The column header label.
    pub label: String,
    /// Whether the column is sortable.
    #[serde(default)]
    pub sortable: bool,
    /// The sort direction if sorted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_direction: Option<String>,
}

/// The table view model.
///
/// Renders a data table with columns, rows, and optional sorting/pagination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableVm {
    /// The table's columns.
    pub columns: Vec<TableColumn>,
    /// The table's rows (each row is an object keyed by column key).
    pub rows: Vec<serde_json::Map<String, Value>>,
    /// Optional pagination for the table.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pagination: Option<PaginationVm>,
    /// Optional empty state message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty_message: Option<String>,
}

impl ViewModel for TableVm {
    const VIEW: &'static str = "table";

    fn schema() -> &'static [FieldSchema] {
        view_schema![
            FieldSchema::required("columns", FieldKind::List),
            FieldSchema::required("rows", FieldKind::List),
            FieldSchema::optional("pagination", FieldKind::Object),
            FieldSchema::optional("empty_message", FieldKind::String),
        ]
    }
}

/// A validation rule for a form field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ValidationRule {
    /// Field must not be empty.
    Required,
    /// String must match a regex pattern.
    Pattern(String),
    /// String length must be at least min.
    MinLength(usize),
    /// String length must be at most max.
    MaxLength(usize),
    /// Number must be at least min.
    Min(f64),
    /// Number must be at most max.
    Max(f64),
    /// Must be a valid email format.
    Email,
    /// Must be a valid URL format.
    Url,
    /// Custom validation by name (resolved through the engine's conditional registry).
    Custom(String),
}

/// A form field definition with validation rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormField {
    /// The field name.
    pub name: String,
    /// The field label.
    pub label: String,
    /// The field type (text, email, password, select, textarea, checkbox, radio, hidden, number, date).
    pub field_type: String,
    /// Whether the field is required.
    #[serde(default)]
    pub required: bool,
    /// Validation rules for this field.
    #[serde(default)]
    pub validation_rules: Vec<ValidationRule>,
    /// The field's current value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// Validation error message for this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Options for select/radio fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<FormOption>>,
    /// Placeholder text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

/// An option for select/radio fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FormOption {
    /// The option value.
    pub value: String,
    /// The option label.
    pub label: String,
    /// Whether this option is selected.
    #[serde(default)]
    pub selected: bool,
}

/// The form view model.
///
/// Renders a form with fields, validation errors, and CSRF protection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormVm {
    /// The form's action URL.
    pub action: String,
    /// The form's HTTP method.
    pub method: String,
    /// The form's fields.
    pub fields: Vec<FormField>,
    /// The CSRF token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csrf_token: Option<String>,
    /// General form-level error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ViewModel for FormVm {
    const VIEW: &'static str = "form";

    fn schema() -> &'static [FieldSchema] {
        view_schema![
            FieldSchema::required("action", FieldKind::String),
            FieldSchema::required("method", FieldKind::String),
            FieldSchema::required("fields", FieldKind::List),
            FieldSchema::optional("csrf_token", FieldKind::String),
            FieldSchema::optional("error", FieldKind::String),
        ]
    }
}

/// Validation logic for FormVm - separate from the ViewModel trait implementation.
impl FormVm {
    /// Validate the form's fields against their declared rules.
    /// Returns a vector of field errors (name -> error message).
    /// If the form is valid, returns an empty vector.
    pub fn validate(&self, submitted: &serde_json::Map<String, Value>) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        for field in &self.fields {
            let submitted_value = submitted.get(&field.name);
            
            // Check required
            if field.required {
                let is_empty = match submitted_value {
                    Some(Value::String(s)) => s.trim().is_empty(),
                    Some(Value::Null) | None => true,
                    _ => false,
                };
                if is_empty {
                    errors.push((field.name.clone(), format!("{} is required", field.label)));
                    continue;
                }
            }
            
            // Skip validation if no value provided and not required
            let Some(value) = submitted_value else { continue };
            
            // Validate each rule
            for rule in &field.validation_rules {
                let error_msg = match rule {
                    ValidationRule::Required => None, // Already checked above
                    ValidationRule::Pattern(pattern) => {
                        if let Value::String(s) = value {
                            let re = regex::Regex::new(pattern).ok();
                            if !re.map(|r| r.is_match(s)).unwrap_or(false) {
                                Some(format!("{} does not match required pattern", field.label))
                            } else { None }
                        } else { None }
                    }
                    ValidationRule::MinLength(min) => {
                        if let Value::String(s) = value {
                            if s.chars().count() < *min {
                                Some(format!("{} must be at least {} characters", field.label, min))
                            } else { None }
                        } else { None }
                    }
                    ValidationRule::MaxLength(max) => {
                        if let Value::String(s) = value {
                            if s.chars().count() > *max {
                                Some(format!("{} must be at most {} characters", field.label, max))
                            } else { None }
                        } else { None }
                    }
                    ValidationRule::Min(min) => {
                        if let Value::Number(n) = value {
                            if let Some(n) = n.as_f64() {
                                if n < *min {
                                    Some(format!("{} must be at least {}", field.label, min))
                                } else { None }
                            } else { None }
                        } else { None }
                    }
                    ValidationRule::Max(max) => {
                        if let Value::Number(n) = value {
                            if let Some(n) = n.as_f64() {
                                if n > *max {
                                    Some(format!("{} must be at most {}", field.label, max))
                                } else { None }
                            } else { None }
                        } else { None }
                    }
                    ValidationRule::Email => {
                        if let Value::String(s) = value {
                            if !EMAIL_REGEX.is_match(s) {
                                Some(format!("{} must be a valid email address", field.label))
                            } else { None }
                        } else { None }
                    }
                    ValidationRule::Url => {
                        if let Value::String(s) = value {
                            if !URL_REGEX.is_match(s) {
                                Some(format!("{} must be a valid URL", field.label))
                            } else { None }
                        } else { None }
                    }
                    ValidationRule::Custom(_name) => {
                        // Custom validations would be resolved through the engine's conditional registry
                        // For now, we skip custom validations in this basic implementation
                        None
                    }
                };
                
                if let Some(msg) = error_msg {
                    errors.push((field.name.clone(), msg));
                    break; // Stop at first error per field
                }
            }
        }
        errors
    }
}

/// The empty state view model.
///
/// Renders an empty state illustration with a message and optional action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmptyVm {
    /// The empty state title.
    pub title: String,
    /// The empty state message.
    pub message: String,
    /// Optional action button label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_label: Option<String>,
    /// Optional action button URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_url: Option<String>,
    /// Optional illustration/icon name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub illustration: Option<String>,
}

impl ViewModel for EmptyVm {
    const VIEW: &'static str = "empty";

    fn schema() -> &'static [FieldSchema] {
        view_schema![
            FieldSchema::required("title", FieldKind::String),
            FieldSchema::required("message", FieldKind::String),
            FieldSchema::optional("action_label", FieldKind::String),
            FieldSchema::optional("action_url", FieldKind::String),
            FieldSchema::optional("illustration", FieldKind::String),
        ]
    }
}

/// The operations dashboard view model.
///
/// Renders the ops dashboard with queue, jobs, schedule, and trace sections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpsVm {
    /// The dashboard title.
    pub title: String,
    /// When the dashboard was generated.
    pub generated_at: String,
    /// The refresh interval in seconds.
    pub refresh_secs: u64,
    /// The dashboard sections.
    pub sections: serde_json::Map<String, Value>,
}

impl ViewModel for OpsVm {
    const VIEW: &'static str = "dashboard.ops";

    fn schema() -> &'static [FieldSchema] {
        view_schema![
            FieldSchema::required("title", FieldKind::String),
            FieldSchema::required("generated_at", FieldKind::String),
            FieldSchema::required("refresh_secs", FieldKind::Int),
            FieldSchema::required("sections", FieldKind::Object),
        ]
    }
}

/// The registry the framework ships: the built-in views, bound to the
/// built-in view models.
///
/// An application registers its own bindings on top of this one, so a
/// deployment can replace any built-in view with its own model.
pub fn builtin_registry() -> ViewRegistry {
    let mut registry = ViewRegistry::new();
    for view in ["errors.404", "errors.500", "errors.maintenance"] {
        registry.register(ViewBinding::new::<ErrorVm>(view));
    }
    registry.register(ViewBinding::new::<PaginationVm>("pagination"));
    registry.register(ViewBinding::new::<TableVm>("table"));
    registry.register(ViewBinding::new::<FormVm>("form"));
    registry.register(ViewBinding::new::<EmptyVm>("empty"));
    registry.register(ViewBinding::new::<OpsVm>("dashboard.ops"));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewmodel::view_model_value;

    #[test]
    fn a_built_in_view_is_registered_by_the_framework() {
        let registry = builtin_registry();
        assert_eq!(registry.len(), 8);
        for view in ["errors.404", "errors.500", "errors.maintenance"] {
            let binding = registry
                .binding(view)
                .unwrap_or_else(|| panic!("{view} is registered"));
            assert!(
                binding.model.contains("ErrorVm"),
                "{view} binds to the error model: {}",
                binding.model
            );
            assert_eq!(binding.schema.len(), 6);
        }
        for (view, expected_model, expected_schema_len) in [
            ("pagination", "PaginationVm", 7),
            ("table", "TableVm", 4),
            ("form", "FormVm", 5),
            ("empty", "EmptyVm", 5),
            ("dashboard.ops", "OpsVm", 4),
        ] {
            let binding = registry
                .binding(view)
                .unwrap_or_else(|| panic!("{view} is registered"));
            assert!(
                binding.model.contains(expected_model),
                "{view} binds to {}: {}",
                expected_model,
                binding.model
            );
            assert_eq!(binding.schema.len(), expected_schema_len, "{view} schema len");
        }
    }

    #[test]
    fn the_error_model_carries_the_status_and_the_code() {
        let value = view_model_value(&ErrorVm {
            status: 404,
            title: "Not Found".to_string(),
            message: "the page does not exist".to_string(),
            code: "NOT-FOUND".to_string(),
            path: "/missing".to_string(),
            debug: None,
        })
        .expect("the model honours its declaration");
        assert_eq!(value["status"], serde_json::json!(404));
        assert_eq!(value["title"], serde_json::json!("Not Found"));
        assert_eq!(value["code"], serde_json::json!("NOT-FOUND"));
        assert_eq!(value["path"], serde_json::json!("/missing"));
        assert!(
            value.get("debug").is_none(),
            "the debug detail is absent unless the deployment opens the gate: {value}"
        );
    }

    #[test]
    fn the_error_model_carries_the_debug_detail_when_there_is_one() {
        let value = view_model_value(&ErrorVm {
            status: 500,
            title: "Internal Server Error".to_string(),
            message: "a database error occurred".to_string(),
            code: "INTERNAL".to_string(),
            path: "/boom".to_string(),
            debug: Some(serde_json::json!({"cause": "redacted"})),
        })
        .expect("the model honours its declaration");
        assert_eq!(value["debug"], serde_json::json!({"cause": "redacted"}));
    }

    #[test]
    fn the_pagination_view_model_carries_the_window() {
        let value = view_model_value(&PaginationVm {
            current_page: 3,
            last_page: 10,
            total: 100,
            per_page: 10,
            pages: vec![1, 2, 3, 4, 5],
            base_url: "/items".to_string(),
            query: None,
        })
        .expect("the model honours its declaration");
        assert_eq!(value["current_page"], serde_json::json!(3));
        assert_eq!(value["last_page"], serde_json::json!(10));
        assert_eq!(value["total"], serde_json::json!(100));
        assert_eq!(value["per_page"], serde_json::json!(10));
        assert_eq!(value["pages"], serde_json::json!([1, 2, 3, 4, 5]));
        assert_eq!(value["base_url"], serde_json::json!("/items"));
    }

    #[test]
    fn the_table_view_model_declares_its_columns() {
        let value = view_model_value(&TableVm {
            columns: vec![
                TableColumn {
                    key: "id".to_string(),
                    label: "ID".to_string(),
                    sortable: true,
                    sort_direction: Some("asc".to_string()),
                },
                TableColumn {
                    key: "name".to_string(),
                    label: "Name".to_string(),
                    sortable: true,
                    sort_direction: None,
                },
            ],
            rows: vec![],
            pagination: None,
            empty_message: None,
        })
        .expect("the model honours its declaration");
        assert_eq!(value["columns"].as_array().unwrap().len(), 2);
        assert_eq!(value["columns"][0]["key"], serde_json::json!("id"));
        assert_eq!(value["columns"][0]["label"], serde_json::json!("ID"));
        assert_eq!(value["columns"][0]["sortable"], serde_json::json!(true));
        assert_eq!(value["rows"], serde_json::json!([]));
    }

    #[test]
    fn the_form_view_model_carries_fields_and_errors() {
        let value = view_model_value(&FormVm {
            action: "/submit".to_string(),
            method: "POST".to_string(),
            fields: vec![
                FormField {
                    name: "email".to_string(),
                    label: "Email".to_string(),
                    field_type: "email".to_string(),
                    required: true,
                    validation_rules: vec![],
                    value: Some(serde_json::json!("test@example.com")),
                    error: None,
                    options: None,
                    placeholder: Some("Enter email".to_string()),
                },
                FormField {
                    name: "password".to_string(),
                    label: "Password".to_string(),
                    field_type: "password".to_string(),
                    required: true,
                    validation_rules: vec![],
                    value: None,
                    error: Some("too short".to_string()),
                    options: None,
                    placeholder: None,
                },
            ],
            csrf_token: Some("token123".to_string()),
            error: None,
        })
        .expect("the model honours its declaration");
        assert_eq!(value["action"], serde_json::json!("/submit"));
        assert_eq!(value["method"], serde_json::json!("POST"));
        assert_eq!(value["fields"].as_array().unwrap().len(), 2);
        assert_eq!(value["fields"][0]["name"], serde_json::json!("email"));
        assert_eq!(value["fields"][0]["required"], serde_json::json!(true));
        assert_eq!(value["fields"][1]["error"], serde_json::json!("too short"));
        assert_eq!(value["csrf_token"], serde_json::json!("token123"));
    }

    #[test]
    fn the_empty_view_model_carries_message_and_action() {
        let value = view_model_value(&EmptyVm {
            title: "No appointments".to_string(),
            message: "You have no appointments yet.".to_string(),
            action_label: Some("Book one".to_string()),
            action_url: Some("/appointments/new".to_string()),
            illustration: Some("calendar".to_string()),
        })
        .expect("the model honours its declaration");
        assert_eq!(value["title"], serde_json::json!("No appointments"));
        assert_eq!(value["message"], serde_json::json!("You have no appointments yet."));
        assert_eq!(value["action_label"], serde_json::json!("Book one"));
        assert_eq!(value["action_url"], serde_json::json!("/appointments/new"));
        assert_eq!(value["illustration"], serde_json::json!("calendar"));
    }

    #[test]
    fn the_ops_view_model_carries_sections() {
        let mut sections = serde_json::Map::new();
        sections.insert("queue".to_string(), serde_json::json!({"depth": 5}));
        sections.insert("jobs".to_string(), serde_json::json!({"rows": []}));

        let value = view_model_value(&OpsVm {
            title: "Operations".to_string(),
            generated_at: "2026-10-22T10:00:00Z".to_string(),
            refresh_secs: 10,
            sections,
        })
        .expect("the model honours its declaration");
        assert_eq!(value["title"], serde_json::json!("Operations"));
        assert_eq!(value["generated_at"], serde_json::json!("2026-10-22T10:00:00Z"));
        assert_eq!(value["refresh_secs"], serde_json::json!(10));
        assert!(value["sections"]["queue"]["depth"].is_number());
    }
}
