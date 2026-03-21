//! ECS task definition parsing and types.

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

    /// IAM role ARN for the task (containers assume this role via credentials).
    #[serde(default)]
    #[allow(dead_code)]
    pub task_role_arn: Option<String>,

    /// IAM role ARN for the execution agent (used for pulling images, etc.).
    #[serde(default)]
    #[allow(dead_code)]
    pub execution_role_arn: Option<String>,

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
    #[allow(dead_code)]
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

    /// Secrets (Secrets Manager ARN references).
    #[serde(default)]
    pub secrets: Vec<Secret>,

    /// CPU units (1024 = 1 vCPU).
    #[allow(dead_code)]
    pub cpu: Option<u32>,

    /// Hard memory limit (MiB).
    #[allow(dead_code)]
    pub memory: Option<u32>,

    /// Soft memory limit (MiB).
    #[allow(dead_code)]
    pub memory_reservation: Option<u32>,

    /// Container dependencies (startup ordering).
    #[serde(default)]
    #[allow(dead_code)]
    pub depends_on: Vec<DependsOn>,

    /// Health check configuration.
    #[serde(default)]
    #[allow(dead_code)]
    pub health_check: Option<HealthCheck>,
}

const fn default_essential() -> bool {
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

/// Secret reference (Secrets Manager ARN).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Secret {
    /// Environment variable name to inject.
    pub name: String,
    /// ARN of the secret in Secrets Manager.
    pub value_from: String,
}

/// Dependency condition for `dependsOn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DependencyCondition {
    /// Container has started (default).
    Start,
    /// Container has exited (any exit code).
    Complete,
    /// Container has exited with code 0.
    Success,
    /// Container's health check reports healthy.
    Healthy,
}

/// Container dependency reference.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct DependsOn {
    /// Name of the container this dependency refers to.
    pub container_name: String,
    /// Condition that must be met before starting the dependent container.
    pub condition: DependencyCondition,
}

/// Health check configuration (ECS-compatible, seconds).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct HealthCheck {
    /// Health check command (e.g. `["CMD-SHELL", "curl -f http://localhost/"]`).
    pub command: Vec<String>,

    /// Interval between health checks in seconds (default: 30).
    #[serde(default = "default_health_interval")]
    pub interval: u32,

    /// Timeout for each health check in seconds (default: 5).
    #[serde(default = "default_health_timeout")]
    pub timeout: u32,

    /// Number of consecutive failures before marking unhealthy (default: 3).
    #[serde(default = "default_health_retries")]
    pub retries: u32,

    /// Grace period before health checks start in seconds (default: 0).
    #[serde(default)]
    pub start_period: u32,
}

const fn default_health_interval() -> u32 {
    30
}

const fn default_health_timeout() -> u32 {
    5
}

const fn default_health_retries() -> u32 {
    3
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
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

    #[test]
    fn parse_secrets_field() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "secrets": [
                        { "name": "DB_PASSWORD", "valueFrom": "arn:aws:secretsmanager:us-east-1:123:secret:db-pass" },
                        { "name": "API_KEY", "valueFrom": "arn:aws:secretsmanager:us-east-1:123:secret:api-key" }
                    ]
                }
            ]
        }"#;
        let task_def = TaskDefinition::from_json(json).expect("should parse");
        let secrets = &task_def.container_definitions[0].secrets;
        assert_eq!(secrets.len(), 2);
        assert_eq!(secrets[0].name, "DB_PASSWORD");
        assert_eq!(
            secrets[0].value_from,
            "arn:aws:secretsmanager:us-east-1:123:secret:db-pass"
        );
        assert_eq!(secrets[1].name, "API_KEY");
        assert_eq!(
            secrets[1].value_from,
            "arn:aws:secretsmanager:us-east-1:123:secret:api-key"
        );
    }

    #[test]
    fn parse_secrets_empty_default() {
        let task_def = TaskDefinition::from_json(minimal_json()).expect("should parse");
        assert!(task_def.container_definitions[0].secrets.is_empty());
    }

    #[test]
    fn parse_task_role_arn() {
        let json = r#"{
            "family": "test",
            "taskRoleArn": "arn:aws:iam::123456789012:role/my-role",
            "executionRoleArn": "arn:aws:iam::123456789012:role/exec-role",
            "containerDefinitions": [
                { "name": "app", "image": "alpine:latest" }
            ]
        }"#;
        let task_def = TaskDefinition::from_json(json).expect("should parse");
        assert_eq!(
            task_def.task_role_arn.as_deref(),
            Some("arn:aws:iam::123456789012:role/my-role")
        );
        assert_eq!(
            task_def.execution_role_arn.as_deref(),
            Some("arn:aws:iam::123456789012:role/exec-role")
        );
    }

    #[test]
    fn parse_no_role_arns_default() {
        let task_def = TaskDefinition::from_json(minimal_json()).expect("should parse");
        assert!(task_def.task_role_arn.is_none());
        assert!(task_def.execution_role_arn.is_none());
    }

    #[test]
    fn from_file_not_found() {
        let err = TaskDefinition::from_file(Path::new("/nonexistent/task.json")).unwrap_err();
        assert!(
            matches!(err, TaskDefError::ReadFile { .. }),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_depends_on_field() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "db",
                    "image": "postgres:16",
                    "healthCheck": {
                        "command": ["CMD-SHELL", "pg_isready"]
                    }
                },
                {
                    "name": "app",
                    "image": "my-app:latest",
                    "dependsOn": [
                        { "containerName": "db", "condition": "HEALTHY" }
                    ]
                }
            ]
        }"#;
        let task_def = TaskDefinition::from_json(json).expect("should parse");
        let app = &task_def.container_definitions[1];
        assert_eq!(app.depends_on.len(), 1);
        assert_eq!(app.depends_on[0].container_name, "db");
        assert_eq!(app.depends_on[0].condition, DependencyCondition::Healthy);
    }

    #[test]
    fn parse_depends_on_empty_default() {
        let task_def = TaskDefinition::from_json(minimal_json()).expect("should parse");
        assert!(task_def.container_definitions[0].depends_on.is_empty());
    }

    #[test]
    fn parse_health_check_field() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "healthCheck": {
                        "command": ["CMD-SHELL", "curl -f http://localhost/"],
                        "interval": 10,
                        "timeout": 3,
                        "retries": 5,
                        "startPeriod": 15
                    }
                }
            ]
        }"#;
        let task_def = TaskDefinition::from_json(json).expect("should parse");
        let hc = task_def.container_definitions[0]
            .health_check
            .as_ref()
            .expect("should have health check");
        assert_eq!(hc.command, vec!["CMD-SHELL", "curl -f http://localhost/"]);
        assert_eq!(hc.interval, 10);
        assert_eq!(hc.timeout, 3);
        assert_eq!(hc.retries, 5);
        assert_eq!(hc.start_period, 15);
    }

    #[test]
    fn parse_health_check_defaults() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "healthCheck": {
                        "command": ["CMD-SHELL", "true"]
                    }
                }
            ]
        }"#;
        let task_def = TaskDefinition::from_json(json).expect("should parse");
        let hc = task_def.container_definitions[0]
            .health_check
            .as_ref()
            .expect("should have health check");
        assert_eq!(hc.command, vec!["CMD-SHELL", "true"]);
        assert_eq!(hc.interval, 30);
        assert_eq!(hc.timeout, 5);
        assert_eq!(hc.retries, 3);
        assert_eq!(hc.start_period, 0);
    }

    #[test]
    fn parse_health_check_none_default() {
        let task_def = TaskDefinition::from_json(minimal_json()).expect("should parse");
        assert!(task_def.container_definitions[0].health_check.is_none());
    }

    #[test]
    fn parse_dependency_condition_variants() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "a", "image": "alpine:latest" },
                { "name": "b", "image": "alpine:latest" },
                { "name": "c", "image": "alpine:latest" },
                { "name": "d", "image": "alpine:latest" },
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "dependsOn": [
                        { "containerName": "a", "condition": "START" },
                        { "containerName": "b", "condition": "COMPLETE" },
                        { "containerName": "c", "condition": "SUCCESS" },
                        { "containerName": "d", "condition": "HEALTHY" }
                    ]
                }
            ]
        }"#;
        let task_def = TaskDefinition::from_json(json).expect("should parse");
        let deps = &task_def.container_definitions[4].depends_on;
        assert_eq!(deps.len(), 4);
        assert_eq!(deps[0].condition, DependencyCondition::Start);
        assert_eq!(deps[1].condition, DependencyCondition::Complete);
        assert_eq!(deps[2].condition, DependencyCondition::Success);
        assert_eq!(deps[3].condition, DependencyCondition::Healthy);
    }
}
