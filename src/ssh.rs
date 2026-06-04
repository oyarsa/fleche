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
//! - **Debug logging**: Use `--debug` for verbose SSH output.

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

/// Default timeout for SSH command execution.
const DEFAULT_EXEC_TIMEOUT_SECS: u64 = 60;

/// Default timeout for SSH connection establishment.
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 30;

/// Returns the path to the SSH log file (`~/.config/fleche/ssh.log`).
fn ssh_log_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("fleche").join("ssh.log"))
}

/// Returns the directory for SSH `ControlMaster` sockets.
/// Creates the directory if it doesn't exist.
///
/// Uses `/tmp/fleche-ssh-<uid>/` to keep paths short (Unix sockets have ~104 byte limit).
#[cfg(unix)]
pub fn ssh_socket_dir() -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let uid = nix::unistd::getuid();
    let dir = PathBuf::from(format!("/tmp/fleche-ssh-{uid}"));
    let _ = std::fs::create_dir_all(&dir);
    // Set permissions to owner-only (0700)
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    dir
}

/// Returns the directory for SSH `ControlMaster` sockets.
/// Creates the directory if it doesn't exist.
#[cfg(not(unix))]
pub fn ssh_socket_dir() -> PathBuf {
    let dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fleche-ssh");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Guard holding an exclusive lock that serializes `ControlMaster` creation.
///
/// While held, other `fleche` processes block in [`lock_control_master`] until
/// it is dropped, by which point this process's master socket is live and can
/// be reused. The lock releases automatically when the guard goes out of scope.
#[cfg(unix)]
pub struct ControlMasterLock {
    _lock: nix::fcntl::Flock<std::fs::File>,
}

/// Acquires a cross-process lock that serializes SSH `ControlMaster` creation
/// for `host`.
///
/// Concurrent `fleche` invocations otherwise race to create the shared master
/// socket; the losers fail before the master is accepting connections (most
/// visibly, the single-shot rsync step dies). Holding this lock around the
/// first SSH connection ensures exactly one process establishes the master
/// while the rest wait and then reuse it. Release the guard before the actual
/// transfers so they still run in parallel over the established master.
///
/// Best-effort: returns `None` if the lock file or lock cannot be obtained, in
/// which case the caller proceeds without serialization.
#[cfg(unix)]
pub fn lock_control_master(host: &str) -> Option<ControlMasterLock> {
    use nix::fcntl::{Flock, FlockArg};

    let safe_host = host.replace(['/', ':'], "_");
    let lock_path = ssh_socket_dir().join(format!("{safe_host}.lock"));
    let file = std::fs::File::create(&lock_path).ok()?;
    match Flock::lock(file, FlockArg::LockExclusive) {
        Ok(lock) => Some(ControlMasterLock { _lock: lock }),
        Err(_) => None,
    }
}

/// No-op guard on non-Unix platforms, which do not use `ControlMaster`.
#[cfg(not(unix))]
pub struct ControlMasterLock;

/// No-op on non-Unix platforms (see the Unix variant): without `ControlMaster`
/// each invocation opens its own connection, so there is no master to race on.
#[cfg(not(unix))]
pub fn lock_control_master(_host: &str) -> Option<ControlMasterLock> {
    None
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

/// Formats a timeout error with helpful context and suggestions.
fn format_timeout_error(command: &str, timeout: Duration) -> String {
    let is_sbatch = command.contains("sbatch");

    let mut msg = format!("Command timed out after {timeout:?}");

    if is_sbatch {
        msg.push_str("\n\nThis usually means the Slurm scheduler is overloaded or down.");
        msg.push_str("\nRun 'fleche ping' to check cluster status.");
    } else {
        msg.push_str("\n\nThis may indicate:");
        msg.push_str("\n  - The remote host is slow or overloaded");
        msg.push_str("\n  - Network connectivity issues");
        msg.push_str("\n  - A stale SSH connection");
    }

    msg
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
pub struct SshClient {
    /// The remote host to connect to (can be a hostname or SSH config alias).
    host: String,
    /// Enable verbose SSH output (`-v` flag).
    debug: bool,
    /// Timeout for SSH command execution.
    exec_timeout: Duration,
    /// Timeout for SSH connection establishment.
    connect_timeout: Duration,
}

impl SshClient {
    /// Creates a new SSH client for the given host with default timeouts.
    ///
    /// The host can be a hostname, IP address, or an alias defined in `~/.ssh/config`.
    /// Set `debug` to true to print verbose SSH output to terminal.
    pub fn new(host: &str, debug: bool) -> Self {
        Self::with_timeouts(
            host,
            debug,
            DEFAULT_EXEC_TIMEOUT_SECS,
            DEFAULT_CONNECT_TIMEOUT_SECS,
        )
    }

    /// Creates a new SSH client with custom timeout settings.
    ///
    /// - `exec_timeout_secs`: Maximum time for a command to complete
    /// - `connect_timeout_secs`: Maximum time to establish the SSH connection
    pub fn with_timeouts(
        host: &str,
        debug: bool,
        exec_timeout_secs: u64,
        connect_timeout_secs: u64,
    ) -> Self {
        SshClient {
            host: host.to_string(),
            debug,
            exec_timeout: Duration::from_secs(exec_timeout_secs),
            connect_timeout: Duration::from_secs(connect_timeout_secs),
        }
    }

    /// Kills the `ControlMaster` socket for this host, forcing a fresh connection.
    ///
    /// This is useful when a socket becomes stale (e.g., after network issues).
    /// No-op on non-Unix platforms where `ControlMaster` is not used.
    #[cfg(unix)]
    async fn kill_control_socket(&self) {
        let socket_dir = ssh_socket_dir();
        let control_path = socket_dir.join("%r@%h-%p");

        // Try graceful exit first
        let _ = Command::new("ssh")
            .args(["-O", "exit"])
            .args(["-o", &format!("ControlPath=\"{}\"", control_path.display())])
            .arg(&self.host)
            .output()
            .await;

        // Also try to remove any matching socket files directly
        if let Ok(mut entries) = tokio::fs::read_dir(&socket_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name();
                if name.to_string_lossy().contains(&self.host) {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                }
            }
        }

        append_to_ssh_log(
            &self.host,
            "[socket cleanup]",
            "Killed stale control socket",
        );
    }

    /// No-op on non-Unix platforms.
    #[cfg(not(unix))]
    async fn kill_control_socket(&self) {}

    /// Returns the base SSH arguments including `ControlMaster` for connection multiplexing (Unix only).
    fn ssh_args(&self) -> Vec<String> {
        let mut args = vec![
            "-o".to_string(),
            "ClearAllForwardings=yes".to_string(),
            // Timeout options to prevent hanging
            "-o".to_string(),
            format!("ConnectTimeout={}", self.connect_timeout.as_secs()),
            "-o".to_string(),
            "ServerAliveInterval=15".to_string(),
            "-o".to_string(),
            "ServerAliveCountMax=3".to_string(),
            // Disable interactive prompts (MFA, password) - fail instead of hang
            "-o".to_string(),
            "BatchMode=yes".to_string(),
        ];

        // Add `ControlMaster` options for connection multiplexing (Unix only)
        #[cfg(unix)]
        {
            let socket_dir = ssh_socket_dir();
            let control_path = socket_dir.join("%r@%h-%p");
            args.extend([
                "-o".to_string(),
                "ControlMaster=auto".to_string(),
                "-o".to_string(),
                format!("ControlPath=\"{}\"", control_path.display()),
                "-o".to_string(),
                "ControlPersist=600".to_string(),
            ]);
        }

        // Add verbose flag only in debug mode
        if self.debug {
            args.insert(0, "-v".to_string());
        }

        args
    }

    /// Executes a command on the remote host and returns its stdout.
    ///
    /// Automatically retries on connection failures with exponential backoff.
    /// If a command times out, kills the control socket and retries once.
    pub async fn exec(&self, command: &str) -> Result<String> {
        match self.exec_inner(command).await {
            Ok(result) => Ok(result),
            Err(FlecheError::SshTimeout(_)) => {
                // Timeout likely means stale socket - kill it and retry once
                self.kill_control_socket().await;
                self.exec_inner(command).await
            }
            Err(e) => Err(e),
        }
    }

    /// Inner implementation of exec with retries and timeout.
    async fn exec_inner(&self, command: &str) -> Result<String> {
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

            let output_future = Command::new("ssh")
                .args(self.ssh_args())
                .arg(&self.host)
                .arg(command)
                .output();

            let output = match tokio::time::timeout(self.exec_timeout, output_future).await {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => {
                    return Err(FlecheError::SshConnection(format!(
                        "Failed to execute ssh: {e}"
                    )));
                }
                Err(_) => {
                    append_to_ssh_log(
                        &self.host,
                        command,
                        &format!("Command timed out after {:?}", self.exec_timeout),
                    );
                    return Err(FlecheError::SshTimeout(format_timeout_error(
                        command,
                        self.exec_timeout,
                    )));
                }
            };

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

        Err(last_error.expect("loop sets last_error on retryable failures"))
    }

    /// Executes a command on the remote host, allowing non-zero exit codes.
    ///
    /// Returns a tuple of (success, stdout, stderr) regardless of exit status.
    /// Only returns an error if the SSH connection itself fails.
    /// Automatically retries on connection failures with exponential backoff.
    /// If a command times out, kills the control socket and retries once.
    pub async fn exec_allow_failure(&self, command: &str) -> Result<(bool, String, String)> {
        match self.exec_allow_failure_inner(command).await {
            Ok(result) => Ok(result),
            Err(FlecheError::SshTimeout(_)) => {
                // Timeout likely means stale socket - kill it and retry once
                self.kill_control_socket().await;
                self.exec_allow_failure_inner(command).await
            }
            Err(e) => Err(e),
        }
    }

    /// Inner implementation of `exec_allow_failure` with retries and timeout.
    async fn exec_allow_failure_inner(&self, command: &str) -> Result<(bool, String, String)> {
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

            let output_future = Command::new("ssh")
                .args(self.ssh_args())
                .arg(&self.host)
                .arg(command)
                .output();

            let output = match tokio::time::timeout(self.exec_timeout, output_future).await {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => {
                    return Err(FlecheError::SshConnection(format!(
                        "Failed to execute ssh: {e}"
                    )));
                }
                Err(_) => {
                    append_to_ssh_log(
                        &self.host,
                        command,
                        &format!("Command timed out after {:?}", self.exec_timeout),
                    );
                    return Err(FlecheError::SshTimeout(format_timeout_error(
                        command,
                        self.exec_timeout,
                    )));
                }
            };

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
        let output_future = Command::new("ssh")
            .args(self.ssh_args())
            .arg(&self.host)
            .arg(command)
            .output();

        let output = match tokio::time::timeout(self.exec_timeout, output_future).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Err(FlecheError::SshConnection(format!(
                    "Failed to execute ssh: {e}"
                )));
            }
            Err(_) => {
                return Err(FlecheError::SshTimeout(format_timeout_error(
                    command,
                    self.exec_timeout,
                )));
            }
        };

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

    /// Checks if a path is a directory on the remote host.
    ///
    /// Returns `Ok(true)` if it's a directory, `Ok(false)` if it's a file,
    /// and an error if the path doesn't exist or can't be accessed.
    pub async fn is_dir(&self, path: &str) -> Result<bool> {
        let (success, _, _) = self
            .exec_allow_failure(&format!("test -d {}", shell_escape(path)))
            .await?;
        Ok(success)
    }

    /// Lists all files recursively under a directory on the remote host.
    ///
    /// Returns paths relative to the given directory.
    /// Returns an empty list if the path is not a directory.
    pub async fn list_files_recursive(&self, path: &str) -> Result<Vec<String>> {
        let output = self
            .exec(&format!(
                "find {} -type f 2>/dev/null || true",
                shell_escape(path)
            ))
            .await?;

        // Strip the path prefix to get relative paths (POSIX-compatible alternative to -printf '%P')
        let prefix = format!("{}/", path.trim_end_matches('/'));

        Ok(output
            .lines()
            .filter(|line| !line.is_empty())
            .filter_map(|line| line.strip_prefix(&prefix))
            .map(String::from)
            .collect())
    }

    /// Spawns a process that follows one or more files on the remote host.
    ///
    /// Uses `tail -F -n +1` which will retry if the file doesn't exist yet
    /// and start from the beginning of the file. This ensures no output is
    /// missed even if the job writes output before tail connects.
    /// Stderr is suppressed to hide "file doesn't exist" messages during
    /// the retry period (unless debug mode).
    ///
    /// The child process's stdout is inherited by the current process.
    pub fn tail_follow(&self, paths: &[&str]) -> Result<tokio::process::Child> {
        // In debug mode, show stderr for SSH verbose output
        let stderr_cfg = if self.debug {
            Stdio::inherit()
        } else {
            Stdio::null()
        };

        let escaped: Vec<String> = paths.iter().map(|p| shell_escape(p)).collect();
        let paths_arg = escaped.join(" ");

        // -F: follow by name (retry if file doesn't exist)
        // -n +1: start from line 1 (beginning of file, not end)
        // -q: suppress headers when following multiple files
        let child = Command::new("ssh")
            .args(self.ssh_args())
            .arg(&self.host)
            .arg(format!("tail -F -n +1 -q {paths_arg} 2>/dev/null"))
            .stdout(Stdio::inherit())
            .stderr(stderr_cfg)
            .spawn()
            .map_err(|e| FlecheError::SshConnection(format!("Failed to spawn ssh: {e}")))?;

        Ok(child)
    }
}

/// Escapes a string for safe use in a shell command.
///
/// Shell-escapes a string for safe use in remote SSH commands.
///
/// Handles two kinds of shell expansion:
/// - Tilde: `~/path` becomes `~/'path'` so `~` is expanded by the shell
/// - Variables: `/path/${VAR}/rest` becomes `'/path/'${VAR}'/rest'` so
///   `${VAR}` references are expanded by the remote shell
///
/// All other content is single-quoted to prevent injection.
pub fn shell_escape(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        format!("~/{}", quote_with_vars(rest))
    } else {
        quote_with_vars(s)
    }
}

/// Single-quotes a string, leaving `${...}` references unquoted for shell expansion.
fn quote_with_vars(s: &str) -> String {
    // Split the input into segments: literal text and ${...} variable references.
    // Literal segments are single-quoted; variable references are left bare.
    let mut segments: Vec<String> = Vec::new();
    let mut rest = s;

    while let Some(start) = rest.find("${") {
        if let Some(end) = rest[start..].find('}') {
            let literal = &rest[..start];
            if !literal.is_empty() {
                segments.push(quote_single(literal));
            }
            segments.push(rest[start..=(start + end)].to_string());
            rest = &rest[start + end + 1..];
        } else {
            break;
        }
    }

    if !rest.is_empty() {
        segments.push(quote_single(rest));
    }

    if segments.is_empty() {
        "''".to_string()
    } else {
        segments.join("")
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
    fn test_quote_with_vars_simple() {
        assert_eq!(quote_with_vars("hello"), "'hello'");
        assert_eq!(quote_with_vars("path/to/file"), "'path/to/file'");
    }

    #[test]
    fn test_quote_with_vars_spaces() {
        assert_eq!(quote_with_vars("hello world"), "'hello world'");
    }

    #[test]
    fn test_quote_with_vars_single_quotes() {
        assert_eq!(quote_with_vars("it's"), "'it'\\''s'");
    }

    #[cfg(unix)]
    #[test]
    fn control_master_lock_acquires_and_releases() {
        // A fresh lock can be taken, and after the guard drops a new lock for
        // the same host can be taken again.
        let first = lock_control_master("lock-test-host");
        assert!(first.is_some());
        drop(first);

        let second = lock_control_master("lock-test-host");
        assert!(second.is_some());
    }

    #[test]
    fn test_quote_with_vars_empty() {
        assert_eq!(quote_with_vars(""), "''");
    }

    #[test]
    fn test_quote_with_vars_variable() {
        assert_eq!(
            quote_with_vars("/scratch/${SSH_USER}/fleche"),
            "'/scratch/'${SSH_USER}'/fleche'"
        );
    }

    #[test]
    fn test_quote_with_vars_multiple() {
        assert_eq!(quote_with_vars("${A}/mid/${B}"), "${A}'/mid/'${B}");
    }

    #[test]
    fn test_quote_with_vars_unclosed_brace() {
        // Unclosed ${... is treated as literal
        assert_eq!(quote_with_vars("${NOPE"), "'${NOPE'");
    }

    #[test]
    fn test_shell_escape_simple() {
        assert_eq!(shell_escape("hello"), "'hello'");
        assert_eq!(shell_escape("/path/to/file"), "'/path/to/file'");
    }

    #[test]
    fn test_shell_escape_tilde_expansion() {
        assert_eq!(shell_escape("~/path"), "~/'path'");
        assert_eq!(shell_escape("~/path/to/file"), "~/'path/to/file'");
    }

    #[test]
    fn test_shell_escape_tilde_not_at_start() {
        assert_eq!(shell_escape("/home/~user"), "'/home/~user'");
        assert_eq!(shell_escape("some~path"), "'some~path'");
    }

    #[test]
    fn test_shell_escape_special_chars() {
        assert_eq!(shell_escape("file with spaces"), "'file with spaces'");
        // Bare $var (no braces) stays quoted — only ${...} is unquoted
        assert_eq!(shell_escape("file$var"), "'file$var'");
        assert_eq!(shell_escape("file;cmd"), "'file;cmd'");
    }

    #[test]
    fn test_shell_escape_tilde_with_special_chars() {
        assert_eq!(shell_escape("~/my files"), "~/'my files'");
        assert_eq!(shell_escape("~/path's"), "~/'path'\\''s'");
    }

    #[test]
    fn test_shell_escape_variable_in_path() {
        assert_eq!(
            shell_escape("/scratch/users/${SSH_USER}/fleche"),
            "'/scratch/users/'${SSH_USER}'/fleche'"
        );
    }

    #[test]
    fn test_shell_escape_tilde_with_variable() {
        assert_eq!(shell_escape("~/${USER}/fleche"), "~/${USER}'/fleche'");
    }
}
