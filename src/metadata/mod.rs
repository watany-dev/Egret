//! ECS metadata endpoint mock server.
//!
//! Provides ECS Task Metadata Endpoint V4 compatible responses and
//! an AWS credential provider endpoint for containers running locally.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::credentials::AwsCredentials;
use crate::taskdef::{ContainerDefinition, TaskDefinition};

/// Metadata server errors.
#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    /// Failed to bind the server to a port.
    #[error("failed to bind metadata server: {0}")]
    Bind(#[source] std::io::Error),
}

/// Shared state for the metadata/credentials server.
pub struct ServerState {
    /// Task-level metadata.
    pub task_metadata: TaskMetadata,
    /// Per-container metadata, keyed by container name.
    pub container_metadata: HashMap<String, ContainerMetadata>,
    /// AWS credentials (None if unavailable).
    pub credentials: Option<AwsCredentials>,
    /// Mapping from container name to Docker container ID (populated after creation).
    pub container_ids: HashMap<String, String>,
    /// Authorization token for the credentials endpoint.
    pub auth_token: String,
}

/// Task-level metadata (ECS v4 `/task` response).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TaskMetadata {
    /// Cluster name.
    pub cluster: String,
    /// Task ARN.
    #[serde(rename = "TaskARN")]
    pub task_arn: String,
    /// Task family name.
    pub family: String,
    /// Task definition revision.
    pub revision: String,
    /// Desired task status.
    pub desired_status: String,
    /// Known (current) task status.
    pub known_status: String,
    /// All containers in the task.
    pub containers: Vec<ContainerMetadata>,
    /// Launch type (always "EC2" for local).
    pub launch_type: String,
    /// Availability zone (always "local").
    pub availability_zone: String,
    /// Task role ARN (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_role_arn: Option<String>,
}

/// Container-level metadata (ECS v4 container response).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerMetadata {
    /// Docker container ID.
    pub docker_id: String,
    /// Container name from the task definition.
    pub name: String,
    /// Docker container name (family-name format).
    pub docker_name: String,
    /// Container image.
    pub image: String,
    /// Image ID (sha256 digest).
    #[serde(rename = "ImageID")]
    pub image_id: String,
    /// Container labels.
    pub labels: HashMap<String, String>,
    /// Desired container status.
    pub desired_status: String,
    /// Known (current) container status.
    pub known_status: String,
    /// Container ARN.
    #[serde(rename = "ContainerARN")]
    pub container_arn: String,
    /// Network information.
    pub networks: Vec<NetworkMetadata>,
    /// Container type (NORMAL or ESSENTIAL).
    #[serde(rename = "Type")]
    pub container_type: String,
    /// Resource limits (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<ContainerLimits>,
    /// Creation timestamp (ISO 8601).
    pub created_at: String,
    /// Start timestamp (ISO 8601).
    pub started_at: String,
}

/// Container resource limits.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerLimits {
    /// CPU units.
    #[serde(rename = "CPU")]
    pub cpu: u32,
    /// Memory limit in MiB.
    pub memory: u32,
}

/// Network metadata within container metadata.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkMetadata {
    /// Network mode.
    pub network_mode: String,
    /// IPv4 addresses assigned.
    #[serde(rename = "IPv4Addresses")]
    pub ipv4_addresses: Vec<String>,
}

/// Build task-level metadata from a task definition.
pub fn build_task_metadata(task_def: &TaskDefinition) -> TaskMetadata {
    let task_arn = format!(
        "arn:aws:ecs:local:000000000000:task/lecs/{}",
        task_def.family
    );

    let containers: Vec<ContainerMetadata> = task_def
        .container_definitions
        .iter()
        .map(|def| build_container_metadata(&task_def.family, def))
        .collect();

    TaskMetadata {
        cluster: "lecs-local".to_string(),
        task_arn,
        family: task_def.family.clone(),
        revision: "0".to_string(),
        desired_status: "RUNNING".to_string(),
        known_status: "RUNNING".to_string(),
        containers,
        launch_type: "EC2".to_string(),
        availability_zone: "local".to_string(),
        task_role_arn: task_def.task_role_arn.clone(),
    }
}

/// Build container-level metadata from a container definition.
pub fn build_container_metadata(family: &str, def: &ContainerDefinition) -> ContainerMetadata {
    let docker_name = format!("{family}-{}", def.name);
    let container_arn = format!(
        "arn:aws:ecs:local:000000000000:container/lecs/{}/{}",
        family, def.name
    );

    let labels = HashMap::from([
        (crate::labels::MANAGED.to_string(), "true".to_string()),
        (crate::labels::TASK.to_string(), family.to_string()),
        (crate::labels::CONTAINER.to_string(), def.name.clone()),
    ]);

    let limits = match (def.cpu, def.memory) {
        (Some(cpu), Some(memory)) => Some(ContainerLimits { cpu, memory }),
        _ => None,
    };

    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    ContainerMetadata {
        docker_id: String::new(), // Updated after container creation
        name: def.name.clone(),
        docker_name,
        image: def.image.clone(),
        image_id: String::new(), // Not available before pull
        labels,
        desired_status: "RUNNING".to_string(),
        known_status: "RUNNING".to_string(),
        container_arn,
        networks: vec![NetworkMetadata {
            network_mode: "bridge".to_string(),
            ipv4_addresses: vec![],
        }],
        container_type: "NORMAL".to_string(),
        limits,
        created_at: now.clone(),
        started_at: now,
    }
}

/// Type alias for the shared server state.
pub type SharedState = Arc<RwLock<ServerState>>;

/// Metadata server handle for lifecycle management.
pub struct MetadataServer {
    /// The port the server is listening on.
    pub port: u16,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl MetadataServer {
    /// Start the metadata server on a random available port.
    ///
    /// Returns a handle that can be used to shut down the server.
    pub async fn start(state: SharedState) -> Result<Self, MetadataError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(MetadataError::Bind)?;

        let port = listener.local_addr().map_err(MetadataError::Bind)?.port();

        let app = build_router(state);

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    shutdown_rx.await.ok();
                })
                .await
                .ok();
        });

        tracing::info!(port, "Metadata server started");
        Ok(Self {
            port,
            shutdown_tx,
            handle,
        })
    }

    /// Gracefully shut down the server.
    pub async fn shutdown(self) {
        self.shutdown_tx.send(()).ok();
        self.handle.await.ok();
        tracing::info!("Metadata server stopped");
    }
}

/// Update a container's Docker ID after creation.
pub async fn update_container_id(state: &SharedState, name: &str, docker_id: &str) {
    let mut state = state.write().await;
    state
        .container_ids
        .insert(name.to_string(), docker_id.to_string());
    if let Some(meta) = state.container_metadata.get_mut(name) {
        meta.docker_id = docker_id.to_string();
    }
    // Also update in task_metadata.containers
    for container in &mut state.task_metadata.containers {
        if container.name == name {
            container.docker_id = docker_id.to_string();
        }
    }
}

/// Generate an authorization token for the credentials endpoint.
///
/// Uses the OS-provided CSPRNG via `getrandom` to produce a 128-bit
/// cryptographically random hex token (32 hex characters).
pub fn generate_auth_token() -> String {
    use std::fmt::Write;

    let mut buf = [0u8; 16];
    // OS CSPRNG failure is unrecoverable — no fallback possible.
    #[allow(clippy::expect_used)]
    getrandom::fill(&mut buf).expect("OS CSPRNG unavailable");
    buf.iter().fold(String::with_capacity(32), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Maximum request body size (1 MB).
const MAX_BODY_SIZE: usize = 1_024 * 1_024;

fn build_router(state: SharedState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/credentials", get(credentials_handler))
        .route("/v4/{container_name}", get(container_metadata_handler))
        .route("/v4/{container_name}/task", get(task_metadata_handler))
        .route("/v4/{container_name}/stats", get(stats_not_implemented))
        .route(
            "/v4/{container_name}/task/stats",
            get(stats_not_implemented),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY_SIZE))
        .with_state(state)
}

async fn health_handler() -> StatusCode {
    StatusCode::OK
}

async fn credentials_handler(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let state = state.read().await;

    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == state.auth_token);

    if !authorized {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    state.credentials.as_ref().map_or_else(
        || StatusCode::NOT_FOUND.into_response(),
        |creds| (StatusCode::OK, Json(serde_json::to_value(creds).ok())).into_response(),
    )
}

async fn container_metadata_handler(
    State(state): State<SharedState>,
    Path(container_name): Path<String>,
) -> impl IntoResponse {
    let state = state.read().await;
    state.container_metadata.get(&container_name).map_or_else(
        || StatusCode::NOT_FOUND.into_response(),
        |meta| (StatusCode::OK, Json(serde_json::to_value(meta).ok())).into_response(),
    )
}

async fn task_metadata_handler(
    State(state): State<SharedState>,
    Path(container_name): Path<String>,
) -> impl IntoResponse {
    let state = state.read().await;
    // Verify container exists
    if !state.container_metadata.contains_key(&container_name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    (
        StatusCode::OK,
        Json(serde_json::to_value(&state.task_metadata).ok()),
    )
        .into_response()
}

async fn stats_not_implemented() -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::taskdef::{ContainerDefinition, NetworkMode, TaskDefinition};

    fn sample_task_def() -> TaskDefinition {
        TaskDefinition {
            family: "my-app".to_string(),
            network_mode: NetworkMode::Bridge,
            task_role_arn: Some("arn:aws:iam::123456789012:role/my-role".to_string()),
            execution_role_arn: None,
            volumes: vec![],
            container_definitions: vec![
                ContainerDefinition {
                    name: "web".to_string(),
                    image: "nginx:latest".to_string(),
                    cpu: Some(256),
                    memory: Some(512),
                    ..Default::default()
                },
                ContainerDefinition {
                    name: "sidecar".to_string(),
                    image: "redis:latest".to_string(),
                    essential: false,
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn task_metadata_json_format() {
        let task_def = sample_task_def();
        let meta = build_task_metadata(&task_def);
        let json = serde_json::to_value(&meta).expect("should serialize");

        // Verify PascalCase keys
        assert_eq!(json["Cluster"], "lecs-local");
        assert_eq!(
            json["TaskARN"],
            "arn:aws:ecs:local:000000000000:task/lecs/my-app"
        );
        assert_eq!(json["Family"], "my-app");
        assert_eq!(json["Revision"], "0");
        assert_eq!(json["DesiredStatus"], "RUNNING");
        assert_eq!(json["KnownStatus"], "RUNNING");
        assert_eq!(json["LaunchType"], "EC2");
        assert_eq!(json["AvailabilityZone"], "local");
        assert_eq!(
            json["TaskRoleArn"],
            "arn:aws:iam::123456789012:role/my-role"
        );
    }

    #[test]
    fn container_metadata_json_format() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "nginx:latest".to_string(),
            cpu: Some(256),
            memory: Some(512),
            ..Default::default()
        };

        let meta = build_container_metadata("my-app", &def);
        let json = serde_json::to_value(&meta).expect("should serialize");

        // Verify custom-renamed keys
        assert!(json.get("DockerId").is_some());
        assert_eq!(json["Name"], "app");
        assert_eq!(json["DockerName"], "my-app-app");
        assert_eq!(json["Image"], "nginx:latest");
        assert!(json.get("ImageID").is_some());
        assert_eq!(
            json["ContainerARN"],
            "arn:aws:ecs:local:000000000000:container/lecs/my-app/app"
        );
        assert_eq!(json["Type"], "NORMAL");
        assert!(json.get("CreatedAt").is_some());
        assert!(json.get("StartedAt").is_some());
    }

    #[test]
    fn build_task_metadata_sets_family() {
        let task_def = sample_task_def();
        let meta = build_task_metadata(&task_def);

        assert_eq!(meta.family, "my-app");
        assert_eq!(meta.cluster, "lecs-local");
        assert_eq!(
            meta.task_role_arn.as_deref(),
            Some("arn:aws:iam::123456789012:role/my-role")
        );
    }

    #[test]
    fn build_task_metadata_includes_containers() {
        let task_def = sample_task_def();
        let meta = build_task_metadata(&task_def);

        assert_eq!(meta.containers.len(), 2);
        assert_eq!(meta.containers[0].name, "web");
        assert_eq!(meta.containers[1].name, "sidecar");
    }

    #[test]
    fn build_container_metadata_limits() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "nginx:latest".to_string(),
            cpu: Some(256),
            memory: Some(512),
            ..Default::default()
        };

        let meta = build_container_metadata("test", &def);
        let limits = meta.limits.expect("should have limits");
        assert_eq!(limits.cpu, 256);
        assert_eq!(limits.memory, 512);
    }

    #[test]
    fn build_container_metadata_no_limits() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "nginx:latest".to_string(),
            ..Default::default()
        };

        let meta = build_container_metadata("test", &def);
        assert!(meta.limits.is_none());

        // Also verify Limits is omitted from JSON
        let json = serde_json::to_value(&meta).expect("should serialize");
        assert!(json.get("Limits").is_none());
    }

    #[test]
    fn build_task_metadata_no_role_arn() {
        let task_def = TaskDefinition {
            family: "test".to_string(),
            network_mode: NetworkMode::Bridge,
            task_role_arn: None,
            execution_role_arn: None,
            volumes: vec![],
            container_definitions: vec![ContainerDefinition {
                name: "app".to_string(),
                image: "alpine:latest".to_string(),
                ..Default::default()
            }],
        };

        let meta = build_task_metadata(&task_def);
        assert!(meta.task_role_arn.is_none());

        let json = serde_json::to_value(&meta).expect("should serialize");
        assert!(json.get("TaskRoleArn").is_none());
    }

    #[test]
    fn build_container_metadata_partial_limits_omitted() {
        // Only cpu set, no memory — limits should be None
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            cpu: Some(256),
            ..Default::default()
        };

        let meta = build_container_metadata("test", &def);
        assert!(meta.limits.is_none());
    }

    #[test]
    fn metadata_error_display() {
        let err = MetadataError::Bind(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "port in use",
        ));
        assert!(err.to_string().contains("failed to bind metadata server"));
    }

    // --- Server integration tests ---

    fn build_test_state(with_credentials: bool) -> SharedState {
        let task_def = sample_task_def();
        let task_metadata = build_task_metadata(&task_def);
        let container_metadata: HashMap<String, ContainerMetadata> = task_def
            .container_definitions
            .iter()
            .map(|def| {
                (
                    def.name.clone(),
                    build_container_metadata(&task_def.family, def),
                )
            })
            .collect();

        let credentials = if with_credentials {
            Some(AwsCredentials {
                access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
                secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
                token: Some("session-token".to_string()),
                expiration: "2026-03-21T01:00:00Z".to_string(),
                role_arn: None,
            })
        } else {
            None
        };

        Arc::new(RwLock::new(ServerState {
            task_metadata,
            container_metadata,
            credentials,
            container_ids: HashMap::new(),
            auth_token: "test-auth-token".to_string(),
        }))
    }

    /// Helper to start a test server and return (port, state).
    async fn start_test_server(with_credentials: bool) -> (u16, SharedState, MetadataServer) {
        let state = build_test_state(with_credentials);
        let server = MetadataServer::start(state.clone())
            .await
            .expect("should start");
        (server.port, state, server)
    }

    fn http_client() -> reqwest::Client {
        reqwest::Client::new()
    }

    #[tokio::test]
    async fn server_starts_on_random_port() {
        let (port, _, server) = start_test_server(true).await;
        assert_ne!(port, 0);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn health_endpoint_returns_200() {
        let (port, _, server) = start_test_server(true).await;
        let resp = reqwest::get(format!("http://127.0.0.1:{port}/health"))
            .await
            .expect("should connect");
        assert_eq!(resp.status(), 200);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn container_metadata_endpoint_returns_json() {
        let (port, _, server) = start_test_server(true).await;
        let resp = reqwest::get(format!("http://127.0.0.1:{port}/v4/web"))
            .await
            .expect("should connect");
        assert_eq!(resp.status(), 200);
        let json: serde_json::Value = resp.json().await.expect("should parse json");
        assert_eq!(json["Name"], "web");
        assert_eq!(json["DockerName"], "my-app-web");
        assert_eq!(json["Image"], "nginx:latest");
        server.shutdown().await;
    }

    #[tokio::test]
    async fn container_metadata_has_correct_docker_id() {
        let (port, state, server) = start_test_server(true).await;
        update_container_id(&state, "web", "abc123def456").await;

        let resp = reqwest::get(format!("http://127.0.0.1:{port}/v4/web"))
            .await
            .expect("should connect");
        let json: serde_json::Value = resp.json().await.expect("should parse");
        assert_eq!(json["DockerId"], "abc123def456");
        server.shutdown().await;
    }

    #[tokio::test]
    async fn task_metadata_endpoint_returns_json() {
        let (port, _, server) = start_test_server(true).await;
        let resp = reqwest::get(format!("http://127.0.0.1:{port}/v4/web/task"))
            .await
            .expect("should connect");
        assert_eq!(resp.status(), 200);
        let json: serde_json::Value = resp.json().await.expect("should parse json");
        assert_eq!(json["Family"], "my-app");
        assert_eq!(json["Cluster"], "lecs-local");
        assert_eq!(
            json["Containers"]
                .as_array()
                .expect("should be array")
                .len(),
            2
        );
        server.shutdown().await;
    }

    #[tokio::test]
    async fn credentials_endpoint_returns_json_with_valid_token() {
        let (port, _, server) = start_test_server(true).await;
        let resp = http_client()
            .get(format!("http://127.0.0.1:{port}/credentials"))
            .header("Authorization", "test-auth-token")
            .send()
            .await
            .expect("should connect");
        assert_eq!(resp.status(), 200);
        let json: serde_json::Value = resp.json().await.expect("should parse json");
        assert_eq!(json["AccessKeyId"], "AKIAIOSFODNN7EXAMPLE");
        server.shutdown().await;
    }

    #[tokio::test]
    async fn credentials_endpoint_returns_401_without_token() {
        let (port, _, server) = start_test_server(true).await;
        let resp = reqwest::get(format!("http://127.0.0.1:{port}/credentials"))
            .await
            .expect("should connect");
        assert_eq!(resp.status(), 401);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn credentials_endpoint_returns_401_with_wrong_token() {
        let (port, _, server) = start_test_server(true).await;
        let resp = http_client()
            .get(format!("http://127.0.0.1:{port}/credentials"))
            .header("Authorization", "wrong-token")
            .send()
            .await
            .expect("should connect");
        assert_eq!(resp.status(), 401);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn credentials_endpoint_returns_404_when_none() {
        let (port, _, server) = start_test_server(false).await;
        let resp = http_client()
            .get(format!("http://127.0.0.1:{port}/credentials"))
            .header("Authorization", "test-auth-token")
            .send()
            .await
            .expect("should connect");
        assert_eq!(resp.status(), 404);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn unknown_container_returns_404() {
        let (port, _, server) = start_test_server(true).await;
        let resp = reqwest::get(format!("http://127.0.0.1:{port}/v4/nonexistent"))
            .await
            .expect("should connect");
        assert_eq!(resp.status(), 404);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn unknown_container_task_returns_404() {
        let (port, _, server) = start_test_server(true).await;
        let resp = reqwest::get(format!("http://127.0.0.1:{port}/v4/nonexistent/task"))
            .await
            .expect("should connect");
        assert_eq!(resp.status(), 404);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn stats_endpoint_returns_501() {
        let (port, _, server) = start_test_server(true).await;
        let resp = reqwest::get(format!("http://127.0.0.1:{port}/v4/web/stats"))
            .await
            .expect("should connect");
        assert_eq!(resp.status(), 501);

        let resp = reqwest::get(format!("http://127.0.0.1:{port}/v4/web/task/stats"))
            .await
            .expect("should connect");
        assert_eq!(resp.status(), 501);
        server.shutdown().await;
    }

    #[test]
    fn generate_auth_token_is_nonempty_hex() {
        let token = generate_auth_token();
        assert_eq!(token.len(), 32);
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "token should be hex: {token}"
        );
    }

    #[test]
    fn generate_auth_token_is_unique() {
        let t1 = generate_auth_token();
        let t2 = generate_auth_token();
        assert_ne!(t1, t2, "consecutive tokens should differ");
    }

    #[test]
    fn generate_auth_token_length_is_consistent() {
        for _ in 0..100 {
            let token = generate_auth_token();
            assert_eq!(token.len(), 32, "token should always be 32 hex chars");
            assert!(
                token.chars().all(|c| c.is_ascii_hexdigit()),
                "token should be hex: {token}"
            );
        }
    }

    #[tokio::test]
    async fn server_shutdown_gracefully() {
        let (port, _, server) = start_test_server(true).await;
        // Verify it's running
        let resp = reqwest::get(format!("http://127.0.0.1:{port}/health"))
            .await
            .expect("should connect");
        assert_eq!(resp.status(), 200);

        // Shutdown
        server.shutdown().await;

        // After shutdown, connection should fail
        let result = reqwest::get(format!("http://127.0.0.1:{port}/health")).await;
        assert!(result.is_err());
    }
}
