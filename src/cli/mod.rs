//! CLI command definitions and argument parsing.

pub mod logs;
pub mod ps;
pub mod run;
pub mod stop;
pub mod version;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Egret - Local ECS task runner
#[derive(Parser)]
#[command(name = "egret", about = "Run ECS task definitions locally")]
pub struct Cli {
    /// Container runtime socket URL (e.g., `unix:///run/podman/podman.sock`)
    #[arg(long, global = true, env = "CONTAINER_HOST")]
    pub host: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run an ECS task definition locally
    Run(RunArgs),
    /// Stop a running local task
    Stop(StopArgs),
    /// List running tasks
    Ps(PsArgs),
    /// Show logs for a container
    Logs(LogsArgs),
    /// Show version information
    Version,
}

#[derive(Parser)]
pub struct RunArgs {
    /// Path to ECS task definition JSON file
    #[arg(short = 'f', long = "task-definition")]
    pub task_definition: PathBuf,

    /// Path to local override file
    #[arg(short, long)]
    pub r#override: Option<PathBuf>,

    /// Path to local secrets mapping file
    #[arg(short, long)]
    pub secrets: Option<PathBuf>,

    /// Disable the ECS metadata/credentials sidecar server
    #[arg(long)]
    pub no_metadata: bool,
}

#[derive(Parser)]
pub struct StopArgs {
    /// Task name or ID to stop
    pub task: Option<String>,

    /// Stop all running tasks
    #[arg(long)]
    pub all: bool,
}

#[derive(Parser)]
pub struct PsArgs {
    /// Filter by task family name
    pub task: Option<String>,
}

#[derive(Parser)]
pub struct LogsArgs {
    /// Container name (e.g., "app" or "my-task-app")
    pub container: String,

    /// Follow log output (like tail -f)
    #[arg(short, long)]
    pub follow: bool,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn parse_version_command() {
        let cli = Cli::try_parse_from(["egret", "version"]).expect("should parse");
        assert!(matches!(cli.command, Command::Version));
    }

    #[test]
    fn parse_run_command() {
        let cli = Cli::try_parse_from(["egret", "run", "-f", "task.json"]).expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert_eq!(args.task_definition.to_str(), Some("task.json"));
                assert!(args.r#override.is_none());
                assert!(args.secrets.is_none());
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_with_override_and_secrets() {
        let cli = Cli::try_parse_from([
            "egret",
            "run",
            "-f",
            "task.json",
            "--override",
            "override.json",
            "--secrets",
            "secrets.json",
        ])
        .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert_eq!(args.task_definition.to_str(), Some("task.json"));
                assert_eq!(
                    args.r#override.as_ref().unwrap().to_str(),
                    Some("override.json")
                );
                assert_eq!(
                    args.secrets.as_ref().unwrap().to_str(),
                    Some("secrets.json")
                );
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_with_host_flag() {
        let cli = Cli::try_parse_from([
            "egret",
            "--host",
            "unix:///run/podman/podman.sock",
            "run",
            "-f",
            "task.json",
        ])
        .expect("should parse");
        assert_eq!(cli.host.as_deref(), Some("unix:///run/podman/podman.sock"));
    }

    #[test]
    fn parse_host_flag_is_optional() {
        let cli = Cli::try_parse_from(["egret", "run", "-f", "task.json"]).expect("should parse");
        assert!(cli.host.is_none());
    }

    #[test]
    fn parse_host_with_tcp() {
        let cli = Cli::try_parse_from([
            "egret",
            "--host",
            "tcp://localhost:2375",
            "run",
            "-f",
            "task.json",
        ])
        .expect("should parse");
        assert_eq!(cli.host.as_deref(), Some("tcp://localhost:2375"));
    }

    #[test]
    fn parse_stop_all() {
        let cli = Cli::try_parse_from(["egret", "stop", "--all"]).expect("should parse");
        match cli.command {
            Command::Stop(args) => {
                assert!(args.all);
                assert!(args.task.is_none());
            }
            _ => panic!("expected Stop command"),
        }
    }

    #[test]
    fn parse_ps_no_args() {
        let cli = Cli::try_parse_from(["egret", "ps"]).expect("should parse");
        match cli.command {
            Command::Ps(args) => {
                assert!(args.task.is_none());
            }
            _ => panic!("expected Ps command"),
        }
    }

    #[test]
    fn parse_ps_with_task_filter() {
        let cli = Cli::try_parse_from(["egret", "ps", "my-app"]).expect("should parse");
        match cli.command {
            Command::Ps(args) => {
                assert_eq!(args.task.as_deref(), Some("my-app"));
            }
            _ => panic!("expected Ps command"),
        }
    }

    #[test]
    fn parse_logs_command() {
        let cli = Cli::try_parse_from(["egret", "logs", "app"]).expect("should parse");
        match cli.command {
            Command::Logs(args) => {
                assert_eq!(args.container, "app");
                assert!(!args.follow);
            }
            _ => panic!("expected Logs command"),
        }
    }

    #[test]
    fn parse_logs_with_follow() {
        let cli = Cli::try_parse_from(["egret", "logs", "app", "--follow"]).expect("should parse");
        match cli.command {
            Command::Logs(args) => {
                assert_eq!(args.container, "app");
                assert!(args.follow);
            }
            _ => panic!("expected Logs command"),
        }
    }

    #[test]
    fn parse_logs_with_short_follow() {
        let cli = Cli::try_parse_from(["egret", "logs", "app", "-f"]).expect("should parse");
        match cli.command {
            Command::Logs(args) => {
                assert_eq!(args.container, "app");
                assert!(args.follow);
            }
            _ => panic!("expected Logs command"),
        }
    }
}
