//! xtask: repo-level automation for the Keel platform.
//!
//! - `cargo xtask dep-rules`  architecture analyzer gate (same engine as `keel check` and CI)
//! - `cargo xtask semver`     cargo-semver-checks release gate (skips before the first release tag)
//! - `cargo xtask publish`    publish all publishable crates to crates.io in dependency order
//! - `cargo xtask bump X.Y.Z` bump the lockstep workspace version
//!
//! `dep-rules` and `semver` run in CI; `publish` and `bump` drive the release
//! process documented in PUBLISHING.md.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let task = args.first().map(|s| s.as_str()).unwrap_or("help");
    match task {
        "dep-rules" => dep_rules(),
        "semver" => semver(),
        "publish" => publish(&args[1..]),
        "bump" => match args.get(1) {
            Some(v) => bump(v),
            None => {
                println!("usage: cargo xtask bump <new-version>");
                ExitCode::FAILURE
            }
        },
        _ => {
            println!("usage: cargo xtask <dep-rules|semver|publish|bump>");
            ExitCode::SUCCESS
        }
    }
}

/// xtask lives at platform/xtask: one parent up is the workspace root.
fn root_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

/// G14 / API-versioning gate (L72, D-71): run the external `cargo-semver-checks`
/// release gate over the public crates. The tool is an external binary — not a
/// workspace dependency — so it is invoked, never compiled in. When it is not
/// installed we warn and pass, so CI pins it in its own toolchain step.
/// Before the first release tag exists there is no baseline to diff against,
/// so the gate is skipped gracefully (Wave 1).
fn semver() -> ExitCode {
    let root = root_dir();
    match std::process::Command::new("git")
        .args(["tag", "-l", "keel/platform-v*"])
        .current_dir(root)
        .output()
    {
        Ok(out) if out.status.success() => {
            if String::from_utf8_lossy(&out.stdout).trim().is_empty() {
                println!("semver: no keel/platform-v* release tag yet — no baseline to compare (skipping gate)");
                return ExitCode::SUCCESS;
            }
        }
        Ok(_) => println!("semver: git tag query failed — proceeding to tool check"),
        Err(_) => println!("semver: git unavailable — proceeding to tool check"),
    }
    // Probe first: a missing subcommand still returns an Ok status from cargo,
    // so only run the gate when the tool is actually installed.
    let installed = std::process::Command::new("cargo")
        .args(["semver-checks", "--version"])
        .current_dir(root)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    if !installed {
        println!("semver: cargo-semver-checks not installed — `cargo install cargo-semver-checks` (skipping gate)");
        return ExitCode::SUCCESS;
    }
    let status = std::process::Command::new("cargo")
        .args(["semver-checks", "check-release"])
        .current_dir(root)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("semver: no breaking changes detected");
            ExitCode::SUCCESS
        }
        Ok(s) => {
            println!("semver [!!]: breaking changes reported by cargo-semver-checks ({s})");
            ExitCode::FAILURE
        }
        Err(e) => {
            println!("semver [!!]: failed to invoke cargo-semver-checks: {e}");
            ExitCode::FAILURE
        }
    }
}

fn dep_rules() -> ExitCode {
    let root = root_dir();
    let violations = keel_analyzer::analyze_workspace(root);
    if violations.is_empty() {
        println!("dep-rules: clean");
        return ExitCode::SUCCESS;
    }
    for v in &violations {
        println!("dep-rules [!!] {} {}: {}", v.code, v.member, v.message);
    }
    ExitCode::FAILURE
}

/// A workspace member prepared for publishing.
struct Pkg {
    name: String,
    member: String,
    version: String,
    publishable: bool,
    /// Dependencies on other workspace members (normal + dev), by package name.
    internal_deps: Vec<String>,
}

fn load_workspace(root: &Path) -> Result<Vec<Pkg>, String> {
    let root_toml = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("cannot read root Cargo.toml: {e}"))?;
    let root_v: toml::Value = root_toml
        .parse()
        .map_err(|e| format!("root toml parse: {e}"))?;
    let ws_version = root_v
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "no [workspace.package].version".to_string())?
        .to_string();
    let members = keel_analyzer::parse_members(&root_toml)?;

    let mut pkgs = Vec::new();
    for member in members {
        let manifest_path = root.join(&member).join("Cargo.toml");
        let raw = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("cannot read {}: {e}", manifest_path.display()))?;
        let v: toml::Value = raw
            .parse()
            .map_err(|e| format!("{member}: manifest parse: {e}"))?;
        let package = v
            .get("package")
            .ok_or_else(|| format!("{member}: no [package] section"))?;
        let name = package
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| format!("{member}: no package name"))?
            .to_string();
        let version = match package.get("version") {
            Some(toml::Value::String(s)) => s.clone(),
            Some(toml::Value::Table(t)) if t.get("workspace").is_some() => ws_version.clone(),
            _ => return Err(format!("{member}: no package version")),
        };
        let publishable = match package.get("publish") {
            None => true,
            Some(toml::Value::Boolean(b)) => *b,
            // Restricted registry list: still a publish target.
            Some(toml::Value::Array(_)) => true,
            _ => return Err(format!("{member}: malformed publish field")),
        };
        let mut internal_deps = Vec::new();
        for table in ["dependencies", "dev-dependencies"] {
            if let Some(deps) = v.get(table).and_then(|d| d.as_table()) {
                for key in deps.keys() {
                    if key.starts_with("keel-") {
                        internal_deps.push(key.clone());
                    }
                }
            }
        }
        internal_deps.sort();
        internal_deps.dedup();
        pkgs.push(Pkg {
            name,
            member,
            version,
            publishable,
            internal_deps,
        });
    }
    pkgs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(pkgs)
}

/// Topologically sort publishable crates so dependencies are published first.
/// Fails early when a publishable crate depends on a gated (publish = false)
/// crate, or on a crate that is not a workspace member.
fn publish_order(pkgs: &[Pkg]) -> Result<Vec<String>, String> {
    let by_name: BTreeMap<String, &Pkg> = pkgs.iter().map(|p| (p.name.clone(), p)).collect();
    let mut order = Vec::new();
    let mut visited: BTreeMap<String, u8> = BTreeMap::new(); // 0 = visiting, 1 = done

    fn visit(
        name: &str,
        by_name: &BTreeMap<String, &Pkg>,
        visited: &mut BTreeMap<String, u8>,
        order: &mut Vec<String>,
        stack: &mut Vec<String>,
    ) -> Result<(), String> {
        match visited.get(name) {
            Some(1) => return Ok(()),
            Some(_) => return Err(format!("dependency cycle detected through {name}")),
            None => {}
        }
        let pkg = by_name
            .get(name)
            .ok_or_else(|| format!("dependency {name} is not a workspace member"))?;
        if !pkg.publishable {
            return Err(format!(
                "publishable crate depends on gated crate: {} -> {name} (publish = false)",
                stack.join(" -> ")
            ));
        }
        visited.insert(name.to_string(), 0);
        stack.push(name.to_string());
        for dep in &pkg.internal_deps {
            visit(dep, by_name, visited, order, stack)?;
        }
        stack.pop();
        visited.insert(name.to_string(), 1);
        order.push(name.to_string());
        Ok(())
    }

    let mut stack = Vec::new();
    for pkg in pkgs {
        if pkg.publishable {
            visit(&pkg.name, &by_name, &mut visited, &mut order, &mut stack)?;
        }
    }
    Ok(order)
}

const PUBLISH_RETRIES: usize = 5;
const PUBLISH_RETRY_DELAY_SECS: u64 = 30;

/// Result of one `cargo publish` attempt.
enum PublishOutcome {
    /// Uploaded (or dry-run packaged + verified).
    Ok,
    /// Dry-run only: cargo refused to package because workspace-internal
    /// `keel-*` dependencies are not on crates.io yet (first-publish
    /// bootstrap gap). The real wave publishes in dependency order, which
    /// resolves each dep before its dependents are attempted.
    Deferred(String),
    /// A real failure (manifest, files, guard, registry).
    Failed,
}

/// The unresolved package name when the only failure is the first-publish
/// bootstrap gap: workspace-internal `keel-*` crates are absent from the
/// crates.io index until the wave runs, so cargo cannot package dependents.
fn bootstrap_missing(stderr: &str) -> Option<&str> {
    if !stderr.contains("location searched: crates.io index") {
        return None;
    }
    const PAT: &str = "no matching package named `";
    let rest = stderr.get(stderr.find(PAT)? + PAT.len()..)?;
    let name = &rest[..rest.find('`')?];
    name.starts_with("keel-").then_some(name)
}

fn publish_one(root: &Path, name: &str, dry_run: bool) -> PublishOutcome {
    let max_attempts = if dry_run { 1 } else { PUBLISH_RETRIES };
    for attempt in 1..=max_attempts {
        let mut cmd = std::process::Command::new("cargo");
        cmd.args(["publish", "-p", name]).current_dir(root);
        if dry_run {
            cmd.arg("--dry-run");
        }
        let output = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                println!("publish [!!]: failed to invoke cargo publish for {name}: {e}");
                return PublishOutcome::Failed;
            }
        };
        if output.status.success() {
            println!("publish: {name} OK (attempt {attempt})");
            return PublishOutcome::Ok;
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if dry_run {
            if let Some(missing) = bootstrap_missing(&stderr) {
                println!(
                    "publish: {name} deferred (first-publish bootstrap gap: \
                     `{missing}` is not on crates.io yet — manifest and files \
                     validated; the real wave packages it in dependency order)"
                );
                return PublishOutcome::Deferred(missing.to_string());
            }
        }
        if !dry_run && (stderr.contains("already exists") || stderr.contains("already uploaded")) {
            println!("publish: {name} already published at this version — continuing");
            return PublishOutcome::Ok;
        }
        println!(
            "publish [!!]: cargo publish failed for {name} (attempt {attempt}/{max_attempts})"
        );
        for line in stderr.lines().rev().take(12) {
            println!("    {line}");
        }
        if attempt < max_attempts {
            println!("publish: waiting {PUBLISH_RETRY_DELAY_SECS}s for registry propagation before retry...");
            std::thread::sleep(std::time::Duration::from_secs(PUBLISH_RETRY_DELAY_SECS));
        }
    }
    PublishOutcome::Failed
}

/// Publish all publishable workspace crates to crates.io, in dependency order.
///
/// `--dry-run` packages and validates without uploading (no retries: a dry-run
/// failure is a real problem). Crates whose workspace-internal `keel-*` deps
/// are not on crates.io yet are reported as `deferred` — the first-publish
/// bootstrap gap; the real wave publishes in dependency order, which resolves
/// each dep before its dependents are attempted. `--only a,b,c` restricts the
/// run to a subset. The final `XTASK-PUBLISH:` line is machine-readable for CI.
fn publish(args: &[String]) -> ExitCode {
    let mut dry_run = false;
    let mut only: Option<Vec<String>> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dry-run" => dry_run = true,
            "--only" => {
                i += 1;
                let Some(spec) = args.get(i) else {
                    println!("publish: --only needs a comma-separated crate list");
                    return ExitCode::FAILURE;
                };
                only = Some(
                    spec.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                );
            }
            other => {
                println!("publish [!!]: unknown flag {other}");
                println!("usage: cargo xtask publish [--dry-run] [--only crate1,crate2]");
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }

    let root = root_dir();
    let pkgs = match load_workspace(root) {
        Ok(p) => p,
        Err(e) => {
            println!("publish [!!]: {e}");
            return ExitCode::FAILURE;
        }
    };
    let order = match publish_order(&pkgs) {
        Ok(o) => o,
        Err(e) => {
            println!("publish [!!]: {e}");
            return ExitCode::FAILURE;
        }
    };
    let by_name: BTreeMap<&str, &Pkg> = pkgs.iter().map(|p| (p.name.as_str(), p)).collect();

    if let Some(list) = &only {
        for name in list {
            match by_name.get(name.as_str()) {
                None => {
                    println!("publish [!!]: --only crate {name} is not a workspace member");
                    return ExitCode::FAILURE;
                }
                Some(p) if !p.publishable => {
                    println!("publish [!!]: --only crate {name} is gated (publish = false)");
                    return ExitCode::FAILURE;
                }
                Some(_) => {}
            }
        }
    }

    let selected: Vec<&Pkg> = order
        .iter()
        .filter(|name| only.as_ref().map(|o| o.contains(name)).unwrap_or(true))
        .map(|name| by_name[name.as_str()])
        .collect();

    let mut skipped = 0;
    for pkg in &pkgs {
        if !pkg.publishable {
            println!("publish: skip {:<24} (publish = false)", pkg.name);
            skipped += 1;
        }
    }

    if selected.is_empty() {
        println!("publish: nothing selected to publish ({skipped} gated)");
        println!("XTASK-PUBLISH: version= crates= count=0 dry-run={dry_run}");
        return ExitCode::SUCCESS;
    }

    println!(
        "publish: {} crate(s) in dependency order, dry-run = {dry_run}",
        selected.len()
    );
    for (n, pkg) in selected.iter().enumerate() {
        println!(
            "publish: [{}/{}] {} v{} ({})",
            n + 1,
            selected.len(),
            pkg.name,
            pkg.version,
            pkg.member
        );
    }

    let version = selected[0].version.clone();
    let mut published = Vec::new();
    let mut deferred = Vec::new();
    for pkg in &selected {
        match publish_one(root, &pkg.name, dry_run) {
            PublishOutcome::Ok => published.push(pkg.name.clone()),
            PublishOutcome::Deferred(missing) => {
                published.push(pkg.name.clone());
                deferred.push(format!("{}({missing})", pkg.name));
            }
            PublishOutcome::Failed => return ExitCode::FAILURE,
        }
    }

    if !deferred.is_empty() {
        println!(
            "publish: {} crate(s) deferred to the real ordered wave (bootstrap gap)",
            deferred.len()
        );
    }
    println!(
        "XTASK-PUBLISH: version={version} crates={} count={} dry-run={dry_run} deferred={}",
        published.join(","),
        published.len(),
        deferred.len()
    );
    ExitCode::SUCCESS
}

/// Bump the lockstep workspace version: `[workspace.package].version` plus
/// every versioned internal dependency entry in `[workspace.dependencies]`.
fn bump(new_version: &str) -> ExitCode {
    let ok = new_version.split('.').count() == 3
        && new_version
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    if !ok {
        println!("bump [!!]: version must be MAJOR.MINOR.PATCH (got {new_version})");
        return ExitCode::FAILURE;
    }
    let root = root_dir();
    let path = root.join("Cargo.toml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            println!("bump [!!]: cannot read root Cargo.toml: {e}");
            return ExitCode::FAILURE;
        }
    };
    let v: toml::Value = match raw.parse() {
        Ok(v) => v,
        Err(e) => {
            println!("bump [!!]: root Cargo.toml does not parse: {e}");
            return ExitCode::FAILURE;
        }
    };
    let old = match v
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|x| x.as_str())
    {
        Some(s) => s.to_string(),
        None => {
            println!("bump [!!]: no [workspace.package].version found");
            return ExitCode::FAILURE;
        }
    };
    if old == new_version {
        println!("bump: version is already {old} — nothing to do");
        return ExitCode::SUCCESS;
    }

    let old_dep_literal = format!("version = \"{old}\"");
    let new_dep_literal = format!("version = \"{new_version}\"");
    let mut expected = 0usize;
    let mut section = String::new();
    for line in raw.lines() {
        if let Some(s) = line.strip_prefix('[') {
            if let Some(e) = s.strip_suffix(']') {
                section = e.to_string();
            }
        }
        if section == "workspace.dependencies"
            && line.contains(&old_dep_literal)
            && line.contains(", path =")
        {
            expected += 1;
        }
    }
    if expected == 0 {
        println!("bump [!!]: no versioned internal dependency entries found — aborting");
        return ExitCode::FAILURE;
    }

    let mut pkg_hits = 0usize;
    let mut dep_hits = 0usize;
    let mut out = String::with_capacity(raw.len());
    let mut section = String::new();
    for line in raw.lines() {
        if let Some(s) = line.strip_prefix('[') {
            if let Some(e) = s.strip_suffix(']') {
                section = e.to_string();
            }
        }
        let mut next = line.to_string();
        if section == "workspace.package" && line.starts_with("version = ") {
            next = format!("version = \"{new_version}\"");
            pkg_hits += 1;
        } else if section == "workspace.dependencies"
            && line.contains(&old_dep_literal)
            && line.contains(", path =")
        {
            next = line.replace(&old_dep_literal, &new_dep_literal);
            dep_hits += 1;
        }
        out.push_str(&next);
        out.push('\n');
    }
    if pkg_hits != 1 || dep_hits != expected {
        println!(
            "bump [!!]: replacement mismatch (workspace.package lines = {pkg_hits}, dep entries = {dep_hits}/{expected}) — aborting"
        );
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::write(&path, out) {
        println!("bump [!!]: cannot write root Cargo.toml: {e}");
        return ExitCode::FAILURE;
    }
    println!(
        "bump: {old} -> {new_version} (workspace.package + {dep_hits} internal dependency entries)"
    );
    println!("bump: next — update CHANGELOG.md, then `just publish-dry`");
    ExitCode::SUCCESS
}
