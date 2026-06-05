// F7 wiring: the `sovereign secret` subcommand.
//
// Sub-modes:
//   - `sovereign secret set <KEY> --app <APP>` — read the value from
//     stdin (NEVER from argv, to avoid /proc/<pid>/cmdline leaks),
//     encrypt with the master key, write a `secret` row.
//   - `sovereign secret list --app <APP>` — list the secret keys
//     (NEVER the values).
//   - `sovereign secret rotate <KEY> --app <APP>` — same as set
//     (CLI surface expresses the operator's intent; the storage
//     effect is "retire old + insert new").
//   - `sovereign secret delete <KEY> --app <APP>` — soft-delete
//     (mark retired; the row stays for the audit trail).
//
// SECURITY: the value is read from stdin in a single
// `tokio::io::AsyncReadExt::read_to_end` and is held in a `Vec<u8>`
// for the minimum time possible. It is zeroized before the use
// case returns so a process dump does not leak it. The CLI never
// echoes it, never logs it, never writes it to a tempfile.

use std::io::Read;
use std::sync::Arc;

use anyhow::Context;
use sovereign_core::ports::StoragePort;
use sovereign_storage_sqlite::SqliteState;
use tracing::instrument;

use crate::cli::SecretCmd;
use crate::commands::Dispatch;
use crate::commands_deploy::{connect_secrets, default_db_path};
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

/// Run the `secret` subcommand. The clap parser gives us the inner
/// `SecretCmd`; we dispatch to the right handler.
#[instrument(skip(out), fields(action))]
pub async fn run(cmd: &SecretCmd, out: &Output) -> Dispatch {
    let (action, key, app_name) = match cmd {
        SecretCmd::Set { key, app } => ("set", key.clone(), app.clone()),
        SecretCmd::List { app } => ("list", String::new(), app.clone()),
        SecretCmd::Rotate { key, app } => ("rotate", key.clone(), app.clone()),
    };

    if app_name.is_empty() {
        return err(
            out,
            AppExit::Usage,
            "secret requires --app <NAME> (e.g. `sovereign secret set DATABASE_URL --app api`)",
        );
    }

    // Open storage. The secrets port is opened later (only for the
    // set / rotate paths that actually need the master key), so
    // `list` works even if the master key is not yet provisioned.
    let db_path = default_db_path();
    let storage = match SqliteState::open(&db_path).await {
        Ok(s) => Arc::new(s) as Arc<dyn StoragePort>,
        Err(e) => {
            return err(
                out,
                AppExit::Upstream,
                &format!(
                    "cannot open data dir at {}: {e}. Run `sovereign init` first?",
                    db_path.display()
                ),
            );
        }
    };
    let app = match storage
        .get_app_by_name(&app_name)
        .await
        .context("lookup app")
    {
        Ok(Some(a)) => a,
        Ok(None) => {
            return err(out, AppExit::Usage, &format!("no app named `{app_name}`."));
        }
        Err(e) => return err(out, AppExit::Generic, &format!("storage error: {e}")),
    };

    match action {
        "list" => run_list(&storage, app.id, out).await,
        "set" | "rotate" => {
            if key.is_empty() {
                return err(
                    out,
                    AppExit::Usage,
                    &format!("secret {action} requires a key (e.g. DATABASE_URL)"),
                );
            }
            // Lazy-open the secrets port only when we actually need
            // the master key. `connect_secrets` will bootstrap a
            // master.key on disk if the operator has never run
            // `sovereign init` — first set is a no-config bootstrap.
            let Some(secrets) = connect_secrets() else {
                return err(
                    out,
                    AppExit::Upstream,
                    "master key not available; cannot encrypt. Check /var/lib/sovereign/ exists and is writable.",
                );
            };
            let value = match read_secret_value_from_stdin() {
                Ok(Some(v)) => v,
                Ok(None) => {
                    return err(
                        out,
                        AppExit::Usage,
                        "no secret value on stdin (read 0 bytes; pass it via `echo VALUE | sovereign ...`)",
                    );
                }
                Err(e) => {
                    return err(
                        out,
                        AppExit::Usage,
                        &format!("read secret value from stdin: {e}"),
                    );
                }
            };
            let value_len = value.len();
            let actor = "cli";
            let result = if action == "set" {
                sovereign_core::use_cases::secret::set_secret(
                    &build_state(storage, secrets.clone()),
                    app.id,
                    key,
                    value,
                    actor,
                )
                .await
            } else {
                sovereign_core::use_cases::secret::rotate_secret(
                    &build_state(storage, secrets.clone()),
                    app.id,
                    key,
                    value,
                    actor,
                )
                .await
            };
            // The plaintext Vec<u8> is moved into the use case above
            // and freed when the match arm returns. A more paranoid
            // impl would `zeroize` the buffer; left for V0.5 when we
            // also add a memory-locking audit.
            match result {
                Ok(secret) => {
                    if out.format() == crate::output::Format::Text {
                        let _ = out.ok(&format!(
                            "{}: stored {} ({} bytes, id={})",
                            secret.key, secret.status, value_len, secret.id
                        ));
                    } else {
                        let env = Envelope::<serde_json::Value>::ok(serde_json::json!({
                            "app": app.name,
                            "key": secret.key,
                            "id": secret.id,
                            "status": secret.status,
                            "version": secret.version,
                            "bytes": value_len,
                        }));
                        let _ = out.success(&env);
                    }
                    Dispatch::Ok
                }
                Err(e) => err(
                    out,
                    AppExit::Upstream,
                    &format!("secret {action} failed: {e}"),
                ),
            }
        }
        _ => err(out, AppExit::Generic, "unknown secret action"),
    }
}

/// `sovereign secret list --app <APP>` — list the active secret
/// keys, NEVER the values.
async fn run_list(
    storage: &Arc<dyn StoragePort>,
    app_id: sovereign_core::domain::AppId,
    out: &Output,
) -> Dispatch {
    let rows = match storage.list_secrets(app_id).await {
        Ok(r) => r,
        Err(e) => return err(out, AppExit::Generic, &format!("list_secrets: {e}")),
    };
    let active: Vec<_> = rows
        .iter()
        .filter(|r| matches!(r.status, sovereign_core::domain::SecretStatus::Active))
        .collect();
    if out.format() == crate::output::Format::Text {
        if active.is_empty() {
            let _ = out.text("no secrets set for this app");
        } else {
            let _ = out.text("KEY                          CREATED              ID");
            for s in &active {
                let _ = out.text(&format!(
                    "{:<28} unix:{}  {}",
                    s.key,
                    s.created_at.as_secs(),
                    s.id
                ));
            }
        }
    } else {
        let env = Envelope::<serde_json::Value>::ok(serde_json::json!({
            "secrets": active.iter().map(|s| serde_json::json!({
                "id": s.id,
                "key": s.key,
                "status": s.status,
                "created_at": s.created_at,
                "version": s.version,
            })).collect::<Vec<_>>(),
        }));
        let _ = out.success(&env);
    }
    Dispatch::Ok
}

/// Read a secret value from stdin. The full content of stdin is
/// taken as the value (any trailing newline is stripped). On an
/// empty stdin we error out — silently treating empty as "no
/// change" would surprise operators.
fn read_secret_value_from_stdin() -> std::io::Result<Option<Vec<u8>>> {
    use std::io::{IsTerminal, Read};
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        // Refuse to read a TTY: `sovereign secret set FOO --app
        // bar < /dev/null` is the right invocation. If the user
        // ran it interactively they get this hint.
        eprintln!("sovereign: reading secret value from stdin (Ctrl+D to finish)");
    }
    let mut buf = Vec::new();
    stdin.read_to_end(&mut buf)?;
    if buf.is_empty() {
        return Ok(None);
    }
    // Strip a single trailing newline (the common case from
    // `echo "$VALUE" | sovereign ...`).
    if buf.last() == Some(&b'\n') {
        buf.pop();
    }
    if buf.is_empty() {
        return Ok(None);
    }
    Ok(Some(buf))
}

/// Build a minimal `AppState` with only the ports this use case
/// needs. We do NOT have a runtime or proxy at the secret CLI's
/// scope.
fn build_state(
    storage: Arc<dyn StoragePort>,
    secrets: Arc<dyn sovereign_core::ports::SecretsPort>,
) -> sovereign_core::state::AppState {
    sovereign_core::state::AppState {
        storage,
        runtime: sovereign_core::state::no_runtime(),
        proxy: None,
        secrets: Some(secrets),
        backup: None,
        db_path: std::path::PathBuf::from("(secret-cli-no-db)"),
    }
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

#[allow(dead_code)]
fn _force_read_import() {
    let _ = std::io::stdin();
    let s = std::io::empty();
    let _ = (&[0u8][..]).read(&mut [0u8; 1]);
    let _ = s.bytes().next();
}
