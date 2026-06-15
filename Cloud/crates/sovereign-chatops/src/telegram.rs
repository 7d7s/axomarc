// P7: Telegram Bot API client — send messages, get updates, edit, callbacks.
//
// Uses reqwest for HTTP. The client is stateless; all state
// (offset, bound chat IDs) is managed by the poller loop.

use reqwest::Client;
use serde::Deserialize;

use crate::types::{ChatAction, InlineKeyboardMarkup, Message, Update};

/// Telegram Bot API base URL.
const API_BASE: &str = "https://api.telegram.org";

/// Response from getUpdates.
#[derive(Debug, Deserialize)]
pub struct GetUpdatesResponse {
    pub ok: bool,
    pub result: Vec<Update>,
}

/// Response from sendMessage / editMessageText.
#[derive(Debug, Deserialize)]
pub struct SendMessageResponse {
    pub ok: bool,
    pub result: Option<Message>,
}

/// Generic API response (for answerCallbackQuery, sendChatAction).
#[derive(Debug, Deserialize)]
pub struct ApiResponse {
    pub ok: bool,
    pub description: Option<String>,
}

/// Telegram API error.
#[derive(Debug, thiserror::Error)]
pub enum TelegramError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error: {description}")]
    Api { description: String, retry_after: Option<u64> },

    #[error("rate limited, retry after {0}s")]
    RateLimited(u64),
}

impl TelegramError {
    pub fn is_rate_limit(&self) -> bool {
        matches!(self, Self::RateLimited(_))
    }

    pub fn retry_after(&self) -> Option<u64> {
        match self {
            Self::RateLimited(secs) => Some(*secs),
            Self::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

/// Client for the Telegram Bot API.
#[derive(Clone)]
pub struct TelegramClient {
    api_base: String,
    client: Client,
}

impl TelegramClient {
    pub fn new(token: &str) -> Self {
        Self {
            api_base: format!("{API_BASE}/bot{token}"),
            client: Client::new(),
        }
    }

    // -----------------------------------------------------------------------
    // getUpdates (long-polling)
    // -----------------------------------------------------------------------

    /// Fetch updates from Telegram (long-polling).
    /// Requests both messages and callback_queries.
    pub async fn get_updates(&self, offset: i64, timeout: u64) -> Result<Vec<Update>, TelegramError> {
        let url = format!(
            "{}/getUpdates?offset={offset}&timeout={timeout}&allowed_updates=[\"message\",\"callback_query\"]",
            self.api_base
        );
        let resp = self.client.get(&url).send().await?.error_for_status()?;

        // Check for 429 (rate limit)
        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let retry_after = body["parameters"]["retry_after"]
                .as_u64()
                .unwrap_or(5);
            return Err(TelegramError::RateLimited(retry_after));
        }

        let body: GetUpdatesResponse = resp.json().await?;
        if !body.ok {
            return Err(TelegramError::Api {
                description: "getUpdates returned ok=false".to_string(),
                retry_after: None,
            });
        }
        Ok(body.result)
    }

    // -----------------------------------------------------------------------
    // sendMessage variants
    // -----------------------------------------------------------------------

    /// Send a text message to a chat (MarkdownV2).
    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
    ) -> Result<(), TelegramError> {
        let url = format!("{}/sendMessage", self.api_base);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "MarkdownV2",
        });
        let resp = self.client.post(&url).json(&body).send().await?;

        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let retry_after = body["parameters"]["retry_after"]
                .as_u64()
                .unwrap_or(5);
            return Err(TelegramError::RateLimited(retry_after));
        }

        let _ = resp.error_for_status()?;
        Ok(())
    }

    /// Send a text message without parse_mode (plain text).
    pub async fn send_message_plain(
        &self,
        chat_id: i64,
        text: &str,
    ) -> Result<(), TelegramError> {
        let url = format!("{}/sendMessage", self.api_base);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
        });
        let resp = self.client.post(&url).json(&body).send().await?;

        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let retry_after = body["parameters"]["retry_after"]
                .as_u64()
                .unwrap_or(5);
            return Err(TelegramError::RateLimited(retry_after));
        }

        let _ = resp.error_for_status()?;
        Ok(())
    }

    /// Send a message with an inline keyboard.
    pub async fn send_message_with_keyboard(
        &self,
        chat_id: i64,
        text: &str,
        keyboard: &InlineKeyboardMarkup,
    ) -> Result<(), TelegramError> {
        let url = format!("{}/sendMessage", self.api_base);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "reply_markup": keyboard,
        });
        let resp = self.client.post(&url).json(&body).send().await?;

        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let retry_after = body["parameters"]["retry_after"]
                .as_u64()
                .unwrap_or(5);
            return Err(TelegramError::RateLimited(retry_after));
        }

        let _ = resp.error_for_status()?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // editMessageText (for live progress updates)
    // -----------------------------------------------------------------------

    /// Edit an existing message's text (plain, no parse_mode).
    pub async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
    ) -> Result<(), TelegramError> {
        let url = format!("{}/editMessageText", self.api_base);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": text,
        });
        let resp = self.client.post(&url).json(&body).send().await?;

        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let retry_after = body["parameters"]["retry_after"]
                .as_u64()
                .unwrap_or(5);
            return Err(TelegramError::RateLimited(retry_after));
        }

        let _ = resp.error_for_status()?;
        Ok(())
    }

    /// Edit an existing message's text and attach a new inline keyboard.
    pub async fn edit_message_text_with_keyboard(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        keyboard: &InlineKeyboardMarkup,
    ) -> Result<(), TelegramError> {
        let url = format!("{}/editMessageText", self.api_base);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": text,
            "reply_markup": keyboard,
        });
        let resp = self.client.post(&url).json(&body).send().await?;

        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let retry_after = body["parameters"]["retry_after"]
                .as_u64()
                .unwrap_or(5);
            return Err(TelegramError::RateLimited(retry_after));
        }

        let _ = resp.error_for_status()?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // answerCallbackQuery (acknowledge inline button press)
    // -----------------------------------------------------------------------

    /// Answer a callback query to acknowledge the button press.
    /// Must be called within 30s or Telegram shows a loading spinner.
    pub async fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
        show_alert: bool,
    ) -> Result<(), TelegramError> {
        let url = format!("{}/answerCallbackQuery", self.api_base);
        let mut body = serde_json::json!({
            "callback_query_id": callback_query_id,
            "show_alert": show_alert,
        });
        if let Some(t) = text {
            body["text"] = serde_json::Value::String(t.to_string());
        }
        let resp = self.client.post(&url).json(&body).send().await?;

        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let retry_after = body["parameters"]["retry_after"]
                .as_u64()
                .unwrap_or(5);
            return Err(TelegramError::RateLimited(retry_after));
        }

        let _ = resp.error_for_status()?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // sendChatAction (typing indicator)
    // -----------------------------------------------------------------------

    /// Send a chat action (typing indicator). Shows "typing..." for 5 seconds.
    pub async fn send_chat_action(
        &self,
        chat_id: i64,
        action: ChatAction,
    ) -> Result<(), TelegramError> {
        let url = format!("{}/sendChatAction", self.api_base);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "action": action.as_str(),
        });
        let resp = self.client.post(&url).json(&body).send().await?;

        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let retry_after = body["parameters"]["retry_after"]
                .as_u64()
                .unwrap_or(5);
            return Err(TelegramError::RateLimited(retry_after));
        }

        let _ = resp.error_for_status()?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // deleteMessage (cleanup)
    // -----------------------------------------------------------------------

    /// Delete a message.
    pub async fn delete_message(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<(), TelegramError> {
        let url = format!("{}/deleteMessage", self.api_base);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id,
        });
        let resp = self.client.post(&url).json(&body).send().await?;

        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let retry_after = body["parameters"]["retry_after"]
                .as_u64()
                .unwrap_or(5);
            return Err(TelegramError::RateLimited(retry_after));
        }

        let _ = resp.error_for_status()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_error_is_rate_limit() {
        assert!(TelegramError::RateLimited(5).is_rate_limit());
        assert!(!TelegramError::Api {
            description: "test".to_string(),
            retry_after: None,
        }
        .is_rate_limit());
    }

    #[test]
    fn telegram_error_retry_after() {
        assert_eq!(TelegramError::RateLimited(10).retry_after(), Some(10));
        assert_eq!(
            TelegramError::Api {
                description: "test".to_string(),
                retry_after: Some(3),
            }
            .retry_after(),
            Some(3)
        );
        // Http errors have no retry_after
        let err = TelegramError::Api {
            description: "test".to_string(),
            retry_after: None,
        };
        assert_eq!(err.retry_after(), None);
    }

    #[test]
    fn client_is_clone() {
        let client = TelegramClient::new("test-token");
        let _cloned = client.clone();
    }
}
