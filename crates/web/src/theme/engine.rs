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

use super::page::{page_name, PageSettings};
use super::router::{Params, RouteTable};
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
    routes: Arc<RouteTable>,
    pages: HashMap<String, PageSettings>,
    /// Each page template's compiled-to-author line map.
    line_maps: HashMap<String, Vec<usize>>,
    /// Each page name's stored path (`home` to `home.htm`).
    page_paths: HashMap<String, String>,
    /// Each layout's settings bag, by path.
    layout_settings: HashMap<String, serde_json::Value>,
    /// Each page's whole settings bag, by name.
    page_bags: HashMap<String, serde_json::Value>,
}

/// A page's source as the environment compiles it, with a map from each
/// compiled line to the author's.
///
/// A page that names a layout and does not extend a template itself is
/// wrapped: `{% extends %}` and `{% block page %}` open on its first line, so
/// the body keeps its line numbers. A block the page defines at its top level
/// (`{% block title %}`) is lifted out of the page block to the end, blank
/// lines holding its place, so it fills the layout's block without rendering
/// where it stood; the map carries its lines back.
fn compose_page(source: &str, layout: Option<&str>) -> (String, Vec<usize>) {
    let lines = source.lines().count().max(1);
    let identity: Vec<usize> = (1..=lines).collect();
    let Some(layout) = layout else {
        return (source.to_string(), identity);
    };
    if source.contains("{% extends") || source.contains("{%- extends") {
        return (source.to_string(), identity);
    }
    let tag = regex::Regex::new(
        r"\{%-?\s*(block\s+[A-Za-z_][A-Za-z0-9_]*|endblock(?:\s+[A-Za-z_][A-Za-z0-9_]*)?)\s*-?%\}",
    )
    .expect("a fixed expression");
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    let mut depth = 0usize;
    let mut open = None;
    for m in tag.find_iter(source) {
        if m.as_str().contains("endblock") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                if let Some(start) = open.take() {
                    spans.push(start..m.end());
                }
            }
        } else {
            if depth == 0 {
                open = Some(m.start());
            }
            depth += 1;
        }
    }
    let mut body = String::with_capacity(source.len());
    let mut hoisted: Vec<(usize, &str)> = Vec::new();
    let mut at = 0;
    for span in &spans {
        body.push_str(&source[at..span.start]);
        let text = &source[span.clone()];
        body.extend(std::iter::repeat_n('\n', text.matches('\n').count()));
        hoisted.push((source[..span.start].matches('\n').count() + 1, text));
        at = span.end;
    }
    body.push_str(&source[at..]);
    let mut compiled = format!(
        "{{% extends {:?} %}}{{% block page %}}{body}{{% endblock %}}",
        template_name(Kind::Layout, layout)
    );
    let mut map: Vec<usize> = (1..=body.lines().count().max(1)).collect();
    for (origin, text) in hoisted {
        compiled.push('\n');
        compiled.push_str(text);
        map.extend(text.lines().enumerate().map(|(i, _)| origin + i));
    }
    (compiled, map)
}

/// The theme as templates see it: its settings values, then `code`, `name`,
/// `author`, `homepage` and `description`, which a setting cannot shadow.
fn theme_value(theme: &Theme) -> serde_json::Value {
    let mut map = match &theme.settings {
        serde_json::Value::Object(values) => values.clone(),
        _ => serde_json::Map::new(),
    };
    map.insert("code".into(), theme.code.clone().into());
    map.insert("name".into(), theme.config.name.clone().into());
    map.insert("author".into(), theme.config.author.clone().into());
    map.insert("homepage".into(), theme.config.homepage.clone().into());
    map.insert(
        "description".into(),
        theme.config.description.clone().into(),
    );
    serde_json::Value::Object(map)
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
        let mut page_rows: HashMap<String, (serde_json::Value, String)> = HashMap::new();
        let mut layout_settings: HashMap<String, serde_json::Value> = HashMap::new();
        // Ancestors first, so a nearer theme's row replaces a farther one's.
        for theme in chain.iter().rev() {
            for t in store::list_templates(db, theme.id, None).await? {
                match t.kind {
                    Kind::Content => {
                        content.insert(t.path, t.markup);
                    }
                    Kind::Page => {
                        page_rows.insert(t.path, (t.settings, t.markup));
                    }
                    Kind::Layout => {
                        layout_settings.insert(t.path.clone(), t.settings);
                        sources.insert(template_name(t.kind, &t.path), t.markup);
                    }
                    _ => {
                        sources.insert(template_name(t.kind, &t.path), t.markup);
                    }
                }
            }
        }
        // A page with settings that do not parse is left out of the routes
        // but still compiles, so the editor can open it.
        let mut routes = RouteTable::default();
        let mut pages: HashMap<String, PageSettings> = HashMap::new();
        let mut line_maps: HashMap<String, Vec<usize>> = HashMap::new();
        let mut page_paths: HashMap<String, String> = HashMap::new();
        let mut page_bags: HashMap<String, serde_json::Value> = HashMap::new();
        for (path, (settings, markup)) in page_rows {
            let name = page_name(&path).to_string();
            page_paths.insert(name.clone(), path.clone());
            page_bags.insert(name.clone(), settings.clone());
            let parsed = PageSettings::from_value(&settings).unwrap_or_default();
            if let Ok(pattern) = parsed.pattern() {
                routes.add(name.clone(), pattern);
            }
            let layout = parsed.layout_path();
            let (compiled, map) = compose_page(&markup, layout.as_deref());
            let template = template_name(Kind::Page, &path);
            sources.insert(template.clone(), compiled);
            line_maps.insert(template, map);
            pages.insert(name, parsed);
        }
        let routes = Arc::new(routes);
        let theme = chain
            .into_iter()
            .next()
            .expect("a chain starts with its theme");
        let mut env = base_environment();
        env.add_global("theme", Value::from(Serde(theme_value(&theme))));
        let content = Arc::new(content);
        env.add_function(
            "content",
            move |name: String, kwargs: Kwargs| -> Result<Value, minijinja::Error> {
                let Some(body) = content.get(&name) else {
                    return Err(minijinja::Error::new(
                        ErrorKind::TemplateNotFound,
                        format!("no content block {name:?}"),
                    ));
                };
                // `{name}` in the block is the named variable, escaped.
                let mut text = body.clone();
                for key in kwargs.args() {
                    let value: Value = kwargs.get(key)?;
                    text = text.replace(
                        &format!("{{{key}}}"),
                        &minijinja::HtmlEscape(&value.to_string()).to_string(),
                    );
                }
                kwargs.assert_all_used()?;
                Ok(Value::from_safe_string(render_content(&name, &text)))
            },
        );
        env.add_function("partial", partial);
        let code = theme.code.clone();
        env.add_function("asset", move |path: String| -> Value {
            Value::from_safe_string(format!(
                "/themes/{code}/assets/{}",
                path.trim_start_matches('/')
            ))
        });
        let table = Arc::clone(&routes);
        env.add_function(
            "page",
            move |name: String, kwargs: Kwargs| -> Result<Value, minijinja::Error> {
                let mut params = Params::new();
                for key in kwargs.args() {
                    params.insert(key.to_string(), kwargs.get::<String>(key)?);
                }
                kwargs.assert_all_used()?;
                table
                    .url(&name, &params)
                    .map(Value::from_safe_string)
                    .ok_or_else(|| {
                        minijinja::Error::new(
                            ErrorKind::InvalidOperation,
                            format!("no page {name:?}"),
                        )
                    })
            },
        );
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
            routes,
            pages,
            line_maps,
            page_paths,
            layout_settings,
            page_bags,
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

    /// The page a request path resolves to, with its parameters.
    pub fn resolve(&self, path: &str) -> Option<(&str, Params)> {
        self.routes.resolve(path)
    }

    /// A page's settings by name (its path without the extension).
    pub fn page(&self, name: &str) -> Option<&PageSettings> {
        self.pages.get(name)
    }

    /// A page's stored path by name.
    pub fn page_path(&self, name: &str) -> Option<&str> {
        self.page_paths.get(name).map(String::as_str)
    }

    /// A layout's settings bag by path.
    pub fn layout_settings(&self, path: &str) -> Option<&serde_json::Value> {
        self.layout_settings.get(path)
    }

    /// A page's whole settings bag by name, `components` included.
    pub fn page_bag(&self, name: &str) -> Option<&serde_json::Value> {
        self.page_bags.get(name)
    }

    /// The URL of the page `name` with `params`.
    pub fn url_for(&self, name: &str, params: &Params) -> Option<String> {
        self.routes.url(name, params)
    }

    /// The route table: every page's pattern in matching order.
    pub fn routes(&self) -> &RouteTable {
        &self.routes
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
        let name = template_name(kind, path);
        let template = self.env.get_template(&name)?;
        template
            .render(Value::from(Serde(ctx)))
            .map_err(|e| self.author_line(RenderError::from(e)))
    }

    /// An error's line as the author sees it, for a page the engine composed.
    pub fn author_line(&self, mut err: RenderError) -> RenderError {
        if let (Some(template), Some(line)) = (&err.template, err.line) {
            if let Some(map) = self.line_maps.get(template) {
                if let Some(original) = map.get(line.saturating_sub(1)) {
                    err.line = Some(*original);
                }
            }
        }
        err
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
/// named variables, the request's `this` and `head`, and the globals.
fn partial(state: &State, name: String, kwargs: Kwargs) -> Result<Value, minijinja::Error> {
    let template = state
        .env()
        .get_template(&template_name(Kind::Partial, &name))?;
    let mut pairs: Vec<(String, Value)> = ["this", "head"]
        .into_iter()
        .filter_map(|key| state.lookup(key).map(|v| (key.to_string(), v)))
        .collect();
    for key in kwargs.args() {
        pairs.push((key.to_string(), kwargs.get::<Value>(key)?));
    }
    kwargs.assert_all_used()?;
    Ok(Value::from_safe_string(
        template.render(Value::from_pairs(pairs))?,
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_composed_into_its_layout_with_its_blocks_lifted() {
        let (compiled, map) = compose_page(
            "a\n{% block title %}T\nU{% endblock %}\nb {{ x }}",
            Some("default.htm"),
        );
        assert_eq!(
            compiled,
            "{% extends \"layouts/default.htm\" %}{% block page %}a\n\n\nb {{ x }}{% endblock %}\n{% block title %}T\nU{% endblock %}"
        );
        // Body lines keep their numbers; the lifted block maps back to 2 and 3.
        assert_eq!(map, vec![1, 2, 3, 4, 2, 3]);

        let (plain, map) = compose_page("x\ny", None);
        assert_eq!(plain, "x\ny");
        assert_eq!(map, vec![1, 2]);
        let (own, _) = compose_page("{% extends \"layouts/a.htm\" %}", Some("default.htm"));
        assert!(own.starts_with("{% extends \"layouts/a.htm\" %}"));
        // An unmatched block is left where it is.
        let (unmatched, _) = compose_page("{% block t %}x", Some("default.htm"));
        assert!(unmatched.contains("{% block page %}{% block t %}x{% endblock %}"));
    }
}
