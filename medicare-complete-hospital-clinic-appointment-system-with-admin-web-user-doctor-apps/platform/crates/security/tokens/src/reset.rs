//! Password reset flows (S2 item 21, L23 completion): request -> deliver token
//! (transport's job) -> consume once -> set new password hash. Password
//! strength policy lives here; hashing stays in keel-identity (argon2id).

use std::sync::Arc;

use keel_foundation::{AppError, PrincipalId};
use keel_identity::{verify_password, Secret};
use time::Duration;

use crate::{IssuedToken, TokenPurpose, TokenService};

/// Password reset redemption window (short by design).
pub const RESET_TTL: Duration = Duration::minutes(60);
/// Minimum password length.
pub const MIN_PASSWORD_LEN: usize = 10;

/// Credential access needed by the reset flow. Implemented wherever
/// credentials are stored (postgres adapter in production, fakes in tests).
pub trait PasswordStore: Send + Sync {
    fn password_hash(&self, principal: PrincipalId) -> Result<Option<String>, AppError>;
    fn set_password_hash(&self, principal: PrincipalId, hash: &str) -> Result<(), AppError>;
}

/// Password strength policy (item 21): length + character classes.
/// Deliberately simple and deterministic; breach lists belong elsewhere.
pub fn validate_password_strength(password: &Secret) -> Result<(), AppError> {
    let pw = password.expose();
    if pw.chars().count() < MIN_PASSWORD_LEN {
        return Err(AppError::validation(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    let has_letter = pw.chars().any(|c| c.is_alphabetic());
    let has_digit = pw.chars().any(|c| c.is_ascii_digit());
    if !has_letter || !has_digit {
        return Err(AppError::validation(
            "password must contain letters and digits",
        ));
    }
    Ok(())
}

/// Reset flow mediator: tokens + credential store, no transport knowledge.
pub struct PasswordReset {
    tokens: TokenService,
    passwords: Arc<dyn PasswordStore>,
}

impl PasswordReset {
    pub fn new(tokens: TokenService, passwords: Arc<dyn PasswordStore>) -> Self {
        Self { tokens, passwords }
    }

    /// Step 1: issue a single-use reset token for a principal. The caller
    /// delivers the plaintext through a registered contact channel.
    pub fn request_reset(&self, principal: PrincipalId) -> Result<IssuedToken, AppError> {
        self.tokens.issue(
            principal,
            TokenPurpose::Reset,
            RESET_TTL,
            Some("password-reset".into()),
        )
    }

    /// Step 2: redeem the token and set the new password. Fails on unknown,
    /// expired, or already-used tokens, and on weak passwords (no state
    /// changes in that case).
    pub fn complete_reset(
        &self,
        plaintext: &str,
        new_password: &Secret,
    ) -> Result<PrincipalId, AppError> {
        let token = self.tokens.consume(TokenPurpose::Reset, plaintext)?;
        validate_password_strength(new_password)?;
        let hash = keel_identity::hash_password(new_password)?;
        self.passwords
            .set_password_hash(token.principal_id, &hash)?;
        Ok(token.principal_id)
    }

    /// Change an authenticated password (old password verified first).
    pub fn change_password(
        &self,
        principal: PrincipalId,
        current: &Secret,
        new_password: &Secret,
    ) -> Result<(), AppError> {
        let stored = self
            .passwords
            .password_hash(principal)?
            .ok_or_else(|| AppError::denied("no password credential on this account"))?;
        if !verify_password(current, &stored)? {
            return Err(AppError::denied("current password does not match"));
        }
        validate_password_strength(new_password)?;
        if new_password.expose() == current.expose() {
            return Err(AppError::validation(
                "new password must differ from the current one",
            ));
        }
        let hash = keel_identity::hash_password(new_password)?;
        self.passwords.set_password_hash(principal, &hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakePasswords {
        hashes: Mutex<HashMap<PrincipalId, String>>,
    }

    impl PasswordStore for FakePasswords {
        fn password_hash(&self, principal: PrincipalId) -> Result<Option<String>, AppError> {
            Ok(self.hashes.lock().unwrap().get(&principal).cloned())
        }
        fn set_password_hash(&self, principal: PrincipalId, hash: &str) -> Result<(), AppError> {
            self.hashes
                .lock()
                .unwrap()
                .insert(principal, hash.to_string());
            Ok(())
        }
    }

    fn flow() -> (PasswordReset, PrincipalId) {
        let passwords = Arc::new(FakePasswords::default());
        let principal = PrincipalId::new();
        let hash = keel_identity::hash_password(&Secret::new("old-password-99")).unwrap();
        passwords.hashes.lock().unwrap().insert(principal, hash);
        let tokens = TokenService::new(Arc::new(crate::InMemoryTokenStore::new()));
        (PasswordReset::new(tokens, passwords), principal)
    }

    #[test]
    fn reset_flow_end_to_end() {
        let (flow, principal) = flow();
        let issued = flow.request_reset(principal).unwrap();
        let done = flow
            .complete_reset(&issued.plaintext, &Secret::new("new-password-42"))
            .unwrap();
        assert_eq!(done, principal);
        let hash = flow.passwords.password_hash(principal).unwrap().unwrap();
        assert!(verify_password(&Secret::new("new-password-42"), &hash).unwrap());
        // token replay is rejected
        assert!(flow
            .complete_reset(&issued.plaintext, &Secret::new("another-pass-42"))
            .is_err());
    }

    #[test]
    fn weak_password_rejected_without_state_change() {
        let (flow, principal) = flow();
        let issued = flow.request_reset(principal).unwrap();
        assert!(flow
            .complete_reset(&issued.plaintext, &Secret::new("short1"))
            .is_err());
        assert!(flow
            .complete_reset(&issued.plaintext, &Secret::new("nodigitsatall"))
            .is_err());
        // failed attempt consumed the token (single-use), proving ordering:
        let hash = flow.passwords.password_hash(principal).unwrap().unwrap();
        assert!(verify_password(&Secret::new("old-password-99"), &hash).unwrap());
    }

    #[test]
    fn change_password_guards() {
        let (flow, principal) = flow();
        assert!(flow
            .change_password(
                principal,
                &Secret::new("wrong"),
                &Secret::new("new-password-42")
            )
            .is_err());
        assert!(flow
            .change_password(
                principal,
                &Secret::new("old-password-99"),
                &Secret::new("old-password-99")
            )
            .is_err());
        flow.change_password(
            principal,
            &Secret::new("old-password-99"),
            &Secret::new("new-password-42"),
        )
        .unwrap();
        let hash = flow.passwords.password_hash(principal).unwrap().unwrap();
        assert!(verify_password(&Secret::new("new-password-42"), &hash).unwrap());
    }

    #[test]
    fn strength_policy() {
        assert!(validate_password_strength(&Secret::new("abc1234567")).is_ok());
        assert!(validate_password_strength(&Secret::new("abc")).is_err());
        assert!(validate_password_strength(&Secret::new("letters-only-here")).is_err());
        assert!(validate_password_strength(&Secret::new("12345678901")).is_err());
    }
}
