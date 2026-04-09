//! `CloudFormation` template parser for ECS task definitions.
//!
//! Parses `CloudFormation` template JSON (including CDK synthesized output)
//! and extracts `AWS::ECS::TaskDefinition` resources, converting them
//! into [`TaskDefinition`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use super::{TaskDefError, TaskDefinition};

/// Resource type identifier for ECS task definitions in `CloudFormation`.
const CFN_ECS_TASK_DEF_TYPE: &str = "AWS::ECS::TaskDefinition";

/// Maximum `CloudFormation` template file size (10 MB).
const MAX_CFN_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Known `CloudFormation` intrinsic function keys.
const INTRINSIC_FUNCTION_KEYS: &[&str] = &[
    "Ref",
    "Fn::Sub",
    "Fn::Join",
    "Fn::GetAtt",
    "Fn::ImportValue",
    "Fn::Select",
    "Fn::Split",
    "Fn::If",
    "Fn::Base64",
    "Fn::Cidr",
    "Fn::FindInMap",
    "Fn::GetAZs",
    "Fn::Transform",
];

// ── CloudFormation JSON schema types ────────────────────────────────

/// Top-level structure of a `CloudFormation` template.
#[derive(Debug, Deserialize)]
struct CfnTemplate {
    /// `CloudFormation` resources.
    #[serde(rename = "Resources")]
    resources: Option<HashMap<String, CfnResource>>,
}

/// A `CloudFormation` resource.
#[derive(Debug, Deserialize)]
struct CfnResource {
    /// Resource type (e.g. `AWS::ECS::TaskDefinition`).
    #[serde(rename = "Type")]
    resource_type: String,
    /// Resource properties as raw JSON (`PascalCase` keys).
    #[serde(rename = "Properties")]
    properties: Option<Value>,
}

// ── Public API ───────────────────────────────────────────────────────

/// Parse a `CloudFormation` template file and extract an ECS task definition.
///
/// Automatically detects the format based on file extension:
/// - `.yaml` / `.yml` → YAML parser
/// - `.json` or other → JSON parser (with YAML fallback)
///
/// If the template contains multiple `AWS::ECS::TaskDefinition` resources,
/// `resource_id` must be provided to select one by logical ID.
pub fn from_cfn_file(
    path: &Path,
    resource_id: Option<&str>,
) -> Result<TaskDefinition, TaskDefError> {
    let metadata = std::fs::metadata(path).map_err(|source| TaskDefError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > MAX_CFN_FILE_SIZE {
        return Err(TaskDefError::FileTooLarge {
            path: path.to_path_buf(),
            size: metadata.len(),
            max: MAX_CFN_FILE_SIZE,
        });
    }
    let content = std::fs::read_to_string(path).map_err(|source| TaskDefError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "yaml" | "yml" => from_cfn_yaml(&content, resource_id),
        "json" => from_cfn_json(&content, resource_id),
        _ => {
            // Unknown extension: try JSON first, fall back to YAML.
            from_cfn_json(&content, resource_id).or_else(|_| from_cfn_yaml(&content, resource_id))
        }
    }
}

/// Parse a `CloudFormation` template YAML string and extract an ECS task definition.
///
/// YAML custom tags (e.g. `!Ref`, `!Sub`) are converted to their JSON-form
/// equivalents during deserialization, so the existing `detect_intrinsic_functions()`
/// logic catches them.
///
/// If the template contains multiple `AWS::ECS::TaskDefinition` resources,
/// `resource_id` must be provided to select one by logical ID.
pub fn from_cfn_yaml(
    yaml: &str,
    resource_id: Option<&str>,
) -> Result<TaskDefinition, TaskDefError> {
    // Deserialize YAML into a generic JSON Value first, then convert to CfnTemplate.
    // This approach handles YAML-specific types (anchors, aliases, tagged values) and
    // produces a serde_json::Value that fits the existing downstream pipeline.
    let value: Value =
        serde_yaml_ng::from_str(yaml).map_err(|e| TaskDefError::ParseCfnYaml(e.to_string()))?;

    let template: CfnTemplate =
        serde_json::from_value(value).map_err(|e| TaskDefError::ParseCfnYaml(e.to_string()))?;

    let resources = collect_ecs_resources(&template)?;
    let (logical_id, properties) = select_resource(&resources, resource_id)?;

    detect_intrinsic_functions(properties, logical_id)?;

    let camel_value = convert_keys_to_camel_case(properties.clone());

    let task_def: TaskDefinition = serde_json::from_value(camel_value).map_err(|e| {
        TaskDefError::ParseCfnYaml(format!(
            "failed to deserialize CloudFormation YAML properties for '{logical_id}': {e}"
        ))
    })?;

    task_def.validate()?;
    Ok(task_def)
}

/// Discover and parse ECS task definitions from a CDK output directory (`cdk.out/`).
///
/// Scans `*.template.json` files in the given directory, collects all
/// `AWS::ECS::TaskDefinition` resources, and selects one. When exactly one
/// resource exists across all templates it is returned automatically; otherwise
/// `resource_id` must narrow the selection.
pub fn discover_cdk_template(
    cdk_dir: &Path,
    resource_id: Option<&str>,
) -> Result<TaskDefinition, TaskDefError> {
    if !cdk_dir.is_dir() {
        return Err(TaskDefError::CdkDirectoryNotFound {
            path: cdk_dir.to_path_buf(),
        });
    }

    let template_files = list_cdk_template_files(cdk_dir)?;

    if template_files.is_empty() {
        return Err(TaskDefError::CdkNoTemplatesFound {
            path: cdk_dir.to_path_buf(),
        });
    }

    // Collect (template_path, logical_id, properties_json) for all ECS resources.
    let mut all_resources: Vec<(PathBuf, String, Value)> = Vec::new();

    for template_path in &template_files {
        let content =
            std::fs::read_to_string(template_path).map_err(|source| TaskDefError::ReadFile {
                path: template_path.clone(),
                source,
            })?;

        let template: CfnTemplate = match serde_json::from_str(&content) {
            Ok(t) => t,
            Err(_) => continue, // Skip non-parseable files.
        };

        if let Ok(resources) = collect_ecs_resources(&template) {
            for (logical_id, properties) in resources {
                all_resources.push((
                    template_path.clone(),
                    logical_id.to_string(),
                    properties.clone(),
                ));
            }
        }
    }

    if all_resources.is_empty() {
        return Err(TaskDefError::CdkNoEcsResourcesFound {
            path: cdk_dir.to_path_buf(),
        });
    }

    let (_, logical_id, properties) = select_cdk_resource(&all_resources, resource_id)?;

    detect_intrinsic_functions(&properties, &logical_id)?;

    let camel_value = convert_keys_to_camel_case(properties);

    let task_def: TaskDefinition = serde_json::from_value(camel_value).map_err(|e| {
        TaskDefError::ParseCfnJson(format!(
            "failed to deserialize CDK properties for '{logical_id}': {e}"
        ))
    })?;

    task_def.validate()?;
    Ok(task_def)
}

/// List all `*.template.json` files in a CDK output directory (sorted for deterministic order).
fn list_cdk_template_files(dir: &Path) -> Result<Vec<PathBuf>, TaskDefError> {
    let entries = std::fs::read_dir(dir).map_err(|source| TaskDefError::ReadFile {
        path: dir.to_path_buf(),
        source,
    })?;

    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".template.json"))
        })
        .collect();

    files.sort();
    Ok(files)
}

/// Select a single ECS resource from all CDK resources found across templates.
fn select_cdk_resource(
    resources: &[(PathBuf, String, Value)],
    resource_id: Option<&str>,
) -> Result<(PathBuf, String, Value), TaskDefError> {
    match resource_id {
        Some(id) => {
            for (path, logical_id, properties) in resources {
                if logical_id == id {
                    return Ok((path.clone(), logical_id.clone(), properties.clone()));
                }
            }
            let available: Vec<String> = resources.iter().map(|(_, id, _)| id.clone()).collect();
            Err(TaskDefError::CdkResourceNotFound {
                resource_id: id.to_string(),
                available,
            })
        }
        None => {
            if resources.len() == 1 {
                let (path, id, props) = &resources[0];
                Ok((path.clone(), id.clone(), props.clone()))
            } else {
                let mut candidates: Vec<String> =
                    resources.iter().map(|(_, id, _)| id.clone()).collect();
                candidates.sort();
                Err(TaskDefError::CdkMultipleResources { candidates })
            }
        }
    }
}

/// Parse a `CloudFormation` template JSON string and extract an ECS task definition.
///
/// If the template contains multiple `AWS::ECS::TaskDefinition` resources,
/// `resource_id` must be provided to select one by logical ID.
pub fn from_cfn_json(
    json: &str,
    resource_id: Option<&str>,
) -> Result<TaskDefinition, TaskDefError> {
    let template: CfnTemplate =
        serde_json::from_str(json).map_err(|e| TaskDefError::ParseCfnJson(e.to_string()))?;

    let resources = collect_ecs_resources(&template)?;

    let (logical_id, properties) = select_resource(&resources, resource_id)?;

    // Check for intrinsic functions before converting keys.
    detect_intrinsic_functions(properties, logical_id)?;

    // Convert PascalCase keys to camelCase for compatibility with existing types.
    let camel_value = convert_keys_to_camel_case(properties.clone());

    let task_def: TaskDefinition = serde_json::from_value(camel_value).map_err(|e| {
        TaskDefError::ParseCfnJson(format!(
            "failed to deserialize CloudFormation properties for '{logical_id}': {e}"
        ))
    })?;

    task_def.validate()?;
    Ok(task_def)
}

// ── Internal helpers ─────────────────────────────────────────────────

/// Collected ECS resource: (`logical_id`, properties).
type EcsResource<'a> = (&'a str, &'a Value);

/// Collect all `AWS::ECS::TaskDefinition` resources from the template.
fn collect_ecs_resources(template: &CfnTemplate) -> Result<Vec<EcsResource<'_>>, TaskDefError> {
    let resources = template
        .resources
        .as_ref()
        .ok_or(TaskDefError::CfnNoEcsResource)?;

    let ecs_resources: Vec<EcsResource<'_>> = resources
        .iter()
        .filter(|(_, r)| r.resource_type == CFN_ECS_TASK_DEF_TYPE)
        .filter_map(|(id, r)| r.properties.as_ref().map(|p| (id.as_str(), p)))
        .collect();

    if ecs_resources.is_empty() {
        return Err(TaskDefError::CfnNoEcsResource);
    }

    Ok(ecs_resources)
}

/// Select a single resource from the collected list.
fn select_resource<'a>(
    resources: &[EcsResource<'a>],
    resource_id: Option<&str>,
) -> Result<EcsResource<'a>, TaskDefError> {
    match resource_id {
        Some(id) => {
            for &(logical_id, properties) in resources {
                if logical_id == id {
                    return Ok((logical_id, properties));
                }
            }
            Err(TaskDefError::CfnResourceNotFound(id.to_string()))
        }
        None => {
            if resources.len() == 1 {
                Ok(resources[0])
            } else {
                let mut ids: Vec<String> =
                    resources.iter().map(|(id, _)| (*id).to_string()).collect();
                ids.sort();
                Err(TaskDefError::CfnMultipleResources { resources: ids })
            }
        }
    }
}

/// Recursively detect `CloudFormation` intrinsic functions in a JSON value.
///
/// This must be called **before** converting keys to camelCase to avoid
/// false positives from user data.
fn detect_intrinsic_functions(value: &Value, context: &str) -> Result<(), TaskDefError> {
    match value {
        Value::Object(map) => {
            // Detect intrinsics by object shape: a single-key map whose key is an intrinsic.
            // This avoids false positives from user-defined maps that happen to contain
            // keys like "Ref" alongside other entries.
            if map.len() == 1
                && let Some(key) = map.keys().next()
                && INTRINSIC_FUNCTION_KEYS.contains(&key.as_str())
            {
                return Err(TaskDefError::CfnIntrinsicFunction {
                    field: context.to_string(),
                    detail: format!(
                        "'{key}' is a CloudFormation intrinsic function and cannot be resolved in ECS task-definition properties. Provide a template where task-definition properties are concrete values (no CloudFormation intrinsics)."
                    ),
                });
            }
            for (key, v) in map {
                detect_intrinsic_functions(v, &format!("{context}.{key}"))?;
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                detect_intrinsic_functions(v, &format!("{context}[{i}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Convert a single `PascalCase` string to `camelCase`.
///
/// Only lowercases the first character. This is sufficient for all
/// ECS `TaskDefinition` fields (e.g. `ContainerPort` → `containerPort`).
fn pascal_to_camel(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| {
        let lower: String = c.to_lowercase().collect();
        lower + chars.as_str()
    })
}

/// Recursively convert all object keys from `PascalCase` to `camelCase`.
fn convert_keys_to_camel_case(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let converted = map
                .into_iter()
                .map(|(k, v)| (pascal_to_camel(&k), convert_keys_to_camel_case(v)))
                .collect();
            Value::Object(converted)
        }
        Value::Array(arr) => {
            Value::Array(arr.into_iter().map(convert_keys_to_camel_case).collect())
        }
        other => other,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn minimal_template() -> String {
        r#"{
            "AWSTemplateFormatVersion": "2010-09-09",
            "Resources": {
                "MyTaskDef": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "my-app",
                        "ContainerDefinitions": [
                            {
                                "Name": "app",
                                "Image": "nginx:latest",
                                "Essential": true
                            }
                        ]
                    }
                }
            }
        }"#
        .to_string()
    }

    fn full_template() -> String {
        r#"{
            "AWSTemplateFormatVersion": "2010-09-09",
            "Resources": {
                "MyTaskDef": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "my-app",
                        "TaskRoleArn": "arn:aws:iam::123456789012:role/task-role",
                        "ExecutionRoleArn": "arn:aws:iam::123456789012:role/exec-role",
                        "ContainerDefinitions": [
                            {
                                "Name": "app",
                                "Image": "nginx:latest",
                                "Essential": true,
                                "Command": ["nginx", "-g", "daemon off;"],
                                "EntryPoint": ["/docker-entrypoint.sh"],
                                "Environment": [
                                    { "Name": "ENV_VAR", "Value": "some-value" }
                                ],
                                "PortMappings": [
                                    { "ContainerPort": 80, "HostPort": 8080, "Protocol": "tcp" }
                                ],
                                "Cpu": 256,
                                "Memory": 512,
                                "MemoryReservation": 256
                            }
                        ]
                    }
                }
            }
        }"#
        .to_string()
    }

    #[test]
    fn parse_single_resource() {
        let td = from_cfn_json(&full_template(), None).unwrap();
        assert_eq!(td.family, "my-app");
        assert_eq!(td.container_definitions.len(), 1);
        let c = &td.container_definitions[0];
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
    fn parse_minimal_template() {
        let td = from_cfn_json(&minimal_template(), None).unwrap();
        assert_eq!(td.family, "my-app");
        assert_eq!(td.container_definitions.len(), 1);
        assert_eq!(td.container_definitions[0].name, "app");
        assert!(td.volumes.is_empty());
    }

    #[test]
    fn parse_with_volumes() {
        let json = r#"{
            "Resources": {
                "Task": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "vol-app",
                        "ContainerDefinitions": [
                            {
                                "Name": "app",
                                "Image": "nginx:latest",
                                "MountPoints": [
                                    {
                                        "SourceVolume": "data",
                                        "ContainerPath": "/data",
                                        "ReadOnly": false
                                    }
                                ]
                            }
                        ],
                        "Volumes": [
                            {
                                "Name": "data",
                                "Host": {
                                    "SourcePath": "/host/data"
                                }
                            }
                        ]
                    }
                }
            }
        }"#;
        let td = from_cfn_json(json, None).unwrap();
        assert_eq!(td.volumes.len(), 1);
        assert_eq!(td.volumes[0].name, "data");
        let host = td.volumes[0].host.as_ref().unwrap();
        assert_eq!(host.source_path, "/host/data");
        assert_eq!(td.container_definitions[0].mount_points.len(), 1);
        assert_eq!(
            td.container_definitions[0].mount_points[0].source_volume,
            "data"
        );
        assert_eq!(
            td.container_definitions[0].mount_points[0].container_path,
            "/data"
        );
        assert!(!td.container_definitions[0].mount_points[0].read_only);
    }

    #[test]
    fn parse_with_health_check() {
        let json = r#"{
            "Resources": {
                "Task": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "hc-app",
                        "ContainerDefinitions": [
                            {
                                "Name": "app",
                                "Image": "nginx:latest",
                                "HealthCheck": {
                                    "Command": ["CMD-SHELL", "curl -f http://localhost/"],
                                    "Interval": 30,
                                    "Timeout": 5,
                                    "Retries": 3,
                                    "StartPeriod": 10
                                }
                            }
                        ]
                    }
                }
            }
        }"#;
        let td = from_cfn_json(json, None).unwrap();
        let hc = td.container_definitions[0].health_check.as_ref().unwrap();
        assert_eq!(hc.command, vec!["CMD-SHELL", "curl -f http://localhost/"]);
        assert_eq!(hc.interval, 30);
        assert_eq!(hc.timeout, 5);
        assert_eq!(hc.retries, 3);
        assert_eq!(hc.start_period, 10);
    }

    #[test]
    fn parse_with_depends_on() {
        let json = r#"{
            "Resources": {
                "Task": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "dep-app",
                        "ContainerDefinitions": [
                            {
                                "Name": "db",
                                "Image": "postgres:15",
                                "Essential": true,
                                "HealthCheck": {
                                    "Command": ["CMD-SHELL", "pg_isready"]
                                }
                            },
                            {
                                "Name": "app",
                                "Image": "myapp:latest",
                                "Essential": true,
                                "DependsOn": [
                                    {
                                        "ContainerName": "db",
                                        "Condition": "HEALTHY"
                                    }
                                ]
                            }
                        ]
                    }
                }
            }
        }"#;
        let td = from_cfn_json(json, None).unwrap();
        assert_eq!(td.container_definitions.len(), 2);
        let app = &td.container_definitions[1];
        assert_eq!(app.depends_on.len(), 1);
        assert_eq!(app.depends_on[0].container_name, "db");
    }

    #[test]
    fn parse_with_secrets() {
        let json = r#"{
            "Resources": {
                "Task": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "sec-app",
                        "ContainerDefinitions": [
                            {
                                "Name": "app",
                                "Image": "myapp:latest",
                                "Secrets": [
                                    {
                                        "Name": "DB_PASSWORD",
                                        "ValueFrom": "arn:aws:secretsmanager:us-east-1:123456789012:secret:prod/db-pass"
                                    }
                                ]
                            }
                        ]
                    }
                }
            }
        }"#;
        let td = from_cfn_json(json, None).unwrap();
        assert_eq!(td.container_definitions[0].secrets.len(), 1);
        assert_eq!(td.container_definitions[0].secrets[0].name, "DB_PASSWORD");
        assert_eq!(
            td.container_definitions[0].secrets[0].value_from,
            "arn:aws:secretsmanager:us-east-1:123456789012:secret:prod/db-pass"
        );
    }

    #[test]
    fn parse_with_port_mappings() {
        let json = r#"{
            "Resources": {
                "Task": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "port-app",
                        "ContainerDefinitions": [
                            {
                                "Name": "app",
                                "Image": "myapp:latest",
                                "PortMappings": [
                                    { "ContainerPort": 8080 },
                                    { "ContainerPort": 443, "HostPort": 8443, "Protocol": "tcp" }
                                ]
                            }
                        ]
                    }
                }
            }
        }"#;
        let td = from_cfn_json(json, None).unwrap();
        let pm = &td.container_definitions[0].port_mappings;
        assert_eq!(pm.len(), 2);
        assert_eq!(pm[0].container_port, 8080);
        assert_eq!(pm[0].host_port, None);
        assert_eq!(pm[1].container_port, 443);
        assert_eq!(pm[1].host_port, Some(8443));
    }

    #[test]
    fn select_resource_by_id() {
        let json = r#"{
            "Resources": {
                "TaskA": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "app-a",
                        "ContainerDefinitions": [
                            { "Name": "a", "Image": "a:latest" }
                        ]
                    }
                },
                "TaskB": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "app-b",
                        "ContainerDefinitions": [
                            { "Name": "b", "Image": "b:latest" }
                        ]
                    }
                }
            }
        }"#;
        let td = from_cfn_json(json, Some("TaskB")).unwrap();
        assert_eq!(td.family, "app-b");
    }

    #[test]
    fn error_multiple_resources() {
        let json = r#"{
            "Resources": {
                "TaskA": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "app-a",
                        "ContainerDefinitions": [
                            { "Name": "a", "Image": "a:latest" }
                        ]
                    }
                },
                "TaskB": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "app-b",
                        "ContainerDefinitions": [
                            { "Name": "b", "Image": "b:latest" }
                        ]
                    }
                }
            }
        }"#;
        let err = from_cfn_json(json, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("multiple"));
        assert!(msg.contains("--cfn-resource"));
    }

    #[test]
    fn error_no_ecs_resource() {
        let json = r#"{
            "Resources": {
                "MyBucket": {
                    "Type": "AWS::S3::Bucket",
                    "Properties": {}
                }
            }
        }"#;
        let err = from_cfn_json(json, None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CfnNoEcsResource),
            "expected CfnNoEcsResource, got: {err}"
        );
    }

    #[test]
    fn error_resource_not_found() {
        let json = &minimal_template();
        let err = from_cfn_json(json, Some("NonExistent")).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CfnResourceNotFound(_)),
            "expected CfnResourceNotFound, got: {err}"
        );
    }

    #[test]
    fn error_intrinsic_ref() {
        let json = r#"{
            "Resources": {
                "Task": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "my-app",
                        "TaskRoleArn": { "Ref": "TaskRoleParam" },
                        "ContainerDefinitions": [
                            { "Name": "app", "Image": "nginx:latest" }
                        ]
                    }
                }
            }
        }"#;
        let err = from_cfn_json(json, None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CfnIntrinsicFunction { .. }),
            "expected CfnIntrinsicFunction, got: {err}"
        );
        assert!(err.to_string().contains("Ref"));
    }

    #[test]
    fn error_intrinsic_fn_sub() {
        let json = r#"{
            "Resources": {
                "Task": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "my-app",
                        "ContainerDefinitions": [
                            {
                                "Name": "app",
                                "Image": { "Fn::Sub": "${AWS::AccountId}.dkr.ecr.${AWS::Region}.amazonaws.com/myapp:latest" }
                            }
                        ]
                    }
                }
            }
        }"#;
        let err = from_cfn_json(json, None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CfnIntrinsicFunction { .. }),
            "expected CfnIntrinsicFunction, got: {err}"
        );
        assert!(err.to_string().contains("Fn::Sub"));
    }

    #[test]
    fn error_intrinsic_fn_join() {
        let json = r#"{
            "Resources": {
                "Task": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "my-app",
                        "ContainerDefinitions": [
                            {
                                "Name": "app",
                                "Image": "nginx:latest",
                                "Environment": [
                                    {
                                        "Name": "URL",
                                        "Value": { "Fn::Join": ["", ["https://", "example.com"]] }
                                    }
                                ]
                            }
                        ]
                    }
                }
            }
        }"#;
        let err = from_cfn_json(json, None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CfnIntrinsicFunction { .. }),
            "expected CfnIntrinsicFunction, got: {err}"
        );
    }

    #[test]
    fn error_empty_resources() {
        let json = r#"{ "Resources": {} }"#;
        let err = from_cfn_json(json, None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CfnNoEcsResource),
            "expected CfnNoEcsResource, got: {err}"
        );
    }

    #[test]
    fn error_invalid_json() {
        let err = from_cfn_json("not json", None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::ParseCfnJson(_)),
            "expected ParseCfnJson, got: {err}"
        );
    }

    #[test]
    fn error_file_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.json");
        // Write a file just over the limit.
        #[allow(clippy::cast_possible_truncation)]
        let content = "x".repeat((MAX_CFN_FILE_SIZE + 1) as usize);
        std::fs::write(&path, content).unwrap();
        let err = from_cfn_file(&path, None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::FileTooLarge { .. }),
            "expected FileTooLarge, got: {err}"
        );
    }

    #[test]
    fn pascal_to_camel_conversion() {
        assert_eq!(pascal_to_camel("ContainerPort"), "containerPort");
        assert_eq!(pascal_to_camel("Family"), "family");
        assert_eq!(pascal_to_camel("HealthCheck"), "healthCheck");
        assert_eq!(pascal_to_camel("SourcePath"), "sourcePath");
        assert_eq!(pascal_to_camel(""), "");
        assert_eq!(pascal_to_camel("a"), "a");
        assert_eq!(pascal_to_camel("A"), "a");
    }

    #[test]
    fn ignore_unknown_resource_types() {
        let json = r#"{
            "Resources": {
                "MyBucket": {
                    "Type": "AWS::S3::Bucket",
                    "Properties": {}
                },
                "MyTask": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "my-app",
                        "ContainerDefinitions": [
                            { "Name": "app", "Image": "nginx:latest" }
                        ]
                    }
                },
                "MyQueue": {
                    "Type": "AWS::SQS::Queue",
                    "Properties": {}
                }
            }
        }"#;
        let td = from_cfn_json(json, None).unwrap();
        assert_eq!(td.family, "my-app");
    }

    #[test]
    fn from_cfn_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("template.json");
        std::fs::write(&path, minimal_template()).unwrap();
        let td = from_cfn_file(&path, None).unwrap();
        assert_eq!(td.family, "my-app");
    }

    #[test]
    fn no_false_positive_intrinsic_in_multi_key_object() {
        // A user-defined map with "Ref" as one of multiple keys should NOT trigger
        // intrinsic detection (intrinsics are single-key objects).
        let json = r#"{
            "Resources": {
                "Task": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "my-app",
                        "ContainerDefinitions": [
                            {
                                "Name": "app",
                                "Image": "nginx:latest",
                                "DockerLabels": {
                                    "Ref": "some-label-value",
                                    "other": "data"
                                }
                            }
                        ]
                    }
                }
            }
        }"#;
        let td = from_cfn_json(json, None).unwrap();
        assert_eq!(td.family, "my-app");
    }

    #[test]
    fn error_multiple_resources_sorted() {
        let json = r#"{
            "Resources": {
                "ZetaTask": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "app-z",
                        "ContainerDefinitions": [
                            { "Name": "z", "Image": "z:latest" }
                        ]
                    }
                },
                "AlphaTask": {
                    "Type": "AWS::ECS::TaskDefinition",
                    "Properties": {
                        "Family": "app-a",
                        "ContainerDefinitions": [
                            { "Name": "a", "Image": "a:latest" }
                        ]
                    }
                }
            }
        }"#;
        let err = from_cfn_json(json, None).unwrap_err();
        let msg = err.to_string();
        // IDs should be sorted alphabetically regardless of HashMap order
        let alpha_pos = msg.find("AlphaTask").expect("should contain AlphaTask");
        let zeta_pos = msg.find("ZetaTask").expect("should contain ZetaTask");
        assert!(
            alpha_pos < zeta_pos,
            "AlphaTask should appear before ZetaTask in sorted output"
        );
    }

    #[test]
    fn error_missing_resources_key() {
        let json = r#"{ "AWSTemplateFormatVersion": "2010-09-09" }"#;
        let err = from_cfn_json(json, None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CfnNoEcsResource),
            "expected CfnNoEcsResource, got: {err}"
        );
    }

    // ── YAML tests ──────────────────────────────────────────────────

    fn minimal_yaml_template() -> String {
        r"
AWSTemplateFormatVersion: '2010-09-09'
Resources:
  MyTaskDef:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: my-app
      ContainerDefinitions:
        - Name: app
          Image: nginx:latest
          Essential: true
"
        .to_string()
    }

    #[test]
    fn yaml_parse_minimal() {
        let td = from_cfn_yaml(&minimal_yaml_template(), None).unwrap();
        assert_eq!(td.family, "my-app");
        assert_eq!(td.container_definitions.len(), 1);
        assert_eq!(td.container_definitions[0].name, "app");
        assert_eq!(td.container_definitions[0].image, "nginx:latest");
        assert!(td.container_definitions[0].essential);
    }

    #[test]
    fn yaml_parse_full_template() {
        let yaml = r#"
Resources:
  MyTaskDef:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: my-app
      TaskRoleArn: "arn:aws:iam::123456789012:role/task-role"
      ExecutionRoleArn: "arn:aws:iam::123456789012:role/exec-role"
      ContainerDefinitions:
        - Name: app
          Image: nginx:latest
          Essential: true
          Command:
            - nginx
            - "-g"
            - "daemon off;"
          EntryPoint:
            - /docker-entrypoint.sh
          Environment:
            - Name: ENV_VAR
              Value: some-value
          PortMappings:
            - ContainerPort: 80
              HostPort: 8080
              Protocol: tcp
          Cpu: 256
          Memory: 512
          MemoryReservation: 256
"#;
        let td = from_cfn_yaml(yaml, None).unwrap();
        assert_eq!(td.family, "my-app");
        let c = &td.container_definitions[0];
        assert_eq!(c.command, vec!["nginx", "-g", "daemon off;"]);
        assert_eq!(c.entry_point, vec!["/docker-entrypoint.sh"]);
        assert_eq!(c.environment.len(), 1);
        assert_eq!(c.port_mappings.len(), 1);
        assert_eq!(c.port_mappings[0].container_port, 80);
        assert_eq!(c.port_mappings[0].host_port, Some(8080));
        assert_eq!(c.cpu, Some(256));
        assert_eq!(c.memory, Some(512));
    }

    #[test]
    fn yaml_parse_with_volumes() {
        let yaml = r"
Resources:
  Task:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: vol-app
      ContainerDefinitions:
        - Name: app
          Image: nginx:latest
          MountPoints:
            - SourceVolume: data
              ContainerPath: /data
              ReadOnly: false
      Volumes:
        - Name: data
          Host:
            SourcePath: /host/data
";
        let td = from_cfn_yaml(yaml, None).unwrap();
        assert_eq!(td.volumes.len(), 1);
        assert_eq!(td.volumes[0].name, "data");
        let host = td.volumes[0].host.as_ref().unwrap();
        assert_eq!(host.source_path, "/host/data");
    }

    #[test]
    fn yaml_parse_with_depends_on() {
        let yaml = r"
Resources:
  Task:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: dep-app
      ContainerDefinitions:
        - Name: db
          Image: postgres:15
          Essential: true
          HealthCheck:
            Command:
              - CMD-SHELL
              - pg_isready
        - Name: app
          Image: myapp:latest
          Essential: true
          DependsOn:
            - ContainerName: db
              Condition: HEALTHY
";
        let td = from_cfn_yaml(yaml, None).unwrap();
        assert_eq!(td.container_definitions.len(), 2);
        let app = &td.container_definitions[1];
        assert_eq!(app.depends_on.len(), 1);
        assert_eq!(app.depends_on[0].container_name, "db");
    }

    #[test]
    fn yaml_select_resource_by_id() {
        let yaml = r"
Resources:
  TaskA:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: app-a
      ContainerDefinitions:
        - Name: a
          Image: a:latest
  TaskB:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: app-b
      ContainerDefinitions:
        - Name: b
          Image: b:latest
";
        let td = from_cfn_yaml(yaml, Some("TaskB")).unwrap();
        assert_eq!(td.family, "app-b");
    }

    #[test]
    fn yaml_error_invalid() {
        let err = from_cfn_yaml("invalid: yaml: [", None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::ParseCfnYaml(_)),
            "expected ParseCfnYaml, got: {err}"
        );
    }

    #[test]
    fn yaml_error_no_ecs_resource() {
        let yaml = r"
Resources:
  MyBucket:
    Type: AWS::S3::Bucket
    Properties: {}
";
        let err = from_cfn_yaml(yaml, None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CfnNoEcsResource),
            "expected CfnNoEcsResource, got: {err}"
        );
    }

    #[test]
    fn yaml_error_multiple_resources() {
        let yaml = r"
Resources:
  TaskA:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: app-a
      ContainerDefinitions:
        - Name: a
          Image: a:latest
  TaskB:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: app-b
      ContainerDefinitions:
        - Name: b
          Image: b:latest
";
        let err = from_cfn_yaml(yaml, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("multiple"));
    }

    #[test]
    fn yaml_file_extension_detection() {
        let dir = tempfile::tempdir().unwrap();

        // .yaml extension → YAML parser
        let yaml_path = dir.path().join("template.yaml");
        std::fs::write(&yaml_path, minimal_yaml_template()).unwrap();
        let td = from_cfn_file(&yaml_path, None).unwrap();
        assert_eq!(td.family, "my-app");

        // .yml extension → YAML parser
        let yml_path = dir.path().join("template.yml");
        std::fs::write(&yml_path, minimal_yaml_template()).unwrap();
        let td = from_cfn_file(&yml_path, None).unwrap();
        assert_eq!(td.family, "my-app");

        // .json extension → JSON parser
        let json_path = dir.path().join("template.json");
        std::fs::write(&json_path, minimal_template()).unwrap();
        let td = from_cfn_file(&json_path, None).unwrap();
        assert_eq!(td.family, "my-app");
    }

    #[test]
    fn yaml_unknown_extension_fallback() {
        let dir = tempfile::tempdir().unwrap();

        // Unknown extension with valid YAML → should succeed via fallback
        let path = dir.path().join("template.cfn");
        std::fs::write(&path, minimal_yaml_template()).unwrap();
        let td = from_cfn_file(&path, None).unwrap();
        assert_eq!(td.family, "my-app");

        // Unknown extension with valid JSON → should succeed
        let path2 = dir.path().join("template.txt");
        std::fs::write(&path2, minimal_template()).unwrap();
        let td = from_cfn_file(&path2, None).unwrap();
        assert_eq!(td.family, "my-app");
    }

    #[test]
    fn yaml_parse_with_secrets() {
        let yaml = r#"
Resources:
  Task:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: sec-app
      ContainerDefinitions:
        - Name: app
          Image: myapp:latest
          Secrets:
            - Name: DB_PASSWORD
              ValueFrom: "arn:aws:secretsmanager:us-east-1:123456789012:secret:prod/db-pass"
"#;
        let td = from_cfn_yaml(yaml, None).unwrap();
        assert_eq!(td.container_definitions[0].secrets.len(), 1);
        assert_eq!(td.container_definitions[0].secrets[0].name, "DB_PASSWORD");
    }

    #[test]
    fn yaml_error_intrinsic_ref_tag() {
        // YAML custom tag !Ref cannot be deserialized into serde_json::Value,
        // so it triggers a ParseCfnYaml error during the initial parse step.
        let yaml = r"
Resources:
  Task:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: my-app
      TaskRoleArn: !Ref TaskRoleParam
      ContainerDefinitions:
        - Name: app
          Image: nginx:latest
";
        let err = from_cfn_yaml(yaml, None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::ParseCfnYaml(_)),
            "expected ParseCfnYaml, got: {err}"
        );
    }

    #[test]
    fn yaml_error_intrinsic_sub_tag() {
        let yaml = r#"
Resources:
  Task:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: my-app
      ContainerDefinitions:
        - Name: app
          Image: !Sub "${AWS::AccountId}.dkr.ecr.${AWS::Region}.amazonaws.com/myapp:latest"
"#;
        let err = from_cfn_yaml(yaml, None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::ParseCfnYaml(_)),
            "expected ParseCfnYaml, got: {err}"
        );
    }

    #[test]
    fn yaml_anchors_and_aliases() {
        // YAML anchors (&) and aliases (*) should be resolved by the parser.
        let yaml = r"
Resources:
  Task:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: anchor-app
      ContainerDefinitions:
        - &base_container
          Name: app
          Image: nginx:latest
          Essential: true
          Environment:
            - Name: ENV
              Value: prod
";
        let td = from_cfn_yaml(yaml, None).unwrap();
        assert_eq!(td.family, "anchor-app");
        assert_eq!(td.container_definitions[0].environment.len(), 1);
    }

    #[test]
    fn yaml_empty_template() {
        let err = from_cfn_yaml("", None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::ParseCfnYaml(_)),
            "expected ParseCfnYaml, got: {err}"
        );
    }

    #[test]
    fn yaml_json_equivalent_output() {
        // Verify that the same template in JSON and YAML produces identical TaskDefinition.
        let json_td = from_cfn_json(&minimal_template(), None).unwrap();
        let yaml_td = from_cfn_yaml(&minimal_yaml_template(), None).unwrap();

        assert_eq!(json_td.family, yaml_td.family);
        assert_eq!(
            json_td.container_definitions.len(),
            yaml_td.container_definitions.len()
        );
        assert_eq!(
            json_td.container_definitions[0].name,
            yaml_td.container_definitions[0].name
        );
        assert_eq!(
            json_td.container_definitions[0].image,
            yaml_td.container_definitions[0].image
        );
    }

    // ── CDK discovery tests ─────────────────────────────────────────

    fn cdk_template(family: &str, logical_id: &str) -> String {
        format!(
            r#"{{
                "Resources": {{
                    "{logical_id}": {{
                        "Type": "AWS::ECS::TaskDefinition",
                        "Properties": {{
                            "Family": "{family}",
                            "ContainerDefinitions": [
                                {{"Name": "app", "Image": "nginx:latest"}}
                            ]
                        }}
                    }}
                }}
            }}"#
        )
    }

    #[test]
    fn cdk_discover_single_template() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("MyStack.template.json"),
            cdk_template("cdk-app", "TaskDef"),
        )
        .unwrap();

        let td = discover_cdk_template(dir.path(), None).unwrap();
        assert_eq!(td.family, "cdk-app");
    }

    #[test]
    fn cdk_discover_multiple_templates_single_ecs() {
        let dir = tempfile::tempdir().unwrap();
        // Template with ECS resource
        std::fs::write(
            dir.path().join("AppStack.template.json"),
            cdk_template("cdk-app", "TaskDef"),
        )
        .unwrap();
        // Template without ECS resource
        std::fs::write(
            dir.path().join("InfraStack.template.json"),
            r#"{"Resources":{"Bucket":{"Type":"AWS::S3::Bucket","Properties":{}}}}"#,
        )
        .unwrap();

        let td = discover_cdk_template(dir.path(), None).unwrap();
        assert_eq!(td.family, "cdk-app");
    }

    #[test]
    fn cdk_discover_select_by_resource_id() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Stack1.template.json"),
            cdk_template("app-1", "TaskDefA"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Stack2.template.json"),
            cdk_template("app-2", "TaskDefB"),
        )
        .unwrap();

        let td = discover_cdk_template(dir.path(), Some("TaskDefB")).unwrap();
        assert_eq!(td.family, "app-2");
    }

    #[test]
    fn cdk_error_directory_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let bad_path = dir.path().join("nonexistent");
        let err = discover_cdk_template(&bad_path, None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CdkDirectoryNotFound { .. }),
            "expected CdkDirectoryNotFound, got: {err}"
        );
    }

    #[test]
    fn cdk_error_no_templates() {
        let dir = tempfile::tempdir().unwrap();
        // Empty directory
        let err = discover_cdk_template(dir.path(), None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CdkNoTemplatesFound { .. }),
            "expected CdkNoTemplatesFound, got: {err}"
        );
    }

    #[test]
    fn cdk_error_no_ecs_resources() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Stack.template.json"),
            r#"{"Resources":{"Bucket":{"Type":"AWS::S3::Bucket","Properties":{}}}}"#,
        )
        .unwrap();

        let err = discover_cdk_template(dir.path(), None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CdkNoEcsResourcesFound { .. }),
            "expected CdkNoEcsResourcesFound, got: {err}"
        );
    }

    #[test]
    fn cdk_error_multiple_resources_no_selection() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Stack1.template.json"),
            cdk_template("app-1", "TaskDefA"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Stack2.template.json"),
            cdk_template("app-2", "TaskDefB"),
        )
        .unwrap();

        let err = discover_cdk_template(dir.path(), None).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CdkMultipleResources { .. }),
            "expected CdkMultipleResources, got: {err}"
        );
        assert!(err.to_string().contains("--cdk-resource"));
    }

    #[test]
    fn cdk_error_resource_not_found() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Stack.template.json"),
            cdk_template("app", "TaskDef"),
        )
        .unwrap();

        let err = discover_cdk_template(dir.path(), Some("NonExistent")).unwrap_err();
        assert!(
            matches!(err, TaskDefError::CdkResourceNotFound { .. }),
            "expected CdkResourceNotFound, got: {err}"
        );
    }
}
