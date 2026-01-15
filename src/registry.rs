//! Local job registry backed by `SQLite`.
//!
//! This module provides persistent storage for job records, including their status,
//! configuration, and associated tags. The database is stored in the user's config
//! directory (`~/.config/fleche/jobs.db`).

use crate::config::ResolvedJob;
use crate::error::{FlecheError, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// A record of a submitted job stored in the local registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    /// Unique identifier for the job (e.g., "train-20240115-120000-abc1").
    pub id: String,
    /// Slurm job ID assigned by the scheduler, if submitted.
    pub slurm_id: Option<String>,
    /// Name of the job definition from fleche.toml.
    pub job_name: String,
    /// Name of the project (from fleche.toml).
    pub project_name: String,
    /// Local path to the project directory.
    pub project_path: String,
    /// Remote host where the job runs.
    pub remote_host: String,
    /// Remote directory containing the job files.
    pub remote_path: String,
    /// The command executed by the job.
    pub command: String,
    /// Current status of the job.
    pub status: JobStatus,
    /// JSON-serialized job configuration for reference.
    pub config_json: String,
    /// When the job was created.
    pub created_at: DateTime<Utc>,
    /// When the job record was last updated.
    pub updated_at: DateTime<Utc>,
    /// Whether outputs have been synced back to local.
    pub outputs_synced: bool,
    /// User-defined key-value tags for filtering jobs.
    pub tags: HashMap<String, String>,
}

/// The status of a job in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// Job is waiting in the Slurm queue.
    Pending,
    /// Job is currently executing.
    Running,
    /// Job finished successfully.
    Completed,
    /// Job failed or was terminated due to an error.
    Failed,
    /// Job was cancelled by the user.
    Cancelled,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Pending => write!(f, "pending"),
            JobStatus::Running => write!(f, "running"),
            JobStatus::Completed => write!(f, "completed"),
            JobStatus::Failed => write!(f, "failed"),
            JobStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::str::FromStr for JobStatus {
    type Err = FlecheError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(JobStatus::Pending),
            "running" => Ok(JobStatus::Running),
            "completed" => Ok(JobStatus::Completed),
            "failed" => Ok(JobStatus::Failed),
            "cancelled" => Ok(JobStatus::Cancelled),
            _ => Err(FlecheError::Other(format!("Unknown status: {s}"))),
        }
    }
}

/// SQLite-backed registry for storing job records.
pub struct Registry {
    /// The database connection.
    conn: Connection,
}

impl Registry {
    /// Opens the registry, creating the database file if it doesn't exist.
    ///
    /// The database is stored at `~/.config/fleche/jobs.db`.
    pub fn open() -> Result<Self> {
        let db_path = get_db_path()?;

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&db_path)?;
        let registry = Registry { conn };
        registry.init_schema()?;
        Ok(registry)
    }

    /// Initializes the database schema if it doesn't exist.
    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r"
            CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                slurm_id TEXT,
                job_name TEXT NOT NULL,
                project_name TEXT NOT NULL,
                project_path TEXT NOT NULL,
                remote_host TEXT NOT NULL,
                remote_path TEXT NOT NULL,
                command TEXT NOT NULL,
                status TEXT NOT NULL,
                config_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                outputs_synced INTEGER DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status);
            CREATE INDEX IF NOT EXISTS idx_jobs_project ON jobs(project_path);
            CREATE INDEX IF NOT EXISTS idx_jobs_created ON jobs(created_at);

            CREATE TABLE IF NOT EXISTS job_tags (
                job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (job_id, key)
            );

            CREATE INDEX IF NOT EXISTS idx_job_tags_key_value ON job_tags(key, value);
            ",
        )?;
        Ok(())
    }

    /// Inserts a new job record into the registry.
    pub fn insert_job(
        &self,
        id: &str,
        slurm_id: Option<&str>,
        job: &ResolvedJob,
        project_name: &str,
        project_path: &str,
        remote_host: &str,
        remote_path: &str,
        tags: &[(String, String)],
    ) -> Result<()> {
        let now = Utc::now();
        let config_json = serde_json::to_string(job)?;

        self.conn.execute(
            r"
            INSERT INTO jobs (id, slurm_id, job_name, project_name, project_path,
                              remote_host, remote_path, command, status, config_json,
                              created_at, updated_at, outputs_synced)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0)
            ",
            params![
                id,
                slurm_id,
                job.name,
                project_name,
                project_path,
                remote_host,
                remote_path,
                job.command,
                JobStatus::Pending.to_string(),
                config_json,
                now.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )?;

        // Insert tags
        for (key, value) in tags {
            self.conn.execute(
                "INSERT INTO job_tags (job_id, key, value) VALUES (?1, ?2, ?3)",
                params![id, key, value],
            )?;
        }

        Ok(())
    }

    /// Updates the status of a job.
    pub fn update_status(&self, id: &str, status: JobStatus) -> Result<()> {
        let now = Utc::now();
        self.conn.execute(
            "UPDATE jobs SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.to_string(), now.to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Updates the Slurm job ID for a job.
    #[allow(dead_code)]
    pub fn update_slurm_id(&self, id: &str, slurm_id: &str) -> Result<()> {
        let now = Utc::now();
        self.conn.execute(
            "UPDATE jobs SET slurm_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![slurm_id, now.to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Marks a job's outputs as synced.
    pub fn set_outputs_synced(&self, id: &str) -> Result<()> {
        let now = Utc::now();
        self.conn.execute(
            "UPDATE jobs SET outputs_synced = 1, updated_at = ?1 WHERE id = ?2",
            params![now.to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Retrieves a job by its ID.
    pub fn get_job(&self, id: &str) -> Result<JobRecord> {
        let mut stmt = self.conn.prepare(
            r"
            SELECT id, slurm_id, job_name, project_name, project_path,
                   remote_host, remote_path, command, status, config_json,
                   created_at, updated_at, outputs_synced
            FROM jobs WHERE id = ?1
            ",
        )?;

        let job = stmt
            .query_row(params![id], |row| {
                Ok(JobRecord {
                    id: row.get(0)?,
                    slurm_id: row.get(1)?,
                    job_name: row.get(2)?,
                    project_name: row.get(3)?,
                    project_path: row.get(4)?,
                    remote_host: row.get(5)?,
                    remote_path: row.get(6)?,
                    command: row.get(7)?,
                    status: row
                        .get::<_, String>(8)?
                        .parse()
                        .unwrap_or(JobStatus::Pending),
                    config_json: row.get(9)?,
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(10)?)
                        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
                    updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(11)?)
                        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
                    outputs_synced: row.get::<_, i32>(12)? == 1,
                    tags: HashMap::new(),
                })
            })
            .map_err(|_| FlecheError::JobIdNotFound(id.to_string()))?;

        // Load tags
        let tags = self.get_tags(&job.id)?;
        Ok(JobRecord { tags, ..job })
    }

    /// Retrieves tags for a job.
    fn get_tags(&self, job_id: &str) -> Result<HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM job_tags WHERE job_id = ?1")?;
        let tags = stmt
            .query_map(params![job_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(std::result::Result::ok)
            .collect();
        Ok(tags)
    }

    /// Lists jobs matching the given filters.
    ///
    /// Jobs can be filtered by project path, status, and tags. Results are
    /// ordered by creation time (newest first) and limited to `limit` results.
    pub fn list_jobs(
        &self,
        project_filter: Option<&str>,
        status_filter: Option<JobStatus>,
        tag_filters: &[(String, String)],
        limit: usize,
    ) -> Result<Vec<JobRecord>> {
        let mut sql = String::from(
            r"
            SELECT DISTINCT j.id, j.slurm_id, j.job_name, j.project_name, j.project_path,
                   j.remote_host, j.remote_path, j.command, j.status, j.config_json,
                   j.created_at, j.updated_at, j.outputs_synced
            FROM jobs j
            ",
        );

        let mut conditions = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        // Add tag joins
        for (i, _) in tag_filters.iter().enumerate() {
            sql.push_str(&format!(" INNER JOIN job_tags t{i} ON j.id = t{i}.job_id"));
        }

        // Add tag conditions
        for (i, (key, value)) in tag_filters.iter().enumerate() {
            conditions.push(format!("t{i}.key = ? AND t{i}.value = ?"));
            params_vec.push(Box::new(key.clone()));
            params_vec.push(Box::new(value.clone()));
        }

        if let Some(project) = project_filter {
            conditions.push("j.project_path LIKE ?".to_string());
            params_vec.push(Box::new(format!("%{project}%")));
        }

        if let Some(status) = status_filter {
            conditions.push("j.status = ?".to_string());
            params_vec.push(Box::new(status.to_string()));
        }

        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }

        sql.push_str(" ORDER BY j.created_at DESC LIMIT ?");
        params_vec.push(Box::new(i64::try_from(limit).unwrap_or(i64::MAX)));

        let mut stmt = self.conn.prepare(&sql)?;

        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(std::convert::AsRef::as_ref).collect();

        let jobs = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(JobRecord {
                    id: row.get(0)?,
                    slurm_id: row.get(1)?,
                    job_name: row.get(2)?,
                    project_name: row.get(3)?,
                    project_path: row.get(4)?,
                    remote_host: row.get(5)?,
                    remote_path: row.get(6)?,
                    command: row.get(7)?,
                    status: row
                        .get::<_, String>(8)?
                        .parse()
                        .unwrap_or(JobStatus::Pending),
                    config_json: row.get(9)?,
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(10)?)
                        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
                    updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(11)?)
                        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
                    outputs_synced: row.get::<_, i32>(12)? == 1,
                    tags: HashMap::new(),
                })
            })?
            .filter_map(std::result::Result::ok)
            .collect::<Vec<_>>();

        // Load tags for each job
        let mut jobs_with_tags = Vec::new();
        for job in jobs {
            let tags = self.get_tags(&job.id)?;
            jobs_with_tags.push(JobRecord { tags, ..job });
        }

        Ok(jobs_with_tags)
    }

    /// Lists finished jobs older than the given duration.
    ///
    /// Only returns jobs with status completed, failed, or cancelled.
    pub fn list_jobs_older_than(&self, duration: Duration) -> Result<Vec<JobRecord>> {
        let cutoff = Utc::now() - duration;
        let mut stmt = self.conn.prepare(
            r"
            SELECT id, slurm_id, job_name, project_name, project_path,
                   remote_host, remote_path, command, status, config_json,
                   created_at, updated_at, outputs_synced
            FROM jobs
            WHERE created_at < ?1 AND status IN ('completed', 'failed', 'cancelled')
            ORDER BY created_at DESC
            ",
        )?;

        let jobs = stmt
            .query_map(params![cutoff.to_rfc3339()], |row| {
                Ok(JobRecord {
                    id: row.get(0)?,
                    slurm_id: row.get(1)?,
                    job_name: row.get(2)?,
                    project_name: row.get(3)?,
                    project_path: row.get(4)?,
                    remote_host: row.get(5)?,
                    remote_path: row.get(6)?,
                    command: row.get(7)?,
                    status: row
                        .get::<_, String>(8)?
                        .parse()
                        .unwrap_or(JobStatus::Pending),
                    config_json: row.get(9)?,
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(10)?)
                        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
                    updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(11)?)
                        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
                    outputs_synced: row.get::<_, i32>(12)? == 1,
                    tags: HashMap::new(),
                })
            })?
            .filter_map(std::result::Result::ok)
            .collect::<Vec<_>>();

        let mut jobs_with_tags = Vec::new();
        for job in jobs {
            let tags = self.get_tags(&job.id)?;
            jobs_with_tags.push(JobRecord { tags, ..job });
        }

        Ok(jobs_with_tags)
    }

    /// Lists all finished jobs (completed, failed, or cancelled).
    pub fn list_finished_jobs(&self) -> Result<Vec<JobRecord>> {
        let mut stmt = self.conn.prepare(
            r"
            SELECT id, slurm_id, job_name, project_name, project_path,
                   remote_host, remote_path, command, status, config_json,
                   created_at, updated_at, outputs_synced
            FROM jobs
            WHERE status IN ('completed', 'failed', 'cancelled')
            ORDER BY created_at DESC
            ",
        )?;

        let jobs = stmt
            .query_map([], |row| {
                Ok(JobRecord {
                    id: row.get(0)?,
                    slurm_id: row.get(1)?,
                    job_name: row.get(2)?,
                    project_name: row.get(3)?,
                    project_path: row.get(4)?,
                    remote_host: row.get(5)?,
                    remote_path: row.get(6)?,
                    command: row.get(7)?,
                    status: row
                        .get::<_, String>(8)?
                        .parse()
                        .unwrap_or(JobStatus::Pending),
                    config_json: row.get(9)?,
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(10)?)
                        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
                    updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(11)?)
                        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
                    outputs_synced: row.get::<_, i32>(12)? == 1,
                    tags: HashMap::new(),
                })
            })?
            .filter_map(std::result::Result::ok)
            .collect::<Vec<_>>();

        let mut jobs_with_tags = Vec::new();
        for job in jobs {
            let tags = self.get_tags(&job.id)?;
            jobs_with_tags.push(JobRecord { tags, ..job });
        }

        Ok(jobs_with_tags)
    }

    /// Lists all active jobs (pending or running).
    ///
    /// Used to refresh job statuses from Slurm before displaying.
    pub fn list_active_jobs(&self) -> Result<Vec<JobRecord>> {
        let mut stmt = self.conn.prepare(
            r"
            SELECT id, slurm_id, job_name, project_name, project_path,
                   remote_host, remote_path, command, status, config_json,
                   created_at, updated_at, outputs_synced
            FROM jobs
            WHERE status IN ('pending', 'running')
            ORDER BY created_at DESC
            ",
        )?;

        let jobs = stmt
            .query_map([], |row| {
                Ok(JobRecord {
                    id: row.get(0)?,
                    slurm_id: row.get(1)?,
                    job_name: row.get(2)?,
                    project_name: row.get(3)?,
                    project_path: row.get(4)?,
                    remote_host: row.get(5)?,
                    remote_path: row.get(6)?,
                    command: row.get(7)?,
                    status: row
                        .get::<_, String>(8)?
                        .parse()
                        .unwrap_or(JobStatus::Pending),
                    config_json: row.get(9)?,
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(10)?)
                        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
                    updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(11)?)
                        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
                    outputs_synced: row.get::<_, i32>(12)? == 1,
                    tags: HashMap::new(),
                })
            })?
            .filter_map(std::result::Result::ok)
            .collect::<Vec<_>>();

        let mut jobs_with_tags = Vec::new();
        for job in jobs {
            let tags = self.get_tags(&job.id)?;
            jobs_with_tags.push(JobRecord { tags, ..job });
        }

        Ok(jobs_with_tags)
    }

    /// Deletes a job from the registry.
    ///
    /// Tags are automatically deleted via the CASCADE constraint.
    pub fn delete_job(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM jobs WHERE id = ?1", params![id])?;
        Ok(())
    }
}

/// Returns the path to the registry database file.
fn get_db_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| FlecheError::Other("Could not find config directory".to_string()))?;
    Ok(config_dir.join("fleche").join("jobs.db"))
}

/// Parses a duration string like "7d", "24h", or "30m".
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim().to_lowercase();

    if let Some(days) = s.strip_suffix('d') {
        let n: i64 = days
            .parse()
            .map_err(|_| FlecheError::Other(format!("Invalid duration: {s}")))?;
        return Ok(Duration::days(n));
    }

    if let Some(hours) = s.strip_suffix('h') {
        let n: i64 = hours
            .parse()
            .map_err(|_| FlecheError::Other(format!("Invalid duration: {s}")))?;
        return Ok(Duration::hours(n));
    }

    if let Some(minutes) = s.strip_suffix('m') {
        let n: i64 = minutes
            .parse()
            .map_err(|_| FlecheError::Other(format!("Invalid duration: {s}")))?;
        return Ok(Duration::minutes(n));
    }

    Err(FlecheError::Other(format!(
        "Invalid duration format: {s}. Use format like 7d, 24h, 30m"
    )))
}
