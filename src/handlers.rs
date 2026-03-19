//! Command handlers for CLI commands with non-trivial logic.
//!
//! Most commands delegate directly to the [`crate::job`] module. This module contains
//! handlers for commands that have additional logic beyond simple delegation.

use crate::config::{Config, ResolvedJob, generate_init_config};
use crate::output::OutputFormat;
use crate::registry::Registry;
use anyhow::{Context, Result};
use console::style;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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

/// Handles the `check` command - validates and displays configuration.
pub fn check(config: &Config) {
    println!("{} Configuration is valid", style("✓").green());
    println!();

    print_config_summary(config);
    print_available_jobs(config);
}

fn print_config_summary(config: &Config) {
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
    if let Some(dotenv) = config.dotenv_file() {
        println!("  {:<14} {}", style("Dotenv file:").bold(), dotenv);
    }
}

fn print_available_jobs(config: &Config) {
    let job_names = config.job_names();
    println!();
    if job_names.is_empty() {
        println!(
            "  {}",
            style("No jobs defined. Add jobs to fleche.toml or create fleche/*.toml files.")
                .yellow()
        );
    } else {
        println!("  {}", style("Available jobs:").bold());
        for name in job_names {
            println!("    - {name}");
        }
    }
}

/// A job definition from configuration, for JSON output.
#[derive(Serialize)]
struct JobDefinition {
    name: String,
    command: Option<String>,
}

/// Converts config job definitions into structured entries for JSON output.
fn to_job_definitions(config: &Config) -> Vec<JobDefinition> {
    config
        .job_names()
        .iter()
        .filter_map(|name| {
            config.jobs.get(name).map(|def| JobDefinition {
                name: name.clone(),
                command: def.command.clone(),
            })
        })
        .collect()
}

/// Handles the `jobs` command - lists available jobs from config.
pub fn list_jobs(config: &Config, format: OutputFormat) -> Result<()> {
    let jobs = to_job_definitions(config);

    format.print(&jobs, || {
        if jobs.is_empty() {
            println!(
                "{}",
                style("No jobs defined. Add jobs to fleche.toml or create fleche/*.toml files.")
                    .yellow()
            );
            return Ok(());
        }

        for job in &jobs {
            if let Some(ref cmd) = job.command {
                println!("{} {}", style(&job.name).bold(), style(cmd).dim());
            } else {
                println!("{}", style(&job.name).bold());
            }
        }
        Ok(())
    })
}

/// Handles the `skill --install` command - installs the fleche skill for AI agents.
pub fn install_skill(scope: crate::cli::InstallScope, agent: crate::cli::Agent) -> Result<()> {
    let skill_content = include_str!("../docs/skill.md");

    let path = skill_install_path(scope, agent)?;

    let content = match agent {
        // Claude Code skills use YAML frontmatter
        crate::cli::Agent::Claude => skill_content,
        // Codex uses plain markdown
        crate::cli::Agent::Codex => strip_frontmatter(skill_content),
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    println!("{} Installed {}", style("✓").green(), path.display());

    Ok(())
}

/// Returns the installation path for a given scope and agent.
fn skill_install_path(
    scope: crate::cli::InstallScope,
    agent: crate::cli::Agent,
) -> Result<PathBuf> {
    use crate::cli::{Agent, InstallScope};

    match (scope, agent) {
        (InstallScope::Project, Agent::Claude) => {
            Ok(PathBuf::from(".claude/skills/fleche/SKILL.md"))
        }
        (InstallScope::Global, Agent::Claude) => {
            let home = std::env::var("HOME").context("HOME not set")?;
            Ok(PathBuf::from(home).join(".claude/skills/fleche/SKILL.md"))
        }
        // Codex reads AGENTS.md per directory; subdirectory AGENTS.md files
        // are loaded when the agent works in that directory.
        (InstallScope::Project, Agent::Codex) => Ok(PathBuf::from(".codex/AGENTS.md")),
        (InstallScope::Global, Agent::Codex) => {
            let home = std::env::var("HOME").context("HOME not set")?;
            Ok(PathBuf::from(home).join(".codex/instructions.md"))
        }
    }
}

/// Strips YAML frontmatter (delimited by `---`) from the beginning of a string.
fn strip_frontmatter(content: &str) -> &str {
    if let Some(rest) = content.strip_prefix("---\n") {
        if let Some(pos) = rest.find("\n---\n") {
            return &rest[pos + 5..];
        }
    }
    content
}

/// Handles the `compare` command - shows differences between two jobs.
pub fn compare_jobs(first_id: &str, second_id: &str) -> Result<()> {
    let registry = Registry::open()?;
    let job_a = registry.get_job(first_id)?;
    let job_b = registry.get_job(second_id)?;

    let config_a: ResolvedJob =
        serde_json::from_str(&job_a.config_json).context("parsing config for first job")?;
    let config_b: ResolvedJob =
        serde_json::from_str(&job_b.config_json).context("parsing config for second job")?;

    print_comparison_header(&job_a.id, &job_b.id);

    let col_width = 35;
    print_basic_comparison(&job_a, &job_b, &config_a, &config_b, col_width);
    print_slurm_comparison(&config_a, &config_b, col_width);
    print_env_comparison(&config_a, &config_b, col_width);
    print_tags_comparison(&job_a.tags, &job_b.tags, col_width);
    print_notes_comparison(job_a.note.as_deref(), job_b.note.as_deref(), col_width);
    print_io_comparison(&config_a, &config_b);

    println!();
    Ok(())
}

fn print_comparison_header(id_a: &str, id_b: &str) {
    println!(
        "{} {} vs {}",
        style("Comparing").bold(),
        style(id_a).cyan(),
        style(id_b).cyan()
    );
    println!();
}

fn print_basic_comparison(
    job_a: &crate::registry::JobRecord,
    job_b: &crate::registry::JobRecord,
    config_a: &ResolvedJob,
    config_b: &ResolvedJob,
    col_width: usize,
) {
    // Header with job IDs
    println!(
        "  {:<15} {:>col_width$}  {:>col_width$}",
        "",
        style(&job_a.id).dim(),
        style(&job_b.id).dim()
    );
    println!("  {}", "-".repeat(15 + col_width * 2 + 4));

    compare_field("Job name", &config_a.name, &config_b.name, col_width);
    compare_field("Command", &config_a.command, &config_b.command, col_width);
    compare_field("Host", &config_a.host, &config_b.host, col_width);
    compare_field(
        "Status",
        &format!("{:?}", job_a.status),
        &format!("{:?}", job_b.status),
        col_width,
    );
}

fn print_slurm_comparison(config_a: &ResolvedJob, config_b: &ResolvedJob, col_width: usize) {
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
}

fn print_env_comparison(config_a: &ResolvedJob, config_b: &ResolvedJob, col_width: usize) {
    let all_keys: BTreeSet<_> = config_a.env.keys().chain(config_b.env.keys()).collect();

    if all_keys.is_empty() {
        return;
    }

    println!();
    println!("  {}", style("Environment").bold());
    for key in all_keys {
        let val_a = config_a.env.get(key).map(String::as_str);
        let val_b = config_b.env.get(key).map(String::as_str);
        compare_option_field(key, val_a, val_b, col_width);
    }
}

fn print_tags_comparison(
    tags_a: &std::collections::HashMap<String, String>,
    tags_b: &std::collections::HashMap<String, String>,
    col_width: usize,
) {
    let all_keys: BTreeSet<_> = tags_a.keys().chain(tags_b.keys()).collect();

    if all_keys.is_empty() {
        return;
    }

    println!();
    println!("  {}", style("Tags").bold());
    for key in all_keys {
        let val_a = tags_a.get(key).map(String::as_str);
        let val_b = tags_b.get(key).map(String::as_str);
        compare_option_field(key, val_a, val_b, col_width);
    }
}

fn print_notes_comparison(note_a: Option<&str>, note_b: Option<&str>, col_width: usize) {
    if note_a.is_none() && note_b.is_none() {
        return;
    }

    println!();
    println!("  {}", style("Notes").bold());
    compare_option_field("Note", note_a, note_b, col_width);
}

fn print_io_comparison(config_a: &ResolvedJob, config_b: &ResolvedJob) {
    if config_a.inputs != config_b.inputs {
        println!();
        println!("  {}", style("Inputs").bold());
        println!("    A: {}", format_list(&config_a.inputs));
        println!("    B: {}", format_list(&config_b.inputs));
    }

    if config_a.outputs != config_b.outputs {
        println!();
        println!("  {}", style("Outputs").bold());
        println!("    A: {}", format_list(&config_a.outputs));
        println!("    B: {}", format_list(&config_b.outputs));
    }
}

fn format_list(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}

// =============================================================================
// Comparison helpers
// =============================================================================

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
    let str_a = val_a.map_or_else(|| "-".to_string(), |v| v.to_string());
    let str_b = val_b.map_or_else(|| "-".to_string(), |v| v.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_str_exact() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_str_long() {
        // "hello world" (11 chars) truncated to 8 -> "hello w…"
        assert_eq!(truncate_str("hello world", 8), "hello w…");
    }

    #[test]
    fn test_truncate_str_empty() {
        assert_eq!(truncate_str("", 10), "");
    }

    #[test]
    fn test_strip_frontmatter() {
        let input = "---\nname: test\ndescription: hello\n---\n\n# Content\nBody here.\n";
        assert_eq!(strip_frontmatter(input), "\n# Content\nBody here.\n");
    }

    #[test]
    fn test_strip_frontmatter_no_frontmatter() {
        let input = "# Just markdown\nNo frontmatter here.\n";
        assert_eq!(strip_frontmatter(input), input);
    }
}
