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

use serde::Serialize;
use serde_json::Value;

use crate::registry::{ViewBinding, ViewRegistry};
use crate::view_schema;
use crate::viewmodel::{FieldKind, FieldSchema, ViewModel};

/// The error page's model.
///
/// The error page is a **family** of views (404, 500, maintenance), so
/// the model names the family; the registry binds each view of the family
/// to it.
#[derive(Debug, Clone, Serialize)]
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
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewmodel::view_model_value;

    #[test]
    fn a_built_in_view_is_registered_by_the_framework() {
        let registry = builtin_registry();
        assert_eq!(registry.len(), 3);
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
}
