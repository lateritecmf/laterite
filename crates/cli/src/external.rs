//! A command `lat` does not know itself runs the application's: a module's
//! command, compiled into the binary with the module (`lat pdf check` runs
//! `cargo run -- pdf check` in the project).

use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::project::Project;

/// Runs `args` (the command's name, then its words) through the application.
pub fn run(args: Vec<String>) -> Result<()> {
    let name = args.first().cloned().unwrap_or_default();
    let project = Project::locate().with_context(|| {
        format!(
            "`{name}` is not a lat command; outside an application there is no module to run it"
        )
    })?;
    if !project.root.join("Cargo.toml").is_file() {
        bail!(
            "`{name}` is not a lat command, and {} has no application binary whose modules could run it",
            project.root.display()
        );
    }
    let status = Command::new("cargo")
        .arg("run")
        .arg("-q")
        .current_dir(&project.root)
        .arg("--")
        .args(&args)
        .status()
        .context("could not run `cargo run`; is the Rust toolchain installed and on PATH?")?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
