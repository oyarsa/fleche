//! Local job registry backed by `SQLite`.
//!
//! This module provides persistent storage for job records, including their status,
//! configuration, and associated tags. The database is stored in the user's config
//! directory (`~/.config/fleche/jobs.db`).

use crate::config::ResolvedJob;
use crate::error::{FlecheError, Result};
use chrono::{DateTime, Duration, Utc};
use regex::{Regex, RegexBuilder};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize, Serializer};
use std::collections::HashMap;
use std::path::PathBuf;

/// SQL columns for SELECT queries on the jobs table.
const JOB_SELECT_COLUMNS: &str = r"
    id, slurm_id, job_name, project_name, project_path,
    remote_host, remote_path, command, status, config_json,
    created_at, updated_at, outputs_synced, note, archived, exit_code,
    slurm_state, sacct_exit_code";

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
    #[serde(rename = "config", serialize_with = "serialize_config")]
    pub config_json: String,
    /// When the job was created.
    pub created_at: DateTime<Utc>,
    /// When the job record was last updated.
    pub updated_at: DateTime<Utc>,
    /// Whether outputs have been synced back to local.
    pub outputs_synced: bool,
    /// User-defined key-value tags for filtering jobs.
    pub tags: HashMap<String, String>,
    /// Optional user note/annotation for the job.
    pub note: Option<String>,
    /// Whether this job is archived (hidden from normal listings).
    pub archived: bool,
    /// Process exit code (0 = success, non-zero = failure).
    pub exit_code: Option<i32>,
    /// Raw Slurm job state from sacct (e.g., "TIMEOUT", "`OUT_OF_MEMORY`", "COMPLETED").
    pub slurm_state: Option<String>,
    /// Raw sacct exit code in "exitcode:signal" format (e.g., "0:0", "1:0", "0:15").
    pub sacct_exit_code: Option<String>,
}

/// Serializes the raw `config_json` string as a structured [`ResolvedJob`] object.
///
/// Falls back to `null` if the stored JSON can't be parsed (e.g., old records
/// from before a schema change).
fn serialize_config<S: Serializer>(config: &str, s: S) -> std::result::Result<S::Ok, S::Error> {
    match serde_json::from_str::<ResolvedJob>(config) {
        Ok(resolved) => resolved.serialize(s),
        Err(_) => s.serialize_none(),
    }
}

impl JobRecord {
    /// Creates a `JobRecord` from a `SQLite` row.
    ///
    /// The row must have columns in the order defined by `JOB_SELECT_COLUMNS`.
    /// Tags are initialized to an empty `HashMap` and should be loaded separately.
    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
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
                .expect("job status in database should be valid"),
            config_json: row.get(9)?,
            // Fall back to current time if timestamp is corrupted (allows viewing jobs)
            created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(10)?)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
            updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(11)?)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
            outputs_synced: row.get::<_, i32>(12)? == 1,
            tags: HashMap::new(),
            note: row.get(13)?,
            archived: row.get::<_, Option<i32>>(14)?.unwrap_or(0) == 1,
            exit_code: row.get(15)?,
            slurm_state: row.get(16)?,
            sacct_exit_code: row.get(17)?,
        })
    }
}

/// Controls which jobs are visible based on their archived state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ArchivedFilter {
    /// Show only non-archived jobs.
    #[default]
    ExcludeArchived,
    /// Show only archived jobs.
    OnlyArchived,
    /// Show all jobs regardless of archived state.
    IncludeAll,
}

/// Live status information returned by job backends.
///
/// Bundles the job status, optional exit code, and optional raw Slurm state
/// so callers don't need to thread multiple values separately.
pub struct LiveStatus {
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    pub slurm_state: Option<String>,
    pub sacct_exit_code: Option<String>,
}

impl LiveStatus {
    /// Creates a `LiveStatus` with only a status and no exit/Slurm details.
    pub fn new(status: JobStatus) -> Self {
        Self {
            status,
            exit_code: None,
            slurm_state: None,
            sacct_exit_code: None,
        }
    }

    /// Creates a `LiveStatus` with a status and exit code.
    pub fn with_exit_code(status: JobStatus, exit_code: i32) -> Self {
        Self {
            exit_code: Some(exit_code),
            ..Self::new(status)
        }
    }
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
            _ => Err(FlecheError::UnknownJobStatus(s.to_string())),
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
                outputs_synced INTEGER DEFAULT 0,
                note TEXT
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

        // Migration: add note column if it doesn't exist (for existing databases)
        let _ = self
            .conn
            .execute("ALTER TABLE jobs ADD COLUMN note TEXT", []);

        // Migration: add archived column if it doesn't exist (for existing databases)
        let _ = self.conn.execute(
            "ALTER TABLE jobs ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            [],
        );

        // Migration: add exit_code column if it doesn't exist (for existing databases)
        let _ = self
            .conn
            .execute("ALTER TABLE jobs ADD COLUMN exit_code INTEGER", []);

        // Migration: add slurm_state column if it doesn't exist (for existing databases)
        let _ = self
            .conn
            .execute("ALTER TABLE jobs ADD COLUMN slurm_state TEXT", []);

        // Migration: add sacct_exit_code column if it doesn't exist (for existing databases)
        let _ = self
            .conn
            .execute("ALTER TABLE jobs ADD COLUMN sacct_exit_code TEXT", []);

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
        note: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now();
        let config_json = serde_json::to_string(job)?;

        self.conn.execute(
            r"
            INSERT INTO jobs (id, slurm_id, job_name, project_name, project_path,
                              remote_host, remote_path, command, status, config_json,
                              created_at, updated_at, outputs_synced, note)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, ?13)
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
                note,
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

    /// Sets or updates the note for a job.
    pub fn set_note(&self, id: &str, note: Option<&str>) -> Result<()> {
        let now = Utc::now();
        self.conn.execute(
            "UPDATE jobs SET note = ?1, updated_at = ?2 WHERE id = ?3",
            params![note, now.to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Updates the status, exit code, and optional Slurm state of a job.
    pub fn update_status(&self, id: &str, live: &LiveStatus) -> Result<()> {
        let now = Utc::now();
        self.conn.execute(
            "UPDATE jobs SET status = ?1, updated_at = ?2, exit_code = ?3, slurm_state = ?4, sacct_exit_code = ?5 WHERE id = ?6",
            params![live.status.to_string(), now.to_rfc3339(), live.exit_code, live.slurm_state, live.sacct_exit_code, id],
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

    /// Retrieves a job by its numeric index (1-based, most recent first).
    ///
    /// Indices correspond to the position in the default job list (non-archived,
    /// ordered by `created_at DESC`), matching the `#` column in `fleche status`.
    ///
    /// Delegates to `list_jobs` so that ordering and filtering are defined in one place.
    fn get_job_by_index(&self, index: usize) -> Result<JobRecord> {
        let jobs = self.list_all_jobs(index)?;
        if let Some(job) = jobs.into_iter().last() {
            Ok(job)
        } else {
            let count: usize = self.conn.query_row(
                "SELECT COUNT(*) FROM jobs WHERE (archived = 0 OR archived IS NULL)",
                [],
                |row| row.get(0),
            )?;
            Err(FlecheError::JobIdNotFound(format!(
                "index {index} (only {count} jobs exist)"
            )))
        }
    }

    /// Retrieves a job by its ID, unique suffix, or numeric index.
    ///
    /// If `id` is a positive integer, it is treated as a 1-based index into the
    /// global job list (most recent first), matching the `#` column in `fleche status`.
    /// Otherwise, if `id` exactly matches a job ID, that job is returned.
    /// Otherwise, we search for jobs whose ID ends with `id`. If exactly one match
    /// is found, that job is returned. If multiple matches are found, an error is
    /// returned listing the ambiguous matches.
    pub fn get_job(&self, id: &str) -> Result<JobRecord> {
        // Try numeric index first
        if let Ok(index) = id.parse::<usize>() {
            if index >= 1 {
                return self.get_job_by_index(index);
            }
        }

        // Try exact match
        let sql = format!("SELECT {JOB_SELECT_COLUMNS} FROM jobs WHERE id = ?1");
        let mut stmt = self.conn.prepare(&sql)?;

        if let Ok(job) = stmt.query_row(params![id], JobRecord::from_row) {
            let tags = self.get_tags(&job.id)?;
            return Ok(JobRecord { tags, ..job });
        }

        // Try suffix match
        let suffix_pattern = format!("%{id}");
        let sql = format!(
            "SELECT {JOB_SELECT_COLUMNS} FROM jobs WHERE id LIKE ?1 ORDER BY created_at DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let matches: Vec<JobRecord> = stmt
            .query_map(params![suffix_pattern], JobRecord::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        match <[_; 1]>::try_from(matches) {
            Ok([job]) => {
                let tags = self.get_tags(&job.id)?;
                Ok(JobRecord { tags, ..job })
            }
            Err(v) if v.is_empty() => Err(FlecheError::JobIdNotFound(id.to_string())),
            Err(v) => {
                let ids: Vec<&str> = v.iter().map(|j| j.id.as_str()).collect();
                Err(FlecheError::AmbiguousJobId(id.to_string(), ids.join(", ")))
            }
        }
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
            .collect::<std::result::Result<HashMap<_, _>, _>>()?;
        Ok(tags)
    }

    /// Loads tags for a list of jobs, returning new records with tags populated.
    fn load_tags_for_jobs(&self, jobs: Vec<JobRecord>) -> Result<Vec<JobRecord>> {
        jobs.into_iter()
            .map(|job| {
                let tags = self.get_tags(&job.id)?;
                Ok(JobRecord { tags, ..job })
            })
            .collect()
    }

    /// Lists the most recent non-archived jobs with no filters.
    ///
    /// Convenience wrapper around `list_jobs` for the common case of fetching
    /// the global job list (e.g., for index-based lookup or statistics).
    pub fn list_all_jobs(&self, limit: usize) -> Result<Vec<JobRecord>> {
        self.list_jobs(
            None,
            &[],
            None,
            None,
            &[],
            ArchivedFilter::ExcludeArchived,
            limit,
        )
    }

    /// Lists the most recent non-archived jobs filtered by tags only.
    pub fn list_jobs_by_tags(
        &self,
        tags: &[(String, String)],
        limit: usize,
    ) -> Result<Vec<JobRecord>> {
        self.list_jobs(
            None,
            &[],
            None,
            None,
            tags,
            ArchivedFilter::ExcludeArchived,
            limit,
        )
    }

    /// Lists jobs matching the given filters.
    ///
    /// Jobs can be filtered by project path, status, name prefix, note content, and tags.
    /// Results are ordered by creation time (newest first) and limited to `limit` results.
    ///
    pub fn list_jobs(
        &self,
        project_filter: Option<&str>,
        status_filters: &[JobStatus],
        name_filter: Option<&str>,
        note_filter: Option<&str>,
        tag_filters: &[(String, String)],
        archived_filter: ArchivedFilter,
        limit: usize,
    ) -> Result<Vec<JobRecord>> {
        let mut sql = String::from(
            r"
            SELECT DISTINCT j.id, j.slurm_id, j.job_name, j.project_name, j.project_path,
                   j.remote_host, j.remote_path, j.command, j.status, j.config_json,
                   j.created_at, j.updated_at, j.outputs_synced, j.note, j.archived,
                   j.exit_code, j.slurm_state, j.sacct_exit_code
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

        if !status_filters.is_empty() {
            let placeholders: Vec<&str> = status_filters.iter().map(|_| "?").collect();
            conditions.push(format!("j.status IN ({})", placeholders.join(", ")));
            for status in status_filters {
                params_vec.push(Box::new(status.to_string()));
            }
        }

        match archived_filter {
            ArchivedFilter::ExcludeArchived => {
                conditions.push("(j.archived = 0 OR j.archived IS NULL)".to_string());
            }
            ArchivedFilter::OnlyArchived => {
                conditions.push("j.archived = 1".to_string());
            }
            ArchivedFilter::IncludeAll => {}
        }

        // Build regex for name filtering (applied in Rust after SQL query)
        let name_regex = if let Some(pattern) = name_filter {
            let regex = build_job_filter_pattern(pattern);
            Some(Regex::new(&regex).map_err(|e| {
                FlecheError::InvalidRegexPattern(format!("--name '{pattern}': {e}"))
            })?)
        } else {
            None
        };

        // Build regex for note filtering (applied in Rust, case-insensitive)
        let note_regex = if let Some(pattern) = note_filter {
            let regex = build_job_filter_pattern(pattern);
            Some(
                RegexBuilder::new(&regex)
                    .case_insensitive(true)
                    .build()
                    .map_err(|e| {
                        FlecheError::InvalidRegexPattern(format!("--note '{pattern}': {e}"))
                    })?,
            )
        } else {
            None
        };

        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }

        // Fetch more rows than needed when filtering by regex (applied in Rust).
        // Use a reasonable cap to prevent loading the entire database.
        let sql_limit: i64 = if name_filter.is_some() || note_filter.is_some() {
            i64::try_from(limit.saturating_mul(10).max(1000)).unwrap_or(i64::MAX)
        } else {
            i64::try_from(limit).unwrap_or(i64::MAX)
        };
        sql.push_str(" ORDER BY j.created_at DESC LIMIT ?");
        params_vec.push(Box::new(sql_limit));

        let mut stmt = self.conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(std::convert::AsRef::as_ref).collect();

        let jobs: Vec<_> = stmt
            .query_map(params_refs.as_slice(), JobRecord::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|job| name_regex.as_ref().is_none_or(|re| re.is_match(&job.id)))
            .filter(|job| {
                note_regex
                    .as_ref()
                    .is_none_or(|re| job.note.as_ref().is_some_and(|n| re.is_match(n)))
            })
            .take(limit)
            .collect();

        self.load_tags_for_jobs(jobs)
    }

    /// Lists finished jobs older than the given duration.
    ///
    /// Only returns jobs with status completed, failed, or cancelled.
    /// Excludes archived jobs.
    pub fn list_jobs_older_than(&self, duration: Duration) -> Result<Vec<JobRecord>> {
        let cutoff = Utc::now() - duration;
        let sql = format!(
            "SELECT {JOB_SELECT_COLUMNS} FROM jobs \
             WHERE created_at < ?1 AND status IN ('completed', 'failed', 'cancelled') \
             AND (archived = 0 OR archived IS NULL) \
             ORDER BY created_at DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let jobs: Vec<_> = stmt
            .query_map(params![cutoff.to_rfc3339()], JobRecord::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        self.load_tags_for_jobs(jobs)
    }

    /// Lists all finished jobs (completed, failed, or cancelled).
    /// Excludes archived jobs.
    pub fn list_finished_jobs(&self) -> Result<Vec<JobRecord>> {
        let sql = format!(
            "SELECT {JOB_SELECT_COLUMNS} FROM jobs \
             WHERE status IN ('completed', 'failed', 'cancelled') \
             AND (archived = 0 OR archived IS NULL) \
             ORDER BY created_at DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let jobs: Vec<_> = stmt
            .query_map([], JobRecord::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        self.load_tags_for_jobs(jobs)
    }

    /// Lists all active jobs (pending or running).
    ///
    /// Used to refresh job statuses from Slurm before displaying.
    /// Excludes archived jobs.
    pub fn list_active_jobs(&self) -> Result<Vec<JobRecord>> {
        let sql = format!(
            "SELECT {JOB_SELECT_COLUMNS} FROM jobs \
             WHERE status IN ('pending', 'running') \
             AND (archived = 0 OR archived IS NULL) \
             ORDER BY created_at DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let jobs: Vec<_> = stmt
            .query_map([], JobRecord::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        self.load_tags_for_jobs(jobs)
    }

    /// Deletes a job from the registry.
    ///
    /// Tags are automatically deleted via the CASCADE constraint.
    pub fn delete_job(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM jobs WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Archives a job, hiding it from normal listings.
    pub fn archive_job(&self, id: &str) -> Result<()> {
        let now = Utc::now();
        self.conn.execute(
            "UPDATE jobs SET archived = 1, updated_at = ?1 WHERE id = ?2",
            params![now.to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Unarchives a job, restoring it to normal listings.
    pub fn unarchive_job(&self, id: &str) -> Result<()> {
        let now = Utc::now();
        self.conn.execute(
            "UPDATE jobs SET archived = 0, updated_at = ?1 WHERE id = ?2",
            params![now.to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Lists all archived jobs.
    pub fn list_archived_jobs(&self) -> Result<Vec<JobRecord>> {
        let sql = format!(
            "SELECT {JOB_SELECT_COLUMNS} FROM jobs \
             WHERE archived = 1 \
             ORDER BY created_at DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let jobs: Vec<_> = stmt
            .query_map([], JobRecord::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        self.load_tags_for_jobs(jobs)
    }

    /// Lists archived jobs older than the given duration.
    pub fn list_archived_jobs_older_than(&self, duration: Duration) -> Result<Vec<JobRecord>> {
        let cutoff = Utc::now() - duration;
        let sql = format!(
            "SELECT {JOB_SELECT_COLUMNS} FROM jobs \
             WHERE created_at < ?1 AND archived = 1 \
             ORDER BY created_at DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let jobs: Vec<_> = stmt
            .query_map(params![cutoff.to_rfc3339()], JobRecord::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        self.load_tags_for_jobs(jobs)
    }

    /// Lists all unique tag key-value pairs across all jobs.
    pub fn list_unique_tags(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT key, value FROM job_tags ORDER BY key, value")?;
        let tags = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(tags)
    }
}

/// Builds a permissive regex for job filters.
///
/// Adds implicit `.*` around unanchored patterns for substring matching.
pub fn build_job_filter_pattern(pattern: &str) -> String {
    let pattern = if pattern.starts_with('^') {
        pattern.to_string()
    } else {
        format!(".*{pattern}")
    };

    if pattern.ends_with('$') {
        pattern
    } else {
        format!("{pattern}.*")
    }
}

/// Returns the path to the registry database file.
fn get_db_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().ok_or(FlecheError::ConfigDirNotFound)?;
    Ok(config_dir.join("fleche").join("jobs.db"))
}

/// Parses a duration string like "7d", "24h", or "30m".
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim().to_lowercase();

    if let Some(days) = s.strip_suffix('d') {
        let n: i64 = days
            .parse()
            .map_err(|_| FlecheError::InvalidDuration(s.clone()))?;
        return Ok(Duration::days(n));
    }

    if let Some(hours) = s.strip_suffix('h') {
        let n: i64 = hours
            .parse()
            .map_err(|_| FlecheError::InvalidDuration(s.clone()))?;
        return Ok(Duration::hours(n));
    }

    if let Some(minutes) = s.strip_suffix('m') {
        let n: i64 = minutes
            .parse()
            .map_err(|_| FlecheError::InvalidDuration(s.clone()))?;
        return Ok(Duration::minutes(n));
    }

    Err(FlecheError::InvalidDuration(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_days() {
        let d = parse_duration("7d").unwrap();
        assert_eq!(d.num_days(), 7);

        let d = parse_duration("1d").unwrap();
        assert_eq!(d.num_days(), 1);

        let d = parse_duration("30D").unwrap();
        assert_eq!(d.num_days(), 30);
    }

    #[test]
    fn test_parse_duration_hours() {
        let d = parse_duration("24h").unwrap();
        assert_eq!(d.num_hours(), 24);

        let d = parse_duration("1h").unwrap();
        assert_eq!(d.num_hours(), 1);

        let d = parse_duration("48H").unwrap();
        assert_eq!(d.num_hours(), 48);
    }

    #[test]
    fn test_parse_duration_minutes() {
        let d = parse_duration("30m").unwrap();
        assert_eq!(d.num_minutes(), 30);

        let d = parse_duration("60M").unwrap();
        assert_eq!(d.num_minutes(), 60);
    }

    #[test]
    fn test_parse_duration_with_whitespace() {
        let d = parse_duration("  7d  ").unwrap();
        assert_eq!(d.num_days(), 7);
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration("7").is_err());
        assert!(parse_duration("d").is_err());
        assert!(parse_duration("7x").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn test_build_job_filter_pattern_unanchored() {
        assert_eq!(build_job_filter_pattern("train"), ".*train.*");
    }

    #[test]
    fn test_build_job_filter_pattern_anchored_start() {
        assert_eq!(build_job_filter_pattern("^train"), "^train.*");
    }

    #[test]
    fn test_build_job_filter_pattern_anchored_end() {
        assert_eq!(build_job_filter_pattern("train$"), ".*train$");
    }

    #[test]
    fn test_build_job_filter_pattern_anchored_both() {
        assert_eq!(build_job_filter_pattern("^train$"), "^train$");
    }

    #[test]
    fn test_job_status_to_string() {
        assert_eq!(JobStatus::Pending.to_string(), "pending");
        assert_eq!(JobStatus::Running.to_string(), "running");
        assert_eq!(JobStatus::Completed.to_string(), "completed");
        assert_eq!(JobStatus::Failed.to_string(), "failed");
        assert_eq!(JobStatus::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn test_job_status_from_str() {
        assert_eq!("pending".parse::<JobStatus>().unwrap(), JobStatus::Pending);
        assert_eq!("running".parse::<JobStatus>().unwrap(), JobStatus::Running);
        assert_eq!(
            "completed".parse::<JobStatus>().unwrap(),
            JobStatus::Completed
        );
        assert_eq!("failed".parse::<JobStatus>().unwrap(), JobStatus::Failed);
        assert_eq!(
            "cancelled".parse::<JobStatus>().unwrap(),
            JobStatus::Cancelled
        );
    }

    #[test]
    fn test_job_status_from_str_case_insensitive() {
        assert_eq!("PENDING".parse::<JobStatus>().unwrap(), JobStatus::Pending);
        assert_eq!("Running".parse::<JobStatus>().unwrap(), JobStatus::Running);
        assert_eq!(
            "COMPLETED".parse::<JobStatus>().unwrap(),
            JobStatus::Completed
        );
    }

    #[test]
    fn test_job_status_from_str_invalid() {
        assert!("unknown".parse::<JobStatus>().is_err());
        assert!("".parse::<JobStatus>().is_err());
        assert!("pend".parse::<JobStatus>().is_err());
    }
}
