use anyhow::Result;

use super::StopArgs;

pub async fn execute(args: &StopArgs) -> Result<()> {
    if args.all {
        println!("Stopping all running tasks...");
    } else if let Some(task) = &args.task {
        println!("Stopping task: {task}");
    } else {
        anyhow::bail!("Specify a task name or use --all to stop all tasks.");
    }

    // Phase 1: Docker cleanup
    println!("Not yet implemented. Coming in Iteration 4.");

    Ok(())
}
