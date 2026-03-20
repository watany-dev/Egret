mod cli;

use clap::Parser;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = cli::Cli::parse();

    match cli.command {
        cli::Command::Run(args) => cli::run::execute(&args),
        cli::Command::Stop(args) => cli::stop::execute(&args),
        cli::Command::Version => cli::version::execute(),
    }
}
