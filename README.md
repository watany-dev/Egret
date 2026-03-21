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

Parses an ECS task definition JSON file, creates a bridge network, starts all containers, and streams their logs with `[container-name]` prefixes.

```bash
egret run -f path/to/task-definition.json
```

#### Options

| Flag | Env Var | Description |
|------|---------|-------------|
| `-f, --task-definition` | — | Path to ECS task definition JSON (required) |
| `--override` | — | Path to local override file (`egret-override.json`) |
| `-s, --secrets` | — | Path to local secrets mapping file (`secrets.local.json`) |
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
      "name": "app",
      "image": "nginx:latest",
      "essential": true,
      "command": ["nginx", "-g", "daemon off;"],
      "entryPoint": ["/docker-entrypoint.sh"],
      "environment": [
        { "name": "PORT", "value": "8080" }
      ],
      "portMappings": [
        { "containerPort": 80, "hostPort": 8080, "protocol": "tcp" }
      ],
      "secrets": [
        { "name": "DB_PASSWORD", "valueFrom": "arn:aws:secretsmanager:us-east-1:123456789:secret:prod/db-password" }
      ],
      "cpu": 256,
      "memory": 512
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

## Architecture

```
src/
├── main.rs              # Async entry point (clap + tokio)
├── cli/                 # CLI commands: run, stop, version
├── taskdef/             # ECS task definition JSON parser
├── container/           # OCI container runtime client (bollard, Docker/Podman)
├── overrides/           # Local override configuration
├── secrets/             # Secrets local resolver
├── orchestrator/        # Container lifecycle & dependsOn DAG (Phase 4)
├── metadata/            # ECS metadata endpoint mock (Phase 3)
└── credentials/         # Credential provider mock (Phase 3)
```

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` | CLI framework (derive) |
| `bollard` | OCI container runtime API client (Docker/Podman) |
| `tokio` | Async runtime |
| `serde` / `serde_json` | JSON handling |
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
make audit      # cargo deny check advisories
make deny       # cargo deny check (advisories + licenses + bans + sources)
make doc        # cargo doc with -D warnings
```

## Roadmap

- **Phase 0**: CLI skeleton + dev ecosystem ✅
- **Phase 1**: Task definition parser + container run/stop ✅
- **Phase 2**: Local overrides + secrets resolution ✅
- **Phase 3**: Metadata + credentials sidecar
- **Phase 4**: dependsOn DAG + health checks
- **Phase 5**: Volumes + log coloring + UX improvements

See [docs/ROADMAP.md](docs/ROADMAP.md) for details.

## License

Apache-2.0
