// argon2id password hashing — reuses the same KDF params as the master key.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2, PasswordHash, PasswordVerifier,
};

/// Parameters match the master key KDF: 64 MiB / t=3 / p=1.
const PARAMS: argon2::Params = match argon2::Params::new(64 * 1024, 3, 1, None) {
    Ok(p) => p,
    Err(_) => unreachable!(),
};

/// Hash a plaintext password. Returns the PHC string.
pub fn hash_password(plaintext: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, PARAMS);
    let hash = argon2
        .hash_password(plaintext.as_bytes(), &salt)
        .map_err(|e| AuthError::Hash(e.to_string()))?;
    Ok(hash.to_string())
}

/// Verify a plaintext password against a PHC hash.
pub fn verify_password(plaintext: &str, hash: &str) -> Result<bool, AuthError> {
    let parsed = PasswordHash::new(hash).map_err(|e| AuthError::Hash(e.to_string()))?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, PARAMS);
    Ok(argon2
        .verify_password(plaintext.as_bytes(), &parsed)
        .is_ok())
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("password hash error: {0}")]
    Hash(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let h = hash_password("hunter2").unwrap();
        assert!(verify_password("hunter2", &h).unwrap());
        assert!(!verify_password("wrong", &h).unwrap());
    }

    #[test]
    fn different_hashes_for_same_password() {
        let h1 = hash_password("test").unwrap();
        let h2 = hash_password("test").unwrap();
        assert_ne!(h1, h2, "salt randomization");
        assert!(verify_password("test", &h1).unwrap());
        assert!(verify_password("test", &h2).unwrap());
    }
}
