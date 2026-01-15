//! File synchronization using rsync.
//!
//! This module provides functions for syncing files between the local machine
//! and a remote host using rsync.

use crate::error::{FlecheError, Result};
use std::path::Path;
use tokio::process::Command;

/// Returns the SSH command options for rsync.
fn rsync_ssh_cmd() -> String {
    // Base command with timeout and batch mode options
    let mut cmd = concat!(
        "ssh -v",
        " -o ClearAllForwardings=yes",
        " -o ConnectTimeout=30",
        " -o ServerAliveInterval=15",
        " -o ServerAliveCountMax=3",
        " -o BatchMode=yes",
    )
    .to_string();

    // Add ControlMaster options using short /tmp path (Unix sockets have ~104 byte limit)
    let uid = unsafe { libc::getuid() };
    let socket_dir = std::path::PathBuf::from(format!("/tmp/fleche-ssh-{uid}"));
    let _ = std::fs::create_dir_all(&socket_dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&socket_dir, std::fs::Permissions::from_mode(0o700));
    }
    let control_path = socket_dir.join("%r@%h-%p");
    cmd.push_str(&format!(
        " -o ControlMaster=auto -o 'ControlPath={}' -o ControlPersist=600",
        control_path.display()
    ));

    cmd
}

/// Statistics from an rsync transfer.
pub struct SyncStats {
    /// The number of bytes sent during the transfer.
    pub bytes_sent: u64,
}

impl SyncStats {
    /// Parses transfer statistics from rsync's `--stats` output.
    fn parse_from_rsync_output(output: &str) -> Self {
        let bytes_sent = output
            .lines()
            .find(|line| line.starts_with("Total bytes sent:"))
            .and_then(|line| {
                line.strip_prefix("Total bytes sent:")
                    .map(|s| s.trim().replace(',', "").parse().unwrap_or(0))
            })
            .unwrap_or(0);
        Self { bytes_sent }
    }

    /// Formats the byte count as a human-readable string (e.g., "1.5 MB").
    #[allow(clippy::cast_precision_loss)]
    pub fn human_readable(&self) -> String {
        const KB: u64 = 1024;
        const MB: u64 = 1024 * KB;
        const GB: u64 = 1024 * MB;

        if self.bytes_sent >= GB {
            format!("{:.1} GB", self.bytes_sent as f64 / GB as f64)
        } else if self.bytes_sent >= MB {
            format!("{:.1} MB", self.bytes_sent as f64 / MB as f64)
        } else if self.bytes_sent >= KB {
            format!("{:.1} KB", self.bytes_sent as f64 / KB as f64)
        } else {
            format!("{} B", self.bytes_sent)
        }
    }
}

/// Syncs project files to the remote workspace.
///
/// Uses rsync with compression, archive mode, and respects `.gitignore`.
pub async fn sync_project_to_workspace(
    source: &Path,
    host: &str,
    workspace: &str,
) -> Result<SyncStats> {
    let mut cmd = Command::new("rsync");
    cmd.args(["-e", &rsync_ssh_cmd()]);
    cmd.args([
        "-avz",
        "--stats",
        "--exclude=.git",
        "--filter=:- .gitignore",
    ]);

    // Ensure source path ends with / to copy contents, not the directory itself
    let source_str = format!("{}/", source.display());
    cmd.arg(&source_str);
    cmd.arg(format!("{host}:{workspace}/"));

    let output = cmd
        .output()
        .await
        .map_err(|e| FlecheError::RsyncFailed(format!("Failed to execute rsync: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FlecheError::RsyncFailed(format!("rsync failed: {stderr}")));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(SyncStats::parse_from_rsync_output(&stdout))
}

/// Syncs input files to the remote workspace.
///
/// These are typically gitignored files that need to be uploaded.
pub async fn sync_inputs_to_workspace(
    source: &Path,
    inputs: &[String],
    host: &str,
    workspace: &str,
) -> Result<SyncStats> {
    if inputs.is_empty() {
        return Ok(SyncStats { bytes_sent: 0 });
    }

    let mut total_bytes: u64 = 0;

    for input in inputs {
        let input_path = source.join(input);
        let is_dir = input_path.is_dir();

        let mut cmd = Command::new("rsync");
        cmd.args(["-e", &rsync_ssh_cmd()]);
        cmd.args(["-avz", "--stats"]);

        if is_dir {
            // For directories, ensure trailing slash to copy contents
            let source_str = format!("{}/", input_path.display());
            // Remove trailing slash from input for destination path
            let dest_path = input.trim_end_matches('/');
            cmd.arg(&source_str);
            cmd.arg(format!("{host}:{workspace}/{dest_path}/"));
        } else {
            cmd.arg(input_path.to_string_lossy().as_ref());
            // Ensure parent directory structure is preserved
            let dest_dir = Path::new(input).parent().map_or_else(
                || format!("{workspace}/"),
                |p| format!("{workspace}/{}/", p.display()),
            );
            cmd.arg(format!("{host}:{dest_dir}"));
        }

        let output = cmd
            .output()
            .await
            .map_err(|e| FlecheError::RsyncFailed(format!("Failed to execute rsync: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(FlecheError::RsyncFailed(format!(
                "rsync failed for '{input}': {stderr}"
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        total_bytes += SyncStats::parse_from_rsync_output(&stdout).bytes_sent;
    }

    Ok(SyncStats {
        bytes_sent: total_bytes,
    })
}

/// Downloads outputs from the remote workspace to local.
pub async fn download_outputs(
    host: &str,
    workspace: &str,
    outputs: &[String],
    local_base: &Path,
) -> Result<()> {
    for output in outputs {
        let remote_path = format!("{host}:{workspace}/{output}");
        let local_path = local_base.join(output);

        // Ensure local parent directory exists
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut cmd = Command::new("rsync");
        cmd.args(["-e", &rsync_ssh_cmd()]);
        cmd.args(["-avz"]);
        cmd.arg(&remote_path);

        // If path ends with /, it's a directory
        if output.ends_with('/') {
            cmd.arg(format!("{}/", local_path.display()));
        } else {
            cmd.arg(local_path.to_string_lossy().as_ref());
        }

        let output_result = cmd
            .output()
            .await
            .map_err(|e| FlecheError::RsyncFailed(format!("Failed to execute rsync: {e}")))?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            return Err(FlecheError::RsyncFailed(format!(
                "rsync failed for '{output}': {stderr}"
            )));
        }
    }

    Ok(())
}

/// Downloads a specific path from the remote workspace to local.
pub async fn download_path(
    host: &str,
    workspace: &str,
    path: &str,
    local_base: &Path,
) -> Result<()> {
    download_outputs(host, workspace, &[path.to_string()], local_base).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rsync_output_with_bytes() {
        let output = r#"
sending incremental file list
./
src/

Number of files: 42 (reg: 35, dir: 7)
Number of created files: 0
Number of deleted files: 0
Number of regular files transferred: 5
Total file size: 125,432 bytes
Total transferred file size: 12,345 bytes
Literal data: 12,345 bytes
Matched data: 0 bytes
File list size: 1,234
File list generation time: 0.001 seconds
File list transfer time: 0.000 seconds
Total bytes sent: 15,678
Total bytes received: 234

sent 15,678 bytes  received 234 bytes  31,824.00 bytes/sec
total size is 125,432  speedup is 7.88
"#;

        let stats = SyncStats::parse_from_rsync_output(output);
        assert_eq!(stats.bytes_sent, 15678);
    }

    #[test]
    fn test_parse_rsync_output_no_commas() {
        let output = "Total bytes sent: 1234\nTotal bytes received: 56";
        let stats = SyncStats::parse_from_rsync_output(output);
        assert_eq!(stats.bytes_sent, 1234);
    }

    #[test]
    fn test_parse_rsync_output_missing_line() {
        let output = "some other output\nno bytes sent line here";
        let stats = SyncStats::parse_from_rsync_output(output);
        assert_eq!(stats.bytes_sent, 0);
    }

    #[test]
    fn test_parse_rsync_output_empty() {
        let stats = SyncStats::parse_from_rsync_output("");
        assert_eq!(stats.bytes_sent, 0);
    }

    #[test]
    fn test_human_readable_bytes() {
        let stats = SyncStats { bytes_sent: 500 };
        assert_eq!(stats.human_readable(), "500 B");
    }

    #[test]
    fn test_human_readable_kilobytes() {
        let stats = SyncStats { bytes_sent: 1024 };
        assert_eq!(stats.human_readable(), "1.0 KB");

        let stats = SyncStats { bytes_sent: 1536 };
        assert_eq!(stats.human_readable(), "1.5 KB");

        let stats = SyncStats {
            bytes_sent: 500_000,
        };
        assert_eq!(stats.human_readable(), "488.3 KB");
    }

    #[test]
    fn test_human_readable_megabytes() {
        let stats = SyncStats {
            bytes_sent: 1024 * 1024,
        };
        assert_eq!(stats.human_readable(), "1.0 MB");

        let stats = SyncStats {
            bytes_sent: 5 * 1024 * 1024 + 512 * 1024,
        };
        assert_eq!(stats.human_readable(), "5.5 MB");
    }

    #[test]
    fn test_human_readable_gigabytes() {
        let stats = SyncStats {
            bytes_sent: 1024 * 1024 * 1024,
        };
        assert_eq!(stats.human_readable(), "1.0 GB");

        let stats = SyncStats {
            bytes_sent: 2 * 1024 * 1024 * 1024 + 256 * 1024 * 1024,
        };
        // 2.25 GB rounds to 2.2 with banker's rounding (round half to even)
        assert_eq!(stats.human_readable(), "2.2 GB");
    }

    #[test]
    fn test_human_readable_zero() {
        let stats = SyncStats { bytes_sent: 0 };
        assert_eq!(stats.human_readable(), "0 B");
    }
}
