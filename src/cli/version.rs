/// Execute the `version` subcommand.
#[allow(clippy::print_stdout)]
pub fn execute() {
    println!("egret {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_execute_does_not_panic() {
        execute();
    }
}
