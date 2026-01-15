# fleche

A CLI tool for submitting and managing jobs on remote Slurm clusters via SSH. Eliminates the need for manual SSH, rsync, and sbatch boilerplate by providing a single command interface.

## Features

- **Submit jobs** to remote Slurm clusters via SSH
- **Sync project code** respecting `.gitignore`, plus explicit input files
- **Track job status** and retrieve outputs
- **Parameterized jobs** via environment variable overrides
- **Job tagging** for organization and filtering
- **Centralized job registry** across all projects

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

# Submit a job
fleche run <job-name>

# Check status
fleche status

# View logs
fleche logs <job-id>

# Download results
fleche sync <job-id>
```

## Configuration

fleche looks for `fleche.toml` in the current directory or parent directories.

### Minimal Example

```toml
[remote]
host = "cluster"          # SSH host from ~/.ssh/config
base_path = "~/fleche"    # Where jobs are stored on remote

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
partition = "cpu"
time = "1:00:00"

[jobs.train]
command = "scripts/train.sh"
inputs = ["data/"]        # Sync these before running (even if gitignored)
outputs = ["results/"]    # Pull these after completion

[jobs.train.slurm]        # Override Slurm settings for this job
partition = "gpu"
gpus = 1
time = "8:00:00"
memory = "32G"

[jobs.train.env]          # Additional env vars for this job
CONFIG = "default"
```

### Separate Job Files

Jobs can also be defined in `fleche/*.toml`. The filename becomes the job name:

```
fleche/
  train_basic.toml
  train_advanced.toml
  experiments/ablation_v1.toml  # -> job name: experiments/ablation_v1
```

## Commands

| Command                   | Description                      |
|---------------------------|----------------------------------|
| `fleche run [job] [opts]` | Submit a job to the cluster      |
| `fleche status [job-id]`  | Show job status                  |
| `fleche logs <job-id>`    | View job output                  |
| `fleche sync <job-id>`    | Pull output files                |
| `fleche list`             | List all jobs                    |
| `fleche cancel <job-id>`  | Cancel a job                     |
| `fleche clean [job-id]`   | Remove job and remote files      |
| `fleche init`             | Create starter config            |
| `fleche check`            | Validate config                  |
| `fleche guide`            | Print comprehensive usage guide  |

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
  --follow              Tail output after submission
  --dry-run             Print sbatch script without submitting
```

### List Options

```bash
fleche list [options]

Options:
  --project <path>      Filter by project path
  --status <status>     Filter by status
  --tag <KEY=VALUE>     Filter by tag (repeatable)
  --failed              Shorthand for --status failed
  --running             Shorthand for --status running
  --completed           Shorthand for --status completed
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
fleche run --command "hostname" --partition cpu --time 0:05:00
```

### Tagging Jobs

Add tags to track experiments:

```bash
fleche run train --env CONFIG=llama --tag experiment=ablation --tag model=8b
fleche list --tag experiment=ablation
```

### Monitoring

```bash
# Stream logs (Ctrl+C to disconnect; job keeps running)
fleche logs <job-id> --follow

# Check stderr
fleche logs <job-id> --stderr

# Pull outputs while job is still running
fleche sync <job-id> --partial
```

### Cleanup

```bash
# Remove a specific job
fleche clean <job-id>

# Remove all completed/failed jobs
fleche clean --all

# Remove jobs older than 7 days
fleche clean --older-than 7d
```

## Architecture

fleche runs entirely on your local machine. All cluster interaction happens via standard Unix tools:

- **ssh** for remote command execution (sbatch, squeue, scancel, sacct)
- **rsync** for file synchronization

There is no agent or daemon on the remote server. This approach leverages your existing SSH configuration (`~/.ssh/config`), ssh-agent, ProxyJump, etc.

## File Locations

| Purpose                  | Location                                     |
|--------------------------|----------------------------------------------|
| Project config           | `fleche.toml` in repository root             |
| Job definitions          | `fleche/*.toml` in repository root           |
| Job registry             | `~/.config/fleche/jobs.db` (SQLite)          |
| Remote working directory | `<base_path>/<project>/.fleche/<job-id>/`    |
| Shared input cache       | `<base_path>/<project>/.fleche/cache/`       |

## Shared Input Cache

Input files are stored in a shared cache to avoid duplicating large datasets across jobs.
All job artifacts live under `.fleche/`, so you can add it to `.gitignore` on the remote:

```
<base_path>/<project>/.fleche/
  cache/
    data/              # Shared input data
    models/            # Shared model files
  train-abc123/
    data -> ../cache/data      # Symlink to cache
    models -> ../cache/models
  train-def456/
    data -> ../cache/data      # Same cache, no duplication
```

When you run a job:
1. Inputs are synced to `.fleche/cache/<input-path>`
2. A symlink is created in the job directory pointing to the cache
3. Subsequent jobs reuse the cache (rsync updates changed files)

This means running 10 jobs with `inputs = ["data/"]` only stores one copy of `data/` on the cluster.

## Job Lifecycle

1. **Config loaded** from `fleche.toml` and `fleche/*.toml`
2. **Job resolved** with merged settings (global -> job -> CLI)
3. **Job ID generated** with timestamp and random suffix
4. **Remote directory created**
5. **Project code synced** via rsync (respects `.gitignore`)
6. **Inputs synced to shared cache** and symlinked into job directory
7. **sbatch script generated and uploaded**
8. **Job submitted** to Slurm
9. **Job recorded** in local registry

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

- Rust 1.70+ (for building)
- SSH access to the remote cluster
- rsync installed locally and on the cluster
- Slurm scheduler on the remote cluster

## Tips

- Use `--dry-run` to preview the sbatch script before submitting
- Use `fleche check` to validate config after editing
- Job IDs look like `train-20260114-153042-847-x7k2`
- Ctrl+C during `--follow` disconnects but doesn't cancel the job
- The job registry is at `~/.config/fleche/jobs.db`

## License

GPLv3
