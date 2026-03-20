use super::StopArgs;

pub fn execute(args: &StopArgs) {
    if args.all {
        println!("Stopping all running tasks...");
    } else if let Some(task) = &args.task {
        println!("Stopping task: {task}");
    } else {
        println!("Specify a task name or use --all to stop all tasks.");
    }

    // Phase 1: Docker cleanup
    println!("Not yet implemented. Coming in Phase 1.");
}
