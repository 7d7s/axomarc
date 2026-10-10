//! Public API registry (blueprint item 49, L70): every public framework API
//! owns a stable API-ID plus governance metadata (stability, introduced,
//! deprecated, replacement, breaking-version). `kelson upgrade` reads this
//! registry; CI gates breaking changes against it (G14).

use serde::Deserialize;

/// The registry source, compiled into the binary so tools and tests always
/// agree with the checked-in file.
pub const API_REGISTRY_TOML: &str = include_str!("../api-registry.toml");

/// One governed public API.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ApiEntry {
    /// Stable governance id, e.g. `AUTH-API-0042`.
    pub id: String,
    /// The public surface this entry governs.
    pub api: String,
    /// `stable` | `beta` | `experimental` | `deprecated`.
    pub stability: String,
    /// Framework version that introduced the API.
    pub introduced: String,
    /// Version that deprecated the API (only for deprecated entries).
    pub deprecated: Option<String>,
    /// Successor API (required together with deprecation).
    pub replacement: Option<String>,
    /// Next major version allowed to remove the API.
    pub breaking_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegistryFile {
    api: Vec<ApiEntry>,
}

/// Parse the compiled-in registry. Panics only on a corrupt checked-in file
/// (caught by `registry_is_valid` long before runtime).
pub fn load_registry() -> Vec<ApiEntry> {
    parse_registry(API_REGISTRY_TOML).expect("api-registry.toml is valid")
}

/// Parse registry TOML text (exposed for the CLI upgrade report).
pub fn parse_registry(raw: &str) -> Result<Vec<ApiEntry>, String> {
    let file: RegistryFile = toml::from_str(raw).map_err(|e| e.to_string())?;
    validate(&file.api).map_err(|e| e.to_string())?;
    Ok(file.api)
}

fn validate(entries: &[ApiEntry]) -> Result<(), String> {
    if entries.is_empty() {
        return Err("api registry is empty".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for entry in entries {
        if !seen.insert(entry.id.clone()) {
            return Err(format!("duplicate API-ID {}", entry.id));
        }
        let valid_shape = entry
            .id
            .rsplit_once("-API-")
            .map(|(prefix, serial)| {
                !prefix.is_empty()
                    && !serial.is_empty()
                    && serial.chars().all(|c| c.is_ascii_digit())
            })
            .unwrap_or(false);
        if !valid_shape {
            return Err(format!(
                "API-ID {} must look like PREFIX-API-0001",
                entry.id
            ));
        }
        if !matches!(
            entry.stability.as_str(),
            "stable" | "beta" | "experimental" | "deprecated"
        ) {
            return Err(format!(
                "{}: unknown stability {:?}",
                entry.id, entry.stability
            ));
        }
        if entry.stability == "deprecated" {
            if entry.deprecated.is_none() {
                return Err(format!(
                    "{}: deprecated without a deprecation version",
                    entry.id
                ));
            }
            if entry.replacement.is_none() && entry.breaking_version.is_none() {
                return Err(format!(
                    "{}: deprecated without replacement or breaking-version",
                    entry.id
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_valid() {
        let entries = load_registry();
        assert!(entries.len() >= 10, "registry covers the public surface");
        assert!(entries.iter().all(|e| !e.api.is_empty()));
    }

    #[test]
    fn deprecated_entries_carry_migration_data() {
        let raw = r#"
[[api]]
id = "DEMO-API-0001"
api = "demo.old"
stability = "deprecated"
introduced = "0.1"
deprecated = "0.9"
replacement = "DEMO-API-0002"
breaking_version = "1.0"

[[api]]
id = "DEMO-API-0002"
api = "demo.new"
stability = "stable"
introduced = "0.9"
"#;
        let entries = parse_registry(raw).expect("valid deprecated entry parses");
        let old = entries.iter().find(|e| e.id == "DEMO-API-0001").unwrap();
        assert_eq!(old.replacement.as_deref(), Some("DEMO-API-0002"));
        assert_eq!(old.breaking_version.as_deref(), Some("1.0"));
    }

    #[test]
    fn invalid_entries_is_rejected() {
        let raw = r#"
[[api]]
id = "bad-id"
api = "demo.old"
stability = "deprecated"
introduced = "0.1"
"#;
        let err = parse_registry(raw).unwrap_err();
        assert!(err.contains("bad-id"), "reports the offending id: {err}");
    }
}
