// F4.6: `sovereign init` — framework detection + `app.yaml` writer.
//
// The V0 scanner reads up to 5 files in the current directory to
// pick a framework; explicit `--framework <name>` overrides the
// auto-detection. The output is a `app.yaml` that the F4 deploy
// use case knows how to consume (port, health_path, build/run cmd,
// optional `image`).
//
// The scanner is deliberately conservative: when in doubt it picks
// `Generic` and asks the operator for the build command in a
// follow-up comment. The goal is "sovereign init never silently
// picks the wrong framework," not "sovereign init is clever."

use std::path::Path;

use serde::Serialize;
use tracing::{info, warn};

use crate::cli::Framework;
use crate::commands::Dispatch;
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

/// The result of an `init` run. JSON-serializable so the `--format json`
/// envelope can carry it.
#[derive(Debug, Serialize)]
pub struct InitResult {
    pub detected_framework: Framework,
    pub app_yaml_path: std::path::PathBuf,
    pub app_name: String,
    pub port: u16,
    pub health_path: String,
    pub build_cmd: Option<String>,
    pub run_cmd: Option<String>,
    pub image: Option<String>,
    pub next_steps: Vec<String>,
}

/// Run `sovereign init`. `framework_override` is the clap
/// `--framework` value (None means auto-detect). `force` lets us
/// overwrite an existing `app.yaml`. `output` is the path to write
/// to (defaults to `./app.yaml` upstream).
pub async fn run(
    out: &Output,
    cwd: &Path,
    framework_override: Framework,
    force: bool,
    output: &Path,
    name: Option<String>,
) -> Dispatch {
    if output.exists() && !force {
        return err(
            out,
            AppExit::Usage,
            &format!(
                "{} already exists; re-run with --force to overwrite",
                output.display()
            ),
        );
    }

    let detected = match framework_override {
        Framework::Auto => detect(cwd),
        f => f,
    };
    info!(framework = ?detected, cwd = %cwd.display(), "framework chosen");

    let app_name = name.unwrap_or_else(|| {
        cwd.file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "app".into())
    });

    let spec = build_app_spec(detected, &app_name);
    let yaml = render_app_yaml(&spec, &app_name);

    if let Err(e) = std::fs::write(output, &yaml) {
        return err(
            out,
            AppExit::Upstream,
            &format!("cannot write {}: {e}", output.display()),
        );
    }
    info!(path = %output.display(), "wrote app.yaml");

    let result = InitResult {
        detected_framework: detected,
        app_yaml_path: output.to_path_buf(),
        app_name,
        port: spec.port,
        health_path: spec.health_path.clone(),
        build_cmd: spec.build_cmd.clone(),
        run_cmd: spec.run_cmd.clone(),
        image: spec.image.clone(),
        next_steps: next_steps_for(detected),
    };
    if out.format() == crate::output::Format::Text {
        let _ = out.ok(&format!(
            "wrote {} (framework={:?}, port={}, health={})",
            result.app_yaml_path.display(),
            result.detected_framework,
            result.port,
            result.health_path
        ));
        for line in &result.next_steps {
            let _ = out.ok(line);
        }
    } else {
        let env = Envelope::<InitResult>::ok(result);
        let _ = out.success(&env);
    }
    Dispatch::Ok
}

/// A framework-agnostic description of how to build / run / health-check
/// the app. `render_app_yaml` turns one of these into a YAML string.
#[derive(Debug, Clone)]
struct AppSpec {
    framework: Framework,
    port: u16,
    health_path: String,
    build_cmd: Option<String>,
    run_cmd: Option<String>,
    image: Option<String>,
    /// Extra YAML lines (framework-specific) to append after the
    /// core fields. Used to hint e.g. the build context or a custom
    /// start command without bloating the type with N optional
    /// `Option<String>` fields.
    extra: Vec<(String, String)>,
}

fn build_app_spec(framework: Framework, _app_name: &str) -> AppSpec {
    match framework {
        Framework::Fastapi => AppSpec {
            framework,
            port: 8000,
            health_path: "/health".into(),
            build_cmd: Some("pip install -r requirements.txt".into()),
            run_cmd: Some("uvicorn main:app --host 0.0.0.0 --port 8000".into()),
            image: Some("python:3.12-slim".into()),
            extra: vec![("python_version".into(), "3.12".into())],
        },
        Framework::Nextjs => AppSpec {
            framework,
            port: 3000,
            health_path: "/api/health".into(),
            build_cmd: Some("npm ci && npm run build".into()),
            run_cmd: Some("npm start -- -p 3000".into()),
            image: Some("node:20-alpine".into()),
            extra: vec![("node_version".into(), "20".into())],
        },
        Framework::Express => AppSpec {
            framework,
            port: 3000,
            health_path: "/healthz".into(),
            build_cmd: Some("npm ci".into()),
            run_cmd: Some("node server.js".into()),
            image: Some("node:20-alpine".into()),
            extra: vec![],
        },
        Framework::Go => AppSpec {
            framework,
            port: 8080,
            health_path: "/healthz".into(),
            build_cmd: Some("go build -o app .".into()),
            run_cmd: Some("./app".into()),
            image: Some("golang:1.22-alpine".into()),
            extra: vec![("go_version".into(), "1.22".into())],
        },
        Framework::Rails => AppSpec {
            framework,
            port: 3000,
            health_path: "/up".into(),
            build_cmd: Some("bundle install".into()),
            run_cmd: Some("bin/rails server -b 0.0.0.0 -p 3000".into()),
            image: Some("ruby:3.3-slim".into()),
            extra: vec![],
        },
        Framework::Laravel => AppSpec {
            framework,
            port: 8000,
            health_path: "/up".into(),
            build_cmd: Some("composer install --no-dev --optimize-autoloader".into()),
            run_cmd: Some("php artisan serve --host=0.0.0.0 --port=8000".into()),
            image: Some("php:8.3-cli".into()),
            extra: vec![],
        },
        Framework::Astro => AppSpec {
            framework,
            port: 4321,
            health_path: "/".into(),
            build_cmd: Some("npm ci && npm run build".into()),
            run_cmd: Some("node ./dist/server/entry.mjs".into()),
            image: Some("node:20-alpine".into()),
            extra: vec![],
        },
        Framework::Static => AppSpec {
            framework,
            port: 8080,
            health_path: "/".into(),
            build_cmd: None,
            run_cmd: Some("npx --yes serve -l 8080 .".into()),
            image: Some("node:20-alpine".into()),
            extra: vec![("static_dir".into(), ".".into())],
        },
        Framework::Generic => {
            warn!("no framework detected; emitting a Generic app.yaml — fill in build_cmd and run_cmd before deploy");
            AppSpec {
                framework,
                port: 8080,
                health_path: "/health".into(),
                build_cmd: None,
                run_cmd: None,
                image: Some("alpine:3.20".into()),
                extra: vec![],
            }
        }
        Framework::Auto => unreachable!("Auto is resolved to a concrete framework upstream"),
    }
}

fn render_app_yaml(spec: &AppSpec, app_name: &str) -> String {
    let mut s = String::new();
    s.push_str("# Generated by `sovereign init` — edit freely; the deploy use case reads\n");
    s.push_str("# the same fields regardless of how they got here.\n");
    s.push_str(&format!("name: {app_name}\n"));
    s.push_str(&format!("framework: {}\n", framework_str(spec.framework)));
    s.push_str(&format!("port: {}\n", spec.port));
    s.push_str(&format!("health_path: {}\n", spec.health_path));
    s.push_str("strategy: bluegreen\n");
    s.push_str("shutdown_grace_period: 10s\n");
    if let Some(image) = &spec.image {
        s.push_str(&format!("image: {image}\n"));
    } else {
        s.push_str("# image: ghcr.io/you/<app>:latest   # set before first deploy\n");
    }
    if let Some(cmd) = &spec.build_cmd {
        s.push_str(&format!("build_cmd: {cmd:?}\n"));
    } else {
        s.push_str("# build_cmd: \"npm ci && npm run build\"\n");
    }
    if let Some(cmd) = &spec.run_cmd {
        s.push_str(&format!("run_cmd: {cmd:?}\n"));
    } else {
        s.push_str("# run_cmd: \"./app\"\n");
    }
    for (k, v) in &spec.extra {
        s.push_str(&format!("{k}: {v:?}\n"));
    }
    s.push('\n');
    s
}

fn framework_str(f: Framework) -> &'static str {
    match f {
        Framework::Auto => "auto",
        Framework::Fastapi => "fastapi",
        Framework::Nextjs => "nextjs",
        Framework::Express => "express",
        Framework::Go => "go",
        Framework::Rails => "rails",
        Framework::Laravel => "laravel",
        Framework::Astro => "astro",
        Framework::Static => "static",
        Framework::Generic => "generic",
    }
}

/// Inspect `cwd` and pick a framework. The order of the checks
/// matters: more-specific signals (a real `package.json` with
/// `next` as a dep) win over less-specific ones (just a
/// `package.json`). The first match wins.
fn detect(cwd: &Path) -> Framework {
    if let Some(fw) = detect_python(cwd) {
        return fw;
    }
    if let Some(fw) = detect_node(cwd) {
        return fw;
    }
    if let Some(fw) = detect_go(cwd) {
        return fw;
    }
    if let Some(fw) = detect_ruby(cwd) {
        return fw;
    }
    if let Some(fw) = detect_php(cwd) {
        return fw;
    }
    if cwd.join("index.html").is_file() {
        return Framework::Static;
    }
    Framework::Generic
}

fn detect_python(cwd: &Path) -> Option<Framework> {
    let pyproject = std::fs::read_to_string(cwd.join("pyproject.toml")).ok()?;
    // Cheap-and-cheerful: if the file mentions `fastapi` somewhere,
    // assume FastAPI. The full dependency-parser is V0.5.
    if pyproject.to_lowercase().contains("fastapi") {
        return Some(Framework::Fastapi);
    }
    None
}

fn detect_node(cwd: &Path) -> Option<Framework> {
    let pkg = std::fs::read_to_string(cwd.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&pkg).ok()?;
    let deps = node_deps(&v);
    if deps.iter().any(|d| d == "next") {
        Some(Framework::Nextjs)
    } else if deps.iter().any(|d| d == "express") {
        Some(Framework::Express)
    } else if deps.iter().any(|d| d == "astro") {
        Some(Framework::Astro)
    } else {
        None
    }
}

fn node_deps(v: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(obj) = v.get("dependencies").and_then(|x| x.as_object()) {
        for k in obj.keys() {
            out.push(k.clone());
        }
    }
    if let Some(obj) = v.get("devDependencies").and_then(|x| x.as_object()) {
        for k in obj.keys() {
            out.push(k.clone());
        }
    }
    out
}

fn detect_go(cwd: &Path) -> Option<Framework> {
    if cwd.join("go.mod").is_file() {
        Some(Framework::Go)
    } else {
        None
    }
}

fn detect_ruby(cwd: &Path) -> Option<Framework> {
    let gemfile = std::fs::read_to_string(cwd.join("Gemfile")).ok()?;
    if gemfile.to_lowercase().contains("rails") {
        Some(Framework::Rails)
    } else {
        None
    }
}

fn detect_php(cwd: &Path) -> Option<Framework> {
    if cwd.join("composer.json").is_file() && cwd.join("artisan").is_file() {
        Some(Framework::Laravel)
    } else {
        None
    }
}

fn next_steps_for(fw: Framework) -> Vec<String> {
    let mut v = vec![
        format!(
            "Next: review {} and adjust port/health_path if needed.",
            "app.yaml"
        ),
        "Next: run `sovereign login` to provision the master key.".to_string(),
        "Next: run `sovereign deploy --app <NAME>` to ship.".to_string(),
    ];
    if matches!(fw, Framework::Generic) {
        v.insert(
            0,
            "No framework detected. Fill in `build_cmd` and `run_cmd` before deploy.".to_string(),
        );
    }
    v
}

fn err(out: &Output, code: AppExit, msg: &str) -> Dispatch {
    if out.format() == crate::output::Format::Text {
        let _ = out.err(msg);
    } else {
        let env = Envelope::<serde_json::Value>::err(code, serde_json::json!({"error": msg}), msg);
        let _ = out.error(&env);
    }
    Dispatch::Err(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sovereign-init-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_fastapi_via_pyproject() {
        let dir = fresh_dir();
        std::fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"x\"\ndependencies = [\"fastapi\", \"uvicorn\"]\n",
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Fastapi);
    }

    #[test]
    fn detects_nextjs_via_package_json() {
        let dir = fresh_dir();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"x","dependencies":{"next":"14.0.0"}}"#,
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Nextjs);
    }

    #[test]
    fn detects_express() {
        let dir = fresh_dir();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"x","dependencies":{"express":"4.18.0"}}"#,
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Express);
    }

    #[test]
    fn detects_go() {
        let dir = fresh_dir();
        std::fs::write(dir.join("go.mod"), "module x\n\ngo 1.22\n").unwrap();
        assert_eq!(detect(&dir), Framework::Go);
    }

    #[test]
    fn detects_rails_via_gemfile() {
        let dir = fresh_dir();
        std::fs::write(
            dir.join("Gemfile"),
            "source 'https://rubygems.org'\ngem 'rails'\n",
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Rails);
    }

    #[test]
    fn detects_static_via_index_html() {
        let dir = fresh_dir();
        std::fs::write(dir.join("index.html"), "<html></html>").unwrap();
        assert_eq!(detect(&dir), Framework::Static);
    }

    #[test]
    fn falls_back_to_generic() {
        let dir = fresh_dir();
        std::fs::write(dir.join("README.md"), "# nothing here").unwrap();
        assert_eq!(detect(&dir), Framework::Generic);
    }

    #[test]
    fn renders_yaml_with_required_fields() {
        let spec = build_app_spec(Framework::Fastapi, "api");
        let yaml = render_app_yaml(&spec, "api");
        assert!(yaml.contains("name: api"));
        assert!(yaml.contains("framework: fastapi"));
        assert!(yaml.contains("port: 8000"));
        assert!(yaml.contains("health_path: /health"));
        assert!(yaml.contains("build_cmd:"));
        assert!(yaml.contains("run_cmd:"));
    }

    #[test]
    fn generic_yaml_has_uncommented_placeholders() {
        let spec = build_app_spec(Framework::Generic, "x");
        let yaml = render_app_yaml(&spec, "x");
        assert!(yaml.contains("framework: generic"));
        assert!(yaml.contains("# build_cmd:"));
        assert!(yaml.contains("# run_cmd:"));
    }
}
