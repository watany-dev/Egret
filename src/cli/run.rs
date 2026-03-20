use super::RunArgs;

pub fn execute(args: &RunArgs) {
    tracing::info!(
        task_definition = %args.task_definition.display(),
        "Starting ECS task locally"
    );

    // Phase 1: Parse task definition, create containers, run
    println!(
        "egret run: task definition = {}",
        args.task_definition.display()
    );
    println!("Not yet implemented. Coming in Phase 1.");
}
