//! # kelson-ai-tools — tool contract (items 33/34)
//!
//! A tool is a named binding onto one platform operation: the model sees a
//! schema, the runtime executes the operation through the normal pipeline —
//! capability, scope and policy apply unchanged. Tools are generated from
//! `Command`/`Query` contracts, never hand-wired around them.

use kelson_ai::ModelTool;
use kelson_contracts::{Command, Query};
use kelson_foundation::OperationId;
use serde::{Deserialize, Serialize};

/// One tool exposed to a model or MCP client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Tool name visible to the model (`snake_case`, unique per registry).
    pub name: String,
    pub description: String,
    /// The operation executed when the tool is called.
    pub operation: OperationId,
    /// Required capability, mirrored from the operation's plan metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// JSON schema of the tool arguments (becomes the operation input).
    #[serde(default = "default_schema")]
    pub input_schema: serde_json::Value,
}

fn default_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object", "additionalProperties": true })
}

impl ToolSpec {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        operation: OperationId,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            operation,
            capability: None,
            input_schema: default_schema(),
        }
    }

    pub fn with_capability(mut self, capability: impl Into<String>) -> Self {
        self.capability = Some(capability.into());
        self
    }

    pub fn with_schema(mut self, schema: serde_json::Value) -> Self {
        self.input_schema = schema;
        self
    }

    /// Generate the tool descriptor from a `Command` contract — the canonical
    /// way tools enter the registry (item 34).
    pub fn for_command<C: Command>(
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: operation_to_tool_name(C::OPERATION),
            description: description.into(),
            operation: OperationId::parse(C::OPERATION)
                .expect("Command::OPERATION is a valid module.operation id"),
            capability: C::CAPABILITY.map(|c| c.to_string()),
            input_schema,
        }
    }

    /// Generate the tool descriptor from a `Query` contract.
    pub fn for_query<Q: Query>(description: impl Into<String>) -> Self {
        Self {
            name: operation_to_tool_name(Q::OPERATION),
            description: description.into(),
            operation: OperationId::parse(Q::OPERATION)
                .expect("Query::OPERATION is a valid module.operation id"),
            capability: None,
            input_schema: default_schema(),
        }
    }

    /// Model-facing projection of this tool.
    pub fn model_tool(&self) -> ModelTool {
        ModelTool {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
        }
    }
}

/// `module.operation` -> `module__operation` (MCP/tool names are flat).
fn operation_to_tool_name(operation: &str) -> String {
    operation.replace('.', "__")
}

/// Runtime lookup surface for the agent loop and MCP generation.
pub trait ToolRegistry: Send + Sync {
    fn specs(&self) -> Vec<ToolSpec>;
    fn get(&self, name: &str) -> Option<ToolSpec>;
}

/// Static registry assembled at wiring time.
#[derive(Debug, Default, Clone)]
pub struct StaticToolRegistry {
    specs: Vec<ToolSpec>,
}

impl StaticToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_tools(specs: Vec<ToolSpec>) -> Self {
        Self { specs }
    }

    pub fn push(&mut self, spec: ToolSpec) {
        self.specs.push(spec);
    }
}

impl ToolRegistry for StaticToolRegistry {
    fn specs(&self) -> Vec<ToolSpec> {
        self.specs.clone()
    }

    fn get(&self, name: &str) -> Option<ToolSpec> {
        self.specs.iter().find(|spec| spec.name == name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_foundation::ExecutionProfileKind;

    struct CreateNote;
    impl Command for CreateNote {
        type Output = serde_json::Value;
        const OPERATION: &'static str = "notes.create";
        const PROFILE: ExecutionProfileKind = ExecutionProfileKind::Command;
        const CAPABILITY: Option<&'static str> = Some("notes.create");
    }

    struct ListNotes;
    impl Query for ListNotes {
        type Output = serde_json::Value;
        const OPERATION: &'static str = "notes.list";
    }

    #[test]
    fn tool_generated_from_command_contract() {
        let spec = ToolSpec::for_command::<CreateNote>(
            "Create a note",
            serde_json::json!({ "type": "object" }),
        );
        assert_eq!(spec.name, "notes__create");
        assert_eq!(spec.operation.as_str(), "notes.create");
        assert_eq!(spec.capability.as_deref(), Some("notes.create"));
        let model = spec.model_tool();
        assert_eq!(model.name, "notes__create");
    }

    #[test]
    fn tool_generated_from_query_contract() {
        let spec = ToolSpec::for_query::<ListNotes>("List notes");
        assert_eq!(spec.name, "notes__list");
        assert!(spec.capability.is_none());
    }

    #[test]
    fn registry_lookups_by_name() {
        let registry = StaticToolRegistry::with_tools(vec![
            ToolSpec::for_command::<CreateNote>("Create a note", default_schema()),
            ToolSpec::for_query::<ListNotes>("List notes"),
        ]);
        assert!(registry.get("notes__create").is_some());
        assert!(registry.get("missing").is_none());
        assert_eq!(registry.specs().len(), 2);
    }
}
