use crate::error::{FlecheError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub host: String,
    pub base_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlurmConfig {
    pub partition: Option<String>,
    pub time: Option<String>,
    pub gpus: Option<u32>,
    pub cpus: Option<u32>,
    pub memory: Option<String>,
    pub constraint: Option<String>,
    pub nodes: Option<u32>,
    pub exclude: Option<String>,
}

impl SlurmConfig {
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobDefinition {
    pub command: Option<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub slurm: SlurmConfig,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedJob {
    pub name: String,
    pub command: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub slurm: SlurmConfig,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub project_name: String,
    pub project_path: PathBuf,
    pub remote: RemoteConfig,
    pub global_env: HashMap<String, String>,
    pub global_slurm: SlurmConfig,
    pub jobs: HashMap<String, JobDefinition>,
}

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
    pub fn find_and_load() -> Result<Config> {
        let config_path = find_config_file()?;
        Self::load_from_path(&config_path)
    }

    pub fn load_from_path(config_path: &Path) -> Result<Config> {
        let project_path = config_path
            .parent()
            .ok_or_else(|| FlecheError::ConfigParse("Invalid config path".to_string()))?
            .to_path_buf();

        let content = std::fs::read_to_string(config_path)
            .map_err(|e| FlecheError::ConfigParse(format!("Failed to read config: {}", e)))?;

        let raw: RawConfig = toml::from_str(&content)
            .map_err(|e| FlecheError::ConfigParse(format!("Failed to parse TOML: {}", e)))?;

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

    pub fn resolve_job(
        &self,
        job_name: Option<&str>,
        command_override: Option<&str>,
        env_overrides: &[(String, String)],
        slurm_overrides: SlurmConfig,
    ) -> Result<ResolvedJob> {
        let (name, job_def) = match job_name {
            Some(name) => {
                let job = self.jobs.get(name).ok_or_else(|| {
                    let available: Vec<_> = self.jobs.keys().cloned().collect();
                    FlecheError::JobNotFound(name.to_string(), available.join(", "))
                })?;
                (name.to_string(), job.clone())
            }
            None => {
                // Ad-hoc job
                if command_override.is_none() {
                    return Err(FlecheError::NoJobOrCommand);
                }
                ("adhoc".to_string(), JobDefinition::default())
            }
        };

        let command = command_override
            .map(|s| s.to_string())
            .or(job_def.command.clone())
            .ok_or_else(|| FlecheError::MissingField(format!("command for job '{}'", name)))?;

        // Merge slurm: global -> job -> CLI
        let merged_slurm = self.global_slurm.merge(&job_def.slurm);
        let final_slurm = merged_slurm.merge(&slurm_overrides);

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

    pub fn job_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.jobs.keys().cloned().collect();
        names.sort();
        names
    }
}

fn find_config_file() -> Result<PathBuf> {
    let mut current = std::env::current_dir().map_err(|e| {
        FlecheError::ConfigParse(format!("Failed to get current directory: {}", e))
    })?;

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

fn load_jobs_from_dir(
    base_dir: &Path,
    current_dir: &Path,
    jobs: &mut HashMap<String, JobDefinition>,
) -> Result<()> {
    let entries = std::fs::read_dir(current_dir).map_err(|e| {
        FlecheError::ConfigParse(format!("Failed to read fleche directory: {}", e))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            FlecheError::ConfigParse(format!("Failed to read directory entry: {}", e))
        })?;
        let path = entry.path();

        if path.is_dir() {
            load_jobs_from_dir(base_dir, &path, jobs)?;
        } else if let Some(ext) = path.extension()
            && ext == "toml"
        {
            let relative = path
                .strip_prefix(base_dir)
                .map_err(|e| FlecheError::ConfigParse(format!("Path error: {}", e)))?;

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
