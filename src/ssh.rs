use crate::error::{FlecheError, Result};
use std::process::Stdio;
use tokio::process::Command;

pub struct SshClient {
    host: String,
}

impl SshClient {
    pub fn new(host: &str) -> Self {
        SshClient {
            host: host.to_string(),
        }
    }

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

    pub async fn mkdir(&self, path: &str) -> Result<()> {
        self.exec(&format!("mkdir -p {}", shell_escape(path)))
            .await?;
        Ok(())
    }

    pub async fn rm_rf(&self, path: &str) -> Result<()> {
        self.exec(&format!("rm -rf {}", shell_escape(path))).await?;
        Ok(())
    }

    pub async fn write_file(&self, path: &str, content: &str) -> Result<()> {
        // Use heredoc to write content
        let command = format!(
            "cat > {} << 'RJOB_EOF'\n{}\nRJOB_EOF",
            shell_escape(path),
            content
        );
        self.exec(&command).await?;
        Ok(())
    }

    pub async fn cat(&self, path: &str) -> Result<String> {
        self.exec(&format!("cat {}", shell_escape(path))).await
    }

    pub fn tail_follow(&self, path: &str) -> Result<tokio::process::Child> {
        let child = Command::new("ssh")
            .arg(&self.host)
            .arg(format!("tail -f {}", shell_escape(path)))
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| FlecheError::SshConnection(format!("Failed to spawn ssh: {e}")))?;

        Ok(child)
    }

    #[allow(dead_code)]
    pub async fn file_exists(&self, path: &str) -> Result<bool> {
        let (success, _, _) = self
            .exec_allow_failure(&format!("test -f {}", shell_escape(path)))
            .await?;
        Ok(success)
    }

    pub async fn symlink(&self, target: &str, link_path: &str) -> Result<()> {
        // Remove existing file/link if present, then create symlink
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

fn shell_escape(s: &str) -> String {
    // Handle tilde expansion: ~/... -> ~/'...' (tilde must be unquoted to expand)
    if let Some(rest) = s.strip_prefix("~/") {
        format!("~/{}", quote_single(rest))
    } else {
        quote_single(s)
    }
}

fn quote_single(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
