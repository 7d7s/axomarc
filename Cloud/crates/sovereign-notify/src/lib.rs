//! Notification adapter. Stubbed in F1; V0.5 will implement the 10 alert
//! channels: email, Slack, Discord, Telegram, webhook, PagerDuty, Opsgenie,
//! Signal, ntfy, and log-only.

#![deny(unsafe_code)]
#![allow(missing_docs)]

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
