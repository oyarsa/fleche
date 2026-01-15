//! SSH client for executing commands on remote hosts.
//!
//! This module provides a simple SSH client that wraps the system's `ssh` command
//! to execute commands on remote hosts. It handles shell escaping and provides
//! convenience methods for common file operations.

use crate::error::{FlecheError, Result};
use std::process::Stdio;
use tokio::process::Command;

/// A client for executing commands on a remote host via SSH.
///
/// Uses the system's `ssh` command under the hood, so SSH keys and config
/// should be set up in `~/.ssh/config` for passwordless authentication.
pub struct SshClient {
    /// The remote host to connect to (can be a hostname or SSH config alias).
    host: String,
}

impl SshClient {
    /// Creates a new SSH client for the given host.
    ///
    /// The host can be a hostname, IP address, or an alias defined in `~/.ssh/config`.
    pub fn new(host: &str) -> Self {
        SshClient {
            host: host.to_string(),
        }
    }

    /// Executes a command on the remote host and returns its stdout.
    ///
    /// Returns an error if the command exits with a non-zero status.
    pub async fn exec(&self, command: &str) -> Result<String> {
        let output = Command::new("ssh")
            .arg(&self.host)
            .arg(command)
            .output()
            .await
            .map_err(|e| FlecheError::SshConnection(format!("Failed to execute ssh: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(FlecheError::SshCommand(format!(
                "Command failed with exit code {:?}\nstdout: {}\nstderr: {}",
                output.status.code(),
                stdout,
                stderr
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Executes a command on the remote host, allowing non-zero exit codes.
    ///
    /// Returns a tuple of (success, stdout, stderr) regardless of exit status.
    /// Only returns an error if the SSH connection itself fails.
    pub async fn exec_allow_failure(&self, command: &str) -> Result<(bool, String, String)> {
        let output = Command::new("ssh")
            .arg(&self.host)
            .arg(command)
            .output()
            .await
            .map_err(|e| FlecheError::SshConnection(format!("Failed to execute ssh: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok((output.status.success(), stdout, stderr))
    }

    /// Creates a directory on the remote host, including parent directories.
    ///
    /// Equivalent to `mkdir -p <path>`.
    pub async fn mkdir(&self, path: &str) -> Result<()> {
        self.exec(&format!("mkdir -p {}", shell_escape(path)))
            .await?;
        Ok(())
    }

    /// Recursively removes a file or directory on the remote host.
    ///
    /// Equivalent to `rm -rf <path>`.
    pub async fn rm_rf(&self, path: &str) -> Result<()> {
        self.exec(&format!("rm -rf {}", shell_escape(path))).await?;
        Ok(())
    }

    /// Writes content to a file on the remote host.
    ///
    /// Uses a heredoc to safely transfer the content without shell interpretation.
    pub async fn write_file(&self, path: &str, content: &str) -> Result<()> {
        let command = format!(
            "cat > {} << 'RJOB_EOF'\n{}\nRJOB_EOF",
            shell_escape(path),
            content
        );
        self.exec(&command).await?;
        Ok(())
    }

    /// Reads the contents of a file on the remote host.
    pub async fn cat(&self, path: &str) -> Result<String> {
        self.exec(&format!("cat {}", shell_escape(path))).await
    }

    /// Spawns a process that follows a file on the remote host.
    ///
    /// Uses `tail -F` which will retry if the file doesn't exist yet,
    /// making it suitable for following log files from jobs that are
    /// still pending in the queue.
    ///
    /// The child process's stdout and stderr are inherited by the current process.
    pub fn tail_follow(&self, path: &str) -> Result<tokio::process::Child> {
        let child = Command::new("ssh")
            .arg(&self.host)
            .arg(format!("tail -F {}", shell_escape(path)))
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| FlecheError::SshConnection(format!("Failed to spawn ssh: {e}")))?;

        Ok(child)
    }

    /// Checks if a file exists on the remote host.
    #[allow(dead_code)]
    pub async fn file_exists(&self, path: &str) -> Result<bool> {
        let (success, _, _) = self
            .exec_allow_failure(&format!("test -f {}", shell_escape(path)))
            .await?;
        Ok(success)
    }

    /// Creates a symbolic link on the remote host.
    ///
    /// If a file or link already exists at `link_path`, it is removed first.
    pub async fn symlink(&self, target: &str, link_path: &str) -> Result<()> {
        self.exec(&format!(
            "rm -rf {} && ln -s {} {}",
            shell_escape(link_path),
            shell_escape(target),
            shell_escape(link_path)
        ))
        .await?;
        Ok(())
    }
}

/// Escapes a string for safe use in a shell command.
///
/// Handles tilde expansion specially: `~/path` becomes `~/'path'` so that
/// the tilde is expanded by the shell while the rest of the path is quoted.
pub fn shell_escape(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        format!("~/{}", quote_single(rest))
    } else {
        quote_single(s)
    }
}

/// Wraps a string in single quotes, escaping any existing single quotes.
fn quote_single(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
