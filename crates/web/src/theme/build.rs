//! A theme rendered to files: every page whose URL has no parameter, at its
//! URL, through the same render path the site serves, plus the theme's text
//! assets. The output is a plain directory for any static host.

use super::engine::Engine;
use super::router::Params;
use super::{site, store, Kind, ThemeError};
use crate::StaticSite;
use laterite_core::Db;
use std::path::Path;
use std::sync::Arc;

use laterite_core::strata::async_trait;
use laterite_core::AnyRowExt;
use sea_query::{Alias, Expr, Order, Query};

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
    build_with(db, code, out_dir, base_url, &[], &[], &[]).await
}

/// The parameter values a page pattern is built for: one page per set.
#[async_trait]
pub trait Paths: Send + Sync {
    /// Every set of parameters, each the URL of one page.
    async fn params(&self, db: &Db) -> Result<Vec<Params>, ThemeError>;
}

/// A module's answer to which pages a pattern has (`/events/:slug`): the
/// build renders the page once per set of parameters. Registered as a
/// contribution; a pattern nobody answers for is skipped, as before.
pub struct PathsReg {
    pub pattern: String,
    pub paths: Arc<dyn Paths>,
}

impl PathsReg {
    pub fn new(pattern: impl Into<String>, paths: Arc<dyn Paths>) -> Self {
        Self {
            pattern: pattern.into(),
            paths,
        }
    }
}

/// The values of one column, each under the parameter named: the keys of a
/// table as the pages of `/<table>/:slug`.
pub struct TableKeys {
    table: String,
    column: String,
    param: String,
}

impl TableKeys {
    /// `column` of `table` as the pattern's parameter of the same name.
    pub fn new(table: impl Into<String>, column: impl Into<String>) -> Self {
        let column = column.into();
        Self {
            table: table.into(),
            param: column.clone(),
            column,
        }
    }

    /// Names the parameter when it differs from the column.
    pub fn param(mut self, param: impl Into<String>) -> Self {
        self.param = param.into();
        self
    }
}

#[async_trait]
impl Paths for TableKeys {
    async fn params(&self, db: &Db) -> Result<Vec<Params>, ThemeError> {
        let (sql, values) = laterite_core::query::build(
            db.backend,
            Query::select()
                .expr_as(
                    Expr::col(Alias::new(&self.column))
                        .cast_as(Alias::new(laterite_core::query::text_cast(db.backend))),
                    Alias::new("key"),
                )
                .from(Alias::new(&self.table))
                .order_by(Alias::new(&self.column), Order::Asc)
                .to_owned(),
        );
        let rows = laterite_core::query::bind_values(sqlx::query(&sql), values)
            .fetch_all(&db.pool)
            .await
            .map_err(|e| ThemeError::Config(e.to_string()))?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(key) = row
                .get_text_opt("key")
                .map_err(|e| ThemeError::Config(e.to_string()))?
                .filter(|k| !k.is_empty())
            {
                let mut params = Params::new();
                params.insert(self.param.clone(), key);
                out.push(params);
            }
        }
        Ok(out)
    }
}

/// Whether a URL's last segment carries an extension, as a file does.
fn names_file(url: &str) -> bool {
    url.rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.') && !last.starts_with('.'))
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
    paths: &[&PathsReg],
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
        if settings.hidden {
            report.skipped.push(name.to_string());
            continue;
        }
        // A page with parameters is built once per set a module answers
        // for its pattern; one nobody answers for is skipped.
        let sets: Vec<Params> = if pattern.is_static() {
            vec![Params::new()]
        } else {
            let mut sets = Vec::new();
            for reg in paths.iter().filter(|r| r.pattern == pattern.source()) {
                sets.extend(reg.paths.params(db).await?);
            }
            if sets.is_empty() {
                report.skipped.push(name.to_string());
                continue;
            }
            sets
        };
        for params in sets {
            let Some(url) = pattern.fill(&params) else {
                continue;
            };
            // A clean URL is served with a trailing slash by a static host
            // (`about/index.html` at `/about/`), so that is the URL the page's
            // canonical link and `this.url` carry; the root and a file keep theirs.
            let served = if url == "/" || names_file(&url) {
                url.clone()
            } else {
                format!("{}/", url.trim_end_matches('/'))
            };
            let extras = site::run_components(
                &engine, components, db, &theme, name, &params, &served, false,
            )
            .await?;
            let html = site::render_page_with(
                &engine, &theme, name, &params, &served, false, &origin, extras,
            )
            .map_err(ThemeError::Template)?;
            let html = site::finish_page(
                &engine, components, db, &theme, name, &params, &served, false, &origin, html,
            )
            .await
            .map_err(ThemeError::Template)?;
            // A URL naming a file (`/robots.txt`, `/sitemap.xml`) is written as
            // that file; every other URL is a clean one.
            if names_file(&url) {
                out.file(url.trim_start_matches('/'), &html)?;
            } else {
                out.page(&url, &html)?;
                if url == "/404" {
                    out.file("404.html", &html)?;
                }
            }
            report.pages.push(url);
        }
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
