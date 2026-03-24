use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use futures_util::StreamExt;
use tokio::task::JoinHandle;

use super::RunArgs;
use crate::container::{
    ContainerClient, ContainerConfig, ContainerRuntime, HealthCheckConfig, PortMappingConfig,
};
use crate::credentials;
use crate::events::{EventSink, EventType, LifecycleEvent, NullEventSink};
use crate::metadata::{
    self, ContainerMetadata, MetadataServer, ServerState, SharedState, build_container_metadata,
    build_task_metadata,
};
use crate::orchestrator::{ContainerSpec, orchestrate_startup};
use crate::overrides::OverrideConfig;
use crate::profile;
use crate::secrets::SecretsResolver;
use crate::taskdef::{
    ContainerDefinition, Environment, MountPoint, TaskDefinition, Volume, cloudformation, terraform,
};

/// Execute the `run` subcommand.
#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout, clippy::too_many_lines)]
pub async fn execute(args: &RunArgs, host: Option<&str>) -> Result<()> {
    // Determine the input file path for profile resolution.
    let input_path = args
        .task_definition
        .as_deref()
        .or(args.from_tf.as_deref())
        .or(args.from_cfn.as_deref())
        .ok_or_else(|| {
            anyhow::anyhow!("either --task-definition, --from-tf, or --from-cfn must be provided")
        })?;

    // Resolve profile paths
    let resolved = profile::resolve_from_args(
        input_path,
        args.profile.as_deref(),
        args.r#override.as_deref(),
        args.secrets.as_deref(),
    )?;

    let mut task_def = if let Some(tf_path) = &args.from_tf {
        terraform::from_terraform_file(tf_path, args.tf_resource.as_deref())?
    } else if let Some(cfn_path) = &args.from_cfn {
        cloudformation::from_cfn_file(cfn_path, args.cfn_resource.as_deref())?
    } else {
        TaskDefinition::from_file(input_path)?
    };
    tracing::info!(family = %task_def.family, containers = task_def.container_definitions.len(), "Parsed task definition");

    // Apply overrides if provided
    if let Some(override_path) = &resolved.override_path {
        let override_config = OverrideConfig::from_file(override_path)?;
        override_config.apply(&mut task_def);
        tracing::info!("Applied overrides from {}", override_path.display());
    }

    // Resolve secrets if provided
    let has_secrets = task_def
        .container_definitions
        .iter()
        .any(|c| !c.secrets.is_empty());

    if let Some(secrets_path) = &resolved.secrets_path {
        let secrets_resolver = SecretsResolver::from_file(secrets_path)?;
        for container in &mut task_def.container_definitions {
            let secret_env_vars = secrets_resolver.resolve(&container.secrets)?;
            for (name, value) in secret_env_vars {
                container.environment.push(Environment { name, value });
            }
        }
        tracing::info!("Resolved secrets from {}", secrets_path.display());
    } else if has_secrets {
        tracing::warn!(
            "Task definition has secrets but --secrets flag was not provided. Secret values will not be resolved."
        );
    }

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

/// Create the network and start all containers for a task definition.
///
/// Uses the orchestrator to resolve `dependsOn` DAG and start containers in
/// the correct order, waiting for dependency conditions between layers.
pub async fn run_task(
    client: &(impl ContainerRuntime + ?Sized),
    task_def: &TaskDefinition,
    metadata_port: Option<u16>,
    auth_token: Option<&str>,
    event_sink: &dyn EventSink,
) -> Result<(String, Vec<(String, String)>)> {
    let network_name = client.create_network(&task_def.family).await?;
    tracing::info!(network = %network_name, "Created network");

    let specs: Vec<ContainerSpec> = task_def
        .container_definitions
        .iter()
        .map(|def| {
            let config = build_container_config(
                &task_def.family,
                def,
                &network_name,
                metadata_port,
                &task_def.volumes,
                auth_token,
            );
            ContainerSpec {
                name: def.name.clone(),
                config,
                depends_on: def.depends_on.clone(),
                health_check: def.health_check.clone(),
                essential: def.essential,
            }
        })
        .collect();

    match orchestrate_startup(client, specs, event_sink).await {
        Ok(result) => Ok((network_name, result.started)),
        Err((partial, err)) => {
            cleanup(
                client,
                &partial.started,
                &network_name,
                event_sink,
                &task_def.family,
            )
            .await;
            Err(err.into())
        }
    }
}

/// Start the metadata/credentials sidecar server.
#[cfg(not(tarpaulin_include))]
pub(super) async fn start_metadata_server(
    task_def: &TaskDefinition,
) -> Result<(MetadataServer, SharedState)> {
    // Load AWS credentials (best-effort)
    let aws_creds = match credentials::load_local_credentials(task_def.task_role_arn.as_deref())
        .await
    {
        Ok(creds) => {
            tracing::info!("Loaded AWS credentials for metadata server");
            Some(creds)
        }
        Err(e) => {
            tracing::warn!(error = %e, "Could not load AWS credentials; credential endpoint will return 404");
            None
        }
    };

    let task_metadata = build_task_metadata(task_def);
    let container_metadata: HashMap<String, ContainerMetadata> = task_def
        .container_definitions
        .iter()
        .map(|def| {
            (
                def.name.clone(),
                build_container_metadata(&task_def.family, def),
            )
        })
        .collect();

    let auth_token = metadata::generate_auth_token();
    tracing::debug!("Generated credentials auth token");

    let state = Arc::new(tokio::sync::RwLock::new(ServerState {
        task_metadata,
        container_metadata,
        credentials: aws_creds,
        container_ids: HashMap::new(),
        auth_token,
    }));

    let server = MetadataServer::start(state.clone()).await?;
    tracing::info!(
        port = server.port,
        "ECS metadata server running on http://127.0.0.1:{}",
        server.port
    );

    Ok((server, state))
}

/// Best-effort cleanup: stop and remove containers, then remove the network.
pub async fn cleanup(
    client: &(impl ContainerRuntime + ?Sized),
    containers: &[(String, String)],
    network: &str,
    event_sink: &dyn EventSink,
    family: &str,
) {
    for (id, name) in containers {
        if let Err(e) = client.stop_container(id, None).await {
            tracing::warn!(container = %name, error = %e, "Failed to stop container");
        }
        if let Err(e) = client.remove_container(id).await {
            tracing::warn!(container = %name, error = %e, "Failed to remove container");
        }
        tracing::info!(container = %name, "Cleaned up");
    }

    if let Err(e) = client.remove_network(network).await {
        tracing::warn!(network = %network, error = %e, "Failed to remove network");
    }
    tracing::info!(network = %network, "Network removed");

    event_sink.emit(&LifecycleEvent::new(
        EventType::CleanupCompleted,
        family,
        None,
        None,
    ));
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

/// Resolve mount points against task-level volumes into Docker bind mount strings.
///
/// Returns bind strings in format `host_path:container_path` or `host_path:container_path:ro`.
/// Volumes without `host.source_path` (Docker-managed volumes) are skipped with a warning.
fn resolve_binds(mount_points: &[MountPoint], volumes: &[Volume]) -> Vec<String> {
    let volume_map: HashMap<&str, &Volume> = volumes.iter().map(|v| (v.name.as_str(), v)).collect();

    mount_points
        .iter()
        .filter_map(|mp| {
            let volume = volume_map.get(mp.source_volume.as_str())?;
            let Some(host) = &volume.host else {
                tracing::warn!(
                    volume = %volume.name,
                    "Skipping volume without host.sourcePath (Docker-managed volumes not supported)"
                );
                return None;
            };

            let bind = if mp.read_only {
                format!("{}:{}:ro", host.source_path, mp.container_path)
            } else {
                format!("{}:{}", host.source_path, mp.container_path)
            };
            Some(bind)
        })
        .collect()
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn build_container_config(
    family: &str,
    def: &ContainerDefinition,
    network: &str,
    metadata_port: Option<u16>,
    volumes: &[Volume],
    auth_token: Option<&str>,
) -> ContainerConfig {
    // Start with user-defined docker labels, then override with lecs management labels
    let mut labels: HashMap<String, String> = def.docker_labels.clone();
    labels.insert("lecs.managed".into(), "true".into());
    labels.insert("lecs.task".into(), family.into());
    labels.insert("lecs.container".into(), def.name.clone());

    // Store secret names for inspect masking
    if !def.secrets.is_empty() {
        let secret_names: Vec<String> = def.secrets.iter().map(|s| s.name.clone()).collect();
        labels.insert("lecs.secrets".into(), secret_names.join(","));
    }

    // Store stop timeout for cleanup
    if let Some(timeout) = def.stop_timeout {
        labels.insert("lecs.stop_timeout".into(), timeout.to_string());
    }

    // Store dependency info for ps display
    if !def.depends_on.is_empty() {
        let deps: Vec<String> = def
            .depends_on
            .iter()
            .map(|d| format!("{}:{:?}", d.container_name, d.condition))
            .collect();
        labels.insert("lecs.depends_on".into(), deps.join(","));
    }

    let mut env: Vec<String> = def
        .environment
        .iter()
        .map(|e| format!("{}={}", e.name, e.value))
        .collect();

    if let Some(port) = metadata_port {
        env.push(format!(
            "ECS_CONTAINER_METADATA_URI_V4=http://host.docker.internal:{port}/v4/{}",
            def.name
        ));
        env.push(format!(
            "AWS_CONTAINER_CREDENTIALS_FULL_URI=http://host.docker.internal:{port}/credentials"
        ));
        if let Some(token) = auth_token {
            env.push(format!("AWS_CONTAINER_AUTHORIZATION_TOKEN={token}"));
        }
    }

    let port_mappings = def
        .port_mappings
        .iter()
        .map(|p| PortMappingConfig {
            host_port: p.host_port.unwrap_or(p.container_port),
            container_port: p.container_port,
            protocol: p.protocol.clone(),
        })
        .collect();

    let health_check = def.health_check.as_ref().map(|hc| {
        const NANOS_PER_SEC: i64 = 1_000_000_000;
        HealthCheckConfig {
            test: hc.command.clone(),
            interval_ns: i64::from(hc.interval) * NANOS_PER_SEC,
            timeout_ns: i64::from(hc.timeout) * NANOS_PER_SEC,
            retries: i64::from(hc.retries),
            start_period_ns: i64::from(hc.start_period) * NANOS_PER_SEC,
        }
    });

    let binds = resolve_binds(&def.mount_points, volumes);

    ContainerConfig {
        name: format!("{family}-{}", def.name),
        image: def.image.clone(),
        command: def.command.clone(),
        entry_point: def.entry_point.clone(),
        env,
        port_mappings,
        network: network.into(),
        network_aliases: vec![def.name.clone()],
        labels,
        extra_hosts: {
            let mut hosts: Vec<String> = def
                .extra_hosts
                .iter()
                .map(|h| format!("{}:{}", h.hostname, h.ip_address))
                .collect();
            // Add default host.docker.internal mapping unless user overrides it
            if !hosts.iter().any(|h| h.starts_with("host.docker.internal:")) {
                hosts.push("host.docker.internal:host-gateway".to_string());
            }
            hosts
        },
        health_check,
        binds,
        working_dir: def.working_directory.clone(),
        user: def.user.clone(),
        cpu_units: def.cpu,
        memory_mib: def.memory,
        memory_reservation_mib: def.memory_reservation,
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

    output
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use clap::Parser;

    use super::*;
    use crate::cli::{Cli, Command};
    use crate::container::test_support::MockContainerClient;
    use crate::taskdef::{
        ContainerDefinition, DependencyCondition, DependsOn, Environment, ExtraHost, MountPoint,
        PortMapping, Volume, VolumeHost,
    };

    fn single_container_taskdef() -> TaskDefinition {
        TaskDefinition {
            family: "web".to_string(),
            task_role_arn: None,
            execution_role_arn: None,
            volumes: vec![],
            container_definitions: vec![ContainerDefinition {
                name: "app".to_string(),
                image: "nginx:latest".to_string(),
                ..Default::default()
            }],
        }
    }

    fn two_container_taskdef() -> TaskDefinition {
        TaskDefinition {
            family: "multi".to_string(),
            task_role_arn: None,
            execution_role_arn: None,
            volumes: vec![],
            container_definitions: vec![
                ContainerDefinition {
                    name: "app".to_string(),
                    image: "nginx:latest".to_string(),
                    ..Default::default()
                },
                ContainerDefinition {
                    name: "sidecar".to_string(),
                    image: "redis:latest".to_string(),
                    essential: false,
                    ..Default::default()
                },
            ],
        }
    }

    #[tokio::test]
    async fn run_task_single_container() {
        let mock = MockContainerClient {
            create_network_results: Mutex::new(VecDeque::from([Ok("lecs-web".to_string())])),
            create_container_results: Mutex::new(VecDeque::from([Ok("container-1".to_string())])),
            start_container_results: Mutex::new(VecDeque::from([Ok(())])),
            ..MockContainerClient::new()
        };

        let task_def = single_container_taskdef();
        let (network, containers) = run_task(&mock, &task_def, None, None, &NullEventSink)
            .await
            .expect("should succeed");

        assert_eq!(network, "lecs-web");
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].0, "container-1");
        assert_eq!(containers[0].1, "app");
    }

    #[tokio::test]
    async fn run_task_multi_container() {
        let mock = MockContainerClient {
            create_network_results: Mutex::new(VecDeque::from([Ok("lecs-multi".to_string())])),
            create_container_results: Mutex::new(VecDeque::from([
                Ok("c1".to_string()),
                Ok("c2".to_string()),
            ])),
            start_container_results: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
            ..MockContainerClient::new()
        };

        let task_def = two_container_taskdef();
        let (_, containers) = run_task(&mock, &task_def, None, None, &NullEventSink)
            .await
            .expect("should succeed");

        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].1, "app");
        assert_eq!(containers[1].1, "sidecar");
    }

    #[tokio::test]
    async fn run_task_network_failure() {
        let mock = MockContainerClient {
            create_network_results: Mutex::new(VecDeque::from([Err(
                crate::container::ContainerError::RuntimeNotRunning,
            )])),
            ..MockContainerClient::new()
        };

        let task_def = single_container_taskdef();
        let result = run_task(&mock, &task_def, None, None, &NullEventSink).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_task_container_create_failure() {
        let mock = MockContainerClient {
            create_network_results: Mutex::new(VecDeque::from([Ok("lecs-multi".to_string())])),
            create_container_results: Mutex::new(VecDeque::from([
                Ok("c1".to_string()),
                Err(crate::container::ContainerError::RuntimeNotRunning),
            ])),
            start_container_results: Mutex::new(VecDeque::from([Ok(())])),
            ..MockContainerClient::new()
        };

        let task_def = two_container_taskdef();
        let result = run_task(&mock, &task_def, None, None, &NullEventSink).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cleanup_success() {
        let mock = MockContainerClient {
            stop_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_network_results: Mutex::new(VecDeque::from([Ok(())])),
            ..MockContainerClient::new()
        };

        let containers = vec![("c1".to_string(), "app".to_string())];
        cleanup(&mock, &containers, "lecs-test", &NullEventSink, "test").await;
        // No panic = success (best-effort cleanup)
    }

    #[tokio::test]
    async fn cleanup_tolerates_stop_failure() {
        let mock = MockContainerClient {
            stop_container_results: Mutex::new(VecDeque::from([Err(
                crate::container::ContainerError::RuntimeNotRunning,
            )])),
            remove_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_network_results: Mutex::new(VecDeque::from([Ok(())])),
            ..MockContainerClient::new()
        };

        let containers = vec![("c1".to_string(), "app".to_string())];
        cleanup(&mock, &containers, "lecs-test", &NullEventSink, "test").await;
    }

    #[tokio::test]
    async fn cleanup_tolerates_remove_failure() {
        let mock = MockContainerClient {
            stop_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_network_results: Mutex::new(VecDeque::from([Err(
                crate::container::ContainerError::RuntimeNotRunning,
            )])),
            ..MockContainerClient::new()
        };

        let containers = vec![("c1".to_string(), "app".to_string())];
        cleanup(&mock, &containers, "lecs-test", &NullEventSink, "test").await;
    }

    #[tokio::test]
    async fn cleanup_emits_cleanup_completed_event() {
        use crate::events::CollectingEventSink;

        let mock = MockContainerClient {
            stop_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_container_results: Mutex::new(VecDeque::from([Ok(())])),
            remove_network_results: Mutex::new(VecDeque::from([Ok(())])),
            ..MockContainerClient::new()
        };

        let containers = vec![("c1".to_string(), "app".to_string())];
        let sink = CollectingEventSink::new();
        cleanup(&mock, &containers, "lecs-test", &sink, "my-app").await;

        let events = sink.events();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].event_type,
            crate::events::EventType::CleanupCompleted
        ));
        assert_eq!(events[0].family, "my-app");
        assert!(events[0].container_name.is_none());
    }

    #[test]
    fn build_container_config_basic() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "nginx:latest".to_string(),
            command: vec!["nginx".into(), "-g".into(), "daemon off;".into()],
            entry_point: vec!["/docker-entrypoint.sh".into()],
            environment: vec![Environment {
                name: "PORT".to_string(),
                value: "8080".to_string(),
            }],
            port_mappings: vec![PortMapping {
                container_port: 80,
                host_port: Some(8080),
                protocol: "tcp".to_string(),
            }],
            cpu: Some(256),
            memory: Some(512),
            ..Default::default()
        };

        let config = build_container_config("my-app", &def, "lecs-my-app", None, &[], None);

        assert_eq!(config.name, "my-app-app");
        assert_eq!(config.image, "nginx:latest");
        assert_eq!(config.command, vec!["nginx", "-g", "daemon off;"]);
        assert_eq!(config.entry_point, vec!["/docker-entrypoint.sh"]);
        assert_eq!(config.env, vec!["PORT=8080"]);
        assert_eq!(config.port_mappings.len(), 1);
        assert_eq!(config.port_mappings[0].host_port, 8080);
        assert_eq!(config.port_mappings[0].container_port, 80);
        assert_eq!(config.port_mappings[0].protocol, "tcp");
        assert_eq!(config.network, "lecs-my-app");
        assert_eq!(config.network_aliases, vec!["app"]);
        assert_eq!(config.labels.get("lecs.managed").unwrap(), "true");
        assert_eq!(config.labels.get("lecs.task").unwrap(), "my-app");
        assert_eq!(config.labels.get("lecs.container").unwrap(), "app");
    }

    #[test]
    fn build_container_config_port_default() {
        let def = ContainerDefinition {
            name: "web".to_string(),
            image: "alpine:latest".to_string(),
            port_mappings: vec![PortMapping {
                container_port: 3000,
                host_port: None,
                protocol: "tcp".to_string(),
            }],
            ..Default::default()
        };

        let config = build_container_config("test", &def, "lecs-test", None, &[], None);

        // host_port defaults to container_port
        assert_eq!(config.port_mappings[0].host_port, 3000);
        assert_eq!(config.port_mappings[0].container_port, 3000);
    }

    #[test]
    fn build_container_config_docker_labels_merged() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            docker_labels: HashMap::from([
                ("com.example.env".into(), "dev".into()),
                ("lecs.managed".into(), "user-override".into()),
            ]),
            ..Default::default()
        };

        let config = build_container_config("test", &def, "lecs-test", None, &[], None);

        // User labels are included
        assert_eq!(config.labels.get("com.example.env").unwrap(), "dev");
        // Lecs management labels take precedence over user labels
        assert_eq!(config.labels.get("lecs.managed").unwrap(), "true");
    }

    #[test]
    fn build_container_config_extra_hosts_merged() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            extra_hosts: vec![ExtraHost {
                hostname: "myhost".to_string(),
                ip_address: "10.0.0.1".to_string(),
            }],
            ..Default::default()
        };

        let config = build_container_config("test", &def, "lecs-test", None, &[], None);
        assert!(config.extra_hosts.contains(&"myhost:10.0.0.1".to_string()));
        // Default host.docker.internal should still be present
        assert!(
            config
                .extra_hosts
                .contains(&"host.docker.internal:host-gateway".to_string())
        );
    }

    #[test]
    fn build_container_config_extra_hosts_user_overrides_default() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            extra_hosts: vec![ExtraHost {
                hostname: "host.docker.internal".to_string(),
                ip_address: "192.168.1.1".to_string(),
            }],
            ..Default::default()
        };

        let config = build_container_config("test", &def, "lecs-test", None, &[], None);
        assert_eq!(config.extra_hosts, vec!["host.docker.internal:192.168.1.1"]);
    }

    #[test]
    fn build_container_config_has_extra_hosts() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            ..Default::default()
        };

        let config = build_container_config("test", &def, "lecs-test", None, &[], None);
        assert_eq!(
            config.extra_hosts,
            vec!["host.docker.internal:host-gateway"]
        );
    }

    #[test]
    fn build_container_config_empty_optionals() {
        let def = ContainerDefinition {
            name: "minimal".to_string(),
            image: "alpine:latest".to_string(),
            ..Default::default()
        };

        let config = build_container_config("test", &def, "lecs-test", None, &[], None);

        assert!(config.command.is_empty());
        assert!(config.entry_point.is_empty());
        assert!(config.env.is_empty());
        assert!(config.port_mappings.is_empty());
    }

    #[test]
    fn build_container_config_with_metadata_port() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            ..Default::default()
        };

        let config = build_container_config("test", &def, "lecs-test", Some(12345), &[], None);

        assert!(config.env.contains(
            &"ECS_CONTAINER_METADATA_URI_V4=http://host.docker.internal:12345/v4/app".to_string()
        ));
        assert!(
            config.env.contains(
                &"AWS_CONTAINER_CREDENTIALS_FULL_URI=http://host.docker.internal:12345/credentials"
                    .to_string()
            )
        );
    }

    #[test]
    fn build_container_config_with_auth_token() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            ..Default::default()
        };

        let config = build_container_config(
            "test",
            &def,
            "lecs-test",
            Some(12345),
            &[],
            Some("my-secret-token"),
        );

        assert!(
            config
                .env
                .contains(&"AWS_CONTAINER_AUTHORIZATION_TOKEN=my-secret-token".to_string())
        );
    }

    #[test]
    fn build_container_config_without_auth_token() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            ..Default::default()
        };

        let config = build_container_config("test", &def, "lecs-test", Some(12345), &[], None);

        assert!(
            !config
                .env
                .iter()
                .any(|e| e.starts_with("AWS_CONTAINER_AUTHORIZATION_TOKEN="))
        );
    }

    #[test]
    fn build_container_config_without_metadata_port() {
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            ..Default::default()
        };

        let config = build_container_config("test", &def, "lecs-test", None, &[], None);

        assert!(
            !config
                .env
                .iter()
                .any(|e| e.starts_with("ECS_CONTAINER_METADATA_URI_V4="))
        );
        assert!(
            !config
                .env
                .iter()
                .any(|e| e.starts_with("AWS_CONTAINER_CREDENTIALS_FULL_URI="))
        );
    }

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
    fn build_container_config_with_health_check() {
        use crate::taskdef::HealthCheck;

        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            health_check: Some(HealthCheck {
                command: vec!["CMD-SHELL".into(), "curl -f http://localhost/".into()],
                interval: 10,
                timeout: 5,
                retries: 3,
                start_period: 15,
            }),
            ..Default::default()
        };

        let config = build_container_config("test", &def, "lecs-test", None, &[], None);
        let hc = config
            .health_check
            .as_ref()
            .expect("should have health check");
        assert_eq!(hc.test, vec!["CMD-SHELL", "curl -f http://localhost/"]);
        assert_eq!(hc.interval_ns, 10_000_000_000);
        assert_eq!(hc.timeout_ns, 5_000_000_000);
        assert_eq!(hc.retries, 3);
        assert_eq!(hc.start_period_ns, 15_000_000_000);
    }

    #[tokio::test]
    async fn run_task_with_dependencies() {
        let mock = MockContainerClient {
            create_network_results: Mutex::new(VecDeque::from([Ok("lecs-test".to_string())])),
            create_container_results: Mutex::new(VecDeque::from([
                Ok("db-id".to_string()),
                Ok("app-id".to_string()),
            ])),
            start_container_results: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
            ..MockContainerClient::new()
        };

        let task_def = TaskDefinition {
            family: "test".to_string(),
            task_role_arn: None,
            execution_role_arn: None,
            volumes: vec![],
            container_definitions: vec![
                ContainerDefinition {
                    name: "db".to_string(),
                    image: "postgres:16".to_string(),
                    ..Default::default()
                },
                ContainerDefinition {
                    name: "app".to_string(),
                    image: "my-app:latest".to_string(),
                    depends_on: vec![DependsOn {
                        container_name: "db".to_string(),
                        condition: DependencyCondition::Start,
                    }],
                    ..Default::default()
                },
            ],
        };

        let (network, containers) = run_task(&mock, &task_def, None, None, &NullEventSink)
            .await
            .expect("should succeed");

        assert_eq!(network, "lecs-test");
        assert_eq!(containers.len(), 2);
        // DAG ensures db starts before app
        assert_eq!(containers[0].1, "db");
        assert_eq!(containers[1].1, "app");
    }

    #[test]
    fn resolve_binds_single_mount() {
        let volumes = vec![Volume {
            name: "data".to_string(),
            host: Some(VolumeHost {
                source_path: "/host/data".to_string(),
            }),
        }];
        let mount_points = vec![MountPoint {
            source_volume: "data".to_string(),
            container_path: "/app/data".to_string(),
            read_only: false,
        }];

        let binds = resolve_binds(&mount_points, &volumes);
        assert_eq!(binds, vec!["/host/data:/app/data"]);
    }

    #[test]
    fn resolve_binds_read_only() {
        let volumes = vec![Volume {
            name: "config".to_string(),
            host: Some(VolumeHost {
                source_path: "/host/config".to_string(),
            }),
        }];
        let mount_points = vec![MountPoint {
            source_volume: "config".to_string(),
            container_path: "/app/config".to_string(),
            read_only: true,
        }];

        let binds = resolve_binds(&mount_points, &volumes);
        assert_eq!(binds, vec!["/host/config:/app/config:ro"]);
    }

    #[test]
    fn resolve_binds_skips_docker_managed_volume() {
        let volumes = vec![
            Volume {
                name: "data".to_string(),
                host: Some(VolumeHost {
                    source_path: "/host/data".to_string(),
                }),
            },
            Volume {
                name: "cache".to_string(),
                host: None,
            },
        ];
        let mount_points = vec![
            MountPoint {
                source_volume: "data".to_string(),
                container_path: "/app/data".to_string(),
                read_only: false,
            },
            MountPoint {
                source_volume: "cache".to_string(),
                container_path: "/tmp/cache".to_string(),
                read_only: true,
            },
        ];

        let binds = resolve_binds(&mount_points, &volumes);
        assert_eq!(binds, vec!["/host/data:/app/data"]);
    }

    #[test]
    fn build_container_config_with_volumes() {
        let volumes = vec![Volume {
            name: "data".to_string(),
            host: Some(VolumeHost {
                source_path: "/host/data".to_string(),
            }),
        }];
        let def = ContainerDefinition {
            name: "app".to_string(),
            image: "alpine:latest".to_string(),
            mount_points: vec![MountPoint {
                source_volume: "data".to_string(),
                container_path: "/app/data".to_string(),
                read_only: false,
            }],
            ..Default::default()
        };

        let config = build_container_config("test", &def, "lecs-test", None, &volumes, None);
        assert_eq!(config.binds, vec!["/host/data:/app/data"]);
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
