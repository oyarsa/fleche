//! SOCKS proxy tunnel for routing traffic through the remote host.
//!
//! Opens an SSH dynamic port forward (`ssh -D`) to create a local SOCKS proxy,
//! runs a user command with proxy environment variables set, and tears down the
//! tunnel when the command exits.

use anyhow::{Context, bail};
use console::style;

/// Finds an available TCP port by binding to port 0.
fn find_available_port() -> anyhow::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .context("finding available port for SOCKS proxy")?;
    Ok(listener.local_addr()?.port())
}

/// Checks whether something is listening on the given local port.
fn is_port_listening(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// Starts an SSH SOCKS proxy tunnel, returning the child process handle.
///
/// Spawns `ssh -D <port> -N <host>` and polls until the port is accepting
/// connections. The caller owns the child and must kill it when done.
async fn start_proxy(host: &str, port: u16, debug: bool) -> anyhow::Result<std::process::Child> {
    let mut args: Vec<String> = Vec::new();

    if debug {
        args.push("-v".to_string());
    }

    // NOTE: We intentionally omit ClearAllForwardings=yes (which the regular
    // SshClient uses) because it would prevent our -D forward from working.
    // We also skip ControlMaster so the -D binding is guaranteed to be on
    // this specific connection rather than silently ignored by an existing master.
    args.extend([
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=15".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
        "-o".to_string(),
        "ExitOnForwardFailure=yes".to_string(),
        "-D".to_string(),
        format!("127.0.0.1:{port}"),
        "-N".to_string(),
        host.to_string(),
    ]);

    let stderr_cfg = if debug {
        std::process::Stdio::inherit()
    } else {
        std::process::Stdio::null()
    };

    let mut child = std::process::Command::new("ssh")
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr_cfg)
        .spawn()
        .context("spawning ssh for SOCKS proxy")?;

    // Poll until the proxy port accepts connections or SSH exits
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(30);

    loop {
        if is_port_listening(port) {
            return Ok(child);
        }

        if let Some(status) = child.try_wait()? {
            bail!(
                "SSH proxy exited with code {} before the tunnel was ready.\n\
                 Check SSH connectivity: ssh {host}",
                status
                    .code()
                    .map_or_else(|| "unknown".to_string(), |c| c.to_string()),
            );
        }

        if start.elapsed() > timeout {
            let _ = child.kill();
            bail!(
                "Timed out waiting for SOCKS proxy on port {port}.\n\
                 Check SSH connectivity: ssh {host}"
            );
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Opens a SOCKS proxy tunnel to the remote host and runs a command through it.
///
/// Sets `ALL_PROXY`, `HTTP_PROXY`, `HTTPS_PROXY` (and lowercase variants),
/// `NO_PROXY`, and fleche-specific variables on the child process. The tunnel
/// is torn down when the command exits.
///
/// Returns the child command's exit code.
pub async fn run_proxy_command(
    host: &str,
    command: &[String],
    port_override: Option<u16>,
    debug: bool,
) -> anyhow::Result<i32> {
    let port = match port_override {
        Some(p) => p,
        None => find_available_port()?,
    };

    let mut proxy = start_proxy(host, port, debug).await?;
    eprintln!(
        "{} SOCKS proxy listening on 127.0.0.1:{port}",
        style("*").cyan(),
    );

    let proxy_url = format!("socks5h://127.0.0.1:{port}");

    let result = tokio::process::Command::new(&command[0])
        .args(&command[1..])
        .env("ALL_PROXY", &proxy_url)
        .env("HTTP_PROXY", &proxy_url)
        .env("HTTPS_PROXY", &proxy_url)
        .env("http_proxy", &proxy_url)
        .env("https_proxy", &proxy_url)
        .env("NO_PROXY", "localhost,127.0.0.1,::1")
        .env("no_proxy", "localhost,127.0.0.1,::1")
        .env("FLECHE_PROXY", &proxy_url)
        .env("FLECHE_PROXY_PORT", port.to_string())
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .with_context(|| format!("executing '{}'", command[0]));

    let _ = proxy.kill();
    let _ = proxy.wait();

    Ok(result?.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_available_port() {
        let port = find_available_port().unwrap();
        assert!(port > 0);
    }

    #[test]
    fn test_is_port_listening_unbound() {
        assert!(!is_port_listening(39_172));
    }

    #[test]
    fn test_is_port_listening_bound() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(is_port_listening(port));
    }
}
