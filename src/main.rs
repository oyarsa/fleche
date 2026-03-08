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
//! - [`job`]: High-level job operations (run, status, logs, sync, etc.)
//! - [`registry`]: Local `SQLite` database for tracking submitted jobs
//! - [`slurm`]: Slurm-specific operations (sbatch generation, status queries)
//! - [`ssh`]: SSH client for remote command execution
//! - [`sync`]: File synchronization using rsync

mod cli;
mod config;
mod diagnostics;
mod error;
mod handlers;
mod job;
mod local;
mod proxy;
mod registry;
mod runtime;
mod slurm;
mod ssh;
mod sync;

use anyhow::Result;
use clap::{CommandFactory, Parser};
use cli::{Cli, Commands};
use config::Config;
use console::style;
use job::StatusOptions;
use registry::ArchivedFilter;
use runtime::RuntimeCtx;
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

    // Optional settings are used by commands that can run without a project config.
    let optional_settings = Config::find_and_load().ok().map(|c| c.settings);
    let runtime_ctx = RuntimeCtx::from_optional_settings(cli.debug, optional_settings.as_ref());

    match cli.command {
        Commands::Run(args) => {
            let config = Config::find_and_load()?;
            let runtime_ctx = RuntimeCtx::from_settings(cli.debug, &config.settings);
            let slurm_overrides = slurm_config_from_cli(
                args.partition,
                args.time,
                args.gpus,
                args.cpus,
                args.memory,
                args.constraint,
                args.nodes,
                args.exclude,
            );

            job::run_job(
                &config,
                args.job_or_command.as_deref(),
                args.command.as_deref(),
                &args.env_vars,
                &args.tags,
                slurm_overrides,
                args.host.as_deref(),
                job::RunJobOptions {
                    background: args.bg,
                    notify: args.notify,
                    dry_run: args.dry_run,
                    after: args.after,
                    retry: args.retry,
                    note: args.note,
                    exec: args.exec,
                },
                runtime_ctx,
            )
            .await?;
        }

        Commands::Exec {
            command,
            env_vars,
            host,
            no_sync,
        } => {
            let config = Config::find_and_load()?;
            let runtime_ctx = RuntimeCtx::from_settings(cli.debug, &config.settings);
            job::exec_command(
                &config,
                &command,
                &env_vars,
                host.as_deref(),
                no_sync,
                runtime_ctx,
            )
            .await?;
        }

        Commands::Status(args) => {
            let archived_filter = if args.archived {
                ArchivedFilter::OnlyArchived
            } else if args.all_jobs {
                ArchivedFilter::IncludeAll
            } else {
                ArchivedFilter::ExcludeArchived
            };

            job::show_status(
                args.job_id.as_deref(),
                StatusOptions {
                    filters: &args.filter,
                    name: args.name.as_deref(),
                    tags: &args.tags,
                    last: args.last,
                    default_limit: optional_settings.as_ref().map(|s| s.default_list_limit),
                    archived: archived_filter,
                    compact: args.compact,
                    json: cli.json,
                },
                runtime_ctx,
            )
            .await?;
        }

        Commands::Logs(args) => {
            job::show_logs(
                args.job_id.as_deref(),
                &args.tags,
                args.note.as_deref(),
                job::ShowLogsOptions {
                    follow: args.follow,
                    only_stdout: args.stdout,
                    only_stderr: args.stderr,
                    tail: args.tail,
                    raw: args.raw,
                    ctx: runtime_ctx,
                },
            )
            .await?;
        }

        Commands::Download(args) => {
            job::download_outputs(
                args.job_id.as_deref(),
                args.partial,
                args.path.as_deref(),
                &args.filter,
                &args.tags,
                args.dry_run,
                runtime_ctx,
            )
            .await?;
        }

        Commands::Cancel(args) => {
            job::cancel_jobs(
                args.job_id.as_deref(),
                &args.tags,
                job::CancelJobsOptions {
                    all: args.all,
                    skip_confirm: args.yes,
                    dry_run: args.dry_run,
                    json: cli.json,
                    ctx: runtime_ctx,
                },
            )
            .await?;
        }

        Commands::Clean(args) => {
            job::clean_jobs(
                args.job_id.as_deref(),
                args.older_than.as_deref(),
                &args.tags,
                job::CleanJobsOptions {
                    all: args.all,
                    clean_workspace: args.workspace,
                    archive: args.archive,
                    unarchive: args.unarchive,
                    dry_run: args.dry_run,
                    skip_confirm: args.yes,
                    json: cli.json,
                    ctx: runtime_ctx,
                },
            )
            .await?;
        }

        Commands::Jobs => {
            let config = Config::find_and_load()?;
            handlers::list_jobs(&config, cli.json);
        }

        Commands::Tags => {
            job::list_tags(cli.json)?;
        }

        Commands::Rerun { job_id, bg, tags } => {
            let config = Config::find_and_load()?;
            let runtime_ctx = RuntimeCtx::from_settings(cli.debug, &config.settings);
            job::rerun_job(&config, &job_id, &tags, bg, runtime_ctx).await?;
        }

        Commands::Init => handlers::init()?,

        Commands::Check { remote } => {
            let config = Config::find_and_load()?;
            let runtime_ctx = RuntimeCtx::from_settings(cli.debug, &config.settings);
            handlers::check(&config);
            if remote {
                diagnostics::check_remote(&config, runtime_ctx).await?;
            }
        }

        Commands::Guide => {
            print!("{}", include_str!("../docs/guide.md"));
        }

        Commands::Doctor => {
            diagnostics::doctor(runtime_ctx.debug).await?;
        }

        Commands::Ping => {
            let config = Config::find_and_load()?;
            let runtime_ctx = RuntimeCtx::from_settings(cli.debug, &config.settings);
            job::ping_cluster(&config, runtime_ctx).await?;
        }

        Commands::Wait {
            job_id,
            notify,
            tags,
        } => {
            job::wait_for_job(job_id.as_deref(), notify, &tags, cli.json, runtime_ctx).await?;
        }

        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "fleche", &mut std::io::stdout());
        }

        Commands::Stats { job_id, last, tags } => {
            job::show_stats(job_id.as_deref(), last, &tags, cli.json, runtime_ctx).await?;
        }

        Commands::Note { job_id, note } => {
            job::note_job(&job_id, note.as_deref())?;
        }

        Commands::Compare { job_a, job_b } => {
            handlers::compare_jobs(&job_a, &job_b)?;
        }

        Commands::Proxy {
            command,
            port,
            host,
        } => {
            let host = match host {
                Some(h) => h,
                None => Config::find_and_load()?.remote.host,
            };
            let code = proxy::run_proxy_command(&host, &command, port, cli.debug).await?;
            if code != 0 {
                std::process::exit(code);
            }
        }
    }

    Ok(())
}
