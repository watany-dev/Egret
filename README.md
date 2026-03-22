# Egret

Local ECS task runner — run ECS task definitions locally by satisfying the runtime contract ECS apps expect (metadata endpoints, credential providers, dependsOn, health checks), without recreating the ECS control plane.

## Quick Start

```bash
# Build
make build

# Run a task definition locally
egret run -f task-definition.json

# Run with local overrides and secrets
egret run -f task-definition.json --override egret-override.json --secrets secrets.local.json

# Run with a specific container runtime socket
egret run -f task-definition.json --host unix:///run/podman/podman.sock

# Run without metadata/credentials sidecar
egret run -f task-definition.json --no-metadata

# Dry-run: show resolved configuration without starting containers
egret run -f task-definition.json --dry-run

# Validate a task definition
egret validate -f task-definition.json

# Generate starter files for a new project
egret init --dir my-project --image nginx:latest --family my-app

# List running tasks
egret ps

# Show logs for a container
egret logs app

# Stop a specific task
egret stop <family-name>

# Stop all running tasks
egret stop --all

# Show version
egret version
```

## Installation

### From Source

```bash
git clone https://github.com/watany-dev/Egret.git
cd Egret
make build
# Binary is at target/release/egret
```

### Requirements

- Rust 1.93+ (edition 2024)
- Docker or Podman (daemon must be running)

## Usage

### `egret run`

Parses an ECS task definition JSON file, creates a bridge network, starts containers in `dependsOn` DAG order (with health check and condition waiting), and streams their logs with `[container-name]` prefixes.

```bash
egret run -f path/to/task-definition.json
```

#### Options

| Flag | Env Var | Description |
|------|---------|-------------|
| `-f, --task-definition` | — | Path to ECS task definition JSON (required) |
| `--override` | — | Path to local override file (`egret-override.json`) |
| `-s, --secrets` | — | Path to local secrets mapping file (`secrets.local.json`) |
| `--no-metadata` | — | Disable ECS metadata/credentials sidecar |
| `--dry-run` | — | Show resolved configuration without starting containers |
| `--host` | `CONTAINER_HOST` | Container runtime socket URL |

Press `Ctrl+C` to gracefully stop all containers and clean up resources.

### `egret stop`

Stops and removes Egret-managed containers and networks, identified by OCI labels.

```bash
# Stop a specific task by family name
egret stop my-app

# Stop all Egret-managed tasks
egret stop --all
```

### `egret validate`

Performs static analysis of task definition files, detecting errors before runtime.

```bash
egret validate -f task-definition.json
egret validate -f task-definition.json --override egret-override.json --secrets secrets.local.json
```

Checks include:
- Image name format validation
- Host port conflict detection
- `dependsOn` reference and cycle detection
- Secret ARN format validation
- Override container name cross-validation
- Common mistakes (all containers non-essential, no port mappings)

Diagnostics include field paths, severity levels (error/warning), and fix suggestions.

### `egret init`

Generates starter files for a new Egret project.

```bash
egret init
egret init --dir my-project --image node:20 --family web-service
```

| Flag | Default | Description |
|------|---------|-------------|
| `--dir` | `.` | Output directory |
| `--image` | `nginx:latest` | Container image for the initial definition |
| `--family` | `my-app` | Task family name |

Creates: `task-definition.json`, `egret-override.json`, `secrets.local.json`. Existing files are skipped.

### `egret ps`

Lists running Egret-managed tasks.

```bash
egret ps
egret ps my-app
```

### `egret logs`

Shows logs for a specific container.

```bash
egret logs app
egret logs app --follow
```

| Flag | Description |
|------|-------------|
| `-f, --follow` | Follow log output (like `tail -f`) |

### Container Runtime

Egret supports both Docker and Podman via the Docker-compatible API. The runtime socket is resolved in the following priority:

1. `--host` flag or `CONTAINER_HOST` env var (explicit)
2. `DOCKER_HOST` env var (Docker standard)
3. Docker default socket (`/var/run/docker.sock`)
4. Rootless Podman socket (`$XDG_RUNTIME_DIR/podman/podman.sock`)
5. Rootful Podman socket (`/run/podman/podman.sock`)

### Task Definition Format

Egret accepts standard ECS task definition JSON. Unsupported fields are silently ignored.

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

Egret respects ECS `dependsOn` declarations. Containers are started in topological order, and dependency conditions are enforced between startup layers:

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

When a container depends on another with the `HEALTHY` condition, Egret polls the container's health status until it becomes healthy or times out.

### ECS Metadata + Credentials

By default, Egret starts a local HTTP server that mocks the ECS metadata and credentials endpoints. Each container receives environment variables pointing to this server:

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
├── cli/                 # CLI commands: run, stop, ps, logs, init, validate, version
├── taskdef/             # ECS task definition JSON parser & validation diagnostics
├── container/           # OCI container runtime client (bollard, Docker/Podman)
├── overrides/           # Local override configuration
├── secrets/             # Secrets local resolver
├── orchestrator/        # Container lifecycle & dependsOn DAG
├── metadata/            # ECS metadata endpoint mock (axum HTTP server)
└── credentials/         # AWS credential provider (aws-config)
```

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` | CLI framework (derive) |
| `bollard` | OCI container runtime API client (Docker/Podman) |
| `tokio` | Async runtime |
| `serde` / `serde_json` | JSON handling |
| `axum` | HTTP server for metadata/credentials endpoints |
| `aws-config` / `aws-credential-types` | AWS credential chain loading |
| `chrono` | Date/time handling (credential expiration) |
| `tracing` | Structured logging |
| `anyhow` / `thiserror` | Error handling |
| `futures-util` | Stream processing for container logs |

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
- **Phase 7**: Observability + diagnostics
- **Phase 8**: Workflow acceleration

See [docs/ROADMAP.md](docs/ROADMAP.md) for details.

## License

Apache-2.0
