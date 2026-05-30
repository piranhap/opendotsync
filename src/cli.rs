use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "opendotsync",
    about = "Cross-machine dotfile synchronization with intelligent merge strategies",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Override home directory
    #[arg(long, global = true)]
    pub home: Option<PathBuf>,

    /// Override config file path
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Verbose output
    #[arg(long, short, global = true)]
    pub verbose: bool,

    /// Suppress non-error output
    #[arg(long, short, global = true)]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize opendotsync in the current home directory
    Init {
        /// Backend type: local, rclone, github
        #[arg(long, default_value = "local")]
        backend: String,

        /// Profile name
        #[arg(long, default_value = "default")]
        profile: String,

        /// Storage path: local dir, rclone remote path, or GitHub repo URL
        #[arg(long)]
        path: Option<PathBuf>,
    },

    /// Create snapshot and upload
    Push {
        /// Overwrite remote even if versions conflict
        #[arg(long)]
        force: bool,

        /// Optional note attached to this snapshot
        #[arg(long, short)]
        message: Option<String>,

        /// Fail on version conflict instead of auto-pulling first
        #[arg(long)]
        no_auto_pull: bool,
    },

    /// Download latest snapshot and merge
    Pull {
        /// Pull a specific version instead of latest
        #[arg(long)]
        version: Option<u64>,

        /// Overwrite all local files (no merge, destructive)
        #[arg(long)]
        overwrite: bool,
    },

    /// Show sync status
    Status,

    /// Show projected snapshot size with breakdown by file
    Du,

    /// Pull remote changes, merge, then push local changes
    Sync {
        /// Show what would happen without making changes
        #[arg(long)]
        dry_run: bool,

        /// Force push even if local version is behind
        #[arg(long)]
        force: bool,
    },

    /// Show diff between local files and last-pulled snapshot
    Diff {
        /// Diff against latest remote snapshot
        #[arg(long)]
        remote: bool,
    },

    /// Manage the tracked paths list
    Paths {
        #[command(subcommand)]
        action: PathsAction,
    },

    /// Manage merge conflicts
    Conflicts {
        #[command(subcommand)]
        action: ConflictsAction,
    },

    /// Show snapshot history
    Log {
        /// Number of entries to show
        #[arg(short, default_value = "10")]
        n: usize,
    },

    /// Restore files from a specific snapshot version
    Restore {
        /// Version to restore from
        version: u64,

        /// Restore only a specific file
        #[arg(long)]
        path: Option<String>,
    },

    /// Manage profiles
    Profiles {
        #[command(subcommand)]
        action: ProfilesAction,
    },

    /// Show or edit configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Generate or manage encryption keys
    EncryptKey {
        #[command(subcommand)]
        action: EncryptKeyAction,
    },

    /// Install systemd/launchd timer for automatic sync
    InstallTimer {
        /// Uninstall instead of install
        #[arg(long)]
        uninstall: bool,
    },
}

#[derive(Subcommand)]
pub enum PathsAction {
    /// Show current paths list (allowlist + blacklist)
    List,
    /// Open paths list in $EDITOR
    Edit,
    /// Add a path to the allowlist (things to sync)
    Add {
        path: String,
        /// Merge type: none, history, union
        #[arg(default_value = "none")]
        merge_type: String,
    },
    /// Remove a path from the allowlist
    Remove { path: String },
    /// Manage the blacklist (patterns never to sync)
    Exclude {
        #[command(subcommand)]
        action: ExcludeAction,
    },
    /// Check whether a given path would be synced, and why
    Check { path: String },
}

#[derive(Subcommand)]
pub enum ExcludeAction {
    /// List blacklist patterns
    List,
    /// Add a blacklist pattern (glob, e.g. "*.secret" or ".ssh/id_*")
    Add { pattern: String },
    /// Remove a blacklist pattern
    Remove { pattern: String },
}

#[derive(Subcommand)]
pub enum ConflictsAction {
    /// Show unresolved conflicts
    List,
    /// Show diff between local and conflicted version
    Show { path: String },
    /// Keep local, discard conflict
    AcceptLocal { path: String },
    /// Replace local with conflicted version
    AcceptRemote { path: String },
    /// Open merge tool for all conflicts
    Resolve,
}

#[derive(Subcommand)]
pub enum ProfilesAction {
    /// List all profiles
    List,
    /// Create a new profile
    Create { name: String },
    /// Switch to a different profile
    Switch { name: String },
    /// Merge one profile into another
    Merge { from: String, to: String },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print current config
    Show,
    /// Open config in $EDITOR
    Edit,
}

#[derive(Subcommand)]
pub enum EncryptKeyAction {
    /// Create new age keypair
    Generate,
    /// Export public key
    Export,
}
