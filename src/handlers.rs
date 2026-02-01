//! Command handlers for CLI commands with non-trivial logic.
//!
//! Most commands delegate directly to the [`job`] module. This module contains
//! handlers for commands that have additional logic beyond simple delegation.

use crate::config::{Config, generate_init_config};
use anyhow::Result;
use console::style;
use std::path::Path;

/// Handles the `init` command - creates a starter fleche.toml.
pub fn init() -> Result<()> {
    let config_path = Path::new("fleche.toml");
    if config_path.exists() {
        anyhow::bail!("fleche.toml already exists in current directory");
    }

    std::fs::write(config_path, generate_init_config())?;
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
