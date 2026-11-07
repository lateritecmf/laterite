//! `lat seed`: runs the application's seeders. They are compiled into the
//! application's binary with its modules, so this runs `cargo run -- seed` in
//! the project, as `lat serve` runs `serve`.

use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Args;

use crate::project::Project;

#[derive(Args)]
pub struct SeedArgs {
    /// Run only this module's seeders (`acme.blog`).
    module: Option<String>,
    /// Build and run in release mode.
    #[arg(long)]
    release: bool,
    /// The binary's package, when the project is a workspace of several
    /// (`-p` to cargo).
    #[arg(long, short = 'p')]
    package: Option<String>,
}

pub fn run(args: SeedArgs) -> Result<()> {
    let project = Project::locate()?;
    if !project.root.join("Cargo.toml").is_file() {
        bail!(
            "{} has no Cargo project: a site of files has no database to seed",
            project.root.display()
        );
    }
    let mut cmd = Command::new("cargo");
    cmd.arg("run").arg("-q").current_dir(&project.root);
    if args.release {
        cmd.arg("--release");
    }
    if let Some(package) = &args.package {
        cmd.arg("-p").arg(package);
    }
    cmd.arg("--").arg("seed");
    if let Some(module) = &args.module {
        cmd.arg(module);
    }
    let status = cmd
        .status()
        .context("could not run `cargo run`; is the Rust toolchain installed and on PATH?")?;
    if !status.success() {
        bail!("seeding failed");
    }
    Ok(())
}
