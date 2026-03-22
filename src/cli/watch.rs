//! `egret watch` command implementation.
//!
//! Watches task definition, override, and secrets files for changes
//! and automatically restarts the task on modification.

use std::path::PathBuf;

use anyhow::{Result, bail};

use super::WatchArgs;

/// Collect all paths that should be watched for changes.
pub fn collect_watch_paths(args: &WatchArgs) -> Vec<PathBuf> {
    let mut paths = vec![args.task_definition.clone()];
    if let Some(ref p) = args.r#override {
        paths.push(p.clone());
    }
    if let Some(ref p) = args.secrets {
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
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use notify::{EventKind, RecursiveMode, Watcher};

    use crate::container::ContainerClient;
    use crate::events::{EventSink, NdjsonEventSink, NullEventSink};
    use crate::profile;

    let base_dir = args
        .task_definition
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let config = profile::load_config_with_warning(base_dir);
    let effective_profile = profile::effective_profile(args.profile.as_deref(), config.as_ref());

    let watch_paths = collect_watch_paths(args);
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
    let load_result =
        load_and_run_task(args, base_dir, effective_profile, &client, &*event_sink).await;
    let (mut network, mut containers) = match load_result {
        Ok(result) => result,
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
                super::run::cleanup(&*client, &containers, &network, &*event_sink, "watch").await;

                match load_and_run_task(args, base_dir, effective_profile, &client, &*event_sink).await {
                    Ok((new_network, new_containers)) => {
                        network = new_network;
                        containers = new_containers;
                        println!("Task restarted. Watching for changes...");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Task restart failed, waiting for next change...");
                    }
                }
            }
        }
    }

    super::run::cleanup(&*client, &containers, &network, &*event_sink, "watch").await;
    println!("Watch mode stopped.");
    Ok(())
}

/// Load task definition, apply overrides/secrets, and start containers.
#[cfg(not(tarpaulin_include))]
async fn load_and_run_task(
    args: &WatchArgs,
    base_dir: &std::path::Path,
    effective_profile: Option<&str>,
    client: &crate::container::ContainerClient,
    event_sink: &dyn crate::events::EventSink,
) -> Result<(String, Vec<(String, String)>)> {
    use crate::overrides::OverrideConfig;
    use crate::secrets::SecretsResolver;
    use crate::taskdef::{Environment, TaskDefinition};

    let resolved = crate::profile::resolve(
        base_dir,
        effective_profile,
        args.r#override.as_deref(),
        args.secrets.as_deref(),
    )?;

    let mut task_def = TaskDefinition::from_file(&args.task_definition)?;

    if let Some(override_path) = &resolved.override_path {
        let override_config = OverrideConfig::from_file(override_path)?;
        override_config.apply(&mut task_def);
    }

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
    } else if has_secrets {
        tracing::warn!("Task definition has secrets but --secrets flag was not provided.");
    }

    super::run::run_task(client, &task_def, None, None, event_sink).await
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
        WatchArgs {
            task_definition: task_def,
            r#override: override_path,
            secrets: secrets_path,
            profile: None,
            no_metadata: false,
            events: false,
            debounce: 500,
            watch_paths: extra_paths,
        }
    }

    #[test]
    fn collect_watch_paths_basic() {
        let args = make_watch_args(PathBuf::from("task.json"), None, None, vec![]);
        let paths = collect_watch_paths(&args);
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
        let paths = collect_watch_paths(&args);
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
        let paths = collect_watch_paths(&args);
        assert_eq!(paths.len(), 3);
        assert!(paths.contains(&PathBuf::from("/app/src")));
        assert!(paths.contains(&PathBuf::from("/app/config")));
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
