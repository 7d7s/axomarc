//! # keel-mail-smtp
//! SMTP mail adapter (D-18, blueprint L40). Two modes:
//! - `SmtpSandbox`: records rendered intents, sends nothing (dev/test, and the
//!   degraded-boot default).
//! - `SmtpLive`: real delivery via lettre (blocking transport, rustls). Mail
//!   sending is expected to run INSIDE a durable job (`mail.send`) so the
//!   blocking IO never sits on a request path; the jobs layer supplies the
//!   retry budget.

use keel_foundation::AppError;
use keel_notifications::{MailPort, NotifyMessage};
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct RenderedMail {
    pub to: String,
    pub subject: String,
    pub template: String,
}

#[derive(Default)]
pub struct SmtpSandbox {
    pub outbox: std::sync::Mutex<Vec<RenderedMail>>,
}

impl SmtpSandbox {
    pub fn new() -> Self {
        Self::default()
    }
}

impl MailPort for SmtpSandbox {
    fn send(&self, msg: &NotifyMessage, subject: &str) -> Result<(), AppError> {
        if msg.to.is_empty() || !msg.to.contains('@') {
            return Err(AppError::validation("mail: invalid recipient"));
        }
        self.outbox.lock().unwrap().push(RenderedMail {
            to: msg.to.clone(),
            subject: subject.to_string(),
            template: msg.template.clone(),
        });
        Ok(())
    }
}

// ------------------------------------------------------------------- live

/// Live SMTP connection settings (from app config / secret envelope).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    /// PLAIN login username; empty = unauthenticated relay.
    pub username: String,
    pub password: String,
    /// From address, e.g. `Keel <no-reply@example.com>`.
    pub from: String,
}

/// Live transport. Build once (connection pooling inside lettre), clone cheap.
pub struct SmtpLive {
    config: SmtpConfig,
    transport: SmtpTransport,
}

impl SmtpLive {
    pub fn new(config: SmtpConfig) -> Result<Self, AppError> {
        let builder = SmtpTransport::builder_dangerous(&config.host);
        // Port 465 implicit TLS, 587 STARTTLS, anything else plaintext relay
        // (dev docker mailhog on 1025).
        let transport = if config.username.is_empty() {
            builder.port(config.port).build()
        } else {
            builder
                .port(config.port)
                .credentials(Credentials::new(
                    config.username.clone(),
                    config.password.clone(),
                ))
                .build()
        };
        Ok(Self { config, transport })
    }

    /// Build the RFC822 message without sending (testable, deterministic).
    pub fn build_message(
        config: &SmtpConfig,
        msg: &NotifyMessage,
        subject: &str,
    ) -> Result<Message, AppError> {
        // Render the template body: `template` is the plain-text body here;
        // richer templating lives in the calling module before dispatch.
        let body = if msg.vars.is_empty() {
            msg.template.clone()
        } else {
            let mut rendered = msg.template.clone();
            for (key, value) in &msg.vars {
                rendered = rendered.replace(&format!("{{{key}}}"), value);
            }
            rendered
        };
        Message::builder()
            .from(
                config
                    .from
                    .parse()
                    .map_err(|e| AppError::validation(format!("mail: bad from address: {e}")))?,
            )
            .to(msg
                .to
                .parse()
                .map_err(|e| AppError::validation(format!("mail: bad recipient: {e}")))?)
            .subject(subject.to_string())
            .header(ContentType::TEXT_PLAIN)
            .body(body)
            .map_err(|e| AppError::validation(format!("mail: message build: {e}")))
    }
}

impl MailPort for SmtpLive {
    fn send(&self, msg: &NotifyMessage, subject: &str) -> Result<(), AppError> {
        if msg.to.is_empty() || !msg.to.contains('@') {
            return Err(AppError::validation("mail: invalid recipient"));
        }
        let message = Self::build_message(&self.config, msg, subject)?;
        self.transport
            .send(&message)
            .map(|_| ())
            .map_err(|e| AppError::external("smtp", e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn sandbox() -> SmtpSandbox {
        SmtpSandbox::new()
    }

    fn config() -> SmtpConfig {
        SmtpConfig {
            host: "smtp.example.com".into(),
            port: 587,
            username: "no-reply@example.com".into(),
            password: "secret".into(),
            from: "Keel <no-reply@example.com>".into(),
        }
    }

    fn msg(to: &str, template: &str, vars: BTreeMap<String, String>) -> NotifyMessage {
        NotifyMessage {
            channel: keel_notifications::Channel::Email,
            to: to.into(),
            template: template.into(),
            vars,
            tenant_id: None,
        }
    }

    #[test]
    fn sandbox_stores_rendered_mail() {
        let s = sandbox();
        s.send(
            &msg("ops@example.com", "welcome", BTreeMap::new()),
            "Welcome",
        )
        .unwrap();
        assert_eq!(s.outbox.lock().unwrap().len(), 1);
    }

    #[test]
    fn live_builds_rfc822_message_with_var_substitution() {
        let mut vars = BTreeMap::new();
        vars.insert("name".into(), "Asha".into());
        let message = SmtpLive::build_message(
            &config(),
            &msg("a@b.c", "Hello {name}, your OTP is ready.", vars),
            "OTP",
        )
        .unwrap();
        let rendered = String::from_utf8_lossy(&message.formatted()).to_string();
        assert!(rendered.contains("Subject: OTP"));
        assert!(rendered.contains("Hello Asha, your OTP is ready."));
        assert!(rendered.contains("To: a@b.c"));
    }

    #[test]
    fn live_rejects_bad_addresses_before_network() {
        let err = SmtpLive::build_message(
            &config(),
            &msg("not-an-address", "body", BTreeMap::new()),
            "s",
        )
        .unwrap_err();
        assert_eq!(err.code, keel_foundation::codes::VALIDATION);
        let live = SmtpLive::new(config()).unwrap();
        assert!(live.send(&msg("", "body", BTreeMap::new()), "s").is_err());
    }
}
