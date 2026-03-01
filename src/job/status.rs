//! Job status operations - viewing status, notes, and tags.

use crate::error::{FlecheError, Result};
use crate::local;
use crate::registry::{ArchivedFilter, JobRecord, JobStatus, Registry, build_job_filter_pattern};
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

/// Filtering and display options for job listings.
///
/// All fields default to their permissive/empty value, so callers can set only
/// the fields they care about via struct update syntax:
///
/// ```ignore
/// show_status(None, StatusOptions { last: Some(5), ..Default::default() }, ctx).await?;
/// ```
#[derive(Default)]
pub struct StatusOptions<'a> {
    /// Status strings (e.g. `"failed"`, `"running"`) to restrict results.
    pub filters: &'a [String],
    /// Regex pattern matched against job IDs.
    pub name: Option<&'a str>,
    /// Key-value pairs that must all match for a job to be included.
    pub tags: &'a [(String, String)],
    /// Maximum number of jobs to display. Falls back to `default_limit`,
    /// then to [`DEFAULT_LIST_LIMIT`].
    pub last: Option<usize>,
    /// Caller-supplied default (typically from config) used when `last` is `None`.
    pub default_limit: Option<usize>,
    /// Controls visibility of archived jobs. The default
    /// [`ExcludeArchived`](ArchivedFilter::ExcludeArchived) variant displays
    /// numeric indices; other variants omit them.
    pub archived: ArchivedFilter,
}

/// Queries the live status and exit code of a job from its execution environment.
///
/// Dispatches to the appropriate backend (local process, Slurm, or remote exec)
/// based on the job record. Does not update the registry — callers handle that.
async fn query_live_status(job: &JobRecord, ctx: &RuntimeCtx) -> Result<(JobStatus, Option<i32>)> {
    if job.remote_host == "local" {
        let project_path = PathBuf::from(&job.project_path);
        return local::get_local_job_status(&project_path, &job.id);
    }

    let ssh = ctx.ssh(&job.remote_host);
    if let Some(ref slurm_id) = job.slurm_id {
        get_job_status(&ssh, slurm_id).await
    } else {
        let job_dir = job_path_from_workspace(&job.remote_path, &job.id);
        get_remote_direct_job_status(&ssh, &job_dir).await
    }
}

/// Shows the status of a specific job or lists recent jobs.
///
/// When `job_id` is provided, queries the job's live status from its execution
/// environment (local process, Slurm, or remote exec), updates the registry,
/// and prints detailed information. The `opts` fields are ignored.
///
/// When `job_id` is `None`, refreshes all active jobs and lists recent ones
/// filtered according to `opts`.
pub async fn show_status(
    job_id: Option<&str>,
    opts: StatusOptions<'_>,
    ctx: RuntimeCtx,
) -> Result<()> {
    let registry = Registry::open()?;

    if let Some(id) = job_id {
        show_job_detail(&registry, id, &ctx).await?;
    } else {
        refresh_active_job_statuses(&registry, &ctx).await?;
        list_recent_jobs(&registry, &opts)?;
    }

    Ok(())
}

/// Shows detailed status for a single job, refreshing its live status first.
async fn show_job_detail(registry: &Registry, id: &str, ctx: &RuntimeCtx) -> Result<()> {
    let job = registry.get_job(id)?;

    let (current_status, exit_code) = match query_live_status(&job, ctx).await {
        Ok((status, exit_code)) => {
            registry.update_status(&job.id, status, exit_code)?;
            (status, exit_code)
        }
        Err(_) => (job.status, job.exit_code),
    };

    print_job_details(&job, current_status, exit_code);
    Ok(())
}

/// Lists recent jobs with optional filtering.
fn list_recent_jobs(registry: &Registry, opts: &StatusOptions<'_>) -> Result<()> {
    let status_filters: Vec<JobStatus> = opts
        .filters
        .iter()
        .map(|f| f.parse())
        .collect::<Result<Vec<_>>>()?;

    let limit = opts
        .last
        .unwrap_or_else(|| opts.default_limit.unwrap_or(DEFAULT_LIST_LIMIT));

    let jobs = if opts.archived == ArchivedFilter::ExcludeArchived {
        // Default view: filter in Rust to preserve global indices that
        // match get_job_by_index().
        let (indices, jobs) = list_indexed_jobs(registry, opts, &status_filters, limit)?;
        if !jobs.is_empty() {
            print_indexed_job_table(&jobs, &indices);
        }
        jobs
    } else {
        // Archived or "show all" view: indices would not match
        // get_job_by_index(), so omit them.
        let jobs = registry.list_jobs(
            None,
            &status_filters,
            opts.name,
            None,
            opts.tags,
            opts.archived,
            limit,
        )?;
        if !jobs.is_empty() {
            print_job_table(&jobs);
        }
        jobs
    };

    if jobs.is_empty() {
        println!("No jobs found. Run `fleche run` to submit a job.");
    }

    Ok(())
}

/// Fetches and filters non-archived jobs, returning 1-based global indices
/// alongside the matched records.
fn list_indexed_jobs(
    registry: &Registry,
    opts: &StatusOptions<'_>,
    status_filters: &[JobStatus],
    limit: usize,
) -> Result<(Vec<usize>, Vec<JobRecord>)> {
    let has_filters = !status_filters.is_empty() || opts.name.is_some() || !opts.tags.is_empty();
    let fetch_limit = if has_filters {
        limit.saturating_mul(10).max(1000)
    } else {
        limit
    };

    let all_jobs = registry.list_all_jobs(fetch_limit)?;

    let name_re = opts
        .name
        .map(|p| {
            let pattern = build_job_filter_pattern(p);
            Regex::new(&pattern)
                .map_err(|e| FlecheError::InvalidRegexPattern(format!("--name '{p}': {e}")))
        })
        .transpose()?;

    Ok(all_jobs
        .into_iter()
        .enumerate()
        .filter(|(_, job)| {
            (status_filters.is_empty() || status_filters.contains(&job.status))
                && name_re.as_ref().is_none_or(|re| re.is_match(&job.id))
                && opts
                    .tags
                    .iter()
                    .all(|(k, v)| job.tags.get(k).is_some_and(|tv| tv == v))
        })
        .take(limit)
        .map(|(i, job)| (i + 1, job))
        .unzip())
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
pub async fn refresh_active_job_statuses(registry: &Registry, ctx: &RuntimeCtx) -> Result<()> {
    let active_jobs = registry.list_active_jobs()?;

    for job in active_jobs {
        if let Ok((status, exit_code)) = query_live_status(&job, ctx).await {
            if status != job.status {
                registry.update_status(&job.id, status, exit_code)?;
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
            .list_jobs(
                None,
                &[],
                None,
                note_filter,
                tags,
                ArchivedFilter::ExcludeArchived,
                1,
            )?
            .into_iter()
            .next()
            .ok_or(FlecheError::NoRecentJob)
    }
}
