//! # kelson-export
//! Sentinel (S5) export fabric: console, file (JSONL + rotation), OTLP/HTTP
//! JSON (D-75, no opentelemetry deps), generic HTTP and RFC5424 syslog
//! exporters, each wrappable in `ManagedExporter` for circuit breaking,
//! bounded retries under a global retry budget, and health snapshots.

pub mod breaker;
pub mod console;
pub mod file;
pub mod format;
pub mod http;
pub mod managed;
pub mod otlp;
pub mod retry;
pub mod syslog;

pub use breaker::{BreakerConfig, BreakerState, CircuitBreaker};
pub use console::{ConsoleExporter, ConsoleFormat};
pub use file::{FileExporter, FileExporterConfig, FileSink, RealFileSink};
pub use format::{event_to_jsonl, event_to_pretty, redact_line, sanitize_text};
pub use http::{HttpExporter, HttpExporterConfig};
pub use managed::{ExporterHealth, ManagedExporter};
pub use otlp::{OtlpExporter, OtlpExporterConfig};
pub use retry::{RetryBudget, RetryPolicy};
pub use syslog::SyslogExporter;
