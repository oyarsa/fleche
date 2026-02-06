//! Job execution operations - running and re-running jobs on remote clusters.

use crate::config::{Config, ResolvedJob, SlurmConfig};
use crate::error::{FlecheError, Result};
use crate::local;
use crate::registry::{JobStatus, Registry};
use crate::runtime::{RuntimeCtx, send_notification};
use crate::slurm::{generate_sbatch_script, get_job_status, submit_job};
use crate::ssh::shell_escape;
use crate::sync::{sync_inputs_to_workspace, sync_project_to_workspace};
use chrono::Utc;
use console::style;
use rand::Rng;
use std::io::Write;
use std::time::Duration;

use super::{job_path, workspace_path};

/// Checks that the system shell is available for local command execution.
///
/// On Unix, checks for `sh`. On Windows, `cmd.exe` is always available.
fn require_shell() -> Result<()> {
    #[cfg(unix)]
    if std::process::Command::new("sh")
        .arg("-c")
        .arg("true")
        .output()
        .is_err()
    {
        return Err(FlecheError::MissingDependency(
            "sh not found. Local execution requires a Unix shell.\n  \
             Windows: Install Git Bash, WSL, or Cygwin"
                .to_string(),
        ));
    }
    Ok(())
}

/// Options for running a job.
#[derive(Debug, Default)]
pub struct RunJobOptions {
    /// Run in background (don't stream output).
    pub background: bool,
    /// Send terminal notification when job completes.
    pub notify: bool,
    /// Print generated sbatch script without submitting.
    pub dry_run: bool,
    /// Job ID to wait for before starting.
    pub after: Option<String>,
    /// Number of times to retry on failure (with exponential backoff).
    pub retry: Option<u32>,
    /// Note/annotation to attach to the job.
    pub note: Option<String>,
}

/// Runs a job on the remote cluster via Slurm (or locally if host is "local").
///
/// This is the main entry point for job submission. It:
/// 1. Resolves the job configuration with all overrides applied
/// 2. Syncs project code to the shared workspace (remote only)
/// 3. Syncs input files to the workspace (remote only)
/// 4. Creates a job directory for logs/metadata
/// 5. Uploads the generated sbatch script (remote only)
/// 6. Submits the job to Slurm (or runs locally)
/// 7. Streams the job output (unless --bg is specified)
pub async fn run_job(
    config: &Config,
    job_or_command: Option<&str>,
    command_override: Option<&str>,
    env_overrides: &[(String, String)],
    tags: &[(String, String)],
    slurm_overrides: SlurmConfig,
    host_override: Option<&str>,
    opts: RunJobOptions,
    ctx: RuntimeCtx,
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

    // Determine final host: CLI override -> job definition -> remote.host
    let host = host_override.map_or_else(|| job.host.clone(), String::from);

    // Branch based on host
    if host == "local" {
        return run_job_locally(config, &job, tags, &opts, ctx).await;
    }

    // Resolve dependency if specified
    let dependency_slurm_id = if let Some(ref dep_job_id) = opts.after {
        let registry = Registry::open()?;
        let dep_job = registry.get_job(dep_job_id)?;
        let slurm_id = dep_job
            .slurm_id
            .ok_or_else(|| FlecheError::NoSlurmId(dep_job.id.clone()))?;
        Some(slurm_id)
    } else {
        None
    };

    // Remote execution path
    let job_id = generate_job_id(&job.name);
    let workspace = workspace_path(config);

    if opts.dry_run {
        let job_dir = job_path(config, &job_id);
        let script = generate_sbatch_script(&job_id, &job, &workspace, &job_dir);
        println!(
            "{}",
            style("[dry-run] Generated sbatch script:").bold().yellow()
        );
        println!();
        println!("{script}");
        return Ok(());
    }

    let job_dir = job_path(config, &job_id);
    let ssh =
        prepare_remote_workspace(config, &host, &workspace, &job_dir, &job.inputs, ctx).await?;

    // Retry loop
    let max_attempts = opts.retry.map_or(1, |r| r + 1);
    let mut attempt = 0;

    loop {
        attempt += 1;

        // Generate new job ID for each attempt
        let job_id = if attempt == 1 {
            job_id.clone()
        } else {
            generate_job_id(&job.name)
        };
        let job_dir = job_path(config, &job_id);

        // Create job directory for this attempt
        if attempt > 1 {
            ssh.mkdir(&job_dir).await?;
        }

        // Generate and upload script
        let script = generate_sbatch_script(&job_id, &job, &workspace, &job_dir);

        println!("{} Submitting job to Slurm...", style("[4/4]").bold().dim());
        ssh.write_file(&format!("{job_dir}/job.sbatch"), &script)
            .await?;

        // Submit job (with optional dependency, only on first attempt)
        let dep = if attempt == 1 {
            dependency_slurm_id.as_deref()
        } else {
            None
        };
        let slurm_id = submit_job(&ssh, &job_dir, dep).await?;

        // Record in registry (note only on first attempt)
        let registry = Registry::open()?;
        let job_note = if attempt == 1 {
            opts.note.as_deref()
        } else {
            None
        };
        registry.insert_job(
            &job_id,
            Some(&slurm_id),
            &job,
            &config.project_name,
            &config.project_path.to_string_lossy(),
            &host,
            &workspace,
            tags,
            job_note,
        )?;

        println!();
        if attempt > 1 {
            println!(
                "{} {} (attempt {}/{})",
                style("Job ID:").green().bold(),
                job_id,
                attempt,
                max_attempts
            );
        } else {
            println!("{} {}", style("Job ID:").green().bold(), job_id);
        }
        println!("{} {}", style("Slurm ID:").green().bold(), slurm_id);

        if opts.background {
            if ctx.should_notify(opts.notify) {
                // Wait for job completion in background and notify
                wait_and_notify(&job_id, &host, ctx).await?;
            }
            // Background mode doesn't support retry (we don't wait for completion)
            break;
        }

        // Foreground mode: follow logs and check result
        println!();
        let log_path = format!("{job_dir}/job.out");
        let final_status = follow_job_logs(&host, &slurm_id, &log_path, ctx).await?;

        // Update registry with final status
        registry.update_status(&job_id, final_status)?;

        // Check if we should retry
        if final_status == JobStatus::Failed && attempt < max_attempts {
            let delay_secs = ctx.retry_base_delay_secs * (1 << (attempt - 1));
            println!();
            println!(
                "{} Retrying in {} seconds (attempt {}/{})...",
                style("↻").yellow().bold(),
                delay_secs,
                attempt + 1,
                max_attempts
            );
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;
            println!();
        } else {
            break;
        }
    }

    Ok(())
}

/// Runs a job locally (when host is "local").
async fn run_job_locally(
    config: &Config,
    job: &ResolvedJob,
    tags: &[(String, String)],
    opts: &RunJobOptions,
    ctx: RuntimeCtx,
) -> Result<()> {
    require_shell()?;

    // Check dependency if specified
    if let Some(ref dep_job_id) = opts.after {
        let registry = Registry::open()?;
        let dep_job = registry.get_job(dep_job_id)?;

        if dep_job.status != JobStatus::Completed {
            return Err(FlecheError::MissingDependency(format!(
                "Dependency job '{}' has not completed successfully (status: {:?}). \
                 Use 'fleche wait {}' to wait for it.",
                dep_job.id, dep_job.status, dep_job.id
            )));
        }
    }

    let job_id = generate_job_id(&job.name);

    // Warn about features that don't apply locally
    if !job.inputs.is_empty() {
        eprintln!(
            "{}",
            style("Warning: inputs are ignored for local jobs (files are already local)").yellow()
        );
    }
    if !job.outputs.is_empty() {
        eprintln!(
            "{}",
            style("Warning: outputs are ignored for local jobs (files are already local)").yellow()
        );
    }
    if job.slurm.partition.is_some()
        || job.slurm.time.is_some()
        || job.slurm.gpus.is_some()
        || job.slurm.cpus.is_some()
        || job.slurm.memory.is_some()
    {
        eprintln!(
            "{}",
            style("Warning: Slurm options are ignored for local jobs").yellow()
        );
    }

    if opts.dry_run {
        println!("{}", style("[dry-run] Would run locally:").bold().yellow());
        println!();
        println!("  Command: {}", job.command);
        println!("  Working directory: {}", config.project_path.display());
        if !job.env.is_empty() {
            println!("  Environment:");
            for (k, v) in &job.env {
                println!("    {k}={v}");
            }
        }
        return Ok(());
    }

    // Create local job directory
    let job_dir = local::ensure_job_dir(&config.project_path, &job_id)?;

    // Record in registry (with remote_host="local" and remote_path=project_path)
    let registry = Registry::open()?;
    registry.insert_job(
        &job_id,
        None, // No Slurm ID for local jobs
        job,
        &config.project_name,
        &config.project_path.to_string_lossy(),
        "local",
        &config.project_path.to_string_lossy(),
        tags,
        opts.note.as_deref(),
    )?;

    println!("{} {}", style("Job ID:").green().bold(), job_id);
    println!("{} {}", style("Job directory:").dim(), job_dir.display());
    println!();

    if opts.background {
        #[cfg(windows)]
        return Err(FlecheError::MissingDependency(
            "Background local jobs (--bg) are not supported on Windows.\n  \
             Use foreground mode or run in WSL."
                .to_string(),
        ));

        // Run in background
        #[cfg(unix)]
        let pid = local::run_background(&config.project_path, &job_id, &job.command, &job.env)?;
        println!("{} {}", style("PID:").green().bold(), pid);
        println!(
            "{}",
            style("Job running in background. Use 'fleche logs' to view output.").dim()
        );

        // Update status to running
        registry.update_status(&job_id, JobStatus::Running)?;

        if ctx.should_notify(opts.notify) {
            // Spawn a background task to wait and notify
            let project_path = config.project_path.clone();
            let job_id_clone = job_id.clone();
            let poll_interval = ctx.poll_interval_local_secs;
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(poll_interval)).await;
                    match local::get_local_job_status(&project_path, &job_id_clone) {
                        Ok(status) => {
                            if let Ok(registry) = Registry::open() {
                                let _ = registry.update_status(&job_id_clone, status);
                            }
                            match status {
                                JobStatus::Completed => {
                                    send_notification(&format!(
                                        "Job {job_id_clone} completed successfully."
                                    ));
                                    break;
                                }
                                JobStatus::Failed => {
                                    send_notification(&format!("Job {job_id_clone} failed."));
                                    break;
                                }
                                JobStatus::Cancelled => {
                                    send_notification(&format!(
                                        "Job {job_id_clone} was cancelled."
                                    ));
                                    break;
                                }
                                _ => {}
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }
    } else {
        // Run in foreground with retry support
        let max_attempts = opts.retry.map_or(1, |r| r + 1);
        let mut attempt = 0;

        loop {
            attempt += 1;

            // Generate new job ID for retries
            let job_id = if attempt == 1 {
                job_id.clone()
            } else {
                let new_id = generate_job_id(&job.name);
                let _job_dir = local::ensure_job_dir(&config.project_path, &new_id)?;
                let registry = Registry::open()?;
                registry.insert_job(
                    &new_id,
                    None,
                    job,
                    &config.project_name,
                    &config.project_path.to_string_lossy(),
                    "local",
                    &config.project_path.to_string_lossy(),
                    tags,
                    None, // Retries don't get a note
                )?;
                println!("{} {}", style("Job ID:").green().bold(), new_id);
                println!();
                new_id
            };

            println!(
                "{}",
                style("Running locally (Ctrl+C to cancel)...").yellow()
            );
            if attempt > 1 {
                println!(
                    "{}",
                    style(format!("(attempt {attempt}/{max_attempts})")).dim()
                );
            }
            println!();

            // Update status to running
            let registry = Registry::open()?;
            registry.update_status(&job_id, JobStatus::Running)?;

            let exit_code =
                local::run_foreground(&config.project_path, &job_id, &job.command, &job.env)?;

            let final_status = if exit_code == 0 {
                JobStatus::Completed
            } else {
                JobStatus::Failed
            };
            registry.update_status(&job_id, final_status)?;

            println!();
            if exit_code == 0 {
                println!("{}", style("Job completed successfully.").green().bold());
                if ctx.should_notify(opts.notify) {
                    send_notification(&format!("Job {job_id} completed successfully."));
                }
                break;
            }

            println!(
                "{} (exit code: {})",
                style("Job failed.").red().bold(),
                exit_code
            );

            // Check if we should retry
            if attempt < max_attempts {
                let delay_secs = ctx.retry_base_delay_secs * (1 << (attempt - 1));
                println!();
                println!(
                    "{} Retrying in {} seconds (attempt {}/{})...",
                    style("↻").yellow().bold(),
                    delay_secs,
                    attempt + 1,
                    max_attempts
                );
                tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                println!();
            } else {
                if ctx.should_notify(opts.notify) {
                    send_notification(&format!("Job {job_id} failed."));
                }
                break;
            }
        }
    }

    Ok(())
}

/// Prepares remote workspace and job directory, then syncs code and inputs.
async fn prepare_remote_workspace(
    config: &Config,
    host: &str,
    workspace: &str,
    job_dir: &str,
    inputs: &[String],
    ctx: RuntimeCtx,
) -> Result<crate::ssh::SshClient> {
    let ssh = ctx.ssh(host);

    println!(
        "{} Creating remote directories...",
        style("[1/4]").bold().dim()
    );
    ssh.mkdir(workspace).await?;
    ssh.mkdir(job_dir).await?;

    print!("{} Syncing project code...", style("[2/4]").bold().dim());
    let _ = std::io::stdout().flush();
    let stats = sync_project_to_workspace(&config.project_path, host, workspace).await?;
    println!(" {}", style(format!("({})", stats.human_readable())).dim());

    if inputs.is_empty() {
        println!("{} No input files to sync", style("[3/4]").bold().dim());
    } else {
        print!("{} Syncing input files...", style("[3/4]").bold().dim());
        let _ = std::io::stdout().flush();
        let stats = sync_inputs_to_workspace(&config.project_path, inputs, host, workspace).await?;
        println!(" {}", style(format!("({})", stats.human_readable())).dim());
    }

    Ok(ssh)
}

/// Re-runs a previous job with the same settings.
///
/// Fetches the job configuration from the registry and submits a new job
/// with the same command, Slurm settings, and environment variables.
pub async fn rerun_job(
    config: &Config,
    job_id: &str,
    tags: &[(String, String)],
    background: bool,
    ctx: RuntimeCtx,
) -> Result<()> {
    let registry = Registry::open()?;
    let old_job = registry.get_job(job_id)?;

    // Deserialize the old job's configuration
    let resolved: ResolvedJob = serde_json::from_str(&old_job.config_json)?;

    // Merge old job's tags with new tags (new tags take precedence)
    let mut merged_tags: Vec<(String, String)> = old_job
        .tags
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (k, v) in tags {
        if let Some(pos) = merged_tags.iter().position(|(key, _)| key == k) {
            merged_tags[pos] = (k.clone(), v.clone());
        } else {
            merged_tags.push((k.clone(), v.clone()));
        }
    }

    // Run with the old job's resolved configuration
    run_job_with_resolved(config, &resolved, &merged_tags, background, ctx).await
}

/// Runs a job with an already-resolved configuration.
async fn run_job_with_resolved(
    config: &Config,
    job: &ResolvedJob,
    tags: &[(String, String)],
    background: bool,
    ctx: RuntimeCtx,
) -> Result<()> {
    if job.host == "local" {
        let opts = RunJobOptions {
            background,
            notify: false,
            dry_run: false,
            after: None,
            retry: None,
            note: None,
        };
        return run_job_locally(config, job, tags, &opts, ctx).await;
    }

    let workspace = workspace_path(config);
    let host = job.host.clone();

    // Generate unique job ID
    let job_id = generate_job_id(&job.name);
    let job_dir = job_path(config, &job_id);
    let ssh =
        prepare_remote_workspace(config, &host, &workspace, &job_dir, &job.inputs, ctx).await?;

    // Generate and upload sbatch script
    println!("{} Submitting job to Slurm...", style("[4/4]").bold().dim());
    let script = generate_sbatch_script(&job_id, job, &workspace, &job_dir);
    ssh.write_file(&format!("{job_dir}/job.sbatch"), &script)
        .await?;

    // Submit job (no dependency for rerun)
    let slurm_id = submit_job(&ssh, &job_dir, None).await?;

    // Record in registry
    let registry = Registry::open()?;
    registry.insert_job(
        &job_id,
        Some(&slurm_id),
        job,
        &config.project_name,
        &config.project_path.to_string_lossy(),
        &host,
        &workspace,
        tags,
        None, // Reruns don't get a note
    )?;

    println!();
    println!("{} {}", style("Job ID:").green().bold(), job_id);
    println!("{} {}", style("Slurm ID:").green().bold(), slurm_id);

    if !background {
        println!();
        let log_path = format!("{job_dir}/job.out");
        follow_job_logs(&host, &slurm_id, &log_path, ctx).await?;
    }

    Ok(())
}

/// Executes a command directly via SSH (no Slurm), or locally if host is "local".
///
/// For remote: syncs the project and inputs, then runs the command directly over SSH.
/// For local: runs the command directly in the project directory.
/// Useful for quick tests or interactive work.
pub async fn exec_command(
    config: &Config,
    command: &str,
    env_overrides: &[(String, String)],
    host_override: Option<&str>,
    ctx: RuntimeCtx,
) -> Result<()> {
    let host = host_override.map_or_else(|| config.remote.host.clone(), String::from);

    // Local execution path
    if host == "local" {
        return exec_command_locally(config, command, env_overrides);
    }

    // Remote execution path
    let workspace = workspace_path(config);
    let ssh = ctx.ssh(&host);

    // Create workspace if needed
    println!(
        "{} Creating remote directories...",
        style("[1/3]").bold().dim()
    );
    ssh.mkdir(&workspace).await?;

    // Sync project code to workspace
    print!("{} Syncing project code...", style("[2/3]").bold().dim());
    let _ = std::io::stdout().flush();
    let stats = sync_project_to_workspace(&config.project_path, &host, &workspace).await?;
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
        let stats =
            sync_inputs_to_workspace(&config.project_path, &global_inputs, &host, &workspace)
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
        return Err(FlecheError::SshCommand(
            "Command exited with non-zero status".to_string(),
        ));
    }

    Ok(())
}

/// Executes a command locally (when host is "local").
fn exec_command_locally(
    config: &Config,
    command: &str,
    env_overrides: &[(String, String)],
) -> Result<()> {
    require_shell()?;

    println!(
        "{} Executing command locally...",
        style("[1/1]").bold().dim()
    );
    println!();

    let mut cmd = local::shell_command(command);
    cmd.current_dir(&config.project_path);

    // Add environment variables
    for (k, v) in env_overrides {
        cmd.env(k, v);
    }

    let status = cmd.status()?;

    if !status.success() {
        return Err(FlecheError::SshCommand(
            "Command exited with non-zero status".to_string(),
        ));
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
///
/// Returns the final job status when the job completes.
async fn follow_job_logs(
    host: &str,
    slurm_id: &str,
    log_path: &str,
    ctx: RuntimeCtx,
) -> Result<JobStatus> {
    println!(
        "{}",
        style("Streaming output (Ctrl+C to disconnect, job keeps running)...").yellow()
    );

    let ssh = ctx.ssh(host);
    let mut child = ssh.tail_follow(log_path)?;

    // Poll job status until it reaches a terminal state
    let slurm_id = slurm_id.to_string();
    let host = host.to_string();
    let slurm_id_for_check = slurm_id.clone();
    let host_for_check = host.clone();
    let status_check = async move {
        loop {
            tokio::time::sleep(Duration::from_secs(ctx.poll_interval_remote_secs)).await;
            let ssh = ctx.ssh(&host_for_check);
            if let Ok(status) = get_job_status(&ssh, &slurm_id_for_check).await {
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
    let final_status = tokio::select! {
        _ = child.wait() => {
            // Tail exited on its own - check final status
            let ssh = ctx.ssh(&host);
            get_job_status(&ssh, &slurm_id).await.unwrap_or(JobStatus::Failed)
        }
        status = status_check => {
            // Job finished, kill tail and print status
            let _ = child.kill().await;

            // Give a moment for any final output to flush
            tokio::time::sleep(Duration::from_millis(500)).await;

            status
        }
    };

    println!();
    let message = match final_status {
        JobStatus::Completed => "Job completed successfully.",
        JobStatus::Failed => "Job failed.",
        JobStatus::Cancelled => "Job cancelled.",
        _ => "Job finished.",
    };

    match final_status {
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

    send_notification(message);

    Ok(final_status)
}

/// Waits for a job to complete and sends a terminal notification.
///
/// Polls the job status every few seconds until it reaches a terminal state.
async fn wait_and_notify(job_id: &str, remote_host: &str, ctx: RuntimeCtx) -> Result<()> {
    println!(
        "{}",
        style("Waiting for job to complete (will notify when done)...").dim()
    );

    let registry = Registry::open()?;
    let ssh = ctx.ssh(remote_host);

    loop {
        let job = registry.get_job(job_id)?;
        if let Some(ref slurm_id) = job.slurm_id {
            let status = get_job_status(&ssh, slurm_id).await?;
            registry.update_status(job_id, status)?;

            match status {
                JobStatus::Completed => {
                    let message = format!("Job {job_id} completed successfully.");
                    println!("{}", style(&message).green().bold());
                    send_notification(&message);
                    return Ok(());
                }
                JobStatus::Failed => {
                    let message = format!("Job {job_id} failed.");
                    println!("{}", style(&message).red().bold());
                    send_notification(&message);
                    return Ok(());
                }
                JobStatus::Cancelled => {
                    let message = format!("Job {job_id} was cancelled.");
                    println!("{}", style(&message).yellow().bold());
                    send_notification(&message);
                    return Ok(());
                }
                _ => {}
            }
        }

        tokio::time::sleep(Duration::from_secs(ctx.poll_interval_remote_secs)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_job_id_format() {
        let id = generate_job_id("train");

        // Starts with job name
        assert!(id.starts_with("train-"));

        // Has expected structure: name-YYYYMMDD-HHMMSS-mmm-xxxx
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5); // train, date, time, millis, suffix

        // Timestamp parts are numeric
        assert!(parts[1].chars().all(|c| c.is_ascii_digit())); // YYYYMMDD
        assert!(parts[2].chars().all(|c| c.is_ascii_digit())); // HHMMSS

        // Suffix is 4 lowercase alphanumeric
        assert_eq!(parts[4].len(), 4);
        assert!(
            parts[4]
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }

    #[test]
    fn test_generate_job_id_uniqueness() {
        let ids: Vec<String> = (0..100).map(|_| generate_job_id("test")).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len());
    }

    #[test]
    fn test_generate_job_id_with_hyphenated_name() {
        let id = generate_job_id("my-job");

        assert!(id.starts_with("my-job-"));

        // Still has correct structure despite hyphens in name
        let suffix = id.split('-').next_back().unwrap();
        assert_eq!(suffix.len(), 4);
    }
}
