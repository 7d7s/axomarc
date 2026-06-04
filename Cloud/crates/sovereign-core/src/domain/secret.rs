// `Secret` — a single key/value pair encrypted at rest (age, F7). The
// domain carries the *ciphertext* and metadata only; plaintext is
// handled by the secrets port and never lives on disk in cleartext.

use super::id::{AppId, SecretId};
use super::timestamp::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash,
    Serialize, Deserialize, sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum SecretStatus {
    Active,
    Rotating,
    Retired,
}

impl SecretStatus {
    pub fn can_transition_to(self, other: Self) -> bool {
        use SecretStatus::*;
        matches!(
            (self, other),
            (Active, Rotating) | (Rotating, Active) | (Rotating, Retired)
        )
    }
}

impl std::fmt::Display for SecretStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Active => "active",
            Self::Rotating => "rotating",
            Self::Retired => "retired",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct Secret {
    pub id: SecretId,
    pub app_id: AppId,
    pub key: String,
    /// age-encrypted ciphertext. Never logged. Never returned to the CLI
    /// (the secrets port exposes a read-with-acknowledgement flow).
    pub ciphertext: Vec<u8>,
    pub created_at: Timestamp,
    pub rotated_at: Option<Timestamp>,
    pub version: i64,
    pub status: SecretStatus,
}

#[derive(Debug, Clone)]
pub struct NewSecret {
    pub app_id: AppId,
    pub key: String,
    /// Plaintext or already-encrypted bytes — the port decides.
    pub ciphertext: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_state_machine() {
        use SecretStatus::*;
        assert!(Active.can_transition_to(Rotating));
        assert!(Rotating.can_transition_to(Active));
        assert!(Rotating.can_transition_to(Retired));
        assert!(!Active.can_transition_to(Retired));
        assert!(!Retired.can_transition_to(Active));
    }
}
