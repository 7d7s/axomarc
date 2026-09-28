//! Response envelope + error body (04-API-CONTRACTS §1).
use serde::{Deserialize, Serialize};
use kernel_core::KernelError;

#[derive(Debug, Serialize, Deserialize)]
pub struct Meta {
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_of: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    pub request_id: Option<String>,
}

impl<T> Envelope<T> {
    pub fn ok(data: T) -> Self {
        Self { data: Some(data), meta: Some(Meta { request_id: None, etag: None, cursor: None, has_more: None, replay_of: None }), error: None }
    }

    pub fn created(data: T) -> Self {
        Self::ok(data)
    }

    pub fn error_with(err: &KernelError, message: &str) -> Self {
        Self {
            data: None,
            meta: None,
            error: Some(ErrorBody {
                code: err.code().to_string(),
                message: message.to_string(),
                details: Some(serde_json::json!({ "doc_url": err.doc_url() })),
                request_id: None,
            }),
        }
    }
}