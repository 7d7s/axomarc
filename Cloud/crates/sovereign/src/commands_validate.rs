//! `sovereign validate` — wraps `app_yaml::validate_path` and
//! pretty-prints the result on the shared `Output` sink.

use std::path::Path;

use crate::app_yaml;
use crate::commands::Dispatch;
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

/// Run `sovereign validate <path>`. The CLI prints one error per
/// line in text mode (or a single JSON envelope in `--json` mode).
/// Returns `Dispatch::Err(AppExit::Usage)` on validation failure
/// so CI hooks can detect the bad file via `$?` == 2.
pub async fn run(out: &Output, path: &Path) -> Dispatch {
    match app_yaml::validate_path(path) {
        Ok(cfg) => {
            if matches!(out.format(), crate::output::Format::Text) {
                let _ = out.ok(&format!(
                    "{} is valid (framework={}, port={}, health={})",
                    path.display(),
                    cfg.framework,
                    cfg.port,
                    cfg.health_path
                ));
            } else {
                let env = Envelope::<serde_json::Value>::ok(serde_json::json!({
                    "path": path,
                    "config": cfg,
                }));
                let _ = out.success(&env);
            }
            Dispatch::Ok
        }
        Err(errs) => {
            if matches!(out.format(), crate::output::Format::Text) {
                for e in &errs {
                    let _ = out.err(e);
                }
            } else {
                let env = Envelope::<serde_json::Value>::err(
                    AppExit::Usage,
                    serde_json::json!({"path": path, "errors": errs}),
                    errs.join("; "),
                );
                let _ = out.error(&env);
            }
            Dispatch::Err(AppExit::Usage)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{Format, Output};
    use std::io::Write;

    /// Write `content` to a `NamedTempFile` and return its path.
    /// The file handle is leaked (`mem::forget`) so the path stays
    /// valid for the duration of the test; the OS reclaims the file
    /// when the test process exits.
    fn write_yaml(content: &str) -> (std::path::PathBuf, tempfile::NamedTempFile) {
        let f = tempfile::NamedTempFile::new().expect("named temp");
        let p = f.path().to_path_buf();
        let mut fh = f.reopen().expect("reopen");
        fh.write_all(content.as_bytes()).expect("write");
        (p, f)
    }

    #[tokio::test]
    async fn validates_good_yaml() {
        let (path, _f) = write_yaml(
            r#"
name: my-api
framework: fastapi
port: 8000
health_path: /health
strategy: bluegreen
shutdown_grace_period: 30s
image: python:3.12-slim
run_cmd: uvicorn main:app --host 0.0.0.0 --port 8000
"#,
        );
        let out = Output::new(Format::Text, false);
        let d = run(&out, &path).await;
        assert!(matches!(d, crate::commands::Dispatch::Ok));
    }

    #[tokio::test]
    async fn prints_per_line_errors() {
        let (path, _f) = write_yaml(
            r#"
name: My_Bad_Name
framework: unknownframework
port: 8080
health_path: nope
strategy: sideways
shutdown_grace_period: zero
image: "::not a ref::"
"#,
        );
        let out = Output::new(Format::Text, false);
        let d = run(&out, &path).await;
        assert!(matches!(d, crate::commands::Dispatch::Err(AppExit::Usage)));
    }

    #[tokio::test]
    async fn reports_missing_file() {
        let path = std::path::PathBuf::from("/nonexistent/app.yaml");
        let out = Output::new(Format::Text, false);
        let d = run(&out, &path).await;
        assert!(matches!(d, crate::commands::Dispatch::Err(AppExit::Usage)));
    }
}
