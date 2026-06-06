use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use tracing::{info, warn};

use sovereign_core::ports::{
    update::Binary, Release, Sha256, UpdateChannel, UpdateError, UpdatePort, UpdateRecord, Version,
};

/// The in-tree `UpdatePort` implementation. Uses `reqwest` to
/// download the manifest and the binary tarball. The manifest URL
/// is `https://releases.sovereignruntime.dev/{channel}.json` and
/// the binary URL is the manifest's `binaries[<triple>].url`.
pub struct HttpUpdate {
    manifest_base: String,
    client: Client,
    current_version: Version,
    /// `sovereign-storage-sqlite` impl. Wrapped in `Option` so
    /// unit tests that don't need a DB can construct a `HttpUpdate`
    /// without one.
    storage: Option<Arc<dyn sovereign_core::ports::StoragePort>>,
}

impl HttpUpdate {
    pub fn new(manifest_base: impl Into<String>) -> Self {
        Self {
            manifest_base: manifest_base.into(),
            client: Client::new(),
            current_version: Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 1, 0)),
            storage: None,
        }
    }

    pub fn with_storage(
        manifest_base: impl Into<String>,
        storage: Arc<dyn sovereign_core::ports::StoragePort>,
    ) -> Self {
        Self {
            manifest_base: manifest_base.into(),
            client: Client::new(),
            current_version: Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 1, 0)),
            storage: Some(storage),
        }
    }

    pub fn storage(mut self, storage: Arc<dyn sovereign_core::ports::StoragePort>) -> Self {
        self.storage = Some(storage);
        self
    }

    fn manifest_url(&self, channel: UpdateChannel) -> String {
        format!(
            "{}/{}.json",
            self.manifest_base.trim_end_matches('/'),
            channel.as_str()
        )
    }
}

#[async_trait]
impl UpdatePort for HttpUpdate {
    async fn current_version(&self) -> Result<Version, UpdateError> {
        Ok(self.current_version.clone())
    }

    async fn latest(&self, channel: UpdateChannel) -> Result<Release, UpdateError> {
        let url = self.manifest_url(channel);
        info!("fetching update manifest from {}", url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(UpdateError::Network(format!(
                "{} returned {}",
                url,
                resp.status()
            )));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        serde_json::from_str(&body).map_err(|e| UpdateError::ManifestParse(e.to_string()))
    }

    async fn fetch(
        &self,
        release: &Release,
        target_triple: &str,
        dest: &Path,
    ) -> Result<Sha256, UpdateError> {
        let binary: &Binary = release
            .binaries
            .get(target_triple)
            .ok_or_else(|| UpdateError::UnsupportedTriple(target_triple.to_string()))?;
        info!("downloading {} -> {}", binary.url, dest.display());
        let resp = self
            .client
            .get(&binary.url)
            .send()
            .await
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(UpdateError::Network(format!(
                "{} returned {}",
                binary.url,
                resp.status()
            )));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                UpdateError::ReadFailed(parent.display().to_string(), e.to_string())
            })?;
        }
        tokio::fs::write(dest, &bytes)
            .await
            .map_err(|e| UpdateError::ReadFailed(dest.display().to_string(), e.to_string()))?;
        let actual = Sha256::compute(&bytes);
        if actual.to_string() != binary.sha256 {
            warn!(
                "sha256 mismatch: expected {} got {}; removing partial download",
                binary.sha256, actual
            );
            let _ = tokio::fs::remove_file(dest).await;
            return Err(UpdateError::ChecksumMismatch {
                expected: binary.sha256.clone(),
                actual: actual.to_string(),
            });
        }
        Ok(actual)
    }

    async fn apply(
        &self,
        new_binary: &Path,
        current_binary: &Path,
        backup_dir: &Path,
    ) -> Result<UpdateRecord, UpdateError> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| UpdateError::Network("no storage configured".into()))?;
        sovereign_core::use_cases::update::apply_update(
            new_binary,
            current_binary,
            backup_dir,
            storage.clone(),
        )
        .await
    }

    async fn rollback(&self, record: &UpdateRecord) -> Result<(), UpdateError> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| UpdateError::Network("no storage configured".into()))?;
        // The caller is responsible for telling us the running
        // binary path; for the trait default we re-derive it from
        // std::env::current_exe().
        let current = std::env::current_exe()
            .map_err(|e| UpdateError::ReadFailed("current_exe".into(), e.to_string()))?;
        sovereign_core::use_cases::update::rollback_update(record, &current, storage.clone()).await
    }
}
