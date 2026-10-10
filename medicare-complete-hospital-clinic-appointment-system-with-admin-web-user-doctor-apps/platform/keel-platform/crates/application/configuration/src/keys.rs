//! Application key generation (S2 item 18): `kelson key:generate` support.
//! The app key is 32 random bytes, rendered Laravel-style as base64:<64 chars>.
//! Production asserts a non-placeholder key at boot (see assert_secrets).

use rand::RngCore;

const KEY_BYTES: usize = 32;
const DEV_PLACEHOLDER: &str = "base64:dev-only-not-for-prod";

/// Generate a fresh application key: base64:<32 random bytes>.
pub fn generate_app_key() -> String {
    let mut bytes = [0u8; KEY_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("base64:{}", base64_encode(&bytes))
}

/// Dev placeholder written by scaffolds; never valid in prod.
pub fn dev_placeholder_key() -> &'static str {
    DEV_PLACEHOLDER
}

pub fn is_placeholder(key: &str) -> bool {
    key.is_empty() || key.contains("dev-only") || key == DEV_PLACEHOLDER
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_key_shape() {
        let key = generate_app_key();
        let payload = key.strip_prefix("base64:").unwrap();
        assert_eq!(payload.len(), 44); // 32 bytes -> 44 base64 chars
        assert!(payload
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='));
    }

    #[test]
    fn keys_are_unique() {
        assert_ne!(generate_app_key(), generate_app_key());
    }

    #[test]
    fn base64_roundtrip_sample() {
        assert_eq!(base64_encode(b"kelson"), "a2Vsc29u");
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"abc"), "YWJj");
        assert_eq!(base64_encode(b"ab"), "YWI=");
    }

    #[test]
    fn placeholder_detection() {
        assert!(is_placeholder(""));
        assert!(is_placeholder(DEV_PLACEHOLDER));
        assert!(is_placeholder("base64:some-dev-only-key"));
        assert!(!is_placeholder("base64:aRealKeyMaterial"));
    }
}
