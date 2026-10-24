//! Mail templates, layouts and partials: files a module registers, rows an
//! admin may edit, and the renderer that turns a template and its variables
//! into a subject, a text body and an HTML body.
//!
//! A file has a header of `key = "value"` lines, then sections split by a
//! line of `==`: the text body, the HTML body and, for a layout, the CSS.
//! Bodies are templates (`{{ name }}`, `{% if %}`, `{{ text|markdown }}`);
//! a template's HTML body is Markdown. Partials are called as
//! `{{ partial("button", url=link, body="Sign in") }}`; a longer body is
//! captured first with `{% set body %}...{% endset %}`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use minijinja::value::{Kwargs, Value};
use minijinja::{Environment, State, UndefinedBehavior};
use sea_query::{Expr, Iden, OnConflict, Order, Query};
use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::error::{CoreError, CoreResult};
use crate::query::{bind_values, build, AnyRowExt};
use crate::Db;

/// A template file under a code; see [`super::MailRegistry`].
#[derive(Debug, Clone, Copy)]
pub struct MailTemplateReg {
    pub code: &'static str,
    pub file: &'static str,
}

/// A layout file under a code.
#[derive(Debug, Clone, Copy)]
pub struct MailLayoutReg {
    pub code: &'static str,
    pub file: &'static str,
}

/// A partial file under a code.
#[derive(Debug, Clone, Copy)]
pub struct MailPartialReg {
    pub code: &'static str,
    pub file: &'static str,
}

/// The cache namespace every rendered template depends on; an edit bumps
/// it.
pub const NAMESPACE: &str = "laterite.mail::templates";

/// A parsed file: its header and its sections.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sections {
    pub settings: BTreeMap<String, String>,
    pub text: String,
    pub html: String,
    pub css: String,
}

/// Parses a sectioned file. With one section it is the HTML body alone;
/// with two, the header and the HTML body; with three, header, text and
/// HTML; with four, header, text, HTML and CSS.
pub fn parse_file(file: &str) -> Sections {
    let parts: Vec<&str> = file.split("\n==\n").collect();
    let parts: Vec<String> = parts
        .into_iter()
        .map(|p| p.trim_matches('\n').to_string())
        .collect();
    let (header, text, html, css) = match parts.len() {
        0 => (String::new(), String::new(), String::new(), String::new()),
        1 => (
            String::new(),
            String::new(),
            parts[0].clone(),
            String::new(),
        ),
        2 => (
            parts[0].clone(),
            String::new(),
            parts[1].clone(),
            String::new(),
        ),
        3 => (
            parts[0].clone(),
            parts[1].clone(),
            parts[2].clone(),
            String::new(),
        ),
        _ => (
            parts[0].clone(),
            parts[1].clone(),
            parts[2].clone(),
            parts[3].clone(),
        ),
    };
    let mut settings = BTreeMap::new();
    for line in header.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            settings.insert(key.trim().to_string(), value);
        }
    }
    Sections {
        settings,
        text,
        html,
        css,
    }
}

/// A template row: what a message is rendered from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub code: String,
    pub subject: String,
    pub description: String,
    /// The layout's code; `None` renders the body alone.
    pub layout: Option<String>,
    pub content_text: String,
    pub content_html: String,
    /// Edited in the admin: the file no longer refreshes it.
    pub is_custom: bool,
}

/// A layout row: what wraps a template's body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub code: String,
    pub name: String,
    pub content_text: String,
    pub content_html: String,
    pub content_css: String,
    pub is_custom: bool,
}

/// A partial row: a piece a body or a layout calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Partial {
    pub code: String,
    pub name: String,
    pub content_text: String,
    pub content_html: String,
    pub is_custom: bool,
}

/// What rendering produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rendered {
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
}

#[derive(Iden)]
pub(crate) enum MailTemplates {
    Table,
    Code,
    Subject,
    Description,
    Layout,
    ContentText,
    ContentHtml,
    IsCustom,
}

#[derive(Iden)]
pub(crate) enum MailLayouts {
    Table,
    Code,
    Name,
    ContentText,
    ContentHtml,
    ContentCss,
    IsCustom,
}

#[derive(Iden)]
pub(crate) enum MailPartials {
    Table,
    Code,
    Name,
    ContentText,
    ContentHtml,
    IsCustom,
}

/// The files as registered, kept so a row resets to its file.
#[derive(Default)]
struct Files {
    templates: HashMap<&'static str, &'static str>,
    layouts: HashMap<&'static str, &'static str>,
    partials: HashMap<&'static str, &'static str>,
}

struct Inner {
    db: Db,
    cache: Cache,
    files: RwLock<Files>,
    brand_css: RwLock<String>,
    app_name: RwLock<String>,
}

/// The templates handle: reads, edits, seeding and rendering.
#[derive(Clone)]
pub struct Templates {
    inner: Arc<Inner>,
}

const TTL: Duration = Duration::from_secs(3600);

impl Templates {
    pub fn new(db: Db, cache: Cache) -> Self {
        cache.allow(NAMESPACE);
        Self {
            inner: Arc::new(Inner {
                db,
                cache,
                files: RwLock::new(Files::default()),
                brand_css: RwLock::new(String::new()),
                app_name: RwLock::new("Laterite".to_string()),
            }),
        }
    }

    /// The application name layouts show.
    pub fn set_app_name(&self, name: &str) {
        *self
            .inner
            .app_name
            .write()
            .unwrap_or_else(|e| e.into_inner()) = name.to_string();
    }

    /// The brand stylesheet every layout includes before its own CSS.
    pub async fn set_brand_css(&self, css: &str) {
        *self
            .inner
            .brand_css
            .write()
            .unwrap_or_else(|e| e.into_inner()) = css.to_string();
        let _ = self.inner.cache.bump(NAMESPACE).await;
    }

    pub fn brand_css(&self) -> String {
        self.inner
            .brand_css
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Seeds the tables from the registered files: a new code is inserted,
    /// a row the admin has not edited is refreshed, a code no file names
    /// is removed unless edited.
    pub async fn seed(
        &self,
        templates: &[MailTemplateReg],
        layouts: &[MailLayoutReg],
        partials: &[MailPartialReg],
    ) -> CoreResult<()> {
        {
            let mut files = self.inner.files.write().unwrap_or_else(|e| e.into_inner());
            files.templates = templates.iter().map(|r| (r.code, r.file)).collect();
            files.layouts = layouts.iter().map(|r| (r.code, r.file)).collect();
            files.partials = partials.iter().map(|r| (r.code, r.file)).collect();
        }
        let db = &self.inner.db;
        for reg in templates {
            let row = template_from_file(reg.code, reg.file);
            if find_template(db, reg.code)
                .await?
                .is_none_or(|t| !t.is_custom)
            {
                upsert_template(db, &row).await?;
            }
        }
        for reg in layouts {
            let row = layout_from_file(reg.code, reg.file);
            if find_layout(db, reg.code)
                .await?
                .is_none_or(|l| !l.is_custom)
            {
                upsert_layout(db, &row).await?;
            }
        }
        for reg in partials {
            let row = partial_from_file(reg.code, reg.file);
            if find_partial(db, reg.code)
                .await?
                .is_none_or(|p| !p.is_custom)
            {
                upsert_partial(db, &row).await?;
            }
        }
        for t in list_templates(db).await? {
            if !t.is_custom && !templates.iter().any(|r| r.code == t.code) {
                delete_template(db, &t.code).await?;
            }
        }
        for l in list_layouts(db).await? {
            if !l.is_custom && !layouts.iter().any(|r| r.code == l.code) {
                delete_layout(db, &l.code).await?;
            }
        }
        for p in list_partials(db).await? {
            if !p.is_custom && !partials.iter().any(|r| r.code == p.code) {
                delete_partial(db, &p.code).await?;
            }
        }
        let _ = self.inner.cache.bump(NAMESPACE).await;
        Ok(())
    }

    pub async fn templates(&self) -> CoreResult<Vec<Template>> {
        list_templates(&self.inner.db).await
    }

    pub async fn layouts(&self) -> CoreResult<Vec<Layout>> {
        list_layouts(&self.inner.db).await
    }

    pub async fn partials(&self) -> CoreResult<Vec<Partial>> {
        list_partials(&self.inner.db).await
    }

    pub async fn template(&self, code: &str) -> CoreResult<Option<Template>> {
        find_template(&self.inner.db, code).await
    }

    pub async fn layout(&self, code: &str) -> CoreResult<Option<Layout>> {
        find_layout(&self.inner.db, code).await
    }

    pub async fn partial(&self, code: &str) -> CoreResult<Option<Partial>> {
        find_partial(&self.inner.db, code).await
    }

    /// Writes a template as edited; it is custom from now and its file no
    /// longer refreshes it.
    pub async fn save_template(&self, template: &Template) -> CoreResult<()> {
        let mut row = template.clone();
        row.is_custom = true;
        upsert_template(&self.inner.db, &row).await?;
        self.bump().await;
        Ok(())
    }

    pub async fn save_layout(&self, layout: &Layout) -> CoreResult<()> {
        let mut row = layout.clone();
        row.is_custom = true;
        upsert_layout(&self.inner.db, &row).await?;
        self.bump().await;
        Ok(())
    }

    pub async fn save_partial(&self, partial: &Partial) -> CoreResult<()> {
        let mut row = partial.clone();
        row.is_custom = true;
        upsert_partial(&self.inner.db, &row).await?;
        self.bump().await;
        Ok(())
    }

    /// Puts a template back to its file; `false` when no file registers
    /// the code (a custom one the admin made), in which case nothing moves.
    pub async fn reset_template(&self, code: &str) -> CoreResult<bool> {
        let file = self.file_of(|f| f.templates.get(code).copied());
        let Some(file) = file else {
            return Ok(false);
        };
        upsert_template(&self.inner.db, &template_from_file(code, file)).await?;
        self.bump().await;
        Ok(true)
    }

    pub async fn reset_layout(&self, code: &str) -> CoreResult<bool> {
        let file = self.file_of(|f| f.layouts.get(code).copied());
        let Some(file) = file else {
            return Ok(false);
        };
        upsert_layout(&self.inner.db, &layout_from_file(code, file)).await?;
        self.bump().await;
        Ok(true)
    }

    pub async fn reset_partial(&self, code: &str) -> CoreResult<bool> {
        let file = self.file_of(|f| f.partials.get(code).copied());
        let Some(file) = file else {
            return Ok(false);
        };
        upsert_partial(&self.inner.db, &partial_from_file(code, file)).await?;
        self.bump().await;
        Ok(true)
    }

    /// Removes a template no file registers; one a file registers comes
    /// back at the next boot, so it is reset instead.
    pub async fn delete_template(&self, code: &str) -> CoreResult<bool> {
        if self.file_of(|f| f.templates.get(code).copied()).is_some() {
            return self.reset_template(code).await;
        }
        delete_template(&self.inner.db, code).await?;
        self.bump().await;
        Ok(true)
    }

    pub async fn delete_layout(&self, code: &str) -> CoreResult<bool> {
        if self.file_of(|f| f.layouts.get(code).copied()).is_some() {
            return self.reset_layout(code).await;
        }
        delete_layout(&self.inner.db, code).await?;
        self.bump().await;
        Ok(true)
    }

    pub async fn delete_partial(&self, code: &str) -> CoreResult<bool> {
        if self.file_of(|f| f.partials.get(code).copied()).is_some() {
            return self.reset_partial(code).await;
        }
        delete_partial(&self.inner.db, code).await?;
        self.bump().await;
        Ok(true)
    }

    /// Whether a file registers `code` as a template.
    pub fn has_template_file(&self, code: &str) -> bool {
        self.file_of(|f| f.templates.get(code).copied()).is_some()
    }

    pub fn has_layout_file(&self, code: &str) -> bool {
        self.file_of(|f| f.layouts.get(code).copied()).is_some()
    }

    pub fn has_partial_file(&self, code: &str) -> bool {
        self.file_of(|f| f.partials.get(code).copied()).is_some()
    }

    fn file_of(&self, pick: impl FnOnce(&Files) -> Option<&'static str>) -> Option<&'static str> {
        let files = self.inner.files.read().unwrap_or_else(|e| e.into_inner());
        pick(&files)
    }

    async fn bump(&self) {
        let _ = self.inner.cache.bump(NAMESPACE).await;
    }

    /// Renders the template `code` with `vars`: the subject, the text body
    /// and the HTML body wrapped in the layout with the brand stylesheet
    /// inlined. An unregistered code is `NotFound`; an undefined variable
    /// renders empty.
    pub async fn render(&self, code: &str, vars: &serde_json::Value) -> CoreResult<Rendered> {
        let bundle = self.bundle(code).await?;
        self.render_bundle(&bundle, vars)
    }

    /// Renders `template` as given (an unsaved edit) with `vars`.
    pub async fn render_draft(
        &self,
        template: &Template,
        vars: &serde_json::Value,
    ) -> CoreResult<Rendered> {
        let mut bundle = self.pieces().await?;
        bundle.template = template.clone();
        self.render_bundle(&bundle, vars)
    }

    async fn pieces(&self) -> CoreResult<Bundle> {
        let key = "laterite.mail::pieces";
        let pieces: Pieces = self
            .inner
            .cache
            .remember(key, TTL, &[NAMESPACE], || async {
                Ok(Pieces {
                    layouts: list_layouts(&self.inner.db).await?,
                    partials: list_partials(&self.inner.db).await?,
                })
            })
            .await?;
        Ok(Bundle {
            template: Template {
                code: String::new(),
                subject: String::new(),
                description: String::new(),
                layout: None,
                content_text: String::new(),
                content_html: String::new(),
                is_custom: false,
            },
            layouts: pieces.layouts,
            partials: pieces.partials,
        })
    }

    async fn bundle(&self, code: &str) -> CoreResult<Bundle> {
        let key = format!("laterite.mail::template:{code}");
        let template: Option<Template> = self
            .inner
            .cache
            .remember(&key, TTL, &[NAMESPACE], || {
                find_template(&self.inner.db, code)
            })
            .await?;
        let template =
            template.ok_or_else(|| CoreError::NotFound(format!("mail template `{code}`")))?;
        let mut bundle = self.pieces().await?;
        bundle.template = template;
        Ok(bundle)
    }

    fn render_bundle(&self, bundle: &Bundle, vars: &serde_json::Value) -> CoreResult<Rendered> {
        let app_name = self
            .inner
            .app_name
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let brand_css = self.brand_css();
        let layout = bundle
            .template
            .layout
            .as_deref()
            .and_then(|code| bundle.layouts.iter().find(|l| l.code == code));
        let mut base = minijinja::context! { app_name => app_name.clone() };
        if let serde_json::Value::Object(_) = vars {
            base = minijinja::context! { ..Value::from(minijinja::value::Serde(vars)), ..base };
        }

        // HTML.
        let mut html_env = environment(&bundle.partials, false);
        html_env.add_template_owned("subject".to_string(), bundle.template.subject.clone())?;
        html_env.add_template_owned("body".to_string(), bundle.template.content_html.clone())?;
        if let Some(layout) = layout {
            html_env.add_template_owned("layout".to_string(), layout.content_html.clone())?;
            html_env.add_template_owned("css".to_string(), layout.content_css.clone())?;
        }
        let subject = html_env.get_template("subject")?.render(&base)?;
        let body_source = html_env.get_template("body")?.render(&base)?;
        let body_html = crate::markdown::render(&body_source);
        let html = match layout {
            Some(_) => {
                let brand = brand_values(&brand_css);
                let css = html_env
                    .get_template("css")?
                    .render(minijinja::context! { brand => brand, ..base.clone() })?;
                let page = html_env
                    .get_template("layout")?
                    .render(minijinja::context! {
                        content => Value::from_safe_string(body_html),
                        css => Value::from_safe_string(css.clone()),
                        brand_css => Value::from_safe_string(brand_css.clone()),
                        subject => subject.clone(),
                        ..base.clone()
                    })?;
                let inliner = css_inline::CSSInliner::options()
                    .load_remote_stylesheets(false)
                    .extra_css(Some(std::borrow::Cow::Owned(format!("{brand_css}\n{css}"))))
                    .build();
                inliner.inline(&page).map_err(|e| {
                    CoreError::Internal(format!("mail css could not be inlined: {e}"))
                })?
            }
            None => body_html,
        };

        // Text.
        let mut text_env = environment(&bundle.partials, true);
        text_env.add_template_owned("body".to_string(), bundle.template.content_text.clone())?;
        if let Some(layout) = layout {
            text_env.add_template_owned("layout".to_string(), layout.content_text.clone())?;
        }
        let body_text = if bundle.template.content_text.trim().is_empty() {
            text_of(&body_source)
        } else {
            text_env.get_template("body")?.render(&base)?
        };
        let text = match layout.filter(|l| !l.content_text.trim().is_empty()) {
            Some(_) => text_env
                .get_template("layout")?
                .render(minijinja::context! {
                    content => body_text,
                    subject => subject.clone(),
                    ..base
                })?,
            None => body_text,
        };
        Ok(Rendered {
            subject: subject.trim().to_string(),
            text: tidy_text(&text),
            html: Some(html),
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Pieces {
    layouts: Vec<Layout>,
    partials: Vec<Partial>,
}

struct Bundle {
    template: Template,
    layouts: Vec<Layout>,
    partials: Vec<Partial>,
}

impl From<minijinja::Error> for CoreError {
    fn from(e: minijinja::Error) -> Self {
        CoreError::Internal(format!("mail template: {e}"))
    }
}

/// An environment with the partials loaded, the `partial()` function and
/// the `markdown` filter; undefined variables render empty.
fn environment(partials: &[Partial], text: bool) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Lenient);
    for partial in partials {
        let source = if text {
            partial.content_text.clone()
        } else {
            partial.content_html.clone()
        };
        let _ = env.add_template_owned(format!("partial:{}", partial.code), source);
    }
    env.add_filter("markdown", |value: String| -> Value {
        Value::from_safe_string(crate::markdown::render(&value))
    });
    env.add_function(
        "partial",
        |state: &State, name: String, kwargs: Kwargs| -> Result<Value, minijinja::Error> {
            let template = state.env().get_template(&format!("partial:{name}"))?;
            let mut ctx: BTreeMap<String, Value> = BTreeMap::new();
            for key in kwargs.args() {
                ctx.insert(key.to_string(), kwargs.get(key)?);
            }
            // The caller's own variables stay visible to the partial.
            let parent = state.lookup("app_name");
            if let Some(app_name) = parent {
                ctx.entry("app_name".to_string()).or_insert(app_name);
            }
            kwargs.assert_all_used()?;
            let out = template.render(Value::from_object(ctx))?;
            Ok(Value::from_safe_string(out))
        },
    );
    env
}

/// The brand stylesheet's custom properties as a map, for a layout's CSS to
/// read as `{{ brand.button_bg }}`.
fn brand_values(css: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for part in css.split(';') {
        let Some(at) = part.find("--mail-") else {
            continue;
        };
        let rest = &part[at + "--mail-".len()..];
        {
            if let Some((key, value)) = rest.split_once(':') {
                out.insert(
                    key.trim().replace('-', "_"),
                    value.trim().trim_end_matches('}').trim().to_string(),
                );
            }
        }
    }
    out
}

/// A plain-text reading of a Markdown or HTML body: tags gone, entities
/// resolved, blank lines kept.
fn text_of(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut in_tag = false;
    for ch in source.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn tidy_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

fn template_from_file(code: &str, file: &str) -> Template {
    let s = parse_file(file);
    Template {
        code: code.to_string(),
        subject: s.settings.get("subject").cloned().unwrap_or_default(),
        description: s.settings.get("description").cloned().unwrap_or_default(),
        layout: s
            .settings
            .get("layout")
            .cloned()
            .filter(|l| !l.is_empty())
            .or_else(|| Some("default".to_string())),
        content_text: s.text,
        content_html: s.html,
        is_custom: false,
    }
}

fn layout_from_file(code: &str, file: &str) -> Layout {
    let s = parse_file(file);
    Layout {
        code: code.to_string(),
        name: s
            .settings
            .get("name")
            .cloned()
            .unwrap_or_else(|| code.to_string()),
        content_text: s.text,
        content_html: s.html,
        content_css: s.css,
        is_custom: false,
    }
}

fn partial_from_file(code: &str, file: &str) -> Partial {
    let s = parse_file(file);
    Partial {
        code: code.to_string(),
        name: s
            .settings
            .get("name")
            .cloned()
            .unwrap_or_else(|| code.to_string()),
        content_text: s.text,
        content_html: s.html,
        is_custom: false,
    }
}

// ---- rows

fn template_of(r: &sqlx::any::AnyRow) -> CoreResult<Template> {
    Ok(Template {
        code: r.get_text("code")?,
        subject: r.get_text("subject")?,
        description: r.get_text("description")?,
        layout: r.get_text_opt("layout")?.filter(|l| !l.is_empty()),
        content_text: r.get_text("content_text")?,
        content_html: r.get_text("content_html")?,
        is_custom: r.get_bool("is_custom")?,
    })
}

async fn list_templates(db: &Db) -> CoreResult<Vec<Template>> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                MailTemplates::Code,
                MailTemplates::Subject,
                MailTemplates::Description,
                MailTemplates::Layout,
                MailTemplates::ContentText,
                MailTemplates::ContentHtml,
                MailTemplates::IsCustom,
            ])
            .from(MailTemplates::Table)
            .order_by(MailTemplates::Code, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter().map(template_of).collect()
}

async fn find_template(db: &Db, code: &str) -> CoreResult<Option<Template>> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                MailTemplates::Code,
                MailTemplates::Subject,
                MailTemplates::Description,
                MailTemplates::Layout,
                MailTemplates::ContentText,
                MailTemplates::ContentHtml,
                MailTemplates::IsCustom,
            ])
            .from(MailTemplates::Table)
            .and_where(Expr::col(MailTemplates::Code).eq(code))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    row.as_ref().map(template_of).transpose()
}

async fn upsert_template(db: &Db, t: &Template) -> CoreResult<()> {
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(MailTemplates::Table)
            .columns([
                MailTemplates::Code,
                MailTemplates::Subject,
                MailTemplates::Description,
                MailTemplates::Layout,
                MailTemplates::ContentText,
                MailTemplates::ContentHtml,
                MailTemplates::IsCustom,
            ])
            .values_panic([
                t.code.as_str().into(),
                t.subject.as_str().into(),
                t.description.as_str().into(),
                t.layout.clone().into(),
                t.content_text.as_str().into(),
                t.content_html.as_str().into(),
                t.is_custom.into(),
            ])
            .on_conflict(
                OnConflict::column(MailTemplates::Code)
                    .update_columns([
                        MailTemplates::Subject,
                        MailTemplates::Description,
                        MailTemplates::Layout,
                        MailTemplates::ContentText,
                        MailTemplates::ContentHtml,
                        MailTemplates::IsCustom,
                    ])
                    .to_owned(),
            )
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

async fn delete_template(db: &Db, code: &str) -> CoreResult<()> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(MailTemplates::Table)
            .and_where(Expr::col(MailTemplates::Code).eq(code))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

fn layout_of(r: &sqlx::any::AnyRow) -> CoreResult<Layout> {
    Ok(Layout {
        code: r.get_text("code")?,
        name: r.get_text("name")?,
        content_text: r.get_text("content_text")?,
        content_html: r.get_text("content_html")?,
        content_css: r.get_text("content_css")?,
        is_custom: r.get_bool("is_custom")?,
    })
}

async fn list_layouts(db: &Db) -> CoreResult<Vec<Layout>> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                MailLayouts::Code,
                MailLayouts::Name,
                MailLayouts::ContentText,
                MailLayouts::ContentHtml,
                MailLayouts::ContentCss,
                MailLayouts::IsCustom,
            ])
            .from(MailLayouts::Table)
            .order_by(MailLayouts::Code, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter().map(layout_of).collect()
}

async fn find_layout(db: &Db, code: &str) -> CoreResult<Option<Layout>> {
    Ok(list_layouts(db).await?.into_iter().find(|l| l.code == code))
}

async fn upsert_layout(db: &Db, l: &Layout) -> CoreResult<()> {
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(MailLayouts::Table)
            .columns([
                MailLayouts::Code,
                MailLayouts::Name,
                MailLayouts::ContentText,
                MailLayouts::ContentHtml,
                MailLayouts::ContentCss,
                MailLayouts::IsCustom,
            ])
            .values_panic([
                l.code.as_str().into(),
                l.name.as_str().into(),
                l.content_text.as_str().into(),
                l.content_html.as_str().into(),
                l.content_css.as_str().into(),
                l.is_custom.into(),
            ])
            .on_conflict(
                OnConflict::column(MailLayouts::Code)
                    .update_columns([
                        MailLayouts::Name,
                        MailLayouts::ContentText,
                        MailLayouts::ContentHtml,
                        MailLayouts::ContentCss,
                        MailLayouts::IsCustom,
                    ])
                    .to_owned(),
            )
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

async fn delete_layout(db: &Db, code: &str) -> CoreResult<()> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(MailLayouts::Table)
            .and_where(Expr::col(MailLayouts::Code).eq(code))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

fn partial_of(r: &sqlx::any::AnyRow) -> CoreResult<Partial> {
    Ok(Partial {
        code: r.get_text("code")?,
        name: r.get_text("name")?,
        content_text: r.get_text("content_text")?,
        content_html: r.get_text("content_html")?,
        is_custom: r.get_bool("is_custom")?,
    })
}

async fn list_partials(db: &Db) -> CoreResult<Vec<Partial>> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                MailPartials::Code,
                MailPartials::Name,
                MailPartials::ContentText,
                MailPartials::ContentHtml,
                MailPartials::IsCustom,
            ])
            .from(MailPartials::Table)
            .order_by(MailPartials::Code, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter().map(partial_of).collect()
}

async fn find_partial(db: &Db, code: &str) -> CoreResult<Option<Partial>> {
    Ok(list_partials(db)
        .await?
        .into_iter()
        .find(|p| p.code == code))
}

async fn upsert_partial(db: &Db, p: &Partial) -> CoreResult<()> {
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(MailPartials::Table)
            .columns([
                MailPartials::Code,
                MailPartials::Name,
                MailPartials::ContentText,
                MailPartials::ContentHtml,
                MailPartials::IsCustom,
            ])
            .values_panic([
                p.code.as_str().into(),
                p.name.as_str().into(),
                p.content_text.as_str().into(),
                p.content_html.as_str().into(),
                p.is_custom.into(),
            ])
            .on_conflict(
                OnConflict::column(MailPartials::Code)
                    .update_columns([
                        MailPartials::Name,
                        MailPartials::ContentText,
                        MailPartials::ContentHtml,
                        MailPartials::IsCustom,
                    ])
                    .to_owned(),
            )
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

async fn delete_partial(db: &Db, code: &str) -> CoreResult<()> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(MailPartials::Table)
            .and_where(Expr::col(MailPartials::Code).eq(code))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_splits_into_header_and_sections() {
        let s = parse_file("subject = \"Hi {{ name }}\"\nlayout = plain\n==\nplain {{ name }}\n==\n# Hi {{ name }}\n");
        assert_eq!(s.settings["subject"], "Hi {{ name }}");
        assert_eq!(s.settings["layout"], "plain");
        assert_eq!(s.text, "plain {{ name }}");
        assert_eq!(s.html, "# Hi {{ name }}");
        let two = parse_file("name = \"Button\"\n==\n<a>{{ body }}</a>");
        assert_eq!(two.settings["name"], "Button");
        assert_eq!(two.html, "<a>{{ body }}</a>");
        assert!(two.text.is_empty());
        let four = parse_file(
            "name = L\n==\n{{ content }}\n==\n<html>{{ content }}</html>\n==\nbody { color: red }",
        );
        assert_eq!(four.css, "body { color: red }");
    }

    #[test]
    fn brand_properties_become_a_map_and_text_loses_its_tags() {
        let map = brand_values(":root { --mail-button-bg: #c0392b; --mail-text-color: #222; }");
        assert_eq!(map["button_bg"], "#c0392b");
        assert_eq!(map["text_color"], "#222");
        assert_eq!(text_of("<p>Hi &amp; bye</p>"), "Hi & bye");
        assert_eq!(tidy_text("a\n\n\n\nb  \n"), "a\n\nb");
    }
}
