//! Wave 8 B-24/B-25 (docs/19): boot-time package config defaults.
//!
//! A package listed in the root `packages.toml` index may ship a
//! `[config]` table in its `module.toml` manifest, and the operator may
//! materialize an editable copy at `config/packages/<name>.toml`
//! (`keel config:publish`). At boot the layers merge UNDER the app config
//! file (documented layer: package `[config]` <- published copy <- config
//! file <- .env <- environment): the config file wins per key, the
//! published copy wins over the package's own manifest defaults, and
//! among packages the FIRST indexed entry wins. Every layer is
//! schema-checked on its own before merging, so a package (or an edit to
//! its published copy) never smuggles a key past GRAPH-CONFIG-SCHEMA and
//! the error always names the layer at fault.

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
        let mut pkg_defaults = toml::Value::Table(toml::map::Map::new());
        let mut any = false;
        if let Some(config) = manifest.config.as_ref() {
            if !config.is_table() {
                return Err(AppError::validation(format!(
                    "package {}: [config] must be a table",
                    entry.name
                )));
            }
            // Schema-check the manifest defaults themselves, fail-closed: a
            // package never smuggles an unknown key past GRAPH-CONFIG-SCHEMA,
            // and the error names the package rather than blaming the app's
            // config file.
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
            merge_first_wins(&mut pkg_defaults, config);
            any = true;
        }
        // Wave 8 B-25: the published copy (`keel config:publish`) overlays
        // the package's manifest defaults — the operator's edits win per
        // key, still under the app config file. It is schema-checked on its
        // own so a typo names the file the operator edited.
        let published = root
            .join("config")
            .join("packages")
            .join(format!("{}.toml", entry.name));
        if published.is_file() {
            let raw = std::fs::read_to_string(&published).map_err(|e| {
                AppError::validation(format!("config read {}: {e}", published.display()))
            })?;
            let violations = schema_violations(&raw);
            if !violations.is_empty() {
                return Err(AppError::validation(format!(
                    "{}: {} unknown key(s) in {}: {}",
                    CONFIG_SCHEMA_CODE,
                    violations.len(),
                    published.display(),
                    violations
                        .iter()
                        .map(|v| v.key.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            let published_value: toml::Value = raw.parse().map_err(|e| {
                AppError::validation(format!("config parse {}: {e}", published.display()))
            })?;
            pkg_defaults = merge_file_over(&pkg_defaults, published_value);
            any = true;
        }
        if any {
            merge_first_wins(&mut defaults, &pkg_defaults);
            shipped = true;
        }
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
