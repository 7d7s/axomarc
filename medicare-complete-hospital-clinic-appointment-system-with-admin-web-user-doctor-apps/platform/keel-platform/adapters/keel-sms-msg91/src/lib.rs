//! # keel-sms-msg91
//! MSG91 SMS adapter (D-16, blueprint L40). Two modes:
//! - `Msg91Sandbox`: validates + records intent, sends nothing (dev/test).
//! - `Msg91Live`: real delivery over HTTPS (reqwest blocking). SMS sends run
//!   inside durable jobs (`sms.send`) so blocking IO stays off request paths
//!   and the jobs layer owns the retry budget.
//!
//! Security: the auth key travels in the `authkey` HEADER (never in the URL
//! query string), and error messages never echo it back.

use keel_foundation::AppError;
use keel_notifications::{NotifyMessage, SmsPort};
use serde::{Deserialize, Serialize};

const MSG91_ENDPOINT: &str = "https://api.msg91.com/api/v2/sendsms";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Msg91Config {
    /// MSG91 auth key (secret — comes from the secret envelope at boot).
    pub auth_key: String,
    pub sender_id: String,
    /// DLT registration ids (required for Indian transactional traffic).
    pub dlt_entity_id: Option<String>,
    pub dlt_template_id: Option<String>,
    #[serde(default)]
    pub country_code: u16,
}

impl Msg91Config {
    /// Sandbox-era shape: presence flag only, no key material.
    pub fn auth_key_present(&self) -> bool {
        !self.auth_key.is_empty()
    }
}

/// Sandbox adapter: validates the message contract, records intent, sends nothing.
pub struct Msg91Sandbox {
    pub config: Msg91Config,
    pub sent: std::sync::Mutex<Vec<NotifyMessage>>,
}

impl Msg91Sandbox {
    pub fn new(config: Msg91Config) -> Self {
        Self {
            config,
            sent: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl SmsPort for Msg91Sandbox {
    fn send(&self, msg: &NotifyMessage) -> Result<(), AppError> {
        validate(msg)?;
        self.sent.lock().unwrap().push(msg.clone());
        Ok(())
    }
}

fn validate(msg: &NotifyMessage) -> Result<(), AppError> {
    if msg.to.is_empty() {
        return Err(AppError::validation("sms: missing recipient"));
    }
    if msg.template.is_empty() {
        return Err(AppError::validation("sms: missing template"));
    }
    Ok(())
}

/// Render `{var}` placeholders; unreplaced placeholders are a validation
/// error (sending a literal `{otp}` to a patient is never acceptable).
pub fn render_body(
    template: &str,
    vars: &std::collections::BTreeMap<String, String>,
) -> Result<String, AppError> {
    let mut rendered = template.to_string();
    for (key, value) in vars {
        rendered = rendered.replace(&format!("{{{key}}}"), value);
    }
    if let Some(start) = rendered.find('{') {
        if rendered[start + 1..].contains('}') {
            return Err(AppError::validation(format!(
                "sms: template {:?} has unreplaced placeholders",
                template
            )));
        }
    }
    Ok(rendered)
}

/// Pure request construction (no network) — what the live transport sends.
#[derive(Debug)]
pub struct Msg91Request {
    pub url: String,
    pub auth_header: String,
    pub body: serde_json::Value,
}

pub fn build_request(config: &Msg91Config, msg: &NotifyMessage) -> Result<Msg91Request, AppError> {
    validate(msg)?;
    if config.auth_key.is_empty() {
        return Err(AppError::validation("sms: MSG91 auth key not configured"));
    }
    let body_text = render_body(&msg.template, &msg.vars)?;
    let to: String = msg.to.chars().filter(|c| c.is_ascii_digit()).collect();
    if to.len() < 10 {
        return Err(AppError::validation("sms: recipient has too few digits"));
    }
    let sms = serde_json::json!({ "message": body_text, "to": [to] });
    let mut root = serde_json::json!({
        "country": if config.country_code == 0 { 91 } else { config.country_code },
        "sender": config.sender_id,
        "route": "4",
        "sms": [sms],
    });
    if let Some(dlt) = &config.dlt_template_id {
        root["DLTTemplateId"] = serde_json::Value::String(dlt.clone());
    }
    if let Some(entity) = &config.dlt_entity_id {
        root["DLTEntityId"] = serde_json::Value::String(entity.clone());
    }
    Ok(Msg91Request {
        url: MSG91_ENDPOINT.to_string(),
        auth_header: config.auth_key.clone(),
        body: root,
    })
}

/// Live transport (reqwest blocking).
pub struct Msg91Live {
    pub config: Msg91Config,
    client: reqwest::blocking::Client,
}

impl Msg91Live {
    pub fn new(config: Msg91Config) -> Result<Self, AppError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| AppError::internal(format!("msg91 client build: {e}")))?;
        Ok(Self { config, client })
    }
}

#[derive(Debug, Deserialize)]
struct Msg91Response {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    message: Option<String>,
}

impl SmsPort for Msg91Live {
    fn send(&self, msg: &NotifyMessage) -> Result<(), AppError> {
        let request = build_request(&self.config, msg)?;
        let response = self
            .client
            .post(&request.url)
            .header("authkey", &request.auth_header)
            .header("content-type", "application/json")
            .json(&request.body)
            .send()
            .map_err(|e| AppError::external("msg91", e.to_string()))?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(AppError::external(
                "msg91",
                format!("http {status}: {}", truncate(&text)),
            ));
        }
        match serde_json::from_str::<Msg91Response>(&text) {
            Ok(parsed) if parsed.kind.eq_ignore_ascii_case("error") => Err(AppError::external(
                "msg91",
                parsed.message.unwrap_or_else(|| truncate(&text)),
            )),
            _ => Ok(()),
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

    fn sandbox_config() -> Msg91Config {
        Msg91Config {
            auth_key: String::new(),
            sender_id: "MEDICR".into(),
            dlt_entity_id: None,
            dlt_template_id: None,
            country_code: 91,
        }
    }

    fn live_config() -> Msg91Config {
        Msg91Config {
            auth_key: "k3y".into(),
            dlt_template_id: Some("T1".into()),
            ..sandbox_config()
        }
    }

    fn msg(to: &str, template: &str, vars: BTreeMap<String, String>) -> NotifyMessage {
        NotifyMessage {
            channel: keel_notifications::Channel::Sms,
            to: to.into(),
            template: template.into(),
            vars,
            tenant_id: None,
            locale: None,
        }
    }

    #[test]
    fn sandbox_records_sends() {
        let s = Msg91Sandbox::new(sandbox_config());
        s.send(&msg("+919876543210", "otp", BTreeMap::new()))
            .unwrap();
        assert_eq!(s.sent.lock().unwrap().len(), 1);
    }

    #[test]
    fn rejects_missing_recipient() {
        let s = Msg91Sandbox::new(sandbox_config());
        assert!(s.send(&msg("", "otp", BTreeMap::new())).is_err());
    }

    #[test]
    fn build_request_shapes_the_msg91_payload() {
        let mut vars = BTreeMap::new();
        vars.insert("otp".into(), "4477".into());
        let req = build_request(
            &live_config(),
            &msg("+91 98765 43210", "Your OTP is {otp}", vars),
        )
        .unwrap();
        assert_eq!(req.url, MSG91_ENDPOINT);
        assert_eq!(req.auth_header, "k3y");
        assert!(
            !req.body.to_string().contains("k3y"),
            "auth key never in the body/url"
        );
        assert_eq!(req.body["sender"], "MEDICR");
        assert_eq!(req.body["sms"][0]["to"][0], "919876543210");
        assert_eq!(req.body["sms"][0]["message"], "Your OTP is 4477");
        assert_eq!(req.body["DLTTemplateId"], "T1");
    }

    #[test]
    fn unreplaced_placeholder_is_rejected() {
        let req = build_request(
            &live_config(),
            &msg("+919876543210", "Your OTP is {otp}", BTreeMap::new()),
        );
        assert!(req.is_err());
    }

    #[test]
    fn missing_auth_key_is_validation_error() {
        let err = build_request(
            &sandbox_config(),
            &msg("+919876543210", "hi", BTreeMap::new()),
        )
        .unwrap_err();
        assert_eq!(err.code, keel_foundation::codes::VALIDATION);
    }
}
