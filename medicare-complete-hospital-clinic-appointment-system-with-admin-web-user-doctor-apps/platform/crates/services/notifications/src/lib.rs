//! # keel-notifications
//! Notification ports (MSG91 SMS / SMTP mail / FCM push per D-16..D-18).
//! Adapters implement; hub dispatches; failures follow the external dependency
//! behavior matrix (timeout, 429, duplicate risk...).

use keel_foundation::{AppError, PrincipalId, TenantId};
use keel_storage::{ObjectRef, StoragePort};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Sms,
    Email,
    Push,
    /// Wave 6 P5: database-channel inbox row (19-PARITY "Notifications v2",
    /// the `notifications` table). Additive: queued payloads only ever name
    /// it from P5 on.
    Database,
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
    /// Wave 6 P3: locale for catalog resolution of `template` (`None` =
    /// the catalog default). Serde-default additive: queued payloads from
    /// before the field still load.
    #[serde(default)]
    pub locale: Option<String>,
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

// --------------------------------------------------- Wave 6 P5: notifications v2

/// Wave 6 P5: a typed notification (19-PARITY "Notifications v2"). Modules
/// declare their notification types by implementing this; [`NotifyHub`]
/// renders each channel and dispatches through the channel ports.
pub trait NotificationDef: Send + Sync {
    /// Stable type key: the `notifications.type` row column and the default
    /// mail view name under `views/notifications/` (dots map to directories).
    fn type_key(&self) -> &str;

    /// Mail channel: subject + text body (+ from/reply-to/headers). The hub
    /// fills `to` from the delivery target and renders the HTML body from
    /// [`Self::template`] when `html` is left unset.
    fn to_mail(&self) -> Mailable;

    /// SMS channel: the message text (or a catalog key the hub resolves
    /// through the boot-loaded message catalog).
    fn to_sms(&self) -> String;

    /// Push channel: the nudge payload. D-17 nudge-not-payload — the hub
    /// forwards `signal` (defaulting to the type key) plus an optional
    /// `delta_cursor`; never free-form data, which the push port forbids.
    fn to_push(&self) -> serde_json::Value;

    /// Database channel: the row's `data` jsonb payload. The same value is
    /// the template context for [`Self::template`].
    fn to_database(&self) -> serde_json::Value;

    /// HTML mail body view under `views/notifications/`. Defaults to
    /// `notifications.<type_key>` (convention-over-configuration); `None`
    /// renders a text-only mail. When a view port is wired, a template that
    /// fails to render fails the dispatch (fail-closed); without a port the
    /// text body stands (the `views: Option` degradation posture). An `html`
    /// body already set on the Mailable wins and skips the render.
    fn template(&self) -> Option<String> {
        Some(format!("notifications.{}", self.type_key()))
    }
}

/// Wave 6 P5: dispatch envelope for one typed notification — the row
/// principal/tenant (database channel) and the locale the mail/SMS strings
/// resolve in (`None` = the catalog default locale).
#[derive(Debug, Clone)]
pub struct NotificationContext {
    pub principal_id: PrincipalId,
    pub tenant_id: Option<TenantId>,
    pub locale: Option<String>,
}

/// Wave 6 P5: one `notifications` row (19-PARITY table: `id, principal_id,
/// tenant_id, type, data jsonb, read_at, created_at`). `kind` maps the SQL
/// keyword column `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationRow {
    pub id: Uuid,
    pub principal_id: PrincipalId,
    pub tenant_id: Option<TenantId>,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: serde_json::Value,
    #[serde(with = "time::serde::iso8601::option")]
    pub read_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
}

/// One newest-first page of rows plus the feed counts the list endpoint
/// ships as meta (`total` rows, `unread` still open).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationPage {
    pub rows: Vec<NotificationRow>,
    pub total: u64,
    pub unread: u64,
}

/// Dyn-safe async handle (the `OutboxStore` posture: async trait, boxing at
/// the impl).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Wave 6 P5: database-channel port. Implemented by keel-postgres (the
/// `notifications` table) and by [`InMemoryNotificationStore`] for degraded
/// boot and tests (D-52). Every method is principal-scoped: a caller can
/// only ever read or mark its own rows.
pub trait NotificationStore: Send + Sync {
    fn insert(&self, row: NotificationRow) -> BoxFuture<'_, Result<(), AppError>>;
    /// Newest-first page for one principal (`created_at DESC, id DESC`),
    /// with the feed counts computed over ALL of the principal's rows.
    fn list(
        &self,
        principal: PrincipalId,
        offset: u64,
        limit: u64,
    ) -> BoxFuture<'_, Result<NotificationPage, AppError>>;
    /// Mark ids read; only rows still unread count (idempotent re-marking
    /// reports 0). Returns the rows changed.
    fn mark_read(
        &self,
        principal: PrincipalId,
        ids: Vec<Uuid>,
    ) -> BoxFuture<'_, Result<u64, AppError>>;
}

/// Degraded-boot/test database channel (D-52): per-instance rows with the
/// same ordering, scoping and idempotence rules as the postgres store.
#[derive(Debug, Default)]
pub struct InMemoryNotificationStore {
    rows: Mutex<Vec<NotificationRow>>,
}

impl InMemoryNotificationStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl NotificationStore for InMemoryNotificationStore {
    fn insert(&self, row: NotificationRow) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            self.rows.lock().unwrap().push(row);
            Ok(())
        })
    }

    fn list(
        &self,
        principal: PrincipalId,
        offset: u64,
        limit: u64,
    ) -> BoxFuture<'_, Result<NotificationPage, AppError>> {
        Box::pin(async move {
            let rows = self.rows.lock().unwrap();
            let mut mine: Vec<&NotificationRow> = rows
                .iter()
                .filter(|row| row.principal_id == principal)
                .collect();
            mine.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.cmp(&a.id)));
            let total = mine.len() as u64;
            let unread = mine.iter().filter(|row| row.read_at.is_none()).count() as u64;
            let page = mine
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .cloned()
                .collect();
            Ok(NotificationPage {
                rows: page,
                total,
                unread,
            })
        })
    }

    fn mark_read(
        &self,
        principal: PrincipalId,
        ids: Vec<Uuid>,
    ) -> BoxFuture<'_, Result<u64, AppError>> {
        Box::pin(async move {
            let now = OffsetDateTime::now_utc();
            let mut rows = self.rows.lock().unwrap();
            let mut updated = 0u64;
            for row in rows.iter_mut() {
                if row.principal_id == principal && row.read_at.is_none() && ids.contains(&row.id)
                {
                    row.read_at = Some(now);
                    updated += 1;
                }
            }
            Ok(updated)
        })
    }
}

/// Wave 6 P5: HTML body seam (19-PARITY "HTML mail bodies rendered via
/// Wave 2 views"). The service layer cannot depend on the web layer, so the
/// views engine reaches the hub through this port — keel-api adapts
/// `ViewsEngine` onto it.
pub trait NotificationViewRenderer: Send + Sync {
    fn render(&self, template: &str, context: serde_json::Value) -> Result<String, AppError>;
}

/// Dispatch hub: route by channel. Missing adapter = explicit error, never silent.
pub struct NotifyHub {
    pub sms: Option<Box<dyn SmsPort>>,
    pub mail: Option<Box<dyn MailPort>>,
    pub push: Option<Box<dyn PushPort>>,
    /// Wave 6 P3: the boot-loaded message catalog for localized
    /// notification strings; `None` (`empty()`) keeps templates literal.
    messages: Option<std::sync::Arc<keel_foundation::MessageCatalog>>,
    /// Wave 6 P5: database-channel store (`None` = unwired → the explicit
    /// missing-adapter error on the `Database` channel).
    store: Option<std::sync::Arc<dyn NotificationStore>>,
    /// Wave 6 P5: HTML body renderer for `views/notifications/` (`None` =
    /// text-only mail, the `views: Option` degradation posture).
    views: Option<std::sync::Arc<dyn NotificationViewRenderer>>,
}

impl NotifyHub {
    pub fn empty() -> Self {
        Self {
            sms: None,
            mail: None,
            push: None,
            messages: None,
            store: None,
            views: None,
        }
    }

    /// Wave 6 P3: a hub that resolves notification strings through the
    /// boot-loaded catalog (producers carry the request locale per
    /// message; `None` falls back to the catalog default locale).
    pub fn with_catalog(messages: std::sync::Arc<keel_foundation::MessageCatalog>) -> Self {
        Self {
            sms: None,
            mail: None,
            push: None,
            messages: Some(messages),
            store: None,
            views: None,
        }
    }

    /// Wave 6 P5: attach the database channel (the postgres store at boot,
    /// the in-memory store on degraded hosts).
    pub fn with_store(mut self, store: std::sync::Arc<dyn NotificationStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Wave 6 P5: attach the notification HTML view renderer (keel-api
    /// adapts the Wave 2 views engine onto the port).
    pub fn with_view_renderer(
        mut self,
        renderer: std::sync::Arc<dyn NotificationViewRenderer>,
    ) -> Self {
        self.views = Some(renderer);
        self
    }

    pub fn dispatch(&self, msg: &NotifyMessage) -> Result<(), AppError> {
        let localized = self.localized(msg);
        let msg = localized.as_ref().unwrap_or(msg);
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
            // The typed path writes rows through the async store port — the
            // sync template dispatch never carries the database channel.
            Channel::Database => Err(AppError::not_implemented(
                "database rows require NotifyHub::dispatch_notification",
            )),
        }
    }

    /// Wave 6 P3: resolve `template` as a catalog key when the hub carries
    /// the boot-loaded catalog — `vars` ride as interpolation params, so a
    /// translated template reaches the adapter fully rendered. A template
    /// that is not a catalog key stands exactly as given (literal
    /// templates never change meaning).
    fn localized(&self, msg: &NotifyMessage) -> Option<NotifyMessage> {
        let catalog = self.messages.as_ref()?;
        let locale = msg
            .locale
            .as_deref()
            .unwrap_or_else(|| catalog.default_locale());
        let params: Vec<(&str, &str)> = msg
            .vars
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let out = keel_foundation::translate(catalog, locale, &msg.template, &params);
        // No locale hit (key miss → `locale_used` stays `None`) → the
        // literal template stands as-is.
        out.locale_used.as_ref()?;
        Some(NotifyMessage {
            template: out.text,
            ..msg.clone()
        })
    }

    /// Wave 6 P5: deliver a typed notification on one channel (19-PARITY
    /// "Notifications v2"). Mail fills `to` from the target and renders the
    /// HTML body through the view port when `to_mail()` left it unset; the
    /// database channel writes the row through the store port — async by
    /// nature, hence `async fn` while the adapter ports stay sync.
    ///
    /// `to` is the routing address (email / phone / device token) and is
    /// required for every channel except `Database`.
    pub async fn dispatch_notification(
        &self,
        n: &dyn NotificationDef,
        channel: Channel,
        to: Option<&str>,
        ctx: &NotificationContext,
    ) -> Result<(), AppError> {
        match channel {
            Channel::Database => {
                let store = self
                    .store
                    .as_ref()
                    .ok_or_else(|| AppError::not_implemented("notification database store"))?;
                store
                    .insert(NotificationRow {
                        id: Uuid::now_v7(),
                        principal_id: ctx.principal_id,
                        tenant_id: ctx.tenant_id,
                        kind: n.type_key().to_string(),
                        data: n.to_database(),
                        read_at: None,
                        created_at: OffsetDateTime::now_utc(),
                    })
                    .await
            }
            Channel::Email => {
                let target = delivery_target(channel, to)?;
                let mut mail = n.to_mail();
                // The dispatcher is the routing authority: `to_mail()`
                // carries subject/body, never the recipient.
                mail.to = vec![target.to_string()];
                if !mail.attachments.is_empty() {
                    return Err(AppError::not_implemented(
                        "notification mail attachments (resolve through StoragePort and send via MailPort::send_mailable)",
                    ));
                }
                if mail.html.is_none() {
                    if let Some(template) = n.template() {
                        if let Some(renderer) = &self.views {
                            mail.html = Some(renderer.render(&template, n.to_database())?);
                        }
                    }
                }
                self.mail
                    .as_ref()
                    .ok_or_else(|| AppError::not_implemented("mail adapter"))?
                    .send_mailable(&mail, &[])
            }
            Channel::Sms | Channel::Push => {
                let target = delivery_target(channel, to)?;
                let (template, vars) = match channel {
                    Channel::Sms => (n.to_sms(), BTreeMap::new()),
                    _ => push_nudge(n),
                };
                self.dispatch(&NotifyMessage {
                    channel,
                    to: target.to_string(),
                    template,
                    vars,
                    tenant_id: ctx.tenant_id,
                    locale: ctx.locale.clone(),
                })
            }
        }
    }
}

/// Target presence check for the routing channels (the database row needs
/// no address — it is addressed by principal).
fn delivery_target(channel: Channel, to: Option<&str>) -> Result<&str, AppError> {
    let target = to.unwrap_or_default().trim();
    if target.is_empty() {
        return Err(AppError::validation(format!(
            "notification {channel:?} channel requires a delivery target"
        )));
    }
    Ok(target)
}

/// D-17 nudge-not-payload: the push `NotifyMessage` carries the signal
/// (payload `signal`, defaulting to the type key) plus an optional
/// `delta_cursor` — never free-form data.
fn push_nudge(n: &dyn NotificationDef) -> (String, BTreeMap<String, String>) {
    let payload = n.to_push();
    let signal = payload
        .get("signal")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| n.type_key())
        .to_string();
    let mut vars = BTreeMap::new();
    if let Some(cursor) = payload.get("delta_cursor").and_then(serde_json::Value::as_str) {
        vars.insert("delta_cursor".to_string(), cursor.to_string());
    }
    (signal, vars)
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
            locale: None,
        };
        let err = hub.dispatch(&msg).unwrap_err();
        assert_eq!(err.code, keel_foundation::codes::NOT_IMPLEMENTED);
    }

    /// Wave 6 P3: a catalog-carrying hub resolves notification templates
    /// as keys (vars interpolated, per-message locale winning over the
    /// catalog default); literal templates and catalog-less hubs are
    /// untouched.
    #[test]
    fn hub_localizes_template_keys_through_the_catalog() {
        let mut catalog = keel_foundation::MessageCatalog::new("en");
        catalog
            .load("en", &[("notify.booked", "Booked for {patient}")])
            .unwrap();
        catalog
            .define("fr", "notify.booked", "Réservé pour {patient}")
            .unwrap();
        let hub = NotifyHub::with_catalog(std::sync::Arc::new(catalog));

        let msg = NotifyMessage {
            channel: Channel::Sms,
            to: "+1".into(),
            template: "notify.booked".into(),
            vars: BTreeMap::from([("patient".to_string(), "Ada".into())]),
            tenant_id: None,
            locale: Some("fr".into()),
        };
        let localized = hub.localized(&msg).expect("key resolves");
        assert_eq!(localized.template, "Réservé pour Ada");

        // No per-message locale → the catalog default locale.
        let msg = NotifyMessage {
            locale: None,
            ..msg.clone()
        };
        let localized = hub.localized(&msg).expect("key resolves");
        assert_eq!(localized.template, "Booked for Ada");

        // A literal template (not a key) stands exactly as given.
        let msg = NotifyMessage {
            template: "literal-body".into(),
            ..msg
        };
        assert!(hub.localized(&msg).is_none());

        // No catalog → never localized (the `empty()` posture).
        assert!(NotifyHub::empty().localized(&msg).is_none());
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

    // ------------------------------------------------- Wave 6 P5: notifications v2

    /// Sample typed notification exercising every channel renderer.
    struct BookedNotification {
        patient: &'static str,
        slots: u32,
    }

    impl NotificationDef for BookedNotification {
        fn type_key(&self) -> &str {
            "appointment.booked"
        }
        fn to_mail(&self) -> Mailable {
            Mailable {
                to: vec![],
                cc: vec![],
                bcc: vec![],
                from: None,
                reply_to: None,
                subject: format!("Booked for {}", self.patient),
                text: Some(format!(
                    "{} appointment(s) for {}",
                    self.slots, self.patient
                )),
                html: None,
                headers: vec![],
                attachments: vec![],
            }
        }
        fn to_sms(&self) -> String {
            format!("Booked for {}", self.patient)
        }
        fn to_push(&self) -> serde_json::Value {
            serde_json::json!({ "delta_cursor": "42" })
        }
        fn to_database(&self) -> serde_json::Value {
            serde_json::json!({ "patient": self.patient, "slots": self.slots })
        }
    }

    /// A notification that attaches through the storage-backed path — the
    /// dispatch path must refuse it (attachments need the P1 resolve seam).
    struct AttachmentsNotification;

    impl NotificationDef for AttachmentsNotification {
        fn type_key(&self) -> &str {
            "invoice.ready"
        }
        fn to_mail(&self) -> Mailable {
            let mut m = mail();
            m.attachments.push(attachment("invoice.pdf"));
            m
        }
        fn to_sms(&self) -> String {
            "invoice ready".into()
        }
        fn to_push(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn to_database(&self) -> serde_json::Value {
            serde_json::json!({})
        }
    }

    #[derive(Default)]
    struct RecordingMail {
        sent: std::sync::Arc<Mutex<Vec<Mailable>>>,
    }

    impl MailPort for RecordingMail {
        fn send(&self, _msg: &NotifyMessage, _subject: &str) -> Result<(), AppError> {
            Ok(())
        }
        fn send_mailable(
            &self,
            mail: &Mailable,
            _attachments: &[ResolvedMailAttachment],
        ) -> Result<(), AppError> {
            // Real transports validate before touching the network.
            mail.validate()?;
            self.sent.lock().unwrap().push(mail.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingSms {
        sent: std::sync::Arc<Mutex<Vec<NotifyMessage>>>,
    }

    impl SmsPort for RecordingSms {
        fn send(&self, msg: &NotifyMessage) -> Result<(), AppError> {
            self.sent.lock().unwrap().push(msg.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingPush {
        sent: std::sync::Arc<Mutex<Vec<NotifyMessage>>>,
    }

    impl PushPort for RecordingPush {
        fn send(&self, msg: &NotifyMessage) -> Result<(), AppError> {
            self.sent.lock().unwrap().push(msg.clone());
            Ok(())
        }
    }

    /// Renders `templates/<template>` decorated with the `patient` from the
    /// context (proves template name + `to_database()` context reach the view).
    struct EchoRenderer;

    impl NotificationViewRenderer for EchoRenderer {
        fn render(
            &self,
            template: &str,
            context: serde_json::Value,
        ) -> Result<String, AppError> {
            Ok(format!(
                "<view:{template}>{}</view>",
                context["patient"].as_str().unwrap_or("")
            ))
        }
    }

    struct BrokenRenderer;

    impl NotificationViewRenderer for BrokenRenderer {
        fn render(
            &self,
            _template: &str,
            _context: serde_json::Value,
        ) -> Result<String, AppError> {
            Err(AppError::internal("view exploded"))
        }
    }

    fn ctx_for(principal: PrincipalId) -> NotificationContext {
        NotificationContext {
            principal_id: principal,
            tenant_id: None,
            locale: None,
        }
    }

    /// One typed notification reaches every port: mail carries the filled
    /// `to`, the rendered HTML view and the text body; SMS carries the
    /// rendered text; push carries the nudge (signal defaults to the type
    /// key, delta cursor forwarded, patient data never leaked); the database
    /// channel writes the row.
    #[tokio::test]
    async fn typed_notification_renders_every_channel_through_the_ports() {
        let mail_log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sms_log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let push_log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let store = std::sync::Arc::new(InMemoryNotificationStore::new());

        let mut hub = NotifyHub::empty()
            .with_store(store.clone())
            .with_view_renderer(std::sync::Arc::new(EchoRenderer));
        hub.mail = Some(Box::new(RecordingMail {
            sent: mail_log.clone(),
        }));
        hub.sms = Some(Box::new(RecordingSms {
            sent: sms_log.clone(),
        }));
        hub.push = Some(Box::new(RecordingPush {
            sent: push_log.clone(),
        }));

        let note = BookedNotification {
            patient: "Ada",
            slots: 2,
        };
        let ctx = ctx_for(PrincipalId::new());

        hub.dispatch_notification(&note, Channel::Email, Some("ada@example.com"), &ctx)
            .await
            .expect("mail dispatches");
        {
            let sent = mail_log.lock().unwrap();
            assert_eq!(sent.len(), 1);
            let mail = &sent[0];
            assert_eq!(mail.to, vec!["ada@example.com".to_string()]);
            assert_eq!(mail.subject, "Booked for Ada");
            assert_eq!(
                mail.text.as_deref(),
                Some("2 appointment(s) for Ada")
            );
            assert_eq!(
                mail.html.as_deref(),
                Some("<view:notifications.appointment.booked>Ada</view>"),
                "default template path renders to_database() context"
            );
        }

        hub.dispatch_notification(&note, Channel::Sms, Some("+1555000"), &ctx)
            .await
            .expect("sms dispatches");
        {
            let sent = sms_log.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].template, "Booked for Ada");
            assert_eq!(sent[0].channel, Channel::Sms);
        }

        hub.dispatch_notification(&note, Channel::Push, Some("device-1"), &ctx)
            .await
            .expect("push dispatches");
        {
            let sent = push_log.lock().unwrap();
            assert_eq!(sent.len(), 1);
            // Nudge-not-payload: signal defaults to the type key, the cursor
            // rides, the patient never does.
            assert_eq!(sent[0].template, "appointment.booked");
            assert_eq!(
                sent[0].vars.get("delta_cursor").map(String::as_str),
                Some("42")
            );
            assert!(!sent[0].vars.contains_key("patient"));
        }

        hub.dispatch_notification(&note, Channel::Database, None, &ctx)
            .await
            .expect("database dispatches");
        let page = store.list(ctx.principal_id, 0, 10).await.expect("lists");
        assert_eq!(page.total, 1);
        assert_eq!(page.unread, 1);
        let row = &page.rows[0];
        assert_eq!(row.kind, "appointment.booked");
        assert_eq!(row.data, serde_json::json!({ "patient": "Ada", "slots": 2 }));
        assert!(row.read_at.is_none());
        assert_eq!(row.principal_id, ctx.principal_id);
    }

    /// Without a view port the text body stands (degradation, matching
    /// `AppState::views`); with a broken template the dispatch fails closed.
    #[tokio::test]
    async fn mail_html_degrades_without_a_port_and_fails_closed_when_broken() {
        let note = BookedNotification {
            patient: "Ada",
            slots: 1,
        };
        let ctx = ctx_for(PrincipalId::new());

        // No renderer: html stays unset, the text body carries the mail.
        let mail_log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let mut hub = NotifyHub::empty();
        hub.mail = Some(Box::new(RecordingMail {
            sent: mail_log.clone(),
        }));
        hub.dispatch_notification(&note, Channel::Email, Some("ada@example.com"), &ctx)
            .await
            .expect("degraded dispatches");
        {
            let sent = mail_log.lock().unwrap();
            assert!(sent[0].html.is_none(), "no port -> text-only mail");
            assert!(sent[0].text.is_some());
        }

        // Renderer present but the template render errors -> dispatch errors.
        let mut hub = NotifyHub::empty()
            .with_view_renderer(std::sync::Arc::new(BrokenRenderer));
        hub.mail = Some(Box::new(RecordingMail::default()));
        let err = hub
            .dispatch_notification(&note, Channel::Email, Some("ada@example.com"), &ctx)
            .await
            .expect_err("broken template refuses the send");
        assert_eq!(err.code, keel_foundation::codes::INTERNAL);
    }

    /// Attachment-bearing mail cannot ride the dispatch path: the bytes need
    /// the P1 resolve seam, so the dispatch fails closed instead of dropping
    /// them silently.
    #[tokio::test]
    async fn notification_mail_attachments_refuse_the_dispatch_path() {
        let mut hub = NotifyHub::empty();
        hub.mail = Some(Box::new(RecordingMail::default()));
        let ctx = ctx_for(PrincipalId::new());
        let err = hub
            .dispatch_notification(&AttachmentsNotification, Channel::Email, Some("ops@example.com"), &ctx)
            .await
            .expect_err("attachments refuse");
        assert_eq!(err.code, keel_foundation::codes::NOT_IMPLEMENTED);
    }

    /// Missing targets, adapters and store ports are all explicit errors —
    /// never a silent drop, including the raw template `dispatch` path for
    /// the database channel.
    #[tokio::test]
    async fn dispatch_fails_closed_without_targets_adapters_or_store() {
        let hub = NotifyHub::empty();
        let ctx = ctx_for(PrincipalId::new());
        let note = BookedNotification {
            patient: "Ada",
            slots: 1,
        };

        for channel in [Channel::Email, Channel::Sms, Channel::Push] {
            let err = hub
                .dispatch_notification(&note, channel, None, &ctx)
                .await
                .expect_err("target required");
            assert_eq!(err.code, keel_foundation::codes::VALIDATION, "{channel:?}");
            let err = hub
                .dispatch_notification(&note, channel, Some("  "), &ctx)
                .await
                .expect_err("blank target refused");
            assert_eq!(err.code, keel_foundation::codes::VALIDATION, "{channel:?}");
        }

        let err = hub
            .dispatch_notification(&note, Channel::Email, Some("ada@example.com"), &ctx)
            .await
            .expect_err("adapter missing");
        assert_eq!(err.code, keel_foundation::codes::NOT_IMPLEMENTED);

        let err = hub
            .dispatch_notification(&note, Channel::Database, None, &ctx)
            .await
            .expect_err("store missing");
        assert_eq!(err.code, keel_foundation::codes::NOT_IMPLEMENTED);

        let raw = NotifyMessage {
            channel: Channel::Database,
            to: String::new(),
            template: "appointment.booked".into(),
            vars: BTreeMap::new(),
            tenant_id: None,
            locale: None,
        };
        let err = hub.dispatch(&raw).expect_err("raw path refuses rows");
        assert_eq!(err.code, keel_foundation::codes::NOT_IMPLEMENTED);
    }

    /// The database channel scoping + pagination contract: principal-scoped
    /// reads, newest-first order, feed counts over ALL rows, and idempotent
    /// mark-read that another principal can never apply.
    #[tokio::test]
    async fn database_channel_scopes_reads_marks_idempotently_and_paginates() {
        let store = std::sync::Arc::new(InMemoryNotificationStore::new());
        let hub = NotifyHub::empty().with_store(store.clone());
        let mine = PrincipalId::new();
        let other = PrincipalId::new();
        let ctx_mine = ctx_for(mine);
        let ctx_other = ctx_for(other);
        let note = BookedNotification {
            patient: "Ada",
            slots: 1,
        };

        for _ in 0..3 {
            hub.dispatch_notification(&note, Channel::Database, None, &ctx_mine)
                .await
                .expect("row inserts");
        }
        hub.dispatch_notification(&note, Channel::Database, None, &ctx_other)
            .await
            .expect("other row inserts");

        let first = store.list(mine, 0, 2).await.expect("page one");
        assert_eq!(first.total, 3, "counts span every row of the principal");
        assert_eq!(first.unread, 3);
        assert_eq!(first.rows.len(), 2);
        let second = store.list(mine, 2, 2).await.expect("page two");
        assert_eq!(second.rows.len(), 1);

        // Newest-first (created_at/id descending) regardless of page.
        let all = store.list(mine, 0, 10).await.expect("all rows");
        let ids: Vec<Uuid> = all.rows.iter().map(|r| r.id).collect();
        let mut desc = ids.clone();
        desc.sort_by(|a, b| b.cmp(a));
        assert_eq!(ids, desc);

        // Mark-read is principal-scoped and idempotent.
        let target: Vec<Uuid> = all.rows.iter().take(2).map(|r| r.id).collect();
        let foreign = store
            .mark_read(other, target.clone())
            .await
            .expect("attempt");
        assert_eq!(foreign, 0, "another principal cannot mark my rows");
        let updated = store.mark_read(mine, target.clone()).await.expect("marks");
        assert_eq!(updated, 2);
        let again = store
            .mark_read(mine, target.clone())
            .await
            .expect("re-marks");
        assert_eq!(again, 0, "already-read rows never recount");

        let after = store.list(mine, 0, 10).await.expect("counts");
        assert_eq!(after.total, 3);
        assert_eq!(after.unread, 1, "exactly one row stays open");

        // The other principal's row is untouched and still unread.
        let theirs = store.list(other, 0, 10).await.expect("their rows");
        assert_eq!(theirs.total, 1);
        assert_eq!(theirs.unread, 1);
    }
}
