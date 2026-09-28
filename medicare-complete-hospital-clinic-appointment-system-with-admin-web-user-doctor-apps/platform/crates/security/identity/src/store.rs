//! Identity store ports (S7): `PrincipalStore` + `CredentialStore`.
//! The HTTP auth flows (register/login/reset) resolve identity through these
//! ports; `keel-postgres` implements them on the `principals`/`credentials`
//! tables, and the in-memory impls serve degraded boot and tests.

use std::sync::RwLock;

use keel_foundation::{AppError, PrincipalId};

use crate::{Credential, CredentialKind, Principal, PrincipalStatus, Secret};

/// Principal persistence port. Emails/phones are unique identities.
pub trait PrincipalStore: Send + Sync {
    fn create(&self, principal: Principal) -> Result<Principal, AppError>;
    fn by_id(&self, id: PrincipalId) -> Result<Option<Principal>, AppError>;
    fn by_email(&self, email: &str) -> Result<Option<Principal>, AppError>;
    fn update_status(&self, id: PrincipalId, status: PrincipalStatus) -> Result<(), AppError>;
    /// Record contact proof (B-23): set the email verification timestamp.
    fn mark_email_verified(&self, id: PrincipalId) -> Result<(), AppError>;
}

/// Credential persistence port. One active password credential per principal.
pub trait CredentialStore: Send + Sync {
    fn create(&self, credential: Credential) -> Result<Credential, AppError>;
    /// The principal's argon2 password credential (if any).
    fn password_for(&self, principal: PrincipalId) -> Result<Option<Credential>, AppError>;
    /// Swap the stored password hash (password reset); NOT_FOUND if the
    /// principal has no password credential.
    fn replace_password_hash(
        &self,
        principal: PrincipalId,
        new_hash: Secret,
    ) -> Result<(), AppError>;
}

/// Verify a plaintext password against a stored credential. Returns false
/// (never a distinguishing error) for kind mismatches.
pub fn verify_credential(credential: &Credential, password: &Secret) -> Result<bool, AppError> {
    if credential.kind != CredentialKind::PasswordArgon2 {
        return Ok(false);
    }
    crate::verify_password(password, credential.secret_hash.expose())
}

/// Degraded-boot / test store: principals keyed by id.
#[derive(Debug, Default)]
pub struct InMemoryPrincipalStore {
    inner: RwLock<Vec<Principal>>,
}

impl InMemoryPrincipalStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PrincipalStore for InMemoryPrincipalStore {
    fn create(&self, principal: Principal) -> Result<Principal, AppError> {
        let mut rows = self.inner.write().unwrap();
        if let Some(email) = &principal.email {
            if rows
                .iter()
                .any(|p| p.email.as_deref() == Some(email.as_str()))
            {
                return Err(AppError::conflict(format!(
                    "principal with email {email} exists"
                )));
            }
        }
        if rows.iter().any(|p| p.id == principal.id) {
            return Err(AppError::conflict(format!("principal {}", principal.id)));
        }
        rows.push(principal.clone());
        Ok(principal)
    }

    fn by_id(&self, id: PrincipalId) -> Result<Option<Principal>, AppError> {
        Ok(self
            .inner
            .read()
            .unwrap()
            .iter()
            .find(|p| p.id == id)
            .cloned())
    }

    fn by_email(&self, email: &str) -> Result<Option<Principal>, AppError> {
        Ok(self
            .inner
            .read()
            .unwrap()
            .iter()
            .find(|p| p.email.as_deref() == Some(email))
            .cloned())
    }

    fn update_status(&self, id: PrincipalId, status: PrincipalStatus) -> Result<(), AppError> {
        let mut rows = self.inner.write().unwrap();
        let Some(row) = rows.iter_mut().find(|p| p.id == id) else {
            return Err(AppError::not_found(format!("principal {id}")));
        };
        row.status = status;
        Ok(())
    }

    fn mark_email_verified(&self, id: PrincipalId) -> Result<(), AppError> {
        let mut rows = self.inner.write().unwrap();
        let Some(row) = rows.iter_mut().find(|p| p.id == id) else {
            return Err(AppError::not_found(format!("principal {id}")));
        };
        if row.email_verified_at.is_none() {
            row.email_verified_at = Some(time::OffsetDateTime::now_utc());
        }
        Ok(())
    }
}

/// Degraded-boot / test store: credentials keyed by id.
#[derive(Debug, Default)]
pub struct InMemoryCredentialStore {
    inner: RwLock<Vec<Credential>>,
}

impl InMemoryCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CredentialStore for InMemoryCredentialStore {
    fn create(&self, credential: Credential) -> Result<Credential, AppError> {
        let mut rows = self.inner.write().unwrap();
        if credential.kind == CredentialKind::PasswordArgon2
            && rows.iter().any(|c| {
                c.principal_id == credential.principal_id
                    && c.kind == CredentialKind::PasswordArgon2
            })
        {
            return Err(AppError::conflict(format!(
                "password credential exists for principal {}",
                credential.principal_id
            )));
        }
        rows.push(credential.clone());
        Ok(credential)
    }

    fn password_for(&self, principal: PrincipalId) -> Result<Option<Credential>, AppError> {
        Ok(self
            .inner
            .read()
            .unwrap()
            .iter()
            .find(|c| c.principal_id == principal && c.kind == CredentialKind::PasswordArgon2)
            .cloned())
    }

    fn replace_password_hash(
        &self,
        principal: PrincipalId,
        new_hash: Secret,
    ) -> Result<(), AppError> {
        let mut rows = self.inner.write().unwrap();
        let Some(row) = rows
            .iter_mut()
            .find(|c| c.principal_id == principal && c.kind == CredentialKind::PasswordArgon2)
        else {
            return Err(AppError::not_found(format!(
                "password credential for {principal}"
            )));
        };
        row.secret_hash = new_hash;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn principal(email: &str) -> Principal {
        Principal {
            id: PrincipalId::new(),
            status: PrincipalStatus::Active,
            email: Some(email.into()),
            phone: None,
            email_verified_at: None,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    fn password_credential(pid: PrincipalId, hash: &str) -> Credential {
        Credential {
            id: Uuid::now_v7(),
            principal_id: pid,
            kind: CredentialKind::PasswordArgon2,
            secret_hash: Secret::new(hash),
            created_at: OffsetDateTime::now_utc(),
            expires_at: None,
        }
    }

    #[test]
    fn principal_roundtrip_and_email_uniqueness() {
        let store = InMemoryPrincipalStore::new();
        let p = store.create(principal("a@b.co")).unwrap();
        assert_eq!(
            store.by_id(p.id).unwrap().unwrap().email.as_deref(),
            Some("a@b.co")
        );
        assert_eq!(store.by_email("a@b.co").unwrap().unwrap().id, p.id);
        assert!(store.by_email("other@b.co").unwrap().is_none());
        assert!(store.create(principal("a@b.co")).is_err());
        store
            .update_status(p.id, PrincipalStatus::Suspended)
            .unwrap();
        assert!(!store.by_id(p.id).unwrap().unwrap().is_valid());
        assert!(store
            .update_status(PrincipalId::new(), PrincipalStatus::Active)
            .is_err());
    }

    #[test]
    fn credential_roundtrip_and_password_swap() {
        let store = InMemoryCredentialStore::new();
        let pid = PrincipalId::new();
        store
            .create(password_credential(pid, "$argon2id$AAA"))
            .unwrap();
        assert!(
            store
                .create(password_credential(pid, "$argon2id$BBB"))
                .is_err(),
            "one password per principal"
        );
        assert!(store.password_for(pid).unwrap().is_some());
        assert!(store.password_for(PrincipalId::new()).unwrap().is_none());
        store
            .replace_password_hash(pid, Secret::new("$argon2id$CCC"))
            .unwrap();
        assert_eq!(
            store
                .password_for(pid)
                .unwrap()
                .unwrap()
                .secret_hash
                .expose(),
            "$argon2id$CCC"
        );
        assert!(store
            .replace_password_hash(PrincipalId::new(), Secret::new("x"))
            .is_err());
    }

    #[test]
    fn verify_credential_matches_hashed_password() {
        let pw = Secret::new("hunter2-hunter2");
        let hash = crate::hash_password(&pw).unwrap();
        let cred = password_credential(PrincipalId::new(), &hash);
        assert!(verify_credential(&cred, &pw).unwrap());
        assert!(!verify_credential(&cred, &Secret::new("wrong")).unwrap());
        let otp = Credential {
            kind: CredentialKind::Otp,
            ..cred
        };
        assert!(!verify_credential(&otp, &pw).unwrap());
    }
}
