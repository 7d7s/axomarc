//! # keel-notifications
//! Notification ports (MSG91 SMS / SMTP mail / FCM push per D-16..D-18).
//! Adapters implement; hub dispatches; failures follow the external dependency
//! behavior matrix (timeout, 429, duplicate risk...).

use keel_foundation::{AppError, TenantId};
use keel_storage::{ObjectRef, StoragePort};
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

/// Wave 6 P1 attachment: the bytes live in storage, the Mailable (and the
/// `mail.send` job payload) carries only the object reference. The send path
/// streams the bytes through [`StoragePort::get_capped`] under the upload cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailAttachment {
    pub filename: String,
    /// MIME type of the attachment (`application/pdf`, `image/png`, ...).
    pub content_type: String,
    pub object: ObjectRef,
}

/// One attachment after size-capped resolution against a [`StoragePort`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMailAttachment {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Default attachment cap for mail: mirrors
/// `UploadLimits::default().max_file_bytes` (8 MiB). A keel-api test pins the
/// two together so an attachment the upload edge accepts is always sendable.
pub const DEFAULT_MAX_ATTACHMENT_BYTES: u64 = 8 * 1024 * 1024;

/// One rich outgoing mail (docs/19 Wave 6 `Mailable`): recipients, text/html
/// bodies, extra headers and storage-backed attachments. Serializable — this
/// is the payload of the queued `mail.send` job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mailable {
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    /// Sender override; falls back to the selected mailer's `from`.
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub reply_to: Option<String>,
    pub subject: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub html: Option<String>,
    /// Extra RFC822 headers (`X-Keel-Tenant`, `List-Unsubscribe`, ...).
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub attachments: Vec<MailAttachment>,
}

impl Mailable {
    /// Fail-closed shape check the transports run before touching a network:
    /// at least one `to`, every address looks like one, a body exists, and
    /// attachment descriptors are complete.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.to.is_empty() {
            return Err(AppError::validation(
                "mail: at least one `to` recipient is required",
            ));
        }
        for addr in self.to.iter().chain(&self.cc).chain(&self.bcc) {
            if addr.is_empty() || !addr.contains('@') {
                return Err(AppError::validation(format!(
                    "mail: invalid recipient {addr:?}"
                )));
            }
        }
        if self.text.is_none() && self.html.is_none() {
            return Err(AppError::validation("mail: a text or html body is required"));
        }
        for att in &self.attachments {
            if att.filename.is_empty() || att.content_type.is_empty() {
                return Err(AppError::validation(
                    "mail: attachment filename and content_type are required",
                ));
            }
            if att.object.key.is_empty() {
                return Err(AppError::validation(
                    "mail: attachment object key is empty",
                ));
            }
        }
        Ok(())
    }

    /// Stream every attachment through the storage port under `max_bytes`
    /// (the upload-limit cap). Over the cap is `CAP_LIMIT_EXCEEDED` — the
    /// send never proceeds with a truncated or oversized body.
    pub fn resolve_attachments(
        &self,
        storage: &dyn StoragePort,
        max_bytes: u64,
    ) -> Result<Vec<ResolvedMailAttachment>, AppError> {
        self.attachments
            .iter()
            .map(|att| {
                let bytes = storage.get_capped(&att.object, max_bytes)?;
                Ok(ResolvedMailAttachment {
                    filename: att.filename.clone(),
                    content_type: att.content_type.clone(),
                    bytes,
                })
            })
            .collect()
    }
}

pub trait SmsPort: Send + Sync {
    fn send(&self, msg: &NotifyMessage) -> Result<(), AppError>;
}

pub trait MailPort: Send + Sync {
    fn send(&self, msg: &NotifyMessage, subject: &str) -> Result<(), AppError>;
    /// Wave 6 P1: rich mail path — cc/bcc, text+html bodies, custom headers
    /// and already-resolved (size-capped) attachments. Implementations run
    /// `Mailable::validate` before touching the transport.
    fn send_mailable(
        &self,
        mail: &Mailable,
        attachments: &[ResolvedMailAttachment],
    ) -> Result<(), AppError>;
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

    /// Minimal storage stand-in whose `get` returns a fixed blob — the default
    /// `get_capped` (byte check after the read) is what runs here.
    struct BlobStorage(Vec<u8>);

    impl StoragePort for BlobStorage {
        fn put(&self, _bucket: &str, _key: &str, _bytes: Vec<u8>) -> Result<ObjectRef, AppError> {
            Err(AppError::not_implemented("test put"))
        }
        fn get(&self, _obj: &ObjectRef) -> Result<Vec<u8>, AppError> {
            Ok(self.0.clone())
        }
        fn delete(&self, _obj: &ObjectRef) -> Result<(), AppError> {
            Ok(())
        }
        fn presigned_url(&self, _obj: &ObjectRef, _ttl_secs: u64) -> Result<String, AppError> {
            Err(AppError::not_implemented("test presign"))
        }
    }

    fn attachment(key: &str) -> MailAttachment {
        MailAttachment {
            filename: "invoice.pdf".into(),
            content_type: "application/pdf".into(),
            object: ObjectRef {
                bucket: "m".into(),
                key: key.into(),
            },
        }
    }

    fn mail() -> Mailable {
        Mailable {
            to: vec!["ops@example.com".into()],
            cc: vec![],
            bcc: vec![],
            from: None,
            reply_to: None,
            subject: "Invoice".into(),
            text: Some("Your invoice is ready.".into()),
            html: None,
            headers: vec![],
            attachments: vec![],
        }
    }

    /// Attachment resolution carries metadata + capped bytes, and the cap is
    /// enforced before the mail can be handed to a transport.
    #[test]
    fn resolve_attachments_caps_bytes_and_carries_metadata() {
        let storage = BlobStorage(vec![1u8; 512]);
        let mut m = mail();
        m.attachments.push(attachment("invoice.pdf"));

        let resolved = m.resolve_attachments(&storage, 512).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].filename, "invoice.pdf");
        assert_eq!(resolved[0].content_type, "application/pdf");
        assert_eq!(resolved[0].bytes.len(), 512);

        let err = m.resolve_attachments(&storage, 511).unwrap_err();
        assert_eq!(err.code, keel_foundation::codes::CAP_LIMIT);
    }

    #[test]
    fn mailable_validate_rejects_malformed_shape() {
        // no recipients
        let mut m = mail();
        m.to.clear();
        assert!(m.validate().is_err());
        // malformed cc recipient
        let mut m = mail();
        m.cc.push("not-an-address".into());
        let err = m.validate().unwrap_err();
        assert_eq!(err.code, keel_foundation::codes::VALIDATION);
        // missing body
        let mut m = mail();
        m.text = None;
        assert!(m.validate().is_err());
        // incomplete attachment descriptor
        let mut m = mail();
        m.attachments.push(MailAttachment {
            filename: String::new(),
            content_type: "application/pdf".into(),
            object: ObjectRef {
                bucket: "m".into(),
                key: "k".into(),
            },
        });
        assert!(m.validate().is_err());
        // valid passes
        assert!(mail().validate().is_ok());
    }

    /// The job payload keeps the object reference (never inline bytes) and
    /// round-trips through serde — this is the `mail.send` contract.
    #[test]
    fn mailable_job_payload_roundtrips_with_object_refs_only() {
        let mut m = mail();
        m.cc.push("audit@example.com".into());
        m.headers.push(("X-Keel-Tenant".into(), "t-1".into()));
        m.attachments.push(attachment("invoice.pdf"));

        let payload = serde_json::to_value(&m).expect("serializes");
        let raw = payload.to_string();
        assert!(raw.contains("invoice.pdf"));
        assert!(!raw.contains("\"bytes\""), "payload carries refs, not bytes");

        let back: Mailable = serde_json::from_value(payload).expect("deserializes");
        assert_eq!(back, m);
    }
}
