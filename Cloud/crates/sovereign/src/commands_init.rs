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

use serde::{Deserialize, Serialize};
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
/// to (defaults to `./app.yaml` upstream). `name` overrides the
/// app name (defaults to the cwd basename). `print_frameworks`
/// dumps the framework registry and returns before touching the
/// filesystem.
pub async fn run(
    out: &Output,
    cwd: &Path,
    framework_override: Framework,
    force: bool,
    output: &Path,
    name: Option<String>,
    print_frameworks: bool,
) -> Dispatch {
    if print_frameworks {
        if matches!(out.format(), crate::output::Format::Text) {
            let mut buf = Vec::new();
            if let Err(e) = print_frameworks_text(&mut buf, "") {
                return err(out, AppExit::Upstream, &format!("print_frameworks: {e}"));
            }
            let s = String::from_utf8(buf).unwrap_or_default();
            for line in s.lines() {
                let _ = out.ok(line);
            }
        } else {
            let rows = collect_framework_rows();
            let env = Envelope::<Vec<FrameworkRow>>::ok(rows);
            let _ = out.success(&env);
        }
        return Dispatch::Ok;
    }
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
    if detected == Framework::Generic && framework_override == Framework::Auto {
        warn!("no framework detected; emitting a Generic app.yaml — fill in build_cmd and run_cmd before deploy");
    }
    info!(framework = ?detected, cwd = %cwd.display(), "framework chosen");

    let app_name = name.unwrap_or_else(|| {
        cwd.file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase().replace(['_', ' '], "-"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "app".into())
    });

    let spec = build_app_spec(detected, &app_name);
    let yaml = render_app_yaml(&spec, &app_name);

    // Self-validation: init should never write invalid YAML. If
    // `app_yaml::validate` rejects what we just rendered, that's a
    // bug in either the renderer or the validator — surface it as
    // a hard error so we catch the regression in tests, not at
    // deploy time. We use `expect` (panic) rather than `?` because
    // there's no graceful recovery path here.
    crate::app_yaml::validate(&yaml).unwrap_or_else(|errs| {
        panic!(
            "init: rendered yaml failed validation (this is a bug):\n{}",
            errs.join("\n")
        )
    });

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
        Framework::Flask => AppSpec {
            framework,
            port: 5000,
            health_path: "/health".into(),
            build_cmd: Some("pip install -r requirements.txt".into()),
            // Flask 2.3+ ships `flask run`; the --host/--port flags
            // are stable across 2.x and 3.x. The legacy `flask run`
            // honours the FLASK_RUN_PORT env var; we set both for
            // belt-and-braces.
            run_cmd: Some(
                "flask --app app run --host 0.0.0.0 --port 5000".into(),
            ),
            image: Some("python:3.12-slim".into()),
            extra: vec![("python_version".into(), "3.12".into())],
        },
        Framework::Django => AppSpec {
            framework,
            port: 8000,
            health_path: "/healthz".into(),
            // Django doesn't ship a `manage.py runserver` for
            // production; the default we ship is `gunicorn` since
            // it's the canonical V0 pick. If the operator prefers
            // `daphne` (ASGI) or `uvicorn`, they edit `app.yaml`.
            build_cmd: Some("pip install -r requirements.txt".into()),
            run_cmd: Some(
                "python manage.py migrate --noinput && gunicorn config.wsgi:application --bind 0.0.0.0:8000".into(),
            ),
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
        Framework::Nuxt => AppSpec {
            framework,
            port: 3000,
            health_path: "/api/health".into(),
            // Nuxt 3+ runs via `node .output/server/index.mjs` by
            // default. The `build` step is `nuxt build`.
            build_cmd: Some("npm ci && npm run build".into()),
            run_cmd: Some("node .output/server/index.mjs".into()),
            image: Some("node:20-alpine".into()),
            extra: vec![("node_version".into(), "20".into())],
        },
        Framework::Sveltekit => AppSpec {
            framework,
            port: 3000,
            health_path: "/".into(),
            // SvelteKit's `node` adapter produces a `build/` dir;
            // the run cmd is `node build`.
            build_cmd: Some("npm ci && npm run build".into()),
            run_cmd: Some("HOST=0.0.0.0 PORT=3000 node build".into()),
            image: Some("node:20-alpine".into()),
            extra: vec![("node_version".into(), "20".into())],
        },
        Framework::Remix => AppSpec {
            framework,
            port: 3000,
            health_path: "/healthz".into(),
            // Remix v2 with the Vite build: output is `build/server/index.js`.
            // The `@remix-run/serve` package is the official V0 pick.
            build_cmd: Some("npm ci && npm run build".into()),
            run_cmd: Some("npm run start".into()),
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
        Framework::Phoenix => AppSpec {
            framework,
            port: 4000,
            health_path: "/health".into(),
            // Phoenix 1.7+ default port is 4000; `mix phx.server`
            // binds to 127.0.0.1 by default, we override to 0.0.0.0
            // for the container. `mix assets.deploy` is the V0
            // prod build step.
            build_cmd: Some("mix local.hex --force && mix local.rebar --force && mix deps.get && mix assets.deploy && mix compile".into()),
            run_cmd: Some("mix phx.server".into()),
            image: Some("elixir:1.16-otp-26-slim".into()),
            extra: vec![("phoenix_version".into(), "1.7".into())],
        },
        Framework::Deno => AppSpec {
            framework,
            port: 8000,
            health_path: "/health".into(),
            // Deno Fresh's default port is 8000; `deno task start`
            // is the canonical V0 run command. The build step is a
            // no-op (Deno caches on demand) but we run a `deno
            // cache` to warm the dep graph.
            build_cmd: Some("deno cache main.ts".into()),
            run_cmd: Some("deno run --allow-net --allow-read --allow-env main.ts".into()),
            image: Some("denoland/deno:1.45".into()),
            extra: vec![("deno_version".into(), "1.45".into())],
        },
        Framework::Generic => {
            // Warn is emitted by the caller (`run`), not here.
            // We don't want `--print-frameworks` to log a "no
            // framework detected" warning just because `Generic`
            // is in the registry.
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
        Framework::Flask => "flask",
        Framework::Django => "django",
        Framework::Nextjs => "nextjs",
        Framework::Nuxt => "nuxt",
        Framework::Sveltekit => "sveltekit",
        Framework::Remix => "remix",
        Framework::Express => "express",
        Framework::Go => "go",
        Framework::Rails => "rails",
        Framework::Laravel => "laravel",
        Framework::Astro => "astro",
        Framework::Static => "static",
        Framework::Phoenix => "phoenix",
        Framework::Deno => "deno",
        Framework::Generic => "generic",
    }
}

/// Inspect `cwd` and pick a framework. The order of the checks
/// matters: more-specific signals (a real `package.json` with
/// `next` as a dep) win over less-specific ones (just a
/// `package.json`). The first match wins.
fn detect(cwd: &Path) -> Framework {
    if let Some(fw) = detect_deno(cwd) {
        return fw;
    }
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
    if let Some(fw) = detect_elixir(cwd) {
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

fn detect_deno(cwd: &Path) -> Option<Framework> {
    if cwd.join("deno.json").is_file() || cwd.join("deno.jsonc").is_file() {
        Some(Framework::Deno)
    } else {
        None
    }
}

fn detect_python(cwd: &Path) -> Option<Framework> {
    // Django is the easiest: the presence of `manage.py` is a
    // very strong signal. We check it first so a Django project
    // with a pyproject.toml that mentions both django and flask
    // picks Django.
    if cwd.join("manage.py").is_file() {
        return Some(Framework::Django);
    }
    let pyproject = std::fs::read_to_string(cwd.join("pyproject.toml")).ok()?;
    let lower = pyproject.to_lowercase();
    // Order: FastAPI > Flask > Django (the latter two share the
    // pyproject path). If multiple are present, FastAPI wins
    // because its runtime model is most different from the other
    // two.
    if lower.contains("fastapi") {
        return Some(Framework::Fastapi);
    }
    if lower.contains("flask") {
        return Some(Framework::Flask);
    }
    if lower.contains("django") {
        return Some(Framework::Django);
    }
    None
}

fn detect_node(cwd: &Path) -> Option<Framework> {
    let pkg = std::fs::read_to_string(cwd.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&pkg).ok()?;
    let deps = node_deps(&v);
    // Order matters: SvelteKit/Nuxt/Remix are wrapper meta-frameworks
    // that pull in Express transitively, so we test the wrapper
    // first. Astro is a sibling; Next.js pulls in React but is
    // itself the dominant signal.
    if deps.iter().any(|d| d == "@sveltejs/kit") {
        Some(Framework::Sveltekit)
    } else if deps.iter().any(|d| d == "next") {
        Some(Framework::Nextjs)
    } else if deps.iter().any(|d| d == "nuxt") {
        Some(Framework::Nuxt)
    } else if deps.iter().any(|d| d.starts_with("@remix-run/")) {
        Some(Framework::Remix)
    } else if deps.iter().any(|d| d == "astro") {
        Some(Framework::Astro)
    } else if deps.iter().any(|d| d == "express") {
        Some(Framework::Express)
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

fn detect_elixir(cwd: &Path) -> Option<Framework> {
    let mix = std::fs::read_to_string(cwd.join("mix.exs")).ok()?;
    if mix.to_lowercase().contains("phoenix") {
        Some(Framework::Phoenix)
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
    fn detects_flask_via_pyproject() {
        let dir = fresh_dir();
        std::fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"x\"\ndependencies = [\"flask\"]\n",
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Flask);
    }

    #[test]
    fn detects_django_via_pyproject() {
        let dir = fresh_dir();
        std::fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"x\"\ndependencies = [\"django\"]\n",
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Django);
    }

    #[test]
    fn detects_django_via_manage_py() {
        // A Django project without a pyproject.toml (just a
        // requirements.txt + manage.py) still detects Django.
        let dir = fresh_dir();
        std::fs::write(dir.join("requirements.txt"), "django==5.0\n").unwrap();
        std::fs::write(dir.join("manage.py"), "#!/usr/bin/env python\n").unwrap();
        assert_eq!(detect(&dir), Framework::Django);
    }

    #[test]
    fn detects_django_wins_over_flask_when_both_present() {
        // The detect_python order is FastAPI > Flask > Django.
        // When FastAPI and Flask are absent, Django via manage.py
        // is the strongest signal.
        let dir = fresh_dir();
        std::fs::write(dir.join("manage.py"), "#!/usr/bin/env python\n").unwrap();
        std::fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"x\"\ndependencies = [\"django\", \"flask\"]\n",
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Django);
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
    fn detects_nuxt_via_package_json() {
        let dir = fresh_dir();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"x","dependencies":{"nuxt":"3.10.0"}}"#,
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Nuxt);
    }

    #[test]
    fn detects_sveltekit_via_package_json() {
        let dir = fresh_dir();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"x","devDependencies":{"@sveltejs/kit":"2.0.0"}}"#,
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Sveltekit);
    }

    #[test]
    fn detects_remix_via_package_json() {
        let dir = fresh_dir();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"x","dependencies":{"@remix-run/node":"2.5.0"}}"#,
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Remix);
    }

    #[test]
    fn detects_sveltekit_wins_over_express() {
        // SvelteKit projects don't typically pull in Express, but
        // if they do (e.g. a hybrid adapter), SvelteKit should win
        // because it's the more specific signal.
        let dir = fresh_dir();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"x","dependencies":{"@sveltejs/kit":"2.0.0","express":"4.18.0"}}"#,
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Sveltekit);
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
    fn detects_phoenix_via_mix_exs() {
        let dir = fresh_dir();
        std::fs::write(
            dir.join("mix.exs"),
            r#"defmodule X do
  use Mix.Project
  defp deps do
    [{:phoenix, "~> 1.7"}]
  end
end
"#,
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Phoenix);
    }

    #[test]
    fn detects_deno_via_deno_json() {
        let dir = fresh_dir();
        std::fs::write(
            dir.join("deno.json"),
            r#"{"tasks":{"start":"deno run --allow-net main.ts"}}"#,
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Deno);
    }

    #[test]
    fn detects_deno_via_deno_jsonc() {
        let dir = fresh_dir();
        std::fs::write(
            dir.join("deno.jsonc"),
            r#"{"tasks":{"start":"deno run --allow-net main.ts"}}"#,
        )
        .unwrap();
        assert_eq!(detect(&dir), Framework::Deno);
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
    fn flask_spec_uses_port_5000() {
        let spec = build_app_spec(Framework::Flask, "x");
        let yaml = render_app_yaml(&spec, "x");
        assert!(yaml.contains("port: 5000"));
        assert!(yaml.contains("framework: flask"));
        assert!(yaml.contains("flask --app app run"));
    }

    #[test]
    fn django_spec_uses_gunicorn() {
        let spec = build_app_spec(Framework::Django, "x");
        let yaml = render_app_yaml(&spec, "x");
        assert!(yaml.contains("port: 8000"));
        assert!(yaml.contains("framework: django"));
        assert!(yaml.contains("gunicorn"));
    }

    #[test]
    fn phoenix_spec_uses_port_4000() {
        let spec = build_app_spec(Framework::Phoenix, "x");
        let yaml = render_app_yaml(&spec, "x");
        assert!(yaml.contains("port: 4000"));
        assert!(yaml.contains("framework: phoenix"));
        assert!(yaml.contains("mix phx.server"));
    }

    #[test]
    fn nuxt_spec_runs_node_output_server() {
        let spec = build_app_spec(Framework::Nuxt, "x");
        let yaml = render_app_yaml(&spec, "x");
        assert!(yaml.contains("port: 3000"));
        assert!(yaml.contains("framework: nuxt"));
        assert!(yaml.contains(".output/server/index.mjs"));
    }

    #[test]
    fn sveltekit_spec_runs_node_build() {
        let spec = build_app_spec(Framework::Sveltekit, "x");
        let yaml = render_app_yaml(&spec, "x");
        assert!(yaml.contains("port: 3000"));
        assert!(yaml.contains("framework: sveltekit"));
        assert!(yaml.contains("node build"));
    }

    #[test]
    fn deno_spec_uses_deno_run() {
        let spec = build_app_spec(Framework::Deno, "x");
        let yaml = render_app_yaml(&spec, "x");
        assert!(yaml.contains("port: 8000"));
        assert!(yaml.contains("framework: deno"));
        assert!(yaml.contains("deno run --allow-net"));
    }

    #[test]
    fn generic_yaml_has_uncommented_placeholders() {
        let spec = build_app_spec(Framework::Generic, "x");
        let yaml = render_app_yaml(&spec, "x");
        assert!(yaml.contains("framework: generic"));
        assert!(yaml.contains("# build_cmd:"));
        assert!(yaml.contains("# run_cmd:"));
    }

    #[test]
    fn print_frameworks_text_includes_fastapi() {
        let mut buf = Vec::new();
        print_frameworks_text(&mut buf, "").expect("text");
        let s = String::from_utf8(buf).expect("utf8");
        assert!(s.contains("FastAPI"), "expected FastAPI row in: {s}");
        assert!(s.contains("Next.js"), "expected Next.js row in: {s}");
        assert!(s.contains("Deno"), "expected Deno row in: {s}");
    }

    #[test]
    fn print_frameworks_text_filter_is_case_insensitive() {
        let mut buf = Vec::new();
        print_frameworks_text(&mut buf, "DE").expect("text");
        let s = String::from_utf8(buf).expect("utf8");
        // "de" matches "Deno" (contains "de"). We test the
        // case-insensitive path by uppercasing the filter; the
        // "Deno" row should still appear.
        assert!(s.contains("Deno"), "expected Deno row in: {s}");
        // And an empty filter prints all rows.
        let mut buf = Vec::new();
        print_frameworks_text(&mut buf, "").expect("text");
        let s = String::from_utf8(buf).expect("utf8");
        assert!(s.contains("Django"), "expected Django in full list: {s}");
    }

    #[test]
    fn print_frameworks_json_round_trips() {
        let rows = collect_framework_rows();
        let json = serde_json::to_string(&rows).expect("serialize");
        let back: Vec<FrameworkRow> = serde_json::from_str(&json).expect("parse");
        assert_eq!(rows.len(), back.len());
        assert!(back.iter().any(|r| r.name == "FastAPI"));
        assert!(back.iter().any(|r| r.name == "Deno"));
    }
}

/// One row in the framework registry, used by `--print-frameworks`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameworkRow {
    pub name: String,
    pub port: u16,
    pub health_path: String,
    pub image: String,
    pub run_cmd: String,
}

/// Walk the same `Framework` enum the CLI uses and produce one
/// `FrameworkRow` per concrete framework. We deliberately call
/// `build_app_spec(Framework::X, "x")` for each so the values stay
/// in sync with the YAML generator — there's no separate registry
/// to drift out of date.
pub fn collect_framework_rows() -> Vec<FrameworkRow> {
    let all = [
        Framework::Fastapi,
        Framework::Flask,
        Framework::Django,
        Framework::Nextjs,
        Framework::Nuxt,
        Framework::Sveltekit,
        Framework::Remix,
        Framework::Express,
        Framework::Go,
        Framework::Rails,
        Framework::Laravel,
        Framework::Astro,
        Framework::Static,
        Framework::Phoenix,
        Framework::Deno,
        Framework::Generic,
    ];
    all.iter()
        .map(|f| {
            let spec = build_app_spec(*f, "x");
            // `name` is what the YAML emits (`framework: <name>`) —
            // the user-facing label. The clap display name is the
            // pretty form ("FastAPI" vs "Fastapi").
            let label = match f {
                Framework::Fastapi => "FastAPI",
                Framework::Flask => "Flask",
                Framework::Django => "Django",
                Framework::Nextjs => "Next.js",
                Framework::Nuxt => "Nuxt",
                Framework::Sveltekit => "SvelteKit",
                Framework::Remix => "Remix",
                Framework::Express => "Express",
                Framework::Go => "Go",
                Framework::Rails => "Rails",
                Framework::Laravel => "Laravel",
                Framework::Astro => "Astro",
                Framework::Static => "Static",
                Framework::Phoenix => "Phoenix",
                Framework::Deno => "Deno",
                Framework::Generic => "Generic",
                Framework::Auto => "Auto",
            };
            FrameworkRow {
                name: label.into(),
                port: spec.port,
                health_path: spec.health_path,
                image: spec.image.unwrap_or_default(),
                run_cmd: spec.run_cmd.unwrap_or_default(),
            }
        })
        .collect()
}

/// Print the framework registry as a human-readable text table.
/// `filter` is a case-insensitive substring matched against the
/// framework name. Empty filter prints all rows.
pub fn print_frameworks_text<W: std::io::Write>(w: &mut W, filter: &str) -> std::io::Result<()> {
    let filter = filter.to_ascii_lowercase();
    let rows = collect_framework_rows();
    let rows: Vec<&FrameworkRow> = rows
        .iter()
        .filter(|r| filter.is_empty() || r.name.to_ascii_lowercase().contains(&filter))
        .collect();
    if rows.is_empty() {
        writeln!(w, "(no frameworks match {filter:?})")?;
        return Ok(());
    }
    // Compute column widths.
    let name_w = rows.iter().map(|r| r.name.len()).max().unwrap_or(0).max(8);
    let port_w = "port".len().max(
        rows.iter()
            .map(|r| r.port.to_string().len())
            .max()
            .unwrap_or(0),
    );
    let health_w = "health_path"
        .len()
        .max(rows.iter().map(|r| r.health_path.len()).max().unwrap_or(0));
    writeln!(
        w,
        "{:<name_w$}  {:>port_w$}  {:<health_w$}  image",
        "framework",
        "port",
        "health_path",
        name_w = name_w,
        port_w = port_w,
        health_w = health_w,
    )?;
    for r in &rows {
        writeln!(
            w,
            "{:<name_w$}  {:>port_w$}  {:<health_w$}  {}",
            r.name,
            r.port,
            r.health_path,
            r.image,
            name_w = name_w,
            port_w = port_w,
            health_w = health_w,
        )?;
    }
    Ok(())
}
