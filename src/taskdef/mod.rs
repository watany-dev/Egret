use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;

/// Task definition parsing errors.
#[derive(Debug, thiserror::Error)]
pub enum TaskDefError {
    #[error("failed to read task definition from {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse task definition JSON: {0}")]
    ParseJson(#[from] serde_json::Error),

    #[error("task definition validation failed: {0}")]
    Validation(String),
}

/// ECS task definition top-level structure.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDefinition {
    /// Task family name (used for network name and labels).
    pub family: String,

    /// Container definitions.
    pub container_definitions: Vec<ContainerDefinition>,
}

/// Individual container definition.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerDefinition {
    /// Container name (used as Docker container name and DNS alias).
    pub name: String,

    /// Docker image.
    pub image: String,

    /// Essential flag (default: true).
    #[serde(default = "default_essential")]
    pub essential: bool,

    /// CMD equivalent.
    #[serde(default)]
    pub command: Vec<String>,

    /// ENTRYPOINT equivalent.
    #[serde(default)]
    pub entry_point: Vec<String>,

    /// Environment variables.
    #[serde(default)]
    pub environment: Vec<Environment>,

    /// Port mappings.
    #[serde(default)]
    pub port_mappings: Vec<PortMapping>,

    /// CPU units (1024 = 1 vCPU).
    pub cpu: Option<u32>,

    /// Hard memory limit (MiB).
    pub memory: Option<u32>,

    /// Soft memory limit (MiB).
    pub memory_reservation: Option<u32>,
}

fn default_essential() -> bool {
    true
}

/// Environment variable name-value pair.
#[derive(Debug, Clone, Deserialize)]
pub struct Environment {
    pub name: String,
    pub value: String,
}

/// Port mapping configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortMapping {
    /// Container-side port.
    pub container_port: u16,

    /// Host-side port (defaults to container port if omitted).
    pub host_port: Option<u16>,

    /// Protocol (default: "tcp").
    #[serde(default = "default_protocol")]
    pub protocol: String,
}

fn default_protocol() -> String {
    "tcp".to_string()
}

impl TaskDefinition {
    /// Load a task definition from a file path.
    pub fn from_file(path: &Path) -> Result<Self, TaskDefError> {
        let content = std::fs::read_to_string(path).map_err(|source| TaskDefError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json(&content)
    }

    /// Parse a task definition from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, TaskDefError> {
        let task_def: Self = serde_json::from_str(json)?;
        task_def.validate()?;
        Ok(task_def)
    }

    /// Validate the task definition (fail-fast).
    fn validate(&self) -> Result<(), TaskDefError> {
        if self.family.is_empty() {
            return Err(TaskDefError::Validation(
                "family must not be empty".to_string(),
            ));
        }
        if self.container_definitions.is_empty() {
            return Err(TaskDefError::Validation(
                "containerDefinitions must not be empty".to_string(),
            ));
        }
        for (i, container) in self.container_definitions.iter().enumerate() {
            if container.name.is_empty() {
                return Err(TaskDefError::Validation(format!(
                    "container name must not be empty at index {i}"
                )));
            }
            if container.image.is_empty() {
                return Err(TaskDefError::Validation(format!(
                    "container image must not be empty for container '{}'",
                    container.name
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_json() -> &'static str {
        r#"{
            "family": "my-app",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "essential": true,
                    "command": ["nginx", "-g", "daemon off;"],
                    "entryPoint": ["/docker-entrypoint.sh"],
                    "environment": [
                        { "name": "ENV_VAR", "value": "some-value" }
                    ],
                    "portMappings": [
                        { "containerPort": 80, "hostPort": 8080, "protocol": "tcp" }
                    ],
                    "cpu": 256,
                    "memory": 512,
                    "memoryReservation": 256
                }
            ]
        }"#
    }

    fn minimal_json() -> &'static str {
        r#"{
            "family": "minimal",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest"
                }
            ]
        }"#
    }

    #[test]
    fn parse_full_json() {
        let task_def = TaskDefinition::from_json(full_json()).expect("should parse");
        assert_eq!(task_def.family, "my-app");
        assert_eq!(task_def.container_definitions.len(), 1);

        let c = &task_def.container_definitions[0];
        assert_eq!(c.name, "app");
        assert_eq!(c.image, "nginx:latest");
        assert!(c.essential);
        assert_eq!(c.command, vec!["nginx", "-g", "daemon off;"]);
        assert_eq!(c.entry_point, vec!["/docker-entrypoint.sh"]);
        assert_eq!(c.environment.len(), 1);
        assert_eq!(c.environment[0].name, "ENV_VAR");
        assert_eq!(c.environment[0].value, "some-value");
        assert_eq!(c.port_mappings.len(), 1);
        assert_eq!(c.port_mappings[0].container_port, 80);
        assert_eq!(c.port_mappings[0].host_port, Some(8080));
        assert_eq!(c.port_mappings[0].protocol, "tcp");
        assert_eq!(c.cpu, Some(256));
        assert_eq!(c.memory, Some(512));
        assert_eq!(c.memory_reservation, Some(256));
    }

    #[test]
    fn parse_minimal_json() {
        let task_def = TaskDefinition::from_json(minimal_json()).expect("should parse");
        assert_eq!(task_def.family, "minimal");

        let c = &task_def.container_definitions[0];
        assert_eq!(c.name, "app");
        assert_eq!(c.image, "alpine:latest");
        assert!(c.essential); // default
        assert!(c.command.is_empty());
        assert!(c.entry_point.is_empty());
        assert!(c.environment.is_empty());
        assert!(c.port_mappings.is_empty());
        assert_eq!(c.cpu, None);
        assert_eq!(c.memory, None);
        assert_eq!(c.memory_reservation, None);
    }

    #[test]
    fn parse_ignores_unknown_fields() {
        let json = r#"{
            "family": "test",
            "taskRoleArn": "arn:aws:iam::123:role/test",
            "networkMode": "awsvpc",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "dockerLabels": { "env": "dev" },
                    "logConfiguration": { "logDriver": "awslogs" }
                }
            ]
        }"#;
        let task_def = TaskDefinition::from_json(json).expect("should ignore unknown fields");
        assert_eq!(task_def.family, "test");
    }

    #[test]
    fn error_empty_family() {
        let json = r#"{
            "family": "",
            "containerDefinitions": [
                { "name": "app", "image": "alpine:latest" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("family must not be empty")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn error_empty_container_definitions() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": []
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("containerDefinitions must not be empty")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn error_invalid_json() {
        let err = TaskDefinition::from_json("not json").unwrap_err();
        assert!(matches!(err, TaskDefError::ParseJson(_)));
    }

    #[test]
    fn error_missing_required_field() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(matches!(err, TaskDefError::ParseJson(_)));
    }

    #[test]
    fn error_empty_container_name() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "", "image": "alpine:latest" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("container name must not be empty")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn error_empty_container_image() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app", "image": "" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("container image must not be empty")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn defaults_essential_true() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app", "image": "alpine:latest" }
            ]
        }"#;
        let task_def = TaskDefinition::from_json(json).expect("should parse");
        assert!(task_def.container_definitions[0].essential);
    }

    #[test]
    fn defaults_protocol_tcp() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "portMappings": [{ "containerPort": 80 }]
                }
            ]
        }"#;
        let task_def = TaskDefinition::from_json(json).expect("should parse");
        let pm = &task_def.container_definitions[0].port_mappings[0];
        assert_eq!(pm.protocol, "tcp");
        assert_eq!(pm.host_port, None);
    }
}
