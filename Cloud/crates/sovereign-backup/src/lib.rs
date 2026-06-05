//! Backup adapter. Stubbed in F1; F8 will implement Litestream-style WAL
//! streaming + daily snapshot + S3 offsite + restore drill.

#![deny(unsafe_code)]
#![allow(missing_docs)]

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
