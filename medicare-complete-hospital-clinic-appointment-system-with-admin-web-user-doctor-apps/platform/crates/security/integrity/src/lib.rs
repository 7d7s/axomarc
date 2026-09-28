//! # keel-integrity
//!
//! Sentinel (S5) audit-integrity primitives, placed in the security layer
//! (D-74) so the data layer (`keel-postgres`) can compute chain hashes
//! inside the engine transaction even though it must not depend on the
//! observability class. Verification tooling (`keel observe audit verify`)
//! lives in the CLI, which may depend on both layers.
//!
//! - [`chain`] — sha256 prev-hash chaining anchored per `(tenant, day)`
//!   segment, canonical row serialization, chain verification.
//! - [`hmac`] — manual HMAC-SHA256 for optional segment-head signatures
//!   keyed by the application key.

pub mod chain;
pub mod hmac;

pub use chain::{
    audit_hash, canonical_audit_row, canonical_json, chain_id_for, verify_chain, BreakReason,
    ChainBreak, ChainRow, GENESIS_HASH,
};
pub use hmac::{hmac_sha256, sign_chain_head, verify_chain_head};
