//! `egret init` command implementation.

use anyhow::Result;

use super::InitArgs;

/// Execute the `init` subcommand.
#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout)]
pub fn execute(args: &InitArgs) -> Result<()> {
    if !args.dir.exists() {
        anyhow::bail!(
            "directory '{}' does not exist; create it first",
            args.dir.display()
        );
    }

    let container_name = "app";
    let mut created = Vec::new();

    let files: Vec<(&str, String)> = vec![
        (
            "task-definition.json",
            generate_task_definition(&args.family, &args.image),
        ),
        (
            "egret-override.json",
            generate_override_template(container_name),
        ),
        ("secrets.local.json", generate_secrets_template()),
    ];

    for (filename, content) in &files {
        let path = args.dir.join(filename);
        if path.exists() {
            println!("skip: {filename} already exists");
        } else {
            std::fs::write(&path, content)?;
            println!("created: {filename}");
            created.push(*filename);
        }
    }

    if created.is_empty() {
        println!("\nAll files already exist. Nothing to do.");
    } else {
        println!("\nNext steps:");
        println!(
            "  egret validate -f {}/task-definition.json",
            args.dir.display()
        );
        println!(
            "  egret run -f {}/task-definition.json --override {}/egret-override.json",
            args.dir.display(),
            args.dir.display()
        );
    }

    Ok(())
}

/// Generate a task definition JSON template.
pub fn generate_task_definition(family: &str, image: &str) -> String {
    let value = serde_json::json!({
        "family": family,
        "containerDefinitions": [
            {
                "name": "app",
                "image": image,
                "essential": true,
                "portMappings": [
                    {
                        "containerPort": 80,
                        "hostPort": 8080,
                        "protocol": "tcp"
                    }
                ],
                "environment": [
                    {
                        "name": "ENV_VAR",
                        "value": "value"
                    }
                ]
            }
        ]
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// Generate an override JSON template.
pub fn generate_override_template(container_name: &str) -> String {
    let value = serde_json::json!({
        "containerOverrides": {
            (container_name): {
                "environment": {
                    "DEBUG": "true"
                }
            }
        }
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// Generate a secrets mapping JSON template.
pub fn generate_secrets_template() -> String {
    let value = serde_json::json!({
        "arn:aws:secretsmanager:us-east-1:123456789012:secret:example": "local-secret-value"
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::taskdef::TaskDefinition;

    use super::*;

    #[test]
    fn generate_task_definition_default() {
        let json = generate_task_definition("my-app", "nginx:latest");
        assert!(json.contains("\"family\": \"my-app\""));
        assert!(json.contains("\"image\": \"nginx:latest\""));
    }

    #[test]
    fn generate_task_definition_custom() {
        let json = generate_task_definition("custom-family", "alpine:3.18");
        assert!(json.contains("\"family\": \"custom-family\""));
        assert!(json.contains("\"image\": \"alpine:3.18\""));
    }

    #[test]
    fn generated_task_def_is_parseable() {
        let json = generate_task_definition("test-app", "nginx:latest");
        let result = TaskDefinition::from_json(&json);
        assert!(
            result.is_ok(),
            "generated JSON should be parseable: {result:?}"
        );
    }

    #[test]
    fn generate_override_template_valid() {
        let json = generate_override_template("app");
        let result: Result<serde_json::Value, _> = serde_json::from_str(&json);
        assert!(result.is_ok(), "override template should be valid JSON");
        let val = result.unwrap();
        assert!(val["containerOverrides"]["app"].is_object());
    }

    #[test]
    fn generate_secrets_template_valid() {
        let json = generate_secrets_template();
        let result: Result<serde_json::Value, _> = serde_json::from_str(&json);
        assert!(result.is_ok(), "secrets template should be valid JSON");
    }

    #[test]
    fn generate_task_definition_with_family_flag() {
        let json = generate_task_definition("web-service", "nginx:latest");
        assert!(json.contains("\"family\": \"web-service\""));
    }

    #[test]
    fn generate_task_definition_with_image_flag() {
        let json = generate_task_definition("my-app", "node:20-alpine");
        assert!(json.contains("\"image\": \"node:20-alpine\""));
    }

    #[test]
    fn generated_task_def_has_essential_container() {
        let json = generate_task_definition("test", "alpine:latest");
        let td = TaskDefinition::from_json(&json).unwrap();
        assert!(td.container_definitions[0].essential);
    }

    #[test]
    fn generated_task_def_has_port_mapping() {
        let json = generate_task_definition("test", "alpine:latest");
        let td = TaskDefinition::from_json(&json).unwrap();
        assert!(!td.container_definitions[0].port_mappings.is_empty());
    }

    #[test]
    fn generated_files_are_consistent() {
        let task_json = generate_task_definition("test", "alpine:latest");
        let td = TaskDefinition::from_json(&task_json).unwrap();
        let container_name = &td.container_definitions[0].name;

        let override_json = generate_override_template(container_name);
        let val: serde_json::Value = serde_json::from_str(&override_json).unwrap();
        assert!(
            val["containerOverrides"][container_name].is_object(),
            "override container name should match task def container name"
        );
    }
}
