// P7: Telegram chatops adapter — types shared across the crate.

use serde::{Deserialize, Serialize};

/// A Telegram update from getUpdates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
    /// Callback query from inline keyboard button presses.
    #[serde(default)]
    pub callback_query: Option<CallbackQuery>,
}

/// A Telegram message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub message_id: i64,
    pub from: Option<User>,
    pub chat: Chat,
    pub text: Option<String>,
}

/// A Telegram user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub first_name: String,
    pub last_name: Option<String>,
    pub username: Option<String>,
}

/// A Telegram chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
}

/// A callback query from an inline keyboard button press.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackQuery {
    /// Unique callback query identifier.
    pub id: String,
    /// The user who pressed the button.
    pub from: User,
    /// The message that contained the inline keyboard.
    pub message: Option<Message>,
    /// The data associated with the button (from `callback_data`).
    pub data: Option<String>,
}

// ---------------------------------------------------------------------------
// Inline keyboard types for Telegram Bot API
// ---------------------------------------------------------------------------

/// An inline keyboard button.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineKeyboardButton {
    /// Button text.
    pub text: String,
    /// Callback data sent to the bot when the button is pressed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_data: Option<String>,
    /// URL for link buttons.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl InlineKeyboardButton {
    /// Create a callback button (press sends data to bot).
    pub fn callback(text: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            callback_data: Some(data.into()),
            url: None,
        }
    }

    /// Create a URL link button.
    pub fn url(text: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            callback_data: None,
            url: Some(url.into()),
        }
    }
}

/// An inline keyboard (rows of buttons shown below a message).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineKeyboardMarkup {
    /// Array of button rows, each row is an array of buttons.
    pub inline_keyboard: Vec<Vec<InlineKeyboardButton>>,
}

impl InlineKeyboardMarkup {
    /// Create a keyboard from a single row of buttons.
    pub fn row(buttons: Vec<InlineKeyboardButton>) -> Self {
        Self {
            inline_keyboard: vec![buttons],
        }
    }

    /// Create a keyboard from multiple rows.
    pub fn rows(rows: Vec<Vec<InlineKeyboardButton>>) -> Self {
        Self { inline_keyboard: rows }
    }
}

// ---------------------------------------------------------------------------
// Chat action types for typing indicators
// ---------------------------------------------------------------------------

/// Telegram chat action (typing indicator).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatAction {
    Typing,
    UploadPhoto,
    UploadVideo,
    UploadDocument,
    FindLocation,
}

impl ChatAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Typing => "typing",
            Self::UploadPhoto => "upload_photo",
            Self::UploadVideo => "upload_video",
            Self::UploadDocument => "upload_document",
            Self::FindLocation => "find_location",
        }
    }
}

// ---------------------------------------------------------------------------
// Command parsing
// ---------------------------------------------------------------------------

/// Parsed bot command from a message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    pub command: String,
    pub args: Vec<String>,
}

/// Parse a Telegram message text into a command + args.
///
/// Handles both `/command arg1 arg2` and `/command@botname arg1 arg2`.
pub fn parse_command(text: &str) -> Option<ParsedCommand> {
    let text = text.trim();
    let cmd_part = text.strip_prefix('/')?;
    let parts: Vec<&str> = cmd_part.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }
    // Strip @botname if present
    let command = parts[0]
        .split('@')
        .next()
        .unwrap_or(parts[0])
        .to_string();
    let args = parts[1..].iter().map(|s| s.to_string()).collect();
    Some(ParsedCommand { command, args })
}

// ---------------------------------------------------------------------------
// Binding code (6-digit, 5-min TTL)
// ---------------------------------------------------------------------------

/// Generate a 6-digit binding code.
pub fn generate_binding_code() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    format!("{:06}", rng.gen_range(0..1_000_000))
}

/// A binding code with TTL metadata.
#[derive(Debug, Clone)]
pub struct BindingCode {
    pub code: String,
    pub user_id: String,
    pub created_at: std::time::Instant,
    pub ttl: std::time::Duration,
}

impl BindingCode {
    pub fn new(user_id: String) -> Self {
        Self {
            code: generate_binding_code(),
            user_id,
            created_at: std::time::Instant::now(),
            ttl: std::time::Duration::from_secs(300), // 5 minutes
        }
    }

    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_command() {
        let p = parse_command("/status").unwrap();
        assert_eq!(p.command, "status");
        assert!(p.args.is_empty());
    }

    #[test]
    fn parse_command_with_args() {
        let p = parse_command("/deploy myapp").unwrap();
        assert_eq!(p.command, "deploy");
        assert_eq!(p.args, vec!["myapp"]);
    }

    #[test]
    fn parse_command_with_botname() {
        let p = parse_command("/status@mybot").unwrap();
        assert_eq!(p.command, "status");
        assert!(p.args.is_empty());
    }

    #[test]
    fn parse_command_with_botname_and_args() {
        let p = parse_command("/deploy@mybot myapp --wait").unwrap();
        assert_eq!(p.command, "deploy");
        assert_eq!(p.args, vec!["myapp", "--wait"]);
    }

    #[test]
    fn parse_non_command_returns_none() {
        assert!(parse_command("hello world").is_none());
        assert!(parse_command("").is_none());
    }

    #[test]
    fn binding_code_is_6_digits() {
        let code = generate_binding_code();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn binding_code_ttl_not_expired_immediately() {
        let b = BindingCode::new("user1".to_string());
        assert!(!b.is_expired());
    }

    #[test]
    fn inline_keyboard_button_callback() {
        let btn = InlineKeyboardButton::callback("Approve", "confirm:deploy:myapp");
        assert_eq!(btn.text, "Approve");
        assert_eq!(btn.callback_data.as_deref(), Some("confirm:deploy:myapp"));
        assert!(btn.url.is_none());
    }

    #[test]
    fn inline_keyboard_button_url() {
        let btn = InlineKeyboardButton::url("Docs", "https://example.com");
        assert!(btn.callback_data.is_none());
        assert_eq!(btn.url.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn inline_keyboard_markup_row() {
        let kb = InlineKeyboardMarkup::row(vec![
            InlineKeyboardButton::callback("Yes", "yes"),
            InlineKeyboardButton::callback("No", "no"),
        ]);
        assert_eq!(kb.inline_keyboard.len(), 1);
        assert_eq!(kb.inline_keyboard[0].len(), 2);
    }

    #[test]
    fn chat_action_as_str() {
        assert_eq!(ChatAction::Typing.as_str(), "typing");
        assert_eq!(ChatAction::UploadDocument.as_str(), "upload_document");
    }

    #[test]
    fn callback_query_deserialize() {
        let json = r#"{
            "id": "123",
            "from": {"id": 1, "first_name": "Test", "username": "test"},
            "data": "confirm:deploy:myapp"
        }"#;
        let cq: CallbackQuery = serde_json::from_str(json).unwrap();
        assert_eq!(cq.id, "123");
        assert_eq!(cq.data.as_deref(), Some("confirm:deploy:myapp"));
        assert_eq!(cq.from.id, 1);
    }

    #[test]
    fn update_with_callback_query() {
        let json = r#"{
            "update_id": 1,
            "callback_query": {
                "id": "456",
                "from": {"id": 2, "first_name": "Bob"},
                "data": "approve"
            }
        }"#;
        let update: Update = serde_json::from_str(json).unwrap();
        assert!(update.message.is_none());
        let cq = update.callback_query.unwrap();
        assert_eq!(cq.data.as_deref(), Some("approve"));
    }

    #[test]
    fn update_without_callback_query() {
        let json = r#"{
            "update_id": 2,
            "message": {
                "message_id": 10,
                "chat": {"id": 100, "type": "private"},
                "text": "/help"
            }
        }"#;
        let update: Update = serde_json::from_str(json).unwrap();
        assert!(update.callback_query.is_none());
        assert!(update.message.is_some());
    }
}
