//! OCI container runtime client integration.

use std::collections::HashMap;

use bollard::Docker;
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, LogsOptions, RemoveContainerOptions,
    StopContainerOptions,
};
use bollard::models::{EndpointSettings, HealthConfig, HostConfig, PortBinding};
use bollard::network::{CreateNetworkOptions, ListNetworksOptions};
use futures_util::Stream;
use futures_util::StreamExt;

/// Container runtime errors.
#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("Container runtime is not running. Please start Docker or Podman and try again.")]
    RuntimeNotRunning,

    #[error("Container runtime API error: {0}")]
    Api(#[from] bollard::errors::Error),
}

/// Abstraction over container runtime operations for testability.
pub trait ContainerRuntime: Send + Sync {
    /// Create an Egret network. Reuses if it already exists.
    async fn create_network(&self, family: &str) -> Result<String, ContainerError>;

    /// Remove a network by name.
    async fn remove_network(&self, name: &str) -> Result<(), ContainerError>;

    /// List Egret-managed networks, optionally filtered by task family.
    async fn list_networks(
        &self,
        task_filter: Option<&str>,
    ) -> Result<Vec<NetworkInfo>, ContainerError>;

    /// Create a container (does not start it). Returns the container ID.
    async fn create_container(&self, config: &ContainerConfig) -> Result<String, ContainerError>;

    /// Start a container by ID.
    async fn start_container(&self, id: &str) -> Result<(), ContainerError>;

    /// Stop a container by ID.
    async fn stop_container(&self, id: &str) -> Result<(), ContainerError>;

    /// Remove a container by ID.
    async fn remove_container(&self, id: &str) -> Result<(), ContainerError>;

    /// List Egret-managed containers, optionally filtered by task family.
    async fn list_containers(
        &self,
        task_filter: Option<&str>,
    ) -> Result<Vec<ContainerInfo>, ContainerError>;

    /// Inspect a container and return its current state.
    async fn inspect_container(&self, id: &str) -> Result<ContainerInspection, ContainerError>;

    /// Wait for a container to exit. Returns the exit status code.
    async fn wait_container(&self, id: &str) -> Result<WaitResult, ContainerError>;
}

/// Egret container runtime client wrapping bollard.
pub struct ContainerClient {
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
    /// Extra host-to-IP mappings (e.g., `host.docker.internal:host-gateway`).
    pub extra_hosts: Vec<String>,
    /// Docker HEALTHCHECK configuration.
    pub health_check: Option<HealthCheckConfig>,
}

/// Docker HEALTHCHECK configuration (nanosecond units).
pub struct HealthCheckConfig {
    /// Health check command (e.g. `["CMD-SHELL", "curl -f http://localhost/"]`).
    pub test: Vec<String>,
    /// Interval between health checks in nanoseconds.
    pub interval_ns: i64,
    /// Timeout for each check in nanoseconds.
    pub timeout_ns: i64,
    /// Number of consecutive failures before marking unhealthy.
    pub retries: i64,
    /// Grace period before health checks start in nanoseconds.
    pub start_period_ns: i64,
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

/// Result of inspecting a container.
pub struct ContainerInspection {
    #[allow(dead_code)]
    pub id: String,
    pub state: ContainerState,
}

/// Container state from inspection.
pub struct ContainerState {
    /// Status string (e.g., "running", "exited").
    #[allow(dead_code)]
    pub status: String,
    /// Whether the container is running.
    #[allow(dead_code)]
    pub running: bool,
    /// Exit code (if exited).
    #[allow(dead_code)]
    pub exit_code: Option<i64>,
    /// Health check status (e.g., "healthy", "unhealthy", "starting").
    pub health_status: Option<String>,
}

/// Result of waiting for a container to exit.
pub struct WaitResult {
    pub status_code: i64,
}

/// Host URL scheme classification.
#[derive(Debug, PartialEq, Eq)]
enum HostScheme {
    Unix,
    Tcp,
}

/// Parse a host URL into its scheme and address.
///
/// - `unix:///path` → `(Unix, "/path")`
/// - `tcp://host:port` → `(Tcp, "host:port")`
/// - `/path` (bare path) → `(Unix, "/path")`
fn parse_host_url(url: &str) -> (HostScheme, &str) {
    url.strip_prefix("unix://").map_or_else(
        || {
            url.strip_prefix("tcp://")
                .map_or((HostScheme::Unix, url), |addr| (HostScheme::Tcp, addr))
        },
        |path| (HostScheme::Unix, path),
    )
}

/// Return candidate socket paths for Podman runtime connection.
///
/// Docker default paths are handled by bollard's `connect_with_local_defaults`.
fn podman_socket_candidates() -> Vec<String> {
    let mut candidates = Vec::new();

    // Rootless Podman (XDG Base Directory Specification)
    if let Ok(xdg_dir) = std::env::var("XDG_RUNTIME_DIR") {
        candidates.push(format!("{xdg_dir}/podman/podman.sock"));
    }

    // Rootful Podman
    candidates.push("/run/podman/podman.sock".to_string());

    candidates
}

#[cfg(not(tarpaulin_include))]
impl ContainerClient {
    /// Connect to the container runtime.
    ///
    /// Priority:
    /// 1. Explicit host (`--host` flag or `CONTAINER_HOST` env)
    /// 2. `DOCKER_HOST` env / Docker default sockets (via bollard)
    /// 3. Podman socket auto-detection (rootless → rootful)
    pub async fn connect(host: Option<&str>) -> Result<Self, ContainerError> {
        // 1. Explicit host specified
        if let Some(url) = host {
            return Self::connect_to_host(url).await;
        }

        // 2. Bollard defaults (DOCKER_HOST env, Docker standard sockets)
        if let Ok(docker) = Docker::connect_with_local_defaults()
            && docker.ping().await.is_ok()
        {
            return Ok(Self { docker });
        }

        // 3. Podman socket auto-detection
        for candidate in podman_socket_candidates() {
            let path = std::path::Path::new(&candidate);
            if path.exists()
                && let Ok(docker) =
                    Docker::connect_with_unix(&candidate, 120, bollard::API_DEFAULT_VERSION)
                && docker.ping().await.is_ok()
            {
                tracing::info!(socket = %candidate, "Connected to container runtime via Podman socket");
                return Ok(Self { docker });
            }
        }

        Err(ContainerError::RuntimeNotRunning)
    }

    /// Connect to a specific host URL.
    async fn connect_to_host(url: &str) -> Result<Self, ContainerError> {
        let docker = match parse_host_url(url) {
            (HostScheme::Unix, path) => {
                Docker::connect_with_unix(path, 120, bollard::API_DEFAULT_VERSION)
                    .map_err(|_| ContainerError::RuntimeNotRunning)?
            }
            (HostScheme::Tcp, addr) => {
                tracing::warn!(
                    addr,
                    "Connecting to Docker daemon over unencrypted HTTP — \
                     credentials and container data may be exposed on the network"
                );
                let http_url = format!("http://{addr}");
                Docker::connect_with_http(&http_url, 120, bollard::API_DEFAULT_VERSION)
                    .map_err(|_| ContainerError::RuntimeNotRunning)?
            }
        };
        docker
            .ping()
            .await
            .map_err(|_| ContainerError::RuntimeNotRunning)?;
        Ok(Self { docker })
    }

    /// Stream container logs (follow mode).
    pub fn stream_logs(&self, id: &str) -> impl Stream<Item = Result<String, ContainerError>> + '_ {
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
                    .map_err(ContainerError::from)
            })
    }
}

#[cfg(not(tarpaulin_include))]
impl ContainerRuntime for ContainerClient {
    async fn create_network(&self, family: &str) -> Result<String, ContainerError> {
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

    async fn remove_network(&self, name: &str) -> Result<(), ContainerError> {
        self.docker.remove_network(name).await?;
        Ok(())
    }

    async fn list_networks(
        &self,
        task_filter: Option<&str>,
    ) -> Result<Vec<NetworkInfo>, ContainerError> {
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

    async fn create_container(&self, config: &ContainerConfig) -> Result<String, ContainerError> {
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

    async fn start_container(&self, id: &str) -> Result<(), ContainerError> {
        self.docker.start_container::<String>(id, None).await?;
        Ok(())
    }

    async fn stop_container(&self, id: &str) -> Result<(), ContainerError> {
        self.docker
            .stop_container(id, Some(StopContainerOptions { t: 10 }))
            .await?;
        Ok(())
    }

    async fn remove_container(&self, id: &str) -> Result<(), ContainerError> {
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
    ) -> Result<Vec<ContainerInfo>, ContainerError> {
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

    async fn inspect_container(&self, id: &str) -> Result<ContainerInspection, ContainerError> {
        let resp = self.docker.inspect_container(id, None).await?;
        let state = resp.state.as_ref();

        Ok(ContainerInspection {
            id: resp.id.unwrap_or_default(),
            state: ContainerState {
                status: state
                    .and_then(|s| s.status.as_ref())
                    .map(|s| format!("{s:?}").to_lowercase())
                    .unwrap_or_default(),
                running: state.and_then(|s| s.running).unwrap_or(false),
                exit_code: state.and_then(|s| s.exit_code),
                health_status: state
                    .and_then(|s| s.health.as_ref())
                    .and_then(|h| h.status.as_ref())
                    .map(|s| format!("{s:?}").to_lowercase()),
            },
        })
    }

    async fn wait_container(&self, id: &str) -> Result<WaitResult, ContainerError> {
        let response = self
            .docker
            .wait_container::<String>(id, None)
            .next()
            .await
            .ok_or(ContainerError::RuntimeNotRunning)?
            .map_err(ContainerError::from)?;
        Ok(WaitResult {
            status_code: response.status_code,
        })
    }
}

/// Build a bollard container `Config` from an Egret `ContainerConfig`.
///
/// Pure function — no container runtime interaction.
#[allow(clippy::zero_sized_map_values)] // Container API requires HashMap for exposed_ports
pub fn build_bollard_config(config: &ContainerConfig) -> Config<String> {
    let mut exposed_ports: HashMap<String, HashMap<(), ()>> = HashMap::new();
    let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();

    for pm in &config.port_mappings {
        let container_key = format!("{}/{}", pm.container_port, pm.protocol);
        exposed_ports.insert(container_key.clone(), HashMap::default());
        port_bindings.insert(
            container_key,
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
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

    let extra_hosts = if config.extra_hosts.is_empty() {
        None
    } else {
        Some(config.extra_hosts.clone())
    };

    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        extra_hosts,
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

    let healthcheck = config.health_check.as_ref().map(|hc| HealthConfig {
        test: Some(hc.test.clone()),
        interval: Some(hc.interval_ns),
        timeout: Some(hc.timeout_ns),
        retries: Some(hc.retries),
        start_period: Some(hc.start_period_ns),
        start_interval: None,
    });

    Config {
        image: Some(config.image.clone()),
        cmd,
        entrypoint,
        env,
        exposed_ports: Some(exposed_ports),
        host_config: Some(host_config),
        networking_config: Some(networking_config),
        labels: Some(config.labels.clone()),
        healthcheck,
        ..Default::default()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::struct_field_names)]
pub mod test_support {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    /// Mock container runtime client for testing.
    pub struct MockContainerClient {
        pub create_network_results: Mutex<VecDeque<Result<String, ContainerError>>>,
        pub create_container_results: Mutex<VecDeque<Result<String, ContainerError>>>,
        pub start_container_results: Mutex<VecDeque<Result<(), ContainerError>>>,
        pub stop_container_results: Mutex<VecDeque<Result<(), ContainerError>>>,
        pub remove_container_results: Mutex<VecDeque<Result<(), ContainerError>>>,
        pub remove_network_results: Mutex<VecDeque<Result<(), ContainerError>>>,
        pub list_containers_results: Mutex<VecDeque<Result<Vec<ContainerInfo>, ContainerError>>>,
        pub list_networks_results: Mutex<VecDeque<Result<Vec<NetworkInfo>, ContainerError>>>,
        pub inspect_container_results: Mutex<VecDeque<Result<ContainerInspection, ContainerError>>>,
        pub wait_container_results: Mutex<VecDeque<Result<WaitResult, ContainerError>>>,
    }

    impl MockContainerClient {
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
                inspect_container_results: Mutex::new(VecDeque::new()),
                wait_container_results: Mutex::new(VecDeque::new()),
            }
        }
    }

    /// Pop the next result from a mock queue,
    /// returning `ContainerError::RuntimeNotRunning` if the queue is empty.
    fn pop_result<T>(
        queue: &Mutex<VecDeque<Result<T, ContainerError>>>,
    ) -> Result<T, ContainerError> {
        queue
            .lock()
            .ok()
            .and_then(|mut q| q.pop_front())
            .unwrap_or(Err(ContainerError::RuntimeNotRunning))
    }

    impl ContainerRuntime for MockContainerClient {
        async fn create_network(&self, _family: &str) -> Result<String, ContainerError> {
            pop_result(&self.create_network_results)
        }

        async fn remove_network(&self, _name: &str) -> Result<(), ContainerError> {
            pop_result(&self.remove_network_results)
        }

        async fn list_networks(
            &self,
            _task_filter: Option<&str>,
        ) -> Result<Vec<NetworkInfo>, ContainerError> {
            pop_result(&self.list_networks_results)
        }

        async fn create_container(
            &self,
            _config: &ContainerConfig,
        ) -> Result<String, ContainerError> {
            pop_result(&self.create_container_results)
        }

        async fn start_container(&self, _id: &str) -> Result<(), ContainerError> {
            pop_result(&self.start_container_results)
        }

        async fn stop_container(&self, _id: &str) -> Result<(), ContainerError> {
            pop_result(&self.stop_container_results)
        }

        async fn remove_container(&self, _id: &str) -> Result<(), ContainerError> {
            pop_result(&self.remove_container_results)
        }

        async fn list_containers(
            &self,
            _task_filter: Option<&str>,
        ) -> Result<Vec<ContainerInfo>, ContainerError> {
            pop_result(&self.list_containers_results)
        }

        async fn inspect_container(
            &self,
            _id: &str,
        ) -> Result<ContainerInspection, ContainerError> {
            pop_result(&self.inspect_container_results)
        }

        async fn wait_container(&self, _id: &str) -> Result<WaitResult, ContainerError> {
            pop_result(&self.wait_container_results)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
            extra_hosts: vec![],
            health_check: None,
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
        let bindings = host_config.port_bindings.as_ref().expect("port_bindings");
        let binding = bindings
            .get("80/tcp")
            .expect("80/tcp binding")
            .as_ref()
            .expect("binding vec");
        assert_eq!(binding.len(), 1);
        assert_eq!(binding[0].host_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(binding[0].host_port.as_deref(), Some("8080"));
    }

    #[test]
    fn build_bollard_config_extra_hosts() {
        let mut config = sample_config();
        config.extra_hosts = vec!["host.docker.internal:host-gateway".to_string()];
        let result = build_bollard_config(&config);

        let host_config = result.host_config.as_ref().expect("host_config");
        let extra = host_config.extra_hosts.as_ref().expect("extra_hosts");
        assert_eq!(extra, &["host.docker.internal:host-gateway"]);
    }

    #[test]
    fn build_bollard_config_empty_extra_hosts() {
        let config = sample_config();
        let result = build_bollard_config(&config);

        let host_config = result.host_config.as_ref().expect("host_config");
        assert!(host_config.extra_hosts.is_none());
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
            extra_hosts: vec![],
            health_check: None,
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

        let net_config = result
            .networking_config
            .as_ref()
            .expect("networking_config");
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
    fn container_error_display() {
        let err = ContainerError::RuntimeNotRunning;
        assert_eq!(
            err.to_string(),
            "Container runtime is not running. Please start Docker or Podman and try again."
        );
    }

    #[test]
    fn podman_socket_candidates_includes_rootful() {
        let candidates = podman_socket_candidates();
        assert!(candidates.iter().any(|c| c == "/run/podman/podman.sock"));
    }

    #[test]
    fn podman_socket_candidates_rootful_is_last() {
        let candidates = podman_socket_candidates();
        assert_eq!(
            candidates.last().map(String::as_str),
            Some("/run/podman/podman.sock")
        );
    }

    #[test]
    fn parse_host_url_unix() {
        let (scheme, path) = parse_host_url("unix:///run/podman/podman.sock");
        assert_eq!(scheme, HostScheme::Unix);
        assert_eq!(path, "/run/podman/podman.sock");
    }

    #[test]
    fn parse_host_url_tcp() {
        let (scheme, addr) = parse_host_url("tcp://localhost:2375");
        assert_eq!(scheme, HostScheme::Tcp);
        assert_eq!(addr, "localhost:2375");
    }

    #[test]
    fn parse_host_url_bare_path() {
        let (scheme, path) = parse_host_url("/run/podman/podman.sock");
        assert_eq!(scheme, HostScheme::Unix);
        assert_eq!(path, "/run/podman/podman.sock");
    }

    #[test]
    fn build_bollard_config_with_healthcheck() {
        let mut config = sample_config();
        config.health_check = Some(HealthCheckConfig {
            test: vec!["CMD-SHELL".into(), "curl -f http://localhost/".into()],
            interval_ns: 10_000_000_000,
            timeout_ns: 5_000_000_000,
            retries: 3,
            start_period_ns: 15_000_000_000,
        });

        let result = build_bollard_config(&config);
        let hc = result.healthcheck.as_ref().expect("healthcheck");
        assert_eq!(
            hc.test.as_deref(),
            Some(
                [
                    "CMD-SHELL".to_string(),
                    "curl -f http://localhost/".to_string()
                ]
                .as_slice()
            )
        );
        assert_eq!(hc.interval, Some(10_000_000_000));
        assert_eq!(hc.timeout, Some(5_000_000_000));
        assert_eq!(hc.retries, Some(3));
        assert_eq!(hc.start_period, Some(15_000_000_000));
    }

    #[test]
    fn build_bollard_config_without_healthcheck() {
        let config = sample_config();
        let result = build_bollard_config(&config);
        assert!(result.healthcheck.is_none());
    }
}
