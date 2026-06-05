//! Encrypted secret store. Stubbed in F1; F7 will implement envelope
//! encryption with `age` + Argon2id. The master key never leaves the box.

#![deny(unsafe_code)]
#![allow(missing_docs)]

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
