# fleche

A CLI tool for submitting and managing jobs on remote Slurm clusters via SSH. Eliminates the need for manual SSH, rsync, and sbatch boilerplate by providing a single command interface.

## Features

- **Submit jobs** to remote Slurm clusters via SSH
- **Sync project code** respecting `.gitignore`, plus explicit input files
- **Stream output** in real-time by default
- **Track job status** and download outputs
- **Direct SSH execution** for quick tests without Slurm
- **Job chaining** via shared workspace
- **Parameterized jobs** via environment variable overrides
- **Job tagging** for organization and filtering

## Installation

```bash
# Build from source
cargo build --release
# The binary is at target/release/fleche

# Or install globally
cargo install --path .
```

## Quick Start

```bash
# Initialize a new project
fleche init

# Edit fleche.toml to configure your remote host and jobs
# Then validate your config
fleche check

# Preview what would be submitted
fleche run <job-name> --dry-run

# Submit a job (streams output by default)
fleche run <job-name>

# Submit without streaming
fleche run <job-name> --bg

# Check status
fleche status

# View logs (defaults to most recent job)
fleche logs

# Download results
fleche download
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
```

### Separate Job Files

Jobs can also be defined in `fleche/*.toml`. The filename becomes the job name:

```
fleche/
  train.toml
  eval.toml
  experiments/ablation.toml  # -> job name: experiments/ablation
```

## Commands

| Command                      | Description                                    |
|------------------------------|------------------------------------------------|
| `fleche run [job\|cmd] [opts]` | Submit a job to the cluster                   |
| `fleche rerun <job-id>`      | Re-run a previous job with same settings       |
| `fleche exec <cmd>`          | Run command directly via SSH (no Slurm)        |
| `fleche status [job-id]`     | Show job status (defaults to listing all)      |
| `fleche logs [job-id]`       | View job output (defaults to most recent)      |
| `fleche download [job-id]`   | Pull output files (defaults to most recent)    |
| `fleche cancel [job-id]`     | Cancel a job (defaults to most recent active)  |
| `fleche clean [job-id]`      | Remove job and remote files                    |
| `fleche tags`                | List all unique tags across jobs               |
| `fleche init`                | Create starter config                          |
| `fleche check`               | Validate config                                |
| `fleche guide`               | Print comprehensive usage guide                |

All commands except `run`, `rerun`, `exec`, `tags`, `init`, `check`, and `guide` support `--tag` for filtering.

### Run Options

```bash
fleche run <job-name> [options]

Options:
  --command <cmd>       Override or provide command
  --env <KEY=VALUE>     Set environment variable (repeatable)
  --tag <KEY=VALUE>     Add tag for filtering (repeatable)
  --partition <name>    Override Slurm partition
  --time <duration>     Override wall time
  --gpus <n>            Override GPU count
  --cpus <n>            Override CPU count
  --memory <size>       Override memory
  --constraint <str>    Override constraint
  --bg                  Run in background (don't stream output)
  --dry-run             Print sbatch script without submitting
```

### Status Options

```bash
fleche status [job-id] [options]

Options:
  --filter <status>     Filter by status (repeatable: pending, running, completed, failed, cancelled)
  --tag <KEY=VALUE>     Filter by tag (repeatable)
  -n, --last <N>        Number of jobs to show (default: 20)
```

### Filtering Options

Most commands support `--tag` for filtering:

```bash
fleche logs --tag <KEY=VALUE>       # Logs from most recent job with tag
fleche download --tag <KEY=VALUE>   # Download from most recent job with tag
fleche cancel --tag <KEY=VALUE>     # Cancel most recent active job with tag
fleche cancel --all --tag <K=V>     # Cancel all active jobs with tag
fleche clean --all --tag <KEY=VALUE># Clean all finished jobs with tag
```

## Common Workflows

### Parameterized Jobs

Use `--env` to pass parameters:

```bash
fleche run train --env CONFIG=llama_basic --env EPOCHS=100
```

In your command, reference as `$CONFIG` and `$EPOCHS`.

### Quick GPU Test

Override command to test environment:

```bash
fleche run train --command "nvidia-smi"
```

Uses train's Slurm config but runs a different command.

### Ad-hoc Commands

Run without a job definition:

```bash
fleche run "python test.py" --partition cpu --time 0:30:00
```

### Direct SSH Execution

For quick tests without waiting in the Slurm queue:

```bash
fleche exec "python test.py"
fleche exec "ls -la"
```

This syncs your project and runs the command directly over SSH.

### Job Chaining

Jobs share a workspace, so outputs from one job are available to the next:

```bash
fleche run train          # Creates checkpoints/
fleche run eval           # Can read checkpoints/ from train
fleche download           # Download results from eval
```

No need for explicit dependencies - files persist in the shared workspace.

### Tagging Jobs

Add tags to track and filter experiments:

```bash
# Tag jobs when submitting
fleche run train --tag experiment=ablation --tag model=8b
fleche run train --tag experiment=baseline --tag model=8b

# Filter status by tag
fleche status --tag experiment=ablation
fleche status --tag model=8b --filter running

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

# Download a specific path
fleche download --path results/metrics.json
```

### Cleanup

```bash
# Remove a specific job
fleche clean <job-id>

# Remove all completed/failed jobs
fleche clean --all

# Remove jobs older than 7 days
fleche clean --older-than 7d

# Also delete the shared workspace
fleche clean --all --workspace
```

## Architecture

fleche runs entirely on your local machine. All cluster interaction happens via standard Unix tools:

- **ssh** for remote command execution (sbatch, squeue, scancel, sacct)
- **rsync** for file synchronization

There is no agent or daemon on the remote server. This approach leverages your existing SSH configuration (`~/.ssh/config`), ssh-agent, ProxyJump, etc.

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

## File Locations

| Purpose                  | Location                                     |
|--------------------------|----------------------------------------------|
| Project config           | `fleche.toml` in repository root             |
| Job definitions          | `fleche/*.toml` in repository root           |
| Job registry             | `~/.config/fleche/jobs.db` (SQLite)          |
| Remote workspace         | `<base_path>/<project>/.fleche/workspace/`   |
| Remote job logs          | `<base_path>/<project>/.fleche/jobs/<id>/`   |

## Job Lifecycle

1. **Config loaded** from `fleche.toml` and `fleche/*.toml`
2. **Job resolved** with merged settings (global -> job -> CLI)
3. **Job ID generated** with timestamp and random suffix
4. **Remote directories created** (workspace + job dir)
5. **Project code synced** to workspace via rsync (respects `.gitignore`)
6. **Input files synced** to workspace
7. **sbatch script generated and uploaded** to job dir
8. **Job submitted** to Slurm
9. **Job recorded** in local registry
10. **Output streamed** (unless `--bg`)

## Slurm Options

These can be set in config or passed via CLI:

| Option       | sbatch flag     | Example   |
|--------------|-----------------|-----------|
| `partition`  | --partition     | `gpu`     |
| `time`       | --time          | `8:00:00` |
| `gpus`       | --gpus          | `1`       |
| `cpus`       | --cpus-per-task | `16`      |
| `memory`     | --mem           | `32G`     |
| `constraint` | --constraint    | `a100`    |
| `nodes`      | --nodes         | `2`       |
| `exclude`    | --exclude       | `node01`  |

## Job Status Values

| Status      | Description                |
|-------------|----------------------------|
| pending     | Submitted, waiting in queue|
| running     | Currently executing        |
| completed   | Finished successfully      |
| failed      | Finished with error        |
| cancelled   | Cancelled by user          |

## Requirements

- Rust 1.85+ (for building, required by Rust 2024 edition)
- SSH access to the remote cluster
- rsync installed locally and on the cluster
- Slurm scheduler on the remote cluster

## Tips

- Use `--dry-run` to preview the sbatch script before submitting
- Use `fleche check` to validate config after editing
- Job IDs look like `train-20260115-153042-847-x7k2`
- Ctrl+C during streaming disconnects but doesn't cancel the job
- Use `fleche exec` for quick tests without Slurm queue wait
- Jobs share workspace, so chained jobs can read each other's outputs
- The job registry is at `~/.config/fleche/jobs.db`

## License

GPLv3
