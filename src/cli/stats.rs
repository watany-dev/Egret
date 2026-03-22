use std::fmt::Write;

use anyhow::Result;

use super::StatsArgs;
use crate::cli::ps::format_bytes;
use crate::container::{ContainerClient, ContainerInfo, ContainerRuntime, ContainerStats};

/// Execute the `stats` subcommand.
#[cfg(not(tarpaulin_include))]
pub async fn execute(args: &StatsArgs, host: Option<&str>) -> Result<()> {
    let client = ContainerClient::connect(host).await?;
    execute_with_client(args, &client).await
}

/// Show resource usage statistics (testable with mock).
#[allow(clippy::print_stdout)]
pub async fn execute_with_client(
    args: &StatsArgs,
    client: &(impl ContainerRuntime + ?Sized),
) -> Result<()> {
    let task_filter = args.family.as_deref();
    let containers = client.list_containers(task_filter).await?;

    if containers.is_empty() {
        println!("No egret containers found.");
        return Ok(());
    }

    // Collect stats for each container
    let mut stats_data: Vec<(&ContainerInfo, Option<ContainerStats>)> = Vec::new();
    for container in &containers {
        let stats = client.stats_container(&container.id).await.ok();
        stats_data.push((container, stats));
    }

    println!("{}", format_stats_table(&stats_data));

    Ok(())
}

/// Format a stats value, returning "N/A" if stats are unavailable.
fn format_stat<F: Fn(&ContainerStats) -> String>(stats: Option<&ContainerStats>, f: F) -> String {
    stats.map_or_else(|| "N/A".to_string(), f)
}

/// Format resource usage as a table.
pub fn format_stats_table(data: &[(&ContainerInfo, Option<ContainerStats>)]) -> String {
    let headers = ["NAME", "CPU %", "MEM USAGE / LIMIT", "NET I/O", "BLOCK I/O"];

    let cpu_values: Vec<String> = data
        .iter()
        .map(|(_, s)| format_stat(s.as_ref(), |s| format!("{:.2}%", s.cpu_percent)))
        .collect();
    let mem_values: Vec<String> = data
        .iter()
        .map(|(_, s)| {
            format_stat(s.as_ref(), |s| {
                format!(
                    "{} / {}",
                    format_bytes(s.memory_usage),
                    format_bytes(s.memory_limit)
                )
            })
        })
        .collect();
    let net_values: Vec<String> = data
        .iter()
        .map(|(_, s)| {
            format_stat(s.as_ref(), |s| {
                format!(
                    "{} / {}",
                    format_bytes(s.net_rx_bytes),
                    format_bytes(s.net_tx_bytes)
                )
            })
        })
        .collect();
    let block_values: Vec<String> = data
        .iter()
        .map(|(_, s)| {
            format_stat(s.as_ref(), |s| {
                format!(
                    "{} / {}",
                    format_bytes(s.block_read_bytes),
                    format_bytes(s.block_write_bytes)
                )
            })
        })
        .collect();

    let name_w = col_width(data.iter().map(|(c, _)| c.name.len()), headers[0].len());
    let cpu_w = col_width(cpu_values.iter().map(String::len), headers[1].len());
    let mem_w = col_width(mem_values.iter().map(String::len), headers[2].len());
    let net_w = col_width(net_values.iter().map(String::len), headers[3].len());

    let mut output = String::new();
    let _ = writeln!(
        output,
        "{:<name_w$}  {:<cpu_w$}  {:<mem_w$}  {:<net_w$}  {}",
        headers[0], headers[1], headers[2], headers[3], headers[4],
    );

    for (i, (container, _)) in data.iter().enumerate() {
        let _ = writeln!(
            output,
            "{:<name_w$}  {:<cpu_w$}  {:<mem_w$}  {:<net_w$}  {}",
            container.name, cpu_values[i], mem_values[i], net_values[i], block_values[i],
        );
    }

    if output.ends_with('\n') {
        output.pop();
    }
    output
}

/// Calculate column width.
fn col_width(data_widths: impl Iterator<Item = usize>, header_width: usize) -> usize {
    data_widths.max().unwrap_or(0).max(header_width)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::container::test_support::MockContainerClient;

    fn container_info(name: &str) -> ContainerInfo {
        ContainerInfo {
            id: format!("{name}-id"),
            name: name.to_string(),
            image: "nginx:latest".to_string(),
            family: "my-app".to_string(),
            state: "running".to_string(),
            health_status: None,
            ports: vec![],
            started_at: None,
        }
    }

    fn sample_stats() -> ContainerStats {
        ContainerStats {
            cpu_percent: 12.5,
            memory_usage: 52_428_800,     // 50 MiB
            memory_limit: 536_870_912,    // 512 MiB
            net_rx_bytes: 1_048_576,      // 1 MiB
            net_tx_bytes: 524_288,        // 512 KiB
            block_read_bytes: 2_097_152,  // 2 MiB
            block_write_bytes: 1_048_576, // 1 MiB
        }
    }

    #[tokio::test]
    async fn stats_no_containers() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![])])),
            ..MockContainerClient::new()
        };

        let args = StatsArgs { family: None };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[tokio::test]
    async fn stats_with_containers() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![container_info("web")])])),
            stats_container_results: Mutex::new(VecDeque::from([Ok(sample_stats())])),
            ..MockContainerClient::new()
        };

        let args = StatsArgs { family: None };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[tokio::test]
    async fn stats_with_family_filter() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![container_info("web")])])),
            stats_container_results: Mutex::new(VecDeque::from([Ok(sample_stats())])),
            ..MockContainerClient::new()
        };

        let args = StatsArgs {
            family: Some("my-app".to_string()),
        };
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[tokio::test]
    async fn stats_container_error_shows_na() {
        let mock = MockContainerClient {
            list_containers_results: Mutex::new(VecDeque::from([Ok(vec![container_info("web")])])),
            // stats_container_results is empty, so it returns RuntimeNotRunning
            ..MockContainerClient::new()
        };

        let args = StatsArgs { family: None };
        // Should succeed even when stats fail — shows "N/A"
        execute_with_client(&args, &mock)
            .await
            .expect("should succeed");
    }

    #[test]
    fn format_stats_table_with_data() {
        let info = container_info("web");
        let stats = sample_stats();
        let data = vec![(&info, Some(stats))];
        let table = format_stats_table(&data);

        assert!(table.contains("NAME"));
        assert!(table.contains("CPU %"));
        assert!(table.contains("MEM USAGE / LIMIT"));
        assert!(table.contains("NET I/O"));
        assert!(table.contains("BLOCK I/O"));
        assert!(table.contains("12.50%"));
        assert!(table.contains("50.0 MiB"));
        assert!(table.contains("512.0 MiB"));
    }

    #[test]
    fn format_stats_table_na_when_none() {
        let info = container_info("web");
        let data = vec![(&info, None)];
        let table = format_stats_table(&data);

        assert!(table.contains("N/A"));
    }
}
