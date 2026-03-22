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

    let container = find_container(&containers, &args.container).map_err(|e| match e {
        FindContainerError::NotFound => anyhow::anyhow!(
            "No container found matching '{}'. Use 'egret ps' to list running containers.",
            args.container
        ),
        FindContainerError::Ambiguous(names) => anyhow::anyhow!(
            "Ambiguous container name '{}'. Matching containers: {}",
            args.container,
            names
        ),
    })?;

    let id = container.id.clone();

    let mut stream = client.stream_logs(&id, args.follow);
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

/// Find a container by name (exact match → unambiguous partial match).
///
/// Returns an error when multiple containers partially match the query.
fn find_container<'a>(
    containers: &'a [ContainerInfo],
    query: &str,
) -> Result<&'a ContainerInfo, FindContainerError> {
    // 1. Exact match
    if let Some(c) = containers.iter().find(|c| c.name == query) {
        return Ok(c);
    }

    // 2. Partial match — reject if ambiguous
    let matches: Vec<&ContainerInfo> = containers
        .iter()
        .filter(|c| c.name.contains(query))
        .collect();

    match matches.len() {
        0 => Err(FindContainerError::NotFound),
        1 => Ok(matches[0]),
        _ => {
            let names: Vec<&str> = matches.iter().map(|c| c.name.as_str()).collect();
            Err(FindContainerError::Ambiguous(names.join(", ")))
        }
    }
}

/// Errors returned by [`find_container`].
#[derive(Debug)]
enum FindContainerError {
    NotFound,
    Ambiguous(String),
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
            health_status: None,
            ports: vec![],
            started_at: None,
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
        assert!(matches!(result, Err(FindContainerError::NotFound)));
    }

    #[test]
    fn find_container_exact_match_preferred() {
        let containers = vec![container_info("app"), container_info("my-app-app")];
        let result = find_container(&containers, "app");
        assert_eq!(result.unwrap().name, "app");
    }

    #[test]
    fn find_container_ambiguous_partial_match() {
        let containers = vec![
            container_info("my-app-web"),
            container_info("my-app-worker"),
        ];
        let result = find_container(&containers, "my-app");
        assert!(matches!(result, Err(FindContainerError::Ambiguous(_))));
    }
}
