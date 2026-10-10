//! Console exporter: rendered lines to stdout (json or pretty), with
//! line-level redaction + CRLF sanitization.

use std::io::Write;

use kelson_telemetry::{Event, ExportError, ExportOk, Exporter};

use crate::format::{event_to_jsonl, event_to_pretty, redact_line};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConsoleFormat {
    #[default]
    Json,
    Pretty,
}

pub struct ConsoleExporter {
    pub format: ConsoleFormat,
    write: Box<dyn Fn(&str) + Send + Sync>,
}

impl ConsoleExporter {
    pub fn stdout(format: ConsoleFormat) -> Self {
        Self {
            format,
            write: Box::new(|line: &str| {
                let mut out = std::io::stdout().lock();
                let _ = writeln!(out, "{line}");
            }),
        }
    }

    /// Test seam: capture rendered lines.
    pub fn with_writer(format: ConsoleFormat, write: Box<dyn Fn(&str) + Send + Sync>) -> Self {
        Self { format, write }
    }
}

impl Exporter for ConsoleExporter {
    fn name(&self) -> &str {
        "console"
    }

    fn export(&self, batch: &[Event]) -> Result<ExportOk, ExportError> {
        let mut bytes = 0usize;
        for event in batch {
            let raw = match self.format {
                ConsoleFormat::Json => event_to_jsonl(event),
                ConsoleFormat::Pretty => event_to_pretty(event),
            };
            let line = redact_line(&raw);
            bytes += line.len() + 1;
            (self.write)(&line);
        }
        Ok(ExportOk {
            bytes,
            events: batch.len(),
        })
    }
}
