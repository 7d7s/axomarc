//! Row probes for the database rules (`unique`/`exists`, Wave 4 Phase 4).
//!
//! The trait pair lives in the foundation so the validator evaluates DB
//! rules without depending on the data layer: the HTTP edge adapts the
//! module-registered `model.probes` service (kelson-models) behind
//! [`DbProbeLookup`] and attaches it to the [`crate::ValidationContext`].
//!
//! Failure model (docs/19 P4): a store error — or a probe that was never
//! registered for the model — is an `Err`, never a silent pass. The edge
//! renders it as a `DB_ERROR` 503 envelope, so a down database refuses the
//! request instead of letting validation "succeed" against no rows.

use kelson_foundation::TenantId;
use std::sync::Arc;

/// One model's membership probe (implemented over the module's
/// `ModelStore` by the `model.probes` service).
pub trait DbProbe: Send + Sync {
    /// `Ok(true)` when at least one live row matches `column = value` plus
    /// every literal `wheres` pair, scoped to `tenant` and excluding
    /// `ignore_id` when given. `Err(detail)` on any store failure — the
    /// detail is internal, the envelope is the public `DB_ERROR` message.
    fn has_row(
        &self,
        tenant: Option<TenantId>,
        column: &str,
        value: &str,
        wheres: &[(String, String)],
        ignore_id: Option<&str>,
    ) -> Result<bool, String>;
}

/// Model-name → probe lookup, wired from the edge's `model.probes`
/// registry. `None` for an unregistered model is a fail-closed error
/// (503), not a pass.
pub trait DbProbeLookup: Send + Sync {
    fn probe(&self, model: &str) -> Option<Arc<dyn DbProbe>>;
}
