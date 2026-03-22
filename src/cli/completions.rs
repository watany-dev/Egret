//! `lecs completions` command implementation.

use std::io::Write;

use clap::CommandFactory;
use clap_complete::Shell;

use super::{Cli, CompletionsArgs};

/// Execute the `completions` subcommand.
#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout)]
pub fn execute(args: &CompletionsArgs) {
    generate_to_writer(args.shell, &mut std::io::stdout());
}

/// Generate shell completion script to the given writer.
pub fn generate_to_writer(shell: Shell, writer: &mut impl Write) {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "lecs", writer);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn generates_bash_completions() {
        let mut buf = Vec::new();
        generate_to_writer(Shell::Bash, &mut buf);
        let output = String::from_utf8(buf).expect("valid utf-8");
        assert!(!output.is_empty(), "bash completions should not be empty");
        assert!(
            output.contains("lecs"),
            "bash completions should contain 'lecs'"
        );
    }

    #[test]
    fn generates_zsh_completions() {
        let mut buf = Vec::new();
        generate_to_writer(Shell::Zsh, &mut buf);
        let output = String::from_utf8(buf).expect("valid utf-8");
        assert!(!output.is_empty(), "zsh completions should not be empty");
        assert!(
            output.contains("lecs"),
            "zsh completions should contain 'lecs'"
        );
    }

    #[test]
    fn generates_fish_completions() {
        let mut buf = Vec::new();
        generate_to_writer(Shell::Fish, &mut buf);
        let output = String::from_utf8(buf).expect("valid utf-8");
        assert!(!output.is_empty(), "fish completions should not be empty");
        assert!(
            output.contains("lecs"),
            "fish completions should contain 'lecs'"
        );
    }
}
