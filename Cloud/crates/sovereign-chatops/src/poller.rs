// P7: Telegram long-polling loop — fetch updates, dispatch commands.
//
// The poller runs forever, fetching updates from Telegram and
// dispatching them to the command registry. It handles:
// - /start <code> binding (unauthenticated)
// - All other commands (require binding)
// - Rate limiting (per-chat, 10 mutable/hour, read-only unlimited)
// - Callback queries (inline keyboard button presses)
// - Typing indicators before long operations
// - RBAC checks (ChatopsExec action)
// - Audit logging for mutating commands
// - Natural language intent parsing (fallback when no /command)
// - Confirmation state machine (yes/no responses to pending actions)

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::commands::{
    BindingStore, ChatRateLimiter, ChatopsError, CommandContext, CommandRegistry,
};
use crate::intent::parse_intent;
use crate::state::{ChatStateManager, PendingAction};
use crate::telegram::{TelegramClient, TelegramError};
use crate::commands::CommandResponse;
use crate::types::{parse_command, CallbackQuery, ChatAction};
use sovereign_core::domain::audit::{AuditEvent, AuditKind};
use sovereign_core::domain::timestamp::Timestamp;
use sovereign_core::ports::StoragePort;

/// Mutable commands rate limit (per chat, per hour).
const MUTABLE_RATE_LIMIT: u32 = 10;

/// Affirmative responses to a confirmation prompt.
const AFFIRMATIVE: &[&str] = &["yes", "y", "confirm", "approve", "ok", "do it", "go", "👍", "✅"];
/// Negative responses to a confirmation prompt.
const NEGATIVE: &[&str] = &["no", "n", "reject", "cancel", "stop", "nope", "nah", "❌"];

/// The main poller loop. Blocks forever.
pub async fn run_poller(
    client: TelegramClient,
    registry: Arc<tokio::sync::RwLock<CommandRegistry>>,
    bindings: Arc<dyn BindingStore>,
    storage: Arc<dyn StoragePort>,
    mut notification_rx: broadcast::Receiver<String>,
) {
    let mut offset: i64 = 0;
    let rate_limiter = Arc::new(ChatRateLimiter::new(MUTABLE_RATE_LIMIT));
    let state_mgr = Arc::new(ChatStateManager::new());

    info!("telegram poller starting");

    loop {
        tokio::select! {
            // Fetch updates from Telegram
            result = client.get_updates(offset, 30) => {
                match result {
                    Ok(updates) => {
                        for update in updates {
                            offset = offset.max(update.update_id + 1);
                            if let Some(msg) = update.message {
                                let client = client.clone();
                                let registry = registry.clone();
                                let bindings = bindings.clone();
                                let rate_limiter = rate_limiter.clone();
                                let storage = storage.clone();
                                let state_mgr = state_mgr.clone();
                                tokio::spawn(async move {
                                    handle_message(client, registry, bindings, rate_limiter, storage, state_mgr, msg).await;
                                });
                            } else if let Some(cq) = update.callback_query {
                                let client = client.clone();
                                let registry = registry.clone();
                                let bindings = bindings.clone();
                                let rate_limiter = rate_limiter.clone();
                                let storage = storage.clone();
                                let state_mgr = state_mgr.clone();
                                tokio::spawn(async move {
                                    handle_callback_query(client, registry, bindings, rate_limiter, storage, state_mgr, cq).await;
                                });
                            }
                        }
                    }
                    Err(TelegramError::RateLimited(secs)) => {
                        warn!("telegram rate limited, sleeping {secs}s");
                        tokio::time::sleep(Duration::from_secs(secs)).await;
                    }
                    Err(e) => {
                        error!("telegram fetch error: {e}");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
            // Receive auto-notifications and forward to every bound chat.
            Ok(text) = notification_rx.recv() => {
                info!("broadcast received, forwarding to bound chats");
                let client = client.clone();
                let bindings = bindings.clone();
                tokio::spawn(async move {
                    match bindings.list_bindings().await {
                        Ok(rows) => {
                            let mut sent = 0usize;
                            let mut failed = 0usize;
                            for (chat_id, _user_id) in rows {
                                match client.send_message(chat_id, &text).await {
                                    Ok(_) => sent += 1,
                                    Err(e) => {
                                        failed += 1;
                                        warn!(chat_id, error = %e, "broadcast send failed");
                                    }
                                }
                            }
                            debug!(sent, failed, "broadcast complete");
                        }
                        Err(e) => {
                            warn!(error = %e, "broadcast failed to list bindings");
                        }
                    }
                });
            }
        }
    }
}

/// Handle a single incoming message.
async fn handle_message(
    client: TelegramClient,
    registry: Arc<tokio::sync::RwLock<CommandRegistry>>,
    bindings: Arc<dyn BindingStore>,
    rate_limiter: Arc<ChatRateLimiter>,
    storage: Arc<dyn StoragePort>,
    state_mgr: Arc<ChatStateManager>,
    msg: crate::types::Message,
) {
    let chat_id = msg.chat.id;
    let message_id = msg.message_id;
    let from = msg.from.clone();
    let text = match msg.text {
        Some(t) => t,
        None => return, // non-text messages are ignored
    };

    // Try /command parsing first
    let parsed_cmd = parse_command(&text);

    // If no /command, check if user is in a confirmation state
    if parsed_cmd.is_none() {
        let text_lower = text.trim().to_lowercase();

        // Check for confirmation response
        if state_mgr.is_pending(chat_id).await {
            if AFFIRMATIVE.contains(&text_lower.as_str()) {
                let action = state_mgr.resolve_confirmation(chat_id).await;
                if let Some(action) = action {
                    execute_confirmed_action(&client, &storage, &state_mgr, chat_id, message_id, action).await;
                    return;
                }
            } else if NEGATIVE.contains(&text_lower.as_str()) {
                state_mgr.resolve_confirmation(chat_id).await;
                let _ = client
                    .edit_message_text(chat_id, message_id, "Action cancelled.")
                    .await;
                return;
            }
            // If neither yes/no, fall through to NL intent parsing
        }

        // Try natural language intent
        if let Some(intent) = parse_intent(&text) {
            let _ = client
                .send_message_plain(
                    chat_id,
                    &format!("I think you want to: /{} {}", intent.command, intent.args.join(" ")),
                )
                .await;

            // Check binding
            let user_id = match bindings.get_user_id(chat_id).await {
                Ok(Some(uid)) => uid,
                Ok(None) => {
                    let _ = client
                        .send_message_plain(
                            chat_id,
                            "Not bound. Send /start <code> to link your Telegram account.",
                        )
                        .await;
                    return;
                }
                Err(e) => {
                    let _ = client
                        .send_message_plain(chat_id, &format!("Error: {e}"))
                        .await;
                    return;
                }
            };

            // Rate limit
            let reg = registry.read().await;
            let is_mutating = reg.get(&intent.command)
                .map(|cmd| cmd.is_mutating())
                .unwrap_or(false);

            if is_mutating && !rate_limiter.check(chat_id).await {
                let _ = client
                    .send_message_plain(chat_id, "Rate limit exceeded. Try again later.")
                    .await;
                return;
            }

            let _ = client.send_chat_action(chat_id, ChatAction::Typing).await;

            let ctx = CommandContext {
                user_id,
                chat_id,
                message_id,
                from,
                raw_text: text,
                storage: storage.clone(),
            };

            if let Some(cmd) = reg.get(&intent.command) {
                match cmd.run(&ctx, &intent.args).await {
                    Ok(response) => {
                        // If response includes a confirmation keyboard, store state
                        if let CommandResponse::TextWithKeyboard { .. } = &response {
                            if let Some(action) = intent_to_pending_action(&intent.command, &intent.args) {
                                state_mgr.set(
                                    chat_id,
                                    crate::state::ChatState::PendingConfirmation {
                                        action,
                                        created_at: std::time::Instant::now(),
                                        prompt_message_id: message_id,
                                    },
                                )
                                .await;
                            }
                        }
                        send_response(&client, chat_id, message_id, response).await;
                    }
                    Err(e) => {
                        let _ = client
                            .send_message_plain(chat_id, &format!("Error: {e}"))
                            .await;
                    }
                }
            }
            return;
        }

        // No command, no confirmation, no intent — show help hint
        let _ = client
            .send_message_plain(chat_id, "Send /help to see available commands.")
            .await;
        return;
    }

    let parsed = parsed_cmd.unwrap();

    // /start is special — it handles binding and doesn't require auth
    if parsed.command == "start" {
        let reg = registry.read().await;
        if let Some(cmd) = reg.get("start") {
            let ctx = CommandContext {
                user_id: String::new(),
                chat_id,
                message_id,
                from,
                raw_text: text,
                storage,
            };
            match cmd.run(&ctx, &parsed.args).await {
                Ok(response) => {
                    send_response(&client, chat_id, message_id, response).await;
                }
                Err(e) => {
                    let _ = client.send_message_plain(chat_id, &format!("Error: {e}")).await;
                }
            }
        }
        return;
    }

    // All other commands require binding
    let user_id = match bindings.get_user_id(chat_id).await {
        Ok(Some(uid)) => uid,
        Ok(None) => {
            let _ = client
                .send_message_plain(
                    chat_id,
                    "Not bound. Send /start <code> to link your Telegram account.\nGet the code from: sovereign chatops init",
                )
                .await;
            return;
        }
        Err(e) => {
            let _ = client
                .send_message_plain(chat_id, &format!("Error checking binding: {e}"))
                .await;
            return;
        }
    };

    // RBAC check: verify user has ChatopsExec permission
    let actor = sovereign_core::rbac::Actor::User {
        id: user_id.clone(),
        role: sovereign_core::domain::user::UserRole::Readonly, // default; could look up real role
        scopes: std::collections::HashSet::new(),
    };
    if let Err(e) = sovereign_core::rbac::check(&actor, sovereign_core::rbac::Action::ChatopsExec) {
        let _ = client
            .send_message_plain(chat_id, &format!("Unauthorized: {e}"))
            .await;
        return;
    }

    // Rate limit mutable commands
    let reg = registry.read().await;
    let is_mutating = reg.get(&parsed.command)
        .map(|cmd| cmd.is_mutating())
        .unwrap_or(false);

    if is_mutating && !rate_limiter.check(chat_id).await {
        let remaining = rate_limiter.remaining(chat_id).await;
        let _ = client
            .send_message_plain(
                chat_id,
                &format!(
                    "Rate limit exceeded. Mutable commands (/deploy, /rollback, /backup) are limited to {MUTABLE_RATE_LIMIT}/hour.\nTry again later. ({remaining} remaining after cooldown)"
                ),
            )
            .await;
        return;
    }

    // Send typing indicator before command execution
    let _ = client.send_chat_action(chat_id, ChatAction::Typing).await;

    let ctx = CommandContext {
        user_id,
        chat_id,
        message_id,
        from,
        raw_text: text,
        storage: storage.clone(),
    };

    match reg.get(&parsed.command) {
        Some(cmd) => match cmd.run(&ctx, &parsed.args).await {
            Ok(response) => {
                // If response includes a confirmation keyboard, store state
                if let CommandResponse::TextWithKeyboard { .. } = &response {
                    if let Some(action) = intent_to_pending_action(&parsed.command, &parsed.args) {
                        state_mgr.set(
                            chat_id,
                            crate::state::ChatState::PendingConfirmation {
                                action,
                                created_at: std::time::Instant::now(),
                                prompt_message_id: message_id,
                            },
                        )
                        .await;
                    }
                }

                // Audit log for mutating commands
                if is_mutating {
                    let audit = AuditEvent {
                        id: None,
                        ts: Timestamp::now(),
                        actor: format!("chatops:{}", ctx.user_id),
                        kind: match parsed.command.as_str() {
                            "deploy" => AuditKind::Deploy,
                            "rollback" => AuditKind::Rollback,
                            "backup" => AuditKind::Backup,
                            "secret" => AuditKind::SecretChange,
                            _ => AuditKind::Webhook,
                        },
                        target: parsed.args.first().map(|a| format!("app:{a}")),
                        payload: serde_json::json!({
                            "action": parsed.command,
                            "chat_id": chat_id,
                            "user_id": ctx.user_id,
                        }),
                        policy_decision: None,
                    };
                    if let Err(e) = storage.append_audit(audit).await {
                        warn!("failed to audit chatops action: {e}");
                    }
                }
                send_response(&client, chat_id, message_id, response).await;
            }
            Err(ChatopsError::UnknownCommand(name)) => {
                let _ = client
                    .send_message_plain(chat_id, &format!("Unknown command: /{name}"))
                    .await;
            }
            Err(e) => {
                let _ = client
                    .send_message_plain(chat_id, &format!("Error: {e}"))
                    .await;
            }
        },
        None => {
            let _ = client
                .send_message_plain(chat_id, &format!("Unknown command: /{}", parsed.command))
                .await;
        }
    }
}

/// Execute a confirmed action after the user approves via inline keyboard or text.
async fn execute_confirmed_action(
    client: &TelegramClient,
    storage: &Arc<dyn StoragePort>,
    _state_mgr: &Arc<ChatStateManager>,
    chat_id: i64,
    message_id: i64,
    action: PendingAction,
) {
    let _ = client.send_chat_action(chat_id, ChatAction::Typing).await;

    let app_name = action.app_name().to_string();

    // Look up the app
    let app = match storage.get_app_by_name(&app_name).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            let _ = client
                .edit_message_text(chat_id, message_id, &format!("App `{app_name}` not found."))
                .await;
            return;
        }
        Err(e) => {
            let _ = client
                .edit_message_text(chat_id, message_id, &format!("Error: {e}"))
                .await;
            return;
        }
    };

    let result = match action {
        PendingAction::Deploy { .. } => {
            let req = sovereign_core::use_cases::deploy::DeployRequest {
                app_id: app.id,
                image_ref: app.image_ref.clone(),
                strategy: sovereign_core::domain::deployment::Strategy::Recreate,
                wait: false,
                actor: "chatops:user".to_string(),
            };
            sovereign_core::use_cases::deploy::start_deploy(
                &sovereign_core::state::AppState {
                    storage: storage.clone(),
                    runtime: sovereign_core::state::no_runtime(),
                    proxy: None,
                    secrets: None,
                    backup: None,
                    db_path: std::path::PathBuf::new(),
                },
                req,
            ).await
        }
        PendingAction::Rollback { .. } => {
            let req = sovereign_core::use_cases::rollback::RollbackRequest {
                app_id: app.id,
                to: None,
                actor: "chatops:user".to_string(),
            };
            sovereign_core::use_cases::rollback::start_rollback(
                &sovereign_core::state::AppState {
                    storage: storage.clone(),
                    runtime: sovereign_core::state::no_runtime(),
                    proxy: None,
                    secrets: None,
                    backup: None,
                    db_path: std::path::PathBuf::new(),
                },
                req,
            ).await.map(|r| sovereign_core::use_cases::deploy::DeployResult {
                deployment: r.deployment,
                url: String::new(),
                health: None,
            })
        }
        PendingAction::Backup { .. } => {
            sovereign_core::use_cases::backup::create_backup(
                &sovereign_core::state::AppState {
                    storage: storage.clone(),
                    runtime: sovereign_core::state::no_runtime(),
                    proxy: None,
                    secrets: None,
                    backup: None,
                    db_path: std::path::PathBuf::new(),
                },
                "chatops:user",
            ).await.map(|b| sovereign_core::use_cases::deploy::DeployResult {
                deployment: sovereign_core::domain::deployment::Deployment {
                    id: sovereign_core::domain::id::DeploymentId::generate(),
                    app_id: app.id,
                    image_ref: format!("backup:{}", b.id),
                    strategy: sovereign_core::domain::deployment::Strategy::Recreate,
                    status: sovereign_core::domain::deployment::DeploymentStatus::Healthy,
                    started_at: b.started_at,
                    finished_at: b.finished_at,
                    triggered_by: "chatops:user".to_string(),
                    risk_score: None,
                    policy_decision: None,
                    error: None,
                    target_deployment_id: None,
                    version: 1,
                },
                url: String::new(),
                health: None,
            })
        }
    };

    match result {
        Ok(_) => {
            let _ = client
                .edit_message_text(
                    chat_id,
                    message_id,
                    &format!("✅ {} completed successfully.", action.describe()),
                )
                .await;
        }
        Err(e) => {
            let _ = client
                .edit_message_text(
                    chat_id,
                    message_id,
                    &format!("❌ {} failed: {e}", action.describe()),
                )
                .await;
        }
    }
}

/// Handle a callback query from an inline keyboard button press.
async fn handle_callback_query(
    client: TelegramClient,
    _registry: Arc<tokio::sync::RwLock<CommandRegistry>>,
    bindings: Arc<dyn BindingStore>,
    _rate_limiter: Arc<ChatRateLimiter>,
    storage: Arc<dyn StoragePort>,
    state_mgr: Arc<ChatStateManager>,
    cq: CallbackQuery,
) {
    let data = match cq.data {
        Some(d) => d,
        None => return,
    };

    let chat_id = cq.message.as_ref().map(|m| m.chat.id).unwrap_or(0);
    let message_id = cq.message.as_ref().map(|m| m.message_id).unwrap_or(0);

    // Parse callback data: "action:verb:app_name"
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    if parts.len() < 3 {
        let _ = client
            .answer_callback_query(&cq.id, Some("Invalid callback data"), false)
            .await;
        return;
    }

    let action = parts[0];
    let verb = parts[1];
    let app_name = parts[2];

    // Check binding
    let _user_id = match bindings.get_user_id(chat_id).await {
        Ok(Some(uid)) => uid,
        _ => {
            let _ = client
                .answer_callback_query(&cq.id, Some("Not bound"), true)
                .await;
            return;
        }
    };

    if verb == "cancel" {
        let _ = client
            .answer_callback_query(&cq.id, Some("Cancelled"), false)
            .await;
        let _ = client
            .edit_message_text(chat_id, message_id, &format!("{action} on `{app_name}` cancelled."))
            .await;
        // Clear any pending state
        state_mgr.resolve_confirmation(chat_id).await;
        return;
    }

    if verb == "confirm" {
        // Acknowledge the button press
        let _ = client
            .answer_callback_query(&cq.id, Some("Processing..."), false)
            .await;

        // Resolve any pending state
        let pending = state_mgr.resolve_confirmation(chat_id).await;

        // Use the pending action or construct from callback data
        let effective_action = pending.unwrap_or_else(|| match action {
            "deploy" => PendingAction::Deploy { app_name: app_name.to_string() },
            "rollback" => PendingAction::Rollback { app_name: app_name.to_string() },
            "backup" => PendingAction::Backup { app_name: app_name.to_string() },
            _ => PendingAction::Deploy { app_name: app_name.to_string() },
        });

        execute_confirmed_action(&client, &storage, &state_mgr, chat_id, message_id, effective_action).await;
    }
}

/// Send a CommandResponse to the chat.
async fn send_response(
    client: &TelegramClient,
    chat_id: i64,
    _message_id: i64,
    response: CommandResponse,
) {
    match response {
        CommandResponse::Text(text) => {
            let _ = client.send_message_plain(chat_id, &text).await;
        }
        CommandResponse::TextWithKeyboard { text, keyboard } => {
            let _ = client.send_message_with_keyboard(chat_id, &text, &keyboard).await;
        }
        CommandResponse::EditMessage { message_id: mid, text } => {
            let _ = client.edit_message_text(chat_id, mid, &text).await;
        }
        CommandResponse::EditMessageWithKeyboard {
            message_id: mid,
            text,
            keyboard,
        } => {
            let _ = client.edit_message_text_with_keyboard(chat_id, mid, &text, &keyboard).await;
        }
        CommandResponse::AnswerCallback { text, show_alert } => {
            let _ = client.send_message_plain(chat_id, &text).await;
            let _ = show_alert;
        }
        CommandResponse::NoReply => {}
    }
}

/// Map a command name + args to a PendingAction for state tracking.
fn intent_to_pending_action(command: &str, args: &[String]) -> Option<PendingAction> {
    let app_name = args.first()?.clone();
    match command {
        "deploy" => Some(PendingAction::Deploy { app_name }),
        "rollback" => Some(PendingAction::Rollback { app_name }),
        "backup" => Some(PendingAction::Backup { app_name }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poller_module_compiles() {
        // smoke test
    }

    #[tokio::test]
    async fn rate_limiter_allows_within_limit() {
        let limiter = ChatRateLimiter::new(3);
        let chat_id = 42;
        assert!(limiter.check(chat_id).await);
        assert!(limiter.check(chat_id).await);
        assert!(limiter.check(chat_id).await);
        assert!(!limiter.check(chat_id).await); // 4th denied
    }

    #[tokio::test]
    async fn rate_limiter_zero_means_unlimited() {
        let limiter = ChatRateLimiter::new(0);
        for _ in 0..100 {
            assert!(limiter.check(1).await);
        }
    }

    #[tokio::test]
    async fn rate_limiter_separate_chats() {
        let limiter = ChatRateLimiter::new(1);
        assert!(limiter.check(1).await);
        assert!(!limiter.check(1).await);
        assert!(limiter.check(2).await);
    }

    #[test]
    fn callback_data_parsing() {
        let data = "deploy:confirm:myapp";
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        assert_eq!(parts, vec!["deploy", "confirm", "myapp"]);
    }

    #[test]
    fn callback_data_cancel() {
        let data = "rollback:cancel:myapp";
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        assert_eq!(parts[0], "rollback");
        assert_eq!(parts[1], "cancel");
    }

    #[test]
    fn affirmative_matches() {
        assert!(AFFIRMATIVE.contains(&"yes"));
        assert!(AFFIRMATIVE.contains(&"y"));
        assert!(AFFIRMATIVE.contains(&"confirm"));
        assert!(AFFIRMATIVE.contains(&"approve"));
        assert!(AFFIRMATIVE.contains(&"ok"));
        assert!(AFFIRMATIVE.contains(&"👍"));
    }

    #[test]
    fn negative_matches() {
        assert!(NEGATIVE.contains(&"no"));
        assert!(NEGATIVE.contains(&"n"));
        assert!(NEGATIVE.contains(&"reject"));
        assert!(NEGATIVE.contains(&"cancel"));
        assert!(NEGATIVE.contains(&"❌"));
    }

    #[test]
    fn intent_to_pending_deploy() {
        let action = intent_to_pending_action("deploy", &["myapp".into()]).unwrap();
        assert_eq!(action, PendingAction::Deploy { app_name: "myapp".into() });
    }

    #[test]
    fn intent_to_pending_rollback() {
        let action = intent_to_pending_action("rollback", &["api".into()]).unwrap();
        assert_eq!(action, PendingAction::Rollback { app_name: "api".into() });
    }

    #[test]
    fn intent_to_pending_backup() {
        let action = intent_to_pending_action("backup", &["web".into()]).unwrap();
        assert_eq!(action, PendingAction::Backup { app_name: "web".into() });
    }

    #[test]
    fn intent_to_pending_status_returns_none() {
        assert!(intent_to_pending_action("status", &[]).is_none());
    }

    #[test]
    fn intent_to_pending_no_args_returns_none() {
        assert!(intent_to_pending_action("deploy", &[]).is_none());
    }
}
