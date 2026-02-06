//! Job cleanup operations - cancel and clean.

use crate::error::{FlecheError, Result};
use crate::local;
use crate::registry::{JobRecord, JobStatus, Registry, parse_duration};
use crate::runtime::{SshTimeouts, ssh_client};
use crate::slurm::cancel_job;
use console::style;
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

use super::job_path_from_workspace;

/// Options for cleaning up jobs.
#[derive(Debug, Default)]
pub struct CleanJobsOptions {
    /// Clean all completed/failed jobs.
    pub all: bool,
    /// Also delete the shared workspace.
    pub clean_workspace: bool,
    /// Archive jobs instead of deleting.
    pub archive: bool,
    /// Restore archived jobs.
    pub unarchive: bool,
    /// Skip confirmation prompt.
    pub skip_confirm: bool,
    /// Enable verbose SSH output.
    pub debug: bool,
    /// Optional SSH timeout settings.
    pub ssh_timeouts: Option<SshTimeouts>,
}

/// Cancels running or pending Slurm jobs.
pub async fn cancel_jobs(
    job_id: Option<&str>,
    all: bool,
    skip_confirm: bool,
    tags: &[(String, String)],
    debug: bool,
    ssh_timeouts: Option<SshTimeouts>,
) -> Result<()> {
    let registry = Registry::open()?;

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
    } else if all {
        // Get active jobs, optionally filtered by tags
        if tags.is_empty() {
            registry.list_active_jobs()?
        } else {
            registry
                .list_jobs(None, &[], None, None, tags, None, usize::MAX)?
                .into_iter()
                .filter(|j| matches!(j.status, JobStatus::Pending | JobStatus::Running))
                .collect()
        }
    } else {
        // Cancel most recent active job (optionally filtered by tags)
        let active: Vec<JobRecord> = if tags.is_empty() {
            registry.list_active_jobs()?
        } else {
            registry
                .list_jobs(None, &[], None, None, tags, None, usize::MAX)?
                .into_iter()
                .filter(|j| matches!(j.status, JobStatus::Pending | JobStatus::Running))
                .collect()
        };
        if active.is_empty() {
            println!("No active jobs to cancel.");
            return Ok(());
        }
        let mut active = active;
        active.truncate(1);
        active
    };

    if jobs_to_cancel.is_empty() {
        println!("No active jobs to cancel.");
        return Ok(());
    }

    // Show jobs and confirm if multiple
    if jobs_to_cancel.len() > 1 || all {
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

        if !skip_confirm && !confirm("Cancel these jobs?")? {
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
                    registry.update_status(&job.id, JobStatus::Cancelled)?;
                    println!("{} Job {} cancelled", style("✓").green(), job.id);
                }
                Ok(false) => {
                    eprintln!("  Warning: Could not cancel {} (process not found)", job.id);
                }
                Err(e) => {
                    eprintln!("  Warning: Could not cancel {}: {e}", job.id);
                }
            }
        } else {
            // Cancel remote Slurm job
            let Some(ref slurm_id) = job.slurm_id else {
                eprintln!("  Warning: Job {} has no Slurm ID, skipping", job.id);
                continue;
            };

            let ssh = ssh_client(&job.remote_host, debug, ssh_timeouts);
            if let Err(e) = cancel_job(&ssh, slurm_id).await {
                eprintln!("  Warning: Could not cancel {}: {e}", job.id);
                continue;
            }
            registry.update_status(&job.id, JobStatus::Cancelled)?;
            println!("{} Job {} cancelled", style("✓").green(), job.id);
        }
    }

    Ok(())
}

/// Cleans up jobs by removing them from the registry and deleting remote job files.
/// Also supports archiving/unarchiving jobs.
pub async fn clean_jobs(
    job_id: Option<&str>,
    older_than: Option<&str>,
    tags: &[(String, String)],
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
            println!("No archived jobs to restore.");
            return Ok(());
        }

        for job in &jobs_to_unarchive {
            registry.unarchive_job(&job.id)?;
            println!(
                "{} Restored job {} from archive",
                style("✓").green(),
                job.id
            );
        }

        return Ok(());
    }

    // For archive/clean: get jobs to process
    let jobs_to_clean: Vec<JobRecord> = if let Some(id) = job_id {
        vec![registry.get_job(id)?]
    } else if opts.all {
        // Get finished jobs, optionally filtered by tags
        if tags.is_empty() {
            registry.list_finished_jobs()?
        } else {
            registry
                .list_jobs(None, &[], None, None, tags, None, usize::MAX)?
                .into_iter()
                .filter(|j| {
                    matches!(
                        j.status,
                        JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
                    )
                })
                .collect()
        }
    } else if let Some(duration_str) = older_than {
        let duration = parse_duration(duration_str)?;
        let older_jobs = registry.list_jobs_older_than(duration)?;
        // Filter by tags if provided
        if tags.is_empty() {
            older_jobs
        } else {
            older_jobs
                .into_iter()
                .filter(|j| tags.iter().all(|(k, v)| j.tags.get(k) == Some(v)))
                .collect()
        }
    } else {
        println!("Specify a job ID, --all, or --older-than");
        return Ok(());
    };

    if jobs_to_clean.is_empty() && !opts.clean_workspace {
        println!(
            "No jobs to {}.",
            if opts.archive { "archive" } else { "clean" }
        );
        return Ok(());
    }

    // Handle --archive mode: archive jobs instead of deleting
    if opts.archive {
        if !jobs_to_clean.is_empty()
            && (jobs_to_clean.len() > 1 || opts.all || older_than.is_some())
        {
            println!("Jobs to archive:");
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

        if !opts.skip_confirm && !confirm("Archive these jobs?")? {
            println!("Cancelled.");
            return Ok(());
        }

        for job in &jobs_to_clean {
            registry.archive_job(&job.id)?;
            println!("{} Archived job {}", style("✓").green(), job.id);
        }

        return Ok(());
    }

    // Normal clean mode: delete jobs
    // Show jobs and confirm
    if !jobs_to_clean.is_empty() && (jobs_to_clean.len() > 1 || opts.all || older_than.is_some()) {
        println!("Jobs to clean:");
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

    if opts.clean_workspace {
        println!(
            "{}",
            style("WARNING: This will also delete the shared workspace!")
                .red()
                .bold()
        );
    }

    if !opts.skip_confirm && !confirm("Proceed with cleanup?")? {
        println!("Cancelled.");
        return Ok(());
    }

    // Clean job directories
    for job in &jobs_to_clean {
        print!("Cleaning {}... ", job.id);

        if job.remote_host == "local" {
            // Clean local job directory
            let project_path = PathBuf::from(&job.project_path);
            if let Err(e) = local::clean_local_job(&project_path, &job.id) {
                eprintln!("warning: could not delete job directory: {e}");
            }
        } else {
            // Clean remote job directory (logs/metadata only, not workspace)
            let ssh = ssh_client(&job.remote_host, opts.debug, opts.ssh_timeouts);
            let job_dir = job_path_from_workspace(&job.remote_path, &job.id);
            if let Err(e) = ssh.rm_rf(&job_dir).await {
                eprintln!("warning: could not delete job directory: {e}");
            }
        }

        registry.delete_job(&job.id)?;
        println!("{}", style("done").green());
    }

    // Clean workspaces if requested (only for remote jobs)
    if opts.clean_workspace {
        let active_jobs = registry.list_active_jobs()?;
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

            let ssh = ssh_client(&key.0, opts.debug, opts.ssh_timeouts);
            print!("Cleaning workspace on {}... ", key.0);
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

    if !jobs_to_clean.is_empty() {
        println!(
            "\n{} Cleaned {} job(s)",
            style("✓").green(),
            jobs_to_clean.len()
        );
    }

    Ok(())
}

/// Prompts the user for confirmation.
pub fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/N] ");
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}
