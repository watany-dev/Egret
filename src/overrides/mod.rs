//! Local override configuration.
//!
//! Applies local overrides (image tags, environment variables, port mappings)
//! to a parsed task definition without modifying the original file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::taskdef::{Environment, PortMapping, TaskDefinition};

/// Override configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum OverrideError {
    #[error("failed to read override file from {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse override JSON: {0}")]
    ParseJson(#[from] serde_json::Error),
}

/// Top-level override configuration (`egret-override.json`).
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverrideConfig {
    /// Per-container overrides keyed by container name.
    #[serde(default)]
    pub container_overrides: HashMap<String, ContainerOverride>,
}

/// Per-container override values.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerOverride {
    /// Replace the container image (including tag).
    pub image: Option<String>,

    /// Add or replace environment variables (key → value).
    pub environment: Option<HashMap<String, String>>,

    /// Replace all port mappings for this container.
    pub port_mappings: Option<Vec<PortMapping>>,
}

impl OverrideConfig {
    /// Load an override config from a file path.
    pub fn from_file(path: &Path) -> Result<Self, OverrideError> {
        let content = std::fs::read_to_string(path).map_err(|source| OverrideError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json(&content)
    }

    /// Parse an override config from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, OverrideError> {
        let config: Self = serde_json::from_str(json)?;
        Ok(config)
    }

    /// Apply overrides to a task definition in place.
    ///
    /// Unknown container names are logged as warnings and skipped.
    pub fn apply(&self, task_def: &mut TaskDefinition) {
        for (container_name, overrides) in &self.container_overrides {
            let Some(container) = task_def
                .container_definitions
                .iter_mut()
                .find(|c| c.name == *container_name)
            else {
                tracing::warn!(
                    container = %container_name,
                    "Override references unknown container, skipping"
                );
                continue;
            };

            // Image override
            if let Some(image) = &overrides.image {
                container.image.clone_from(image);
            }

            // Environment override (add or replace by key)
            if let Some(env_overrides) = &overrides.environment {
                for (key, value) in env_overrides {
                    if let Some(existing) =
                        container.environment.iter_mut().find(|e| e.name == *key)
                    {
                        existing.value.clone_from(value);
                    } else {
                        container.environment.push(Environment {
                            name: key.clone(),
                            value: value.clone(),
                        });
                    }
                }
            }

            // Port mappings override (full replacement)
            if let Some(port_mappings) = &overrides.port_mappings {
                container.port_mappings.clone_from(port_mappings);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::taskdef::{ContainerDefinition, Environment, PortMapping};

    fn sample_task_def() -> TaskDefinition {
        TaskDefinition {
            family: "test".to_string(),
            task_role_arn: None,
            execution_role_arn: None,
            volumes: vec![],
            container_definitions: vec![ContainerDefinition {
                name: "app".to_string(),
                image: "nginx:latest".to_string(),
                essential: true,
                command: vec![],
                entry_point: vec![],
                environment: vec![Environment {
                    name: "ENV_VAR".to_string(),
                    value: "original".to_string(),
                }],
                port_mappings: vec![PortMapping {
                    container_port: 80,
                    host_port: Some(8080),
                    protocol: "tcp".to_string(),
                }],
                secrets: vec![],
                cpu: None,
                memory: None,
                memory_reservation: None,
                depends_on: vec![],
                health_check: None,
                mount_points: vec![],
            }],
        }
    }

    #[test]
    fn parse_full_override() {
        let json = r#"{
            "containerOverrides": {
                "app": {
                    "image": "nginx:1.25-alpine",
                    "environment": {
                        "DEBUG": "true"
                    },
                    "portMappings": [
                        { "containerPort": 80, "hostPort": 9090 }
                    ]
                }
            }
        }"#;
        let config = OverrideConfig::from_json(json).expect("should parse");
        assert_eq!(config.container_overrides.len(), 1);
        let app = &config.container_overrides["app"];
        assert_eq!(app.image.as_deref(), Some("nginx:1.25-alpine"));
        assert!(app.environment.is_some());
        assert!(app.port_mappings.is_some());
    }

    #[test]
    fn parse_empty_override() {
        let json = r#"{ "containerOverrides": {} }"#;
        let config = OverrideConfig::from_json(json).expect("should parse");
        assert!(config.container_overrides.is_empty());
    }

    #[test]
    fn apply_replaces_image() {
        let json = r#"{
            "containerOverrides": {
                "app": { "image": "nginx:1.25-alpine" }
            }
        }"#;
        let config = OverrideConfig::from_json(json).expect("should parse");
        let mut task_def = sample_task_def();
        config.apply(&mut task_def);

        assert_eq!(task_def.container_definitions[0].image, "nginx:1.25-alpine");
    }

    #[test]
    fn apply_adds_new_env_var() {
        let json = r#"{
            "containerOverrides": {
                "app": { "environment": { "NEW_VAR": "new-value" } }
            }
        }"#;
        let config = OverrideConfig::from_json(json).expect("should parse");
        let mut task_def = sample_task_def();
        config.apply(&mut task_def);

        let env = &task_def.container_definitions[0].environment;
        assert_eq!(env.len(), 2);
        assert!(
            env.iter()
                .any(|e| e.name == "NEW_VAR" && e.value == "new-value")
        );
        // Original env var preserved
        assert!(
            env.iter()
                .any(|e| e.name == "ENV_VAR" && e.value == "original")
        );
    }

    #[test]
    fn apply_replaces_existing_env_var() {
        let json = r#"{
            "containerOverrides": {
                "app": { "environment": { "ENV_VAR": "overridden" } }
            }
        }"#;
        let config = OverrideConfig::from_json(json).expect("should parse");
        let mut task_def = sample_task_def();
        config.apply(&mut task_def);

        let env = &task_def.container_definitions[0].environment;
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].name, "ENV_VAR");
        assert_eq!(env[0].value, "overridden");
    }

    #[test]
    fn apply_replaces_port_mappings() {
        let json = r#"{
            "containerOverrides": {
                "app": {
                    "portMappings": [
                        { "containerPort": 8080, "hostPort": 9090, "protocol": "tcp" },
                        { "containerPort": 443 }
                    ]
                }
            }
        }"#;
        let config = OverrideConfig::from_json(json).expect("should parse");
        let mut task_def = sample_task_def();
        config.apply(&mut task_def);

        let ports = &task_def.container_definitions[0].port_mappings;
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].container_port, 8080);
        assert_eq!(ports[0].host_port, Some(9090));
        assert_eq!(ports[1].container_port, 443);
        assert_eq!(ports[1].host_port, None);
    }

    #[test]
    fn apply_unknown_container_skips() {
        let json = r#"{
            "containerOverrides": {
                "nonexistent": { "image": "foo:bar" }
            }
        }"#;
        let config = OverrideConfig::from_json(json).expect("should parse");
        let mut task_def = sample_task_def();
        let original_image = task_def.container_definitions[0].image.clone();
        config.apply(&mut task_def);

        // Original task def unchanged
        assert_eq!(task_def.container_definitions[0].image, original_image);
    }

    #[test]
    fn apply_no_mutation_when_empty() {
        let json = r#"{ "containerOverrides": {} }"#;
        let config = OverrideConfig::from_json(json).expect("should parse");
        let mut task_def = sample_task_def();
        let original_image = task_def.container_definitions[0].image.clone();
        config.apply(&mut task_def);

        assert_eq!(task_def.container_definitions[0].image, original_image);
    }

    #[test]
    fn error_invalid_json() {
        let err = OverrideConfig::from_json("not json").unwrap_err();
        assert!(matches!(err, OverrideError::ParseJson(_)));
    }

    #[test]
    fn error_file_not_found() {
        let err = OverrideConfig::from_file(Path::new("/nonexistent/override.json")).unwrap_err();
        assert!(
            matches!(err, OverrideError::ReadFile { .. }),
            "unexpected error: {err}"
        );
    }
}
