// P7: Chatops command dispatcher — routes /commands to handlers.
//
// The dispatcher maintains a registry of commands and a binding
// store for Telegram chat_id → sovereign user_id mapping.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::types::{InlineKeyboardMarkup, User};
use sovereign_core::ports::StoragePort;

/// Error type for command execution.
#[derive(Debug, thiserror::Error)]
pub enum ChatopsError {
    #[error("not bound: send /start <code> to link your Telegram account")]
    NotBound,

    #[error("unknown command: /{0}")]
    UnknownCommand(String),

    #[error("missing argument: {0}")]
    MissingArgument(String),

    #[error("app not found: {0}")]
    AppNotFound(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("upstream error: {0}")]
    Upstream(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),
}

/// Response from a command handler. Commands can return plain text,
/// text with inline keyboards, or operations that edit existing messages.
#[derive(Debug, Clone)]
pub enum CommandResponse {
    /// Send a new plain-text message.
    Text(String),
    /// Send a new message with an inline keyboard.
    TextWithKeyboard {
        text: String,
        keyboard: InlineKeyboardMarkup,
    },
    /// Edit an existing message (for progress updates).
    EditMessage {
        message_id: i64,
        text: String,
    },
    /// Edit an existing message with a new inline keyboard.
    EditMessageWithKeyboard {
        message_id: i64,
        text: String,
        keyboard: InlineKeyboardMarkup,
    },
    /// Answer a callback query (acknowledge button press).
    AnswerCallback {
        text: String,
        show_alert: bool,
    },
    /// No response needed (command already handled it internally).
    NoReply,
}

impl CommandResponse {
    /// Convenience: plain text response.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// Convenience: text with inline keyboard.
    pub fn with_keyboard(text: impl Into<String>, keyboard: InlineKeyboardMarkup) -> Self {
        Self::TextWithKeyboard {
            text: text.into(),
            keyboard,
        }
    }
}

/// Context passed to every command handler.
pub struct CommandContext {
    /// The sovereign user ID (from binding).
    pub user_id: String,
    /// The Telegram chat ID.
    pub chat_id: i64,
    /// The message ID (for edit operations).
    pub message_id: i64,
    /// The user who sent the message.
    pub from: Option<User>,
    /// The raw message text.
    pub raw_text: String,
    /// Storage port for database queries.
    pub storage: Arc<dyn StoragePort>,
}

/// A chatops command handler.
#[async_trait]
pub trait Command: Send + Sync {
    /// The command name (without `/`).
    fn name(&self) -> &'static str;

    /// Whether this command requires a bound user.
    fn requires_binding(&self) -> bool {
        true
    }

    /// Whether this command mutates state (counts toward rate limit).
    fn is_mutating(&self) -> bool {
        false
    }

    /// Execute the command.
    async fn run(
        &self,
        ctx: &CommandContext,
        args: &[String],
    ) -> Result<CommandResponse, ChatopsError>;
}

/// Registry of available commands.
pub struct CommandRegistry {
    commands: HashMap<String, Arc<dyn Command>>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
        }
    }

    pub fn register(&mut self, cmd: Arc<dyn Command>) {
        self.commands.insert(cmd.name().to_string(), cmd);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Command>> {
        self.commands.get(name).cloned()
    }

    pub fn command_names(&self) -> Vec<&str> {
        self.commands.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Store for Telegram chat_id → sovereign user_id bindings.
#[async_trait]
pub trait BindingStore: Send + Sync {
    /// Look up the bound user_id for a chat_id.
    async fn get_user_id(&self, chat_id: i64) -> Result<Option<String>, ChatopsError>;

    /// Bind a chat_id to a user_id.
    async fn bind(&self, chat_id: i64, user_id: &str) -> Result<(), ChatopsError>;

    /// Unbind a chat_id.
    async fn unbind(&self, chat_id: i64) -> Result<(), ChatopsError>;

    /// List all bindings as (chat_id, user_id) pairs.
    async fn list_bindings(&self) -> Result<Vec<(i64, String)>, ChatopsError>;

    /// Unbind all chats for a given user_id.
    async fn unbind_by_user(&self, user_id: &str) -> Result<u64, ChatopsError>;
}

/// In-memory binding store (for tests and single-instance use).
pub struct InMemoryBindingStore {
    bindings: RwLock<HashMap<i64, String>>,
}

impl InMemoryBindingStore {
    pub fn new() -> Self {
        Self {
            bindings: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryBindingStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BindingStore for InMemoryBindingStore {
    async fn get_user_id(&self, chat_id: i64) -> Result<Option<String>, ChatopsError> {
        Ok(self.bindings.read().await.get(&chat_id).cloned())
    }

    async fn bind(&self, chat_id: i64, user_id: &str) -> Result<(), ChatopsError> {
        self.bindings
            .write()
            .await
            .insert(chat_id, user_id.to_string());
        Ok(())
    }

    async fn unbind(&self, chat_id: i64) -> Result<(), ChatopsError> {
        self.bindings.write().await.remove(&chat_id);
        Ok(())
    }

    async fn list_bindings(&self) -> Result<Vec<(i64, String)>, ChatopsError> {
        let map = self.bindings.read().await;
        Ok(map.iter().map(|(&k, v)| (k, v.clone())).collect())
    }

    async fn unbind_by_user(&self, user_id: &str) -> Result<u64, ChatopsError> {
        let mut map = self.bindings.write().await;
        let before = map.len();
        map.retain(|_, v| v != user_id);
        Ok((before - map.len()) as u64)
    }
}

/// Store for binding codes (6-digit → user_id).
#[async_trait]
pub trait BindingCodeStore: Send + Sync {
    /// Create a new binding code for a user.
    async fn create_code(&self, user_id: &str) -> Result<String, ChatopsError>;

    /// Consume a binding code, returning the user_id if valid and not expired.
    async fn consume_code(&self, code: &str) -> Result<Option<String>, ChatopsError>;
}

/// In-memory binding code store.
pub struct InMemoryBindingCodeStore {
    codes: RwLock<HashMap<String, crate::types::BindingCode>>,
}

impl InMemoryBindingCodeStore {
    pub fn new() -> Self {
        Self {
            codes: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryBindingCodeStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BindingCodeStore for InMemoryBindingCodeStore {
    async fn create_code(&self, user_id: &str) -> Result<String, ChatopsError> {
        let code = crate::types::BindingCode::new(user_id.to_string());
        let code_str = code.code.clone();
        self.codes.write().await.insert(code_str.clone(), code);
        Ok(code_str)
    }

    async fn consume_code(&self, code: &str) -> Result<Option<String>, ChatopsError> {
        let mut codes = self.codes.write().await;
        match codes.remove(code) {
            Some(bc) if !bc.is_expired() => Ok(Some(bc.user_id)),
            Some(_) => Ok(None), // expired
            None => Ok(None),    // not found
        }
    }
}

// ---------------------------------------------------------------------------
// Per-chat rate limiter (mutable commands: 10/hour/chat)
// ---------------------------------------------------------------------------

/// Rate limiter for mutable chatops commands. Tracks command counts
/// per chat_id in a sliding 1-hour window.
pub struct ChatRateLimiter {
    inner: RwLock<HashMap<i64, Vec<std::time::Instant>>>,
    max_per_hour: u32,
}

impl ChatRateLimiter {
    pub fn new(max_per_hour: u32) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            max_per_hour,
        }
    }

    /// Check if a chat is within its rate limit. Returns `true` if allowed.
    pub async fn check(&self, chat_id: i64) -> bool {
        if self.max_per_hour == 0 {
            return true; // 0 = unlimited
        }
        let mut map = self.inner.write().await;
        let now = std::time::Instant::now();
        let window = std::time::Duration::from_secs(3600);
        let timestamps = map.entry(chat_id).or_default();
        // Purge entries older than 1 hour
        timestamps.retain(|t| now.duration_since(*t) < window);
        if timestamps.len() >= self.max_per_hour as usize {
            return false;
        }
        timestamps.push(now);
        true
    }

    /// How many mutable commands remain in the current window for a chat.
    pub async fn remaining(&self, chat_id: i64) -> u32 {
        if self.max_per_hour == 0 {
            return u32::MAX;
        }
        let map = self.inner.read().await;
        let now = std::time::Instant::now();
        let window = std::time::Duration::from_secs(3600);
        let count = map
            .get(&chat_id)
            .map(|ts| ts.iter().filter(|t| now.duration_since(**t) < window).count() as u32)
            .unwrap_or(0);
        self.max_per_hour.saturating_sub(count)
    }
}

impl Default for ChatRateLimiter {
    fn default() -> Self {
        Self::new(10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_binding_store_roundtrip() {
        let store = InMemoryBindingStore::new();
        assert_eq!(store.get_user_id(123).await.unwrap(), None);
        store.bind(123, "user1").await.unwrap();
        assert_eq!(store.get_user_id(123).await.unwrap(), Some("user1".to_string()));
        store.unbind(123).await.unwrap();
        assert_eq!(store.get_user_id(123).await.unwrap(), None);
    }

    #[tokio::test]
    async fn in_memory_list_bindings() {
        let store = InMemoryBindingStore::new();
        store.bind(10, "alice").await.unwrap();
        store.bind(20, "bob").await.unwrap();
        let mut bindings = store.list_bindings().await.unwrap();
        bindings.sort_by_key(|b| b.0);
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0], (10, "alice".to_string()));
        assert_eq!(bindings[1], (20, "bob".to_string()));
    }

    #[tokio::test]
    async fn in_memory_unbind_by_user() {
        let store = InMemoryBindingStore::new();
        store.bind(10, "alice").await.unwrap();
        store.bind(20, "alice").await.unwrap();
        store.bind(30, "bob").await.unwrap();
        let revoked = store.unbind_by_user("alice").await.unwrap();
        assert_eq!(revoked, 2);
        assert_eq!(store.get_user_id(10).await.unwrap(), None);
        assert_eq!(store.get_user_id(20).await.unwrap(), None);
        assert_eq!(store.get_user_id(30).await.unwrap(), Some("bob".to_string()));
    }

    #[tokio::test]
    async fn binding_code_store_roundtrip() {
        let store = InMemoryBindingCodeStore::new();
        let code = store.create_code("user1").await.unwrap();
        assert_eq!(code.len(), 6);
        let user = store.consume_code(&code).await.unwrap();
        assert_eq!(user, Some("user1".to_string()));
        // Code is consumed, second use returns None
        let user2 = store.consume_code(&code).await.unwrap();
        assert_eq!(user2, None);
    }

    #[test]
    fn command_registry_register_and_get() {
        struct DummyCmd;
        #[async_trait]
        impl Command for DummyCmd {
            fn name(&self) -> &'static str { "test" }
            async fn run(&self, _ctx: &CommandContext, _args: &[String]) -> Result<CommandResponse, ChatopsError> {
                Ok(CommandResponse::text("ok"))
            }
        }

        let mut reg = CommandRegistry::new();
        reg.register(Arc::new(DummyCmd));
        assert!(reg.get("test").is_some());
        assert!(reg.get("missing").is_none());
        assert!(reg.command_names().contains(&"test"));
    }

    #[test]
    fn command_response_text() {
        let r = CommandResponse::text("hello");
        match r {
            CommandResponse::Text(t) => assert_eq!(t, "hello"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn command_response_with_keyboard() {
        use crate::types::InlineKeyboardButton;
        let kb = InlineKeyboardMarkup::row(vec![
            InlineKeyboardButton::callback("Yes", "y"),
        ]);
        let r = CommandResponse::with_keyboard("Confirm?", kb);
        match r {
            CommandResponse::TextWithKeyboard { text, keyboard } => {
                assert_eq!(text, "Confirm?");
                assert_eq!(keyboard.inline_keyboard.len(), 1);
            }
            _ => panic!("expected TextWithKeyboard"),
        }
    }
}
