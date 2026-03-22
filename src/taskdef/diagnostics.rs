//! Rich validation diagnostics for task definitions.
//!
//! Provides structured diagnostic types that collect all validation issues
//! (rather than fail-fast) with field paths, suggestions, and severity levels.

use std::collections::HashMap;
use std::fmt;

use super::TaskDefinition;

/// Severity level for validation diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Must be fixed before running.
    Error,
    /// Likely a mistake but not fatal.
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => write!(f, "error"),
            Self::Warning => write!(f, "warning"),
        }
    }
}

/// A structured validation diagnostic with context for human-friendly output.
#[derive(Debug, Clone)]
pub struct ValidationDiagnostic {
    /// Severity of this diagnostic.
    pub severity: Severity,
    /// JSON-like field path (e.g., `containerDefinitions[0].image`).
    pub field_path: String,
    /// Human-readable error message.
    pub message: String,
    /// Optional suggestion for how to fix the issue.
    pub suggestion: Option<String>,
}

impl fmt::Display for ValidationDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} - {}", self.severity, self.field_path, self.message)?;
        if let Some(suggestion) = &self.suggestion {
            write!(f, " (hint: {suggestion})")?;
        }
        Ok(())
    }
}

/// Result of comprehensive validation containing all diagnostics.
#[derive(Debug)]
pub struct ValidationReport {
    /// All collected diagnostics.
    pub diagnostics: Vec<ValidationDiagnostic>,
}

impl ValidationReport {
    /// Returns `true` if the report contains any error-level diagnostics.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Count of error-level diagnostics.
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }

    /// Count of warning-level diagnostics.
    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count()
    }
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for diagnostic in &self.diagnostics {
            writeln!(f, "{diagnostic}")?;
        }
        let errors = self.error_count();
        let warnings = self.warning_count();
        write!(
            f,
            "{errors} error(s), {warnings} warning(s)"
        )
    }
}

/// Run all extended validation checks on a parsed task definition.
///
/// Unlike `TaskDefinition::validate()` (fail-fast), this collects all issues.
pub fn validate_extended(task_def: &TaskDefinition) -> ValidationReport {
    let mut diagnostics = Vec::new();

    // Image format checks
    for (i, container) in task_def.container_definitions.iter().enumerate() {
        if let Some(d) = check_image_format(
            &container.image,
            &format!("containerDefinitions[{i}].image"),
        ) {
            diagnostics.push(d);
        }
    }

    // Port conflict checks
    diagnostics.extend(check_port_conflicts(task_def));

    ValidationReport { diagnostics }
}

/// Validate Docker image name format.
///
/// Rejects images with whitespace, leading/trailing `/` or `:`,
/// consecutive `//` or `::`, and names not starting with an alphanumeric character.
fn check_image_format(image: &str, field_path: &str) -> Option<ValidationDiagnostic> {
    if image.is_empty() {
        // Already caught by TaskDefinition::validate(), but include for completeness.
        return Some(ValidationDiagnostic {
            severity: Severity::Error,
            field_path: field_path.to_string(),
            message: "image name must not be empty".to_string(),
            suggestion: Some("specify a valid image like 'nginx:latest'".to_string()),
        });
    }

    if image.chars().any(|c| c.is_whitespace()) {
        return Some(ValidationDiagnostic {
            severity: Severity::Error,
            field_path: field_path.to_string(),
            message: format!("image name '{image}' contains whitespace"),
            suggestion: Some("remove whitespace from the image name".to_string()),
        });
    }

    if image.starts_with('/') || image.ends_with('/') {
        return Some(ValidationDiagnostic {
            severity: Severity::Error,
            field_path: field_path.to_string(),
            message: format!("image name '{image}' must not start or end with '/'"),
            suggestion: None,
        });
    }

    if image.starts_with(':') || image.ends_with(':') {
        return Some(ValidationDiagnostic {
            severity: Severity::Error,
            field_path: field_path.to_string(),
            message: format!("image name '{image}' must not start or end with ':'"),
            suggestion: None,
        });
    }

    if image.contains("//") || image.contains("::") {
        return Some(ValidationDiagnostic {
            severity: Severity::Error,
            field_path: field_path.to_string(),
            message: format!("image name '{image}' contains consecutive '//' or '::'"),
            suggestion: None,
        });
    }

    // Must start with an alphanumeric character.
    if let Some(first) = image.chars().next() {
        if !first.is_ascii_alphanumeric() {
            return Some(ValidationDiagnostic {
                severity: Severity::Error,
                field_path: field_path.to_string(),
                message: format!(
                    "image name '{image}' must start with an alphanumeric character"
                ),
                suggestion: None,
            });
        }
    }

    None
}

/// Detect host port conflicts across all containers.
///
/// Two port mappings conflict if they map to the same host port with the same protocol.
fn check_port_conflicts(task_def: &TaskDefinition) -> Vec<ValidationDiagnostic> {
    let mut diagnostics = Vec::new();
    // (host_port, protocol) -> (container_index, port_mapping_index, container_name)
    let mut seen: HashMap<(u16, String), (usize, usize, String)> = HashMap::new();

    for (ci, container) in task_def.container_definitions.iter().enumerate() {
        for (pi, pm) in container.port_mappings.iter().enumerate() {
            let host_port = pm.host_port.unwrap_or(pm.container_port);
            let key = (host_port, pm.protocol.clone());

            if let Some((prev_ci, prev_pi, prev_name)) = seen.get(&key) {
                diagnostics.push(ValidationDiagnostic {
                    severity: Severity::Error,
                    field_path: format!(
                        "containerDefinitions[{ci}].portMappings[{pi}]"
                    ),
                    message: format!(
                        "host port {host_port}/{} conflicts with containerDefinitions[{prev_ci}].portMappings[{prev_pi}] (container '{prev_name}')",
                        pm.protocol
                    ),
                    suggestion: Some("use a different host port".to_string()),
                });
            } else {
                seen.insert(key, (ci, pi, container.name.clone()));
            }
        }
    }

    diagnostics
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn severity_display() {
        assert_eq!(Severity::Error.to_string(), "error");
        assert_eq!(Severity::Warning.to_string(), "warning");
    }

    #[test]
    fn diagnostic_display_without_suggestion() {
        let d = ValidationDiagnostic {
            severity: Severity::Error,
            field_path: "containerDefinitions[0].image".to_string(),
            message: "image name contains whitespace".to_string(),
            suggestion: None,
        };
        assert_eq!(
            d.to_string(),
            "error: containerDefinitions[0].image - image name contains whitespace"
        );
    }

    #[test]
    fn diagnostic_display_with_suggestion() {
        let d = ValidationDiagnostic {
            severity: Severity::Warning,
            field_path: "containerDefinitions[0].essential".to_string(),
            message: "all containers have essential=false".to_string(),
            suggestion: Some("set at least one container as essential".to_string()),
        };
        assert_eq!(
            d.to_string(),
            "warning: containerDefinitions[0].essential - all containers have essential=false (hint: set at least one container as essential)"
        );
    }

    #[test]
    fn report_counts_and_has_errors() {
        let report = ValidationReport {
            diagnostics: vec![
                ValidationDiagnostic {
                    severity: Severity::Error,
                    field_path: "family".to_string(),
                    message: "empty".to_string(),
                    suggestion: None,
                },
                ValidationDiagnostic {
                    severity: Severity::Warning,
                    field_path: "containerDefinitions".to_string(),
                    message: "no port mappings".to_string(),
                    suggestion: None,
                },
                ValidationDiagnostic {
                    severity: Severity::Error,
                    field_path: "containerDefinitions[0].image".to_string(),
                    message: "invalid".to_string(),
                    suggestion: None,
                },
            ],
        };
        assert!(report.has_errors());
        assert_eq!(report.error_count(), 2);
        assert_eq!(report.warning_count(), 1);
    }

    #[test]
    fn report_no_errors() {
        let report = ValidationReport {
            diagnostics: vec![ValidationDiagnostic {
                severity: Severity::Warning,
                field_path: "containerDefinitions".to_string(),
                message: "no port mappings".to_string(),
                suggestion: None,
            }],
        };
        assert!(!report.has_errors());
        assert_eq!(report.error_count(), 0);
        assert_eq!(report.warning_count(), 1);
    }

    #[test]
    fn report_display() {
        let report = ValidationReport {
            diagnostics: vec![
                ValidationDiagnostic {
                    severity: Severity::Error,
                    field_path: "family".to_string(),
                    message: "empty".to_string(),
                    suggestion: None,
                },
                ValidationDiagnostic {
                    severity: Severity::Warning,
                    field_path: "ports".to_string(),
                    message: "none".to_string(),
                    suggestion: Some("add port mappings".to_string()),
                },
            ],
        };
        let output = report.to_string();
        assert!(output.contains("error: family - empty"));
        assert!(output.contains("warning: ports - none (hint: add port mappings)"));
        assert!(output.contains("1 error(s), 1 warning(s)"));
    }

    #[test]
    fn empty_report() {
        let report = ValidationReport {
            diagnostics: vec![],
        };
        assert!(!report.has_errors());
        assert_eq!(report.error_count(), 0);
        assert_eq!(report.warning_count(), 0);
        assert!(report.to_string().contains("0 error(s), 0 warning(s)"));
    }

    // --- Image format validation tests ---

    #[test]
    fn image_format_valid_simple() {
        assert!(check_image_format("nginx", "test").is_none());
    }

    #[test]
    fn image_format_valid_with_tag() {
        assert!(check_image_format("nginx:latest", "test").is_none());
    }

    #[test]
    fn image_format_valid_registry() {
        assert!(check_image_format("myregistry.io/app:v1.2.3", "test").is_none());
    }

    #[test]
    fn image_format_valid_multi_level() {
        assert!(check_image_format("registry.example.com/org/app:latest", "test").is_none());
    }

    #[test]
    fn image_format_valid_sha256() {
        assert!(check_image_format("nginx@sha256:abc123def", "test").is_none());
    }

    #[test]
    fn image_format_invalid_whitespace() {
        let d = check_image_format("nginx latest", "containerDefinitions[0].image").unwrap();
        assert_eq!(d.severity, Severity::Error);
        assert!(d.message.contains("whitespace"));
    }

    #[test]
    fn image_format_invalid_leading_slash() {
        let d = check_image_format("/nginx", "test").unwrap();
        assert_eq!(d.severity, Severity::Error);
        assert!(d.message.contains("start or end with '/'"));
    }

    #[test]
    fn image_format_invalid_trailing_colon() {
        let d = check_image_format("nginx:", "test").unwrap();
        assert_eq!(d.severity, Severity::Error);
        assert!(d.message.contains("start or end with ':'"));
    }

    #[test]
    fn image_format_invalid_consecutive_slashes() {
        let d = check_image_format("registry//image", "test").unwrap();
        assert_eq!(d.severity, Severity::Error);
        assert!(d.message.contains("consecutive"));
    }

    #[test]
    fn image_format_invalid_starts_nonalpha() {
        let d = check_image_format(".hidden", "test").unwrap();
        assert_eq!(d.severity, Severity::Error);
        assert!(d.message.contains("alphanumeric"));
    }

    // --- Port conflict tests ---

    fn make_task_def(json: &str) -> TaskDefinition {
        TaskDefinition::from_json(json).expect("test task def should parse")
    }

    #[test]
    fn port_conflicts_none() {
        let td = make_task_def(r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "a",
                    "image": "alpine:latest",
                    "portMappings": [
                        { "containerPort": 80, "hostPort": 8080 }
                    ]
                },
                {
                    "name": "b",
                    "image": "alpine:latest",
                    "portMappings": [
                        { "containerPort": 80, "hostPort": 8081 }
                    ]
                }
            ]
        }"#);
        assert!(check_port_conflicts(&td).is_empty());
    }

    #[test]
    fn port_conflicts_same_host_port_different_protocol_ok() {
        let td = make_task_def(r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "a",
                    "image": "alpine:latest",
                    "portMappings": [
                        { "containerPort": 80, "hostPort": 8080, "protocol": "tcp" },
                        { "containerPort": 80, "hostPort": 8080, "protocol": "udp" }
                    ]
                }
            ]
        }"#);
        assert!(check_port_conflicts(&td).is_empty());
    }

    #[test]
    fn port_conflicts_same_host_port_same_protocol() {
        let td = make_task_def(r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "a",
                    "image": "alpine:latest",
                    "portMappings": [
                        { "containerPort": 80, "hostPort": 8080 },
                        { "containerPort": 81, "hostPort": 8080 }
                    ]
                }
            ]
        }"#);
        let conflicts = check_port_conflicts(&td);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].severity, Severity::Error);
        assert!(conflicts[0].message.contains("8080/tcp"));
    }

    #[test]
    fn port_conflicts_across_containers() {
        let td = make_task_def(r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "web",
                    "image": "nginx:latest",
                    "portMappings": [
                        { "containerPort": 80, "hostPort": 8080 }
                    ]
                },
                {
                    "name": "api",
                    "image": "node:latest",
                    "portMappings": [
                        { "containerPort": 3000, "hostPort": 8080 }
                    ]
                }
            ]
        }"#);
        let conflicts = check_port_conflicts(&td);
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].message.contains("web"));
    }

    // --- validate_extended integration tests ---

    #[test]
    fn validate_extended_valid_task() {
        let td = make_task_def(r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "portMappings": [{ "containerPort": 80, "hostPort": 8080 }]
                }
            ]
        }"#);
        let report = validate_extended(&td);
        assert!(!report.has_errors());
    }

    #[test]
    fn validate_extended_catches_image_and_port_issues() {
        let td = make_task_def(r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "a",
                    "image": ".invalid-image",
                    "portMappings": [
                        { "containerPort": 80, "hostPort": 8080 },
                        { "containerPort": 81, "hostPort": 8080 }
                    ]
                }
            ]
        }"#);
        let report = validate_extended(&td);
        assert_eq!(report.error_count(), 2);
    }

    #[test]
    fn port_conflicts_host_port_defaults_to_container_port() {
        // When hostPort is omitted, it defaults to containerPort.
        // Two containers both mapping containerPort=80 without hostPort should conflict.
        let td = make_task_def(r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "a",
                    "image": "alpine:latest",
                    "portMappings": [{ "containerPort": 80 }]
                },
                {
                    "name": "b",
                    "image": "alpine:latest",
                    "portMappings": [{ "containerPort": 80 }]
                }
            ]
        }"#);
        let conflicts = check_port_conflicts(&td);
        assert_eq!(conflicts.len(), 1);
    }
}
