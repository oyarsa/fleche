//! Job log viewing operations.

use crate::error::Result;
use crate::local;
use crate::registry::Registry;
use crate::ssh::SshClient;
use console::style;
use std::path::PathBuf;

use super::{job_path_from_workspace, status::resolve_job};

/// Options for displaying job logs.
#[derive(Debug, Default)]
pub struct ShowLogsOptions {
    /// Stream logs in real-time.
    pub follow: bool,
    /// Show only stdout.
    pub only_stdout: bool,
    /// Show only stderr.
    pub only_stderr: bool,
    /// Show only the last N lines.
    pub tail: Option<usize>,
    /// Strip ANSI escape codes from output.
    pub raw: bool,
    /// Enable verbose SSH output.
    pub debug: bool,
}

/// Displays logs from a job's stdout or stderr.
pub async fn show_logs(
    job_id: Option<&str>,
    tags: &[(String, String)],
    note_filter: Option<&str>,
    opts: ShowLogsOptions,
) -> Result<()> {
    let registry = Registry::open()?;
    let job = resolve_job(&registry, job_id, tags, note_filter)?;

    // Determine which streams to show
    let show_stdout = !opts.only_stderr || opts.only_stdout;
    let show_stderr = !opts.only_stdout || opts.only_stderr;
    let show_both = show_stdout && show_stderr;

    // Strip ANSI codes if --raw is set or if stdout is not a terminal (piped)
    let strip_ansi = opts.raw || !std::io::IsTerminal::is_terminal(&std::io::stdout());

    // Handle local jobs
    if job.remote_host == "local" {
        let project_path = PathBuf::from(&job.project_path);

        if opts.follow {
            println!(
                "{}",
                style("Following output (Ctrl+C to disconnect)...").yellow()
            );
            local::follow_local_logs(&project_path, &job.id)?;
        } else if show_both {
            println!("{}", style("=== STDOUT ===").bold());
            match local::read_local_logs(
                &project_path,
                &job.id,
                local::LogStream::Stdout,
                opts.tail,
            ) {
                Ok(content) => print!("{}", maybe_strip_ansi(&content, strip_ansi)),
                Err(e) => eprintln!("Error reading stdout: {e}"),
            }

            println!();
            println!("{}", style("=== STDERR ===").bold());
            match local::read_local_logs(
                &project_path,
                &job.id,
                local::LogStream::Stderr,
                opts.tail,
            ) {
                Ok(content) => print!("{}", maybe_strip_ansi(&content, strip_ansi)),
                Err(e) => eprintln!("Error reading stderr: {e}"),
            }
        } else {
            let stream = if show_stderr {
                local::LogStream::Stderr
            } else {
                local::LogStream::Stdout
            };
            let content = local::read_local_logs(&project_path, &job.id, stream, opts.tail)?;
            print!("{}", maybe_strip_ansi(&content, strip_ansi));
        }

        return Ok(());
    }

    // Remote job handling
    let ssh = SshClient::new(&job.remote_host, opts.debug);

    let log_base = job_path_from_workspace(&job.remote_path, &job.id);

    if opts.follow {
        let log_file = if opts.only_stderr {
            "job.err"
        } else {
            "job.out"
        };
        let log_path = format!("{log_base}/{log_file}");

        if job.slurm_id.is_some() {
            println!(
                "{}",
                style("Following output (Ctrl+C to disconnect)...").yellow()
            );
        }
        let mut child = ssh.tail_follow(&log_path)?;
        let _ = child.wait().await;
    } else if show_both {
        println!("{}", style("=== STDOUT ===").bold());
        let stdout_path = format!("{log_base}/job.out");
        match ssh.cat_tail(&stdout_path, opts.tail).await {
            Ok(content) => print!("{}", maybe_strip_ansi(&content, strip_ansi)),
            Err(e) => eprintln!("Error reading stdout: {e}"),
        }

        println!();
        println!("{}", style("=== STDERR ===").bold());
        let stderr_path = format!("{log_base}/job.err");
        match ssh.cat_tail(&stderr_path, opts.tail).await {
            Ok(content) => print!("{}", maybe_strip_ansi(&content, strip_ansi)),
            Err(e) => eprintln!("Error reading stderr: {e}"),
        }
    } else {
        let log_file = if show_stderr { "job.err" } else { "job.out" };
        let log_path = format!("{log_base}/{log_file}");

        let content = ssh.cat_tail(&log_path, opts.tail).await?;
        print!("{}", maybe_strip_ansi(&content, strip_ansi));
    }

    Ok(())
}

/// Strips ANSI escape codes from a string if `strip` is true.
fn maybe_strip_ansi(s: &str, strip: bool) -> std::borrow::Cow<'_, str> {
    if strip {
        strip_ansi_codes(s).into()
    } else {
        s.into()
    }
}

/// Strips ANSI escape codes from a string.
///
/// Handles common ANSI sequences: CSI (ESC [), OSC (ESC ]), and basic escape codes.
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // ESC character - start of escape sequence
            match chars.peek() {
                Some('[') => {
                    // CSI sequence: ESC [ ... (ends with letter or @-~)
                    chars.next(); // consume '['
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc.is_ascii_alphabetic() || ('@'..='~').contains(&nc) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC sequence: ESC ] ... (ends with BEL or ST)
                    chars.next(); // consume ']'
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc == '\x07' {
                            // BEL
                            break;
                        }
                        if nc == '\x1b' {
                            // ST (ESC \)
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                Some('(' | ')') => {
                    // Character set selection: ESC ( or ESC )
                    chars.next();
                    chars.next(); // skip the designator
                }
                _ => {
                    // Single-character escape or unknown - skip next char
                    chars.next();
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_codes_csi_colors() {
        // Basic color codes
        assert_eq!(strip_ansi_codes("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(
            strip_ansi_codes("\x1b[1;32mbold green\x1b[0m"),
            "bold green"
        );
        assert_eq!(
            strip_ansi_codes("\x1b[38;5;196mextended\x1b[0m"),
            "extended"
        );
    }

    #[test]
    fn test_strip_ansi_codes_csi_cursor() {
        // Cursor movement
        assert_eq!(strip_ansi_codes("\x1b[2Jclear"), "clear");
        assert_eq!(strip_ansi_codes("\x1b[10;20Hposition"), "position");
    }

    #[test]
    fn test_strip_ansi_codes_osc() {
        // OSC sequences (terminal title, notifications)
        assert_eq!(strip_ansi_codes("\x1b]0;title\x07text"), "text");
        assert_eq!(strip_ansi_codes("\x1b]9;notification\x07text"), "text");
    }

    #[test]
    fn test_strip_ansi_codes_preserves_text() {
        assert_eq!(strip_ansi_codes("plain text"), "plain text");
        assert_eq!(strip_ansi_codes("line1\nline2"), "line1\nline2");
        assert_eq!(strip_ansi_codes(""), "");
    }

    #[test]
    fn test_strip_ansi_codes_complex() {
        let input = "\x1b[1mBold\x1b[0m and \x1b[31mred\x1b[0m text";
        assert_eq!(strip_ansi_codes(input), "Bold and red text");
    }

    #[test]
    fn test_strip_ansi_codes_progress_bar() {
        // Common progress bar output with cursor control
        let input = "Progress: \x1b[32m50%\x1b[0m \x1b[K";
        assert_eq!(strip_ansi_codes(input), "Progress: 50% ");
    }
}
