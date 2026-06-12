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
        " (2026-06-12)\n\n", // Update date when releasing
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

    /// Continuously display the job status table, refreshing periodically
    ///
    /// Clears the screen and redraws the recent-jobs table every N seconds
    /// (default 1s), like the Unix `watch` command. Accepts the same filters
    /// as `status`. Runs until interrupted with Ctrl+C.
    Watch(WatchArgs),

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

    /// Archive or delete finished jobs
    ///
    /// By default, jobs are archived (hidden from listings but preserved).
    /// Use --delete to permanently remove jobs and their remote files.
    /// Use --workspace with --delete to also clear the shared workspace.
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

    /// Print or install the fleche skill for AI coding agents
    ///
    /// Prints the fleche skill reference to stdout. Use --install to
    /// write it to .agents/skills/ (with a symlink from .claude/skills/).
    Skill {
        /// Install the skill to project or global scope
        #[arg(long, value_name = "SCOPE")]
        install: Option<InstallScope>,
    },

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

        /// Filter the default job by 'type:query' (repeatable, `ANDed`).
        ///
        /// Types: status, name (ID regex), tag (key=value), note (regex).
        /// Without a type prefix the value is a status.
        #[arg(short = 'f', long)]
        filter: Vec<String>,
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

        /// Filter jobs by 'type:query' (repeatable, `ANDed` together).
        ///
        /// Types: status, name (ID regex), tag (key=value), note (regex).
        /// Without a type prefix the value is a status.
        #[arg(short = 'f', long)]
        filter: Vec<String>,
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

/// Where to install the fleche skill.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum InstallScope {
    /// Install to the current project directory
    Project,
    /// Install to the user-level config directory
    Global,
}

#[derive(clap::Args)]
pub struct RunArgs {
    /// Job name from config
    #[arg(value_name = "JOB")]
    pub job_or_command: Option<String>,

    /// Override a job command, or provide an ad hoc command when no job is given
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

    /// Filter jobs by 'type:query' (repeatable, `ANDed` together).
    ///
    /// Types: status, name (ID regex), tag (key=value), note (regex). Without a
    /// type prefix the value is a status. Examples: -f running, -f name:^train,
    /// -f tag:env=prod, -f note:baseline.
    #[arg(short = 'f', long)]
    pub filter: Vec<String>,

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
pub struct WatchArgs {
    /// Filter jobs by 'type:query' (repeatable, `ANDed` together).
    ///
    /// Types: status, name (ID regex), tag (key=value), note (regex). Without a
    /// type prefix the value is a status. Examples: -f running, -f name:^train,
    /// -f tag:env=prod, -f note:baseline.
    #[arg(short = 'f', long)]
    pub filter: Vec<String>,

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

    /// Refresh interval in seconds (fractional allowed, e.g. 0.5)
    #[arg(short = 'i', long, default_value_t = 1.0)]
    pub interval: f64,
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

    /// Filter the default job by 'type:query' (repeatable, `ANDed` together).
    ///
    /// Types: status, name (ID regex), tag (key=value), note (regex). Without a
    /// type prefix the value is a status. (No -f short here; -f is --follow.)
    #[arg(long)]
    pub filter: Vec<String>,
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
    pub glob: Vec<String>,

    /// Select the job by 'type:query' (repeatable, `ANDed` together).
    ///
    /// Types: status, name (ID regex), tag (key=value), note (regex). Without a
    /// type prefix the value is a status.
    #[arg(short = 'f', long)]
    pub filter: Vec<String>,

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

    /// Filter jobs by 'type:query' (repeatable, `ANDed` together).
    ///
    /// Types: status, name (ID regex), tag (key=value), note (regex). Without a
    /// type prefix the value is a status.
    #[arg(short = 'f', long)]
    pub filter: Vec<String>,
}

#[derive(clap::Args)]
pub struct CleanArgs {
    /// Job ID (optional with --all or --before)
    pub job_id: Option<String>,

    /// Clean all completed/failed jobs
    #[arg(long)]
    pub all: bool,

    /// Filter jobs by 'type:query' (repeatable, `ANDed` together).
    ///
    /// Types: status, name (ID regex), tag (key=value), note (regex). Without a
    /// type prefix the value is a status.
    #[arg(short = 'f', long)]
    pub filter: Vec<String>,

    /// Clean jobs created before a delta (7d, 24h, 30m) or timestamp
    /// (2026-06-05, '2026-06-05 14:30', or RFC3339)
    #[arg(long)]
    pub before: Option<String>,

    /// Permanently delete jobs instead of archiving
    #[arg(long, conflicts_with = "unarchive")]
    pub delete: bool,

    /// Also delete the shared workspace (requires --delete)
    #[arg(long, requires = "delete")]
    pub workspace: bool,

    /// Target archived jobs (for --delete or --unarchive)
    #[arg(long)]
    pub archived: bool,

    /// Restore archived job to normal listings
    #[arg(long, conflicts_with = "delete")]
    pub unarchive: bool,

    /// Show what would be done without actually doing it
    #[arg(long)]
    pub dry_run: bool,

    /// Skip confirmation prompt
    #[arg(short, long)]
    pub yes: bool,
}

/// Parses a KEY=VALUE string into a tuple.
fn parse_key_value(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.splitn(2, '=').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid format '{s}'. Expected KEY=VALUE"));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

/// A single parsed `--filter` clause selecting jobs by one attribute.
#[derive(Debug, PartialEq, Eq)]
pub enum JobFilter {
    /// Match jobs whose status equals this value.
    Status(String),
    /// Match jobs whose ID matches this regex.
    Name(String),
    /// Match jobs carrying this `key=value` tag.
    Tag(String, String),
    /// Match jobs whose note matches this regex (case-insensitive).
    Note(String),
}

/// Parses one `--filter` value of the form `type:query`.
///
/// Recognized types are `status`, `name`, `tag`, and `note`. A value with no
/// recognized `type:` prefix is treated as a status (the common case), so
/// `--filter completed` and `--filter status:completed` are equivalent. The
/// split is on the first `:` only, so regex queries may contain colons (e.g.
/// `name:(?i:train)`).
fn parse_job_filter(raw: &str) -> Result<JobFilter, String> {
    let Some((kind, query)) = raw.split_once(':') else {
        // No prefix: treat the whole value as a status.
        return Ok(JobFilter::Status(raw.to_string()));
    };

    match kind {
        "status" => Ok(JobFilter::Status(query.to_string())),
        "name" => Ok(JobFilter::Name(query.to_string())),
        "note" => Ok(JobFilter::Note(query.to_string())),
        "tag" => {
            let (key, value) = parse_key_value(query)
                .map_err(|_| format!("tag filter must be 'tag:key=value', got 'tag:{query}'"))?;
            Ok(JobFilter::Tag(key, value))
        }
        other => Err(format!(
            "unknown filter type '{other}' in '{raw}'; \
             expected status, name, tag, or note (or omit the type for a status)"
        )),
    }
}

/// The set of job-selection predicates gathered from `--filter`, bucketed by
/// attribute.
///
/// All buckets are `ANDed` together when listing. Statuses act as set membership
/// (a job matches if its status is any of them); repeated `name`/`note` clauses
/// keep the last value.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FilterSelection {
    /// Statuses to match (membership test).
    pub statuses: Vec<String>,
    /// Job-ID regex, if any.
    pub name: Option<String>,
    /// Tags that must all be present.
    pub tags: Vec<(String, String)>,
    /// Note regex, if any.
    pub note: Option<String>,
}

/// Builds a [`FilterSelection`] from the unified `--filter` values.
pub fn collect_filters(filters: &[String]) -> Result<FilterSelection, String> {
    let mut selection = FilterSelection::default();

    for raw in filters {
        match parse_job_filter(raw)? {
            JobFilter::Status(s) => selection.statuses.push(s),
            JobFilter::Name(n) => selection.name = Some(n),
            JobFilter::Note(n) => selection.note = Some(n),
            JobFilter::Tag(k, v) => selection.tags.push((k, v)),
        }
    }

    Ok(selection)
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
    fn test_parse_job_filter_defaults_to_status() {
        assert_eq!(
            parse_job_filter("completed").unwrap(),
            JobFilter::Status("completed".to_string())
        );
        assert_eq!(
            parse_job_filter("status:running").unwrap(),
            JobFilter::Status("running".to_string())
        );
    }

    #[test]
    fn test_parse_job_filter_no_prefix_equals_status_prefix() {
        // An untyped value must be exactly equivalent to a `status:` value.
        for status in ["completed", "running", "pending", "failed", "cancelled"] {
            assert_eq!(
                parse_job_filter(status).unwrap(),
                parse_job_filter(&format!("status:{status}")).unwrap(),
            );
        }
    }

    #[test]
    fn test_parse_job_filter_typed() {
        assert_eq!(
            parse_job_filter("name:^train").unwrap(),
            JobFilter::Name("^train".to_string())
        );
        assert_eq!(
            parse_job_filter("note:baseline").unwrap(),
            JobFilter::Note("baseline".to_string())
        );
        assert_eq!(
            parse_job_filter("tag:env=prod").unwrap(),
            JobFilter::Tag("env".to_string(), "prod".to_string())
        );
    }

    #[test]
    fn test_parse_job_filter_splits_on_first_colon() {
        // Regex queries may contain colons; only the first splits type/query.
        assert_eq!(
            parse_job_filter("name:(?i:train)").unwrap(),
            JobFilter::Name("(?i:train)".to_string())
        );
    }

    #[test]
    fn test_parse_job_filter_unknown_type_errors() {
        assert!(parse_job_filter("naem:foo").is_err());
    }

    #[test]
    fn test_parse_job_filter_bad_tag_errors() {
        assert!(parse_job_filter("tag:novalue").is_err());
    }

    #[test]
    fn test_collect_filters_buckets() {
        let filters = vec![
            "running".to_string(),
            "name:^train".to_string(),
            "tag:env=prod".to_string(),
            "tag:team=ml".to_string(),
            "note:baseline".to_string(),
        ];
        let sel = collect_filters(&filters).unwrap();

        assert_eq!(sel.statuses, vec!["running".to_string()]);
        assert_eq!(sel.name, Some("^train".to_string()));
        assert_eq!(sel.note, Some("baseline".to_string()));
        // Multiple tags are kept and ANDed.
        assert_eq!(
            sel.tags,
            vec![
                ("env".to_string(), "prod".to_string()),
                ("team".to_string(), "ml".to_string()),
            ]
        );
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
