//! Wave 8 B-24 (docs/19): boot-time package config defaults.
//!
//! A package listed in the root `packages.toml` index may ship a
//! `[config]` table in its `module.toml` manifest. At boot those tables
//! merge UNDER the app config file (documented layer: package `[config]`
//! <- config file <- .env <- environment), so a package supplies defaults
//! without being published or hand-edited into the host app. Precedence is
//! per key: the file always wins, keys only packages declare fill in, and
//! among packages the FIRST indexed entry wins. Defaults are
//! schema-checked on their own before merging, so a package never smuggles
//! a key past GRAPH-CONFIG-SCHEMA.

use std::path::Path;

use keel_foundation::AppError;

use crate::{schema_violations, CONFIG_SCHEMA_CODE};

/// Merge every indexed package's `[config]` defaults under `file_raw`.
/// No index (or no `[config]` sections) returns the file untouched.
pub(crate) fn apply(config_path: &Path, file_raw: String) -> Result<String, AppError> {
    // The config file lives at <root>/config/<file>.toml by convention;
    // the package index sits at <root>/packages.toml beside config/.
    let Some(root) = config_path.parent().and_then(|dir| dir.parent()) else {
        return Ok(file_raw);
    };
    let index = keel_registry::PackageIndex::load(root)
        .map_err(|e| AppError::validation(format!("package index: {e}")))?;
    let mut defaults = toml::Value::Table(toml::map::Map::new());
    let mut shipped = false;
    for entry in &index.package {
        let manifest = index
            .entry_manifest(root, entry)
            .map_err(|e| AppError::validation(format!("package config: {e}")))?;
        let Some(config) = manifest.config.as_ref() else {
            continue;
        };
        if !config.is_table() {
            return Err(AppError::validation(format!(
                "package {}: [config] must be a table",
                entry.name
            )));
        }
        // Schema-check the defaults themselves, fail-closed: a package
        // never smuggles an unknown key past GRAPH-CONFIG-SCHEMA, and the
        // error names the package rather than blaming the app's config file.
        let raw = toml::to_string(config).map_err(|e| {
            AppError::validation(format!("package {}: [config] serialize: {e}", entry.name))
        })?;
        let violations = schema_violations(&raw);
        if !violations.is_empty() {
            return Err(AppError::validation(format!(
                "{}: package {}: {} unknown key(s) in [config]: {}",
                CONFIG_SCHEMA_CODE,
                entry.name,
                violations.len(),
                violations
                    .iter()
                    .map(|v| v.key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        merge_first_wins(&mut defaults, config);
        shipped = true;
    }
    if !shipped {
        return Ok(file_raw);
    }
    let file: toml::Value = file_raw
        .parse()
        .map_err(|e| AppError::validation(format!("config parse failed: {e}")))?;
    let merged = merge_file_over(&defaults, file);
    toml::to_string(&merged)
        .map_err(|e| AppError::validation(format!("config merge serialize: {e}")))
}

/// Across packages the FIRST indexed entry wins: an existing key is never
/// overwritten; tables merge recursively.
fn merge_first_wins(acc: &mut toml::Value, incoming: &toml::Value) {
    let (Some(acc), toml::Value::Table(incoming)) = (acc.as_table_mut(), incoming) else {
        return;
    };
    for (key, value) in incoming {
        match acc.get_mut(key) {
            Some(existing) => merge_first_wins(existing, value),
            None => {
                acc.insert(key.clone(), value.clone());
            }
        }
    }
}

/// The config file wins over package defaults per key, recursively; keys
/// only packages declare fill in.
fn merge_file_over(defaults: &toml::Value, file: toml::Value) -> toml::Value {
    match (defaults, &file) {
        (toml::Value::Table(d), toml::Value::Table(f)) => {
            let mut out = d.clone();
            for (key, value) in f {
                match out.get_mut(key) {
                    Some(existing) => *existing = merge_file_over(existing, value.clone()),
                    None => {
                        out.insert(key.clone(), value.clone());
                    }
                }
            }
            toml::Value::Table(out)
        }
        (_, _) => file,
    }
}
