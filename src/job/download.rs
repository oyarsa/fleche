//! Job output download operations.

use crate::config::ResolvedJob;
use crate::error::{FlecheError, Result};
use crate::registry::{JobStatus, Registry};
use crate::runtime::RuntimeCtx;
use crate::slurm::get_job_status;
use crate::ssh::SshClient;
use crate::sync::{
    DownloadOptions, download_outputs as sync_download_outputs, download_path as sync_download_path,
};
use console::style;
use globset::{Glob, GlobSet, GlobSetBuilder};

use super::status::resolve_job;

/// Downloads output files from a job's workspace back to the local project.
pub async fn download_outputs(
    job_id: Option<&str>,
    partial: bool,
    specific_path: Option<&str>,
    filters: &[String],
    tags: &[(String, String)],
    dry_run: bool,
    ctx: RuntimeCtx,
) -> Result<()> {
    let registry = Registry::open()?;
    let job = resolve_job(&registry, job_id, tags, None)?;

    // Local jobs don't need downloading - files are already in place
    if job.remote_host == "local" {
        println!(
            "{}",
            style("Job ran locally - output files are already in the project directory.").green()
        );
        return Ok(());
    }

    let ssh = ctx.ssh(&job.remote_host);

    // Check job status
    if !partial && matches!(job.status, JobStatus::Pending | JobStatus::Running) {
        if let Some(ref slurm_id) = job.slurm_id {
            let current_status = get_job_status(&ssh, slurm_id).await.unwrap_or(job.status);
            if matches!(current_status, JobStatus::Pending | JobStatus::Running) {
                eprintln!(
                    "{}",
                    style("Warning: Job is still running. Use --partial to download anyway.")
                        .yellow()
                );
                return Ok(());
            }
        }
    }

    let local_path = std::path::PathBuf::from(&job.project_path);
    let download_options = DownloadOptions { dry_run };

    if let Some(path) = specific_path {
        if dry_run {
            println!("Would download {path} from workspace...");
        } else {
            println!("Downloading {path} from workspace...");
        }
        sync_download_path(
            &job.remote_host,
            &job.remote_path,
            path,
            &local_path,
            &download_options,
        )
        .await?;
    } else {
        // Parse config to get outputs
        let resolved: ResolvedJob = serde_json::from_str(&job.config_json)?;

        if resolved.outputs.is_empty() {
            println!("No outputs defined for this job.");
            return Ok(());
        }

        // Determine files to download
        let outputs = if filters.is_empty() {
            // No filters: use configured outputs as-is (fast path)
            resolved.outputs
        } else {
            // Filters provided: expand directories and filter individual files
            expand_and_filter_outputs(&ssh, &job.remote_path, &resolved.outputs, filters).await?
        };

        if outputs.is_empty() {
            println!("No outputs match the specified filters.");
            return Ok(());
        }

        if dry_run {
            println!("Would download outputs from workspace:");
        } else {
            println!("Downloading outputs from workspace...");
        }
        for output in &outputs {
            println!("  {output}");
        }
        sync_download_outputs(
            &job.remote_host,
            &job.remote_path,
            &outputs,
            &local_path,
            &download_options,
        )
        .await?;
    }

    if !dry_run {
        registry.set_outputs_synced(&job.id)?;
        println!("{}", style("Download complete.").green());
    }

    Ok(())
}

/// Expands directory outputs to individual files and applies glob filters.
///
/// For each configured output:
/// - If it's a file: check if it matches the filter
/// - If it's a directory: list all files recursively and include those that match
async fn expand_and_filter_outputs(
    ssh: &SshClient,
    workspace: &str,
    outputs: &[String],
    filters: &[String],
) -> Result<Vec<String>> {
    let (includes, excludes) = build_filter_glob_sets(filters)?;
    let mut result = Vec::new();

    for output in outputs {
        let output_trimmed = output.trim_end_matches('/');
        let remote_path = format!("{workspace}/{output_trimmed}");

        if ssh.is_dir(&remote_path).await? {
            // It's a directory: list all files and filter
            let files = ssh.list_files_recursive(&remote_path).await?;
            for file in files {
                let full_path = format!("{output_trimmed}/{file}");
                if matches_filters(&full_path, includes.as_ref(), excludes.as_ref()) {
                    result.push(full_path);
                }
            }
        } else {
            // It's a file: check if it matches
            if matches_filters(output_trimmed, includes.as_ref(), excludes.as_ref()) {
                result.push(output_trimmed.to_string());
            }
        }
    }

    Ok(result)
}

/// Checks if a path matches the include/exclude filters.
fn matches_filters(path: &str, includes: Option<&GlobSet>, excludes: Option<&GlobSet>) -> bool {
    let matches_include = match includes {
        Some(set) => set.is_match(path),
        None => true,
    };

    let matches_exclude = match excludes {
        Some(set) => set.is_match(path),
        None => false,
    };

    matches_include && !matches_exclude
}

/// Builds include and exclude glob sets from filter patterns.
///
/// Patterns prefixed with `!` are exclusions, others are inclusions.
fn build_filter_glob_sets(filters: &[String]) -> Result<(Option<GlobSet>, Option<GlobSet>)> {
    let mut includes = GlobSetBuilder::new();
    let mut excludes = GlobSetBuilder::new();
    let mut has_includes = false;
    let mut has_excludes = false;

    for pattern in filters {
        if let Some(exclude_pattern) = pattern.strip_prefix('!') {
            excludes.add(Glob::new(exclude_pattern).map_err(|e| {
                FlecheError::InvalidGlobPattern(format!("'{exclude_pattern}': {e}"))
            })?);
            has_excludes = true;
        } else {
            includes.add(
                Glob::new(pattern)
                    .map_err(|e| FlecheError::InvalidGlobPattern(format!("'{pattern}': {e}")))?,
            );
            has_includes = true;
        }
    }

    let include_set = if has_includes {
        Some(
            includes
                .build()
                .map_err(|e| FlecheError::InvalidGlobPattern(e.to_string()))?,
        )
    } else {
        None
    };

    let exclude_set = if has_excludes {
        Some(
            excludes
                .build()
                .map_err(|e| FlecheError::InvalidGlobPattern(e.to_string()))?,
        )
    } else {
        None
    };

    Ok((include_set, exclude_set))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_filters_no_filters() {
        assert!(matches_filters("a.json", None, None));
        assert!(matches_filters("b.csv", None, None));
    }

    #[test]
    fn test_matches_filters_include_only() {
        let (includes, excludes) = build_filter_glob_sets(&["*.json".to_string()]).unwrap();
        assert!(matches_filters(
            "predictions.json",
            includes.as_ref(),
            excludes.as_ref()
        ));
        assert!(!matches_filters(
            "model.pt",
            includes.as_ref(),
            excludes.as_ref()
        ));
        assert!(!matches_filters(
            "data.csv",
            includes.as_ref(),
            excludes.as_ref()
        ));
    }

    #[test]
    fn test_matches_filters_multiple_includes() {
        let (includes, excludes) =
            build_filter_glob_sets(&["*.json".to_string(), "*.csv".to_string()]).unwrap();
        assert!(matches_filters(
            "predictions.json",
            includes.as_ref(),
            excludes.as_ref()
        ));
        assert!(!matches_filters(
            "model.pt",
            includes.as_ref(),
            excludes.as_ref()
        ));
        assert!(matches_filters(
            "data.csv",
            includes.as_ref(),
            excludes.as_ref()
        ));
    }

    #[test]
    fn test_matches_filters_exclude_only() {
        let (includes, excludes) =
            build_filter_glob_sets(&["!checkpoints/**".to_string()]).unwrap();
        assert!(matches_filters(
            "predictions.json",
            includes.as_ref(),
            excludes.as_ref()
        ));
        assert!(!matches_filters(
            "checkpoints/model.pt",
            includes.as_ref(),
            excludes.as_ref()
        ));
        assert!(!matches_filters(
            "checkpoints/final.pt",
            includes.as_ref(),
            excludes.as_ref()
        ));
    }

    #[test]
    fn test_matches_filters_include_and_exclude() {
        let (includes, excludes) =
            build_filter_glob_sets(&["*.json".to_string(), "!checkpoints/**".to_string()]).unwrap();
        assert!(matches_filters(
            "results/predictions.json",
            includes.as_ref(),
            excludes.as_ref()
        ));
        assert!(matches_filters(
            "results/debug.json",
            includes.as_ref(),
            excludes.as_ref()
        ));
        assert!(!matches_filters(
            "checkpoints/model.json",
            includes.as_ref(),
            excludes.as_ref()
        ));
    }

    #[test]
    fn test_matches_filters_directory_with_trailing_slash() {
        let (includes, excludes) = build_filter_glob_sets(&["output".to_string()]).unwrap();
        // Note: matches_filters doesn't strip trailing slash, that's handled by caller
        assert!(matches_filters(
            "output",
            includes.as_ref(),
            excludes.as_ref()
        ));
        assert!(!matches_filters(
            "checkpoints",
            includes.as_ref(),
            excludes.as_ref()
        ));
    }

    #[test]
    fn test_build_filter_glob_sets_invalid_pattern() {
        let result = build_filter_glob_sets(&["[invalid".to_string()]);
        assert!(result.is_err());
    }
}
