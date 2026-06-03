---
name: fleche
description: Reference documentation for fleche CLI (remote Slurm job runner). Use when working with fleche.toml, submitting or monitoring jobs, downloading results, or troubleshooting fleche.
---

# Fleche (Remote Job Submission)

`fleche` is a utility for running jobs on remote Slurm clusters via SSH.
Configuration is in `fleche.toml`. Run `fleche skill --install` to install this reference for AI coding agents.

## Key Concepts

- **Check `fleche.toml` first** for available jobs (or run `fleche jobs`)
- Most commands default to most recent job if no job-id given
- Short ID suffix works (e.g., `x7k2` instead of full `train-20260115-153042-847-x7k2`)
- Numeric index aliases from `fleche status` work anywhere a job ID is accepted (e.g., `fleche logs 1`)
- Config supports `${VAR}` substitution from env vars, `.env` file, and `${PROJECT}` built-in
- **`--filter` vs `--tag` vs `--name`**: `--filter` is for job STATUS, `--tag` is for your custom tags, `--name` is regex on job ID
- Use `--json` flag on supported commands for machine-readable output

## Quick Start

```bash
fleche init                          # Create starter fleche.toml
fleche check                         # Validate config
fleche run <job> --dry-run           # Preview sbatch script + files to sync
fleche run <job>                     # Submit and stream output
fleche run <job> --bg                # Submit without streaming
fleche run <job> --bg --notify       # Background + terminal notification
fleche run <job> --ntfy my-topic     # Push notifications via ntfy.sh
fleche wait <job-id>                 # Wait for completion
fleche status                        # Check status
fleche logs                          # View logs (most recent job)
fleche download                      # Download results
```

## Running Jobs

```bash
fleche run <job>                              # Submit and stream output (Ctrl+C disconnects, job keeps running)
fleche run <job> --bg                         # Run in background (--notify for alerts)
fleche run <job> --env VAR=value --tag key=value  # Set env vars and tags
fleche run <job> --note "description"         # Add note to document experiment
fleche run <job> --command "nvidia-smi"       # Override command (keeps job's Slurm config)
fleche run <job> --dry-run                    # Preview sbatch script + files to sync without submitting
fleche run <job> --host local                 # Run locally instead of on remote Slurm cluster
fleche run <job> --after <job-id>             # Run after another job completes (dependency)
fleche run <job> --retry 3                    # Auto-retry on failure with exponential backoff
fleche run <job> --exec                       # Bypass Slurm, run directly via SSH for this run
fleche run <job> --ntfy my-topic              # Push notifications via ntfy.sh on state changes
fleche run "command" --gpus 1 --time 1:00:00  # Adhoc Slurm command (no job definition)
fleche rerun <job-id>                         # Re-run previous job with same settings
fleche exec <cmd>                             # Run directly via SSH, no Slurm (quick tests)
fleche exec <cmd> --no-sync                   # Skip project sync (code already on remote)
fleche exec <cmd> --host local                # Run command locally without SSH
```

## Monitoring

```bash
fleche status -n 20                     # Show last 20 jobs
  --filter running                      #   Filter by status (running/pending/completed/failed/cancelled)
  --tag key=value                       #   Filter by tag
  --name 'pattern'                      #   Filter by job ID regex (substring match, use ^/$ to anchor)
  --archived                            #   Show only archived jobs
  --all-jobs                            #   Show all jobs including archived
fleche logs [job-id]                    # View logs (--raw to strip ANSI, --follow to stream)
  -n 50                                 #   Show only last N lines
  --stdout / --stderr                   #   Show only one stream
  --note 'pattern'                      #   Filter by note content (case-insensitive regex)
fleche wait [job-id]                    # Wait for completion (--notify for alerts, --ntfy for push)
fleche stats [job-id]                   # Show resource usage (elapsed time, CPU time, max memory)
fleche note <job-id> [text]             # View or set job note
fleche ping                             # Check Slurm cluster health
fleche check                            # Validate config after editing
fleche check --remote                   # Validate config against remote server (SSH, Slurm, disk space)
fleche doctor                           # Comprehensive troubleshooting diagnostics
fleche compare <a> <b>                  # Compare two job configurations side-by-side
fleche tags                             # List unique tags across all jobs
fleche jobs                             # List available jobs from configuration
fleche proxy -- <cmd>                   # Route traffic through SSH SOCKS tunnel to remote host
```

## Results

```bash
fleche download [job-id]                # Download output files (--partial while job running)
  --filter "*.json"                     #   Download only specific file types (repeatable, recursive)
  --filter "!checkpoints/**"            #   Exclude files/directories with ! prefix
  --dry-run                             #   Preview what would be downloaded
```

## Cleanup

```bash
fleche cancel [job-id]                  # Cancel job (--all for all active, --tag to filter)
fleche cancel --dry-run                 # Preview what would be cancelled
fleche clean [job-id]                   # Archive job (default: hides without deleting)
fleche clean --all                      # Archive all finished jobs
fleche clean --all --filter failed      # Archive only failed jobs
fleche clean --older-than 2h -y         # Archive old jobs periodically
fleche clean --delete [job-id]          # Permanently delete job and remote files
fleche clean --delete --archived --all  # Delete all archived jobs
fleche clean --delete --workspace       # Also delete shared workspace (use with caution)
fleche clean --unarchive [job-id]       # Restore archived job
fleche clean --dry-run                  # Preview what would be done
```

## Configuration

fleche looks for `fleche.toml` in the current directory or parent directories.

### Minimal Example

```toml
[remote]
host = "cluster"          # SSH host from ~/.ssh/config
base_path = "~/fleche"    # Where projects are stored on remote

[jobs.train]
command = "python train.py"
```

### Full Example

```toml
[project]
name = "my-project"       # Optional, defaults to directory name

[remote]
host = "cluster"
base_path = "~/fleche"

[env]                     # Environment variables for all jobs
HF_HOME = "/scratch/cache"
PYTHONUNBUFFERED = "1"

[slurm]                   # Default Slurm settings
partition = "gpu"
time = "4:00:00"
gpus = 1

[jobs.train]
command = "python train.py"
inputs = ["data/"]        # gitignored files to copy to workspace
outputs = ["checkpoints/"]# files to download after completion

[jobs.train.slurm]        # Override Slurm settings for this job
gpus = 4
time = "24:00:00"
memory = "64G"

[jobs.train.env]          # Additional env vars for this job
CONFIG = "default"

[jobs.setup]
command = "bash setup.sh"
exec = true               # Run directly via SSH, skip Slurm

# Optional settings to tune behavior
[settings]
# default_list_limit = 20           # Jobs shown in fleche status
# poll_interval_local_secs = 2      # Status check interval for local jobs
# poll_interval_remote_secs = 5     # Status check interval for remote jobs
# ssh_timeout_secs = 60             # SSH command timeout
# ssh_connect_timeout_secs = 30     # SSH connection timeout
# retry_base_delay_secs = 30        # Base delay for --retry backoff
```

### Environment Variable Substitution

Config values support `${VAR}` substitution, resolved from (highest precedence first):
1. CLI `--env` overrides (e.g., `--env DATASET=orc`)
2. Built-in variables (`${PROJECT}` = value of `project.name`)
3. Job-specific `[jobs.<name>.env]` entries
4. Global `[env]` entries (in definition order)
5. System environment variables (e.g., `$USER`, `$HOME`)
6. Variables from `.env` file in the project directory

This means `--env` can override any variable used in commands, inputs, or outputs.

```toml
[project]
name = "graphmind"

[remote]
base_path = "/scratch/${USER}/fleche"

[env]
CACHE = "/scratch/${USER}/cache"
UV_CACHE = "${CACHE}/uv"
# Use ${PROJECT} to avoid hardcoding the project name
UV_PROJECT_ENVIRONMENT = "${CACHE}/${PROJECT}/.venv"
```

Use `${VAR:-default}` for optional variables:

```toml
[remote]
base_path = "${SCRATCH:-/tmp}/${USER}/fleche"
```

### Using .env Files

For project-specific variables, create a `.env` file:

```bash
# .env (gitignored)
SSH_USER=k21220155
SCRATCH=/scratch/users/k21220155
```

```toml
# fleche.toml
[remote]
base_path = "${SCRATCH}/fleche"
```

This enables user-agnostic configs that can be committed to version control.

### Forwarding .env Variables to Jobs

By default, `.env` variables are only used for `${VAR}` expansion in config values.
They are NOT exported into job environments. To inject all variables from a dotenv
file as exports in the sbatch script, use the `dotenv` option:

```toml
# fleche.toml
dotenv = ".env"           # All vars from .env are exported in every job
```

Per-job override (replaces global, not additive):

```toml
dotenv = ".env"

[jobs.train]
dotenv = ".env.train"     # This job uses .env.train instead of .env
```

Precedence (lowest to highest):
1. `dotenv` file variables
2. Global `[env]`
3. Job-specific `[jobs.<name>.env]`
4. CLI `--env`

The configured file must exist — unlike the implicit `.env` lookup, a missing
`dotenv` file is an error.

### Separate Job Files

Jobs can also be defined in `fleche/*.toml`. The filename becomes the job name:

```
fleche/
  train.toml
  eval.toml
  inference.toml
```

## Common Workflows

### Parameterised Jobs

Use `--env` to pass parameters or override defaults:

```toml
# fleche/train.toml
command = "python train.py --dataset ${DATASET} --config ${CONFIG}"

[env]
DATASET = "default_dataset"   # Default value
CONFIG = "base_config"        # Default value
```

```bash
# Override defaults from CLI
fleche run train --env DATASET=orc --env CONFIG=llama_orc

# The command becomes: python train.py --dataset orc --config llama_orc
```

CLI `--env` values override config defaults during `${VAR}` expansion.

### Quick GPU Test

Override command to test environment:

```bash
fleche run train --command "nvidia-smi"
```

This uses train's Slurm config (partition, gpus) but runs a different command.

### Ad-hoc Commands

Run without a job definition:

```bash
fleche run "python test.py" --partition cpu --time 0:30:00
```

### Direct SSH Execution (No Slurm)

For quick tests or non-GPU work, use exec to bypass Slurm:

```bash
fleche exec "python test.py"
fleche exec "ls -la"
```

This syncs your project and runs the command directly over SSH.
Use `--no-sync` to skip syncing (useful when code is already on the remote):

```bash
fleche exec "python test.py" --no-sync
```

### Exec Mode (Configured Direct Execution)

For jobs that should always run directly via SSH (bypassing Slurm), set `exec = true`
in the job definition. Unlike `fleche exec`, exec mode jobs are tracked in the registry
with full support for status, logs, cancel, wait, retry, and background execution.

```toml
[jobs.setup]
command = "bash setup.sh"
exec = true
```

```bash
# Run in foreground (streams output)
fleche run setup

# Run in background
fleche run setup --bg

# All standard operations work
fleche status
fleche logs
fleche cancel
fleche wait
```

Use `--exec` to override any job to run directly:

```bash
fleche run train --exec   # Bypasses Slurm for this run only
```

Slurm options are ignored for exec jobs (a warning is shown if any are set).

### Local Execution

Run jobs on your local machine instead of a remote cluster:

```bash
# Run locally via CLI flag
fleche run train --host local

# Or configure in fleche.toml
[jobs.test]
command = "python test.py"
host = "local"
```

Local jobs run directly in the project directory with logs in `.fleche/jobs/{id}/`.
Use `--host local` with `fleche exec` for quick local command execution:

```bash
fleche exec "python -c 'print(1+1)'" --host local
```

### Tagging Jobs

Add tags to track and filter experiments:

```bash
# Tag jobs when submitting
fleche run train --tag experiment=ablation --tag model=8b
fleche run train --tag experiment=baseline --tag model=8b

# Filter status by tag
fleche status --tag experiment=ablation
fleche status --tag model=8b --filter running

# Filter by job name (regex pattern, implicit .* around)
fleche status --name 123             # jobs containing "123"
fleche status --name '^train'        # jobs starting with "train"
fleche status --name 'ablation$'     # jobs ending with "ablation"

# View logs from most recent job with specific tag
fleche logs --tag experiment=ablation

# Download outputs from most recent job with tag
fleche download --tag experiment=ablation

# Cancel all jobs with a specific tag
fleche cancel --all --tag experiment=test

# Clean up old experiment jobs
fleche clean --all --tag experiment=old
fleche clean --older-than 7d --tag experiment=ablation
```

Tags are shown in status output below each job that has them.

### Monitoring

```bash
# View logs (defaults to most recent job)
fleche logs

# Show only the last 50 lines
fleche logs -n 50

# Show only stdout or only stderr
fleche logs --stdout
fleche logs --stderr

# Stream logs in real-time (Ctrl+C to disconnect; job keeps running)
fleche logs --follow

# Pull outputs while job is still running
fleche download --partial

# Download only specific file types (searches inside directories)
fleche download --filter "*.json" --filter "*.csv"

# Download everything except checkpoints
fleche download --filter "!checkpoints/**"

# Preview what would be downloaded without actually downloading
fleche download --dry-run
fleche download --dry-run --filter "*.json"
```

### Job Chaining

Jobs share a workspace, so outputs from one job are available to the next:

```bash
fleche run train          # Creates checkpoints/
fleche run eval           # Can read checkpoints/ from train
fleche download           # Download results from eval
```

No need for explicit dependencies - files persist in the shared workspace.

### Job Dependencies

Use `--after` to run a job only after another completes successfully:

```bash
# Submit training job
fleche run train --bg
# Job ID: train-20260119-120000-abc1

# Submit eval to run after train completes
fleche run eval --after abc1
```

The second job waits in the Slurm queue until the dependency finishes with exit code 0.

### Automatic Retries

Use `--retry` to automatically retry failed jobs with exponential backoff:

```bash
# Retry up to 3 times on failure (30s, 60s, 120s delays)
fleche run train --retry 3
```

Each retry creates a new job ID. Works for both Slurm and local jobs (foreground only).

### Job Notes

Annotate jobs with notes for later reference:

```bash
# Add note when submitting
fleche run train --note "testing new learning rate"

# Add or update note later
fleche note <job-id> "increased batch size to 64"

# View note
fleche note <job-id>

# Notes also shown in fleche status <job-id>

# Search logs by note content (case-insensitive regex)
fleche logs --note "learning rate"
fleche logs --note "experiment.*baseline"
```

### Archiving Jobs

By default, `fleche clean` archives jobs (hides them from listings without deleting):

```bash
# Archive a job (default behavior)
fleche clean <job-id>

# Archive all finished jobs
fleche clean --all

# Archive only failed jobs
fleche clean --all --filter failed

# View archived jobs
fleche status --archived

# View all jobs including archived
fleche status --all-jobs

# Restore an archived job
fleche clean --unarchive <job-id>

# Restore all archived jobs
fleche clean --unarchive --all

# Permanently delete jobs (removes files)
fleche clean --delete <job-id>

# Delete all archived jobs
fleche clean --delete --archived --all

# Delete archived jobs older than 30 days
fleche clean --delete --archived --older-than 30d
```

Archived jobs are hidden from `fleche status` by default but their data is preserved.

### Resource Statistics

View resource usage for completed Slurm jobs:

```bash
# Stats for most recent job
fleche stats

# Stats for last 5 jobs
fleche stats -n 5

# Stats for specific job
fleche stats <job-id>
```

Shows elapsed time, CPU time, max memory, node, and allocated resources from sacct.

Resource usage is also shown in `fleche status <job-id>` for finished Slurm jobs:

```bash
fleche status <job-id>
# ...
#   Resource usage:
#     Node:         gpu-node01
#     Elapsed:      01:23:45
#     CPU time:     02:30:00
#     Max memory:   4096K
#     Resources:    4 CPU, 1 GPU, 16G mem
```

### Push Notifications (ntfy.sh)

Get push notifications on your phone or desktop when jobs change state:

```bash
# Notify on all state changes (submitted, running, completed/failed)
fleche run train --ntfy my-topic

# Works with background jobs
fleche run train --bg --ntfy my-topic

# Wait for an existing job with notifications
fleche wait <job-id> --ntfy my-topic

# Re-run with notifications
fleche rerun <job-id> --ntfy my-topic
```

Subscribe to notifications at `https://ntfy.sh/my-topic` or install the
ntfy app on your phone. Choose a unique topic name to avoid conflicts.

Notifications are sent for each state transition:
- **Submitted** — job entered the Slurm queue (low priority)
- **Running** — job started executing (default priority)
- **Completed** — job finished successfully (high priority)
- **Failed** — job failed (urgent priority)
- **Cancelled** — job was cancelled (high priority)

Job notes (from `--note`) are included in the notification body when present.

## Commands Reference

| Command | Description |
|---------|-------------|
| `fleche run [job\|cmd] [opts]` | Submit a job via Slurm (or directly with `--exec`, locally with `--host local`) |
| `fleche run <job> --dry-run` | Preview sbatch script and list the project/input files that would be synced |
| `fleche rerun <job-id>` | Re-run a previous job with same settings |
| `fleche exec <cmd>` | Run command directly via SSH (or locally with `--host local`, `--no-sync` to skip sync) |
| `fleche status [job-id\|#N]` | Show job status (defaults to listing all) |
| `fleche status -n 50` | Show last 50 jobs |
| `fleche status --filter running` | Filter by status (repeatable) |
| `fleche status --name <pattern>` | Filter by name (regex, implicit `.*` around) |
| `fleche status --tag <k=v>` | Filter jobs by tag |
| `fleche logs [job-id]` | View job output (defaults to most recent) |
| `fleche logs --raw` | Strip ANSI codes (auto when piped) |
| `fleche logs --tag <k=v>` | Logs from most recent job with tag |
| `fleche logs --note <pattern>` | Logs from most recent job matching note |
| `fleche download [job-id]` | Pull output files (defaults to most recent) |
| `fleche download --filter <pat>` | Filter by glob, searches inside directories (`!` to exclude) |
| `fleche download --dry-run` | Preview what would be downloaded |
| `fleche download --tag <k=v>` | Download from most recent job with tag |
| `fleche cancel [job-id]` | Cancel a job (defaults to most recent active) |
| `fleche cancel --all [--tag <k=v>]` | Cancel all (or tagged) active jobs |
| `fleche cancel --dry-run` | Show what would be cancelled without cancelling |
| `fleche clean [job-id]` | Archive job (hide from listings) |
| `fleche clean --all [--tag <k=v>]` | Archive all (or tagged) finished jobs |
| `fleche clean --all --filter failed` | Archive only failed jobs |
| `fleche clean --older-than <dur>` | Archive jobs older than duration |
| `fleche clean --delete [job-id]` | Permanently delete job and remote files |
| `fleche clean --delete --archived --all` | Delete all archived jobs |
| `fleche clean --delete --workspace` | Also delete shared workspace |
| `fleche clean --dry-run` | Show what would be done without doing it |
| `fleche clean --unarchive [job-id]` | Restore archived job |
| `fleche status --archived` | Show only archived jobs |
| `fleche status --all-jobs` | Show all jobs including archived |
| `fleche tags` | List all unique tags across jobs |
| `fleche wait [job-id]` | Wait for job to complete |
| `fleche wait --notify` | Wait and send terminal notification when done |
| `fleche wait --ntfy <topic>` | Wait and send push notifications via ntfy.sh |
| `fleche stats [job-id]` | Show resource usage (time, CPU, memory, node) |
| `fleche stats -n 5` | Show stats for last N jobs |
| `fleche note <job-id> [text]` | View or set job note |
| `fleche ping` | Check Slurm cluster health |
| `fleche init` | Create starter config |
| `fleche check` | Validate config |
| `fleche check --remote` | Also validate against remote server |
| `fleche doctor` | Comprehensive troubleshooting diagnostics |
| `fleche compare <a> <b>` | Compare two job configurations side-by-side |
| `fleche proxy -- <cmd>` | Run command through SOCKS proxy to remote host |
| `fleche jobs` | List available jobs from configuration |
| `fleche skill` | Print this skill reference |
| `fleche skill --install project` | Install skill to current project |
| `fleche skill --install global` | Install skill to user config |
| `fleche completions <shell>` | Generate shell completions (bash/zsh/fish) |

## Slurm Options

These can be set in config or passed via CLI:

| Option | sbatch flag | Example |
|--------|-------------|---------|
| `--partition` | --partition | `--partition gpu` |
| `--time` | --time | `--time 8:00:00` |
| `--gpus` | --gpus | `--gpus 1` |
| `--cpus` | --cpus-per-task | `--cpus 16` |
| `--memory` | --mem | `--memory 32G` |
| `--constraint` | --constraint | `--constraint a100` |
| `--nodes` | --nodes | `--nodes 2` |
| `--exclude` | --exclude | `--exclude node01,node02` |

## Remote Directory Structure

All jobs share a workspace directory:

```
<base_path>/<project>/
  .fleche/
    workspace/          # Shared workspace (project code + inputs)
      train.py
      data/
      checkpoints/
    jobs/               # Per-job logs and metadata
      train-abc123/
        job.sbatch
        job.out
        job.err
      eval-def456/
        ...
```

- Project code is synced to `workspace/`, respecting `.gitignore`
- Files in `inputs` are copied to `workspace/` (for gitignored data)
- Job commands run with `workspace/` as their working directory
- Job logs go to `jobs/<job-id>/`
- `fleche download` copies `outputs` from `workspace/` to local
- An empty `inputs`/`outputs` entry (e.g. a `${VAR}` that expands to `""`) is
  rejected and the job is not run — set the variable to a real path or remove
  the entry

## JSON Output

Use the global `--json` flag to get machine-readable output from any supported
command. This is useful for scripting, piping to `jq`, or when fleche is driven
by an AI agent.

```bash
fleche status --json                   # List jobs as JSON
fleche status --json <job-id>          # Detailed status as JSON
fleche jobs --json                     # Available job definitions
fleche tags --json                     # All tags
fleche stats --json                    # Resource stats
fleche wait --json                     # Wait and get final status as JSON
fleche cancel --dry-run --json         # Preview cancellation as JSON
fleche clean --all --dry-run --json    # Preview cleanup as JSON
```

The `--json` flag is supported by: `status`, `jobs`, `tags`, `stats`, `wait`,
`cancel`, and `clean`.

## Dry Run

Use `--dry-run` to preview what a command would do without side effects:

```bash
fleche run train --dry-run             # Preview sbatch script + project/input files to sync
fleche download --dry-run              # Preview downloads
fleche cancel --all --dry-run          # Preview cancellation
fleche clean --older-than 7d --dry-run # Preview cleanup
fleche clean --all --dry-run           # Preview cleanup
```

## Troubleshooting

### Validate Configuration

```bash
fleche check                           # Check config syntax locally
fleche check --remote                  # Validate against remote server
```

The `--remote` flag tests:
- SSH connectivity with timing
- Slurm controller availability
- Partition existence and node count
- Constraint validity for the partition
- Base path writability
- Available disk space

### Comprehensive Diagnostics

```bash
fleche doctor
```

Runs a full diagnostic check including:
- Local tools (ssh, rsync)
- Configuration validity
- Job registry health (stale jobs, old jobs)
- Remote connection and Slurm status
- Disk space warnings

### Compare Job Configurations

```bash
fleche compare <job-a> <job-b>
```

Shows differences in command, Slurm settings, environment, tags, and more.
Useful for debugging why one job succeeded while another failed.

### Numeric Index Aliases

`fleche status` shows a `#` column with 1-based indices (1 = most recent).
Use these numbers anywhere a job ID is accepted:

```bash
fleche status
#  ID                                            STATUS       SLURM ID     CREATED
1  train-20260301-120000-abc1                    running      12345        2026-03-01 12:00
2  eval-20260228-090000-def2                     completed    12340        2026-02-28 09:00
3  train-20260227-150000-ghi3                    failed       12335        2026-02-27 15:00

# Use index instead of job ID
fleche logs 1           # Logs for most recent job
fleche cancel 1         # Cancel most recent job
fleche download 2       # Download outputs from job #2
fleche stats 3          # Stats for job #3
fleche status 1         # Detailed status for job #1
```

Indices are stable within a session — they correspond to position in the
unfiltered global list. Filtered views may show gaps (e.g., `#1, #4, #7`)
but the numbers always resolve to the same job.

### SOCKS Proxy

Route traffic through the remote host using an SSH SOCKS tunnel:

```bash
fleche proxy -- curl https://example.com              # Route through cluster
fleche proxy -- wget https://huggingface.co/weights   # Download via cluster network
fleche proxy --port 1080 -- curl https://example.com  # Use specific port
fleche proxy --host other -- curl https://example.com  # Override host
```

The tunnel opens automatically, sets `ALL_PROXY`/`HTTP_PROXY`/`HTTPS_PROXY`
environment variables on the child process, and closes when the command exits.

## Tips

- Use `--dry-run` to preview the sbatch script before submitting
- Use `fleche check --remote` to validate config against the server
- Use `fleche doctor` when things aren't working as expected
- Job IDs look like `train-20260115-153042-847-x7k2` (use suffix like `x7k2` for short)
- Use numeric indices from `fleche status` for quick access (e.g., `fleche logs 1`)
- The job registry is at `~/.config/fleche/jobs.db`
- Ctrl+C during streaming disconnects but doesn't cancel the job
- Exit codes are tracked and shown in `fleche status <job-id>` and failure messages
- Raw Slurm state (e.g., TIMEOUT, OUT_OF_MEMORY, PREEMPTED) is shown in `fleche status <job-id>` for Slurm jobs
- Slurm resources at submission (partition, memory, time, GPUs, etc.) are shown in `fleche status <job-id>` — useful after Slurm purges the job record
- Resource usage (elapsed time, CPU, memory, node) is shown in `fleche status <job-id>` for finished Slurm jobs
- Use `fleche exec` for quick ad-hoc tests without Slurm queue wait
- Use `exec = true` in config for jobs that should always bypass Slurm
- Jobs share workspace, so chained jobs can read each other's outputs
- Use `--retry` for flaky jobs that may fail due to transient issues
- Use `--note` to document experiment parameters for future reference
- Use `fleche clean` to archive old jobs without deleting them
- Use `fleche jobs` to see what jobs are available in the project
- Use `fleche proxy` to route traffic through the cluster's network
- Use `--ntfy <topic>` to get push notifications on your phone via ntfy.sh
- Enable shell completions: `fleche completions bash >> ~/.bashrc`
