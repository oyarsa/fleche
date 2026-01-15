//! SSH client for executing commands on remote hosts.
//!
//! This module provides a simple SSH client that wraps the system's `ssh` command
//! to execute commands on remote hosts. It handles shell escaping and provides
//! convenience methods for common file operations.
//!
//! ## Features
//!
//! - **Connection multiplexing**: Uses SSH `ControlMaster` to share connections,
//!   avoiding rate limiting issues with concurrent commands.
//! - **Automatic retries**: Retries failed connections with exponential backoff.
//! - **Verbose logging**: All SSH output logged to `~/.config/fleche/ssh.log`.

use crate::error::{FlecheError, Result};
use chrono::Utc;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Maximum number of retry attempts for SSH commands.
const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (doubles each retry).
const RETRY_BASE_DELAY: Duration = Duration::from_secs(1);

/// Returns the path to the SSH log file (`~/.config/fleche/ssh.log`).
fn ssh_log_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("fleche").join("ssh.log"))
}

/// Returns the directory for SSH `ControlMaster` sockets.
/// Creates the directory if it doesn't exist.
fn ssh_socket_dir() -> Option<PathBuf> {
    let dir = dirs::config_dir().map(|p| p.join("fleche").join("ssh-sockets"))?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

/// Checks if an SSH error looks like a connection/auth failure that might succeed on retry.
fn is_retryable_error(stderr: &str) -> bool {
    stderr.contains("Permission denied")
        || stderr.contains("Connection refused")
        || stderr.contains("Connection reset")
        || stderr.contains("Connection timed out")
        || stderr.contains("No route to host")
        || stderr.contains("Host is down")
}

/// Appends SSH verbose output to the log file.
fn append_to_ssh_log(host: &str, command: &str, stderr: &str) {
    let Some(log_path) = ssh_log_path() else {
        return;
    };

    // Ensure parent directory exists
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Truncate log if it's too large (> 1MB)
    if let Ok(metadata) = std::fs::metadata(&log_path) {
        if metadata.len() > 1_000_000 {
            let _ = File::create(&log_path); // Truncate
        }
    }

    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) else {
        return;
    };

    let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    let _ = writeln!(file, "\n=== [{timestamp}] ssh {host} {command} ===");
    let _ = writeln!(file, "{stderr}");
}

/// A client for executing commands on a remote host via SSH.
///
/// Uses the system's `ssh` command under the hood, so SSH keys and config
/// should be set up in `~/.ssh/config` for passwordless authentication.
///
/// All SSH commands run with `-v` for verbose output, which is logged to
/// `~/.config/fleche/ssh.log` for debugging connection issues.
pub struct SshClient {
    /// The remote host to connect to (can be a hostname or SSH config alias).
    host: String,
    /// Print verbose SSH output to terminal (in addition to logging).
    debug: bool,
}

impl SshClient {
    /// Creates a new SSH client for the given host.
    ///
    /// The host can be a hostname, IP address, or an alias defined in `~/.ssh/config`.
    /// Set `debug` to true to print verbose SSH output to terminal.
    pub fn new(host: &str, debug: bool) -> Self {
        SshClient {
            host: host.to_string(),
            debug,
        }
    }

    /// Returns the base SSH arguments including `ControlMaster` for connection multiplexing.
    #[allow(clippy::unused_self)]
    fn ssh_args(&self) -> Vec<String> {
        let mut args = vec![
            "-v".to_string(),
            "-o".to_string(),
            "ClearAllForwardings=yes".to_string(),
        ];

        // Add `ControlMaster` options if we can create the socket directory
        if let Some(socket_dir) = ssh_socket_dir() {
            let control_path = socket_dir.join("%r@%h-%p");
            args.extend([
                "-o".to_string(),
                "ControlMaster=auto".to_string(),
                "-o".to_string(),
                format!("ControlPath={}", control_path.display()),
                "-o".to_string(),
                "ControlPersist=600".to_string(),
            ]);
        }

        args
    }

    /// Executes a command on the remote host and returns its stdout.
    ///
    /// Automatically retries on connection failures with exponential backoff.
    /// SSH verbose output is always logged to `~/.config/fleche/ssh.log`.
    pub async fn exec(&self, command: &str) -> Result<String> {
        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = RETRY_BASE_DELAY * 2_u32.pow(attempt - 1);
                append_to_ssh_log(
                    &self.host,
                    command,
                    &format!("Retry attempt {attempt}/{MAX_RETRIES} after {delay:?}"),
                );
                tokio::time::sleep(delay).await;
            }

            let output = Command::new("ssh")
                .args(self.ssh_args())
                .arg(&self.host)
                .arg(command)
                .output()
                .await
                .map_err(|e| FlecheError::SshConnection(format!("Failed to execute ssh: {e}")))?;

            let stderr = String::from_utf8_lossy(&output.stderr);
            append_to_ssh_log(&self.host, command, &stderr);

            if self.debug {
                eprint!("{stderr}");
            }

            if output.status.success() {
                return Ok(String::from_utf8_lossy(&output.stdout).to_string());
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let error = FlecheError::SshCommand(format!(
                "Command failed with exit code {:?}\nstdout: {}\nstderr: {}",
                output.status.code(),
                stdout,
                stderr
            ));

            // Only retry on connection/auth errors, not command failures
            if !is_retryable_error(&stderr) {
                return Err(error);
            }

            last_error = Some(error);
        }

        Err(last_error.unwrap())
    }

    /// Executes a command on the remote host, allowing non-zero exit codes.
    ///
    /// Returns a tuple of (success, stdout, stderr) regardless of exit status.
    /// Only returns an error if the SSH connection itself fails.
    /// Automatically retries on connection failures with exponential backoff.
    /// SSH verbose output is always logged to `~/.config/fleche/ssh.log`.
    pub async fn exec_allow_failure(&self, command: &str) -> Result<(bool, String, String)> {
        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = RETRY_BASE_DELAY * 2_u32.pow(attempt - 1);
                append_to_ssh_log(
                    &self.host,
                    command,
                    &format!("Retry attempt {attempt}/{MAX_RETRIES} after {delay:?}"),
                );
                tokio::time::sleep(delay).await;
            }

            let output = Command::new("ssh")
                .args(self.ssh_args())
                .arg(&self.host)
                .arg(command)
                .output()
                .await
                .map_err(|e| FlecheError::SshConnection(format!("Failed to execute ssh: {e}")))?;

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            append_to_ssh_log(&self.host, command, &stderr);

            if self.debug {
                eprint!("{stderr}");
            }

            // If SSH connection failed (not the remote command), retry
            // SSH connection errors have exit code 255
            if output.status.code() == Some(255) && is_retryable_error(&stderr) {
                continue;
            }

            return Ok((output.status.success(), stdout, stderr));
        }

        // If we get here, all retries failed - return the last attempt's result
        let output = Command::new("ssh")
            .args(self.ssh_args())
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

    /// Reads a file, optionally limiting to the last N lines.
    pub async fn cat_tail(&self, path: &str, tail: Option<usize>) -> Result<String> {
        let cmd = if let Some(n) = tail {
            format!("tail -n {n} {}", shell_escape(path))
        } else {
            format!("cat {}", shell_escape(path))
        };
        self.exec(&cmd).await
    }

    /// Spawns a process that follows a file on the remote host.
    ///
    /// Uses `tail -F` which will retry if the file doesn't exist yet,
    /// making it suitable for following log files from jobs that are
    /// still pending in the queue. Stderr is suppressed to hide "file
    /// doesn't exist" messages during the retry period (unless debug mode).
    ///
    /// The child process's stdout is inherited by the current process.
    pub fn tail_follow(&self, path: &str) -> Result<tokio::process::Child> {
        // In debug mode, show stderr for SSH verbose output
        let stderr_cfg = if self.debug {
            Stdio::inherit()
        } else {
            Stdio::null()
        };

        let child = Command::new("ssh")
            .args(self.ssh_args())
            .arg(&self.host)
            .arg(format!("tail -F {} 2>/dev/null", shell_escape(path)))
            .stdout(Stdio::inherit())
            .stderr(stderr_cfg)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quote_single_simple() {
        assert_eq!(quote_single("hello"), "'hello'");
        assert_eq!(quote_single("path/to/file"), "'path/to/file'");
    }

    #[test]
    fn test_quote_single_with_spaces() {
        assert_eq!(quote_single("hello world"), "'hello world'");
        assert_eq!(quote_single("path with spaces"), "'path with spaces'");
    }

    #[test]
    fn test_quote_single_with_single_quotes() {
        assert_eq!(quote_single("it's"), "'it'\\''s'");
        assert_eq!(quote_single("don't"), "'don'\\''t'");
    }

    #[test]
    fn test_quote_single_empty() {
        assert_eq!(quote_single(""), "''");
    }

    #[test]
    fn test_shell_escape_simple() {
        assert_eq!(shell_escape("hello"), "'hello'");
        assert_eq!(shell_escape("/path/to/file"), "'/path/to/file'");
    }

    #[test]
    fn test_shell_escape_tilde_expansion() {
        // Tilde at start should be preserved for shell expansion
        assert_eq!(shell_escape("~/path"), "~/'path'");
        assert_eq!(shell_escape("~/path/to/file"), "~/'path/to/file'");
    }

    #[test]
    fn test_shell_escape_tilde_not_at_start() {
        // Tilde not at start should be quoted normally
        assert_eq!(shell_escape("/home/~user"), "'/home/~user'");
        assert_eq!(shell_escape("some~path"), "'some~path'");
    }

    #[test]
    fn test_shell_escape_special_chars() {
        assert_eq!(shell_escape("file with spaces"), "'file with spaces'");
        assert_eq!(shell_escape("file$var"), "'file$var'");
        assert_eq!(shell_escape("file;cmd"), "'file;cmd'");
    }

    #[test]
    fn test_shell_escape_tilde_with_special_chars() {
        assert_eq!(shell_escape("~/my files"), "~/'my files'");
        assert_eq!(shell_escape("~/path's"), "~/'path'\\''s'");
    }
}
