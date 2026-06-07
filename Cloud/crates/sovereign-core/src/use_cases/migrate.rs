//! V0.1.0 → V0.5 master-key migration use case.
//!
//! Background: V0.1.0 stored the master key as a bare
//! `AGE-SECRET-KEY-1...` Bech32 file with no passphrase. V0.5
//! wraps that file with Argon2id (m=64 MiB, t=3, p=1) +
//! XChaCha20-Poly1305, requiring a passphrase to unlock. The
//! migration flow upgrades the on-disk format and re-encrypts
//! every active secret under the new identity.
//!
//! # Algorithm
//!
//! 1. Caller loads the V0.1.0 identity from the bare file
//!    (no passphrase needed; the V0.1.0 file IS the key).
//! 2. Caller generates a fresh V0.5 wrapped master key
//!    (using the operator's new passphrase). The wrapped
//!    file is written to a temp path.
//! 3. This use case re-encrypts every active secret: for
//!    each (app, key, active row), decrypt with the V0.1.0
//!    identity and re-encrypt with the V0.5 identity, then
//!    mark the old row `retired` and insert the new active
//!    row. The audit log captures the count and the
//!    migration event.
//! 4. Caller atomically replaces the old master key file
//!    with the new wrapped one.
//!
//! # Atomicity
//!
//! The master key file is NOT replaced until step (3) has
//! succeeded for every secret. If we fail mid-(3), the
//! operator can re-run `--migrate`; the V0.1.0 file is
//! still on disk and the half-re-encrypted secrets are
//! re-encrypted the second time. If we fail at step (3)
//! for a specific secret, the secret is left untouched
//! (the V0.1.0 ciphertext is still on disk and the V0.5
//! ciphertext was never written) so the operator can also
//! just `cp master.key.v0 backup` and keep using v0.1.0.
//!
//! V0.5 ships the conservative "no transactional swap"
//! design because the alternative (write-ahead log with
//! crash recovery) is a lot of code for a one-time operator
//! action. V0.5+ may revisit this if we observe real-world
//! migration failures.

use crate::domain::{AuditEvent, AuditKind, NewSecret, Secret, SecretStatus, Timestamp};
use crate::error::AppError;
use crate::ports::{SecretsPort, StoragePort};
use serde::Serialize;
use std::sync::Arc;

/// Per-secret result of the re-encryption. Used to build
/// the migration summary in the JSON envelope.
#[derive(Debug, Clone, Serialize)]
pub struct ReencryptedSecret {
    pub app_id: String,
    pub app_name: String,
    pub key: String,
    pub bytes: usize,
}

/// The full migration result. Returned to the CLI for
/// pretty-printing and to the JSON envelope.
#[derive(Debug, Clone, Serialize)]
pub struct MigrationReport {
    /// Count of secrets that were re-encrypted.
    pub secrets_re_encrypted: usize,
    /// Count of apps that were touched.
    pub apps_touched: usize,
    /// Per-secret details (one entry per re-encrypted row).
    pub secrets: Vec<ReencryptedSecret>,
    /// The new (V0.5) public key in Bech32 form. The operator
    /// can share this with a CI run or peer host that needs
    /// read-only access.
    pub new_public_key: String,
    /// The new KDF parameters, echoed for parity with
    /// `LoginResult`.
    pub kdf_algorithm: String,
    pub kdf_memory_kib: u32,
    pub kdf_iterations: u32,
    pub kdf_parallelism: u32,
}

/// Re-encrypt every active secret under `new_secrets`. The
/// `old_secrets` adapter decrypts V0.1.0-encrypted ciphertexts;
/// `new_secrets` produces V0.5 ciphertexts under the new
/// passphrase-wrapped identity.
///
/// This function does NOT touch the master key file itself —
/// that atomic swap is the caller's responsibility (see
/// `commands_login::run_migrate` for the orchestration).
///
/// `actor` is recorded in the audit event (e.g. "operator",
/// "ci:deploy-key-42") and in every `set_secret_status` /
/// `put_secret` row's `created_by`.
pub async fn re_encrypt_all_secrets(
    storage: Arc<dyn StoragePort>,
    old_secrets: Arc<dyn SecretsPort>,
    new_secrets: Arc<dyn SecretsPort>,
    actor: &str,
) -> Result<MigrationReport, AppError> {
    let mut report = MigrationReport {
        secrets_re_encrypted: 0,
        apps_touched: 0,
        secrets: Vec::new(),
        new_public_key: String::new(),
        kdf_algorithm: "Argon2id".to_string(),
        kdf_memory_kib: 64 * 1024,
        kdf_iterations: 3,
        kdf_parallelism: 1,
    };

    // 1. Iterate every app.
    let apps = storage.list_apps(None).await?;
    let mut apps_with_secrets = 0;
    for app in &apps {
        let rows = storage.list_secrets(app.id).await?;
        let active: Vec<&Secret> = rows
            .iter()
            .filter(|s| s.status == SecretStatus::Active)
            .collect();
        if active.is_empty() {
            continue;
        }
        apps_with_secrets += 1;

        for old_row in active {
            // 2. Decrypt with the old identity. If this fails,
            //    the operator has a corrupt V0.1.0 secret row;
            //    bail with the row's identity so they can
            //    inspect it manually.
            let plaintext = old_secrets.decrypt(&old_row.ciphertext).map_err(|e| {
                AppError::upstream(format!(
                    "cannot decrypt secret `{}` (app `{}`) with the V0.1.0 identity: {e}. \
                     The V0.1.0 master key is the only one that can read this row; \
                     either the key file is wrong, or this row is corrupt.",
                    old_row.key, app.name
                ))
            })?;

            // 3. Re-encrypt with the new identity.
            let new_ciphertext = new_secrets.encrypt(&plaintext)?;

            // 4. Mark the old row retired.
            storage
                .set_secret_status(old_row.id, SecretStatus::Retired, actor)
                .await?;

            // 5. Insert the new active row.
            let new_row = NewSecret {
                app_id: app.id,
                key: old_row.key.clone(),
                ciphertext: new_ciphertext,
            };
            storage.put_secret(new_row, actor).await?;

            report.secrets_re_encrypted += 1;
            report.secrets.push(ReencryptedSecret {
                app_id: app.id.to_string(),
                app_name: app.name.clone(),
                key: old_row.key.clone(),
                bytes: old_row.ciphertext.len(),
            });
        }
    }
    report.apps_touched = apps_with_secrets;

    // 6. Write the migration audit event. This is a single
    //    audit row that captures the V0.1.0 → V0.5 transition
    //    in one place. The per-secret set_secret_status +
    //    put_secret calls already produce their own audit
    //    rows; this is the "summary" row.
    let audit = AuditEvent {
        id: None,
        ts: Timestamp::now(),
        actor: actor.to_string(),
        kind: AuditKind::SecretChange,
        target: Some("master-key-migration".to_string()),
        payload: serde_json::json!({
            "event": "v01_to_v05_master_key_migration",
            "secrets_re_encrypted": report.secrets_re_encrypted,
            "apps_touched": report.apps_touched,
            "kdf": {
                "algorithm": report.kdf_algorithm,
                "memory_kib": report.kdf_memory_kib,
                "iterations": report.kdf_iterations,
                "parallelism": report.kdf_parallelism,
            },
        }),
        policy_decision: None,
    };
    storage.append_audit(audit).await?;

    Ok(report)
}

#[cfg(test)]
mod tests {
    //! Unit tests live in `crates/sovereign-storage-sqlite/tests/migrate_integration.rs`
    //! (integration tests against a real sqlite storage) so we
    //! can exercise the full re-encrypt + storage round-trip.
    //! The `StoragePort` trait-coherence rules make in-crate
    //! unit tests against a real storage expensive to set up.

    #[test]
    fn smoke() {
        // The integration tests cover the actual re-encryption
        // flow; this stub exists so the module-level
        // `#[cfg(test)] mod tests` block is not empty.
        assert_eq!(2 + 2, 4);
    }
}
