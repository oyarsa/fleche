//! Push notifications via ntfy.sh.
//!
//! Sends HTTP POST requests to `ntfy.sh/<topic>` on job state changes.
//! All errors are logged to stderr and never propagated — notifications
//! are best-effort and must not break the main workflow.

use crate::registry::JobStatus;

/// Sends a push notification to ntfy.sh.
///
/// Fire-and-forget: errors are logged to stderr, never propagated.
fn send_ntfy(topic: &str, title: &str, message: &str, priority: &str, tags: &str) {
    let url = format!("https://ntfy.sh/{topic}");
    let result = ureq::post(&url)
        .header("Title", title)
        .header("Priority", priority)
        .header("Tags", tags)
        .send(message);

    if let Err(e) = result {
        eprintln!("ntfy: failed to send notification: {e}");
    }
}

/// Sends an ntfy notification for a job state transition.
///
/// No-ops if `old_status == Some(new_status)` (no actual transition).
/// Appends the job note to the message body when present.
pub fn notify_state_change(
    topic: &str,
    job_id: &str,
    old_status: Option<JobStatus>,
    new_status: JobStatus,
    note: Option<&str>,
) {
    if old_status == Some(new_status) {
        return;
    }

    let (title, priority, tags) = match new_status {
        JobStatus::Pending => ("Job submitted", "low", "hourglass"),
        JobStatus::Running => ("Job started", "default", "arrow_forward"),
        JobStatus::Completed => ("Job completed", "high", "white_check_mark"),
        JobStatus::Failed => ("Job failed", "urgent", "x"),
        JobStatus::Cancelled => ("Job cancelled", "high", "no_entry"),
    };

    let summary = match new_status {
        JobStatus::Pending => format!("Job {job_id} submitted"),
        JobStatus::Running => format!("Job {job_id} started running"),
        JobStatus::Completed => format!("Job {job_id} completed"),
        JobStatus::Failed => format!("Job {job_id} failed"),
        JobStatus::Cancelled => format!("Job {job_id} cancelled"),
    };

    let body = match note {
        Some(n) => format!("{summary}\nNote: {n}"),
        None => summary,
    };

    send_ntfy(topic, title, &body, priority, tags);
}
