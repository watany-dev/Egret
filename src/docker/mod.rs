//! Docker Engine API client integration.

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

/// Abstraction over Docker operations for testability.
pub trait DockerApi: Send + Sync {
    /// Create an Egret network. Reuses if it already exists.
    async fn create_network(&self, family: &str) -> Result<String, DockerError>;

    /// Remove a network by name.
    async fn remove_network(&self, name: &str) -> Result<(), DockerError>;

    /// List Egret-managed networks, optionally filtered by task family.
    async fn list_networks(
        &self,
        task_filter: Option<&str>,
    ) -> Result<Vec<NetworkInfo>, DockerError>;

    /// Create a container (does not start it). Returns the container ID.
    async fn create_container(&self, config: &ContainerConfig) -> Result<String, DockerError>;

    /// Start a container by ID.
    async fn start_container(&self, id: &str) -> Result<(), DockerError>;

    /// Stop a container by ID.
    async fn stop_container(&self, id: &str) -> Result<(), DockerError>;

    /// Remove a container by ID.
    async fn remove_container(&self, id: &str) -> Result<(), DockerError>;

    /// List Egret-managed containers, optionally filtered by task family.
    async fn list_containers(
        &self,
        task_filter: Option<&str>,
    ) -> Result<Vec<ContainerInfo>, DockerError>;
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

#[cfg(not(tarpaulin_include))]
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

#[cfg(not(tarpaulin_include))]
impl DockerApi for DockerClient {
    async fn create_network(&self, family: &str) -> Result<String, DockerError> {
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

    async fn remove_network(&self, name: &str) -> Result<(), DockerError> {
        self.docker.remove_network(name).await?;
        Ok(())
    }

    async fn list_networks(
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

    async fn create_container(&self, config: &ContainerConfig) -> Result<String, DockerError> {
        let container_config = build_bollard_config(config);

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

    async fn start_container(&self, id: &str) -> Result<(), DockerError> {
        self.docker.start_container::<String>(id, None).await?;
        Ok(())
    }

    async fn stop_container(&self, id: &str) -> Result<(), DockerError> {
        self.docker
            .stop_container(id, Some(StopContainerOptions { t: 10 }))
            .await?;
        Ok(())
    }

    async fn remove_container(&self, id: &str) -> Result<(), DockerError> {
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

    async fn list_containers(
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
}

/// Build a bollard container `Config` from an Egret `ContainerConfig`.
///
/// Pure function — no Docker daemon interaction.
#[allow(clippy::zero_sized_map_values)] // Docker API requires HashMap for exposed_ports
pub fn build_bollard_config(config: &ContainerConfig) -> Config<String> {
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

    Config {
        image: Some(config.image.clone()),
        cmd,
        entrypoint,
        env,
        exposed_ports: Some(exposed_ports),
        host_config: Some(host_config),
        networking_config: Some(networking_config),
        labels: Some(config.labels.clone()),
        ..Default::default()
    }
}

#[cfg(test)]
pub mod test_support {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    /// Mock Docker client for testing CLI orchestration logic.
    pub struct MockDockerClient {
        pub create_network_results: Mutex<VecDeque<Result<String, DockerError>>>,
        pub create_container_results: Mutex<VecDeque<Result<String, DockerError>>>,
        pub start_container_results: Mutex<VecDeque<Result<(), DockerError>>>,
        pub stop_container_results: Mutex<VecDeque<Result<(), DockerError>>>,
        pub remove_container_results: Mutex<VecDeque<Result<(), DockerError>>>,
        pub remove_network_results: Mutex<VecDeque<Result<(), DockerError>>>,
        pub list_containers_results: Mutex<VecDeque<Result<Vec<ContainerInfo>, DockerError>>>,
        pub list_networks_results: Mutex<VecDeque<Result<Vec<NetworkInfo>, DockerError>>>,
    }

    impl MockDockerClient {
        pub fn new() -> Self {
            Self {
                create_network_results: Mutex::new(VecDeque::new()),
                create_container_results: Mutex::new(VecDeque::new()),
                start_container_results: Mutex::new(VecDeque::new()),
                stop_container_results: Mutex::new(VecDeque::new()),
                remove_container_results: Mutex::new(VecDeque::new()),
                remove_network_results: Mutex::new(VecDeque::new()),
                list_containers_results: Mutex::new(VecDeque::new()),
                list_networks_results: Mutex::new(VecDeque::new()),
            }
        }
    }

    /// Pop the next result from a `Mutex<VecDeque<Result<T, DockerError>>>`,
    /// returning `DockerError::DaemonNotRunning` if the queue is empty.
    fn pop_result<T>(queue: &Mutex<VecDeque<Result<T, DockerError>>>) -> Result<T, DockerError> {
        queue
            .lock()
            .ok()
            .and_then(|mut q| q.pop_front())
            .unwrap_or(Err(DockerError::DaemonNotRunning))
    }

    impl DockerApi for MockDockerClient {
        async fn create_network(&self, _family: &str) -> Result<String, DockerError> {
            pop_result(&self.create_network_results)
        }

        async fn remove_network(&self, _name: &str) -> Result<(), DockerError> {
            pop_result(&self.remove_network_results)
        }

        async fn list_networks(
            &self,
            _task_filter: Option<&str>,
        ) -> Result<Vec<NetworkInfo>, DockerError> {
            pop_result(&self.list_networks_results)
        }

        async fn create_container(
            &self,
            _config: &ContainerConfig,
        ) -> Result<String, DockerError> {
            pop_result(&self.create_container_results)
        }

        async fn start_container(&self, _id: &str) -> Result<(), DockerError> {
            pop_result(&self.start_container_results)
        }

        async fn stop_container(&self, _id: &str) -> Result<(), DockerError> {
            pop_result(&self.stop_container_results)
        }

        async fn remove_container(&self, _id: &str) -> Result<(), DockerError> {
            pop_result(&self.remove_container_results)
        }

        async fn list_containers(
            &self,
            _task_filter: Option<&str>,
        ) -> Result<Vec<ContainerInfo>, DockerError> {
            pop_result(&self.list_containers_results)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> ContainerConfig {
        ContainerConfig {
            name: "test-app".to_string(),
            image: "nginx:latest".to_string(),
            command: vec!["nginx".into(), "-g".into(), "daemon off;".into()],
            entry_point: vec!["/entrypoint.sh".into()],
            env: vec!["PORT=8080".to_string()],
            port_mappings: vec![PortMappingConfig {
                host_port: 8080,
                container_port: 80,
                protocol: "tcp".to_string(),
            }],
            network: "egret-test".to_string(),
            network_aliases: vec!["app".to_string()],
            labels: HashMap::from([("egret.managed".into(), "true".into())]),
        }
    }

    #[test]
    fn build_bollard_config_with_ports() {
        let config = sample_config();
        let result = build_bollard_config(&config);

        // Verify exposed ports key format
        let exposed = result.exposed_ports.as_ref().expect("exposed_ports");
        assert!(exposed.contains_key("80/tcp"));

        // Verify port bindings
        let host_config = result.host_config.as_ref().expect("host_config");
        let bindings = host_config
            .port_bindings
            .as_ref()
            .expect("port_bindings");
        let binding = bindings
            .get("80/tcp")
            .expect("80/tcp binding")
            .as_ref()
            .expect("binding vec");
        assert_eq!(binding.len(), 1);
        assert_eq!(binding[0].host_ip.as_deref(), Some("0.0.0.0"));
        assert_eq!(binding[0].host_port.as_deref(), Some("8080"));
    }

    #[test]
    fn build_bollard_config_empty_cmd_and_env() {
        let config = ContainerConfig {
            name: "min".to_string(),
            image: "alpine".to_string(),
            command: vec![],
            entry_point: vec![],
            env: vec![],
            port_mappings: vec![],
            network: "net".to_string(),
            network_aliases: vec![],
            labels: HashMap::new(),
        };
        let result = build_bollard_config(&config);

        assert!(result.cmd.is_none());
        assert!(result.entrypoint.is_none());
        assert!(result.env.is_none());
    }

    #[test]
    fn build_bollard_config_with_cmd_and_entrypoint() {
        let config = sample_config();
        let result = build_bollard_config(&config);

        let cmd = result.cmd.expect("cmd should be Some");
        assert_eq!(cmd, vec!["nginx", "-g", "daemon off;"]);

        let ep = result.entrypoint.expect("entrypoint should be Some");
        assert_eq!(ep, vec!["/entrypoint.sh"]);

        let env = result.env.expect("env should be Some");
        assert_eq!(env, vec!["PORT=8080"]);
    }

    #[test]
    fn build_bollard_config_networking() {
        let config = sample_config();
        let result = build_bollard_config(&config);

        let net_config = result.networking_config.as_ref().expect("networking_config");
        let endpoint = net_config
            .endpoints_config
            .get("egret-test")
            .expect("endpoint for egret-test");
        assert_eq!(
            endpoint.aliases.as_deref(),
            Some(["app".to_string()].as_slice())
        );
    }

    #[test]
    fn docker_error_display() {
        let err = DockerError::DaemonNotRunning;
        assert_eq!(
            err.to_string(),
            "Docker daemon is not running. Please start Docker and try again."
        );
    }
}
