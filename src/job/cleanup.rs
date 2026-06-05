//! Job cleanup operations - cancel and clean.

use crate::error::{FlecheError, Result};
use crate::local;
use crate::output::OutputFormat;
use crate::registry::{JobRecord, JobStatus, LiveStatus, Registry, parse_duration};
use crate::runtime::RuntimeCtx;
use crate::slurm::cancel_job;
use console::style;
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

use super::cancel_remote_direct_job;
use super::job_path_from_workspace;
use super::{CompiledFilters, JobFilters, list_matching};

/// Options for cleaning up jobs.
#[derive(Debug, Default)]
pub struct CleanJobsOptions {
    /// Clean all completed/failed jobs.
    pub all: bool,
    /// Permanently delete instead of archiving.
    pub delete: bool,
    /// Also delete the shared workspace.
    pub clean_workspace: bool,
    /// Target archived jobs.
    pub include_archived: bool,
    /// Restore archived jobs.
    pub unarchive: bool,
    /// Show what would be done without actually doing it.
    pub dry_run: bool,
    /// Skip confirmation prompt.
    pub skip_confirm: bool,
    /// Output format (human-readable or JSON).
    pub format: OutputFormat,
    /// Shared runtime settings.
    pub ctx: RuntimeCtx,
}

/// Options for cancelling jobs.
#[derive(Debug, Default)]
pub struct CancelJobsOptions {
    /// Cancel all active jobs.
    pub all: bool,
    /// Skip confirmation prompt.
    pub skip_confirm: bool,
    /// Show what would be cancelled without actually cancelling.
    pub dry_run: bool,
    /// Output format (human-readable or JSON).
    pub format: OutputFormat,
    /// Shared runtime settings.
    pub ctx: RuntimeCtx,
}

/// Cancels running or pending Slurm jobs.
pub async fn cancel_jobs(
    job_id: Option<&str>,
    filters: JobFilters<'_>,
    opts: CancelJobsOptions,
) -> Result<()> {
    let registry = Registry::open()?;

    // Active jobs matching the filters (fast path when no filters are given).
    let active_matching = || -> Result<Vec<JobRecord>> {
        let jobs = if filters.is_empty() {
            registry.list_active_jobs()?
        } else {
            list_matching(&registry, filters, usize::MAX)?
                .into_iter()
                .filter(|j| matches!(j.status, JobStatus::Pending | JobStatus::Running))
                .collect()
        };
        Ok(jobs)
    };

    let jobs_to_cancel: Vec<JobRecord> = if let Some(id) = job_id {
        let job = registry.get_job(id)?;
        if matches!(
            job.status,
            JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
        ) {
            return Err(FlecheError::CannotCancel(
                id.to_string(),
                job.status.to_string(),
            ));
        }
        vec![job]
    } else if opts.all {
        active_matching()?
    } else {
        // Cancel the most recent active job matching the filters.
        let active = active_matching()?;
        if active.is_empty() {
            let empty: Vec<JobRecord> = Vec::new();
            return opts.format.print(&empty, || {
                println!("No active jobs to cancel.");
                Ok(())
            });
        }
        let mut active = active;
        active.truncate(1);
        active
    };

    if jobs_to_cancel.is_empty() {
        let empty: Vec<JobRecord> = Vec::new();
        return opts.format.print(&empty, || {
            println!("No active jobs to cancel.");
            Ok(())
        });
    }

    // Dry run: show what would be cancelled and exit
    if opts.dry_run {
        return opts.format.print(&jobs_to_cancel, || {
            println!("Would cancel {} job(s):", jobs_to_cancel.len());
            for job in &jobs_to_cancel {
                println!(
                    "  {} ({}) - {}",
                    job.id,
                    style(&job.status).yellow(),
                    job.job_name
                );
            }
            Ok(())
        });
    }

    // Show jobs and confirm if multiple
    if opts.format.is_human() && (jobs_to_cancel.len() > 1 || opts.all) {
        println!("Jobs to cancel:");
        for job in &jobs_to_cancel {
            println!(
                "  {} ({}) - {}",
                job.id,
                style(&job.status).yellow(),
                job.job_name
            );
        }
        println!();

        if !opts.skip_confirm && !confirm("Cancel these jobs?")? {
            println!("Cancelled.");
            return Ok(());
        }
    }

    for job in &jobs_to_cancel {
        if job.remote_host == "local" {
            // Cancel local job
            let project_path = PathBuf::from(&job.project_path);
            match local::cancel_local_job(&project_path, &job.id) {
                Ok(true) => {
                    registry.update_status(
                        &job.id,
                        &LiveStatus::with_exit_code(JobStatus::Cancelled, 143),
                    )?;
                    if opts.format.is_human() {
                        println!("{} Job {} cancelled", style("✓").green(), job.id);
                    }
                }
                Ok(false) => {
                    eprintln!("  Warning: Could not cancel {} (process not found)", job.id);
                }
                Err(e) => {
                    eprintln!("  Warning: Could not cancel {}: {e}", job.id);
                }
            }
        } else if let Some(ref slurm_id) = job.slurm_id {
            // Cancel remote Slurm job
            let ssh = opts.ctx.ssh(&job.remote_host);
            if let Err(e) = cancel_job(&ssh, slurm_id).await {
                eprintln!("  Warning: Could not cancel {}: {e}", job.id);
                continue;
            }
            registry.update_status(&job.id, &LiveStatus::new(JobStatus::Cancelled))?;
            if opts.format.is_human() {
                println!("{} Job {} cancelled", style("✓").green(), job.id);
            }
        } else {
            // Cancel remote direct (exec) job
            let ssh = opts.ctx.ssh(&job.remote_host);
            let job_dir = job_path_from_workspace(&job.remote_path, &job.id);
            match cancel_remote_direct_job(&ssh, &job_dir).await {
                Ok(true) => {
                    registry.update_status(
                        &job.id,
                        &LiveStatus::with_exit_code(JobStatus::Cancelled, 143),
                    )?;
                    if opts.format.is_human() {
                        println!("{} Job {} cancelled", style("✓").green(), job.id);
                    }
                }
                Ok(false) => {
                    eprintln!("  Warning: Could not cancel {} (process not found)", job.id);
                }
                Err(e) => {
                    eprintln!("  Warning: Could not cancel {}: {e}", job.id);
                }
            }
        }
    }

    opts.format.print_json_only(&jobs_to_cancel)
}

/// Cleans up jobs by removing them from the registry and deleting remote job files.
/// Also supports archiving/unarchiving jobs.
pub async fn clean_jobs(
    job_id: Option<&str>,
    older_than: Option<&str>,
    filters: JobFilters<'_>,
    opts: CleanJobsOptions,
) -> Result<()> {
    let registry = Registry::open()?;

    // Handle --unarchive mode: restore archived jobs
    if opts.unarchive {
        let jobs_to_unarchive: Vec<JobRecord> = if let Some(id) = job_id {
            let job = registry.get_job(id)?;
            if !job.archived {
                println!("Job {} is not archived.", job.id);
                return Ok(());
            }
            vec![job]
        } else if opts.all {
            registry.list_archived_jobs()?
        } else {
            println!("Specify a job ID or --all with --unarchive");
            return Ok(());
        };

        if jobs_to_unarchive.is_empty() {
            let empty: Vec<JobRecord> = Vec::new();
            return opts.format.print(&empty, || {
                println!("No archived jobs to restore.");
                Ok(())
            });
        }

        if opts.dry_run {
            return opts.format.print(&jobs_to_unarchive, || {
                println!(
                    "Would restore {} job(s) from archive:",
                    jobs_to_unarchive.len()
                );
                for job in &jobs_to_unarchive {
                    println!("  {} - {}", job.id, job.job_name);
                }
                Ok(())
            });
        }

        for job in &jobs_to_unarchive {
            registry.unarchive_job(&job.id)?;
            if opts.format.is_human() {
                println!(
                    "{} Restored job {} from archive",
                    style("✓").green(),
                    job.id
                );
            }
        }

        return opts.format.print_json_only(&jobs_to_unarchive);
    }

    // For archive/delete: gather the candidate set per mode, then apply the
    // unified filters (status, name, note, tags) uniformly below.
    let jobs_to_clean: Vec<JobRecord> = if let Some(id) = job_id {
        vec![registry.get_job(id)?]
    } else if opts.all {
        if opts.include_archived {
            registry.list_archived_jobs()?
        } else {
            registry.list_finished_jobs()?
        }
    } else if let Some(duration_str) = older_than {
        let duration = parse_duration(duration_str)?;
        if opts.include_archived {
            registry.list_archived_jobs_older_than(duration)?
        } else {
            registry.list_jobs_older_than(duration)?
        }
    } else {
        println!("Specify a job ID, --all, or --older-than");
        return Ok(());
    };

    // Apply the unified --filter predicates (status/name/note/tags).
    let matcher = CompiledFilters::compile(filters)?;
    let jobs_to_clean: Vec<JobRecord> = jobs_to_clean
        .into_iter()
        .filter(|j| matcher.matches(j))
        .collect();

    let action = if opts.delete { "delete" } else { "archive" };

    if jobs_to_clean.is_empty() && !opts.clean_workspace {
        let empty: Vec<JobRecord> = Vec::new();
        return opts.format.print(&empty, || {
            println!("No jobs to {action}.");
            Ok(())
        });
    }

    // Dry run: show what would be affected and exit
    if opts.dry_run {
        return opts.format.print(&jobs_to_clean, || {
            println!("Would {action} {} job(s):", jobs_to_clean.len());
            for job in &jobs_to_clean {
                println!(
                    "  {} ({}) - {}",
                    job.id,
                    style(&job.status).cyan(),
                    job.job_name
                );
            }
            Ok(())
        });
    }

    // Show jobs and confirm if multiple
    if opts.format.is_human()
        && !jobs_to_clean.is_empty()
        && (jobs_to_clean.len() > 1 || opts.all || older_than.is_some())
    {
        println!("Jobs to {action}:");
        for job in &jobs_to_clean {
            println!(
                "  {} ({}) - {}",
                job.id,
                style(&job.status).cyan(),
                job.job_name
            );
        }
        println!();
    }

    // Default mode: archive jobs
    if !opts.delete {
        let confirm_msg = format!("Archive {} job(s)?", jobs_to_clean.len());
        if opts.format.is_human() && !opts.skip_confirm && !confirm(&confirm_msg)? {
            println!("Cancelled.");
            return Ok(());
        }

        for job in &jobs_to_clean {
            registry.archive_job(&job.id)?;
            if opts.format.is_human() {
                println!("{} Archived job {}", style("✓").green(), job.id);
            }
        }

        return opts.format.print_json_only(&jobs_to_clean);
    }

    // --delete mode: permanently remove jobs
    if opts.format.is_human() && opts.clean_workspace {
        println!(
            "{}",
            style("WARNING: This will also delete the shared workspace!")
                .red()
                .bold()
        );
    }

    if opts.format.is_human() && !opts.skip_confirm && !confirm("Permanently delete?")? {
        println!("Cancelled.");
        return Ok(());
    }

    // Snapshot active jobs before deletions so workspace checks remain accurate
    // even if selected running jobs are removed from the registry.
    let active_jobs_snapshot = if opts.clean_workspace {
        Some(registry.list_active_jobs()?)
    } else {
        None
    };

    // Delete job directories
    for job in &jobs_to_clean {
        if opts.format.is_human() {
            print!("Deleting {}... ", job.id);
        }

        if job.remote_host == "local" {
            let project_path = PathBuf::from(&job.project_path);
            if let Err(e) = local::clean_local_job(&project_path, &job.id) {
                eprintln!("warning: could not delete job directory: {e}");
            }
        } else {
            let ssh = opts.ctx.ssh(&job.remote_host);
            let job_dir = job_path_from_workspace(&job.remote_path, &job.id);
            if let Err(e) = ssh.rm_rf(&job_dir).await {
                eprintln!("warning: could not delete job directory: {e}");
            }
        }

        registry.delete_job(&job.id)?;
        if opts.format.is_human() {
            println!("{}", style("done").green());
        }
    }

    // Clean workspaces if requested (only for remote jobs)
    if opts.clean_workspace {
        let active_jobs = active_jobs_snapshot
            .as_ref()
            .expect("snapshot exists when clean_workspace is enabled");
        let mut seen = HashSet::new();
        let mut cleaned_any = false;

        for job in &jobs_to_clean {
            if job.remote_host == "local" {
                continue;
            }

            let key = (job.remote_host.clone(), job.remote_path.clone());
            if !seen.insert(key.clone()) {
                continue;
            }

            let has_active = active_jobs
                .iter()
                .any(|j| j.remote_host == key.0 && j.remote_path == key.1);
            if has_active {
                eprintln!(
                    "{}",
                    style(format!(
                        "warning: skipping workspace '{}' on '{}' because it has active jobs",
                        key.1, key.0
                    ))
                    .yellow()
                );
                continue;
            }

            let ssh = opts.ctx.ssh(&key.0);
            print!("Deleting workspace on {}... ", key.0);
            if let Err(e) = ssh.rm_rf(&key.1).await {
                eprintln!("warning: could not delete workspace: {e}");
            } else {
                println!("{}", style("done").green());
                cleaned_any = true;
            }
        }

        if !cleaned_any && jobs_to_clean.iter().all(|j| j.remote_host == "local") {
            println!(
                "{}",
                style("Note: --workspace has no effect for local jobs.").yellow()
            );
        }
    }

    opts.format.print(&jobs_to_clean, || {
        if !jobs_to_clean.is_empty() {
            println!(
                "\n{} Deleted {} job(s)",
                style("✓").green(),
                jobs_to_clean.len()
            );
        }
        Ok(())
    })
}

/// Prompts the user for confirmation.
pub fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/N] ");
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}
