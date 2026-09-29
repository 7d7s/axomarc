//! OpenAPI 3.1 emission from the platform graph (L71 SDK generation, S6).
//!
//! The module manifests (`module.toml`) are the single source of truth:
//! every declared route becomes one operation. SDKs for any language are
//! generated downstream from this document with standard ecosystem tooling.
//! Emission is deterministic (sorted paths, stable shapes) so diffs are
//! reviewable and CI can gate on it.

use keel_registry::PlatformGraph;
use serde_json::json;

/// Wave 1: compile one field's manifest rule tokens into OpenAPI schema
/// constraints (required/min/max/one_of, type + format hints, numeric
/// comparisons). Tokens speak the same grammar as the HTTP edge
/// (keel_validation::Rule), so the SDK contract cannot drift from the
/// enforced validation. Emission stays exhaustive over the Rule enum —
/// new grammar lands here in the same commit.
fn field_constraints(tokens: &[String]) -> (serde_json::Map<String, serde_json::Value>, bool) {
    let mut schema = serde_json::Map::new();
    let mut required = false;
    // Type declaration tracked separately so `nullable` can widen it to a
    // 3.1 type array (["string", "null"]) instead of a separate keyword.
    let mut declared: Option<&'static str> = None;
    let mut nullable = false;
    let rules = keel_validation::parse_rules(tokens).unwrap_or_default();
    for rule in &rules {
        match rule {
            keel_validation::Rule::Required => required = true,
            keel_validation::Rule::Min(n) => {
                schema.insert("minLength".into(), json!(n));
            }
            keel_validation::Rule::Max(n) => {
                schema.insert("maxLength".into(), json!(n));
            }
            keel_validation::Rule::OneOf(values) => {
                schema.insert("enum".into(), json!(values));
            }
            keel_validation::Rule::Range(lo, hi) => {
                schema.insert("minimum".into(), json!(lo));
                schema.insert("maximum".into(), json!(hi));
            }
            keel_validation::Rule::Email => {
                schema.insert("format".into(), json!("email"));
            }
            keel_validation::Rule::Url => {
                schema.insert("format".into(), json!("uri"));
            }
            keel_validation::Rule::Uuid => {
                schema.insert("format".into(), json!("uuid"));
            }
            keel_validation::Rule::Date => {
                schema.insert("format".into(), json!("date-time"));
            }
            keel_validation::Rule::E164 => {}
            // Wave 4 types -> JSON Schema types (OpenAPI 3.1).
            keel_validation::Rule::Nullable => nullable = true,
            keel_validation::Rule::Boolean => declared = Some("boolean"),
            keel_validation::Rule::Integer => declared = Some("integer"),
            keel_validation::Rule::Numeric => declared = Some("number"),
            keel_validation::Rule::String => declared = Some("string"),
            keel_validation::Rule::Array => declared = Some("array"),
            // No standard JSON Schema hint for these; runtime enforces them.
            keel_validation::Rule::Json
            | keel_validation::Rule::Ascii
            | keel_validation::Rule::Alpha
            | keel_validation::Rule::AlphaNum
            | keel_validation::Rule::AlphaDash
            | keel_validation::Rule::Lowercase
            | keel_validation::Rule::Uppercase
            | keel_validation::Rule::StartsWith(_)
            | keel_validation::Rule::EndsWith(_)
            | keel_validation::Rule::Contains(_)
            | keel_validation::Rule::DoesntStartWith(_)
            | keel_validation::Rule::DoesntEndWith(_)
            | keel_validation::Rule::DoesntContain(_)
            | keel_validation::Rule::HexColor
            | keel_validation::Rule::MacAddress
            | keel_validation::Rule::Timezone
            | keel_validation::Rule::Ulid
            | keel_validation::Rule::Decimal(_)
            | keel_validation::Rule::Digits(_)
            | keel_validation::Rule::DigitsBetween(_, _)
            // Presence/conditional and cross-field rules describe WHEN a
            // field must validate, not its wire shape — no schema hints.
            | keel_validation::Rule::RequiredIf(_, _)
            | keel_validation::Rule::RequiredUnless(_, _)
            | keel_validation::Rule::RequiredWith(_)
            | keel_validation::Rule::RequiredWithout(_)
            | keel_validation::Rule::Accepted
            | keel_validation::Rule::AcceptedIf(_, _)
            | keel_validation::Rule::Declined
            | keel_validation::Rule::DeclinedIf(_, _)
            | keel_validation::Rule::Prohibited
            | keel_validation::Rule::ProhibitedIf(_, _)
            | keel_validation::Rule::ProhibitedWith(_)
            | keel_validation::Rule::Exclude
            | keel_validation::Rule::ExcludeIf(_, _)
            | keel_validation::Rule::Missing
            | keel_validation::Rule::MissingIf(_, _)
            | keel_validation::Rule::MissingWith(_)
            | keel_validation::Rule::Sometimes
            | keel_validation::Rule::Filled
            | keel_validation::Rule::Present
            | keel_validation::Rule::Same(_)
            | keel_validation::Rule::Different(_)
            | keel_validation::Rule::Confirmed
            | keel_validation::Rule::InArray(_)
            | keel_validation::Rule::Distinct
            | keel_validation::Rule::Bail
            // Dates/files/auth are runtime-only checks (comparisons target
            // sibling fields or stored parts; no static wire-shape hint).
            | keel_validation::Rule::DateFormat(_)
            | keel_validation::Rule::Before(_)
            | keel_validation::Rule::After(_)
            | keel_validation::Rule::BeforeOrEqual(_)
            | keel_validation::Rule::AfterOrEqual(_)
            | keel_validation::Rule::File
            | keel_validation::Rule::Image
            | keel_validation::Rule::Mimes(_)
            | keel_validation::Rule::MimeTypes(_)
            | keel_validation::Rule::Extensions(_)
            | keel_validation::Rule::Dimensions(_)
            | keel_validation::Rule::Password
            | keel_validation::Rule::CurrentPassword
            | keel_validation::Rule::Letters
            | keel_validation::Rule::Numbers
            | keel_validation::Rule::Symbols
            | keel_validation::Rule::MixedCase => {}
            keel_validation::Rule::MultipleOf(n) => {
                schema.insert("multipleOf".into(), json!(n));
            }
            // Numeric comparisons: 3.1 uses numeric exclusiveMinimum/Maximum.
            // Field-ref targets cannot be expressed statically — skipped.
            keel_validation::Rule::Gt(keel_validation::NumTarget::Literal(n)) => {
                schema.insert("exclusiveMinimum".into(), json!(n));
            }
            keel_validation::Rule::Gte(keel_validation::NumTarget::Literal(n)) => {
                schema.insert("minimum".into(), json!(n));
            }
            keel_validation::Rule::Lt(keel_validation::NumTarget::Literal(n)) => {
                schema.insert("exclusiveMaximum".into(), json!(n));
            }
            keel_validation::Rule::Lte(keel_validation::NumTarget::Literal(n)) => {
                schema.insert("maximum".into(), json!(n));
            }
            keel_validation::Rule::Gt(_)
            | keel_validation::Rule::Gte(_)
            | keel_validation::Rule::Lt(_)
            | keel_validation::Rule::Lte(_) => {}
        }
    }
    let ty = match (declared, nullable) {
        (Some(t), true) => json!([t, "null"]),
        (Some(t), false) => json!(t),
        (None, true) => json!(["string", "null"]),
        (None, false) => json!("string"),
    };
    schema.insert("type".into(), ty);
    (schema, required)
}

/// Wave 1: request body schema assembled from the module's [[validation]]
/// entries for one operation. None when the operation declares no rules.
fn request_body_schema(
    module: &keel_registry::ModuleManifest,
    operation: &str,
) -> Option<serde_json::Value> {
    let entries: Vec<&keel_registry::ValidationSpec> = module
        .validation
        .iter()
        .filter(|s| s.operation == operation)
        .collect();
    if entries.is_empty() {
        return None;
    }
    let mut properties = serde_json::Map::new();
    let mut required: Vec<String> = Vec::new();
    for spec in entries {
        let (schema, is_required) = field_constraints(&spec.rules);
        properties.insert(spec.field.clone(), serde_json::Value::Object(schema));
        if is_required {
            required.push(spec.field.clone());
        }
    }
    let mut schema = serde_json::Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), serde_json::Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), json!(required));
    }
    Some(serde_json::Value::Object(schema))
}

/// One path parameter detected inside a route path (`/api/v1/items/{id}`).
fn path_parameters(path: &str) -> Vec<serde_json::Value> {
    path.split('/')
        .filter(|segment| segment.starts_with('{') && segment.ends_with('}') && segment.len() > 2)
        .map(|segment| {
            json!({
                "name": &segment[1..segment.len() - 1],
                "in": "path",
                "required": true,
                "schema": { "type": "string" },
            })
        })
        .collect()
}

/// Response envelopes follow the frozen wire shape (11-SPEC §10):
/// success bodies are application envelopes; errors never leak internals.
fn responses() -> serde_json::Value {
    json!({
        "200": {
            "description": "Success envelope { ok: true, data: ... }",
            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Envelope" } } },
        },
        "default": {
            "description": "Error envelope { ok: false, error: { code, message, retryable } }",
            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ErrorEnvelope" } } },
        },
    })
}

/// Generate an OpenAPI 3.1 document from the compiled platform graph.
/// `title`/`version` describe the emitted API (usually the platform version).
pub fn emit_openapi(graph: &PlatformGraph, title: &str, version: &str) -> serde_json::Value {
    let mut paths: std::collections::BTreeMap<String, serde_json::Map<String, serde_json::Value>> =
        std::collections::BTreeMap::new();

    for module in &graph.modules {
        for route in &module.routes {
            let method = route.method.to_lowercase();
            let qualified_operation = if route.operation.contains('.') {
                route.operation.clone()
            } else {
                format!("{}.{}", module.name(), route.operation)
            };
            let mut operation = serde_json::Map::new();
            operation.insert(
                "operationId".into(),
                json!(qualified_operation.replace('.', "__")),
            );
            operation.insert("summary".into(), json!(qualified_operation));
            operation.insert("tags".into(), json!([module.name().to_string()]));
            let parameters = path_parameters(&route.path);
            if !parameters.is_empty() {
                operation.insert("parameters".into(), json!(parameters));
            }
            // Wave 1: manifest FormRequest constraints on the request body
            // (body methods only), plus the OpenAPI-native deprecation
            // marker for deprecated routes (L72).
            if matches!(method.as_str(), "post" | "put" | "patch") {
                if let Some(schema) = request_body_schema(module, &route.operation) {
                    let body_required = schema
                        .get("required")
                        .and_then(|v| v.as_array())
                        .map(|a| !a.is_empty())
                        .unwrap_or(false);
                    operation.insert(
                        "requestBody".into(),
                        json!({
                            "required": body_required,
                            "content": { "application/json": { "schema": schema } },
                        }),
                    );
                }
            }
            if route.deprecated {
                operation.insert("deprecated".into(), json!(true));
            }
            operation.insert("responses".into(), responses());
            paths
                .entry(route.path.clone())
                .or_default()
                .insert(method, serde_json::Value::Object(operation));
        }
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": title,
            "version": version,
            "description": "Generated from module.toml manifests (keel sdk:openapi). Authorization is capability-based and enforced by the execution engine; OpenAPI describes shape only.",
        },
        "paths": serde_json::Value::Object(paths.into_iter().map(|(path, methods)| (path, serde_json::Value::Object(methods))).collect()),
        "components": {
            "schemas": {
                "Envelope": {
                    "type": "object",
                    "description": "Frozen wire envelope (11-SPEC §10).",
                },
                "ErrorEnvelope": {
                    "type": "object",
                    "description": "{ ok: false, error: { code, message, retryable, meta? } } — internals never leak.",
                },
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_registry::ModuleManifest;

    fn manifest() -> ModuleManifest {
        ModuleManifest::from_toml(
            r#"
[module]
name = "appointments"
version = "0.1.0"
maturity = "L0"
plane = "tenant"
description = "Clinic appointments"

[capabilities]
book = { profile = "COMMAND" }
list = { profile = "READ" }

[[routes]]
method = "POST"
path = "/api/v1/appointments"
operation = "book"
capability = "book"

[[routes]]
method = "GET"
path = "/api/v1/appointments/{id}"
operation = "list"
capability = "list"
"#,
        )
        .expect("manifest parses")
    }

    #[test]
    fn openapi_follows_the_platform_graph() {
        let graph = PlatformGraph::compile(vec![manifest()]).expect("graph compiles");
        let doc = emit_openapi(&graph, "keel-platform", "0.1.0");
        assert_eq!(doc["openapi"], "3.1.0");
        assert_eq!(doc["info"]["title"], "keel-platform");

        let paths = doc["paths"].as_object().expect("paths object");
        assert_eq!(paths.len(), 2);
        let book = &doc["paths"]["/api/v1/appointments"]["post"];
        assert_eq!(book["operationId"], "appointments__book");
        assert_eq!(book["tags"][0], "appointments");
        assert!(book["responses"]["200"].is_object());

        let list = &doc["paths"]["/api/v1/appointments/{id}"]["get"];
        assert_eq!(list["operationId"], "appointments__list");
        assert_eq!(list["parameters"][0]["name"], "id");
        assert_eq!(list["parameters"][0]["in"], "path");
    }

    #[test]
    fn emission_is_deterministic() {
        let graph = PlatformGraph::compile(vec![manifest()]).expect("graph compiles");
        let first = emit_openapi(&graph, "t", "1").to_string();
        let second = emit_openapi(&graph, "t", "1").to_string();
        assert_eq!(first, second);
    }

    fn constrained_manifest() -> ModuleManifest {
        ModuleManifest::from_toml(
            r#"
[module]
name = "notes"
version = "0.1.0"
maturity = "L0"
plane = "tenant"
description = "Notes"

[capabilities]
create = { profile = "COMMAND" }

[[routes]]
method = "POST"
path = "/api/v1/notes"
operation = "create"
capability = "create"

[[routes]]
method = "GET"
path = "/api/v1/notes/legacy"
operation = "create"
capability = "create"
deprecated = true

[[validation]]
operation = "create"
field = "title"
rules = ["required", "max:120"]

[[validation]]
operation = "create"
field = "body"
rules = ["min:10"]

[[validation]]
operation = "create"
field = "kind"
rules = ["one_of:personal,work"]
"#,
        )
        .expect("manifest parses")
    }

    /// Wave 1: sdk:openapi emits required/min/max/one_of constraints from
    /// [[validation]] entries, and deprecated routes carry the
    /// OpenAPI-native marker.
    #[test]
    fn openapi_emits_validation_constraints_and_deprecation() {
        let graph = PlatformGraph::compile(vec![constrained_manifest()]).expect("graph compiles");
        let doc = emit_openapi(&graph, "keel-platform", "0.1.0");

        let create = &doc["paths"]["/api/v1/notes"]["post"];
        let schema = &create["requestBody"]["content"]["application/json"]["schema"];
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["title"]["maxLength"], 120);
        assert_eq!(schema["properties"]["body"]["minLength"], 10);
        assert_eq!(schema["properties"]["kind"]["enum"], json!(["personal", "work"]));
        let required = schema["required"].as_array().expect("required list");
        assert!(required.contains(&json!("title")));
        assert!(!required.contains(&json!("body")));
        assert_eq!(create["requestBody"]["required"], true);

        // Deprecated route carries the OpenAPI-native marker; GET routes
        // carry no request body.
        let legacy = &doc["paths"]["/api/v1/notes/legacy"]["get"];
        assert_eq!(legacy["deprecated"], true);
        assert!(legacy.get("requestBody").is_none());
    }

    /// Wave 4 Phase 1: type rules override the default `"type": "string"`
    /// (3.1 type arrays with `nullable`), numeric comparisons emit the
    /// JSON-Schema-2020-12 numeric keywords, and `multipleOf` carries over.
    #[test]
    fn openapi_emits_wave4_type_and_numeric_hints() {
        let manifest = ModuleManifest::from_toml(
            r#"
[module]
name = "ledger"
version = "0.1.0"
maturity = "L0"
plane = "tenant"
description = "Ledger"

[capabilities]
create = { profile = "COMMAND" }

[[routes]]
method = "POST"
path = "/api/v1/ledger"
operation = "create"
capability = "create"

[[validation]]
operation = "create"
field = "amount"
rules = ["required", "numeric", "gt:0", "lte:1000", "multiple_of:0.5"]

[[validation]]
operation = "create"
field = "note"
rules = ["nullable", "string", "max:64"]

[[validation]]
operation = "create"
field = "kind"
rules = ["string", "max:16"]
"#,
        )
        .expect("manifest parses");

        let graph = PlatformGraph::compile(vec![manifest]).expect("graph compiles");
        let doc = emit_openapi(&graph, "keel-platform", "0.1.0");
        let schema = &doc["paths"]["/api/v1/ledger"]["post"]["requestBody"]["content"]
            ["application/json"]["schema"];

        let amount = &schema["properties"]["amount"];
        assert_eq!(amount["type"], "number");
        assert_eq!(amount["exclusiveMinimum"], json!(0.0));
        assert_eq!(amount["maximum"], json!(1000.0));
        assert_eq!(amount["multipleOf"], json!(0.5));

        // nullable widens the type to a 3.1 type array; without it the
        // declared (or default) type stays a plain string.
        assert_eq!(json!(["string", "null"]), schema["properties"]["note"]["type"]);
        assert_eq!(json!("string"), schema["properties"]["kind"]["type"]);

        let required = schema["required"].as_array().expect("required list");
        assert!(required.contains(&json!("amount")));
        assert!(!required.contains(&json!("note")));
    }
}
