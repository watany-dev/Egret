use std::collections::HashSet;
use std::fmt::Write;

use anyhow::{Result, bail};

use super::InspectArgs;
use crate::container::{ContainerClient, ContainerRuntime};

/// Execute the `inspect` subcommand.
#[cfg(not(tarpaulin_include))]
pub async fn execute(args: &InspectArgs, host: Option<&str>) -> Result<()> {
    let client = ContainerClient::connect(host).await?;
    execute_with_client(args, &client).await
}

/// Inspect Lecs-managed containers for a given task family (testable with mock).
#[allow(clippy::print_stdout)]
pub async fn execute_with_client(
    args: &InspectArgs,
    client: &(impl ContainerRuntime + ?Sized),
) -> Result<()> {
    let containers = client.list_containers(Some(&args.task)).await?;

    if containers.is_empty() {
        bail!("No running containers found for task '{}'.", args.task);
    }

    let mut output = String::new();
    let _ = writeln!(output, "Task: {}", args.task);
    let _ = writeln!(output, "Containers: {}", containers.len());
    let _ = writeln!(output);

    for container in &containers {
        let inspection = client.inspect_container(&container.id).await?;

        let _ = writeln!(output, "--- {} ---", container.name);
        let _ = writeln!(output, "  ID:     {}", container.id);
        let _ = writeln!(output, "  Image:  {}", inspection.image);
        let _ = writeln!(output, "  Status: {}", inspection.state.status);

        if let Some(health) = &inspection.state.health_status {
            let _ = writeln!(output, "  Health: {health}");
        }

        if let Some(started) = &inspection.started_at {
            let _ = writeln!(output, "  Started: {started}");
        }

        if let Some(network) = &inspection.network_name {
            let _ = writeln!(output, "  Network: {network}");
        }

        // Display ports
        if !inspection.ports.is_empty() {
            let ports_str: Vec<String> = inspection
                .ports
                .iter()
                .map(|p| {
                    p.host_port.map_or_else(
                        || format!("{}/{}", p.container_port, p.protocol),
                        |hp| format!("{hp}->{}/{}", p.container_port, p.protocol),
                    )
                })
                .collect();
            let _ = writeln!(output, "  Ports:  {}", ports_str.join(", "));
        }

        // Display environment variables with secret masking
        if !inspection.env.is_empty() {
            let secret_names = parse_secret_names(
                inspection
                    .labels
                    .get(crate::labels::SECRETS)
                    .map(String::as_str),
            );
            let _ = writeln!(output, "  Environment:");
            for env_var in &inspection.env {
                let masked = mask_env_var(env_var, &secret_names);
                let _ = writeln!(output, "    {masked}");
            }
        }

        let _ = writeln!(output);
    }

    // Remove trailing newlines
    let trimmed = output.trim_end();
    println!("{trimmed}");

    Ok(())
}

/// Extract secret names from a comma-separated label value.
fn parse_secret_names(label_value: Option<&str>) -> HashSet<String> {
    label_value
        .filter(|v| !v.is_empty())
        .map(|v| v.split(',').map(String::from).collect())
        .unwrap_or_default()
}

/// Environment variable names that must always be masked, regardless of
/// whether the container declares them as secrets. These are values injected
/// by Lecs itself that grant access to sensitive sidecar endpoints.
const ALWAYS_MASK_ENV_NAMES: &[&str] = &["AWS_CONTAINER_AUTHORIZATION_TOKEN"];

/// Mask the value of an environment variable if its name is in the secret set
/// or in the always-mask list.
fn mask_env_var(env_var: &str, secret_names: &HashSet<String>) -> String {
    env_var.split_once('=').map_or_else(
        || env_var.to_string(),
        |(name, _value)| {
            if secret_names.contains(name) || ALWAYS_MASK_ENV_NAMES.contains(&name) {
                format!("{name}=******")
            } else {
                env_var.to_string()
            }
        },
    )
}

/// Format an inspect output for a set of containers (pure function for testing).
#[allow(dead_code)]
pub fn format_inspect_env(env: &[String], secret_names: &HashSet<String>) -> Vec<String> {
    env.iter().map(|e| mask_env_var(e, secret_names)).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::container::test_support::MockContainerClient;
    use crate::container::{ContainerInfo, ContainerInspection, ContainerState, PortInfo};

    fn make_container_info(name: &str, family: &str) -> ContainerInfo {
        ContainerInfo {
            id: format!("{name}-id"),
            name: name.to_string(),
            image: "nginx:latest".to_string(),
            family: family.to_string(),
            state: "running".to_string(),
            health_status: Some("healthy".to_string()),
            ports: vec![PortInfo {
                host_port: Some(8080),
                container_port: 80,
                protocol: "tcp".to_string(),
            }],
            started_at: None,
        }
    }

    fn make_inspection(id: &str) -> ContainerInspection {
        ContainerInspection {
            id: id.into(),
            state: ContainerState {
                status: "running".into(),
                running: true,
                exit_code: None,
                health_status: Some("healthy".into()),
            },
            image: "nginx:latest".into(),
            env: vec!["PORT=8080".into(), "DB_PASSWORD=secret123".into()],
            network_name: Some("lecs-my-app".into()),
            ports: vec![PortInfo {
                host_port: Some(8080),
                container_port: 80,
                protocol: "tcp".to_string(),
            }],
            started_at: Some("2025-01-15T10:30:00Z".into()),
            labels: std::collections::HashMap::new(),
        }
    }

    #[tokio::test]
    async fn inspect_no_containers_fails() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![])])),
            ..MockContainerClient::new()
        };

        let args = InspectArgs {
            task: "my-app".to_string(),
        };
        let result = execute_with_client(&args, &mock).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No running containers")
        );
    }

    #[tokio::test]
    async fn inspect_shows_container_details() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![make_container_info(
                "web", "my-app",
            )])])),
            inspect_container_results: Mutex::new(VecDeque::from([Ok(make_inspection("web-id"))])),
            ..MockContainerClient::new()
        };

        let args = InspectArgs {
            task: "my-app".to_string(),
        };
        // Should succeed without error
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[tokio::test]
    async fn inspect_multiple_containers() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![
                make_container_info("web", "my-app"),
                make_container_info("db", "my-app"),
            ])])),
            inspect_container_results: Mutex::new(VecDeque::from([
                Ok(make_inspection("web-id")),
                Ok(make_inspection("db-id")),
            ])),
            ..MockContainerClient::new()
        };

        let args = InspectArgs {
            task: "my-app".to_string(),
        };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[test]
    fn mask_env_var_secret() {
        let secret_names: HashSet<String> = std::iter::once("DB_PASSWORD".to_string()).collect();
        assert_eq!(
            mask_env_var("DB_PASSWORD=secret123", &secret_names),
            "DB_PASSWORD=******"
        );
    }

    #[test]
    fn mask_env_var_non_secret() {
        let secret_names: HashSet<String> = HashSet::new();
        assert_eq!(mask_env_var("PORT=8080", &secret_names), "PORT=8080");
    }

    #[test]
    fn mask_env_var_no_equals() {
        let secret_names: HashSet<String> = HashSet::new();
        assert_eq!(mask_env_var("PATH", &secret_names), "PATH");
    }

    #[test]
    fn mask_env_var_always_masks_auth_token() {
        let secret_names: HashSet<String> = HashSet::new();
        assert_eq!(
            mask_env_var("AWS_CONTAINER_AUTHORIZATION_TOKEN=deadbeef", &secret_names),
            "AWS_CONTAINER_AUTHORIZATION_TOKEN=******"
        );
    }

    #[test]
    fn mask_env_var_always_mask_independent_of_secrets_label() {
        // Even if the container did not declare secrets, the auth token is masked.
        let secret_names: HashSet<String> = std::iter::once("OTHER_SECRET".to_string()).collect();
        assert_eq!(
            mask_env_var("AWS_CONTAINER_AUTHORIZATION_TOKEN=abc123", &secret_names),
            "AWS_CONTAINER_AUTHORIZATION_TOKEN=******"
        );
    }

    #[test]
    fn parse_secret_names_from_label() {
        let names = parse_secret_names(Some("DB_PASSWORD,SECRET_KEY"));
        assert_eq!(names.len(), 2);
        assert!(names.contains("DB_PASSWORD"));
        assert!(names.contains("SECRET_KEY"));
    }

    #[test]
    fn parse_secret_names_empty() {
        assert!(parse_secret_names(None).is_empty());
        assert!(parse_secret_names(Some("")).is_empty());
    }

    #[tokio::test]
    async fn inspect_with_secret_masking() {
        let mut inspection = make_inspection("web-id");
        inspection
            .labels
            .insert(crate::labels::SECRETS.into(), "DB_PASSWORD".into());

        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![make_container_info(
                "web", "my-app",
            )])])),
            inspect_container_results: Mutex::new(VecDeque::from([Ok(inspection)])),
            ..MockContainerClient::new()
        };

        let args = InspectArgs {
            task: "my-app".to_string(),
        };
        // Should succeed — secret masking happens internally
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[test]
    fn format_inspect_env_masks_secrets() {
        let env = vec!["PORT=8080".into(), "SECRET_KEY=abc123".into()];
        let secrets: HashSet<String> = std::iter::once("SECRET_KEY".to_string()).collect();
        let result = format_inspect_env(&env, &secrets);
        assert_eq!(result[0], "PORT=8080");
        assert_eq!(result[1], "SECRET_KEY=******");
    }
}
