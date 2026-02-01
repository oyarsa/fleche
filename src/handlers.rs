//! Command handlers for CLI commands with non-trivial logic.
//!
//! Most commands delegate directly to the [`job`] module. This module contains
//! handlers for commands that have additional logic beyond simple delegation.

use crate::config::{Config, ResolvedJob, generate_init_config};
use crate::registry::{JobStatus, Registry};
use crate::ssh::SshClient;
use anyhow::{Context, Result};
use console::style;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

/// Handles the `init` command - creates a starter fleche.toml.
pub fn init() -> Result<()> {
    let config_path = Path::new("fleche.toml");
    if config_path.exists() {
        anyhow::bail!("fleche.toml already exists in current directory");
    }

    std::fs::write(config_path, generate_init_config())
        .context("writing fleche.toml to current directory")?;
    println!("{} Created fleche.toml", style("✓").green());
    println!("Edit the file to configure your remote host and jobs.");
    Ok(())
}

/// Handles the `check` command - validates configuration.
pub fn check() -> Result<()> {
    let config = Config::find_and_load()?;

    println!("{} Configuration is valid", style("✓").green());
    println!();
    println!("  {:<14} {}", style("Project:").bold(), config.project_name);
    println!(
        "  {:<14} {}",
        style("Remote host:").bold(),
        config.remote.host
    );
    println!(
        "  {:<14} {}",
        style("Base path:").bold(),
        config.remote.base_path
    );
    println!(
        "  {:<14} {}",
        style("Config path:").bold(),
        config.project_path.join("fleche.toml").display()
    );

    let job_names = config.job_names();
    if job_names.is_empty() {
        println!();
        println!(
            "  {}",
            style("No jobs defined. Add jobs to fleche.toml or create fleche/*.toml files.")
                .yellow()
        );
    } else {
        println!();
        println!("  {}", style("Available jobs:").bold());
        for name in job_names {
            println!("    - {name}");
        }
    }

    Ok(())
}

/// Handles the `check --remote` command - validates configuration against the server.
pub async fn check_remote(debug: bool) -> Result<()> {
    let config = Config::find_and_load()?;

    // First show local config (same as regular check)
    println!("{} Configuration is valid", style("✓").green());
    println!();
    println!("  {:<14} {}", style("Project:").bold(), config.project_name);
    println!(
        "  {:<14} {}",
        style("Remote host:").bold(),
        config.remote.host
    );
    println!(
        "  {:<14} {}",
        style("Base path:").bold(),
        config.remote.base_path
    );

    println!();
    println!("{}", style("Remote Validation").bold().underlined());
    println!();

    let ssh = SshClient::new(&config.remote.host, debug);

    // 1. Check SSH connectivity with timing
    print!("  SSH connection... ");
    let _ = std::io::stdout().flush();
    let start = Instant::now();
    match ssh.exec("echo ok").await {
        Ok(_) => {
            let elapsed = start.elapsed();
            println!("{} ({}ms)", style("connected").green(), elapsed.as_millis());
        }
        Err(e) => {
            println!("{}", style("FAILED").red().bold());
            println!("    {e}");
            println!(
                "    {}",
                style("Check your SSH configuration and network connection").yellow()
            );
            return Ok(());
        }
    }

    // 2. Check Slurm availability
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

    // 3. Check configured partition (if any)
    if let Some(ref partition) = config.global_slurm.partition {
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

                // 4. Check constraint if configured
                if let Some(ref constraint) = config.global_slurm.constraint {
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
            }
            _ => {
                println!("{}", style("NOT FOUND").red().bold());
                // Try to list available partitions
                let cmd = "sinfo --noheader -o '%P' 2>/dev/null | sort -u | head -10";
                if let Ok((true, stdout, _)) = ssh.exec_allow_failure(cmd).await {
                    let partitions: Vec<&str> = stdout.lines().collect();
                    if !partitions.is_empty() {
                        println!("    Available partitions: {}", partitions.join(", "));
                    }
                }
            }
        }
    }

    // 5. Check base path is writable
    print!("  Base path writable... ");
    let _ = std::io::stdout().flush();
    let cmd = format!(
        "test -d {} && test -w {} && echo yes || echo no",
        &config.remote.base_path, &config.remote.base_path
    );
    match ssh.exec_allow_failure(&cmd).await {
        Ok((true, stdout, _)) if stdout.trim() == "yes" => {
            println!("{}", style("yes").green());
        }
        _ => {
            // Check if it's because directory doesn't exist (that's OK, we'll create it)
            let mkdir_cmd = format!(
                "mkdir -p {} && test -w {} && echo yes || echo no",
                &config.remote.base_path, &config.remote.base_path
            );
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

    // 6. Check disk space
    print!("  Disk space... ");
    let _ = std::io::stdout().flush();
    let cmd = format!("df -h {} 2>/dev/null | tail -1", &config.remote.base_path);
    match ssh.exec_allow_failure(&cmd).await {
        Ok((true, stdout, _)) if !stdout.trim().is_empty() => {
            let parts: Vec<&str> = stdout.split_whitespace().collect();
            if parts.len() >= 4 {
                let available = parts.get(3).unwrap_or(&"?");
                let use_percent = parts.get(4).unwrap_or(&"?%");
                let use_num: u32 = use_percent.trim_end_matches('%').parse().unwrap_or(0);

                if use_num >= 90 {
                    println!(
                        "{} ({} available, {} used)",
                        style("LOW").red().bold(),
                        available,
                        use_percent
                    );
                    println!(
                        "    {}",
                        style("Consider cleaning up old jobs with `fleche clean --older-than 30d`")
                            .yellow()
                    );
                } else if use_num >= 75 {
                    println!(
                        "{} ({} available, {} used)",
                        style("OK").yellow(),
                        available,
                        use_percent
                    );
                } else {
                    println!("{} ({} available)", style("OK").green(), available);
                }
            } else {
                println!("{}", style("could not parse").dim());
            }
        }
        _ => println!("{}", style("could not check").dim()),
    }

    println!();
    Ok(())
}

/// Handles the `doctor` command - comprehensive diagnostic for troubleshooting.
pub async fn doctor(debug: bool) -> Result<()> {
    use chrono::Duration;
    use std::process::Command;

    println!("{}", style("fleche doctor").bold().underlined());
    println!();

    let mut issues: Vec<String> = Vec::new();

    // 1. Check required tools
    println!("{}", style("Local Environment").bold());
    println!();

    print!("  ssh... ");
    let _ = std::io::stdout().flush();
    if Command::new("ssh").arg("-V").output().is_ok() {
        println!("{}", style("installed").green());
    } else {
        println!("{}", style("NOT FOUND").red().bold());
        issues.push("Install OpenSSH client".to_string());
    }

    print!("  rsync... ");
    let _ = std::io::stdout().flush();
    if Command::new("rsync").arg("--version").output().is_ok() {
        println!("{}", style("installed").green());
    } else {
        println!("{}", style("NOT FOUND").red().bold());
        issues.push("Install rsync".to_string());
    }

    println!();

    // 2. Check configuration
    println!("{}", style("Configuration").bold());
    println!();

    let config = match Config::find_and_load() {
        Ok(c) => {
            println!("  fleche.toml... {}", style("valid").green());
            println!("    Project: {}", c.project_name);
            println!("    Remote: {}:{}", c.remote.host, c.remote.base_path);
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
            None
        }
    };

    println!();

    // 3. Check registry
    println!("{}", style("Job Registry").bold());
    println!();

    match Registry::open() {
        Ok(registry) => {
            println!("  Database... {}", style("OK").green());

            // Count jobs by status
            let all_jobs = registry.list_jobs(None, &[], None, None, &[], None, 10000);
            let archived_jobs = registry.list_archived_jobs();

            if let Ok(jobs) = &all_jobs {
                let total = jobs.len();
                let pending = jobs
                    .iter()
                    .filter(|j| j.status == JobStatus::Pending)
                    .count();
                let running = jobs
                    .iter()
                    .filter(|j| j.status == JobStatus::Running)
                    .count();
                let completed = jobs
                    .iter()
                    .filter(|j| j.status == JobStatus::Completed)
                    .count();
                let failed = jobs
                    .iter()
                    .filter(|j| j.status == JobStatus::Failed)
                    .count();
                let cancelled = jobs
                    .iter()
                    .filter(|j| j.status == JobStatus::Cancelled)
                    .count();

                println!("  Total jobs: {total}");
                if pending > 0 || running > 0 {
                    println!(
                        "    Active: {} pending, {} running",
                        style(pending).cyan(),
                        style(running).green()
                    );
                }
                if completed > 0 || failed > 0 || cancelled > 0 {
                    println!(
                        "    Finished: {completed} completed, {failed} failed, {cancelled} cancelled"
                    );
                }

                // Check for stale running jobs (running for more than 7 days)
                let stale_running: Vec<_> = jobs
                    .iter()
                    .filter(|j| {
                        j.status == JobStatus::Running
                            && chrono::Utc::now().signed_duration_since(j.created_at)
                                > Duration::days(7)
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
                    issues.push(
                        "Check stale jobs with `fleche status` - they may be stuck".to_string(),
                    );
                }

                // Check for old jobs that could be cleaned
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
                        issues.push(
                            "Consider `fleche clean --older-than 30d` to clean old jobs"
                                .to_string(),
                        );
                    }
                }
            }

            if let Ok(archived) = archived_jobs {
                if !archived.is_empty() {
                    println!("  Archived: {}", archived.len());
                }
            }
        }
        Err(e) => {
            println!("  Database... {}", style("ERROR").red().bold());
            println!("    {e}");
            issues.push(format!("Database error: {e}"));
        }
    }

    println!();

    // 4. Remote validation (only if we have a config)
    if let Some(ref config) = config {
        println!("{}", style("Remote Connection").bold());
        println!();

        let ssh = SshClient::new(&config.remote.host, debug);

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
                print!("  Slurm controller... ");
                let _ = std::io::stdout().flush();
                if let Ok((true, stdout, _)) =
                    ssh.exec_allow_failure("scontrol ping 2>/dev/null").await
                {
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

                // Check disk space
                print!("  Disk space... ");
                let _ = std::io::stdout().flush();
                let cmd = format!("df -h {} 2>/dev/null | tail -1", &config.remote.base_path);
                if let Ok((true, stdout, _)) = ssh.exec_allow_failure(&cmd).await {
                    let parts: Vec<&str> = stdout.split_whitespace().collect();
                    if parts.len() >= 5 {
                        let available = parts.get(3).unwrap_or(&"?");
                        let use_percent = parts.get(4).unwrap_or(&"?%");
                        let use_num: u32 = use_percent.trim_end_matches('%').parse().unwrap_or(0);

                        if use_num >= 90 {
                            println!(
                                "{} ({} available)",
                                style("CRITICAL").red().bold(),
                                available
                            );
                            issues.push(format!(
                                "Disk space critically low ({use_num}%) - run `fleche clean --older-than 7d`"
                            ));
                        } else if use_num >= 75 {
                            println!(
                                "{} ({} available, {}% used)",
                                style("OK").yellow(),
                                available,
                                use_num
                            );
                        } else {
                            println!("{} ({} available)", style("OK").green(), available);
                        }
                    } else {
                        println!("{}", style("could not parse").dim());
                    }
                } else {
                    println!("{}", style("could not check").dim());
                }
            }
            Err(e) => {
                println!("{}", style("FAILED").red().bold());
                println!("    {e}");
                issues.push(format!("SSH connection failed: {e}"));
            }
        }

        println!();
    }

    // 5. Summary
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

    println!();
    Ok(())
}

/// Handles the `compare` command - shows differences between two jobs.
pub fn compare_jobs(first_id: &str, second_id: &str) -> Result<()> {
    let registry = Registry::open()?;
    let job_a = registry.get_job(first_id)?;
    let job_b = registry.get_job(second_id)?;

    // Parse the resolved job configs
    let config_a: ResolvedJob =
        serde_json::from_str(&job_a.config_json).context("parsing config for first job")?;
    let config_b: ResolvedJob =
        serde_json::from_str(&job_b.config_json).context("parsing config for second job")?;

    println!(
        "{} {} vs {}",
        style("Comparing").bold(),
        style(&job_a.id).cyan(),
        style(&job_b.id).cyan()
    );
    println!();

    // Header with job IDs
    let col_width = 35;
    println!(
        "  {:<15} {:>col_width$}  {:>col_width$}",
        "",
        style(&job_a.id).dim(),
        style(&job_b.id).dim()
    );
    println!("  {}", "-".repeat(15 + col_width * 2 + 4));

    // Compare basic fields
    compare_field("Job name", &config_a.name, &config_b.name, col_width);
    compare_field("Command", &config_a.command, &config_b.command, col_width);
    compare_field("Host", &config_a.host, &config_b.host, col_width);
    compare_field(
        "Status",
        &format!("{:?}", job_a.status),
        &format!("{:?}", job_b.status),
        col_width,
    );

    // Compare Slurm settings
    println!();
    println!("  {}", style("Slurm Settings").bold());
    compare_option_field(
        "Partition",
        config_a.slurm.partition.as_deref(),
        config_b.slurm.partition.as_deref(),
        col_width,
    );
    compare_option_field(
        "Time",
        config_a.slurm.time.as_deref(),
        config_b.slurm.time.as_deref(),
        col_width,
    );
    compare_option_u32("GPUs", config_a.slurm.gpus, config_b.slurm.gpus, col_width);
    compare_option_u32("CPUs", config_a.slurm.cpus, config_b.slurm.cpus, col_width);
    compare_option_field(
        "Memory",
        config_a.slurm.memory.as_deref(),
        config_b.slurm.memory.as_deref(),
        col_width,
    );
    compare_option_field(
        "Constraint",
        config_a.slurm.constraint.as_deref(),
        config_b.slurm.constraint.as_deref(),
        col_width,
    );
    compare_option_u32(
        "Nodes",
        config_a.slurm.nodes,
        config_b.slurm.nodes,
        col_width,
    );
    compare_option_field(
        "Exclude",
        config_a.slurm.exclude.as_deref(),
        config_b.slurm.exclude.as_deref(),
        col_width,
    );

    // Compare environment variables
    let all_env_keys: std::collections::BTreeSet<_> =
        config_a.env.keys().chain(config_b.env.keys()).collect();

    if !all_env_keys.is_empty() {
        println!();
        println!("  {}", style("Environment").bold());
        for key in all_env_keys {
            let val_a = config_a.env.get(key).map(String::as_str);
            let val_b = config_b.env.get(key).map(String::as_str);
            compare_option_field(key, val_a, val_b, col_width);
        }
    }

    // Compare tags
    let all_tag_keys: std::collections::BTreeSet<_> =
        job_a.tags.keys().chain(job_b.tags.keys()).collect();

    if !all_tag_keys.is_empty() {
        println!();
        println!("  {}", style("Tags").bold());
        for key in all_tag_keys {
            let val_a = job_a.tags.get(key).map(String::as_str);
            let val_b = job_b.tags.get(key).map(String::as_str);
            compare_option_field(key, val_a, val_b, col_width);
        }
    }

    // Compare notes
    if job_a.note.is_some() || job_b.note.is_some() {
        println!();
        println!("  {}", style("Notes").bold());
        compare_option_field(
            "Note",
            job_a.note.as_deref(),
            job_b.note.as_deref(),
            col_width,
        );
    }

    // Compare inputs/outputs if different
    if config_a.inputs != config_b.inputs {
        println!();
        println!("  {}", style("Inputs").bold());
        println!(
            "    A: {}",
            if config_a.inputs.is_empty() {
                "(none)".to_string()
            } else {
                config_a.inputs.join(", ")
            }
        );
        println!(
            "    B: {}",
            if config_b.inputs.is_empty() {
                "(none)".to_string()
            } else {
                config_b.inputs.join(", ")
            }
        );
    }

    if config_a.outputs != config_b.outputs {
        println!();
        println!("  {}", style("Outputs").bold());
        println!(
            "    A: {}",
            if config_a.outputs.is_empty() {
                "(none)".to_string()
            } else {
                config_a.outputs.join(", ")
            }
        );
        println!(
            "    B: {}",
            if config_b.outputs.is_empty() {
                "(none)".to_string()
            } else {
                config_b.outputs.join(", ")
            }
        );
    }

    println!();
    Ok(())
}

/// Compares two string values and prints them with highlighting for differences.
fn compare_field(label: &str, val_a: &str, val_b: &str, col_width: usize) {
    let truncated_a = truncate_str(val_a, col_width);
    let truncated_b = truncate_str(val_b, col_width);

    if val_a == val_b {
        println!(
            "  {:<15} {:>col_width$}  {:>col_width$}",
            style(label).dim(),
            style(&truncated_a).dim(),
            style(&truncated_b).dim()
        );
    } else {
        println!(
            "  {:<15} {:>col_width$}  {:>col_width$}",
            style(label).bold(),
            style(&truncated_a).red(),
            style(&truncated_b).green()
        );
    }
}

/// Compares two optional string values.
fn compare_option_field(label: &str, val_a: Option<&str>, val_b: Option<&str>, col_width: usize) {
    let str_a = val_a.unwrap_or("-");
    let str_b = val_b.unwrap_or("-");
    compare_field(label, str_a, str_b, col_width);
}

/// Compares two optional u32 values.
fn compare_option_u32(label: &str, val_a: Option<u32>, val_b: Option<u32>, col_width: usize) {
    let str_a = val_a.map_or("-".to_string(), |v| v.to_string());
    let str_b = val_b.map_or("-".to_string(), |v| v.to_string());
    compare_field(label, &str_a, &str_b, col_width);
}

/// Truncates a string to fit within the specified width.
fn truncate_str(s: &str, max_width: usize) -> String {
    if s.len() <= max_width {
        s.to_string()
    } else {
        format!("{}…", &s[..max_width - 1])
    }
}
