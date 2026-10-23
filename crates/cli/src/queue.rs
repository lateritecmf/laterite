//! `lat queue`: the jobs waiting, the failed list, and a worker.

use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use laterite_core::queue::{DatabaseQueue, QueueDriver};

use crate::project::Project;

#[derive(Subcommand)]
pub enum QueueCommand {
    /// Run the application's worker (`<app> queue work`) from here.
    Work(WorkArgs),
    /// How many jobs wait on each queue.
    List,
    /// The failed list.
    Failed,
    /// Put a failed job back on its queue: an id, or `all`.
    Retry { id: String },
    /// Drop a failed job: an id, or `all`.
    Forget { id: String },
    /// Drop every waiting job, or those on one queue.
    Flush {
        #[arg(long)]
        queue: Option<String>,
    },
}

#[derive(Args)]
pub struct WorkArgs {
    /// Process one job (or find none) and exit.
    #[arg(long)]
    once: bool,
    /// Exit once no job is waiting.
    #[arg(long)]
    stop_when_empty: bool,
    /// The queues to take from, in priority order (`high,default`).
    #[arg(long)]
    queue: Option<String>,
    /// Build and run in release mode.
    #[arg(long)]
    release: bool,
}

pub async fn run(command: QueueCommand, database_url: Option<String>) -> Result<()> {
    if let QueueCommand::Work(args) = command {
        return work(args);
    }
    let db = crate::connect(database_url).await?;
    let driver = DatabaseQueue::new(db);
    match command {
        QueueCommand::Work(_) => unreachable!("handled above"),
        QueueCommand::List => {
            let counts = driver.counts().await.context("reading the queues")?;
            if counts.is_empty() {
                println!("No job is waiting.");
            }
            for c in counts {
                println!("{}\t{} waiting\t{} running", c.queue, c.pending, c.reserved);
            }
        }
        QueueCommand::Failed => {
            let failed = driver.failed().await.context("reading the failed list")?;
            if failed.is_empty() {
                println!("The failed list is empty.");
            }
            for f in failed {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    f.id, f.failed_at, f.queue, f.name, f.error
                );
            }
        }
        QueueCommand::Retry { id } => {
            let moved = driver.retry(parse_id(&id)?).await.context("retrying")?;
            println!("Put {moved} job(s) back on the queue.");
        }
        QueueCommand::Forget { id } => {
            let gone = driver.forget(parse_id(&id)?).await.context("forgetting")?;
            println!("Dropped {gone} failed job(s).");
        }
        QueueCommand::Flush { queue } => {
            let gone = driver.clear(queue.as_deref()).await.context("flushing")?;
            println!("Dropped {gone} waiting job(s).");
        }
    }
    Ok(())
}

fn parse_id(id: &str) -> Result<Option<i64>> {
    if id == "all" {
        return Ok(None);
    }
    id.parse::<i64>()
        .map(Some)
        .with_context(|| format!("`{id}` is not a job id or `all`"))
}

/// The worker lives in the application binary, since the jobs do; this runs
/// `cargo run -- queue work` in the project, as `lat serve` runs the server.
fn work(args: WorkArgs) -> Result<()> {
    let project = Project::locate()?;
    if !project.root.join("Cargo.toml").is_file() {
        bail!(
            "run `lat queue work` inside a Laterite application (one created by `lat new`): \
             no Cargo.toml in {}.",
            project.root.display()
        );
    }
    let mut cmd = Command::new("cargo");
    cmd.arg("run").current_dir(&project.root);
    if args.release {
        cmd.arg("--release");
    }
    cmd.arg("--").arg("queue").arg("work");
    if args.once {
        cmd.arg("--once");
    }
    if args.stop_when_empty {
        cmd.arg("--stop-when-empty");
    }
    if let Some(queue) = &args.queue {
        cmd.arg("--queue").arg(queue);
    }
    let status = cmd.status().context("could not start cargo run")?;
    if !status.success() {
        bail!("the worker exited with {status}");
    }
    Ok(())
}
