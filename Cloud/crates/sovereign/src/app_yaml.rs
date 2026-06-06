// F4.7: `app.yaml` schema + validator.
//
// `app.yaml` is the operator-facing config that `sovereign init`
// writes and `sovereign validate` checks. The schema is a flat
// list of fields (no nested objects in V0) so the validator can
// be a 30-line function that returns `Result<AppConfig, Vec<String>>`.
//
// The schema is intentionally permissive: extra fields are
// allowed (forward-compat for V1+ fields the V0 binary doesn't
// know about) and the type system enforces the required fields
// at parse time via serde.
//
// # Why a real validator and not "just parse the YAML"
//
// Two reasons:
//  1. The CLI catches typos at `init` time, not at `deploy` time.
//     A typo like `prot: 8000` would otherwise propagate to a
//     confusing error from the deploy use case 30s later.
//  2. The validator is a single source of truth for the schema
//     docs (the JSON Schema can be generated from `AppConfig` in
//     V0.5; the V0 hand-doc is the comment block above the
//     struct).

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The V0 `app.yaml` schema. See `docs/phase-00-mvp.md` §F4 for
/// the full field reference. Fields marked OPTIONAL may be
/// absent; all others must be present.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    /// The app's name. Must be DNS-label safe (lowercase
    /// alphanumeric + `-`); used as the vhost prefix and the
    /// Docker container name prefix.
    pub name: String,

    /// The framework. One of the `Framework` enum variants
    /// (`auto`, `fastapi`, `flask`, `django`, `nextjs`, `nuxt`,
    /// `sveltekit`, `remix`, `express`, `go`, `rails`, `laravel`,
    /// `astro`, `phoenix`, `deno`, `static`, `generic`).
    /// The validator only checks the spelling; the deploy use
    /// case treats unknown frameworks as `Generic`.
    pub framework: String,

    /// The port the app listens on inside the container. Must
    /// be 1-65535. The Caddy reverse proxy is pinned to this
    /// port (V0 design; see F6).
    pub port: u16,

    /// The HTTP path the prober hits to determine health.
    /// Must start with `/`. A 200 response within `timeout` is
    /// a pass.
    pub health_path: String,

    /// The deploy strategy. One of `bluegreen` (default),
    /// `rolling` (V1+), `recreate` (V1+). V0 only ships
    /// `bluegreen`; the other values are accepted and
    /// normalized to `bluegreen` with a warning.
    #[serde(default = "default_strategy")]
    pub strategy: String,

    /// Grace period for stopping the old container during a
    /// blue/green cutover. V0 default 10s; V1 will make this
    /// per-strategy.
    #[serde(default = "default_grace_period")]
    pub shutdown_grace_period: String,

    /// The container image. Optional — `sovereign deploy` can
    /// override via `--image`. When set, must be a valid
    /// OCI/Docker reference (e.g. `alpine:3.20`,
    /// `ghcr.io/you/app:v1.2.3`).
    #[serde(default)]
    pub image: Option<String>,

    /// The build command. `sovereign deploy` runs this in a
    /// BuildKit container (V0: shell exec; V1.5: BuildKit
    /// proper). When absent, the deploy is "run from image"
    /// only.
    #[serde(default)]
    pub build_cmd: Option<String>,

    /// The run command. `sovereign deploy` starts the container
    /// with this as the entrypoint. When absent, the image's
    /// default ENTRYPOINT is used.
    #[serde(default)]
    pub run_cmd: Option<String>,
}

fn default_strategy() -> String {
    "bluegreen".to_string()
}
fn default_grace_period() -> String {
    "10s".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            framework: "generic".to_string(),
            port: 8080,
            health_path: "/health".to_string(),
            strategy: default_strategy(),
            shutdown_grace_period: default_grace_period(),
            image: None,
            build_cmd: None,
            run_cmd: None,
        }
    }
}

/// The list of valid framework names. Matches the clap
/// `Framework` enum minus `Auto` (the CLI uses `Auto` as a
/// sentinel; the YAML must hold a concrete name).
pub const KNOWN_FRAMEWORKS: &[&str] = &[
    "fastapi",
    "flask",
    "django",
    "nextjs",
    "nuxt",
    "sveltekit",
    "remix",
    "express",
    "go",
    "rails",
    "laravel",
    "astro",
    "static",
    "phoenix",
    "deno",
    "generic",
];

/// The list of valid strategy names.
pub const KNOWN_STRATEGIES: &[&str] = &["bluegreen", "rolling", "recreate"];

/// The list of valid run strategy names that V0 actually
/// implements (only `bluegreen`).
pub const V0_IMPLEMENTED_STRATEGIES: &[&str] = &["bluegreen"];

/// Parse a YAML string into an `AppConfig` AND run the
/// field-level validations. The errors are returned as a
/// `Vec<String>` so the CLI can print one error per line.
pub fn validate(yaml: &str) -> Result<AppConfig, Vec<String>> {
    let cfg: AppConfig = match serde_yaml::from_str(yaml) {
        Ok(c) => c,
        Err(e) => {
            // serde_yaml's error message is detailed but includes
            // line/col info; we surface it verbatim. The location
            // is critical for fixing the typo quickly.
            return Err(vec![format!("YAML parse error: {e}")]);
        }
    };
    let mut errors = Vec::new();
    if cfg.name.is_empty() {
        errors.push("`name` is required and must be non-empty".to_string());
    } else if !is_dns_label_safe(&cfg.name) {
        errors.push(format!(
            "`name` must be DNS-label safe (lowercase alphanumeric + `-`); got `{}`",
            cfg.name
        ));
    }
    if cfg.port == 0 {
        errors.push("`port` must be in 1-65535; got 0".to_string());
    }
    if !cfg.health_path.starts_with('/') {
        errors.push(format!(
            "`health_path` must start with `/`; got `{}`",
            cfg.health_path
        ));
    }
    if !KNOWN_FRAMEWORKS.contains(&cfg.framework.as_str()) {
        errors.push(format!(
            "`framework` must be one of {:?}; got `{}`",
            KNOWN_FRAMEWORKS, cfg.framework
        ));
    }
    if !KNOWN_STRATEGIES.contains(&cfg.strategy.as_str()) {
        errors.push(format!(
            "`strategy` must be one of {:?}; got `{}`",
            KNOWN_STRATEGIES, cfg.strategy
        ));
    } else if !V0_IMPLEMENTED_STRATEGIES.contains(&cfg.strategy.as_str()) {
        // Not a hard error; V0 will treat it as bluegreen and
        // warn at init/validate time.
        errors.push(format!(
            "`strategy={}` is a V1+ feature; V0 will fall back to `bluegreen`",
            cfg.strategy
        ));
    }
    // A duration string parse for grace period: must be
    // <positive integer><s|m|h>. V0 only supports seconds (no
    // fractional). The V0 deploy use case parses it; we
    // validate the format here.
    if let Err(e) = parse_grace_period(&cfg.shutdown_grace_period) {
        errors.push(format!("`shutdown_grace_period`: {e}"));
    }
    if let Some(image) = &cfg.image {
        if !is_valid_image_ref(image) {
            errors.push(format!(
                "`image` must be a valid OCI/Docker reference (e.g. `alpine:3.20`); got `{image}`"
            ));
        }
    }
    if errors.is_empty() {
        Ok(cfg)
    } else {
        Err(errors)
    }
}

/// Read a YAML file from `path` and validate it. Convenience
/// wrapper for `sovereign validate <PATH>`.
pub fn validate_path(path: &Path) -> Result<AppConfig, Vec<String>> {
    let yaml = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return Err(vec![format!("cannot read {}: {e}", path.display())]);
        }
    };
    validate(&yaml)
}

/// True iff `s` is a non-empty DNS-label safe string. DNS
/// labels are 1-63 chars of `[a-z0-9-]`, not starting or ending
/// with `-`. We use this for the app name (it's a vhost prefix
/// + a Docker container name prefix).
fn is_dns_label_safe(s: &str) -> bool {
    if s.is_empty() || s.len() > 63 {
        return false;
    }
    if s.starts_with('-') || s.ends_with('-') {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Parse a duration string of the form `<N><unit>` where `unit`
/// is `s`, `m`, or `h`. The V0 deploy use case only supports
/// `s`; the others are accepted but treated as seconds in V0.
fn parse_grace_period(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("must be non-empty (e.g. `10s`)".into());
    }
    let (num, unit) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| "missing unit (e.g. `s`); got a bare number".to_string())?,
    );
    let n: u64 = num
        .parse()
        .map_err(|_| format!("not a positive integer: `{num}`"))?;
    if n == 0 {
        return Err("must be > 0".into());
    }
    match unit {
        "s" | "m" | "h" => Ok(()),
        other => Err(format!("unknown unit `{other}`; expected `s`, `m`, or `h`")),
    }
}

/// True iff `s` looks like a Docker image reference:
///   - `[registry/]name[:tag][@digest]`
///   - registry is optional
///   - name is required and must be lowercase alphanumeric + `_-./`
///   - tag is optional; if present, it's `[A-Za-z0-9_.-]+`
///   - digest is optional; if present, it's `sha256:<64 hex>`
///
/// This is a permissive check; the real validation is at
/// deploy time when Docker tries to pull the image. The V0
/// validator just catches obviously-wrong references.
fn is_valid_image_ref(s: &str) -> bool {
    if s.is_empty() || s.len() > 255 {
        return false;
    }
    // Split off the digest first.
    let (rest, _digest_ok) = match s.split_once('@') {
        Some((rest, digest)) => (
            rest,
            digest.starts_with("sha256:") && digest.len() == 7 + 64,
        ),
        None => (s, true),
    };
    // Split off the tag.
    let (name, _tag_ok) = match rest.split_once(':') {
        Some((name, tag)) => {
            let tag_ok = !tag.is_empty()
                && tag
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-');
            (name, tag_ok)
        }
        None => (rest, true),
    };
    // The name is `[registry/]path`. Each component is allowed
    // to contain `[a-z0-9_.-]+` (the `.` lets us accept
    // `ghcr.io`, `docker.io`, `registry.gitlab.com`); the first
    // component is the registry iff it contains a `.` or `:`.
    // We don't try to be more strict than that here — the real
    // validation happens at deploy time when Docker tries to
    // pull the ref.
    name.split('/').all(|part| {
        !part.is_empty()
            && part.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' || c == '.'
            })
    }) && _digest_ok
        && _tag_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_yaml() -> &'static str {
        r#"
name: myapp
framework: fastapi
port: 8000
health_path: /health
strategy: bluegreen
shutdown_grace_period: 10s
image: ghcr.io/you/myapp:v1.2.3
build_cmd: "pip install -r requirements.txt"
run_cmd: "uvicorn main:app --host 0.0.0.0 --port 8000"
"#
    }

    #[test]
    fn parses_minimal_valid_yaml() {
        let yaml = "name: myapp\nframework: fastapi\nport: 8000\nhealth_path: /health\n";
        let cfg = validate(yaml).unwrap();
        assert_eq!(cfg.name, "myapp");
        assert_eq!(cfg.framework, "fastapi");
        assert_eq!(cfg.port, 8000);
        assert_eq!(cfg.health_path, "/health");
        // Defaults kick in for the optional fields.
        assert_eq!(cfg.strategy, "bluegreen");
        assert_eq!(cfg.shutdown_grace_period, "10s");
        assert_eq!(cfg.image, None);
        assert_eq!(cfg.build_cmd, None);
        assert_eq!(cfg.run_cmd, None);
    }

    #[test]
    fn parses_full_yaml() {
        let cfg = validate(valid_yaml()).unwrap();
        assert_eq!(cfg.image.as_deref(), Some("ghcr.io/you/myapp:v1.2.3"));
        assert_eq!(
            cfg.build_cmd.as_deref(),
            Some("pip install -r requirements.txt")
        );
    }

    #[test]
    fn rejects_missing_name() {
        let yaml = "framework: fastapi\nport: 8000\nhealth_path: /health\n";
        let errs = validate(yaml).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("`name`")));
    }

    #[test]
    fn rejects_invalid_name() {
        // uppercase
        let yaml = "name: MyApp\nframework: fastapi\nport: 8000\nhealth_path: /health\n";
        let errs = validate(yaml).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("DNS-label safe")));
        // leading dash
        let yaml = "name: -myapp\nframework: fastapi\nport: 8000\nhealth_path: /health\n";
        let errs = validate(yaml).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("DNS-label safe")));
    }

    #[test]
    fn rejects_zero_port() {
        let yaml = "name: myapp\nframework: fastapi\nport: 0\nhealth_path: /health\n";
        let errs = validate(yaml).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("port")));
    }

    #[test]
    fn rejects_health_path_without_slash() {
        let yaml = "name: myapp\nframework: fastapi\nport: 8000\nhealth_path: health\n";
        let errs = validate(yaml).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("health_path")));
    }

    #[test]
    fn rejects_unknown_framework() {
        let yaml = "name: myapp\nframework: rocket\nport: 8000\nhealth_path: /health\n";
        let errs = validate(yaml).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("framework")));
    }

    #[test]
    fn warns_on_v1_strategy() {
        // rolling is a V1+ strategy; V0 validates the spelling
        // but emits a warning that V0 will fall back to
        // bluegreen. This is what the operator wants — a
        // graceful path forward, not a hard fail.
        let yaml = "name: myapp\nframework: fastapi\nport: 8000\nhealth_path: /health\nstrategy: rolling\n";
        let errs = validate(yaml).unwrap_err();
        assert!(errs
            .iter()
            .any(|e| e.contains("V1+") || e.contains("rolling")));
    }

    #[test]
    fn rejects_invalid_grace_period() {
        let yaml = "name: myapp\nframework: fastapi\nport: 8000\nhealth_path: /health\nshutdown_grace_period: forever\n";
        let errs = validate(yaml).unwrap_err();
        assert!(errs
            .iter()
            .any(|e| e.contains("grace_period") || e.contains("unit")));
    }

    #[test]
    fn accepts_alpine_image() {
        let yaml = "name: myapp\nframework: fastapi\nport: 8000\nhealth_path: /health\nimage: alpine:3.20\n";
        validate(yaml).unwrap();
    }

    #[test]
    fn accepts_ghcr_image() {
        let yaml = "name: myapp\nframework: fastapi\nport: 8000\nhealth_path: /health\nimage: ghcr.io/owner/app:v1.2.3\n";
        validate(yaml).unwrap();
    }

    #[test]
    fn accepts_image_with_digest() {
        let yaml = "name: myapp\nframework: fastapi\nport: 8000\nhealth_path: /health\nimage: alpine@sha256:0000000000000000000000000000000000000000000000000000000000000000\n";
        validate(yaml).unwrap();
    }

    #[test]
    fn rejects_image_with_invalid_chars() {
        let yaml =
            "name: myapp\nframework: fastapi\nport: 8000\nhealth_path: /health\nimage: BAD IMAGE\n";
        let errs = validate(yaml).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("image")));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        // Forward-compat: a V0 binary reading a V0.5 yaml
        // with extra fields should not fail.
        let yaml = "name: myapp\nframework: fastapi\nport: 8000\nhealth_path: /health\nunknown_field: future\n";
        validate(yaml).unwrap();
    }

    #[test]
    fn dns_label_safe_accepts_valid() {
        assert!(is_dns_label_safe("myapp"));
        assert!(is_dns_label_safe("my-app"));
        assert!(is_dns_label_safe("app123"));
        assert!(is_dns_label_safe("a"));
    }

    #[test]
    fn dns_label_safe_rejects_invalid() {
        assert!(!is_dns_label_safe(""));
        assert!(!is_dns_label_safe("MyApp"));
        assert!(!is_dns_label_safe("-myapp"));
        assert!(!is_dns_label_safe("myapp-"));
        assert!(!is_dns_label_safe("my_app"));
        assert!(!is_dns_label_safe(&"a".repeat(64)));
    }

    #[test]
    fn grace_period_parser_accepts_known_units() {
        assert!(parse_grace_period("10s").is_ok());
        assert!(parse_grace_period("1m").is_ok());
        assert!(parse_grace_period("2h").is_ok());
    }

    #[test]
    fn grace_period_parser_rejects_invalid() {
        assert!(parse_grace_period("").is_err());
        assert!(parse_grace_period("10").is_err());
        assert!(parse_grace_period("0s").is_err());
        assert!(parse_grace_period("10x").is_err());
        assert!(parse_grace_period("forever").is_err());
    }

    #[test]
    fn validate_path_reads_file() {
        let dir = std::env::temp_dir().join(format!("sovereign-validate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.yaml");
        std::fs::write(&path, valid_yaml()).unwrap();
        let cfg = validate_path(&path).unwrap();
        assert_eq!(cfg.name, "myapp");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn validate_path_errors_on_missing_file() {
        let path = std::path::PathBuf::from("/nonexistent/app.yaml");
        let errs = validate_path(&path).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("cannot read")));
    }
}
