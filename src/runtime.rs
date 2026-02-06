//! Runtime helpers shared across command handlers.

use crate::config::Settings;
use crate::ssh::SshClient;
use std::io::Write;

/// SSH timeout settings in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SshTimeouts {
    /// Maximum time for a command to complete.
    pub exec_timeout_secs: u64,
    /// Maximum time to establish the SSH connection.
    pub connect_timeout_secs: u64,
}

/// Extracts SSH timeout settings from config settings.
pub fn ssh_timeouts_from_settings(settings: &Settings) -> SshTimeouts {
    SshTimeouts {
        exec_timeout_secs: settings.ssh_timeout_secs,
        connect_timeout_secs: settings.ssh_connect_timeout_secs,
    }
}

/// Creates an SSH client with optional configured timeouts.
pub fn ssh_client(host: &str, debug: bool, timeouts: Option<SshTimeouts>) -> SshClient {
    if let Some(timeouts) = timeouts {
        SshClient::with_timeouts(
            host,
            debug,
            timeouts.exec_timeout_secs,
            timeouts.connect_timeout_secs,
        )
    } else {
        SshClient::new(host, debug)
    }
}

/// Sends a terminal notification using OSC 9.
pub fn send_notification(message: &str) {
    if std::env::var_os("TMUX").is_some() {
        print!("\x1bPtmux;\x1b\x1b]9;fleche: {message}\x07\x1b\\");
    } else {
        print!("\x1b]9;fleche: {message}\x07");
    }
    let _ = std::io::stdout().flush();
}
