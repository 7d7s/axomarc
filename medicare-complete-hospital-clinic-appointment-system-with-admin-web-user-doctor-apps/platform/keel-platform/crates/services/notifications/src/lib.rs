//! # kelson-notifications
//! Notification ports (MSG91 SMS / SMTP mail / FCM push per D-16..D-18).
//! Adapters implement; hub dispatches; failures follow the external dependency
//! behavior matrix (timeout, 429, duplicate risk...).

use kelson_foundation::{AppError, CorrelationId, PrincipalId, TenantId};
use kelson_storage::{ObjectRef, StoragePort};
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
/// `UploadLimits::default().max_file_bytes` (8 MiB). A kelson-demo test pins the
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
    /// Extra RFC822 headers (`X-Kelson-Tenant`, `List-Unsubscribe`, ...).
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

    /// Wave 6 P6: queued-delivery opt-in (19-PARITY `queue_via =
    /// "<job_type>"`): `Some(job)` renders this notification into a
    /// [`QueuedNotificationDelivery`] envelope and hands it to the hub's
    /// [`NotificationQueuePort`] under that job name instead of
    /// delivering inline. `None` (the default) keeps dispatch immediate —
    /// "immediate dispatch remains the default". A def returning `Some`
    /// while the host wired no port fails closed as `not_implemented`
    /// (never a silent drop).
    fn queue_via(&self) -> Option<&str> {
        None
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
    /// The **origin** of this dispatch: the correlation of the interaction it
    /// belongs to and of the thing that directly caused it (the request).
    ///
    /// A queued delivery stamps this onto the durable job row, so the worker
    /// can be traced back to the request that caused the work. A dispatch with
    /// no inbound request (a scheduled sweep, an admin action) leaves it
    /// empty — that is the correct value there, not a gap.
    pub origin: DeliveryOrigin,
}

/// The correlation ids a dispatch (and the durable job it enqueues) carries.
/// `Default` = no inbound request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryOrigin {
    /// The correlation of the business interaction this work belongs to.
    pub correlation_id: Option<CorrelationId>,
    /// The correlation of the direct cause — normally the request.
    pub causation_id: Option<CorrelationId>,
}

impl DeliveryOrigin {
    /// An origin built from a request's correlation: the interaction and the
    /// cause are the same id (the request started the interaction).
    pub fn from_request(correlation_id: CorrelationId) -> Self {
        Self {
            correlation_id: Some(correlation_id),
            causation_id: Some(correlation_id),
        }
    }

    /// Is there an inbound request behind this dispatch at all?
    pub fn is_empty(&self) -> bool {
        self.correlation_id.is_none() && self.causation_id.is_none()
    }
}

// --------------------------------------------------- Wave 6 P6: on-demand + queued delivery

/// Wave 6 P6: context for a delivery to a RAW target — an on-demand
/// notifiable has no principal record behind the address (19-PARITY
/// "on-demand notifiables": invites and ops mail to an address the
/// identity store has never seen).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OnDemandContext {
    /// Tenant the render runs under (`None` = platform-scoped).
    pub tenant_id: Option<TenantId>,
    /// Catalog locale for the render (`None` = the catalog default).
    #[serde(default)]
    pub locale: Option<String>,
    /// The origin of the dispatch (the request behind it), stamped onto a
    /// queued delivery's durable job like any other dispatch.
    #[serde(default)]
    pub origin: DeliveryOrigin,
}

/// Wave 6 P6: audit evidence for one on-demand dispatch (19-PARITY
/// "stores an audit entry with a hash of the target — not the raw
/// address"). The raw target never leaves
/// [`NotifyHub::dispatch_on_demand`]; only `target_hash` does, so the
/// entry is safe to persist, log or thread through an execution unit's
/// `record_audit` detail. The hub writes no audit rows itself — D-74:
/// audits ride the engine transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OnDemandAudit {
    pub type_key: String,
    pub channel: Channel,
    /// sha256 hex of the trimmed, as-given target (no case folding).
    pub target_hash: String,
    #[serde(with = "time::serde::iso8601")]
    pub at: OffsetDateTime,
}

impl OnDemandAudit {
    /// Audit action name for `AuditRecord::action` when a producer records
    /// the returned entry in-unit.
    pub const ACTION: &'static str = "notification.on_demand";

    /// sha256 hex of the target exactly as it is delivered (trimmed).
    pub fn hash_target(target: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(target.as_bytes());
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// The `detail` value for an `AuditRecord`: carries the hash, never
    /// the raw target.
    pub fn detail(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({}))
    }
}

/// Wave 6 P6: the rendered artifact one channel carries through the queue.
/// Everything is rendered at enqueue time (locale, target and HTML are
/// request-time knowledge); the consumer half only performs I/O.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "channel", rename_all = "snake_case")]
pub enum DeliveryPayload {
    Mail(Mailable),
    Sms(NotifyMessage),
    Push(NotifyMessage),
    Database(NotificationRow),
}

/// Wave 6 P6: envelope queued under a def's `queue_via` job name —
/// versioned (`v`) so a consumer refuses a payload shape it does not know
/// (retry → dead-letter) instead of guessing at it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuedNotificationDelivery {
    /// The envelope version this build renders and accepts.
    pub v: u8,
    pub type_key: String,
    pub locale: Option<String>,
    pub principal_id: Option<PrincipalId>,
    pub tenant_id: Option<TenantId>,
    #[serde(with = "time::serde::iso8601")]
    pub at: OffsetDateTime,
    pub payload: DeliveryPayload,
}

impl QueuedNotificationDelivery {
    /// The envelope version this build renders and drains.
    pub const VERSION: u8 = 1;
}

/// Wave 6 P6: job name of the queued delivery — a def's `queue_via`
/// returns it, kelson-demo registers the buildable def (enqueue side), and
/// kelson-worker registers the matching drain handler. One name, both
/// halves: a typo fails `registry.build` loudly at dispatch.
pub const NOTIFICATION_SEND_JOB: &str = "notification.send";

/// Wave 6 P6 queue seam (19-PARITY "queued delivery"): the hub renders
/// the channel delivery and hands it to the port under the def's
/// `queue_via` job name; the host implements the port over its job store
/// and job registry (`registry.build` refuses an unregistered name — a
/// `queue_via` typo is a loud error, never a silent drop).
pub trait NotificationQueuePort: Send + Sync {
    fn enqueue(
        &self,
        job_type: &str,
        delivery: &QueuedNotificationDelivery,
        origin: &DeliveryOrigin,
    ) -> Result<(), AppError>;
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

/// Wave 6 P5: database-channel port. Implemented by kelson-postgres (the
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
/// views engine reaches the hub through this port — kelson-demo adapts
/// `ViewsEngine` onto it.
pub trait NotificationViewRenderer: Send + Sync {
    fn render(&self, template: &str, context: serde_json::Value) -> Result<String, AppError>;
}

/// Wave 7 P1: one recorded dispatch (19-PARITY testkit `NotificationFake`) —
/// who it was for, which notification type, on which channel. `principal` is
/// `None` for on-demand dispatches (no principal record stands behind a raw
/// target).
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchRecord {
    pub principal: Option<PrincipalId>,
    pub notification_type: String,
    pub channel: Channel,
}

/// Wave 7 P1: dispatch observation seam (19-PARITY testkit
/// `NotificationFake`). The hub taps each SUCCESSFUL dispatch at the two
/// `NotificationDef` entry points (`dispatch_notification`,
/// `dispatch_on_demand`); [`NotifyHub::deliver_queued`] deliberately does
/// NOT tap — it fulfils a dispatch the entry point already recorded, and
/// tapping there would double count. The legacy raw
/// [`NotifyHub::dispatch`] path carries no type key, so it is not tapped
/// either. `None` (the default) records nothing.
pub trait DispatchTap: Send + Sync {
    fn on_dispatch(&self, record: &DispatchRecord);
}

/// Dispatch hub: route by channel. Missing adapter = explicit error, never silent.
pub struct NotifyHub {
    pub sms: Option<Box<dyn SmsPort>>,
    pub mail: Option<Box<dyn MailPort>>,
    pub push: Option<Box<dyn PushPort>>,
    /// Wave 6 P3: the boot-loaded message catalog for localized
    /// notification strings; `None` (`empty()`) keeps templates literal.
    messages: Option<std::sync::Arc<kelson_foundation::MessageCatalog>>,
    /// Wave 6 P5: database-channel store (`None` = unwired → the explicit
    /// missing-adapter error on the `Database` channel).
    store: Option<std::sync::Arc<dyn NotificationStore>>,
    /// Wave 6 P5: HTML body renderer for `views/notifications/` (`None` =
    /// text-only mail, the `views: Option` degradation posture).
    views: Option<std::sync::Arc<dyn NotificationViewRenderer>>,
    /// Wave 6 P6: queue seam for defs opting into
    /// [`NotificationDef::queue_via`] (`None` = immediate dispatch only;
    /// a `queue_via` def with no port fails closed as `not_implemented`).
    queue: Option<Box<dyn NotificationQueuePort>>,
    /// Wave 7 P1: dispatch observation hook (testkit `NotificationFake`);
    /// `None` (the default) records nothing.
    tap: Option<std::sync::Arc<dyn DispatchTap>>,
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
            queue: None,
            tap: None,
        }
    }

    /// Wave 6 P3: a hub that resolves notification strings through the
    /// boot-loaded catalog (producers carry the request locale per
    /// message; `None` falls back to the catalog default locale).
    pub fn with_catalog(messages: std::sync::Arc<kelson_foundation::MessageCatalog>) -> Self {
        Self {
            sms: None,
            mail: None,
            push: None,
            messages: Some(messages),
            store: None,
            views: None,
            queue: None,
            tap: None,
        }
    }

    /// Wave 6 P5: attach the database channel (the postgres store at boot,
    /// the in-memory store on degraded hosts).
    pub fn with_store(mut self, store: std::sync::Arc<dyn NotificationStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Wave 6 P5: attach the notification HTML view renderer (kelson-demo
    /// adapts the Wave 2 views engine onto the port).
    pub fn with_view_renderer(
        mut self,
        renderer: std::sync::Arc<dyn NotificationViewRenderer>,
    ) -> Self {
        self.views = Some(renderer);
        self
    }

    /// Wave 6 P6: attach the queued-delivery port (the host's job store +
    /// job registry glue); defs opting into `queue_via` enqueue through it
    /// instead of delivering inline.
    pub fn with_queue(mut self, port: Box<dyn NotificationQueuePort>) -> Self {
        self.queue = Some(port);
        self
    }

    /// Wave 7 P1: attach a dispatch observer (the testkit
    /// `NotificationFake` records through it); consumes the hub like the
    /// other builder seams.
    pub fn with_tap(mut self, tap: std::sync::Arc<dyn DispatchTap>) -> Self {
        self.tap = Some(tap);
        self
    }

    /// Wave 7 P1: record one successful dispatch through the attached tap
    /// (`None` tap = no-op).
    fn observe(
        &self,
        principal: Option<PrincipalId>,
        notification_type: &str,
        channel: Channel,
    ) {
        if let Some(tap) = self.tap.as_ref() {
            tap.on_dispatch(&DispatchRecord {
                principal,
                notification_type: notification_type.to_string(),
                channel,
            });
        }
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
        self.messages.as_ref()?;
        let resolved = self.resolve_key(&msg.template, msg.locale.as_deref(), &msg.vars);
        (resolved != msg.template).then(|| NotifyMessage {
            template: resolved,
            ..msg.clone()
        })
    }

    /// Wave 6 P6: one resolution path for every rendered string — `text`
    /// resolved as a catalog key in `locale` with `vars` interpolated; a
    /// missing catalog or a key miss leaves the literal standing exactly
    /// as given (the P3 posture, now shared by the SMS, mail subject/text
    /// and sync dispatch paths).
    fn resolve_key(
        &self,
        text: &str,
        locale: Option<&str>,
        vars: &BTreeMap<String, String>,
    ) -> String {
        let Some(catalog) = self.messages.as_ref() else {
            return text.to_string();
        };
        let resolved = locale.unwrap_or_else(|| catalog.default_locale());
        let params: Vec<(&str, &str)> = vars
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let out = kelson_foundation::translate(catalog, resolved, text, &params);
        match out.locale_used {
            Some(_) => out.text,
            None => text.to_string(),
        }
    }

    /// Wave 6 P5: deliver a typed notification on one channel (19-PARITY
    /// "Notifications v2"). Mail fills `to` from the target and renders the
    /// HTML body through the view port when `to_mail()` left it unset; the
    /// database channel writes the row through the store port — async by
    /// nature, hence `async fn` while the adapter ports stay sync.
    ///
    /// Wave 6 P6: a def opting into [`NotificationDef::queue_via`] is
    /// RENDERED here (request-time locale, target and views) and handed to
    /// the [`NotificationQueuePort`] envelope instead of delivered inline;
    /// immediate dispatch remains the default.
    ///
    /// `to` is the routing address (email / phone / device token) and is
    /// required for every channel except `Database`.
    ///
    /// Wave 7 P1: a dispatch that returns `Ok` is tapped through
    /// [`DispatchTap`] when one is wired (a failed render/delivery/enqueue
    /// records nothing).
    pub async fn dispatch_notification(
        &self,
        n: &dyn NotificationDef,
        channel: Channel,
        to: Option<&str>,
        ctx: &NotificationContext,
    ) -> Result<(), AppError> {
        let at = OffsetDateTime::now_utc();
        let payload = self.render_delivery(
            n,
            channel,
            to,
            Some(ctx.principal_id),
            ctx.tenant_id,
            ctx.locale.as_deref(),
        )?;
        let dispatched = match n.queue_via() {
            Some(job) => {
                let port = self
                    .queue
                    .as_ref()
                    .ok_or_else(|| AppError::not_implemented("notification queue port"))?;
                port.enqueue(
                    job,
                    &envelope_for(
                        n.type_key(),
                        ctx.locale.clone(),
                        Some(ctx.principal_id),
                        ctx.tenant_id,
                        at,
                        payload,
                    ),
                    &ctx.origin,
                )
            }
            None => self.deliver_payload(payload).await,
        };
        if dispatched.is_ok() {
            // Wave 7 P1: only successful dispatches reach the tap.
            self.observe(Some(ctx.principal_id), n.type_key(), channel);
        }
        dispatched
    }

    /// Wave 6 P6: deliver to a RAW target with no principal record behind
    /// it (19-PARITY "on-demand notifiables"). Routing channels only — the
    /// database row is addressed by principal, so `Channel::Database` is
    /// refused up front.
    ///
    /// Returns the audit evidence: the target is trimmed, rendered and
    /// delivered right here, and only its sha256 hash leaves the call —
    /// persist [`OnDemandAudit::detail`] through an execution unit's
    /// `record_audit` (D-74: audits ride the engine transaction; the hub
    /// writes no audit rows itself).
    pub async fn dispatch_on_demand(
        &self,
        n: &dyn NotificationDef,
        channel: Channel,
        target: &str,
        ctx: &OnDemandContext,
    ) -> Result<OnDemandAudit, AppError> {
        if channel == Channel::Database {
            return Err(AppError::validation(
                "on-demand notifications address a raw target; the database channel requires a principal",
            ));
        }
        let at = OffsetDateTime::now_utc();
        let target = delivery_target(channel, Some(target))?.to_string();
        let payload = self.render_delivery(
            n,
            channel,
            Some(&target),
            None,
            ctx.tenant_id,
            ctx.locale.as_deref(),
        )?;
        match n.queue_via() {
            Some(job) => {
                let port = self
                    .queue
                    .as_ref()
                    .ok_or_else(|| AppError::not_implemented("notification queue port"))?;
                port.enqueue(
                    job,
                    &envelope_for(
                        n.type_key(),
                        ctx.locale.clone(),
                        None,
                        ctx.tenant_id,
                        at,
                        payload,
                    ),
                    &ctx.origin,
                )?;
            }
            None => self.deliver_payload(payload).await?,
        }
        // Wave 7 P1: reaching here means the dispatch succeeded (every
        // failure `?`-ed out above); an on-demand dispatch has no principal.
        self.observe(None, n.type_key(), channel);
        Ok(OnDemandAudit {
            type_key: n.type_key().to_string(),
            channel,
            target_hash: OnDemandAudit::hash_target(&target),
            at,
        })
    }

    /// Wave 6 P6: drain one queued delivery — the consumer surface a
    /// `notification.send` job handler calls (19-PARITY "queued
    /// delivery"). The payload is pre-rendered at enqueue (localized text,
    /// filled target, viewed HTML); this half only performs I/O. An
    /// envelope version this build does not know is refused loudly (retry
    /// → dead-letter) rather than guessed at.
    pub async fn deliver_queued(
        &self,
        delivery: QueuedNotificationDelivery,
    ) -> Result<(), AppError> {
        if delivery.v != QueuedNotificationDelivery::VERSION {
            return Err(AppError::validation(format!(
                "notification delivery envelope v{} (this build renders v{})",
                delivery.v,
                QueuedNotificationDelivery::VERSION
            )));
        }
        self.deliver_payload(delivery.payload).await
    }

    /// Wave 6 P6: render the channel artifact WITHOUT performing I/O —
    /// the single definition the immediate path and the queued envelope
    /// share (locale, target and views are request-time knowledge; a
    /// queued consumer only performs I/O). `principal: None` marks an
    /// on-demand delivery; only routing channels may go on-demand.
    fn render_delivery(
        &self,
        n: &dyn NotificationDef,
        channel: Channel,
        to: Option<&str>,
        principal: Option<PrincipalId>,
        tenant_id: Option<TenantId>,
        locale: Option<&str>,
    ) -> Result<DeliveryPayload, AppError> {
        match channel {
            Channel::Database => {
                let principal_id = principal.ok_or_else(|| {
                    AppError::validation(
                        "on-demand notifications address a raw target; the database channel requires a principal",
                    )
                })?;
                Ok(DeliveryPayload::Database(NotificationRow {
                    id: Uuid::now_v7(),
                    principal_id,
                    tenant_id,
                    kind: n.type_key().to_string(),
                    data: n.to_database(),
                    read_at: None,
                    created_at: OffsetDateTime::now_utc(),
                }))
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
                // Wave 6 P6: subject and text resolve as catalog keys with
                // the database payload's top-level scalars as
                // `{patient}`-style params (a literal stands as given).
                let vars = payload_vars(n);
                mail.subject = self.resolve_key(&mail.subject, locale, &vars);
                if let Some(text) = mail.text.take() {
                    mail.text = Some(self.resolve_key(&text, locale, &vars));
                }
                if mail.html.is_none() {
                    if let Some(template) = n.template() {
                        if let Some(renderer) = &self.views {
                            mail.html = Some(renderer.render(
                                &template,
                                view_context(n, locale, self.messages.as_deref()),
                            )?);
                        }
                    }
                }
                Ok(DeliveryPayload::Mail(mail))
            }
            Channel::Sms => {
                let target = delivery_target(channel, to)?;
                let vars = payload_vars(n);
                Ok(DeliveryPayload::Sms(NotifyMessage {
                    channel: Channel::Sms,
                    to: target.to_string(),
                    template: self.resolve_key(&n.to_sms(), locale, &vars),
                    vars: BTreeMap::new(),
                    tenant_id,
                    locale: locale.map(str::to_string),
                }))
            }
            Channel::Push => {
                let target = delivery_target(channel, to)?;
                let (template, vars) = push_nudge(n);
                Ok(DeliveryPayload::Push(NotifyMessage {
                    channel: Channel::Push,
                    to: target.to_string(),
                    template,
                    vars,
                    tenant_id,
                    locale: locale.map(str::to_string),
                }))
            }
        }
    }

    /// Wave 6 P6: the I/O half of a rendered delivery — the shared tail of
    /// the immediate dispatch paths and [`Self::deliver_queued`].
    async fn deliver_payload(&self, payload: DeliveryPayload) -> Result<(), AppError> {
        match payload {
            DeliveryPayload::Mail(mail) => self
                .mail
                .as_ref()
                .ok_or_else(|| AppError::not_implemented("mail adapter"))?
                .send_mailable(&mail, &[]),
            DeliveryPayload::Sms(msg) => self
                .sms
                .as_ref()
                .ok_or_else(|| AppError::not_implemented("sms adapter"))?
                .send(&msg),
            DeliveryPayload::Push(msg) => self
                .push
                .as_ref()
                .ok_or_else(|| AppError::not_implemented("push adapter"))?
                .send(&msg),
            DeliveryPayload::Database(row) => {
                let store = self
                    .store
                    .as_ref()
                    .ok_or_else(|| AppError::not_implemented("notification database store"))?;
                store.insert(row).await
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

/// Wave 6 P6: build the queued-delivery envelope around a rendered
/// payload (one `at` shared by the envelope and its on-demand audit).
fn envelope_for(
    type_key: &str,
    locale: Option<String>,
    principal_id: Option<PrincipalId>,
    tenant_id: Option<TenantId>,
    at: OffsetDateTime,
    payload: DeliveryPayload,
) -> QueuedNotificationDelivery {
    QueuedNotificationDelivery {
        v: QueuedNotificationDelivery::VERSION,
        type_key: type_key.to_string(),
        locale,
        principal_id,
        tenant_id,
        at,
        payload,
    }
}

/// Wave 6 P6: the database payload's top-level scalars as catalog
/// interpolation params — `{"patient": "Ada", "slots": 1}` becomes
/// `{patient}` / `{slots}` for the `{patient}`-style notification strings
/// (nested objects, arrays and nulls are not params).
fn payload_vars(n: &dyn NotificationDef) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    if let Some(object) = n.to_database().as_object() {
        for (key, value) in object {
            let scalar = match value {
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Number(number) => Some(number.to_string()),
                serde_json::Value::Bool(flag) => Some(flag.to_string()),
                _ => None,
            };
            if let Some(scalar) = scalar {
                vars.insert(key.clone(), scalar);
            }
        }
    }
    vars
}

/// Wave 6 P6: the view context the notification HTML body renders with —
/// `to_database()` exactly as P5, plus the route-identical locale
/// injections (`_locale` + the flat `_lang` map) when the hub carries a
/// catalog, so notification views can use `@lang` / the `trans` filter
/// the same way request-bound views do. No catalog = the P5 context,
/// byte for byte.
fn view_context(
    n: &dyn NotificationDef,
    locale: Option<&str>,
    catalog: Option<&kelson_foundation::MessageCatalog>,
) -> serde_json::Value {
    let mut context = n.to_database();
    if let Some(catalog) = catalog {
        let locale = locale.unwrap_or_else(|| catalog.default_locale());
        if let Some(object) = context.as_object_mut() {
            object.insert(
                "_locale".into(),
                serde_json::Value::String(locale.to_string()),
            );
            object.insert(
                "_lang".into(),
                serde_json::to_value(catalog.flat(locale)).unwrap_or_default(),
            );
        }
    }
    context
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
        assert_eq!(err.code, kelson_foundation::codes::NOT_IMPLEMENTED);
    }

    /// Wave 6 P3: a catalog-carrying hub resolves notification templates
    /// as keys (vars interpolated, per-message locale winning over the
    /// catalog default); literal templates and catalog-less hubs are
    /// untouched.
    #[test]
    fn hub_localizes_template_keys_through_the_catalog() {
        let mut catalog = kelson_foundation::MessageCatalog::new("en");
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
        assert_eq!(err.code, kelson_foundation::codes::CAP_LIMIT);
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
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);
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
        m.headers.push(("X-Kelson-Tenant".into(), "t-1".into()));
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
            origin: DeliveryOrigin::default(),
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
        assert_eq!(err.code, kelson_foundation::codes::INTERNAL);
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
        assert_eq!(err.code, kelson_foundation::codes::NOT_IMPLEMENTED);
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
            assert_eq!(err.code, kelson_foundation::codes::VALIDATION, "{channel:?}");
            let err = hub
                .dispatch_notification(&note, channel, Some("  "), &ctx)
                .await
                .expect_err("blank target refused");
            assert_eq!(err.code, kelson_foundation::codes::VALIDATION, "{channel:?}");
        }

        let err = hub
            .dispatch_notification(&note, Channel::Email, Some("ada@example.com"), &ctx)
            .await
            .expect_err("adapter missing");
        assert_eq!(err.code, kelson_foundation::codes::NOT_IMPLEMENTED);

        let err = hub
            .dispatch_notification(&note, Channel::Database, None, &ctx)
            .await
            .expect_err("store missing");
        assert_eq!(err.code, kelson_foundation::codes::NOT_IMPLEMENTED);

        let raw = NotifyMessage {
            channel: Channel::Database,
            to: String::new(),
            template: "appointment.booked".into(),
            vars: BTreeMap::new(),
            tenant_id: None,
            locale: None,
        };
        let err = hub.dispatch(&raw).expect_err("raw path refuses rows");
        assert_eq!(err.code, kelson_foundation::codes::NOT_IMPLEMENTED);
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

    // ------------------------------------------------- Wave 6 P6: on-demand + queued delivery

    /// Sample def that opts into queued delivery for every dispatch.
    struct QueuedBookNotification;

    impl NotificationDef for QueuedBookNotification {
        fn type_key(&self) -> &str {
            "appointment.queued"
        }
        fn to_mail(&self) -> Mailable {
            mail()
        }
        fn to_sms(&self) -> String {
            "queued".into()
        }
        fn to_push(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn to_database(&self) -> serde_json::Value {
            serde_json::json!({ "patient": "Ada" })
        }
        fn queue_via(&self) -> Option<&str> {
            Some("notification.send")
        }
    }

    /// Sample def speaking the catalog-key notification strings (the shape
    /// the appointment producer ships): mail subject/text and SMS text are
    /// `notifications.appointment_booked`; the payload carries `{patient}`.
    struct KeyedNotification {
        patient: &'static str,
    }

    impl NotificationDef for KeyedNotification {
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
                subject: "notifications.appointment_booked".into(),
                text: Some("notifications.appointment_booked".into()),
                html: None,
                headers: vec![],
                attachments: vec![],
            }
        }
        fn to_sms(&self) -> String {
            "notifications.appointment_booked".into()
        }
        fn to_push(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn to_database(&self) -> serde_json::Value {
            serde_json::json!({ "patient": self.patient })
        }
    }

    fn appointment_catalog() -> kelson_foundation::MessageCatalog {
        let mut catalog = kelson_foundation::MessageCatalog::new("en");
        catalog
            .load(
                "en",
                &[(
                    "notifications.appointment_booked",
                    "Appointment booked for {patient}",
                )],
            )
            .expect("en loads");
        catalog
            .define(
                "fr",
                "notifications.appointment_booked",
                "Rendez-vous réservé pour {patient}",
            )
            .expect("fr defines");
        catalog
    }

    #[derive(Default)]
    struct RecordingQueue {
        enqueued: std::sync::Arc<Mutex<Vec<(String, QueuedNotificationDelivery, DeliveryOrigin)>>>,
    }

    impl NotificationQueuePort for RecordingQueue {
        fn enqueue(
            &self,
            job_type: &str,
            delivery: &QueuedNotificationDelivery,
            origin: &DeliveryOrigin,
        ) -> Result<(), AppError> {
            self.enqueued
                .lock()
                .unwrap()
                .push((job_type.to_string(), delivery.clone(), *origin));
            Ok(())
        }
    }

    /// F-7 (row 7): a queued delivery carries the ORIGIN of the dispatch — the
    /// correlation of the request behind it — so the host can stamp it onto the
    /// durable job row. Without it, a worker cannot be traced back to the
    /// request that caused the work.
    #[tokio::test]
    async fn a_queued_delivery_carries_the_dispatch_origin() {
        let enqueued = std::sync::Arc::new(Mutex::new(Vec::new()));
        let hub = NotifyHub::empty().with_queue(Box::new(RecordingQueue {
            enqueued: enqueued.clone(),
        }));
        let correlation = kelson_foundation::CorrelationId::new();
        let mut ctx = ctx_for(PrincipalId::new());
        ctx.origin = DeliveryOrigin::from_request(correlation);

        hub.dispatch_notification(
            &QueuedBookNotification,
            Channel::Email,
            Some("ada@example.com"),
            &ctx,
        )
        .await
        .expect("mail enqueued");

        let captured = enqueued.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let (_, _, origin) = &captured[0];
        assert_eq!(
            origin.correlation_id,
            Some(correlation),
            "the queued delivery carries the request's correlation"
        );
        assert_eq!(
            origin.causation_id,
            Some(correlation),
            "and the request is also the direct cause"
        );
        assert!(!origin.is_empty());
    }

    /// The other half of the decision: a dispatch with NO inbound request
    /// (a scheduled sweep, an admin action) carries an EMPTY origin — that is
    /// the correct value there, not a missing stamp.
    #[tokio::test]
    async fn a_dispatch_without_a_request_carries_an_empty_origin() {
        let enqueued = std::sync::Arc::new(Mutex::new(Vec::new()));
        let hub = NotifyHub::empty().with_queue(Box::new(RecordingQueue {
            enqueued: enqueued.clone(),
        }));
        let ctx = ctx_for(PrincipalId::new());

        hub.dispatch_notification(
            &QueuedBookNotification,
            Channel::Email,
            Some("ada@example.com"),
            &ctx,
        )
        .await
        .expect("mail enqueued");

        let captured = enqueued.lock().unwrap();
        let (_, _, origin) = &captured[0];
        assert!(
            origin.is_empty(),
            "platform work carries no request correlation"
        );
    }

    /// Captures the context the HTML renderer was invoked with.
    #[derive(Clone, Default)]
    struct ContextCapture(std::sync::Arc<Mutex<Option<serde_json::Value>>>);

    impl NotificationViewRenderer for ContextCapture {
        fn render(
            &self,
            _template: &str,
            context: serde_json::Value,
        ) -> Result<String, AppError> {
            *self.0.lock().unwrap() = Some(context);
            Ok("<captured/>".into())
        }
    }

    /// Wave 6 P6: `queue_via` renders the channel into an envelope and
    /// enqueues it under the job name — with NO mail port and NO store
    /// wired, so an inline delivery would have failed closed; success
    /// proves no I/O happened at dispatch time.
    #[tokio::test]
    async fn queue_via_renders_into_an_envelope_instead_of_delivering() {
        let enqueued = std::sync::Arc::new(Mutex::new(Vec::new()));
        let hub = NotifyHub::empty().with_queue(Box::new(RecordingQueue {
            enqueued: enqueued.clone(),
        }));
        let ctx = ctx_for(PrincipalId::new());
        let queued = QueuedBookNotification;

        hub.dispatch_notification(&queued, Channel::Email, Some("ada@example.com"), &ctx)
            .await
            .expect("mail enqueued");
        hub.dispatch_notification(&queued, Channel::Database, None, &ctx)
            .await
            .expect("row enqueued");

        let captured = enqueued.lock().unwrap();
        assert_eq!(captured.len(), 2);
        let (job, mail_delivery, origin) = &captured[0];
        assert_eq!(job, "notification.send");
        assert_eq!(origin, &DeliveryOrigin::default());
        assert_eq!(mail_delivery.v, QueuedNotificationDelivery::VERSION);
        assert_eq!(mail_delivery.type_key, "appointment.queued");
        assert_eq!(mail_delivery.principal_id, Some(ctx.principal_id));
        assert!(mail_delivery.locale.is_none());
        match &mail_delivery.payload {
            DeliveryPayload::Mail(m) => {
                assert_eq!(m.to, vec!["ada@example.com".to_string()]);
                assert_eq!(m.subject, "Invoice");
            }
            other => panic!("expected a mail payload, got {other:?}"),
        }
        match &captured[1].1.payload {
            DeliveryPayload::Database(row) => {
                assert_eq!(row.kind, "appointment.queued");
                assert_eq!(row.principal_id, ctx.principal_id);
                assert_eq!(row.data, serde_json::json!({ "patient": "Ada" }));
            }
            other => panic!("expected a database payload, got {other:?}"),
        }
    }

    /// Wave 6 P6: a def opting into `queue_via` while the host wired no
    /// port fails closed as `not_implemented` — nothing is delivered
    /// inline, ever.
    #[tokio::test]
    async fn queue_via_without_a_port_fails_closed() {
        let mail_log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let mut hub = NotifyHub::empty();
        hub.mail = Some(Box::new(RecordingMail {
            sent: mail_log.clone(),
        }));
        let ctx = ctx_for(PrincipalId::new());

        let err = hub
            .dispatch_notification(
                &QueuedBookNotification,
                Channel::Email,
                Some("ada@example.com"),
                &ctx,
            )
            .await
            .expect_err("no port refuses the dispatch");
        assert_eq!(err.code, kelson_foundation::codes::NOT_IMPLEMENTED);
        assert!(
            mail_log.lock().unwrap().is_empty(),
            "nothing was delivered inline"
        );
    }

    /// Wave 6 P6 / docs-19 test plan: the on-demand dispatch delivers to
    /// the raw target (trimmed) and returns audit evidence carrying the
    /// sha256 hash — the raw address never leaves the call, not even in
    /// the persistable `detail()`.
    #[tokio::test]
    async fn on_demand_dispatch_delivers_and_returns_hashed_audit_evidence() {
        let mail_log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let mut hub = NotifyHub::empty();
        hub.mail = Some(Box::new(RecordingMail {
            sent: mail_log.clone(),
        }));

        let audit = hub
            .dispatch_on_demand(
                &BookedNotification {
                    patient: "Ada",
                    slots: 1,
                },
                Channel::Email,
                "  ada@example.com ",
                &OnDemandContext {
                    tenant_id: None,
                    locale: Some("fr".into()),
                    origin: DeliveryOrigin::default(),
                },
            )
            .await
            .expect("delivers");
        assert_eq!(
            mail_log.lock().unwrap()[0].to,
            vec!["ada@example.com".to_string()],
            "the trimmed target is the recipient"
        );

        assert_eq!(OnDemandAudit::ACTION, "notification.on_demand");
        assert_eq!(audit.type_key, "appointment.booked");
        assert_eq!(audit.channel, Channel::Email);
        assert_eq!(audit.target_hash, OnDemandAudit::hash_target("ada@example.com"));
        assert_eq!(audit.target_hash.len(), 64, "sha256 hex");
        assert_ne!(audit.target_hash, "ada@example.com");
        assert_eq!(
            OnDemandAudit::hash_target("ada@example.com"),
            OnDemandAudit::hash_target("ada@example.com"),
            "hashing is deterministic"
        );
        assert_ne!(
            OnDemandAudit::hash_target("ada@example.com"),
            OnDemandAudit::hash_target("ada@other.com"),
            "hashes are input-sensitive"
        );

        let detail = audit.detail().to_string();
        assert!(detail.contains(&audit.target_hash), "detail carries the hash");
        assert!(
            !detail.contains("ada@example.com"),
            "the raw target never escapes the dispatch"
        );
    }

    /// Wave 6 P6: the database row is addressed by principal, so on-demand
    /// dispatch refuses it; blank targets are validation errors on every
    /// routing channel.
    #[tokio::test]
    async fn on_demand_refuses_the_database_channel_and_blank_targets() {
        let hub = NotifyHub::empty()
            .with_store(std::sync::Arc::new(InMemoryNotificationStore::new()));
        let note = BookedNotification {
            patient: "Ada",
            slots: 1,
        };
        let err = hub
            .dispatch_on_demand(&note, Channel::Database, "ada@example.com", &OnDemandContext::default())
            .await
            .expect_err("database channel needs a principal");
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);

        let err = hub
            .dispatch_on_demand(&note, Channel::Email, "   ", &OnDemandContext::default())
            .await
            .expect_err("blank target refused");
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);
    }

    /// Wave 6 P6 localization: subject, text and SMS all resolve the
    /// catalog key in the dispatch locale with `{patient}` interpolated
    /// from the database payload's scalars (the lang-file contract); the
    /// catalog default locale applies when none is carried.
    #[tokio::test]
    async fn mail_and_sms_render_localized_with_payload_vars() {
        let mail_log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sms_log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let mut hub = NotifyHub::with_catalog(std::sync::Arc::new(appointment_catalog()));
        hub.mail = Some(Box::new(RecordingMail {
            sent: mail_log.clone(),
        }));
        hub.sms = Some(Box::new(RecordingSms {
            sent: sms_log.clone(),
        }));
        let note = KeyedNotification { patient: "Ada" };

        let ctx_fr = NotificationContext {
            principal_id: PrincipalId::new(),
            tenant_id: None,
            locale: Some("fr".into()),
            origin: DeliveryOrigin::default(),
        };
        hub.dispatch_notification(&note, Channel::Email, Some("ada@example.com"), &ctx_fr)
            .await
            .expect("fr mail renders");
        hub.dispatch_notification(&note, Channel::Sms, Some("+1555000"), &ctx_fr)
            .await
            .expect("fr sms renders");
        let ctx_en = NotificationContext {
            locale: Some("en".into()),
            ..ctx_fr.clone()
        };
        hub.dispatch_notification(&note, Channel::Sms, Some("+1555000"), &ctx_en)
            .await
            .expect("en sms renders");

        {
            let sent = mail_log.lock().unwrap();
            assert_eq!(sent[0].subject, "Rendez-vous réservé pour Ada");
            assert_eq!(sent[0].text.as_deref(), Some("Rendez-vous réservé pour Ada"));
        }
        let sent = sms_log.lock().unwrap();
        assert_eq!(sent[0].template, "Rendez-vous réservé pour Ada");
        assert_eq!(sent[1].template, "Appointment booked for Ada");
    }

    /// Wave 6 P6: the envelope is a wire value — it survives the serde
    /// round trip every job payload takes, drains through the consumer
    /// surface performing only I/O, and an unknown version is refused
    /// loudly (retry → dead-letter), never guessed at.
    #[tokio::test]
    async fn queued_envelope_round_trips_and_drains_through_deliver_queued() {
        let enqueued = std::sync::Arc::new(Mutex::new(Vec::new()));
        let producer = NotifyHub::empty().with_queue(Box::new(RecordingQueue {
            enqueued: enqueued.clone(),
        }));
        let ctx = ctx_for(PrincipalId::new());
        producer
            .dispatch_notification(
                &QueuedBookNotification,
                Channel::Email,
                Some("ada@example.com"),
                &ctx,
            )
            .await
            .expect("enqueued");
        let envelope = enqueued.lock().unwrap()[0].1.clone();
        let json = serde_json::to_value(&envelope).expect("serializes");
        let back: QueuedNotificationDelivery = serde_json::from_value(json).expect("deserializes");
        assert_eq!(back, envelope);

        let mail_log = std::sync::Arc::new(Mutex::new(Vec::new()));
        let mut consumer = NotifyHub::empty();
        consumer.mail = Some(Box::new(RecordingMail {
            sent: mail_log.clone(),
        }));
        consumer.deliver_queued(back).await.expect("drains");
        assert_eq!(
            mail_log.lock().unwrap()[0].to,
            vec!["ada@example.com".to_string()],
            "the pre-rendered target rides the envelope"
        );

        let future = QueuedNotificationDelivery {
            v: 99,
            ..envelope
        };
        let err = consumer
            .deliver_queued(future)
            .await
            .expect_err("unknown version refused");
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);
    }

    /// Wave 6 P6: the notification view context is `to_database()` plus
    /// the route-identical locale injections (`_locale` + flat `_lang`)
    /// when a catalog is wired — so `@lang`/`trans` work in notification
    /// views; without a catalog the P5 context stands byte for byte.
    #[tokio::test]
    async fn notification_view_context_carries_the_route_locale_injections() {
        let capture = ContextCapture::default();
        let mut hub = NotifyHub::with_catalog(std::sync::Arc::new(appointment_catalog()))
            .with_view_renderer(std::sync::Arc::new(capture.clone()));
        hub.mail = Some(Box::new(RecordingMail::default()));
        let ctx = NotificationContext {
            principal_id: PrincipalId::new(),
            tenant_id: None,
            locale: Some("fr".into()),
            origin: DeliveryOrigin::default(),
        };
        hub.dispatch_notification(
            &KeyedNotification { patient: "Ada" },
            Channel::Email,
            Some("ada@example.com"),
            &ctx,
        )
        .await
        .expect("renders");
        let context = capture.0.lock().unwrap().clone().expect("renderer ran");
        assert_eq!(context["_locale"], serde_json::json!("fr"));
        assert_eq!(
            context["_lang"]["notifications.appointment_booked"],
            serde_json::json!("Rendez-vous réservé pour {patient}")
        );
        assert_eq!(context["patient"], serde_json::json!("Ada"), "P5 payload fields intact");

        // No catalog = the P5 context, byte for byte.
        let bare = ContextCapture::default();
        let mut hub = NotifyHub::empty().with_view_renderer(std::sync::Arc::new(bare.clone()));
        hub.mail = Some(Box::new(RecordingMail::default()));
        hub.dispatch_notification(
            &KeyedNotification { patient: "Ada" },
            Channel::Email,
            Some("ada@example.com"),
            &ctx,
        )
        .await
        .expect("renders");
        let context = bare.0.lock().unwrap().clone().expect("renderer ran");
        assert_eq!(
            context,
            serde_json::json!({ "patient": "Ada" }),
            "no catalog -> no locale injections"
        );
    }
}
