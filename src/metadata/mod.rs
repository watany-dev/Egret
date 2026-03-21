//! ECS metadata endpoint mock server.
//!
//! Provides ECS Task Metadata Endpoint V4 compatible responses and
//! an AWS credential provider endpoint for containers running locally.

use std::collections::HashMap;

use serde::Serialize;

use crate::credentials::AwsCredentials;
use crate::taskdef::{ContainerDefinition, TaskDefinition};

/// Metadata server errors.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum MetadataError {
    /// Failed to bind the server to a port.
    #[error("failed to bind metadata server: {0}")]
    Bind(#[source] std::io::Error),

    /// Generic server error.
    #[error("metadata server error: {0}")]
    Server(String),
}

/// Shared state for the metadata/credentials server.
#[allow(dead_code)]
pub struct ServerState {
    /// Task-level metadata.
    pub task_metadata: TaskMetadata,
    /// Per-container metadata, keyed by container name.
    pub container_metadata: HashMap<String, ContainerMetadata>,
    /// AWS credentials (None if unavailable).
    pub credentials: Option<AwsCredentials>,
    /// Mapping from container name to Docker container ID (populated after creation).
    pub container_ids: HashMap<String, String>,
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
#[allow(dead_code)]
pub fn build_task_metadata(task_def: &TaskDefinition) -> TaskMetadata {
    let task_arn = format!(
        "arn:aws:ecs:local:000000000000:task/egret/{}",
        task_def.family
    );

    let containers: Vec<ContainerMetadata> = task_def
        .container_definitions
        .iter()
        .map(|def| build_container_metadata(&task_def.family, def))
        .collect();

    TaskMetadata {
        cluster: "egret-local".to_string(),
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
#[allow(dead_code)]
pub fn build_container_metadata(family: &str, def: &ContainerDefinition) -> ContainerMetadata {
    let docker_name = format!("{family}-{}", def.name);
    let container_arn = format!(
        "arn:aws:ecs:local:000000000000:container/egret/{}/{}",
        family, def.name
    );

    let labels = HashMap::from([
        ("egret.managed".to_string(), "true".to_string()),
        ("egret.task".to_string(), family.to_string()),
        ("egret.container".to_string(), def.name.clone()),
    ]);

    let limits = match (def.cpu, def.memory) {
        (Some(cpu), Some(memory)) => Some(ContainerLimits { cpu, memory }),
        _ => None,
    };

    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::taskdef::{ContainerDefinition, TaskDefinition};

    fn sample_task_def() -> TaskDefinition {
        TaskDefinition {
            family: "my-app".to_string(),
            task_role_arn: Some("arn:aws:iam::123456789012:role/my-role".to_string()),
            execution_role_arn: None,
            container_definitions: vec![
                ContainerDefinition {
                    name: "web".to_string(),
                    image: "nginx:latest".to_string(),
                    essential: true,
                    command: vec![],
                    entry_point: vec![],
                    environment: vec![],
                    port_mappings: vec![],
                    secrets: vec![],
                    cpu: Some(256),
                    memory: Some(512),
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

    #[test]
    fn task_metadata_json_format() {
        let task_def = sample_task_def();
        let meta = build_task_metadata(&task_def);
        let json = serde_json::to_value(&meta).expect("should serialize");

        // Verify PascalCase keys
        assert_eq!(json["Cluster"], "egret-local");
        assert_eq!(
            json["TaskARN"],
            "arn:aws:ecs:local:000000000000:task/egret/my-app"
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
            essential: true,
            command: vec![],
            entry_point: vec![],
            environment: vec![],
            port_mappings: vec![],
            secrets: vec![],
            cpu: Some(256),
            memory: Some(512),
            memory_reservation: None,
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
            "arn:aws:ecs:local:000000000000:container/egret/my-app/app"
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
        assert_eq!(meta.cluster, "egret-local");
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
            essential: true,
            command: vec![],
            entry_point: vec![],
            environment: vec![],
            port_mappings: vec![],
            secrets: vec![],
            cpu: Some(256),
            memory: Some(512),
            memory_reservation: None,
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
            task_role_arn: None,
            execution_role_arn: None,
            container_definitions: vec![ContainerDefinition {
                name: "app".to_string(),
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
            essential: true,
            command: vec![],
            entry_point: vec![],
            environment: vec![],
            port_mappings: vec![],
            secrets: vec![],
            cpu: Some(256),
            memory: None,
            memory_reservation: None,
        };

        let meta = build_container_metadata("test", &def);
        assert!(meta.limits.is_none());
    }

    #[test]
    fn metadata_error_display() {
        let err = MetadataError::Server("test error".to_string());
        assert_eq!(err.to_string(), "metadata server error: test error");
    }
}
