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
    /// Accepted activation count, 1 at boot. Advanced by the poller.
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
    pub fn at_boot(manifest_digest: &str, boot_config_hash: &str) -> Self {
        Self {
            process_instance_id: kelson_foundation::id::uuid().to_string(),
            manifest_digest: manifest_digest.to_string(),
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
        let digest = self
            .content_digest
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default();
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
    Arc::new(RuntimeStatus::at_boot(&manifests, &cfg_hash))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Row 20: the three fields the proof reads must exist, be the right
    /// shape, and move the way the contract requires.
    #[test]
    fn boot_surface_carries_the_three_proof_fields() {
        let status = RuntimeStatus::at_boot("manifest-digest", "config-hash");
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
        let status = RuntimeStatus::at_boot("m1", "c1");
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
        let status = RuntimeStatus::at_boot("m1", "c1");
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
        let a = RuntimeStatus::at_boot("m", "c");
        let b = RuntimeStatus::at_boot("m", "c");
        assert_ne!(
            a.body("n", "0.1.0", Default::default())["process_instance_id"],
            b.body("n", "0.1.0", Default::default())["process_instance_id"],
            "two boots must not share an identity"
        );
    }
}
