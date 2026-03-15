# Changelog

All notable changes to this project will be documented in this file.

## [6.16.0] - 2026-03-15

### Added
- Push notifications via ntfy.sh with `--ntfy <topic>` flag
  - Sends HTTP POST to `ntfy.sh/<topic>` on job state changes
  - Notifications for all transitions: submitted, running, completed,
    failed, cancelled — with appropriate priority and tags
  - Available on `fleche run`, `fleche wait`, and `fleche rerun`
  - Job notes (from `--note`) are included in notification body
  - Fire-and-forget: notification failures are logged but never block
    the main workflow

## [6.15.0] - 2026-03-09

### Changed
- Exit codes from sacct are now stored and displayed in their raw
  `exit:signal` format (e.g. `1:0`, `0:9`) instead of just the numeric
  exit code, preserving the signal component reported by Slurm

## [6.14.3] - 2026-03-09

### Fixed
- Fixed panic ("Broken pipe") when piping output through `head`, `tail`, etc.
  by resetting the SIGPIPE handler to its default at program start

## [6.14.2] - 2026-03-09

### Fixed
- Fixed compilation on non-unix platforms by gating background job code
  (`run_background`, `shell_escape`) with `#[cfg(unix)]`

## [6.14.1] - 2026-03-06

### Changed
- GitHub Releases now include the relevant CHANGELOG section as release notes
- Release workflow builds Linux x86_64, Linux arm64, and macOS arm64 binaries
  and publishes to crates.io automatically on tag push
- `cargo binstall fleche` and `cargo install fleche` now work

## [6.14.0] - 2026-03-06

### Added
- Slurm resources at submission time shown in `fleche status <job-id>`
  - Displays partition, memory, time, GPUs, CPUs, nodes, constraint, and
    exclude — only the fields that were actually set
  - Snapshot is taken from the fully resolved config (global → job → CLI
    overrides) at submission time, so it remains accurate after Slurm purges
    the job record or after `fleche.toml` has been updated
  - Omitted for local and exec (direct SSH) jobs

## [6.13.0] - 2026-03-01

### Added
- Raw Slurm job state stored from sacct and shown in `fleche status <job-id>`
  - Captures the exact Slurm terminal state: `TIMEOUT`, `OUT_OF_MEMORY`,
    `NODE_FAIL`, `PREEMPTED`, `CANCELLED`, `COMPLETED`, etc.
  - Displayed with color: green for COMPLETED, yellow for
    CANCELLED/PREEMPTED/TIMEOUT, red for FAILED/OUT_OF_MEMORY/NODE_FAIL
  - Stored in the registry alongside exit code for historical reference
  - Enables distinguishing failure reasons that all map to `failed` status

## [6.12.0] - 2026-03-02

### Added
- Exit code tracking — numeric exit codes are now stored in the job registry
  and displayed in `fleche status <job-id>` (green for 0, red for non-zero)
  - Failure messages include exit code: "Job failed (exit code: 1)."
  - Slurm exit codes parsed from sacct `ExitCode` field (handles signal encoding)
  - Local and remote-exec backends report exit codes from their exit_code files
- `--no-sync` flag for `fleche exec` to skip project/input syncing before execution
  - Useful when code is already on the remote or for commands that don't need project files

## [6.11.0] - 2026-03-01

### Added
- Numeric index aliases for jobs — `fleche status` now shows a `#` column
  with 1-based indices (1 = most recent), usable anywhere a job ID is accepted
  - `fleche logs 1`, `fleche cancel 1`, `fleche download 2`, etc.
  - Indices correspond to the unfiltered global list; filtered views show gaps
    but indices always resolve to the same job

### Changed
- Internal refactoring: extracted `StatusOptions` struct, `ArchivedFilter` enum,
  named structs for CLI command variants, and shared `query_live_status` helper

## [6.10.0] - 2026-02-28

### Added
- `fleche proxy -- <cmd>` subcommand for routing traffic through a SOCKS proxy
  tunnel to the remote host
  - Opens SSH dynamic port forward, sets proxy environment variables
    (`ALL_PROXY`, `HTTP_PROXY`, `HTTPS_PROXY`, etc.), runs the command, and
    tears down the tunnel on exit
  - `--port` to specify a fixed port (default: random available port)
  - `--host` to override the remote host from fleche.toml
- `fleche jobs` subcommand to list available jobs from configuration
  - Reads fleche.toml and fleche/*.toml files
  - Shows each job name with its command

## [6.9.0] - 2026-02-17

### Changed
- Output following now shows both stdout and stderr interleaved
  - Applies to `fleche run` (foreground), `fleche logs --follow`, and local jobs
  - Previously only stdout was shown; stderr was silently discarded
  - `fleche logs --stdout` and `--stderr` still filter to a single stream

## [6.8.0] - 2026-02-11

### Fixed
- Shell escaping now preserves `${...}` variable references for remote expansion
  - Previously, `shell_escape()` single-quoted entire paths, preventing the remote
    shell from expanding variables like `${SSH_USER}` in `base_path`
  - Now literal segments are single-quoted while `${...}` references are left bare
  - Example: `/scratch/${USER}/fleche` becomes `'/scratch/'${USER}'/fleche'`

## [6.7.0] - 2026-02-10

### Added
- **Exec mode** for running configured jobs directly via SSH, bypassing Slurm
  - Set `exec = true` in job definition to always run directly
  - `--exec` CLI flag to override any job to run directly for a single invocation
  - Full support for foreground streaming, background (`--bg`), retry, status,
    logs, cancel, and wait — same as Slurm jobs
  - Slurm options are ignored with a warning when exec mode is active
  - Remote job status tracked via PID and exit_code files
- **Dotenv forwarding** with `dotenv = ".env"` config option
  - Injects all variables from a dotenv file into job environments as exports
  - Per-job override: `dotenv = ".env.train"` replaces global (not additive)
  - Precedence: dotenv < global `[env]` < job `[env]` < CLI `--env`
  - Configured file must exist (missing file is an error)

### Changed
- Internal refactoring: introduced `RuntimeCtx` for shared runtime settings
  across all command handlers (SSH timeouts, poll intervals, debug flag)
- Replaced SSH timeout tuple alias with typed `SshTimeouts` struct
- Snapshot active jobs before workspace cleanup to prevent deleting
  workspaces with running jobs from other clean batches

## [6.6.2] - 2026-02-01

### Changed
- Enable additional clippy nursery lints for code quality:
  - `redundant_clone`: Avoid unnecessary .clone() calls
  - `or_fun_call`: Use lazy evaluation with or_else/map_or_else
  - `redundant_pub_crate`: Simplify visibility in private modules
  - `branches_sharing_code`: Deduplicate code in if/else branches
- Use functional patterns (map_or, is_none_or, is_some_and) where appropriate

## [6.6.1] - 2026-02-01

### Changed
- Added unit tests for Slurm output parsing (squeue, sacct, disk usage, AllocTRES)
- Added tests for truncate functions
- Use `expect()` instead of `unwrap()` for invariant documentation

## [6.6.0] - 2026-02-01

### Added
- `fleche check --remote` to validate configuration against the remote server
  - Tests SSH connectivity with timing
  - Checks Slurm controller availability
  - Validates partition existence with node count
  - Verifies constraint validity for the configured partition
  - Confirms base path writability
  - Reports disk space with warnings for low space
- `fleche doctor` for comprehensive troubleshooting diagnostics
  - Checks local tools (ssh, rsync)
  - Validates configuration file
  - Reports job registry health (counts, stale jobs, old jobs needing cleanup)
  - Tests remote connection, Slurm status, and disk space
  - Provides actionable suggestions for issues found
- `fleche compare <job-a> <job-b>` to diff two job configurations
  - Shows side-by-side comparison with color-coded differences
  - Compares command, host, status, Slurm settings, environment, tags, notes
- Configurable `[settings]` section in fleche.toml
  - `default_list_limit`: Jobs shown in status (default: 20)
  - `poll_interval_local_secs`: Local job poll interval (default: 2)
  - `poll_interval_remote_secs`: Remote job poll interval (default: 5)
  - `ssh_timeout_secs`: SSH command timeout (default: 60)
  - `ssh_connect_timeout_secs`: SSH connection timeout (default: 30)
  - `retry_base_delay_secs`: Base delay for --retry backoff (default: 30)

### Changed
- All settings are now actually wired up and used throughout the codebase
- Improved error messages for I/O errors with contextual information
- Internal refactoring: extracted diagnostics module, simplified handlers

## [6.5.1] - 2026-01-20

### Fixed
- Missing `archived` column in `list_jobs` SELECT query (caused "Invalid column index: 14" error)

## [6.5.0] - 2026-01-20

### Added
- Note search in `fleche logs` with `--note <pattern>` flag
  - Filter jobs by note content using case-insensitive regex
  - Example: `fleche logs --note "learning rate"` finds jobs with matching notes
- Job archiving to hide completed jobs without deleting them
  - `fleche clean --archive <job-id>` archives a job
  - `fleche clean --archive --all` archives all finished jobs
  - `fleche clean --unarchive <job-id>` restores archived jobs
  - `fleche status --archived` shows only archived jobs
  - `fleche status --all-jobs` shows all jobs including archived
  - Archived jobs are hidden from normal listings but data is preserved

### Fixed
- Missing `note` column in `list_jobs` SELECT query (caused notes to not load properly)

## [6.4.0] - 2026-01-19

### Added
- Shell completions via clap (`fleche completions bash/zsh/fish`)
- Job dependencies with `--after` flag to run after another job completes
  - For Slurm jobs, uses `--dependency=afterok:<slurm_id>`
  - For local jobs, checks completion status before starting
- Resource statistics command (`fleche stats`) showing elapsed time, CPU time, max memory via sacct
- Automatic retries with exponential backoff (`--retry N`)
  - Delays: 30s, 60s, 120s, 240s...
  - Each retry creates a new job ID
  - Works for both Slurm and local jobs (foreground only)
- Job notes for annotations (`--note` flag and `fleche note` subcommand)
  - Add note at job creation: `fleche run train --note "testing new LR"`
  - Add/update note later: `fleche note <job-id> "note text"`
  - View note: `fleche note <job-id>` or `fleche status <job-id>`

## [6.3.0] - 2026-01-20

### Added
- Windows support for foreground local jobs (`--host local` without `--bg`)
  - Uses `cmd /c` on Windows, `sh -c` on Unix
  - Background local jobs (`--bg`) show a clear error on Windows

## [6.2.1] - 2026-01-20

### Fixed
- Use POSIX-compatible `find` for remote file listing (fixes compatibility with BSD/macOS servers)
- Check for `ssh` and `rsync` at startup with helpful install instructions for each platform

## [6.2.0] - 2026-01-19

### Added
- Local job execution with `--host local` flag
  - Run jobs on your local machine instead of a remote Slurm cluster
  - Configure per-job with `host = "local"` in job definition
  - Local jobs run directly in project directory with logs in `.fleche/jobs/{id}/`
  - Supports foreground and background (`--bg`) execution modes
  - All standard operations work: `status`, `logs`, `cancel`, `clean`, `wait`

### Changed
- Replaced unsafe `libc` calls with safe abstractions
  - Process management now uses `sysinfo` crate for cross-platform compatibility
  - Unix user ID lookup now uses `nix` crate

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
