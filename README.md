# Lecs

Local ECS task runner — run ECS task definitions locally by satisfying the runtime contract ECS apps expect (metadata endpoints, credential providers, dependsOn, health checks), without recreating the ECS control plane.

## Quick Start

```bash
# Build
make build

# Run a task definition locally
lecs run -f task-definition.json

# Run with local overrides and secrets
lecs run -f task-definition.json --override lecs-override.json --secrets secrets.local.json

# Run with a profile (loads lecs-override.dev.json + secrets.dev.json)
lecs run -f task-definition.json --profile dev

# Run from Terraform plan/state output
lecs run --from-tf <(terraform show -json tfplan)
lecs run --from-tf plan.json --tf-resource aws_ecs_task_definition.app

# Run with a specific container runtime socket
lecs run -f task-definition.json --host unix:///run/podman/podman.sock

# Run without metadata/credentials sidecar
lecs run -f task-definition.json --no-metadata

# Dry-run: show resolved configuration without starting containers
lecs run -f task-definition.json --dry-run

# Validate a task definition
lecs validate -f task-definition.json

# Validate from Terraform plan output
lecs validate --from-tf plan.json

# Generate starter files for a new project
lecs init --dir my-project --image nginx:latest --family my-app

# List running tasks
lecs ps

# Show logs for a container
lecs logs app

# Inspect running task details
lecs inspect my-app

# Show resource usage (CPU, memory, I/O)
lecs stats

# Show execution history
lecs history

# Run with structured lifecycle events (NDJSON to stderr)
lecs run -f task-definition.json --events

# Stop a specific task
lecs stop <family-name>

# Stop all running tasks
lecs stop --all

# Compare two task definitions semantically
lecs diff task-v1.json task-v2.json

# Generate shell completion scripts
lecs completions bash > ~/.bash_completion.d/lecs
lecs completions zsh > ~/.zfunc/_lecs
lecs completions fish > ~/.config/fish/completions/lecs.fish

# Show version
lecs version
```

## Installation

### From Source

```bash
git clone https://github.com/watany-dev/Lecs.git
cd Lecs
make build
# Binary is at target/release/lecs
```

### Requirements

- Rust 1.93+ (edition 2024)
- Docker or Podman (daemon must be running)

## Usage

### `lecs run`

Parses an ECS task definition JSON file, creates a bridge network, starts containers in `dependsOn` DAG order (with health check and condition waiting), and streams their logs with `[container-name]` prefixes.

```bash
lecs run -f path/to/task-definition.json
```

#### Options

| Flag | Env Var | Description |
|------|---------|-------------|
| `-f, --task-definition` | — | Path to ECS task definition JSON (required unless `--from-tf`) |
| `--from-tf` | — | Path to `terraform show -json` output (alternative to `-f`) |
| `--tf-resource` | — | Terraform resource address (when plan has multiple ECS task definitions) |
| `-o, --override` | — | Path to local override file (`lecs-override.json`) |
| `-s, --secrets` | — | Path to local secrets mapping file (`secrets.local.json`) |
| `-p, --profile` | — | Profile name for convention-based override/secrets resolution |
| `--no-metadata` | — | Disable ECS metadata/credentials sidecar |
| `--dry-run` | — | Show resolved configuration without starting containers |
| `--events` | — | Emit structured lifecycle events (NDJSON) to stderr |
| `--host` | `CONTAINER_HOST` | Container runtime socket URL |

Press `Ctrl+C` to gracefully stop all containers and clean up resources.

### `lecs stop`

Stops and removes Lecs-managed containers and networks, identified by OCI labels.

```bash
# Stop a specific task by family name
lecs stop my-app

# Stop all Lecs-managed tasks
lecs stop --all
```

### `lecs validate`

Performs static analysis of task definition files, detecting errors before runtime.

```bash
lecs validate -f task-definition.json
lecs validate -f task-definition.json --override lecs-override.json --secrets secrets.local.json
lecs validate --from-tf plan.json
```

| Flag | Description |
|------|-------------|
| `-f, --task-definition` | Path to ECS task definition JSON (required unless `--from-tf`) |
| `--from-tf` | Path to `terraform show -json` output (alternative to `-f`) |
| `--tf-resource` | Terraform resource address (when plan has multiple ECS task definitions) |
| `-o, --override` | Path to local override file |
| `-s, --secrets` | Path to local secrets mapping file |
| `-p, --profile` | Profile name for convention-based override/secrets resolution |

Checks include:
- Image name format validation
- Host port conflict detection
- `dependsOn` reference and cycle detection
- Secret ARN format validation
- Override container name cross-validation
- Common mistakes (all containers non-essential, no port mappings)

Diagnostics include field paths, severity levels (error/warning), and fix suggestions.

### `lecs init`

Generates starter files for a new Lecs project.

```bash
lecs init
lecs init --dir my-project --image node:20 --family web-service
```

| Flag | Default | Description |
|------|---------|-------------|
| `--dir` | `.` | Output directory |
| `--image` | `nginx:latest` | Container image for the initial definition |
| `--family` | `my-app` | Task family name |

Creates: `task-definition.json`, `lecs-override.json`, `secrets.local.json`, `.lecs.toml`. Existing files are skipped.

### `lecs ps`

Lists running Lecs-managed tasks with status, health, ports, and uptime.

```bash
lecs ps
lecs ps my-app
lecs ps --output json
```

| Flag | Description |
|------|-------------|
| `--output` | Output format: `table` (default), `json` |

### `lecs inspect`

Shows detailed configuration for a running task (container IDs, images, environment variables with secrets masked).

```bash
lecs inspect my-app
```

### `lecs stats`

Shows resource usage snapshot (CPU%, memory, network I/O, block I/O) for running containers.

```bash
lecs stats
lecs stats my-app
```

### `lecs history`

Displays execution history stored in `~/.lecs/history.json`.

```bash
lecs history
lecs history --clear
```

| Flag | Description |
|------|-------------|
| `--clear` | Delete all execution history |

### `lecs logs`

Shows logs for a specific container.

```bash
lecs logs app
lecs logs app --follow
```

| Flag | Description |
|------|-------------|
| `-f, --follow` | Follow log output (like `tail -f`) |

### `lecs watch`

Watches task definition and related files for changes and automatically restarts the task.

```bash
lecs watch -f task-definition.json
lecs watch --from-tf plan.json
lecs watch -f task-definition.json --debounce 1000 --watch-path ./src
```

| Flag | Default | Description |
|------|---------|-------------|
| `-f, --task-definition` | — | Path to ECS task definition JSON (required unless `--from-tf`) |
| `--from-tf` | — | Path to `terraform show -json` output (alternative to `-f`) |
| `--tf-resource` | — | Terraform resource address |
| `-o, --override` | — | Path to local override file |
| `-s, --secrets` | — | Path to local secrets mapping file |
| `-p, --profile` | — | Profile name |
| `--debounce` | `500` | Debounce interval in milliseconds |
| `--watch-path` | — | Additional paths to watch (repeatable) |
| `--no-metadata` | — | Disable ECS metadata/credentials sidecar |
| `--events` | — | Emit structured lifecycle events (NDJSON) to stderr |

### `lecs diff`

Compares two task definition files semantically, showing differences at the container, environment variable, and port level.

```bash
lecs diff task-v1.json task-v2.json
lecs diff --no-color task-v1.json task-v2.json
```

Output shows added (`+`), removed (`-`), and changed (`~`) fields organized by container.

### `lecs completions`

Generates shell completion scripts for bash, zsh, or fish.

```bash
lecs completions bash   # Output bash completions
lecs completions zsh    # Output zsh completions
lecs completions fish   # Output fish completions
```

### Container Runtime

Lecs supports both Docker and Podman via the Docker-compatible API. The runtime socket is resolved in the following priority:

1. `--host` flag or `CONTAINER_HOST` env var (explicit)
2. `DOCKER_HOST` env var (Docker standard)
3. Docker default socket (`/var/run/docker.sock`)
4. Rootless Podman socket (`$XDG_RUNTIME_DIR/podman/podman.sock`)
5. Rootful Podman socket (`/run/podman/podman.sock`)

### Task Definition Format

Lecs accepts standard ECS task definition JSON. Unsupported fields are silently ignored.

```json
{
  "family": "my-app",
  "containerDefinitions": [
    {
      "name": "db",
      "image": "postgres:16",
      "essential": true,
      "healthCheck": {
        "command": ["CMD-SHELL", "pg_isready -U postgres"],
        "interval": 10,
        "timeout": 5,
        "retries": 5,
        "startPeriod": 30
      },
      "environment": [
        { "name": "POSTGRES_PASSWORD", "value": "dev" }
      ],
      "portMappings": [
        { "containerPort": 5432 }
      ]
    },
    {
      "name": "app",
      "image": "nginx:latest",
      "essential": true,
      "dependsOn": [
        { "containerName": "db", "condition": "HEALTHY" }
      ],
      "command": ["nginx", "-g", "daemon off;"],
      "environment": [
        { "name": "PORT", "value": "8080" }
      ],
      "portMappings": [
        { "containerPort": 80, "hostPort": 8080, "protocol": "tcp" }
      ],
      "secrets": [
        { "name": "DB_PASSWORD", "valueFrom": "arn:aws:secretsmanager:us-east-1:123456789:secret:prod/db-password" }
      ]
    }
  ]
}
```

### Terraform Plan/State Input

Lecs can read `terraform show -json` output directly, extracting `aws_ecs_task_definition` resources without requiring a separate task definition JSON file.

```bash
# Generate Terraform plan JSON
terraform plan -out=tfplan
terraform show -json tfplan > plan.json

# Run from Terraform plan
lecs run --from-tf plan.json

# Validate from Terraform plan
lecs validate --from-tf plan.json

# When plan has multiple ECS task definitions, specify the resource address
lecs run --from-tf plan.json --tf-resource aws_ecs_task_definition.app

# Run from Terraform state
terraform show -json > state.json
lecs run --from-tf state.json
```

Both plan output (`planned_values`) and state output (`values`) are supported. Child modules are searched recursively. The `container_definitions` JSON string inside Terraform output is automatically parsed (double deserialization).

### Configuration Profiles

Profiles provide convention-based override and secrets file resolution:

```bash
# Uses lecs-override.dev.json and secrets.dev.json if they exist
lecs run -f task-definition.json --profile dev

# Profile works with validate and watch too
lecs validate -f task-definition.json --profile staging
lecs watch -f task-definition.json --profile dev
```

Set a default profile in `.lecs.toml`:

```toml
default_profile = "dev"
```

Profile resolution priority:
1. Explicit `--override` / `--secrets` flags (highest)
2. Profile-resolved paths (e.g., `lecs-override.dev.json`)
3. Default profile from `.lecs.toml`

### Local Overrides

Override images, environment variables, and port mappings without editing the task definition:

```json
{
  "containerOverrides": {
    "app": {
      "image": "nginx:1.25-alpine",
      "environment": {
        "DEBUG": "true"
      },
      "portMappings": [
        { "containerPort": 80, "hostPort": 9090 }
      ]
    }
  }
}
```

### Secrets Resolution

Map Secrets Manager ARNs to local plaintext values:

```json
{
  "arn:aws:secretsmanager:us-east-1:123456789:secret:prod/db-password": "local-db-password"
}
```

### Container Dependencies (`dependsOn`)

Lecs respects ECS `dependsOn` declarations. Containers are started in topological order, and dependency conditions are enforced between startup layers:

| Condition | Behavior |
|-----------|----------|
| `START` | Dependency container has started |
| `COMPLETE` | Dependency container has exited (any exit code) |
| `SUCCESS` | Dependency container has exited with code 0 |
| `HEALTHY` | Dependency container's health check reports healthy |

Cyclic dependencies are detected and reported as errors.

### Health Checks

The `healthCheck` field is translated to a Docker `HEALTHCHECK` configuration:

```json
{
  "healthCheck": {
    "command": ["CMD-SHELL", "curl -f http://localhost/ || exit 1"],
    "interval": 10,
    "timeout": 5,
    "retries": 3,
    "startPeriod": 30
  }
}
```

When a container depends on another with the `HEALTHY` condition, Lecs polls the container's health status until it becomes healthy or times out.

### ECS Metadata + Credentials

By default, Lecs starts a local HTTP server that mocks the ECS metadata and credentials endpoints. Each container receives environment variables pointing to this server:

- `ECS_CONTAINER_METADATA_URI_V4` — Container and task metadata (ECS v4 format)
- `AWS_CONTAINER_CREDENTIALS_FULL_URI` — AWS credentials from the local credential chain

The server is accessible from containers via `host.docker.internal`. Available endpoints:

| Endpoint | Description |
|----------|-------------|
| `GET /v4/{container_name}` | Container metadata JSON |
| `GET /v4/{container_name}/task` | Task metadata JSON |
| `GET /credentials` | AWS credentials (from local credential chain) |
| `GET /health` | Health check |

Use `--no-metadata` to disable this feature entirely (no server started, no env vars injected).

The task definition's `taskRoleArn` and `executionRoleArn` fields are parsed and included in the metadata response.

## Architecture

```
src/
├── main.rs              # Async entry point (clap + tokio)
├── cli/                 # CLI commands: run, stop, ps, logs, init, validate, inspect, stats, history, diff, watch, completions, version
├── taskdef/             # ECS task definition JSON parser, validation diagnostics & Terraform input converter
├── container/           # OCI container runtime client (bollard, Docker/Podman)
├── overrides/           # Local override configuration
├── secrets/             # Secrets local resolver
├── profile/             # Profile-based file resolution (.lecs.toml)
├── orchestrator/        # Container lifecycle & dependsOn DAG
├── metadata/            # ECS metadata endpoint mock (axum HTTP server)
├── credentials/         # AWS credential provider (aws-config)
├── events/              # Structured lifecycle event logging (NDJSON)
└── history/             # Execution history persistence
```

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` / `clap_complete` | CLI framework (derive) + shell completion generation |
| `bollard` | OCI container runtime API client (Docker/Podman) |
| `tokio` | Async runtime |
| `serde` / `serde_json` | JSON handling |
| `axum` | HTTP server for metadata/credentials endpoints |
| `aws-config` / `aws-credential-types` | AWS credential chain loading |
| `chrono` | Date/time handling (credential expiration) |
| `tracing` | Structured logging |
| `anyhow` / `thiserror` | Error handling |
| `futures-util` | Stream processing for container logs |
| `toml` | Configuration file parsing (`.lecs.toml`) |
| `notify` | File system watching (`lecs watch`) |
| `getrandom` | CSPRNG for auth token generation |

## Development

```bash
make build      # cargo build --release
make test       # cargo test
make lint       # cargo clippy -- -D warnings
make fmt        # cargo fmt
make fmt-check  # cargo fmt -- --check
make check      # fmt-check + lint + test + doc + deny (matches CI)
make coverage   # cargo tarpaulin (95% minimum)
make deny       # cargo deny check (advisories + licenses + bans + sources)
make doc        # cargo doc with -D warnings
make clean      # cargo clean
```

## Roadmap

- **Phase 0**: CLI skeleton + dev ecosystem ✅
- **Phase 1**: Task definition parser + container run/stop ✅
- **Phase 2**: Local overrides + secrets resolution ✅
- **Phase 2.5**: Container runtime compatibility (Docker + Podman) ✅
- **Phase 3**: Metadata + credentials sidecar ✅
- **Phase 4**: dependsOn DAG + health checks ✅
- **Phase 5**: Volumes + log coloring + UX improvements ✅
- **Phase 6**: Validation + init + dry-run ✅
- **Phase 7**: Observability + diagnostics ✅
- **Phase 8**: Workflow acceleration ✅
- **Phase 9**: Terraform compatibility ✅

See [docs/ROADMAP.md](docs/ROADMAP.md) for details.

## License

Apache-2.0
