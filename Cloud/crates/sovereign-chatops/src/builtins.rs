// P7: Built-in chatops commands — /help, /status, /apps, /doctor,
// /secret, /deploy, /rollback, /backup, /deployments, /start (binding).

use std::sync::Arc;

use async_trait::async_trait;

use crate::commands::{
    BindingCodeStore, BindingStore, ChatopsError, Command, CommandContext, CommandResponse,
    CommandRegistry,
};
use crate::telegram::TelegramClient;
use crate::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use sovereign_core::ports::StoragePort;

/// /help — list available commands.
pub struct HelpCmd {
    pub registry: Arc<tokio::sync::RwLock<CommandRegistry>>,
}

#[async_trait]
impl Command for HelpCmd {
    fn name(&self) -> &'static str { "help" }
    fn requires_binding(&self) -> bool { false }

    async fn run(&self, _ctx: &CommandContext, _args: &[String]) -> Result<CommandResponse, ChatopsError> {
        let reg = self.registry.read().await;
        let names = reg.command_names();
        let mut lines = vec!["Available commands:".to_string()];
        for name in &names {
            lines.push(format!("  /{name}"));
        }
        Ok(CommandResponse::text(lines.join("\n")))
    }
}

/// /start <code> — bind Telegram chat to sovereign user.
pub struct StartCmd {
    pub binding_codes: Arc<dyn BindingCodeStore>,
    pub bindings: Arc<dyn BindingStore>,
}

#[async_trait]
impl Command for StartCmd {
    fn name(&self) -> &'static str { "start" }
    fn requires_binding(&self) -> bool { false }

    async fn run(&self, ctx: &CommandContext, args: &[String]) -> Result<CommandResponse, ChatopsError> {
        if args.is_empty() {
            return Ok(CommandResponse::text("Usage: /start <6-digit-code>\nGet the code from: sovereign chatops init"));
        }
        let code = &args[0];
        match self.binding_codes.consume_code(code).await? {
            Some(user_id) => {
                self.bindings.bind(ctx.chat_id, &user_id).await?;
                Ok(CommandResponse::text(format!("Bound to user {user_id}. You can now use bot commands.")))
            }
            None => Ok(CommandResponse::text("Invalid or expired code. Get a new one: sovereign chatops init")),
        }
    }
}

/// /status — fleet summary. Real implementation queries StoragePort.
pub struct StatusCmd;

#[async_trait]
impl Command for StatusCmd {
    fn name(&self) -> &'static str { "status" }

    async fn run(&self, ctx: &CommandContext, _args: &[String]) -> Result<CommandResponse, ChatopsError> {
        let apps = ctx.storage.list_apps(None).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?;

        if apps.is_empty() {
            return Ok(CommandResponse::text("Fleet status: no apps configured."));
        }

        let mut lines = vec![format!("Fleet status ({} app{}):", apps.len(), if apps.len() == 1 { "" } else { "s" })];
        for app in &apps {
            let current = ctx.storage.get_current_deployment(app.id).await
                .map_err(|e| ChatopsError::Storage(e.to_string()))?;
            let status_str = match current {
                Some(dep) => format!("{} ({})", dep.status, dep.image_ref),
                None => "no deployments".to_string(),
            };
            lines.push(format!("  {} [{}] — {}", app.name, app.env, status_str));
        }
        Ok(CommandResponse::text(lines.join("\n")))
    }
}

/// /apps — list user's apps. Real implementation queries StoragePort.
pub struct AppsCmd;

#[async_trait]
impl Command for AppsCmd {
    fn name(&self) -> &'static str { "apps" }

    async fn run(&self, ctx: &CommandContext, _args: &[String]) -> Result<CommandResponse, ChatopsError> {
        let apps = ctx.storage.list_apps(None).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?;

        if apps.is_empty() {
            return Ok(CommandResponse::text("No apps configured."));
        }

        let mut lines = vec![format!("Apps ({}):", apps.len())];
        for app in &apps {
            lines.push(format!(
                "  {} — mode={} env={} status={}",
                app.name, app.deploy_mode, app.env, app.status
            ));
        }
        Ok(CommandResponse::text(lines.join("\n")))
    }
}

/// /doctor — run sovereign doctor summary. Checks schema version + table count.
pub struct DoctorCmd;

#[async_trait]
impl Command for DoctorCmd {
    fn name(&self) -> &'static str { "doctor" }

    async fn run(&self, ctx: &CommandContext, _args: &[String]) -> Result<CommandResponse, ChatopsError> {
        let mut checks = Vec::new();

        // Storage check
        match ctx.storage.schema_version().await {
            Ok(Some(v)) => checks.push(format!("Storage: schema v{v} ✓")),
            Ok(None) => checks.push("Storage: no schema (not initialized)".to_string()),
            Err(e) => checks.push(format!("Storage: ERROR — {e}")),
        }

        // Table count
        match ctx.storage.core_tables().await {
            Ok(tables) => checks.push(format!("Tables: {} core tables ✓", tables.len())),
            Err(e) => checks.push(format!("Tables: ERROR — {e}")),
        }

        // Audit count
        match ctx.storage.count_audit().await {
            Ok(n) => checks.push(format!("Audit events: {n}")),
            Err(e) => checks.push(format!("Audit: ERROR — {e}")),
        }

        // App count
        match ctx.storage.list_apps(None).await {
            Ok(apps) => checks.push(format!("Apps: {} configured", apps.len())),
            Err(e) => checks.push(format!("Apps: ERROR — {e}")),
        }

        Ok(CommandResponse::text(format!("Doctor results:\n{}", checks.join("\n"))))
    }
}

/// /secret list <app> — list secret keys (never values).
pub struct SecretListCmd;

#[async_trait]
impl Command for SecretListCmd {
    fn name(&self) -> &'static str { "secret" }

    async fn run(&self, ctx: &CommandContext, args: &[String]) -> Result<CommandResponse, ChatopsError> {
        if args.is_empty() || args[0] != "list" {
            return Ok(CommandResponse::text("Usage: /secret list <app>"));
        }
        if args.len() < 2 {
            return Err(ChatopsError::MissingArgument("app name".to_string()));
        }
        let app_name = &args[1];
        let app = ctx.storage.get_app_by_name(app_name).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?
            .ok_or_else(|| ChatopsError::AppNotFound(app_name.clone()))?;

        let secrets = ctx.storage.list_secrets(app.id).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?;

        if secrets.is_empty() {
            return Ok(CommandResponse::text(format!("No secrets for {app_name}.")));
        }

        let mut lines = vec![format!("Secrets for {app_name} ({}):", secrets.len())];
        for s in &secrets {
            lines.push(format!("  {} [v{}] — {}", s.key, s.version, s.status));
        }
        Ok(CommandResponse::text(lines.join("\n")))
    }
}

/// /deploy <app> — trigger a deploy. Shows confirmation keyboard.
pub struct DeployCmd;

#[async_trait]
impl Command for DeployCmd {
    fn name(&self) -> &'static str { "deploy" }
    fn is_mutating(&self) -> bool { true }

    async fn run(&self, ctx: &CommandContext, args: &[String]) -> Result<CommandResponse, ChatopsError> {
        if args.is_empty() {
            return Err(ChatopsError::MissingArgument("app name".to_string()));
        }
        let app_name = &args[0];

        // Verify app exists
        let app = ctx.storage.get_app_by_name(app_name).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?
            .ok_or_else(|| ChatopsError::AppNotFound(app_name.clone()))?;

        // Get current deployment for context
        let current = ctx.storage.get_current_deployment(app.id).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?;

        let current_str = match &current {
            Some(dep) => format!("current: {} ({})", dep.image_ref, dep.status),
            None => "no current deployment".to_string(),
        };

        // Show confirmation keyboard
        let keyboard = InlineKeyboardMarkup::row(vec![
            InlineKeyboardButton::callback("Approve", format!("deploy:confirm:{app_name}")),
            InlineKeyboardButton::callback("Cancel", format!("deploy:cancel:{app_name}")),
        ]);

        Ok(CommandResponse::with_keyboard(
            format!("Deploy `{app_name}`?\n{current_str}"),
            keyboard,
        ))
    }
}

/// /rollback <app> — rollback to previous version. Shows confirmation keyboard.
pub struct RollbackCmd;

#[async_trait]
impl Command for RollbackCmd {
    fn name(&self) -> &'static str { "rollback" }
    fn is_mutating(&self) -> bool { true }

    async fn run(&self, ctx: &CommandContext, args: &[String]) -> Result<CommandResponse, ChatopsError> {
        if args.is_empty() {
            return Err(ChatopsError::MissingArgument("app name".to_string()));
        }
        let app_name = &args[0];

        let app = ctx.storage.get_app_by_name(app_name).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?
            .ok_or_else(|| ChatopsError::AppNotFound(app_name.clone()))?;

        let current = ctx.storage.get_current_deployment(app.id).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?;

        let current_str = match &current {
            Some(dep) => format!("current: {} ({})", dep.image_ref, dep.status),
            None => "no current deployment".to_string(),
        };

        let keyboard = InlineKeyboardMarkup::row(vec![
            InlineKeyboardButton::callback("Confirm Rollback", format!("rollback:confirm:{app_name}")),
            InlineKeyboardButton::callback("Cancel", format!("rollback:cancel:{app_name}")),
        ]);

        Ok(CommandResponse::with_keyboard(
            format!("Rollback `{app_name}`?\n{current_str}"),
            keyboard,
        ))
    }
}

/// /backup <app> — trigger a backup. Shows confirmation keyboard.
pub struct BackupCmd;

#[async_trait]
impl Command for BackupCmd {
    fn name(&self) -> &'static str { "backup" }
    fn is_mutating(&self) -> bool { true }

    async fn run(&self, ctx: &CommandContext, args: &[String]) -> Result<CommandResponse, ChatopsError> {
        if args.is_empty() {
            return Err(ChatopsError::MissingArgument("app name".to_string()));
        }
        let app_name = &args[0];

        // Verify app exists
        let _app = ctx.storage.get_app_by_name(app_name).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?
            .ok_or_else(|| ChatopsError::AppNotFound(app_name.clone()))?;

        let keyboard = InlineKeyboardMarkup::row(vec![
            InlineKeyboardButton::callback("Confirm Backup", format!("backup:confirm:{app_name}")),
            InlineKeyboardButton::callback("Cancel", format!("backup:cancel:{app_name}")),
        ]);

        Ok(CommandResponse::with_keyboard(
            format!("Backup `{app_name}`?"),
            keyboard,
        ))
    }
}

/// /deployments <app> — list last 5 deployments.
pub struct DeploymentsCmd;

#[async_trait]
impl Command for DeploymentsCmd {
    fn name(&self) -> &'static str { "deployments" }

    async fn run(&self, ctx: &CommandContext, args: &[String]) -> Result<CommandResponse, ChatopsError> {
        if args.is_empty() {
            return Err(ChatopsError::MissingArgument("app name".to_string()));
        }
        let app_name = &args[0];

        let app = ctx.storage.get_app_by_name(app_name).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?
            .ok_or_else(|| ChatopsError::AppNotFound(app_name.clone()))?;

        let deps = ctx.storage.list_deployments(app.id, 5).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?;

        if deps.is_empty() {
            return Ok(CommandResponse::text(format!("No deployments for {app_name}.")));
        }

        let mut lines = vec![format!("Recent deployments for {app_name}:")];
        for d in &deps {
            let age = format_age(d.started_at);
            lines.push(format!(
                "  {} — {} ({}) {}",
                &d.id.to_string()[..8],
                d.image_ref,
                d.status,
                age
            ));
        }
        Ok(CommandResponse::text(lines.join("\n")))
    }
}

/// /monitor <app> — one-shot CPU/RAM/disk for the host.
pub struct MonitorCmd;

#[async_trait]
impl Command for MonitorCmd {
    fn name(&self) -> &'static str { "monitor" }

    async fn run(&self, _ctx: &CommandContext, _args: &[String]) -> Result<CommandResponse, ChatopsError> {
        // Host-level stats. On Linux we read /proc/stat + /proc/meminfo
        // synchronously (one-shot is fine). On macOS/Windows we report
        // a clean "not supported on this OS" line. Per-app stats would
        // come from cgroup/systemd and are out of scope for /monitor.
        let stats = read_host_stats().await;
        Ok(CommandResponse::text(stats))
    }
}

/// /watch <app> — start live status monitoring for an app.
pub struct WatchCmd;

#[async_trait]
impl Command for WatchCmd {
    fn name(&self) -> &'static str { "watch" }

    async fn run(&self, ctx: &CommandContext, args: &[String]) -> Result<CommandResponse, ChatopsError> {
        if args.is_empty() {
            return Err(ChatopsError::MissingArgument("app name".to_string()));
        }
        let app_name = &args[0];

        // Verify app exists
        let app = ctx.storage.get_app_by_name(app_name).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?
            .ok_or_else(|| ChatopsError::AppNotFound(app_name.clone()))?;

        // Get current deployment status
        let current = ctx.storage.get_current_deployment(app.id).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?;

        let status_str = match &current {
            Some(dep) => format!("{} ({})", dep.status, dep.image_ref),
            None => "no deployments".to_string(),
        };

        Ok(CommandResponse::text(format!(
            "👀 Watching `{app_name}` (updates every 30s, /unwatch to stop)\n\n    Current: {status_str}"
        )))
    }
}

/// /unwatch — stop live status monitoring.
pub struct UnwatchCmd;

#[async_trait]
impl Command for UnwatchCmd {
    fn name(&self) -> &'static str { "unwatch" }

    async fn run(&self, _ctx: &CommandContext, _args: &[String]) -> Result<CommandResponse, ChatopsError> {
        // The actual watch state is managed by ChatStateManager in the poller.
        // This command just tells the user it was stopped; the poller checks
        // state_mgr.stop_watching() on each tick.
        Ok(CommandResponse::text("Stopped watching. Send /watch <app> to start again."))
    }
}

/// /audit — show recent audit events.
pub struct AuditCmd;

#[async_trait]
impl Command for AuditCmd {
    fn name(&self) -> &'static str { "audit" }

    async fn run(&self, ctx: &CommandContext, args: &[String]) -> Result<CommandResponse, ChatopsError> {
        let limit: u32 = args.first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);

        let query = sovereign_core::domain::audit::AuditQuery {
            limit: limit.min(25),
            ..Default::default()
        };

        let events = ctx.storage.query_audit(query).await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?;

        if events.is_empty() {
            return Ok(CommandResponse::text("No audit events."));
        }

        let mut lines = vec![format!("Audit events ({}):", events.len())];
        for ev in &events {
            let ts = chrono::DateTime::from_timestamp(ev.ts.0, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| ev.ts.0.to_string());
            lines.push(format!(
                "  [{}] {} — {} {}",
                ts,
                ev.actor,
                ev.kind,
                ev.target.as_deref().unwrap_or("")
            ));
        }
        Ok(CommandResponse::text(lines.join("\n")))
    }
}

/// /notify <app> <events> — configure notification preferences.
pub struct NotifyCmd;

#[async_trait]
impl Command for NotifyCmd {
    fn name(&self) -> &'static str { "notify" }

    async fn run(&self, ctx: &CommandContext, args: &[String]) -> Result<CommandResponse, ChatopsError> {
        if args.is_empty() {
            return Ok(CommandResponse::text(
                "Usage: /notify <app|all> <events>\n\
                 Events: deploys, failures, all\n\
                 Example: /notify all deploys,failures"
            ));
        }
        // Per-user notification preferences. The preferences are
        // recorded as an audit event so the operator has a paper
        // trail of who set what. A persistent notify_preference table
        // is the V1 add once the sovereign-notify crate ships (Stream
        // B of the MNC plan); until then the audit log is the source
        // of truth.
        let target = &args[0];
        let events = if args.len() > 1 { &args[1] } else { "failures" };
        let event = sovereign_core::domain::audit::AuditEvent {
            id: None,
            ts: sovereign_core::domain::timestamp::Timestamp::now(),
            actor: ctx.user_id.clone(),
            kind: sovereign_core::domain::audit::kind::DOCTOR_RUN,
            target: Some(format!("notify:{target}")),
            payload: serde_json::json!({
                "target": target,
                "events": events,
                "kind": "notify_pref_set",
            }),
            policy_decision: None,
        };
        if let Err(e) = ctx.storage.append_audit(event).await {
            return Err(ChatopsError::Storage(e.to_string()));
        }
        tracing::info!(target, events, user = %ctx.user_id, "notify preference recorded");
        Ok(CommandResponse::text(format!(
            "Notification preferences recorded for `{target}`: {events}"
        )))
    }
}

// ---------------------------------------------------------------------------
// Helper: format age from timestamp
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Helper: read host-level CPU/RAM/disk stats
// ---------------------------------------------------------------------------

async fn read_host_stats() -> String {
    #[cfg(target_os = "linux")]
    {
        let cpu = read_cpu_pct().await;
        let (ram_used_mb, ram_total_mb) = read_meminfo().await;
        let (disk_used_gb, disk_total_gb) = read_df_root().await;
        return format!(
            "Host monitor (live):\n  CPU: {cpu}%\n  RAM: {ram_used_mb}MB / {ram_total_mb}MB\n  Disk: {disk_used_gb}GB / {disk_total_gb}GB"
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        "Host monitor: not supported on this OS (Linux only).".to_string()
    }
}

#[cfg(target_os = "linux")]
async fn read_cpu_pct() -> String {
    let s1 = match tokio::fs::read_to_string("/proc/stat").await {
        Ok(s) => s,
        Err(_) => return "—".into(),
    };
    let total1: u64 = s1
        .lines()
        .find(|l| l.starts_with("cpu "))
        .map(|l| l.split_whitespace().skip(1).filter_map(|x| x.parse::<u64>().ok()).sum())
        .unwrap_or(0);
    let idle1: u64 = s1
        .lines()
        .find(|l| l.starts_with("cpu "))
        .and_then(|l| l.split_whitespace().nth(4))
        .and_then(|x| x.parse::<u64>().ok())
        .unwrap_or(0);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let s2 = match tokio::fs::read_to_string("/proc/stat").await {
        Ok(s) => s,
        Err(_) => return "—".into(),
    };
    let total2: u64 = s2
        .lines()
        .find(|l| l.starts_with("cpu "))
        .map(|l| l.split_whitespace().skip(1).filter_map(|x| x.parse::<u64>().ok()).sum())
        .unwrap_or(0);
    let idle2: u64 = s2
        .lines()
        .find(|l| l.starts_with("cpu "))
        .and_then(|l| l.split_whitespace().nth(4))
        .and_then(|x| x.parse::<u64>().ok())
        .unwrap_or(0);
    let dtotal = total2.saturating_sub(total1);
    let didle = idle2.saturating_sub(idle1);
    if dtotal == 0 { return "0.0".into(); }
    let pct = 100.0 * (dtotal.saturating_sub(didle)) as f64 / dtotal as f64;
    format!("{pct:.1}")
}

#[cfg(target_os = "linux")]
async fn read_meminfo() -> (String, String) {
    let s = match tokio::fs::read_to_string("/proc/meminfo").await {
        Ok(s) => s,
        Err(_) => return ("—".into(), "—".into()),
    };
    let total_kb = parse_kb(&s, "MemTotal");
    let avail_kb = parse_kb(&s, "MemAvailable");
    match (total_kb, avail_kb) {
        (Some(t), Some(a)) if t > 0 => {
            let used = t.saturating_sub(a);
            ((used / 1024).to_string(), (t / 1024).to_string())
        }
        _ => ("—".into(), "—".into()),
    }
}

#[cfg(target_os = "linux")]
fn parse_kb(s: &str, key: &str) -> Option<u64> {
    s.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse::<u64>().ok())
}

#[cfg(target_os = "linux")]
async fn read_df_root() -> (String, String) {
    let out = match tokio::process::Command::new("df")
        .args(["-BG", "--output=used,size", "/"])
        .output()
        .await
    {
        Ok(o) if o.status.success() => o,
        _ => return ("—".into(), "—".into()),
    };
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().nth(1).unwrap_or("");
    let mut it = line.split_whitespace();
    let used = it.next().unwrap_or("—").trim_end_matches('G').to_string();
    let total = it.next().unwrap_or("—").trim_end_matches('G').to_string();
    (used, total)
}


// ---------------------------------------------------------------------------
// Helper: format age from timestamp
// ---------------------------------------------------------------------------

fn format_age(ts: sovereign_core::domain::timestamp::Timestamp) -> String {
    let now = sovereign_core::domain::timestamp::Timestamp::now();
    let diff_secs = now.0 - ts.0;
    if diff_secs < 0 {
        return "in the future".to_string();
    }
    let diff_secs = diff_secs as u64;
    if diff_secs < 60 {
        format!("{diff_secs}s ago")
    } else if diff_secs < 3600 {
        format!("{}m ago", diff_secs / 60)
    } else if diff_secs < 86400 {
        format!("{}h ago", diff_secs / 3600)
    } else {
        format!("{}d ago", diff_secs / 86400)
    }
}

// ---------------------------------------------------------------------------
// builtins() constructor
// ---------------------------------------------------------------------------

/// Build a CommandRegistry with all built-in commands.
pub fn builtins(
    registry: Arc<tokio::sync::RwLock<CommandRegistry>>,
    binding_codes: Arc<dyn BindingCodeStore>,
    bindings: Arc<dyn BindingStore>,
    storage: Arc<dyn StoragePort>,
    _client: Arc<TelegramClient>,
) -> CommandRegistry {
    let mut reg = CommandRegistry::new();
    reg.register(Arc::new(HelpCmd { registry: registry.clone() }));
    reg.register(Arc::new(StartCmd { binding_codes, bindings }));
    reg.register(Arc::new(StatusCmd));
    reg.register(Arc::new(AppsCmd));
    reg.register(Arc::new(DoctorCmd));
    reg.register(Arc::new(SecretListCmd));
    reg.register(Arc::new(DeployCmd));
    reg.register(Arc::new(RollbackCmd));
    reg.register(Arc::new(BackupCmd));
    reg.register(Arc::new(DeploymentsCmd));
    reg.register(Arc::new(MonitorCmd));
    reg.register(Arc::new(WatchCmd));
    reg.register(Arc::new(UnwatchCmd));
    reg.register(Arc::new(AuditCmd));
    reg.register(Arc::new(NotifyCmd));
    let _ = storage; // used by command structs via CommandContext
    let _ = _client;
    reg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::InMemoryBindingCodeStore;
    use crate::commands::InMemoryBindingStore;
    fn test_ctx(chat_id: i64) -> CommandContext {
        CommandContext {
            user_id: "user1".to_string(),
            chat_id,
            message_id: 1,
            from: None,
            raw_text: "/status".to_string(),
            storage: Arc::new(NoOpStorage),
        }
    }

    // Minimal NoOp storage for tests that don't need real data
    struct NoOpStorage;

    #[async_trait::async_trait]
    impl sovereign_core::ports::StoragePort for NoOpStorage {
        async fn schema_version(&self) -> Result<Option<i64>, sovereign_core::error::AppError> { Ok(Some(9)) }
        async fn core_tables(&self) -> Result<Vec<String>, sovereign_core::error::AppError> { Ok(vec![]) }
        async fn get_app(&self, _id: sovereign_core::domain::id::AppId) -> Result<Option<sovereign_core::domain::app::App>, sovereign_core::error::AppError> { Ok(None) }
        async fn get_app_by_name(&self, _name: &str) -> Result<Option<sovereign_core::domain::app::App>, sovereign_core::error::AppError> { Ok(None) }
        async fn list_apps(&self, _owner: Option<&str>) -> Result<Vec<sovereign_core::domain::app::App>, sovereign_core::error::AppError> { Ok(vec![]) }
        async fn create_app(&self, _new: sovereign_core::domain::app::NewApp, _actor: &str) -> Result<sovereign_core::domain::app::App, sovereign_core::error::AppError> { unimplemented!() }
        async fn update_app(&self, _id: sovereign_core::domain::id::AppId, _update: sovereign_core::domain::app::AppUpdate, _expected_version: i64, _actor: &str) -> Result<sovereign_core::domain::app::App, sovereign_core::error::AppError> { unimplemented!() }
        async fn archive_app(&self, _id: sovereign_core::domain::id::AppId, _expected_version: i64, _actor: &str) -> Result<sovereign_core::domain::app::App, sovereign_core::error::AppError> { unimplemented!() }
        async fn begin_deployment(&self, _new: sovereign_core::domain::deployment::NewDeployment, _actor: &str) -> Result<sovereign_core::domain::deployment::Deployment, sovereign_core::error::AppError> { unimplemented!() }
        async fn transition_deployment(&self, _id: sovereign_core::domain::id::DeploymentId, _event: sovereign_core::domain::deployment::DeploymentEvent, _actor: &str) -> Result<sovereign_core::domain::deployment::Deployment, sovereign_core::error::AppError> { unimplemented!() }
        async fn get_deployment(&self, _id: sovereign_core::domain::id::DeploymentId) -> Result<Option<sovereign_core::domain::deployment::Deployment>, sovereign_core::error::AppError> { Ok(None) }
        async fn list_deployments(&self, _app: sovereign_core::domain::id::AppId, _limit: u32) -> Result<Vec<sovereign_core::domain::deployment::Deployment>, sovereign_core::error::AppError> { Ok(vec![]) }
        async fn get_current_deployment(&self, _app: sovereign_core::domain::id::AppId) -> Result<Option<sovereign_core::domain::deployment::Deployment>, sovereign_core::error::AppError> { Ok(None) }
        async fn list_healthy_deployments_before(&self, _app: sovereign_core::domain::id::AppId, _before: sovereign_core::domain::timestamp::Timestamp, _limit: u32) -> Result<Vec<sovereign_core::domain::deployment::Deployment>, sovereign_core::error::AppError> { Ok(vec![]) }
        async fn set_rollback_target(&self, _id: sovereign_core::domain::id::DeploymentId, _target: sovereign_core::domain::id::DeploymentId, _actor: &str) -> Result<sovereign_core::domain::deployment::Deployment, sovereign_core::error::AppError> { unimplemented!() }
        async fn add_domain(&self, _new: sovereign_core::domain::hostname::NewDomain, _actor: &str) -> Result<sovereign_core::domain::hostname::Domain, sovereign_core::error::AppError> { unimplemented!() }
        async fn remove_domain(&self, _id: sovereign_core::domain::id::DomainId, _actor: &str) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
        async fn list_domains(&self, _app: sovereign_core::domain::id::AppId) -> Result<Vec<sovereign_core::domain::hostname::Domain>, sovereign_core::error::AppError> { Ok(vec![]) }
        async fn put_secret(&self, _new: sovereign_core::domain::secret::NewSecret, _actor: &str) -> Result<sovereign_core::domain::secret::Secret, sovereign_core::error::AppError> { unimplemented!() }
        async fn get_secret(&self, _id: sovereign_core::domain::id::SecretId) -> Result<Option<sovereign_core::domain::secret::Secret>, sovereign_core::error::AppError> { Ok(None) }
        async fn list_secrets(&self, _app: sovereign_core::domain::id::AppId) -> Result<Vec<sovereign_core::domain::secret::Secret>, sovereign_core::error::AppError> { Ok(vec![]) }
        async fn delete_secret(&self, _id: sovereign_core::domain::id::SecretId, _actor: &str) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
        async fn rotate_secret(&self, _id: sovereign_core::domain::id::SecretId, _new_ciphertext: Vec<u8>, _actor: &str) -> Result<sovereign_core::domain::secret::Secret, sovereign_core::error::AppError> { unimplemented!() }
        async fn set_secret_status(&self, _id: sovereign_core::domain::id::SecretId, _status: sovereign_core::domain::secret::SecretStatus, _actor: &str) -> Result<sovereign_core::domain::secret::Secret, sovereign_core::error::AppError> { unimplemented!() }
        async fn add_server(&self, _new: sovereign_core::domain::server::NewServer) -> Result<sovereign_core::domain::server::Server, sovereign_core::error::AppError> { unimplemented!() }
        async fn get_server(&self, _id: sovereign_core::domain::id::ServerId) -> Result<Option<sovereign_core::domain::server::Server>, sovereign_core::error::AppError> { Ok(None) }
        async fn list_servers(&self) -> Result<Vec<sovereign_core::domain::server::Server>, sovereign_core::error::AppError> { Ok(vec![]) }
        async fn set_server_status(&self, _id: sovereign_core::domain::id::ServerId, _status: sovereign_core::domain::server::ServerStatus, _actor: &str) -> Result<sovereign_core::domain::server::Server, sovereign_core::error::AppError> { unimplemented!() }
        async fn touch_server(&self, _id: sovereign_core::domain::id::ServerId, _at: sovereign_core::domain::timestamp::Timestamp) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
        async fn remove_server(&self, _id: sovereign_core::domain::id::ServerId, _actor: &str) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
        async fn begin_backup(&self, _new: sovereign_core::domain::backup::NewBackup, _actor: &str) -> Result<sovereign_core::domain::backup::Backup, sovereign_core::error::AppError> { unimplemented!() }
        async fn complete_backup(&self, _id: sovereign_core::domain::id::BackupId, _status: sovereign_core::domain::backup::BackupStatus, _size_bytes: Option<i64>, _location: Option<String>, _error: Option<String>, _actor: &str) -> Result<sovereign_core::domain::backup::Backup, sovereign_core::error::AppError> { unimplemented!() }
        async fn verify_backup(&self, _id: sovereign_core::domain::id::BackupId, _verify_result: serde_json::Value, _actor: &str) -> Result<sovereign_core::domain::backup::Backup, sovereign_core::error::AppError> { unimplemented!() }
        async fn get_backup(&self, _id: sovereign_core::domain::id::BackupId) -> Result<Option<sovereign_core::domain::backup::Backup>, sovereign_core::error::AppError> { Ok(None) }
        async fn list_backups(&self, _limit: u32) -> Result<Vec<sovereign_core::domain::backup::Backup>, sovereign_core::error::AppError> { Ok(vec![]) }
        async fn create_user(&self, _new: sovereign_core::domain::user::NewUser) -> Result<sovereign_core::domain::user::User, sovereign_core::error::AppError> { unimplemented!() }
        async fn get_user(&self, _id: sovereign_core::domain::id::UserId) -> Result<Option<sovereign_core::domain::user::User>, sovereign_core::error::AppError> { Ok(None) }
        async fn get_user_by_email(&self, _email: &str) -> Result<Option<sovereign_core::domain::user::User>, sovereign_core::error::AppError> { Ok(None) }
        async fn list_users(&self) -> Result<Vec<sovereign_core::domain::user::User>, sovereign_core::error::AppError> { Ok(vec![]) }
        async fn set_user_role(&self, _id: sovereign_core::domain::id::UserId, _role: sovereign_core::domain::user::UserRole, _actor: &str) -> Result<sovereign_core::domain::user::User, sovereign_core::error::AppError> { unimplemented!() }
        async fn touch_user(&self, _id: sovereign_core::domain::id::UserId, _at: sovereign_core::domain::timestamp::Timestamp) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
        async fn set_user_password_hash(&self, _id: sovereign_core::domain::id::UserId, _hash: Option<&str>) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
        async fn get_user_password_hash(&self, _id: sovereign_core::domain::id::UserId) -> Result<Option<String>, sovereign_core::error::AppError> { Ok(None) }
        async fn set_user_display_name(&self, _id: sovereign_core::domain::id::UserId, _display_name: &str) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
        async fn disable_user(&self, _id: sovereign_core::domain::id::UserId, _at: sovereign_core::domain::timestamp::Timestamp) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
        async fn enable_user(&self, _id: sovereign_core::domain::id::UserId) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
        async fn create_api_token(&self, _user_id: sovereign_core::domain::id::UserId, _name: &str, _hash: &str, _scopes: &str, _created_at: sovereign_core::domain::timestamp::Timestamp, _expires_at: Option<sovereign_core::domain::timestamp::Timestamp>) -> Result<sovereign_core::domain::user::ApiToken, sovereign_core::error::AppError> { unimplemented!() }
        async fn list_api_tokens(&self, _user_id: sovereign_core::domain::id::UserId) -> Result<Vec<sovereign_core::domain::user::ApiToken>, sovereign_core::error::AppError> { Ok(vec![]) }
        async fn get_api_token(&self, _user_id: sovereign_core::domain::id::UserId, _name: &str) -> Result<Option<sovereign_core::domain::user::ApiToken>, sovereign_core::error::AppError> { Ok(None) }
        async fn delete_api_token(&self, _user_id: sovereign_core::domain::id::UserId, _name: &str) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
        async fn get_bootstrap_admin(&self) -> Result<Option<sovereign_core::domain::id::UserId>, sovereign_core::error::AppError> { Ok(None) }
        async fn set_bootstrap_admin(&self, _user_id: sovereign_core::domain::id::UserId) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
        async fn append_audit(&self, _event: sovereign_core::domain::audit::AuditEvent) -> Result<(), sovereign_core::error::AppError> { Ok(()) }
        async fn query_audit(&self, _q: sovereign_core::domain::audit::AuditQuery) -> Result<Vec<sovereign_core::domain::audit::AuditEvent>, sovereign_core::error::AppError> { Ok(vec![]) }
        async fn count_audit(&self) -> Result<i64, sovereign_core::error::AppError> { Ok(0) }
        async fn vacuum_into(&self, _target_path: &std::path::Path) -> Result<u64, sovereign_core::error::AppError> { Ok(0) }
        async fn record_update(&self, _record: &sovereign_core::ports::update::UpdateRecord) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
        async fn last_update(&self) -> Result<Option<sovereign_core::ports::update::UpdateRecord>, sovereign_core::error::AppError> { Ok(None) }
        async fn list_updates(&self, _limit: u32) -> Result<Vec<sovereign_core::ports::update::UpdateRecord>, sovereign_core::error::AppError> { Ok(vec![]) }
        async fn mark_update_rolled_back(&self, _sha256: &str) -> Result<(), sovereign_core::error::AppError> { unimplemented!() }
    }

    #[tokio::test]
    async fn help_lists_commands() {
        let reg = Arc::new(tokio::sync::RwLock::new(CommandRegistry::new()));
        let bc: Arc<dyn BindingCodeStore> = Arc::new(InMemoryBindingCodeStore::new());
        let bs: Arc<dyn BindingStore> = Arc::new(InMemoryBindingStore::new());
        let storage: Arc<dyn StoragePort> = Arc::new(NoOpStorage);
        let client = Arc::new(TelegramClient::new("test"));
        let built = builtins(reg.clone(), bc, bs, storage, client);
        *reg.write().await = built;

        let cmd = HelpCmd { registry: reg };
        let ctx = test_ctx(1);
        let result = cmd.run(&ctx, &[]).await.unwrap();
        match result {
            CommandResponse::Text(t) => {
                assert!(t.contains("/status"));
                assert!(t.contains("/deploy"));
            }
            _ => panic!("expected Text"),
        }
    }

    #[tokio::test]
    async fn start_requires_code() {
        let bc: Arc<dyn BindingCodeStore> = Arc::new(InMemoryBindingCodeStore::new());
        let bs: Arc<dyn BindingStore> = Arc::new(InMemoryBindingStore::new());
        let cmd = StartCmd { binding_codes: bc, bindings: bs };
        let ctx = test_ctx(123);
        let result = cmd.run(&ctx, &[]).await.unwrap();
        match result {
            CommandResponse::Text(t) => assert!(t.contains("Usage")),
            _ => panic!("expected Text"),
        }
    }

    #[tokio::test]
    async fn start_binds_valid_code() {
        let bc: Arc<dyn BindingCodeStore> = Arc::new(InMemoryBindingCodeStore::new());
        let bs: Arc<dyn BindingStore> = Arc::new(InMemoryBindingStore::new());
        let code = bc.create_code("alice").await.unwrap();
        let cmd = StartCmd { binding_codes: bc, bindings: bs.clone() };
        let ctx = test_ctx(456);
        let result = cmd.run(&ctx, &[code]).await.unwrap();
        match result {
            CommandResponse::Text(t) => assert!(t.contains("Bound to user alice")),
            _ => panic!("expected Text"),
        }
        assert_eq!(bs.get_user_id(456).await.unwrap(), Some("alice".to_string()));
    }

    #[tokio::test]
    async fn status_empty_fleet() {
        let ctx = test_ctx(1);
        let cmd = StatusCmd;
        let result = cmd.run(&ctx, &[]).await.unwrap();
        match result {
            CommandResponse::Text(t) => assert!(t.contains("no apps")),
            _ => panic!("expected Text"),
        }
    }

    #[tokio::test]
    async fn deploy_requires_app() {
        let ctx = test_ctx(1);
        let cmd = DeployCmd;
        let err = cmd.run(&ctx, &[]).await.unwrap_err();
        assert!(matches!(err, ChatopsError::MissingArgument(_)));
    }

    #[tokio::test]
    async fn deploy_shows_error_for_missing_app() {
        let ctx = test_ctx(1);
        let cmd = DeployCmd;
        let err = cmd.run(&ctx, &["myapp".to_string()]).await.unwrap_err();
        assert!(matches!(err, ChatopsError::AppNotFound(_)));
    }

    #[tokio::test]
    async fn rollback_shows_error_for_missing_app() {
        let ctx = test_ctx(1);
        let cmd = RollbackCmd;
        let err = cmd.run(&ctx, &["myapp".to_string()]).await.unwrap_err();
        assert!(matches!(err, ChatopsError::AppNotFound(_)));
    }

    #[tokio::test]
    async fn backup_shows_error_for_missing_app() {
        let ctx = test_ctx(1);
        let cmd = BackupCmd;
        let err = cmd.run(&ctx, &["myapp".to_string()]).await.unwrap_err();
        assert!(matches!(err, ChatopsError::AppNotFound(_)));
    }

    #[test]
    fn deploy_is_mutating() {
        assert!(DeployCmd.is_mutating());
        assert!(RollbackCmd.is_mutating());
        assert!(BackupCmd.is_mutating());
        assert!(!StatusCmd.is_mutating());
        assert!(!AppsCmd.is_mutating());
        assert!(!HelpCmd { registry: Arc::new(tokio::sync::RwLock::new(CommandRegistry::new())) }.is_mutating());
    }

    #[test]
    fn format_age_seconds() {
        let now = sovereign_core::domain::timestamp::Timestamp::now();
        assert_eq!(format_age(now), "0s ago");
    }

    #[tokio::test]
    async fn watch_requires_app() {
        let ctx = test_ctx(1);
        let cmd = WatchCmd;
        let err = cmd.run(&ctx, &[]).await.unwrap_err();
        assert!(matches!(err, ChatopsError::MissingArgument(_)));
    }

    #[tokio::test]
    async fn watch_missing_app() {
        let ctx = test_ctx(1);
        let cmd = WatchCmd;
        let err = cmd.run(&ctx, &["nonexistent".to_string()]).await.unwrap_err();
        assert!(matches!(err, ChatopsError::AppNotFound(_)));
    }

    #[tokio::test]
    async fn unwatch_returns_text() {
        let ctx = test_ctx(1);
        let cmd = UnwatchCmd;
        let result = cmd.run(&ctx, &[]).await.unwrap();
        match result {
            CommandResponse::Text(t) => assert!(t.contains("Stopped")),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn watch_not_mutating() {
        assert!(!WatchCmd.is_mutating());
        assert!(!UnwatchCmd.is_mutating());
        assert!(!AuditCmd.is_mutating());
        assert!(!NotifyCmd.is_mutating());
    }

    #[tokio::test]
    async fn audit_returns_empty_for_noop() {
        let ctx = test_ctx(1);
        let cmd = AuditCmd;
        let result = cmd.run(&ctx, &[]).await.unwrap();
        match result {
            CommandResponse::Text(t) => assert!(t.contains("No audit events")),
            _ => panic!("expected Text"),
        }
    }

    #[tokio::test]
    async fn notify_usage_when_no_args() {
        let ctx = test_ctx(1);
        let cmd = NotifyCmd;
        let result = cmd.run(&ctx, &[]).await.unwrap();
        match result {
            CommandResponse::Text(t) => assert!(t.contains("Usage")),
            _ => panic!("expected Text"),
        }
    }

    #[tokio::test]
    async fn notify_sets_preference() {
        let ctx = test_ctx(1);
        let cmd = NotifyCmd;
        let result = cmd.run(&ctx, &["all".into(), "deploys".into()]).await.unwrap();
        match result {
            CommandResponse::Text(t) => {
                assert!(t.contains("all"));
                assert!(t.contains("deploys"));
            }
            _ => panic!("expected Text"),
        }
    }
}
