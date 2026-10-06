//! Exception fingerprinting (Sentinel S5 §24): stable grouping keys so spikes
//! of the same failure collapse into one group instead of thousands of events.
//! Deterministic, dependency-light: sha256 over a normalized signature.

use sha2::{Digest, Sha256};

/// Maximum frames folded into a fingerprint.
pub const MAX_FINGERPRINT_FRAMES: usize = 5;

/// Normalize one signature component: lowercase, collapse whitespace, replace
/// digit runs with `#` (ids/ports/line numbers stop splitting groups).
pub fn normalize_component(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut prev_ws = false;
    let mut in_digits = false;
    for ch in lower.chars() {
        if ch.is_ascii_digit() {
            if !in_digits {
                out.push('#');
                in_digits = true;
            }
            continue;
        }
        in_digits = false;
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
            continue;
        }
        prev_ws = false;
        out.push(ch);
    }
    out.trim().to_string()
}

/// Stable fingerprint: first 16 hex chars of sha256 over the normalized
/// `(type, message, top frames, route)` signature.
pub fn exception_fingerprint(
    type_name: &str,
    message: &str,
    frames: &[String],
    route: Option<&str>,
) -> String {
    let mut sig = String::new();
    sig.push_str(&normalize_component(type_name));
    sig.push('\n');
    sig.push_str(&normalize_component(message));
    for frame in frames.iter().take(MAX_FINGERPRINT_FRAMES) {
        sig.push('\n');
        sig.push_str(&normalize_component(frame));
    }
    if let Some(route) = route {
        sig.push('\n');
        sig.push_str(&normalize_component(route));
    }
    let digest = Sha256::digest(sig.as_bytes());
    let mut hex = String::with_capacity(16);
    for b in digest.iter().take(8) {
        hex.push_str(&format!("{b:02x}"));
    }
    format!("err_{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_strips_volatile_parts() {
        assert_eq!(
            normalize_component("Conn refused at 10.0.0.1:5432"),
            "conn refused at #.#.#.#:#"
        );
        assert_eq!(
            normalize_component("  multiple   spaces "),
            "multiple spaces"
        );
    }

    #[test]
    fn fingerprint_stable_across_ids_and_volatile() {
        let frames = vec!["order.rs:120 pay()".to_string()];
        let a = exception_fingerprint(
            "PaymentTimeout",
            "gateway 500 for order 42",
            &frames,
            Some("POST /orders"),
        );
        let b = exception_fingerprint(
            "PaymentTimeout",
            "gateway 500 for order 9999",
            &frames,
            Some("POST /orders"),
        );
        assert_eq!(a, b);
        let c = exception_fingerprint(
            "PaymentTimeout",
            "gateway 500 for order 9999",
            &frames,
            Some("POST /invoices"),
        );
        assert_ne!(a, c);
        assert!(a.starts_with("err_"));
    }
}
