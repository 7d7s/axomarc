//! Passphrase-wrapped master key (V0.5+).
//!
//! On disk, the master key file is a binary blob:
//!
//! ```text
//! +---------------------------+
//! | magic       (24 bytes)    |  = "AGE-SOVEREIGN-WRAPPED-V1\n"
//! | salt        (16 bytes)    |  = random per-key
//! | m_cost      (4 bytes LE)  |  = Argon2id memory cost (KiB)
//! | t_cost      (4 bytes LE)  |  = Argon2id iterations
//! | p_cost      (4 bytes LE)  |  = Argon2id parallelism
//! | nonce       (24 bytes)    |  = XChaCha20-Poly1305 nonce
//! | ciphertext  (... bytes)   |  = plaintext + 16 byte tag
//! +---------------------------+
//! ```
//!
//! The KDF is Argon2id (`argon2` crate). The AEAD is
//! XChaCha20-Poly1305 (`chacha20poly1305` crate). Both are
//! constant-time implementations audited by their respective
//! upstream projects; this module adds no crypto on top of them.
//!
//! # Why these parameters
//!
//! `M_COST_KIB=64 * 1024` (64 MiB), `T_COST=3`, `P_COST=1` follow
//! the OWASP 2024 password-hashing recommendations for
//! interactive use (the "low-memory" tier). At these settings an
//! unlock on a CX22 takes ~300 ms — well within the budget for
//! a CLI that runs at most a few times per session. Bumping
//! `T_COST` to 6 would double that for marginal security gain;
//! bumping `M_COST_KIB` to 256 MiB would also double it but is
//! unsafe on the canonical 4 GiB-RAM CX22 under deployment load.

use anyhow::{anyhow, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroize;

/// The on-disk magic that identifies a V1 wrapped key. Includes
/// the trailing newline so a hexdump is self-describing.
pub const MAGIC: &[u8; 25] = b"AGE-SOVEREIGN-WRAPPED-V1\n";

/// Salt length in bytes. 128 bits matches the OWASP 2024
/// minimum for Argon2id.
pub const SALT_LEN: usize = 16;

/// Nonce length for the XChaCha20-Poly1305 AEAD. 24 bytes
/// (192 bits) means random nonces are safe up to 2^64 messages
/// per key (the XChaCha20 construction extends IETF ChaCha20's
/// 12-byte nonce with a 192-bit one).
pub const NONCE_LEN: usize = 24;

/// Argon2id memory cost in KiB. 64 MiB.
pub const M_COST_KIB: u32 = 64 * 1024;

/// Argon2id iterations.
pub const T_COST: u32 = 3;

/// Argon2id parallelism (lanes). 1 is fine for a single-user CLI.
pub const P_COST: u32 = 1;

/// AEAD key length (32 bytes = 256 bits).
const KEY_LEN: usize = 32;

/// The on-disk header length in bytes (NOT including the
/// ciphertext, which has a variable length matching the
/// plaintext + 16 tag bytes).
pub const HEADER_LEN: usize = MAGIC.len() + SALT_LEN + 4 + 4 + 4 + NONCE_LEN;

/// Wrap a plaintext `AGE-SECRET-KEY-1...` Bech32 string under
/// `passphrase` and return the binary file contents. The
/// passphrase is held in a `SecretString` so it does not
/// accidentally get logged.
pub fn wrap(passphrase: &SecretString, plaintext_bech32: &str) -> Result<Vec<u8>> {
    let mut rng = rand::thread_rng();
    let mut salt = [0u8; SALT_LEN];
    rng.fill_bytes(&mut salt);
    let mut nonce = [0u8; NONCE_LEN];
    rng.fill_bytes(&mut nonce);

    let mut key = derive_key(
        passphrase.expose_secret().as_bytes(),
        &salt,
        M_COST_KIB,
        T_COST,
        P_COST,
    )?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
    let ct = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext_bech32.as_bytes(),
                aad: MAGIC,
            },
        )
        .map_err(|e| anyhow!("encrypt: {e}"))?;
    key.zeroize();

    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&M_COST_KIB.to_le_bytes());
    out.extend_from_slice(&T_COST.to_le_bytes());
    out.extend_from_slice(&P_COST.to_le_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Unwrap a V1 file: verify the magic, derive the key, decrypt
/// the ciphertext, return the Bech32 plaintext. The KDF
/// parameters are read from the file header (so we can rotate
/// `M_COST` later without a migration), but the `KEY_LEN`,
/// `SALT_LEN`, and `NONCE_LEN` are fixed by the V1 spec.
pub fn unwrap(passphrase: &SecretString, file: &[u8]) -> Result<String> {
    if file.len() < HEADER_LEN + 16 {
        return Err(anyhow!(
            "wrapped key file is too short ({} bytes; need at least {})",
            file.len(),
            HEADER_LEN + 16
        ));
    }
    if &file[..MAGIC.len()] != MAGIC {
        return Err(anyhow!(
            "not a V1 wrapped key (bad magic); got {:?}",
            String::from_utf8_lossy(&file[..MAGIC.len().min(file.len())])
        ));
    }
    let mut off = MAGIC.len();
    let salt: [u8; SALT_LEN] = file[off..off + SALT_LEN].try_into().context("read salt")?;
    off += SALT_LEN;
    let m_cost = u32::from_le_bytes(file[off..off + 4].try_into().context("read m_cost")?);
    off += 4;
    let t_cost = u32::from_le_bytes(file[off..off + 4].try_into().context("read t_cost")?);
    off += 4;
    let p_cost = u32::from_le_bytes(file[off..off + 4].try_into().context("read p_cost")?);
    off += 4;
    let nonce: [u8; NONCE_LEN] = file[off..off + NONCE_LEN]
        .try_into()
        .context("read nonce")?;
    off += NONCE_LEN;
    let ct = &file[off..];

    // Sanity-bound the KDF params to prevent a maliciously-crafted
    // file from forcing us to allocate gigabytes of memory or run
    // a billion Argon2id iterations.
    if !(8 * 1024..=1024 * 1024).contains(&m_cost) {
        return Err(anyhow!(
            "m_cost {m_cost} KiB out of bounds (8 MiB ..= 1 GiB)"
        ));
    }
    if !(1..=10).contains(&t_cost) {
        return Err(anyhow!("t_cost {t_cost} out of bounds (1..=10)"));
    }
    if !(1..=8).contains(&p_cost) {
        return Err(anyhow!("p_cost {p_cost} out of bounds (1..=8)"));
    }

    let mut key = derive_key(
        passphrase.expose_secret().as_bytes(),
        &salt,
        m_cost,
        t_cost,
        p_cost,
    )?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
    let pt = cipher
        .decrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: ct,
                aad: MAGIC,
            },
        )
        .map_err(|_| anyhow!("wrong passphrase or corrupt wrapped key"));
    key.zeroize();
    let pt = pt?;
    String::from_utf8(pt).context("unwrapped plaintext is not valid UTF-8")
}

/// Is this file (or file contents) a V1 wrapped key? Cheap
/// check that only looks at the magic.
pub fn is_wrapped_v1(file: &[u8]) -> bool {
    file.len() >= MAGIC.len() && &file[..MAGIC.len()] == MAGIC
}

/// Derive a 32-byte AEAD key from `passphrase` and `salt` using
/// Argon2id with the given parameters.
fn derive_key(
    passphrase: &[u8],
    salt: &[u8],
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
) -> Result<[u8; KEY_LEN]> {
    let params = Params::new(m_cost, t_cost, p_cost, Some(KEY_LEN))
        .map_err(|e| anyhow!("Argon2id params: {e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; KEY_LEN];
    argon
        .hash_password_into(passphrase, salt, &mut out)
        .map_err(|e| anyhow!("Argon2id hash: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PASSPHRASE: &str = "correct horse battery staple";

    fn pp() -> SecretString {
        SecretString::new(TEST_PASSPHRASE.to_string().into())
    }

    /// A real `AGE-SECRET-KEY-1...` Bech32 string is 163
    /// characters + newline = 164 bytes (X25519 keys are always
    /// the same length). We use a stand-in string here for
    /// round-trip tests; the actual format check happens in the
    /// `age_secrets` round-trip test which generates a real key.
    const TEST_PLAINTEXT: &str = "AGE-SECRET-KEY-1AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n";

    #[test]
    fn round_trip() {
        let wrapped = wrap(&pp(), TEST_PLAINTEXT).expect("wrap");
        assert!(is_wrapped_v1(&wrapped));
        let recovered = unwrap(&pp(), &wrapped).expect("unwrap");
        assert_eq!(recovered, TEST_PLAINTEXT);
    }

    #[test]
    fn wrong_passphrase_fails() {
        let wrapped = wrap(&pp(), TEST_PLAINTEXT).expect("wrap");
        let wrong = SecretString::new("wrong passphrase".to_string().into());
        let result = unwrap(&wrong, &wrapped);
        assert!(result.is_err(), "wrong passphrase must fail; got Ok");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("wrong passphrase") || msg.contains("corrupt"),
            "expected clear error, got: {msg}"
        );
    }

    #[test]
    fn rejects_bad_magic() {
        let mut garbage = vec![0u8; HEADER_LEN + 32];
        garbage[..8].copy_from_slice(b"NOT-OURS");
        let result = unwrap(&pp(), &garbage);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("not a V1"), "expected magic error, got: {msg}");
    }

    #[test]
    fn rejects_truncated() {
        let wrapped = wrap(&pp(), TEST_PLAINTEXT).expect("wrap");
        let truncated = &wrapped[..wrapped.len() - 5];
        let result = unwrap(&pp(), truncated);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_tampered_ciphertext() {
        let mut wrapped = wrap(&pp(), TEST_PLAINTEXT).expect("wrap");
        // Flip one byte in the ciphertext region.
        let last = wrapped.len() - 1;
        wrapped[last] ^= 0xFF;
        let result = unwrap(&pp(), &wrapped);
        assert!(result.is_err(), "tampered ciphertext must fail AEAD auth");
    }

    #[test]
    fn rejects_oversized_m_cost() {
        // Build a hand-crafted header with m_cost = 2 GiB.
        let mut bad = Vec::new();
        bad.extend_from_slice(MAGIC);
        bad.extend_from_slice(&[0u8; SALT_LEN]);
        bad.extend_from_slice(&((2u32 * 1024 * 1024).to_le_bytes())); // 2 GiB > 1 GiB cap
        bad.extend_from_slice(&T_COST.to_le_bytes());
        bad.extend_from_slice(&P_COST.to_le_bytes());
        bad.extend_from_slice(&[0u8; NONCE_LEN]);
        bad.extend_from_slice(&[0u8; 32]);
        let result = unwrap(&pp(), &bad);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("m_cost"), "expected m_cost error, got: {msg}");
    }

    #[test]
    fn header_layout_matches_spec() {
        let wrapped = wrap(&pp(), TEST_PLAINTEXT).expect("wrap");
        // 25 magic + 16 salt + 4 m_cost + 4 t_cost + 4 p_cost +
        // 24 nonce + plaintext + 16 tag.
        let expected = HEADER_LEN + TEST_PLAINTEXT.len() + 16;
        assert_eq!(wrapped.len(), expected);
        assert_eq!(HEADER_LEN, 25 + 16 + 4 + 4 + 4 + 24);
        assert_eq!(MAGIC.len(), 25);
    }
}
