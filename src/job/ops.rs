//! Job operations - status, logs, download, cancel, clean, wait, and ping.

use crate::config::{Config, ResolvedJob};
use crate::error::{FlecheError, Result};
use crate::registry::{JobRecord, JobStatus, Registry, parse_duration};
use crate::slurm::{cancel_job, get_job_status};
use crate::ssh::SshClient;
use crate::sync::{download_outputs as sync_download_outputs, download_path as sync_download_path};
use console::style;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::io::Write;
use std::time::Duration;

use super::display::{print_job_details, print_job_table};

/// Options for displaying job logs.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct ShowLogsOptions {
    /// Stream logs in real-time.
    pub follow: bool,
    /// Show only stdout.
    pub only_stdout: bool,
    /// Show only stderr.
    pub only_stderr: bool,
    /// Show only the last N lines.
    pub tail: Option<usize>,
    /// Strip ANSI escape codes from output.
    pub raw: bool,
    /// Enable verbose SSH output.
    pub debug: bool,
}

/// Options for cleaning up jobs.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct CleanJobsOptions {
    /// Clean all completed/failed jobs.
    pub all: bool,
    /// Also delete the shared workspace.
    pub clean_workspace: bool,
    /// Skip confirmation prompt.
    pub skip_confirm: bool,
    /// Enable verbose SSH output.
    pub debug: bool,
}

/// Shows the status of a specific job or lists recent jobs.
pub async fn show_status(
    job_id: Option<&str>,
    filters: &[String],
    name_filter: Option<&str>,
    tags: &[(String, String)],
    last: Option<usize>,
    debug: bool,
) -> Result<()> {
    let registry = Registry::open()?;

    if let Some(id) = job_id {
        let job = registry.get_job(id)?;
        let ssh = SshClient::new(&job.remote_host, debug);

        // Get current status from Slurm
        let current_status = if let Some(ref slurm_id) = job.slurm_id {
            match get_job_status(&ssh, slurm_id).await {
                Ok(status) => {
                    registry.update_status(&job.id, status)?;
                    status
                }
                Err(_) => job.status,
            }
        } else {
            job.status
        };

        print_job_details(&job, current_status);
    } else {
        // Refresh status for all pending/running jobs
        refresh_active_job_statuses(&registry, debug).await?;

        // Parse status filters
        let status_filters: Vec<JobStatus> = filters
            .iter()
            .map(|f| f.parse())
            .collect::<Result<Vec<_>>>()?;

        let limit = last.unwrap_or(20);
        let jobs = registry.list_jobs(None, &status_filters, name_filter, tags, limit)?;

        if jobs.is_empty() {
            println!("No jobs found. Run `fleche run` to submit a job.");
            return Ok(());
        }

        print_job_table(&jobs);
    }

    Ok(())
}

/// Lists all unique tags across jobs.
pub fn list_tags() -> Result<()> {
    let registry = Registry::open()?;
    let tags = registry.list_unique_tags()?;

    if tags.is_empty() {
        println!("No tags found. Use --tag when running jobs to add tags.");
        return Ok(());
    }

    // Group by key
    let mut current_key = String::new();
    for (key, value) in &tags {
        if key != &current_key {
            if !current_key.is_empty() {
                println!();
            }
            println!("{}", style(key).bold());
            current_key.clone_from(key);
        }
        println!("  {value}");
    }

    Ok(())
}

/// Displays logs from a job's stdout or stderr.
pub async fn show_logs(
    job_id: Option<&str>,
    tags: &[(String, String)],
    opts: ShowLogsOptions,
) -> Result<()> {
    let registry = Registry::open()?;

    // If no job ID provided, use most recent job (optionally filtered by tags)
    let job = if let Some(id) = job_id {
        registry.get_job(id)?
    } else {
        registry
            .list_jobs(None, &[], None, tags, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                FlecheError::Other("No jobs found. Run `fleche run` to submit a job.".to_string())
            })?
    };

    let ssh = SshClient::new(&job.remote_host, opts.debug);

    // Job logs are in the job directory, not workspace
    // remote_path is the workspace, job logs are in ../jobs/<job_id>/
    let base = job.remote_path.trim_end_matches("/workspace");
    let log_base = format!("{}/jobs/{}", base, job.id);

    // Determine which streams to show
    let show_stdout = !opts.only_stderr || opts.only_stdout;
    let show_stderr = !opts.only_stdout || opts.only_stderr;
    let show_both = show_stdout && show_stderr;

    // Strip ANSI codes if --raw is set or if stdout is not a terminal (piped)
    let strip_ansi = opts.raw || !std::io::IsTerminal::is_terminal(&std::io::stdout());

    if opts.follow {
        let log_file = if opts.only_stderr {
            "job.err"
        } else {
            "job.out"
        };
        let log_path = format!("{log_base}/{log_file}");

        if job.slurm_id.is_some() {
            println!(
                "{}",
                style("Following output (Ctrl+C to disconnect)...").yellow()
            );
        }
        let mut child = ssh.tail_follow(&log_path)?;
        let _ = child.wait().await;
    } else if show_both {
        println!("{}", style("=== STDOUT ===").bold());
        let stdout_path = format!("{log_base}/job.out");
        match ssh.cat_tail(&stdout_path, opts.tail).await {
            Ok(content) => print!("{}", maybe_strip_ansi(&content, strip_ansi)),
            Err(e) => eprintln!("Error reading stdout: {e}"),
        }

        println!();
        println!("{}", style("=== STDERR ===").bold());
        let stderr_path = format!("{log_base}/job.err");
        match ssh.cat_tail(&stderr_path, opts.tail).await {
            Ok(content) => print!("{}", maybe_strip_ansi(&content, strip_ansi)),
            Err(e) => eprintln!("Error reading stderr: {e}"),
        }
    } else {
        let log_file = if show_stderr { "job.err" } else { "job.out" };
        let log_path = format!("{log_base}/{log_file}");

        let content = ssh.cat_tail(&log_path, opts.tail).await?;
        print!("{}", maybe_strip_ansi(&content, strip_ansi));
    }

    Ok(())
}

/// Downloads output files from a job's workspace back to the local project.
pub async fn download_outputs(
    job_id: Option<&str>,
    partial: bool,
    specific_path: Option<&str>,
    filters: &[String],
    tags: &[(String, String)],
    debug: bool,
) -> Result<()> {
    let registry = Registry::open()?;

    // If no job ID provided, use most recent job (optionally filtered by tags)
    let job = if let Some(id) = job_id {
        registry.get_job(id)?
    } else {
        registry
            .list_jobs(None, &[], None, tags, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                FlecheError::Other("No jobs found. Run `fleche run` to submit a job.".to_string())
            })?
    };

    // Check job status
    if !partial && matches!(job.status, JobStatus::Pending | JobStatus::Running) {
        if let Some(ref slurm_id) = job.slurm_id {
            let ssh = SshClient::new(&job.remote_host, debug);
            let current_status = get_job_status(&ssh, slurm_id).await.unwrap_or(job.status);
            if matches!(current_status, JobStatus::Pending | JobStatus::Running) {
                eprintln!(
                    "{}",
                    style("Warning: Job is still running. Use --partial to download anyway.")
                        .yellow()
                );
                return Ok(());
            }
        }
    }

    let local_path = std::path::PathBuf::from(&job.project_path);

    if let Some(path) = specific_path {
        println!("Downloading {path} from workspace...");
        sync_download_path(&job.remote_host, &job.remote_path, path, &local_path).await?;
    } else {
        // Parse config to get outputs
        let resolved: ResolvedJob = serde_json::from_str(&job.config_json)?;

        if resolved.outputs.is_empty() {
            println!("No outputs defined for this job.");
            return Ok(());
        }

        // Apply filters if provided
        let (includes, excludes) = build_filter_glob_sets(filters)?;
        let outputs = filter_outputs(&resolved.outputs, includes.as_ref(), excludes.as_ref());

        if outputs.is_empty() {
            println!("No outputs match the specified filters.");
            return Ok(());
        }

        println!("Downloading outputs from workspace...");
        for output in &outputs {
            println!("  {output}");
        }
        sync_download_outputs(&job.remote_host, &job.remote_path, &outputs, &local_path).await?;
    }

    registry.set_outputs_synced(&job.id)?;
    println!("{}", style("Download complete.").green());

    Ok(())
}

/// Cancels running or pending Slurm jobs.
pub async fn cancel_jobs(
    job_id: Option<&str>,
    all: bool,
    skip_confirm: bool,
    tags: &[(String, String)],
    debug: bool,
) -> Result<()> {
    let registry = Registry::open()?;

    let jobs_to_cancel: Vec<JobRecord> = if let Some(id) = job_id {
        let job = registry.get_job(id)?;
        if matches!(
            job.status,
            JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
        ) {
            return Err(FlecheError::CannotCancel(
                id.to_string(),
                job.status.to_string(),
            ));
        }
        vec![job]
    } else if all {
        // Get active jobs, optionally filtered by tags
        if tags.is_empty() {
            registry.list_active_jobs()?
        } else {
            registry
                .list_jobs(None, &[], None, tags, usize::MAX)?
                .into_iter()
                .filter(|j| matches!(j.status, JobStatus::Pending | JobStatus::Running))
                .collect()
        }
    } else {
        // Cancel most recent active job (optionally filtered by tags)
        let active: Vec<JobRecord> = if tags.is_empty() {
            registry.list_active_jobs()?
        } else {
            registry
                .list_jobs(None, &[], None, tags, usize::MAX)?
                .into_iter()
                .filter(|j| matches!(j.status, JobStatus::Pending | JobStatus::Running))
                .collect()
        };
        if active.is_empty() {
            println!("No active jobs to cancel.");
            return Ok(());
        }
        let mut active = active;
        active.truncate(1);
        active
    };

    if jobs_to_cancel.is_empty() {
        println!("No active jobs to cancel.");
        return Ok(());
    }

    // Show jobs and confirm if multiple
    if jobs_to_cancel.len() > 1 || all {
        println!("Jobs to cancel:");
        for job in &jobs_to_cancel {
            println!(
                "  {} ({}) - {}",
                job.id,
                style(&job.status).yellow(),
                job.job_name
            );
        }
        println!();

        if !skip_confirm && !confirm("Cancel these jobs?")? {
            println!("Cancelled.");
            return Ok(());
        }
    }

    for job in &jobs_to_cancel {
        let Some(ref slurm_id) = job.slurm_id else {
            eprintln!("  Warning: Job {} has no Slurm ID, skipping", job.id);
            continue;
        };

        let ssh = SshClient::new(&job.remote_host, debug);
        if let Err(e) = cancel_job(&ssh, slurm_id).await {
            eprintln!("  Warning: Could not cancel {}: {e}", job.id);
            continue;
        }
        registry.update_status(&job.id, JobStatus::Cancelled)?;
        println!("{} Job {} cancelled", style("✓").green(), job.id);
    }

    Ok(())
}

/// Cleans up jobs by removing them from the registry and deleting remote job files.
pub async fn clean_jobs(
    job_id: Option<&str>,
    older_than: Option<&str>,
    tags: &[(String, String)],
    opts: CleanJobsOptions,
) -> Result<()> {
    let registry = Registry::open()?;

    let jobs_to_clean: Vec<JobRecord> = if let Some(id) = job_id {
        vec![registry.get_job(id)?]
    } else if opts.all {
        // Get finished jobs, optionally filtered by tags
        if tags.is_empty() {
            registry.list_finished_jobs()?
        } else {
            registry
                .list_jobs(None, &[], None, tags, usize::MAX)?
                .into_iter()
                .filter(|j| {
                    matches!(
                        j.status,
                        JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
                    )
                })
                .collect()
        }
    } else if let Some(duration_str) = older_than {
        let duration = parse_duration(duration_str)?;
        let older_jobs = registry.list_jobs_older_than(duration)?;
        // Filter by tags if provided
        if tags.is_empty() {
            older_jobs
        } else {
            older_jobs
                .into_iter()
                .filter(|j| tags.iter().all(|(k, v)| j.tags.get(k) == Some(v)))
                .collect()
        }
    } else {
        println!("Specify a job ID, --all, or --older-than");
        return Ok(());
    };

    if jobs_to_clean.is_empty() && !opts.clean_workspace {
        println!("No jobs to clean.");
        return Ok(());
    }

    // Show jobs and confirm
    if !jobs_to_clean.is_empty() && (jobs_to_clean.len() > 1 || opts.all || older_than.is_some()) {
        println!("Jobs to clean:");
        for job in &jobs_to_clean {
            println!(
                "  {} ({}) - {}",
                job.id,
                style(&job.status).cyan(),
                job.job_name
            );
        }
        println!();
    }

    if opts.clean_workspace {
        println!(
            "{}",
            style("WARNING: This will also delete the shared workspace!")
                .red()
                .bold()
        );
    }

    if !opts.skip_confirm && !confirm("Proceed with cleanup?")? {
        println!("Cancelled.");
        return Ok(());
    }

    // Clean job directories
    for job in &jobs_to_clean {
        print!("Cleaning {}... ", job.id);

        // Delete job directory (logs/metadata only, not workspace)
        let ssh = SshClient::new(&job.remote_host, opts.debug);
        let job_dir = format!(
            "{}/.fleche/jobs/{}",
            job.remote_path
                .trim_end_matches("/workspace")
                .trim_end_matches("/.fleche/workspace"),
            job.id
        );
        if let Err(e) = ssh.rm_rf(&job_dir).await {
            eprintln!("warning: could not delete job directory: {e}");
        }

        registry.delete_job(&job.id)?;
        println!("{}", style("done").green());
    }

    // Clean workspace if requested
    if opts.clean_workspace {
        if let Some(job) = jobs_to_clean.first() {
            let ssh = SshClient::new(&job.remote_host, opts.debug);
            print!("Cleaning workspace... ");
            if let Err(e) = ssh.rm_rf(&job.remote_path).await {
                eprintln!("warning: could not delete workspace: {e}");
            } else {
                println!("{}", style("done").green());
            }
        }
    }

    if !jobs_to_clean.is_empty() {
        println!(
            "\n{} Cleaned {} job(s)",
            style("✓").green(),
            jobs_to_clean.len()
        );
    }

    Ok(())
}

/// Waits for a job to complete.
///
/// Polls the job status until it reaches a terminal state (completed, failed, cancelled).
pub async fn wait_for_job(
    job_id: Option<&str>,
    notify: bool,
    tags: &[(String, String)],
    debug: bool,
) -> Result<()> {
    let registry = Registry::open()?;

    // Resolve job ID
    let job = if let Some(id) = job_id {
        registry.get_job(id)?
    } else {
        registry
            .list_jobs(None, &[], None, tags, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                FlecheError::Other("No jobs found. Run `fleche run` to submit a job.".to_string())
            })?
    };

    let ssh = SshClient::new(&job.remote_host, debug);

    println!("Waiting for job {}...", style(&job.id).bold());

    loop {
        if let Some(ref slurm_id) = job.slurm_id {
            let status = get_job_status(&ssh, slurm_id).await?;
            registry.update_status(&job.id, status)?;

            let message = match status {
                JobStatus::Completed => {
                    let msg = format!("Job {} completed successfully.", job.id);
                    println!("{}", style(&msg).green().bold());
                    Some(msg)
                }
                JobStatus::Failed => {
                    let msg = format!("Job {} failed.", job.id);
                    println!("{}", style(&msg).red().bold());
                    Some(msg)
                }
                JobStatus::Cancelled => {
                    let msg = format!("Job {} was cancelled.", job.id);
                    println!("{}", style(&msg).yellow().bold());
                    Some(msg)
                }
                _ => None,
            };

            if let Some(msg) = message {
                if notify {
                    send_notification(&msg);
                }
                return Ok(());
            }
        } else {
            return Err(FlecheError::Other(format!(
                "Job {} has no Slurm ID",
                job.id
            )));
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Pings the Slurm controller to check cluster health.
///
/// Runs `scontrol ping` on the remote host and reports the status of the
/// Slurm controller(s). Useful for diagnosing timeout issues.
pub async fn ping_cluster(config: &Config, debug: bool) -> Result<()> {
    let ssh = SshClient::new(&config.remote.host, debug);

    println!(
        "Pinging Slurm controller on {}...",
        style(&config.remote.host).bold()
    );
    println!();

    let (success, stdout, stderr) = ssh.exec_allow_failure("scontrol ping").await?;

    if success {
        // Parse and display the output
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Color-code UP/DOWN status
            if line.contains("is UP") {
                println!("{}", style(line).green());
            } else if line.contains("is DOWN") {
                println!("{}", style(line).red());
            } else {
                println!("{line}");
            }
        }
        println!();

        if stdout.contains("is DOWN") {
            println!(
                "{}",
                style("Warning: One or more controllers are down. Jobs may be slow or fail.")
                    .yellow()
            );
        } else {
            println!("{}", style("Cluster is healthy.").green().bold());
        }
    } else {
        // scontrol ping failed entirely
        eprintln!("{}", style("Failed to ping Slurm controller.").red());
        if !stderr.is_empty() {
            eprintln!("{stderr}");
        }
        return Err(FlecheError::Other(
            "Could not reach Slurm controller".to_string(),
        ));
    }

    Ok(())
}

// --- Private helper functions ---

/// Refreshes the status of all pending/running jobs from Slurm.
async fn refresh_active_job_statuses(registry: &Registry, debug: bool) -> Result<()> {
    let active_jobs = registry.list_active_jobs()?;

    for job in active_jobs {
        if let Some(ref slurm_id) = job.slurm_id {
            let ssh = SshClient::new(&job.remote_host, debug);
            if let Ok(status) = get_job_status(&ssh, slurm_id).await {
                if status != job.status {
                    registry.update_status(&job.id, status)?;
                }
            }
        }
    }

    Ok(())
}

/// Prompts the user for confirmation.
fn confirm(prompt: &str) -> Result<bool> {
    use std::io::{self, Write};

    print!("{prompt} [y/N] ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Sends a terminal notification using OSC 9.
fn send_notification(message: &str) {
    print!("\x1b]9;fleche: {message}\x07");
    let _ = std::io::stdout().flush();
}

/// Strips ANSI escape codes from a string if `strip` is true.
fn maybe_strip_ansi(s: &str, strip: bool) -> std::borrow::Cow<'_, str> {
    if strip {
        strip_ansi_codes(s).into()
    } else {
        s.into()
    }
}

/// Strips ANSI escape codes from a string.
///
/// Handles common ANSI sequences: CSI (ESC [), OSC (ESC ]), and basic escape codes.
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // ESC character - start of escape sequence
            match chars.peek() {
                Some('[') => {
                    // CSI sequence: ESC [ ... (ends with letter or @-~)
                    chars.next(); // consume '['
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc.is_ascii_alphabetic() || ('@'..='~').contains(&nc) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC sequence: ESC ] ... (ends with BEL or ST)
                    chars.next(); // consume ']'
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc == '\x07' {
                            // BEL
                            break;
                        }
                        if nc == '\x1b' {
                            // ST (ESC \)
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                Some('(' | ')') => {
                    // Character set selection: ESC ( or ESC )
                    chars.next();
                    chars.next(); // skip the designator
                }
                _ => {
                    // Single-character escape or unknown - skip next char
                    chars.next();
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Builds include and exclude glob sets from filter patterns.
///
/// Patterns prefixed with `!` are exclusions, others are inclusions.
fn build_filter_glob_sets(filters: &[String]) -> Result<(Option<GlobSet>, Option<GlobSet>)> {
    let mut includes = GlobSetBuilder::new();
    let mut excludes = GlobSetBuilder::new();
    let mut has_includes = false;
    let mut has_excludes = false;

    for pattern in filters {
        if let Some(exclude_pattern) = pattern.strip_prefix('!') {
            excludes.add(Glob::new(exclude_pattern).map_err(|e| {
                FlecheError::Other(format!("Invalid exclude pattern '{exclude_pattern}': {e}"))
            })?);
            has_excludes = true;
        } else {
            includes.add(Glob::new(pattern).map_err(|e| {
                FlecheError::Other(format!("Invalid filter pattern '{pattern}': {e}"))
            })?);
            has_includes = true;
        }
    }

    let include_set =
        if has_includes {
            Some(includes.build().map_err(|e| {
                FlecheError::Other(format!("Failed to build include glob set: {e}"))
            })?)
        } else {
            None
        };

    let exclude_set =
        if has_excludes {
            Some(excludes.build().map_err(|e| {
                FlecheError::Other(format!("Failed to build exclude glob set: {e}"))
            })?)
        } else {
            None
        };

    Ok((include_set, exclude_set))
}

/// Filters outputs based on include/exclude glob sets.
///
/// Semantics:
/// - No filters: return all outputs
/// - Include patterns only: output must match at least one
/// - Exclude patterns only: output must not match any
/// - Both: output must match an include AND not match any exclude
fn filter_outputs(
    outputs: &[String],
    includes: Option<&GlobSet>,
    excludes: Option<&GlobSet>,
) -> Vec<String> {
    outputs
        .iter()
        .filter(|output| {
            // Strip trailing slash for matching (directories like "output/" should match "output/**")
            let path = output.trim_end_matches('/');

            // Check include patterns
            let matches_include = match includes {
                Some(set) => set.is_match(path),
                None => true, // No includes means all match
            };

            // Check exclude patterns
            let matches_exclude = match excludes {
                Some(set) => set.is_match(path),
                None => false, // No excludes means none excluded
            };

            matches_include && !matches_exclude
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_codes_csi_colors() {
        // Basic color codes
        assert_eq!(strip_ansi_codes("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(
            strip_ansi_codes("\x1b[1;32mbold green\x1b[0m"),
            "bold green"
        );
        assert_eq!(
            strip_ansi_codes("\x1b[38;5;196mextended\x1b[0m"),
            "extended"
        );
    }

    #[test]
    fn test_strip_ansi_codes_csi_cursor() {
        // Cursor movement
        assert_eq!(strip_ansi_codes("\x1b[2Jclear"), "clear");
        assert_eq!(strip_ansi_codes("\x1b[10;20Hposition"), "position");
    }

    #[test]
    fn test_strip_ansi_codes_osc() {
        // OSC sequences (terminal title, notifications)
        assert_eq!(strip_ansi_codes("\x1b]0;title\x07text"), "text");
        assert_eq!(strip_ansi_codes("\x1b]9;notification\x07text"), "text");
    }

    #[test]
    fn test_strip_ansi_codes_preserves_text() {
        assert_eq!(strip_ansi_codes("plain text"), "plain text");
        assert_eq!(strip_ansi_codes("line1\nline2"), "line1\nline2");
        assert_eq!(strip_ansi_codes(""), "");
    }

    #[test]
    fn test_strip_ansi_codes_complex() {
        let input = "\x1b[1mBold\x1b[0m and \x1b[31mred\x1b[0m text";
        assert_eq!(strip_ansi_codes(input), "Bold and red text");
    }

    #[test]
    fn test_strip_ansi_codes_progress_bar() {
        // Common progress bar output with cursor control
        let input = "Progress: \x1b[32m50%\x1b[0m \x1b[K";
        assert_eq!(strip_ansi_codes(input), "Progress: 50% ");
    }

    #[test]
    fn test_filter_outputs_no_filters() {
        let outputs = vec!["a.json".to_string(), "b.csv".to_string()];
        let result = filter_outputs(&outputs, None, None);
        assert_eq!(result, outputs);
    }

    #[test]
    fn test_filter_outputs_include_only() {
        let outputs = vec![
            "predictions.json".to_string(),
            "model.pt".to_string(),
            "data.csv".to_string(),
        ];
        let (includes, excludes) = build_filter_glob_sets(&["*.json".to_string()]).unwrap();
        let result = filter_outputs(&outputs, includes.as_ref(), excludes.as_ref());
        assert_eq!(result, vec!["predictions.json"]);
    }

    #[test]
    fn test_filter_outputs_multiple_includes() {
        let outputs = vec![
            "predictions.json".to_string(),
            "model.pt".to_string(),
            "data.csv".to_string(),
        ];
        let (includes, excludes) =
            build_filter_glob_sets(&["*.json".to_string(), "*.csv".to_string()]).unwrap();
        let result = filter_outputs(&outputs, includes.as_ref(), excludes.as_ref());
        assert_eq!(result, vec!["predictions.json", "data.csv"]);
    }

    #[test]
    fn test_filter_outputs_exclude_only() {
        let outputs = vec![
            "predictions.json".to_string(),
            "checkpoints/model.pt".to_string(),
            "checkpoints/final.pt".to_string(),
        ];
        let (includes, excludes) =
            build_filter_glob_sets(&["!checkpoints/**".to_string()]).unwrap();
        let result = filter_outputs(&outputs, includes.as_ref(), excludes.as_ref());
        assert_eq!(result, vec!["predictions.json"]);
    }

    #[test]
    fn test_filter_outputs_include_and_exclude() {
        let outputs = vec![
            "results/predictions.json".to_string(),
            "results/debug.json".to_string(),
            "checkpoints/model.json".to_string(),
        ];
        let (includes, excludes) =
            build_filter_glob_sets(&["*.json".to_string(), "!checkpoints/**".to_string()]).unwrap();
        let result = filter_outputs(&outputs, includes.as_ref(), excludes.as_ref());
        assert_eq!(
            result,
            vec!["results/predictions.json", "results/debug.json"]
        );
    }

    #[test]
    fn test_filter_outputs_directory_with_trailing_slash() {
        let outputs = vec!["output/".to_string(), "checkpoints/".to_string()];
        let (includes, excludes) = build_filter_glob_sets(&["output".to_string()]).unwrap();
        let result = filter_outputs(&outputs, includes.as_ref(), excludes.as_ref());
        assert_eq!(result, vec!["output/"]);
    }

    #[test]
    fn test_build_filter_glob_sets_invalid_pattern() {
        let result = build_filter_glob_sets(&["[invalid".to_string()]);
        assert!(result.is_err());
    }
}
