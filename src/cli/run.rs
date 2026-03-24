use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use futures_util::StreamExt;
use tokio::task::JoinHandle;

use super::RunArgs;
use super::task_lifecycle::{cleanup, run_task, start_metadata_server};
use crate::container::ContainerClient;
use crate::events::{EventSink, NullEventSink};
use crate::metadata;
use crate::taskdef::{ContainerDefinition, TaskDefinition};

/// Execute the `run` subcommand.
#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout, clippy::too_many_lines)]
pub async fn execute(args: &RunArgs, host: Option<&str>) -> Result<()> {
    let task_def = args.source.load_task_def()?;
    tracing::info!(family = %task_def.family, containers = task_def.container_definitions.len(), "Parsed task definition");

    // Dry-run: display resolved configuration and exit
    if args.dry_run {
        let secret_names: HashSet<String> = task_def
            .container_definitions
            .iter()
            .flat_map(|c| c.secrets.iter().map(|s| s.name.clone()))
            .collect();
        let output = display_dry_run(&task_def, &secret_names);
        println!("{output}");
        return Ok(());
    }

    let client = Arc::new(ContainerClient::connect(host).await?);

    // Start metadata server if enabled
    let (metadata_server, metadata_state) = if args.no_metadata {
        (None, None)
    } else {
        match start_metadata_server(&task_def).await {
            Ok((server, state)) => (Some(server), Some(state)),
            Err(e) => {
                return Err(e);
            }
        }
    };

    let metadata_port = metadata_server.as_ref().map(|s| s.port);
    let auth_token = if let Some(state) = &metadata_state {
        Some(state.read().await.auth_token.clone())
    } else {
        None
    };
    let event_sink: Box<dyn EventSink> = if args.events {
        Box::new(crate::events::NdjsonEventSink)
    } else {
        Box::new(NullEventSink)
    };
    let (network_name, containers) = run_task(
        &*client,
        &task_def,
        metadata_port,
        auth_token.as_deref(),
        &*event_sink,
    )
    .await?;

    // Update container IDs in metadata server state
    if let Some(state) = &metadata_state {
        for (id, name) in &containers {
            metadata::update_container_id(state, name, id).await;
        }
    }

    stream_logs_until_signal(&client, &containers).await;

    // Shutdown metadata server
    if let Some(server) = metadata_server {
        server.shutdown().await;
    }

    cleanup(
        &*client,
        &containers,
        &network_name,
        &*event_sink,
        &task_def.family,
    )
    .await;

    Ok(())
}

/// ANSI color codes for log multiplexing (12 distinct colors).
const COLORS: &[&str] = &[
    "32", "33", "34", "35", "36", "91", "92", "93", "94", "95", "96", "31",
];

/// Return the ANSI color code for a container at the given index.
fn container_color(index: usize) -> &'static str {
    COLORS[index % COLORS.len()]
}

/// Format a log line with ANSI color-coded container prefix.
fn format_log_line(name: &str, line: &str, color: &str) -> String {
    format!("\x1b[{color}m[{name}]\x1b[0m {line}")
}

#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout)]
async fn stream_logs_until_signal(client: &Arc<ContainerClient>, containers: &[(String, String)]) {
    let mut handles: Vec<JoinHandle<()>> = Vec::new();

    for (i, (id, name)) in containers.iter().enumerate() {
        let client = Arc::clone(client);
        let id = id.clone();
        let name = name.clone();
        let color = container_color(i).to_string();

        handles.push(tokio::spawn(async move {
            let mut stream = client.stream_logs(&id, true);
            while let Some(result) = stream.next().await {
                match result {
                    Ok(line) => println!("{}", format_log_line(&name, &line, &color)),
                    Err(e) => {
                        tracing::warn!(container = %name, error = %e, "Log stream error");
                        break;
                    }
                }
            }
        }));
    }

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("Received SIGINT, shutting down...");

    for handle in &handles {
        handle.abort();
    }
}

/// Format resolved configuration for dry-run output.
fn display_dry_run(task_def: &TaskDefinition, secret_names: &HashSet<String>) -> String {
    use std::fmt::Write;

    let mut output = String::new();
    let _ = writeln!(
        output,
        "Dry-run: {} (no containers will be started)",
        task_def.family
    );
    let _ = writeln!(output, "Network: lecs-{}", task_def.family);

    for (i, container) in task_def.container_definitions.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }
        output.push_str(&format_container_dry_run(
            &task_def.family,
            container,
            secret_names,
        ));
    }

    output
}

/// Format a single container's resolved configuration for dry-run output.
fn format_container_dry_run(
    family: &str,
    container: &ContainerDefinition,
    secret_names: &HashSet<String>,
) -> String {
    use std::fmt::Write;

    let mut output = String::new();
    let _ = writeln!(output, "Container: {}-{}", family, container.name);
    let _ = writeln!(output, "  Image: {}", container.image);

    if !container.environment.is_empty() {
        output.push_str("  Environment:\n");
        for env in &container.environment {
            if secret_names.contains(&env.name) {
                let _ = writeln!(output, "    {}=******", env.name);
            } else {
                let _ = writeln!(output, "    {}={}", env.name, env.value);
            }
        }
    }

    if !container.port_mappings.is_empty() {
        output.push_str("  Ports:\n");
        for pm in &container.port_mappings {
            let host_port = pm.host_port.unwrap_or(pm.container_port);
            let _ = writeln!(
                output,
                "    {}:{}/{}",
                host_port, pm.container_port, pm.protocol
            );
        }
    }

    if !container.depends_on.is_empty() {
        let deps: Vec<String> = container
            .depends_on
            .iter()
            .map(|d| format!("{} ({:?})", d.container_name, d.condition))
            .collect();
        let _ = writeln!(output, "  Depends on: {}", deps.join(", "));
    }

    if let Some(hc) = &container.health_check {
        let _ = writeln!(output, "  Health check: {}", hc.command.join(" "));
    }

    if let Some(wd) = &container.working_directory {
        let _ = writeln!(output, "  Working directory: {wd}");
    }

    if let Some(user) = &container.user {
        let _ = writeln!(output, "  User: {user}");
    }

    if let Some(timeout) = container.stop_timeout {
        let _ = writeln!(output, "  Stop timeout: {timeout}s");
    }

    if container.cpu.is_some()
        || container.memory.is_some()
        || container.memory_reservation.is_some()
    {
        output.push_str("  Resources:\n");
        if let Some(cpu) = container.cpu {
            let _ = writeln!(output, "    CPU: {cpu} units");
        }
        if let Some(mem) = container.memory {
            let _ = writeln!(output, "    Memory: {mem} MiB (hard limit)");
        }
        if let Some(mem) = container.memory_reservation {
            let _ = writeln!(output, "    Memory reservation: {mem} MiB (soft limit)");
        }
    }

    if !container.docker_labels.is_empty() {
        output.push_str("  Docker labels:\n");
        for (key, val) in &container.docker_labels {
            let _ = writeln!(output, "    {key}={val}");
        }
    }

    format_dry_run_extended_fields(&mut output, container);

    output
}

/// Format extended task definition fields (environmentFiles, ulimits, linuxParameters) for dry-run.
fn format_dry_run_extended_fields(output: &mut String, container: &ContainerDefinition) {
    use std::fmt::Write;

    if !container.environment_files.is_empty() {
        output.push_str("  Environment files:\n");
        for ef in &container.environment_files {
            let _ = writeln!(output, "    {}", ef.value);
        }
    }

    if !container.ulimits.is_empty() {
        output.push_str("  Ulimits:\n");
        for u in &container.ulimits {
            let _ = writeln!(
                output,
                "    {}: soft={}, hard={}",
                u.name, u.soft_limit, u.hard_limit
            );
        }
    }

    if let Some(lp) = &container.linux_parameters {
        output.push_str("  Linux parameters:\n");
        if let Some(init) = lp.init_process_enabled {
            let _ = writeln!(output, "    Init process: {init}");
        }
        if let Some(shm) = lp.shared_memory_size {
            let _ = writeln!(output, "    Shared memory: {shm} MiB");
        }
        if !lp.tmpfs.is_empty() {
            output.push_str("    Tmpfs:\n");
            for t in &lp.tmpfs {
                let _ = writeln!(output, "      {} ({} MiB)", t.container_path, t.size);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::collections::HashMap;

    use clap::Parser;

    use super::*;
    use crate::cli::{Cli, Command};
    use crate::taskdef::{ContainerDefinition, Environment, PortMapping};

    #[test]
    fn parse_run_with_no_metadata_flag() {
        let cli = Cli::try_parse_from(["lecs", "run", "-f", "task.json", "--no-metadata"])
            .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert!(args.no_metadata);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_without_no_metadata_flag() {
        let cli = Cli::try_parse_from(["lecs", "run", "-f", "task.json"]).expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert!(!args.no_metadata);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn container_color_returns_correct_codes() {
        assert_eq!(container_color(0), "32"); // Green
        assert_eq!(container_color(1), "33"); // Yellow
        assert_eq!(container_color(11), "31"); // Red (last)
    }

    #[test]
    fn container_color_wraps_around() {
        assert_eq!(container_color(12), "32"); // Wraps to Green
        assert_eq!(container_color(24), "32"); // Wraps again
    }

    #[test]
    fn format_log_line_produces_ansi_output() {
        let result = format_log_line("app", "hello world", "32");
        assert_eq!(result, "\x1b[32m[app]\x1b[0m hello world");
    }

    // --- dry-run tests ---

    #[test]
    fn format_container_dry_run_basic() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "nginx:latest".to_string(),
            ..Default::default()
        };
        let output = format_container_dry_run("test", &def, &HashSet::new());
        assert!(output.contains("Container: test-app"));
        assert!(output.contains("Image: nginx:latest"));
    }

    #[test]
    fn format_container_dry_run_with_ports() {
        let def = ContainerDefinition {
            name: "web".to_string(),
            image: "nginx:latest".to_string(),
            port_mappings: vec![PortMapping {
                container_port: 80,
                host_port: Some(8080),
                protocol: "tcp".to_string(),
            }],
            ..Default::default()
        };
        let output = format_container_dry_run("test", &def, &HashSet::new());
        assert!(output.contains("8080:80/tcp"));
    }

    #[test]
    fn format_container_dry_run_masks_secrets() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            environment: vec![
                Environment {
                    name: "PUBLIC_VAR".to_string(),
                    value: "visible".to_string(),
                },
                Environment {
                    name: "DB_PASSWORD".to_string(),
                    value: "super-secret".to_string(),
                },
            ],
            ..Default::default()
        };
        let secret_names: HashSet<String> = std::iter::once("DB_PASSWORD".to_string()).collect();
        let output = format_container_dry_run("test", &def, &secret_names);
        assert!(output.contains("PUBLIC_VAR=visible"));
        assert!(output.contains("DB_PASSWORD=******"));
        assert!(!output.contains("super-secret"));
    }

    #[test]
    fn format_container_dry_run_with_depends_on() {
        use crate::taskdef::DependencyCondition;
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            depends_on: vec![crate::taskdef::DependsOn {
                container_name: "db".to_string(),
                condition: DependencyCondition::Healthy,
            }],
            ..Default::default()
        };
        let output = format_container_dry_run("test", &def, &HashSet::new());
        assert!(output.contains("Depends on:"));
        assert!(output.contains("db"));
    }

    #[test]
    fn format_container_dry_run_with_health_check() {
        use crate::taskdef::HealthCheck;
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            health_check: Some(HealthCheck {
                command: vec!["CMD-SHELL".into(), "curl -f http://localhost/".into()],
                interval: 10,
                timeout: 5,
                retries: 3,
                start_period: 0,
            }),
            ..Default::default()
        };
        let output = format_container_dry_run("test", &def, &HashSet::new());
        assert!(output.contains("Health check: CMD-SHELL curl -f http://localhost/"));
    }

    #[test]
    fn display_dry_run_multi_container() {
        let td = TaskDefinition {
            family: "my-app".to_string(),
            task_role_arn: None,
            execution_role_arn: None,
            volumes: vec![],
            container_definitions: vec![
                ContainerDefinition {
                    name: "web".to_string(),
                    image: "nginx:latest".to_string(),
                    port_mappings: vec![PortMapping {
                        container_port: 80,
                        host_port: Some(8080),
                        protocol: "tcp".to_string(),
                    }],
                    ..Default::default()
                },
                ContainerDefinition {
                    name: "api".to_string(),
                    image: "node:20".to_string(),
                    port_mappings: vec![PortMapping {
                        container_port: 3000,
                        host_port: Some(3000),
                        protocol: "tcp".to_string(),
                    }],
                    ..Default::default()
                },
            ],
        };
        let output = display_dry_run(&td, &HashSet::new());
        assert!(output.contains("Network: lecs-my-app"));
        assert!(output.contains("Container: my-app-web"));
        assert!(output.contains("Container: my-app-api"));
    }

    #[test]
    fn display_dry_run_new_fields() {
        use crate::taskdef::{EnvironmentFile, LinuxParameters, TmpfsMount, Ulimit};
        let td = TaskDefinition {
            family: "test".to_string(),
            task_role_arn: None,
            execution_role_arn: None,
            volumes: vec![],
            container_definitions: vec![ContainerDefinition {
                name: "app".to_string(),
                image: "nginx:latest".to_string(),
                environment_files: vec![EnvironmentFile {
                    value: "app.env".to_string(),
                    r#type: "s3".to_string(),
                }],
                ulimits: vec![Ulimit {
                    name: "nofile".to_string(),
                    soft_limit: 1024,
                    hard_limit: 4096,
                }],
                linux_parameters: Some(LinuxParameters {
                    init_process_enabled: Some(true),
                    shared_memory_size: Some(256),
                    tmpfs: vec![TmpfsMount {
                        container_path: "/run".to_string(),
                        size: 64,
                        mount_options: vec!["rw".to_string()],
                    }],
                }),
                ..Default::default()
            }],
        };
        let output = display_dry_run(&td, &HashSet::new());
        assert!(output.contains("Environment files:"));
        assert!(output.contains("app.env"));
        assert!(output.contains("Ulimits:"));
        assert!(output.contains("nofile: soft=1024, hard=4096"));
        assert!(output.contains("Linux parameters:"));
        assert!(output.contains("Init process: true"));
        assert!(output.contains("Shared memory: 256 MiB"));
        assert!(output.contains("/run (64 MiB)"));
    }

    #[test]
    fn format_container_dry_run_with_new_fields() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            working_directory: Some("/app/work".to_string()),
            user: Some("nobody".to_string()),
            stop_timeout: Some(45),
            cpu: Some(512),
            memory: Some(1024),
            memory_reservation: Some(256),
            docker_labels: HashMap::from([("com.example.version".into(), "1.0".into())]),
            ..Default::default()
        };

        let output = format_container_dry_run("test", &def, &HashSet::new());

        assert!(output.contains("Working directory: /app/work"));
        assert!(output.contains("User: nobody"));
        assert!(output.contains("Stop timeout: 45s"));
        assert!(output.contains("Resources:"));
        assert!(output.contains("CPU: 512 units"));
        assert!(output.contains("Memory: 1024 MiB (hard limit)"));
        assert!(output.contains("Memory reservation: 256 MiB (soft limit)"));
        assert!(output.contains("Docker labels:"));
        assert!(output.contains("com.example.version=1.0"));
    }

    #[test]
    fn parse_run_with_dry_run_flag() {
        let cli = Cli::try_parse_from(["lecs", "run", "-f", "task.json", "--dry-run"])
            .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert!(args.dry_run);
            }
            _ => panic!("expected Run command"),
        }
    }
}
