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
pub use run::{
    RunJobOptions, cancel_remote_direct_job, exec_command, get_remote_direct_job_status, rerun_job,
    run_job,
};
pub use status::{list_tags, note_job, show_status};

/// Returns the workspace path for a project on the remote host.
pub fn workspace_path(config: &Config) -> String {
    format!(
        "{}/{}/.fleche/workspace",
        config.remote.base_path, config.project_name
    )
}

/// Returns the jobs directory path for a project on the remote host.
pub fn jobs_base_path(config: &Config) -> String {
    format!(
        "{}/{}/.fleche/jobs",
        config.remote.base_path, config.project_name
    )
}

/// Returns the path for a specific job's metadata/logs directory.
pub fn job_path(config: &Config, job_id: &str) -> String {
    format!("{}/{}", jobs_base_path(config), job_id)
}

/// Returns the jobs directory path from a workspace path.
///
/// Workspace paths are expected to end with `/workspace`.
pub fn jobs_base_from_workspace(workspace: &str) -> String {
    format!("{}/jobs", workspace.trim_end_matches("/workspace"))
}

/// Returns the path for a specific job's metadata/logs directory from a workspace path.
pub fn job_path_from_workspace(workspace: &str, job_id: &str) -> String {
    format!("{}/{}", jobs_base_from_workspace(workspace), job_id)
}
