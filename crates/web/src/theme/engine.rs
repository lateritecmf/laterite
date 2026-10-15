//! Rendering: a theme's templates compiled into one environment.
//!
//! An [`Engine`] holds a theme's layouts, pages, partials and assets, with
//! every ancestor's filling in for the ones the theme lacks, as named
//! templates: `layouts/default.htm`, `pages/home.htm`, `partials/nav.htm`,
//! `assets/css/site.css`. A page extends a layout with `{% extends
//! "layouts/default.htm" %}` and fills its blocks; a template includes a
//! partial with `{% include "partials/nav.htm" %}` or renders one with its own
//! variables through `partial("nav.htm", title="Acme")`; `content("intro.md")`
//! places a content block, Markdown rendered to HTML. HTML is escaped
//! everywhere but inside an asset. A printed undefined value is an error; a
//! tested one is not.
//!
//! [`check`] parses a template without rendering it, so a save can refuse a
//! template that would fail. An [`EngineCache`] keeps one engine per theme and
//! rebuilds it when the theme or an ancestor changes.

use super::{store, Kind, Theme, ThemeError};
use laterite_core::Db;
use minijinja::value::{Kwargs, Serde, Value};
use minijinja::{AutoEscape, Environment, ErrorKind, State, UndefinedBehavior};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

/// The instruction budget a render may spend, so a runaway loop ends as an
/// error rather than a hung request.
const FUEL: u64 = 5_000_000;

/// The environment name of a template: the kind's folder and the path.
pub fn template_name(kind: Kind, path: &str) -> String {
    format!("{}/{path}", folder(kind))
}

fn folder(kind: Kind) -> &'static str {
    match kind {
        Kind::Layout => "layouts",
        Kind::Page => "pages",
        Kind::Partial => "partials",
        Kind::Content => "content",
        Kind::Asset => "assets",
    }
}

/// What went wrong rendering or parsing, with the template and line when known.
#[derive(Debug, Clone)]
pub struct RenderError {
    /// The environment name of the template the error is in.
    pub template: Option<String>,
    /// The line within it, from one.
    pub line: Option<usize>,
    /// The engine's error class, as text (`syntax error`, `undefined value`).
    pub kind: String,
    pub message: String,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.template, self.line) {
            (Some(t), Some(l)) => write!(f, "{t}:{l}: {}", self.message),
            (Some(t), None) => write!(f, "{t}: {}", self.message),
            _ => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<minijinja::Error> for RenderError {
    fn from(e: minijinja::Error) -> Self {
        let message = match e.detail() {
            Some(detail) => format!("{}: {detail}", e.kind()),
            None => e.kind().to_string(),
        };
        RenderError {
            template: e.name().map(str::to_string),
            line: e.line(),
            kind: e.kind().to_string(),
            message,
        }
    }
}

/// A theme's templates, compiled and ready to render.
pub struct Engine {
    env: Environment<'static>,
    theme: Theme,
    stamp: String,
    names: Vec<String>,
}

/// The theme as templates see it: `theme.name`, `theme.code`.
#[derive(Serialize)]
struct ThemeView<'a> {
    code: &'a str,
    name: &'a str,
    author: &'a str,
    homepage: &'a str,
    description: &'a str,
}

/// The stamp of a chain: every theme's `updated_at`, nearest first. A change
/// to the theme or any ancestor changes it.
fn stamp_of(chain: &[Theme]) -> String {
    chain
        .iter()
        .map(|t| format!("{}@{}", t.code, t.updated_at))
        .collect::<Vec<_>>()
        .join(";")
}

impl Engine {
    /// Compiles the theme `code` with its ancestors.
    pub async fn build(db: &Db, code: &str) -> Result<Engine, ThemeError> {
        Engine::build_with(db, code, |_| {}).await
    }

    /// Like [`Engine::build`], with a hook that adds the application's own
    /// functions, filters and globals to the environment before the templates
    /// compile.
    pub async fn build_with(
        db: &Db,
        code: &str,
        configure: impl FnOnce(&mut Environment<'static>),
    ) -> Result<Engine, ThemeError> {
        let chain = store::chain(db, code).await?;
        let stamp = stamp_of(&chain);
        let mut sources: HashMap<String, String> = HashMap::new();
        let mut content: HashMap<String, String> = HashMap::new();
        // Ancestors first, so a nearer theme's row replaces a farther one's.
        for theme in chain.iter().rev() {
            for t in store::list_templates(db, theme.id, None).await? {
                if t.kind == Kind::Content {
                    content.insert(t.path, t.markup);
                } else {
                    sources.insert(template_name(t.kind, &t.path), t.markup);
                }
            }
        }
        let theme = chain
            .into_iter()
            .next()
            .expect("a chain starts with its theme");
        let mut env = base_environment();
        env.add_global(
            "theme",
            Value::from(Serde(ThemeView {
                code: &theme.code,
                name: &theme.config.name,
                author: &theme.config.author,
                homepage: &theme.config.homepage,
                description: &theme.config.description,
            })),
        );
        let content = Arc::new(content);
        env.add_function(
            "content",
            move |name: String| -> Result<Value, minijinja::Error> {
                match content.get(&name) {
                    Some(body) => Ok(Value::from_safe_string(render_content(&name, body))),
                    None => Err(minijinja::Error::new(
                        ErrorKind::TemplateNotFound,
                        format!("no content block {name:?}"),
                    )),
                }
            },
        );
        env.add_function("partial", partial);
        configure(&mut env);
        let mut names: Vec<String> = sources.keys().cloned().collect();
        names.sort();
        for (name, source) in sources {
            env.add_template_owned(name, source)
                .map_err(|e| ThemeError::Template(RenderError::from(e)))?;
        }
        Ok(Engine {
            env,
            theme,
            stamp,
            names,
        })
    }

    /// The theme this engine renders.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Changes when the theme or an ancestor does; [`EngineCache`] compares it.
    pub fn stamp(&self) -> &str {
        &self.stamp
    }

    /// The environment, for a render path that needs more than [`Engine::render`].
    pub fn env(&self) -> &Environment<'static> {
        &self.env
    }

    /// Every compiled template's name, sorted.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Whether the theme, or an ancestor, has this template.
    pub fn has(&self, kind: Kind, path: &str) -> bool {
        self.names.binary_search(&template_name(kind, path)).is_ok()
    }

    /// Renders a template with `ctx` as its variables.
    pub fn render(
        &self,
        kind: Kind,
        path: &str,
        ctx: impl Serialize,
    ) -> Result<String, RenderError> {
        let template = self.env.get_template(&template_name(kind, path))?;
        Ok(template.render(Value::from(Serde(ctx)))?)
    }
}

/// The environment every theme compiles into: HTML escaped outside assets,
/// a printed undefined value an error, a fuel budget.
fn base_environment() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_auto_escape_callback(|name| {
        if name.starts_with("assets/") {
            AutoEscape::None
        } else {
            AutoEscape::Html
        }
    });
    env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
    env.set_fuel(Some(FUEL));
    env
}

/// `partial("nav.htm", title="Acme")`: renders `partials/nav.htm` with the
/// named variables and the globals.
fn partial(state: &State, name: String, kwargs: Kwargs) -> Result<Value, minijinja::Error> {
    let template = state
        .env()
        .get_template(&template_name(Kind::Partial, &name))?;
    let ctx = Value::from_pairs(
        kwargs
            .args()
            .map(|k| (k.to_string(), kwargs.get::<Value>(k).unwrap_or_default())),
    );
    kwargs.assert_all_used()?;
    Ok(Value::from_safe_string(template.render(ctx)?))
}

/// A content block as HTML: Markdown rendered (`.md`), HTML as it is (`.htm`,
/// `.html`), anything else escaped as text.
pub fn render_content(path: &str, body: &str) -> String {
    let ext = path.rsplit('.').next().unwrap_or_default();
    match ext {
        "md" | "markdown" => {
            let mut options = pulldown_cmark::Options::empty();
            options.insert(pulldown_cmark::Options::ENABLE_TABLES);
            options.insert(pulldown_cmark::Options::ENABLE_STRIKETHROUGH);
            options.insert(pulldown_cmark::Options::ENABLE_FOOTNOTES);
            let parser = pulldown_cmark::Parser::new_ext(body, options);
            let mut html = String::with_capacity(body.len() * 2);
            pulldown_cmark::html::push_html(&mut html, parser);
            html
        }
        "htm" | "html" => body.to_string(),
        _ => minijinja::HtmlEscape(body).to_string(),
    }
}

/// Parses a template's markup without rendering it. The error carries the
/// line, so an editor can point at it.
pub fn check(source: &str) -> Result<(), RenderError> {
    let env = base_environment();
    env.template_from_str(source)?;
    Ok(())
}

/// One engine per theme, rebuilt when the theme or an ancestor changes.
#[derive(Default)]
pub struct EngineCache {
    engines: RwLock<HashMap<String, Arc<Engine>>>,
}

impl EngineCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The engine for `code`, built or rebuilt as its stamp requires.
    pub async fn get(&self, db: &Db, code: &str) -> Result<Arc<Engine>, ThemeError> {
        self.get_with(db, code, |_| {}).await
    }

    /// Like [`EngineCache::get`], configuring a rebuilt engine as
    /// [`Engine::build_with`] does.
    pub async fn get_with(
        &self,
        db: &Db,
        code: &str,
        configure: impl FnOnce(&mut Environment<'static>),
    ) -> Result<Arc<Engine>, ThemeError> {
        let chain = store::chain(db, code).await?;
        let stamp = stamp_of(&chain);
        if let Some(engine) = self.engines.read().expect("engine cache").get(code) {
            if engine.stamp == stamp {
                return Ok(Arc::clone(engine));
            }
        }
        let engine = Arc::new(Engine::build_with(db, code, configure).await?);
        self.engines
            .write()
            .expect("engine cache")
            .insert(code.to_string(), Arc::clone(&engine));
        Ok(engine)
    }

    /// Drops the engine for `code`, so the next request rebuilds it.
    pub fn invalidate(&self, code: &str) {
        self.engines.write().expect("engine cache").remove(code);
    }
}
