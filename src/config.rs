//! Configuration parsing and job resolution.
//!
//! This module handles loading the `fleche.toml` configuration file, discovering
//! job definitions (both inline and from separate files), and resolving job
//! parameters with proper precedence (global -> job -> CLI overrides).

use crate::error::{FlecheError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Project-level configuration from the `[project]` section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    /// Project name (defaults to directory name if not specified).
    pub name: Option<String>,
}

/// Remote host configuration from the `[remote]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    /// SSH host (hostname, IP, or ~/.ssh/config alias).
    pub host: String,
    /// Base directory on the remote host for fleche data.
    pub base_path: String,
}

/// Slurm resource configuration.
///
/// All fields are optional; unset fields inherit from the parent configuration
/// (global -> job definition -> CLI overrides).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlurmConfig {
    /// Slurm partition to submit to.
    pub partition: Option<String>,
    /// Time limit (e.g., "1:00:00" for 1 hour).
    pub time: Option<String>,
    /// Number of GPUs requested.
    pub gpus: Option<u32>,
    /// Number of CPUs per task.
    pub cpus: Option<u32>,
    /// Memory limit (e.g., "32G").
    pub memory: Option<String>,
    /// Node constraint expression.
    pub constraint: Option<String>,
    /// Number of nodes.
    pub nodes: Option<u32>,
    /// Nodes to exclude.
    pub exclude: Option<String>,
}

impl SlurmConfig {
    /// Merges this config with another, with `other` taking precedence.
    ///
    /// Fields set in `other` override fields in `self`; unset fields in `other`
    /// fall back to `self`.
    pub fn merge(&self, other: &SlurmConfig) -> SlurmConfig {
        SlurmConfig {
            partition: other.partition.clone().or_else(|| self.partition.clone()),
            time: other.time.clone().or_else(|| self.time.clone()),
            gpus: other.gpus.or(self.gpus),
            cpus: other.cpus.or(self.cpus),
            memory: other.memory.clone().or_else(|| self.memory.clone()),
            constraint: other.constraint.clone().or_else(|| self.constraint.clone()),
            nodes: other.nodes.or(self.nodes),
            exclude: other.exclude.clone().or_else(|| self.exclude.clone()),
        }
    }
}

/// A job definition from `[jobs.<name>]` or a separate `fleche/<name>.toml` file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobDefinition {
    /// The shell command to execute.
    pub command: Option<String>,
    /// Input paths to sync to a shared cache.
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Output paths to sync back after completion.
    #[serde(default)]
    pub outputs: Vec<String>,
    /// Slurm configuration for this job.
    #[serde(default)]
    pub slurm: SlurmConfig,
    /// Environment variables specific to this job.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// A fully resolved job ready for submission.
///
/// Contains all parameters needed to generate an sbatch script and submit the job,
/// with all inheritance and overrides applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedJob {
    /// Job name (from definition or "adhoc" for command-line jobs).
    pub name: String,
    /// The shell command to execute.
    pub command: String,
    /// Input paths to sync to a shared cache.
    pub inputs: Vec<String>,
    /// Output paths to sync back after completion.
    pub outputs: Vec<String>,
    /// Final Slurm configuration after all merges.
    pub slurm: SlurmConfig,
    /// Final environment variables after all merges.
    pub env: HashMap<String, String>,
}

/// The complete loaded configuration for a project.
#[derive(Debug, Clone)]
pub struct Config {
    /// Project name (for organizing jobs on the remote).
    pub project_name: String,
    /// Local path to the project directory (where fleche.toml is).
    pub project_path: PathBuf,
    /// Remote host configuration.
    pub remote: RemoteConfig,
    /// Global environment variables applied to all jobs.
    pub global_env: HashMap<String, String>,
    /// Global Slurm configuration inherited by all jobs.
    pub global_slurm: SlurmConfig,
    /// All job definitions indexed by name.
    pub jobs: HashMap<String, JobDefinition>,
}

/// Raw config structure for TOML deserialization.
#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    project: ProjectConfig,
    remote: Option<RemoteConfig>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    slurm: SlurmConfig,
    #[serde(default)]
    jobs: HashMap<String, JobDefinition>,
}

/// Raw job file structure for TOML deserialization.
#[derive(Debug, Deserialize)]
struct RawJobFile {
    command: Option<String>,
    #[serde(default)]
    inputs: Vec<String>,
    #[serde(default)]
    outputs: Vec<String>,
    #[serde(default)]
    slurm: SlurmConfig,
    #[serde(default)]
    env: HashMap<String, String>,
}

impl Config {
    /// Finds fleche.toml in the current directory or parents and loads it.
    pub fn find_and_load() -> Result<Config> {
        let config_path = find_config_file()?;
        Self::load_from_path(&config_path)
    }

    /// Loads configuration from a specific path.
    pub fn load_from_path(config_path: &Path) -> Result<Config> {
        let project_path = config_path
            .parent()
            .ok_or_else(|| FlecheError::ConfigParse("Invalid config path".to_string()))?
            .to_path_buf();

        let content = std::fs::read_to_string(config_path)
            .map_err(|e| FlecheError::ConfigParse(format!("Failed to read config: {e}")))?;

        let raw: RawConfig = toml::from_str(&content)
            .map_err(|e| FlecheError::ConfigParse(format!("Failed to parse TOML: {e}")))?;

        let remote = raw
            .remote
            .ok_or_else(|| FlecheError::MissingField("remote".to_string()))?;

        let project_name = raw.project.name.unwrap_or_else(|| {
            project_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unnamed")
                .to_string()
        });

        let mut jobs = raw.jobs;

        // Load jobs from fleche/ directory
        let fleche_dir = project_path.join("fleche");
        if fleche_dir.is_dir() {
            load_jobs_from_dir(&fleche_dir, &fleche_dir, &mut jobs)?;
        }

        Ok(Config {
            project_name,
            project_path,
            remote,
            global_env: raw.env,
            global_slurm: raw.slurm,
            jobs,
        })
    }

    /// Resolves a job with all overrides applied.
    ///
    /// The resolution order is:
    /// 1. Global settings from fleche.toml
    /// 2. Job definition settings
    /// 3. Command-line overrides
    pub fn resolve_job(
        &self,
        job_name: Option<&str>,
        command_override: Option<&str>,
        env_overrides: &[(String, String)],
        slurm_overrides: &SlurmConfig,
    ) -> Result<ResolvedJob> {
        let (name, job_def) = if let Some(name) = job_name {
            let job = self.jobs.get(name).ok_or_else(|| {
                let available: Vec<_> = self.jobs.keys().cloned().collect();
                FlecheError::JobNotFound(name.to_string(), available.join(", "))
            })?;
            (name.to_string(), job.clone())
        } else {
            // Ad-hoc job
            if command_override.is_none() {
                return Err(FlecheError::NoJobOrCommand);
            }
            ("adhoc".to_string(), JobDefinition::default())
        };

        let command = command_override
            .map(std::string::ToString::to_string)
            .or(job_def.command.clone())
            .ok_or_else(|| FlecheError::MissingField(format!("command for job '{name}'")))?;

        // Merge slurm: global -> job -> CLI
        let merged_slurm = self.global_slurm.merge(&job_def.slurm);
        let final_slurm = merged_slurm.merge(slurm_overrides);

        // Merge env: global -> job -> CLI
        let mut merged_env = self.global_env.clone();
        merged_env.extend(job_def.env.clone());
        for (k, v) in env_overrides {
            merged_env.insert(k.clone(), v.clone());
        }

        Ok(ResolvedJob {
            name,
            command,
            inputs: job_def.inputs,
            outputs: job_def.outputs,
            slurm: final_slurm,
            env: merged_env,
        })
    }

    /// Returns all job names, sorted alphabetically.
    pub fn job_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.jobs.keys().cloned().collect();
        names.sort();
        names
    }
}

/// Searches for fleche.toml starting from the current directory and going up.
fn find_config_file() -> Result<PathBuf> {
    let mut current = std::env::current_dir()
        .map_err(|e| FlecheError::ConfigParse(format!("Failed to get current directory: {e}")))?;

    loop {
        let config_path = current.join("fleche.toml");
        if config_path.exists() {
            return Ok(config_path);
        }

        if !current.pop() {
            return Err(FlecheError::ConfigNotFound);
        }
    }
}

/// Recursively loads job definitions from TOML files in the fleche/ directory.
fn load_jobs_from_dir(
    base_dir: &Path,
    current_dir: &Path,
    jobs: &mut HashMap<String, JobDefinition>,
) -> Result<()> {
    let entries = std::fs::read_dir(current_dir)
        .map_err(|e| FlecheError::ConfigParse(format!("Failed to read fleche directory: {e}")))?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            FlecheError::ConfigParse(format!("Failed to read directory entry: {e}"))
        })?;
        let path = entry.path();

        if path.is_dir() {
            load_jobs_from_dir(base_dir, &path, jobs)?;
        } else if let Some(ext) = path.extension()
            && ext == "toml"
        {
            let relative = path
                .strip_prefix(base_dir)
                .map_err(|e| FlecheError::ConfigParse(format!("Path error: {e}")))?;

            // Job name is path without .toml extension
            let job_name = relative
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/");

            if jobs.contains_key(&job_name) {
                return Err(FlecheError::DuplicateJob(
                    job_name,
                    format!("fleche.toml and {}", path.display()),
                ));
            }

            let content = std::fs::read_to_string(&path).map_err(|e| {
                FlecheError::ConfigParse(format!("Failed to read {}: {}", path.display(), e))
            })?;

            let raw: RawJobFile = toml::from_str(&content).map_err(|e| {
                FlecheError::ConfigParse(format!("Failed to parse {}: {}", path.display(), e))
            })?;

            jobs.insert(
                job_name,
                JobDefinition {
                    command: raw.command,
                    inputs: raw.inputs,
                    outputs: raw.outputs,
                    slurm: raw.slurm,
                    env: raw.env,
                },
            );
        }
    }

    Ok(())
}

/// Generates a template fleche.toml configuration file.
pub fn generate_init_config() -> &'static str {
    r#"[project]
# name = "my-project"  # Optional, defaults to directory name

[remote]
host = "cluster"                    # SSH host (from ~/.ssh/config or full address)
base_path = "~/fleche"              # Remote base directory

[env]
# Global environment variables for all jobs
# HF_HOME = "/scratch/cache/huggingface"
# PYTHONUNBUFFERED = "1"

[slurm]
# Global Slurm defaults
# partition = "cpu"
# time = "1:00:00"

# Example inline job definition:
# [jobs.example]
# command = "echo 'Hello, World!'"
# inputs = []
# outputs = []
#
# [jobs.example.slurm]
# partition = "cpu"
# time = "0:10:00"

# Jobs can also be defined in separate files under fleche/*.toml
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slurm_merge() {
        let base = SlurmConfig {
            partition: Some("cpu".to_string()),
            time: Some("1:00:00".to_string()),
            gpus: None,
            cpus: Some(4),
            memory: None,
            constraint: None,
            nodes: None,
            exclude: None,
        };

        let override_config = SlurmConfig {
            partition: Some("gpu".to_string()),
            time: None,
            gpus: Some(1),
            cpus: None,
            memory: Some("32G".to_string()),
            constraint: None,
            nodes: None,
            exclude: None,
        };

        let merged = base.merge(&override_config);

        assert_eq!(merged.partition, Some("gpu".to_string()));
        assert_eq!(merged.time, Some("1:00:00".to_string()));
        assert_eq!(merged.gpus, Some(1));
        assert_eq!(merged.cpus, Some(4));
        assert_eq!(merged.memory, Some("32G".to_string()));
    }
}
