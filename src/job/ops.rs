//! Miscellaneous job operations - wait, ping, and stats.

use crate::config::Config;
use crate::error::{FlecheError, Result};
use crate::local;
use crate::registry::{JobStatus, Registry};
use crate::runtime::{RuntimeCtx, send_notification};
use crate::slurm::{get_job_resource_usage, get_job_status};
use console::style;
use std::path::PathBuf;
use std::time::Duration;

use super::status::resolve_job;

/// Waits for a job to complete.
///
/// Polls the job status until it reaches a terminal state (completed, failed, cancelled).
/// Poll intervals can be customized via the config settings.
pub async fn wait_for_job(
    job_id: Option<&str>,
    notify: bool,
    tags: &[(String, String)],
    ctx: RuntimeCtx,
) -> Result<()> {
    let registry = Registry::open()?;
    let job = resolve_job(&registry, job_id, tags, None)?;

    println!("Waiting for job {}...", style(&job.id).bold());

    // Local job handling
    if job.remote_host == "local" {
        let project_path = PathBuf::from(&job.project_path);

        loop {
            let status = local::get_local_job_status(&project_path, &job.id)?;
            registry.update_status(&job.id, status)?;

            if let Some(msg) = format_terminal_status(&job.id, status) {
                if ctx.should_notify(notify) {
                    send_notification(&msg);
                }
                return Ok(());
            }

            tokio::time::sleep(Duration::from_secs(ctx.poll_interval_local_secs)).await;
        }
    }

    // Remote job handling
    let ssh = ctx.ssh(&job.remote_host);

    loop {
        if let Some(ref slurm_id) = job.slurm_id {
            let status = get_job_status(&ssh, slurm_id).await?;
            registry.update_status(&job.id, status)?;

            if let Some(msg) = format_terminal_status(&job.id, status) {
                if ctx.should_notify(notify) {
                    send_notification(&msg);
                }
                return Ok(());
            }
        } else {
            return Err(FlecheError::NoSlurmId(job.id.clone()));
        }

        tokio::time::sleep(Duration::from_secs(ctx.poll_interval_remote_secs)).await;
    }
}

/// Formats a message for terminal job states, returning None for non-terminal states.
fn format_terminal_status(job_id: &str, status: JobStatus) -> Option<String> {
    match status {
        JobStatus::Completed => {
            let msg = format!("Job {job_id} completed successfully.");
            println!("{}", style(&msg).green().bold());
            Some(msg)
        }
        JobStatus::Failed => {
            let msg = format!("Job {job_id} failed.");
            println!("{}", style(&msg).red().bold());
            Some(msg)
        }
        JobStatus::Cancelled => {
            let msg = format!("Job {job_id} was cancelled.");
            println!("{}", style(&msg).yellow().bold());
            Some(msg)
        }
        _ => None,
    }
}

/// Pings the Slurm controller to check cluster health.
///
/// Runs `scontrol ping` on the remote host and reports the status of the
/// Slurm controller(s). Useful for diagnosing timeout issues.
pub async fn ping_cluster(config: &Config, ctx: RuntimeCtx) -> Result<()> {
    let ssh = ctx.ssh(&config.remote.host);

    println!(
        "Pinging Slurm controller on {}...",
        style(&config.remote.host).bold()
    );
    println!();

    let (success, stdout, stderr) = ssh.exec_allow_failure("scontrol ping").await?;

    if success {
        // Parse and display the output
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Color-code UP/DOWN status
            if line.contains("is UP") {
                println!("{}", style(line).green());
            } else if line.contains("is DOWN") {
                println!("{}", style(line).red());
            } else {
                println!("{line}");
            }
        }
        println!();

        if stdout.contains("is DOWN") {
            println!(
                "{}",
                style("Warning: One or more controllers are down. Jobs may be slow or fail.")
                    .yellow()
            );
        } else {
            println!("{}", style("Cluster is healthy.").green().bold());
        }
    } else {
        // scontrol ping failed entirely
        eprintln!("{}", style("Failed to ping Slurm controller.").red());
        if !stderr.is_empty() {
            eprintln!("{stderr}");
        }
        return Err(FlecheError::SlurmUnavailable);
    }

    Ok(())
}

/// Shows resource usage statistics for completed jobs.
///
/// Queries Slurm's sacct to display elapsed time, CPU time, memory usage,
/// and allocated resources for jobs.
pub async fn show_stats(
    job_id: Option<&str>,
    last: usize,
    tags: &[(String, String)],
    ctx: RuntimeCtx,
) -> Result<()> {
    let registry = Registry::open()?;

    // Get jobs to show stats for
    let jobs = if let Some(id) = job_id {
        vec![registry.get_job(id)?]
    } else {
        registry.list_jobs(None, &[], None, None, tags, None, last)?
    };

    if jobs.is_empty() {
        println!("No jobs found.");
        return Ok(());
    }

    // Filter to remote jobs with slurm IDs (local jobs don't have sacct stats)
    let remote_jobs: Vec<_> = jobs
        .iter()
        .filter(|j| j.remote_host != "local" && j.slurm_id.is_some())
        .collect();

    if remote_jobs.is_empty() {
        println!("No remote Slurm jobs found. Stats are only available for Slurm jobs.");
        return Ok(());
    }

    // Print header
    println!(
        "{:<12} {:<10} {:<12} {:<12} {:<10} {}",
        style("JOB ID").bold(),
        style("STATUS").bold(),
        style("ELAPSED").bold(),
        style("CPU TIME").bold(),
        style("MAX MEM").bold(),
        style("RESOURCES").bold()
    );
    println!("{}", "-".repeat(80));

    for job in remote_jobs {
        let slurm_id = job
            .slurm_id
            .as_ref()
            .expect("already filtered to jobs with slurm_id");
        let ssh = ctx.ssh(&job.remote_host);

        match get_job_resource_usage(&ssh, slurm_id).await {
            Ok(usage) => {
                let status_styled = match job.status {
                    JobStatus::Completed => style(job.status.to_string()).green(),
                    JobStatus::Failed => style(job.status.to_string()).red(),
                    JobStatus::Cancelled => style(job.status.to_string()).yellow(),
                    JobStatus::Running => style(job.status.to_string()).cyan(),
                    JobStatus::Pending => style(job.status.to_string()).dim(),
                };

                // Parse allocated resources for cleaner display
                let resources = parse_alloc_tres(&usage.alloc_tres);

                println!(
                    "{:<12} {:<10} {:<12} {:<12} {:<10} {}",
                    truncate_id(&job.id),
                    status_styled,
                    if usage.elapsed.is_empty() {
                        "-".to_string()
                    } else {
                        usage.elapsed
                    },
                    if usage.total_cpu.is_empty() {
                        "-".to_string()
                    } else {
                        usage.total_cpu
                    },
                    if usage.max_rss.is_empty() {
                        "-".to_string()
                    } else {
                        usage.max_rss
                    },
                    resources
                );
            }
            Err(e) => {
                eprintln!(
                    "{:<12} {} ({})",
                    truncate_id(&job.id),
                    style("error").red(),
                    e
                );
            }
        }
    }

    Ok(())
}

// --- Private helper functions ---

/// Truncates a job ID for display (shows first 10 chars).
fn truncate_id(id: &str) -> &str {
    if id.len() <= 10 { id } else { &id[..10] }
}

/// Parses the `AllocTRES` string into a human-readable format.
///
/// Input: "billing=8,cpu=4,gres/gpu=1,mem=16G,node=1"
/// Output: "4 CPU, 1 GPU, 16G mem"
fn parse_alloc_tres(tres: &str) -> String {
    if tres.is_empty() {
        return "-".to_string();
    }

    let mut cpus = None;
    let mut gpus = None;
    let mut mem = None;

    for part in tres.split(',') {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or("");
        let value = kv.next().unwrap_or("");

        match key {
            "cpu" => cpus = Some(value.to_string()),
            "gres/gpu" => gpus = Some(value.to_string()),
            "mem" => mem = Some(value.to_string()),
            _ => {}
        }
    }

    let mut parts = Vec::new();
    if let Some(c) = cpus {
        parts.push(format!("{c} CPU"));
    }
    if let Some(g) = gpus {
        if g != "0" {
            parts.push(format!("{g} GPU"));
        }
    }
    if let Some(m) = mem {
        parts.push(format!("{m} mem"));
    }

    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_alloc_tres_cpu_mem_node() {
        // Real sacct output from job without GPU
        let tres = "cpu=1,mem=1000M,node=1";
        let result = parse_alloc_tres(tres);
        assert_eq!(result, "1 CPU, 1000M mem");
    }

    #[test]
    fn test_parse_alloc_tres_with_gpu() {
        // Real sacct output from GPU job
        let tres = "cpu=1,gres/gpu=1,node=1";
        let result = parse_alloc_tres(tres);
        assert_eq!(result, "1 CPU, 1 GPU");
    }

    #[test]
    fn test_parse_alloc_tres_full() {
        // Full format with billing
        let tres = "billing=8,cpu=4,gres/gpu=1,mem=16G,node=1";
        let result = parse_alloc_tres(tres);
        assert_eq!(result, "4 CPU, 1 GPU, 16G mem");
    }

    #[test]
    fn test_parse_alloc_tres_empty() {
        assert_eq!(parse_alloc_tres(""), "-");
    }

    #[test]
    fn test_parse_alloc_tres_zero_gpu() {
        // GPU=0 should be omitted from output
        let tres = "cpu=2,gres/gpu=0,mem=8G,node=1";
        let result = parse_alloc_tres(tres);
        assert_eq!(result, "2 CPU, 8G mem");
    }

    #[test]
    fn test_truncate_id_short() {
        assert_eq!(truncate_id("abc123"), "abc123");
    }

    #[test]
    fn test_truncate_id_exact() {
        assert_eq!(truncate_id("1234567890"), "1234567890");
    }

    #[test]
    fn test_truncate_id_long() {
        assert_eq!(truncate_id("train-abc12345-xyz"), "train-abc1");
    }
}
