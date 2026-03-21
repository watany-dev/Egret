use anyhow::Result;

use super::StopArgs;
use crate::docker::{DockerApi, DockerClient};

/// Execute the `stop` subcommand.
#[allow(clippy::print_stdout)]
pub async fn execute(args: &StopArgs) -> Result<()> {
    let task_filter = if args.all {
        None
    } else if let Some(task) = &args.task {
        Some(task.as_str())
    } else {
        anyhow::bail!("Specify a task name or use --all to stop all tasks.");
    };

    // Connect to Docker
    let client = DockerClient::connect().await?;

    // Stop and remove containers (best-effort)
    let containers = client.list_containers(task_filter).await?;
    if containers.is_empty() {
        println!("No egret containers found.");
        return Ok(());
    }

    for container in &containers {
        if let Err(e) = client.stop_container(&container.id).await {
            tracing::warn!(container = %container.name, error = %e, "Failed to stop container");
        }
        if let Err(e) = client.remove_container(&container.id).await {
            tracing::warn!(container = %container.name, error = %e, "Failed to remove container");
        }
        println!("Stopped: {}", container.name);
    }

    // Remove networks
    let networks = client.list_networks(task_filter).await?;
    for network in &networks {
        if let Err(e) = client.remove_network(&network.name).await {
            tracing::warn!(network = %network.name, error = %e, "Failed to remove network");
        }
        println!("Removed network: {}", network.name);
    }

    Ok(())
}
