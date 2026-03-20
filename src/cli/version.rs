/// Execute the `version` subcommand.
#[allow(clippy::print_stdout)]
pub fn execute() {
    println!("egret {}", env!("CARGO_PKG_VERSION"));
}
