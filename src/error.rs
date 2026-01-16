//! Error types for fleche.
//!
//! This module defines all error types that can occur during fleche operations,
//! including configuration parsing, SSH connections, rsync transfers, and Slurm
//! job submission.

use thiserror::Error;

/// All possible errors that can occur in fleche.
#[derive(Error, Debug)]
pub enum FlecheError {
    /// No fleche.toml configuration file found in the current directory or any parent.
    #[error("No fleche.toml found in current directory or parents")]
    ConfigNotFound,

    /// Failed to parse the fleche.toml configuration file.
    #[error("Failed to parse config file: {0}")]
    ConfigParse(String),

    /// The specified job name was not found in the configuration.
    #[error("Job '{0}' not found. Available jobs: {1}")]
    JobNotFound(String, String),

    /// A job name is defined multiple times in the configuration.
    #[error("Duplicate job name '{0}' defined in: {1}")]
    DuplicateJob(String, String),

    /// A required configuration field is missing.
    #[error("Missing required field '{0}' in config")]
    MissingField(String),

    /// Failed to establish an SSH connection to the remote host.
    #[error("SSH connection failed: {0}")]
    SshConnection(String),

    /// An SSH command executed on the remote host failed.
    #[error("SSH command failed: {0}")]
    SshCommand(String),

    /// An SSH command timed out (likely stale `ControlMaster` socket).
    #[error("SSH timeout: {0}")]
    SshTimeout(String),

    /// An rsync file transfer operation failed.
    #[error("Rsync failed: {0}")]
    RsyncFailed(String),

    /// Failed to submit a job to Slurm via sbatch.
    #[error("Sbatch submission failed: {0}")]
    SbatchFailed(String),

    /// The specified job ID was not found in the local registry.
    #[error("Job '{0}' not found in registry. Run `fleche status` to see available jobs.")]
    JobIdNotFound(String),

    /// Multiple jobs match the given suffix.
    #[error("Multiple jobs match '{0}':\n  {1}\nUse a longer suffix to disambiguate.")]
    AmbiguousJobId(String, String),

    /// A database operation failed.
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// An I/O operation failed.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization or deserialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Neither a job name nor a command was provided.
    #[error("Either job-name or --command must be provided")]
    NoJobOrCommand,

    /// Cannot cancel a job that is already in a terminal state.
    #[error("Cannot cancel job '{0}': status is {1}")]
    CannotCancel(String, String),

    /// A generic error for cases not covered by other variants.
    #[error("{0}")]
    Other(String),
}

/// A Result type alias using [`FlecheError`] as the error type.
pub type Result<T> = std::result::Result<T, FlecheError>;
