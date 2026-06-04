//! File synchronization using rsync.
//!
//! This module provides functions for syncing files between the local machine
//! and a remote host using rsync.

use crate::error::{FlecheError, Result};
use crate::ssh::ssh_socket_dir;
use std::path::Path;
use tokio::process::Command;

/// Returns the SSH command options for rsync.
///
/// Note: no `ssh -v` here. The verbose banner gets folded into rsync's stderr,
/// and on failure that banner (`OpenSSH_x.y ...`) would mask the real error.
fn rsync_ssh_cmd() -> String {
    // Base command with timeout and batch mode options
    let mut cmd = concat!(
        "ssh",
        " -o ClearAllForwardings=yes",
        " -o ConnectTimeout=30",
        " -o ServerAliveInterval=15",
        " -o ServerAliveCountMax=3",
        " -o BatchMode=yes",
    )
    .to_string();

    // Add ControlMaster options using shared socket directory
    let control_path = ssh_socket_dir().join("%r@%h-%p");
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

/// The rsync filters used when syncing project code (excludes `.git`,
/// respects `.gitignore`). Shared between the real sync and the dry-run lister
/// so the listed files match what would actually be transferred.
const PROJECT_FILTER_ARGS: [&str; 2] = ["--exclude=.git", "--filter=:- .gitignore"];

/// A placeholder destination for local dry-run listings. rsync requires a
/// destination argument, but `--dry-run` never writes, so this is never created.
const DRY_RUN_DEST: &str = "fleche-dry-run-placeholder/";

/// Parses rsync `--out-format=%n` output into a list of relative paths.
///
/// Directory entries (paths ending in `/`, including the `./` root) are omitted
/// so only actual files are listed.
fn parse_sync_file_list(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty() && !line.ends_with('/'))
        .map(String::from)
        .collect()
}

/// Runs rsync locally in dry-run mode and returns the files it would transfer.
///
/// No remote connection is made; the destination is a placeholder that is never
/// written because of `--dry-run`.
async fn list_sync_files(source_with_slash: &str, extra_args: &[&str]) -> Result<Vec<String>> {
    let mut cmd = Command::new("rsync");
    cmd.args(["-az", "--dry-run", "--out-format=%n"]);
    cmd.args(extra_args);
    cmd.arg(source_with_slash);
    cmd.arg(DRY_RUN_DEST);

    let output = cmd
        .output()
        .await
        .map_err(|e| FlecheError::RsyncFailed(format!("Failed to execute rsync: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FlecheError::RsyncFailed(format!("rsync failed: {stderr}")));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_sync_file_list(&stdout))
}

/// Lists the project files that would be synced to the remote workspace.
///
/// Uses the same filters as [`sync_project_to_workspace`] so the result matches
/// what an actual sync would upload to a fresh workspace.
pub async fn list_project_sync_files(source: &Path) -> Result<Vec<String>> {
    let source_str = format!("{}/", source.display());
    list_sync_files(&source_str, &PROJECT_FILTER_ARGS).await
}

/// Lists the input files that would be synced to the remote workspace.
///
/// Mirrors [`sync_inputs_to_workspace`]: directories are expanded to their
/// contents (prefixed with the input path), and plain files are listed as-is.
pub async fn list_input_sync_files(source: &Path, inputs: &[String]) -> Result<Vec<String>> {
    let mut files = Vec::new();

    for input in inputs {
        // Defense in depth: skip empty entries (see sync_inputs_to_workspace).
        if input.trim().is_empty() {
            continue;
        }

        let input_path = source.join(input);

        if input_path.is_dir() {
            let source_str = format!("{}/", input_path.display());
            let prefix = input.trim_end_matches('/');
            for file in list_sync_files(&source_str, &[]).await? {
                files.push(format!("{prefix}/{file}"));
            }
        } else {
            files.push(input.clone());
        }
    }

    Ok(files)
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
    cmd.args(["-avz", "--stats"]);
    cmd.args(PROJECT_FILTER_ARGS);

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
        // Defense in depth: an empty entry joins to the project root and would
        // make rsync sync the whole tree. resolve_job already rejects these.
        if input.trim().is_empty() {
            continue;
        }

        let input_path = source.join(input);
        let is_dir = input_path.is_dir();

        let mut cmd = Command::new("rsync");
        cmd.args(["-e", &rsync_ssh_cmd()]);
        cmd.args(["-avz", "--stats", "--mkpath"]);

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

/// Options for downloading outputs.
#[derive(Default)]
pub struct DownloadOptions {
    /// If true, show what would be downloaded without actually downloading.
    pub dry_run: bool,
}

/// Downloads outputs from the remote workspace to local.
pub async fn download_outputs(
    host: &str,
    workspace: &str,
    outputs: &[String],
    local_base: &Path,
    options: &DownloadOptions,
) -> Result<()> {
    for output in outputs {
        // Defense in depth: an empty entry would make rsync pull the entire
        // remote workspace into the local project root.
        if output.trim().is_empty() {
            continue;
        }

        let remote_path = format!("{host}:{workspace}/{output}");
        let local_path = local_base.join(output);

        // Ensure local parent directory exists (even for dry-run, rsync needs it)
        if !options.dry_run {
            if let Some(parent) = local_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        let mut cmd = Command::new("rsync");
        cmd.args(["-e", &rsync_ssh_cmd()]);
        cmd.args(["-avz"]);

        if options.dry_run {
            cmd.arg("--dry-run");
        }

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

        if options.dry_run {
            // Print rsync's dry-run output
            let stdout = String::from_utf8_lossy(&output_result.stdout);
            print!("{stdout}");
        }

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
    options: &DownloadOptions,
) -> Result<()> {
    download_outputs(host, workspace, &[path.to_string()], local_base, options).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rsync_output_with_bytes() {
        let output = r"
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
";

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

    #[tokio::test]
    async fn test_list_input_sync_files_skips_empty_entries() {
        // Regression: an empty input entry must NOT expand to the project root.
        // With only an empty entry, nothing should be listed.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tracked.txt"), "data").unwrap();
        std::fs::create_dir(dir.path().join("ignored")).unwrap();
        std::fs::write(dir.path().join("ignored/big.bin"), "huge").unwrap();

        let files = list_input_sync_files(dir.path(), &[String::new()])
            .await
            .unwrap();
        assert!(
            files.is_empty(),
            "empty input entry must not sync project files, got: {files:?}"
        );

        // A whitespace-only entry is also skipped.
        let files = list_input_sync_files(dir.path(), &["   ".to_string()])
            .await
            .unwrap();
        assert!(
            files.is_empty(),
            "whitespace entry must be skipped, got: {files:?}"
        );
    }

    #[test]
    fn test_parse_sync_file_list_filters_dirs() {
        let output = "./\n.gitignore\na.txt\nsrc/\nsrc/b.txt\n";
        let files = parse_sync_file_list(output);
        assert_eq!(files, vec![".gitignore", "a.txt", "src/b.txt"]);
    }

    #[test]
    fn test_parse_sync_file_list_empty() {
        assert!(parse_sync_file_list("").is_empty());
        assert!(parse_sync_file_list("./\nsrc/\n").is_empty());
    }

    #[test]
    fn test_human_readable_zero() {
        let stats = SyncStats { bytes_sent: 0 };
        assert_eq!(stats.human_readable(), "0 B");
    }
}
