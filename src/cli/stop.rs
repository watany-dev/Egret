use anyhow::Result;

use super::StopArgs;
use crate::container::{ContainerClient, ContainerRuntime};

/// Execute the `stop` subcommand.
#[cfg(not(tarpaulin_include))]
pub async fn execute(args: &StopArgs, host: Option<&str>) -> Result<()> {
    let client = ContainerClient::connect(host).await?;
    execute_with_client(args, &client).await
}

/// Stop and clean up containers and networks for a task (or all tasks).
#[allow(clippy::print_stdout)]
pub async fn execute_with_client(
    args: &StopArgs,
    client: &(impl ContainerRuntime + ?Sized),
) -> Result<()> {
    let task_filter = if args.all {
        None
    } else if let Some(task) = &args.task {
        Some(task.as_str())
    } else {
        anyhow::bail!("Specify a task name or use --all to stop all tasks.");
    };

    // Stop and remove containers (best-effort)
    let containers = client.list_containers(task_filter).await?;
    if containers.is_empty() {
        println!("No egret containers found.");
        return Ok(());
    }

    for container in &containers {
        if let Err(e) = client.stop_container(&container.id).await {
            tracing::warn!(container = %container.name, error = %e, "Failed to stop container");
        }
        if let Err(e) = client.remove_container(&container.id).await {
            tracing::warn!(container = %container.name, error = %e, "Failed to remove container");
        }
        println!("Stopped: {}", container.name);
    }

    // Remove networks
    let networks = client.list_networks(task_filter).await?;
    for network in &networks {
        if let Err(e) = client.remove_network(&network.name).await {
            tracing::warn!(network = %network.name, error = %e, "Failed to remove network");
        }
        println!("Removed network: {}", network.name);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::container::test_support::MockContainerClient;
    use crate::container::{ContainerError, ContainerInfo, NetworkInfo};

    fn container_info(id: &str, name: &str) -> ContainerInfo {
        ContainerInfo {
            id: id.to_string(),
            name: name.to_string(),
            family: "test".to_string(),
            state: "running".to_string(),
        }
    }

    fn network_info(name: &str) -> NetworkInfo {
        NetworkInfo {
            id: format!("{name}-id"),
            name: name.to_string(),
        }
    }

    #[tokio::test]
    async fn stop_specific_task() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![container_info(
                "c1", "web-app",
            )])])),
            stop_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_container_results: Mutex::new(VecDeque::from([Ok(())])),
            list_networks_results: Mutex::new(VecDeque::from([Ok(vec![network_info(
                "egret-web",
            )])])),
            remove_network_results: Mutex::new(VecDeque::from([Ok(())])),
            ..MockContainerClient::new()
        };

        let args = StopArgs {
            task: Some("web".to_string()),
            all: false,
        };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[tokio::test]
    async fn stop_all_tasks() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![
                container_info("c1", "app1"),
                container_info("c2", "app2"),
            ])])),
            stop_container_results: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
            remove_container_results: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
            list_networks_results: Mutex::new(VecDeque::from([Ok(vec![])])),
            ..MockContainerClient::new()
        };

        let args = StopArgs {
            task: None,
            all: true,
        };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[tokio::test]
    async fn stop_no_task_no_all_flag() {
        let mock = MockContainerClient::new();

        let args = StopArgs {
            task: None,
            all: false,
        };
        let err = execute_with_client(&args, &mock)
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("Specify a task name or use --all"));
    }

    #[tokio::test]
    async fn stop_no_containers_found() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![])])),
            ..MockContainerClient::new()
        };

        let args = StopArgs {
            task: Some("nonexistent".to_string()),
            all: false,
        };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[tokio::test]
    async fn stop_tolerates_stop_failure() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![container_info(
                "c1", "app",
            )])])),
            stop_container_results: Mutex::new(VecDeque::from([Err(
                ContainerError::RuntimeNotRunning,
            )])),
            remove_container_results: Mutex::new(VecDeque::from([Ok(())])),
            list_networks_results: Mutex::new(VecDeque::from([Ok(vec![])])),
            ..MockContainerClient::new()
        };

        let args = StopArgs {
            task: Some("test".to_string()),
            all: false,
        };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed despite stop failure");
    }

    #[tokio::test]
    async fn stop_tolerates_network_remove_failure() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![container_info(
                "c1", "app",
            )])])),
            stop_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_container_results: Mutex::new(VecDeque::from([Ok(())])),
            list_networks_results: Mutex::new(VecDeque::from([Ok(vec![network_info(
                "egret-test",
            )])])),
            remove_network_results: Mutex::new(VecDeque::from([Err(
                ContainerError::RuntimeNotRunning,
            )])),
            ..MockContainerClient::new()
        };

        let args = StopArgs {
            task: Some("test".to_string()),
            all: false,
        };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed despite network failure");
    }
}
