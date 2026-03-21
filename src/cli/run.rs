use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use futures_util::StreamExt;
use tokio::task::JoinHandle;

use super::RunArgs;
use crate::container::{ContainerClient, ContainerConfig, ContainerRuntime, PortMappingConfig};
use crate::overrides::OverrideConfig;
use crate::secrets::SecretsResolver;
use crate::taskdef::{ContainerDefinition, Environment, TaskDefinition};

/// Execute the `run` subcommand.
#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout)]
pub async fn execute(args: &RunArgs, host: Option<&str>) -> Result<()> {
    let mut task_def = TaskDefinition::from_file(&args.task_definition)?;
    tracing::info!(family = %task_def.family, containers = task_def.container_definitions.len(), "Parsed task definition");

    // Apply overrides if provided
    if let Some(override_path) = &args.r#override {
        let override_config = OverrideConfig::from_file(override_path)?;
        override_config.apply(&mut task_def);
        tracing::info!("Applied overrides from {}", override_path.display());
    }

    // Resolve secrets if provided
    let has_secrets = task_def
        .container_definitions
        .iter()
        .any(|c| !c.secrets.is_empty());

    if let Some(secrets_path) = &args.secrets {
        let secrets_resolver = SecretsResolver::from_file(secrets_path)?;
        for container in &mut task_def.container_definitions {
            let secret_env_vars = secrets_resolver.resolve(&container.secrets)?;
            for (name, value) in secret_env_vars {
                container.environment.push(Environment { name, value });
            }
        }
        tracing::info!("Resolved secrets from {}", secrets_path.display());
    } else if has_secrets {
        tracing::warn!(
            "Task definition has secrets but --secrets flag was not provided. Secret values will not be resolved."
        );
    }

    let client = Arc::new(ContainerClient::connect(host).await?);

    let (network_name, containers) = run_task(&*client, &task_def).await?;

    stream_logs_until_signal(&client, &containers).await;

    cleanup(&*client, &containers, &network_name).await;

    Ok(())
}

/// Create the network and start all containers for a task definition.
pub async fn run_task(
    client: &(impl ContainerRuntime + ?Sized),
    task_def: &TaskDefinition,
) -> Result<(String, Vec<(String, String)>)> {
    let network_name = client.create_network(&task_def.family).await?;
    tracing::info!(network = %network_name, "Created network");

    let mut containers = Vec::new();
    for container_def in &task_def.container_definitions {
        let config = build_container_config(&task_def.family, container_def, &network_name);
        let id = client.create_container(&config).await?;
        client.start_container(&id).await?;
        containers.push((id, container_def.name.clone()));
        tracing::info!(container = %container_def.name, "Started container");
    }

    Ok((network_name, containers))
}

/// Best-effort cleanup: stop and remove containers, then remove the network.
pub async fn cleanup(
    client: &(impl ContainerRuntime + ?Sized),
    containers: &[(String, String)],
    network: &str,
) {
    for (id, name) in containers {
        if let Err(e) = client.stop_container(id).await {
            tracing::warn!(container = %name, error = %e, "Failed to stop container");
        }
        if let Err(e) = client.remove_container(id).await {
            tracing::warn!(container = %name, error = %e, "Failed to remove container");
        }
        tracing::info!(container = %name, "Cleaned up");
    }

    if let Err(e) = client.remove_network(network).await {
        tracing::warn!(network = %network, error = %e, "Failed to remove network");
    }
    tracing::info!(network = %network, "Network removed");
}

#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout)]
async fn stream_logs_until_signal(client: &Arc<ContainerClient>, containers: &[(String, String)]) {
    let mut handles: Vec<JoinHandle<()>> = Vec::new();

    for (id, name) in containers {
        let client = Arc::clone(client);
        let id = id.clone();
        let name = name.clone();

        handles.push(tokio::spawn(async move {
            let mut stream = client.stream_logs(&id);
            while let Some(result) = stream.next().await {
                match result {
                    Ok(line) => println!("[{name}] {line}"),
                    Err(e) => {
                        tracing::warn!(container = %name, error = %e, "Log stream error");
                        break;
                    }
                }
            }
        }));
    }

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("Received SIGINT, shutting down...");

    for handle in &handles {
        handle.abort();
    }
}

fn build_container_config(
    family: &str,
    def: &ContainerDefinition,
    network: &str,
) -> ContainerConfig {
    let labels = HashMap::from([
        ("egret.managed".into(), "true".into()),
        ("egret.task".into(), family.into()),
        ("egret.container".into(), def.name.clone()),
    ]);

    let env = def
        .environment
        .iter()
        .map(|e| format!("{}={}", e.name, e.value))
        .collect();

    let port_mappings = def
        .port_mappings
        .iter()
        .map(|p| PortMappingConfig {
            host_port: p.host_port.unwrap_or(p.container_port),
            container_port: p.container_port,
            protocol: p.protocol.clone(),
        })
        .collect();

    ContainerConfig {
        name: format!("{family}-{}", def.name),
        image: def.image.clone(),
        command: def.command.clone(),
        entry_point: def.entry_point.clone(),
        env,
        port_mappings,
        network: network.into(),
        network_aliases: vec![def.name.clone()],
        labels,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::container::test_support::MockContainerClient;
    use crate::taskdef::{ContainerDefinition, Environment, PortMapping};

    fn single_container_taskdef() -> TaskDefinition {
        TaskDefinition {
            family: "web".to_string(),
            container_definitions: vec![ContainerDefinition {
                name: "app".to_string(),
                image: "nginx:latest".to_string(),
                essential: true,
                command: vec![],
                entry_point: vec![],
                environment: vec![],
                port_mappings: vec![],
                secrets: vec![],
                cpu: None,
                memory: None,
                memory_reservation: None,
            }],
        }
    }

    fn two_container_taskdef() -> TaskDefinition {
        TaskDefinition {
            family: "multi".to_string(),
            container_definitions: vec![
                ContainerDefinition {
                    name: "app".to_string(),
                    image: "nginx:latest".to_string(),
                    essential: true,
                    command: vec![],
                    entry_point: vec![],
                    environment: vec![],
                    port_mappings: vec![],
                    secrets: vec![],
                    cpu: None,
                    memory: None,
                    memory_reservation: None,
                },
                ContainerDefinition {
                    name: "sidecar".to_string(),
                    image: "redis:latest".to_string(),
                    essential: false,
                    command: vec![],
                    entry_point: vec![],
                    environment: vec![],
                    port_mappings: vec![],
                    secrets: vec![],
                    cpu: None,
                    memory: None,
                    memory_reservation: None,
                },
            ],
        }
    }

    #[tokio::test]
    async fn run_task_single_container() {
        let mock = MockContainerClient {
            create_network_results: Mutex::new(VecDeque::from([Ok("egret-web".to_string())])),
            create_container_results: Mutex::new(VecDeque::from([Ok("container-1".to_string())])),
            start_container_results: Mutex::new(VecDeque::from([Ok(())])),
            ..MockContainerClient::new()
        };

        let task_def = single_container_taskdef();
        let (network, containers) = run_task(&mock, &task_def).await.expect("should succeed");

        assert_eq!(network, "egret-web");
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].0, "container-1");
        assert_eq!(containers[0].1, "app");
    }

    #[tokio::test]
    async fn run_task_multi_container() {
        let mock = MockContainerClient {
            create_network_results: Mutex::new(VecDeque::from([Ok("egret-multi".to_string())])),
            create_container_results: Mutex::new(VecDeque::from([
                Ok("c1".to_string()),
                Ok("c2".to_string()),
            ])),
            start_container_results: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
            ..MockContainerClient::new()
        };

        let task_def = two_container_taskdef();
        let (_, containers) = run_task(&mock, &task_def).await.expect("should succeed");

        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].1, "app");
        assert_eq!(containers[1].1, "sidecar");
    }

    #[tokio::test]
    async fn run_task_network_failure() {
        let mock = MockContainerClient {
            create_network_results: Mutex::new(VecDeque::from([Err(
                crate::container::ContainerError::RuntimeNotRunning,
            )])),
            ..MockContainerClient::new()
        };

        let task_def = single_container_taskdef();
        let result = run_task(&mock, &task_def).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_task_container_create_failure() {
        let mock = MockContainerClient {
            create_network_results: Mutex::new(VecDeque::from([Ok("egret-multi".to_string())])),
            create_container_results: Mutex::new(VecDeque::from([
                Ok("c1".to_string()),
                Err(crate::container::ContainerError::RuntimeNotRunning),
            ])),
            start_container_results: Mutex::new(VecDeque::from([Ok(())])),
            ..MockContainerClient::new()
        };

        let task_def = two_container_taskdef();
        let result = run_task(&mock, &task_def).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cleanup_success() {
        let mock = MockContainerClient {
            stop_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_network_results: Mutex::new(VecDeque::from([Ok(())])),
            ..MockContainerClient::new()
        };

        let containers = vec![("c1".to_string(), "app".to_string())];
        cleanup(&mock, &containers, "egret-test").await;
        // No panic = success (best-effort cleanup)
    }

    #[tokio::test]
    async fn cleanup_tolerates_stop_failure() {
        let mock = MockContainerClient {
            stop_container_results: Mutex::new(VecDeque::from([Err(
                crate::container::ContainerError::RuntimeNotRunning,
            )])),
            remove_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_network_results: Mutex::new(VecDeque::from([Ok(())])),
            ..MockContainerClient::new()
        };

        let containers = vec![("c1".to_string(), "app".to_string())];
        cleanup(&mock, &containers, "egret-test").await;
    }

    #[tokio::test]
    async fn cleanup_tolerates_remove_failure() {
        let mock = MockContainerClient {
            stop_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_network_results: Mutex::new(VecDeque::from([Err(
                crate::container::ContainerError::RuntimeNotRunning,
            )])),
            ..MockContainerClient::new()
        };

        let containers = vec![("c1".to_string(), "app".to_string())];
        cleanup(&mock, &containers, "egret-test").await;
    }

    #[test]
    fn build_container_config_basic() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "nginx:latest".to_string(),
            essential: true,
            command: vec!["nginx".into(), "-g".into(), "daemon off;".into()],
            entry_point: vec!["/docker-entrypoint.sh".into()],
            environment: vec![Environment {
                name: "PORT".to_string(),
                value: "8080".to_string(),
            }],
            port_mappings: vec![PortMapping {
                container_port: 80,
                host_port: Some(8080),
                protocol: "tcp".to_string(),
            }],
            secrets: vec![],
            cpu: Some(256),
            memory: Some(512),
            memory_reservation: None,
        };

        let config = build_container_config("my-app", &def, "egret-my-app");

        assert_eq!(config.name, "my-app-app");
        assert_eq!(config.image, "nginx:latest");
        assert_eq!(config.command, vec!["nginx", "-g", "daemon off;"]);
        assert_eq!(config.entry_point, vec!["/docker-entrypoint.sh"]);
        assert_eq!(config.env, vec!["PORT=8080"]);
        assert_eq!(config.port_mappings.len(), 1);
        assert_eq!(config.port_mappings[0].host_port, 8080);
        assert_eq!(config.port_mappings[0].container_port, 80);
        assert_eq!(config.port_mappings[0].protocol, "tcp");
        assert_eq!(config.network, "egret-my-app");
        assert_eq!(config.network_aliases, vec!["app"]);
        assert_eq!(config.labels.get("egret.managed").unwrap(), "true");
        assert_eq!(config.labels.get("egret.task").unwrap(), "my-app");
        assert_eq!(config.labels.get("egret.container").unwrap(), "app");
    }

    #[test]
    fn build_container_config_port_default() {
        let def = ContainerDefinition {
            name: "web".to_string(),
            image: "alpine:latest".to_string(),
            essential: true,
            command: vec![],
            entry_point: vec![],
            environment: vec![],
            port_mappings: vec![PortMapping {
                container_port: 3000,
                host_port: None,
                protocol: "tcp".to_string(),
            }],
            secrets: vec![],
            cpu: None,
            memory: None,
            memory_reservation: None,
        };

        let config = build_container_config("test", &def, "egret-test");

        // host_port defaults to container_port
        assert_eq!(config.port_mappings[0].host_port, 3000);
        assert_eq!(config.port_mappings[0].container_port, 3000);
    }

    #[test]
    fn build_container_config_empty_optionals() {
        let def = ContainerDefinition {
            name: "minimal".to_string(),
            image: "alpine:latest".to_string(),
            essential: true,
            command: vec![],
            entry_point: vec![],
            environment: vec![],
            port_mappings: vec![],
            secrets: vec![],
            cpu: None,
            memory: None,
            memory_reservation: None,
        };

        let config = build_container_config("test", &def, "egret-test");

        assert!(config.command.is_empty());
        assert!(config.entry_point.is_empty());
        assert!(config.env.is_empty());
        assert!(config.port_mappings.is_empty());
    }
}
