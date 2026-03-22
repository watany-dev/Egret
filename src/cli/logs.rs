use anyhow::Result;
use futures_util::StreamExt;

use super::LogsArgs;
use crate::container::{ContainerClient, ContainerInfo, ContainerRuntime};

/// Execute the `logs` subcommand.
#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout)]
pub async fn execute(args: &LogsArgs, host: Option<&str>) -> Result<()> {
    let client = ContainerClient::connect(host).await?;
    let containers = client.list_containers(None).await?;

    let container = find_container(&containers, &args.container).ok_or_else(|| {
        anyhow::anyhow!(
            "No container found matching '{}'. Use 'egret ps' to list running containers.",
            args.container
        )
    })?;

    let id = container.id.clone();

    // stream_logs uses follow mode; for non-follow we read until the stream ends
    let mut stream = client.stream_logs(&id);
    while let Some(result) = stream.next().await {
        match result {
            Ok(line) => println!("{line}"),
            Err(e) => {
                tracing::warn!(error = %e, "Log stream error");
                break;
            }
        }
    }

    Ok(())
}

/// Find a container by name (exact match → contains fallback).
fn find_container<'a>(containers: &'a [ContainerInfo], query: &str) -> Option<&'a ContainerInfo> {
    // 1. Exact match
    if let Some(c) = containers.iter().find(|c| c.name == query) {
        return Some(c);
    }

    // 2. Contains fallback
    containers.iter().find(|c| c.name.contains(query))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn container_info(name: &str) -> ContainerInfo {
        ContainerInfo {
            id: format!("{name}-id"),
            name: name.to_string(),
            image: "alpine:latest".to_string(),
            family: "test".to_string(),
            state: "running".to_string(),
        }
    }

    #[test]
    fn find_container_exact_match() {
        let containers = vec![
            container_info("my-app-web"),
            container_info("my-app-sidecar"),
        ];
        let result = find_container(&containers, "my-app-web");
        assert_eq!(result.unwrap().name, "my-app-web");
    }

    #[test]
    fn find_container_partial_match() {
        let containers = vec![
            container_info("my-app-web"),
            container_info("my-app-sidecar"),
        ];
        let result = find_container(&containers, "web");
        assert_eq!(result.unwrap().name, "my-app-web");
    }

    #[test]
    fn find_container_not_found() {
        let containers = vec![container_info("my-app-web")];
        let result = find_container(&containers, "nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn find_container_exact_match_preferred() {
        let containers = vec![container_info("app"), container_info("my-app-app")];
        let result = find_container(&containers, "app");
        assert_eq!(result.unwrap().name, "app");
    }
}
