use crate::error::{Result, RjobError};
use std::path::Path;
use tokio::process::Command;

pub async fn sync_to_remote(
    source: &Path,
    host: &str,
    dest: &str,
    respect_gitignore: bool,
) -> Result<()> {
    let mut cmd = Command::new("rsync");
    cmd.args(["-avz", "--delete"]);

    if respect_gitignore {
        cmd.arg("--filter=:- .gitignore");
    }

    // Ensure source path ends with / to copy contents, not the directory itself
    let source_str = format!("{}/", source.display());
    cmd.arg(&source_str);
    cmd.arg(format!("{}:{}", host, dest));

    let output = cmd
        .output()
        .await
        .map_err(|e| RjobError::RsyncFailed(format!("Failed to execute rsync: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RjobError::RsyncFailed(format!(
            "rsync failed: {}",
            stderr
        )));
    }

    Ok(())
}

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
        let dest_str = format!("{}:{}/{}/", host, dest_base, relative_path);
        cmd.arg(&source_str);
        cmd.arg(&dest_str);
    } else {
        // For files, sync the file itself
        cmd.arg(source_path.to_string_lossy().as_ref());

        // Ensure parent directory exists on remote
        let parent = Path::new(relative_path).parent();
        let dest_str = if let Some(p) = parent {
            if p.as_os_str().is_empty() {
                format!("{}:{}/", host, dest_base)
            } else {
                format!("{}:{}/{}/", host, dest_base, p.display())
            }
        } else {
            format!("{}:{}/", host, dest_base)
        };
        cmd.arg(&dest_str);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| RjobError::RsyncFailed(format!("Failed to execute rsync: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RjobError::RsyncFailed(format!(
            "rsync failed for '{}': {}",
            relative_path, stderr
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
    let remote_path = format!("{}:{}/{}", host, remote_base, relative_path);
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
        .map_err(|e| RjobError::RsyncFailed(format!("Failed to execute rsync: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RjobError::RsyncFailed(format!(
            "rsync failed for '{}': {}",
            relative_path, stderr
        )));
    }

    Ok(())
}
