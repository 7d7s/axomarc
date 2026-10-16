//! `TokenStore` as a guest sees it.
//!
//! The service above this store — `kelson_tokens::TokenService` — is a
//! *concrete* type, so a guest constructs the real one with this behind it.
//! Token minting, hashing and expiry therefore keep their real
//! implementation inside the guest; only persistence crosses the seam. That
//! is the cheap half of ADR 0020 §1, and it is worth having: a guest that
//! had to re-implement token semantics would be a second implementation to
//! keep correct.
//!
//! `verify` and `consume` read a row by hash, so they are reads; issuing and
//! revoking are writes and reach the write import, where the host applies the
//! capability_write posture.

use kelson_foundation::{AppError, PrincipalId};
use kelson_tokens::{AuthToken, TokenPurpose, TokenStore};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{host_call, Port};

fn encode<T: serde::Serialize>(args: T) -> Result<String, AppError> {
    serde_json::to_string(&args)
        .map_err(|e| AppError::internal(format!("module sdk: arguments are not encodable: {e}")))
}

fn decode<T: serde::de::DeserializeOwned>(body: &str, method: &str) -> Result<T, AppError> {
    serde_json::from_str(body)
        .map_err(|e| AppError::internal(format!("module sdk: {method} answer is unreadable: {e}")))
}

/// A `TokenStore` whose rows live wherever the host keeps them.
#[derive(Debug, Default, Clone, Copy)]
pub struct HostTokenStore;

impl HostTokenStore {
    pub const fn new() -> Self {
        Self
    }
}

impl TokenStore for HostTokenStore {
    fn insert(&self, token: AuthToken) -> Result<(), AppError> {
        let out = host_call(Port::TokenWrite, "insert", &encode((token,))?)?;
        acknowledgement(&out, "insert")
    }

    fn find_by_hash(
        &self,
        purpose: TokenPurpose,
        hash: &str,
    ) -> Result<Option<AuthToken>, AppError> {
        let out = host_call(Port::TokenRead, "find_by_hash", &encode((purpose, hash))?)?;
        decode(&out, "find_by_hash")
    }

    fn mark_consumed(&self, id: Uuid, at: OffsetDateTime) -> Result<bool, AppError> {
        let out = host_call(Port::TokenWrite, "mark_consumed", &encode((id, at))?)?;
        decode(&out, "mark_consumed")
    }

    fn revoke(&self, id: Uuid) -> Result<(), AppError> {
        let out = host_call(Port::TokenWrite, "revoke", &encode((id,))?)?;
        acknowledgement(&out, "revoke")
    }

    fn list_for(&self, principal: PrincipalId) -> Result<Vec<AuthToken>, AppError> {
        let out = host_call(Port::TokenRead, "list_for", &encode((principal,))?)?;
        decode(&out, "list_for")
    }
}

/// Read a write's verdict.
///
/// A `()`-returning store method has no value to decode, and the empty
/// string is refusal on every returning import — so a successful void write
/// and a denied one would otherwise be the same bytes, and the caller could
/// not tell "stored" from "refused". The host therefore answers a void write
/// with a non-empty acknowledgement, and this is the one place that
/// distinction is read back.
fn acknowledgement(out: &str, method: &str) -> Result<(), AppError> {
    if out.trim().is_empty() {
        return Err(AppError::internal(format!(
            "module sdk: host refused {method}"
        )));
    }
    Ok(())
}
