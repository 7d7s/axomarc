//! # kelson-tracing
//! Sentinel (S5) span-side primitives: W3C `traceparent` parse/serialize over
//! kelson-foundation typed ids, and the deterministic adaptive sampling ladder.

pub mod sampler;
pub mod traceparent;

pub use sampler::{escalation, EscalationSignals, RateTier, ERROR_RATE_ELEVATED, ERROR_RATE_FULL, LATENCY_RATIO_ELEVATED, LATENCY_RATIO_FULL, QUEUE_FILL_FULL};
pub use traceparent::{make_traceparent, parse_traceparent, Traceparent};
