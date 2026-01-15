use crate::error::{FlecheError, Result};
use crate::ssh::SshClient;
use std::path::Path;
use tokio::process::Command;

/// Stats from an rsync transfer
pub struct SyncStats {
    pub bytes_sent: u64,
}

impl SyncStats {
    fn parse_from_rsync_output(output: &str) -> Self {
        // Parse "Total bytes sent: 1,234" from rsync --stats output
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
            format!("{} bytes", self.bytes_sent)
        }
    }
}

pub async fn sync_to_remote(
    source: &Path,
    host: &str,
    dest: &str,
    respect_gitignore: bool,
) -> Result<SyncStats> {
    let mut cmd = Command::new("rsync");
    cmd.args(["-avz", "--delete", "--stats"]);

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

/// Estimate how much data would be transferred without actually syncing
pub async fn estimate_sync_size(source: &Path, respect_gitignore: bool) -> Result<SyncStats> {
    let mut cmd = Command::new("rsync");
    // --dry-run doesn't transfer, just calculates
    // Using /dev/null as dest since we just want to measure source
    cmd.args(["-avz", "--dry-run", "--stats"]);

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

#[allow(dead_code)]
pub async fn sync_path_to_remote(
    source_base: &Path,
    relative_path: &str,
    host: &str,
    dest_base: &str,
) -> Result<()> {
    let source_path = source_base.join(relative_path);

    // Determine if it's a directory or file
    let is_dir = source_path.is_dir();

    let mut cmd = Command::new("rsync");
    cmd.args(["-avz"]);

    if is_dir {
        // For directories, ensure trailing slash
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

/// Sync an input path to a shared cache and create a symlink in the job directory.
///
/// Cache structure:
///   <`base_path`>/<project>/.fleche/cache/<input-path>
///
/// Job directory gets a symlink:
///   <`base_path`>/<project>/.fleche/<job-id>/<input-path> -> ../cache/<input-path>
pub async fn sync_input_cached(
    source_base: &Path,
    relative_path: &str,
    host: &str,
    fleche_base: &str, // e.g., ~/fleche/my-project/.fleche
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

    // Create symlink in job directory
    // Job is at .fleche/<job-id>/, cache is at .fleche/cache/
    // So symlink target is: ../cache/<path>
    let link_path = format!("{fleche_base}/{job_id}/{normalized_path}");
    let symlink_target = format!("../cache/{normalized_path}");

    // Ensure parent directory of symlink exists
    if let Some(parent) = Path::new(&link_path).parent()
        && !parent.as_os_str().is_empty()
    {
        ssh.mkdir(&parent.to_string_lossy()).await?;
    }

    ssh.symlink(&symlink_target, &link_path).await?;

    Ok(SyncStats::parse_from_rsync_output(&stdout))
}
