//! fleche - Remote job runner for Slurm clusters.
//!
//! This is the main entry point for the fleche CLI tool. It parses command-line
//! arguments and dispatches to the appropriate handler in the [`job`] module.
//!
//! # Architecture
//!
//! The codebase is organized into the following modules:
//!
//! - [`cli`]: Command-line argument parsing using clap
//! - [`config`]: Configuration file parsing and job resolution
//! - [`error`]: Error types and result aliases
//! - [`guide`]: Built-in usage guide text
//! - [`job`]: High-level job operations (run, status, logs, sync, etc.)
//! - [`registry`]: Local `SQLite` database for tracking submitted jobs
//! - [`slurm`]: Slurm-specific operations (sbatch generation, status queries)
//! - [`ssh`]: SSH client for remote command execution
//! - [`sync`]: File synchronization using rsync

#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::format_push_string
)]

mod cli;
mod config;
mod error;
mod guide;
mod job;
mod registry;
mod slurm;
mod ssh;
mod sync;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use config::{Config, generate_init_config};
use console::style;
use slurm::slurm_config_from_cli;
use std::path::Path;

/// Entry point for the fleche CLI.
///
/// Calls [`run`] and handles any errors by printing them to stderr and exiting
/// with a non-zero status code.
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{} {}", style("Error:").red().bold(), e);
        std::process::exit(1);
    }
}

/// Parses CLI arguments and dispatches to the appropriate command handler.
///
/// This function is the main dispatcher for all fleche subcommands. Each command
/// is handled by calling the corresponding function in the [`job`] module, except
/// for `init`, `check`, and `guide` which are handled inline.
async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            job_or_command,
            command,
            bg,
            env_vars,
            tags,
            partition,
            time,
            gpus,
            cpus,
            memory,
            constraint,
            nodes,
            exclude,
            dry_run,
        } => {
            let config = Config::find_and_load()?;
            let slurm_overrides = slurm_config_from_cli(
                partition, time, gpus, cpus, memory, constraint, nodes, exclude,
            );

            job::run_job(
                &config,
                job_or_command.as_deref(),
                command.as_deref(),
                &env_vars,
                &tags,
                slurm_overrides,
                bg,
                dry_run,
                cli.debug,
            )
            .await?;
        }

        Commands::Exec { command, env_vars } => {
            let config = Config::find_and_load()?;
            job::exec_command(&config, &command, &env_vars, cli.debug).await?;
        }

        Commands::Status {
            job_id,
            filter,
            tags,
        } => {
            job::show_status(job_id.as_deref(), filter.as_deref(), &tags, cli.debug).await?;
        }

        Commands::Logs {
            job_id,
            follow,
            stdout,
            stderr,
            tail,
            tags,
        } => {
            job::show_logs(
                job_id.as_deref(),
                follow,
                stdout,
                stderr,
                tail,
                &tags,
                cli.debug,
            )
            .await?;
        }

        Commands::Download {
            job_id,
            partial,
            path,
            tags,
        } => {
            job::download_outputs(
                job_id.as_deref(),
                partial,
                path.as_deref(),
                &tags,
                cli.debug,
            )
            .await?;
        }

        Commands::Cancel {
            job_id,
            all,
            yes,
            tags,
        } => {
            job::cancel_jobs(job_id.as_deref(), all, yes, &tags, cli.debug).await?;
        }

        Commands::Clean {
            job_id,
            all,
            older_than,
            workspace,
            yes,
            tags,
        } => {
            job::clean_jobs(
                job_id.as_deref(),
                all,
                older_than.as_deref(),
                workspace,
                yes,
                &tags,
                cli.debug,
            )
            .await?;
        }

        Commands::Init => {
            let config_path = Path::new("fleche.toml");
            if config_path.exists() {
                eprintln!(
                    "{} fleche.toml already exists in current directory",
                    style("Error:").red().bold()
                );
                std::process::exit(1);
            }

            std::fs::write(config_path, generate_init_config())?;
            println!("{} Created fleche.toml", style("✓").green());
            println!("Edit the file to configure your remote host and jobs.");
        }

        Commands::Check => match Config::find_and_load() {
            Ok(config) => {
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
            }
            Err(e) => {
                eprintln!("{} {}", style("✗").red(), e);
                std::process::exit(1);
            }
        },

        Commands::Guide => {
            println!("{}", guide::GUIDE_TEXT);
        }
    }

    Ok(())
}
