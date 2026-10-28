//! A static website built by its own binary: the theme folder named by
//! `[theme] folder`, rendered through the renderer the theme module
//! contributes, with every module's seed, site files and the config's copies.
//! The preview serves the build from a scratch directory, rebuilt on change.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use laterite_core::strata::async_trait;
use laterite_core::{Db, Registry, SiteContext};

/// What the renderer is given: the build's database, the theme folder, the
/// output, the origin, and every contribution (markup and components among
/// them).
pub struct StaticBuildCx<'a> {
    pub db: &'a Db,
    pub folder: &'a Path,
    pub out: &'a Path,
    pub base_url: &'a str,
    pub contributions: &'a Registry,
}

/// Renders a theme folder into a directory. The theme module contributes
/// one; [`crate::bootstrap::Bootstrap`] calls it in static mode.
#[async_trait]
pub trait StaticRenderer: Send + Sync {
    /// Loads the theme folder into the database afresh and returns the
    /// theme's code.
    async fn load(&self, cx: &StaticBuildCx<'_>) -> Result<String>;
    /// Renders the theme's pages and assets into `cx.out`, returning the
    /// page URLs written.
    async fn build(&self, cx: &StaticBuildCx<'_>, code: &str) -> Result<Vec<String>>;
}

/// The renderer a module contributes.
pub struct StaticRendererReg(pub Arc<dyn StaticRenderer>);

/// Writes a site file at `path` under `out`, confined to it.
pub fn write_site_file(out: &Path, path: &str, body: &str) -> Result<()> {
    let relative = path.trim_start_matches('/');
    if relative.is_empty() || relative.split('/').any(|part| part == "..") {
        anyhow::bail!("`{path}` is not a path inside the output");
    }
    let target = out.join(relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, body).with_context(|| format!("writing {}", target.display()))
}

/// A directory to copy into the output: the source, the destination under
/// the output root, and whether a missing source fails the build (named on
/// the command line) or is noted and skipped (declared in the config, such as
/// a handbook only the deploy build puts in place).
#[derive(Debug, Clone)]
pub struct Copy {
    pub src: PathBuf,
    pub dest: String,
    pub required: bool,
}

impl Copy {
    /// Parses `<src>:<dest>` values, sources relative to `root`.
    pub fn parse(values: &[String], root: &Path, required: bool) -> Result<Vec<Copy>> {
        values
            .iter()
            .map(|value| {
                let (src, dest) = value
                    .rsplit_once(':')
                    .filter(|(src, dest)| !src.is_empty() && !dest.is_empty())
                    .with_context(|| format!("a copy takes <src>:<dest>, got {value:?}"))?;
                let dest = dest.trim_matches('/');
                if dest.split('/').any(|part| part == "..") {
                    anyhow::bail!("copy destination {dest:?} climbs out of the output");
                }
                Ok(Copy {
                    src: root.join(src),
                    dest: dest.to_string(),
                    required,
                })
            })
            .collect()
    }

    /// Copies the directory into `<out>/<dest>`. `Ok(false)` when an optional
    /// source is absent, after saying so.
    pub fn apply(&self, out: &Path) -> Result<bool> {
        if !self.src.is_dir() {
            if self.required {
                anyhow::bail!("copy source {} is not a directory", self.src.display());
            }
            println!(
                "Skipped the copy of {} to {}: not a directory here",
                self.src.display(),
                self.dest
            );
            return Ok(false);
        }
        // A source holding the output would copy the output into itself without end.
        let src = self.src.canonicalize()?;
        if out.canonicalize().is_ok_and(|o| o.starts_with(&src)) {
            anyhow::bail!(
                "copy source {} contains the output directory",
                self.src.display()
            );
        }
        copy_dir(&src, &out.join(&self.dest))
            .with_context(|| format!("could not copy {} into {}", self.src.display(), self.dest))?;
        Ok(true)
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// The newest modification time under `dir`, hidden entries skipped.
pub fn newest_change(dir: &Path) -> std::time::SystemTime {
    let mut newest = std::time::SystemTime::UNIX_EPOCH;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let Ok(read) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in read.flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                newest = newest.max(modified);
            }
        }
    }
    newest
}

/// A build the preview runs: given the output directory, writes the site.
/// Its future need not be `Send`; the preview runs it on a blocking thread.
pub type Rebuild = Arc<
    dyn Fn(PathBuf) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>>>>
        + Send
        + Sync,
>;

/// Serves a static build from a scratch directory, rebuilding it through
/// `rebuild` whenever a file under `watch` changes. A clean URL maps to its
/// `index.html`, anything else to the file at that path; an unknown URL gets
/// the build's `404.html` when it has one. Stops on Ctrl-C.
pub async fn preview(watch: PathBuf, listen: &str, rebuild: Rebuild) -> Result<()> {
    let watch = watch
        .canonicalize()
        .with_context(|| format!("no theme folder at {}", watch.display()))?;
    let out = std::env::temp_dir().join(format!("lat-preview-{}", std::process::id()));
    let preview = Arc::new(Preview {
        watch,
        out,
        rebuild,
        built: std::sync::Mutex::new(None),
    });
    Arc::clone(&preview).refresh().await?;
    println!(
        "Previewing {} at http://{listen} (rebuilt on change; Ctrl-C to stop)",
        preview.watch.display()
    );
    let app = axum::Router::new()
        .fallback(axum::routing::get(serve_file))
        .with_state(Arc::clone(&preview));
    let listener = tokio::net::TcpListener::bind(listen)
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

struct Preview {
    watch: PathBuf,
    out: PathBuf,
    rebuild: Rebuild,
    /// The newest change the last build saw.
    built: std::sync::Mutex<Option<std::time::SystemTime>>,
}

impl Preview {
    /// Rebuilds when the folder changed since the last build. The build runs
    /// on a blocking thread with its own `block_on`, since a migration
    /// runner's future is not `Send` and a request handler's must be.
    async fn refresh(self: Arc<Self>) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            let newest = newest_change(&self.watch);
            let mut built = self.built.lock().unwrap_or_else(|e| e.into_inner());
            if *built == Some(newest) {
                return Ok(());
            }
            let _ = std::fs::remove_dir_all(&self.out);
            std::fs::create_dir_all(&self.out)?;
            tokio::runtime::Handle::current().block_on((self.rebuild)(self.out.clone()))?;
            *built = Some(newest);
            Ok(())
        })
        .await
        .context("the preview build panicked")?
    }
}

async fn serve_file(
    axum::extract::State(preview): axum::extract::State<Arc<Preview>>,
    uri: axum::http::Uri,
) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    if let Err(e) = Arc::clone(&preview).refresh().await {
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
                return ([(header::CONTENT_TYPE, mime_of(&file))], bytes).into_response();
            }
        }
    }
    match std::fs::read(preview.out.join("404.html")) {
        Ok(bytes) => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

/// A content type from the file's extension, for the kinds a site serves.
pub fn mime_of(path: &Path) -> &'static str {
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

/// The site context a static build renders site files with: no admin path.
pub fn site_context(base_url: &str) -> SiteContext {
    SiteContext::new(base_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_file_stays_inside_the_output() {
        let tmp = tempfile::tempdir().unwrap();
        write_site_file(tmp.path(), "/robots.txt", "User-agent: *\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("robots.txt")).unwrap(),
            "User-agent: *\n"
        );
        write_site_file(tmp.path(), ".well-known/security.txt", "x").unwrap();
        assert!(tmp.path().join(".well-known/security.txt").is_file());
        assert!(write_site_file(tmp.path(), "../escape.txt", "x").is_err());
        assert!(write_site_file(tmp.path(), "/", "x").is_err());
    }

    #[test]
    fn copies_parse_and_apply() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("site");
        std::fs::create_dir_all(root.join("static/img")).unwrap();
        std::fs::write(root.join("static/img/a.png"), "png").unwrap();
        let copies = Copy::parse(
            &["static:assets".to_string(), "missing:docs".to_string()],
            &root,
            false,
        )
        .unwrap();
        let out = tmp.path().join("dist");
        std::fs::create_dir_all(&out).unwrap();
        assert!(copies[0].apply(&out).unwrap());
        assert!(out.join("assets/img/a.png").is_file());
        assert!(
            !copies[1].apply(&out).unwrap(),
            "an optional copy is skipped"
        );
        let required = Copy::parse(&["missing:docs".to_string()], &root, true).unwrap();
        assert!(required[0].apply(&out).is_err());
        assert!(Copy::parse(&["a:../b".to_string()], &root, true).is_err());
        assert!(Copy::parse(&["nocolon".to_string()], &root, true).is_err());
    }
}
