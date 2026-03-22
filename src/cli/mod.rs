//! CLI command definitions and argument parsing.

pub mod completions;
pub mod diff;
pub mod history;
pub mod init;
pub mod inspect;
pub mod logs;
pub mod ps;
pub mod run;
pub mod stats;
pub mod stop;
pub mod validate;
pub mod version;
pub mod watch;

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
    /// Generate starter files for a new Egret project
    Init(InitArgs),
    /// Validate task definition and related files
    Validate(ValidateArgs),
    /// Inspect a running task's configuration
    Inspect(InspectArgs),
    /// Show live resource usage statistics
    Stats(StatsArgs),
    /// Show execution history
    History(HistoryArgs),
    /// Show version information
    Version,
    /// Generate shell completion scripts
    Completions(CompletionsArgs),
    /// Compare two task definition files semantically
    Diff(DiffArgs),
    /// Watch files and auto-restart on changes
    Watch(WatchArgs),
}

#[derive(Parser)]
pub struct CompletionsArgs {
    /// Shell type (bash, zsh, fish)
    pub shell: clap_complete::Shell,
}

#[derive(Parser)]
pub struct DiffArgs {
    /// First task definition file
    pub file1: PathBuf,
    /// Second task definition file
    pub file2: PathBuf,
    /// Disable colored output
    #[arg(long)]
    pub no_color: bool,
}

#[derive(Parser)]
pub struct WatchArgs {
    /// Path to ECS task definition JSON file
    #[arg(short = 'f', long = "task-definition")]
    pub task_definition: PathBuf,

    /// Path to local override file
    #[arg(short, long)]
    pub r#override: Option<PathBuf>,

    /// Path to local secrets mapping file
    #[arg(short, long)]
    pub secrets: Option<PathBuf>,

    /// Profile name for loading convention-based override/secrets files
    #[arg(short, long)]
    pub profile: Option<String>,

    /// Disable the ECS metadata/credentials sidecar server
    #[arg(long)]
    pub no_metadata: bool,

    /// Output lifecycle events as NDJSON to stderr
    #[arg(long)]
    pub events: bool,

    /// Debounce interval in milliseconds
    #[arg(long, default_value_t = 500)]
    pub debounce: u64,

    /// Additional paths to watch for changes (repeatable)
    #[arg(long = "watch-path")]
    pub watch_paths: Vec<PathBuf>,
}

#[derive(Parser)]
pub struct HistoryArgs {
    /// Clear all history
    #[arg(long)]
    pub clear: bool,
}

#[derive(Parser)]
pub struct InspectArgs {
    /// Task family name to inspect
    pub family: String,
}

#[derive(Parser)]
pub struct StatsArgs {
    /// Filter by task family name
    pub family: Option<String>,
}

#[derive(Parser)]
pub struct ValidateArgs {
    /// Path to ECS task definition JSON file
    #[arg(short = 'f', long = "task-definition")]
    pub task_definition: PathBuf,

    /// Path to local override file (optional, validates cross-references)
    #[arg(short, long)]
    pub r#override: Option<PathBuf>,

    /// Path to local secrets mapping file (optional)
    #[arg(short, long)]
    pub secrets: Option<PathBuf>,

    /// Profile name for loading convention-based override/secrets files
    #[arg(short, long)]
    pub profile: Option<String>,
}

#[derive(Parser)]
pub struct InitArgs {
    /// Output directory (default: current directory)
    #[arg(short, long, default_value = ".")]
    pub dir: PathBuf,

    /// Container image for the initial container definition
    #[arg(long, default_value = "nginx:latest")]
    pub image: String,

    /// Task family name
    #[arg(long, default_value = "my-app")]
    pub family: String,
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

    /// Profile name for loading convention-based override/secrets files
    #[arg(short, long)]
    pub profile: Option<String>,

    /// Disable the ECS metadata/credentials sidecar server
    #[arg(long)]
    pub no_metadata: bool,

    /// Validate and display configuration without starting containers
    #[arg(long)]
    pub dry_run: bool,

    /// Output lifecycle events as NDJSON to stderr
    #[arg(long)]
    pub events: bool,
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

    /// Output format: table (default), json
    #[arg(short, long, value_enum, default_value_t)]
    pub output: OutputFormat,
}

/// Output format for the `ps` command.
#[derive(Clone, Default, clap::ValueEnum)]
pub enum OutputFormat {
    /// Default table view
    #[default]
    Table,
    /// JSON output
    Json,
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
    fn parse_run_with_events() {
        let cli = Cli::try_parse_from(["egret", "run", "-f", "task.json", "--events"])
            .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert!(args.events);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_without_events() {
        let cli = Cli::try_parse_from(["egret", "run", "-f", "task.json"]).expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert!(!args.events);
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
                assert!(matches!(args.output, OutputFormat::Table));
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
    fn parse_ps_with_json_output() {
        let cli = Cli::try_parse_from(["egret", "ps", "--output", "json"]).expect("should parse");
        match cli.command {
            Command::Ps(args) => {
                assert!(matches!(args.output, OutputFormat::Json));
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

    #[test]
    fn parse_validate_command() {
        let cli =
            Cli::try_parse_from(["egret", "validate", "-f", "task.json"]).expect("should parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(args.task_definition.to_str(), Some("task.json"));
                assert!(args.r#override.is_none());
                assert!(args.secrets.is_none());
            }
            _ => panic!("expected Validate command"),
        }
    }

    #[test]
    fn parse_validate_with_override() {
        let cli = Cli::try_parse_from([
            "egret",
            "validate",
            "-f",
            "task.json",
            "--override",
            "override.json",
        ])
        .expect("should parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(
                    args.r#override.as_ref().unwrap().to_str(),
                    Some("override.json")
                );
            }
            _ => panic!("expected Validate command"),
        }
    }

    #[test]
    fn parse_validate_with_secrets() {
        let cli = Cli::try_parse_from([
            "egret",
            "validate",
            "-f",
            "task.json",
            "--secrets",
            "secrets.json",
        ])
        .expect("should parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(
                    args.secrets.as_ref().unwrap().to_str(),
                    Some("secrets.json")
                );
            }
            _ => panic!("expected Validate command"),
        }
    }

    #[test]
    fn parse_inspect_command() {
        let cli = Cli::try_parse_from(["egret", "inspect", "my-app"]).expect("should parse");
        match cli.command {
            Command::Inspect(args) => {
                assert_eq!(args.family, "my-app");
            }
            _ => panic!("expected Inspect command"),
        }
    }

    #[test]
    fn parse_stats_command_defaults() {
        let cli = Cli::try_parse_from(["egret", "stats"]).expect("should parse");
        match cli.command {
            Command::Stats(args) => {
                assert!(args.family.is_none());
            }
            _ => panic!("expected Stats command"),
        }
    }

    #[test]
    fn parse_stats_with_family() {
        let cli = Cli::try_parse_from(["egret", "stats", "my-app"]).expect("should parse");
        match cli.command {
            Command::Stats(args) => {
                assert_eq!(args.family.as_deref(), Some("my-app"));
            }
            _ => panic!("expected Stats command"),
        }
    }

    #[test]
    fn parse_history_command() {
        let cli = Cli::try_parse_from(["egret", "history"]).expect("should parse");
        match cli.command {
            Command::History(args) => {
                assert!(!args.clear);
            }
            _ => panic!("expected History command"),
        }
    }

    #[test]
    fn parse_history_with_clear() {
        let cli = Cli::try_parse_from(["egret", "history", "--clear"]).expect("should parse");
        match cli.command {
            Command::History(args) => {
                assert!(args.clear);
            }
            _ => panic!("expected History command"),
        }
    }

    #[test]
    fn parse_diff_command() {
        let cli = Cli::try_parse_from(["egret", "diff", "a.json", "b.json"]).expect("should parse");
        match cli.command {
            Command::Diff(args) => {
                assert_eq!(args.file1.to_str(), Some("a.json"));
                assert_eq!(args.file2.to_str(), Some("b.json"));
                assert!(!args.no_color);
            }
            _ => panic!("expected Diff command"),
        }
    }

    #[test]
    fn parse_diff_with_no_color() {
        let cli = Cli::try_parse_from(["egret", "diff", "--no-color", "a.json", "b.json"])
            .expect("should parse");
        match cli.command {
            Command::Diff(args) => {
                assert!(args.no_color);
            }
            _ => panic!("expected Diff command"),
        }
    }

    #[test]
    fn parse_completions_bash() {
        let cli = Cli::try_parse_from(["egret", "completions", "bash"]).expect("should parse");
        match cli.command {
            Command::Completions(args) => {
                assert_eq!(args.shell, clap_complete::Shell::Bash);
            }
            _ => panic!("expected Completions command"),
        }
    }

    #[test]
    fn parse_completions_zsh() {
        let cli = Cli::try_parse_from(["egret", "completions", "zsh"]).expect("should parse");
        match cli.command {
            Command::Completions(args) => {
                assert_eq!(args.shell, clap_complete::Shell::Zsh);
            }
            _ => panic!("expected Completions command"),
        }
    }

    #[test]
    fn parse_completions_fish() {
        let cli = Cli::try_parse_from(["egret", "completions", "fish"]).expect("should parse");
        match cli.command {
            Command::Completions(args) => {
                assert_eq!(args.shell, clap_complete::Shell::Fish);
            }
            _ => panic!("expected Completions command"),
        }
    }

    #[test]
    fn parse_init_command_defaults() {
        let cli = Cli::try_parse_from(["egret", "init"]).expect("should parse");
        match cli.command {
            Command::Init(args) => {
                assert_eq!(args.dir.to_str(), Some("."));
                assert_eq!(args.image, "nginx:latest");
                assert_eq!(args.family, "my-app");
            }
            _ => panic!("expected Init command"),
        }
    }

    #[test]
    fn parse_run_with_profile() {
        let cli = Cli::try_parse_from(["egret", "run", "-f", "task.json", "--profile", "dev"])
            .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert_eq!(args.profile.as_deref(), Some("dev"));
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_with_short_profile() {
        let cli = Cli::try_parse_from(["egret", "run", "-f", "task.json", "-p", "staging"])
            .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert_eq!(args.profile.as_deref(), Some("staging"));
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_profile_with_override() {
        let cli = Cli::try_parse_from([
            "egret",
            "run",
            "-f",
            "task.json",
            "--profile",
            "dev",
            "--override",
            "custom.json",
        ])
        .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert_eq!(args.profile.as_deref(), Some("dev"));
                assert_eq!(
                    args.r#override.as_ref().unwrap().to_str(),
                    Some("custom.json")
                );
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_without_profile() {
        let cli = Cli::try_parse_from(["egret", "run", "-f", "task.json"]).expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert!(args.profile.is_none());
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_validate_with_profile() {
        let cli = Cli::try_parse_from(["egret", "validate", "-f", "task.json", "--profile", "dev"])
            .expect("should parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(args.profile.as_deref(), Some("dev"));
            }
            _ => panic!("expected Validate command"),
        }
    }

    #[test]
    fn parse_init_with_flags() {
        let cli = Cli::try_parse_from([
            "egret",
            "init",
            "--image",
            "node:20",
            "--family",
            "web-service",
            "--dir",
            "/tmp/project",
        ])
        .expect("should parse");
        match cli.command {
            Command::Init(args) => {
                assert_eq!(args.dir.to_str(), Some("/tmp/project"));
                assert_eq!(args.image, "node:20");
                assert_eq!(args.family, "web-service");
            }
            _ => panic!("expected Init command"),
        }
    }

    #[test]
    fn parse_watch_command() {
        let cli = Cli::try_parse_from(["egret", "watch", "-f", "task.json"]).expect("should parse");
        match cli.command {
            Command::Watch(args) => {
                assert_eq!(args.task_definition.to_str(), Some("task.json"));
                assert_eq!(args.debounce, 500);
                assert!(args.watch_paths.is_empty());
                assert!(!args.no_metadata);
                assert!(!args.events);
            }
            _ => panic!("expected Watch command"),
        }
    }

    #[test]
    fn parse_watch_with_debounce() {
        let cli = Cli::try_parse_from(["egret", "watch", "-f", "task.json", "--debounce", "1000"])
            .expect("should parse");
        match cli.command {
            Command::Watch(args) => {
                assert_eq!(args.debounce, 1000);
            }
            _ => panic!("expected Watch command"),
        }
    }

    #[test]
    fn parse_watch_with_watch_paths() {
        let cli = Cli::try_parse_from([
            "egret",
            "watch",
            "-f",
            "task.json",
            "--watch-path",
            "/app/src",
            "--watch-path",
            "/app/config",
        ])
        .expect("should parse");
        match cli.command {
            Command::Watch(args) => {
                assert_eq!(args.watch_paths.len(), 2);
                assert_eq!(args.watch_paths[0].to_str(), Some("/app/src"));
                assert_eq!(args.watch_paths[1].to_str(), Some("/app/config"));
            }
            _ => panic!("expected Watch command"),
        }
    }
}
