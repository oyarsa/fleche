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

mod cli;
mod config;
mod error;
mod guide;
mod handlers;
mod job;
mod local;
mod registry;
mod slurm;
mod ssh;
mod sync;

use anyhow::Result;
use clap::{CommandFactory, Parser};
use cli::{Cli, Commands};
use config::Config;
use console::style;
use slurm::slurm_config_from_cli;

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

/// Checks that required external tools are available.
fn check_dependencies() -> Result<()> {
    use std::process::Command;

    if Command::new("ssh").arg("-V").output().is_err() {
        anyhow::bail!(
            "ssh not found. Install it with:\n  \
             macOS:  Pre-installed (check /usr/bin/ssh)\n  \
             Ubuntu: apt install openssh-client\n  \
             Windows: Install Git Bash, WSL, or OpenSSH"
        );
    }

    if Command::new("rsync").arg("--version").output().is_err() {
        anyhow::bail!(
            "rsync not found. Install it with:\n  \
             macOS:  brew install rsync\n  \
             Ubuntu: apt install rsync\n  \
             Fedora: dnf install rsync"
        );
    }
    Ok(())
}

/// Parses CLI arguments and dispatches to the appropriate command handler.
///
/// This function is the main dispatcher for all fleche subcommands. Each command
/// is handled by calling the corresponding function in the [`job`] module, except
/// for `init`, `check`, and `guide` which are handled inline.
async fn run() -> Result<()> {
    let cli = Cli::parse();

    check_dependencies()?;

    // Change to specified directory if -C/--directory was provided
    if let Some(ref dir) = cli.directory {
        std::env::set_current_dir(dir).map_err(|e| {
            anyhow::anyhow!("Cannot change to directory '{}': {}", dir.display(), e)
        })?;
    }

    match cli.command {
        Commands::Run {
            job_or_command,
            command,
            bg,
            notify,
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
            after,
            dry_run,
            host,
            retry,
            note,
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
                host.as_deref(),
                job::RunJobOptions {
                    background: bg,
                    notify,
                    dry_run,
                    debug: cli.debug,
                    after,
                    retry,
                    note,
                },
            )
            .await?;
        }

        Commands::Exec {
            command,
            env_vars,
            host,
        } => {
            let config = Config::find_and_load()?;
            job::exec_command(&config, &command, &env_vars, host.as_deref(), cli.debug).await?;
        }

        Commands::Status {
            job_id,
            filter,
            name,
            tags,
            last,
            archived,
            all_jobs,
        } => {
            // Determine archived filter based on flags
            let archived_filter = if archived {
                Some(true) // Show only archived
            } else if all_jobs {
                Some(false) // Show all (both archived and non-archived)
            } else {
                None // Default: show only non-archived
            };

            // Try to load config for settings, but don't fail if not in a project
            let default_limit = Config::find_and_load()
                .ok()
                .map(|c| c.settings.default_list_limit);

            job::show_status(
                job_id.as_deref(),
                &filter,
                name.as_deref(),
                &tags,
                last,
                default_limit,
                archived_filter,
                cli.debug,
            )
            .await?;
        }

        Commands::Logs {
            job_id,
            follow,
            stdout,
            stderr,
            tail,
            raw,
            tags,
            note,
        } => {
            job::show_logs(
                job_id.as_deref(),
                &tags,
                note.as_deref(),
                job::ShowLogsOptions {
                    follow,
                    only_stdout: stdout,
                    only_stderr: stderr,
                    tail,
                    raw,
                    debug: cli.debug,
                },
            )
            .await?;
        }

        Commands::Download {
            job_id,
            partial,
            path,
            filter,
            tags,
            dry_run,
        } => {
            job::download_outputs(
                job_id.as_deref(),
                partial,
                path.as_deref(),
                &filter,
                &tags,
                dry_run,
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
            archive,
            unarchive,
            yes,
            tags,
        } => {
            job::clean_jobs(
                job_id.as_deref(),
                older_than.as_deref(),
                &tags,
                job::CleanJobsOptions {
                    all,
                    clean_workspace: workspace,
                    archive,
                    unarchive,
                    skip_confirm: yes,
                    debug: cli.debug,
                },
            )
            .await?;
        }

        Commands::Tags => {
            job::list_tags()?;
        }

        Commands::Rerun { job_id, bg, tags } => {
            let config = Config::find_and_load()?;
            job::rerun_job(&config, &job_id, &tags, bg, cli.debug).await?;
        }

        Commands::Init => handlers::init()?,

        Commands::Check { remote } => {
            if remote {
                handlers::check_remote(cli.debug).await?;
            } else {
                handlers::check()?;
            }
        }

        Commands::Guide => {
            println!("{}", guide::GUIDE_TEXT);
        }

        Commands::Doctor => {
            handlers::doctor(cli.debug).await?;
        }

        Commands::Ping => {
            let config = Config::find_and_load()?;
            job::ping_cluster(&config, cli.debug).await?;
        }

        Commands::Wait {
            job_id,
            notify,
            tags,
        } => {
            job::wait_for_job(job_id.as_deref(), notify, &tags, cli.debug).await?;
        }

        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "fleche", &mut std::io::stdout());
        }

        Commands::Stats { job_id, last, tags } => {
            job::show_stats(job_id.as_deref(), last, &tags, cli.debug).await?;
        }

        Commands::Note { job_id, note } => {
            job::note_job(&job_id, note.as_deref())?;
        }

        Commands::Compare { job_a, job_b } => {
            handlers::compare_jobs(&job_a, &job_b)?;
        }
    }

    Ok(())
}
