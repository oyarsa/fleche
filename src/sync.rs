//! File synchronization using rsync.
//!
//! This module provides functions for syncing files between the local machine
//! and a remote host using rsync. It supports both uploading project files to
//! the remote and downloading outputs back.

use crate::error::{FlecheError, Result};
use crate::ssh::SshClient;
use std::path::Path;
use tokio::process::Command;

/// Returns the SSH command for rsync with `ControlMaster` options for connection multiplexing.
fn rsync_ssh_cmd() -> String {
    let mut cmd = "ssh -v -o ClearAllForwardings=yes".to_string();

    // Add `ControlMaster` options if we can create the socket directory
    if let Some(config_dir) = dirs::config_dir() {
        let socket_dir = config_dir.join("fleche").join("ssh-sockets");
        let _ = std::fs::create_dir_all(&socket_dir);
        let control_path = socket_dir.join("%r@%h-%p");
        cmd.push_str(&format!(
            " -o ControlMaster=auto -o 'ControlPath={}' -o ControlPersist=600",
            control_path.display()
        ));
    }

    cmd
}

/// Statistics from an rsync transfer.
pub struct SyncStats {
    /// The number of bytes sent during the transfer.
    pub bytes_sent: u64,
}

impl SyncStats {
    /// Parses transfer statistics from rsync's `--stats` output.
    ///
    /// Looks for the "Total bytes sent: X" line and extracts the byte count.
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

/// Syncs a local directory to a remote host.
///
/// Uses rsync with compression (`-z`), archive mode (`-a`), and verbose output (`-v`).
/// The `--delete` flag removes files on the remote that don't exist locally.
/// The `.git` directory is always excluded.
///
/// If `respect_gitignore` is true, files matching patterns in `.gitignore` are excluded.
pub async fn sync_to_remote(
    source: &Path,
    host: &str,
    dest: &str,
    respect_gitignore: bool,
) -> Result<SyncStats> {
    let mut cmd = Command::new("rsync");
    cmd.args(["-e", &rsync_ssh_cmd()]);
    cmd.args(["-avz", "--delete", "--stats", "--exclude=.git"]);

    if respect_gitignore {
        cmd.arg("--filter=:- .gitignore");
    }

    // Ensure source path ends with / to copy contents, not the directory itself
    let source_str = format!("{}/", source.display());
    cmd.arg(&source_str);
    cmd.arg(format!("{host}:{dest}"));

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

/// Estimates how much data would be transferred without actually syncing.
///
/// Performs a dry-run rsync to calculate the transfer size. Useful for
/// showing progress information before a potentially long sync operation.
pub async fn estimate_sync_size(source: &Path, respect_gitignore: bool) -> Result<SyncStats> {
    let mut cmd = Command::new("rsync");
    cmd.args(["-avz", "--dry-run", "--stats", "--exclude=.git"]);

    if respect_gitignore {
        cmd.arg("--filter=:- .gitignore");
    }

    let source_str = format!("{}/", source.display());
    cmd.arg(&source_str);
    cmd.arg("/dev/null");

    let output = cmd
        .output()
        .await
        .map_err(|e| FlecheError::RsyncFailed(format!("Failed to execute rsync: {e}")))?;

    // Note: rsync dry-run to /dev/null may show warnings but still provides stats
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(SyncStats::parse_from_rsync_output(&stdout))
}

/// Syncs a specific path (file or directory) to the remote host.
///
/// Unlike [`sync_to_remote`], this syncs a single path relative to a base directory,
/// preserving the directory structure on the remote.
#[allow(dead_code)]
pub async fn sync_path_to_remote(
    source_base: &Path,
    relative_path: &str,
    host: &str,
    dest_base: &str,
) -> Result<()> {
    let source_path = source_base.join(relative_path);
    let is_dir = source_path.is_dir();

    let mut cmd = Command::new("rsync");
    cmd.args(["-e", &rsync_ssh_cmd()]);
    cmd.args(["-avz"]);

    if is_dir {
        // For directories, ensure trailing slash to copy contents
        let source_str = format!("{}/", source_path.display());
        let dest_str = format!("{host}:{dest_base}/{relative_path}/");
        cmd.arg(&source_str);
        cmd.arg(&dest_str);
    } else {
        // For files, sync the file itself
        cmd.arg(source_path.to_string_lossy().as_ref());

        // Ensure parent directory exists on remote
        let dest_str = if let Some(p) = Path::new(relative_path).parent()
            && !p.as_os_str().is_empty()
        {
            format!("{}:{}/{}/", host, dest_base, p.display())
        } else {
            format!("{host}:{dest_base}/")
        };
        cmd.arg(&dest_str);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| FlecheError::RsyncFailed(format!("Failed to execute rsync: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FlecheError::RsyncFailed(format!(
            "rsync failed for '{relative_path}': {stderr}"
        )));
    }

    Ok(())
}

/// Syncs a path from the remote host to the local machine.
///
/// Downloads a file or directory from `remote_base/relative_path` on the remote
/// host to `local_base/relative_path` locally. Creates parent directories as needed.
pub async fn sync_from_remote(
    host: &str,
    remote_base: &str,
    relative_path: &str,
    local_base: &Path,
) -> Result<()> {
    let remote_path = format!("{host}:{remote_base}/{relative_path}");
    let local_path = local_base.join(relative_path);

    // Ensure local parent directory exists
    if let Some(parent) = local_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut cmd = Command::new("rsync");
    cmd.args(["-e", &rsync_ssh_cmd()]);
    cmd.args(["-avz"]);
    cmd.arg(&remote_path);

    // If path ends with /, it's a directory
    if relative_path.ends_with('/') {
        cmd.arg(format!("{}/", local_path.display()));
    } else {
        // Could be file or directory - rsync handles both
        cmd.arg(local_path.to_string_lossy().as_ref());
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| FlecheError::RsyncFailed(format!("Failed to execute rsync: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FlecheError::RsyncFailed(format!(
            "rsync failed for '{relative_path}': {stderr}"
        )));
    }

    Ok(())
}

/// Calculates the relative symlink target path from a job subdirectory to the cache.
///
/// The path needs enough `..` components to navigate from the symlink location
/// back to the fleche base directory where `cache/` lives.
fn symlink_target_for_cache(normalized_path: &str) -> String {
    let depth = normalized_path.matches('/').count() + 1;
    let dotdots = "../".repeat(depth);
    format!("{dotdots}cache/{normalized_path}")
}

/// Syncs an input path to a shared cache and creates a symlink in the job directory.
///
/// This enables sharing large input files (like datasets) across multiple jobs
/// without copying them each time. The cache is stored at:
///
/// ```text
/// <fleche_base>/cache/<input-path>
/// ```
///
/// Each job directory gets a symlink pointing to the cached data. The relative
/// path depth is calculated based on the input path:
///
/// ```text
/// <fleche_base>/<job-id>/data -> ../cache/data
/// <fleche_base>/<job-id>/output/models -> ../../cache/output/models
/// ```
pub async fn sync_input_cached(
    source_base: &Path,
    relative_path: &str,
    host: &str,
    fleche_base: &str,
    job_id: &str,
    ssh: &SshClient,
) -> Result<SyncStats> {
    let source_path = source_base.join(relative_path);
    let is_dir = source_path.is_dir();

    // Normalize the path (remove trailing slashes for consistent cache keys)
    let normalized_path = relative_path.trim_end_matches('/');

    // Cache destination: .fleche/cache/<path>
    let cache_path = format!("{fleche_base}/cache/{normalized_path}");

    // Ensure cache parent directory exists
    let cache_parent = Path::new(&cache_path).parent().map_or_else(
        || format!("{fleche_base}/cache"),
        |p| p.to_string_lossy().to_string(),
    );
    ssh.mkdir(&cache_parent).await?;

    // Sync to cache
    let mut cmd = Command::new("rsync");
    cmd.args(["-e", &rsync_ssh_cmd()]);
    cmd.args(["-avz", "--stats"]);

    if is_dir {
        let source_str = format!("{}/", source_path.display());
        let dest_str = format!("{host}:{cache_path}/");
        cmd.arg(&source_str);
        cmd.arg(&dest_str);
    } else {
        cmd.arg(source_path.to_string_lossy().as_ref());
        cmd.arg(format!("{host}:{cache_path}"));
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| FlecheError::RsyncFailed(format!("Failed to execute rsync: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FlecheError::RsyncFailed(format!(
            "rsync failed for '{relative_path}': {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Create symlink in job directory pointing to cache
    let link_path = format!("{fleche_base}/{job_id}/{normalized_path}");
    let symlink_target = symlink_target_for_cache(normalized_path);

    // Ensure parent directory of symlink exists
    if let Some(parent) = Path::new(&link_path).parent()
        && !parent.as_os_str().is_empty()
    {
        ssh.mkdir(&parent.to_string_lossy()).await?;
    }

    ssh.symlink(&symlink_target, &link_path).await?;

    Ok(SyncStats::parse_from_rsync_output(&stdout))
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

    #[test]
    fn test_symlink_target_single_level() {
        // data -> ../cache/data
        assert_eq!(symlink_target_for_cache("data"), "../cache/data");
    }

    #[test]
    fn test_symlink_target_two_levels() {
        // output/models -> ../../cache/output/models
        assert_eq!(
            symlink_target_for_cache("output/models"),
            "../../cache/output/models"
        );
    }

    #[test]
    fn test_symlink_target_three_levels() {
        // output/baselines/llama_data -> ../../../cache/output/baselines/llama_data
        assert_eq!(
            symlink_target_for_cache("output/baselines/llama_data"),
            "../../../cache/output/baselines/llama_data"
        );
    }

    #[test]
    fn test_symlink_target_deep_nesting() {
        // a/b/c/d/e -> ../../../../../cache/a/b/c/d/e
        assert_eq!(
            symlink_target_for_cache("a/b/c/d/e"),
            "../../../../../cache/a/b/c/d/e"
        );
    }
}
