use crate::config::ResolvedJob;
use crate::error::{FlecheError, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub slurm_id: Option<String>,
    pub job_name: String,
    pub project_name: String,
    pub project_path: String,
    pub remote_host: String,
    pub remote_path: String,
    pub command: String,
    pub status: JobStatus,
    pub config_json: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub outputs_synced: bool,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
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

pub struct Registry {
    conn: Connection,
}

impl Registry {
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

    pub fn update_status(&self, id: &str, status: JobStatus) -> Result<()> {
        let now = Utc::now();
        self.conn.execute(
            "UPDATE jobs SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.to_string(), now.to_rfc3339(), id],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn update_slurm_id(&self, id: &str, slurm_id: &str) -> Result<()> {
        let now = Utc::now();
        self.conn.execute(
            "UPDATE jobs SET slurm_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![slurm_id, now.to_rfc3339(), id],
        )?;
        Ok(())
    }

    pub fn set_outputs_synced(&self, id: &str) -> Result<()> {
        let now = Utc::now();
        self.conn.execute(
            "UPDATE jobs SET outputs_synced = 1, updated_at = ?1 WHERE id = ?2",
            params![now.to_rfc3339(), id],
        )?;
        Ok(())
    }

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

    pub fn delete_job(&self, id: &str) -> Result<()> {
        // Tags are deleted automatically via CASCADE
        self.conn
            .execute("DELETE FROM jobs WHERE id = ?1", params![id])?;
        Ok(())
    }
}

fn get_db_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| FlecheError::Other("Could not find config directory".to_string()))?;
    Ok(config_dir.join("fleche").join("jobs.db"))
}

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
