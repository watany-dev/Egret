//! CLI command definitions and argument parsing.

pub mod completions;
pub mod exec;
pub mod format;
pub mod init;
pub mod inspect;
pub mod logs;
pub mod ps;
pub mod run;
pub mod stats;
pub mod stop;
pub mod task_lifecycle;
pub mod validate;
pub mod version;
pub mod watch;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Lecs - Local ECS task runner
#[derive(Parser)]
#[command(name = "lecs", about = "Run ECS task definitions locally")]
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
    /// Generate starter files for a new Lecs project
    Init(InitArgs),
    /// Validate task definition and related files
    Validate(ValidateArgs),
    /// Inspect a running task's configuration
    Inspect(InspectArgs),
    /// Show live resource usage statistics
    Stats(StatsArgs),
    /// Show version information
    Version,
    /// Generate shell completion scripts
    Completions(CompletionsArgs),
    /// Watch files and auto-restart on changes
    Watch(WatchArgs),
    /// Execute a command in a running container
    Exec(ExecArgs),
}

#[derive(Parser)]
pub struct CompletionsArgs {
    /// Shell type (bash, zsh, fish)
    pub shell: clap_complete::Shell,
}

#[derive(Parser)]
pub struct ExecArgs {
    /// Container name (exact or partial match)
    pub container: String,
    /// Command to execute (default: /bin/sh)
    #[arg(last = true)]
    pub command: Vec<String>,
}

/// Common arguments for specifying the task definition source, overrides, secrets, and profile.
///
/// Shared by `run`, `validate`, and `watch` commands via `#[command(flatten)]`.
#[derive(Parser)]
pub struct TaskDefSourceArgs {
    /// Path to ECS task definition JSON file
    #[arg(
        short = 'f',
        long = "task-definition",
        conflicts_with_all = ["from_tf", "from_cfn"],
        required_unless_present_any = ["from_tf", "from_cfn"]
    )]
    pub task_definition: Option<PathBuf>,

    /// Path to Terraform show JSON file (alternative to --task-definition)
    #[arg(long = "from-tf", conflicts_with_all = ["task_definition", "from_cfn"])]
    pub from_tf: Option<PathBuf>,

    /// Terraform resource address (required when multiple ECS task definitions exist)
    #[arg(long = "tf-resource", requires = "from_tf")]
    pub tf_resource: Option<String>,

    /// Path to `CloudFormation` template JSON file (alternative to --task-definition)
    #[arg(long = "from-cfn", conflicts_with_all = ["task_definition", "from_tf"])]
    pub from_cfn: Option<PathBuf>,

    /// `CloudFormation` logical resource ID (required when multiple ECS task definitions exist)
    #[arg(long = "cfn-resource", requires = "from_cfn")]
    pub cfn_resource: Option<String>,

    /// Path to local override file
    #[arg(short, long)]
    pub r#override: Option<PathBuf>,

    /// Path to local secrets mapping file
    #[arg(short, long)]
    pub secrets: Option<PathBuf>,

    /// Profile name for loading convention-based override/secrets files
    #[arg(short, long)]
    pub profile: Option<String>,
}

impl TaskDefSourceArgs {
    /// Return whichever input file path was provided.
    pub fn input_path(&self) -> anyhow::Result<&std::path::Path> {
        self.task_definition
            .as_deref()
            .or(self.from_tf.as_deref())
            .or(self.from_cfn.as_deref())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "either --task-definition, --from-tf, or --from-cfn must be provided"
                )
            })
    }

    /// Parse the task definition from the selected input source.
    pub fn parse_task_def(&self) -> anyhow::Result<crate::taskdef::TaskDefinition> {
        use crate::taskdef::{TaskDefinition, cloudformation, terraform};

        if let Some(tf_path) = &self.from_tf {
            Ok(terraform::from_terraform_file(
                tf_path,
                self.tf_resource.as_deref(),
            )?)
        } else if let Some(cfn_path) = &self.from_cfn {
            Ok(cloudformation::from_cfn_file(
                cfn_path,
                self.cfn_resource.as_deref(),
            )?)
        } else {
            let path = self.input_path()?;
            Ok(TaskDefinition::from_file(path)?)
        }
    }

    /// Full pipeline: parse → resolve profile → apply overrides → resolve secrets.
    pub fn load_task_def(&self) -> anyhow::Result<crate::taskdef::TaskDefinition> {
        use crate::overrides::OverrideConfig;
        use crate::secrets::SecretsResolver;
        use crate::taskdef::Environment;

        let input_path = self.input_path()?;
        let resolved = crate::profile::resolve_from_args(
            input_path,
            self.profile.as_deref(),
            self.r#override.as_deref(),
            self.secrets.as_deref(),
        )?;

        let mut task_def = self.parse_task_def()?;

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

        // Load environment files (.env format) if specified in container definitions
        let base_dir = input_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        for container in &mut task_def.container_definitions {
            if !container.environment_files.is_empty() {
                let env_vars = crate::taskdef::load_environment_files(
                    &container.environment_files,
                    base_dir,
                )?;
                // environmentFiles are loaded first; explicit environment entries override them.
                // We prepend env file vars so that container.environment (appended later) wins.
                let existing: Vec<Environment> = std::mem::take(&mut container.environment);
                for (name, value) in env_vars {
                    container.environment.push(Environment { name, value });
                }
                // Re-append existing environment entries so they take precedence
                container.environment.extend(existing);
                tracing::info!(
                    container = %container.name,
                    "Loaded environment files"
                );
            }
        }

        Ok(task_def)
    }
}

#[derive(Parser)]
pub struct WatchArgs {
    /// Task definition source, overrides, secrets, and profile
    #[command(flatten)]
    pub source: TaskDefSourceArgs,

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
    /// Task definition source, overrides, secrets, and profile
    #[command(flatten)]
    pub source: TaskDefSourceArgs,
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
    /// Task definition source, overrides, secrets, and profile
    #[command(flatten)]
    pub source: TaskDefSourceArgs,

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
        let cli = Cli::try_parse_from(["lecs", "version"]).expect("should parse");
        assert!(matches!(cli.command, Command::Version));
    }

    #[test]
    fn parse_run_command() {
        let cli = Cli::try_parse_from(["lecs", "run", "-f", "task.json"]).expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert_eq!(
                    args.source.task_definition.as_ref().unwrap().to_str(),
                    Some("task.json")
                );
                assert!(args.source.r#override.is_none());
                assert!(args.source.secrets.is_none());
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_with_events() {
        let cli = Cli::try_parse_from(["lecs", "run", "-f", "task.json", "--events"])
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
        let cli = Cli::try_parse_from(["lecs", "run", "-f", "task.json"]).expect("should parse");
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
            "lecs",
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
                assert_eq!(
                    args.source.task_definition.as_ref().unwrap().to_str(),
                    Some("task.json")
                );
                assert_eq!(
                    args.source.r#override.as_ref().unwrap().to_str(),
                    Some("override.json")
                );
                assert_eq!(
                    args.source.secrets.as_ref().unwrap().to_str(),
                    Some("secrets.json")
                );
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_with_host_flag() {
        let cli = Cli::try_parse_from([
            "lecs",
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
        let cli = Cli::try_parse_from(["lecs", "run", "-f", "task.json"]).expect("should parse");
        assert!(cli.host.is_none());
    }

    #[test]
    fn parse_host_with_tcp() {
        let cli = Cli::try_parse_from([
            "lecs",
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
        let cli = Cli::try_parse_from(["lecs", "stop", "--all"]).expect("should parse");
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
        let cli = Cli::try_parse_from(["lecs", "ps"]).expect("should parse");
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
        let cli = Cli::try_parse_from(["lecs", "ps", "my-app"]).expect("should parse");
        match cli.command {
            Command::Ps(args) => {
                assert_eq!(args.task.as_deref(), Some("my-app"));
            }
            _ => panic!("expected Ps command"),
        }
    }

    #[test]
    fn parse_ps_with_json_output() {
        let cli = Cli::try_parse_from(["lecs", "ps", "--output", "json"]).expect("should parse");
        match cli.command {
            Command::Ps(args) => {
                assert!(matches!(args.output, OutputFormat::Json));
            }
            _ => panic!("expected Ps command"),
        }
    }

    #[test]
    fn parse_logs_command() {
        let cli = Cli::try_parse_from(["lecs", "logs", "app"]).expect("should parse");
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
        let cli = Cli::try_parse_from(["lecs", "logs", "app", "--follow"]).expect("should parse");
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
        let cli = Cli::try_parse_from(["lecs", "logs", "app", "-f"]).expect("should parse");
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
            Cli::try_parse_from(["lecs", "validate", "-f", "task.json"]).expect("should parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(
                    args.source.task_definition.as_ref().unwrap().to_str(),
                    Some("task.json")
                );
                assert!(args.source.r#override.is_none());
                assert!(args.source.secrets.is_none());
            }
            _ => panic!("expected Validate command"),
        }
    }

    #[test]
    fn parse_validate_with_override() {
        let cli = Cli::try_parse_from([
            "lecs",
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
                    args.source.r#override.as_ref().unwrap().to_str(),
                    Some("override.json")
                );
            }
            _ => panic!("expected Validate command"),
        }
    }

    #[test]
    fn parse_validate_with_secrets() {
        let cli = Cli::try_parse_from([
            "lecs",
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
                    args.source.secrets.as_ref().unwrap().to_str(),
                    Some("secrets.json")
                );
            }
            _ => panic!("expected Validate command"),
        }
    }

    #[test]
    fn parse_inspect_command() {
        let cli = Cli::try_parse_from(["lecs", "inspect", "my-app"]).expect("should parse");
        match cli.command {
            Command::Inspect(args) => {
                assert_eq!(args.family, "my-app");
            }
            _ => panic!("expected Inspect command"),
        }
    }

    #[test]
    fn parse_stats_command_defaults() {
        let cli = Cli::try_parse_from(["lecs", "stats"]).expect("should parse");
        match cli.command {
            Command::Stats(args) => {
                assert!(args.family.is_none());
            }
            _ => panic!("expected Stats command"),
        }
    }

    #[test]
    fn parse_stats_with_family() {
        let cli = Cli::try_parse_from(["lecs", "stats", "my-app"]).expect("should parse");
        match cli.command {
            Command::Stats(args) => {
                assert_eq!(args.family.as_deref(), Some("my-app"));
            }
            _ => panic!("expected Stats command"),
        }
    }

    #[test]
    fn parse_completions_bash() {
        let cli = Cli::try_parse_from(["lecs", "completions", "bash"]).expect("should parse");
        match cli.command {
            Command::Completions(args) => {
                assert_eq!(args.shell, clap_complete::Shell::Bash);
            }
            _ => panic!("expected Completions command"),
        }
    }

    #[test]
    fn parse_completions_zsh() {
        let cli = Cli::try_parse_from(["lecs", "completions", "zsh"]).expect("should parse");
        match cli.command {
            Command::Completions(args) => {
                assert_eq!(args.shell, clap_complete::Shell::Zsh);
            }
            _ => panic!("expected Completions command"),
        }
    }

    #[test]
    fn parse_completions_fish() {
        let cli = Cli::try_parse_from(["lecs", "completions", "fish"]).expect("should parse");
        match cli.command {
            Command::Completions(args) => {
                assert_eq!(args.shell, clap_complete::Shell::Fish);
            }
            _ => panic!("expected Completions command"),
        }
    }

    #[test]
    fn parse_init_command_defaults() {
        let cli = Cli::try_parse_from(["lecs", "init"]).expect("should parse");
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
        let cli = Cli::try_parse_from(["lecs", "run", "-f", "task.json", "--profile", "dev"])
            .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert_eq!(args.source.profile.as_deref(), Some("dev"));
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_with_short_profile() {
        let cli = Cli::try_parse_from(["lecs", "run", "-f", "task.json", "-p", "staging"])
            .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert_eq!(args.source.profile.as_deref(), Some("staging"));
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_profile_with_override() {
        let cli = Cli::try_parse_from([
            "lecs",
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
                assert_eq!(args.source.profile.as_deref(), Some("dev"));
                assert_eq!(
                    args.source.r#override.as_ref().unwrap().to_str(),
                    Some("custom.json")
                );
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_without_profile() {
        let cli = Cli::try_parse_from(["lecs", "run", "-f", "task.json"]).expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert!(args.source.profile.is_none());
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_validate_with_profile() {
        let cli = Cli::try_parse_from(["lecs", "validate", "-f", "task.json", "--profile", "dev"])
            .expect("should parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(args.source.profile.as_deref(), Some("dev"));
            }
            _ => panic!("expected Validate command"),
        }
    }

    #[test]
    fn parse_init_with_flags() {
        let cli = Cli::try_parse_from([
            "lecs",
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
        let cli = Cli::try_parse_from(["lecs", "watch", "-f", "task.json"]).expect("should parse");
        match cli.command {
            Command::Watch(args) => {
                assert_eq!(
                    args.source.task_definition.as_ref().unwrap().to_str(),
                    Some("task.json")
                );
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
        let cli = Cli::try_parse_from(["lecs", "watch", "-f", "task.json", "--debounce", "1000"])
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
            "lecs",
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

    // --from-tf tests

    #[test]
    fn parse_run_with_from_tf() {
        let cli =
            Cli::try_parse_from(["lecs", "run", "--from-tf", "plan.json"]).expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert!(args.source.task_definition.is_none());
                assert_eq!(
                    args.source.from_tf.as_ref().unwrap().to_str(),
                    Some("plan.json")
                );
                assert!(args.source.tf_resource.is_none());
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_with_from_tf_and_resource() {
        let cli = Cli::try_parse_from([
            "lecs",
            "run",
            "--from-tf",
            "plan.json",
            "--tf-resource",
            "aws_ecs_task_definition.app",
        ])
        .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert_eq!(
                    args.source.from_tf.as_ref().unwrap().to_str(),
                    Some("plan.json")
                );
                assert_eq!(
                    args.source.tf_resource.as_deref(),
                    Some("aws_ecs_task_definition.app")
                );
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_from_tf_conflicts_with_task_definition() {
        let result =
            Cli::try_parse_from(["lecs", "run", "-f", "task.json", "--from-tf", "plan.json"]);
        assert!(result.is_err(), "should fail: -f and --from-tf conflict");
    }

    #[test]
    fn parse_run_requires_either_f_or_from_tf() {
        let result = Cli::try_parse_from(["lecs", "run"]);
        assert!(
            result.is_err(),
            "should fail: neither -f nor --from-tf provided"
        );
    }

    #[test]
    fn parse_run_tf_resource_requires_from_tf() {
        // --tf-resource alone (without --from-tf) should fail
        let result = Cli::try_parse_from([
            "lecs",
            "run",
            "--tf-resource",
            "aws_ecs_task_definition.app",
        ]);
        assert!(
            result.is_err(),
            "should fail: --tf-resource requires --from-tf"
        );
    }

    #[test]
    fn parse_validate_with_from_tf() {
        let cli = Cli::try_parse_from(["lecs", "validate", "--from-tf", "plan.json"])
            .expect("should parse");
        match cli.command {
            Command::Validate(args) => {
                assert!(args.source.task_definition.is_none());
                assert_eq!(
                    args.source.from_tf.as_ref().unwrap().to_str(),
                    Some("plan.json")
                );
            }
            _ => panic!("expected Validate command"),
        }
    }

    #[test]
    fn parse_watch_with_from_tf() {
        let cli =
            Cli::try_parse_from(["lecs", "watch", "--from-tf", "plan.json"]).expect("should parse");
        match cli.command {
            Command::Watch(args) => {
                assert!(args.source.task_definition.is_none());
                assert_eq!(
                    args.source.from_tf.as_ref().unwrap().to_str(),
                    Some("plan.json")
                );
            }
            _ => panic!("expected Watch command"),
        }
    }

    #[test]
    fn parse_run_with_from_cfn() {
        let cli = Cli::try_parse_from(["lecs", "run", "--from-cfn", "template.json"])
            .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert!(args.source.task_definition.is_none());
                assert!(args.source.from_tf.is_none());
                assert_eq!(
                    args.source.from_cfn.as_ref().unwrap().to_str(),
                    Some("template.json")
                );
                assert!(args.source.cfn_resource.is_none());
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_from_cfn_with_resource() {
        let cli = Cli::try_parse_from([
            "lecs",
            "run",
            "--from-cfn",
            "template.json",
            "--cfn-resource",
            "MyTaskDef",
        ])
        .expect("should parse");
        match cli.command {
            Command::Run(args) => {
                assert_eq!(
                    args.source.from_cfn.as_ref().unwrap().to_str(),
                    Some("template.json")
                );
                assert_eq!(args.source.cfn_resource.as_deref(), Some("MyTaskDef"));
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn parse_run_cfn_conflicts_with_tf() {
        let result = Cli::try_parse_from([
            "lecs",
            "run",
            "--from-cfn",
            "template.json",
            "--from-tf",
            "plan.json",
        ]);
        assert!(
            result.is_err(),
            "should fail: --from-cfn and --from-tf conflict"
        );
    }

    #[test]
    fn parse_run_cfn_conflicts_with_f() {
        let result = Cli::try_parse_from([
            "lecs",
            "run",
            "-f",
            "task.json",
            "--from-cfn",
            "template.json",
        ]);
        assert!(result.is_err(), "should fail: -f and --from-cfn conflict");
    }

    #[test]
    fn parse_validate_with_from_cfn() {
        let cli = Cli::try_parse_from(["lecs", "validate", "--from-cfn", "template.json"])
            .expect("should parse");
        match cli.command {
            Command::Validate(args) => {
                assert!(args.source.task_definition.is_none());
                assert!(args.source.from_tf.is_none());
                assert_eq!(
                    args.source.from_cfn.as_ref().unwrap().to_str(),
                    Some("template.json")
                );
            }
            _ => panic!("expected Validate command"),
        }
    }

    #[test]
    fn parse_exec_no_command() {
        let cli = Cli::try_parse_from(["lecs", "exec", "app"]).expect("should parse");
        match cli.command {
            Command::Exec(args) => {
                assert_eq!(args.container, "app");
                assert!(args.command.is_empty());
            }
            _ => panic!("expected Exec command"),
        }
    }

    #[test]
    fn parse_exec_with_double_dash() {
        let cli =
            Cli::try_parse_from(["lecs", "exec", "app", "--", "ls", "-la"]).expect("should parse");
        match cli.command {
            Command::Exec(args) => {
                assert_eq!(args.container, "app");
                assert_eq!(args.command, vec!["ls", "-la"]);
            }
            _ => panic!("expected Exec command"),
        }
    }

    #[test]
    fn parse_exec_hyphen_args_require_double_dash() {
        // -la is interpreted as a flag without --, so this should fail
        let result = Cli::try_parse_from(["lecs", "exec", "app", "-la"]);
        assert!(result.is_err(), "hyphen-prefixed args need -- separator");
    }

    #[test]
    fn parse_watch_with_from_cfn() {
        let cli = Cli::try_parse_from(["lecs", "watch", "--from-cfn", "template.json"])
            .expect("should parse");
        match cli.command {
            Command::Watch(args) => {
                assert!(args.source.task_definition.is_none());
                assert!(args.source.from_tf.is_none());
                assert_eq!(
                    args.source.from_cfn.as_ref().unwrap().to_str(),
                    Some("template.json")
                );
            }
            _ => panic!("expected Watch command"),
        }
    }
}
