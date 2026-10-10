//! Logging system (S1): one init point per process, json/pretty/file outputs,
//! sensitive-field redaction on every line, request spans.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kelson_foundation::{AppError, RequestId, TraceId};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

/// Output format for logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// One JSON object per line (production default).
    #[default]
    Json,
    /// Human-readable (dev).
    Pretty,
}

/// Process logging posture. Built by the host from config; defaults are safe.
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// `EnvFilter` directive string, e.g. `info`, `kelson_api=debug,info`.
    pub level: String,
    pub format: LogFormat,
    /// Optional file mirror (append; parent dir must exist).
    pub file: Option<PathBuf>,
    /// Extra field names to redact (on top of the kernel default set).
    pub extra_redactions: Vec<String>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            format: LogFormat::Json,
            file: None,
            extra_redactions: Vec::new(),
        }
    }
}

impl LogConfig {
    pub fn pretty() -> Self {
        Self {
            format: LogFormat::Pretty,
            ..Self::default()
        }
    }
    pub fn with_level(mut self, level: impl Into<String>) -> Self {
        self.level = level.into();
        self
    }
    pub fn with_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file = Some(path.into());
        self
    }
}

/// Kernel default sensitive fields (never logged in clear).
pub const DEFAULT_REDACTIONS: &[&str] = &[
    "password",
    "secret",
    "jwt_secret",
    "authorization",
    "cookie",
    "token",
    "api_key",
    "otp",
    "ssn",
    "credit_card",
    "phi",
];

fn redaction_fields(config: &LogConfig) -> Arc<Vec<String>> {
    let mut fields: Vec<String> = DEFAULT_REDACTIONS
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    for f in &config.extra_redactions {
        let lower = f.to_lowercase();
        if !fields.contains(&lower) {
            fields.push(lower);
        }
    }
    Arc::new(fields)
}

/// Redact sensitive `key: value` pairs in one formatted log line.
/// Handles both JSON (`"password":"x"`) and pretty (`password="x"` /
/// `token=abc`) shapes. Values become `[REDACTED]`.
pub fn redact_str(line: &str, fields: &[String]) -> String {
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let mut matched: Option<usize> = None; // field length if we hit key start here
                                               // Key must start at a boundary: line start, '{', ',', '"', space, or '('-adjacent.
        let start_key =
            |pos: usize| pos == 0 || matches!(bytes[pos - 1], b'{' | b',' | b'"' | b' ' | b'(');
        if start_key(i) {
            for f in fields {
                let end = i + f.len();
                if end < bytes.len()
                    && bytes
                        .get(i..end)
                        .is_some_and(|b| b.eq_ignore_ascii_case(f.as_bytes()))
                {
                    let after = bytes[end];
                    let colon_form = after == b':'
                        || (after == b'"' && end + 1 < bytes.len() && bytes[end + 1] == b':');
                    let eq_form = after == b'=';
                    if colon_form || eq_form {
                        matched = Some(f.len());
                        break;
                    }
                }
            }
        }
        match matched {
            Some(len) => {
                // Emit the key exactly as-is.
                out.push_str(&line[i..i + len]);
                i += len;
                // Copy the separator portion (": / =" plus optional quote before ':').
                while i < bytes.len() && matches!(bytes[i], b':' | b' ' | b'"' | b'=') {
                    out.push(bytes[i] as char);
                    i += 1;
                    if bytes[i - 1] == b'='
                        || (bytes[i - 1] == b':' && !matches!(bytes.get(i), Some(b' ')))
                    {
                        break;
                    }
                }
                // Mask the value.
                if i < bytes.len() && bytes[i] == b'"' {
                    // quoted value: skip to closing quote
                    out.push_str("\"[REDACTED]\"");
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1; // consume closing quote
                    }
                } else {
                    // bare value: up to whitespace / ',' / '}'
                    out.push_str("[REDACTED]");
                    while i < bytes.len() && !matches!(bytes[i], b' ' | b',' | b'}' | b')' | b'\n')
                    {
                        i += 1;
                    }
                }
            }
            None => {
                // Copy one whole char so multi-byte UTF-8 survives intact.
                let ch = line[i..].chars().next().expect("i is a char boundary");
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}

/// Line-buffered redacting writer; mirrors to stdout + optional file.
pub struct LineRedactor {
    buffer: Vec<u8>,
    file: Option<Arc<Mutex<std::fs::File>>>,
    fields: Arc<Vec<String>>,
}

impl Write for LineRedactor {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        while let Some(pos) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&line).into_owned();
            let redacted = redact_str(&text, &self.fields);
            let mut stdout = std::io::stdout();
            stdout.write_all(redacted.as_bytes())?;
            if let Some(file) = &self.file {
                if let Ok(mut f) = file.lock() {
                    let _ = f.write_all(redacted.as_bytes());
                }
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buffer.is_empty() {
            let text = String::from_utf8_lossy(&self.buffer).into_owned();
            self.buffer.clear();
            let redacted = redact_str(&text, &self.fields);
            std::io::stdout().write_all(redacted.as_bytes())?;
            if let Some(file) = &self.file {
                if let Ok(mut f) = file.lock() {
                    let _ = f.write_all(redacted.as_bytes());
                }
            }
        }
        Ok(())
    }
}

impl Drop for LineRedactor {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

#[derive(Clone)]
struct LogWriters {
    file: Option<Arc<Mutex<std::fs::File>>>,
    fields: Arc<Vec<String>>,
}

impl<'a> MakeWriter<'a> for LogWriters {
    type Writer = LineRedactor;
    fn make_writer(&'a self) -> Self::Writer {
        LineRedactor {
            buffer: Vec::new(),
            file: self.file.clone(),
            fields: self.fields.clone(),
        }
    }
}

/// Install the global tracing subscriber (json or pretty; stdout + optional
/// file mirror; redaction always on). Once per process.
pub fn init_logging(config: &LogConfig) -> Result<(), AppError> {
    let filter = EnvFilter::try_new(&config.level).map_err(|e| {
        AppError::validation(format!(
            "invalid log level directive {:?}: {e}",
            config.level
        ))
    })?;
    let file = match &config.file {
        Some(path) => Some(Arc::new(Mutex::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| {
                    AppError::internal(format!("cannot open log file {}: {e}", path.display()))
                })?,
        ))),
        None => None,
    };
    let writers = LogWriters {
        file,
        fields: redaction_fields(config),
    };
    let result = match config.format {
        LogFormat::Json => tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_writer(writers)
            .try_init(),
        LogFormat::Pretty => tracing_subscriber::fmt()
            .pretty()
            .with_env_filter(filter)
            .with_writer(writers)
            .try_init(),
    };
    result.map_err(|e| AppError::internal(format!("logging already initialized: {e}")))
}

/// HTTP/transport request span (S1): joins request → execution via trace_id.
pub fn request_span(
    request_id: RequestId,
    trace_id: TraceId,
    method: &str,
    path: &str,
) -> tracing::Span {
    tracing::info_span!(
        "request",
        request_id = %request_id,
        trace_id = %trace_id,
        method = %method,
        path = %path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> Vec<String> {
        DEFAULT_REDACTIONS
            .iter()
            .map(|s| s.to_lowercase())
            .collect()
    }

    #[test]
    fn redacts_json_values() {
        let line = r#"{"level":"INFO","password":"hunter2","jwt_secret":"abc.def","user":"alice"}"#;
        let out = redact_str(line, &fields());
        assert!(out.contains(r#""password":"[REDACTED]""#), "got: {out}");
        assert!(out.contains(r#""jwt_secret":"[REDACTED]""#), "got: {out}");
        assert!(out.contains(r#""user":"alice""#));
        assert!(!out.contains("hunter2"));
    }

    #[test]
    fn redacts_pretty_values() {
        let line = r#"request started password="hunter2" token=abc123 user=alice"#;
        let out = redact_str(line, &fields());
        assert!(out.contains(r#"password="[REDACTED]""#), "got: {out}");
        assert!(out.contains("token=[REDACTED]"), "got: {out}");
        assert!(out.contains("user=alice"));
        assert!(!out.contains("hunter2"));
        assert!(!out.contains("abc123"));
    }

    #[test]
    fn redaction_is_case_insensitive_on_keys() {
        let out = redact_str("{\"Password\":\"x\"}", &fields());
        assert!(out.contains("[REDACTED]"), "got: {out}");
        assert!(!out.contains("\"x\""));
    }

    #[test]
    fn non_sensitive_lines_pass_through() {
        let line = r#"{"message":"booted","services":3}"#;
        assert_eq!(redact_str(line, &fields()), line);
    }

    #[test]
    fn multibyte_lines_survive_redaction() {
        // Regression: byte-wise scanning panicked on multi-byte characters ('\u{2026}').
        let line = "slow statement \u{2026} summary: \"insert into credentials (id, \u{2026})\"";
        assert_eq!(redact_str(line, &fields()), line);
        let mixed = "caf\u{e9} password=hunter2 done \u{2026} \u{5b8c}\u{4e86}";
        let out = redact_str(mixed, &fields());
        assert!(out.contains("password=[REDACTED]"), "got: {out}");
        assert!(!out.contains("hunter2"));
        assert!(out.starts_with("caf\u{e9} "), "got: {out}");
        assert!(
            out.ends_with(" done \u{2026} \u{5b8c}\u{4e86}"),
            "got: {out}"
        );
    }

    #[test]
    fn config_extras_merge_with_defaults() {
        let cfg = LogConfig {
            extra_redactions: vec!["employee_id".into()],
            ..LogConfig::default()
        };
        let fields = redaction_fields(&cfg);
        assert!(fields.contains(&"password".to_string()));
        assert!(fields.contains(&"employee_id".to_string()));
    }
}
