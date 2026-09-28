//! # keel-identity
//! Identity model (11-SPEC §3): `Principal ≠ Actor ≠ Membership ≠ Credential ≠ Session`.
//! No `user_id == principal_id == actor_id` shortcuts.

pub mod store;

pub use store::{
    verify_credential, CredentialStore, InMemoryCredentialStore, InMemoryPrincipalStore,
    PrincipalStore,
};

use keel_foundation::{ActorId, AppError, MembershipId, PrincipalId, TenantId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// A secret wrapper: never Debug/Display-printed. Serde is allowed because the
/// wrapped value at rest is hash material (Argon2 PHC), never plaintext.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalStatus {
    Active,
    Suspended,
    Deleted,
}

/// Global identity: the authority holder across tenants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub id: PrincipalId,
    pub status: PrincipalStatus,
    pub email: Option<String>,
    pub phone: Option<String>,
    /// When the contact email was proven (B-23 email verification);
    /// None = not yet verified.
    #[serde(default, with = "time::serde::iso8601::option")]
    pub email_verified_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
}

impl Principal {
    pub fn is_valid(&self) -> bool {
        self.status == PrincipalStatus::Active
    }
}

/// Tenant-scoped working profile of a principal (one per tenant).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorProfile {
    pub id: ActorId,
    pub principal_id: PrincipalId,
    pub tenant_id: TenantId,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipStatus {
    Invited,
    Active,
    Suspended,
    Revoked,
}

/// The edge that connects a principal to a tenant/org (never implied).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Membership {
    pub id: MembershipId,
    pub principal_id: PrincipalId,
    pub tenant_id: TenantId,
    pub status: MembershipStatus,
    #[serde(with = "time::serde::iso8601")]
    pub joined_at: OffsetDateTime,
}

impl Membership {
    pub fn is_active(&self) -> bool {
        self.status == MembershipStatus::Active
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    PasswordArgon2,
    Otp,
    ApiKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub id: Uuid,
    pub principal_id: PrincipalId,
    pub kind: CredentialKind,
    pub secret_hash: Secret,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601::option")]
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub principal_id: PrincipalId,
    #[serde(with = "time::serde::iso8601")]
    pub expires_at: OffsetDateTime,
    pub source_ip: Option<String>,
    pub user_agent: Option<String>,
}

impl Session {
    pub fn expired(&self, now: OffsetDateTime) -> bool {
        now >= self.expires_at
    }
}

// --- password hashing (argon2id, parameterized defaults) ---

pub fn hash_password(password: &Secret) -> Result<String, AppError> {
    use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
    let salt = SaltString::generate(&mut rand::thread_rng());
    Argon2::default()
        .hash_password(password.expose().as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::internal(format!("password hash failed: {e}")))
}

pub fn verify_password(password: &Secret, encoded: &str) -> Result<bool, AppError> {
    use argon2::{password_hash::PasswordHash, Argon2, PasswordVerifier};
    let parsed = PasswordHash::new(encoded)
        .map_err(|e| AppError::internal(format!("bad hash encoding: {e}")))?;
    Ok(Argon2::default()
        .verify_password(password.expose().as_bytes(), &parsed)
        .is_ok())
}

// --- OTP ---

pub fn generate_otp_code(digit_count: u32) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..digit_count)
        .map(|_| rng.gen_range(0..10).to_string())
        .collect()
}

pub fn otp_matches(expected: &str, provided: &str) -> bool {
    constant_time_eq(expected.as_bytes(), provided.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let pw = Secret::new("correct horse battery staple");
        let hash = hash_password(&pw).unwrap();
        assert!(verify_password(&pw, &hash).unwrap());
        assert!(!verify_password(&Secret::new("wrong"), &hash).unwrap());
    }

    #[test]
    fn otp_shape_and_match() {
        let code = generate_otp_code(6);
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        assert!(otp_matches(&code, &code.clone()));
        assert!(!otp_matches(&code, "000000"));
    }

    #[test]
    fn membership_active_semantics() {
        let m = Membership {
            id: MembershipId::new(),
            principal_id: PrincipalId::new(),
            tenant_id: TenantId::new(),
            status: MembershipStatus::Suspended,
            joined_at: OffsetDateTime::now_utc(),
        };
        assert!(!m.is_active());
    }
}
