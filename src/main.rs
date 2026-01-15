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
use config::{generate_init_config, Config};
use console::style;
use slurm::slurm_config_from_cli;
use std::path::Path;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{} {}", style("Error:").red().bold(), e);
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            job_name,
            command,
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
            follow,
            dry_run,
        } => {
            let config = Config::find_and_load()?;
            let slurm_overrides =
                slurm_config_from_cli(partition, time, gpus, cpus, memory, constraint, nodes, exclude);

            job::run_job(
                &config,
                job_name.as_deref(),
                command.as_deref(),
                &env_vars,
                &tags,
                slurm_overrides,
                follow,
                dry_run,
            )
            .await?;
        }

        Commands::Status { job_id } => {
            job::show_status(job_id.as_deref()).await?;
        }

        Commands::Logs {
            job_id,
            follow,
            stderr,
            both,
        } => {
            job::show_logs(&job_id, follow, stderr, both).await?;
        }

        Commands::Sync { job_id, partial } => {
            job::sync_outputs(&job_id, partial).await?;
        }

        Commands::List {
            project,
            status,
            tags,
            failed,
            running,
            completed,
        } => {
            job::list_jobs(
                project.as_deref(),
                status.as_deref(),
                &tags,
                failed,
                running,
                completed,
            )?;
        }

        Commands::Cancel { job_id } => {
            job::cancel_slurm_job(&job_id).await?;
        }

        Commands::Clean {
            job_id,
            all,
            older_than,
        } => {
            job::clean_job(job_id.as_deref(), all, older_than.as_deref()).await?;
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
            println!(
                "{} Created fleche.toml",
                style("✓").green()
            );
            println!("Edit the file to configure your remote host and jobs.");
        }

        Commands::Check => {
            match Config::find_and_load() {
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
            }
        }

        Commands::Guide => {
            println!("{}", guide::GUIDE_TEXT);
        }
    }

    Ok(())
}
