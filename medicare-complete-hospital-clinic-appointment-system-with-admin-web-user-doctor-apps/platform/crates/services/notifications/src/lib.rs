//! # keel-notifications
//! Notification ports (MSG91 SMS / SMTP mail / FCM push per D-16..D-18).
//! Adapters implement; hub dispatches; failures follow the external dependency
//! behavior matrix (timeout, 429, duplicate risk...).

use keel_foundation::{AppError, TenantId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Sms,
    Email,
    Push,
}

/// `to` is a delivery address (phone/email/device token). Sensitive: adapters log
/// redacted forms only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyMessage {
    pub channel: Channel,
    pub to: String,
    pub template: String,
    pub vars: BTreeMap<String, String>,
    pub tenant_id: Option<TenantId>,
}

pub trait SmsPort: Send + Sync {
    fn send(&self, msg: &NotifyMessage) -> Result<(), AppError>;
}

pub trait MailPort: Send + Sync {
    fn send(&self, msg: &NotifyMessage, subject: &str) -> Result<(), AppError>;
}

pub trait PushPort: Send + Sync {
    fn send(&self, msg: &NotifyMessage) -> Result<(), AppError>;
}

/// Dispatch hub: route by channel. Missing adapter = explicit error, never silent.
pub struct NotifyHub {
    pub sms: Option<Box<dyn SmsPort>>,
    pub mail: Option<Box<dyn MailPort>>,
    pub push: Option<Box<dyn PushPort>>,
}

impl NotifyHub {
    pub fn empty() -> Self {
        Self {
            sms: None,
            mail: None,
            push: None,
        }
    }

    pub fn dispatch(&self, msg: &NotifyMessage) -> Result<(), AppError> {
        match msg.channel {
            Channel::Sms => self
                .sms
                .as_ref()
                .ok_or_else(|| AppError::not_implemented("sms adapter"))?
                .send(msg),
            Channel::Email => self
                .mail
                .as_ref()
                .ok_or_else(|| AppError::not_implemented("mail adapter"))?
                .send(msg, msg.template.as_str()),
            Channel::Push => self
                .push
                .as_ref()
                .ok_or_else(|| AppError::not_implemented("push adapter"))?
                .send(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_adapter_is_explicit() {
        let hub = NotifyHub::empty();
        let msg = NotifyMessage {
            channel: Channel::Sms,
            to: "+919876543210".into(),
            template: "otp".into(),
            vars: BTreeMap::new(),
            tenant_id: None,
        };
        let err = hub.dispatch(&msg).unwrap_err();
        assert_eq!(err.code, keel_foundation::codes::NOT_IMPLEMENTED);
    }
}
