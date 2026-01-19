//! Configuration parsing and job resolution.
//!
//! This module handles loading the `fleche.toml` configuration file, discovering
//! job definitions (both inline and from separate files), and resolving job
//! parameters with proper precedence (global -> job -> CLI overrides).

use crate::error::{FlecheError, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Loads variables from a `.env` file if present.
///
/// Returns an empty `HashMap` if the file doesn't exist.
/// Variables are loaded as literal values (no expansion).
fn load_dotenv(project_path: &Path) -> HashMap<String, String> {
    let dotenv_path = project_path.join(".env");
    let mut vars = HashMap::new();

    if let Ok(iter) = dotenvy::from_path_iter(&dotenv_path) {
        for item in iter.flatten() {
            vars.insert(item.0, item.1);
        }
    }

    vars
}

/// Expands `${VAR}` patterns in a string.
///
/// Variables are resolved in order (highest precedence first):
/// 1. Built-in variables (`PROJECT`)
/// 2. The provided context (previously expanded config values)
/// 3. System environment variables
/// 4. Variables from `.env` file
///
/// Supports `${VAR:-default}` syntax for default values when a variable is undefined.
fn expand_variables(
    value: &str,
    project_name: &str,
    context: &IndexMap<String, String>,
    dotenv: &HashMap<String, String>,
) -> Result<String> {
    shellexpand::env_with_context(
        value,
        |var| -> std::result::Result<Option<Cow<'_, str>>, std::convert::Infallible> {
            Ok(
                // 1. Built-in variables
                if var == "PROJECT" {
                    Some(Cow::Owned(project_name.to_string()))
                } else {
                    None
                }
                // 2. Previously-defined [env] entries
                .or_else(|| context.get(var).map(|v| Cow::Borrowed(v.as_str())))
                // 3. System environment variables
                .or_else(|| std::env::var(var).ok().map(Cow::Owned))
                // 4. .env file
                .or_else(|| dotenv.get(var).map(|v| Cow::Owned(v.clone()))),
            )
        },
    )
    .map(std::borrow::Cow::into_owned)
    .map_err(|e| FlecheError::ConfigParse(format!("variable expansion failed: {e}")))
}

/// Expands variables in an env map, allowing earlier entries to be referenced by later ones.
fn expand_env_map(
    env: IndexMap<String, String>,
    project_name: &str,
    dotenv: &HashMap<String, String>,
) -> Result<IndexMap<String, String>> {
    let mut expanded = IndexMap::new();
    for (key, value) in env {
        let expanded_value = expand_variables(&value, project_name, &expanded, dotenv)?;
        expanded.insert(key, expanded_value);
    }
    Ok(expanded)
}

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
///
/// All string fields store raw (unexpanded) values. Variable expansion happens
/// in `resolve_job` after merging with CLI overrides, ensuring `--env` takes precedence.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobDefinition {
    /// The shell command to execute (raw, unexpanded).
    pub command: Option<String>,
    /// Input paths to sync to a shared cache (raw, unexpanded).
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Output paths to sync back after completion (raw, unexpanded).
    #[serde(default)]
    pub outputs: Vec<String>,
    /// Slurm configuration for this job.
    #[serde(default)]
    pub slurm: SlurmConfig,
    /// Environment variables specific to this job (raw, unexpanded).
    #[serde(default)]
    pub env: IndexMap<String, String>,
    /// Host to run on (defaults to remote.host, use "local" for local execution).
    pub host: Option<String>,
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
    pub env: IndexMap<String, String>,
    /// Target host ("local" for local execution, otherwise remote host).
    pub host: String,
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
    /// Global environment variables applied to all jobs (raw, unexpanded).
    /// Variable expansion happens in `resolve_job` after merging with CLI overrides.
    pub global_env: IndexMap<String, String>,
    /// Variables loaded from .env file (for expansion lookups).
    dotenv: HashMap<String, String>,
    /// Global Slurm configuration inherited by all jobs.
    pub global_slurm: SlurmConfig,
    /// All job definitions indexed by name (raw, unexpanded).
    pub jobs: HashMap<String, JobDefinition>,
}

/// Raw config structure for TOML deserialization.
#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    project: ProjectConfig,
    remote: Option<RemoteConfig>,
    #[serde(default)]
    env: IndexMap<String, String>,
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
    env: IndexMap<String, String>,
    host: Option<String>,
}

impl Config {
    /// Finds fleche.toml in the current directory or parents and loads it.
    pub fn find_and_load() -> Result<Config> {
        let config_path = find_config_file()?;
        Self::load_from_path(&config_path)
    }

    /// Loads configuration from a specific path.
    ///
    /// Parses TOML and loads job definitions. Variable expansion (`${VAR}` patterns)
    /// is deferred to `resolve_job` so that CLI `--env` overrides take precedence.
    pub fn load_from_path(config_path: &Path) -> Result<Config> {
        let project_path = config_path
            .parent()
            .ok_or_else(|| FlecheError::ConfigParse("Invalid config path".to_string()))?
            .to_path_buf();

        // Load .env file if present (provides defaults for variable expansion)
        let dotenv = load_dotenv(&project_path);

        let content = std::fs::read_to_string(config_path)
            .map_err(|e| FlecheError::ConfigParse(format!("Failed to read config: {e}")))?;

        let raw: RawConfig = toml::from_str(&content)
            .map_err(|e| FlecheError::ConfigParse(format!("Failed to parse TOML: {e}")))?;

        let raw_remote = raw
            .remote
            .ok_or_else(|| FlecheError::MissingField("remote".to_string()))?;

        let project_name = raw.project.name.unwrap_or_else(|| {
            project_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unnamed")
                .to_string()
        });

        // Expand remote.base_path (needed for setup, uses only global env + system env)
        let expanded_global_env = expand_env_map(raw.env.clone(), &project_name, &dotenv)?;
        let remote = RemoteConfig {
            host: raw_remote.host,
            base_path: expand_variables(
                &raw_remote.base_path,
                &project_name,
                &expanded_global_env,
                &dotenv,
            )?,
        };

        // Store raw (unexpanded) global env - expansion happens in resolve_job
        let global_env = raw.env;

        let mut jobs = raw.jobs;

        // Load jobs from fleche/ directory (stored as raw, unexpanded values)
        let fleche_dir = project_path.join("fleche");
        if fleche_dir.is_dir() {
            load_jobs_from_dir(&fleche_dir, &fleche_dir, &mut jobs)?;
        }

        Ok(Config {
            project_name,
            project_path,
            remote,
            global_env,
            dotenv,
            global_slurm: raw.slurm,
            jobs,
        })
    }

    /// Resolves a job with all overrides applied.
    ///
    /// The resolution order is:
    /// 1. Global settings from fleche.toml
    /// 2. Job definition settings
    /// 3. Command-line overrides (highest precedence)
    ///
    /// Variable expansion (`${VAR}` patterns) happens after merging, so CLI `--env`
    /// overrides are available during expansion.
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

        // Merge slurm: global -> job -> CLI
        let merged_slurm = self.global_slurm.merge(&job_def.slurm);
        let final_slurm = merged_slurm.merge(slurm_overrides);

        // Merge raw env: global -> job -> CLI (all unexpanded)
        let mut raw_env = self.global_env.clone();
        raw_env.extend(job_def.env.clone());
        for (k, v) in env_overrides {
            raw_env.insert(k.clone(), v.clone());
        }

        // Expand env variables (earlier entries can be referenced by later ones)
        let expanded_env = expand_env_map(raw_env, &self.project_name, &self.dotenv)?;

        // Expand command, inputs, and outputs using the fully merged+expanded env
        let raw_command = command_override
            .map(std::string::ToString::to_string)
            .or(job_def.command.clone())
            .ok_or_else(|| FlecheError::MissingField(format!("command for job '{name}'")))?;

        let command = expand_variables(
            &raw_command,
            &self.project_name,
            &expanded_env,
            &self.dotenv,
        )?;

        let inputs = job_def
            .inputs
            .iter()
            .map(|v| expand_variables(v, &self.project_name, &expanded_env, &self.dotenv))
            .collect::<Result<Vec<_>>>()?;

        let outputs = job_def
            .outputs
            .iter()
            .map(|v| expand_variables(v, &self.project_name, &expanded_env, &self.dotenv))
            .collect::<Result<Vec<_>>>()?;

        // Resolve host: job definition -> remote.host
        let host = job_def
            .host
            .clone()
            .unwrap_or_else(|| self.remote.host.clone());

        Ok(ResolvedJob {
            name,
            command,
            inputs,
            outputs,
            slurm: final_slurm,
            env: expanded_env,
            host,
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
                    host: raw.host,
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
base_path = "~/fleche"              # Remote base directory for all projects

[env]
# Global environment variables for all jobs
# HF_HOME = "/scratch/cache/huggingface"
# PYTHONUNBUFFERED = "1"

[slurm]
# Global Slurm defaults (inherited by all jobs)
# partition = "gpu"
# time = "4:00:00"
# gpus = 1
# cpus = 8
# memory = "32G"

# Example job definition:
# [jobs.train]
# command = "python train.py"
# inputs = ["data/"]          # gitignored files to copy to workspace
# outputs = ["checkpoints/"]  # files to download with `fleche download`
#
# [jobs.train.slurm]
# time = "24:00:00"
# gpus = 4

# Jobs can also be defined in separate files: fleche/train.toml, fleche/eval.toml
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

    #[test]
    fn test_expand_variables_from_system_env() {
        // USER is typically always set
        let context = IndexMap::new();
        let dotenv = HashMap::new();
        let result = expand_variables("/home/${USER}", "test", &context, &dotenv).unwrap();
        assert!(result.starts_with("/home/"));
        assert!(!result.contains("${"));
    }

    #[test]
    fn test_expand_variables_from_context() {
        let mut context = IndexMap::new();
        context.insert("CACHE".to_string(), "/scratch/cache".to_string());
        let dotenv = HashMap::new();
        let result = expand_variables("${CACHE}/data", "test", &context, &dotenv).unwrap();
        assert_eq!(result, "/scratch/cache/data");
    }

    #[test]
    fn test_expand_variables_context_takes_precedence() {
        // Context should take precedence over system env
        let mut context = IndexMap::new();
        context.insert("USER".to_string(), "override_user".to_string());
        let dotenv = HashMap::new();
        let result = expand_variables("${USER}", "test", &context, &dotenv).unwrap();
        assert_eq!(result, "override_user");
    }

    #[test]
    fn test_expand_variables_with_default() {
        let context = IndexMap::new();
        let dotenv = HashMap::new();
        let result =
            expand_variables("${UNDEFINED_VAR:-default_value}", "test", &context, &dotenv).unwrap();
        assert_eq!(result, "default_value");
    }

    #[test]
    fn test_expand_env_map_ordering() {
        let mut env = IndexMap::new();
        env.insert("BASE".to_string(), "/scratch".to_string());
        env.insert("CACHE".to_string(), "${BASE}/cache".to_string());
        env.insert("UV_CACHE".to_string(), "${CACHE}/uv".to_string());

        let dotenv = HashMap::new();
        let expanded = expand_env_map(env, "test", &dotenv).unwrap();

        assert_eq!(expanded.get("BASE").unwrap(), "/scratch");
        assert_eq!(expanded.get("CACHE").unwrap(), "/scratch/cache");
        assert_eq!(expanded.get("UV_CACHE").unwrap(), "/scratch/cache/uv");
    }

    #[test]
    fn test_expand_variables_no_expansion_needed() {
        let context = IndexMap::new();
        let dotenv = HashMap::new();
        let result = expand_variables("/plain/path/no/vars", "test", &context, &dotenv).unwrap();
        assert_eq!(result, "/plain/path/no/vars");
    }

    #[test]
    fn test_expand_variables_from_dotenv() {
        let context = IndexMap::new();
        let mut dotenv = HashMap::new();
        dotenv.insert("MY_VAR".to_string(), "from_dotenv".to_string());
        let result = expand_variables("${MY_VAR}", "test", &context, &dotenv).unwrap();
        assert_eq!(result, "from_dotenv");
    }

    #[test]
    fn test_expand_variables_system_env_beats_dotenv() {
        // System env should take precedence over dotenv
        let context = IndexMap::new();
        let mut dotenv = HashMap::new();
        dotenv.insert("USER".to_string(), "dotenv_user".to_string());
        let result = expand_variables("${USER}", "test", &context, &dotenv).unwrap();
        // USER from system env should win
        assert_ne!(result, "dotenv_user");
    }

    #[test]
    fn test_expand_variables_context_beats_dotenv() {
        // Context should take precedence over dotenv
        let mut context = IndexMap::new();
        context.insert("MY_VAR".to_string(), "from_context".to_string());
        let mut dotenv = HashMap::new();
        dotenv.insert("MY_VAR".to_string(), "from_dotenv".to_string());
        let result = expand_variables("${MY_VAR}", "test", &context, &dotenv).unwrap();
        assert_eq!(result, "from_context");
    }

    #[test]
    fn test_expand_variables_project_builtin() {
        let context = IndexMap::new();
        let dotenv = HashMap::new();
        let result = expand_variables("${PROJECT}", "myproject", &context, &dotenv).unwrap();
        assert_eq!(result, "myproject");
    }

    #[test]
    fn test_expand_variables_project_in_path() {
        let context = IndexMap::new();
        let dotenv = HashMap::new();
        let result =
            expand_variables("/scratch/${PROJECT}/.venv", "graphmind", &context, &dotenv).unwrap();
        assert_eq!(result, "/scratch/graphmind/.venv");
    }

    #[test]
    fn test_expand_variables_project_beats_all() {
        // PROJECT should have highest precedence
        let mut context = IndexMap::new();
        context.insert("PROJECT".to_string(), "from_context".to_string());
        let mut dotenv = HashMap::new();
        dotenv.insert("PROJECT".to_string(), "from_dotenv".to_string());
        let result = expand_variables("${PROJECT}", "builtin", &context, &dotenv).unwrap();
        assert_eq!(result, "builtin");
    }
}
