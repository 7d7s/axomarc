//! Wave 4 Phase 5 (docs/19 L211): the config cache artifact.
//!
//! `keel config:cache` pins the frozen effective config to
//! `storage/framework/config.json` together with its content hash; boot
//! re-derives the effective config and hash-checks the artifact with
//! **drift-refuse semantics identical to the Wave 1 route cache**: absent
//! artifact boots live, a present artifact must match the current content
//! (config file + environment overlay), corrupt artifacts refuse boot
//! (fail-closed).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Artifact ABI: bump when the pinned config representation changes shape
/// (stale artifacts then refuse boot, mirroring `MODULE_ABI` on the route
/// cache).
pub const CONFIG_CACHE_ABI: u32 = 1;

/// Code every drift/corruption refusal names (mirrors the GRAPH-* gate
/// family and the route-cache refusal pointer).
pub const CONFIG_CACHE_CODE: &str = "CONFIG-CACHE-STALE";

/// Deployment cache artifact: the frozen effective config pinned to a
/// content hash (hash-mismatch-refuse boot, Wave 4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigCacheArtifact {
    pub abi: u32,
    pub config_hash: String,
    pub config: serde_json::Value,
}

impl ConfigCacheArtifact {
    /// Write the artifact (parent dirs created).
    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Load the artifact; `Ok(None)` when absent. Malformed content is an
    /// error (fail-closed: a corrupt cache refuses boot).
    pub fn load(path: &Path) -> Result<Option<Self>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let raw =
            std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let artifact: Self =
            serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Some(artifact))
    }

    /// True when this artifact pins exactly the given effective-config hash
    /// under the current ABI.
    pub fn matches_hash(&self, current_hash: &str) -> bool {
        self.abi == CONFIG_CACHE_ABI && self.config_hash == current_hash
    }
}

/// Canonical config-cache artifact location under a workspace root (the
/// route cache lives beside it at `storage/framework/routes.json`).
pub fn config_cache_path(root: &Path) -> PathBuf {
    root.join("storage").join("framework").join("config.json")
}

/// Content hash over the canonical effective-config JSON (the pin).
/// Deterministic: the JSON comes from one `PlatformConfig` serialization
/// order, so equal configs always hash equal.
pub fn config_hash(config_json: &serde_json::Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(config_json.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Boot gate: absent artifact -> `Ok(())` (live config); present artifact
/// must hash-match the current effective config, else boot is refused with
/// a pointer to `keel config:cache` / `keel config:clear`. Corrupt
/// artifacts refuse boot too.
pub fn config_cache_check(root: &Path, current_hash: &str) -> Result<(), String> {
    let path = config_cache_path(root);
    let Some(artifact) = ConfigCacheArtifact::load(&path)? else {
        return Ok(());
    };
    if !artifact.matches_hash(current_hash) {
        return Err(format!(
            "{CONFIG_CACHE_CODE}: config cache {} is stale (config file or environment drift); run keel config:cache to rebuild or keel config:clear to drop it",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> serde_json::Value {
        serde_json::json!({
            "app": { "name": "keel", "env": "dev" },
            "auth": { "jwt_secret": "dev-only-not-for-prod" },
        })
    }

    #[test]
    fn hash_is_deterministic_and_content_sensitive() {
        let a = config_hash(&sample());
        assert_eq!(a, config_hash(&sample()), "deterministic");
        assert_eq!(a.len(), 64, "sha256 hex");
        let mut changed = sample();
        changed["auth"]["jwt_secret"] = "rotated".into();
        assert_ne!(a, config_hash(&changed), "secret drift changes the hash");
    }

    #[test]
    fn check_accepts_matching_pin_and_absent_artifact() {
        let dir = std::env::temp_dir().join(format!(
            "keel-registry-configcache-ok-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let hash = config_hash(&sample());

        // Absent artifact: boots live, no opinion.
        config_cache_check(&dir, &hash).expect("absent cache is fine");

        let artifact = ConfigCacheArtifact {
            abi: CONFIG_CACHE_ABI,
            config_hash: hash.clone(),
            config: sample(),
        };
        let path = config_cache_path(&dir);
        artifact.write(&path).expect("artifact writes");
        config_cache_check(&dir, &hash).expect("matching pin accepted");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_refuses_drift_abi_mismatch_and_corruption() {
        let dir = std::env::temp_dir().join(format!(
            "keel-registry-configcache-stale-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = config_cache_path(&dir);

        // Config drift (file edit or env change) -> refuse with a pointer.
        let artifact = ConfigCacheArtifact {
            abi: CONFIG_CACHE_ABI,
            config_hash: config_hash(&sample()),
            config: sample(),
        };
        artifact.write(&path).unwrap();
        let err = config_cache_check(&dir, &config_hash(&serde_json::json!({"app": {}})))
            .expect_err("stale cache refuses boot");
        assert!(err.contains(CONFIG_CACHE_CODE) && err.contains("config:cache"));

        // ABI change -> refuse even when the hash matches.
        let stale_abi = ConfigCacheArtifact {
            abi: CONFIG_CACHE_ABI + 1,
            config_hash: config_hash(&sample()),
            config: sample(),
        };
        stale_abi.write(&path).unwrap();
        config_cache_check(&dir, &config_hash(&sample()))
            .expect_err("abi mismatch refuses boot");

        // Corrupt artifact -> refuse boot (fail closed).
        std::fs::write(&path, "{ not json").unwrap();
        config_cache_check(&dir, &config_hash(&sample()))
            .expect_err("corrupt artifact refuses boot");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
