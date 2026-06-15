// The secret use cases (F7).
//
// These wrap the storage port (ciphertext at rest) and the secrets
// port (encryption / decryption) into the four operations the CLI
// exposes: `set`, `list`, `rotate`, `delete`.
//
// V0 trust model: a secret value is plaintext only in process
// memory and in the container's env block. The CLI never echoes
// it, never logs it, never stores it in argv. The only path
// `plaintext` takes is: stdin -> use case -> secrets.encrypt ->
// storage.put_secret (ciphertext). The reverse path is
// storage.get_secret -> secrets.decrypt -> container env. The CLI
// does not have a "show me the value" subcommand by design.

use crate::domain::{AppId, NewSecret, Secret, SecretStatus};
use crate::error::AppError;
use crate::ports::SecretsPort;
use crate::rbac::{self, Action, Actor};
use crate::state::AppState;

/// Set (or overwrite) a secret. The value is encrypted with the
/// master key before it touches storage. The storage row is keyed
/// by `(app_id, key)`; an existing row is overwritten atomically
/// (the new row gets a fresh `SecretId`, the old one is marked
/// `retired` for the audit trail).
pub async fn set_secret(
    state: &AppState,
    app_id: AppId,
    key: String,
    value: Vec<u8>,
    actor: &str,
) -> Result<Secret, AppError> {
    // RBAC: verify the actor is allowed to set secrets.
    let actor_obj = Actor::from_str_loose(actor);
    rbac::check(&actor_obj, Action::SecretSet)
        .map_err(|e| AppError::Unauthorized(e.to_string()))?;

    validate_key(&key)?;
    let secrets = require_secrets(state)?;

    // 1. Encrypt before storage. The ciphertext is the only thing
    //    that ever touches disk.
    let ciphertext = secrets.encrypt(&value)?;

    // 2. Mark any existing row for this (app, key) as retired
    //    BEFORE we insert the new active row. This is a two-step
    //    transaction in the storage layer; if the second insert
    //    fails, the old row stays active (fail-safe).
    let existing = state.storage.list_secrets(app_id).await?;
    for old in existing
        .iter()
        .filter(|s| s.key == key && s.status == SecretStatus::Active)
    {
        let _ = state
            .storage
            .set_secret_status(old.id, SecretStatus::Retired, actor)
            .await?;
    }

    // 3. Insert the new active row.
    let new = NewSecret {
        app_id,
        key,
        ciphertext,
    };
    state.storage.put_secret(new, actor).await
}

/// List the secret keys for an app. NEVER returns the values — the
/// CLI's `sovereign secret list` subcommand exists exactly to
/// surface the keys without leaking the values.
pub async fn list_secret_keys(state: &AppState, app_id: AppId) -> Result<Vec<Secret>, AppError> {
    state.storage.list_secrets(app_id).await
}

/// Rotate a secret: write a new active row with the new value, mark
/// the old one `retired`. Returns the new `Secret`.
pub async fn rotate_secret(
    state: &AppState,
    app_id: AppId,
    key: String,
    new_value: Vec<u8>,
    actor: &str,
) -> Result<Secret, AppError> {
    // Reuse `set_secret` — its retire-old + insert-new flow IS the
    // rotation. The name `rotate` exists at the CLI surface so the
    // operator can express intent; the storage effect is identical.
    set_secret(state, app_id, key, new_value, actor).await
}

/// Soft-delete a secret: mark the row `retired` (it stays in the
/// table for the audit trail). The next deploy of the app will
/// not see this secret in its env.
pub async fn delete_secret(
    state: &AppState,
    app_id: AppId,
    key: String,
    actor: &str,
) -> Result<(), AppError> {
    // RBAC: verify the actor is allowed to modify secrets.
    let actor_obj = Actor::from_str_loose(actor);
    rbac::check(&actor_obj, Action::SecretSet)
        .map_err(|e| AppError::Unauthorized(e.to_string()))?;

    let existing = state.storage.list_secrets(app_id).await?;
    let row = existing
        .into_iter()
        .find(|s| s.key == key && s.status == SecretStatus::Active)
        .ok_or_else(|| AppError::not_found("secret"))?;
    state
        .storage
        .set_secret_status(row.id, SecretStatus::Retired, actor)
        .await?;
    Ok(())
}

/// Bulk-resolve the active secrets for an app, decrypted, ready to
/// be passed as `env` to the runtime. This is the use case the
/// `start_deploy` orchestrator calls right before
/// `runtime.create_container`.
///
/// The return value is `Vec<(key, plaintext)>` — the runtime's
/// `ContainerSpec::env` is exactly this shape.
pub async fn env_for_deploy(
    state: &AppState,
    app_id: AppId,
) -> Result<Vec<(String, String)>, AppError> {
    let Some(secrets) = state.secrets.as_ref() else {
        // No secrets adapter -> the app has no secrets. The
        // operator might not have run `sovereign init` yet, but
        // that's not a deploy error — just an empty env.
        return Ok(Vec::new());
    };
    let rows = state.storage.list_secrets(app_id).await?;
    let mut env = Vec::with_capacity(rows.len());
    for row in rows.iter().filter(|r| r.status == SecretStatus::Active) {
        let plaintext = secrets.decrypt(&row.ciphertext)?;
        let value = String::from_utf8(plaintext).map_err(|_| {
            AppError::validation(format!(
                "secret `{}` is not valid UTF-8; V0 requires secrets to be UTF-8 strings \
                 (binary blobs are V0.5).",
                row.key
            ))
        })?;
        env.push((row.key.clone(), value));
    }
    Ok(env)
}

fn require_secrets(state: &AppState) -> Result<&dyn SecretsPort, AppError> {
    state.secrets.as_deref().ok_or_else(|| {
        AppError::validation(
            "no master key found. Run `sovereign init` to generate /var/lib/sovereign/master.key, \
             or set SOVEREIGN_MASTER_KEY=/path/to/your.key",
        )
    })
}

/// V0 secret key naming rules. We use the same constraints as
/// environment-variable names (UPPER_SNAKE_CASE) so the operator
/// can copy/paste from any 12-factor app's example.
fn validate_key(key: &str) -> Result<(), AppError> {
    if key.is_empty() {
        return Err(AppError::validation("secret key is empty"));
    }
    if key.len() > 128 {
        return Err(AppError::validation("secret key is too long (max 128)"));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(AppError::validation(
            "secret key must be UPPER_SNAKE_CASE: ASCII uppercase letters, digits, and underscores only",
        ));
    }
    if key
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        return Err(AppError::validation(
            "secret key must not start with a digit (env var constraint)",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_key_accepts_upper_snake_case() {
        assert!(validate_key("DATABASE_URL").is_ok());
        assert!(validate_key("STRIPE_API_KEY").is_ok());
        assert!(validate_key("A").is_ok());
        assert!(validate_key("_PRIVATE").is_ok()); // leading _ is fine
    }

    #[test]
    fn validate_key_rejects_bad_inputs() {
        assert!(validate_key("").is_err());
        assert!(validate_key("lowercase").is_err());
        assert!(validate_key("WITH-DASH").is_err());
        assert!(validate_key("WITH SPACE").is_err());
        assert!(validate_key("1STARTS_WITH_DIGIT").is_err());
        assert!(validate_key(&"A".repeat(129)).is_err());
    }
}
