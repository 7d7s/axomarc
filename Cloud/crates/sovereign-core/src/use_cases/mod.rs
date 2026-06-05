// The use cases. V0 ships the deploy + health + rollback use cases;
// backup, secrets, etc. land in their respective feature phases.
//
// Every use case takes `&AppState` (the DI container from
// `sovereign_core::state`) so the same code runs in the CLI, the
// TUI, and (V1) the control-plane HTTP server.

pub mod backup;
pub mod deploy;
pub mod health;
pub mod rollback;
pub mod secret;
