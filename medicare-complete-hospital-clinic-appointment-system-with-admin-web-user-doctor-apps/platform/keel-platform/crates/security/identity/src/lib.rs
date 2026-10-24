//! # kelson-identity
//! Identity model (11-SPEC §3): `Principal ≠ Actor ≠ Membership ≠ Credential ≠ Session`.
//! No `user_id == principal_id == actor_id` shortcuts.

pub mod store;

pub use store::{
    verify_credential, CredentialStore, InMemoryCredentialStore, InMemoryMembershipStore,
    InMemoryPrincipalStore, MembershipStore, PrincipalStore,
};

use kelson_foundation::{ActorId, AppError, MembershipId, PrincipalId, TenantId};
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
    hash_password_with(&PasswordParams::default(), password)
}

pub fn verify_password(password: &Secret, encoded: &str) -> Result<bool, AppError> {
    use argon2::{password_hash::PasswordHash, Argon2, PasswordVerifier};
    let parsed = PasswordHash::new(encoded)
        .map_err(|e| AppError::internal(format!("bad hash encoding: {e}")))?;
    Ok(Argon2::default()
        .verify_password(password.expose().as_bytes(), &parsed)
        .is_ok())
}

// --- parameterized hashing (Wave 7 P5, `[identity]`) ---

/// Argon2id work factors for password verifiers (Wave 7 P5): `memory` in
/// kibibytes (PHC `m`), `time` = passes (PHC `t`), `parallelism` = lanes
/// (PHC `p`). Defaults mirror `argon2::Params`' own baseline, so a stock
/// `[identity]` config hashes exactly as before the section existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordParams {
    pub memory: u32,
    pub time: u32,
    pub parallelism: u32,
}

impl Default for PasswordParams {
    fn default() -> Self {
        Self {
            memory: argon2::Params::DEFAULT_M_COST,
            time: argon2::Params::DEFAULT_T_COST,
            parallelism: argon2::Params::DEFAULT_P_COST,
        }
    }
}

impl PasswordParams {
    /// The hasher for these factors (argon2id v19). Out-of-range factors —
    /// a config typo like `time = 0` or `memory` under `8 * parallelism` —
    /// are refused HERE, where they are checked, never half-way through a
    /// register or login.
    pub fn to_argon2(&self) -> Result<argon2::Argon2<'static>, AppError> {
        let params = argon2::Params::new(self.memory, self.time, self.parallelism, None)
            .map_err(|e| AppError::validation(format!("invalid [identity] argon2 factors: {e}")))?;
        Ok(argon2::Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            params,
        ))
    }
}

/// Hash with explicit factors. The PHC string embeds them, so verification
/// (which reads the STORED factors from the encoding) and drift detection
/// (`password_params_of`) both work regardless of who hashed it.
pub fn hash_password_with(params: &PasswordParams, password: &Secret) -> Result<String, AppError> {
    use argon2::{password_hash::SaltString, PasswordHasher};
    let salt = SaltString::generate(&mut rand::thread_rng());
    params
        .to_argon2()?
        .hash_password(password.expose().as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::internal(format!("password hash failed: {e}")))
}

/// The factors a stored PHC verifier carries (Wave 7 P5 drift detection):
/// `None` when the encoding does not parse as argon2.
pub fn password_params_of(encoded: &str) -> Option<PasswordParams> {
    use argon2::password_hash::PasswordHash;
    let parsed = PasswordHash::new(encoded).ok()?;
    let params = argon2::Params::try_from(&parsed).ok()?;
    Some(PasswordParams {
        memory: params.m_cost(),
        time: params.t_cost(),
        parallelism: params.p_cost(),
    })
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

    /// A **malformed verifier row** (a corrupt hash in the database — a bad
    /// encoding, a truncated PHC string, a hash of another algorithm) is
    /// REFUSED, never treated as a match and never a panic: a corrupt row must
    /// fail closed. The public message stays the generic one (the parse detail
    /// is the internal context) so the caller cannot tell a corrupt row from
    /// any other internal failure.
    #[test]
    fn a_malformed_verifier_row_is_refused() {
        let pw = Secret::new("correct horse battery staple");
        for corrupt in [
            "",
            "not-a-hash",
            "$argon2id$v=19$m=19456,t=2,p=1$",
            "$argon2id$m=19456,t=2,p=1$c29tZXNhbHQ$",
            "$2y$10$abcdefghijklmnopqrstuv",
            "$argon2id$v=19$m=notanumber,t=2,p=1$c29tZXNhbHQ$aaaa",
        ] {
            let err =
                verify_password(&pw, corrupt).expect_err("a malformed verifier row is refused");
            assert_eq!(
                err.public_message, "internal error",
                "the public refusal is the generic one: {corrupt:?}"
            );
            assert!(
                err.internal
                    .unwrap_or_default()
                    .contains("bad hash encoding"),
                "the detail is kept for the operator: {corrupt:?}"
            );
        }
    }

    /// Wave 7 P5: configured factors embed in the PHC string and read back;
    /// verification honors the STORED factors, so a drifted verifier still
    /// checks out (that is what makes rehash-on-login possible) while the
    /// comparison against the baseline flags the drift to be healed.
    #[test]
    fn password_params_roundtrip_and_drift_is_detectable() {
        let pw = Secret::new("correct horse battery staple");
        let drifted = PasswordParams {
            memory: 4096,
            time: 3,
            parallelism: 1,
        };
        let stored = hash_password_with(&drifted, &pw).unwrap();
        assert_eq!(password_params_of(&stored), Some(drifted));
        assert!(verify_password(&pw, &stored).unwrap());
        assert!(!verify_password(&Secret::new("wrong"), &stored).unwrap());
        assert_ne!(
            password_params_of(&stored),
            Some(PasswordParams::default()),
            "drift against the baseline is visible to the login check"
        );

        // The baseline itself is unchanged: default hashing embeds the
        // argon2 defaults verbatim (non-regression for existing verifiers).
        let baseline = hash_password(&pw).unwrap();
        assert_eq!(
            password_params_of(&baseline),
            Some(PasswordParams::default())
        );
        assert_eq!(
            (
                PasswordParams::default().memory,
                PasswordParams::default().time,
                PasswordParams::default().parallelism
            ),
            (19_456, 2, 1),
            "the documented [identity] defaults stay pinned to argon2's baseline"
        );
        assert!(verify_password(&pw, &baseline).unwrap());
    }

    /// Wave 7 P5: out-of-range factors fail where they are checked
    /// (`to_argon2`), never halfway through a register or login.
    #[test]
    fn out_of_range_factors_are_refused_at_the_boundary() {
        assert!(PasswordParams {
            memory: 3,
            time: 2,
            parallelism: 1,
        }
        .to_argon2()
        .is_err());
        assert!(PasswordParams {
            time: 0,
            ..PasswordParams::default()
        }
        .to_argon2()
        .is_err());
        assert!(PasswordParams::default().to_argon2().is_ok());
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
