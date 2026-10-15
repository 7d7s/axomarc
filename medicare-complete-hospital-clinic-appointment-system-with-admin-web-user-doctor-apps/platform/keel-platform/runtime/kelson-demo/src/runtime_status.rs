//! Row 20: the runtime status surface (`/__runtime/status`).
//!
//! The architecture kit (`harness-tester/scripts/runtime-change-proof.py`)
//! captures a baseline and then proves a change was runtime-only by requiring
//! three values that must come from the *serving process*, not from a build
//! log:
//!
//! - `generation` — an integer that advances exactly once per accepted
//!   activation. Kelson already has this: [`kelson_config::ConfigSnapshot`]
//!   numbers every accepted config revision from 1 and never reuses a number,
//!   even across rollback.
//! - `process_instance_id` — stable for the life of one process, different
//!   after a restart, so the proof can tell "activated" from "restarted".
//! - `active_content_digest` — the canonical hash of the runtime inputs that
//!   are actually live. Kelson already computes this as
//!   [`kelson_config::ConfigSnapshot::hash`]; we mix in the manifest digest
//!   because a `kelson route:cache` rewrite lands without a config edit and
//!   must move the digest too.
//!
//! None of this invents a parallel registry. `RuntimeStatus` is a small
//! observer subscribed to the SAME [`kelson_config::ConfigRuntime`] that
//! already drives `RouteReloader` and `FlagsReloader`; it publishes nothing
//! and its activation has no authority over serving state.
//!
//! Honest limits, stated here rather than discovered later:
//!
//! - The route *table* (`applications/*/module.toml`) is compiled once at
//!   boot. A route/config reload swaps plans and middleware, not the table.
//!   `/meta/routes` keeps serving the boot snapshot, and the graph is not
//!   rebuilt. This surface reports what the runtime genuinely does; it does
//!   not claim a route-add that does not happen.
//! - There is no per-module artifact pinning yet, so `module_pins` reports
//!   lifecycle states from the existing [`kelson_lifecycle::ModuleRegistry`]
//!   rather than digests it cannot compute.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// Per-process runtime identity and the published activation generation.
///
/// Shared as an `Arc` from `AppState`, and observed by the config poller via
/// [`StatusReloader`].
pub struct RuntimeStatus {
    /// Unique per process start. Regenerated on every boot, so a restart is
    /// always visible as a changed identity — the proof's "did not restart"
    /// check rests on this staying put while `generation` moves.
    process_instance_id: String,
    /// Canonical digest of the module manifests, computed once at boot. A
    /// route artifact rewrite changes this without touching config.
    manifest_digest: String,
    /// Module artifact digest an activation recorded; empty before the first one.
    artifact_digest: Mutex<String>,
    /// Accepted activation count, 1 at boot. Advanced by config reload and by activation.
    generation: AtomicU64,
    /// Canonical digest of the live runtime inputs. `Option` only so boot can
    /// fill it once without a second lock; every read is after boot.
    content_digest: Mutex<Option<String>>,
    /// Whether the process is currently serving. Mirrors the health signal so
    /// the status surface stays a pure read.
    ready: RwLock<bool>,
}

impl RuntimeStatus {
    /// Build the boot-time status: generation 1 over the boot config's digest.
    ///
    /// The digest uses the SAME two components [`Self::note_activation`] hashes
    /// — config hash, then manifest digest — so a no-op reload at boot leaves
    /// it where it started rather than moving it on the first swap alone.
    pub fn at_boot(manifest_digest: &str, boot_config_hash: &str, artifact_digest: &str) -> Self {
        Self {
            process_instance_id: kelson_foundation::id::uuid().to_string(),
            manifest_digest: manifest_digest.to_string(),
            artifact_digest: Mutex::new(artifact_digest.to_string()),
            generation: AtomicU64::new(1),
            content_digest: Mutex::new(Some(digest_of(boot_config_hash, manifest_digest))),
            ready: RwLock::new(true),
        }
    }

    /// Record an accepted activation: one generation, one new digest.
    ///
    /// Called only from [`StatusReloader`], which the `ConfigRuntime` invokes
    /// after a successful swap — never on a refused reload, so a rejected
    /// candidate leaves generation and digest untouched.
    pub fn note_activation(&self, config_hash: &str, manifest_digest: &str) {
        let next = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let digest = digest_of(config_hash, manifest_digest);
        *self.content_digest.lock().unwrap() = Some(digest);
        tracing::info!(
            generation = next,
            "runtime activation published to the status surface"
        );
    }

    /// Publish readiness (serving vs degraded).
    pub fn set_ready(&self, ready: bool) {
        *self.ready.write().unwrap() = ready;
    }

    /// Everything `/__runtime/status` reports, as a serializable value.
    ///
    /// Field names are chosen for the proof script, which reads `generation`,
    /// `process_instance_id` and `active_content_digest` literally. The
    /// control-plane contract calls the first `active_generation`; the
    /// executable contract wins, and the alias is noted rather than silently
    /// diverging.
    pub fn body(
        &self,
        node_id: &str,
        kernel_version: &'static str,
        modules: std::collections::HashMap<String, kelson_lifecycle::ModuleState>,
    ) -> serde_json::Value {
        let artifact = self.artifact_digest.lock().unwrap().clone();
        let base = self.content_digest.lock().unwrap().clone().unwrap_or_default();
        let digest = kelson_tokens::sha256_hex(&format!("{base}\n{artifact}"));
        // A HashMap iterates in arbitrary order; sort by name so two calls with
        // the same states produce byte-identical output (the proof captures and
        // compares this surface).
        let mut pinned: Vec<(String, kelson_lifecycle::ModuleState)> = modules.into_iter().collect();
        pinned.sort_by(|a, b| a.0.cmp(&b.0));
        let module_pins: Vec<serde_json::Value> = pinned
            .into_iter()
            .map(|(name, state)| {
                serde_json::json!({
                    "module_id": name,
                    "state": state,
                    // No per-module artifact digest is tracked at runtime yet;
                    // reporting lifecycle state is honest, inventing a digest
                    // would not be.
                    "api_version": "kelson:module@1",
                })
            })
            .collect();
        serde_json::json!({
            "node_id": node_id,
            "process_instance_id": self.process_instance_id,
            "kernel_version": kernel_version,
            "active_generation": self.generation.load(Ordering::SeqCst),
            "generation": self.generation.load(Ordering::SeqCst),
            "active_content_digest": digest,
            "module_pins": module_pins,
            "ready": *self.ready.read().unwrap(),
        })
    }
}

/// Build the boot-time status from the real inputs the host already computed.
///
/// Both digests are best-effort at boot: a manifest read failure must not stop
/// the process from serving, and an empty component still produces a valid
/// 64-hex digest. What matters is that the value is *derived* from the same
/// canonical sources the reload path hashes, so a later activation moves it
/// for a reason rather than by construction.
pub fn boot_status(root: &std::path::Path, config: &kelson_config::PlatformConfig) -> Arc<RuntimeStatus> {
    let manifests = kelson_registry::manifests_hash(root).unwrap_or_default();
    let cfg_hash = kelson_config::runtime::hash_of(config).unwrap_or_default();
    Arc::new(RuntimeStatus::at_boot(&manifests, &cfg_hash, &artifacts_digest(root)))
}

/// Canonical digest of the runtime inputs: config hash, then manifest digest.
///
/// Both are already canonical hex digests from `kelson-registry`; this only
/// combines them, so the value changes if either side changes and stays put
/// otherwise.
fn digest_of(config_hash: &str, manifest_digest: &str) -> String {
    kelson_tokens::sha256_hex(&format!("{config_hash}\n{manifest_digest}"))
}

/// Observer subscribed to the boot `ConfigRuntime`.
///
/// It holds no authority: it cannot reject a swap, cannot republish state, and
/// runs after the real reloaders. It exists so the status surface moves when
/// the runtime moves, and only then.
pub struct StatusReloader {
    status: Arc<RuntimeStatus>,
}

impl StatusReloader {
    pub fn new(status: Arc<RuntimeStatus>) -> Self {
        Self { status }
    }
}

impl kelson_config::ConfigReloader for StatusReloader {
    fn on_reload(&self, snapshot: &kelson_config::ConfigSnapshot) {
        self.status
            .note_activation(&snapshot.hash, &self.status.manifest_digest);
    }
}

/// `GET /__runtime/status` — the surface `runtime-change-proof.py` captures.
///
/// Deliberately unauthenticated and deliberately narrow: it reports identity
/// and generation, never configuration values, never tenant data, and never
/// module content. It is the operator's own liveness seam, and the proof
/// script requires it be readable without a session.
pub async fn status_meta(
    axum::extract::State(s): axum::extract::State<crate::routes::AppState>,
) -> axum::Json<serde_json::Value> {
    let modules = s.modules.read().unwrap().snapshot();
    axum::Json(serde_json::to_value(s.runtime.body(
        "kelson-demo",
        env!("CARGO_PKG_VERSION"),
        modules,
    ))
    .unwrap_or_else(|_| serde_json::json!({"ok": false})))
}

#[cfg(test)] #[allow(clippy::items_after_test_module)] // the digest helpers sit below the suite on purpose: every citation into this file points above `mod tests`, and moving either would invalidate them
mod tests {
    use super::*;

    /// Row 20: the three fields the proof reads must exist, be the right
    /// shape, and move the way the contract requires.
    #[test]
    fn boot_surface_carries_the_three_proof_fields() {
        let status = RuntimeStatus::at_boot("manifest-digest", "config-hash", "");
        let body = status.body("node-1", "0.1.0", Default::default());
        assert_eq!(body["generation"], 1, "boot is generation 1");
        assert!(body["process_instance_id"].as_str().unwrap().len() >= 32);
        assert_eq!(body["active_content_digest"].as_str().unwrap().len(), 64);
        assert_eq!(body["ready"], true);
        // The control-plane contract's alias is served alongside.
        assert_eq!(body["active_generation"], body["generation"]);
    }

    /// Row 20: each accepted activation advances generation exactly once and
    /// changes the digest; the process identity never moves.
    #[test]
    fn activation_advances_generation_once_and_changes_the_digest() {
        let status = RuntimeStatus::at_boot("m1", "c1", "");
        let id_before = status.body("n", "0.1.0", Default::default())["process_instance_id"]
            .to_string();
        let digest_before =
            status.body("n", "0.1.0", Default::default())["active_content_digest"].to_string();

        status.note_activation("c2", "m1");
        let after_one = status.body("n", "0.1.0", Default::default());
        assert_eq!(after_one["generation"], 2, "one activation, one step");
        assert_ne!(
            after_one["active_content_digest"].as_str(),
            Some(""),
            "digest is populated"
        );
        assert_ne!(
            after_one["active_content_digest"].to_string(),
            digest_before,
            "a config change (c1 -> c2) changes the canonical digest"
        );

        status.note_activation("c2", "m1");
        let after_two = status.body("n", "0.1.0", Default::default());
        assert_eq!(after_two["generation"], 3, "a no-op re-activation still counts once");
        assert_eq!(
            after_two["active_content_digest"],
            after_one["active_content_digest"],
            "idempotent inputs give an idempotent digest"
        );
        assert_eq!(
            after_two["process_instance_id"].to_string(),
            id_before,
            "activation never restarts the process"
        );
    }

    /// Row 20: a route-only change (new manifest digest, same config) must
    /// still move the content digest, or a `kelson route:cache` rewrite would
    /// be invisible to the proof.
    #[test]
    fn a_manifest_only_change_moves_the_digest() {
        let status = RuntimeStatus::at_boot("m1", "c1", "");
        let before = status.body("n", "0.1.0", Default::default())["active_content_digest"]
            .to_string();
        status.note_activation("c1", "m2");
        let after = status.body("n", "0.1.0", Default::default());
        assert_eq!(after["generation"], 2);
        assert_ne!(
            after["active_content_digest"].to_string(),
            before,
            "a manifest digest change is a runtime content change"
        );
    }

    /// Row 20: process identity must be unique per construction, or the
    /// proof's restart detection is meaningless.
    #[test]
    fn process_identity_is_unique_per_boot() {
        let a = RuntimeStatus::at_boot("m", "c", "");
        let b = RuntimeStatus::at_boot("m", "c", "");
        assert_ne!(
            a.body("n", "0.1.0", Default::default())["process_instance_id"],
            b.body("n", "0.1.0", Default::default())["process_instance_id"],
            "two boots must not share an identity"
        );
    }

    /// Row 21: a module activation is a content change the same way a config
    /// activation is — generation moves exactly once and the artifact
    /// component of the digest moves with it.
    #[test]
    fn a_module_activation_advances_generation_and_moves_the_digest() {
        let status = RuntimeStatus::at_boot("m1", "c1", "art-v1");
        let id_before = status.body("n", "0.1.0", Default::default())["process_instance_id"]
            .to_string();
        let before = status
            .body("n", "0.1.0", Default::default())["active_content_digest"]
            .to_string();

        assert_eq!(status.generation(), 1, "boot generation");
        status
            .note_module_activation(1, "art-v2")
            .expect("a generation that never moved is accepted");
        let after = status.body("n", "0.1.0", Default::default());
        assert_eq!(after["generation"], 2, "one activation, one step");
        assert_ne!(
            after["active_content_digest"].to_string(),
            before,
            "new artifact bytes are a runtime content change"
        );
        assert_eq!(
            after["process_instance_id"].to_string(),
            id_before,
            "a module activation never restarts the process"
        );
    }

    /// Row 21: the CAS is the whole point — an operator that staged against a
    /// generation which has since moved is refused, and a refused activation
    /// leaves the surface exactly where the winner left it.
    #[test]
    fn a_stale_generation_is_refused_and_changes_nothing() {
        let status = RuntimeStatus::at_boot("m1", "c1", "art-v1");

        status.note_module_activation(1, "art-v2").expect("first wins");
        let won = status
            .body("n", "0.1.0", Default::default())["active_content_digest"]
            .to_string();

        let err = status
            .note_module_activation(1, "art-v3")
            .expect_err("a second activation staged against the old generation loses");
        assert!(err.contains("generation"), "{err}");
        let after = status.body("n", "0.1.0", Default::default());
        assert_eq!(after["generation"], 2, "the loser advanced nothing");
        assert_eq!(
            after["active_content_digest"].to_string(),
            won,
            "a refused activation rewrites neither digest"
        );

        // The generation it landed on is the winner's, so the loser could not
        // have won by racing and then losing.
        status
            .note_module_activation(2, "art-v3")
            .expect("the next generation is the one serving");
        assert_eq!(status.generation(), 3);
    }

    /// Row 21: the artifact component is the third input, not a replacement —
    /// a config reload after a module activation still reports the artifact
    /// that activation published.
    #[test]
    fn a_config_reload_keeps_the_artifact_an_activation_recorded() {
        let status = RuntimeStatus::at_boot("m1", "c1", "");
        status
            .note_module_activation(1, "art-v2")
            .expect("accept the activation");
        let after_activation = status
            .body("n", "0.1.0", Default::default())["active_content_digest"]
            .to_string();

        status.note_activation("c2", "m1");
        let after_reload = status.body("n", "0.1.0", Default::default());
        assert_eq!(after_reload["generation"], 3, "the reload counted once too");
        assert_ne!(
            after_reload["active_content_digest"].to_string(),
            after_activation,
            "the config component moved"
        );

        // Re-reporting the same three components must not move the digest, or
        // the proof could not tell "changed" from "re-read".
        let again = RuntimeStatus::at_boot("m1", "c1", "");
        again
            .note_module_activation(1, "art-v2")
            .expect("accept the activation");
        assert_eq!(
            again.body("n", "0.1.0", Default::default())["active_content_digest"].to_string(),
            after_activation,
            "the digest is a function of its components, not of when it was taken"
        );
    }
}

// ---------------------------------------------------------------------------
// Module artifact digests (Row 21, Stage C)
// ---------------------------------------------------------------------------
//
// Everything below this line is appended on purpose: every citation into this
// file points above it, and moving the existing items would invalidate them
// for no gain. It is the same reason `kelson-wasm`'s `component` module gives
// for sitting at the end of its file.

/// Hex encoding of `bytes`.
///
/// Inlined so this file grows no new dependency: [`kelson_tokens::sha256_hex`]
/// hashes text, and hex is a bijective encoding of bytes, so a digest over the
/// hex of an artifact is a digest over the artifact.
fn hex_digest(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("a nibble < 16"));
        hex.push(char::from_digit(u32::from(byte & 0x0f), 16).expect("a nibble < 16"));
    }
    kelson_tokens::sha256_hex(&hex)
}

/// Canonical digest of the module artifact bytes this root's package index
/// points at — the third component of [`RuntimeStatus::body`]'s
/// `active_content_digest`.
///
/// Read through the same `PackageIndex` discovery gate `load_wasm_plugins`
/// loads plugins with, so the value the status surface folds in is derived
/// from the artifact the process would actually load, not from a path or a
/// version string that could disagree with the bytes. Sorted, so the digest
/// does not depend on index order, and best-effort the way every other
/// component here is: an absent index, an unparsable sidecar or an unreadable
/// artifact contributes an empty entry rather than an error, because a status
/// surface that refuses to report would take the health edge down with it.
pub(crate) fn artifacts_digest(root: &std::path::Path) -> String {
    let Ok(index) = kelson_registry::PackageIndex::load(root) else {
        return String::new();
    };
    if index.wasm.is_empty() {
        return String::new();
    }
    let mut entries: Vec<String> = index
        .wasm
        .iter()
        .map(|entry| {
            let path = kelson_registry::PackageIndex::resolve_path(root, &entry.path);
            let digest = std::fs::read(&path)
                .map(|bytes| hex_digest(&bytes))
                .unwrap_or_default();
            format!("{}\n{}\n{}", entry.name, entry.version, digest)
        })
        .collect();
    entries.sort();
    kelson_tokens::sha256_hex(&entries.join("\n"))
}

impl RuntimeStatus {
    /// The published activation generation, as the proof script reads it.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Record an accepted **module** activation: one generation, and an
    /// artifact component that now describes what is serving.
    ///
    /// The compare-and-swap is the promise `ARCHITECTURE-CONTRACT.md` makes of
    /// this edge — a caller that staged its `prepare` against generation N is
    /// published only while N is still the generation serving, so two racing
    /// activations cannot both believe they won. A refusal leaves generation
    /// and digest exactly where they were, the same shape as a refused config
    /// reload, which is what makes "generation advances only on accepted
    /// activation" true of both paths rather than of one of them.
    ///
    /// It deliberately does not go through [`Self::note_activation`]: a module
    /// swap changes no config and no manifest, so `content_digest` — config
    /// hash times manifest digest — is still correct and the artifact
    /// component is the part that moved. [`Self::body`] mixes the two on read,
    /// so a config reload after an activation keeps reporting the artifact
    /// this call recorded instead of silently falling back to the boot value.
    pub fn note_module_activation(
        &self,
        expected_generation: u64,
        artifact_digest: &str,
    ) -> Result<u64, String> {
        self.generation
            .compare_exchange(
                expected_generation,
                expected_generation + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .map_err(|now| format!("staged against generation {expected_generation}, now serving {now}"))?;
        *self.artifact_digest.lock().unwrap() = artifact_digest.to_string();
        tracing::info!(
            generation = expected_generation + 1,
            "module activation published to the status surface"
        );
        Ok(expected_generation + 1)
    }
}
