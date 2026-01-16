//! Job operations for running, monitoring, and managing remote jobs.
//!
//! This module contains the core business logic for fleche, including:
//! - Running jobs (syncing files, submitting to Slurm, streaming output)
//! - Executing commands directly via SSH
//! - Querying job status
//! - Viewing logs
//! - Downloading outputs back to local
//! - Listing, cancelling, and cleaning up jobs

use crate::config::{Config, ResolvedJob, SlurmConfig};
use crate::error::{FlecheError, Result};
use crate::registry::{JobRecord, JobStatus, Registry, parse_duration};
use crate::slurm::{cancel_job, generate_sbatch_script, get_job_status, submit_job};
use crate::ssh::SshClient;
use crate::sync::{
    download_outputs as sync_download_outputs, download_path as sync_download_path,
    sync_inputs_to_workspace, sync_project_to_workspace,
};
use chrono::Utc;
use console::style;
use rand::Rng;
use std::io::Write;
use std::time::Duration;

/// Returns the workspace path for a project on the remote host.
fn workspace_path(config: &Config) -> String {
    format!(
        "{}/{}/.fleche/workspace",
        config.remote.base_path, config.project_name
    )
}

/// Returns the jobs directory path for a project on the remote host.
fn jobs_base_path(config: &Config) -> String {
    format!(
        "{}/{}/.fleche/jobs",
        config.remote.base_path, config.project_name
    )
}

/// Returns the path for a specific job's metadata/logs directory.
fn job_path(config: &Config, job_id: &str) -> String {
    format!("{}/{}", jobs_base_path(config), job_id)
}

/// Runs a job on the remote cluster via Slurm.
///
/// This is the main entry point for job submission. It:
/// 1. Resolves the job configuration with all overrides applied
/// 2. Syncs project code to the shared workspace
/// 3. Syncs input files to the workspace
/// 4. Creates a job directory for logs/metadata
/// 5. Uploads the generated sbatch script
/// 6. Submits the job to Slurm
/// 7. Streams the job output (unless --bg is specified)
#[allow(clippy::too_many_arguments)]
pub async fn run_job(
    config: &Config,
    job_or_command: Option<&str>,
    command_override: Option<&str>,
    env_overrides: &[(String, String)],
    tags: &[(String, String)],
    slurm_overrides: SlurmConfig,
    background: bool,
    dry_run: bool,
    debug: bool,
) -> Result<()> {
    // Determine if job_or_command is a job name or a command
    let (job_name, actual_command) = if let Some(joc) = job_or_command {
        if config.jobs.contains_key(joc) {
            // It's a job name
            (Some(joc), command_override)
        } else {
            // It's a command (or unrecognized job name - will be used as command)
            (None, Some(joc))
        }
    } else {
        (None, command_override)
    };

    let job = config.resolve_job(job_name, actual_command, env_overrides, &slurm_overrides)?;
    let job_id = generate_job_id(&job.name);

    let workspace = workspace_path(config);
    let job_dir = job_path(config, &job_id);

    // Generate script - runs in workspace, logs go to job directory
    let script = generate_sbatch_script(&job_id, &job, &workspace, &job_dir);

    if dry_run {
        println!(
            "{}",
            style("[dry-run] Generated sbatch script:").bold().yellow()
        );
        println!();
        println!("{script}");
        return Ok(());
    }

    let ssh = SshClient::new(&config.remote.host, debug);

    // Create directories
    println!(
        "{} Creating remote directories...",
        style("[1/4]").bold().dim()
    );
    ssh.mkdir(&workspace).await?;
    ssh.mkdir(&job_dir).await?;

    // Sync project code to workspace
    print!("{} Syncing project code...", style("[2/4]").bold().dim());
    let _ = std::io::stdout().flush();
    let stats =
        sync_project_to_workspace(&config.project_path, &config.remote.host, &workspace).await?;
    println!(" {}", style(format!("({})", stats.human_readable())).dim());

    // Sync inputs to workspace
    if job.inputs.is_empty() {
        println!("{} No input files to sync", style("[3/4]").bold().dim());
    } else {
        print!("{} Syncing input files...", style("[3/4]").bold().dim());
        let _ = std::io::stdout().flush();
        let stats = sync_inputs_to_workspace(
            &config.project_path,
            &job.inputs,
            &config.remote.host,
            &workspace,
        )
        .await?;
        println!(" {}", style(format!("({})", stats.human_readable())).dim());
    }

    // Upload script to job directory
    println!("{} Submitting job to Slurm...", style("[4/4]").bold().dim());
    ssh.write_file(&format!("{job_dir}/job.sbatch"), &script)
        .await?;

    // Submit job
    let slurm_id = submit_job(&ssh, &job_dir).await?;

    // Record in registry
    let registry = Registry::open()?;
    registry.insert_job(
        &job_id,
        Some(&slurm_id),
        &job,
        &config.project_name,
        &config.project_path.to_string_lossy(),
        &config.remote.host,
        &workspace,
        tags,
    )?;

    println!();
    println!("{} {}", style("Job ID:").green().bold(), job_id);
    println!("{} {}", style("Slurm ID:").green().bold(), slurm_id);

    if !background {
        println!();
        let log_path = format!("{job_dir}/job.out");
        follow_job_logs(&config.remote.host, &slurm_id, &log_path, debug).await?;
    }

    Ok(())
}

/// Executes a command directly via SSH (no Slurm).
///
/// Syncs the project and inputs, then runs the command directly over SSH.
/// Useful for quick tests or interactive work.
pub async fn exec_command(
    config: &Config,
    command: &str,
    env_overrides: &[(String, String)],
    debug: bool,
) -> Result<()> {
    let workspace = workspace_path(config);
    let ssh = SshClient::new(&config.remote.host, debug);

    // Create workspace if needed
    println!(
        "{} Creating remote directories...",
        style("[1/3]").bold().dim()
    );
    ssh.mkdir(&workspace).await?;

    // Sync project code to workspace
    print!("{} Syncing project code...", style("[2/3]").bold().dim());
    let _ = std::io::stdout().flush();
    let stats =
        sync_project_to_workspace(&config.project_path, &config.remote.host, &workspace).await?;
    println!(" {}", style(format!("({})", stats.human_readable())).dim());

    // Sync global inputs
    let global_inputs: Vec<String> = config
        .jobs
        .values()
        .flat_map(|j| j.inputs.clone())
        .collect();

    if global_inputs.is_empty() {
        println!("{} Executing command...", style("[3/3]").bold().dim());
    } else {
        print!("{} Syncing input files...", style("[3/3]").bold().dim());
        let _ = std::io::stdout().flush();
        let stats = sync_inputs_to_workspace(
            &config.project_path,
            &global_inputs,
            &config.remote.host,
            &workspace,
        )
        .await?;
        println!(" {}", style(format!("({})", stats.human_readable())).dim());
    }

    // Build environment string
    let env_str = if env_overrides.is_empty() {
        String::new()
    } else {
        let vars: Vec<String> = env_overrides
            .iter()
            .map(|(k, v)| format!("{}={}", k, shell_escape(v)))
            .collect();
        format!("{} ", vars.join(" "))
    };

    // Execute command in workspace
    println!();
    let full_command = format!("cd {} && {}{}", shell_escape(&workspace), env_str, command);
    let (success, stdout, stderr) = ssh.exec_allow_failure(&full_command).await?;

    // Print output
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }

    if !success {
        return Err(FlecheError::Other("Command failed".to_string()));
    }

    Ok(())
}

/// Simple shell escape - wraps in single quotes.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Shows the status of a specific job or lists recent jobs.
pub async fn show_status(job_id: Option<&str>, filter: Option<&str>, debug: bool) -> Result<()> {
    let registry = Registry::open()?;

    if let Some(id) = job_id {
        let job = registry.get_job(id)?;
        let ssh = SshClient::new(&job.remote_host, debug);

        // Get current status from Slurm
        let current_status = if let Some(ref slurm_id) = job.slurm_id {
            match get_job_status(&ssh, slurm_id).await {
                Ok(status) => {
                    registry.update_status(&job.id, status)?;
                    status
                }
                Err(_) => job.status,
            }
        } else {
            job.status
        };

        print_job_details(&job, current_status);
    } else {
        // Refresh status for all pending/running jobs
        refresh_active_job_statuses(&registry, debug).await?;

        // Determine status filter
        let status_filter = if let Some(f) = filter {
            Some(f.parse()?)
        } else {
            None
        };

        let jobs = registry.list_jobs(None, status_filter, &[], 20)?;

        if jobs.is_empty() {
            println!("No jobs found. Run `fleche run` to submit a job.");
            return Ok(());
        }

        print_job_table(&jobs);
    }

    Ok(())
}

/// Displays logs from a job's stdout or stderr.
#[allow(clippy::fn_params_excessive_bools)]
pub async fn show_logs(
    job_id: Option<&str>,
    follow: bool,
    only_stdout: bool,
    only_stderr: bool,
    tail: Option<usize>,
    debug: bool,
) -> Result<()> {
    let registry = Registry::open()?;

    // If no job ID provided, use most recent job
    let job = if let Some(id) = job_id {
        registry.get_job(id)?
    } else {
        registry.get_most_recent_job(None)?.ok_or_else(|| {
            FlecheError::Other("No jobs found. Run `fleche run` to submit a job.".to_string())
        })?
    };

    let ssh = SshClient::new(&job.remote_host, debug);

    // Job logs are in the job directory, not workspace
    // remote_path is the workspace, job logs are in ../jobs/<job_id>/
    let base = job.remote_path.trim_end_matches("/workspace");
    let log_base = format!("{}/jobs/{}", base, job.id);

    // Determine which streams to show
    let show_stdout = !only_stderr || only_stdout;
    let show_stderr = !only_stdout || only_stderr;
    let show_both = show_stdout && show_stderr;

    if follow {
        let log_file = if only_stderr { "job.err" } else { "job.out" };
        let log_path = format!("{log_base}/{log_file}");

        if let Some(ref slurm_id) = job.slurm_id {
            follow_job_logs(&job.remote_host, slurm_id, &log_path, debug).await?;
        } else {
            println!(
                "{}",
                style("Following output (Ctrl+C to disconnect)...").yellow()
            );
            let mut child = ssh.tail_follow(&log_path)?;
            let _ = child.wait().await;
        }
    } else if show_both {
        println!("{}", style("=== STDOUT ===").bold());
        let stdout_path = format!("{log_base}/job.out");
        match ssh.cat_tail(&stdout_path, tail).await {
            Ok(content) => print!("{content}"),
            Err(e) => eprintln!("Error reading stdout: {e}"),
        }

        println!();
        println!("{}", style("=== STDERR ===").bold());
        let stderr_path = format!("{log_base}/job.err");
        match ssh.cat_tail(&stderr_path, tail).await {
            Ok(content) => print!("{content}"),
            Err(e) => eprintln!("Error reading stderr: {e}"),
        }
    } else {
        let log_file = if show_stderr { "job.err" } else { "job.out" };
        let log_path = format!("{log_base}/{log_file}");

        let content = ssh.cat_tail(&log_path, tail).await?;
        print!("{content}");
    }

    Ok(())
}

/// Downloads output files from a job's workspace back to the local project.
pub async fn download_outputs(
    job_id: Option<&str>,
    partial: bool,
    specific_path: Option<&str>,
    debug: bool,
) -> Result<()> {
    let registry = Registry::open()?;

    // If no job ID provided, use most recent job
    let job = if let Some(id) = job_id {
        registry.get_job(id)?
    } else {
        registry.get_most_recent_job(None)?.ok_or_else(|| {
            FlecheError::Other("No jobs found. Run `fleche run` to submit a job.".to_string())
        })?
    };

    // Check job status
    if !partial && matches!(job.status, JobStatus::Pending | JobStatus::Running) {
        if let Some(ref slurm_id) = job.slurm_id {
            let ssh = SshClient::new(&job.remote_host, debug);
            let current_status = get_job_status(&ssh, slurm_id).await.unwrap_or(job.status);
            if matches!(current_status, JobStatus::Pending | JobStatus::Running) {
                eprintln!(
                    "{}",
                    style("Warning: Job is still running. Use --partial to download anyway.")
                        .yellow()
                );
                return Ok(());
            }
        }
    }

    let local_path = std::path::PathBuf::from(&job.project_path);

    if let Some(path) = specific_path {
        println!("Downloading {path} from workspace...");
        sync_download_path(&job.remote_host, &job.remote_path, path, &local_path).await?;
    } else {
        // Parse config to get outputs
        let resolved: ResolvedJob = serde_json::from_str(&job.config_json)?;

        if resolved.outputs.is_empty() {
            println!("No outputs defined for this job.");
            return Ok(());
        }

        println!("Downloading outputs from workspace...");
        for output in &resolved.outputs {
            println!("  {output}");
        }
        sync_download_outputs(
            &job.remote_host,
            &job.remote_path,
            &resolved.outputs,
            &local_path,
        )
        .await?;
    }

    registry.set_outputs_synced(&job.id)?;
    println!("{}", style("Download complete.").green());

    Ok(())
}

/// Refreshes the status of all pending/running jobs from Slurm.
async fn refresh_active_job_statuses(registry: &Registry, debug: bool) -> Result<()> {
    let active_jobs = registry.list_active_jobs()?;

    for job in active_jobs {
        if let Some(ref slurm_id) = job.slurm_id {
            let ssh = SshClient::new(&job.remote_host, debug);
            if let Ok(status) = get_job_status(&ssh, slurm_id).await {
                if status != job.status {
                    registry.update_status(&job.id, status)?;
                }
            }
        }
    }

    Ok(())
}

/// Cancels running or pending Slurm jobs.
pub async fn cancel_jobs(
    job_id: Option<&str>,
    all: bool,
    skip_confirm: bool,
    debug: bool,
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
        registry.list_active_jobs()?
    } else {
        // Cancel most recent running job
        let active = registry.list_active_jobs()?;
        if active.is_empty() {
            println!("No active jobs to cancel.");
            return Ok(());
        }
        vec![active.into_iter().next().unwrap()]
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
        let Some(ref slurm_id) = job.slurm_id else {
            eprintln!("  Warning: Job {} has no Slurm ID, skipping", job.id);
            continue;
        };

        let ssh = SshClient::new(&job.remote_host, debug);
        if let Err(e) = cancel_job(&ssh, slurm_id).await {
            eprintln!("  Warning: Could not cancel {}: {e}", job.id);
            continue;
        }
        registry.update_status(&job.id, JobStatus::Cancelled)?;
        println!("{} Job {} cancelled", style("✓").green(), job.id);
    }

    Ok(())
}

/// Prompts the user for confirmation.
fn confirm(prompt: &str) -> Result<bool> {
    use std::io::{self, Write};

    print!("{prompt} [y/N] ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Cleans up jobs by removing them from the registry and deleting remote job files.
#[allow(clippy::fn_params_excessive_bools)]
pub async fn clean_jobs(
    job_id: Option<&str>,
    all: bool,
    older_than: Option<&str>,
    clean_workspace: bool,
    skip_confirm: bool,
    debug: bool,
) -> Result<()> {
    let registry = Registry::open()?;

    let jobs_to_clean: Vec<JobRecord> = if let Some(id) = job_id {
        vec![registry.get_job(id)?]
    } else if all {
        registry.list_finished_jobs()?
    } else if let Some(duration_str) = older_than {
        let duration = parse_duration(duration_str)?;
        registry.list_jobs_older_than(duration)?
    } else {
        println!("Specify a job ID, --all, or --older-than");
        return Ok(());
    };

    if jobs_to_clean.is_empty() && !clean_workspace {
        println!("No jobs to clean.");
        return Ok(());
    }

    // Show jobs and confirm
    if !jobs_to_clean.is_empty() && (jobs_to_clean.len() > 1 || all || older_than.is_some()) {
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

    if clean_workspace {
        println!(
            "{}",
            style("WARNING: This will also delete the shared workspace!")
                .red()
                .bold()
        );
    }

    if !skip_confirm && !confirm("Proceed with cleanup?")? {
        println!("Cancelled.");
        return Ok(());
    }

    // Clean job directories
    for job in &jobs_to_clean {
        print!("Cleaning {}... ", job.id);

        // Delete job directory (logs/metadata only, not workspace)
        let ssh = SshClient::new(&job.remote_host, debug);
        let job_dir = format!(
            "{}/.fleche/jobs/{}",
            job.remote_path
                .trim_end_matches("/workspace")
                .trim_end_matches("/.fleche/workspace"),
            job.id
        );
        if let Err(e) = ssh.rm_rf(&job_dir).await {
            eprintln!("warning: could not delete job directory: {e}");
        }

        registry.delete_job(&job.id)?;
        println!("{}", style("done").green());
    }

    // Clean workspace if requested
    if clean_workspace {
        if let Some(job) = jobs_to_clean.first() {
            let ssh = SshClient::new(&job.remote_host, debug);
            print!("Cleaning workspace... ");
            if let Err(e) = ssh.rm_rf(&job.remote_path).await {
                eprintln!("warning: could not delete workspace: {e}");
            } else {
                println!("{}", style("done").green());
            }
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

/// Generates a unique job ID from the job name and current timestamp.
fn generate_job_id(job_name: &str) -> String {
    let now = Utc::now();
    let suffix: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(4)
        .map(char::from)
        .collect::<String>()
        .to_lowercase();
    format!(
        "{}-{}-{}",
        job_name,
        now.format("%Y%m%d-%H%M%S-%3f"),
        suffix
    )
}

/// Follows job logs and automatically exits when the job finishes.
async fn follow_job_logs(host: &str, slurm_id: &str, log_path: &str, debug: bool) -> Result<()> {
    println!(
        "{}",
        style("Streaming output (Ctrl+C to disconnect, job keeps running)...").yellow()
    );

    let ssh = SshClient::new(host, debug);
    let mut child = ssh.tail_follow(log_path)?;

    // Poll job status until it reaches a terminal state
    let slurm_id = slurm_id.to_string();
    let host = host.to_string();
    let status_check = async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let ssh = SshClient::new(&host, debug);
            if let Ok(status) = get_job_status(&ssh, &slurm_id).await {
                match status {
                    JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => {
                        return status;
                    }
                    _ => {}
                }
            }
        }
    };

    // Wait for either the tail process to exit or the job to finish
    tokio::select! {
        _ = child.wait() => {
            // Tail exited on its own (unlikely unless error)
        }
        status = status_check => {
            // Job finished, kill tail and print status
            let _ = child.kill().await;

            // Give a moment for any final output to flush
            tokio::time::sleep(Duration::from_millis(500)).await;

            println!();
            let message = match status {
                JobStatus::Completed => "Job completed successfully.",
                JobStatus::Failed => "Job failed.",
                JobStatus::Cancelled => "Job cancelled.",
                _ => "Job finished.",
            };

            match status {
                JobStatus::Completed => {
                    println!("{}", style(message).green().bold());
                }
                JobStatus::Failed => {
                    println!("{}", style(message).red().bold());
                }
                JobStatus::Cancelled => {
                    println!("{}", style(message).yellow().bold());
                }
                _ => {}
            }

            // Send terminal notification (OSC 9)
            print!("\x1b]9;{message}\x07");
        }
    }

    Ok(())
}

/// Prints detailed information about a single job.
fn print_job_details(job: &JobRecord, status: JobStatus) {
    println!("{}", style("Job Details").bold().underlined());
    println!();
    println!("  {:<14} {}", style("ID:").bold(), job.id);
    println!(
        "  {:<14} {}",
        style("Slurm ID:").bold(),
        job.slurm_id.as_deref().unwrap_or("-")
    );
    println!("  {:<14} {}", style("Job Name:").bold(), job.job_name);
    println!("  {:<14} {}", style("Project:").bold(), job.project_name);
    println!(
        "  {:<14} {}",
        style("Status:").bold(),
        format_status(status)
    );
    println!("  {:<14} {}", style("Remote Host:").bold(), job.remote_host);
    println!("  {:<14} {}", style("Workspace:").bold(), job.remote_path);
    println!(
        "  {:<14} {}",
        style("Created:").bold(),
        job.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    );

    if !job.tags.is_empty() {
        println!();
        println!("  {}", style("Tags:").bold());
        for (key, value) in &job.tags {
            println!("    {key}={value}");
        }
    }

    println!();
    println!("  {}", style("Command:").bold());
    for line in job.command.lines() {
        println!("    {line}");
    }
}

/// Prints a table of jobs.
fn print_job_table(jobs: &[JobRecord]) {
    println!(
        "{:<45} {:<12} {:<12} {:<20}",
        style("ID").bold().underlined(),
        style("STATUS").bold().underlined(),
        style("SLURM ID").bold().underlined(),
        style("CREATED").bold().underlined(),
    );

    for job in jobs {
        println!(
            "{:<45} {} {:<12} {:<20}",
            truncate(&job.id, 44),
            format_status(job.status),
            job.slurm_id.as_deref().unwrap_or("-"),
            job.created_at.format("%Y-%m-%d %H:%M"),
        );
        if !job.tags.is_empty() {
            let tags: Vec<String> = job.tags.iter().map(|(k, v)| format!("{k}={v}")).collect();
            println!("    {}", style(tags.join(" ")).dim());
        }
    }
}

/// Formats a job status with appropriate colors and fixed width.
fn format_status(status: JobStatus) -> String {
    match status {
        JobStatus::Pending => style(format!("{:<12}", "pending")).yellow().to_string(),
        JobStatus::Running => style(format!("{:<12}", "running")).blue().to_string(),
        JobStatus::Completed => style(format!("{:<12}", "completed")).green().to_string(),
        JobStatus::Failed => style(format!("{:<12}", "failed")).red().to_string(),
        JobStatus::Cancelled => style(format!("{:<12}", "cancelled")).dim().to_string(),
    }
}

/// Truncates a string to a maximum length, adding "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
