//! Local job execution.
//!
//! This module handles running jobs on the local machine instead of a remote cluster.
//! Local jobs run directly in the project directory with logs stored in `.fleche/jobs/{id}/`.

use crate::error::{IoResultExt, Result};
use crate::registry::JobStatus;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Creates a command that runs the given string through the system shell.
///
/// On Unix, uses `sh -c`. On Windows, uses `cmd /c`.
pub fn shell_command(command: &str) -> Command {
    #[cfg(unix)]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/c").arg(command);
        cmd
    }
}

/// Returns the local job directory path for a given job ID.
///
/// Creates the directory if it doesn't exist.
pub fn local_job_dir(project_path: &Path, job_id: &str) -> PathBuf {
    project_path.join(".fleche/jobs").join(job_id)
}

/// Ensures the local job directory exists.
pub fn ensure_job_dir(project_path: &Path, job_id: &str) -> Result<PathBuf> {
    let dir = local_job_dir(project_path, job_id);
    fs::create_dir_all(&dir)
        .io_context(|| format!("creating job directory '{}'", dir.display()))?;
    Ok(dir)
}

/// Runs a command in foreground mode, streaming output to terminal and capturing to log files.
///
/// Returns the exit code of the command.
pub fn run_foreground(
    project_path: &Path,
    job_id: &str,
    command: &str,
    env: &indexmap::IndexMap<String, String>,
) -> Result<i32> {
    let job_dir = ensure_job_dir(project_path, job_id)?;
    let stdout_path = job_dir.join("job.out");
    let stderr_path = job_dir.join("job.err");
    let exit_code_path = job_dir.join("exit_code");

    // Create log files
    let mut stdout_file = File::create(&stdout_path)
        .io_context(|| format!("creating stdout log '{}'", stdout_path.display()))?;
    let mut stderr_file = File::create(&stderr_path)
        .io_context(|| format!("creating stderr log '{}'", stderr_path.display()))?;

    // Build and run the command
    let mut child = shell_command(command)
        .current_dir(project_path)
        .envs(env.iter())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .io_context(|| format!("spawning command for job '{job_id}'"))?;

    // Stream stdout
    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();

    // Spawn threads to handle stdout and stderr concurrently
    let stdout_handle = std::thread::spawn(move || {
        if let Some(stdout) = child_stdout {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(std::result::Result::ok) {
                println!("{line}");
                let _ = writeln!(stdout_file, "{line}");
            }
        }
    });

    let stderr_handle = std::thread::spawn(move || {
        if let Some(stderr) = child_stderr {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(std::result::Result::ok) {
                eprintln!("{line}");
                let _ = writeln!(stderr_file, "{line}");
            }
        }
    });

    // Wait for output handling to complete
    let _ = stdout_handle.join();
    let _ = stderr_handle.join();

    // Wait for command to finish
    let status = child
        .wait()
        .io_context(|| format!("waiting for job '{job_id}' to finish"))?;
    let exit_code = status.code().unwrap_or(1);

    // Write exit code
    fs::write(&exit_code_path, exit_code.to_string())
        .io_context(|| format!("writing exit code for job '{job_id}'"))?;

    Ok(exit_code)
}

/// Runs a command in background mode (daemonized).
///
/// Creates a wrapper script that runs the command and writes the exit code on completion.
/// Returns the PID of the background process.
pub fn run_background(
    project_path: &Path,
    job_id: &str,
    command: &str,
    env: &indexmap::IndexMap<String, String>,
) -> Result<u32> {
    let job_dir = ensure_job_dir(project_path, job_id)?;
    let stdout_path = job_dir.join("job.out");
    let stderr_path = job_dir.join("job.err");
    let exit_code_path = job_dir.join("exit_code");
    let pid_path = job_dir.join("pid");
    let script_path = job_dir.join("run.sh");

    // Create wrapper script that writes exit code on completion
    let script = format!(
        r"#!/bin/sh
cd {}
{command}
echo $? > {}
",
        shell_escape(&project_path.to_string_lossy()),
        shell_escape(&exit_code_path.to_string_lossy())
    );

    fs::write(&script_path, &script)
        .io_context(|| format!("writing job script '{}'", script_path.display()))?;

    // Make script executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)
            .io_context(|| format!("reading script metadata '{}'", script_path.display()))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)
            .io_context(|| format!("setting script permissions '{}'", script_path.display()))?;
    }

    // Create output files
    let stdout_file = File::create(&stdout_path)
        .io_context(|| format!("creating stdout log '{}'", stdout_path.display()))?;
    let stderr_file = File::create(&stderr_path)
        .io_context(|| format!("creating stderr log '{}'", stderr_path.display()))?;

    // Spawn detached process using nohup-style approach
    let child = Command::new("sh")
        .arg(&script_path)
        .current_dir(project_path)
        .envs(env.iter())
        .stdout(stdout_file)
        .stderr(stderr_file)
        .stdin(Stdio::null())
        .spawn()
        .io_context(|| format!("spawning background job '{job_id}'"))?;

    let pid = child.id();

    // Write PID file
    fs::write(&pid_path, pid.to_string())
        .io_context(|| format!("writing PID file for job '{job_id}'"))?;

    Ok(pid)
}

/// Gets the status of a local job by checking PID and `exit_code` files.
pub fn get_local_job_status(project_path: &Path, job_id: &str) -> Result<JobStatus> {
    let job_dir = local_job_dir(project_path, job_id);
    let exit_code_path = job_dir.join("exit_code");
    let pid_path = job_dir.join("pid");

    // Check if exit_code exists (job finished)
    if exit_code_path.exists() {
        let exit_code: i32 = fs::read_to_string(&exit_code_path)
            .io_context(|| format!("reading exit code for job '{job_id}'"))?
            .trim()
            .parse()
            .unwrap_or(1);

        return Ok(if exit_code == 0 {
            JobStatus::Completed
        } else {
            JobStatus::Failed
        });
    }

    // Check if PID exists and process is still running
    if pid_path.exists() {
        let pid: u32 = fs::read_to_string(&pid_path)
            .io_context(|| format!("reading PID file for job '{job_id}'"))?
            .trim()
            .parse()
            .unwrap_or(0);

        if pid > 0 && is_process_running(pid) {
            return Ok(JobStatus::Running);
        }

        // PID exists but process is not running - job failed without writing exit code
        return Ok(JobStatus::Failed);
    }

    // No PID file - job hasn't started or something went wrong
    Ok(JobStatus::Pending)
}

/// Cancels a local job by sending SIGTERM to the process.
pub fn cancel_local_job(project_path: &Path, job_id: &str) -> Result<bool> {
    use sysinfo::{Pid, Signal, System};

    let job_dir = local_job_dir(project_path, job_id);
    let pid_path = job_dir.join("pid");
    let exit_code_path = job_dir.join("exit_code");

    if !pid_path.exists() {
        return Ok(false);
    }

    let pid: u32 = fs::read_to_string(&pid_path)
        .io_context(|| format!("reading PID file for job '{job_id}'"))?
        .trim()
        .parse()
        .unwrap_or(0);

    if pid == 0 {
        return Ok(false);
    }

    let sys = System::new_all();
    if let Some(process) = sys.process(Pid::from_u32(pid)) {
        // Send SIGTERM on Unix, TerminateProcess on Windows
        let killed = process.kill_with(Signal::Term).unwrap_or(false);
        if killed {
            // Write exit code to indicate cancellation (143 = 128 + 15 SIGTERM)
            fs::write(&exit_code_path, "143")
                .io_context(|| format!("writing cancellation exit code for job '{job_id}'"))?;
            return Ok(true);
        }
    }

    Ok(false)
}

/// Reads local job logs.
pub fn read_local_logs(
    project_path: &Path,
    job_id: &str,
    stream: LogStream,
    tail: Option<usize>,
) -> Result<String> {
    let job_dir = local_job_dir(project_path, job_id);
    let log_path = match stream {
        LogStream::Stdout => job_dir.join("job.out"),
        LogStream::Stderr => job_dir.join("job.err"),
    };

    if !log_path.exists() {
        return Ok(String::new());
    }

    let content = fs::read_to_string(&log_path)
        .io_context(|| format!("reading log file '{}'", log_path.display()))?;

    if let Some(n) = tail {
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(n);
        Ok(lines[start..].join("\n"))
    } else {
        Ok(content)
    }
}

/// Which log stream to read.
#[derive(Clone, Copy)]
pub enum LogStream {
    Stdout,
    Stderr,
}

/// Cleans up a local job directory.
pub fn clean_local_job(project_path: &Path, job_id: &str) -> Result<()> {
    let job_dir = local_job_dir(project_path, job_id);
    if job_dir.exists() {
        fs::remove_dir_all(&job_dir)
            .io_context(|| format!("removing job directory '{}'", job_dir.display()))?;
    }
    Ok(())
}

/// Checks if a process with the given PID is still running.
fn is_process_running(pid: u32) -> bool {
    use sysinfo::{Pid, System};
    let sys = System::new_all();
    sys.process(Pid::from_u32(pid)).is_some()
}

/// Shell-escapes a string for safe use in shell commands.
fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/' || c == '.')
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Follows a local log file in real-time (similar to tail -f).
pub fn follow_local_logs(project_path: &Path, job_id: &str) -> Result<()> {
    let job_dir = local_job_dir(project_path, job_id);
    let log_path = job_dir.join("job.out");
    let exit_code_path = job_dir.join("exit_code");

    // Wait for log file to exist
    while !log_path.exists() {
        if exit_code_path.exists() {
            // Job finished before we could start following
            if log_path.exists() {
                print!(
                    "{}",
                    fs::read_to_string(&log_path)
                        .io_context(|| format!("reading log file '{}'", log_path.display()))?
                );
            }
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let file = File::open(&log_path)
        .io_context(|| format!("opening log file '{}'", log_path.display()))?;
    let mut reader = BufReader::new(file);
    let mut buffer = String::new();

    loop {
        buffer.clear();
        match reader.read_to_string(&mut buffer) {
            Ok(0) => {
                // No new data, check if job finished
                if exit_code_path.exists() {
                    // Read any final output
                    buffer.clear();
                    let _ = reader.read_to_string(&mut buffer);
                    if !buffer.is_empty() {
                        print!("{buffer}");
                    }
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Ok(_) => {
                print!("{buffer}");
                let _ = std::io::stdout().flush();
            }
            Err(_) => break,
        }
    }

    Ok(())
}
