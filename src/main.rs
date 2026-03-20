mod cli;
mod docker;
mod taskdef;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = cli::Cli::parse();

    match cli.command {
        cli::Command::Run(args) => cli::run::execute(&args).await?,
        cli::Command::Stop(args) => cli::stop::execute(&args).await?,
        cli::Command::Version => cli::version::execute(),
    }

    Ok(())
}
