//! Views engine (Wave 2): template discovery, the compiled-AST cache and
//! the extension registries (custom directives, Blade::if-style custom
//! conditionals, view hooks, package namespace resolution).
//!
//! Compiled cache contract: the parsed AST of every template is serialized
//! under the cache dir as JSON, pinned by the template's content hash (the
//! mtime is recorded alongside for operations). Any drift between the
//! stored source hash and the file on disk invalidates the entry and the
//! template reparses. keel view:cache precompiles everything up front;
//! keel view:clear drops every compiled entry (fail-closed: a corrupt
//! cache entry reparses from source, never boots stale).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::render::Renderer;
use crate::Template;

// ------------------------------------------------------------- error codes

/// Stable views error codes (fail-closed; reported at the edge like the
/// GRAPH-* gates).
pub const VIEW_PARSE: &str = "VIEW-PARSE";
pub const VIEW_NOT_FOUND: &str = "VIEW-NOT-FOUND";
pub const VIEW_RENDER: &str = "VIEW-RENDER";
pub const VIEW_CACHE: &str = "VIEW-CACHE";
pub const VIEW_LOOP: &str = "VIEW-LOOP";

/// One views failure (code + human-readable message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewError {
    pub code: String,
    pub message: String,
}

impl ViewError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ViewError {}

// ------------------------------------------------------- extension handler

/// Custom directive renderer (name, raw argument text, merged context).
/// The returned string is emitted verbatim (the handler owns escaping).
pub type DirectiveHandler =
    dyn Fn(&str, Option<&str>, &Value) -> Result<String, ViewError> + Send + Sync;

/// Blade::if-style custom conditional (args, merged context) -> bool.
pub type ConditionalHandler = dyn Fn(&[Value], &Value) -> bool + Send + Sync;

/// View hook (composer equivalent): mutates the context before render.
pub type HookHandler = dyn Fn(&mut Value) + Send + Sync;

/// Package namespace resolver: (namespace, name) -> template path.
/// Wired by Wave 8 package discovery; absent hook = namespaces fail closed.
pub type ResolverFn = dyn Fn(&str, &str) -> Option<PathBuf> + Send + Sync;

// ------------------------------------------------------------------ config

/// [views] config section shape (every key optional).
#[derive(Debug, Clone, Default, Deserialize)]
struct ViewsFile {
    #[serde(default)]
    paths: Option<Vec<String>>,
    #[serde(default)]
    cache_dir: Option<String>,
    #[serde(default)]
    env: Option<String>,
}

/// Engine configuration. Defaults match the Laravel layout: templates under
/// resources/views, compiled cache under storage/framework/views.
#[derive(Debug, Clone)]
pub struct ViewsConfig {
    /// Template roots searched in order (first hit wins).
    pub paths: Vec<PathBuf>,
    /// Compiled-AST cache dir; None disables the disk cache.
    pub cache_dir: Option<PathBuf>,
    /// Environment name for @env/@production.
    pub env: String,
}

impl Default for ViewsConfig {
    fn default() -> Self {
        Self {
            paths: vec![PathBuf::from("resources/views")],
            cache_dir: Some(PathBuf::from("storage/framework/views")),
            env: "production".into(),
        }
    }
}

impl ViewsConfig {
    /// Parse a config document. Accepts either a bare views document or a
    /// full config document with a [views] table.
    pub fn from_toml(raw: &str) -> Result<Self, ViewError> {
        let value: toml::Value = raw
            .parse()
            .map_err(|e| ViewError::new(VIEW_PARSE, format!("views config parse failed: {e}")))?;
        let table = value.get("views").cloned().unwrap_or(value);
        let file: ViewsFile = table
            .try_into()
            .map_err(|e| ViewError::new(VIEW_PARSE, format!("views config rejected: {e}")))?;
        let defaults = Self::default();
        Ok(Self {
            paths: file
                .paths
                .map(|p| p.into_iter().map(PathBuf::from).collect())
                .unwrap_or(defaults.paths),
            cache_dir: file
                .cache_dir
                .map(PathBuf::from)
                .or(defaults.cache_dir),
            env: file.env.unwrap_or(defaults.env),
        })
    }
}

// ------------------------------------------------------------------ engine

/// In-memory compiled-template memo (keyed by view name).
struct Memo {
    source_hash: String,
    template: Template,
}

/// On-disk compiled-cache entry.
#[derive(Serialize, Deserialize)]
struct DiskEntry {
    source_hash: String,
    mtime_secs: i64,
    mtime_nanos: u32,
    template: Template,
}

/// The views engine: template discovery + compiled cache + the extension
/// registries. Shared behind an Arc at the web edge (Send + Sync).
pub struct ViewsEngine {
    config: ViewsConfig,
    directives: Mutex<BTreeMap<String, Arc<DirectiveHandler>>>,
    conditionals: Mutex<BTreeMap<String, Arc<ConditionalHandler>>>,
    hooks: Mutex<BTreeMap<String, Vec<Arc<HookHandler>>>>,
    resolver: Mutex<Option<Arc<ResolverFn>>>,
    memo: Mutex<HashMap<String, Memo>>,
}

impl ViewsEngine {
    pub fn new(config: ViewsConfig) -> Self {
        Self {
            config,
            directives: Mutex::new(BTreeMap::new()),
            conditionals: Mutex::new(BTreeMap::new()),
            hooks: Mutex::new(BTreeMap::new()),
            resolver: Mutex::new(None),
            memo: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &ViewsConfig {
        &self.config
    }

    // ------------------------------------------------------- registries

    /// Register a custom directive (emitted verbatim; handler owns escaping).
    pub fn register_directive<F>(&self, name: impl Into<String>, handler: F)
    where
        F: Fn(&str, Option<&str>, &Value) -> Result<String, ViewError> + Send + Sync + 'static,
    {
        self.directives
            .lock()
            .unwrap()
            .insert(name.into(), Arc::new(handler));
    }

    /// Register a Blade::if-style custom conditional.
    pub fn register_conditional<F>(&self, name: impl Into<String>, handler: F)
    where
        F: Fn(&[Value], &Value) -> bool + Send + Sync + 'static,
    {
        self.conditionals
            .lock()
            .unwrap()
            .insert(name.into(), Arc::new(handler));
    }

    /// Register a view hook (composer equivalent) for one view name; "*"
    /// hooks run for every view.
    pub fn register_hook<F>(&self, view: impl Into<String>, hook: F)
    where
        F: Fn(&mut Value) + Send + Sync + 'static,
    {
        self.hooks
            .lock()
            .unwrap()
            .entry(view.into())
            .or_default()
            .push(Arc::new(hook));
    }

    /// Install the package namespace resolver (pkg::view, Wave 8).
    pub fn set_resolver<F>(&self, resolver: F)
    where
        F: Fn(&str, &str) -> Option<PathBuf> + Send + Sync + 'static,
    {
        *self.resolver.lock().unwrap() = Some(Arc::new(resolver));
    }

    pub(crate) fn directive(&self, name: &str) -> Option<Arc<DirectiveHandler>> {
        self.directives.lock().unwrap().get(name).cloned()
    }

    pub(crate) fn conditional(&self, name: &str) -> Option<Arc<ConditionalHandler>> {
        self.conditionals.lock().unwrap().get(name).cloned()
    }

    pub(crate) fn hooks_for(&self, name: &str) -> Vec<Arc<HookHandler>> {
        let hooks = self.hooks.lock().unwrap();
        let mut out = Vec::new();
        if let Some(global) = hooks.get("*") {
            out.extend(global.iter().cloned());
        }
        if let Some(specific) = hooks.get(name) {
            out.extend(specific.iter().cloned());
        }
        out
    }

    // --------------------------------------------------------- discovery

    /// Resolve a view name to a template path. Dot paths map to directories
    /// (errors.404 -> errors/404.html); pkg::name goes through the
    /// namespace resolver. Fail-closed: VIEW-NOT-FOUND otherwise.
    pub fn resolve(&self, name: &str) -> Result<PathBuf, ViewError> {
        if let Some((ns, rest)) = name.split_once("::") {
            let resolver = self.resolver.lock().unwrap().clone();
            return match resolver {
                Some(r) => r(ns, rest).ok_or_else(|| {
                    ViewError::new(
                        VIEW_NOT_FOUND,
                        format!("no template for {name:?} in package namespace {ns:?}"),
                    )
                }),
                None => Err(ViewError::new(
                    VIEW_NOT_FOUND,
                    format!("package namespace {ns:?} has no resolver installed"),
                )),
            };
        }
        let rel = name.replace('.', "/");
        let file = if rel.ends_with(".html") {
            rel
        } else {
            format!("{rel}.html")
        };
        for root in &self.config.paths {
            let candidate = root.join(&file);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        Err(ViewError::new(
            VIEW_NOT_FOUND,
            format!(
                "view {name:?} not found (searched {})",
                self.config
                    .paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ))
    }

    pub fn exists(&self, name: &str) -> bool {
        self.resolve(name).is_ok()
    }

    // ---------------------------------------------------- template loading

    /// Load + compile one template (memory memo, then disk cache, then
    /// parse). Content-hash pinned: drift reparses from source.
    pub fn template(&self, name: &str) -> Result<Template, ViewError> {
        let path = self.resolve(name)?;
        let source = std::fs::read_to_string(&path).map_err(|e| {
            ViewError::new(VIEW_CACHE, format!("{}: {e}", path.display()))
        })?;
        let source_hash = hex_hash(source.as_bytes());

        if let Some(memo) = self.memo.lock().unwrap().get(name) {
            if memo.source_hash == source_hash {
                return Ok(memo.template.clone());
            }
        }

        let mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok());

        let cache_path = self.cache_path(name);
        if let Some(cache_path) = &cache_path {
            if cache_path.is_file() {
                match std::fs::read_to_string(cache_path)
                    .map_err(|e| e.to_string())
                    .and_then(|raw| {
                        serde_json::from_str::<DiskEntry>(&raw).map_err(|e| e.to_string())
                    }) {
                    Ok(entry) if entry.source_hash == source_hash => {
                        self.memo.lock().unwrap().insert(
                            name.to_string(),
                            Memo {
                                source_hash: source_hash.clone(),
                                template: entry.template.clone(),
                            },
                        );
                        return Ok(entry.template);
                    }
                    _ => {
                        // Corrupt or stale entry: drop it and reparse (fail
                        // closed toward the source, never toward staleness).
                        let _ = std::fs::remove_file(cache_path);
                    }
                }
            }
        }

        let template = crate::parse_template(&source)?;

        if let Some(cache_path) = &cache_path {
            if let Some(dir) = cache_path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| {
                    ViewError::new(VIEW_CACHE, format!("{}: {e}", dir.display()))
                })?;
            }
            let (secs, nanos) = mtime
                .map(|d| (d.as_secs() as i64, d.subsec_nanos()))
                .unwrap_or((0, 0));
            let entry = DiskEntry {
                source_hash: source_hash.clone(),
                mtime_secs: secs,
                mtime_nanos: nanos,
                template: template.clone(),
            };
            let json = serde_json::to_string(&entry)
                .map_err(|e| ViewError::new(VIEW_CACHE, format!("compile failed: {e}")))?;
            std::fs::write(cache_path, json).map_err(|e| {
                ViewError::new(VIEW_CACHE, format!("{}: {e}", cache_path.display()))
            })?;
        }

        self.memo.lock().unwrap().insert(
            name.to_string(),
            Memo {
                source_hash,
                template: template.clone(),
            },
        );
        Ok(template)
    }

    /// Compiled entries are keyed by view name only; the stored source
    /// hash (+ mtime) pins freshness, so drift overwrites the same entry
    /// instead of accumulating stale files.
    fn cache_path(&self, name: &str) -> Option<PathBuf> {
        self.config
            .cache_dir
            .as_ref()
            .map(|dir| dir.join(format!("{}.json", hex_hash(name.as_bytes()))))
    }

    // ------------------------------------------------------------ render

    /// Render one view with a JSON context. View hooks (composers) run
    /// before rendering; "*" hooks run for every view.
    pub fn render(&self, name: &str, mut context: Value) -> Result<String, ViewError> {
        for hook in self.hooks_for(name) {
            hook(&mut context);
        }
        let template = self.template(name)?;
        let mut renderer = Renderer::new(self, context);
        renderer.render_template(&template, name)
    }

    // -------------------------------------------------------- cache admin

    /// Precompile every discoverable template into the compiled cache
    /// (keel view:cache). Returns the number of templates compiled.
    pub fn precompile_all(&self) -> Result<usize, ViewError> {
        let mut names = Vec::new();
        for root in &self.config.paths {
            collect_view_names(root, root, &mut names);
        }
        names.sort();
        names.dedup();
        let mut count = 0;
        for name in names {
            self.template(&name)?;
            count += 1;
        }
        Ok(count)
    }

    /// Drop every compiled cache entry (keel view:clear). Returns the
    /// number of entries removed.
    pub fn clear_cache(&self) -> Result<usize, ViewError> {
        self.memo.lock().unwrap().clear();
        let Some(dir) = &self.config.cache_dir else {
            return Ok(0);
        };
        if !dir.exists() {
            return Ok(0);
        }
        let mut files = Vec::new();
        collect_files(dir, dir, &mut files);
        let mut count = 0;
        for file in files {
            if file.extension().and_then(|e| e.to_str()) == Some("json") {
                std::fs::remove_file(&file).map_err(|e| {
                    ViewError::new(VIEW_CACHE, format!("{}: {e}", file.display()))
                })?;
                count += 1;
            }
        }
        Ok(count)
    }

    /// Drop **one** compiled cache entry (Wave 4 Phase 5,
    /// `keel cache:forget views:<name>`): the in-process memo plus the disk
    /// artifact. Returns true when anything was removed.
    pub fn forget_cache(&self, name: &str) -> Result<bool, ViewError> {
        let removed_memo = self.memo.lock().unwrap().remove(name).is_some();
        let mut removed_disk = false;
        if let Some(path) = self.cache_path(name) {
            if path.is_file() {
                std::fs::remove_file(&path).map_err(|e| {
                    ViewError::new(VIEW_CACHE, format!("{}: {e}", path.display()))
                })?;
                removed_disk = true;
            }
        }
        Ok(removed_memo || removed_disk)
    }
}

fn hex_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Recursively collect view names (relative path minus .html, dot-joined).
fn collect_view_names(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_view_names(&path, root, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("html") {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_str = rel.to_string_lossy();
                let name = rel_str
                    .strip_suffix(".html")
                    .unwrap_or(&rel_str)
                    .replace(['/', '\\'], ".");
                out.push(name);
            }
        }
    }
}

/// Recursively collect regular files under dir.
fn collect_files(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let _ = root;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, root, out);
        } else {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Build an engine over a temp workspace with the given template files
    /// (paths relative to resources/views).
    fn engine_with(files: &[(&str, &str)]) -> (PathBuf, ViewsEngine) {
        let dir = std::env::temp_dir().join(format!(
            "keel-views-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let views = dir.join("resources").join("views");
        std::fs::create_dir_all(&views).unwrap();
        for (rel, body) in files {
            let path = views.join(rel.replace('.', "/") + ".html");
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, body).unwrap();
        }
        let config = ViewsConfig {
            paths: vec![views],
            cache_dir: Some(dir.join("storage").join("framework").join("views")),
            env: "testing".into(),
        };
        let engine = ViewsEngine::new(config);
        (dir, engine)
    }

    fn render(engine: &ViewsEngine, name: &str, ctx: Value) -> String {
        engine
            .render(name, ctx)
            .unwrap_or_else(|e| panic!("render {name} failed: {e}"))
    }

    #[test]
    fn escaping_security_facet() {
        let (_dir, engine) = engine_with(&[(
            "page",
            "{{ text }}|{!! raw !!}|{{ missing }}",
        )]);
        let out = render(
            &engine,
            "page",
            json!({ "text": "<b>\"a\" & 'b'</b>", "raw": "<i>ok</i>" }),
        );
        assert_eq!(
            out,
            "&lt;b&gt;&quot;a&quot; &amp; &#39;b&#39;&lt;/b&gt;|<i>ok</i>|"
        );
    }

    #[test]
    fn filters_and_dot_paths() {
        let (_dir, engine) = engine_with(&[(
            "page",
            "{{ user.name | upper }}/{{ tags | join(\", \") }}/{{ missing | default(\"n/a\") }}",
        )]);
        let out = render(
            &engine,
            "page",
            json!({ "user": { "name": "ada" }, "tags": ["x", "y"] }),
        );
        assert_eq!(out, "ADA/x, y/n/a");
    }

    /// Wave 6 P3: the renderer's `trans` filter resolves literal keys,
    /// path values and explicit arguments through the injected `_lang`
    /// flat map; with no map every key degrades to itself.
    #[test]
    fn trans_filter_renders_through_lang_map() {
        let (_dir, engine) = engine_with(&[(
            "page",
            "{{ \"dashboard.title\" | trans }}|{{ status_key | trans }}|{{ \"missing.key\" | trans }}|{{ \"a.b\" | upper | trans(\"page.title\") }}",
        )]);
        let out = render(
            &engine,
            "page",
            json!({
                "_lang": {
                    "dashboard.title": "Dashboard",
                    "appointments.booked": "Booked",
                    "page.title": "Title"
                },
                "status_key": "appointments.booked"
            }),
        );
        assert_eq!(out, "Dashboard|Booked|missing.key|Title");

        // No `_lang` map injected: everything degrades safely.
        let bare = render(&engine, "page", json!({ "status_key": "k2" }));
        assert_eq!(bare, "dashboard.title|k2|missing.key|page.title");
    }

    /// Wave 6 P3: a quoted literal echoes through the normal escape path
    /// (Blade parity for `{{ "text" }}`), filters included.
    #[test]
    fn quoted_literal_echo_escapes() {
        let (_dir, engine) = engine_with(&[("page", "{{ \"hello <b>\" }}/{{ \"x\" | upper }}")]);
        assert_eq!(render(&engine, "page", json!({})), "hello &lt;b&gt;/X");
    }

    #[test]
    fn conditionals_if_elseif_else_unless() {
        let (_dir, engine) = engine_with(&[(
            "page",
            "@if(count > 2)MANY@elseif(count == 1)ONE@else FEW@endif|@unless(hidden)SHOWN@endunless",
        )]);
        assert_eq!(
            render(&engine, "page", json!({ "count": 5, "hidden": false })),
            "MANY|SHOWN"
        );
        assert_eq!(
            render(&engine, "page", json!({ "count": 1, "hidden": true })),
            "ONE|"
        );
    }

    #[test]
    fn custom_conditional_blade_if_parity() {
        let (_dir, engine) = engine_with(&[("page", "@if(admin()) A @endif")]);
        engine.register_conditional("admin", |args, ctx| {
            let _ = args;
            ctx.get("role").and_then(|v| v.as_str()) == Some("admin")
        });
        assert_eq!(render(&engine, "page", json!({ "role": "admin" })).trim(), "A");
        assert_eq!(render(&engine, "page", json!({ "role": "user" })).trim(), "");
        // Unregistered custom conditionals fail closed (false).
        let (_dir2, engine2) = engine_with(&[("page", "@if(mystery()) X @endif")]);
        assert_eq!(render(&engine2, "page", json!({})).trim(), "");
    }

    #[test]
    fn loops_foreach_forelse_for() {
        let (_dir, engine) = engine_with(&[
            (
                "each",
                "@foreach(items as item)[{{ item }}]@endforeach",
            ),
            (
                "kv",
                "@foreach(map as k => v){{ k }}={{ v }};@endforeach",
            ),
            (
                "orelse",
                "@forelse(items as item)[{{ item }}]@empty none @endforelse",
            ),
            ("range", "@for(i in 0..3){{ i }}@endfor"),
            ("cstyle", "@for($i = 0; $i < 4; $i += 2){{ i }}@endfor"),
        ]);
        assert_eq!(render(&engine, "each", json!({ "items": ["a", "b"] })), "[a][b]");
        assert_eq!(
            render(&engine, "kv", json!({ "map": { "x": 1, "y": 2 } })),
            "x=1;y=2;"
        );
        assert_eq!(
            render(&engine, "orelse", json!({ "items": [] })).trim(),
            "none"
        );
        assert_eq!(
            render(&engine, "orelse", json!({ "items": ["a"] })),
            "[a]"
        );
        assert_eq!(render(&engine, "range", json!({})), "012");
        assert_eq!(render(&engine, "cstyle", json!({})), "02");
    }

    #[test]
    fn runaway_for_fails_closed() {
        let (_dir, engine) = engine_with(&[("bad", "@for(i = 0; i < 10; i += 0)x@endfor")]);
        let err = engine.render("bad", json!({})).unwrap_err();
        assert_eq!(err.code, VIEW_LOOP);
    }

    #[test]
    fn once_renders_once_inside_loops() {
        let (_dir, engine) = engine_with(&[(
            "page",
            "@foreach(items as item)@once[head]@endonce{{ item }}@endforeach",
        )]);
        assert_eq!(
            render(&engine, "page", json!({ "items": ["a", "b", "c"] })),
            "[head]abc"
        );
    }

    #[test]
    fn extends_sections_yield_parent_show() {
        let (_dir, engine) = engine_with(&[
            (
                "layouts.app",
                "@section(\"body\")layout-body@endsection<t>@yield(\"title\", \"untitled\")</t><b>@yield(\"body\")</b><s>@section(\"side\")layout-side@endsection@yield(\"side\")</s>",
            ),
            (
                "child",
                "@extends(\"layouts.app\")@section(\"title\")Page@endsection@section(\"body\")main @parent extra@endsection",
            ),
            (
                "parented",
                "@extends(\"layouts.app\")@section(\"body\")@parent@endsection@section(\"side\")kid-side@show",
            ),
        ]);
        // Child overrides win; @parent pulls the layout's section content.
        let out = render(&engine, "child", json!({}));
        assert!(out.contains("<t>Page</t>"), "{out}");
        assert!(out.contains("<b>main layout-body extra</b>"), "{out}");
        assert!(out.contains("<s>layout-side</s>"), "{out}");

        // @show emits the section in place; @parent alone inherits.
        let out = render(&engine, "parented", json!({}));
        assert!(out.contains("<b>layout-body</b>"), "{out}");
        assert!(out.contains("<s>kid-side</s>"), "{out}");
    }

    #[test]
    fn include_variants() {
        let (_dir, engine) = engine_with(&[
            ("page", "A@include(\"part\", [\"who\" => name])B@includeIf(\"ghost\")C@includeWhen(flag, \"cond\")D"),
            ("part", "[{{ who }}]"),
            ("cond", "(cond)"),
        ]);
        assert_eq!(
            render(&engine, "page", json!({ "name": "ada", "flag": true })),
            "A[ada]BC(cond)D"
        );
        assert_eq!(
            render(&engine, "page", json!({ "name": "ada", "flag": false })),
            "A[ada]BCD"
        );
        // Missing include without includeIf fails closed.
        let (_dir2, engine2) = engine_with(&[("page", "@include(\"ghost\")")]);
        let err = engine2.render("page", json!({})).unwrap_err();
        assert_eq!(err.code, VIEW_NOT_FOUND);
    }

    #[test]
    fn component_slots_and_props() {
        let (_dir, engine) = engine_with(&[
            (
                "alert",
                "@props([\"level\"])<div class=\"{{ level }}\"><h>{{ title }}</h><p>{{ slot }}</p></div>",
            ),
            (
                "page",
                "@component(\"alert\", [\"level\" => kind])@slot(\"title\")T@endslot BODY @endcomponent",
            ),
        ]);
        assert_eq!(
            render(&engine, "page", json!({ "kind": "warn" })),
            "<div class=\"warn\"><h>T</h><p> BODY </p></div>"
        );
    }

    #[test]
    fn push_and_stack() {
        let (_dir, engine) = engine_with(&[(
            "page",
            "@push(\"scripts\")s1@endpush@push(\"scripts\")s2@endpush[<x>@stack(\"scripts\")</x>]",
        )]);
        assert_eq!(render(&engine, "page", json!({})), "[<x>s1s2</x>]");
    }

    #[test]
    fn csrf_method_error_lang_js() {
        let (_dir, engine) = engine_with(&[(
            "page",
            "@csrf @method(\"PUT\") @error(\"email\")E:{{ message }}@enderror @lang(\"hi\") @js(cfg)",
        )]);
        let out = render(
            &engine,
            "page",
            json!({
                "_csrf": "tok&1",
                "_errors": { "email": "required" },
                "_lang": { "hi": "hello" },
                "cfg": { "a": 1 }
            }),
        );
        assert!(out.contains("name=\"_token\" value=\"tok&amp;1\""), "{out}");
        assert!(out.contains("name=\"_method\" value=\"PUT\""), "{out}");
        assert!(out.contains("E:required"), "{out}");
        assert!(out.contains("hello"), "{out}");
        assert!(out.contains("{\"a\":1}"), "{out}");
    }

    #[test]
    fn lang_falls_back_to_the_key() {
        let (_dir, engine) = engine_with(&[("page", "@lang(\"missing.key\")")]);
        assert_eq!(render(&engine, "page", json!({})), "missing.key");
    }

    #[test]
    fn class_style_and_attr_helpers() {
        let (_dir, engine) = engine_with(&[(
            "page",
            "<i @class([\"btn\", \"on\" => active]) @style([\"color: red\" => hot]) @checked(done) @disabled(off)>",
        )]);
        let out = render(
            &engine,
            "page",
            json!({ "active": true, "hot": false, "done": true, "off": false }),
        );
        assert!(out.contains("class=\"btn on\""), "{out}");
        assert!(!out.contains("style="), "{out}");
        assert!(out.contains("checked"), "{out}");
        assert!(!out.contains("disabled"), "{out}");
    }

    #[test]
    fn auth_guest_env_production() {
        let (_dir, engine) = engine_with(&[(
            "page",
            "@auth YA @else NA @endauth @guest YG @else NG @endguest @env(\"testing\") YT @else NT @endenv @production YP @else NP @endproduction",
        )]);
        let out = render(&engine, "page", json!({ "_auth": { "id": "u1" } }));
        assert!(out.contains("YA") && out.contains("NG"), "{out}");
        assert!(out.contains("YT") && out.contains("NP"), "{out}");
        assert!(!out.contains("NA") && !out.contains("YG"), "{out}");
        let out = render(&engine, "page", json!({}));
        assert!(out.contains("NA") && out.contains("YG"), "{out}");
        assert!(out.contains("YT") && out.contains("NP"), "{out}");
        assert!(!out.contains("YA") && !out.contains("NG"), "{out}");
    }

    #[test]
    fn custom_directives_registered_and_literal_fallback() {
        let (_dir, engine) = engine_with(&[("page", "@money(amt) @media (min)")]);
        engine.register_directive("money", |_name, arg, ctx| {
            let arg = arg.unwrap_or_default();
            let path = crate::expr::parse_path(arg.trim());
            let v = crate::expr::resolve_path(ctx, &path)
                .cloned()
                .unwrap_or(Value::Null);
            Ok(format!("$<{}>", crate::expr::to_display(&v)))
        });
        let out = render(&engine, "page", json!({ "amt": 5 }));
        assert!(out.contains("$<5>"), "{out}");
        // Unregistered directives render literally (Blade parity; keeps CSS
        // like @media intact).
        assert!(out.contains("@media(min)"), "{out}");
    }

    #[test]
    fn view_hooks_compose_context() {
        let (_dir, engine) = engine_with(&[("page", "{{ injected }}")]);
        engine.register_hook("page", |ctx| {
            if let Some(map) = ctx.as_object_mut() {
                map.insert("injected".into(), json!("via-hook"));
            }
        });
        assert_eq!(render(&engine, "page", json!({})), "via-hook");
    }

    #[test]
    fn compiled_cache_roundtrip_and_invalidation() {
        let (dir, engine) = engine_with(&[("page", "v1 {{ x }}")]);
        assert_eq!(render(&engine, "page", json!({ "x": 1 })), "v1 1");
        let cache_dir = dir.join("storage").join("framework").join("views");
        let compiled: Vec<_> = std::fs::read_dir(&cache_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
            .collect();
        assert_eq!(compiled.len(), 1, "one compiled entry expected");

        // Same content: cache hit (memo), identical output.
        assert_eq!(render(&engine, "page", json!({ "x": 2 })), "v1 2");

        // Drift: content hash changes -> reparse, new output.
        std::fs::write(
            dir.join("resources").join("views").join("page.html"),
            "v2 {{ x }}",
        )
        .unwrap();
        assert_eq!(render(&engine, "page", json!({ "x": 3 })), "v2 3");

        // precompile_all + clear_cache admin.
        assert_eq!(engine.precompile_all().unwrap(), 1);
        assert_eq!(engine.clear_cache().unwrap(), 1);
        assert_eq!(render(&engine, "page", json!({ "x": 4 })), "v2 4");
    }

    #[test]
    fn package_namespace_resolver() {
        let (dir, engine) = engine_with(&[]);
        let pkg_views = dir.join("packages").join("acme").join("views");
        std::fs::create_dir_all(&pkg_views).unwrap();
        std::fs::write(pkg_views.join("welcome.html"), "pkg says hi").unwrap();
        let root = pkg_views.clone();
        engine.set_resolver(move |ns, name| {
            if ns == "acme" {
                let p = root.join(name.replace('.', "/") + ".html");
                p.is_file().then_some(p)
            } else {
                None
            }
        });
        assert_eq!(render(&engine, "acme::welcome", json!({})), "pkg says hi");
        let err = engine.render("other::welcome", json!({})).unwrap_err();
        assert_eq!(err.code, VIEW_NOT_FOUND);
        // No resolver installed -> namespaces fail closed.
        let (_dir2, engine2) = engine_with(&[]);
        assert_eq!(
            engine2.render("acme::welcome", json!({})).unwrap_err().code,
            VIEW_NOT_FOUND
        );
    }

    #[test]
    fn superseded_directives_refused_at_render_entry() {
        let (_dir, engine) = engine_with(&[("page", "x @php evil @endphp")]);
        let err = engine.render("page", json!({})).unwrap_err();
        assert_eq!(err.code, VIEW_PARSE);
    }

    #[test]
    fn config_parses_views_section() {
        let cfg = ViewsConfig::from_toml(
            "[views]\npaths = [\"a\", \"b\"]\ncache_dir = \"c\"\nenv = \"dev\"\n",
        )
        .unwrap();
        assert_eq!(cfg.paths.len(), 2);
        assert_eq!(cfg.cache_dir.unwrap(), PathBuf::from("c"));
        assert_eq!(cfg.env, "dev");
        // Bare document form + defaults.
        let cfg = ViewsConfig::from_toml("paths = [\"x\"]").unwrap();
        assert_eq!(cfg.paths, vec![PathBuf::from("x")]);
        assert!(cfg.cache_dir.is_some());
    }
}
