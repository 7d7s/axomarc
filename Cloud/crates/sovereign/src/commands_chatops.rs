// P7: Chatops CLI handlers — `sovereign chatops init|start|revoke|bindings`.
//
// These subcommands manage the Telegram bot lifecycle:
//   - `init` generates a 6-digit binding code + deeplink
//   - `start` runs the Telegram long-polling loop
//   - `revoke` unbinds a user's Telegram chat
//   - `bindings` lists active Telegram bindings

use std::sync::Arc;

use crate::cli::ChatopsCmd;
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

pub async fn run(cmd: &ChatopsCmd, out: &Output) -> crate::commands::Dispatch {
    match cmd {
        ChatopsCmd::Init { user } => run_init(user, out),
        ChatopsCmd::Start { token } => run_start(token.as_deref(), out).await,
        ChatopsCmd::Revoke { user } => run_revoke(user, out).await,
        ChatopsCmd::Bindings => run_bindings(out).await,
    }
}

/// `sovereign chatops init --user <email>` — generate a 6-digit code.
fn run_init(user: &str, out: &Output) -> crate::commands::Dispatch {
    let code = sovereign_chatops::types::generate_binding_code();
    // The bot name is read from SOVEREIGN_CHATOPS_BOT (env) first,
    // then falls back to the V0 default sovereign_bot. Operators
    // on Telegram with their own bot can override without rebuilding.
    let bot_name = std::env::var("SOVEREIGN_CHATOPS_BOT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "sovereign_bot".to_string());
    let deeplink = format!("https://t.me/{bot_name}?start={code}");

    match out.format() {
        crate::output::Format::Text => {
            let _ = out.ok(&format!(
                "Binding code: {code}\n\
                 Share this deeplink with the user:\n  {deeplink}\n\
                 The user clicks the link and sends /start {code} to the bot."
            ));
        }
        _ => {
            let env = Envelope::ok(BindingInitData {
                code: &code,
                deeplink: &deeplink,
                user,
            });
            let _ = out.success(&env);
        }
    }
    crate::commands::Dispatch::Ok
}

/// Build SQLite-backed binding stores from the default database.
async fn build_binding_stores() -> Option<(
    Arc<dyn sovereign_chatops::commands::BindingCodeStore>,
    Arc<dyn sovereign_chatops::commands::BindingStore>,
)> {
    let db_path = crate::connect::default_db_path();
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        db_path.to_string_lossy()
    ))
    .await
    .ok()?;
    let store = sovereign_chatops::sqlite_store::SqliteBindingStore::new(pool.clone());
    store.migrate().await.ok()?;
    // Two separate Arc wraps sharing the same pool
    let codes: Arc<dyn sovereign_chatops::commands::BindingCodeStore> =
        Arc::new(sovereign_chatops::sqlite_store::SqliteBindingStore::new(pool.clone()));
    let bindings: Arc<dyn sovereign_chatops::commands::BindingStore> =
        Arc::new(sovereign_chatops::sqlite_store::SqliteBindingStore::new(pool));
    Some((codes, bindings))
}

/// `sovereign chatops start` — run the Telegram poller.
async fn run_start(token: Option<&str>, out: &Output) -> crate::commands::Dispatch {
    let token = match token {
        Some(t) => t.to_string(),
        None => {
            let _ = out.err("SOVEREIGN_TELEGRAM_TOKEN not set. Use --token or set the env var.");
            return crate::commands::Dispatch::Err(AppExit::Usage);
        }
    };

    let _ = out.ok("Starting Telegram poller (Ctrl+C to stop)...");

    let client = Arc::new(sovereign_chatops::telegram::TelegramClient::new(&token));
    let registry = Arc::new(tokio::sync::RwLock::new(
        sovereign_chatops::commands::CommandRegistry::new(),
    ));

    // Try to open SQLite storage; fall back to no-runtime sentinel
    let storage: Arc<dyn sovereign_core::ports::StoragePort> =
        match crate::commands_daemon::build_store().await {
            Some(s) => s,
            None => {
                let _ = out.err("No database found. Run `sovereign init` first.");
                return crate::commands::Dispatch::Err(AppExit::Usage);
            }
        };

    // Use SQLite-backed binding stores when available
    let (binding_codes, bindings) = match build_binding_stores().await {
        Some(stores) => {
            let _ = out.ok("Using SQLite-backed binding stores.");
            stores
        }
        None => {
            let _ = out.ok("SQLite unavailable, using in-memory binding stores.");
            (
                Arc::new(sovereign_chatops::commands::InMemoryBindingCodeStore::new())
                    as Arc<dyn sovereign_chatops::commands::BindingCodeStore>,
                Arc::new(sovereign_chatops::commands::InMemoryBindingStore::new())
                    as Arc<dyn sovereign_chatops::commands::BindingStore>,
            )
        }
    };

    // Build the command registry with built-in commands
    let builtins = sovereign_chatops::builtins::builtins(
        registry.clone(),
        binding_codes,
        bindings.clone(),
        storage.clone(),
        client.clone(),
    );
    *registry.write().await = builtins;

    // Create a broadcast channel for auto-notifications
    let (_tx, rx) = tokio::sync::broadcast::channel::<String>(64);

    // Run the poller (blocks until Ctrl+C)
    sovereign_chatops::poller::run_poller(client.as_ref().clone(), registry, bindings, storage, rx).await;

    crate::commands::Dispatch::Ok
}

/// `sovereign chatops revoke --user <email>` — unbind a user.
async fn run_revoke(user: &str, out: &Output) -> crate::commands::Dispatch {
    match build_binding_stores().await {
        Some((_codes, store)) => {
            let count = match store.unbind_by_user(user).await {
                Ok(c) => c,
                Err(e) => {
                    let _ = out.err(&format!("failed to revoke bindings: {e}"));
                    return crate::commands::Dispatch::Err(AppExit::Generic);
                }
            };
            if count == 0 {
                let _ = out.ok(&format!("No Telegram bindings found for {user}"));
            } else {
                let _ = out.ok(&format!(
                    "Revoked {count} Telegram binding(s) for {user}"
                ));
            }
        }
        None => {
            let _ = out.err("cannot open chatops binding store; run sovereign init first");
            return crate::commands::Dispatch::Err(AppExit::Upstream);
        }
    }
    crate::commands::Dispatch::Ok
}

/// `sovereign chatops bindings` — list active bindings.
async fn run_bindings(out: &Output) -> crate::commands::Dispatch {
    match build_binding_stores().await {
        Some((_codes, store)) => {
            let bindings = match store.list_bindings().await {
                Ok(b) => b,
                Err(e) => {
                    let _ = out.err(&format!("failed to list bindings: {e}"));
                    return crate::commands::Dispatch::Err(AppExit::Generic);
                }
            };
            if bindings.is_empty() {
                let _ = out.ok("No active Telegram bindings.");
                return crate::commands::Dispatch::Ok;
            }
            match out.format() {
                crate::output::Format::Text => {
                    let mut lines = vec!["Active Telegram bindings:".to_string()];
                    for (chat_id, user_id) in &bindings {
                        lines.push(format!("  {chat_id} -> {user_id}"));
                    }
                    let _ = out.ok(&lines.join("\n"));
                }
                _ => {
                    let env = Envelope::ok(BindingListData {
                        bindings: bindings
                            .into_iter()
                            .map(|(chat_id, user)| BindingEntry { user, chat_id })
                            .collect(),
                    });
                    let _ = out.success(&env);
                }
            }
        }
        None => {
            let payload = BindingListData { bindings: vec![] };
            let env = Envelope::err(
                AppExit::Upstream,
                payload,
                "cannot open chatops binding store; run `sovereign init` first",
            );
            let _ = out.error(&env);
            return crate::commands::Dispatch::Err(AppExit::Upstream);
        }
    }
    crate::commands::Dispatch::Ok
}

#[derive(serde::Serialize)]
struct BindingInitData<'a> {
    code: &'a str,
    deeplink: &'a str,
    user: &'a str,
}

#[derive(serde::Serialize)]
struct BindingListData {
    bindings: Vec<BindingEntry>,
}

#[derive(serde::Serialize)]
struct BindingEntry {
    user: String,
    chat_id: i64,
}
