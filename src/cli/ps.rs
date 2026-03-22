use anyhow::Result;

use super::PsArgs;
use crate::container::{ContainerClient, ContainerInfo, ContainerRuntime};

/// Execute the `ps` subcommand.
#[cfg(not(tarpaulin_include))]
pub async fn execute(args: &PsArgs, host: Option<&str>) -> Result<()> {
    let client = ContainerClient::connect(host).await?;
    execute_with_client(args, &client).await
}

/// List Egret-managed containers (testable with mock).
#[allow(clippy::print_stdout)]
pub async fn execute_with_client(
    args: &PsArgs,
    client: &(impl ContainerRuntime + ?Sized),
) -> Result<()> {
    let task_filter = args.task.as_deref();
    let containers = client.list_containers(task_filter).await?;

    if containers.is_empty() {
        println!("No egret containers found.");
        return Ok(());
    }

    println!("{}", format_table(&containers));

    Ok(())
}

/// Format container list as a table string with aligned columns.
fn format_table(containers: &[ContainerInfo]) -> String {
    let headers = ["NAME", "IMAGE", "STATUS", "TASK"];

    // Calculate column widths
    let name_width = containers
        .iter()
        .map(|c| c.name.len())
        .max()
        .unwrap_or(0)
        .max(headers[0].len());
    let image_width = containers
        .iter()
        .map(|c| c.image.len())
        .max()
        .unwrap_or(0)
        .max(headers[1].len());
    let status_width = containers
        .iter()
        .map(|c| c.state.len())
        .max()
        .unwrap_or(0)
        .max(headers[2].len());

    let mut lines = Vec::new();

    // Header
    lines.push(format!(
        "{:<name_width$}  {:<image_width$}  {:<status_width$}  {}",
        headers[0], headers[1], headers[2], headers[3],
    ));

    // Rows
    for c in containers {
        lines.push(format!(
            "{:<name_width$}  {:<image_width$}  {:<status_width$}  {}",
            c.name, c.image, c.state, c.family,
        ));
    }

    lines.join("\n")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::container::test_support::MockContainerClient;

    fn container_info(name: &str, image: &str, state: &str, family: &str) -> ContainerInfo {
        ContainerInfo {
            id: format!("{name}-id"),
            name: name.to_string(),
            image: image.to_string(),
            family: family.to_string(),
            state: state.to_string(),
            health_status: None,
            ports: vec![],
            started_at: None,
        }
    }

    #[test]
    fn format_table_single_container() {
        let containers = vec![container_info(
            "my-app-web",
            "nginx:latest",
            "running",
            "my-app",
        )];
        let table = format_table(&containers);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("NAME"));
        assert!(lines[1].starts_with("my-app-web"));
        assert!(lines[1].contains("nginx:latest"));
        assert!(lines[1].contains("running"));
        assert!(lines[1].contains("my-app"));
    }

    #[test]
    fn format_table_multiple_containers() {
        let containers = vec![
            container_info("my-app-web", "nginx:latest", "running", "my-app"),
            container_info("my-app-sidecar", "redis:7", "running", "my-app"),
        ];
        let table = format_table(&containers);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3);
        // Columns should be aligned
        assert!(lines[0].starts_with("NAME"));
        assert!(lines[1].starts_with("my-app-web"));
        assert!(lines[2].starts_with("my-app-sidecar"));
    }

    #[tokio::test]
    async fn ps_no_containers() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![])])),
            ..MockContainerClient::new()
        };

        let args = PsArgs { task: None };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[tokio::test]
    async fn ps_with_containers() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![container_info(
                "web",
                "nginx:latest",
                "running",
                "my-app",
            )])])),
            ..MockContainerClient::new()
        };

        let args = PsArgs { task: None };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[tokio::test]
    async fn ps_with_task_filter() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![container_info(
                "web",
                "nginx:latest",
                "running",
                "my-app",
            )])])),
            ..MockContainerClient::new()
        };

        let args = PsArgs {
            task: Some("my-app".to_string()),
        };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }
}
