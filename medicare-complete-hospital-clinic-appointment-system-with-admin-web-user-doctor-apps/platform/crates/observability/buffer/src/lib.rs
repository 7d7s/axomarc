//! # keel-buffer
//! Sentinel (S5) buffering: bounded ingest queue with counted overflow,
//! backpressure shed ladder, and a checksummed disk-backed WAL with watermark
//! replay (durability classes are defined in `keel-telemetry::envelope`).

pub mod backpressure;
pub mod memqueue;
pub mod wal;

pub use backpressure::{decide, shed_level, DropDecision, ShedLevel};
pub use memqueue::BoundedQueue;
pub use wal::{Wal, WalConfig, WalRecord, WalStats};
