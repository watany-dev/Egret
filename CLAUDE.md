# CLAUDE.md

## Project Overview

Egret is a local ECS task runner — run ECS task definitions locally by satisfying the runtime contract ECS apps expect (metadata endpoints, credential providers, dependsOn, health checks), without recreating the ECS control plane.

## Build & Development

```bash
make build      # cargo build --release
make test       # cargo test
make lint       # cargo clippy -- -D warnings
make fmt        # cargo fmt
make fmt-check  # cargo fmt -- --check
make check      # fmt-check + lint + test
```

## Architecture

- `src/cli/` — CLI commands (clap): run, stop, version
- `src/taskdef/` — ECS task definition JSON parser and types
- `src/docker/` — Docker Engine API client (bollard)
- `src/orchestrator/` — Container lifecycle and dependsOn DAG
- `src/metadata/` — ECS metadata endpoint mock (axum)
- `src/credentials/` — Credential provider mock
- `src/secrets/` — Secrets local resolver

## Key Dependencies

- `clap` — CLI framework
- `bollard` — Docker API client
- `tokio` — Async runtime
- `serde`/`serde_json` — JSON handling
- `axum` — HTTP server for metadata/credentials
- `tracing` — Structured logging
- `anyhow`/`thiserror` — Error handling
