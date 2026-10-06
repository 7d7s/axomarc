//! .env loading (S2 item 18, L03). Layering stays deterministic:
//!   defaults <- config file <- .env <- .env.<env> <- real environment
//! .env values only fill variables that are not already set, so the real
//! environment ALWAYS wins over any .env file. Files live next to the config
//! file (config/.env, config/.env.prod, ...).

use std::path::{Path, PathBuf};

use keel_foundation::AppError;

/// Parse .env content: KEY=VALUE per line, optional quotes, export prefix,
/// comments, CRLF tolerant. Malformed lines are skipped, never fatal.
pub fn parse_dotenv(raw: &str) -> Vec<(String, String)> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim().trim_matches('\u{feff}').trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let value = unquote(value.trim());
        out.push((key.to_string(), value));
    }
    out
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            let inner = &value[1..value.len() - 1];
            return inner.to_string();
        }
    }
    // Unquoted: strip inline comments and trailing whitespace.
    match value.split_once(" #") {
        Some((head, _)) => head.trim().to_string(),
        None => value.to_string(),
    }
}

/// Load one .env file into the process environment (unset vars only).
/// Returns the number of variables actually applied.
pub fn load_dotenv_file(path: &Path) -> Result<usize, AppError> {
    if !path.exists() {
        return Ok(0);
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|e| AppError::validation(format!("cannot read {}: {e}", path.display())))?;
    let mut applied = 0usize;
    for (key, value) in parse_dotenv(&raw) {
        if std::env::var(&key).is_err() {
            // SAFETY: .env loading runs at boot, before any threads read env.
            std::env::set_var(&key, value);
            applied += 1;
        }
    }
    Ok(applied)
}

/// Load config/.env (always) — first dotenv layer.
pub fn load_dotenv(config_dir: &Path) -> Result<usize, AppError> {
    load_dotenv_file(&config_dir.join(".env"))
}

/// Load config/.env.<env> after the environment is known — second dotenv layer.
pub fn load_dotenv_for_env(config_dir: &Path, env_name: &str) -> Result<usize, AppError> {
    load_dotenv_file(&config_dir.join(format!(".env.{env_name}")))
}

/// The directory a config path lives in (for locating .env siblings).
pub fn config_dir_of(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_shapes() {
        let raw = "﻿NAME=keel
# comment
export KEEL_FLAG=on
QUOTED=\"hello world\"
SINGLE='keep #hash'
INLINE=value # trailing

BAD LINE
=novalue
";
        let pairs = parse_dotenv(raw);
        let get = |k: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("NAME").unwrap(), "keel");
        assert_eq!(get("KEEL_FLAG").unwrap(), "on");
        assert_eq!(get("QUOTED").unwrap(), "hello world");
        assert_eq!(get("SINGLE").unwrap(), "keep #hash");
        assert_eq!(get("INLINE").unwrap(), "value");
        assert!(get("BAD LINE").is_none());
    }

    #[test]
    fn quoted_values_keep_interior() {
        let pairs = parse_dotenv("K=\"a=b #notcomment\"\n");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1, "a=b #notcomment");
    }

    #[test]
    fn load_applies_only_unset_vars() {
        let dir = std::env::temp_dir().join(format!("keel-dotenv-{}", uuid_like()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".env"),
            "KEEL_DOTENV_TEST_A=from-file
KEEL_DOTENV_TEST_B=from-file
",
        )
        .unwrap();
        std::env::set_var("KEEL_DOTENV_TEST_B", "real-env");
        let applied = load_dotenv(&dir).unwrap();
        assert_eq!(applied, 1);
        assert_eq!(std::env::var("KEEL_DOTENV_TEST_A").unwrap(), "from-file");
        assert_eq!(std::env::var("KEEL_DOTENV_TEST_B").unwrap(), "real-env");
        std::env::remove_var("KEEL_DOTENV_TEST_A");
        std::env::remove_var("KEEL_DOTENV_TEST_B");
        std::fs::remove_dir_all(&dir).ok();
    }

    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string()
    }
}
