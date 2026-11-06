//! `lat routes`: every URL the application answers, with its owner, as a
//! table or JSON, filtered by path, method, kind, owner or permission. The
//! map lives in the application's binary (plugins are compiled in), so this
//! runs `cargo run -- routes --json` in the project and filters what comes
//! back, as `lat serve` runs `serve`.

use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Args;
use laterite_admin::routemap::{render_table, RouteRow};

use crate::project::Project;

#[derive(Args)]
pub struct RoutesArgs {
    /// Keep rows whose path contains this text (`events`, `/admin/`).
    #[arg(long)]
    path: Option<String>,
    /// Keep rows answering this method (`GET`, `POST`, `QUERY`).
    #[arg(long)]
    method: Option<String>,
    /// Keep rows of this kind: resource, screen, settings, public, page, framework.
    #[arg(long)]
    kind: Option<String>,
    /// Keep rows this module owns (`acme.blog`, `the framework`).
    #[arg(long)]
    owner: Option<String>,
    /// Keep rows gated by this permission.
    #[arg(long)]
    permission: Option<String>,
    /// Keep rows no permission gates.
    #[arg(long)]
    public: bool,
    /// Print the rows as JSON.
    #[arg(long)]
    json: bool,
    /// Build and run in release mode.
    #[arg(long)]
    release: bool,
    /// The binary's package, when the project is a workspace of several
    /// (`-p` to cargo).
    #[arg(long, short = 'p')]
    package: Option<String>,
}

pub fn run(args: RoutesArgs) -> Result<()> {
    let project = Project::locate()?;
    if !project.root.join("Cargo.toml").is_file() {
        bail!(
            "{} has no Cargo project: a site of files has no routes beyond its theme's pages",
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
    cmd.arg("--").arg("routes").arg("--json");
    let out = cmd
        .output()
        .context("could not run `cargo run`; is the Rust toolchain installed and on PATH?")?;
    if !out.status.success() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        bail!("the application did not print its routes");
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // The map is the last line of stdout; the boot may log above it.
    let line = text
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('['))
        .unwrap_or_default();
    let rows: Vec<RouteRow> = serde_json::from_str(line.trim())
        .context("the application printed something other than its route map")?;
    let rows = filter(rows, &args);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        print!("{}", render_table(&rows));
    }
    Ok(())
}

/// The rows every given filter keeps.
fn filter(rows: Vec<RouteRow>, args: &RoutesArgs) -> Vec<RouteRow> {
    let method = args.method.as_deref().map(str::to_ascii_uppercase);
    rows.into_iter()
        .filter(|r| args.path.as_deref().is_none_or(|p| r.path.contains(p)))
        .filter(|r| {
            method
                .as_deref()
                .is_none_or(|m| r.methods.iter().any(|x| x == m))
        })
        .filter(|r| {
            args.kind
                .as_deref()
                .is_none_or(|k| r.kind.eq_ignore_ascii_case(k))
        })
        .filter(|r| args.owner.as_deref().is_none_or(|o| r.owner == o))
        .filter(|r| {
            args.permission
                .as_deref()
                .is_none_or(|p| r.permission.as_deref() == Some(p))
        })
        .filter(|r| !args.public || r.permission.is_none())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<RouteRow> {
        vec![
            RouteRow::new(
                &["GET"],
                "/admin/events",
                "resource",
                "acme.site",
                Some("acme.site.manage"),
            ),
            RouteRow::new(
                &["GET", "POST"],
                "/admin/events/new",
                "resource",
                "acme.site",
                Some("acme.site.manage"),
            ),
            RouteRow::new(&["GET"], "/events/:slug", "page", "theme acme", None),
            RouteRow::new(
                &["GET", "POST"],
                "/admin/login",
                "framework",
                "the framework",
                None,
            ),
        ]
    }

    fn args() -> RoutesArgs {
        RoutesArgs {
            path: None,
            method: None,
            kind: None,
            owner: None,
            permission: None,
            public: false,
            json: false,
            release: false,
            package: None,
        }
    }

    #[test]
    fn filters_combine() {
        let mut a = args();
        a.path = Some("events".into());
        assert_eq!(filter(rows(), &a).len(), 3);
        a.method = Some("post".into());
        assert_eq!(filter(rows(), &a).len(), 1);
        let mut b = args();
        b.public = true;
        assert_eq!(filter(rows(), &b).len(), 2);
        b.kind = Some("page".into());
        assert_eq!(filter(rows(), &b)[0].path, "/events/:slug");
        let mut c = args();
        c.owner = Some("the framework".into());
        assert_eq!(filter(rows(), &c).len(), 1);
        let mut d = args();
        d.method = Some("QUERY".into());
        assert!(
            filter(rows(), &d).is_empty(),
            "QUERY is accepted and matches nothing yet"
        );
    }
}
