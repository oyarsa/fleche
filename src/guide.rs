//! Built-in usage guide for fleche.
//!
//! This module contains a comprehensive usage guide that is displayed when the
//! user runs `fleche guide`. The guide covers configuration, common workflows,
//! and command reference.

/// The full text of the fleche usage guide.
///
/// This is a comprehensive guide covering:
/// - Quick start examples
/// - Configuration file format
/// - Common workflows and patterns
/// - Command reference
/// - Slurm options
/// - File sync behavior
pub const GUIDE_TEXT: &str = r#"# fleche - Remote Job Runner

fleche submits and manages jobs on remote Slurm clusters via SSH.

## Quick Start

```bash
# Check your config is valid
fleche check

# Preview what would be submitted
fleche run <job-name> --dry-run

# Submit a job
fleche run <job-name>

# Watch output in real-time
fleche run <job-name> --follow

# Check status
fleche status

# View logs of a completed/running job
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
inputs = ["data/"]        # Sync these before running
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
  evaluate.toml
```

## Common Workflows

### Parameterised Jobs

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

This uses train's Slurm config (partition, gpus) but runs a different command.

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

## Commands Reference

| Command | Description |
|---------|-------------|
| `fleche run [job] [opts]` | Submit a job |
| `fleche status [job-id]` | Show job status |
| `fleche logs <job-id>` | View job output |
| `fleche sync <job-id>` | Pull output files |
| `fleche list` | List all jobs |
| `fleche cancel <job-id>` | Cancel a job |
| `fleche clean [job-id]` | Remove job and remote files |
| `fleche init` | Create starter config |
| `fleche check` | Validate config |

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

## File Sync Behavior

1. Project code is synced with rsync, respecting `.gitignore`
2. Paths in `inputs` are synced to a shared cache and symlinked into the job directory
3. After job completion, `fleche sync` pulls paths listed in `outputs`

Each job runs in its own directory: `<base_path>/<project>/.fleche/<job-id>/`

### Shared Input Cache

Input files are stored in a shared cache to avoid duplicating large datasets:

```
<base_path>/<project>/.fleche/
  cache/
    data/           # Shared input data
    models/         # Shared model files
  train-abc123/
    data -> ../cache/data      # Symlink to cache
    models -> ../cache/models
  train-def456/
    data -> ../cache/data      # Same cache, no duplication
```

All job artifacts live under `.fleche/`, so you can add it to `.gitignore` on the remote.
Inputs are synced to `cache/` and symlinked. Subsequent jobs reuse the cache.

## Tips

- Use `--dry-run` to preview the sbatch script before submitting
- Use `fleche check` to validate config after editing
- Job IDs look like `train-20260114-153042-847-x7k2`
- The job registry is at `~/.config/fleche/jobs.db`
- Ctrl+C during `--follow` disconnects but doesn't cancel the job
"#;
