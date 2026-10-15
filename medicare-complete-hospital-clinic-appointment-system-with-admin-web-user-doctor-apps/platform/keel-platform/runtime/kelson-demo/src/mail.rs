//! Row 19 (auth mail): the edge's `[mail]` failover chain behind the
//! notifications `MailPort` trait — the same layer-matrix seam kelson-worker
//! owns (adapters speak `MailChain`, the notifications layer speaks
//! `MailPort`, and the runtime owns both sides), so both runtimes deliver
//! through one transport stack.

use kelson_foundation::AppError;
use kelson_notifications::{MailPort, Mailable, NotifyMessage, ResolvedMailAttachment};

/// Build the edge mail port from the frozen `[mail]` section. An unknown
/// chain member, a duplicate name, or an smtp mailer without a host refuses
/// boot (`MailChain::new` owns those checks) — a config typo never degrades
/// into a silently-dropped mailer mid-chain. The default chain is `log`
/// (`MailSection::default`), so a boot with no `[mail]` section still
/// records token mail instead of losing it; `transport = "smtp"` turns the
/// same chain live.
pub(crate) fn port_from(
    section: &kelson_config::MailSection,
) -> Result<Option<std::sync::Arc<dyn MailPort>>, AppError> {
    use kelson_mail_smtp::{MailerSpec, TransportKind};
    let specs = section
        .mailers
        .iter()
        .map(|(name, mailer)| MailerSpec {
            name: name.clone(),
            transport: match mailer.transport {
                kelson_config::MailerTransport::Log => TransportKind::Log,
                kelson_config::MailerTransport::Smtp => TransportKind::Smtp,
            },
            host: mailer.host.clone(),
            port: mailer.port,
            username: mailer.username.clone(),
            password: mailer.password.clone(),
            from: mailer.from.clone(),
            timeout_ms: mailer.timeout_ms,
        })
        .collect();
    let chain = kelson_mail_smtp::MailChain::new(&section.default, specs)?;
    Ok(Some(std::sync::Arc::new(ChainMail(chain))))
}

/// The bridge (mirrors kelson-worker's `ChainMail`): the rich path delegates
/// untouched (the mailable validator runs before the transport) and the
/// simple sync path becomes a text-only mailable.
struct ChainMail(kelson_mail_smtp::MailChain);

impl MailPort for ChainMail {
    fn send(&self, msg: &NotifyMessage, subject: &str) -> Result<(), AppError> {
        let mail = Mailable {
            to: vec![msg.to.clone()],
            cc: vec![],
            bcc: vec![],
            from: None,
            reply_to: None,
            subject: subject.to_string(),
            text: Some(msg.template.clone()),
            html: None,
            headers: vec![],
            attachments: vec![],
        };
        mail.validate()?;
        self.0.send_mailable(&mail, &[])
    }

    fn send_mailable(
        &self,
        mail: &Mailable,
        attachments: &[ResolvedMailAttachment],
    ) -> Result<(), AppError> {
        self.0.send_mailable(mail, attachments)
    }
}

#[cfg(test)]
mod mail_chain_tests {
    use super::*;

    /// The default chain (no `[mail]` section at all) is the built-in `log`
    /// mailer: a fresh boot records token mail instead of dropping it.
    #[test]
    fn default_section_builds_the_builtin_log_chain() {
        let port = port_from(&kelson_config::MailSection::default())
            .expect("default [mail] builds")
            .expect("a port is wired");
        let sent = port.send(
            &NotifyMessage {
                channel: kelson_notifications::Channel::Email,
                to: "patient@clinic.test".into(),
                template: "reset token: abc123".into(),
                vars: Default::default(),
                tenant_id: None,
                locale: None,
            },
            "Reset your password",
        );
        assert!(sent.is_ok(), "{sent:?}");
    }

    /// A chain member no mailer declares refuses boot — the same fail-closed
    /// posture kelson-worker has: a typo never becomes a silent drop.
    #[test]
    fn an_undeclared_chain_member_refuses_boot() {
        let section = kelson_config::MailSection {
            default: "smtp".into(),
            mailers: Default::default(),
        };
        assert!(port_from(&section).is_err());
    }
}
