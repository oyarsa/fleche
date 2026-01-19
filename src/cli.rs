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
        " (2026-01-20)\n", // Update date when releasing
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

    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Commands,
}

/// All available subcommands.
#[derive(Subcommand)]
pub enum Commands {
    /// Run a job on the remote cluster via Slurm
    ///
    /// Syncs your project, submits to Slurm, and streams output.
    /// Use --bg to run in background without streaming.
    Run {
        /// Job name from config, or command to run (in quotes)
        #[arg(value_name = "JOB_OR_COMMAND")]
        job_or_command: Option<String>,

        /// Override or provide command (if job name given)
        #[arg(long)]
        command: Option<String>,

        /// Run in background (don't stream output)
        #[arg(long)]
        bg: bool,

        /// Send terminal notification when job completes (useful with --bg)
        #[arg(long)]
        notify: bool,

        /// Set environment variable (repeatable)
        #[arg(long = "env", value_parser = parse_key_value)]
        env_vars: Vec<(String, String)>,

        /// Add tag for filtering/organization (repeatable)
        #[arg(long = "tag", value_parser = parse_key_value)]
        tags: Vec<(String, String)>,

        /// Override Slurm partition
        #[arg(long)]
        partition: Option<String>,

        /// Override wall time
        #[arg(long)]
        time: Option<String>,

        /// Override GPU count
        #[arg(long)]
        gpus: Option<u32>,

        /// Override CPU count
        #[arg(long)]
        cpus: Option<u32>,

        /// Override memory
        #[arg(long)]
        memory: Option<String>,

        /// Override constraint
        #[arg(long)]
        constraint: Option<String>,

        /// Override nodes
        #[arg(long)]
        nodes: Option<u32>,

        /// Override exclude
        #[arg(long)]
        exclude: Option<String>,

        /// Run after another job completes successfully
        ///
        /// Takes a job ID (or suffix). The new job will only start after
        /// the dependency job completes with exit code 0.
        #[arg(long)]
        after: Option<String>,

        /// Print generated sbatch script without submitting
        #[arg(long)]
        dry_run: bool,

        /// Run on specific host ("local" for local execution)
        #[arg(long)]
        host: Option<String>,

        /// Retry failed jobs with exponential backoff (e.g., --retry 3)
        #[arg(long)]
        retry: Option<u32>,

        /// Add a note/annotation to the job
        #[arg(long)]
        note: Option<String>,
    },

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
    },

    /// Show status of jobs
    ///
    /// Without arguments, lists recent jobs.
    /// With a job ID, shows detailed status.
    Status {
        /// Job ID to check (default: list recent jobs)
        job_id: Option<String>,

        /// Filter by status (pending, running, completed, failed, cancelled) - repeatable
        #[arg(long)]
        filter: Vec<String>,

        /// Filter by job name regex (e.g., "123" matches "train-123-xy", "^train" matches "train-foo")
        #[arg(long)]
        name: Option<String>,

        /// Filter by tag (repeatable)
        #[arg(long = "tag", value_parser = parse_key_value)]
        tags: Vec<(String, String)>,

        /// Number of jobs to show (default: 20)
        #[arg(short = 'n', long)]
        last: Option<usize>,
    },

    /// Fetch and display job logs
    ///
    /// Without a job ID, shows logs of the most recent job.
    Logs {
        /// Job ID (default: most recent job)
        job_id: Option<String>,

        /// Stream logs in real-time (Ctrl+C to disconnect)
        #[arg(long, short)]
        follow: bool,

        /// Show only stdout (default shows both stdout and stderr)
        #[arg(long)]
        stdout: bool,

        /// Show only stderr (default shows both stdout and stderr)
        #[arg(long)]
        stderr: bool,

        /// Show only the last N lines
        #[arg(short = 'n', long)]
        tail: Option<usize>,

        /// Strip ANSI escape codes from output (auto-detected when piped)
        #[arg(long)]
        raw: bool,

        /// Filter by tag when using default job (repeatable)
        #[arg(long = "tag", value_parser = parse_key_value)]
        tags: Vec<(String, String)>,
    },

    /// Download output files from remote to local
    ///
    /// Without a job ID, downloads outputs from the most recent job.
    Download {
        /// Job ID (default: most recent job)
        job_id: Option<String>,

        /// Download even if job is still running
        #[arg(long)]
        partial: bool,

        /// Specific path to download (default: all configured outputs)
        #[arg(long)]
        path: Option<String>,

        /// Filter outputs by glob pattern (repeatable). Prefix with ! to exclude.
        #[arg(long)]
        filter: Vec<String>,

        /// Filter by tag when using default job (repeatable)
        #[arg(long = "tag", value_parser = parse_key_value)]
        tags: Vec<(String, String)>,

        /// Show what would be downloaded without actually downloading
        #[arg(long)]
        dry_run: bool,
    },

    /// Cancel a running or pending job
    ///
    /// Without arguments, cancels the most recent running job.
    Cancel {
        /// Job ID (default: most recent running job)
        job_id: Option<String>,

        /// Cancel all running/pending jobs
        #[arg(long)]
        all: bool,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,

        /// Filter by tag (repeatable)
        #[arg(long = "tag", value_parser = parse_key_value)]
        tags: Vec<(String, String)>,
    },

    /// Remove job from registry and delete remote job files
    ///
    /// This removes job logs and metadata, but NOT the workspace.
    /// Use --workspace to also clear the shared workspace.
    Clean {
        /// Job ID (optional with --all or --older-than)
        job_id: Option<String>,

        /// Clean all completed/failed jobs
        #[arg(long)]
        all: bool,

        /// Clean jobs older than duration (e.g., 7d, 24h)
        #[arg(long)]
        older_than: Option<String>,

        /// Also delete the shared workspace
        #[arg(long)]
        workspace: bool,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,

        /// Filter by tag (repeatable)
        #[arg(long = "tag", value_parser = parse_key_value)]
        tags: Vec<(String, String)>,
    },

    /// List all unique tags across jobs
    Tags,

    /// Re-run a previous job with the same settings
    Rerun {
        /// Job ID to re-run
        job_id: String,

        /// Run in background (don't stream output)
        #[arg(long)]
        bg: bool,

        /// Add tag for filtering/organization (repeatable)
        #[arg(long = "tag", value_parser = parse_key_value)]
        tags: Vec<(String, String)>,
    },

    /// Create a starter fleche.toml in current directory
    Init,

    /// Validate configuration without running anything
    Check,

    /// Print a comprehensive usage guide (for LLMs and humans)
    Guide,

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
