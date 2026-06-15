// P7: Per-chat state machine — tracks multi-step operations.
//
// Each chat_id has a state that governs how incoming messages are
// interpreted. For example, if a user is in PendingConfirmation state,
// a "yes"/"no" response is interpreted as approve/reject rather than
// a new command.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

/// Maximum time to wait for a confirmation response.
const CONFIRMATION_TTL: Duration = Duration::from_secs(300); // 5 minutes

/// An action that can be confirmed by the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingAction {
    Deploy { app_name: String },
    Rollback { app_name: String },
    Backup { app_name: String },
}

impl PendingAction {
    pub fn action_name(&self) -> &'static str {
        match self {
            Self::Deploy { .. } => "deploy",
            Self::Rollback { .. } => "rollback",
            Self::Backup { .. } => "backup",
        }
    }

    pub fn app_name(&self) -> &str {
        match self {
            Self::Deploy { app_name }
            | Self::Rollback { app_name }
            | Self::Backup { app_name } => app_name,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Deploy { app_name } => format!("deploy `{app_name}`"),
            Self::Rollback { app_name } => format!("rollback `{app_name}`"),
            Self::Backup { app_name } => format!("backup `{app_name}`"),
        }
    }
}

/// State for a single chat.
#[derive(Debug, Clone)]
pub enum ChatState {
    /// No pending action. Ready for new commands.
    Idle,
    /// Waiting for the user to confirm or cancel an action.
    PendingConfirmation {
        action: PendingAction,
        created_at: Instant,
        /// The message ID of the confirmation prompt (for editing).
        prompt_message_id: i64,
    },
    /// Watching a live status feed (polling).
    Watching {
        app_name: String,
        last_status: String,
        started_at: Instant,
    },
}

impl ChatState {
    /// Whether this state has expired (for TTL-based cleanup).
    pub fn is_expired(&self) -> bool {
        match self {
            Self::PendingConfirmation { created_at, .. } => created_at.elapsed() > CONFIRMATION_TTL,
            Self::Watching { started_at, .. } => started_at.elapsed() > Duration::from_secs(3600),
            Self::Idle => false,
        }
    }

    /// Human-readable description of the current state.
    pub fn describe(&self) -> String {
        match self {
            Self::Idle => "idle".to_string(),
            Self::PendingConfirmation { action, .. } => {
                format!("waiting for confirmation: {}", action.describe())
            }
            Self::Watching { app_name, .. } => format!("watching `{app_name}`"),
        }
    }
}

/// Per-chat state manager. Thread-safe.
pub struct ChatStateManager {
    states: RwLock<HashMap<i64, ChatState>>,
}

impl ChatStateManager {
    pub fn new() -> Self {
        Self {
            states: RwLock::new(HashMap::new()),
        }
    }

    /// Get the current state for a chat.
    pub async fn get(&self, chat_id: i64) -> ChatState {
        let states = self.states.read().await;
        states.get(&chat_id).cloned().unwrap_or(ChatState::Idle)
    }

    /// Set the state for a chat.
    pub async fn set(&self, chat_id: i64, state: ChatState) {
        self.states.write().await.insert(chat_id, state);
    }

    /// Clear the state for a chat (set to Idle).
    pub async fn clear(&self, chat_id: i64) {
        self.states.write().await.remove(&chat_id);
    }

    /// Transition from PendingConfirmation to Idle (after confirm/cancel/expire).
    pub async fn resolve_confirmation(&self, chat_id: i64) -> Option<PendingAction> {
        let mut states = self.states.write().await;
        match states.remove(&chat_id) {
            Some(ChatState::PendingConfirmation { action, .. }) => Some(action),
            _ => None,
        }
    }

    /// Check if a chat is in PendingConfirmation state and not expired.
    pub async fn is_pending(&self, chat_id: i64) -> bool {
        let states = self.states.read().await;
        matches!(
            states.get(&chat_id),
            Some(ChatState::PendingConfirmation { .. })
        )
    }

    /// Get the pending action for a chat, if any and not expired.
    pub async fn get_pending_action(&self, chat_id: i64) -> Option<PendingAction> {
        let states = self.states.read().await;
        match states.get(&chat_id) {
            Some(ChatState::PendingConfirmation { action, created_at, .. })
                if created_at.elapsed() <= CONFIRMATION_TTL =>
            {
                Some(action.clone())
            }
            _ => None,
        }
    }

    /// Set a watch state for a chat.
    pub async fn set_watching(&self, chat_id: i64, app_name: &str, message_id: i64) {
        self.states.write().await.insert(
            chat_id,
            ChatState::Watching {
                app_name: app_name.to_string(),
                last_status: String::new(),
                started_at: Instant::now(),
            },
        );
        let _ = message_id;
    }

    /// Get the watch app name for a chat, if watching.
    pub async fn get_watch_app(&self, chat_id: i64) -> Option<String> {
        let states = self.states.read().await;
        match states.get(&chat_id) {
            Some(ChatState::Watching { app_name, .. }) => Some(app_name.clone()),
            _ => None,
        }
    }

    /// Stop watching for a chat.
    pub async fn stop_watching(&self, chat_id: i64) -> bool {
        let mut states = self.states.write().await;
        matches!(states.remove(&chat_id), Some(ChatState::Watching { .. }))
    }

    /// Clean up expired states. Returns the number of cleaned entries.
    pub async fn cleanup_expired(&self) -> usize {
        let mut states = self.states.write().await;
        let before = states.len();
        states.retain(|_, state| !state.is_expired());
        before - states.len()
    }

    /// Get the prompt message ID for a pending confirmation.
    pub async fn get_prompt_message_id(&self, chat_id: i64) -> Option<i64> {
        let states = self.states.read().await;
        match states.get(&chat_id) {
            Some(ChatState::PendingConfirmation { prompt_message_id, .. }) => {
                Some(*prompt_message_id)
            }
            _ => None,
        }
    }
}

impl Default for ChatStateManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn idle_state_by_default() {
        let mgr = ChatStateManager::new();
        assert!(matches!(mgr.get(1).await, ChatState::Idle));
    }

    #[tokio::test]
    async fn set_and_get_state() {
        let mgr = ChatStateManager::new();
        mgr.set(
            1,
            ChatState::PendingConfirmation {
                action: PendingAction::Deploy {
                    app_name: "myapp".to_string(),
                },
                created_at: Instant::now(),
                prompt_message_id: 42,
            },
        )
        .await;
        assert!(mgr.is_pending(1).await);
        let action = mgr.get_pending_action(1).await.unwrap();
        assert_eq!(action, PendingAction::Deploy { app_name: "myapp".to_string() });
    }

    #[tokio::test]
    async fn resolve_confirmation_returns_action() {
        let mgr = ChatStateManager::new();
        mgr.set(
            1,
            ChatState::PendingConfirmation {
                action: PendingAction::Rollback {
                    app_name: "api".to_string(),
                },
                created_at: Instant::now(),
                prompt_message_id: 10,
            },
        )
        .await;
        let action = mgr.resolve_confirmation(1).await.unwrap();
        assert_eq!(action, PendingAction::Rollback { app_name: "api".to_string() });
        // After resolve, state is cleared
        assert!(!mgr.is_pending(1).await);
    }

    #[tokio::test]
    async fn clear_state() {
        let mgr = ChatStateManager::new();
        mgr.set(
            1,
            ChatState::Watching {
                app_name: "web".to_string(),
                last_status: String::new(),
                started_at: Instant::now(),
            },
        )
        .await;
        assert_eq!(mgr.get_watch_app(1).await, Some("web".to_string()));
        mgr.clear(1).await;
        assert!(matches!(mgr.get(1).await, ChatState::Idle));
    }

    #[tokio::test]
    async fn watch_lifecycle() {
        let mgr = ChatStateManager::new();
        mgr.set_watching(1, "myapp", 100).await;
        assert_eq!(mgr.get_watch_app(1).await, Some("myapp".to_string()));
        assert!(mgr.stop_watching(1).await);
        assert!(mgr.get_watch_app(1).await.is_none());
    }

    #[tokio::test]
    async fn expired_confirmation_is_not_pending() {
        let mgr = ChatStateManager::new();
        mgr.set(
            1,
            ChatState::PendingConfirmation {
                action: PendingAction::Deploy {
                    app_name: "x".to_string(),
                },
                created_at: Instant::now() - Duration::from_secs(600), // expired
                prompt_message_id: 1,
            },
        )
        .await;
        // Expired confirmation should not be returned
        assert!(mgr.get_pending_action(1).await.is_none());
    }

    #[tokio::test]
    async fn cleanup_removes_expired() {
        let mgr = ChatStateManager::new();
        mgr.set(
            1,
            ChatState::PendingConfirmation {
                action: PendingAction::Deploy {
                    app_name: "x".to_string(),
                },
                created_at: Instant::now() - Duration::from_secs(600),
                prompt_message_id: 1,
            },
        )
        .await;
        mgr.set(
            2,
            ChatState::PendingConfirmation {
                action: PendingAction::Backup {
                    app_name: "y".to_string(),
                },
                created_at: Instant::now(), // not expired
                prompt_message_id: 2,
            },
        )
        .await;
        let cleaned = mgr.cleanup_expired().await;
        assert_eq!(cleaned, 1);
        // Chat 2 should still be pending
        assert!(mgr.is_pending(2).await);
    }

    #[test]
    fn pending_action_describe() {
        assert_eq!(
            PendingAction::Deploy { app_name: "a".into() }.describe(),
            "deploy `a`"
        );
        assert_eq!(
            PendingAction::Rollback { app_name: "b".into() }.describe(),
            "rollback `b`"
        );
        assert_eq!(
            PendingAction::Backup { app_name: "c".into() }.describe(),
            "backup `c`"
        );
    }

    #[test]
    fn pending_action_app_name() {
        assert_eq!(PendingAction::Deploy { app_name: "x".into() }.app_name(), "x");
        assert_eq!(PendingAction::Rollback { app_name: "y".into() }.app_name(), "y");
        assert_eq!(PendingAction::Backup { app_name: "z".into() }.app_name(), "z");
    }

    #[test]
    fn state_describe() {
        assert_eq!(ChatState::Idle.describe(), "idle");
        let s = ChatState::Watching {
            app_name: "web".into(),
            last_status: String::new(),
            started_at: Instant::now(),
        };
        assert_eq!(s.describe(), "watching `web`");
    }
}
