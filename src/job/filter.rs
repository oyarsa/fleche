//! Shared job-selection filtering.
//!
//! All listing/resolution commands select jobs by the same four attributes —
//! status, ID (name), tags, and note — gathered from the unified `--filter`
//! flag. [`JobFilters`] is the borrowed view passed to handlers; [`list_matching`]
//! and [`resolve_job`] run the query against the registry, and [`CompiledFilters`]
//! does in-memory matching for paths that gather candidates another way (e.g.
//! `clean --before`).

use crate::error::{FlecheError, Result};
use crate::registry::{ArchivedFilter, JobRecord, JobStatus, Registry, build_job_filter_pattern};
use regex::{Regex, RegexBuilder};

/// A borrowed set of job-selection predicates, `ANDed` together.
///
/// Statuses are a membership test (a job matches if its status is any of them);
/// `name` and `note` are regexes; `tags` must all be present.
#[derive(Clone, Copy, Default)]
pub struct JobFilters<'a> {
    /// Status strings (e.g. `"running"`); empty means "any status".
    pub statuses: &'a [String],
    /// Regex matched against the job ID.
    pub name: Option<&'a str>,
    /// Key-value pairs that must all match.
    pub tags: &'a [(String, String)],
    /// Regex matched against the job note (case-insensitive).
    pub note: Option<&'a str>,
}

impl JobFilters<'_> {
    /// Returns true when no predicate is set (matches every job).
    pub fn is_empty(&self) -> bool {
        self.statuses.is_empty()
            && self.name.is_none()
            && self.tags.is_empty()
            && self.note.is_none()
    }
}

/// Parses raw status strings into [`JobStatus`] values.
pub fn parse_statuses(statuses: &[String]) -> Result<Vec<JobStatus>> {
    statuses.iter().map(|s| s.parse()).collect()
}

/// Lists non-archived jobs matching `filters`, newest first, up to `limit`.
pub fn list_matching(
    registry: &Registry,
    filters: JobFilters<'_>,
    limit: usize,
) -> Result<Vec<JobRecord>> {
    list_matching_archived(registry, filters, ArchivedFilter::ExcludeArchived, limit)
}

/// Like [`list_matching`] but with an explicit archived filter.
pub fn list_matching_archived(
    registry: &Registry,
    filters: JobFilters<'_>,
    archived: ArchivedFilter,
    limit: usize,
) -> Result<Vec<JobRecord>> {
    let statuses = parse_statuses(filters.statuses)?;
    registry.list_jobs(
        None,
        &statuses,
        filters.name,
        filters.note,
        filters.tags,
        archived,
        limit,
    )
}

/// Resolves an explicit job ID, or the most recent job matching `filters`.
pub fn resolve_job(
    registry: &Registry,
    job_id: Option<&str>,
    filters: JobFilters<'_>,
) -> Result<JobRecord> {
    if let Some(id) = job_id {
        registry.get_job(id)
    } else {
        list_matching(registry, filters, 1)?
            .into_iter()
            .next()
            .ok_or(FlecheError::NoRecentJob)
    }
}

/// Pre-compiled filters for in-memory matching against already-fetched jobs.
///
/// Used where candidates are gathered by a query that `list_jobs` can't express
/// (e.g. "older than a duration"), so the filters are applied in Rust instead.
pub struct CompiledFilters {
    statuses: Vec<JobStatus>,
    name: Option<Regex>,
    note: Option<Regex>,
    tags: Vec<(String, String)>,
}

impl CompiledFilters {
    /// Compiles the regex predicates and parses statuses once up front.
    pub fn compile(filters: JobFilters<'_>) -> Result<Self> {
        let name = filters
            .name
            .map(|p| {
                Regex::new(&build_job_filter_pattern(p))
                    .map_err(|e| FlecheError::InvalidRegexPattern(format!("name '{p}': {e}")))
            })
            .transpose()?;

        let note = filters
            .note
            .map(|p| {
                RegexBuilder::new(&build_job_filter_pattern(p))
                    .case_insensitive(true)
                    .build()
                    .map_err(|e| FlecheError::InvalidRegexPattern(format!("note '{p}': {e}")))
            })
            .transpose()?;

        Ok(Self {
            statuses: parse_statuses(filters.statuses)?,
            name,
            note,
            tags: filters.tags.to_vec(),
        })
    }

    /// Returns true if `job` satisfies all configured predicates.
    pub fn matches(&self, job: &JobRecord) -> bool {
        (self.statuses.is_empty() || self.statuses.contains(&job.status))
            && self.name.as_ref().is_none_or(|re| re.is_match(&job.id))
            && self
                .note
                .as_ref()
                .is_none_or(|re| job.note.as_ref().is_some_and(|n| re.is_match(n)))
            && self.tags.iter().all(|(k, v)| job.tags.get(k) == Some(v))
    }
}
