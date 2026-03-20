use std::collections::HashMap;

use bollard::Docker;
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, LogsOptions, RemoveContainerOptions,
    StopContainerOptions,
};
use bollard::models::{EndpointSettings, HostConfig, PortBinding};
use bollard::network::{CreateNetworkOptions, ListNetworksOptions};
use futures_util::Stream;
use futures_util::StreamExt;

/// Docker client errors.
#[derive(Debug, thiserror::Error)]
pub enum DockerError {
    #[error("Docker daemon is not running. Please start Docker and try again.")]
    DaemonNotRunning,

    #[error("Docker API error: {0}")]
    Api(#[from] bollard::errors::Error),
}

/// Egret Docker client wrapping bollard.
pub struct DockerClient {
    docker: Docker,
}

/// Container creation configuration.
pub struct ContainerConfig {
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub entry_point: Vec<String>,
    pub env: Vec<String>,
    pub port_mappings: Vec<PortMappingConfig>,
    pub network: String,
    pub network_aliases: Vec<String>,
    pub labels: HashMap<String, String>,
}

/// Port mapping configuration.
pub struct PortMappingConfig {
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String,
}

/// Information about an Egret-managed container.
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    #[allow(dead_code)]
    pub family: String,
    #[allow(dead_code)]
    pub state: String,
}

/// Information about an Egret-managed network.
pub struct NetworkInfo {
    #[allow(dead_code)]
    pub id: String,
    pub name: String,
}

impl DockerClient {
    /// Connect to the Docker daemon and verify with a ping.
    pub async fn connect() -> Result<Self, DockerError> {
        let docker =
            Docker::connect_with_local_defaults().map_err(|_| DockerError::DaemonNotRunning)?;
        docker
            .ping()
            .await
            .map_err(|_| DockerError::DaemonNotRunning)?;
        Ok(Self { docker })
    }

    // --- Network ---

    /// Create an Egret network (`egret-<family>`). Reuses if it already exists.
    pub async fn create_network(&self, family: &str) -> Result<String, DockerError> {
        let name = format!("egret-{family}");

        let labels = HashMap::from([("egret.managed", "true"), ("egret.task", family)]);

        // Check if network already exists
        let existing = self
            .docker
            .list_networks(Some(ListNetworksOptions {
                filters: HashMap::from([("name".to_string(), vec![name.clone()])]),
            }))
            .await?;

        for net in &existing {
            if net.name.as_deref() == Some(&name) {
                return Ok(name);
            }
        }

        self.docker
            .create_network(CreateNetworkOptions {
                name: name.as_str(),
                driver: "bridge",
                labels,
                ..Default::default()
            })
            .await?;

        Ok(name)
    }

    /// Remove a network by name.
    pub async fn remove_network(&self, name: &str) -> Result<(), DockerError> {
        self.docker.remove_network(name).await?;
        Ok(())
    }

    /// List Egret-managed networks, optionally filtered by task family.
    pub async fn list_networks(
        &self,
        task_filter: Option<&str>,
    ) -> Result<Vec<NetworkInfo>, DockerError> {
        let mut label_filters = vec!["egret.managed=true".to_string()];
        if let Some(family) = task_filter {
            label_filters.push(format!("egret.task={family}"));
        }

        let networks = self
            .docker
            .list_networks(Some(ListNetworksOptions {
                filters: HashMap::from([("label".to_string(), label_filters)]),
            }))
            .await?;

        Ok(networks
            .into_iter()
            .filter_map(|n| {
                Some(NetworkInfo {
                    id: n.id?,
                    name: n.name?,
                })
            })
            .collect())
    }

    // --- Container ---

    /// Create a container (does not start it). Returns the container ID.
    #[allow(clippy::zero_sized_map_values)] // Docker API requires HashMap for exposed_ports
    pub async fn create_container(&self, config: &ContainerConfig) -> Result<String, DockerError> {
        let mut exposed_ports: HashMap<String, HashMap<(), ()>> = HashMap::new();
        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();

        for pm in &config.port_mappings {
            let container_key = format!("{}/{}", pm.container_port, pm.protocol);
            exposed_ports.insert(container_key.clone(), HashMap::default());
            port_bindings.insert(
                container_key,
                Some(vec![PortBinding {
                    host_ip: Some("0.0.0.0".to_string()),
                    host_port: Some(pm.host_port.to_string()),
                }]),
            );
        }

        let endpoint_settings = EndpointSettings {
            aliases: Some(config.network_aliases.clone()),
            ..Default::default()
        };

        let networking_config = bollard::container::NetworkingConfig {
            endpoints_config: HashMap::from([(config.network.clone(), endpoint_settings)]),
        };

        let host_config = HostConfig {
            port_bindings: Some(port_bindings),
            ..Default::default()
        };

        let cmd = if config.command.is_empty() {
            None
        } else {
            Some(config.command.clone())
        };

        let entrypoint = if config.entry_point.is_empty() {
            None
        } else {
            Some(config.entry_point.clone())
        };

        let env = if config.env.is_empty() {
            None
        } else {
            Some(config.env.clone())
        };

        let container_config = Config {
            image: Some(config.image.clone()),
            cmd,
            entrypoint,
            env,
            exposed_ports: Some(exposed_ports),
            host_config: Some(host_config),
            networking_config: Some(networking_config),
            labels: Some(config.labels.clone()),
            ..Default::default()
        };

        let response = self
            .docker
            .create_container(
                Some(CreateContainerOptions {
                    name: config.name.as_str(),
                    platform: None,
                }),
                container_config,
            )
            .await?;

        Ok(response.id)
    }

    /// Start a container by ID.
    pub async fn start_container(&self, id: &str) -> Result<(), DockerError> {
        self.docker.start_container::<String>(id, None).await?;
        Ok(())
    }

    /// Stop a container by ID (10 second timeout, then kill).
    pub async fn stop_container(&self, id: &str) -> Result<(), DockerError> {
        self.docker
            .stop_container(id, Some(StopContainerOptions { t: 10 }))
            .await?;
        Ok(())
    }

    /// Remove a container by ID.
    pub async fn remove_container(&self, id: &str) -> Result<(), DockerError> {
        self.docker
            .remove_container(
                id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await?;
        Ok(())
    }

    /// List Egret-managed containers, optionally filtered by task family.
    pub async fn list_containers(
        &self,
        task_filter: Option<&str>,
    ) -> Result<Vec<ContainerInfo>, DockerError> {
        let mut label_filters = vec!["egret.managed=true".to_string()];
        if let Some(family) = task_filter {
            label_filters.push(format!("egret.task={family}"));
        }

        let containers = self
            .docker
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters: HashMap::from([("label".to_string(), label_filters)]),
                ..Default::default()
            }))
            .await?;

        Ok(containers
            .into_iter()
            .filter_map(|c| {
                let labels = c.labels.as_ref()?;
                Some(ContainerInfo {
                    id: c.id?,
                    name: c
                        .names
                        .as_ref()
                        .and_then(|n| n.first())
                        .map(|n| n.trim_start_matches('/').to_string())
                        .unwrap_or_default(),
                    family: labels.get("egret.task").cloned().unwrap_or_default(),
                    state: c.state.unwrap_or_default(),
                })
            })
            .collect())
    }

    // --- Logs ---

    /// Stream container logs (follow mode).
    pub fn stream_logs(&self, id: &str) -> impl Stream<Item = Result<String, DockerError>> + '_ {
        self.docker
            .logs(
                id,
                Some(LogsOptions::<String> {
                    follow: true,
                    stdout: true,
                    stderr: true,
                    ..Default::default()
                }),
            )
            .map(|result| {
                result
                    .map(|output| output.to_string())
                    .map_err(DockerError::from)
            })
    }
}
