//! # keel-ai-mcp — MCP tool surface (item 34, L49)
//!
//! The MCP manifest is *generated* from the same `Command`/`Query` contracts
//! that feed the in-process agent runtime. There is no parallel authorization
//! story: an MCP tool call enters through the exact same
//! `ToolSpec -> operation -> Engine` path, so capability, scope and policy
//! apply unchanged. The manifest only describes what exists — it grants
//! nothing.

use keel_ai_tools::ToolSpec;
use keel_foundation::OperationId;
use keel_registry::PlatformGraph;

/// Permissive argument schema for tools whose operation input shape is not
/// declared at manifest level. Validation stays on the operation side.
fn permissive_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object", "additionalProperties": true })
}

/// Generate an MCP-style tool manifest from a set of tool specs.
///
/// Shape: `{ name, version, tools: [{ name, description, inputSchema }] }`.
/// `inputSchema` is the JSON schema the client must satisfy; the operation
/// re-validates on execution.
pub fn generate_manifest(name: &str, version: &str, tools: &[ToolSpec]) -> serde_json::Value {
    let tools: Vec<serde_json::Value> = tools
        .iter()
        .map(|spec| {
            serde_json::json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": spec.input_schema,
            })
        })
        .collect();
    serde_json::json!({ "name": name, "version": version, "tools": tools })
}

/// Derive one tool spec per registered operation from the compiled platform
/// graph. Tool names are the flattened operation id (`module__operation`).
///
/// The capability mirrors the qualified operation id (framework convention:
/// `module.toml` capabilities are keyed by operation). It is metadata for
/// MCP clients — the actual check still happens in the engine at call time.
pub fn tools_from_graph(graph: &PlatformGraph) -> Vec<ToolSpec> {
    graph
        .operations
        .iter()
        .map(|(operation, module)| {
            let operation_id = OperationId::parse(operation)
                .expect("graph operations are valid module.operation ids");
            ToolSpec::new(
                operation.replace('.', "__"),
                format!("{operation} (owned by {module})"),
                operation_id,
            )
            .with_capability(operation.clone())
            .with_schema(permissive_schema())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn graph() -> PlatformGraph {
        PlatformGraph {
            modules: Vec::new(),
            operations: [
                ("billing.charge".to_string(), "billing".to_string()),
                ("notes.create".to_string(), "notes".to_string()),
            ]
            .into_iter()
            .collect(),
            events: BTreeMap::new(),
        }
    }

    #[test]
    fn tools_follow_the_platform_graph() {
        let tools = tools_from_graph(&graph());
        assert_eq!(tools.len(), 2);
        let note = tools
            .iter()
            .find(|t| t.name == "notes__create")
            .expect("notes tool");
        assert_eq!(note.operation.as_str(), "notes.create");
        assert_eq!(note.capability.as_deref(), Some("notes.create"));
        assert!(note.description.contains("owned by notes"));
    }

    #[test]
    fn manifest_has_the_mcp_shape() {
        let tools = tools_from_graph(&graph());
        let manifest = generate_manifest("keel-platform", "0.1.0", &tools);
        assert_eq!(manifest["name"], "keel-platform");
        assert_eq!(manifest["version"], "0.1.0");
        let list = manifest["tools"].as_array().expect("tools array");
        assert_eq!(list.len(), 2);
        let first = &list[0];
        assert!(first["name"].is_string());
        assert!(first["description"].is_string());
        assert_eq!(first["inputSchema"]["type"], "object");
    }

    #[test]
    fn manifest_round_trips_tool_names() {
        let tools = tools_from_graph(&graph());
        let manifest = generate_manifest("keel-platform", "0.1.0", &tools);
        let names: Vec<&str> = manifest["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["billing__charge", "notes__create"]);
    }
}
