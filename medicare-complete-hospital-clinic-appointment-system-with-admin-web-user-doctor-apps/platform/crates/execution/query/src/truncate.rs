//! Typed TRUNCATE builder (Wave 3).
//!
//! TRUNCATE is structural and destructive, so the compiled form is gated:
//! [`Truncate::build_gated`] refuses outside dev/test. The builder itself
//! stays environment-agnostic — the Execution class cannot depend on
//! `keel-config` — so callers pass `cfg.app.env.as_str()` and the gate
//! matches the two allowed tokens (`dev`, `test`) fail-closed on anything
//! else.

use keel_foundation::AppError;

use crate::select::Ident;
use crate::CompiledWrite;

/// Builder for `TRUNCATE TABLE <ident>`.
#[derive(Debug, Clone)]
pub struct Truncate {
    table: Ident,
}

impl Truncate {
    pub fn table(table: Ident) -> Self {
        Self { table }
    }

    /// Compile the statement WITHOUT an environment gate. Use only behind a
    /// gate you own (tests, migrations teardown); production paths must call
    /// [`Truncate::build_gated`].
    pub fn build(&self) -> Result<CompiledWrite, AppError> {
        Ok(CompiledWrite {
            sql: format!("TRUNCATE TABLE {}", self.table.as_str()),
            params: Vec::new(),
        })
    }

    /// Compile only when `env` is a dev/test environment; staging, prod, and
    /// any unrecognized token are refused (fail closed).
    pub fn build_gated(&self, env: &str) -> Result<CompiledWrite, AppError> {
        match env {
            "dev" | "test" => self.build(),
            other => Err(AppError::denied(format!(
                "TRUNCATE refused outside dev/test environment (env {other:?})"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: &str) -> Ident {
        Ident::new(raw).unwrap()
    }

    #[test]
    fn truncate_shape() {
        let q = Truncate::table(id("audit_logs")).build().unwrap();
        assert_eq!(q.sql, "TRUNCATE TABLE audit_logs");
        assert!(q.params.is_empty());
    }

    #[test]
    fn truncate_ident_is_validated() {
        assert!(Ident::new("audit_logs; DROP TABLE users").is_err());
        assert!(Truncate::table(Ident::new("a.b").unwrap()).build().is_ok());
    }

    #[test]
    fn truncate_gate_allows_dev_and_test_only() {
        let t = Truncate::table(id("sessions"));
        assert!(t.build_gated("dev").is_ok());
        assert!(t.build_gated("test").is_ok());

        for env in ["staging", "prod", "production", "", "DEV"] {
            let err = t.build_gated(env).unwrap_err();
            assert!(
                err.public_message.contains("dev/test"),
                "env {env:?} must be refused, got: {}",
                err.public_message
            );
        }
    }
}
