//! Engine ports surface: authorization + idempotency re-exports.
//! Implementations live outside the TCB (keel-policy, keel-postgres, memory).

// Authorization contract lives in keel-contracts (foundation): the security
// layer implements the port without depending on the engine (STAGE-0-SPEC §3).
pub use keel_contracts::{AuthorizationPort, AuthzDecision, AuthzRequest};

// Idempotency moved to its own engine module (S1 extraction); re-exports keep
// the historical `keel_execution::ports::Idempotency*` paths stable for adapters.
pub use crate::idempotency::{payload_hash, IdempotencyCheck, IdempotencyKey, IdempotencyPort};
