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
//! Issuing and revoking reach the write import, where the host checks the
//! guest's capability profile; verifying by hash and listing are reads.

use kelson_foundation::{AppError, PrincipalId};
use kelson_module_contract::{
    FindByHashArgs, ListForArgs, MarkConsumedArgs, RevokeArgs, TokenInsertArgs,
};
use kelson_tokens::{AuthToken, TokenPurpose, TokenStore};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{call, Port};

/// Decode a host answer into the method's result type.
fn decode<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, AppError> {
    serde_json::from_value(value)
        .map_err(|e| AppError::internal(format!("module sdk: seam answer is unreadable: {e}")))
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
        call(
            Port::TokenWrite,
            "insert",
            &TokenInsertArgs { token },
        )
        .map(|_| ())
    }

    fn find_by_hash(
        &self,
        purpose: TokenPurpose,
        hash: &str,
    ) -> Result<Option<AuthToken>, AppError> {
        decode(call(
            Port::TokenRead,
            "find_by_hash",
            &FindByHashArgs {
                purpose,
                hash: hash.to_string(),
            },
        )?)
    }

    fn mark_consumed(&self, id: Uuid, at: OffsetDateTime) -> Result<bool, AppError> {
        decode(call(
            Port::TokenWrite,
            "mark_consumed",
            &MarkConsumedArgs { id, at },
        )?)
    }

    fn revoke(&self, id: Uuid) -> Result<(), AppError> {
        call(Port::TokenWrite, "revoke", &RevokeArgs { id }).map(|_| ())
    }

    fn list_for(&self, principal: PrincipalId) -> Result<Vec<AuthToken>, AppError> {
        decode(call(
            Port::TokenRead,
            "list_for",
            &ListForArgs { principal },
        )?)
    }
}
