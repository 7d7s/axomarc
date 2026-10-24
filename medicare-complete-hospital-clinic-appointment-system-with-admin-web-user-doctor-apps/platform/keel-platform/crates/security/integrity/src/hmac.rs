//! Manual HMAC-SHA256 over sha2 (no new external dependency; D-75 budget
//! discipline) for optional signing of chain segment heads keyed by the
//! application key (`app.key`).

use sha2::{Digest, Sha256};
use uuid::Uuid;

const BLOCK_LEN: usize = 64;

/// RFC 2104 HMAC with SHA-256. Keys longer than the block size are hashed
/// first; shorter keys are zero-padded.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block_key = [0u8; BLOCK_LEN];
    if key.len() > BLOCK_LEN {
        let hashed = Sha256::digest(key);
        block_key[..hashed.len()].copy_from_slice(&hashed);
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK_LEN];
    let mut opad = [0x5cu8; BLOCK_LEN];
    for index in 0..BLOCK_LEN {
        ipad[index] ^= block_key[index];
        opad[index] ^= block_key[index];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

/// Sign a chain segment head: `HMAC(app_key, chain_id || head_hash || rows_be)`.
pub fn sign_chain_head(
    app_key: &[u8],
    chain_id: &Uuid,
    head_hash: &[u8; 32],
    rows: u64,
) -> [u8; 32] {
    let mut message = Vec::with_capacity(16 + 32 + 8);
    message.extend_from_slice(chain_id.as_bytes());
    message.extend_from_slice(head_hash);
    message.extend_from_slice(&rows.to_be_bytes());
    hmac_sha256(app_key, &message)
}

/// Constant-time verification of a segment head signature.
pub fn verify_chain_head(
    app_key: &[u8],
    chain_id: &Uuid,
    head_hash: &[u8; 32],
    rows: u64,
    signature: &[u8; 32],
) -> bool {
    let expected = sign_chain_head(app_key, chain_id, head_hash, rows);
    constant_time_eq(&expected, signature)
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for index in 0..32 {
        diff |= a[index] ^ b[index];
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn known_vectors() {
        let mac = hmac_sha256(b"key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(
            hex(&mac),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
        // RFC 4231 test case 1.
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn long_key_hashed_first() {
        let mac = hmac_sha256(&[0xaa; 130], b"payload");
        assert_ne!(mac, [0u8; 32]);
        assert_eq!(mac, hmac_sha256(&[0xaa; 130], b"payload"));
    }

    #[test]
    fn head_signature_roundtrip() {
        let chain_id = Uuid::now_v7();
        let head = [7u8; 32];
        let signature = sign_chain_head(b"app-key", &chain_id, &head, 42);
        assert!(verify_chain_head(
            b"app-key", &chain_id, &head, 42, &signature
        ));
        assert!(!verify_chain_head(
            b"app-key", &chain_id, &head, 43, &signature
        ));
        assert!(!verify_chain_head(
            b"other-key",
            &chain_id,
            &head,
            42,
            &signature
        ));
    }
}
