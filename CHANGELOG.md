# Changelog

All notable changes to this project will be documented in this file.

## [6.1.0] - 2026-01-19

### Added
- `--dry-run` flag for `fleche download` to preview what would be downloaded
  - Shows files that match filters without actually downloading
  - Passes through to rsync's dry-run for accurate transfer preview

### Changed
- `--filter` now searches inside directories for matching files
  - Previously matched only against configured output path names
  - Now lists files recursively on remote and filters individual files
  - Example: `--filter "*.json"` finds JSON files inside `outputs/` directory

## [6.0.1] - 2026-01-19

### Fixed
- Shell-escape Slurm job IDs in commands to prevent injection
- Validate environment variable names before exporting in sbatch scripts
- Truncate job names to fit Slurm's length limit (~200 chars)

### Changed
- Added `NoSlurmId` and `SlurmUnavailable` error variants for clearer errors
- Removed generic `FlecheError::Other` (all usages now have specific variants)
- Extracted SSH socket directory setup to shared function

## [6.0.0] - 2026-01-19

### Changed
- Error messages are now more specific with dedicated error types
  - `NoRecentJob` for operations expecting a recent job
  - `InvalidDuration` for duration parsing (e.g., `7d`, `24h`)
  - `InvalidGlobPattern` for filter patterns
  - `SlurmQueryFailed` for Slurm status queries
- Internal refactoring: options structs replace multiple boolean parameters
- Removed unused internal functions

### Added
- Additional unit tests for `generate_job_id` and `truncate` functions

## [5.9.0] - 2026-01-18

### Added
- `--filter` flag for `fleche download` to selectively download outputs
  - Accepts glob patterns: `--filter "*.json"` downloads only JSON files
  - Repeatable: `--filter "*.json" --filter "*.csv"` for multiple patterns
  - Prefix with `!` to exclude: `--filter "!checkpoints/**"` skips checkpoints
  - Combine includes and excludes: `--filter "*.json" --filter "!debug/**"`

## [5.8.2] - 2026-01-17

### Fixed
- SSH verbose output (`debug1:`, `OpenSSH_` lines) no longer shown without `--debug`
  - Previously, SSH always ran with `-v` flag, causing debug output to appear in `fleche exec`

## [5.8.1] - 2026-01-17

### Fixed
- `--env` now correctly overrides config variables during expansion
  - Previously, `${VAR}` in commands/inputs/outputs was expanded at config load time, before CLI `--env` values were known
  - Now expansion happens after merging: global env → job env → CLI `--env` (highest precedence)

## [5.8.0] - 2026-01-17

### Added
- Built-in `${PROJECT}` variable that expands to `project.name`
  - Enables DRY configs: `UV_PROJECT_ENVIRONMENT = "${CACHE}/${PROJECT}/.venv"`
  - Has highest precedence (cannot be overridden by env vars)

## [5.7.0] - 2026-01-17

### Added
- Automatic `.env` file loading for variable expansion
  - Variables in `.env` are available as fallbacks after system env vars
  - Enables project-specific defaults without external tools (mise, direnv)
  - Resolution order: config `[env]` → system env → `.env` file

## [5.6.0] - 2026-01-17

### Added
- Environment variable substitution in config files with `${VAR}` syntax
  - Variables resolve to previously-defined `[env]` entries or system env vars
  - Supports `${VAR:-default}` for fallback values
  - Works in `remote.base_path`, `[env]`, job `inputs`/`outputs`/`command`/`env`
  - Enables user-agnostic configs (e.g., `base_path = "/scratch/${USER}/fleche"`)

## [5.5.1] - 2026-01-17

### Fixed
- `--name` filter now correctly matches against job ID instead of job definition name

## [5.5.0] - 2026-01-17

### Changed
- `--name` filter now uses regex instead of glob patterns, with implicit `.*` around
  - `fleche status --name 123` matches jobs containing "123" (e.g., "train-123-xy")
  - `fleche status --name '^train'` matches jobs starting with "train"
  - `fleche status --name 'ablation$'` matches jobs ending with "ablation"

## [5.4.0] - 2026-01-17

### Added
- `--name` filter for `fleche status` to filter jobs by name pattern

## [5.3.0] - 2026-01-16

### Added
- `fleche ping` command to check Slurm cluster health via `scontrol ping`
- `fleche wait` command to wait for a job to complete (with optional `--notify`)
- `--notify` flag for `fleche run` to send terminal notification when background job completes
- `--raw` flag for `fleche logs` to strip ANSI escape codes from output
- Auto-strip ANSI codes when logs output is piped (detected via `isatty`)
- Short job ID matching by suffix (e.g., `fleche logs 7rhh` instead of full ID)

### Changed
- Timeout error messages now provide context-specific suggestions (sbatch timeouts suggest `fleche ping`)
- Terminal notifications now prefixed with "fleche:" for clarity
- Job not found error now suggests `fleche status` instead of `fleche list`

## [5.2.1] - 2026-01-16

### Changed
- Version output now follows GNU style with copyright, license, and release date

## [5.2.0] - 2026-01-16

### Added
- `fleche tags` command to list all unique tags across jobs
- `fleche rerun <job-id>` command to re-run a previous job with same settings
- `-n/--last <N>` option for `fleche status` to limit number of jobs shown (default: 20)
- Support multiple `--filter` values for status (e.g., `--filter running --filter pending`)

### Fixed
- Streaming race condition: quick jobs could complete before output was captured
- Tag filtering with `--all` no longer silently truncates at 100/1000 jobs

### Changed
- Job name now shown on second line in status when different from job ID prefix
- Improved visibility of job names and tags in status output

## [5.1.0] - 2026-01-16

### Added
- Tag filtering for `status`, `logs`, `download`, `cancel`, and `clean` commands
- Tags now displayed in status table output (dimmed second line below each job)
- `--tag` option is repeatable to filter by multiple tags

### Examples
```bash
fleche status --tag experiment=ablation
fleche logs --tag model=8b
fleche download --tag experiment=ablation
fleche cancel --all --tag experiment=test
fleche clean --older-than 7d --tag experiment=old
```

## [5.0.4] - 2026-01-15

### Added
- SSH command execution timeout (60s) with automatic retry on stale socket
- Auto-cleanup of stale ControlMaster sockets on timeout

## [5.0.3] - 2026-01-15

### Fixed
- rsync now creates parent directories for nested input paths (`--mkpath`)

## [5.0.2] - 2026-01-15

### Fixed
- Updated README for v5.0.0 changes

## [5.0.1] - 2026-01-15

### Fixed
- Code formatting issues

## [5.0.0] - 2026-01-15

### Breaking Changes
- `fleche sync` renamed to `fleche download`
- `fleche list` merged into `fleche status` (list is now the default)
- Removed isolated job directories - all jobs now share a workspace
- Removed input file caching/symlinks - files are copied directly

### Added
- `fleche exec` - execute commands directly via SSH without Slurm
- `--bg` flag for `fleche run` to run in background (streaming is now default)
- Job ID is now optional for `logs`, `download`, `cancel` (defaults to most recent)
- `--filter` option for `fleche status` to filter by job status
- `--path` option for `fleche download` to download specific paths
- `--workspace` flag for `fleche clean` to also delete the shared workspace

### Changed
- Streaming output is now the default (use `--bg` to opt out)
- All jobs run in a shared workspace directory (`.fleche/workspace/`)
- Job logs and metadata go to separate directory (`.fleche/jobs/<id>/`)
- Simplified sync - just rsync to workspace, no hash checking or caching
- Input files are copied directly, not symlinked from cache

### Removed
- `--follow` flag (streaming is now default)
- `fleche list` (merged into `fleche status`)
- Input caching and symlinks

## [4.6.0] - 2026-01-15

### Added
- `fleche cancel --all` to cancel all running/pending jobs with confirmation
- `fleche clean --all` now requires confirmation before deleting (use `-y` to skip)
- `-y/--yes` flag for both cancel and clean to skip confirmation prompts

## [4.5.0] - 2026-01-15

### Added
- SSH connection timeout (`ConnectTimeout=30`) to fail fast on unreachable hosts
- SSH keepalive (`ServerAliveInterval=15`, `ServerAliveCountMax=3`) to detect dead connections
- SSH batch mode (`BatchMode=yes`) to fail immediately on MFA/password prompts instead of hanging

## [4.4.0] - 2026-01-15

### Fixed
- SSH socket path now uses `/tmp/fleche-ssh-<uid>/` to avoid Unix domain socket path length limit (~104 bytes)

## [4.3.0] - 2026-01-15

### Fixed
- Quote `ControlPath` value in SSH options to handle special characters in path

## [4.2.0] - 2026-01-15

### Fixed
- SSH `ControlMaster` option now uses correct case (was incorrectly backtick-quoted)

## [4.1.0] - 2026-01-15

### Fixed
- Input symlinks now use correct relative path depth for nested directories (e.g., `output/baselines/data` now correctly links to `../../../cache/...` instead of `../cache/...`)

## [4.0.0] - 2026-01-15

### Added
- SSH `ControlMaster` connection multiplexing to avoid rate limiting when running parallel commands
- Automatic retry with exponential backoff for SSH connection failures (3 retries, 1s/2s/4s delays)
- rsync now also uses `ControlMaster` for consistent connection sharing

### Changed
- SSH sockets stored in `~/.config/fleche/ssh-sockets/`
- Connections persist for 10 minutes after last use (`ControlPersist=600`)

## [3.2.0] - 2026-01-15

### Added
- SSH verbose output now always logged to `~/.config/fleche/ssh.log` for debugging intermittent connection issues (auto-truncates at 1MB)

## [3.1.0] - 2026-01-15

### Added
- `--debug` global flag for verbose SSH output to diagnose connection issues

## [3.0.0] - 2026-01-15

### Added
- `--tail/-n` option to limit logs output to last N lines
- `--stdout` flag to show only stdout in logs

### Changed
- `fleche logs` now shows both stdout and stderr by default
- `--stderr` flag now means "show only stderr" (previously was "show stderr instead of stdout")

## [2.0.0] - 2025-01-15

### Added
- Auto-exit follow mode when job completes (no more manual Ctrl+C needed)
- Terminal notification (OSC 9) when job finishes in follow mode
- Automatic job status refresh from Slurm when running `fleche list`
- Comprehensive documentation for all modules
- Unit tests for pure functions (46 tests covering parsing, escaping, formatting)

### Fixed
- Table column alignment with colored status text
- `human_readable()` now uses "B" consistently with KB/MB/GB

### Changed
- Disable SSH port forwarding for all commands (`-o ClearAllForwardings=yes`)
- Suppress `tail -F` stderr to hide "file doesn't exist" messages
- Extracted `parse_squeue_state` and `parse_sacct_state` for testability

## [0.1.0] - 2025-01-15 (pre-CalVer)

Initial release with core functionality:
- Submit jobs to remote Slurm clusters via SSH
- Job configuration via `fleche.toml` and `fleche/*.toml` files
- Commands: run, status, logs, sync, list, cancel, clean, init, check, guide
- Slurm resource configuration (partition, time, gpus, cpus, memory, etc.)
- Input file caching with symlinks to avoid duplication
- Output syncing back to local machine
- Job tagging for organization and filtering
- Local SQLite registry for tracking jobs
