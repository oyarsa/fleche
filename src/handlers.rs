//! Command handlers for CLI commands with non-trivial logic.
//!
//! Most commands delegate directly to the [`job`] module. This module contains
//! handlers for commands that have additional logic beyond simple delegation.

use crate::config::{Config, generate_init_config};
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
