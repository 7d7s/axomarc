//! kernel-core: shared contracts for every module and kernel crate.
//! Spec: docs/rust/10-STRUCTURE-AND-GUIDELINES §3-§4. No IO, no axum, no sqlx here.
pub mod clock;
pub mod context;
pub mod delta;
pub mod error;
pub mod ids;
pub mod module;

pub use clock::*;
pub use context::*;
pub use delta::*;
pub use error::*;
pub use ids::*;
pub use module::*;