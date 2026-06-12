# fleche

A CLI tool for submitting and managing jobs on remote Slurm clusters via SSH. Eliminates the need for manual SSH, rsync, and sbatch boilerplate by providing a single command interface.

## Features

- **Submit jobs** to remote Slurm clusters via SSH
- **Sync project code** respecting `.gitignore`, plus explicit input files
- **Stream output** in real-time by default
- **Track job status** and download outputs
- **Direct SSH execution** for quick tests without Slurm
- **Exec mode** for configured jobs that bypass Slurm (`exec = true`)
- **Local execution** for running jobs on your machine
- **Job chaining** via shared workspace
- **Job dependencies** with `--after` for sequential workflows
- **Automatic retries** with exponential backoff
- **Parameterized jobs** via environment variable overrides
- **Job tagging** for organization and filtering
- **Job notes** for annotating experiments (with search)
- **Push notifications** via ntfy.sh on job state changes
- **Job archiving** to hide completed jobs without deletion
- **Resource statistics** via sacct integration
- **SOCKS proxy** for routing traffic through the cluster
- **Shell completions** for bash, zsh, and fish

## Installation

```bash
cargo install fleche
```

## Quick Start

```bash
# Initialize a new project
fleche init

# Try the generated local smoke job
fleche run smoke

# Edit fleche.toml to configure your remote host and jobs
# Then validate your config
fleche check

# Preview what would be submitted (sbatch script + files to sync)
fleche run <job-name> --dry-run

# Submit a job (streams output by default)
fleche run <job-name>

# Submit without streaming
fleche run <job-name> --bg

# Check status
fleche status

# Watch status live, refreshing every second
fleche watch

# View logs (defaults to most recent job)
fleche logs

# Download results
fleche download
```

## Documentation

View the built-in skill reference from the CLI, or install it locally for AI coding agents:

```bash
fleche skill                    # Print skill reference to stdout
fleche skill --install project  # Install for current project
fleche skill --install global   # Install for all projects
```

## Configuration Examples

GPU Python job:

```toml
[slurm]
partition = "gpu"
time = "4:00:00"
gpus = 1
cpus = 8
memory = "32G"

[jobs.train]
command = "python train.py --config configs/base.yaml"
inputs = ["data/"]
outputs = ["checkpoints/", "metrics.json"]
```

`uv` job with a shared cache:

```toml
[env]
UV_CACHE_DIR = "/scratch/${USER}/uv-cache"
UV_PROJECT_ENVIRONMENT = "/scratch/${USER}/${PROJECT}/.venv"

[jobs.train]
command = "uv run python train.py"
inputs = ["data/"]
outputs = ["outputs/"]
```

## Requirements

- SSH access to the remote cluster
- rsync installed locally and on the cluster
- Slurm scheduler on the remote cluster (not required for `exec = true` jobs)

## License

GPLv3
