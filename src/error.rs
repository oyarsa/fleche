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

    /// An `inputs`/`outputs` entry resolved to an empty string, which would
    /// make rsync sync the entire project root (bypassing `.gitignore`).
    #[error(
        "Job '{job}' has an empty {field} entry at index {index} (from {raw:?}). \
         A `${{VAR}}` likely expanded to an empty string. \
         Remove the entry or set the variable to a real path."
    )]
    EmptyPathEntry {
        /// The job whose configuration contains the empty entry.
        job: String,
        /// Which list the entry came from (`inputs` or `outputs`).
        field: String,
        /// The zero-based index of the offending entry.
        index: usize,
        /// The raw (pre-expansion) value, e.g. `"${OPTIONAL}"`.
        raw: String,
    },

    /// Failed to establish an SSH connection to the remote host.
    #[error("SSH connection failed: {0}")]
    SshConnection(String),

    /// An SSH command executed on the remote host failed.
    #[error("SSH command failed: {0}")]
    SshCommand(String),

    /// An SSH command timed out (likely stale `ControlMaster` socket).
    #[error("{0}")]
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

    /// An I/O operation failed with additional context.
    #[error("{context}: {source}")]
    IoContext {
        context: String,
        #[source]
        source: std::io::Error,
    },

    /// JSON serialization or deserialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Neither a job name nor a command was provided.
    #[error("Provide a job name, or use --command for an ad hoc command")]
    NoJobOrCommand,

    /// Cannot cancel a job that is already in a terminal state.
    #[error("Cannot cancel job '{0}': status is {1}")]
    CannotCancel(String, String),

    /// No jobs found when trying to use the most recent job.
    #[error("No jobs found. Run 'fleche run' to submit a job.")]
    NoRecentJob,

    /// Invalid duration format for time-based operations.
    #[error("Invalid duration: {0}. Use format like 7d, 24h, 30m")]
    InvalidDuration(String),

    /// Invalid glob pattern for filtering files.
    #[error("Invalid glob pattern: {0}")]
    InvalidGlobPattern(String),

    /// Invalid regex pattern for filtering jobs.
    #[error("Invalid regex pattern: {0}")]
    InvalidRegexPattern(String),

    /// Unknown job status string from database.
    #[error("Unknown job status: {0}")]
    UnknownJobStatus(String),

    /// Failed to query Slurm job status.
    #[error("Could not query Slurm job status: {0}")]
    SlurmQueryFailed(String),

    /// Could not find the system config directory.
    #[error("Could not find config directory")]
    ConfigDirNotFound,

    /// Job has no Slurm ID (not yet submitted or submission failed).
    #[error("Job '{0}' has no Slurm ID")]
    NoSlurmId(String),

    /// Could not reach the Slurm controller.
    #[error("Could not reach Slurm controller")]
    SlurmUnavailable,

    /// A required external dependency is not available.
    #[error("{0}")]
    MissingDependency(String),

    /// A command-line argument had an invalid value.
    #[error("{0}")]
    InvalidArgument(String),
}

impl FlecheError {
    /// Process exit code to use when this error reaches the top level.
    ///
    /// Buckets errors into a few categories so scripts can distinguish
    /// "nothing to do" (not found) from "couldn't reach the cluster"
    /// (remote failure) from bad input (invalid argument, same code clap
    /// itself uses for usage errors) from everything else.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::ConfigNotFound
            | Self::JobNotFound(..)
            | Self::JobIdNotFound(..)
            | Self::AmbiguousJobId(..)
            | Self::NoRecentJob
            | Self::NoSlurmId(..) => 3,

            Self::SshConnection(..)
            | Self::SshCommand(..)
            | Self::SshTimeout(..)
            | Self::RsyncFailed(..)
            | Self::SbatchFailed(..)
            | Self::SlurmQueryFailed(..)
            | Self::SlurmUnavailable
            | Self::MissingDependency(..) => 4,

            Self::InvalidArgument(..)
            | Self::NoJobOrCommand
            | Self::InvalidDuration(..)
            | Self::InvalidGlobPattern(..)
            | Self::InvalidRegexPattern(..)
            | Self::CannotCancel(..) => 2,

            _ => 1,
        }
    }

    /// Whether this error represents something unanticipated (a bug or
    /// environment corruption) rather than an ordinary, actionable failure.
    ///
    /// Used to decide whether to point the user at the issue tracker.
    pub fn is_unexpected(&self) -> bool {
        matches!(
            self,
            Self::Io(..)
                | Self::IoContext { .. }
                | Self::Database(..)
                | Self::Json(..)
                | Self::ConfigDirNotFound
                | Self::UnknownJobStatus(..)
        )
    }
}

/// A Result type alias using [`FlecheError`] as the error type.
pub type Result<T> = std::result::Result<T, FlecheError>;

/// Extension trait for adding context to `std::io::Result`.
///
/// This is similar to `anyhow::Context` but specifically for I/O errors,
/// preserving the original error as a source while adding descriptive context.
///
/// # Example
///
/// ```ignore
/// use crate::error::IoResultExt;
///
/// std::fs::read_to_string(path)
///     .io_context(|| format!("reading config from '{}'", path.display()))?;
/// ```
pub trait IoResultExt<T> {
    /// Adds context to an I/O error, converting it to a `FlecheError`.
    fn io_context<C, F>(self, context: F) -> Result<T>
    where
        C: Into<String>,
        F: FnOnce() -> C;
}

impl<T> IoResultExt<T> for std::io::Result<T> {
    fn io_context<C, F>(self, context: F) -> Result<T>
    where
        C: Into<String>,
        F: FnOnce() -> C,
    {
        self.map_err(|source| FlecheError::IoContext {
            context: context().into(),
            source,
        })
    }
}
