//! Caddy reverse-proxy adapter. Stubbed in F1; F6 will implement the
//! `ProxyPort` trait by talking to Caddy's admin API on `:2019`.

#![deny(unsafe_code)]
#![allow(missing_docs)]

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
