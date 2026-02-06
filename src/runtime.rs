//! Runtime helpers shared across command handlers.

use crate::config::Settings;
use crate::ssh::SshClient;
use std::io::Write;

/// SSH timeout settings: `(exec_timeout_secs, connect_timeout_secs)`.
pub type SshTimeouts = (u64, u64);

/// Extracts SSH timeout settings from config settings.
pub fn ssh_timeouts_from_settings(settings: &Settings) -> SshTimeouts {
    (settings.ssh_timeout_secs, settings.ssh_connect_timeout_secs)
}

/// Creates an SSH client with optional configured timeouts.
pub fn ssh_client(host: &str, debug: bool, timeouts: Option<SshTimeouts>) -> SshClient {
    if let Some((exec_timeout_secs, connect_timeout_secs)) = timeouts {
        SshClient::with_timeouts(host, debug, exec_timeout_secs, connect_timeout_secs)
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
