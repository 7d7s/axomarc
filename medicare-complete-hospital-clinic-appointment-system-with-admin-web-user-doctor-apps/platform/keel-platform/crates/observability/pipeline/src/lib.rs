//! # keel-pipeline
//! Sentinel (S5) telemetry processing: declarative stage chain (normalize →
//! classify → redact → fingerprint → sample → dedupe → correlate → route),
//! built-in processors, the restricted-grammar policy engine (no arbitrary
//! code; evaluation is total) and the category route table.

pub mod graph;
pub mod policy;
pub mod processors;
pub mod router;

pub use graph::PipelineGraph;
pub use policy::{
    parse_expr, Action, CmpOp, Expr, Field, PolicyDecision, PolicyEngine, PolicyFacts, PolicyRule,
    Value,
};
pub use processors::{
    Classify, Correlate, Dedupe, Fingerprint, Normalize, Redact, Sample, DEFAULT_MAX_FIELDS,
    DEFAULT_MAX_MESSAGE_LEN,
};
pub use router::RouteTable;
