//! # kelson-mail-smtp
//! SMTP mail adapter (D-18, blueprint L40). Three surfaces:
//! - `SmtpSandbox`: records rendered intents (legacy `NotifyMessage` path)
//!   and `Mailable`s, sends nothing (dev/test, degraded-boot default).
//! - `SmtpLive`: real delivery via lettre (blocking transport, rustls).
//! - `MailChain` (Wave 6 P1): the `[mail]` failover chain — an ordered list
//!   of named mailers where the first healthy transport wins; health is a
//!   TCP connect probe bounded by the mailer's timeout (`log` never probes).
//!
//! Mail sending is expected to run INSIDE a durable job (`mail.send`) so the
//! blocking IO never sits on a request path; the jobs layer supplies the
//! retry budget (retry → backoff → dead-letter).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use kelson_foundation::{codes, AppError, ErrorCategory, Retryability, Severity};
use kelson_jobs::{BoxFuture, DurableJob, JobHandler, JobRegistry, JobStore};
use kelson_notifications::{MailPort, Mailable, NotifyMessage, ResolvedMailAttachment};
use kelson_storage::StoragePort;
use lettre::message::header::{ContentType, HeaderName, HeaderValue};
use lettre::message::{Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RenderedMail {
    pub to: String,
    pub subject: String,
    pub template: String,
}

/// One `Mailable` recorded by the sandbox / `log` transport — the dev/test
/// assertion surface for queued sends (recipient sets, bodies, attachments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedMail {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachment_count: usize,
}

#[derive(Default)]
pub struct SmtpSandbox {
    /// Legacy `NotifyMessage` renders (kept for the `NotifyHub` path).
    pub outbox: Mutex<Vec<RenderedMail>>,
    /// Wave 6 P1: `Mailable`s this sandbox recorded instead of sending.
    pub mailables: Mutex<Vec<RecordedMail>>,
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

    fn send_mailable(
        &self,
        mail: &Mailable,
        attachments: &[ResolvedMailAttachment],
    ) -> Result<(), AppError> {
        mail.validate()?;
        self.mailables.lock().unwrap().push(RecordedMail {
            to: mail.to.clone(),
            cc: mail.cc.clone(),
            bcc: mail.bcc.clone(),
            subject: mail.subject.clone(),
            text: mail.text.clone(),
            html: mail.html.clone(),
            attachment_count: attachments.len(),
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
    /// From address, e.g. `Kelson <no-reply@example.com>`.
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

    /// Build the RFC822 message for a `Mailable` with already-resolved
    /// (size-capped) attachments — testable without a network. Text and html
    /// become an `alternative` part, attachments a `mixed` wrapper with
    /// `Content-Disposition: attachment`, custom headers are inserted raw
    /// (lettre RFC2047-encodes + folds the value, no header injection), and
    /// the `Bcc` header is dropped from the rendered message (envelope only).
    pub fn build_mailable(
        config: &SmtpConfig,
        mail: &Mailable,
        attachments: &[ResolvedMailAttachment],
    ) -> Result<Message, AppError> {
        mail.validate()?;
        let from = mail.from.as_deref().unwrap_or(config.from.as_str());
        if from.is_empty() {
            return Err(AppError::validation(
                "mail: no sender address (set the mailer's from)",
            ));
        }
        let mailbox = |addr: &str| {
            addr.parse()
                .map_err(|e| AppError::validation(format!("mail: bad address {addr:?}: {e}")))
        };
        let mut builder = Message::builder()
            .from(mailbox(from)?)
            .subject(mail.subject.clone());
        if let Some(reply) = mail.reply_to.as_deref() {
            builder = builder.reply_to(mailbox(reply)?);
        }
        for addr in &mail.to {
            builder = builder.to(mailbox(addr)?);
        }
        for addr in &mail.cc {
            builder = builder.cc(mailbox(addr)?);
        }
        for addr in &mail.bcc {
            builder = builder.bcc(mailbox(addr)?);
        }

        // Body: one part when a single side exists, alternative when both.
        enum Body {
            Single(SinglePart),
            Multi(MultiPart),
        }
        let body = match (&mail.text, &mail.html) {
            (Some(text), None) => Body::Single(SinglePart::plain(text.clone())),
            (None, Some(html)) => Body::Single(SinglePart::html(html.clone())),
            (Some(text), Some(html)) => Body::Multi(
                MultiPart::alternative()
                    .singlepart(SinglePart::plain(text.clone()))
                    .singlepart(SinglePart::html(html.clone())),
            ),
            // validate() already refused a body-less mail; stay defensive.
            (None, None) => {
                return Err(AppError::validation(
                    "mail: a text or html body is required",
                ))
            }
        };

        let message = if attachments.is_empty() {
            match body {
                Body::Single(part) => builder.singlepart(part),
                Body::Multi(part) => builder.multipart(part),
            }
        } else {
            let mut mixed = match body {
                Body::Single(part) => MultiPart::mixed().singlepart(part),
                Body::Multi(part) => MultiPart::mixed().multipart(part),
            };
            for att in attachments {
                let content_type = ContentType::parse(&att.content_type).map_err(|e| {
                    AppError::validation(format!(
                        "mail: bad attachment content type {:?}: {e}",
                        att.content_type
                    ))
                })?;
                mixed = mixed.singlepart(
                    Attachment::new(att.filename.clone()).body(att.bytes.clone(), content_type),
                );
            }
            builder.multipart(mixed)
        }
        .map_err(|e| AppError::validation(format!("mail: message build: {e}")))?;

        let mut message = message;
        for (name, value) in &mail.headers {
            let header_name = HeaderName::new_from_ascii(name.clone()).map_err(|e| {
                AppError::validation(format!("mail: bad header name {name:?}: {e}"))
            })?;
            message
                .headers_mut()
                .insert_raw(HeaderValue::new(header_name, value.clone()));
        }
        Ok(message)
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

    fn send_mailable(
        &self,
        mail: &Mailable,
        attachments: &[ResolvedMailAttachment],
    ) -> Result<(), AppError> {
        let message = Self::build_mailable(&self.config, mail, attachments)?;
        self.transport
            .send(&message)
            .map(|_| ())
            .map_err(|e| AppError::external("smtp", e.to_string()))
    }
}

// ------------------------------------------------------------------ chain

/// Transport of one chain mailer. Kept neutral here: the config's
/// `MailerTransport` converts in kelson-worker because the dependency matrix
/// forbids adapters -> application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// Record the mail instead of sending (dev/test, degraded default).
    Log,
    /// Real SMTP delivery via lettre.
    Smtp,
}

/// One `[mail.mailers.<name>]` entry, converted at boot (kelson-worker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailerSpec {
    pub name: String,
    pub transport: TransportKind,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
    /// Health-probe timeout used by failover selection.
    pub timeout_ms: u64,
}

impl MailerSpec {
    pub fn smtp_config(&self) -> SmtpConfig {
        SmtpConfig {
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            password: self.password.clone(),
            from: self.from.clone(),
        }
    }

    /// Health = TCP connect probe bounded by the mailer's timeout; `log` is
    /// an in-process recorder and never probes. Name resolution uses the OS
    /// resolver (same as the SMTP client itself), so the timeout bounds the
    /// connect, not DNS.
    pub fn healthy(&self) -> bool {
        match self.transport {
            TransportKind::Log => true,
            TransportKind::Smtp => {
                use std::net::{TcpStream, ToSocketAddrs};
                let timeout = std::time::Duration::from_millis(self.timeout_ms.max(1));
                let Ok(mut addrs) = (self.host.as_str(), self.port).to_socket_addrs() else {
                    return false;
                };
                addrs.any(|addr| TcpStream::connect_timeout(&addr, timeout).is_ok())
            }
        }
    }
}

fn default_probe_timeout_ms() -> u64 {
    2000
}

enum BuiltMailer {
    Log(SmtpSandbox),
    // Boxed: SmtpTransport (pooled client config) dwarfs the sandbox and the
    // enum is cloned around by reference anyway.
    Smtp(Box<SmtpLive>),
}

struct ChainEntry {
    spec: MailerSpec,
    mailer: BuiltMailer,
}

/// Ordered failover chain over named mailers (`mail.default`): every send
/// selects the first healthy transport; an all-unhealthy chain is a
/// retryable external error, so the durable job backs off, retries the
/// probe, and dead-letters when the budget runs out. An unknown chain member
/// or an smtp mailer without a host is a boot-refusing config error.
pub struct MailChain {
    chain: Vec<String>,
    entries: BTreeMap<String, ChainEntry>,
}

/// Chain = names + mailer kinds; SMTP transports and sandbox recorders are
/// intentionally not printed (credentials stay out of debug output).
impl std::fmt::Debug for MailChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailChain")
            .field("chain", &self.chain)
            .finish_non_exhaustive()
    }
}

impl MailChain {
    /// `chain` is the comma-separated `mail.default` list; `specs` carries
    /// one entry per declared `[mail.mailers.<name>]`. `log` is built in:
    /// naming it without a spec uses the built-in recorder, an explicit
    /// entry wins, and any other undeclared name is refused.
    pub fn new(chain: &str, specs: Vec<MailerSpec>) -> Result<Self, AppError> {
        let names: Vec<String> = chain
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if names.is_empty() {
            return Err(AppError::validation(
                "mail: mail.default is empty (set it to a comma-separated mailer list)",
            ));
        }
        let mut declared: BTreeMap<String, MailerSpec> = BTreeMap::new();
        for spec in specs {
            declared.insert(spec.name.clone(), spec);
        }

        let mut entries: BTreeMap<String, ChainEntry> = BTreeMap::new();
        for name in &names {
            if entries.contains_key(name) {
                return Err(AppError::validation(format!(
                    "mail: mail.default repeats mailer {name:?}"
                )));
            }
            let spec = match declared.get(name) {
                Some(spec) => spec.clone(),
                None if name == "log" => MailChain::builtin_log(name),
                None => {
                    let mut known: Vec<&str> =
                        declared.keys().map(|s| s.as_str()).collect::<Vec<_>>();
                    known.push("log");
                    known.sort_unstable();
                    return Err(AppError::validation(format!(
                        "mail: unknown mailer {name:?} in mail.default (known: {})",
                        known.join(", ")
                    )));
                }
            };
            if spec.transport == TransportKind::Smtp && spec.host.is_empty() {
                return Err(AppError::validation(format!(
                    "mail: mailer {name:?} is smtp but has no host"
                )));
            }
            let mailer = match spec.transport {
                TransportKind::Log => BuiltMailer::Log(SmtpSandbox::new()),
                TransportKind::Smtp => {
                    BuiltMailer::Smtp(Box::new(SmtpLive::new(spec.smtp_config())?))
                }
            };
            entries.insert(name.clone(), ChainEntry { spec, mailer });
        }
        Ok(Self {
            chain: names,
            entries,
        })
    }

    fn builtin_log(name: &str) -> MailerSpec {
        MailerSpec {
            name: name.to_string(),
            transport: TransportKind::Log,
            host: String::new(),
            port: 0,
            username: String::new(),
            password: String::new(),
            from: String::new(),
            timeout_ms: default_probe_timeout_ms(),
        }
    }

    /// Chain member names in order (the `mail.default` list).
    pub fn names(&self) -> &[String] {
        &self.chain
    }

    /// First healthy chain member (probe order = chain order). Err means
    /// every member is unhealthy — a retryable external error.
    pub fn select(&self) -> Result<&str, AppError> {
        let mut unhealthy = Vec::new();
        for name in &self.chain {
            if self.entries[name.as_str()].spec.healthy() {
                return Ok(name.as_str());
            }
            unhealthy.push(name.clone());
        }
        // The detail IS the public message (same posture as the worker's
        // deadline error): `dispatch_once` records `public_message` into the
        // job's `last_error`, so `queue:failed` must say which mailers were
        // unhealthy. Category External + Retryable keeps the retry policy —
        // the probe may heal between attempts before the budget dead-letters.
        Err(AppError::new(
            codes::EXTERNAL,
            ErrorCategory::External,
            Severity::Medium,
            Retryability::Retryable,
            format!("all chain mailers unhealthy: {}", unhealthy.join(", ")),
        ))
    }

    /// Send through the first healthy chain member.
    pub fn send_mailable(
        &self,
        mail: &Mailable,
        attachments: &[ResolvedMailAttachment],
    ) -> Result<(), AppError> {
        let name = self.select()?;
        match &self.entries[name].mailer {
            BuiltMailer::Log(sandbox) => sandbox.send_mailable(mail, attachments),
            BuiltMailer::Smtp(live) => live.send_mailable(mail, attachments),
        }
    }

    /// Every `Mailable` recorded by `log` transport members, in chain order
    /// (dev/test assertion surface for queued sends).
    pub fn recorded(&self) -> Vec<RecordedMail> {
        self.chain
            .iter()
            .filter_map(|name| match &self.entries[name.as_str()].mailer {
                BuiltMailer::Log(sandbox) => Some(sandbox.mailables.lock().unwrap().clone()),
                BuiltMailer::Smtp(_) => None,
            })
            .flatten()
            .collect()
    }
}

// --------------------------------------------------------------- job glue

/// Job name of the queued send: kelson-worker registers the [`JobDef`] and
/// [`MailSendHandler`]; producers enqueue through [`enqueue_mailable`].
pub const MAIL_SEND_JOB: &str = "mail.send";

/// Build the durable `mail.send` job for a Mailable and enqueue it. The
/// payload is the serialized Mailable — object references only; attachment
/// bytes resolve at run time through the storage port under the upload cap.
pub fn enqueue_mailable(
    store: &dyn JobStore,
    registry: &JobRegistry,
    mail: &Mailable,
    now: OffsetDateTime,
) -> Result<Uuid, AppError> {
    mail.validate()?;
    let payload = serde_json::to_value(mail)
        .map_err(|e| AppError::validation(format!("mail.send payload: {e}")))?;
    let job = registry.build(MAIL_SEND_JOB, payload, now)?;
    store.enqueue(job)
}

/// `mail.send` handler (docs/19 Wave 6): deserialize → resolve attachments
/// (size-capped) → select the first healthy chain transport → send. Any Err
/// maps to `JobStore::fail` (retry with backoff, then dead-letter).
pub struct MailSendHandler {
    chain: MailChain,
    storage: Option<Arc<dyn StoragePort>>,
    max_attachment_bytes: u64,
}

impl MailSendHandler {
    /// `storage` arrives with the disk registry (Wave 6 P2); until then a
    /// mail that carries attachments fails explicitly instead of dropping
    /// them silently.
    pub fn new(chain: MailChain, storage: Option<Arc<dyn StoragePort>>) -> Self {
        Self {
            chain,
            storage,
            max_attachment_bytes: kelson_notifications::DEFAULT_MAX_ATTACHMENT_BYTES,
        }
    }

    /// Synchronous core — the `JobHandler` wrapper is a thin `Box::pin`
    /// around it (the blocking SMTP posture, D-18).
    pub fn run(&self, payload: &serde_json::Value) -> Result<(), AppError> {
        let mail: Mailable = serde_json::from_value(payload.clone())
            .map_err(|e| AppError::validation(format!("mail.send payload: {e}")))?;
        mail.validate()?;
        let attachments = if mail.attachments.is_empty() {
            Vec::new()
        } else {
            let storage = self.storage.as_deref().ok_or_else(|| {
                // Public message = the reason (it lands verbatim in
                // `last_error`); retryable so a storage-wired restart can
                // still drain the attempt before the budget dead-letters it.
                AppError::new(
                    codes::INTERNAL,
                    ErrorCategory::Internal,
                    Severity::High,
                    Retryability::Retryable,
                    "mail.send: attachments need a storage port (disk wiring lands Wave 6 P2)",
                )
            })?;
            mail.resolve_attachments(storage, self.max_attachment_bytes)?
        };
        self.chain.send_mailable(&mail, &attachments)
    }
}

impl JobHandler for MailSendHandler {
    fn name(&self) -> &str {
        MAIL_SEND_JOB
    }

    fn handle<'a>(&'a self, job: &'a DurableJob) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move { self.run(&job.payload) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_jobs::{InMemoryJobStore, JobDef, JobStatus, JobStoreAdmin};
    use kelson_storage::ObjectRef;
    use std::net::TcpListener;

    fn config() -> SmtpConfig {
        SmtpConfig {
            host: "smtp.example.com".into(),
            port: 587,
            username: "no-reply@example.com".into(),
            password: "secret".into(),
            from: "Kelson <no-reply@example.com>".into(),
        }
    }

    fn msg(to: &str, template: &str) -> NotifyMessage {
        NotifyMessage {
            channel: kelson_notifications::Channel::Email,
            to: to.into(),
            template: template.into(),
            vars: std::collections::BTreeMap::new(),
            tenant_id: None,
            locale: None,
        }
    }

    fn mailable() -> Mailable {
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

    fn smtp_spec(name: &str, host: &str, port: u16) -> MailerSpec {
        MailerSpec {
            name: name.into(),
            transport: TransportKind::Smtp,
            host: host.into(),
            port,
            username: String::new(),
            password: String::new(),
            from: "Kelson <no-reply@example.com>".into(),
            timeout_ms: 250,
        }
    }

    /// Port 1 on loopback is privileged (never bound by tests) → connection
    /// refused, quickly and deterministically.
    const DEAD_PORT: u16 = 1;

    #[test]
    fn sandbox_stores_rendered_mail() {
        let s = SmtpSandbox::new();
        s.send(&msg("ops@example.com", "welcome"), "Welcome")
            .unwrap();
        assert_eq!(s.outbox.lock().unwrap().len(), 1);
    }

    #[test]
    fn live_builds_rfc822_message_with_var_substitution() {
        let mut vars = std::collections::BTreeMap::new();
        vars.insert("name".into(), "Asha".into());
        let mut m = msg("a@b.c", "Hello {name}, your OTP is ready.");
        m.vars = vars;
        let message = SmtpLive::build_message(&config(), &m, "OTP").unwrap();
        let rendered = String::from_utf8_lossy(&message.formatted()).to_string();
        assert!(rendered.contains("Subject: OTP"));
        assert!(rendered.contains("Hello Asha, your OTP is ready."));
        assert!(rendered.contains("To: a@b.c"));
    }

    #[test]
    fn live_rejects_bad_addresses_before_network() {
        let err =
            SmtpLive::build_message(&config(), &msg("not-an-address", "body"), "s").unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);
        let live = SmtpLive::new(config()).unwrap();
        assert!(live.send(&msg("", "body"), "s").is_err());
    }

    /// Chain selection: the dead SMTP member is skipped, the log member
    /// wins, and the send lands in its recorder.
    #[test]
    fn chain_selects_first_healthy_and_falls_back_to_log() {
        let chain =
            MailChain::new("dead,log", vec![smtp_spec("dead", "127.0.0.1", DEAD_PORT)]).unwrap();
        assert_eq!(chain.names().join(","), "dead,log");
        assert_eq!(chain.select().unwrap(), "log");

        let mut mail = mailable();
        mail.cc.push("audit@example.com".into());
        chain.send_mailable(&mail, &[]).unwrap();
        let recorded = chain.recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].subject, "Invoice");
        assert_eq!(recorded[0].cc, vec!["audit@example.com"]);
        assert_eq!(recorded[0].attachment_count, 0);
    }

    /// A listening SMTP endpoint passes the probe and is selected first.
    #[test]
    fn chain_selects_a_healthy_smtp_member() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let chain = MailChain::new("live,log", vec![smtp_spec("live", "127.0.0.1", port)]).unwrap();
        assert_eq!(chain.select().unwrap(), "live");
        // log-only chain: the in-process recorder is always healthy.
        let log_only = MailChain::new("log", Vec::new()).unwrap();
        assert_eq!(log_only.select().unwrap(), "log");
    }

    /// Config errors refuse boot: unknown member, empty chain, duplicate
    /// member, smtp without host. The built-in log needs no declaration.
    #[test]
    fn chain_rejects_bad_configuration() {
        let err = MailChain::new("primry", Vec::new()).unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);
        assert!(
            err.public_message.contains("unknown mailer"),
            "{}",
            err.public_message
        );

        assert!(MailChain::new(" , ", Vec::new()).is_err(), "empty chain");
        assert!(MailChain::new("log,log", Vec::new()).is_err(), "duplicate");

        let mut spec = smtp_spec("relay", "", 587);
        spec.host.clear();
        let err = MailChain::new("relay", vec![spec]).unwrap_err();
        assert!(
            err.public_message.contains("no host"),
            "{}",
            err.public_message
        );

        // Built-in `log` resolves with zero declared mailers, and an
        // explicit entry under the same name wins.
        assert!(MailChain::new("log", Vec::new()).is_ok());
        let mut explicit = MailerSpec {
            name: "log".into(),
            transport: TransportKind::Log,
            host: String::new(),
            port: 0,
            username: String::new(),
            password: String::new(),
            from: "Kelson <no-reply@example.com>".into(),
            timeout_ms: 2000,
        };
        explicit.from = "custom@example.com".into();
        let chain = MailChain::new("log", vec![explicit.clone()]).unwrap();
        assert_eq!(chain.select().unwrap(), "log");
    }

    /// Every member down → retryable external error (the job policy then
    /// owns retry → backoff → dead-letter).
    #[test]
    fn all_unhealthy_chain_is_an_external_error() {
        let chain =
            MailChain::new("dead", vec![smtp_spec("dead", "127.0.0.1", DEAD_PORT)]).unwrap();
        let err = chain.send_mailable(&mailable(), &[]).unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::EXTERNAL);
        assert!(
            err.public_message.contains("unhealthy"),
            "{}",
            err.public_message
        );
    }

    /// The log/sandbox transport records the full recipient set and the
    /// attachment count, and refuses malformed mail before recording.
    #[test]
    fn sandbox_records_mailables_and_refuses_bad_recipients() {
        let sandbox = SmtpSandbox::new();
        let mut mail = mailable();
        mail.cc.push("audit@example.com".into());
        mail.bcc.push("vault@example.com".into());
        let atts = vec![ResolvedMailAttachment {
            filename: "invoice.pdf".into(),
            content_type: "application/pdf".into(),
            bytes: b"%PDF-1.4".to_vec(),
        }];
        sandbox.send_mailable(&mail, &atts).unwrap();

        let recorded = sandbox.mailables.lock().unwrap();
        assert_eq!(recorded[0].to, vec!["ops@example.com"]);
        assert_eq!(recorded[0].cc, vec!["audit@example.com"]);
        assert_eq!(recorded[0].bcc, vec!["vault@example.com"]);
        assert_eq!(recorded[0].attachment_count, 1);
        drop(recorded);

        let mut bad = mailable();
        bad.to = vec!["not-an-address".into()];
        assert!(sandbox.send_mailable(&bad, &[]).is_err());
        assert_eq!(
            sandbox.mailables.lock().unwrap().len(),
            1,
            "nothing recorded"
        );
    }

    /// Full message build: alternative body, cc/reply-to, custom header,
    /// attachment disposition — and Bcc stays out of the rendered headers.
    #[test]
    fn live_builds_multipart_message_with_attachments_and_headers() {
        let mut mail = mailable();
        mail.html = Some("<h1>Invoice</h1>".into());
        mail.cc.push("audit@example.com".into());
        mail.bcc.push("vault@example.com".into());
        mail.reply_to = Some("support@example.com".into());
        mail.headers.push(("X-Kelson-Tenant".into(), "t-1".into()));
        let atts = vec![ResolvedMailAttachment {
            filename: "invoice.pdf".into(),
            content_type: "application/pdf".into(),
            bytes: b"%PDF-1.4".to_vec(),
        }];

        let message = SmtpLive::build_mailable(&config(), &mail, &atts).unwrap();
        let rendered = String::from_utf8_lossy(&message.formatted()).to_string();
        assert!(rendered.contains("Subject: Invoice"));
        assert!(rendered.contains("Cc: audit@example.com"));
        assert!(rendered.contains("Reply-To: support@example.com"));
        assert!(rendered.contains("X-Kelson-Tenant: t-1"));
        assert!(rendered.contains("multipart/mixed"));
        assert!(rendered.contains("multipart/alternative"));
        assert!(rendered.contains("Content-Disposition: attachment"));
        assert!(rendered.contains("filename=\"invoice.pdf\""));
        assert!(
            !rendered.contains("vault@example.com"),
            "Bcc never renders (envelope only)"
        );
        assert!(rendered.contains("Your invoice is ready."));
        assert!(rendered.contains("<h1>Invoice</h1>"));
    }

    /// Text-only mail renders a single part; a mail without any body (only
    /// reachable by bypassing validate) and a bad attachment content type
    /// are refused.
    #[test]
    fn live_build_refuses_bad_attachment_content_type() {
        let message = SmtpLive::build_mailable(&config(), &mailable(), &[]).unwrap();
        let rendered = String::from_utf8_lossy(&message.formatted()).to_string();
        assert!(
            !rendered.contains("multipart"),
            "single part, no attachments"
        );

        let atts = vec![ResolvedMailAttachment {
            filename: "x.bin".into(),
            content_type: "not a mime type".into(),
            bytes: vec![1],
        }];
        let err = SmtpLive::build_mailable(&config(), &mailable(), &atts).unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);
    }

    /// In-memory storage for the handler's attachment resolution.
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

    fn mail_with_attachment() -> Mailable {
        let mut mail = mailable();
        mail.attachments.push(kelson_notifications::MailAttachment {
            filename: "invoice.pdf".into(),
            content_type: "application/pdf".into(),
            object: kelson_storage::ObjectRef {
                bucket: "m".into(),
                key: "invoice.pdf".into(),
            },
        });
        mail
    }

    /// The handler resolves attachments under the cap (over-cap fails closed
    /// with CAP_LIMIT_EXCEEDED) and records the send on the log transport.
    #[test]
    fn handler_resolves_capped_attachments_and_sends() {
        let chain = MailChain::new("log", Vec::new()).unwrap();

        // Storage present: attachment resolves, is counted, and the send
        // lands on the log transport.
        let handler = MailSendHandler::new(
            MailChain::new("log", Vec::new()).unwrap(),
            Some(Arc::new(BlobStorage(vec![7u8; 1024]))),
        );
        let payload = serde_json::to_value(mail_with_attachment()).unwrap();
        handler.run(&payload).unwrap();
        let recorded = handler.chain.recorded();
        assert_eq!(recorded[0].attachment_count, 1);

        // Attachments without a storage port: explicit wiring error (P2).
        let bare = MailSendHandler::new(chain, None);
        let err = bare.run(&payload).unwrap_err();
        assert!(
            err.public_message.contains("storage port"),
            "{}",
            err.public_message
        );
    }

    #[test]
    fn handler_refuses_oversized_attachment_and_bad_payload() {
        // 64-byte cap exceeded by a 1024-byte object → CAP_LIMIT_EXCEEDED.
        let mut handler = MailSendHandler::new(
            MailChain::new("log", Vec::new()).unwrap(),
            Some(Arc::new(BlobStorage(vec![7u8; 1024]))),
        );
        handler.max_attachment_bytes = 64;
        let payload = serde_json::to_value(mail_with_attachment()).unwrap();
        let err = handler.run(&payload).unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::CAP_LIMIT);

        // Payload that is not a Mailable → validation error (no retry storm).
        let bad = serde_json::json!({"nope": 1});
        let err = handler.run(&bad).unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);

        // Well-formed payload without attachments works with no storage.
        let bare = MailSendHandler::new(MailChain::new("log", Vec::new()).unwrap(), None);
        let payload = serde_json::to_value(mailable()).unwrap();
        assert!(bare.run(&payload).is_ok());
    }

    /// End-to-end queue semantics with the real handler: the first attempt
    /// fails → the store records the error and schedules a backoff retry;
    /// with the retry budget burned the same failure dead-letters the job.
    #[test]
    fn queued_send_retries_then_dead_letters() {
        // A chain that can never become healthy: nothing listens on port 1.
        let chain =
            MailChain::new("dead", vec![smtp_spec("dead", "127.0.0.1", DEAD_PORT)]).unwrap();
        let handler = MailSendHandler::new(chain, None);

        let mut registry = JobRegistry::new();
        registry
            .register(JobDef::new(MAIL_SEND_JOB))
            .expect("job def registers once");
        let store = InMemoryJobStore::new();

        let mail = mailable();
        let now = OffsetDateTime::now_utc();

        // Attempt 1: claim → handler fails (external, all unhealthy) → the
        // store schedules a backoff retry and keeps the error.
        let id = enqueue_mailable(&store, &registry, &mail, now).expect("enqueues");
        let job = store.claim("w1").unwrap().expect("claimable");
        assert_eq!(job.attempt, 1);
        let err = handler.run(&job.payload).unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::EXTERNAL);
        let next = store.fail(job.id, &err.public_message).unwrap();
        assert_eq!(next, JobStatus::Pending, "retry scheduled, not dead yet");
        let listed = store.list(Some(JobStatus::Pending), 10).unwrap();
        assert_eq!(listed[0].id, id);
        assert!(listed[0]
            .last_error
            .as_deref()
            .unwrap()
            .contains("unhealthy"));
        assert!(
            listed[0].available_at > now,
            "backoff pushes the next claim into the future"
        );

        // Budget burned (attempt 2 of max 3 already used before claim): the
        // third failure exhausts the budget → dead-letter.
        let mut burnt = registry
            .build(MAIL_SEND_JOB, serde_json::to_value(&mail).unwrap(), now)
            .unwrap();
        burnt.attempt = 2;
        store.enqueue(burnt).unwrap();
        let job = store.claim("w1").unwrap().expect("second job claimable");
        assert_eq!(job.attempt, 3);
        let err = handler.run(&job.payload).unwrap_err();
        let next = store.fail(job.id, &err.public_message).unwrap();
        assert_eq!(next, JobStatus::Dead, "retry budget exhausted");
        let dead = store.list(Some(JobStatus::Dead), 10).unwrap();
        assert_eq!(dead.len(), 1);
        assert!(dead[0].last_error.is_some());
    }

    /// The producer helper validates before enqueueing and requires the
    /// registered def — unknown job name refuses instead of enqueuing a
    /// job nothing will ever claim.
    #[test]
    fn enqueue_requires_registered_def_and_valid_mail() {
        let store = InMemoryJobStore::new();
        let registry = JobRegistry::new(); // deliberately empty
        let now = OffsetDateTime::now_utc();
        let err = enqueue_mailable(&store, &registry, &mailable(), now).unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::NOT_FOUND);

        let mut registry = JobRegistry::new();
        registry.register(JobDef::new(MAIL_SEND_JOB)).unwrap();
        let mut bad = mailable();
        bad.to.clear();
        assert!(enqueue_mailable(&store, &registry, &bad, now).is_err());

        let id = enqueue_mailable(&store, &registry, &mailable(), now).unwrap();
        let job = store.claim("w").unwrap().expect("enqueued job claimable");
        assert_eq!(job.id, id);
        assert_eq!(job.name, MAIL_SEND_JOB);
        assert_eq!(job.queue, "default");
    }
}
