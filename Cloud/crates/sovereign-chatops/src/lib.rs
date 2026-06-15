// # sovereign-chatops
//
// **Telegram chatops adapter for Sovereign.**
//
// Provides a long-polling Telegram bot that dispatches commands to Sovereign
// use cases. Enables operators to manage their fleet from Telegram.
//
// ## Features
//
// - **6-digit code binding** — Secure Telegram chat → Sovereign user mapping
// - **13 commands** — /help, /start, /status, /apps, /doctor, /secret, /deploy, /rollback, /backup, /deployments, /watch, /unwatch, /audit, /notify
// - **Confirmation keyboards** — Mutating commands require approval
// - **Natural language intents** — "deploy myapp" works like /deploy myapp
// - **Per-user rate limiting** — 10 mutable commands per hour per chat
// - **RBAC enforcement** — Respects Sovereign's role-based access control
// - **Audit logging** — All commands logged to audit_event table
// - **SQLite persistence** — Bindings survive restarts
// - **Auto-notifications** — Broadcast channel for deploy events
//
// ## Quick Start
//
// ```bash
// # Generate binding code
// sovereign chatops init --user alice@example.com
//
// # Start the bot
// sovereign chatops start --token 123456:ABC-DEF...
//
// # User binds: /start 482913
// ```
//
// ## Commands
//
// | Command | Description | Mutating |
// |---------|-------------|----------|
// | `/help` | List available commands | No |
// | `/start <code>` | Bind Telegram chat to user | No |
// | `/status` | Fleet status overview | No |
// | `/apps` | List all apps | No |
// | `/doctor` | Run health checks | No |
// | `/secret list <app>` | List secret keys | No |
// | `/deploy <app>` | Deploy app (with confirmation) | Yes |
// | `/rollback <app>` | Rollback app (with confirmation) | Yes |
// | `/backup <app>` | Create backup (with confirmation) | Yes |
// | `/deployments <app>` | Recent deployments | No |
// | `/watch <app>` | Monitor app status | No |
// | `/unwatch` | Stop monitoring | No |
// | `/audit` | Recent audit events | No |
// | `/notify <target> <events>` | Set notification preferences | No |
//
// ## Architecture
//
// ```text
// Telegram API
//     │
//     ▼
// poller::run_poller()
//     │
//     ├─ types::parse_command()
//     ├─ intent::parse_intent() (fallback)
//     ├─ state::ChatStateManager (confirmation flow)
//     ├─ commands::CommandRegistry (dispatch)
//     │   └─ builtins::* (13 commands)
//     ├─ rbac::check() (access control)
//     ├─ storage.append_audit() (logging)
//     └─ telegram::TelegramClient (responses)
// ```

#![deny(unsafe_code)]
#![allow(missing_docs)]

pub mod builtins;
pub mod commands;
pub mod intent;
pub mod poller;
pub mod sqlite_store;
pub mod state;
pub mod telegram;
pub mod types;

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
