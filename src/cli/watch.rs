//! `lecs watch` command implementation.
//!
//! Watches task definition, override, and secrets files for changes
//! and automatically restarts the task on modification.

use std::path::PathBuf;

use anyhow::{Result, bail};

use super::WatchArgs;
use crate::profile::ResolvedPaths;

/// Return the input file path (task definition, Terraform, or `CloudFormation` file).
///
/// # Errors
///
/// Returns an error if none of `--task-definition`, `--from-tf`, or `--from-cfn` is provided.
fn input_path(args: &WatchArgs) -> Result<&std::path::Path> {
    args.source.input_path()
}

/// Collect all paths that should be watched for changes.
///
/// Includes the task definition (or Terraform file), CLI-specified override/secrets,
/// profile-resolved override/secrets (if different from CLI args), and any extra watch paths.
pub fn collect_watch_paths(
    input: &std::path::Path,
    args: &WatchArgs,
    resolved: &ResolvedPaths,
) -> Vec<PathBuf> {
    let mut paths = vec![input.to_path_buf()];
    if let Some(ref p) = args.source.r#override {
        paths.push(p.clone());
    }
    if let Some(ref p) = args.source.secrets {
        paths.push(p.clone());
    }
    // Add profile-resolved paths that weren't already added via CLI flags
    if let Some(ref p) = resolved.override_path
        && !paths.contains(p)
    {
        paths.push(p.clone());
    }
    if let Some(ref p) = resolved.secrets_path
        && !paths.contains(p)
    {
        paths.push(p.clone());
    }
    for p in &args.watch_paths {
        paths.push(p.clone());
    }
    paths
}

/// Validate that all watch paths exist.
pub fn validate_watch_paths(paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        if !path.exists() {
            bail!("Watch path does not exist: {}", path.display());
        }
    }
    Ok(())
}

/// Execute the `watch` subcommand.
#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout, clippy::too_many_lines)]
pub async fn execute(args: &WatchArgs, host: Option<&str>) -> Result<()> {
    use std::sync::Arc;
    use std::time::Duration;

    use notify::{EventKind, RecursiveMode, Watcher};

    use crate::container::ContainerClient;
    use crate::events::{EventSink, NdjsonEventSink, NullEventSink};
    use crate::profile;

    let path = input_path(args)?;

    let resolved = profile::resolve_from_args(
        path,
        args.source.profile.as_deref(),
        args.source.r#override.as_deref(),
        args.source.secrets.as_deref(),
    )?;

    let watch_paths = collect_watch_paths(path, args, &resolved);
    validate_watch_paths(&watch_paths)?;

    let debounce = Duration::from_millis(args.debounce);

    // Set up file watcher with tokio channel bridge
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            match event.kind {
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_) => {
                    let _ = tx.send(());
                }
                _ => {}
            }
        }
    })?;

    for path in &watch_paths {
        let watch_target = if path.is_file() {
            path.parent().unwrap_or(path)
        } else {
            path.as_path()
        };
        watcher.watch(watch_target, RecursiveMode::NonRecursive)?;
    }

    let client = Arc::new(ContainerClient::connect(host).await?);
    let event_sink: Box<dyn EventSink> = if args.events {
        Box::new(NdjsonEventSink)
    } else {
        Box::new(NullEventSink)
    };

    println!("Starting watch mode (debounce: {}ms)...", args.debounce);
    println!(
        "Watching: {}",
        watch_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Initial run
    let mut state = match load_and_run_task(args, &client, &*event_sink).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "Initial task startup failed");
            return Err(e);
        }
    };

    println!("Task started. Watching for changes...");

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Received SIGINT, shutting down...");
                break;
            }
            Some(()) = rx.recv() => {
                // Debounce: drain any queued events and wait
                tokio::time::sleep(debounce).await;
                while rx.try_recv().is_ok() {}

                println!("\nFile change detected, restarting...");
                if let Some(server) = state.metadata_server.take() {
                    server.shutdown().await;
                }
                super::task_lifecycle::cleanup(
                    &*client,
                    &state.containers,
                    &state.network,
                    &*event_sink,
                    &state.family,
                )
                .await;

                match load_and_run_task(args, &client, &*event_sink).await {
                    Ok(new_state) => {
                        state = new_state;
                        println!("Task restarted. Watching for changes...");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Task restart failed, waiting for next change...");
                    }
                }
            }
        }
    }

    if let Some(server) = state.metadata_server.take() {
        server.shutdown().await;
    }
    super::task_lifecycle::cleanup(
        &*client,
        &state.containers,
        &state.network,
        &*event_sink,
        &state.family,
    )
    .await;
    println!("Watch mode stopped.");
    Ok(())
}

/// State of a running watch task (containers + optional metadata server).
struct WatchTaskState {
    network: String,
    containers: Vec<(String, String)>,
    family: String,
    metadata_server: Option<crate::metadata::MetadataServer>,
}

/// Load task definition, apply overrides/secrets, start metadata server, and start containers.
#[cfg(not(tarpaulin_include))]
async fn load_and_run_task(
    args: &WatchArgs,
    client: &crate::container::ContainerClient,
    event_sink: &dyn crate::events::EventSink,
) -> Result<WatchTaskState> {
    let task_def = args.source.load_task_def()?;

    // Start metadata server if enabled (mirrors run::execute behavior)
    let (metadata_server, metadata_state) = if args.no_metadata {
        (None, None)
    } else {
        match super::task_lifecycle::start_metadata_server(&task_def).await {
            Ok((server, state)) => (Some(server), Some(state)),
            Err(e) => {
                tracing::warn!(error = %e, "Failed to start metadata server, continuing without it");
                (None, None)
            }
        }
    };

    let metadata_port = metadata_server.as_ref().map(|s| s.port);
    let auth_token = if let Some(state) = &metadata_state {
        Some(state.read().await.auth_token.clone())
    } else {
        None
    };

    let family = task_def.family.clone();
    let (network, containers) = super::task_lifecycle::run_task(
        client,
        &task_def,
        metadata_port,
        auth_token.as_deref(),
        event_sink,
    )
    .await?;

    // Update container IDs in metadata server state
    if let Some(state) = &metadata_state {
        for (id, name) in &containers {
            crate::metadata::update_container_id(state, name, id).await;
        }
    }

    Ok(WatchTaskState {
        network,
        containers,
        family,
        metadata_server,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn make_watch_args(
        task_def: PathBuf,
        override_path: Option<PathBuf>,
        secrets_path: Option<PathBuf>,
        extra_paths: Vec<PathBuf>,
    ) -> WatchArgs {
        use crate::cli::TaskDefSourceArgs;
        WatchArgs {
            source: TaskDefSourceArgs {
                task_definition: Some(task_def),
                from_tf: None,
                tf_resource: None,
                from_cfn: None,
                cfn_resource: None,
                r#override: override_path,
                secrets: secrets_path,
                profile: None,
            },
            no_metadata: false,
            events: false,
            debounce: 500,
            watch_paths: extra_paths,
        }
    }

    fn no_resolved() -> ResolvedPaths {
        ResolvedPaths {
            override_path: None,
            secrets_path: None,
        }
    }

    #[test]
    fn collect_watch_paths_basic() {
        let args = make_watch_args(PathBuf::from("task.json"), None, None, vec![]);
        let paths = collect_watch_paths(
            args.source.task_definition.as_deref().unwrap(),
            &args,
            &no_resolved(),
        );
        assert_eq!(paths, vec![PathBuf::from("task.json")]);
    }

    #[test]
    fn collect_watch_paths_with_override_and_secrets() {
        let args = make_watch_args(
            PathBuf::from("task.json"),
            Some(PathBuf::from("override.json")),
            Some(PathBuf::from("secrets.json")),
            vec![],
        );
        let paths = collect_watch_paths(
            args.source.task_definition.as_deref().unwrap(),
            &args,
            &no_resolved(),
        );
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0], PathBuf::from("task.json"));
        assert_eq!(paths[1], PathBuf::from("override.json"));
        assert_eq!(paths[2], PathBuf::from("secrets.json"));
    }

    #[test]
    fn collect_watch_paths_with_extra_paths() {
        let args = make_watch_args(
            PathBuf::from("task.json"),
            None,
            None,
            vec![PathBuf::from("/app/src"), PathBuf::from("/app/config")],
        );
        let paths = collect_watch_paths(
            args.source.task_definition.as_deref().unwrap(),
            &args,
            &no_resolved(),
        );
        assert_eq!(paths.len(), 3);
        assert!(paths.contains(&PathBuf::from("/app/src")));
        assert!(paths.contains(&PathBuf::from("/app/config")));
    }

    #[test]
    fn collect_watch_paths_with_profile_resolved() {
        let args = make_watch_args(PathBuf::from("task.json"), None, None, vec![]);
        let resolved = ResolvedPaths {
            override_path: Some(PathBuf::from("lecs-override.dev.json")),
            secrets_path: Some(PathBuf::from("secrets.dev.json")),
        };
        let paths = collect_watch_paths(
            args.source.task_definition.as_deref().unwrap(),
            &args,
            &resolved,
        );
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0], PathBuf::from("task.json"));
        assert!(paths.contains(&PathBuf::from("lecs-override.dev.json")));
        assert!(paths.contains(&PathBuf::from("secrets.dev.json")));
    }

    #[test]
    fn collect_watch_paths_profile_deduplicates_cli_args() {
        // CLI flag and profile resolve to the same path — should not duplicate
        let args = make_watch_args(
            PathBuf::from("task.json"),
            Some(PathBuf::from("override.json")),
            None,
            vec![],
        );
        let resolved = ResolvedPaths {
            override_path: Some(PathBuf::from("override.json")),
            secrets_path: None,
        };
        let paths = collect_watch_paths(
            args.source.task_definition.as_deref().unwrap(),
            &args,
            &resolved,
        );
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from("task.json"));
        assert_eq!(paths[1], PathBuf::from("override.json"));
    }

    fn make_watch_args_from_tf(
        tf_path: PathBuf,
        tf_resource: Option<String>,
        override_path: Option<PathBuf>,
        secrets_path: Option<PathBuf>,
        extra_paths: Vec<PathBuf>,
    ) -> WatchArgs {
        use crate::cli::TaskDefSourceArgs;
        WatchArgs {
            source: TaskDefSourceArgs {
                task_definition: None,
                from_tf: Some(tf_path),
                tf_resource,
                from_cfn: None,
                cfn_resource: None,
                r#override: override_path,
                secrets: secrets_path,
                profile: None,
            },
            no_metadata: false,
            events: false,
            debounce: 500,
            watch_paths: extra_paths,
        }
    }

    #[test]
    fn collect_watch_paths_from_tf() {
        let args = make_watch_args_from_tf(PathBuf::from("plan.json"), None, None, None, vec![]);
        let paths = collect_watch_paths(
            args.source.from_tf.as_deref().unwrap(),
            &args,
            &no_resolved(),
        );
        assert_eq!(paths, vec![PathBuf::from("plan.json")]);
    }

    #[test]
    fn collect_watch_paths_from_tf_with_override_and_secrets() {
        let args = make_watch_args_from_tf(
            PathBuf::from("plan.json"),
            Some("aws_ecs_task_definition.app".to_string()),
            Some(PathBuf::from("override.json")),
            Some(PathBuf::from("secrets.json")),
            vec![PathBuf::from("/app/src")],
        );
        let paths = collect_watch_paths(
            args.source.from_tf.as_deref().unwrap(),
            &args,
            &no_resolved(),
        );
        assert_eq!(paths.len(), 4);
        assert_eq!(paths[0], PathBuf::from("plan.json"));
        assert_eq!(paths[1], PathBuf::from("override.json"));
        assert_eq!(paths[2], PathBuf::from("secrets.json"));
        assert_eq!(paths[3], PathBuf::from("/app/src"));
    }

    #[test]
    fn collect_watch_paths_from_tf_deduplicates_profile() {
        let args = make_watch_args_from_tf(
            PathBuf::from("plan.json"),
            None,
            Some(PathBuf::from("override.json")),
            None,
            vec![],
        );
        let resolved = ResolvedPaths {
            override_path: Some(PathBuf::from("override.json")),
            secrets_path: None,
        };
        let paths = collect_watch_paths(args.source.from_tf.as_deref().unwrap(), &args, &resolved);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from("plan.json"));
        assert_eq!(paths[1], PathBuf::from("override.json"));
    }

    #[test]
    fn input_path_from_task_definition() {
        let args = make_watch_args(PathBuf::from("task.json"), None, None, vec![]);
        let path = input_path(&args).unwrap();
        assert_eq!(path, std::path::Path::new("task.json"));
    }

    #[test]
    fn input_path_from_tf() {
        let args = make_watch_args_from_tf(PathBuf::from("plan.json"), None, None, None, vec![]);
        let path = input_path(&args).unwrap();
        assert_eq!(path, std::path::Path::new("plan.json"));
    }

    #[test]
    fn input_path_from_cfn() {
        use crate::cli::TaskDefSourceArgs;
        let args = WatchArgs {
            source: TaskDefSourceArgs {
                task_definition: None,
                from_tf: None,
                tf_resource: None,
                from_cfn: Some(PathBuf::from("template.json")),
                cfn_resource: None,
                r#override: None,
                secrets: None,
                profile: None,
            },
            no_metadata: false,
            events: false,
            debounce: 500,
            watch_paths: vec![],
        };
        let path = input_path(&args).unwrap();
        assert_eq!(path, std::path::Path::new("template.json"));
    }

    #[test]
    fn input_path_none_errors() {
        use crate::cli::TaskDefSourceArgs;
        let args = WatchArgs {
            source: TaskDefSourceArgs {
                task_definition: None,
                from_tf: None,
                tf_resource: None,
                from_cfn: None,
                cfn_resource: None,
                r#override: None,
                secrets: None,
                profile: None,
            },
            no_metadata: false,
            events: false,
            debounce: 500,
            watch_paths: vec![],
        };
        let result = input_path(&args);
        assert!(result.is_err());
    }

    #[test]
    fn validate_watch_paths_missing_file() {
        let paths = vec![PathBuf::from("/nonexistent/path/file.json")];
        let result = validate_watch_paths(&paths);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("does not exist"));
    }

    #[test]
    fn validate_watch_paths_all_exist() {
        let dir = std::env::temp_dir();
        let result = validate_watch_paths(&[dir]);
        assert!(result.is_ok());
    }
}
