//! `lat theme`: a theme out of the database as a zip, and back in.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use laterite_core::Db;
use laterite_web::theme::archive;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ThemeCommand {
    /// Write a theme and its templates to a zip.
    Export(ExportArgs),
    /// Create or update a theme from a zip.
    Import(ImportArgs),
    /// List the themes.
    List,
    /// Render a theme's fixed pages and assets to a directory, for a static host.
    Build(BuildArgs),
}

#[derive(Args)]
pub struct BuildArgs {
    /// The theme's code.
    pub code: String,
    /// The output directory. Default: `dist`.
    #[arg(long, short, default_value = "dist")]
    pub out: PathBuf,
    /// The site's origin for absolute links. Default: the application's
    /// `app.url`, else one derived from `server.listen`.
    #[arg(long)]
    pub base_url: Option<String>,
}

#[derive(Args)]
pub struct ExportArgs {
    /// The theme's code.
    pub code: String,
    /// Where to write the zip. Default: `<code>.zip` in the current directory.
    #[arg(long, short)]
    pub out: Option<PathBuf>,
}

#[derive(Args)]
pub struct ImportArgs {
    /// The theme's code: created from the archive's theme.yaml when it does
    /// not exist.
    pub code: String,
    /// The zip to read.
    pub file: PathBuf,
    /// Replace templates and the config the theme already has. Each replaced
    /// template keeps a revision.
    #[arg(long)]
    pub replace: bool,
}

pub async fn run(command: ThemeCommand, db: Db) -> Result<()> {
    match command {
        ThemeCommand::Export(args) => {
            let bytes = archive::export(&db, &args.code)
                .await
                .with_context(|| format!("could not export the theme {:?}", args.code))?;
            let out = args
                .out
                .unwrap_or_else(|| PathBuf::from(format!("{}.zip", args.code)));
            std::fs::write(&out, &bytes)
                .with_context(|| format!("could not write {}", out.display()))?;
            println!("Wrote {} ({} bytes)", out.display(), bytes.len());
        }
        ThemeCommand::Import(args) => {
            let bytes = std::fs::read(&args.file)
                .with_context(|| format!("could not read {}", args.file.display()))?;
            let themes =
                laterite_web::theme::Themes::new(db.clone(), crate::cache::open(&db).await?);
            let report = themes
                .import(&args.code, &bytes, args.replace, None)
                .await
                .with_context(|| format!("could not import into the theme {:?}", args.code))?;
            println!(
                "{} {}: {} template(s) written, {} skipped{}",
                if report.created { "Created" } else { "Updated" },
                args.code,
                report.imported,
                report.skipped,
                if report.ignored.is_empty() {
                    String::new()
                } else {
                    format!(", ignored {}", report.ignored.join(", "))
                }
            );
            if report.skipped > 0 && !args.replace {
                println!("Pass --replace to overwrite the templates the theme already has.");
            }
        }
        ThemeCommand::Build(args) => {
            let base_url = args.base_url.unwrap_or_else(base_url_from_project);
            let report = laterite_web::theme::build::build(&db, &args.code, &args.out, &base_url)
                .await
                .with_context(|| format!("could not build the theme {:?}", args.code))?;
            println!(
                "Wrote {} page(s) and {} asset(s) to {} for {base_url}",
                report.pages.len(),
                report.assets.len(),
                args.out.display()
            );
            for url in &report.pages {
                println!("  {url}");
            }
            if !report.skipped.is_empty() {
                println!(
                    "Skipped {} page(s) with parameters or hidden: {}",
                    report.skipped.len(),
                    report.skipped.join(", ")
                );
            }
        }
        ThemeCommand::List => {
            let active = laterite_web::theme::active_theme(&db)
                .await?
                .map(|t| t.code);
            for theme in laterite_web::theme::store::list_themes(&db).await? {
                let mark = if active.as_deref() == Some(theme.code.as_str()) {
                    " (active)"
                } else {
                    ""
                };
                let parent = theme.parent.map(|p| format!(" < {p}")).unwrap_or_default();
                println!("{}{}: {}{}", theme.code, parent, theme.config.name, mark);
            }
        }
    }
    Ok(())
}

/// The site's origin from the application found upward: `app.url`, else
/// derived from `server.listen`; `http://localhost` with no application.
fn base_url_from_project() -> String {
    #[derive(serde::Deserialize, Default)]
    struct App {
        #[serde(default)]
        url: Option<String>,
    }
    #[derive(serde::Deserialize, Default)]
    struct Server {
        #[serde(default)]
        listen: Option<String>,
    }
    #[derive(serde::Deserialize, Default)]
    struct Only {
        #[serde(default)]
        app: App,
        #[serde(default)]
        server: Server,
    }
    let only: Only = crate::project::Project::locate()
        .and_then(|p| p.load::<Only>())
        .unwrap_or_default();
    laterite_core::config::base_url(
        only.app.url.as_deref(),
        only.server.listen.as_deref().unwrap_or("127.0.0.1:8080"),
    )
}
