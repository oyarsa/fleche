# Changelog

All notable changes to this project will be documented in this file.

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
