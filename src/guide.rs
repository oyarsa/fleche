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
# Initialize config in current directory
fleche init

# Check your config is valid
fleche check

# Preview what would be submitted
fleche run <job-name> --dry-run

# Submit a job (streams output by default)
fleche run <job-name>

# Submit without streaming (returns immediately)
fleche run <job-name> --bg

# Submit in background but get notified when done
fleche run <job-name> --bg --notify

# Wait for a job to complete
fleche wait <job-id>

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

# Download only specific file types
fleche download --filter "*.json" --filter "*.csv"

# Download everything except checkpoints
fleche download --filter "!checkpoints/**"
```

### Job Chaining

Jobs share a workspace, so outputs from one job are available to the next:

```bash
fleche run train          # Creates checkpoints/
fleche run eval           # Can read checkpoints/ from train
fleche download           # Download results from eval
```

No need for explicit dependencies - files persist in the shared workspace.

## Commands Reference

| Command | Description |
|---------|-------------|
| `fleche run [job\|cmd] [opts]` | Submit a job via Slurm |
| `fleche rerun <job-id>` | Re-run a previous job with same settings |
| `fleche exec <cmd>` | Run command directly via SSH (no Slurm) |
| `fleche status [job-id]` | Show job status (defaults to listing all) |
| `fleche status -n 50` | Show last 50 jobs |
| `fleche status --filter running` | Filter by status (repeatable) |
| `fleche status --name <pattern>` | Filter by name (regex, implicit `.*` around) |
| `fleche status --tag <k=v>` | Filter jobs by tag |
| `fleche logs [job-id]` | View job output (defaults to most recent) |
| `fleche logs --raw` | Strip ANSI codes (auto when piped) |
| `fleche logs --tag <k=v>` | Logs from most recent job with tag |
| `fleche download [job-id]` | Pull output files (defaults to most recent) |
| `fleche download --filter <pat>` | Filter outputs by glob (repeatable, `!` to exclude) |
| `fleche download --tag <k=v>` | Download from most recent job with tag |
| `fleche cancel [job-id]` | Cancel a job (defaults to most recent active) |
| `fleche cancel --all [--tag <k=v>]` | Cancel all (or tagged) active jobs |
| `fleche clean [job-id]` | Remove job and remote files |
| `fleche clean --all [--tag <k=v>]` | Clean all (or tagged) finished jobs |
| `fleche clean --older-than <dur>` | Clean jobs older than duration |
| `fleche clean --workspace` | Also delete shared workspace |
| `fleche tags` | List all unique tags across jobs |
| `fleche wait [job-id]` | Wait for job to complete |
| `fleche wait --notify` | Wait and send notification when done |
| `fleche ping` | Check Slurm cluster health |
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

## Tips

- Use `--dry-run` to preview the sbatch script before submitting
- Use `fleche check` to validate config after editing
- Job IDs look like `train-20260115-153042-847-x7k2` (use suffix like `x7k2` for short)
- The job registry is at `~/.config/fleche/jobs.db`
- Ctrl+C during streaming disconnects but doesn't cancel the job
- Use `fleche exec` for quick tests without Slurm queue wait
- Jobs share workspace, so chained jobs can read each other's outputs
"#;
