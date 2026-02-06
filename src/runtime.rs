//! Runtime helpers shared across command handlers.

use std::io::Write;

/// Sends a terminal notification using OSC 9.
pub fn send_notification(message: &str) {
    if std::env::var_os("TMUX").is_some() {
        print!("\x1bPtmux;\x1b\x1b]9;fleche: {message}\x07\x1b\\");
    } else {
        print!("\x1b]9;fleche: {message}\x07");
    }
    let _ = std::io::stdout().flush();
}
