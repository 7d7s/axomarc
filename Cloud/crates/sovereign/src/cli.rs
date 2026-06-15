// F2: The CLI surface — top-level struct, subcommand tree, global flags.
// Per docs/phase-00-mvp.md §F2 (the 8 sub-tasks) and
// docs/product-ux.md §2 (the 8 CLI non-negotiables).
//
// The CLI is the product. This module is the single source of truth for every
// flag, subcommand, and help string the operator sees. The clap derive macros
// generate the help text, the shell completions, the man pages, and the
// OpenAPI spec from this one definition.

use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

/// The top-level CLI. The global flags are available on every subcommand.
#[derive(Parser, Debug)]
#[command(
    name = "sovereign",
    version,
    about = "Sovereign Application Runtime — single-binary, self-hosted deployment runtime",
    long_about = "Run your own cloud. Single binary. No DevOps team.\n\
                  Sovereign is a Rust deployment runtime that ships your apps \
                  on your VPS, your hardware, or your air-gapped network. \
                  It is one binary, one SQLite file, and one CLI.",
    after_long_help = "\
QUICKSTART:
  sovereign init                       # detect your framework, write app.yaml
  sovereign login                      # open the browser, sign in to the control plane
  sovereign deploy                     # build, ship, get a live URL
  sovereign logs --app <APP> --follow  # tail the logs of a running app
  sovereign status                     # see your fleet at a glance
  sovereign doctor                     # diagnose the host (the on-call's first command)

MORE:
  sovereign rollback --app <APP> --list    # see all deploys of an app
  sovereign secret set DATABASE_URL --app <APP>   # set a secret (reads value from stdin)
  sovereign backup create --app postgres   # snapshot the database
  sovereign backup list                    # see all backups
  sovereign completions bash > /etc/bash_completion.d/sovereign   # install completions
  sovereign man /usr/local/share/man/man1/   # install man pages

DOCS:
  https://sovereignruntime.dev/docs/
  Per docs/phase-00-mvp.md §F2 (the 8 non-negotiables) and §F3+ for each subcommand."
)]
pub struct Cli {
    /// Control plane URL (env: SOVEREIGN_URL, default: http://127.0.0.1:7878)
    #[arg(
        long,
        env = "SOVEREIGN_URL",
        global = true,
        default_value = "http://127.0.0.1:7878"
    )]
    pub url: String,

    /// API token (env: SOVEREIGN_TOKEN)
    #[arg(long, env = "SOVEREIGN_TOKEN", global = true, hide_env_values = true)]
    pub token: Option<String>,

    /// Output format (auto-detected: text on TTY, JSON on a pipe)
    #[arg(long, value_enum, global = true, default_value_t = Format::Auto)]
    pub format: Format,

    /// Disable colored output (also: NO_COLOR=1)
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Show what would be done, but do not do it
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Verbosity (repeat for more: -v, -vv, -vvv)
    #[arg(long, short = 'v', global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

// Re-export the Output enum from the output module so clap can see it.
pub use crate::output::Format;

/// The subcommand tree. V0 ships the 10 subcommands in the F2 spec;
/// V1+ will add the rest (init's framework scan, tui, doctor, sovereignty-test, etc.).
#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Initialize a new app in the current directory
    Init {
        /// Framework to detect (auto-detect by default)
        #[arg(long, value_enum, default_value_t = Framework::Auto)]
        framework: Framework,
        /// Overwrite an existing `app.yaml` without prompting
        #[arg(long)]
        force: bool,
        /// Path to write `app.yaml` (defaults to `./app.yaml`)
        #[arg(long, default_value = "app.yaml")]
        output: std::path::PathBuf,
        /// App name (defaults to the current directory name)
        #[arg(long)]
        name: Option<String>,
        /// Print the framework registry (all supported frameworks
        /// with their default port, health_path, and run command)
        /// and exit. Does not scan the current directory.
        #[arg(long)]
        print_frameworks: bool,
    },

    /// Log in to the control plane (V0: generates / unlocks the local
    /// age master key; real OIDC is V2). Reads the passphrase from
    /// `SOVEREIGN_PASSPHRASE` or stdin.
    Login {
        /// Skip the interactive prompt; require `SOVEREIGN_PASSPHRASE`
        /// to be set. Useful for CI / scripted `init` flows.
        #[arg(long)]
        no_input: bool,
        /// Master-key path override (defaults to
        /// `/var/lib/sovereign/master.key` on Linux, the
        /// `directorie data` dir on macOS, `%APPDATA%\sovereign\`
        /// on Windows)
        #[arg(long)]
        master_key: Option<std::path::PathBuf>,
        /// Migrate an existing V0.1.0 bare-Bech32 master key
        /// to the V0.5 Argon2id-wrapped format and re-encrypt
        /// every secret under the new identity. Requires
        /// `SOVEREIGN_PASSPHRASE` (the new passphrase).
        #[arg(long)]
        migrate: bool,
    },

    /// Deploy an app
    Deploy {
        /// App to deploy (defaults to the app in the current directory)
        #[arg(long)]
        app: Option<String>,
        /// Image to deploy (overrides the app.yaml). For native mode,
        /// this is the path to the .sov archive.
        #[arg(long)]
        image: Option<String>,
        /// Deployment strategy
        #[arg(long, value_enum, default_value_t = Strategy::BlueGreen)]
        strategy: Strategy,
        /// Wait for the deploy to be healthy before exiting
        #[arg(long)]
        wait: bool,
        /// Don't write `sovereign.lock` in the repo (F5 sub-task 5)
        #[arg(long)]
        no_lock: bool,
        /// (Native mode) Binary path inside the .sov archive
        #[arg(long)]
        native_binary: Option<String>,
        /// (Native mode) Exec start template (e.g. "./myapp --port {port}")
        #[arg(long)]
        native_exec_start: Option<String>,
    },

    /// Roll back an app to a previous deployment
    Rollback {
        /// App to roll back
        app: String,
        /// Deployment ID to roll back to (defaults to the previous successful deploy)
        #[arg(long)]
        to: Option<String>,
        /// List all deployments of the app
        #[arg(long)]
        list: bool,
        /// Max deployments to show in --list
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },

    /// Stream logs from a running app
    Logs {
        /// App to stream logs from
        app: String,
        /// Number of lines to show before following
        #[arg(long, default_value_t = 100)]
        tail: usize,
        /// Follow the log stream (Ctrl+C to exit)
        #[arg(long, short = 'f')]
        follow: bool,
    },

    /// Show app status
    Status {
        /// App to show (defaults to all apps)
        #[arg(long)]
        app: Option<String>,
    },

    /// Manage hostname -> app routing (F6). On `add`, a Caddy route
    /// is pushed to the admin API so the hostname serves the app.
    Domain {
        #[command(subcommand)]
        cmd: DomainCmd,
    },

    /// Manage secrets (zero-disk injection; values never touch the box)
    Secret {
        #[command(subcommand)]
        cmd: SecretCmd,
    },

    /// Manage backups (snapshot, verify, restore, list)
    Backup {
        #[command(subcommand)]
        cmd: BackupCmd,
    },

    /// Run the diagnostic checks (F9). The V0 binary ships the
    /// `basic` level only; higher levels return a clear "V1+"
    /// error so the operator knows what to upgrade to.
    Doctor {
        /// Diagnostic level (V0 supports `basic` only)
        #[arg(long, value_enum, default_value_t = DoctorLevelArg::Basic)]
        level: DoctorLevelArg,
        /// Print a longer description for every check, not just the
        /// short status glyph.
        #[arg(long)]
        explain: bool,
        /// Run any available auto-fix for failing checks (basic only)
        #[arg(long)]
        fix: bool,
        /// Write a markdown report to the given path (default:
        /// `/var/log/sovereign/doctor-<timestamp>.md` on Linux)
        #[arg(long)]
        report: Option<std::path::PathBuf>,
        /// Emit the full report as JSON on stdout
        #[arg(long)]
        json: bool,
    },

    /// Manage self-updates (F10). The V0 binary supports
    /// `check`, `apply`, `rollback`, and `history` against a
    /// release manifest endpoint.
    Update {
        #[command(subcommand)]
        cmd: UpdateCmd,
    },

    /// Generate shell completions (bash, zsh, fish, nushell, powershell)
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },

    /// Validate an `app.yaml` against the V0 schema. Useful for
    /// catching typos (`prot` instead of `port`, `Dockerfile`
    /// instead of `image`, etc.) before `sovereign deploy`.
    Validate {
        /// Path to the app.yaml to validate (defaults to `./app.yaml`)
        #[arg(default_value = "app.yaml")]
        path: std::path::PathBuf,
    },

    /// Manage system services (V0.6). Install, remove, check status,
    /// and restart managed services (nginx, mysql, mariadb, redis,
    /// vsftpd, letsencrypt, phpmyadmin).
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },

    /// Start or manage the long-running daemon process. The daemon
    /// runs the HTTP server (webhooks, health, metrics) and the
    /// Telegram poller as a single systemd service.
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCmd,
    },

    /// Manage Telegram chatops (bot binding, poller lifecycle)
    Chatops {
        #[command(subcommand)]
        cmd: ChatopsCmd,
    },

    /// Manage users (add, list, disable, enable, whoami)
    User {
        #[command(subcommand)]
        cmd: UserCmd,
    },

    /// Manage API tokens (create, list, revoke)
    Token {
        #[command(subcommand)]
        cmd: TokenCmd,
    },

    /// Generate the man page (writes sovereign.1 to the given directory)
    Man {
        /// Output directory (must exist)
        dir: String,
    },

    /// Print the version
    Version,
}

/// The framework to detect / generate. Auto-detects by reading package.json,
/// requirements.txt, go.mod, Gemfile, composer.json, mix.exs, etc.
#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Framework {
    #[default]
    Auto,
    Fastapi,
    Flask,
    Django,
    Nextjs,
    Nuxt,
    Sveltekit,
    Remix,
    Laravel,
    Go,
    Rails,
    Astro,
    Static,
    Express,
    Phoenix,
    Deno,
    Generic,
}

/// The deploy strategy. V0 ships BlueGreen; Rolling and Recreate are V1+.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Strategy {
    #[default]
    BlueGreen,
    Rolling,
    Recreate,
}

/// The supported shells for completion generation.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Nushell,
    Powershell,
}

/// The secret subcommand tree. F7 implements the actual store.
#[derive(Subcommand, Debug)]
pub enum SecretCmd {
    /// Set a secret (reads the value from stdin to avoid /proc/<pid>/cmdline leaks)
    Set {
        /// Secret key (e.g. DATABASE_URL)
        key: String,
        /// App the secret belongs to
        #[arg(long)]
        app: String,
    },
    /// List the secret keys for an app (values are NEVER shown)
    List {
        /// App to list secrets for
        app: String,
    },
    /// Rotate a secret (generates a new value, deploys a new revision)
    Rotate {
        /// Secret key to rotate
        key: String,
        /// App the secret belongs to
        #[arg(long)]
        app: String,
    },
}

/// The backup subcommand tree. F8 implements the actual snapshot logic.
#[derive(Subcommand, Debug)]
pub enum BackupCmd {
    /// Create a backup of an app's data
    Create {
        /// App to back up
        #[arg(long)]
        app: String,
    },
    /// List all backups
    List,
    /// Verify a backup (open the snapshot, run integrity_check, count rows)
    Verify {
        /// Backup ID to verify
        backup_id: String,
    },
    /// Restore a backup to a target path (live-restore is rejected in V0)
    Restore {
        /// Backup ID to restore
        backup_id: String,
        /// Destination path for the restored SQLite file
        #[arg(long)]
        to: String,
    },
}

/// The service subcommand tree (V0.6). Install, remove, check status,
/// and restart managed system services.
#[derive(Subcommand, Clone, Debug)]
pub enum ServiceCmd {
    /// Install a system service (apt + systemd)
    Install {
        /// Service to install (nginx, mysql, mariadb, redis, vsftpd, letsencrypt, phpmyadmin)
        kind: String,
        /// Pinned version (e.g. "1.24.0-0ubuntu3"). If omitted, installs the latest.
        #[arg(long)]
        version: Option<String>,
        /// Skip `systemctl enable --now` after install
        #[arg(long)]
        no_start: bool,
    },
    /// Remove (purge) a system service
    Remove {
        /// Service to remove
        kind: String,
        /// Remove config files too (apt purge vs remove)
        #[arg(long)]
        purge: bool,
    },
    /// Check the status of a system service
    Status {
        /// Service to check (or "all" for every managed service)
        kind: String,
    },
    /// Restart a system service (systemctl restart)
    Restart {
        /// Service to restart
        kind: String,
    },
    /// Validate configuration for a system service (e.g. `nginx -t`)
    Validate {
        /// Service to validate
        kind: String,
    },
    /// List all managed system services with their current status
    List,
}

/// The daemon subcommand tree. Runs the long-lived HTTP server and
/// Telegram poller as a single process, managed by systemd.
#[derive(Subcommand, Debug)]
pub enum DaemonCmd {
    /// Start the daemon (blocks until SIGTERM/SIGINT)
    Start {
        /// Listen address override (default: 127.0.0.1:8443)
        #[arg(long)]
        listen: Option<String>,
    },
    /// Install the systemd unit file and enable the service
    Install {
        /// Override the unit name (default: sovereign-daemon)
        #[arg(long, default_value = "sovereign-daemon")]
        unit_name: String,
    },
    /// Show daemon status (is it running? uptime? version?)
    Status,
    /// Show recent daemon logs (journalctl -u sovereign-daemon -n 50)
    Logs {
        /// Number of lines to show
        #[arg(long, default_value_t = 50)]
        lines: u32,
    },
}

/// The chatops subcommand tree. Manages the Telegram bot binding
/// and poller lifecycle.
#[derive(Subcommand, Debug)]
pub enum ChatopsCmd {
    /// Generate a 6-digit binding code and print a deeplink.
    /// The operator shares the deeplink with the user; the user
    /// clicks it to send `/start <code>` to the bot.
    Init {
        /// Sovereign user email to bind
        #[arg(long)]
        user: String,
    },
    /// Start the Telegram long-polling loop (blocks).
    /// Requires SOVEREIGN_TELEGRAM_TOKEN env var or --token flag.
    Start {
        /// Telegram bot token (env: SOVEREIGN_TELEGRAM_TOKEN)
        #[arg(long, env = "SOVEREIGN_TELEGRAM_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
    /// Revoke a Telegram binding (unbind chat_id from user)
    Revoke {
        /// User email to unbind
        #[arg(long)]
        user: String,
    },
    /// List active Telegram bindings
    Bindings,
}

/// User management subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum UserCmd {
    /// Add a new user. First user becomes admin (bootstrap).
    Add {
        /// Email address
        email: String,
        /// Password (reads from stdin if not provided)
        #[arg(long)]
        password: bool,
        /// Role (default: viewer)
        #[arg(long, default_value = "viewer")]
        role: String,
        /// Display name
        #[arg(long)]
        name: Option<String>,
    },
    /// List all users
    List {
        /// Filter by role
        #[arg(long)]
        role: Option<String>,
    },
    /// Disable a user (soft-delete)
    Disable {
        /// Email of user to disable
        email: String,
    },
    /// Re-enable a disabled user
    Enable {
        /// Email of user to enable
        email: String,
    },
    /// Show current user context
    Whoami,
}

/// API token management subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum TokenCmd {
    /// Create a new API token
    Create {
        /// Token name
        name: String,
        /// Comma-separated scopes (e.g. "app.deploy,secret.read")
        #[arg(long)]
        scopes: String,
        /// TTL (e.g. "30d", "24h")
        #[arg(long)]
        ttl: Option<String>,
    },
    /// List API tokens
    List,
    /// Revoke an API token
    Revoke {
        /// Token name to revoke
        name: String,
    },
}

/// The diagnostic level argument for the CLI. Mirrors
/// `sovereign_doctor::DoctorLevel` but uses a local `ValueEnum`
/// so the CLI binary does not have to depend on the doctor crate's
/// clap feature surface.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoctorLevelArg {
    Basic,
    Standard,
    Full,
    Paranoid,
    Custom,
}

impl DoctorLevelArg {
    pub fn to_doctor_level(self) -> sovereign_doctor::DoctorLevel {
        match self {
            DoctorLevelArg::Basic => sovereign_doctor::DoctorLevel::Basic,
            DoctorLevelArg::Standard => sovereign_doctor::DoctorLevel::Standard,
            DoctorLevelArg::Full => sovereign_doctor::DoctorLevel::Full,
            DoctorLevelArg::Paranoid => sovereign_doctor::DoctorLevel::Paranoid,
            DoctorLevelArg::Custom => sovereign_doctor::DoctorLevel::Custom,
        }
    }
}

/// The domain subcommand tree (F6). V0 implements `add` and `list`;
/// `remove` and `inspect` (TLS state) are V1.
#[derive(Subcommand, Debug)]
pub enum DomainCmd {
    /// Add a hostname that points to the current app. Pushes a Caddy
    /// route and writes a row in the `domain` table.
    Add {
        /// Hostname to add (e.g. `api.example.com`)
        hostname: String,
        /// App the hostname points to
        #[arg(long)]
        app: String,
    },
    /// List hostnames registered for an app
    List {
        /// App to list hostnames for
        #[arg(long)]
        app: String,
    },
}

/// The update subcommand tree (F10). V0 implements `check`, `apply`,
/// `rollback`, and `history`. Manifest base URL defaults to
/// `https://releases.sovereignruntime.dev`.
#[derive(Subcommand, Debug)]
pub enum UpdateCmd {
    /// Check for a newer version (no download, no install)
    Check {
        /// Release channel (V0 supports `stable` only)
        #[arg(long, value_enum, default_value_t = UpdateChannelArg::Stable)]
        channel: UpdateChannelArg,
        /// Manifest base URL (env: SOVEREIGN_UPDATE_MANIFEST)
        #[arg(long, env = "SOVEREIGN_UPDATE_MANIFEST")]
        manifest: Option<String>,
    },
    /// Download and apply a new version (atomic swap, previous binary is kept
    /// in the backup dir so `update rollback` can restore it)
    Apply {
        /// Release channel (V0 supports `stable` only)
        #[arg(long, value_enum, default_value_t = UpdateChannelArg::Stable)]
        channel: UpdateChannelArg,
        /// Target triple (e.g. x86_64-unknown-linux-musl); defaults to the
        /// current binary's triple
        #[arg(long)]
        target: Option<String>,
        /// Manifest base URL (env: SOVEREIGN_UPDATE_MANIFEST)
        #[arg(long, env = "SOVEREIGN_UPDATE_MANIFEST")]
        manifest: Option<String>,
        /// Don't actually swap; download + verify only
        #[arg(long)]
        no_swap: bool,
    },
    /// Roll back to the previous version (looks up the last update record
    /// in `update_history`)
    Rollback {
        /// Manifest base URL (env: SOVEREIGN_UPDATE_MANIFEST)
        #[arg(long, env = "SOVEREIGN_UPDATE_MANIFEST")]
        manifest: Option<String>,
    },
    /// List recent update history (newest first)
    History {
        /// Maximum records to show
        #[arg(long, default_value_t = 10)]
        limit: u32,
    },
}

/// The update channel argument for the CLI. Mirrors
/// `sovereign_core::ports::update::UpdateChannel`.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateChannelArg {
    Stable,
    Rc,
    Nightly,
}

impl UpdateChannelArg {
    pub fn to_update_channel(self) -> sovereign_core::ports::update::UpdateChannel {
        use sovereign_core::ports::update::UpdateChannel;
        match self {
            UpdateChannelArg::Stable => UpdateChannel::Stable,
            UpdateChannelArg::Rc => UpdateChannel::Rc,
            UpdateChannelArg::Nightly => UpdateChannel::Nightly,
        }
    }
}

impl Cli {
    /// Returns true if the user passed `--dry-run` (either globally or on
    /// the subcommand). F2 only has the global flag; F3+ subcommands may
    /// also accept it locally.
    #[allow(dead_code)]
    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// The effective output format (resolves `Auto` to `Text` or `Json`
    /// based on TTY detection). See `output::Format::resolve`.
    pub fn effective_format(&self) -> Format {
        self.format
    }
}
