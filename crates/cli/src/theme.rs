//! `lat theme`: a theme out of the database as a zip, and back in; a theme
//! built to files or previewed, from the database or from a folder alone.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use laterite_admin::statics::{self, Copy};
use laterite_core::Db;
use laterite_web::theme::{archive, folder};
use std::path::{Path, PathBuf};

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
    /// Preview a theme folder in the browser, rebuilt on every change.
    Serve(ServeArgs),
}

#[derive(Args)]
pub struct BuildArgs {
    /// The theme's code. Not needed for a theme folder, which names it.
    pub code: Option<String>,
    /// Build from a theme folder (its export layout) with no application and
    /// no database. Default: the project's `theme.folder`.
    #[arg(long, value_name = "DIR")]
    pub from: Option<PathBuf>,
    /// The output directory. Default: `dist`.
    #[arg(long, short, default_value = "dist")]
    pub out: PathBuf,
    /// The site's origin for absolute links. Default: the application's
    /// `app.url`, else one derived from `server.listen`.
    #[arg(long)]
    pub base_url: Option<String>,
    /// Copy a directory into the output: `<src>:<dest>` (`docs/book:docs`
    /// puts `docs/book/` at `<out>/docs/`). Repeatable, added to the
    /// project's `theme.copy`.
    #[arg(long, value_name = "SRC:DEST")]
    pub copy: Vec<String>,
}

#[derive(Args)]
pub struct ServeArgs {
    /// The theme folder to preview; its name is the theme's code. Default:
    /// the project's `theme.folder`.
    #[arg(long, value_name = "DIR")]
    pub from: Option<PathBuf>,
    /// The address to listen on. Default: the project's `server.listen`,
    /// else `127.0.0.1:8080`.
    #[arg(long)]
    pub listen: Option<String>,
    /// Copy a directory into the preview: `<src>:<dest>`, as `build --copy`.
    #[arg(long, value_name = "SRC:DEST")]
    pub copy: Vec<String>,
}

/// What a static website's project states for `lat theme`: `[theme] folder`
/// and `copy` in `config/default.toml`, paths relative to the project root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteConfig {
    /// The project root, when a project was found.
    pub root: Option<PathBuf>,
    /// Whether the project has a Cargo project of its own (a static website
    /// with plugins, or an application).
    pub has_crate: bool,
    /// The theme folder, absolute.
    pub folder: Option<PathBuf>,
    /// `<src>:<dest>` copies, their sources made absolute.
    pub copy: Vec<String>,
    /// The project's `server.listen`, for the preview.
    pub listen: Option<String>,
}

/// The site config of the project found upward, or an empty one.
pub fn site_config() -> SiteConfig {
    crate::project::Project::locate()
        .ok()
        .and_then(|p| site_config_of(&p).ok())
        .unwrap_or_default()
}

/// The site config of `project`.
pub fn site_config_of(project: &crate::project::Project) -> Result<SiteConfig> {
    #[derive(serde::Deserialize, Default)]
    struct Theme {
        #[serde(default)]
        folder: Option<String>,
        #[serde(default)]
        copy: Vec<String>,
    }
    #[derive(serde::Deserialize, Default)]
    struct Server {
        #[serde(default)]
        listen: Option<String>,
    }
    #[derive(serde::Deserialize, Default)]
    struct Only {
        #[serde(default)]
        theme: Theme,
        #[serde(default)]
        server: Server,
    }
    let only: Only = project.load()?;
    let root = &project.root;
    Ok(SiteConfig {
        root: Some(root.clone()),
        has_crate: root.join("Cargo.toml").is_file(),
        folder: only.theme.folder.map(|f| root.join(f)),
        copy: only
            .theme
            .copy
            .into_iter()
            .map(|value| match value.rsplit_once(':') {
                Some((src, dest)) => format!("{}:{dest}", root.join(src).display()),
                None => value,
            })
            .collect(),
        listen: only.server.listen,
    })
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

pub async fn run(command: ThemeCommand, database_url: Option<String>) -> Result<()> {
    // A build or preview from a folder needs no application; everything else
    // reads the application's database. The folder is `--from`, else what the
    // project's config states.
    let site = site_config();
    let own_binary = site.folder.is_some() && site.has_crate;
    match command {
        // A static project with plugins builds through its binary, which
        // links them; the copies named here are applied afterwards.
        ThemeCommand::Build(args) if args.from.is_none() && own_binary => {
            let root = site.root.clone().unwrap_or_default();
            let mut run = vec!["build".to_string(), "--out".to_string()];
            run.push(args.out.to_string_lossy().to_string());
            if let Some(base_url) = &args.base_url {
                run.push("--base-url".to_string());
                run.push(base_url.clone());
            }
            project_binary(&root, &run, None)?;
            apply_copies(&flag_copies(&args.copy)?, &args.out)
        }
        ThemeCommand::Build(args) if args.from.is_some() || site.folder.is_some() => {
            let dir = args
                .from
                .clone()
                .or(site.folder.clone())
                .unwrap_or_default();
            let loaded = folder::load(&dir).await?;
            let base_url = args.base_url.clone().unwrap_or_else(base_url_from_project);
            let copies = [site_copies(&site)?, flag_copies(&args.copy)?].concat();
            build(&loaded.db, &loaded.code, &args, &base_url, &copies).await
        }
        ThemeCommand::Serve(args) if args.from.is_none() && own_binary => {
            let root = site.root.clone().unwrap_or_default();
            let listen = args.listen.or(site.listen.clone());
            project_binary(&root, &["serve".to_string()], listen.as_deref())
        }
        ThemeCommand::Serve(mut args) => {
            args.from = args.from.or(site.folder.clone());
            args.listen = args.listen.or(site.listen.clone());
            let copies = [site_copies(&site)?, flag_copies(&args.copy)?].concat();
            serve(args, copies).await
        }
        command => {
            let db = crate::connect(database_url).await?;
            run_with_db(command, db).await
        }
    }
}

async fn run_with_db(command: ThemeCommand, db: Db) -> Result<()> {
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
            let code = args.code.clone().context(
                "name the theme to build, pass --from <folder>, or set [theme] folder in config/default.toml",
            )?;
            let base_url = args.base_url.clone().unwrap_or_else(base_url_from_project);
            let copies = flag_copies(&args.copy)?;
            build(&db, &code, &args, &base_url, &copies).await?;
        }
        ThemeCommand::Serve(_) => unreachable!("served without a database"),
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

/// The config's copies, each optional: a declared source absent on this
/// machine is skipped with a note.
pub(crate) fn site_copies(site: &SiteConfig) -> Result<Vec<Copy>> {
    Copy::parse(&site.copy, Path::new("."), false)
}

/// Copies named on the command line, each required.
fn flag_copies(values: &[String]) -> Result<Vec<Copy>> {
    Copy::parse(values, Path::new("."), true)
}

/// Builds `code` from `db` into the output, then the copies.
async fn build(
    db: &Db,
    code: &str,
    args: &BuildArgs,
    base_url: &str,
    copies: &[Copy],
) -> Result<()> {
    let report = laterite_web::theme::build::build(db, code, &args.out, base_url)
        .await
        .with_context(|| format!("could not build the theme {code:?}"))?;
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
    apply_copies(copies, &args.out)
}

fn apply_copies(copies: &[Copy], out: &Path) -> Result<()> {
    for copy in copies {
        if copy.apply(out)? {
            println!(
                "Copied {} to {}/{}",
                copy.src.display(),
                out.display(),
                copy.dest
            );
        }
    }
    Ok(())
}

/// `lat theme serve`: previews a theme folder, rebuilt on every change, from
/// `lat` alone. A static project with a Cargo project of its own previews
/// through its binary instead (see [`project_binary`]).
pub(crate) async fn serve(args: ServeArgs, copies: Vec<Copy>) -> Result<()> {
    let from = args.from.context(
        "name the theme folder to preview: --from <folder>, or [theme] folder in config/default.toml",
    )?;
    let listen = args.listen.unwrap_or_else(|| "127.0.0.1:8080".to_string());
    let base_url = format!("http://{listen}");
    let dir = from.clone();
    let rebuild: statics::Rebuild = std::sync::Arc::new(move |out: PathBuf| {
        let dir = dir.clone();
        let base_url = base_url.clone();
        let copies = copies.clone();
        Box::pin(async move {
            let loaded = folder::load(&dir).await?;
            laterite_web::theme::build::build(&loaded.db, &loaded.code, &out, &base_url)
                .await
                .with_context(|| format!("could not build the theme {:?}", loaded.code))?;
            for copy in &copies {
                copy.apply(&out)?;
            }
            Ok(())
        })
    });
    statics::preview(from, &listen, rebuild).await
}

/// Runs a static project's own binary: `cargo run -- <args>` in the project
/// root, so the plugins it links take part in the build or the preview.
pub(crate) fn project_binary(root: &Path, args: &[String], listen: Option<&str>) -> Result<()> {
    let project = crate::project::Project::locate_from(root)?;
    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("run")
        .arg("--quiet")
        .current_dir(root)
        .arg("--")
        .args(args);
    if let Some(listen) = listen {
        cmd.env(project.env_key("SERVER__LISTEN"), listen);
    }
    let status = cmd
        .status()
        .context("could not run `cargo run`; is the Rust toolchain installed and on PATH?")?;
    if !status.success() {
        anyhow::bail!("the site's binary exited with {status}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_project_states_its_theme_folder_and_copies() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("acme");
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/default.toml"),
            "[app]\nname = \"Acme\"\n[server]\nlisten = \"127.0.0.1:3000\"\n[theme]\nfolder = \"themes/acme\"\ncopy = [\"static:static\", \"docs/book:docs\"]\n",
        )
        .unwrap();
        let project = crate::project::Project::locate_from(&root.join("config")).unwrap();
        let site = site_config_of(&project).unwrap();
        assert_eq!(site.folder, Some(root.join("themes/acme")));
        assert_eq!(
            site.copy,
            vec![
                format!("{}:static", root.join("static").display()),
                format!("{}:docs", root.join("docs/book").display()),
            ]
        );
        assert_eq!(site.listen.as_deref(), Some("127.0.0.1:3000"));

        std::fs::write(root.join("config/default.toml"), "[app]\nname = \"Acme\"\n").unwrap();
        let project = crate::project::Project::locate_from(&root).unwrap();
        let bare = site_config_of(&project).unwrap();
        assert_eq!(bare.folder, None);
        assert!(bare.copy.is_empty() && bare.listen.is_none() && !bare.has_crate);
    }
}
