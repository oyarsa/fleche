use thiserror::Error;

#[derive(Error, Debug)]
pub enum RjobError {
    #[error("No rjob.toml found in current directory or parents")]
    ConfigNotFound,

    #[error("Failed to parse config file: {0}")]
    ConfigParse(String),

    #[error("Job '{0}' not found. Available jobs: {1}")]
    JobNotFound(String, String),

    #[error("Duplicate job name '{0}' defined in: {1}")]
    DuplicateJob(String, String),

    #[error("Missing required field '{0}' in config")]
    MissingField(String),

    #[error("SSH connection failed: {0}")]
    SshConnection(String),

    #[error("SSH command failed: {0}")]
    SshCommand(String),

    #[error("Rsync failed: {0}")]
    RsyncFailed(String),

    #[error("Sbatch submission failed: {0}")]
    SbatchFailed(String),

    #[error("Job '{0}' not found in registry. Run `rjob list` to see available jobs.")]
    JobIdNotFound(String),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Either job-name or --command must be provided")]
    NoJobOrCommand,

    #[error("Cannot cancel job '{0}': status is {1}")]
    CannotCancel(String, String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, RjobError>;
