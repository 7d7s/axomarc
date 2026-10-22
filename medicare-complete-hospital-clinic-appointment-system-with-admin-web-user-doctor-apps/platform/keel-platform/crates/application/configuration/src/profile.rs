//! The declarative profile layer.
//!
//! A **profile** is a named, versioned bundle of configuration that a
//! deployment selects with one variable: `KEEL_APP_PROFILE`. It is the
//! deployment's *preset* - the shape of "production" or "tenant-saas" or
//! "kiosk" - and it is a LAYER, not an override:
//!
//! ```text
//! profile  <  package defaults  <  config file  <  .env  <  environment
//! ```
//!
//! The profile file lives at `<config dir>/profiles/<name>.toml`, beside the
//! config file it refines.
//!
//! Two rules keep the layer honest:
//!
//! * every layer is schema-checked **on its own** before merging, so a
//!   profile can never smuggle an unknown key past
//!   `GRAPH-CONFIG-SCHEMA`, and the error names the PROFILE at fault
//!   rather than blaming the application's config;
//! * a profile that is named but **missing** refuses the load. A
//!   deployment that asked for a preset and silently got the default is
//!   worse than one that failed to boot.

use std::path::Path;

use kelson_foundation::AppError;

use crate::{schema_violations, CONFIG_SCHEMA_CODE};

/// The environment variable that selects the profile.
pub const PROFILE_ENV: &str = "KEEL_APP_PROFILE";

/// The directory (beside the config file) that holds the profiles.
pub const PROFILES_DIR: &str = "profiles";

/// The profile name an environment value selects.
///
/// The environment READ belongs to the configuration crate's boot path -
/// the one sanctioned place for it - so this only interprets the value.
/// A blank value selects nothing (an empty profile name is not a profile).
pub fn selected_from_env(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The path of a named profile, beside the config file.
pub fn profile_path(config_path: &Path, name: &str) -> Option<std::path::PathBuf> {
    let dir = config_path.parent()?.join(PROFILES_DIR);
    // The name is a single path segment: a profile name that could escape
    // the profiles directory is refused before it reaches the filesystem.
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains(':')
    {
        return None;
    }
    Some(dir.join(format!("{name}.toml")))
}

/// Merge the named profile UNDER `file_raw`.
///
/// No profile named (or no config file) returns the input untouched.
///
/// The name is passed IN: the environment read happens in the
/// configuration crate's boot path, so this module never touches the
/// process environment and a test can name a profile explicitly.
pub(crate) fn apply_named(
    config_path: &Path,
    name: Option<&str>,
    file_raw: String,
) -> Result<String, AppError> {
    // The file is parsed once here: the profile name may come from the
    // file's own `app.profile`, which is only known after the parse.
    let file_value: toml::Value = toml::from_str(&file_raw)
        .map_err(|e| AppError::validation(format!("the config file does not parse: {e}")))?;
    // The selector: the environment wins (a deployment overrides its own
    // file), and the file's `app.profile` is the fallback.
    let name = name
        .map(str::to_string)
        .or_else(|| {
            file_value
                .get("app")
                .and_then(|app| app.get("profile"))
                .and_then(|p| p.as_str())
                .map(str::to_string)
        });
    let Some(name) = name else {
        return Ok(file_raw);
    };
    let Some(path) = profile_path(config_path, &name) else {
        return Err(AppError::validation(format!(
            "the profile {name:?} is not a valid profile name (no path separators, no '..')"
        )));
    };
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        AppError::validation(format!(
            "the profile {name:?} was selected but its file could not be read ({}): {e}",
            path.display()
        ))
    })?;

    // The profile is schema-checked on its own: it never smuggles an
    // unknown key past GRAPH-CONFIG-SCHEMA.
    let violations = schema_violations(&raw);
    if !violations.is_empty() {
        return Err(AppError::validation(format!(
            "{}: profile {name}: {} unknown key(s): {}",
            CONFIG_SCHEMA_CODE,
            violations.len(),
            violations
                .iter()
                .map(|v| v.key.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    let profile: toml::Value = toml::from_str(&raw).map_err(|e| {
        AppError::validation(format!("the profile {name:?} does not parse ({}): {e}", path.display()))
    })?;
    let file = file_value;

    // The file wins per key: the profile fills only the keys the file
    // omits.
    toml::to_string(&crate::package_defaults::merge_file_over(
        &profile, file,
    ))
    .map_err(|e| AppError::validation(format!("the profile merge failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kelson-profile-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("config").join(PROFILES_DIR)).unwrap();
        dir
    }

    fn config_path(dir: &Path) -> PathBuf {
        dir.join("config").join("dev.toml")
    }

    #[test]
    fn the_profile_fills_the_keys_the_file_omits() {
        let dir = scratch("fill");
        std::fs::write(
            dir.join("config").join(PROFILES_DIR).join("saas.toml"),
            "[app]\nname = \"from-the-profile\"\nlisten = \"127.0.0.1:9000\"\n",
        )
        .unwrap();
        std::fs::write(
            config_path(&dir),
            "[app]\nname = \"from-the-file\"\n",
        )
        .unwrap();

        let merged = apply_named(
            &config_path(&dir),
            Some("saas"),
            std::fs::read_to_string(config_path(&dir)).unwrap(),
        )
        .unwrap();

        let value: toml::Value = toml::from_str(&merged).unwrap();
        assert_eq!(
            value["app"]["name"].as_str(),
            Some("from-the-file"),
            "the file wins per key"
        );
        assert_eq!(
            value["app"]["listen"].as_str(),
            Some("127.0.0.1:9000"),
            "the profile fills the keys the file omits"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_named_profile_that_is_missing_refuses_the_load() {
        let dir = scratch("missing");
        let err = apply_named(&config_path(&dir), Some("absent"), "[app]\nname = \"x\"\n".to_string())
            .expect_err("a missing profile refuses the load");
        assert!(
            err.public_message.contains("was selected but its file could not be read"),
            "{}",
            err.public_message
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_profile_with_an_unknown_key_is_refused() {
        let dir = scratch("unknown");
        std::fs::write(
            dir.join("config").join(PROFILES_DIR).join("bad.toml"),
            "[app]\nnam = \"typo\"\n",
        )
        .unwrap();
        let err = apply_named(&config_path(&dir), Some("bad"), "[app]\nname = \"x\"\n".to_string())
            .expect_err("an unknown key in a profile is refused");
        assert!(
            err.public_message.contains("unknown key"),
            "{}",
            err.public_message
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_file_can_name_its_own_profile() {
        let dir = scratch("self-named");
        std::fs::write(
            dir.join("config").join(PROFILES_DIR).join("saas.toml"),
            "[app]\nlisten = \"127.0.0.1:9000\"\n",
        )
        .unwrap();
        // The file names the profile itself: no environment variable.
        let raw = "[app]\nname = \"x\"\nprofile = \"saas\"\n".to_string();
        let merged = apply_named(&config_path(&dir), None, raw).unwrap();
        let value: toml::Value = toml::from_str(&merged).unwrap();
        assert_eq!(
            value["app"]["listen"].as_str(),
            Some("127.0.0.1:9000"),
            "the profile the file named filled the key the file omitted"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_explicit_name_wins_over_the_files_own() {
        let dir = scratch("explicit");
        std::fs::write(
            dir.join("config").join(PROFILES_DIR).join("one.toml"),
            "[app]\nlisten = \"127.0.0.1:9001\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("config").join(PROFILES_DIR).join("two.toml"),
            "[app]\nlisten = \"127.0.0.1:9002\"\n",
        )
        .unwrap();
        let raw = "[app]\nname = \"x\"\nprofile = \"one\"\n".to_string();
        let merged = apply_named(&config_path(&dir), Some("two"), raw).unwrap();
        let value: toml::Value = toml::from_str(&merged).unwrap();
        assert_eq!(
            value["app"]["listen"].as_str(),
            Some("127.0.0.1:9002"),
            "the explicit name wins over the file's own"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_profile_name_cannot_escape_the_profiles_directory() {
        let dir = scratch("escape");
        assert!(profile_path(&config_path(&dir), "../secrets").is_none());
        assert!(profile_path(&config_path(&dir), "a/b").is_none());
        assert!(profile_path(&config_path(&dir), "ok").is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_profile_selected_leaves_the_file_untouched() {
        let dir = scratch("none");
        let raw = "[app]\nname = \"x\"\n".to_string();
        let merged = apply_named(&config_path(&dir), None, raw.clone()).unwrap();
        assert_eq!(merged, raw);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The environment VALUE is interpreted here; the READ happens in the
    /// boot path (the sanctioned place). A blank value selects nothing.
    #[test]
    fn the_environment_value_is_interpreted_not_read() {
        assert_eq!(
            selected_from_env(Some("  production  ".to_string())),
            Some("production".to_string()),
            "the value is trimmed"
        );
        assert_eq!(selected_from_env(Some("   ".to_string())), None);
        assert_eq!(selected_from_env(Some(String::new())), None);
        assert_eq!(selected_from_env(None), None);
    }

    /// The effective profile is RECORDED on the config: the file that names
    /// its own profile keeps that name (it is not erased by the recording),
    /// and an operator can always see which preset the instance runs.
    #[test]
    fn a_self_named_profile_is_recorded_on_the_config() {
        let dir = scratch("recorded");
        std::fs::write(
            dir.join("config").join(PROFILES_DIR).join("saas.toml"),
            "[app]\nname = \"from-the-profile\"\n",
        )
        .unwrap();
        let path = config_path(&dir);
        std::fs::write(&path, "[app]\nname = \"x\"\nprofile = \"saas\"\n").unwrap();

        let cfg = crate::PlatformConfig::load(Some(&path)).expect("the config loads");
        assert_eq!(
            cfg.app.profile, "saas",
            "the effective profile is recorded, so an operator can see it"
        );
        assert_eq!(cfg.app.name, "x", "the file still wins per key");
        std::fs::remove_dir_all(&dir).ok();
    }
}
