// F2: The Output formatter.
// Per docs/phase-00-mvp.md §F2 sub-tasks 3-4, and docs/product-ux.md §2
// (the 8 CLI non-negotiables), §6 (NO_COLOR), §8 (telemetry — what to
// instrument, what to forbid).
//
// The contract:
//   - text: human-readable, colored when stdout is a TTY, plain when piped
//   - json: structured JSON, one envelope per command
//   - color: respects --no-color, NO_COLOR env var (https://no-color.org),
//            and TTY auto-detection (only when stdout is a TTY)
//   - errors: the same JSON envelope is used for success + failure so agents
//             can parse a single shape
//   - the formatter NEVER prints PII, secret values, or content from the
//     audit log (per product-ux.md §8.2)

use crate::exit::AppExit;
use clap::ValueEnum;
use serde::Serialize;
use std::io::{self, IsTerminal, Write};

/// The output format. `Auto` resolves to `Text` if stdout is a TTY, else `Json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Auto,
    Text,
    Json,
}

impl Format {
    /// Resolve `Auto` to the actual format based on whether stdout is a TTY.
    pub fn resolve(self) -> Format {
        match self {
            Format::Auto => {
                if io::stdout().is_terminal() {
                    Format::Text
                } else {
                    Format::Json
                }
            }
            f => f,
        }
    }
}

/// The single envelope shape for JSON output. Used for both success and
/// failure so agents have one schema to parse.
#[derive(Debug, Serialize)]
pub struct Envelope<T: Serialize> {
    /// "ok" or "error"
    pub status: &'static str,
    /// The exit code (0 for success, non-zero for failure)
    pub exit_code: u8,
    /// The data payload (command-specific)
    pub data: T,
    /// Optional human-readable message (text-mode only; agents should rely on `data`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The ISO-8601 timestamp
    pub ts: String,
}

impl<T: Serialize> Envelope<T> {
    /// Build a success envelope.
    pub fn ok(data: T) -> Self {
        Self {
            status: "ok",
            exit_code: 0,
            data,
            message: None,
            ts: now_iso8601(),
        }
    }

    /// Build an error envelope.
    pub fn err(exit: AppExit, data: T, message: impl Into<String>) -> Self {
        Self {
            status: "error",
            exit_code: exit as u8,
            data,
            message: Some(message.into()),
            ts: now_iso8601(),
        }
    }
}

/// The output sink. Text mode writes to stdout, JSON mode writes to stdout
/// (one envelope per line). The formatter is the single place that decides
/// "is this human or agent?"
pub struct Output {
    format: Format,
    color: bool,
}

impl Output {
    /// Build an output sink. `format = Auto` is resolved against the TTY.
    /// `color` follows `--no-color`, `NO_COLOR=1`, and TTY auto-detection.
    pub fn new(format: Format, no_color: bool) -> Self {
        let format = format.resolve();
        let color =
            !no_color && io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        Self { format, color }
    }

    /// The resolved format.
    pub fn format(&self) -> Format {
        self.format
    }

    /// Print text-mode output.
    pub fn text(&self, msg: &str) -> io::Result<()> {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        handle.write_all(msg.as_bytes())?;
        handle.write_all(b"\n")?;
        Ok(())
    }

    /// Print a colored line. Color is applied only if `Output::color` is true.
    pub fn text_colored(&self, color: Color, msg: &str) -> io::Result<()> {
        if self.color {
            self.text(&format!("{}{}\x1b[0m", color.ansi(), msg))
        } else {
            self.text(msg)
        }
    }

    /// Print a green checkmark.
    pub fn ok(&self, msg: &str) -> io::Result<()> {
        self.text_colored(Color::Green, &format!("✓ {msg}"))
    }

    /// Print a yellow warning.
    pub fn warn(&self, msg: &str) -> io::Result<()> {
        self.text_colored(Color::Yellow, &format!("⚠ {msg}"))
    }

    /// Print a red error.
    pub fn err(&self, msg: &str) -> io::Result<()> {
        self.text_colored(Color::Red, &format!("✗ {msg}"))
    }

    /// Print a JSON envelope. Always goes to stdout, regardless of mode.
    pub fn json_envelope<T: Serialize>(&self, env: &Envelope<T>) -> io::Result<()> {
        let json = serde_json::to_string_pretty(env)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        handle.write_all(json.as_bytes())?;
        handle.write_all(b"\n")?;
        Ok(())
    }

    /// Print a success envelope. In text mode, prints the human message
    /// (or a default); in JSON mode, prints the envelope.
    pub fn success<T: Serialize>(&self, env: &Envelope<T>) -> io::Result<()> {
        match self.format {
            Format::Text => {
                if let Some(msg) = &env.message {
                    self.text(msg)?;
                } else {
                    self.text(&format!("[ok] {}", env.ts))?;
                }
            }
            Format::Json | Format::Auto => {
                self.json_envelope(env)?;
            }
        }
        Ok(())
    }

    /// Print an error. In text mode, the error goes to stderr; in json mode,
    /// the envelope goes to stdout.
    pub fn error<T: Serialize>(&self, env: &Envelope<T>) -> io::Result<()> {
        match self.format {
            Format::Text => {
                let stderr = io::stderr();
                let mut handle = stderr.lock();
                if let Some(msg) = &env.message {
                    handle.write_all(b"error: ")?;
                    handle.write_all(msg.as_bytes())?;
                    handle.write_all(b"\n")?;
                } else {
                    handle.write_all(b"error: (no message)\n")?;
                }
            }
            Format::Json | Format::Auto => {
                self.json_envelope(env)?;
            }
        }
        Ok(())
    }
}

/// ANSI color codes. The 4 colors we use: green (ok), yellow (warn), red (err),
/// blue (info). The codes are applied as `<color>...<reset>` around the
/// message, so the output is correct on any terminal that supports the
/// 16-color ANSI palette.
#[derive(Debug, Clone, Copy)]
pub enum Color {
    Green,
    Yellow,
    Red,
    Blue,
    Cyan,
    Gray,
}

impl Color {
    fn ansi(self) -> &'static str {
        match self {
            Color::Green => "\x1b[32m",
            Color::Yellow => "\x1b[33m",
            Color::Red => "\x1b[31m",
            Color::Blue => "\x1b[34m",
            Color::Cyan => "\x1b[36m",
            Color::Gray => "\x1b[90m",
        }
    }
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day, hour, min, sec) = epoch_to_ymdhms(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn epoch_to_ymdhms(epoch_secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    // Pure-Rust conversion avoiding `chrono` for F2 (F3 will use chrono for the audit log).
    // Algorithm from Howard Hinnant's date.h — public domain.
    let days = (epoch_secs / 86400) as i64;
    let secs_of_day = (epoch_secs % 86400) as u32;
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y } as i32;
    (year, m, d, hour, min, sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_zero_is_1970_01_01() {
        assert_eq!(epoch_to_ymdhms(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn epoch_one_day_is_1970_01_02() {
        assert_eq!(epoch_to_ymdhms(86_400), (1970, 1, 2, 0, 0, 0));
    }

    #[test]
    fn epoch_2026_06_04_14_22() {
        // 2026-06-04 14:22:00 UTC = 1_780_582_920
        // (day 20_608 from epoch + 14*3600 + 22*60 = 1_780_531_200 + 51_720)
        let (y, m, d, h, mi, s) = epoch_to_ymdhms(1_780_582_920);
        assert_eq!(y, 2026);
        assert_eq!(m, 6);
        assert_eq!(d, 4);
        assert_eq!(h, 14);
        assert_eq!(mi, 22);
        assert_eq!(s, 0);
    }

    #[test]
    fn format_resolve_text_on_tty() {
        // The test runner's stdout is not a TTY, so Auto -> Json.
        let f = Format::Auto.resolve();
        if io::stdout().is_terminal() {
            assert_eq!(f, Format::Text);
        } else {
            assert_eq!(f, Format::Json);
        }
    }

    #[test]
    fn format_resolve_explicit() {
        assert_eq!(Format::Text.resolve(), Format::Text);
        assert_eq!(Format::Json.resolve(), Format::Json);
    }
}
