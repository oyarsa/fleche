//! Diagnostic and validation utilities.
//!
//! This module contains functions for validating configuration against remote
//! servers and comprehensive troubleshooting diagnostics.

use crate::config::Config;
use crate::registry::{JobStatus, Registry};
use crate::ssh::SshClient;
use anyhow::Result;
use chrono::Duration;
use console::style;
use std::io::Write;
use std::time::Instant;

/// Validates configuration against the remote server.
pub async fn check_remote(config: &Config, debug: bool) -> Result<()> {
    println!("{}", style("Remote Validation").bold().underlined());
    println!();

    let ssh = SshClient::new(&config.remote.host, debug);

    // Check SSH connectivity
    if !check_ssh_connection(&ssh).await {
        return Ok(());
    }

    // Check Slurm controller
    check_slurm_controller(&ssh).await;

    // Check partition and constraints
    if let Some(ref partition) = config.global_slurm.partition {
        check_partition(&ssh, partition, config.global_slurm.constraint.as_deref()).await;
    }

    // Check base path
    check_base_path(&ssh, &config.remote.base_path).await;

    // Check disk space
    check_disk_space(&ssh, &config.remote.base_path).await;

    println!();
    Ok(())
}

async fn check_ssh_connection(ssh: &SshClient) -> bool {
    print!("  SSH connection... ");
    let _ = std::io::stdout().flush();

    let start = Instant::now();
    match ssh.exec("echo ok").await {
        Ok(_) => {
            let elapsed = start.elapsed();
            println!("{} ({}ms)", style("connected").green(), elapsed.as_millis());
            true
        }
        Err(e) => {
            println!("{}", style("FAILED").red().bold());
            println!("    {e}");
            println!(
                "    {}",
                style("Check your SSH configuration and network connection").yellow()
            );
            false
        }
    }
}

async fn check_slurm_controller(ssh: &SshClient) {
    print!("  Slurm controller... ");
    let _ = std::io::stdout().flush();

    if let Ok((true, stdout, _)) = ssh.exec_allow_failure("scontrol ping 2>/dev/null").await {
        if stdout.contains("is UP") {
            println!("{}", style("responding").green());
        } else if stdout.contains("is DOWN") {
            println!("{}", style("DOWN").red().bold());
            println!(
                "    {}",
                style("The Slurm controller is down - jobs may fail").yellow()
            );
        } else {
            println!("{}", style("responding").green());
        }
    } else {
        println!("{}", style("not available").yellow());
        println!(
            "    {}",
            style("Slurm may not be installed or accessible on this host").dim()
        );
    }
}

async fn check_partition(ssh: &SshClient, partition: &str, constraint: Option<&str>) {
    print!("  Partition '{partition}'... ");
    let _ = std::io::stdout().flush();

    let cmd = format!("sinfo -p {partition} --noheader 2>/dev/null | head -1");
    match ssh.exec_allow_failure(&cmd).await {
        Ok((true, stdout, _)) if !stdout.trim().is_empty() => {
            // Parse sinfo output for node count
            let parts: Vec<&str> = stdout.split_whitespace().collect();
            if parts.len() >= 4 {
                let nodes = parts.get(3).unwrap_or(&"?");
                println!("{} ({} nodes)", style("exists").green(), nodes);
            } else {
                println!("{}", style("exists").green());
            }

            // Check constraint if configured
            if let Some(constraint) = constraint {
                check_constraint(ssh, partition, constraint).await;
            }
        }
        _ => {
            println!("{}", style("NOT FOUND").red().bold());
            list_available_partitions(ssh).await;
        }
    }
}

async fn check_constraint(ssh: &SshClient, partition: &str, constraint: &str) {
    print!("  Constraint '{constraint}'... ");
    let _ = std::io::stdout().flush();

    let cmd = format!(
        "sinfo -p {partition} -o '%f' --noheader 2>/dev/null | sort -u | tr '\\n' ',' | sed 's/,$//'"
    );

    match ssh.exec_allow_failure(&cmd).await {
        Ok((true, stdout, _)) => {
            let features: Vec<&str> = stdout
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();

            if features.iter().any(|f| f.contains(constraint)) {
                println!("{}", style("valid").green());
            } else {
                println!("{}", style("NOT FOUND").red().bold());
                if features.is_empty() {
                    println!(
                        "    {}",
                        style("No features available on this partition").yellow()
                    );
                } else {
                    println!("    Available features: {}", features.join(", "));
                }
            }
        }
        _ => println!("{}", style("could not check").dim()),
    }
}

async fn list_available_partitions(ssh: &SshClient) {
    let cmd = "sinfo --noheader -o '%P' 2>/dev/null | sort -u | head -10";
    if let Ok((true, stdout, _)) = ssh.exec_allow_failure(cmd).await {
        let partitions: Vec<&str> = stdout.lines().collect();
        if !partitions.is_empty() {
            println!("    Available partitions: {}", partitions.join(", "));
        }
    }
}

async fn check_base_path(ssh: &SshClient, base_path: &str) {
    print!("  Base path writable... ");
    let _ = std::io::stdout().flush();

    let cmd = format!("test -d {base_path} && test -w {base_path} && echo yes || echo no");

    match ssh.exec_allow_failure(&cmd).await {
        Ok((true, stdout, _)) if stdout.trim() == "yes" => {
            println!("{}", style("yes").green());
        }
        _ => {
            // Try creating the directory
            let mkdir_cmd =
                format!("mkdir -p {base_path} && test -w {base_path} && echo yes || echo no");
            match ssh.exec_allow_failure(&mkdir_cmd).await {
                Ok((true, stdout, _)) if stdout.trim() == "yes" => {
                    println!("{} (created)", style("yes").green());
                }
                _ => {
                    println!("{}", style("NO").red().bold());
                    println!(
                        "    {}",
                        style("Cannot write to base path - check permissions").yellow()
                    );
                }
            }
        }
    }
}

async fn check_disk_space(ssh: &SshClient, base_path: &str) {
    print!("  Disk space... ");
    let _ = std::io::stdout().flush();

    let cmd = format!("df -h {base_path} 2>/dev/null | tail -1");

    match ssh.exec_allow_failure(&cmd).await {
        Ok((true, stdout, _)) if !stdout.trim().is_empty() => {
            if let Some(result) = parse_disk_usage(&stdout) {
                print_disk_status(&result);
            } else {
                println!("{}", style("could not parse").dim());
            }
        }
        _ => println!("{}", style("could not check").dim()),
    }
}

struct DiskUsage {
    available: String,
    use_percent: u32,
}

fn parse_disk_usage(df_output: &str) -> Option<DiskUsage> {
    let parts: Vec<&str> = df_output.split_whitespace().collect();
    if parts.len() >= 5 {
        let available = parts.get(3)?.to_string();
        let use_percent = parts.get(4)?.trim_end_matches('%').parse().ok()?;
        Some(DiskUsage {
            available,
            use_percent,
        })
    } else {
        None
    }
}

fn print_disk_status(usage: &DiskUsage) {
    if usage.use_percent >= 90 {
        println!(
            "{} ({} available, {}% used)",
            style("LOW").red().bold(),
            usage.available,
            usage.use_percent
        );
        println!(
            "    {}",
            style("Consider cleaning up old jobs with `fleche clean --older-than 30d`").yellow()
        );
    } else if usage.use_percent >= 75 {
        println!(
            "{} ({} available, {}% used)",
            style("OK").yellow(),
            usage.available,
            usage.use_percent
        );
    } else {
        println!("{} ({} available)", style("OK").green(), usage.available);
    }
}

// =============================================================================
// doctor implementation
// =============================================================================

/// Runs comprehensive diagnostics for troubleshooting.
pub async fn doctor(debug: bool) -> Result<()> {
    println!("{}", style("fleche doctor").bold().underlined());
    println!();

    let mut issues: Vec<String> = Vec::new();

    // Check local environment
    check_local_environment(&mut issues);

    // Check configuration
    let config = check_configuration(&mut issues);

    // Check job registry
    check_registry(&mut issues);

    // Check remote connection (if config exists)
    if let Some(ref config) = config {
        check_remote_connection(config, debug, &mut issues).await;
    }

    // Print summary
    print_issues_summary(&issues);

    println!();
    Ok(())
}

fn check_local_environment(issues: &mut Vec<String>) {
    use std::process::Command;

    println!("{}", style("Local Environment").bold());
    println!();

    // Check ssh
    print!("  ssh... ");
    let _ = std::io::stdout().flush();
    if Command::new("ssh").arg("-V").output().is_ok() {
        println!("{}", style("installed").green());
    } else {
        println!("{}", style("NOT FOUND").red().bold());
        issues.push("Install OpenSSH client".to_string());
    }

    // Check rsync
    print!("  rsync... ");
    let _ = std::io::stdout().flush();
    if Command::new("rsync").arg("--version").output().is_ok() {
        println!("{}", style("installed").green());
    } else {
        println!("{}", style("NOT FOUND").red().bold());
        issues.push("Install rsync".to_string());
    }

    println!();
}

fn check_configuration(issues: &mut Vec<String>) -> Option<Config> {
    println!("{}", style("Configuration").bold());
    println!();

    match Config::find_and_load() {
        Ok(c) => {
            println!("  fleche.toml... {}", style("valid").green());
            println!("    Project: {}", c.project_name);
            println!("    Remote: {}:{}", c.remote.host, c.remote.base_path);
            println!();
            Some(c)
        }
        Err(e) => {
            let err_msg = format!("{e}");
            if err_msg.contains("not found") {
                println!("  fleche.toml... {}", style("NOT FOUND").yellow());
                println!("    Run `fleche init` to create a configuration file");
            } else {
                println!("  fleche.toml... {}", style("INVALID").red().bold());
                println!("    {e}");
                issues.push(format!("Fix configuration: {e}"));
            }
            println!();
            None
        }
    }
}

fn check_registry(issues: &mut Vec<String>) {
    println!("{}", style("Job Registry").bold());
    println!();

    match Registry::open() {
        Ok(registry) => {
            println!("  Database... {}", style("OK").green());
            check_job_statistics(&registry, issues);
        }
        Err(e) => {
            println!("  Database... {}", style("ERROR").red().bold());
            println!("    {e}");
            issues.push(format!("Database error: {e}"));
        }
    }

    println!();
}

fn check_job_statistics(registry: &Registry, issues: &mut Vec<String>) {
    let all_jobs = registry.list_jobs(None, &[], None, None, &[], None, 10000);
    let archived_jobs = registry.list_archived_jobs();

    if let Ok(jobs) = &all_jobs {
        let stats = JobStats::from_jobs(jobs);
        println!("  Total jobs: {}", stats.total);

        if stats.pending > 0 || stats.running > 0 {
            println!(
                "    Active: {} pending, {} running",
                style(stats.pending).cyan(),
                style(stats.running).green()
            );
        }

        if stats.completed > 0 || stats.failed > 0 || stats.cancelled > 0 {
            println!(
                "    Finished: {} completed, {} failed, {} cancelled",
                stats.completed, stats.failed, stats.cancelled
            );
        }

        // Check for stale jobs
        check_stale_jobs(jobs, issues);

        // Check for cleanable jobs
        check_cleanable_jobs(registry, issues);
    }

    if let Ok(archived) = archived_jobs {
        if !archived.is_empty() {
            println!("  Archived: {}", archived.len());
        }
    }
}

struct JobStats {
    total: usize,
    pending: usize,
    running: usize,
    completed: usize,
    failed: usize,
    cancelled: usize,
}

impl JobStats {
    fn from_jobs(jobs: &[crate::registry::JobRecord]) -> Self {
        Self {
            total: jobs.len(),
            pending: jobs
                .iter()
                .filter(|j| j.status == JobStatus::Pending)
                .count(),
            running: jobs
                .iter()
                .filter(|j| j.status == JobStatus::Running)
                .count(),
            completed: jobs
                .iter()
                .filter(|j| j.status == JobStatus::Completed)
                .count(),
            failed: jobs
                .iter()
                .filter(|j| j.status == JobStatus::Failed)
                .count(),
            cancelled: jobs
                .iter()
                .filter(|j| j.status == JobStatus::Cancelled)
                .count(),
        }
    }
}

fn check_stale_jobs(jobs: &[crate::registry::JobRecord], issues: &mut Vec<String>) {
    let stale_running: Vec<_> = jobs
        .iter()
        .filter(|j| {
            j.status == JobStatus::Running
                && chrono::Utc::now().signed_duration_since(j.created_at) > Duration::days(7)
        })
        .collect();

    if !stale_running.is_empty() {
        println!();
        println!(
            "  {} {} job(s) running for over 7 days:",
            style("⚠").yellow(),
            stale_running.len()
        );
        for job in stale_running.iter().take(3) {
            println!(
                "    - {} (started {})",
                job.id,
                job.created_at.format("%Y-%m-%d")
            );
        }
        issues.push("Check stale jobs with `fleche status` - they may be stuck".to_string());
    }
}

fn check_cleanable_jobs(registry: &Registry, issues: &mut Vec<String>) {
    if let Ok(old_jobs) = registry.list_jobs_older_than(Duration::days(30)) {
        let cleanable: Vec<_> = old_jobs
            .iter()
            .filter(|j| {
                matches!(
                    j.status,
                    JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
                )
            })
            .collect();

        if cleanable.len() > 10 {
            println!();
            println!(
                "  {} {} jobs older than 30 days could be cleaned",
                style("ℹ").blue(),
                cleanable.len()
            );
            issues.push("Consider `fleche clean --older-than 30d` to clean old jobs".to_string());
        }
    }
}

async fn check_remote_connection(config: &Config, debug: bool, issues: &mut Vec<String>) {
    println!("{}", style("Remote Connection").bold());
    println!();

    let ssh = SshClient::new(&config.remote.host, debug);

    // Check SSH
    print!("  SSH connection... ");
    let _ = std::io::stdout().flush();

    let start = Instant::now();
    match ssh.exec("echo ok").await {
        Ok(_) => {
            let elapsed = start.elapsed();
            if elapsed.as_millis() > 5000 {
                println!(
                    "{} ({}ms - {})",
                    style("slow").yellow(),
                    elapsed.as_millis(),
                    style("connection is slow").dim()
                );
                issues.push("SSH connection is slow - check network or SSH config".to_string());
            } else {
                println!("{} ({}ms)", style("OK").green(), elapsed.as_millis());
            }

            // Check Slurm
            check_slurm_for_doctor(&ssh, issues).await;

            // Check disk space
            check_disk_for_doctor(&ssh, &config.remote.base_path, issues).await;
        }
        Err(e) => {
            println!("{}", style("FAILED").red().bold());
            println!("    {e}");
            issues.push(format!("SSH connection failed: {e}"));
        }
    }

    println!();
}

async fn check_slurm_for_doctor(ssh: &SshClient, issues: &mut Vec<String>) {
    print!("  Slurm controller... ");
    let _ = std::io::stdout().flush();

    if let Ok((true, stdout, _)) = ssh.exec_allow_failure("scontrol ping 2>/dev/null").await {
        if stdout.contains("is UP") {
            println!("{}", style("UP").green());
        } else if stdout.contains("is DOWN") {
            println!("{}", style("DOWN").red().bold());
            issues.push("Slurm controller is down".to_string());
        } else {
            println!("{}", style("responding").green());
        }
    } else {
        println!("{}", style("not available").yellow());
    }
}

async fn check_disk_for_doctor(ssh: &SshClient, base_path: &str, issues: &mut Vec<String>) {
    print!("  Disk space... ");
    let _ = std::io::stdout().flush();

    let cmd = format!("df -h {base_path} 2>/dev/null | tail -1");

    if let Ok((true, stdout, _)) = ssh.exec_allow_failure(&cmd).await {
        if let Some(usage) = parse_disk_usage(&stdout) {
            if usage.use_percent >= 90 {
                println!(
                    "{} ({} available)",
                    style("CRITICAL").red().bold(),
                    usage.available
                );
                issues.push(format!(
                    "Disk space critically low ({}%) - run `fleche clean --older-than 7d`",
                    usage.use_percent
                ));
            } else if usage.use_percent >= 75 {
                println!(
                    "{} ({} available, {}% used)",
                    style("OK").yellow(),
                    usage.available,
                    usage.use_percent
                );
            } else {
                println!("{} ({} available)", style("OK").green(), usage.available);
            }
        } else {
            println!("{}", style("could not parse").dim());
        }
    } else {
        println!("{}", style("could not check").dim());
    }
}

fn print_issues_summary(issues: &[String]) {
    if issues.is_empty() {
        println!(
            "{} {}",
            style("✓").green().bold(),
            style("No issues found").green()
        );
    } else {
        println!(
            "{} {} issue(s) found:",
            style("⚠").yellow().bold(),
            issues.len()
        );
        println!();
        for (i, issue) in issues.iter().enumerate() {
            println!("  {}. {}", i + 1, issue);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_disk_usage_standard() {
        // Standard df output
        let output = "/dev/sda1 100G 50G 50G 50% /home";
        let usage = parse_disk_usage(output).unwrap();
        assert_eq!(usage.available, "50G");
        assert_eq!(usage.use_percent, 50);
    }

    #[test]
    fn test_parse_disk_usage_cephfs() {
        // Real output from cephfs with multiple IPs
        let output = "172.17.22.11:3300,172.17.22.12:3300,172.17.22.13:3300,172.17.22.19:3300,172.17.22.20:3300:/   20P  7.3P   13P  38% /cephfs";
        let usage = parse_disk_usage(output).unwrap();
        assert_eq!(usage.available, "13P");
        assert_eq!(usage.use_percent, 38);
    }

    #[test]
    fn test_parse_disk_usage_high_usage() {
        let output = "/dev/sda1 100G 95G 5G 95% /home";
        let usage = parse_disk_usage(output).unwrap();
        assert_eq!(usage.available, "5G");
        assert_eq!(usage.use_percent, 95);
    }

    #[test]
    fn test_parse_disk_usage_incomplete() {
        // Not enough fields
        let output = "/dev/sda1 100G";
        assert!(parse_disk_usage(output).is_none());
    }
}
