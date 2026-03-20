# Egret

Local ECS task runner — run ECS task definitions locally by satisfying the runtime contract ECS apps expect (metadata endpoints, credential providers, dependsOn, health checks), without recreating the ECS control plane.

## Quick Start

```bash
# Build
make build

# Run a task definition locally
egret run -f task-definition.json

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

- Rust 1.85+ (edition 2024)
- Docker (daemon must be running)

## Usage

### `egret run`

Parses an ECS task definition JSON file, creates a Docker bridge network, starts all containers, and streams their logs with `[container-name]` prefixes.

```bash
egret run -f path/to/task-definition.json
```

Press `Ctrl+C` to gracefully stop all containers and clean up resources.

### `egret stop`

Stops and removes Egret-managed containers and networks, identified by Docker labels.

```bash
# Stop a specific task by family name
egret stop my-app

# Stop all Egret-managed tasks
egret stop --all
```

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
      "cpu": 256,
      "memory": 512
    }
  ]
}
```

## Architecture

```
src/
├── main.rs              # Async entry point (clap + tokio)
├── cli/                 # CLI commands: run, stop, version
├── taskdef/             # ECS task definition JSON parser
├── docker/              # Docker Engine API client (bollard)
├── orchestrator/        # Container lifecycle & dependsOn DAG (Phase 4)
├── metadata/            # ECS metadata endpoint mock (Phase 3)
├── credentials/         # Credential provider mock (Phase 3)
└── secrets/             # Secrets local resolver (Phase 2)
```

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` | CLI framework (derive) |
| `bollard` | Docker Engine API client |
| `tokio` | Async runtime |
| `serde` / `serde_json` | JSON handling |
| `tracing` | Structured logging |
| `anyhow` / `thiserror` | Error handling |
| `futures-util` | Stream processing for Docker logs |

## Development

```bash
make build      # cargo build --release
make test       # cargo test
make lint       # cargo clippy -- -D warnings
make fmt        # cargo fmt
make fmt-check  # cargo fmt -- --check
make check      # fmt-check + lint + test (matches CI)
```

## Roadmap

- **Phase 0**: CLI skeleton + dev ecosystem ✅
- **Phase 1**: Task definition parser + container run/stop ✅
- **Phase 2**: Local overrides + secrets resolution
- **Phase 3**: Metadata + credentials sidecar
- **Phase 4**: dependsOn DAG + health checks
- **Phase 5**: Volumes + log coloring + UX improvements

See [docs/ROADMAP.md](docs/ROADMAP.md) for details.

## License

Apache-2.0
