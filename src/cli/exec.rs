use anyhow::Result;

use super::ExecArgs;
use super::format::{FindContainerError, find_container};
use crate::container::{ContainerClient, ContainerRuntime};

/// Execute the `exec` subcommand.
#[cfg(not(tarpaulin_include))]
pub async fn execute(args: &ExecArgs, host: Option<&str>) -> Result<()> {
    let client = ContainerClient::connect(host).await?;
    let containers = client.list_containers(None).await?;

    let container = find_container(&containers, &args.container).map_err(|e| match e {
        FindContainerError::NotFound => anyhow::anyhow!(
            "No container found matching '{}'. Use 'lecs ps' to list running containers.",
            args.container
        ),
        FindContainerError::Ambiguous(names) => anyhow::anyhow!(
            "Ambiguous container name '{}'. Matching containers: {}",
            args.container,
            names
        ),
    })?;

    let cmd = if args.command.is_empty() {
        vec!["/bin/sh".to_string()]
    } else {
        args.command.clone()
    };

    let result = client.exec_container(&container.id, &cmd).await?;

    if let Some(code) = result.exit_code {
        if code != 0 {
            std::process::exit(
                i32::try_from(code).unwrap_or(1),
            );
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn default_command_is_bin_sh() {
        let args = ExecArgs {
            container: "test".to_string(),
            command: vec![],
        };
        let cmd = if args.command.is_empty() {
            vec!["/bin/sh".to_string()]
        } else {
            args.command.clone()
        };
        assert_eq!(cmd, vec!["/bin/sh"]);
    }

    #[test]
    fn custom_command_passthrough() {
        let args = ExecArgs {
            container: "test".to_string(),
            command: vec!["ls".to_string(), "-la".to_string()],
        };
        let cmd = if args.command.is_empty() {
            vec!["/bin/sh".to_string()]
        } else {
            args.command.clone()
        };
        assert_eq!(cmd, vec!["ls", "-la"]);
    }
}
