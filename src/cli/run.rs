use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use futures_util::StreamExt;
use tokio::task::JoinHandle;

use super::RunArgs;
use super::task_lifecycle::{
    RestartContext, RestartOutcome, cleanup, restart_container, run_task, start_metadata_server,
};
use crate::container::ContainerClient;
use crate::credentials::CredentialRefresher;
use crate::events::{EventSink, EventType, LifecycleEvent, NullEventSink};
use crate::metadata::{self, SharedState};
use crate::orchestrator::{
    EssentialExit, OrchestratorError, RestartPolicy, RestartTracker, watch_essential_exit,
};
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

    // Service mode: auto-restart, long-running until Ctrl+C.
    if args.service {
        let result = run_service_loop(
            &client,
            &task_def,
            metadata_port,
            auth_token.as_deref(),
            metadata_state.as_ref(),
            &*event_sink,
            args.restart.into(),
            args.max_restarts,
        )
        .await;

        if let Some(server) = metadata_server {
            server.shutdown().await;
        }
        return result;
    }

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

/// Spawn a background watcher that sends an `EssentialExit` on the channel
/// when the container with the given id exits.
#[cfg(not(tarpaulin_include))]
fn spawn_essential_watcher(
    client: Arc<ContainerClient>,
    id: String,
    name: String,
    tx: tokio::sync::mpsc::UnboundedSender<EssentialExit>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let exit = watch_essential_exit(&*client, &id, &name).await;
        let _ = tx.send(exit);
    })
}

/// Spawn a background log streamer that prints formatted lines to stdout until
/// the stream closes or the task is aborted.
#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout)]
fn spawn_log_stream(
    client: Arc<ContainerClient>,
    id: String,
    name: String,
    color: String,
) -> JoinHandle<()> {
    tokio::spawn(async move {
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
    })
}

/// Run the task in service mode: auto-restart containers, long-running until
/// Ctrl+C or max restarts exceeded.
///
/// Monitors essential containers via dedicated watcher tasks that forward
/// exits through an mpsc channel. On each exit, the container's
/// `RestartTracker` determines whether to restart (with exponential backoff)
/// or to give up. Non-essential containers are not restarted; their log
/// streams naturally terminate on exit.
#[cfg(not(tarpaulin_include))]
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub async fn run_service_loop(
    client: &Arc<ContainerClient>,
    task_def: &TaskDefinition,
    metadata_port: Option<u16>,
    auth_token: Option<&str>,
    metadata_state: Option<&SharedState>,
    event_sink: &dyn EventSink,
    restart_policy: RestartPolicy,
    max_restarts: u32,
) -> Result<()> {
    // 1. Initial startup via standard orchestration path.
    let (network_name, started_containers) =
        run_task(&**client, task_def, metadata_port, auth_token, event_sink).await?;

    // Update metadata container IDs.
    if let Some(state) = metadata_state {
        for (id, name) in &started_containers {
            metadata::update_container_id(state, name, id).await;
        }
    }

    // 2. Index container configs and trackers by name.
    let configs: HashMap<String, crate::container::ContainerConfig> = task_def
        .container_definitions
        .iter()
        .map(|def| {
            let config = super::task_lifecycle::build_container_config(
                &task_def.family,
                def,
                &network_name,
                metadata_port,
                &task_def.volumes,
                auth_token,
            );
            (def.name.clone(), config)
        })
        .collect();

    let essential_names: HashSet<String> = task_def
        .container_definitions
        .iter()
        .filter(|d| d.essential)
        .map(|d| d.name.clone())
        .collect();

    let mut trackers: HashMap<String, RestartTracker> = essential_names
        .iter()
        .map(|name| {
            (
                name.clone(),
                RestartTracker::new(restart_policy, max_restarts),
            )
        })
        .collect();

    // 3. Track current container IDs by name (mutable as containers restart).
    let mut id_by_name: HashMap<String, String> = started_containers
        .iter()
        .map(|(id, name)| (name.clone(), id.clone()))
        .collect();

    // 4. Start background credential refresher (only if metadata server is active).
    let credential_refresher_handle: Option<JoinHandle<()>> = metadata_state.map(|state| {
        let refresher = CredentialRefresher::new(Arc::clone(state), task_def.task_role_arn.clone());
        refresher.start()
    });

    // 5. Spawn log streams and essential watchers.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<EssentialExit>();
    let mut log_handles: HashMap<String, JoinHandle<()>> = HashMap::new();
    let mut watcher_handles: HashMap<String, JoinHandle<()>> = HashMap::new();

    for (i, (id, name)) in started_containers.iter().enumerate() {
        let color = container_color(i).to_string();
        log_handles.insert(
            name.clone(),
            spawn_log_stream(Arc::clone(client), id.clone(), name.clone(), color),
        );
        if essential_names.contains(name) {
            watcher_handles.insert(
                name.clone(),
                spawn_essential_watcher(Arc::clone(client), id.clone(), name.clone(), tx.clone()),
            );
        }
    }

    // Keep the original sender so restarted essential containers can spawn
    // replacement watchers. Because `tx` remains alive here, `rx` will not
    // close solely because existing watcher tasks exit.
    let result: Result<()> = loop {
        tokio::select! {
            Some(exit) = rx.recv() => {
                let name = exit.container_name.clone();
                let Some(tracker) = trackers.get_mut(&name) else {
                    // Non-essential (should not happen as we only spawn watchers for essential).
                    continue;
                };

                if !tracker.should_restart(exit.exit_code) {
                    // Either policy=None, max_restarts reached, or OnFailure with exit 0.
                    if tracker.restart_count() >= tracker.max_restarts() {
                        event_sink.emit(&LifecycleEvent::new(
                            EventType::MaxRestartsExceeded,
                            &task_def.family,
                            Some(&name),
                            Some(&format!("exit code: {}", exit.exit_code)),
                        ));
                        break Err(OrchestratorError::MaxRestartsExceeded(
                            name,
                            tracker.max_restarts(),
                        )
                        .into());
                    }
                    event_sink.emit(&LifecycleEvent::new(
                        EventType::Exited,
                        &task_def.family,
                        Some(&name),
                        Some(&format!("exit code: {}", exit.exit_code)),
                    ));
                    break Ok(());
                }

                // Backoff, then try restart (with retries on failure).
                let restart_ctx = RestartContext {
                    metadata_state,
                    event_sink,
                    family: &task_def.family,
                };
                match attempt_restart(
                    client,
                    &name,
                    id_by_name.get(&name).cloned(),
                    (&configs[&name], &restart_ctx, tracker),
                )
                .await
                {
                    Ok(new_id) => {
                        id_by_name.insert(name.clone(), new_id.clone());
                        // Respawn log stream and watcher for the new container.
                        if let Some(h) = log_handles.remove(&name) {
                            h.abort();
                        }
                        log_handles.insert(
                            name.clone(),
                            spawn_log_stream(
                                Arc::clone(client),
                                new_id.clone(),
                                name.clone(),
                                container_color(log_handles.len()).to_string(),
                            ),
                        );
                        watcher_handles.insert(
                            name.clone(),
                            spawn_essential_watcher(
                                Arc::clone(client),
                                new_id,
                                name.clone(),
                                tx.clone(),
                            ),
                        );
                    }
                    Err(err) => {
                        event_sink.emit(&LifecycleEvent::new(
                            EventType::MaxRestartsExceeded,
                            &task_def.family,
                            Some(&name),
                            None,
                        ));
                        break Err(err);
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Received SIGINT, shutting down service mode...");
                break Ok(());
            }
        }
    };

    // 6. Shutdown: abort refresher/watchers/streams, then cleanup containers.
    if let Some(h) = credential_refresher_handle {
        h.abort();
    }
    for h in watcher_handles.values() {
        h.abort();
    }
    for h in log_handles.values() {
        h.abort();
    }

    // Build final containers list (current IDs) for cleanup.
    let final_containers: Vec<(String, String)> = id_by_name
        .into_iter()
        .map(|(name, id)| (id, name))
        .collect();
    cleanup(
        &**client,
        &final_containers,
        &network_name,
        event_sink,
        &task_def.family,
    )
    .await;

    result
}

/// Attempt to restart a container, retrying on intermediate failures until
/// `max_restarts` is exceeded. Returns the new container ID on success.
#[cfg(not(tarpaulin_include))]
async fn attempt_restart(
    client: &Arc<ContainerClient>,
    name: &str,
    mut old_id: Option<String>,
    restart: (
        &crate::container::ContainerConfig,
        &RestartContext<'_>,
        &mut RestartTracker,
    ),
) -> Result<String> {
    let (config, ctx, tracker) = restart;
    loop {
        let backoff = tracker.next_backoff();
        tracing::info!(container = %name, backoff_secs = backoff.as_secs(), "Backing off before restart");
        tokio::time::sleep(backoff).await;
        tracker.record_restart();

        match restart_container(&**client, name, old_id.as_deref(), config, ctx).await {
            RestartOutcome::Replaced { new_id } => return Ok(new_id),
            RestartOutcome::FailedBeforeRemoval(e) => {
                tracing::warn!(container = %name, error = %e, "Restart failed (stop/remove)");
                // old_id still exists in runtime; leave unchanged and retry.
            }
            RestartOutcome::CreateFailed(e) => {
                tracing::warn!(container = %name, error = %e, "Restart failed (create/start)");
                // No container left in the runtime; stay on the create-only path.
                old_id = None;
            }
        }

        if tracker.restart_count() >= tracker.max_restarts() {
            return Err(OrchestratorError::MaxRestartsExceeded(
                name.to_string(),
                tracker.max_restarts(),
            )
            .into());
        }
    }
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

    #[test]
    fn parse_run_with_service_flag() {
        let cli = Cli::try_parse_from(["lecs", "run", "-f", "task.json", "--service"])
            .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert!(args.service);
                // Default restart policy is on-failure
                assert!(matches!(
                    args.restart,
                    crate::cli::RestartPolicyArg::OnFailure
                ));
                assert_eq!(args.max_restarts, 10);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_service_with_custom_policy_and_max() {
        let cli = Cli::try_parse_from([
            "lecs",
            "run",
            "-f",
            "task.json",
            "--service",
            "--restart",
            "always",
            "--max-restarts",
            "5",
        ])
        .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert!(args.service);
                assert!(matches!(args.restart, crate::cli::RestartPolicyArg::Always));
                assert_eq!(args.max_restarts, 5);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_service_conflicts_with_dry_run() {
        let result =
            Cli::try_parse_from(["lecs", "run", "-f", "task.json", "--service", "--dry-run"]);
        assert!(
            result.is_err(),
            "expected --service and --dry-run to conflict"
        );
    }

    #[test]
    fn parse_run_restart_without_service_fails() {
        // --restart requires --service
        let result = Cli::try_parse_from(["lecs", "run", "-f", "task.json", "--restart", "always"]);
        assert!(result.is_err(), "--restart without --service should fail");
    }

    #[test]
    fn restart_policy_arg_converts_to_orchestrator_policy() {
        use crate::cli::RestartPolicyArg;
        use crate::orchestrator::RestartPolicy;
        assert_eq!(
            RestartPolicy::from(RestartPolicyArg::None),
            RestartPolicy::None
        );
        assert_eq!(
            RestartPolicy::from(RestartPolicyArg::OnFailure),
            RestartPolicy::OnFailure
        );
        assert_eq!(
            RestartPolicy::from(RestartPolicyArg::Always),
            RestartPolicy::Always
        );
    }
}
