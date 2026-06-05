// F2: The CLI surface — top-level struct, subcommand tree, global flags.
// Per docs/phase-00-mvp.md §F2 (the 8 sub-tasks) and
// docs/product-ux.md §2 (the 8 CLI non-negotiables).
//
// The CLI is the product. This module is the single source of truth for every
// flag, subcommand, and help string the operator sees. The clap derive macros
// generate the help text, the shell completions, the man pages, and the
// OpenAPI spec from this one definition.

use clap::{Parser, Subcommand, ValueEnum};

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
    },

    /// Log in to the control plane (opens the system browser, device-code flow)
    Login,

    /// Deploy an app
    Deploy {
        /// App to deploy (defaults to the app in the current directory)
        #[arg(long)]
        app: Option<String>,
        /// Image to deploy (overrides the app.yaml)
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

    /// Generate shell completions (bash, zsh, fish, nushell, powershell)
    Completions {
        /// Shell to generate completions for
        shell: Shell,
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
/// requirements.txt, go.mod, Gemfile, composer.json, etc.
#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Framework {
    #[default]
    Auto,
    Fastapi,
    Nextjs,
    Laravel,
    Go,
    Rails,
    Astro,
    Static,
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
    /// Verify a backup (restore to a scratch directory, assert row counts)
    Verify {
        /// Backup ID to verify
        backup_id: String,
    },
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

impl Cli {
    /// Returns true if the user passed `--dry-run` (either globally or on
    /// the subcommand). F2 only has the global flag; F3+ subcommands may
    /// also accept it locally.
    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// The effective output format (resolves `Auto` to `Text` or `Json`
    /// based on TTY detection). See `output::Format::resolve`.
    pub fn effective_format(&self) -> Format {
        self.format
    }
}
