//! `lat schedule`: the entries' last runs, and the ticker.

use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};

use crate::project::Project;

#[derive(Subcommand)]
pub enum ScheduleCommand {
    /// Run the application's ticker (`<app> schedule run`) from here.
    Run(RunArgs),
    /// Every entry that has run on this deployment, with its last run.
    List,
}

#[derive(Args)]
pub struct RunArgs {
    /// Evaluate this minute's entries once and exit.
    #[arg(long)]
    once: bool,
    /// Build and run in release mode.
    #[arg(long)]
    release: bool,
}

pub async fn run(command: ScheduleCommand, database_url: Option<String>) -> Result<()> {
    match command {
        ScheduleCommand::Run(args) => tick(args),
        ScheduleCommand::List => {
            let db = crate::connect(database_url).await?;
            let rows = laterite_core::schedule::recorded(&db)
                .await
                .context("reading the schedule runs")?;
            if rows.is_empty() {
                println!("No entry has run yet on this deployment.");
            }
            for row in rows {
                println!(
                    "{}\t{}\t{}\t{}",
                    row.name,
                    row.last_status.as_str(),
                    row.last_started_at.as_deref().unwrap_or("-"),
                    row.last_error.as_deref().unwrap_or("")
                );
            }
            Ok(())
        }
    }
}

/// The ticker lives in the application binary, since the entries do; this
/// runs `cargo run -- schedule run` in the project, as `lat serve` runs the
/// server.
fn tick(args: RunArgs) -> Result<()> {
    let project = Project::locate()?;
    if !project.root.join("Cargo.toml").is_file() {
        bail!(
            "run `lat schedule run` inside a Laterite application (one created by `lat new`): \
             no Cargo.toml in {}.",
            project.root.display()
        );
    }
    let mut cmd = Command::new("cargo");
    cmd.arg("run").current_dir(&project.root);
    if args.release {
        cmd.arg("--release");
    }
    cmd.arg("--").arg("schedule").arg("run");
    if args.once {
        cmd.arg("--once");
    }
    let status = cmd.status().context("could not start cargo run")?;
    if !status.success() {
        bail!("the ticker exited with {status}");
    }
    Ok(())
}
