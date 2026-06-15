// F6 wiring: the `sovereign domain` subcommand.
//
// Sub-modes:
//   - `sovereign domain add <host> --app <APP>` — register the
//     hostname, push a Caddy route, write a `domain` row.
//   - `sovereign domain list --app <APP>` — list the hostnames.
//
// `add` does not wait for a deploy; the operator adds the hostname
// after the first deploy of the app. Multiple hostnames per app are
// allowed (the `domain.hostname` UNIQUE constraint in the migration
// is across the whole table, not per app — which is the right
// behaviour: one hostname points at exactly one app).

use std::sync::Arc;

use anyhow::Context;
use sovereign_core::domain::NewDomain;
use sovereign_core::ports::{default_v0_host, StoragePort};
use sovereign_storage_sqlite::SqliteState;
use tracing::instrument;

use crate::cli::DomainCmd;
use crate::commands::Dispatch;
use crate::connect;
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

/// Run the `domain` subcommand. The clap parser gives us the inner
/// `DomainCmd`; we collapse the two variants into the right handler.
#[instrument(skip(out), fields(action))]
pub async fn run(cmd: &DomainCmd, out: &Output) -> Dispatch {
    let (action, hostname, app_name) = match cmd {
        DomainCmd::Add { hostname, app } => ("add", Some(hostname.clone()), app.clone()),
        DomainCmd::List { app } => ("list", None, app.clone()),
    };

    if app_name.is_empty() {
        return err(
            out,
            AppExit::Usage,
            "domain requires --app <NAME> (e.g. `sovereign domain add api.example.com --app api`)",
        );
    }

    let db_path = connect::default_db_path();
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
        "add" => {
            let host = hostname.unwrap_or_default();
            if host.is_empty() {
                return err(out, AppExit::Usage, "domain add requires a hostname");
            }
            run_add(&storage, &app.name, app.id, &host, out).await
        }
        _ => err(out, AppExit::Generic, "unknown domain action"),
    }
}

/// `sovereign domain add <host> --app <APP>`
async fn run_add(
    storage: &Arc<dyn StoragePort>,
    app_name: &str,
    app_id: sovereign_core::domain::AppId,
    host: &str,
    out: &Output,
) -> Dispatch {
    // 1. Push the Caddy route. We do this BEFORE the storage write so
    //    a Caddy failure does not leave a half-registered domain.
    //    The route targets 127.0.0.1:8080 (the V0 pinned host port).
    let proxy = connect::connect_proxy().await;
    let Some(proxy) = proxy else {
        return err(
            out,
            AppExit::Upstream,
            "caddy admin API not reachable; cannot wire a hostname. Start Caddy or set CADDY_ADMIN_URL.",
        );
    };
    if let Err(e) = proxy.add_route(host, 8080).await {
        return err(
            out,
            AppExit::Upstream,
            &format!("caddy add_route({host}) failed: {e}"),
        );
    }

    // 2. Write the domain row. tls_status is `valid` for V0 (Caddy's
    //    internal CA) and we leave tls_expires NULL — the F9 doctor
    //    will add real expiry tracking once we have a CA store.
    let new = NewDomain {
        app_id,
        hostname: host.to_string(),
    };
    let stored = match storage.add_domain(new, "cli").await {
        Ok(d) => d,
        Err(e) => {
            // The route is live but the DB write failed. Best-effort
            // cleanup: remove the route. If this fails too, the
            // operator gets a clear "see audit" message.
            let _ = proxy.remove_route(host).await;
            return err(
                out,
                AppExit::Generic,
                &format!("storage add_domain failed: {e}"),
            );
        }
    };

    // 3. Print the success envelope. The URL is what the operator
    //    opens in their browser (after adding the /etc/hosts entry
    //    for the .sovereign.local test hostnames).
    if out.format() == crate::output::Format::Text {
        let _ = out.text(&format!("{} is now reachable at https://{host}", app_name));
        let _ = out.text(&format!(
            "  (added to domain table; id={}, status={})",
            stored.id, stored.tls_status
        ));
    } else {
        let env = Envelope::<serde_json::Value>::ok(serde_json::json!({
            "app": app_name,
            "hostname": host,
            "url": format!("https://{host}"),
            "domain_id": stored.id,
            "tls_status": stored.tls_status,
            "default_v0_host": default_v0_host(&app_id.to_string()),
        }));
        let _ = out.success(&env);
    }
    Dispatch::Ok
}

/// `sovereign domain list --app <APP>`
async fn run_list(
    storage: &Arc<dyn StoragePort>,
    app_id: sovereign_core::domain::AppId,
    out: &Output,
) -> Dispatch {
    let domains = match storage.list_domains(app_id).await {
        Ok(d) => d,
        Err(e) => return err(out, AppExit::Generic, &format!("list_domains: {e}")),
    };
    if out.format() == crate::output::Format::Text {
        if domains.is_empty() {
            let _ = out.text("no hostnames registered for this app");
        } else {
            let _ = out.text("HOSTNAME                       TLS_STATUS  CREATED");
            for d in &domains {
                let _ = out.text(&format!(
                    "{:<30} {:<11} unix:{}",
                    d.hostname,
                    d.tls_status,
                    d.created_at.as_secs()
                ));
            }
        }
    } else {
        let env = Envelope::<serde_json::Value>::ok(serde_json::json!({
            "domains": domains.iter().map(|d| serde_json::json!({
                "id": d.id,
                "hostname": d.hostname,
                "tls_status": d.tls_status,
                "created_at": d.created_at,
            })).collect::<Vec<_>>(),
        }));
        let _ = out.success(&env);
    }
    Dispatch::Ok
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
