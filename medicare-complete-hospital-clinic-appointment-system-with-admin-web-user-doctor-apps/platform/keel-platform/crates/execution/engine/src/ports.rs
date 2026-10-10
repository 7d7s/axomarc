//! Engine ports surface: authorization + idempotency re-exports.
//! Implementations live outside the TCB (kelson-policy, kelson-postgres, memory).

// Authorization contract lives in kelson-contracts (foundation): the security
// layer implements the port without depending on the engine (STAGE-0-SPEC §3).
pub use kelson_contracts::{AuthorizationPort, AuthzDecision, AuthzRequest};

// Idempotency moved to its own engine module (S1 extraction); re-exports keep
// the historical `kelson_execution::ports::Idempotency*` paths stable for adapters.
pub use crate::idempotency::{payload_hash, IdempotencyCheck, IdempotencyKey, IdempotencyPort};
