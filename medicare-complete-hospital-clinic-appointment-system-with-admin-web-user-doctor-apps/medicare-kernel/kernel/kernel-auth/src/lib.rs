//! kernel-auth: AUTH-01..08 building blocks. OTP issue/verify endpoints land in M1;
//! the SMS provider adapter trait is declared here (D-16: MSG91 first).
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("hash failure")]
    Hash,
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("jwt: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error("expired")]
    Expired,
}

// ---------------- passwords (AUTH-02) ----------------

pub fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| AuthError::Hash)
}

pub fn verify_password(password: &str, encoded: &str) -> Result<bool, AuthError> {
    let hash = PasswordHash::new(encoded).map_err(|_| AuthError::InvalidCredentials)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok())
}

// ---------------- OTP (D-13 server-side, D-16 MSG91 adapter) ----------------

/// 6-digit OTP. Skeleton derives randomness from uuid v7; production uses a
/// CSPRNG (M1-04). Stored hashed (argon2) in otp_codes; never logged (AUTH-07).
pub fn generate_otp() -> String {
    let generated = uuid::Uuid::now_v7();
    let bytes = generated.as_bytes();
    let mut acc: u32 = 0;
    for b in bytes {
        acc = acc.wrapping_mul(31).wrapping_add(u32::from(*b));
    }
    format!("{:06}", acc % 1_000_000)
}

pub fn hash_otp(code: &str) -> Result<String, AuthError> {
    hash_password(code)
}

pub fn verify_otp(code: &str, encoded: &str) -> Result<bool, AuthError> {
    verify_password(code, encoded)
}

// ---------------- sessions (AUTH-03) ----------------
// TODO(M1-02): ES256 + kid rotation. Dev skeleton signs HS256 (config jwt.mode).

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionClaims {
    pub sub: String,          // identity id
    pub plane: String,        // tenant | platform | support
    pub tenant: Option<String>,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
}

pub fn sign_access_token(claims: &SessionClaims, secret: &str) -> Result<String, AuthError> {
    let key = jsonwebtoken::EncodingKey::from_secret(secret.as_bytes());
    Ok(jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        claims,
        &key,
    )?)
}

pub fn verify_access_token(token: &str, secret: &str) -> Result<SessionClaims, AuthError> {
    let key = jsonwebtoken::DecodingKey::from_secret(secret.as_bytes());
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.leeway = 30;
    let data = jsonwebtoken::decode::<SessionClaims>(token, &key, &validation)?;
    Ok(data.claims)
}

// ---------------- provider adapters (D-16/D-17/D-18) ----------------

/// SMS/OTP delivery adapter. MSG91 impl wires in M1; messages go out via this
/// trait only (server-side OTP per D-13).
pub trait OtpChannel: Send + Sync {
    fn provider(&self) -> &'static str;
    fn send_otp(&self, phone_e164: &str, code_hash_ref: &str) -> Result<(), String>;
}

#[derive(Debug, Default)]
pub struct Msg91Stub;

impl OtpChannel for Msg91Stub {
    fn provider(&self) -> &'static str {
        "msg91"
    }
    fn send_otp(&self, phone_e164: &str, _code_hash_ref: &str) -> Result<(), String> {
        tracing::warn!(phone_prefix = &phone_e164[..phone_e164.len().min(4)], "MSG91 adapter not wired (M1-04)");
        Ok(())
    }
}