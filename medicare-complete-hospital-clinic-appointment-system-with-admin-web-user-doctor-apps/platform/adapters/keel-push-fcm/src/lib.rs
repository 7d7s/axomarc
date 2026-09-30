//! # keel-push-fcm
//! Firebase Cloud Messaging adapter (D-17, blueprint L40). Two modes:
//! - `FcmSandbox`: records nudges, sends nothing (dev/test).
//! - `FcmLive`: real delivery over HTTPS (reqwest blocking) against the FCM
//!   legacy HTTP endpoint with a server key. Nudge-not-payload rule is
//!   enforced HERE, not by convention: the push body carries the template
//!   name as the signal plus (optionally) a delta cursor — never vars, which
//!   may contain PHI.
//!
//! Note: FCM v1 API with OAuth2 service-account tokens is the upgrade path;
//! the legacy endpoint keeps the adapter dependency-free of a JWT stack.

use keel_foundation::AppError;
use keel_notifications::{NotifyMessage, PushPort};
use serde::{Deserialize, Serialize};

const FCM_ENDPOINT: &str = "https://fcm.googleapis.com/fcm/send";

#[derive(Debug, Clone)]
pub struct PushNudge {
    pub device_token: String,
    pub signal: String,
}

#[derive(Default)]
pub struct FcmSandbox {
    pub nudges: std::sync::Mutex<Vec<PushNudge>>,
}

impl FcmSandbox {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PushPort for FcmSandbox {
    fn send(&self, msg: &NotifyMessage) -> Result<(), AppError> {
        validate(msg)?;
        // Nudge-not-payload: template name is the signal; no vars in the push.
        self.nudges.lock().unwrap().push(PushNudge {
            device_token: msg.to.clone(),
            signal: msg.template.clone(),
        });
        Ok(())
    }
}

fn validate(msg: &NotifyMessage) -> Result<(), AppError> {
    if msg.to.is_empty() {
        return Err(AppError::validation("push: missing device token"));
    }
    if msg.template.is_empty() {
        return Err(AppError::validation("push: missing signal"));
    }
    Ok(())
}

// ------------------------------------------------------------------- live

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcmConfig {
    /// Legacy FCM server key (`key=...` authorization).
    pub server_key: String,
}

/// Pure request construction (no network) — what the live transport sends.
#[derive(Debug)]
pub struct FcmRequest {
    pub url: String,
    pub authorization: String,
    pub body: serde_json::Value,
}

pub fn build_request(config: &FcmConfig, msg: &NotifyMessage) -> Result<FcmRequest, AppError> {
    validate(msg)?;
    if config.server_key.is_empty() {
        return Err(AppError::validation("push: FCM server key not configured"));
    }
    // Nudge-not-payload: signal + optional delta cursor from vars ONLY when
    // the var is explicitly the cursor key; everything else stays out.
    let mut data = serde_json::json!({ "signal": msg.template });
    if let Some(cursor) = msg.vars.get("delta_cursor") {
        data["delta_cursor"] = serde_json::Value::String(cursor.clone());
    }
    let body = serde_json::json!({
        "to": msg.to,
        "data": data,
        "priority": "high",
    });
    Ok(FcmRequest {
        url: FCM_ENDPOINT.to_string(),
        authorization: format!("key={}", config.server_key),
        body,
    })
}

pub struct FcmLive {
    pub config: FcmConfig,
    client: reqwest::blocking::Client,
}

#[derive(Debug, Deserialize)]
struct FcmResponse {
    #[serde(default, rename = "messageId")]
    message_id: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<serde_json::Value>,
    #[serde(default, rename = "multicast_id")]
    multicast_id: Option<serde_json::Value>,
}

impl FcmLive {
    pub fn new(config: FcmConfig) -> Result<Self, AppError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| AppError::internal(format!("fcm client build: {e}")))?;
        Ok(Self { config, client })
    }
}

impl PushPort for FcmLive {
    fn send(&self, msg: &NotifyMessage) -> Result<(), AppError> {
        let request = build_request(&self.config, msg)?;
        let response = self
            .client
            .post(&request.url)
            .header("authorization", &request.authorization)
            .header("content-type", "application/json")
            .json(&request.body)
            .send()
            .map_err(|e| AppError::external("fcm", e.to_string()))?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(AppError::external(
                "fcm",
                format!("http {status}: {}", truncate(&text)),
            ));
        }
        match serde_json::from_str::<FcmResponse>(&text) {
            Ok(parsed) => {
                if parsed.error.is_some()
                    && parsed.message_id.is_none()
                    && parsed.multicast_id.is_none()
                {
                    Err(AppError::external("fcm", truncate(&text)))
                } else {
                    Ok(())
                }
            }
            Err(_) => Ok(()), // legacy endpoint sometimes returns empty 200
        }
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn msg(token: &str, signal: &str, vars: BTreeMap<String, String>) -> NotifyMessage {
        NotifyMessage {
            channel: keel_notifications::Channel::Push,
            to: token.into(),
            template: signal.into(),
            vars,
            tenant_id: None,
            locale: None,
        }
    }

    #[test]
    fn nudge_not_payload() {
        let s = FcmSandbox::new();
        let mut vars = BTreeMap::new();
        vars.insert("phi".to_string(), "secret".to_string());
        s.send(&msg("device-token-1", "appointment.reminder", vars))
            .unwrap();
        let nudges = s.nudges.lock().unwrap();
        assert_eq!(nudges.len(), 1);
        assert_eq!(nudges[0].signal, "appointment.reminder");
        assert!(!nudges[0].device_token.is_empty());
    }

    #[test]
    fn build_request_never_includes_phi_vars() {
        let config = FcmConfig {
            server_key: "srv".into(),
        };
        let mut vars = BTreeMap::new();
        vars.insert("phi".to_string(), "secret".to_string());
        vars.insert("delta_cursor".to_string(), "42".into());
        let req = build_request(&config, &msg("tok", "appointment.reminder", vars)).unwrap();
        let body = req.body.to_string();
        assert!(
            !body.contains("secret"),
            "PHI var must not ride the push: {body}"
        );
        assert!(body.contains("appointment.reminder"));
        assert!(body.contains("42"), "delta cursor is allowed");
        assert_eq!(req.authorization, "key=srv");
        assert!(!body.contains("srv"));
    }

    #[test]
    fn missing_token_or_key_rejected() {
        let config = FcmConfig {
            server_key: "srv".into(),
        };
        assert!(build_request(&config, &msg("", "sig", BTreeMap::new())).is_err());
        let no_key = FcmConfig {
            server_key: String::new(),
        };
        let err = build_request(&no_key, &msg("tok", "sig", BTreeMap::new())).unwrap_err();
        assert_eq!(err.code, keel_foundation::codes::VALIDATION);
    }
}
