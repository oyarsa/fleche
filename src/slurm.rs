//! Slurm workload manager integration.
//!
//! This module provides functions for generating sbatch scripts, submitting jobs
//! to Slurm, querying job status, and cancelling jobs.

use crate::config::{ResolvedJob, SlurmConfig};
use crate::error::{FlecheError, Result};
use crate::registry::JobStatus;
use crate::ssh::{SshClient, shell_escape};

/// Generates an sbatch script for a job.
///
/// The script includes:
/// - SBATCH directives for job name, output files, and resource requirements
/// - Environment variable exports
/// - cd to workspace before running the command
/// - The job command
///
/// The job runs in `workspace` but logs are written to `job_dir`.
pub fn generate_sbatch_script(
    job_id: &str,
    job: &ResolvedJob,
    workspace: &str,
    job_dir: &str,
) -> String {
    let mut script = String::new();

    script.push_str("#!/bin/bash\n");
    script.push_str(&format!(
        "#SBATCH --job-name={}\n",
        truncate_job_name(job_id)
    ));
    // Output files go to the job directory, not the workspace
    script.push_str(&format!("#SBATCH --output={job_dir}/job.out\n"));
    script.push_str(&format!("#SBATCH --error={job_dir}/job.err\n"));

    let slurm = &job.slurm;

    if let Some(ref partition) = slurm.partition {
        script.push_str(&format!("#SBATCH --partition={partition}\n"));
    }

    if let Some(ref time) = slurm.time {
        script.push_str(&format!("#SBATCH --time={time}\n"));
    }

    if let Some(gpus) = slurm.gpus {
        if gpus > 0 {
            script.push_str(&format!("#SBATCH --gpus={gpus}\n"));
        }
    }

    if let Some(cpus) = slurm.cpus {
        script.push_str(&format!("#SBATCH --cpus-per-task={cpus}\n"));
    }

    if let Some(ref memory) = slurm.memory {
        script.push_str(&format!("#SBATCH --mem={memory}\n"));
    }

    if let Some(ref constraint) = slurm.constraint {
        if !constraint.is_empty() {
            script.push_str(&format!("#SBATCH --constraint={constraint}\n"));
        }
    }

    if let Some(nodes) = slurm.nodes {
        script.push_str(&format!("#SBATCH --nodes={nodes}\n"));
    }

    if let Some(ref exclude) = slurm.exclude {
        if !exclude.is_empty() {
            script.push_str(&format!("#SBATCH --exclude={exclude}\n"));
        }
    }

    script.push('\n');

    // Environment variables (skip invalid names)
    let valid_env: Vec<_> = job
        .env
        .iter()
        .filter(|(k, _)| is_valid_env_var_name(k))
        .collect();
    if !valid_env.is_empty() {
        script.push_str("# Environment variables\n");
        for (key, value) in valid_env {
            script.push_str(&format!("export {key}=\"{}\"\n", escape_bash_value(value)));
        }
        script.push('\n');
    }

    // Change to workspace directory
    script.push_str("# Change to workspace\n");
    script.push_str(&format!("cd {}\n\n", shell_escape(workspace)));

    // Command
    script.push_str("# Execute command\n");
    script.push_str(&job.command);
    if !job.command.ends_with('\n') {
        script.push('\n');
    }

    script
}

/// Escapes special characters in a string for use in a bash double-quoted string.
fn escape_bash_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
}

/// Maximum job name length for Slurm (conservative limit).
const MAX_JOB_NAME_LENGTH: usize = 200;

/// Validates that an environment variable name is valid for bash.
///
/// Valid names start with a letter or underscore, followed by letters, digits, or underscores.
fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

/// Truncates a job name to fit within Slurm's limits.
fn truncate_job_name(name: &str) -> &str {
    if name.len() <= MAX_JOB_NAME_LENGTH {
        name
    } else {
        &name[..MAX_JOB_NAME_LENGTH]
    }
}

/// Submits a job to Slurm using sbatch.
///
/// Expects a `job.sbatch` file to exist in `remote_dir`. Returns the Slurm job ID
/// assigned to the submitted job.
///
/// If `dependency` is provided, the job will only start after the dependency
/// job completes successfully (`--dependency=afterok:<slurm_id>`).
pub async fn submit_job(
    ssh: &SshClient,
    remote_dir: &str,
    dependency: Option<&str>,
) -> Result<String> {
    let dep_flag = dependency
        .map(|slurm_id| format!(" --dependency=afterok:{slurm_id}"))
        .unwrap_or_default();

    let output = ssh
        .exec(&format!(
            "cd {} && sbatch{dep_flag} job.sbatch",
            shell_escape(remote_dir)
        ))
        .await?;

    // Parse "Submitted batch job 12345"
    let slurm_id = output
        .lines()
        .find_map(|line| {
            line.strip_prefix("Submitted batch job")
                .and_then(|rest| rest.split_whitespace().next())
                .map(str::to_string)
        })
        .ok_or_else(|| {
            FlecheError::SbatchFailed(format!("Could not parse sbatch output: {output}"))
        })?;

    Ok(slurm_id)
}

/// Parses a Slurm state string from squeue into a `JobStatus`.
///
/// squeue shows jobs that are still in the queue (pending or running).
fn parse_squeue_state(state: &str) -> JobStatus {
    match state.to_uppercase().as_str() {
        "PENDING" | "CONFIGURING" | "RESV_DEL_HOLD" | "REQUEUE_FED" | "REQUEUE_HOLD"
        | "REQUEUED" | "SPECIAL_EXIT" => JobStatus::Pending,
        "RUNNING" | "COMPLETING" | "SIGNALING" | "STAGE_OUT" | "STOPPED" | "SUSPENDED" => {
            JobStatus::Running
        }
        _ => JobStatus::Running, // Default to running if in queue
    }
}

/// Parses a Slurm state string from sacct into a `JobStatus`.
///
/// sacct shows the final state of completed jobs. Handles state strings
/// like "CANCELLED by 12345" by taking only the first word.
#[allow(clippy::match_same_arms)] // Explicit mapping of known Slurm states is clearer
fn parse_sacct_state(state: &str) -> JobStatus {
    let state = state.to_uppercase();
    // Remove any trailing state details like "CANCELLED by 12345"
    let state = state.split_whitespace().next().unwrap_or(&state);

    match state {
        "COMPLETED" => JobStatus::Completed,
        "FAILED" | "TIMEOUT" | "OUT_OF_MEMORY" | "NODE_FAIL" | "PREEMPTED" | "BOOT_FAIL"
        | "DEADLINE" => JobStatus::Failed,
        "CANCELLED" => JobStatus::Cancelled,
        "PENDING" => JobStatus::Pending,
        "RUNNING" => JobStatus::Running,
        _ => JobStatus::Failed, // Unknown state, assume failed
    }
}

/// Queries the status of a Slurm job.
///
/// First checks `squeue` to see if the job is still in the queue (pending or running).
/// If not found in the queue, falls back to `sacct` to get the final state.
pub async fn get_job_status(ssh: &SshClient, slurm_id: &str) -> Result<JobStatus> {
    let escaped_id = shell_escape(slurm_id);

    // First try squeue to see if job is still in queue
    let (success, stdout, _) = ssh
        .exec_allow_failure(&format!("squeue -j {escaped_id} -h -o %T"))
        .await?;

    if success && !stdout.trim().is_empty() {
        return Ok(parse_squeue_state(stdout.trim()));
    }

    // Job not in queue, check sacct for final state
    let (success, stdout, _) = ssh
        .exec_allow_failure(&format!(
            "sacct -j {escaped_id} -n -o State --parsable2 | head -1"
        ))
        .await?;

    if success && !stdout.trim().is_empty() {
        return Ok(parse_sacct_state(stdout.trim()));
    }

    Err(FlecheError::SlurmQueryFailed(slurm_id.to_string()))
}

/// Cancels a Slurm job using scancel.
pub async fn cancel_job(ssh: &SshClient, slurm_id: &str) -> Result<()> {
    ssh.exec(&format!("scancel {}", shell_escape(slurm_id)))
        .await?;
    Ok(())
}

/// Resource usage statistics from a completed Slurm job.
#[derive(Debug, Default)]
pub struct JobResourceUsage {
    /// Wall clock time (e.g., "01:23:45")
    pub elapsed: String,
    /// Total CPU time used (e.g., "02:30:00")
    pub total_cpu: String,
    /// Maximum memory used (e.g., "4.5G")
    pub max_rss: String,
    /// Allocated resources (e.g., "billing=8,cpu=4,gres/gpu=1,mem=16G")
    pub alloc_tres: String,
}

/// Queries resource usage for a Slurm job using sacct.
///
/// Returns usage statistics for completed jobs. Returns default values
/// if the job hasn't completed or sacct data is unavailable.
pub async fn get_job_resource_usage(ssh: &SshClient, slurm_id: &str) -> Result<JobResourceUsage> {
    let escaped_id = shell_escape(slurm_id);

    // Query sacct for resource usage (use .batch suffix for actual job step stats)
    let output = ssh
        .exec(&format!(
            "sacct -j {escaped_id}.batch -n -o Elapsed,TotalCPU,MaxRSS,AllocTRES --parsable2 2>/dev/null || \
             sacct -j {escaped_id} -n -o Elapsed,TotalCPU,MaxRSS,AllocTRES --parsable2 | head -1"
        ))
        .await?;

    let line = output.lines().next().unwrap_or("");
    let fields: Vec<&str> = line.split('|').collect();

    Ok(JobResourceUsage {
        elapsed: fields.first().unwrap_or(&"").to_string(),
        total_cpu: fields.get(1).unwrap_or(&"").to_string(),
        max_rss: fields.get(2).unwrap_or(&"").to_string(),
        alloc_tres: fields.get(3).unwrap_or(&"").to_string(),
    })
}

/// Creates a [`SlurmConfig`] from CLI arguments.
///
/// Used to pass command-line overrides for Slurm options.
pub fn slurm_config_from_cli(
    partition: Option<String>,
    time: Option<String>,
    gpus: Option<u32>,
    cpus: Option<u32>,
    memory: Option<String>,
    constraint: Option<String>,
    nodes: Option<u32>,
    exclude: Option<String>,
) -> SlurmConfig {
    SlurmConfig {
        partition,
        time,
        gpus,
        cpus,
        memory,
        constraint,
        nodes,
        exclude,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn test_escape_bash_value() {
        assert_eq!(escape_bash_value("simple"), "simple");
        assert_eq!(escape_bash_value("with\"quote"), "with\\\"quote");
        assert_eq!(escape_bash_value("with$var"), "with\\$var");
        assert_eq!(escape_bash_value("with`cmd`"), "with\\`cmd\\`");
        assert_eq!(escape_bash_value("with\\backslash"), "with\\\\backslash");
        assert_eq!(
            escape_bash_value("all\"$`\\special"),
            "all\\\"\\$\\`\\\\special"
        );
    }

    #[test]
    fn test_parse_squeue_state_pending() {
        assert_eq!(parse_squeue_state("PENDING"), JobStatus::Pending);
        assert_eq!(parse_squeue_state("pending"), JobStatus::Pending);
        assert_eq!(parse_squeue_state("CONFIGURING"), JobStatus::Pending);
        assert_eq!(parse_squeue_state("REQUEUED"), JobStatus::Pending);
    }

    #[test]
    fn test_parse_squeue_state_running() {
        assert_eq!(parse_squeue_state("RUNNING"), JobStatus::Running);
        assert_eq!(parse_squeue_state("running"), JobStatus::Running);
        assert_eq!(parse_squeue_state("COMPLETING"), JobStatus::Running);
        assert_eq!(parse_squeue_state("SUSPENDED"), JobStatus::Running);
    }

    #[test]
    fn test_parse_squeue_state_unknown_defaults_to_running() {
        // Unknown states in queue default to running
        assert_eq!(parse_squeue_state("UNKNOWN"), JobStatus::Running);
        assert_eq!(parse_squeue_state("WEIRD_STATE"), JobStatus::Running);
    }

    #[test]
    fn test_parse_sacct_state_completed() {
        assert_eq!(parse_sacct_state("COMPLETED"), JobStatus::Completed);
        assert_eq!(parse_sacct_state("completed"), JobStatus::Completed);
    }

    #[test]
    fn test_parse_sacct_state_failed() {
        assert_eq!(parse_sacct_state("FAILED"), JobStatus::Failed);
        assert_eq!(parse_sacct_state("TIMEOUT"), JobStatus::Failed);
        assert_eq!(parse_sacct_state("OUT_OF_MEMORY"), JobStatus::Failed);
        assert_eq!(parse_sacct_state("NODE_FAIL"), JobStatus::Failed);
    }

    #[test]
    fn test_parse_sacct_state_cancelled() {
        assert_eq!(parse_sacct_state("CANCELLED"), JobStatus::Cancelled);
        // Handles "CANCELLED by 12345" format
        assert_eq!(
            parse_sacct_state("CANCELLED by 12345"),
            JobStatus::Cancelled
        );
    }

    #[test]
    fn test_parse_sacct_state_unknown_defaults_to_failed() {
        assert_eq!(parse_sacct_state("UNKNOWN"), JobStatus::Failed);
        assert_eq!(parse_sacct_state("WEIRD_STATE"), JobStatus::Failed);
    }

    #[test]
    fn test_generate_sbatch_script_basic() {
        let job = ResolvedJob {
            name: "test".to_string(),
            command: "echo hello".to_string(),
            inputs: vec![],
            outputs: vec![],
            slurm: SlurmConfig::default(),
            env: IndexMap::new(),
            host: "test".to_string(),
        };

        let script = generate_sbatch_script("test-123", &job, "/workspace", "/jobs/test-123");

        assert!(script.starts_with("#!/bin/bash\n"));
        assert!(script.contains("#SBATCH --job-name=test-123"));
        assert!(script.contains("#SBATCH --output=/jobs/test-123/job.out"));
        assert!(script.contains("#SBATCH --error=/jobs/test-123/job.err"));
        assert!(script.contains("cd '/workspace'"));
        assert!(script.contains("echo hello"));
    }

    #[test]
    fn test_generate_sbatch_script_with_slurm_options() {
        let job = ResolvedJob {
            name: "test".to_string(),
            command: "python train.py".to_string(),
            inputs: vec![],
            outputs: vec![],
            slurm: SlurmConfig {
                partition: Some("gpu".to_string()),
                time: Some("8:00:00".to_string()),
                gpus: Some(2),
                cpus: Some(16),
                memory: Some("64G".to_string()),
                constraint: Some("a100".to_string()),
                nodes: Some(1),
                exclude: Some("node01".to_string()),
            },
            env: IndexMap::new(),
            host: "test".to_string(),
        };

        let script = generate_sbatch_script("train-456", &job, "/workspace", "/jobs/train-456");

        assert!(script.contains("#SBATCH --partition=gpu"));
        assert!(script.contains("#SBATCH --time=8:00:00"));
        assert!(script.contains("#SBATCH --gpus=2"));
        assert!(script.contains("#SBATCH --cpus-per-task=16"));
        assert!(script.contains("#SBATCH --mem=64G"));
        assert!(script.contains("#SBATCH --constraint=a100"));
        assert!(script.contains("#SBATCH --nodes=1"));
        assert!(script.contains("#SBATCH --exclude=node01"));
    }

    #[test]
    fn test_generate_sbatch_script_with_env_vars() {
        let mut env = IndexMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        env.insert("PATH_VAR".to_string(), "/some/path".to_string());

        let job = ResolvedJob {
            name: "test".to_string(),
            command: "echo $FOO".to_string(),
            inputs: vec![],
            outputs: vec![],
            slurm: SlurmConfig::default(),
            env,
            host: "test".to_string(),
        };

        let script = generate_sbatch_script("test-789", &job, "/ws", "/jobs/test-789");

        assert!(script.contains("export FOO=\"bar\""));
        assert!(script.contains("export PATH_VAR=\"/some/path\""));
    }

    #[test]
    fn test_generate_sbatch_script_escapes_env_values() {
        let mut env = IndexMap::new();
        env.insert("QUOTED".to_string(), "value\"with\"quotes".to_string());

        let job = ResolvedJob {
            name: "test".to_string(),
            command: "echo test".to_string(),
            inputs: vec![],
            outputs: vec![],
            slurm: SlurmConfig::default(),
            env,
            host: "test".to_string(),
        };

        let script = generate_sbatch_script("test-esc", &job, "/ws", "/jobs/test-esc");

        assert!(script.contains("export QUOTED=\"value\\\"with\\\"quotes\""));
    }
}
