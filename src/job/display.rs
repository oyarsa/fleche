//! Display helpers for formatting job information.

use crate::registry::{JobRecord, JobStatus};
use console::style;

/// Prints detailed information about a single job.
pub fn print_job_details(job: &JobRecord) {
    println!("{}", style("Job Details").bold().underlined());
    println!();
    println!("  {:<14} {}", style("ID:").bold(), job.id);
    println!(
        "  {:<14} {}",
        style("Slurm ID:").bold(),
        job.slurm_id.as_deref().unwrap_or("-")
    );
    println!("  {:<14} {}", style("Job Name:").bold(), job.job_name);
    println!("  {:<14} {}", style("Project:").bold(), job.project_name);
    println!(
        "  {:<14} {}",
        style("Status:").bold(),
        format_status(job.status)
    );
    if let Some(code) = job.exit_code {
        let styled_code = if code == 0 {
            style(code.to_string()).green()
        } else {
            style(code.to_string()).red()
        };
        println!("  {:<14} {}", style("Exit Code:").bold(), styled_code);
    }
    if let Some(ref slurm_state) = job.slurm_state {
        println!(
            "  {:<14} {}",
            style("Slurm State:").bold(),
            format_slurm_state(slurm_state)
        );
    }
    println!("  {:<14} {}", style("Remote Host:").bold(), job.remote_host);
    println!("  {:<14} {}", style("Workspace:").bold(), job.remote_path);
    println!(
        "  {:<14} {}",
        style("Created:").bold(),
        job.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    );

    if let Some(ref note) = job.note {
        println!();
        println!("  {:<14} {}", style("Note:").bold(), note);
    }

    if !job.tags.is_empty() {
        println!();
        println!("  {}", style("Tags:").bold());
        for (key, value) in &job.tags {
            println!("    {key}={value}");
        }
    }

    println!();
    println!("  {}", style("Command:").bold());
    for line in job.command.lines() {
        println!("    {line}");
    }
}

/// Prints a table of jobs with a `#` column showing global index positions.
///
/// The indices correspond to positions in the unfiltered global job list,
/// matching what `get_job_by_index` resolves. Filtered views show gaps
/// (e.g., `#3, #7, #10`) so numeric lookup always works correctly.
pub fn print_indexed_job_table(jobs: &[JobRecord], global_indices: &[usize]) {
    println!(
        "{:>3}  {:<45} {:<12} {:<12} {:<20}",
        style("#").bold().underlined(),
        style("ID").bold().underlined(),
        style("STATUS").bold().underlined(),
        style("SLURM ID").bold().underlined(),
        style("CREATED").bold().underlined(),
    );

    for (idx, job) in global_indices.iter().zip(jobs) {
        println!(
            "{}  {:<45} {} {:<12} {:<20}",
            style(format!("{idx:>3}")).dim(),
            truncate(&job.id, 44),
            format_status(job.status),
            job.slurm_id.as_deref().unwrap_or("-"),
            job.created_at.format("%Y-%m-%d %H:%M"),
        );
        print_job_subtitle(job, "      ");
    }
}

/// Prints a table of jobs without index numbers.
///
/// Used for views (e.g., archived) where numeric indices would not resolve
/// correctly via `get_job_by_index`.
pub fn print_job_table(jobs: &[JobRecord]) {
    println!(
        "{:<45} {:<12} {:<12} {:<20}",
        style("ID").bold().underlined(),
        style("STATUS").bold().underlined(),
        style("SLURM ID").bold().underlined(),
        style("CREATED").bold().underlined(),
    );

    for job in jobs {
        println!(
            "{:<45} {} {:<12} {:<20}",
            truncate(&job.id, 44),
            format_status(job.status),
            job.slurm_id.as_deref().unwrap_or("-"),
            job.created_at.format("%Y-%m-%d %H:%M"),
        );
        print_job_subtitle(job, "    ");
    }
}

/// Prints the optional subtitle line (job name, tags, and/or note) below a job row.
fn print_job_subtitle(job: &JobRecord, indent: &str) {
    let id_prefix = job.id.split('-').next().unwrap_or("");
    let show_name = job.job_name != id_prefix;
    let has_tags = !job.tags.is_empty();
    let has_note = job.note.is_some();

    if !show_name && !has_tags && !has_note {
        return;
    }

    let mut parts: Vec<String> = Vec::new();
    if show_name {
        parts.push(job.job_name.clone());
    }
    if has_tags {
        let tags: Vec<String> = job.tags.iter().map(|(k, v)| format!("{k}={v}")).collect();
        parts.push(tags.join(" "));
    }
    if let Some(ref note) = job.note {
        parts.push(format!("\"{}\"", truncate(note, 40)));
    }

    println!("{indent}{}", style(parts.join("  ")).dim());
}

/// Formats a job status with appropriate colors and fixed width.
pub fn format_status(status: JobStatus) -> String {
    match status {
        JobStatus::Pending => style(format!("{:<12}", "pending")).yellow().to_string(),
        JobStatus::Running => style(format!("{:<12}", "running")).blue().to_string(),
        JobStatus::Completed => style(format!("{:<12}", "completed")).green().to_string(),
        JobStatus::Failed => style(format!("{:<12}", "failed")).red().to_string(),
        JobStatus::Cancelled => style(format!("{:<12}", "cancelled")).dim().to_string(),
    }
}

/// Formats a raw Slurm state string with appropriate colors.
fn format_slurm_state(state: &str) -> String {
    let base = state.split_whitespace().next().unwrap_or(state);
    match base.to_uppercase().as_str() {
        "COMPLETED" => style(state.to_string()).green().to_string(),
        "CANCELLED" | "PREEMPTED" | "TIMEOUT" => style(state.to_string()).yellow().to_string(),
        "FAILED" | "OUT_OF_MEMORY" | "NODE_FAIL" | "BOOT_FAIL" | "DEADLINE" => {
            style(state.to_string()).red().to_string()
        }
        _ => state.to_string(),
    }
}

/// Truncates a string to a maximum length, adding "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_long_string() {
        assert_eq!(truncate("hello world", 8), "hello...");
    }

    #[test]
    fn test_truncate_empty_string() {
        assert_eq!(truncate("", 10), "");
    }
}
