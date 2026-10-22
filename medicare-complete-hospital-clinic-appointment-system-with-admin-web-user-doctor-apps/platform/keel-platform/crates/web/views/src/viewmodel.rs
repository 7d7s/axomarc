//! The ViewModel: the presentation state of one view.
//!
//! A **view model** is the typed, serialisable state a template renders
//! from. It is the Model-View-ViewModel layer's middle: it knows the
//! domain result, it does not know the view.
//!
//! Three properties, and each one is enforced rather than documented:
//!
//! * **the view's data requirements are declared.** A view model declares
//!   the fields a template may read ([`ViewModel::schema`]), so the
//!   requirements are a contract instead of a set of string keys that
//!   quietly drift apart;
//! * **the declared fields are checked.** A required field that is missing
//!   is refused, and a field whose value does not match its declared kind
//!   is refused — fail-closed, like the rest of the engine. A view model
//!   that does not honour its own declaration never reaches a template;
//! * **the escape hatch is explicit.** [`ViewModel::extra`] merges raw
//!   JSON for the rare dynamic field. Those keys are *undeclared* by
//!   definition, and the declared fields win a collision, so the typed
//!   model stays authoritative.
//!
//! A view model is a **value, not a session**: it is built per request,
//! serialised, and discarded. A view that wants live state reads a read
//! model (a declared, refreshable projection with a staleness surface),
//! never a mutable object in the request path.
//!
//! The binder — how a template reaches a view model — lands with the
//! `@vm` directive; this module provides the vocabulary it binds to.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::engine::{ViewError, VIEW_MODEL};
use crate::expr::Segment;

/// The namespace a bound view model is read from.
///
/// The binder (`@vm`) puts the model's value here, and every `vm.*` path
/// a template reads is checked against the model's declaration.
pub const VM_NAMESPACE: &str = "vm";

/// The kind of a declared field.
///
/// The kinds describe what a **template may read**, not how it is
/// rendered: escaping is the template's business, so there is no "raw
/// HTML" kind here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// A text value.
    String,
    /// A whole number.
    Int,
    /// A fractional number.
    Float,
    /// A boolean.
    Bool,
    /// An array.
    List,
    /// An object.
    Object,
    /// A timestamp: a string in RFC 3339 form.
    Timestamp,
}

impl FieldKind {
    /// Whether a value satisfies this kind.
    ///
    /// The check is deliberately narrow: an integer does **not** satisfy
    /// `Float` (a template that formats `3` and `3.0` differently must
    /// declare the kind it means), and a timestamp is a string (the
    /// value's shape, not its meaning).
    pub fn accepts(&self, value: &Value) -> bool {
        match self {
            FieldKind::String | FieldKind::Timestamp => value.is_string(),
            FieldKind::Int => value.is_i64() || value.is_u64(),
            FieldKind::Float => value.is_f64(),
            FieldKind::Bool => value.is_boolean(),
            FieldKind::List => value.is_array(),
            FieldKind::Object => value.is_object(),
        }
    }

    /// The kind's name, for the error message.
    pub fn as_str(&self) -> &'static str {
        match self {
            FieldKind::String => "string",
            FieldKind::Int => "int",
            FieldKind::Float => "float",
            FieldKind::Bool => "bool",
            FieldKind::List => "list",
            FieldKind::Object => "object",
            FieldKind::Timestamp => "timestamp",
        }
    }
}

/// One declared data requirement of a view.
///
/// The `path` is a dotted name (`"user.name"`); the root of the path is
/// the model's own field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSchema {
    /// The dotted path of the field.
    pub path: &'static str,
    /// What the field's value must be.
    pub kind: FieldKind,
    /// Whether the field must be present.
    pub required: bool,
}

impl FieldSchema {
    /// A required field of a kind.
    pub const fn required(path: &'static str, kind: FieldKind) -> Self {
        Self {
            path,
            kind,
            required: true,
        }
    }

    /// An optional field of a kind.
    pub const fn optional(path: &'static str, kind: FieldKind) -> Self {
        Self {
            path,
            kind,
            required: false,
        }
    }
}

/// The presentation state of one view.
pub trait ViewModel: Serialize {
    /// The template this model renders into.
    const VIEW: &'static str;

    /// The view's declared data requirements: the fields a template may
    /// read.
    fn schema() -> &'static [FieldSchema];

    /// The escape hatch: raw JSON merged into the serialised model.
    ///
    /// The merged keys are undeclared by definition; the declared fields
    /// win a collision.
    fn extra(&self) -> Option<&Map<String, Value>> {
        None
    }
}

/// The declared field at a path, if the schema declares one.
pub fn schema_field<'a>(
    schema: &'a [FieldSchema],
    path: &str,
) -> Option<&'a FieldSchema> {
    schema.iter().find(|field| field.path == path)
}

/// The dotted key path of a segment path, ignoring array indices.
///
/// A declaration names the **keys** of a view model, so the indices are
/// dropped: `rows[0].name` is the path `rows.name`, and the shape inside
/// a declared list is the application's business.
pub fn key_path(path: &[Segment]) -> String {
    path.iter()
        .filter_map(|segment| match segment {
            Segment::Key(key) => Some(key.as_str()),
            Segment::Index(_) => None,
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// The declaration that governs a dotted path under the `vm` namespace.
///
/// The **longest declared prefix** wins. The second element of the pair
/// says whether the declaration is the path itself (`true`) or a prefix
/// of it (`false` - the path is nested under a declared object or list).
/// `None` means the path is undeclared, which the binder refuses.
///
/// The render-time check and the build-time lint share this function, so
/// the two can never disagree about what a template may read.
pub fn governing_field<'a>(
    schema: &'a [FieldSchema],
    path: &str,
) -> Option<(&'a FieldSchema, bool)> {
    let keys: Vec<&str> = path.split('.').filter(|key| !key.is_empty()).collect();
    for take in (1..=keys.len()).rev() {
        let candidate = keys[..take].join(".");
        if let Some(field) = schema_field(schema, &candidate) {
            return Some((field, take == keys.len()));
        }
    }
    None
}

/// Declare a view model's schema.
///
/// A `&'static [FieldSchema]` cannot be written inline (a temporary would
/// not live long enough), so the schema is a `static`. This macro writes
/// that `static` for you:
///
/// ```ignore
/// impl ViewModel for OpsVm {
///     const VIEW: &'static str = "dashboard.ops";
///     fn schema() -> &'static [FieldSchema] {
///         view_schema![
///             FieldSchema::required("depth", FieldKind::Int),
///             FieldSchema::optional("note", FieldKind::String),
///         ]
///     }
/// }
/// ```
#[macro_export]
macro_rules! view_schema {
    ($($field:expr),* $(,)?) => {{
        static SCHEMA: &[$crate::viewmodel::FieldSchema] = &[$($field),*];
        SCHEMA
    }};
}

/// The value at a dotted path.
pub fn value_at<'v>(value: &'v Value, path: &str) -> Option<&'v Value> {
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = current.get(segment)?;
    }
    Some(current)
}

/// The context value a view model renders from.
///
/// The model's declared fields are serialised, the escape hatch is merged
/// under them, and the declaration is **checked**: a required field that
/// is missing, and a field whose value does not match its declared kind,
/// are refused. A view model that does not honour its own declaration
/// never reaches a template.
pub fn view_model_value<M: ViewModel + ?Sized>(model: &M) -> Result<Value, ViewError> {
    let mut value = match serde_json::to_value(model) {
        Ok(value) => value,
        Err(error) => {
            return Err(ViewError::new(
                VIEW_MODEL,
                format!("the view model {} does not serialise: {error}", M::VIEW),
            ))
        }
    };
    // The escape hatch goes UNDER the declared fields: the typed model is
    // authoritative, so a collision never lets raw JSON override it.
    if let Some(extra) = model.extra() {
        if let Some(object) = value.as_object_mut() {
            for (key, extra_value) in extra {
                if !object.contains_key(key) {
                    object.insert(key.clone(), extra_value.clone());
                }
            }
        }
    }
    check_schema(&value, M::schema(), M::VIEW)?;
    Ok(value)
}

/// Check a value against a declared schema.
///
/// The check is fail-closed: a required field that is absent (or null) is
/// an error, and a present field whose value does not match its declared
/// kind is an error.
pub fn check_schema(
    value: &Value,
    schema: &[FieldSchema],
    view: &str,
) -> Result<(), ViewError> {
    for field in schema {
        let found = value_at(value, field.path);
        match found {
            None | Some(Value::Null) => {
                if field.required {
                    return Err(ViewError::new(
                        VIEW_MODEL,
                        format!(
                            "the view model for {view} is missing the required field {:?}",
                            field.path
                        ),
                    ));
                }
            }
            Some(found) => {
                if !field.kind.accepts(found) {
                    return Err(ViewError::new(
                        VIEW_MODEL,
                        format!(
                            "the field {:?} of the view model for {view} is a {}, not a {}",
                            field.path,
                            value_kind(found),
                            field.kind.as_str()
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The name of a value's actual kind, for the error message.
///
/// The declared kinds and the runtime kinds are deliberately distinct
/// vocabularies: a declared `Int` is a number that is whole, while the
/// runtime kind of `3.5` is a `float`. The refusal names both.
pub fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(number) => {
            if number.is_f64() {
                "float"
            } else {
                "int"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A view model with two declared fields and one escape-hatch key.
    #[derive(Serialize)]
    struct OpsVm {
        depth: u64,
        title: String,
    }

    impl ViewModel for OpsVm {
        const VIEW: &'static str = "dashboard.ops";
        fn schema() -> &'static [FieldSchema] {
            crate::view_schema![
                FieldSchema::required("depth", FieldKind::Int),
                FieldSchema::required("title", FieldKind::String),
                FieldSchema::optional("note", FieldKind::String),
            ]
        }
    }

    /// A view model whose escape hatch collides with a declared field.
    #[derive(Serialize)]
    struct CollidingVm {
        depth: u64,
    }

    impl ViewModel for CollidingVm {
        const VIEW: &'static str = "colliding";
        fn schema() -> &'static [FieldSchema] {
            crate::view_schema![FieldSchema::required("depth", FieldKind::Int)]
        }
        fn extra(&self) -> Option<&Map<String, Value>> {
            static EXTRA: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
            Some(EXTRA.get_or_init(|| {
                let mut map = Map::new();
                map.insert("depth".to_string(), json!(999));
                map.insert("note".to_string(), json!("from the escape hatch"));
                map
            }))
        }
    }

    /// A view model that forgets a required field.
    #[derive(Serialize)]
    struct IncompleteVm {
        title: String,
    }

    impl ViewModel for IncompleteVm {
        const VIEW: &'static str = "incomplete";
        fn schema() -> &'static [FieldSchema] {
            crate::view_schema![
                FieldSchema::required("depth", FieldKind::Int),
                FieldSchema::required("title", FieldKind::String),
            ]
        }
    }

    /// A view model whose field is not the kind it declares.
    #[derive(Serialize)]
    struct WrongKindVm {
        depth: String,
    }

    impl ViewModel for WrongKindVm {
        const VIEW: &'static str = "wrong-kind";
        fn schema() -> &'static [FieldSchema] {
            crate::view_schema![FieldSchema::required("depth", FieldKind::Int)]
        }
    }

    #[test]
    fn a_view_model_declares_its_fields() {
        let schema = OpsVm::schema();
        assert_eq!(schema.len(), 3);
        assert_eq!(schema[0].path, "depth");
        assert_eq!(schema[0].kind, FieldKind::Int);
        assert!(schema[0].required);
        assert!(!schema[2].required, "an optional field is optional");
        assert_eq!(OpsVm::VIEW, "dashboard.ops");
        assert_eq!(
            schema_field(schema, "title").map(|f| f.kind),
            Some(FieldKind::String),
            "a declared field is found by its path"
        );
        assert!(
            schema_field(schema, "missing").is_none(),
            "an undeclared path is not found"
        );
    }

    #[test]
    fn a_view_model_serialises_to_the_context() {
        let value = view_model_value(&OpsVm {
            depth: 7,
            title: "queue".to_string(),
        })
        .expect("the model honours its declaration");
        assert_eq!(value["depth"], json!(7));
        assert_eq!(value["title"], json!("queue"));
        assert_eq!(
            value_at(&value, "depth"),
            Some(&json!(7)),
            "a dotted path reaches the value"
        );
        assert_eq!(value_at(&value, "depth.nope"), None);
    }

    #[test]
    fn the_escape_hatch_merges_undeclared_json() {
        let value = view_model_value(&CollidingVm { depth: 3 }).expect("the model serialises");
        assert_eq!(
            value["note"],
            json!("from the escape hatch"),
            "the escape hatch's undeclared key is merged"
        );
        assert_eq!(
            value["depth"],
            json!(3),
            "the declared field wins a collision: the typed model is authoritative"
        );
    }

    #[test]
    fn a_missing_required_field_is_refused() {
        let error = view_model_value(&IncompleteVm {
            title: "x".to_string(),
        })
        .expect_err("a model that forgets a required field is refused");
        assert_eq!(error.code, VIEW_MODEL);
        assert!(
            error.message.contains("depth"),
            "the refusal names the field: {}",
            error.message
        );
    }

    #[test]
    fn a_field_of_the_wrong_type_is_refused() {
        let error = view_model_value(&WrongKindVm {
            depth: "seven".to_string(),
        })
        .expect_err("a field of the wrong kind is refused");
        assert_eq!(error.code, VIEW_MODEL);
        assert!(
            error.message.contains("depth") && error.message.contains("int"),
            "the refusal names the field and the kind: {}",
            error.message
        );
    }

    #[test]
    fn the_declared_kinds_accept_the_values_they_describe() {
        assert!(FieldKind::String.accepts(&json!("x")));
        assert!(FieldKind::Timestamp.accepts(&json!("2026-01-01T00:00:00Z")));
        assert!(FieldKind::Int.accepts(&json!(3)));
        assert!(
            !FieldKind::Float.accepts(&json!(3)),
            "an integer is not a float: the declared kind must mean what it says"
        );
        assert!(FieldKind::Float.accepts(&json!(3.5)));
        assert!(FieldKind::Bool.accepts(&json!(true)));
        assert!(FieldKind::List.accepts(&json!([1, 2])));
        assert!(FieldKind::Object.accepts(&json!({"a": 1})));
        assert!(!FieldKind::Int.accepts(&json!("3")));
        assert!(!FieldKind::Object.accepts(&json!([1, 2])));
    }
}
