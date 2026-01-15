use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "fleche")]
#[command(about = "Remote job runner for Slurm clusters")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run a job on the remote cluster
    Run {
        /// Job name from config (optional if --command is provided)
        job_name: Option<String>,

        /// Override or provide command
        #[arg(long)]
        command: Option<String>,

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

        /// Tail output after submission
        #[arg(long)]
        follow: bool,

        /// Print generated sbatch script without submitting
        #[arg(long)]
        dry_run: bool,
    },

    /// Show status of a job, or all recent jobs if no ID provided
    Status {
        /// Job ID to check
        job_id: Option<String>,
    },

    /// Fetch and display job logs
    Logs {
        /// Job ID
        job_id: String,

        /// Stream logs in real-time (Ctrl+C to disconnect)
        #[arg(long)]
        follow: bool,

        /// Show stderr instead of stdout
        #[arg(long)]
        stderr: bool,

        /// Show both stdout and stderr
        #[arg(long)]
        both: bool,
    },

    /// Pull output files from a completed job to local project directory
    Sync {
        /// Job ID
        job_id: String,

        /// Suppress warning when syncing from a running job
        #[arg(long)]
        partial: bool,
    },

    /// List all jobs from the registry
    List {
        /// Filter by project path
        #[arg(long)]
        project: Option<String>,

        /// Filter by status (pending, running, completed, failed, cancelled)
        #[arg(long)]
        status: Option<String>,

        /// Filter by tag (repeatable, all must match)
        #[arg(long = "tag", value_parser = parse_key_value)]
        tags: Vec<(String, String)>,

        /// Shorthand for --status failed
        #[arg(long)]
        failed: bool,

        /// Shorthand for --status running
        #[arg(long)]
        running: bool,

        /// Shorthand for --status completed
        #[arg(long)]
        completed: bool,
    },

    /// Cancel a running or pending job
    Cancel {
        /// Job ID
        job_id: String,
    },

    /// Remove job from registry and delete remote directory
    Clean {
        /// Job ID (optional with --all or --older-than)
        job_id: Option<String>,

        /// Clean all completed/failed jobs
        #[arg(long)]
        all: bool,

        /// Clean jobs older than duration (e.g., 7d, 24h)
        #[arg(long)]
        older_than: Option<String>,
    },

    /// Create a starter fleche.toml in current directory
    Init,

    /// Validate configuration without running anything
    Check,

    /// Print a comprehensive usage guide (for LLMs and humans)
    Guide,
}

fn parse_key_value(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.splitn(2, '=').collect();
    if parts.len() != 2 {
        return Err(format!(
            "Invalid format '{s}'. Expected KEY=VALUE"
        ));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}
