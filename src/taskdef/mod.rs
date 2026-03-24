//! ECS task definition parsing and types.

pub mod cloudformation;
pub mod diagnostics;
pub mod terraform;

use std::collections::HashMap;
use std::collections::HashSet;
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

    #[error("task definition file too large ({size} bytes, max {max} bytes): {path}")]
    FileTooLarge { path: PathBuf, size: u64, max: u64 },

    #[error("no aws_ecs_task_definition resource found in Terraform JSON")]
    TerraformNoEcsResource,

    #[error(
        "multiple aws_ecs_task_definition resources found: {resources:?}. Use --tf-resource to specify one"
    )]
    TerraformMultipleResources { resources: Vec<String> },

    #[error("terraform resource '{0}' not found")]
    TerraformResourceNotFound(String),

    #[error("failed to parse Terraform JSON: {0}")]
    ParseTerraformJson(String),

    #[error("no AWS::ECS::TaskDefinition resource found in CloudFormation template")]
    CfnNoEcsResource,

    #[error(
        "multiple AWS::ECS::TaskDefinition resources found: {resources:?}. Use --cfn-resource to specify one"
    )]
    CfnMultipleResources { resources: Vec<String> },

    #[error("CloudFormation resource '{0}' not found")]
    CfnResourceNotFound(String),

    #[error("failed to parse CloudFormation template: {0}")]
    ParseCfnJson(String),

    #[error("CloudFormation intrinsic function found in {field}: {detail}")]
    CfnIntrinsicFunction { field: String, detail: String },

    #[error("failed to read environment file {path}: {source}")]
    EnvironmentFileRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("invalid line in environment file {path} at line {line_number}: {detail}")]
    EnvironmentFileParse {
        path: PathBuf,
        line_number: usize,
        detail: String,
    },
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

    /// Task-level volume definitions.
    #[serde(default)]
    pub volumes: Vec<Volume>,

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

    /// Secrets (Secrets Manager ARN references).
    #[serde(default)]
    pub secrets: Vec<Secret>,

    /// CPU units (1024 = 1 vCPU).
    pub cpu: Option<u32>,

    /// Hard memory limit (MiB).
    pub memory: Option<u32>,

    /// Soft memory limit (MiB).
    pub memory_reservation: Option<u32>,

    /// Container dependencies (startup ordering).
    #[serde(default)]
    pub depends_on: Vec<DependsOn>,

    /// Health check configuration.
    #[serde(default)]
    pub health_check: Option<HealthCheck>,

    /// Mount points referencing task-level volumes.
    #[serde(default)]
    pub mount_points: Vec<MountPoint>,

    /// Docker labels to apply to the container.
    #[serde(default)]
    pub docker_labels: HashMap<String, String>,

    /// Working directory inside the container.
    #[serde(default)]
    pub working_directory: Option<String>,

    /// User to run the container as (e.g., "uid", "uid:gid", "username").
    #[serde(default)]
    pub user: Option<String>,

    /// Extra host-to-IP mappings.
    #[serde(default)]
    pub extra_hosts: Vec<ExtraHost>,

    /// Timeout in seconds before the container is forcibly killed (ECS default: 30).
    #[serde(default)]
    pub stop_timeout: Option<u32>,

    /// Paths to environment files (.env format) for additional environment variables.
    #[serde(default)]
    pub environment_files: Vec<EnvironmentFile>,

    /// Resource limits (ulimits) for the container.
    #[serde(default)]
    pub ulimits: Vec<Ulimit>,
}

impl Default for ContainerDefinition {
    fn default() -> Self {
        Self {
            name: String::new(),
            image: String::new(),
            essential: true,
            command: Vec::new(),
            entry_point: Vec::new(),
            environment: Vec::new(),
            port_mappings: Vec::new(),
            secrets: Vec::new(),
            cpu: None,
            memory: None,
            memory_reservation: None,
            depends_on: Vec::new(),
            health_check: None,
            mount_points: Vec::new(),
            docker_labels: HashMap::new(),
            working_directory: None,
            user: None,
            extra_hosts: Vec::new(),
            stop_timeout: None,
            environment_files: Vec::new(),
            ulimits: Vec::new(),
        }
    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
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
pub struct DependsOn {
    /// Name of the container this dependency refers to.
    pub container_name: String,
    /// Condition that must be met before starting the dependent container.
    pub condition: DependencyCondition,
}

/// Health check configuration (ECS-compatible, seconds).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// Task-level volume definition (ECS-compatible).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    /// Volume name (referenced by mountPoints).
    pub name: String,
    /// Host path for bind mount. None for Docker-managed volumes (skipped).
    #[serde(default)]
    pub host: Option<VolumeHost>,
}

/// Host path for bind mount volumes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeHost {
    /// Absolute path on the host machine.
    pub source_path: String,
}

/// Container mount point referencing a task-level volume.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MountPoint {
    /// Name of the volume to mount (must match a Volume.name).
    pub source_volume: String,
    /// Absolute path inside the container.
    pub container_path: String,
    /// Mount as read-only (default: false).
    #[serde(default)]
    pub read_only: bool,
}

/// Extra host-to-IP mapping (ECS extraHosts format).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraHost {
    /// Hostname to map.
    pub hostname: String,
    /// IP address to map to.
    pub ip_address: String,
}

/// Environment file reference (ECS environmentFiles format).
///
/// In ECS, `value` is an S3 ARN. Lecs treats it as a local file path
/// and loads `.env`-formatted key-value pairs from it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentFile {
    /// File path (S3 ARN in ECS; local path in Lecs).
    pub value: String,
    /// File type — always "s3" in ECS. Lecs ignores this field.
    #[serde(default = "default_env_file_type")]
    pub r#type: String,
}

fn default_env_file_type() -> String {
    "s3".to_string()
}

/// Resource limit (ulimit) for a container.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ulimit {
    /// Ulimit name (e.g., "nofile", "memlock", "nproc").
    pub name: String,
    /// Soft limit.
    pub soft_limit: i64,
    /// Hard limit.
    pub hard_limit: i64,
}

/// Load environment variables from `.env`-formatted files.
///
/// Each file is read line by line. Lines starting with `#` are comments,
/// empty lines are skipped, and lines without `=` are ignored.
/// Returns key-value pairs in insertion order; later files override earlier ones.
pub fn load_environment_files(
    files: &[EnvironmentFile],
    base_dir: &Path,
) -> Result<Vec<(String, String)>, TaskDefError> {
    let mut vars = Vec::new();
    for ef in files {
        let path = base_dir.join(&ef.value);
        let content = std::fs::read_to_string(&path).map_err(|source| {
            TaskDefError::EnvironmentFileRead {
                path: path.clone(),
                source,
            }
        })?;
        for (line_number, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = trimmed.split_once('=') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                // Strip surrounding quotes if present
                let value = strip_quotes(&value);
                if !key.is_empty() {
                    vars.push((key, value));
                }
            } else {
                tracing::warn!(
                    file = %path.display(),
                    line = line_number + 1,
                    "Skipping line without '=' in environment file"
                );
            }
        }
    }
    Ok(vars)
}

/// Strip matching surrounding single or double quotes from a value.
fn strip_quotes(s: &str) -> String {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"'))
            || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
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

/// Maximum task definition file size (10 MB).
const MAX_TASKDEF_FILE_SIZE: u64 = 10 * 1024 * 1024;

impl TaskDefinition {
    /// Load a task definition from a file path.
    pub fn from_file(path: &Path) -> Result<Self, TaskDefError> {
        let metadata = std::fs::metadata(path).map_err(|source| TaskDefError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
        if metadata.len() > MAX_TASKDEF_FILE_SIZE {
            return Err(TaskDefError::FileTooLarge {
                path: path.to_path_buf(),
                size: metadata.len(),
                max: MAX_TASKDEF_FILE_SIZE,
            });
        }
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

    /// Validate a health check command.
    fn validate_health_check(hc: &HealthCheck, container_name: &str) -> Result<(), TaskDefError> {
        if hc.command.is_empty() {
            return Err(TaskDefError::Validation(format!(
                "healthCheck command must not be empty for container '{container_name}'"
            )));
        }
        let valid_prefixes = ["CMD-SHELL", "CMD", "NONE"];
        if !valid_prefixes.contains(&hc.command[0].as_str()) {
            return Err(TaskDefError::Validation(format!(
                "healthCheck command must start with CMD-SHELL, CMD, or NONE for container '{container_name}', got '{}'",
                hc.command[0]
            )));
        }
        Ok(())
    }

    /// Validate `dependsOn` references.
    fn validate_depends_on(&self) -> Result<(), TaskDefError> {
        let names: HashSet<&str> = self
            .container_definitions
            .iter()
            .map(|c| c.name.as_str())
            .collect();

        let has_health_check: HashSet<&str> = self
            .container_definitions
            .iter()
            .filter(|c| c.health_check.is_some())
            .map(|c| c.name.as_str())
            .collect();

        for container in &self.container_definitions {
            for dep in &container.depends_on {
                if dep.container_name == container.name {
                    return Err(TaskDefError::Validation(format!(
                        "container '{}' has a self-referencing dependsOn",
                        container.name
                    )));
                }
                if !names.contains(dep.container_name.as_str()) {
                    return Err(TaskDefError::Validation(format!(
                        "container '{}' depends on unknown container '{}'",
                        container.name, dep.container_name
                    )));
                }
                if dep.condition == DependencyCondition::Healthy
                    && !has_health_check.contains(dep.container_name.as_str())
                {
                    return Err(TaskDefError::Validation(format!(
                        "container '{}' depends on '{}' with HEALTHY condition, but '{}' has no healthCheck",
                        container.name, dep.container_name, dep.container_name
                    )));
                }
            }
        }
        Ok(())
    }

    /// Validate that a path is absolute and does not contain parent directory traversal.
    fn validate_path_safety(
        path: &str,
        field_name: &str,
        context: &str,
    ) -> Result<(), TaskDefError> {
        if !path.starts_with('/') {
            return Err(TaskDefError::Validation(format!(
                "{context}: {field_name} must be an absolute path, got '{path}'"
            )));
        }
        for component in std::path::Path::new(path).components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(TaskDefError::Validation(format!(
                    "{context}: {field_name} must not contain '..' path traversal, got '{path}'"
                )));
            }
        }
        Ok(())
    }

    /// Validate `mountPoints` references against task-level `volumes`.
    fn validate_mount_points(&self) -> Result<(), TaskDefError> {
        let volume_names: HashSet<&str> = self.volumes.iter().map(|v| v.name.as_str()).collect();

        for container in &self.container_definitions {
            for mp in &container.mount_points {
                if !volume_names.contains(mp.source_volume.as_str()) {
                    return Err(TaskDefError::Validation(format!(
                        "container '{}' references unknown volume '{}'",
                        container.name, mp.source_volume
                    )));
                }
                if mp.container_path.is_empty() {
                    return Err(TaskDefError::Validation(format!(
                        "container '{}' has mountPoint with empty containerPath for volume '{}'",
                        container.name, mp.source_volume
                    )));
                }
                Self::validate_path_safety(
                    &mp.container_path,
                    "containerPath",
                    &format!(
                        "container '{}', volume '{}'",
                        container.name, mp.source_volume
                    ),
                )?;
            }
        }

        // Validate host.source_path when host is present
        for volume in &self.volumes {
            if let Some(host) = &volume.host {
                if host.source_path.is_empty() {
                    return Err(TaskDefError::Validation(format!(
                        "volume '{}' has empty host.sourcePath",
                        volume.name
                    )));
                }
                Self::validate_path_safety(
                    &host.source_path,
                    "host.sourcePath",
                    &format!("volume '{}'", volume.name),
                )?;
            }
        }

        Ok(())
    }

    /// Maximum length for a container name (ECS specification).
    const MAX_CONTAINER_NAME_LEN: usize = 255;

    /// Validate that a container name matches ECS naming rules: `[a-zA-Z0-9_-]{1,255}`.
    fn validate_container_name(name: &str, index: usize) -> Result<(), TaskDefError> {
        if name.is_empty() {
            return Err(TaskDefError::Validation(format!(
                "container name must not be empty at index {index}"
            )));
        }
        if name.len() > Self::MAX_CONTAINER_NAME_LEN {
            return Err(TaskDefError::Validation(format!(
                "container name must not exceed {} characters at index {index}, \
                 got {} characters: '{name}'",
                Self::MAX_CONTAINER_NAME_LEN,
                name.len()
            )));
        }
        if let Some(pos) = name.find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
            // `pos` is a byte offset from `find`, but all valid chars are ASCII (1 byte each),
            // so byte offset == char index for the first invalid character.
            let invalid_char = name[pos..].chars().next().unwrap_or('?');
            return Err(TaskDefError::Validation(format!(
                "container name contains invalid character '{invalid_char}' at position {pos} \
                 in '{name}' at index {index} (allowed: a-z, A-Z, 0-9, _, -)"
            )));
        }
        Ok(())
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
            Self::validate_container_name(&container.name, i)?;
            if container.image.is_empty() {
                return Err(TaskDefError::Validation(format!(
                    "container image must not be empty for container '{}'",
                    container.name
                )));
            }
            if let Some(hc) = &container.health_check {
                Self::validate_health_check(hc, &container.name)?;
            }
            if let Some(wd) = &container.working_directory {
                Self::validate_path_safety(
                    wd,
                    "workingDirectory",
                    &format!("container '{}'", container.name),
                )?;
            }
        }
        self.validate_depends_on()?;
        self.validate_mount_points()
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
                { "name": "d", "image": "alpine:latest", "healthCheck": { "command": ["CMD-SHELL", "true"] } },
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

    #[test]
    fn validate_depends_on_unknown_container() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "dependsOn": [
                        { "containerName": "nonexistent", "condition": "START" }
                    ]
                }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("unknown container 'nonexistent'")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_depends_on_self_reference() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "dependsOn": [
                        { "containerName": "app", "condition": "START" }
                    ]
                }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("self-referencing")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_depends_on_healthy_requires_health_check() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "db", "image": "postgres:16" },
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "dependsOn": [
                        { "containerName": "db", "condition": "HEALTHY" }
                    ]
                }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("has no healthCheck")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_health_check_empty_command() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "healthCheck": { "command": [] }
                }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("healthCheck command must not be empty")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_health_check_invalid_prefix() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "healthCheck": { "command": ["INVALID", "curl localhost"] }
                }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("CMD-SHELL, CMD, or NONE")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_health_check_cmd_prefix_valid() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "healthCheck": { "command": ["CMD", "/bin/check"] }
                }
            ]
        }"#;
        let result = TaskDefinition::from_json(json);
        assert!(result.is_ok(), "CMD prefix should be valid");
    }

    #[test]
    fn validate_health_check_none_prefix_valid() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "healthCheck": { "command": ["NONE"] }
                }
            ]
        }"#;
        let result = TaskDefinition::from_json(json);
        assert!(result.is_ok(), "NONE prefix should be valid");
    }

    #[test]
    fn error_file_too_large_display() {
        let err = TaskDefError::FileTooLarge {
            path: PathBuf::from("/tmp/big.json"),
            size: 20_000_000,
            max: MAX_TASKDEF_FILE_SIZE,
        };
        let msg = err.to_string();
        assert!(msg.contains("too large"));
        assert!(msg.contains("20000000"));
    }

    #[test]
    fn from_file_reads_valid_json() {
        let dir = std::env::temp_dir().join("lecs-test-taskdef");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("valid.json");
        std::fs::write(
            &path,
            r#"{"family":"test","containerDefinitions":[{"name":"app","image":"alpine:latest"}]}"#,
        )
        .unwrap();
        let task_def = TaskDefinition::from_file(&path).expect("should parse from file");
        assert_eq!(task_def.family, "test");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn validate_depends_on_valid() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "db",
                    "image": "postgres:16",
                    "healthCheck": { "command": ["CMD-SHELL", "pg_isready"] }
                },
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "dependsOn": [
                        { "containerName": "db", "condition": "HEALTHY" }
                    ]
                }
            ]
        }"#;
        let task_def = TaskDefinition::from_json(json).expect("should parse valid dependsOn");
        assert_eq!(task_def.container_definitions[1].depends_on.len(), 1);
    }

    #[test]
    fn parse_volumes_and_mount_points() {
        let json = r#"{
            "family": "test",
            "volumes": [
                { "name": "app-data", "host": { "sourcePath": "/home/user/data" } },
                { "name": "tmp-cache" }
            ],
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "mountPoints": [
                        { "sourceVolume": "app-data", "containerPath": "/data", "readOnly": false },
                        { "sourceVolume": "tmp-cache", "containerPath": "/tmp/cache", "readOnly": true }
                    ]
                }
            ]
        }"#;
        let task_def = TaskDefinition::from_json(json).expect("should parse");
        assert_eq!(task_def.volumes.len(), 2);
        assert_eq!(task_def.volumes[0].name, "app-data");
        assert_eq!(
            task_def.volumes[0].host.as_ref().unwrap().source_path,
            "/home/user/data"
        );
        assert!(task_def.volumes[1].host.is_none());

        let mps = &task_def.container_definitions[0].mount_points;
        assert_eq!(mps.len(), 2);
        assert_eq!(mps[0].source_volume, "app-data");
        assert_eq!(mps[0].container_path, "/data");
        assert!(!mps[0].read_only);
        assert_eq!(mps[1].source_volume, "tmp-cache");
        assert_eq!(mps[1].container_path, "/tmp/cache");
        assert!(mps[1].read_only);
    }

    #[test]
    fn parse_volumes_empty_default() {
        let task_def = TaskDefinition::from_json(minimal_json()).expect("should parse");
        assert!(task_def.volumes.is_empty());
        assert!(task_def.container_definitions[0].mount_points.is_empty());
    }

    #[test]
    fn parse_mount_points_read_only_default_false() {
        let json = r#"{
            "family": "test",
            "volumes": [
                { "name": "data", "host": { "sourcePath": "/data" } }
            ],
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "mountPoints": [
                        { "sourceVolume": "data", "containerPath": "/data" }
                    ]
                }
            ]
        }"#;
        let task_def = TaskDefinition::from_json(json).expect("should parse");
        assert!(!task_def.container_definitions[0].mount_points[0].read_only);
    }

    #[test]
    fn validate_mount_points_unknown_volume() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "mountPoints": [
                        { "sourceVolume": "nonexistent", "containerPath": "/data" }
                    ]
                }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("unknown volume 'nonexistent'")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_mount_points_empty_container_path() {
        let json = r#"{
            "family": "test",
            "volumes": [
                { "name": "data", "host": { "sourcePath": "/data" } }
            ],
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "mountPoints": [
                        { "sourceVolume": "data", "containerPath": "" }
                    ]
                }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("empty containerPath")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_volume_empty_source_path() {
        let json = r#"{
            "family": "test",
            "volumes": [
                { "name": "data", "host": { "sourcePath": "" } }
            ],
            "containerDefinitions": [
                { "name": "app", "image": "alpine:latest" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("empty host.sourcePath")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_source_path_relative_rejected() {
        let json = r#"{
            "family": "test",
            "volumes": [
                { "name": "data", "host": { "sourcePath": "relative/path" } }
            ],
            "containerDefinitions": [
                { "name": "app", "image": "alpine:latest" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("must be an absolute path")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_source_path_traversal_rejected() {
        let json = r#"{
            "family": "test",
            "volumes": [
                { "name": "data", "host": { "sourcePath": "/safe/../../etc/shadow" } }
            ],
            "containerDefinitions": [
                { "name": "app", "image": "alpine:latest" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("must not contain '..' path traversal")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_source_path_bare_traversal_rejected() {
        let json = r#"{
            "family": "test",
            "volumes": [
                { "name": "data", "host": { "sourcePath": "../../../etc" } }
            ],
            "containerDefinitions": [
                { "name": "app", "image": "alpine:latest" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("must be an absolute path")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_container_path_relative_rejected() {
        let json = r#"{
            "family": "test",
            "volumes": [
                { "name": "data", "host": { "sourcePath": "/data" } }
            ],
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "mountPoints": [
                        { "sourceVolume": "data", "containerPath": "relative" }
                    ]
                }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("must be an absolute path")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_container_path_traversal_rejected() {
        let json = r#"{
            "family": "test",
            "volumes": [
                { "name": "data", "host": { "sourcePath": "/data" } }
            ],
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "mountPoints": [
                        { "sourceVolume": "data", "containerPath": "/app/../escape" }
                    ]
                }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("must not contain '..' path traversal")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_path_with_dot_allowed() {
        let json = r#"{
            "family": "test",
            "volumes": [
                { "name": "data", "host": { "sourcePath": "/path/./with/dot" } }
            ],
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "alpine:latest",
                    "mountPoints": [
                        { "sourceVolume": "data", "containerPath": "/app/./data" }
                    ]
                }
            ]
        }"#;
        let result = TaskDefinition::from_json(json);
        assert!(result.is_ok(), "single dot in path should be allowed");
    }

    // --- Container name validation (VULN-004) ---

    #[test]
    fn validate_container_name_alphanumeric() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app", "image": "alpine:latest" }
            ]
        }"#;
        assert!(TaskDefinition::from_json(json).is_ok());
    }

    #[test]
    fn validate_container_name_with_hyphen() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "my-app", "image": "alpine:latest" }
            ]
        }"#;
        assert!(TaskDefinition::from_json(json).is_ok());
    }

    #[test]
    fn validate_container_name_with_underscore() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "my_app", "image": "alpine:latest" }
            ]
        }"#;
        assert!(TaskDefinition::from_json(json).is_ok());
    }

    #[test]
    fn validate_container_name_numeric_start() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "123app", "image": "alpine:latest" }
            ]
        }"#;
        assert!(TaskDefinition::from_json(json).is_ok());
    }

    #[test]
    fn validate_container_name_max_length() {
        let long_name = "a".repeat(255);
        let json = format!(
            r#"{{"family": "test", "containerDefinitions": [{{"name": "{long_name}", "image": "alpine:latest"}}]}}"#
        );
        assert!(
            TaskDefinition::from_json(&json).is_ok(),
            "255-char name should be valid"
        );
    }

    #[test]
    fn validate_container_name_exceeds_max_length() {
        let long_name = "a".repeat(256);
        let json = format!(
            r#"{{"family": "test", "containerDefinitions": [{{"name": "{long_name}", "image": "alpine:latest"}}]}}"#
        );
        let err = TaskDefinition::from_json(&json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("must not exceed 255 characters")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_container_name_space_rejected() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "my app", "image": "alpine:latest" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("invalid character")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_container_name_dot_rejected() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "my.app", "image": "alpine:latest" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("invalid character")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_container_name_slash_rejected() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "my/app", "image": "alpine:latest" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("invalid character")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_container_name_unicode_rejected() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "アプリ", "image": "alpine:latest" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("invalid character")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_container_name_special_chars_rejected() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app;rm -rf", "image": "alpine:latest" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("invalid character")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_container_name_path_traversal_rejected() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "../escape", "image": "alpine:latest" }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("invalid character")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_docker_labels() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "dockerLabels": {
                        "com.example.env": "dev",
                        "com.example.team": "platform"
                    }
                }
            ]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        let labels = &td.container_definitions[0].docker_labels;
        assert_eq!(labels.len(), 2);
        assert_eq!(labels.get("com.example.env").unwrap(), "dev");
        assert_eq!(labels.get("com.example.team").unwrap(), "platform");
    }

    #[test]
    fn parse_docker_labels_defaults_to_empty() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app", "image": "nginx:latest" }
            ]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert!(td.container_definitions[0].docker_labels.is_empty());
    }

    #[test]
    fn parse_working_directory() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "workingDirectory": "/app"
                }
            ]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert_eq!(
            td.container_definitions[0].working_directory.as_deref(),
            Some("/app")
        );
    }

    #[test]
    fn validate_working_directory_relative_path_rejected() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "workingDirectory": "relative/path"
                }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("absolute path")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_working_directory_traversal_rejected() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "workingDirectory": "/app/../etc"
                }
            ]
        }"#;
        let err = TaskDefinition::from_json(json).unwrap_err();
        assert!(
            matches!(err, TaskDefError::Validation(ref msg) if msg.contains("path traversal")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_user_field() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "user": "1000:1000"
                }
            ]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert_eq!(
            td.container_definitions[0].user.as_deref(),
            Some("1000:1000")
        );
    }

    #[test]
    fn parse_user_field_defaults_to_none() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app", "image": "nginx:latest" }
            ]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert!(td.container_definitions[0].user.is_none());
    }

    #[test]
    fn parse_extra_hosts() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "extraHosts": [
                        { "hostname": "myhost", "ipAddress": "10.0.0.1" }
                    ]
                }
            ]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert_eq!(td.container_definitions[0].extra_hosts.len(), 1);
        assert_eq!(
            td.container_definitions[0].extra_hosts[0].hostname,
            "myhost"
        );
        assert_eq!(
            td.container_definitions[0].extra_hosts[0].ip_address,
            "10.0.0.1"
        );
    }

    #[test]
    fn parse_extra_hosts_defaults_to_empty() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app", "image": "nginx:latest" }
            ]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert!(td.container_definitions[0].extra_hosts.is_empty());
    }

    #[test]
    fn parse_stop_timeout() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "stopTimeout": 60
                }
            ]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert_eq!(td.container_definitions[0].stop_timeout, Some(60));
    }

    #[test]
    fn parse_stop_timeout_defaults_to_none() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app", "image": "nginx:latest" }
            ]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert!(td.container_definitions[0].stop_timeout.is_none());
    }

    // --- environmentFiles tests ---

    #[test]
    fn parse_environment_files_field() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "environmentFiles": [
                    { "value": "app.env", "type": "s3" }
                ]
            }]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert_eq!(td.container_definitions[0].environment_files.len(), 1);
        assert_eq!(td.container_definitions[0].environment_files[0].value, "app.env");
        assert_eq!(td.container_definitions[0].environment_files[0].r#type, "s3");
    }

    #[test]
    fn parse_environment_files_defaults_to_empty() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [{ "name": "app", "image": "nginx:latest" }]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert!(td.container_definitions[0].environment_files.is_empty());
    }

    #[test]
    fn parse_environment_files_type_defaults_to_s3() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "environmentFiles": [{ "value": "test.env" }]
            }]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert_eq!(td.container_definitions[0].environment_files[0].r#type, "s3");
    }

    #[test]
    fn load_environment_files_basic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.env"), "FOO=bar\nBAZ=qux\n").unwrap();
        let files = vec![EnvironmentFile {
            value: "test.env".to_string(),
            r#type: "s3".to_string(),
        }];
        let vars = load_environment_files(&files, dir.path()).unwrap();
        assert_eq!(vars, vec![
            ("FOO".to_string(), "bar".to_string()),
            ("BAZ".to_string(), "qux".to_string()),
        ]);
    }

    #[test]
    fn load_environment_files_comments_and_empty_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test.env"),
            "# This is a comment\n\nFOO=bar\n  \n# Another comment\nBAZ=qux\n",
        )
        .unwrap();
        let files = vec![EnvironmentFile {
            value: "test.env".to_string(),
            r#type: "s3".to_string(),
        }];
        let vars = load_environment_files(&files, dir.path()).unwrap();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[0], ("FOO".to_string(), "bar".to_string()));
    }

    #[test]
    fn load_environment_files_quoted_values() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test.env"),
            "FOO=\"hello world\"\nBAR='single quoted'\n",
        )
        .unwrap();
        let files = vec![EnvironmentFile {
            value: "test.env".to_string(),
            r#type: "s3".to_string(),
        }];
        let vars = load_environment_files(&files, dir.path()).unwrap();
        assert_eq!(vars[0], ("FOO".to_string(), "hello world".to_string()));
        assert_eq!(vars[1], ("BAR".to_string(), "single quoted".to_string()));
    }

    #[test]
    fn load_environment_files_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![EnvironmentFile {
            value: "nonexistent.env".to_string(),
            r#type: "s3".to_string(),
        }];
        let result = load_environment_files(&files, dir.path());
        assert!(matches!(result, Err(TaskDefError::EnvironmentFileRead { .. })));
    }

    #[test]
    fn load_environment_files_value_with_equals() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.env"), "CONNECTION=host=db;port=5432\n").unwrap();
        let files = vec![EnvironmentFile {
            value: "test.env".to_string(),
            r#type: "s3".to_string(),
        }];
        let vars = load_environment_files(&files, dir.path()).unwrap();
        assert_eq!(
            vars[0],
            ("CONNECTION".to_string(), "host=db;port=5432".to_string())
        );
    }

    #[test]
    fn load_environment_files_multiple_files_ordering() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.env"), "FOO=first\nBAR=a\n").unwrap();
        std::fs::write(dir.path().join("b.env"), "FOO=second\nBAZ=b\n").unwrap();
        let files = vec![
            EnvironmentFile {
                value: "a.env".to_string(),
                r#type: "s3".to_string(),
            },
            EnvironmentFile {
                value: "b.env".to_string(),
                r#type: "s3".to_string(),
            },
        ];
        let vars = load_environment_files(&files, dir.path()).unwrap();
        // Both FOO entries are present; the caller decides override order
        assert_eq!(vars.len(), 4);
        assert_eq!(vars[0].0, "FOO");
        assert_eq!(vars[0].1, "first");
        assert_eq!(vars[2].0, "FOO");
        assert_eq!(vars[2].1, "second");
    }

    // --- ulimits tests ---

    #[test]
    fn parse_ulimits_field() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "ulimits": [
                    { "name": "nofile", "softLimit": 1024, "hardLimit": 4096 },
                    { "name": "memlock", "softLimit": -1, "hardLimit": -1 }
                ]
            }]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert_eq!(td.container_definitions[0].ulimits.len(), 2);
        assert_eq!(td.container_definitions[0].ulimits[0].name, "nofile");
        assert_eq!(td.container_definitions[0].ulimits[0].soft_limit, 1024);
        assert_eq!(td.container_definitions[0].ulimits[0].hard_limit, 4096);
        assert_eq!(td.container_definitions[0].ulimits[1].name, "memlock");
        assert_eq!(td.container_definitions[0].ulimits[1].soft_limit, -1);
    }

    #[test]
    fn parse_ulimits_defaults_to_empty() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [{ "name": "app", "image": "nginx:latest" }]
        }"#;
        let td = TaskDefinition::from_json(json).unwrap();
        assert!(td.container_definitions[0].ulimits.is_empty());
    }

    #[test]
    fn strip_quotes_removes_matching_quotes() {
        assert_eq!(strip_quotes("\"hello\""), "hello");
        assert_eq!(strip_quotes("'hello'"), "hello");
        assert_eq!(strip_quotes("hello"), "hello");
        assert_eq!(strip_quotes("\"mismatched'"), "\"mismatched'");
        assert_eq!(strip_quotes("\"\""), "");
    }
}
