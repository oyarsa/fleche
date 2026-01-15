# Changelog

All notable changes to this project will be documented in this file.

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
