pub const GUIDE_TEXT: &str = r#"# rjob - Remote Job Runner

rjob submits and manages jobs on remote Slurm clusters via SSH.

## Quick Start

```bash
# Check your config is valid
rjob check

# Preview what would be submitted
rjob run <job-name> --dry-run

# Submit a job
rjob run <job-name>

# Watch output in real-time
rjob run <job-name> --follow

# Check status
rjob status

# View logs of a completed/running job
rjob logs <job-id>

# Download results
rjob sync <job-id>
```

## Configuration

rjob looks for `rjob.toml` in the current directory or parent directories.

### Minimal Example

```toml
[remote]
host = "cluster"          # SSH host from ~/.ssh/config
base_path = "~/rjob"      # Where jobs are stored on remote

[jobs.train]
command = "python train.py"
```

### Full Example

```toml
[project]
name = "my-project"       # Optional, defaults to directory name

[remote]
host = "cluster"
base_path = "~/rjob"

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

Jobs can also be defined in `rjob/*.toml`. The filename becomes the job name:

```
rjob/
  train_basic.toml
  train_advanced.toml
  evaluate.toml
```

## Common Workflows

### Parameterised Jobs

Use `--env` to pass parameters:

```bash
rjob run train --env CONFIG=llama_basic --env EPOCHS=100
```

In your command, reference as `$CONFIG` and `$EPOCHS`.

### Quick GPU Test

Override command to test environment:

```bash
rjob run train --command "nvidia-smi"
```

This uses train's Slurm config (partition, gpus) but runs a different command.

### Ad-hoc Commands

Run without a job definition:

```bash
rjob run --command "hostname" --partition cpu --time 0:05:00
```

### Tagging Jobs

Add tags to track experiments:

```bash
rjob run train --env CONFIG=llama --tag experiment=ablation --tag model=8b
rjob list --tag experiment=ablation
```

### Monitoring

```bash
# Stream logs (Ctrl+C to disconnect; job keeps running)
rjob logs <job-id> --follow

# Check stderr
rjob logs <job-id> --stderr

# Pull outputs while job is still running
rjob sync <job-id> --partial
```

## Commands Reference

| Command | Description |
|---------|-------------|
| `rjob run [job] [opts]` | Submit a job |
| `rjob status [job-id]` | Show job status |
| `rjob logs <job-id>` | View job output |
| `rjob sync <job-id>` | Pull output files |
| `rjob list` | List all jobs |
| `rjob cancel <job-id>` | Cancel a job |
| `rjob clean [job-id]` | Remove job and remote files |
| `rjob init` | Create starter config |
| `rjob check` | Validate config |

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
2. Paths in `inputs` are explicitly synced (even if gitignored)
3. After job completion, `rjob sync` pulls paths listed in `outputs`

Each job runs in its own directory: `<base_path>/<project>/<job-id>/`

## Tips

- Use `--dry-run` to preview the sbatch script before submitting
- Use `rjob check` to validate config after editing
- Job IDs look like `train-20260114-153042-847-x7k2`
- The job registry is at `~/.config/rjob/jobs.db`
- Ctrl+C during `--follow` disconnects but doesn't cancel the job
"#;
