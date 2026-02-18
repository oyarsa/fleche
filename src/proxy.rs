//! SOCKS proxy tunnel for routing traffic through the remote host.
//!
//! Opens an SSH dynamic port forward (`ssh -D`) to create a local SOCKS proxy,
//! then runs a user command with proxy environment variables set. The proxy
//! tunnel is cached per-host so repeated invocations reuse the same connection.

use crate::ssh::ssh_socket_dir;
use anyhow::{Context, bail};
use console::style;
use std::path::PathBuf;

/// Information about a running SOCKS proxy tunnel.
struct CachedProxy {
    port: u16,
    pid: u32,
}

/// Returns the path to the proxy cache file for a given host.
fn cache_path(host: &str) -> PathBuf {
    let safe_host = host.replace(['/', '\\', ':'], "_");
    ssh_socket_dir().join(format!("{safe_host}.proxy"))
}

/// Finds an available TCP port by binding to port 0.
fn find_available_port() -> anyhow::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .context("finding available port for SOCKS proxy")?;
    Ok(listener.local_addr()?.port())
}

/// Checks whether a process with the given PID is alive.
#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid).ok() else {
        return false;
    };
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

#[cfg(not(unix))]
fn is_process_alive(_pid: u32) -> bool {
    false
}

/// Checks whether something is listening on the given local port.
fn is_port_listening(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// Reads cached proxy info and validates it's still alive.
fn load_cached_proxy(host: &str) -> Option<CachedProxy> {
    let content = std::fs::read_to_string(cache_path(host)).ok()?;
    let mut lines = content.lines();
    let port: u16 = lines.next()?.parse().ok()?;
    let pid: u32 = lines.next()?.parse().ok()?;

    if is_process_alive(pid) && is_port_listening(port) {
        Some(CachedProxy { port, pid })
    } else {
        let _ = std::fs::remove_file(cache_path(host));
        None
    }
}

/// Writes proxy info to the cache file.
fn save_proxy_cache(host: &str, proxy: &CachedProxy) {
    let _ = std::fs::write(cache_path(host), format!("{}\n{}\n", proxy.port, proxy.pid));
}

/// Kills a proxy process by PID.
#[cfg(unix)]
fn kill_proxy(pid: u32) {
    let Some(pid) = i32::try_from(pid).ok() else {
        return;
    };
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    );
}

#[cfg(not(unix))]
fn kill_proxy(_pid: u32) {}

/// Starts a new SSH SOCKS proxy tunnel.
///
/// Spawns `ssh -D <port> -N <host>` and waits for the port to become
/// available. The SSH process is intentionally left running after we drop
/// its handle so it can be reused by subsequent invocations.
async fn start_proxy(host: &str, port: u16, debug: bool) -> anyhow::Result<CachedProxy> {
    let mut args: Vec<String> = Vec::new();

    if debug {
        args.push("-v".to_string());
    }

    // NOTE: We intentionally omit ClearAllForwardings=yes (which the regular
    // SshClient uses) because it would prevent our -D forward from working.
    // We also use a separate connection (no ControlMaster) so the -D binding
    // is guaranteed to be on this specific connection.
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

    let pid = child.id();

    // Poll until the proxy port becomes available or SSH exits
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(30);

    loop {
        if is_port_listening(port) {
            break;
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

    // Drop the child handle without waiting; the SSH process keeps running.
    drop(child);

    Ok(CachedProxy { port, pid })
}

/// Resolves the proxy to use: returns a cached one or starts a fresh tunnel.
async fn resolve_proxy(
    host: &str,
    port_override: Option<u16>,
    debug: bool,
) -> anyhow::Result<CachedProxy> {
    if let Some(cached) = load_cached_proxy(host) {
        if port_override.is_none() || port_override == Some(cached.port) {
            eprintln!(
                "{} Reusing proxy tunnel on port {}",
                style("*").cyan(),
                cached.port
            );
            return Ok(cached);
        }
        // User wants a different port; tear down the old proxy
        kill_proxy(cached.pid);
        let _ = std::fs::remove_file(cache_path(host));
    }

    let port = match port_override {
        Some(p) => p,
        None => find_available_port()?,
    };

    let proxy = start_proxy(host, port, debug).await?;
    save_proxy_cache(host, &proxy);
    eprintln!(
        "{} SOCKS proxy listening on 127.0.0.1:{}",
        style("*").cyan(),
        proxy.port
    );

    Ok(proxy)
}

/// Opens a SOCKS proxy tunnel to the remote host and runs a command through it.
///
/// Sets `ALL_PROXY`, `HTTP_PROXY`, `HTTPS_PROXY` (and lowercase variants),
/// `NO_PROXY`, and fleche-specific variables on the child process. The proxy
/// tunnel is cached per-host so repeated calls reuse the same SSH connection.
///
/// Returns the child command's exit code.
pub async fn run_proxy_command(
    host: &str,
    command: &[String],
    port_override: Option<u16>,
    debug: bool,
) -> anyhow::Result<i32> {
    let proxy = resolve_proxy(host, port_override, debug).await?;
    let proxy_url = format!("socks5h://127.0.0.1:{}", proxy.port);

    let status = tokio::process::Command::new(&command[0])
        .args(&command[1..])
        .env("ALL_PROXY", &proxy_url)
        .env("HTTP_PROXY", &proxy_url)
        .env("HTTPS_PROXY", &proxy_url)
        .env("http_proxy", &proxy_url)
        .env("https_proxy", &proxy_url)
        .env("NO_PROXY", "localhost,127.0.0.1,::1")
        .env("no_proxy", "localhost,127.0.0.1,::1")
        .env("FLECHE_PROXY", &proxy_url)
        .env("FLECHE_PROXY_PORT", proxy.port.to_string())
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .with_context(|| format!("executing '{}'", command[0]))?;

    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_path_simple_host() {
        let path = cache_path("cluster");
        assert!(path.to_string_lossy().ends_with("cluster.proxy"));
    }

    #[test]
    fn test_cache_path_host_with_special_chars() {
        let path = cache_path("user@host:22");
        assert!(path.to_string_lossy().ends_with("user@host_22.proxy"));
    }

    #[test]
    fn test_find_available_port() {
        let port = find_available_port().unwrap();
        assert!(port > 0);
    }

    #[test]
    fn test_is_process_alive_current_process() {
        // Our own process should be alive
        let pid = std::process::id();
        assert!(is_process_alive(pid));
    }

    #[test]
    fn test_is_process_alive_nonexistent() {
        // PID 0 is the kernel, but kill(0, 0) checks the calling process group.
        // Use an unlikely high PID instead.
        assert!(!is_process_alive(4_000_000));
    }

    #[test]
    fn test_is_port_listening_unbound() {
        // An unlikely port should not be listening
        assert!(!is_port_listening(39_172));
    }

    #[test]
    fn test_load_cached_proxy_missing_file() {
        assert!(load_cached_proxy("nonexistent_host_12345").is_none());
    }
}
