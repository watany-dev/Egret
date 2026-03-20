pub mod run;
pub mod stop;
pub mod version;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Egret - Local ECS task runner
#[derive(Parser)]
#[command(name = "egret", about = "Run ECS task definitions locally")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run an ECS task definition locally
    Run(RunArgs),
    /// Stop a running local task
    Stop(StopArgs),
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
}

#[derive(Parser)]
pub struct StopArgs {
    /// Task name or ID to stop
    pub task: Option<String>,

    /// Stop all running tasks
    #[arg(long)]
    pub all: bool,
}
