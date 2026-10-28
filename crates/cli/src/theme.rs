//! `lat theme`: a theme out of the database as a zip, and back in; a theme
//! built to files or previewed, from the database or from a folder alone.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
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
    match command {
        ThemeCommand::Build(args) if args.from.is_some() || site.folder.is_some() => {
            let dir = args
                .from
                .clone()
                .or(site.folder.clone())
                .unwrap_or_default();
            let loaded = folder::load(&dir).await?;
            let base_url = args.base_url.clone().unwrap_or_else(base_url_from_project);
            let copies = [site_copies(&site)?, copies(&args.copy, true)?].concat();
            build(&loaded.db, &loaded.code, &args, &base_url, &copies).await
        }
        ThemeCommand::Serve(mut args) => {
            args.from = args.from.or(site.folder.clone());
            args.listen = args.listen.or(site.listen.clone());
            let copies = [site_copies(&site)?, copies(&args.copy, true)?].concat();
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
            let copies = copies(&args.copy, true)?;
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

/// A directory to copy into the output: the source, the destination under the
/// output root, and whether a missing source fails the build (a `--copy` on
/// the command line) or is noted and skipped (one the config declares, such
/// as a handbook only the deploy build puts in place).
#[derive(Debug, Clone)]
pub(crate) struct Copy {
    src: PathBuf,
    dest: String,
    required: bool,
}

/// The config's copies, each optional.
pub(crate) fn site_copies(site: &SiteConfig) -> Result<Vec<Copy>> {
    copies(&site.copy, false)
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
    for copy in copies {
        if copy_into(&args.out, base_url, copy)? {
            println!(
                "Copied {} to {}/{}",
                copy.src.display(),
                args.out.display(),
                copy.dest
            );
        }
    }
    Ok(())
}

/// Parses copy values: `<src>:<dest>`, the destination relative to the
/// output root and never climbing out of it.
fn copies(values: &[String], required: bool) -> Result<Vec<Copy>> {
    values
        .iter()
        .map(|value| {
            let (src, dest) = value
                .rsplit_once(':')
                .filter(|(src, dest)| !src.is_empty() && !dest.is_empty())
                .with_context(|| format!("--copy takes <src>:<dest>, got {value:?}"))?;
            let dest = dest.trim_matches('/');
            if dest.split('/').any(|part| part == "..") {
                anyhow::bail!("--copy destination {dest:?} climbs out of the output");
            }
            Ok(Copy {
                src: PathBuf::from(src),
                dest: dest.to_string(),
                required,
            })
        })
        .collect()
}

/// Copies the directory into `<out>/<dest>`. `Ok(false)` when an optional
/// source is absent, after saying so.
fn copy_into(out: &Path, base_url: &str, copy: &Copy) -> Result<bool> {
    let (src, dest) = (&copy.src, copy.dest.as_str());
    if !src.is_dir() {
        if copy.required {
            anyhow::bail!("copy source {} is not a directory", src.display());
        }
        println!(
            "Skipped the copy of {} to {dest}: not a directory here",
            src.display()
        );
        return Ok(false);
    }
    // A source holding the output would copy the output into itself without end.
    let src_abs = src.canonicalize()?;
    if out
        .canonicalize()
        .is_ok_and(|out_abs| out_abs.starts_with(&src_abs))
    {
        anyhow::bail!(
            "--copy source {} contains the output directory",
            src.display()
        );
    }
    laterite_web::StaticSite::new(out, base_url)?
        .assets(src, dest)
        .with_context(|| format!("could not copy {} into {dest}", src.display()))?;
    Ok(true)
}

/// `lat theme serve --from <dir>`: builds the folder into a scratch directory
/// and serves it, rebuilding when a file in the folder changes. What the
/// browser shows is what `build` writes, byte for byte.
pub(crate) async fn serve(args: ServeArgs, copies: Vec<Copy>) -> Result<()> {
    use std::sync::{Arc, Mutex};

    let from = args.from.context(
        "name the theme folder to preview: --from <folder>, or [theme] folder in config/default.toml",
    )?;
    let dir = from
        .canonicalize()
        .with_context(|| format!("no theme folder at {}", from.display()))?;
    let listen = args.listen.unwrap_or_else(|| "127.0.0.1:8080".to_string());
    let base_url = format!("http://{listen}");
    let out = std::env::temp_dir().join(format!("lat-theme-serve-{}", std::process::id()));
    let preview = Arc::new(Preview {
        dir,
        out,
        base_url: base_url.clone(),
        copies,
        built: Mutex::new(None),
    });
    Arc::clone(&preview).rebuild().await?;
    println!(
        "Previewing {} at {base_url} (rebuilt on change; Ctrl-C to stop)",
        preview.dir.display()
    );

    let app = axum::Router::new()
        .fallback(axum::routing::get(serve_file))
        .with_state(Arc::clone(&preview));
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("could not listen on {listen}"))?;
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    let _ = std::fs::remove_dir_all(&preview.out);
    result.context("the preview server failed")
}

/// The preview's folder, its scratch output and when it was last built.
struct Preview {
    dir: PathBuf,
    out: PathBuf,
    base_url: String,
    copies: Vec<Copy>,
    /// The newest change the last build saw.
    built: std::sync::Mutex<Option<std::time::SystemTime>>,
}

impl Preview {
    /// The newest modification time under the folder.
    fn newest_change(&self) -> std::time::SystemTime {
        let mut newest = std::time::SystemTime::UNIX_EPOCH;
        let mut pending = vec![self.dir.clone()];
        while let Some(folder) = pending.pop() {
            let Ok(read) = std::fs::read_dir(&folder) else {
                continue;
            };
            for entry in read.flatten() {
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    pending.push(path);
                } else if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                    newest = newest.max(modified);
                }
            }
        }
        newest
    }

    /// Rebuilds when the folder changed since the last build. The load and
    /// the build run on a blocking thread with their own `block_on`, since the
    /// migration runner's future is not `Send` and a request handler's must be.
    async fn rebuild(self: std::sync::Arc<Self>) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            let newest = self.newest_change();
            let mut built = self.built.lock().unwrap_or_else(|e| e.into_inner());
            if *built == Some(newest) {
                return Ok(());
            }
            tokio::runtime::Handle::current().block_on(async {
                let loaded = folder::load(&self.dir).await?;
                let _ = std::fs::remove_dir_all(&self.out);
                laterite_web::theme::build::build(
                    &loaded.db,
                    &loaded.code,
                    &self.out,
                    &self.base_url,
                )
                .await
                .with_context(|| format!("could not build the theme {:?}", loaded.code))?;
                for copy in &self.copies {
                    copy_into(&self.out, &self.base_url, copy)?;
                }
                anyhow::Ok(())
            })?;
            *built = Some(newest);
            Ok(())
        })
        .await
        .context("the preview build panicked")?
    }
}

/// Serves one file of the built preview: a clean URL maps to its
/// `index.html`, anything else to the file at that path; an unknown URL gets
/// the theme's `404.html` when it has one.
async fn serve_file(
    axum::extract::State(preview): axum::extract::State<std::sync::Arc<Preview>>,
    uri: axum::http::Uri,
) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    if let Err(e) = std::sync::Arc::clone(&preview).rebuild().await {
        let text = format!("Build failed:\n\n{e:#}\n");
        return (StatusCode::INTERNAL_SERVER_ERROR, text).into_response();
    }
    let path = uri.path().trim_matches('/');
    if path.split('/').any(|part| part == "..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let candidates = if path.is_empty() {
        vec![preview.out.join("index.html")]
    } else {
        vec![
            preview.out.join(path),
            preview.out.join(path).join("index.html"),
        ]
    };
    for file in candidates {
        if file.is_file() {
            if let Ok(bytes) = std::fs::read(&file) {
                let mime = mime_of(&file);
                return ([(header::CONTENT_TYPE, mime)], bytes).into_response();
            }
        }
    }
    let missing = preview.out.join("404.html");
    match std::fs::read(&missing) {
        Ok(bytes) => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

/// A content type from the file's extension, for the few kinds a site serves.
fn mime_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("txt") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
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
        assert_eq!(site_config_of(&project).unwrap(), SiteConfig::default());
    }
}
