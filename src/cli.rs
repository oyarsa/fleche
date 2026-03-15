//! Command-line interface definition.
//!
//! This module defines the CLI structure using clap. All subcommands and their
//! arguments are defined here, with argument parsing handled by clap's derive macros.

use clap::{Parser, Subcommand};
use clap_complete::Shell;

/// GNU-style long version string with copyright and license.
///
/// Note: Update the date literal below when cutting a new release.
fn long_version() -> &'static str {
    concat!(
        env!("CARGO_PKG_VERSION"),
        " (2026-03-09)\n\n", // Update date when releasing
        "Copyright (C) 2026 Italo Silva\n",
        "License GPLv3+: GNU GPL version 3 or later <https://gnu.org/licenses/gpl.html>\n",
        "This is free software: you are free to change and redistribute it.\n",
        "There is NO WARRANTY, to the extent permitted by law."
    )
}

/// The main CLI structure for fleche.
#[derive(Parser)]
#[command(name = "fleche")]
#[command(about = "Remote job runner for Slurm clusters")]
#[command(version, long_version = long_version())]
pub struct Cli {
    /// Run as if fleche was started in this directory
    #[arg(short = 'C', long = "directory", global = true, value_name = "PATH")]
    pub directory: Option<std::path::PathBuf>,

    /// Enable verbose SSH output for debugging connection issues
    #[arg(long, global = true)]
    pub debug: bool,

    /// Output results as JSON (for scripting and AI agents)
    #[arg(long, global = true)]
    pub json: bool,

    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Commands,
}

/// All available subcommands.
#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Commands {
    /// Run a job on the remote cluster via Slurm
    ///
    /// Syncs your project, submits to Slurm, and streams output.
    /// Use --bg to run in background without streaming.
    Run(RunArgs),

    /// Execute a command directly via SSH (no Slurm)
    ///
    /// Syncs your project and runs the command directly over SSH.
    /// Useful for quick tests or interactive work.
    Exec {
        /// Command to run (in quotes)
        command: String,

        /// Set environment variable (repeatable)
        #[arg(long = "env", value_parser = parse_key_value)]
        env_vars: Vec<(String, String)>,

        /// Run on specific host ("local" for local execution)
        #[arg(long)]
        host: Option<String>,

        /// Skip syncing project code and inputs before execution
        #[arg(long)]
        no_sync: bool,
    },

    /// Show status of jobs
    ///
    /// Without arguments, lists recent jobs.
    /// With a job ID, shows detailed status.
    Status(StatusArgs),

    /// Fetch and display job logs
    ///
    /// Without a job ID, shows logs of the most recent job.
    Logs(LogsArgs),

    /// Download output files from remote to local
    ///
    /// Without a job ID, downloads outputs from the most recent job.
    Download(DownloadArgs),

    /// Cancel a running or pending job
    ///
    /// Without arguments, cancels the most recent running job.
    Cancel(CancelArgs),

    /// Remove job from registry and delete remote job files
    ///
    /// This removes job logs and metadata, but NOT the workspace.
    /// Use --workspace to also clear the shared workspace.
    /// Use --archive to hide jobs without deleting them.
    Clean(CleanArgs),

    /// List available jobs from configuration
    ///
    /// Reads fleche.toml (and fleche/*.toml files) and prints all defined
    /// job names with their commands.
    Jobs,

    /// List all unique tags across jobs
    Tags,

    /// Re-run a previous job with the same settings
    Rerun {
        /// Job ID to re-run
        job_id: String,

        /// Run in background (don't stream output)
        #[arg(long)]
        bg: bool,

        /// Send push notifications via ntfy.sh on state changes
        #[arg(long, value_name = "TOPIC")]
        ntfy: Option<String>,

        /// Add tag for filtering/organization (repeatable)
        #[arg(long = "tag", value_parser = parse_key_value)]
        tags: Vec<(String, String)>,
    },

    /// Create a starter fleche.toml in current directory
    Init,

    /// Validate configuration without running anything
    ///
    /// By default, only validates the local configuration file.
    /// Use --remote to also check SSH connectivity, Slurm availability,
    /// partition validity, and disk space.
    Check {
        /// Also validate against the remote server
        #[arg(long)]
        remote: bool,
    },

    /// Print a comprehensive usage guide (for LLMs and humans)
    Guide,

    /// Comprehensive diagnostic for troubleshooting
    ///
    /// Checks local environment, SSH connectivity, Slurm status, and registry
    /// health. Provides suggestions for fixing common issues.
    Doctor,

    /// Check cluster health by pinging the Slurm controller
    ///
    /// Runs `scontrol ping` on the remote host to verify the Slurm
    /// scheduler is responsive. Useful for diagnosing timeout issues.
    Ping,

    /// Wait for a job to complete
    ///
    /// Polls job status until it reaches a terminal state (completed, failed, cancelled).
    /// Useful for scripting or waiting on background jobs.
    Wait {
        /// Job ID to wait for (default: most recent job)
        job_id: Option<String>,

        /// Send terminal notification when job completes
        #[arg(long)]
        notify: bool,

        /// Send push notifications via ntfy.sh on state changes
        #[arg(long, value_name = "TOPIC")]
        ntfy: Option<String>,

        /// Filter by tag when using default job (repeatable)
        #[arg(long = "tag", value_parser = parse_key_value)]
        tags: Vec<(String, String)>,
    },

    /// Generate shell completions
    ///
    /// Prints completion script for the specified shell to stdout.
    /// Add to your shell config, e.g.: `fleche completions bash >> ~/.bashrc`
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Show resource usage statistics for jobs
    ///
    /// Queries Slurm's sacct to show elapsed time, CPU time, memory usage,
    /// and allocated resources for completed jobs.
    Stats {
        /// Job ID to show stats for (default: most recent job)
        job_id: Option<String>,

        /// Show stats for last N jobs
        #[arg(long, short = 'n', default_value = "1")]
        last: usize,

        /// Filter by tag (repeatable)
        #[arg(long = "tag", value_parser = parse_key_value)]
        tags: Vec<(String, String)>,
    },

    /// Add or view a note on a job
    ///
    /// Without a note, displays the existing note for the job.
    /// With a note, sets or updates the job's note.
    Note {
        /// Job ID to annotate
        job_id: String,

        /// Note text to set (omit to view existing note)
        note: Option<String>,
    },

    /// Compare two jobs side-by-side
    ///
    /// Shows differences in configuration, environment, Slurm settings,
    /// tags, and status between two jobs.
    Compare {
        /// First job ID
        job_a: String,

        /// Second job ID
        job_b: String,
    },

    /// Run a command through a SOCKS proxy tunnel to the remote host
    ///
    /// Opens an SSH dynamic port forward to the configured remote, sets
    /// proxy environment variables (`ALL_PROXY`, `HTTP_PROXY`, `HTTPS_PROXY`,
    /// etc.), and runs the given command. The tunnel is cached per-host
    /// so repeated invocations reuse the same connection.
    ///
    /// Example: fleche proxy -- curl <https://example.com>
    Proxy {
        /// Command and arguments to run through the proxy
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,

        /// SOCKS proxy port (default: random available port)
        #[arg(long)]
        port: Option<u16>,

        /// Override remote host (default: from fleche.toml)
        #[arg(long)]
        host: Option<String>,
    },
}

#[derive(clap::Args)]
pub struct RunArgs {
    /// Job name from config, or command to run (in quotes)
    #[arg(value_name = "JOB_OR_COMMAND")]
    pub job_or_command: Option<String>,

    /// Override or provide command (if job name given)
    #[arg(long)]
    pub command: Option<String>,

    /// Run in background (don't stream output)
    #[arg(long)]
    pub bg: bool,

    /// Send terminal notification when job completes (useful with --bg)
    #[arg(long)]
    pub notify: bool,

    /// Send push notifications via ntfy.sh on state changes
    #[arg(long, value_name = "TOPIC")]
    pub ntfy: Option<String>,

    /// Set environment variable (repeatable)
    #[arg(long = "env", value_parser = parse_key_value)]
    pub env_vars: Vec<(String, String)>,

    /// Add tag for filtering/organization (repeatable)
    #[arg(long = "tag", value_parser = parse_key_value)]
    pub tags: Vec<(String, String)>,

    /// Override Slurm partition
    #[arg(long)]
    pub partition: Option<String>,

    /// Override wall time
    #[arg(long)]
    pub time: Option<String>,

    /// Override GPU count
    #[arg(long)]
    pub gpus: Option<u32>,

    /// Override CPU count
    #[arg(long)]
    pub cpus: Option<u32>,

    /// Override memory
    #[arg(long)]
    pub memory: Option<String>,

    /// Override constraint
    #[arg(long)]
    pub constraint: Option<String>,

    /// Override nodes
    #[arg(long)]
    pub nodes: Option<u32>,

    /// Override exclude
    #[arg(long)]
    pub exclude: Option<String>,

    /// Run after another job completes successfully
    ///
    /// Takes a job ID (or suffix). The new job will only start after
    /// the dependency job completes with exit code 0.
    #[arg(long)]
    pub after: Option<String>,

    /// Print generated sbatch script without submitting
    #[arg(long)]
    pub dry_run: bool,

    /// Run on specific host ("local" for local execution)
    #[arg(long)]
    pub host: Option<String>,

    /// Run directly via SSH instead of submitting to Slurm
    #[arg(long)]
    pub exec: bool,

    /// Retry failed jobs with exponential backoff (e.g., --retry 3)
    #[arg(long)]
    pub retry: Option<u32>,

    /// Add a note/annotation to the job
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(clap::Args)]
pub struct StatusArgs {
    /// Job ID to check (default: list recent jobs)
    pub job_id: Option<String>,

    /// Filter by status (pending, running, completed, failed, cancelled) - repeatable
    #[arg(long)]
    pub filter: Vec<String>,

    /// Filter by job name regex (e.g., "123" matches "train-123-xy", "^train" matches "train-foo")
    #[arg(long)]
    pub name: Option<String>,

    /// Filter by tag (repeatable)
    #[arg(long = "tag", value_parser = parse_key_value)]
    pub tags: Vec<(String, String)>,

    /// Number of jobs to show (default: 20)
    #[arg(short = 'n', long)]
    pub last: Option<usize>,

    /// Show only archived jobs
    #[arg(long)]
    pub archived: bool,

    /// Show all jobs including archived
    #[arg(long = "all-jobs", conflicts_with = "archived")]
    pub all_jobs: bool,

    /// Hide the subtitle line (job name, tags, note) below each row
    #[arg(long)]
    pub compact: bool,
}

#[derive(clap::Args)]
pub struct LogsArgs {
    /// Job ID (default: most recent job)
    pub job_id: Option<String>,

    /// Stream logs in real-time (Ctrl+C to disconnect)
    #[arg(long, short)]
    pub follow: bool,

    /// Show only stdout (default shows both stdout and stderr)
    #[arg(long)]
    pub stdout: bool,

    /// Show only stderr (default shows both stdout and stderr)
    #[arg(long)]
    pub stderr: bool,

    /// Show only the last N lines
    #[arg(short = 'n', long)]
    pub tail: Option<usize>,

    /// Strip ANSI escape codes from output (auto-detected when piped)
    #[arg(long)]
    pub raw: bool,

    /// Filter by tag when using default job (repeatable)
    #[arg(long = "tag", value_parser = parse_key_value)]
    pub tags: Vec<(String, String)>,

    /// Filter by note content (regex pattern, case-insensitive)
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(clap::Args)]
pub struct DownloadArgs {
    /// Job ID (default: most recent job)
    pub job_id: Option<String>,

    /// Download even if job is still running
    #[arg(long)]
    pub partial: bool,

    /// Specific path to download (default: all configured outputs)
    #[arg(long)]
    pub path: Option<String>,

    /// Filter outputs by glob pattern (repeatable). Prefix with ! to exclude.
    #[arg(long)]
    pub filter: Vec<String>,

    /// Filter by tag when using default job (repeatable)
    #[arg(long = "tag", value_parser = parse_key_value)]
    pub tags: Vec<(String, String)>,

    /// Show what would be downloaded without actually downloading
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(clap::Args)]
pub struct CancelArgs {
    /// Job ID (default: most recent running job)
    pub job_id: Option<String>,

    /// Cancel all running/pending jobs
    #[arg(long)]
    pub all: bool,

    /// Show what would be cancelled without actually cancelling
    #[arg(long)]
    pub dry_run: bool,

    /// Skip confirmation prompt
    #[arg(short, long)]
    pub yes: bool,

    /// Filter by tag (repeatable)
    #[arg(long = "tag", value_parser = parse_key_value)]
    pub tags: Vec<(String, String)>,
}

#[derive(clap::Args)]
pub struct CleanArgs {
    /// Job ID (optional with --all or --older-than)
    pub job_id: Option<String>,

    /// Clean all completed/failed jobs
    #[arg(long)]
    pub all: bool,

    /// Clean jobs older than duration (e.g., 7d, 24h)
    #[arg(long)]
    pub older_than: Option<String>,

    /// Also delete the shared workspace
    #[arg(long)]
    pub workspace: bool,

    /// Archive job instead of deleting (hides from listings)
    #[arg(long)]
    pub archive: bool,

    /// Restore archived job to normal listings
    #[arg(long, conflicts_with_all = ["archive", "workspace"])]
    pub unarchive: bool,

    /// Show what would be done without actually doing it
    #[arg(long)]
    pub dry_run: bool,

    /// Skip confirmation prompt
    #[arg(short, long)]
    pub yes: bool,

    /// Filter by tag (repeatable)
    #[arg(long = "tag", value_parser = parse_key_value)]
    pub tags: Vec<(String, String)>,
}

/// Parses a KEY=VALUE string into a tuple.
fn parse_key_value(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.splitn(2, '=').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid format '{s}'. Expected KEY=VALUE"));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_key_value_simple() {
        let (k, v) = parse_key_value("FOO=bar").unwrap();
        assert_eq!(k, "FOO");
        assert_eq!(v, "bar");
    }

    #[test]
    fn test_parse_key_value_with_equals_in_value() {
        // Value can contain equals signs
        let (k, v) = parse_key_value("CONFIG=a=b=c").unwrap();
        assert_eq!(k, "CONFIG");
        assert_eq!(v, "a=b=c");
    }

    #[test]
    fn test_parse_key_value_empty_value() {
        let (k, v) = parse_key_value("EMPTY=").unwrap();
        assert_eq!(k, "EMPTY");
        assert_eq!(v, "");
    }

    #[test]
    fn test_parse_key_value_spaces_in_value() {
        let (k, v) = parse_key_value("MSG=hello world").unwrap();
        assert_eq!(k, "MSG");
        assert_eq!(v, "hello world");
    }

    #[test]
    fn test_parse_key_value_no_equals() {
        assert!(parse_key_value("NOEQUALS").is_err());
    }

    #[test]
    fn test_parse_key_value_empty() {
        assert!(parse_key_value("").is_err());
    }
}
