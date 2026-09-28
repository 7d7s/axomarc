//! # keel-ai — AI client contract (item 33, L47, D-67)
//!
//! AI plugs into the execution system, it is not an isolated module:
//! agent -> tool -> execution -> capability -> scope -> policy -> command.
//! This crate is the provider port (`AiClient`) plus a deterministic stub
//! provider for dev/test. Live providers (OpenAI & friends) implement the
//! same port behind an adapter — the runtime never sees the provider.

use std::collections::VecDeque;
use std::sync::Mutex;

use keel_foundation::AppError;
use serde::{Deserialize, Serialize};

/// Conversation role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A tool invocation requested by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Stable id for correlating the tool result back to the call.
    pub id: String,
    /// Tool name (must exist in the agent's tool registry).
    pub name: String,
    /// Arguments handed to the bound operation as its execution input.
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// One conversation message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiMessage {
    pub role: Role,
    #[serde(default)]
    pub content: String,
    /// Assistant-only: requested tool executions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Tool-role: the tool call this message answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl AiMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    pub fn tool_calls(calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: calls,
            tool_call_id: None,
        }
    }
}

/// Model-facing tool descriptor (JSON-schema shaped description).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelTool {
    pub name: String,
    pub description: String,
    /// JSON schema of the tool arguments.
    pub input_schema: serde_json::Value,
}

/// One completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub messages: Vec<AiMessage>,
    /// Tools the model may call this turn.
    #[serde(default)]
    pub tools: Vec<ModelTool>,
    pub temperature: f32,
}

/// Token accounting (stub reports zeros; live providers fill it).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// One completion response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub message: AiMessage,
    pub usage: Usage,
}

/// The provider port. Providers must be safe to call concurrently.
pub trait AiClient: Send + Sync {
    fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, AppError>;
    /// Deterministic or provider embedding for vector search.
    fn embed(&self, input: &str) -> Result<Vec<f32>, AppError>;
}

/// Scripted, deterministic provider for dev/test (D-67 stub). Plays queued
/// turns in order; falls back to an echo assistant when the script runs out.
#[derive(Debug, Default)]
pub struct StubAiClient {
    turns: Mutex<VecDeque<AiMessage>>,
}

impl StubAiClient {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue the next assistant turn (content and/or tool calls).
    pub fn push_turn(&self, message: AiMessage) {
        self.turns.lock().expect("stub poisoned").push_back(message);
    }
}

impl AiClient for StubAiClient {
    fn complete(&self, _request: &CompletionRequest) -> Result<CompletionResponse, AppError> {
        let next = self
            .turns
            .lock()
            .expect("stub poisoned")
            .pop_front()
            .unwrap_or_else(|| AiMessage::assistant("stub: no scripted turn"));
        Ok(CompletionResponse {
            message: next,
            usage: Usage::default(),
        })
    }

    fn embed(&self, input: &str) -> Result<Vec<f32>, AppError> {
        Ok(deterministic_embedding(input, 64))
    }
}

/// Hash-token embedding: identical text -> identical unit vector, disjoint
/// text -> ~orthogonal. Good enough for tests and the in-memory vector path.
pub fn deterministic_embedding(input: &str, dims: usize) -> Vec<f32> {
    let mut vector = vec![0.0f32; dims.max(1)];
    for token in input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
    {
        // FNV-1a 64 over the token bytes.
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in token.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        let slot = (hash % vector.len() as u64) as usize;
        vector[slot] += 1.0;
    }
    let norm: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector.iter_mut() {
            *value /= norm;
        }
    }
    vector
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_plays_scripted_turns_then_echoes() {
        let stub = StubAiClient::new();
        stub.push_turn(AiMessage::assistant("first"));
        let request = CompletionRequest {
            messages: vec![AiMessage::user("hi")],
            tools: vec![],
            temperature: 0.0,
        };
        assert_eq!(stub.complete(&request).unwrap().message.content, "first");
        assert_eq!(
            stub.complete(&request).unwrap().message.content,
            "stub: no scripted turn"
        );
    }

    #[test]
    fn embedding_is_deterministic_and_normalized() {
        let a = deterministic_embedding("invoice total", 64);
        let b = deterministic_embedding("invoice total", 64);
        assert_eq!(a, b);
        let norm: f32 = a.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
        let c = deterministic_embedding("completely different words", 64);
        assert_ne!(a, c);
    }

    #[test]
    fn message_constructors_carry_roles() {
        assert_eq!(AiMessage::system("s").role, Role::System);
        assert_eq!(AiMessage::user("u").role, Role::User);
        let tool = AiMessage::tool_result("call-1", "done");
        assert_eq!(tool.tool_call_id.as_deref(), Some("call-1"));
    }
}
