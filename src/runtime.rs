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

/// Notification behavior policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum NotifyPolicy {
    /// Only notify when a command explicitly asks for notifications.
    #[default]
    CliFlagOnly,
    /// Always notify on terminal states.
    Always,
    /// Never notify.
    Never,
}


/// Shared runtime settings used by command handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCtx {
    /// Enable verbose SSH output.
    pub debug: bool,
    /// SSH timeout settings to apply when creating clients.
    pub ssh_timeouts: Option<SshTimeouts>,
    /// Poll interval in seconds when waiting for local jobs.
    pub poll_interval_local_secs: u64,
    /// Poll interval in seconds when waiting for remote jobs.
    pub poll_interval_remote_secs: u64,
    /// Base delay in seconds for exponential retry backoff.
    pub retry_base_delay_secs: u64,
    /// Default notification policy.
    pub notify_policy: NotifyPolicy,
}

impl Default for RuntimeCtx {
    fn default() -> Self {
        Self::from_optional_settings(false, None)
    }
}

impl RuntimeCtx {
    /// Builds a runtime context from project settings.
    pub fn from_settings(debug: bool, settings: &Settings) -> Self {
        let notify_policy = match std::env::var("FLECHE_NOTIFY_POLICY").ok().as_deref() {
            Some("always") => NotifyPolicy::Always,
            Some("never") => NotifyPolicy::Never,
            _ => NotifyPolicy::CliFlagOnly,
        };

        Self {
            debug,
            ssh_timeouts: Some(ssh_timeouts_from_settings(settings)),
            poll_interval_local_secs: settings.poll_interval_local_secs,
            poll_interval_remote_secs: settings.poll_interval_remote_secs,
            retry_base_delay_secs: settings.retry_base_delay_secs,
            notify_policy,
        }
    }

    /// Builds a runtime context with optional project settings.
    ///
    /// If `settings` is `None`, default settings are used.
    pub fn from_optional_settings(debug: bool, settings: Option<&Settings>) -> Self {
        if let Some(settings) = settings {
            Self::from_settings(debug, settings)
        } else {
            Self::from_settings(debug, &Settings::default())
        }
    }

    /// Creates an SSH client for a host using this context.
    pub fn ssh(&self, host: &str) -> SshClient {
        ssh_client(host, self.debug, self.ssh_timeouts)
    }

    /// Resolves whether notification should be sent for this event.
    pub fn should_notify(&self, requested: bool) -> bool {
        match self.notify_policy {
            NotifyPolicy::CliFlagOnly => requested,
            NotifyPolicy::Always => true,
            NotifyPolicy::Never => false,
        }
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
