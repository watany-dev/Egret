use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use futures_util::StreamExt;
use tokio::task::JoinHandle;

use super::RunArgs;
use crate::docker::{ContainerConfig, DockerApi, DockerClient, PortMappingConfig};
use crate::taskdef::{ContainerDefinition, TaskDefinition};

/// Execute the `run` subcommand.
#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout)]
pub async fn execute(args: &RunArgs) -> Result<()> {
    let task_def = TaskDefinition::from_file(&args.task_definition)?;
    tracing::info!(family = %task_def.family, containers = task_def.container_definitions.len(), "Parsed task definition");

    let client = Arc::new(DockerClient::connect().await?);

    let (network_name, containers) = run_task(&*client, &task_def).await?;

    stream_logs_until_signal(&client, &containers).await;

    cleanup(&*client, &containers, &network_name).await;

    Ok(())
}

/// Create the network and start all containers for a task definition.
pub async fn run_task(
    client: &(impl DockerApi + ?Sized),
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
    client: &(impl DockerApi + ?Sized),
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
async fn stream_logs_until_signal(client: &Arc<DockerClient>, containers: &[(String, String)]) {
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
mod tests {
    use super::*;
    use crate::taskdef::{ContainerDefinition, Environment, PortMapping};

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
