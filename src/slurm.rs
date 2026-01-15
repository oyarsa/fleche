use crate::config::{ResolvedJob, SlurmConfig};
use crate::error::{FlecheError, Result};
use crate::registry::JobStatus;
use crate::ssh::SshClient;

pub fn generate_sbatch_script(job_id: &str, job: &ResolvedJob) -> String {
    let mut script = String::new();

    script.push_str("#!/bin/bash\n");
    script.push_str(&format!("#SBATCH --job-name={}\n", job_id));
    script.push_str("#SBATCH --output=job.out\n");
    script.push_str("#SBATCH --error=job.err\n");

    let slurm = &job.slurm;

    if let Some(ref partition) = slurm.partition {
        script.push_str(&format!("#SBATCH --partition={}\n", partition));
    }

    if let Some(ref time) = slurm.time {
        script.push_str(&format!("#SBATCH --time={}\n", time));
    }

    if let Some(gpus) = slurm.gpus {
        script.push_str(&format!("#SBATCH --gpus={}\n", gpus));
    }

    if let Some(cpus) = slurm.cpus {
        script.push_str(&format!("#SBATCH --cpus-per-task={}\n", cpus));
    }

    if let Some(ref memory) = slurm.memory {
        script.push_str(&format!("#SBATCH --mem={}\n", memory));
    }

    if let Some(ref constraint) = slurm.constraint {
        script.push_str(&format!("#SBATCH --constraint={}\n", constraint));
    }

    if let Some(nodes) = slurm.nodes {
        script.push_str(&format!("#SBATCH --nodes={}\n", nodes));
    }

    if let Some(ref exclude) = slurm.exclude {
        script.push_str(&format!("#SBATCH --exclude={}\n", exclude));
    }

    script.push('\n');

    // Environment variables
    if !job.env.is_empty() {
        script.push_str("# Environment variables\n");
        for (key, value) in &job.env {
            script.push_str(&format!("export {}=\"{}\"\n", key, escape_bash_value(value)));
        }
        script.push('\n');
    }

    // Command
    script.push_str("# Execute command\n");
    script.push_str(&job.command);
    if !job.command.ends_with('\n') {
        script.push('\n');
    }

    script
}

fn escape_bash_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
}

pub async fn submit_job(ssh: &SshClient, remote_dir: &str) -> Result<String> {
    let output = ssh
        .exec(&format!(
            "cd {} && sbatch job.sbatch",
            shell_escape(remote_dir)
        ))
        .await?;

    // Parse "Submitted batch job 12345"
    let slurm_id = output
        .lines()
        .find_map(|line| {
            if line.starts_with("Submitted batch job") {
                line.split_whitespace().last().map(|s| s.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            FlecheError::SbatchFailed(format!("Could not parse sbatch output: {}", output))
        })?;

    Ok(slurm_id)
}

pub async fn get_job_status(ssh: &SshClient, slurm_id: &str) -> Result<JobStatus> {
    // First try squeue to see if job is still in queue
    let (success, stdout, _) = ssh
        .exec_allow_failure(&format!("squeue -j {} -h -o %T", slurm_id))
        .await?;

    if success && !stdout.trim().is_empty() {
        let state = stdout.trim().to_uppercase();
        return Ok(match state.as_str() {
            "PENDING" | "CONFIGURING" | "RESV_DEL_HOLD" | "REQUEUE_FED" | "REQUEUE_HOLD"
            | "REQUEUED" | "SPECIAL_EXIT" => JobStatus::Pending,
            "RUNNING" | "COMPLETING" | "SIGNALING" | "STAGE_OUT" | "STOPPED" | "SUSPENDED" => {
                JobStatus::Running
            }
            _ => JobStatus::Running, // Default to running if in queue
        });
    }

    // Job not in queue, check sacct for final state
    let (success, stdout, _) = ssh
        .exec_allow_failure(&format!(
            "sacct -j {} -n -o State --parsable2 | head -1",
            slurm_id
        ))
        .await?;

    if success && !stdout.trim().is_empty() {
        let state = stdout.trim().to_uppercase();
        // Remove any trailing state details like "CANCELLED by 12345"
        let state = state.split_whitespace().next().unwrap_or(&state);

        return Ok(match state {
            "COMPLETED" => JobStatus::Completed,
            "FAILED" | "TIMEOUT" | "OUT_OF_MEMORY" | "NODE_FAIL" | "PREEMPTED" | "BOOT_FAIL"
            | "DEADLINE" => JobStatus::Failed,
            "CANCELLED" => JobStatus::Cancelled,
            "PENDING" => JobStatus::Pending,
            "RUNNING" => JobStatus::Running,
            _ => {
                // Unknown state, try to infer from presence of output file
                JobStatus::Failed
            }
        });
    }

    // Fallback: check if job.out exists (job likely ran)
    // This handles cases where sacct isn't available
    Err(FlecheError::Other(format!(
        "Could not determine status for slurm job {}",
        slurm_id
    )))
}

pub async fn cancel_job(ssh: &SshClient, slurm_id: &str) -> Result<()> {
    ssh.exec(&format!("scancel {}", slurm_id)).await?;
    Ok(())
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

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
