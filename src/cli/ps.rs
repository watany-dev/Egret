use std::fmt::Write;

use anyhow::Result;
use serde::Serialize;

use super::{OutputFormat, PsArgs};
use crate::container::{ContainerClient, ContainerInfo, ContainerRuntime, PortInfo};

/// Execute the `ps` subcommand.
#[cfg(not(tarpaulin_include))]
pub async fn execute(args: &PsArgs, host: Option<&str>) -> Result<()> {
    let client = ContainerClient::connect(host).await?;
    execute_with_client(args, &client).await
}

/// List Lecs-managed containers (testable with mock).
#[allow(clippy::print_stdout)]
pub async fn execute_with_client(
    args: &PsArgs,
    client: &(impl ContainerRuntime + ?Sized),
) -> Result<()> {
    let task_filter = args.task.as_deref();
    let containers = client.list_containers(task_filter).await?;

    if containers.is_empty() {
        println!("No lecs containers found.");
        return Ok(());
    }

    match args.output {
        OutputFormat::Json => println!("{}", format_json(&containers)),
        OutputFormat::Table => {
            println!("{}", format_table(&containers));
        }
    }

    Ok(())
}

/// Serializable container info for JSON output.
#[derive(Serialize)]
struct ContainerJsonView {
    name: String,
    image: String,
    status: String,
    task: String,
    health: Option<String>,
    ports: Vec<String>,
    uptime: Option<String>,
}

/// Format container list as JSON.
fn format_json(containers: &[ContainerInfo]) -> String {
    let views: Vec<ContainerJsonView> = containers
        .iter()
        .map(|c| ContainerJsonView {
            name: c.name.clone(),
            image: c.image.clone(),
            status: c.state.clone(),
            task: c.family.clone(),
            health: c.health_status.clone(),
            ports: c.ports.iter().map(format_port).collect(),
            uptime: c.started_at.as_deref().map(format_uptime),
        })
        .collect();
    // serde_json::to_string_pretty should not fail for valid data
    serde_json::to_string_pretty(&views).unwrap_or_else(|_| "[]".to_string())
}

/// Format container list as a table string with aligned columns.
fn format_table(containers: &[ContainerInfo]) -> String {
    let headers = [
        "NAME", "IMAGE", "STATUS", "HEALTH", "PORTS", "UPTIME", "TASK",
    ];

    let health_values: Vec<String> = containers
        .iter()
        .map(|c| c.health_status.as_deref().unwrap_or("-").to_string())
        .collect();
    let ports_values: Vec<String> = containers.iter().map(|c| format_ports(&c.ports)).collect();
    let uptime_values: Vec<String> = containers
        .iter()
        .map(|c| {
            c.started_at
                .as_deref()
                .map_or_else(|| "-".to_string(), format_uptime)
        })
        .collect();

    // Calculate column widths
    let name_w = col_width(containers.iter().map(|c| c.name.len()), headers[0].len());
    let image_w = col_width(containers.iter().map(|c| c.image.len()), headers[1].len());
    let status_w = col_width(containers.iter().map(|c| c.state.len()), headers[2].len());
    let health_w = col_width(health_values.iter().map(String::len), headers[3].len());
    let ports_w = col_width(ports_values.iter().map(String::len), headers[4].len());
    let uptime_w = col_width(uptime_values.iter().map(String::len), headers[5].len());

    let mut output = String::new();

    // Header
    let _ = writeln!(
        output,
        "{:<name_w$}  {:<image_w$}  {:<status_w$}  {:<health_w$}  {:<ports_w$}  {:<uptime_w$}  {}",
        headers[0], headers[1], headers[2], headers[3], headers[4], headers[5], headers[6],
    );

    // Rows
    for (i, c) in containers.iter().enumerate() {
        let _ = writeln!(
            output,
            "{:<name_w$}  {:<image_w$}  {:<status_w$}  {:<health_w$}  {:<ports_w$}  {:<uptime_w$}  {}",
            c.name, c.image, c.state, health_values[i], ports_values[i], uptime_values[i], c.family,
        );
    }

    // Remove trailing newline
    if output.ends_with('\n') {
        output.pop();
    }

    output
}

/// Calculate column width: max(data widths, header width).
fn col_width(data_widths: impl Iterator<Item = usize>, header_width: usize) -> usize {
    data_widths.max().unwrap_or(0).max(header_width)
}

/// Format a single port mapping for display.
fn format_port(port: &PortInfo) -> String {
    port.host_port.map_or_else(
        || format!("{}/{}", port.container_port, port.protocol),
        |host_port| format!("{host_port}->{}/{}", port.container_port, port.protocol),
    )
}

/// Format a list of port mappings as comma-separated string.
fn format_ports(ports: &[PortInfo]) -> String {
    if ports.is_empty() {
        return "-".to_string();
    }
    ports.iter().map(format_port).collect::<Vec<_>>().join(", ")
}

/// Format an ISO 8601 timestamp as human-readable uptime.
fn format_uptime(started_at: &str) -> String {
    let Ok(started) = chrono::DateTime::parse_from_rfc3339(started_at) else {
        return "-".to_string();
    };
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(started);

    let total_secs = duration.num_seconds();
    if total_secs < 0 {
        return "-".to_string();
    }

    format_duration_secs(total_secs)
}

/// Format a duration in seconds as a human-readable string.
fn format_duration_secs(total_secs: i64) -> String {
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins}m")
    } else if mins > 0 {
        format!("{mins}m{secs}s")
    } else {
        format!("{secs}s")
    }
}

/// Format bytes as human-readable size.
#[allow(clippy::cast_precision_loss)]
pub fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;

    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
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

    #[allow(clippy::too_many_arguments)]
    fn container_info_full(
        name: &str,
        image: &str,
        state: &str,
        family: &str,
        health: Option<&str>,
        ports: Vec<PortInfo>,
    ) -> ContainerInfo {
        ContainerInfo {
            id: format!("{name}-id"),
            name: name.to_string(),
            image: image.to_string(),
            family: family.to_string(),
            state: state.to_string(),
            health_status: health.map(String::from),
            ports,
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
        assert!(lines[0].contains("NAME"));
        assert!(lines[0].contains("HEALTH"));
        assert!(lines[0].contains("PORTS"));
        assert!(lines[0].contains("UPTIME"));
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
        assert!(lines[0].contains("NAME"));
        assert!(lines[1].starts_with("my-app-web"));
        assert!(lines[2].starts_with("my-app-sidecar"));
    }

    #[test]
    fn format_table_with_health_and_ports() {
        let containers = vec![container_info_full(
            "web",
            "nginx:latest",
            "running",
            "my-app",
            Some("healthy"),
            vec![PortInfo {
                host_port: Some(8080),
                container_port: 80,
                protocol: "tcp".to_string(),
            }],
        )];
        let table = format_table(&containers);
        assert!(table.contains("healthy"));
        assert!(table.contains("8080->80/tcp"));
    }

    #[test]
    fn format_table_no_health_shows_dash() {
        let containers = vec![container_info("web", "nginx:latest", "running", "my-app")];
        let table = format_table(&containers);
        // The health column should show "-"
        let lines: Vec<&str> = table.lines().collect();
        assert!(lines[1].contains('-'));
    }

    #[test]
    fn format_json_output() {
        let containers = vec![container_info_full(
            "web",
            "nginx:latest",
            "running",
            "my-app",
            Some("healthy"),
            vec![PortInfo {
                host_port: Some(8080),
                container_port: 80,
                protocol: "tcp".to_string(),
            }],
        )];
        let json = format_json(&containers);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let arr = parsed.as_array().expect("should be array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "web");
        assert_eq!(arr[0]["status"], "running");
        assert_eq!(arr[0]["health"], "healthy");
        let ports = arr[0]["ports"].as_array().expect("ports array");
        assert_eq!(ports[0], "8080->80/tcp");
    }

    #[test]
    fn format_port_with_host_port() {
        let port = PortInfo {
            host_port: Some(8080),
            container_port: 80,
            protocol: "tcp".to_string(),
        };
        assert_eq!(format_port(&port), "8080->80/tcp");
    }

    #[test]
    fn format_port_without_host_port() {
        let port = PortInfo {
            host_port: None,
            container_port: 80,
            protocol: "tcp".to_string(),
        };
        assert_eq!(format_port(&port), "80/tcp");
    }

    #[test]
    fn format_ports_empty() {
        assert_eq!(format_ports(&[]), "-");
    }

    #[test]
    fn format_ports_multiple() {
        let ports = vec![
            PortInfo {
                host_port: Some(8080),
                container_port: 80,
                protocol: "tcp".to_string(),
            },
            PortInfo {
                host_port: Some(3000),
                container_port: 3000,
                protocol: "tcp".to_string(),
            },
        ];
        assert_eq!(format_ports(&ports), "8080->80/tcp, 3000->3000/tcp");
    }

    #[test]
    fn format_duration_secs_seconds() {
        assert_eq!(format_duration_secs(30), "30s");
    }

    #[test]
    fn format_duration_secs_minutes() {
        assert_eq!(format_duration_secs(150), "2m30s");
    }

    #[test]
    fn format_duration_secs_hours() {
        assert_eq!(format_duration_secs(3700), "1h1m");
    }

    #[test]
    fn format_duration_secs_days() {
        assert_eq!(format_duration_secs(90000), "1d1h");
    }

    #[test]
    fn format_bytes_values() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1_572_864), "1.5 MiB");
        assert_eq!(format_bytes(1_610_612_736), "1.5 GiB");
    }

    #[tokio::test]
    async fn ps_no_containers() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![])])),
            ..MockContainerClient::new()
        };

        let args = PsArgs {
            task: None,
            output: OutputFormat::Table,
        };
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

        let args = PsArgs {
            task: None,
            output: OutputFormat::Table,
        };
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
            output: OutputFormat::Table,
        };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[test]
    fn format_uptime_invalid_timestamp() {
        assert_eq!(format_uptime("not-a-timestamp"), "-");
    }

    #[test]
    fn format_uptime_future_timestamp() {
        // A future timestamp should produce negative duration → "-"
        assert_eq!(format_uptime("2099-01-01T00:00:00Z"), "-");
    }

    #[tokio::test]
    async fn ps_json_output() {
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
            task: None,
            output: OutputFormat::Json,
        };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }
}
