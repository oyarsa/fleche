//! Job operations for running, monitoring, and managing remote jobs.
//!
//! This module contains the core business logic for fleche, including:
//! - Running jobs (syncing files, submitting to Slurm, streaming output)
//! - Executing commands directly via SSH
//! - Querying job status
//! - Viewing logs
//! - Downloading outputs back to local
//! - Listing, cancelling, and cleaning up jobs

mod cleanup;
mod display;
mod download;
mod logs;
mod ops;
mod run;
mod status;

use crate::config::Config;

// Re-export public API
pub use cleanup::{CleanJobsOptions, cancel_jobs, clean_jobs};
pub use download::download_outputs;
pub use logs::{ShowLogsOptions, show_logs};
pub use ops::{ping_cluster, show_stats, wait_for_job};
pub use run::{RunJobOptions, exec_command, rerun_job, run_job};
pub use status::{list_tags, note_job, show_status};

/// Returns the workspace path for a project on the remote host.
pub(crate) fn workspace_path(config: &Config) -> String {
    format!(
        "{}/{}/.fleche/workspace",
        config.remote.base_path, config.project_name
    )
}

/// Returns the jobs directory path for a project on the remote host.
pub(crate) fn jobs_base_path(config: &Config) -> String {
    format!(
        "{}/{}/.fleche/jobs",
        config.remote.base_path, config.project_name
    )
}

/// Returns the path for a specific job's metadata/logs directory.
pub(crate) fn job_path(config: &Config, job_id: &str) -> String {
    format!("{}/{}", jobs_base_path(config), job_id)
}
