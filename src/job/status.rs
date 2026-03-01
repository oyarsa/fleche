//! Job status operations - viewing status, notes, and tags.

use crate::error::{FlecheError, Result};
use crate::local;
use crate::registry::{JobRecord, JobStatus, Registry, build_job_filter_pattern};
use crate::runtime::RuntimeCtx;
use crate::slurm::get_job_status;
use console::style;
use regex::Regex;
use std::path::PathBuf;

use super::display::{print_indexed_job_table, print_job_details, print_job_table};
use super::get_remote_direct_job_status;
use super::job_path_from_workspace;

/// Default number of jobs to show when no limit is specified and no config is available.
const DEFAULT_LIST_LIMIT: usize = 20;

/// Shows the status of a specific job or lists recent jobs.
///
/// The `archived_filter` parameter controls visibility of archived jobs:
/// - `None`: Show only non-archived jobs (default)
/// - `Some(true)`: Show only archived jobs
/// - `Some(false)`: Show all jobs (both archived and non-archived)
///
/// The `default_limit` parameter specifies the default number of jobs to show
/// when `last` is None. If `default_limit` is also None, uses `DEFAULT_LIST_LIMIT`.
pub async fn show_status(
    job_id: Option<&str>,
    filters: &[String],
    name_filter: Option<&str>,
    tags: &[(String, String)],
    last: Option<usize>,
    default_limit: Option<usize>,
    archived_filter: Option<bool>,
    ctx: RuntimeCtx,
) -> Result<()> {
    let registry = Registry::open()?;

    if let Some(id) = job_id {
        let job = registry.get_job(id)?;

        // Get current status
        let current_status = if job.remote_host == "local" {
            // Local job - check local status
            let project_path = PathBuf::from(&job.project_path);
            match local::get_local_job_status(&project_path, &job.id) {
                Ok(status) => {
                    registry.update_status(&job.id, status)?;
                    status
                }
                Err(_) => job.status,
            }
        } else if let Some(ref slurm_id) = job.slurm_id {
            // Remote Slurm job - check Slurm status
            let ssh = ctx.ssh(&job.remote_host);
            match get_job_status(&ssh, slurm_id).await {
                Ok(status) => {
                    registry.update_status(&job.id, status)?;
                    status
                }
                Err(_) => job.status,
            }
        } else {
            // Remote direct (exec) job - check via PID/exit_code files
            let ssh = ctx.ssh(&job.remote_host);
            let job_dir = job_path_from_workspace(&job.remote_path, &job.id);
            match get_remote_direct_job_status(&ssh, &job_dir).await {
                Ok(status) => {
                    registry.update_status(&job.id, status)?;
                    status
                }
                Err(_) => job.status,
            }
        };

        print_job_details(&job, current_status);
    } else {
        // Refresh status for all pending/running jobs
        refresh_active_job_statuses(&registry, ctx).await?;

        // Parse status filters
        let status_filters: Vec<JobStatus> = filters
            .iter()
            .map(|f| f.parse())
            .collect::<Result<Vec<_>>>()?;

        let limit = last.unwrap_or_else(|| default_limit.unwrap_or(DEFAULT_LIST_LIMIT));

        if archived_filter.is_none() {
            // Default (non-archived) view: fetch global list, filter in Rust,
            // and preserve global indices so they match get_job_by_index().
            let has_filters =
                !status_filters.is_empty() || name_filter.is_some() || !tags.is_empty();
            let fetch_limit = if has_filters {
                limit.saturating_mul(10).max(1000)
            } else {
                limit
            };

            let all_jobs = registry.list_jobs(None, &[], None, None, &[], None, fetch_limit)?;

            let name_re = name_filter
                .map(|p| {
                    let pattern = build_job_filter_pattern(p);
                    Regex::new(&pattern)
                        .map_err(|e| FlecheError::InvalidRegexPattern(format!("--name '{p}': {e}")))
                })
                .transpose()?;

            let (indices, jobs): (Vec<usize>, Vec<JobRecord>) = all_jobs
                .into_iter()
                .enumerate()
                .filter(|(_, job)| {
                    status_filters.is_empty() || status_filters.contains(&job.status)
                })
                .filter(|(_, job)| name_re.as_ref().is_none_or(|re| re.is_match(&job.id)))
                .filter(|(_, job)| {
                    tags.iter()
                        .all(|(k, v)| job.tags.get(k).is_some_and(|tv| tv == v))
                })
                .take(limit)
                .map(|(i, job)| (i + 1, job))
                .unzip();

            if jobs.is_empty() {
                println!("No jobs found. Run `fleche run` to submit a job.");
                return Ok(());
            }

            print_indexed_job_table(&jobs, &indices);
        } else {
            // Archived or "show all" view: indices would not match
            // get_job_by_index(), so omit them.
            let jobs = registry.list_jobs(
                None,
                &status_filters,
                name_filter,
                None,
                tags,
                archived_filter,
                limit,
            )?;

            if jobs.is_empty() {
                println!("No jobs found. Run `fleche run` to submit a job.");
                return Ok(());
            }

            print_job_table(&jobs);
        }
    }

    Ok(())
}

/// Adds or displays a note on a job.
///
/// If `note` is provided, sets or updates the job's note.
/// If `note` is `None`, displays the existing note (if any).
pub fn note_job(job_id: &str, note: Option<&str>) -> Result<()> {
    let registry = Registry::open()?;
    let job = registry.get_job(job_id)?;

    if let Some(note_text) = note {
        registry.set_note(&job.id, Some(note_text))?;
        println!(
            "{} Note set for job {}",
            style("✓").green(),
            style(&job.id).bold()
        );
    } else {
        // Display existing note
        match job.note {
            Some(ref note_text) => {
                println!("{} {}", style("Note:").bold(), note_text);
            }
            None => {
                println!("No note set for job {}.", job.id);
            }
        }
    }

    Ok(())
}

/// Lists all unique tags across jobs.
pub fn list_tags() -> Result<()> {
    let registry = Registry::open()?;
    let tags = registry.list_unique_tags()?;

    if tags.is_empty() {
        println!("No tags found. Use --tag when running jobs to add tags.");
        return Ok(());
    }

    // Group by key
    let mut current_key = String::new();
    for (key, value) in &tags {
        if key != &current_key {
            if !current_key.is_empty() {
                println!();
            }
            println!("{}", style(key).bold());
            current_key.clone_from(key);
        }
        println!("  {value}");
    }

    Ok(())
}

/// Refreshes the status of all pending/running jobs from Slurm or local process status.
pub async fn refresh_active_job_statuses(registry: &Registry, ctx: RuntimeCtx) -> Result<()> {
    let active_jobs = registry.list_active_jobs()?;

    for job in active_jobs {
        if job.remote_host == "local" {
            // Check local job status
            let project_path = PathBuf::from(&job.project_path);
            if let Ok(status) = local::get_local_job_status(&project_path, &job.id) {
                if status != job.status {
                    registry.update_status(&job.id, status)?;
                }
            }
        } else if let Some(ref slurm_id) = job.slurm_id {
            // Check remote Slurm job status
            let ssh = ctx.ssh(&job.remote_host);
            if let Ok(status) = get_job_status(&ssh, slurm_id).await {
                if status != job.status {
                    registry.update_status(&job.id, status)?;
                }
            }
        } else {
            // Remote direct (exec) job - check via PID/exit_code files
            let ssh = ctx.ssh(&job.remote_host);
            let job_dir = job_path_from_workspace(&job.remote_path, &job.id);
            if let Ok(status) = get_remote_direct_job_status(&ssh, &job_dir).await {
                if status != job.status {
                    registry.update_status(&job.id, status)?;
                }
            }
        }
    }

    Ok(())
}

/// Resolves a job ID or gets the most recent job matching criteria.
pub fn resolve_job(
    registry: &Registry,
    job_id: Option<&str>,
    tags: &[(String, String)],
    note_filter: Option<&str>,
) -> Result<JobRecord> {
    if let Some(id) = job_id {
        registry.get_job(id)
    } else {
        registry
            .list_jobs(None, &[], None, note_filter, tags, None, 1)?
            .into_iter()
            .next()
            .ok_or(FlecheError::NoRecentJob)
    }
}
