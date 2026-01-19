//! Job execution operations - running and re-running jobs on remote clusters.

use crate::config::{Config, ResolvedJob, SlurmConfig};
use crate::error::{FlecheError, Result};
use crate::registry::{JobStatus, Registry};
use crate::slurm::{generate_sbatch_script, get_job_status, submit_job};
use crate::ssh::{SshClient, shell_escape};
use crate::sync::{sync_inputs_to_workspace, sync_project_to_workspace};
use chrono::Utc;
use console::style;
use rand::Rng;
use std::io::Write;
use std::time::Duration;

use super::{job_path, workspace_path};

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
#[allow(clippy::fn_params_excessive_bools)]
pub async fn run_job(
    config: &Config,
    job_or_command: Option<&str>,
    command_override: Option<&str>,
    env_overrides: &[(String, String)],
    tags: &[(String, String)],
    slurm_overrides: SlurmConfig,
    background: bool,
    notify: bool,
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
    } else if notify {
        // Wait for job completion in background and notify
        wait_and_notify(&job_id, &config.remote.host, debug).await?;
    }

    Ok(())
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
    debug: bool,
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
    run_job_with_resolved(config, &resolved, &merged_tags, background, debug).await
}

/// Runs a job with an already-resolved configuration.
async fn run_job_with_resolved(
    config: &Config,
    job: &ResolvedJob,
    tags: &[(String, String)],
    background: bool,
    debug: bool,
) -> Result<()> {
    let workspace = workspace_path(config);
    let ssh = SshClient::new(&config.remote.host, debug);

    // Generate unique job ID
    let job_id = generate_job_id(&job.name);
    let job_dir = format!(
        "{}/{}/.fleche/jobs/{}",
        config.remote.base_path, config.project_name, job_id
    );

    // Create directories
    println!(
        "{} Creating remote directories...",
        style("[1/4]").bold().dim()
    );
    ssh.mkdir(&workspace).await?;
    ssh.mkdir(&job_dir).await?;

    // Sync project code
    print!("{} Syncing project code...", style("[2/4]").bold().dim());
    let _ = std::io::stdout().flush();
    let stats =
        sync_project_to_workspace(&config.project_path, &config.remote.host, &workspace).await?;
    println!(" {}", style(format!("({})", stats.human_readable())).dim());

    // Sync inputs if any
    if job.inputs.is_empty() {
        println!("{} Submitting job to Slurm...", style("[3/4]").bold().dim());
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

    // Generate and upload sbatch script
    println!("{} Submitting job to Slurm...", style("[4/4]").bold().dim());
    let script = generate_sbatch_script(&job_id, job, &workspace, &job_dir);
    ssh.write_file(&format!("{job_dir}/job.sbatch"), &script)
        .await?;

    // Submit job
    let slurm_id = submit_job(&ssh, &job_dir).await?;

    // Record in registry
    let registry = Registry::open()?;
    registry.insert_job(
        &job_id,
        Some(&slurm_id),
        job,
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

            send_notification(message);
        }
    }

    Ok(())
}

/// Waits for a job to complete and sends a terminal notification.
///
/// Polls the job status every few seconds until it reaches a terminal state.
async fn wait_and_notify(job_id: &str, remote_host: &str, debug: bool) -> Result<()> {
    println!(
        "{}",
        style("Waiting for job to complete (will notify when done)...").dim()
    );

    let registry = Registry::open()?;
    let ssh = SshClient::new(remote_host, debug);

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

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Sends a terminal notification using OSC 9.
fn send_notification(message: &str) {
    print!("\x1b]9;fleche: {message}\x07");
    let _ = std::io::stdout().flush();
}
