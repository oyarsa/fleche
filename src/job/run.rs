//! Job execution operations - running and re-running jobs on remote clusters.

use crate::config::{Config, ResolvedJob, SlurmConfig, reject_empty_path_entries};
use crate::error::{FlecheError, Result};
use crate::local;
use crate::ntfy;
use crate::registry::{JobStatus, LiveStatus, Registry};
use crate::runtime::{RuntimeCtx, send_notification};
use crate::slurm::{generate_sbatch_script, get_job_status, submit_job};
use crate::ssh::{SshClient, shell_escape};
use crate::sync::{
    list_input_sync_files, list_project_sync_files, sync_inputs_to_workspace,
    sync_project_to_workspace,
};
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
    /// Send push notifications via ntfy.sh on state changes.
    pub ntfy_topic: Option<String>,
    /// Print generated sbatch script without submitting.
    pub dry_run: bool,
    /// Job ID to wait for before starting.
    pub after: Option<String>,
    /// Number of times to retry on failure (with exponential backoff).
    pub retry: Option<u32>,
    /// Note/annotation to attach to the job.
    pub note: Option<String>,
    /// CLI override: run directly via SSH instead of submitting to Slurm.
    pub exec: bool,
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
    // A positional argument is always a configured job name. Ad hoc commands use
    // `--command`, which prevents job-name typos from becoming shell commands.
    let (job_name, actual_command) = if let Some(joc) = job_or_command {
        (Some(joc), command_override)
    } else {
        (None, command_override)
    };

    let mut job = config.resolve_job(
        job_name,
        actual_command,
        env_overrides,
        &slurm_overrides,
        host_override,
    )?;

    // CLI --exec overrides config
    if opts.exec {
        job.exec = true;
    }

    let host = job.host.clone();

    // Branch based on host
    if host == "local" {
        return run_job_locally(config, &job, tags, &opts, ctx).await;
    }

    // Direct remote execution (exec mode) bypasses Slurm
    if job.exec {
        return run_job_direct_remote(config, &job, &host, tags, &opts, ctx).await;
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
    let workspace = workspace_path(config)?;

    if opts.dry_run {
        let job_dir = job_path(config, &job_id)?;
        let script = generate_sbatch_script(&job_id, &job, &workspace, &job_dir);
        println!(
            "{}",
            style("[dry-run] Generated sbatch script:").bold().yellow()
        );
        println!();
        println!("{script}");
        println!();
        print_dry_run_synced_files(config, &job.inputs).await?;
        return Ok(());
    }

    let job_dir = job_path(config, &job_id)?;
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
        let job_dir = job_path(config, &job_id)?;

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

        // Send ntfy pending notification
        if let Some(ref topic) = opts.ntfy_topic {
            ntfy::notify_state_change(
                topic,
                &job_id,
                None,
                JobStatus::Pending,
                opts.note.as_deref(),
            );
        }

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
            if ctx.should_notify(opts.notify) || opts.ntfy_topic.is_some() {
                // Wait for job completion in background and notify
                wait_and_notify(&job_id, &host, opts.ntfy_topic.as_deref(), ctx).await?;
            }
            // Background mode doesn't support retry (we don't wait for completion)
            break;
        }

        // Foreground mode: follow logs and check result
        println!();
        let live = follow_job_logs(
            &host,
            &slurm_id,
            &job_dir,
            opts.ntfy_topic.as_deref(),
            &job_id,
            ctx,
        )
        .await?;

        // Update registry with final status
        registry.update_status(&job_id, &live)?;

        // Check if we should retry
        if live.status == JobStatus::Failed && attempt < max_attempts {
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
        {
            let pid = local::run_background(&config.project_path, &job_id, &job.command, &job.env)?;
            println!("{} {}", style("PID:").green().bold(), pid);
            println!(
                "{}",
                style("Job running in background. Use 'fleche logs' to view output.").dim()
            );

            // Update status to running
            registry.update_status(&job_id, &LiveStatus::new(JobStatus::Running))?;

            if ctx.should_notify(opts.notify) || opts.ntfy_topic.is_some() {
                // Spawn a background task to wait and notify
                let project_path = config.project_path.clone();
                let job_id_clone = job_id.clone();
                let poll_interval = ctx.poll_interval_local_secs;
                let ntfy_topic = opts.ntfy_topic.clone();
                let note = opts.note.clone();
                let should_term_notify = ctx.should_notify(opts.notify);
                tokio::spawn(async move {
                    let mut prev_status: Option<JobStatus> = Some(JobStatus::Running);
                    loop {
                        tokio::time::sleep(Duration::from_secs(poll_interval)).await;
                        match local::get_local_job_status(&project_path, &job_id_clone) {
                            Ok(live) => {
                                if let Ok(registry) = Registry::open() {
                                    let _ = registry.update_status(&job_id_clone, &live);
                                }
                                if let Some(ref topic) = ntfy_topic {
                                    ntfy::notify_state_change(
                                        topic,
                                        &job_id_clone,
                                        prev_status,
                                        live.status,
                                        note.as_deref(),
                                    );
                                    prev_status = Some(live.status);
                                }
                                match live.status {
                                    JobStatus::Completed => {
                                        if should_term_notify {
                                            send_notification(&format!(
                                                "Job {job_id_clone} completed successfully."
                                            ));
                                        }
                                        break;
                                    }
                                    JobStatus::Failed => {
                                        if should_term_notify {
                                            send_notification(&format!(
                                                "Job {job_id_clone} failed."
                                            ));
                                        }
                                        break;
                                    }
                                    JobStatus::Cancelled => {
                                        if should_term_notify {
                                            send_notification(&format!(
                                                "Job {job_id_clone} was cancelled."
                                            ));
                                        }
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
            registry.update_status(&job_id, &LiveStatus::new(JobStatus::Running))?;

            let exit_code =
                local::run_foreground(&config.project_path, &job_id, &job.command, &job.env)?;

            let final_status = if exit_code == 0 {
                JobStatus::Completed
            } else {
                JobStatus::Failed
            };
            registry.update_status(
                &job_id,
                &LiveStatus::with_exit_code(final_status, exit_code),
            )?;

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
    {
        // Serialize SSH ControlMaster creation so concurrent `fleche run`
        // processes don't race to open the shared master socket (the loser's
        // rsync would otherwise fail). The first mkdir establishes the master;
        // the lock releases at the end of this block, before the transfers, so
        // they still run in parallel over the now-established master.
        //
        // Acquire on a blocking thread so waiting on a sibling's lock doesn't
        // stall the async runtime; the guard (an flock on an fd) is valid
        // regardless of which thread took it. Best-effort: proceed unlocked if
        // the blocking task fails.
        let _master_lock = {
            let host = host.to_string();
            tokio::task::spawn_blocking(move || crate::ssh::lock_control_master(&host))
                .await
                .ok()
                .flatten()
        };
        ssh.mkdir(workspace).await?;
        ssh.mkdir(job_dir).await?;
    }

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

/// Prints the files that would be synced to the remote workspace.
///
/// Used by dry-run to show project code and input files without connecting to
/// the remote.
async fn print_dry_run_synced_files(config: &Config, inputs: &[String]) -> Result<()> {
    let project_files = list_project_sync_files(&config.project_path).await?;
    println!(
        "{}",
        style(format!(
            "[dry-run] Project files to sync ({}):",
            project_files.len()
        ))
        .bold()
        .yellow()
    );
    for file in &project_files {
        println!("  {file}");
    }

    let input_files = list_input_sync_files(&config.project_path, inputs).await?;
    if !input_files.is_empty() {
        println!();
        println!(
            "{}",
            style(format!(
                "[dry-run] Input files to sync ({}):",
                input_files.len()
            ))
            .bold()
            .yellow()
        );
        for file in &input_files {
            println!("  {file}");
        }
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
    ntfy_topic: Option<&str>,
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
    run_job_with_resolved(config, &resolved, &merged_tags, background, ntfy_topic, ctx).await
}

/// Runs a job with an already-resolved configuration.
async fn run_job_with_resolved(
    config: &Config,
    job: &ResolvedJob,
    tags: &[(String, String)],
    background: bool,
    ntfy_topic: Option<&str>,
    ctx: RuntimeCtx,
) -> Result<()> {
    if job.host == "local" {
        let opts = RunJobOptions {
            background,
            notify: false,
            ntfy_topic: ntfy_topic.map(String::from),
            dry_run: false,
            after: None,
            retry: None,
            note: None,
            exec: false,
        };
        return run_job_locally(config, job, tags, &opts, ctx).await;
    }

    // Direct remote execution (exec mode) bypasses Slurm
    if job.exec {
        let opts = RunJobOptions {
            background,
            notify: false,
            ntfy_topic: ntfy_topic.map(String::from),
            dry_run: false,
            after: None,
            retry: None,
            note: None,
            exec: true,
        };
        return run_job_direct_remote(config, job, &job.host, tags, &opts, ctx).await;
    }

    let workspace = workspace_path(config)?;
    let host = job.host.clone();

    // Generate unique job ID
    let job_id = generate_job_id(&job.name);
    let job_dir = job_path(config, &job_id)?;
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

    // Send ntfy pending notification
    if let Some(topic) = ntfy_topic {
        ntfy::notify_state_change(topic, &job_id, None, JobStatus::Pending, None);
    }

    println!();
    println!("{} {}", style("Job ID:").green().bold(), job_id);
    println!("{} {}", style("Slurm ID:").green().bold(), slurm_id);

    if !background {
        println!();
        follow_job_logs(&host, &slurm_id, &job_dir, ntfy_topic, &job_id, ctx).await?;
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
    no_sync: bool,
    ctx: RuntimeCtx,
) -> Result<()> {
    let host = match host_override {
        Some(host) => host.to_string(),
        None => config.require_remote()?.host.clone(),
    };

    // Local execution path
    if host == "local" {
        return exec_command_locally(config, command, env_overrides);
    }

    // Remote execution path
    let workspace = workspace_path(config)?;
    let ssh = ctx.ssh(&host);

    if no_sync {
        println!("Skipping sync, executing command directly...");
    } else {
        // Reject empty input entries before touching the network, so `fleche
        // exec` fails fast with the same error as `fleche run` instead of
        // silently skipping them (see Config::resolve_job).
        for (name, job) in &config.jobs {
            reject_empty_path_entries(name, "inputs", &job.inputs, &job.inputs)?;
        }

        // Create workspace if needed
        println!(
            "{} Creating remote directories...",
            style("[1/3]").bold().dim()
        );
        {
            // Serialize SSH ControlMaster creation across concurrent processes
            // before the single-shot rsync (see prepare_remote_workspace).
            // Acquire off the reactor so the wait doesn't stall the runtime.
            let _master_lock = {
                let host = host.clone();
                tokio::task::spawn_blocking(move || crate::ssh::lock_control_master(&host))
                    .await
                    .ok()
                    .flatten()
            };
            ssh.mkdir(&workspace).await?;
        }

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

/// The character set for job ID suffixes: lowercase letters and digits.
///
/// Restricted to lowercase to avoid collisions on case-insensitive filesystems
/// (e.g. macOS) where the ID is used as a directory name.
const SUFFIX_CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Generates a unique job ID from the job name and current timestamp.
fn generate_job_id(job_name: &str) -> String {
    let mut rng = rand::thread_rng();
    let suffix: String = (0..4)
        .map(|_| SUFFIX_CHARSET[rng.gen_range(0..SUFFIX_CHARSET.len())] as char)
        .collect();
    let now = Utc::now();
    format!(
        "{}-{}-{}",
        job_name,
        now.format("%Y%m%d-%H%M%S-%3f"),
        suffix
    )
}

/// Follows job logs and automatically exits when the job finishes.
///
/// Returns the final live status when the job completes.
async fn follow_job_logs(
    host: &str,
    slurm_id: &str,
    job_dir: &str,
    ntfy_topic: Option<&str>,
    job_id: &str,
    ctx: RuntimeCtx,
) -> Result<LiveStatus> {
    println!(
        "{}",
        style("Streaming output (Ctrl+C to disconnect, job keeps running)...").yellow()
    );

    let ssh = ctx.ssh(host);
    let stdout_path = format!("{job_dir}/job.out");
    let stderr_path = format!("{job_dir}/job.err");
    let mut child = ssh.tail_follow(&[&stdout_path, &stderr_path])?;

    // Poll job status until it reaches a terminal state
    let slurm_id = slurm_id.to_string();
    let host = host.to_string();
    let slurm_id_for_check = slurm_id.clone();
    let host_for_check = host.clone();
    let ntfy_topic_owned = ntfy_topic.map(String::from);
    let job_id_owned = job_id.to_string();
    let status_check = async move {
        let mut prev_status: Option<JobStatus> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(ctx.poll_interval_remote_secs)).await;
            let ssh = ctx.ssh(&host_for_check);
            if let Ok(live) = get_job_status(&ssh, &slurm_id_for_check).await {
                if let Some(ref topic) = ntfy_topic_owned {
                    ntfy::notify_state_change(topic, &job_id_owned, prev_status, live.status, None);
                    prev_status = Some(live.status);
                }
                match live.status {
                    JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => {
                        return live;
                    }
                    _ => {}
                }
            }
        }
    };

    // Wait for either the tail process to exit or the job to finish
    let live = tokio::select! {
        _ = child.wait() => {
            // Tail exited on its own - check final status.
            // Retry a few times because Slurm accounting (sacct) can lag behind
            // the actual job completion, causing a transient lookup failure.
            let ssh = ctx.ssh(&host);
            let mut result = None;
            for attempt in 0..6 {
                if attempt > 0 {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                if let Ok(live) = get_job_status(&ssh, &slurm_id).await {
                    result = Some(live);
                    break;
                }
            }
            result.unwrap_or_else(|| LiveStatus::new(JobStatus::Failed))
        }
        result = status_check => {
            // Job finished, kill tail and print status
            let _ = child.kill().await;

            // Give a moment for any final output to flush
            tokio::time::sleep(Duration::from_millis(500)).await;

            result
        }
    };

    println!();
    let message = match live.status {
        JobStatus::Completed => "Job completed successfully.".to_string(),
        JobStatus::Failed => match live.exit_code {
            Some(code) => format!("Job failed (exit code: {code})."),
            None => "Job failed.".to_string(),
        },
        JobStatus::Cancelled => "Job cancelled.".to_string(),
        _ => "Job finished.".to_string(),
    };

    match live.status {
        JobStatus::Completed => {
            println!("{}", style(&message).green().bold());
        }
        JobStatus::Failed => {
            println!("{}", style(&message).red().bold());
        }
        JobStatus::Cancelled => {
            println!("{}", style(&message).yellow().bold());
        }
        _ => {}
    }

    send_notification(&message);

    Ok(live)
}

/// Waits for a job to complete and sends a terminal notification.
///
/// Polls the job status every few seconds until it reaches a terminal state.
async fn wait_and_notify(
    job_id: &str,
    remote_host: &str,
    ntfy_topic: Option<&str>,
    ctx: RuntimeCtx,
) -> Result<()> {
    println!(
        "{}",
        style("Waiting for job to complete (will notify when done)...").dim()
    );

    let registry = Registry::open()?;
    let ssh = ctx.ssh(remote_host);
    let mut prev_status: Option<JobStatus> = None;

    loop {
        let job = registry.get_job(job_id)?;
        if let Some(ref slurm_id) = job.slurm_id {
            let live = get_job_status(&ssh, slurm_id).await?;
            registry.update_status(job_id, &live)?;

            if let Some(topic) = ntfy_topic {
                ntfy::notify_state_change(topic, job_id, prev_status, live.status, None);
                prev_status = Some(live.status);
            }

            match live.status {
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

/// Runs a job directly on a remote host via SSH (no Slurm).
async fn run_job_direct_remote(
    config: &Config,
    job: &ResolvedJob,
    host: &str,
    tags: &[(String, String)],
    opts: &RunJobOptions,
    ctx: RuntimeCtx,
) -> Result<()> {
    // Warn about Slurm options that don't apply in exec mode
    if job.slurm.partition.is_some()
        || job.slurm.time.is_some()
        || job.slurm.gpus.is_some()
        || job.slurm.cpus.is_some()
        || job.slurm.memory.is_some()
    {
        eprintln!(
            "{}",
            style("Warning: Slurm options are ignored for exec jobs").yellow()
        );
    }

    let job_id = generate_job_id(&job.name);
    let workspace = workspace_path(config)?;
    let job_dir = job_path(config, &job_id)?;

    if opts.dry_run {
        let script = generate_exec_script(job, &workspace, &job_dir);
        println!(
            "{}",
            style("[dry-run] Generated exec script:").bold().yellow()
        );
        println!();
        println!("{script}");
        println!();
        print_dry_run_synced_files(config, &job.inputs).await?;
        return Ok(());
    }

    let ssh =
        prepare_remote_workspace(config, host, &workspace, &job_dir, &job.inputs, ctx).await?;

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
        let job_dir = job_path(config, &job_id)?;

        // Create job directory for this attempt
        if attempt > 1 {
            ssh.mkdir(&job_dir).await?;
        }

        // Generate and upload exec script
        let script = generate_exec_script(job, &workspace, &job_dir);

        println!(
            "{} Starting remote exec job...",
            style("[4/4]").bold().dim()
        );
        ssh.write_file(&format!("{job_dir}/run.sh"), &script)
            .await?;

        // Start the job via nohup
        ssh.exec(&format!(
            "nohup sh {job_dir}/run.sh > /dev/null 2>&1 & echo started"
        ))
        .await?;

        // Record in registry (slurm_id = None for exec jobs)
        let registry = Registry::open()?;
        let job_note = if attempt == 1 {
            opts.note.as_deref()
        } else {
            None
        };
        registry.insert_job(
            &job_id,
            None, // No Slurm ID for exec jobs
            job,
            &config.project_name,
            &config.project_path.to_string_lossy(),
            host,
            &workspace,
            tags,
            job_note,
        )?;

        // Update status to running
        registry.update_status(&job_id, &LiveStatus::new(JobStatus::Running))?;

        // Send ntfy running notification (exec jobs go straight to running)
        if let Some(ref topic) = opts.ntfy_topic {
            ntfy::notify_state_change(
                topic,
                &job_id,
                None,
                JobStatus::Running,
                opts.note.as_deref(),
            );
        }

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

        if opts.background {
            println!(
                "{}",
                style("Job running in background. Use 'fleche logs' to view output.").dim()
            );

            if ctx.should_notify(opts.notify) || opts.ntfy_topic.is_some() {
                wait_and_notify_direct(&job_id, host, &job_dir, opts.ntfy_topic.as_deref(), ctx)
                    .await?;
            }
            // Background mode doesn't support retry
            break;
        }

        // Foreground mode: follow logs and check result
        println!();
        let live = follow_direct_job_logs(host, &job_dir, opts.ntfy_topic.as_deref(), &job_id, ctx)
            .await?;

        // Update registry with final status
        registry.update_status(&job_id, &live)?;

        // Check if we should retry
        if live.status == JobStatus::Failed && attempt < max_attempts {
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

/// Generates a wrapper script for direct remote execution.
///
/// The script writes a PID file, sets up the environment, runs the command,
/// and writes the exit code on completion. This mirrors the local background
/// execution pattern but runs on the remote host.
fn generate_exec_script(job: &ResolvedJob, workspace: &str, job_dir: &str) -> String {
    let mut script = String::from("#!/bin/sh\n");
    script.push_str(&format!("echo $$ > {job_dir}/pid\n"));
    script.push_str(&format!("cd {}\n", shell_escape(workspace)));

    // Environment variables
    for (key, value) in &job.env {
        script.push_str(&format!("export {}={}\n", key, shell_escape(value)));
    }

    // Command with output redirection
    script.push_str(&format!(
        "{} > {job_dir}/job.out 2> {job_dir}/job.err\n",
        job.command
    ));
    script.push_str(&format!("echo $? > {job_dir}/exit_code\n"));

    script
}

/// Checks the status of a remote direct (exec) job by inspecting files via SSH.
///
/// Checks in order:
/// 1. `exit_code` file exists → completed (0) or failed (non-zero)
/// 2. `pid` file exists and process running → running
/// 3. `pid` file exists but process gone → failed (crashed without exit code)
/// 4. Neither file → pending
pub async fn get_remote_direct_job_status(ssh: &SshClient, job_dir: &str) -> Result<LiveStatus> {
    // Check if exit_code exists
    let (has_exit_code, exit_code_content, _) = ssh
        .exec_allow_failure(&format!("cat {job_dir}/exit_code 2>/dev/null"))
        .await?;

    if has_exit_code && !exit_code_content.trim().is_empty() {
        let code: i32 = exit_code_content.trim().parse().unwrap_or(1);
        let status = if code == 0 {
            JobStatus::Completed
        } else {
            JobStatus::Failed
        };
        return Ok(LiveStatus::with_exit_code(status, code));
    }

    // Check if PID exists and process is running
    let (has_pid, pid_content, _) = ssh
        .exec_allow_failure(&format!("cat {job_dir}/pid 2>/dev/null"))
        .await?;

    if has_pid && !pid_content.trim().is_empty() {
        let pid = pid_content.trim();
        // Check if process is still running
        let (is_running, _, _) = ssh
            .exec_allow_failure(&format!("kill -0 {pid} 2>/dev/null"))
            .await?;

        if is_running {
            return Ok(LiveStatus::new(JobStatus::Running));
        }

        // PID exists but process is gone - job failed without writing exit code
        return Ok(LiveStatus::new(JobStatus::Failed));
    }

    // No PID file - job hasn't started yet
    Ok(LiveStatus::new(JobStatus::Pending))
}

/// Follows logs of a remote direct job and polls for completion.
///
/// Returns the final live status when the job completes.
async fn follow_direct_job_logs(
    host: &str,
    job_dir: &str,
    ntfy_topic: Option<&str>,
    job_id: &str,
    ctx: RuntimeCtx,
) -> Result<LiveStatus> {
    println!(
        "{}",
        style("Streaming output (Ctrl+C to disconnect, job keeps running)...").yellow()
    );

    let ssh = ctx.ssh(host);
    let stdout_path = format!("{job_dir}/job.out");
    let stderr_path = format!("{job_dir}/job.err");
    let mut child = ssh.tail_follow(&[&stdout_path, &stderr_path])?;

    // Poll job status until it reaches a terminal state
    let job_dir_owned = job_dir.to_string();
    let host_owned = host.to_string();
    let ntfy_topic_owned = ntfy_topic.map(String::from);
    let job_id_owned = job_id.to_string();
    let status_check = async move {
        let mut prev_status: Option<JobStatus> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(ctx.poll_interval_remote_secs)).await;
            let ssh = ctx.ssh(&host_owned);
            if let Ok(live) = get_remote_direct_job_status(&ssh, &job_dir_owned).await {
                if let Some(ref topic) = ntfy_topic_owned {
                    ntfy::notify_state_change(topic, &job_id_owned, prev_status, live.status, None);
                    prev_status = Some(live.status);
                }
                match live.status {
                    JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => {
                        return live;
                    }
                    _ => {}
                }
            }
        }
    };

    // Wait for either the tail process to exit or the job to finish
    let live = tokio::select! {
        _ = child.wait() => {
            // Tail exited on its own - check final status with retries
            let ssh = ctx.ssh(host);
            let mut result = None;
            for attempt in 0..6 {
                if attempt > 0 {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                if let Ok(live) = get_remote_direct_job_status(&ssh, job_dir).await {
                    result = Some(live);
                    break;
                }
            }
            result.unwrap_or_else(|| LiveStatus::new(JobStatus::Failed))
        }
        result = status_check => {
            // Job finished, kill tail
            let _ = child.kill().await;
            tokio::time::sleep(Duration::from_millis(500)).await;
            result
        }
    };

    println!();
    let message = match live.status {
        JobStatus::Completed => "Job completed successfully.".to_string(),
        JobStatus::Failed => match live.exit_code {
            Some(code) => format!("Job failed (exit code: {code})."),
            None => "Job failed.".to_string(),
        },
        JobStatus::Cancelled => "Job cancelled.".to_string(),
        _ => "Job finished.".to_string(),
    };

    match live.status {
        JobStatus::Completed => {
            println!("{}", style(&message).green().bold());
        }
        JobStatus::Failed => {
            println!("{}", style(&message).red().bold());
        }
        JobStatus::Cancelled => {
            println!("{}", style(&message).yellow().bold());
        }
        _ => {}
    }

    send_notification(&message);

    Ok(live)
}

/// Waits for a remote direct job to complete and sends a notification.
async fn wait_and_notify_direct(
    job_id: &str,
    remote_host: &str,
    job_dir: &str,
    ntfy_topic: Option<&str>,
    ctx: RuntimeCtx,
) -> Result<()> {
    println!(
        "{}",
        style("Waiting for job to complete (will notify when done)...").dim()
    );

    let ssh = ctx.ssh(remote_host);
    let mut prev_status: Option<JobStatus> = None;

    loop {
        let live = get_remote_direct_job_status(&ssh, job_dir).await?;

        if let Ok(registry) = Registry::open() {
            let _ = registry.update_status(job_id, &live);
        }

        if let Some(topic) = ntfy_topic {
            ntfy::notify_state_change(topic, job_id, prev_status, live.status, None);
            prev_status = Some(live.status);
        }

        match live.status {
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

        tokio::time::sleep(Duration::from_secs(ctx.poll_interval_remote_secs)).await;
    }
}

/// Cancels a remote direct job by killing its PID via SSH.
///
/// Returns `Ok(true)` if the process was killed, `Ok(false)` if no PID found.
pub async fn cancel_remote_direct_job(ssh: &SshClient, job_dir: &str) -> Result<bool> {
    // Read PID file
    let (has_pid, pid_content, _) = ssh
        .exec_allow_failure(&format!("cat {job_dir}/pid 2>/dev/null"))
        .await?;

    if !has_pid || pid_content.trim().is_empty() {
        return Ok(false);
    }

    let pid = pid_content.trim();

    // Kill the process
    let (success, _, _) = ssh
        .exec_allow_failure(&format!("kill {pid} 2>/dev/null"))
        .await?;

    if success {
        // Write exit code to indicate cancellation (143 = 128 + 15 SIGTERM)
        let _ = ssh
            .exec_allow_failure(&format!("echo 143 > {job_dir}/exit_code"))
            .await;
    }

    Ok(success)
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

    #[test]
    fn test_generate_exec_script_basic() {
        use crate::config::{ResolvedJob, SlurmConfig};
        use indexmap::IndexMap;

        let job = ResolvedJob {
            name: "test".to_string(),
            command: "echo hello".to_string(),
            inputs: vec![],
            outputs: vec![],
            slurm: SlurmConfig::default(),
            env: IndexMap::new(),
            host: "cluster".to_string(),
            exec: true,
        };

        let script = generate_exec_script(&job, "/workspace", "/jobs/test-123");

        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("echo $$ > /jobs/test-123/pid"));
        assert!(script.contains("cd '/workspace'"));
        assert!(script.contains("echo hello > /jobs/test-123/job.out 2> /jobs/test-123/job.err"));
        assert!(script.contains("echo $? > /jobs/test-123/exit_code"));
    }

    #[test]
    fn test_generate_exec_script_with_env() {
        use crate::config::{ResolvedJob, SlurmConfig};
        use indexmap::IndexMap;

        let mut env = IndexMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        env.insert("PATH_VAR".to_string(), "/some/path".to_string());

        let job = ResolvedJob {
            name: "test".to_string(),
            command: "python train.py".to_string(),
            inputs: vec![],
            outputs: vec![],
            slurm: SlurmConfig::default(),
            env,
            host: "cluster".to_string(),
            exec: true,
        };

        let script = generate_exec_script(&job, "/ws", "/jobs/test-456");

        assert!(script.contains("export FOO='bar'"));
        assert!(script.contains("export PATH_VAR='/some/path'"));
        assert!(script.contains("python train.py > /jobs/test-456/job.out"));
    }
}
