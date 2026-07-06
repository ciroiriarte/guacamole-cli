//! Command-line surface for `gua` (issue #8).
//!
//! The command tree mirrors the README's proposed shape. Leaf commands that
//! belong to later roadmap phases are wired but return
//! [`gua_core::Error::Unimplemented`] so the skeleton is honest about scope.

use clap::{Parser, Subcommand, ValueEnum};

use gua_core::output::OutputFormat;

/// Native command-line / native client for Apache Guacamole.
#[derive(Debug, Parser)]
#[command(name = "gua", version, about, long_about = None)]
pub struct Cli {
    /// Configuration profile / context to use (overrides `GUA_PROFILE`).
    #[arg(long, global = true)]
    pub profile: Option<String>,

    /// Gateway base URL, e.g. `https://host/guacamole` (overrides the profile).
    #[arg(long, global = true)]
    pub server: Option<String>,

    /// Output format for command results.
    #[arg(long, value_enum, global = true)]
    pub output: Option<OutputArg>,

    /// Increase logging verbosity (repeatable).
    #[arg(short = 'v', long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Decrease logging verbosity (repeatable).
    #[arg(short = 'q', long, action = clap::ArgAction::Count, global = true)]
    pub quiet: u8,

    /// Emit logs as JSON.
    #[arg(long, global = true)]
    pub log_json: bool,

    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// Net verbosity for [`gua_core::logging::init`]: `+v` raises, `-q` lowers.
    pub fn verbosity(&self) -> i8 {
        (i16::from(self.verbose) - i16::from(self.quiet)).clamp(-1, 3) as i8
    }
}

/// Output format as a clap value enum, mapped to [`OutputFormat`].
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputArg {
    /// Human-friendly aligned table.
    Table,
    /// Machine-readable JSON.
    Json,
    /// YAML.
    Yaml,
}

impl From<OutputArg> for OutputFormat {
    fn from(v: OutputArg) -> Self {
        match v {
            OutputArg::Table => OutputFormat::Table,
            OutputArg::Json => OutputFormat::Json,
            OutputArg::Yaml => OutputFormat::Yaml,
        }
    }
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Authenticate to the gateway and store a session token.
    Login(LoginArgs),
    /// Invalidate the stored session token.
    Logout,
    /// Manage connections.
    #[command(subcommand)]
    Connection(ConnectionCmd),
    /// Manage active sessions.
    #[command(subcommand)]
    Session(SessionCmd),
    /// Connect to a connection (text → TUI, graphical → GUI window).
    Connect(ConnectArgs),
    /// Work with session recordings.
    #[command(subcommand)]
    Record(RecordCmd),
    /// Read and write local configuration.
    #[command(subcommand)]
    Config(ConfigCmd),
}

/// `gua login ...`
#[derive(Debug, clap::Args)]
pub struct LoginArgs {
    /// Username to authenticate as (or GUA_USERNAME).
    #[arg(short, long, env = "GUA_USERNAME")]
    pub username: Option<String>,

    /// Password (or GUA_PASSWORD). Omit to read from stdin.
    #[arg(short, long, env = "GUA_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,
}

/// `gua connection ...`
#[derive(Debug, Subcommand)]
pub enum ConnectionCmd {
    /// List connections.
    List,
    /// Show a single connection.
    Get {
        /// Connection identifier.
        id: String,
    },
    /// Create a connection.
    Create,
    /// Update a connection.
    Update {
        /// Connection identifier.
        id: String,
    },
    /// Delete a connection.
    Delete {
        /// Connection identifier.
        id: String,
    },
    /// Share a connection via a sharing profile.
    Share {
        /// Connection identifier.
        id: String,
    },
}

/// `gua session ...`
#[derive(Debug, Subcommand)]
pub enum SessionCmd {
    /// List active sessions.
    List,
    /// Kill an active session.
    Kill {
        /// Active-connection identifier (UUID).
        id: String,
    },
}

/// `gua connect ...`
#[derive(Debug, clap::Args)]
pub struct ConnectArgs {
    /// Connection identifier to connect to.
    pub id: String,
    /// Mount the connection's shared drive at this path via FUSE.
    #[arg(long, value_name = "PATH")]
    pub mount: Option<String>,
}

/// `gua record ...`
#[derive(Debug, Subcommand)]
pub enum RecordCmd {
    /// Download a recording.
    Get {
        /// Recording identifier.
        id: String,
    },
    /// Play back a recording.
    Play {
        /// Recording identifier.
        id: String,
    },
}

/// `gua config ...`
#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    /// List profiles and their fields.
    List,
    /// Print a single field from a profile.
    Get {
        /// Field name (server|data_source|output|tls_insecure).
        key: String,
    },
    /// Set a single field on a profile.
    Set {
        /// Field name (server|data_source|output|tls_insecure).
        key: String,
        /// New value.
        value: String,
    },
    /// Print the path to the configuration file.
    Path,
}
