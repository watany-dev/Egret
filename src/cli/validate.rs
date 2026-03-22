//! `egret validate` command implementation.

use anyhow::{Context, Result};

use crate::overrides::OverrideConfig;
use crate::secrets::SecretsResolver;
use crate::taskdef::TaskDefinition;
use crate::taskdef::diagnostics::{self, Severity, ValidationDiagnostic, ValidationReport};

use super::ValidateArgs;

/// Execute the `validate` subcommand.
#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout)]
pub fn execute(args: &ValidateArgs) -> Result<()> {
    let task_json = std::fs::read_to_string(&args.task_definition)?;
    let override_json = args
        .r#override
        .as_ref()
        .map(std::fs::read_to_string)
        .transpose()?;
    let secrets_json = args
        .secrets
        .as_ref()
        .map(std::fs::read_to_string)
        .transpose()?;

    execute_from_json(
        &task_json,
        override_json.as_deref(),
        secrets_json.as_deref(),
    )
}

/// Core validation logic (testable without filesystem).
#[allow(clippy::print_stdout)]
pub fn execute_from_json(
    task_json: &str,
    override_json: Option<&str>,
    secrets_json: Option<&str>,
) -> Result<()> {
    // Parse the task definition
    let task_def = TaskDefinition::from_json(task_json)
        .context("validation failed: could not parse task definition")?;

    // Run extended validation
    let mut report = diagnostics::validate_extended(&task_def);

    // Cross-validate overrides if provided
    if let Some(json) = override_json {
        let overrides = OverrideConfig::from_json(json)
            .context("validation failed: could not parse override file")?;
        let diags = diagnostics::validate_overrides(&task_def, &overrides);
        report.diagnostics.extend(diags);
    }

    // Cross-validate secrets if provided
    if let Some(json) = secrets_json {
        let resolver = SecretsResolver::from_json(json)
            .context("validation failed: could not parse secrets file")?;
        let diags = validate_secrets_coverage(&task_def, &resolver);
        report.diagnostics.extend(diags);
    }

    print_report(&report);

    if report.has_errors() {
        anyhow::bail!("validation failed with {} error(s)", report.error_count());
    }

    Ok(())
}

/// Check that all secret ARNs in the task definition exist in the secrets mapping.
fn validate_secrets_coverage(
    task_def: &TaskDefinition,
    resolver: &SecretsResolver,
) -> Vec<ValidationDiagnostic> {
    let mut diagnostics = Vec::new();

    for (ci, container) in task_def.container_definitions.iter().enumerate() {
        for (si, secret) in container.secrets.iter().enumerate() {
            if resolver.resolve(std::slice::from_ref(secret)).is_err() {
                diagnostics.push(ValidationDiagnostic {
                    severity: Severity::Error,
                    field_path: format!("containerDefinitions[{ci}].secrets[{si}].valueFrom"),
                    message: format!(
                        "secret ARN '{}' not found in secrets mapping",
                        secret.value_from
                    ),
                    suggestion: Some("add this ARN to your secrets.local.json file".to_string()),
                });
            }
        }
    }

    diagnostics
}

/// Print the validation report to stdout.
#[allow(clippy::print_stdout)]
fn print_report(report: &ValidationReport) {
    if report.diagnostics.is_empty() {
        println!("Validation passed.");
        return;
    }

    println!("{report}");

    if !report.has_errors() {
        println!("Validation passed (with warnings).");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn minimal_valid_json() -> &'static str {
        r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "portMappings": [{ "containerPort": 80, "hostPort": 8080 }]
                }
            ]
        }"#
    }

    #[test]
    fn valid_task_definition_passes() {
        let result = execute_from_json(minimal_valid_json(), None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn invalid_json_fails() {
        let result = execute_from_json("not json", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_image_format_detected() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": ".invalid image",
                    "portMappings": [{ "containerPort": 80, "hostPort": 8080 }]
                }
            ]
        }"#;
        let result = execute_from_json(json, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn port_conflict_detected() {
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "a",
                    "image": "nginx:latest",
                    "portMappings": [
                        { "containerPort": 80, "hostPort": 8080 },
                        { "containerPort": 81, "hostPort": 8080 }
                    ]
                }
            ]
        }"#;
        let result = execute_from_json(json, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn override_unknown_container_error() {
        let override_json = r#"{
            "containerOverrides": {
                "nonexistent": { "image": "alpine:3.18" }
            }
        }"#;
        let result = execute_from_json(minimal_valid_json(), Some(override_json), None);
        assert!(result.is_err());
    }

    #[test]
    fn warnings_do_not_fail() {
        // Task with all essential=false and no ports — produces warnings only
        let json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "worker",
                    "image": "alpine:latest",
                    "essential": false
                }
            ]
        }"#;
        let result = execute_from_json(json, None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn secrets_arn_validated() {
        let task_json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "secrets": [
                        { "name": "DB_PASS", "valueFrom": "arn:aws:secretsmanager:us-east-1:123:secret:my-secret" }
                    ]
                }
            ]
        }"#;
        let secrets_json = r#"{
            "arn:aws:secretsmanager:us-east-1:123:secret:other-secret": "value"
        }"#;
        let result = execute_from_json(task_json, None, Some(secrets_json));
        assert!(result.is_err());
    }

    #[test]
    fn secrets_all_covered_passes() {
        let task_json = r#"{
            "family": "test",
            "containerDefinitions": [
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "secrets": [
                        { "name": "DB_PASS", "valueFrom": "arn:aws:secretsmanager:us-east-1:123:secret:my-secret" }
                    ],
                    "portMappings": [{ "containerPort": 80 }]
                }
            ]
        }"#;
        let secrets_json = r#"{
            "arn:aws:secretsmanager:us-east-1:123:secret:my-secret": "local-value"
        }"#;
        let result = execute_from_json(task_json, None, Some(secrets_json));
        assert!(result.is_ok());
    }
}
