//! A theme rendered to files: every page whose URL has no parameter, at its
//! URL, through the same render path the site serves, plus the theme's text
//! assets. The output is a plain directory for any static host.

use super::engine::Engine;
use super::router::Params;
use super::{site, store, Kind, ThemeError};
use crate::StaticSite;
use laterite_core::Db;
use std::path::Path;

/// What a build wrote.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BuildReport {
    /// The pages written, by URL.
    pub pages: Vec<String>,
    /// The assets written, by path under the output directory.
    pub assets: Vec<String>,
    /// The pages left out: a URL with parameters, or a hidden page.
    pub skipped: Vec<String>,
}

/// Renders the theme `code` into `out_dir` as a site at `base_url`. A page
/// at `/404` is also written as `404.html`, the file most static hosts serve
/// for an unmatched URL.
pub async fn build(
    db: &Db,
    code: &str,
    out_dir: &Path,
    base_url: &str,
) -> Result<BuildReport, ThemeError> {
    build_with(db, code, out_dir, base_url, &[], &[]).await
}

/// Like [`build`], with the `markup` and `components` modules registered, as
/// the site has them.
pub async fn build_with(
    db: &Db,
    code: &str,
    out_dir: &Path,
    base_url: &str,
    markup: &[std::sync::Arc<dyn super::markup::Markup>],
    components: &[&super::component::ComponentReg],
) -> Result<BuildReport, ThemeError> {
    let theme = store::find_theme(db, code)
        .await?
        .ok_or_else(|| ThemeError::NotFound(code.to_string()))?;
    let origin = base_url.trim_end_matches('/').to_string();
    let engine = Engine::build_with(db, code, {
        let origin = origin.clone();
        move |env| {
            super::markup::install_all(markup, env);
            super::component::install_markup(components, env);
            site::add_url(env, origin);
        }
    })
    .await?;
    let mut out = StaticSite::new(out_dir, &origin)?;
    let mut report = BuildReport::default();
    let mut names: Vec<&str> = engine.routes().names();
    names.sort_unstable();
    for name in names {
        let settings = engine.page(name).cloned().unwrap_or_default();
        let pattern = match settings.pattern() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !pattern.is_static() || settings.hidden {
            report.skipped.push(name.to_string());
            continue;
        }
        let url = pattern.source().to_string();
        let extras = site::run_components(
            &engine,
            components,
            db,
            &theme,
            name,
            &Params::new(),
            &url,
            false,
        )
        .await?;
        let html = site::render_page_with(
            &engine,
            &theme,
            name,
            &Params::new(),
            &url,
            false,
            &origin,
            extras,
        )
        .map_err(ThemeError::Template)?;
        out.page(&url, &html)?;
        if url == "/404" {
            out.file("404.html", &html)?;
        }
        report.pages.push(url);
    }
    report.pages.sort();
    report.skipped.sort();
    for template in engine.names() {
        let Some(path) = template.strip_prefix("assets/") else {
            continue;
        };
        let body = engine
            .render(Kind::Asset, path, ())
            .map_err(ThemeError::Template)?;
        let target = format!("themes/{code}/assets/{path}");
        out.file(&target, &body)?;
        report.assets.push(target);
    }
    Ok(report)
}
