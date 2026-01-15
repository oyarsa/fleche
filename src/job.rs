use crate::config::{Config, ResolvedJob, SlurmConfig};
use crate::error::{FlecheError, Result};
use crate::registry::{parse_duration, JobRecord, JobStatus, Registry};
use crate::slurm::{cancel_job, generate_sbatch_script, get_job_status, submit_job};
use crate::ssh::SshClient;
use crate::sync::{sync_from_remote, sync_input_cached, sync_to_remote};
use chrono::Utc;
use console::style;
use rand::Rng;

pub async fn run_job(
    config: &Config,
    job_name: Option<&str>,
    command_override: Option<&str>,
    env_overrides: &[(String, String)],
    tags: &[(String, String)],
    slurm_overrides: SlurmConfig,
    follow: bool,
    dry_run: bool,
) -> Result<()> {
    let job = config.resolve_job(job_name, command_override, env_overrides, slurm_overrides)?;
    let job_id = generate_job_id(&job.name);

    let remote_path = format!(
        "{}/{}/.fleche/{}",
        config.remote.base_path, config.project_name, job_id
    );

    // Generate script
    let script = generate_sbatch_script(&job_id, &job);

    if dry_run {
        println!("{}", script);
        return Ok(());
    }

    let ssh = SshClient::new(&config.remote.host);

    // Create remote directory
    println!(
        "{} Creating remote directory...",
        style("[1/5]").bold().dim()
    );
    ssh.mkdir(&remote_path).await?;

    // Sync project code
    println!(
        "{} Syncing project code...",
        style("[2/5]").bold().dim()
    );
    sync_to_remote(&config.project_path, &config.remote.host, &remote_path, true).await?;

    // Sync explicit inputs to shared cache
    let fleche_base = format!("{}/{}/.fleche", config.remote.base_path, config.project_name);
    if !job.inputs.is_empty() {
        println!(
            "{} Syncing input files (cached)...",
            style("[3/5]").bold().dim()
        );
        for input in &job.inputs {
            sync_input_cached(
                &config.project_path,
                input,
                &config.remote.host,
                &fleche_base,
                &job_id,
                &ssh,
            )
            .await?;
        }
    } else {
        println!(
            "{} No input files to sync",
            style("[3/5]").bold().dim()
        );
    }

    // Upload script
    println!(
        "{} Uploading job script...",
        style("[4/5]").bold().dim()
    );
    ssh.write_file(&format!("{}/job.sbatch", remote_path), &script)
        .await?;

    // Submit job
    println!(
        "{} Submitting job to Slurm...",
        style("[5/5]").bold().dim()
    );
    let slurm_id = submit_job(&ssh, &remote_path).await?;

    // Record in registry
    let registry = Registry::open()?;
    registry.insert_job(
        &job_id,
        Some(&slurm_id),
        &job,
        &config.project_name,
        &config.project_path.to_string_lossy(),
        &config.remote.host,
        &remote_path,
        tags,
    )?;

    println!();
    println!("{} {}", style("Job ID:").green().bold(), job_id);
    println!("{} {}", style("Slurm ID:").green().bold(), slurm_id);
    println!("{} {}", style("Remote path:").dim(), remote_path);

    if follow {
        println!();
        println!("{}", style("Following output (Ctrl+C to disconnect)...").yellow());
        let log_path = format!("{}/job.out", remote_path);
        let mut child = ssh.tail_follow(&log_path).await?;
        let _ = child.wait().await;
    }

    Ok(())
}

pub async fn show_status(job_id: Option<&str>) -> Result<()> {
    let registry = Registry::open()?;

    if let Some(id) = job_id {
        let job = registry.get_job(id)?;
        let ssh = SshClient::new(&job.remote_host);

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
        // Show recent jobs
        let jobs = registry.list_jobs(None, None, &[], 20)?;

        if jobs.is_empty() {
            println!("No jobs found. Run `rjob run` to submit a job.");
            return Ok(());
        }

        print_job_table(&jobs);
    }

    Ok(())
}

pub async fn show_logs(job_id: &str, follow: bool, stderr: bool, both: bool) -> Result<()> {
    let registry = Registry::open()?;
    let job = registry.get_job(job_id)?;
    let ssh = SshClient::new(&job.remote_host);

    if both {
        println!("{}", style("=== STDOUT ===").bold());
        let stdout_path = format!("{}/job.out", job.remote_path);
        match ssh.cat(&stdout_path).await {
            Ok(content) => print!("{}", content),
            Err(e) => eprintln!("Error reading stdout: {}", e),
        }

        println!();
        println!("{}", style("=== STDERR ===").bold());
        let stderr_path = format!("{}/job.err", job.remote_path);
        match ssh.cat(&stderr_path).await {
            Ok(content) => print!("{}", content),
            Err(e) => eprintln!("Error reading stderr: {}", e),
        }
    } else if follow {
        let log_file = if stderr { "job.err" } else { "job.out" };
        let log_path = format!("{}/{}", job.remote_path, log_file);

        println!(
            "{}",
            style("Following output (Ctrl+C to disconnect)...").yellow()
        );
        let mut child = ssh.tail_follow(&log_path).await?;
        let _ = child.wait().await;
    } else {
        let log_file = if stderr { "job.err" } else { "job.out" };
        let log_path = format!("{}/{}", job.remote_path, log_file);

        let content = ssh.cat(&log_path).await?;
        print!("{}", content);
    }

    Ok(())
}

pub async fn sync_outputs(job_id: &str, partial: bool) -> Result<()> {
    let registry = Registry::open()?;
    let job = registry.get_job(job_id)?;

    // Check job status
    if !partial
        && matches!(job.status, JobStatus::Pending | JobStatus::Running)
        && let Some(ref slurm_id) = job.slurm_id
    {
        let ssh = SshClient::new(&job.remote_host);
        let current_status = get_job_status(&ssh, slurm_id).await.unwrap_or(job.status);
        if matches!(current_status, JobStatus::Pending | JobStatus::Running) {
            eprintln!(
                "{}",
                style("Warning: Job is still running. Use --partial to sync anyway.").yellow()
            );
            return Ok(());
        }
    }

    // Parse config to get outputs
    let resolved: ResolvedJob = serde_json::from_str(&job.config_json)?;

    if resolved.outputs.is_empty() {
        println!("No outputs defined for this job.");
        return Ok(());
    }

    let local_path = std::path::PathBuf::from(&job.project_path);

    println!("Syncing outputs from {}...", job.remote_path);
    for output in &resolved.outputs {
        println!("  {}", output);
        sync_from_remote(&job.remote_host, &job.remote_path, output, &local_path).await?;
    }

    registry.set_outputs_synced(job_id)?;
    println!("{}", style("Outputs synced successfully.").green());

    Ok(())
}

pub async fn list_jobs(
    project_filter: Option<&str>,
    status_filter: Option<&str>,
    tags: &[(String, String)],
    failed: bool,
    running: bool,
    completed: bool,
) -> Result<()> {
    let registry = Registry::open()?;

    // Determine status filter
    let status = if failed {
        Some(JobStatus::Failed)
    } else if running {
        Some(JobStatus::Running)
    } else if completed {
        Some(JobStatus::Completed)
    } else if let Some(s) = status_filter {
        Some(s.parse()?)
    } else {
        None
    };

    let jobs = registry.list_jobs(project_filter, status, tags, 100)?;

    if jobs.is_empty() {
        println!("No jobs found.");
        return Ok(());
    }

    print_job_table(&jobs);

    Ok(())
}

pub async fn cancel_slurm_job(job_id: &str) -> Result<()> {
    let registry = Registry::open()?;
    let job = registry.get_job(job_id)?;

    if matches!(job.status, JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled) {
        return Err(FlecheError::CannotCancel(
            job_id.to_string(),
            job.status.to_string(),
        ));
    }

    let Some(ref slurm_id) = job.slurm_id else {
        return Err(FlecheError::Other("Job has no Slurm ID".to_string()));
    };

    let ssh = SshClient::new(&job.remote_host);
    cancel_job(&ssh, slurm_id).await?;
    registry.update_status(job_id, JobStatus::Cancelled)?;
    println!("{} Job {} cancelled", style("✓").green(), job_id);

    Ok(())
}

pub async fn clean_job(job_id: Option<&str>, all: bool, older_than: Option<&str>) -> Result<()> {
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

    if jobs_to_clean.is_empty() {
        println!("No jobs to clean.");
        return Ok(());
    }

    for job in &jobs_to_clean {
        println!("Cleaning {}...", job.id);

        // Delete remote directory
        let ssh = SshClient::new(&job.remote_host);
        if let Err(e) = ssh.rm_rf(&job.remote_path).await {
            eprintln!("  Warning: Could not delete remote directory: {}", e);
        }

        // Delete from registry
        registry.delete_job(&job.id)?;
    }

    println!(
        "{} Cleaned {} job(s)",
        style("✓").green(),
        jobs_to_clean.len()
    );

    Ok(())
}

fn generate_job_id(job_name: &str) -> String {
    let now = Utc::now();
    let suffix: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(4)
        .map(char::from)
        .collect::<String>()
        .to_lowercase();
    format!("{}-{}-{}", job_name, now.format("%Y%m%d-%H%M%S-%3f"), suffix)
}

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
    println!("  {:<14} {}", style("Remote Path:").bold(), job.remote_path);
    println!(
        "  {:<14} {}",
        style("Created:").bold(),
        job.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    );

    if !job.tags.is_empty() {
        println!();
        println!("  {}", style("Tags:").bold());
        for (key, value) in &job.tags {
            println!("    {}={}", key, value);
        }
    }

    println!();
    println!("  {}", style("Command:").bold());
    for line in job.command.lines() {
        println!("    {}", line);
    }
}

fn print_job_table(jobs: &[JobRecord]) {
    // Header
    println!(
        "{:<45} {:<12} {:<12} {:<20}",
        style("ID").bold().underlined(),
        style("STATUS").bold().underlined(),
        style("SLURM ID").bold().underlined(),
        style("CREATED").bold().underlined(),
    );

    for job in jobs {
        println!(
            "{:<45} {:<12} {:<12} {:<20}",
            truncate(&job.id, 44),
            format_status(job.status),
            job.slurm_id.as_deref().unwrap_or("-"),
            job.created_at.format("%Y-%m-%d %H:%M"),
        );
    }
}

fn format_status(status: JobStatus) -> String {
    match status {
        JobStatus::Pending => style("pending").yellow().to_string(),
        JobStatus::Running => style("running").blue().to_string(),
        JobStatus::Completed => style("completed").green().to_string(),
        JobStatus::Failed => style("failed").red().to_string(),
        JobStatus::Cancelled => style("cancelled").dim().to_string(),
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
